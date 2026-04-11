//! Git worktree creation, cleanup, and TTL management.

use std::collections::HashSet;
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
///    removed with `std::fs::remove_dir_all`. After the orphan sweep
///    finishes, `git worktree prune` is run **once per distinct base
///    repo** in `orphan_base_repos` so every allowlisted repo's admin
///    dir gets its stale `.git/worktrees/<name>` entries cleaned up,
///    not just whichever repo happened to be passed in.
///
/// The `protected` set MUST include every worktree path the DB
/// currently knows about across all statuses (queued / leased /
/// running / terminal-but-not-expired). Without that, the orphan
/// pass would reap directories belonging to in-flight jobs whose
/// `worktree_path` has already been written — causing the running
/// agent process to operate in a deleted directory. Callers can
/// build this set from `JobStore::known_worktree_paths()`.
///
/// `orphan_base_repos` is the complete set of base repo paths that
/// could have owned an orphan. The caller (`worktree_cleanup_loop`)
/// builds it by iterating `Config::repo_policies()` and collecting
/// every resolved `base_repo_path`. Duplicates are allowed — the
/// orphan pass deduplicates internally before running `git worktree
/// prune`. An empty slice is legal but means no `prune` is attempted,
/// so stale admin entries may linger until the next full sweep with a
/// non-empty slice.
///
/// Returns every path that was successfully removed (tracked + orphan).
pub fn cleanup_by_owner(
    owned: &[(PathBuf, PathBuf)],
    worktree_root: &Path,
    ttl_minutes: u64,
    orphan_base_repos: &[PathBuf],
    protected: &HashSet<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let mut tracked: HashSet<PathBuf> = HashSet::new();

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

    let mut swept_any_orphan = false;
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

        // Orphans have no reliable owner pointer, so skip the
        // speculative `git worktree remove` (which would only succeed
        // for the lucky case where the orphan's admin entry happened
        // to live under a single known repo) and go straight to
        // filesystem removal. The `git worktree prune` sweep below
        // picks up the stale admin entries for every allowlisted base
        // repo, which is what actually keeps `.git/worktrees/` clean
        // in multi-repo installs.
        if let Err(fs_err) = std::fs::remove_dir_all(&path) {
            warn!(
                path = %path.display(),
                error = %fs_err,
                "fs::remove_dir_all failed for orphan"
            );
            continue;
        }
        removed.push(path);
        swept_any_orphan = true;
    }

    // Run `git worktree prune` once per distinct base repo so every
    // allowlisted install gets its `.git/worktrees/` bookkeeping
    // cleaned up, regardless of which repo owned the orphan. Skipped
    // entirely when no orphans were reaped — `prune` is idempotent,
    // but it is still a fork-per-repo and worth avoiding in the
    // common steady-state cleanup cycle.
    if swept_any_orphan {
        let mut seen: HashSet<&Path> = HashSet::new();
        for base in orphan_base_repos {
            if !seen.insert(base.as_path()) {
                continue;
            }
            // `Command::output()` returns `Ok` for any non-spawn
            // failure — including `git` itself exiting non-zero
            // (locked admin dir, read-only filesystem, etc.). Check
            // the status explicitly so those cases do not silently
            // leave stale bookkeeping behind.
            match git().args(["worktree", "prune"]).current_dir(base).output() {
                Ok(out) if !out.status.success() => {
                    warn!(
                        base_repo = %base.display(),
                        stderr = %String::from_utf8_lossy(&out.stderr),
                        "`git worktree prune` exited non-zero for orphan bookkeeping; continuing"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        base_repo = %base.display(),
                        error = %e,
                        "failed to spawn `git worktree prune` for orphan bookkeeping; continuing"
                    );
                }
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
        let removed = cleanup_by_owner(
            &owned,
            &wt_root,
            60,
            std::slice::from_ref(&repo),
            &HashSet::new(),
        )
        .expect("cleanup");

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
        let removed = cleanup_by_owner(
            &owned,
            &wt_root,
            60,
            std::slice::from_ref(&repo_a),
            &HashSet::new(),
        )
        .expect("cleanup");

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
        let removed = cleanup_by_owner(
            &owned,
            &wt_root,
            60,
            std::slice::from_ref(&repo),
            &HashSet::new(),
        )
        .expect("cleanup");

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
        let removed = cleanup_by_owner(
            &[],
            &wt_root,
            0,
            std::slice::from_ref(&repo),
            &HashSet::new(),
        )
        .expect("cleanup");

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
        let removed = cleanup_by_owner(
            &[],
            &wt_root,
            60,
            std::slice::from_ref(&repo),
            &HashSet::new(),
        )
        .expect("cleanup");

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

        let removed = cleanup_by_owner(
            &[],
            &wt_root,
            0,
            std::slice::from_ref(&repo),
            &HashSet::new(),
        )
        .expect("cleanup");

        assert!(!removed.contains(&unrelated));
        assert!(unrelated.exists(), "non-reviewq dir must be untouched");
    }

    #[test]
    fn cleanup_by_owner_returns_empty_when_root_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("never-created");

        let removed = cleanup_by_owner(
            &[],
            &wt_root,
            0,
            std::slice::from_ref(&repo),
            &HashSet::new(),
        )
        .expect("cleanup");
        assert!(removed.is_empty());
    }

    #[test]
    fn cleanup_by_owner_orphan_pass_prunes_every_allowlisted_repo() {
        // The pre-refactor version only ran `git worktree prune` on a
        // single "orphan_base_repo" passed by the caller, so in a
        // multi-repo install an orphan owned by a *different*
        // allowlisted repo would get its directory removed but leave a
        // stale `.git/worktrees/<name>` admin entry behind. The new
        // signature takes the full slice of distinct base repos and
        // prunes each of them; this test pins that behavior.
        let tmp = TempDir::new().expect("tempdir");
        let repo_a = tmp.path().join("repo_a");
        let repo_b = tmp.path().join("repo_b");
        init_repo(&repo_a);
        init_repo(&repo_b);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        // Create a real worktree under repo_b and its `.git/worktrees`
        // admin entry, then remove the actual working dir *without*
        // telling git about it. That simulates an orphan whose only
        // "owner" metadata lives inside repo_b's admin dir.
        let sha_b = head_sha(&repo_b);
        let wt_b = create(&repo_b, &wt_root, 42, &sha_b).expect("create wt_b");
        assert!(wt_b.exists());

        // Yank just the directory, leaving the `.git/worktrees/reviewq-42`
        // entry behind. `git worktree prune` on repo_b must find and
        // remove it. `git worktree prune` on repo_a must NOT touch it
        // (wrong repo) — meaning the sweep only works if the caller
        // passes repo_b in the slice, not just repo_a.
        std::fs::remove_dir_all(&wt_b).expect("remove wt_b dir only");

        // Re-create the directory so the orphan pass has something to
        // reap. (It was the worktree we just removed, so recreating it
        // keeps git's admin entry pointing at it but reviewq sees it
        // as a stray dir.)
        std::fs::create_dir_all(&wt_b).expect("recreate wt_b as orphan");
        std::fs::write(wt_b.join("stale.txt"), "old").expect("write stale");

        let orphan_admin = repo_b.join(".git/worktrees/reviewq-42");
        assert!(
            orphan_admin.exists(),
            "precondition: git admin entry must still exist before sweep"
        );

        // Correct call: pass both base repos. Orphan gets reaped AND
        // repo_b's admin entry gets pruned.
        let removed = cleanup_by_owner(
            &[],
            &wt_root,
            0,
            &[repo_a.clone(), repo_b.clone()],
            &HashSet::new(),
        )
        .expect("cleanup");

        assert!(removed.contains(&wt_b), "orphan dir must be removed");
        assert!(!wt_b.exists(), "orphan dir should be gone after sweep");
        assert!(
            !orphan_admin.exists(),
            "repo_b's stale admin entry must be pruned: {:?}",
            orphan_admin
        );
    }

    #[test]
    fn cleanup_by_owner_orphan_pass_leaves_bookkeeping_stale_when_wrong_repo_passed() {
        // Companion to the test above: documents the old failure mode
        // and guards against a regression where someone "optimizes" the
        // caller to only pass a single repo. Orphan owned by repo_b,
        // caller only passes repo_a — the *directory* is still removed
        // (fs::remove_dir_all always works), but repo_b's admin entry
        // remains stale because we never ran `git worktree prune`
        // against repo_b.
        let tmp = TempDir::new().expect("tempdir");
        let repo_a = tmp.path().join("repo_a");
        let repo_b = tmp.path().join("repo_b");
        init_repo(&repo_a);
        init_repo(&repo_b);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha_b = head_sha(&repo_b);
        let wt_b = create(&repo_b, &wt_root, 99, &sha_b).expect("create wt_b");
        std::fs::remove_dir_all(&wt_b).expect("remove wt_b dir only");
        std::fs::create_dir_all(&wt_b).expect("recreate as orphan");

        let orphan_admin = repo_b.join(".git/worktrees/reviewq-99");
        assert!(orphan_admin.exists(), "precondition");

        // Wrong call: only pass repo_a. Directory still reaped, but
        // repo_b's bookkeeping stays stale.
        let _ = cleanup_by_owner(
            &[],
            &wt_root,
            0,
            std::slice::from_ref(&repo_a),
            &HashSet::new(),
        )
        .expect("cleanup");

        assert!(!wt_b.exists(), "orphan dir is still reaped");
        assert!(
            orphan_admin.exists(),
            "repo_b's admin entry stays stale when caller forgets to pass it"
        );
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

        let removed = cleanup_by_owner(&[], &wt_root, 0, std::slice::from_ref(&repo), &protected)
            .expect("cleanup");

        assert!(
            !removed.contains(&in_flight),
            "protected path must not be removed: {removed:?}"
        );
        assert!(in_flight.exists(), "protected dir must still exist");
    }
}
