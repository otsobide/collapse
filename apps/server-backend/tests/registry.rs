//! Unit tests for the job registry.
//!
//! Two halves: the behaviour the handlers and the worker rely on (unchanged
//! from when this was a `HashMap`, and exercised against an in-memory
//! database), and the durability that replacing it bought, which needs a real
//! file and a second `open` standing in for a restart.

use collapse_server_backend::models::{Envelope, Job, JobStatus};
use collapse_server_backend::registry::{now_unix, Registry, DATABASE_FILE, SCHEMA_VERSION};
use collapse_core::Algorithm;
use tempfile::TempDir;

fn job(id: &str) -> Job {
    Job::new(
        id.to_string(),
        "notes.txt".to_string(),
        Algorithm::Zip,
        3,
        Envelope::None,
    )
}

/// Behaviour tests run against a database that never touches disk.
fn registry() -> Registry {
    Registry::in_memory().expect("an in-memory registry opens")
}

/// Durability tests need a real file, and a second `open` on the same
/// directory is what a restart looks like from here.
fn reopen(dir: &TempDir) -> Registry {
    Registry::open(dir.path()).expect("the registry opens")
}

// -------------------------------------------------------------- add and get --

#[test]
fn add_then_get_returns_the_job() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();

    let stored = registry.get("j1").unwrap().expect("job should be stored");
    assert_eq!(stored.job_id, "j1");
    assert_eq!(stored.status, JobStatus::Queued);
}

#[test]
fn get_returns_none_for_an_unknown_job() {
    assert!(registry().get("ghost").unwrap().is_none());
}

#[test]
fn every_field_survives_the_round_trip() {
    // The row is the wire contract's storage: anything lost here is lost from
    // the status endpoint too.
    let registry = registry();
    let mut original = Job::new(
        "j1".to_string(),
        "photos".to_string(),
        Algorithm::SevenZ,
        5,
        Envelope::Tar,
    );
    original.status = JobStatus::Failed;
    original.error_message = Some("boom".to_string());
    registry.add(&original).unwrap();

    let stored = registry.get("j1").unwrap().unwrap();
    assert_eq!(stored.job_id, original.job_id);
    assert_eq!(stored.name, original.name);
    assert_eq!(stored.archive_name, original.archive_name);
    assert_eq!(stored.algorithm, original.algorithm);
    assert_eq!(stored.level, original.level);
    assert_eq!(stored.envelope, original.envelope);
    assert_eq!(stored.status, original.status);
    assert_eq!(stored.error_message, original.error_message);
}

#[test]
fn get_returns_a_snapshot_not_a_handle() {
    // Handlers mutate what `get` hands them (setting error messages, etc.);
    // that must not reach the registry.
    let registry = registry();
    registry.add(&job("j1")).unwrap();

    let mut copy = registry.get("j1").unwrap().unwrap();
    copy.status = JobStatus::Completed;

    assert_eq!(
        registry.get("j1").unwrap().unwrap().status,
        JobStatus::Queued
    );
}

#[test]
fn jobs_are_keyed_independently() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();
    registry.add(&job("j2")).unwrap();

    registry
        .update_status("j1", JobStatus::Completed, None)
        .unwrap();

    assert_eq!(
        registry.get("j1").unwrap().unwrap().status,
        JobStatus::Completed
    );
    assert_eq!(
        registry.get("j2").unwrap().unwrap().status,
        JobStatus::Queued
    );
}

#[test]
fn adding_the_same_id_twice_replaces_the_job() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();
    registry
        .update_status("j1", JobStatus::Completed, None)
        .unwrap();
    registry.add(&job("j1")).unwrap();

    assert_eq!(
        registry.get("j1").unwrap().unwrap().status,
        JobStatus::Queued
    );
}

// ------------------------------------------------------------ status updates --

#[test]
fn update_status_advances_the_lifecycle() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();

    registry
        .update_status("j1", JobStatus::Compressing, None)
        .unwrap();
    assert_eq!(
        registry.get("j1").unwrap().unwrap().status,
        JobStatus::Compressing
    );

    registry
        .update_status("j1", JobStatus::Completed, None)
        .unwrap();
    assert_eq!(
        registry.get("j1").unwrap().unwrap().status,
        JobStatus::Completed
    );
}

#[test]
fn update_status_records_the_failure_message() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();

    registry
        .update_status("j1", JobStatus::Failed, Some("boom".to_string()))
        .unwrap();

    let stored = registry.get("j1").unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(stored.error_message.as_deref(), Some("boom"));
}

#[test]
fn update_status_clears_a_previous_message() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();
    registry
        .update_status("j1", JobStatus::Failed, Some("boom".to_string()))
        .unwrap();

    registry
        .update_status("j1", JobStatus::Completed, None)
        .unwrap();

    assert!(registry
        .get("j1")
        .unwrap()
        .unwrap()
        .error_message
        .is_none());
}

/// The worker updates a job that a concurrent DELETE may already have
/// removed; that must be a no-op, not a resurrection.
#[test]
fn update_status_on_an_unknown_job_does_nothing() {
    let registry = registry();
    registry
        .update_status("ghost", JobStatus::Completed, None)
        .unwrap();
    assert!(registry.get("ghost").unwrap().is_none());
}

// ----------------------------------------------------------------- removal --

#[test]
fn forget_drops_the_job_and_says_it_did() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();

    assert!(registry.forget("j1").unwrap(), "there was one to drop");
    assert!(registry.get("j1").unwrap().is_none());
}

#[test]
fn forget_says_so_when_there_was_nothing_to_drop() {
    assert!(!registry().forget("ghost").unwrap());
}

// ------------------------------------------------------------- listing jobs --

#[test]
fn ids_lists_every_job() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();
    registry.add(&job("j2")).unwrap();

    let mut ids = registry.ids().unwrap();
    ids.sort();
    assert_eq!(ids, vec!["j1".to_string(), "j2".to_string()]);
}

#[test]
fn unfinished_lists_only_the_jobs_a_worker_still_owns() {
    let registry = registry();
    for id in ["queued", "compressing", "completed", "failed"] {
        registry.add(&job(id)).unwrap();
    }
    registry
        .update_status("compressing", JobStatus::Compressing, None)
        .unwrap();
    registry
        .update_status("completed", JobStatus::Completed, None)
        .unwrap();
    registry
        .update_status("failed", JobStatus::Failed, Some("boom".into()))
        .unwrap();

    let mut unfinished = registry.unfinished().unwrap();
    unfinished.sort();
    assert_eq!(
        unfinished,
        vec!["compressing".to_string(), "queued".to_string()]
    );
}

// ---------------------------------------------------------------- durability --

#[test]
fn a_job_survives_reopening_the_registry() {
    // The whole point of the database: this is what a restart does to a job
    // that nobody has deleted yet.
    let dir = TempDir::new().unwrap();
    {
        let registry = reopen(&dir);
        registry.add(&job("j1")).unwrap();
        registry
            .update_status("j1", JobStatus::Completed, None)
            .unwrap();
    }

    let stored = reopen(&dir)
        .get("j1")
        .unwrap()
        .expect("the job outlives the process that created it");
    assert_eq!(stored.status, JobStatus::Completed);
    assert_eq!(stored.archive_name, "notes.txt.zip");
}

#[test]
fn a_failure_message_survives_reopening() {
    let dir = TempDir::new().unwrap();
    {
        let registry = reopen(&dir);
        registry.add(&job("j1")).unwrap();
        registry
            .update_status("j1", JobStatus::Failed, Some("disk full".into()))
            .unwrap();
    }

    let stored = reopen(&dir).get("j1").unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(stored.error_message.as_deref(), Some("disk full"));
}

#[test]
fn a_removed_job_stays_removed() {
    let dir = TempDir::new().unwrap();
    {
        let registry = reopen(&dir);
        registry.add(&job("j1")).unwrap();
        registry.forget("j1").unwrap();
    }

    assert!(reopen(&dir).get("j1").unwrap().is_none());
}

#[test]
fn the_database_is_a_file_in_the_staging_directory() {
    // Its shape matters to the reconciliation, which walks that directory and
    // treats every *directory* as a job: the database must never look like
    // one.
    let dir = TempDir::new().unwrap();
    let registry = reopen(&dir);
    registry.add(&job("j1")).unwrap();

    let database = dir.path().join(DATABASE_FILE);
    assert!(database.is_file(), "the registry is stored in a file");
}

#[test]
fn reopening_keeps_the_schema_version() {
    let dir = TempDir::new().unwrap();
    drop(reopen(&dir));
    drop(reopen(&dir));

    let connection = rusqlite::Connection::open(dir.path().join(DATABASE_FILE)).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
}

// ------------------------------------------------------------- concurrency --

/// The handlers and the worker hit the registry from different threads at the
/// same time, and they do it inline rather than through a thread pool. A
/// database behind a mutex has to survive that without deadlocking or losing a
/// write, which is the assumption that made "no spawn_blocking" acceptable.
#[test]
fn concurrent_writers_and_readers_all_land() {
    use std::sync::Arc;

    let dir = TempDir::new().unwrap();
    let registry = Arc::new(reopen(&dir));

    let writers: Vec<_> = (0..8)
        .map(|worker| {
            let registry = registry.clone();
            std::thread::spawn(move || {
                for n in 0..25 {
                    let id = format!("job-{worker}-{n}");
                    registry.add(&job(&id)).unwrap();
                    registry
                        .update_status(&id, JobStatus::Completed, None)
                        .unwrap();
                    // A reader racing the writers, the way a polling client does.
                    assert_eq!(
                        registry.get(&id).unwrap().unwrap().status,
                        JobStatus::Completed
                    );
                }
            })
        })
        .collect();

    for writer in writers {
        writer.join().expect("no writer panicked or deadlocked");
    }

    assert_eq!(registry.ids().unwrap().len(), 8 * 25, "every write landed");
    assert!(
        registry.unfinished().unwrap().is_empty(),
        "and every one of them finished"
    );
}

// ------------------------------------------- rows this build cannot read --

/// Write a row straight into the database, bypassing the registry, the way a
/// newer version of the server or a hand edit would.
fn plant_row(dir: &TempDir, job_id: &str, algorithm: &str, server_version: Option<&str>) {
    let connection =
        rusqlite::Connection::open(dir.path().join(DATABASE_FILE)).expect("the database opens");
    connection
        .execute(
            "INSERT OR REPLACE INTO jobs
                 (job_id, name, archive_name, algorithm, level, envelope, status,
                  error_message, created_at, updated_at, server_version)
             VALUES (?1, 'notes.txt', 'notes.txt.zip', ?2, 3, 'none', 'completed',
                     NULL, 0, 0, ?3)",
            rusqlite::params![job_id, algorithm, server_version],
        )
        .expect("the row is written");
}

/// A value from a version that knows a format this one does not. The schema is
/// unchanged (an algorithm is not a column), so no version gate can catch it:
/// it surfaces here, on the read.
#[test]
fn a_row_this_build_cannot_read_says_who_wrote_it_and_why() {
    let dir = TempDir::new().unwrap();
    drop(reopen(&dir));
    plant_row(&dir, "from-the-future", "zstd", Some("0.9.0"));

    let error = reopen(&dir)
        .get("from-the-future")
        .expect_err("a format this build does not know is not readable");

    let message = error.to_string();
    assert!(message.contains("0.9.0"), "names the build that wrote it: {message}");
    assert!(message.contains("zstd"), "names the value: {message}");
    assert!(message.contains("algorithm"), "names the field: {message}");
    assert!(
        !message.contains("column"),
        "and not the database's own words: {message}"
    );
}

#[test]
fn a_row_with_no_recorded_version_still_explains_itself() {
    let dir = TempDir::new().unwrap();
    drop(reopen(&dir));
    plant_row(&dir, "anonymous", "zstd", None);

    let message = reopen(&dir).get("anonymous").unwrap_err().to_string();
    assert!(message.contains("a different version"), "got {message}");
    assert!(message.contains("zstd"), "got {message}");
}

/// The point of not reading a row in order to delete it: one row nobody can
/// parse must not be able to stop the registry being cleaned up.
#[test]
fn a_row_that_cannot_be_read_can_still_be_forgotten() {
    let dir = TempDir::new().unwrap();
    drop(reopen(&dir));
    plant_row(&dir, "unreadable", "zstd", Some("0.9.0"));

    let registry = reopen(&dir);
    assert!(registry.forget("unreadable").unwrap());
    assert!(registry.ids().unwrap().is_empty());
}

/// And it does not hide the jobs around it from the listings maintenance uses.
#[test]
fn an_unreadable_row_does_not_hide_the_jobs_beside_it() {
    let dir = TempDir::new().unwrap();
    let registry = reopen(&dir);
    registry.add(&job("healthy")).unwrap();
    // Terminal, so the reaper's query is entitled to return it.
    registry
        .update_status("healthy", JobStatus::Completed, None)
        .unwrap();
    drop(registry);
    plant_row(&dir, "unreadable", "zstd", Some("0.9.0"));

    let registry = reopen(&dir);
    let mut ids = registry.ids().unwrap();
    ids.sort();
    assert_eq!(ids, vec!["healthy".to_string(), "unreadable".to_string()]);

    // Both come back: the listings maintenance runs on never interpret a row,
    // so an unreadable one cannot hide its neighbours from the reaper.
    let mut expired = registry.expired(now_unix() + 3600).unwrap();
    expired.sort();
    assert_eq!(expired, vec!["healthy".to_string(), "unreadable".to_string()]);
}

// ------------------------------------------------------------- migrations --

#[test]
fn a_registry_from_a_newer_schema_is_refused() {
    // The other half of the same problem, caught where it can still be acted
    // on: at startup, with a message saying what to do.
    let dir = TempDir::new().unwrap();
    drop(reopen(&dir));
    rusqlite::Connection::open(dir.path().join(DATABASE_FILE))
        .unwrap()
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();

    let message = match Registry::open(dir.path()) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a registry from a newer schema must not open"),
    };
    assert!(message.contains("newer Collapse"), "got {message}");
    assert!(
        message.contains(&(SCHEMA_VERSION + 1).to_string()),
        "names what it found: {message}"
    );
}

#[test]
fn a_registry_from_the_previous_schema_is_migrated() {
    // Version 1 had no server_version column. Opening it must add the column
    // and keep the rows, not start over.
    let dir = TempDir::new().unwrap();
    {
        let connection = rusqlite::Connection::open(dir.path().join(DATABASE_FILE)).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE jobs (
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
                 INSERT INTO jobs VALUES
                     ('old', 'notes.txt', 'notes.txt.zip', 'zip', 3, 'none',
                      'completed', NULL, 0, 0);",
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
    }

    let registry = Registry::open(dir.path()).expect("the older schema is migrated, not refused");

    let job = registry.get("old").unwrap().expect("the job survives");
    assert_eq!(job.archive_name, "notes.txt.zip");
    assert_eq!(
        registry.get("old").unwrap().unwrap().status,
        JobStatus::Completed
    );

    let version: i64 = rusqlite::Connection::open(dir.path().join(DATABASE_FILE))
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
}

#[test]
fn a_new_row_records_the_build_that_wrote_it() {
    let dir = TempDir::new().unwrap();
    let registry = reopen(&dir);
    registry.add(&job("j1")).unwrap();

    let written: Option<String> = rusqlite::Connection::open(dir.path().join(DATABASE_FILE))
        .unwrap()
        .query_row(
            "SELECT server_version FROM jobs WHERE job_id = 'j1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(written.as_deref(), Some(env!("CARGO_PKG_VERSION")));
}
