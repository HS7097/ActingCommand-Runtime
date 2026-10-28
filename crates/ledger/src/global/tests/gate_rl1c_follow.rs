// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c follow-up (#411 6444982862 L1; 0.12 wave 1 slice W1-9, fix round 1, review
// rd-1028/review-w1-9.md L1): a range read under a deadline (`GlobalLedger::open_range_within`)
// stops wherever the deadline passes, in the middle of its chunked rows and at the head row it
// reads after them included, and every such stop is the request error
// `ledger_read_budget_exceeded` with nothing kept.
//
// The deadline is the fake read clock's (`LedgerReadClock`): it passes after a number of looks.
// The test tries every number below the looks of the whole read, so it does not depend on how
// many looks a row takes; the read counter (`LedgerReadCounter`) tells which event and link rows
// each stopped read had read, and so where its deadline passed. No sleep, no wall clock.

use super::missing_material::formal_root;
use super::*;
use crate::global::{GlobalLedgerRangeRead, GlobalLedgerRangeRequest};
use crate::{LedgerCode, LedgerReadClock, LedgerReadCounter, LedgerSequencesRead};
use std::collections::BTreeSet;

/// The events appended to the root.
const APPENDED: usize = 16;
/// The range read: the rows after `AFTER` through `THROUGH`, several of the tests' chunks of the
/// range read, below the head, so the head row is read after them, on its own.
const AFTER: u64 = 2;
const THROUGH: u64 = 12;

/// Every row of the range, without eviction proofs.
fn request() -> GlobalLedgerRangeRequest {
    GlobalLedgerRangeRequest {
        query: EventQuery::default(),
        after_sequence: AFTER,
        through_sequence: Some(THROUGH),
        matches: usize::MAX,
        eviction_proofs: false,
    }
}

/// One range read whose deadline the fake read clock passes after a number of looks.
struct Attempt {
    /// What the read answered.
    result: GlobalLedgerResult<Option<GlobalLedgerRangeRead>>,
    /// The looks it took at its deadline.
    looks: u64,
    /// The event and link rows it read with their identity before it ended.
    read: LedgerSequencesRead,
}

/// The range read of `root` under a deadline that passes after `ahead` looks.
fn attempt(root: &Path, ahead: u64) -> Attempt {
    let clock = LedgerReadClock::arm(ahead);
    let counter = LedgerReadCounter::start();
    let result = GlobalLedger::open_range_within(root, &request(), clock.deadline());
    let read = counter.sequences_read();
    let looks = clock.looks();
    drop(counter);
    drop(clock);
    Attempt {
        result,
        looks,
        read,
    }
}

#[test]
fn rl1c_follow_l1_a_range_read_stops_at_its_deadline_in_its_chunked_rows_and_at_its_head_row() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    for index in 0..APPENDED {
        ledger
            .append(event(&format!("range-{index}")))
            .expect("history");
    }
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    drop(database);
    assert!(
        head > THROUGH,
        "setup: the head row is read apart from the range: head {head}"
    );
    let range = (AFTER + 1..=THROUGH).collect::<BTreeSet<_>>();

    // A deadline that never passes: the read keeps the range's rows, having read them and the
    // head row.
    let whole = attempt(root.path(), u64::MAX);
    let kept = match whole.result {
        Ok(Some(read)) => read.into_events(),
        Ok(None) => panic!("setup: an SQLite root"),
        Err(error) => panic!("setup: the whole range read answers: {error:?}"),
    };
    assert_eq!(
        kept.iter()
            .map(PersistedEvent::sequence)
            .collect::<BTreeSet<_>>(),
        range,
        "setup: the range's rows"
    );
    assert!(
        range.is_subset(&whole.read.events)
            && range.is_subset(&whole.read.links)
            && whole.read.events.contains(&head),
        "setup: the read reads the range's rows and the head row: {:?}",
        whole.read
    );

    // Every deadline that passes before the read's last look stops it, and nothing is kept; the
    // rows each stopped read had read say where its deadline passed.
    let mut in_the_chunks = 0_u64;
    let mut at_the_head_row = 0_u64;
    let mut in_the_head_row = 0_u64;
    for ahead in 0..whole.looks {
        let stopped = attempt(root.path(), ahead);
        let Err(error) = &stopped.result else {
            panic!(
                "a deadline at {ahead} of {} looks: the read answered",
                whole.looks
            );
        };
        assert!(
            !error.is_fatal() && error.code() == LedgerCode::LedgerReadBudgetExceeded.as_str(),
            "a deadline at {ahead} of {} looks: {error:?}",
            whole.looks
        );
        let rows = stopped.read.events.intersection(&range).count();
        let links = stopped.read.links.intersection(&range).count();
        let head_read = stopped.read.events.contains(&head);
        if rows > 0 && rows < range.len() {
            // Some of the range's rows read, not all: in the chunked rows.
            in_the_chunks += 1;
        } else if rows == range.len() && links == range.len() {
            // Every chunk read with its link rows: at the head row, before its row was read
            // (its statement) or after (its authentication, its link and artifact rows).
            if head_read {
                in_the_head_row += 1;
            } else {
                at_the_head_row += 1;
            }
        }
    }
    assert!(
        in_the_chunks > 0,
        "no deadline passed in the chunked rows ({} looks)",
        whole.looks
    );
    assert!(
        at_the_head_row > 0,
        "no deadline passed at the head row's statement ({} looks)",
        whole.looks
    );
    assert!(
        in_the_head_row > 0,
        "no deadline passed after the head row was read ({} looks)",
        whole.looks
    );
}
