// SPDX-License-Identifier: AGPL-3.0-only

// SCH-M slice CF-2, the kernel (frozen model `rd-1024/sched-design/model.md` §3.3 D-9 and its §6
// row CF-2; `contracts/resource-readings.md`): a reading mark may read a completion time, a
// countdown `H:MM:SS` declared as `{"type": "timestamp_ms", "format": "clock_hms",
// "base": "observation"}`. The declaration gate admits it with `clock_hms` only; a mark with an
// agreement holds when the instants its frames convert to (each frame's capture time plus the
// countdown it shows) lie within (frames - 1) × interval_ms + 1000 ms of one another.
//
// The device shows one page at a time: a frame's first pixel is the page's colour. On `terminal`
// its second pixel carries a running countdown to the fixed instant `READY_MS`, in whole seconds
// (three bytes, big-endian); `misread_after_ms` makes the frames captured that long after the
// page's first frame show three minutes more, as an OCR misread of one digit would. The device
// clock moves only by the run's pauses; captures are stamped with it from `FIRST_CAPTURE_MS`. The
// OCR provider reads the countdown in the pixel its region starts on, at confidence 0.99.

use super::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 4;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGES: [(&str, [u8; 3]); 2] = [("home", [255, 0, 0]), ("terminal", [0, 0, 255])];
const READY_AT: &str = "resource.neutral.ready.slot";
// The device clock's start, and the instant the terminal page's countdown runs to: one hour and
// five minutes (and a little) after it.
const FIRST_CAPTURE_MS: u64 = 1_700_000_000_000;
const READY_MS: u64 = FIRST_CAPTURE_MS + 3_900_000 + 400;
// The mark's agreement: three frames 1000 ms apart, so its tolerance is 2 × 1000 + 1000 ms.
const FRAMES: u64 = 3;
const INTERVAL_MS: u64 = 1_000;
// A misread of one digit: the frame shows three minutes more than the time left.
const MISREAD_S: u64 = 180;

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// Reads the countdown in the pixel its region starts on, `H:MM:SS`, at confidence 0.99; a pixel
// that carries none reads as no text.
#[derive(Debug, Default)]
struct ShownCountdown;

impl VisionProvider for ShownCountdown {
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
        _request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "readings request execution evidence",
        ))
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let start = usize::try_from(request.region.x).expect("a region on the frame") * 3;
        let bytes = &request.frame.rgb8_pixels[start..start + 3];
        let seconds = u64::from_be_bytes([0, 0, 0, 0, 0, bytes[0], bytes[1], bytes[2]]);
        let text = if seconds == 0 {
            String::new()
        } else {
            format!(
                "{}:{:02}:{:02}",
                seconds / 3_600,
                (seconds / 60) % 60,
                seconds % 60
            )
        };
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                confidence: Some(0.99),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: "countdown-ocr".to_owned(),
                session_id: "countdown-session".to_owned(),
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
                model_ref: request.model_ref.to_owned(),
                model_sha256: request.model_sha256.to_owned(),
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

// The device, its clock and the run's records.
struct Device {
    page: &'static str,
    elapsed_ms: u64,
    first_terminal_ms: Option<u64>,
    misread_after_ms: Option<u64>,
    traces: Vec<ContainedTaskTrace>,
}

impl Device {
    // Starts on `home`; the tap at column 1 moves to `terminal`. With `misread_after_ms`, the
    // terminal frames captured that long after its first frame show three minutes more.
    fn on_home(misread_after_ms: Option<u64>) -> Self {
        Self {
            page: "home",
            elapsed_ms: 0,
            first_terminal_ms: None,
            misread_after_ms,
            traces: Vec::new(),
        }
    }

    // The readings and the misses of each joined frame that read anything: its capture time in
    // Unix ms, its readings as (key, value), and its misses as (key, problem).
    #[allow(clippy::type_complexity)]
    fn joined(
        &self,
    ) -> Vec<(
        u64,
        Vec<(String, u64)>,
        Vec<(String, ResourceReadingProblem)>,
    )> {
        self.traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::ResourceReadings {
                    captured_at,
                    readings,
                    unresolved,
                } if !readings.is_empty() || !unresolved.is_empty() => Some((
                    u64::try_from(
                        captured_at
                            .duration_since(UNIX_EPOCH)
                            .expect("a capture after the epoch")
                            .as_millis(),
                    )
                    .expect("a capture time in u64"),
                    readings
                        .iter()
                        .map(|reading| (reading.fact_key.clone(), reading.value))
                        .collect(),
                    unresolved
                        .iter()
                        .map(|miss| (miss.declaration.fact_key.clone(), miss.problem))
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let now_ms = FIRST_CAPTURE_MS + self.elapsed_ms;
        let mut pixels = vec![0_u8; WIDTH as usize * 3];
        pixels[..3].copy_from_slice(&colour(self.page));
        if self.page == "terminal" {
            let first = *self.first_terminal_ms.get_or_insert(now_ms);
            let mut seconds = READY_MS.saturating_sub(now_ms) / 1_000;
            if self
                .misread_after_ms
                .is_some_and(|after| now_ms >= first + after)
            {
                seconds += MISREAD_S;
            }
            pixels[3..6].copy_from_slice(&seconds.to_be_bytes()[5..]);
        }
        let mut frame = Frame::from_pixels(
            WIDTH,
            1,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame");
        frame.captured_at = UNIX_EPOCH + Duration::from_millis(now_ms);
        Ok(frame.into())
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        let InputAction::Tap { x: 1, .. } = action else {
            return Err("the fixture takes the tap at column 1 only");
        };
        self.page = "terminal";
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }

    fn sampling_supported(&self) -> bool {
        true
    }

    fn pause(&mut self, duration: Duration) {
        self.elapsed_ms += u64::try_from(duration.as_millis()).expect("a pause in u64");
    }
}

// The pages' colour targets on the first pixel and the countdown on the second.
fn pack_json() -> Value {
    let mut targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    targets.push(json!({
        "type": "ocr", "id": "count/countdown", "region": {"x": 1, "y": 0, "width": 1, "height": 1},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
        "case_sensitive": false, "minimum_confidence": 0.9, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    }));
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    })
}

fn pages_json() -> Value {
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    json!({"schema_version": "0.3", "pages": pages})
}

fn control(max_steps: Option<usize>) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.countdown.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 200,
        "timeout_ms": 60000
    });
    if let Some(max_steps) = max_steps {
        control["max_steps"] = json!(max_steps);
    }
    control
}

// The countdown mark on `terminal`, its value declared as `value`; with `agreement`, the mark's
// agreement.
fn countdown_mark(value: Value, agreement: Option<Value>) -> Value {
    let mut mark = json!({
        "id": "slot", "fact_key": READY_AT, "page_id": "terminal",
        "target_id": "count/countdown", "trim": "whitespace_v1", "value": value,
        "minimum_confidence_milli": 900, "valid_for_ms": 43200000
    });
    if let Some(agreement) = agreement {
        mark["agreement"] = agreement;
    }
    mark
}

// A completion time read as a countdown, with `format`.
fn timestamp_value(format: &str) -> Value {
    json!({"type": "timestamp_ms", "format": format, "base": "observation"})
}

// The page-graph task: one tap `open` from its entry page `home` to `terminal`, with `marks`.
fn open_task(marks: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {"mappings": [{"outcome_key": "done",
                                             "effect": "no_designated_effect",
                                             "terminal_pages": ["terminal"]}]},
        "operations": [{
            "id": "open", "from": "home", "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }],
        "resource_readings": marks
    })
}

fn prepared(program: Value) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(control(None)).expect("task control");
    let program: TaskProgram = serde_json::from_value(program).expect("task program");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack_json().to_string()).expect("recognition pack"),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ShownCountdown),
    )
    .expect("recognition evaluator");
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": PAGES
                .iter()
                .map(|(page, _)| json!({"id": format!("neutral/{page}"),
                                        "required": [format!("page/{page}")]}))
                .collect::<Vec<_>>()
        }))
        .expect("page set"),
    )
    .expect("page detector");
    let scheduling_outcome = program.scheduling_outcome.clone();
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

// Admits the task `task` as a sealed package, through the declaration gate.
fn admit(task: Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control(Some(1))),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack_json()),
        (
            "resources/recognition/neutral.test.pages.json",
            pages_json(),
        ),
    ] {
        zip.start_file(path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, &value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    let bytes = zip.finish().expect("finish zip").into_inner();
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &bytes,
        Arc::new(ShownCountdown),
    )
}

// The pointer and reason of the declaration gate's refusal.
fn refused_at(result: Result<PreparedContainedTask, ContainedTaskError>) -> (String, String) {
    let error = match result {
        Ok(_) => panic!("the declaration is refused"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        SharedCode::ResourceDeclarationInvalid.as_str()
    );
    let issue = error
        .declaration_issue()
        .expect("the refusal names its declaration");
    (issue.field_path.clone(), format!("{:?}", issue.reason))
}

fn succeeded(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) if outcome.outcome == TaskOutcome::Success => outcome,
        other => panic!("expected a successful run, got {other:?}"),
    }
}

// D-9, row item "`timestamp_ms` with another format refused": a countdown mark declared with
// `format` `clock_hms` and `base` `observation` is admitted and marks its key; the same mark with
// any other format is refused by the declaration gate, `InvalidValue` at the mark's
// `value/format`.
#[test]
fn cf2_a_countdown_mark_is_admitted_with_clock_hms_and_refused_with_another_format() {
    let admitted = admit(open_task(vec![countdown_mark(
        timestamp_value("clock_hms"),
        None,
    )]))
    .unwrap_or_else(|error| panic!("a clock_hms countdown mark is admitted: {error:?}"));
    assert_eq!(admitted.reading_keys(), [READY_AT]);

    for format in [
        "ascii_decimal",
        "comma_grouped",
        "current_capacity",
        "times_prefixed",
    ] {
        assert_eq!(
            refused_at(admit(open_task(vec![countdown_mark(
                timestamp_value(format),
                None
            )]))),
            (
                "/resource_readings/0/value/format".to_owned(),
                format!("{:?}", ResourceDeclarationReason::InvalidValue)
            ),
            "{format}"
        );
    }
}

// D-9, row item "agreement passes inside the tolerance and fails outside": the countdown mark
// with an agreement of three frames 1000 ms apart, whose tolerance is (3 - 1) × 1000 + 1000 ms.
// A running countdown converts to one instant on every frame (within the display's second), so
// the mark holds: it gives one reading, the handed-off frame's, whose frame time plus its
// countdown is the instant the countdown runs to, to the second. When the last frame shows three
// minutes more (a misread digit), its instant is 180 000 ms away from the others, outside the
// tolerance: the mark gives no reading and the miss `no_agreement`.
#[test]
fn cf2_an_agreement_compares_the_frames_instants_within_its_tolerance() {
    let agreement = json!({"frames": FRAMES, "interval_ms": INTERVAL_MS});
    let task = || {
        open_task(vec![countdown_mark(
            timestamp_value("clock_hms"),
            Some(agreement.clone()),
        )])
    };

    let mut running = Device::on_home(None);
    succeeded(prepared(task()).run(&mut running));
    let joined = running.joined();
    assert_eq!(joined.len(), 1, "one frame read the mark: {joined:?}");
    let (captured_at_ms, readings, misses) = &joined[0];
    assert_eq!(misses, &Vec::new(), "inside the tolerance: no miss");
    assert_eq!(readings.len(), 1, "one reading: {readings:?}");
    let (key, seconds) = &readings[0];
    assert_eq!(key, READY_AT);
    let instant = captured_at_ms + seconds * 1_000;
    assert!(
        instant <= READY_MS && READY_MS - instant < 1_000,
        "the frame time plus the countdown is the instant it runs to, to the second: {instant}"
    );

    // The last frame, 2000 ms after the handed-off one, shows three minutes more.
    let mut misread = Device::on_home(Some(INTERVAL_MS + INTERVAL_MS / 2));
    succeeded(prepared(task()).run(&mut misread));
    let joined = misread.joined();
    assert_eq!(joined.len(), 1, "one frame read the mark: {joined:?}");
    let (_, readings, misses) = &joined[0];
    assert_eq!(readings, &Vec::new(), "outside the tolerance: no reading");
    assert_eq!(
        misses,
        &vec![(READY_AT.to_owned(), ResourceReadingProblem::NoAgreement)]
    );
}

// Coordinator ruling of 2026-10-24 on CF-2 (answer to the slice's question 2): a completion time
// is no amount, so a `resource_adjustments` entry that names the key of a `timestamp_ms` mark is
// refused at declaration, at that entry's path, with the existing declaration-error code
// (`resource_declaration_invalid`, `InvalidValue`): an entry read after its operation (no
// `amount`) and one that reads the change the screen shows (`amount`) alike.
#[test]
fn cf2_an_adjustment_of_a_completion_time_key_is_refused_at_its_entry() {
    for adjustment in [
        json!({"id": "spent", "fact_key": READY_AT, "operation_id": "open"}),
        json!({"id": "spent", "fact_key": READY_AT, "operation_id": "open",
               "amount": {"target_id": "count/countdown", "direction": "consume"}}),
    ] {
        let mut task = open_task(vec![countdown_mark(timestamp_value("clock_hms"), None)]);
        task["resource_adjustments"] = json!([adjustment.clone()]);
        assert_eq!(
            refused_at(admit(task)),
            (
                "/resource_adjustments/0".to_owned(),
                format!("{:?}", ResourceDeclarationReason::InvalidValue)
            ),
            "{adjustment}"
        );
    }
}
