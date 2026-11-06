// SPDX-License-Identifier: AGPL-3.0-only

//! One measurement run (design §1.2, §1.5 items 3-5): the safety preflight, the host facts, the
//! workload, the model identity, ONNX Runtime initialisation, the raw `ort` sessions and the
//! production engine (`PpocrCtcLoader::load_ocr` and `OcrEngine::read_text_view`), cold and warm,
//! then the raw recognizer widths, the consistency section and the loaded modules.
//!
//! Order of the cold measurements: ONNX Runtime initialisation, the first session of the process
//! (the raw detector, which creates the CUDA context or the DirectML device), the second session
//! (the raw recognizer), the raw detector's first run per input shape, the engine's load and its
//! first read of every request (the first full-frame read builds the engine's detector session).
//!
//! Workflow #407 GPU-0b: `--mode template` shares the preflight, the watcher, the host facts, the
//! workload and ONNX Runtime initialisation, and measures in `template_run.rs` instead.

use crate::cli::{BenchMode, RunOptions};
use crate::consistency::{self, BaselineReport};
use crate::error::{BenchCode, BenchError};
use crate::report::{
    BenchIdentity, BlockOutput, CPU_EXECUTION_PROVIDER, ColdSection, CounterStatus, CudaSection,
    EngineSection, HostSection, MemoryMark, ModelPlacement, ModelRow, ModuleRecord, REPORT_SCHEMA,
    RawSection, Report, ReportStatus, RequestOutput, RequestRecord, RuntimeSection, SafetySection,
    ShapeSummary, ShapeTiming, Variant, WorkloadSection,
};
use crate::safety::{SafetyLimits, SafetyMonitor};
use crate::stats::percentiles;
use crate::template_run;
use crate::watch::{FailureSink, Watcher};
use crate::win::{self, OsMemoryReader};
use crate::workload::{
    RequestKind, Workload, WorkloadBlocks, WorkloadFrame, WorkloadRect, WorkloadRequest,
    parse_workload,
};
use actingcommand_ppocr_onnx_json_provider::PpocrCtcLoader;
use actingcommand_ppocr_onnx_json_provider::bench::{
    self as engine_bench, BenchInput, BenchOrtFailure, BenchSessionPlan,
};
use actingcommand_vision_ffi::{
    CudaDeviceSelector, OcrBlockMode, OcrEngine, OcrInferenceRequestView, OcrInferenceResult,
    OcrLineMerge, OcrModelLoad, OnnxExecutionProvider, PpocrCtcParams, VisionFfiError, VisionFrame,
    VisionModelLoader, VisionPixelFormat, VisionRect, VisionRuntimeSpec, enumerate_cuda_devices,
    list_vision_models, next_ocr_session_id, ppocr_model_set_sha256, sha256_hex,
};
use ort::session::Session;
use ort::value::Tensor;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The production install root; no path the bench reads or writes may lie under it.
pub const PRODUCTION_INSTALL_ROOT: &str = r"F:\AC";
/// How long ONNX Runtime initialisation and a model load may wait for another initialisation.
const LOAD_WAIT: Duration = Duration::from_secs(600);

pub(crate) type Monitor = SafetyMonitor<OsMemoryReader>;

/// The report before anything ran: the variant, the bench identity and the safety limits.
pub fn empty_report(options: &RunOptions) -> Report {
    Report {
        schema: REPORT_SCHEMA,
        status: ReportStatus::Ok,
        error: None,
        phase_at_failure: None,
        variant: Variant {
            mode: options.mode,
            label: options.label.clone(),
            execution_provider: options.execution_provider,
            device: options.device,
            tuning: options.tuning,
            warmup: options.warmup,
            reps: options.reps,
        },
        bench: BenchIdentity {
            crate_version: env!("CARGO_PKG_VERSION"),
            executable: None,
            executable_sha256: None,
            process_id: win::current_process_id(),
        },
        safety: SafetySection {
            limits: SafetyLimits::default(),
            priority_class_expected: win::BELOW_NORMAL_CLASS,
            priority_class_read_back: None,
            preflight: None,
        },
        host: HostSection::default(),
        workload: None,
        models: Vec::new(),
        runtime: None,
        memory: Vec::new(),
        cold: ColdSection::default(),
        raw: RawSection::default(),
        engine: EngineSection::default(),
        shapes: ShapeSummary::default(),
        phases: Vec::new(),
        gpu_counters: CounterStatus::default(),
        consistency: None,
        placement: Vec::new(),
        template: None,
        modules: Vec::new(),
        wall_clock_us: 0,
    }
}

/// Refuses any path under the production install root and an `--out` that already exists
/// (the bench writes one new file and nothing else).
pub fn check_footprint(options: &RunOptions) -> Result<(), BenchError> {
    let mut paths = vec![
        ("--vision-root", &options.vision_root),
        ("--models-dir", &options.models_dir),
        ("--workload", &options.workload),
        ("--out", &options.out),
    ];
    if let Some(baseline) = &options.baseline {
        paths.push(("--baseline", baseline));
    }
    for (flag, path) in paths {
        refuse_production_path(flag, path)?;
    }
    if options.out.exists() {
        return Err(BenchError::new(
            BenchCode::FootprintRefused,
            format!(
                "--out {} exists; the bench only writes a new file",
                options.out.display()
            ),
        ));
    }
    if let Some(parent) = options.out.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(BenchError::new(
            BenchCode::OutputUnwritable,
            format!(
                "the folder of --out {} does not exist",
                options.out.display()
            ),
        ));
    }
    Ok(())
}

pub(crate) fn refuse_production_path(what: &str, path: &Path) -> Result<(), BenchError> {
    if is_under(path, Path::new(PRODUCTION_INSTALL_ROOT)) {
        return Err(BenchError::new(
            BenchCode::FootprintRefused,
            format!(
                "{what} {} lies under the production install root {PRODUCTION_INSTALL_ROOT}",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Whether `path` (as given, absolute, or resolved through links) lies under `root`; letters
/// compare without case, as Windows paths do.
fn is_under(path: &Path, root: &Path) -> bool {
    let mut candidates = vec![std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())];
    if let Ok(canonical) = path.canonicalize() {
        candidates.push(strip_verbatim(canonical));
    } else if let Some(parent) = path.parent()
        && let Ok(canonical) = parent.canonicalize()
    {
        candidates.push(strip_verbatim(canonical));
    }
    candidates.iter().any(|candidate| {
        let mut parts = candidate.components();
        root.components().all(|expected| {
            parts.next().is_some_and(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
            })
        })
    })
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    let stripped = path
        .to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
        .filter(|rest| !rest.starts_with(r"UNC\"))
        .map(PathBuf::from);
    stripped.unwrap_or(path)
}

/// Writes the report to `--out` as a new file.
pub fn write_report(out: &Path, report: &Report) -> Result<(), BenchError> {
    let bytes = serde_json::to_vec_pretty(report).map_err(|error| {
        BenchError::new(
            BenchCode::OutputUnwritable,
            format!("the report does not serialise: {error}"),
        )
    })?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| {
            BenchError::new(
                BenchCode::OutputUnwritable,
                format!(
                    "the report could not be written to {}: {error}",
                    out.display()
                ),
            )
        })
}

/// The whole run. The report keeps everything measured before a failure.
pub fn run(options: &RunOptions, report: &mut Report) -> Result<(), BenchError> {
    report.phase_at_failure = Some("preflight".to_owned());
    report.safety.priority_class_read_back = Some(win::set_below_normal_priority()?);
    let limits = SafetyLimits::default();
    let mut monitor = SafetyMonitor::new(limits, OsMemoryReader);
    report.safety.preflight = Some(monitor.preflight()?);
    let watcher = Watcher::start(
        limits,
        FailureSink {
            out: options.out.clone(),
            variant: report.variant.clone(),
        },
        "preflight",
    )?;
    let measured = match options.mode {
        BenchMode::Ocr => measure(options, report, &watcher, &mut monitor),
        BenchMode::Template => template_run::measure(options, report, &watcher, &mut monitor),
    };
    let finished = watcher.finish();
    match finished {
        Ok((phases, counters)) => {
            report.phases = phases;
            report.gpu_counters = counters;
        }
        Err(error) if measured.is_ok() => return Err(error),
        Err(error) => eprintln!("ac-vision-ep-bench: {error}"),
    }
    measured?;
    report.phase_at_failure = None;
    Ok(())
}

pub(crate) fn phase(report: &mut Report, watcher: &Watcher, name: &str) {
    report.phase_at_failure = Some(name.to_owned());
    watcher.phase(name);
}

pub(crate) fn mark(
    report: &mut Report,
    monitor: &mut Monitor,
    name: &str,
) -> Result<(), BenchError> {
    let reading = monitor.check()?;
    report.memory.push(MemoryMark {
        mark: name.to_owned(),
        reading,
    });
    Ok(())
}

struct Model {
    spec: actingcommand_vision_ffi::OcrModelSpec,
    detector_path: PathBuf,
    detector: Vec<u8>,
    recognizer: Vec<u8>,
    params: PpocrCtcParams,
}

fn measure(
    options: &RunOptions,
    report: &mut Report,
    watcher: &Watcher,
    monitor: &mut Monitor,
) -> Result<(), BenchError> {
    phase(report, watcher, "host");
    record_executable(report);
    let cuda_selector = host_facts(options, report)?;

    phase(report, watcher, "workload");
    let workload = read_workload(&options.workload, report)?;
    let frames = load_frames(&workload)?;
    let baseline = options.baseline.as_deref().map(read_baseline).transpose()?;

    phase(report, watcher, "models");
    let model = read_model(options, &workload, report)?;

    phase(report, watcher, "ort_init");
    let (runtime, plan) = initialise(options, cuda_selector, report, monitor)?;

    phase(report, watcher, "raw_sessions");
    let detector_model = (
        "det",
        model.detector.as_slice(),
        model.detector_path.as_path(),
    );
    let recognizer_model = (
        "rec",
        model.recognizer.as_slice(),
        model.spec.recognizer_path.as_path(),
    );
    let started = Instant::now();
    let mut raw_detector = engine_bench::build_session(detector_model.1, detector_model.2, &plan)
        .map_err(|failure| {
        failed_session(
            report,
            &plan,
            detector_model,
            "the raw detector session",
            &failure,
        )
    })?;
    report.cold.first_session_us = Some(micros(started.elapsed()));
    mark(report, monitor, "after_first_session_raw_detector")?;
    // Workflow #407 GPU-0d: ONNX Runtime loaded the CUDA provider while it built the first
    // session; the provider and the NVIDIA libraries come from the runtime's own folder.
    if runtime.execution_provider == OnnxExecutionProvider::Cuda {
        report.modules = win::loaded_modules()?;
        check_runtime_modules(&runtime, &report.modules, "after the first session")?;
    }
    let started = Instant::now();
    let mut raw_recognizer =
        engine_bench::build_session(recognizer_model.1, recognizer_model.2, &plan).map_err(
            |failure| {
                failed_session(
                    report,
                    &plan,
                    recognizer_model,
                    "the raw recognizer session",
                    &failure,
                )
            },
        )?;
    report.cold.second_session_us = Some(micros(started.elapsed()));
    mark(report, monitor, "after_second_session_raw_recognizer")?;

    // Workflow #407 GPU-0c: where ONNX Runtime placed each model's nodes, after the timed cold
    // sessions so the placement sessions do not warm them.
    let mut placement_unread = Vec::new();
    if options.execution_provider != OnnxExecutionProvider::Cpu {
        phase(report, watcher, "placement");
        for (name, bytes, path) in [detector_model, recognizer_model] {
            let placement =
                ModelPlacement::from_read(name, engine_bench::node_placement(bytes, path, &plan));
            if let Some(log_error) = &placement.log_error {
                placement_unread.push(format!("{name}: {log_error}"));
            } else if let Some(failure) = &placement.session_failure {
                placement_unread.push(format!(
                    "{name}: the placement session failed although the raw session built: {}",
                    failure.message.as_deref().unwrap_or(&failure.step)
                ));
            }
            eprintln!(
                "ac-vision-ep-bench: {} {name} placement: {} on non-CPU providers, {} on {CPU_EXECUTION_PROVIDER}{}",
                options.label,
                placement.non_cpu_nodes,
                placement.cpu_nodes,
                if placement.cpu_nodes > 0 {
                    format!(" ({})", placement.cpu_op_summary())
                } else {
                    String::new()
                }
            );
            report.placement.push(placement);
        }
        mark(report, monitor, "after_placement")?;
    }

    phase(report, watcher, "raw_detector");
    let mut detector_inputs = BTreeMap::<String, (String, BenchInput, usize)>::new();
    for (frame, image) in workload.frames.iter().zip(&frames) {
        for request in &frame.requests {
            if request.kind(frame) != RequestKind::FullFrame {
                continue;
            }
            let input = engine_bench::detection_input(
                &raw_detector,
                &model.params,
                &image.view(),
                vision_rect(request.region),
            )
            .map_err(|error| inference_error("the raw detector input", &error))?;
            detector_inputs
                .entry(shape_text(&input.shape))
                .or_insert_with(|| (format!("{}/{}", frame.id, request.id), input, 0))
                .2 += 1;
        }
    }
    for (source, input, regions) in detector_inputs.values() {
        let timing = time_raw(
            &mut raw_detector,
            input,
            source.clone(),
            *regions,
            options,
            monitor,
        )?;
        report.raw.detector.push(timing);
    }
    mark(report, monitor, "after_raw_detector")?;

    phase(report, watcher, "engine_load");
    let loader = PpocrCtcLoader::new(runtime.clone(), None);
    let session_id =
        next_ocr_session_id().map_err(|error| runtime_error("the OCR session identity", &error))?;
    let started = Instant::now();
    let loaded = loader
        .load_ocr(
            &model.spec,
            &workload.model.model_sha256,
            &session_id,
            Instant::now() + LOAD_WAIT,
        )
        .map_err(|error| session_error("PpocrCtcLoader::load_ocr", &error))?;
    let mut engine = match loaded {
        OcrModelLoad::Loaded { engine, .. } => engine,
        OcrModelLoad::Mismatch { model_sha256 } => {
            return Err(BenchError::new(
                BenchCode::ModelIdentityMismatch,
                format!(
                    "the engine hashed the model set to {model_sha256}, the workload names {}",
                    workload.model.model_sha256
                ),
            ));
        }
    };
    report.cold.engine_load_ocr_us = Some(micros(started.elapsed()));
    mark(report, monitor, "after_engine_load")?;

    phase(report, watcher, "engine_cold");
    let gap_factor = model.params.detector.merge_gap_factor;
    let mut seen_shapes = BTreeSet::new();
    let mut records = Vec::new();
    for (frame, image) in workload.frames.iter().zip(&frames) {
        for request in &frame.requests {
            monitor.check()?;
            let kind = request.kind(frame);
            let started = Instant::now();
            let result = read(&mut *engine, image, frame, request, gap_factor)?;
            let first_run_us = micros(started.elapsed());
            if kind == RequestKind::FullFrame && report.cold.engine_first_full_frame_us.is_none() {
                report.cold.engine_first_full_frame_us = Some(first_run_us);
            }
            let output = output_of(&result);
            let shapes = request_shapes(
                &raw_detector,
                &raw_recognizer,
                &model.params,
                image,
                request,
                kind,
                &output,
            )?;
            let new_shapes = shapes
                .into_iter()
                .filter(|shape| seen_shapes.insert(shape.clone()))
                .collect();
            records.push(RequestRecord {
                frame_id: frame.id.clone(),
                request_id: request.id.clone(),
                kind,
                region: request.region,
                blocks: request.blocks,
                first_run_us,
                new_shapes,
                matches_truth: request
                    .expected_text
                    .as_ref()
                    .map(|expected| *expected == output.text),
                expected_text: request.expected_text.clone(),
                output,
                warm: None,
                warm_cpu: None,
                unstable_repetitions: 0,
            });
        }
    }
    report.engine.requests = records.clone();
    mark(report, monitor, "after_engine_cold")?;

    phase(report, watcher, "engine_warm");
    let mut pooled = BTreeMap::<RequestKind, (Vec<u64>, Vec<u64>)>::new();
    let mut unstable_total = 0u64;
    for (record, (frame, image, request)) in records.iter_mut().zip(work_items(&workload, &frames))
    {
        for _ in 0..options.warmup {
            monitor.check()?;
            read(&mut *engine, image, frame, request, gap_factor)?;
        }
        let mut wall = Vec::with_capacity(options.reps as usize);
        let mut cpu = Vec::with_capacity(options.reps as usize);
        for _ in 0..options.reps {
            monitor.check()?;
            let cpu_before = win::process_cpu_time_us()?;
            let started = Instant::now();
            let result = read(&mut *engine, image, frame, request, gap_factor)?;
            let elapsed = micros(started.elapsed());
            let cpu_after = win::process_cpu_time_us()?;
            wall.push(elapsed);
            cpu.push(cpu_after.saturating_sub(cpu_before));
            if !record.output.same_reading(&output_of(&result)) {
                record.unstable_repetitions += 1;
            }
        }
        unstable_total += u64::from(record.unstable_repetitions);
        record.warm = percentiles(&wall);
        record.warm_cpu = percentiles(&cpu);
        let entry = pooled.entry(record.kind).or_default();
        entry.0.extend(wall);
        entry.1.extend(cpu);
    }
    let pool = |kind: RequestKind| pooled.get(&kind);
    report.engine.full_frame = pool(RequestKind::FullFrame).and_then(|(wall, _)| percentiles(wall));
    report.engine.full_frame_cpu =
        pool(RequestKind::FullFrame).and_then(|(_, cpu)| percentiles(cpu));
    report.engine.roi = pool(RequestKind::Roi).and_then(|(wall, _)| percentiles(wall));
    report.engine.roi_cpu = pool(RequestKind::Roi).and_then(|(_, cpu)| percentiles(cpu));
    if let (Some(full), Some(roi)) = (report.engine.full_frame, report.engine.roi) {
        report.engine.mixed_p95_us = Some(full.p95.midpoint(roi.p95));
        report.engine.mixed_weights =
            BTreeMap::from([("full_frame".to_owned(), 0.5), ("roi".to_owned(), 0.5)]);
    }
    report.engine.unstable_repetitions = unstable_total;
    report.engine.requests = records.clone();
    mark(report, monitor, "after_engine_warm")?;

    phase(report, watcher, "raw_recognizer");
    let mut recognizer_inputs = BTreeMap::<String, (String, BenchInput, usize)>::new();
    for (record, (_, image, _)) in records.iter().zip(work_items(&workload, &frames)) {
        let regions: Vec<WorkloadRect> = match record.kind {
            RequestKind::Roi => vec![record.region],
            RequestKind::FullFrame => record
                .output
                .blocks
                .iter()
                .map(|block| block.rect)
                .collect(),
        };
        for region in regions {
            let input = engine_bench::recognition_input(
                &raw_recognizer,
                &model.params,
                &image.view(),
                vision_rect(region),
            )
            .map_err(|error| inference_error("the raw recognizer input", &error))?;
            let source = format!(
                "{}/{} {}x{}@{},{}",
                record.frame_id, record.request_id, region.width, region.height, region.x, region.y
            );
            recognizer_inputs
                .entry(shape_text(&input.shape))
                .or_insert_with(|| (source, input, 0))
                .2 += 1;
        }
    }
    for (source, input, regions) in recognizer_inputs.values() {
        let timing = time_raw(
            &mut raw_recognizer,
            input,
            source.clone(),
            *regions,
            options,
            monitor,
        )?;
        report.raw.recognizer.push(timing);
    }
    mark(report, monitor, "after_raw_recognizer")?;
    report.shapes.detector = detector_inputs.keys().cloned().collect();
    report.shapes.recognizer = recognizer_inputs.keys().cloned().collect();
    report.cold.distinct_input_shapes = detector_inputs.len() + recognizer_inputs.len();

    phase(report, watcher, "consistency");
    if let Some(baseline) = &baseline {
        report.consistency = Some(consistency::compare(baseline, &records)?);
    }

    phase(report, watcher, "modules");
    report.modules = win::loaded_modules()?;
    if runtime.execution_provider == OnnxExecutionProvider::Cuda {
        check_runtime_modules(&runtime, &report.modules, "at the end of the run")?;
    }
    if !placement_unread.is_empty() {
        phase(report, watcher, "placement");
        return Err(BenchError::new(
            BenchCode::PlacementUnread,
            format!(
                "every measurement finished, but ONNX Runtime's node placement could not be read: {}",
                placement_unread.join("; ")
            ),
        ));
    }
    Ok(())
}

/// Workflow #407 GPU-0c: the named failure of a raw session that did not build, with ONNX
/// Runtime's status whole. For a GPU variant the model's node placement is read and recorded
/// too, and the error names the nodes ONNX Runtime put on its CPU execution provider (why a
/// strict session fails).
fn failed_session(
    report: &mut Report,
    plan: &BenchSessionPlan,
    (name, bytes, path): (&'static str, &[u8], &Path),
    what: &str,
    failure: &BenchOrtFailure,
) -> BenchError {
    /// The CPU-placed nodes the error names one by one (the report lists them all).
    const NAMED_NODES: usize = 24;
    let mut error = BenchError::from_ort(BenchCode::SessionFailed, what, failure);
    if plan.execution_provider() == OnnxExecutionProvider::Cpu {
        return error;
    }
    let placement =
        ModelPlacement::from_read(name, engine_bench::node_placement(bytes, path, plan));
    if placement.cpu_nodes > 0 {
        let named: Vec<String> = placement
            .cpu_node_list
            .iter()
            .take(NAMED_NODES)
            .map(|node| format!("{} ({})", node.op_type, node.name))
            .collect();
        error.message.push_str(&format!(
            "; ONNX Runtime placed {} of the {} nodes of {name} on {CPU_EXECUTION_PROVIDER} ({}; read from {}): {}{}",
            placement.cpu_nodes,
            placement.cpu_nodes + placement.non_cpu_nodes,
            placement.cpu_op_summary(),
            placement.read_from,
            named.join(", "),
            if placement.cpu_node_list.len() > named.len() {
                ", …"
            } else {
                ""
            }
        ));
    } else if let Some(log_error) = &placement.log_error {
        error.message.push_str(&format!(
            "; the node placement of {name} could not be read: {log_error}"
        ));
    } else {
        error.message.push_str(&format!(
            "; ONNX Runtime placed none of the {} nodes of {name} on {CPU_EXECUTION_PROVIDER}",
            placement.non_cpu_nodes
        ));
    }
    report.placement.push(placement);
    error
}

/// The bench's own executable and its digest.
pub(crate) fn record_executable(report: &mut Report) {
    if let Ok(executable) = std::env::current_exe() {
        report.bench.executable_sha256 = std::fs::read(&executable)
            .ok()
            .map(|bytes| sha256_hex(&bytes));
        report.bench.executable = Some(executable.display().to_string());
    }
}

/// ONNX Runtime for this process: the vision root's runtime with the variant's DirectML adapter
/// and tuning, initialised as the engine initialises it, and the engine's session plan; fills
/// the runtime section and the initialisation time.
pub(crate) fn initialise(
    options: &RunOptions,
    cuda_selector: Option<CudaDeviceSelector>,
    report: &mut Report,
    monitor: &mut Monitor,
) -> Result<(VisionRuntimeSpec, BenchSessionPlan), BenchError> {
    let mut runtime = VisionRuntimeSpec::from_vision_root(
        &options.vision_root,
        options.execution_provider,
        cuda_selector,
    )
    .map_err(|error| runtime_error("the vision root's ONNX Runtime", &error))?;
    if options.execution_provider == OnnxExecutionProvider::Directml {
        runtime.directml_device_id = options.device;
    }
    runtime.session_tuning = options.tuning;
    mark(report, monitor, "before_ort_init")?;
    let started = Instant::now();
    let facts = engine_bench::initialise_runtime(&runtime, Instant::now() + LOAD_WAIT)
        .map_err(|error| runtime_error("ONNX Runtime initialisation", &error))?;
    report.cold.ort_init_us = Some(micros(started.elapsed()));
    mark(report, monitor, "after_ort_init")?;
    let plan = BenchSessionPlan::for_runtime(&runtime, &facts)
        .map_err(|error| runtime_error("the session plan", &error))?;
    report.runtime = Some(RuntimeSection {
        onnxruntime_library: facts.onnxruntime_library.display().to_string(),
        onnxruntime_version: facts.onnxruntime_version.clone(),
        onnxruntime_sha256: facts.onnxruntime_sha256.clone(),
        onnxruntime_build_info: facts.onnxruntime_build_info.clone(),
        runtime_library_closure: runtime
            .runtime_library_closure
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        onnxruntime_loaded_libraries: runtime
            .onnxruntime_loaded_libraries
            .iter()
            .map(|library| library.path.display().to_string())
            .collect(),
        cuda_driver_version: facts.cuda_driver_version,
        resolved_cuda_device: facts.resolved_cuda_device.clone(),
        registered_execution_providers: plan.registered_execution_providers().to_vec(),
        cpu_ep_registered: plan.cpu_ep_registered(),
        cpu_fallback_disabled: plan.cpu_fallback_disabled(),
    });
    Ok((runtime, plan))
}

/// Workflow #407 GPU-0d: once a CUDA session exists, every library ONNX Runtime loads itself (the
/// CUDA provider) is loaded from its declared path, and every loaded module named like a DLL of
/// the runtime's folder (`ort\`: the NVIDIA libraries the provider imports or loads later, ONNX
/// Runtime and its bridge) is loaded from that folder. A library that is not loaded, or a module
/// from any other folder, is `bench_runtime_unavailable` naming the paths.
pub(crate) fn check_runtime_modules(
    runtime: &VisionRuntimeSpec,
    modules: &[ModuleRecord],
    when: &str,
) -> Result<(), BenchError> {
    let unavailable = |detail: String| {
        BenchError::new(
            BenchCode::RuntimeUnavailable,
            format!("the CUDA runtime modules {when}: {detail}"),
        )
    };
    let folder = runtime.onnxruntime_library.parent().ok_or_else(|| {
        unavailable(format!(
            "the ONNX Runtime library {} has no folder",
            runtime.onnxruntime_library.display()
        ))
    })?;
    let listing_error = |error: std::io::Error| {
        unavailable(format!("{} cannot be listed: {error}", folder.display()))
    };
    let mut shipped = BTreeSet::new();
    for entry in std::fs::read_dir(folder).map_err(listing_error)? {
        let name = entry
            .map_err(listing_error)?
            .file_name()
            .to_string_lossy()
            .to_lowercase();
        if name.ends_with(".dll") {
            shipped.insert(name);
        }
    }
    let not_loaded: Vec<String> = runtime
        .onnxruntime_loaded_libraries
        .iter()
        .filter(|library| {
            !modules
                .iter()
                .any(|module| same_path(Path::new(&module.path), &library.path))
        })
        .map(|library| library.path.display().to_string())
        .collect();
    let foreign: Vec<&str> = modules
        .iter()
        .filter(|module| {
            let path = Path::new(&module.path);
            path.file_name()
                .is_some_and(|name| shipped.contains(&name.to_string_lossy().to_lowercase()))
                && !path
                    .parent()
                    .is_some_and(|parent| same_path(parent, folder))
        })
        .map(|module| module.path.as_str())
        .collect();
    let mut problems = Vec::new();
    if !not_loaded.is_empty() {
        problems.push(format!("not loaded: {}", not_loaded.join(", ")));
    }
    if !foreign.is_empty() {
        problems.push(format!(
            "loaded from outside {}: {}",
            folder.display(),
            foreign.join(", ")
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(unavailable(problems.join("; ")))
    }
}

/// Whether two paths name the same file or folder: equal as absolute paths with letters compared
/// without case, as Windows compares them, or equal once canonicalised.
fn same_path(left: &Path, right: &Path) -> bool {
    let key = |path: &Path| {
        strip_verbatim(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    key(left) == key(right)
        || matches!(
            (left.canonicalize(), right.canonicalize()),
            (Ok(left), Ok(right)) if left == right
        )
}

/// The DXGI adapters, the DirectML adapter and the CUDA device; the CUDA selector of a CUDA run.
pub(crate) fn host_facts(
    options: &RunOptions,
    report: &mut Report,
) -> Result<Option<CudaDeviceSelector>, BenchError> {
    match win::dxgi_adapters() {
        Ok(adapters) => report.host.dxgi_adapters = adapters,
        Err(error) if options.execution_provider == OnnxExecutionProvider::Directml => {
            return Err(error);
        }
        Err(error) => report.host.dxgi_error = Some(error),
    }
    let device = options.device.unwrap_or_default();
    match options.execution_provider {
        OnnxExecutionProvider::Cpu => Ok(None),
        OnnxExecutionProvider::Directml => {
            let adapter = report
                .host
                .dxgi_adapters
                .iter()
                .find(|adapter| adapter.index == device)
                .cloned()
                .ok_or_else(|| {
                    BenchError::new(
                        BenchCode::DeviceUnknown,
                        format!(
                            "DXGI adapter index {device} does not exist; the host lists {} adapters (run --list-adapters)",
                            report.host.dxgi_adapters.len()
                        ),
                    )
                })?;
            if adapter.software {
                return Err(BenchError::new(
                    BenchCode::DeviceUnknown,
                    format!(
                        "DXGI adapter {device} ({}) is a software adapter",
                        adapter.description
                    ),
                ));
            }
            report.host.directml_adapter = Some(adapter);
            Ok(None)
        }
        OnnxExecutionProvider::Cuda => {
            let inventory = enumerate_cuda_devices()
                .map_err(|error| runtime_error("the CUDA device inventory", &error))?;
            let selected = inventory
                .devices
                .iter()
                .find(|candidate| candidate.ordinal == device)
                .cloned()
                .ok_or_else(|| {
                    BenchError::new(
                        BenchCode::DeviceUnknown,
                        format!(
                            "CUDA ordinal {device} does not exist; the driver lists {} devices",
                            inventory.devices.len()
                        ),
                    )
                })?;
            let selector = CudaDeviceSelector {
                ordinal: device,
                expected_stable_identity: selected.stable_identity.clone(),
            };
            report.host.cuda = Some(CudaSection {
                driver_version: inventory.driver_version,
                devices: inventory.devices,
                selected,
            });
            Ok(Some(selector))
        }
    }
}

pub(crate) fn read_workload(path: &Path, report: &mut Report) -> Result<Workload, BenchError> {
    let bytes = std::fs::read(path).map_err(|error| {
        BenchError::new(
            BenchCode::WorkloadInvalid,
            format!("the workload {} cannot be read: {error}", path.display()),
        )
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        BenchError::new(
            BenchCode::WorkloadInvalid,
            format!("the workload is not UTF-8: {error}"),
        )
    })?;
    let workload = parse_workload(text)?;
    let (full_frame_requests, roi_requests) = workload.request_counts();
    report.workload = Some(WorkloadSection {
        path: path.display().to_string(),
        sha256: sha256_hex(&bytes),
        frames: workload.frames.len(),
        full_frame_requests,
        roi_requests,
        model_ref: workload.model.model_ref.clone(),
        model_sha256: workload.model.model_sha256.clone(),
    });
    Ok(workload)
}

fn read_baseline(path: &Path) -> Result<BaselineReport, BenchError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        BenchError::new(
            BenchCode::BaselineInvalid,
            format!("the baseline {} cannot be read: {error}", path.display()),
        )
    })?;
    consistency::parse_baseline(&text)
}

/// A frame's PNG bytes, read from outside the production install root and checked against the
/// digest the workload records.
pub(crate) fn read_frame_png(frame: &WorkloadFrame) -> Result<Vec<u8>, BenchError> {
    refuse_production_path("frame", &frame.path)?;
    let bytes = std::fs::read(&frame.path)
        .map_err(|error| frame_invalid(frame, format!("cannot be read: {error}")))?;
    let digest = sha256_hex(&bytes);
    if digest != frame.sha256 {
        return Err(frame_invalid(
            frame,
            format!("SHA-256 is {digest}, the workload records {}", frame.sha256),
        ));
    }
    Ok(bytes)
}

pub(crate) fn frame_invalid(frame: &WorkloadFrame, message: String) -> BenchError {
    BenchError::new(
        BenchCode::FrameInvalid,
        format!("frame {} ({}): {message}", frame.id, frame.path.display()),
    )
}

/// Every frame, checked against its digest and size and decoded to RGB.
fn load_frames(workload: &Workload) -> Result<Vec<VisionFrame>, BenchError> {
    let mut frames = Vec::with_capacity(workload.frames.len());
    for frame in &workload.frames {
        let invalid = |message: String| frame_invalid(frame, message);
        let bytes = read_frame_png(frame)?;
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map_err(|error| invalid(format!("is no readable PNG: {error}")))?
            .to_rgb8();
        if decoded.width() != frame.width || decoded.height() != frame.height {
            return Err(invalid(format!(
                "is {}x{}, the workload records {}x{}",
                decoded.width(),
                decoded.height(),
                frame.width,
                frame.height
            )));
        }
        let (width, height) = (decoded.width(), decoded.height());
        frames.push(
            VisionFrame::new(width, height, VisionPixelFormat::Rgb8, decoded.into_raw())
                .map_err(|error| invalid(error.to_string()))?,
        );
    }
    Ok(frames)
}

/// The workload's OCR model folder, its files read once and its content identity checked.
fn read_model(
    options: &RunOptions,
    workload: &Workload,
    report: &mut Report,
) -> Result<Model, BenchError> {
    let model_error = |message: String| BenchError::new(BenchCode::ModelInvalid, message);
    let listing = list_vision_models(&options.models_dir)
        .map_err(|error| model_error(format!("the models folder: {error}")))?;
    let spec = listing
        .ocr
        .iter()
        .find(|spec| spec.model_ref == workload.model.model_ref)
        .cloned()
        .ok_or_else(|| {
            model_error(format!(
                "{} holds no OCR folder {}",
                options.models_dir.display(),
                workload.model.model_ref
            ))
        })?;
    let detector_path = spec.detector_path.clone().ok_or_else(|| {
        model_error(format!(
            "OCR folder {} has no detector; the full-frame reads need it",
            spec.model_ref
        ))
    })?;
    let read = |path: &Path| {
        std::fs::read(path)
            .map_err(|error| model_error(format!("{} cannot be read: {error}", path.display())))
    };
    let detector = read(detector_path.as_path())?;
    let recognizer = read(spec.recognizer_path.as_path())?;
    let dictionary = read(spec.dictionary_path.as_path())?;
    let hashes = [
        sha256_hex(&detector),
        sha256_hex(&recognizer),
        sha256_hex(&dictionary),
    ];
    let model_sha256 = ppocr_model_set_sha256(Some(&hashes[0]), &hashes[1], &hashes[2], None)
        .map_err(|error| model_error(error.to_string()))?;
    for (role, path, bytes, sha256) in [
        ("det", &detector_path, detector.len(), &hashes[0]),
        ("rec", &spec.recognizer_path, recognizer.len(), &hashes[1]),
        (
            "dictionary",
            &spec.dictionary_path,
            dictionary.len(),
            &hashes[2],
        ),
    ] {
        report.models.push(ModelRow {
            role,
            path: Some(path.display().to_string()),
            bytes: Some(bytes as u64),
            sha256: Some(sha256.clone()),
            status: "measured".to_owned(),
        });
    }
    report.models.push(ModelRow {
        role: "nn",
        path: None,
        bytes: None,
        sha256: None,
        status: if listing.nn.is_empty() {
            "n/a: the vision root holds no NN classifier".to_owned()
        } else {
            format!(
                "n/a: GPU-0a measures the OCR model only ({} NN folders listed)",
                listing.nn.len()
            )
        },
    });
    if model_sha256 != workload.model.model_sha256 {
        return Err(BenchError::new(
            BenchCode::ModelIdentityMismatch,
            format!(
                "OCR folder {} hashes to the model set {model_sha256}, the workload names {}",
                spec.model_ref, workload.model.model_sha256
            ),
        ));
    }
    let params = spec.description.ppocr_params();
    Ok(Model {
        spec,
        detector_path,
        detector,
        recognizer,
        params,
    })
}

/// Every request of the workload with its frame and decoded image, in workload order (the
/// order of the engine records).
fn work_items<'a>(
    workload: &'a Workload,
    frames: &'a [VisionFrame],
) -> impl Iterator<Item = (&'a WorkloadFrame, &'a VisionFrame, &'a WorkloadRequest)> {
    workload
        .frames
        .iter()
        .zip(frames)
        .flat_map(|(frame, image)| {
            frame
                .requests
                .iter()
                .map(move |request| (frame, image, request))
        })
}

/// One raw input shape: its first `session.run`, the warm-up, then the timed repetitions.
fn time_raw(
    session: &mut Session,
    input: &BenchInput,
    source: String,
    regions: usize,
    options: &RunOptions,
    monitor: &mut Monitor,
) -> Result<ShapeTiming, BenchError> {
    let shape = shape_text(&input.shape);
    let tensor = Tensor::from_array((input.shape.clone(), input.data.clone().into_boxed_slice()))
        .map_err(|error| {
        BenchError::from_ort_error(
            BenchCode::InferenceFailed,
            &format!("the {shape} input tensor"),
            &error,
        )
    })?;
    monitor.check()?;
    let started = Instant::now();
    run_raw(session, &tensor, &shape)?;
    let first_run_us = micros(started.elapsed());
    for _ in 0..options.warmup {
        monitor.check()?;
        run_raw(session, &tensor, &shape)?;
    }
    let mut wall = Vec::with_capacity(options.reps as usize);
    let mut cpu = Vec::with_capacity(options.reps as usize);
    for _ in 0..options.reps {
        monitor.check()?;
        let cpu_before = win::process_cpu_time_us()?;
        let started = Instant::now();
        run_raw(session, &tensor, &shape)?;
        wall.push(micros(started.elapsed()));
        cpu.push(win::process_cpu_time_us()?.saturating_sub(cpu_before));
    }
    Ok(ShapeTiming {
        shape,
        source,
        regions,
        first_run_us,
        warm: percentiles(&wall),
        warm_cpu: percentiles(&cpu),
    })
}

fn run_raw(session: &mut Session, tensor: &Tensor<f32>, shape: &str) -> Result<(), BenchError> {
    let outputs = session.run(ort::inputs![tensor]).map_err(|error| {
        BenchError::from_ort_error(
            BenchCode::InferenceFailed,
            &format!("raw session.run on {shape} failed"),
            &error,
        )
    })?;
    drop(outputs);
    Ok(())
}

/// One engine read of one workload request.
fn read(
    engine: &mut (dyn OcrEngine + Send),
    image: &VisionFrame,
    frame: &WorkloadFrame,
    request: &WorkloadRequest,
    gap_factor: u32,
) -> Result<OcrInferenceResult, BenchError> {
    let blocks = match request.blocks {
        WorkloadBlocks::Implicit => OcrBlockMode::Implicit,
        WorkloadBlocks::Region => OcrBlockMode::Region,
        WorkloadBlocks::Detect => OcrBlockMode::Detect {
            line_merge: OcrLineMerge::GapFactor(gap_factor),
        },
    };
    engine
        .read_text_view(OcrInferenceRequestView {
            frame: image.view(),
            region: vision_rect(request.region),
            languages: &request.languages,
            timeout_ms: request.timeout_ms,
            blocks,
        })
        .map_err(|error| {
            BenchError::new(
                BenchCode::InferenceFailed,
                format!("frame {} request {}: {error}", frame.id, request.id),
            )
        })
}

/// The input shapes an engine read uses: the detector's for a full-frame read and one
/// recognizer shape per block read (blocks with empty text are not reported by the engine).
fn request_shapes(
    detector: &Session,
    recognizer: &Session,
    params: &PpocrCtcParams,
    image: &VisionFrame,
    request: &WorkloadRequest,
    kind: RequestKind,
    output: &RequestOutput,
) -> Result<Vec<String>, BenchError> {
    let view = image.view();
    let recognizer_shape = |region: WorkloadRect| {
        engine_bench::recognition_input(recognizer, params, &view, vision_rect(region))
            .map(|input| format!("rec:{}", shape_text(&input.shape)))
            .map_err(|error| inference_error("a recognizer input shape", &error))
    };
    match kind {
        RequestKind::Roi => Ok(vec![recognizer_shape(request.region)?]),
        RequestKind::FullFrame => {
            let detector_input =
                engine_bench::detection_input(detector, params, &view, vision_rect(request.region))
                    .map_err(|error| inference_error("a detector input shape", &error))?;
            let mut shapes = vec![format!("det:{}", shape_text(&detector_input.shape))];
            for block in &output.blocks {
                shapes.push(recognizer_shape(block.rect)?);
            }
            Ok(shapes)
        }
    }
}

fn output_of(result: &OcrInferenceResult) -> RequestOutput {
    RequestOutput {
        text: result.text.clone(),
        confidence: result.confidence,
        blocks: result
            .blocks
            .iter()
            .map(|block| BlockOutput {
                text: block.text.clone(),
                rect: WorkloadRect {
                    x: block.rect.x,
                    y: block.rect.y,
                    width: block.rect.width,
                    height: block.rect.height,
                },
                confidence: block.confidence,
            })
            .collect(),
        detected_boxes: result.detection.map(|count| count.detected),
        box_limit: result.detection.map(|count| count.limit),
    }
}

fn vision_rect(rect: WorkloadRect) -> VisionRect {
    VisionRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn shape_text(shape: &[i64]) -> String {
    shape
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join("x")
}

pub(crate) fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

// Workflow #407 GPU-0c: every conversion keeps the library load behind a vision error.
fn runtime_error(what: &str, error: &VisionFfiError) -> BenchError {
    BenchError::from_vision(BenchCode::RuntimeUnavailable, what, error)
}

pub(crate) fn session_error(what: &str, error: &VisionFfiError) -> BenchError {
    BenchError::from_vision(BenchCode::SessionFailed, what, error)
}

fn inference_error(what: &str, error: &VisionFfiError) -> BenchError {
    BenchError::from_vision(BenchCode::InferenceFailed, what, error)
}
