//! Job execution orchestrator with process group management.

pub mod cancel;
pub mod process;

use std::path::Path;
use std::sync::Arc;

use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::traits::{Clock, JobStore, ReviewExecutor};
use crate::types::{Job, JobStatus};

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
    config: &Config,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()>
where
    S: JobStore + 'static,
    E: ReviewExecutor + 'static,
    C: Clock,
{
    let semaphore = Arc::new(Semaphore::new(config.execution.max_concurrency));
    let mut job_tasks: JoinSet<()> = JoinSet::new();

    let global_base_repo = config
        .execution
        .base_repo_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory is accessible"));
    let worktree_root = config
        .execution
        .worktree_root
        .clone()
        .unwrap_or_else(|| global_base_repo.join(".worktrees"));
    let policies = config.repo_policies();

    loop {
        // Drain completed tasks so JoinSet doesn't grow unboundedly.
        while job_tasks.try_join_next().is_some() {}

        // Check for shutdown before leasing new work.
        if *shutdown_rx.borrow() {
            break;
        }

        // Recover stale leases before polling for new work.
        recover_stale_leases(&*store);

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
                // Wait for either shutdown or poll interval.
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(
                        config.polling.interval_seconds,
                    )) => {}
                    _ = shutdown_rx.changed() => {}
                }
                continue;
            }
            Err(e) => {
                drop(permit);
                error!(error = %e, "failed to lease next job");
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    _ = shutdown_rx.changed() => {}
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
    // Transition leased → running so stale lease recovery won't re-queue us.
    if let Err(e) = store.mark_running(job.id, std::process::id()) {
        error!(job_id = job.id, error = %e, "failed to mark job as running");
        let _ = store.complete(job.id, JobStatus::Failed, None);
        return;
    }

    let worktree_path =
        match crate::worktree::create(base_repo, worktree_root, job.id, &job.head_sha) {
            Ok(path) => path,
            Err(e) => {
                error!(job_id = job.id, error = %e, "failed to create worktree");
                let _ = store.complete(job.id, JobStatus::Failed, None);
                return;
            }
        };

    match executor.execute(&job, &worktree_path).await {
        Ok(result) => {
            let status = if result.exit_code == 0 {
                JobStatus::Succeeded
            } else {
                JobStatus::Failed
            };

            if let Some(ref markdown) = result.review_markdown {
                let _ = store.store_review_output(job.id, markdown);
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
            warn!(job_id = job.id, error = %e, "review execution failed");
            let _ = store.complete(job.id, JobStatus::Failed, None);
        }
    }

    if let Err(e) = crate::worktree::remove(base_repo, &worktree_path) {
        warn!(job_id = job.id, error = %e, "failed to remove worktree after job");
    }
}

/// Re-queue jobs whose leases have expired (crash recovery).
fn recover_stale_leases<S: JobStore>(store: &S) {
    let stale = match store.find_stale_leases() {
        Ok(jobs) => jobs,
        Err(e) => {
            warn!(error = %e, "failed to query stale leases");
            return;
        }
    };

    for job in stale {
        if job.retry_count >= job.max_retries {
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
}
