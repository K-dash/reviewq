use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use reviewq::traits::JobStore;
use tokio::sync::watch;
use tracing::{error, info, warn};

/// reviewq — automatic PR review queue daemon.
#[derive(Debug, Parser)]
#[command(name = "reviewq", version, about)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Show the status of all jobs.
    Status {
        /// Filter by job status (queued, running, succeeded, failed, canceled).
        #[arg(short, long)]
        status: Option<String>,

        /// Filter by repository (owner/name).
        #[arg(short, long)]
        repo: Option<String>,
    },

    /// Tail the log of a running job.
    Tail {
        /// Job ID to tail.
        job_id: i64,
    },

    /// Open a PR URL or job result in the browser.
    Open {
        /// PR URL or job ID.
        target: String,
    },

    /// Launch the interactive TUI.
    Tui,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Resolve the configuration file path.
///
/// Priority: `--config` flag > `~/.reviewq/config.yml` default.
fn resolve_config_path(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    dirs::home_dir()
        .map(|h| h.join(".reviewq").join("config.yml"))
        .unwrap_or_else(|| PathBuf::from("reviewq.yml"))
}

async fn run(cli: Cli) -> reviewq::error::Result<()> {
    let config_path = resolve_config_path(cli.config);
    let mut config = reviewq::config::Config::load(&config_path)?;
    config.expand_paths();

    match cli.command {
        Some(Commands::Status { status, repo }) => {
            let db = reviewq::db::Database::open(&config.daemon.state.sqlite_path)?;
            reviewq::cli::status(&db, status.as_deref(), repo.as_deref())
        }
        Some(Commands::Tail { job_id }) => {
            let db = reviewq::db::Database::open(&config.daemon.state.sqlite_path)?;
            reviewq::cli::tail(&db, job_id, &config.daemon.output.dir)
        }
        Some(Commands::Open { target }) => {
            let db = reviewq::db::Database::open(&config.daemon.state.sqlite_path)?;
            reviewq::cli::open_target(&db, &target)
        }
        Some(Commands::Tui) => {
            // TUI needs a runnable worktree setup at its back so
            // `x` (cancel) / `r` (retry) keystrokes don't later
            // explode inside the runner. Read-only subcommands
            // (`status` / `tail` / `open`) skip this check so a
            // broken filesystem path doesn't block post-mortem.
            config.validate_paths()?;
            let db = reviewq::db::Database::open(&config.daemon.state.sqlite_path)?;
            reviewq::tui::run(&db, &config.daemon.output.dir, &config.daemon.logging.dir)
        }
        None => {
            // Same rationale as the TUI branch: fail fast before
            // the daemon even begins if `base_repo_path` is wrong.
            config.validate_paths()?;
            run_daemon(config, config_path).await
        }
    }
}

/// Run the daemon: detect PRs, execute reviews, clean up worktrees.
async fn run_daemon(
    config: reviewq::config::Config,
    config_path: PathBuf,
) -> reviewq::error::Result<()> {
    // Initialize logging (hold guard for program lifetime).
    let _log_guard = reviewq::logging::init(Some(&config.daemon.logging.dir));

    info!("starting reviewq daemon");

    // Single-instance enforcement via PID file.
    let _pid_file = reviewq::daemon::PidFile::acquire(&config.daemon.logging.dir)?;

    // Open database with configured lease duration.
    let db = Arc::new(
        reviewq::db::Database::open(&config.daemon.state.sqlite_path)?
            .with_lease_minutes(config.daemon.execution.lease_minutes),
    );

    // Resolve GitHub token and create API client.
    let token =
        reviewq::auth::resolve_token(&config.daemon.auth.method, &config.daemon.auth.fallback_env)?;
    let github = reviewq::github::GitHubApi::new(token);

    // Create the review executor with a built-in fallback command. Each job
    // carries its own resolved command (see detector), so this default only
    // applies to legacy or test paths that bypass the detector.
    let default_command = reviewq::types::AgentKind::default().default_command(None);
    let executor = Arc::new(reviewq::executor::CommandExecutor::new(
        default_command,
        config.daemon.cancel.clone(),
        config.daemon.output.dir.clone(),
    ));

    // Set up signal handlers for graceful shutdown.
    let (mut shutdown_rx, mut reload_rx, wake_notify) = reviewq::daemon::setup_signals().await?;

    // Config broadcast channel: tasks re-read at each loop iteration.
    let (config_tx, config_rx) = watch::channel(Arc::new(config));

    // Spawn the detector loop (PR polling).
    let detector_db = Arc::clone(&db);
    let detector_config_rx = config_rx.clone();
    let mut detector_handle = tokio::spawn(async move {
        reviewq::detector::run(&github, &*detector_db, detector_config_rx).await
    });

    // Spawn the runner loop (job execution).
    let runner_db = Arc::clone(&db);
    let runner_executor = Arc::clone(&executor);
    let runner_config_rx = config_rx.clone();
    let runner_shutdown_rx = shutdown_rx.clone();
    let mut runner_handle = tokio::spawn(async move {
        let clock = reviewq::traits::UtcClock;
        reviewq::runner::run(
            runner_db,
            runner_executor,
            &clock,
            runner_config_rx,
            runner_shutdown_rx,
            wake_notify,
        )
        .await
    });

    // Spawn the worktree cleanup loop.
    let cleanup_config_rx = config_rx.clone();
    let cleanup_db = Arc::clone(&db);
    let mut cleanup_handle =
        tokio::spawn(async move { worktree_cleanup_loop(cleanup_config_rx, cleanup_db).await });

    // Wait for shutdown signal, reload signal, or any task failure/exit.
    // Track which task already resolved to avoid double-await.
    let mut detector_done = false;
    let mut runner_done = false;
    let mut cleanup_done = false;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                info!("shutdown signal received, stopping daemon");
                break;
            }
            result = reload_rx.changed() => {
                match result {
                    Ok(()) => reload_config(&config_path, &config_tx),
                    Err(_) => {
                        warn!("reload signal channel closed, config reload disabled");
                        break;
                    }
                }
            }
            result = &mut detector_handle, if !detector_done => {
                detector_done = true;
                match result {
                    Ok(Err(e)) => error!(error = %e, "detector exited with error"),
                    Err(e) => error!(error = %e, "detector task panicked"),
                    Ok(Ok(())) => warn!("detector exited unexpectedly"),
                }
                break;
            }
            result = &mut runner_handle, if !runner_done => {
                runner_done = true;
                match result {
                    Ok(Err(e)) => error!(error = %e, "runner exited with error"),
                    Err(e) => error!(error = %e, "runner task panicked"),
                    Ok(Ok(())) => warn!("runner exited unexpectedly"),
                }
                break;
            }
            result = &mut cleanup_handle, if !cleanup_done => {
                cleanup_done = true;
                match result {
                    Err(e) => error!(error = %e, "cleanup task panicked"),
                    Ok(()) => warn!("cleanup loop exited unexpectedly"),
                }
                break;
            }
        }
    }

    // Graceful shutdown:
    // - runner observes shutdown_rx and drains in-flight jobs before exiting
    // - detector and cleanup don't observe shutdown, so abort them
    info!("shutting down background tasks");

    if !detector_done {
        detector_handle.abort();
        let _ = detector_handle.await;
    }
    if !cleanup_done {
        cleanup_handle.abort();
        let _ = cleanup_handle.await;
    }
    if !runner_done {
        // Wait for runner to gracefully drain in-flight jobs.
        match runner_handle.await {
            Ok(Err(e)) => error!(error = %e, "runner exited with error during shutdown"),
            Err(e) => error!(error = %e, "runner task failed during shutdown"),
            Ok(Ok(())) => info!("runner shut down gracefully"),
        }
    }

    info!("reviewq daemon stopped");
    Ok(())
}

/// Re-read config from disk and broadcast to all tasks.
fn reload_config(
    config_path: &std::path::Path,
    config_tx: &watch::Sender<Arc<reviewq::config::Config>>,
) {
    info!("config reload triggered");

    let mut new_config = match reviewq::config::Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "config reload failed: keeping previous config");
            return;
        }
    };
    new_config.expand_paths();

    // Filesystem check for every resolved `base_repo_path`. Missing
    // or non-directory paths are treated like any other reload error:
    // we log and keep the old config so the daemon does not die
    // because the user temporarily moved a clone aside. `warn!`
    // (not `error!`) because this is an explicitly *recoverable*
    // condition — the operator may already be fixing it and should
    // not be paged.
    if let Err(e) = new_config.validate_paths() {
        warn!(error = %e, "config reload: validate_paths failed, keeping previous config");
        return;
    }

    let old_config = config_tx.borrow().clone();

    let changes = reviewq::config::Config::diff_summary(&old_config, &new_config);
    if changes.is_empty() {
        info!("config reload: no changes detected");
        return;
    }

    for change in &changes {
        if change.contains("restart required") {
            warn!(change = %change, "config change requires restart to take effect");
        } else {
            info!(change = %change, "config changed");
        }
    }

    if config_tx.send(Arc::new(new_config)).is_err() {
        warn!("config broadcast failed: no active receivers");
        return;
    }
    info!("config reloaded successfully");
}

/// Execute one full worktree cleanup pass.
///
/// DB-driven: queries `JobStore::expired_terminal_worktrees` to get
/// `(repo, worktree_path)` pairs for every terminal job whose TTL has
/// elapsed, resolves each `repo` back to its
/// `RepoPolicy.base_repo_path` via the policy chain, and hands the
/// list to `worktree::cleanup_by_owner` so `git worktree remove` runs
/// against the correct base clone.
///
/// The set of **currently-known** worktree paths (all statuses, not
/// just expired terminal) is snapshotted into `protected` so the
/// orphan pass inside `cleanup_by_owner` cannot reap a directory
/// belonging to an in-flight job whose status has not yet transitioned
/// to terminal.
///
/// Rows whose repo is no longer in the allowlist (the user edited
/// config between job completion and cleanup) are **skipped** with a
/// `warn!` rather than silently retargeted at a fallback base. Their
/// paths are folded into `protected` so the orphan pass will not pick
/// them up with the wrong base either.
///
/// After each successful tracked or orphan removal, the corresponding
/// `jobs.worktree_path` column is cleared so the next cycle's query
/// does not re-issue a removal against a directory that is already
/// gone.
///
/// This is a synchronous function pulled out of the async loop so it
/// can be unit-tested without standing up a tokio runtime. The async
/// loop calls it via `tokio::task::spawn_blocking` because the
/// underlying work — `git worktree remove` subprocesses and
/// filesystem walks — would otherwise stall a runtime worker.
fn cleanup_once(
    store: &reviewq::db::Database,
    config: &reviewq::config::Config,
) -> reviewq::error::Result<Vec<PathBuf>> {
    let worktree_root = config.daemon.execution.effective_worktree_root();
    let ttl_minutes = config.daemon.cleanup.ttl_minutes;

    // Every distinct `base_repo_path` resolved by the policy chain.
    // `cleanup_by_owner`'s orphan pass runs `git worktree prune`
    // against each one after reaping stale directories, so every
    // allowlisted install keeps its `.git/worktrees/` bookkeeping
    // clean — not just the repo that happened to be picked
    // arbitrarily (the pre-refactor behavior). Duplicates are
    // deduplicated here; the policy chain guarantees every entry
    // has `Some(path)` thanks to `Config::validate`.
    let orphan_base_repos: Vec<PathBuf> = {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut out: Vec<PathBuf> = Vec::new();
        for policy in config.repo_policies() {
            if let Some(path) = policy.base_repo_path
                && seen.insert(path.clone())
            {
                out.push(path);
            }
        }
        out
    };

    // Snapshot every worktree path the DB currently knows about.
    // The orphan pass inside `cleanup_by_owner` must exclude all
    // of these so it cannot reap an in-flight job's worktree.
    //
    // This snapshot is taken *before* the `expired_terminal_worktrees`
    // query below, and the two calls run under separate mutex
    // locks. A job that starts between them will be missing from
    // both sets — the mtime guard inside `cleanup_by_owner` is the
    // backstop (a just-created directory cannot satisfy a
    // sensibly-configured TTL). See `JobStore::known_worktree_paths`
    // for the full atomicity argument.
    let mut protected: HashSet<PathBuf> = store.known_worktree_paths()?.into_iter().collect();

    // Pull the expired terminal worktrees and pair each with its
    // owning base repo. If `base_repo_for` returns `None` (the
    // repo was dropped from the allowlist between job completion
    // and cleanup), skip the row and keep it in `protected` so
    // the orphan pass does not pick it up with a wrong base.
    let mut owned: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (repo, wt_path) in store.expired_terminal_worktrees(ttl_minutes)? {
        match config.base_repo_for(&repo) {
            Some(base) => owned.push((base, wt_path)),
            None => {
                warn!(
                    repo = %repo,
                    path = %wt_path.display(),
                    "base_repo_path not configured for repo; skipping worktree cleanup"
                );
                protected.insert(wt_path);
            }
        }
    }

    let removed = reviewq::worktree::cleanup_by_owner(
        &owned,
        &worktree_root,
        ttl_minutes,
        &orphan_base_repos,
        &protected,
    )?;

    // NULL out the `worktree_path` column for every path we
    // successfully removed (tracked and orphan), so the next
    // cycle's DB query does not return them.
    for path in &removed {
        if let Err(e) = store.clear_worktree_path(path) {
            warn!(
                path = %path.display(),
                error = %e,
                "failed to clear worktree_path after removal"
            );
        }
    }

    Ok(removed)
}

/// Periodically remove expired worktrees.
///
/// Calls `cleanup_once` once per iteration, then sleeps for
/// `daemon.cleanup.interval_minutes`. The sleep happens at the **end**
/// of the loop body so the first sweep runs immediately on daemon
/// startup — without that, daemon installs that are restarted before
/// the first interval elapses would never sweep at all (a real bug
/// for users who run reviewq intermittently).
///
/// `cleanup_once` is invoked through `tokio::task::spawn_blocking`
/// because it spawns `git worktree remove` subprocesses and walks
/// the filesystem; running that work directly on the async loop
/// would stall a runtime worker.
async fn worktree_cleanup_loop(
    mut config_rx: watch::Receiver<Arc<reviewq::config::Config>>,
    store: Arc<reviewq::db::Database>,
) {
    loop {
        let config = config_rx.borrow_and_update().clone();
        let interval = std::time::Duration::from_secs(config.daemon.cleanup.interval_minutes * 60);

        let store_for_blocking = Arc::clone(&store);
        let config_for_blocking = Arc::clone(&config);
        let join = tokio::task::spawn_blocking(move || {
            cleanup_once(&store_for_blocking, &config_for_blocking)
        })
        .await;

        // None of these arms break the loop — every error path
        // logs and falls through to the sleep, so transient DB
        // hiccups or one-off subprocess failures cannot wedge
        // cleanup until the next daemon restart.
        match join {
            Ok(Ok(removed)) if !removed.is_empty() => {
                info!(count = removed.len(), "cleaned up expired worktrees");
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => warn!(error = %e, "worktree cleanup failed"),
            Err(e) => warn!(error = %e, "cleanup task panicked or was cancelled"),
        }

        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn minimal_yaml() -> &'static str {
        // `/tmp` is the simplest always-existent, always-a-directory
        // path that is acceptable to `validate_paths`. Tests that
        // exercise `reload_config` run through that check too, so a
        // bogus `/tmp/fake` would be rejected and the "old config
        // retained" assertions would pass spuriously.
        "repos:\n  defaults:\n    base_repo_path: /tmp\n  allowlist:\n    - repo: org/repo\ndaemon:\n  polling:\n    interval_seconds: 60\n"
    }

    fn write_config(dir: &std::path::Path, yaml: &str) -> PathBuf {
        let path = dir.join("config.yml");
        let mut f = std::fs::File::create(&path).expect("create config");
        f.write_all(yaml.as_bytes()).expect("write config");
        path
    }

    #[test]
    fn reload_config_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = write_config(dir.path(), minimal_yaml());

        let mut initial = reviewq::config::Config::load(&config_path).expect("load");
        initial.expand_paths();
        let (config_tx, config_rx) = watch::channel(Arc::new(initial));

        // Rewrite with changed polling interval
        write_config(
            dir.path(),
            "repos:\n  defaults:\n    base_repo_path: /tmp\n  allowlist:\n    - repo: org/repo\ndaemon:\n  polling:\n    interval_seconds: 120\n",
        );

        reload_config(&config_path, &config_tx);

        let updated = config_rx.borrow().clone();
        assert_eq!(updated.daemon.polling.interval_seconds, 120);
    }

    #[test]
    fn reload_config_invalid_yaml_keeps_old() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = write_config(dir.path(), minimal_yaml());

        let mut initial = reviewq::config::Config::load(&config_path).expect("load");
        initial.expand_paths();
        let (config_tx, config_rx) = watch::channel(Arc::new(initial));

        // Overwrite with invalid YAML
        write_config(dir.path(), "this is not valid yaml: [[[");

        reload_config(&config_path, &config_tx);

        // Old config should be retained
        let current = config_rx.borrow().clone();
        assert_eq!(current.daemon.polling.interval_seconds, 60);
    }

    #[test]
    fn reload_config_validation_failure_keeps_old() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = write_config(dir.path(), minimal_yaml());

        let mut initial = reviewq::config::Config::load(&config_path).expect("load");
        initial.expand_paths();
        let (config_tx, config_rx) = watch::channel(Arc::new(initial));

        // Overwrite with empty allowlist (validation error)
        write_config(dir.path(), "repos:\n  allowlist: []\n");

        reload_config(&config_path, &config_tx);

        // Old config should be retained
        let current = config_rx.borrow().clone();
        assert_eq!(current.repos.allowlist.len(), 1);
    }

    #[test]
    fn reload_config_path_missing_keeps_old_config() {
        // Regression guard for the graceful-degradation rule added
        // alongside the `validate_paths` mandatory-startup check: when
        // a hot reload produces a structurally-valid config whose
        // `base_repo_path` has been deleted on disk, the daemon keeps
        // running on the old config rather than dying.
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = write_config(dir.path(), minimal_yaml());

        let mut initial = reviewq::config::Config::load(&config_path).expect("load");
        initial.expand_paths();
        let (config_tx, config_rx) = watch::channel(Arc::new(initial));

        // Overwrite with a config whose base_repo_path points at a
        // path that definitely does not exist. validate() accepts it
        // (structurally valid), validate_paths() must reject it, and
        // reload_config must keep the old config.
        let bad_yaml = "repos:\n  defaults:\n    base_repo_path: /tmp/reviewq-reload-test-nowhere-aaa\n  allowlist:\n    - repo: org/repo\ndaemon:\n  polling:\n    interval_seconds: 120\n";
        write_config(dir.path(), bad_yaml);

        reload_config(&config_path, &config_tx);

        let current = config_rx.borrow().clone();
        assert_eq!(
            current.daemon.polling.interval_seconds, 60,
            "old config must be retained after validate_paths failure"
        );
    }

    #[test]
    fn reload_config_no_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = write_config(dir.path(), minimal_yaml());

        let mut initial = reviewq::config::Config::load(&config_path).expect("load");
        initial.expand_paths();
        let (config_tx, _config_rx) = watch::channel(Arc::new(initial));

        // Reload the same config — should detect no changes and not send
        reload_config(&config_path, &config_tx);

        // No assertion on value since it stays the same; this test verifies
        // no panic and the "no changes detected" path executes.
    }

    // ----------------------------------------------------------------
    // cleanup_once / worktree_cleanup_loop test helpers
    // ----------------------------------------------------------------
    //
    // These mirror the helpers in `src/worktree.rs::tests`. Duplication
    // is intentional: cargo `cfg(test)` is per-crate, so the binary's
    // test build cannot import test-only items from the lib.

    fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let mut cmd = std::process::Command::new("git");
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        let output = cmd.args(args).current_dir(cwd).output().expect("git spawn");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo(path: &std::path::Path) {
        std::fs::create_dir_all(path).expect("create repo dir");
        run_git(path, &["init", "-q"]);
        run_git(path, &["config", "user.email", "test@example.com"]);
        run_git(path, &["config", "user.name", "test"]);
        run_git(path, &["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("README.md"), "init\n").expect("write README");
        run_git(path, &["add", "README.md"]);
        run_git(path, &["commit", "-q", "-m", "init"]);
    }

    fn head_sha(repo: &std::path::Path) -> String {
        let mut cmd = std::process::Command::new("git");
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        let output = cmd
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .expect("git rev-parse");
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_owned()
    }

    /// Insert a job, attach its worktree path, and mark it succeeded.
    /// Returns the new job id.
    fn enqueue_terminal_job_with_worktree(
        db: &reviewq::db::Database,
        owner: &str,
        name: &str,
        pr_number: u64,
        head_sha: &str,
        worktree_path: &std::path::Path,
    ) -> i64 {
        use reviewq::types::{AgentKind, JobStatus, NewJob, RepoId};
        let new_job = NewJob {
            repo: RepoId::new(owner, name),
            pr_number,
            head_sha: head_sha.to_owned(),
            agent_kind: AgentKind::Claude,
            command: Some("echo".into()),
            prompt_template: None,
            max_retries: 3,
        };
        let job = db.enqueue(new_job).expect("enqueue");
        db.store_worktree_path(job.id, worktree_path)
            .expect("store worktree path");
        db.complete(job.id, JobStatus::Succeeded, Some(0))
            .expect("complete");
        job.id
    }

    /// Build a single-repo Config with the given worktree_root and
    /// per-repo base_repo_path. The allowlist contains exactly
    /// `<owner>/<name>`. Cleanup TTL is set in minutes; the loop
    /// interval defaults to 60 minutes for tests that need a long
    /// gap to prove "first sweep happens at startup".
    fn build_test_config(
        owner: &str,
        name: &str,
        base_repo: &std::path::Path,
        worktree_root: &std::path::Path,
        ttl_minutes: u64,
        interval_minutes: u64,
    ) -> reviewq::config::Config {
        let yaml = format!(
            "daemon:\n  \
               execution:\n    \
                 worktree_root: \"{wt_root}\"\n  \
               cleanup:\n    \
                 ttl_minutes: {ttl_minutes}\n    \
                 interval_minutes: {interval_minutes}\n\
             repos:\n  \
               defaults:\n    \
                 base_repo_path: \"{base_repo}\"\n  \
               allowlist:\n    \
                 - repo: {owner}/{name}\n",
            wt_root = worktree_root.display(),
            base_repo = base_repo.display(),
        );
        reviewq::config::Config::from_yaml(&yaml).expect("parse test config")
    }

    // ----------------------------------------------------------------
    // cleanup_once tests
    // ----------------------------------------------------------------

    #[test]
    fn cleanup_once_removes_expired_tracked_worktree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt_path = reviewq::worktree::create(&repo, &wt_root, 1, &sha).expect("create wt");
        assert!(wt_path.exists());

        let db = reviewq::db::Database::open_in_memory().expect("db");
        enqueue_terminal_job_with_worktree(&db, "owner", "repo", 1, &sha, &wt_path);

        let config = build_test_config("owner", "repo", &repo, &wt_root, 0, 60);

        let removed = cleanup_once(&db, &config).expect("cleanup_once");

        assert!(removed.contains(&wt_path), "tracked wt must be removed");
        assert!(!wt_path.exists(), "tracked wt dir must be gone");

        let known = db.known_worktree_paths().expect("known");
        assert!(
            !known.contains(&wt_path),
            "DB worktree_path must be cleared after removal"
        );
    }

    #[test]
    fn cleanup_once_preserves_in_flight_worktree_via_protected() {
        use reviewq::types::{AgentKind, NewJob, RepoId};

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt_path = reviewq::worktree::create(&repo, &wt_root, 1, &sha).expect("create wt");

        let db = reviewq::db::Database::open_in_memory().expect("db");
        // Enqueue, lease, mark running, store worktree path. Crucially,
        // do NOT call complete() — the job is still in-flight.
        let new_job = NewJob {
            repo: RepoId::new("owner", "repo"),
            pr_number: 1,
            head_sha: sha.clone(),
            agent_kind: AgentKind::Claude,
            command: Some("echo".into()),
            prompt_template: None,
            max_retries: 3,
        };
        let job = db.enqueue(new_job).expect("enqueue");
        let leased = db.lease_next().expect("lease").expect("has job");
        db.mark_running(leased.id, 1234).expect("mark running");
        db.store_worktree_path(job.id, &wt_path)
            .expect("store path");

        // ttl=0 would normally make everything eligible, but the
        // running job's path is in `protected` so the orphan pass
        // must skip it.
        let config = build_test_config("owner", "repo", &repo, &wt_root, 0, 60);

        let removed = cleanup_once(&db, &config).expect("cleanup_once");

        assert!(
            !removed.contains(&wt_path),
            "in-flight wt must not be removed: {removed:?}"
        );
        assert!(wt_path.exists(), "in-flight wt dir must still exist");
    }

    #[test]
    fn cleanup_once_skips_repo_not_in_allowlist() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt_path = reviewq::worktree::create(&repo, &wt_root, 1, &sha).expect("create wt");

        let db = reviewq::db::Database::open_in_memory().expect("db");
        // Job is for "owner/repo"...
        enqueue_terminal_job_with_worktree(&db, "owner", "repo", 1, &sha, &wt_path);

        // ...but the config allowlist only knows about a different repo.
        // base_repo_for("owner/repo") therefore returns None, and the
        // row should be skipped (warn) rather than retargeted.
        let config = build_test_config("other", "other", &repo, &wt_root, 0, 60);

        let removed = cleanup_once(&db, &config).expect("cleanup_once");

        assert!(
            !removed.contains(&wt_path),
            "skipped repo wt must not be removed: {removed:?}"
        );
        assert!(wt_path.exists(), "wt dir must still exist");

        let known = db.known_worktree_paths().expect("known");
        assert!(
            known.contains(&wt_path),
            "DB row must NOT be cleared when the row was skipped"
        );
    }

    #[test]
    fn cleanup_once_sweeps_orphan_dir_past_ttl() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        // Stray reviewq-* directory not registered in the DB.
        let orphan = wt_root.join("reviewq-orphan");
        std::fs::create_dir_all(&orphan).expect("create orphan");
        std::fs::write(orphan.join("stale.txt"), "old").expect("write stale");

        let db = reviewq::db::Database::open_in_memory().expect("db");
        let config = build_test_config("owner", "repo", &repo, &wt_root, 0, 60);

        let removed = cleanup_once(&db, &config).expect("cleanup_once");

        assert!(
            removed.contains(&orphan),
            "orphan must be swept by orphan pass: {removed:?}"
        );
        assert!(!orphan.exists());
    }

    #[test]
    fn cleanup_once_clears_db_rows_after_removal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt1 = reviewq::worktree::create(&repo, &wt_root, 1, &sha).expect("wt1");
        let wt2 = reviewq::worktree::create(&repo, &wt_root, 2, &sha).expect("wt2");

        let db = reviewq::db::Database::open_in_memory().expect("db");
        enqueue_terminal_job_with_worktree(&db, "owner", "repo", 1, &sha, &wt1);
        enqueue_terminal_job_with_worktree(&db, "owner", "repo", 2, &sha, &wt2);

        let config = build_test_config("owner", "repo", &repo, &wt_root, 0, 60);

        let removed = cleanup_once(&db, &config).expect("cleanup_once");
        assert_eq!(removed.len(), 2);

        let known = db.known_worktree_paths().expect("known");
        assert!(
            known.is_empty(),
            "all worktree_path columns must be NULL after removal: {known:?}"
        );
    }

    #[test]
    fn cleanup_once_continues_past_partial_failure() {
        // Two tracked worktrees: one real, one whose path never
        // existed on disk. `git worktree remove` will fail on the
        // ghost entry, but the sweep must still remove the real one
        // and clear only the real one's DB row.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt_good = reviewq::worktree::create(&repo, &wt_root, 1, &sha).expect("wt good");
        let wt_ghost = wt_root.join("reviewq-9999"); // never created via git

        let db = reviewq::db::Database::open_in_memory().expect("db");
        enqueue_terminal_job_with_worktree(&db, "owner", "repo", 1, &sha, &wt_good);
        enqueue_terminal_job_with_worktree(&db, "owner", "repo", 2, &sha, &wt_ghost);

        let config = build_test_config("owner", "repo", &repo, &wt_root, 0, 60);

        let removed = cleanup_once(&db, &config).expect("cleanup_once");

        assert!(
            removed.contains(&wt_good),
            "good wt must still be removed despite ghost failure"
        );
        assert!(!wt_good.exists());

        let known = db.known_worktree_paths().expect("known");
        assert!(
            !known.contains(&wt_good),
            "good DB row must be cleared after successful removal"
        );
        assert!(
            known.contains(&wt_ghost),
            "ghost DB row must NOT be cleared when removal failed"
        );
    }

    // ----------------------------------------------------------------
    // worktree_cleanup_loop integration test
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn worktree_cleanup_loop_runs_immediately_on_startup() {
        // Regression guard for the "sleep at top of loop" bug: with
        // sleep at the top, intermittently-run daemons would never
        // sweep because the user shut down before the first interval
        // elapsed. Setting interval to 60 minutes here means a
        // top-of-loop sleep would block forever; the test deadline
        // (2 s) catches that case.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        init_repo(&repo);
        let wt_root = tmp.path().join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("create wt_root");

        let sha = head_sha(&repo);
        let wt_path = reviewq::worktree::create(&repo, &wt_root, 1, &sha).expect("create wt");

        let db = Arc::new(reviewq::db::Database::open_in_memory().expect("db"));
        enqueue_terminal_job_with_worktree(&db, "owner", "repo", 1, &sha, &wt_path);

        let config = build_test_config("owner", "repo", &repo, &wt_root, 0, 60);
        let (_tx, config_rx) = watch::channel(Arc::new(config));

        let db_clone = Arc::clone(&db);
        let handle = tokio::spawn(async move { worktree_cleanup_loop(config_rx, db_clone).await });

        // cleanup_once runs on a spawn_blocking thread, so tokio's
        // virtual time would not affect it. Poll the filesystem
        // directly. Generous deadline; the first sweep should
        // complete in well under a second.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while wt_path.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        handle.abort();
        let _ = handle.await;

        assert!(
            !wt_path.exists(),
            "first iteration must run immediately on startup"
        );
    }
}
