//! Job execution orchestrator with process group management.

pub mod cancel;
pub mod process;

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::traits::{Clock, JobStore, ReviewExecutor};
use crate::types::{Job, JobStatus};

/// Run the job execution loop.
///
/// Continuously polls for leased jobs and spawns each as an independent
/// tokio task with bounded concurrency via a semaphore.
pub async fn run<S, E, C>(
    store: Arc<S>,
    executor: Arc<E>,
    _clock: &C,
    config: &Config,
) -> Result<()>
where
    S: JobStore + 'static,
    E: ReviewExecutor + 'static,
    C: Clock,
{
    let semaphore = Arc::new(Semaphore::new(config.execution.max_concurrency));

    let base_repo = config
        .execution
        .base_repo_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory is accessible"));
    let worktree_root = config
        .execution
        .worktree_root
        .clone()
        .unwrap_or_else(|| base_repo.join(".worktrees"));

    loop {
        // Recover stale leases before polling for new work.
        recover_stale_leases(&*store);

        let permit =
            semaphore.clone().acquire_owned().await.map_err(|e| {
                crate::error::ReviewqError::Runner(format!("semaphore closed: {e}"))
            })?;

        let job = match store.lease_next() {
            Ok(Some(job)) => job,
            Ok(None) => {
                drop(permit);
                tokio::time::sleep(std::time::Duration::from_secs(
                    config.polling.interval_seconds,
                ))
                .await;
                continue;
            }
            Err(e) => {
                drop(permit);
                error!(error = %e, "failed to lease next job");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        info!(
            job_id = job.id,
            repo = %job.repo,
            pr = job.pr_number,
            "leased job for execution"
        );

        // Clone Arcs and paths for the spawned task.
        let store = Arc::clone(&store);
        let executor = Arc::clone(&executor);
        let base_repo = base_repo.clone();
        let worktree_root = worktree_root.clone();

        tokio::spawn(async move {
            execute_job(&*store, &*executor, job, &base_repo, &worktree_root).await;
            drop(permit);
        });
    }
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
