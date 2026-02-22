//! SHA-change detection and stale job cancellation.
//!
//! When a PR's head SHA changes, cancel the old review and re-queue.

use tracing::{info, warn};

use crate::error::Result;
use crate::traits::JobStore;
use crate::types::{AgentKind, JobFilter, PullRequest};

/// Check if a PR's SHA has changed from what we have in active jobs.
/// If so, cancel stale jobs and return true (caller should re-queue).
pub fn handle_sha_change<S: JobStore>(
    store: &S,
    pr: &PullRequest,
    agent_kind: &AgentKind,
) -> Result<bool> {
    let filter = JobFilter {
        repo: Some(pr.repo.clone()),
        pr_number: Some(pr.number),
        status: None,
    };
    let jobs = store.list_jobs(&filter)?;

    let mut sha_changed = false;
    for job in &jobs {
        if job.agent_kind != *agent_kind {
            continue;
        }
        // Skip jobs that are already in a terminal state
        if job.status.is_terminal() {
            continue;
        }
        // Active job with a different SHA means the PR was force-pushed
        if job.head_sha != pr.head_sha {
            info!(
                job_id = job.id,
                old_sha = %job.head_sha,
                new_sha = %pr.head_sha,
                pr = pr.number,
                "SHA changed, canceling stale job"
            );
            if let Err(e) = store.cancel(job.id) {
                warn!(job_id = job.id, error = %e, "failed to cancel stale job");
            }
            sha_changed = true;
        }
    }

    Ok(sha_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::traits::JobStore;
    use crate::types::{JobStatus, NewJob, PrState, RepoId};

    fn make_pr(sha: &str) -> PullRequest {
        PullRequest {
            repo: RepoId::new("owner", "repo"),
            number: 42,
            url: "https://github.com/owner/repo/pull/42".into(),
            head_sha: sha.into(),
            author: "alice".into(),
            requested_reviewers: vec!["bob".into()],
            state: PrState::Open,
            draft: false,
        }
    }

    #[test]
    fn no_change_when_no_jobs() {
        let db = Database::open_in_memory().expect("db");
        let pr = make_pr("sha_new");
        assert!(!handle_sha_change(&db, &pr, &AgentKind::Claude).expect("should succeed"));
    }

    #[test]
    fn no_change_when_sha_matches() {
        let db = Database::open_in_memory().expect("db");
        db.enqueue(NewJob {
            repo: RepoId::new("owner", "repo"),
            pr_number: 42,
            head_sha: "sha1".into(),
            agent_kind: AgentKind::Claude,
            command: None,
            prompt_template: None,
            max_retries: 3,
        })
        .expect("enqueue");

        let pr = make_pr("sha1");
        assert!(!handle_sha_change(&db, &pr, &AgentKind::Claude).expect("should succeed"));
    }

    #[test]
    fn detects_sha_change_and_cancels() {
        let db = Database::open_in_memory().expect("db");
        let job = db
            .enqueue(NewJob {
                repo: RepoId::new("owner", "repo"),
                pr_number: 42,
                head_sha: "old_sha".into(),
                agent_kind: AgentKind::Claude,
                command: None,
                prompt_template: None,
                max_retries: 3,
            })
            .expect("enqueue");

        let pr = make_pr("new_sha");
        assert!(handle_sha_change(&db, &pr, &AgentKind::Claude).expect("should succeed"));

        // Verify the old job was canceled
        let filter = JobFilter {
            status: Some(JobStatus::Canceled),
            ..Default::default()
        };
        let canceled = db.list_jobs(&filter).expect("list");
        assert_eq!(canceled.len(), 1);
        assert_eq!(canceled[0].id, job.id);
    }

    #[test]
    fn ignores_terminal_jobs() {
        let db = Database::open_in_memory().expect("db");
        let job = db
            .enqueue(NewJob {
                repo: RepoId::new("owner", "repo"),
                pr_number: 42,
                head_sha: "old_sha".into(),
                agent_kind: AgentKind::Claude,
                command: None,
                prompt_template: None,
                max_retries: 3,
            })
            .expect("enqueue");

        // Mark job as succeeded (terminal)
        db.complete(job.id, JobStatus::Succeeded, Some(0))
            .expect("complete");

        let pr = make_pr("new_sha");
        // Should not detect change because the old job is already terminal
        assert!(!handle_sha_change(&db, &pr, &AgentKind::Claude).expect("should succeed"));
    }

    #[test]
    fn ignores_different_agent_kind() {
        let db = Database::open_in_memory().expect("db");
        db.enqueue(NewJob {
            repo: RepoId::new("owner", "repo"),
            pr_number: 42,
            head_sha: "old_sha".into(),
            agent_kind: AgentKind::Codex,
            command: None,
            prompt_template: None,
            max_retries: 3,
        })
        .expect("enqueue");

        let pr = make_pr("new_sha");
        // Should not detect change because it's a different agent kind
        assert!(!handle_sha_change(&db, &pr, &AgentKind::Claude).expect("should succeed"));
    }
}
