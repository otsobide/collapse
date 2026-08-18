//! The job registry, kept in a SQLite database beside the staged files.
//!
//! It is the server's only mutable state, and it outlives the process: a
//! restart finds the jobs it left behind, which is what lets a client keep
//! polling (and, above all, keep deleting) across one. Before this it lived in
//! a `HashMap`, so a restart forgot every job while its files stayed on the
//! volume, unreachable and unremovable through the API.
//!
//! Reads and writes are small, indexed and single-writer, so they run inline
//! rather than through `spawn_blocking`: SQLite answers them from the page
//! cache in microseconds, and the alternative costs a thread hop on every
//! poll.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};

use collapse_core::Algorithm;

use crate::models::{Envelope, Job, JobStatus};

/// Stamped in `PRAGMA user_version`. Bump it when the schema changes, and add
/// the migration that takes the previous version there.
pub const SCHEMA_VERSION: i64 = 1;

/// File name of the database inside the staging directory. It sits beside the
/// per-job directories, and is a file rather than a directory so anything
/// walking the staging area can tell the two apart.
pub const DATABASE_FILE: &str = "jobs.db";

/// Jobs, and nothing else. Errors are returned rather than swallowed: a
/// registry that quietly forgets a write would hand out 202s for jobs that do
/// not exist.
pub struct Registry {
    connection: Mutex<Connection>,
}

impl Registry {
    /// Open (creating it if needed) the registry inside a staging directory.
    pub fn open(storage_dir: &Path) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open(storage_dir.join(DATABASE_FILE))?)
    }

    /// A registry that lives only as long as the process, which is what the
    /// tests use. A server started with one behaves the way this server did
    /// before the database existed: it forgets everything when it stops.
    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> rusqlite::Result<Self> {
        // WAL keeps a reader (a client polling) from blocking the writer (the
        // worker); NORMAL is the usual companion to it, trading a crash-window
        // for not fsyncing on every status change. An in-memory database
        // ignores both.
        connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                 job_id        TEXT PRIMARY KEY,
                 name          TEXT NOT NULL,
                 archive_name  TEXT NOT NULL,
                 algorithm     TEXT NOT NULL,
                 level         INTEGER NOT NULL,
                 envelope      TEXT NOT NULL,
                 status        TEXT NOT NULL,
                 error_message TEXT,
                 created_at    INTEGER NOT NULL,
                 updated_at    INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS jobs_status ON jobs (status);",
        )?;
        connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Record a job. Replaces any job already under that id, the way the map
    /// it replaces did.
    pub fn add(&self, job: &Job) -> rusqlite::Result<()> {
        let now = now();
        self.connection.lock().unwrap().execute(
            "INSERT OR REPLACE INTO jobs
                 (job_id, name, archive_name, algorithm, level, envelope,
                  status, error_message, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                job.job_id,
                job.name,
                job.archive_name,
                job.algorithm.to_string(),
                job.level,
                job.envelope.to_string(),
                job.status.to_string(),
                job.error_message,
                now,
            ],
        )?;
        Ok(())
    }

    /// A snapshot of one job, or `None` if there is no such job. Callers
    /// mutate what they are handed, so this is deliberately a copy.
    pub fn get(&self, job_id: &str) -> rusqlite::Result<Option<Job>> {
        self.connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT job_id, name, archive_name, algorithm, level, envelope,
                        status, error_message
                 FROM jobs WHERE job_id = ?1",
                params![job_id],
                job_from_row,
            )
            .optional()
    }

    /// Move a job along its lifecycle. Unknown ids are a no-op: the worker
    /// updates jobs a concurrent DELETE may already have removed, and that
    /// must not resurrect them.
    pub fn update_status(
        &self,
        job_id: &str,
        status: JobStatus,
        error_message: Option<String>,
    ) -> rusqlite::Result<()> {
        self.connection.lock().unwrap().execute(
            "UPDATE jobs SET status = ?2, error_message = ?3, updated_at = ?4
             WHERE job_id = ?1",
            params![job_id, status.to_string(), error_message, now()],
        )?;
        Ok(())
    }

    /// Forget a job, handing back what was removed so the caller can report
    /// it. `None` means there was nothing to remove.
    pub fn remove(&self, job_id: &str) -> rusqlite::Result<Option<Job>> {
        let connection = self.connection.lock().unwrap();
        let job = connection
            .query_row(
                "SELECT job_id, name, archive_name, algorithm, level, envelope,
                        status, error_message
                 FROM jobs WHERE job_id = ?1",
                params![job_id],
                job_from_row,
            )
            .optional()?;

        if job.is_some() {
            connection.execute("DELETE FROM jobs WHERE job_id = ?1", params![job_id])?;
        }
        Ok(job)
    }

    /// Every job id the registry knows, for reconciling against what is on
    /// disk. The set is small (jobs are deleted as they are collected), so it
    /// is read whole rather than paged.
    pub fn ids(&self) -> rusqlite::Result<Vec<String>> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection.prepare("SELECT job_id FROM jobs")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Finished jobs untouched since `deadline` (unix seconds).
    ///
    /// Only terminal ones: a queued or compressing job belongs to the worker,
    /// however old it looks, and reaping it under the worker would leave the
    /// compression writing into a directory that no longer exists.
    pub fn expired(&self, deadline: i64) -> rusqlite::Result<Vec<String>> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection.prepare(
            "SELECT job_id FROM jobs
             WHERE status IN ('completed', 'failed') AND updated_at < ?1",
        )?;
        let ids = statement
            .query_map(params![deadline], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Mark a job as still wanted, so the reaper's clock starts again.
    ///
    /// Downloading is what counts as wanted: polling is not, deliberately. A
    /// client that polls a finished job forever without ever fetching it has
    /// abandoned it in every sense that matters to disk.
    pub fn touch(&self, job_id: &str) -> rusqlite::Result<()> {
        self.connection.lock().unwrap().execute(
            "UPDATE jobs SET updated_at = ?2 WHERE job_id = ?1",
            params![job_id, now()],
        )?;
        Ok(())
    }

    /// The jobs no worker is working on any more, which after a restart means
    /// the ones that were interrupted by it.
    pub fn unfinished(&self) -> rusqlite::Result<Vec<String>> {
        let connection = self.connection.lock().unwrap();
        let mut statement =
            connection.prepare("SELECT job_id FROM jobs WHERE status IN ('queued', 'compressing')")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }
}

/// Unix seconds. Only ever compared against other rows, so a clock that jumps
/// costs accuracy in reports, not correctness of the flow.
pub fn now_unix() -> i64 {
    now()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}

/// Rebuild a job from its row.
///
/// The three enums are stored as the strings they travel as, so the database
/// is readable with any SQLite client. A value that does not parse means the
/// file was written by something else, which is reported rather than guessed
/// at.
fn job_from_row(row: &Row<'_>) -> rusqlite::Result<Job> {
    let parse = |column: usize, text: String, kind: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            format!("unknown {kind}: {text}").into(),
        )
    };

    let algorithm: String = row.get(3)?;
    let envelope: String = row.get(5)?;
    let status: String = row.get(6)?;

    Ok(Job {
        job_id: row.get(0)?,
        name: row.get(1)?,
        archive_name: row.get(2)?,
        algorithm: algorithm
            .parse::<Algorithm>()
            .map_err(|_| parse(3, algorithm, "algorithm"))?,
        level: row.get(4)?,
        envelope: envelope
            .parse::<Envelope>()
            .map_err(|_| parse(5, envelope, "envelope"))?,
        status: status
            .parse::<JobStatus>()
            .map_err(|_| parse(6, status, "status"))?,
        error_message: row.get(7)?,
    })
}
