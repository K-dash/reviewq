//! Unified error types for reviewq.

use std::io;

/// Classifies errors by origin for retry-aware handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Authentication failure (token invalid/expired).
    Auth,
    /// Network / HTTP transport error.
    Network,
    /// GitHub API rate limit exceeded.
    RateLimit,
    /// SQLite / database error.
    Db,
    /// Child process management error.
    Process,
    /// Configuration error.
    Config,
}

impl ErrorKind {
    /// Whether this error kind is typically retryable.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Network | Self::RateLimit)
    }
}

/// The unified error type for reviewq.
#[derive(Debug, thiserror::Error)]
pub enum ReviewqError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("GitHub API error: {message}")]
    GitHub { message: String, kind: ErrorKind },

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("authentication error: {0}")]
    Auth(String),

    #[error("runner error: {0}")]
    Runner(String),

    #[error("rate limit exceeded, retry after {retry_after_secs}s")]
    RateLimit { retry_after_secs: u64 },

    #[error("process error: {0}")]
    Process(String),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yml::Error),
}

impl ReviewqError {
    /// Returns the error kind for retry classification.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Config(_) | Self::Yaml(_) => ErrorKind::Config,
            Self::Database(_) => ErrorKind::Db,
            Self::GitHub { kind, .. } => *kind,
            Self::Http(_) => ErrorKind::Network,
            Self::Io(_) => ErrorKind::Process,
            Self::Auth(_) => ErrorKind::Auth,
            Self::Runner(_) => ErrorKind::Process,
            Self::RateLimit { .. } => ErrorKind::RateLimit,
            Self::Process(_) => ErrorKind::Process,
        }
    }

    /// Whether this error is typically retryable.
    pub fn is_retryable(&self) -> bool {
        self.kind().is_retryable()
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, ReviewqError>;
