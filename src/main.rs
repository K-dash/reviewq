use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
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
            let db = reviewq::db::Database::open(&config.state.sqlite_path)?;
            reviewq::cli::status(&db, status.as_deref(), repo.as_deref())
        }
        Some(Commands::Tail { job_id }) => {
            let db = reviewq::db::Database::open(&config.state.sqlite_path)?;
            reviewq::cli::tail(&db, job_id)
        }
        Some(Commands::Open { target }) => {
            let db = reviewq::db::Database::open(&config.state.sqlite_path)?;
            reviewq::cli::open_target(&db, &target)
        }
        Some(Commands::Tui) => {
            let db = reviewq::db::Database::open(&config.state.sqlite_path)?;
            reviewq::tui::run(&db, &config.output.dir, &config.logging.dir)
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
    let _log_guard = reviewq::logging::init(Some(&config.logging.dir));

    info!("starting reviewq daemon");

    // Single-instance enforcement via PID file.
    let _pid_file = reviewq::daemon::PidFile::acquire(&config.logging.dir)?;

    // Open database with configured lease duration.
    let db = Arc::new(
        reviewq::db::Database::open(&config.state.sqlite_path)?
            .with_lease_minutes(config.execution.lease_minutes),
    );

    // Resolve GitHub token and create API client.
    let token = reviewq::auth::resolve_token(&config.auth.method, &config.auth.fallback_env)?;
    let github = reviewq::github::GitHubApi::new(token);

    // Create the review executor.
    let default_agent = config.runner.agent.clone().unwrap_or_default();
    let default_command = default_agent.default_command(config.runner.model.as_deref());
    let executor = Arc::new(reviewq::executor::CommandExecutor::new(
        default_command,
        config.cancel.clone(),
        config.output.dir.clone(),
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
    let mut cleanup_handle =
        tokio::spawn(async move { worktree_cleanup_loop(cleanup_config_rx).await });

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
async fn worktree_cleanup_loop(mut config_rx: watch::Receiver<Arc<reviewq::config::Config>>) {
    loop {
        let config = config_rx.borrow_and_update().clone();
        let base_repo =
            config.execution.base_repo_path.clone().unwrap_or_else(|| {
                std::env::current_dir().expect("current directory is accessible")
            });
        let worktree_root = config
            .execution
            .worktree_root
            .clone()
            .unwrap_or_else(|| base_repo.join(".worktrees"));
        let interval = std::time::Duration::from_secs(config.cleanup.interval_minutes * 60);

        tokio::time::sleep(interval).await;
        match reviewq::worktree::cleanup(&base_repo, &worktree_root, config.cleanup.ttl_minutes) {
            Ok(removed) if !removed.is_empty() => {
                info!(count = removed.len(), "cleaned up expired worktrees");
            }
            Err(e) => {
                warn!(error = %e, "worktree cleanup failed");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn minimal_yaml() -> &'static str {
        "repos:\n  allowlist:\n    - repo: org/repo\npolling:\n  interval_seconds: 60\n"
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
            "repos:\n  allowlist:\n    - repo: org/repo\npolling:\n  interval_seconds: 120\n",
        );

        reload_config(&config_path, &config_tx);

        let updated = config_rx.borrow().clone();
        assert_eq!(updated.polling.interval_seconds, 120);
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
        assert_eq!(current.polling.interval_seconds, 60);
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
