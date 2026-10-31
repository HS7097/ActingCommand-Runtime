// SPDX-License-Identifier: AGPL-3.0-only

//! D1 (plan v2.1 §4.4 on the fake backend, §3.4, §3.8, §3.11.3): one producer for many viewers,
//! the slow consumer, the lease end, the S soft-unsubscribe grace, stale samples after a
//! generation change or a gate, and the results' exit codes and priority.

mod hub_fixture;

use actingcommand_capture_bench::backend::{
    OpenFailureCause, OpenFailureHandling, open_failure_handling,
};
use actingcommand_capture_bench::clock::Clock;
use actingcommand_capture_bench::hub::{Mode, Op, RecordKind, SampleOp, Step, run_blocking};
use actingcommand_capture_bench::model::{Disposition, GateKind, PathKind, RunResult};
use actingcommand_capture_bench::views::ViewMessage;
use hub_fixture::{fixture, instant_script};

fn sample(fx: &hub_fixture::Fixture) -> Disposition {
    let mut op = SampleOp::new(0, PathKind::Nemu, fx.owner.clone());
    run_blocking(&fx.hub, &mut op).disposition
}

#[test]
fn many_viewers_share_one_producer() {
    let fx = fixture(instant_script(), true);
    assert!(fx.hub.views.claim_producer(0));
    assert!(
        !fx.hub.views.claim_producer(0),
        "a second producer is refused"
    );
    assert!(!fx.hub.views.claim_producer(0));
    assert_eq!(fx.hub.views.producers(0), 1);
    for subscriber in [2, 3] {
        fx.hub.subscribe(&fx.owner, 0, subscriber, 3_000_000);
    }
    assert_eq!(sample(&fx), Disposition::Delivered);
    for subscriber in [1, 2, 3] {
        match fx.hub.views.take(subscriber) {
            Some(ViewMessage::Frame(frame)) => assert_eq!(frame.preview_sequence, 1),
            other => panic!("viewer {subscriber} got {other:?}"),
        }
    }
}

#[test]
fn a_slow_consumer_keeps_only_the_newest_frame_and_the_others_are_unaffected() {
    let fx = fixture(instant_script(), true);
    fx.hub.subscribe(&fx.owner, 0, 2, 3_000_000);
    for _ in 0..5 {
        assert_eq!(sample(&fx), Disposition::Delivered);
        // Viewer 2 takes every frame; viewer 1 takes none.
        assert!(matches!(fx.hub.views.take(2), Some(ViewMessage::Frame(_))));
    }
    match fx.hub.views.take(1) {
        Some(ViewMessage::Frame(frame)) => assert_eq!(frame.preview_sequence, 5),
        other => panic!("the slow viewer got {other:?}"),
    }
    assert_eq!(
        fx.hub.views.counts(1),
        Some((1, 4)),
        "four frames replaced, none queued"
    );
    assert_eq!(fx.hub.views.counts(2), Some((5, 0)));
}

#[test]
fn after_the_last_lease_ends_no_sample_is_granted_within_ttl_plus_the_check_period() {
    let fx = fixture(instant_script(), true);
    let renewed = fx.clock.now_us();
    assert_eq!(sample(&fx), Disposition::Delivered);
    let check = 500_000;
    let mut refused_at = None;
    for _ in 0..20 {
        fx.clock.sleep_us(check);
        fx.hub.sweep(&fx.owner, 0);
        if sample(&fx) == Disposition::SkippedNoLease {
            refused_at = Some(fx.clock.now_us());
            break;
        }
    }
    let refused_at = refused_at.expect("the lease ends");
    assert!(
        refused_at - renewed <= 3_000_000 + check,
        "after {} µs",
        refused_at - renewed
    );

    // An active unsubscribe ends the grants at once.
    let lease = fx.hub.subscribe(&fx.owner, 0, 9, 3_000_000).lease_id;
    assert_eq!(sample(&fx), Disposition::Delivered);
    fx.hub.unsubscribe(&fx.owner, 0, lease, 0);
    assert_eq!(sample(&fx), Disposition::SkippedNoLease);
}

#[test]
fn the_s_grace_is_reused_expires_and_never_waits_for_a_hard_gate() {
    let fx = fixture(instant_script(), true);
    fx.hub.unsubscribe(&fx.owner, 0, fx.lease, 5_000_000);
    assert_eq!(fx.hub.grace_expired(&fx.owner, 0), Some(false));
    fx.clock.sleep_us(1_000_000);
    let lease = fx.hub.subscribe(&fx.owner, 0, 2, 3_000_000).lease_id;
    fx.hub.unsubscribe(&fx.owner, 0, lease, 5_000_000);
    fx.clock.sleep_us(5_100_000);
    assert_eq!(fx.hub.grace_expired(&fx.owner, 0), Some(true));
    let lease = fx.hub.subscribe(&fx.owner, 0, 3, 3_000_000).lease_id;
    // A plan cannot stretch the grace beyond 10 s.
    fx.hub.unsubscribe(&fx.owner, 0, lease, 60_000_000);
    fx.hub.set_gate(&fx.owner, 0, GateKind::Blackout, true);
    assert!(
        fx.hub.end_grace_hard(&fx.owner, 0),
        "a hard gate ends the grace at once"
    );
    let grace = fx.hub.snapshot(&fx.owner, |state| state.instances[0].grace);
    assert_eq!(grace.entered, 3);
    assert_eq!(grace.reused, 1);
    assert_eq!(grace.expired_closed, 1);
    assert_eq!(grace.hard_closed, 1);
}

/// Steps a sample to just after its native call returned (barrier g5).
fn sample_to_g5(fx: &hub_fixture::Fixture) -> SampleOp {
    let mut op = SampleOp::new(0, PathKind::Nemu, fx.owner.clone());
    while op.barrier() != "g5" {
        match op.step(&fx.hub, Mode::NonBlocking) {
            Step::Progress => {}
            other => panic!("the sample stopped early: {other:?}"),
        }
    }
    op
}

fn finish(fx: &hub_fixture::Fixture, mut op: SampleOp) -> Disposition {
    loop {
        if let Step::Done(outcome) = op.step(&fx.hub, Mode::NonBlocking) {
            return outcome.disposition;
        }
    }
}

#[test]
fn a_generation_change_after_the_grant_discards_the_sample() {
    let fx = fixture(instant_script(), false);
    let op = sample_to_g5(&fx);
    let other = actingcommand_capture_bench::locks::LockOwner::new("reopen");
    fx.hub.bump_generation(&other, 0);
    assert_eq!(
        finish(&fx, op),
        Disposition::DiscardedStale("generation".to_owned())
    );
    assert!(fx.hub.violations(&fx.owner).is_empty());
    assert!(
        fx.sink
            .records()
            .iter()
            .any(|record| matches!(record.kind, RecordKind::Discard { .. }))
    );
}

#[test]
fn a_gate_after_the_grant_discards_the_sample_and_withdraws_the_picture() {
    let fx = fixture(instant_script(), false);
    let op = sample_to_g5(&fx);
    let other = actingcommand_capture_bench::locks::LockOwner::new("gate");
    fx.hub.set_gate(&other, 0, GateKind::Fence, true);
    assert!(matches!(
        fx.hub.views.take(1),
        Some(ViewMessage::Withdrawn { .. })
    ));
    assert_eq!(
        finish(&fx, op),
        Disposition::DiscardedStale("gate:fence".to_owned())
    );
    assert!(fx.hub.violations(&fx.owner).is_empty());
}

#[test]
fn every_result_has_its_exit_code_and_the_highest_priority_decides() {
    let codes = [
        (RunResult::Completed, 0),
        (RunResult::Failed, 10),
        (RunResult::Incomplete, 11),
        (RunResult::Aborted, 12),
        (RunResult::RefusedStart, 13),
        (RunResult::Unconfirmed, 20),
        (RunResult::StdioUnrestored, 21),
        (RunResult::InternalInvariant, 30),
    ];
    for (result, code) in codes {
        assert_eq!(result.exit_code(), code, "{result:?}");
    }
    // 21 > 20 > 30 > 10 > 12 > 11 > 0.
    let order = [
        RunResult::StdioUnrestored,
        RunResult::Unconfirmed,
        RunResult::InternalInvariant,
        RunResult::Failed,
        RunResult::Aborted,
        RunResult::Incomplete,
        RunResult::Completed,
    ];
    for (index, higher) in order.iter().enumerate() {
        for lower in &order[index..] {
            assert_eq!(RunResult::decisive(&[*lower, *higher]), *higher);
        }
    }
    assert_eq!(RunResult::decisive(&[]), RunResult::Completed);
}

#[test]
fn table_b_reopens_only_the_three_start_timeouts() {
    use OpenFailureCause::*;
    for cause in [ConnectTimeout, HeaderTimeout, FirstFrameTimeout] {
        assert_eq!(open_failure_handling(cause), OpenFailureHandling::Reopen);
        assert!(cause.reopenable());
    }
    for cause in [
        LocalCopy,
        WmSize,
        Push,
        Forward,
        ShellSpawn,
        ServerExitedBeforeAccept,
        ProtocolFirstByteOrCodec,
        SocketOrReader,
        StreamEndedBeforeFirstFrame,
        NemuOpen,
    ] {
        assert_eq!(
            open_failure_handling(cause),
            OpenFailureHandling::CellPath,
            "{cause:?}"
        );
        assert!(!cause.reopenable());
    }
    assert_eq!(
        open_failure_handling(ConfigInvalid),
        OpenFailureHandling::RefuseStart
    );
    assert_eq!(
        open_failure_handling(ServerDigestMismatch),
        OpenFailureHandling::PathNotReady
    );
    assert_eq!(
        open_failure_handling(CropAlignment),
        OpenFailureHandling::PathNotReady
    );
    assert_eq!(
        open_failure_handling(DeviceVerify),
        OpenFailureHandling::CellInstance
    );
    assert_eq!(
        open_failure_handling(CleanupUnconfirmed),
        OpenFailureHandling::WholeUnconfirmed
    );
}
