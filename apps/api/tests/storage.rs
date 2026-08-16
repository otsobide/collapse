//! Unit tests for the per-job on-disk staging.

use collapse_api::storage::Storage;
use collapse_core::Algorithm;

fn storage() -> (Storage, tempfile::TempDir) {
    let base = tempfile::TempDir::new().unwrap();
    (Storage::new(base.path().to_path_buf()), base)
}

// -------------------------------------------------------------------- paths --

#[test]
fn input_path_sits_under_the_job_directory() {
    let (storage, base) = storage();
    assert_eq!(
        storage.input_path("job1", "notes.txt"),
        base.path().join("job1").join("input").join("notes.txt")
    );
}

#[test]
fn output_path_is_named_after_the_algorithm() {
    let (storage, base) = storage();
    for (algorithm, expected) in [
        (Algorithm::Zip, "archive.zip"),
        (Algorithm::SevenZ, "archive.7z"),
        (Algorithm::Tar, "archive.tar"),
    ] {
        assert_eq!(
            storage.output_path("job1", algorithm),
            base.path().join("job1").join(expected)
        );
    }
}

/// Uploads live in their own subdirectory, so no upload name — not even
/// `archive.zip` — can land on the path the compressor is about to write.
#[test]
fn input_and_output_paths_never_collide() {
    let (storage, _base) = storage();
    for (name, algorithm) in [
        ("archive.zip", Algorithm::Zip),
        ("archive.7z", Algorithm::SevenZ),
        ("archive.tar", Algorithm::Tar),
    ] {
        assert_ne!(
            storage.input_path("job1", name),
            storage.output_path("job1", algorithm),
            "{name} collides with its own output"
        );
    }
}

#[test]
fn each_job_gets_its_own_directory() {
    let (storage, _base) = storage();
    assert_ne!(
        storage.input_path("job1", "notes.txt"),
        storage.input_path("job2", "notes.txt")
    );
}

// ------------------------------------------------------------------- saving --

#[test]
fn save_input_creates_the_job_directory_and_writes_the_bytes() {
    let (storage, _base) = storage();

    storage.save_input("job1", "notes.txt", b"payload").unwrap();

    let path = storage.input_path("job1", "notes.txt");
    assert_eq!(std::fs::read(&path).unwrap(), b"payload");
}

#[test]
fn save_input_accepts_an_empty_upload() {
    let (storage, _base) = storage();

    storage.save_input("job1", "empty.txt", b"").unwrap();

    assert_eq!(
        std::fs::read(storage.input_path("job1", "empty.txt")).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn same_file_name_in_two_jobs_does_not_collide() {
    let (storage, _base) = storage();

    storage.save_input("job1", "notes.txt", b"first").unwrap();
    storage.save_input("job2", "notes.txt", b"second").unwrap();

    assert_eq!(std::fs::read(storage.input_path("job1", "notes.txt")).unwrap(), b"first");
    assert_eq!(std::fs::read(storage.input_path("job2", "notes.txt")).unwrap(), b"second");
}

// ----------------------------------------------------------------- deletion --

#[test]
fn delete_job_removes_the_input_and_the_archive() {
    let (storage, base) = storage();
    storage.save_input("job1", "notes.txt", b"payload").unwrap();
    std::fs::write(storage.output_path("job1", Algorithm::Zip), b"archive").unwrap();

    assert!(storage.delete_job("job1"));

    assert!(!base.path().join("job1").exists());
}

#[test]
fn delete_job_returns_false_for_an_unknown_job() {
    let (storage, _base) = storage();
    assert!(!storage.delete_job("ghost"));
}

#[test]
fn delete_job_leaves_other_jobs_alone() {
    let (storage, _base) = storage();
    storage.save_input("job1", "notes.txt", b"first").unwrap();
    storage.save_input("job2", "notes.txt", b"second").unwrap();

    storage.delete_job("job1");

    assert!(!storage.input_path("job1", "notes.txt").exists());
    assert_eq!(std::fs::read(storage.input_path("job2", "notes.txt")).unwrap(), b"second");
}

#[test]
fn delete_job_is_not_idempotent_about_its_answer() {
    // The second call reports "nothing to delete", which is what makes the
    // DELETE endpoint able to say whether files were actually removed.
    let (storage, _base) = storage();
    storage.save_input("job1", "notes.txt", b"payload").unwrap();

    assert!(storage.delete_job("job1"));
    assert!(!storage.delete_job("job1"));
}
