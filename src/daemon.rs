//! Daemon lifecycle: PID lock, signal handling, graceful shutdown.
//!
//! Provides single-instance enforcement via PID files and signal-based
//! shutdown / config-reload notification channels.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Notify, watch};
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
///
/// Returns true for `Ok` (signal delivered but not actually sent because
/// `sig == 0`) and for `EPERM` (process exists but we lack permission to
/// signal it). Any other error (most commonly `ESRCH`) means the process
/// is not alive.
pub(crate) fn is_process_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal;
    use nix::unistd::Pid;

    match signal::kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => true,
        // EPERM: the process exists but we cannot signal it. From a
        // liveness perspective the daemon is still running, so return
        // true — the caller's own permission errors will surface
        // separately when they try to actually signal the process.
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Daemon health check
// ---------------------------------------------------------------------------

/// Observed daemon liveness, derived from the pidfile at
/// `<logging_dir>/reviewq.pid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonHealth {
    /// The pidfile exists, its contents parse as a valid PID, and
    /// `kill(pid, 0)` reports the process as reachable (including the
    /// `EPERM` case — the daemon is running, we just lack permission
    /// to signal it, which `nudge_daemon` will surface separately).
    Alive(u32),
    /// The pidfile is missing, its contents cannot be parsed, or the
    /// referenced PID is not alive. From the caller's perspective the
    /// daemon is not running.
    Dead,
}

/// Read the pidfile under `logging_dir` and return the observed daemon
/// liveness. This is a cheap call (one stat + one `kill(pid, 0)`,
/// μs-range total) and is safe to invoke on every TUI frame.
///
/// PID validation is deliberately strict:
/// - The pidfile is parsed as an `i32`, not a `u32`, so values that
///   overflow `i32::MAX` fail the parse and map to `Dead` instead of
///   wrapping into a negative PID.
/// - `pid <= 0` is rejected as `Dead`. POSIX `kill(0, 0)` targets the
///   caller's process group and `kill(-N, 0)` targets the group with
///   ID `N`, so a naive `u32 -> i32` cast of a zero or over-large
///   pidfile value could make `is_process_alive` report a bogus
///   `Alive` for a pid the daemon cannot possibly own. Explicitly
///   rejecting `pid <= 0` here keeps `daemon_health` and
///   `nudge_daemon` in lock-step — both refuse the same invalid
///   pidfile states — so the TUI guard layers cannot leak a
///   fail-open path.
pub fn daemon_health(logging_dir: &Path) -> DaemonHealth {
    let path = logging_dir.join("reviewq.pid");
    let Ok(contents) = fs::read_to_string(&path) else {
        return DaemonHealth::Dead;
    };
    let Ok(pid_i32) = contents.trim().parse::<i32>() else {
        return DaemonHealth::Dead;
    };
    if pid_i32 <= 0 {
        return DaemonHealth::Dead;
    }
    // Safe cast: pid_i32 is strictly positive here, so it fits u32.
    let pid = pid_i32 as u32;
    if is_process_alive(pid) {
        DaemonHealth::Alive(pid)
    } else {
        DaemonHealth::Dead
    }
}

// ---------------------------------------------------------------------------
// Signal handling
// ---------------------------------------------------------------------------

/// Set up Unix signal handlers for graceful lifecycle management.
///
/// - **SIGINT / SIGTERM** trigger a shutdown notification.
/// - **SIGHUP** triggers a config-reload notification.
/// - **SIGUSR1** wakes the runner loop to check for queued jobs immediately.
///
/// Returns `(shutdown_rx, reload_rx, wake_notify)`.
pub async fn setup_signals() -> Result<(watch::Receiver<bool>, watch::Receiver<bool>, Arc<Notify>)>
{
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (reload_tx, reload_rx) = watch::channel(false);
    let wake_notify = Arc::new(Notify::new());

    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| ReviewqError::Process(format!("failed to register SIGINT handler: {e}")))?;
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| ReviewqError::Process(format!("failed to register SIGTERM handler: {e}")))?;
    let mut sighup = signal(SignalKind::hangup())
        .map_err(|e| ReviewqError::Process(format!("failed to register SIGHUP handler: {e}")))?;
    let mut sigusr1 = signal(SignalKind::user_defined1())
        .map_err(|e| ReviewqError::Process(format!("failed to register SIGUSR1 handler: {e}")))?;

    let wake = Arc::clone(&wake_notify);
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
                _ = sigusr1.recv() => {
                    info!("received SIGUSR1, waking runner");
                    wake.notify_one();
                }
            }
        }
    });

    Ok((shutdown_rx, reload_rx, wake_notify))
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

    // ---------------------------------------------------------------
    // daemon_health / DaemonHealth
    //
    // Regression tests for the "TUI fires cancel/retry into the void"
    // bug where queued mutations got applied to the DB even though the
    // daemon had been stopped. The health check below is the first
    // layer of defense the TUI uses to gate write actions.
    // ---------------------------------------------------------------

    #[test]
    fn daemon_health_returns_alive_for_live_pid() {
        let dir = TempDir::new().expect("temp dir");
        // PidFile::acquire writes the current process's PID, which is
        // alive by construction.
        let _pid_file = PidFile::acquire(dir.path()).expect("acquire");
        let health = daemon_health(dir.path());
        match health {
            DaemonHealth::Alive(pid) => assert_eq!(pid, std::process::id()),
            DaemonHealth::Dead => panic!("expected Alive, got Dead"),
        }
    }

    #[test]
    fn daemon_health_returns_dead_when_pidfile_missing() {
        let dir = TempDir::new().expect("temp dir");
        assert!(matches!(daemon_health(dir.path()), DaemonHealth::Dead));
    }

    #[test]
    fn daemon_health_returns_dead_when_pidfile_unparsable() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("reviewq.pid"), "not-a-pid").expect("write bogus pidfile");
        assert!(matches!(daemon_health(dir.path()), DaemonHealth::Dead));
    }

    #[test]
    fn daemon_health_returns_dead_when_pidfile_points_at_stale_pid() {
        let dir = TempDir::new().expect("temp dir");
        // Same well-above-typical PID used by is_process_alive_nonexistent.
        fs::write(dir.path().join("reviewq.pid"), "4000000").expect("write stale pidfile");
        assert!(matches!(daemon_health(dir.path()), DaemonHealth::Dead));
    }

    #[test]
    fn daemon_health_returns_dead_when_pidfile_contains_zero() {
        // Regression: a naive `u32 -> i32` cast of "0" would call
        // `kill(0, 0)`, which POSIX interprets as targeting the
        // caller's process group and reports success. Without the
        // explicit `pid <= 0` reject in daemon_health, that would
        // flip the TUI guard into fail-open and re-open the silent
        // corruption window this feature is supposed to close.
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("reviewq.pid"), "0").expect("write zero pidfile");
        assert!(matches!(daemon_health(dir.path()), DaemonHealth::Dead));
    }

    #[test]
    fn daemon_health_returns_dead_when_pidfile_contains_negative_pid() {
        // `-1` parses as i32, but `pid <= 0` must reject it before it
        // reaches `is_process_alive`. A raw `kill(-1, 0)` would target
        // every process the caller owns, which must never count as
        // "daemon alive".
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("reviewq.pid"), "-1").expect("write negative pidfile");
        assert!(matches!(daemon_health(dir.path()), DaemonHealth::Dead));
    }

    #[test]
    fn daemon_health_returns_dead_when_pidfile_overflows_i32() {
        // u32::MAX is outside i32 range, so the parse must fail and
        // the result must be `Dead` rather than wrapping into a
        // negative PID.
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("reviewq.pid"), "9999999999").expect("write overflow pidfile");
        assert!(matches!(daemon_health(dir.path()), DaemonHealth::Dead));
    }
}
