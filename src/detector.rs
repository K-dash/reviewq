//! PR detection polling loop.
//!
//! Calls GitHub API, filters via rule engine, checks idempotency,
//! and submits new jobs to the database.

use std::sync::Arc;

use tokio::sync::watch;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::config::{Config, RepoPolicy};
use crate::error::Result;
use crate::traits::{GitHubClient, JobStore};
use crate::types::{AgentKind, NewJob, RepoId};

/// Run the detector loop.
///
/// Re-reads configuration from `config_rx` at each iteration so that
/// changes broadcast via SIGHUP take effect without restarting.
pub async fn run<G, S>(
    github: &G,
    store: &S,
    mut config_rx: watch::Receiver<Arc<Config>>,
) -> Result<()>
where
    G: GitHubClient,
    S: JobStore,
{
    let username = github.authenticated_user().await?;

    loop {
        let config = config_rx.borrow_and_update().clone();
        let policies = config.repo_policies();
        let repo_ids: Vec<RepoId> = policies.iter().map(|p| p.id.clone()).collect();

        match detect_once(github, store, &config, &username, &repo_ids, &policies).await {
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
    repo_ids: &[RepoId],
    policies: &[RepoPolicy],
) -> Result<usize>
where
    G: GitHubClient,
    S: JobStore,
{
    // Split repos into two groups based on reviewer check policy:
    // - reviewed_repos: use Search API with review-requested:{username} filter
    // - unfiltered_repos: use per-repo PR listing (no reviewer filter)
    let (reviewed_repos, unfiltered_repos): (Vec<RepoId>, Vec<RepoId>) =
        repo_ids.iter().cloned().partition(|id| {
            policies
                .iter()
                .find(|p| &p.id == id)
                .is_none_or(|p| !p.skip_reviewer_check)
        });

    let mut prs = Vec::new();

    // Fetch PRs where the user is a requested reviewer.
    if !reviewed_repos.is_empty() {
        let searched = github.search_review_requested(&reviewed_repos).await?;
        prs.extend(searched);
    }

    // Fetch all open PRs for repos that skip the reviewer check.
    for repo_id in &unfiltered_repos {
        match github.list_open_prs(repo_id).await {
            Ok(listed) => prs.extend(listed),
            Err(e) => {
                warn!(repo = %repo_id, error = %e, "failed to list open PRs");
            }
        }
    }

    info!(pr_count = prs.len(), "fetched PRs from GitHub");

    let agent_kind = AgentKind::default();
    let mut enqueued = 0;

    for pr in &prs {
        // Look up per-repo policy.
        let policy = policies.iter().find(|p| p.id == pr.repo);
        let skip_self = policy.is_none_or(|p| p.skip_self_authored);
        let skip_reviewer = policy.is_some_and(|p| p.skip_reviewer_check);
        let review_on_push = policy.is_none_or(|p| p.review_on_push);

        // Apply filtering rules
        if !crate::rules::should_process(pr, username, repo_ids, skip_self, skip_reviewer) {
            continue;
        }

        // Handle SHA changes (cancel stale jobs) — always runs regardless
        // of review_on_push to prevent stale reviews from completing.
        let sha_changed = crate::update::handle_sha_change(store, pr, &agent_kind)?;
        if sha_changed {
            info!(
                pr = pr.number,
                repo = %pr.repo,
                new_sha = %pr.head_sha,
                "SHA changed, re-queuing"
            );
        }

        // Check idempotency: when review_on_push is false, use PR-level
        // dedup (ignores SHA) so succeeded reviews block re-queue.
        let is_dup = if review_on_push {
            crate::idempotency::is_duplicate(store, &pr.repo, pr.number, &pr.head_sha, &agent_kind)?
        } else {
            crate::idempotency::is_duplicate_for_pr(store, &pr.repo, pr.number, &agent_kind)?
        };
        if is_dup {
            continue;
        }

        // Resolve command: per-repo override > global runner.command.
        let command = policy
            .and_then(|p| p.command.clone())
            .or_else(|| config.runner.command.clone());

        // Resolve prompt_template: per-repo override > global runner.prompt_template.
        let prompt_template = policy
            .and_then(|p| p.prompt_template.clone())
            .or_else(|| config.runner.prompt_template.clone());

        // Enqueue new job
        let new_job = NewJob {
            repo: pr.repo.clone(),
            pr_number: pr.number,
            head_sha: pr.head_sha.clone(),
            agent_kind: agent_kind.clone(),
            command,
            prompt_template,
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

        async fn list_open_prs(&self, repo: &RepoId) -> Result<Vec<PullRequest>> {
            Ok(self
                .prs
                .iter()
                .filter(|pr| &pr.repo == repo)
                .cloned()
                .collect())
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
    - repo: org/repo
polling:
  interval_seconds: 60
"#,
        )
        .expect("config should parse")
    }

    fn test_config_skip_self_disabled() -> Config {
        Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      skip_self_authored: false
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
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
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
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle should enqueue
        let count1 = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Second cycle should skip (idempotent)
        let count2 = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
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
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "alice", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn accepts_self_authored_when_skip_disabled() {
        let mut pr = make_pr(1, "sha1");
        pr.requested_reviewers.push("alice".into());
        let github = MockGitHub {
            username: "alice".into(),
            prs: vec![pr],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config_skip_self_disabled();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "alice", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn per_repo_command_overrides_global() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      command: "per-repo-cmd"
runner:
  command: "global-cmd"
polling:
  interval_seconds: 60
"#,
        )
        .expect("config");
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs[0].command.as_deref(), Some("per-repo-cmd"));
    }

    #[tokio::test]
    async fn global_command_used_when_no_per_repo_override() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
runner:
  command: "global-cmd"
polling:
  interval_seconds: 60
"#,
        )
        .expect("config");
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs[0].command.as_deref(), Some("global-cmd"));
    }

    #[tokio::test]
    async fn per_repo_prompt_template_overrides_global() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      prompt_template: "per-repo-prompt"
runner:
  prompt_template: "global-prompt"
polling:
  interval_seconds: 60
"#,
        )
        .expect("config");
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs[0].prompt_template.as_deref(), Some("per-repo-prompt"));
    }

    #[tokio::test]
    async fn global_prompt_template_used_when_no_per_repo_override() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
runner:
  prompt_template: "global-prompt"
polling:
  interval_seconds: 60
"#,
        )
        .expect("config");
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs[0].prompt_template.as_deref(), Some("global-prompt"));
    }

    #[tokio::test]
    async fn no_prompt_template_when_both_none() {
        let github = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "sha1")],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert!(jobs[0].prompt_template.is_none());
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
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn requeues_on_sha_change() {
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle with old SHA
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Second cycle with new SHA
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repo_ids, &policies)
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
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 0);
    }

    fn test_config_skip_reviewer_check() -> Config {
        Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      skip_self_authored: false
      skip_reviewer_check: true
polling:
  interval_seconds: 60
"#,
        )
        .expect("config should parse")
    }

    #[tokio::test]
    async fn enqueues_via_list_open_prs_when_skip_reviewer_check() {
        // Self-authored PR with no reviewer assignment — should be
        // fetched via list_open_prs and enqueued when skip_reviewer_check is true.
        let mut pr = make_pr(1, "sha1");
        pr.author = "alice".into();
        pr.requested_reviewers.clear();

        let github = MockGitHub {
            username: "alice".into(),
            prs: vec![pr],
        };
        let db = Database::open_in_memory().expect("db");
        let config = test_config_skip_reviewer_check();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        let count = detect_once(&github, &db, &config, "alice", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].pr_number, 1);
    }

    fn test_config_review_on_push_false() -> Config {
        Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      review_on_push: false
polling:
  interval_seconds: 60
"#,
        )
        .expect("config should parse")
    }

    #[tokio::test]
    async fn review_on_push_false_does_not_requeue_after_success() {
        let db = Database::open_in_memory().expect("db");
        let config = test_config_review_on_push_false();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle: enqueue and succeed
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Mark the job as succeeded
        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        db.complete(jobs[0].id, crate::types::JobStatus::Succeeded, Some(0))
            .expect("complete");

        // Second cycle with new SHA — should NOT re-queue
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count2, 0, "succeeded PR should not be re-queued");
    }

    #[tokio::test]
    async fn review_on_push_false_cancels_stale_and_queues_first_review() {
        let db = Database::open_in_memory().expect("db");
        let config = test_config_review_on_push_false();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle: enqueue with old SHA (still queued, not succeeded)
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Second cycle with new SHA — old job should be canceled, new one queued
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(
            count2, 1,
            "first review should still be queued on SHA change"
        );

        // Verify: old job canceled, new job queued
        let all_jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(all_jobs.len(), 2);
        let canceled = db
            .list_jobs(&JobFilter {
                status: Some(crate::types::JobStatus::Canceled),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(canceled.len(), 1);
        assert_eq!(canceled[0].head_sha, "old_sha");
    }

    #[tokio::test]
    async fn review_on_push_false_allows_retry_after_failure() {
        let db = Database::open_in_memory().expect("db");
        let config = test_config_review_on_push_false();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle: enqueue and fail
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        db.complete(jobs[0].id, crate::types::JobStatus::Failed, Some(1))
            .expect("complete");

        // Second cycle with new SHA — should re-queue (failed is retryable)
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count2, 1, "failed job should allow retry");
    }

    #[tokio::test]
    async fn review_on_push_true_requeues_on_sha_change() {
        // Verify default behavior (review_on_push: true) is unchanged.
        let db = Database::open_in_memory().expect("db");
        let config = test_config();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        db.complete(jobs[0].id, crate::types::JobStatus::Succeeded, Some(0))
            .expect("complete");

        // Second cycle with new SHA — should re-queue (default behavior)
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count2, 1, "default behavior should re-queue on SHA change");
    }

    #[tokio::test]
    async fn review_on_push_false_cancels_leased_and_queues_new() {
        let db = Database::open_in_memory().expect("db");
        let config = test_config_review_on_push_false();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle: enqueue with old SHA
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Simulate the job being leased (in-flight)
        let leased = db.lease_next().expect("lease").expect("should have job");
        assert_eq!(leased.status, crate::types::JobStatus::Leased);

        // Second cycle with new SHA — leased job should be canceled, new one queued
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(
            count2, 1,
            "leased job on old SHA should be canceled and new job queued"
        );

        let canceled = db
            .list_jobs(&JobFilter {
                status: Some(crate::types::JobStatus::Canceled),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(canceled.len(), 1);
        assert_eq!(canceled[0].head_sha, "old_sha");
    }

    #[tokio::test]
    async fn review_on_push_false_cancels_running_and_queues_new() {
        let db = Database::open_in_memory().expect("db");
        let config = test_config_review_on_push_false();
        let policies = config.repo_policies();
        let repo_ids = config.repo_ids();

        // First cycle: enqueue with old SHA
        let github1 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "old_sha")],
        };
        let count1 = detect_once(&github1, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(count1, 1);

        // Simulate the job being leased then running
        let leased = db.lease_next().expect("lease").expect("should have job");
        db.mark_running(leased.id, 12345).expect("mark running");

        // Second cycle with new SHA — running job should be canceled, new one queued
        let github2 = MockGitHub {
            username: "bob".into(),
            prs: vec![make_pr(1, "new_sha")],
        };
        let count2 = detect_once(&github2, &db, &config, "bob", &repo_ids, &policies)
            .await
            .expect("should succeed");
        assert_eq!(
            count2, 1,
            "running job on old SHA should be canceled and new job queued"
        );

        let canceled = db
            .list_jobs(&JobFilter {
                status: Some(crate::types::JobStatus::Canceled),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(canceled.len(), 1);
        assert_eq!(canceled[0].head_sha, "old_sha");
    }
}
