//! Unit tests for the job model, including the JSON shape clients parse.

use collapse_core::Algorithm;
use collapse_server_backend::models::{Envelope, Job, JobStatus, Verify};

fn job(name: &str, algorithm: Algorithm) -> Job {
    Job::new(
        "abc123".to_string(),
        name.to_string(),
        algorithm,
        3,
        Envelope::None,
        Verify::Index,
    )
}

// ------------------------------------------------------------- construction --

#[test]
fn archive_name_appends_the_algorithm_extension() {
    assert_eq!(
        job("notes.txt", Algorithm::Zip).archive_name,
        "notes.txt.zip"
    );
    assert_eq!(
        job("notes.txt", Algorithm::SevenZ).archive_name,
        "notes.txt.7z"
    );
    assert_eq!(
        job("notes.txt", Algorithm::Tar).archive_name,
        "notes.txt.tar"
    );
}

#[test]
fn archive_name_keeps_the_original_extension() {
    // The source extension is kept, not replaced: notes.txt.zip, so
    // extracting restores the original name.
    assert_eq!(
        job("photo.jpeg", Algorithm::Zip).archive_name,
        "photo.jpeg.zip"
    );
    assert_eq!(
        job("no-extension", Algorithm::Zip).archive_name,
        "no-extension.zip"
    );
}

#[test]
fn a_new_job_starts_queued_without_an_error() {
    let job = job("notes.txt", Algorithm::Zip);
    assert_eq!(job.status, JobStatus::Queued);
    assert!(job.error_message.is_none());
    assert_eq!(job.job_id, "abc123");
    assert_eq!(job.name, "notes.txt");
    assert_eq!(job.level, 3);
}

// ----------------------------------------------------------- wire contract --

/// The CLI matches on these exact strings to decide whether to keep polling,
/// and nothing type-checks that crossing — pin them here.
#[test]
fn job_status_serializes_lowercase() {
    let json = |status| serde_json::to_string(&status).unwrap();
    assert_eq!(json(JobStatus::Queued), "\"queued\"");
    assert_eq!(json(JobStatus::Compressing), "\"compressing\"");
    assert_eq!(json(JobStatus::Completed), "\"completed\"");
    assert_eq!(json(JobStatus::Failed), "\"failed\"");
}

/// The 202 body and the status endpoint serialize a `Job` as-is, so its field
/// names are the API's response schema.
#[test]
fn job_serializes_the_expected_fields() {
    let value = serde_json::to_value(job("notes.txt", Algorithm::SevenZ)).unwrap();
    let object = value.as_object().unwrap();

    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "algorithm",
            "archive_name",
            "envelope",
            "error_message",
            "job_id",
            "level",
            "name",
            "status",
            "verify",
        ]
    );

    assert_eq!(value["job_id"], "abc123");
    assert_eq!(value["name"], "notes.txt");
    assert_eq!(value["archive_name"], "notes.txt.7z");
    assert_eq!(value["algorithm"], "7z");
    assert_eq!(value["level"], 3);
    assert_eq!(value["status"], "queued");
    assert_eq!(value["envelope"], "none");
    assert_eq!(value["verify"], "index");
    assert_eq!(value["error_message"], serde_json::Value::Null);
}

#[test]
fn a_status_prints_and_parses_what_the_wire_carries() {
    // The database column, the log line and the JSON all use this spelling,
    // and the registry parses rows back through it, so a mismatch would make
    // a job unreadable after a restart.
    for status in [
        JobStatus::Queued,
        JobStatus::Compressing,
        JobStatus::Completed,
        JobStatus::Failed,
    ] {
        let text = serde_json::to_value(status).unwrap();
        let text = text.as_str().unwrap();
        assert_eq!(status.to_string(), text);
        assert_eq!(text.parse::<JobStatus>().unwrap(), status);
    }

    assert!("half-done".parse::<JobStatus>().is_err());
}

#[test]
fn only_finished_jobs_are_terminal() {
    // What the reconciliation keys off: the worker owns everything else.
    assert!(JobStatus::Completed.is_terminal());
    assert!(JobStatus::Failed.is_terminal());
    assert!(!JobStatus::Queued.is_terminal());
    assert!(!JobStatus::Compressing.is_terminal());
}

#[test]
fn an_envelope_prints_what_the_wire_carries() {
    // Logs quote the envelope, and an operator reads them against the request
    // that produced them, so the two spellings have to agree.
    for envelope in [Envelope::None, Envelope::Tar] {
        assert_eq!(
            envelope.to_string(),
            serde_json::to_value(envelope).unwrap().as_str().unwrap()
        );
    }
}

#[test]
fn a_verify_depth_prints_and_parses_what_the_wire_carries() {
    // Three places have to agree on one spelling: the query parameter a client
    // sends, the JSON a client reads back, and the database column the worker
    // reads. The column is parsed with `FromStr` and written with `Display`, so
    // a disagreement between those two would make every job unreadable after a
    // restart, and a disagreement with serde would make the wire lie.
    for verify in [Verify::Index, Verify::Contents] {
        let json = serde_json::to_value(verify).unwrap();
        let text = json.as_str().unwrap();
        assert_eq!(verify.to_string(), text);
        assert_eq!(text.parse::<Verify>().unwrap(), verify);
    }

    assert_eq!(Verify::Index.to_string(), "index");
    assert_eq!(Verify::Contents.to_string(), "contents");
}

#[test]
fn an_unknown_verify_depth_says_what_it_would_have_accepted() {
    // The message goes straight into the 400 body, so a caller who guessed
    // `full` or `true` has to be able to see what to send instead.
    let error = "full".parse::<Verify>().unwrap_err();
    assert!(error.contains("full"), "quotes what was sent: {error}");
    assert!(error.contains("index"), "names the choices: {error}");
    assert!(error.contains("contents"), "names the choices: {error}");
}

#[test]
fn each_depth_selects_the_engine_depth_of_the_same_name() {
    // The whole point of the parameter: swap these two arms and every job asks
    // the engine for the wrong amount of work while still reporting the right
    // one on the wire.
    assert_eq!(
        collapse_core::Verify::from(Verify::Index),
        collapse_core::Verify::Index
    );
    assert_eq!(
        collapse_core::Verify::from(Verify::Contents),
        collapse_core::Verify::Contents
    );
}

#[test]
fn a_failed_job_carries_its_message() {
    let mut job = job("notes.txt", Algorithm::Zip);
    job.status = JobStatus::Failed;
    job.error_message = Some("disk full".to_string());

    let value = serde_json::to_value(&job).unwrap();
    assert_eq!(value["status"], "failed");
    assert_eq!(value["error_message"], "disk full");
}
