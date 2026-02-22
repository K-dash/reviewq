//! YAML configuration parsing, validation, and defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Result, ReviewqError};

/// Top-level configuration for reviewq.
#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Default, Deserialize)]
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
///       command: "claude code review"
///       max_concurrency: 3
///       base_repo_path: "/path/to/local/clone"
/// ```
#[derive(Debug, Clone, Deserialize)]
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

    /// Override the global `runner.command` for this repo.
    #[serde(default)]
    pub command: Option<String>,

    /// Override the global `runner.prompt_template` for this repo.
    #[serde(default)]
    pub prompt_template: Option<String>,

    /// Override the global `execution.max_concurrency` for this repo.
    /// Reserved for future use; not yet wired into the runner.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Path to the local clone of this repository.
    /// Overrides the global `execution.base_repo_path` for worktree creation.
    #[serde(default)]
    pub base_repo_path: Option<PathBuf>,
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
    pub command: Option<String>,
    pub prompt_template: Option<String>,
    /// Reserved for future use; not yet wired into the runner.
    pub max_concurrency: Option<usize>,
    /// Path to the local clone of this repository.
    pub base_repo_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerConfig {
    #[serde(default)]
    pub command: Option<String>,

    /// Prompt template for the AI review agent.
    /// Supports the same template variables as `command`.
    /// The rendered prompt is available as `{prompt}` and `{prompt_file}` in the command.
    #[serde(default)]
    pub prompt_template: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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
                    command: entry.command.clone(),
                    prompt_template: entry.prompt_template.clone(),
                    max_concurrency: entry.max_concurrency,
                    base_repo_path: entry.base_repo_path.clone(),
                })
            })
            .collect()
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
        assert!(config.repos.allowlist[0].command.is_none());
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
      command: "claude review"
      max_concurrency: 3
    - repo: org/repo2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.repos.allowlist.len(), 2);

        let e0 = &config.repos.allowlist[0];
        assert_eq!(e0.repo, "org/repo1");
        assert!(!e0.skip_self_authored);
        assert_eq!(e0.command.as_deref(), Some("claude review"));
        assert_eq!(e0.max_concurrency, Some(3));

        let e1 = &config.repos.allowlist[1];
        assert_eq!(e1.repo, "org/repo2");
        assert!(e1.skip_self_authored);
        assert!(e1.command.is_none());
        assert!(e1.max_concurrency.is_none());
    }

    #[test]
    fn repo_policies_returns_parsed_entries() {
        let yaml = r#"
repos:
  allowlist:
    - repo: org/repo
      skip_self_authored: false
      command: "echo review"
      max_concurrency: 2
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        let policies = config.repo_policies();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].id, crate::types::RepoId::new("org", "repo"));
        assert!(!policies[0].skip_self_authored);
        assert_eq!(policies[0].command.as_deref(), Some("echo review"));
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
  command: "claude -p '{prompt}'"
  prompt_template: "Review {pr_url}"
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(
            config.runner.prompt_template.as_deref(),
            Some("Review {pr_url}")
        );
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
}
