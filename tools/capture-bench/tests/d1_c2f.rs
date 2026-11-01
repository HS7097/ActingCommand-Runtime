// SPDX-License-Identifier: AGPL-3.0-only

//! D1 (plan v2.1 §6.2) for what D2b found, on the fake backend under the virtual clock (no real
//! clock, no sleep): the fake APKs at the real display sizes, so that portrait decoding, the step
//! trials, C4 and C6 run on E2, and the band the real APK draws fits both orientations; each fake
//! APK's own `apk_instance`; `viewer_write_failed`'s `expected` once a stop has begun;
//! `heartbeat.json`'s `working_set_bytes`; what the supervisor's end writes into `summary.json`
//! and `closing.json`.

mod common;
mod hub_fixture;

use actingcommand_capture_bench::apk::{ApkClient, ApkConnector, FakeApks, geometry_of_info};
use actingcommand_capture_bench::clock::{Clock, VirtualClock};
use actingcommand_capture_bench::fake::{
    CloseHang, FakeCheckScript, FakeDevice, FakeScript, FaultAt, Latch, SampleFault, fake_apks,
};
use actingcommand_capture_bench::journal::Journal;
use actingcommand_capture_bench::metrics::MetricsSample;
use actingcommand_capture_bench::model::{BackendKind, GateKind, GlobalGate};
use actingcommand_capture_bench::plan::Plan;
use actingcommand_capture_bench::timecode::{BandGeometry, DecodeStatus, Payload, decode, render};
use actingcommand_capture_bench::viewer::feed_viewer;
use actingcommand_capture_bench::views::ViewMessage;
use common::{at, cell, harness, limits, plan, preview, run_cell_result};
use hub_fixture::{Fixture, fixture, instant_script};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const HOURS: u64 = 3_600_000;

const E1_APK: &str = "[[instances]]\nname = \"E1\"\nserial = \"127.0.0.1:16544\"\nnemu_instance = 5\norientation = \"landscape\"\ntouch = \"adb_input\"\napk = true\n";
const E2_APK: &str = "[[instances]]\nname = \"E2\"\nserial = \"127.0.0.1:16576\"\nnemu_instance = 6\norientation = \"portrait\"\ntouch = \"adb_input\"\napk = true\n";

/// The plan's `[apk]` as `run --fake` plans have it: no cell, so the real APK's default, 24.
const APK_TABLE: &str = "[apk]\npings = 20\n";

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

/// A plan with the given tables after `[safety]`, the instances and the cells.
fn plan_with(run_id: &str, tables: &str, instances: &str, cells: &str) -> String {
    format!(
        r#"run_id = "{run_id}"
until_unix_ms = {until}
total_duration_ms = 36000000
window = "D1"

[safety]
adb_path = 'C:\fake\adb.exe'
adb_version = "37.0.1-15733141"
mumu_folder = 'C:\fake\MuMu'
priority_class = "normal"
serial_whitelist = ["127.0.0.1:16544", "127.0.0.1:16576"]
nemu_whitelist = [5, 6]

{tables}

{instances}

{cells}
"#,
        until = common::UNIX_ORIGIN_MS + 10 * HOURS,
    )
}

/// A harness with the fake APKs `run --fake` gives this plan (the same function, the real sizes).
fn run_fake_harness(name: &str, plan_text: &str) -> common::Harness {
    let h0 = harness(
        name,
        name,
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let parsed = Plan::parse(plan_text).expect("the plan parses");
    let set = fake_apks(&h0.dyn_clock(), &parsed);
    let script = FakeScript {
        patterns: set.patterns,
        ..FakeScript::default()
    };
    common::Harness {
        device: FakeDevice::new(h0.dyn_clock(), script, FakeCheckScript::default()),
        apks: set.apks,
        ..h0
    }
}

fn records(h: &common::Harness, file: &str, kind: &str) -> Vec<Value> {
    h.lines(file)
        .into_iter()
        .filter(|line| line["kind"] == kind)
        .collect()
}

fn of_type(events: &[Value], kind: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event["type"] == kind)
        .cloned()
        .collect()
}

// ---- the real APK's band, against PROTOCOL §4.2 ----

#[test]
fn the_real_apks_band_fits_and_decodes_the_same_way_in_both_orientations() {
    let payload = Payload {
        version: 1,
        mode: 3,
        seq: 0x0102_0304,
        t_vsync_dev_ns: 9_000_000_000,
        event: 7,
        aux: 0x08,
    };
    for (width, height) in [(1280_u32, 720_u32), (720, 1280)] {
        for bytes_per_pixel in [3_usize, 4] {
            // INFO's view offset: none, and one a status bar would give.
            for (offset_x, offset_y) in [(0_u32, 0_u32), (0, 48)] {
                let geometry = BandGeometry {
                    cell: 24,
                    offset_x,
                    offset_y,
                };
                assert!(geometry.fits(width, height), "{width} x {height}");
                // PROTOCOL §4.2's table: the band is [24, 24, 600, 216) on both screens.
                assert_eq!(
                    geometry.cell_rect(0, 0),
                    (offset_x + 24, offset_y + 24, offset_x + 48, offset_y + 48)
                );
                assert_eq!(
                    geometry.cell_rect(7, 23),
                    (
                        offset_x + 576,
                        offset_y + 192,
                        offset_x + 600,
                        offset_y + 216
                    )
                );
                let mut pixels = vec![128_u8; (width * height) as usize * bytes_per_pixel];
                render(&mut pixels, width, bytes_per_pixel, geometry, &payload);
                let decoded = decode(&pixels, width, height, bytes_per_pixel, geometry);
                assert_eq!(decoded.status, DecodeStatus::Ok, "{width} x {height}");
                assert_eq!(decoded.payload, Some(payload));
            }
        }
    }
    // D2's fake display: the band of 600 px does not fit in 360 px.
    let narrow_frame = vec![128_u8; 360 * 640 * 4];
    let narrow = BandGeometry::default();
    assert!(!narrow.fits(360, 640));
    assert_eq!(
        decode(&narrow_frame, 360, 640, 4, narrow).status,
        DecodeStatus::OutOfFrame
    );
}

#[test]
fn the_geometry_comes_from_info_the_same_way_in_both_orientations() {
    let expected = BandGeometry {
        cell: 24,
        offset_x: 0,
        offset_y: 48,
    };
    for (width, height) in [(1280, 720), (720, 1280)] {
        // PROTOCOL §5: `layout.cell`, and `view.x`, `view.y`.
        let info = json!({
            "cell": 24,
            "layout": { "width": width, "height": height, "cell": 24, "band": [24, 24, 600, 216] },
            "view": { "width": width, "height": height, "x": 0, "y": 48 },
        });
        assert_eq!(geometry_of_info(&info), Some(expected));
    }
}

// ---- the fake APKs of `run --fake` ----

#[test]
fn run_fake_apks_have_a_hello_instance_each_also_after_a_restart() {
    let text = plan_with(
        "instances",
        APK_TABLE,
        &format!("{E1_APK}\n{E2_APK}"),
        &cell("CA", 60_000, &[&preview("N", 1_000)]),
    );
    let parsed = Plan::parse(&text).expect("the plan parses");
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(common::UNIX_ORIGIN_MS));
    let set = fake_apks(&clock, &parsed);
    let connector = FakeApks {
        apks: set.apks.clone(),
    };
    let hello = |instance: &str| {
        let link = connector.connect(instance).expect("a link");
        ApkClient::new(link, Arc::clone(&clock))
            .hello()
            .expect("HELLO")
            .instance
    };
    assert_eq!(hello("E1"), "fa01000000000001");
    assert_eq!(hello("E2"), "fa02000000000001");
    set.apks["E2"].restart(false);
    assert_eq!(hello("E2"), "fa02000000000002");
    assert_eq!(hello("E1"), "fa01000000000001");
}

#[test]
fn e1_and_e2_calib_records_carry_different_apk_instances() {
    let cells = cell(
        "K",
        600_000,
        &[
            "instance = \"E1\"\nmode = \"calibrate\"\nreps = 1\nquota = 10",
            "instance = \"E2\"\nmode = \"calibrate\"\nreps = 1\nquota = 10",
        ],
    );
    let text = plan_with("calib", APK_TABLE, &format!("{E1_APK}\n{E2_APK}"), &cells);
    let h = run_fake_harness("calib", &text);
    let outcome = h.run(text);
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let calib = of_type(&h.lines("calib-D1.jsonl"), "calib");
    assert_eq!(calib.len(), 2, "{calib:?}");
    for record in &calib {
        let expected = match record["instance"].as_str() {
            Some("E1") => "fa01000000000001",
            Some("E2") => "fa02000000000001",
            other => panic!("an instance outside the plan: {other:?}"),
        };
        assert_eq!(record["apk_instance"], expected, "{record}");
    }
}

#[test]
fn portrait_decoding_step_trials_c4_and_c6_run_on_e2_at_the_real_size() {
    let segments = |path: &str, mode: &str| {
        format!(
            "instance = \"E2\"\npath = \"{path}\"\nmode = \"preview\"\nduration_ms = 1000\napk_mode = \"{mode}\""
        )
    };
    let calibrate = "instance = \"E2\"\nmode = \"calibrate\"\nreps = 1\nquota = 10";
    let cells = cell(
        "C1",
        600_000,
        &[
            calibrate,
            "instance = \"E2\"\npath = \"S\"\nmode = \"step_trials\"\nreps = 3",
        ],
    ) + &cell(
        "K",
        600_000,
        &[
            calibrate,
            "instance = \"E2\"\npath = \"S\"\nmode = \"step_trials\"\nreps = 2",
        ],
    ) + &cell(
        "C3",
        600_000,
        &[&segments("N", "pattern"), &segments("S", "pattern")],
    ) + &cell(
        "C4",
        600_000,
        &[&segments("N", "static"), &segments("S", "static")],
    ) + &cell(
        "C6",
        600_000,
        &[
            "instance = \"E2\"\npath = \"N\"\nmode = \"task_floor\"\nquota = 3\napk_mode = \"flip\"",
            "instance = \"E2\"\npath = \"S\"\nmode = \"task_floor\"\nquota = 3\napk_mode = \"flip\"",
        ],
    );
    let text = plan_with(
        "portrait",
        APK_TABLE,
        &format!("{E1_APK}\n{E2_APK}"),
        &cells,
    );
    let h = run_fake_harness("portrait", &text);
    let outcome = h.run(text);
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let apk = &h.apks["E2"];

    // C3: every delivered frame decodes, and shows E2's own APK at the frame's source time.
    for file in ["opps-C3-N.jsonl", "opps-C3-S.jsonl"] {
        let delivered: Vec<Value> = records(&h, file, "preview")
            .into_iter()
            .filter(|record| record["disposition"] == "delivered")
            .collect();
        assert!(!delivered.is_empty(), "{file}");
        for record in &delivered {
            assert_eq!(record["orientation"], "P", "{record}");
            let content = &record["content"];
            assert_eq!(content["decoded"], true, "{record}");
            assert_eq!(content["status"], "ok", "{record}");
            let vsync = content["t_vsync_dev_ns"].as_i64().expect("t_vsync_dev_ns");
            let source = record["known_source_us"].as_u64().expect("known_source_us");
            let at_source = apk.device_ns_at(source);
            assert!(
                vsync <= at_source && at_source - vsync < apk.frame_period_ns,
                "{record}"
            );
        }
    }

    // C4: the still picture has no code, as on the real APK; the frames fit, they just show none.
    for file in ["opps-C4-N.jsonl", "opps-C4-S.jsonl"] {
        let delivered: Vec<Value> = records(&h, file, "preview")
            .into_iter()
            .filter(|record| record["disposition"] == "delivered")
            .collect();
        assert!(!delivered.is_empty(), "{file}");
        for record in &delivered {
            assert_ne!(record["content"]["status"], "out_of_frame", "{record}");
        }
    }

    // C1 and K: the step is seen on every trial; D2b's E2 saw none of 15.
    let arrivals = of_type(&h.lines("calib-D1.jsonl"), "step_arrival");
    assert_eq!(arrivals.len(), 5, "{arrivals:?}");
    for arrival in &arrivals {
        assert_eq!(arrival["instance"], "E2", "{arrival}");
        assert!(arrival["t_recv_us"].is_u64(), "{arrival}");
    }

    // C6: the taps are mapped on 720 x 1280, the frames decode, and the flip seen is the tap's.
    for file in ["opps-C6-N.jsonl", "opps-C6-S.jsonl"] {
        let admissions = records(&h, file, "admission");
        assert_eq!(admissions.len(), 3, "{file}");
        for admission in &admissions {
            assert_eq!(admission["tap"]["display_w"], 720, "{admission}");
            assert_eq!(admission["tap"]["display_h"], 1280, "{admission}");
            assert_eq!(admission["content"]["status"], "ok", "{admission}");
            assert_eq!(
                admission["first_flip"]["j"], admission["tap"]["j"],
                "{admission}"
            );
        }
    }
}

// ---- viewer_write_failed ----

/// A pipe that takes `writes_left` writes and then fails with `os_error`.
struct ClosingPipe {
    writes_left: usize,
    os_error: i32,
}

impl Write for ClosingPipe {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.writes_left == 0 {
            return Err(std::io::Error::from_raw_os_error(self.os_error));
        }
        self.writes_left -= 1;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The record of one failed viewer write: `before` sets the scene on the hub, then the writer
/// takes a `cell_ended` from its mailbox and its pipe fails.
fn write_failure(
    name: &str,
    stopping: bool,
    os_error: i32,
    writes_left: usize,
    before: impl FnOnce(&Fixture),
) -> Value {
    const SUBSCRIBER: u64 = 56;
    let fx = fixture(instant_script(), true);
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "capture-bench-d1/c2f-{name}-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let clock: Arc<dyn Clock> = fx.clock.clone();
    let journal = Journal::open(&root, clock).expect("the journal opens");
    fx.hub.views.open_mailbox(SUBSCRIBER, 0);
    before(&fx);
    fx.hub.views.broadcast(Some(0), ViewMessage::CellEnded);
    let mut pipe = ClosingPipe {
        writes_left,
        os_error,
    };
    feed_viewer(
        &mut pipe,
        &fx.hub,
        &journal,
        0,
        SUBSCRIBER,
        &json!({ "op": "lease" }),
        &AtomicBool::new(stopping),
    );
    journal.flush();
    let text = std::fs::read_to_string(root.join("events.jsonl")).expect("events.jsonl");
    let failures: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
        .filter(|event| event["type"] == "viewer_write_failed")
        .collect();
    assert_eq!(failures.len(), 1, "{name}: {text}");
    failures[0].clone()
}

#[test]
fn a_closed_pipe_after_any_stop_gate_is_expected_with_or_without_the_viewers_own_stop() {
    let gates: [(&str, fn(&Fixture)); 3] = [
        ("run-stop", |fx: &Fixture| {
            fx.hub.set_global(&fx.owner, GlobalGate::RunStop, true);
        }),
        ("cell-end", |fx: &Fixture| {
            fx.hub.set_global(&fx.owner, GlobalGate::CellEnd, true);
        }),
        ("instance-stop", |fx: &Fixture| {
            fx.hub.set_gate(&fx.owner, 0, GateKind::StopClass, true);
        }),
    ];
    for (name, gate) in gates {
        // os error 232 is what D2b's STOP and hang-N runs recorded; 109 is the other closed pipe.
        for os_error in [232, 109] {
            let record = write_failure(name, false, os_error, 1, gate);
            assert_eq!(record["expected"], true, "{name}: {record}");
            assert_eq!(
                record["cause"], "pipe_closed_by_the_viewer_stop",
                "{record}"
            );
            assert_eq!(record["os_error"], os_error, "{record}");
            assert_eq!(record["viewer_id"], 56, "{record}");
        }
    }
    // The viewer's own stop, with no gate in force, as before.
    let own = write_failure("own-stop", true, 232, 1, |_| {});
    assert_eq!(own["expected"], true, "{own}");
}

#[test]
fn a_write_failure_inside_a_segment_stays_a_real_failure() {
    // No gate at all: after the hello (writes_left 1) and at the hello (writes_left 0).
    for writes_left in [1, 0] {
        let record = write_failure("segment", false, 232, writes_left, |_| {});
        assert_eq!(record["expected"], false, "{record}");
        assert_eq!(
            record["cause"], "write_failed_during_the_segment",
            "{record}"
        );
    }
    // A blackout is no stop: the viewer's child goes on, so its pipe has no reason to close.
    let blackout = write_failure("blackout", false, 232, 1, |fx: &Fixture| {
        fx.hub.set_gate(&fx.owner, 0, GateKind::Blackout, true);
    });
    assert_eq!(blackout["expected"], false, "{blackout}");
    // After a stop gate, a failure that is not a closed pipe is still not expected.
    let other = write_failure("other", false, 5, 1, |fx: &Fixture| {
        fx.hub.set_global(&fx.owner, GlobalGate::RunStop, true);
    });
    assert_eq!(other["expected"], false, "{other}");
    assert_eq!(other["cause"], "write_failed_while_stopping", "{other}");
}

// ---- heartbeat.json ----

#[test]
fn the_heartbeat_shows_the_newest_working_set_on_a_run_of_one_segment() {
    const BYTES: u64 = 17_500_000;
    let h = harness(
        "heartbeat",
        "heartbeat",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    // One segment: `status` is written once, before any sample.
    let run = h.spawn(
        plan(
            "heartbeat",
            10 * HOURS,
            &cell("CA", 60_000, &[&preview("N", 4_000)]),
        ),
        limits(),
        vec![MetricsSample {
            own_working_set_bytes: Some(BYTES),
            ..MetricsSample::default()
        }],
    );
    let outcome = run.join().expect("the run");
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    assert_eq!(
        h.json("heartbeat.json")["run"]["working_set_bytes"],
        BYTES,
        "the host file has it: {:?}",
        h.lines("host-CA.jsonl").first()
    );
}

// ---- the supervisor's end ----

fn hang_at(instance: &str, kind: BackendKind) -> (CloseHang, Arc<Latch>) {
    let latch = Latch::new();
    (
        CloseHang {
            at: FaultAt {
                instance: instance.to_owned(),
                kind,
                from_call: 1,
                count: 1,
            },
            latch: Arc::clone(&latch),
            on_entry: None,
        },
        latch,
    )
}

fn with_script(h0: common::Harness, script: FakeScript) -> common::Harness {
    common::Harness {
        device: FakeDevice::new(h0.dyn_clock(), script, FakeCheckScript::default()),
        ..h0
    }
}

fn texts(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a list")
        .iter()
        .map(|text| text.as_str().expect("a text").to_owned())
        .collect()
}

#[test]
fn the_supervisors_summary_lists_the_close_the_unconfirmed_and_every_cell() {
    let h0 = harness(
        "end-cells",
        "end-cells",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let (close_hang, latch) = hang_at("E1", BackendKind::Nemu);
    let h = with_script(
        h0,
        FakeScript {
            close_hangs: vec![close_hang],
            ..FakeScript::default()
        },
    );
    // CA ends normally, CB's N close never returns, CC is never opened.
    let cells = cell("CA", 60_000, &[&preview("S", 1_000)])
        + &cell("CB", 60_000, &[&preview("N", 1_000)])
        + &cell("CC", 60_000, &[&preview("S", 1_000)]);
    let run = h.spawn(plan("end-cells", 10 * HOURS, &cells), limits(), Vec::new());
    assert_eq!(h.exit.wait_first(), 20);
    latch.open();
    assert_eq!(run.join().expect("the run").exit_code, 20);

    let summary = h.json("summary.json");
    assert_eq!(summary["result"], "unconfirmed", "{summary}");
    assert_eq!(summary["exit_code"], 20, "{summary}");
    assert_eq!(summary["forced_by"], "close_not_returned", "{summary}");
    assert!(
        texts(&summary["results"]).contains(&"unconfirmed".to_owned()),
        "{summary}"
    );

    assert_eq!(run_cell_result(&summary, "CA", "S").0, "cell_completed");
    assert_eq!(
        run_cell_result(&summary, "CB", "N"),
        (
            "cell_incomplete".to_owned(),
            "close_not_returned".to_owned()
        )
    );
    assert_eq!(
        run_cell_result(&summary, "CC", "S"),
        (
            "untested".to_owned(),
            "not started: close_not_returned".to_owned()
        )
    );

    let closes = summary["closes"].as_array().expect("closes");
    assert_eq!(closes.len(), 2, "{closes:?}");
    assert_eq!(closes[0]["backend"], "S", "{closes:?}");
    assert_eq!(closes[0]["confirmed"], true, "{closes:?}");
    let hung = &closes[1];
    assert_eq!(hung["instance"], "E1", "{hung}");
    assert_eq!(hung["backend"], "N", "{hung}");
    assert_eq!(hung["reason"], "segment_end", "{hung}");
    assert_eq!(hung["confirmed"], false, "{hung}");
    assert_eq!(
        hung["host_side"],
        json!({ "close": "unconfirmed", "detail": "close_not_returned" }),
        "{hung}"
    );

    assert_eq!(
        texts(&summary["unconfirmed"]),
        vec!["E1 N close 2: close_not_returned; the close thread is still running".to_owned()]
    );

    // closing.json still lists it, as the supervisor declared it.
    let closing = h.json("closing.json");
    let closing = closing.as_array().expect("closing.json");
    assert_eq!(closing.len(), 1, "{closing:?}");
    assert_eq!(closing[0]["close_id"], 2, "{closing:?}");
    assert_eq!(closing[0]["backend"], "N", "{closing:?}");
    assert_eq!(closing[0]["state"], "declared_unconfirmed", "{closing:?}");
}

#[test]
fn the_supervisors_summary_keeps_the_stop_gate_and_says_s_had_no_device_side_check() {
    let h0 = harness(
        "end-stop",
        "end-stop",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let (close_hang, latch) = hang_at("E1", BackendKind::Scrcpy);
    let stop = h0.run_dir.join("STOP");
    let write_stop: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        std::fs::write(&stop, b"").expect("write the control file");
    });
    let h = with_script(
        h0,
        FakeScript {
            close_hangs: vec![close_hang],
            on_sample: vec![(at(BackendKind::Scrcpy, 4, 1), write_stop)],
            ..FakeScript::default()
        },
    );
    let run = h.spawn(
        plan(
            "end-stop",
            10 * HOURS,
            &cell("CA", 60_000, &[&preview("S", 10_000)]),
        ),
        limits(),
        Vec::new(),
    );
    assert_eq!(h.exit.wait_first(), 20);
    latch.open();
    assert_eq!(run.join().expect("the run").exit_code, 20);
    let summary = h.json("summary.json");
    let stops = of_type(&h.events(), "run_stop");
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert_eq!(
        summary["t_stop_new_us"], stops[0]["t_stop_new_us"],
        "{summary}"
    );
    assert_eq!(
        summary["run_stop_gate_seq"], stops[0]["gate_seq"],
        "{summary}"
    );
    assert_eq!(
        run_cell_result(&summary, "CA", "S"),
        (
            "cell_incomplete".to_owned(),
            "close_not_returned".to_owned()
        )
    );
    let closes = summary["closes"].as_array().expect("closes");
    assert_eq!(closes.len(), 1, "{closes:?}");
    assert_eq!(closes[0]["confirmed"], false, "{closes:?}");
    assert_eq!(closes[0]["device_side"]["checked"], false, "{closes:?}");
    assert!(
        closes[0]["null_reasons"]["device_side.gone_within_ms"].is_string(),
        "{closes:?}"
    );
}

#[test]
fn the_supervisors_summary_keeps_a_fatal_error_of_the_cell_in_progress() {
    let h0 = harness(
        "end-fatal",
        "end-fatal",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let (close_hang, latch) = hang_at("E1", BackendKind::Nemu);
    let h = with_script(
        h0,
        FakeScript {
            close_hangs: vec![close_hang],
            sample_faults: vec![(at(BackendKind::Nemu, 2, 1), SampleFault::PixelReturnNonZero)],
            ..FakeScript::default()
        },
    );
    let run = h.spawn(
        plan(
            "end-fatal",
            10 * HOURS,
            &cell("CA", 60_000, &[&preview("N", 5_000)]),
        ),
        limits(),
        Vec::new(),
    );
    assert_eq!(h.exit.wait_first(), 20);
    latch.open();
    assert_eq!(run.join().expect("the run").exit_code, 20);
    assert_eq!(
        run_cell_result(&h.json("summary.json"), "CA", "N"),
        (
            "cell_failed".to_owned(),
            "nemu_capture_return_nonzero".to_owned()
        )
    );
}

#[test]
fn the_supervisors_summary_is_on_disk_before_the_emergency_closes_and_lists_them_after() {
    let h0 = harness(
        "end-emergency",
        "end-emergency",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    // A segment with the formal loop has S and N open and closes S first: S hangs, so N is left
    // for the supervisor's emergency close; no call is in flight then, so it always gets it. At
    // the entry of that close the summary is read from disk, as a hang there would leave it.
    let (s_hang, s_latch) = hang_at("E1", BackendKind::Scrcpy);
    let (n_hang, n_latch) = hang_at("E1", BackendKind::Nemu);
    let on_disk: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let on_entry: Arc<dyn Fn() + Send + Sync> = {
        let on_disk = Arc::clone(&on_disk);
        let n_latch = Arc::clone(&n_latch);
        let path = h0.run_dir.join("summary.json");
        Arc::new(move || {
            *on_disk.lock().expect("the note") = std::fs::read_to_string(&path).ok();
            n_latch.open();
        })
    };
    let n_hang = CloseHang {
        on_entry: Some(on_entry),
        ..n_hang
    };
    let h = with_script(
        h0,
        FakeScript {
            close_hangs: vec![s_hang, n_hang],
            ..FakeScript::default()
        },
    );
    let segment = "instance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 1000\n\
        formal = { quota = 2 }";
    let run = h.spawn(
        plan("end-emergency", 10 * HOURS, &cell("CA", 60_000, &[segment])),
        limits(),
        Vec::new(),
    );
    assert_eq!(h.exit.wait_first(), 20);
    s_latch.open();
    assert_eq!(run.join().expect("the run").exit_code, 20);

    let before = on_disk.lock().expect("the note").clone();
    let before: Value =
        serde_json::from_str(&before.expect("summary.json before the emergency close"))
            .expect("a JSON file");
    assert_eq!(before["result"], "unconfirmed", "{before}");
    assert_eq!(
        before["closes"].as_array().map(Vec::len),
        Some(1),
        "{before}"
    );
    assert_eq!(texts(&before["unconfirmed"]).len(), 1, "{before}");

    let summary = h.json("summary.json");
    let unconfirmed = texts(&summary["unconfirmed"]);
    assert_eq!(unconfirmed.len(), 2, "{unconfirmed:?}");
    assert!(
        unconfirmed[0].starts_with("E1 S close 1: "),
        "{unconfirmed:?}"
    );
    assert!(
        unconfirmed[1].starts_with("E1 N emergency close: "),
        "{unconfirmed:?}"
    );
    let closes = summary["closes"].as_array().expect("closes");
    assert_eq!(closes.len(), 2, "{closes:?}");
    assert_eq!(closes[0]["backend"], "S", "{closes:?}");
    assert_eq!(closes[1]["backend"], "N", "{closes:?}");
    assert_eq!(closes[1]["reason"], "emergency", "{closes:?}");
    assert_eq!(closes[1]["confirmed"], false, "{closes:?}");
}
