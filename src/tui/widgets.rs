//! Shared styled widgets and theme constants for the TUI.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::types::{Job, JobStatus};

// ── Theme constants ──────────────────────────────────────────────

/// Teal bold — used for view titles.
pub const TITLE_STYLE: Style = Style::new()
    .fg(Color::Indexed(37))
    .add_modifier(Modifier::BOLD);

/// Dark gray — used for status lines, scroll indicators, metadata.
pub const STATUS_STYLE: Style = Style::new().fg(Color::DarkGray);

/// Dark cyan background + bold — used for the selected row in queue view.
pub const SELECTED_STYLE: Style = Style::new()
    .bg(Color::Indexed(23))
    .add_modifier(Modifier::BOLD);

/// Gray — used for help bar key labels.
pub const HELP_KEY_STYLE: Style = Style::new().fg(Color::Indexed(246));

/// Dimmer gray — used for help bar descriptions.
pub const HELP_DESC_STYLE: Style = Style::new().fg(Color::Indexed(240));

/// Gray — used for help bar `▕` separators.
pub const SEPARATOR_STYLE: Style = Style::new().fg(Color::Indexed(242));

/// Green — used for flash/success messages.
pub const FLASH_STYLE: Style = Style::new().fg(Color::Green);

// ── Job status colors ────────────────────────────────────────────

/// Get the display color for a job status.
pub fn status_color(status: JobStatus) -> Color {
    match status {
        JobStatus::Queued => Color::Yellow,
        JobStatus::Leased => Color::Blue,
        JobStatus::Running => Color::LightCyan,
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

/// Format a job's status badge, showing "CANCELING" when a cancel has been
/// requested but the job has not yet reached a terminal state.
pub fn status_badge_for_job(job: &Job) -> (String, Style) {
    if job.is_cancel_requested() && !job.status.is_terminal() {
        let style = Style::default()
            .fg(Color::Indexed(208)) // orange
            .add_modifier(Modifier::BOLD);
        ("CANCELING".to_owned(), style)
    } else {
        status_badge(job.status)
    }
}

// ── Formatting helpers ───────────────────────────────────────────

/// Format a timestamp for display (compact relative form or absolute).
pub fn format_timestamp(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}

/// Truncate a SHA to 7 characters for display.
pub fn short_sha(sha: &str) -> &str {
    if sha.len() > 7 { &sha[..7] } else { sha }
}

// ── Help bar ─────────────────────────────────────────────────────

/// Render a two-tone help bar into a single-line area.
///
/// Each item is a `(key, description)` pair.  Keys are rendered in
/// `HELP_KEY_STYLE` and descriptions in `HELP_DESC_STYLE`, separated
/// by thin `▕` dividers in `SEPARATOR_STYLE`.
pub fn render_help_bar(f: &mut Frame, area: Rect, items: &[(&str, &str)]) {
    let mut spans: Vec<Span<'_>> = Vec::new();
    for (i, (key, desc)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ▕ ", SEPARATOR_STYLE));
        }
        spans.push(Span::styled(*key, HELP_KEY_STYLE));
        spans.push(Span::styled(" ", Style::default()));
        spans.push(Span::styled(*desc, HELP_DESC_STYLE));
    }
    f.render_widget(Line::from(spans), area);
}
