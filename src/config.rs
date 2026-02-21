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
    pub allowlist: Vec<String>,
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
    PathBuf::from("./output")
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

        for repo in &self.repos.allowlist {
            if !repo.contains('/') {
                return Err(ReviewqError::Config(format!(
                    "invalid repo format '{repo}': expected 'owner/name'"
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
        }
    }

    /// Parse a repo string from the allowlist into a `RepoId`.
    pub fn parse_allowlist(&self) -> Vec<crate::types::RepoId> {
        self.repos
            .allowlist
            .iter()
            .filter_map(|s| {
                let (owner, name) = s.split_once('/')?;
                Some(crate::types::RepoId::new(owner, name))
            })
            .collect()
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
    - owner/repo
"#;
        let config = Config::from_yaml(yaml).expect("should parse");
        assert_eq!(config.repos.allowlist, vec!["owner/repo"]);
        assert_eq!(config.polling.interval_seconds, 300);
        assert_eq!(config.execution.max_concurrency, 10);
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
    - just-a-name
"#;
        let err = Config::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("owner/name"));
    }

    #[test]
    fn full_config_roundtrip() {
        let yaml = r#"
repos:
  allowlist:
    - org/repo1
    - org/repo2
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
