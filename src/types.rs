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
    pub fn default_command(&self) -> &'static str {
        match self {
            Self::Claude => {
                r#"set -o pipefail; claude -p "$(cat "{prompt_file}")" | tee "{output_path}""#
            }
            Self::Codex => {
                r#"set -o pipefail; codex exec --sandbox danger-full-access - < "{prompt_file}" | tee "{output_path}""#
            }
        }
    }
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
    fn job_status_terminal() {
        assert!(JobStatus::Succeeded.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(JobStatus::Canceled.is_terminal());
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Leased.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
    }
}
