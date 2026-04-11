//! Review view: displays the review markdown output.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::app::App;
use super::widgets::{self, TITLE_STYLE};

/// Render the review output view.
pub fn render(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Min(1),    // content
        Constraint::Length(1), // help bar
    ])
    .split(area);

    // Title
    f.render_widget(Line::styled("Review Output", TITLE_STYLE), chunks[0]);

    // Publish viewport height so scroll actions can compute page size
    // and the event loop can clamp scroll offset against content length.
    app.content_viewport_height = chunks[1].height;
    app.clamp_content_scroll();

    // Content
    let paragraph = Paragraph::new(app.review_text.as_str())
        .wrap(Wrap { trim: false })
        .scroll((app.content_scroll, 0));
    f.render_widget(paragraph, chunks[1]);

    // Help bar — note the list is the only exit: q/Esc both return to queue.
    widgets::render_help_bar(
        f,
        chunks[2],
        &[
            ("↑/↓", "scroll"),
            ("PgUp/PgDn", "page"),
            ("o", "browser"),
            ("Esc/q", "back to list"),
        ],
    );
}
