//! Unit tests for the pure input validators.

use collapse_server_backend::validate::{header_safe, is_bare_file_name};

// ------------------------------------------------------------ bare names --

#[test]
fn ordinary_file_names_are_accepted() {
    // Nothing here may be rejected: a guard that is too strict silently
    // breaks legitimate uploads.
    for name in [
        "notes.txt",
        "archive.tar.gz",
        ".hidden",
        "..hidden",       // leading dots, but not the `..` component
        "a..b.txt",       // `..` inside a name is just characters
        "...",            // three dots is a legal file name
        "résumé.pdf",
        "file name with spaces.txt",
        "-",
        "a",
        "UPPER.ZIP",
        "1234",
    ] {
        assert!(is_bare_file_name(name), "{name:?} should be accepted");
    }
}

#[test]
fn traversal_and_separator_names_are_rejected() {
    for name in [
        "",
        ".",
        "..",
        "../evil.txt",
        "../../etc/passwd",
        "/etc/passwd",
        "a/b.txt",
        "./a.txt",
        "dir/",
        "a\\b.txt",       // backslash: a path separator on Windows
        "..\\..\\evil",
        "\\\\server\\share",
        "a\0b.txt",       // NUL: rejected before it can truncate a path
    ] {
        assert!(!is_bare_file_name(name), "{name:?} should be rejected");
    }
}

// -------------------------------------------------------- header sanitizing --

#[test]
fn header_safe_strips_quote_backslash_and_newlines() {
    assert_eq!(header_safe("we\"ird.txt"), "weird.txt");
    assert_eq!(header_safe("back\\slash.txt"), "backslash.txt");
    // A header-injection attempt collapses into an inert single line.
    assert_eq!(
        header_safe("evil.txt\r\nSet-Cookie: a=b"),
        "evil.txtSet-Cookie: a=b"
    );
}

#[test]
fn header_safe_leaves_ordinary_names_untouched() {
    for name in ["notes.txt.zip", "résumé.pdf.7z", "a b c.tar", ""] {
        assert_eq!(header_safe(name), name);
    }
}

#[test]
fn header_safe_can_reduce_a_name_to_nothing() {
    // Every character is strippable: the result is empty rather than a
    // half-escaped value.
    assert_eq!(header_safe("\"\"\\\r\n"), "");
}
