//! Tail view: displays stdout/stderr of a job.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Key binding hint for the tail view.
const KEY_HINTS: &str = " Esc Back  q Quit ";

/// Render the tail view for a job's log output.
pub fn render(f: &mut Frame, log_content: &str, area: Rect) {
    let line_count = log_content.lines().count() as u16;
    // Auto-scroll: if content is taller than the view, scroll to the bottom.
    let inner_height = area.height.saturating_sub(3); // borders + hint line
    let scroll = line_count.saturating_sub(inner_height);

    let paragraph = Paragraph::new(log_content)
        .block(Block::default().borders(Borders::ALL).title(" Log Output "))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

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
