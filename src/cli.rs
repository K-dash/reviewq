//! CLI subcommand implementations: status, tail, open.

use std::io::{self, BufRead, Seek, SeekFrom};

use crate::db::Database;
use crate::error::{Result, ReviewqError};
use crate::traits::JobStore;
use crate::types::{JobFilter, JobStatus, RepoId};

/// Execute the `status` subcommand: print the job queue as a table.
pub fn status(db: &Database, status_filter: Option<&str>, repo_filter: Option<&str>) -> Result<()> {
    let filter = build_filter(status_filter, repo_filter)?;
    let jobs = db.list_jobs(&filter)?;

    if jobs.is_empty() {
        println!("No jobs found.");
        return Ok(());
    }

    // Print table header
    println!(
        "{:<6} {:<25} {:<6} {:<10} {:<8} {:<10} Created",
        "ID", "Repo", "PR#", "SHA", "Agent", "Status"
    );
    println!("{}", "-".repeat(80));

    for job in &jobs {
        let sha_display = if job.head_sha.len() > 8 {
            &job.head_sha[..8]
        } else {
            &job.head_sha
        };
        println!(
            "{:<6} {:<25} {:<6} {:<10} {:<8} {:<10} {}",
            job.id,
            job.repo.full_name(),
            job.pr_number,
            sha_display,
            job.agent_kind,
            job.status,
            job.created_at.format("%Y-%m-%d %H:%M"),
        );
    }

    Ok(())
}

/// Execute the `tail` subcommand: stream job logs.
///
/// If the job is still running, polls the file for new content until the job
/// finishes or the user interrupts with Ctrl-C.
pub fn tail(db: &Database, job_id: i64) -> Result<()> {
    let jobs = db.list_jobs(&JobFilter {
        pr_number: None,
        status: None,
        repo: None,
    })?;

    let job = jobs
        .into_iter()
        .find(|j| j.id == job_id)
        .ok_or_else(|| ReviewqError::Process(format!("job {job_id} not found")))?;

    let log_path = job
        .stdout_path
        .as_ref()
        .ok_or_else(|| ReviewqError::Process(format!("job {job_id} has no log file")))?;

    if !log_path.exists() {
        return Err(ReviewqError::Process(format!(
            "log file does not exist: {}",
            log_path.display()
        )));
    }

    let mut file = std::fs::File::open(log_path)?;
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    // Print existing content
    {
        let reader = io::BufReader::new(&file);
        for line in reader.lines() {
            let line = line?;
            io::Write::write_all(&mut stdout_lock, line.as_bytes())?;
            io::Write::write_all(&mut stdout_lock, b"\n")?;
        }
    }

    // If the job is still active, poll for new content
    if !job.status.is_terminal() {
        let poll_interval = std::time::Duration::from_millis(500);
        let mut pos = file.seek(SeekFrom::End(0))?;

        loop {
            std::thread::sleep(poll_interval);

            // Re-check job status
            let current_jobs = db.list_jobs(&JobFilter::default())?;
            let current = current_jobs.iter().find(|j| j.id == job_id);
            let is_done = current.is_none_or(|j| j.status.is_terminal());

            // Read any new content
            let new_pos = file.metadata()?.len();
            if new_pos > pos {
                file.seek(SeekFrom::Start(pos))?;
                let reader = io::BufReader::new(&file);
                for line in reader.lines() {
                    let line = line?;
                    io::Write::write_all(&mut stdout_lock, line.as_bytes())?;
                    io::Write::write_all(&mut stdout_lock, b"\n")?;
                }
                pos = new_pos;
            }

            if is_done {
                break;
            }
        }
    }

    Ok(())
}

/// Execute the `open` subcommand: open a PR URL or job result in the browser.
pub fn open_target(db: &Database, target: &str) -> Result<()> {
    // Try parsing target as a job ID first
    if let Ok(job_id) = target.parse::<i64>() {
        let jobs = db.list_jobs(&JobFilter::default())?;
        if let Some(job) = jobs.iter().find(|j| j.id == job_id) {
            let url = format!(
                "https://github.com/{}/pull/{}",
                job.repo.full_name(),
                job.pr_number
            );
            open::that(&url)
                .map_err(|e| ReviewqError::Process(format!("failed to open browser: {e}")))?;
            println!("Opened {url}");
            return Ok(());
        }
        return Err(ReviewqError::Process(format!("job {job_id} not found")));
    }

    // Otherwise treat as a URL and open directly
    open::that(target)
        .map_err(|e| ReviewqError::Process(format!("failed to open browser: {e}")))?;
    println!("Opened {target}");
    Ok(())
}

/// Build a `JobFilter` from optional CLI string arguments.
fn build_filter(status: Option<&str>, repo: Option<&str>) -> Result<JobFilter> {
    let status = match status {
        Some(s) => {
            let parsed = JobStatus::from_db(s).ok_or_else(|| {
                ReviewqError::Config(format!(
                    "invalid status filter '{s}': expected one of queued, leased, running, succeeded, failed, canceled"
                ))
            })?;
            Some(parsed)
        }
        None => None,
    };

    let repo = match repo {
        Some(r) => {
            let (owner, name) = r.split_once('/').ok_or_else(|| {
                ReviewqError::Config(format!(
                    "invalid repo filter '{r}': expected 'owner/name' format"
                ))
            })?;
            Some(RepoId::new(owner, name))
        }
        None => None,
    };

    Ok(JobFilter {
        status,
        repo,
        pr_number: None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, NewJob};

    fn test_db() -> Database {
        Database::open_in_memory().expect("in-memory DB should open")
    }

    fn sample_job(sha: &str) -> NewJob {
        NewJob {
            repo: RepoId::new("owner", "repo"),
            pr_number: 42,
            head_sha: sha.into(),
            agent_kind: AgentKind::Claude,
            command: Some("echo review".into()),
            prompt_template: None,
            max_retries: 3,
        }
    }

    #[test]
    fn build_filter_no_args() {
        let filter = build_filter(None, None).expect("should succeed");
        assert!(filter.status.is_none());
        assert!(filter.repo.is_none());
    }

    #[test]
    fn build_filter_with_status() {
        let filter = build_filter(Some("queued"), None).expect("should succeed");
        assert_eq!(filter.status, Some(JobStatus::Queued));
    }

    #[test]
    fn build_filter_with_repo() {
        let filter = build_filter(None, Some("org/repo")).expect("should succeed");
        let repo = filter.repo.expect("should have repo");
        assert_eq!(repo.owner, "org");
        assert_eq!(repo.name, "repo");
    }

    #[test]
    fn build_filter_invalid_status() {
        let result = build_filter(Some("bogus"), None);
        assert!(result.is_err());
    }

    #[test]
    fn build_filter_invalid_repo() {
        let result = build_filter(None, Some("noslash"));
        assert!(result.is_err());
    }

    #[test]
    fn status_empty_db() {
        let db = test_db();
        // Should not panic; prints "No jobs found."
        status(&db, None, None).expect("status should succeed on empty db");
    }

    #[test]
    fn status_with_jobs() {
        let db = test_db();
        db.enqueue(sample_job("aabbccdd11223344")).expect("enqueue");
        status(&db, None, None).expect("status should succeed");
    }

    #[test]
    fn open_target_not_found() {
        let db = test_db();
        let result = open_target(&db, "999");
        assert!(result.is_err());
    }
}
