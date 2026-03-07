//! Git worktree creation, cleanup, and TTL management.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use tracing::{info, warn};

use crate::error::{Result, ReviewqError};

/// Create a new git worktree for a job.
///
/// Fetches latest refs from origin first (so PR head SHAs are available),
/// then creates a detached HEAD worktree at `{worktree_root}/reviewq-{job_id}`
/// checked out to `head_sha`.
pub fn create(
    base_repo: &Path,
    worktree_root: &Path,
    job_id: i64,
    head_sha: &str,
) -> Result<PathBuf> {
    // Fetch latest refs so the PR's head SHA is available locally.
    let fetch_output = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(base_repo)
        .output()
        .map_err(|e| ReviewqError::Process(format!("failed to spawn git fetch: {e}")))?;

    if !fetch_output.status.success() {
        let stderr = String::from_utf8_lossy(&fetch_output.stderr);
        warn!(%stderr, "git fetch origin failed, proceeding anyway");
    }

    let worktree_path = worktree_root.join(format!("reviewq-{job_id}"));

    // Clean up stale worktree registration or leftover directory.
    // Two failure modes exist:
    //   1. Directory exists but git doesn't track it (e.g. DB reset reused job_id)
    //   2. Directory is gone but git metadata still references it (e.g. manual rm)
    // We handle both by always pruning, then force-removing if needed.
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(base_repo)
        .output();
    if worktree_path.exists() {
        warn!(path = %worktree_path.display(), "worktree path already exists, removing stale entry");
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&worktree_path)
            .current_dir(base_repo)
            .output();
        if worktree_path.exists() {
            std::fs::remove_dir_all(&worktree_path).map_err(|e| {
                ReviewqError::Process(format!(
                    "failed to remove stale worktree dir {}: {e}",
                    worktree_path.display()
                ))
            })?;
        }
    }

    let output = Command::new("git")
        .args(["worktree", "add", "--detach"])
        .arg(&worktree_path)
        .arg(head_sha)
        .current_dir(base_repo)
        .output()
        .map_err(|e| ReviewqError::Process(format!("failed to spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ReviewqError::Process(format!(
            "git worktree add failed: {stderr}"
        )));
    }

    info!(
        job_id,
        path = %worktree_path.display(),
        "created worktree"
    );
    Ok(worktree_path)
}

/// Remove a git worktree and prune stale entries.
pub fn remove(base_repo: &Path, worktree_path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .current_dir(base_repo)
        .output()
        .map_err(|e| ReviewqError::Process(format!("failed to spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ReviewqError::Process(format!(
            "git worktree remove failed: {stderr}"
        )));
    }

    // Prune any stale worktree metadata
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(base_repo)
        .output();

    info!(path = %worktree_path.display(), "removed worktree");
    Ok(())
}

/// Clean up worktrees older than `ttl_minutes`.
///
/// Scans `worktree_root` for directories matching the `reviewq-*` naming
/// pattern and removes any whose modification time exceeds the TTL.
/// Returns the list of paths that were successfully removed.
pub fn cleanup(base_repo: &Path, worktree_root: &Path, ttl_minutes: u64) -> Result<Vec<PathBuf>> {
    let ttl = std::time::Duration::from_secs(ttl_minutes * 60);
    let now = SystemTime::now();
    let mut removed = Vec::new();

    let entries = match std::fs::read_dir(worktree_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
        Err(e) => {
            return Err(ReviewqError::Process(format!(
                "failed to read worktree root {}: {e}",
                worktree_root.display()
            )));
        }
    };

    for entry in entries {
        let entry = entry
            .map_err(|e| ReviewqError::Process(format!("failed to read directory entry: {e}")))?;

        let path = entry.path();

        // Only process directories matching our naming convention
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("reviewq-") => n.to_owned(),
            _ => continue,
        };

        if !path.is_dir() {
            continue;
        }

        let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);

        let age = now.duration_since(modified).unwrap_or_default();

        if age > ttl {
            info!(
                path = %path.display(),
                name,
                age_minutes = age.as_secs() / 60,
                "cleaning up expired worktree"
            );
            match remove(base_repo, &path) {
                Ok(()) => removed.push(path),
                Err(e) => {
                    warn!(path = %entry.path().display(), error = %e, "failed to remove worktree")
                }
            }
        }
    }

    Ok(removed)
}
