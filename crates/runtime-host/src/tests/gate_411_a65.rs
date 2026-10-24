// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A65 (target v0.12.12; production 10-24 on v0.12.10: the first start after a 37-hour
//! epoch of more than 300k records failed with `ledger_failure` during
//! `settle_previous_epoch_contained_runs`, whose one unpaged read of the whole previous epoch
//! the writer's 1 s read budget stopped; the watchdog's next start passed only because the failed
//! start had appended its own start event, so the earlier epoch's open direct runs were never
//! settled). Requirements of the slice (brief A65, frozen by the coordinator 10-24):
//!
//! - (a) R1: a previous epoch with more records than one unpaged read returns within the test's
//!   read budget, holding one open direct run: the start succeeds and settles that run;
//! - (b) R3: the same open run, then a start that fails after appending its start event, then a
//!   start that succeeds: the run is settled.
//!
//! The previous epoch is the epoch-close child process, killed hard with its direct run in
//! flight. Test (a) restarts under a 1 ms read budget, the smallest
//! `ledger_observer.read_budget_ms` takes, on a writer clock whose readings lie at least 1 ms
//! apart: every look at the clock finds the budget spent, so a writer read that is no page stops
//! at its 256th row (`READ_BUDGET_CHECK_ROWS`) however fast it reads, and "more than one unpaged
//! read returns" is a row count, not a wall clock. Existing tests missed this: the epoch-close
//! gates restart a ledger of a few hundred records under the default 1 s budget, and no test
//! started again after a start that failed.

use super::gate_epoch_close::{
    ALIAS, crash_mid_run_with, epoch_config, run_terminal, start_host_with,
};
use super::instance_workers::closed_ledger_events;
use super::*;
use crate::codes::HostCode;
use crate::codes::test_spellings::TestCode;
use actingcommand_ledger::LedgerMonotonicClock;

/// Test (a): the ordinary records the previous epoch holds before its run, two startup-read
/// pages' worth and more than one unpaged read returns within `READ_BUDGET` on
/// `one_ms_per_reading_clock`.
const FILLER_RECORDS: usize = 2 * crate::recovery_read::RECOVERY_PAGE_EVENTS;

/// Test (a): the restart's read budget, the smallest `ledger_observer.read_budget_ms` takes.
const READ_BUDGET: Duration = Duration::from_millis(1);

/// Test (a): the restart's ledger writer clock, the process's monotonic milliseconds plus one
/// for every reading before. Two readings lie at least 1 ms apart, so a read under
/// `READ_BUDGET` stops at its first look at the clock however fast its rows are read; the close
/// bound and the stall watch, on the same clock, run no slower than the process's clock.
fn one_ms_per_reading_clock() -> LedgerMonotonicClock {
    let origin = Instant::now();
    let readings = Arc::new(AtomicU64::new(0));
    Arc::new(move || {
        let elapsed_ms = u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        elapsed_ms.saturating_add(readings.fetch_add(1, Ordering::SeqCst))
    })
}

/// The settlement the requirement asks for: one terminal of `run`, cancelled with
/// `contained_task_recovered_after_restart`.
fn recovered_after_restart() -> Vec<(TaskOutcome, Option<String>)> {
    vec![(
        TaskOutcome::Cancelled,
        Some(
            HostCode::ContainedTaskRecoveredAfterRestart
                .as_str()
                .to_owned(),
        ),
    )]
}

/// Every task terminal of `run` in the closed ledger of `root`, with its failure code.
fn run_terminals(root: &Path, run: &RunId) -> Vec<(TaskOutcome, Option<String>)> {
    closed_ledger_events(root)
        .iter()
        .filter_map(|event| run_terminal(event, run))
        .map(|(task_result, code)| (task_result, code.map(str::to_owned)))
        .collect()
}

/// The sequences of the start events (`runtime.started`, `runtime.takeover`) among `events`.
fn start_events(events: &[PersistedEvent]) -> Vec<u64> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::RuntimeStarted | EventType::RuntimeTakeover
            )
        })
        .map(PersistedEvent::sequence)
        .collect()
}

/// Test (a), R1: the previous epoch holds more records than one unpaged read returns within the
/// read budget, and one open direct run; the start succeeds and settles the run.
#[test]
fn a65_a_a_previous_epoch_beyond_one_unpaged_read_is_settled_and_the_start_succeeds() {
    let crashed = crash_mid_run_with(FILLER_RECORDS);
    let epoch_start = *start_events(&crashed.prefix)
        .last()
        .expect("the killed epoch's start event");
    let epoch_records = crashed
        .prefix
        .iter()
        .filter(|event| event.sequence() > epoch_start)
        .count();
    assert!(
        epoch_records > FILLER_RECORDS,
        "the previous epoch holds {epoch_records} records"
    );
    assert!(
        run_terminals(crashed.root.path(), &crashed.run_id).is_empty(),
        "the killed run is open"
    );

    let host = start_host_with(
        epoch_config(crashed.root.path())
            .with_ledger_read_budget_for_test(READ_BUDGET, one_ms_per_reading_clock()),
        crashed.instance_id,
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the started Runtime");

    assert_eq!(
        run_terminals(crashed.root.path(), &crashed.run_id),
        recovered_after_restart()
    );
}

/// Test (b), R3: the open run, then a start that fails after appending its start event, then a
/// start that succeeds: the run is settled.
#[test]
fn a65_b_a_start_that_failed_after_its_start_event_leaves_the_open_run_to_the_next_start() {
    let crashed = crash_mid_run_with(0);
    let starts_before = start_events(&crashed.prefix).len();

    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let Err(failed) = RuntimeHost::start(
        epoch_config(crashed.root.path()).with_start_failure_after_start_event_for_test(),
        Arc::new(FakeProvider::one(ALIAS, crashed.instance_id, state)),
    ) else {
        panic!("the start did not fail after its start event");
    };
    assert_eq!(
        failed.code(),
        TestCode::StartFailedAfterStartEventForTest.as_str(),
        "{failed:?}"
    );
    let after_failure = closed_ledger_events(crashed.root.path());
    assert_eq!(
        start_events(&after_failure).len(),
        starts_before + 1,
        "the failed start appended its start event"
    );
    assert!(
        run_terminals(crashed.root.path(), &crashed.run_id).is_empty(),
        "the run is still open"
    );

    let host = start_host_with(epoch_config(crashed.root.path()), crashed.instance_id);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the started Runtime");

    assert_eq!(
        run_terminals(crashed.root.path(), &crashed.run_id),
        recovered_after_restart()
    );
}
