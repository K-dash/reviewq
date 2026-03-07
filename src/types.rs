//! Shared domain types used across all modules.

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Repository
// ---------------------------------------------------------------------------

/// A GitHub repository identifier (owner/name).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId {
    pub owner: String,
    pub name: String,
}

impl RepoId {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }

    /// Returns `"owner/name"` form.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

// ---------------------------------------------------------------------------
// Pull Request
// ---------------------------------------------------------------------------

/// State of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// A normalized pull request from the GitHub API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub repo: RepoId,
    pub number: u64,
    pub url: String,
    pub head_sha: String,
    pub author: String,
    pub requested_reviewers: Vec<String>,
    pub state: PrState,
    pub draft: bool,
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// The kind of review agent to use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

impl AgentKind {
    /// Parse from a string stored in the database.
    ///
    /// Unknown values fall back to `Claude` with a warning log.
    pub fn from_db(s: &str) -> Self {
        match s {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            other => {
                tracing::warn!(
                    value = other,
                    "unknown agent_kind in DB, falling back to claude"
                );
                Self::Claude
            }
        }
    }

    /// Serialize to a string for database storage.
    pub fn as_db_str(&self) -> &str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// Return the default shell command template for this agent.
    ///
    /// The template may contain `{prompt_file}` and `{output_path}` placeholders
    /// that are interpolated by the executor before spawning.
    ///
    /// Commands use `--output-format json` (Claude) or `--json` (Codex) so the
    /// executor can parse session IDs from the structured output.
    ///
    /// When `model` is `Some`, a `--model <name>` flag is injected into the
    /// command so the agent uses a specific model.
    pub fn default_command(&self, model: Option<&str>) -> String {
        let model_flag = model.map(|m| format!(" --model {m}")).unwrap_or_default();
        match self {
            Self::Claude => {
                format!(
                    r#"claude -p "$(cat "{{prompt_file}}")"{model_flag} --output-format json --allowedTools Read Grep Glob Bash WebFetch WebSearch Agent Skill"#
                )
            }
            Self::Codex => {
                format!(
                    r#"codex exec --json{model_flag} --sandbox danger-full-access - < "{{prompt_file}}""#
                )
            }
        }
    }

    /// Parse agent-specific structured output to extract session ID and markdown.
    ///
    /// Returns `(session_id, markdown)`. Both are `None` on parse failure,
    /// allowing the caller to fall back to other sources.
    pub fn parse_output(&self, raw: &str) -> (Option<String>, Option<String>) {
        match self {
            Self::Claude => parse_claude_json(raw),
            Self::Codex => parse_codex_jsonl(raw),
        }
    }

    /// Return the CLI command to resume a session with this agent.
    ///
    /// If `repo_path` is provided, the command is prefixed with `cd <path> &&`
    /// so users can paste and run it directly.
    pub fn resume_command(&self, session_id: &str, repo_path: Option<&std::path::Path>) -> String {
        let resume = match self {
            Self::Claude => format!("claude --resume {session_id}"),
            Self::Codex => format!("codex exec resume {session_id}"),
        };
        match repo_path {
            Some(p) => format!("cd {} && {resume}", p.display()),
            None => resume,
        }
    }
}

/// Parse Claude `--output-format json` output.
///
/// Claude outputs a JSON array of conversation events:
/// `[{type:"system", session_id:...}, {type:"assistant", message:{content:[...]}}, ..., {type:"result", result:"...", session_id:"..."}]`
///
/// The review text comes from `result.result`. If that is empty (e.g. the
/// agent only used tools), we collect `assistant` → `message.content[]`
/// text blocks instead.
fn parse_claude_json(raw: &str) -> (Option<String>, Option<String>) {
    let val: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };

    let arr = match val.as_array() {
        Some(a) => a,
        None => return (None, None),
    };

    let mut session_id: Option<String> = None;
    let mut result_text: Option<String> = None;
    let mut assistant_texts: Vec<String> = Vec::new();

    for item in arr {
        // session_id appears on every entry; grab it once.
        if session_id.is_none()
            && let Some(sid) = item.get("session_id").and_then(|v| v.as_str())
        {
            session_id = Some(sid.to_owned());
        }

        match item.get("type").and_then(|v| v.as_str()) {
            Some("result") => {
                if let Some(r) = item
                    .get("result")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    result_text = Some(r.to_owned());
                }
            }
            Some("assistant") => {
                if let Some(content) = item
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for c in content {
                        if c.get("type").and_then(|v| v.as_str()) == Some("text")
                            && let Some(text) = c.get("text").and_then(|v| v.as_str())
                        {
                            assistant_texts.push(text.to_owned());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let markdown = result_text.or_else(|| {
        if assistant_texts.is_empty() {
            None
        } else {
            Some(assistant_texts.join("\n\n"))
        }
    });

    (session_id, markdown)
}

/// Parse Codex `--json` JSONL output.
///
/// Scans lines for `thread.started` (session ID) and `item.completed`
/// with `agent_message` content (review text).
fn parse_codex_jsonl(raw: &str) -> (Option<String>, Option<String>) {
    let mut session_id: Option<String> = None;
    let mut texts: Vec<String> = Vec::new();

    for line in raw.lines() {
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Extract thread_id from thread.started event.
        if val.get("type").and_then(|v| v.as_str()) == Some("thread.started") {
            if let Some(tid) = val.get("thread_id").and_then(|v| v.as_str()) {
                session_id = Some(tid.to_owned());
            }
            continue;
        }

        // Extract text from item.completed → agent_message.
        // Codex has two output formats:
        //   - item.text (direct text field on the item)
        //   - item.content[].output_text.text (array of content blocks)
        if val.get("type").and_then(|v| v.as_str()) == Some("item.completed")
            && let Some(item) = val.get("item")
            && item.get("type").and_then(|v| v.as_str()) == Some("agent_message")
        {
            // Format 1: direct text field
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                texts.push(text.to_owned());
            }
            // Format 2: content array with output_text entries
            else if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                for entry in content {
                    if entry.get("type").and_then(|v| v.as_str()) == Some("output_text")
                        && let Some(text) = entry.get("text").and_then(|v| v.as_str())
                    {
                        texts.push(text.to_owned());
                    }
                }
            }
        }
    }

    let markdown = if texts.is_empty() {
        None
    } else {
        Some(texts.join(""))
    };
    (session_id, markdown)
}

// ---------------------------------------------------------------------------
// Job
// ---------------------------------------------------------------------------

/// Job status reflecting the queue state machine:
///
/// ```text
/// Queued → Leased → Running → Succeeded | Failed | Canceled
///                     ↓
///                (crash/timeout)
///                     ↓
///               lease expired → re-queued (retry)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Leased,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl JobStatus {
    /// Whether the job is in a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }

    /// Parse from a string stored in the database.
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "leased" => Some(Self::Leased),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "canceled" => Some(Self::Canceled),
            _ => None,
        }
    }

    /// Serialize to a string for database storage.
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_db_str())
    }
}

/// A review job tracked in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub repo: RepoId,
    pub pr_number: u64,
    pub head_sha: String,
    pub agent_kind: AgentKind,
    pub status: JobStatus,
    pub leased_at: Option<DateTime<Utc>>,
    pub lease_expires: Option<DateTime<Utc>>,
    pub retry_count: i32,
    pub max_retries: i32,
    pub command: Option<String>,
    pub prompt_template: Option<String>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub review_output: Option<String>,
    pub session_id: Option<String>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Job {
    /// Whether a cancel has been requested for this job.
    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested_at.is_some()
    }
}

/// Data needed to create a new job (before it has an id).
#[derive(Debug, Clone)]
pub struct NewJob {
    pub repo: RepoId,
    pub pr_number: u64,
    pub head_sha: String,
    pub agent_kind: AgentKind,
    pub command: Option<String>,
    pub prompt_template: Option<String>,
    pub max_retries: i32,
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// The composite key used for idempotency checks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub repo: RepoId,
    pub pr_number: u64,
    pub head_sha: String,
    pub agent_kind: AgentKind,
}

// ---------------------------------------------------------------------------
// Filters / Results
// ---------------------------------------------------------------------------

/// Filter criteria for listing jobs.
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    pub status: Option<JobStatus>,
    pub repo: Option<RepoId>,
    pub pr_number: Option<u64>,
}

/// The result of a review execution.
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub exit_code: i32,
    pub review_markdown: Option<String>,
    pub session_id: Option<String>,
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_from_db_known_values() {
        assert_eq!(AgentKind::from_db("claude"), AgentKind::Claude);
        assert_eq!(AgentKind::from_db("codex"), AgentKind::Codex);
    }

    #[test]
    fn agent_kind_from_db_unknown_falls_back_to_claude() {
        assert_eq!(AgentKind::from_db("unknown"), AgentKind::Claude);
        assert_eq!(AgentKind::from_db(""), AgentKind::Claude);
        assert_eq!(AgentKind::from_db("custom:my-agent"), AgentKind::Claude);
    }

    #[test]
    fn agent_kind_roundtrip() {
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            assert_eq!(AgentKind::from_db(kind.as_db_str()), kind);
        }
    }

    #[test]
    fn agent_kind_default_command_contains_agent_name() {
        assert!(AgentKind::Claude.default_command(None).contains("claude"));
        assert!(AgentKind::Codex.default_command(None).contains("codex"));
    }

    #[test]
    fn default_command_with_model() {
        let claude_cmd = AgentKind::Claude.default_command(Some("claude-sonnet-4-5-20250514"));
        assert!(claude_cmd.contains("--model claude-sonnet-4-5-20250514"));
        assert!(claude_cmd.contains("claude -p"));

        let codex_cmd = AgentKind::Codex.default_command(Some("gpt-5.3-codex"));
        assert!(codex_cmd.contains("--model gpt-5.3-codex"));
        assert!(codex_cmd.contains("codex exec"));
    }

    #[test]
    fn default_command_without_model() {
        let claude_cmd = AgentKind::Claude.default_command(None);
        assert!(!claude_cmd.contains("--model"));
        assert!(claude_cmd.contains("{prompt_file}"));

        let codex_cmd = AgentKind::Codex.default_command(None);
        assert!(!codex_cmd.contains("--model"));
        assert!(codex_cmd.contains("{prompt_file}"));
    }

    #[test]
    fn parse_claude_json_valid() {
        let raw = r##"[{"type":"system","session_id":"abc-123"},{"type":"result","session_id":"abc-123","result":"# LGTM\nAll good."}]"##;
        let (sid, md) = AgentKind::Claude.parse_output(raw);
        assert_eq!(sid.as_deref(), Some("abc-123"));
        assert_eq!(md.as_deref(), Some("# LGTM\nAll good."));
    }

    #[test]
    fn parse_claude_json_array_format() {
        let raw = r##"[{"type":"system","subtype":"init","session_id":"sid-arr"},{"type":"assistant","session_id":"sid-arr"},{"type":"result","session_id":"sid-arr","result":"# Review\nLGTM"}]"##;
        let (sid, md) = AgentKind::Claude.parse_output(raw);
        assert_eq!(sid.as_deref(), Some("sid-arr"));
        assert_eq!(md.as_deref(), Some("# Review\nLGTM"));
    }

    #[test]
    fn parse_claude_json_malformed() {
        let (sid, md) = AgentKind::Claude.parse_output("not json at all");
        assert!(sid.is_none());
        assert!(md.is_none());
    }

    #[test]
    fn parse_claude_json_empty() {
        let (sid, md) = AgentKind::Claude.parse_output("");
        assert!(sid.is_none());
        assert!(md.is_none());
    }

    #[test]
    fn parse_codex_jsonl_valid() {
        let raw = r##"{"type":"thread.started","thread_id":"tid-999"}
{"type":"item.completed","item":{"type":"agent_message","content":[{"type":"output_text","text":"# Review\n"}]}}
{"type":"item.completed","item":{"type":"agent_message","content":[{"type":"output_text","text":"LGTM"}]}}"##;
        let (sid, md) = AgentKind::Codex.parse_output(raw);
        assert_eq!(sid.as_deref(), Some("tid-999"));
        assert_eq!(md.as_deref(), Some("# Review\nLGTM"));
    }

    #[test]
    fn parse_codex_jsonl_direct_text_format() {
        let raw = r##"{"type":"thread.started","thread_id":"tid-direct"}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"# Direct Review\nLGTM"}}"##;
        let (sid, md) = AgentKind::Codex.parse_output(raw);
        assert_eq!(sid.as_deref(), Some("tid-direct"));
        assert_eq!(md.as_deref(), Some("# Direct Review\nLGTM"));
    }

    #[test]
    fn parse_codex_jsonl_missing_thread_started() {
        let raw = r##"{"type":"item.completed","item":{"type":"agent_message","content":[{"type":"output_text","text":"hello"}]}}"##;
        let (sid, md) = AgentKind::Codex.parse_output(raw);
        assert!(sid.is_none());
        assert_eq!(md.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_codex_jsonl_empty() {
        let (sid, md) = AgentKind::Codex.parse_output("");
        assert!(sid.is_none());
        assert!(md.is_none());
    }

    #[test]
    fn resume_command_claude_without_path() {
        assert_eq!(
            AgentKind::Claude.resume_command("abc-123", None),
            "claude --resume abc-123"
        );
    }

    #[test]
    fn resume_command_codex_without_path() {
        assert_eq!(
            AgentKind::Codex.resume_command("tid-999", None),
            "codex exec resume tid-999"
        );
    }

    #[test]
    fn resume_command_with_repo_path() {
        let path = std::path::Path::new("/home/user/my-repo");
        assert_eq!(
            AgentKind::Claude.resume_command("abc-123", Some(path)),
            "cd /home/user/my-repo && claude --resume abc-123"
        );
        assert_eq!(
            AgentKind::Codex.resume_command("tid-999", Some(path)),
            "cd /home/user/my-repo && codex exec resume tid-999"
        );
    }

    #[test]
    fn job_status_terminal() {
        assert!(JobStatus::Succeeded.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(JobStatus::Canceled.is_terminal());
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Leased.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
    }
}
