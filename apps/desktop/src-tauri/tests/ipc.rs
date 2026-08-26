//! Lockstep guard for the JS/Rust IPC boundary.
//!
//! Four places describe the same set of commands and **nothing type-checks
//! the crossing**, so a rename compiles, ships, and only breaks when a user
//! clicks the button:
//!
//!   1. `src/lib.rs`               -> `tauri::generate_handler![...]`
//!   2. `src/commands.rs`          -> the `#[tauri::command]` functions
//!   3. `../src/**/*.{vue,js,ts}`  -> the `invoke('...')` string literals
//!   4. `../tests/App.test.js`     -> the `if (cmd === '...')` stub switch
//!
//! Argument names cross camelCase on the JS side and snake_case on the Rust
//! side, so App.vue's `outputDir` binds to `extract_archive`'s `output_dir`
//! parameter, and the wire name of a command is its function name. Neither is
//! a law of nature: both are defaults of `#[tauri::command]` (tauri-macros
//! 2.6.3 starts from `ArgumentCase::Camel` and `RenamePolicy::Keep`), and the
//! attribute can switch either off with `rename_all = "snake_case"` or
//! `rename = "compressPath"`. This whole file models the defaults, so one test
//! pins the assumption instead of trusting it:
//! `no_command_attribute_renames_the_wire_contract` reads the arguments of
//! every `#[tauri::command]` and fails if the model stops applying.
//!
//! What this file does NOT check is **types**. Payload keys are matched to
//! Rust parameter names only, never to the values behind them, because the
//! frontend sends expressions (`level: level.value`) rather than literals a
//! parser could type. `command_signatures_are_pinned_with_their_types` freezes
//! the Rust half of that gap, so `level: u32` cannot quietly become
//! `level: String`; a frontend that starts sending a string for a `u32` is
//! still caught at runtime only.
//!
//! Invocations are read from the script blocks of `.vue` files and from whole
//! `.js`/`.ts` files. A `.vue` template is deliberately skipped: an apostrophe
//! in ordinary prose would open a string literal for the scanner and swallow
//! the rest of the file. The consequence is a real limitation, so keep calling
//! `invoke` from a script block, never from an inline template handler, or
//! this guard cannot see the call.
//!
//! The cross-checks read the real source files at run time and compare the
//! four sides against each other, so they fail on a genuine rename and not on
//! a reformat: whitespace, line breaks, either quote style and trailing commas
//! are all tolerated (verified by reformatting the handler list onto one line
//! and the `invoke` call across several, which changes nothing here). Any file
//! that cannot be read or parsed panics loudly, because a lockstep test that
//! quietly finds zero commands is worse than no test at all.
//!
//! Two tables are restated by hand on purpose, and only two: `BASELINE`, read
//! by `the_parsers_find_the_commands_this_app_ships`, and the signature table
//! in `command_signatures_are_pinned_with_their_types`. Comparing the sources
//! only to each other cannot notice a parser that has gone blind, nor a type
//! that changed on both sides at once, so those two anchor the rest. Each is
//! compared for equality, not containment, so a fifth command cannot slip past
//! either one without a deliberate edit here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// The commands this app has shipped since the remote-compression work
/// landed. Purely a canary for the parsers below: if a parse silently starts
/// returning nothing, every other test in this file would pass vacuously.
///
/// The handler list is compared against this one for **equality**, not
/// containment, so adding a fifth command is also a deliberate edit here.
/// That matters: a command missing from this list would get none of the
/// anti-vacuity protection the canary exists to provide.
const BASELINE: [&str; 5] = [
    "check_server",
    "compress_path",
    "extract_archive",
    "is_directory",
    "unwritable_names",
];

/// Quote characters that open a string literal, per language. Needed by every
/// scanner here so a `//` inside `'http://localhost:8000'` is not mistaken for
/// a comment, and a `,` inside a string does not split a list.
const JS_QUOTES: [char; 3] = ['\'', '"', '`'];
const RUST_QUOTES: [char; 1] = ['"'];

/// Frontend file extensions that may contain an `invoke(...)` call.
const FRONTEND_EXTENSIONS: [&str; 3] = ["vue", "js", "ts"];

// ------------------------------------------------------------------ reading --

/// Read a file relative to this crate's manifest directory, or fail with a
/// message that says which side of the boundary went missing.
fn read_source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "tests/ipc.rs cannot read {}: {e}\n\
             This test only means something if it can read every side of the IPC boundary. \
             Restore the file or fix the path in tests/ipc.rs.",
            path.display()
        )
    })
}

fn lib_rs() -> String {
    read_source("src/lib.rs")
}

fn commands_rs() -> String {
    read_source("src/commands.rs")
}

fn app_test_js() -> String {
    read_source("../tests/App.test.js")
}

/// A chunk of frontend source that may hold `invoke(...)` calls: a `.vue`
/// script block, or a whole `.js`/`.ts` file.
#[derive(Debug)]
struct FrontendChunk {
    /// Path as a human reads it in the repo, for example `src/App.vue`.
    label: String,
    /// The scannable text.
    body: String,
    /// Lines that precede `body` in its file, so reported line numbers match.
    lines_above: usize,
}

/// Every `<script ...>` block of a `.vue` file, with the number of lines above
/// each one. A file with no script block yields nothing (it cannot invoke).
fn vue_scripts(src: &str, label: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = src[from..].find("<script") {
        let tag = from + rel;
        let Some(rel_gt) = src[tag..].find('>') else {
            panic!("tests/ipc.rs: a <script> tag in {label} is never closed.");
        };
        let body = tag + rel_gt + 1;
        let rel_end = src[body..].find("</script>").unwrap_or_else(|| {
            panic!("tests/ipc.rs: a <script> block in {label} is never closed.")
        });
        let end = body + rel_end;
        out.push((
            src[body..end].to_string(),
            src[..body].matches('\n').count(),
        ));
        from = end + "</script>".len();
    }
    out
}

/// Every scannable chunk of the desktop frontend, walked from `../src`.
///
/// Walking beats naming `App.vue`: the guard must not silently narrow to
/// nothing the day a call moves into a composable or a second component.
fn frontend_chunks() -> Vec<FrontendChunk> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src");
    let mut chunks = Vec::new();
    let mut files = 0usize;
    let mut stack = vec![root.clone()];

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).unwrap_or_else(|e| {
            panic!(
                "tests/ipc.rs cannot list {}: {e}\n\
                 The desktop frontend is one side of the IPC boundary; without it this file \
                 guards nothing.",
                dir.display()
            )
        });
        let mut paths: Vec<PathBuf> = entries
            .map(|entry| {
                entry
                    .unwrap_or_else(|e| {
                        panic!(
                            "tests/ipc.rs cannot read an entry of {}: {e}",
                            dir.display()
                        )
                    })
                    .path()
            })
            .collect();
        // Deterministic order, so two runs report the same file first.
        paths.sort();

        for path in paths {
            if path.is_dir() {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                // Nothing generated or vendored is part of this app's source.
                if matches!(name.as_str(), "node_modules" | "dist" | "target") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let extension = path
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !FRONTEND_EXTENSIONS.contains(&extension.as_str()) {
                continue;
            }
            files += 1;
            let label = label_of(&root, &path);
            let src = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("tests/ipc.rs cannot read {}: {e}", path.display()));
            if extension == "vue" {
                for (body, lines_above) in vue_scripts(&src, &label) {
                    chunks.push(FrontendChunk {
                        label: label.clone(),
                        body,
                        lines_above,
                    });
                }
            } else {
                chunks.push(FrontendChunk {
                    label,
                    body: src,
                    lines_above: 0,
                });
            }
        }
    }

    assert!(
        files > 0,
        "tests/ipc.rs walked {} and found no .vue/.js/.ts file. Either the frontend moved, or \
         this path is wrong; either way the IPC boundary is unguarded until it is fixed.",
        root.display()
    );
    chunks
}

/// `src/App.vue` and the like, for messages a human can act on.
fn label_of(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    format!("src/{}", relative.display())
}

// ------------------------------------------------------------------ scanning --

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Drop `//` and `/* */` comments while copying string literals verbatim.
///
/// Comment stripping has to be string-aware in both languages: App.vue's
/// script contains the literal `'... http://localhost:8000'`, and naive
/// stripping would eat half of it and then mis-parse everything after.
fn strip_comments(src: &str, quotes: &[char]) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if quotes.contains(&c) {
            out.push(c);
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                out.push(d);
                i += 1;
                if d == '\\' {
                    if i < chars.len() {
                        out.push(chars[i]);
                        i += 1;
                    }
                } else if d == c {
                    break;
                }
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            // Keep the newline so reported line numbers stay honest.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                if chars[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(chars.len());
            out.push(' ');
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Index of a whole-word occurrence of `word`, so `fn` does not match inside
/// an identifier and `invoke` does not match `reinvoke`.
fn find_word(chars: &[char], word: &str, from: usize) -> Option<usize> {
    let needle: Vec<char> = word.chars().collect();
    let mut i = from;
    while i + needle.len() <= chars.len() {
        if chars[i..i + needle.len()] == needle[..] {
            let before_ok = i == 0 || !is_ident_char(chars[i - 1]);
            let after = i + needle.len();
            let after_ok = after >= chars.len() || !is_ident_char(chars[after]);
            if before_ok && after_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Index of the delimiter matching the one at `open_idx`, skipping strings.
fn matching_delimiter(chars: &[char], open_idx: usize, quotes: &[char]) -> Option<usize> {
    let open = chars[open_idx];
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let mut depth = 0usize;
    let mut i = open_idx;
    while i < chars.len() {
        let c = chars[i];
        if quotes.contains(&c) {
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                i += 1;
                if d == '\\' {
                    i += 1;
                } else if d == c {
                    break;
                }
            }
            continue;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Split on commas that sit at nesting depth zero, so `Option<String>` stays
/// one parameter and a nested object literal stays one payload value. Trailing
/// commas produce no empty entry.
fn split_top_level(chars: &[char], quotes: &[char], count_angles: bool) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth: i32 = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if quotes.contains(&c) {
            cur.push(c);
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                cur.push(d);
                i += 1;
                if d == '\\' {
                    if i < chars.len() {
                        cur.push(chars[i]);
                        i += 1;
                    }
                } else if d == c {
                    break;
                }
            }
            continue;
        }
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            '<' if count_angles => {
                depth += 1;
                cur.push(c);
            }
            '>' if count_angles => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    parts.push(cur.trim().to_string());
    parts.retain(|p| !p.is_empty());
    parts
}

fn skip_ws(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

fn line_of(chars: &[char], idx: usize) -> usize {
    1 + chars[..idx.min(chars.len())]
        .iter()
        .filter(|c| **c == '\n')
        .count()
}

/// A type as written, minus whitespace, so `Option < String >` and
/// `Option<String>` compare equal.
fn normalized_type(ty: &str) -> String {
    ty.chars().filter(|c| !c.is_whitespace()).collect()
}

// ------------------------------------------------------------------- naming --

/// Rust parameter name to the payload key Tauri expects, mirroring the
/// camelCase the `#[tauri::command]` macro applies to argument names by
/// default. This is the authoritative direction: `output_dir` -> `outputDir`.
/// Valid only while no command carries `rename_all`, which is exactly what
/// `no_command_attribute_renames_the_wire_contract` pins.
fn snake_to_camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper_next = false;
    for c in name.chars() {
        if c == '_' {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The inverse, used only to phrase failures in terms of the Rust parameter a
/// stray payload key would need.
fn camel_to_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

// ------------------------------------------------------------------- parsing --

/// The command names listed inside `tauri::generate_handler![...]`, reduced to
/// their last path segment (`commands::is_directory` -> `is_directory`).
fn parse_handler_commands(src: &str) -> Vec<String> {
    let stripped = strip_comments(src, &RUST_QUOTES);
    let chars: Vec<char> = stripped.chars().collect();
    let macro_at = find_word(&chars, "generate_handler", 0).unwrap_or_else(|| {
        panic!(
            "tests/ipc.rs found no `generate_handler!` in src/lib.rs. Either the command \
             registration moved, or this parser needs updating; do not leave the IPC \
             boundary unguarded."
        )
    });
    let mut i = macro_at + "generate_handler".len();
    if chars.get(i) == Some(&'!') {
        i += 1;
    }
    i = skip_ws(&chars, i);
    let open = *chars.get(i).unwrap_or_else(|| {
        panic!("tests/ipc.rs: `generate_handler!` in src/lib.rs is not followed by a delimiter.")
    });
    assert!(
        matches!(open, '[' | '(' | '{'),
        "tests/ipc.rs: `generate_handler!` in src/lib.rs is followed by `{open}`, not a \
         delimiter this parser understands."
    );
    let close = matching_delimiter(&chars, i, &RUST_QUOTES).unwrap_or_else(|| {
        panic!("tests/ipc.rs: the `generate_handler!` list in src/lib.rs is never closed.")
    });

    split_top_level(&chars[i + 1..close], &RUST_QUOTES, true)
        .into_iter()
        .map(|entry| {
            entry
                .rsplit("::")
                .next()
                .unwrap_or(&entry)
                .trim()
                .to_string()
        })
        .collect()
}

/// A single `#[tauri::command]` function's parameters, in declaration order.
#[derive(Debug)]
struct RustParam {
    name: String,
    ty: String,
}

/// One `#[tauri::command]` function in `src/commands.rs`.
#[derive(Debug)]
struct RustCommand {
    /// The attribute's arguments: empty for the bare `#[tauri::command]`, or
    /// the text inside the parentheses (`rename_all = "snake_case"`, `async`,
    /// and so on). Read by `no_command_attribute_renames_the_wire_contract`,
    /// which is what keeps the rest of this file's naming model honest.
    attribute_args: String,
    params: Vec<RustParam>,
    /// Line of the attribute in src/commands.rs.
    line: usize,
}

/// Parameters Tauri fills in itself, so the webview never sends them and they
/// are not part of the payload contract. None exist in this app today; the
/// allowance is here so adding an `AppHandle` does not produce a failure that
/// says nothing.
///
/// Checked against tauri 2.11.5 (the version in Cargo.lock): each of these has
/// a `CommandArg` impl that reads the invoke message rather than the argument
/// key. `Channel<T>` looks like one and is not: `ipc/channel.rs` deserializes
/// a `String` out of the payload under the argument key, so the webview MUST
/// send it, and listing it here would hide a missing key in both directions.
fn injected_by_tauri(ty: &str) -> bool {
    let ty = ty.trim().trim_start_matches('&').trim();
    let ty = ty.rsplit("::").next().unwrap_or(ty);
    let head = ty.split(['<', ' ']).next().unwrap_or(ty);
    matches!(
        head,
        "AppHandle"
            | "Window"
            | "WebviewWindow"
            | "Webview"
            | "State"
            | "Request"
            | "CommandScope"
            | "GlobalScope"
    )
}

/// Signatures of every `#[tauri::command]` in `src/commands.rs`.
fn parse_rust_commands(src: &str) -> BTreeMap<String, RustCommand> {
    let stripped = strip_comments(src, &RUST_QUOTES);
    let chars: Vec<char> = stripped.chars().collect();
    let mut out = BTreeMap::new();

    let attribute: Vec<char> = "#[tauri::command".chars().collect();
    let mut i = 0;
    while i + attribute.len() <= chars.len() {
        if chars[i..i + attribute.len()] != attribute[..] {
            i += 1;
            continue;
        }
        // `#[tauri::commander]` and friends are not this attribute.
        if is_ident_char(chars.get(i + attribute.len()).copied().unwrap_or(' ')) {
            i += 1;
            continue;
        }
        let line = line_of(&chars, i);

        // Capture the whole attribute, arguments included: `rename_all` and
        // `rename` change the wire contract this file models, so the parser
        // has to at least see them.
        let attribute_end = matching_delimiter(&chars, i + 1, &RUST_QUOTES).unwrap_or_else(|| {
            panic!(
                "tests/ipc.rs: the `#[tauri::command` at line {line} of src/commands.rs is \
                 never closed."
            )
        });
        let inside: String = chars[i + 2..attribute_end].iter().collect();
        let tail = inside
            .strip_prefix("tauri::command")
            .unwrap_or_default()
            .trim()
            .to_string();
        let attribute_args = if tail.is_empty() {
            String::new()
        } else {
            tail.strip_prefix('(')
                .and_then(|t| t.strip_suffix(')'))
                .unwrap_or_else(|| {
                    panic!(
                        "tests/ipc.rs: cannot read the arguments of `#[tauri::command{tail}]` at \
                         line {line} of src/commands.rs. This parser has to read them: \
                         `rename_all` and `rename` silently change the names the webview must \
                         send."
                    )
                })
                .trim()
                .to_string()
        };

        let fn_at = find_word(&chars, "fn", attribute_end).unwrap_or_else(|| {
            panic!(
                "tests/ipc.rs: a `#[tauri::command]` in src/commands.rs (line {line}) is not \
                 followed by a function."
            )
        });
        let name_start = skip_ws(&chars, fn_at + 2);
        let mut name_end = name_start;
        while name_end < chars.len() && is_ident_char(chars[name_end]) {
            name_end += 1;
        }
        let name: String = chars[name_start..name_end].iter().collect();
        assert!(
            !name.is_empty(),
            "tests/ipc.rs: could not read the name of the command at line {} of \
             src/commands.rs.",
            line_of(&chars, fn_at)
        );

        let paren = skip_ws(&chars, name_end);
        assert_eq!(
            chars.get(paren),
            Some(&'('),
            "tests/ipc.rs: the command `{name}` in src/commands.rs has no parameter list \
             this parser can read (generic commands are not supported here)."
        );
        let paren_end = matching_delimiter(&chars, paren, &RUST_QUOTES).unwrap_or_else(|| {
            panic!(
                "tests/ipc.rs: the parameter list of `{name}` in src/commands.rs is never closed."
            )
        });

        let params = split_top_level(&chars[paren + 1..paren_end], &RUST_QUOTES, true)
            .into_iter()
            .map(|param| {
                let (raw_name, ty) = param.split_once(':').unwrap_or_else(|| {
                    panic!(
                        "tests/ipc.rs: cannot read the parameter `{param}` of `{name}` in \
                         src/commands.rs (expected `name: Type`)."
                    )
                });
                RustParam {
                    name: raw_name
                        .trim()
                        .trim_start_matches("mut ")
                        .trim()
                        .to_string(),
                    ty: ty.trim().to_string(),
                }
            })
            .collect();

        out.insert(
            name,
            RustCommand {
                attribute_args,
                params,
                line,
            },
        );
        i = paren_end;
    }
    out
}

/// One `invoke('name', { ... })` call site in the frontend.
#[derive(Debug)]
struct Invocation {
    command: String,
    keys: Vec<String>,
    file: String,
    line: usize,
}

/// Every `invoke(...)` call in one chunk of frontend source, with the payload
/// keys each one supplies. Shorthand properties (`{ path }`) and quoted keys
/// are both understood; a payload that is not an object literal is a hard
/// failure, because this test could not verify it and must not pretend
/// otherwise.
fn parse_invocations(chunk: &FrontendChunk) -> Vec<Invocation> {
    let stripped = strip_comments(&chunk.body, &JS_QUOTES);
    let chars: Vec<char> = stripped.chars().collect();
    let file = chunk.label.clone();
    let mut out = Vec::new();

    let mut from = 0;
    while let Some(at) = find_word(&chars, "invoke", from) {
        from = at + "invoke".len();
        // `import { invoke } ...` and any member access are not call sites.
        if at > 0 && chars[at - 1] == '.' {
            continue;
        }
        let mut i = skip_ws(&chars, from);
        if chars.get(i) != Some(&'(') {
            continue;
        }
        let line = chunk.lines_above + line_of(&chars, at);
        let call_end = matching_delimiter(&chars, i, &JS_QUOTES).unwrap_or_else(|| {
            panic!("tests/ipc.rs: an `invoke(` call at {file} line {line} is never closed.")
        });

        i = skip_ws(&chars, i + 1);
        let quote = *chars.get(i).unwrap_or(&' ');
        assert!(
            JS_QUOTES.contains(&quote),
            "tests/ipc.rs: the `invoke(` at {file} line {line} is not called with a literal \
             command name. Keep the name a literal so this lockstep test can see it."
        );
        i += 1;
        let name_start = i;
        while i < chars.len() && chars[i] != quote {
            if chars[i] == '\\' {
                i += 1;
            }
            i += 1;
        }
        let command: String = chars[name_start..i].iter().collect();
        i = skip_ws(&chars, i + 1);

        let mut keys = Vec::new();
        if chars.get(i) == Some(&',') {
            i = skip_ws(&chars, i + 1);
            if i < call_end {
                assert_eq!(
                    chars.get(i),
                    Some(&'{'),
                    "tests/ipc.rs: `invoke('{command}')` at {file} line {line} passes a payload \
                     that is not an object literal, so the argument names cannot be checked. \
                     Pass a literal, or this guard is blind."
                );
                let obj_end = matching_delimiter(&chars, i, &JS_QUOTES).unwrap_or_else(|| {
                    panic!(
                        "tests/ipc.rs: the payload of `invoke('{command}')` at {file} line \
                         {line} is never closed."
                    )
                });
                keys = parse_object_keys(&chars[i + 1..obj_end], &command, &file, line);
            }
        }

        out.push(Invocation {
            command,
            keys,
            file: file.clone(),
            line,
        });
        from = call_end;
    }
    out
}

/// Every `invoke(...)` call in the whole desktop frontend.
fn all_invocations() -> Vec<Invocation> {
    frontend_chunks()
        .iter()
        .flat_map(parse_invocations)
        .collect()
}

fn parse_object_keys(inner: &[char], command: &str, file: &str, line: usize) -> Vec<String> {
    split_top_level(inner, &JS_QUOTES, false)
        .into_iter()
        .map(|entry| {
            assert!(
                !entry.starts_with("..."),
                "tests/ipc.rs: the payload of `invoke('{command}')` at {file} line {line} \
                 spreads `{entry}`, so its argument names cannot be checked. Spell the keys \
                 out, or this guard is blind."
            );
            // `key: value` or the shorthand `key`.
            let key = match entry.find(':') {
                Some(at) => entry[..at].trim().to_string(),
                None => entry.trim().to_string(),
            };
            let key = key
                .trim_matches(|c| c == '\'' || c == '"' || c == '`')
                .to_string();
            assert!(
                !key.is_empty() && key.chars().all(is_ident_char),
                "tests/ipc.rs: `{key}` in the payload of `invoke('{command}')` at {file} line \
                 {line} is not a plain identifier key, so it cannot be matched to a Rust \
                 parameter."
            );
            key
        })
        .collect()
}

/// The command names the Vitest stub switch answers, from lines shaped like
/// `if (cmd === 'is_directory')`.
fn parse_stub_commands(src: &str) -> BTreeSet<String> {
    let stripped = strip_comments(src, &JS_QUOTES);
    let chars: Vec<char> = stripped.chars().collect();
    let mut out = BTreeSet::new();

    let mut from = 0;
    while let Some(at) = find_word(&chars, "cmd", from) {
        from = at + 3;
        let mut i = skip_ws(&chars, from);
        // Accept `===` and `==`; matching the operands the other way round is
        // not needed, the repo writes `cmd === 'name'`.
        if chars.get(i) != Some(&'=') {
            continue;
        }
        while chars.get(i) == Some(&'=') {
            i += 1;
        }
        i = skip_ws(&chars, i);
        let quote = *chars.get(i).unwrap_or(&' ');
        if !JS_QUOTES.contains(&quote) {
            continue;
        }
        i += 1;
        let start = i;
        while i < chars.len() && chars[i] != quote {
            i += 1;
        }
        out.insert(chars[start..i].iter().collect());
        from = i;
    }
    out
}

// ------------------------------------------------------------------- sanity --

#[test]
fn the_parsers_find_the_commands_this_app_ships() {
    // A lockstep test that silently parses nothing passes every other check in
    // this file vacuously. This canary is the only thing standing between a
    // broken parser and a green, useless suite.
    let handler: BTreeSet<String> = parse_handler_commands(&lib_rs()).into_iter().collect();
    let baseline: BTreeSet<String> = BASELINE.iter().map(|s| s.to_string()).collect();
    // Equality here, containment below: a command that never enters BASELINE
    // would be exempt from this canary, so adding one has to be deliberate.
    assert_eq!(
        handler, baseline,
        "the commands registered in `tauri::generate_handler![...]` (src/lib.rs) are no longer \
         the ones BASELINE in tests/ipc.rs names.\n\
         If a command was added, add it to BASELINE so the parsers in this file are proved to \
         see it; if one was removed, remove it here too."
    );

    let chunks = frontend_chunks();
    let scanned: Vec<&str> = chunks.iter().map(|c| c.label.as_str()).collect();
    assert!(
        !scanned.is_empty(),
        "tests/ipc.rs found no frontend source to scan for `invoke(...)` calls."
    );

    let rust: BTreeSet<String> = parse_rust_commands(&commands_rs()).into_keys().collect();
    let invoked: BTreeSet<String> = chunks
        .iter()
        .flat_map(parse_invocations)
        .map(|i| i.command)
        .collect();
    let stubs = parse_stub_commands(&app_test_js());

    for (label, found) in [
        ("#[tauri::command] fns in src/commands.rs", rust),
        ("invoke('...') across the frontend", invoked),
        ("the stub switch in tests/App.test.js", stubs),
    ] {
        for expected in BASELINE {
            assert!(
                found.contains(expected),
                "tests/ipc.rs parsed {label} and did not find `{expected}`.\n\
                 Found: {found:?}\n\
                 Frontend sources scanned: {scanned:?}\n\
                 Either the parser in tests/ipc.rs broke on a formatting change, or the \
                 command really was removed everywhere; in that second case update BASELINE \
                 in tests/ipc.rs deliberately."
            );
        }
    }
}

#[test]
fn no_command_attribute_renames_the_wire_contract() {
    // Everything else in this file assumes the defaults of `#[tauri::command]`:
    // the wire name of a command is its function name, and its argument keys
    // are the camelCase of its parameter names. Both are one attribute argument
    // away from being false (tauri-macros 2.6.3 accepts `rename`, `rename_all`,
    // `root` and `async`), and either change would ship a broken app with every
    // other test here green. So pin the assumption rather than trust it.
    for (name, command) in parse_rust_commands(&commands_rs()) {
        let args = command.attribute_args.trim().to_string();
        if args.is_empty() {
            continue;
        }
        let chars: Vec<char> = args.chars().collect();
        assert!(
            find_word(&chars, "rename_all", 0).is_none(),
            "`{name}` in src/commands.rs carries `#[tauri::command({args})]` (line {}).\n\
             `rename_all` changes the argument keys the webview has to send, and tests/ipc.rs \
             models them as the camelCase of the Rust parameter names, so every payload check \
             in this file is now wrong. Drop the argument, or teach `snake_to_camel` and its \
             call sites the new casing.",
            command.line
        );
        assert!(
            find_word(&chars, "rename", 0).is_none(),
            "`{name}` in src/commands.rs carries `#[tauri::command({args})]` (line {}).\n\
             `rename` changes the name the webview has to invoke, while tests/ipc.rs matches \
             the function name, so every registration check in this file is now comparing the \
             wrong string. Drop the argument, or teach this file to read the renamed literal.",
            command.line
        );
        for argument in split_top_level(&chars, &RUST_QUOTES, false) {
            let head: String = argument.chars().take_while(|c| is_ident_char(*c)).collect();
            assert!(
                matches!(head.as_str(), "async" | "root"),
                "`{name}` in src/commands.rs carries the unknown attribute argument \
                 `{argument}` (line {}).\n\
                 tests/ipc.rs only knows that `async` and `root` leave the wire contract alone. \
                 Work out what this one does to the command name and to the argument keys, \
                 then either allow it here or fix the model in this file.",
                command.line
            );
        }
    }
}

#[test]
fn every_command_that_can_block_is_marked_async() {
    // The only place this can be pinned. A `#[tauri::command]` is an ordinary
    // function, so every other test in this crate calls these directly and gets
    // the identical result whatever the attribute says: the argument changes
    // how TAURI invokes them, and nothing else.
    //
    // What it changes is which thread runs the body. Bare, tauri-macros 2.6.3
    // compiles a synchronous command to its `sync` path, which runs inline on
    // the thread handling the IPC message, so the window stops repainting until
    // the call returns. `async` moves it to `sync_threadpool`, off that thread.
    //
    // Measured before this was fixed: `check_server` against an unroutable
    // address froze the window for the whole of ureq's 30 second connect
    // timeout, and a compression froze it for as long as the compression took.
    const MUST_NOT_BLOCK: [(&str, &str); 4] = [
        (
            "compress_path",
            "compresses a whole tree, or waits on a server with no read timeout",
        ),
        ("extract_archive", "unpacks a whole archive"),
        (
            "unwritable_names",
            "reads the listing of a whole archive, which for a tar means walking every header",
        ),
        (
            "check_server",
            "waits out a connect timeout when the address is wrong",
        ),
    ];
    // The exception, listed rather than merely absent so that adding a command
    // here is a decision someone made on purpose.
    const MAY_BLOCK: [(&str, &str); 1] = [(
        "is_directory",
        "one stat, called while the user is still choosing",
    )];

    let commands = parse_rust_commands(&commands_rs());
    for (name, why) in MUST_NOT_BLOCK {
        let command = commands
            .get(name)
            .unwrap_or_else(|| panic!("`{name}` is gone from src/commands.rs: update this list"));
        let args: Vec<char> = command.attribute_args.chars().collect();
        assert!(
            find_word(&args, "async", 0).is_some(),
            "`{name}` (src/commands.rs line {}) has no `async` in its attribute, and it {why}.\n\
             Without it Tauri runs the body on the thread handling the IPC message and the \
             window freezes for the whole call. Write `#[tauri::command(async)]`, or, if this \
             command genuinely cannot block any more, move it to MAY_BLOCK with the reason.",
            command.line
        );
    }
    for (name, why) in MAY_BLOCK {
        let command = commands
            .get(name)
            .unwrap_or_else(|| panic!("`{name}` is gone from src/commands.rs: update this list"));
        let args: Vec<char> = command.attribute_args.chars().collect();
        assert!(
            find_word(&args, "async", 0).is_none(),
            "`{name}` (src/commands.rs line {}) is marked `async`, but it is listed here as \
             cheap enough not to need it ({why}).\n\
             Marking it is not harmful, it just costs a round trip through the runtime for a \
             call the UI makes while the user is still choosing. If it now does real work, \
             move it to MUST_NOT_BLOCK.",
            command.line
        );
    }
    // Nothing may sit in neither list: a new command has to be classified.
    let listed: BTreeSet<&str> = MUST_NOT_BLOCK
        .iter()
        .chain(MAY_BLOCK.iter())
        .map(|(name, _)| *name)
        .collect();
    let found: BTreeSet<&str> = commands.keys().map(|k| k.as_str()).collect();
    assert_eq!(
        found, listed,
        "src/commands.rs and this test disagree about which commands exist. Every command has \
         to be in MUST_NOT_BLOCK or in MAY_BLOCK, so that whether it can freeze the window is \
         something someone decided rather than something nobody noticed."
    );
}

// -------------------------------------------------------------- registration --

#[test]
fn every_invoked_command_is_registered_in_generate_handler() {
    let handler: BTreeSet<String> = parse_handler_commands(&lib_rs()).into_iter().collect();
    for call in all_invocations() {
        assert!(
            handler.contains(&call.command),
            "{} line {} calls invoke('{}'), which is NOT in \
             `tauri::generate_handler![...]` in src/lib.rs.\n\
             Nothing type-checks this crossing: the app compiles and the call fails at \
             runtime with \"command {} not found\". Fix it by adding `commands::{}` to the \
             handler list, or by correcting the string in the frontend.",
            call.file,
            call.line,
            call.command,
            call.command,
            call.command
        );
    }
}

#[test]
fn every_registered_command_is_invoked_by_the_frontend() {
    // Not symmetry for its own sake: the reference implementation shipped a
    // registered-but-unused `extract_file` command, dead surface nobody
    // noticed because nothing checks this direction.
    let invoked: BTreeSet<String> = all_invocations().into_iter().map(|c| c.command).collect();
    for command in parse_handler_commands(&lib_rs()) {
        assert!(
            invoked.contains(&command),
            "`{command}` is registered in `tauri::generate_handler![...]` in src/lib.rs but \
             never invoked anywhere under apps/desktop/src.\n\
             Either the frontend call was renamed or dropped (fix the caller), or the command \
             is dead surface (drop it from the handler list and from src/commands.rs). The \
             reference implementation carried exactly this defect."
        );
    }
}

#[test]
fn every_registered_command_exists_in_commands_rs() {
    // The handler list names paths, not functions the compiler resolves for
    // this test, so pin that each entry really is a `#[tauri::command]`.
    let defined = parse_rust_commands(&commands_rs());
    for command in parse_handler_commands(&lib_rs()) {
        assert!(
            defined.contains_key(&command),
            "`{command}` is listed in `tauri::generate_handler![...]` in src/lib.rs but no \
             `#[tauri::command]` by that name exists in src/commands.rs.\n\
             Known commands: {:?}",
            defined.keys().collect::<Vec<_>>()
        );
    }
}

#[test]
fn every_command_in_commands_rs_is_registered() {
    // The converse of the test above, and the one that catches dead surface at
    // its source: a `#[tauri::command]` nobody registered reads like part of
    // the IPC contract, is documented as such, and can never be called. The
    // reference implementation's unused `extract_file` is this same defect
    // seen from the handler side.
    let handler: BTreeSet<String> = parse_handler_commands(&lib_rs()).into_iter().collect();
    for (command, defined) in parse_rust_commands(&commands_rs()) {
        assert!(
            handler.contains(&command),
            "`{command}` is a `#[tauri::command]` in src/commands.rs (line {}) that no \
             `tauri::generate_handler![...]` in src/lib.rs registers.\n\
             The webview cannot reach it: invoking it fails with \"command {command} not \
             found\". Register it, or delete it.",
            defined.line
        );
    }
}

#[test]
fn every_command_is_covered_by_the_vitest_stub_switch() {
    // The Vitest mock in apps/desktop/tests/App.test.js is a switch on the
    // command name that ends in a bare `return null`, so an unstubbed command
    // does not fail the JS run: it resolves to null. What that costs depends on
    // the command, and it was measured by deleting each branch and re-running
    // the Vitest suite. Three branches are load-bearing: dropping
    // `compress_path`, `extract_archive` or `unwritable_names` turns a case
    // red, because each is read for something the component then renders (the
    // last one is read for `entries.length`, so a null throws where a stub
    // would have answered). Two are cosmetic: `is_directory` returns false and
    // `check_server` returns null, and with either branch gone the suite still
    // passes, since no assertion can tell those values from the fallthrough
    // null.
    //
    // So this test does not claim every branch is load-bearing. It keeps the
    // switch in lockstep with the handler list, so a command added on the Rust
    // side starts life with a deliberate stub instead of a silent null.
    let stubs = parse_stub_commands(&app_test_js());
    for command in parse_handler_commands(&lib_rs()) {
        assert!(
            stubs.contains(&command),
            "`{command}` is registered in src/lib.rs but the stub switch in \
             apps/desktop/tests/App.test.js has no `if (cmd === '{command}')` branch.\n\
             The mock ends in a bare `return null`, so a Vitest case that reaches this command \
             gets null instead of an answer the component can use, and may pass while \
             exercising nothing. Add the branch, returning what the real command returns."
        );
    }
}

// ------------------------------------------------------------------ payloads --

#[test]
fn every_invoke_payload_key_binds_to_a_rust_parameter() {
    // Names only. Nothing here compares a payload value against the Rust type
    // it has to deserialize into, because the frontend sends expressions
    // (`level: level.value`) rather than literals this parser could type: a
    // frontend that starts sending `String(level.value)` for a `u32` is still
    // a runtime-only failure. `command_signatures_are_pinned_with_their_types`
    // freezes the Rust half of that gap.
    let commands = parse_rust_commands(&commands_rs());
    for call in all_invocations() {
        let Some(command) = commands.get(&call.command) else {
            continue; // covered by every_invoked_command_is_registered_in_generate_handler
        };
        let expected: BTreeSet<String> = command
            .params
            .iter()
            .filter(|p| !injected_by_tauri(&p.ty))
            .map(|p| snake_to_camel(&p.name))
            .collect();

        for key in &call.keys {
            assert!(
                expected.contains(key),
                "{} line {} sends `{key}` to invoke('{}'), but `{}` in src/commands.rs has no \
                 such parameter.\n\
                 Tauri camelCases the Rust parameter names, so `{key}` would need a Rust \
                 parameter called `{}`. Parameters it does have (as the webview must spell \
                 them): {:?}\n\
                 Nothing type-checks this crossing: an unknown key makes the call fail at \
                 runtime only. Fix the key in the frontend or the signature in \
                 src/commands.rs.",
                call.file,
                call.line,
                call.command,
                call.command,
                camel_to_snake(key),
                expected
            );
        }
    }
}

#[test]
fn every_rust_parameter_is_supplied_by_the_invoke_payload() {
    // Names only, for the same reason as the test above.
    let commands = parse_rust_commands(&commands_rs());
    for call in all_invocations() {
        let Some(command) = commands.get(&call.command) else {
            continue;
        };
        let supplied: BTreeSet<&String> = call.keys.iter().collect();
        for param in command.params.iter().filter(|p| !injected_by_tauri(&p.ty)) {
            let key = snake_to_camel(&param.name);
            assert!(
                supplied.contains(&key),
                "`{}` in src/commands.rs takes `{}: {}`, but the invoke('{}') call at {} line \
                 {} does not send `{key}`. It sends: {:?}\n\
                 An `Option<..>` left unsupplied quietly deserializes to None (a remote \
                 compression would silently run locally), and a required one fails the call \
                 at runtime. Add the key to the frontend, or drop the parameter in \
                 src/commands.rs.",
                call.command,
                param.name,
                param.ty,
                call.command,
                call.file,
                call.line,
                call.keys
            );
        }
    }
}

#[test]
fn command_signatures_are_pinned_with_their_types() {
    // The payload checks above match names; serde matches names AND types, and
    // rejects the call either way. `level: u32` becoming `level: String`, or
    // `server: Option<String>` becoming `server: String` (which turns every
    // local compression into a failed call, since the frontend sends null for
    // it), is invisible to every other test in this file. So freeze the
    // declared signatures and make a change to them deliberate.
    //
    // Sets, not sequences: Tauri binds arguments by key, so reordering two
    // parameters changes nothing on the wire and must not fail here.
    let expected: [(&str, &[(&str, &str)]); 5] = [
        ("check_server", &[("url", "String")]),
        (
            "compress_path",
            &[
                ("path", "String"),
                ("output", "String"),
                ("format", "String"),
                ("level", "u32"),
                ("server", "Option<String>"),
                ("overwrite", "bool"),
                ("verify", "bool"),
            ],
        ),
        (
            "extract_archive",
            &[
                ("archive", "String"),
                ("output_dir", "String"),
                // The user's answers for the entry names this host cannot
                // write, one character to what it becomes. A `HashMap` here
                // would deserialize identically and report two bad keys in a
                // different order on every run.
                ("replacements", "BTreeMap<String, String>"),
            ],
        ),
        ("is_directory", &[("path", "String")]),
        ("unwritable_names", &[("archive", "String")]),
    ];

    // One list of commands, not two: BASELINE decides what ships, this table
    // has to keep up with it.
    assert_eq!(
        expected
            .iter()
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>(),
        BASELINE.iter().copied().collect::<BTreeSet<_>>(),
        "the signature table in tests/ipc.rs no longer covers the same commands as BASELINE. \
         A new command needs its signature pinned here too, or it crosses the boundary \
         untyped."
    );

    let commands = parse_rust_commands(&commands_rs());
    for (name, params) in expected {
        let command = commands.get(name).unwrap_or_else(|| {
            panic!(
                "src/commands.rs no longer defines a `#[tauri::command]` called `{name}`. It \
                 defines: {:?}",
                commands.keys().collect::<Vec<_>>()
            )
        });
        let found: BTreeSet<(String, String)> = command
            .params
            .iter()
            .map(|p| (p.name.clone(), normalized_type(&p.ty)))
            .collect();
        let want: BTreeSet<(String, String)> = params
            .iter()
            .map(|(n, t)| (n.to_string(), normalized_type(t)))
            .collect();
        assert_eq!(
            found, want,
            "the signature of `{name}` in src/commands.rs changed.\n\
             The frontend builds this payload by hand and nothing type-checks it: serde \
             rejects a value of the wrong type exactly as it rejects an unknown key, at \
             runtime, in front of the user. If the change is intended, update this table and \
             the invoke call that feeds it."
        );
    }
}
