// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0b: the template mode's run (design §1.2 "模板匹配", §1.5 "GPU-0b", §2.5).
//!
//! Every template of the workload's `template_search` runs without and with its mask, once per
//! scale set, over every whole frame: one case each. Per case, after a cold first run and the
//! warm-up on the first frame, every frame (and repetition) is timed in three rows:
//!
//! 1. the CPU exact score map, `Scene::exact_score_map` (one scale) or `Scene::exact_score_maps`
//!    (several), with the production 5 s limit on the system clock;
//! 2. today's direct enumeration, `Scene::match_template_all`, which takes no mask and one scale
//!    (several scales run one search per resampled template), each search with its production
//!    5 s limit; a search the limit stops counts as incomplete;
//! 3. the generated Conv graph (`onnx.rs`) on this process's execution provider, and the CPU
//!    assembly of the float score from its sums.
//!
//! On the first repetition of every frame the float scores are compared with the exact map's
//! (read back through `ExactScoreMap::select` from a second map whose clock stands still, so the
//! read-back never meets the limit). After the last frame, ε = 4 × the case's largest |Δ| gives
//! the recall of the exact matches at `threshold − ε` and the positions kept there.
//!
//! A case whose graph, session, run or search fails is recorded with its named failure and the
//! next case runs; the process still fails at the end, naming them. Safety stops end the run at
//! once, as in the OCR mode.

use crate::cli::RunOptions;
use crate::error::{BenchCode, BenchError};
use crate::onnx::{self, ConvGraph, ConvOutputKind, output_name};
use crate::report::{
    AccuracyRow, ConvRow, DirectEnumerationRow, ExactMapRow, Report, ScaledTemplateRecord,
    TemplateCaseFailure, TemplateCaseRecord, TemplateFrameRecord, TemplateRecord, TemplateSection,
};
use crate::run::{self, Monitor};
use crate::stats::percentiles;
use crate::template::{
    self, MARGIN_FACTOR, MASK_MIN_SELECTED_PIXELS, RecallCounts, ScaleSums, TemplateStatistics,
    WindowSums,
};
use crate::watch::Watcher;
use crate::win;
use crate::workload::{TemplateMetric, TemplateSearchSpec, Workload, WorkloadTemplate, decode_hex};
use actingcommand_ppocr_onnx_json_provider::bench::{self as engine_bench, BenchSessionPlan};
use actingcommand_recognition::resample::{self, SCALE_IDENTITY_MILLI};
use actingcommand_recognition::{
    ExactScoreMap, MatchMetric, RecognitionError, RegionSpectra, ScaledScoreMaps, Scene,
    SearchClock, TemplateMask, TemplateMatchTimingStage, TemplateSearch,
};
use actingcommand_vision_ffi::{OnnxExecutionProvider, sha256_hex};
use image::{GrayImage, ImageFormat};
use ort::session::{Session, SessionOutputs};
use ort::value::Tensor;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

/// One whole frame: its scene, as the recognition crate reads it, and its gray image.
struct Frame {
    id: String,
    scene: Scene,
    gray: GrayImage,
}

/// One workload template, decoded, with every scale the scale sets name.
struct PreparedTemplate {
    spec: WorkloadTemplate,
    png: Vec<u8>,
    metric: MatchMetric,
    mask: TemplateMask,
    scaled: BTreeMap<u32, ScaledTemplate>,
}

/// A template resampled by one factor, as every search resamples it, and its PNG for today's
/// direct enumeration (which reads a PNG and takes no scale).
struct ScaledTemplate {
    gray: GrayImage,
    png: Vec<u8>,
}

struct Context<'a> {
    options: &'a RunOptions,
    plan: &'a BenchSessionPlan,
    frames: &'a [Frame],
    width: u32,
    height: u32,
    suppress_iou_milli: u16,
}

pub(crate) fn measure(
    options: &RunOptions,
    report: &mut Report,
    watcher: &Watcher,
    monitor: &mut Monitor,
) -> Result<(), BenchError> {
    run::phase(report, watcher, "host");
    run::record_executable(report);
    let cuda_selector = run::host_facts(options, report)?;

    run::phase(report, watcher, "workload");
    let workload = run::read_workload(&options.workload, report)?;
    let spec = workload.template_search.clone().ok_or_else(|| {
        BenchError::new(
            BenchCode::WorkloadInvalid,
            "--mode template needs a workload with template_search",
        )
    })?;
    let frames = load_frames(&workload)?;
    let (width, height) = frame_size(&workload)?;
    let templates = prepare_templates(&spec, width, height)?;
    run::mark(report, monitor, "after_template_workload")?;

    run::phase(report, watcher, "ort_init");
    let (runtime, plan) = run::initialise(options, cuda_selector, report, monitor)?;

    report.template = Some(TemplateSection {
        frame_width: width,
        frame_height: height,
        frames: frames.iter().map(|frame| frame.id.clone()).collect(),
        direct_enumeration: options.direct_enumeration,
        suppress_iou_milli: spec.suppress_iou_milli,
        margin_factor: MARGIN_FACTOR,
        templates: spec
            .templates
            .iter()
            .map(|template| TemplateRecord {
                id: template.id.clone(),
                size_class_px: template.size_class_px,
                source: template.source.clone(),
                sha256: template.sha256.clone(),
                width: template.width,
                height: template.height,
                metric: template.metric,
                threshold: template.threshold,
                mask: template.mask,
            })
            .collect(),
        cases: Vec::new(),
        failed_cases: Vec::new(),
    });
    let context = Context {
        options,
        plan: &plan,
        frames: &frames,
        width,
        height,
        suppress_iou_milli: spec.suppress_iou_milli,
    };
    let mut total = 0_usize;
    let mut failures = Vec::new();
    let mut inference_checked = false;
    for template in &templates {
        for masked in [false, true] {
            for scales in &spec.scale_sets_milli {
                total += 1;
                let label = case_label(&template.spec.id, masked, scales);
                run::phase(report, watcher, &format!("template {label}"));
                let outcome = run_case(&context, template, masked, scales, &label, monitor);
                let ran = outcome.is_ok();
                let Some(section) = report.template.as_mut() else {
                    return Err(BenchError::new(
                        BenchCode::OutputUnwritable,
                        "the template section vanished from the report",
                    ));
                };
                match outcome {
                    Ok(record) => section.cases.push(record),
                    Err(error) if is_case_failure(error.code) => {
                        eprintln!("ac-vision-ep-bench: template case {label}: {error}");
                        failures.push(format!("{label} ({})", error.code));
                        section.failed_cases.push(TemplateCaseFailure {
                            case: label.clone(),
                            error,
                        });
                    }
                    Err(error) => return Err(error),
                }
                // Workflow #407 GPU-0e: after the first case that ran its Conv graph, every
                // cuDNN library loaded comes from the runtime's own folder.
                if ran
                    && !inference_checked
                    && runtime.execution_provider == OnnxExecutionProvider::Cuda
                {
                    inference_checked = true;
                    report.modules = win::loaded_modules()?;
                    run::check_runtime_modules(&runtime, report, "after the first inference")?;
                }
                run::mark(report, monitor, &format!("after_template {label}"))?;
            }
        }
    }

    run::phase(report, watcher, "modules");
    report.modules = win::loaded_modules()?;
    let first_failure = report
        .template
        .as_ref()
        .and_then(|section| section.failed_cases.first())
        .map(|failure| failure.error.code);
    if let Some(code) = first_failure {
        return Err(BenchError::new(
            code,
            format!(
                "{} of {total} template cases failed: {}",
                failures.len(),
                failures.join(", ")
            ),
        ));
    }
    // Workflow #407 GPU-0d: after the CUDA sessions, the provider and the NVIDIA libraries come
    // from the runtime's own folder (a failed case is reported first, above).
    if runtime.execution_provider == OnnxExecutionProvider::Cuda {
        run::check_runtime_modules(&runtime, report, "at the end of the run")?;
    }
    Ok(())
}

/// A failure that belongs to one case; any other ends the run.
fn is_case_failure(code: BenchCode) -> bool {
    matches!(
        code,
        BenchCode::GraphInvalid
            | BenchCode::SessionFailed
            | BenchCode::InferenceFailed
            | BenchCode::TemplateSearchFailed
    )
}

fn case_label(template_id: &str, masked: bool, scales: &[u32]) -> String {
    let scales = scales
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("-");
    let mask = if masked { "masked" } else { "unmasked" };
    format!("{template_id}/{mask}/{scales}")
}

fn load_frames(workload: &Workload) -> Result<Vec<Frame>, BenchError> {
    let mut frames = Vec::with_capacity(workload.frames.len());
    for frame in &workload.frames {
        let bytes = run::read_frame_png(frame)?;
        let scene = Scene::from_png(&bytes)
            .map_err(|error| run::frame_invalid(frame, format!("is no readable scene: {error}")))?;
        let gray = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
            .map_err(|error| run::frame_invalid(frame, format!("is no readable PNG: {error}")))?
            .to_luma8();
        if (scene.width(), scene.height()) != (frame.width, frame.height)
            || gray.dimensions() != (frame.width, frame.height)
        {
            return Err(run::frame_invalid(
                frame,
                format!(
                    "is {}x{}, the workload records {}x{}",
                    scene.width(),
                    scene.height(),
                    frame.width,
                    frame.height
                ),
            ));
        }
        frames.push(Frame {
            id: frame.id.clone(),
            scene,
            gray,
        });
    }
    Ok(frames)
}

/// The one size every frame of the template mode shares (one graph serves every frame).
fn frame_size(workload: &Workload) -> Result<(u32, u32), BenchError> {
    let sizes: BTreeSet<(u32, u32)> = workload
        .frames
        .iter()
        .map(|frame| (frame.width, frame.height))
        .collect();
    match sizes.into_iter().collect::<Vec<_>>().as_slice() {
        [size] => Ok(*size),
        sizes => Err(BenchError::new(
            BenchCode::WorkloadInvalid,
            format!("--mode template needs frames of one size; the workload has {sizes:?}"),
        )),
    }
}

fn prepare_templates(
    spec: &TemplateSearchSpec,
    frame_width: u32,
    frame_height: u32,
) -> Result<Vec<PreparedTemplate>, BenchError> {
    let factors: BTreeSet<u32> = spec.scale_sets_milli.iter().flatten().copied().collect();
    let mut templates = Vec::with_capacity(spec.templates.len());
    for template in &spec.templates {
        let invalid = |message: String| {
            BenchError::new(
                BenchCode::WorkloadInvalid,
                format!("template {}: {message}", template.id),
            )
        };
        let png = decode_hex(&template.png_hex).map_err(|error| invalid(error.message))?;
        let digest = sha256_hex(&png);
        if digest != template.sha256 {
            return Err(invalid(format!(
                "SHA-256 is {digest}, the workload records {}",
                template.sha256
            )));
        }
        let gray = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .map_err(|error| invalid(format!("is no readable PNG: {error}")))?
            .to_luma8();
        if gray.dimensions() != (template.width, template.height) {
            return Err(invalid(format!(
                "is {}x{}, the workload records {}x{}",
                gray.width(),
                gray.height(),
                template.width,
                template.height
            )));
        }
        let mask = TemplateMask {
            lower: template.mask.lower,
            upper: template.mask.upper,
        };
        let mut scaled = BTreeMap::new();
        for factor in &factors {
            let (scaled_gray, scaled_png) = if *factor == SCALE_IDENTITY_MILLI {
                (gray.clone(), png.clone())
            } else {
                let width = resample::scaled_length(gray.width(), *factor);
                let height = resample::scaled_length(gray.height(), *factor);
                if width == 0 || height == 0 {
                    return Err(invalid(format!("scaled by {factor} milli it has no pixel")));
                }
                let resampled = resample::resample(&gray, width, height)
                    .map_err(|error| invalid(format!("resampling by {factor} milli: {error}")))?;
                let encoded = encode_png(&resampled).map_err(invalid)?;
                (resampled, encoded)
            };
            if scaled_gray.width() > frame_width || scaled_gray.height() > frame_height {
                return Err(invalid(format!(
                    "scaled by {factor} milli it is {}x{}, larger than the {frame_width}x{frame_height} frames",
                    scaled_gray.width(),
                    scaled_gray.height()
                )));
            }
            let selected = TemplateStatistics::of(&scaled_gray, Some(mask)).selected_pixels;
            if selected < MASK_MIN_SELECTED_PIXELS {
                return Err(invalid(format!(
                    "scaled by {factor} milli its mask selects {selected} pixels, fewer than {MASK_MIN_SELECTED_PIXELS}"
                )));
            }
            scaled.insert(
                *factor,
                ScaledTemplate {
                    gray: scaled_gray,
                    png: scaled_png,
                },
            );
        }
        templates.push(PreparedTemplate {
            spec: template.clone(),
            png,
            metric: match_metric(template.metric),
            mask,
            scaled,
        });
    }
    Ok(templates)
}

fn encode_png(gray: &GrayImage) -> Result<Vec<u8>, String> {
    let mut bytes = Cursor::new(Vec::new());
    gray.write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| format!("the resampled template does not encode as PNG: {error}"))?;
    Ok(bytes.into_inner())
}

fn match_metric(metric: TemplateMetric) -> MatchMetric {
    match metric {
        TemplateMetric::CcorrNormed => MatchMetric::CrossCorrelationNormalized,
        TemplateMetric::CcoeffNormed => MatchMetric::CorrelationCoefficientNormalized,
    }
}

/// The samples of one case's timed loop.
#[derive(Default)]
struct Samples {
    exact_us: Vec<u64>,
    exact_incomplete: u64,
    direct_us: Vec<u64>,
    direct_complete: u64,
    direct_incomplete: u64,
    direct_matches: u64,
    run_us: Vec<u64>,
    prefilter_us: Vec<u64>,
    prefilter_cpu_us: Vec<u64>,
}

fn run_case(
    context: &Context<'_>,
    template: &PreparedTemplate,
    masked: bool,
    scales: &[u32],
    label: &str,
    monitor: &mut Monitor,
) -> Result<TemplateCaseRecord, BenchError> {
    let mask = masked.then_some(template.mask);
    let scaled = scales
        .iter()
        .map(|factor| {
            template.scaled.get(factor).ok_or_else(|| {
                BenchError::new(
                    BenchCode::WorkloadInvalid,
                    format!("{label}: scale {factor} milli was not prepared"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut scale_records = Vec::with_capacity(scaled.len());
    let mut statistics = Vec::with_capacity(scaled.len());
    let mut positions = Vec::with_capacity(scaled.len());
    for (factor, scale) in scales.iter().zip(&scaled) {
        let (width, height) = scale.gray.dimensions();
        let columns = (context.width - width + 1) as usize;
        let rows = (context.height - height + 1) as usize;
        let scale_statistics = TemplateStatistics::of(&scale.gray, mask);
        scale_records.push(ScaledTemplateRecord {
            scale_milli: *factor,
            width,
            height,
            positions: (columns * rows) as u64,
            scored_pixels: scale_statistics.selected_pixels,
            exact_map_engine: template::exact_map_engine(
                context.width,
                context.height,
                width,
                height,
            ),
        });
        statistics.push(scale_statistics);
        positions.push((columns, rows));
    }
    let positions_per_frame: usize = positions.iter().map(|(columns, rows)| columns * rows).sum();

    let graph = ConvGraph {
        frame_width: context.width,
        frame_height: context.height,
        scales: scaled
            .iter()
            .map(|scale| template::conv_kernel(&scale.gray, mask))
            .collect(),
    };
    let bytes = graph.encode()?;
    let description = onnx::describe_model(&bytes)?;
    if description.outputs.len() != graph.outputs().len() {
        return Err(BenchError::new(
            BenchCode::GraphInvalid,
            format!(
                "{label}: the graph reads back with {} outputs, it was written with {}",
                description.outputs.len(),
                graph.outputs().len()
            ),
        ));
    }
    let started = Instant::now();
    let session = engine_bench::build_session(
        &bytes,
        Path::new(&format!("generated:{label}")),
        context.plan,
    )
    .map_err(|failure| {
        BenchError::from_ort(
            BenchCode::SessionFailed,
            &format!("the Conv graph session of {label}"),
            &failure,
        )
    })?;
    let session_build_us = run::micros(started.elapsed());
    let mut conv = ConvSearch {
        session,
        statistics,
        positions,
        metric: template.metric,
        masked,
        input_shape: graph.input_shape().to_vec(),
    };

    let first_frame = &context.frames[0];
    monitor.check()?;
    let first_run_us = conv.prefilter(&first_frame.gray)?.run_us;
    let direct = !masked && context.options.direct_enumeration;
    for _ in 0..context.options.warmup {
        monitor.check()?;
        timed_exact(&first_frame.scene, template, mask, scales)?;
        conv.prefilter(&first_frame.gray)?;
    }
    if direct && context.options.warmup > 0 {
        // The direct scorer keeps nothing between searches: one search warms the caches.
        monitor.check()?;
        timed_direct(
            &first_frame.scene,
            template,
            scales,
            context.suppress_iou_milli,
        )?;
    }

    let mut samples = Samples::default();
    let mut frame_records = Vec::with_capacity(context.frames.len());
    let mut kept_inputs = Vec::with_capacity(context.frames.len());
    let mut max_delta = 0.0_f64;
    for frame in context.frames {
        for repetition in 0..context.options.reps {
            monitor.check()?;
            let (exact_us, exact_incomplete) = timed_exact(&frame.scene, template, mask, scales)?;
            samples.exact_us.push(exact_us);
            samples.exact_incomplete += u64::from(exact_incomplete);
            let direct_run = if direct {
                monitor.check()?;
                let search =
                    timed_direct(&frame.scene, template, scales, context.suppress_iou_milli)?;
                samples.direct_us.push(search.us);
                samples.direct_complete += search.complete;
                samples.direct_incomplete += search.incomplete;
                samples.direct_matches += search.matches;
                Some(search)
            } else {
                None
            };
            monitor.check()?;
            let prefilter = conv.prefilter(&frame.gray)?;
            samples.run_us.push(prefilter.run_us);
            samples.prefilter_us.push(prefilter.total_us);
            samples.prefilter_cpu_us.push(prefilter.cpu_us);
            if repetition != 0 {
                continue;
            }
            monitor.check()?;
            let exact = exact_raw_scores(&frame.scene, template, mask, scales, positions_per_frame)
                .map_err(|error| prefixed(label, &frame.id, error))?;
            if prefilter.scores.len() != exact.len() {
                return Err(BenchError::new(
                    BenchCode::InferenceFailed,
                    format!(
                        "{label} frame {}: the float path scored {} positions, the exact map {}",
                        frame.id,
                        prefilter.scores.len(),
                        exact.len()
                    ),
                ));
            }
            let delta = template::max_abs_delta(&exact, &prefilter.scores);
            max_delta = max_delta.max(delta);
            let matches = template::exact_match_indices(&exact, template.spec.threshold);
            drop(exact);
            let float_normalized: Vec<f32> = prefilter
                .scores
                .iter()
                .copied()
                .map(template::normalized)
                .collect();
            kept_inputs.push((float_normalized, matches));
            frame_records.push(TemplateFrameRecord {
                frame_id: frame.id.clone(),
                exact_map_us: exact_us,
                exact_map_incomplete: exact_incomplete,
                direct_us: direct_run.as_ref().map(|search| search.us),
                direct_incomplete: direct_run.as_ref().map(|search| search.incomplete > 0),
                conv_run_us: prefilter.run_us,
                prefilter_us: prefilter.total_us,
                max_abs_delta: delta,
                counts: RecallCounts::default(),
            });
        }
    }

    let margin = MARGIN_FACTOR * max_delta;
    let mut counts = RecallCounts::default();
    let mut kept_per_frame = Vec::with_capacity(frame_records.len());
    for (record, (float_normalized, matches)) in frame_records.iter_mut().zip(&kept_inputs) {
        let frame_counts =
            template::count_kept(float_normalized, matches, template.spec.threshold, margin);
        record.counts = frame_counts;
        counts.add(frame_counts);
        kept_per_frame.push(frame_counts.kept);
    }

    let direct_status = if masked {
        "skipped: today's direct enumeration takes no mask".to_owned()
    } else if !context.options.direct_enumeration {
        "skipped: --direct-enumeration off".to_owned()
    } else {
        "measured".to_owned()
    };
    Ok(TemplateCaseRecord {
        case: label.to_owned(),
        template_id: template.spec.id.clone(),
        masked,
        scales_milli: scales.to_vec(),
        scales: scale_records,
        positions_per_frame: positions_per_frame as u64,
        exact_map: ExactMapRow {
            call: if matches!(scales, [SCALE_IDENTITY_MILLI]) {
                "Scene::exact_score_map"
            } else {
                "Scene::exact_score_maps"
            },
            warm: percentiles(&samples.exact_us),
            incomplete: samples.exact_incomplete,
        },
        direct_enumeration: DirectEnumerationRow {
            status: direct_status,
            searches_per_frame: if direct { scales.len() } else { 0 },
            warm: percentiles(&samples.direct_us),
            complete: samples.direct_complete,
            incomplete: samples.direct_incomplete,
            matches: samples.direct_matches,
        },
        conv: ConvRow {
            graph_bytes: bytes.len(),
            graph_sha256: sha256_hex(&bytes),
            nodes: description.nodes.len(),
            outputs: description.outputs.len(),
            session_build_us,
            first_run_us,
            run: percentiles(&samples.run_us),
            prefilter: percentiles(&samples.prefilter_us),
            prefilter_cpu: percentiles(&samples.prefilter_cpu_us),
        },
        accuracy: AccuracyRow {
            max_abs_delta: max_delta,
            margin,
            counts,
            recall: counts.recall(),
            kept_per_frame: percentiles(&kept_per_frame),
        },
        frames: frame_records,
    })
}

/// The exact maps of one search.
enum ExactMaps {
    One(ExactScoreMap),
    Scaled(ScaledScoreMaps),
}

impl ExactMaps {
    fn maps(&self) -> &[ExactScoreMap] {
        match self {
            Self::One(map) => std::slice::from_ref(map),
            Self::Scaled(maps) => maps.maps(),
        }
    }
}

/// `Scene::exact_score_map` for the one declared scale, `Scene::exact_score_maps` otherwise, over
/// the whole frame with a fresh `RegionSpectra`.
fn exact_maps(
    scene: &Scene,
    template: &PreparedTemplate,
    mask: Option<TemplateMask>,
    scales: &[u32],
    clock: &SearchClock,
) -> Result<ExactMaps, RecognitionError> {
    let mut spectra = RegionSpectra::default();
    if matches!(scales, [SCALE_IDENTITY_MILLI]) {
        scene
            .exact_score_map(
                &template.png,
                None,
                template.metric,
                TemplateMatchTimingStage::Exact,
                mask,
                &mut spectra,
                clock,
            )
            .map(ExactMaps::One)
    } else {
        scene
            .exact_score_maps(
                &template.png,
                None,
                template.metric,
                TemplateMatchTimingStage::Exact,
                mask,
                scales,
                &mut spectra,
                clock,
            )
            .map(ExactMaps::Scaled)
    }
}

/// One timed exact search with the production limit; true when the limit stopped it.
fn timed_exact(
    scene: &Scene,
    template: &PreparedTemplate,
    mask: Option<TemplateMask>,
    scales: &[u32],
) -> Result<(u64, bool), BenchError> {
    let started = Instant::now();
    let result = exact_maps(scene, template, mask, scales, &SearchClock::system());
    let elapsed = run::micros(started.elapsed());
    match result {
        Ok(_) => Ok((elapsed, false)),
        Err(error) if error.timing().is_some() => Ok((elapsed, true)),
        Err(error) => Err(search_error("the exact score map", &error)),
    }
}

/// Every position's exact raw score, row-major per scale in the declared order of the scales,
/// from maps whose clock stands still.
fn exact_raw_scores(
    scene: &Scene,
    template: &PreparedTemplate,
    mask: Option<TemplateMask>,
    scales: &[u32],
    expected: usize,
) -> Result<Vec<f32>, BenchError> {
    let frozen = Instant::now();
    let clock = SearchClock::new(move || frozen);
    let maps = exact_maps(scene, template, mask, scales, &clock)
        .map_err(|error| search_error("the exact score map for the accuracy", &error))?;
    let mut raw = Vec::with_capacity(expected);
    for map in maps.maps() {
        // A threshold of 0 and an acceptance that refuses every position visit every position
        // once, in row-major order.
        map.select(0.0, |found| {
            raw.push(found.raw_score);
            Ok(false)
        })
        .map_err(|error| search_error("reading the exact scores back", &error))?;
    }
    if raw.len() != expected {
        return Err(BenchError::new(
            BenchCode::TemplateSearchFailed,
            format!(
                "the exact maps read back {} positions, the case has {expected}",
                raw.len()
            ),
        ));
    }
    Ok(raw)
}

struct DirectRun {
    us: u64,
    complete: u64,
    incomplete: u64,
    matches: u64,
}

/// Today's direct enumeration on one frame: one `match_template_all` per scale, every position
/// admitted.
fn timed_direct(
    scene: &Scene,
    template: &PreparedTemplate,
    scales: &[u32],
    suppress_iou_milli: u16,
) -> Result<DirectRun, BenchError> {
    let mut outcome = DirectRun {
        us: 0,
        complete: 0,
        incomplete: 0,
        matches: 0,
    };
    let started = Instant::now();
    for factor in scales {
        let scaled = template.scaled.get(factor).ok_or_else(|| {
            BenchError::new(
                BenchCode::WorkloadInvalid,
                format!("scale {factor} milli was not prepared"),
            )
        })?;
        let search = scene
            .match_template_all(
                &scaled.png,
                None,
                template.metric,
                template.spec.threshold,
                suppress_iou_milli,
                |_| Ok(true),
            )
            .map_err(|error| search_error("today's direct enumeration", &error))?;
        match search {
            TemplateSearch::Complete(found) => {
                outcome.complete += 1;
                outcome.matches += found.len() as u64;
            }
            TemplateSearch::Incomplete { found, .. } => {
                outcome.incomplete += 1;
                outcome.matches += found as u64;
            }
        }
    }
    outcome.us = run::micros(started.elapsed());
    Ok(outcome)
}

/// The Conv graph's session of one case and what the CPU assembly needs.
struct ConvSearch {
    session: Session,
    statistics: Vec<TemplateStatistics>,
    positions: Vec<(usize, usize)>,
    metric: MatchMetric,
    masked: bool,
    input_shape: Vec<i64>,
}

struct Prefilter {
    run_us: u64,
    total_us: u64,
    cpu_us: u64,
    /// Every position's float raw score, row-major per scale.
    scores: Vec<f32>,
}

impl ConvSearch {
    /// The input tensor, `session.run`, and the float score of every position.
    fn prefilter(&mut self, frame: &GrayImage) -> Result<Prefilter, BenchError> {
        let cpu_before = win::process_cpu_time_us()?;
        let started = Instant::now();
        let input: Vec<f32> = frame
            .as_raw()
            .iter()
            .map(|value| f32::from(*value))
            .collect();
        let tensor = Tensor::from_array((self.input_shape.clone(), input.into_boxed_slice()))
            .map_err(|error| {
                BenchError::from_ort_error(
                    BenchCode::InferenceFailed,
                    "the Conv input tensor",
                    &error,
                )
            })?;
        let run_started = Instant::now();
        let outputs = self.session.run(ort::inputs![&tensor]).map_err(|error| {
            BenchError::from_ort_error(
                BenchCode::InferenceFailed,
                "the Conv graph run failed",
                &error,
            )
        })?;
        let run_us = run::micros(run_started.elapsed());
        let window = (!self.masked).then(|| WindowSums::new(frame));
        let total: usize = self
            .positions
            .iter()
            .map(|(columns, rows)| columns * rows)
            .sum();
        let mut scores = Vec::with_capacity(total);
        for (index, statistics) in self.statistics.iter().enumerate() {
            let (columns, rows) = self.positions[index];
            let count = columns * rows;
            let products = output_values(
                &outputs,
                &output_name(ConvOutputKind::Products, index),
                count,
            )?;
            let masked = if self.masked {
                Some((
                    output_values(
                        &outputs,
                        &output_name(ConvOutputKind::ImageSums, index),
                        count,
                    )?,
                    output_values(
                        &outputs,
                        &output_name(ConvOutputKind::SquaredSums, index),
                        count,
                    )?,
                ))
            } else {
                None
            };
            template::assemble_scores(
                statistics,
                self.metric,
                ScaleSums { products, masked },
                window.as_ref(),
                (columns, rows),
                &mut scores,
            )
            .map_err(inference_error)?;
        }
        drop(outputs);
        let total_us = run::micros(started.elapsed());
        let cpu_us = win::process_cpu_time_us()?.saturating_sub(cpu_before);
        Ok(Prefilter {
            run_us,
            total_us,
            cpu_us,
            scores,
        })
    }
}

/// The `f32` values of the output `name`, which must hold `count` of them.
fn output_values<'o>(
    outputs: &'o SessionOutputs<'_>,
    name: &str,
    count: usize,
) -> Result<&'o [f32], BenchError> {
    let value = outputs
        .get(name)
        .ok_or_else(|| inference_error(format!("the Conv graph returned no output {name}")))?;
    let (shape, values) = value.try_extract_tensor::<f32>().map_err(|error| {
        BenchError::from_ort_error(
            BenchCode::InferenceFailed,
            &format!("the Conv output {name} is not an f32 tensor"),
            &error,
        )
    })?;
    if values.len() != count {
        return Err(inference_error(format!(
            "the Conv output {name} has shape {:?} ({} values), the case needs {count}",
            &**shape,
            values.len()
        )));
    }
    Ok(values)
}

/// Workflow #407 GPU-0c: the prefix keeps everything else of the error (its ONNX Runtime status
/// and library load).
fn prefixed(label: &str, frame: &str, mut error: BenchError) -> BenchError {
    error.message = format!("{label} frame {frame}: {}", error.message);
    error
}

fn search_error(what: &str, error: &RecognitionError) -> BenchError {
    BenchError::new(BenchCode::TemplateSearchFailed, format!("{what}: {error}"))
}

fn inference_error(message: String) -> BenchError {
    BenchError::new(BenchCode::InferenceFailed, message)
}
