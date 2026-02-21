//! Shared styled widgets for the TUI.

use chrono::{DateTime, Utc};
use ratatui::style::{Color, Modifier, Style};

use crate::types::JobStatus;

/// Get the display color for a job status.
pub fn status_color(status: JobStatus) -> Color {
    match status {
        JobStatus::Queued => Color::Yellow,
        JobStatus::Leased => Color::Blue,
        JobStatus::Running => Color::Cyan,
        JobStatus::Succeeded => Color::Green,
        JobStatus::Failed => Color::Red,
        JobStatus::Canceled => Color::Gray,
    }
}

/// Format a job status as a styled string.
pub fn status_badge(status: JobStatus) -> (String, Style) {
    let label = match status {
        JobStatus::Queued => "QUEUED",
        JobStatus::Leased => "LEASED",
        JobStatus::Running => "RUNNING",
        JobStatus::Succeeded => "OK",
        JobStatus::Failed => "FAILED",
        JobStatus::Canceled => "CANCEL",
    };
    let style = Style::default()
        .fg(status_color(status))
        .add_modifier(Modifier::BOLD);
    (label.to_owned(), style)
}

/// Format a timestamp for display (compact relative form or absolute).
pub fn format_timestamp(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}

/// Truncate a SHA to 7 characters for display.
pub fn short_sha(sha: &str) -> &str {
    if sha.len() > 7 { &sha[..7] } else { sha }
}
