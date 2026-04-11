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
            let db = reviewq::db::Database::open(&config.daemon.state.sqlite_path)?;
            reviewq::tui::run(&db, &config.daemon.output.dir, &config.daemon.logging.dir)
        }
        None => run_daemon(config, config_path).await,
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

/// Periodically remove expired worktrees.
///
/// DB-driven: queries `JobStore::expired_terminal_worktrees` to get
/// `(repo, worktree_path)` pairs for every terminal job whose TTL has
/// elapsed, resolves each `repo` back to its
/// `RepoPolicy.base_repo_path` via the policy chain, and hands the
/// list to `worktree::cleanup_by_owner` so `git worktree remove` runs
/// against the correct base clone.
///
/// The set of **currently-known** worktree paths (all statuses, not
/// just expired terminal) is passed through as `protected` so the
/// orphan pass inside `cleanup_by_owner` cannot reap a directory
/// belonging to an in-flight job whose status has not yet transitioned
/// to terminal.
///
/// After each successful tracked removal, the loop clears the
/// corresponding `jobs.worktree_path` column so the next cycle does
/// not re-query the same row and reissue a removal against a
/// directory that is already gone.
async fn worktree_cleanup_loop(
    mut config_rx: watch::Receiver<Arc<reviewq::config::Config>>,
    store: Arc<reviewq::db::Database>,
) {
    loop {
        let config = config_rx.borrow_and_update().clone();
        let worktree_root = config.daemon.execution.effective_worktree_root();
        let interval = std::time::Duration::from_secs(config.daemon.cleanup.interval_minutes * 60);
        let ttl_minutes = config.daemon.cleanup.ttl_minutes;

        // Fallback base repo for the orphan pass only. Tracked
        // worktrees resolve their own base repo per-policy below —
        // rows for which that resolution fails are skipped, not
        // silently retargeted at this fallback.
        let orphan_base_repo = config
            .repos
            .defaults
            .base_repo_path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("current directory is accessible"));

        tokio::time::sleep(interval).await;

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
        let mut protected: HashSet<PathBuf> = match store.known_worktree_paths() {
            Ok(paths) => paths.into_iter().collect(),
            Err(e) => {
                warn!(error = %e, "failed to snapshot known worktree paths");
                continue;
            }
        };

        // Pull the expired terminal worktrees and pair each with its
        // owning base repo. If `base_repo_for` returns `None` (the
        // repo was dropped from the allowlist between job completion
        // and cleanup), skip the row and keep it in `protected` so
        // the orphan pass does not pick it up with a wrong base.
        let owned: Vec<(PathBuf, PathBuf)> = match store.expired_terminal_worktrees(ttl_minutes) {
            Ok(rows) => {
                let mut out = Vec::with_capacity(rows.len());
                for (repo, wt_path) in rows {
                    match config.base_repo_for(&repo) {
                        Some(base) => out.push((base, wt_path)),
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
                out
            }
            Err(e) => {
                warn!(error = %e, "failed to query expired worktrees");
                continue;
            }
        };

        match reviewq::worktree::cleanup_by_owner(
            &owned,
            &worktree_root,
            ttl_minutes,
            &orphan_base_repo,
            &protected,
        ) {
            Ok(removed) => {
                if !removed.is_empty() {
                    info!(count = removed.len(), "cleaned up expired worktrees");
                }
                // NULL out the `worktree_path` column for every path
                // we successfully removed (tracked and orphan), so
                // the next cycle's DB query does not return them.
                for path in &removed {
                    if let Err(e) = store.clear_worktree_path(path) {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to clear worktree_path after removal"
                        );
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "worktree cleanup failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn minimal_yaml() -> &'static str {
        "repos:\n  allowlist:\n    - repo: org/repo\ndaemon:\n  polling:\n    interval_seconds: 60\n"
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
            "repos:\n  allowlist:\n    - repo: org/repo\ndaemon:\n  polling:\n    interval_seconds: 120\n",
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
}
