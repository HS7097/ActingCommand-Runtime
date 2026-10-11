// SPDX-License-Identifier: AGPL-3.0-only

//! P-1 gate tests of the scheduler (test plan v2.2 row P-1, `rd-1009/tests/test-ci-plan.md:162`;
//! `inventory-policy.md` gaps G1-G6 and retirement R3). Every expected value comes from the
//! cited requirement, not from what the code does today.

use super::scenario::{ClientQueue, LeaseEnd, Scenario};
use super::{claim_config, connection, epoch, held_instance, holder, ids, instance, request};
use crate::facts::{RuntimeFactChange, RuntimeFactError, RuntimeFactStore};
use crate::{
    ClaimGate, ClaimKind, LeaseReleaseReason, QueuedLease, ReleasedLease, SchedulerError,
    SeedScheduler,
};
use actingcommand_contract::{
    FactValue, InstanceId, LeasePriority, LeaseToken, MAX_RUNTIME_FACTS, OriginModule, RequestId,
    RuntimeFactInvalidation, RuntimeFactInvalidationReason, RuntimeFactRecord, RuntimeFactScope,
};
use std::collections::BTreeSet;

/// A client wait well inside `claim_config`'s 500 ms queue timeout bound.
const QUEUE_TIMEOUT_MS: u64 = 400;
/// The TTL a ladder renews to before each rung (model-369-queue.md H-3 step 1).
const RUNG_TTL_MS: u64 = 1_000;
/// A fixed observation time for the runtime fact records.
const OBSERVED_AT_UNIX_MS: u64 = 1_760_000_000_000;
/// The ledger position a snapshot is bound to.
const SNAPSHOT_LEDGER_POSITION: u64 = 42;

fn holder_kind(scenario: &Scenario, instance_id: InstanceId) -> Option<ClaimKind> {
    scenario
        .scheduler()
        .active_lease(instance_id)
        .map(|lease| lease.kind())
}

fn holder_token(scenario: &Scenario, instance_id: InstanceId) -> Option<LeaseToken> {
    scenario
        .scheduler()
        .active_lease(instance_id)
        .map(|lease| lease.token().clone())
}

fn queued(scenario: &Scenario, instance_id: InstanceId) -> Vec<RequestId> {
    scenario
        .scheduler()
        .queued_on(instance_id)
        .expect("queue status")
        .iter()
        .map(QueuedLease::request_id)
        .collect()
}

/// G1: while A has a holder or an eligible entry, a policy dispatch is refused
/// (model-369-queue.md Q-3 (6), W-1).
fn assert_policy_dispatch_refused(scenario: &mut Scenario, instance_id: InstanceId, step: &str) {
    let probe = scenario.probe(instance_id, ClaimGate::Open);
    assert!(
        matches!(
            probe,
            Err(SchedulerError::Busy { .. } | SchedulerError::QueueAhead { .. })
        ),
        "{step}: a policy dispatch must be refused while A is held or waited for, got {probe:?}"
    );
}

/// P-1 G1 (test-ci-plan.md:162; inventory-policy.md:81; req-basis-cp.md:92). The #369 sequence of
/// 10-07 (model-369-queue.md §1, H-1, H-3, §6) on one instance with no takeover cooldown: a
/// failed run's lease end hands the key to its ladder, which keeps it in one continuous hold
/// through every rung; an operator entry that waits meanwhile stays queued and gets the key only
/// after the ladder. The harness checks its invariants after every step.
#[test]
fn gate_p1_g1_a_failed_runs_key_stays_with_its_ladder_through_every_rung() {
    let mut scenario = Scenario::new();
    let a = scenario.new_instance();

    // Step 1: A is free and its queue is empty.
    assert_eq!(holder_kind(&scenario, a), None);
    assert!(queued(&scenario, a).is_empty());
    scenario
        .probe(a, ClaimGate::Open)
        .expect("step 1: a free instance with nothing waiting admits a policy dispatch");

    // Step 2: a policy dispatch, run R, is granted.
    let run = scenario.claim(a, ClaimKind::PolicyDispatch, LeasePriority::Normal);
    let run_hold = scenario
        .try_claim(run, ClaimGate::Open)
        .expect("step 2: the policy dispatch is granted");
    assert_eq!(holder_kind(&scenario, a), Some(ClaimKind::PolicyDispatch));
    assert_policy_dispatch_refused(&mut scenario, a, "step 2");

    // Step 3: while R holds A, an operator claim is queued on A.
    scenario.tick(10);
    let operator = scenario.claim(a, ClaimKind::DirectTaskRun, LeasePriority::Normal);
    scenario
        .enqueue(operator)
        .expect("step 3: the operator claim is queued");
    assert_eq!(queued(&scenario, a), [operator.request_id]);
    assert_policy_dispatch_refused(&mut scenario, a, "step 3");

    // Step 4: R fails; its lease end is the hand-off to a ladder claim (H-1).
    scenario.tick(10);
    let ladder = scenario.claim(a, ClaimKind::Ladder, LeasePriority::High);
    let mut ladder_hold = scenario
        .hand_off(&run_hold, ladder, ClaimGate::Open)
        .expect("step 4: the failed run's lease end hands the key to its ladder");
    assert_eq!(ladder_hold.token.holder_id(), ladder.holder_id);
    assert_eq!(holder_kind(&scenario, a), Some(ClaimKind::Ladder));
    assert_eq!(
        queued(&scenario, a),
        [operator.request_id],
        "step 4: the hand-off bypasses the queue and keeps it"
    );
    assert_policy_dispatch_refused(&mut scenario, a, "step 4");

    // Step 5: three rungs. Before each rung the ladder renews and queues its continuation; the
    // rung run's lease end hands the key on through the pump (H-3).
    for rung in 1..=3 {
        let step = format!("step 5, rung {rung}");
        scenario.tick(10);
        scenario
            .renew_with_ttl(&mut ladder_hold, RUNG_TTL_MS)
            .unwrap_or_else(|error| panic!("{step}: the ladder's renewal is refused: {error:?}"));
        assert_eq!(holder_kind(&scenario, a), Some(ClaimKind::Ladder), "{step}");
        assert_policy_dispatch_refused(&mut scenario, a, &step);

        let continuation = scenario.continuation(&ladder_hold);
        scenario.enqueue(continuation).unwrap_or_else(|error| {
            panic!("{step}: the ladder's continuation is refused: {error:?}")
        });
        assert_eq!(
            queued(&scenario, a),
            [continuation.request_id, operator.request_id],
            "{step}: the continuation is queued ahead of the operator entry"
        );
        assert_eq!(holder_kind(&scenario, a), Some(ClaimKind::Ladder), "{step}");
        assert_policy_dispatch_refused(&mut scenario, a, &step);

        scenario.tick(10);
        ladder_hold = scenario
            .end_lease(&ladder_hold, LeaseEnd::BackendFailure, ClaimGate::Open)
            .unwrap_or_else(|| panic!("{step}: the rung run's lease end hands the key on"));
        assert_eq!(
            ladder_hold.request_id, continuation.request_id,
            "{step}: the continuation, not the operator entry, gets the key"
        );
        assert_eq!(ladder_hold.token.holder_id(), ladder.holder_id, "{step}");
        assert_eq!(holder_kind(&scenario, a), Some(ClaimKind::Ladder), "{step}");
        assert_eq!(queued(&scenario, a), [operator.request_id], "{step}");
        assert_policy_dispatch_refused(&mut scenario, a, &step);
    }

    // Step 6: the ladder finishes; its lease end grants the operator entry, and the operator's
    // own lease end leaves A free.
    scenario.tick(10);
    let operator_hold = scenario
        .end_lease(&ladder_hold, LeaseEnd::Explicit, ClaimGate::Open)
        .expect("step 6: the ladder's lease end grants the waiting operator entry");
    assert_eq!(operator_hold.request_id, operator.request_id);
    assert_eq!(holder_kind(&scenario, a), Some(ClaimKind::DirectTaskRun));
    assert!(queued(&scenario, a).is_empty());
    assert_policy_dispatch_refused(&mut scenario, a, "step 6");

    scenario.tick(10);
    assert!(
        scenario
            .end_lease(&operator_hold, LeaseEnd::Explicit, ClaimGate::Open)
            .is_none(),
        "step 6: nothing waits after the operator entry"
    );
    assert_eq!(holder_kind(&scenario, a), None);
    assert!(queued(&scenario, a).is_empty());
    scenario
        .probe(a, ClaimGate::Open)
        .expect("after step 6 A has no holder and no entry: a policy dispatch is admitted");
}

/// P-1 R3 (test-ci-plan.md:273; inventory-policy.md:110; c3a:65, :69, :160). Replaces
/// `zero_stagger_same_instance_has_one_grant_and_one_busy_denial`: two acquires on one instance,
/// in either order, give one grant and one busy refusal.
#[test]
fn gate_p1_r3_two_acquires_on_one_instance_grant_one_and_refuse_one_in_either_order() {
    for order in [[0, 1], [1, 0]] {
        let mut scenario = Scenario::new();
        let a = scenario.new_instance();
        let clients = [scenario.client(), scenario.client()];
        let granted = scenario
            .acquire(clients[order[0]], a)
            .expect("the first acquire is granted");
        assert_eq!(holder_token(&scenario, a), Some(granted.token));
        let refused = scenario.acquire(clients[order[1]], a);
        assert!(
            matches!(refused, Err(SchedulerError::Busy { .. })),
            "order {order:?}: the second acquire is refused busy, got {refused:?}"
        );
    }
}

/// P-1 R3 (test-ci-plan.md:273; inventory-policy.md:110; c3a:65, :69). Replaces
/// `different_instances_are_independent`: acquires on two instances are both granted, in either
/// order.
#[test]
fn gate_p1_r3_acquires_on_two_instances_are_both_granted_in_either_order() {
    for order in [[0, 1], [1, 0]] {
        let mut scenario = Scenario::new();
        let instances = [scenario.new_instance(), scenario.new_instance()];
        let clients = [scenario.client(), scenario.client()];
        let mut granted = Vec::new();
        for index in order {
            granted.push(
                scenario
                    .acquire(clients[index], instances[index])
                    .expect("an acquire on its own instance is granted")
                    .token,
            );
        }
        let active = scenario.scheduler().active_tokens();
        assert_eq!(active.len(), 2, "order {order:?}");
        assert!(
            granted.iter().all(|token| active.contains(token)),
            "order {order:?}"
        );
    }
}

/// P-1 R3 (test-ci-plan.md:273; inventory-policy.md:110; c3a:65). Replaces
/// `zero_stagger_queued_admission_has_one_grant_and_one_queue`: two queued requests on one free
/// instance, in either order, give one committed lease and one queued entry.
#[test]
fn gate_p1_r3_two_queued_requests_on_one_free_instance_grant_one_and_queue_one_in_either_order() {
    for order in [[0, 1], [1, 0]] {
        let mut scenario = Scenario::new();
        let a = scenario.new_instance();
        let clients = [scenario.client(), scenario.client()];
        let ClientQueue::Granted(granted) = scenario.queue_lease(
            clients[order[0]],
            a,
            LeasePriority::Normal,
            QUEUE_TIMEOUT_MS,
        ) else {
            panic!("order {order:?}: the first queued request on a free instance is a lease");
        };
        assert_eq!(holder_token(&scenario, a), Some(granted.token));
        assert!(
            matches!(
                scenario.queue_lease(
                    clients[order[1]],
                    a,
                    LeasePriority::Normal,
                    QUEUE_TIMEOUT_MS,
                ),
                ClientQueue::Waiting
            ),
            "order {order:?}: the second queued request waits"
        );
        assert_eq!(queued(&scenario, a), [clients[order[1]].request_id]);
    }
}

/// The states of G2 (inventory-policy.md:82).
#[derive(Debug, Clone, Copy)]
enum PolicyReadState {
    Free,
    HeldByAPolicyRun,
    LadderWithItsContinuationQueued,
    RoutineEntryGateOpen,
    RoutineEntryGateHeld,
    OperatorEntryGateHeld,
    ExpiredHeadBeforeAnEligibleEntry,
    TakeoverCooldown,
}

/// Builds a G2 state on instance A and names the gate the read and the admission use.
fn policy_read_state(state: PolicyReadState) -> (Scenario, InstanceId, ClaimGate) {
    if let PolicyReadState::TakeoverCooldown = state {
        let (mut scenario, a) = Scenario::after_takeover();
        scenario.tick(50);
        return (scenario, a, ClaimGate::Open);
    }
    let mut scenario = Scenario::new();
    let a = scenario.new_instance();
    if let PolicyReadState::Free = state {
        return (scenario, a, ClaimGate::Open);
    }
    let run = scenario.claim(a, ClaimKind::PolicyDispatch, LeasePriority::Normal);
    let run_hold = scenario
        .try_claim(run, ClaimGate::Open)
        .expect("a policy run holds A");
    let gate = match state {
        PolicyReadState::Free | PolicyReadState::TakeoverCooldown => {
            unreachable!("built above")
        }
        PolicyReadState::HeldByAPolicyRun => ClaimGate::Open,
        PolicyReadState::LadderWithItsContinuationQueued => {
            let ladder = scenario.claim(a, ClaimKind::Ladder, LeasePriority::High);
            let ladder_hold = scenario
                .hand_off(&run_hold, ladder, ClaimGate::Open)
                .expect("the run's lease end hands A to its ladder");
            let continuation = scenario.continuation(&ladder_hold);
            scenario
                .enqueue(continuation)
                .expect("the ladder queues its continuation");
            ClaimGate::Open
        }
        PolicyReadState::RoutineEntryGateOpen | PolicyReadState::RoutineEntryGateHeld => {
            let routine = scenario.claim(a, ClaimKind::Autostart, LeasePriority::Normal);
            scenario.enqueue(routine).expect("a routine entry waits");
            scenario.release_before_pump(&run_hold);
            if let PolicyReadState::RoutineEntryGateOpen = state {
                ClaimGate::Open
            } else {
                ClaimGate::RoutineHeld
            }
        }
        PolicyReadState::OperatorEntryGateHeld => {
            let operator = scenario.claim(a, ClaimKind::EmulatorControl, LeasePriority::Normal);
            scenario.enqueue(operator).expect("an operator entry waits");
            scenario.release_before_pump(&run_hold);
            ClaimGate::RoutineHeld
        }
        PolicyReadState::ExpiredHeadBeforeAnEligibleEntry => {
            let client = scenario.client();
            assert!(matches!(
                scenario.queue_lease(client, a, LeasePriority::High, 10),
                ClientQueue::Waiting
            ));
            let startup = scenario.claim(a, ClaimKind::StartupPackage, LeasePriority::Normal);
            scenario.enqueue(startup).expect("a startup entry waits");
            scenario.release_before_pump(&run_hold);
            scenario.tick(50);
            ClaimGate::Open
        }
    };
    (scenario, a, gate)
}

/// P-1 G2 (test-ci-plan.md:162; inventory-policy.md:82; req-basis-cp.md:93). The policy read the
/// host makes (runtime-host `policy_dispatch.rs:72-104`: a holder whose expiry is after now,
/// `first_eligible_kind`, `cooldown_remaining_ms`) agrees with the policy admission, a probe
/// `PolicyDispatch` try under the same gate at the same instant (model-369-queue.md Q-3 (5),
/// (6); W-1 `:346-351`; accept-369-s4.md:4-5).
#[test]
fn gate_p1_g2_the_policy_read_agrees_with_the_policy_admission() {
    // Q-3 (6): policy dispatches only with no holder, no eligible entry and no takeover cooldown.
    let states = [
        (PolicyReadState::Free, true),
        (PolicyReadState::HeldByAPolicyRun, false),
        (PolicyReadState::LadderWithItsContinuationQueued, false),
        (PolicyReadState::RoutineEntryGateOpen, false),
        (PolicyReadState::RoutineEntryGateHeld, true),
        (PolicyReadState::OperatorEntryGateHeld, false),
        (PolicyReadState::ExpiredHeadBeforeAnEligibleEntry, false),
        (PolicyReadState::TakeoverCooldown, false),
    ];
    for (state, dispatchable) in states {
        let (mut scenario, a, gate) = policy_read_state(state);
        let now = scenario.monotonic_ms();
        let scheduler = scenario.scheduler();
        let held_kind = scheduler
            .active_lease(a)
            .filter(|lease| lease.token().expires_at_monotonic_ms() > now)
            .map(|lease| lease.kind());
        let waiting = scheduler.first_eligible_kind(a, gate, now);
        let cooldown = scheduler.cooldown_remaining_ms(a, now);
        let read_dispatchable = held_kind.is_none() && waiting.is_none() && cooldown.is_none();
        let admission = scenario.probe(a, gate);

        assert_eq!(read_dispatchable, dispatchable, "{state:?}: Q-3 (6)");
        assert_eq!(
            admission.is_ok(),
            read_dispatchable,
            "{state:?}: the read says dispatchable exactly when the admission grants, got {admission:?}"
        );
        if held_kind.is_none() {
            if let Some(kind) = waiting {
                assert!(
                    matches!(
                        admission,
                        Err(SchedulerError::QueueAhead { kind: refused, .. }) if refused == kind
                    ),
                    "{state:?}: the refusal names the waiting kind {kind:?}, got {admission:?}"
                );
            } else if let Some(remaining_ms) = cooldown {
                assert_eq!(
                    admission,
                    Err(SchedulerError::Cooldown {
                        retry_after_ms: remaining_ms
                    }),
                    "{state:?}: a cooldown defers with a wake at its end"
                );
            }
        }
    }
}

/// P-1 G3 (test-ci-plan.md:162; inventory-policy.md:83; req-basis-cp.md:94-95). The fatality of
/// every `SchedulerError` variant on both surfaces, `is_fatal()` and `projection().fatal`. The
/// match has no wildcard arm, so a new variant does not compile until it is classified.
/// The `InvalidConfig` row is c3a:43 for v0.12.0; B2 (v0.12.1, model-378.md:499) adapts it.
#[test]
fn gate_p1_g3_scheduler_error_fatality_table() {
    let issuer = ids();
    let holder_id = holder(&issuer).1;
    let lease_id = *issuer.mint_lease_id().expect("lease").transport();
    let samples = [
        SchedulerError::InvalidConfig,
        SchedulerError::InvalidConnection,
        SchedulerError::IdentifierIssuance,
        SchedulerError::ExpiryOverflow,
        SchedulerError::Busy {
            holder_id,
            lease_id,
            expires_at_monotonic_ms: 1_001,
        },
        SchedulerError::Cooldown {
            retry_after_ms: 100,
        },
        SchedulerError::StaleOwnerEpoch,
        SchedulerError::LeaseMismatch,
        SchedulerError::InstanceMismatch,
        SchedulerError::HolderMismatch,
        SchedulerError::ConnectionMismatch,
        SchedulerError::LeaseExpired,
        SchedulerError::LeaseNotExpired,
        SchedulerError::LeaseMissing,
        SchedulerError::QueueFull,
        SchedulerError::QueueExpired,
        SchedulerError::QueueMissing,
        SchedulerError::QueueConnectionMismatch,
        SchedulerError::QueueRequestMismatch,
        SchedulerError::QueueTimeoutInvalid,
        SchedulerError::QueueSequenceOverflow,
        SchedulerError::StepSequenceOverflow,
        SchedulerError::TransferNotSafe,
        SchedulerError::DestructiveStateMismatch,
        SchedulerError::ResourceCloseOnly,
        SchedulerError::QueueAhead {
            kind: ClaimKind::Autostart,
            position: 1,
        },
    ];
    for error in samples {
        let fatal = match &error {
            // c3a:43: a configuration that breaks the heartbeat relation is fatal.
            SchedulerError::InvalidConfig => Some(true),
            // Alice 10-07: no key means the claim announces a wait, not a failure; c3a:19, the
            // busy rejection; #376, a refusal never becomes fatal.
            SchedulerError::Busy { .. }
            | SchedulerError::QueueAhead { .. }
            | SchedulerError::Cooldown { .. }
            | SchedulerError::QueueFull
            | SchedulerError::QueueExpired
            | SchedulerError::QueueMissing
            | SchedulerError::QueueConnectionMismatch
            | SchedulerError::QueueRequestMismatch
            | SchedulerError::QueueTimeoutInvalid
            | SchedulerError::LeaseMismatch
            | SchedulerError::LeaseExpired
            | SchedulerError::LeaseNotExpired
            | SchedulerError::LeaseMissing => Some(false),
            // D2 (answered "assert"): each refuses one request made under or for a lease; fatal
            // only when the Runtime as a whole cannot continue (model-378.md:103-105).
            SchedulerError::StaleOwnerEpoch
            | SchedulerError::InstanceMismatch
            | SchedulerError::HolderMismatch
            | SchedulerError::ConnectionMismatch
            | SchedulerError::TransferNotSafe
            | SchedulerError::DestructiveStateMismatch
            | SchedulerError::ResourceCloseOnly => Some(false),
            // Not written for now (coordinator-rulings-c-items.md:20): identifier and overflow.
            SchedulerError::IdentifierIssuance
            | SchedulerError::InvalidConnection
            | SchedulerError::ExpiryOverflow
            | SchedulerError::QueueSequenceOverflow
            | SchedulerError::StepSequenceOverflow => None,
        };
        if let Some(fatal) = fatal {
            assert_eq!(error.is_fatal(), fatal, "{error:?}: is_fatal()");
            assert_eq!(
                error.projection().fatal,
                fatal,
                "{error:?}: projection().fatal"
            );
        }
    }
}

/// P-1 G4 (test-ci-plan.md:162; inventory-policy.md:84; req-basis-cp.md:96). An install drain
/// cancels every entry of the instance except the holder-owned continuation and leaves the holder
/// and the other instances as they are; the holder's next lease end grants only the continuation
/// (model-369-queue.md §5.2 `:529`; Q-3 (0)).
#[test]
fn gate_p1_g4_install_drain_keeps_only_the_continuation() {
    let mut scenario = Scenario::new();
    let a = scenario.new_instance();
    let b = scenario.new_instance();

    let run = scenario.claim(a, ClaimKind::PolicyDispatch, LeasePriority::Normal);
    let run_hold = scenario
        .try_claim(run, ClaimGate::Open)
        .expect("a policy run holds A");
    let ladder = scenario.claim(a, ClaimKind::Ladder, LeasePriority::High);
    let ladder_hold = scenario
        .hand_off(&run_hold, ladder, ClaimGate::Open)
        .expect("a ladder holds A");
    let routine = scenario.claim(a, ClaimKind::Autostart, LeasePriority::Normal);
    let operator = scenario.claim(a, ClaimKind::EmulatorControl, LeasePriority::Normal);
    let startup = scenario.claim(a, ClaimKind::StartupPackage, LeasePriority::Normal);
    for entry in [routine, operator, startup] {
        scenario.enqueue(entry).expect("an entry waits on A");
    }
    let client = scenario.client();
    assert!(matches!(
        scenario.queue_lease(client, a, LeasePriority::Normal, QUEUE_TIMEOUT_MS),
        ClientQueue::Waiting
    ));
    let continuation = scenario.continuation(&ladder_hold);
    scenario
        .enqueue(continuation)
        .expect("the ladder queues its continuation");

    let b_run = scenario.claim(b, ClaimKind::PolicyDispatch, LeasePriority::Normal);
    scenario
        .try_claim(b_run, ClaimGate::Open)
        .expect("a policy run holds B");
    let b_operator = scenario.claim(b, ClaimKind::DirectTaskRun, LeasePriority::Normal);
    scenario.enqueue(b_operator).expect("an entry waits on B");
    let b_client = scenario.client();
    assert!(matches!(
        scenario.queue_lease(b_client, b, LeasePriority::Normal, QUEUE_TIMEOUT_MS),
        ClientQueue::Waiting
    ));
    let a_holder = holder_token(&scenario, a);
    let b_holder = holder_token(&scenario, b);
    let b_queue = queued(&scenario, b);

    let drained = scenario.drain(a);
    assert_eq!(
        drained.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            routine.request_id,
            operator.request_id,
            startup.request_id,
            client.request_id,
        ]),
        "the drain cancels exactly A's entries that are not holder-owned"
    );
    assert_eq!(queued(&scenario, a), [continuation.request_id]);
    assert_eq!(
        holder_token(&scenario, a),
        a_holder,
        "A's holder is unchanged"
    );
    assert_eq!(
        holder_token(&scenario, b),
        b_holder,
        "B's holder is unchanged"
    );
    assert_eq!(queued(&scenario, b), b_queue, "B's queue is unchanged");

    let next = scenario
        .end_lease(&ladder_hold, LeaseEnd::BackendFailure, ClaimGate::Open)
        .expect("the holder's next lease end grants the continuation");
    assert_eq!(next.request_id, continuation.request_id);
    assert!(queued(&scenario, a).is_empty());
}

/// P-1 G5 (a) (test-ci-plan.md:162, :216; inventory-policy.md:85; req-basis-cp.md:98). An owned
/// release with each owned reason returns that token and that reason and frees the instance
/// (c3a:84-86, :96-97; read-session-resource-close.md:112-119; scheduling-pause.md:104-106).
#[test]
fn gate_p1_g5_owned_releases_free_the_instance_for_each_owned_reason() {
    let issuer = ids();
    let mut scheduler =
        SeedScheduler::new(epoch(&issuer), claim_config(), [], 0).expect("scheduler");
    for reason in [
        LeaseReleaseReason::HostShutdown,
        LeaseReleaseReason::Disconnect,
        LeaseReleaseReason::BackendFailure,
        LeaseReleaseReason::InstancePaused,
        LeaseReleaseReason::ConnectionPrepared,
    ] {
        let (instance_id, token) = held_instance(&issuer, &mut scheduler);
        let released = scheduler
            .release_owned(&token, connection(1), reason)
            .unwrap_or_else(|error| panic!("{reason:?}: the owned release is refused: {error:?}"));
        assert_eq!(
            released,
            ReleasedLease {
                token: token.clone(),
                reason,
            }
        );
        assert!(scheduler.active_lease(instance_id).is_none(), "{reason:?}");
        assert!(!scheduler.active_tokens().contains(&token), "{reason:?}");
    }
}

/// P-1 G5 (b) (test-ci-plan.md:162, :216; inventory-policy.md:85; req-basis-cp.md:98). A lease
/// granted at `t` with TTL `T` has the deadline `d = t + T`; one millisecond before it the lease
/// renews by the fixed TTL under the same lease id, and it is not expired (c3a:65; key 5).
#[test]
fn gate_p1_g5_a_lease_is_not_expired_one_millisecond_before_its_deadline() {
    let issuer = ids();
    let mut scheduler =
        SeedScheduler::new(epoch(&issuer), claim_config(), [], 0).expect("scheduler");
    let lease_ttl_ms = scheduler.config().lease_ttl_ms;
    let granted_at = 10;
    let deadline = granted_at + lease_ttl_ms;
    let renewing = scheduler
        .acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            connection(1),
            granted_at,
        )
        .expect("the renewing lease is granted");
    let expiring_instance = instance(&issuer);
    let expiring = scheduler
        .acquire(
            request(&issuer),
            expiring_instance,
            holder(&issuer).1,
            connection(2),
            granted_at,
        )
        .expect("the other lease is granted");
    assert_eq!(renewing.expires_at_monotonic_ms(), deadline);
    assert_eq!(expiring.expires_at_monotonic_ms(), deadline);

    let renewed = scheduler
        .renew(request(&issuer), &renewing, connection(1), deadline - 1)
        .expect("a renewal one millisecond before the deadline succeeds");
    assert_eq!(renewed.lease_id(), renewing.lease_id());
    assert_eq!(
        renewed.expires_at_monotonic_ms(),
        deadline - 1 + lease_ttl_ms
    );

    assert_eq!(
        scheduler.expire_token(&expiring, deadline - 1),
        Err(SchedulerError::LeaseNotExpired)
    );
    assert_eq!(
        scheduler
            .active_lease(expiring_instance)
            .map(|lease| lease.token().clone()),
        Some(expiring),
        "the lease stays active one millisecond before its deadline"
    );
}

/// P-1 G5 (c) (test-ci-plan.md:162; inventory-policy.md:85; req-basis-cp.md:98). A rollback frees
/// the instance and forgets the token; the instance can be granted again (README.md:98, the
/// two-phase prepare and commit).
#[test]
fn gate_p1_g5_rollback_frees_the_instance_and_forgets_the_token() {
    let issuer = ids();
    let mut scheduler =
        SeedScheduler::new(epoch(&issuer), claim_config(), [], 0).expect("scheduler");
    let (instance_id, token) = held_instance(&issuer, &mut scheduler);
    scheduler
        .rollback_lease(&token)
        .expect("the committed lease rolls back");
    assert!(scheduler.active_lease(instance_id).is_none());
    assert!(!scheduler.active_tokens().contains(&token));
    assert!(
        scheduler.connection_for_token(&token).is_err(),
        "a rolled-back token is refused"
    );
    scheduler
        .acquire(
            request(&issuer),
            instance_id,
            holder(&issuer).1,
            connection(2),
            2,
        )
        .expect("a new try on the instance is granted");
}

/// G6's record builder (P-1 G6, settled old D4): a contract record with a neutral value.
fn runtime_fact(
    scope: RuntimeFactScope,
    key: &str,
    value: FactValue,
    observed_at_unix_ms: u64,
    ttl_ms: Option<u64>,
) -> RuntimeFactRecord {
    RuntimeFactRecord {
        scope,
        key: key.to_owned(),
        value,
        observed_at_unix_ms,
        source: OriginModule::Runtime,
        ttl_ms,
    }
}

/// P-1 G6 (test-ci-plan.md:162; inventory-policy.md:86; req-basis-cp.md:100). The store's record
/// table, its capacity bound and single invalidation follow the contract
/// (runtime-fact-store.md:63-80).
#[test]
fn gate_p1_g6_runtime_fact_store_follows_the_contract_record_rules() {
    let issuer = ids();
    let scope = RuntimeFactScope::Instance {
        instance_id: instance(&issuer),
    };
    let key = "device.fixture-session";
    let at = OBSERVED_AT_UNIX_MS;
    let mut store = RuntimeFactStore::new();

    let first = runtime_fact(
        scope.clone(),
        key,
        FactValue::Boolean(true),
        at,
        Some(60_000),
    );
    assert_eq!(store.record(first.clone()), Ok(RuntimeFactChange::Inserted));
    assert_eq!(store.record(first), Ok(RuntimeFactChange::Unchanged));
    let newer = runtime_fact(
        scope.clone(),
        key,
        FactValue::Boolean(false),
        at + 1,
        Some(60_000),
    );
    assert_eq!(store.record(newer.clone()), Ok(RuntimeFactChange::Updated));
    assert_eq!(store.get(&scope, key), Some(&newer));
    for (case, observed_at_unix_ms) in [("the same millisecond", at + 1), ("an older one", at)] {
        let late = runtime_fact(
            scope.clone(),
            key,
            FactValue::Boolean(true),
            observed_at_unix_ms,
            Some(60_000),
        );
        assert_eq!(
            store.record(late),
            Err(RuntimeFactError::Stale {
                existing_observed_at_unix_ms: at + 1
            }),
            "{case}"
        );
        assert_eq!(store.get(&scope, key), Some(&newer), "{case}");
    }

    let mut full = RuntimeFactStore::new();
    let held = (0..MAX_RUNTIME_FACTS)
        .map(|index| {
            runtime_fact(
                RuntimeFactScope::Runtime,
                &format!("host.fixture-{index}"),
                FactValue::Boolean(true),
                at,
                None,
            )
        })
        .collect::<Vec<_>>();
    for record in &held {
        assert_eq!(full.record(record.clone()), Ok(RuntimeFactChange::Inserted));
    }
    assert_eq!(
        full.record(runtime_fact(
            RuntimeFactScope::Runtime,
            "host.fixture-one-more",
            FactValue::Boolean(true),
            at,
            None,
        )),
        Err(RuntimeFactError::CapacityExceeded {
            limit: MAX_RUNTIME_FACTS
        })
    );
    assert_eq!(full.len(), MAX_RUNTIME_FACTS);
    assert!(
        held.iter()
            .all(|record| full.get(&record.scope, &record.key) == Some(record)),
        "capacity overflow evicts no record"
    );

    assert_eq!(
        store.invalidate(
            &scope,
            "device.fixture-absent",
            RuntimeFactInvalidationReason::Operator,
            at + 2,
        ),
        Err(RuntimeFactError::Missing)
    );
    assert_eq!(
        store.invalidate(&scope, key, RuntimeFactInvalidationReason::Operator, at + 2),
        Ok(RuntimeFactInvalidation {
            scope: scope.clone(),
            key: key.to_owned(),
            reason: RuntimeFactInvalidationReason::Operator,
            at_unix_ms: at + 2,
        })
    );
    assert_eq!(store.get(&scope, key), None);
}

/// P-1 G6 (test-ci-plan.md:162; inventory-policy.md:86; req-basis-cp.md:100). Invalidating one
/// instance under some families drops exactly that instance's records of those families, one
/// audit entry each (runtime-fact-store.md:75-77; the contract names no order).
#[test]
fn gate_p1_g6_invalidate_instance_drops_only_the_named_families_of_that_instance() {
    let issuer = ids();
    let a = instance(&issuer);
    let scope_a = RuntimeFactScope::Instance { instance_id: a };
    let scope_b = RuntimeFactScope::Instance {
        instance_id: instance(&issuer),
    };
    let at = OBSERVED_AT_UNIX_MS;
    let ready = || FactValue::Boolean(true);
    let records = [
        runtime_fact(scope_a.clone(), "device.fixture-session", ready(), at, None),
        runtime_fact(scope_a.clone(), "device.fixture-input", ready(), at, None),
        runtime_fact(
            scope_a.clone(),
            "application.fixture-foreground",
            FactValue::String("fixture.app".to_owned()),
            at,
            Some(60_000),
        ),
        runtime_fact(scope_a.clone(), "backend.fixture-check", ready(), at, None),
        runtime_fact(scope_a.clone(), "task.fixture-settled", ready(), at, None),
        runtime_fact(scope_b.clone(), "device.fixture-session", ready(), at, None),
        runtime_fact(
            scope_b,
            "application.fixture-foreground",
            FactValue::String("fixture.app".to_owned()),
            at,
            Some(60_000),
        ),
        runtime_fact(
            RuntimeFactScope::Runtime,
            "device.fixture-bridge",
            ready(),
            at,
            None,
        ),
    ];
    let mut store = RuntimeFactStore::new();
    for record in &records {
        store.record(record.clone()).expect("record");
    }
    let families = ["device.", "application."];

    let dropped = store.invalidate_instance(
        a,
        &families,
        RuntimeFactInvalidationReason::DeviceClosed,
        at + 5,
    );
    assert!(dropped.iter().all(|entry| {
        entry.scope == scope_a
            && entry.reason == RuntimeFactInvalidationReason::DeviceClosed
            && entry.at_unix_ms == at + 5
    }));
    let expected = [
        "device.fixture-session",
        "device.fixture-input",
        "application.fixture-foreground",
    ]
    .map(str::to_owned);
    assert_eq!(
        dropped.len(),
        expected.len(),
        "one audit entry per dropped record"
    );
    assert_eq!(
        dropped
            .iter()
            .map(|entry| entry.key.clone())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(expected)
    );
    for record in &records {
        let named =
            record.scope == scope_a && families.iter().any(|family| record.key.starts_with(family));
        assert_eq!(
            store.get(&record.scope, &record.key),
            (!named).then_some(record),
            "{:?} {}",
            record.scope,
            record.key
        );
    }
}

/// P-1 G6 (test-ci-plan.md:162; inventory-policy.md:86; req-basis-cp.md:100). A fresh store that
/// replays a snapshot and then applies the records and invalidations appended after it, in
/// order, holds exactly the live store (runtime-fact-store.md:78-80, :156-160).
#[test]
fn gate_p1_g6_snapshot_plus_tail_rebuilds_the_live_store() {
    enum Appended {
        Recorded(RuntimeFactRecord),
        Invalidated(RuntimeFactInvalidation),
    }
    let issuer = ids();
    let a = instance(&issuer);
    let scope_a = RuntimeFactScope::Instance { instance_id: a };
    let scope_b = RuntimeFactScope::Instance {
        instance_id: instance(&issuer),
    };
    let at = OBSERVED_AT_UNIX_MS;
    let mut live = RuntimeFactStore::new();
    for record in [
        runtime_fact(
            scope_a.clone(),
            "device.fixture-session",
            FactValue::Boolean(true),
            at,
            None,
        ),
        runtime_fact(
            scope_a.clone(),
            "backend.fixture-check",
            FactValue::Boolean(true),
            at,
            None,
        ),
        runtime_fact(
            RuntimeFactScope::Runtime,
            "host.fixture-load",
            FactValue::Integer(30),
            at,
            Some(60_000),
        ),
        runtime_fact(
            scope_b.clone(),
            "task.fixture-settled",
            FactValue::Boolean(true),
            at,
            None,
        ),
    ] {
        live.record(record).expect("record before the snapshot");
    }
    live.record(runtime_fact(
        scope_a.clone(),
        "device.fixture-session",
        FactValue::Boolean(false),
        at + 10,
        None,
    ))
    .expect("update before the snapshot");
    live.invalidate(
        &scope_b,
        "task.fixture-settled",
        RuntimeFactInvalidationReason::Operator,
        at + 20,
    )
    .expect("invalidation before the snapshot");
    let snapshot = live.snapshot(SNAPSHOT_LEDGER_POSITION, at + 30);

    let mut tail = Vec::new();
    for record in [
        runtime_fact(
            scope_a.clone(),
            "application.fixture-foreground",
            FactValue::String("fixture.app".to_owned()),
            at + 40,
            Some(60_000),
        ),
        runtime_fact(
            RuntimeFactScope::Runtime,
            "host.fixture-load",
            FactValue::Integer(45),
            at + 50,
            Some(60_000),
        ),
        runtime_fact(
            scope_b,
            "device.fixture-session",
            FactValue::Boolean(true),
            at + 60,
            None,
        ),
    ] {
        live.record(record.clone())
            .expect("record after the snapshot");
        tail.push(Appended::Recorded(record));
    }
    tail.push(Appended::Invalidated(
        live.invalidate(
            &scope_a,
            "backend.fixture-check",
            RuntimeFactInvalidationReason::Operator,
            at + 70,
        )
        .expect("invalidation after the snapshot"),
    ));
    tail.extend(
        live.invalidate_instance(
            a,
            &["device."],
            RuntimeFactInvalidationReason::RuntimeTakeover,
            at + 80,
        )
        .into_iter()
        .map(Appended::Invalidated),
    );

    let mut rebuilt = RuntimeFactStore::new();
    rebuilt.replay(&snapshot).expect("replay the snapshot");
    for appended in tail {
        match appended {
            Appended::Recorded(record) => {
                rebuilt.record(record).expect("apply a tail record");
            }
            Appended::Invalidated(entry) => {
                rebuilt
                    .invalidate(&entry.scope, &entry.key, entry.reason, entry.at_unix_ms)
                    .expect("apply a tail invalidation");
            }
        }
    }
    assert_eq!(
        rebuilt.records().collect::<Vec<_>>(),
        live.records().collect::<Vec<_>>()
    );
}
