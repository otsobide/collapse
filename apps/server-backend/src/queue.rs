use std::sync::Arc;

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

async fn process_job(registry: &Registry, storage: &Storage, job_id: &str) {
    // The job may have been deleted while queued.
    let Some(job) = registry.get(job_id) else {
        return;
    };
    registry.update_status(job_id, JobStatus::Compressing, None);

    let input = storage.input_path(job_id, &job.name);
    let output = storage.output_path(job_id, job.algorithm);
    let tree = storage.tree_path(job_id);
    let name = job.name.clone();
    let algorithm = job.algorithm;
    let level = job.level;
    let envelope = job.envelope;

    let result = tokio::task::spawn_blocking(move || match envelope {
        Envelope::None => compress(&input, &output, &name, algorithm, level)
            .map_err(|e| e.to_string()),
        Envelope::Tar => unwrap_and_compress(&input, &tree, &name, &output, algorithm, level),
    })
    .await;

    match result {
        Ok(Ok(())) => registry.update_status(job_id, JobStatus::Completed, None),
        Ok(Err(message)) => registry.update_status(job_id, JobStatus::Failed, Some(message)),
        Err(e) => registry.update_status(job_id, JobStatus::Failed, Some(e.to_string())),
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
