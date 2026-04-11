//! TUI entry point: terminal setup, event loop, view routing.

pub mod app;
pub mod prompt_view;
pub mod queue_view;
pub mod review_view;
pub mod widgets;

use std::io;
use std::path::Path;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use self::app::{Action, App, View};
use crate::error::Result;
use crate::traits::JobStore;
use crate::types::JobFilter;

/// Errors from nudging the daemon via SIGUSR1.
enum NudgeError {
    /// Daemon process is not running (ESRCH).
    NotRunning,
    /// Insufficient permissions to signal the daemon (EPERM).
    PermissionDenied,
    /// PID file is missing or contains invalid data.
    InvalidPidFile(String),
}

/// Send SIGUSR1 to the daemon to wake the runner loop.
fn nudge_daemon(pid_file: &Path) -> std::result::Result<(), NudgeError> {
    let contents = std::fs::read_to_string(pid_file)
        .map_err(|e| NudgeError::InvalidPidFile(format!("{e}")))?;
    let pid: i32 = contents
        .trim()
        .parse()
        .map_err(|e| NudgeError::InvalidPidFile(format!("bad PID value: {e}")))?;
    if pid <= 0 {
        return Err(NudgeError::InvalidPidFile(format!("invalid PID: {pid}")));
    }
    signal::kill(Pid::from_raw(pid), Signal::SIGUSR1).map_err(|e| match e {
        nix::errno::Errno::ESRCH => NudgeError::NotRunning,
        nix::errno::Errno::EPERM => NudgeError::PermissionDenied,
        other => NudgeError::InvalidPidFile(format!("kill failed: {other}")),
    })
}

/// Run the TUI application.
pub fn run<S: JobStore>(store: &S, output_dir: &Path, logging_dir: &Path) -> Result<()> {
    let pid_file = logging_dir.join("reviewq.pid");
    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Install panic hook to restore terminal on crash
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    let mut app = App::new(output_dir.to_path_buf());

    // Initial load
    app.update_jobs(store.list_jobs(&JobFilter::default())?);

    // Event loop
    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        if event::poll(std::time::Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(action) = map_key(key, &app) {
                        app.dispatch(action);
                    }
                }
                Event::Mouse(mouse) => {
                    if let Some(action) = map_mouse(mouse, &app) {
                        app.dispatch(action);
                    }
                }
                _ => {}
            }
        }

        // Open browser if dispatch requested it.
        if let Some(path) = app.pending_open.take()
            && open::that(&path).is_err()
        {
            // Browser open failed — fall back to TUI review view.
            app.review_text = format!(
                "[Failed to open browser: {}]\n\n{}",
                path.display(),
                app.review_text
            );
            app.view = View::Review;
            // Reset scroll so the fallback review starts at the top
            // regardless of any previous content_scroll state.
            app.content_scroll = 0;
        }

        // Process pending cancel request (DB write BEFORE nudge).
        if let Some(job_id) = app.pending_cancel.take() {
            match store.request_cancel(job_id) {
                Ok(()) => {
                    // nudge stays true — daemon should wake to process the cancel.
                }
                Err(e) => {
                    // Cancel DB write failed — suppress the nudge.
                    app.pending_nudge = false;
                    app.flash(format!("Failed to request cancel: {e}"));
                }
            }
        }

        // Process pending retry request (DB write BEFORE nudge).
        if let Some(job_id) = app.pending_retry.take() {
            match store.retry_job(job_id) {
                Ok(()) => {
                    // nudge stays true — daemon should wake to process the retried job.
                }
                Err(e) => {
                    // Retry DB write failed — suppress the nudge.
                    app.pending_nudge = false;
                    app.flash(format!("Failed to retry job: {e}"));
                }
            }
        }

        // Nudge daemon if dispatch requested it.
        if app.pending_nudge {
            app.pending_nudge = false;
            match nudge_daemon(&pid_file) {
                Ok(()) => {
                    // Don't overwrite cancel message — only set if no message yet.
                    if app.status_message.is_none() {
                        app.flash("Nudged daemon to start review");
                    }
                }
                Err(NudgeError::NotRunning) => {
                    app.flash("Daemon is not running");
                }
                Err(NudgeError::PermissionDenied) => {
                    app.flash("Permission denied: cannot signal daemon");
                }
                Err(NudgeError::InvalidPidFile(detail)) => {
                    app.flash(format!("Daemon is not running (PID file: {detail})"));
                }
            }
        }

        if app.should_quit {
            break;
        }

        // Periodic refresh from the store. If the user pressed R, also emit a
        // completion flash so they see confirmation that the reload ran.
        let manual_refresh = std::mem::take(&mut app.pending_refresh);
        let jobs = store.list_jobs(&JobFilter::default())?;
        let job_count = jobs.len();
        app.update_jobs(jobs);
        if manual_refresh {
            app.flash(format!("Refreshed ({job_count} jobs)"));
        }

        // Expire any transient flash messages whose TTL has elapsed.
        app.tick_status();
    }

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;

    Ok(())
}

/// Route rendering to the appropriate view based on current app state.
fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();

    match app.view {
        View::Queue => queue_view::render(f, app, area),
        View::Review => review_view::render(f, app, area),
        View::Prompt => prompt_view::render(f, app, area),
    }
}

/// Map a key event to an action based on the current view.
///
/// Modal exit rule: the TUI can only be closed from the Queue view. In the
/// Review / Prompt views, `q`, `Esc`, and `Ctrl-C` all go back to the queue
/// rather than quitting, so the user always has a chance to review the list
/// before exiting.
fn map_key(key: event::KeyEvent, app: &App) -> Option<Action> {
    let ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');

    match app.view {
        View::Queue => {
            if ctrl_c {
                return Some(Action::Quit);
            }
            match key.code {
                KeyCode::Char('q') => Some(Action::Quit),
                KeyCode::Char('j') | KeyCode::Down => Some(Action::NavigateDown),
                KeyCode::Char('k') | KeyCode::Up => Some(Action::NavigateUp),
                KeyCode::Enter => Some(Action::SelectJob),
                KeyCode::Char('p') => Some(Action::ShowPrompt),
                KeyCode::Char('x') => Some(Action::CancelJob),
                KeyCode::Char('r') => Some(Action::RetryJob),
                KeyCode::Char('s') => Some(Action::StartReview),
                KeyCode::Char('c') => Some(Action::CopySessionId),
                KeyCode::Char('R') => Some(Action::Refresh),
                KeyCode::Char('o') => Some(Action::OpenInBrowser),
                _ => None,
            }
        }
        View::Review | View::Prompt => {
            // All exits funnel through GoBack — the Queue view is the only
            // place the user can actually quit or Ctrl-C out of the TUI.
            if ctrl_c {
                return Some(Action::GoBack);
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => Some(Action::GoBack),
                KeyCode::Char('o') if app.view == View::Review => Some(Action::OpenInBrowser),
                KeyCode::Up | KeyCode::Char('k') => Some(Action::ScrollContentUp),
                KeyCode::Down | KeyCode::Char('j') => Some(Action::ScrollContentDown),
                KeyCode::PageUp => Some(Action::ScrollContentPageUp),
                KeyCode::PageDown | KeyCode::Char(' ') => Some(Action::ScrollContentPageDown),
                _ => None,
            }
        }
    }
}

/// Map a mouse event to an action based on the current view.
///
/// - Queue: left-click selects the row under the cursor; wheel navigates the
///   selection up/down by one.
/// - Review / Prompt: wheel scrolls the content by one line.
fn map_mouse(mouse: MouseEvent, app: &App) -> Option<Action> {
    match app.view {
        View::Queue => match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let area = app.last_table_area?;
                let target = click_to_row_index(area, mouse.column, mouse.row, &app.table_state)?;
                if target < app.jobs.len() {
                    Some(Action::SelectRow(target))
                } else {
                    None
                }
            }
            MouseEventKind::ScrollDown => Some(Action::NavigateDown),
            MouseEventKind::ScrollUp => Some(Action::NavigateUp),
            _ => None,
        },
        View::Review | View::Prompt => match mouse.kind {
            MouseEventKind::ScrollDown => Some(Action::ScrollContentDown),
            MouseEventKind::ScrollUp => Some(Action::ScrollContentUp),
            _ => None,
        },
    }
}

/// Convert a mouse click at `(col, row)` inside the queue table area into an
/// absolute job index, using the table's current scroll offset.
///
/// Returns `None` if the click is outside `area`. The returned index is not
/// bounded by the current `jobs.len()` — callers must clamp before using it.
///
/// The queue view's `Table` widget is rendered with
/// `.highlight_symbol("▸ ")`, which prepends a 2-column marker to the
/// selected row. This function intentionally does NOT subtract those
/// columns from `col`: selection is row-oriented, so a click anywhere in
/// a row (including on the highlight symbol columns) resolves to the same
/// row index. The only column-related check is that the click is within
/// `area`'s horizontal bounds.
///
/// This function also relies on the invariant that the `Table` widget is
/// rendered WITHOUT a `.header(...)` call in `queue_view::render`, so
/// `area.y` aligns with data row 0. See the comment at the
/// `app.last_table_area = Some(chunks[4])` assignment for details.
fn click_to_row_index(
    area: Rect,
    col: u16,
    row: u16,
    state: &ratatui::widgets::TableState,
) -> Option<usize> {
    if col < area.x || col >= area.x.saturating_add(area.width) {
        return None;
    }
    if row < area.y || row >= area.y.saturating_add(area.height) {
        return None;
    }
    let relative = (row - area.y) as usize;
    Some(state.offset().saturating_add(relative))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui::widgets::TableState;
    use std::path::PathBuf;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn make_app() -> App {
        App::new(PathBuf::from("/tmp"))
    }

    // -------- modal exit rule: only Queue can quit --------

    #[test]
    fn ctrl_c_quits_from_queue_view() {
        let app = make_app();
        assert!(matches!(map_key(ctrl('c'), &app), Some(Action::Quit)));
    }

    #[test]
    fn ctrl_c_from_prompt_view_goes_back_not_quit() {
        let mut app = make_app();
        app.view = View::Prompt;
        assert!(matches!(map_key(ctrl('c'), &app), Some(Action::GoBack)));
    }

    #[test]
    fn ctrl_c_from_review_view_goes_back_not_quit() {
        let mut app = make_app();
        app.view = View::Review;
        assert!(matches!(map_key(ctrl('c'), &app), Some(Action::GoBack)));
    }

    #[test]
    fn q_from_prompt_view_goes_back_not_quit() {
        let mut app = make_app();
        app.view = View::Prompt;
        assert!(matches!(
            map_key(key(KeyCode::Char('q')), &app),
            Some(Action::GoBack)
        ));
    }

    #[test]
    fn q_from_review_view_goes_back_not_quit() {
        let mut app = make_app();
        app.view = View::Review;
        assert!(matches!(
            map_key(key(KeyCode::Char('q')), &app),
            Some(Action::GoBack)
        ));
    }

    #[test]
    fn q_from_queue_quits() {
        let app = make_app();
        assert!(matches!(
            map_key(key(KeyCode::Char('q')), &app),
            Some(Action::Quit)
        ));
    }

    #[test]
    fn esc_from_prompt_goes_back() {
        let mut app = make_app();
        app.view = View::Prompt;
        assert!(matches!(
            map_key(key(KeyCode::Esc), &app),
            Some(Action::GoBack)
        ));
    }

    #[test]
    fn esc_from_review_goes_back() {
        // Symmetry with esc_from_prompt_goes_back: Esc from any modal
        // view must route to GoBack, never to Quit. The modal-exit
        // contract covers all three keys (q, Esc, Ctrl-C) and both
        // views (Review, Prompt).
        let mut app = make_app();
        app.view = View::Review;
        assert!(matches!(
            map_key(key(KeyCode::Esc), &app),
            Some(Action::GoBack)
        ));
    }

    // -------- content scroll keybindings --------

    #[test]
    fn arrow_keys_scroll_content_in_review() {
        let mut app = make_app();
        app.view = View::Review;
        assert!(matches!(
            map_key(key(KeyCode::Down), &app),
            Some(Action::ScrollContentDown)
        ));
        assert!(matches!(
            map_key(key(KeyCode::Up), &app),
            Some(Action::ScrollContentUp)
        ));
        assert!(matches!(
            map_key(key(KeyCode::PageDown), &app),
            Some(Action::ScrollContentPageDown)
        ));
        assert!(matches!(
            map_key(key(KeyCode::PageUp), &app),
            Some(Action::ScrollContentPageUp)
        ));
    }

    // -------- mouse handling in Queue --------

    #[test]
    fn wheel_in_queue_navigates_selection() {
        let app = make_app();
        assert!(matches!(
            map_mouse(mouse(MouseEventKind::ScrollDown, 0, 0), &app),
            Some(Action::NavigateDown)
        ));
        assert!(matches!(
            map_mouse(mouse(MouseEventKind::ScrollUp, 0, 0), &app),
            Some(Action::NavigateUp)
        ));
    }

    #[test]
    fn left_click_outside_table_area_is_ignored() {
        let mut app = make_app();
        app.last_table_area = Some(Rect::new(0, 4, 80, 10));
        let evt = mouse(MouseEventKind::Down(MouseButton::Left), 40, 2);
        assert!(map_mouse(evt, &app).is_none());
    }

    #[test]
    fn left_click_without_cached_area_is_ignored() {
        let app = make_app();
        let evt = mouse(MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert!(map_mouse(evt, &app).is_none());
    }

    #[test]
    fn left_click_on_visible_row_selects_that_row() {
        let mut app = make_app();
        // Fake a 3-job list with the table body at y=4..14.
        use crate::types::{AgentKind, Job, JobStatus, RepoId};
        use chrono::Utc;
        let jobs: Vec<Job> = (0..3)
            .map(|i| Job {
                id: i,
                repo: RepoId::new("o", "r"),
                pr_number: 1,
                head_sha: "abc".into(),
                agent_kind: AgentKind::Claude,
                status: JobStatus::Queued,
                leased_at: None,
                lease_expires: None,
                retry_count: 0,
                max_retries: 3,
                command: None,
                prompt_template: None,
                pid: None,
                exit_code: None,
                stdout_path: None,
                stderr_path: None,
                worktree_path: None,
                review_output: None,
                session_id: None,
                cancel_requested_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .collect();
        app.update_jobs(jobs);
        app.last_table_area = Some(Rect::new(0, 4, 80, 10));

        // Click on the second row (area.y + 1 = 5).
        let evt = mouse(MouseEventKind::Down(MouseButton::Left), 10, 5);
        assert!(matches!(map_mouse(evt, &app), Some(Action::SelectRow(1))));
    }

    #[test]
    fn click_to_row_index_respects_table_offset() {
        let mut state = TableState::default();
        *state.offset_mut() = 5;
        let area = Rect::new(0, 4, 80, 10);
        // Click on first visible row → offset(5) + 0 = 5.
        assert_eq!(click_to_row_index(area, 10, 4, &state), Some(5));
        // Click on second visible row → 6.
        assert_eq!(click_to_row_index(area, 10, 5, &state), Some(6));
    }

    #[test]
    fn wheel_in_prompt_scrolls_content() {
        let mut app = make_app();
        app.view = View::Prompt;
        assert!(matches!(
            map_mouse(mouse(MouseEventKind::ScrollDown, 0, 0), &app),
            Some(Action::ScrollContentDown)
        ));
        assert!(matches!(
            map_mouse(mouse(MouseEventKind::ScrollUp, 0, 0), &app),
            Some(Action::ScrollContentUp)
        ));
    }
}
