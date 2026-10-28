// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c follow-ups (#411 6444982862 L1, 6445846389 review 2 L1; 0.12 wave 1 slice W1-9):
// `actingd suspended`'s report.
//
// - L1: a run the report shows is read once, within the deadline: a deadline that passes while
//   the run's rows are read leaves the run unread (`step` null, frames `unavailable` with
//   `ledger_read_budget_exceeded`) and the report answers `ledger_read_incomplete`, instead of
//   the read running on until its caller ends the process.
// - Review 2 L1: the walk for the latest daemon start's `config.subsystems` fact reads at most
//   its most rows, and a walk that ends there, or at the deadline, leaves the start unknown
//   (`daemon_start_unknown`, `ledger_read_incomplete`) while the rest of the report answers.
//
// The fixture is RL-1c fix 2's closed state root (`gate_rl1c_fix2::fixture`): two daemon starts'
// `config.subsystems` facts, one paused scheduled run, then `LATER_FACTS` other workbench facts,
// so the walk reads those first. Deadlines are judged by the fake read clock (`LedgerReadClock`),
// which passes after a fixed number of looks; no sleep, and the test reads no wall clock.
//
// Fix round 1 (review rd-1028/review-w1-9.md L1): the L1 test also sets deadlines over the whole
// of the run's read, found by search, not only at one count after the selection. Where in the
// range read a deadline passes (its chunked rows, its head row) is told apart by the rows it had
// read, which only the ledger's own test counter sees: that is the ledger gate
// `gate_rl1c_follow` (`crates/ledger/src/global/tests/gate_rl1c_follow.rs`).

use super::gate_rl1c_fix2::{
    LATER_FACTS, LATEST_START_UNIX_MS, PAUSED_TASK, fixture, old_full_read, report_within, warnings,
};
use super::*;
use crate::SuspendedReadBudget;
use actingcommand_ledger::LedgerReadClock;
use serde_json::{Value, json};

/// The report on `root` under a deadline the fake read clock passes after `ahead` looks.
fn report_at(root: &TempDir, ahead: u64) -> Value {
    let clock = LedgerReadClock::arm(ahead);
    let report = report_within(root, SuspendedReadBudget::until(Some(clock.deadline())));
    drop(clock);
    report
}

/// The looks the whole report takes at its deadline when the deadline never passes.
fn looks_of_whole_report(root: &TempDir) -> u64 {
    let never = LedgerReadClock::arm(u64::MAX);
    let report = report_within(root, SuspendedReadBudget::until(Some(never.deadline())));
    let looks = never.looks();
    drop(never);
    assert_eq!(
        report["daemon_started_at_unix_ms"],
        json!(LATEST_START_UNIX_MS),
        "setup: the whole report finds the latest start: {report:#}"
    );
    looks
}

/// The fewest looks in `0..=high` at which `holds(report)`, which fails at none and holds at
/// `high` and at every count above the fewest.
fn fewest_looks(root: &TempDir, high: u64, holds: impl Fn(&Value) -> bool) -> u64 {
    let (mut low, mut high) = (0_u64, high);
    assert!(
        !holds(&report_at(root, low)),
        "setup: no look holds nothing"
    );
    assert!(holds(&report_at(root, high)), "setup: every look holds it");
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if holds(&report_at(root, middle)) {
            high = middle;
        } else {
            low = middle;
        }
    }
    high
}

/// The most look counts [`spread`] gives: each is one whole report.
const SAMPLES: u64 = 16;

/// Up to [`SAMPLES`] look counts spread evenly over `low..=high` (`low < high`), both ends
/// included; every count when there are no more.
fn spread(low: u64, high: u64) -> Vec<u64> {
    let intervals = (high - low).min(SAMPLES - 1);
    (0..=intervals)
        .map(|step| low + (high - low) * step / intervals)
        .collect()
}

/// The paused pair, which the report lists whatever it left unread.
fn paused_pair(report: &Value) -> &Value {
    let pair = &report["suspended"][0];
    assert_eq!(pair["task_id"], json!(PAUSED_TASK), "{report:#}");
    pair
}

/// L1 (#411 6444982862): the deadline passes two looks after the selection and the daemon start
/// are read, so the paused run's own read starts and is under way when it passes. The run is
/// left unread (`step` null, frames `unavailable` with `ledger_read_budget_exceeded`) and the
/// report warns `ledger_read_incomplete` last; the rest answers (`through_sequence`, the daemon
/// start). A deadline that never passes reads the run.
///
/// Fix round 1 (review rd-1028/review-w1-9.md L1): not only at that look. `read` is the fewest
/// looks at which the run is read, so every look of the run's own read (its range read's extent
/// check, its chunked rows, the head row read after them, and the check after it) comes after
/// `started` and by `read`, however many looks each takes. Deadlines spread over that window,
/// both ends included, each leave the run unread with the rest answered as above.
#[test]
fn rl1c_follow_l1_a_deadline_passing_in_a_runs_read_leaves_the_run_unread() {
    let root = fixture(true);
    let (head, _, _) = old_full_read(&root);
    let looks = looks_of_whole_report(&root);
    let started = fewest_looks(&root, looks, |report| {
        !report["daemon_started_at_unix_ms"].is_null()
    });

    let report = report_at(&root, started + 2);
    assert_eq!(report["through_sequence"], json!(head), "{report:#}");
    assert_eq!(
        report["daemon_started_at_unix_ms"],
        json!(LATEST_START_UNIX_MS),
        "{report:#}"
    );
    let pair = paused_pair(&report);
    assert_eq!(pair["step"], Value::Null, "{report:#}");
    let comparison = &pair["frames"]["comparison"];
    assert_eq!(comparison["status"], json!("unavailable"), "{report:#}");
    assert_eq!(
        comparison["outcome"]["code"],
        json!("ledger_read_budget_exceeded"),
        "{report:#}"
    );
    assert_eq!(
        warnings(&report).last().map(String::as_str),
        Some("ledger_read_incomplete"),
        "{report:#}"
    );

    // Fix round 1 (L1): the whole window of the run's read, not one count in it.
    let read = fewest_looks(&root, looks, |report| {
        !report["suspended"][0]["step"].is_null()
    });
    assert!(
        read > started + 2,
        "the run's read takes looks: the selection by {started}, the run by {read}"
    );
    for ahead in spread(started + 1, read - 1) {
        let report = report_at(&root, ahead);
        assert_eq!(
            report["through_sequence"],
            json!(head),
            "{ahead} looks: {report:#}"
        );
        assert_eq!(
            report["daemon_started_at_unix_ms"],
            json!(LATEST_START_UNIX_MS),
            "{ahead} looks: {report:#}"
        );
        let pair = paused_pair(&report);
        assert_eq!(pair["step"], Value::Null, "{ahead} looks: {report:#}");
        let comparison = &pair["frames"]["comparison"];
        assert_eq!(
            comparison["status"],
            json!("unavailable"),
            "{ahead} looks: {report:#}"
        );
        assert_eq!(
            comparison["outcome"]["code"],
            json!("ledger_read_budget_exceeded"),
            "{ahead} looks: {report:#}"
        );
        assert_eq!(
            warnings(&report).last().map(String::as_str),
            Some("ledger_read_incomplete"),
            "{ahead} looks: {report:#}"
        );
    }

    let whole = report_at(&root, u64::MAX);
    assert!(
        !paused_pair(&whole)["step"].is_null(),
        "the run read has a step: {whole:#}"
    );
    assert!(
        !warnings(&whole).contains(&"ledger_read_incomplete".to_owned()),
        "{whole:#}"
    );
}

/// Review 2 L1 (#411 6445846389), the row bound: a walk for the daemon start that reads its most
/// rows, fewer than the fact rows after the latest start, ends without the start: the report
/// answers `daemon_started_at_unix_ms` null with `daemon_start_unknown` and
/// `ledger_read_incomplete`, and the rest as without the bound (the paused pair with its step).
/// With room for those rows the walk finds the latest start, and the report is the default one.
#[test]
fn rl1c_follow_review2_l1_the_daemon_start_walk_stops_at_its_most_rows() {
    let root = fixture(true);
    let (head, started, _) = old_full_read(&root);
    assert_eq!(started, Some(LATEST_START_UNIX_MS), "setup");
    let later_facts = usize::try_from(LATER_FACTS).expect("a count");

    let bounded = report_within(
        &root,
        SuspendedReadBudget {
            daemon_start_rows: later_facts / 3,
            ..SuspendedReadBudget::until(None)
        },
    );
    assert_eq!(bounded["through_sequence"], json!(head), "{bounded:#}");
    assert_eq!(
        bounded["daemon_started_at_unix_ms"],
        Value::Null,
        "{bounded:#}"
    );
    let warned = warnings(&bounded);
    assert!(
        warned.contains(&"daemon_start_unknown".to_owned()),
        "{bounded:#}"
    );
    assert_eq!(
        warned.last().map(String::as_str),
        Some("ledger_read_incomplete"),
        "{bounded:#}"
    );
    assert!(
        !paused_pair(&bounded)["step"].is_null(),
        "the rest is read: {bounded:#}"
    );

    let room = report_within(
        &root,
        SuspendedReadBudget {
            daemon_start_rows: later_facts + 1,
            ..SuspendedReadBudget::until(None)
        },
    );
    assert_eq!(
        room["daemon_started_at_unix_ms"],
        json!(LATEST_START_UNIX_MS),
        "{room:#}"
    );
    assert!(
        !warnings(&room).iter().any(|warning| {
            warning == "daemon_start_unknown" || warning == "ledger_read_incomplete"
        }),
        "{room:#}"
    );
    assert_eq!(
        room,
        report_within(&root, SuspendedReadBudget::until(None)),
        "the default bound reads as much"
    );
}

/// Review 2 L1 (#411 6445846389), the deadline: at the fewest looks at which the report keeps
/// what it read before the walk for the daemon start (`through_sequence`), the deadline passes
/// in that walk, before it read the fact rows after the latest start. The report answers the
/// rest (the head, the paused pair), with `daemon_started_at_unix_ms` null,
/// `daemon_start_unknown` and `ledger_read_incomplete` last, instead of answering empty lists.
#[test]
fn rl1c_follow_review2_l1_a_deadline_in_the_daemon_start_walk_keeps_the_rest() {
    let root = fixture(true);
    let (head, _, _) = old_full_read(&root);
    let looks = looks_of_whole_report(&root);
    let kept = fewest_looks(&root, looks, |report| !report["through_sequence"].is_null());
    assert!(
        looks > kept + LATER_FACTS,
        "the walk had not read the fact rows after the latest start: {kept} of {looks} looks"
    );

    let report = report_at(&root, kept);
    assert_eq!(report["through_sequence"], json!(head), "{report:#}");
    assert_eq!(
        report["daemon_started_at_unix_ms"],
        Value::Null,
        "{report:#}"
    );
    paused_pair(&report);
    let warned = warnings(&report);
    assert!(
        warned.contains(&"daemon_start_unknown".to_owned()),
        "{report:#}"
    );
    assert_eq!(
        warned.last().map(String::as_str),
        Some("ledger_read_incomplete"),
        "{report:#}"
    );
}
