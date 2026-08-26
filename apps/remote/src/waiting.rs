//! The poll loop: how long to wait between asking, and the seam that lets it
//! be tested without spending the time it is deciding how to spend.
//!
//! Kept apart from [`crate::protocol`] on purpose. That module answers "what
//! does this JSON mean"; this one answers "how often should I ask". They used
//! to be one file and the split is what makes the second question answerable
//! by a test in microseconds.
//!
//! The inversion is two one-method traits, [`Sleeper`] and [`Poller`]. Between
//! them a test can express the thing that actually matters and that no stub
//! server can express deterministically: **a job that takes exactly N
//! milliseconds**. The fake sleeper accumulates virtual time instead of
//! spending real time, and the fake poller reports the job finished once that
//! accumulation passes N.
//!
//! [`wait_for`] returns [`Waited`], its own account of what it did. That is
//! deliberate: a test asserting on the returned count is asserting on the
//! subject, while a test reaching into the fake to count calls is asserting on
//! the fake. The second kind passes when the fake is wrong.

use std::time::Duration;

use crate::protocol::Progress;
use crate::RemoteError;

/// How long to wait before the **first** re-poll of a job the server has not
/// finished yet.
///
/// Short on purpose. Nearly every archive a person compresses is done in less
/// time than a person notices, and the old schedule waited a flat 200 ms
/// before asking a second time, so a five byte file took ~235 ms end to end
/// with almost none of that spent compressing (issue #48).
///
/// Not zero, and not one millisecond: the point is to stop making a finished
/// job wait, not to spin on a server that is genuinely busy.
pub const FIRST_POLL_DELAY: Duration = Duration::from_millis(10);

/// The ceiling the wait grows to, and the interval a long job settles into.
///
/// Deliberately the old fixed interval, so nothing about a job that takes
/// minutes changes: it reaches this after five polls and stays here.
///
/// It is also the bound on how much *worse* the backoff can be than the old
/// schedule. A job that finishes just after the ramp is polled again a whole
/// ceiling later, where the flat schedule might have caught it sooner. That
/// band is narrow and bounded, and `the_backoff_is_never_worse_by_more_than_
/// one_ceiling` pins it.
pub const MAX_POLL_DELAY: Duration = Duration::from_millis(200);

/// The wait before the next poll, given the wait before the last one.
///
/// Doubles until it reaches [`MAX_POLL_DELAY`], so the schedule is
/// 10, 20, 40, 80, 160, 200, 200, ... It reaches the ceiling in 310 ms and
/// costs a job of any real length about three extra requests over its whole
/// life, against saving ~190 ms on every job that was already done.
pub fn next_delay(previous: Duration) -> Duration {
    previous.saturating_mul(2).min(MAX_POLL_DELAY)
}

/// The passage of time, injected rather than called directly.
///
/// One method, and it is the only thing the loop does that a test cannot
/// afford to let happen for real.
pub trait Sleeper {
    fn sleep(&self, delay: Duration);
}

/// One question to the server: how is the job doing?
///
/// The address and the job id are the implementation's business, so this takes
/// no arguments: a test's fake has no server to address.
pub trait Poller {
    fn poll(&self) -> Result<Progress, RemoteError>;
}

/// Real time.
pub struct RealSleeper;

impl Sleeper for RealSleeper {
    fn sleep(&self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

/// What [`wait_for`] did, so a caller (or a test) can account for it without
/// inspecting the collaborators it was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waited {
    /// How many times the server was asked, including the one that settled it.
    pub polls: u32,
    /// How long was spent sleeping between those questions. Not wall clock:
    /// the requests themselves are not counted, because this is the part the
    /// schedule controls.
    pub slept: Duration,
}

/// Poll until the job settles, waiting a little longer each time.
///
/// **Unbounded, still.** A server that answers `compressing` forever keeps this
/// running forever (issue #71). A backoff changes how often it asks, not
/// whether it ever stops. The seam here makes a deadline a few lines, and a
/// deliberately separate change.
pub fn wait_for(sleeper: &dyn Sleeper, poller: &dyn Poller) -> Result<Waited, RemoteError> {
    let mut delay = FIRST_POLL_DELAY;
    let mut account = Waited {
        polls: 0,
        slept: Duration::ZERO,
    };
    loop {
        account.polls += 1;
        match poller.poll()? {
            Progress::Ready => return Ok(account),
            Progress::Waiting => {
                sleeper.sleep(delay);
                account.slept += delay;
                delay = next_delay(delay);
            }
        }
    }
}
