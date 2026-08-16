use std::collections::HashMap;
use std::sync::Mutex;

use crate::models::{Job, JobStatus};

/// In-memory job registry: a `Mutex`-guarded map, not persisted across
/// restarts. Jobs live here until deleted through the API.
pub(crate) struct Registry {
    jobs: Mutex<HashMap<String, Job>>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn add(&self, job: Job) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.insert(job.job_id.clone(), job);
    }

    pub(crate) fn get(&self, job_id: &str) -> Option<Job> {
        let jobs = self.jobs.lock().unwrap();
        jobs.get(job_id).cloned()
    }

    pub(crate) fn update_status(
        &self,
        job_id: &str,
        status: JobStatus,
        error_message: Option<String>,
    ) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = status;
            job.error_message = error_message;
        }
    }

    pub(crate) fn remove(&self, job_id: &str) -> Option<Job> {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.remove(job_id)
    }
}
