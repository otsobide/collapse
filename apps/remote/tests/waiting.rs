//! The poll loop, driven through its injected collaborators.
//!
//! Every case here runs in microseconds and spends no real time, which is the
//! whole reason [`Sleeper`] and [`Poller`] exist. The fake sleeper accumulates
//! virtual time instead of spending it, and the fake job reads that
//! accumulation to answer "am I done yet". Between them they express the thing
//! a stub HTTP server cannot express deterministically: a job that takes
//! **exactly** N milliseconds.
//!
//! Assertions are on the [`Waited`] the loop returns, not on counters inside
//! the fakes. A test that reaches into a fake is testing the fake, and passes
//! when the fake is wrong.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use collapse_remote::protocol::Progress;
use collapse_remote::waiting::{
    next_delay, wait_for, Poller, Sleeper, Waited, FIRST_POLL_DELAY, MAX_POLL_DELAY,
};
use collapse_remote::RemoteError;

const MS: fn(u64) -> Duration = Duration::from_millis;

// ------------------------------------------------------------------ fakes --

/// A sleeper that does not sleep: it records the wait and moves virtual time
/// forward by it.
#[derive(Default)]
struct VirtualClock {
    elapsed: Cell<Duration>,
    waits: RefCell<Vec<Duration>>,
}

impl VirtualClock {
    fn elapsed(&self) -> Duration {
        self.elapsed.get()
    }
    fn waits(&self) -> Vec<Duration> {
        self.waits.borrow().clone()
    }
}

impl Sleeper for VirtualClock {
    fn sleep(&self, delay: Duration) {
        self.elapsed.set(self.elapsed.get() + delay);
        self.waits.borrow_mut().push(delay);
    }
}

/// A job that finishes once the caller has waited `takes`.
///
/// This is the mock the issue is really about: compression that takes a known
/// amount of time, with none of it actually elapsing.
struct JobTaking<'a> {
    clock: &'a VirtualClock,
    takes: Duration,
}

impl Poller for JobTaking<'_> {
    fn poll(&self) -> Result<Progress, RemoteError> {
        Ok(if self.clock.elapsed() >= self.takes {
            Progress::Ready
        } else {
            Progress::Waiting
        })
    }
}

/// A server that answers, then breaks.
struct FailsAfter {
    remaining: Cell<u32>,
}

impl Poller for FailsAfter {
    fn poll(&self) -> Result<Progress, RemoteError> {
        if self.remaining.get() == 0 {
            return Err(RemoteError::Malformed(
                "the server stopped making sense".into(),
            ));
        }
        self.remaining.set(self.remaining.get() - 1);
        Ok(Progress::Waiting)
    }
}

/// Run a job of a known length and report what the loop did.
fn run(takes: Duration) -> (Waited, Vec<Duration>) {
    let clock = VirtualClock::default();
    let job = JobTaking {
        clock: &clock,
        takes,
    };
    let account = wait_for(&clock, &job).expect("the job finishes");
    (account, clock.waits())
}

/// What the old flat schedule would have spent on the same job: poll, and if
/// it is not ready, sleep a whole ceiling.
fn under_the_old_schedule(takes: Duration) -> Duration {
    let mut slept = Duration::ZERO;
    while slept < takes {
        slept += MAX_POLL_DELAY;
    }
    slept
}

// ------------------------------------------------------------ issue #48 --

/// The case the issue is about, stated exactly.
///
/// A job that finishes almost immediately, but not before the client's first
/// question. Under the old schedule the caller then slept a full 200 ms; it now
/// sleeps 10 and asks again. Same two polls either way: the difference is
/// entirely the wait, which is why the fix is a schedule and not a protocol
/// change.
#[test]
fn a_job_that_finishes_just_after_the_first_question_waits_ten_milliseconds() {
    let (account, waits) = run(MS(1));

    assert_eq!(account.polls, 2);
    assert_eq!(account.slept, MS(10));
    assert_eq!(waits, vec![MS(10)]);
    assert_eq!(
        under_the_old_schedule(MS(1)),
        MS(200),
        "the old schedule this replaces"
    );
}

/// A job already finished when the first question arrives never sleeps at all.
/// True before and after; pinned so a future schedule cannot introduce a wait
/// before the first poll.
#[test]
fn a_job_already_finished_is_never_slept_on() {
    let (account, waits) = run(Duration::ZERO);
    assert_eq!(
        account,
        Waited {
            polls: 1,
            slept: Duration::ZERO
        }
    );
    assert!(waits.is_empty(), "it waited for a finished job: {waits:?}");
}

// -------------------------------------------------------------- the ramp --

/// The exact sequence, which is the part a reader of the constant cannot see.
#[test]
fn the_wait_doubles_from_ten_to_the_ceiling_and_then_holds() {
    let (_, waits) = run(MS(2_000));

    assert_eq!(
        &waits[..8],
        &[
            MS(10),
            MS(20),
            MS(40),
            MS(80),
            MS(160),
            MS(200),
            MS(200),
            MS(200)
        ],
        "got {waits:?}"
    );
    assert!(
        waits.iter().all(|w| *w <= MAX_POLL_DELAY),
        "past the ceiling: {waits:?}"
    );
    assert!(
        waits.windows(2).all(|p| p[1] >= p[0]),
        "not monotonic: {waits:?}"
    );
}

/// A job of any real length pays almost nothing for the ramp: it reaches the
/// ceiling in 310 ms and five polls, and after that behaves exactly as before.
#[test]
fn a_long_job_costs_only_the_ramp() {
    let (account, waits) = run(MS(10_000));

    let ramp: Duration = waits.iter().take_while(|w| **w < MAX_POLL_DELAY).sum();
    assert_eq!(ramp, MS(310), "the ramp changed length");

    // Against the old schedule over the same job: a handful of extra requests
    // on a job that ran for ten seconds.
    let old_polls =
        (under_the_old_schedule(MS(10_000)).as_millis() / MAX_POLL_DELAY.as_millis()) as u32 + 1;
    assert!(
        account.polls <= old_polls + 4,
        "{} polls against the old {old_polls}",
        account.polls
    );
}

// ------------------------------------------------- honest about the cost --

/// The backoff is **not** uniformly faster, and this pins how much slower it
/// can be.
///
/// A job that finishes just after the ramp is asked again a whole ceiling
/// later, where the flat schedule might have caught it sooner: at 199 ms the
/// old schedule finished at 200 and this one finishes at 310. The band is
/// narrow and bounded by one ceiling, and that bound is the promise worth
/// keeping.
#[test]
fn the_backoff_is_never_worse_by_more_than_one_ceiling() {
    let mut worst = Duration::ZERO;
    let mut worst_at = Duration::ZERO;
    for ms in (0..2_000).step_by(7) {
        let takes = MS(ms);
        let new = run(takes).0.slept;
        let old = under_the_old_schedule(takes);
        if new > old && new - old > worst {
            worst = new - old;
            worst_at = takes;
        }
    }
    assert!(
        worst < MAX_POLL_DELAY,
        "a job taking {worst_at:?} is {worst:?} slower, past the one-ceiling bound"
    );
    assert!(
        worst > Duration::ZERO,
        "if nothing is ever slower this test has stopped measuring anything"
    );
}

/// And what it buys: every job that finishes within the first four waits, which
/// is the common case, is strictly faster than the flat schedule was.
#[test]
fn everything_that_finishes_inside_the_ramp_is_faster_than_before() {
    for ms in 1..=150 {
        let takes = MS(ms);
        let new = run(takes).0.slept;
        let old = under_the_old_schedule(takes);
        assert!(
            new < old,
            "a job taking {takes:?} waited {new:?}, no better than the old {old:?}"
        );
    }
}

// ------------------------------------------------------------ the account --

/// The count includes the poll that settled it, so it is the number of requests
/// actually issued and not the number of waits.
#[test]
fn the_account_counts_the_question_that_settled_it() {
    let (account, waits) = run(MS(1));
    assert_eq!(account.polls as usize, waits.len() + 1);
}

// ------------------------------------------------------------- giving up --

/// A poller that fails stops the loop rather than retrying forever.
#[test]
fn an_error_from_the_server_ends_the_wait() {
    let clock = VirtualClock::default();
    let poller = FailsAfter {
        remaining: Cell::new(3),
    };
    let outcome = wait_for(&clock, &poller);
    assert!(outcome.is_err(), "got {outcome:?}");
    // It did back off between the three answers it did get.
    assert_eq!(clock.waits(), vec![MS(10), MS(20), MS(40)]);
}

// ---------------------------------------------------- the schedule itself --

/// `next_delay` is public, so it must be total: no overflow panic on a nonsense
/// input.
#[test]
fn the_schedule_saturates_instead_of_overflowing() {
    assert_eq!(next_delay(Duration::MAX), MAX_POLL_DELAY);
    assert_eq!(next_delay(Duration::ZERO), Duration::ZERO);
    assert_eq!(next_delay(FIRST_POLL_DELAY), MS(20));
}

/// The first wait has to stay in the band that makes the whole change worth
/// having: short enough that a finished job is not waiting, long enough that a
/// busy server is not hammered.
#[test]
fn the_first_wait_stays_in_its_band() {
    assert!(FIRST_POLL_DELAY >= MS(5), "too close to a spin");
    assert!(
        FIRST_POLL_DELAY <= MS(25),
        "a finished job would still be waiting"
    );
    assert!(FIRST_POLL_DELAY < MAX_POLL_DELAY);
}
