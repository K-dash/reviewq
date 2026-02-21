//! Idempotency checking for job deduplication.
//!
//! Key: (repo, PR number, head SHA, agent kind).

use crate::error::Result;
use crate::traits::JobStore;
use crate::types::{AgentKind, IdempotencyKey, RepoId};

/// Check if a (repo, PR, SHA, agent) combination has already been processed.
pub fn is_duplicate<S: JobStore>(
    store: &S,
    repo: &RepoId,
    pr_number: u64,
    head_sha: &str,
    agent: &AgentKind,
) -> Result<bool> {
    let key = IdempotencyKey {
        repo: repo.clone(),
        pr_number,
        head_sha: head_sha.to_owned(),
        agent_kind: agent.clone(),
    };
    store.is_processed(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::traits::JobStore;
    use crate::types::NewJob;

    #[test]
    fn not_duplicate_when_no_jobs() {
        let db = Database::open_in_memory().expect("db should open");
        let repo = RepoId::new("owner", "repo");
        let result = is_duplicate(&db, &repo, 1, "sha1", &AgentKind::Claude);
        assert!(!result.expect("should succeed"));
    }

    #[test]
    fn duplicate_when_job_exists() {
        let db = Database::open_in_memory().expect("db should open");
        let repo = RepoId::new("owner", "repo");
        db.enqueue(NewJob {
            repo: repo.clone(),
            pr_number: 1,
            head_sha: "sha1".into(),
            agent_kind: AgentKind::Claude,
            command: None,
            max_retries: 3,
        })
        .expect("enqueue should succeed");

        let result = is_duplicate(&db, &repo, 1, "sha1", &AgentKind::Claude);
        assert!(result.expect("should succeed"));
    }

    #[test]
    fn different_sha_is_not_duplicate() {
        let db = Database::open_in_memory().expect("db should open");
        let repo = RepoId::new("owner", "repo");
        db.enqueue(NewJob {
            repo: repo.clone(),
            pr_number: 1,
            head_sha: "sha1".into(),
            agent_kind: AgentKind::Claude,
            command: None,
            max_retries: 3,
        })
        .expect("enqueue should succeed");

        let result = is_duplicate(&db, &repo, 1, "sha2", &AgentKind::Claude);
        assert!(!result.expect("should succeed"));
    }

    #[test]
    fn failed_job_is_not_duplicate() {
        let db = Database::open_in_memory().expect("db should open");
        let repo = RepoId::new("owner", "repo");
        let job = db
            .enqueue(NewJob {
                repo: repo.clone(),
                pr_number: 1,
                head_sha: "sha1".into(),
                agent_kind: AgentKind::Claude,
                command: None,
                max_retries: 3,
            })
            .expect("enqueue should succeed");

        db.complete(job.id, crate::types::JobStatus::Failed, Some(1))
            .expect("complete should succeed");

        // Failed jobs should be eligible for re-enqueueing.
        let result = is_duplicate(&db, &repo, 1, "sha1", &AgentKind::Claude);
        assert!(!result.expect("should succeed"));
    }

    #[test]
    fn succeeded_job_is_duplicate() {
        let db = Database::open_in_memory().expect("db should open");
        let repo = RepoId::new("owner", "repo");
        let job = db
            .enqueue(NewJob {
                repo: repo.clone(),
                pr_number: 1,
                head_sha: "sha1".into(),
                agent_kind: AgentKind::Claude,
                command: None,
                max_retries: 3,
            })
            .expect("enqueue should succeed");

        db.complete(job.id, crate::types::JobStatus::Succeeded, Some(0))
            .expect("complete should succeed");

        // Succeeded jobs should block re-enqueueing.
        let result = is_duplicate(&db, &repo, 1, "sha1", &AgentKind::Claude);
        assert!(result.expect("should succeed"));
    }

    #[test]
    fn canceled_job_is_not_duplicate() {
        let db = Database::open_in_memory().expect("db should open");
        let repo = RepoId::new("owner", "repo");
        let job = db
            .enqueue(NewJob {
                repo: repo.clone(),
                pr_number: 1,
                head_sha: "sha1".into(),
                agent_kind: AgentKind::Claude,
                command: None,
                max_retries: 3,
            })
            .expect("enqueue should succeed");

        db.cancel(job.id).expect("cancel should succeed");

        // Canceled jobs should be eligible for re-enqueueing.
        let result = is_duplicate(&db, &repo, 1, "sha1", &AgentKind::Claude);
        assert!(!result.expect("should succeed"));
    }
}
