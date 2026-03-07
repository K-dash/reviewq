//! TUI application state and action dispatch.

use std::path::{Path, PathBuf};

use crate::types::{Job, JobStatus};

/// Active view in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Queue,
    Tail,
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
    TailLog,
    ShowPrompt,
    CancelJob,
    RetryJob,
    StartReview,
    CopySessionId,
    OpenInBrowser,
    GoBack,
    Refresh,
}

/// Application state.
pub struct App {
    pub view: View,
    pub jobs: Vec<Job>,
    pub selected_index: usize,
    pub should_quit: bool,
    pub status_message: Option<String>,
    /// Cached log content for the tail view.
    pub log_content: String,
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
}

impl App {
    pub fn new(output_dir: PathBuf) -> Self {
        Self {
            view: View::Queue,
            jobs: Vec::new(),
            selected_index: 0,
            should_quit: false,
            status_message: None,
            log_content: String::new(),
            review_text: String::new(),
            command_text: String::new(),
            output_dir,
            pending_open: None,
            pending_nudge: false,
        }
    }

    /// Get the currently selected job, if any.
    pub fn selected_job(&self) -> Option<&Job> {
        self.jobs.get(self.selected_index)
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
                                self.status_message = Some(format!(
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
                            }
                        }
                    } else {
                        // No review output yet — fall back to tail view so the
                        // user can see logs (useful for running / queued jobs).
                        let content = load_log_content(job);
                        self.log_content = content;
                        self.view = View::Tail;
                    }
                }
            }
            Action::TailLog => {
                if let Some(job) = self.selected_job() {
                    let content = load_log_content(job);
                    self.log_content = content;
                    self.view = View::Tail;
                }
            }
            Action::ShowPrompt => {
                if let Some(job) = self.selected_job() {
                    self.command_text = build_prompt_display(job, &self.output_dir);
                    self.view = View::Prompt;
                }
            }
            Action::CancelJob => {
                if let Some(job) = self.selected_job() {
                    if !job.status.is_terminal() {
                        self.status_message = Some(format!("Cancel requested for job {}", job.id));
                    } else {
                        self.status_message =
                            Some(format!("Job {} is already in terminal state", job.id));
                    }
                }
            }
            Action::RetryJob => {
                if let Some(job) = self.selected_job() {
                    if job.status == JobStatus::Failed || job.status == JobStatus::Canceled {
                        self.status_message = Some(format!("Retry requested for job {}", job.id));
                    } else {
                        self.status_message =
                            Some(format!("Job {} is not in a retriable state", job.id));
                    }
                }
            }
            Action::StartReview => {
                if let Some(job) = self.selected_job() {
                    if job.status == JobStatus::Queued {
                        self.pending_nudge = true;
                        self.status_message = Some("Nudging daemon to start review...".to_owned());
                    } else {
                        self.status_message =
                            Some(format!("Job {} is not in queued state", job.id));
                    }
                }
            }
            Action::CopySessionId => {
                if let Some(job) = self.selected_job() {
                    if let Some(ref sid) = job.session_id {
                        let cmd = job.agent_kind.resume_command(sid);
                        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&cmd)) {
                            Ok(()) => {
                                self.status_message = Some(format!("Copied: {cmd}"));
                            }
                            Err(e) => {
                                self.status_message = Some(format!("Clipboard error: {e}"));
                            }
                        }
                    } else {
                        self.status_message = Some("No session ID available".to_owned());
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
            }
            Action::Refresh => {
                self.status_message = Some("Refreshing...".to_owned());
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

/// Load log content from a job's stdout/stderr files.
fn load_log_content(job: &Job) -> String {
    let mut content = String::new();

    if let Some(ref path) = job.stdout_path {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                content.push_str("=== stdout ===\n");
                content.push_str(&text);
            }
            Err(e) => {
                content.push_str(&format!("(unable to read stdout: {e})\n"));
            }
        }
    }

    if let Some(ref path) = job.stderr_path {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str("=== stderr ===\n");
                content.push_str(&text);
            }
            Err(e) => {
                content.push_str(&format!("(unable to read stderr: {e})\n"));
            }
        }
    }

    if content.is_empty() {
        content.push_str("(no log output available)");
    }

    content
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
        app.view = View::Tail;
        app.dispatch(Action::GoBack);
        assert_eq!(app.view, View::Queue);
    }

    #[test]
    fn select_job_without_review_output_shows_tail() {
        let (mut app, _tmp) = make_app();
        app.update_jobs(vec![make_job(1, JobStatus::Succeeded)]);

        app.dispatch(Action::SelectJob);
        assert_eq!(app.view, View::Tail);
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
}
