//! Integration tests for the reviewq job lifecycle.

use reviewq::config::CancelConfig;
use reviewq::db::Database;
use reviewq::executor::CommandExecutor;
use reviewq::traits::{JobStore, ReviewExecutor};
use reviewq::types::{AgentKind, JobFilter, JobStatus, NewJob, RepoId};

use chrono::Utc;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_db() -> Database {
    Database::open_in_memory().expect("in-memory DB should open")
}

fn sample_job() -> NewJob {
    NewJob {
        repo: RepoId::new("owner", "repo"),
        pr_number: 42,
        head_sha: "aabbccdd11223344".into(),
        agent_kind: AgentKind::Claude,
        command: Some("echo review".into()),
        max_retries: 3,
    }
}

// ---------------------------------------------------------------------------
// Job lifecycle through DB
// ---------------------------------------------------------------------------

#[test]
fn job_lifecycle_enqueue_to_complete() {
    let db = test_db();

    // Enqueue
    let job = db.enqueue(sample_job()).expect("enqueue");
    assert_eq!(job.status, JobStatus::Queued);

    // Lease
    let leased = db.lease_next().expect("lease").expect("should have a job");
    assert_eq!(leased.id, job.id);
    assert_eq!(leased.status, JobStatus::Leased);
    assert!(leased.leased_at.is_some());
    assert!(leased.lease_expires.is_some());

    // Mark running
    db.mark_running(leased.id, 12345).expect("mark_running");
    let jobs = db.list_jobs(&JobFilter::default()).expect("list");
    let running = jobs.iter().find(|j| j.id == job.id).expect("find job");
    assert_eq!(running.status, JobStatus::Running);
    assert_eq!(running.pid, Some(12345));

    // Store review output
    db.store_review_output(job.id, "# LGTM\n\nLooks good!")
        .expect("store_review_output");

    // Complete
    db.complete(job.id, JobStatus::Succeeded, Some(0))
        .expect("complete");

    // Verify final state
    let final_jobs = db
        .list_jobs(&JobFilter {
            status: Some(JobStatus::Succeeded),
            ..Default::default()
        })
        .expect("list");
    assert_eq!(final_jobs.len(), 1);
    assert_eq!(final_jobs[0].exit_code, Some(0));
    assert_eq!(
        final_jobs[0].review_output.as_deref(),
        Some("# LGTM\n\nLooks good!")
    );
}

#[test]
fn job_lifecycle_cancel() {
    let db = test_db();

    let job = db.enqueue(sample_job()).expect("enqueue");
    let _leased = db.lease_next().expect("lease");
    db.mark_running(job.id, 99999).expect("mark_running");

    // Cancel the running job
    db.cancel(job.id).expect("cancel");

    let jobs = db.list_jobs(&JobFilter::default()).expect("list");
    let canceled = jobs.iter().find(|j| j.id == job.id).expect("find job");
    assert_eq!(canceled.status, JobStatus::Canceled);
}

#[test]
fn stale_lease_requeue() {
    let db = test_db();

    let job = db.enqueue(sample_job()).expect("enqueue");
    let leased = db.lease_next().expect("lease").expect("job");
    assert_eq!(leased.status, JobStatus::Leased);

    // Requeue as if lease expired
    db.requeue_stale(job.id).expect("requeue_stale");

    // Should be queued again with incremented retry count
    let jobs = db.list_jobs(&JobFilter::default()).expect("list");
    let requeued = jobs.iter().find(|j| j.id == job.id).expect("find job");
    assert_eq!(requeued.status, JobStatus::Queued);
    assert_eq!(requeued.retry_count, 1);

    // Can be leased again
    let re_leased = db.lease_next().expect("lease").expect("job");
    assert_eq!(re_leased.id, job.id);
}

// ---------------------------------------------------------------------------
// CommandExecutor with real processes
// ---------------------------------------------------------------------------

fn make_test_job(id: i64, command: Option<&str>) -> reviewq::types::Job {
    reviewq::types::Job {
        id,
        repo: RepoId::new("owner", "repo"),
        pr_number: 1,
        head_sha: "abc123".into(),
        agent_kind: AgentKind::Claude,
        status: JobStatus::Running,
        leased_at: Some(Utc::now()),
        lease_expires: Some(Utc::now()),
        retry_count: 0,
        max_retries: 3,
        command: command.map(String::from),
        pid: None,
        exit_code: None,
        stdout_path: None,
        stderr_path: None,
        worktree_path: None,
        review_output: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[tokio::test]
async fn executor_runs_command_and_captures_output() {
    let tmp = TempDir::new().expect("temp dir");
    let output_dir = tmp.path().join("output");
    let worktree = tmp.path().join("worktree");
    std::fs::create_dir_all(&worktree).expect("create worktree dir");

    let executor = CommandExecutor::new(
        "echo 'hello from review'".into(),
        CancelConfig::default(),
        output_dir.clone(),
    );

    let job = make_test_job(42, None);
    let result = executor.execute(&job, &worktree).await.expect("execute");

    assert_eq!(result.exit_code, 0);

    // Verify stdout log was written.
    let stdout_content =
        std::fs::read_to_string(output_dir.join("job-42-stdout.log")).expect("read stdout");
    assert!(stdout_content.contains("hello from review"));
}

#[tokio::test]
async fn executor_reads_review_md_from_worktree() {
    let tmp = TempDir::new().expect("temp dir");
    let output_dir = tmp.path().join("output");
    let worktree = tmp.path().join("worktree");
    std::fs::create_dir_all(&worktree).expect("create worktree dir");

    // Pre-create REVIEW.md (simulating a review agent that writes output).
    std::fs::write(worktree.join("REVIEW.md"), "# Excellent code\n\nLGTM!")
        .expect("write REVIEW.md");

    let executor = CommandExecutor::new("true".into(), CancelConfig::default(), output_dir);

    let job = make_test_job(1, None);
    let result = executor.execute(&job, &worktree).await.expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(
        result.review_markdown.as_deref(),
        Some("# Excellent code\n\nLGTM!")
    );
}
