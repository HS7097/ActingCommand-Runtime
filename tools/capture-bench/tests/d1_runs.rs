// SPDX-License-Identifier: AGPL-3.0-only

//! D1 (plan v2.1 §6.2) items 3, 6 and 7 and the secretary's constraint: whole runs on the fake
//! backend under the virtual clock — cells and their ends, the whole-run stop and its order, the
//! results and exit codes, the error classes of §3.11.4 tables A and B, the close that never
//! returns, and the device-side close confirmation of §3.5.1.

mod common;

use actingcommand_capture_bench::backend::{IdentityMethod, OpenFailureCause};
use actingcommand_capture_bench::fake::{
    CloseHang, FakeCall, FakeCheckScript, FakeScript, Latch, SampleFault,
};
use actingcommand_capture_bench::metrics::MetricsSample;
use actingcommand_capture_bench::model::{BackendKind, RunResult};
use actingcommand_capture_bench::safety::{BenchLock, RefuseStartChecks};
use common::{at, cell, cell_result, harness, limits, plan, position, preview, run_cell_result};
use serde_json::Value;
use std::sync::Arc;

const HOURS: u64 = 3_600_000;

fn write_file(dir: &std::path::Path, name: &str) -> Arc<dyn Fn() + Send + Sync> {
    let path = dir.join(name);
    Arc::new(move || {
        std::fs::write(&path, b"").expect("write the control file");
    })
}

fn summary_cell(harness: &common::Harness, cell: &str) -> Value {
    harness.json(&format!("summary-{cell}.json"))
}

#[test]
fn d1_6_1_first_cell_completes_then_the_second_runs_without_a_run_stop_gate_between() {
    let h = harness(
        "two-cells",
        "two-cells",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let cells =
        cell("CA", 60_000, &[&preview("N", 3_000)]) + &cell("CB", 60_000, &[&preview("S", 3_000)]);
    let outcome = h.run(plan("two-cells", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    assert_eq!(outcome.result, RunResult::Completed);
    assert_eq!(
        cell_result(&summary_cell(&h, "CA"), "N").0,
        "cell_completed"
    );
    assert_eq!(
        cell_result(&summary_cell(&h, "CB"), "S").0,
        "cell_completed"
    );
    let events = h.events();
    let second = position(&events, "cell_start", |event| event["cell"] == "CB").expect("CB starts");
    let first_end =
        position(&events, "gate", |event| event["gate"] == "cell_end").expect("a cell-end gate");
    assert!(
        first_end < second,
        "the first cell ends before the second starts"
    );
    assert!(
        position(&events, "gate", |event| event["gate"] == "stop").is_none_or(|stop| stop > second),
        "no whole-run stop gate before the second cell"
    );
    assert!(
        events[first_end..second]
            .iter()
            .any(|event| event["type"] == "generation"),
        "the generation moves between the cells"
    );
    // Every close was announced on disk first and confirmed.
    for (index, event) in events.iter().enumerate() {
        if event["type"] == "closed" {
            let opened = position(&events, "closing", |closing| {
                closing["close_id"] == event["close_id"]
            })
            .expect("a closing record");
            assert!(opened < index);
            assert_eq!(event["result"], "Confirmed");
        }
    }
    assert_eq!(h.json("summary.json")["result"], "completed");
}

#[test]
fn d1_6_2_a_slow_first_cell_ends_at_its_limit_quota_short_and_the_second_still_runs() {
    let h = harness(
        "quota-short",
        "quota-short",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let slow = "instance = \"E1\"\npath = \"N\"\nmode = \"preview\"\nquota = 1000000";
    let cells = cell("CA", 2_000, &[slow]) + &cell("CB", 60_000, &[&preview("N", 2_000)]);
    let outcome = h.run(plan("quota-short", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 11, "{outcome:?}");
    let (result, reason) = cell_result(&summary_cell(&h, "CA"), "N");
    assert_eq!(result, "cell_incomplete");
    assert!(reason.starts_with("quota_short"), "{reason}");
    assert_eq!(
        cell_result(&summary_cell(&h, "CB"), "N").0,
        "cell_completed"
    );
    let events = h.events();
    let start = position(&events, "cell_start", |event| event["cell"] == "CA").expect("CA starts");
    let stop_new = position(&events, "segment_stop_new", |event| {
        event["segment"]
            .as_str()
            .is_some_and(|segment| segment.starts_with("CA"))
    })
    .expect("the cell's stop of new grants");
    let elapsed =
        events[stop_new]["t_us"].as_u64().unwrap() - events[start]["t_us"].as_u64().unwrap();
    // Ended at the limit, within one controller period; not extended, not rerun.
    assert!(
        (2_000_000..=2_100_000).contains(&elapsed),
        "ended after {elapsed} µs"
    );
    assert!(position(&events, "cell_start", |event| event["cell"] == "CB").is_some());
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "cell_start" && event["cell"] == "CA")
            .count(),
        1
    );
}

fn stop_mid_first_cell(
    file: &str,
) -> (
    common::Harness,
    actingcommand_capture_bench::runner::RunOutcome,
) {
    let mut script = FakeScript::default();
    let h0 = harness(
        &format!("control-{file}"),
        "control",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    script
        .on_sample
        .push((at(BackendKind::Nemu, 5, 1), write_file(&h0.run_dir, file)));
    let h = common::Harness {
        device: actingcommand_capture_bench::fake::FakeDevice::new(
            h0.dyn_clock(),
            script,
            FakeCheckScript::default(),
        ),
        ..h0
    };
    let cells =
        cell("CA", 60_000, &[&preview("N", 10_000)]) + &cell("CB", 60_000, &[&preview("N", 2_000)]);
    let outcome = h.run(plan("control", 10 * HOURS, &cells));
    (h, outcome)
}

#[test]
fn d1_6_3_stop_in_the_first_cell_stops_the_whole_run_in_the_five_steps() {
    let (h, outcome) = stop_mid_first_cell("STOP");
    assert_eq!(outcome.exit_code, 11, "{outcome:?}");
    let summary = h.json("summary.json");
    assert_eq!(summary["result"], "incomplete");
    let (result, reason) = run_cell_result(&summary, "CA", "N");
    assert_eq!(result, "cell_incomplete");
    assert_eq!(reason, "stop");
    assert_eq!(run_cell_result(&summary, "CB", "N").0, "untested");
    let events = h.events();
    let control =
        position(&events, "control_file", |event| event["file"] == "STOP").expect("STOP seen");
    let gate =
        position(&events, "gate", |event| event["gate"] == "stop").expect("the run-stop gate");
    let closing = position(&events, "closing", |_| true).expect("the close is announced");
    let closed = position(&events, "closed", |_| true).expect("the close is confirmed");
    let terminal = position(&events, "terminal", |_| true).expect("the summary is written");
    // 1 stop new → 2/3 drain and close (announced first) → 5 summary.
    assert!(control < gate && gate < closing && closing < closed && closed < terminal);
    assert!(position(&events, "cell_start", |event| event["cell"] == "CB").is_none());
}

#[test]
fn d1_6_3_abort_does_the_same_steps_and_exits_12() {
    let (h, outcome) = stop_mid_first_cell("ABORT");
    assert_eq!(outcome.exit_code, 12, "{outcome:?}");
    assert_eq!(h.json("summary.json")["result"], "aborted");
}

#[test]
fn d1_6_3_the_global_deadline_leaves_the_next_cell_untested() {
    let h = harness(
        "deadline",
        "deadline",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let cells =
        cell("CA", 3_000, &[&preview("N", 2_000)]) + &cell("CB", 3_000, &[&preview("N", 2_000)]);
    let outcome = h.run(plan("deadline", 5_000, &cells));
    assert_eq!(outcome.exit_code, 11, "{outcome:?}");
    let summary = h.json("summary.json");
    assert_eq!(summary["stop_cause"]["cause"], "deadline");
    assert_eq!(run_cell_result(&summary, "CA", "N").0, "cell_completed");
    let (result, reason) = run_cell_result(&summary, "CB", "N");
    assert_eq!(result, "untested");
    assert!(reason.contains("deadline"), "{reason}");
}

#[test]
fn d1_6_3_the_bench_working_set_cap_stops_the_run_and_no_host_memory_gate_does() {
    let h = harness(
        "working-set",
        "working-set",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let low_host = MetricsSample {
        own_working_set_bytes: Some(100 << 20),
        host_available_bytes: Some(1 << 30),
        ..MetricsSample::default()
    };
    let over_cap = MetricsSample {
        own_working_set_bytes: Some(1_100 << 20),
        host_available_bytes: Some(1 << 30),
        ..MetricsSample::default()
    };
    let cells = cell("CA", 60_000, &[&preview("N", 10_000)]);
    let samples = vec![low_host.clone(), low_host.clone(), low_host, over_cap];
    let outcome = h
        .spawn(plan("working-set", 10 * HOURS, &cells), limits(), samples)
        .join()
        .expect("the run");
    assert_eq!(outcome.exit_code, 11, "{outcome:?}");
    let summary = h.json("summary.json");
    // Ruling 141: a host with 1 GiB available is no stop; only the bench's own cap is.
    assert_eq!(summary["stop_cause"]["cause"], "working_set_cap");
    assert_eq!(summary["stop_causes"].as_array().map(Vec::len), Some(1));
}

#[test]
fn d1_3_a_stdio_restore_failure_stops_the_run_stdio_unrestored_21() {
    let mut script = FakeScript::default();
    script
        .sample_faults
        .push((at(BackendKind::Nemu, 4, 1), SampleFault::StdioRestoreFailed));
    let h = harness(
        "stdio-restore",
        "stdio-restore",
        script,
        FakeCheckScript::default(),
    );
    let outcome = h.run(plan(
        "stdio-restore",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 5_000)]),
    ));
    assert_eq!(outcome.exit_code, 21, "{outcome:?}");
    assert_eq!(h.json("summary.json")["result"], "stdio_unrestored");
}

#[test]
fn d1_6_4_a_nemu_nonzero_return_fails_n_and_b_model_and_leaves_s_running() {
    let mut script = FakeScript::default();
    script
        .sample_faults
        .push((at(BackendKind::Nemu, 3, 1), SampleFault::PixelReturnNonZero));
    let h = harness(
        "nemu-fatal",
        "nemu-fatal",
        script,
        FakeCheckScript::default(),
    );
    let cells = cell(
        "CA",
        60_000,
        &[
            &preview("N", 2_000),
            &preview("S", 2_000),
            &preview("B_model", 2_000),
        ],
    ) + &cell("CB", 60_000, &[&preview("S", 2_000)])
        + &cell("CC", 60_000, &[&preview("N", 2_000)]);
    let outcome = h.run(plan("nemu-fatal", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 10, "{outcome:?}");
    let summary = h.json("summary.json");
    let (n, code) = run_cell_result(&summary, "CA", "N");
    assert_eq!(n, "cell_failed");
    assert_eq!(code, "nemu_capture_return_nonzero");
    assert_eq!(run_cell_result(&summary, "CA", "B_model").0, "cell_failed");
    assert_eq!(run_cell_result(&summary, "CA", "S").0, "cell_completed");
    assert_eq!(run_cell_result(&summary, "CB", "S").0, "cell_completed");
    let (later, reason) = run_cell_result(&summary, "CC", "N");
    assert_eq!(later, "untested");
    assert!(reason.contains("prior fatal"), "{reason}");
    // No sample on the same backend after the fatal one, and the backend never reopened.
    let calls = h.device.calls();
    let nemu_samples: Vec<u64> = calls
        .iter()
        .filter_map(|call| match call {
            FakeCall::Sample {
                kind: BackendKind::Nemu,
                call,
                after_close,
                ..
            } => {
                assert!(!after_close, "a sample after the close");
                Some(*call)
            }
            _ => None,
        })
        .collect();
    assert_eq!(nemu_samples.iter().max(), Some(&3));
    let nemu_opens = calls
        .iter()
        .filter(|call| {
            matches!(
                call,
                FakeCall::Open {
                    kind: BackendKind::Nemu,
                    ..
                }
            )
        })
        .count();
    assert_eq!(nemu_opens, 1);
    assert!(calls.iter().any(|call| matches!(
        call,
        FakeCall::Close {
            kind: BackendKind::Nemu,
            ..
        }
    )));
}

#[test]
fn d1_3_a_probe_nonzero_and_a_size_change_also_stop_only_the_path() {
    for fault in [
        SampleFault::ProbeReturnNonZero,
        SampleFault::SizeChangedDuringCall,
    ] {
        let mut script = FakeScript::default();
        script
            .sample_faults
            .push((at(BackendKind::Nemu, 2, 1), fault));
        let h = harness(
            "nemu-fatal-kinds",
            "nemu-fatal-kinds",
            script,
            FakeCheckScript::default(),
        );
        let cells = cell("CA", 60_000, &[&preview("N", 2_000), &preview("S", 2_000)]);
        let outcome = h.run(plan("nemu-fatal-kinds", 10 * HOURS, &cells));
        assert_eq!(outcome.exit_code, 10, "{fault:?}: {outcome:?}");
        let summary = h.json("summary.json");
        assert_eq!(run_cell_result(&summary, "CA", "N").0, "cell_failed");
        assert_eq!(run_cell_result(&summary, "CA", "S").0, "cell_completed");
    }
}

#[test]
fn d1_3_a_poisoned_stops_the_whole_run_unconfirmed() {
    let mut script = FakeScript::default();
    script
        .sample_faults
        .push((at(BackendKind::Nemu, 3, 1), SampleFault::Poisoned));
    let h = harness("poisoned", "poisoned", script, FakeCheckScript::default());
    let cells =
        cell("CA", 60_000, &[&preview("N", 5_000)]) + &cell("CB", 60_000, &[&preview("S", 2_000)]);
    let outcome = h.run(plan("poisoned", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    let summary = h.json("summary.json");
    assert_eq!(summary["result"], "unconfirmed");
    assert_eq!(run_cell_result(&summary, "CB", "S").0, "untested");
}

#[test]
fn d1_3_a_stdio_busy_three_times_in_a_row_goes_on_and_the_fourth_stops_the_path() {
    let mut script = FakeScript::default();
    script
        .sample_faults
        .push((at(BackendKind::Nemu, 2, 3), SampleFault::StdioBusy));
    let h = harness("busy-3", "busy-3", script, FakeCheckScript::default());
    let outcome = h.run(plan(
        "busy-3",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 3_000)]),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let busy = h
        .lines("opps-CA-N.jsonl")
        .iter()
        .filter(|line| line["disposition"] == "failed:vendor_stdio_redirect_busy")
        .count();
    assert_eq!(busy, 3, "every original failure is counted");

    let mut script = FakeScript::default();
    script
        .sample_faults
        .push((at(BackendKind::Nemu, 2, 4), SampleFault::StdioBusy));
    let h = harness("busy-4", "busy-4", script, FakeCheckScript::default());
    let outcome = h.run(plan(
        "busy-4",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 3_000)]),
    ));
    assert_eq!(outcome.exit_code, 10, "{outcome:?}");
    assert_eq!(
        run_cell_result(&h.json("summary.json"), "CA", "N").0,
        "cell_failed"
    );
}

#[test]
fn secretary_constraint_s_started_without_a_confirmed_identity_makes_the_whole_run_unconfirmed() {
    let check = FakeCheckScript {
        positive_fails: vec!["E1".to_owned()],
        ..FakeCheckScript::default()
    };
    let h = harness("identity", "identity", FakeScript::default(), check);
    let cells =
        cell("CA", 60_000, &[&preview("S", 3_000)]) + &cell("CB", 60_000, &[&preview("N", 2_000)]);
    let outcome = h.run(plan("identity", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    let summary = h.json("summary.json");
    assert_eq!(summary["result"], "unconfirmed");
    assert!(
        !summary["unconfirmed"]
            .as_array()
            .expect("the list")
            .is_empty()
    );
    // Never a conditional skip, and no sampling without the positive check.
    assert_ne!(run_cell_result(&summary, "CA", "S").0, "conditional_skip");
    assert_eq!(run_cell_result(&summary, "CB", "N").0, "untested");
    let calls = h.device.calls();
    assert!(!calls.iter().any(|call| matches!(
        call,
        FakeCall::Sample {
            kind: BackendKind::Scrcpy,
            ..
        }
    )));
    assert!(calls.iter().any(|call| matches!(
        call,
        FakeCall::Close {
            kind: BackendKind::Scrcpy,
            ..
        }
    )));
    let events = h.events();
    assert!(position(&events, "scrcpy_identity_failed", |_| true).is_some());
    assert!(
        position(&events, "closing", |event| event["reason"]
            == "identity_unconfirmed")
        .is_some()
    );
}

#[test]
fn a_device_side_disappearance_that_cannot_be_confirmed_is_unconfirmed() {
    for check in [
        FakeCheckScript {
            gone_unconfirmed: vec!["E1".to_owned()],
            ..FakeCheckScript::default()
        },
        FakeCheckScript {
            gone_residual: vec!["E1".to_owned()],
            ..FakeCheckScript::default()
        },
    ] {
        let h = harness("gone", "gone", FakeScript::default(), check);
        let outcome = h.run(plan(
            "gone",
            10 * HOURS,
            &cell("CA", 60_000, &[&preview("S", 2_000)]),
        ));
        assert_eq!(outcome.exit_code, 20, "{outcome:?}");
        assert_eq!(h.json("summary.json")["result"], "unconfirmed");
    }
}

#[test]
fn s_is_not_ready_only_when_no_method_works_before_anything_starts() {
    let check = FakeCheckScript {
        negative_fails: IdentityMethod::CANDIDATES.to_vec(),
        ..FakeCheckScript::default()
    };
    let h = harness("not-ready", "not-ready", FakeScript::default(), check);
    let cells = cell("CA", 60_000, &[&preview("S", 2_000), &preview("N", 2_000)]);
    let outcome = h.run(plan("not-ready", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let summary = h.json("summary.json");
    assert_eq!(run_cell_result(&summary, "CA", "S").0, "conditional_skip");
    assert_eq!(run_cell_result(&summary, "CA", "N").0, "cell_completed");
    assert!(!h.device.calls().iter().any(|call| matches!(
        call,
        FakeCall::Open {
            kind: BackendKind::Scrcpy,
            ..
        }
    )));
}

fn s_open_faults(
    cause: OpenFailureCause,
    count: u64,
    cleanup: bool,
) -> (
    common::Harness,
    actingcommand_capture_bench::runner::RunOutcome,
) {
    let mut script = FakeScript::default();
    script
        .open_faults
        .push((at(BackendKind::Scrcpy, 1, count), cause, cleanup));
    let h = harness(
        &format!("open-{}", cause.as_str()),
        "open",
        script,
        FakeCheckScript::default(),
    );
    let outcome = h.run(plan(
        "open",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("S", 2_000)]),
    ));
    (h, outcome)
}

fn scrcpy_opens(h: &common::Harness) -> Vec<Option<u32>> {
    h.device
        .calls()
        .iter()
        .filter_map(|call| match call {
            FakeCall::Open {
                kind: BackendKind::Scrcpy,
                scid,
                ..
            } => Some(*scid),
            _ => None,
        })
        .collect()
}

#[test]
fn d1_3_table_b_the_three_start_timeouts_reopen_at_most_twice_with_a_new_scid() {
    for cause in [
        OpenFailureCause::ConnectTimeout,
        OpenFailureCause::HeaderTimeout,
        OpenFailureCause::FirstFrameTimeout,
    ] {
        let (h, outcome) = s_open_faults(cause, 2, true);
        assert_eq!(outcome.exit_code, 0, "{cause:?}: {outcome:?}");
        let opens = scrcpy_opens(&h);
        assert_eq!(opens.len(), 3);
        let distinct: std::collections::BTreeSet<_> = opens.iter().collect();
        assert_eq!(distinct.len(), 3, "a new scid for every reopen");
        let absent = h
            .device
            .check_calls()
            .iter()
            .filter(|call| call.starts_with("absent:"))
            .count();
        assert_eq!(
            absent, 2,
            "the old server and forward are checked gone before each reopen"
        );

        let (h, outcome) = s_open_faults(cause, 3, true);
        assert_eq!(outcome.exit_code, 10, "{cause:?}: {outcome:?}");
        assert_eq!(
            scrcpy_opens(&h).len(),
            3,
            "the third failure stops the path"
        );
    }
}

#[test]
fn d1_3_table_b_other_open_failures_do_not_reopen_and_an_unconfirmed_cleanup_stops_the_run() {
    for cause in [
        OpenFailureCause::Push,
        OpenFailureCause::Forward,
        OpenFailureCause::ShellSpawn,
        OpenFailureCause::ServerExitedBeforeAccept,
        OpenFailureCause::ProtocolFirstByteOrCodec,
        OpenFailureCause::SocketOrReader,
        OpenFailureCause::StreamEndedBeforeFirstFrame,
        OpenFailureCause::WmSize,
        OpenFailureCause::LocalCopy,
        OpenFailureCause::DeviceVerify,
    ] {
        let (h, outcome) = s_open_faults(cause, 5, true);
        assert_eq!(outcome.exit_code, 10, "{cause:?}: {outcome:?}");
        assert_eq!(scrcpy_opens(&h).len(), 1, "{cause:?} is not reopened");
    }
    for cause in [
        OpenFailureCause::ServerDigestMismatch,
        OpenFailureCause::CropAlignment,
    ] {
        let (h, outcome) = s_open_faults(cause, 5, true);
        assert_eq!(outcome.exit_code, 0, "{cause:?}: {outcome:?}");
        assert_eq!(
            run_cell_result(&h.json("summary.json"), "CA", "S").0,
            "conditional_skip"
        );
    }
    let (h, outcome) = s_open_faults(OpenFailureCause::FirstFrameTimeout, 1, false);
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    assert_eq!(scrcpy_opens(&h).len(), 1);
}

#[test]
fn d1_3_table_b_a_failed_cold_start_counts_as_infinity_and_is_not_reopened() {
    let mut script = FakeScript::default();
    script.open_faults.push((
        at(BackendKind::Scrcpy, 2, 1),
        OpenFailureCause::FirstFrameTimeout,
        true,
    ));
    let h = harness("cold", "cold", script, FakeCheckScript::default());
    let cold = "instance = \"E1\"\npath = \"S\"\nmode = \"cold_start\"\nreps = 3";
    let outcome = h.run(plan("cold", 10 * HOURS, &cell("C2", 60_000, &[cold])));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    assert_eq!(scrcpy_opens(&h).len(), 3);
    let lines = h.lines("opps-C2-S.jsonl");
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1]["counted_as"], "infinity");
    assert!(lines[1]["t_first_frame_us"].is_null());
    assert_eq!(lines[0]["kind"], "cold_start");
}

fn hang(
    kind: BackendKind,
    on_entry: Option<Arc<dyn Fn() + Send + Sync>>,
) -> (CloseHang, Arc<Latch>) {
    let latch = Latch::new();
    (
        CloseHang {
            at: at(kind, 1, 1),
            latch: Arc::clone(&latch),
            on_entry,
        },
        latch,
    )
}

#[test]
fn d1_7_an_s_close_stuck_in_join_at_a_cell_end_is_recorded_unconfirmed_and_exits_20() {
    let h0 = harness(
        "hang-s",
        "hang-s",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let (close_hang, latch) = hang(BackendKind::Scrcpy, Some(write_file(&h0.run_dir, "STOP")));
    let script = FakeScript {
        close_hangs: vec![close_hang],
        ..FakeScript::default()
    };
    let h = common::Harness {
        device: actingcommand_capture_bench::fake::FakeDevice::new(
            h0.dyn_clock(),
            script,
            FakeCheckScript::default(),
        ),
        ..h0
    };
    let cells =
        cell("CA", 60_000, &[&preview("S", 2_000)]) + &cell("CB", 60_000, &[&preview("N", 2_000)]);
    let run = h.spawn(plan("hang-s", 10 * HOURS, &cells), limits(), Vec::new());
    assert_eq!(h.exit.wait_first(), 20, "the controlled self-exit");
    latch.open();
    let outcome = run.join().expect("the run");
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    let events = h.events();
    let closing =
        position(&events, "closing", |event| event["backend"] == "S").expect("closing first");
    let control =
        position(&events, "control_file", |event| event["file"] == "STOP").expect("STOP recorded");
    let unconfirmed = position(&events, "close_unconfirmed", |event| {
        event["reason"] == "close_not_returned"
    })
    .expect("close_not_returned");
    let exit = position(&events, "self_exit", |event| event["code"] == 20).expect("self_exit");
    assert!(closing < control && control < unconfirmed && unconfirmed < exit);
    assert_eq!(h.json("summary.json")["result"], "unconfirmed");
    let heartbeats = h.lines("heartbeat-closing.jsonl");
    assert!(
        heartbeats.len() >= 2,
        "the heartbeat goes on during the hang"
    );
    assert!(
        heartbeats
            .iter()
            .all(|line| line["closing"][0]["backend"] == "S")
    );
    assert!(position(&events, "cell_start", |event| event["cell"] == "CB").is_none());
}

#[test]
fn d1_7_an_n_close_that_never_returns_in_the_whole_run_stop_exits_20() {
    let h0 = harness(
        "hang-n",
        "hang-n",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let (close_hang, latch) = hang(BackendKind::Nemu, None);
    let mut script = FakeScript {
        close_hangs: vec![close_hang],
        ..FakeScript::default()
    };
    script
        .on_sample
        .push((at(BackendKind::Nemu, 4, 1), write_file(&h0.run_dir, "STOP")));
    let h = common::Harness {
        device: actingcommand_capture_bench::fake::FakeDevice::new(
            h0.dyn_clock(),
            script,
            FakeCheckScript::default(),
        ),
        ..h0
    };
    let run = h.spawn(
        plan(
            "hang-n",
            10 * HOURS,
            &cell("CA", 60_000, &[&preview("N", 10_000)]),
        ),
        limits(),
        Vec::new(),
    );
    assert_eq!(h.exit.wait_first(), 20);
    latch.open();
    let outcome = run.join().expect("the run");
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    let events = h.events();
    let gate =
        position(&events, "gate", |event| event["gate"] == "stop").expect("the run-stop gate");
    let closing = position(&events, "closing", |event| event["backend"] == "N").expect("closing");
    let unconfirmed = position(&events, "close_unconfirmed", |event| {
        event["reason"] == "close_not_returned"
    })
    .expect("close_not_returned");
    assert!(gate < closing && closing < unconfirmed);
}

#[test]
fn d1_7_with_self_exit_off_the_heartbeat_shows_terminal_pending_since() {
    let h0 = harness(
        "hang-pending",
        "hang-pending",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let (close_hang, latch) = hang(BackendKind::Scrcpy, None);
    let script = FakeScript {
        close_hangs: vec![close_hang],
        ..FakeScript::default()
    };
    let h = common::Harness {
        device: actingcommand_capture_bench::fake::FakeDevice::new(
            h0.dyn_clock(),
            script,
            FakeCheckScript::default(),
        ),
        ..h0
    };
    let mut no_exit = limits();
    no_exit.self_exit = false;
    let run = h.spawn(
        plan(
            "hang-pending",
            10 * HOURS,
            &cell("CA", 60_000, &[&preview("S", 2_000)]),
        ),
        no_exit,
        Vec::new(),
    );
    assert_eq!(h.exit.wait_first(), -1, "pending instead of an exit");
    let heartbeat = h.json("heartbeat.json");
    assert!(
        heartbeat["terminal_pending_since_us"].as_u64().is_some(),
        "{heartbeat}"
    );
    latch.open();
    let outcome = run.join().expect("the run");
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    assert!(
        h.exit.codes().iter().all(|code| *code == -1),
        "no self-exit"
    );
}

#[test]
fn a_second_bench_refuses_to_start_while_the_fixed_lock_is_held() {
    let h = harness(
        "lock-held",
        "lock-held",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let held = BenchLock::acquire(&h.roots.lock_path).expect("the first bench's lock");
    let outcome = h.run(plan(
        "lock-held",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 1_000)]),
    ));
    assert_eq!(outcome.exit_code, 13, "{outcome:?}");
    drop(held);
    let outcome = h.run(plan(
        "lock-held",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 1_000)]),
    ));
    assert_eq!(
        outcome.exit_code, 0,
        "the lock is free once the first holder ends: {outcome:?}"
    );
}

#[test]
fn refused_starts_touch_no_device_and_exit_13() {
    let h = harness(
        "refused",
        "refused",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let outcome = h
        .spawn_with(
            plan(
                "refused",
                10 * HOURS,
                &cell("CA", 60_000, &[&preview("N", 1_000)]),
            ),
            limits(),
            Vec::new(),
            Box::new(RefuseStartChecks("adb_version_mismatch: fake".to_owned())),
        )
        .join()
        .expect("the run");
    assert_eq!(outcome.exit_code, 13);
    assert_eq!(h.json("summary.json")["result"], "refused_start");
    // A deadline in the past, a serial or Nemu index outside the whitelist.
    let past = plan("refused", 0, &cell("CA", 60_000, &[&preview("N", 1_000)]));
    assert_eq!(h.run(past).exit_code, 13);
    let outside = plan(
        "refused",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 1_000)]),
    )
    .replace("nemu_instance = 5", "nemu_instance = 2");
    assert_eq!(h.run(outside).exit_code, 13);
    let serial = plan(
        "refused",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 1_000)]),
    )
    .replace(
        "serial = \"127.0.0.1:16544\"",
        "serial = \"127.0.0.1:16384\"",
    );
    assert_eq!(h.run(serial).exit_code, 13);
    assert!(
        h.device.calls().is_empty(),
        "no device call before the start check passes"
    );
}
