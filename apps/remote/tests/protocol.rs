//! Unit tests for the pure remote protocol helpers: URL building, reading
//! the server's JSON, and the poll loop's decision on each job status.

use collapse_remote::protocol::{
    base_url, healthy, job_id_of, next_delay, progress_of, rejection_message, Progress,
    FIRST_POLL_DELAY, MAX_POLL_DELAY,
};
use collapse_remote::RemoteError;
use serde_json::json;
use std::time::Duration;

fn message_of(error: RemoteError) -> String {
    error.to_string()
}

// ------------------------------------------------------------------- URLs --

#[test]
fn base_url_trims_trailing_slashes() {
    assert_eq!(base_url("http://host:8000").unwrap(), "http://host:8000");
    assert_eq!(base_url("http://host:8000/").unwrap(), "http://host:8000");
    assert_eq!(base_url("http://host:8000///").unwrap(), "http://host:8000");
}

#[test]
fn base_url_keeps_a_path_prefix() {
    // A server mounted under a path must keep it: only the trailing
    // separator goes.
    assert_eq!(
        base_url("http://host/collapse/").unwrap(),
        "http://host/collapse"
    );
    assert_eq!(
        base_url("https://host/api/v1").unwrap(),
        "https://host/api/v1"
    );
}

/// A blank address is refused here, once, so no front-end has to decide what
/// it means: the desktop used to read `""` as "compress locally" and `"   "`
/// as a real destination, while the CLI sent both over the wire.
#[test]
fn a_blank_address_is_not_an_address() {
    for blank in ["", " ", "   ", "\t", "\n", " \t \n "] {
        let message = match base_url(blank) {
            Err(error) => message_of(error),
            Ok(base) => panic!("{blank:?} was accepted as the address {base:?}"),
        };
        // The message has to name the mistake and show a real address: the
        // old failure said "cannot reach the server at    ", which blames the
        // network for a destination that was never typed.
        assert!(
            message.contains("blank"),
            "{blank:?} must be named as blank: {message:?}"
        );
        assert!(
            message.contains("http://localhost:8000"),
            "{blank:?} must be shown what an address looks like: {message:?}"
        );
        assert!(
            !message.contains("cannot reach"),
            "{blank:?} is not a reachability problem: {message:?}"
        );
    }
}

/// What "blank" means, spelled out where the answer is written. The guard is
/// `server.trim().is_empty()`, so the rule is Rust's `char::is_whitespace`
/// (the Unicode White_Space property), not the space and tab an ASCII check
/// would cover. Every caller's own table tries two or three spellings and
/// stops, so this is the only place the rule itself is pinned.
///
/// None of these is invented: a non-breaking space is what comes out of a
/// pasted web page, `\r\n` is what a wrapper script gets from a file it read
/// as a line, and an ideographic space is what an IME leaves behind. Narrow
/// the guard to `is_empty()`, or to a hand-written `[' ', '\t']` check, and
/// every row below turns red instead of being sent to a server with no name.
#[test]
fn blank_is_whatever_rust_calls_whitespace() {
    for blank in [
        "\r",
        "\r\n",
        "\u{000b}",         // vertical tab
        "\u{000c}",         // form feed
        "\u{00a0}",         // no-break space
        "\u{2003}",         // em space
        "\u{3000}",         // ideographic space
        " \r\n\t\u{00a0} ", // and a mix of them
    ] {
        match base_url(blank) {
            Err(RemoteError::BlankServer) => {}
            Err(other) => panic!("{blank:?} was refused as {other:?}, not as a blank address"),
            Ok(base) => panic!("{blank:?} was accepted as the address {base:?}"),
        }
    }
}

/// The other side of that rule, stated rather than left to be discovered: a
/// zero-width space is not whitespace to Rust, so it is not blank here and
/// travels on as an address for the HTTP client to reject. Pinned so the
/// guard cannot quietly grow into a "looks invisible to a human" check whose
/// boundary nobody could predict, and so the boundary it does have is on the
/// record.
#[test]
fn a_zero_width_character_is_not_blank() {
    for invisible in ["\u{200b}", "\u{feff}"] {
        assert_eq!(
            base_url(invisible).unwrap(),
            invisible,
            "{invisible:?} is not whitespace, so this crate has no verdict on it"
        );
    }
}

/// The guard is emptiness, not "looks like a URL": whatever else is wrong
/// with an address is for the HTTP client to report, and rejecting more here
/// would refuse hosts this crate has no business judging.
#[test]
fn a_non_blank_address_is_passed_through() {
    for address in ["localhost:8000", "not a url", "http://host /x"] {
        assert_eq!(base_url(address).unwrap(), address);
    }
}

/// An address of nothing but slashes is not an address either.
///
/// It has no whitespace to trim, so an emptiness check on the raw input calls
/// it non blank; trimming its slashes afterwards then leaves nothing, and the
/// endpoints are joined onto an empty base. That reached the user as "cannot
/// reach the server at : ...", the nameless-server message issue #65 set out
/// to remove, by the one road the first version of the fix did not cover.
/// Normalizing before deciding folds it into the same refusal.
#[test]
fn an_address_of_only_slashes_is_refused_like_a_blank_one() {
    for slashes in ["/", "//", "///", "  //  "] {
        assert!(
            matches!(base_url(slashes), Err(RemoteError::BlankServer)),
            "{slashes:?} is nothing but separators, so there is no address in it"
        );
    }
}

/// Surrounding whitespace is stripped, and the trailing slash with it.
///
/// The order matters and is the point: trimming the whitespace first is what
/// lets the slash trim see the slash. Before that, a trailing space defeated
/// it, so `http://host:8000/ ` kept the separator this function exists to
/// remove and every endpoint was joined onto a double slash.
#[test]
fn surrounding_whitespace_is_stripped_from_a_real_address() {
    for (typed, expected) in [
        (" http://host:8000 ", "http://host:8000"),
        ("http://host:8000/ ", "http://host:8000"),
        ("\thttp://host:8000/\n", "http://host:8000"),
        ("  http://host:8000///  ", "http://host:8000"),
    ] {
        assert_eq!(
            base_url(typed).unwrap(),
            expected,
            "{typed:?} should normalize to {expected:?}"
        );
    }
}

// --------------------------------------------------------------- job ids --

#[test]
fn job_id_is_read_from_the_accepted_body() {
    let body = json!({ "job_id": "abc123", "status": "queued" });
    assert_eq!(job_id_of(&body).unwrap(), "abc123");
}

#[test]
fn a_body_without_a_usable_job_id_is_an_error() {
    // Missing, wrong type, or null: all unusable, and none may panic.
    for body in [
        json!({ "status": "queued" }),
        json!({ "job_id": 42 }),
        json!({ "job_id": null }),
        json!({}),
    ] {
        let error = job_id_of(&body).expect_err("should be rejected");
        assert!(
            message_of(error).contains("no job_id"),
            "unexpected message for {body}"
        );
    }
}

// ------------------------------------------------------- polling decision --

#[test]
fn in_progress_statuses_keep_the_client_waiting() {
    for status in ["queued", "compressing"] {
        let job = json!({ "job_id": "a", "status": status });
        assert_eq!(progress_of(&job).unwrap(), Progress::Waiting, "{status}");
    }
}

#[test]
fn completed_lets_the_client_download() {
    let job = json!({ "job_id": "a", "status": "completed" });
    assert_eq!(progress_of(&job).unwrap(), Progress::Ready);
}

#[test]
fn a_failed_job_surfaces_the_server_message() {
    let job = json!({ "job_id": "a", "status": "failed", "error_message": "disk full" });
    let message = message_of(progress_of(&job).expect_err("should fail"));

    assert!(message.contains("disk full"), "got {message:?}");
    assert!(message.contains("server-side"), "got {message:?}");
}

#[test]
fn a_failed_job_without_a_message_still_fails() {
    for job in [
        json!({ "status": "failed" }),
        json!({ "status": "failed", "error_message": null }),
    ] {
        let message = message_of(progress_of(&job).expect_err("should fail"));
        assert!(message.contains("compression failed"), "got {message:?}");
    }
}

/// The poll loop must not spin on a server that answers something else:
/// these used to be swallowed by the catch-all and polled forever.
#[test]
fn an_unknown_status_is_an_error_not_a_reason_to_wait() {
    for status in ["cancelled", "COMPLETED", "", "pending"] {
        let job = json!({ "job_id": "a", "status": status });
        let message = message_of(progress_of(&job).expect_err("{status} should fail"));
        assert!(
            message.contains("unexpected job status"),
            "{status}: got {message:?}"
        );
    }
}

#[test]
fn a_body_without_a_status_is_an_error() {
    for job in [json!({ "job_id": "a" }), json!({ "status": 7 }), json!({})] {
        let message = message_of(progress_of(&job).expect_err("should fail"));
        assert!(message.contains("no status"), "got {message:?}");
    }
}

// ------------------------------------------------------------ health probe --

#[test]
fn a_healthy_server_is_accepted() {
    assert!(healthy(&json!({ "status": "ok" })).is_ok());
}

/// Something answered, but it is not a Collapse server: worth catching while
/// the user is still typing the address rather than after an upload.
#[test]
fn anything_else_is_not_a_collapse_server() {
    for body in [
        json!({ "status": "degraded" }),
        json!({ "ok": true }),
        json!({}),
        json!("ok"),
    ] {
        let message = message_of(healthy(&body).expect_err("should be rejected"));
        assert!(message.contains("does not look like"), "got {message:?}");
    }
}

// ------------------------------------------------------- rejection bodies --

#[test]
fn rejection_prefers_the_servers_detail_field() {
    let message = rejection_message(400, r#"{"detail":"Invalid file name."}"#);

    assert!(message.contains("400"), "got {message:?}");
    assert!(message.contains("Invalid file name."), "got {message:?}");
}

#[test]
fn rejection_falls_back_to_the_raw_body() {
    // Not the API's error shape (a proxy or another server answered).
    for body in ["<html>502 Bad Gateway</html>", r#"{"error":"nope"}"#] {
        let message = rejection_message(502, body);
        assert!(message.contains(body), "got {message:?}");
    }
}

#[test]
fn rejection_without_a_body_still_names_the_status() {
    let message = rejection_message(413, "");
    assert!(message.contains("413"), "got {message:?}");
    // No dangling separator when there is nothing to append.
    assert!(!message.ends_with(": "), "got {message:?}");
}

#[test]
fn rejection_handles_a_detail_that_is_not_a_string() {
    // Must not panic or print "null"; falls back to showing the body.
    let body = r#"{"detail":{"code":7}}"#;
    let message = rejection_message(400, body);
    assert!(message.contains(body), "got {message:?}");
}

// ------------------------------------------------------- the poll schedule --

/// The whole point of issue #48: a job the server has already finished must
/// not be made to wait out a fixed interval before anyone asks again.
///
/// The measurement in the issue was ~235 ms for a five byte file against ~23 ms
/// locally, and almost none of that was the server. It was this delay.
#[test]
fn the_first_wait_is_short_enough_not_to_dominate_a_tiny_job() {
    assert!(
        FIRST_POLL_DELAY <= Duration::from_millis(25),
        "a finished job would still be waiting: {FIRST_POLL_DELAY:?}"
    );
    // And not so short that a busy server is hammered: this is a backoff, not
    // a spin.
    assert!(
        FIRST_POLL_DELAY >= Duration::from_millis(5),
        "too close to a spin: {FIRST_POLL_DELAY:?}"
    );
}

/// The schedule doubles, reaches the ceiling and stays there.
#[test]
fn the_wait_doubles_up_to_the_ceiling_and_then_holds() {
    let mut delay = FIRST_POLL_DELAY;
    let mut schedule = vec![delay];
    for _ in 0..12 {
        delay = next_delay(delay);
        schedule.push(delay);
    }

    assert_eq!(
        &schedule[..6],
        &[
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(40),
            Duration::from_millis(80),
            Duration::from_millis(160),
            Duration::from_millis(200),
        ],
        "got {schedule:?}"
    );
    // Never above the ceiling, and never smaller than the step before it.
    for pair in schedule.windows(2) {
        assert!(pair[1] <= MAX_POLL_DELAY, "past the ceiling: {schedule:?}");
        assert!(pair[1] >= pair[0], "not monotonic: {schedule:?}");
    }
    assert_eq!(*schedule.last().unwrap(), MAX_POLL_DELAY);
}

/// A long job must not pay for the ramp. Reaching the ceiling costs 310 ms
/// spread over five extra requests, which is the whole price of the change.
#[test]
fn reaching_the_ceiling_is_cheap_for_a_job_that_runs_for_minutes() {
    let mut delay = FIRST_POLL_DELAY;
    let mut total = delay;
    let mut polls = 1;
    while delay < MAX_POLL_DELAY {
        delay = next_delay(delay);
        total += delay;
        polls += 1;
    }
    assert_eq!(polls, 6, "the ramp got longer");
    assert!(
        total <= Duration::from_millis(600),
        "the ramp costs too much: {total:?}"
    );
}

/// `next_delay` must be total: no overflow panic on a nonsense input, since it
/// is public and nothing stops a caller feeding it one.
#[test]
fn the_schedule_saturates_instead_of_overflowing() {
    assert_eq!(next_delay(Duration::MAX), MAX_POLL_DELAY);
    assert_eq!(next_delay(Duration::ZERO), Duration::ZERO);
}
