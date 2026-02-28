//! Low-level process group spawning via setpgid.

use std::path::Path;

use tokio::process::Command;

use crate::error::{Result, ReviewqError};

/// Spawn a review command in a new process group.
///
/// The child process is placed into its own process group via `setpgid(0, 0)`
/// so that the entire group can be signalled for cancellation.
///
/// Stdout and stderr are redirected to files at the given paths.
///
/// Returns the child handle and its PID.
pub async fn spawn_in_group(
    command: &str,
    workdir: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    env_vars: &[(String, String)],
) -> Result<(tokio::process::Child, u32)> {
    let stdout_file = std::fs::File::create(stdout_path).map_err(|e| {
        ReviewqError::Process(format!(
            "failed to create stdout file {}: {e}",
            stdout_path.display()
        ))
    })?;

    let stderr_file = std::fs::File::create(stderr_path).map_err(|e| {
        ReviewqError::Process(format!(
            "failed to create stderr file {}: {e}",
            stderr_path.display()
        ))
    })?;

    // SAFETY: setpgid(0, 0) is async-signal-safe and only affects the
    // forked child process before exec. This is the standard idiom for
    // creating a new process group.
    let child = unsafe {
        Command::new("sh")
            .args(["-c", command])
            .current_dir(workdir)
            .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(std::process::Stdio::null())
            .stdout(stdout_file)
            .stderr(stderr_file)
            .pre_exec(|| {
                nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))
                    .map_err(std::io::Error::other)?;
                Ok(())
            })
            .spawn()
            .map_err(|e| ReviewqError::Process(format!("failed to spawn command: {e}")))?
    };

    let pid = child
        .id()
        .ok_or_else(|| ReviewqError::Process("child exited before PID could be read".into()))?;

    Ok((child, pid))
}
