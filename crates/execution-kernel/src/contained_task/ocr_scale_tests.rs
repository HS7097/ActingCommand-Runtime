// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 OCR-SCALE-1 (contracts/ocr-fields.md, section Task schema `0.9` fields tasks;
// register §1.251): a task of schema 0.9 may declare OCR fields, in the schema 0.8 form and
// behind the schema 0.8 gates, and a field's box follows its anchor. Each package goes through
// `PreparedContainedTask::load`. It is a synthetic `neutral` package: one page told by a red first
// pixel, a noise icon, and a three-digit reading beside the icon. A scripted provider reads the
// digits whose whole cells its region holds, so a box that cuts a cell reads fewer digits.

use super::unique_tests::encode_png;
use super::*;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition::resample::{resample_pixels, scale_signed, scaled_length};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrExecutionProviderKind, OcrProviderObservation,
    OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock, VisionProviderError,
    VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::sync::Mutex;
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 200;
const HEIGHT: u32 = 120;
const ICON_WIDTH: u32 = 24;
const ICON_HEIGHT: u32 = 18;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONTROL_V1: &str = "Lab-1y.control.v1";
const CONTROL_V2: &str = "Lab-1y.control.v2";
const DECLARATION_INVALID: &str = "resource_declaration_invalid";
const PROGRAM_INVALID: &str = "contained_task_program_invalid";
const PHASES_INVALID: &str = "contained_task_phases_invalid";
const TARGET_INVALID: &str = "contained_task_post_admission_ocr_target_invalid";
const SCALES: [u32; 2] = [1000, 900];

/// The icon's gray values, row-major: neutral noise with no 0.
fn icon_values() -> Vec<u8> {
    let mut state = 0x0428_0251_u64;
    (0..ICON_WIDTH * ICON_HEIGHT)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 33) % 255 + 1) as u8
        })
        .collect()
}

fn icon_png() -> Vec<u8> {
    let values = icon_values();
    encode_png(ICON_WIDTH, ICON_HEIGHT, |x, y| {
        [values[(y * ICON_WIDTH + x) as usize]; 3]
    })
}

/// Where the icon is shown: its top-left corner and the factor of the interface's zoom.
#[derive(Clone, Copy)]
struct Shown {
    left: i32,
    top: i32,
    factor: u32,
}

/// The page, with the icon resampled by its factor as every search resamples it, when shown.
fn frame(shown: Option<Shown>) -> Frame {
    let mut pixels = vec![0_u8; (WIDTH * HEIGHT * 3) as usize];
    pixels[0..3].copy_from_slice(&[255, 0, 0]);
    if let Some(shown) = shown {
        let width = scaled_length(ICON_WIDTH, shown.factor);
        let height = scaled_length(ICON_HEIGHT, shown.factor);
        let icon = resample_pixels(
            &icon_values(),
            (ICON_WIDTH, ICON_HEIGHT),
            1,
            (width, height),
        )
        .expect("resampled icon");
        for y in 0..height {
            for x in 0..width {
                let row = shown.top as u32 + y;
                let column = shown.left as u32 + x;
                let start = ((row * WIDTH + column) * 3) as usize;
                let value = icon[(y * width + x) as usize];
                pixels[start..start + 3].copy_from_slice(&[value; 3]);
            }
        }
    }
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

/// The reading's three digit cells beside the shown icon. At factor 1000 they lie 30, 46 and 62
/// pixels right of the icon's left edge and 2 below its top, each 14 by 16, and the whole layer
/// scales together.
fn printed_cells(shown: Shown) -> Vec<(PackRect, char)> {
    let scale = |value: i32| scale_signed(value, shown.factor).expect("scaled length");
    [(30, '8'), (46, '5'), (62, '0')]
        .into_iter()
        .map(|(x, digit)| {
            (
                PackRect {
                    x: shown.left + scale(x),
                    y: shown.top + scale(2),
                    width: scale(14),
                    height: scale(16),
                },
                digit,
            )
        })
        .collect()
}

/// Reads the digits whose whole cells the request's region holds, and logs every region.
#[derive(Debug)]
struct PrintedCells {
    cells: Vec<(PackRect, char)>,
    regions: Mutex<Vec<PackRect>>,
}

impl PrintedCells {
    fn new(shown: Option<Shown>) -> Arc<Self> {
        Arc::new(Self {
            cells: shown.map(printed_cells).unwrap_or_default(),
            regions: Mutex::new(Vec::new()),
        })
    }

    fn regions(&self) -> Vec<PackRect> {
        self.regions.lock().expect("region log").clone()
    }
}

impl VisionProvider for PrintedCells {
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
        self.read_text_with_execution_evidence(request)
            .map(|observation| observation.result)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let region = request.region;
        let call = {
            let mut regions = self.regions.lock().expect("region log");
            regions.push(region);
            regions.len()
        };
        let text = self
            .cells
            .iter()
            .filter(|(cell, _)| {
                cell.x >= region.x
                    && cell.y >= region.y
                    && cell.x + cell.width <= region.x + region.width
                    && cell.y + cell.height <= region.y + region.height
            })
            .map(|(_, digit)| *digit)
            .collect::<String>();
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: region,
                    confidence: Some(0.9),
                }],
                confidence: Some(0.9),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("call-{call}"),
                session_id: "session-1".to_owned(),
                session_generation: 1,
                requested_provider: OcrExecutionProviderKind::Cpu,
                resolved_provider: OcrExecutionProviderKind::Cpu,
                requested_cuda_ordinal: None,
                requested_cuda_identity: None,
                resolved_cuda_ordinal: None,
                resolved_cuda_identity: None,
                provider_implementation: "fixture-ocr".to_owned(),
                provider_binary_sha256: "b".repeat(64),
                runtime_version: "fixture-runtime".to_owned(),
                model_ref: "PP-OCRv6_medium".to_owned(),
                model_sha256: MODEL_SHA256.to_owned(),
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_forbidden: true,
                fallback_observed: None,
                complete: true,
            }),
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

/// A zero-input fields task of `schema`: the page `panel` is its target, field and terminal page.
fn task(schema: &str) -> Value {
    json!({
        "schema_version": schema, "task_id": "task", "game": "neutral", "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT}, "target_page": "panel",
        "post_admission_ocr": {
            "mode": "fields_v1", "page_ids": ["panel"],
            "fields": [{"id": "reading", "group": "snapshot", "target_id": "text/reading",
                        "required": true, "privacy": "public", "trim": "whitespace_v1",
                        "value": {"type": "unsigned_integer", "min": 0, "max": 9999}}],
            "limits": {"max_frames": 1, "max_items": 8, "max_string_bytes": 64,
                       "max_total_bytes": 4096, "max_truth_entries": 8},
            "outcome_key": "fields_recorded"
        },
        "scheduling_outcome": {"mappings": [{"outcome_key": "fields_recorded",
            "effect": "no_designated_effect", "terminal_pages": ["panel"]}]},
        "operations": []
    })
}

/// The package of `task` under a control of `control_schema`. The field's box hangs 28 right of
/// the icon's left edge, 56 by 20, and the icon declares `scales` when `factors` has any.
fn package(task: &Value, control_schema: &str, factors: &[u32]) -> Vec<u8> {
    let control = json!({
        "schema_version": control_schema,
        "package_id": "neutral.fields.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 30_000,
        "timeout_ms": 60_000
    });
    let mut icon = json!({"type": "template", "id": "icon/anchor",
                          "template_path": "assets/icon.png", "region": "full_frame",
                          "threshold": 0.9});
    if !factors.is_empty() {
        icon["scales"] = json!({"factors_milli": factors});
    }
    let pack = json!({
        "schema_version": "0.7", "game": "neutral", "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/panel",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            icon,
            {"type": "ocr", "id": "text/reading",
             "region": {"mode": "template_relative", "anchor_target_id": "icon/anchor",
                        "offset": {"x": 28, "y": 0}, "width": 56, "height": 20},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["850"],
             "case_sensitive": false, "minimum_confidence": 0.5,
             "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256}
        ]
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        {"id": "neutral/panel", "required": ["page/panel"], "optional": [], "forbidden": []}
    ]});
    // A dictionary whose two items are one item once normalized, for its refusal row.
    let ambiguous = json!({"schema_version": "actingcommand.ocr-truth-set.v1",
                           "items": ["item", "ITEM"]});
    let bytes = |value: &Value| serde_json::to_vec(value).expect("JSON bytes");
    let entries = [
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        (
            "resources/operations/task/ambiguous.json",
            bytes(&ambiguous),
        ),
        ("resources/assets/icon.png", icon_png()),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in &entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// The control every package of `schema` takes (a task schema 0.9 takes `Lab-1y.control.v2`).
fn control_of(schema: &str) -> &'static str {
    if schema == "0.9" {
        CONTROL_V2
    } else {
        CONTROL_V1
    }
}

/// The code `task` is refused with under `control_schema`.
fn refused(case: &str, task: &Value, control_schema: &str) -> String {
    match PreparedContainedTask::load("neutral.instance", &package(task, control_schema, &[])) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error.code().as_str().to_owned(),
    }
}

/// A device that always shows one frame and records every trace and input.
struct ShowRuntime {
    frame: Frame,
    traces: Vec<ContainedTaskTrace>,
    inputs: Vec<InputAction>,
}

impl ContainedTaskRuntime for ShowRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        Ok(self
            .frame
            .try_clone()
            .expect("copy the fixture frame")
            .into())
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

impl ShowRuntime {
    /// The one field of the run's fields report.
    fn field(&self) -> &OcrFieldResult {
        self.traces
            .iter()
            .find_map(|trace| match trace {
                ContainedTaskTrace::PostAdmissionOcrFields { report } => {
                    Some(&report.records[0].fields[0])
                }
                _ => None,
            })
            .expect("the fields report")
    }
}

/// A run's result, with the device and the provider afterwards.
type Ran = (
    Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
    ShowRuntime,
    Arc<PrintedCells>,
);

/// The package of `schema` with `factors` admitted and run on a frame that shows the icon at
/// `shown`.
fn run(schema: &str, factors: &[u32], shown: Option<Shown>) -> Ran {
    let provider = PrintedCells::new(shown);
    let vision: Arc<dyn VisionProvider> = provider.clone();
    let prepared = match PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package(&task(schema), control_of(schema), factors),
        vision,
    ) {
        Ok(prepared) => prepared,
        Err(error) => panic!("schema {schema}: refused: {error}"),
    };
    assert!(prepared.has_ocr_fields(), "schema {schema}: fields");
    let mut runtime = ShowRuntime {
        frame: frame(shown),
        traces: Vec::new(),
        inputs: Vec::new(),
    };
    let result = prepared.run(&mut runtime);
    (result, runtime, provider)
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

// The rule of the box: the offset and the size scale by the factor the anchor matched at, each
// `(|v| * f + 500) div 1000`, from the matched top-left corner. The evidence keeps what was
// declared. The icon sits elsewhere at 900, so a box that stayed put would read nothing.
#[test]
fn a_box_following_an_anchor_reads_the_same_cells_at_factors_1000_and_900() {
    for (shown, region, anchor) in [
        (
            Shown {
                left: 20,
                top: 30,
                factor: 1000,
            },
            rect(48, 30, 56, 20),
            (24, 18),
        ),
        (
            Shown {
                left: 90,
                top: 70,
                factor: 900,
            },
            rect(115, 70, 50, 18),
            (22, 16),
        ),
    ] {
        let (result, runtime, provider) = run("0.9", &SCALES, Some(shown));

        let factor = shown.factor;
        let outcome = result.expect("the fields task succeeds");
        assert_eq!(outcome.outcome, TaskOutcome::Success, "factor {factor}");
        assert_eq!(provider.regions(), [region], "factor {factor}");
        let field = runtime.field();
        assert_eq!(
            field.value,
            Some(OcrFieldValue::UnsignedInteger(850)),
            "factor {factor}: every digit, none cut"
        );
        let evidence = field.region.as_ref().expect("region evidence");
        assert_eq!(
            evidence.roi,
            Some(actingcommand_contract::OcrRegionRect {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            }),
            "factor {factor}"
        );
        let matched = evidence.anchor_match.as_ref().expect("the anchor's match");
        assert_eq!(
            (
                matched.rect.x,
                matched.rect.y,
                matched.rect.width,
                matched.rect.height
            ),
            (shown.left, shown.top, anchor.0, anchor.1),
            "factor {factor}: the resampled box"
        );
        assert_eq!(
            (evidence.width, evidence.height),
            (56, 20),
            "factor {factor}: the declared size stays in the evidence"
        );
        assert!(runtime.inputs.is_empty(), "factor {factor}: zero inputs");
    }
}

// An absent anchor is the existing named failure: no provider call, no value, and no fixed box to
// fall back to.
#[test]
fn an_absent_anchor_ends_in_the_named_failure_with_no_read() {
    let (result, runtime, provider) = run("0.9", &SCALES, None);

    let Err(ContainedTaskRunError::Task(error)) = result else {
        panic!("a required field without its anchor fails the task");
    };
    assert_eq!(
        error.code().as_str(),
        "contained_task_ocr_fields_unresolved"
    );
    assert!(provider.regions().is_empty(), "no provider call");
    let field = runtime.field();
    assert_eq!(field.reason, OcrFieldReason::RegionUnresolved);
    assert_eq!(field.value, None);
    assert_eq!(
        field.region.as_ref().expect("region evidence").unresolved,
        Some(actingcommand_contract::OcrRegionUnresolvedReason::AnchorNotMatched)
    );
}

// An anchor without `scales` is as it was, in both schemas: the box follows its position at its
// declared size.
#[test]
fn a_box_on_an_anchor_without_scales_keeps_its_declared_size_in_both_schemas() {
    for schema in ["0.8", "0.9"] {
        let shown = Shown {
            left: 90,
            top: 70,
            factor: 1000,
        };
        let (result, runtime, provider) = run(schema, &[], Some(shown));

        let outcome = result.expect("the fields task succeeds");
        assert_eq!(outcome.outcome, TaskOutcome::Success, "schema {schema}");
        assert_eq!(
            provider.regions(),
            [rect(118, 70, 56, 20)],
            "schema {schema}"
        );
        assert_eq!(
            runtime.field().value,
            Some(OcrFieldValue::UnsignedInteger(850)),
            "schema {schema}"
        );
    }
}

// The gates of schema 0.8 hold in schema 0.9: each changed declaration is refused with the code
// schema 0.8 refuses it with.
#[test]
fn schema_0_9_refuses_what_schema_0_8_refuses_with_the_same_code() {
    let dictionary = |path: &str| json!({"type": "dictionary_entry", "dictionary": {"path": path}});
    let missing_dictionary = dictionary("missing.json");
    let ambiguous_dictionary = dictionary("ambiguous.json");
    let change = |task: &mut Value, row: usize| {
        let declaration = &mut task["post_admission_ocr"];
        match row {
            0 => declaration["fields"][0]["target_id"] = json!("text/missing"),
            1 => declaration["fields"][0]["target_id"] = json!("icon/anchor"),
            2 => declaration["page_ids"] = json!(["other"]),
            3 => declaration["fields"][0]["value"] = missing_dictionary.clone(),
            4 => declaration["fields"][0]["value"] = ambiguous_dictionary.clone(),
            _ => {
                task.as_object_mut()
                    .expect("task")
                    .remove("scheduling_outcome");
            }
        }
    };
    let expected = [
        TARGET_INVALID,
        TARGET_INVALID,
        "ocr_fields_zero_input_pages_invalid",
        DECLARATION_INVALID,
        "ocr_fields_dictionary_ambiguous",
        "ocr_fields_outcome_invalid",
    ];
    for (row, code) in expected.into_iter().enumerate() {
        for schema in ["0.8", "0.9"] {
            let mut changed = task(schema);
            change(&mut changed, row);
            assert_eq!(
                refused(
                    &format!("row {row}, schema {schema}"),
                    &changed,
                    control_of(schema)
                ),
                code,
                "row {row}, schema {schema}"
            );
        }
    }
}

// Which schema takes which form: schema 0.8 still requires fields; schema 0.9 may go without them
// only with an operation, takes no comparison, and keeps its control.
#[test]
fn each_schema_takes_its_declaration_form_and_its_control() {
    let without_ocr = |schema: &str| {
        let mut task = task(schema);
        task.as_object_mut()
            .expect("task")
            .remove("post_admission_ocr");
        task
    };
    let mut comparison = task("0.9");
    comparison["post_admission_ocr"] = json!({
        "page_id": "panel", "target_id": "text/reading",
        "truth_set": {"path": "ambiguous.json"},
        "normalization": "trim_lowercase_v1", "comparison": "exact_set_v1",
        "limits": {"max_frames": 1, "max_items": 8, "max_string_bytes": 64,
                   "max_total_bytes": 4096, "max_truth_entries": 8},
        "outcome_key": "fields_recorded"
    });
    for (case, task, control, code) in [
        (
            "0.8 without fields",
            without_ocr("0.8"),
            CONTROL_V1,
            DECLARATION_INVALID,
        ),
        (
            "0.9 without fields or an operation",
            without_ocr("0.9"),
            CONTROL_V2,
            PROGRAM_INVALID,
        ),
        (
            "0.9 with a comparison",
            comparison,
            CONTROL_V2,
            DECLARATION_INVALID,
        ),
        (
            "0.9 under control v1",
            task("0.9"),
            CONTROL_V1,
            PHASES_INVALID,
        ),
        (
            "0.8 under control v2",
            task("0.8"),
            CONTROL_V2,
            PHASES_INVALID,
        ),
    ] {
        assert_eq!(refused(case, &task, control), code, "{case}");
    }
}
