//! Git worktree creation, cleanup, and TTL management.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use tracing::{info, warn};

use crate::error::{Result, ReviewqError};

/// Build a `git` command whose environment is stripped of the three
/// variables git uses to locate the current repository. When the
/// `reviewq` binary (or its test suite) runs inside a `git commit`
/// pre-commit hook, git exports `GIT_DIR`, `GIT_WORK_TREE`, and
/// `GIT_INDEX_FILE` into the child process — and those would override
/// any `current_dir()` we set on a freshly spawned `git` subcommand,
/// causing the call to operate on the outer repo rather than on the
/// repo we just pointed at. Scrub them so every git invocation in
/// this module resolves relative to its `current_dir`.
fn git() -> Command {
    let mut cmd = Command::new("git");
    cmd.env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    cmd
}

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
    let fetch_output = git()
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
    let _ = git()
        .args(["worktree", "prune"])
        .current_dir(base_repo)
        .output();
    if worktree_path.exists() {
        warn!(path = %worktree_path.display(), "worktree path already exists, removing stale entry");
        let _ = git()
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

    let output = git()
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
///
/// Uses `git worktree remove --force`. The force flag means uncommitted
/// changes inside the worktree are silently discarded — which is safe
/// for reviewq because every worktree is ephemeral scratch space for a
/// single agent-run job. Returns `Err` when the path is not a
/// registered worktree under `base_repo`, which callers use to
/// distinguish "remove succeeded" from "wrong owner".
pub fn remove(base_repo: &Path, worktree_path: &Path) -> Result<()> {
    let output = git()
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
    let _ = git()
        .args(["worktree", "prune"])
        .current_dir(base_repo)
        .output();

    info!(path = %worktree_path.display(), "removed worktree");
    Ok(())
}

/// Clean up expired worktrees, paired with their owning base repo.
///
/// Two-pass design:
///
/// 1. **Tracked pass.** For each `(base_repo, worktree_path)` in
///    `owned`, run `git worktree remove --force` against the correct
///    base repo. The caller (`worktree_cleanup_loop`) gets this list
///    from the DB via `JobStore::expired_terminal_worktrees`, so the
///    ownership pairing is always correct. Failures on one entry are
///    logged and the sweep continues.
///
/// 2. **Orphan pass.** Scans `worktree_root` for `reviewq-*`
///    directories that are NOT in `owned` and NOT in `protected` —
///    these are leftovers from jobs whose DB row was purged or never
///    written (crash recovery, manual `DELETE FROM jobs`, etc.).
///    Orphans older than `ttl_minutes` (by filesystem mtime) are
///    removed: first by asking git via `orphan_base_repo`, and if
///    that fails, by falling back to `std::fs::remove_dir_all` plus
///    a best-effort `git worktree prune`.
///
/// The `protected` set MUST include every worktree path the DB
/// currently knows about across all statuses (queued / leased /
/// running / terminal-but-not-expired). Without that, the orphan
/// pass would reap directories belonging to in-flight jobs whose
/// `worktree_path` has already been written — causing the running
/// agent process to operate in a deleted directory. Callers can
/// build this set from `JobStore::known_worktree_paths()`.
///
/// Returns every path that was successfully removed (tracked + orphan).
pub fn cleanup_by_owner(
    owned: &[(PathBuf, PathBuf)],
    worktree_root: &Path,
    ttl_minutes: u64,
    orphan_base_repo: &Path,
    protected: &std::collections::HashSet<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let mut tracked: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    // --- Tracked pass -------------------------------------------------
    for (base_repo, wt_path) in owned {
        tracked.insert(wt_path.clone());
        match remove(base_repo, wt_path) {
            Ok(()) => removed.push(wt_path.clone()),
            Err(e) => {
                warn!(
                    path = %wt_path.display(),
                    base_repo = %base_repo.display(),
                    error = %e,
                    "failed to remove tracked worktree (continuing sweep)"
                );
            }
        }
    }

    // --- Orphan pass --------------------------------------------------
    let ttl = std::time::Duration::from_secs(ttl_minutes * 60);
    let now = SystemTime::now();

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

        // Skip both the paths we just swept in the tracked pass and
        // anything the DB still considers live — that latter category
        // covers in-flight jobs whose status has not yet transitioned
        // to terminal, so they aren't in `owned` yet but must not be
        // reaped out from under the runner.
        if tracked.contains(&path) || protected.contains(&path) {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("reviewq-") => n.to_owned(),
            _ => continue,
        };

        if !path.is_dir() {
            continue;
        }

        let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        if age < ttl {
            continue;
        }

        info!(
            path = %path.display(),
            name,
            age_minutes = age.as_secs() / 60,
            "cleaning up orphan worktree"
        );

        // Try git first (might work if the orphan happens to be
        // registered under `orphan_base_repo`); otherwise fall back to
        // plain filesystem removal plus `git worktree prune`.
        match remove(orphan_base_repo, &path) {
            Ok(()) => removed.push(path),
            Err(git_err) => {
                warn!(
                    path = %path.display(),
                    error = %git_err,
                    "git worktree remove failed for orphan, falling back to fs::remove_dir_all"
                );
                if let Err(fs_err) = std::fs::remove_dir_all(&path) {
                    warn!(
                        path = %path.display(),
                        error = %fs_err,
                        "fs::remove_dir_all failed for orphan"
                    );
                    continue;
                }
                let _ = git()
                    .args(["worktree", "prune"])
                    .current_dir(orphan_base_repo)
                    .output();
                removed.push(path);
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    /// Initialize a minimal git repo with a single commit so
    /// `git worktree add --detach <sha>` has something to check out.
    fn init_repo(path: &Path) {
        std::fs::create_dir_all(path).expect("create repo dir");
        run_git(path, &["init", "-q"]);
        run_git(path, &["config", "user.email", "test@example.com"]);
        run_git(path, &["config", "user.name", "test"]);
        run_git(path, &["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("README.md"), "init\n").expect("write README");
        run_git(path, &["add", "README.md"]);
        run_git(path, &["commit", "-q", "-m", "init"]);
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = git()
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git spawn");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn head_sha(repo: &Path) -> String {
        let output = git()
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .expect("git rev-parse");
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_owned()
    }

    #[test]
    fn cleanup_by_owner_removes_tracked_worktrees() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt_path = create(&repo, &wt_root, 1, &sha).expect("create worktree");
        assert!(wt_path.exists());

        let owned = vec![(repo.clone(), wt_path.clone())];
        let removed =
            cleanup_by_owner(&owned, &wt_root, 60, &repo, &HashSet::new()).expect("cleanup");

        assert_eq!(removed, vec![wt_path.clone()]);
        assert!(!wt_path.exists(), "worktree should be gone");
    }

    #[test]
    fn cleanup_by_owner_pairs_each_worktree_with_its_own_repo() {
        // The core motivation: two repos with disjoint base paths, each
        // owning its own worktree under a shared worktree_root. The
        // pre-refactor cleanup would have guessed wrong on at least one.
        let tmp = TempDir::new().expect("tempdir");
        let repo_a = tmp.path().join("repo_a");
        let repo_b = tmp.path().join("repo_b");
        init_repo(&repo_a);
        init_repo(&repo_b);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha_a = head_sha(&repo_a);
        let sha_b = head_sha(&repo_b);
        let wt_a = create(&repo_a, &wt_root, 10, &sha_a).expect("create wt_a");
        let wt_b = create(&repo_b, &wt_root, 20, &sha_b).expect("create wt_b");
        assert!(wt_a.exists() && wt_b.exists());

        let owned = vec![
            (repo_a.clone(), wt_a.clone()),
            (repo_b.clone(), wt_b.clone()),
        ];
        let removed =
            cleanup_by_owner(&owned, &wt_root, 60, &repo_a, &HashSet::new()).expect("cleanup");

        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&wt_a));
        assert!(removed.contains(&wt_b));
        assert!(!wt_a.exists());
        assert!(!wt_b.exists());
    }

    #[test]
    fn cleanup_by_owner_continues_past_one_broken_entry() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt_good = create(&repo, &wt_root, 1, &sha).expect("create good wt");

        // Pretend there's a second tracked entry that never existed —
        // `git worktree remove` will fail on it.
        let fake = wt_root.join("reviewq-9999");
        let owned = vec![
            (repo.clone(), fake.clone()),
            (repo.clone(), wt_good.clone()),
        ];
        let removed =
            cleanup_by_owner(&owned, &wt_root, 60, &repo, &HashSet::new()).expect("cleanup");

        assert!(removed.contains(&wt_good));
        assert!(!wt_good.exists());
    }

    #[test]
    fn cleanup_by_owner_sweeps_orphan_reviewq_dirs_past_ttl() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        // A stray `reviewq-*` directory that was never registered with
        // git (nor tracked via DB).
        let orphan = wt_root.join("reviewq-orphan");
        std::fs::create_dir_all(&orphan).expect("create orphan");
        std::fs::write(orphan.join("stale.txt"), "old").expect("write stale");

        // ttl=0 treats everything as expired, so the orphan pass kicks
        // in without having to manipulate mtime.
        let removed = cleanup_by_owner(&[], &wt_root, 0, &repo, &HashSet::new()).expect("cleanup");

        assert!(removed.contains(&orphan), "orphan not removed: {removed:?}");
        assert!(!orphan.exists(), "orphan dir should be gone");
    }

    #[test]
    fn cleanup_by_owner_preserves_recent_orphans() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let recent = wt_root.join("reviewq-fresh");
        std::fs::create_dir_all(&recent).expect("create recent");

        // ttl = 60 minutes is plenty; the dir was just created.
        let removed = cleanup_by_owner(&[], &wt_root, 60, &repo, &HashSet::new()).expect("cleanup");

        assert!(
            !removed.contains(&recent),
            "recent orphan must be preserved: {removed:?}"
        );
        assert!(recent.exists(), "recent orphan dir should still exist");
    }

    #[test]
    fn cleanup_by_owner_ignores_non_reviewq_dirs() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let unrelated = wt_root.join("not-ours");
        std::fs::create_dir_all(&unrelated).expect("create unrelated");

        let removed = cleanup_by_owner(&[], &wt_root, 0, &repo, &HashSet::new()).expect("cleanup");

        assert!(!removed.contains(&unrelated));
        assert!(unrelated.exists(), "non-reviewq dir must be untouched");
    }

    #[test]
    fn cleanup_by_owner_returns_empty_when_root_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("never-created");

        let removed = cleanup_by_owner(&[], &wt_root, 0, &repo, &HashSet::new()).expect("cleanup");
        assert!(removed.is_empty());
    }

    #[test]
    fn cleanup_by_owner_skips_protected_paths_in_orphan_pass() {
        // Regression guard for the race where a runner has already
        // created a `reviewq-*` directory and stored its path in the
        // DB, but the job is still non-terminal (so it isn't in
        // `owned`). The caller passes the DB's `known_worktree_paths`
        // as `protected`, and the orphan pass must honor it even if
        // the directory looks expired by mtime (ttl=0).
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let in_flight = wt_root.join("reviewq-42");
        std::fs::create_dir_all(&in_flight).expect("create in-flight");

        let mut protected = HashSet::new();
        protected.insert(in_flight.clone());

        let removed = cleanup_by_owner(&[], &wt_root, 0, &repo, &protected).expect("cleanup");

        assert!(
            !removed.contains(&in_flight),
            "protected path must not be removed: {removed:?}"
        );
        assert!(in_flight.exists(), "protected dir must still exist");
    }
}
