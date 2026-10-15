// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #372 K4 (K372 addendum of 2026-10-15, K4.5 rows K4-T1 to K4-T7; with #361 R7): an
// unserved clock occurrence lost to its `missed` grace is recorded once as `clock_slot_expired`
// (info), one absorbed by a later occurrence with no other record as `clock_slot_superseded`
// (warning); only for a pair with a dispatch history that would have come due. The scenarios
// run generic tasks on the fixture instance of `gate_372k2` (activity window 06:00-23:30 UTC,
// UTC clocks, a manual Runtime clock); the records are read back from the ledger by their
// `signal_id` prefix. Every expected value comes from the frozen model row.

use super::clock_slot_report::{ledger_events, record_sequence, report};
use super::gate_372k2::{
    SLOT_TASK, SlotRow, SlotTask, Slots, at, slot_rows, slot_sources, slot_task,
};
use super::*;
use crate::codes::HostCode;
use crate::policy_host::clock_slot::{SlotRecord, SlotValues, clock_slot_signal, signal_id};
use actingcommand_contract::PolicyPlanningSignalPayload;
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::{Value, json};

// The grace of every `missed` rule of these scenarios: two hours.
const GRACE_MS: u64 = 7_200_000;
const HOUR_MS: u64 = 3_600_000;

// A UTC `daily` clock at `minutes`, with `missed: skip_after` of `grace_ms` when given.
fn daily_clock(minutes: &[u16], grace_ms: Option<u64>) -> Value {
    let mut clock = json!({
        "kind": "clock",
        "schedule": {
            "kind": "daily",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "minutes_of_day": minutes
        }
    });
    if let Some(grace_ms) = grace_ms {
        clock["missed"] = json!({"kind": "skip_after", "grace_ms": grace_ms});
    }
    clock
}

// The slot task with a `daily` clock at `minutes` that skips an occurrence after GRACE_MS.
fn missed_task(minutes: &[u16], daily_limit: u32) -> SlotTask {
    SlotTask {
        id: SLOT_TASK,
        priority: 100,
        trigger: daily_clock(minutes, Some(GRACE_MS)),
        daily_limit,
    }
}

// `sources` declared as `actingcommand.scheduling.v2`, the version `missed` needs; each
// timeline event gets its explicit unbounded validity.
fn v2(mut sources: CatalogSources) -> CatalogSources {
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value = serde_json::from_slice(&source.bytes).expect("fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("V2 fixture bytes");
    }
    sources
}

// The (kind, occurrence, next occurrence) of each record, in ledger order.
fn lapses(rows: &[SlotRow]) -> Vec<(&str, Option<u64>, Option<u64>)> {
    rows.iter()
        .map(|row| {
            (
                row.kind.as_str(),
                row.time("occurrence_unix_ms"),
                row.time("next_occurrence_unix_ms"),
            )
        })
        .collect()
}

// Evaluates at `unix_ms`, admits the dispatch the cycle selected for `task_id` and records it
// failed, as a failed run of the task does. The catalog is already approved by an earlier
// dispatch.
fn dispatch_failed(slots: &mut Slots, unix_ms: u64, task_id: &str) {
    let cycle = slots.evaluate(unix_ms);
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| panic!("{task_id} is selected at {unix_ms}: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("dispatch reason chain")
        .clone();
    let PolicyDispatchAdmission::Granted { context } = slots
        .host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&slots.host, &intent))
        .unwrap_or_else(|error| panic!("admit {task_id} at {unix_ms}: {error}"))
    else {
        panic!("{task_id} is granted at {unix_ms}");
    };
    slots
        .host
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            &PolicyExecutionInput::Failed {
                outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                failure_identity: None,
                class: PolicyFailureClass::Recoverable,
            },
        )
        .expect("record the failed run");
    slots
        .host
        .release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
}

// Whether the cycle selected `task_id`.
fn selects(cycle: &PolicyCycle, task_id: &str) -> bool {
    cycle
        .pending_dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id)
}

// The outcome of the one planning signal `signal_id` in the host's ledger.
fn recorded_outcome(host: &RuntimeHost, signal_id: &str) -> Outcome {
    let outcomes = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        })
        .expect("planning signals")
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
                if payload.signal_id() == signal_id =>
            {
                Some(payload.outcome().clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(outcomes.len(), 1, "one record {signal_id}");
    outcomes[0].clone()
}

// K4-T1, expired after waiting in the round: `daily [06:30]` with `missed {7 200 000}`,
// dispatched the day before. A client claim keeps the instance from 06:20 until 08:40, as
// another selected task's run would. The first cycle at or after 08:30 writes one
// `clock_slot_expired` {O 06:30, grace_ms 7 200 000, next tomorrow 06:30}; later cycles and a
// host restart add nothing, and the pair is not dispatched that day.
#[test]
fn k4_t1_an_occurrence_kept_waiting_past_its_grace_is_recorded_expired_once() {
    let sources = v2(slot_sources(1, &[missed_task(&[390], 1)], None));
    let mut slots = Slots::start(&sources, at(0, 6, 0));
    slots.dispatch(at(0, 6, 30), SLOT_TASK);
    slots.new_rows();

    slots.evaluate(at(1, 6, 20));
    let holder = slots.hold();
    for unix_ms in [at(1, 6, 30), at(1, 7, 30), at(1, 8, 29)] {
        slots.evaluate(unix_ms);
    }
    assert!(
        slots.new_rows().is_empty(),
        "nothing while the occurrence waits inside its grace and its window"
    );

    slots.evaluate(at(1, 8, 30));
    let rows = slots.new_rows();
    assert_eq!(rows.len(), 1, "one expired record at 08:30: {rows:#?}");
    let expired = &rows[0];
    assert_eq!(
        (expired.kind.as_str(), expired.code.as_str()),
        ("expired", "clock_slot_expired")
    );
    assert_eq!(expired.task_id, SLOT_TASK);
    assert_eq!(expired.observed_at_unix_ms, at(1, 8, 30));
    assert_eq!(expired.time("occurrence_unix_ms"), Some(at(1, 6, 30)));
    assert_eq!(expired.time("grace_ms"), Some(GRACE_MS));
    assert_eq!(expired.time("next_occurrence_unix_ms"), Some(at(2, 6, 30)));

    slots.evaluate(at(1, 8, 35));
    Slots::release(holder);
    for unix_ms in [at(1, 8, 41), at(1, 12, 0), at(1, 23, 0)] {
        let cycle = slots.evaluate(unix_ms);
        assert!(
            !selects(&cycle, SLOT_TASK),
            "the expired occurrence is not dispatched at {unix_ms}"
        );
    }
    let mut slots = slots.restart(at(1, 23, 5));
    let cycle = slots.evaluate(at(1, 23, 5));
    assert!(!selects(&cycle, SLOT_TASK));
    assert!(
        slots.new_rows().is_empty(),
        "later cycles and a restart add nothing"
    );
    slots.close();
}

// K4-T2, expired after a refusal: `daily [06:00, 06:30]` with `missed {7 200 000}` and one run a
// day. The 06:00 run spends the budget, so 06:30 gives one refused record; 08:30 gives one
// expired record of the same occurrence; `actingd suspended` shows the refused entry settled
// `expired`, beside the expired entry.
#[test]
fn k4_t2_a_refused_occurrence_past_its_grace_is_recorded_expired_and_its_refusal_settles_expired() {
    let sources = v2(slot_sources(1, &[missed_task(&[360, 390], 1)], None));
    let mut slots = Slots::start(&sources, at(0, 6, 0));
    slots.dispatch(at(0, 6, 1), SLOT_TASK);
    slots.new_rows();

    slots.evaluate(at(0, 6, 30));
    let rows = slots.new_rows();
    assert_eq!(
        lapses(&rows),
        [("refused", Some(at(0, 6, 30)), Some(at(1, 6, 0)))],
        "{rows:#?}"
    );
    assert_eq!(rows[0].text("gate_code"), Some("policy_budget_exhausted"));
    slots.evaluate(at(0, 8, 29));
    assert!(slots.new_rows().is_empty());
    slots.evaluate(at(0, 8, 30));
    let rows = slots.new_rows();
    assert_eq!(
        lapses(&rows),
        [("expired", Some(at(0, 6, 30)), Some(at(1, 6, 0)))],
        "the expiry is written after the refusal: {rows:#?}"
    );
    assert_eq!(rows[0].time("grace_ms"), Some(GRACE_MS));

    let root = slots.close_keeping_root();
    let report = report(&root, &sources);
    let entries = report["clock_slots"]
        .as_array()
        .unwrap_or_else(|| panic!("clock_slots is a list: {report}"));
    let fate = |code: &str| {
        let entry = entries
            .iter()
            .find(|entry| entry["code"] == json!(code))
            .unwrap_or_else(|| panic!("a {code} entry: {report}"));
        assert_eq!(entry["occurrence_unix_ms"], json!(at(0, 6, 30)), "{entry}");
        (
            entry["slot_fate"].clone(),
            entry["slot_fate_settled"].clone(),
        )
    };
    assert_eq!(fate("clock_slot_refused"), (json!("expired"), json!(true)));
    assert_eq!(fate("clock_slot_expired"), (json!("expired"), json!(true)));
}

// K4-T3, superseded: `daily [06:00, 19:00]` without `missed`, dispatched on day 1 at 06:05; no
// evaluation from day 1 18:00 until day 2 06:10 (the host is closed). The first evaluation of
// day 2 writes one `clock_slot_superseded` {O day 1 19:00, next day 2 06:00} at warning, and
// the 06:00 run goes ahead.
#[test]
fn k4_t3_an_occurrence_absorbed_while_the_host_was_closed_is_recorded_superseded() {
    let sources = slot_sources(1, &[slot_task(&[360, 1140], 2)], None);
    let mut slots = Slots::start(&sources, at(1, 6, 0));
    slots.dispatch(at(1, 6, 5), SLOT_TASK);
    slots.evaluate(at(1, 17, 59));
    assert!(slots.new_rows().is_empty());

    let mut slots = slots.restart(at(2, 6, 10));
    slots.dispatch(at(2, 6, 10), SLOT_TASK);
    let rows = slots.new_rows();
    assert_eq!(
        lapses(&rows),
        [("superseded", Some(at(1, 19, 0)), Some(at(2, 6, 0)))],
        "{rows:#?}"
    );
    assert_eq!(rows[0].code, "clock_slot_superseded");
    assert_eq!(rows[0].observed_at_unix_ms, at(2, 6, 10));
    let id = signal_id(
        SlotRecord::Superseded,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        at(1, 19, 0),
    );
    assert_eq!(
        recorded_outcome(&slots.host, &id).category(),
        Category::Warning
    );
    slots.close();
}

// K4-T4, not live, no history, already traced. A follower `all(clock {missed},
// dependency_completed(head, succeeded))` whose head failed gets no expired record; a pair
// that was never dispatched gets none; an occurrence that has a refused record and collapses
// into the next one gets no superseded record.
#[test]
fn k4_t4_a_pair_that_is_not_live_never_ran_or_was_already_traced_gets_no_lapse_record() {
    const HEAD: &str = "fixture.head";
    const FOLLOWER: &str = "fixture.follower";
    let head = SlotTask {
        id: HEAD,
        priority: 200,
        trigger: daily_clock(&[390], None),
        daily_limit: 1,
    };
    let follower = SlotTask {
        id: FOLLOWER,
        priority: 100,
        trigger: json!({
            "kind": "all",
            "predicates": [
                daily_clock(&[390], Some(GRACE_MS)),
                {"kind": "dependency_completed", "task_id": HEAD, "terminal_states": ["succeeded"]}
            ]
        }),
        daily_limit: 1,
    };
    let mut slots = Slots::start(&v2(slot_sources(1, &[head, follower], None)), at(0, 6, 0));
    slots.dispatch(at(0, 6, 30), HEAD);
    slots.dispatch(at(0, 6, 31), FOLLOWER);
    dispatch_failed(&mut slots, at(1, 6, 30), HEAD);
    for unix_ms in [at(1, 6, 31), at(1, 8, 30), at(1, 9, 0)] {
        slots.evaluate(unix_ms);
    }
    let rows = slots.new_rows();
    assert!(
        rows.iter().all(|row| row.task_id != FOLLOWER),
        "a follower whose head failed is not live: {rows:#?}"
    );
    slots.close();

    let mut slots = Slots::start(
        &v2(slot_sources(1, &[missed_task(&[390], 1)], None)),
        at(0, 6, 0),
    );
    slots.evaluate(at(0, 6, 20));
    let holder = slots.hold();
    for unix_ms in [at(0, 6, 30), at(0, 8, 30)] {
        slots.evaluate(unix_ms);
    }
    slots.evaluate(at(0, 8, 35));
    Slots::release(holder);
    let cycle = slots.evaluate(at(0, 9, 0));
    assert!(
        !selects(&cycle, SLOT_TASK),
        "R7's grace bounds the first run"
    );
    let rows = slots.new_rows();
    assert!(
        rows.is_empty(),
        "a pair that was never dispatched: {rows:#?}"
    );
    slots.close();

    let mut slots = Slots::start(
        &slot_sources(1, &[slot_task(&[360, 1140], 1)], None),
        at(0, 6, 0),
    );
    slots.dispatch(at(0, 6, 5), SLOT_TASK);
    slots.evaluate(at(0, 19, 0));
    let rows = slots.new_rows();
    assert_eq!(
        lapses(&rows),
        [("refused", Some(at(0, 19, 0)), Some(at(1, 6, 0)))],
        "{rows:#?}"
    );
    slots.dispatch(at(1, 6, 1), SLOT_TASK);
    let rows = slots.new_rows();
    assert!(
        rows.iter().all(|row| row.kind != "superseded"),
        "the refused occurrence already has its trace: {rows:#?}"
    );
    slots.close();
}

// K4-T5, the look-back: after three days without evaluations, superseded records are written
// only for occurrences within 48 h of the evaluation, and for at most 32 occurrences of a pair
// (the newest, the one still due among them).
#[test]
fn k4_t5_superseded_records_reach_back_48_hours_and_32_occurrences() {
    let mut slots = Slots::start(
        &slot_sources(1, &[slot_task(&[360, 1140], 2)], None),
        at(0, 6, 0),
    );
    slots.dispatch(at(0, 6, 5), SLOT_TASK);
    slots.new_rows();
    let mut slots = slots.restart(at(3, 6, 10));
    slots.dispatch(at(3, 6, 10), SLOT_TASK);
    let mut rows = lapses(&slots.new_rows())
        .into_iter()
        .map(|(kind, occurrence, next)| (kind.to_owned(), occurrence, next))
        .collect::<Vec<_>>();
    rows.sort_by_key(|(_, occurrence, _)| *occurrence);
    assert_eq!(
        rows,
        [
            (
                "superseded".to_owned(),
                Some(at(1, 19, 0)),
                Some(at(2, 6, 0))
            ),
            (
                "superseded".to_owned(),
                Some(at(2, 6, 0)),
                Some(at(2, 19, 0))
            ),
            (
                "superseded".to_owned(),
                Some(at(2, 19, 0)),
                Some(at(3, 6, 0))
            ),
        ],
        "day 0 19:00 and day 1 06:00 lie more than 48 h back"
    );
    slots.close();

    let hourly = SlotTask {
        id: SLOT_TASK,
        priority: 100,
        trigger: json!({
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": HOUR_MS,
                "anchor_ms": 0
            }
        }),
        daily_limit: 24,
    };
    let mut slots = Slots::start(&slot_sources(1, &[hourly], None), at(0, 6, 0));
    slots.dispatch(at(0, 6, 5), SLOT_TASK);
    slots.new_rows();
    let mut slots = slots.restart(at(3, 6, 10));
    slots.evaluate(at(3, 6, 10));
    let mut occurrences = slots
        .new_rows()
        .iter()
        .filter(|row| row.kind == "superseded")
        .map(|row| {
            (
                row.time("occurrence_unix_ms").expect("an occurrence"),
                row.time("next_occurrence_unix_ms").expect("its successor"),
            )
        })
        .collect::<Vec<_>>();
    occurrences.sort_unstable();
    // The 32 newest occurrences are day 1 23:00 to day 3 06:00; the newest is still due.
    let expected = (0..31)
        .map(|hour| {
            let occurrence = at(1, 23, 0) + hour * HOUR_MS;
            (occurrence, occurrence + HOUR_MS)
        })
        .collect::<Vec<_>>();
    assert_eq!(occurrences, expected);
    slots.close();
}

// K4-T6, once and round trip: across 60 s cycles and a restart a superseded occurrence keeps
// one record (a client claim keeps the pair waiting, so every cycle reads the occurrence
// again); an expired and a superseded record serialize with exactly the planning-signal
// key set, decode and encode to the same bytes, and their projection rows decode to them; the
// `signal_id`s of both kinds match their golden digest, computed apart from the code under
// test with `py -3 -c "import hashlib; print(hashlib.sha256(b'\x00'.join([b'fixture.slot',
// b'fixture-instance-a', b'1700042400000'])).hexdigest())"` (K2's golden case).
#[test]
fn k4_t6_each_lapse_is_written_once_round_trips_and_keeps_its_golden_id() {
    let sources = slot_sources(1, &[slot_task(&[360, 1140], 2)], None);
    let mut slots = Slots::start(&sources, at(1, 6, 0));
    slots.dispatch(at(1, 6, 5), SLOT_TASK);
    let mut slots = slots.restart(at(2, 6, 10));
    let holder = slots.hold();
    for unix_ms in [at(2, 6, 10), at(2, 6, 11), at(2, 6, 12)] {
        slots.evaluate(unix_ms);
    }
    Slots::release(holder);
    let mut slots = slots.restart(at(2, 6, 13));
    slots.evaluate(at(2, 6, 13));
    let superseded = slot_rows(&slots.host)
        .into_iter()
        .filter(|row| row.kind == "superseded")
        .count();
    assert_eq!(superseded, 1, "one record per pair, occurrence and code");
    slots.close();

    const DIGEST: &str = "71e71662307a84a87c8c3106c9825315b88842d26ebdaa8bae127ab06c2facc2";
    assert_eq!(at(0, 10, 0), 1_700_042_400_000);
    for (kind, word) in [
        (SlotRecord::Expired, "expired"),
        (SlotRecord::Superseded, "superseded"),
    ] {
        assert_eq!(
            signal_id(kind, SLOT_TASK, POLICY_INSTANCE_ALIAS, at(0, 10, 0)),
            format!("signal:clock_slot:{word}:{DIGEST}")
        );
    }

    let expired = SlotValues {
        occurrence_unix_ms: at(1, 6, 30),
        grace_ms: Some(GRACE_MS),
        next_occurrence_unix_ms: Some(at(2, 6, 30)),
        ..SlotValues::default()
    };
    let superseded = SlotValues {
        occurrence_unix_ms: at(1, 19, 0),
        next_occurrence_unix_ms: Some(at(2, 6, 0)),
        ..SlotValues::default()
    };
    let signals = [
        (SlotRecord::Expired, &expired, at(1, 8, 30)),
        (SlotRecord::Superseded, &superseded, at(2, 6, 10)),
    ]
    .map(|(kind, values, observed_at_unix_ms)| {
        clock_slot_signal(
            kind,
            SLOT_TASK,
            POLICY_INSTANCE_ALIAS,
            values,
            observed_at_unix_ms,
        )
    });
    let slots = Slots::start(&sources, at(2, 7, 0));
    for signal in &signals {
        slots
            .host
            .record_policy_planning_signal(signal.clone())
            .expect("record the clock-slot signal");
    }
    let events = slots
        .host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        })
        .expect("planning signals");
    let payloads = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload)) => Some(payload),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(payloads.len(), signals.len());
    for (payload, signal) in payloads.iter().zip(&signals) {
        assert_eq!(payload.signal_id(), signal.signal_id);
        assert_eq!(payload.outcome(), &signal.outcome);
        let bytes = serde_json::to_vec(payload).expect("encode the record");
        let record: Value = serde_json::from_slice(&bytes).expect("record JSON");
        let keys = record
            .as_object()
            .expect("a record object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "action",
                "signal_id",
                "instance_id",
                "task_id",
                "kind",
                "outcome",
                "observed_at_unix_ms",
                "audit",
            ])
        );
        let decoded: PolicyPlanningSignalPayload =
            serde_json::from_slice(&bytes).expect("decode the record");
        assert_eq!(
            serde_json::to_vec(&decoded).expect("encode the record again"),
            bytes
        );
    }
    let root = slots.close_keeping_root();
    let state =
        RuntimeStateStore::open(root.path(), b"runtime-host-test-salt").expect("open the state");
    for signal in &signals {
        let (_, stored) = state
            .load_planning_signal(&signal.signal_id)
            .expect("read the projection")
            .expect("a projection row");
        assert_eq!(&stored, signal);
    }
}

// K4-T7, the view half: a ledger with a refused record, an expired record of the same
// occurrence and a superseded record of another gives `clock_slots` entries for both new codes,
// the expired entry and the refused one settled `expired`, the superseded one settled
// `collapsed`.
#[test]
fn k4_t7_actingd_suspended_lists_expired_and_superseded_entries_with_their_fate() {
    let sources = slot_sources(1, &[slot_task(&[390, 1140], 2)], None);
    let refused = SlotValues {
        gate_code: Some(HostCode::PolicyBudgetExhausted.code()),
        budget_dimension: Some(actingcommand_contract::PolicyBudgetDimension::TaskDaily),
        occurrence_unix_ms: at(1, 6, 30),
        window_end_unix_ms: Some(at(1, 23, 30)),
        next_eligible_unix_ms: Some(at(2, 6, 0)),
        next_occurrence_unix_ms: Some(at(1, 19, 0)),
        ..SlotValues::default()
    };
    let expired = SlotValues {
        occurrence_unix_ms: at(1, 6, 30),
        grace_ms: Some(GRACE_MS),
        next_occurrence_unix_ms: Some(at(1, 19, 0)),
        ..SlotValues::default()
    };
    let superseded = SlotValues {
        occurrence_unix_ms: at(1, 19, 0),
        next_occurrence_unix_ms: Some(at(2, 6, 30)),
        ..SlotValues::default()
    };
    let slots = Slots::start(&sources, at(2, 7, 0));
    let mut ids = Vec::new();
    for (kind, values, observed_at_unix_ms) in [
        (SlotRecord::Refused, &refused, at(1, 6, 30)),
        (SlotRecord::Expired, &expired, at(1, 8, 30)),
        (SlotRecord::Superseded, &superseded, at(2, 6, 40)),
    ] {
        let signal = clock_slot_signal(
            kind,
            SLOT_TASK,
            POLICY_INSTANCE_ALIAS,
            values,
            observed_at_unix_ms,
        );
        ids.push(signal.signal_id.clone());
        slots
            .host
            .record_policy_planning_signal(signal)
            .expect("record the clock-slot signal");
    }
    let root = slots.close_keeping_root();
    let events = ledger_events(&root);
    let report = report(&root, &sources);
    let recorded = |signal_id: &str| {
        let sequence = record_sequence(&events, signal_id);
        let time = events
            .iter()
            .find(|event| event.sequence() == sequence)
            .map(PersistedEvent::timestamp_unix_ms)
            .expect("the record's event");
        (sequence, time)
    };
    let (refused_sequence, refused_time) = recorded(&ids[0]);
    let (expired_sequence, expired_time) = recorded(&ids[1]);
    let (superseded_sequence, superseded_time) = recorded(&ids[2]);
    assert_eq!(
        report["clock_slots"],
        json!([
            {
                "code": "clock_slot_superseded",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "occurrence_unix_ms": at(1, 19, 0),
                "next_occurrence_unix_ms": at(2, 6, 30),
                "slot_fate": "collapsed",
                "slot_fate_settled": true,
                "recorded_sequence": superseded_sequence,
                "recorded_at_unix_ms": superseded_time,
            },
            {
                "code": "clock_slot_expired",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "occurrence_unix_ms": at(1, 6, 30),
                "grace_ms": GRACE_MS,
                "next_occurrence_unix_ms": at(1, 19, 0),
                "slot_fate": "expired",
                "slot_fate_settled": true,
                "recorded_sequence": expired_sequence,
                "recorded_at_unix_ms": expired_time,
            },
            {
                "code": "clock_slot_refused",
                "task_id": SLOT_TASK,
                "instance_id": POLICY_INSTANCE_ALIAS,
                "gate_code": "policy_budget_exhausted",
                "budget_dimension": "task_daily",
                "occurrence_unix_ms": at(1, 6, 30),
                "window_end_unix_ms": at(1, 23, 30),
                "next_eligible_unix_ms": at(2, 6, 0),
                "next_occurrence_unix_ms": at(1, 19, 0),
                "slot_fate": "expired",
                "slot_fate_settled": true,
                "recorded_sequence": refused_sequence,
                "recorded_at_unix_ms": refused_time,
            },
        ]),
        "{report}"
    );
}
