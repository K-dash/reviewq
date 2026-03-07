//! SQLite-backed state management implementing the [`JobStore`] trait.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, ReviewqError};
use crate::traits::JobStore;
use crate::types::{AgentKind, IdempotencyKey, Job, JobFilter, JobStatus, NewJob, RepoId};

/// Default lease duration in minutes.
const DEFAULT_LEASE_MINUTES: i64 = 5;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS processed (
    repo_owner TEXT NOT NULL,
    repo_name  TEXT NOT NULL,
    pr_number  INTEGER NOT NULL,
    head_sha   TEXT NOT NULL,
    agent_kind TEXT NOT NULL DEFAULT 'claude',
    processed_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (repo_owner, repo_name, pr_number, head_sha, agent_kind)
);

CREATE TABLE IF NOT EXISTS jobs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_owner    TEXT NOT NULL,
    repo_name     TEXT NOT NULL,
    pr_number     INTEGER NOT NULL,
    head_sha      TEXT NOT NULL,
    agent_kind    TEXT NOT NULL DEFAULT 'claude',
    status        TEXT NOT NULL DEFAULT 'queued',
    leased_at     TEXT,
    lease_expires TEXT,
    retry_count   INTEGER NOT NULL DEFAULT 0,
    max_retries   INTEGER NOT NULL DEFAULT 3,
    command       TEXT,
    prompt_template TEXT,
    pid           INTEGER,
    exit_code     INTEGER,
    stdout_path   TEXT,
    stderr_path   TEXT,
    worktree_path TEXT,
    review_output TEXT,
    session_id    TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(repo_owner, repo_name, pr_number, head_sha, agent_kind)
);
"#;

// ---------------------------------------------------------------------------
// Database wrapper
// ---------------------------------------------------------------------------

/// SQLite-backed implementation of [`JobStore`].
///
/// The `Connection` is wrapped in a [`Mutex`] because `rusqlite::Connection`
/// is `Send` but not `Sync`. The mutex makes `Database` safe to share across
/// threads via `&Database` (satisfying the `Sync` bound on `JobStore`).
pub struct Database {
    conn: Mutex<Connection>,
    lease_minutes: i64,
}

impl Database {
    /// Open (or create) the database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Mutex::new(conn),
            lease_minutes: DEFAULT_LEASE_MINUTES,
        };
        db.initialize()?;
        Ok(db)
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Mutex::new(conn),
            lease_minutes: DEFAULT_LEASE_MINUTES,
        };
        db.initialize()?;
        Ok(db)
    }

    /// Set the lease duration.
    pub fn with_lease_minutes(mut self, minutes: i64) -> Self {
        self.lease_minutes = minutes;
        self
    }

    fn initialize(&self) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Self::migrate(&conn)?;
        Ok(())
    }

    /// Run idempotent migrations for schema evolution.
    fn migrate(conn: &Connection) -> Result<()> {
        // Add prompt_template column if it doesn't exist (added in v0.x).
        let has_prompt_template: bool = conn
            .prepare("PRAGMA table_info(jobs)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .any(|name| name.as_deref() == Ok("prompt_template"));

        if !has_prompt_template {
            conn.execute_batch("ALTER TABLE jobs ADD COLUMN prompt_template TEXT;")?;
        }

        // Add session_id column if it doesn't exist.
        let has_session_id: bool = conn
            .prepare("PRAGMA table_info(jobs)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .any(|name| name.as_deref() == Ok("session_id"));

        if !has_session_id {
            conn.execute_batch("ALTER TABLE jobs ADD COLUMN session_id TEXT;")?;
        }
        Ok(())
    }

    /// Lock the connection and return a guard.
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("db mutex poisoned")
    }
}

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

fn parse_datetime(s: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|naive| naive.and_utc())
        .unwrap_or_default()
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let status_str: String = row.get("status")?;
    let leased_at: Option<String> = row.get("leased_at")?;
    let lease_expires: Option<String> = row.get("lease_expires")?;
    let created_at: String = row.get("created_at")?;
    let updated_at: String = row.get("updated_at")?;
    let stdout_path: Option<String> = row.get("stdout_path")?;
    let stderr_path: Option<String> = row.get("stderr_path")?;
    let worktree_path: Option<String> = row.get("worktree_path")?;

    Ok(Job {
        id: row.get("id")?,
        repo: RepoId::new(
            row.get::<_, String>("repo_owner")?,
            row.get::<_, String>("repo_name")?,
        ),
        pr_number: row.get::<_, i64>("pr_number")? as u64,
        head_sha: row.get("head_sha")?,
        agent_kind: AgentKind::from_db(&row.get::<_, String>("agent_kind")?),
        status: JobStatus::from_db(&status_str).unwrap_or(JobStatus::Queued),
        leased_at: leased_at.map(|s| parse_datetime(&s)),
        lease_expires: lease_expires.map(|s| parse_datetime(&s)),
        retry_count: row.get("retry_count")?,
        max_retries: row.get("max_retries")?,
        command: row.get("command")?,
        prompt_template: row.get("prompt_template")?,
        pid: row.get::<_, Option<i64>>("pid")?.map(|p| p as u32),
        exit_code: row.get("exit_code")?,
        stdout_path: stdout_path.map(Into::into),
        stderr_path: stderr_path.map(Into::into),
        worktree_path: worktree_path.map(Into::into),
        review_output: row.get("review_output")?,
        session_id: row.get("session_id")?,
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}

// ---------------------------------------------------------------------------
// JobStore implementation
// ---------------------------------------------------------------------------

impl JobStore for Database {
    fn enqueue(&self, job: NewJob) -> Result<Job> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO jobs (repo_owner, repo_name, pr_number, head_sha, agent_kind, command, prompt_template, max_retries)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                job.repo.owner,
                job.repo.name,
                job.pr_number as i64,
                job.head_sha,
                job.agent_kind.as_db_str(),
                job.command,
                job.prompt_template,
                job.max_retries,
            ],
        )?;

        let id = conn.last_insert_rowid();
        let row = conn.query_row("SELECT * FROM jobs WHERE id = ?1", params![id], row_to_job)?;
        Ok(row)
    }

    fn lease_next(&self) -> Result<Option<Job>> {
        let conn = self.lock_conn();
        // Atomic lease acquisition: single UPDATE statement avoids TOCTOU races.
        let result = conn
            .query_row(
                &format!(
                    "UPDATE jobs
                     SET status = 'leased',
                         leased_at = datetime('now'),
                         lease_expires = datetime('now', '+{} minutes'),
                         updated_at = datetime('now')
                     WHERE id = (
                         SELECT id FROM jobs
                         WHERE status = 'queued'
                         ORDER BY created_at ASC
                         LIMIT 1
                     )
                     RETURNING *",
                    self.lease_minutes
                ),
                [],
                row_to_job,
            )
            .optional()?;
        Ok(result)
    }

    fn complete(&self, id: i64, status: JobStatus, exit_code: Option<i32>) -> Result<()> {
        let conn = self.lock_conn();
        let rows = conn.execute(
            "UPDATE jobs SET status = ?1, exit_code = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            params![status.as_db_str(), exit_code, id],
        )?;
        if rows == 0 {
            return Err(ReviewqError::Database(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    fn cancel(&self, id: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE jobs SET status = 'canceled', updated_at = datetime('now')
             WHERE id = ?1 AND status NOT IN ('succeeded', 'failed', 'canceled')",
            params![id],
        )?;
        Ok(())
    }

    fn is_processed(&self, key: &IdempotencyKey) -> Result<bool> {
        let conn = self.lock_conn();
        // Only succeeded/running/leased/queued jobs block re-enqueueing.
        // Failed and canceled jobs are eligible for retry.
        let exists: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM jobs
                WHERE repo_owner = ?1 AND repo_name = ?2
                  AND pr_number = ?3 AND head_sha = ?4 AND agent_kind = ?5
                  AND status NOT IN ('failed', 'canceled')
            )",
            params![
                key.repo.owner,
                key.repo.name,
                key.pr_number as i64,
                key.head_sha,
                key.agent_kind.as_db_str(),
            ],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    fn list_jobs(&self, filter: &JobFilter) -> Result<Vec<Job>> {
        let conn = self.lock_conn();
        let mut sql = "SELECT * FROM jobs WHERE 1=1".to_owned();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(status) = filter.status {
            param_values.push(Box::new(status.as_db_str().to_owned()));
            sql.push_str(&format!(" AND status = ?{}", param_values.len()));
        }
        if let Some(ref repo) = filter.repo {
            param_values.push(Box::new(repo.owner.clone()));
            sql.push_str(&format!(" AND repo_owner = ?{}", param_values.len()));
            param_values.push(Box::new(repo.name.clone()));
            sql.push_str(&format!(" AND repo_name = ?{}", param_values.len()));
        }
        if let Some(pr) = filter.pr_number {
            param_values.push(Box::new(pr as i64));
            sql.push_str(&format!(" AND pr_number = ?{}", param_values.len()));
        }

        sql.push_str(" ORDER BY created_at DESC");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let jobs = stmt
            .query_map(params_refs.as_slice(), row_to_job)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    fn find_stale_leases(&self) -> Result<Vec<Job>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM jobs
             WHERE status = 'leased'
               AND lease_expires IS NOT NULL
               AND lease_expires < datetime('now')",
        )?;
        let jobs = stmt
            .query_map([], row_to_job)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    fn mark_running(&self, id: i64, pid: u32) -> Result<()> {
        let conn = self.lock_conn();
        let rows = conn.execute(
            "UPDATE jobs SET status = 'running', pid = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'leased'",
            params![pid as i64, id],
        )?;
        if rows == 0 {
            return Err(ReviewqError::Database(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    fn store_review_output(&self, id: i64, markdown: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE jobs SET review_output = ?1, updated_at = datetime('now')
             WHERE id = ?2",
            params![markdown, id],
        )?;
        Ok(())
    }

    fn store_session_id(&self, id: i64, session_id: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE jobs SET session_id = ?1, updated_at = datetime('now')
             WHERE id = ?2",
            params![session_id, id],
        )?;
        Ok(())
    }

    fn store_worktree_path(&self, id: i64, path: &Path) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE jobs SET worktree_path = ?1, updated_at = datetime('now')
             WHERE id = ?2",
            params![path.display().to_string(), id],
        )?;
        Ok(())
    }

    fn requeue_stale(&self, id: i64) -> Result<()> {
        let conn = self.lock_conn();
        let rows = conn.execute(
            "UPDATE jobs SET status = 'queued', leased_at = NULL, lease_expires = NULL,
                    retry_count = retry_count + 1, updated_at = datetime('now')
             WHERE id = ?1 AND status = 'leased'",
            params![id],
        )?;
        if rows == 0 {
            return Err(ReviewqError::Database(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    fn is_pr_reviewed(&self, repo: &RepoId, pr_number: u64, agent: &AgentKind) -> Result<bool> {
        let conn = self.lock_conn();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM jobs
                WHERE repo_owner = ?1 AND repo_name = ?2
                  AND pr_number = ?3 AND agent_kind = ?4
                  AND status NOT IN ('failed', 'canceled')
            )",
            params![repo.owner, repo.name, pr_number as i64, agent.as_db_str(),],
            |row| row.get(0),
        )?;
        Ok(exists)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, NewJob, RepoId};

    fn test_db() -> Database {
        Database::open_in_memory().expect("in-memory DB should open")
    }

    fn sample_job() -> NewJob {
        NewJob {
            repo: RepoId::new("owner", "repo"),
            pr_number: 42,
            head_sha: "abc123".into(),
            agent_kind: AgentKind::Claude,
            command: Some("echo review".into()),
            prompt_template: None,
            max_retries: 3,
        }
    }

    #[test]
    fn enqueue_and_retrieve() {
        let db = test_db();
        let job = db.enqueue(sample_job()).expect("enqueue should succeed");

        assert_eq!(job.pr_number, 42);
        assert_eq!(job.head_sha, "abc123");
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.agent_kind, AgentKind::Claude);
    }

    #[test]
    fn idempotency_check() {
        let db = test_db();
        let new_job = sample_job();
        let key = IdempotencyKey {
            repo: new_job.repo.clone(),
            pr_number: new_job.pr_number,
            head_sha: new_job.head_sha.clone(),
            agent_kind: new_job.agent_kind.clone(),
        };

        assert!(!db.is_processed(&key).expect("check should succeed"));
        db.enqueue(new_job).expect("enqueue should succeed");
        assert!(db.is_processed(&key).expect("check should succeed"));
    }

    #[test]
    fn lease_next_fifo() {
        let db = test_db();

        // Enqueue two jobs
        let mut job1 = sample_job();
        job1.head_sha = "sha1".into();
        let mut job2 = sample_job();
        job2.head_sha = "sha2".into();

        db.enqueue(job1).expect("enqueue 1");
        db.enqueue(job2).expect("enqueue 2");

        // First lease should get job1 (FIFO)
        let leased = db
            .lease_next()
            .expect("lease should succeed")
            .expect("should have a job");
        assert_eq!(leased.head_sha, "sha1");
        assert_eq!(leased.status, JobStatus::Leased);
        assert!(leased.leased_at.is_some());
        assert!(leased.lease_expires.is_some());

        // Second lease should get job2
        let leased2 = db
            .lease_next()
            .expect("lease should succeed")
            .expect("should have a job");
        assert_eq!(leased2.head_sha, "sha2");

        // No more jobs to lease
        assert!(db.lease_next().expect("lease should succeed").is_none());
    }

    #[test]
    fn complete_job() {
        let db = test_db();
        let job = db.enqueue(sample_job()).expect("enqueue");
        let leased = db.lease_next().expect("lease").expect("has job");
        db.mark_running(leased.id, 1234).expect("mark running");
        db.complete(job.id, JobStatus::Succeeded, Some(0))
            .expect("complete");

        let jobs = db
            .list_jobs(&JobFilter {
                status: Some(JobStatus::Succeeded),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].exit_code, Some(0));
    }

    #[test]
    fn cancel_job() {
        let db = test_db();
        let job = db.enqueue(sample_job()).expect("enqueue");
        db.cancel(job.id).expect("cancel");

        let jobs = db
            .list_jobs(&JobFilter {
                status: Some(JobStatus::Canceled),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn list_jobs_with_filter() {
        let db = test_db();

        let mut j1 = sample_job();
        j1.head_sha = "sha_a".into();
        let mut j2 = sample_job();
        j2.head_sha = "sha_b".into();

        db.enqueue(j1).expect("enqueue 1");
        let job2 = db.enqueue(j2).expect("enqueue 2");
        db.cancel(job2.id).expect("cancel");

        // List only queued
        let queued = db
            .list_jobs(&JobFilter {
                status: Some(JobStatus::Queued),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(queued.len(), 1);

        // List all
        let all = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn store_review_output() {
        let db = test_db();
        let job = db.enqueue(sample_job()).expect("enqueue");
        db.store_review_output(job.id, "# Review\nLGTM")
            .expect("store");

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs[0].review_output.as_deref(), Some("# Review\nLGTM"));
    }

    #[test]
    fn store_session_id_roundtrip() {
        let db = test_db();
        let job = db.enqueue(sample_job()).expect("enqueue");
        assert!(job.session_id.is_none());

        db.store_session_id(job.id, "sess-abc-123")
            .expect("store session_id");

        let jobs = db.list_jobs(&JobFilter::default()).expect("list");
        assert_eq!(jobs[0].session_id.as_deref(), Some("sess-abc-123"));
    }

    #[test]
    fn enqueue_with_prompt_template() {
        let db = test_db();
        let mut job = sample_job();
        job.prompt_template = Some("Review {pr_url} for {repo}".into());
        let stored = db.enqueue(job).expect("enqueue should succeed");
        assert_eq!(
            stored.prompt_template.as_deref(),
            Some("Review {pr_url} for {repo}")
        );
    }

    #[test]
    fn migration_idempotency() {
        // Opening the database twice should not fail — the migration is idempotent.
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("test.db");
        let _db1 = Database::open(&db_path).expect("first open");
        drop(_db1);
        let _db2 = Database::open(&db_path).expect("second open should not fail");
    }

    #[test]
    fn duplicate_enqueue_rejected() {
        let db = test_db();
        db.enqueue(sample_job()).expect("first enqueue");
        let result = db.enqueue(sample_job());
        assert!(
            result.is_err(),
            "duplicate should be rejected by UNIQUE constraint"
        );
    }

    #[test]
    fn is_pr_reviewed_false_when_no_jobs() {
        let db = test_db();
        let repo = RepoId::new("owner", "repo");
        assert!(
            !db.is_pr_reviewed(&repo, 42, &AgentKind::Claude)
                .expect("should succeed")
        );
    }

    #[test]
    fn is_pr_reviewed_true_for_succeeded_job() {
        let db = test_db();
        let repo = RepoId::new("owner", "repo");
        let job = db.enqueue(sample_job()).expect("enqueue");
        db.complete(job.id, JobStatus::Succeeded, Some(0))
            .expect("complete");
        assert!(
            db.is_pr_reviewed(&repo, 42, &AgentKind::Claude)
                .expect("should succeed")
        );
    }

    #[test]
    fn is_pr_reviewed_true_for_queued_job() {
        let db = test_db();
        let repo = RepoId::new("owner", "repo");
        db.enqueue(sample_job()).expect("enqueue");
        // Queued jobs should also block (review in progress).
        assert!(
            db.is_pr_reviewed(&repo, 42, &AgentKind::Claude)
                .expect("should succeed")
        );
    }

    #[test]
    fn is_pr_reviewed_false_for_failed_job() {
        let db = test_db();
        let repo = RepoId::new("owner", "repo");
        let job = db.enqueue(sample_job()).expect("enqueue");
        db.complete(job.id, JobStatus::Failed, Some(1))
            .expect("complete");
        assert!(
            !db.is_pr_reviewed(&repo, 42, &AgentKind::Claude)
                .expect("should succeed")
        );
    }

    #[test]
    fn is_pr_reviewed_false_for_canceled_job() {
        let db = test_db();
        let repo = RepoId::new("owner", "repo");
        let job = db.enqueue(sample_job()).expect("enqueue");
        db.cancel(job.id).expect("cancel");
        assert!(
            !db.is_pr_reviewed(&repo, 42, &AgentKind::Claude)
                .expect("should succeed")
        );
    }

    #[test]
    fn is_pr_reviewed_true_across_different_shas() {
        let db = test_db();
        let repo = RepoId::new("owner", "repo");
        let mut job = sample_job();
        job.head_sha = "sha_old".into();
        let stored = db.enqueue(job).expect("enqueue");
        db.complete(stored.id, JobStatus::Succeeded, Some(0))
            .expect("complete");

        // Different SHA should still be considered reviewed (PR-level check).
        assert!(
            db.is_pr_reviewed(&repo, 42, &AgentKind::Claude)
                .expect("should succeed")
        );
    }
}
