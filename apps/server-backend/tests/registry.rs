//! Unit tests for the job registry.
//!
//! Two halves: the behaviour the handlers and the worker rely on (unchanged
//! from when this was a `HashMap`, and exercised against an in-memory
//! database), and the durability that replacing it bought, which needs a real
//! file and a second `open` standing in for a restart.

use collapse_server_backend::models::{Envelope, Job, JobStatus};
use collapse_server_backend::registry::{Registry, DATABASE_FILE, SCHEMA_VERSION};
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
fn remove_returns_the_job_and_forgets_it() {
    let registry = registry();
    registry.add(&job("j1")).unwrap();

    let removed = registry
        .remove("j1")
        .unwrap()
        .expect("removed job is returned");
    assert_eq!(removed.job_id, "j1");
    assert!(registry.get("j1").unwrap().is_none());
}

#[test]
fn remove_returns_none_for_an_unknown_job() {
    assert!(registry().remove("ghost").unwrap().is_none());
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
        registry.remove("j1").unwrap();
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
