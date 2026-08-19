//! Lockstep guard for the JS/Rust IPC boundary.
//!
//! Three places describe the same set of commands and **nothing type-checks
//! the crossing**, so a rename compiles, ships, and only breaks when a user
//! clicks the button:
//!
//!   1. `src/lib.rs`            -> `tauri::generate_handler![...]`
//!   2. `../src/App.vue`        -> the `invoke('...')` string literals
//!   3. `../tests/App.test.js`  -> the `if (cmd === '...')` stub switch
//!
//! Argument names cross camelCase on the JS side and snake_case on the Rust
//! side (Tauri derives the argument struct with serde's `rename_all =
//! "camelCase"`), so App.vue's `outputDir` binds to `extract_archive`'s
//! `output_dir` parameter. Pinning that mapping is the main point of this
//! file.
//!
//! The tests read the real source files at run time and parse them, rather
//! than restating the command list, so they fail on a genuine rename and not
//! on a reformat. Everything the parsers depend on is ordinary syntax
//! (whitespace, line breaks, either quote style, trailing commas are all
//! tolerated), and any file that cannot be read or parsed panics loudly: a
//! lockstep test that quietly finds zero commands is worse than no test at
//! all, which is why `the_parsers_find_the_commands_this_app_ships` exists.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

/// The commands this app has shipped since the remote-compression work
/// landed. Purely a canary for the parsers below: if a parse silently starts
/// returning nothing, every other test in this file would pass vacuously.
/// Deliberately removing a command means updating this list on purpose.
const BASELINE: [&str; 4] = [
    "check_server",
    "compress_path",
    "extract_archive",
    "is_directory",
];

/// Quote characters that open a string literal, per language. Needed by every
/// scanner here so a `//` inside `'http://localhost:8000'` is not mistaken for
/// a comment, and a `,` inside a string does not split a list.
const JS_QUOTES: [char; 3] = ['\'', '"', '`'];
const RUST_QUOTES: [char; 1] = ['"'];

// ------------------------------------------------------------------ reading --

/// Read a file relative to this crate's manifest directory, or fail with a
/// message that says which side of the boundary went missing.
fn read_source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "tests/ipc.rs cannot read {}: {e}\n\
             This test only means something if it can read all three sides of the IPC \
             boundary. Restore the file or fix the path in tests/ipc.rs.",
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

fn app_vue() -> String {
    read_source("../src/App.vue")
}

fn app_test_js() -> String {
    read_source("../tests/App.test.js")
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

// ------------------------------------------------------------------- naming --

/// Rust parameter name to the payload key Tauri expects, mirroring the serde
/// `rename_all = "camelCase"` Tauri puts on the generated argument struct.
/// This is the authoritative direction: `output_dir` -> `outputDir`.
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

/// Parameters Tauri injects itself. The webview never sends these, so they are
/// not part of the payload contract. None exist today; the allowance is here
/// so adding an `AppHandle` does not produce a failure that says nothing.
fn injected_by_tauri(ty: &str) -> bool {
    let ty = ty.trim().trim_start_matches('&').trim();
    let ty = ty.rsplit("::").next().unwrap_or(ty);
    let head = ty.split(['<', ' ']).next().unwrap_or(ty);
    matches!(
        head,
        "AppHandle" | "Window" | "WebviewWindow" | "Webview" | "State" | "Request" | "Channel"
    )
}

/// Signatures of every `#[tauri::command]` in `src/commands.rs`.
fn parse_rust_commands(src: &str) -> BTreeMap<String, Vec<RustParam>> {
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
        let fn_at = find_word(&chars, "fn", i).unwrap_or_else(|| {
            panic!(
                "tests/ipc.rs: a `#[tauri::command]` in src/commands.rs (line {}) is not \
                 followed by a function.",
                line_of(&chars, i)
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
            panic!("tests/ipc.rs: the parameter list of `{name}` in src/commands.rs is never closed.")
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

        out.insert(name, params);
        i = paren_end;
    }
    out
}

/// One `invoke('name', { ... })` call site in App.vue.
#[derive(Debug)]
struct Invocation {
    command: String,
    keys: Vec<String>,
    line: usize,
}

/// The `<script setup>` body of a `.vue` file, plus the number of lines above
/// it, so reported line numbers match the real file.
fn vue_script(src: &str) -> (String, usize) {
    let tag = src
        .find("<script")
        .expect("tests/ipc.rs: App.vue has no <script> block.");
    let body = src[tag..]
        .find('>')
        .map(|k| tag + k + 1)
        .expect("tests/ipc.rs: the <script> tag in App.vue is never closed.");
    let end = src[body..]
        .find("</script>")
        .map(|k| body + k)
        .expect("tests/ipc.rs: the <script> block in App.vue is never closed.");
    let lines_above = src[..body].matches('\n').count();
    (src[body..end].to_string(), lines_above)
}

/// Every `invoke(...)` call in App.vue's script, with the payload keys each
/// one supplies. Shorthand properties (`{ path }`) and quoted keys are both
/// understood; a payload that is not an object literal is a hard failure,
/// because this test could not verify it and must not pretend otherwise.
fn parse_invocations(src: &str) -> Vec<Invocation> {
    let (script, lines_above) = vue_script(src);
    let stripped = strip_comments(&script, &JS_QUOTES);
    let chars: Vec<char> = stripped.chars().collect();
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
        let line = lines_above + line_of(&chars, at);
        let call_end = matching_delimiter(&chars, i, &JS_QUOTES).unwrap_or_else(|| {
            panic!("tests/ipc.rs: an `invoke(` call at App.vue line {line} is never closed.")
        });

        i = skip_ws(&chars, i + 1);
        let quote = *chars.get(i).unwrap_or(&' ');
        assert!(
            JS_QUOTES.contains(&quote),
            "tests/ipc.rs: the `invoke(` at App.vue line {line} is not called with a literal \
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
                    "tests/ipc.rs: `invoke('{command}')` at App.vue line {line} passes a payload \
                     that is not an object literal, so the argument names cannot be checked. \
                     Pass a literal, or this guard is blind."
                );
                let obj_end = matching_delimiter(&chars, i, &JS_QUOTES).unwrap_or_else(|| {
                    panic!(
                        "tests/ipc.rs: the payload of `invoke('{command}')` at App.vue line \
                         {line} is never closed."
                    )
                });
                keys = parse_object_keys(&chars[i + 1..obj_end], &command, line);
            }
        }

        out.push(Invocation {
            command,
            keys,
            line,
        });
        from = call_end;
    }
    out
}

fn parse_object_keys(inner: &[char], command: &str, line: usize) -> Vec<String> {
    split_top_level(inner, &JS_QUOTES, false)
        .into_iter()
        .map(|entry| {
            assert!(
                !entry.starts_with("..."),
                "tests/ipc.rs: the payload of `invoke('{command}')` at App.vue line {line} \
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
                "tests/ipc.rs: `{key}` in the payload of `invoke('{command}')` at App.vue line \
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
        // Accept `===` and `==`, in either order of operands is not needed:
        // the repo writes `cmd === 'name'`.
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
    let handler = parse_handler_commands(&lib_rs());
    let rust: BTreeSet<String> = parse_rust_commands(&commands_rs()).into_keys().collect();
    let invoked: BTreeSet<String> = parse_invocations(&app_vue())
        .into_iter()
        .map(|i| i.command)
        .collect();
    let stubs = parse_stub_commands(&app_test_js());

    for (label, found) in [
        ("generate_handler! in src/lib.rs", handler.iter().cloned().collect::<BTreeSet<_>>()),
        ("#[tauri::command] fns in src/commands.rs", rust),
        ("invoke('...') in src/App.vue", invoked),
        ("the stub switch in tests/App.test.js", stubs),
    ] {
        for expected in BASELINE {
            assert!(
                found.contains(expected),
                "tests/ipc.rs parsed {label} and did not find `{expected}`.\n\
                 Found: {found:?}\n\
                 Either the parser in tests/ipc.rs broke on a formatting change, or the \
                 command really was removed everywhere; in that second case update BASELINE \
                 in tests/ipc.rs deliberately."
            );
        }
    }
}

// -------------------------------------------------------------- registration --

#[test]
fn every_invoked_command_is_registered_in_generate_handler() {
    let handler: BTreeSet<String> = parse_handler_commands(&lib_rs()).into_iter().collect();
    for call in parse_invocations(&app_vue()) {
        assert!(
            handler.contains(&call.command),
            "App.vue line {} calls invoke('{}'), which is NOT in \
             `tauri::generate_handler![...]` in src/lib.rs.\n\
             Nothing type-checks this crossing: the app compiles and the call fails at \
             runtime with \"command {} not found\". Fix it by adding \
             `commands::{}` to the handler list, or by correcting the string in App.vue.",
            call.line, call.command, call.command, call.command
        );
    }
}

#[test]
fn every_registered_command_is_invoked_by_app_vue() {
    // Not symmetry for its own sake: the reference implementation shipped a
    // registered-but-unused `extract_file` command, dead surface nobody
    // noticed because nothing checks this direction.
    let invoked: BTreeSet<String> = parse_invocations(&app_vue())
        .into_iter()
        .map(|c| c.command)
        .collect();
    for command in parse_handler_commands(&lib_rs()) {
        assert!(
            invoked.contains(&command),
            "`{command}` is registered in `tauri::generate_handler![...]` in src/lib.rs but \
             never invoked from src/App.vue.\n\
             Either the frontend call was renamed or dropped (fix App.vue), or the command \
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
fn every_command_is_covered_by_the_vitest_stub_switch() {
    // tests/App.test.js mocks invoke with a switch that falls through to
    // `return null`. An uncovered command therefore does not fail the Vitest
    // run, it silently resolves to null and the assertions around it lose all
    // meaning.
    let stubs = parse_stub_commands(&app_test_js());
    for command in parse_handler_commands(&lib_rs()) {
        assert!(
            stubs.contains(&command),
            "`{command}` is registered in src/lib.rs but the stub switch in \
             apps/desktop/tests/App.test.js has no `if (cmd === '{command}')` branch.\n\
             The mock falls through to `return null`, so a Vitest run would exercise this \
             command against a stub that knows nothing about it and still pass. Add the \
             branch to tests/App.test.js."
        );
    }
}

// ------------------------------------------------------------------ payloads --

#[test]
fn every_invoke_payload_key_binds_to_a_rust_parameter() {
    let commands = parse_rust_commands(&commands_rs());
    for call in parse_invocations(&app_vue()) {
        let Some(params) = commands.get(&call.command) else {
            continue; // covered by every_invoked_command_is_registered_in_generate_handler
        };
        let expected: BTreeSet<String> = params
            .iter()
            .filter(|p| !injected_by_tauri(&p.ty))
            .map(|p| snake_to_camel(&p.name))
            .collect();

        for key in &call.keys {
            assert!(
                expected.contains(key),
                "App.vue line {} sends `{key}` to invoke('{}'), but `{}` in \
                 src/commands.rs has no such parameter.\n\
                 Tauri camelCases the Rust parameter names, so `{key}` would need a Rust \
                 parameter called `{}`. Parameters it does have (as the webview must spell \
                 them): {:?}\n\
                 Nothing type-checks this crossing: an unknown key makes the call fail at \
                 runtime only. Fix the key in App.vue or the signature in src/commands.rs.",
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
    let commands = parse_rust_commands(&commands_rs());
    for call in parse_invocations(&app_vue()) {
        let Some(params) = commands.get(&call.command) else {
            continue;
        };
        let supplied: BTreeSet<&String> = call.keys.iter().collect();
        for param in params.iter().filter(|p| !injected_by_tauri(&p.ty)) {
            let key = snake_to_camel(&param.name);
            assert!(
                supplied.contains(&key),
                "`{}` in src/commands.rs takes `{}: {}`, but the invoke('{}') call at \
                 App.vue line {} does not send `{key}`. It sends: {:?}\n\
                 An `Option<..>` left unsupplied quietly deserializes to None (a remote \
                 compression would silently run locally), and a required one fails the call \
                 at runtime. Add the key to App.vue, or drop the parameter in \
                 src/commands.rs.",
                call.command,
                param.name,
                param.ty,
                call.command,
                call.line,
                call.keys
            );
        }
    }
}

#[test]
fn extract_archive_receives_output_dir_camel_cased() {
    // The concrete hazard the whole file exists for: App.vue writes
    // `outputDir` and Rust declares `output_dir`. Get either half wrong and
    // extraction fails only when a user clicks Extract.
    let commands = parse_rust_commands(&commands_rs());
    let params = commands
        .get("extract_archive")
        .expect("src/commands.rs no longer defines `extract_archive`.");
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["archive", "output_dir"],
        "`extract_archive` in src/commands.rs no longer takes (archive, output_dir). The \
         payload in src/App.vue and the assertion in tests/App.test.js both spell these \
         names out and neither is type-checked."
    );

    let call = parse_invocations(&app_vue())
        .into_iter()
        .find(|c| c.command == "extract_archive")
        .expect("src/App.vue no longer invokes `extract_archive`.");
    assert!(
        call.keys.iter().any(|k| k == "outputDir"),
        "src/App.vue line {} sends {:?} to `extract_archive`. Tauri camelCases the Rust \
         parameter names, so `output_dir` must be sent as `outputDir`; the snake_case \
         spelling deserializes to nothing and the call fails at runtime.",
        call.line,
        call.keys
    );
    assert!(
        !call.keys.iter().any(|k| k == "output_dir"),
        "src/App.vue sends `output_dir` to `extract_archive`. Tauri expects the camelCase \
         `outputDir`; the snake_case spelling is dropped and the command fails at runtime."
    );
}

#[test]
fn compress_path_receives_the_whole_option_set() {
    // compress_path is the widest crossing (five arguments, one of them the
    // Option that chooses local versus remote), so pin its shape explicitly:
    // a dropped `server` key would make every remote compression run locally
    // with no error anywhere.
    let commands = parse_rust_commands(&commands_rs());
    let params = commands
        .get("compress_path")
        .expect("src/commands.rs no longer defines `compress_path`.");
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["path", "output", "format", "level", "server"],
        "`compress_path` in src/commands.rs changed shape. src/App.vue builds this payload \
         by hand and nothing type-checks it."
    );

    let call = parse_invocations(&app_vue())
        .into_iter()
        .find(|c| c.command == "compress_path")
        .expect("src/App.vue no longer invokes `compress_path`.");
    let keys: BTreeSet<&str> = call.keys.iter().map(String::as_str).collect();
    for expected in ["path", "output", "format", "level", "server"] {
        assert!(
            keys.contains(expected),
            "the invoke('compress_path') at src/App.vue line {} does not send `{expected}`. \
             It sends: {:?}",
            call.line,
            call.keys
        );
    }
}
