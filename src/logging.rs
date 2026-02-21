//! Logging / tracing setup.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Initialize the tracing subscriber.
///
/// - Console output is always enabled.
/// - If `log_dir` is provided, a file appender is also set up.
///
/// Returns a [`WorkerGuard`] that must be held for the lifetime of the
/// program to ensure buffered logs are flushed on shutdown.
pub fn init(log_dir: Option<&Path>) -> Option<WorkerGuard> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("reviewq=info,warn"));

    if let Some(dir) = log_dir {
        let file_appender = tracing_appender::rolling::daily(dir, "reviewq.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();

        Some(guard)
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();

        None
    }
}
