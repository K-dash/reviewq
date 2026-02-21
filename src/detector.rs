//! PR detection polling loop.
//!
//! Calls GitHub API, filters via rule engine, checks idempotency,
//! and submits new jobs to the database.

use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::error::Result;
use crate::traits::{GitHubClient, JobStore};
use crate::types::{AgentKind, NewJob, RepoId};

/// Run the detector loop.
pub async fn run<G, S>(github: &G, store: &S, config: &Config) -> Result<()>
where
    G: GitHubClient,
    S: JobStore,
{
    let username = github.authenticated_user().await?;
    let repos = config.parse_allowlist();

    loop {
        match detect_once(github, store, config, &username, &repos).await {
            Ok(count) => info!(new_jobs = count, "detection cycle complete"),
            Err(e) => {
                if e.is_retryable() {
                    warn!(error = %e, "detection cycle failed (retryable)");
                } else {
                    error!(error = %e, "detection cycle failed");
                }
            }
        }
        sleep(Duration::from_secs(config.polling.interval_seconds)).await;
    }
}

/// Single detection cycle: search PRs, apply rules, check idempotency, enqueue.
async fn detect_once<G, S>(
    github: &G,
    store: &S,
    config: &Config,
    username: &str,
    repos: &[RepoId],
) -> Result<usize>
where
    G: GitHubClient,
    S: JobStore,
{
    let prs = github.search_review_requested(repos).await?;
    info!(pr_count = prs.len(), "fetched PRs from GitHub");

    let agent_kind = AgentKind::default();
    let mut enqueued = 0;

    for pr in &prs {
        // Apply filtering rules
        if !crate::rules::should_process(pr, username, repos) {
            continue;
        }

        // Handle SHA changes (cancel stale jobs)
        let sha_changed = crate::update::handle_sha_change(store, pr, &agent_kind)?;
        if sha_changed {
            info!(
                pr = pr.number,
                repo = %pr.repo,
                new_sha = %pr.head_sha,
                "SHA changed, re-queuing"
            );
        }

        // Check idempotency
        if crate::idempotency::is_duplicate(store, &pr.repo, pr.number, &pr.head_sha, &agent_kind)?
        {
            continue;
        }

        // Enqueue new job
        let new_job = NewJob {
            repo: pr.repo.clone(),
            pr_number: pr.number,
            head_sha: pr.head_sha.clone(),
            agent_kind: agent_kind.clone(),
            command: config.runner.command.clone(),
            max_retries: 3,
        };

        match store.enqueue(new_job) {
            Ok(job) => {
                info!(
                    job_id = job.id,
                    pr = pr.number,
                    repo = %pr.repo,
                    sha = %pr.head_sha,
                    "enqueued new review job"
                );
                enqueued += 1;
            }
            Err(e) => {
                warn!(
                    pr = pr.number,
                    repo = %pr.repo,
                    error = %e,
                    "failed to enqueue job"
                );
            }
        }
    }

    Ok(enqueued)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::traits::JobStore;
    use crate::types::{JobFilter, PrState, PullRequest};

    /// A mock GitHub client for testing.
    struct MockGitHub {
        username: String,
        prs: Vec<PullRequest>,
    }

    impl GitHubClient for MockGitHub {
        async fn search_review_requested(&self, _repos: &[RepoId]) -> Result<Vec<PullRequest>> {
            Ok(self.prs.clone())
        }

        async fn requested_reviewers(
            &self,
            _repo: &RepoId,
            _pr_number: u64,
        ) -> Result<Vec<String>> {
            Ok(vec![])
        }

        async fn authenticated_user(&self) -> Result<String> {
            Ok(self.username.clone())
        }
    }

    fn test_config() -> Config {
        Config::from_yaml(
            r#"
repos:
  allowlist:
    - org/repo
polling:
  interval_seconds: 60
"#,
        )
        .expect("config should parse")
    }

    fn make_pr(number: u64, sha: &str) -> PullRequest {
        PullRequest {
            repo: RepoId::new("org", "repo"),
            number,
            url: format!("https://github.com/org/repo/pull/{number}"),
            head_sha: sha.into(),
            author: "alice".into(),
            requested_reviewers: vec!["bob".into()],
            state: PrState::Open,
            draft: false,
        }
    }

    #[tokio::test]
    async fn enqueues_new_pr() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let repos = config.parse_allowlist();

        let count = detect_once(&github, &db, &config, "bob", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].pr_number, 1);
        assert_eq!(jobs[0].head_sha, "sha1");
    }

    #[tokio::test]
    async fn skips_duplicate_pr() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let repos = config.parse_allowlist();

        // First cycle should enqueue
        let count1 = detect_once(&github, &db, &config, "bob", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Second cycle should skip (idempotent)
        let count2 = detect_once(&github, &db, &config, "bob", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count2, 0);
    }

    #[tokio::test]
    async fn skips_self_authored() {
        let github = MockGitHub {
            username: "alice".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let repos = config.parse_allowlist();

        let count = detect_once(&github, &db, &config, "alice", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn skips_draft_pr() {
        let mut pr = make_pr(1, "sha1");
        pr.draft = true;
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![pr],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let repos = config.parse_allowlist();

        let count = detect_once(&github, &db, &config, "bob", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn requeues_on_sha_change() {
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let repos = config.parse_allowlist();

        // First cycle with old SHA
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Second cycle with new SHA
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count2, 1);

        // Should have 2 jobs total: old one canceled, new one queued
        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs.len(), 2);
    }

    #[tokio::test]
    async fn no_prs_returns_zero() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let repos = config.parse_allowlist();

        let count = detect_once(&github, &db, &config, "bob", &repos)
            .await
            .expect("should succeed");
        assert_eq!(count, 0);
    }
}
