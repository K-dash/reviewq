//! Tail view: displays stdout/stderr of a job.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::widgets::{self, SEPARATOR_STYLE, TITLE_STYLE};

/// Render the tail view for a job's log output.
pub fn render(f: &mut Frame, log_content: &str, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Length(1), // separator
        Constraint::Min(1),    // content
        Constraint::Length(1), // help bar
    ])
    .split(area);

    // Title
    f.render_widget(Line::styled("Log Output", TITLE_STYLE), chunks[0]);

    // Separator
    let sep = "─".repeat(area.width as usize);
    f.render_widget(Line::styled(sep, SEPARATOR_STYLE), chunks[1]);

    // Content with auto-scroll
    let line_count = log_content.lines().count() as u16;
    let inner_height = chunks[2].height;
    let scroll = line_count.saturating_sub(inner_height);

    let paragraph = Paragraph::new(log_content)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(paragraph, chunks[2]);

    // Help bar
    widgets::render_help_bar(f, chunks[3], &[("Esc", "back"), ("q", "quit")]);
}
