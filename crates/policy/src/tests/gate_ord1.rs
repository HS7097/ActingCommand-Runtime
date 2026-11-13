// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 ORD-1 (register §1.350; the owner's rulings on board #437 as the coordinator
// recorded them, not the owner's words; contracts/scheduling/v2/README.md;
// contracts/standard-pack-schedule.md, "`requires`"):
// - ORD-1a: a V2 task may declare `max_queue_wait_ms`, the longest time its candidate may wait in
//   its device's queue, from one minute to one day; a V1 document that names it, a value of
//   another type and a value outside the range are refused at the member. A `schedule.json` rule
//   writes it as any V2 member, it passes through to the composed task, and the file names
//   `queue_wait` in `requires`. A catalog without it keeps its golden hashes.
// - ORD-1b (contracts/scheduling/README.md, "Order within one device"; contracts/device-group.md,
//   "Ranking across members"): with the order inputs, a device of several stays with the member
//   in front while one of its contenders, in its own order, ends by the binding waiter's deadline
//   (the earliest deadline among the other members' contenders, then the allocation order), and
//   otherwise the binding waiter goes. A pick its admission defers gives way and the selection
//   runs again; the device's chain successor of the front goes first whether or not it fits, a
//   promoted request of the front only when it fits. Held members have no waiting task, and a
//   waiting member's fact refresh waits with the default limit. Every contender that does not run
//   carries the order's values on `instance_already_selected` or `device_group_waiting`
//   `selected`. An instance alone on its device never reads a declared limit.
//
// Every catalog is neutral; nothing reads a clock. Members A, B and C share the device `DEVICE`;
// times are minutes around the fixture's NOW; every task is hourly and due at NOW.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::codes::DeviceGroupWait;
use crate::{
    CandidateEligibility, CappedMemory, CatalogCompileFailure, CatalogDiagnosticCode,
    CatalogDocumentSource, CatalogSources, ChainHandoverInputs, CompiledCatalog,
    CompletedActivityWindow, DEFAULT_MAX_QUEUE_WAIT_MS, DecisionReason, DeviceGroups, DeviceOrder,
    DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FACT_REFRESH_EXPECTED_DURATION_MS, FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshReader,
    FactRefreshSettings, HostResourceSnapshot, InstanceHold, InstanceHolds, InstancePoolLines,
    LeftOutFacts, NoProgressGuards, OwedRerun, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, QUEUE_WAIT_SUBSYSTEM, RemedyPlan, RestingPage, RunSlots,
    SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState,
    StandardPackBinding, StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, compile_catalog,
    compose_standard_pack_rules, evaluate_with_device_groups,
};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    BundleIndex, StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
/// The task member ORD-1a adds.
const MEMBER: &str = "max_queue_wait_ms";
/// The neutral example catalog's golden identities (as in `gate_sched_c1a`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// ORD-1a test 1, the compiler. In a V1 document the member is `unknown_field`; a string, `null`
// and a fraction are `type_mismatch`; 0, 59 999, 86 400 001 and 2^53 − 1 are `limit_exceeded`,
// each at `/tasks/0/max_queue_wait_ms`. The bounds 60 000 and 86 400 000 compile, and the compiled
// task carries the value written.
#[test]
fn ord1a_t1_the_compiler_holds_max_queue_wait_ms_to_v2_and_its_range() {
    let pointer = format!("/tasks/0/{MEMBER}");
    let failure = compile_catalog(&sources(
        SCHEDULING_SCHEMA_VERSION,
        vec![with(
            task_body("task.v1", "instance-a"),
            MEMBER,
            json!(600_000),
        )],
    ))
    .expect_err("V1 names no max_queue_wait_ms");
    assert!(
        refused_at(&failure, CatalogDiagnosticCode::UnknownField, &pointer),
        "{:?}",
        failure.diagnostics()
    );

    let refused = |case: &str, value: Value, code: CatalogDiagnosticCode| {
        let failure = compile_catalog(&sources(
            SCHEDULING_SCHEMA_VERSION_V2,
            vec![with(task_body("task.v2", "instance-a"), MEMBER, value)],
        ))
        .expect_err(case);
        assert!(
            refused_at(&failure, code, &pointer),
            "{case}: {:?}",
            failure.diagnostics()
        );
    };
    for (case, value) in [
        ("a string", json!("600000")),
        ("null", Value::Null),
        ("a fraction", json!(1.5)),
    ] {
        refused(case, value, CatalogDiagnosticCode::TypeMismatch);
    }
    for value in [0_u64, 59_999, 86_400_001, 9_007_199_254_740_991] {
        refused(
            &format!("{value}"),
            json!(value),
            CatalogDiagnosticCode::LimitExceeded,
        );
    }

    let compiled = compile_catalog(&sources(
        SCHEDULING_SCHEMA_VERSION_V2,
        vec![
            with(task_body("task.least", "instance-a"), MEMBER, json!(60_000)),
            with(
                task_body("task.most", "instance-a"),
                MEMBER,
                json!(86_400_000),
            ),
            task_body("task.none", "instance-a"),
        ],
    ))
    .unwrap_or_else(|failure| panic!("the bounds compile: {:?}", failure.diagnostics()));
    let limits = compiled
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task.max_queue_wait_ms))
        .collect::<Vec<_>>();
    assert_eq!(
        limits,
        [
            ("task.least", Some(60_000)),
            ("task.most", Some(86_400_000)),
            ("task.none", None),
        ]
    );
}

// ORD-1a test 2, the composer. A rule's `max_queue_wait_ms` passes through to its composed task
// as written, and a rule without it composes none. Refused: a file that writes it without naming
// `queue_wait` in `requires`, `schedule_invalid` at the first such rule's member; a value the
// compiler refuses, mapped back to the rule's member; and, on a Runtime whose manifest does not
// name `queue_wait`, the file at `requires` (`schedule_requires_missing`).
#[test]
fn ord1a_t2_the_composer_passes_it_through_and_refuses_it_without_requires() {
    let requires = [QUEUE_WAIT_SUBSYSTEM];
    let rules = || {
        vec![
            schedule_rule("rule.plain"),
            with(schedule_rule("rule.waits"), MEMBER, json!(600_000)),
        ]
    };
    let composed = compose(&schedule(rules(), &requires), None)
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let limits = composed
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task.max_queue_wait_ms))
        .collect::<Vec<_>>();
    assert_eq!(
        limits,
        [
            ("rule.plain".to_owned(), None),
            ("rule.waits".to_owned(), Some(600_000)),
        ]
    );
    let tasks = serde_json::from_slice::<Value>(&composed.sources().tasks.bytes)
        .expect("the composed tasks document");
    assert_eq!(tasks["tasks"][1][MEMBER], json!(600_000), "{tasks}");
    assert!(tasks["tasks"][0].get(MEMBER).is_none(), "{tasks}");

    // `requires` does not name the subsystem: the first rule that writes the member.
    let failure = compose(&schedule(rules(), &[]), None).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, format!("/rules/1/{MEMBER}"));

    // A value the compiler refuses, mapped back to the rule.
    let mut short = rules();
    short[1][MEMBER] = json!(59_999);
    let failure = compose(&schedule(short, &requires), None).expect_err("out of range");
    assert!(
        failure.issues().iter().any(|refused| {
            refused.issue.problem == StandardPackProblem::ScheduleInvalid
                && refused.issue.pointer == format!("/rules/1/{MEMBER}")
        }),
        "{failure}"
    );

    // A Runtime whose manifest does not name the subsystem refuses the file at `requires`.
    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    let failure = compose(&schedule(rules(), &requires), Some(&older)).expect_err("older");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/0");
}

// ORD-1a test 3, identity. The neutral example catalog keeps its V1 and V2 golden identities, and
// a V2 catalog whose tasks do not write the member serializes none.
#[test]
fn ord1a_t3_without_the_member_every_catalog_hash_is_unchanged() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
        let text = serde_json::to_string(&example.catalog().tasks.tasks).expect("tasks");
        assert!(!text.contains(MEMBER), "{version}: {text}");
    }
    let plain = compile_catalog(&sources(
        SCHEDULING_SCHEMA_VERSION_V2,
        vec![task_body("task.plain", "instance-a")],
    ))
    .unwrap_or_else(|failure| panic!("the plain catalog compiles: {:?}", failure.diagnostics()));
    let text = serde_json::to_string(&plain.catalog().tasks.tasks).expect("tasks");
    assert!(!text.contains(MEMBER), "{text}");
}

// ORD-1b test 1. A is in front with a1, five minutes long; B's b1, first in the allocation order,
// has waited 20 of its 30 minutes. a1 ends before b1's limit, so the device stays with A, and b1
// waits with its limit and its ten minutes left. Without the order inputs b1, first in the
// allocation order, wins the device as MAPP-1 built it.
#[test]
fn ord1b_t1_a_front_task_that_fits_goes_before_a_waiting_task() {
    let tasks = || vec![task(A1, A, 100, minutes(5)), task(B1, B, 900, MINUTE_MS)];
    let plain = World::new(tasks(), &[A, B], device(&[A, B])).round().0;
    assert_eq!(dispatched(&plain), [(B1, B)]);
    let order = DeviceOrder::default()
        .front(A)
        .wait_start(B1, B, NOW - minutes(20));
    let evaluation = World::new(tasks(), &[A, B], ordered(&[A, B], order))
        .round()
        .0;
    assert_eq!(dispatched(&evaluation), [(A1, A)]);
    assert_waits(&evaluation, B1, B, (A, A1), limit(minutes(30), minutes(10)));
}

// ORD-1b test 2. A is in front with a1 (20 minutes), a2 (4) and a3 (1) in its own order; B's b1 has
// five minutes left. a1 does not fit and is passed over with its 20 minutes and b1's five; a2, the
// first that fits in A's order, goes, not the shortest; a3 waits behind it as before.
#[test]
fn ord1b_t2_the_first_front_task_that_fits_in_its_own_order_wins() {
    let order = DeviceOrder::default()
        .front(A)
        .wait_start(B1, B, NOW - minutes(25));
    let evaluation = World::new(
        vec![
            task(A1, A, 300, minutes(20)),
            task(A2, A, 200, minutes(4)),
            task(A3, A, 100, MINUTE_MS),
            task(B1, B, 900, MINUTE_MS),
        ],
        &[A, B],
        ordered(&[A, B], order),
    )
    .round()
    .0;
    assert_eq!(dispatched(&evaluation), [(A2, A)]);
    assert_behind(&evaluation, A1, A, Some(fit(minutes(20), minutes(5))));
    assert_behind(&evaluation, A3, A, None);
    assert_waits(&evaluation, B1, B, (A, A2), limit(minutes(30), minutes(5)));
}

// ORD-1b test 3. A is in front with a1 (20 minutes); B's b1, earlier in the allocation order, has
// ten minutes left and C's c1 five. Nothing of A ends in time, so the earliest limit goes: c1. a1
// waits with its 20 minutes and c1's five, b1 with its limit and its ten.
#[test]
fn ord1b_t3_nothing_of_the_front_fits_the_earliest_limit_goes() {
    let order = DeviceOrder::default()
        .front(A)
        .wait_start(B1, B, NOW - minutes(20))
        .wait_start(C1, C, NOW - minutes(25));
    let evaluation = World::new(
        vec![
            task(A1, A, 100, minutes(20)),
            task(B1, B, 900, MINUTE_MS),
            task(C1, C, 500, MINUTE_MS),
        ],
        &[A, B, C],
        ordered(&[A, B, C], order),
    )
    .round()
    .0;
    assert_eq!(dispatched(&evaluation), [(C1, C)]);
    assert_waits(&evaluation, A1, A, (C, C1), fit(minutes(20), minutes(5)));
    assert_waits(&evaluation, B1, B, (C, C1), limit(minutes(30), minutes(10)));
}

// ORD-1b test 4. A is in front with a1 (one minute); B's b1 is ten minutes past its limit and C's
// c1, earlier in the allocation order, five. A run is never interrupted, so a task past its limit
// goes at the next free round: the most overdue, b1. Both waiting rows show no time left.
#[test]
fn ord1b_t4_a_waiting_task_past_its_limit_goes_first() {
    let order = DeviceOrder::default()
        .front(A)
        .wait_start(B1, B, NOW - minutes(40))
        .wait_start(C1, C, NOW - minutes(35));
    let evaluation = World::new(
        vec![
            task(A1, A, 100, MINUTE_MS),
            task(B1, B, 500, MINUTE_MS),
            task(C1, C, 900, MINUTE_MS),
        ],
        &[A, B, C],
        ordered(&[A, B, C], order),
    )
    .round()
    .0;
    assert_eq!(dispatched(&evaluation), [(B1, B)]);
    assert_waits(&evaluation, A1, A, (B, B1), fit(MINUTE_MS, 0));
    assert_waits(&evaluation, C1, C, (B, B1), limit(minutes(30), 0));
}

// ORD-1b test 5. No member is in front and no wait has a start. With every limit the default every
// deadline is the same, so the allocation order decides: a1, MAPP-1's pick. When c1, last in the
// allocation order, declares ten minutes, its limit ends first and it goes.
#[test]
fn ord1b_t5_the_front_unknown_takes_the_earliest_limit_then_the_allocation_order() {
    let tasks = |c1: Value| vec![task(A1, A, 900, MINUTE_MS), task(B1, B, 500, MINUTE_MS), c1];
    let c1 = || task(C1, C, 100, MINUTE_MS);
    let plain = World::new(tasks(c1()), &[A, B, C], device(&[A, B, C]))
        .round()
        .0;
    let evaluation = World::new(
        tasks(c1()),
        &[A, B, C],
        ordered(&[A, B, C], DeviceOrder::default()),
    )
    .round()
    .0;
    assert_eq!(dispatched(&plain), [(A1, A)]);
    assert_eq!(dispatched(&evaluation), [(A1, A)]);
    assert_waits(&evaluation, B1, B, (A, A1), limit(minutes(30), minutes(30)));
    assert_waits(&evaluation, C1, C, (A, A1), limit(minutes(30), minutes(30)));

    let declared = World::new(
        tasks(limited(c1(), minutes(10))),
        &[A, B, C],
        ordered(&[A, B, C], DeviceOrder::default()),
    )
    .round()
    .0;
    assert_eq!(dispatched(&declared), [(C1, C)]);
    assert_waits(&declared, A1, A, (C, C1), limit(minutes(30), minutes(30)));
    assert_waits(&declared, B1, B, (C, C1), limit(minutes(30), minutes(30)));
}

// ORD-1b test 6. A is in front with a1 (20 minutes); C's c1 has five minutes left and B's b1 25.
// Nothing of A ends by c1's limit, so c1 is the switch target, and its admission defers it: it
// keeps its own deferral, and the selection runs again without it. Against b1's 25 minutes a1
// fits and goes. The admission was asked about c1, then a1.
#[test]
fn ord1b_t6_a_deferred_switch_target_gives_way_and_the_selection_runs_again() {
    let order = DeviceOrder::default()
        .front(A)
        .wait_start(B1, B, NOW - minutes(5))
        .wait_start(C1, C, NOW - minutes(25));
    let mut world = World::new(
        vec![
            task(A1, A, 100, minutes(20)),
            task(B1, B, 500, MINUTE_MS),
            task(C1, C, 900, MINUTE_MS),
        ],
        &[A, B, C],
        ordered(&[A, B, C], order),
    );
    world.deferred = vec![C1];
    let (evaluation, asked) = world.round();
    assert_eq!(asked, [C1, A1]);
    assert_eq!(dispatched(&evaluation), [(A1, A)]);
    let deferred = decision(&evaluation, C1, C);
    assert_eq!(deferred.state, SchedulingDecisionState::Deferred);
    assert_eq!(deferred.reasons.last(), Some(&not_loaded()));
    assert_waits(&evaluation, B1, B, (A, A1), limit(minutes(30), minutes(25)));
}

/// One row of test 7: whether A is in front, a2's duration, b1's start, the tasks the admission
/// defers, and what the round must show.
struct Row7 {
    name: &'static str,
    front: bool,
    a2_ms: u64,
    b1_start: u64,
    deferred: &'static [&'static str],
    check: fn(&PolicyEvaluation),
}

// ORD-1b test 7, the design note's table (minutes; A in front; the default 30; A's own order a1,
// a2): a1 20, a2 5; b1 waited 15 (its limit ends at +15); c1 waited 5 of its declared 12 (+7); c2
// waited 25 of its declared 60 (+35). The allocation order is a1, a2, b1, c1, c2. Each row's
// evaluation is then compared byte for byte under all six orders of the three instances in the
// facts, with the task snapshots as given and reversed.
#[test]
fn ord1b_t7_three_members_crossed_limits_and_durations_select_alike_in_every_input_order() {
    let rows = [
        // c1 binds (+7): a1 does not fit, a2 does.
        Row7 {
            name: "1: as given",
            front: true,
            a2_ms: minutes(5),
            b1_start: NOW - minutes(15),
            deferred: &[],
            check: |evaluation| {
                assert_eq!(dispatched(evaluation), [(A2, A)]);
                assert_behind(evaluation, A1, A, Some(fit(minutes(20), minutes(7))));
                assert_waits(evaluation, B1, B, (A, A2), limit(minutes(30), minutes(15)));
                assert_waits(evaluation, C1, C, (A, A2), limit(minutes(12), minutes(7)));
                assert_waits(evaluation, C2, C, (A, A2), limit(minutes(60), minutes(35)));
            },
        },
        // a2 needs 8: nothing of A fits, and c1 goes, not b1 or c2, which waited longer.
        Row7 {
            name: "2: a2 eight minutes",
            front: true,
            a2_ms: minutes(8),
            b1_start: NOW - minutes(15),
            deferred: &[],
            check: |evaluation| {
                assert_eq!(dispatched(evaluation), [(C1, C)]);
                assert_waits(evaluation, A1, A, (C, C1), fit(minutes(20), minutes(7)));
                assert_waits(evaluation, A2, A, (C, C1), fit(minutes(8), minutes(7)));
                assert_waits(evaluation, B1, B, (C, C1), limit(minutes(30), minutes(15)));
                assert_behind(evaluation, C2, C, None);
            },
        },
        // Row 2 with c1 deferred by its admission: b1 binds (+15), and a2 fits.
        Row7 {
            name: "3: row 2, c1 deferred",
            front: true,
            a2_ms: minutes(8),
            b1_start: NOW - minutes(15),
            deferred: &[C1],
            check: |evaluation| {
                assert_eq!(dispatched(evaluation), [(A2, A)]);
                let deferred = decision(evaluation, C1, C);
                assert_eq!(deferred.state, SchedulingDecisionState::Deferred);
                assert_eq!(deferred.reasons.last(), Some(&not_loaded()));
                assert_behind(evaluation, A1, A, Some(fit(minutes(20), minutes(15))));
                assert_waits(evaluation, B1, B, (A, A2), limit(minutes(30), minutes(15)));
                assert_waits(evaluation, C2, C, (A, A2), limit(minutes(60), minutes(35)));
            },
        },
        // Row 1 with no member in front: every contender waits, a1 and a2 from now; c1 binds.
        Row7 {
            name: "4: row 1, the front unknown",
            front: false,
            a2_ms: minutes(5),
            b1_start: NOW - minutes(15),
            deferred: &[],
            check: |evaluation| {
                assert_eq!(dispatched(evaluation), [(C1, C)]);
                assert_waits(evaluation, A1, A, (C, C1), limit(minutes(30), minutes(30)));
                assert_waits(evaluation, A2, A, (C, C1), limit(minutes(30), minutes(30)));
                assert_waits(evaluation, B1, B, (C, C1), limit(minutes(30), minutes(15)));
                assert_behind(evaluation, C2, C, None);
            },
        },
        // Row 1 with b1 waited 40: past its limit (−10), it goes.
        Row7 {
            name: "5: row 1, b1 past its limit",
            front: true,
            a2_ms: minutes(5),
            b1_start: NOW - minutes(40),
            deferred: &[],
            check: |evaluation| {
                assert_eq!(dispatched(evaluation), [(B1, B)]);
                assert_waits(evaluation, A1, A, (B, B1), fit(minutes(20), 0));
                assert_waits(evaluation, A2, A, (B, B1), fit(minutes(5), 0));
                assert_waits(evaluation, C1, C, (B, B1), limit(minutes(12), minutes(7)));
                assert_waits(evaluation, C2, C, (B, B1), limit(minutes(60), minutes(35)));
            },
        },
        // b1 and c1 both end at +7 and nothing of A fits (a2 eight minutes, as row 2): b1, earlier
        // in the allocation order, goes.
        Row7 {
            name: "6: b1 and c1 both end at +7",
            front: true,
            a2_ms: minutes(8),
            b1_start: NOW - minutes(23),
            deferred: &[],
            check: |evaluation| {
                assert_eq!(dispatched(evaluation), [(B1, B)]);
                assert_waits(evaluation, A1, A, (B, B1), fit(minutes(20), minutes(7)));
                assert_waits(evaluation, A2, A, (B, B1), fit(minutes(8), minutes(7)));
                assert_waits(evaluation, C1, C, (B, B1), limit(minutes(12), minutes(7)));
                assert_waits(evaluation, C2, C, (B, B1), limit(minutes(60), minutes(35)));
            },
        },
    ];
    for row in &rows {
        let world = |instances: &[&'static str], reversed: bool| {
            let mut order = DeviceOrder::default()
                .wait_start(B1, B, row.b1_start)
                .wait_start(C1, C, NOW - minutes(5))
                .wait_start(C2, C, NOW - minutes(25));
            if row.front {
                order = order.front(A);
            }
            let mut world = World::new(
                vec![
                    task(A1, A, 900, minutes(20)),
                    task(A2, A, 800, row.a2_ms),
                    task(B1, B, 700, MINUTE_MS),
                    limited(task(C1, C, 600, MINUTE_MS), minutes(12)),
                    limited(task(C2, C, 500, MINUTE_MS), minutes(60)),
                ],
                instances,
                ordered(&[A, B, C], order),
            );
            world.snapshots = [(A1, A), (A2, A), (B1, B), (C1, C), (C2, C)]
                .into_iter()
                .map(|(task_id, alias)| earlier_run(task_id, alias))
                .collect();
            if reversed {
                world.snapshots.reverse();
            }
            world.deferred = row.deferred.to_vec();
            world
        };
        let base = world(&[A, B, C], false).round().0;
        (row.check)(&base);
        let bytes = serde_json::to_vec(&base).expect("the evaluation serializes");
        for instances in [
            [A, B, C],
            [A, C, B],
            [B, A, C],
            [B, C, A],
            [C, A, B],
            [C, B, A],
        ] {
            for reversed in [false, true] {
                let other = world(&instances, reversed).round().0;
                assert_eq!(
                    serde_json::to_vec(&other).expect("the evaluation serializes"),
                    bytes,
                    "{}: {instances:?}, snapshots reversed: {reversed}",
                    row.name
                );
            }
        }
    }
}

// ORD-1b test 8. A is in front with a1 (20 minutes); B's b1 is ten minutes past its limit, so
// unheld it goes. A held member has no contender and so no waiting task: under a switch cooldown
// (a whole-instance hold) a1 goes and b1 waits with the hold's reason and no wake; under a pause or
// a person's hold as the cycle starts a1 goes and b1 waits behind it `selected`, without the
// order's values.
#[test]
fn ord1b_t8_a_held_member_has_no_waiting_task() {
    let world = || {
        World::new(
            vec![task(A1, A, 900, minutes(20)), task(B1, B, 100, MINUTE_MS)],
            &[A, B],
            ordered(
                &[A, B],
                DeviceOrder::default()
                    .front(A)
                    .wait_start(B1, B, NOW - minutes(40)),
            ),
        )
    };
    let free = world().round().0;
    assert_eq!(dispatched(&free), [(B1, B)]);
    assert_waits(&free, A1, A, (B, B1), fit(minutes(20), 0));

    let cooldown = held(COOLDOWN);
    let mut cooling = world();
    cooling.holds.hold(
        B,
        cooldown.clone(),
        Code::relayed(COOLDOWN).expect("a registered code"),
        BTreeSet::new(),
    );
    let evaluation = cooling.round().0;
    assert_eq!(dispatched(&evaluation), [(A1, A)]);
    let b1 = decision(&evaluation, B1, B);
    assert_eq!(b1.state, SchedulingDecisionState::Deferred);
    assert_eq!(b1.reasons.last(), Some(&cooldown));
    assert_eq!(b1.next_wake_unix_ms, None);

    for spelling in [PAUSED, PLAYER_HELD] {
        let mut paused = world();
        paused.cycle_holds = BTreeMap::from([(
            B.to_owned(),
            InstanceHold {
                reason: held(spelling),
                next_wake_unix_ms: None,
            },
        )]);
        let evaluation = paused.round().0;
        assert_eq!(dispatched(&evaluation), [(A1, A)], "{spelling}");
        assert_last(
            &evaluation,
            B1,
            B,
            PolicyCode::DeviceGroupWaiting,
            &[
                ("instance_id", json!(B)),
                ("alias", json!(A)),
                (
                    "device_group_wait",
                    json!(DeviceGroupWait::Selected.as_str()),
                ),
                ("task_id", json!(A1)),
            ],
        );
    }
}

// ORD-1b test 9. B's task READ reads a reading of B that is missing, so it is no candidate and B's
// recognition package (the fact refresh) is asked for it; the refresh declares no limit and takes
// the order's default. A is in front with a1 (five minutes); the refresh waited 20 minutes. With
// the default of 30 a1 ends before its ten minutes left and goes, and the refresh waits with the
// default and its time left; with a default of 60 likewise, with 40 left. READ, given a start 40
// minutes ago, is no waiting task: it waits for the refresh (`fact_refresh_pending`, gate
// `device_group_waiting`) and carries none of the order's values.
#[test]
fn ord1b_t9_a_waiting_members_fact_refresh_takes_the_default_and_its_candidates_do_not_wait() {
    for (default_ms, left_ms) in [(None, minutes(10)), (Some(minutes(60)), minutes(40))] {
        let mut order = DeviceOrder::default()
            .front(A)
            .wait_start(FACT_REFRESH_TASK_ID, B, NOW - minutes(20))
            .wait_start(READ, B, NOW - minutes(40));
        if let Some(default_ms) = default_ms {
            order = order.default_max_queue_wait_ms(default_ms);
        }
        let mut world = World::new(
            vec![task(A1, A, 100, minutes(5)), reading_task(READ, B, 900)],
            &[A, B],
            ordered(&[A, B], order),
        );
        world.readers = vec![FactRefreshReader {
            instance_id: B.to_owned(),
            package_id: "package.reader".to_owned(),
            marked_keys: BTreeSet::from([READING_KEY.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }];
        let evaluation = world.round().0;
        assert_eq!(dispatched(&evaluation), [(A1, A)], "{default_ms:?}");
        let limit_ms = default_ms.unwrap_or(DEFAULT_MAX_QUEUE_WAIT_MS);
        assert_waits(
            &evaluation,
            FACT_REFRESH_TASK_ID,
            B,
            (A, A1),
            limit(limit_ms, left_ms),
        );
        let read = decision(&evaluation, READ, B);
        let last = &read.reasons.last().expect("a reason").outcome;
        assert!(
            last.is(PolicyCode::FactRefreshPending),
            "{:?}",
            read.reasons
        );
        assert_eq!(
            last.value("gate_code"),
            Some(&json!(PolicyCode::DeviceGroupWaiting.code().as_str()))
        );
        assert!(
            read.reasons
                .iter()
                .all(|reason| reason.outcome.value("limit_ms").is_none()),
            "{:?}",
            read.reasons
        );
    }
}

// ORD-1b test 10, amendment 1. A is in front: its chain step CLOSE (ten minutes) follows LEAD's run
// on the page A rests on, and SHORT (two minutes) ranks before CLOSE; B's OTHER has five minutes
// left. CLOSE hands over: as A's first contender it goes though it does not end in time, OTHER
// waits with its limit, and SHORT waits for the chain step. Without the handover, an owed rerun
// that promotes CLOSE puts it first in A's own order, but a promoted request goes first only when
// it fits: it is passed over with its ten minutes and OTHER's five, and SHORT goes.
#[test]
fn ord1b_t10_a_chain_successor_goes_first_whether_or_not_it_fits() {
    let world = |page_id: &str| {
        let mut world = World::new(
            vec![
                with(
                    task(LEAD, A, 200, MINUTE_MS),
                    "cooldown_ms",
                    json!(2 * HOUR_MS),
                ),
                with(
                    task(CLOSE, A, 50, minutes(10)),
                    "trigger",
                    json!({
                        "kind": "dependency_completed",
                        "task_id": LEAD,
                        "terminal_states": ["succeeded"]
                    }),
                ),
                task(SHORT, A, 100, minutes(2)),
                task(OTHER, B, 500, MINUTE_MS),
            ],
            &[A, B],
            ordered(
                &[A, B],
                DeviceOrder::default()
                    .front(A)
                    .wait_start(OTHER, B, NOW - minutes(25)),
            ),
        );
        world.snapshots = vec![lead_state()];
        world.chain = ChainHandoverInputs {
            resting_pages: BTreeMap::from([(
                A.to_owned(),
                RestingPage {
                    page_id: page_id.to_owned(),
                    observed_at_unix_ms: NOW - 180_000,
                },
            )]),
            successor_wait_codes: Vec::new(),
        };
        world
    };
    let handover = world(RESULT).round().0;
    assert_eq!(dispatched(&handover), [(CLOSE, A)]);
    assert_waits(
        &handover,
        OTHER,
        B,
        (A, CLOSE),
        limit(minutes(30), minutes(5)),
    );
    let short = decision(&handover, SHORT, A);
    assert!(
        short
            .reasons
            .last()
            .is_some_and(|reason| reason.outcome.is(PolicyCode::PolicyChainHandoverWaiting)),
        "{:?}",
        short.reasons
    );

    let mut promoted = world(HOME);
    promoted.owed = OwedReruns {
        pairs: BTreeMap::from([(
            (CLOSE.to_owned(), A.to_owned()),
            OwedRerun {
                reason: owed_reason(),
                promote: true,
            },
        )]),
    };
    let evaluation = promoted.round().0;
    assert_eq!(dispatched(&evaluation), [(SHORT, A)]);
    assert_behind(&evaluation, CLOSE, A, Some(fit(minutes(10), minutes(5))));
    assert_waits(
        &evaluation,
        OTHER,
        B,
        (A, SHORT),
        limit(minutes(30), minutes(5)),
    );
}

// ORD-1b test 11. A is in front with a1 (20 minutes); B's b1 waited 20 minutes. With the default of
// 30 it has ten left, a1 does not fit and b1 goes; when b1 declares 60 it has 40 left, a1 fits and
// goes, and b1 waits with its declared limit.
#[test]
fn ord1b_t11_a_declared_limit_replaces_the_default() {
    let world = |b1: Value| {
        World::new(
            vec![task(A1, A, 100, minutes(20)), b1],
            &[A, B],
            ordered(
                &[A, B],
                DeviceOrder::default()
                    .front(A)
                    .wait_start(B1, B, NOW - minutes(20)),
            ),
        )
    };
    let default = world(task(B1, B, 900, MINUTE_MS)).round().0;
    assert_eq!(dispatched(&default), [(B1, B)]);
    assert_waits(&default, A1, A, (B, B1), fit(minutes(20), minutes(10)));
    let declared = world(limited(task(B1, B, 900, MINUTE_MS), minutes(60)))
        .round()
        .0;
    assert_eq!(dispatched(&declared), [(A1, A)]);
    assert_waits(&declared, B1, B, (A, A1), limit(minutes(60), minutes(40)));
}

// ORD-1b, a lone instance. L is alone on its device; its tasks Z1 (20 minutes) and Z2 declare the
// smallest and the largest limit, with starts long past. With no map, with a map that names L
// alone and with a device of several beside it, each with the order inputs where there is a map,
// the evaluation is the same in every decision as the one of the catalog without the member: the
// member is read only on a device of several.
#[test]
fn ord1b_without_a_device_of_several_a_declared_limit_changes_no_decision() {
    let tasks = |declared: bool| {
        let lone = |spec: Value, limit_ms: u64| {
            if declared {
                limited(spec, limit_ms)
            } else {
                spec
            }
        };
        vec![
            task(A1, A, 900, minutes(20)),
            task(B1, B, 500, MINUTE_MS),
            lone(task(Z1, L, 300, minutes(20)), MINUTE_MS),
            lone(task(Z2, L, 200, MINUTE_MS), DAY_MS),
        ]
    };
    let order = || {
        DeviceOrder::default()
            .front(L)
            .wait_start(Z1, L, NOW - minutes(40))
            .wait_start(Z2, L, NOW - minutes(40))
    };
    let alone = DeviceGroups::new(BTreeMap::from([(L.to_owned(), "device.lone".to_owned())]));
    for (case, groups) in [
        ("no map", DeviceGroups::default()),
        ("a map naming it alone", alone.with_order(order())),
        (
            "a device of several beside it",
            ordered(
                &[A, B],
                order().front(A).wait_start(B1, B, NOW - minutes(40)),
            ),
        ),
    ] {
        let declared = World::new(tasks(true), &[A, B, L], groups.clone())
            .round()
            .0;
        let undeclared = World::new(tasks(false), &[A, B, L], groups).round().0;
        assert_eq!(declared.decisions, undeclared.decisions, "{case}");
    }
}

// ---- The ORD-1b fixture

const A: &str = "neutral.a";
const B: &str = "neutral.b";
const C: &str = "neutral.c";
/// An instance alone on its own device.
const L: &str = "neutral.lone";
/// The device key the Runtime gives A, B and C (the text of the smallest member id).
const DEVICE: &str = "device.key";
const MINUTE_MS: u64 = 60_000;
const A1: &str = "task.a1";
const A2: &str = "task.a2";
const A3: &str = "task.a3";
const B1: &str = "task.b1";
const C1: &str = "task.c1";
const C2: &str = "task.c2";
const Z1: &str = "task.z1";
const Z2: &str = "task.z2";
/// Test 10: a chain on A, a task ranked before its step, and a task of B.
const LEAD: &str = "task.lead";
const CLOSE: &str = "task.close";
const SHORT: &str = "task.short";
const OTHER: &str = "task.other";
/// CLOSE's entry page, and a page outside its entry page set.
const RESULT: &str = "neutral/result";
const HOME: &str = "neutral/home";
/// Test 9: a task of B whose trigger reads a reading B's recognition package writes.
const READ: &str = "task.read";
const READING_KEY: &str = "resource.neutral";
/// The admission's deferral the fixture's eligibility callback gives a task it defers.
const NOT_LOADED: &str = "pack_registry_not_loaded";
/// The Runtime's reasons for a held member: a switch cooldown, a pause, a person's hold.
const COOLDOWN: &str = "dispatch_instance_cooldown";
const PAUSED: &str = "dispatch_paused_instance";
const PLAYER_HELD: &str = "dispatch_player_held";
/// The Runtime's reason for an owed rerun.
const OWED: &str = "policy_rerun_owed";

const fn minutes(count: u64) -> u64 {
    count * MINUTE_MS
}

/// One round's inputs beside its catalog, at NOW.
struct World {
    catalog: CompiledCatalog,
    instances: Vec<&'static str>,
    snapshots: Vec<TaskRuntimeSnapshot>,
    groups: DeviceGroups,
    /// The tasks whose admission the eligibility callback defers.
    deferred: Vec<&'static str>,
    holds: InstanceHolds,
    /// The members held as the cycle starts.
    cycle_holds: BTreeMap<String, InstanceHold>,
    owed: OwedReruns,
    chain: ChainHandoverInputs,
    readers: Vec<FactRefreshReader>,
}

impl World {
    /// The V2 catalog of `tasks` on `instances`, in that order, with `groups`.
    fn new(tasks: Vec<Value>, instances: &[&'static str], groups: DeviceGroups) -> Self {
        Self {
            catalog: compile_catalog(&sources(SCHEDULING_SCHEMA_VERSION_V2, tasks)).unwrap_or_else(
                |failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()),
            ),
            instances: instances.to_vec(),
            snapshots: Vec::new(),
            groups,
            deferred: Vec::new(),
            holds: InstanceHolds::default(),
            cycle_holds: BTreeMap::new(),
            owed: OwedReruns::default(),
            chain: ChainHandoverInputs::default(),
            readers: Vec::new(),
        }
    }

    /// The round, and the tasks the admission was asked about, in order.
    fn round(&self) -> (PolicyEvaluation, Vec<String>) {
        let facts = EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-ord1".to_owned(),
            facts: Vec::new(),
            outcomes: Vec::new(),
            tasks: self.snapshots.clone(),
            instances: self
                .instances
                .iter()
                .map(|alias| instance(alias, &[]))
                .collect(),
            priority_offsets: Vec::new(),
        };
        let resources = EvaluationResources {
            pools: Vec::new(),
            hosts: facts
                .instances
                .iter()
                .map(|instance| HostResourceSnapshot {
                    host_id: instance.host_id.clone(),
                    cpu_available_milli: 1_000,
                    gpu_available_milli: 1_000,
                    io_available_milli: 1_000,
                    host_responsiveness_basis_points: 10_000,
                    third_party_pressure_basis_points: 0,
                    heavy_dispatch_limit: 1,
                    active_heavy_dispatches: 0,
                    heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
                })
                .collect(),
        };
        let refresh = FactRefreshInputs {
            settings: FactRefreshSettings {
                enabled: true,
                min_interval_ms: 300_000,
                daily_limit: 96,
                failure_backoff_ms: 600_000,
                max_consecutive_failures: 3,
                expected_duration_ms: FACT_REFRESH_EXPECTED_DURATION_MS,
            },
            readers: self.readers.clone(),
            instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
            instance_holds: self.cycle_holds.clone(),
            idle_closed: BTreeMap::new(),
        };
        let mut asked = Vec::new();
        let evaluation = evaluate_with_device_groups::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: NOW,
                monotonic_ms: NOW,
            },
            11,
            &BTreeSet::new(),
            &refresh,
            &RemedyPlan::default(),
            &self.chain,
            &RunSlots::default(),
            &self.owed,
            &self.holds,
            &NoProgressGuards::default(),
            &mut CappedMemory::default(),
            &InstancePoolLines::default(),
            &LeftOutFacts::default(),
            &self.groups,
            |task, _, page_id| Ok(task.id == CLOSE && page_id == RESULT),
            |intent: &DispatchIntent| {
                asked.push(intent.task_id.clone());
                Ok(
                    if self.deferred.iter().any(|task| *task == intent.task_id) {
                        CandidateEligibility::Deferred {
                            reason: not_loaded(),
                            next_wake_unix_ms: None,
                        }
                    } else {
                        CandidateEligibility::Eligible
                    },
                )
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"));
        (evaluation, asked)
    }
}

/// `members` on one device, without the order inputs (MAPP-1).
fn device(members: &[&str]) -> DeviceGroups {
    DeviceGroups::new(
        members
            .iter()
            .map(|member| ((*member).to_owned(), DEVICE.to_owned()))
            .collect(),
    )
}

/// `members` on one device, with the order inputs `order`.
fn ordered(members: &[&str], order: DeviceOrder) -> DeviceGroups {
    device(members).with_order(order)
}

/// [`task_body`] of `instance_id` with its priority and duration, without the run-count bounds a
/// V2 task need not write.
fn task(task_id: &str, instance_id: &str, priority: i16, expected_ms: u64) -> Value {
    let mut task = task_body(task_id, instance_id);
    task["priority"] = json!(priority);
    task["expected_duration_ms"] = json!(expected_ms);
    task.as_object_mut()
        .expect("a task object")
        .shift_remove("loop_budget");
    task
}

/// `task` declaring the longest queue wait `limit_ms`.
fn limited(task: Value, limit_ms: u64) -> Value {
    with(task, MEMBER, json!(limit_ms))
}

/// A one-minute task of `instance_id` whose trigger reads [`READING_KEY`] of its instance, which no
/// fact holds.
fn reading_task(task_id: &str, instance_id: &str, priority: i16) -> Value {
    with(
        task(task_id, instance_id, priority, MINUTE_MS),
        "trigger",
        json!({
            "kind": "fact",
            "scope": {"kind": "instance", "instance_id": instance_id},
            "fact_key": READING_KEY,
            "comparison": "less_than",
            "value": {"type": "integer", "value": 20},
            "max_age_ms": 900000
        }),
    )
}

/// The latest run of `task_id` on `alias`, two hours before NOW: it served an earlier occurrence
/// of the hourly clock and its cooldown is over.
fn earlier_run(task_id: &str, alias: &str) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: alias.to_owned(),
        last_dispatched_unix_ms: Some(NOW - 2 * HOUR_MS),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

/// LEAD's latest run on A: admitted ten minutes before the round, succeeded two minutes before it.
fn lead_state() -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: LEAD.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(NOW - 600_000),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: Some(CompletedActivityWindow {
            window_id: "window.fixture".to_owned(),
            completed_at_unix_ms: NOW - 120_000,
        }),
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

/// The admission's deferral of a task the fixture defers.
fn not_loaded() -> DecisionReason {
    DecisionReason {
        outcome: Outcome::relayed(NOT_LOADED),
        detail: format!("fixture admission: {NOT_LOADED}"),
    }
}

/// The Runtime's reason `spelling` for B.
fn held(spelling: &'static str) -> DecisionReason {
    DecisionReason {
        outcome: Outcome::relayed(spelling).instance_id(B),
        detail: format!("fixture hold: {spelling}"),
    }
}

/// The Runtime's reason for CLOSE's owed rerun on A.
fn owed_reason() -> DecisionReason {
    DecisionReason {
        outcome: Outcome::new(Code::relayed(OWED).expect("a registered code"))
            .task_id(CLOSE)
            .instance_id(A)
            .count(1_u32),
        detail: "a rerun is owed".to_owned(),
    }
}

fn decision<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
    instance_id: &str,
) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(instance_id)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {instance_id}: {evaluation:#?}"))
}

/// The (task, instance) of every dispatch, in instance order.
fn dispatched(evaluation: &PolicyEvaluation) -> Vec<(&str, &str)> {
    let mut dispatched = evaluation
        .dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.as_str(), intent.instance_id.as_str()))
        .collect::<Vec<_>>();
    dispatched.sort_by(|left, right| left.1.cmp(right.1));
    dispatched
}

/// `limit_ms` and `remaining_ms`: a waiting task's limit and its time left.
fn limit(limit_ms: u64, remaining_ms: u64) -> [(&'static str, u64); 2] {
    [("limit_ms", limit_ms), ("remaining_ms", remaining_ms)]
}

/// `required_ms` and `remaining_ms`: a front task's duration and the binding waiter's time left.
fn fit(required_ms: u64, remaining_ms: u64) -> [(&'static str, u64); 2] {
    [("required_ms", required_ms), ("remaining_ms", remaining_ms)]
}

/// `task_id` on `instance_id` waits for `winner` (its member and task) with `device_group_waiting`
/// `selected` and exactly `extra` beside the four values of MAPP-1.
fn assert_waits(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    instance_id: &str,
    winner: (&str, &str),
    extra: [(&str, u64); 2],
) {
    let mut values = vec![
        ("instance_id", json!(instance_id)),
        ("alias", json!(winner.0)),
        (
            "device_group_wait",
            json!(DeviceGroupWait::Selected.as_str()),
        ),
        ("task_id", json!(winner.1)),
    ];
    values.extend(extra.iter().map(|(key, value)| (*key, json!(value))));
    assert_last(
        evaluation,
        task_id,
        instance_id,
        PolicyCode::DeviceGroupWaiting,
        &values,
    );
}

/// `task_id` on `instance_id` waits behind its own instance's winner with
/// `instance_already_selected`, with exactly `fit` as its values (none without).
fn assert_behind(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    instance_id: &str,
    fit: Option<[(&str, u64); 2]>,
) {
    let values = fit
        .iter()
        .flatten()
        .map(|(key, value)| (*key, json!(value)))
        .collect::<Vec<_>>();
    assert_last(
        evaluation,
        task_id,
        instance_id,
        PolicyCode::InstanceAlreadySelected,
        &values,
    );
}

/// The last reason of `task_id` on `instance_id` is `code` with exactly `values`.
fn assert_last(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    instance_id: &str,
    code: PolicyCode,
    values: &[(&str, Value)],
) {
    let decision = decision(evaluation, task_id, instance_id);
    let last = &decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason of {task_id} on {instance_id}: {decision:?}"))
        .outcome;
    assert!(
        last.is(code),
        "{task_id} on {instance_id}: {:?}",
        decision.reasons
    );
    let expected = values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<Map<String, Value>>();
    assert_eq!(last.values(), &expected, "{task_id} on {instance_id}");
}

// ---- The ORD-1a fixture

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
}

/// One hourly task of `instance_id`, priority 100, a minute long, written with every run-count
/// bound so that it compiles as V1 too; its feedback stop stays false.
fn task_body(task_id: &str, instance_id: &str) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "instance", "instance_id": instance_id},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": HOUR_MS,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": task_id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// One activity profile of the game, open all day on every weekday.
fn profile() -> Value {
    json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    })
}

/// The four documents of a catalog of `version` with `tasks`, no pool and no event.
fn sources(version: &str, tasks: Vec<Value>) -> CatalogSources {
    let header = json!({
        "catalog_id": "fixture.ord1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-ord1"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({"schema_version": version, "catalog": header});
        document[member] = entries;
        document
    };
    CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile()]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    }
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-ord1/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed).
fn example(version: &str) -> CatalogSources {
    let parse = |bytes: &[u8]| -> Value { serde_json::from_slice(bytes).expect("example JSON") };
    let mut documents = [
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/pools.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/activity.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
        )),
    ];
    for declared in &mut documents {
        declared["schema_version"] = json!(version);
        if version == SCHEDULING_SCHEMA_VERSION_V2
            && let Some(events) = declared.get_mut("events").and_then(Value::as_array_mut)
        {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
    }
    let [tasks, pools, activity, timeline] = documents;
    CatalogSources {
        tasks: source("example-tasks", &tasks),
        pools: source("example-pools", &pools),
        activity: source("example-activity", &activity),
        timeline: source("example-timeline", &timeline),
        selection: None,
    }
}

/// A `schedule.json` rule running the package of its own id: [`task_body`] without scope,
/// entrypoint, procedure and instance overrides, with `package_id`.
fn schedule_rule(id: &str) -> Value {
    let mut rule = task_body(id, "instance-a");
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    rule
}

/// `schedule.json` with these rules, naming `requires`.
fn schedule(rules: Vec<Value>, requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": [],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule; with
/// `subsystems`, a Runtime manifest checks `requires`.
fn compose(
    schedule: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .collect::<BTreeSet<_>>();
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packages
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>()
    });
    let bundle = serde_json::to_vec(&bundle).expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let maintenance = serde_json::from_slice::<BundleIndex>(&bundle)
        .expect("a bundle index")
        .maintenance()
        .to_vec();
    let schedule = serde_json::to_vec(schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &maintenance,
        },
    )]);
    let instances = BTreeMap::from([(
        "instance-a".to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems,
        catalog_version: 1,
    })
}
