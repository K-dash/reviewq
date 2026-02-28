//! Queue view: displays the list of jobs with keybinding support.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};

use super::app::App;
use super::widgets;

/// Key binding hints displayed at the bottom of the queue view.
const KEY_HINTS: &str =
    " j/↓ Down  k/↑ Up  Enter Open  t Tail  p Prompt  s Start  x Cancel  r Retry  o PR  q Quit ";

/// Render the queue view.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let header_cells = ["ID", "Repo", "PR#", "SHA", "Agent", "Status", "Created"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = app
        .jobs
        .iter()
        .map(|job| {
            let (badge_text, badge_style) = widgets::status_badge(job.status);
            Row::new(vec![
                Cell::from(job.id.to_string()),
                Cell::from(job.repo.full_name()),
                Cell::from(format!("#{}", job.pr_number)),
                Cell::from(widgets::short_sha(&job.head_sha).to_owned()),
                Cell::from(job.agent_kind.to_string()),
                Cell::from(badge_text).style(badge_style),
                Cell::from(widgets::format_timestamp(&job.created_at)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Min(20),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(9),
        Constraint::Length(18),
    ];

    let title = format!(" reviewq — {} job(s) ", app.jobs.len());
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = TableState::default();
    state.select(Some(app.selected_index));

    f.render_stateful_widget(table, area, &mut state);

    // Render status message or key hints in the last line
    if area.height > 2 {
        let hint_area = Rect {
            x: area.x,
            y: area.y + area.height - 1,
            width: area.width,
            height: 1,
        };

        let hint_line = if let Some(ref msg) = app.status_message {
            Line::from(Span::styled(
                format!(" {msg} "),
                Style::default().fg(ratatui::style::Color::Yellow),
            ))
        } else {
            Line::from(Span::styled(
                KEY_HINTS,
                Style::default().fg(ratatui::style::Color::DarkGray),
            ))
        };

        f.render_widget(hint_line, hint_area);
    }
}
