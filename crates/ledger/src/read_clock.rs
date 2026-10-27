// SPDX-License-Identifier: AGPL-3.0-only

//! ROUT-L RL-1c fix 2 (#411 L3): the clock a ledger read's deadline is judged by. It is the wall
//! clock. In tests (`test-hooks`), a [`LedgerReadClock`] armed on a thread replaces it for every
//! deadline that thread judges: the fake clock moves one step at each look and its deadline
//! passes after a fixed number of looks, so a test makes a deadline pass in the middle of a read
//! without a sleep and without reading the wall clock.

use std::time::Instant;

/// Whether `deadline` has passed for a ledger read: the wall clock, or, while a
/// [`LedgerReadClock`] is armed on this thread (tests only), that fake clock.
pub fn read_deadline_passed(deadline: Instant) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        if let Some(passed) = fake::look() {
            return passed;
        }
    }
    Instant::now() >= deadline
}

#[cfg(any(test, feature = "test-hooks"))]
pub use fake::LedgerReadClock;

#[cfg(any(test, feature = "test-hooks"))]
mod fake {
    use std::cell::Cell;
    use std::marker::PhantomData;
    use std::time::Instant;

    /// The armed fake clock of this thread: the looks taken, and how many looks see the
    /// deadline still ahead.
    #[derive(Clone, Copy)]
    struct Armed {
        looks: u64,
        ahead: u64,
    }

    thread_local! {
        static ARMED: Cell<Option<Armed>> = const { Cell::new(None) };
    }

    /// One look at this thread's fake clock: `None` when none is armed, else whether its
    /// deadline has passed (from the look after the `ahead` first ones on).
    pub(super) fn look() -> Option<bool> {
        ARMED.with(|armed| {
            let mut clock = armed.get()?;
            clock.looks = clock.looks.saturating_add(1);
            armed.set(Some(clock));
            Some(clock.looks > clock.ahead)
        })
    }

    /// A fake clock for the ledger reads of this thread, until dropped: every deadline this
    /// thread judges is still ahead at the first `ahead` looks and passed from the next one on,
    /// whatever its value and the wall clock. Neither sleeps nor reads the wall clock while
    /// judging.
    pub struct LedgerReadClock {
        deadline: Instant,
        /// Armed on one thread; dropped there.
        _thread: PhantomData<*const ()>,
    }

    impl LedgerReadClock {
        /// Arms the fake clock on this thread; the deadline passes after `ahead` looks
        /// (`u64::MAX`: never).
        pub fn arm(ahead: u64) -> Self {
            ARMED.with(|armed| armed.set(Some(Armed { looks: 0, ahead })));
            Self {
                // Only a value to hand to a read; the fake clock alone says when it passes.
                deadline: Instant::now(),
                _thread: PhantomData,
            }
        }

        /// A deadline to give the reads this clock judges.
        pub fn deadline(&self) -> Instant {
            self.deadline
        }

        /// The looks taken at this clock so far.
        pub fn looks(&self) -> u64 {
            ARMED.with(|armed| armed.get().map_or(0, |clock| clock.looks))
        }
    }

    impl Drop for LedgerReadClock {
        fn drop(&mut self) {
            ARMED.with(|armed| armed.set(None));
        }
    }
}
