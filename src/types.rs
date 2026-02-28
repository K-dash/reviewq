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
    Custom(String),
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Codex => write!(f, "codex"),
            Self::Custom(name) => write!(f, "{name}"),
        }
    }
}

impl AgentKind {
    /// Parse from a string stored in the database.
    pub fn from_db(s: &str) -> Self {
        match s {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            other => Self::Custom(other.to_owned()),
        }
    }

    /// Serialize to a string for database storage.
    pub fn as_db_str(&self) -> &str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Custom(s) => s,
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
