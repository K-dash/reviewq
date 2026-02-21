//! Prompt view: shows the command used for a job.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Key binding hint for the prompt view.
const KEY_HINTS: &str = " Esc Back  q Quit ";

/// Render the prompt/command view for a job.
pub fn render(f: &mut Frame, command: &str, area: Rect) {
    let paragraph = Paragraph::new(command)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Command / Prompt "),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);

    // Render key hints
    if area.height > 2 {
        let hint_area = Rect {
            x: area.x,
            y: area.y + area.height - 1,
            width: area.width,
            height: 1,
        };
        let hint =
            Line::from(KEY_HINTS).style(Style::default().fg(ratatui::style::Color::DarkGray));
        f.render_widget(hint, hint_area);
    }
}
