// SPDX-License-Identifier: AGPL-3.0-only

//! D1 (plan v2.1 §6.2) for the C2d fields the analysis lacked, on the fake backend and the fake
//! APK under the virtual clock (no real clock, no sleep): `closes[].device_side.gone_within_ms`
//! (E.s_close's "2 s 内"), the preview records' `labelled_content_time_us` (A.static_label), the
//! formal loop's native returns in the `drain` record (E.drain), the dispatch records' `variant`
//! (C7) and `first_flip`'s two reasons (F.identity, F.visible).

mod common;

use actingcommand_capture_bench::apk::FakeApk;
use actingcommand_capture_bench::fake::{
    FakeCheckScript, FakeDevice, FakePattern, FakeScript, SampleFault,
};
use actingcommand_capture_bench::model::BackendKind;
use actingcommand_capture_bench::timecode::BandGeometry;
use common::{at, cell, harness, plan, position, preview};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

const HOURS: u64 = 3_600_000;

const E1_APK: &str = "[[instances]]\nname = \"E1\"\nserial = \"127.0.0.1:16544\"\nnemu_instance = 5\norientation = \"landscape\"\ntouch = \"adb_input\"\napk = true\n";

const APK_TABLE: &str = "[apk]\ncell = 8\npings = 20\n";

const FLIP_NOT_SEEN: &str = "no frame showed the tap's flip within the wait";
const FLIP_BEFORE_UNDECODED: &str = "no frame before the tap gave a decodable flip number";

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

/// A harness whose fake frames show the fake APK's band (cell 8 on a 216 × 208 display).
fn apk_harness(
    name: &str,
    configure: impl FnOnce(&mut FakeScript, &Arc<FakeApk>),
) -> common::Harness {
    let h0 = harness(
        name,
        name,
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let apk = FakeApk::new(h0.dyn_clock(), 7_000_000_000);
    apk.set_cell(8);
    apk.set_display(216, 208);
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
    common::Harness {
        device: FakeDevice::new(h0.dyn_clock(), script, FakeCheckScript::default()),
        apks,
        ..h0
    }
}

/// The S closes of `summary.json`.
fn s_closes(h: &common::Harness) -> Vec<Value> {
    h.json("summary.json")["closes"]
        .as_array()
        .expect("summary closes")
        .iter()
        .filter(|close| close["backend"] == "S")
        .cloned()
        .collect()
}

fn records(h: &common::Harness, file: &str, kind: &str) -> Vec<Value> {
    h.lines(file)
        .into_iter()
        .filter(|line| line["kind"] == kind)
        .collect()
}

#[test]
fn an_s_close_records_in_milliseconds_when_its_server_was_seen_gone_or_why_not() {
    // The disappearance check takes 1.2 s, then 2.5 s, on the clock.
    for (delay_us, within_2_s) in [(1_200_000_u64, true), (2_500_000, false)] {
        let check = FakeCheckScript {
            gone_after_us: BTreeMap::from([("E1".to_owned(), delay_us)]),
            ..FakeCheckScript::default()
        };
        let h = harness("gone-ms", "gone-ms", FakeScript::default(), check);
        let outcome = h.run(plan(
            "gone-ms",
            10 * HOURS,
            &cell("CA", 60_000, &[&preview("S", 2_000)]),
        ));
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        let closes = s_closes(&h);
        assert_eq!(closes.len(), 1, "{closes:?}");
        let side = &closes[0]["device_side"];
        assert_eq!(side["gone"], true, "{side}");
        let within = side["gone_within_ms"].as_u64().expect("gone_within_ms");
        let after_request = side["gone_after_request_ms"]
            .as_u64()
            .expect("gone_after_request_ms");
        assert!(within >= delay_us / 1_000, "{side}");
        assert_eq!(within <= 2_000, within_2_s, "{side}");
        assert!(after_request >= within, "{side}");
        assert!(closes[0].get("null_reasons").is_none(), "{}", closes[0]);
    }

    // A disappearance that cannot be confirmed has no time, and says why.
    let check = FakeCheckScript {
        gone_unconfirmed: vec!["E1".to_owned()],
        ..FakeCheckScript::default()
    };
    let h = harness("gone-null", "gone-null", FakeScript::default(), check);
    let outcome = h.run(plan(
        "gone-null",
        10 * HOURS,
        &cell("CA", 60_000, &[&preview("S", 2_000)]),
    ));
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    let closes = s_closes(&h);
    assert_eq!(closes.len(), 1, "{closes:?}");
    let close = &closes[0];
    assert_eq!(close["device_side"]["gone"], false, "{close}");
    assert!(close["device_side"]["gone_within_ms"].is_null(), "{close}");
    assert!(close["device_side"]["gone_after_request_ms"].is_null());
    assert!(
        close["null_reasons"]["device_side.gone_within_ms"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("device_side_unconfirmed")),
        "{close}"
    );
}

#[test]
fn every_delivered_preview_frame_carries_the_time_its_path_labels_it_with() {
    let mut script = FakeScript::default();
    script
        .sample_faults
        .push((at(BackendKind::Nemu, 3, 1), SampleFault::StdioBusy));
    let h = harness("labelled", "labelled", script, FakeCheckScript::default());
    let cells =
        cell("CA", 60_000, &[&preview("N", 1_000)]) + &cell("CB", 60_000, &[&preview("S", 1_000)]);
    let outcome = h.run(plan("labelled", 10 * HOURS, &cells));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    for (file, time) in [
        ("opps-CA-N.jsonl", "t_built_us"),
        ("opps-CB-S.jsonl", "t_recv_us"),
    ] {
        let previews = records(&h, file, "preview");
        let delivered: Vec<&Value> = previews
            .iter()
            .filter(|record| record["disposition"] == "delivered")
            .collect();
        assert!(!delivered.is_empty(), "{file}");
        for record in &delivered {
            let labelled = record["labelled_content_time_us"]
                .as_u64()
                .unwrap_or_else(|| panic!("{file}: {record}"));
            assert_eq!(Some(labelled), record["timing"][time].as_u64(), "{record}");
        }
        for record in previews
            .iter()
            .filter(|record| record["disposition"] != "delivered")
        {
            assert!(record["labelled_content_time_us"].is_null(), "{record}");
            assert_eq!(
                record["null_reasons"]["labelled_content_time_us"], "not delivered",
                "{record}"
            );
        }
    }
    assert!(
        records(&h, "opps-CA-N.jsonl", "preview")
            .iter()
            .any(|record| record["disposition"] != "delivered"),
        "the injected failure is a preview opportunity"
    );
}

#[test]
fn the_formal_drain_has_the_native_return_of_a_dispatch_in_flight_or_marks_it_unconfirmed() {
    // Each fake Nemu capture takes 2 s, so the first dispatch is in flight when the 1 s segment
    // stops new work.
    let segment =
        "instance = \"E1\"\nmode = \"preview\"\nduration_ms = 1000\nformal = { period_ms = 500 }";

    let script = FakeScript {
        nemu_latency_us: 2_000_000,
        ..FakeScript::default()
    };
    let h = harness(
        "drain-native",
        "drain-native",
        script,
        FakeCheckScript::default(),
    );
    let outcome = h.run(plan(
        "drain-native",
        10 * HOURS,
        &cell("CA", 60_000, &[segment]),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    let drain = &events[position(&events, "drain", |_| true).expect("drain")];
    let calls = drain["calls"].as_array().expect("drain calls");
    assert!(!calls.is_empty(), "{drain}");
    for call in calls {
        assert_eq!(call["unconfirmed"], false, "{call}");
        let native = call["t_native_return_us"]
            .as_u64()
            .expect("t_native_return_us");
        let caller = call["t_caller_return_us"]
            .as_u64()
            .expect("t_caller_return_us");
        assert!(
            native >= drain["t_start_us"].as_u64().expect("t_start_us"),
            "{call}"
        );
        assert!(native <= caller, "{call}");
    }

    // The same dispatch whose first capture times out: its native call never returns.
    let mut script = FakeScript {
        nemu_latency_us: 2_000_000,
        ..FakeScript::default()
    };
    script
        .sample_faults
        .push((at(BackendKind::Nemu, 1, 1), SampleFault::Poisoned));
    let h = harness(
        "drain-unconfirmed",
        "drain-unconfirmed",
        script,
        FakeCheckScript::default(),
    );
    let outcome = h.run(plan(
        "drain-unconfirmed",
        10 * HOURS,
        &cell("CA", 60_000, &[segment]),
    ));
    assert_eq!(outcome.exit_code, 20, "{outcome:?}");
    let events = h.events();
    let drain = &events[position(&events, "drain", |_| true).expect("drain")];
    let calls = drain["calls"].as_array().expect("drain calls");
    assert_eq!(calls.len(), 1, "{drain}");
    assert_eq!(calls[0]["unconfirmed"], true, "{drain}");
    assert!(calls[0]["t_native_return_us"].is_null(), "{drain}");
    assert!(calls[0]["t_caller_return_us"].is_u64(), "{drain}");
    assert!(
        calls[0]["null_reasons"]["t_native_return_us"].is_string(),
        "{drain}"
    );
}

#[test]
fn every_dispatch_record_says_whether_it_is_the_c7_variant_whatever_its_segment_is_called() {
    let h = harness(
        "variant",
        "variant",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let main = "name = \"first\"\ninstance = \"E1\"\nmode = \"preview\"\nduration_ms = 5000\n\
        formal = { quota = 3 }";
    let variant = "name = \"second\"\ninstance = \"E1\"\nmode = \"preview\"\nduration_ms = 5000\n\
        formal = { quota = 3, capture_from_scrcpy = true }";
    let outcome = h.run(plan(
        "variant",
        10 * HOURS,
        &cell("C7", 60_000, &[main, variant]),
    ));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let dispatches = records(&h, "opps-C7-off.jsonl", "dispatch");
    for (segment, expected) in [("first", false), ("second", true)] {
        let of_segment: Vec<&Value> = dispatches
            .iter()
            .filter(|record| record["segment"] == segment)
            .collect();
        assert!(!of_segment.is_empty(), "{segment}: {dispatches:?}");
        assert!(
            of_segment
                .iter()
                .all(|record| record["variant"] == expected),
            "{segment}: {of_segment:?}"
        );
    }
}

#[test]
fn first_flip_tells_an_undecodable_frame_before_the_tap_from_a_flip_not_seen_within_the_wait() {
    let c6 = cell(
        "C6",
        600_000,
        &["instance = \"E1\"\npath = \"N\"\nmode = \"task_floor\"\nquota = 2\napk_mode = \"flip\""],
    );

    // Trial 1's capture before the tap fails (the first Nemu call): n + 1 is not known.
    let h = apk_harness("flip-before", |script, _| {
        script
            .sample_faults
            .push((at(BackendKind::Nemu, 1, 1), SampleFault::StdioBusy));
    });
    let outcome = h.run(plan_with("flip-before", APK_TABLE, E1_APK, &c6));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let admissions = records(&h, "opps-C6-N.jsonl", "admission");
    assert_eq!(admissions.len(), 2, "{admissions:?}");
    assert!(admissions[0]["first_flip"].is_null(), "{}", admissions[0]);
    assert_eq!(
        admissions[0]["null_reasons"]["first_flip"], FLIP_BEFORE_UNDECODED,
        "{}",
        admissions[0]
    );
    assert_eq!(admissions[1]["first_flip"]["n"], 2, "{}", admissions[1]);

    // Another tap lands while trial 1's floor capture runs (the second Nemu call): the frames
    // show n + 2, and n + 1 is never seen.
    let h = apk_harness("flip-unseen", |script, apk| {
        let extra = Arc::clone(apk);
        let action: Arc<dyn Fn() + Send + Sync> = Arc::new(move || extra.tap(10, 10));
        script.on_sample.push((at(BackendKind::Nemu, 2, 1), action));
    });
    let outcome = h.run(plan_with("flip-unseen", APK_TABLE, E1_APK, &c6));
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let admissions = records(&h, "opps-C6-N.jsonl", "admission");
    assert_eq!(admissions.len(), 2, "{admissions:?}");
    assert!(admissions[0]["first_flip"].is_null(), "{}", admissions[0]);
    assert_eq!(
        admissions[0]["null_reasons"]["first_flip"], FLIP_NOT_SEEN,
        "{}",
        admissions[0]
    );
    assert_eq!(admissions[1]["first_flip"]["n"], 3, "{}", admissions[1]);
}
