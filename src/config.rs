//! YAML configuration parsing, validation, and defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Result, ReviewqError};

/// Top-level configuration for reviewq.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub repos: ReposConfig,

    #[serde(default)]
    pub polling: PollingConfig,

    #[serde(default)]
    pub auth: AuthConfig,

    #[serde(default)]
    pub execution: ExecutionConfig,

    #[serde(default)]
    pub runner: RunnerConfig,

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

// ---------------------------------------------------------------------------
// Sub-configs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReposConfig {
    #[serde(default)]
    pub allowlist: Vec<RepoEntry>,
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
///       command: "claude code review"
///       max_concurrency: 3
///       base_repo_path: "/path/to/local/clone"
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoEntry {
    /// Repository in `"owner/name"` format.
    pub repo: String,

    /// Skip PRs authored by the authenticated user. Default: true.
    #[serde(default = "default_true")]
    pub skip_self_authored: bool,

    /// Process all open PRs regardless of reviewer assignment. Default: false.
    /// When true, PRs are picked up even if the authenticated user is not
    /// in the `requested_reviewers` list. Useful for self-review workflows.
    #[serde(default)]
    pub skip_reviewer_check: bool,

    /// Re-review on every push (force-push / additional commit). Default: true.
    /// When false, a PR with a prior succeeded review is not re-queued on SHA
    /// change, but in-flight reviews on stale SHAs are still canceled.
    #[serde(default = "default_true")]
    pub review_on_push: bool,

    /// Override the global `runner.agent` for this repo.
    #[serde(default)]
    pub agent: Option<crate::types::AgentKind>,

    /// Override the global `runner.prompt_template` for this repo.
    #[serde(default)]
    pub prompt_template: Option<String>,

    /// Override the global `runner.model` for this repo.
    #[serde(default)]
    pub model: Option<String>,

    /// Override the global `execution.max_concurrency` for this repo.
    /// Reserved for future use; not yet wired into the runner.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Path to the local clone of this repository.
    /// Overrides the global `execution.base_repo_path` for worktree creation.
    #[serde(default)]
    pub base_repo_path: Option<PathBuf>,

    /// PR numbers to exclude from review.
    #[serde(default)]
    pub ignore_prs: Vec<u64>,
}

fn default_true() -> bool {
    true
}

/// Parsed per-repository policy with a resolved `RepoId`.
#[derive(Debug, Clone)]
pub struct RepoPolicy {
    pub id: crate::types::RepoId,
    pub skip_self_authored: bool,
    pub skip_reviewer_check: bool,
    pub review_on_push: bool,
    pub agent: Option<crate::types::AgentKind>,
    pub prompt_template: Option<String>,
    pub model: Option<String>,
    /// Reserved for future use; not yet wired into the runner.
    pub max_concurrency: Option<usize>,
    /// Path to the local clone of this repository.
    pub base_repo_path: Option<PathBuf>,
    /// PR numbers to exclude from review.
    pub ignore_prs: Vec<u64>,
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
    pub base_repo_path: Option<PathBuf>,
    pub worktree_root: Option<PathBuf>,

    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,

    #[serde(default = "default_lease_minutes")]
    pub lease_minutes: i64,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            base_repo_path: None,
            worktree_root: None,
            max_concurrency: default_max_concurrency(),
            lease_minutes: default_lease_minutes(),
        }
    }
}

fn default_max_concurrency() -> usize {
    10
}

fn default_lease_minutes() -> i64 {
    5
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerConfig {
    /// The agent to use for reviews (claude or codex). Default: claude.
    #[serde(default)]
    pub agent: Option<crate::types::AgentKind>,

    /// Prompt template for the AI review agent.
    /// Supports the same template variables as the command.
    /// The rendered prompt is available as `{prompt}` and `{prompt_file}` in the command.
    #[serde(default)]
    pub prompt_template: Option<String>,

    /// The model to pass to the agent via `--model` flag.
    #[serde(default)]
    pub model: Option<String>,
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
// Loading
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

        // Validate model names (global and per-repo).
        if let Some(ref m) = self.runner.model
            && !is_valid_model_name(m)
        {
            return Err(ReviewqError::Config(format!(
                "invalid runner.model '{}': must match [A-Za-z0-9._:-]+",
                m
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

        if self.polling.interval_seconds == 0 {
            return Err(ReviewqError::Config(
                "polling.interval_seconds must be > 0".into(),
            ));
        }

        Ok(())
    }

    /// Expand `~` in paths to the user's home directory.
    pub fn expand_paths(&mut self) {
        if let Some(home) = dirs::home_dir() {
            expand_tilde(&mut self.logging.dir, &home);
            expand_tilde(&mut self.state.sqlite_path, &home);
            expand_tilde(&mut self.output.dir, &home);
        }
    }

    /// Parse the allowlist into per-repository policies.
    pub fn repo_policies(&self) -> Vec<RepoPolicy> {
        self.repos
            .allowlist
            .iter()
            .filter_map(|entry| {
                let (owner, name) = entry.repo.split_once('/')?;
                Some(RepoPolicy {
                    id: crate::types::RepoId::new(owner, name),
                    skip_self_authored: entry.skip_self_authored,
                    skip_reviewer_check: entry.skip_reviewer_check,
                    review_on_push: entry.review_on_push,
                    agent: entry.agent.clone(),
                    prompt_template: entry.prompt_template.clone(),
                    model: entry.model.clone(),
                    max_concurrency: entry.max_concurrency,
                    base_repo_path: entry.base_repo_path.clone(),
                    ignore_prs: entry.ignore_prs.clone(),
                })
            })
            .collect()
    }

    /// Compare two configs and return human-readable change descriptions.
    ///
    /// Also flags fields that require a restart to take effect.
    pub fn diff_summary(old: &Config, new: &Config) -> Vec<String> {
        let mut changes = Vec::new();

        if old.repos != new.repos {
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
                    "repos.allowlist changed: {:?} -> {:?}",
                    old_repos, new_repos
                ));
            }
            // Report per-repo review_on_push changes specifically.
            for new_entry in &new.repos.allowlist {
                if let Some(old_entry) = old
                    .repos
                    .allowlist
                    .iter()
                    .find(|e| e.repo == new_entry.repo)
                    .filter(|old_entry| old_entry.review_on_push != new_entry.review_on_push)
                {
                    changes.push(format!(
                        "repos.allowlist[{}].review_on_push changed: {} -> {}",
                        new_entry.repo, old_entry.review_on_push, new_entry.review_on_push
                    ));
                }
            }
            // Fallback: if repo list is the same but other per-repo settings
            // changed (command, prompt_template, etc.), emit a generic line
            // so the change isn't silently swallowed.
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
                                || old_entry.max_concurrency != new_entry.max_concurrency
                                || old_entry.base_repo_path != new_entry.base_repo_path
                                || old_entry.ignore_prs != new_entry.ignore_prs
                        })
                });
                if has_other_changes {
                    changes.push("repos.allowlist per-repo settings changed".to_string());
                }
            }
        }

        if old.polling != new.polling {
            changes.push(format!(
                "polling.interval_seconds changed: {} -> {}",
                old.polling.interval_seconds, new.polling.interval_seconds
            ));
        }

        if old.auth != new.auth {
            changes.push("auth changed (restart required)".to_string());
        }

        if old.execution.max_concurrency != new.execution.max_concurrency {
            changes.push(format!(
                "execution.max_concurrency changed: {} -> {} (restart required)",
                old.execution.max_concurrency, new.execution.max_concurrency
            ));
        }

        if old.execution.base_repo_path != new.execution.base_repo_path {
            changes.push(format!(
                "execution.base_repo_path changed: {:?} -> {:?}",
                old.execution.base_repo_path, new.execution.base_repo_path
            ));
        }

        if old.execution.worktree_root != new.execution.worktree_root {
            changes.push(format!(
                "execution.worktree_root changed: {:?} -> {:?}",
                old.execution.worktree_root, new.execution.worktree_root
            ));
        }

        if old.runner.agent != new.runner.agent {
            changes.push(format!(
                "runner.agent changed: {:?} -> {:?} (restart required)",
                old.runner.agent, new.runner.agent
            ));
        }

        if old.runner.prompt_template != new.runner.prompt_template {
            changes.push(format!(
                "runner.prompt_template changed: {:?} -> {:?}",
                old.runner.prompt_template, new.runner.prompt_template
            ));
        }

        if old.runner.model != new.runner.model {
            changes.push(format!(
                "runner.model changed: {:?} -> {:?}",
                old.runner.model, new.runner.model
            ));
        }

        if old.cancel != new.cancel {
            changes.push("cancel changed (restart required)".to_string());
        }

        if old.cleanup != new.cleanup {
            changes.push(format!(
                "cleanup changed: ttl={}->{}min, interval={}->{}min",
                old.cleanup.ttl_minutes,
                new.cleanup.ttl_minutes,
                old.cleanup.interval_minutes,
                new.cleanup.interval_minutes
            ));
        }

        if old.logging != new.logging {
            changes.push("logging changed (restart required)".to_string());
        }

        if old.state != new.state {
            changes.push("state changed (restart required)".to_string());
        }

        if old.output != new.output {
            changes.push(format!(
                "output.dir changed: {:?} -> {:?}",
                old.output.dir, new.output.dir
            ));
        }

        changes
    }

    /// Extract just the repo IDs from the allowlist.
    pub fn repo_ids(&self) -> Vec<crate::types::RepoId> {
        self.repo_policies().into_iter().map(|p| p.id).collect()
    }

    /// Resolve the local clone path for a given repository.
    ///
    /// Priority: per-repo `base_repo_path` > global `execution.base_repo_path`.
    pub fn base_repo_for(&self, repo: &crate::types::RepoId) -> Option<PathBuf> {
        self.repo_policies()
            .iter()
            .find(|p| &p.id == repo)
            .and_then(|p| p.base_repo_path.clone())
            .or_else(|| self.execution.base_repo_path.clone())
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

    #[test]
    fn parse_minimal_config() {
        let yaml = r#"
repos:
  allowlist:
    - repo: owner/repo
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.repos.allowlist.len(), 1);
        assert_eq!(config.repos.allowlist[0].repo, "owner/repo");
        assert!(config.repos.allowlist[0].skip_self_authored);
        assert!(config.repos.allowlist[0].agent.is_none());
        assert!(config.repos.allowlist[0].max_concurrency.is_none());
        assert_eq!(config.polling.interval_seconds, 300);
        assert_eq!(config.execution.max_concurrency, 10);
    }

    #[test]
    fn parse_per_repo_overrides() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo1
      skip_self_authored: false
      agent: codex
      max_concurrency: 3
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.repos.allowlist.len(), 2);

        let e0 = &config.repos.allowlist[0];
        assert_eq!(e0.repo, "org/repo1");
        assert!(!e0.skip_self_authored);
        assert_eq!(e0.agent, Some(crate::types::AgentKind::Codex));
        assert_eq!(e0.max_concurrency, Some(3));

        let e1 = &config.repos.allowlist[1];
        assert_eq!(e1.repo, "org/repo2");
        assert!(e1.skip_self_authored);
        assert!(e1.agent.is_none());
        assert!(e1.max_concurrency.is_none());
    }

    #[test]
    fn repo_policies_returns_parsed_entries() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
      skip_self_authored: false
      agent: codex
      max_concurrency: 2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        let policies = config.repo_policies();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].id, crate::types::RepoId::new("org", "repo"));
        assert!(!policies[0].skip_self_authored);
        assert_eq!(policies[0].agent, Some(crate::types::AgentKind::Codex));
        assert_eq!(policies[0].max_concurrency, Some(2));
    }

    #[test]
    fn repo_ids_extracts_ids() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo1
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        let ids = config.repo_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], crate::types::RepoId::new("org", "repo1"));
        assert_eq!(ids[1], crate::types::RepoId::new("org", "repo2"));
    }

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
    fn parse_prompt_template_in_runner() {
        let yaml = r#"
repos:
  allowlist:
    - repo: owner/repo
runner:
  agent: claude
  prompt_template: "Review {pr_url}"
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(
            config.runner.prompt_template.as_deref(),
            Some("Review {pr_url}")
        );
        assert_eq!(config.runner.agent, Some(crate::types::AgentKind::Claude));
    }

    #[test]
    fn parse_per_repo_prompt_template() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo1
      prompt_template: "Custom prompt for repo1"
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        let policies = config.repo_policies();
        assert_eq!(
            policies[0].prompt_template.as_deref(),
            Some("Custom prompt for repo1")
        );
        assert!(policies[1].prompt_template.is_none());
    }

    #[test]
    fn full_config_roundtrip() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo1
    - repo: org/repo2
polling:
  interval_seconds: 60
auth:
  method: gh
  fallback_env: GITHUB_TOKEN
execution:
  max_concurrency: 5
cancel:
  sigint_timeout_seconds: 3
  sigterm_timeout_seconds: 10
  sigkill_timeout_seconds: 3
cleanup:
  ttl_minutes: 720
  interval_minutes: 15
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.polling.interval_seconds, 60);
        assert_eq!(config.execution.max_concurrency, 5);
        assert_eq!(config.cancel.sigint_timeout_seconds, 3);
        assert_eq!(config.cleanup.ttl_minutes, 720);
    }

    #[test]
    fn diff_summary_no_changes() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
polling:
  interval_seconds: 60
"#;
        let config = Config::from_yaml(yaml).expect("parse");
        let changes = Config::diff_summary(&config, &config);
        assert!(changes.is_empty());
    }

    #[test]
    fn diff_summary_detects_polling_change() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
polling:
  interval_seconds: 60
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
polling:
  interval_seconds: 120
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("polling.interval_seconds"));
        assert!(changes[0].contains("60"));
        assert!(changes[0].contains("120"));
    }

    #[test]
    fn diff_summary_detects_restart_required() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
execution:
  max_concurrency: 5
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
execution:
  max_concurrency: 20
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
    fn diff_summary_detects_repo_change() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo1
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo2
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(changes.iter().any(|c| c.contains("repos.allowlist")));
    }

    #[test]
    fn diff_summary_multiple_changes() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
polling:
  interval_seconds: 60
cleanup:
  ttl_minutes: 1440
  interval_minutes: 30
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
polling:
  interval_seconds: 120
cleanup:
  ttl_minutes: 720
  interval_minutes: 15
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().any(|c| c.contains("polling")));
        assert!(changes.iter().any(|c| c.contains("cleanup")));
    }

    #[test]
    fn parse_review_on_push_false() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
      review_on_push: false
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert!(!config.repos.allowlist[0].review_on_push);
        let policies = config.repo_policies();
        assert!(!policies[0].review_on_push);
    }

    #[test]
    fn review_on_push_defaults_to_true() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert!(config.repos.allowlist[0].review_on_push);
        let policies = config.repo_policies();
        assert!(policies[0].review_on_push);
    }

    #[test]
    fn diff_summary_detects_review_on_push_change() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      review_on_push: true
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      review_on_push: false
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("review_on_push"));
        assert!(changes[0].contains("true"));
        assert!(changes[0].contains("false"));
        // Should not have a generic "repos.allowlist changed" line
        // since the repo list itself didn't change.
        assert!(!changes[0].contains("repos.allowlist changed"));
    }

    #[test]
    fn diff_summary_review_on_push_and_agent_both_changed() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      review_on_push: true
      agent: claude
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      review_on_push: false
      agent: codex
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        // Should have 2 lines: specific review_on_push + generic per-repo settings
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().any(|c| c.contains("review_on_push")));
        assert!(
            changes
                .iter()
                .any(|c| c.contains("per-repo settings changed"))
        );
    }

    #[test]
    fn parse_model_in_runner() {
        let yaml = r#"
repos:
  allowlist:
    - repo: owner/repo
runner:
  model: claude-sonnet-4-5-20250514
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(
            config.runner.model.as_deref(),
            Some("claude-sonnet-4-5-20250514")
        );
    }

    #[test]
    fn parse_per_repo_model() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo1
      model: gpt-5.3-codex
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        let policies = config.repo_policies();
        assert_eq!(policies[0].model.as_deref(), Some("gpt-5.3-codex"));
        assert!(policies[1].model.is_none());
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
            let yaml =
                format!("repos:\n  allowlist:\n    - repo: org/repo\nrunner:\n  model: {name}\n");
            Config::from_yaml(&yaml)
                .unwrap_or_else(|e| panic!("model '{name}' should be valid: {e}"));
        }
    }

    #[test]
    fn invalid_model_names_rejected() {
        for name in ["model name", "model;rm", "$(echo hi)", "mod\"el", ""] {
            let yaml = format!(
                "repos:\n  allowlist:\n    - repo: org/repo\nrunner:\n  model: \"{name}\"\n"
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
  allowlist:
    - repo: org/repo
      model: "bad model"
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("invalid model"));
    }

    #[test]
    fn diff_summary_detects_runner_model_change() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
runner:
  model: gpt-5.4
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
runner:
  model: gpt-5.3-codex
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(
            changes.iter().any(|c| c.contains("runner.model")),
            "should detect runner.model change: {:?}",
            changes
        );
    }

    #[test]
    fn diff_summary_detects_per_repo_model_change() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      model: gpt-5.4
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
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
                .any(|c| c.contains("per-repo settings changed")),
            "should detect per-repo model change: {:?}",
            changes
        );
    }

    #[test]
    fn diff_summary_only_agent_changed() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      agent: claude
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      agent: codex
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        // Should have the fallback line for other per-repo settings
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("per-repo settings changed"));
    }

    #[test]
    fn parse_ignore_prs() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
      ignore_prs: [9520, 9521, 9522]
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.repos.allowlist[0].ignore_prs, vec![9520, 9521, 9522]);
        let policies = config.repo_policies();
        assert_eq!(policies[0].ignore_prs, vec![9520, 9521, 9522]);
    }

    #[test]
    fn ignore_prs_defaults_to_empty() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert!(config.repos.allowlist[0].ignore_prs.is_empty());
    }

    #[test]
    fn diff_summary_detects_ignore_prs_change() {
        let old = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
"#,
        )
        .expect("parse");
        let new = Config::from_yaml(
            r#"
repos:
  allowlist:
    - repo: org/repo
      ignore_prs: [100]
"#,
        )
        .expect("parse");
        let changes = Config::diff_summary(&old, &new);
        assert!(
            changes
                .iter()
                .any(|c| c.contains("per-repo settings changed")),
        );
    }
}
