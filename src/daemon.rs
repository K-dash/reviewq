//! Daemon lifecycle: PID lock, signal handling, graceful shutdown.
//!
//! Provides single-instance enforcement via PID files and signal-based
//! shutdown / config-reload notification channels.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tracing::info;

use crate::error::{Result, ReviewqError};

// ---------------------------------------------------------------------------
// PID file management
// ---------------------------------------------------------------------------

/// PID file manager for single-instance enforcement.
///
/// On creation the file is written with the current process ID. If a live
/// process already owns the PID file, creation fails with an error. The
/// file is automatically removed when the `PidFile` is dropped.
#[derive(Debug)]
pub struct PidFile {
    path: PathBuf,
}

impl PidFile {
    /// Acquire the PID file at `dir/reviewq.pid`.
    ///
    /// Returns an error if another reviewq instance is already running
    /// (detected by checking whether the recorded PID is alive).
    pub fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join("reviewq.pid");

        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                ReviewqError::Process(format!("failed to create PID directory: {e}"))
            })?;
        }

        // Check for an existing PID file before attempting exclusive create.
        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path)
                && let Ok(pid) = contents.trim().parse::<u32>()
                && is_process_alive(pid)
            {
                return Err(ReviewqError::Process(format!(
                    "another reviewq instance is running (PID {pid})"
                )));
            }
            // Stale PID file — remove it before writing a new one.
            let _ = fs::remove_file(&path);
        }

        // Atomic create: O_CREAT | O_EXCL prevents TOCTOU between the
        // stale-check above and writing the new PID.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    ReviewqError::Process(
                        "another reviewq instance acquired the PID file concurrently".into(),
                    )
                } else {
                    ReviewqError::Process(format!("failed to create PID file: {e}"))
                }
            })?;

        file.write_all(std::process::id().to_string().as_bytes())
            .map_err(|e| ReviewqError::Process(format!("failed to write PID file: {e}")))?;

        Ok(Self { path })
    }

    /// Remove the PID file.
    pub fn release(&self) {
        let _ = fs::remove_file(&self.path);
    }

    /// Return the path to the PID file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        self.release();
    }
}

/// Check whether a process with the given PID is alive using `kill(pid, 0)`.
fn is_process_alive(pid: u32) -> bool {
    use nix::sys::signal;
    use nix::unistd::Pid;

    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

// ---------------------------------------------------------------------------
// Signal handling
// ---------------------------------------------------------------------------

/// Set up Unix signal handlers for graceful lifecycle management.
///
/// - **SIGINT / SIGTERM** trigger a shutdown notification.
/// - **SIGHUP** triggers a config-reload notification.
///
/// Returns `(shutdown_rx, reload_rx)` — watch receivers that flip to `true`
/// when the corresponding event fires.
pub async fn setup_signals() -> Result<(watch::Receiver<bool>, watch::Receiver<bool>)> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (reload_tx, reload_rx) = watch::channel(false);

    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| ReviewqError::Process(format!("failed to register SIGINT handler: {e}")))?;
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| ReviewqError::Process(format!("failed to register SIGTERM handler: {e}")))?;
    let mut sighup = signal(SignalKind::hangup())
        .map_err(|e| ReviewqError::Process(format!("failed to register SIGHUP handler: {e}")))?;

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    info!("received SIGINT, initiating shutdown");
                    let _ = shutdown_tx.send(true);
                    break;
                }
                _ = sigterm.recv() => {
                    info!("received SIGTERM, initiating shutdown");
                    let _ = shutdown_tx.send(true);
                    break;
                }
                _ = sighup.recv() => {
                    info!("received SIGHUP, requesting config reload");
                    let _ = reload_tx.send(true);
                }
            }
        }
    });

    Ok((shutdown_rx, reload_rx))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquire_and_release_pid_file() {
        let dir = TempDir::new().expect("temp dir");
        let pid_file = PidFile::acquire(dir.path()).expect("acquire should succeed");

        let contents = fs::read_to_string(pid_file.path()).expect("read PID file");
        assert_eq!(
            contents.trim().parse::<u32>().expect("parse PID"),
            std::process::id()
        );

        pid_file.release();
        assert!(
            !pid_file.path().exists(),
            "PID file should be removed after release"
        );
    }

    #[test]
    fn acquire_detects_running_instance() {
        let dir = TempDir::new().expect("temp dir");

        // First acquisition should succeed
        let _pid_file = PidFile::acquire(dir.path()).expect("first acquire");

        // Second acquisition should fail (our own PID is alive)
        let result = PidFile::acquire(dir.path());
        assert!(result.is_err(), "should detect running instance");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("another reviewq instance is running"),
            "error message should mention running instance, got: {err_msg}"
        );
    }

    #[test]
    fn acquire_cleans_stale_pid_file() {
        let dir = TempDir::new().expect("temp dir");
        let pid_path = dir.path().join("reviewq.pid");

        // Write a fake PID that definitely does not exist
        // PID 4_000_000 is well above typical PID ranges
        fs::write(&pid_path, "4000000").expect("write stale PID");

        // Should succeed because the stale process is not alive
        let pid_file = PidFile::acquire(dir.path()).expect("should clean stale PID");
        let contents = fs::read_to_string(pid_file.path()).expect("read PID file");
        assert_eq!(
            contents.trim().parse::<u32>().expect("parse PID"),
            std::process::id()
        );
    }

    #[test]
    fn drop_removes_pid_file() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("reviewq.pid");

        {
            let _pid_file = PidFile::acquire(dir.path()).expect("acquire");
            assert!(path.exists(), "PID file should exist while held");
        }
        // PidFile has been dropped
        assert!(!path.exists(), "PID file should be removed on drop");
    }

    #[test]
    fn is_process_alive_current() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn is_process_alive_nonexistent() {
        // PID well above typical range
        assert!(!is_process_alive(4_000_000));
    }
}
