//! External boundary traits for testability and future extensibility.
//!
//! Only these 4 traits are introduced as abstractions. All internal logic
//! uses concrete types. Static dispatch via generics — no `dyn Trait`,
//! no `async-trait` crate (native `async fn` in traits, stable since 1.75).

use std::path::Path;

use chrono::{DateTime, Utc};
use tokio::sync::oneshot;

use crate::error::Result;
use crate::types::{
    AgentKind, IdempotencyKey, Job, JobFilter, JobStatus, NewJob, PullRequest, RepoId, ReviewResult,
};

// ---------------------------------------------------------------------------
// GitHubClient — abstracts GitHub API interactions
// ---------------------------------------------------------------------------

/// GitHub API operations (mockable for tests).
pub trait GitHubClient: Send + Sync {
    /// Search for PRs where the authenticated user is a requested reviewer.
    fn search_review_requested(
        &self,
        repos: &[RepoId],
    ) -> impl std::future::Future<Output = Result<Vec<PullRequest>>> + Send;

    /// List all open PRs for a specific repository (no reviewer filter).
    ///
    /// Used for repos with `skip_reviewer_check: true` where the GitHub
    /// Search API's `review-requested:{user}` filter would be too restrictive.
    fn list_open_prs(
        &self,
        repo: &RepoId,
    ) -> impl std::future::Future<Output = Result<Vec<PullRequest>>> + Send;

    /// Get the requested reviewers for a specific PR (the Source of Truth).
    fn requested_reviewers(
        &self,
        repo: &RepoId,
        pr_number: u64,
    ) -> impl std::future::Future<Output = Result<Vec<String>>> + Send;

    /// Get the authenticated user's login name.
    fn authenticated_user(&self) -> impl std::future::Future<Output = Result<String>> + Send;
}

// ---------------------------------------------------------------------------
// JobStore — abstracts job persistence
// ---------------------------------------------------------------------------

/// Job persistence (SQLite implementation, mockable for tests).
///
/// All methods are synchronous because SQLite operations are blocking
/// and should be called from a blocking context (e.g., `spawn_blocking`).
pub trait JobStore: Send + Sync {
    /// Insert a new job in `queued` status.
    fn enqueue(&self, job: NewJob) -> Result<Job>;

    /// Atomically lease the next queued job (FIFO).
    ///
    /// Uses a single `UPDATE ... WHERE id = (SELECT ...) RETURNING *`
    /// statement for atomic acquisition without TOCTOU races.
    fn lease_next(&self) -> Result<Option<Job>>;

    /// Mark a job as completed (succeeded or failed).
    fn complete(&self, id: i64, status: JobStatus, exit_code: Option<i32>) -> Result<()>;

    /// Request cancellation of a job (sets `cancel_requested_at`).
    ///
    /// Does NOT change the job's status — the runner is responsible for
    /// transitioning to `Canceled` after killing the process.
    fn request_cancel(&self, id: i64) -> Result<()>;

    /// Reset a failed or canceled job back to queued for retry.
    ///
    /// Resets `retry_count` to 0 (manual retry is distinct from automatic
    /// stale/orphan recovery) and NULLs out all execution-related columns
    /// so the job runs cleanly. `created_at` is preserved so retried jobs
    /// get priority in the queue.
    fn retry_job(&self, id: i64) -> Result<()>;

    /// Check if a cancel has been requested for the given job.
    fn is_cancel_requested(&self, id: i64) -> Result<bool>;

    /// Sweep queued jobs that have a pending cancel request → mark them canceled.
    ///
    /// Returns the IDs of jobs that were canceled.
    fn cancel_queued_requested(&self) -> Result<Vec<i64>>;

    /// Check if a job with the given idempotency key has been processed.
    fn is_processed(&self, key: &IdempotencyKey) -> Result<bool>;

    /// List jobs matching the given filter.
    fn list_jobs(&self, filter: &JobFilter) -> Result<Vec<Job>>;

    /// Find jobs whose leases have expired (for crash recovery).
    fn find_stale_leases(&self) -> Result<Vec<Job>>;

    /// Update a job's status to running and record its PID.
    fn mark_running(&self, id: i64, pid: u32) -> Result<()>;

    /// Store review output for a completed job.
    fn store_review_output(&self, id: i64, markdown: &str) -> Result<()>;

    /// Store the session ID extracted from agent output.
    fn store_session_id(&self, id: i64, session_id: &str) -> Result<()>;

    /// Store the worktree path for a running job.
    fn store_worktree_path(&self, id: i64, path: &Path) -> Result<()>;

    /// Re-queue a stale leased job for retry (increment retry_count, reset to queued).
    fn requeue_stale(&self, id: i64) -> Result<()>;

    /// Re-queue an orphaned running job after daemon crash recovery.
    ///
    /// Intended for jobs that were marked `running` by a previous daemon
    /// instance whose PID is no longer alive.
    fn requeue_running(&self, id: i64) -> Result<()>;

    /// Store the stdout/stderr log file paths for a job.
    fn store_log_paths(&self, id: i64, stdout: &Path, stderr: &Path) -> Result<()>;

    /// Check if a PR has already been reviewed (any SHA) by the given agent.
    ///
    /// Returns true when a non-failed, non-canceled job exists for the
    /// (repo, pr_number, agent) triple, regardless of head SHA.
    fn is_pr_reviewed(&self, repo: &RepoId, pr_number: u64, agent: &AgentKind) -> Result<bool>;

    /// Return `(repo, worktree_path)` pairs for terminal jobs whose
    /// `worktree_path` is set and whose `updated_at` is at least
    /// `ttl_minutes` old. Excludes jobs still `queued` / `leased` /
    /// `running`.
    ///
    /// Used by the worktree cleanup loop to resolve each expired
    /// worktree back to its owning repo so `git worktree remove` can
    /// be called against the correct base clone.
    fn expired_terminal_worktrees(
        &self,
        ttl_minutes: u64,
    ) -> Result<Vec<(RepoId, std::path::PathBuf)>>;

    /// Return every `worktree_path` currently known to the DB, across
    /// all job statuses (queued / leased / running / terminal).
    ///
    /// Used by the cleanup loop to protect worktrees that belong to
    /// in-flight jobs from being reaped by the orphan pass before
    /// their owning row has transitioned to a terminal status.
    ///
    /// This is **intentionally a separate call** from
    /// `expired_terminal_worktrees`: the cleanup loop issues both
    /// back-to-back under two different `Mutex` locks, which creates
    /// a theoretical race where a job that starts between the two
    /// queries is absent from both snapshots. The filesystem mtime
    /// guard inside `worktree::cleanup_by_owner`'s orphan pass is the
    /// backstop: a just-created directory cannot yet be old enough
    /// to trigger TTL expiry unless the operator configures
    /// `daemon.cleanup.ttl_minutes = 0`, which is not a supported
    /// production configuration.
    fn known_worktree_paths(&self) -> Result<Vec<std::path::PathBuf>>;

    /// Clear the `worktree_path` column for any job whose current
    /// value matches `path`.
    ///
    /// Called after a successful `git worktree remove` so the next
    /// cleanup cycle does not re-query the same row and reissue a
    /// removal that will fail because the directory is already gone.
    ///
    /// In practice each path is unique (the runner names worktrees
    /// `reviewq-{job_id}`), so this matches at most one row. The SQL
    /// filter (`WHERE worktree_path = ?1`) is written as an
    /// unrestricted `UPDATE` so future schema changes that relax
    /// uniqueness still produce well-defined behavior — all matching
    /// rows are cleared.
    fn clear_worktree_path(&self, path: &std::path::Path) -> Result<()>;
}

// ---------------------------------------------------------------------------
// ReviewExecutor — abstracts review agent execution
// ---------------------------------------------------------------------------

/// Review execution (Claude / Codex / etc., swappable).
pub trait ReviewExecutor: Send + Sync {
    /// Execute a review for the given job in the specified worktree.
    fn execute(
        &self,
        job: &Job,
        worktree: &Path,
        pid_tx: Option<oneshot::Sender<u32>>,
    ) -> impl std::future::Future<Output = Result<ReviewResult>> + Send;

    /// Cancel a running review.
    fn cancel(&self, job: &Job) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Clear any executor-local process tracking for the given job.
    fn clear_active_pid(&self, _job_id: i64) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Clock — abstracts time for deterministic tests
// ---------------------------------------------------------------------------

/// Time abstraction for deterministic tests.
pub trait Clock: Send + Sync {
    /// Returns the current UTC timestamp.
    fn now(&self) -> DateTime<Utc>;
}

/// Production clock that delegates to `chrono::Utc::now()`.
#[derive(Debug, Clone, Copy)]
pub struct UtcClock;

impl Clock for UtcClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
