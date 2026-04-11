//! TUI application state and action dispatch.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::widgets::TableState;

use crate::types::{Job, JobStatus};

/// Default lifetime for transient status "flash" messages.
const STATUS_FLASH_TTL: Duration = Duration::from_secs(3);

/// Active view in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Queue,
    Review,
    Prompt,
}

/// Actions that can be dispatched from keybindings.
#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    NavigateUp,
    NavigateDown,
    SelectJob,
    ShowPrompt,
    CancelJob,
    RetryJob,
    StartReview,
    CopySessionId,
    OpenInBrowser,
    GoBack,
    Refresh,
    /// Select a row in the queue by absolute index (e.g. from a mouse click).
    /// The index is clamped to the current job list bounds.
    SelectRow(usize),
    /// Scroll the active content view (Review / Prompt) up by one line.
    ScrollContentUp,
    /// Scroll the active content view (Review / Prompt) down by one line.
    ScrollContentDown,
    /// Scroll the active content view up by one page.
    ScrollContentPageUp,
    /// Scroll the active content view down by one page.
    ScrollContentPageDown,
}

/// Application state.
pub struct App {
    pub view: View,
    pub jobs: Vec<Job>,
    pub selected_index: usize,
    pub should_quit: bool,
    pub status_message: Option<String>,
    /// Cached review text for the review view (fallback).
    pub review_text: String,
    /// Cached command text for the prompt view.
    pub command_text: String,
    /// Output directory for review HTML/markdown files.
    pub output_dir: PathBuf,
    /// Path to open in the browser after dispatch completes.
    /// The event loop in `tui/mod.rs` drains this to call `open::that`.
    pub pending_open: Option<PathBuf>,
    /// Whether to nudge the daemon to wake up and process queued jobs.
    pub pending_nudge: bool,
    /// Job ID to cancel (deferred to the event loop for DB access).
    pub pending_cancel: Option<i64>,
    /// Job ID to retry (deferred to the event loop for DB access).
    pub pending_retry: Option<i64>,
    /// Whether the user asked for a manual refresh (handled by the event loop).
    pub pending_refresh: bool,
    /// Persistent queue table scroll/selection state, used so mouse clicks
    /// and scroll events can map between viewport rows and absolute indices.
    pub table_state: TableState,
    /// Last-rendered screen rect of the queue table body. Updated by the
    /// queue view renderer and consumed by the mouse handler.
    pub last_table_area: Option<Rect>,
    /// Scroll offset (in lines) for the Review / Prompt content views.
    pub content_scroll: u16,
    /// Height of the Review / Prompt content area, in lines. Used to
    /// convert "page" scrolls into line counts.
    pub content_viewport_height: u16,
    /// When the current `status_message` should auto-clear, if ever.
    status_expires_at: Option<Instant>,
}

impl App {
    pub fn new(output_dir: PathBuf) -> Self {
        Self {
            view: View::Queue,
            jobs: Vec::new(),
            selected_index: 0,
            should_quit: false,
            status_message: None,
            review_text: String::new(),
            command_text: String::new(),
            output_dir,
            pending_open: None,
            pending_nudge: false,
            pending_cancel: None,
            pending_retry: None,
            pending_refresh: false,
            table_state: TableState::default(),
            last_table_area: None,
            content_scroll: 0,
            content_viewport_height: 0,
            status_expires_at: None,
        }
    }

    /// Set a transient "flash" status message that auto-expires after the
    /// default TTL. Use this for user feedback that should disappear on its
    /// own — e.g. "Refreshed", "Copied", "Cancel requested".
    pub fn flash(&mut self, msg: impl Into<String>) {
        self.flash_with_ttl(msg, STATUS_FLASH_TTL);
    }

    /// Flash a status message with a custom TTL. Primarily useful for tests
    /// that need to force immediate expiry without sleeping.
    pub fn flash_with_ttl(&mut self, msg: impl Into<String>, ttl: Duration) {
        self.status_message = Some(msg.into());
        self.status_expires_at = Some(Instant::now() + ttl);
    }

    /// Clear the status message if its flash expiry has passed. Called from
    /// the event loop on every tick so stale messages don't linger.
    pub fn tick_status(&mut self) {
        if let Some(expires_at) = self.status_expires_at
            && Instant::now() >= expires_at
        {
            self.status_message = None;
            self.status_expires_at = None;
        }
    }

    /// Get the currently selected job, if any.
    pub fn selected_job(&self) -> Option<&Job> {
        self.jobs.get(self.selected_index)
    }

    /// Number of text lines in the content view currently shown. Returns
    /// 0 for the Queue view. Used to clamp scroll offsets so the user
    /// cannot scroll past the end of the content.
    ///
    /// NOTE: counts logical (source) lines, not display lines after
    /// word-wrap. Both Review and Prompt views render with
    /// `Paragraph::wrap`, so the actual scrollable height is at
    /// least this count and may be larger for content that contains
    /// long lines. As a result, `clamp_content_scroll` can permit a
    /// small amount of over-scroll past the last visible display
    /// line. This is a known UX gap, not a safety issue; ratatui
    /// 0.29 does not expose a post-layout wrapped-line count through
    /// its public API, so we cannot compute the exact scrollable
    /// height at this layer.
    pub fn content_line_count(&self) -> usize {
        match self.view {
            View::Queue => 0,
            View::Review => self.review_text.lines().count(),
            View::Prompt => self.command_text.lines().count(),
        }
    }

    /// Clamp `content_scroll` so at least one line of content remains visible.
    /// Call this after render when `content_viewport_height` has been updated.
    pub fn clamp_content_scroll(&mut self) {
        // Saturating cast: `ratatui::Paragraph::scroll` takes `(u16, u16)`,
        // so any logical line count above `u16::MAX` is unrepresentable
        // at the render layer anyway. Clamping at the cast makes the
        // truncation deliberate instead of wrapping to zero.
        let total = self.content_line_count().min(u16::MAX as usize) as u16;
        let viewport = self.content_viewport_height.max(1);
        let max_scroll = total.saturating_sub(viewport);
        if self.content_scroll > max_scroll {
            self.content_scroll = max_scroll;
        }
    }

    /// Handle an action, mutating state.
    pub fn dispatch(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.should_quit = true;
            }
            Action::NavigateUp => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
            }
            Action::NavigateDown => {
                if !self.jobs.is_empty() && self.selected_index < self.jobs.len() - 1 {
                    self.selected_index += 1;
                }
            }
            Action::SelectJob => {
                if let Some(job) = self.selected_job() {
                    if let Some(ref markdown) = job.review_output {
                        // Clone fields needed after releasing the borrow on self.
                        let markdown = markdown.clone();
                        let owner = job.repo.owner.clone();
                        let repo_name = job.repo.name.clone();
                        let pr_number = job.pr_number;
                        let head_sha = job.head_sha.clone();
                        let created_at = job.created_at;

                        match crate::review_html::write_review_files(
                            &markdown,
                            &owner,
                            &repo_name,
                            pr_number,
                            &head_sha,
                            created_at,
                            &self.output_dir,
                        ) {
                            Ok(artifact) => {
                                // Defer browser open to the event loop (avoids
                                // opening a browser during tests).
                                self.flash(format!(
                                    "Opened review: {}",
                                    artifact.html_path.display()
                                ));
                                self.pending_open = Some(artifact.html_path);
                            }
                            Err(e) => {
                                // File generation failed — fall back to TUI
                                // with error note prepended so the user sees why.
                                self.review_text =
                                    format!("[HTML generation failed: {e}]\n\n{markdown}");
                                self.view = View::Review;
                                self.content_scroll = 0;
                            }
                        }
                    } else {
                        let job_id = job.id;
                        self.flash(format!("No review output yet for job {job_id}"));
                    }
                }
            }
            Action::ShowPrompt => {
                if let Some(job) = self.selected_job() {
                    self.command_text = build_prompt_display(job, &self.output_dir);
                    self.view = View::Prompt;
                    self.content_scroll = 0;
                }
            }
            Action::CancelJob => {
                if let Some(job) = self.selected_job() {
                    if job.is_cancel_requested() {
                        self.flash(format!("Cancel already requested for job {}", job.id));
                    } else if !job.status.is_terminal() {
                        let job_id = job.id;
                        self.pending_cancel = Some(job_id);
                        self.pending_nudge = true;
                        self.flash(format!("Cancel requested for job {job_id}"));
                    } else {
                        self.flash(format!("Job {} is already in terminal state", job.id));
                    }
                }
            }
            Action::RetryJob => {
                if let Some(job) = self.selected_job() {
                    if job.status == JobStatus::Failed || job.status == JobStatus::Canceled {
                        let job_id = job.id;
                        self.pending_retry = Some(job_id);
                        self.pending_nudge = true;
                        self.flash(format!("Retry requested for job {job_id}"));
                    } else {
                        self.flash(format!("Job {} is not in a retriable state", job.id));
                    }
                }
            }
            Action::StartReview => {
                if let Some(job) = self.selected_job() {
                    if job.status == JobStatus::Queued {
                        self.pending_nudge = true;
                        self.flash("Nudging daemon to start review...");
                    } else {
                        self.flash(format!("Job {} is not in queued state", job.id));
                    }
                }
            }
            Action::CopySessionId => {
                if let Some(job) = self.selected_job() {
                    if let Some(ref sid) = job.session_id {
                        // Use worktree path as cwd — agent sessions are
                        // stored under ~/.claude/projects/<cwd>.
                        let cmd = job
                            .agent_kind
                            .resume_command(sid, job.worktree_path.as_deref());
                        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&cmd)) {
                            Ok(()) => {
                                self.flash(format!("Copied: {cmd}"));
                            }
                            Err(e) => {
                                self.flash(format!("Clipboard error: {e}"));
                            }
                        }
                    } else {
                        self.flash("No session ID available");
                    }
                }
            }
            Action::OpenInBrowser => {
                if let Some(job) = self.selected_job() {
                    let url = format!(
                        "https://github.com/{}/pull/{}",
                        job.repo.full_name(),
                        job.pr_number
                    );
                    let _ = open::that(&url);
                }
            }
            Action::GoBack => {
                self.view = View::Queue;
                self.status_message = None;
                self.status_expires_at = None;
                self.content_scroll = 0;
            }
            Action::Refresh => {
                // Defer the actual reload to the event loop; it already has
                // access to the store. The loop will flash "Refreshed" on
                // success, which auto-clears after the flash TTL.
                self.pending_refresh = true;
            }
            Action::SelectRow(idx) => {
                if !self.jobs.is_empty() {
                    self.selected_index = idx.min(self.jobs.len() - 1);
                }
            }
            Action::ScrollContentUp => {
                self.content_scroll = self.content_scroll.saturating_sub(1);
            }
            Action::ScrollContentDown => {
                self.content_scroll = self.content_scroll.saturating_add(1);
            }
            Action::ScrollContentPageUp => {
                let page = self.content_viewport_height.max(1);
                self.content_scroll = self.content_scroll.saturating_sub(page);
            }
            Action::ScrollContentPageDown => {
                let page = self.content_viewport_height.max(1);
                self.content_scroll = self.content_scroll.saturating_add(page);
            }
        }
    }

    /// Update the job list (called after refresh from DB).
    pub fn update_jobs(&mut self, jobs: Vec<Job>) {
        self.jobs = jobs;
        // Clamp selected index to valid range
        if self.jobs.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.jobs.len() {
            self.selected_index = self.jobs.len() - 1;
        }
    }
}

/// Build the display text for the prompt view.
///
/// Shows the command with template variables (`{output_path}`, etc.) and
/// `REVIEWQ_*` environment variables expanded, followed by the full rendered
/// prompt content read from the prompt file written by the executor.
fn build_prompt_display(job: &Job, output_dir: &Path) -> String {
    let raw_cmd = job.command.as_deref().unwrap_or("(no command)");

    // Resolve values for interpolation.
    let repo = job.repo.full_name();
    let pr_number = job.pr_number.to_string();
    let pr_url = format!("https://github.com/{}/pull/{}", repo, pr_number);
    let job_id = job.id.to_string();
    let worktree_display = job
        .worktree_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<worktree>".into());
    let output_path = job
        .worktree_path
        .as_ref()
        .map(|p| p.join("REVIEW.md").display().to_string())
        .unwrap_or_else(|| "<output_path>".into());
    let prompt_file_path = output_dir
        .join(format!("job-{}-prompt.txt", job.id))
        .display()
        .to_string();

    // Expand {var} template placeholders.
    let cmd = raw_cmd
        .replace("{pr_url}", &pr_url)
        .replace("{repo}", &repo)
        .replace("{pr_number}", &pr_number)
        .replace("{head_sha}", &job.head_sha)
        .replace("{worktree_path}", &worktree_display)
        .replace("{job_id}", &job_id)
        .replace("{output_path}", &output_path)
        .replace("{prompt_file}", &prompt_file_path);

    // Read the rendered prompt from the file the executor writes.
    let prompt_content =
        std::fs::read_to_string(output_dir.join(format!("job-{}-prompt.txt", job.id)))
            .unwrap_or_else(|_| "(prompt file not available)".into());

    format!("── Command ──\n{cmd}\n\n── Prompt ──\n{prompt_content}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, RepoId};
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_app() -> (App, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let app = App::new(tmp.path().to_path_buf());
        (app, tmp)
    }

    fn make_job(id: i64, status: JobStatus) -> Job {
        Job {
            id,
            repo: RepoId::new("owner", "repo"),
            pr_number: 1,
            head_sha: "abc123".into(),
            agent_kind: AgentKind::Claude,
            status,
            leased_at: None,
            lease_expires: None,
            retry_count: 0,
            max_retries: 3,
            command: Some("echo test".into()),
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
        }
    }

    #[test]
    fn new_app_defaults() {
        let (app, _tmp) = make_app();
        assert_eq!(app.view, View::Queue);
        assert!(app.jobs.is_empty());
        assert_eq!(app.selected_index, 0);
        assert!(!app.should_quit);
    }

    #[test]
    fn navigation_clamps_to_bounds() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![
            make_job(1, JobStatus::Queued),
            make_job(2, JobStatus::Running),
            make_job(3, JobStatus::Succeeded),
        ]);

        // Navigate down past end
        app.dispatch(Action::NavigateDown);
        app.dispatch(Action::NavigateDown);
        app.dispatch(Action::NavigateDown); // should clamp
        assert_eq!(app.selected_index, 2);

        // Navigate up past beginning
        app.dispatch(Action::NavigateUp);
        app.dispatch(Action::NavigateUp);
        app.dispatch(Action::NavigateUp); // should clamp
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn update_jobs_clamps_index() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![
            make_job(1, JobStatus::Queued),
            make_job(2, JobStatus::Queued),
            make_job(3, JobStatus::Queued),
        ]);
        app.selected_index = 2;

        // Shrink list — index should clamp
        app.update_jobs(vec![make_job(1, JobStatus::Queued)]);
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn quit_action_sets_flag() {
        let (mut app, _tmp) = make_app();
        app.dispatch(Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn go_back_returns_to_queue() {
        let (mut app, _tmp) = make_app();
        app.view = View::Review;
        app.dispatch(Action::GoBack);
        assert_eq!(app.view, View::Queue);
    }

    #[test]
    fn select_job_without_review_output_shows_message() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![make_job(1, JobStatus::Succeeded)]);

        app.dispatch(Action::SelectJob);
        assert_eq!(app.view, View::Queue);
        assert_eq!(
            app.status_message.as_deref(),
            Some("No review output yet for job 1")
        );
    }

    #[test]
    fn show_prompt_displays_interpolated_command_and_prompt() {
        let (mut app, _tmp) = make_app();
        let mut job = make_job(1, JobStatus::Running);
        job.command = Some("claude -p {output_path}".into());
        job.worktree_path = Some(PathBuf::from("/tmp/wt"));
        app.update_jobs(vec![job]);

        // Write a fake prompt file.
        let prompt_file = app.output_dir.join("job-1-prompt.txt");
        std::fs::write(&prompt_file, "Review owner/repo PR #1").expect("write prompt file");

        app.dispatch(Action::ShowPrompt);
        assert_eq!(app.view, View::Prompt);
        // Command should have {output_path} expanded.
        assert!(
            app.command_text.contains("claude -p /tmp/wt/REVIEW.md"),
            "template vars not expanded: {}",
            app.command_text
        );
        // Rendered prompt content should be present.
        assert!(
            app.command_text.contains("Review owner/repo PR #1"),
            "prompt content missing: {}",
            app.command_text
        );
    }

    #[test]
    fn show_prompt_without_prompt_file_shows_fallback() {
        let (mut app, _tmp) = make_app();
        let job = make_job(1, JobStatus::Queued);
        app.update_jobs(vec![job]);

        app.dispatch(Action::ShowPrompt);
        assert_eq!(app.view, View::Prompt);
        assert!(
            app.command_text.contains("(prompt file not available)"),
            "fallback message missing: {}",
            app.command_text
        );
    }

    #[test]
    fn select_job_with_review_output_generates_html() {
        let (mut app, _tmp) = make_app();
        let mut job = make_job(1, JobStatus::Succeeded);
        job.review_output = Some("# LGTM\n\nAll good.".into());
        app.update_jobs(vec![job]);

        app.dispatch(Action::SelectJob);

        // dispatch sets pending_open instead of calling open::that directly,
        // so no browser is launched during tests.
        assert!(app.pending_open.is_some(), "should have pending_open set");
        let html_path = app.pending_open.as_ref().unwrap();
        assert!(html_path.exists(), "HTML file should have been written");
        assert!(html_path.to_str().unwrap().ends_with(".html"));

        // Also verify .md was written alongside.
        let md_path = html_path.with_extension("md");
        assert!(md_path.exists(), "markdown file should have been written");

        // Should stay on Queue view (browser open is deferred to event loop).
        assert_eq!(app.view, View::Queue);
    }

    #[test]
    fn retry_job_sets_pending_retry_for_failed() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![make_job(1, JobStatus::Failed)]);
        app.dispatch(Action::RetryJob);
        assert_eq!(app.pending_retry, Some(1));
        assert!(app.pending_nudge);
        assert!(
            app.status_message
                .as_deref()
                .unwrap()
                .contains("Retry requested")
        );
    }

    #[test]
    fn retry_job_sets_pending_retry_for_canceled() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![make_job(1, JobStatus::Canceled)]);
        app.dispatch(Action::RetryJob);
        assert_eq!(app.pending_retry, Some(1));
        assert!(app.pending_nudge);
    }

    #[test]
    fn retry_job_rejected_for_non_terminal() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![make_job(1, JobStatus::Running)]);
        app.dispatch(Action::RetryJob);
        assert!(app.pending_retry.is_none());
        assert!(!app.pending_nudge);
        assert!(
            app.status_message
                .as_deref()
                .unwrap()
                .contains("not in a retriable state")
        );
    }

    #[test]
    fn copy_session_id_without_session_shows_message() {
        let (mut app, _tmp) = make_app();
        let job = make_job(1, JobStatus::Succeeded);
        app.update_jobs(vec![job]);

        app.dispatch(Action::CopySessionId);
        assert_eq!(
            app.status_message.as_deref(),
            Some("No session ID available")
        );
    }

    #[test]
    fn flash_sets_message_and_persists_until_expiry() {
        let (mut app, _tmp) = make_app();
        app.flash("hello world");
        assert_eq!(app.status_message.as_deref(), Some("hello world"));
        // Not yet expired → tick is a no-op.
        app.tick_status();
        assert_eq!(app.status_message.as_deref(), Some("hello world"));
    }

    #[test]
    fn tick_status_clears_expired_flash() {
        let (mut app, _tmp) = make_app();
        // Zero-duration TTL expires instantly without sleeping.
        app.flash_with_ttl("gone soon", Duration::from_millis(0));
        assert_eq!(app.status_message.as_deref(), Some("gone soon"));
        app.tick_status();
        assert!(
            app.status_message.is_none(),
            "expired flash should be cleared"
        );
    }

    #[test]
    fn tick_status_is_noop_when_no_message() {
        let (mut app, _tmp) = make_app();
        app.tick_status();
        assert!(app.status_message.is_none());
    }

    #[test]
    fn flash_replaces_prior_message_and_resets_expiry() {
        let (mut app, _tmp) = make_app();
        app.flash_with_ttl("first", Duration::from_millis(0));
        // Overwrite with a fresh long-lived flash — prior expiry should not
        // carry over and clear the new message.
        app.flash("second");
        app.tick_status();
        assert_eq!(app.status_message.as_deref(), Some("second"));
    }

    #[test]
    fn select_row_clamps_to_jobs_length() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![
            make_job(1, JobStatus::Queued),
            make_job(2, JobStatus::Queued),
        ]);
        app.dispatch(Action::SelectRow(99));
        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn select_row_is_noop_when_jobs_empty() {
        let (mut app, _tmp) = make_app();
        app.dispatch(Action::SelectRow(5));
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn scroll_content_respects_saturating_bounds_and_viewport() {
        let (mut app, _tmp) = make_app();
        app.view = View::Prompt;
        app.command_text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // 20 lines of content
        assert_eq!(app.content_line_count(), 20);

        // Scroll up from 0 should saturate to 0.
        app.dispatch(Action::ScrollContentUp);
        assert_eq!(app.content_scroll, 0);

        // Scroll down increments.
        app.dispatch(Action::ScrollContentDown);
        assert_eq!(app.content_scroll, 1);

        // Page down uses viewport height (fallback 1 until the view is rendered).
        app.content_viewport_height = 5;
        app.dispatch(Action::ScrollContentPageDown);
        assert_eq!(app.content_scroll, 6);

        // Clamp does not allow scrolling past (content - viewport).
        app.content_scroll = 50;
        app.clamp_content_scroll();
        assert_eq!(app.content_scroll, 20 - 5);

        // Page up uses viewport height.
        app.dispatch(Action::ScrollContentPageUp);
        assert_eq!(app.content_scroll, 10);
    }

    #[test]
    fn go_back_resets_content_scroll() {
        let (mut app, _tmp) = make_app();
        app.view = View::Review;
        app.review_text = "a\nb\nc\nd".into();
        app.content_scroll = 3;
        app.dispatch(Action::GoBack);
        assert_eq!(app.view, View::Queue);
        assert_eq!(app.content_scroll, 0);
    }

    #[test]
    fn show_prompt_resets_content_scroll() {
        let (mut app, _tmp) = make_app();
        let job = make_job(1, JobStatus::Queued);
        app.update_jobs(vec![job]);
        app.content_scroll = 42;
        app.dispatch(Action::ShowPrompt);
        assert_eq!(app.view, View::Prompt);
        assert_eq!(app.content_scroll, 0);
    }

    #[test]
    fn refresh_action_sets_pending_refresh_without_stale_status() {
        let (mut app, _tmp) = make_app();
        app.dispatch(Action::Refresh);
        assert!(
            app.pending_refresh,
            "Refresh action must set pending_refresh flag"
        );
        // Regression guard for the "Refreshing..." stuck-forever bug:
        // dispatch itself must not leave a stale "Refreshing..." message;
        // the event loop publishes "Refreshed" after it actually reloads.
        assert!(
            app.status_message.is_none(),
            "Refresh dispatch should not set a stale status message, got {:?}",
            app.status_message
        );
    }
}
