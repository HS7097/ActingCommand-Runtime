// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #372 K3 (model-372-skip v3 §4.5, §9 K3): the `clock_slots` section of
//! `actingd suspended`. The records come from K2's own recording, driven through the policy
//! cycle by the scenarios of `gate_372k2`, or, for what no cycle writes, are appended as
//! planning signals. The report is read from the closed state root as `actingd suspended`
//! reads it, and every expected value comes from the frozen model: the entry members, the
//! fate rules of §3.2 and §3.3, the 48 h window, the cap of 200, the unparsed entry.

use super::gate_372k2::{SLOT_TASK, Slots, at, slot_sources, slot_task};
use super::*;
use crate::codes::HostCode;
use crate::policy_host::clock_slot::{SlotRecord, SlotValues, clock_slot_signal, signal_id};
use crate::suspension_report::{SlotDispatch, SlotSignal, clock_slot_entry};
use actingcommand_contract::PolicyBudgetDimension;
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};

/// `actingd suspended` on a closed state root, with the catalog the scenario activated.
fn report(root: &TempDir, sources: &CatalogSources) -> Value {
    let host = config(root);
    crate::suspension_report(&SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &host,
        catalog: sources,
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"))
}

/// Every event of the closed ledger.
fn ledger_events(root: &TempDir) -> Vec<PersistedEvent> {
    GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root.path())
            .sqlite_material_not_read(),
        |_| None,
    )
    .expect("read the closed ledger")
    .query(&EventQuery::default())
}

/// The ledger sequence of the one record `signal_id`.
fn record_sequence(events: &[PersistedEvent], signal_id: &str) -> u64 {
    let found = events
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
                    if payload.signal_id() == signal_id
            )
        })
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "one record {signal_id}: {found:?}");
    found[0]
}

/// `next_dispatch` naming the run admitted at `unix_ms`: its admission's sequence and time.
fn admitted_at(events: &[PersistedEvent], unix_ms: u64) -> Value {
    let found = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::PolicyDispatchAdmitted
                && event.timestamp_unix_ms() == unix_ms
        })
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "one admission at {unix_ms}: {found:?}");
    json!({"sequence": found[0], "admitted_at_unix_ms": unix_ms})
}

/// A refused record settled `next_window` by the next morning's run, and two owed records the
/// runs that selected them served: K2's overnight activation (`gate_372k2` test 1),
/// `daily [07:00, 19:00]` with a limit of 2, run on to the next morning.
#[test]
fn clock_slots_settle_a_refused_slot_next_window_and_owed_runs_served() {
    let sources = slot_sources(1, &[slot_task(&[420, 1140], 2)], None);
    let mut slots = Slots::start(&sources, at(0, 23, 40));
    slots.evaluate(at(0, 23, 40));
    slots.dispatch(at(1, 6, 9), SLOT_TASK);
    slots.dispatch(at(1, 7, 0), SLOT_TASK);
    slots.evaluate(at(1, 19, 0));
    slots.dispatch(at(2, 6, 5), SLOT_TASK);
    let root = slots.close_keeping_root();
    let events = ledger_events(&root);
    let report = report(&root, &sources);

    let first_run = signal_id(
        SlotRecord::Owed,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        at(0, 19, 0),
    );
    let refused = signal_id(
        SlotRecord::Refused,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        at(1, 19, 0),
    );
    let next_morning = signal_id(
        SlotRecord::Owed,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        at(1, 19, 0),
    );
    assert_eq!(
        report["clock_slots"],
        json!([
            {
                "code": "clock_slot_owed",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "occurrence_unix_ms": at(1, 19, 0),
                "window_start_unix_ms": at(2, 6, 0),
                "next_occurrence_unix_ms": at(2, 7, 0),
                "slot_fate": "served",
                "slot_fate_settled": true,
                "recorded_sequence": record_sequence(&events, &next_morning),
                "recorded_at_unix_ms": at(2, 6, 5),
                "next_dispatch": admitted_at(&events, at(2, 6, 5)),
            },
            {
                "code": "clock_slot_refused",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "gate_code": "policy_budget_exhausted",
                "budget_dimension": "task_daily",
                "occurrence_unix_ms": at(1, 19, 0),
                "window_end_unix_ms": at(1, 23, 30),
                "next_eligible_unix_ms": at(2, 6, 0),
                "next_occurrence_unix_ms": at(2, 7, 0),
                "slot_fate": "next_window",
                "slot_fate_settled": true,
                "recorded_sequence": record_sequence(&events, &refused),
                "recorded_at_unix_ms": at(1, 19, 0),
                "next_dispatch": admitted_at(&events, at(2, 6, 5)),
            },
            {
                "code": "clock_slot_owed",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "occurrence_unix_ms": at(0, 19, 0),
                "window_start_unix_ms": at(1, 6, 0),
                "next_occurrence_unix_ms": at(1, 7, 0),
                "first_run": true,
                "slot_fate": "served",
                "slot_fate_settled": true,
                "recorded_sequence": record_sequence(&events, &first_run),
                "recorded_at_unix_ms": at(1, 6, 9),
                "next_dispatch": admitted_at(&events, at(1, 6, 9)),
            },
        ]),
        "{report:#}"
    );
    assert!(report.get("clock_slots_total").is_none(), "{report:#}");
}

/// K2's pause case (`gate_372k2` test 3): an instance pause at the 10:00 slot of
/// `daily [10:00]`; with `settle`, the resume and the 10:30 run after it.
fn paused_slot(settle: bool) -> (Value, Vec<PersistedEvent>) {
    let sources = slot_sources(1, &[slot_task(&[600], 1)], None);
    let mut slots = Slots::start(&sources, at(0, 9, 0));
    slots.pause();
    slots.evaluate(at(0, 10, 0));
    if settle {
        slots.resume();
        slots.dispatch(at(0, 10, 30), SLOT_TASK);
    }
    let root = slots.close_keeping_root();
    (report(&root, &sources), ledger_events(&root))
}

/// A deferred record is `pending` while the pause holds and before its next occurrence, and
/// the run after the resume settles it `same_window`.
#[test]
fn clock_slots_show_a_deferred_slot_pending_then_settled_same_window() {
    let paused = signal_id(
        SlotRecord::Deferred,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        at(0, 10, 0),
    );
    let deferred = |events: &[PersistedEvent]| {
        json!({
            "code": "clock_slot_deferred",
            "task_id": SLOT_TASK,
            "instance_id": POLICY_INSTANCE_ALIAS,
            "gate_code": "dispatch_paused_instance",
            "occurrence_unix_ms": at(0, 10, 0),
            "window_end_unix_ms": at(0, 23, 30),
            "next_occurrence_unix_ms": at(1, 10, 0),
            "recorded_sequence": record_sequence(events, &paused),
            "recorded_at_unix_ms": at(0, 10, 0),
        })
    };

    let (report, events) = paused_slot(false);
    let mut pending = deferred(&events);
    pending["slot_fate"] = json!("pending");
    pending["slot_fate_settled"] = json!(false);
    assert_eq!(report["clock_slots"], json!([pending]), "{report:#}");

    let (report, events) = paused_slot(true);
    let mut settled = deferred(&events);
    settled["slot_fate"] = json!("same_window");
    settled["slot_fate_settled"] = json!(true);
    settled["next_dispatch"] = admitted_at(&events, at(0, 10, 30));
    assert_eq!(report["clock_slots"], json!([settled]), "{report:#}");
}

/// An owed selection that was never admitted is `absorbed` by the run that later served the
/// next occurrence; the first run before it was served by its own admission.
#[test]
fn clock_slots_settle_an_owed_selection_never_admitted_as_absorbed() {
    let sources = slot_sources(1, &[slot_task(&[390], 1)], None);
    let mut slots = Slots::start(&sources, at(1, 6, 0));
    slots.dispatch(at(1, 6, 7), SLOT_TASK);
    let cycle = slots.evaluate(at(2, 6, 7));
    assert!(
        cycle
            .pending_dispatch_intents
            .iter()
            .any(|intent| intent.task_id == SLOT_TASK),
        "the pair is selected at 06:07"
    );
    // That selection is not admitted; the run after the 06:30 slot is.
    slots.dispatch(at(2, 6, 31), SLOT_TASK);
    let root = slots.close_keeping_root();
    let events = ledger_events(&root);
    let report = report(&root, &sources);

    let first_run = signal_id(
        SlotRecord::Owed,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        at(0, 6, 30),
    );
    let never_admitted = signal_id(
        SlotRecord::Owed,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        at(1, 6, 30),
    );
    assert_eq!(
        report["clock_slots"],
        json!([
            {
                "code": "clock_slot_owed",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "occurrence_unix_ms": at(1, 6, 30),
                "window_start_unix_ms": at(2, 6, 0),
                "next_occurrence_unix_ms": at(2, 6, 30),
                "slot_fate": "absorbed",
                "slot_fate_settled": true,
                "recorded_sequence": record_sequence(&events, &never_admitted),
                "recorded_at_unix_ms": at(2, 6, 7),
                "next_dispatch": admitted_at(&events, at(2, 6, 31)),
            },
            {
                "code": "clock_slot_owed",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "occurrence_unix_ms": at(0, 6, 30),
                "window_start_unix_ms": at(1, 6, 0),
                "next_occurrence_unix_ms": at(1, 6, 30),
                "first_run": true,
                "slot_fate": "served",
                "slot_fate_settled": true,
                "recorded_sequence": record_sequence(&events, &first_run),
                "recorded_at_unix_ms": at(1, 6, 7),
                "next_dispatch": admitted_at(&events, at(1, 6, 7)),
            },
        ]),
        "{report:#}"
    );
}

/// Other planning signals are not clock slots: a ledger whose only planning signal is a
/// detection reservation gives an empty list.
#[test]
fn clock_slots_are_empty_on_a_ledger_with_only_detection_signals() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("detection runtime host");
    let sources = detection_policy_sources(1);
    host.activate_policy_catalog(&sources)
        .expect("activate detection catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &detection_policy_facts(false, "snapshot:clock-slot-report-detection"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 2_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 2_000,
            },
            2,
            PolicyTrigger::FactsChanged,
        )
        .expect("reserve detection quota");
    assert_eq!(
        cycle.detection_planning_signals.len(),
        1,
        "one detection reservation: {:?}",
        cycle.detection_planning_signals
    );
    assert!(
        cycle.clock_slot_signals.is_empty(),
        "no clock slot: {:?}",
        cycle.clock_slot_signals
    );
    host.close().expect("close detection runtime");

    let report = report(&root, &sources);
    assert_eq!(report["clock_slots"], json!([]), "{report:#}");
    assert!(report.get("clock_slots_total").is_none(), "{report:#}");
}

/// `actingd suspended` on a root where only `signals` were appended, at 12:00 on day 1.
fn appended(signals: Vec<PolicyPlanningSignalEventData>) -> Value {
    let sources = slot_sources(1, &[slot_task(&[600], 1)], None);
    let slots = Slots::start(&sources, at(1, 12, 0));
    for signal in signals {
        slots
            .host
            .record_policy_planning_signal(signal)
            .expect("append the planning signal");
    }
    report(&slots.close_keeping_root(), &sources)
}

/// A clock-slot record whose outcome does not read back, here one whose code is not a
/// clock-slot code (Workflow #378 A2-P: the record carries an outcome, not a token), is an
/// unparsed entry naming that code and adds no report warning.
#[test]
fn a_clock_slot_token_that_does_not_read_back_is_unparsed_without_a_warning() {
    let mut signal = clock_slot_signal(
        SlotRecord::Refused,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        &SlotValues {
            gate_code: Some(HostCode::PolicyBudgetExhausted.code()),
            budget_dimension: Some(PolicyBudgetDimension::TaskDaily),
            occurrence_unix_ms: at(1, 10, 0),
            window_end_unix_ms: Some(at(1, 23, 30)),
            next_eligible_unix_ms: Some(at(2, 6, 0)),
            next_occurrence_unix_ms: Some(at(2, 10, 0)),
            ..SlotValues::default()
        },
        at(1, 12, 0),
    );
    signal.outcome = Outcome::new(HostCode::MaintenanceRecheckSuggested);
    let id = signal.signal_id.clone();

    let without = appended(Vec::new());
    let report = appended(vec![signal]);
    assert_eq!(
        report["clock_slots"],
        json!([{
            "parse_state": "unparsed",
            "signal_id": id,
            "code": "maintenance_recheck_suggested"
        }]),
        "{report:#}"
    );
    assert_eq!(report["warnings"], without["warnings"], "{report:#}");
}

/// More than 200 records in the window: the 200 newest, newest first, and their number.
#[test]
fn clock_slots_keep_the_200_newest_and_count_them_all() {
    let occurrences = (0..201)
        .map(|index| at(0, 0, 0) + index * 60_000)
        .collect::<Vec<_>>();
    let signals = occurrences
        .iter()
        .map(|occurrence| {
            clock_slot_signal(
                SlotRecord::Refused,
                SLOT_TASK,
                POLICY_INSTANCE_ALIAS,
                &SlotValues {
                    gate_code: Some(HostCode::PolicyBudgetExhausted.code()),
                    budget_dimension: Some(PolicyBudgetDimension::TaskDaily),
                    occurrence_unix_ms: *occurrence,
                    ..SlotValues::default()
                },
                at(1, 12, 0),
            )
        })
        .collect();
    let report = appended(signals);
    let shown = report["clock_slots"]
        .as_array()
        .unwrap_or_else(|| panic!("clock_slots is a list: {report:#}"))
        .iter()
        .map(|entry| entry["occurrence_unix_ms"].as_u64())
        .collect::<Vec<_>>();
    let newest = occurrences
        .iter()
        .rev()
        .take(200)
        .map(|occurrence| Some(*occurrence))
        .collect::<Vec<_>>();
    assert_eq!(shown, newest);
    assert_eq!(report["clock_slots_total"], json!(201));
}

/// The golden JSON of one entry of each code, from K2's own outcome: a refused record settled
/// by the next morning's run, a deferred record not yet settled, an owed first run served.
#[test]
fn clock_slot_entries_of_each_code_match_their_golden_json() {
    let entry = |kind, values: SlotValues, recorded: (u64, u64), dispatch, reference| {
        let signal = clock_slot_signal(kind, SLOT_TASK, POLICY_INSTANCE_ALIAS, &values, recorded.1);
        serde_json::to_string(&clock_slot_entry(
            &SlotSignal {
                sequence: recorded.0,
                recorded_at_unix_ms: recorded.1,
                signal_id: &signal.signal_id,
                task_id: signal.task_id.as_deref(),
                instance_id: &signal.instance_id,
                outcome: &signal.outcome,
            },
            dispatch,
            reference,
        ))
        .expect("encode the entry")
    };

    let refused = SlotValues {
        gate_code: Some(HostCode::PolicyBudgetExhausted.code()),
        budget_dimension: Some(PolicyBudgetDimension::TaskDaily),
        occurrence_unix_ms: at(1, 19, 0),
        window_end_unix_ms: Some(at(1, 23, 30)),
        next_eligible_unix_ms: Some(at(2, 6, 0)),
        next_occurrence_unix_ms: Some(at(2, 7, 0)),
        ..SlotValues::default()
    };
    assert_eq!(
        entry(
            SlotRecord::Refused,
            refused,
            (120, at(1, 19, 0)),
            Some(SlotDispatch {
                sequence: 130,
                admitted_at_unix_ms: at(2, 6, 5),
            }),
            at(2, 6, 5),
        ),
        r#"{"code":"clock_slot_refused","task_id":"fixture.slot","instance_id":"fixture-instance-a","gate_code":"policy_budget_exhausted","budget_dimension":"task_daily","occurrence_unix_ms":1700161200000,"window_end_unix_ms":1700177400000,"next_eligible_unix_ms":1700200800000,"next_occurrence_unix_ms":1700204400000,"slot_fate":"next_window","slot_fate_settled":true,"recorded_sequence":120,"recorded_at_unix_ms":1700161200000,"next_dispatch":{"sequence":130,"admitted_at_unix_ms":1700201100000}}"#
    );

    let deferred = SlotValues {
        gate_code: Some(HostCode::DispatchPausedInstance.code()),
        occurrence_unix_ms: at(0, 10, 0),
        window_end_unix_ms: Some(at(0, 23, 30)),
        next_occurrence_unix_ms: Some(at(1, 10, 0)),
        ..SlotValues::default()
    };
    assert_eq!(
        entry(
            SlotRecord::Deferred,
            deferred,
            (40, at(0, 10, 0)),
            None,
            at(0, 10, 0),
        ),
        r#"{"code":"clock_slot_deferred","task_id":"fixture.slot","instance_id":"fixture-instance-a","gate_code":"dispatch_paused_instance","occurrence_unix_ms":1700042400000,"window_end_unix_ms":1700091000000,"next_occurrence_unix_ms":1700128800000,"slot_fate":"pending","slot_fate_settled":false,"recorded_sequence":40,"recorded_at_unix_ms":1700042400000}"#
    );

    let owed = SlotValues {
        occurrence_unix_ms: at(0, 19, 0),
        window_start_unix_ms: Some(at(1, 6, 0)),
        next_occurrence_unix_ms: Some(at(1, 7, 0)),
        first_run: true,
        ..SlotValues::default()
    };
    assert_eq!(
        entry(
            SlotRecord::Owed,
            owed,
            (60, at(1, 6, 9)),
            Some(SlotDispatch {
                sequence: 65,
                admitted_at_unix_ms: at(1, 6, 9),
            }),
            at(1, 6, 9),
        ),
        r#"{"code":"clock_slot_owed","task_id":"fixture.slot","instance_id":"fixture-instance-a","occurrence_unix_ms":1700074800000,"window_start_unix_ms":1700114400000,"next_occurrence_unix_ms":1700118000000,"first_run":true,"slot_fate":"served","slot_fate_settled":true,"recorded_sequence":60,"recorded_at_unix_ms":1700114940000,"next_dispatch":{"sequence":65,"admitted_at_unix_ms":1700114940000}}"#
    );
}
