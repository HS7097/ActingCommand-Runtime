// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-6b, MCP-19a (host keys 126 and 127; register §1.143 M19A-2, M19A-3;
// contracts/scheduling/README.md, "Why A Candidate Does Not Run"): the policy host keeps the
// latest evaluation's not-run explanation in memory, per instance: its earliest wake, its
// selected decision with the dispatch state the projection holds when the read is answered, every
// other decision as a candidate row (why = the outcome of its last reason, its reason codes, its
// wake, since when the same why holds) and one not-running row, `due_task_missing` when nothing
// is due. The general read ScheduleExplanation answers it from memory and appends nothing; an
// unknown alias is refused instance_unknown and a reply past one frame
// runtime_schedule_explanation_too_large_for_frame. Every scenario runs generic tasks on the
// fixture instance of `gate_372k2` (UTC clocks, a manual Runtime clock). A task that must not be
// due before its time takes a one-time clock (`at`): a daily clock with no dispatch history is
// due at once with the previous day's occurrence.

use super::gate_372k2::{SLOT_TASK, SlotTask, Slots, at, slot_sources, slot_task};
use super::gate_383_ao4q::ledger_head;
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::ScheduleExplanationRowCode;
use actingcommand_contract::{
    RuntimeScheduleExplanation, ScheduleCandidateEligibility, ScheduleCandidateState,
    ScheduleDispatchState, ScheduleExplanationInstance,
};
use actingcommand_policy::PolicyCode;
use serde_json::Value;

const SECOND_TASK: &str = "fixture.second";
const LATER_TASK: &str = "fixture.later";
/// A frame bound that holds a request, a refusal and an answer without candidate rows, but not
/// the candidate rows of [`FRAME_TASKS`].
const FRAME_BYTES: usize = 2_048;
const FRAME_TASKS: [&str; 24] = [
    "fixture.frame-a",
    "fixture.frame-b",
    "fixture.frame-c",
    "fixture.frame-d",
    "fixture.frame-e",
    "fixture.frame-f",
    "fixture.frame-g",
    "fixture.frame-h",
    "fixture.frame-i",
    "fixture.frame-j",
    "fixture.frame-k",
    "fixture.frame-l",
    "fixture.frame-m",
    "fixture.frame-n",
    "fixture.frame-o",
    "fixture.frame-p",
    "fixture.frame-q",
    "fixture.frame-r",
    "fixture.frame-s",
    "fixture.frame-t",
    "fixture.frame-u",
    "fixture.frame-v",
    "fixture.frame-w",
    "fixture.frame-x",
];

/// A task whose UTC clock occurs once, at `at_ms`: not due before it, due from it on.
fn once_at(id: &'static str, priority: u32, at_ms: u64) -> SlotTask {
    SlotTask {
        id,
        priority,
        trigger: serde_json::json!({
            "kind": "clock",
            "schedule": {
                "kind": "at",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "etc/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "at_ms": at_ms
            }
        }),
        daily_limit: 1,
    }
}

/// The answer of a ScheduleExplanation read, which must complete.
fn read(
    host: &RuntimeHost,
    instance: Option<&str>,
    candidates: bool,
) -> RuntimeScheduleExplanation {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ScheduleExplanation {
        instance: instance.map(str::to_owned),
        candidates,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
        panic!(
            "expected a schedule explanation, got {:?}",
            receipt.result()
        );
    };
    (**explanation).clone()
}

/// The fixture instance's one entry.
fn entry(explanation: &RuntimeScheduleExplanation) -> &ScheduleExplanationInstance {
    let [entry] = explanation.instances.as_slice() else {
        panic!("one instance entry: {explanation:#?}");
    };
    assert_eq!(entry.instance_alias, POLICY_INSTANCE_ALIAS);
    entry
}

/// Each candidate row as (task, state, eligibility, why code, since), in answer order; every
/// row's reason codes end with its why.
fn rows(
    entry: &ScheduleExplanationInstance,
) -> Vec<(
    &str,
    ScheduleCandidateState,
    ScheduleCandidateEligibility,
    &str,
    u64,
)> {
    entry
        .candidates
        .iter()
        .map(|row| {
            assert_eq!(row.reasons.last(), Some(row.why.code()), "{row:?}");
            (
                row.task_id.as_str(),
                row.state,
                row.eligibility,
                row.why.code().as_str(),
                row.since_unix_ms,
            )
        })
        .collect()
}

/// The not-running row as (task, why code, since).
fn not_running(entry: &ScheduleExplanationInstance) -> (Option<&str>, &str, u64) {
    (
        entry.not_running.task_id.as_deref(),
        entry.not_running.why.code().as_str(),
        entry.not_running.since_unix_ms,
    )
}

fn empty() -> RuntimeScheduleExplanation {
    RuntimeScheduleExplanation {
        evaluated_at_unix_ms: None,
        catalog_hash: None,
        instances: Vec::new(),
    }
}

// Requirements 3 and 5: the selected task with its decision id, and every other decision as a
// row whose why is its last reason, due rows first; the not-running row is the first due row.
#[test]
fn mcp19a_a_candidate_that_did_not_run_shows_its_why_reasons_and_wake() {
    let tasks = [
        SlotTask {
            priority: 200,
            ..slot_task(&[600], 1)
        },
        SlotTask {
            id: SECOND_TASK,
            priority: 100,
            ..slot_task(&[600], 1)
        },
        once_at(LATER_TASK, 100, at(0, 15, 0)),
    ];
    let mut slots = Slots::start(&slot_sources(1, &tasks, None), at(0, 10, 0));
    let cycle = slots.evaluate(at(0, 10, 1));
    let intent = cycle
        .evaluation
        .as_ref()
        .expect("evaluation")
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == SLOT_TASK)
        .expect("the higher-ranked task is selected")
        .clone();

    let explanation = read(&slots.host, Some(POLICY_INSTANCE_ALIAS), true);
    assert_eq!(explanation.evaluated_at_unix_ms, Some(at(0, 10, 1)));
    assert_eq!(
        explanation.catalog_hash.as_deref(),
        Some(intent.catalog_hash.as_str())
    );
    let entry = entry(&explanation);
    let selected = entry.selected.as_ref().expect("a selected task");
    assert_eq!(
        (selected.task_id.as_str(), selected.decision_id.as_str()),
        (SLOT_TASK, intent.decision_id.as_str())
    );
    assert_eq!(
        rows(entry),
        vec![
            (
                SECOND_TASK,
                ScheduleCandidateState::Deferred,
                ScheduleCandidateEligibility::True,
                PolicyCode::InstanceAlreadySelected.as_str(),
                at(0, 10, 1),
            ),
            (
                LATER_TASK,
                ScheduleCandidateState::Blocked,
                ScheduleCandidateEligibility::False,
                PolicyCode::TriggerFalse.as_str(),
                at(0, 10, 1),
            ),
        ]
    );
    assert_eq!(entry.candidates[1].next_wake_unix_ms, Some(at(0, 15, 0)));
    assert_eq!(
        not_running(entry),
        (
            Some(SECOND_TASK),
            PolicyCode::InstanceAlreadySelected.as_str(),
            at(0, 10, 1)
        )
    );
    assert_eq!(entry.candidates_total, 2);
    slots.close();
}

// Requirements 3 and 5: with no due row the not-running row is due_task_missing naming the
// instance and its earliest wake; the rest are ordered by their wake; without candidates the
// same entry has no rows and still counts them.
#[test]
fn mcp19a_with_nothing_due_the_row_is_due_task_missing_with_the_earliest_wake() {
    let tasks = [
        once_at(SLOT_TASK, 100, at(0, 10, 0)),
        once_at(LATER_TASK, 100, at(0, 15, 0)),
    ];
    let mut slots = Slots::start(&slot_sources(1, &tasks, None), at(0, 8, 0));
    slots.evaluate(at(0, 8, 0));

    let explanation = read(&slots.host, Some(POLICY_INSTANCE_ALIAS), true);
    let entry = entry(&explanation);
    assert_eq!(entry.next_wake_unix_ms, Some(at(0, 10, 0)));
    assert_eq!(entry.selected, None);
    assert_eq!(entry.not_running.task_id, None);
    assert!(
        entry
            .not_running
            .why
            .is(ScheduleExplanationRowCode::DueTaskMissing),
        "{:?}",
        entry.not_running
    );
    assert_eq!(
        entry
            .not_running
            .why
            .value("instance_id")
            .and_then(Value::as_str),
        Some(POLICY_INSTANCE_ALIAS)
    );
    assert_eq!(
        entry
            .not_running
            .why
            .value("next_eligible_unix_ms")
            .and_then(Value::as_u64),
        Some(at(0, 10, 0))
    );
    assert_eq!(
        entry
            .candidates
            .iter()
            .map(|row| (row.task_id.as_str(), row.next_wake_unix_ms))
            .collect::<Vec<_>>(),
        vec![
            (SLOT_TASK, Some(at(0, 10, 0))),
            (LATER_TASK, Some(at(0, 15, 0)))
        ]
    );

    let without = read(&slots.host, Some(POLICY_INSTANCE_ALIAS), false);
    let without = self::entry(&without);
    assert!(without.candidates.is_empty(), "{without:?}");
    assert_eq!(without.candidates_total, 2);
    assert_eq!(
        *without,
        ScheduleExplanationInstance {
            candidates: Vec::new(),
            ..entry.clone()
        }
    );
    slots.close();
}

// Requirement 4: since is the first evaluation of an unbroken run with the same why, for a
// candidate row and for the not-running row; a changed why starts a new run.
#[test]
fn mcp19a_since_holds_while_the_why_holds_and_restarts_when_it_changes() {
    let mut slots = Slots::start(
        &slot_sources(1, &[once_at(SLOT_TASK, 100, at(0, 10, 0))], None),
        at(0, 9, 0),
    );
    for evaluated_at in [at(0, 9, 0), at(0, 9, 30)] {
        slots.evaluate(evaluated_at);
        let explanation = read(&slots.host, Some(POLICY_INSTANCE_ALIAS), true);
        let entry = entry(&explanation);
        assert_eq!(
            rows(entry),
            vec![(
                SLOT_TASK,
                ScheduleCandidateState::Blocked,
                ScheduleCandidateEligibility::False,
                PolicyCode::TriggerFalse.as_str(),
                at(0, 9, 0),
            )],
            "at {evaluated_at}"
        );
        assert_eq!(
            not_running(entry),
            (
                None,
                ScheduleExplanationRowCode::DueTaskMissing.as_str(),
                at(0, 9, 0)
            ),
            "at {evaluated_at}"
        );
    }

    let holder = slots.hold();
    slots.evaluate(at(0, 10, 1));
    let explanation = read(&slots.host, Some(POLICY_INSTANCE_ALIAS), true);
    let entry = entry(&explanation);
    assert_eq!(
        rows(entry),
        vec![(
            SLOT_TASK,
            ScheduleCandidateState::Deferred,
            ScheduleCandidateEligibility::True,
            HostCode::DispatchInstanceHeld.as_str(),
            at(0, 10, 1),
        )]
    );
    assert_eq!(
        not_running(entry),
        (
            Some(SLOT_TASK),
            HostCode::DispatchInstanceHeld.as_str(),
            at(0, 10, 1)
        )
    );
    Slots::release(holder);
    slots.close();
}

// Requirements 2 and 5: before any evaluation and after a start the answer is empty; the read
// appends nothing and names no request in the ledger; an alias the registry does not hold is
// refused instance_unknown.
#[test]
fn mcp19a_the_read_is_memory_only_and_refuses_an_unknown_alias() {
    let mut slots = Slots::start(
        &slot_sources(1, &[slot_task(&[600], 1)], None),
        at(0, 10, 0),
    );
    assert_eq!(read(&slots.host, None, true), empty());
    slots.evaluate(at(0, 10, 1));

    let mut client = TestClient::connect(&slots.host);
    let mut reader = TestClient::connect(&slots.host);
    let head = ledger_head(&mut reader);
    let request = client.request(RuntimeOperation::ScheduleExplanation {
        instance: Some(POLICY_INSTANCE_ALIAS.to_owned()),
        candidates: true,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
        panic!(
            "expected a schedule explanation, got {:?}",
            receipt.result()
        );
    };
    assert_eq!(explanation.evaluated_at_unix_ms, Some(at(0, 10, 1)));
    assert_eq!(entry(explanation).instance_alias, POLICY_INSTANCE_ALIAS);
    assert_eq!(ledger_head(&mut reader), head, "the read appended a record");
    assert!(
        projected_events(
            &mut reader,
            EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            },
        )
        .is_empty(),
        "the read left a record of its request"
    );

    let unknown = client.send(&client.request(RuntimeOperation::ScheduleExplanation {
        instance: Some("neutral.missing".to_owned()),
        candidates: true,
    }));
    assert_eq!(unknown.state(), RuntimeReceiptState::Denied, "{unknown:?}");
    assert!(
        unknown.outcome().is(RuntimeErrorCode::InstanceUnknown),
        "{unknown:?}"
    );
    drop(reader);
    drop(client);

    let slots = slots.restart(at(0, 10, 2));
    assert_eq!(read(&slots.host, None, true), empty());
    slots.close();
}

// Requirement 3: the selected decision's dispatch state is read when the request is answered:
// pending before its intent is recorded, completed once its run completed.
#[test]
fn mcp19a_the_selected_dispatch_state_follows_the_projection() {
    let mut slots = Slots::start(
        &slot_sources(1, &[slot_task(&[600], 1)], None),
        at(0, 10, 0),
    );
    let cycle = slots.evaluate(at(0, 10, 1));
    let decision_id = cycle
        .evaluation
        .as_ref()
        .expect("evaluation")
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == SLOT_TASK)
        .expect("the task is selected")
        .decision_id
        .clone();
    let explanation = read(&slots.host, Some(POLICY_INSTANCE_ALIAS), true);
    let selected = entry(&explanation)
        .selected
        .clone()
        .expect("a selected task");
    assert_eq!(
        (
            selected.task_id.as_str(),
            selected.decision_id.as_str(),
            selected.dispatch_state
        ),
        (
            SLOT_TASK,
            decision_id.as_str(),
            ScheduleDispatchState::Pending
        )
    );

    slots.dispatch(at(0, 10, 1), SLOT_TASK);
    let explanation = read(&slots.host, Some(POLICY_INSTANCE_ALIAS), true);
    let selected = entry(&explanation)
        .selected
        .clone()
        .expect("a selected task");
    assert_eq!(
        (selected.task_id.as_str(), selected.dispatch_state),
        (SLOT_TASK, ScheduleDispatchState::Completed)
    );
    slots.close();
}

/// A host on the manual clock `clock` whose IPC frames carry at most [`FRAME_BYTES`], with the
/// fixture instance (the `gate_372k2` host with the frame bound).
fn framed_host(root: &TempDir, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    RuntimeHost::start(
        config(root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_maximum_frame_bytes(FRAME_BYTES),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("start the framed host")
}

// Requirement 5: a reply larger than one frame is refused while the request fits; the same read
// without candidates completes, and the Runtime goes on.
#[test]
fn mcp19a_a_reply_past_one_frame_is_refused() {
    let root = TempDir::new().expect("tempdir");
    let evaluated_at = at(0, 8, 0);
    let clock = Arc::new(ManualRuntimeClock::new(evaluated_at, 0));
    let host = framed_host(&root, &clock);
    let tasks = FRAME_TASKS
        .iter()
        .map(|&id| once_at(id, 100, at(0, 15, 0)))
        .collect::<Vec<_>>();
    host.activate_policy_catalog(&slot_sources(1, &tasks, None))
        .expect("activate the catalog");
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    sample_capacity().expect("capacity sample");
    let mut facts = policy_facts();
    facts.outcomes.clear();
    host.evaluate_policy_cycle_with_test_inputs(
        &facts,
        &policy_resources(),
        EvaluationTime {
            unix_ms: evaluated_at,
            monotonic_ms: evaluated_at,
        },
        7,
        PolicyTrigger::Recovery,
    )
    .expect("evaluate");

    let mut client = TestClient::connect(&host);
    let refused = client.send(&client.request(RuntimeOperation::ScheduleExplanation {
        instance: None,
        candidates: true,
    }));
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert!(
        refused
            .outcome()
            .is(HostCode::RuntimeScheduleExplanationTooLargeForFrame),
        "{refused:?}"
    );
    let without = client.send(&client.request(RuntimeOperation::ScheduleExplanation {
        instance: None,
        candidates: false,
    }));
    assert_eq!(
        without.state(),
        RuntimeReceiptState::Completed,
        "{without:?}"
    );
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = without.result() else {
        panic!(
            "expected a schedule explanation, got {:?}",
            without.result()
        );
    };
    let entry = entry(explanation);
    assert!(entry.candidates.is_empty(), "{entry:?}");
    assert_eq!(entry.candidates_total, 24);
    drop(client);
    host.close().expect("close the framed host");
}
