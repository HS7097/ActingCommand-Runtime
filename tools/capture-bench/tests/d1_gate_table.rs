// SPDX-License-Identifier: AGPL-3.0-only

//! D1 item 2 (plan v2.1 §3.7.1): one test per cell of the device entry × gate table. Each sets
//! the gate and drives the entry: E1, E4-E9 at the hub, E2 and E3 (the producers' closes) in a
//! whole run with a scripted gate.

mod common;
mod hub_fixture;

use actingcommand_capture_bench::fake::{FakeCheckScript, FakeScript};
use actingcommand_capture_bench::gates::reaction;
use actingcommand_capture_bench::hub::{
    DispatchOp, DispatchOutcome, ReopenOutcome, SampleOp, auto_reopen, aux_call, manual_refresh,
    run_blocking,
};
use actingcommand_capture_bench::model::{Disposition, Entry, GateKind, PathKind};
use actingcommand_capture_bench::views::ViewMessage;
use hub_fixture::{fixture, instant_script};

fn hub_entry(entry: Entry, gate: GateKind) {
    let expected = reaction(entry, gate);
    let fx = fixture(instant_script(), true);
    fx.hub.set_gate(&fx.owner, 0, gate, true);
    let refused = Disposition::SkippedGate(gate.as_str().to_owned());
    match entry {
        Entry::PreviewSample => {
            let withdrawn = matches!(fx.hub.views.take(1), Some(ViewMessage::Withdrawn { .. }));
            assert_eq!(
                withdrawn,
                expected.withdraws(),
                "{gate:?}: the picture is withdrawn"
            );
            let mut op = SampleOp::new(0, PathKind::Nemu, fx.owner.clone());
            let disposition = run_blocking(&fx.hub, &mut op).disposition;
            if expected.admits(false) {
                assert_eq!(disposition, Disposition::Delivered, "{gate:?}");
            } else {
                assert_eq!(disposition, refused, "{gate:?}");
            }
        }
        Entry::ManualRefresh => {
            let result = manual_refresh(&fx.hub, &fx.owner, 0, PathKind::Nemu);
            if expected.admits(false) {
                assert!(result.is_ok(), "{gate:?}: {result:?}");
            } else {
                assert_eq!(result, Err(refused), "{gate:?}");
            }
        }
        Entry::Calibration | Entry::ApkControl | Entry::DeviceSideCheck => {
            let result = aux_call(&fx.hub, &fx.owner, 0, entry, false, || 1);
            if expected.admits(false) {
                assert_eq!(result, Ok(1), "{entry:?} {gate:?}");
            } else {
                assert_eq!(result, Err(gate), "{entry:?} {gate:?}");
            }
            // Inside a close chain the device-side check goes where the table says so.
            let in_chain = aux_call(&fx.hub, &fx.owner, 0, entry, true, || 2);
            assert_eq!(
                in_chain.is_ok(),
                expected.admits(true),
                "{entry:?} {gate:?} in a close chain"
            );
        }
        Entry::FormalLoop => {
            let mut op = DispatchOp::new(0, fx.owner.clone(), fx.hub.clock.clone(), None);
            let outcome = run_blocking(&fx.hub, &mut op);
            match outcome {
                DispatchOutcome::Refused { gate: by } => {
                    assert!(!expected.admits(false), "{gate:?} refused");
                    assert_eq!(by, gate);
                }
                DispatchOutcome::Granted { .. } => {
                    assert!(expected.admits(false), "{gate:?} granted")
                }
                other => panic!("{gate:?}: {other:?}"),
            }
        }
        Entry::AutoReopen => {
            let generation = fx
                .hub
                .snapshot(&fx.owner, |state| state.instances[0].generation);
            let outcome = auto_reopen(&fx.hub, &fx.owner, 0, generation, |_| Ok(()));
            if expected.admits(false) {
                assert_eq!(
                    outcome,
                    ReopenOutcome::Reopened {
                        generation: generation + 1
                    },
                    "{gate:?}"
                );
            } else {
                assert_eq!(outcome, ReopenOutcome::Refused(gate), "{gate:?}");
            }
        }
        Entry::ScrcpyProducer | Entry::NemuSession => unreachable!("a run-level entry"),
    }
    assert!(fx.hub.violations(&fx.owner).is_empty());
}

/// E2 and E3 in a run: an S preview with the formal loop on N, the gate at 1 s for 1 s. A gate
/// whose cell says 关 closes the producer with the reason `gate:<gate>`; any other leaves it.
fn run_entry(entry: Entry, gate: GateKind) {
    let expected = reaction(entry, gate);
    let name = format!("gate-{}-{}", entry.as_str(), gate.as_str());
    let h = common::harness(
        &name,
        "gate",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let segment = format!(
        "instance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 3000\nformal = {{ period_ms = 500 }}\ngates = [{{ at_ms = 1000, gate = \"{}\", hold_ms = 1000 }}]",
        gate.as_str()
    );
    let outcome = h.run(common::plan(
        "gate",
        36_000_000,
        &common::cell("C10", 60_000, &[&segment]),
    ));
    assert_ne!(outcome.exit_code, 13, "{outcome:?}");
    let backend = match entry {
        Entry::ScrcpyProducer => "S",
        _ => "N",
    };
    let reason = format!("gate:{}", gate.as_str());
    let closed = h.events().iter().any(|event| {
        event["type"] == "closing" && event["backend"] == backend && event["reason"] == reason
    });
    assert_eq!(closed, expected.closes(), "{entry:?} under {gate:?}");
}

macro_rules! hub_cells {
    ($($name:ident: $entry:ident, $gate:ident;)*) => {
        $(
            #[test]
            fn $name() {
                hub_entry(Entry::$entry, GateKind::$gate);
            }
        )*
    };
}

macro_rules! run_cells {
    ($($name:ident: $entry:ident, $gate:ident;)*) => {
        $(
            #[test]
            fn $name() {
                run_entry(Entry::$entry, GateKind::$gate);
            }
        )*
    };
}

hub_cells! {
    e1_preview_sample_blackout: PreviewSample, Blackout;
    e1_preview_sample_fence: PreviewSample, Fence;
    e1_preview_sample_hold_request: PreviewSample, HoldRequest;
    e1_preview_sample_holding: PreviewSample, Holding;
    e1_preview_sample_stop_class: PreviewSample, StopClass;
    e1_preview_sample_routine_pause: PreviewSample, RoutinePause;
    e4_manual_refresh_blackout: ManualRefresh, Blackout;
    e4_manual_refresh_fence: ManualRefresh, Fence;
    e4_manual_refresh_hold_request: ManualRefresh, HoldRequest;
    e4_manual_refresh_holding: ManualRefresh, Holding;
    e4_manual_refresh_stop_class: ManualRefresh, StopClass;
    e4_manual_refresh_routine_pause: ManualRefresh, RoutinePause;
    e5_calibration_blackout: Calibration, Blackout;
    e5_calibration_fence: Calibration, Fence;
    e5_calibration_hold_request: Calibration, HoldRequest;
    e5_calibration_holding: Calibration, Holding;
    e5_calibration_stop_class: Calibration, StopClass;
    e5_calibration_routine_pause: Calibration, RoutinePause;
    e6_apk_control_blackout: ApkControl, Blackout;
    e6_apk_control_fence: ApkControl, Fence;
    e6_apk_control_hold_request: ApkControl, HoldRequest;
    e6_apk_control_holding: ApkControl, Holding;
    e6_apk_control_stop_class: ApkControl, StopClass;
    e6_apk_control_routine_pause: ApkControl, RoutinePause;
    e7_formal_loop_blackout: FormalLoop, Blackout;
    e7_formal_loop_fence: FormalLoop, Fence;
    e7_formal_loop_hold_request: FormalLoop, HoldRequest;
    e7_formal_loop_holding: FormalLoop, Holding;
    e7_formal_loop_stop_class: FormalLoop, StopClass;
    e7_formal_loop_routine_pause: FormalLoop, RoutinePause;
    e8_auto_reopen_blackout: AutoReopen, Blackout;
    e8_auto_reopen_fence: AutoReopen, Fence;
    e8_auto_reopen_hold_request: AutoReopen, HoldRequest;
    e8_auto_reopen_holding: AutoReopen, Holding;
    e8_auto_reopen_stop_class: AutoReopen, StopClass;
    e8_auto_reopen_routine_pause: AutoReopen, RoutinePause;
    e9_device_side_check_blackout: DeviceSideCheck, Blackout;
    e9_device_side_check_fence: DeviceSideCheck, Fence;
    e9_device_side_check_hold_request: DeviceSideCheck, HoldRequest;
    e9_device_side_check_holding: DeviceSideCheck, Holding;
    e9_device_side_check_stop_class: DeviceSideCheck, StopClass;
    e9_device_side_check_routine_pause: DeviceSideCheck, RoutinePause;
}

run_cells! {
    e2_scrcpy_producer_blackout: ScrcpyProducer, Blackout;
    e2_scrcpy_producer_fence: ScrcpyProducer, Fence;
    e2_scrcpy_producer_hold_request: ScrcpyProducer, HoldRequest;
    e2_scrcpy_producer_holding: ScrcpyProducer, Holding;
    e2_scrcpy_producer_stop_class: ScrcpyProducer, StopClass;
    e2_scrcpy_producer_routine_pause: ScrcpyProducer, RoutinePause;
    e3_nemu_session_blackout: NemuSession, Blackout;
    e3_nemu_session_fence: NemuSession, Fence;
    e3_nemu_session_hold_request: NemuSession, HoldRequest;
    e3_nemu_session_holding: NemuSession, Holding;
    e3_nemu_session_stop_class: NemuSession, StopClass;
    e3_nemu_session_routine_pause: NemuSession, RoutinePause;
}
