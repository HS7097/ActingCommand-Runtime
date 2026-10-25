// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #422 LN-3, test row 1 (LEAN-L model §3.6, §5.1; `contracts/ledger-observer.md` §11.1,
// §11.3; register L7, L8, L14): each change of the decision memory (a dispatch pair's control
// state, an instance's activity state) and each wake obligation writes exactly one value record
// with the key's complete state, and the index's `state` row of the key names that record. An
// admission and its run's settlement each change the pair and its activity profile once; the
// replayed admission and the repeated settlement change nothing and write nothing. The monitor
// registry's row is tested with the registry (`monitor.rs`).

use super::*;
use actingcommand_contract::{
    EventId, PolicyActivityStateRecord, PolicyControlStateRecord, PolicyDailyCount,
    PolicyDispatchClockRecord, PolicyJournalDispatchRecord, PolicyRetryRoundRecord,
};
use actingcommand_runtime_database::{DatabaseLocation, RuntimeDatabase};

const DAY_MS: u64 = 86_400_000;

fn id_text(id: &EventId) -> String {
    match serde_json::to_value(id).expect("an event id") {
        serde_json::Value::String(text) => text,
        other => panic!("an event id is a text, not {other}"),
    }
}

// The sequence and the event id the `state` row of `key` names, read from the closed root.
fn state_row(root: &TempDir, key: &str) -> Option<(u64, String)> {
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("database connection");
    let mut statement = connection
        .prepare("SELECT sequence, event_id FROM ledger_index WHERE role='state' AND key=?1")
        .expect("index read");
    let mut rows = statement
        .query_map([key], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .expect("index rows");
    // The ledger's ordered integer encoding.
    rows.next()
        .map(|row| row.expect("an index row"))
        .map(|(sequence, event_id)| ((sequence as u64) ^ (1_u64 << 63), event_id))
}

fn control_records(host: &RuntimeHost) -> Vec<(PersistedEvent, PolicyControlStateRecord)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyControlStateRecorded),
        ..EventQuery::default()
    })
    .expect("the control state records")
    .into_iter()
    .map(|event| {
        let EventPayload::Policy(PolicyPayload::ControlStateRecorded(payload)) = event.payload()
        else {
            panic!("a control state record")
        };
        let state = payload.state().clone();
        (event, state)
    })
    .collect()
}

// Workflow #422 LN-4: the sequence of the latest record of `event_type`.
fn last_sequence(host: &RuntimeHost, event_type: EventType) -> u64 {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        ..EventQuery::default()
    })
    .expect("the records")
    .pop()
    .expect("a record")
    .sequence()
}

fn activity_records(host: &RuntimeHost) -> Vec<(PersistedEvent, PolicyActivityStateRecord)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyActivityStateRecorded),
        ..EventQuery::default()
    })
    .expect("the activity state records")
    .into_iter()
    .map(|event| {
        let EventPayload::Policy(PolicyPayload::ActivityStateRecorded(payload)) = event.payload()
        else {
            panic!("an activity state record")
        };
        let state = payload.state().clone();
        (event, state)
    })
    .collect()
}

#[test]
fn ln3_a_each_decision_memory_change_writes_one_value_record_its_state_row_names() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate the catalog");
    assert!(control_records(&host).is_empty() && activity_records(&host).is_empty());

    // The admission: the pair's day and window counts, its runtime reservation, its retry round
    // and its dispatch clock; the activity profile's counts, reservation and next eligible time.
    let (_, intent, reasons) =
        evaluated_policy_dispatch_at(&host, PolicyTrigger::Recovery, POLICY_NOW_UNIX_MS, 100);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the admission")
    else {
        panic!("expected the admission")
    };
    let admission = context.admission().clone();
    let activity = &admission.activity;
    let window_end = u64::try_from(activity.local_day + 1).expect("a day") * DAY_MS;
    // Workflow #422 LN-4 (register §1.184 LN4-7): the record names the open dispatch by the
    // sequences of its intent and its admission.
    let journal_dispatch = PolicyJournalDispatchRecord {
        intent_sequence: last_sequence(&host, EventType::PolicyDispatchIntent),
        admitted_sequence: Some(last_sequence(&host, EventType::PolicyDispatchAdmitted)),
        completed_sequence: None,
        failures: None,
    };
    let admitted = PolicyControlStateRecord {
        instance_id: intent.instance_id.clone(),
        task_id: intent.task_id.clone(),
        daily_counts: vec![PolicyDailyCount {
            local_day: activity.local_day,
            count: 1,
        }],
        window_counts: 1,
        window_runtime_ms: intent.expected_duration_ms,
        active_window: Some(activity.window_id.clone()),
        failure_streak: None,
        // Workflow #422 LN-4 (register §1.184 LN4-2): the first round this host opened.
        retry_round: Some(PolicyRetryRoundRecord {
            admission: admission.clone(),
            until_unix_ms: window_end,
            expected_duration_ms: intent.expected_duration_ms,
            opened: Some(1),
        }),
        dispatch_clock: Some(PolicyDispatchClockRecord {
            admitted_at_unix_ms: activity.admitted_at_unix_ms,
        }),
        condition_reruns: None,
        failure_settled: None,
        dispatches: vec![journal_dispatch],
    };
    let admitted_activity = PolicyActivityStateRecord {
        instance_id: intent.instance_id.clone(),
        profile_id: activity.profile_id.clone(),
        local_day: activity.local_day,
        daily_count: 1,
        window_id: activity.window_id.clone(),
        window_count: 1,
        window_runtime_ms: intent.expected_duration_ms,
        next_eligible_unix_ms: Some(activity.next_eligible_unix_ms),
    };
    let controls = control_records(&host);
    let activities = activity_records(&host);
    assert_eq!(controls.len(), 1, "one record of the pair's change");
    assert_eq!(activities.len(), 1, "one record of the activity's change");
    assert_eq!(controls[0].1, admitted);
    assert_eq!(activities[0].1, admitted_activity);

    // The exact replay of the admission changes nothing.
    let replay = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the replayed admission");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    assert_eq!(control_records(&host).len(), 1);
    assert_eq!(activity_records(&host).len(), 1);

    // The settlement: the run's actual runtime replaces the reservation, the success ends the
    // retry round, and the completion stops the dispatch clock.
    clock.advance(75_000);
    let settled = host
        .record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("the settlement");
    let PolicyExecutionOutcome::Succeeded { runtime_ms } = settled.outcome else {
        panic!("expected a success")
    };
    // Workflow #422 LN-4 (register §1.184 LN4-2): the success is the first settlement that set
    // or cleared the pair's failure state.
    let settled_pair = PolicyControlStateRecord {
        window_runtime_ms: runtime_ms,
        retry_round: None,
        dispatch_clock: None,
        failure_settled: Some(1),
        // Workflow #422 LN-4: the completed dispatch, with no failure through it.
        dispatches: vec![PolicyJournalDispatchRecord {
            completed_sequence: Some(last_sequence(&host, EventType::PolicyDispatchCompleted)),
            failures: Some(0),
            ..journal_dispatch
        }],
        ..admitted
    };
    let settled_activity = PolicyActivityStateRecord {
        window_runtime_ms: runtime_ms,
        ..admitted_activity
    };
    let controls = control_records(&host);
    let activities = activity_records(&host);
    assert_eq!(controls.len(), 2, "one record of the pair's change");
    assert_eq!(activities.len(), 2, "one record of the activity's change");
    assert_eq!(controls[1].1, settled_pair);
    assert_eq!(activities[1].1, settled_activity);
    let completion = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchCompleted),
            ..EventQuery::default()
        })
        .expect("the completion")
        .pop()
        .expect("the run's completion");
    assert!(controls[1].0.sequence() > completion.sequence());

    // The repeated settlement changes nothing.
    host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("the repeated settlement");
    assert_eq!(control_records(&host).len(), 2);
    assert_eq!(activity_records(&host).len(), 2);
    host.close().expect("close the host");

    // The `state` rows name the latest record of each key.
    for (key, record) in [
        (
            format!("decision_memory/{}/{}", intent.instance_id, intent.task_id),
            &controls[1].0,
        ),
        (
            format!(
                "activity_memory/{}/{}",
                intent.instance_id, activity.profile_id
            ),
            &activities[1].0,
        ),
    ] {
        assert_eq!(
            state_row(&root, &key),
            Some((record.sequence(), id_text(record.event_id()))),
            "{key}"
        );
    }
}

#[test]
fn ln3_a_each_wake_obligation_writes_one_value_record_its_state_row_names() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    // A timeline signal wakes the agent; a goal-missed signal does not.
    for (index, kind) in [
        PolicyPlanningSignalKind::TimelineReached,
        PolicyPlanningSignalKind::GoalMissed,
    ]
    .into_iter()
    .enumerate()
    {
        host.record_policy_planning_signal(PolicyPlanningSignalEventData {
            signal_id: format!("signal:ln3-wake-{index}"),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            task_id: None,
            kind,
            outcome: planning_signal_outcome(kind),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
            detection_budget: None,
        })
        .expect("the planning signal");
    }
    let wakes = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        })
        .expect("the wakes")
        .into_iter()
        .map(|event| {
            let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = event.payload() else {
                panic!("a wake")
            };
            payload.wake().clone()
        })
        .collect::<Vec<_>>();
    let obligations = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::AgentWakeObligationRecorded),
            ..EventQuery::default()
        })
        .expect("the wake obligations");
    assert_eq!(wakes.len(), 1, "the timeline signal wakes");
    assert_eq!(obligations.len(), 1, "one record of the obligation");
    let EventPayload::Agent(AgentPayload::WakeObligationRecorded(obligation)) =
        obligations[0].payload()
    else {
        panic!("a wake obligation")
    };
    assert_eq!(
        obligation.wake(),
        &wakes[0],
        "the obligation's complete state"
    );
    host.close().expect("close the host");

    let wake_id = match serde_json::to_value(wakes[0].wake_id()).expect("a wake id") {
        serde_json::Value::String(text) => text,
        other => panic!("a wake id is a text, not {other}"),
    };
    assert_eq!(
        state_row(&root, &format!("wake/{wake_id}")),
        Some((
            obligations[0].sequence(),
            id_text(obligations[0].event_id())
        ))
    );
}
