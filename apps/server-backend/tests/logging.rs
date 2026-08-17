//! The log filter: what `RUST_LOG` does, and what a bad `RUST_LOG` does not
//! do. The subscriber itself is process-global and installed once, so only the
//! pure half is exercised here.

use collapse_server_backend::logging::{filter, DEFAULT_FILTER};

#[test]
fn no_spec_falls_back_to_the_default() {
    assert_eq!(filter(None).to_string(), DEFAULT_FILTER);
}

#[test]
fn a_level_is_applied() {
    assert_eq!(filter(Some("debug")).to_string(), "debug");
}

#[test]
fn per_target_directives_survive() {
    let spec = "collapse_server_backend=debug,tower_http=warn";
    assert_eq!(filter(Some(spec)).to_string(), spec);
}

#[test]
fn an_unparseable_spec_falls_back_instead_of_muting_the_server() {
    // A typo must not leave the server running blind, and must not stop it
    // from starting either.
    assert_eq!(filter(Some("=,,=")).to_string(), DEFAULT_FILTER);
}

#[test]
fn an_empty_spec_falls_back() {
    // `RUST_LOG=` in a compose file reads as "unset", not as "log nothing".
    assert_eq!(filter(Some("")).to_string(), DEFAULT_FILTER);
    assert_eq!(filter(Some("   ")).to_string(), DEFAULT_FILTER);
}
