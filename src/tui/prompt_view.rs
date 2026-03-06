//! Prompt view: shows the command and rendered prompt for a job.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::widgets::{self, TITLE_STYLE};

/// Render the prompt/command view for a job.
pub fn render(f: &mut Frame, command: &str, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Min(1),    // content
        Constraint::Length(1), // help bar
    ])
    .split(area);

    // Title
    f.render_widget(Line::styled("Command / Prompt", TITLE_STYLE), chunks[0]);

    // Content
    let paragraph = Paragraph::new(command).wrap(Wrap { trim: false });
    f.render_widget(paragraph, chunks[1]);

    // Help bar
    widgets::render_help_bar(f, chunks[2], &[("Esc", "back"), ("q", "quit")]);
}
