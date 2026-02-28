//! External boundary traits for testability and future extensibility.
//!
//! Only these 4 traits are introduced as abstractions. All internal logic
//! uses concrete types. Static dispatch via generics — no `dyn Trait`,
//! no `async-trait` crate (native `async fn` in traits, stable since 1.75).

use std::path::Path;

use chrono::{DateTime, Utc};

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

    /// Cancel a job.
    fn cancel(&self, id: i64) -> Result<()>;

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

    /// Re-queue a stale leased job for retry (increment retry_count, reset to queued).
    fn requeue_stale(&self, id: i64) -> Result<()>;

    /// Check if a PR has already been reviewed (any SHA) by the given agent.
    ///
    /// Returns true when a non-failed, non-canceled job exists for the
    /// (repo, pr_number, agent) triple, regardless of head SHA.
    fn is_pr_reviewed(&self, repo: &RepoId, pr_number: u64, agent: &AgentKind) -> Result<bool>;
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
    ) -> impl std::future::Future<Output = Result<ReviewResult>> + Send;

    /// Cancel a running review.
    fn cancel(&self, job: &Job) -> impl std::future::Future<Output = Result<()>> + Send;
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
