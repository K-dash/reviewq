//! Review view: displays the review markdown output.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::widgets::{self, TITLE_STYLE};

/// Render the review output view.
pub fn render(f: &mut Frame, review_text: &str, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Min(1),    // content
        Constraint::Length(1), // help bar
    ])
    .split(area);

    // Title
    f.render_widget(Line::styled("Review Output", TITLE_STYLE), chunks[0]);

    // Content
    let paragraph = Paragraph::new(review_text).wrap(Wrap { trim: false });
    f.render_widget(paragraph, chunks[1]);

    // Help bar
    widgets::render_help_bar(
        f,
        chunks[2],
        &[("o", "browser"), ("Esc", "back"), ("q", "quit")],
    );
}
