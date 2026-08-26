//! Tests for what happens around a compression: the archive is staged beside
//! its destination and checked before it is allowed to land there.
//!
//! The failure this exists for (issue #70) is not that a compression can fail,
//! it is what a failed compression used to leave behind. zip and tar finalise
//! on drop, so a run that died partway through still closed out a *valid*
//! archive at the destination, silently missing entries: the user saw an error,
//! opened the archive, found it opened fine, and could then delete the
//! originals. `the_old_way_left_a_tar_missing_an_entry` and
//! `the_old_way_left_a_zip_no_check_can_fault` below are that bug, reproduced
//! against the backends, which still write straight to the path they are given.

use std::fs;
use std::path::{Path, PathBuf};

use collapse_core::compression::{
    compress_7z_dir, compress_tar_dir, compress_zip_dir, extract_tar, extract_zip, verify_archive,
};
use collapse_core::{compress, compress_dir, Algorithm, CompressionError, Verify};

const FORMATS: [Algorithm; 3] = [Algorithm::SevenZ, Algorithm::Tar, Algorithm::Zip];

/// Eight kilobytes deflate and LZMA2 cannot shrink, so the archive's data
/// region is about as long as the input and a byte flipped in the middle of the
/// file is certainly inside it rather than in a header.
fn incompressible(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u64).wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

/// `<parent>/data` holding `a.txt` and `b.txt`, plus an empty subdirectory and
/// an empty file: the two shapes an entry listing is most likely to lose.
fn sample_tree(parent: &Path) -> PathBuf {
    let root = parent.join("data");
    fs::create_dir_all(root.join("empty_dir")).unwrap();
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::write(root.join("b.txt"), b"beta").unwrap();
    fs::write(root.join("empty.txt"), b"").unwrap();
    root
}

/// Every entry `compress_dir` is meant to put in an archive of `sample_tree`.
fn sample_tree_entries() -> Vec<String> {
    [
        "data",
        "data/a.txt",
        "data/b.txt",
        "data/empty.txt",
        "data/empty_dir",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

/// Whatever the staging file is called, it says `collapse-part` so a leftover
/// is identifiable; that is the string this asserts on.
fn staging_leftovers(dir: &Path) -> Vec<String> {
    names_in(dir)
        .into_iter()
        .filter(|n| n.contains("collapse-part"))
        .collect()
}

fn flip_byte(path: &Path, offset: usize) {
    let mut bytes = fs::read(path).unwrap();
    bytes[offset] ^= 0xFF;
    fs::write(path, &bytes).unwrap();
}

// -- the reproduction from issue #70 --

/// A tree with one member nobody can read: the compressor gets partway through
/// and fails. This is the case the whole change is about.
#[cfg(unix)]
fn tree_with_an_unreadable_member(parent: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let root = parent.join("data");
    fs::create_dir_all(&root).unwrap();
    // Sorted first, so it is archived before the failure: what makes the
    // leftover a *plausible* archive rather than an empty one.
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    let locked = root.join("b.txt");
    fs::write(&locked, b"beta").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    // Root reads a 0o000 file happily, so in a container this provokes nothing
    // at all. Say so rather than assert something untrue.
    if fs::File::open(&locked).is_ok() {
        eprintln!("skipped: this user can read a 0o000 file, so nothing fails");
        return None;
    }
    Some(root)
}

/// The bug itself, against the two backends that finalise on drop. They still
/// write straight to the path they are handed, so what is left at the
/// destination opens cleanly and is short by the entry that never made it. Only
/// comparing it against what was asked for can tell.
///
/// Delete `verify_archive`'s set comparison and the last assertion fails.
#[cfg(unix)]
#[test]
fn the_old_way_left_a_valid_archive_missing_an_entry() {
    let expected = vec![
        "data".to_string(),
        "data/a.txt".to_string(),
        "data/b.txt".to_string(),
    ];

    for (algorithm, archive_name) in [(Algorithm::Tar, "raw.tar"), (Algorithm::Zip, "raw.zip")] {
        let dir = tempfile::TempDir::new().unwrap();
        let Some(root) = tree_with_an_unreadable_member(dir.path()) else {
            return;
        };
        let archive = dir.path().join(archive_name);

        let err = match algorithm {
            Algorithm::Tar => compress_tar_dir(&root, &archive),
            _ => compress_zip_dir(&root, &archive, 3),
        }
        .unwrap_err();
        assert!(
            matches!(err, CompressionError::Io(_)),
            "{algorithm}: expected the unreadable member to surface as IO, got {err:?}"
        );

        // The archive is there, and it opens. That is the whole problem.
        assert!(archive.exists(), "{algorithm}: nothing was left behind");
        let out = dir.path().join("out");
        let listed = match algorithm {
            Algorithm::Tar => extract_tar(&archive, &out),
            _ => extract_zip(&archive, &out),
        }
        .unwrap_or_else(|e| panic!("{algorithm}: the leftover did not even open: {e}"));
        assert_eq!(
            listed,
            vec!["data/a.txt".to_string()],
            "{algorithm}: the leftover holds only the member read before the failure"
        );

        // And this is what catches it.
        let err = verify_archive(&archive, algorithm, &expected, Verify::Index).unwrap_err();
        assert!(
            err.to_string().contains("data/b.txt"),
            "{algorithm}: the failure must name the entry that is gone: {err}"
        );
    }
}

/// `compress_zip_dir` used to start the entry and *then* read the file, so a
/// member it could not read still got its name into the archive with nothing
/// behind it, and the CRC written for it was the CRC of nothing. That leftover
/// named every entry that was asked for and was entirely self-consistent, so no
/// depth of checking could fault it: an empty file is a legitimate thing for an
/// archive to hold.
///
/// Put the read back after `start_file` and this fails, on an archive that
/// claims to hold `data/b.txt` and holds nothing under that name.
#[cfg(unix)]
#[test]
fn a_member_that_cannot_be_read_is_not_named_in_the_archive_anyway() {
    let dir = tempfile::TempDir::new().unwrap();
    let Some(root) = tree_with_an_unreadable_member(dir.path()) else {
        return;
    };
    let archive = dir.path().join("raw.zip");

    compress_zip_dir(&root, &archive, 3).unwrap_err();

    let out = dir.path().join("out");
    let listed = extract_zip(&archive, &out).expect("the leftover opens like any other zip");
    assert_eq!(
        listed,
        vec!["data/a.txt".to_string()],
        "the member it failed on must not appear at all"
    );
    assert!(!out.join("data/b.txt").exists());
}

/// The same failure through the dispatchers: nothing at all at the output path,
/// for every format.
///
/// Write the archive straight to `output` again and this fails on `exists()`.
#[cfg(unix)]
#[test]
fn a_failed_compression_leaves_nothing_at_the_output_path() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let Some(root) = tree_with_an_unreadable_member(dir.path()) else {
            return;
        };
        let output = dir.path().join(format!("data.{}", algorithm.extension()));

        let err = compress_dir(&root, &output, algorithm, 3, Verify::Index).unwrap_err();

        assert!(
            matches!(err, CompressionError::Io(_)),
            "{algorithm}: expected an IO failure, got {err:?}"
        );
        assert!(
            !output.exists(),
            "{algorithm}: a partial archive was left at {}",
            output.display()
        );
    }
}

/// Nor anything half-written anywhere near it. `StagedOutput` is a guard rather
/// than a pair of calls precisely so the `?` above cannot skip the cleanup.
#[cfg(unix)]
#[test]
fn a_failed_compression_leaves_no_staging_file_behind() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let Some(root) = tree_with_an_unreadable_member(dir.path()) else {
            return;
        };
        let output = dir.path().join(format!("data.{}", algorithm.extension()));

        compress_dir(&root, &output, algorithm, 3, Verify::Index).unwrap_err();

        assert_eq!(
            staging_leftovers(dir.path()),
            Vec::<String>::new(),
            "{algorithm}: a staging file survived the failure"
        );
    }
}

/// The other half of "nothing bad is ever visible at the destination": an
/// archive already sitting there is still the archive that was there.
///
/// This is what a local run could not promise and a remote one always could,
/// since a remote failure never got as far as writing the file.
#[cfg(unix)]
#[test]
fn a_failed_compression_leaves_the_previous_archive_untouched() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let Some(root) = tree_with_an_unreadable_member(dir.path()) else {
            return;
        };
        let output = dir.path().join(format!("data.{}", algorithm.extension()));
        let previous = b"last week's archive, such as it is";
        fs::write(&output, previous).unwrap();

        compress_dir(&root, &output, algorithm, 3, Verify::Index).unwrap_err();

        assert_eq!(
            fs::read(&output).unwrap(),
            previous,
            "{algorithm}: the archive that was already there was destroyed"
        );
    }
}

/// The single-file entry point stages too. A source it cannot open is the
/// easiest way to fail it, and `compress_zip` in particular used to create the
/// output first and leave a zero-byte `.zip` sitting there.
#[test]
fn a_failed_single_file_compression_leaves_nothing_behind() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let output = dir.path().join(format!("out.{}", algorithm.extension()));

        compress(
            &dir.path().join("ghost.txt"),
            &output,
            "ghost.txt",
            algorithm,
            3,
            Verify::Index,
        )
        .unwrap_err();

        assert!(
            !output.exists(),
            "{algorithm}: a stub archive was published"
        );
        assert_eq!(
            staging_leftovers(dir.path()),
            Vec::<String>::new(),
            "{algorithm}: a staging file survived the failure"
        );
    }
}

/// A rename replaces a *name*. An output that happens to be a hardlink to
/// something else therefore stops being one, instead of the compressor writing
/// through the shared inode and taking the other name's content with it.
///
/// This was pinned as a KNOWN LIMITATION in the desktop crate's
/// `replacing_an_output_writes_through_a_hardlink_to_it`, whose own comment
/// named staging and renaming as the fix. Write to `output` directly again and
/// this fails.
///
/// Not `cfg(unix)`: NTFS hardlinks share their data the same way.
#[test]
fn replacing_an_output_no_longer_writes_through_a_hardlink_to_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    fs::write(&src, b"hello").unwrap();
    let output = dir.path().join("out.zip");
    let older = b"an older archive the user agreed to replace";
    fs::write(&output, older).unwrap();
    let bystander = dir.path().join("someone-elses-copy.zip");
    fs::hard_link(&output, &bystander).unwrap();

    compress(&src, &output, "notes.txt", Algorithm::Zip, 3, Verify::Index).unwrap();

    assert_eq!(
        fs::read(&bystander).unwrap(),
        older,
        "the other name for that file was overwritten"
    );
    // And the archive the caller asked for is really there.
    let out = dir.path().join("extracted");
    assert_eq!(extract_zip(&output, &out).unwrap(), vec!["notes.txt"]);
}

/// A run that works leaves the archive and nothing else. Forget to set
/// `committed`, or rename by copying, and this finds the leftover.
#[test]
fn a_successful_compression_leaves_only_the_archive() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let root = sample_tree(dir.path());
        let output = dir.path().join(format!("data.{}", algorithm.extension()));

        compress_dir(&root, &output, algorithm, 3, Verify::Index).unwrap();

        assert!(output.exists(), "{algorithm}: no archive was produced");
        assert_eq!(
            staging_leftovers(dir.path()),
            Vec::<String>::new(),
            "{algorithm}: a staging file survived a successful run"
        );
    }
}

/// The staging name is the output's name plus a suffix, and a file name has a
/// length limit of its own (255 bytes here). Drop the truncation in
/// `keep_bytes` and this fails with `File name too long` on an output name that
/// is perfectly legal.
///
/// Unix only: Windows limits the whole path rather than one component, so the
/// same test there would be measuring the temporary directory's depth.
#[cfg(unix)]
#[test]
fn a_long_output_name_still_compresses() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = sample_tree(dir.path());
    let output = dir.path().join(format!("{}.zip", "n".repeat(250)));

    compress_dir(&root, &output, Algorithm::Zip, 1, Verify::Index)
        .unwrap_or_else(|e| panic!("a 254 byte output name should be fine: {e}"));

    assert!(output.exists());
}

// -- Verify::Index --

/// Every format the crate writes, read back and compared against what
/// `compress_dir` was asked to put in. Directory entries are spelled `data/` by
/// zip and tar and `data` by 7z, so a comparison that did not normalize that
/// away would fail here for two formats out of three.
#[test]
fn index_accepts_what_the_dispatcher_just_wrote() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let root = sample_tree(dir.path());
        let archive = dir.path().join(format!("data.{}", algorithm.extension()));
        compress_dir(&root, &archive, algorithm, 3, Verify::Index).unwrap();

        verify_archive(&archive, algorithm, &sample_tree_entries(), Verify::Index)
            .unwrap_or_else(|e| panic!("{algorithm}: a freshly written archive was refused: {e}"));
    }
}

/// The single-file entry point puts exactly one named entry in the archive.
#[test]
fn index_accepts_a_single_file_archive() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("input.txt");
        fs::write(&src, b"just the one").unwrap();
        let archive = dir.path().join(format!("out.{}", algorithm.extension()));

        compress(&src, &archive, "renamed.dat", algorithm, 3, Verify::Index).unwrap();

        verify_archive(
            &archive,
            algorithm,
            &["renamed.dat".to_string()],
            Verify::Index,
        )
        .unwrap_or_else(|e| panic!("{algorithm}: {e}"));
    }
}

/// A count is not an answer: the message has to say which entry is gone, or
/// nobody can tell whether the archive is worth keeping.
#[test]
fn index_names_the_entries_that_are_missing() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let root = sample_tree(dir.path());
        let archive = dir.path().join(format!("data.{}", algorithm.extension()));
        compress_dir(&root, &archive, algorithm, 3, Verify::Index).unwrap();

        let mut expected = sample_tree_entries();
        expected.push("data/holidays.jpg".to_string());

        let err = verify_archive(&archive, algorithm, &expected, Verify::Index).unwrap_err();

        assert_eq!(
            err.to_string(),
            format!(
                "Verification of {} failed: 1 entry is missing: \"data/holidays.jpg\"",
                archive.display()
            ),
            "{algorithm}"
        );
    }
}

/// The other direction, which is a different bug: an archive holding something
/// nobody asked for.
#[test]
fn index_names_the_entries_that_should_not_be_there() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = sample_tree(dir.path());
    let archive = dir.path().join("data.zip");
    compress_dir(&root, &archive, Algorithm::Zip, 3, Verify::Index).unwrap();

    let expected: Vec<String> = sample_tree_entries()
        .into_iter()
        .filter(|n| n != "data/b.txt" && n != "data/empty.txt")
        .collect();

    let err = verify_archive(&archive, Algorithm::Zip, &expected, Verify::Index).unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "Verification of {} failed: 2 entries are unexpected: \
             \"data/b.txt\", \"data/empty.txt\"",
            archive.display()
        )
    );
}

/// A tree of ten thousand files must not produce a ten thousand name message.
#[test]
fn a_long_list_of_missing_entries_is_cut_short() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = sample_tree(dir.path());
    let archive = dir.path().join("data.tar");
    compress_dir(&root, &archive, Algorithm::Tar, 1, Verify::Index).unwrap();

    let mut expected = sample_tree_entries();
    for i in 0..8 {
        expected.push(format!("data/ghost{i}.txt"));
    }

    let err = verify_archive(&archive, Algorithm::Tar, &expected, Verify::Index).unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "Verification of {} failed: 8 entries are missing: \"data/ghost0.txt\", \
             \"data/ghost1.txt\", \"data/ghost2.txt\", \"data/ghost3.txt\", \
             \"data/ghost4.txt\" and 3 more",
            archive.display()
        )
    );
}

/// A caller has to be able to tell "the archive I just wrote is not right" from
/// "the compressor errored", and a `Failed(String)` cannot be told apart from
/// any other `Failed(String)`.
#[test]
fn a_verification_failure_is_its_own_kind_of_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = sample_tree(dir.path());
    let archive = dir.path().join("data.zip");
    compress_dir(&root, &archive, Algorithm::Zip, 3, Verify::Index).unwrap();

    let err = verify_archive(
        &archive,
        Algorithm::Zip,
        &["nothing/like/it".to_string()],
        Verify::Index,
    )
    .unwrap_err();

    let CompressionError::VerificationFailed {
        archive: named,
        reason,
    } = &err
    else {
        panic!("expected VerificationFailed, got {err:?}");
    };
    assert_eq!(named, &archive, "the error must name the archive");
    assert!(
        reason.contains("nothing/like/it") && reason.contains("unexpected"),
        "the error must say what was wrong: {reason}"
    );
}

/// Reading a file that is not an archive at all is a verification failure too,
/// not an IO error and not a compression error: the compressor was fine, what
/// it produced is not readable.
#[test]
fn an_unreadable_archive_is_a_verification_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    let junk = dir.path().join("junk.zip");
    fs::write(&junk, b"this was never an archive").unwrap();

    let err =
        verify_archive(&junk, Algorithm::Zip, &["a.txt".to_string()], Verify::Index).unwrap_err();

    assert!(
        matches!(err, CompressionError::VerificationFailed { .. }),
        "got {err:?}"
    );
    assert!(
        err.to_string()
            .contains("the archive could not be read back"),
        "{err}"
    );
    // Not "Compression failed: ..." with the reader's words pasted in: that
    // prefix would blame the compressor for a file it wrote correctly.
    assert!(err.to_string().starts_with("Verification of "), "{err}");
}

// -- Verify::Contents --

#[test]
fn contents_accepts_what_the_dispatcher_just_wrote() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let root = sample_tree(dir.path());
        let archive = dir.path().join(format!("data.{}", algorithm.extension()));

        compress_dir(&root, &archive, algorithm, 3, Verify::Contents)
            .unwrap_or_else(|e| panic!("{algorithm}: a good tree was refused: {e}"));

        verify_archive(
            &archive,
            algorithm,
            &sample_tree_entries(),
            Verify::Contents,
        )
        .unwrap_or_else(|e| panic!("{algorithm}: {e}"));
    }
}

/// Decompressing to a sink means to a sink: the check needs no space of its own
/// and cannot itself put anything on disk. Extract to a scratch directory
/// instead and this finds it.
#[test]
fn contents_writes_nothing_to_disk() {
    for algorithm in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let root = sample_tree(dir.path());
        let archive = dir.path().join(format!("data.{}", algorithm.extension()));
        compress_dir(&root, &archive, algorithm, 3, Verify::Index).unwrap();

        let before = names_in(dir.path());
        verify_archive(
            &archive,
            algorithm,
            &sample_tree_entries(),
            Verify::Contents,
        )
        .unwrap();

        assert_eq!(
            names_in(dir.path()),
            before,
            "{algorithm}: verification put something on disk"
        );
    }
}

/// The point of the deeper depth: zip and 7z both store a checksum per entry,
/// and only reading the entry back compares it. The index still lists every
/// name, so `Index` cannot see this and is not supposed to.
///
/// Make `Contents` behave like `Index` and the second half fails.
#[test]
fn contents_catches_a_flipped_bit_that_index_cannot_see() {
    for algorithm in [Algorithm::SevenZ, Algorithm::Zip] {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("input.bin");
        fs::write(&src, incompressible(8192)).unwrap();
        let archive = dir.path().join(format!("out.{}", algorithm.extension()));
        compress(&src, &archive, "input.bin", algorithm, 1, Verify::Contents).unwrap();

        // Halfway through the file. Both formats put the entry data first and
        // their listing at the end, and the data does not compress, so this
        // lands in the payload and leaves the listing intact.
        let midpoint = fs::metadata(&archive).unwrap().len() as usize / 2;
        flip_byte(&archive, midpoint);

        let expected = ["input.bin".to_string()];
        verify_archive(&archive, algorithm, &expected, Verify::Index).unwrap_or_else(|e| {
            panic!("{algorithm}: the listing should be untouched, so Index should pass: {e}")
        });
        let err = verify_archive(&archive, algorithm, &expected, Verify::Contents).unwrap_err();
        assert!(
            matches!(err, CompressionError::VerificationFailed { .. }),
            "{algorithm}: got {err:?}"
        );
        assert!(
            err.to_string().contains("input.bin"),
            "{algorithm}: the failure should name the entry: {err}"
        );
    }
}

/// What tar can and cannot promise, written down as a test rather than as a
/// hopeful sentence in a doc comment.
///
/// tar's only checksum is `cksum`, over the 512 byte header. There is nothing
/// covering an entry's data, so a bit flipped inside a member is invisible to
/// every reader there is, including this one. Saying `Contents` protects a tar
/// the way it protects a zip would be a lie, and this is the test that would
/// have to be deleted to tell it.
#[test]
fn contents_on_a_tar_cannot_see_a_flipped_bit_in_the_data() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.bin");
    fs::write(&src, incompressible(8192)).unwrap();
    let archive = dir.path().join("out.tar");
    compress(
        &src,
        &archive,
        "input.bin",
        Algorithm::Tar,
        1,
        Verify::Contents,
    )
    .unwrap();

    // Well past the 512 byte header, so this is the member's own data.
    flip_byte(&archive, 512 + 4096);

    let expected = ["input.bin".to_string()];
    verify_archive(&archive, Algorithm::Tar, &expected, Verify::Contents)
        .expect("tar stores no checksum over an entry's data, so this cannot be caught");

    // And the damage is real: the extracted file is not the file that went in.
    let out = dir.path().join("out");
    extract_tar(&archive, &out).unwrap();
    assert_ne!(
        fs::read(out.join("input.bin")).unwrap(),
        fs::read(&src).unwrap(),
        "the flip did not change anything, so this test proves nothing"
    );
}

/// The half tar does cover. Its header carries a checksum, so a bent header is
/// caught at both depths.
#[test]
fn a_bent_tar_header_is_caught() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.bin");
    fs::write(&src, b"small enough").unwrap();
    let archive = dir.path().join("out.tar");
    compress(
        &src,
        &archive,
        "input.bin",
        Algorithm::Tar,
        1,
        Verify::Index,
    )
    .unwrap();

    // Byte 4 is inside the name field, which the header checksum covers.
    flip_byte(&archive, 4);

    for depth in [Verify::Index, Verify::Contents] {
        let err = verify_archive(&archive, Algorithm::Tar, &["input.bin".to_string()], depth)
            .unwrap_err();
        assert!(
            matches!(err, CompressionError::VerificationFailed { .. }),
            "{depth:?}: got {err:?}"
        );
    }
}

/// The other half tar does cover, and the one that matters for a compression
/// cut short: a member whose data stops before the header said it would.
#[test]
fn a_tar_member_cut_short_is_caught() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.bin");
    fs::write(&src, incompressible(8192)).unwrap();
    let archive = dir.path().join("out.tar");
    compress(
        &src,
        &archive,
        "input.bin",
        Algorithm::Tar,
        1,
        Verify::Index,
    )
    .unwrap();

    let whole = fs::read(&archive).unwrap();
    fs::write(&archive, &whole[..512 + 4096]).unwrap();

    for depth in [Verify::Index, Verify::Contents] {
        let err = verify_archive(&archive, Algorithm::Tar, &["input.bin".to_string()], depth)
            .unwrap_err();
        assert!(
            matches!(err, CompressionError::VerificationFailed { .. }),
            "{depth:?}: got {err:?}"
        );
    }
}

// -- the dispatchers really do run the check --

/// tar drops a `.` component from an entry name, so asking for
/// `notes/./x.txt` produces an archive holding `notes/x.txt`. The compressor
/// reports success; the archive does not hold what was asked for; the
/// dispatcher must not pretend otherwise.
///
/// This is the one failure that reaches the discard path through verification
/// rather than through the compressor, so it is what proves the dispatcher
/// calls the check at all, and cleans up after it when it fails.
#[test]
fn a_name_the_format_will_not_store_is_refused_and_discarded() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    fs::write(&src, b"whatever").unwrap();
    let output = dir.path().join("out.tar");

    let err = compress(
        &src,
        &output,
        "notes/./x.txt",
        Algorithm::Tar,
        1,
        Verify::Index,
    )
    .unwrap_err();

    assert!(
        matches!(err, CompressionError::VerificationFailed { .. }),
        "got {err:?}"
    );
    // The error names the destination, not the temporary it was checked on:
    // that file is deleted before this returns, and the caller never chose it.
    assert_eq!(
        err.to_string(),
        format!(
            "Verification of {} failed: 1 entry is missing: \"notes/./x.txt\"; \
             1 entry is unexpected: \"notes/x.txt\"",
            output.display()
        )
    );
    assert!(
        !output.exists(),
        "the rejected archive was published anyway"
    );
    assert_eq!(
        staging_leftovers(dir.path()),
        Vec::<String>::new(),
        "the rejected archive was left staged"
    );
}

/// The same, one level up: a rejected `compress_dir` leaves the archive that
/// was already there alone.
#[test]
fn a_rejected_archive_does_not_replace_an_older_one() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    fs::write(&src, b"whatever").unwrap();
    let output = dir.path().join("out.tar");
    let previous = b"the archive from before";
    fs::write(&output, previous).unwrap();

    compress(
        &src,
        &output,
        "notes/./x.txt",
        Algorithm::Tar,
        1,
        Verify::Index,
    )
    .unwrap_err();

    assert_eq!(fs::read(&output).unwrap(), previous);
}

/// 7z has no `Drop` that finalises, so unlike zip and tar it never left a
/// readable-but-short archive; what it left was an unreadable stub. Recorded
/// here because it is the reason the reproduction above only covers two
/// formats, and because the dispatcher now removes it for all three either way.
#[cfg(unix)]
#[test]
fn a_failed_7z_leaves_a_stub_that_is_not_an_archive() {
    let dir = tempfile::TempDir::new().unwrap();
    let Some(root) = tree_with_an_unreadable_member(dir.path()) else {
        return;
    };
    let archive = dir.path().join("raw.7z");

    compress_7z_dir(&root, &archive, 3).unwrap_err();

    assert!(
        archive.exists(),
        "the backend still writes where it is told"
    );
    assert!(
        verify_archive(&archive, Algorithm::SevenZ, &[], Verify::Index).is_err(),
        "a 7z stub should not parse as an archive"
    );
}
