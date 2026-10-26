// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 / #401 DA-2a (model 392 v1.2 §0.2 C1, C10, C11, §5, §12.1 row DA-2a, §13 rows
// DA2-T1, T2, T3, T6, T12, T14 and T15; brief rd-1026/brief-da2a): the display adapter's frame
// gate at the kernel's six capture entries (the entry page, a navigation locate, a sample, a
// page frame, a reading's agreement frame and the Lab's dry-run select). A frame of the
// package's size is the identity and passes as it is; a larger frame of the package's ratio is a
// pure scale, resampled once to the package's size; anything else waits for DA-2b and ends the
// run with `display_map_unresolved` and no input.
//
// The scene is synthetic and neutral, drawn in base pixels (a 1280x720 package) in blocks of
// 2x2: the page's colour in the 4x4 block at the origin, a 24x24 icon at (620, 320), a probe
// square of 4x4 at (100, 50) inside a ring of another colour. A device frame of factor k / 2
// (k = 2: 1280x720, k = 3: 1920x1080) draws each 2x2 base block as a k x k block, so the area
// average of the 1920x1080 frame is the base scene exactly. Every expected device coordinate is
// written out by hand: the base coordinate times 3/2, a box's far edges rounded up.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::unique_tests::encode_png;
use super::*;
use actingcommand_contract::page_projection::Rect as DeviceRect;
use actingcommand_contract::{
    CaptureExtent, CaptureFrameCrop, CaptureFrameTransform, CaptureGeometry,
    CaptureGeometryObservation, CaptureGeometrySource, CaptureRotation, CaptureRotationObservation,
    CaptureRotationSource, CaptureWmSizeKind, DisplayMapKind, InstanceFactContext,
    InstanceFactSnapshot, TaskTimingBudgetOrigin,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    AssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, RecognitionPackResult, VisionProviderError, VisionProviderErrorCode,
    load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

const BASE: (u32, u32) = (1280, 720);
const HOME_PAGE: &str = "neutral/home";
const HOME: [u8; 3] = [255, 0, 0];
const TERMINAL: [u8; 3] = [0, 0, 255];
const PROBE: [u8; 3] = [0, 255, 0];
const RING: [u8; 3] = [255, 255, 0];
const ICON: u32 = 24;
const ICON_X: u32 = 620;
const ICON_Y: u32 = 320;
const PROBE_X: u32 = 100;
const PROBE_Y: u32 = 50;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A neutral pattern of the icon's 12x12 blocks; no pixel is black.
fn icon_block(x: u32, y: u32) -> [u8; 3] {
    [
        ((x * 37 + y * 11) % 250 + 5) as u8,
        ((x * 13 + y * 41 + 7) % 250 + 5) as u8,
        ((x * 29 + y * 17 + 3) % 250 + 5) as u8,
    ]
}

/// The icon template: each of its blocks drawn 2x2.
fn icon_png() -> Vec<u8> {
    encode_png(ICON, ICON, |x, y| icon_block(x / 2, y / 2))
}

#[derive(Debug)]
struct IconAsset(Vec<u8>);

impl AssetResolver for IconAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "icon.png" {
            Ok(self.0.clone())
        } else {
            Err(actingcommand_recognition_pack::RecognitionPackError::fatal(
                "no such fixture asset",
            ))
        }
    }
}

/// An OCR provider that reads the same text everywhere and keeps each request's region and
/// frame size.
#[derive(Debug, Default)]
struct RecordingOcr {
    requests: Mutex<Vec<(PackRect, u32, u32)>>,
}

impl VisionProvider for RecordingOcr {
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
        self.requests.lock().expect("OCR requests").push((
            request.region,
            request.frame.width,
            request.frame.height,
        ));
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: "READY".to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: "READY".to_owned(),
                rect: request.region,
                confidence: Some(0.9),
            }],
            confidence: Some(0.9),
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

/// The base scene's pixel at `(x, y)` with the page colour `page` and the probe's and its ring's
/// colours: constant on every 2x2 block.
fn base_pixel(page: [u8; 3], probe: [u8; 3], ring: [u8; 3], x: u32, y: u32) -> [u8; 3] {
    if x < 4 && y < 4 {
        return page;
    }
    if (ICON_X..ICON_X + ICON).contains(&x) && (ICON_Y..ICON_Y + ICON).contains(&y) {
        return icon_block((x - ICON_X) / 2, (y - ICON_Y) / 2);
    }
    if (PROBE_X..PROBE_X + 4).contains(&x) && (PROBE_Y..PROBE_Y + 4).contains(&y) {
        return probe;
    }
    if (PROBE_X - 4..PROBE_X + 8).contains(&x) && (PROBE_Y - 4..PROBE_Y + 8).contains(&y) {
        return ring;
    }
    [0, 0, 0]
}

/// The device frame of factor `k` / 2 of the scene: `640k` x `360k`, every 2x2 base block drawn
/// as a `k` x `k` block.
fn device_pixels(k: u32, page: [u8; 3], probe: [u8; 3], ring: [u8; 3]) -> (u32, u32, Vec<[u8; 3]>) {
    let (width, height) = (640 * k, 360 * k);
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| base_pixel(page, probe, ring, 2 * (x / k), 2 * (y / k)))
        })
        .collect();
    (width, height, pixels)
}

fn scripted(k: u32, page: [u8; 3]) -> ScriptedFrame {
    let (width, height, pixels) = device_pixels(k, page, PROBE, RING);
    ScriptedFrame::rgb(width, height, &pixels)
}

fn frame_with(k: u32, page: [u8; 3], probe: [u8; 3], ring: [u8; 3]) -> Frame {
    let (width, height, pixels) = device_pixels(k, page, probe, ring);
    Frame::from_pixels(
        width,
        height,
        pixels.concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("a device frame")
}

fn frame_of(k: u32, page: [u8; 3]) -> Frame {
    frame_with(k, page, PROBE, RING)
}

/// A black frame of `width` x `height`.
fn black(width: u32, height: u32) -> ScriptedFrame {
    ScriptedFrame::rgb(width, height, &vec![[0, 0, 0]; (width * height) as usize])
}

/// The recognition pack of a `base` package: the two pages' colours, the probe, the icon and the
/// candidate layout `choices` (one slot around the probe, its `ready` feature read on two
/// frames); with `consensus`, page `home` needs two frames 1 ms apart; with `ocr`, a plain text
/// area of 100x40 at (300, 200).
fn pack(
    base: (u32, u32),
    consensus: bool,
    ocr: bool,
) -> actingcommand_recognition_pack::RecognitionPack {
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": base.0, "height": base.1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 4, "height": 4}, "expected": HOME},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 4, "height": 4}, "expected": TERMINAL},
            {"type": "color", "id": "probe/ready",
             "region": {"x": PROBE_X, "y": PROBE_Y, "width": 4, "height": 4}, "expected": PROBE},
            {"type": "template", "id": "icon", "template_path": "icon.png",
             "region": {"x": 600, "y": 300, "width": 64, "height": 64}, "threshold": 0.9}
        ],
        "candidate_layouts": [{
            "id": "choices",
            "page_id": HOME_PAGE,
            "kind": "fixed_slots",
            "sample_interval_ms": 1,
            "features": [{
                "name": "ready",
                "value": "passed",
                "consensus": {"samples": [{"frame": 0}, {"frame": 1}],
                              "aggregate": {"kind": "k_of_n", "k": 2}}
            }],
            "slots": [{
                "rect": {"x": 96, "y": 46, "width": 12, "height": 12},
                "click": {"x": PROBE_X, "y": PROBE_Y, "width": 4, "height": 4},
                "targets": {"ready": "probe/ready"}
            }]
        }]
    });
    if consensus {
        pack["target_consensus"] = json!({
            "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                          "sample_interval_ms": 1}
        });
    }
    if ocr {
        pack["targets"]
            .as_array_mut()
            .expect("the pack's targets")
            .push(json!({
                "type": "ocr", "id": "text/plain",
                "region": {"x": 300, "y": 200, "width": 100, "height": 40},
                "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
                "expected": ["READY"], "case_sensitive": false, "minimum_confidence": 0.8,
                "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
            }));
    }
    load_pack_from_json_str(&pack.to_string()).expect("recognition pack")
}

fn guard() -> Value {
    json!({
        "page_id": "home",
        "target_id": "probe/ready",
        "expected_rect": {"x": PROBE_X, "y": PROBE_Y, "width": 4, "height": 4},
        "color_probe": "probe/ready"
    })
}

/// The select policy of the layout `choices`: the one ready slot.
fn policy() -> actingcommand_selection_policy::SelectionPolicy {
    let policy: actingcommand_selection_policy::SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "policy.ready",
        "applies_to": {
            "candidate_layout_id": "choices",
            "outcome_keys": {"selected": "ready_selected", "empty": "ready_empty",
                             "insufficient": "ready_insufficient",
                             "ambiguous": "ready_ambiguous", "unknown": "ready_unknown"}
        },
        "fields": [{"name": "ready", "value_type": {"type": "boolean"}}],
        "facts": [],
        "gates": [{"gate_id": "ready",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "ready"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("selection policy");
    policy.validate().expect("valid selection policy");
    policy
}

/// A navigable route of a `base` package from `home` to `terminal`: `open`, a tap on the probe
/// guarded by it, and `pick`, a select step over `choices`.
fn gate_task(
    base: (u32, u32),
    consensus: bool,
    ocr: Option<Arc<RecordingOcr>>,
) -> PreparedContainedTask {
    let space = json!({"width": base.0, "height": base.1});
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.gate.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10_000,
        "timeout_ms": 60_000
    }))
    .expect("task control");
    let mut program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "target_page": "terminal",
        "operations": [
            {"id": "open", "from": "home", "to": "terminal",
             "click": {"kind": "point", "x": PROBE_X + 2, "y": PROBE_Y + 2},
             "guard": guard()},
            {"id": "pick", "from": "home", "to": "terminal", "guard": guard(),
             "select": {"layout_id": "choices",
                        "policy": {"path": "policies/select.json", "sha256": "b".repeat(64)}}}
        ]
    }))
    .expect("task program");
    program.operations[0]
        .validate(&control, TaskOperationDefaults::default(), "0.9")
        .expect("valid operation");
    program.operations[1].prepared_select = Some(Box::new(selection::PreparedSelect::for_test(
        "choices",
        policy(),
    )));
    let pack = pack(base, consensus, ocr.is_some());
    let assets = Arc::new(IconAsset(icon_png()));
    let evaluator = match ocr {
        Some(provider) => RecognitionEvaluator::with_vision_provider(pack, assets, provider),
        None => RecognitionEvaluator::with_asset_resolver(pack, assets),
    }
    .expect("recognition evaluator");
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.3",
            "pages": [
                {"id": HOME_PAGE, "required": ["page/home", "icon"]},
                {"id": "neutral/terminal", "required": ["page/terminal"]}
            ]
        }))
        .expect("page set"),
    )
    .expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

/// A valid declaration of the game: by height, on the screen.
fn declaration() -> StandardPackDisplayLayout {
    serde_json::from_value(json!({
        "schema_version": "actingcommand.display-layout.v1",
        "game": "neutral",
        "scale": {"by": "height", "area": "screen"},
        "anchor_area": "screen",
        "correction": {"radius_px": 16}
    }))
    .expect("display declaration")
}

/// A reading mark of the home page whose agreement reads two frames 1 ms apart.
fn agreement_mark() -> actingcommand_contract::ResourceReadingDeclaration {
    serde_json::from_value(json!({
        "id": "count", "fact_key": "resource.neutral_count", "page_id": "home",
        "target_id": "page/home", "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 9},
        "minimum_confidence_milli": 900, "valid_for_ms": 60_000,
        "agreement": {"frames": 2, "interval_ms": 1}
    }))
    .expect("reading mark")
}

/// A timing context of the runtime's clock with a minute left.
fn timing_of(runtime: &mut ScriptedRuntime) -> ContainedTaskTimingContext {
    let started = runtime.monotonic_now();
    ContainedTaskTimingContext::new(
        started,
        started + Duration::from_secs(60),
        TaskTimingBudgetOrigin::Task,
    )
}

/// What one capture entry gave: the pages it matched (a dry run's refusal detail), the sizes of
/// the captures it recorded, and each recognition's size with its targets' regions.
#[derive(Debug, PartialEq)]
struct Seen {
    matched: Vec<String>,
    captures: Vec<(u32, u32)>,
    recognitions: Vec<((u32, u32), BTreeMap<String, Option<DeviceRect>>)>,
}

fn seen(matched: Vec<String>, runtime: &ScriptedRuntime) -> Seen {
    let mut captures = Vec::new();
    let mut recognitions = Vec::new();
    for trace in runtime.traces() {
        match trace {
            ContainedTaskTrace::CaptureCompleted { width, height } => {
                captures.push((*width, *height));
            }
            ContainedTaskTrace::RecognitionCompleted {
                width,
                height,
                targets,
                ..
            } => recognitions.push((
                (*width, *height),
                targets
                    .iter()
                    .map(|target| (target.target_id.clone(), target.region))
                    .collect(),
            )),
            _ => {}
        }
    }
    Seen {
        matched,
        captures,
        recognitions,
    }
}

/// The kernel's capture entries; `Samples` takes a sample and the page frame.
#[derive(Debug, Clone, Copy)]
enum Entry {
    EntryPage,
    Locate,
    Samples,
    Agreement,
    DryRun,
}

const ENTRIES: [Entry; 5] = [
    Entry::EntryPage,
    Entry::Locate,
    Entry::Samples,
    Entry::Agreement,
    Entry::DryRun,
];

/// `entry` on device frames of factor `k` / 2 (the dry run on the terminal page, the others on
/// the home page).
fn at_entry(entry: Entry, k: u32) -> Seen {
    let task = gate_task(BASE, matches!(entry, Entry::Samples), None);
    let mut runtime = ScriptedRuntime::new(vec![vec![scripted(k, HOME)]]);
    let timing = timing_of(&mut runtime);
    match entry {
        Entry::EntryPage => {
            let matched = match task.recognize_entry_frame(&mut runtime, HOME_PAGE, timing) {
                Ok(EntryFrame::Matched(page)) => vec![page],
                Ok(EntryFrame::Unmatched(_)) => Vec::new(),
                Err(error) => panic!("the entry page at {k}/2: {error:?}"),
            };
            seen(matched, &runtime)
        }
        Entry::Locate => {
            let started = runtime.monotonic_now();
            let scene = task
                .capture_scene(
                    &mut runtime,
                    started,
                    Duration::from_secs(60),
                    TaskTimingBudgetOrigin::Task,
                )
                .unwrap_or_else(|error| panic!("a locate's frame at {k}/2: {error:?}"));
            let matched = task
                .matching_pages_on_scene(&scene, &[HOME_PAGE])
                .expect("the locate's pages");
            seen(matched, &runtime)
        }
        Entry::Samples => {
            let matched =
                match task.capture_frame_scene(&mut runtime, None, None, timing, None, None) {
                    Ok(Ok(observation)) => vec![observation.page_label],
                    Ok(Err(_)) => Vec::new(),
                    Err(error) => panic!("a sampled page frame at {k}/2: {error:?}"),
                };
            seen(matched, &runtime)
        }
        Entry::Agreement => {
            let frames = task
                .capture_agreement_frames(&mut runtime, &[agreement_mark()], PaceClock::UNPACED)
                .unwrap_or_else(|error| panic!("an agreement frame at {k}/2: {error:?}"));
            let matched = frames
                .iter()
                .flat_map(|(_, scene)| {
                    task.matching_pages_on_scene(scene, &[HOME_PAGE])
                        .expect("the agreement frame's pages")
                })
                .collect();
            seen(matched, &runtime)
        }
        Entry::DryRun => {
            let snapshot = InstanceFactSnapshot {
                snapshot_id: "snapshot-a".to_owned(),
                ledger_position: 1,
                context: InstanceFactContext {
                    instance_id: "instance-a".to_owned(),
                    server_id: "test".to_owned(),
                    game_id: "neutral".to_owned(),
                },
                records: Vec::new(),
            };
            let error = dry_run_select(
                &task,
                "pick",
                &frame_of(k, TERMINAL),
                (&snapshot, 1_000),
                &task.default_run_parameters(),
                None,
            )
            .expect_err("a select from home on the terminal page");
            assert_eq!(
                error.code().as_str(),
                ExecutionKernelCode::SelectPageMismatch.as_str(),
                "the dry run at {k}/2"
            );
            seen(
                vec![error.detail().unwrap_or_default().to_owned()],
                &runtime,
            )
        }
    }
}

/// The regions a home recognition reports: the page's colour and the icon's match.
fn home_regions(page: DeviceRect, icon: DeviceRect) -> BTreeMap<String, Option<DeviceRect>> {
    BTreeMap::from([
        ("page/home".to_owned(), Some(page)),
        ("icon".to_owned(), Some(icon)),
    ])
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> DeviceRect {
    DeviceRect {
        x,
        y,
        width,
        height,
    }
}

/// What each entry gives at factor `k` / 2: the same pages, the device frame's size, and the base
/// regions as device boxes (`page` the page colour's, `icon` the icon match's).
fn expected(entry: Entry, k: u32, page: DeviceRect, icon: DeviceRect) -> Seen {
    let size = (640 * k, 360 * k);
    let home = || vec![HOME_PAGE.to_owned()];
    match entry {
        Entry::EntryPage => Seen {
            matched: home(),
            captures: vec![size],
            recognitions: vec![(size, home_regions(page, icon))],
        },
        Entry::Locate | Entry::Agreement => Seen {
            matched: home(),
            captures: vec![size],
            recognitions: Vec::new(),
        },
        Entry::Samples => Seen {
            matched: home(),
            captures: vec![size, size],
            recognitions: vec![(size, home_regions(page, icon))],
        },
        Entry::DryRun => Seen {
            matched: vec!["operation=pick from=home matched_pages=neutral/terminal".to_owned()],
            captures: Vec::new(),
            recognitions: Vec::new(),
        },
    }
}

fn refusal(result: Result<ScaleFrame<'_>, ContainedTaskError>) -> ContainedTaskError {
    match result {
        Err(error) => error,
        Ok(scale) => panic!("expected a refusal, got the map {:?}", scale.map()),
    }
}

fn values(error: &ContainedTaskError) -> Value {
    Value::Object(error.outcome().values().clone())
}

/// `display_map_unresolved` of `problem` at `display_map_build` for a `frame` frame and a `base`
/// package.
fn unresolved(problem: &str, frame: (u32, u32), base: (u32, u32)) -> Value {
    json!({
        "display_problem": problem,
        "stage": "display_map_build",
        "frame_width": frame.0,
        "frame_height": frame.1,
        "base_width": base.0,
        "base_height": base.1
    })
}

fn run_failure(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "expected a task failure, the run ended {:?}",
            outcome.outcome
        ),
    }
}

/// The identity test (model 392 v1.2 §12.2, C16; Alice 10-26, merge is deploy) and DA2-T1: a
/// 1280x720 package on a 1280x720 frame passes the gate as the identity, with or without a
/// declaration; its scale frame is the frame's own buffer and its scene that of the frame. At
/// each of the six capture entries the frame is recognised as before: the same pages, captures
/// and recognitions of the frame's size, and the declared regions unchanged.
#[test]
fn da2a_identity_a_1280x720_package_on_a_1280x720_frame_passes_unchanged_at_all_six_entries() {
    let task = gate_task(BASE, false, None);
    let frame = frame_of(2, HOME);
    for layout in [None, Some(declaration())] {
        let scale = task
            .control
            .resolution
            .validate_frame(&frame, layout.as_ref())
            .expect("the identity");
        assert!(scale.map().is_plain_identity(), "{layout:?}");
        assert_eq!(
            scale.pixels().pixels.as_ptr(),
            frame.pixels.as_ptr(),
            "the scale frame is the frame's own buffer"
        );
        assert_eq!(
            scale.scene().expect("scale scene").rgb8_pixels(),
            scene_from_frame(&frame).expect("frame scene").rgb8_pixels()
        );
    }
    for entry in ENTRIES {
        assert_eq!(
            at_entry(entry, 2),
            expected(entry, 2, rect(0, 0, 4, 4), rect(ICON_X, ICON_Y, ICON, ICON)),
            "{entry:?}"
        );
    }
}

/// DA2-T2 and the brief's pure-scale test: the 1280x720 scene drawn on a 1920x1080 frame is a
/// pure scale (s = 3/2) whose scale frame is the 1280x720 scene exactly; recognised on it, the
/// page passes and the template matches at the same base position, and the recognition reports
/// the frame's 1920x1080 and each region as the device box that covers it: the page colour's
/// (0, 0, 4, 4) as (0, 0, 6, 6), the icon's (620, 320, 24, 24) as (930, 480, 36, 36).
#[test]
fn da2a_t2_a_1920x1080_frame_of_a_1280x720_package_reports_the_same_regions_scaled() {
    let task = gate_task(BASE, false, None);
    let scaled = frame_of(3, HOME);
    let scale = task
        .control
        .resolution
        .validate_frame(&scaled, None)
        .expect("a pure scale");
    assert_eq!(scale.map().kind(), DisplayMapKind::Scale);
    assert_eq!(
        (scale.map().scale().num(), scale.map().scale().den()),
        (3, 2)
    );
    assert_eq!(
        scale.scene().expect("scale scene").rgb8_pixels(),
        scene_from_frame(&frame_of(2, HOME))
            .expect("base scene")
            .rgb8_pixels(),
        "the scale frame is the base scene"
    );
    let mut runtime = ScriptedRuntime::new(vec![vec![scripted(3, HOME)]]);
    let timing = timing_of(&mut runtime);
    let observation = match task.capture_frame_scene(&mut runtime, None, None, timing, None, None) {
        Ok(Ok(observation)) => observation,
        Ok(Err(_)) => panic!("the home page is recognised on the scale frame"),
        Err(error) => panic!("the page frame: {error:?}"),
    };
    assert_eq!(observation.page_label, HOME_PAGE);
    assert_eq!(observation.scene.width(), 1280);
    assert_eq!(observation.scene.height(), 720);
    assert_eq!(
        seen(vec![observation.page_label.clone()], &runtime),
        expected(
            Entry::EntryPage,
            3,
            rect(0, 0, 6, 6),
            rect(930, 480, 36, 36)
        )
    );
}

/// DA2-T12: at each of the six capture entries a 1920x1080 frame of a 1280x720 package passes the
/// gate as a pure scale (none reports `contained_task_frame_resolution_mismatch`, which is
/// retired) and gives what the 1280x720 frame gives, its recorded sizes the frame's and its
/// regions scaled by 3/2.
#[test]
fn da2a_t12_every_capture_entry_passes_a_1920x1080_frame_of_a_1280x720_package() {
    for entry in ENTRIES {
        assert_eq!(
            at_entry(entry, 3),
            expected(entry, 3, rect(0, 0, 6, 6), rect(930, 480, 36, 36)),
            "{entry:?}"
        );
    }
}

/// DA2-T3 as this slice requires it (brief rd-1026/brief-da2a: a translation waits for DA-2b):
/// a 2340x1080 frame of a 1280x720 package is no pure scale. Without a declaration it is
/// `layout_undeclared`; under one (by height, on the screen) it is a translation, refused
/// `translation_unsupported`; a run on it ends with that refusal and no input.
#[test]
fn da2a_t3_a_translation_waits_for_da_2b_and_sends_nothing() {
    let task = gate_task(BASE, false, None);
    let wide = Frame::from_pixels(
        2340,
        1080,
        vec![0; 2340 * 1080 * 3],
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("a 2340x1080 frame");
    let undeclared = refusal(task.control.resolution.validate_frame(&wide, None));
    assert_eq!(undeclared.code().as_str(), "display_map_unresolved");
    assert_eq!(
        values(&undeclared),
        unresolved("layout_undeclared", (2340, 1080), BASE)
    );
    let declared = refusal(
        task.control
            .resolution
            .validate_frame(&wide, Some(&declaration())),
    );
    assert_eq!(
        values(&declared),
        unresolved("translation_unsupported", (2340, 1080), BASE)
    );

    let mut runtime =
        ScriptedRuntime::new(vec![vec![black(2340, 1080)]]).with_display_layout(declaration());
    let error = run_failure(task.run(&mut runtime));
    assert_eq!(error.code().as_str(), "display_map_unresolved");
    assert_eq!(
        values(&error),
        unresolved("translation_unsupported", (2340, 1080), BASE)
    );
    assert!(runtime.inputs().is_empty(), "no input");
}

/// DA2-T6: positional regions are mapped and never widened. On a 1920x1080 frame of a 1280x720
/// package, the guard's colour probe reads its base box (100, 50, 4, 4) of the scale frame, the
/// stability crop cuts the same 4x4 base pixels as on the 1280x720 frame, the still-frames
/// region's change measure sees a change inside the box and none in the ring around it, and a
/// plain text area is read as its base box of the 1280x720 scale frame.
#[test]
fn da2a_t6_positional_regions_are_mapped_and_never_widened() {
    let provider = Arc::new(RecordingOcr::default());
    let task = gate_task(BASE, false, Some(Arc::clone(&provider)));

    let mut runtime = ScriptedRuntime::new(vec![vec![scripted(3, HOME)]]);
    let timing = timing_of(&mut runtime);
    let observation = task
        .capture_frame(&mut runtime, None, None, timing, None, None)
        .expect("the page frame")
        .expect("the home page");
    let (outcome, target) = task.program.operations[0]
        .guard_outcome(
            &task.control,
            &observation,
            &task.evaluator,
            &mut runtime,
            None,
        )
        .expect("the guard's evaluation");
    assert_eq!(
        outcome,
        ContainedTaskGuardOutcome::Passed {
            page_label: HOME_PAGE.to_owned(),
            target_id: "probe/ready".to_owned(),
            target_kind: "color".to_owned(),
        }
    );
    assert_eq!(
        target
            .and_then(|target| target.color)
            .and_then(|color| color.region),
        Some(PackRect {
            x: 100,
            y: 50,
            width: 4,
            height: 4
        })
    );

    let declaration: StabilityTerminationDeclaration = serde_json::from_value(json!({
        "region": {"x": PROBE_X, "y": PROBE_Y, "width": 4, "height": 4},
        "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
        "consecutive_unchanged_threshold": 2,
        "max_steps": 4
    }))
    .expect("stability declaration");
    let scaled = frame_of(3, HOME);
    let scale = task
        .control
        .resolution
        .validate_frame(&scaled, None)
        .expect("a pure scale");
    let sample = stability_sample(&scale, &declaration).expect("the scaled sample");
    assert_eq!(
        sample,
        stability_sample(&frame_of(2, HOME), &declaration).expect("the base sample")
    );
    assert_eq!((sample.width, sample.height), (4, 4));
    assert_eq!(sample.pixels, PROBE.repeat(16));

    let region = actingcommand_recognition::Rect {
        x: 100,
        y: 50,
        width: 4,
        height: 4,
    };
    let digest = |probe: [u8; 3], ring: [u8; 3]| {
        let frame = frame_with(3, HOME, probe, ring);
        let scale = task
            .control
            .resolution
            .validate_frame(&frame, None)
            .expect("a pure scale");
        frame_change_digest(&scale.scene().expect("scale scene"), Some(region))
            .expect("the region's digest")
    };
    let still = digest(PROBE, RING);
    assert_eq!(
        frame_change_milli(&still, &digest(PROBE, TERMINAL)).expect("a change"),
        0,
        "the ring is outside the region"
    );
    assert!(
        frame_change_milli(&still, &digest([255, 255, 255], RING)).expect("a change") > 0,
        "the region itself changed"
    );

    task.evaluator
        .evaluate_target(&scale.scene().expect("scale scene"), "text/plain")
        .expect("the text area's evaluation");
    assert_eq!(
        *provider.requests.lock().expect("OCR requests"),
        vec![(
            PackRect {
                x: 300,
                y: 200,
                width: 100,
                height: 40
            },
            1280,
            720
        )]
    );
}

/// DA2-T14 (model 392 v1.2 §5.1, C11): the frames of one observation keep one map. A sample of a
/// 1280x720 package taken at 1280x720 and its page frame at 1920x1080 have two maps: the
/// observation ends with `recognition_sample_geometry_changed`; two 1920x1080 frames keep one and
/// pass. A candidate sample under another map than the select step's first frame ends the
/// projection with the kernel's own candidate code, `candidate_sample_page_or_geometry_changed`,
/// though their scale frames are both 1280x720; under the same map the projection goes on.
#[test]
fn da2a_t14_frames_of_one_observation_keep_one_map() {
    let task = gate_task(BASE, true, None);
    let mut runtime = ScriptedRuntime::new(vec![vec![scripted(2, HOME), scripted(3, HOME)]]);
    let timing = timing_of(&mut runtime);
    match task.capture_frame_scene(&mut runtime, None, None, timing, None, None) {
        Err(ContainedTaskRunError::Task(error)) => assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::RecognitionSampleGeometryChanged.as_str()
        ),
        Err(other) => panic!("expected the geometry change, got {other:?}"),
        Ok(_) => panic!("expected the geometry change, the observation passed"),
    }
    let mut runtime = ScriptedRuntime::new(vec![vec![scripted(3, HOME), scripted(3, HOME)]]);
    let timing = timing_of(&mut runtime);
    assert!(matches!(
        task.capture_frame_scene(&mut runtime, None, None, timing, None, None),
        Ok(Ok(_))
    ));

    let task = gate_task(BASE, false, None);
    let layout = task
        .evaluator
        .candidate_layout("choices")
        .expect("the candidate layout");
    let operation = &task.program.operations[1];
    let first = |k: u32, runtime: &mut ScriptedRuntime| PageObservation {
        page_label: HOME_PAGE.to_owned(),
        scene: scene_from_frame(&frame_of(2, HOME)).expect("base scene"),
        stability_sample: None,
        input_context: None,
        captured_at: SystemTime::UNIX_EPOCH,
        frame_started: runtime.monotonic_now(),
        sample_scenes: Vec::new(),
        sampling: None,
        display_map: *task
            .control
            .resolution
            .validate_frame(&frame_of(k, HOME), None)
            .expect("the first frame's map")
            .map(),
    };
    let mut runtime = ScriptedRuntime::new(vec![vec![scripted(3, HOME)]]);
    let timing = timing_of(&mut runtime);
    let deadline = timing.deadline();
    let observed = first(2, &mut runtime);
    match task.project_transaction(&mut runtime, operation, &observed, layout, timing, deadline) {
        Err(ContainedTaskRunError::Task(error)) => assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::CandidateSamplePageOrGeometryChanged.as_str()
        ),
        Err(other) => panic!("expected the candidate geometry change, got {other:?}"),
        Ok(_) => panic!("expected the candidate geometry change, the projection passed"),
    }
    let mut runtime = ScriptedRuntime::new(vec![vec![scripted(3, HOME)]]);
    let timing = timing_of(&mut runtime);
    let deadline = timing.deadline();
    let observed = first(3, &mut runtime);
    if let Err(error) =
        task.project_transaction(&mut runtime, operation, &observed, layout, timing, deadline)
    {
        panic!("the same map keeps the projection going, got {error:?}");
    }
}

/// The geometry a stream records with a 1080x2336 frame cropped at y = 2 from a 1080x2340
/// display (a neutral fake serial).
fn cropped_geometry() -> CaptureGeometryObservation {
    let display = CaptureExtent::new(1080, 2340).expect("display extent");
    CaptureGeometryObservation::Observed(CaptureGeometry {
        backend: CaptureBackendName::Scrcpy,
        source: CaptureGeometrySource::AdbDefaultDisplay {
            serial: "TESTSERIAL0001".to_owned(),
            wm_extent: display,
            wm_size_kind: CaptureWmSizeKind::Physical,
        },
        logical_display_extent: display,
        rotation: CaptureRotationObservation::Observed {
            rotation: CaptureRotation::R0,
            source: CaptureRotationSource::DumpsysDisplayOrientation,
        },
        sampled_at: SystemTime::UNIX_EPOCH,
        frame_transform: Some(CaptureFrameTransform::Crop(CaptureFrameCrop {
            x: 0,
            y: 2,
            width: 1080,
            height: 2336,
        })),
        safe_insets: None,
    })
}

/// DA2-T15 as this slice requires it (brief rd-1026/brief-da2a: anything but the identity and a
/// pure scale waits for DA-2b): a 1080x2336 frame cropped at y = 2 from the 1080x2340 display of
/// a 1080x2340 package is the identity on its display but needs its regions moved by the crop,
/// so it is refused `translation_unsupported`, and a run on it sends nothing; the same frame
/// without its crop record is no pure scale of the package (`layout_undeclared`).
#[test]
fn da2a_t15_a_cropped_frame_waits_for_da_2b_and_sends_nothing() {
    let base = (1080, 2340);
    let task = gate_task(base, false, None);
    let mut cropped = Frame::from_pixels(
        1080,
        2336,
        vec![0; 1080 * 2336 * 3],
        PixelFormat::Rgb8,
        CaptureBackendName::Scrcpy,
    )
    .expect("a 1080x2336 frame");
    let unrecorded = refusal(task.control.resolution.validate_frame(&cropped, None));
    assert_eq!(
        values(&unrecorded),
        unresolved("layout_undeclared", (1080, 2336), base)
    );
    cropped.geometry = cropped_geometry();
    let recorded = refusal(task.control.resolution.validate_frame(&cropped, None));
    assert_eq!(recorded.code().as_str(), "display_map_unresolved");
    assert_eq!(
        values(&recorded),
        unresolved("translation_unsupported", (1080, 2336), base)
    );

    let mut runtime = ScriptedRuntime::new(vec![vec![
        black(1080, 2336).with_geometry(cropped_geometry()),
    ]]);
    let error = run_failure(task.run(&mut runtime));
    assert_eq!(
        values(&error),
        unresolved("translation_unsupported", (1080, 2336), base)
    );
    assert!(runtime.inputs().is_empty(), "no input");
}
