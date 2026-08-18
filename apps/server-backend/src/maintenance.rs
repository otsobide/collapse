//! Making the registry and the staging directory agree.
//!
//! Two stores hold one truth: rows in SQLite and directories on disk. They can
//! disagree only after something interrupted the server between writing one
//! and the other, or after a restart cut a job in half. Reconciling at startup
//! is what turns "the database survives" into "nothing is left stranded":
//! without it a persistent registry would remember jobs whose files are gone,
//! and keep ignoring files whose job it never knew.

use crate::error::StartupError;
use crate::models::JobStatus;
use crate::registry::Registry;
use crate::storage::Storage;

/// What a pass had to put right. All zeroes is the normal case.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Jobs still queued or compressing, which a restart cut short.
    pub interrupted: usize,
    /// Jobs whose staged files are gone, so there is nothing left to serve.
    pub without_files: usize,
    /// Directories no job claims, left by a crash or by a server that ran
    /// before the registry was persistent.
    pub orphaned: usize,
}

impl Reconciled {
    /// Whether anything was out of place, so a quiet startup stays quiet.
    pub fn is_clean(&self) -> bool {
        *self == Reconciled::default()
    }
}

/// The message a job interrupted by a restart carries from then on. Clients
/// show `error_message` verbatim, so it is written for a person.
pub const INTERRUPTED: &str = "The server restarted while this job was running.";

/// Delete finished jobs untouched since `deadline` (unix seconds), files and
/// row together. Returns how many went.
///
/// This is the other half of the leak. Reconciling at startup catches what a
/// restart stranded; this catches the client that uploaded, downloaded and
/// then never called `DELETE`, which no amount of bookkeeping can distinguish
/// from one that simply walked away.
///
/// Only finished jobs are considered, so nothing is pulled out from under the
/// worker. A download refreshes the clock, so a client that keeps coming back
/// for its archive keeps it; one that stops asking loses it.
pub fn reap(registry: &Registry, storage: &Storage, deadline: i64) -> Result<usize, StartupError> {
    let mut reaped = 0;
    for job_id in registry.expired(deadline)? {
        // Files first: a row without files is harmless (the next startup drops
        // it), while files without a row would be invisible to everything but
        // the startup sweep.
        storage.delete_job(&job_id);
        registry.remove(&job_id)?;
        reaped += 1;
    }
    Ok(reaped)
}

/// Reconcile the registry against the staging directory. Call once at startup,
/// before the server accepts requests: it assumes no worker is running, which
/// is exactly what makes a `queued` or `compressing` row provably stale.
pub fn reconcile(registry: &Registry, storage: &Storage) -> Result<Reconciled, StartupError> {
    let mut report = Reconciled::default();

    // A job whose files are gone can only 404 on download, so the row goes
    // too. This runs first: a job that is about to be dropped should not also
    // be reported as interrupted, and there is no point writing a status to a
    // row on its way out.
    let mut known = std::collections::HashSet::new();
    for job_id in registry.ids()? {
        if storage.has_job(&job_id) {
            known.insert(job_id);
        } else {
            registry.remove(&job_id)?;
            report.without_files += 1;
        }
    }

    // Nothing is compressing yet, so anything still claiming to be was cut
    // short. They are failed rather than requeued on purpose: the client that
    // asked may be long gone, and an input that kills the worker would
    // otherwise be retried on every boot, turning a restart policy into a
    // crash loop.
    for job_id in registry.unfinished()? {
        registry.update_status(&job_id, JobStatus::Failed, Some(INTERRUPTED.to_string()))?;
        report.interrupted += 1;
    }

    // The other direction: files nobody claims. This is the case that used to
    // grow without bound, since the API could not even name those jobs.
    //
    // The name is matched against the known ids as text, which is safe because
    // every id this server writes is hex; but it is *deleted* by the name the
    // filesystem gave, since a name that is not valid UTF-8 would not survive
    // the round trip through a String.
    for name in storage.staged_jobs()? {
        if known.contains(name.to_string_lossy().as_ref()) {
            continue;
        }
        // Counted only once it is really gone: a report that claims a cleanup
        // it did not do is worse than one that admits the problem, because it
        // reads as clean on every boot while the directory stays forever.
        if storage.delete_job(&name) {
            report.orphaned += 1;
        } else {
            tracing::warn!(entry = %name.to_string_lossy(), "cannot remove a staged directory no job claims");
        }
    }

    Ok(report)
}
