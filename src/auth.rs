//! GitHub token resolution.
//!
//! Default: `gh auth token` (requires `gh` CLI to be authenticated).
//! Fallback: `GITHUB_TOKEN` environment variable.

use std::process::Command;

use crate::error::{Result, ReviewqError};

/// Resolve a GitHub personal access token.
///
/// 1. If `method` is `"gh"`, try `gh auth token`.
/// 2. Fall back to the environment variable named by `fallback_env`.
pub fn resolve_token(method: &str, fallback_env: &str) -> Result<String> {
    if method == "gh"
        && let Ok(token) = resolve_via_gh()
    {
        return Ok(token);
    }

    std::env::var(fallback_env).map_err(|_| {
        ReviewqError::Auth(format!(
            "could not resolve GitHub token: `gh auth token` failed and \
             environment variable {fallback_env} is not set"
        ))
    })
}

fn resolve_via_gh() -> std::result::Result<String, ()> {
    let output = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .map_err(|_| ())?;

    if !output.status.success() {
        return Err(());
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if token.is_empty() {
        return Err(());
    }

    Ok(token)
}
