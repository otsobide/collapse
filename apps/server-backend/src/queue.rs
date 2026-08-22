use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

use std::path::Path;

use collapse_core::compression::extract_tar;
use collapse_core::{compress, compress_dir, Algorithm};

use crate::models::{Envelope, JobStatus};
use crate::registry::Registry;
use crate::storage::{single_root_dir, Storage};

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

/// Record a status change, or say so. The worker has nobody to return an
/// error to, and a status that never lands would leave a client polling a job
/// that is quietly finished.
fn set_status(registry: &Registry, job_id: &str, status: JobStatus, message: Option<String>) {
    if let Err(e) = registry.update_status(job_id, status, message) {
        tracing::error!(job = %job_id, %status, error = %e, "cannot record the job status");
    }
}

async fn process_job(registry: &Registry, storage: &Storage, job_id: &str) {
    let job = match registry.get(job_id) {
        Ok(Some(job)) => job,
        // The job may have been deleted while queued.
        Ok(None) => {
            tracing::debug!(job = %job_id, "gone before it started");
            return;
        }
        Err(e) => {
            tracing::error!(job = %job_id, error = %e, "cannot read the job, skipping it");
            return;
        }
    };
    set_status(registry, job_id, JobStatus::Compressing, None);
    tracing::info!(
        job = %job_id,
        name = %job.name,
        algorithm = %job.algorithm,
        level = job.level,
        "compressing"
    );
    let started = Instant::now();

    let input = storage.input_path(job_id);
    let output = storage.output_path(job_id, job.algorithm);
    let tree = storage.tree_path(job_id);
    let name = job.name.clone();
    let algorithm = job.algorithm;
    let level = job.level;
    let envelope = job.envelope;

    let result = tokio::task::spawn_blocking(move || match envelope {
        Envelope::None => {
            compress(&input, &output, &name, algorithm, level).map_err(|e| e.to_string())
        }
        Envelope::Tar => unwrap_and_compress(&input, &tree, &name, &output, algorithm, level),
    })
    .await;

    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(Ok(())) => {
            set_status(registry, job_id, JobStatus::Completed, None);
            let bytes = std::fs::metadata(storage.output_path(job_id, algorithm))
                .map(|meta| meta.len())
                .unwrap_or_default();
            tracing::info!(job = %job_id, bytes, elapsed_ms, "completed");
        }
        // A rejected upload (a hostile tar, an unreadable source) is the
        // client's problem, not the server's, so it is a warning; a worker
        // that dies mid-job is ours.
        Ok(Err(message)) => {
            set_status(registry, job_id, JobStatus::Failed, Some(message.clone()));
            tracing::warn!(job = %job_id, elapsed_ms, error = %message, "failed");
        }
        Err(e) => {
            set_status(registry, job_id, JobStatus::Failed, Some(e.to_string()));
            tracing::error!(job = %job_id, elapsed_ms, error = %e, "the worker died on this job");
        }
    }
}

/// Unpack a tar envelope and compress the directory it holds.
///
/// Extraction goes through the engine's tar backend, which refuses entries
/// that would escape the output directory and materializes no links, so a
/// hostile tar cannot reach outside the job's own staging area.
fn unwrap_and_compress(
    input: &Path,
    tree: &Path,
    name: &str,
    output: &Path,
    algorithm: Algorithm,
    level: u32,
) -> Result<(), String> {
    extract_tar(input, tree).map_err(|e| e.to_string())?;
    let root = single_root_dir(tree, name)?;
    compress_dir(&root, output, algorithm, level).map_err(|e| e.to_string())
}
