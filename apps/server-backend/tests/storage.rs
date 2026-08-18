//! Unit tests for the per-job on-disk staging.

use collapse_server_backend::storage::{single_root_dir, Storage, UPLOAD_FILE};
use collapse_core::Algorithm;

fn storage() -> (Storage, tempfile::TempDir) {
    let base = tempfile::TempDir::new().unwrap();
    (Storage::new(base.path().to_path_buf()), base)
}

// -------------------------------------------------------------------- paths --

/// The upload is staged under a name this server chose, never the one the
/// caller sent: nothing off the wire is a path component.
#[test]
fn input_path_is_built_only_from_values_the_server_chose() {
    let (storage, base) = storage();
    assert_eq!(
        storage.input_path("job1"),
        base.path().join("job1").join("input").join(UPLOAD_FILE)
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

/// Uploads live in their own subdirectory under a name of the server's
/// choosing, so nothing can land on the path the compressor is about to write.
/// An upload called `archive.zip` used to be able to, and truncated itself.
#[test]
fn input_and_output_paths_never_collide() {
    let (storage, _base) = storage();
    for algorithm in [Algorithm::Zip, Algorithm::SevenZ, Algorithm::Tar] {
        assert_ne!(
            storage.input_path("job1"),
            storage.output_path("job1", algorithm),
            "the upload collides with its own output for {algorithm}"
        );
    }
}

#[test]
fn each_job_gets_its_own_directory() {
    let (storage, _base) = storage();
    assert_ne!(
        storage.input_path("job1"),
        storage.input_path("job2")
    );
}

// ------------------------------------------------------------------- saving --

#[test]
fn save_input_creates_the_job_directory_and_writes_the_bytes() {
    let (storage, _base) = storage();

    storage.save_input("job1", b"payload").unwrap();

    let path = storage.input_path("job1");
    assert_eq!(std::fs::read(&path).unwrap(), b"payload");
}

#[test]
fn save_input_accepts_an_empty_upload() {
    let (storage, _base) = storage();

    storage.save_input("job1", b"").unwrap();

    assert_eq!(
        std::fs::read(storage.input_path("job1")).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn same_file_name_in_two_jobs_does_not_collide() {
    let (storage, _base) = storage();

    storage.save_input("job1", b"first").unwrap();
    storage.save_input("job2", b"second").unwrap();

    assert_eq!(std::fs::read(storage.input_path("job1")).unwrap(), b"first");
    assert_eq!(std::fs::read(storage.input_path("job2")).unwrap(), b"second");
}

// ----------------------------------------------------------------- deletion --

#[test]
fn delete_job_removes_the_input_and_the_archive() {
    let (storage, base) = storage();
    storage.save_input("job1", b"payload").unwrap();
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
    storage.save_input("job1", b"first").unwrap();
    storage.save_input("job2", b"second").unwrap();

    storage.delete_job("job1");

    assert!(!storage.input_path("job1").exists());
    assert_eq!(std::fs::read(storage.input_path("job2")).unwrap(), b"second");
}

// ------------------------------------------------- unpacked tar envelopes --

/// The tar comes from a client, so the shape of what it unpacked to is
/// checked rather than assumed.
#[test]
fn single_root_dir_accepts_the_expected_directory() {
    let tree = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(tree.path().join("photos")).unwrap();

    assert_eq!(
        single_root_dir(tree.path(), "photos").unwrap(),
        tree.path().join("photos")
    );
}

#[test]
fn single_root_dir_rejects_a_file() {
    let tree = tempfile::TempDir::new().unwrap();
    std::fs::write(tree.path().join("photos"), b"not a directory").unwrap();

    let error = single_root_dir(tree.path(), "photos").unwrap_err();
    assert!(error.contains("must hold a directory"), "got {error:?}");
}

/// A tar whose top-level entry is not what the job was created for would make
/// the archive's contents disagree with its name.
#[test]
fn single_root_dir_rejects_a_mismatched_name() {
    let tree = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(tree.path().join("something-else")).unwrap();

    let error = single_root_dir(tree.path(), "photos").unwrap_err();
    assert!(error.contains("something-else"), "got {error:?}");
    assert!(error.contains("photos"), "got {error:?}");
}

#[test]
fn single_root_dir_rejects_anything_but_one_entry() {
    let empty = tempfile::TempDir::new().unwrap();
    assert!(single_root_dir(empty.path(), "photos")
        .unwrap_err()
        .contains("exactly one"));

    let two = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(two.path().join("photos")).unwrap();
    std::fs::create_dir(two.path().join("extra")).unwrap();
    assert!(single_root_dir(two.path(), "photos")
        .unwrap_err()
        .contains("exactly one"));
}

#[test]
fn tree_path_is_apart_from_the_upload_and_the_archive() {
    let (storage, _base) = storage();
    let tree = storage.tree_path("job1");
    assert_ne!(tree, storage.input_path("job1"));
    assert_ne!(tree, storage.output_path("job1", Algorithm::Zip));
}

#[test]
fn delete_job_is_not_idempotent_about_its_answer() {
    // The second call reports "nothing to delete", which is what makes the
    // DELETE endpoint able to say whether files were actually removed.
    let (storage, _base) = storage();
    storage.save_input("job1", b"payload").unwrap();

    assert!(storage.delete_job("job1"));
    assert!(!storage.delete_job("job1"));
}
