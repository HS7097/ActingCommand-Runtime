// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #335 S5a evidence helpers shared by the one-off tests.
// Neutral packages, a scripted runtime, a stub OCR provider and the public resource bundle.
#![allow(dead_code)]

use actingcommand_contract::{
    ContentDirectory, ContentDirectoryVersion, InputAction, PackageRef, content_directory_digest,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ContainedTaskError, ContainedTaskRuntime, ContainedTaskTrace, ExternalExpectedSha256,
    InputFrameContext, ObservedFrame, PreparedContainedTask,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrExecutionProviderKind, OcrProviderExecutionEvidence,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The public umbrella bundle HS7097/ActingCommand@536f048a `bundles/`, built from resource
/// commit 3ff697b.
pub const BUNDLE: &[u8] = include_bytes!("../one_off_s5a/bundle-3ff697b.zip");

pub const HOME: [u8; 3] = [255, 0, 0];
pub const DONE: [u8; 3] = [0, 0, 255];
pub const EMPTY: [u8; 3] = [0, 255, 0];
pub const BLACK: [u8; 3] = [0, 0, 0];

pub fn report(tag: &str, line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-S5A {tag} {}", line.as_ref());
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256Hash::digest(bytes).to_string()
}

pub fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

pub fn frame(left: [u8; 3], right: [u8; 3]) -> Frame {
    Frame::from_pixels(
        2,
        1,
        [left, right].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

pub fn copy(frame: &Frame) -> Frame {
    frame.try_clone().expect("copy fixture frame")
}

pub fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("fixture JSON")
}

pub fn zip_entries(entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(path.as_str(), options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

pub fn read_zip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("zip entry");
        if file.is_dir() {
            continue;
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content).expect("read zip entry");
        entries.insert(file.name().to_owned(), content);
    }
    entries
}

/// The sealed packs of the public bundle, by file name.
pub fn bundle_packs() -> BTreeMap<String, Vec<u8>> {
    read_zip(BUNDLE)
        .into_iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix("packs/")
                .filter(|name| name.ends_with(".zip"))
                .map(|name| (name.to_owned(), bytes))
        })
        .collect()
}

/// The pack whose file name ends with `.<task>.zip`.
pub fn bundle_pack(task: &str) -> Vec<u8> {
    let suffix = format!(".{task}.zip");
    bundle_packs()
        .into_iter()
        .find(|(name, _)| name.ends_with(&suffix))
        .map(|(_, bytes)| bytes)
        .expect("bundle pack")
}

/// Files the parser derives; a content directory declares only its sources.
pub fn is_derived(path: &str) -> bool {
    path == "resources/manifest.json"
        || path == "resources/operations/operations.index.json"
        || path == "resources/operations/operations.primitives.json"
        || (path.starts_with("resources/recognition/")
            && (path.ends_with(".pack.json") || path.ends_with(".pages.json")))
        || (path.starts_with("resources/navigation/") && path.ends_with(".navigation.json"))
}

pub fn unsealed(entries: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    entries
        .iter()
        .filter(|(path, _)| !is_derived(path))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}

pub fn task_path(task: &str) -> String {
    format!("resources/operations/{task}/task.json")
}

pub fn content_reference(entries: &BTreeMap<String, Vec<u8>>) -> PackageRef {
    let digest = content_directory_digest(
        entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), *Sha256Hash::digest(bytes).as_bytes())),
    );
    PackageRef::ContentDirectory(ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: digest,
    })
}

/// Writes a hash-named content directory below a fresh temporary root.
pub fn write_content_directory(label: &str, entries: &BTreeMap<String, Vec<u8>>) -> PathBuf {
    let PackageRef::ContentDirectory(reference) = content_reference(entries) else {
        unreachable!("content directory reference")
    };
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir()
        .join(format!(
            "one-off-s5a-{label}-{}-{unique}",
            std::process::id()
        ))
        .join(&reference.sha256);
    for (path, bytes) in entries {
        let target = root.join(path);
        std::fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
        std::fs::write(target, bytes).expect("write entry");
    }
    root
}

pub fn load_zip(
    bytes: &[u8],
    provider: Option<Arc<dyn VisionProvider>>,
) -> Result<PreparedContainedTask, ContainedTaskError> {
    let expected = ExternalExpectedSha256::parse_hex(&sha256_hex(bytes)).expect("hash");
    match provider {
        Some(provider) => PreparedContainedTask::load_with_vision_provider(
            "one-off-s5a",
            bytes,
            expected,
            provider,
        ),
        None => PreparedContainedTask::load("one-off-s5a", bytes, expected),
    }
}

pub fn load_directory(
    label: &str,
    entries: &BTreeMap<String, Vec<u8>>,
    provider: Option<Arc<dyn VisionProvider>>,
) -> Result<PreparedContainedTask, ContainedTaskError> {
    let directory = write_content_directory(label, entries);
    PreparedContainedTask::load_path(
        "one-off-s5a",
        &directory,
        &content_reference(entries),
        provider,
        deadline(),
    )
}

pub fn describe_error(error: &ContainedTaskError) -> String {
    format!(
        "code={} detail={:?} issue={:?}",
        error.code(),
        error.detail(),
        error.declaration_issue()
    )
}

pub fn describe_task(task: &PreparedContainedTask) -> String {
    format!(
        "admitted task={} package={} mode={} ocr={} max_steps={} home={:?} outcome={:?}",
        task.task_label(),
        task.package_label(),
        task.execution_mode(),
        task.has_post_admission_ocr(),
        task.maximum_executed_steps(),
        task.required_home_entry_page(),
        task.scheduling_outcome()
    )
}

pub fn describe_load(result: &Result<PreparedContainedTask, ContainedTaskError>) -> String {
    match result {
        Ok(task) => describe_task(task),
        Err(error) => format!("refused {}", describe_error(error)),
    }
}

pub fn path_of(directory: &Path) -> String {
    directory.display().to_string()
}

#[derive(Debug, Default)]
pub struct StubOcr {
    pub observations: Mutex<VecDeque<(String, Option<f32>)>>,
    pub calls: AtomicU32,
}

impl StubOcr {
    pub fn with(observations: &[(&str, Option<f32>)]) -> Arc<Self> {
        Arc::new(Self {
            observations: Mutex::new(
                observations
                    .iter()
                    .map(|(text, confidence)| ((*text).to_owned(), *confidence))
                    .collect(),
            ),
            calls: AtomicU32::new(0),
        })
    }

    pub fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

fn execution(call: u32) -> OcrProviderExecutionEvidence {
    OcrProviderExecutionEvidence {
        invocation_id: format!("one-off-{call}"),
        session_id: "one-off-session".to_owned(),
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
        model_sha256: "a".repeat(64),
        cpu_ep_registered: true,
        cpu_fallback_disabled: false,
        fallback_forbidden: true,
        fallback_observed: None,
        complete: true,
    }
}

impl VisionProvider for StubOcr {
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
            VisionProviderErrorCode::Internal,
            "fixture exposes OCR only",
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
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let (text, confidence) = self
            .observations
            .lock()
            .expect("fixture lock")
            .pop_front()
            .ok_or_else(|| {
                VisionProviderError::new(
                    VisionProviderErrorCode::Internal,
                    "fixture observation missing",
                )
            })?;
        let blocks = if text.is_empty() {
            Vec::new()
        } else {
            vec![OcrProviderTextBlock {
                text: text.clone(),
                rect: request.region,
                confidence,
            }]
        };
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text,
                blocks,
                confidence,
            },
            execution: Some(execution(call)),
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Internal,
            "fixture exposes OCR only",
        ))
    }
}

/// Captures the scripted frames in order and repeats the last one.
pub struct TraceRuntime {
    pub frames: VecDeque<Frame>,
    pub captures: usize,
    pub inputs: usize,
    pub traces: Vec<ContainedTaskTrace>,
}

impl TraceRuntime {
    pub fn new(frames: Vec<Frame>) -> Self {
        Self {
            frames: frames.into(),
            captures: 0,
            inputs: 0,
            traces: Vec::new(),
        }
    }

    /// The trace variant names in order.
    pub fn names(&self) -> Vec<String> {
        self.traces
            .iter()
            .map(|trace| {
                format!("{trace:?}")
                    .split(|character: char| !character.is_ascii_alphanumeric())
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    pub fn fingerprint(&self) -> String {
        sha256_hex(format!("{:?}", self.traces).as_bytes())
    }
}

impl ContainedTaskRuntime for TraceRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        let frame = if self.frames.len() > 1 {
            self.frames.pop_front().ok_or("fixture exhausted")?
        } else {
            self.frames
                .front()
                .ok_or("fixture exhausted")?
                .try_clone()
                .map_err(|_| "fixture copy failed")?
        };
        Ok(frame.into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }
}

pub fn ocr_target(id: &str) -> Value {
    json!({
        "type": "ocr",
        "id": id,
        "region": {"x": 1, "y": 0, "width": 1, "height": 1},
        "languages": ["en"],
        "timeout_ms": 1000,
        "match_mode": "contains",
        "expected": ["0"],
        "case_sensitive": true,
        "minimum_confidence": 0.0,
        "model_ref": "PP-OCRv6_medium",
        "model_sha256": "a".repeat(64)
    })
}

pub fn color_target(id: &str, expected: [u8; 3]) -> Value {
    json!({
        "type": "color",
        "id": id,
        "region": {"x": 0, "y": 0, "width": 1, "height": 1},
        "expected": expected
    })
}

pub fn page(id: &str, required: &str) -> Value {
    json!({"id": format!("neutral/{id}"), "required": [required], "optional": [], "forbidden": []})
}

/// A package entry set: control, manifest, the entry task, pack and pages.
pub fn package_entries(
    control: &Value,
    task: &Value,
    targets: &[Value],
    pages: &[Value],
) -> BTreeMap<String, Vec<u8>> {
    let pack = json!({
        "schema_version": "0.6",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    BTreeMap::from([
        ("control.json".to_owned(), json_bytes(control)),
        (
            "resources/manifest.json".to_owned(),
            json_bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (task_path("task"), json_bytes(task)),
        (
            "resources/recognition/neutral.test.pack.json".to_owned(),
            json_bytes(&pack),
        ),
        (
            "resources/recognition/neutral.test.pages.json".to_owned(),
            json_bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ])
}

pub fn control(schema: &str, package: &str, max_steps: u32) -> Value {
    json!({
        "schema_version": schema,
        "package_id": package,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 1000,
        "timeout_ms": 5000,
        "max_steps": max_steps
    })
}

/// A reading of `ocr/credits` into `resource.credits` on `page`.
pub fn reading(page: &str) -> Value {
    json!({
        "id": "credits",
        "fact_key": "resource.credits",
        "page_id": page,
        "target_id": "ocr/credits",
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 9007199254740991_u64, "format": "comma_grouped"},
        "minimum_confidence_milli": 900,
        "valid_for_ms": 21600000
    })
}

/// Schema 0.6: guarded click from home to terminal (the offline fixture shape).
pub fn navigable_package() -> Vec<u8> {
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            }
        }]
    });
    let targets = [
        color_target("page/home", HOME),
        color_target("page/terminal", DONE),
        json!({"type": "color", "id": "guard/ready", "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": EMPTY}),
    ];
    let pages = [page("home", "page/home"), page("terminal", "page/terminal")];
    zip_entries(&package_entries(
        &control("Lab-1y.control.v1", "neutral.one-off.navigable", 2),
        &task,
        &targets,
        &pages,
    ))
}

/// Schema 0.8: zero-input fields on home (the observation package shape), optional readings.
pub fn fields_task(readings: Option<Value>) -> Value {
    let mut task = json!({
        "schema_version": "0.8",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "home",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "fields_recorded", "effect": "no_designated_effect", "terminal_pages": ["home"]}
        ]},
        "post_admission_ocr": {
            "mode": "fields_v1",
            "page_ids": ["home"],
            "fields": [{
                "id": "credits",
                "group": "home_snapshot",
                "target_id": "ocr/credits",
                "required": true,
                "privacy": "public",
                "trim": "whitespace_v1",
                "value": {"type": "unsigned_integer", "min": 0, "max": 9007199254740991_u64, "format": "comma_grouped"}
            }],
            "limits": {"max_frames": 1, "max_items": 2, "max_string_bytes": 64, "max_total_bytes": 4096, "max_truth_entries": 1},
            "outcome_key": "fields_recorded"
        },
        "operations": []
    });
    if let Some(readings) = readings {
        task["resource_readings"] = readings;
    }
    task
}

pub fn fields_package(readings: Option<Value>) -> Vec<u8> {
    let targets = [color_target("page/home", HOME), ocr_target("ocr/credits")];
    let pages = [page("home", "page/home")];
    zip_entries(&package_entries(
        &control("Lab-1y.control.v1", "neutral.one-off.fields", 1),
        &fields_task(readings),
        &targets,
        &pages,
    ))
}

/// Schema 0.9: one designated claim from home to done or empty, optional readings.
pub fn claim_task(readings: Option<Value>) -> Value {
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": ["done", "empty"],
        "scheduling_outcome": {
            "designated_operation": "claim",
            "mappings": [
                {"outcome_key": "claimed", "effect": "designated_effect_completed", "terminal_pages": ["done", "empty"]},
                {"outcome_key": "idle", "effect": "no_designated_effect", "terminal_pages": ["done", "empty"]}
            ]
        },
        "operations": [{
            "id": "claim",
            "from": "home",
            "to": ["done", "empty"],
            "click": {"kind": "point", "x": 0, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    if let Some(readings) = readings {
        task["resource_readings"] = readings;
    }
    task
}

pub fn claim_entries(readings: Option<Value>, pages: &[Value]) -> BTreeMap<String, Vec<u8>> {
    let targets = [
        color_target("page/home", HOME),
        color_target("page/done", DONE),
        color_target("page/empty", EMPTY),
        ocr_target("ocr/credits"),
    ];
    package_entries(
        &control("Lab-1y.control.v2", "neutral.one-off.claim", 2),
        &claim_task(readings),
        &targets,
        pages,
    )
}

pub fn claim_pages() -> Vec<Value> {
    vec![
        page("home", "page/home"),
        page("done", "page/done"),
        page("empty", "page/empty"),
    ]
}

pub fn claim_package(readings: Option<Value>) -> Vec<u8> {
    zip_entries(&claim_entries(readings, &claim_pages()))
}

/// Runs a task over frames and reports its outcome, counts and traces.
pub fn run_and_report(tag: &str, task: &PreparedContainedTask, frames: Vec<Frame>) -> TraceRuntime {
    let mut runtime = TraceRuntime::new(frames);
    let outcome = task.run(&mut runtime);
    report(
        tag,
        format!(
            "outcome={:?} captures={} inputs={} traces={:?} fingerprint={}",
            outcome,
            runtime.captures,
            runtime.inputs,
            runtime.names(),
            runtime.fingerprint()
        ),
    );
    runtime
}
