//! reviewq — automatic PR review queue daemon.
//!
//! Detects PRs where the user is a requested reviewer, triggers AI code
//! review agents, and provides a TUI for monitoring progress.

pub mod auth;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod db;
pub mod detector;
pub mod error;
pub mod executor;
pub mod github;
pub mod idempotency;
pub mod logging;
pub mod rules;
pub mod runner;
pub mod traits;
pub mod tui;
pub mod types;
pub mod update;
pub mod worktree;
