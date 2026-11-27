// SPDX-License-Identifier: AGPL-3.0-only

// #463 SEL-F (contracts/lab-recording.md, "A select the package fixes"; register §1.523): a
// recording's select may give the item the package itself looks for, `wanted.fixed` (one name, or
// names in an order of preference) in place of `wanted.default`, and the package generated from it
// declares no variable for the step: its policy document compares the name read in each slot with
// those literals. Recordings are made as stage one makes them (`tap --record` through
// `record_press` with a fake Runtime, its captures and the caller's clock moving only by its
// waits), on S2-6's synthetic list: 96x64 frames of flat gray, two slots, a scripted OCR provider
// that reads item_a (ALPHA) and item_b (BRAVO) from marker pixels; the press is at (20, 20), in
// slot 0. No device is used; the tests read no clock and never sleep. Every expected value comes
// from the work order's lines, the contract's documents and the values above.
// - `self_t1_a_fixed_select_declares_no_variable_and_its_policy_compares_with_the_literal` (1)
// - `self_t2_a_select_a_person_chooses_with_generates_what_main_generates` (2)
// - `self_t3_a_fixed_select_on_a_list_without_its_item_ends_on_the_on_empty_key` (3)
// - `self_t4_a_fixed_select_presses_the_first_name_of_its_order_the_screen_shows` (4)
// - `self_t6_a_package_with_a_variable_is_converted_without_recording_again` (6)
// - `self_t7_a_root_whose_package_lost_its_variable_is_checked_as_one_without_variables` (7)
// - `self_review_each_wrong_fixed_declaration_is_refused_with_its_word` (the review's table)
// Addendum 1 (ruling 8, `rd-1123/brief-463-self-fix1.md`; "Converting a package from its own
// files"), the package route, `fix_package_select`, on packages `record stop` generated here:
// - `self_t8_a_package_converted_from_its_files_is_what_a_fixed_recording_generates` (8)
// - `self_t9_names_given_in_order_convert_to_what_a_recording_fixing_them_generates` (9)
// - `self_t10_each_refusal_of_the_conversion_has_its_word` (10)
// - `self_t12_a_package_converted_twice_refuses_the_second_time` (12)
// Test 5, a fixed select over a scan, and the scan fixtures of tests 8 and 9 are
// `gate_463_self_scan`; test 11 and the command line's side of test 10 are actinglab's
// `gate_463_self` and actinglab-mcp's `gate_463_self`.

use crate::recording::crosscheck::admit;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest, PointPress,
    RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, RegressionVerdict, SelectFix, SelectFixRequest,
    fix_package_select, record_mark, record_press, record_ready, record_start, record_stop,
    run_regression_case,
};
use actingcommand_contract::{
    InputAction, InputFrameReference, InstanceFactContext, InstanceFactSnapshot, LabError,
    LabResult, control_declares_variables,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    OfflineDecision, PreparedContainedTask, SelectionDryRun, dry_run_select,
    simulate_contained_task_at,
};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_resource_tooling::{
    StandardPackCheckRequest, StandardPackCheckResponse, check_standard_pack_root,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_self";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGE: &str = "state/page";
const OPERATION: &str = "step_01_select";
const TASK: &str = "resources/operations/neutral_task/task.json";
const POLICY: &str = "resources/operations/neutral_task/policies/step_01_select.json";
const NEXT_SEED: u64 = 0x0463_00f0;
// The wanted value's name, and the key under which a run reads the package's variable of it.
const WANTED: &str = "list_item";
const VARIABLE_KEY: &str = "session.dispatch.parameter.list_item";
// The instant of the kernel's dry runs below.
const DRY_RUN_UNIX_MS: u64 = 1_700_000_100_000;
// The point stage one taps on the list: inside slot 0.
const TAP: (i32, i32) = (20, 20);
// The key of the select's empty end, `<task_id>_not_found` by default.
const NOT_FOUND: &str = "neutral_task_not_found";

// The gray value of a marker pixel and the name the provider reads from it.
const ALPHA: u8 = 211;
const BRAVO: u8 = 212;
const MARKERS: [(u8, &str); 2] = [(ALPHA, "ALPHA"), (BRAVO, "BRAVO")];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The slots' rectangles and the regions their names are read in.
const SLOTS: [RecordRect; 2] = [rect(8, 8, 32, 32), rect(48, 8, 32, 32)];
const NAMES: [RecordRect; 2] = [rect(10, 28, 28, 8), rect(50, 28, 28, 8)];

// Reads one block per marker pixel of the requested region at confidence 0.9; a region without a
// marker reads nothing, with no confidence.
#[derive(Debug)]
struct NamesOcr;

impl VisionProvider for NamesOcr {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let region = request.region;
        let (right, bottom) = (region.x + region.width, region.y + region.height);
        let mut blocks = Vec::new();
        for y in region.y..bottom {
            for x in region.x..right {
                let index = (y as usize * request.frame.width as usize + x as usize) * 3;
                let pixel = request.frame.rgb8_pixels.get(index).copied();
                if let Some((_, name)) = MARKERS.iter().find(|(value, _)| Some(*value) == pixel) {
                    blocks.push(OcrProviderTextBlock {
                        text: (*name).to_owned(),
                        rect: PackRect {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        },
                        confidence: Some(0.9),
                    });
                }
            }
        }
        let joined = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            confidence: (!blocks.is_empty()).then_some(0.9),
            text: joined,
            blocks,
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// A deterministic generator of neutral gray noise below every marker value.
struct Noise(u64);

impl Noise {
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// The PNG of a gray screen, row-major values drawn with equal red, green and blue.
fn png(values: &[u8]) -> Vec<u8> {
    let pixels = values
        .iter()
        .flat_map(|value| [*value; 3])
        .collect::<Vec<_>>();
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// The list: item_a's marker in slot 0, item_b's in slot 1.
fn list() -> Vec<u8> {
    list_of([ALPHA, BRAVO])
}

// The list showing in slot `k` the name of the marker given for it.
fn list_of(markers: [u8; 2]) -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for (slot, marker) in (0_u32..).zip(markers) {
        values[(30 * WIDTH + 12 + 40 * slot) as usize] = marker;
    }
    png(&values)
}

// The next screen: a noise patch at (40, 46), with no marker value.
fn next_screen() -> Vec<u8> {
    let mut noise = Noise(NEXT_SEED);
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for row in 0..12 {
        for column in 0..12 {
            values[((46 + row) * WIDTH + 40 + column) as usize] = noise.between(1, 199);
        }
    }
    png(&values)
}

fn frame_of(screen: Vec<u8>) -> Frame {
    Frame::from_png(screen, CaptureBackendName::AdbScreencap).expect("frame")
}

// The page mark of the list: a color mark on a flat area no marker reaches.
fn page_mark() -> Value {
    json!({"id": PAGE, "family": "color", "region": rect(2, 50, 4, 4)})
}

// The wanted value of the variable form: the variable's default.
fn chosen_by_a_person(default: &str) -> Value {
    json!({"name": WANTED, "default": default})
}

// The wanted value the package fixes: the names it looks for, in their order.
fn fixed(names: &[&str]) -> Value {
    json!({"name": WANTED, "fixed": names})
}

// The select over the two slots with the wanted value `wanted` and, when given, `pressed`.
fn select(wanted: Value, pressed: Option<&str>) -> Value {
    let mut declared = json!({
        "slots": [{"rect": SLOTS[0], "name": NAMES[0]}, {"rect": SLOTS[1], "name": NAMES[1]}],
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": [{"id": "item_a", "aliases": ["ALPHA"]}, {"id": "item_b", "aliases": ["BRAVO"]}],
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": wanted
    });
    if let Some(name) = pressed {
        declared["pressed"] = json!(name);
    }
    declared
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

fn inside(point: (i32, i32), area: RecordRect) -> bool {
    point.0 >= area.x
        && point.1 >= area.y
        && point.0 < area.x + area.width
        && point.1 < area.y + area.height
}

// What the command hands the recording for a performed input: no carrier package, no Runtime
// frames.
fn receipt() -> CommitClickRequest {
    CommitClickRequest {
        effect: ClickEffect::Performed,
        has_failure: false,
        carrier_package: None,
        req_id: None,
        correlation_id: None,
        action_id: None,
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

/// The fake Runtime's captures and the caller's clock: the screens its captures take, in order,
/// and the clock, which moves only by the waits.
struct Screens {
    shots: VecDeque<Vec<u8>>,
    taken: u32,
    now: Duration,
}

impl RecordFramePorts for Screens {
    fn wait(&mut self, duration: Duration) {
        self.now += duration;
    }

    fn elapsed(&mut self) -> Duration {
        self.now
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        let shot = self
            .shots
            .pop_front()
            .expect("a capture the test did not script");
        self.taken += 1;
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png: shot,
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
            input_frame: Some(InputFrameReference {
                frame_id: serde_json::from_value(json!(format!("frame_{:032x}", self.taken)))
                    .expect("a frame id"),
                width: WIDTH,
                height: HEIGHT,
            }),
        })
    }
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and
// its Lab recording, before any step, with the scripted OCR provider. The lock is held as one
// command holds it and dropped before the directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_self", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(NamesOcr)),
            temp,
        };
        fs::write(
            old_record_path(&bench.state(), INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": "active",
                "started_at_unix_ms": STARTED_AT
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        record_start(
            &bench.lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `tap 20 20 --record` on `first`, as stage one records it: the fake Runtime performs the
    // input; its first capture is `first` (step 1, f0001), its one capture after the press the
    // next screen (step 2, f0002).
    fn press_on(&self, first: Vec<u8>) {
        let pressed = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap {
                point: RecordPoint { x: TAP.0, y: TAP.1 },
            }),
            after: AfterCapture {
                still_ms: Some(0),
                ..AfterCapture::for_press(Some(800))
            },
        };
        let mut device = Screens {
            shots: VecDeque::from([first, next_screen()]),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        record_press(&self.lock, &pressed, &mut send, &mut device).unwrap_or_else(|error| {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    }

    fn mark(&self, value: Value) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), false, &self.ocr)
    }

    // `record mark` of `value`, which must stand.
    fn marked(&self, value: Value) -> MarkOutcome {
        self.mark(value).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // Step 2, the next screen, recognized by its own template mark.
    fn mark_the_next_screen(&self) {
        self.marked(json!({
            "step": 2,
            "add": [{"id": "ui/next", "family": "template", "region": rect(40, 46, 12, 12),
                     "search": rect(40, 46, 12, 12)}]
        }));
    }

    // `record ready`, read as the command reads it: the instance's recording, no option.
    fn ready(&self) -> Value {
        let outcome = record_ready(
            &self.state(),
            INSTANCE,
            None,
            &RecordStopOptions::default(),
            &self.ocr,
        )
        .unwrap_or_else(|error| {
            panic!(
                "record ready: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
        serde_json::to_value(outcome).expect("the answer as JSON")
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self) -> Value {
        record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr)
            .map(|outcome| {
                let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
                lab["lab_status"] = json!(outcome.lab_status);
                lab
            })
            .unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            })
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }

    // The session file's status changed as the `record` command changes it at the stop, so that
    // the stopped recording takes `record mark` ("Marking after stop").
    fn session_status(&self, status: &str) {
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!(status);
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
    }

    // The press on `first`, step 1 marked with its page mark and the select `declared`, step 2
    // the next screen, then `record stop`, which must generate the package.
    fn generated(&self, first: Vec<u8>, declared: Value) -> Value {
        self.press_on(first);
        self.marked(json!({"step": 1, "add": [page_mark()], "select": declared}));
        self.mark_the_next_screen();
        let lab = self.stop();
        assert_eq!(lab["lab_status"], "generated", "{lab}");
        lab
    }

    // The conversion route (contracts/lab-recording.md, "A select the package fixes", "Converting
    // a package"): on the stopped recording, step 1's select declared again as the recording holds
    // it, its wanted value's default made the one name the package fixes; nothing is recorded
    // again and no device is reached.
    fn converted(&self) -> MarkOutcome {
        let mut declaration = self.recording()["steps"][0]["select"]["declaration"].clone();
        let name = declaration["wanted"]["name"].clone();
        let default = declaration["wanted"]["default"].clone();
        declaration["wanted"] = json!({"name": name, "fixed": [default]});
        self.marked(json!({"step": 1, "select": declaration}))
    }

    // The entries of a package written as its content directory under `<temporary>/<name>`.
    fn package_dir(&self, name: &str, entries: &BTreeMap<String, Vec<u8>>) -> PathBuf {
        let root = self.temp.path().join(name);
        for (path, bytes) in entries {
            let file = path
                .split('/')
                .fold(root.clone(), |dir, part| dir.join(part));
            fs::create_dir_all(file.parent().expect("a parent")).expect("package directories");
            fs::write(file, bytes).expect("package file");
        }
        root
    }
}

// The entries of the package `record stop` wrote.
fn package(lab: &Value) -> BTreeMap<String, Vec<u8>> {
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let container = if lab["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    expand_content_container(&bytes, container, ContainmentLimits::default())
        .expect("package container")
}

fn document(entries: &BTreeMap<String, Vec<u8>>, path: &str) -> Value {
    serde_json::from_slice(entries.get(path).expect("the document")).expect("document JSON")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("UTF-8 text")
}

// The bytes the generator writes a document with: pretty JSON and a final line feed.
fn written(value: &Value) -> String {
    let mut bytes = serde_json::to_vec_pretty(value).expect("JSON");
    bytes.push(b'\n');
    text(&bytes)
}

// The select operation of the package's task.
fn select_operation(task: &Value) -> Value {
    task["operations"]
        .as_array()
        .and_then(|operations| {
            operations
                .iter()
                .find(|operation| operation["id"] == OPERATION)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no select operation in {task}"))
}

// The policy document of step 1's select as the contract writes it, its gates and terms
// `gates` and `scoring` ("What `record stop` generates"): its id, its layout, its outcome keys
// (`<task_id>_step_01_<kind>`, the empty one the default `on_empty` key), the field `name` over
// the two names, `none_allowed` with one candidate, the lowest candidate id first.
fn policy(facts: Value, gates: Value, scoring: Value) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "step_01_select",
        "applies_to": {
            "candidate_layout_id": "layout/step_01",
            "outcome_keys": {
                "selected": "neutral_task_step_01_selected",
                "empty": NOT_FOUND,
                "insufficient": "neutral_task_step_01_insufficient",
                "ambiguous": "neutral_task_step_01_ambiguous",
                "unknown": "neutral_task_step_01_unknown"
            }
        },
        "fields": [{"name": "name", "value_type": {"type": "enum_string",
                                                   "allowed": ["item_a", "item_b"]}}],
        "facts": facts,
        "gates": gates,
        "scoring": scoring,
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// The fixed form ("A select the package fixes"): the gate `wanted.list_item` keeps the names
// `allowed` (`string_in` over the field, an unknown name dropped) and the term `rank.list_item`
// scores each by its place, `entries` its lookup, under the weight −1000; no fact.
fn fixed_policy(allowed: Value, entries: Value) -> Value {
    policy(
        json!([]),
        json!([{
            "gate_id": "wanted.list_item",
            "predicate": {"kind": "string_in", "value": {"source": "field", "field": "name"},
                          "allowed": allowed},
            "on_unknown": {"kind": "drop_candidate"}
        }]),
        json!([{
            "term_id": "rank.list_item",
            "value": {"source": "field", "field": "name"},
            "transform": {"kind": "lookup", "entries": entries},
            "weight_milli": -1000,
            "on_unknown": {"kind": "drop_candidate"}
        }]),
    )
}

// One entry of the fixed form's lookup: the name and its place, 1 for the first.
fn place(name: &str, rank: i64) -> Value {
    json!({"key": {"type": "string", "value": name}, "value_milli": rank})
}

// Today's golden of the variable form (#463 S2-6 fix round 1; "What `record stop` generates"):
// the record list of the variable `list_item` declared, its gate a rank of at least 1, its term the
// rank under the weight −1000, both aborting on an unknown.
fn variable_policy() -> Value {
    let rank = json!({"source": "keyed_fact", "fact_key": VARIABLE_KEY, "key_field": "name",
                      "value_column": "rank", "absent": {"type": "integer", "value": 0}});
    policy(
        json!([{
            "fact_key": VARIABLE_KEY,
            "max_age_ms": 31_536_000_000_u64,
            "minimum_confidence_milli": 1000,
            "value_type": {"type": "record_list", "key_column": "key",
                           "columns": {"rank": {"type": "integer"}}}
        }]),
        json!([{
            "gate_id": "wanted.list_item",
            "predicate": {"kind": "integer_at_least", "value": rank, "threshold": 1},
            "on_unknown": {"kind": "abort_evaluation"}
        }]),
        json!([{
            "term_id": "rank.list_item",
            "value": rank,
            "transform": {"kind": "identity"},
            "weight_milli": -1000,
            "on_unknown": {"kind": "abort_evaluation"}
        }]),
    )
}

// Today's golden of the variable form's control member ("The wanted value is a variable of the
// package"): the `ordered` variable `list_item`, its options the names titled by their aliases, 1
// to 2 of them, its default the select's default alone.
fn variable_declaration(default: &str) -> Value {
    let title = |text: &str| json!({"en-US": text});
    json!([{
        "name": WANTED,
        "type": "ordered",
        "title": title(WANTED),
        "options": [{"value": "item_a", "title": title("ALPHA")},
                    {"value": "item_b", "title": title("BRAVO")}],
        "min_count": 1,
        "max_count": 2,
        "default": [default]
    }])
}

// The kernel's dry run of the package's select step on `frame` as a run handed no value reads it
// (every variable the package declares at its default), at `DRY_RUN_UNIX_MS` on an instance
// snapshot without records.
fn dry_run(prepared: &PreparedContainedTask, frame: &Frame) -> SelectionDryRun {
    let snapshot = InstanceFactSnapshot {
        snapshot_id: "gate.self".to_owned(),
        ledger_position: 0,
        context: InstanceFactContext {
            instance_id: "gate.self".to_owned(),
            server_id: String::new(),
            game_id: String::new(),
        },
        records: Vec::new(),
    };
    let parameters = prepared.default_run_values();
    dry_run_select(
        prepared,
        OPERATION,
        frame,
        (&snapshot, DRY_RUN_UNIX_MS),
        &parameters,
        None,
    )
    .expect("the dry run of the select step")
}

// The slot the dry run chooses on the screen `screen`; `None` when it chooses none.
fn chosen(prepared: &PreparedContainedTask, screen: Vec<u8>) -> Option<u32> {
    let dry = dry_run(prepared, &frame_of(screen));
    dry.decision.selected.first().and_then(|id| {
        dry.candidate_projection
            .candidates()
            .iter()
            .find(|candidate| candidate.id == *id)
            .map(|candidate| candidate.instance_index)
    })
}

// The point the offline simulation of the package taps on the screen `screen`, which must be the
// select step's tap.
fn tapped(prepared: &PreparedContainedTask, screen: Vec<u8>) -> (i32, i32) {
    let decision = simulate_contained_task_at(prepared, vec![frame_of(screen)], 1)
        .expect("simulation")
        .decision;
    match decision {
        OfflineDecision::WouldClick {
            operation_label,
            action: InputAction::Tap { x, y },
            ..
        } => {
            assert_eq!(operation_label, OPERATION);
            (x, y)
        }
        other => panic!("not a tap: {other:?}"),
    }
}

// Each case of the cases' directory `record stop` answered, by its directory's name: its
// `case.json` and its scene.
fn written_cases(lab: &Value) -> BTreeMap<String, (Vec<u8>, Vec<u8>)> {
    let dir = PathBuf::from(
        lab["cases"]["directory"]
            .as_str()
            .unwrap_or_else(|| panic!("no cases directory in {lab}")),
    );
    let mut cases = BTreeMap::new();
    for entry in fs::read_dir(&dir).expect("the cases' directory") {
        let path = entry.expect("an entry").path();
        let name = path
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .into_owned();
        cases.insert(
            name,
            (
                fs::read(path.join("case.json")).expect("case.json"),
                fs::read(path.join("scene.png")).expect("scene.png"),
            ),
        );
    }
    cases
}

// The work order's lines on the cases: every case `record stop` wrote for `lab`, read from disk,
// passes when the one runner of the case contract runs it on the package at `dir`. (A case holds
// pages and targets and sets no value in either form, so the review had the earlier check that it
// names no variable taken out: it showed nothing.)
fn cases_pass(bench: &Bench, lab: &Value, dir: &Path) -> Vec<String> {
    let cases = written_cases(lab);
    assert!(!cases.is_empty(), "{lab}");
    for (id, (case_json, scene)) in &cases {
        let verdict = run_regression_case(dir, case_json, scene, &bench.ocr);
        assert_eq!(
            verdict.verdict,
            RegressionVerdict::Passed,
            "{id}: {verdict:?}"
        );
    }
    cases.into_keys().collect()
}

// The work order's test 1: a select that fixes item_b, the demonstration pressing item_a in slot
// 0: the mark stands, its self-test passed (the pressed name on slot 0, the fixed item on slot 1);
// `record ready` lists nothing; `record stop` generates with every self-check passing, the first
// decision on slot 1, `lab.selects` naming the fixed names; `control.json` declares no
// `variables`; the policy document is the fixed form, its gate comparing with the literal item_b;
// the admitted package declares no variable and takes no value of the name (nothing a console can
// list or set); and its regression cases pass.
#[test]
fn self_t1_a_fixed_select_declares_no_variable_and_its_policy_compares_with_the_literal() {
    let bench = Bench::new();
    bench.press_on(list());
    let marked = bench.marked(json!({
        "step": 1,
        "add": [page_mark()],
        "select": select(fixed(&["item_b"]), Some("item_a"))
    }));
    let checked = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(checked["self_test"]["status"], "passed", "{checked}");
    assert_eq!(
        checked["self_test"]["frames"],
        json!([{"frame_id": "f0001", "pressed": {"result": "selected", "slot": 0},
                "default": {"result": "selected", "slot": 1}}]),
        "{checked}"
    );
    bench.mark_the_next_screen();
    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");

    let lab = bench.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(lab["first_decision"]["status"], "would_click", "{lab}");
    assert_eq!(lab["first_decision"]["slot"], 1, "{lab}");
    assert_eq!(
        lab["selects"][0]["wanted"],
        json!({"name": WANTED, "fixed": ["item_b"]}),
        "{lab}"
    );
    assert_eq!(lab["cases"]["status"], "written", "{lab}");
    let entries = package(&lab);
    let control = document(&entries, "control.json");
    assert_eq!(control.get("variables"), None, "{control}");
    assert_eq!(
        text(&entries[POLICY]),
        written(&fixed_policy(
            json!(["item_b"]),
            json!([place("item_b", 1)])
        ))
    );
    let prepared =
        admit(&entries, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert!(prepared.declared_variables().is_empty());
    assert!(
        prepared
            .given_variable_values(&[(WANTED.to_owned(), json!(["item_a"]))])
            .is_err(),
        "a value of the name is no value of the package"
    );
    let dir = bench.package_dir("package", &entries);
    cases_pass(&bench, &lab, &dir);
}

// The work order's test 2: the same recording with the variable form (the default item_b): the
// recording holds the declaration byte for byte as the request gave it, no `fixed` member; the
// control's `variables` and the policy document are today's golden byte for byte; `lab.selects`
// names `{name, default}` as before.
#[test]
fn self_t2_a_select_a_person_chooses_with_generates_what_main_generates() {
    let bench = Bench::new();
    let given = select(chosen_by_a_person("item_b"), Some("item_a"));
    let lab = bench.generated(list(), given.clone());

    let recorded = bench.recording()["steps"][0]["select"]["declaration"].clone();
    assert_eq!(
        serde_json::to_string(&recorded).expect("JSON text"),
        serde_json::to_string(&given).expect("JSON text")
    );
    assert!(
        !text(&bench.recording_bytes()).contains("\"fixed\""),
        "the recording writes no fixed member"
    );
    let entries = package(&lab);
    let control = document(&entries, "control.json");
    assert_eq!(
        written(&control["variables"]),
        written(&variable_declaration("item_b"))
    );
    assert_eq!(text(&entries[POLICY]), written(&variable_policy()));
    assert_eq!(
        lab["selects"][0]["wanted"],
        json!({"name": WANTED, "default": "item_b"}),
        "{lab}"
    );
}

// The work order's test 3: the list's first screen shows item_a in both slots and the package looks
// for item_b, the demonstration pressing item_a in slot 0. The fixed form and, as today, the
// variable form with the default item_b alike: the first decision is the run's normal end with the
// `on_empty` key; the task maps that key on the step's page and the select operation declares it;
// the kernel's dry run selects nothing and decides that key.
#[test]
fn self_t3_a_fixed_select_on_a_list_without_its_item_ends_on_the_on_empty_key() {
    for wanted in [fixed(&["item_b"]), chosen_by_a_person("item_b")] {
        let bench = Bench::new();
        let lab = bench.generated(
            list_of([ALPHA, ALPHA]),
            select(wanted.clone(), Some("item_a")),
        );

        assert_eq!(
            lab["first_decision"]["status"], "would_end_empty",
            "{wanted}: {lab}"
        );
        assert_eq!(
            lab["first_decision"]["outcome_key"], NOT_FOUND,
            "{wanted}: {lab}"
        );
        let entries = package(&lab);
        let task = document(&entries, TASK);
        assert!(
            task["scheduling_outcome"]["mappings"]
                .as_array()
                .expect("the mappings")
                .contains(
                    &json!({"outcome_key": NOT_FOUND, "effect": "no_designated_effect",
                                  "terminal_pages": ["step_01"]})
                ),
            "{wanted}: {task}"
        );
        assert_eq!(
            select_operation(&task)["select"]["on_empty"],
            json!({"outcome_key": NOT_FOUND}),
            "{wanted}"
        );
        let prepared =
            admit(&entries, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
        let dry = dry_run(&prepared, &frame_of(list_of([ALPHA, ALPHA])));
        assert!(
            dry.decision.selected.is_empty(),
            "{wanted}: {:?}",
            dry.decision.selected
        );
        assert_eq!(dry.decision.outcome_key, NOT_FOUND, "{wanted}");
    }
}

// The work order's test 4: names in an order of preference. Fixed `[item_b, item_a]`: on the list
// (item_a in slot 0, item_b in slot 1) the first decision and the offline simulation press item_b's
// slot 1, although item_a stands in the slot of the lower candidate id; on a list without item_b
// the next name of the order, item_a, is chosen. The order reversed chooses item_a's slot 0 on the
// same list. The policy document lists the names in their order with their places.
#[test]
fn self_t4_a_fixed_select_presses_the_first_name_of_its_order_the_screen_shows() {
    let bench = Bench::new();
    let lab = bench.generated(list(), select(fixed(&["item_b", "item_a"]), Some("item_a")));
    assert_eq!(lab["first_decision"]["status"], "would_click", "{lab}");
    assert_eq!(lab["first_decision"]["slot"], 1, "{lab}");
    let entries = package(&lab);
    assert_eq!(
        text(&entries[POLICY]),
        written(&fixed_policy(
            json!(["item_b", "item_a"]),
            json!([place("item_b", 1), place("item_a", 2)])
        ))
    );
    let prepared =
        admit(&entries, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let point = tapped(&prepared, list());
    assert!(inside(point, SLOTS[1]), "{point:?}");
    assert_eq!(chosen(&prepared, list()), Some(1));
    assert_eq!(chosen(&prepared, list_of([ALPHA, ALPHA])), Some(0));

    let reversed = Bench::new();
    let reversed_lab = reversed.generated(list(), select(fixed(&["item_a", "item_b"]), None));
    assert_eq!(reversed_lab["first_decision"]["slot"], 0, "{reversed_lab}");
    let reversed_prepared = admit(&package(&reversed_lab), &reversed.ocr)
        .unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert_eq!(chosen(&reversed_prepared, list()), Some(0));
    assert_eq!(chosen(&reversed_prepared, list_of([BRAVO, BRAVO])), Some(0));
}

// The work order's test 6 (ruling 4): a package generated with the variable form (default item_b),
// its recording stopped, is converted by the route the contract names: the select declared again
// on the stopped recording as the recording holds it, its default made the one fixed name, then
// `record stop`, which generates it again; nothing is recorded again. After it the control declares
// no variable, the policy document is the fixed form of item_b, the converted package chooses on
// every screen what the old one chose handed no value (slot 1 on the list, the `on_empty` key where
// item_b is absent), its simulation taps the slot the old one tapped, and its regression cases are
// those of before, each passing from disk.
#[test]
fn self_t6_a_package_with_a_variable_is_converted_without_recording_again() {
    let bench = Bench::new();
    let before = bench.generated(list(), select(chosen_by_a_person("item_b"), Some("item_a")));
    bench.session_status("stopped");
    let old = package(&before);
    assert_eq!(
        document(&old, "control.json")["variables"],
        variable_declaration("item_b")
    );
    let old_prepared =
        admit(&old, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));

    let converted = bench.converted();
    assert!(converted.after_stop, "marked after the stop");
    let after = bench.stop();

    assert_eq!(after["lab_status"], "regenerated", "{after}");
    let new = package(&after);
    assert_eq!(
        document(&new, "control.json").get("variables"),
        None,
        "the variable is gone"
    );
    assert_eq!(
        text(&new[POLICY]),
        written(&fixed_policy(
            json!(["item_b"]),
            json!([place("item_b", 1)])
        ))
    );
    for member in ["status", "slot"] {
        assert_eq!(
            after["first_decision"][member], before["first_decision"][member],
            "{after}"
        );
    }
    let new_prepared =
        admit(&new, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    for screen in [list(), list_of([ALPHA, ALPHA]), list_of([BRAVO, ALPHA])] {
        let old_run = dry_run(&old_prepared, &frame_of(screen.clone()));
        let new_run = dry_run(&new_prepared, &frame_of(screen.clone()));
        assert_eq!(new_run.decision.outcome_key, old_run.decision.outcome_key);
        assert_eq!(
            chosen(&new_prepared, screen.clone()),
            chosen(&old_prepared, screen)
        );
    }
    assert_eq!(chosen(&new_prepared, list()), Some(1));
    for prepared in [&old_prepared, &new_prepared] {
        let point = tapped(prepared, list());
        assert!(inside(point, SLOTS[1]), "{point:?}");
    }
    let ids = |lab: &Value| {
        lab["cases"]["ids"]
            .as_array()
            .unwrap_or_else(|| panic!("no case ids in {lab}"))
            .iter()
            .map(|id| id.as_str().expect("a case id").to_owned())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(ids(&after), ids(&before), "{after}");
    let dir = bench.package_dir("converted", &new);
    let passed = cases_pass(&bench, &after, &dir);
    assert_eq!(passed.into_iter().collect::<BTreeSet<_>>(), ids(&after));
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

// A package directory of an installed root as `gate_463_v1` writes one (its control, with
// `variables` when given, its resource table and one task from page `home` to page `page_b`).
fn write_pack(directory: &Path, package_id: &str, variables: Option<&Value>) {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(variables) = variables {
        control["variables"] = variables.clone();
    }
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "SEL-F fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
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
    write_json(&directory.join("control.json"), &control);
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    write_json(
        &directory.join("resources/operations/task/task.json"),
        &task,
    );
}

// An installed game root `<temporary>/<name>/packages/neutral/`: `neutral.a` declaring
// `variables` (when given) and `neutral.b` declaring none, listed in `bundle.json`, and a version 2
// `goals.json` whose second entry places the variable `list_item`.
fn installed(temp: &Path, name: &str, variables: Option<&Value>) -> PathBuf {
    let game_root = temp.join(name).join("packages").join("neutral");
    let mut listed = Vec::new();
    for (package_id, declared) in [("neutral.a", variables), ("neutral.b", None)] {
        write_pack(&game_root.join(package_id), package_id, declared);
        listed.push(json!({
            "package_id": package_id, "server": "test", "entry_task_id": "task",
            "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
        }));
    }
    write_json(
        &game_root.join("bundle.json"),
        &json!({"schema_version": "actingcommand.bundle.v4", "game": "neutral", "packs": listed}),
    );
    write_json(
        &game_root.join("goals.json"),
        &json!({
            "schema_version": "actingcommand.goals.v3",
            "game": "neutral",
            "entries": [
                {"name": "picks", "type": "heading", "title": {"en": "Picks"}},
                {"name": WANTED, "type": "variable"}
            ]
        }),
    );
    game_root
}

fn check(game_root: &Path) -> StandardPackCheckResponse {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
    .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message))
}

// Each `standard_pack_root_invalid` warning of an answer by its problem and pointer.
fn root_problems(checked: &StandardPackCheckResponse) -> Vec<(Value, Value)> {
    checked
        .warnings
        .iter()
        .filter(|warning| warning.code().as_str() == "standard_pack_root_invalid")
        .map(|warning| {
            let values = json!(warning.values());
            (
                values["standard_pack_problem"].clone(),
                values["json_pointer"].clone(),
            )
        })
        .collect()
}

// The work order's test 7 (ruling 6): the control of the package before and after its conversion
// (test 6's route). The rule `standard-pack ready` and the installer's `place` read a package's
// need of `package` 14 by (`control_declares_variables`) holds for the old control and not for the
// converted one. `standard-pack check` of a root whose package declares the old variable, which
// `goals.json` places: no problem, and the variable listed as applying; the same root with the
// converted control: as a root whose packages declare no variable, no listing, and the placement
// is `goal_variable_unknown`, as a placement of any name no package declares; the root is checked.
#[test]
fn self_t7_a_root_whose_package_lost_its_variable_is_checked_as_one_without_variables() {
    let bench = Bench::new();
    let before = bench.generated(list(), select(chosen_by_a_person("item_b"), Some("item_a")));
    bench.session_status("stopped");
    let old_control = package(&before)["control.json"].clone();
    bench.converted();
    let after = bench.stop();
    assert_eq!(after["lab_status"], "regenerated", "{after}");
    let new_control = package(&after)["control.json"].clone();

    assert!(control_declares_variables(&old_control));
    assert!(!control_declares_variables(&new_control));

    let declared = |control: &[u8]| {
        serde_json::from_slice::<Value>(control)
            .expect("control JSON")
            .get("variables")
            .cloned()
    };
    let roots = bench.temp.path();
    let old_root = installed(roots, "old", declared(old_control.as_slice()).as_ref());
    let old = check(&old_root);
    assert_eq!(old.status, "checked");
    assert!(root_problems(&old).is_empty(), "{:?}", root_problems(&old));
    assert_eq!(
        serde_json::to_value(&old.variable_provenance).expect("the listing"),
        json!({"pack": [WANTED], "model": []})
    );

    let new_root = installed(roots, "new", declared(new_control.as_slice()).as_ref());
    let new = check(&new_root);
    let plain = check(&installed(roots, "plain", None));
    assert_eq!(new.status, "checked");
    assert!(new.variable_provenance.is_none());
    assert_eq!(
        root_problems(&new),
        [(json!("goal_variable_unknown"), json!("/entries/1/name"))]
    );
    assert_eq!(root_problems(&new), root_problems(&plain));
    assert!(plain.variable_provenance.is_none());
}

// The review's table of refusals of the recording's declaration ("A select the package fixes",
// "The member"): on the list's first screen, step 1's select with a `wanted` that gives both
// `default` and `fixed`, neither, an empty `fixed`, three names where the select has two, a name
// twice: each `validation_failed`; a name that is none of the select's names: `record_select_invalid`
// with `default_not_in_names`. Each refusal leaves the recording's bytes as they were.
#[test]
fn self_review_each_wrong_fixed_declaration_is_refused_with_its_word() {
    let bench = Bench::new();
    bench.press_on(list());
    let before = bench.recording_bytes();
    let both = json!({"name": WANTED, "default": "item_b", "fixed": ["item_b"]});
    let neither = json!({"name": WANTED});
    let cases = [
        (both, "validation_failed", None),
        (neither, "validation_failed", None),
        (fixed(&[]), "validation_failed", None),
        (
            fixed(&["item_a", "item_b", "item_c"]),
            "validation_failed",
            None,
        ),
        (fixed(&["item_b", "item_b"]), "validation_failed", None),
        (
            fixed(&["item_c"]),
            "record_select_invalid",
            Some("default_not_in_names"),
        ),
    ];
    for (wanted, code, problem) in cases {
        let declared = select(wanted.clone(), Some("item_a"));
        let error = match bench.mark(json!({"step": 1, "add": [page_mark()], "select": declared})) {
            Ok(_) => panic!("{wanted}: the mark stood"),
            Err(error) => error,
        };
        assert_eq!(error.code, code, "{wanted}: {}", error.message);
        if let Some(problem) = problem {
            let details = error.details.as_ref().expect("the refusal's details");
            assert_eq!(details["select_problem"], problem, "{wanted}");
        }
        assert!(
            bench.recording_bytes() == before,
            "{wanted}: the recording is unchanged"
        );
    }
}

// #463 SEL-F, Addendum 1: `package fix-select` of the variable `variable` on the package
// `entries`, fixed to `names` when they are given (the package route, `fix_package_select`).
pub(crate) fn fix_select(
    entries: &BTreeMap<String, Vec<u8>>,
    variable: &str,
    names: Option<&[&str]>,
    ocr: &LabOcr,
) -> LabResult<SelectFix> {
    let fixed = names.map(|given| {
        given
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    });
    fix_package_select(
        entries,
        &SelectFixRequest {
            variable: variable.to_owned(),
            fixed,
        },
        ocr,
    )
}

// The conversion of the variable `list_item` on `entries`, which must stand, and the package with
// the files it changed written over the old ones, as the command writes them.
pub(crate) fn convert_package(
    entries: &BTreeMap<String, Vec<u8>>,
    names: Option<&[&str]>,
    ocr: &LabOcr,
) -> (SelectFix, BTreeMap<String, Vec<u8>>) {
    let fix = fix_select(entries, WANTED, names, ocr).unwrap_or_else(|error| {
        panic!(
            "package fix-select: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });
    let mut changed = entries.clone();
    for file in &fix.files {
        changed.insert(file.file.clone(), file.bytes.clone());
    }
    (fix, changed)
}

// The note the conversion adds to the task's `provenance.select_fixes` for the select step
// `operation`: its variable `list_item`, whose default was `default`, fixed to `fixed`.
pub(crate) fn conversion_note(operation: &str, default: &[&str], fixed: &[&str]) -> Value {
    json!({
        "operation": "package fix-select",
        "step_operation": operation,
        "variable": WANTED,
        "default": default,
        "fixed": fixed
    })
}

// A task document without what a converted package's task may differ in from a recorded one's:
// its provenance and each operation's.
fn without_provenance(task: &Value) -> Value {
    let mut stripped = task.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.shift_remove("provenance");
    }
    if let Some(operations) = stripped["operations"].as_array_mut() {
        for operation in operations {
            if let Some(object) = operation.as_object_mut() {
                object.shift_remove("provenance");
            }
        }
    }
    stripped
}

// Ruling 8's comparison: the package `converted` from its own files against `recorded`, the one
// `record stop` generated for the same recording declared `fixed` with the same names. The same
// files; every one but the task `task` byte for byte (`control.json` and the policy document among
// them); the task the same but for its provenance and its operations'; the conversion's `note` the
// one entry of `provenance.select_fixes`, which the recorded task does not have.
pub(crate) fn assert_as_recorded(
    converted: &BTreeMap<String, Vec<u8>>,
    recorded: &BTreeMap<String, Vec<u8>>,
    task: &str,
    note: &Value,
) {
    assert_eq!(
        converted.keys().collect::<Vec<_>>(),
        recorded.keys().collect::<Vec<_>>()
    );
    for (path, bytes) in recorded {
        if path != task {
            assert!(
                converted[path] == *bytes,
                "{path} differs from the recorded package's"
            );
        }
    }
    let ours = document(converted, task);
    let theirs = document(recorded, task);
    assert_eq!(without_provenance(&ours), without_provenance(&theirs));
    assert_eq!(ours["provenance"]["select_fixes"], json!([note]));
    assert_eq!(theirs["provenance"].get("select_fixes"), None);
}

// A package generated with the variable form, the default item_b, the demonstration pressing
// item_a in slot 0: the `lab` output and the package's entries.
fn variable_package(bench: &Bench) -> (Value, BTreeMap<String, Vec<u8>>) {
    let lab = bench.generated(list(), select(chosen_by_a_person("item_b"), Some("item_a")));
    let entries = package(&lab);
    (lab, entries)
}

// The work order's test 8 (Addendum 1, ruling 8): the package of the variable form converted from
// its own files, no names given: the variable's default fixed. It answers the select step's
// operation, the variable, its default and the names fixed, and each file it changed with the
// members: `control.json` (`/variables`, the last), the policy document (`/facts`, `/gates`,
// `/scoring`) and the task (`/provenance/select_fixes`). The converted package is the one `record
// stop` generates for the same recording declared `fixed: [item_b]`, byte for byte but for the
// task's provenance (`assert_as_recorded`); it declares no variable, is admitted, and the regression
// cases written with the old package pass on it from disk.
#[test]
fn self_t8_a_package_converted_from_its_files_is_what_a_fixed_recording_generates() {
    let bench = Bench::new();
    let (lab, old) = variable_package(&bench);

    let (fix, new) = convert_package(&old, None, &bench.ocr);

    assert_eq!(fix.operation, OPERATION);
    assert_eq!(fix.variable, WANTED);
    assert_eq!(fix.default, ["item_b"]);
    assert_eq!(fix.fixed, ["item_b"]);
    let changed = fix
        .files
        .iter()
        .map(|file| (file.file.as_str(), file.members.clone()))
        .collect::<Vec<_>>();
    let members = |listed: &[&str]| {
        listed
            .iter()
            .map(|member| (*member).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        changed,
        [
            ("control.json", members(&["/variables"])),
            (POLICY, members(&["/facts", "/gates", "/scoring"])),
            (TASK, members(&["/provenance/select_fixes"])),
        ]
    );
    let recorded = Bench::new();
    let fixed_lab = recorded.generated(list(), select(fixed(&["item_b"]), Some("item_a")));
    assert_as_recorded(
        &new,
        &package(&fixed_lab),
        TASK,
        &conversion_note(OPERATION, &["item_b"], &["item_b"]),
    );
    assert_eq!(document(&new, "control.json").get("variables"), None);
    assert_eq!(
        text(&new[POLICY]),
        written(&fixed_policy(
            json!(["item_b"]),
            json!([place("item_b", 1)])
        ))
    );
    let prepared =
        admit(&new, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert!(prepared.declared_variables().is_empty());
    let dir = bench.package_dir("converted", &new);
    cases_pass(&bench, &lab, &dir);
}

// The work order's test 9: the same package converted with names given in an order of preference,
// `[item_a, item_b]` (its default was item_b): the one `record stop` generates for the same recording
// declared `fixed: [item_a, item_b]`, byte for byte but for the task's provenance, its policy
// document the fixed form of that order; admitted, it chooses item_a's slot 0 on the list and slot 0
// where item_b alone shows.
#[test]
fn self_t9_names_given_in_order_convert_to_what_a_recording_fixing_them_generates() {
    let bench = Bench::new();
    let (_, old) = variable_package(&bench);

    let (fix, new) = convert_package(&old, Some(&["item_a", "item_b"][..]), &bench.ocr);

    assert_eq!(fix.default, ["item_b"]);
    assert_eq!(fix.fixed, ["item_a", "item_b"]);
    let recorded = Bench::new();
    let fixed_lab =
        recorded.generated(list(), select(fixed(&["item_a", "item_b"]), Some("item_a")));
    assert_as_recorded(
        &new,
        &package(&fixed_lab),
        TASK,
        &conversion_note(OPERATION, &["item_b"], &["item_a", "item_b"]),
    );
    assert_eq!(
        text(&new[POLICY]),
        written(&fixed_policy(
            json!(["item_a", "item_b"]),
            json!([place("item_a", 1), place("item_b", 2)])
        ))
    );
    let prepared =
        admit(&new, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert_eq!(chosen(&prepared, list()), Some(0));
    assert_eq!(chosen(&prepared, list_of([BRAVO, BRAVO])), Some(0));
}

// The work order's test 10: each refusal of the package route, with its word, on test 8's package
// changed as each case says (`record_select_invalid` and its `select_problem` unless said):
// no such variable (`variable_missing`); the variable read by another policy document, and by
// another step of the task (`variable_read_elsewhere`, the files that read it named); the policy's
// gate and the declaration's title edited by hand (`members_edited`, the file and the member
// named); a default that is no list of names, repeats one or is empty (`default_not_names`); a
// default's name, and a name given, none of the select's names (`default_not_in_names`, the name);
// names given that are no list of 1 to 2 names, each once (`validation_failed`). A refusal answers
// no file to write; that the command then writes none is actinglab's `gate_463_self`.
#[test]
fn self_t10_each_refusal_of_the_conversion_has_its_word() {
    let bench = Bench::new();
    let (_, old) = variable_package(&bench);
    let refused = |entries: &BTreeMap<String, Vec<u8>>, variable: &str, names: Option<&[&str]>| {
        match fix_select(entries, variable, names, &bench.ocr) {
            Ok(fix) => panic!(
                "converted, answering {:?}",
                fix.files.iter().map(|file| &file.file).collect::<Vec<_>>()
            ),
            Err(error) => error,
        }
    };
    // The refusal's code, its `select_problem` and its details.
    let word = |error: &LabError| {
        let details = error.details.clone().unwrap_or(Value::Null);
        (
            error.code.clone(),
            details["select_problem"].clone(),
            details,
        )
    };
    // The package with the document `path` written `value`, as the generator writes a document.
    let with = |path: &str, value: &Value| {
        let mut changed = old.clone();
        changed.insert(path.to_owned(), written(value).into_bytes());
        changed
    };
    let invalid = "record_select_invalid";

    let (code, problem, _) = word(&refused(&old, "other_item", None));
    assert_eq!(
        (code.as_str(), problem),
        (invalid, json!("variable_missing"))
    );

    let extra = "resources/operations/neutral_task/policies/extra.json";
    let reads = json!({"facts": [{"fact_key": VARIABLE_KEY}]});
    let (code, problem, details) = word(&refused(&with(extra, &reads), WANTED, None));
    assert_eq!(
        (code.as_str(), problem),
        (invalid, json!("variable_read_elsewhere"))
    );
    assert_eq!(details["readers"], json!([extra, POLICY]));
    let mut task = document(&old, TASK);
    task["operations"]
        .as_array_mut()
        .expect("the operations")
        .push(json!({"id": "step_09_other", "cap": {"fact_key": VARIABLE_KEY}}));
    let (code, problem, details) = word(&refused(&with(TASK, &task), WANTED, None));
    assert_eq!(
        (code.as_str(), problem),
        (invalid, json!("variable_read_elsewhere"))
    );
    assert_eq!(details["readers"], json!([POLICY, TASK]));

    let mut policy = document(&old, POLICY);
    policy["gates"][0]["predicate"]["threshold"] = json!(2);
    let (code, problem, details) = word(&refused(&with(POLICY, &policy), WANTED, None));
    assert_eq!((code.as_str(), problem), (invalid, json!("members_edited")));
    assert_eq!(
        (&details["file"], &details["member"]),
        (&json!(POLICY), &json!("gates"))
    );
    let mut control = document(&old, "control.json");
    control["variables"][0]["title"] = json!({"en-US": "Edited"});
    let (code, problem, details) = word(&refused(&with("control.json", &control), WANTED, None));
    assert_eq!((code.as_str(), problem), (invalid, json!("members_edited")));
    assert_eq!(
        (&details["file"], &details["member"]),
        (&json!("control.json"), &json!("/variables/0"))
    );

    for default in [json!("item_b"), json!(["item_b", "item_b"]), json!([])] {
        let mut control = document(&old, "control.json");
        control["variables"][0]["default"] = default.clone();
        let (code, problem, _) = word(&refused(&with("control.json", &control), WANTED, None));
        assert_eq!(
            (code.as_str(), problem),
            (invalid, json!("default_not_names")),
            "{default}"
        );
    }

    let mut control = document(&old, "control.json");
    control["variables"][0]["default"] = json!(["item_c"]);
    let (code, problem, details) = word(&refused(&with("control.json", &control), WANTED, None));
    assert_eq!(
        (code.as_str(), problem),
        (invalid, json!("default_not_in_names"))
    );
    assert_eq!(details["name"], "item_c");
    let (code, problem, details) = word(&refused(&old, WANTED, Some(&["item_c"][..])));
    assert_eq!(
        (code.as_str(), problem),
        (invalid, json!("default_not_in_names"))
    );
    assert_eq!(details["name"], "item_c");

    let given: [&[&str]; 3] = [&[], &["item_a", "item_a"], &["item_a", "item_b", "item_c"]];
    for names in given {
        let (code, _, _) = word(&refused(&old, WANTED, Some(names)));
        assert_eq!(code, "validation_failed", "{names:?}");
    }
}

// The work order's test 12: converting twice. Test 8's package converted once, then again, with and
// without names: the second call refuses with `variable_missing` (the variable is gone) and answers
// no file; the package as the first conversion left it is still admitted, its policy document still
// the fixed form of item_b and its provenance still one conversion.
#[test]
fn self_t12_a_package_converted_twice_refuses_the_second_time() {
    let bench = Bench::new();
    let (_, old) = variable_package(&bench);
    let (_, once) = convert_package(&old, None, &bench.ocr);

    for names in [None, Some(&["item_a"][..])] {
        let error = match fix_select(&once, WANTED, names, &bench.ocr) {
            Ok(_) => panic!("{names:?}: converted twice"),
            Err(error) => error,
        };
        assert_eq!(error.code, "record_select_invalid", "{names:?}");
        let details = error.details.as_ref().expect("the refusal's details");
        assert_eq!(details["select_problem"], "variable_missing", "{names:?}");
    }
    let prepared =
        admit(&once, &bench.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert!(prepared.declared_variables().is_empty());
    assert_eq!(
        text(&once[POLICY]),
        written(&fixed_policy(
            json!(["item_b"]),
            json!([place("item_b", 1)])
        ))
    );
    assert_eq!(
        document(&once, TASK)["provenance"]["select_fixes"],
        json!([conversion_note(OPERATION, &["item_b"], &["item_b"])])
    );
}
