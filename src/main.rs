use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    Status,

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

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Status) => {
            eprintln!("TODO: status command (Phase 1 Stream D)");
        }
        Some(Commands::Tail { job_id }) => {
            eprintln!("TODO: tail command for job {job_id} (Phase 1 Stream D)");
        }
        Some(Commands::Open { target }) => {
            eprintln!("TODO: open command for {target} (Phase 1 Stream D)");
        }
        Some(Commands::Tui) => {
            eprintln!("TODO: TUI mode (Phase 1 Stream C)");
        }
        None => {
            eprintln!("TODO: daemon mode (Phase 2 integration)");
        }
    }
}
