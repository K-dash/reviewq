//! Prompt view: shows the command and rendered prompt for a job.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::app::App;
use super::widgets::{self, TITLE_STYLE};

/// Render the prompt/command view for a job.
pub fn render(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Min(1),    // content
        Constraint::Length(1), // help bar
    ])
    .split(area);

    // Title
    f.render_widget(Line::styled("Command / Prompt", TITLE_STYLE), chunks[0]);

    // Publish viewport height so scroll actions can compute page size
    // and clamp scroll offset against content length.
    app.content_viewport_height = chunks[1].height;
    app.clamp_content_scroll();

    // Content
    let paragraph = Paragraph::new(app.command_text.as_str())
        .wrap(Wrap { trim: false })
        .scroll((app.content_scroll, 0));
    f.render_widget(paragraph, chunks[1]);

    // Help bar — the list is the only exit: q/Esc both return to queue.
    widgets::render_help_bar(
        f,
        chunks[2],
        &[
            ("↑/↓", "scroll"),
            ("PgUp/PgDn", "page"),
            ("Esc/q", "back to list"),
        ],
    );
}
