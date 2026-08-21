//! Unit tests for the pure remote protocol helpers: URL building, reading
//! the server's JSON, and the poll loop's decision on each job status.

use collapse_remote::protocol::{
    base_url, healthy, job_id_of, progress_of, rejection_message, Progress,
};
use collapse_remote::RemoteError;
use serde_json::json;

fn message_of(error: RemoteError) -> String {
    error.to_string()
}

// ------------------------------------------------------------------- URLs --

#[test]
fn base_url_trims_trailing_slashes() {
    assert_eq!(base_url("http://host:8000"), "http://host:8000");
    assert_eq!(base_url("http://host:8000/"), "http://host:8000");
    assert_eq!(base_url("http://host:8000///"), "http://host:8000");
}

#[test]
fn base_url_keeps_a_path_prefix() {
    // A server mounted under a path must keep it: only the trailing
    // separator goes.
    assert_eq!(base_url("http://host/collapse/"), "http://host/collapse");
    assert_eq!(base_url("https://host/api/v1"), "https://host/api/v1");
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
