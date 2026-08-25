//! One thing about the worker that no other test in this crate can see.
//!
//! Everything else here drives the app and looks at what came back. That works
//! for every parameter but one: a healthy compression passes at either
//! verification depth, so a job run at `contents` and the same job run at
//! `index` produce byte-identical archives, the same status and the same JSON.
//! A worker that read the flag off the row, reported it faithfully and then
//! handed the engine a hardcoded depth would leave every other test in this
//! crate green while the parameter did nothing at all.
//!
//! So this one reads the worker's source, the way `apps/desktop`'s `tests/ipc.rs`
//! reads three files to prove a crossing that nothing type checks. It is a
//! narrow claim: the worker takes the depth from the job, and never picks one.

/// Whitespace removed, so the assertions survive any reformatting `cargo fmt`
/// decides on.
fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn the_worker_asks_the_engine_for_the_depth_the_job_recorded() {
    let worker = compact(include_str!("../src/queue.rs"));

    // One per compression arm: the single file path and the tar envelope path.
    assert_eq!(
        worker.matches("job.verify.into()").count(),
        2,
        "both compression arms must pass the job's own depth to the engine"
    );
}

#[test]
fn the_worker_never_names_a_verification_depth_itself() {
    let worker = compact(include_str!("../src/queue.rs"));

    // Naming one here is exactly the bug the test above cannot catch on its
    // own: the arms could keep reading `job.verify` for the log while a literal
    // went to the engine.
    for literal in ["Verify::Index", "Verify::Contents"] {
        assert!(
            !worker.contains(&compact(literal)),
            "the worker names {literal} instead of reading the depth off the job"
        );
    }
}
