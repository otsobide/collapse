//! Unit tests for the job registry.
//!
//! Two halves: the behaviour the handlers and the worker rely on (unchanged
//! from when this was a `HashMap`, and exercised against an in-memory
//! database), and the durability that replacing it bought, which needs a real
//! file and a second `open` standing in for a restart.

use collapse_core::Algorithm;
use collapse_server_backend::models::{Envelope, Job, JobStatus, Verify};
use collapse_server_backend::registry::{now_unix, Registry, DATABASE_FILE, SCHEMA_VERSION};
use tempfile::TempDir;

fn job(id: &str) -> Job {
    Job::new(
        id.to_string(),
        "notes.txt".to_string(),
        Algorithm::Zip,
        3,
        Envelope::None,
        Verify::Index,
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
        Verify::Contents,
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
    // The worker is handed a job id and nothing else, so this column is the
    // only way the depth a client asked for reaches the compression. Drop it
    // from the INSERT or the SELECT and every job silently checks at the floor.
    assert_eq!(stored.verify, original.verify);
    assert_eq!(stored.verify, Verify::Contents);
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

    assert!(registry.get("j1").unwrap().unwrap().error_message.is_none());
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

/// The depth is chosen once, on the upload, and read back by a worker that may
/// belong to a different process: a queued job whose server was restarted is
/// the case where nothing but the row remembers what the client asked for.
#[test]
fn the_verification_depth_survives_reopening() {
    let dir = TempDir::new().unwrap();
    {
        let registry = reopen(&dir);
        let mut deep = job("j1");
        deep.verify = Verify::Contents;
        registry.add(&deep).unwrap();
    }

    assert_eq!(
        reopen(&dir).get("j1").unwrap().unwrap().verify,
        Verify::Contents
    );
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
    assert!(
        message.contains("0.9.0"),
        "names the build that wrote it: {message}"
    );
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
    assert_eq!(
        expired,
        vec!["healthy".to_string(), "unreadable".to_string()]
    );
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

/// Read `PRAGMA user_version` off the file, the way the next process would.
fn stamped_version(dir: &TempDir) -> i64 {
    rusqlite::Connection::open(dir.path().join(DATABASE_FILE))
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

/// Build a database at an older schema by hand, the way an older build left
/// it: `columns` is the table as that version spelled it, `values` one row.
fn plant_schema(dir: &TempDir, version: i64, columns: &str, values: &str) {
    let connection = rusqlite::Connection::open(dir.path().join(DATABASE_FILE)).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TABLE jobs ({columns});
             INSERT INTO jobs VALUES ({values});"
        ))
        .unwrap();
    connection
        .pragma_update(None, "user_version", version)
        .unwrap();
}

/// Version 2's table: `server_version`, but no `verify`.
const SCHEMA_2_COLUMNS: &str = "job_id         TEXT PRIMARY KEY,
     name           TEXT NOT NULL,
     archive_name   TEXT NOT NULL,
     algorithm      TEXT NOT NULL,
     level          INTEGER NOT NULL,
     envelope       TEXT NOT NULL,
     status         TEXT NOT NULL,
     error_message  TEXT,
     created_at     INTEGER NOT NULL,
     updated_at     INTEGER NOT NULL,
     server_version TEXT";

#[test]
fn a_registry_from_the_previous_schema_is_migrated() {
    // Version 2 had no `verify` column, so a store written by the build before
    // this one has to gain it and keep its rows, not start over: a server that
    // dropped a released version's jobs on upgrade would lose archives clients
    // had already been promised.
    let dir = TempDir::new().unwrap();
    plant_schema(
        &dir,
        2,
        SCHEMA_2_COLUMNS,
        "'old', 'notes.txt', 'notes.txt.zip', 'zip', 3, 'none',
         'completed', NULL, 0, 0, '0.7.0'",
    );

    let registry = Registry::open(dir.path()).expect("the older schema is migrated, not refused");

    let job = registry.get("old").unwrap().expect("the job survives");
    assert_eq!(job.archive_name, "notes.txt.zip");
    assert_eq!(job.status, JobStatus::Completed);
    // The column's default. Not a claim about what that build did (it verified
    // nothing at all), just the floor: the value only matters to a job the
    // worker still has to run, and the startup reconciliation fails every one
    // of those, so no migrated row ever reaches the worker.
    assert_eq!(job.verify, Verify::Index);

    assert_eq!(stamped_version(&dir), SCHEMA_VERSION);
}

#[test]
fn a_registry_several_schemas_behind_is_walked_all_the_way_forward() {
    // Version 1 had neither `server_version` nor `verify`, so opening it has to
    // apply both steps in order. Written as `else if` the migration would take
    // one hop per open, leaving a database that is stamped current and missing
    // a column, which fails on the first read instead of at startup.
    let dir = TempDir::new().unwrap();
    plant_schema(
        &dir,
        1,
        "job_id        TEXT PRIMARY KEY,
         name          TEXT NOT NULL,
         archive_name  TEXT NOT NULL,
         algorithm     TEXT NOT NULL,
         level         INTEGER NOT NULL,
         envelope      TEXT NOT NULL,
         status        TEXT NOT NULL,
         error_message TEXT,
         created_at    INTEGER NOT NULL,
         updated_at    INTEGER NOT NULL",
        "'old', 'notes.txt', 'notes.txt.zip', 'zip', 3, 'none',
         'completed', NULL, 0, 0",
    );

    let registry = Registry::open(dir.path()).expect("two steps behind is still migrated");

    let stored = registry.get("old").unwrap().expect("the job survives");
    assert_eq!(stored.archive_name, "notes.txt.zip");
    assert_eq!(stored.status, JobStatus::Completed);
    assert_eq!(stored.verify, Verify::Index);
    assert_eq!(stamped_version(&dir), SCHEMA_VERSION);

    // And the row it wrote before the upgrade is not the only one that works:
    // the migrated table takes new jobs at the current schema too.
    let mut fresh = job("new");
    fresh.verify = Verify::Contents;
    registry.add(&fresh).unwrap();
    assert_eq!(
        registry.get("new").unwrap().unwrap().verify,
        Verify::Contents
    );
}

/// The mirror image, at the depth the schema gate cannot see: `verify` is not a
/// new column in a newer build, it is a new *value* in an existing one, so the
/// version is unchanged and the surprise only surfaces on the read.
#[test]
fn a_verification_depth_this_build_does_not_know_is_reported_not_guessed() {
    let dir = TempDir::new().unwrap();
    drop(reopen(&dir));
    rusqlite::Connection::open(dir.path().join(DATABASE_FILE))
        .unwrap()
        .execute(
            "INSERT INTO jobs
                 (job_id, name, archive_name, algorithm, level, envelope, verify,
                  status, error_message, created_at, updated_at, server_version)
             VALUES ('deep', 'notes.txt', 'notes.txt.zip', 'zip', 3, 'none',
                     'paranoid', 'completed', NULL, 0, 0, '0.9.0')",
            [],
        )
        .unwrap();

    let message = reopen(&dir).get("deep").unwrap_err().to_string();
    assert!(message.contains("verify"), "names the field: {message}");
    assert!(message.contains("paranoid"), "names the value: {message}");
    assert!(message.contains("0.9.0"), "names the build: {message}");
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
