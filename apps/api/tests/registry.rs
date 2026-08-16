//! Unit tests for the in-memory job registry.

use collapse_api::models::{Job, JobStatus};
use collapse_api::registry::Registry;
use collapse_core::Algorithm;

fn job(id: &str) -> Job {
    Job::new(id.to_string(), "notes.txt".to_string(), Algorithm::Zip, 3)
}

// -------------------------------------------------------------- add and get --

#[test]
fn add_then_get_returns_the_job() {
    let registry = Registry::new();
    registry.add(job("j1"));

    let stored = registry.get("j1").expect("job should be stored");
    assert_eq!(stored.job_id, "j1");
    assert_eq!(stored.status, JobStatus::Queued);
}

#[test]
fn get_returns_none_for_an_unknown_job() {
    assert!(Registry::new().get("ghost").is_none());
}

#[test]
fn get_returns_a_snapshot_not_a_handle() {
    // Handlers mutate what `get` hands them (setting error messages, etc.);
    // that must not reach the registry behind its lock.
    let registry = Registry::new();
    registry.add(job("j1"));

    let mut copy = registry.get("j1").unwrap();
    copy.status = JobStatus::Completed;

    assert_eq!(registry.get("j1").unwrap().status, JobStatus::Queued);
}

#[test]
fn jobs_are_keyed_independently() {
    let registry = Registry::new();
    registry.add(job("j1"));
    registry.add(job("j2"));

    registry.update_status("j1", JobStatus::Completed, None);

    assert_eq!(registry.get("j1").unwrap().status, JobStatus::Completed);
    assert_eq!(registry.get("j2").unwrap().status, JobStatus::Queued);
}

#[test]
fn adding_the_same_id_twice_replaces_the_job() {
    let registry = Registry::new();
    registry.add(job("j1"));
    registry.update_status("j1", JobStatus::Completed, None);
    registry.add(job("j1"));

    assert_eq!(registry.get("j1").unwrap().status, JobStatus::Queued);
}

// ------------------------------------------------------------ status updates --

#[test]
fn update_status_advances_the_lifecycle() {
    let registry = Registry::new();
    registry.add(job("j1"));

    registry.update_status("j1", JobStatus::Compressing, None);
    assert_eq!(registry.get("j1").unwrap().status, JobStatus::Compressing);

    registry.update_status("j1", JobStatus::Completed, None);
    assert_eq!(registry.get("j1").unwrap().status, JobStatus::Completed);
}

#[test]
fn update_status_records_the_failure_message() {
    let registry = Registry::new();
    registry.add(job("j1"));

    registry.update_status("j1", JobStatus::Failed, Some("boom".to_string()));

    let stored = registry.get("j1").unwrap();
    assert_eq!(stored.status, JobStatus::Failed);
    assert_eq!(stored.error_message.as_deref(), Some("boom"));
}

#[test]
fn update_status_clears_a_previous_message() {
    let registry = Registry::new();
    registry.add(job("j1"));
    registry.update_status("j1", JobStatus::Failed, Some("boom".to_string()));

    registry.update_status("j1", JobStatus::Completed, None);

    assert!(registry.get("j1").unwrap().error_message.is_none());
}

/// The worker updates a job that a concurrent DELETE may already have
/// removed; that must be a no-op, not a resurrection.
#[test]
fn update_status_on_an_unknown_job_does_nothing() {
    let registry = Registry::new();
    registry.update_status("ghost", JobStatus::Completed, None);
    assert!(registry.get("ghost").is_none());
}

// ----------------------------------------------------------------- removal --

#[test]
fn remove_returns_the_job_and_forgets_it() {
    let registry = Registry::new();
    registry.add(job("j1"));

    let removed = registry.remove("j1").expect("removed job is returned");
    assert_eq!(removed.job_id, "j1");
    assert!(registry.get("j1").is_none());
}

#[test]
fn remove_returns_none_for_an_unknown_job() {
    assert!(Registry::new().remove("ghost").is_none());
}
