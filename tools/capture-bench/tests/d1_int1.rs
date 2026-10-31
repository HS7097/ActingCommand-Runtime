// SPDX-License-Identifier: AGPL-3.0-only

//! D1 (plan v2.1 §6.2) for INT-1, the fixes of what the D2 host rehearsal found, on the fake
//! backend under the virtual clock (no real clock, no sleep), each deterministic:
//! - exit 30: `write_json` stages every call under a name of its own and writes one file at a
//!   time; two segments ending together write `closing.json` without a journal failure; a
//!   journal failure does not stop the run and ends it `internal_invariant`;
//! - the flush rule: every appended line is in the file before any flush;
//! - F.n_floor: N admissions carry the first native call's start, S admissions say why not;
//! - E.identity: after a gate clears, no sample is granted before the reopened S proves its
//!   identity;
//! - E.grace: the grace is S's only and never longer than planned;
//! - F.s_late: the fake's `T_est` is early by the margin, as `scrcpy.rs` computes it;
//! - the fake APK of each instance; C8's saved frames per instance; `summary.closes` with the
//!   per-sample reconnect closes; the `viewer_write_failed` record; the entry probes of E4, E5,
//!   E6, E8 and E9 under every gate; the stand-in `adb shell` child's command line.

mod common;

use actingcommand_capture_bench::apk::FakeApk;
use actingcommand_capture_bench::backend::{NativeSpan, PixelLayout, SampledFrame, StreamFacts};
use actingcommand_capture_bench::clock::{Clock, VirtualClock};
use actingcommand_capture_bench::fake::{
    FAKE_FRESH_MARGIN_US, FakeCheckScript, FakeDevice, FakePattern, FakeScript, fake_apks,
    fake_child_arguments, fake_child_lifetime,
};
use actingcommand_capture_bench::gates::reaction;
use actingcommand_capture_bench::journal::Journal;
use actingcommand_capture_bench::model::{Entry, GateKind};
use actingcommand_capture_bench::plan::Plan;
use actingcommand_capture_bench::runner::first_native_start;
use actingcommand_capture_bench::timecode::BandGeometry;
use actingcommand_capture_bench::viewer::viewer_write_failure;
use common::{cell, harness, plan, preview};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const HOURS: u64 = 3_600_000;

const E1_APK: &str = "[[instances]]\nname = \"E1\"\nserial = \"127.0.0.1:16544\"\nnemu_instance = 5\norientation = \"landscape\"\ntouch = \"adb_input\"\napk = true\n";
const E2_APK: &str = "[[instances]]\nname = \"E2\"\nserial = \"127.0.0.1:16576\"\nnemu_instance = 6\norientation = \"portrait\"\ntouch = \"adb_input\"\napk = true\n";
const E2_PLAIN: &str = "[[instances]]\nname = \"E2\"\nserial = \"127.0.0.1:16576\"\nnemu_instance = 6\norientation = \"portrait\"\ntouch = \"adb_input\"\n";
const E1_PLAIN: &str = "[[instances]]\nname = \"E1\"\nserial = \"127.0.0.1:16544\"\nnemu_instance = 5\norientation = \"landscape\"\ntouch = \"adb_input\"\n";

const APK_TABLE: &str = "[apk]\ncell = 8\npings = 20\n";

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

/// A new folder under the test target's temporary directory.
fn scratch(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "capture-bench-d1/int1-{name}-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A harness whose instances each have their own fake APK (cell 8; 216 × 208 landscape, 208 × 216
/// portrait), as `run --fake` gives every APK instance its own (#402 INT-1).
fn apk_harness(
    name: &str,
    instances: &[(&str, bool)],
    configure: impl FnOnce(&mut FakeScript),
) -> common::Harness {
    let h0 = harness(
        name,
        name,
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let mut script = FakeScript::default();
    let mut apks = BTreeMap::new();
    for (index, (instance, portrait)) in instances.iter().enumerate() {
        let apk = FakeApk::new(h0.dyn_clock(), 7_000_000_000 + 1_000_000_000 * index as i64);
        apk.set_cell(8);
        if *portrait {
            apk.set_display(208, 216);
        } else {
            apk.set_display(216, 208);
        }
        script.patterns.insert(
            (*instance).to_owned(),
            FakePattern {
                apk: Arc::clone(&apk),
                geometry: BandGeometry {
                    cell: 8,
                    offset_x: 0,
                    offset_y: 0,
                },
            },
        );
        apks.insert((*instance).to_owned(), apk);
    }
    configure(&mut script);
    common::Harness {
        device: FakeDevice::new(h0.dyn_clock(), script, FakeCheckScript::default()),
        apks,
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

fn us(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("not a time: {value}"))
}

// ---- exit 30: closing.json, write_json, journal failures ----

#[test]
fn write_json_stages_each_call_under_a_name_of_its_own_and_two_writers_never_fail() {
    let root = scratch("json");
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(common::UNIX_ORIGIN_MS));
    let journal = Journal::open(&root, clock).expect("the journal opens");
    // C2d's fixed staged name is taken: a writer that still used it would fail.
    std::fs::create_dir_all(root.join("closing.json.partial")).expect("the folder");
    journal
        .write_json("closing.json", &json!([]))
        .expect("written beside its own staged file");
    let first = journal.staged_name("closing.json");
    let second = journal.staged_name("closing.json");
    assert_ne!(first, second);
    assert!(first.starts_with("closing.json.") && first.ends_with(".partial"));
    // Four writers of one file at once, 50 times each.
    let writers: Vec<_> = (0..4)
        .map(|writer| {
            let journal = Arc::clone(&journal);
            std::thread::spawn(move || {
                for call in 0..50 {
                    journal
                        .write_json("closing.json", &json!([{ "writer": writer, "call": call }]))
                        .expect("a whole-file write");
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().expect("the writer does not panic");
    }
    assert_eq!(journal.failures(), Vec::<String>::new());
    let text = std::fs::read_to_string(root.join("closing.json")).expect("closing.json");
    let written: Value = serde_json::from_str(&text).expect("one whole JSON file");
    assert_eq!(written.as_array().map(Vec::len), Some(1), "{written}");
}

#[test]
fn two_concurrent_segments_ending_together_close_without_a_journal_failure() {
    let h = harness(
        "close-race",
        "close-race",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    // C9-like: E1's and E2's S previews end at the same instant and close at once.
    let cells = "[[cells]]\nid = \"C9\"\nlimit_ms = 60000\nconcurrent = true\n\n\
        [[cells.segments]]\ninstance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 2000\n\n\
        [[cells.segments]]\ninstance = \"E2\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 2000\n";
    let outcome = h.run(plan_with(
        "close-race",
        "",
        &format!("{E1_PLAIN}\n{E2_PLAIN}"),
        cells,
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let summary = h.json("summary.json");
    assert_eq!(summary["journal_failures"], json!([]), "{summary}");
    assert_eq!(summary["result"], "completed");
    assert_eq!(
        h.json("closing.json"),
        json!([]),
        "no close is left in progress"
    );
    let closings = of_type(&h.events(), "closing");
    assert_eq!(
        closings
            .iter()
            .filter(|closing| closing["backend"] == "S")
            .count(),
        2
    );
}

#[test]
fn a_journal_write_failure_does_not_stop_the_run_and_ends_it_internal_invariant() {
    let h = harness(
        "journal-failure",
        "journal-failure",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    // The second cell's opportunity file cannot be opened (a folder has its name).
    std::fs::create_dir_all(h.run_dir.join("opps-CB-N.jsonl")).expect("the folder");
    let cells =
        cell("CA", 60_000, &[&preview("N", 1_000)]) + &cell("CC", 60_000, &[&preview("N", 1_000)]);
    let cells = cells + &cell("CB", 60_000, &[&preview("N", 1_000)]);
    let outcome = h.run(plan("journal-failure", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 30, "{outcome:?}");
    let summary = h.json("summary.json");
    let failures = summary["journal_failures"]
        .as_array()
        .expect("journal_failures");
    assert!(!failures.is_empty(), "{summary}");
    assert!(
        failures.iter().all(|failure| failure
            .as_str()
            .is_some_and(|text| text.contains("opps-CB-N.jsonl"))),
        "{failures:?}"
    );
    assert_eq!(summary["result"], "internal_invariant");
    // The run went on: every cell ran to its end, the one after the failure too.
    for id in ["CA", "CB", "CC"] {
        let (result, _) = common::run_cell_result(&summary, id, "N");
        assert_eq!(result, "cell_completed", "{id}");
    }
    let progress = h.lines("progress.jsonl");
    let ends: Vec<&Value> = progress
        .iter()
        .filter(|line| line["event"] == "cell_end")
        .collect();
    assert_eq!(ends.len(), 3, "{progress:?}");
    assert_eq!(ends[2]["cell"], "CB");
}

// ---- the flush rule ----

#[test]
fn every_appended_line_is_in_its_file_before_any_flush() {
    let root = scratch("lines");
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(common::UNIX_ORIGIN_MS));
    let journal = Journal::open(&root, clock).expect("the journal opens");
    journal.append("opps-CA-N.jsonl", &json!({ "kind": "preview", "seq": 1 }));
    journal.append("opps-CA-N.jsonl", &json!({ "kind": "preview", "seq": 2 }));
    journal.event("cell_start", json!({ "cell": "CA" }));
    // No flush, no sync: the lines are in the files (a crash now would leave them).
    let opps = std::fs::read_to_string(root.join("opps-CA-N.jsonl")).expect("the file");
    assert_eq!(opps.lines().count(), 2, "{opps}");
    let events = std::fs::read_to_string(root.join("events.jsonl")).expect("events.jsonl");
    assert!(
        events.lines().any(|line| line.contains("\"cell_start\"")),
        "{events}"
    );
    assert!(events.ends_with('\n'));
}

// ---- F.n_floor ----

#[test]
fn the_native_start_is_the_first_native_call_never_the_stdio_wait_or_the_flip() {
    let span = |kind: &'static str, start: u64| NativeSpan {
        kind,
        start_us: Some(start),
        end_us: Some(start + 1),
        code: Some(0),
    };
    let frame = SampledFrame {
        width: 1,
        height: 1,
        layout: PixelLayout::Rgba8,
        pixels: vec![0; 4],
        stream_sequence: None,
        device_pts_us: None,
        received_us: None,
        built_us: Some(40),
        built_unix_ms: None,
        native: vec![
            span("stdio_wait", 5),
            span("probe", 10),
            span("pixels", 20),
            span("flip", 30),
        ],
        known_source_us: None,
        floor_check: None,
    };
    assert_eq!(first_native_start(&frame), Some(10));
    let none = SampledFrame {
        native: vec![span("stdio_wait", 5)],
        ..frame
    };
    assert_eq!(first_native_start(&none), None);
}

#[test]
fn n_admissions_carry_the_first_native_start_and_s_admissions_say_why_it_is_null() {
    let h = harness(
        "native-start",
        "native-start",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let c6 = cell(
        "C6",
        600_000,
        &[
            "instance = \"E1\"\npath = \"N\"\nmode = \"task_floor\"\nquota = 3",
            "instance = \"E1\"\npath = \"S\"\nmode = \"task_floor\"\nquota = 3",
        ],
    );
    let outcome = h.run(plan("native-start", 10 * HOURS, &c6));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let n = records(&h, "opps-C6-N.jsonl", "admission");
    assert_eq!(n.len(), 3, "{n:?}");
    for admission in &n {
        let start = us(&admission["native_start_us"]);
        assert!(
            start >= us(&admission["tap"]["t_input_done_us"]),
            "{admission}"
        );
        assert!(
            admission["null_reasons"].get("native_start_us").is_none(),
            "{admission}"
        );
    }
    let s = records(&h, "opps-C6-S.jsonl", "admission");
    assert_eq!(s.len(), 3, "{s:?}");
    for admission in &s {
        assert!(admission["native_start_us"].is_null(), "{admission}");
        assert!(
            admission["null_reasons"]["native_start_us"].is_string(),
            "{admission}"
        );
    }
}

// ---- E.identity ----

#[test]
fn after_a_gate_clears_no_sample_is_granted_before_the_reopened_s_proves_its_identity() {
    // A slow open (300 ms), so samples fall into the reopen after the fence clears.
    let script = FakeScript {
        open_latency_us: 300_000,
        ..FakeScript::default()
    };
    let h = harness(
        "reopen-identity",
        "reopen-identity",
        script,
        FakeCheckScript::default(),
    );
    let segment = "instance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 4000\n\
        gates = [{ at_ms = 1000, gate = \"fence\", hold_ms = 1000 }]";
    let outcome = h.run(plan(
        "reopen-identity",
        10 * HOURS,
        &cell("CA", 60_000, &[segment]),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    // Walk the events: an S grant needs the latest S open's identity positive and its
    // sampling_start, with no S closing since.
    let (mut proven, mut started, mut opens, mut grants) = (None::<String>, false, 0, 0);
    let mut last_open = None::<String>;
    for event in &events {
        match event["type"].as_str() {
            Some("s_open") => {
                opens += 1;
                last_open = event["scid"].as_str().map(str::to_owned);
                proven = None;
                started = false;
            }
            Some("identity_positive") => proven = event["scid"].as_str().map(str::to_owned),
            Some("sampling_start") => {
                started = proven.is_some() && event["scid"].as_str() == proven.as_deref();
            }
            Some("closing") if event["backend"] == "S" => {
                proven = None;
                started = false;
            }
            Some("grant") if event["path"] == "S" => {
                grants += 1;
                assert!(
                    proven.is_some() && proven == last_open && started,
                    "an S grant before the identity positive: {event}"
                );
            }
            Some("discarded_stale") => {
                assert_ne!(event["reason"], "failed:backend_closed", "{event}");
            }
            _ => {}
        }
    }
    assert_eq!(opens, 2, "the open and the reopen after the fence");
    assert!(grants > 0);
    let previews = records(&h, "opps-CA-S.jsonl", "preview");
    assert!(
        previews
            .iter()
            .all(|record| record["disposition"] != "failed:backend_closed"),
        "{previews:?}"
    );
    // The samples that met the reopen were skipped busy (P7 holds admission).
    let cleared = us(&of_type(&events, "gate_clear")[0]["t_record_us"]);
    let reopened = us(&of_type(&events, "sampling_start")[1]["t_us"]);
    assert!(
        previews.iter().any(|record| {
            record["disposition"] == "skipped_busy"
                && (cleared..=reopened).contains(&us(&record["t_tick_us"]))
        }),
        "a sample during the reopen ({cleared}..{reopened}): {previews:?}"
    );
}

// ---- E.grace ----

#[test]
fn the_grace_is_s_only_never_longer_than_planned_and_ends_with_the_cell() {
    let h = harness(
        "grace",
        "grace",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let n = "instance = \"E1\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 3000\n\
        viewer_unsubscribe_ms = 1000";
    let expires = "instance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 8000\n\
        viewer_unsubscribe_ms = 1000";
    let zero = "instance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 3000\n\
        viewer_unsubscribe_ms = 1000\ngrace_ms = 0";
    let cut = "instance = \"E1\"\npath = \"S\"\nmode = \"preview\"\nduration_ms = 3000\n\
        viewer_unsubscribe_ms = 1500";
    let cells = cell("CA", 60_000, &[n])
        + &cell("CB", 60_000, &[expires])
        + &cell("CC", 60_000, &[zero])
        + &cell("CD", 60_000, &[cut]);
    let outcome = h.run(plan("grace", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    let graces = of_type(&events, "grace");
    // N has no grace; each S cell one, inside its own cell.
    assert_eq!(graces.len(), 3, "{graces:?}");
    let progress = h.lines("progress.jsonl");
    let bounds = |id: &str| {
        let at = |event: &str| {
            progress
                .iter()
                .find(|line| line["event"] == event && line["cell"] == id)
                .map(|line| us(&line["t_us"]))
                .unwrap_or_else(|| panic!("{event} of {id}"))
        };
        (at("cell_start"), at("cell_end"))
    };
    for (grace, id) in graces.iter().zip(["CB", "CC", "CD"]) {
        let (start, end) = bounds(id);
        let (enter, exit) = (us(&grace["t_enter_us"]), us(&grace["t_exit_us"]));
        assert!(start <= enter && exit <= end, "{id}: {grace}");
        assert!(exit - enter <= 1_000 * us(&grace["planned_ms"]), "{grace}");
    }
    // The 5 s grace expires at its planned end, seen on time, and S closes right after.
    let expired = &graces[0];
    assert_eq!(expired["exit"], "expired_closed");
    assert_eq!(
        us(&expired["t_exit_us"]) - us(&expired["t_enter_us"]),
        5_000_000
    );
    assert!(
        us(&expired["t_noticed_us"]) - us(&expired["t_exit_us"]) <= 100_000,
        "{expired}"
    );
    let close = of_type(&events, "closing")
        .into_iter()
        .find(|closing| closing["reason"] == "grace_expired")
        .expect("the grace's close");
    assert!(us(&close["t_us"]) >= us(&expired["t_exit_us"]));
    assert!(
        us(&close["t_us"]) - us(&expired["t_exit_us"]) <= 100_000,
        "{close}"
    );
    // Grace 0: it ends where it begins.
    assert_eq!(graces[1]["exit"], "expired_closed");
    assert_eq!(graces[1]["t_exit_us"], graces[1]["t_enter_us"]);
    // The cell ends during the grace: a hard end (the cell-end gate), then the S close.
    assert_eq!(graces[2]["exit"], "hard_gate");
    assert_eq!(graces[2]["gate"], "cell_end");
    assert!(
        of_type(&events, "closing")
            .iter()
            .any(|closing| closing["backend"] == "S"
                && us(&closing["t_us"]) >= us(&graces[2]["t_exit_us"])
                && closing["reason"] == "segment_end")
    );
}

// ---- F.s_late ----

#[test]
fn the_fake_s_floor_admits_the_first_frame_meets_admits_and_its_estimate_is_not_late() {
    let c6 = cell(
        "C6",
        600_000,
        &["instance = \"E1\"\npath = \"S\"\nmode = \"task_floor\"\nquota = 3"],
    );
    let latency = FakeScript::default().scrcpy_latency_us as i64;
    for (bias, late) in [(0_i64, false), (40_000, true)] {
        let script = FakeScript {
            scrcpy_estimate_bias_us: bias,
            ..FakeScript::default()
        };
        let name = format!("t-est-{bias}");
        let h = harness(&name, &name, script, FakeCheckScript::default());
        let outcome = h.run(plan(&name, 10 * HOURS, &c6));
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        let admissions = records(&h, "opps-C6-S.jsonl", "admission");
        assert_eq!(admissions.len(), 3, "{admissions:?}");
        for admission in &admissions {
            let check = &admission["floor_check"];
            let (t_est, floor) = (
                check["t_est_us"].as_i64().expect("t_est_us"),
                check["floor_us"].as_i64().expect("floor_us"),
            );
            let (t_src, t_recv) = (
                check["t_src_known_us"].as_i64().expect("t_src_known_us"),
                check["t_recv_us"].as_i64().expect("t_recv_us"),
            );
            let (pts, offset, margin) = (
                check["pts_us"].as_i64().expect("pts_us"),
                check["offset_min_us"].as_i64().expect("offset_min_us"),
                check["margin_us"].as_i64().expect("margin_us"),
            );
            // scrcpy.rs:484-486 on the fake's clocks: the offset is the transport latency.
            assert_eq!(check["admitted"], true, "{check}");
            assert_eq!(t_est, pts + offset - margin, "{check}");
            assert_eq!((offset, margin), (latency + bias, FAKE_FRESH_MARGIN_US));
            assert!(t_est >= floor && t_recv >= floor, "{check}");
            assert_eq!(t_est > t_src, late, "bias {bias}: {check}");
            if !late {
                // The first frame `meets` admits is the margin less the latency after the floor.
                assert_eq!(t_src - floor, FAKE_FRESH_MARGIN_US - latency, "{check}");
                assert_eq!(t_est, floor, "{check}");
            }
        }
    }
}

// ---- the fake APK per instance ----

#[test]
fn run_fake_gives_every_apk_instance_its_own_fake_apk_clock_and_display() {
    let text = plan_with(
        "apks",
        APK_TABLE,
        &format!("{E1_APK}\n{E2_APK}"),
        &cell("CA", 60_000, &[&preview("N", 1_000)]),
    );
    let parsed = Plan::parse(&text).expect("the plan parses");
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(common::UNIX_ORIGIN_MS));
    let set = fake_apks(&clock, &parsed);
    assert_eq!(
        set.apks.keys().cloned().collect::<Vec<_>>(),
        vec!["E1".to_owned(), "E2".to_owned()]
    );
    assert_eq!(
        set.patterns.keys().cloned().collect::<Vec<_>>(),
        vec!["E1".to_owned(), "E2".to_owned()]
    );
    let (e1, e2) = (&set.apks["E1"], &set.apks["E2"]);
    assert_eq!(e1.display(), (640, 360, 0));
    assert_eq!(e2.display(), (360, 640, 0));
    assert_ne!(e1.device_now_ns(), e2.device_now_ns(), "two device clocks");
    assert!(Arc::ptr_eq(&set.patterns["E2"].apk, e2));
}

#[test]
fn each_instance_shows_its_own_fake_apks_timecode_and_its_taps_flip_only_its_apk() {
    let h = apk_harness("own-apk", &[("E1", false), ("E2", true)], |_| {});
    let cells = cell(
        "CA",
        60_000,
        &[
            "instance = \"E1\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 1000\napk_mode = \"pattern\"",
            "instance = \"E2\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 1000\napk_mode = \"pattern\"",
        ],
    ) + &cell(
        "C6",
        600_000,
        &["instance = \"E2\"\npath = \"N\"\nmode = \"task_floor\"\nquota = 3\napk_mode = \"flip\""],
    );
    let outcome = h.run(plan_with(
        "own-apk",
        APK_TABLE,
        &format!("{E1_APK}\n{E2_APK}"),
        &cells,
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    for instance in ["E1", "E2"] {
        let apk = &h.apks[instance];
        let delivered: Vec<Value> = records(&h, "opps-CA-N.jsonl", "preview")
            .into_iter()
            .filter(|record| record["instance"] == instance && record["disposition"] == "delivered")
            .collect();
        assert!(!delivered.is_empty(), "{instance}");
        for record in &delivered {
            // The frame shows this instance's APK: the vsync of its source time on this APK's
            // device clock (the two APKs' clocks are 1 s apart).
            let content = &record["content"];
            assert_eq!(content["decoded"], true, "{record}");
            let source = us(&record["known_source_us"]);
            let vsync = content["t_vsync_dev_ns"].as_i64().expect("t_vsync_dev_ns");
            let at_source = apk.device_ns_at(source);
            assert!(
                vsync <= at_source && at_source - vsync < apk.frame_period_ns,
                "{instance}: {record}"
            );
        }
    }
    // E2's taps flip E2's APK only.
    let flips = |instance: &str| {
        h.lines(&format!("apk-{instance}.jsonl"))
            .into_iter()
            .filter(|line| line["type"] == "flip")
            .count()
    };
    assert_eq!(flips("E2"), 3);
    assert_eq!(flips("E1"), 0);
    let admissions = records(&h, "opps-C6-N.jsonl", "admission");
    assert!(
        admissions
            .iter()
            .all(|admission| admission["first_flip"]["j"] == admission["tap"]["j"]),
        "{admissions:?}"
    );
}

// ---- C8 per instance ----

#[test]
fn c8_saves_the_frames_of_each_instance_under_its_own_name() {
    let h = apk_harness("c8-instances", &[("E1", false), ("E2", true)], |_| {});
    let source = h.root.join("c8src");
    std::fs::create_dir_all(&source).expect("the picture folder");
    std::fs::write(source.join("a.png"), b"picture a").expect("picture a");
    let truth = json!({
        "package": "packs/neutral",
        "package_sha256": "00",
        "facts": null,
        "images": [
            { "image": "a.png", "kind": "truth", "targets": [
                { "target": "t1", "class": "template", "hit": true },
            ] },
        ],
    });
    std::fs::write(source.join("truth.json"), truth.to_string()).expect("truth.json");
    let table = format!(
        "{APK_TABLE}\n[c8]\nimages_dir = '{}'\ntruth = '{}'\nframes_per_image = 1\n",
        source.display(),
        source.join("truth.json").display()
    );
    let cells = cell(
        "C8",
        600_000,
        &[
            "instance = \"E1\"\npath = \"N\"\nmode = \"static_images\"",
            "instance = \"E2\"\npath = \"N\"\nmode = \"static_images\"",
        ],
    );
    let outcome = h.run(plan_with(
        "c8-instances",
        &table,
        &format!("{E1_APK}\n{E2_APK}"),
        &cells,
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let frames = records(&h, "opps-C8-N.jsonl", "c8_frame");
    assert_eq!(frames.len(), 2, "{frames:?}");
    let paths: BTreeSet<String> = frames
        .iter()
        .map(|frame| frame["saved_path"].as_str().expect("saved_path").to_owned())
        .collect();
    assert_eq!(
        paths,
        BTreeSet::from(["c8/E1/a/N-0.png".to_owned(), "c8/E2/a/N-0.png".to_owned()])
    );
    for path in &paths {
        assert!(h.run_dir.join(path).is_file(), "{path}");
    }
    let csv = std::fs::read_to_string(h.run_dir.join("ocr-coverage.csv")).expect("the list");
    assert!(csv.contains("c8/results/E1/"), "{csv}");
    assert!(csv.contains("c8/results/E2/"), "{csv}");
}

// ---- summary.closes ----

#[test]
fn summary_closes_list_the_per_sample_reconnect_closes() {
    let h = harness(
        "reconnect-closes",
        "reconnect-closes",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let segment = "instance = \"E1\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 1000\n\
        reconnect_each_sample = true";
    let outcome = h.run(plan(
        "reconnect-closes",
        10 * HOURS,
        &cell("CA", 60_000, &[segment]),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let reconnects = records(&h, "opps-CA-N.jsonl", "preview")
        .into_iter()
        .filter(|record| record["reconnect"]["generation"].is_u64())
        .count();
    assert!(reconnects > 0);
    let summary = h.json("summary.json");
    let closes: Vec<&Value> = summary["closes"]
        .as_array()
        .expect("closes")
        .iter()
        .filter(|close| close["reason"] == "reconnect_each_sample")
        .collect();
    assert_eq!(closes.len(), reconnects, "{summary}");
    assert!(
        closes
            .iter()
            .all(|close| close["backend"] == "N" && close["confirmed"] == true),
        "{closes:?}"
    );
    // Every close of the events is in closes[].
    let closings = of_type(&h.events(), "closing").len();
    assert_eq!(summary["closes"].as_array().map(Vec::len), Some(closings));
}

// ---- viewer_write_failed ----

#[test]
fn a_closed_viewer_pipe_is_expected_only_once_the_bench_stops_the_viewer() {
    let broken = std::io::Error::from_raw_os_error(109);
    let record = viewer_write_failure(56, &broken, true);
    assert_eq!(record["expected"], true, "{record}");
    assert_eq!(record["cause"], "pipe_closed_by_the_viewer_stop");
    assert_eq!(record["os_error"], 109);
    assert_eq!(record["viewer_id"], 56);
    let during = viewer_write_failure(56, &broken, false);
    assert_eq!(during["expected"], false, "{during}");
    assert_eq!(during["cause"], "write_failed_during_the_segment");
    let other = viewer_write_failure(56, &std::io::Error::other("disk"), true);
    assert_eq!(other["expected"], false, "{other}");
    assert_eq!(other["cause"], "write_failed_while_stopping");
    let no_data = viewer_write_failure(57, &std::io::Error::from_raw_os_error(232), true);
    assert_eq!(no_data["expected"], true, "{no_data}");
}

// ---- entry × gate: the probes ----

#[test]
fn entry_probes_meet_e4_e5_e6_e8_and_e9_under_every_gate_as_the_table_says() {
    let h = apk_harness("probes", &[("E1", false)], |_| {});
    let probes = "[\n  { at_ms = 4500, entries = [\"E4\", \"E5\", \"E6\", \"E8\", \"E9\"] },\n  \
        { at_ms = 9500, entries = [\"E4\", \"E5\", \"E6\", \"E8\", \"E9\"] },\n  \
        { at_ms = 14500, entries = [\"E4\", \"E5\", \"E6\", \"E8\", \"E9\"] },\n  \
        { at_ms = 19000, entries = [\"E4\", \"E5\", \"E6\", \"E8\", \"E9\"] },\n  \
        { at_ms = 22000, entries = [\"E4\", \"E5\", \"E6\", \"E8\", \"E9\"] },\n]";
    let segment = format!(
        "instance = \"E1\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 25000\n\
        gates = [\n  {{ at_ms = 3000, gate = \"blackout\", hold_ms = 3000 }},\n  \
        {{ at_ms = 8000, gate = \"fence\", hold_ms = 3000 }},\n  \
        {{ at_ms = 13000, gate = \"hold_request\", hold_ms = 3000 }},\n  \
        {{ at_ms = 18000, gate = \"pause\", hold_ms = 2000 }},\n  \
        {{ at_ms = 21000, gate = \"stop\", hold_ms = 2000 }},\n]\n\
        entry_probes = {probes}"
    );
    let outcome = h.run(plan_with(
        "probes",
        APK_TABLE,
        E1_APK,
        &cell("C10", 600_000, &[&segment]),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    let entries = [
        ("E4", Entry::ManualRefresh),
        ("E5", Entry::Calibration),
        ("E6", Entry::ApkControl),
        ("E8", Entry::AutoReopen),
        ("E9", Entry::DeviceSideCheck),
    ];
    let gates = [
        GateKind::Blackout,
        GateKind::Fence,
        GateKind::HoldRequest,
        GateKind::Holding,
        GateKind::StopClass,
        GateKind::RoutinePause,
    ];
    let notes = of_type(&events, "entry_gate");
    for (code, entry) in entries {
        for gate in gates {
            let expected = reaction(entry, gate);
            assert!(
                notes.iter().any(|note| note["entry"] == code
                    && note["gate"] == gate.as_str()
                    && note["reaction"] == expected.label()
                    && note["admitted"] == expected.admits(false)),
                "{code} × {}: {notes:?}",
                gate.as_str()
            );
        }
    }
    // Each probe went ahead exactly when every gate in force admits its entry.
    let attempts = of_type(&events, "entry_probe");
    assert_eq!(attempts.len(), 25, "{attempts:?}");
    for attempt in &attempts {
        let entry = entries
            .iter()
            .find(|(code, _)| attempt["entry"] == *code)
            .map(|(_, entry)| *entry)
            .expect("a probed entry");
        let in_force: Vec<GateKind> = attempt["gates"]
            .as_array()
            .expect("gates")
            .iter()
            .map(|name| {
                *gates
                    .iter()
                    .find(|gate| name == gate.as_str())
                    .expect("a gate name")
            })
            .collect();
        assert!(!in_force.is_empty(), "{attempt}");
        let admits = in_force
            .iter()
            .all(|gate| reaction(entry, *gate).admits(false));
        assert_eq!(attempt["outcome"]["admitted"], admits, "{attempt}");
    }
    // E8 under the pause reconnected N: its close is in closes[], confirmed.
    let summary = h.json("summary.json");
    assert!(
        summary["closes"]
            .as_array()
            .expect("closes")
            .iter()
            .any(|close| close["reason"] == "entry_probe_reopen" && close["confirmed"] == true),
        "{summary}"
    );
}

#[test]
fn an_entry_probe_names_only_the_probed_entries() {
    let segment = "instance = \"E1\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 1000\n\
        entry_probes = [{ at_ms = 500, entries = [\"E7\"] }]";
    let text = plan("probe-plan", 10 * HOURS, &cell("CA", 60_000, &[segment]));
    let parsed = Plan::parse(&text).expect("the plan parses");
    let refused = parsed.validate().expect_err("E7 is not a probe");
    assert!(refused.contains("E7"), "{refused}");
    let late = "instance = \"E1\"\npath = \"N\"\nmode = \"preview\"\nduration_ms = 1000\n\
        entry_probes = [{ at_ms = 1500, entries = [\"E4\"] }]";
    let text = plan("probe-plan", 10 * HOURS, &cell("CA", 60_000, &[late]));
    assert!(Plan::parse(&text).expect("parses").validate().is_err());
}

// ---- the stand-in adb shell child ----

#[test]
fn the_stand_in_adb_shell_child_has_the_real_childs_command_line() {
    let stream = StreamFacts {
        scid: 0x0100_002a,
        local_port: Some(27_042),
        arguments: vec![
            "CLASSPATH=/data/local/tmp/ac402-scrcpy-server.jar".to_owned(),
            "app_process".to_owned(),
            "/".to_owned(),
            "com.genymobile.scrcpy.Server".to_owned(),
            "5.0.1".to_owned(),
            "scid=0100002a".to_owned(),
        ],
        remote_path: "/data/local/tmp/ac402-scrcpy-server.jar".to_owned(),
    };
    let arguments = fake_child_arguments("TESTSERIAL0001", &stream);
    assert_eq!(&arguments[..3], ["-s", "TESTSERIAL0001", "shell"]);
    assert_eq!(arguments.last().map(String::as_str), Some("scid=0100002a"));
    assert_eq!(fake_child_lifetime(&arguments), Some(3_600));
    let mut bounded = arguments.clone();
    bounded.extend(["--max-seconds".to_owned(), "5".to_owned()]);
    assert_eq!(fake_child_lifetime(&bounded), Some(5));
    // Not the real child's shape: refused.
    assert_eq!(fake_child_lifetime(&arguments[1..]), None);
    let mut two = arguments.clone();
    two.push("scid=0100002b".to_owned());
    assert_eq!(fake_child_lifetime(&two), None);
    let mut bad = arguments;
    if let Some(last) = bad.last_mut() {
        *last = "scid=xyz".to_owned();
    }
    assert_eq!(fake_child_lifetime(&bad), None);
    assert_eq!(
        fake_child_lifetime(&["--scid".to_owned(), "0100002a".to_owned()]),
        None
    );
}
