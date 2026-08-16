use std::sync::Arc;

use tokio::sync::mpsc;

use collapse_core::compress;

use crate::models::JobStatus;
use crate::registry::Registry;
use crate::storage::Storage;

/// Spawn the compression worker task. It consumes job IDs from the channel
/// and processes them sequentially, so concurrent uploads queue up instead of
/// oversubscribing the CPU.
pub(crate) fn start_worker(
    registry: Arc<Registry>,
    storage: Arc<Storage>,
    mut rx: mpsc::UnboundedReceiver<String>,
) {
    tokio::spawn(async move {
        while let Some(job_id) = rx.recv().await {
            process_job(&registry, &storage, &job_id).await;
        }
    });
}

async fn process_job(registry: &Registry, storage: &Storage, job_id: &str) {
    // The job may have been deleted while queued.
    let Some(job) = registry.get(job_id) else {
        return;
    };
    registry.update_status(job_id, JobStatus::Compressing, None);

    let input = storage.input_path(job_id, &job.name);
    let output = storage.output_path(job_id, job.algorithm);
    let arcname = job.name.clone();
    let algorithm = job.algorithm;
    let level = job.level;

    let result =
        tokio::task::spawn_blocking(move || compress(&input, &output, &arcname, algorithm, level))
            .await;

    match result {
        Ok(Ok(())) => registry.update_status(job_id, JobStatus::Completed, None),
        Ok(Err(e)) => registry.update_status(job_id, JobStatus::Failed, Some(e.to_string())),
        Err(e) => registry.update_status(job_id, JobStatus::Failed, Some(e.to_string())),
    }
}
