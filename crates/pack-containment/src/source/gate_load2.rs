// SPDX-License-Identifier: AGPL-3.0-only

// LOAD-2 (Workflow #407, register §1.258; design `rd-1031/check-speed/model-loading.md` §2, §7
// LOAD-2; coordinator brief `brief-load-2.md`): a source pack's loading copy is compiled with the
// pack, as the Runtime and `actinglab package bundle` admit a source pack.
//
// - Its pages join the derived page set's `loading` member, outside the page set: the detector
//   lists them as loading pages and never among its pages; its targets join the recognition pack.
// - A page or target id the pack declares itself is refused (`loading_page_collision`,
//   `loading_target_collision`), and so is a template the copy reads that the pack lacks
//   (`loading_template_invalid`), each `standard_pack_root_invalid` naming the copy and its pack.
// - A pack without a copy compiles exactly as before.
//
// A synthetic `neutral` pack of 16 x 8, server `test`: `home` (its entry) and `page_b` (its target)
// told by a colour probe each. The copy declares the template `loading/spinner` (a checkerboard of
// 8 x 8), the colour target `loading/backdrop` and the page `transition` that reads both.

use crate::{Containment, ContainmentError, InstanceId, LoadedBundle};
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const WIDTH: u32 = 16;
const HEIGHT: u32 = 8;
const PACKAGE_ID: &str = "neutral.test.loading";
const COPY: &str = "resources/loading.json";
const SPINNER: &str = "resources/loading/spinner.png";
const PAGES_ENTRY: &str = "resources/recognition/neutral.test.pages.json";
const PACK_ENTRY: &str = "resources/recognition/neutral.test.pack.json";

// An uncompressed RGB PNG of one stored deflate block.
fn encode_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for y in 0..height {
        scanlines.push(0);
        for x in 0..width {
            scanlines.extend_from_slice(&pixel(x, y));
        }
    }
    let length = u16::try_from(scanlines.len()).expect("one stored block");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&scanlines);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &scanlines {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}

// 0 and 255 in a checkerboard of 8 by 8 pixels.
fn spinner_png() -> Vec<u8> {
    encode_png(8, 8, |x, y| {
        if (x + y) % 2 == 0 {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        }
    })
}

fn rect(x: u32, width: u32, height: u32) -> Value {
    json!({"mode": "rect", "rect": {"x": x, "y": 0, "width": width, "height": height}})
}

// The source pack, its task declaring `extra` members.
fn source_entries(extra: Value) -> BTreeMap<String, Vec<u8>> {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": PACKAGE_ID,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task"
    });
    let probe = |id: &str, expected: [u8; 3]| json!({"id": id, "region": rect(0, 1, 1), "expected": expected});
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "LOAD-2 fixture",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home_colour", [0, 0, 255]), probe("page_b_colour", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    for (member, value) in extra.as_object().expect("task members") {
        task[member] = value.clone();
    }
    BTreeMap::from([
        (
            "control.json".to_owned(),
            serde_json::to_vec_pretty(&control).expect("control JSON"),
        ),
        (
            "resources/operations/resources.json".to_owned(),
            br#"{"schema_version":"1.0","resources":[],"resource_count":0}"#.to_vec(),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            serde_json::to_vec_pretty(&task).expect("task JSON"),
        ),
    ])
}

// The copy `package loading --write` makes of a root file that declares the spinner, the backdrop
// and the page `transition`.
fn copy() -> Value {
    json!({
        "schema_version": "actingcommand.pack-loading.v1",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 4.0},
        "max_ms": 30000,
        "anchors": [{"id": "loading/spinner", "template": "loading/spinner.png",
                     "region": rect(0, 8, 8), "threshold": 0.8}],
        "color_probes": [{"id": "loading/backdrop", "region": rect(8, 8, 8),
                          "expected": [8, 8, 8]}],
        "pages": [{"id": "transition", "required": ["loading/spinner", "loading/backdrop"]}]
    })
}

// The pack with the copy `copy` and its template.
fn with_copy(mut entries: BTreeMap<String, Vec<u8>>, copy: &Value) -> BTreeMap<String, Vec<u8>> {
    entries.insert(
        COPY.to_owned(),
        serde_json::to_vec_pretty(copy).expect("copy JSON"),
    );
    entries.insert(SPINNER.to_owned(), spinner_png());
    entries
}

fn admit(
    containment: &mut Containment,
    entries: BTreeMap<String, Vec<u8>>,
) -> Result<&LoadedBundle, ContainmentError> {
    containment.load_content_entries(
        &InstanceId::new("neutral-load2").expect("instance"),
        entries,
        false,
        Instant::now() + Duration::from_secs(60),
    )
}

fn entry_json(bundle: &LoadedBundle, path: &str) -> Value {
    serde_json::from_slice(bundle.entry(path).expect("derived entry")).expect("derived JSON")
}

// The outcome of a refusal of the copy, with its code.
fn refusal(error: &ContainmentError) -> &Outcome {
    match error {
        ContainmentError::LocatedSourceTree { code, outcome } => {
            assert_eq!(code.as_str(), "standard_pack_root_invalid", "{error}");
            outcome
        }
        other => panic!("not a located refusal: {other}"),
    }
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

#[test]
fn pack_loading_compiles_pages_outside_the_page_set() {
    let mut containment = Containment::new();
    let bundle = admit(
        &mut containment,
        with_copy(source_entries(json!({})), &copy()),
    )
    .unwrap_or_else(|error| panic!("the pack with its copy: {error}"));
    let detector = bundle.detector().expect("page detector");
    assert_eq!(
        detector.loading_page_ids().collect::<Vec<_>>(),
        ["neutral/transition"]
    );
    assert_eq!(detector.loading_max_ms(), Some(30_000));
    assert_eq!(
        detector.page_ids().collect::<Vec<_>>(),
        ["neutral/home", "neutral/page_b"]
    );
    assert!(!detector.contains_page("neutral/transition"));
    let pages = entry_json(bundle, PAGES_ENTRY);
    assert_eq!(
        pages["loading"],
        json!({"max_ms": 30000, "pages": [{
            "id": "neutral/transition",
            "required": ["loading/spinner", "loading/backdrop"],
            "optional": [],
            "forbidden": []
        }]})
    );
    let pack = entry_json(bundle, PACK_ENTRY);
    let targets = pack["targets"].as_array().expect("targets");
    let target = |id: &str| {
        targets
            .iter()
            .find(|target| target["id"] == id)
            .unwrap_or_else(|| panic!("no target {id}: {pack}"))
            .clone()
    };
    let spinner = target("loading/spinner");
    assert_eq!(spinner["type"], "template");
    assert_eq!(spinner["template_path"], "loading/spinner.png");
    assert_eq!(spinner["threshold"], 0.8);
    let backdrop = target("loading/backdrop");
    assert_eq!(backdrop["type"], "color");
    // The copy's `color_max_distance`, which the pack's default would not give.
    assert_eq!(backdrop["max_distance"], 4.0);
    assert_eq!(pack["schema_version"], "0.7");
    let evaluator = bundle.evaluator().expect("evaluator");
    for id in ["loading/spinner", "loading/backdrop"] {
        evaluator.target_kind(id).expect("a loading target");
    }
}

#[test]
fn pack_loading_refuses_a_page_or_target_id_the_pack_declares() {
    let mut page = copy();
    page["pages"][0]["id"] = json!("page_b");
    // The pack declares a template of the spinner's id itself.
    let pack_target = json!({"verify_templates": [{"id": "loading/spinner",
        "template": "probe.png", "region": rect(0, 8, 8)}]});
    for (case, entries, problem, pointer) in [
        (
            "a page id of the pack",
            with_copy(source_entries(json!({})), &page),
            "loading_page_collision",
            "/pages/0/id",
        ),
        (
            "a target id of the pack",
            {
                let mut entries = with_copy(source_entries(pack_target), &copy());
                entries.insert(
                    "resources/operations/task/probe.png".to_owned(),
                    spinner_png(),
                );
                entries
            },
            "loading_target_collision",
            "/anchors/0/id",
        ),
    ] {
        let mut containment = Containment::new();
        let error = match admit(&mut containment, entries) {
            Ok(_) => panic!("{case}: admitted"),
            Err(error) => error,
        };
        let outcome = refusal(&error);
        assert_eq!(
            value(outcome, "standard_pack_problem"),
            json!(problem),
            "{case}"
        );
        assert_eq!(value(outcome, "json_pointer"), json!(pointer), "{case}");
        assert_eq!(value(outcome, "path"), json!(COPY), "{case}");
        assert_eq!(value(outcome, "package_id"), json!(PACKAGE_ID), "{case}");
        assert_eq!(value(outcome, "game"), json!("neutral"), "{case}");
    }
}

#[test]
fn pack_loading_refuses_a_missing_template() {
    let mut entries = with_copy(source_entries(json!({})), &copy());
    entries.remove(SPINNER);
    let mut containment = Containment::new();
    let error = match admit(&mut containment, entries) {
        Ok(_) => panic!("admitted without its template"),
        Err(error) => error,
    };
    let outcome = refusal(&error);
    assert_eq!(
        value(outcome, "standard_pack_problem"),
        json!("loading_template_invalid")
    );
    assert_eq!(value(outcome, "json_pointer"), json!("/anchors/0/template"));
    assert_eq!(value(outcome, "path"), json!(COPY));
    assert_eq!(value(outcome, "package_id"), json!(PACKAGE_ID));
}

#[test]
fn pack_without_loading_file_compiles_as_before() {
    let mut containment = Containment::new();
    let bundle = admit(&mut containment, source_entries(json!({})))
        .unwrap_or_else(|error| panic!("the pack: {error}"));
    let detector = bundle.detector().expect("page detector");
    assert_eq!(detector.loading_page_ids().count(), 0);
    assert_eq!(detector.loading_max_ms(), None);
    let pages = entry_json(bundle, PAGES_ENTRY);
    let members = pages
        .as_object()
        .expect("page set")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(members, BTreeSet::from(["pages", "schema_version"]));
    let pack = entry_json(bundle, PACK_ENTRY);
    let ids = pack["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .map(|target| target["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["home_colour", "page_b_colour"]);
}
