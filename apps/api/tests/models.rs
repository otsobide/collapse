//! Unit tests for the job model, including the JSON shape clients parse.

use collapse_api::models::{Job, JobStatus};
use collapse_core::Algorithm;

fn job(name: &str, algorithm: Algorithm) -> Job {
    Job::new("abc123".to_string(), name.to_string(), algorithm, 3)
}

// ------------------------------------------------------------- construction --

#[test]
fn archive_name_appends_the_algorithm_extension() {
    assert_eq!(job("notes.txt", Algorithm::Zip).archive_name, "notes.txt.zip");
    assert_eq!(job("notes.txt", Algorithm::SevenZ).archive_name, "notes.txt.7z");
    assert_eq!(job("notes.txt", Algorithm::Tar).archive_name, "notes.txt.tar");
}

#[test]
fn archive_name_keeps_the_original_extension() {
    // The source extension is kept, not replaced: notes.txt.zip, so
    // extracting restores the original name.
    assert_eq!(job("photo.jpeg", Algorithm::Zip).archive_name, "photo.jpeg.zip");
    assert_eq!(job("no-extension", Algorithm::Zip).archive_name, "no-extension.zip");
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
            "error_message",
            "job_id",
            "level",
            "name",
            "status",
        ]
    );

    assert_eq!(value["job_id"], "abc123");
    assert_eq!(value["name"], "notes.txt");
    assert_eq!(value["archive_name"], "notes.txt.7z");
    assert_eq!(value["algorithm"], "7z");
    assert_eq!(value["level"], 3);
    assert_eq!(value["status"], "queued");
    assert_eq!(value["error_message"], serde_json::Value::Null);
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
