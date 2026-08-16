//! Pure input validators, split out of the handlers — like the desktop app's
//! `paths.js` — so the test crate can exercise them directly: source files
//! here carry no inline `mod tests`.

/// Whether `name` is a bare file name, safe to use both as the arcname stored
/// inside the archive and as the leaf of the job's staging path.
///
/// Rejects the empty name, `.` and `..`, and anything carrying a path
/// separator or a NUL byte. A backslash counts as a separator even on Unix,
/// where it is an ordinary filename character: the archives produced here are
/// opened on Windows too, and `a\b` is a path there.
pub fn is_bare_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// Strip the characters that could break out of a quoted
/// `Content-Disposition` filename.
pub fn header_safe(filename: &str) -> String {
    filename
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\n' | '\r'))
        .collect()
}
