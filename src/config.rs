//! YAML configuration parsing, validation, and defaults.
//!
//! The config is split into two concerns:
//!
//! - [`DaemonConfig`] — resources that exist once per reviewq installation
//!   (state DB, poll loop, auth credential, worktree root, global semaphore,
//!   logging directory, output directory). These are *not* overridable per
//!   repo because the daemon only has one of each.
//! - [`ReposConfig`] — repository-level policy. Fields set on `repos.defaults`
//!   act as a template for every entry in `repos.allowlist`; individual
//!   entries can override any field. Fields that neither `defaults` nor the
//!   entry sets fall back to a built-in constant.
//!
//! Example `config.yml`:
//!
//! ```yaml
//! daemon:
//!   polling:
//!     interval_seconds: 300
//!   auth:
//!     method: gh
//!     fallback_env: GITHUB_TOKEN
//!   execution:
//!     worktree_root: ~/.reviewq/worktrees
//!     max_concurrency: 10
//!     lease_minutes: 5
//!
//! repos:
//!   defaults:
//!     agent: codex
//!     base_repo_path: ~/src
//!   allowlist:
//!     - repo: org/repo-a
//!     - repo: org/repo-b
//!       agent: claude
//!       model: claude-sonnet-4-6
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Result, ReviewqError};

// ---------------------------------------------------------------------------
// Built-in defaults (absolute fallbacks when neither the entry nor
// `repos.defaults` sets a value).
// ---------------------------------------------------------------------------

const BUILTIN_SKIP_SELF_AUTHORED: bool = true;
const BUILTIN_SKIP_REVIEWER_CHECK: bool = false;
const BUILTIN_REVIEW_ON_PUSH: bool = true;

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// Top-level reviewq configuration.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub repos: ReposConfig,
}

// ---------------------------------------------------------------------------
// Daemon-scoped configuration
// ---------------------------------------------------------------------------

/// Daemon-wide settings. One-per-install resources.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonConfig {
    #[serde(default)]
    pub polling: PollingConfig,

    #[serde(default)]
    pub auth: AuthConfig,

    #[serde(default)]
    pub execution: ExecutionConfig,

    #[serde(default)]
    pub cancel: CancelConfig,

    #[serde(default)]
    pub cleanup: CleanupConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub state: StateConfig,

    #[serde(default)]
    pub output: OutputConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollingConfig {
    #[serde(default = "default_polling_interval")]
    pub interval_seconds: u64,
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            interval_seconds: default_polling_interval(),
        }
    }
}

fn default_polling_interval() -> u64 {
    300
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    #[serde(default = "default_auth_method")]
    pub method: String,

    #[serde(default = "default_fallback_env")]
    pub fallback_env: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            method: default_auth_method(),
            fallback_env: default_fallback_env(),
        }
    }
}

fn default_auth_method() -> String {
    "gh".to_owned()
}

fn default_fallback_env() -> String {
    "GITHUB_TOKEN".to_owned()
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
    pub worktree_root: Option<PathBuf>,

    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,

    #[serde(default = "default_lease_minutes")]
    pub lease_minutes: i64,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            worktree_root: None,
            max_concurrency: default_max_concurrency(),
            lease_minutes: default_lease_minutes(),
        }
    }
}

/// Default worktree root directory (`~/.reviewq/worktrees`).
///
/// Placed outside any repository tree so that the review agent does not
/// accidentally pick up CLAUDE.md / AGENTS.md from the host project.
fn default_worktree_root() -> PathBuf {
    PathBuf::from("~/.reviewq/worktrees")
}

impl ExecutionConfig {
    /// Resolve the effective worktree root directory.
    ///
    /// Priority: explicit `worktree_root` config > default `~/.reviewq/worktrees`.
    /// Expands leading `~` to the user's home directory.
    pub fn effective_worktree_root(&self) -> PathBuf {
        let mut path = self
            .worktree_root
            .clone()
            .unwrap_or_else(default_worktree_root);
        if let Some(home) = dirs::home_dir() {
            expand_tilde(&mut path, &home);
        }
        path
    }
}

fn default_max_concurrency() -> usize {
    10
}

fn default_lease_minutes() -> i64 {
    5
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelConfig {
    #[serde(default = "default_sigint_timeout")]
    pub sigint_timeout_seconds: u64,

    #[serde(default = "default_sigterm_timeout")]
    pub sigterm_timeout_seconds: u64,

    #[serde(default = "default_sigkill_timeout")]
    pub sigkill_timeout_seconds: u64,
}

impl Default for CancelConfig {
    fn default() -> Self {
        Self {
            sigint_timeout_seconds: default_sigint_timeout(),
            sigterm_timeout_seconds: default_sigterm_timeout(),
            sigkill_timeout_seconds: default_sigkill_timeout(),
        }
    }
}

fn default_sigint_timeout() -> u64 {
    5
}

fn default_sigterm_timeout() -> u64 {
    15
}

fn default_sigkill_timeout() -> u64 {
    5
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupConfig {
    #[serde(default = "default_cleanup_ttl")]
    pub ttl_minutes: u64,

    #[serde(default = "default_cleanup_interval")]
    pub interval_minutes: u64,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            ttl_minutes: default_cleanup_ttl(),
            interval_minutes: default_cleanup_interval(),
        }
    }
}

fn default_cleanup_ttl() -> u64 {
    1440
}

fn default_cleanup_interval() -> u64 {
    30
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default = "default_log_dir")]
    pub dir: PathBuf,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            dir: default_log_dir(),
        }
    }
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("~/.reviewq/logs")
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateConfig {
    #[serde(default = "default_sqlite_path")]
    pub sqlite_path: PathBuf,
}

impl Default for StateConfig {
    fn default() -> Self {
        Self {
            sqlite_path: default_sqlite_path(),
        }
    }
}

fn default_sqlite_path() -> PathBuf {
    PathBuf::from("~/.reviewq/state.db")
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    #[serde(default = "default_output_dir")]
    pub dir: PathBuf,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            dir: default_output_dir(),
        }
    }
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("~/.reviewq/output")
}

// ---------------------------------------------------------------------------
// Repos / per-repo policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReposConfig {
    /// Template values that apply to every entry in `allowlist` unless the
    /// entry overrides the same field.
    #[serde(default)]
    pub defaults: RepoDefaults,

    /// Explicit list of repositories reviewq is allowed to process.
    #[serde(default)]
    pub allowlist: Vec<RepoEntry>,
}

/// User-level defaults for repo policy fields.
///
/// Every field is `Option<T>`. Unset fields fall back to built-in constants
/// during resolution (see [`Config::repo_policies`]).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoDefaults {
    #[serde(default)]
    pub skip_self_authored: Option<bool>,

    #[serde(default)]
    pub skip_reviewer_check: Option<bool>,

    #[serde(default)]
    pub review_on_push: Option<bool>,

    #[serde(default)]
    pub agent: Option<crate::types::AgentKind>,

    #[serde(default)]
    pub prompt_template: Option<String>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub base_repo_path: Option<PathBuf>,

    #[serde(default)]
    pub ignore_prs: Option<Vec<u64>>,
}

/// Per-repository configuration entry in the YAML allowlist.
///
/// ```yaml
/// repos:
///   allowlist:
///     - repo: "owner/name"
///       skip_self_authored: false
///       skip_reviewer_check: true
///       review_on_push: false
///       agent: codex
///       model: gpt-5.3-codex
///       base_repo_path: "/path/to/local/clone"
///       ignore_prs: [123, 456]
/// ```
///
/// Any field left unset inherits from `repos.defaults`, and any field unset
/// there falls back to a built-in constant.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoEntry {
    /// Repository in `"owner/name"` format.
    pub repo: String,

    #[serde(default)]
    pub skip_self_authored: Option<bool>,

    #[serde(default)]
    pub skip_reviewer_check: Option<bool>,

    #[serde(default)]
    pub review_on_push: Option<bool>,

    #[serde(default)]
    pub agent: Option<crate::types::AgentKind>,

    #[serde(default)]
    pub prompt_template: Option<String>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub base_repo_path: Option<PathBuf>,

    #[serde(default)]
    pub ignore_prs: Option<Vec<u64>>,
}

/// Per-repository policy with every field resolved to a concrete value.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoPolicy {
    pub id: crate::types::RepoId,
    pub skip_self_authored: bool,
    pub skip_reviewer_check: bool,
    pub review_on_push: bool,
    pub agent: crate::types::AgentKind,
    pub prompt_template: Option<String>,
    pub model: Option<String>,
    pub base_repo_path: Option<PathBuf>,
    pub ignore_prs: Vec<u64>,
}

// ---------------------------------------------------------------------------
// Loading, validation, resolution
// ---------------------------------------------------------------------------

impl Config {
    /// Load configuration from a YAML file.
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            ReviewqError::Config(format!(
                "failed to read config file {}: {e}",
                path.display()
            ))
        })?;
        Self::from_yaml(&contents)
    }

    /// Parse configuration from a YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let config: Config = serde_yml::from_str(yaml)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values.
    fn validate(&self) -> Result<()> {
        if self.repos.allowlist.is_empty() {
            return Err(ReviewqError::Config(
                "repos.allowlist must contain at least one repository".into(),
            ));
        }

        let mut seen = std::collections::HashSet::new();
        for entry in &self.repos.allowlist {
            if !entry.repo.contains('/') {
                return Err(ReviewqError::Config(format!(
                    "invalid repo format '{}': expected 'owner/name'",
                    entry.repo
                )));
            }
            if !seen.insert(&entry.repo) {
                return Err(ReviewqError::Config(format!(
                    "duplicate repo '{}' in allowlist",
                    entry.repo
                )));
            }
        }

        // Every allowlisted repo must resolve a base_repo_path — either
        // directly on the entry or inherited from repos.defaults. Without
        // this, the runner and cleanup loop fall back to the process cwd,
        // which makes reviewq's behavior silently depend on where the
        // daemon was started. The filesystem-level check (path exists,
        // path is a directory) lives in `validate_paths`, which is called
        // separately by daemon / TUI startup after `expand_paths`.
        let defaults_base = self.repos.defaults.base_repo_path.is_some();
        for entry in &self.repos.allowlist {
            if entry.base_repo_path.is_none() && !defaults_base {
                return Err(ReviewqError::Config(format!(
                    "base_repo_path is required for repo '{}': set repos.defaults.base_repo_path or repos.allowlist[].base_repo_path",
                    entry.repo
                )));
            }
        }

        // Validate model names (defaults and per-repo).
        if let Some(ref m) = self.repos.defaults.model
            && !is_valid_model_name(m)
        {
            return Err(ReviewqError::Config(format!(
                "invalid repos.defaults.model '{m}': must match [A-Za-z0-9._:-]+"
            )));
        }
        for entry in &self.repos.allowlist {
            if let Some(ref m) = entry.model
                && !is_valid_model_name(m)
            {
                return Err(ReviewqError::Config(format!(
                    "invalid model '{}' for repo '{}': must match [A-Za-z0-9._:-]+",
                    m, entry.repo
                )));
            }
        }

        if self.daemon.polling.interval_seconds == 0 {
            return Err(ReviewqError::Config(
                "daemon.polling.interval_seconds must be > 0".into(),
            ));
        }

        Ok(())
    }

    /// Verify that every resolved `base_repo_path` exists on disk and is a
    /// directory. Must be called **after** [`expand_paths`] so tilde paths
    /// are resolved. Not called from [`from_yaml`] — unit tests that build
    /// configs from in-memory YAML do not need real directories.
    ///
    /// This is called only from the daemon and TUI startup paths. Read-only
    /// subcommands (`status` / `tail` / `open`) skip this check so a broken
    /// filesystem path does not stop the user from inspecting job history.
    pub fn validate_paths(&self) -> Result<()> {
        for policy in self.repo_policies() {
            let Some(path) = policy.base_repo_path.as_ref() else {
                // Structurally invalid — validate() should have caught this.
                // Guard defensively rather than panicking.
                return Err(ReviewqError::Config(format!(
                    "base_repo_path is required for repo '{}'",
                    policy.id
                )));
            };
            if !path.exists() {
                return Err(ReviewqError::Config(format!(
                    "base_repo_path for repo '{}' does not exist: {}",
                    policy.id,
                    path.display()
                )));
            }
            if !path.is_dir() {
                return Err(ReviewqError::Config(format!(
                    "base_repo_path for repo '{}' is not a directory: {}",
                    policy.id,
                    path.display()
                )));
            }
        }
        Ok(())
    }

    /// Expand `~` in paths to the user's home directory.
    pub fn expand_paths(&mut self) {
        let Some(home) = dirs::home_dir() else { return };
        expand_tilde(&mut self.daemon.logging.dir, &home);
        expand_tilde(&mut self.daemon.state.sqlite_path, &home);
        expand_tilde(&mut self.daemon.output.dir, &home);
        if let Some(ref mut p) = self.daemon.execution.worktree_root {
            expand_tilde(p, &home);
        }
        if let Some(ref mut p) = self.repos.defaults.base_repo_path {
            expand_tilde(p, &home);
        }
        for entry in &mut self.repos.allowlist {
            if let Some(ref mut p) = entry.base_repo_path {
                expand_tilde(p, &home);
            }
        }
    }

    /// Resolve the allowlist into concrete per-repository policies.
    ///
    /// Resolution chain for each field: `RepoEntry` → `repos.defaults` →
    /// built-in constant.
    pub fn repo_policies(&self) -> Vec<RepoPolicy> {
        let d = &self.repos.defaults;
        self.repos
            .allowlist
            .iter()
            .filter_map(|entry| {
                let (owner, name) = entry.repo.split_once('/')?;
                Some(RepoPolicy {
                    id: crate::types::RepoId::new(owner, name),
                    skip_self_authored: entry
                        .skip_self_authored
                        .or(d.skip_self_authored)
                        .unwrap_or(BUILTIN_SKIP_SELF_AUTHORED),
                    skip_reviewer_check: entry
                        .skip_reviewer_check
                        .or(d.skip_reviewer_check)
                        .unwrap_or(BUILTIN_SKIP_REVIEWER_CHECK),
                    review_on_push: entry
                        .review_on_push
                        .or(d.review_on_push)
                        .unwrap_or(BUILTIN_REVIEW_ON_PUSH),
                    agent: entry
                        .agent
                        .clone()
                        .or_else(|| d.agent.clone())
                        .unwrap_or_default(),
                    prompt_template: entry
                        .prompt_template
                        .clone()
                        .or_else(|| d.prompt_template.clone()),
                    model: entry.model.clone().or_else(|| d.model.clone()),
                    base_repo_path: entry
                        .base_repo_path
                        .clone()
                        .or_else(|| d.base_repo_path.clone()),
                    ignore_prs: entry
                        .ignore_prs
                        .clone()
                        .or_else(|| d.ignore_prs.clone())
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Extract just the repo IDs from the allowlist.
    pub fn repo_ids(&self) -> Vec<crate::types::RepoId> {
        self.repo_policies().into_iter().map(|p| p.id).collect()
    }

    /// Resolve the effective local clone path for a given repository.
    ///
    /// Priority: `RepoEntry.base_repo_path` > `repos.defaults.base_repo_path`.
    ///
    /// This re-runs `repo_policies()` on every call — intentionally simple
    /// because it's called from convenience and test paths, never from the
    /// runner's hot loop. The runner uses the already-materialized
    /// `&[RepoPolicy]` slice directly.
    pub fn base_repo_for(&self, repo: &crate::types::RepoId) -> Option<PathBuf> {
        self.repo_policies()
            .into_iter()
            .find(|p| &p.id == repo)
            .and_then(|p| p.base_repo_path)
    }

    /// Compare two configs and return human-readable change descriptions.
    ///
    /// Also flags fields that require a restart to take effect.
    pub fn diff_summary(old: &Config, new: &Config) -> Vec<String> {
        let mut changes = Vec::new();

        // --- daemon ---
        if old.daemon.polling != new.daemon.polling {
            changes.push(format!(
                "daemon.polling.interval_seconds changed: {} -> {}",
                old.daemon.polling.interval_seconds, new.daemon.polling.interval_seconds
            ));
        }

        if old.daemon.auth != new.daemon.auth {
            changes.push("daemon.auth changed (restart required)".to_string());
        }

        if old.daemon.execution.max_concurrency != new.daemon.execution.max_concurrency {
            changes.push(format!(
                "daemon.execution.max_concurrency changed: {} -> {} (restart required)",
                old.daemon.execution.max_concurrency, new.daemon.execution.max_concurrency
            ));
        }

        if old.daemon.execution.worktree_root != new.daemon.execution.worktree_root {
            changes.push(format!(
                "daemon.execution.worktree_root changed: {:?} -> {:?}",
                old.daemon.execution.worktree_root, new.daemon.execution.worktree_root
            ));
        }

        if old.daemon.execution.lease_minutes != new.daemon.execution.lease_minutes {
            changes.push(format!(
                "daemon.execution.lease_minutes changed: {} -> {} (restart required)",
                old.daemon.execution.lease_minutes, new.daemon.execution.lease_minutes
            ));
        }

        if old.daemon.cancel != new.daemon.cancel {
            changes.push("daemon.cancel changed (restart required)".to_string());
        }

        if old.daemon.cleanup != new.daemon.cleanup {
            changes.push(format!(
                "daemon.cleanup changed: ttl={}->{}min, interval={}->{}min",
                old.daemon.cleanup.ttl_minutes,
                new.daemon.cleanup.ttl_minutes,
                old.daemon.cleanup.interval_minutes,
                new.daemon.cleanup.interval_minutes
            ));
        }

        if old.daemon.logging != new.daemon.logging {
            changes.push("daemon.logging changed (restart required)".to_string());
        }

        if old.daemon.state != new.daemon.state {
            changes.push("daemon.state changed (restart required)".to_string());
        }

        if old.daemon.output != new.daemon.output {
            changes.push(format!(
                "daemon.output.dir changed: {:?} -> {:?}",
                old.daemon.output.dir, new.daemon.output.dir
            ));
        }

        // --- repos ---
        if old.repos.defaults != new.repos.defaults {
            diff_repo_defaults(&old.repos.defaults, &new.repos.defaults, &mut changes);
        }

        if old.repos.allowlist != new.repos.allowlist {
            let old_repos: Vec<&str> = old
                .repos
                .allowlist
                .iter()
                .map(|e| e.repo.as_str())
                .collect();
            let new_repos: Vec<&str> = new
                .repos
                .allowlist
                .iter()
                .map(|e| e.repo.as_str())
                .collect();
            if old_repos != new_repos {
                changes.push(format!(
                    "repos.allowlist changed: {old_repos:?} -> {new_repos:?}"
                ));
            }
            // Report per-repo review_on_push changes specifically so the user
            // sees exactly which repo toggled the flag.
            for new_entry in &new.repos.allowlist {
                if let Some(old_entry) = old
                    .repos
                    .allowlist
                    .iter()
                    .find(|e| e.repo == new_entry.repo)
                    .filter(|old_entry| old_entry.review_on_push != new_entry.review_on_push)
                {
                    changes.push(format!(
                        "repos.allowlist[{}].review_on_push changed: {:?} -> {:?}",
                        new_entry.repo, old_entry.review_on_push, new_entry.review_on_push
                    ));
                }
            }
            // Fallback: if repo list is the same but other per-repo settings
            // changed, emit a generic line so the change isn't swallowed.
            if old_repos == new_repos {
                let has_other_changes = new.repos.allowlist.iter().any(|new_entry| {
                    old.repos
                        .allowlist
                        .iter()
                        .find(|e| e.repo == new_entry.repo)
                        .is_some_and(|old_entry| {
                            old_entry.skip_self_authored != new_entry.skip_self_authored
                                || old_entry.skip_reviewer_check != new_entry.skip_reviewer_check
                                || old_entry.agent != new_entry.agent
                                || old_entry.prompt_template != new_entry.prompt_template
                                || old_entry.model != new_entry.model
                                || old_entry.base_repo_path != new_entry.base_repo_path
                                || old_entry.ignore_prs != new_entry.ignore_prs
                        })
                });
                if has_other_changes {
                    changes.push("repos.allowlist per-repo settings changed".to_string());
                }
            }
        }

        changes
    }
}

/// Emit a line per field that differs between the two `RepoDefaults`
/// instances so hot-reload observers can see *which* default flipped.
fn diff_repo_defaults(old: &RepoDefaults, new: &RepoDefaults, changes: &mut Vec<String>) {
    if old.skip_self_authored != new.skip_self_authored {
        changes.push(format!(
            "repos.defaults.skip_self_authored changed: {:?} -> {:?}",
            old.skip_self_authored, new.skip_self_authored
        ));
    }
    if old.skip_reviewer_check != new.skip_reviewer_check {
        changes.push(format!(
            "repos.defaults.skip_reviewer_check changed: {:?} -> {:?}",
            old.skip_reviewer_check, new.skip_reviewer_check
        ));
    }
    if old.review_on_push != new.review_on_push {
        changes.push(format!(
            "repos.defaults.review_on_push changed: {:?} -> {:?}",
            old.review_on_push, new.review_on_push
        ));
    }
    if old.agent != new.agent {
        changes.push(format!(
            "repos.defaults.agent changed: {:?} -> {:?}",
            old.agent, new.agent
        ));
    }
    if old.prompt_template != new.prompt_template {
        changes.push(format!(
            "repos.defaults.prompt_template changed: {:?} -> {:?}",
            old.prompt_template, new.prompt_template
        ));
    }
    if old.model != new.model {
        changes.push(format!(
            "repos.defaults.model changed: {:?} -> {:?}",
            old.model, new.model
        ));
    }
    if old.base_repo_path != new.base_repo_path {
        changes.push(format!(
            "repos.defaults.base_repo_path changed: {:?} -> {:?}",
            old.base_repo_path, new.base_repo_path
        ));
    }
    if old.ignore_prs != new.ignore_prs {
        changes.push(format!(
            "repos.defaults.ignore_prs changed: {:?} -> {:?}",
            old.ignore_prs, new.ignore_prs
        ));
    }
}

/// Check if a model name contains only allowed characters: `[A-Za-z0-9._:-]+`.
fn is_valid_model_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b':' || b == b'-')
}

/// Replace a leading `~` with the home directory.
fn expand_tilde(path: &mut PathBuf, home: &Path) {
    if let Ok(stripped) = path.strip_prefix("~") {
        *path = home.join(stripped);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, RepoId};

    // -- parsing --------------------------------------------------------

    #[test]
    fn parse_minimal_config() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: owner/repo
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.repos.allowlist.len(), 1);
        assert_eq!(config.repos.allowlist[0].repo, "owner/repo");
        assert!(config.repos.allowlist[0].skip_self_authored.is_none());
        assert!(config.repos.allowlist[0].agent.is_none());
        assert_eq!(config.daemon.polling.interval_seconds, 300);
        assert_eq!(config.daemon.execution.max_concurrency, 10);
    }

    #[test]
    fn parse_per_repo_overrides() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo1
      skip_self_authored: false
      agent: codex
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.repos.allowlist.len(), 2);

        let e0 = &config.repos.allowlist[0];
        assert_eq!(e0.repo, "org/repo1");
        assert_eq!(e0.skip_self_authored, Some(false));
        assert_eq!(e0.agent, Some(AgentKind::Codex));

        let e1 = &config.repos.allowlist[1];
        assert_eq!(e1.repo, "org/repo2");
        assert!(e1.skip_self_authored.is_none());
        assert!(e1.agent.is_none());
    }

    #[test]
    fn parse_full_daemon_and_repos() {
        let yaml = r#"
daemon:
  polling:
    interval_seconds: 60
  auth:
    method: gh
    fallback_env: GITHUB_TOKEN
  execution:
    max_concurrency: 5
    lease_minutes: 10
  cancel:
    sigint_timeout_seconds: 3
    sigterm_timeout_seconds: 10
    sigkill_timeout_seconds: 3
  cleanup:
    ttl_minutes: 720
    interval_minutes: 15

repos:
  defaults:
    agent: codex
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo1
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.daemon.polling.interval_seconds, 60);
        assert_eq!(config.daemon.execution.max_concurrency, 5);
        assert_eq!(config.daemon.execution.lease_minutes, 10);
        assert_eq!(config.daemon.cancel.sigint_timeout_seconds, 3);
        assert_eq!(config.daemon.cleanup.ttl_minutes, 720);
        assert_eq!(config.repos.defaults.agent, Some(AgentKind::Codex));
    }

    // -- resolution chain -----------------------------------------------

    #[test]
    fn resolve_falls_back_to_builtin_when_nothing_set() {
        // `base_repo_path` is required, so set a minimal defaults value.
        // Every *other* field (agent, model, prompt, skip flags, ignore)
        // falls back to the built-in defaults.
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        let policies = config.repo_policies();
        assert_eq!(policies.len(), 1);
        let p = &policies[0];
        assert_eq!(p.id, RepoId::new("org", "repo"));
        assert!(p.skip_self_authored);
        assert!(!p.skip_reviewer_check);
        assert!(p.review_on_push);
        assert_eq!(p.agent, AgentKind::Claude);
        assert!(p.prompt_template.is_none());
        assert!(p.model.is_none());
        assert_eq!(p.base_repo_path, Some(PathBuf::from("/tmp/fake")));
        assert!(p.ignore_prs.is_empty());
    }

    #[test]
    fn resolve_uses_repos_defaults_when_entry_is_silent() {
        let yaml = r#"
repos:
  defaults:
    agent: codex
    model: gpt-5.3-codex
    prompt_template: "Review {pr_url}"
    base_repo_path: /shared/src
    skip_self_authored: false
    ignore_prs: [1, 2]
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        let p = &config.repo_policies()[0];
        assert_eq!(p.agent, AgentKind::Codex);
        assert_eq!(p.model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(p.prompt_template.as_deref(), Some("Review {pr_url}"));
        assert_eq!(p.base_repo_path, Some(PathBuf::from("/shared/src")));
        assert!(!p.skip_self_authored);
        assert_eq!(p.ignore_prs, vec![1, 2]);
    }

    #[test]
    fn resolve_entry_overrides_defaults() {
        let yaml = r#"
repos:
  defaults:
    agent: codex
    model: gpt-5.3-codex
    base_repo_path: /shared/src
  allowlist:
    - repo: org/repo-a
    - repo: org/repo-b
      agent: claude
      model: claude-sonnet-4-6
      base_repo_path: /custom/src
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        let policies = config.repo_policies();
        assert_eq!(policies.len(), 2);

        // repo-a inherits everything from defaults
        assert_eq!(policies[0].agent, AgentKind::Codex);
        assert_eq!(policies[0].model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(
            policies[0].base_repo_path,
            Some(PathBuf::from("/shared/src"))
        );

        // repo-b overrides
        assert_eq!(policies[1].agent, AgentKind::Claude);
        assert_eq!(policies[1].model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(
            policies[1].base_repo_path,
            Some(PathBuf::from("/custom/src"))
        );
    }

    #[test]
    fn builtin_skip_self_authored_is_true() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        assert!(config.repo_policies()[0].skip_self_authored);
    }

    #[test]
    fn builtin_review_on_push_is_true() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        assert!(config.repo_policies()[0].review_on_push);
    }

    #[test]
    fn entry_can_disable_review_on_push() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
      review_on_push: false
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        assert!(!config.repo_policies()[0].review_on_push);
    }

    #[test]
    fn repo_ids_extracts_ids() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo1
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        let ids = config.repo_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], RepoId::new("org", "repo1"));
        assert_eq!(ids[1], RepoId::new("org", "repo2"));
    }

    #[test]
    fn base_repo_for_prefers_entry_then_defaults() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /shared/src
  allowlist:
    - repo: org/repo-a
    - repo: org/repo-b
      base_repo_path: /custom/src
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        assert_eq!(
            config.base_repo_for(&RepoId::new("org", "repo-a")),
            Some(PathBuf::from("/shared/src"))
        );
        assert_eq!(
            config.base_repo_for(&RepoId::new("org", "repo-b")),
            Some(PathBuf::from("/custom/src"))
        );
        assert_eq!(config.base_repo_for(&RepoId::new("org", "unknown")), None);
    }

    #[test]
    fn base_repo_for_returns_none_for_unknown_repo() {
        // The original version of this test documented the "nothing set"
        // path that returned `None`. With base_repo_path now mandatory,
        // that state is structurally invalid, so instead assert that an
        // *unknown* repo id still returns `None` even when the allowlist
        // has resolved paths.
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        assert_eq!(
            config.base_repo_for(&RepoId::new("org", "repo")),
            Some(PathBuf::from("/tmp/fake"))
        );
        assert_eq!(config.base_repo_for(&RepoId::new("org", "unknown")), None);
    }

    // -- validation -----------------------------------------------------

    #[test]
    fn reject_empty_allowlist() {
        let yaml = r#"
repos:
  allowlist: []
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("allowlist"));
    }

    #[test]
    fn reject_invalid_repo_format() {
        let yaml = r#"
repos:
  allowlist:
    - repo: just-a-name
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("owner/name"));
    }

    #[test]
    fn reject_duplicate_repo() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
    - repo: org/repo
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("duplicate repo"));
    }

    #[test]
    fn valid_model_names_accepted() {
        for name in [
            "claude-sonnet-4-5-20250514",
            "gpt-5.3-codex",
            "gpt-5.4",
            "model:v1.2",
            "a_b-c.d:e",
        ] {
            let yaml = format!(
                "repos:\n  defaults:\n    base_repo_path: /tmp/fake\n    model: {name}\n  allowlist:\n    - repo: org/repo\n"
            );
            Config::from_yaml(&yaml)
                .unwrap_or_else(|e| panic!("model '{name}' should be valid: {e}"));
        }
    }

    #[test]
    fn invalid_defaults_model_rejected() {
        for name in ["model name", "model;rm", "$(echo hi)", "mod\"el", ""] {
            let yaml = format!(
                "repos:\n  defaults:\n    base_repo_path: /tmp/fake\n    model: \"{name}\"\n  allowlist:\n    - repo: org/repo\n"
            );
            assert!(
                Config::from_yaml(&yaml).is_err(),
                "model '{name}' should be rejected"
            );
        }
    }

    #[test]
    fn invalid_per_repo_model_rejected() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
      model: "bad model"
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("invalid model"));
    }

    #[test]
    fn reject_unknown_top_level_key() {
        // The old schema used `runner:` / `polling:` / etc. at the top level.
        // With the new schema they must live under `daemon:` and the old
        // layout should fail to parse.
        let yaml = r#"
runner:
  agent: claude
repos:
  allowlist:
    - repo: org/repo
"#;
        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn reject_zero_polling_interval() {
        let yaml = r#"
daemon:
  polling:
    interval_seconds: 0
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("polling"));
    }

    // -- diff_summary ---------------------------------------------------

    #[test]
    fn diff_summary_no_changes() {
        let yaml = r#"
daemon:
  polling:
    interval_seconds: 60
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        let changes = Config::diff_summary(&config, &config);
        assert!(changes.is_empty());
    }

    #[test]
    fn diff_summary_detects_polling_change() {
        let old = Config::from_yaml(
            r#"
daemon:
  polling:
    interval_seconds: 60
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
daemon:
  polling:
    interval_seconds: 120
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("daemon.polling.interval_seconds"));
        assert!(changes[0].contains("60"));
        assert!(changes[0].contains("120"));
    }

    #[test]
    fn diff_summary_detects_max_concurrency_restart_required() {
        let old = Config::from_yaml(
            r#"
daemon:
  execution:
    max_concurrency: 5
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
daemon:
  execution:
    max_concurrency: 20
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(
            changes
                .iter()
                .any(|c| c.contains("max_concurrency") && c.contains("restart required"))
        );
    }

    #[test]
    fn diff_summary_detects_repo_list_change() {
        let old = Config::from_yaml(
            r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo1
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo2
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(changes.iter().any(|c| c.contains("repos.allowlist")));
    }

    #[test]
    fn diff_summary_detects_review_on_push_change() {
        let old = Config::from_yaml(
            r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
      review_on_push: true
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
      review_on_push: false
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(changes.iter().any(|c| c.contains("review_on_push")));
    }

    #[test]
    fn diff_summary_detects_repos_defaults_agent_change() {
        let old = Config::from_yaml(
            r#"
repos:
  defaults:
    agent: claude
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  defaults:
    agent: codex
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        // Must name the specific field that flipped, not a coarse
        // "repos.defaults changed" line.
        assert!(
            changes.iter().any(|c| c.contains("repos.defaults.agent")
                && c.contains("Claude")
                && c.contains("Codex")),
            "expected field-level diff for agent: {changes:?}"
        );
    }

    #[test]
    fn diff_summary_detects_repos_defaults_model_and_prompt_changes() {
        let old = Config::from_yaml(
            r#"
repos:
  defaults:
    model: gpt-5.3-codex
    prompt_template: "old"
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  defaults:
    model: gpt-5.4
    prompt_template: "new"
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(
            changes.iter().any(|c| c.contains("repos.defaults.model")),
            "expected model diff: {changes:?}"
        );
        assert!(
            changes
                .iter()
                .any(|c| c.contains("repos.defaults.prompt_template")),
            "expected prompt_template diff: {changes:?}"
        );
    }

    #[test]
    fn diff_summary_detects_per_repo_settings_change_fallback() {
        let old = Config::from_yaml(
            r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
      model: gpt-5.4
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
      model: gpt-5.3-codex
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(
            changes
                .iter()
                .any(|c| c.contains("per-repo settings changed"))
        );
    }

    // -- base_repo_path mandatory validation ----------------------------

    #[test]
    fn validate_rejects_missing_base_repo_path() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("base_repo_path"),
            "error should mention base_repo_path, got: {msg}"
        );
        assert!(
            msg.contains("org/repo"),
            "error should name the offending repo, got: {msg}"
        );
    }

    #[test]
    fn validate_accepts_defaults_only_base_repo_path() {
        let yaml = r#"
repos:
  defaults:
    base_repo_path: /tmp/fake
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(
            config.repos.defaults.base_repo_path,
            Some(PathBuf::from("/tmp/fake"))
        );
    }

    #[test]
    fn validate_accepts_per_entry_base_repo_path() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
      base_repo_path: /tmp/entry
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(
            config.repos.allowlist[0].base_repo_path,
            Some(PathBuf::from("/tmp/entry"))
        );
    }

    #[test]
    fn validate_rejects_missing_base_repo_path_for_one_of_many() {
        // Defaults are empty. repo-a supplies its own base_repo_path,
        // but repo-b has nothing to inherit, so validation must point
        // at repo-b specifically rather than accepting the allowlist
        // because "at least one entry has a path set."
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo-a
      base_repo_path: /tmp/a
    - repo: org/repo-b
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("org/repo-b"),
            "error should name repo-b specifically, got: {msg}"
        );
    }

    // -- validate_paths (filesystem checks) -----------------------------

    #[test]
    fn validate_paths_accepts_existing_directory() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let yaml = format!(
            "repos:\n  defaults:\n    base_repo_path: {}\n  allowlist:\n    - repo: org/repo\n",
            tmp.path().display()
        );
        let config = Config::from_yaml(&yaml).expect("parse");
        config.validate_paths().expect("should accept existing dir");
    }

    #[test]
    fn validate_paths_rejects_nonexistent_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let missing = tmp.path().join("nowhere");
        let yaml = format!(
            "repos:\n  defaults:\n    base_repo_path: {}\n  allowlist:\n    - repo: org/repo\n",
            missing.display()
        );
        let config = Config::from_yaml(&yaml).expect("parse");
        let err = config.validate_paths().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("does not exist") || msg.contains("not a directory"),
            "error should explain the path problem, got: {msg}"
        );
        assert!(
            msg.contains("org/repo"),
            "error should name the repo, got: {msg}"
        );
    }

    #[test]
    fn validate_paths_rejects_regular_file() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let file = tmp.path().join("not-a-dir.txt");
        std::fs::write(&file, b"hello").expect("write file");
        let yaml = format!(
            "repos:\n  defaults:\n    base_repo_path: {}\n  allowlist:\n    - repo: org/repo\n",
            file.display()
        );
        let config = Config::from_yaml(&yaml).expect("parse");
        let err = config.validate_paths().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a directory") || msg.contains("does not exist"),
            "error should explain the is_dir check, got: {msg}"
        );
    }

    // -- expand_paths ---------------------------------------------------

    #[test]
    fn expand_paths_expands_tilde_in_base_repo_paths() {
        // Skip the test when the home directory cannot be resolved (e.g. in
        // sandboxed CI).
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let yaml = r#"
repos:
  defaults:
    base_repo_path: ~/src/defaults
  allowlist:
    - repo: org/repo
      base_repo_path: ~/src/custom
"#;
        let mut config = Config::from_yaml(yaml).expect("parse");
        config.expand_paths();
        assert_eq!(
            config.repos.defaults.base_repo_path.as_deref(),
            Some(home.join("src/defaults").as_path())
        );
        assert_eq!(
            config.repos.allowlist[0].base_repo_path.as_deref(),
            Some(home.join("src/custom").as_path())
        );
    }
}
