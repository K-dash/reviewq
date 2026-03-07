//! TUI entry point: terminal setup, event loop, view routing.

pub mod app;
pub mod prompt_view;
pub mod queue_view;
pub mod review_view;
pub mod tail_view;
pub mod widgets;

use std::io;
use std::path::Path;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

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
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Install panic hook to restore terminal on crash
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    let mut app = App::new(output_dir.to_path_buf());

    // Initial load
    app.update_jobs(store.list_jobs(&JobFilter::default())?);

    // Event loop
    loop {
        terminal.draw(|f| draw(f, &app))?;

        if event::poll(std::time::Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && let Some(action) = map_key(key, &app)
        {
            app.dispatch(action);
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
        }

        // Nudge daemon if dispatch requested it.
        if app.pending_nudge {
            app.pending_nudge = false;
            match nudge_daemon(&pid_file) {
                Ok(()) => {
                    app.status_message = Some("Nudged daemon to start review".to_owned());
                }
                Err(NudgeError::NotRunning) => {
                    app.status_message = Some("Daemon is not running".to_owned());
                }
                Err(NudgeError::PermissionDenied) => {
                    app.status_message = Some("Permission denied: cannot signal daemon".to_owned());
                }
                Err(NudgeError::InvalidPidFile(detail)) => {
                    app.status_message =
                        Some(format!("Daemon is not running (PID file: {detail})"));
                }
            }
        }

        if app.should_quit {
            break;
        }

        // Periodic refresh from the store
        app.update_jobs(store.list_jobs(&JobFilter::default())?);

        // Auto-refresh tail view content.
        if app.view == View::Tail {
            app.refresh_tail_log();
        }
    }

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}

/// Route rendering to the appropriate view based on current app state.
fn draw(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();

    match app.view {
        View::Queue => queue_view::render(f, app, area),
        View::Tail => tail_view::render(f, &app.log_content, area),
        View::Review => review_view::render(f, &app.review_text, area),
        View::Prompt => prompt_view::render(f, &app.command_text, area),
    }
}

/// Map a key event to an action based on the current view.
fn map_key(key: event::KeyEvent, app: &App) -> Option<Action> {
    // Ctrl-C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }

    match app.view {
        View::Queue => match key.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('j') | KeyCode::Down => Some(Action::NavigateDown),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::NavigateUp),
            KeyCode::Enter => Some(Action::SelectJob),
            KeyCode::Char('t') => Some(Action::TailLog),
            KeyCode::Char('p') => Some(Action::ShowPrompt),
            KeyCode::Char('x') => Some(Action::CancelJob),
            KeyCode::Char('r') => Some(Action::RetryJob),
            KeyCode::Char('s') => Some(Action::StartReview),
            KeyCode::Char('c') => Some(Action::CopySessionId),
            KeyCode::Char('R') => Some(Action::Refresh),
            KeyCode::Char('o') => Some(Action::OpenInBrowser),
            _ => None,
        },
        View::Tail | View::Review | View::Prompt => match key.code {
            KeyCode::Esc => Some(Action::GoBack),
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('o') if app.view == View::Review => Some(Action::OpenInBrowser),
            _ => None,
        },
    }
}
