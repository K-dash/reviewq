//! Concrete [`ReviewExecutor`] implementation using shell commands.
//!
//! Spawns the configured review command in a process group within the job's
//! worktree, captures stdout/stderr to log files, and reads the review
//! markdown from a well-known file after completion.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::{info, warn};

use crate::config::CancelConfig;
use crate::error::{Result, ReviewqError};
use crate::runner::{cancel, process};
use crate::traits::ReviewExecutor;
use crate::types::{Job, ReviewResult};

/// Well-known filename that review agents write their output to.
const REVIEW_OUTPUT_FILE: &str = "REVIEW.md";

/// Command-based review executor.
///
/// Spawns a shell command in a new process group within the job's worktree.
/// Tracks child PIDs so that in-flight reviews can be canceled via staged
/// signal escalation (SIGINT → SIGTERM → SIGKILL).
pub struct CommandExecutor {
    /// Default shell command to execute for reviews.
    command: String,
    /// Cancel escalation timeouts.
    cancel_config: CancelConfig,
    /// Directory for stdout/stderr log files.
    output_dir: PathBuf,
    /// Active child PIDs keyed by job ID (for cancel support).
    active_pids: Mutex<HashMap<i64, u32>>,
}

impl CommandExecutor {
    /// Create a new executor with the given default command, cancel config,
    /// and output directory for log files.
    pub fn new(command: String, cancel_config: CancelConfig, output_dir: PathBuf) -> Self {
        Self {
            command,
            cancel_config,
            output_dir,
            active_pids: Mutex::new(HashMap::new()),
        }
    }
}

/// Lock `active_pids`, mapping poisoned-mutex to a domain error.
fn lock_active_pids(
    mutex: &Mutex<HashMap<i64, u32>>,
) -> Result<std::sync::MutexGuard<'_, HashMap<i64, u32>>> {
    mutex
        .lock()
        .map_err(|_| ReviewqError::Process("active_pids mutex poisoned".into()))
}

impl ReviewExecutor for CommandExecutor {
    async fn execute(&self, job: &Job, worktree: &Path) -> Result<ReviewResult> {
        // Ensure output directory exists (async I/O — fix #5).
        tokio::fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|e| {
                ReviewqError::Process(format!(
                    "failed to create output directory {}: {e}",
                    self.output_dir.display()
                ))
            })?;

        let stdout_path = self.output_dir.join(format!("job-{}-stdout.log", job.id));
        let stderr_path = self.output_dir.join(format!("job-{}-stderr.log", job.id));

        // Use job-specific command if set, otherwise the default.
        let cmd = job.command.as_deref().unwrap_or(&self.command);

        let (mut child, pid) =
            process::spawn_in_group(cmd, worktree, &stdout_path, &stderr_path).await?;

        info!(
            job_id = job.id,
            pid,
            command = cmd,
            "spawned review process"
        );

        // Track the child PID for cancel support.
        // Lock is not held across .await — only brief HashMap insert.
        lock_active_pids(&self.active_pids)?.insert(job.id, pid);

        // Wait for child — always clean up active_pids even on error (fix #3).
        let wait_result = child.wait().await;

        // Remove from active tracking regardless of wait outcome.
        let _ = lock_active_pids(&self.active_pids).map(|mut guard| guard.remove(&job.id));

        let status = wait_result
            .map_err(|e| ReviewqError::Process(format!("failed to wait on child process: {e}")))?;

        let exit_code = status.code().unwrap_or(-1);

        // Try to read review output from the well-known file (async I/O — fix #5).
        let review_markdown = tokio::fs::read_to_string(worktree.join(REVIEW_OUTPUT_FILE))
            .await
            .ok();

        Ok(ReviewResult {
            exit_code,
            review_markdown,
        })
    }

    async fn cancel(&self, job: &Job) -> Result<()> {
        let pid = lock_active_pids(&self.active_pids)?.get(&job.id).copied();

        match pid {
            Some(pid) => {
                info!(job_id = job.id, pid, "canceling review process");
                cancel::cancel_process_group(pid, &self.cancel_config).await
            }
            None => {
                warn!(job_id = job.id, "no active process found for cancel");
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, JobStatus, RepoId};
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_job(id: i64, command: Option<&str>) -> Job {
        Job {
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
    async fn execute_echo_command() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        let executor = CommandExecutor::new(
            "echo hello".into(),
            CancelConfig::default(),
            output_dir.clone(),
        );

        let job = make_job(1, None);
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        assert!(result.review_markdown.is_none());
        assert!(output_dir.join("job-1-stdout.log").exists());
        assert!(output_dir.join("job-1-stderr.log").exists());
    }

    #[tokio::test]
    async fn execute_reads_review_md() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");
        std::fs::write(worktree.join("REVIEW.md"), "# LGTM").expect("write REVIEW.md");

        let executor = CommandExecutor::new("true".into(), CancelConfig::default(), output_dir);

        let job = make_job(1, None);
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.review_markdown.as_deref(), Some("# LGTM"));
    }

    #[tokio::test]
    async fn execute_failing_command() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        let executor = CommandExecutor::new("exit 1".into(), CancelConfig::default(), output_dir);

        let job = make_job(1, None);
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 1);
    }

    #[tokio::test]
    async fn cancel_nonexistent_job_is_noop() {
        let tmp = TempDir::new().expect("temp dir");
        let executor = CommandExecutor::new(
            "true".into(),
            CancelConfig::default(),
            tmp.path().to_path_buf(),
        );

        let job = make_job(999, None);
        executor.cancel(&job).await.expect("cancel should succeed");
    }

    #[tokio::test]
    async fn job_specific_command_overrides_default() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        let executor = CommandExecutor::new(
            "exit 1".into(), // default would fail
            CancelConfig::default(),
            output_dir,
        );

        // Job-specific command overrides the default.
        let job = make_job(1, Some("echo override"));
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
    }
}
