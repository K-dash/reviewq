//! Queue view: displays the list of jobs in a borderless layout.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table, TableState};

use super::app::App;
use super::widgets::{self, SELECTED_STYLE, STATUS_STYLE, TITLE_STYLE};

/// Render the queue view.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    // Count jobs by status for the status line.
    let (queued, running, done, failed) = {
        let mut q = 0u32;
        let mut r = 0u32;
        let mut d = 0u32;
        let mut fl = 0u32;
        for job in &app.jobs {
            match job.status {
                crate::types::JobStatus::Queued | crate::types::JobStatus::Leased => q += 1,
                crate::types::JobStatus::Running => r += 1,
                crate::types::JobStatus::Succeeded => d += 1,
                crate::types::JobStatus::Failed => fl += 1,
                crate::types::JobStatus::Canceled => {}
            }
        }
        (q, r, d, fl)
    };

    // Vertical layout: title + status + header + separator + table + scroll + flash + help
    let chunks = Layout::vertical([
        Constraint::Length(1), // title
        Constraint::Length(1), // status summary
        Constraint::Length(1), // table header
        Constraint::Length(1), // separator
        Constraint::Min(3),    // job rows
        Constraint::Length(1), // scroll indicator
        Constraint::Length(1), // flash / status message
        Constraint::Length(1), // help bar
    ])
    .split(area);

    // 1. Title
    let title = format!("reviewq queue — {} job(s)", app.jobs.len());
    f.render_widget(Line::styled(title, TITLE_STYLE), chunks[0]);

    // 2. Status summary
    let status_line = format!(
        "Queued: {} | Running: {} | Done: {} | Failed: {}",
        queued, running, done, failed
    );
    f.render_widget(Line::styled(status_line, STATUS_STYLE), chunks[1]);

    // 3. Table header (repo width computed after table area is known)
    // We use chunks[4] width here too; it's available after the layout split.
    let fixed_cols = 6 + 7 + 9 + 8 + 9 + 18;
    let header_repo_w = (chunks[4].width as usize)
        .saturating_sub(fixed_cols as usize)
        .clamp(12, 40);
    let header_spans = vec![Span::styled(
        format!(
            "  {:<6} {:<rw$} {:<7} {:<9} {:<8} {:<9} {:<18}",
            "ID",
            "Repo",
            "PR#",
            "SHA",
            "Agent",
            "Status",
            "Created",
            rw = header_repo_w
        ),
        Style::default()
            .fg(ratatui::style::Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )];
    f.render_widget(Line::from(header_spans), chunks[2]);

    // 4. Separator
    let sep_width = area.width as usize;
    let separator = "─".repeat(sep_width);
    f.render_widget(Line::styled(separator, widgets::SEPARATOR_STYLE), chunks[3]);

    // 5. Job table (borderless)
    let rows: Vec<Row> = app
        .jobs
        .iter()
        .map(|job| {
            let (badge_text, badge_style) = widgets::status_badge_for_job(job);
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

    // Dynamic column widths: fixed columns use Length, Repo gets the remainder.
    // Fixed total: ID(6) + PR#(7) + SHA(9) + Agent(8) + Status(9) + Created(18) + gaps ≈ 57
    // Repo gets whatever is left, clamped to a reasonable max.
    let fixed = 6 + 7 + 9 + 8 + 9 + 18;
    let table_width = chunks[4].width;
    let repo_width = table_width.saturating_sub(fixed).clamp(12, 40);

    let widths = [
        Constraint::Length(6),
        Constraint::Length(repo_width),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(9),
        Constraint::Length(18),
    ];

    let table = Table::new(rows, widths)
        .row_highlight_style(SELECTED_STYLE)
        .highlight_symbol("▸ ");

    let mut state = TableState::default();
    state.select(Some(app.selected_index));
    f.render_stateful_widget(table, chunks[4], &mut state);

    // 6. Scroll indicator
    let visible_rows = chunks[4].height as usize;
    let total_jobs = app.jobs.len();
    if total_jobs > visible_rows {
        let start = app.selected_index.saturating_sub(visible_rows / 2);
        let end = (start + visible_rows).min(total_jobs);
        let scroll_info = format!("[showing {}-{} of {}]", start + 1, end, total_jobs);
        f.render_widget(Line::styled(scroll_info, STATUS_STYLE), chunks[5]);
    }

    // 7. Flash / status message
    if let Some(ref msg) = app.status_message {
        let flash = Line::styled(
            format!(" {msg}"),
            Style::default().fg(ratatui::style::Color::Yellow),
        );
        f.render_widget(flash, chunks[6]);
    }

    // 8. Help bar
    widgets::render_help_bar(
        f,
        chunks[7],
        &[
            ("j/↓", "down"),
            ("k/↑", "up"),
            ("↵", "open"),
            ("p", "prompt"),
            ("s", "start"),
            ("c", "copy sid"),
            ("x", "cancel"),
            ("r", "retry"),
            ("o", "PR"),
            ("R", "refresh"),
            ("q", "quit"),
        ],
    );
}
