//! Startup reconciliation: what happens to jobs and staged files that a
//! restart, or a crash, left disagreeing with each other.

use collapse_server_backend::maintenance::{reconcile, Reconciled, INTERRUPTED};
use collapse_server_backend::models::{Envelope, Job, JobStatus};
use collapse_server_backend::registry::{Registry, DATABASE_FILE};
use collapse_server_backend::storage::Storage;
use collapse_core::Algorithm;
use tempfile::TempDir;

/// A registry and a storage over the same directory, the way the server has
/// them.
fn server(dir: &TempDir) -> (Registry, Storage) {
    (
        Registry::open(dir.path()).expect("the registry opens"),
        Storage::new(dir.path().to_path_buf()),
    )
}

fn job(id: &str) -> Job {
    Job::new(
        id.to_string(),
        "notes.txt".to_string(),
        Algorithm::Zip,
        3,
        Envelope::None,
    )
}

/// A job that exists in both stores: a row, and an upload staged for it.
fn staged_job(registry: &Registry, storage: &Storage, id: &str, status: JobStatus) {
    let mut job = job(id);
    job.status = status;
    registry.add(&job).unwrap();
    storage.save_input(id, "notes.txt", b"hello").unwrap();
}

#[test]
fn an_untouched_server_has_nothing_to_reconcile() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);

    let report = reconcile(&registry, &storage).unwrap();

    assert_eq!(report, Reconciled::default());
    assert!(report.is_clean());
}

#[test]
fn a_completed_job_with_its_files_is_left_alone() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "j1", JobStatus::Completed);

    let report = reconcile(&registry, &storage).unwrap();

    assert!(report.is_clean());
    assert_eq!(
        registry.get("j1").unwrap().unwrap().status,
        JobStatus::Completed
    );
    assert!(storage.has_job("j1"));
}

#[test]
fn jobs_a_restart_cut_short_are_failed_with_a_reason() {
    // Nothing is compressing when the server comes up, so a row claiming to
    // be is provably stale. A client polling it must be told, not left
    // waiting forever on a worker that no longer exists.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "was-queued", JobStatus::Queued);
    staged_job(&registry, &storage, "was-running", JobStatus::Compressing);

    let report = reconcile(&registry, &storage).unwrap();

    assert_eq!(report.interrupted, 2);
    for id in ["was-queued", "was-running"] {
        let job = registry.get(id).unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.error_message.as_deref(), Some(INTERRUPTED));
    }
}

#[test]
fn interrupted_jobs_are_not_requeued() {
    // Deliberate: an input that kills the worker would be retried on every
    // boot, turning a restart policy into a crash loop.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "j1", JobStatus::Compressing);

    reconcile(&registry, &storage).unwrap();

    assert!(registry.unfinished().unwrap().is_empty());
}

#[test]
fn a_job_whose_files_are_gone_is_forgotten() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    registry.add(&job("j1")).unwrap(); // a row, but nothing staged

    let report = reconcile(&registry, &storage).unwrap();

    // Dropped, not also reported as interrupted: the row is queued, but
    // counting it twice would make the startup line read as more damage than
    // there was.
    assert_eq!(
        report,
        Reconciled {
            without_files: 1,
            ..Reconciled::default()
        }
    );
    assert!(
        registry.get("j1").unwrap().is_none(),
        "a job with no files can only 404, so it is dropped"
    );
}

#[test]
fn files_no_job_claims_are_deleted() {
    // The leak this whole change is about: before the registry was durable,
    // a restart left these behind and the API could not even name them.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    storage.save_input("orphan", "notes.txt", b"stranded").unwrap();

    let report = reconcile(&registry, &storage).unwrap();

    assert_eq!(report.orphaned, 1);
    assert!(!storage.has_job("orphan"), "the directory is removed");
}

#[test]
fn the_registrys_own_database_is_never_taken_for_a_job() {
    // The database sits in the same directory as the job folders. Deleting it
    // as an orphan would wipe every job on every startup, so the walk only
    // ever considers directories.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "j1", JobStatus::Completed);

    reconcile(&registry, &storage).unwrap();

    assert!(dir.path().join(DATABASE_FILE).is_file());
    assert!(registry.get("j1").unwrap().is_some());
}

#[test]
fn one_pass_puts_every_kind_of_disagreement_right() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "healthy", JobStatus::Completed);
    staged_job(&registry, &storage, "running", JobStatus::Compressing);
    registry.add(&job("no-files")).unwrap();
    storage.save_input("orphan", "notes.txt", b"stranded").unwrap();

    let report = reconcile(&registry, &storage).unwrap();

    assert_eq!(
        report,
        Reconciled {
            interrupted: 1,
            without_files: 1,
            orphaned: 1,
        }
    );
    assert_eq!(
        registry.get("healthy").unwrap().unwrap().status,
        JobStatus::Completed
    );
    assert_eq!(
        registry.get("running").unwrap().unwrap().status,
        JobStatus::Failed
    );
    assert!(registry.get("no-files").unwrap().is_none());
    assert!(!storage.has_job("orphan"));
}

#[test]
fn reconciling_twice_changes_nothing_the_second_time() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "running", JobStatus::Compressing);
    storage.save_input("orphan", "notes.txt", b"stranded").unwrap();

    reconcile(&registry, &storage).unwrap();
    let second = reconcile(&registry, &storage).unwrap();

    assert!(
        second.is_clean(),
        "a reconciled server restarts into a clean one"
    );
}
