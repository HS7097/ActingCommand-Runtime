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

fn config() -> SchedulerConfig {
    SchedulerConfig {
        maximum_client_heartbeat_interval_ms: 100,
        takeover_cooldown_ms: 150,
        lease_ttl_ms: 1_000,
    }
}

#[test]
fn defaults_freeze_c3a_heartbeat_cooldown_and_ttl() {
    let config = SchedulerConfig::default().validate().expect("defaults");
    assert_eq!(config.maximum_client_heartbeat_interval_ms, 5_000);
    assert_eq!(config.takeover_cooldown_ms, 6_000);
    assert_eq!(config.lease_ttl_ms, 120_000);
}

#[test]
fn invalid_cooldown_relation_is_fatal() {
    let error = SchedulerConfig {
        maximum_client_heartbeat_interval_ms: 100,
        takeover_cooldown_ms: 100,
        lease_ttl_ms: 1_000,
    }
    .validate()
    .expect_err("equal cooldown must fail");
    assert!(error.is_fatal());
    assert_eq!(error.code(), "invalid_scheduler_config");
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

    let release_request = request(&issuer);
    let released = scheduler
        .release(release_request, &renewed, connection_id, 30)
        .expect("release");
    let released_retry = scheduler
        .release(release_request, &renewed, connection_id, 40)
        .expect("release retry");
    assert_eq!(released_retry, released);
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
fn scheduler_surface_contains_no_queue_priority_or_preemption_state() {
    let source = include_str!("lib.rs");
    for forbidden in [
        "QueuedRequest",
        "RequestPriority",
        "preempt",
        "queue_deadline",
    ] {
        assert!(
            !source.contains(forbidden),
            "forbidden C3b term: {forbidden}"
        );
    }
}
