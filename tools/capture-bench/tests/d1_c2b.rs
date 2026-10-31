// SPDX-License-Identifier: AGPL-3.0-only

//! D1 (plan v2.1 §6.2) for the C2b additions, on the fake backend and the fake APK under the
//! virtual clock (no real clock, no sleep): C3's timecode v1 (its test vectors and decode
//! rules), the calibration from PING samples, the APK's record files per APK process and the
//! recalibration after a reset, the step trials, the C6 flip identity, the concurrent cell of C9,
//! C10's rotation and reconnect, C8's saved frames and coverage list, the stand-in `adb shell`
//! child, and the records `drain`, `producer_count`, `entry_gate`, `grace`, `s_open`,
//! `identity_positive`, `sampling_start`, `stdio_size`, `artifact_delta` and `vmm_handles`.

mod common;

use actingcommand_capture_bench::apk::{
    ApkStreams, Calibration, FakeApk, Hello, PingSample, apk_file_name, apk_raw_file_name,
    calibrate, residual_and_drift,
};
use actingcommand_capture_bench::fake::{
    FakeCheckScript, FakeDevice, FakePattern, FakeScript, RecordedChildren, SampleFault,
};
use actingcommand_capture_bench::metrics::ScriptedProbes;
use actingcommand_capture_bench::model::BackendKind;
use actingcommand_capture_bench::plan::Plan;
use actingcommand_capture_bench::timecode::{
    BandGeometry, DecodeStatus, Payload, cells, codeword, crc32, decode, render,
};
use actingcommand_capture_bench::winproc::VmmProcess;
use common::{at, cell, harness, plan, position, preview};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

const HOURS: u64 = 3_600_000;

const E1: &str = "[[instances]]\nname = \"E1\"\nserial = \"127.0.0.1:16544\"\nnemu_instance = 5\norientation = \"landscape\"\ntouch = \"adb_input\"\napk = true\n";
const E1_PLAIN: &str = "[[instances]]\nname = \"E1\"\nserial = \"127.0.0.1:16544\"\nnemu_instance = 5\norientation = \"landscape\"\ntouch = \"adb_input\"\n";
const E2_PLAIN: &str = "[[instances]]\nname = \"E2\"\nserial = \"127.0.0.1:16576\"\nnemu_instance = 6\norientation = \"portrait\"\ntouch = \"adb_input\"\n";

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

const APK_TABLE: &str = "[apk]\ncell = 8\npings = 20\n";

/// A harness whose fake frames show the fake APK's band (cell 8 on a 216 × 208 display).
fn apk_harness(
    name: &str,
    run_id: &str,
    configure: impl FnOnce(&mut FakeScript, &Arc<FakeApk>),
) -> (common::Harness, Arc<FakeApk>) {
    apk_harness_on(name, run_id, (216, 208), configure)
}

/// The same on a display of the given natural size.
fn apk_harness_on(
    name: &str,
    run_id: &str,
    display: (u32, u32),
    configure: impl FnOnce(&mut FakeScript, &Arc<FakeApk>),
) -> (common::Harness, Arc<FakeApk>) {
    let h0 = harness(
        name,
        run_id,
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let apk = FakeApk::new(h0.dyn_clock(), 7_000_000_000);
    apk.set_cell(8);
    apk.set_display(display.0, display.1);
    let mut script = FakeScript {
        pattern: Some(FakePattern {
            apk: Arc::clone(&apk),
            geometry: BandGeometry {
                cell: 8,
                offset_x: 0,
                offset_y: 0,
            },
        }),
        ..FakeScript::default()
    };
    configure(&mut script, &apk);
    let mut apks = BTreeMap::new();
    apks.insert("E1".to_owned(), Arc::clone(&apk));
    let h = common::Harness {
        device: FakeDevice::new(h0.dyn_clock(), script, FakeCheckScript::default()),
        apks,
        ..h0
    };
    (h, apk)
}

fn of_type<'a>(lines: &'a [Value], kind: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["type"] == kind).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn vector_a() -> Payload {
    Payload {
        version: 1,
        mode: 2,
        seq: 0x0102_0304,
        t_vsync_dev_ns: 0x1122_3344_5566_7788,
        event: 0x00ab_cdef,
        aux: 0x5a,
    }
}

#[test]
fn the_timecode_test_vectors_encode_to_the_protocol_codewords_and_decode_back() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    let vectors = [
        (vector_a(), "12010203041122334455667788abcdef5aca17e576"),
        (
            Payload {
                version: 1,
                mode: 1,
                seq: 1,
                t_vsync_dev_ns: 123_456_789_012_345,
                event: 0,
                aux: 0,
            },
            "110000000100007048860ddf7900000000f4a49f58",
        ),
        (
            Payload {
                version: 1,
                mode: 3,
                seq: 0xffff_ffff,
                t_vsync_dev_ns: i64::MAX,
                event: 0x00ff_ffff,
                aux: 0x08,
            },
            "13ffffffff7fffffffffffffffffffff081d32b654",
        ),
    ];
    for (payload, codeword_hex) in vectors {
        assert_eq!(hex(&codeword(&payload)), codeword_hex);
        for bytes_per_pixel in [3_usize, 4] {
            let (width, height) = (640_u32, 240_u32);
            let mut pixels = vec![128_u8; (width * height) as usize * bytes_per_pixel];
            render(
                &mut pixels,
                width,
                bytes_per_pixel,
                BandGeometry::default(),
                &payload,
            );
            let decoded = decode(
                &pixels,
                width,
                height,
                bytes_per_pixel,
                BandGeometry::default(),
            );
            assert_eq!(decoded.status, DecodeStatus::Ok);
            assert_eq!(decoded.payload, Some(payload));
            assert!(decoded.margin.is_some_and(|margin| margin > 100.0));
        }
    }
    let rows = [
        "101100111000111100001111",
        "000100100000000100000010",
        "000000110000010000010001",
        "001000100011001101000100",
        "010101010110011001110111",
        "100010001010101111001101",
        "111011110101101011001010",
        "000101111110010101110110",
    ];
    let grid = cells(&vector_a());
    for (row, expected) in rows.iter().enumerate() {
        let text: String = grid[row]
            .iter()
            .map(|white| if *white { '1' } else { '0' })
            .collect();
        assert_eq!(&text, expected, "row {row} of vector A");
    }
}

#[test]
fn the_decoder_follows_the_protocol_rules_for_shifts_and_bad_codes() {
    let (width, height) = (640_u32, 240_u32);
    let frame = |payload: &Payload, geometry: BandGeometry| {
        let mut pixels = vec![128_u8; (width * height) as usize * 4];
        render(&mut pixels, width, 4, geometry, payload);
        pixels
    };
    let at_origin = BandGeometry::default();
    // A misregistration of a few pixels stays out of the sample squares.
    let shifted = frame(
        &vector_a(),
        BandGeometry {
            cell: 24,
            offset_x: 3,
            offset_y: 2,
        },
    );
    assert_eq!(
        decode(&shifted, width, height, 4, at_origin).payload,
        Some(vector_a())
    );
    // No band: no sync.
    let blank = vec![128_u8; (width * height) as usize * 4];
    assert_eq!(
        decode(&blank, width, height, 4, at_origin).status,
        DecodeStatus::NoSync
    );
    // One data cell flipped: the CRC does not match.
    let mut flipped = frame(&vector_a(), at_origin);
    let (left, top, right, bottom) = at_origin.cell_rect(1, 0);
    for y in top..bottom {
        for x in left..right {
            let index = (y * width + x) as usize * 4;
            flipped[index] = 255 - flipped[index];
            flipped[index + 1] = 255 - flipped[index + 1];
            flipped[index + 2] = 255 - flipped[index + 2];
        }
    }
    assert_eq!(
        decode(&flipped, width, height, 4, at_origin).status,
        DecodeStatus::CrcMismatch
    );
    let version_two = Payload {
        version: 2,
        ..vector_a()
    };
    assert_eq!(
        decode(&frame(&version_two, at_origin), width, height, 4, at_origin).status,
        DecodeStatus::UnsupportedVersion
    );
    let mode_five = Payload {
        mode: 5,
        ..vector_a()
    };
    assert_eq!(
        decode(&frame(&mode_five, at_origin), width, height, 4, at_origin).status,
        DecodeStatus::BadMode
    );
    // A frame too small for the band.
    assert_eq!(
        decode(&blank, 320, 120, 4, at_origin).status,
        DecodeStatus::OutOfFrame
    );
}

#[test]
fn a_calibration_takes_the_minimum_round_trip_in_microseconds() {
    let theta = 5_000.0;
    let sample = |sent: u64, received: u64, asymmetry: f64| {
        let device_us = (sent + received) as f64 / 2.0 + theta + asymmetry;
        let device_ns = (device_us * 1_000.0) as i64;
        PingSample {
            sent_us: sent,
            received_us: received,
            t_rx_dev_ns: device_ns - 500,
            t_tx_dev_ns: device_ns + 500,
        }
    };
    let samples = [sample(1_000, 1_040, 0.0), sample(2_000, 2_010, 3.0)];
    let calibration = calibrate(&samples).expect("a calibration");
    assert_eq!(calibration.rtt_min_us, 10);
    assert!((calibration.theta_us - 5_003.0).abs() < 1e-6);
    assert!((calibration.u_us - 5.0).abs() < 1e-9);
    assert!((calibration.t_dev_us - 7_008.0).abs() < 1e-6);
    assert_eq!(calibration.samples, 2);
    let (lo, hi) = calibration.map_dev_ns(7_008_000);
    assert!((lo - 2_000.0).abs() < 1e-6 && (hi - 2_010.0).abs() < 1e-6);
    assert!(calibrate(&[]).is_none());

    let at = |t_dev_us: f64, theta_us: f64| Calibration {
        t_dev_us,
        t_host_us: t_dev_us - theta_us,
        theta_us,
        u_us: 5.0,
        rtt_min_us: 10,
        samples: 200,
    };
    let previous = [at(1_000_000.0, 5_000.0), at(2_000_000.0, 5_010.0)];
    let on_line = at(3_000_000.0, 5_020.0);
    let (residual, drift) = residual_and_drift(&previous, &on_line);
    assert!(residual.abs() < 1e-9);
    assert!(drift.is_some_and(|ppm| (ppm - 10.0).abs() < 1e-9));
    let (residual, _) = residual_and_drift(&previous, &at(3_000_000.0, 5_027.0));
    assert!((residual - 7.0).abs() < 1e-9);
    assert_eq!(residual_and_drift(&previous[..1], &on_line).0, 0.0);
}

#[test]
fn each_apk_process_has_its_own_epoch_and_a_reset_device_clock_its_own_clock_epoch() {
    let hello = |instance: &str, t_now_dev_ns: i64| Hello {
        instance: instance.to_owned(),
        t_now_dev_ns,
        raw: json!({}),
    };
    let calibration = Calibration {
        t_dev_us: 7_000_000.0,
        t_host_us: 0.0,
        theta_us: 7_000_000.0,
        u_us: 1.0,
        rtt_min_us: 2,
        samples: 200,
    };
    let mut streams = ApkStreams::default();
    let first = streams.on_hello(&hello("a", 7_000_000_000), 0, None);
    assert!(!first.apk_restarted && !first.clock_reset);
    assert_eq!((streams.epoch, streams.clock_epoch), (1, 1));
    streams.vsync_after = 5;
    streams.event_after = 3;
    // The same process and clock.
    let same = streams.on_hello(&hello("a", 8_000_000_000), 1_000_000, Some(&calibration));
    assert!(!same.apk_restarted && !same.clock_reset);
    assert_eq!((streams.epoch, streams.vsync_after), (1, 5));
    // The app restarted: a new epoch, the cursors start over, the clock is the same.
    let restarted = streams.on_hello(&hello("b", 8_500_000_000), 1_500_000, Some(&calibration));
    assert!(restarted.apk_restarted && !restarted.clock_reset);
    assert_eq!(
        (streams.epoch, streams.vsync_after, streams.event_after),
        (2, 0, 0)
    );
    // The instance restarted: the device clock starts again.
    let rebooted = streams.on_hello(&hello("c", 2_000_000_000), 2_000_000, Some(&calibration));
    assert!(rebooted.apk_restarted && rebooted.clock_reset);
    assert_eq!((streams.epoch, streams.clock_epoch), (3, 2));
    assert_eq!(streams.marks()["apk_instance"], "c");
    assert_eq!(apk_file_name("E1", 1), "apk-E1.jsonl");
    assert_eq!(apk_file_name("E1", 3), "apk-E1.e3.jsonl");
    assert_eq!(apk_raw_file_name("E1"), "apkraw-E1.jsonl");
}

#[test]
fn c1_calibrations_and_step_trials_write_the_calibration_file_and_the_apk_records() {
    let (h, _apk) = apk_harness("c1", "c1", |_, _| {});
    let cells = cell(
        "C1",
        600_000,
        &[
            "instance = \"E1\"\nmode = \"calibrate\"\nreps = 2\nquota = 50",
            "instance = \"E1\"\npath = \"S\"\nmode = \"step_trials\"\nreps = 3",
        ],
    );
    let outcome = h.run(plan_with("c1", APK_TABLE, E1, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let calib = h.lines("calib-D1.jsonl");
    let calibrations = of_type(&calib, "calib");
    assert_eq!(calibrations.len(), 2);
    for calibration in &calibrations {
        assert_eq!(calibration["source"], "C1");
        assert_eq!(calibration["n"], 50);
        // device_ns = host_us × 1000 + 7 s, read 1 µs apart: θ is 7 s plus half a µs.
        let theta = calibration["theta_us"].as_f64().expect("theta_us");
        assert!((theta - 7_000_000.5).abs() < 1.0, "theta {theta}");
        assert_eq!(calibration["clock_epoch"], 1);
        assert_eq!(calibration["apk_epoch"], 1);
    }
    assert_eq!(of_type(&calib, "roundtrip").len(), 100);
    let arrivals = of_type(&calib, "step_arrival");
    assert_eq!(arrivals.len(), 3);
    for (index, arrival) in arrivals.iter().enumerate() {
        assert_eq!(arrival["k"], index as u64 + 1);
        assert_eq!(arrival["source"], "C1");
        assert!(arrival["t_recv_us"].is_u64(), "{arrival}");
    }
    let records = h.lines("apk-E1.jsonl");
    assert!(
        records
            .iter()
            .all(|record| matches!(record["type"].as_str(), Some("flip" | "step" | "vsync")))
    );
    let steps: Vec<u64> = of_type(&records, "step")
        .iter()
        .filter_map(|step| step["k"].as_u64())
        .collect();
    assert_eq!(steps, vec![1, 2, 3]);
    assert!(
        of_type(&records, "step")
            .iter()
            .all(|step| step["t_vsync_dev_ns"].is_i64() && step["apk_epoch"] == 1)
    );
    let raw = h.lines("apkraw-E1.jsonl");
    assert!(!of_type(&raw, "mode").is_empty());
}

#[test]
fn an_apk_restart_starts_a_new_record_file_and_a_reboot_a_new_calibration() {
    let (h, _apk) = apk_harness("restart", "restart", |script, apk| {
        let restarted = Arc::clone(apk);
        let action: Arc<dyn Fn() + Send + Sync> = Arc::new(move || restarted.restart(true));
        script.on_sample.push((at(BackendKind::Nemu, 3, 1), action));
    });
    let cells = cell(
        "CA",
        600_000,
        &[
            "instance = \"E1\"\nmode = \"calibrate\"\nreps = 1\nquota = 10",
            &preview("N", 1_000),
        ],
    ) + &cell(
        "CB",
        600_000,
        &["instance = \"E1\"\npath = \"S\"\nmode = \"step_trials\"\nreps = 1"],
    );
    let outcome = h.run(plan_with("restart", APK_TABLE, E1, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    assert!(position(&events, "apk_restart", |event| event["apk_epoch"] == 2).is_some());
    assert!(
        position(&events, "device_clock_reset", |event| event["clock_epoch"]
            == 2)
        .is_some()
    );
    let calib = h.lines("calib-D1.jsonl");
    let calibrations = of_type(&calib, "calib");
    assert_eq!(calibrations.len(), 2, "{calibrations:?}");
    assert_eq!(calibrations[0]["clock_epoch"], 1);
    assert_eq!(calibrations[1]["clock_epoch"], 2);
    assert_eq!(calibrations[1]["source"], "CB");
    let before = h.lines("apk-E1.jsonl");
    assert!(before.iter().all(|record| record["apk_epoch"] == 1));
    let after = h.lines("apk-E1.e2.jsonl");
    let steps = of_type(&after, "step");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["k"], 1);
    assert_eq!(steps[0]["apk_epoch"], 2);
    assert_eq!(steps[0]["clock_epoch"], 2);
}

/// C6 on a display of `size`: every admission decodes its flip with the tapped cell, and the tap
/// was mapped on that display.
fn c6_on(name: &str, size: (u32, u32)) {
    let (h, _apk) = apk_harness_on(name, name, size, |_, _| {});
    let cells = cell(
        "C6",
        600_000,
        &["instance = \"E1\"\npath = \"N\"\nmode = \"task_floor\"\nquota = 8\napk_mode = \"flip\""],
    );
    let outcome = h.run(plan_with(name, APK_TABLE, E1, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let admissions: Vec<Value> = h
        .lines("opps-C6-N.jsonl")
        .into_iter()
        .filter(|line| line["kind"] == "admission")
        .collect();
    assert_eq!(admissions.len(), 8);
    for (index, admission) in admissions.iter().enumerate() {
        let tap = &admission["tap"];
        assert_eq!(tap["display_w"], size.0, "{admission}");
        assert_eq!(tap["display_h"], size.1);
        assert_eq!(tap["rotation"], 0);
        assert_eq!(tap["display_source"], "apk_view");
        let (x, y) = (tap["x"].as_u64().expect("x"), tap["y"].as_u64().expect("y"));
        let j = tap["j"].as_u64().expect("j");
        assert!(x < u64::from(size.0) && y < u64::from(size.1), "{tap}");
        // PROTOCOL §4.2: the tap's cell on this display is the cell the bench meant.
        assert_eq!(
            (4 * y / u64::from(size.1)) * 4 + 4 * x / u64::from(size.0),
            j,
            "{tap}"
        );
        assert_eq!(
            admission["first_flip"]["n"],
            index as u64 + 1,
            "{admission}"
        );
        assert_eq!(admission["first_flip"]["j"], j);
    }
}

#[test]
fn c6_taps_land_in_their_cell_on_a_portrait_display() {
    c6_on("c6-portrait", (208, 216));
}

#[test]
fn c6_taps_land_in_their_cell_on_a_landscape_display() {
    c6_on("c6-landscape", (216, 208));
}

#[test]
fn c6_flips_decode_with_their_identity_and_the_first_frame_that_shows_them() {
    let (h, _apk) = apk_harness("c6", "c6", |_, _| {});
    let cells = cell(
        "C6",
        600_000,
        &["instance = \"E1\"\npath = \"N\"\nmode = \"task_floor\"\nquota = 5\napk_mode = \"flip\""],
    );
    let outcome = h.run(plan_with("c6", APK_TABLE, E1, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let admissions: Vec<Value> = h
        .lines("opps-C6-N.jsonl")
        .into_iter()
        .filter(|line| line["kind"] == "admission")
        .collect();
    assert_eq!(admissions.len(), 5);
    for (index, admission) in admissions.iter().enumerate() {
        assert_eq!(admission["content"]["decoded"], true, "{admission}");
        assert_eq!(admission["content"]["code_mode"], "flip");
        let flip = &admission["first_flip"];
        assert_eq!(flip["n"], index as u64 + 1, "{admission}");
        assert_eq!(flip["j"], admission["tap"]["j"]);
        assert_eq!(flip["is_first_floor_frame"], true);
    }
    let flips: Vec<(u64, u64)> = of_type(&h.lines("apk-E1.jsonl"), "flip")
        .iter()
        .filter_map(|flip| Some((flip["n"].as_u64()?, flip["j"].as_u64()?)))
        .collect();
    let taps: Vec<(u64, u64)> = admissions
        .iter()
        .enumerate()
        .filter_map(|(index, admission)| Some((index as u64 + 1, admission["tap"]["j"].as_u64()?)))
        .collect();
    assert_eq!(flips, taps);
}

#[test]
fn c9_runs_e1_and_e2_together_and_c9_iv_records_the_preview_state() {
    let h = harness(
        "c9",
        "c9",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let cells = "[[cells]]\nid = \"C9\"\nlimit_ms = 60000\nconcurrent = true\n\n\
        [[cells.segments]]\ninstance = \"E1\"\nmode = \"preview\"\nduration_ms = 3000\n\
        formal = { quota = 4, preview_state = \"on\", record_path = \"N\" }\n\n\
        [[cells.segments]]\ninstance = \"E2\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 3000\n";
    let instances = format!("{E1_PLAIN}\n{E2_PLAIN}");
    let outcome = h.run(plan_with("c9", "", &instances, cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let lines = h.lines("opps-C9-N.jsonl");
    let dispatches: Vec<&Value> = lines
        .iter()
        .filter(|line| line["kind"] == "dispatch")
        .collect();
    assert!(!dispatches.is_empty());
    assert!(dispatches.iter().all(|dispatch| {
        dispatch["instance"] == "E1" && dispatch["preview_state"] == "on" && dispatch["path"] == "N"
    }));
    assert!(
        lines
            .iter()
            .any(|line| line["kind"] == "preview" && line["instance"] == "E2")
    );
    // Both segments started before either ended.
    let events = h.events();
    let starts: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| event["type"] == "segment_start")
        .map(|(index, _)| index)
        .collect();
    let first_end = position(&events, "segment_end", |_| true).expect("a segment end");
    assert_eq!(starts.len(), 2);
    assert!(starts.iter().all(|start| *start < first_end));
    assert!(position(&events, "lock_order_violation", |_| true).is_none());
}

#[test]
fn c10_rotates_with_a_new_generation_and_reconnects_s_by_its_identity() {
    let (h, _apk) = apk_harness("c10", "c10", |_, _| {});
    let cells = cell(
        "C10",
        600_000,
        &[
            "instance = \"E1\"\npath = \"N\"\nmode = \"rotate\"\nreps = 2",
            "instance = \"E1\"\npath = \"S\"\nmode = \"reconnect\"\nreps = 2",
            "instance = \"E1\"\npath = \"N\"\nmode = \"reconnect\"\nreps = 1",
        ],
    );
    let outcome = h.run(plan_with("c10", APK_TABLE, E1, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    let rotations: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "rotation")
        .collect();
    assert_eq!(rotations.len(), 2);
    assert_eq!(rotations[0]["to"], "portrait");
    assert_eq!(rotations[0]["frame"], json!([208, 216]));
    assert_eq!(rotations[1]["to"], "landscape");
    assert_eq!(rotations[1]["frame"], json!([216, 208]));
    for rotation in &rotations {
        assert_eq!(rotation["ok"], true);
        assert_eq!(rotation["close_confirmed"], true);
        assert!(
            rotation["generation_after"].as_u64() > rotation["generation_before"].as_u64(),
            "{rotation}"
        );
    }
    let reconnects: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "reconnect" && event["path"] == "S")
        .collect();
    assert_eq!(reconnects.len(), 2);
    for reconnect in &reconnects {
        assert_eq!(reconnect["ok"], true, "{reconnect}");
        assert_ne!(reconnect["old_scid"], reconnect["new_scid"]);
        assert!(reconnect["stopped_pid"].is_u64());
        assert!(reconnect["t_stream_end_us"].is_u64());
        assert_eq!(reconnect["first_sequence_after"], 1);
        assert_eq!(reconnect["close_confirmed"], true);
    }
    assert_eq!(
        h.device
            .check_calls()
            .iter()
            .filter(|call| call.starts_with("stop:E1:"))
            .count(),
        2
    );
    assert!(
        position(&events, "reconnect", |event| event["path"] == "N"
            && event["ok"] == true)
        .is_some()
    );
}

#[test]
fn c8_saves_each_frame_and_lists_every_picture_target_path_and_frame() {
    let (h, _apk) = apk_harness("c8", "c8", |_, _| {});
    let source = h.root.join("c8src");
    std::fs::create_dir_all(&source).expect("the picture folder");
    std::fs::write(source.join("a.png"), b"picture a").expect("picture a");
    std::fs::write(source.join("b.png"), b"picture b").expect("picture b");
    let truth = json!({
        "package": "packs/neutral",
        "package_sha256": "00",
        "facts": null,
        "images": [
            { "image": "a.png", "kind": "truth", "targets": [
                { "target": "t1", "class": "ocr", "hit": true, "text": "12" },
                { "target": "t2", "class": "template", "hit": false },
            ] },
            { "image": "b.png", "kind": "negative", "targets": [
                { "target": "t2", "class": "template", "hit": false },
            ] },
        ],
    });
    std::fs::write(source.join("truth.json"), truth.to_string()).expect("truth.json");
    let table = format!(
        "{APK_TABLE}\n[c8]\nimages_dir = '{}'\ntruth = '{}'\nframes_per_image = 2\n",
        source.display(),
        source.join("truth.json").display()
    );
    let cells = cell(
        "C8",
        600_000,
        &[
            "instance = \"E1\"\npath = \"N\"\nmode = \"static_images\"",
            "instance = \"E1\"\npath = \"S\"\nmode = \"static_images\"",
        ],
    );
    let outcome = h.run(plan_with("c8", &table, E1, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let frames: Vec<Value> = h
        .lines("opps-C8-N.jsonl")
        .into_iter()
        .filter(|line| line["kind"] == "c8_frame")
        .collect();
    assert_eq!(frames.len(), 4);
    for frame in &frames {
        let saved = frame["saved_path"].as_str().expect("saved_path");
        let bytes = std::fs::read(h.run_dir.join(saved)).expect("the saved frame");
        assert_eq!(
            frame["frame_sha256"],
            actingcommand_capture_bench::safety::sha256_hex(&bytes)
        );
    }
    assert!(h.run_dir.join("c8/a/N-0.png").is_file());
    assert!(h.run_dir.join("c8/b/S-8M-1.png").is_file());
    let csv = std::fs::read_to_string(h.run_dir.join("ocr-coverage.csv")).expect("the list");
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some(
            "image,image_kind,target,target_class,path,bitrate,orientation,frame,command,package,package_sha256,facts,frame_count,result_json,truth_hit,truth_text,result_hit,result_text,verdict,verdict_reason"
        )
    );
    let rows: Vec<&str> = lines.collect();
    // The source: 3 picture targets; N and S: 3 targets × 2 frames each.
    assert_eq!(rows.len(), 3 + 6 + 6, "{csv}");
    assert_eq!(
        rows.iter().filter(|row| row.contains(",source,")).count(),
        3
    );
    assert!(
        rows.iter()
            .all(|row| row.ends_with(",not_evaluated,pending_offline_actinglab"))
    );
    assert!(rows.iter().any(|row| row.contains(",S,8M,")));
}

/// #402 C2c: S's first frame of a C8 picture is asked for with the lower bound of the picture
/// switch. Picture a: the first floored sample meets it. Picture b: from S's third sample on,
/// every sample with a time floor is stale (the slot holds only older frames), so the bound of
/// 50 ms on the virtual clock runs out and the frame is taken without it, with the reason.
#[test]
fn c8_s_first_frame_after_a_picture_switch_meets_the_switch_floor_or_falls_back_with_a_reason() {
    let (h, _apk) = apk_harness("c8floor", "c8floor", |script, _| {
        script.sample_faults.push((
            at(BackendKind::Scrcpy, 3, u64::MAX),
            SampleFault::TimeFloorUnmet,
        ));
    });
    let source = h.root.join("c8src");
    std::fs::create_dir_all(&source).expect("the picture folder");
    std::fs::write(source.join("a.png"), b"picture a").expect("picture a");
    std::fs::write(source.join("b.png"), b"picture b").expect("picture b");
    let truth = json!({
        "package": "packs/neutral",
        "package_sha256": "00",
        "facts": null,
        "images": [
            { "image": "a.png", "kind": "truth", "targets": [
                { "target": "t1", "class": "template", "hit": true },
            ] },
            { "image": "b.png", "kind": "truth", "targets": [
                { "target": "t1", "class": "template", "hit": true },
            ] },
        ],
    });
    std::fs::write(source.join("truth.json"), truth.to_string()).expect("truth.json");
    let table = format!(
        "{APK_TABLE}\n[c8]\nimages_dir = '{}'\ntruth = '{}'\nframes_per_image = 2\nswitch_floor_wait_ms = 50\n",
        source.display(),
        source.join("truth.json").display()
    );
    let cells = cell(
        "C8",
        600_000,
        &["instance = \"E1\"\npath = \"S\"\nmode = \"static_images\""],
    );
    let outcome = h.run(plan_with("c8floor", &table, E1, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let frames: Vec<Value> = h
        .lines("opps-C8-S.jsonl")
        .into_iter()
        .filter(|line| line["kind"] == "c8_frame")
        .collect();
    assert_eq!(frames.len(), 4, "{frames:?}");
    fn frame<'a>(frames: &'a [Value], image: &str, index: u64) -> &'a Value {
        frames
            .iter()
            .find(|frame| frame["image"] == image && frame["frame_idx"] == index)
            .unwrap_or_else(|| panic!("the c8_frame of {image} {index}"))
    }
    for line in &frames {
        assert!(
            line["t_switch_us"].is_u64(),
            "every frame has its switch time: {line}"
        );
        assert!(line["t_recv_us"].is_u64(), "{line}");
    }

    // Picture a: received after the switch, at the first attempt.
    let met = frame(&frames, "a.png", 0);
    let switched = met["t_switch_us"].as_u64().expect("t_switch_us");
    assert_eq!(met["floor_after_us"], switched, "{met}");
    assert_eq!(met["floor_outcome"], "met", "{met}");
    assert_eq!(met["floor_attempts"], 1, "{met}");
    assert!(met["floor_fallback_reason"].is_null(), "{met}");
    assert!(
        met["null_reasons"]["floor_fallback_reason"].is_string(),
        "{met}"
    );
    assert!(
        met["t_recv_us"].as_u64().expect("t_recv_us") > switched,
        "{met}"
    );

    // Picture b: the bound runs out on the virtual clock, then the frame without it.
    let fell = frame(&frames, "b.png", 0);
    let switched = fell["t_switch_us"].as_u64().expect("t_switch_us");
    assert_eq!(fell["floor_after_us"], switched, "{fell}");
    assert_eq!(fell["floor_outcome"], "fell_back", "{fell}");
    assert!(
        fell["floor_attempts"].as_u64().expect("floor_attempts") > 1,
        "{fell}"
    );
    let reason = fell["floor_fallback_reason"]
        .as_str()
        .expect("the fallback reason");
    assert!(reason.starts_with("capture_frame_stale"), "{reason}");
    assert!(
        fell["t_recv_us"].as_u64().expect("t_recv_us") >= switched + 50_000,
        "the frame without the bound is taken only after the 50 ms bound: {fell}"
    );
    assert!(h.run_dir.join("c8/b/S-8M-0.png").is_file());

    // The later frames of a picture follow by after_sequence: no lower bound of their own.
    for later in [frame(&frames, "a.png", 1), frame(&frames, "b.png", 1)] {
        assert!(later["floor_after_us"].is_null(), "{later}");
        assert!(later["floor_outcome"].is_null(), "{later}");
        assert!(
            later["null_reasons"]["floor_after_us"].is_string(),
            "{later}"
        );
    }
}

#[test]
fn every_s_open_starts_the_stand_in_adb_shell_child_and_a_confirmed_close_stops_it() {
    let h = harness(
        "children",
        "children",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let children = RecordedChildren::new();
    h.device.set_children(children.clone());
    let outcome = h.run(plan(
        "children",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("S", 1_000)]),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    assert_eq!(children.spawned().len(), 1);
    assert_eq!(children.spawned(), children.stopped());
    assert_eq!(h.device.live_streams(), Vec::<u32>::new());
}

#[test]
fn the_lifecycle_records_drain_producer_entry_gate_grace_and_the_s_identity() {
    let h = harness(
        "records",
        "records",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let segment = "instance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 6000\n\
        viewer_unsubscribe_ms = 1000\ngrace_ms = 5000\n\
        gates = [{ at_ms = 2500, gate = \"fence\", hold_ms = 500 }]";
    let outcome = h.run(plan("records", 10 * HOURS, &cell("CA", 60_000, &[segment])));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    let s_open = position(&events, "s_open", |_| true).expect("s_open");
    let positive = position(&events, "identity_positive", |event| {
        event["negative_control_empty"] == true && event["argv_equal"] == true
    })
    .expect("identity_positive");
    let start = position(&events, "sampling_start", |_| true).expect("sampling_start");
    assert!(s_open < positive && positive < start);
    let producers: Vec<u64> = events
        .iter()
        .filter(|event| event["type"] == "producer_count")
        .filter_map(|event| event["count"].as_u64())
        .collect();
    assert_eq!(producers, vec![1, 0]);
    let drain = position(&events, "drain", |_| true).expect("drain");
    assert!(events[drain]["calls"].is_array());
    assert!(events[drain]["t_end_us"].as_u64() >= events[drain]["t_start_us"].as_u64());
    let grace = position(&events, "grace", |event| event["exit"] == "hard_gate")
        .expect("the grace ended by the fence");
    assert_eq!(events[grace]["planned_ms"], 5000);
    assert!(
        position(&events, "entry_gate", |event| {
            event["entry"] == "E1"
                && event["gate"] == "fence"
                && event["reaction"] == "reject"
                && event["admitted"] == false
        })
        .is_some()
    );
    assert!(
        position(&events, "entry_gate", |event| {
            event["entry"] == "E2" && event["gate"] == "fence" && event["reaction"] == "close"
        })
        .is_some()
    );
}

#[test]
fn each_cell_records_its_stdio_files_artifact_deltas_and_vmm_handles() {
    let mut h = harness(
        "probes",
        "probes",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    h.probes = Arc::new(ScriptedProbes {
        stdio: (1_234, 2),
        vmm: vec![VmmProcess {
            pid: 42,
            parent_pid: 1,
            image: "VMMHeadless-fake.exe".to_owned(),
            handle_count: Some(900),
        }],
    });
    let outcome = h.run(plan(
        "probes",
        10 * HOURS,
        &cell(
            "CA",
            60_000,
            &[&preview("N", 1_000), &preview("B_model", 1_000)],
        ),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let host = h.lines("host-CA.jsonl");
    let sizes = of_type(&host, "stdio_size");
    assert_eq!(sizes.len(), 2);
    assert_eq!(sizes[0]["phase"], "start");
    assert_eq!(sizes[1]["phase"], "end");
    assert!(sizes.iter().all(|size| size["bytes"] == 1_234));
    let deltas = of_type(&host, "artifact_delta");
    let n = deltas
        .iter()
        .find(|delta| delta["path"] == "N")
        .expect("N's delta");
    assert_eq!(n["bytes"], 0);
    assert!(n["frames"].as_u64() > Some(0));
    assert_eq!(n["events"], n["state_changes"]);
    let b = deltas
        .iter()
        .find(|delta| delta["path"] == "B_model")
        .expect("B_model's delta");
    assert!(b["bytes"].as_u64() > Some(0));
    assert!(b["events"].as_u64() > b["state_changes"].as_u64());
    let events = h.events();
    let handles =
        position(&events, "vmm_handles", |event| event["phase"] == "start").expect("vmm_handles");
    assert_eq!(events[handles]["processes"][0]["handle_count"], 900);
}

#[test]
fn a_nemu_ipc_touch_plan_is_valid_now() {
    let text = plan(
        "touch",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("N", 1_000)]),
    )
    .replace("touch = \"adb_input\"", "touch = \"nemu_ipc\"");
    let parsed = Plan::parse(&text).expect("the plan parses");
    assert_eq!(parsed.validate(), Ok(()));
}
