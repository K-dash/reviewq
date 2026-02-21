//! Job execution orchestrator with process group management.

pub mod cancel;
pub mod process;

use std::sync::Arc;

use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::traits::{Clock, JobStore, ReviewExecutor};
use crate::types::JobStatus;

/// Run the job execution loop.
///
/// Continuously polls for leased jobs and executes them with bounded
/// concurrency controlled by a semaphore. Each job is spawned as an
/// independent tokio task so that multiple reviews can run in parallel.
pub async fn run<S, E, C>(store: &S, executor: &E, _clock: &C, config: &Config) -> Result<()>
where
    S: JobStore + 'static,
    E: ReviewExecutor + 'static,
    C: Clock,
{
    let semaphore = Arc::new(Semaphore::new(config.execution.max_concurrency));

    loop {
        // Acquire a concurrency permit before leasing to avoid pulling
        // more jobs than we can run.
        let permit =
            semaphore.clone().acquire_owned().await.map_err(|e| {
                crate::error::ReviewqError::Runner(format!("semaphore closed: {e}"))
            })?;

        let job = match store.lease_next() {
            Ok(Some(job)) => job,
            Ok(None) => {
                // No work available; release the permit and back off.
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

        // Prepare worktree
        let base_repo =
            config.execution.base_repo_path.clone().unwrap_or_else(|| {
                std::env::current_dir().expect("current directory is accessible")
            });
        let worktree_root = config
            .execution
            .worktree_root
            .clone()
            .unwrap_or_else(|| base_repo.join(".worktrees"));

        let worktree_path =
            match crate::worktree::create(&base_repo, &worktree_root, job.id, &job.head_sha) {
                Ok(path) => path,
                Err(e) => {
                    error!(job_id = job.id, error = %e, "failed to create worktree");
                    let _ = store.complete(job.id, JobStatus::Failed, None);
                    drop(permit);
                    continue;
                }
            };

        // Execute the review
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

        // Clean up worktree
        if let Err(e) = crate::worktree::remove(&base_repo, &worktree_path) {
            warn!(job_id = job.id, error = %e, "failed to remove worktree after job");
        }

        drop(permit);
    }
}
