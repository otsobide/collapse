//! Keeping the two stores honest: the startup pass that reconciles what a
//! restart or a crash left disagreeing, and the reaper that collects jobs
//! nobody came back for.

use collapse_core::Algorithm;
use collapse_server_backend::maintenance::{reap, reconcile, Reaped, Reconciled, INTERRUPTED};
use collapse_server_backend::models::{Envelope, Job, JobStatus};
use collapse_server_backend::registry::{now_unix, Registry, DATABASE_FILE};
use collapse_server_backend::storage::{Storage, JOBS_DIR, REGISTRY_DIR};
use tempfile::TempDir;

/// A deadline every job is older than, and one no job is older than. Passing
/// the boundary in explicitly is what keeps these tests off the clock.
const LONG_AGO: i64 = 0;
fn any_moment_now() -> i64 {
    now_unix() + 3600
}

/// A registry and a storage laid out the way `build_app` lays them out: two
/// subdirectories of one storage directory, so either can be a volume of its
/// own.
fn server(dir: &TempDir) -> (Registry, Storage) {
    let registry_dir = dir.path().join(REGISTRY_DIR);
    let jobs_dir = dir.path().join(JOBS_DIR);
    std::fs::create_dir_all(&registry_dir).unwrap();
    std::fs::create_dir_all(&jobs_dir).unwrap();
    (
        Registry::open(&registry_dir).expect("the registry opens"),
        Storage::new(jobs_dir),
    )
}

/// The job staging area inside a test's storage directory.
fn jobs_dir(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join(JOBS_DIR)
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
    storage.save_input(id, b"hello").unwrap();
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
    storage.save_input("orphan", b"stranded").unwrap();

    let report = reconcile(&registry, &storage).unwrap();

    assert_eq!(report.orphaned, 1);
    assert!(!storage.has_job("orphan"), "the directory is removed");
}

#[test]
#[cfg(unix)]
fn an_orphan_that_cannot_be_removed_is_not_counted_as_collected() {
    // The startup half of the accounting the reaper got wrong. `delete_job`
    // now answers with three outcomes instead of a bool, and reconcile reads
    // them: a directory it could not remove must stay out of `orphaned`, or
    // every boot reports a cleanup that never happened while the disk keeps
    // filling. The other orphan is here to prove the pass carries on past the
    // failure: mapping the error out with `?` would refuse to start the whole
    // server over one unremovable directory, with no API left to fix it from.
    //
    // Pinned, not endorsed: `Reconciled` has no `unremovable` field to put the
    // failure in, the way `Reaped` grew one, so a boot that could not remove a
    // single orphan still reports itself clean and only the per-entry warning
    // says otherwise.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    storage.save_input("removable", b"stranded").unwrap();
    storage.save_input("stuck", b"stranded").unwrap();
    let stuck = jobs_dir(&dir).join("stuck");
    std::fs::write(stuck.join("archive.zip"), b"archive").unwrap();
    if !obstruct(&stuck) {
        return;
    }

    let report = reconcile(&registry, &storage).unwrap();
    // The archive, rather than the upload: a removal that fails part of the
    // way through still eats what it managed to unlink first (here the upload,
    // whose own directory is writable), so the file at the top of the job
    // directory is the one that proves the tree is still there.
    let stuck_left = (
        storage.has_job("stuck"),
        stuck.join("archive.zip").is_file(),
    );
    release(&stuck);

    assert_eq!(
        report,
        Reconciled {
            orphaned: 1,
            ..Reconciled::default()
        },
        "only the one that really went is counted"
    );
    assert!(
        !storage.has_job("removable"),
        "the failure does not stop the sweep reaching the next directory"
    );
    assert_eq!(
        stuck_left,
        (true, true),
        "and the one it could not remove is still there, waiting for the next boot"
    );
}

#[test]
fn the_registrys_database_is_not_in_the_area_the_sweep_walks() {
    // The two live in separate directories, so the sweep cannot reach the
    // database at all. When they shared one, the only thing keeping it from
    // being deleted as an orphan was a rule about files versus directories,
    // and that rule is one refactor away from being lost.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "j1", JobStatus::Completed);

    reconcile(&registry, &storage).unwrap();

    assert!(dir.path().join(REGISTRY_DIR).join(DATABASE_FILE).is_file());
    assert!(
        !jobs_dir(&dir).join(DATABASE_FILE).exists(),
        "the database is not among the jobs"
    );
    assert!(registry.get("j1").unwrap().is_some());
}

/// And the split is the whole point: each half can be a volume of its own.
#[test]
fn the_two_halves_live_under_one_parent_in_separate_directories() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "j1", JobStatus::Completed);

    let registry_dir = dir.path().join(REGISTRY_DIR);
    assert!(registry_dir.join(DATABASE_FILE).is_file());
    assert!(jobs_dir(&dir).join("j1").is_dir());
    assert_ne!(registry_dir, jobs_dir(&dir));
    assert_eq!(registry_dir.parent(), jobs_dir(&dir).parent());
}

#[test]
fn one_pass_puts_every_kind_of_disagreement_right() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "healthy", JobStatus::Completed);
    staged_job(&registry, &storage, "running", JobStatus::Compressing);
    registry.add(&job("no-files")).unwrap();
    storage.save_input("orphan", b"stranded").unwrap();

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

// ------------------------------------------------------------- the reaper --

#[test]
fn a_finished_job_nobody_came_back_for_is_reaped() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "forgotten", JobStatus::Completed);

    let report = reap(&registry, &storage, any_moment_now()).unwrap();

    assert_eq!(
        report,
        Reaped {
            collected: 1,
            unremovable: 0
        }
    );
    assert!(registry.get("forgotten").unwrap().is_none());
    assert!(!storage.has_job("forgotten"), "its files go with it");
}

#[test]
fn a_failed_job_is_reaped_too() {
    // Nobody is coming for an archive that does not exist, and its upload is
    // still on disk.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "failed", JobStatus::Failed);

    assert_eq!(
        reap(&registry, &storage, any_moment_now())
            .unwrap()
            .collected,
        1
    );
    assert!(!storage.has_job("failed"));
}

#[test]
fn a_job_inside_its_window_is_left_alone() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "recent", JobStatus::Completed);

    let report = reap(&registry, &storage, LONG_AGO).unwrap();

    assert!(report.is_quiet(), "nothing to collect: {report:?}");
    assert!(registry.get("recent").unwrap().is_some());
    assert!(storage.has_job("recent"));
}

#[test]
fn work_in_progress_is_never_reaped_however_old_it_looks() {
    // The worker owns these. Deleting one under it would leave a compression
    // writing into a directory that no longer exists.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "queued", JobStatus::Queued);
    staged_job(&registry, &storage, "running", JobStatus::Compressing);

    let report = reap(&registry, &storage, any_moment_now()).unwrap();

    assert!(report.is_quiet(), "the worker keeps its jobs: {report:?}");
    assert!(storage.has_job("queued"));
    assert!(storage.has_job("running"));
}

#[test]
fn downloading_restarts_the_clock() {
    // The rule the window is built on: a client that keeps fetching its
    // archive keeps it, one that stops asking loses it.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "wanted", JobStatus::Completed);

    let deadline = now_unix(); // everything up to this instant is expired
    std::thread::sleep(std::time::Duration::from_millis(1100)); // the clock has one-second resolution
    registry.touch("wanted").unwrap();

    assert_eq!(reap(&registry, &storage, deadline).unwrap().collected, 0);
    assert!(storage.has_job("wanted"));
}

#[test]
fn reaping_an_empty_registry_is_a_no_op() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);

    // Quiet in both numbers, which is what keeps an idle server from logging
    // a sweep every few minutes.
    assert_eq!(
        reap(&registry, &storage, any_moment_now()).unwrap(),
        Reaped::default()
    );
}

// -------------------------------------- what the reaper could not collect --
//
// A pass that cannot remove a job's files must say so. It used to forget the
// row and count the job regardless, which left files nothing claimed (the one
// state neither the API nor the reaper can see again, only the next startup)
// under a log line reporting a clean sweep. On Unix the arm is reached through
// mode bits, as below; the case an operator actually meets is a volume gone
// read-only, or a file another process holds open on Windows.

/// Make a job's staging directory refuse to give up what is inside it.
/// Answers whether the obstruction took: as root it does not, since mode bits
/// are not enforced there, and a test that went on to assert a failed removal
/// would be asserting the opposite of what it means.
#[cfg(unix)]
fn obstruct(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Unlinking an entry needs write permission on the directory holding it,
    // so a probe that lands is proof the mode bits are not being enforced.
    let probe = dir.join("probe");
    if std::fs::write(&probe, b"").is_ok() {
        std::fs::remove_file(&probe).unwrap();
        release(dir);
        return false;
    }
    true
}

/// Undo [`obstruct`], so the temp directory can be cleaned up afterwards.
/// Called before the assertions, never after: a panic in between would leave
/// the directory behind for good.
#[cfg(unix)]
fn release(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
#[cfg(unix)]
fn a_job_whose_files_cannot_be_removed_is_not_forgotten() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "stuck", JobStatus::Completed);
    let staged = jobs_dir(&dir).join("stuck");
    // The archive is what an operator finds left behind, so it is what the
    // test looks for afterwards.
    std::fs::write(staged.join("archive.zip"), b"archive").unwrap();
    if !obstruct(&staged) {
        return;
    }

    let report = reap(&registry, &storage, any_moment_now()).unwrap();
    let row = registry.get("stuck").unwrap();
    let archive_left = staged.join("archive.zip").is_file();
    release(&staged);

    assert_eq!(
        report,
        Reaped {
            collected: 0,
            unremovable: 1
        },
        "a sweep must not report a job it could not collect"
    );
    assert!(
        row.is_some(),
        "the row stays, so the next sweep comes back for it"
    );
    assert!(
        archive_left,
        "and it has to: the archive is still on the disk the reaper exists to free"
    );
}

#[test]
fn a_job_whose_files_are_already_gone_is_still_forgotten() {
    // Keeping the row when a removal fails must not also keep it when there
    // was nothing to remove: that would trade the leak on disk for a row
    // nothing will ever collect, which is the same bug in the other store.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "fileless", JobStatus::Completed);
    std::fs::remove_dir_all(jobs_dir(&dir).join("fileless")).unwrap();

    let report = reap(&registry, &storage, any_moment_now()).unwrap();

    assert_eq!(
        report,
        Reaped {
            collected: 1,
            unremovable: 0
        }
    );
    assert!(
        registry.get("fileless").unwrap().is_none(),
        "the row goes with the files it no longer has"
    );
}

#[test]
#[cfg(unix)]
fn the_next_pass_retries_a_job_it_could_not_remove() {
    // Which is the whole reason the row is kept: whatever held the files is
    // usually gone by the next sweep, and nobody has to intervene.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "stuck", JobStatus::Completed);
    let staged = jobs_dir(&dir).join("stuck");
    if !obstruct(&staged) {
        return;
    }

    let first = reap(&registry, &storage, any_moment_now()).unwrap();
    release(&staged);
    let second = reap(&registry, &storage, any_moment_now()).unwrap();

    assert_eq!(first.unremovable, 1, "the first pass really was obstructed");
    assert_eq!(
        second,
        Reaped {
            collected: 1,
            unremovable: 0
        }
    );
    assert!(registry.get("stuck").unwrap().is_none());
    assert!(
        !storage.has_job("stuck"),
        "the files that outlived the first pass go in the second"
    );
}

#[test]
#[cfg(unix)]
fn one_sweep_counts_what_it_collected_and_what_it_could_not_apart() {
    // The two numbers have to be kept apart *within* a pass, which is the
    // shape the bug hid in: the outcome belongs to each job, not to the sweep.
    // Every other test here has one job in it, so a pass that counted three
    // collected because it saw three rows would pass them all.
    //
    // The obstructed job is staged between two collectable ones because
    // `expired` returns rows in the order they were written: a pass that gave
    // up at the first failure would leave the one after it uncollected, and
    // the sweep runs on a timer where that is silent.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "before", JobStatus::Completed);
    staged_job(&registry, &storage, "stuck", JobStatus::Completed);
    staged_job(&registry, &storage, "after", JobStatus::Completed);
    let stuck = jobs_dir(&dir).join("stuck");
    if !obstruct(&stuck) {
        return;
    }

    let report = reap(&registry, &storage, any_moment_now()).unwrap();
    let rows: Vec<bool> = ["before", "after", "stuck"]
        .iter()
        .map(|id| registry.get(id).unwrap().is_some())
        .collect();
    let staged: Vec<bool> = ["before", "after", "stuck"]
        .iter()
        .map(|id| storage.has_job(id))
        .collect();
    release(&stuck);

    assert_eq!(
        report,
        Reaped {
            collected: 2,
            unremovable: 1
        }
    );
    // And each job ended in the state its own outcome calls for.
    assert_eq!(
        rows,
        vec![false, false, true],
        "the collected jobs lose their rows, the obstructed one keeps its own"
    );
    assert_eq!(
        staged,
        vec![false, false, true],
        "and the disk agrees with the registry, job by job"
    );
}

/// `is_quiet` is what decides whether the sweep says anything at all, so a
/// pass that collected nothing but could not remove a job must not be quiet:
/// the count and its warning would be the only trace of the leak, and the
/// server would swallow both. Every other test here reads it on an all-zero
/// report, where an `is_quiet` that only looked at `collected` would pass.
#[test]
fn only_a_sweep_that_did_nothing_at_all_is_quiet() {
    assert!(Reaped::default().is_quiet());
    assert!(!Reaped {
        collected: 1,
        unremovable: 0
    }
    .is_quiet());
    assert!(!Reaped {
        collected: 0,
        unremovable: 1
    }
    .is_quiet());
    assert!(!Reaped {
        collected: 2,
        unremovable: 3
    }
    .is_quiet());
}

#[test]
#[cfg(unix)]
fn a_job_the_reaper_could_not_remove_is_nobody_elses_to_collect() {
    // The trade-off the commit took on knowingly, pinned so a later change
    // has to take it on knowingly too: keeping the row also hides the
    // directory from the startup pass, which only removes what no row claims.
    // Until the reaper wins, those bytes belong to a job that is past its
    // window and that a restart will not reclaim either.
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "stuck", JobStatus::Completed);
    let stuck = jobs_dir(&dir).join("stuck");
    if !obstruct(&stuck) {
        return;
    }

    let swept = reap(&registry, &storage, any_moment_now()).unwrap();
    let restarted = reconcile(&registry, &storage).unwrap();
    let row = registry.get("stuck").unwrap();
    let staged = storage.has_job("stuck");
    release(&stuck);

    assert_eq!(swept.unremovable, 1, "the pass really was obstructed");
    assert!(
        restarted.is_clean(),
        "a claimed directory is not an orphan, so the startup pass leaves it: {restarted:?}"
    );
    assert!(row.is_some(), "the row survives the restart");
    assert!(staged, "and so do the files it points at");
}

/// The reaper and the startup pass have to agree: what one collects, the other
/// must not have to clean up after.
#[test]
fn a_reaped_job_leaves_nothing_for_the_startup_pass() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "forgotten", JobStatus::Completed);

    reap(&registry, &storage, any_moment_now()).unwrap();
    let report = reconcile(&registry, &storage).unwrap();

    assert!(report.is_clean(), "nothing left over: {report:?}");
}

#[test]
fn reconciling_twice_changes_nothing_the_second_time() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "running", JobStatus::Compressing);
    storage.save_input("orphan", b"stranded").unwrap();

    reconcile(&registry, &storage).unwrap();
    let second = reconcile(&registry, &storage).unwrap();

    assert!(
        second.is_clean(),
        "a reconciled server restarts into a clean one"
    );
}

/// A staged directory whose name is not valid UTF-8 must actually be removed,
/// not merely counted. `staged_jobs` reads names off the filesystem, where a
/// name is bytes rather than text, and anything that turns those bytes into a
/// String on the way to `delete_job` deletes a path that does not exist while
/// the report claims otherwise: the directory then leaks forever and every
/// startup says it was cleaned.
#[test]
// Linux only: macOS refuses to create a file name that is not valid UTF-8, so
// the case cannot even be staged there, and Windows names are UTF-16 rather
// than bytes, so `OsStr::from_bytes` does not exist and the case does not
// arise. What those two lose is the proof that `staged_jobs` -> `delete_job`
// carries a name through as bytes rather than through a `String`; the Linux
// leg of CI covers it for all three.
#[cfg(target_os = "linux")]
fn an_orphan_whose_name_is_not_text_is_really_deleted() {
    use std::os::unix::ffi::OsStrExt;

    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);

    let name = std::ffi::OsStr::from_bytes(b"job\xffwith-odd-bytes");
    // In the job area, which is where the sweep looks.
    let orphan = jobs_dir(&dir).join(name);
    std::fs::create_dir_all(orphan.join("input")).unwrap();
    std::fs::write(orphan.join("input/notes.txt"), b"stranded").unwrap();

    let report = reconcile(&registry, &storage).unwrap();

    assert_eq!(report.orphaned, 1, "it is recognised as an orphan");
    assert!(
        !orphan.exists(),
        "and it is gone, not just counted: the report must not claim a cleanup it did not do"
    );
}

// ------------------------------------ rows no build in the world can read --

/// Write a row the way a newer server would, with a format this build has
/// never heard of.
fn plant_unreadable_row(dir: &TempDir, job_id: &str) {
    rusqlite::Connection::open(dir.path().join(REGISTRY_DIR).join(DATABASE_FILE))
        .expect("the database opens")
        .execute(
            "INSERT OR REPLACE INTO jobs
                 (job_id, name, archive_name, algorithm, level, envelope, status,
                  error_message, created_at, updated_at, server_version)
             VALUES (?1, 'notes.txt', 'notes.txt.zst', 'zstd', 3, 'none',
                     'completed', NULL, 0, 0, '0.9.0')",
            rusqlite::params![job_id],
        )
        .expect("the row is written");
}

/// One row nobody can parse used to stop the reaper on its first pass, and it
/// runs every few minutes: disk was never reclaimed again, for any job.
#[test]
fn an_unreadable_row_does_not_stop_the_reaper() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "healthy", JobStatus::Completed);
    plant_unreadable_row(&dir, "unreadable");
    storage.save_input("unreadable", b"stranded").unwrap();

    let report = reap(&registry, &storage, any_moment_now()).unwrap();

    assert_eq!(
        report.collected, 2,
        "both went, including the one it cannot read"
    );
    assert!(!storage.has_job("healthy"));
    assert!(!storage.has_job("unreadable"));
    assert!(registry.ids().unwrap().is_empty());
}

/// And the startup pass, where the same failure was worse: it made the server
/// refuse to boot, and the row could not be removed through an API that was
/// not running.
#[test]
fn an_unreadable_row_does_not_stop_the_server_starting() {
    let dir = TempDir::new().unwrap();
    let (registry, storage) = server(&dir);
    staged_job(&registry, &storage, "healthy", JobStatus::Completed);
    plant_unreadable_row(&dir, "unreadable"); // no files staged for it

    let report = reconcile(&registry, &storage).unwrap();

    assert_eq!(report.without_files, 1, "the file-less row is dropped");
    assert!(registry.get("unreadable").unwrap().is_none());
    assert_eq!(
        registry.get("healthy").unwrap().unwrap().status,
        JobStatus::Completed,
        "and its neighbour is untouched"
    );
}
