//! TUI entry point: terminal setup, event loop, view routing.

pub mod app;
pub mod prompt_view;
pub mod queue_view;
pub mod review_view;
pub mod tail_view;
pub mod widgets;

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use self::app::{Action, App, View};
use crate::error::Result;
use crate::traits::JobStore;
use crate::types::JobFilter;

/// Run the TUI application.
pub fn run<S: JobStore>(store: &S) -> Result<()> {
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

    let mut app = App::new();

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

        if app.should_quit {
            break;
        }

        // Periodic refresh from the store
        app.update_jobs(store.list_jobs(&JobFilter::default())?);
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
