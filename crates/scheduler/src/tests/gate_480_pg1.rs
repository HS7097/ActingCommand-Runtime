// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-1 fix round 2 (register §1.468; `contracts/scheduling-pause.md`, "Instance pause: three
// stages", (c)): the one queued entry that an instance pause's hand-back
// (`SeedScheduler::member_refusing_count`) and a dedicated lease (`SeedScheduler::refusing_count`,
// `SeedScheduler::prepare_dedicated_lease`) pass over: the instance's own autostart claim, and only
// where the caller gives the word that its claim ends at its grant (`AutostartEndsAtGrant` naming
// the instance: the host gives it while a scheduling pause holds the instance's automatic starts).
// On a device whose two members the map keys as the assembly derives it. Fake ids, a fake clock in
// milliseconds, no sleeps.

use super::{claim, claim_config, connection, epoch, holder, ids, instance, request};
use crate::{
    AutostartEndsAtGrant, ClaimGate, ClaimKind, MemberGates, SchedulerError, SeedScheduler,
};
use actingcommand_contract::{IdentifierIssuer, InstanceId, LeasePriority, RequestId};
use std::collections::BTreeMap;

/// The fake clock's reading at every step.
const NOW: u64 = 10;

/// A scheduler for one device of the two members `first` and `second`, keyed as the assembly keys
/// it, with one claim of `kind` serving `member` queued on the free device; and that claim's
/// request.
fn queued(
    issuer: &IdentifierIssuer,
    first: InstanceId,
    second: InstanceId,
    member: InstanceId,
    kind: ClaimKind,
) -> (SeedScheduler, RequestId) {
    let key = first.min(second);
    let mut scheduler = SeedScheduler::new_for_devices(
        epoch(issuer),
        claim_config(),
        BTreeMap::from([(first, key), (second, key)]),
        [],
        0,
    )
    .expect("scheduler");
    let entry = claim(
        issuer,
        member,
        holder(issuer).1,
        connection(1),
        kind,
        LeasePriority::Normal,
    );
    scheduler
        .enqueue_claim(entry, NOW)
        .expect("queued on the free device");
    (scheduler, entry.request_id)
}

/// Whether the dedicated close lease of `member` is prepared under `gate`.
fn close_lease(
    scheduler: &mut SeedScheduler,
    issuer: &IdentifierIssuer,
    member: InstanceId,
    gate: impl MemberGates,
) -> Result<(), SchedulerError> {
    scheduler
        .prepare_dedicated_lease(
            request(issuer),
            member,
            holder(issuer).1,
            connection(9),
            ClaimKind::ResourceClose,
            gate,
            NOW,
        )
        .map(|_| ())
}

/// The requests waiting on the device of `member`, in grant order.
fn queue(scheduler: &SeedScheduler, member: InstanceId) -> Vec<RequestId> {
    scheduler
        .queued_on(member)
        .expect("the device's queue")
        .iter()
        .map(|entry| entry.request_id())
        .collect()
}

// The first member's own queued autostart claim, under a closed routine gate. Without the word, and
// under the word that names the other member, it refuses the first member's pause's hand-back and
// its dedicated close lease as before (`TransferNotSafe`). Under the word that names the first
// member it refuses neither: the hand-back counts nothing, and the close lease is granted ahead of
// it (no `QueueAhead`) while it keeps its place in the queue.
#[test]
fn pg1_fix2_only_the_instances_own_queued_autostart_claim_is_passed_over_under_its_pause() {
    let issuer = ids();
    let first = instance(&issuer);
    let second = instance(&issuer);
    let paused = AutostartEndsAtGrant {
        gates: ClaimGate::RoutineHeld,
        member: first,
    };
    let other_paused = AutostartEndsAtGrant {
        gates: ClaimGate::RoutineHeld,
        member: second,
    };
    let (mut scheduler, own) = queued(&issuer, first, second, first, ClaimKind::Autostart);

    assert_eq!(
        scheduler.member_refusing_count(first, ClaimGate::RoutineHeld, NOW),
        1
    );
    assert_eq!(
        scheduler.refusing_count(first, ClaimGate::RoutineHeld, NOW),
        1
    );
    assert_eq!(
        close_lease(&mut scheduler, &issuer, first, ClaimGate::RoutineHeld),
        Err(SchedulerError::TransferNotSafe),
        "without the word"
    );
    assert_eq!(scheduler.member_refusing_count(first, other_paused, NOW), 1);
    assert_eq!(scheduler.refusing_count(first, other_paused, NOW), 1);
    assert_eq!(
        close_lease(&mut scheduler, &issuer, first, other_paused),
        Err(SchedulerError::TransferNotSafe),
        "under the word of the other member"
    );

    assert_eq!(scheduler.member_refusing_count(first, paused, NOW), 0);
    assert_eq!(scheduler.refusing_count(first, paused, NOW), 0);
    let preparation = scheduler
        .prepare_dedicated_lease(
            request(&issuer),
            first,
            holder(&issuer).1,
            connection(9),
            ClaimKind::ResourceClose,
            paused,
            NOW,
        )
        .expect("the close lease, ahead of the claim");
    let close = scheduler
        .commit_acquire(preparation, NOW)
        .expect("the close lease holds the device");
    assert_eq!(close.instance_id(), first);
    assert_eq!(
        scheduler.active_lease(first).map(|lease| lease.kind()),
        Some(ClaimKind::ResourceClose)
    );
    assert_eq!(
        queue(&scheduler, first),
        vec![own],
        "the claim keeps its place"
    );
}

// Under the word that names the first member, every other queued entry refuses as before: the
// other member's autostart claim (the first member's dedicated close lease, which takes the device;
// the first member's pause counts only the first member's own entries, Workflow #437 MAPP-1b), a
// claim of the first member of another kind (the start's preparation) and an operator's entry of
// the first member (a person's emulator control).
#[test]
fn pg1_fix2_every_other_queued_entry_refuses_as_before_under_the_instances_pause() {
    let issuer = ids();
    let first = instance(&issuer);
    let second = instance(&issuer);
    let paused = AutostartEndsAtGrant {
        gates: ClaimGate::RoutineHeld,
        member: first,
    };
    for (member, kind, refusing_the_pause) in [
        (second, ClaimKind::Autostart, 0),
        (first, ClaimKind::DaemonStartPreparation, 1),
        (first, ClaimKind::EmulatorControl, 1),
    ] {
        let (mut scheduler, _) = queued(&issuer, first, second, member, kind);
        assert_eq!(
            scheduler.member_refusing_count(first, paused, NOW),
            refusing_the_pause,
            "{kind:?}"
        );
        assert_eq!(scheduler.refusing_count(first, paused, NOW), 1, "{kind:?}");
        assert_eq!(
            close_lease(&mut scheduler, &issuer, first, paused),
            Err(SchedulerError::TransferNotSafe),
            "{kind:?}"
        );
    }
}
