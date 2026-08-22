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
pub const SCHEMA_VERSION: i64 = 2;

/// File name of the database inside the registry directory.
pub const DATABASE_FILE: &str = "jobs.db";

/// What can go wrong with the registry, beyond SQLite's own failures.
#[derive(Debug)]
pub enum RegistryError {
    Sql(rusqlite::Error),

    /// The database was written by a build that knows a schema this one does
    /// not. Refusing to open it is the point: a downgrade that carried on
    /// would read columns it does not understand and write rows the newer
    /// build would then have to make sense of.
    FromTheFuture {
        found: i64,
        understood: i64,
    },

    /// A row this build cannot make sense of. The usual cause is the mirror
    /// image of the above: the *schema* did not change (adding an algorithm
    /// does not add a column), so the version gate had nothing to catch, and
    /// the surprise only surfaces when the value is read back.
    ///
    /// It carries what an operator needs to know: which job, which field,
    /// what the value was, and which build wrote it.
    Unreadable {
        job_id: String,
        field: &'static str,
        value: String,
        written_by: Option<String>,
    },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Sql(err) => write!(f, "{err}"),
            RegistryError::FromTheFuture { found, understood } => write!(
                f,
                "This registry was written by a newer Collapse (schema {found}; this server \
                 understands {understood}). Downgrading is not supported: run the newer \
                 version, or start from an empty registry."
            ),
            RegistryError::Unreadable {
                field,
                value,
                written_by,
                ..
            } => {
                let origin = match written_by {
                    Some(version) => format!("Collapse {version}"),
                    None => "a different version of Collapse".to_string(),
                };
                write!(
                    f,
                    "This job was recorded by {origin} and this server ({}) cannot read it: \
                     unknown {field} {value:?}.",
                    env!("CARGO_PKG_VERSION")
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<rusqlite::Error> for RegistryError {
    fn from(err: rusqlite::Error) -> Self {
        RegistryError::Sql(err)
    }
}

/// The registry's own result type.
pub type Result<T> = std::result::Result<T, RegistryError>;

/// Jobs, and nothing else. Errors are returned rather than swallowed: a
/// registry that quietly forgets a write would hand out 202s for jobs that do
/// not exist.
pub struct Registry {
    connection: Mutex<Connection>,
}

impl Registry {
    /// Open (creating it if needed) the registry inside its directory.
    pub fn open(registry_dir: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(registry_dir.join(DATABASE_FILE))?)
    }

    /// A registry that lives only as long as the process, which is what the
    /// tests use. A server started with one behaves the way this server did
    /// before the database existed: it forgets everything when it stops.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        // WAL keeps a reader (a client polling) from blocking the writer (the
        // worker); NORMAL is the usual companion to it, trading a crash-window
        // for not fsyncing on every status change. An in-memory database
        // ignores both.
        connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        migrate(&connection)?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Record a job. Replaces any job already under that id, the way the map
    /// it replaces did.
    pub fn add(&self, job: &Job) -> Result<()> {
        let now = now();
        self.connection.lock().unwrap().execute(
            "INSERT OR REPLACE INTO jobs
                 (job_id, name, archive_name, algorithm, level, envelope,
                  status, error_message, created_at, updated_at, server_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)",
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
                env!("CARGO_PKG_VERSION"),
            ],
        )?;
        Ok(())
    }

    /// A snapshot of one job, or `None` if there is no such job. Callers
    /// mutate what they are handed, so this is deliberately a copy.
    pub fn get(&self, job_id: &str) -> Result<Option<Job>> {
        self.connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT job_id, name, archive_name, algorithm, level, envelope,
                        status, error_message, server_version
                 FROM jobs WHERE job_id = ?1",
                params![job_id],
                raw_from_row,
            )
            .optional()?
            .map(RawJob::into_job)
            .transpose()
    }

    /// Move a job along its lifecycle. Unknown ids are a no-op: the worker
    /// updates jobs a concurrent DELETE may already have removed, and that
    /// must not resurrect them.
    pub fn update_status(
        &self,
        job_id: &str,
        status: JobStatus,
        error_message: Option<String>,
    ) -> Result<()> {
        self.connection.lock().unwrap().execute(
            "UPDATE jobs SET status = ?2, error_message = ?3, updated_at = ?4
             WHERE job_id = ?1",
            params![job_id, status.to_string(), error_message, now()],
        )?;
        Ok(())
    }

    /// Drop a job from the registry. Returns whether there was one.
    ///
    /// Deliberately does **not** read the row first. Nothing that deletes a
    /// job needs to interpret it, and a row this build cannot parse (a value
    /// from a newer version, a hand-edited database) would otherwise be able
    /// to stop the reaper and the startup pass for every other job: one bad
    /// row and disk is never reclaimed again.
    pub fn forget(&self, job_id: &str) -> Result<bool> {
        let removed = self
            .connection
            .lock()
            .unwrap()
            .execute("DELETE FROM jobs WHERE job_id = ?1", params![job_id])?;
        Ok(removed > 0)
    }

    /// Every job id the registry knows, for reconciling against what is on
    /// disk. The set is small (jobs are deleted as they are collected), so it
    /// is read whole rather than paged.
    pub fn ids(&self) -> Result<Vec<String>> {
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
    pub fn expired(&self, deadline: i64) -> Result<Vec<String>> {
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
    pub fn touch(&self, job_id: &str) -> Result<()> {
        self.connection.lock().unwrap().execute(
            "UPDATE jobs SET updated_at = ?2 WHERE job_id = ?1",
            params![job_id, now()],
        )?;
        Ok(())
    }

    /// The jobs no worker is working on any more, which after a restart means
    /// the ones that were interrupted by it.
    pub fn unfinished(&self) -> Result<Vec<String>> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection
            .prepare("SELECT job_id FROM jobs WHERE status IN ('queued', 'compressing')")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }
}

/// Bring the database up to [`SCHEMA_VERSION`], or refuse to touch it.
///
/// `PRAGMA user_version` is 0 on a database that has never been written, which
/// is what tells a fresh file apart from one that needs a step applied.
fn migrate(connection: &Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if version > SCHEMA_VERSION {
        return Err(RegistryError::FromTheFuture {
            found: version,
            understood: SCHEMA_VERSION,
        });
    }

    if version == 0 {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                 job_id         TEXT PRIMARY KEY,
                 name           TEXT NOT NULL,
                 archive_name   TEXT NOT NULL,
                 algorithm      TEXT NOT NULL,
                 level          INTEGER NOT NULL,
                 envelope       TEXT NOT NULL,
                 status         TEXT NOT NULL,
                 error_message  TEXT,
                 created_at     INTEGER NOT NULL,
                 updated_at     INTEGER NOT NULL,
                 server_version TEXT
             );
             CREATE INDEX IF NOT EXISTS jobs_status ON jobs (status);",
        )?;
    } else if version < 2 {
        // 1 -> 2: rows record the build that wrote them, so a value this
        // build cannot read can say where it came from.
        connection.execute_batch("ALTER TABLE jobs ADD COLUMN server_version TEXT;")?;
    }

    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
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

/// A row exactly as it is stored, before anything is interpreted.
///
/// Reading and parsing are separate on purpose. SQLite hands rows back through
/// a closure that can only fail with its own error type, which is how the
/// parse failure used to end up as "Invalid column type Text at index 3": true,
/// unactionable, and hiding the one fact that helps (which build wrote it).
struct RawJob {
    job_id: String,
    name: String,
    archive_name: String,
    algorithm: String,
    level: u32,
    envelope: String,
    status: String,
    error_message: Option<String>,
    server_version: Option<String>,
}

impl RawJob {
    /// Interpret the row, or say precisely what could not be interpreted.
    ///
    /// The three enums are stored as the strings they travel as, so the
    /// database reads sensibly in any SQLite client. A value that does not
    /// parse is reported rather than guessed at: a client asking about its job
    /// deserves the truth or an error, never an invented format.
    fn into_job(self) -> Result<Job> {
        let unreadable = |field: &'static str, value: &str| RegistryError::Unreadable {
            job_id: self.job_id.clone(),
            field,
            value: value.to_string(),
            written_by: self.server_version.clone(),
        };

        let algorithm = self
            .algorithm
            .parse::<Algorithm>()
            .map_err(|_| unreadable("algorithm", &self.algorithm))?;
        let envelope = self
            .envelope
            .parse::<Envelope>()
            .map_err(|_| unreadable("envelope", &self.envelope))?;
        let status = self
            .status
            .parse::<JobStatus>()
            .map_err(|_| unreadable("status", &self.status))?;

        Ok(Job {
            job_id: self.job_id,
            name: self.name,
            archive_name: self.archive_name,
            algorithm,
            level: self.level,
            envelope,
            status,
            error_message: self.error_message,
        })
    }
}

fn raw_from_row(row: &Row<'_>) -> rusqlite::Result<RawJob> {
    Ok(RawJob {
        job_id: row.get(0)?,
        name: row.get(1)?,
        archive_name: row.get(2)?,
        algorithm: row.get(3)?,
        level: row.get(4)?,
        envelope: row.get(5)?,
        status: row.get(6)?,
        error_message: row.get(7)?,
        server_version: row.get(8)?,
    })
}
