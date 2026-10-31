// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-7a2 (register §1.234, since 0.12.35; contracts/player-hold.md, "The one order of
// the control gates"; design R-7a §2.6 and §7.1 "R-7a2（§2.6）"): every host site that says why a
// dispatch does not happen takes its code from the one order of the control gates, the player
// hold (false until R-7b), the instance's pause, the global pause, the instance's #403 fence.
//
// - The word-by-word mapping: every word of `control_gate_code` is the registered runtime-host
//   code the host emits for it, and each of the four effective codes is a word, so a rename on
//   either side fails here.
// - Both pauses: the evaluation's reason and the admission's refusal are
//   `dispatch_paused_instance`; under the global pause alone both are `dispatch_paused_global`.
// - A fenced instance: its candidate is `Deferred` with `dispatch_resource_fenced` (no wake) and
//   no dispatch intent; an intent evaluated before the fence is refused with it; its due clock
//   slot is deferred at first sight, the class of the two pauses; the device request that met the
//   fence keeps `instance_resources_fenced`, and so does the fence's record.
// - Adding and lifting the global pause over an instance pause changes neither the explanation's
//   `why` nor its `since_unix_ms`.
// - The fact refresh's gate is the evaluation's reason for every combination of the gates.
//
// Every scenario runs one generic clock task on the fixture instance with the activity window of
// `gate_372k2` and a manual Runtime clock that moves at most one minute after a fence, well inside
// the fence's recheck interval; the instance's stuck recovery is off, so no ladder acts on it.

use super::gate_372k2::{SLOT_TASK, at, slot_rows, slot_sources, slot_task};
use super::gate_403::{fence_records, status_fence};
use super::*;
use crate::codes::HostCode;
use crate::codes::test_spellings::TestLocation;
use crate::control_gates::{effective_code, host_code};
use crate::fact_refresh::instance_hold;
use crate::host::failure_scope::Contained;
use crate::policy_host::InstanceOccupancy;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    ControlGateCode, ControlGates, ControlInputs, InstanceGate, InstanceStuckRecovery,
    RuntimeScheduleExplanation, ScheduleCandidateEligibility, ScheduleCandidateState,
    SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_policy::{
    CandidateEligibility, EligibilityState, PolicyEvaluation, SchedulingDecisionState, TaskDecision,
};
use actingcommand_scheduler::ClaimKind;
use serde_json::json;

const ALIAS: &str = POLICY_INSTANCE_ALIAS;

/// The four effective codes, in the order of `instance_gate`.
const EFFECTIVE: [HostCode; 4] = [
    HostCode::DispatchPlayerHeld,
    HostCode::DispatchPausedInstance,
    HostCode::DispatchPausedGlobal,
    HostCode::DispatchResourceFenced,
];

/// How the fixture instance is backed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// A device the host self-checks: an instance pause needs one, and a fence withholds its
    /// policy availability, so its candidates stop at placement before the control gates.
    Device,
    /// A fixture whose availability is the evaluation's input: a fence leaves it available, so
    /// its candidates reach the control gates.
    Fixture,
}

/// One host with the fixture instance (stuck recovery off) on a manual Runtime clock, its slot
/// task `daily [10:00]` active, and a direct-run package for the fence.
struct Gates {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    registered: InstanceId,
    package: String,
    approved: bool,
    _root: TempDir,
}

impl Gates {
    fn start(backend: Backend, unix_ms: u64) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let state = Arc::new(FakeState::default());
        let package = super::gate_369s5::direct_run_package(&root, &state);
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let mut facts = policy_facts();
        facts.outcomes.clear();
        let provider = FakeProvider::one(ALIAS, registered, Arc::clone(&state));
        let provider = match backend {
            Backend::Device => provider,
            Backend::Fixture => provider.fixture_simulation(),
        };
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_stuck_recovery(BTreeMap::from([(
                    ALIAS.to_owned(),
                    InstanceStuckRecovery {
                        enabled: false,
                        cooldown_secs: 600,
                    },
                )])),
            Arc::new(provider),
        )
        .expect("start the control-gate host");
        host.activate_policy_catalog(&slot_sources(1, &[slot_task(&[600], 1)], None))
            .expect("activate the slot catalog");
        Self {
            host,
            clock,
            state,
            registered,
            package,
            approved: false,
            _root: root,
        }
    }

    /// Moves the Runtime clock to `unix_ms`, samples the capacity there, and evaluates.
    fn evaluate(&self, unix_ms: u64) -> PolicyEvaluation {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        let mut facts = policy_facts();
        facts.outcomes.clear();
        let cycle = self
            .host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"));
        cycle.evaluation.expect("an evaluated cycle")
    }

    /// Evaluates at `unix_ms`, where the slot task must be selected: its intent and reasons.
    fn selected(&self, unix_ms: u64) -> (DispatchIntent, DecisionReasonChain) {
        let evaluation = self.evaluate(unix_ms);
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == SLOT_TASK && intent.instance_id == ALIAS)
            .unwrap_or_else(|| panic!("the slot task is selected: {evaluation:#?}"))
            .clone();
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the intent's reason chain")
            .clone();
        (intent, chain)
    }

    /// The admission of `intent`, which a control gate must refuse: the refusal, and the outcome
    /// of its `policy.dispatch_rejected` record, which is the refusal's.
    fn refused(
        &mut self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
    ) -> (RuntimeHostError, Outcome) {
        if !self.approved {
            record_policy_approval(&self.host, intent);
            self.approved = true;
        }
        let error = self
            .host
            .admit_policy_dispatch(intent, chain, &policy_context(&self.host, intent))
            .expect_err("a control gate refuses the admission");
        assert!(!error.is_fatal(), "{error:?}");
        let rejected = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::PolicyDispatchRejected),
                ..EventQuery::default()
            })
            .expect("rejected records")
            .into_iter()
            .find_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchRejected(payload))
                    if payload.decision_id() == intent.decision_id =>
                {
                    Some(payload.rejection().expect("the rejection").clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("a rejected record of {}", intent.decision_id));
        assert_eq!(rejected, error.policy_rejection());
        (error, rejected.outcome)
    }

    fn scheduling(&self, operation: RuntimeOperation) {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(operation);
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    fn pause(&self, scope: SchedulingPauseScope) {
        self.scheduling(RuntimeOperation::PauseScheduling {
            scope,
            pause_reason: SchedulingPauseReason::new("r7a2_gate_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        });
    }

    fn resume(&self, scope: SchedulingPauseScope) {
        self.scheduling(RuntimeOperation::ResumeScheduling {
            scope,
            expected: None,
        });
    }

    /// A direct run on the instance whose input leaves its child unconfirmed: the device request
    /// meets the fence it makes. Its receipt.
    fn fence_by_a_direct_run(&self) -> RuntimeReceipt {
        *self
            .state
            .input_error_once
            .lock()
            .expect("one-shot input error") = Some(
            DeviceError::fatal("synthetic input command left its child unconfirmed")
                .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1),
        );
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let run = runtime_request(
            &ids,
            RuntimeOperation::run_contained_task(
                ALIAS,
                ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(self.package.clone()).expect("task request"),
            ),
        );
        self.host
            .process_request_for_test(&run, ConnectionId::new(4291).expect("connection"))
            .expect("the direct run's receipt")
    }

    /// An instance failure of the fixture instance at a boundary (ISO-M, the entry every device
    /// request's failure takes): it fences the instance, and the work ends with the fence's
    /// failure, which it returns.
    fn fence_by_containment(&self) -> RuntimeHostError {
        let failure = RuntimeHostError::instance_fault(
            SharedCode::CaptureBackendOperationFailed,
            TestLocation::ContainFailureForTest,
            RuntimeErrorCode::CaptureFailed,
            self.registered,
        );
        match self
            .host
            .contain_failure_for_test(failure, None)
            .expect("contain the instance's failure")
        {
            Contained::Fenced(fence) => fence,
            other => panic!("the instance's failure fences it: {other:?}"),
        }
    }

    /// The explanation's slot-task row as (why, since) and its not-running row as (task, why,
    /// since).
    fn explanation(&self) -> ((String, u64), (Option<String>, String, u64)) {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::ScheduleExplanation {
            instance: Some(ALIAS.to_owned()),
            candidates: true,
        });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
            panic!("a schedule explanation: {:?}", receipt.result());
        };
        let explanation: &RuntimeScheduleExplanation = explanation;
        let [entry] = explanation.instances.as_slice() else {
            panic!("one instance entry: {explanation:#?}");
        };
        let row = entry
            .candidates
            .iter()
            .find(|row| row.task_id == SLOT_TASK)
            .unwrap_or_else(|| panic!("the slot task's row: {entry:#?}"));
        assert_eq!(row.state, ScheduleCandidateState::Deferred, "{row:?}");
        assert_eq!(
            row.eligibility,
            ScheduleCandidateEligibility::True,
            "{row:?}"
        );
        (
            (row.why.code().as_str().to_owned(), row.since_unix_ms),
            (
                entry.not_running.task_id.clone(),
                entry.not_running.why.code().as_str().to_owned(),
                entry.not_running.since_unix_ms,
            ),
        )
    }

    fn close(self) {
        self.host.close().expect("close the control-gate host");
    }
}

fn instance_scope() -> SchedulingPauseScope {
    SchedulingPauseScope::Instance {
        instance_alias: ALIAS.to_owned(),
    }
}

/// The slot task's decision on the instance.
fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == SLOT_TASK && decision.instance_id.as_deref() == Some(ALIAS)
        })
        .unwrap_or_else(|| panic!("the slot task's decision: {evaluation:#?}"))
}

/// The slot task is deferred by the control gate `code`, with no wake and no dispatch intent;
/// its last reason, which it returns.
fn deferred_by(evaluation: &PolicyEvaluation, code: HostCode) -> Outcome {
    let decision = decision(evaluation);
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    assert_eq!(
        decision.eligibility,
        EligibilityState::True,
        "{decision:#?}"
    );
    assert_eq!(decision.next_wake_unix_ms, None, "{decision:#?}");
    let reason = decision.reasons.last().expect("a reason");
    assert!(reason.outcome.is(code), "{decision:#?}");
    assert!(
        evaluation
            .dispatch_intents
            .iter()
            .all(|intent| intent.instance_id != ALIAS),
        "no dispatch intent on the instance: {evaluation:#?}"
    );
    reason.outcome.clone()
}

/// Every word of `control_gate_code` is a registered runtime-host code, the one the host emits
/// for it, and each of the four effective codes is a word: a rename on either side fails here.
#[test]
fn r7a2_every_control_gate_code_word_is_the_runtime_host_code_it_maps_to() {
    assert_eq!(ControlGateCode::ALL.len(), EFFECTIVE.len());
    for word in ControlGateCode::ALL.iter().copied() {
        let registered = HostCode::ALL
            .iter()
            .copied()
            .find(|code| code.as_str() == word.as_str())
            .unwrap_or_else(|| panic!("`{word}` is no runtime-host code"));
        assert_eq!(host_code(word), registered, "`{word}`");
        assert_eq!(
            Code::relayed(word.as_str()),
            Some(registered.code()),
            "the embedded catalog registers `{word}`"
        );
    }
    assert_eq!(InstanceGate::ALL.len(), EFFECTIVE.len());
    for (gate, code) in InstanceGate::ALL.iter().copied().zip(EFFECTIVE) {
        let word = ControlGateCode::ALL
            .iter()
            .copied()
            .find(|word| word.as_str() == code.as_str())
            .unwrap_or_else(|| panic!("`{}` is no word of control_gate_code", code.as_str()));
        assert_eq!(gate.code(), word, "{gate:?}");
        assert_eq!(host_code(word), code);
        let gates = ControlGates {
            dispatch: vec![gate],
            device: Vec::new(),
        };
        assert_eq!(effective_code(&gates), Some(code), "{gate:?}");
    }
    assert_eq!(effective_code(&ControlGates::default()), None);
}

/// With the instance pause and the global pause both in effect, the evaluation's reason and the
/// admission's refusal of an intent evaluated before them are `dispatch_paused_instance`; under
/// the global pause alone both are `dispatch_paused_global`.
#[test]
fn r7a2_both_pauses_defer_and_refuse_with_the_instance_pause_the_global_alone_with_its_own() {
    let mut gates = Gates::start(Backend::Device, at(0, 10, 0));
    let (intent, chain) = gates.selected(at(0, 10, 0));
    gates.pause(instance_scope());
    gates.pause(SchedulingPauseScope::Global);
    let (error, rejection) = gates.refused(&intent, &chain);
    assert_eq!(error.code(), HostCode::DispatchPausedInstance.as_str());
    assert!(
        rejection.is(HostCode::DispatchPausedInstance),
        "{rejection:?}"
    );
    deferred_by(
        &gates.evaluate(at(0, 10, 1)),
        HostCode::DispatchPausedInstance,
    );
    gates.close();

    let mut gates = Gates::start(Backend::Device, at(0, 10, 0));
    let (intent, chain) = gates.selected(at(0, 10, 0));
    gates.pause(SchedulingPauseScope::Global);
    let (error, rejection) = gates.refused(&intent, &chain);
    assert_eq!(error.code(), HostCode::DispatchPausedGlobal.as_str());
    assert!(
        rejection.is(HostCode::DispatchPausedGlobal),
        "{rejection:?}"
    );
    deferred_by(
        &gates.evaluate(at(0, 10, 1)),
        HostCode::DispatchPausedGlobal,
    );
    gates.close();
}

/// A device request that meets the fence it makes (a direct run whose input leaves its child
/// unconfirmed) still ends with `instance_resources_fenced`, and so does the fence's one record;
/// an intent evaluated before the fence is refused at admission with `dispatch_resource_fenced`,
/// naming the instance.
#[test]
fn r7a2_a_device_request_keeps_the_fence_code_and_an_earlier_intent_is_refused_resource_fenced() {
    let mut gates = Gates::start(Backend::Device, at(0, 10, 0));
    let (intent, chain) = gates.selected(at(0, 10, 0));

    let receipt = gates.fence_by_a_direct_run();
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str(),
        "the device request keeps the fence's code"
    );
    assert!(
        status_fence(&gates.host, ALIAS).is_some_and(|fence| fence.awaiting_person),
        "the instance is fenced"
    );
    let events = gates
        .host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    assert_eq!(fence_records(&events).len(), 1, "one fence record");

    let (error, rejection) = gates.refused(&intent, &chain);
    assert_eq!(error.code(), HostCode::DispatchResourceFenced.as_str());
    assert!(
        rejection.is(HostCode::DispatchResourceFenced),
        "{rejection:?}"
    );
    assert_eq!(
        rejection.value("instance_id"),
        Some(&serde_json::to_value(gates.registered).expect("instance id")),
        "{rejection:?}"
    );
    gates.close();
}

/// A fenced instance whose candidates reach the control gates (a fixture, whose availability the
/// fence does not withhold): the work that met the fence ends with `instance_resources_fenced`,
/// and so does the fence's record; the evaluation defers the candidate with
/// `dispatch_resource_fenced`, naming the instance, with no wake and no dispatch intent; its due
/// clock slot is deferred at first sight (`clock_slot_deferred`, the class of the two pauses).
#[test]
fn r7a2_a_fenced_instance_is_deferred_resource_fenced_and_its_slot_deferred_at_first_sight() {
    let gates = Gates::start(Backend::Fixture, at(0, 10, 0));
    let fence = gates.fence_by_containment();
    assert_eq!(fence.code(), HostCode::InstanceResourcesFenced.as_str());
    assert!(!fence.is_fatal(), "{fence:?}");
    assert!(
        status_fence(&gates.host, ALIAS).is_some(),
        "the instance is fenced"
    );
    let events = gates
        .host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    assert_eq!(fence_records(&events).len(), 1, "one fence record");

    let reason = deferred_by(
        &gates.evaluate(at(0, 10, 1)),
        HostCode::DispatchResourceFenced,
    );
    assert_eq!(
        reason.value("instance_id"),
        Some(&json!(ALIAS)),
        "{reason:?}"
    );
    let rows = slot_rows(&gates.host);
    let [row] = rows.as_slice() else {
        panic!("one clock-slot record: {rows:#?}");
    };
    assert_eq!(
        (row.kind.as_str(), row.code.as_str(), row.task_id.as_str()),
        ("deferred", "clock_slot_deferred", SLOT_TASK)
    );
    assert_eq!(
        row.text("gate_code"),
        Some(HostCode::DispatchResourceFenced.as_str())
    );
    assert_eq!(row.time("occurrence_unix_ms"), Some(at(0, 10, 0)));
    assert_eq!(row.time("next_eligible_unix_ms"), None);
    assert!(
        status_fence(&gates.host, ALIAS).is_some(),
        "the fence still holds"
    );
    gates.close();
}

/// Ruling 116's case: an instance pause holds the instance; adding the global pause over it and
/// lifting it again changes neither the candidate's `why` nor its `since_unix_ms`, nor the
/// not-running row's.
#[test]
fn r7a2_the_global_pause_over_an_instance_pause_changes_neither_why_nor_since() {
    let gates = Gates::start(Backend::Device, at(0, 10, 0));
    gates.pause(instance_scope());
    let paused = HostCode::DispatchPausedInstance.as_str().to_owned();
    let expected = (
        (paused.clone(), at(0, 10, 0)),
        (Some(SLOT_TASK.to_owned()), paused, at(0, 10, 0)),
    );

    deferred_by(
        &gates.evaluate(at(0, 10, 0)),
        HostCode::DispatchPausedInstance,
    );
    assert_eq!(gates.explanation(), expected, "under the instance pause");

    gates.pause(SchedulingPauseScope::Global);
    deferred_by(
        &gates.evaluate(at(0, 10, 1)),
        HostCode::DispatchPausedInstance,
    );
    assert_eq!(gates.explanation(), expected, "with the global pause added");

    gates.resume(SchedulingPauseScope::Global);
    deferred_by(
        &gates.evaluate(at(0, 10, 2)),
        HostCode::DispatchPausedInstance,
    );
    assert_eq!(
        gates.explanation(),
        expected,
        "with the global pause lifted"
    );
    gates.close();
}

/// For every combination of the four gates and every state of the instance's queue, the fact
/// refresh's gate is the evaluation's reason: the first control gate in the one order, else the
/// queue's; a control gate has no wake, and the fence names its instance.
#[test]
fn r7a2_the_fact_refresh_gate_is_the_evaluation_reason_for_every_combination() {
    const WAKE: u64 = 1_700_000_000_000;
    let occupancies = [
        None,
        Some(InstanceOccupancy::Held {
            holder_kind: ClaimKind::DirectTaskRun,
            count: 1,
        }),
        Some(InstanceOccupancy::Cooldown {
            until_unix_ms: WAKE,
        }),
    ];
    for bits in 0_u8..16 {
        let inputs = ControlInputs {
            player_hold: bits & 1 != 0,
            instance_pause: bits & 2 != 0,
            global_pause: bits & 4 != 0,
            resource_fence: bits & 8 != 0,
        };
        // The one order, written out: hold, instance pause, global pause, fence.
        let control = [
            inputs.player_hold,
            inputs.instance_pause,
            inputs.global_pause,
            inputs.resource_fence,
        ]
        .into_iter()
        .zip(EFFECTIVE)
        .find_map(|(active, code)| active.then_some(code));
        let gates = ControlGates::evaluate(inputs);
        assert_eq!(effective_code(&gates), control, "{inputs:?}");
        for occupancy in occupancies {
            let queue = match occupancy {
                Some(InstanceOccupancy::Held { .. }) => Some(HostCode::DispatchInstanceHeld),
                Some(InstanceOccupancy::Cooldown { .. }) => {
                    Some(HostCode::DispatchInstanceCooldown)
                }
                None => None,
            };
            let hold = instance_hold(ALIAS, &gates, occupancy);
            let Some(code) = control.or(queue) else {
                assert!(hold.is_none(), "{inputs:?} {occupancy:?}");
                continue;
            };
            let hold = hold.unwrap_or_else(|| panic!("a hold: {inputs:?} {occupancy:?}"));
            let CandidateEligibility::Deferred {
                reason,
                next_wake_unix_ms,
            } = hold.eligibility(false)
            else {
                panic!("a deferral: {inputs:?} {occupancy:?}");
            };
            let CandidateEligibility::Refresh {
                reason: refresh_reason,
                next_wake_unix_ms: refresh_wake,
                gate,
                limit,
            } = hold.eligibility(true)
            else {
                panic!("a fact refresh deferral: {inputs:?} {occupancy:?}");
            };
            assert!(
                reason.outcome.is(code),
                "{inputs:?} {occupancy:?}: {reason:?}"
            );
            assert_eq!(gate, code.code(), "{inputs:?} {occupancy:?}");
            assert_eq!(refresh_reason, reason, "{inputs:?} {occupancy:?}");
            assert_eq!(refresh_wake, next_wake_unix_ms, "{inputs:?} {occupancy:?}");
            assert!(limit.is_none(), "{inputs:?} {occupancy:?}");
            let policy_hold = hold.policy_hold();
            assert_eq!(policy_hold.reason, reason, "{inputs:?} {occupancy:?}");
            assert_eq!(policy_hold.next_wake_unix_ms, next_wake_unix_ms);
            if control.is_some() {
                assert_eq!(next_wake_unix_ms, None, "{inputs:?} {occupancy:?}");
            }
            if code == HostCode::DispatchResourceFenced {
                assert_eq!(
                    reason.outcome.value("instance_id"),
                    Some(&json!(ALIAS)),
                    "{reason:?}"
                );
            }
        }
    }
}
