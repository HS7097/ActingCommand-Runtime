// SPDX-License-Identifier: AGPL-3.0-only

// #503 CYC-3 (0.12.86; register §1.529, the coordinator's rulings 1 to 4; ledger revision 10), the
// host's half, on the remedy fixture of `gate_411_b15h.rs` and the catalog generations, the cycle
// and the delays of `gate_503_cyc2.rs`; no clock read and no sleep.
// - t1 (ruling 1): an admission records the end of the activity window it was admitted in
//   (`window_until_unix_ms`), the completion repeats it, and it is the end the run's own catalog
//   generation gives (`admitted_window_end`).
// - t2 (ruling 2): after a restart on the same state root, with the run's generation in its files
//   alone (a memo of one generation and two later generations activated), the first evaluated
//   cycle reads no generation for the remedy journal (the delay planned for a read stays untaken;
//   until 0.12.86 this cycle read the run's generation), nothing is remembered for the run, and
//   the journal's window end of the run is the recorded one.
// - t3 (rulings 2 and 4): a run whose admission record carries no window end, as 0.12.85 wrote
//   it, beside one that carries it in the same journal: a start opens the ledger that holds both;
//   the first cycle after it reads the older run's generation once and the next reads none; only
//   the older run's end is remembered, and each run's end is what its generation gives.

use super::gate_411_b15h::{Fixture, MAIN_TASK, RESULT, WINDOW_END_UNIX_MS, next_eligible};
use super::gate_503_cyc2::{DELAY, DELAY_MICROS, catalog_version, remedy_cycle, remedy_fixture};
use super::*;
use crate::policy_control::admit_without_window_end_for_test;
use crate::policy_host::{
    CycleStretch, plan_cycle_stretch_delay_for_test, take_cycle_stretch_delay_for_test,
};
use crate::{PolicyCycle, SetupCall};
use actingcommand_contract::PolicyAdmissionRecord;

/// The admission the one record of `event_type` (`policy.dispatch_admitted` or
/// `policy.dispatch_completed`) of the run `decision_id` carries, as the ledger holds it.
fn recorded_admission(
    host: &RuntimeHost,
    event_type: EventType,
    decision_id: &str,
) -> PolicyAdmissionRecord {
    let records = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(event_type),
            ..EventQuery::default()
        })
        .expect("the dispatch records");
    let admissions = records
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(
                PolicyPayload::DispatchAdmitted(payload)
                | PolicyPayload::DispatchCompleted(payload),
            ) if payload.decision_id() == decision_id => payload.admission().cloned(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admissions.len(),
        1,
        "{event_type:?} of {decision_id}: {admissions:?}"
    );
    admissions
        .into_iter()
        .next()
        .expect("the record's admission")
}

/// The host started again on the fixture's state root at `at`, with a memo of one catalog
/// generation, and the fixture catalog's versions 2 and 3 activated after it: the memo holds one
/// of those two (an activation reads the generation it replaces, so one activation would leave
/// the fixture's first generation in the memo of one), and the first generation, the runs', is in
/// its files alone.
fn restarted_with_a_memo_of_one(fixture: Fixture, at: u64) -> Fixture {
    let fixture = fixture.restart(at);
    fixture
        .host
        .set_catalog_memo_capacity_for_test(1)
        .expect("a memo of one generation");
    for version in [2, 3] {
        fixture
            .host
            .activate_policy_catalog(&catalog_version(version))
            .expect("activate a later generation");
    }
    fixture
}

/// The microseconds a cycle's setup spent reading generations from their files.
fn catalog_read_micros(cycle: PolicyCycle) -> u64 {
    cycle
        .measurement
        .expect("an evaluated cycle carries its measurement")
        .stretches
        .evaluation_setup_calls
        .micros(SetupCall::CatalogReads)
}

#[test]
fn cyc3_t1_an_admission_records_the_end_of_its_window_which_its_generation_gives() {
    let fixture = remedy_fixture();
    let (_, _, run) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, RESULT);
    let decision_id = run.decision_id().to_owned();
    let admitted = recorded_admission(
        &fixture.host,
        EventType::PolicyDispatchAdmitted,
        &decision_id,
    );
    assert_eq!(
        admitted.activity.window_until_unix_ms,
        Some(WINDOW_END_UNIX_MS),
        "the admission records the end of the window it names: {admitted:?}"
    );
    assert_eq!(
        recorded_admission(
            &fixture.host,
            EventType::PolicyDispatchCompleted,
            &decision_id
        ),
        admitted,
        "the completion repeats the admission unchanged"
    );
    assert_eq!(
        run.admission(),
        &admitted,
        "the run's context holds the record"
    );
    // The remedy journal's run: the end the journal gives it and the end its own generation gives
    // (`admitted_window_end`), both the recorded one.
    assert_eq!(
        fixture
            .host
            .remedy_journal_window_ends_for_test()
            .expect("the remedy journal's window ends"),
        vec![(decision_id, WINDOW_END_UNIX_MS, WINDOW_END_UNIX_MS)]
    );
    fixture.host.close().expect("close the runtime host");
}

#[test]
fn cyc3_t2_after_a_restart_the_first_cycle_reads_no_generation_for_a_recorded_window_end() {
    let fixture = remedy_fixture();
    let (_, _, run) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, RESULT);
    let decision_id = run.decision_id().to_owned();
    let recorded = recorded_admission(
        &fixture.host,
        EventType::PolicyDispatchAdmitted,
        &decision_id,
    )
    .activity
    .window_until_unix_ms
    .expect("the admission records its window end");

    // A restart, and the run's generation in its files alone: until 0.12.86 the first cycle read
    // it for the remedy journal, whose memory of window ends a start leaves empty.
    let at = next_eligible(&run);
    let fixture = restarted_with_a_memo_of_one(fixture, at);
    plan_cycle_stretch_delay_for_test(CycleStretch::CatalogRead, DELAY);
    let first = remedy_cycle(&fixture, at, 9);
    assert_eq!(
        take_cycle_stretch_delay_for_test(CycleStretch::CatalogRead),
        Some(DELAY),
        "the first cycle after the restart read no generation from its files"
    );
    assert_eq!(catalog_read_micros(first), 0);
    let kept = fixture
        .host
        .remedy_window_ends_for_test()
        .expect("the window ends the remedy journal keeps");
    assert!(
        kept.is_empty(),
        "nothing is remembered for the run: {kept:?}"
    );

    // The premise: the run's generation is in its files alone, so reading it takes the delay. The
    // journal's end of the run is the recorded one, which the generation gives too.
    plan_cycle_stretch_delay_for_test(CycleStretch::CatalogRead, DELAY);
    let ends = fixture
        .host
        .remedy_journal_window_ends_for_test()
        .expect("the remedy journal's window ends");
    assert_eq!(
        take_cycle_stretch_delay_for_test(CycleStretch::CatalogRead),
        None,
        "the run's generation was read from its files"
    );
    assert_eq!(ends, vec![(decision_id, recorded, recorded)]);
    assert_eq!(recorded, WINDOW_END_UNIX_MS);
    fixture.host.close().expect("close the runtime host");
}

#[test]
fn cyc3_t3_a_run_admitted_without_a_window_end_is_read_once_beside_one_admitted_with_it() {
    let fixture = remedy_fixture();
    // The earlier run is admitted as 0.12.85 admitted every run: its record carries no window end.
    admit_without_window_end_for_test(true);
    let (_, _, earlier) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, RESULT);
    admit_without_window_end_for_test(false);
    let (_, _, later) = fixture.evaluate_and_run(next_eligible(&earlier), 8, MAIN_TASK, RESULT);
    let (earlier_id, later_id) = (
        earlier.decision_id().to_owned(),
        later.decision_id().to_owned(),
    );
    for event_type in [
        EventType::PolicyDispatchAdmitted,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            recorded_admission(&fixture.host, event_type, &earlier_id)
                .activity
                .window_until_unix_ms,
            None,
            "{event_type:?} of the earlier run"
        );
        assert_eq!(
            recorded_admission(&fixture.host, event_type, &later_id)
                .activity
                .window_until_unix_ms,
            Some(WINDOW_END_UNIX_MS),
            "{event_type:?} of the later run"
        );
    }

    // A start opens the ledger that holds both kinds (its replay checks each admission); then the
    // runs' generation is in its files alone.
    let at = next_eligible(&later);
    let fixture = restarted_with_a_memo_of_one(fixture, at);
    plan_cycle_stretch_delay_for_test(CycleStretch::CatalogRead, DELAY);
    let reading = remedy_cycle(&fixture, at, 9);
    assert_eq!(
        take_cycle_stretch_delay_for_test(CycleStretch::CatalogRead),
        None,
        "the first cycle read the earlier run's generation from its files"
    );
    assert!(catalog_read_micros(reading) >= DELAY_MICROS);

    // Another generation is activated, so the runs' generation, which that read put in the memo of
    // one, is in its files alone again; the next cycle reads none: the earlier run's end is
    // remembered, the later run's is its record's.
    fixture
        .host
        .activate_policy_catalog(&catalog_version(4))
        .expect("activate the fourth generation");
    plan_cycle_stretch_delay_for_test(CycleStretch::CatalogRead, DELAY);
    let quiet = remedy_cycle(&fixture, at + 60_000, 10);
    assert_eq!(
        take_cycle_stretch_delay_for_test(CycleStretch::CatalogRead),
        Some(DELAY),
        "no generation read from its files in the next cycle"
    );
    assert_eq!(catalog_read_micros(quiet), 0);

    assert_eq!(
        fixture
            .host
            .remedy_window_ends_for_test()
            .expect("the window ends the remedy journal keeps"),
        vec![(earlier_id.clone(), WINDOW_END_UNIX_MS, WINDOW_END_UNIX_MS)],
        "only the run without a recorded end is remembered"
    );
    assert_eq!(
        fixture
            .host
            .remedy_journal_window_ends_for_test()
            .expect("the remedy journal's window ends"),
        vec![
            (earlier_id, WINDOW_END_UNIX_MS, WINDOW_END_UNIX_MS),
            (later_id, WINDOW_END_UNIX_MS, WINDOW_END_UNIX_MS),
        ]
    );
    fixture.host.close().expect("close the runtime host");
}
