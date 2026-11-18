// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2b (register §1.471; `contracts/scheduling-pause.md`, "Instance pause: three stages",
// (c)): the dedicated close lease of an instance pause's hand-back (`PauseHandBack` naming the
// paused member) does not wait behind the entries of that member which its pause holds back (its
// routine entries: a startup package, a ladder's claim). Every other member's entries, and every
// entry of that member the pause does not hold back, count as before. On a device whose two
// members the map keys as the assembly derives it. Fake ids, a fake clock in milliseconds, no
// sleeps.

use super::{claim, claim_config, connection, epoch, holder, ids, instance, request};
use crate::{
    AutostartEndsAtGrant, ClaimGate, ClaimKind, MemberGates, PauseHandBack, SchedulerError,
    SeedScheduler,
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

// The first member's own startup package and ladder claim, each queued under a closed routine
// gate. Without a word, under PG-1's word alone (`AutostartEndsAtGrant`, which passes only an
// autostart claim) and under the hand-back's word naming the other member, the first member's
// dedicated close lease waits behind it as before (`QueueAhead`: an immediate try jumps no entry,
// and that lease read every entry as an open gate does). Under the hand-back's word naming the
// first member the close lease is granted ahead of it, and the entry keeps its place in the queue.
#[test]
fn pg2b_the_close_lease_of_a_pause_passes_the_entries_its_pause_holds_back() {
    let issuer = ids();
    let first = instance(&issuer);
    let second = instance(&issuer);
    for kind in [ClaimKind::StartupPackage, ClaimKind::Ladder] {
        let (mut scheduler, own) = queued(&issuer, first, second, first, kind);
        let ahead = Err(SchedulerError::QueueAhead { kind, position: 1 });
        assert_eq!(
            close_lease(&mut scheduler, &issuer, first, ClaimGate::RoutineHeld),
            ahead,
            "{kind:?} without a word"
        );
        assert_eq!(
            close_lease(
                &mut scheduler,
                &issuer,
                first,
                AutostartEndsAtGrant {
                    gates: ClaimGate::RoutineHeld,
                    member: first,
                },
            ),
            ahead,
            "{kind:?} under PG-1's word alone"
        );
        assert_eq!(
            close_lease(
                &mut scheduler,
                &issuer,
                first,
                PauseHandBack {
                    gates: ClaimGate::RoutineHeld,
                    member: second,
                },
            ),
            ahead,
            "{kind:?} under the hand-back's word naming the other member"
        );

        let preparation = scheduler
            .prepare_dedicated_lease(
                request(&issuer),
                first,
                holder(&issuer).1,
                connection(9),
                ClaimKind::ResourceClose,
                PauseHandBack {
                    gates: ClaimGate::RoutineHeld,
                    member: first,
                },
                NOW,
            )
            .expect("the close lease, ahead of the entry its pause holds back");
        let close = scheduler
            .commit_acquire(preparation, NOW)
            .expect("the close lease holds the device");
        assert_eq!(close.instance_id(), first, "{kind:?}");
        assert_eq!(
            scheduler.active_lease(first).map(|lease| lease.kind()),
            Some(ClaimKind::ResourceClose),
            "{kind:?}"
        );
        assert_eq!(
            queue(&scheduler, first),
            vec![own],
            "{kind:?}: the entry keeps its place"
        );
    }
}

// Under the hand-back's word naming the first member, every other queued entry counts as before:
// the other member's startup package and ladder claim are ahead of the first member's close lease
// (`QueueAhead`, as an open gate reads them: the hand-back passes only the paused member's own
// entries); a claim of the first member that the pause does not hold back refuses it
// (`TransferNotSafe`: the start's preparation, a person's emulator control); and the first
// member's own autostart claim is passed over as under PG-1's word, the hand-back saying so too.
#[test]
fn pg2b_every_other_queued_entry_counts_as_before_under_the_pause_hand_back() {
    let issuer = ids();
    let first = instance(&issuer);
    let second = instance(&issuer);
    let hand_back = PauseHandBack {
        gates: ClaimGate::RoutineHeld,
        member: first,
    };
    for (member, kind, expected) in [
        (
            second,
            ClaimKind::StartupPackage,
            Err(SchedulerError::QueueAhead {
                kind: ClaimKind::StartupPackage,
                position: 1,
            }),
        ),
        (
            second,
            ClaimKind::Ladder,
            Err(SchedulerError::QueueAhead {
                kind: ClaimKind::Ladder,
                position: 1,
            }),
        ),
        (
            first,
            ClaimKind::DaemonStartPreparation,
            Err(SchedulerError::TransferNotSafe),
        ),
        (
            first,
            ClaimKind::EmulatorControl,
            Err(SchedulerError::TransferNotSafe),
        ),
        (first, ClaimKind::Autostart, Ok(())),
    ] {
        let (mut scheduler, entry) = queued(&issuer, first, second, member, kind);
        assert_eq!(
            close_lease(&mut scheduler, &issuer, first, hand_back),
            expected,
            "{kind:?} of the {}",
            if member == first {
                "paused member"
            } else {
                "other member"
            }
        );
        assert_eq!(
            queue(&scheduler, first),
            vec![entry],
            "{kind:?}: the entry stays queued"
        );
    }
}
