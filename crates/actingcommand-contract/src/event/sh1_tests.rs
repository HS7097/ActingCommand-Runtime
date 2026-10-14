// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 SH-1, the v0.12.0 shape slice: every shape of `contracts/v012-register.md` that
// names SH-1 is decoded, validated and encoded again by its reader. Each test names its register
// rows. No writer exists before the slices the register names, so every stored record here is
// either a JSON record of the registered shape or a record an existing writer makes, extended
// with the new member.

use super::*;
use crate::codes::RuntimeContractCode;
use crate::{
    ContainedTaskCancellationReason, InputPathPoint, MAX_PATH_DRAG_POINTS, PackRegistryFactKey,
    PerformancePressureLevel, RuntimeFactScope, TaskTimingObservations,
    TaskTimingPhaseObservations, TaskTimingStepPacing, TimingObservationClock,
};
use serde_json::{Map, Value, json};

const AT_MS: u64 = 1_752_147_200_000;

fn ids() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn token(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("a JSON value")
}

fn epoch(ids: &IdentifierIssuer) -> Value {
    token(*ids.mint_owner_epoch().expect("owner epoch").transport())
}

fn outcome(code: &str, category: &str) -> Value {
    json!({"code": code, "category": category, "values": {}})
}

fn record(family: &str, kind: &str, data: Value) -> Value {
    json!({"family": family, "payload": {"kind": kind, "data": data}})
}

fn severity(category: &str) -> EventSeverity {
    match category {
        "info" => EventSeverity::Info,
        "warning" => EventSeverity::Warning,
        "error" => EventSeverity::Error,
        other => panic!("no severity for {other}"),
    }
}

// The reader: decode, validate, and encode back to the same record.
fn read_back(stored: &Value) -> EventPayload {
    let payload: EventPayload = serde_json::from_value(stored.clone()).expect("the record decodes");
    payload.validate().expect("the record validates");
    assert_eq!(
        &serde_json::to_value(&payload).expect("the record encodes"),
        stored,
        "the record reads back unchanged"
    );
    payload
}

fn refused(stored: Value) -> SanitizationError {
    let payload: EventPayload = serde_json::from_value(stored).expect("the record decodes");
    payload
        .validate()
        .expect_err("the reader refuses the record")
}

fn data(stored: &mut Value) -> &mut Map<String, Value> {
    stored
        .pointer_mut("/payload/data")
        .and_then(Value::as_object_mut)
        .expect("a payload's data")
}

fn with(mut stored: Value, member: &str, value: Value) -> Value {
    data(&mut stored).insert(member.to_owned(), value);
    stored
}

fn without(mut stored: Value, member: &str) -> Value {
    data(&mut stored).remove(member);
    stored
}

fn state_of(stored: &mut Value) -> &mut Map<String, Value> {
    stored
        .pointer_mut("/payload/data/state")
        .and_then(Value::as_object_mut)
        .expect("a value record's state")
}

// A payload an existing writer makes, as it is stored.
fn written(draft: EventPayloadDraft) -> Value {
    let payload = draft
        .sanitize(&SpyFingerprinter::new())
        .expect("the draft sanitizes");
    payload.validate().expect("the written record validates");
    token(payload)
}

fn dispatch_data() -> PolicyDispatchEventData {
    PolicyDispatchEventData {
        decision_id: "decision:fixture-a".to_owned(),
        task_id: "task:fixture-a".to_owned(),
        instance_id: "instance:fixture-a".to_owned(),
        operation_id: "operation:fixture-a".to_owned(),
        package_digest: format!("sha256:{}", "c".repeat(64)).into(),
        procedure_binding_digest: format!("sha256:{}", "d".repeat(64)),
        reason_chain_id: "reason:fixture-a".to_owned(),
        reasons: vec![PolicyReasonRecord(Outcome::relayed("task_eligible"))],
        catalog_hash: format!("sha256:{}", "b".repeat(64)),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "snapshot:fixture-a".to_owned(),
        approval_fact_ids: Vec::new(),
        urgency_milli: 500,
    }
}

fn admission() -> PolicyAdmissionRecord {
    PolicyAdmissionRecord {
        activity: PolicyActivitySample {
            profile_id: "activity:fixture-a".to_owned(),
            local_day: 20_000,
            window_id: "activity:fixture-a:20000:0".to_owned(),
            admitted_at_unix_ms: AT_MS,
            seed: 7,
            interval_ms: 60_000,
            next_eligible_unix_ms: AT_MS + 60_000,
        },
        budget: PolicyBudgetReceipt {
            task_daily_used: 1,
            task_daily_limit: 24,
            task_window_used: 1,
            task_window_limit: 4,
            task_runtime_reserved_ms: 60_000,
            task_runtime_limit_ms: 300_000,
            activity_daily_used: 1,
            activity_daily_limit: 24,
            activity_window_used: 1,
            activity_window_limit: 4,
            activity_runtime_reserved_ms: 60_000,
            activity_runtime_limit_ms: 7_200_000,
        },
    }
}

fn wake(ids: &IdentifierIssuer, kind: AgentWakeKind) -> AgentWakeData {
    AgentWakeData::new(
        *ids.mint_agent_wake_id().expect("wake id").transport(),
        *ids.mint_correlation_id()
            .expect("correlation id")
            .transport(),
        AgentWakeTrigger::new(
            *ids.mint_instance_id().expect("instance id").transport(),
            kind,
            *ids.mint_event_id().expect("trigger id").transport(),
            1,
            AT_MS,
        )
        .expect("wake trigger"),
        AgentSessionBudget::new(2, 60_000).expect("budget"),
        AgentCapabilityContract::read_only(2).expect("capabilities"),
    )
    .expect("wake")
}

// The JSON object that holds `member`, wherever it is nested.
fn holder<'a>(value: &'a mut Value, member: &str) -> Option<&'a mut Map<String, Value>> {
    match value {
        Value::Object(object) if object.contains_key(member) => Some(object),
        Value::Object(object) => object.values_mut().find_map(|child| holder(child, member)),
        Value::Array(items) => items.iter_mut().find_map(|child| holder(child, member)),
        _ => None,
    }
}

fn lifecycle(ids: &IdentifierIssuer, phase: Value) -> Value {
    record(
        "runtime",
        "lifecycle_observed",
        json!({
            "action": "runtime.action",
            "owner_epoch": epoch(ids),
            "phase": phase,
            "audit": {},
        }),
    )
}

#[test]
fn l1_gap_records_read_back_and_keep_each_gap_kinds_shape() {
    let ids = ids();
    let lineage = epoch(&ids);
    let rejected = record(
        "ledger",
        "gap_recorded",
        json!({
            "action": "ledger.recovery",
            "gap_kind": "record_rejected",
            "gap_start_unix_ms": AT_MS,
            "gap_end_unix_ms": AT_MS,
            "record_count": 1,
            "event_id": token(*ids.mint_event_id().expect("event id").transport()),
            "source": {"source": "instance_facts", "lineage": lineage, "revision": 7},
            "outcome": outcome("ledger_record_rejected", "error"),
            "audit": {},
        }),
    );
    let payload = read_back(&rejected);
    assert_eq!(payload.event_type(), EventType::LedgerGapRecorded);
    assert_eq!(payload.family(), EventFamily::Ledger);
    assert_eq!(payload.action(), EventAction::LedgerRecovery);
    assert_eq!(
        record_severity(EventSeverity::Info, &payload),
        EventSeverity::Error
    );
    let EventPayload::Ledger(LedgerPayload::GapRecorded(gap)) = &payload else {
        panic!("a gap record");
    };
    assert_eq!(gap.gap_kind(), LedgerGapKind::RecordRejected);
    assert_eq!(gap.record_count(), Some(1));
    assert_eq!(
        gap.source().map(|source| (source.source, source.revision)),
        Some((LedgerSource::InstanceFacts, 7))
    );

    // A rejected record is one record at its capture time, with its event id and a change.
    for (member, value) in [
        ("record_count", json!(2)),
        ("gap_end_unix_ms", json!(AT_MS + 1)),
        (
            "source",
            json!({"source": "instance_facts", "lineage": lineage, "revision": 0}),
        ),
    ] {
        refused(with(rejected.clone(), member, value));
    }
    refused(without(rejected, "event_id"));

    for (kind, code, category, other) in [
        (
            "dump_incomplete",
            "ledger_dump_incomplete",
            "warning",
            "error",
        ),
        ("dump_invalid", "ledger_dump_invalid", "error", "warning"),
        (
            "tail_unconfirmed",
            "ledger_tail_unconfirmed",
            "warning",
            "error",
        ),
    ] {
        let gap = record(
            "ledger",
            "gap_recorded",
            json!({
                "action": "ledger.recovery",
                "gap_kind": kind,
                "gap_start_unix_ms": AT_MS,
                "gap_end_unix_ms": AT_MS + 5_000,
                "outcome": outcome(code, category),
                "audit": {},
            }),
        );
        let payload = read_back(&gap);
        assert_eq!(
            record_severity(EventSeverity::Info, &payload),
            severity(category),
            "{kind}"
        );
        // The count of these gaps is unknown, and they name no record.
        refused(with(gap.clone(), "record_count", json!(3)));
        refused(with(
            gap.clone(),
            "event_id",
            token(*ids.mint_event_id().expect("event id").transport()),
        ));
        // The outcome's category is the gap kind's, and the time range is ordered.
        refused(with(gap.clone(), "outcome", outcome(code, other)));
        refused(with(gap, "gap_end_unix_ms", json!(AT_MS - 1)));
    }
}

#[test]
fn l2_l3_probe_findings_and_dump_imports_read_back() {
    let finding = record(
        "ledger",
        "health_observed",
        json!({
            "action": "ledger.probe",
            "outcome": {
                "code": "ledger_capture_spilled",
                "category": "warning",
                "values": {"segment_edge": "started", "count": 12},
            },
            "segment_edge": "started",
            "audit": {},
        }),
    );
    let payload = read_back(&finding);
    assert_eq!(payload.event_type(), EventType::LedgerHealthObserved);
    assert_eq!(payload.action(), EventAction::LedgerProbe);
    assert_eq!(
        record_severity(EventSeverity::Info, &payload),
        EventSeverity::Warning
    );
    let EventPayload::Ledger(LedgerPayload::HealthObserved(health)) = &payload else {
        panic!("a probe finding");
    };
    assert_eq!(health.segment_edge(), Some(LedgerSegmentEdge::Started));
    read_back(&without(finding.clone(), "segment_edge"));
    // A finding is a warning or an error, recorded under the probe action.
    refused(with(
        finding.clone(),
        "outcome",
        outcome("ledger_probe_done", "info"),
    ));
    refused(with(finding, "action", json!("ledger.recovery")));

    let imported = record(
        "ledger",
        "dump_imported",
        json!({
            "action": "ledger.recovery",
            "outcome": {
                "code": "ledger_dump_imported",
                "category": "info",
                "values": {"count": 3, "duplicate_count": 1, "dump_sealed": true},
            },
            "audit": {},
        }),
    );
    let payload = read_back(&imported);
    assert_eq!(payload.event_type(), EventType::LedgerDumpImported);
    assert_eq!(payload.action(), EventAction::LedgerRecovery);
    assert_eq!(
        record_severity(EventSeverity::Info, &payload),
        EventSeverity::Info
    );
    refused(with(
        imported.clone(),
        "outcome",
        outcome("ledger_dump_imported", "warning"),
    ));
    refused(with(imported, "action", json!("ledger.probe")));
}

#[test]
fn l4_a_record_capture_names_its_record_time() {
    let ids = ids();
    let stored = json!({
        "capture_seq": 41,
        "captured_at_unix_ms": AT_MS,
        "source": {"source": "fact_refresh_transitions", "lineage": epoch(&ids), "revision": 1},
    });
    let capture: RecordCapture = serde_json::from_value(stored.clone()).expect("a capture");
    capture
        .validate(AT_MS)
        .expect("the record's time is its capture time");
    assert_eq!(token(capture), stored);
    assert_eq!(
        capture.source.map(|source| source.source),
        Some(LedgerSource::FactRefreshTransitions)
    );
    capture
        .validate(AT_MS + 1)
        .expect_err("a record's time is its capture time");

    let unsourced = json!({"capture_seq": 42, "captured_at_unix_ms": AT_MS});
    let capture: RecordCapture = serde_json::from_value(unsourced.clone()).expect("a capture");
    capture.validate(AT_MS).expect("a capture without a source");
    assert_eq!(token(capture), unsourced);

    let mut unchanged = stored.clone();
    unchanged["source"]["revision"] = json!(0);
    serde_json::from_value::<RecordCapture>(unchanged)
        .expect("a capture")
        .validate(AT_MS)
        .expect_err("a source revision names a change");
    let mut unknown = stored;
    unknown["sequence"] = json!(1);
    assert!(serde_json::from_value::<RecordCapture>(unknown).is_err());
}

#[test]
fn l5_l6_tagged_state_phases_read_back() {
    let ids = ids();
    let stores = json!([
        {
            "source": "instance_facts",
            "previous_lineage": epoch(&ids),
            "previous_revision": 12,
            "lineage": epoch(&ids),
            "loaded_count": 3,
        },
        {"source": "runtime_facts", "lineage": epoch(&ids), "loaded_count": 0},
        {
            "source": "decision_memory",
            "previous_lineage": epoch(&ids),
            "previous_revision": 4,
            "lineage": epoch(&ids),
            "loaded_count": 2,
        },
    ]);
    let loaded = lifecycle(
        &ids,
        json!({"kind": "tagged_state_loaded", "stores": stores.clone()}),
    );
    let payload = read_back(&loaded);
    assert_eq!(payload.event_type(), EventType::RuntimeLifecycleObserved);
    let EventPayload::Runtime(RuntimePayload::LifecycleObserved(observed)) = &payload else {
        panic!("a lifecycle record");
    };
    let RuntimeLifecyclePhase::TaggedStateLoaded { stores: read } = observed.phase() else {
        panic!("the tagged_state_loaded phase");
    };
    assert_eq!(read.len(), 3);
    assert_eq!(read[0].previous_revision, Some(12));
    assert_eq!(read[1].previous_lineage, None);

    let phase = |stores: Value| {
        lifecycle(
            &ids,
            json!({"kind": "tagged_state_loaded", "stores": stores}),
        )
    };
    let mut twice = stores.clone();
    twice[1]["source"] = json!("instance_facts");
    let mut half = stores.clone();
    half[0]
        .as_object_mut()
        .expect("a store")
        .remove("previous_revision");
    let mut same = stores.clone();
    same[0]["lineage"] = same[0]["previous_lineage"].clone();
    for bad in [twice, half, same, json!([])] {
        refused(phase(bad));
    }

    let enabled = lifecycle(&ids, json!({"kind": "tagged_state_enabled"}));
    let payload = read_back(&enabled);
    let EventPayload::Runtime(RuntimePayload::LifecycleObserved(observed)) = &payload else {
        panic!("a lifecycle record");
    };
    assert_eq!(observed.phase(), RuntimeLifecyclePhase::TaggedStateEnabled);
}

#[test]
fn l7_l14_decision_memory_value_records_read_back() {
    let state = record(
        "policy",
        "control_state_recorded",
        json!({
            "action": "policy.control_state",
            "state": {
                "instance_id": "instance:fixture-a",
                "task_id": "task:fixture-a",
                "daily_counts": [
                    {"local_day": 19_999, "count": 4},
                    {"local_day": 20_000, "count": 2},
                ],
                "window_counts": 1,
                "window_runtime_ms": 45_000,
                "active_window": "activity:fixture-a:20000:0",
                "failure_streak": {
                    "failure_code": "contained_task_page_unknown",
                    "failure_identity": "identity:fixture-a",
                    "class": "recoverable",
                    "count": 2,
                    "escalation_streak": 1,
                    "disposition": "retry_scheduled",
                    "retry_at_unix_ms": AT_MS + 60_000,
                    "decision_id": "decision:fixture-a",
                },
                "retry_round": {
                    "admission": token(admission()),
                    "until_unix_ms": AT_MS + 3_600_000,
                    "expected_duration_ms": 60_000,
                },
                "dispatch_clock": {"admitted_at_unix_ms": AT_MS},
                "condition_reruns": {
                    "failure_code": "perf_run_yielded",
                    "count": 1,
                    "expires_at_unix_ms": AT_MS + 86_400_000,
                },
            },
            "audit": {},
        }),
    );
    let payload = read_back(&state);
    assert_eq!(payload.event_type(), EventType::PolicyControlStateRecorded);
    assert_eq!(payload.family(), EventFamily::Policy);
    assert_eq!(payload.action(), EventAction::PolicyControlState);
    let EventPayload::Policy(PolicyPayload::ControlStateRecorded(recorded)) = &payload else {
        panic!("a decision-memory record");
    };
    let recorded = recorded.state();
    assert_eq!(recorded.daily_counts.len(), 2);
    assert_eq!(recorded.window_counts, 1);
    assert_eq!(
        recorded
            .failure_streak
            .as_ref()
            .map(|streak| streak.disposition),
        Some(PolicyFailureDisposition::RetryScheduled)
    );
    assert_eq!(
        recorded
            .condition_reruns
            .as_ref()
            .map(|reruns| reruns.count),
        Some(1)
    );

    // A pair that has counted nothing in a window names no window; the other parts are
    // optional.
    let mut idle = state.clone();
    let idle_state = state_of(&mut idle);
    for member in [
        "active_window",
        "failure_streak",
        "retry_round",
        "dispatch_clock",
        "condition_reruns",
    ] {
        idle_state.remove(member);
    }
    idle_state.insert("window_counts".to_owned(), json!(0));
    idle_state.insert("window_runtime_ms".to_owned(), json!(0));
    read_back(&idle);
    let edits: [(&str, Value); 5] = [
        (
            "daily_counts",
            json!([{"local_day": 1, "count": 1}, {"local_day": 1, "count": 2}]),
        ),
        ("instance_id", json!("")),
        (
            "failure_streak",
            json!({
                "failure_code": "contained_task_page_unknown",
                "class": "recoverable",
                "count": 0,
                "escalation_streak": 0,
                "disposition": "continue",
                "decision_id": "decision:fixture-a",
            }),
        ),
        (
            "condition_reruns",
            json!({"failure_code": "perf_run_yielded", "count": 0, "expires_at_unix_ms": AT_MS}),
        ),
        ("dispatch_clock", json!({"admitted_at_unix_ms": 0})),
    ];
    for (member, value) in edits {
        let mut bad = state.clone();
        state_of(&mut bad).insert(member.to_owned(), value);
        refused(bad);
    }
    let mut windowless = state.clone();
    state_of(&mut windowless).remove("active_window");
    refused(windowless);
    refused(with(state, "action", json!("policy.dispatch")));

    let activity = record(
        "policy",
        "activity_state_recorded",
        json!({
            "action": "policy.control_state",
            "state": {
                "instance_id": "instance:fixture-a",
                "profile_id": "activity:fixture-a",
                "local_day": 20_000,
                "daily_count": 3,
                "window_id": "activity:fixture-a:20000:0",
                "window_count": 1,
                "window_runtime_ms": 45_000,
                "next_eligible_unix_ms": AT_MS + 60_000,
            },
            "audit": {},
        }),
    );
    let payload = read_back(&activity);
    assert_eq!(payload.event_type(), EventType::PolicyActivityStateRecorded);
    assert_eq!(payload.action(), EventAction::PolicyControlState);
    let EventPayload::Policy(PolicyPayload::ActivityStateRecorded(recorded)) = &payload else {
        panic!("an activity-state record");
    };
    assert_eq!(recorded.state().next_eligible_unix_ms, Some(AT_MS + 60_000));
    let mut uneligible = activity.clone();
    state_of(&mut uneligible).insert("next_eligible_unix_ms".to_owned(), json!(0));
    refused(uneligible);
    refused(with(activity, "action", json!("policy.planning")));
}

#[test]
fn l8_a_wake_obligation_reads_back() {
    let ids = ids();
    let obligation = wake(&ids, AgentWakeKind::TimelineReached);
    let stored = record(
        "agent",
        "wake_obligation_recorded",
        json!({"action": "agent.wake", "wake": token(&obligation), "audit": {}}),
    );
    let payload = read_back(&stored);
    assert_eq!(payload.event_type(), EventType::AgentWakeObligationRecorded);
    assert_eq!(payload.family(), EventFamily::Agent);
    assert_eq!(payload.action(), EventAction::AgentWake);
    let EventPayload::Agent(AgentPayload::WakeObligationRecorded(recorded)) = &payload else {
        panic!("a wake obligation");
    };
    assert_eq!(recorded.wake(), &obligation);
    refused(with(stored, "action", json!("agent.session_start")));
}

#[test]
fn l9_dispatch_records_read_input_fact_revisions_or_a_non_zero_position() {
    let ids = ids();
    let intent =
        written(PolicyPayloadDraft::dispatch_intent(dispatch_data(), AuditInput::new()).into());
    // The writers keep writing the position (order-v012 §3.5 item 11).
    assert_eq!(intent["payload"]["data"]["input_ledger_position"], 1);
    assert!(
        intent["payload"]["data"]
            .get("input_fact_revisions")
            .is_none()
    );
    let revisions = json!({
        "instance_facts": {"lineage": epoch(&ids), "revision": 3},
        "runtime_facts": {"lineage": epoch(&ids), "revision": 9},
    });

    let revised = with(
        without(intent.clone(), "input_ledger_position"),
        "input_fact_revisions",
        revisions.clone(),
    );
    let payload = read_back(&revised);
    let EventPayload::Policy(PolicyPayload::DispatchIntent(dispatch)) = &payload else {
        panic!("a dispatch intent");
    };
    assert_eq!(dispatch.input_ledger_position(), 0);
    assert_eq!(
        dispatch
            .input_fact_revisions()
            .map(|read| (read.instance_facts.revision, read.runtime_facts.revision)),
        Some((3, 9))
    );
    read_back(&with(intent.clone(), "input_fact_revisions", revisions));

    // A present position is never 0, and a record names what it read.
    assert!(
        serde_json::from_value::<EventPayload>(with(
            intent.clone(),
            "input_ledger_position",
            json!(0)
        ))
        .is_err()
    );
    let error = refused(without(intent, "input_ledger_position"));
    assert_eq!(
        error.code(),
        SanitizationCode::InvalidPolicyDispatchPosition.code()
    );
}

#[test]
fn l9_selection_records_read_input_fact_revisions_or_a_non_zero_position() {
    let ids = ids();
    let selection = written(revision_3_selection(TaskSelectionConfirmation::PageChanged));
    let revisions = json!({
        "instance_facts": {"lineage": epoch(&ids), "revision": 5},
        "runtime_facts": {"lineage": epoch(&ids), "revision": 2},
    });
    let mut revised = selection.clone();
    let selected = holder(&mut revised, "input_ledger_position").expect("the selection record");
    assert_eq!(selected["input_ledger_position"], 1);
    selected.remove("input_ledger_position");
    selected.insert("input_fact_revisions".to_owned(), revisions);
    let payload = read_back(&revised);
    let EventPayload::Task(TaskPayload::Semantic(semantic)) = &payload else {
        panic!("a task record");
    };
    let TaskSemanticFact::SelectionEvaluated {
        selection: read, ..
    } = semantic.fact()
    else {
        panic!("a selection record");
    };
    assert_eq!(read.input_ledger_position, 0);
    assert_eq!(
        read.input_fact_revisions
            .map(|read| read.instance_facts.revision),
        Some(5)
    );

    let mut zero = selection.clone();
    holder(&mut zero, "input_ledger_position")
        .expect("the selection record")
        .insert("input_ledger_position".to_owned(), json!(0));
    assert!(serde_json::from_value::<EventPayload>(zero).is_err());
    let mut unread = selection;
    holder(&mut unread, "input_ledger_position")
        .expect("the selection record")
        .remove("input_ledger_position");
    refused(unread);
}

#[test]
fn l13_the_view_lists_gain_the_new_record_types() {
    let views = |event_type, severity| {
        LedgerView::memberships(event_type, severity, EventSource::Runtime, false)
    };
    for event_type in [
        EventType::LedgerGapRecorded,
        EventType::LedgerHealthObserved,
        EventType::LedgerDumpImported,
        EventType::PerformancePacingChanged,
        EventType::PerformanceDispatchShareChanged,
        EventType::PerformanceRunHold,
    ] {
        let joined = views(event_type, EventSeverity::Info);
        assert!(joined.contains(&LedgerView::Health), "{event_type:?}");
        assert!(!joined.contains(&LedgerView::Changes), "{event_type:?}");
    }
    for event_type in [
        EventType::PolicyControlStateRecorded,
        EventType::PolicyActivityStateRecorded,
        EventType::PolicyFactRefreshTransition,
        EventType::AgentWakeObligationRecorded,
    ] {
        let joined = views(event_type, EventSeverity::Info);
        assert!(joined.contains(&LedgerView::Changes), "{event_type:?}");
        assert!(!joined.contains(&LedgerView::Health), "{event_type:?}");
    }
    assert!(
        views(EventType::LedgerGapRecorded, EventSeverity::Warning).contains(&LedgerView::Errors)
    );
    assert!(
        views(EventType::LedgerHealthObserved, EventSeverity::Error).contains(&LedgerView::Errors)
    );
}

#[test]
fn d1_a_fact_refresh_transition_reads_back() {
    let transition = record(
        "policy",
        "fact_refresh_transition",
        json!({
            "action": "policy.fact_refresh",
            "cycle_seq": 4,
            "instance_id": "instance:fixture-a",
            "fact_key": "inventory.item_a",
            "outcome": outcome("fact_refresh_resolved", "info"),
            "audit": {},
        }),
    );
    let payload = read_back(&transition);
    assert_eq!(payload.event_type(), EventType::PolicyFactRefreshTransition);
    assert_eq!(payload.family(), EventFamily::Policy);
    assert_eq!(payload.action(), EventAction::PolicyFactRefresh);
    assert_eq!(
        record_severity(EventSeverity::Info, &payload),
        EventSeverity::Info
    );
    let EventPayload::Policy(PolicyPayload::FactRefreshTransition(recorded)) = &payload else {
        panic!("a fact-refresh transition");
    };
    assert_eq!(
        (recorded.cycle_seq(), recorded.fact_key()),
        (4, "inventory.item_a")
    );
    refused(with(
        transition.clone(),
        "action",
        json!("policy.control_state"),
    ));
    refused(with(transition, "instance_id", json!("")));
}

fn condition_failure(condition: Option<PolicyConditionSettlement>) -> PolicyFailureRecord {
    PolicyFailureRecord {
        outcome: Outcome::relayed("contained_task_deadline_exceeded"),
        failure_identity: None,
        reported_success: false,
        original_class: PolicyFailureClass::Recoverable,
        effective_class: PolicyFailureClass::Recoverable,
        consecutive_same_error: 0,
        escalation_streak: 0,
        performance_tax_exempt: false,
        retry_attempt: 0,
        disposition: PolicyFailureDisposition::Continue,
        retry_at_unix_ms: None,
        runtime_ms: 1_000,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(AT_MS)),
        condition,
    }
}

fn execution(outcome: PolicyExecutionOutcome) -> PolicyExecutionEventData {
    PolicyExecutionEventData {
        decision_id: "decision:fixture-a".to_owned(),
        task_id: "task:fixture-a".to_owned(),
        instance_id: "instance:fixture-a".to_owned(),
        observed_at_unix_ms: AT_MS + 1_000,
        outcome,
    }
}

#[test]
fn s1_a_condition_settled_failure_reads_back() {
    // SCHED §4.2: a condition termination keeps the pair's previous counts (0 when none) and
    // its disposition `continue`.
    let settled = PolicyConditionSettlement {
        refunded: true,
        count: 1,
        expires_at_unix_ms: AT_MS + 86_400_000,
    };
    let stored = written(
        PolicyPayloadDraft::execution_recorded(
            execution(PolicyExecutionOutcome::Failed {
                failure: condition_failure(Some(settled)),
            }),
            AuditInput::new(),
        )
        .into(),
    );
    let payload = read_back(&stored);
    let EventPayload::Policy(PolicyPayload::ExecutionRecorded(recorded)) = &payload else {
        panic!("an execution record");
    };
    let PolicyExecutionOutcome::Failed { failure } = recorded.policy_result() else {
        panic!("a failed execution");
    };
    assert_eq!(failure.condition, Some(settled));

    let mut uncounted = stored.clone();
    holder(&mut uncounted, "condition")
        .expect("the failure")
        .insert(
            "condition".to_owned(),
            json!({"refunded": true, "count": 0, "expires_at_unix_ms": AT_MS}),
        );
    refused(uncounted);
    let mut expired = stored;
    holder(&mut expired, "condition")
        .expect("the failure")
        .insert(
            "condition".to_owned(),
            json!({"refunded": false, "count": 1, "expires_at_unix_ms": 0}),
        );
    refused(expired);
    // An ordinary failure still counts from 1.
    let error = EventPayloadDraft::from(PolicyPayloadDraft::execution_recorded(
        execution(PolicyExecutionOutcome::Failed {
            failure: condition_failure(None),
        }),
        AuditInput::new(),
    ))
    .sanitize(&SpyFingerprinter::new())
    .expect_err("an ordinary failure counts from 1");
    assert_eq!(
        error.code(),
        SanitizationCode::InvalidPolicyFailureRecord.code()
    );
}

#[test]
fn s2_p7_w4_cancelled_terminals_name_blackout_stop_and_performance_yield() {
    for (spelling, reason) in [
        (
            "blackout_run_stopped",
            ContainedTaskCancellationReason::BlackoutStop,
        ),
        (
            "perf_run_yielded",
            ContainedTaskCancellationReason::PerformanceYield,
        ),
        (
            "perf_hold_expired",
            ContainedTaskCancellationReason::PerformanceYield,
        ),
        (
            "contained_task_deadline_exceeded",
            ContainedTaskCancellationReason::DeadlineExceeded,
        ),
        (
            "contained_task_cancelled",
            ContainedTaskCancellationReason::ClientRequested,
        ),
        (
            "contained_task_paused",
            ContainedTaskCancellationReason::PausedByOperator,
        ),
        (
            "runtime_fatal",
            ContainedTaskCancellationReason::RecoveredAfterRestart,
        ),
    ] {
        assert_eq!(
            ContainedTaskCancellationReason::from_failure_code(Some(&received_code(spelling))),
            reason,
            "{spelling}"
        );
    }
    assert_eq!(
        ContainedTaskCancellationReason::from_failure_code(None),
        ContainedTaskCancellationReason::RecoveredAfterRestart
    );
    for (reason, spelling) in [
        (
            ContainedTaskCancellationReason::BlackoutStop,
            "blackout_stop",
        ),
        (
            ContainedTaskCancellationReason::PerformanceYield,
            "performance_yield",
        ),
    ] {
        assert_eq!(token(reason), json!(spelling));
        assert_eq!(
            serde_json::from_value::<ContainedTaskCancellationReason>(json!(spelling))
                .expect("the reason decodes"),
            reason
        );
    }
}

#[test]
fn s3_a_blackout_suppression_reads_back() {
    let ids = ids();
    let suppressed = lifecycle(
        &ids,
        json!({
            "kind": "recovery_ladder_suppressed",
            "reason": "blackout",
            "until_unix_ms": AT_MS + 600_000,
        }),
    );
    let payload = read_back(&suppressed);
    let EventPayload::Runtime(RuntimePayload::LifecycleObserved(observed)) = &payload else {
        panic!("a lifecycle record");
    };
    assert!(matches!(
        observed.phase(),
        RuntimeLifecyclePhase::RecoveryLadderSuppressed {
            reason: RecoveryLadderSuppression::Blackout,
            ..
        }
    ));
}

#[test]
fn p1_p3_pacing_transitions_read_back() {
    for (kind, event_type, code) in [
        (
            "pacing_changed",
            EventType::PerformancePacingChanged,
            "perf_stage_changed",
        ),
        (
            "dispatch_share_changed",
            EventType::PerformanceDispatchShareChanged,
            "perf_scheduling_paused",
        ),
        ("run_hold", EventType::PerformanceRunHold, "perf_run_held"),
    ] {
        let stored = record(
            "performance",
            kind,
            json!({
                "action": "performance.observe",
                "outcome": outcome(code, "info"),
                "audit": {},
            }),
        );
        let payload = read_back(&stored);
        assert_eq!(payload.event_type(), event_type);
        assert_eq!(payload.family(), EventFamily::Performance);
        assert_eq!(payload.action(), EventAction::PerformanceObserve);
        assert_eq!(
            record_severity(EventSeverity::Info, &payload),
            EventSeverity::Info
        );
        refused(with(stored.clone(), "outcome", outcome(code, "warning")));
        refused(with(stored, "action", json!("policy.dispatch")));
    }
}

#[test]
fn p4_perf_summary_pacing_members_read_back() {
    let summary = written(
        PerformancePayloadDraft::summary(
            PerformanceSummaryEventData {
                context: PerformanceContext::unavailable(AT_MS),
                foreground: None,
                owned_processes: Vec::new(),
                third_party_high_load: Vec::new(),
                ledger_commits: None,
                capacity: None,
            },
            AuditInput::new(),
        )
        .into(),
    );
    assert!(summary["payload"]["data"].get("pacing").is_none());
    let pacing = json!({
        "responsiveness_bp": 7_200,
        "host_responsiveness_bp": 8_100,
        "emulator_responsiveness_bp": 6_900,
        "pacing_stage": "s1",
        "factor_milli": 1_500,
        "dispatch_share_milli": 1_000,
        "pacing_mode": "full",
        "learning": true,
        "learning_view_stage": "s1",
        "learning_view_share_milli": 1_000,
        "learning_view_allowed": 3,
        "normal_view_stage": "s2",
        "normal_view_share_milli": 750,
        "normal_view_allowed": 3,
        "pacing_phase": "learning",
        "learn_days": 1,
        "learn_active_ms": 3_600_000,
        "instances": [{
            "instance_id": "instance:fixture-a",
            "learning_view_stage": "s1",
            "learning_view_share_milli": 1_000,
            "learning_view_allowed": true,
            "normal_view_stage": "s3",
            "normal_view_share_milli": 500,
            "normal_view_allowed": false,
        }],
    });
    let payload = read_back(&with(summary.clone(), "pacing", pacing.clone()));
    let read = payload
        .performance_summary()
        .and_then(PerformanceSummaryPayload::pacing)
        .expect("the pacing members");
    assert_eq!(read.pacing_stage, PerformancePacingStage::S1);
    assert_eq!(read.pacing_mode, PerformancePacingMode::Full);
    assert_eq!(read.pacing_phase, Some(PerformancePacingPhase::Learning));
    assert_eq!(
        read.instances[0].normal_view_stage,
        PerformancePacingStage::S3
    );

    let mut observing = pacing.clone();
    observing["pacing_mode"] = json!("observe");
    let mut beyond = pacing.clone();
    beyond["responsiveness_bp"] = json!(10_001);
    let mut oversized = pacing.clone();
    oversized["normal_view_share_milli"] = json!(1_001);
    let mut shortened = pacing.clone();
    shortened["factor_milli"] = json!(999);
    let mut repeated = pacing.clone();
    let row = repeated["instances"][0].clone();
    repeated["instances"]
        .as_array_mut()
        .expect("instance rows")
        .push(row);
    for bad in [observing, beyond, oversized, shortened, repeated] {
        refused(with(summary.clone(), "pacing", bad));
    }
}

#[test]
fn p5_p8_task_timing_steps_and_package_load_read_back() {
    let ids = ids();
    let timing = TaskTimingObservations {
        clock: TimingObservationClock::ProcessInstant,
        request_id: *ids.mint_request_id().expect("request id").transport(),
        admission_request_id: None,
        correlation_id: *ids
            .mint_correlation_id()
            .expect("correlation id")
            .transport(),
        task_id: *ids.mint_task_id().expect("task id").transport(),
        run_id: *ids.mint_run_id().expect("run id").transport(),
        preflight: TaskTimingPhaseObservations::default(),
        execution: TaskTimingPhaseObservations::default(),
        finalization: TaskTimingPhaseObservations::default(),
        task_failure: None,
        first_observed_expiry: None,
        steps: vec![
            TaskTimingStepPacing {
                step_index: 0,
                page_settle_ms: Some(420),
                pacing_band: Some(7_000),
                pressure_level: Some(PerformancePressureLevel::Elevated),
                pacing_factor_milli: Some(1_500),
                extra_wait_rounds: Some(1),
            },
            TaskTimingStepPacing {
                step_index: 2,
                page_settle_ms: None,
                pacing_band: Some(9_500),
                pressure_level: Some(PerformancePressureLevel::None),
                pacing_factor_milli: Some(1_000),
                extra_wait_rounds: None,
            },
        ],
        package_load_ms: Some(85),
        package_load_bytes: Some(262_144),
    };
    timing.validate().expect("the evidence validates");
    let stored = token(&timing);
    assert_eq!(stored["steps"][0]["pacing_band"], 7_000);
    assert_eq!(stored["steps"][0]["pressure_level"], "elevated");
    assert_eq!(stored["package_load_bytes"], 262_144);
    let read: TaskTimingObservations =
        serde_json::from_value(stored.clone()).expect("the evidence decodes");
    read.validate().expect("the decoded evidence validates");
    assert_eq!(read, timing);
    assert_eq!(token(read), stored);

    let plain = TaskTimingObservations {
        steps: Vec::new(),
        package_load_ms: None,
        package_load_bytes: None,
        ..timing.clone()
    };
    let plain = token(plain);
    for member in ["steps", "package_load_ms", "package_load_bytes"] {
        assert!(plain.get(member).is_none(), "{member}");
    }

    let step = |step_index, pacing_band, pacing_factor_milli| TaskTimingStepPacing {
        step_index,
        page_settle_ms: None,
        pacing_band,
        pressure_level: None,
        pacing_factor_milli,
        extra_wait_rounds: None,
    };
    for steps in [
        vec![step(2, None, None), step(0, None, None)],
        vec![step(1, None, None), step(1, None, None)],
        vec![step(0, Some(10_001), None)],
        vec![step(0, None, Some(999))],
    ] {
        TaskTimingObservations {
            steps,
            ..timing.clone()
        }
        .validate()
        .expect_err("the reader refuses the steps");
    }
}

#[test]
fn p6_execution_records_carry_wall_and_budget_time() {
    let stored = written(
        PolicyPayloadDraft::execution_recorded(
            execution(PolicyExecutionOutcome::Succeeded { runtime_ms: 60_000 }),
            AuditInput::new(),
        )
        .into(),
    );
    assert!(stored["payload"]["data"].get("wall_elapsed_ms").is_none());
    let timed = with(
        with(stored, "wall_elapsed_ms", json!(90_000)),
        "budget_elapsed_ms",
        json!(60_000),
    );
    let payload = read_back(&timed);
    let EventPayload::Policy(PolicyPayload::ExecutionRecorded(recorded)) = &payload else {
        panic!("an execution record");
    };
    assert_eq!(
        (recorded.wall_elapsed_ms(), recorded.budget_elapsed_ms()),
        (Some(90_000), Some(60_000))
    );
    // The budget clock never runs ahead of the wall clock (PERF §5.3).
    refused(with(timed, "budget_elapsed_ms", json!(90_001)));
}

#[test]
fn u1_command_records_name_their_channel_and_via_on_lab_only() {
    let action = EventAction::RuntimeAction;
    let received = written(CommandPayloadDraft::received(action, AuditInput::new()).into());
    assert!(received["payload"]["data"].get("channel").is_none());
    for channel in ["cli", "mcp", "ui", "lab"] {
        read_back(&with(received.clone(), "channel", json!(channel)));
    }
    for via in ["mcp", "ui"] {
        let stored = with(
            with(received.clone(), "channel", json!("lab")),
            "via",
            json!(via),
        );
        let payload = read_back(&stored);
        let EventPayload::Command(CommandPayload::Received(observed)) = &payload else {
            panic!("a command record");
        };
        assert_eq!(observed.channel(), Some(crate::RuntimeChannel::Lab));
        assert!(observed.via().is_some());
    }
    for written_record in [
        written(
            CommandPayloadDraft::validated(action, EffectDisposition::Performed, AuditInput::new())
                .into(),
        ),
        written(
            CommandPayloadDraft::rejected(
                action,
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ),
    ] {
        read_back(&with(
            with(written_record.clone(), "channel", json!("lab")),
            "via",
            json!("ui"),
        ));
        refused(with(
            with(written_record, "channel", json!("cli")),
            "via",
            json!("ui"),
        ));
    }
    // `via` only on the Lab channel, and only a command record names a channel.
    refused(with(received.clone(), "via", json!("mcp")));
    refused(with(
        with(received, "channel", json!("mcp")),
        "via",
        json!("ui"),
    ));
    let admitted = written(SchedulerPayloadDraft::admitted(action, AuditInput::new()).into());
    refused(with(admitted, "channel", json!("cli")));
}

#[test]
fn u3_pack_registry_keys_read_back_their_entries() {
    let ids = ids();
    let instance_id = *ids.mint_instance_id().expect("instance id").transport();
    for (entry, key) in [
        (
            PackRegistryFactKey::Procedure {
                package_id: "pack-a".to_owned(),
            },
            "host.pack_registry.pack-a",
        ),
        (
            PackRegistryFactKey::Procedure {
                package_id: "startup".to_owned(),
            },
            "host.pack_registry.startup",
        ),
        (
            PackRegistryFactKey::Startup { instance_id },
            "host.pack_registry.startup",
        ),
        (
            PackRegistryFactKey::Prerequisite {
                package_id: "pack-b".to_owned(),
            },
            "host.pack_registry.prerequisite.pack-b",
        ),
    ] {
        assert_eq!(entry.fact_key().expect("a key"), key);
        let scope = entry.scope();
        assert_eq!(
            scope,
            match &entry {
                PackRegistryFactKey::Startup { .. } => RuntimeFactScope::Instance { instance_id },
                _ => RuntimeFactScope::Runtime,
            }
        );
        assert_eq!(
            PackRegistryFactKey::from_fact(&scope, key).expect("the entry"),
            entry
        );
    }
    for entry in [
        PackRegistryFactKey::Procedure {
            package_id: "prerequisite.pack-b".to_owned(),
        },
        PackRegistryFactKey::Procedure {
            package_id: String::new(),
        },
        PackRegistryFactKey::Prerequisite {
            package_id: "pack b".to_owned(),
        },
    ] {
        entry.fact_key().expect_err("the key is refused");
    }
    let instance = RuntimeFactScope::Instance { instance_id };
    for (scope, key) in [
        (&instance, "host.pack_registry.pack-a"),
        (&RuntimeFactScope::Runtime, "host.config_overrides"),
        (
            &RuntimeFactScope::Runtime,
            "host.pack_registry.prerequisite.",
        ),
        (&RuntimeFactScope::Runtime, "host.pack_registry."),
    ] {
        PackRegistryFactKey::from_fact(scope, key).expect_err("no entry");
    }
}

#[test]
fn a1_reserved_wake_kinds_read_back() {
    let ids = ids();
    for (kind, spelling) in [
        (AgentWakeKind::PeriodicReview, "periodic_review"),
        (AgentWakeKind::LadderExhausted, "ladder_exhausted"),
        (AgentWakeKind::TaskPaused, "task_paused"),
    ] {
        assert_eq!(token(kind), json!(spelling));
        let requested = wake(&ids, kind);
        let stored = record(
            "agent",
            "wake_requested",
            json!({"action": "agent.wake", "wake": token(requested), "audit": {}}),
        );
        let payload = read_back(&stored);
        let EventPayload::Agent(AgentPayload::WakeRequested(read)) = &payload else {
            panic!("a wake record");
        };
        assert_eq!(read.wake().kind(), kind);
    }
}

#[test]
fn a2_a_response_record_may_name_its_report_artifact() {
    let ids = ids();
    let requested = wake(&ids, AgentWakeKind::DriftPredicted);
    let session = AgentSessionStatus::started(
        *ids.mint_agent_session_id().expect("session id").transport(),
        &requested,
        AT_MS + 1_000,
    )
    .expect("session");
    let response = AgentSessionResponse::new(
        session.session_id(),
        AgentResponseDisposition::RetryableFailure,
        received_code("transient_failure"),
        AT_MS + 2_000,
    )
    .expect("response");
    let retried = session
        .retry_or_escalate(response.observed_at_unix_ms())
        .expect("retry");
    let responded = written(
        AgentPayloadDraft::response_recorded(
            AgentSessionEventData::new(retried, Some(response)).expect("response event"),
            AuditInput::new(),
        )
        .into(),
    );
    assert!(
        responded["payload"]["data"]
            .get("report_artifact")
            .is_none()
    );
    let artifact = *ids.mint_artifact_id().expect("artifact id").transport();
    let payload = read_back(&with(responded, "report_artifact", token(artifact)));
    let EventPayload::Agent(AgentPayload::ResponseRecorded(recorded)) = &payload else {
        panic!("a response record");
    };
    assert_eq!(recorded.report_artifact(), Some(&artifact));

    let started = written(
        AgentPayloadDraft::session_started(
            AgentSessionEventData::new(session, None).expect("start event"),
            AuditInput::new(),
        )
        .into(),
    );
    refused(with(started, "report_artifact", token(artifact)));
}

#[test]
fn a4_only_a_dispatch_intent_carries_priority_locked() {
    let intent =
        written(PolicyPayloadDraft::dispatch_intent(dispatch_data(), AuditInput::new()).into());
    assert!(intent["payload"]["data"].get("priority_locked").is_none());
    let payload = read_back(&with(intent, "priority_locked", json!(true)));
    let EventPayload::Policy(PolicyPayload::DispatchIntent(dispatch)) = &payload else {
        panic!("a dispatch intent");
    };
    assert!(dispatch.priority_locked());

    let admitted = written(
        PolicyPayloadDraft::dispatch_admitted(dispatch_data(), admission(), AuditInput::new())
            .into(),
    );
    read_back(&admitted);
    refused(with(admitted, "priority_locked", json!(true)));
}

#[test]
fn h1_the_reserved_path_drag_decodes_validates_and_redacts() {
    let drag = json!({
        "kind": "single_touch_path_drag_v1",
        "box_x": 100,
        "box_y": 200,
        "box_width": 400,
        "box_height": 300,
        "points": [{"x": 120, "y": 220}, {"x": 480, "y": 260}, {"x": 150, "y": 480}],
        "segment_duration_ms": [200, 250],
        "corner_hold_ms": 80,
    });
    let action: InputAction = serde_json::from_value(drag.clone()).expect("the action decodes");
    action.validate().expect("the action validates");
    assert_eq!(token(&action), drag);
    assert_eq!(action.event_action(), EventAction::InputSwipe);
    assert_eq!(
        format!("{action:?}"),
        "InputAction::SingleTouchPathDragV1(<redacted-coordinates>)"
    );

    let edited = |edits: &[(&str, Value)]| {
        let mut value = drag.clone();
        for (member, replacement) in edits {
            value[*member] = replacement.clone();
        }
        serde_json::from_value::<InputAction>(value).expect("the action decodes")
    };
    let many = (0..=MAX_PATH_DRAG_POINTS)
        .map(|index| InputPathPoint {
            x: 101 + index as i32,
            y: 201,
        })
        .collect::<Vec<_>>();
    let cases: [(Vec<(&str, Value)>, RuntimeContractCode); 9] = [
        (
            vec![
                ("points", json!([{"x": 120, "y": 220}])),
                ("segment_duration_ms", json!([])),
            ],
            RuntimeContractCode::InvalidSegmentedSwipeContract,
        ),
        (
            vec![
                ("points", token(many)),
                ("segment_duration_ms", token([10_u64; MAX_PATH_DRAG_POINTS])),
            ],
            RuntimeContractCode::InvalidSegmentedSwipeContract,
        ),
        (
            vec![("segment_duration_ms", json!([200]))],
            RuntimeContractCode::InvalidSegmentedSwipeContract,
        ),
        (
            vec![("box_width", json!(0))],
            RuntimeContractCode::InvalidSegmentedSwipeContract,
        ),
        (
            vec![(
                "points",
                json!([{"x": 120, "y": 220}, {"x": 500, "y": 260}, {"x": 150, "y": 480}]),
            )],
            RuntimeContractCode::InvalidInputCoordinate,
        ),
        (
            vec![
                ("box_x", json!(0)),
                (
                    "points",
                    json!([{"x": -1, "y": 220}, {"x": 480, "y": 260}, {"x": 150, "y": 480}]),
                ),
            ],
            RuntimeContractCode::InvalidInputCoordinate,
        ),
        (
            vec![("segment_duration_ms", json!([0, 250]))],
            RuntimeContractCode::InvalidInputDuration,
        ),
        (
            vec![(
                "segment_duration_ms",
                json!([200, crate::MAX_INPUT_DURATION_MS + 1]),
            )],
            RuntimeContractCode::InvalidInputDuration,
        ),
        (
            vec![("corner_hold_ms", json!(crate::MAX_INPUT_DURATION_MS + 1))],
            RuntimeContractCode::InvalidInputDuration,
        ),
    ];
    for (edits, code) in cases {
        let error = edited(edits.as_slice())
            .validate()
            .expect_err("the action is refused");
        assert_eq!(error.code(), code.code(), "{edits:?}");
    }

    // A ledger-validated shape: an input intent's provenance holds it.
    let intent = record(
        "input",
        "intent",
        json!({
            "action": "input.swipe",
            "provenance": {"input_action": drag},
            "audit": {},
        }),
    );
    read_back(&intent);
    refused(with(intent, "action", json!("input.tap")));
}
