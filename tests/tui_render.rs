//! TUI render-layer snapshot tests.
//!
//! These tests drive each view's `render` function against a ratatui
//! `TestBackend` so we can assert the rendered cell buffer without a real
//! terminal. This is what lets the "TUI e2e" guarantee live inside
//! `cargo test` instead of depending on a session-scoped skill marker.

use chrono::{DateTime, Utc};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use reviewq::daemon::DaemonHealth;
use reviewq::tui::app::{App, View};
use reviewq::tui::{prompt_view, queue_view, review_view};
use reviewq::types::{AgentKind, Job, JobStatus, RepoId};
use std::time::Duration;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    Terminal::new(backend).expect("TestBackend terminal should construct")
}

/// Dump a `Buffer` to a single string, one row per line. Styles are stripped —
/// only the cell symbol is kept — which lets tests use plain substring
/// assertions against the rendered frame.
fn buffer_to_string(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::with_capacity((area.width as usize + 1) * area.height as usize);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[track_caller]
fn assert_buffer_contains(term: &Terminal<TestBackend>, needle: &str) {
    let dump = buffer_to_string(term.backend().buffer());
    assert!(
        dump.contains(needle),
        "buffer did not contain `{needle}`. Full buffer:\n{dump}"
    );
}

#[track_caller]
fn assert_buffer_does_not_contain(term: &Terminal<TestBackend>, needle: &str) {
    let dump = buffer_to_string(term.backend().buffer());
    assert!(
        !dump.contains(needle),
        "buffer unexpectedly contained `{needle}`. Full buffer:\n{dump}"
    );
}

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
        .expect("valid rfc3339 literal")
        .with_timezone(&Utc)
}

fn make_job(id: i64, status: JobStatus, owner: &str, repo: &str) -> Job {
    Job {
        id,
        repo: RepoId::new(owner, repo),
        pr_number: (100 + id) as u64,
        head_sha: format!("deadbeef{id:02}cafebabe"),
        agent_kind: AgentKind::Claude,
        status,
        leased_at: None,
        lease_expires: None,
        retry_count: 0,
        max_retries: 3,
        command: Some("echo hi".into()),
        prompt_template: None,
        pid: None,
        exit_code: None,
        stdout_path: None,
        stderr_path: None,
        worktree_path: None,
        review_output: None,
        session_id: None,
        cancel_requested_at: None,
        created_at: fixed_time(),
        updated_at: fixed_time(),
    }
}

fn make_app() -> (App, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let mut app = App::new(tmp.path().to_path_buf());
    // Default to an alive daemon so the majority of render tests that
    // do not care about the health badge render a clean title bar.
    // Tests that specifically exercise the Dead / Unknown paths
    // override this field after calling make_app().
    //
    // This helper intentionally differs from the identically-named
    // helper in `src/tui/mod.rs` (which leaves `daemon_status = None`
    // so its dispatch-guard tests can drive the path explicitly).
    // Do not unify the two — each suite needs a different default.
    app.daemon_status = Some(DaemonHealth::Alive(1));
    (app, tmp)
}

fn draw_queue(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| {
        let area = f.area();
        queue_view::render(f, app, area);
    })
    .expect("draw queue");
}

fn draw_review(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| {
        let area = f.area();
        review_view::render(f, app, area);
    })
    .expect("draw review");
}

fn draw_prompt(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| {
        let area = f.area();
        prompt_view::render(f, app, area);
    })
    .expect("draw prompt");
}

// ---------------------------------------------------------------------------
// Queue view
// ---------------------------------------------------------------------------

#[test]
fn queue_renders_title_with_zero_jobs_when_empty() {
    let mut term = test_terminal(100, 24);
    let (mut app, _tmp) = make_app();
    // Mark the daemon as alive so the title bar does NOT get a DOWN
    // badge in the baseline "title with zero jobs" render.
    app.daemon_status = Some(DaemonHealth::Alive(1));
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "reviewq queue");
    assert_buffer_contains(&term, "0 job(s)");
}

// ---------------------------------------------------------------------------
// Daemon-health title badge
//
// Regression tests for the silent-corruption class of bugs where the TUI
// gives no visual feedback that the daemon is down. The badge lives
// inline on the title row so every queue frame advertises daemon state.
// ---------------------------------------------------------------------------

#[test]
fn title_bar_shows_daemon_down_when_dead() {
    let mut term = test_terminal(100, 24);
    let (mut app, _tmp) = make_app();
    app.daemon_status = Some(DaemonHealth::Dead);
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "reviewq queue");
    assert_buffer_contains(&term, "[daemon: DOWN]");
}

#[test]
fn title_bar_shows_daemon_down_when_status_unknown() {
    // `None` (not yet evaluated) defaults to the conservative side so a
    // startup glitch cannot paint a misleading "alive" title.
    let mut term = test_terminal(100, 24);
    let (mut app, _tmp) = make_app();
    app.daemon_status = None;
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "[daemon: DOWN]");
}

#[test]
fn title_bar_hides_daemon_badge_when_alive() {
    let mut term = test_terminal(100, 24);
    let (mut app, _tmp) = make_app();
    app.daemon_status = Some(DaemonHealth::Alive(1234));
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "reviewq queue");
    assert_buffer_does_not_contain(&term, "[daemon: DOWN]");
}

#[test]
fn title_bar_narrow_width_does_not_panic() {
    // Narrow terminal: 40 columns. The render must not panic and must
    // still contain the job count. The badge may be truncated on a
    // very narrow screen but the baseline title must remain intact.
    let mut term = test_terminal(40, 10);
    let (mut app, _tmp) = make_app();
    app.daemon_status = Some(DaemonHealth::Dead);
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "reviewq queue");
    assert_buffer_contains(&term, "0 job(s)");
}

#[test]
fn queue_renders_status_summary_counts_by_state() {
    let mut term = test_terminal(120, 24);
    let (mut app, _tmp) = make_app();
    app.update_jobs(vec![
        make_job(1, JobStatus::Queued, "owner", "alpha"),
        make_job(2, JobStatus::Running, "owner", "beta"),
        make_job(3, JobStatus::Succeeded, "owner", "gamma"),
        make_job(4, JobStatus::Failed, "owner", "delta"),
    ]);
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "Queued: 1");
    assert_buffer_contains(&term, "Running: 1");
    assert_buffer_contains(&term, "Done: 1");
    assert_buffer_contains(&term, "Failed: 1");
}

#[test]
fn queue_renders_job_rows_with_repo_names() {
    let mut term = test_terminal(120, 24);
    let (mut app, _tmp) = make_app();
    app.update_jobs(vec![
        make_job(7, JobStatus::Queued, "octocat", "hello-world"),
        make_job(8, JobStatus::Running, "rust-lang", "rust"),
    ]);
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "octocat/hello-world");
    assert_buffer_contains(&term, "rust-lang/rust");
}

#[test]
fn queue_highlights_selected_row_with_arrow_marker() {
    let mut term = test_terminal(120, 24);
    let (mut app, _tmp) = make_app();
    app.update_jobs(vec![
        make_job(1, JobStatus::Queued, "o", "r"),
        make_job(2, JobStatus::Queued, "o", "r"),
        make_job(3, JobStatus::Queued, "o", "r"),
    ]);
    app.selected_index = 1;
    draw_queue(&mut term, &mut app);

    // The render must propagate selected_index into the persistent
    // TableState so subsequent mouse / scroll events stay in sync.
    assert_eq!(app.table_state.selected(), Some(1));

    // The queue layout is: title(y=0) + status(y=1) + header(y=2) +
    // separator(y=3), so the table body starts at y=4. With three jobs and
    // no scroll, selected_index=1 must produce the "▸ " highlight marker on
    // y=5 specifically — not just "somewhere in the buffer", which would
    // pass even if the wrong row were highlighted.
    let dump = buffer_to_string(term.backend().buffer());
    let lines: Vec<&str> = dump.lines().collect();
    let highlight_row = 5;
    assert!(
        lines
            .get(highlight_row)
            .is_some_and(|line| line.contains("▸")),
        "expected highlight marker on row {highlight_row} for selected_index=1. \
         Full buffer:\n{dump}"
    );
    // And the *other* data rows must NOT carry the marker.
    for y in [4usize, 6] {
        assert!(
            lines.get(y).is_some_and(|line| !line.contains("▸")),
            "row {y} should not be highlighted. Full buffer:\n{dump}"
        );
    }
}

#[test]
fn queue_render_populates_last_table_area_for_mouse_clicks() {
    let mut term = test_terminal(120, 24);
    let (mut app, _tmp) = make_app();
    app.update_jobs(vec![make_job(1, JobStatus::Queued, "o", "r")]);
    assert!(app.last_table_area.is_none());

    draw_queue(&mut term, &mut app);

    let area = app
        .last_table_area
        .expect("render must publish last_table_area so mouse clicks map to rows");
    assert!(area.height > 0);
    assert!(area.width > 0);
}

#[test]
fn queue_renders_flash_status_message() {
    let mut term = test_terminal(120, 24);
    let (mut app, _tmp) = make_app();
    app.flash("test flash abc123");
    draw_queue(&mut term, &mut app);
    assert_buffer_contains(&term, "test flash abc123");
}

#[test]
fn queue_does_not_render_flash_after_tick_clears_it() {
    let mut term = test_terminal(120, 24);
    let (mut app, _tmp) = make_app();
    // ZERO TTL — the flash is set with `expires_at = now()` and is then
    // tested with `now() >= expires_at` inside `tick_status`. Because
    // `Instant::now()` is monotonic, the second call is guaranteed to be
    // `>=` the first, so the flash always expires by the time tick runs.
    // No sleep is needed and the test cannot flap on a slow host.
    app.flash_with_ttl("expiring now", Duration::ZERO);
    app.tick_status();
    draw_queue(&mut term, &mut app);
    assert_buffer_does_not_contain(&term, "expiring now");
}

// ---------------------------------------------------------------------------
// Review view
// ---------------------------------------------------------------------------

#[test]
fn review_view_renders_title_and_body() {
    let mut term = test_terminal(80, 20);
    let (mut app, _tmp) = make_app();
    app.view = View::Review;
    app.review_text = "Here is the LGTM message".into();
    draw_review(&mut term, &mut app);
    assert_buffer_contains(&term, "Review Output");
    assert_buffer_contains(&term, "LGTM message");
}

#[test]
fn review_view_publishes_viewport_height_for_scroll_clamping() {
    let mut term = test_terminal(80, 20);
    let (mut app, _tmp) = make_app();
    app.view = View::Review;
    app.review_text = "short content".into();
    assert_eq!(app.content_viewport_height, 0);

    draw_review(&mut term, &mut app);

    // The 80x20 terminal allocates 1 row to the title and 1 row to the help
    // bar, so the content area must be ~18 rows. A tighter bound than `> 0`
    // catches layout regressions that would accidentally collapse the
    // content viewport.
    assert!(
        app.content_viewport_height >= 10,
        "viewport height should be at least 10 rows in an 80x20 terminal, got {}",
        app.content_viewport_height
    );
}

// ---------------------------------------------------------------------------
// Prompt view
// ---------------------------------------------------------------------------

#[test]
fn prompt_view_renders_command_and_prompt_sections() {
    let mut term = test_terminal(100, 24);
    let (mut app, _tmp) = make_app();
    app.view = View::Prompt;
    app.command_text = "── Command ──\nclaude -p /tmp/out.md\n\n── Prompt ──\nreview please".into();
    draw_prompt(&mut term, &mut app);
    // Both structural section headers and the wrapped content lines must
    // appear, so renaming a section in prompt_view.rs surfaces as a test
    // failure rather than a silent regression.
    assert_buffer_contains(&term, "── Command ──");
    assert_buffer_contains(&term, "claude -p /tmp/out.md");
    assert_buffer_contains(&term, "── Prompt ──");
    assert_buffer_contains(&term, "review please");
}
