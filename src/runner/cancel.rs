//! Staged cancellation state machine: SIGINT -> SIGTERM -> SIGKILL.

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::config::CancelConfig;
use crate::error::Result;

/// Cancel a process group with staged signals.
///
/// Escalation sequence:
/// 1. Send `SIGINT` to the process group, wait `sigint_timeout`.
/// 2. If still alive, send `SIGTERM`, wait `sigterm_timeout`.
/// 3. If still alive, send `SIGKILL`, wait `sigkill_timeout`.
///
/// Signals are sent to the negative PID so they reach the entire process group.
pub async fn cancel_process_group(pid: u32, config: &CancelConfig) -> Result<()> {
    let pgid = Pid::from_raw(-(pid as i32));

    // Stage 1: SIGINT
    info!(pid, "sending SIGINT to process group");
    let _ = signal::kill(pgid, Signal::SIGINT);
    sleep(Duration::from_secs(config.sigint_timeout_seconds)).await;
    if !is_alive(pid) {
        info!(pid, "process exited after SIGINT");
        return Ok(());
    }

    // Stage 2: SIGTERM
    warn!(pid, "process still alive, sending SIGTERM");
    let _ = signal::kill(pgid, Signal::SIGTERM);
    sleep(Duration::from_secs(config.sigterm_timeout_seconds)).await;
    if !is_alive(pid) {
        info!(pid, "process exited after SIGTERM");
        return Ok(());
    }

    // Stage 3: SIGKILL
    warn!(pid, "process still alive, sending SIGKILL");
    let _ = signal::kill(pgid, Signal::SIGKILL);
    sleep(Duration::from_secs(config.sigkill_timeout_seconds)).await;
    if !is_alive(pid) {
        info!(pid, "process exited after SIGKILL");
    } else {
        warn!(pid, "process still alive after SIGKILL");
    }

    Ok(())
}

/// Check if a process is still alive by sending signal 0.
fn is_alive(pid: u32) -> bool {
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}
