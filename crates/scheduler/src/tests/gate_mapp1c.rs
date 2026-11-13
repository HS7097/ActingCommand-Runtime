// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 MAPP-1c (register §1.350; `contracts/device-group.md`, "The session close of a
// keyless observation"): the grant of the session close ahead
// (`SeedScheduler::prepare_session_close_ahead`), on a device whose two members the map keys as
// the assembly derives it (a group's key is the smallest instance id of its members). Fake ids, a
// fake clock in milliseconds, no sleeps.

use super::{claim, claim_config, connection, epoch, holder, ids, instance, request};
use crate::{
    ClaimGate, ClaimKind, LeasePreparation, LeaseReleaseReason, LeaseTransferReason,
    SchedulerError, SeedScheduler, TransferPreparation,
};
use actingcommand_contract::{IdentifierIssuer, InstanceId, LeasePriority, RequestId};
use std::collections::BTreeMap;

/// Two members of one device and the map the assembly derives for them.
fn group(issuer: &IdentifierIssuer) -> (InstanceId, InstanceId, BTreeMap<InstanceId, InstanceId>) {
    let first = instance(issuer);
    let second = instance(issuer);
    let key = first.min(second);
    (first, second, BTreeMap::from([(first, key), (second, key)]))
}

/// The device's queue, in grant order: each entry's request and the member it serves.
fn queue(scheduler: &SeedScheduler, member: InstanceId) -> Vec<(RequestId, InstanceId)> {
    scheduler
        .queued_on(member)
        .expect("the device's queue")
        .iter()
        .map(|entry| (entry.request_id(), entry.instance_id()))
        .collect()
}

// The close ahead is granted only while no lease is on the device: a lease of either member
// refuses it, busy while it runs and expired once it lapsed and is not ended yet. On the free
// device a claim of the other member waiting in the queue refuses the dedicated close lease
// (`TransferNotSafe`), not the close ahead. The close ahead takes nothing out of the queue, lets no
// queued grant through while it holds the device and carries no write authority, only its close;
// its end names no transfer, and the waiting claim is then the device's next grant, in its place,
// with its own member, holder and lease time.
#[test]
fn mapp1c_a_close_ahead_is_granted_only_on_a_free_device_and_a_queued_claim_keeps_its_place() {
    let issuer = ids();
    let (first, second, device_keys) = group(&issuer);
    let mut scheduler =
        SeedScheduler::new_for_devices(epoch(&issuer), claim_config(), device_keys, [], 0)
            .expect("scheduler");
    let close_connection = connection(9);

    let held = scheduler
        .acquire(
            request(&issuer),
            second,
            holder(&issuer).1,
            connection(1),
            1,
        )
        .expect("the free device");
    assert!(matches!(
        scheduler.prepare_session_close_ahead(
            request(&issuer),
            first,
            holder(&issuer).1,
            close_connection,
            2
        ),
        Err(SchedulerError::Busy { lease_id, .. }) if lease_id == held.lease_id()
    ));
    let lapsed = held.expires_at_monotonic_ms();
    assert_eq!(
        scheduler
            .prepare_session_close_ahead(
                request(&issuer),
                first,
                holder(&issuer).1,
                close_connection,
                lapsed
            )
            .map(|_| ()),
        Err(SchedulerError::LeaseExpired)
    );
    scheduler
        .expire_token(&held, lapsed)
        .expect("the lapsed lease ends");

    let waiting = claim(
        &issuer,
        second,
        holder(&issuer).1,
        connection(2),
        ClaimKind::SelfCheck,
        LeasePriority::Normal,
    );
    let queued = scheduler
        .enqueue_claim(waiting, lapsed)
        .expect("queued on the free device");
    assert_eq!(queued.position(), 1);
    let now = lapsed + 1;
    assert_eq!(
        scheduler
            .prepare_dedicated_lease(
                request(&issuer),
                first,
                holder(&issuer).1,
                close_connection,
                ClaimKind::ResourceClose,
                ClaimGate::Open,
                now
            )
            .map(|_| ()),
        Err(SchedulerError::TransferNotSafe),
        "the dedicated close lease is refused by the waiting claim"
    );
    let preparation = scheduler
        .prepare_session_close_ahead(
            request(&issuer),
            first,
            holder(&issuer).1,
            close_connection,
            now,
        )
        .expect("the close ahead on the free device");
    let close = scheduler
        .commit_acquire(preparation, now)
        .expect("granted ahead of the waiting claim");
    assert_eq!(close.instance_id(), first);
    assert_eq!(
        close.expires_at_monotonic_ms(),
        now + claim_config().lease_ttl_ms
    );
    assert_eq!(
        scheduler.active_lease(second).map(|lease| lease.kind()),
        Some(ClaimKind::ResourceClose)
    );
    assert_eq!(
        queue(&scheduler, first),
        vec![(waiting.request_id, second)],
        "the waiting claim keeps its place"
    );
    assert_eq!(
        scheduler.grant_candidate(second, ClaimGate::Open, now),
        None
    );
    assert_eq!(
        scheduler.validate_write(&close, close_connection, now),
        Err(SchedulerError::ResourceCloseOnly)
    );
    let step = scheduler
        .begin_resource_close(&close, close_connection, now)
        .expect("its close");
    scheduler
        .finish_destructive_step(step, close_connection)
        .expect("the close's step ends");

    let released = scheduler
        .release_owned(&close, close_connection, LeaseReleaseReason::SessionClosed)
        .expect("the close ahead ends");
    assert_eq!(released.reason, LeaseReleaseReason::SessionClosed);
    assert_eq!(
        queue(&scheduler, first),
        vec![(waiting.request_id, second)],
        "its end hands the device to no one"
    );
    let after = now + 1;
    assert_eq!(
        scheduler.grant_candidate(first, ClaimGate::Open, after),
        Some(ClaimKind::SelfCheck)
    );
    let granted = scheduler
        .prepare_queued_grant(first, ClaimGate::Open, after)
        .expect("the queue's grant")
        .expect("the waiting claim");
    assert_eq!(granted.request_id(), waiting.request_id);
    let token = scheduler
        .commit_acquire(LeasePreparation::New(granted), after)
        .expect("the waiting claim holds the device");
    assert_eq!(token.instance_id(), second);
    assert_eq!(token.holder_id(), waiting.holder_id);
    assert_eq!(
        token.expires_at_monotonic_ms(),
        after + waiting.lease_ttl_ms
    );
    assert!(queue(&scheduler, first).is_empty());
}

// In the device's takeover cooldown every other grant waits it out (an acquisition is refused
// with the cooldown's own remaining time, the queue grants nothing); the close ahead is granted
// through it, and only its own token passes the cooldown, for its close and for no write. While
// the cooldown runs it hands the device to no one, the waiting claim included. Its end leaves the
// cooldown as it was: the same end, and its token no longer passes. At that end the waiting claim
// is the device's next grant.
#[test]
fn mapp1c_in_the_cooldown_only_the_close_aheads_token_passes_and_the_cooldown_end_stays() {
    let issuer = ids();
    let (first, second, device_keys) = group(&issuer);
    let start = 1_000;
    let mut scheduler =
        SeedScheduler::new_for_devices(epoch(&issuer), claim_config(), device_keys, [first], start)
            .expect("scheduler");
    let end = start + claim_config().takeover_cooldown_ms;
    let close_connection = connection(9);
    let waiting = claim(
        &issuer,
        second,
        holder(&issuer).1,
        connection(2),
        ClaimKind::SelfCheck,
        LeasePriority::Normal,
    );
    scheduler
        .enqueue_claim(waiting, start + 10)
        .expect("queued in the cooldown");
    let now = start + 20;
    assert_eq!(
        scheduler.cooldown_remaining_ms(second, now),
        Some(end - now)
    );
    assert_eq!(
        scheduler
            .acquire(
                request(&issuer),
                second,
                holder(&issuer).1,
                connection(3),
                now
            )
            .map(|_| ()),
        Err(SchedulerError::Cooldown {
            retry_after_ms: end - now
        })
    );
    assert!(
        scheduler
            .prepare_queued_grant(first, ClaimGate::Open, now)
            .expect("the queue's grant")
            .is_none()
    );

    let preparation = scheduler
        .prepare_session_close_ahead(
            request(&issuer),
            first,
            holder(&issuer).1,
            close_connection,
            now,
        )
        .expect("the close ahead in the cooldown");
    let close = scheduler
        .commit_acquire(preparation, now)
        .expect("granted through the cooldown");
    let step = scheduler
        .begin_resource_close(&close, close_connection, now)
        .expect("its close passes the cooldown");
    scheduler
        .finish_destructive_step(step, close_connection)
        .expect("the close's step ends");
    assert_eq!(
        scheduler.validate_write(&close, close_connection, now),
        Err(SchedulerError::ResourceCloseOnly)
    );
    assert!(matches!(
        scheduler.prepare_transfer_gated(
            &close,
            close_connection,
            LeaseTransferReason::BackendFailure,
            None,
            ClaimGate::Open,
            now
        ),
        Ok(TransferPreparation::NoCandidate)
    ));
    assert_eq!(
        scheduler.cooldown_remaining_ms(second, now),
        Some(end - now)
    );

    scheduler
        .release_owned(&close, close_connection, LeaseReleaseReason::SessionClosed)
        .expect("the close ahead ends");
    assert_eq!(
        scheduler.cooldown_remaining_ms(second, now),
        Some(end - now)
    );
    assert_eq!(
        scheduler
            .begin_resource_close(&close, close_connection, now)
            .map(|_| ()),
        Err(SchedulerError::Cooldown {
            retry_after_ms: end - now
        }),
        "its token no longer passes the cooldown"
    );
    assert_eq!(queue(&scheduler, first), vec![(waiting.request_id, second)]);
    assert_eq!(
        scheduler.grant_candidate(first, ClaimGate::Open, end - 1),
        None
    );
    let granted = scheduler
        .prepare_queued_grant(first, ClaimGate::Open, end)
        .expect("the queue's grant")
        .expect("the waiting claim at the cooldown's end");
    let token = scheduler
        .commit_acquire(LeasePreparation::New(granted), end)
        .expect("the waiting claim holds the device");
    assert_eq!(token.instance_id(), second);
    assert_eq!(token.holder_id(), waiting.holder_id);
}
