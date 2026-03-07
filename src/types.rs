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
    pub fn default_command(&self) -> &'static str {
        match self {
            Self::Claude => r#"claude -p "$(cat "{prompt_file}")" --output-format json"#,
            Self::Codex => r#"codex exec --json --sandbox danger-full-access - < "{prompt_file}""#,
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
    pub fn resume_command(&self, session_id: &str) -> String {
        match self {
            Self::Claude => format!("claude --resume {session_id}"),
            Self::Codex => format!("codex exec resume {session_id}"),
        }
    }
}

/// Parse Claude `--output-format json` output.
///
/// Expects a single JSON object with `session_id` and `result` fields.
fn parse_claude_json(raw: &str) -> (Option<String>, Option<String>) {
    let val: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let session_id = val
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let markdown = val.get("result").and_then(|v| v.as_str()).map(String::from);
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

        // Extract text from item.completed → agent_message → content[].output_text.
        if val.get("type").and_then(|v| v.as_str()) == Some("item.completed")
            && let Some(item) = val.get("item")
            && item.get("type").and_then(|v| v.as_str()) == Some("agent_message")
            && let Some(content) = item.get("content").and_then(|v| v.as_array())
        {
            for entry in content {
                if entry.get("type").and_then(|v| v.as_str()) == Some("output_text")
                    && let Some(text) = entry.get("text").and_then(|v| v.as_str())
                {
                    texts.push(text.to_owned());
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
        assert!(AgentKind::Claude.default_command().contains("claude"));
        assert!(AgentKind::Codex.default_command().contains("codex"));
    }

    #[test]
    fn parse_claude_json_valid() {
        let raw = r##"{"session_id":"abc-123","result":"# LGTM\nAll good."}"##;
        let (sid, md) = AgentKind::Claude.parse_output(raw);
        assert_eq!(sid.as_deref(), Some("abc-123"));
        assert_eq!(md.as_deref(), Some("# LGTM\nAll good."));
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
    fn resume_command_claude() {
        assert_eq!(
            AgentKind::Claude.resume_command("abc-123"),
            "claude --resume abc-123"
        );
    }

    #[test]
    fn resume_command_codex() {
        assert_eq!(
            AgentKind::Codex.resume_command("tid-999"),
            "codex exec resume tid-999"
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
