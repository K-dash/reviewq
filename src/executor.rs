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

/// Maximum size (in bytes) for the `REVIEWQ_PROMPT` environment variable.
/// Prompts larger than this are only available via the prompt file.
const MAX_PROMPT_ENV_SIZE: usize = 128 * 1024;

/// Default prompt template used when no `prompt_template` is configured.
/// Stored as a separate Markdown file for maintainability and loaded at compile time.
const DEFAULT_PROMPT_TEMPLATE: &str = include_str!("prompts/default_review.md");

// ---------------------------------------------------------------------------
// Template variable interpolation
// ---------------------------------------------------------------------------

/// Holds all values available for template variable interpolation and
/// environment variable injection.
struct TemplateContext {
    pr_url: String,
    repo: String,
    pr_number: String,
    head_sha: String,
    worktree_path: String,
    job_id: String,
    output_path: String,
}

impl TemplateContext {
    /// Build a context from a [`Job`] and its worktree path.
    fn new(job: &Job, worktree: &Path) -> Self {
        let repo = job.repo.full_name();
        let pr_number = job.pr_number.to_string();
        let pr_url = format!("https://github.com/{}/pull/{}", repo, pr_number);
        Self {
            pr_url,
            repo,
            pr_number,
            head_sha: job.head_sha.clone(),
            worktree_path: worktree.display().to_string(),
            job_id: job.id.to_string(),
            output_path: worktree.join(REVIEW_OUTPUT_FILE).display().to_string(),
        }
    }

    /// Replace all known `{variable}` placeholders in a template string.
    fn interpolate(&self, template: &str) -> String {
        template
            .replace("{pr_url}", &self.pr_url)
            .replace("{repo}", &self.repo)
            .replace("{pr_number}", &self.pr_number)
            .replace("{head_sha}", &self.head_sha)
            .replace("{worktree_path}", &self.worktree_path)
            .replace("{job_id}", &self.job_id)
            .replace("{output_path}", &self.output_path)
    }

    /// Replace base variables plus additional key-value pairs.
    /// Used for two-phase interpolation: command gets `{prompt}` and `{prompt_file}`
    /// in addition to the base variables.
    ///
    /// To prevent double-expansion (e.g., prompt text containing `{prompt_file}`
    /// being expanded again), extras are replaced via intermediate sentinel tokens.
    fn interpolate_with_extras(&self, template: &str, extras: &[(&str, &str)]) -> String {
        // Step 1: Replace extras placeholders with unique sentinel tokens.
        let mut result = template.to_owned();
        let sentinels: Vec<String> = extras
            .iter()
            .enumerate()
            .map(|(i, _)| format!("\x00__REVIEWQ_EXTRA_{i}__\x00"))
            .collect();
        for ((key, _), sentinel) in extras.iter().zip(&sentinels) {
            result = result.replace(&format!("{{{key}}}"), sentinel);
        }

        // Step 2: Interpolate base variables (cannot touch sentinels).
        result = result
            .replace("{pr_url}", &self.pr_url)
            .replace("{repo}", &self.repo)
            .replace("{pr_number}", &self.pr_number)
            .replace("{head_sha}", &self.head_sha)
            .replace("{worktree_path}", &self.worktree_path)
            .replace("{job_id}", &self.job_id)
            .replace("{output_path}", &self.output_path);

        // Step 3: Replace sentinels with actual extra values (no further expansion).
        for ((_, value), sentinel) in extras.iter().zip(&sentinels) {
            result = result.replace(sentinel, value);
        }
        result
    }

    /// Return `REVIEWQ_*` environment variable pairs for the child process.
    fn env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("REVIEWQ_PR_URL".into(), self.pr_url.clone()),
            ("REVIEWQ_REPO".into(), self.repo.clone()),
            ("REVIEWQ_PR_NUMBER".into(), self.pr_number.clone()),
            ("REVIEWQ_HEAD_SHA".into(), self.head_sha.clone()),
            ("REVIEWQ_WORKTREE_PATH".into(), self.worktree_path.clone()),
            ("REVIEWQ_JOB_ID".into(), self.job_id.clone()),
            ("REVIEWQ_OUTPUT_PATH".into(), self.output_path.clone()),
        ]
    }
}

/// Log a warning for any remaining `{identifier}` patterns in a template after
/// interpolation. Shell constructs like `${VAR}` are intentionally ignored.
fn warn_unknown_variables(text: &str, context: &str) {
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after_brace = &rest[start + 1..];
        if let Some(end) = after_brace.find('}') {
            let name = &after_brace[..end];
            // Only warn for simple identifiers (non-empty, ASCII alphanumeric + underscore).
            // Skip shell constructs like ${VAR} (contains $) or empty braces {}.
            if !name.is_empty()
                && !name.contains('$')
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                warn!(variable = name, context, "unknown template variable");
            }
            rest = &after_brace[end + 1..];
        } else {
            break;
        }
    }
}

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

        let ctx = TemplateContext::new(job, worktree);

        // Phase 1: Interpolate prompt template with base variables.
        let raw_prompt = job
            .prompt_template
            .as_deref()
            .unwrap_or(DEFAULT_PROMPT_TEMPLATE);
        let rendered_prompt = ctx.interpolate(raw_prompt);
        warn_unknown_variables(&rendered_prompt, "prompt");

        // Write rendered prompt to a file (always available regardless of size).
        let prompt_file_path = self.output_dir.join(format!("job-{}-prompt.txt", job.id));
        tokio::fs::write(&prompt_file_path, &rendered_prompt)
            .await
            .map_err(|e| {
                ReviewqError::Process(format!(
                    "failed to write prompt file {}: {e}",
                    prompt_file_path.display()
                ))
            })?;
        let prompt_file_str = prompt_file_path.display().to_string();

        // Phase 2: Interpolate command with base vars + {prompt} + {prompt_file}.
        let raw_cmd = job.command.as_deref().unwrap_or(&self.command);
        let cmd = ctx.interpolate_with_extras(
            raw_cmd,
            &[
                ("prompt", &rendered_prompt),
                ("prompt_file", &prompt_file_str),
            ],
        );
        warn_unknown_variables(&cmd, "command");

        // Build env vars: base + REVIEWQ_PROMPT (if <= 128KB) + REVIEWQ_PROMPT_FILE (always).
        let mut env_vars = ctx.env_vars();
        if rendered_prompt.len() <= MAX_PROMPT_ENV_SIZE {
            env_vars.push(("REVIEWQ_PROMPT".into(), rendered_prompt.clone()));
        } else {
            warn!(
                job_id = job.id,
                prompt_size = rendered_prompt.len(),
                "prompt exceeds 128KB, REVIEWQ_PROMPT env var not set"
            );
        }
        env_vars.push(("REVIEWQ_PROMPT_FILE".into(), prompt_file_str.clone()));

        let (mut child, pid) =
            process::spawn_in_group(&cmd, worktree, &stdout_path, &stderr_path, &env_vars).await?;

        // Log with truncated command when the raw command contained {prompt}
        // to avoid leaking potentially large/sensitive prompt content in logs.
        let cmd_has_prompt = raw_cmd.contains("{prompt}");
        if cmd_has_prompt {
            info!(
                job_id = job.id,
                pid,
                command_length = cmd.len(),
                prompt_file = %prompt_file_str,
                "spawned review process (prompt expanded, full command omitted from log)"
            );
        } else {
            info!(
                job_id = job.id,
                pid,
                command = %cmd,
                "spawned review process"
            );
        }

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
        make_job_with_prompt(id, command, None)
    }

    fn make_job_with_prompt(id: i64, command: Option<&str>, prompt_template: Option<&str>) -> Job {
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
            prompt_template: prompt_template.map(String::from),
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

    // -- TemplateContext unit tests ----------------------------------------

    #[test]
    fn interpolate_replaces_all_variables() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate(
            "{pr_url} {repo} {pr_number} {head_sha} {worktree_path} {job_id} {output_path}",
        );
        assert_eq!(
            cmd,
            format!(
                "https://github.com/owner/repo/pull/1 owner/repo 1 abc123 /tmp/worktree 1 {}",
                worktree.join("REVIEW.md").display()
            )
        );
    }

    #[test]
    fn interpolate_no_variables_passthrough() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate("echo hello");
        assert_eq!(cmd, "echo hello");
    }

    #[test]
    fn interpolate_unknown_variables_left_intact() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate("echo {unknown_var}");
        assert_eq!(cmd, "echo {unknown_var}");
    }

    #[test]
    fn interpolate_partial_variables() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate("review --pr {pr_number} --sha {head_sha}");
        assert_eq!(cmd, "review --pr 1 --sha abc123");
    }

    #[test]
    fn interpolate_repeated_variables() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate("{job_id}-{job_id}");
        assert_eq!(cmd, "1-1");
    }

    #[test]
    fn env_vars_all_present() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let env_vars = ctx.env_vars();
        assert_eq!(env_vars.len(), 7);

        let find = |key: &str| -> String {
            env_vars
                .iter()
                .find(|(k, _)| k == key)
                .unwrap_or_else(|| panic!("{key} not found"))
                .1
                .clone()
        };

        assert_eq!(
            find("REVIEWQ_PR_URL"),
            "https://github.com/owner/repo/pull/1"
        );
        assert_eq!(find("REVIEWQ_REPO"), "owner/repo");
        assert_eq!(find("REVIEWQ_PR_NUMBER"), "1");
        assert_eq!(find("REVIEWQ_HEAD_SHA"), "abc123");
        assert_eq!(find("REVIEWQ_WORKTREE_PATH"), "/tmp/worktree");
        assert_eq!(find("REVIEWQ_JOB_ID"), "1");
        assert_eq!(
            find("REVIEWQ_OUTPUT_PATH"),
            worktree.join("REVIEW.md").display().to_string()
        );
    }

    #[test]
    fn interpolate_with_job_specific_command() {
        let job = make_job(1, Some("custom-review {pr_url} --out {output_path}"));
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        // Job-level command should also be interpolated.
        let raw_cmd = job.command.as_deref().unwrap();
        let cmd = ctx.interpolate(raw_cmd);
        assert_eq!(
            cmd,
            format!(
                "custom-review https://github.com/owner/repo/pull/1 --out {}",
                worktree.join("REVIEW.md").display()
            )
        );
    }

    // -- CommandExecutor integration tests ----------------------------------

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

    // -- interpolate_with_extras unit tests ----------------------------------

    #[test]
    fn interpolate_with_extras_replaces_prompt_and_prompt_file() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate_with_extras(
            "agent --prompt '{prompt}' --file {prompt_file}",
            &[
                ("prompt", "Review this PR"),
                ("prompt_file", "/tmp/prompt.txt"),
            ],
        );
        assert_eq!(
            cmd,
            "agent --prompt 'Review this PR' --file /tmp/prompt.txt"
        );
    }

    #[test]
    fn interpolate_with_extras_also_replaces_base_vars() {
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate_with_extras("{repo} {prompt}", &[("prompt", "hello")]);
        assert_eq!(cmd, "owner/repo hello");
    }

    #[test]
    fn interpolate_with_extras_no_double_expansion() {
        // If prompt value contains "{prompt_file}", it should NOT be expanded.
        let job = make_job(1, None);
        let worktree = Path::new("/tmp/worktree");
        let ctx = TemplateContext::new(&job, worktree);

        let cmd = ctx.interpolate_with_extras(
            "cmd --prompt '{prompt}' --file {prompt_file}",
            &[
                ("prompt", "Use {prompt_file} for input"),
                ("prompt_file", "/tmp/prompt.txt"),
            ],
        );
        assert_eq!(
            cmd,
            "cmd --prompt 'Use {prompt_file} for input' --file /tmp/prompt.txt"
        );
    }

    // -- Prompt rendering integration tests -----------------------------------

    #[tokio::test]
    async fn execute_renders_default_prompt() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        // Command echoes the REVIEWQ_PROMPT env var into REVIEW.md
        let cmd = r#"printf '%s' "$REVIEWQ_PROMPT" > REVIEW.md"#;
        let executor =
            CommandExecutor::new(cmd.into(), CancelConfig::default(), output_dir.clone());

        let job = make_job(1, None);
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        let content = result.review_markdown.expect("REVIEW.md should exist");
        // Default prompt should contain the spec's structured template
        assert!(
            content.contains("Review the following PR"),
            "default prompt not rendered: {content}"
        );
        assert!(
            content.contains("owner/repo"),
            "repo not interpolated in prompt: {content}"
        );
    }

    #[tokio::test]
    async fn execute_renders_custom_prompt() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        let cmd = r#"printf '%s' "$REVIEWQ_PROMPT" > REVIEW.md"#;
        let executor = CommandExecutor::new(cmd.into(), CancelConfig::default(), output_dir);

        let job = make_job_with_prompt(1, None, Some("Custom review for {repo} PR #{pr_number}"));
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        let content = result.review_markdown.expect("REVIEW.md should exist");
        assert_eq!(content, "Custom review for owner/repo PR #1");
    }

    #[tokio::test]
    async fn execute_writes_prompt_file() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        let executor =
            CommandExecutor::new("true".into(), CancelConfig::default(), output_dir.clone());

        let job = make_job_with_prompt(42, None, Some("Prompt for {repo}"));
        let result = executor.execute(&job, &worktree).await.expect("execute");
        assert_eq!(result.exit_code, 0);

        let prompt_file = output_dir.join("job-42-prompt.txt");
        assert!(prompt_file.exists(), "prompt file should be created");
        let content = std::fs::read_to_string(prompt_file).expect("read prompt file");
        assert_eq!(content, "Prompt for owner/repo");
    }

    #[tokio::test]
    async fn prompt_variables_in_command() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        // Command uses both {prompt} and {prompt_file}
        let cmd = r#"printf '%s\n%s' '{prompt}' '{prompt_file}' > REVIEW.md"#;
        let executor =
            CommandExecutor::new(cmd.into(), CancelConfig::default(), output_dir.clone());

        let job = make_job_with_prompt(1, None, Some("Hello {repo}"));
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        let content = result.review_markdown.expect("REVIEW.md should exist");
        assert!(
            content.contains("Hello owner/repo"),
            "prompt not interpolated in command: {content}"
        );
        let prompt_file_path = output_dir.join("job-1-prompt.txt").display().to_string();
        assert!(
            content.contains(&prompt_file_path),
            "prompt_file not interpolated in command: {content}"
        );
    }

    #[tokio::test]
    async fn prompt_file_env_var_always_set() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        let cmd = r#"printf '%s' "$REVIEWQ_PROMPT_FILE" > REVIEW.md"#;
        let executor =
            CommandExecutor::new(cmd.into(), CancelConfig::default(), output_dir.clone());

        let job = make_job(1, None);
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        let content = result.review_markdown.expect("REVIEW.md should exist");
        let expected_path = output_dir.join("job-1-prompt.txt").display().to_string();
        assert_eq!(content, expected_path);
    }

    #[tokio::test]
    async fn large_prompt_skips_env_var() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        // Create a prompt template that expands to > 128KB
        let large_template = "x".repeat(MAX_PROMPT_ENV_SIZE + 1);
        let cmd = r#"printf '%s\n%s' "$REVIEWQ_PROMPT" "$REVIEWQ_PROMPT_FILE" > REVIEW.md"#;
        let executor =
            CommandExecutor::new(cmd.into(), CancelConfig::default(), output_dir.clone());

        let job = make_job_with_prompt(1, None, Some(&large_template));
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        let content = result.review_markdown.expect("REVIEW.md should exist");
        // REVIEWQ_PROMPT should be empty (not set), but REVIEWQ_PROMPT_FILE should be present
        let lines: Vec<&str> = content.split('\n').collect();
        assert!(
            lines[0].is_empty(),
            "REVIEWQ_PROMPT should not be set for large prompts"
        );
        assert!(
            !lines[1].is_empty(),
            "REVIEWQ_PROMPT_FILE should always be set"
        );

        // The prompt file should still contain the full content
        let prompt_file = output_dir.join("job-1-prompt.txt");
        let file_content = std::fs::read_to_string(prompt_file).expect("read prompt file");
        assert_eq!(file_content.len(), MAX_PROMPT_ENV_SIZE + 1);
    }

    #[tokio::test]
    async fn prompt_at_exact_boundary_sets_env_var() {
        let tmp = TempDir::new().expect("temp dir");
        let output_dir = tmp.path().join("output");
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("create worktree dir");

        // Exactly 128KB should still be set (boundary is <=)
        let exact_template = "x".repeat(MAX_PROMPT_ENV_SIZE);
        let cmd = r#"printf '%s' "$REVIEWQ_PROMPT" > REVIEW.md"#;
        let executor =
            CommandExecutor::new(cmd.into(), CancelConfig::default(), output_dir.clone());

        let job = make_job_with_prompt(1, None, Some(&exact_template));
        let result = executor.execute(&job, &worktree).await.expect("execute");

        assert_eq!(result.exit_code, 0);
        let content = result.review_markdown.expect("REVIEW.md should exist");
        assert_eq!(
            content.len(),
            MAX_PROMPT_ENV_SIZE,
            "REVIEWQ_PROMPT should be set for exactly 128KB prompt"
        );
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
