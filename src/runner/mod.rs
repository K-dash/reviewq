//! Job execution orchestrator with process group management.

pub mod cancel;
pub mod process;

use std::path::Path;
use std::sync::Arc;

use nix::sys::signal;
use nix::unistd::Pid;
use tokio::sync::{Notify, Semaphore, oneshot, watch};
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::traits::{Clock, JobStore, ReviewExecutor};
use crate::types::{Job, JobFilter, JobStatus};

/// Run the job execution loop.
///
/// Continuously polls for leased jobs and spawns each as a tracked tokio
/// task (via [`JoinSet`]) with bounded concurrency via a semaphore.
///
/// When `shutdown_rx` fires, the loop stops accepting new jobs and waits
/// for all in-flight jobs to complete before returning.
pub async fn run<S, E, C>(
    store: Arc<S>,
    executor: Arc<E>,
    _clock: &C,
    mut config_rx: watch::Receiver<Arc<Config>>,
    mut shutdown_rx: watch::Receiver<bool>,
    wake: Arc<Notify>,
) -> Result<()>
where
    S: JobStore + 'static,
    E: ReviewExecutor + 'static,
    C: Clock,
{
    // Semaphore is created once from the initial config; changing
    // max_concurrency requires a restart.
    let initial_config = config_rx.borrow().clone();
    let semaphore = Arc::new(Semaphore::new(initial_config.execution.max_concurrency));
    let mut job_tasks: JoinSet<()> = JoinSet::new();

    loop {
        // Re-read config at each iteration so hot-reloaded values take effect.
        let config = config_rx.borrow_and_update().clone();
        let global_base_repo =
            config.execution.base_repo_path.clone().unwrap_or_else(|| {
                std::env::current_dir().expect("current directory is accessible")
            });
        let worktree_root = config.execution.effective_worktree_root();
        let policies = config.repo_policies();

        // Drain completed tasks so JoinSet doesn't grow unboundedly.
        while job_tasks.try_join_next().is_some() {}

        // Check for shutdown before leasing new work.
        if *shutdown_rx.borrow() {
            break;
        }

        // Recover stale leases before polling for new work.
        recover_stale_jobs(&*store);

        // Sweep queued jobs with pending cancel requests.
        match store.cancel_queued_requested() {
            Ok(ids) => {
                for id in &ids {
                    info!(
                        job_id = id,
                        "canceled queued job with pending cancel request"
                    );
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to sweep cancel-requested queued jobs");
            }
        }

        // Acquire concurrency permit, but also listen for shutdown so we
        // don't block here indefinitely when a shutdown is requested.
        let permit = tokio::select! {
            result = semaphore.clone().acquire_owned() => {
                result.map_err(|e| {
                    crate::error::ReviewqError::Runner(format!("semaphore closed: {e}"))
                })?
            }
            _ = shutdown_rx.changed() => {
                break;
            }
        };

        let job = match store.lease_next() {
            Ok(Some(job)) => job,
            Ok(None) => {
                drop(permit);
                // Wait for either shutdown, wake signal, or poll interval.
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(
                        config.polling.interval_seconds,
                    )) => {}
                    _ = shutdown_rx.changed() => {}
                    _ = wake.notified() => {
                        info!("wake signal received, checking for queued jobs");
                    }
                }
                continue;
            }
            Err(e) => {
                drop(permit);
                error!(error = %e, "failed to lease next job");
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    _ = shutdown_rx.changed() => {}
                    _ = wake.notified() => {
                        info!("wake signal received, retrying after error");
                    }
                }
                continue;
            }
        };

        info!(
            job_id = job.id,
            repo = %job.repo,
            pr = job.pr_number,
            "leased job for execution"
        );

        // Resolve per-repo base path, falling back to global.
        let base_repo = policies
            .iter()
            .find(|p| p.id == job.repo)
            .and_then(|p| p.base_repo_path.clone())
            .unwrap_or_else(|| global_base_repo.clone());

        // Clone Arcs and paths for the spawned task.
        let store = Arc::clone(&store);
        let executor = Arc::clone(&executor);
        let worktree_root = worktree_root.clone();

        job_tasks.spawn(async move {
            execute_job(&*store, &*executor, job, &base_repo, &worktree_root).await;
            drop(permit);
        });
    }

    // Graceful shutdown: wait for all in-flight jobs to finish.
    info!(
        in_flight = job_tasks.len(),
        "waiting for in-flight jobs to complete"
    );
    while job_tasks.join_next().await.is_some() {}
    info!("all in-flight jobs completed");

    Ok(())
}

/// Execute a single job: mark running → create worktree → run review → complete → cleanup.
async fn execute_job<S: JobStore, E: ReviewExecutor>(
    store: &S,
    executor: &E,
    job: Job,
    base_repo: &Path,
    worktree_root: &Path,
) {
    let worktree_path =
        match crate::worktree::create(base_repo, worktree_root, job.id, &job.head_sha) {
            Ok(path) => path,
            Err(e) => {
                error!(job_id = job.id, error = %e, "failed to create worktree");
                let _ = store.complete(job.id, JobStatus::Failed, None);
                return;
            }
        };

    if let Err(e) = store.store_worktree_path(job.id, &worktree_path) {
        warn!(job_id = job.id, error = %e, "failed to store worktree_path");
    }

    // Check cancel before even starting execution.
    if store.is_cancel_requested(job.id).unwrap_or(false) {
        info!(
            job_id = job.id,
            "cancel requested before execution, marking canceled"
        );
        let _ = store.complete(job.id, JobStatus::Canceled, None);
        return;
    }

    let (pid_tx, mut pid_rx) = oneshot::channel();
    let mut execution = std::pin::pin!(executor.execute(&job, &worktree_path, Some(pid_tx)));
    let mut running_marked = false;
    let mut cancel_sent = false;

    let execution_result = loop {
        tokio::select! {
            pid = &mut pid_rx, if !running_marked => {
                match pid {
                    Ok(pid) => {
                        // Check cancel before transitioning to running.
                        if store.is_cancel_requested(job.id).unwrap_or(false) {
                            info!(job_id = job.id, pid, "cancel requested during leased window");
                            let _ = executor.cancel(&job).await;
                            let _ = executor.clear_active_pid(job.id);
                            let _ = store.complete(job.id, JobStatus::Canceled, None);
                            return;
                        }
                        if let Err(e) = store.mark_running(job.id, pid) {
                            error!(job_id = job.id, pid, error = %e, "failed to mark job as running");
                            let _ = executor.cancel(&job).await;
                            let _ = executor.clear_active_pid(job.id);
                            let _ = store.complete(job.id, JobStatus::Failed, None);
                            return;
                        }
                    }
                    Err(_) => {
                        warn!(job_id = job.id, "review process exited before reporting PID");
                    }
                }
                running_marked = true;
            }
            // Poll for cancel requests every 2 seconds while running.
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)), if running_marked && !cancel_sent => {
                if store.is_cancel_requested(job.id).unwrap_or(false) {
                    info!(job_id = job.id, "cancel requested, killing review process");
                    let _ = executor.cancel(&job).await;
                    cancel_sent = true;
                    // Don't break — let execution future complete naturally after kill.
                }
            }
            result = &mut execution => break result,
        }
    };

    if !running_marked {
        match pid_rx.try_recv() {
            Ok(pid) => {
                // Check cancel before transitioning to running.
                if store.is_cancel_requested(job.id).unwrap_or(false) {
                    info!(
                        job_id = job.id,
                        pid, "cancel requested during leased window (post-exec)"
                    );
                    let _ = executor.clear_active_pid(job.id);
                    let _ = store.complete(job.id, JobStatus::Canceled, None);
                    return;
                }
                if let Err(e) = store.mark_running(job.id, pid) {
                    error!(job_id = job.id, pid, error = %e, "failed to mark job as running");
                    let _ = executor.cancel(&job).await;
                    let _ = executor.clear_active_pid(job.id);
                    let _ = store.complete(job.id, JobStatus::Failed, None);
                    return;
                }
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                unreachable!("executor finished, so PID channel must be sent or closed")
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
        }
    }

    // If cancel was requested, final status is Canceled regardless of exit code.
    let was_cancel_requested = cancel_sent || store.is_cancel_requested(job.id).unwrap_or(false);

    match execution_result {
        Ok(result) => {
            // Persist log file paths so TUI and CLI tail can find them.
            if let (Some(stdout), Some(stderr)) = (&result.stdout_path, &result.stderr_path) {
                let _ = store.store_log_paths(job.id, stdout, stderr);
            }

            let status = if was_cancel_requested {
                JobStatus::Canceled
            } else if result.exit_code == 0 {
                JobStatus::Succeeded
            } else {
                JobStatus::Failed
            };

            if let Some(ref markdown) = result.review_markdown {
                let _ = store.store_review_output(job.id, markdown);
            }
            if let Some(ref sid) = result.session_id
                && let Err(e) = store.store_session_id(job.id, sid)
            {
                warn!(job_id = job.id, error = %e, "failed to store session_id");
            }

            if let Err(e) = store.complete(job.id, status, Some(result.exit_code)) {
                error!(job_id = job.id, error = %e, "failed to mark job complete");
            } else {
                info!(
                    job_id = job.id,
                    status = %status,
                    exit_code = result.exit_code,
                    "job completed"
                );
            }
        }
        Err(e) => {
            let status = if was_cancel_requested {
                JobStatus::Canceled
            } else {
                JobStatus::Failed
            };
            warn!(job_id = job.id, error = %e, %status, "review execution failed");
            let _ = store.complete(job.id, status, None);
        }
    }

    // Worktree is NOT removed here — the TTL-based cleanup loop handles
    // expiration.  Keeping it around lets users resume agent sessions
    // (e.g. `claude --resume <sid>`) which are tied to the worktree cwd.
}

/// Re-queue abandoned jobs after daemon crash recovery.
fn recover_stale_jobs<S: JobStore>(store: &S) {
    let stale = match store.find_stale_leases() {
        Ok(jobs) => jobs,
        Err(e) => {
            warn!(error = %e, "failed to query stale leases");
            return;
        }
    };

    for job in stale {
        if job.is_cancel_requested() {
            info!(
                job_id = job.id,
                "stale lease with cancel request, marking canceled"
            );
            let _ = store.complete(job.id, JobStatus::Canceled, None);
        } else if job.retry_count >= job.max_retries {
            warn!(
                job_id = job.id,
                "stale lease exceeded max retries, marking failed"
            );
            let _ = store.complete(job.id, JobStatus::Failed, None);
        } else {
            info!(
                job_id = job.id,
                retry = job.retry_count + 1,
                "re-queuing stale lease"
            );
            let _ = store.requeue_stale(job.id);
        }
    }

    let running = match store.list_jobs(&JobFilter {
        status: Some(JobStatus::Running),
        ..Default::default()
    }) {
        Ok(jobs) => jobs,
        Err(e) => {
            warn!(error = %e, "failed to query running jobs for recovery");
            return;
        }
    };

    for job in running {
        let Some(pid) = job.pid else {
            if job.is_cancel_requested() {
                info!(
                    job_id = job.id,
                    "orphaned running job (no PID) with cancel request, marking canceled"
                );
                let _ = store.complete(job.id, JobStatus::Canceled, None);
            } else {
                warn!(
                    job_id = job.id,
                    "running job has no recorded PID, re-queuing"
                );
                let _ = store.requeue_running(job.id);
            }
            continue;
        };

        if job.is_cancel_requested() {
            if is_process_alive(pid) {
                info!(
                    job_id = job.id,
                    pid, "cancel requested for running job with live PID, killing process group"
                );
                kill_process_group(pid);
                // Re-check after kill — if still alive, leave for next recovery cycle.
                if is_process_alive(pid) {
                    warn!(
                        job_id = job.id,
                        pid, "process still alive after SIGKILL, deferring cancel"
                    );
                    continue;
                }
            }
            let _ = store.complete(job.id, JobStatus::Canceled, None);
            continue;
        }

        if is_process_alive(pid) {
            continue;
        }

        if job.retry_count >= job.max_retries {
            warn!(
                job_id = job.id,
                pid, "orphaned running job exceeded max retries, marking failed"
            );
            let _ = store.complete(job.id, JobStatus::Failed, None);
        } else {
            warn!(
                job_id = job.id,
                pid,
                retry = job.retry_count + 1,
                "re-queuing orphaned running job"
            );
            let _ = store.requeue_running(job.id);
        }
    }
}

fn is_process_alive(pid: u32) -> bool {
    let Ok(raw_pid) = i32::try_from(pid) else {
        return false;
    };
    signal::kill(Pid::from_raw(raw_pid), None).is_ok()
}

/// Send SIGKILL to a process group during crash recovery.
///
/// Unlike the staged `cancel::cancel_process_group`, this is a synchronous
/// best-effort kill used only in `recover_stale_jobs` where we don't have
/// an async context or CancelConfig.
fn kill_process_group(pid: u32) {
    let Ok(raw_pid) = i32::try_from(pid) else {
        return;
    };
    let pgid = Pid::from_raw(-raw_pid);
    if let Err(e) = signal::kill(pgid, signal::Signal::SIGKILL) {
        warn!(pid, error = %e, "failed to SIGKILL process group during recovery");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::traits::JobStore;
    use crate::types::{AgentKind, NewJob, RepoId};

    fn sample_job() -> NewJob {
        NewJob {
            repo: RepoId::new("owner", "repo"),
            pr_number: 42,
            head_sha: "abc123".into(),
            agent_kind: AgentKind::Claude,
            command: Some("echo review".into()),
            prompt_template: None,
            max_retries: 3,
        }
    }

    #[test]
    fn recover_stale_jobs_requeues_orphaned_running_job() {
        let db = Database::open_in_memory().expect("db");
        let job = db.enqueue(sample_job()).expect("enqueue");
        let leased = db.lease_next().expect("lease").expect("has job");
        db.mark_running(leased.id, i32::MAX as u32)
            .expect("mark running");

        recover_stale_jobs(&db);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        let recovered = jobs.iter().find(|j| j.id == job.id).expect("find job");
        assert_eq!(recovered.status, JobStatus::Queued);
        assert_eq!(recovered.retry_count, 1);
        assert!(recovered.pid.is_none());
    }
}
