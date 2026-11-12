// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #437 MAPP-1 (register §1.323, §1.328; `contracts/device-group.md`, "One queue, one
// worker, one lease per device", "Never interrupted", "Derived, never stored"): the scheduler
// keyed by the device. The load refuses every group of several until the slice "order within one
// device", so these tests hand the scheduler the member → device key map as the assembly builds
// it (a group's key is the smallest instance id of its members). Fake ids, a fake clock in
// milliseconds, no sleeps.

use super::{
    claim, claim_config, connection, epoch, holder, ids, instance, queued_request, request,
};
use crate::{
    ClaimGate, ClaimKind, LeasePreparation, LeaseTransferReason, QueueAdmissionDecision,
    SchedulerError, SeedScheduler, TransferPreparation,
};
use actingcommand_contract::{IdentifierIssuer, InstanceId, LeasePriority};
use std::collections::BTreeMap;

/// Two members of one device and the map the assembly derives for them.
fn group(issuer: &IdentifierIssuer) -> (InstanceId, InstanceId, BTreeMap<InstanceId, InstanceId>) {
    let first = instance(issuer);
    let second = instance(issuer);
    let key = first.min(second);
    (first, second, BTreeMap::from([(first, key), (second, key)]))
}

fn devices(
    issuer: &IdentifierIssuer,
    device_keys: BTreeMap<InstanceId, InstanceId>,
) -> SeedScheduler {
    SeedScheduler::new_for_devices(epoch(issuer), claim_config(), device_keys, [], 0)
        .expect("scheduler")
}

// Register §1.323, MAPP-1's tests: "two members never hold the device at once (lease, queue,
// admission)". The lease of one member holds the device for the other; the other's claim waits in
// the device's one queue naming its own member, and it holds the device only once the first lets
// it go. Every token names the member it serves; another device goes on meanwhile.
#[test]
fn mapp1_two_members_never_hold_their_device_at_once_lease_queue_and_admission() {
    let issuer = ids();
    let (first, second, device_keys) = group(&issuer);
    let lone = instance(&issuer);
    let mut scheduler = devices(&issuer, device_keys);
    assert_eq!(scheduler.device_key(first), first.min(second));
    assert_eq!(scheduler.device_key(second), first.min(second));
    assert_eq!(scheduler.device_key(lone), lone);

    let first_request = request(&issuer);
    let first_token = scheduler
        .acquire(first_request, first, holder(&issuer).1, connection(1), 1)
        .expect("the free device");
    assert_eq!(first_token.instance_id(), first);
    // The admission of the other member meets the device's holder.
    assert!(matches!(
        scheduler.acquire(
            request(&issuer),
            second,
            holder(&issuer).1,
            connection(2),
            2
        ),
        Err(SchedulerError::Busy { .. })
    ));
    let second_claim = claim(
        &issuer,
        second,
        holder(&issuer).1,
        connection(2),
        ClaimKind::DirectTaskRun,
        LeasePriority::Normal,
    );
    assert!(matches!(
        scheduler.prepare_claim_acquire(second_claim, ClaimGate::Open, 2),
        Err(SchedulerError::Busy { .. })
    ));
    // It waits in the device's one queue, naming its own member.
    let queued = scheduler
        .enqueue_claim(second_claim, 2)
        .expect("queued on the device");
    assert_eq!(queued.instance_id(), second);
    assert_eq!(queued.position(), 1);
    for member in [first, second] {
        let order = scheduler
            .queued_on(member)
            .expect("the device's queue")
            .iter()
            .map(|entry| (entry.request_id(), entry.instance_id()))
            .collect::<Vec<_>>();
        assert_eq!(order, vec![(second_claim.request_id, second)]);
        // An answer about the device is the device's: its holder names the first member.
        assert_eq!(
            scheduler
                .active_lease(member)
                .map(|lease| lease.token().instance_id()),
            Some(first)
        );
    }
    // Another device goes on.
    let lone_token = scheduler
        .acquire(request(&issuer), lone, holder(&issuer).1, connection(3), 3)
        .expect("another device");
    assert_eq!(lone_token.instance_id(), lone);

    // The holder ends; the device passes to the waiting claim, whose token names its member.
    let release = request(&issuer);
    let prepared = scheduler
        .prepare_transfer(
            &first_token,
            connection(1),
            LeaseTransferReason::ExplicitRelease,
            Some(release),
            4,
        )
        .expect("the transfer");
    let TransferPreparation::Ready(prepared) = prepared else {
        panic!("the waiting claim receives the device");
    };
    assert_eq!(prepared.to_token().instance_id(), second);
    let second_token = scheduler
        .commit_transfer(prepared, 4)
        .expect("the device is the second member's");
    assert_eq!(second_token.instance_id(), second);
    assert!(scheduler.queued_on(first).expect("queue").is_empty());
    // Now the first member's admission meets the second member's holder.
    assert!(matches!(
        scheduler.acquire(request(&issuer), first, holder(&issuer).1, connection(1), 5),
        Err(SchedulerError::Busy { .. })
    ));
    // A renewal keeps the member's id.
    let renewed = scheduler
        .renew(request(&issuer), &second_token, connection(2), 6)
        .expect("the renewal");
    assert_eq!(renewed.instance_id(), second);
    // The device's holder is the second member's, and the lone instance holds its own.
    let mut holders = scheduler.active_instance_ids();
    holders.sort_unstable();
    let mut expected = vec![second, lone];
    expected.sort_unstable();
    assert_eq!(holders, expected);
}

// Register §1.323 ("Never interrupted"): in a group of several a client `QueueLease` naming one
// member asks no preemption of a holder that serves another member; on its own member it keeps
// today's rule.
#[test]
fn mapp1_a_client_queue_lease_asks_no_preemption_of_a_holder_that_serves_another_member() {
    let issuer = ids();
    let (first, second, device_keys) = group(&issuer);
    let mut scheduler = devices(&issuer, device_keys);
    let token = scheduler
        .acquire(request(&issuer), first, holder(&issuer).1, connection(1), 1)
        .expect("the free device");
    let other = request(&issuer);
    let outcome = scheduler
        .request_queued(
            queued_request(
                other,
                second,
                holder(&issuer).1,
                connection(2),
                LeasePriority::High,
                400,
            ),
            2,
        )
        .expect("queued");
    let QueueAdmissionDecision::Queued(queued) = outcome.decision() else {
        panic!("the device is held: {outcome:?}");
    };
    assert_eq!(queued.instance_id(), second);
    assert!(!queued.preempt_requested());
    assert!(
        !scheduler
            .active_lease(first)
            .expect("the holder")
            .preempt_requested()
    );
    assert!(matches!(
        scheduler
            .prepare_transfer(
                &token,
                connection(1),
                LeaseTransferReason::Preempted,
                None,
                3
            )
            .expect("the preemption check"),
        TransferPreparation::NoCandidate
    ));
    // The same request on the holder's own member asks the preemption, as on a lone instance.
    scheduler
        .cancel_queued(other, connection(2))
        .expect("cancelled");
    let own = scheduler
        .request_queued(
            queued_request(
                request(&issuer),
                first,
                holder(&issuer).1,
                connection(3),
                LeasePriority::High,
                400,
            ),
            4,
        )
        .expect("queued");
    let QueueAdmissionDecision::Queued(queued) = own.decision() else {
        panic!("the device is held: {own:?}");
    };
    assert!(queued.preempt_requested());
    assert!(
        scheduler
            .active_lease(first)
            .expect("the holder")
            .preempt_requested()
    );
    assert!(matches!(
        scheduler
            .prepare_transfer(
                &token,
                connection(1),
                LeaseTransferReason::Preempted,
                None,
                5
            )
            .expect("the preemption check"),
        TransferPreparation::Ready(_)
    ));
}

// Register §1.323 ("Pause and hold", "One queue, one worker, one lease per device"): a scheduling
// pause stays per member, so each entry of the device's queue is judged by its own member's gate;
// a pause of one member counts only that member's claims, and a dedicated lease of the device
// counts every member's.
#[test]
fn mapp1_each_queued_claim_is_judged_by_its_own_members_gate() {
    let issuer = ids();
    let (first, second, device_keys) = group(&issuer);
    let mut scheduler = devices(&issuer, device_keys);
    let ladder = claim(
        &issuer,
        second,
        holder(&issuer).1,
        connection(2),
        ClaimKind::Ladder,
        LeasePriority::Normal,
    );
    let startup = claim(
        &issuer,
        first,
        holder(&issuer).1,
        connection(1),
        ClaimKind::StartupPackage,
        LeasePriority::Normal,
    );
    for queued in [ladder, startup] {
        scheduler.enqueue_claim(queued, 1).expect("queued");
    }
    // One gate for both: the first arrival goes first, whichever member it serves.
    assert_eq!(
        scheduler.grant_candidate(first, ClaimGate::Open, 2),
        Some(ClaimKind::Ladder)
    );
    assert_eq!(
        scheduler.grant_candidate(first, ClaimGate::RoutineHeld, 2),
        None
    );
    // The second member paused, the first not: the first member's claim is the device's next.
    let paused = |member: InstanceId| {
        if member == second {
            ClaimGate::RoutineHeld
        } else {
            ClaimGate::Open
        }
    };
    assert_eq!(
        scheduler.grant_candidate(second, paused, 2),
        Some(ClaimKind::StartupPackage)
    );
    assert_eq!(scheduler.member_refusing_count(first, paused, 2), 1);
    assert_eq!(scheduler.member_refusing_count(second, paused, 2), 0);
    assert_eq!(
        scheduler.member_refusing_count(second, ClaimGate::Open, 2),
        1
    );
    assert_eq!(scheduler.refusing_count(first, ClaimGate::Open, 2), 2);
    assert_eq!(scheduler.refusing_count(second, paused, 2), 1);
    let prepared = scheduler
        .prepare_queued_grant(second, paused, 3)
        .expect("the grant")
        .expect("an eligible entry");
    assert_eq!(prepared.kind(), ClaimKind::StartupPackage);
    assert_eq!(prepared.token().instance_id(), first);
    let token = scheduler
        .commit_acquire(LeasePreparation::New(prepared), 3)
        .expect("granted");
    assert_eq!(token.instance_id(), first);
    let left = scheduler
        .queued_on(first)
        .expect("queue")
        .iter()
        .map(|entry| (entry.request_id(), entry.instance_id()))
        .collect::<Vec<_>>();
    assert_eq!(left, vec![(ladder.request_id, second)]);
    // Removing one member's claims leaves the other member's.
    assert!(
        scheduler
            .remove_queued_for_instance(first)
            .expect("removed")
            .is_empty()
    );
    assert_eq!(
        scheduler
            .remove_queued_for_instance(second)
            .expect("removed")
            .iter()
            .map(|cancelled| cancelled.queued().instance_id())
            .collect::<Vec<_>>(),
        vec![second]
    );
}

// Register §1.323, MAPP-1's tests: "a restart rebuilds the member → device map from the
// configuration in effect, and the takeover cooldown covers the group". A member the owner
// journal lists gives its whole device the takeover cooldown; the journal lists one member id per
// device; a start whose configuration names no group keys each instance on its own again.
#[test]
fn mapp1_the_takeover_cooldown_of_a_listed_member_covers_its_group() {
    let issuer = ids();
    let (first, second, device_keys) = group(&issuer);
    let lone = instance(&issuer);
    let key = first.min(second);
    let mut scheduler = SeedScheduler::new_for_devices(
        epoch(&issuer),
        claim_config(),
        device_keys,
        [second],
        1_000,
    )
    .expect("scheduler");
    for member in [first, second] {
        assert!(scheduler.cooldown_active(member, 1_000), "{member:?}");
    }
    assert!(!scheduler.cooldown_active(lone, 1_000));
    assert!(matches!(
        scheduler.acquire(
            request(&issuer),
            first,
            holder(&issuer).1,
            connection(1),
            1_000
        ),
        Err(SchedulerError::Cooldown { .. })
    ));
    assert_eq!(scheduler.protected_instance_ids(1_000), vec![key]);
    // After the cooldown (150 ms) the holding member is listed.
    let token = scheduler
        .acquire(
            request(&issuer),
            first,
            holder(&issuer).1,
            connection(1),
            1_200,
        )
        .expect("the device after its cooldown");
    assert_eq!(scheduler.protected_instance_ids(1_200), vec![first]);
    assert_eq!(token.instance_id(), first);

    // The next start's configuration names no group: the listed member alone is cooled down.
    let rebuilt = SeedScheduler::new_for_devices(
        epoch(&issuer),
        claim_config(),
        BTreeMap::new(),
        [second],
        1_000,
    )
    .expect("scheduler");
    assert!(rebuilt.cooldown_active(second, 1_000));
    assert!(!rebuilt.cooldown_active(first, 1_000));
}

// Register §1.323, MAPP-1's tests: "a group of one writes what today writes". A map that keys an
// instance by its own id, as the assembly derives for an instance alone on its device, gives the
// same decisions as no map: two instances are two devices.
#[test]
fn mapp1_an_instance_keyed_by_its_own_id_decides_as_without_a_map() {
    let issuer = ids();
    let first = instance(&issuer);
    let second = instance(&issuer);
    let decisions = |mut scheduler: SeedScheduler| {
        let first_token = scheduler
            .acquire(request(&issuer), first, holder(&issuer).1, connection(1), 1)
            .expect("the first instance");
        let second_token = scheduler
            .acquire(
                request(&issuer),
                second,
                holder(&issuer).1,
                connection(2),
                1,
            )
            .expect("the second instance");
        let busy = matches!(
            scheduler.acquire(request(&issuer), first, holder(&issuer).1, connection(3), 2),
            Err(SchedulerError::Busy { .. })
        );
        let queued = scheduler
            .enqueue_claim(
                claim(
                    &issuer,
                    first,
                    holder(&issuer).1,
                    connection(3),
                    ClaimKind::DirectTaskRun,
                    LeasePriority::Normal,
                ),
                2,
            )
            .expect("queued");
        (
            first_token.instance_id(),
            second_token.instance_id(),
            busy,
            queued.instance_id(),
            queued.position(),
            scheduler.queued_count(second),
            scheduler.active_instance_ids().len(),
        )
    };
    let without = decisions(SeedScheduler::new(epoch(&issuer), claim_config(), [], 0).expect("s"));
    let own_ids = decisions(devices(
        &issuer,
        BTreeMap::from([(first, first), (second, second)]),
    ));
    assert_eq!(without, own_ids);
    assert_eq!(without, (first, second, true, first, 1, 0, 2));
}
