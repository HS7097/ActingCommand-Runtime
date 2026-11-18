// SPDX-License-Identifier: AGPL-3.0-only

//! When `stop` has seen enough (board #479, ACIB-3, fix round 1): the pure decision after each
//! look at the processes that follows its signals.
//!
//! A process the kill signal has ended can still be listed for a moment, until the system has
//! taken it away; one look at once after the signal would answer `still_running` for it, the
//! word of a process the signal did not end. So `stop` looks at once after its signals and then
//! again every [`LOOK_INTERVAL`] while a process it signalled is still listed, never past the
//! command's deadline, and answers `still_running` only when the last look inside the deadline
//! still lists a process it signalled. The verdict comes from the looks alone: what the service
//! answered to each signal is a fact beside it, never the verdict.

use std::time::Duration;

/// How long `stop` waits between two looks (fixed, as the coordinator's ruling asks: about
/// 200 ms).
pub const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// What a look says of the processes `stop` signalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No process the command signalled is listed.
    Stopped,
    /// A process the command signalled is still listed.
    StillRunning,
}

/// What `stop` does after a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// A process it signalled is still listed, and another look fits before the deadline: wait
    /// [`LOOK_INTERVAL`] and look again.
    LookAgain,
    /// The look is the last: its verdict is the command's.
    Done(Verdict),
}

/// The verdict of a look that lists the application's processes `listed`, on the processes
/// `signalled`: [`Verdict::StillRunning`] when it lists one of them. A process it lists that was
/// not signalled (one that started after the first look) is no process the command signalled.
pub fn verdict(signalled: &[u32], listed: &[u32]) -> Verdict {
    if listed.iter().any(|pid| signalled.contains(pid)) {
        Verdict::StillRunning
    } else {
        Verdict::Stopped
    }
}

/// What `stop` does after a look that lists the application's processes `listed`, when it
/// signalled `signalled` and `time_left` is left until its deadline after that look (`None`: no
/// deadline the clock reaches): done, stopped, when no process it signalled is listed; done,
/// still running, when one is and the next look, [`LOOK_INTERVAL`] later, would not start before
/// the deadline; else look again.
pub fn after_look(signalled: &[u32], listed: &[u32], time_left: Option<Duration>) -> Next {
    match verdict(signalled, listed) {
        Verdict::Stopped => Next::Done(Verdict::Stopped),
        Verdict::StillRunning if time_left.is_some_and(|left| left <= LOOK_INTERVAL) => {
            Next::Done(Verdict::StillRunning)
        }
        Verdict::StillRunning => Next::LookAgain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A time left of `milliseconds`.
    fn left(milliseconds: u64) -> Option<Duration> {
        Some(Duration::from_millis(milliseconds))
    }

    /// After each look: stopped as soon as no process the command signalled is listed (none
    /// listed, or only one it did not signal), whatever the time left; while one is listed, look
    /// again when the next look fits before the deadline (more than [`LOOK_INTERVAL`] left, or no
    /// deadline), and still running when it would not (the interval or less left, none at all).
    #[test]
    fn after_a_look_stop_looks_again_has_stopped_or_is_still_running() {
        let signalled = [412, 530];
        let cases: [(&[u32], Option<Duration>, Next, &str); 12] = [
            (
                &[],
                left(5_000),
                Next::Done(Verdict::Stopped),
                "none listed",
            ),
            (
                &[],
                left(0),
                Next::Done(Verdict::Stopped),
                "none listed, no time left",
            ),
            (
                &[],
                None,
                Next::Done(Verdict::Stopped),
                "none listed, no deadline",
            ),
            (
                &[777],
                left(5_000),
                Next::Done(Verdict::Stopped),
                "only a process it did not signal",
            ),
            (
                &[530],
                left(5_000),
                Next::LookAgain,
                "one listed, time left",
            ),
            (
                &[412, 530],
                left(201),
                Next::LookAgain,
                "both listed, just more than the interval",
            ),
            (
                &[412, 777],
                None,
                Next::LookAgain,
                "one listed, no deadline",
            ),
            (
                &[412],
                left(200),
                Next::Done(Verdict::StillRunning),
                "one listed, exactly the interval left",
            ),
            (
                &[530],
                left(199),
                Next::Done(Verdict::StillRunning),
                "one listed, less than the interval left",
            ),
            (
                &[412, 530],
                left(0),
                Next::Done(Verdict::StillRunning),
                "both listed, no time left",
            ),
            (
                &[530, 777],
                left(1),
                Next::Done(Verdict::StillRunning),
                "one listed beside a new one, a millisecond left",
            ),
            (
                &[412],
                Some(LOOK_INTERVAL + Duration::from_micros(1)),
                Next::LookAgain,
                "one listed, a microsecond more than the interval",
            ),
        ];
        for (listed, time_left, next, case) in cases {
            assert_eq!(after_look(&signalled, listed, time_left), next, "{case}");
        }
    }

    /// A look's verdict: still running when it lists a process the command signalled, stopped
    /// when it lists none of them; a look cut by the deadline leaves the verdict of the look
    /// before it, which `stop` answers.
    #[test]
    fn a_look_is_still_running_only_while_it_lists_a_signalled_process() {
        let signalled = [412, 530];
        assert_eq!(verdict(&signalled, &[530]), Verdict::StillRunning);
        assert_eq!(verdict(&signalled, &[412, 530]), Verdict::StillRunning);
        assert_eq!(verdict(&signalled, &[777, 412]), Verdict::StillRunning);
        assert_eq!(verdict(&signalled, &[]), Verdict::Stopped);
        assert_eq!(verdict(&signalled, &[777]), Verdict::Stopped);
        assert_eq!(verdict(&[], &[412]), Verdict::Stopped);
        // A sequence of looks inside a deadline of one second, each 200 ms after the one before
        // and taking no time: the process is listed at the first three and gone at the fourth.
        let looks: [(&[u32], u64); 4] = [(&[412], 1_000), (&[412], 800), (&[412], 600), (&[], 400)];
        let nexts: Vec<Next> = looks
            .iter()
            .map(|(listed, time_left)| after_look(&[412], listed, left(*time_left)))
            .collect();
        assert_eq!(
            nexts,
            [
                Next::LookAgain,
                Next::LookAgain,
                Next::LookAgain,
                Next::Done(Verdict::Stopped)
            ]
        );
        // The same process never gone: the look with 200 ms left is the last inside the
        // deadline.
        let never_gone: Vec<Next> = [1_000, 800, 600, 400, 200]
            .into_iter()
            .map(|time_left| after_look(&[412], &[412], left(time_left)))
            .collect();
        assert_eq!(
            never_gone,
            [
                Next::LookAgain,
                Next::LookAgain,
                Next::LookAgain,
                Next::LookAgain,
                Next::Done(Verdict::StillRunning)
            ]
        );
    }
}
