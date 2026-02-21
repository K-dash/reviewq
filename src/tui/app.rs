//! TUI application state and action dispatch.

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
    /// Cached review text for the review view.
    pub review_text: String,
    /// Cached command text for the prompt view.
    pub command_text: String,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            view: View::Queue,
            jobs: Vec::new(),
            selected_index: 0,
            should_quit: false,
            status_message: None,
            log_content: String::new(),
            review_text: String::new(),
            command_text: String::new(),
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
                    if let Some(ref output) = job.review_output {
                        self.review_text = output.clone();
                        self.view = View::Review;
                    } else {
                        self.status_message = Some("No review output available".to_owned());
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
                    self.command_text = job
                        .command
                        .clone()
                        .unwrap_or_else(|| "(no command)".to_owned());
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
            pid: None,
            exit_code: None,
            stdout_path: None,
            stderr_path: None,
            worktree_path: None,
            review_output: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn new_app_defaults() {
        let app = App::new();
        assert_eq!(app.view, View::Queue);
        assert!(app.jobs.is_empty());
        assert_eq!(app.selected_index, 0);
        assert!(!app.should_quit);
    }

    #[test]
    fn navigation_clamps_to_bounds() {
        let mut app = App::new();
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
        let mut app = App::new();
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
        let mut app = App::new();
        app.dispatch(Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn go_back_returns_to_queue() {
        let mut app = App::new();
        app.view = View::Tail;
        app.dispatch(Action::GoBack);
        assert_eq!(app.view, View::Queue);
    }
}
