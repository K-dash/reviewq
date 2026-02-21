use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing::{error, info, warn};

/// reviewq — automatic PR review queue daemon.
#[derive(Debug, Parser)]
#[command(name = "reviewq", version, about)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, default_value = "reviewq.yml")]
    config: PathBuf,

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

async fn run(cli: Cli) -> reviewq::error::Result<()> {
    let mut config = reviewq::config::Config::load(&cli.config)?;
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
            reviewq::tui::run(&db)
        }
        None => run_daemon(config).await,
    }
}

/// Run the daemon: detect PRs, execute reviews, clean up worktrees.
async fn run_daemon(config: reviewq::config::Config) -> reviewq::error::Result<()> {
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
    let default_command = config
        .runner
        .command
        .clone()
        .unwrap_or_else(|| "echo 'no review command configured'".into());
    let executor = Arc::new(reviewq::executor::CommandExecutor::new(
        default_command,
        config.cancel.clone(),
        config.output.dir.clone(),
    ));

    // Set up signal handlers for graceful shutdown.
    let (mut shutdown_rx, _reload_rx) = reviewq::daemon::setup_signals().await?;

    // Spawn the detector loop (PR polling).
    let detector_db = Arc::clone(&db);
    let detector_config = config.clone();
    let mut detector_handle = tokio::spawn(async move {
        reviewq::detector::run(&github, &*detector_db, &detector_config).await
    });

    // Spawn the runner loop (job execution).
    let runner_db = Arc::clone(&db);
    let runner_executor = Arc::clone(&executor);
    let runner_config = config.clone();
    let runner_shutdown_rx = shutdown_rx.clone();
    let mut runner_handle = tokio::spawn(async move {
        let clock = reviewq::traits::UtcClock;
        reviewq::runner::run(
            runner_db,
            runner_executor,
            &clock,
            &runner_config,
            runner_shutdown_rx,
        )
        .await
    });

    // Spawn the worktree cleanup loop.
    let cleanup_config = config.clone();
    let mut cleanup_handle =
        tokio::spawn(async move { worktree_cleanup_loop(&cleanup_config).await });

    // Wait for shutdown signal or any task failure/exit.
    // Track which task already resolved to avoid double-await.
    let mut detector_done = false;
    let mut runner_done = false;
    let mut cleanup_done = false;

    tokio::select! {
        _ = shutdown_rx.changed() => {
            info!("shutdown signal received, stopping daemon");
        }
        result = &mut detector_handle => {
            detector_done = true;
            match result {
                Ok(Err(e)) => error!(error = %e, "detector exited with error"),
                Err(e) => error!(error = %e, "detector task panicked"),
                Ok(Ok(())) => warn!("detector exited unexpectedly"),
            }
        }
        result = &mut runner_handle => {
            runner_done = true;
            match result {
                Ok(Err(e)) => error!(error = %e, "runner exited with error"),
                Err(e) => error!(error = %e, "runner task panicked"),
                Ok(Ok(())) => warn!("runner exited unexpectedly"),
            }
        }
        result = &mut cleanup_handle => {
            cleanup_done = true;
            match result {
                Err(e) => error!(error = %e, "cleanup task panicked"),
                Ok(()) => warn!("cleanup loop exited unexpectedly"),
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

/// Periodically remove expired worktrees.
async fn worktree_cleanup_loop(config: &reviewq::config::Config) {
    let base_repo = config
        .execution
        .base_repo_path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory is accessible"));
    let worktree_root = config
        .execution
        .worktree_root
        .clone()
        .unwrap_or_else(|| base_repo.join(".worktrees"));
    let interval = std::time::Duration::from_secs(config.cleanup.interval_minutes * 60);

    loop {
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
