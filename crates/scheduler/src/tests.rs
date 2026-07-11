// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{IdentifierIssuer, IssuedHolderId};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

fn ids() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn epoch(issuer: &IdentifierIssuer) -> OwnerEpoch {
    *issuer.mint_owner_epoch().expect("epoch").transport()
}

fn instance(issuer: &IdentifierIssuer) -> InstanceId {
    *issuer.mint_instance_id().expect("instance").transport()
}

fn holder(issuer: &IdentifierIssuer) -> (IssuedHolderId, HolderId) {
    let issued = issuer.mint_holder_id().expect("holder");
    let transport = *issued.transport();
    (issued, transport)
}

fn request(issuer: &IdentifierIssuer) -> RequestId {
    *issuer.mint_request_id().expect("request").transport()
}

fn connection(value: u64) -> ConnectionId {
    ConnectionId::new(value).expect("connection")
}

fn queued_request(
    request_id: RequestId,
    instance_id: InstanceId,
    holder_id: HolderId,
    connection_id: ConnectionId,
    priority: LeasePriority,
    timeout_ms: u64,
) -> QueueLeaseRequest {
    QueueLeaseRequest::new(
        request_id,
        instance_id,
        holder_id,
        connection_id,
        priority,
        timeout_ms,
    )
}

fn config() -> SchedulerConfig {
    SchedulerConfig {
        maximum_client_heartbeat_interval_ms: 100,
        takeover_cooldown_ms: 150,
        lease_ttl_ms: 1_000,
        maximum_queue_timeout_ms: 500,
        max_queue_depth_per_instance: 2,
    }
}

#[test]
fn defaults_freeze_c3a_heartbeat_cooldown_and_ttl() {
    let config = SchedulerConfig::default().validate().expect("defaults");
    assert_eq!(config.maximum_client_heartbeat_interval_ms, 5_000);
    assert_eq!(config.takeover_cooldown_ms, 6_000);
    assert_eq!(config.lease_ttl_ms, 120_000);
    assert_eq!(config.maximum_queue_timeout_ms, 60_000);
    assert_eq!(config.max_queue_depth_per_instance, 64);
}

#[test]
fn invalid_cooldown_relation_is_fatal() {
    let error = SchedulerConfig {
        maximum_client_heartbeat_interval_ms: 100,
        takeover_cooldown_ms: 100,
        lease_ttl_ms: 1_000,
        maximum_queue_timeout_ms: 500,
        max_queue_depth_per_instance: 2,
    }
    .validate()
    .expect_err("equal cooldown must fail");
    assert!(error.is_fatal());
    assert_eq!(error.code(), "invalid_scheduler_config");
}

#[test]
fn queue_limits_are_bounded_at_configuration_time() {
    let mut invalid_timeout = config();
    invalid_timeout.maximum_queue_timeout_ms = MAX_LEASE_QUEUE_TIMEOUT_MS + 1;
    assert_eq!(
        invalid_timeout.validate().expect_err("queue timeout bound"),
        SchedulerError::InvalidConfig
    );

    let mut invalid_depth = config();
    invalid_depth.max_queue_depth_per_instance = MAX_QUEUE_DEPTH_PER_INSTANCE + 1;
    assert_eq!(
        invalid_depth.validate().expect_err("queue depth bound"),
        SchedulerError::InvalidConfig
    );
}

#[test]
fn zero_stagger_same_instance_has_one_grant_and_one_busy_denial() {
    let issuer = ids();
    let scheduler = Arc::new(Mutex::new(
        SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler"),
    ));
    let instance_id = instance(&issuer);
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for index in 1..=2 {
        let scheduler = Arc::clone(&scheduler);
        let barrier = Arc::clone(&barrier);
        let local = ids();
        workers.push(thread::spawn(move || {
            barrier.wait();
            scheduler.lock().expect("scheduler lock").acquire(
                request(&local),
                instance_id,
                holder(&local).1,
                connection(index),
                1,
            )
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(SchedulerError::Busy { .. })))
            .count(),
        1
    );
}

#[test]
fn different_instances_are_independent() {
    let issuer = ids();
    let scheduler = Arc::new(Mutex::new(
        SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler"),
    ));
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for index in 1..=2 {
        let scheduler = Arc::clone(&scheduler);
        let barrier = Arc::clone(&barrier);
        let local = ids();
        let instance_id = instance(&issuer);
        workers.push(thread::spawn(move || {
            barrier.wait();
            scheduler.lock().expect("scheduler lock").acquire(
                request(&local),
                instance_id,
                holder(&local).1,
                connection(index),
                1,
            )
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect::<Vec<_>>();
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(
        scheduler
            .lock()
            .expect("scheduler lock")
            .active_tokens()
            .len(),
        2
    );
}

#[test]
fn renew_and_release_are_idempotent_by_request_id() {
    let issuer = ids();
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let connection_id = connection(1);
    let token = scheduler
        .acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            connection_id,
            1,
        )
        .expect("acquire");
    let renew_request = request(&issuer);
    let renewed = scheduler
        .renew(renew_request, &token, connection_id, 10)
        .expect("renew");
    let renewed_retry = scheduler
        .renew(renew_request, &token, connection_id, 20)
        .expect("renew retry");
    assert_eq!(renewed_retry, renewed);
    assert_eq!(
        scheduler
            .replayed_renew(renew_request, &token, connection_id)
            .expect("recover renew"),
        Some(renewed.clone())
    );

    let release_request = request(&issuer);
    let released = scheduler
        .release(release_request, &renewed, connection_id, 30)
        .expect("release");
    let released_retry = scheduler
        .release(release_request, &renewed, connection_id, 40)
        .expect("release retry");
    assert_eq!(released_retry, released);
    assert_eq!(
        scheduler
            .replayed_release(release_request, &renewed, connection_id)
            .expect("recover release"),
        Some(released)
    );
    assert!(scheduler.active_tokens().is_empty());
}

#[test]
fn repeated_request_id_with_mutated_token_is_not_idempotent() {
    let issuer = ids();
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let connection_id = connection(1);
    let token = scheduler
        .acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            connection_id,
            1,
        )
        .expect("acquire");
    let renew_request = request(&issuer);
    scheduler
        .renew(renew_request, &token, connection_id, 10)
        .expect("renew");
    let mutated = LeaseToken::new(
        token.owner_epoch(),
        token.lease_id(),
        token.instance_id(),
        holder(&issuer).1,
        token.expires_at_monotonic_ms(),
    )
    .expect("mutated token");
    assert_eq!(
        scheduler
            .renew(renew_request, &mutated, connection_id, 20)
            .expect_err("mutated idempotency request"),
        SchedulerError::LeaseMismatch
    );
}

#[test]
fn released_request_cannot_be_replayed_from_another_connection() {
    let issuer = ids();
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let connection_id = connection(1);
    let token = scheduler
        .acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            connection_id,
            1,
        )
        .expect("acquire");
    let release_request = request(&issuer);
    scheduler
        .release(release_request, &token, connection_id, 10)
        .expect("release");
    assert_eq!(
        scheduler
            .release(release_request, &token, connection(2), 20)
            .expect_err("cross-connection replay"),
        SchedulerError::ConnectionMismatch
    );
}

#[test]
fn fencing_rejects_every_mismatched_field_before_write() {
    let issuer = ids();
    let current_epoch = epoch(&issuer);
    let mut scheduler = SeedScheduler::new(current_epoch, config(), [], 0).expect("scheduler");
    let connection_id = connection(1);
    let token = scheduler
        .acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            connection_id,
            1,
        )
        .expect("acquire");

    let stale = LeaseToken::new(
        epoch(&ids()),
        token.lease_id(),
        token.instance_id(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("stale token");
    assert_eq!(
        scheduler
            .validate_write(&stale, connection_id, 2)
            .expect_err("stale epoch"),
        SchedulerError::StaleOwnerEpoch
    );

    let wrong_lease = LeaseToken::new(
        current_epoch,
        *issuer.mint_lease_id().expect("lease").transport(),
        token.instance_id(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong lease token");
    assert_eq!(
        scheduler
            .validate_write(&wrong_lease, connection_id, 2)
            .expect_err("wrong lease"),
        SchedulerError::LeaseMismatch
    );

    let wrong_instance = LeaseToken::new(
        current_epoch,
        token.lease_id(),
        instance(&issuer),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong instance token");
    assert_eq!(
        scheduler
            .validate_write(&wrong_instance, connection_id, 2)
            .expect_err("wrong instance"),
        SchedulerError::InstanceMismatch
    );

    let wrong_holder = LeaseToken::new(
        current_epoch,
        token.lease_id(),
        token.instance_id(),
        holder(&issuer).1,
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong holder token");
    assert_eq!(
        scheduler
            .validate_write(&wrong_holder, connection_id, 2)
            .expect_err("wrong holder"),
        SchedulerError::HolderMismatch
    );
    assert_eq!(
        scheduler
            .validate_write(&token, connection(2), 2)
            .expect_err("wrong connection"),
        SchedulerError::ConnectionMismatch
    );
    assert_eq!(
        scheduler
            .validate_write(&token, connection_id, token.expires_at_monotonic_ms())
            .expect_err("expired"),
        SchedulerError::LeaseExpired
    );
}

#[test]
fn takeover_cooldown_blocks_only_affected_instances() {
    let issuer = ids();
    let affected = instance(&issuer);
    let unaffected = instance(&issuer);
    let mut scheduler =
        SeedScheduler::new(epoch(&issuer), config(), [affected], 1_000).expect("scheduler");
    assert_eq!(scheduler.protected_instance_ids(1_100), vec![affected]);
    let error = scheduler
        .acquire(
            request(&issuer),
            affected,
            holder(&issuer).1,
            connection(1),
            1_100,
        )
        .expect_err("affected instance cooldown");
    assert_eq!(error, SchedulerError::Cooldown { retry_after_ms: 50 });
    scheduler
        .acquire(
            request(&issuer),
            unaffected,
            holder(&issuer).1,
            connection(2),
            1_100,
        )
        .expect("unaffected grant");
    scheduler
        .acquire(
            request(&issuer),
            affected,
            holder(&issuer).1,
            connection(3),
            1_150,
        )
        .expect("grant after cooldown");
    assert!(!scheduler.clear_elapsed_cooldowns(1_150));
}

#[test]
fn elapsed_takeover_cooldowns_leave_the_crash_protection_set() {
    let issuer = ids();
    let affected = instance(&issuer);
    let mut scheduler =
        SeedScheduler::new(epoch(&issuer), config(), [affected], 1_000).expect("scheduler");
    assert!(scheduler.clear_elapsed_cooldowns(1_150));
    assert!(scheduler.protected_instance_ids(1_150).is_empty());
}

#[test]
fn current_epoch_write_is_denied_by_takeover_cooldown_before_lease_lookup() {
    let issuer = ids();
    let owner_epoch = epoch(&issuer);
    let affected = instance(&issuer);
    let scheduler =
        SeedScheduler::new(owner_epoch, config(), [affected], 1_000).expect("scheduler");
    let forged = LeaseToken::new(
        owner_epoch,
        *issuer.mint_lease_id().expect("lease").transport(),
        affected,
        holder(&issuer).1,
        2_000,
    )
    .expect("token");
    assert_eq!(
        scheduler
            .validate_write(&forged, connection(1), 1_100)
            .expect_err("cooldown"),
        SchedulerError::Cooldown { retry_after_ms: 50 }
    );
}

#[test]
fn expiry_and_disconnect_release_owned_leases() {
    let issuer = ids();
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let first_connection = connection(1);
    let first = scheduler
        .acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            first_connection,
            1,
        )
        .expect("first");
    let second = scheduler
        .acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            connection(2),
            1,
        )
        .expect("second");
    assert_eq!(
        scheduler.tokens_for_connection(first_connection),
        vec![first.clone()]
    );
    let disconnected = scheduler
        .release_owned(&first, first_connection, LeaseReleaseReason::Disconnect)
        .expect("disconnect release");
    assert_eq!(disconnected.token, first);
    assert_eq!(disconnected.reason, LeaseReleaseReason::Disconnect);
    assert_eq!(
        scheduler.due_tokens(second.expires_at_monotonic_ms()),
        vec![second.clone()]
    );
    let expired = scheduler
        .expire_token(&second, second.expires_at_monotonic_ms())
        .expect("expiry release");
    assert_eq!(expired.token, second);
    assert_eq!(expired.reason, LeaseReleaseReason::Expired);
    assert!(scheduler.active_tokens().is_empty());
}

#[test]
fn prepared_lease_does_not_mutate_state_before_commit() {
    let issuer = ids();
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let prepared = scheduler
        .prepare_acquire(
            request(&issuer),
            instance(&issuer),
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("prepare");
    assert!(!prepared.is_existing());
    assert!(scheduler.active_tokens().is_empty());
    let token = scheduler.commit_acquire(prepared, 2).expect("commit");
    assert_eq!(scheduler.active_tokens(), vec![token]);
}

#[test]
fn status_accessors_report_queue_depth_and_takeover_cooldown() {
    let issuer = ids();
    let instance_id = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    scheduler
        .acquire(
            request(&issuer),
            instance_id,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("holder");
    scheduler
        .request_queued(
            queued_request(
                request(&issuer),
                instance_id,
                holder(&issuer).1,
                connection(2),
                LeasePriority::Normal,
                400,
            ),
            2,
        )
        .expect("queued request");
    assert_eq!(scheduler.queued_count(instance_id), 1);
    assert!(!scheduler.cooldown_active(instance_id, 2));

    let cooldown = SeedScheduler::new(epoch(&issuer), config(), [instance_id], 10)
        .expect("takeover scheduler");
    assert!(cooldown.cooldown_active(instance_id, 11));
    assert!(!cooldown.cooldown_active(instance_id, 161));
}

#[test]
fn queue_is_bounded_priority_ordered_and_idempotent() {
    let issuer = ids();
    let instance_id = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    scheduler
        .acquire(
            request(&issuer),
            instance_id,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("holder");

    let normal_request = request(&issuer);
    let normal_holder = holder(&issuer).1;
    let normal = scheduler
        .request_queued(
            queued_request(
                normal_request,
                instance_id,
                normal_holder,
                connection(2),
                LeasePriority::Normal,
                400,
            ),
            2,
        )
        .expect("normal queued")
        .into_decision();
    let QueueAdmissionDecision::Queued(normal) = normal else {
        panic!("expected queued normal request");
    };
    assert_eq!(normal.position(), 1);
    assert!(!normal.preempt_requested());

    let high_request = request(&issuer);
    let high_holder = holder(&issuer).1;
    let high = scheduler
        .request_queued(
            queued_request(
                high_request,
                instance_id,
                high_holder,
                connection(3),
                LeasePriority::High,
                400,
            ),
            3,
        )
        .expect("high queued")
        .into_decision();
    let QueueAdmissionDecision::Queued(high) = high else {
        panic!("expected queued high request");
    };
    assert_eq!(high.position(), 1);
    assert!(high.preempt_requested());
    let QueuePoll::Pending(normal_after_reorder) = scheduler
        .poll_queued(normal_request, connection(2), 4)
        .expect("normal pending")
    else {
        panic!("normal request must remain queued");
    };
    assert_eq!(normal_after_reorder.position(), 2);

    let replay = scheduler
        .request_queued(
            queued_request(
                high_request,
                instance_id,
                high_holder,
                connection(3),
                LeasePriority::High,
                400,
            ),
            5,
        )
        .expect("idempotent queue replay")
        .into_decision();
    let QueueAdmissionDecision::Queued(replay) = replay else {
        panic!("expected replayed queue status");
    };
    assert_eq!(replay, high);
    assert_eq!(
        scheduler
            .request_queued(
                queued_request(
                    request(&issuer),
                    instance_id,
                    holder(&issuer).1,
                    connection(4),
                    LeasePriority::Normal,
                    400,
                ),
                6,
            )
            .expect_err("queue capacity"),
        SchedulerError::QueueFull
    );
    assert_eq!(
        scheduler
            .request_queued(
                queued_request(
                    high_request,
                    instance_id,
                    high_holder,
                    connection(9),
                    LeasePriority::High,
                    400,
                ),
                7,
            )
            .expect_err("cross-connection replay"),
        SchedulerError::QueueConnectionMismatch
    );
}

#[test]
fn zero_stagger_queued_admission_has_one_grant_and_one_queue() {
    let issuer = ids();
    let scheduler = Arc::new(Mutex::new(
        SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler"),
    ));
    let instance_id = instance(&issuer);
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for index in 1..=2 {
        let scheduler = Arc::clone(&scheduler);
        let barrier = Arc::clone(&barrier);
        let local = ids();
        workers.push(thread::spawn(move || {
            barrier.wait();
            let mut scheduler = scheduler.lock().expect("scheduler lock");
            let decision = scheduler
                .request_queued(
                    queued_request(
                        request(&local),
                        instance_id,
                        holder(&local).1,
                        connection(index),
                        LeasePriority::Normal,
                        400,
                    ),
                    1,
                )
                .expect("queued admission")
                .into_decision();
            match decision {
                QueueAdmissionDecision::Lease(prepared) => {
                    scheduler.commit_acquire(prepared, 1).expect("grant");
                    "granted"
                }
                QueueAdmissionDecision::Queued(_) => "queued",
            }
        }));
    }
    barrier.wait();
    let outcomes = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == "granted")
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == "queued")
            .count(),
        1
    );
}

#[test]
fn queues_and_transfers_are_partitioned_by_instance() {
    let issuer = ids();
    let first_instance = instance(&issuer);
    let second_instance = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let first = scheduler
        .acquire(
            request(&issuer),
            first_instance,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("first holder");
    let second = scheduler
        .acquire(
            request(&issuer),
            second_instance,
            holder(&issuer).1,
            connection(2),
            1,
        )
        .expect("second holder");
    let first_queued = request(&issuer);
    let second_queued = request(&issuer);
    scheduler
        .request_queued(
            queued_request(
                first_queued,
                first_instance,
                holder(&issuer).1,
                connection(3),
                LeasePriority::High,
                400,
            ),
            2,
        )
        .expect("first queue");
    scheduler
        .request_queued(
            queued_request(
                second_queued,
                second_instance,
                holder(&issuer).1,
                connection(4),
                LeasePriority::Normal,
                400,
            ),
            2,
        )
        .expect("second queue");

    let TransferPreparation::Ready(prepared) = scheduler
        .prepare_transfer(
            &first,
            connection(1),
            LeaseTransferReason::Preempted,
            None,
            3,
        )
        .expect("first preempt")
    else {
        panic!("first instance must preempt");
    };
    scheduler
        .commit_transfer(prepared, 3)
        .expect("first transfer");
    scheduler
        .validate_write(&second, connection(2), 3)
        .expect("second instance remains active");
    assert!(matches!(
        scheduler
            .poll_queued(second_queued, connection(4), 3)
            .expect("second queue remains"),
        QueuePoll::Pending(_)
    ));
    assert!(matches!(
        scheduler
            .poll_queued(first_queued, connection(3), 3)
            .expect("first promoted"),
        QueuePoll::Granted(_)
    ));
}

#[test]
fn destructive_step_defers_preemption_until_prepared_transfer_is_committed() {
    let issuer = ids();
    let instance_id = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let current = scheduler
        .acquire(
            request(&issuer),
            instance_id,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("current lease");
    scheduler
        .begin_destructive_step(&current, connection(1), 2)
        .expect("begin destructive");
    let queued_id = request(&issuer);
    scheduler
        .request_queued(
            queued_request(
                queued_id,
                instance_id,
                holder(&issuer).1,
                connection(2),
                LeasePriority::High,
                400,
            ),
            3,
        )
        .expect("preempt queue");
    assert!(matches!(
        scheduler
            .prepare_transfer(
                &current,
                connection(1),
                LeaseTransferReason::Preempted,
                None,
                4,
            )
            .expect("deferred transfer"),
        TransferPreparation::Deferred
    ));
    scheduler
        .finish_destructive_step(&current, connection(1))
        .expect("finish destructive");
    let TransferPreparation::Ready(prepared) = scheduler
        .prepare_transfer(
            &current,
            connection(1),
            LeaseTransferReason::Preempted,
            None,
            5,
        )
        .expect("prepare transfer")
    else {
        panic!("expected prepared transfer");
    };
    let next = prepared.to_token().clone();
    scheduler
        .validate_write(&current, connection(1), 5)
        .expect("old authority remains before commit");
    assert!(matches!(
        scheduler
            .poll_queued(queued_id, connection(2), 5)
            .expect("still queued before commit"),
        QueuePoll::Pending(_)
    ));
    assert_eq!(
        scheduler
            .commit_transfer(prepared, 5)
            .expect("commit transfer"),
        next
    );
    assert_eq!(
        scheduler
            .validate_write(&current, connection(1), 6)
            .expect_err("old token fenced"),
        SchedulerError::LeaseMismatch
    );
    assert_eq!(
        scheduler
            .poll_queued(queued_id, connection(2), 6)
            .expect("new grant visible"),
        QueuePoll::Granted(next.clone())
    );
    scheduler
        .validate_write(&next, connection(2), 6)
        .expect("new token active");
}

#[test]
fn pending_preemption_blocks_a_new_destructive_step_at_the_safe_boundary() {
    let issuer = ids();
    let instance_id = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let current = scheduler
        .acquire(
            request(&issuer),
            instance_id,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("current lease");
    scheduler
        .request_queued(
            queued_request(
                request(&issuer),
                instance_id,
                holder(&issuer).1,
                connection(2),
                LeasePriority::High,
                400,
            ),
            2,
        )
        .expect("preempt queue");

    assert_eq!(
        scheduler
            .begin_destructive_step(&current, connection(1), 3)
            .expect_err("safe boundary must yield"),
        SchedulerError::TransferNotSafe
    );
}

#[test]
fn equal_priority_waits_but_explicit_release_can_transfer() {
    let issuer = ids();
    let instance_id = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    let current = scheduler
        .acquire(
            request(&issuer),
            instance_id,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("current lease");
    let queued_id = request(&issuer);
    scheduler
        .request_queued(
            queued_request(
                queued_id,
                instance_id,
                holder(&issuer).1,
                connection(2),
                LeasePriority::Normal,
                400,
            ),
            2,
        )
        .expect("queued");
    assert_eq!(
        scheduler
            .prepare_transfer(
                &current,
                connection(1),
                LeaseTransferReason::Preempted,
                Some(request(&issuer)),
                3,
            )
            .expect_err("preempt cannot carry release identity"),
        SchedulerError::QueueRequestMismatch
    );
    assert!(matches!(
        scheduler
            .prepare_transfer(
                &current,
                connection(1),
                LeaseTransferReason::Preempted,
                None,
                3,
            )
            .expect("no equal preempt"),
        TransferPreparation::NoCandidate
    ));
    let release_request = request(&issuer);
    let TransferPreparation::Ready(prepared) = scheduler
        .prepare_transfer(
            &current,
            connection(1),
            LeaseTransferReason::ExplicitRelease,
            Some(release_request),
            3,
        )
        .expect("release transfer")
    else {
        panic!("release must promote queue");
    };
    let next = scheduler.commit_transfer(prepared, 3).expect("commit");
    assert_eq!(
        scheduler
            .replayed_release(release_request, &current, connection(1))
            .expect("release replay")
            .expect("release recorded")
            .reason,
        LeaseReleaseReason::Explicit
    );
    assert_eq!(
        scheduler
            .poll_queued(queued_id, connection(2), 4)
            .expect("promoted"),
        QueuePoll::Granted(next)
    );
}

#[test]
fn expiry_cancellation_and_disconnect_are_visible_and_connection_bound() {
    let issuer = ids();
    let instance_id = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    scheduler
        .acquire(
            request(&issuer),
            instance_id,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("holder");
    let expires = request(&issuer);
    scheduler
        .request_queued(
            queued_request(
                expires,
                instance_id,
                holder(&issuer).1,
                connection(2),
                LeasePriority::Normal,
                10,
            ),
            2,
        )
        .expect("expiring queue");
    assert_eq!(
        scheduler
            .poll_queued(expires, connection(2), 12)
            .expect_err("expired queue"),
        SchedulerError::QueueExpired
    );

    let cancelled = request(&issuer);
    scheduler
        .request_queued(
            queued_request(
                cancelled,
                instance_id,
                holder(&issuer).1,
                connection(3),
                LeasePriority::Normal,
                100,
            ),
            20,
        )
        .expect("cancel queue");
    assert_eq!(
        scheduler
            .cancel_queued(cancelled, connection(4))
            .expect_err("cross-connection cancel"),
        SchedulerError::QueueConnectionMismatch
    );
    assert_eq!(
        scheduler
            .cancel_queued(cancelled, connection(3))
            .expect("cancelled")
            .queued()
            .request_id(),
        cancelled
    );

    let disconnected = request(&issuer);
    scheduler
        .request_queued(
            queued_request(
                disconnected,
                instance_id,
                holder(&issuer).1,
                connection(5),
                LeasePriority::Normal,
                100,
            ),
            30,
        )
        .expect("disconnect queue");
    let removed = scheduler
        .remove_queued_for_connection(connection(5))
        .expect("disconnect cleanup");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].queued().request_id(), disconnected);
    assert_eq!(
        scheduler
            .poll_queued(disconnected, connection(5), 31)
            .expect_err("removed queue"),
        SchedulerError::QueueMissing
    );
}

#[test]
fn queue_timeout_and_request_identity_mismatches_fail_loudly() {
    let issuer = ids();
    let first_instance = instance(&issuer);
    let second_instance = instance(&issuer);
    let mut scheduler = SeedScheduler::new(epoch(&issuer), config(), [], 0).expect("scheduler");
    scheduler
        .acquire(
            request(&issuer),
            first_instance,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("holder");
    let queued_id = request(&issuer);
    let queued_holder = holder(&issuer).1;
    assert_eq!(
        scheduler
            .request_queued(
                queued_request(
                    queued_id,
                    first_instance,
                    queued_holder,
                    connection(2),
                    LeasePriority::Normal,
                    501,
                ),
                2,
            )
            .expect_err("timeout above configured maximum"),
        SchedulerError::QueueTimeoutInvalid
    );
    scheduler
        .request_queued(
            queued_request(
                queued_id,
                first_instance,
                queued_holder,
                connection(2),
                LeasePriority::Normal,
                100,
            ),
            2,
        )
        .expect("queue");
    assert_eq!(
        scheduler
            .request_queued(
                queued_request(
                    queued_id,
                    second_instance,
                    queued_holder,
                    connection(2),
                    LeasePriority::Normal,
                    100,
                ),
                3,
            )
            .expect_err("request cannot move instances"),
        SchedulerError::QueueRequestMismatch
    );
}
