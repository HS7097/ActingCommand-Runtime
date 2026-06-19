// SPDX-License-Identifier: AGPL-3.0-only

use super::{load_evaluator_and_detector, page_error, task_error};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, InputBackend, MaaTouchBackend,
    MaaTouchValidationConfig, ScreencapBackend, combine_operation_and_close,
};
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{PackRect, RecognitionEvaluator};
use actingcommand_task_loop::{
    ProbeDecisionLoop, ProbeReferenceOverrides, ProbeStepDecision, load_probe_plan_from_json_str,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLICK_SPACE_WIDTH: i32 = 1280;
const CLICK_SPACE_HEIGHT: i32 = 720;
const DEFAULT_CLICK_RECT_RADIUS: i32 = 20;
const DEFAULT_FORBIDDEN_RADIUS: i32 = 20;
const DEFAULT_EXPECT_TIMEOUT_MS: u64 = 3000;
const DEFAULT_EXPECT_INTERVAL_MS: u64 = 100;
const MAX_POLL_FRAMES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRunOptions {
    pub pack: PathBuf,
    pub pack_root: PathBuf,
    pub pages: PathBuf,
    pub probe: PathBuf,
    pub run_root: PathBuf,
    pub navigation: Option<PathBuf>,
    pub capture: bool,
    pub scene: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActualClickPoint {
    seed: u64,
    algorithm: &'static str,
    rect: PackRect,
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationFile {
    coordinate_space: Option<NavigationCoordinateSpace>,
    #[serde(default)]
    control_points: HashMap<String, NavigationControlPoint>,
    #[serde(default)]
    navigation: Vec<NavigationRoute>,
    #[serde(default)]
    forbidden_destructive_points: Vec<ForbiddenDestructivePoint>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct NavigationCoordinateSpace {
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationControlPoint {
    point: [i32; 2],
}

#[derive(Debug, Clone, Deserialize)]
struct NavigationRoute {
    id: String,
    click_point: [i32; 2],
    arrive_anchor: Option<ArrivalAnchor>,
}

#[derive(Debug, Clone, Deserialize)]
struct ArrivalAnchor {
    template: String,
    #[serde(default)]
    threshold: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
struct ForbiddenDestructivePoint {
    id: String,
    #[serde(default)]
    point: Option<[i32; 2]>,
    #[serde(default)]
    rect: Option<PackRect>,
    #[serde(default)]
    radius: Option<i32>,
}

#[derive(Debug)]
struct NavigationBridge {
    overrides: ProbeReferenceOverrides,
    arrival_anchors: HashMap<String, ArrivalAnchor>,
    forbidden: Vec<ForbiddenDestructivePoint>,
}

struct OperationJournal {
    run_id: String,
    run_dir: PathBuf,
    events: PathBuf,
    frames: PathBuf,
    observations: PathBuf,
    pack_root: PathBuf,
}

#[derive(Default)]
struct ProbeRunState {
    executed: bool,
    click_count: usize,
    final_page: Option<String>,
    frames: usize,
    observations: usize,
}

pub fn run_probe_command(
    config: MaaTouchValidationConfig,
    options: &ProbeRunOptions,
) -> DeviceResult<String> {
    if !options.capture {
        return Err(DeviceError::fatal("probe-run requires --capture"));
    }
    if options.scene.is_some() {
        return Err(DeviceError::fatal(
            "probe-run does not support --scene for click execution",
        ));
    }

    let probe_json = fs::read_to_string(&options.probe).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read probe plan {}: {err}",
            options.probe.display()
        ))
    })?;
    let journal = OperationJournal::create(options, &probe_json)?;
    let mut state = ProbeRunState::default();
    let result = execute_probe_run(config, options, &probe_json, &journal, &mut state);
    match result {
        Ok(()) => {
            journal.event("run_finished", json!({"result": "completed"}))?;
            journal.summary(options, "completed", &state, None)?;
            Ok(format!(
                "run_id={}\nrun_dir={}\nprobe={}\nresult=completed\nexecuted={}\nclick_count={}\nsummary={}\nevents={}\n",
                journal.run_id,
                journal.run_dir.display(),
                options.probe.display(),
                state.executed,
                state.click_count,
                journal.run_dir.join("summary.json").display(),
                journal.events.display()
            ))
        }
        Err(err) => {
            let _ = journal.event(
                "run_failed",
                json!({"result": "failed", "error": err.to_string()}),
            );
            let _ = journal.summary(options, "failed", &state, Some(err.to_string()));
            Err(err)
        }
    }
}

fn execute_probe_run(
    config: MaaTouchValidationConfig,
    options: &ProbeRunOptions,
    probe_json: &str,
    journal: &OperationJournal,
    state: &mut ProbeRunState,
) -> DeviceResult<()> {
    journal.event(
        "run_started",
        json!({
            "run_id": journal.run_id,
            "destructive_allowed": false,
            "command": env::args().collect::<Vec<_>>().join(" ")
        }),
    )?;
    journal.record_inputs(options)?;

    let (evaluator, detector) =
        load_evaluator_and_detector(&options.pack, &options.pack_root, &options.pages)?;
    journal.event("pack_loaded", json!({"path": options.pack}))?;
    journal.event("pages_loaded", json!({"path": options.pages}))?;

    let probe_plan = load_probe_plan_from_json_str(probe_json).map_err(task_error)?;
    let probe_loop = ProbeDecisionLoop::new(probe_plan).map_err(task_error)?;
    journal.event(
        "probe_plan_loaded",
        json!({"id": probe_loop.plan().id, "steps": probe_loop.plan().steps.len()}),
    )?;

    let navigation = match &options.navigation {
        Some(path) => Some(load_navigation_bridge(path)?),
        None => None,
    };
    let overrides = navigation
        .as_ref()
        .map(|bridge| &bridge.overrides)
        .cloned()
        .unwrap_or_default();
    probe_loop
        .validate_with_overrides(&detector, &evaluator, &overrides)
        .map_err(task_error)?;
    journal.event("validate_done", json!({"status": "passed"}))?;

    let mut capture = ScreencapBackend::new(config.adb.clone(), config.target.clone());
    let before = capture_frame(&mut capture, journal, "000_before.png", "before")?;
    state.frames += 1;
    let mut scene = Scene::from_png(&before).map_err(|err| DeviceError::fatal(err.to_string()))?;
    let seed_base = run_seed();
    let mut backend = None::<MaaTouchBackend>;

    for (step_index, step) in probe_loop.plan().steps.iter().enumerate() {
        journal.event(
            "step_started",
            json!({"step_id": step.id, "index": step_index}),
        )?;
        let decision = probe_loop
            .decide_step_with_known_page(
                step,
                &detector,
                &evaluator,
                &scene,
                &overrides,
                state.final_page.as_deref(),
            )
            .map_err(task_error)?;

        match decision {
            ProbeStepDecision::SkippedPageGuard {
                page_id,
                evaluation,
                ..
            } => {
                journal.event(
                    "click_skipped",
                    json!({
                        "step_id": step.id,
                        "reason": "page_guard_not_matched",
                        "page_id": page_id,
                        "message": evaluation.message
                    }),
                )?;
            }
            ProbeStepDecision::SkippedExternalPageGuard {
                page_id,
                current_page_id,
                ..
            } => {
                journal.event(
                    "click_skipped",
                    json!({
                        "step_id": step.id,
                        "reason": "external_page_guard_not_matched",
                        "page_id": page_id,
                        "current_page_id": current_page_id
                    }),
                )?;
            }
            ProbeStepDecision::DetectPage {
                page_id,
                evaluation,
                ..
            }
            | ProbeStepDecision::ObservePage {
                page_id,
                evaluation,
                ..
            } => {
                state.observations += 1;
                journal.event(
                    "observe_done",
                    json!({
                        "step_id": step.id,
                        "page_id": page_id,
                        "matched": evaluation.matched,
                        "message": evaluation.message
                    }),
                )?;
            }
            ProbeStepDecision::ObserveTargets { evaluations, .. } => {
                state.observations += evaluations.len();
                journal.event(
                    "observe_done",
                    json!({
                        "step_id": step.id,
                        "targets": evaluations.iter().map(|evaluation| {
                            json!({
                                "id": evaluation.id,
                                "passed": evaluation.passed,
                                "message": evaluation.message
                            })
                        }).collect::<Vec<_>>()
                    }),
                )?;
            }
            ProbeStepDecision::Click {
                target_id,
                click,
                expect_after,
                ..
            } => {
                if state.click_count >= probe_loop.max_navigation_clicks() {
                    return Err(DeviceError::fatal(
                        "probe-run navigation click limit exceeded",
                    ));
                }
                journal.event(
                    "safety_check_started",
                    json!({"step_id": step.id, "target_id": target_id}),
                )?;
                validate_rect_in_click_space(click)?;
                let actual = actual_click_point(click, seed_base ^ hash_text(&step.id));
                validate_point_in_click_space(actual.x, actual.y)?;
                if let Some(bridge) = &navigation {
                    bridge.validate_not_forbidden(actual.x, actual.y)?;
                }
                journal.event(
                    "safety_check_done",
                    json!({
                        "step_id": step.id,
                        "target_id": target_id,
                        "actual_click_point": actual_click_json(actual)
                    }),
                )?;

                let backend_ref = ensure_maatouch_backend(&mut backend, &config, journal)?;
                journal.event(
                    "click_started",
                    json!({"step_id": step.id, "target_id": target_id}),
                )?;
                if let Err(err) = backend_ref.tap(actual.x, actual.y) {
                    return close_backend_after_error(&mut backend, err);
                }
                state.executed = true;
                state.click_count += 1;
                journal.event(
                    "click_done",
                    json!({
                        "step_id": step.id,
                        "target_id": target_id,
                        "actual_click_point": actual_click_json(actual)
                    }),
                )?;

                let poll_context = ArrivalPollContext {
                    journal,
                    detector: &detector,
                    evaluator: &evaluator,
                    navigation: navigation.as_ref(),
                    page_id: &expect_after.page_id,
                    step_id: &step.id,
                };
                let poll_timing = ArrivalPollTiming {
                    timeout_ms: expect_after.timeout_ms.unwrap_or(DEFAULT_EXPECT_TIMEOUT_MS),
                    interval_ms: expect_after
                        .interval_ms
                        .unwrap_or(DEFAULT_EXPECT_INTERVAL_MS),
                };
                let arrived = match poll_arrival(&mut capture, poll_context, poll_timing) {
                    Ok(arrived) => arrived,
                    Err(err) => return close_backend_after_error(&mut backend, err),
                };
                scene = arrived.scene;
                state.frames += arrived.frames;
                state.final_page = Some(expect_after.page_id);
            }
        }
        journal.event("step_finished", json!({"step_id": step.id}))?;
    }

    if let Some(mut backend) = backend {
        let operation = Ok(());
        let close = backend.close();
        combine_operation_and_close(operation, close)?;
    }

    Ok(())
}

fn close_backend_after_error<T>(
    backend: &mut Option<MaaTouchBackend>,
    err: DeviceError,
) -> DeviceResult<T> {
    if let Some(mut backend) = backend.take() {
        combine_operation_and_close(Err(err), backend.close())?;
        unreachable!("combine_operation_and_close returned Ok for an operation error");
    }
    Err(err)
}

fn ensure_maatouch_backend<'a>(
    backend: &'a mut Option<MaaTouchBackend>,
    config: &MaaTouchValidationConfig,
    journal: &OperationJournal,
) -> DeviceResult<&'a mut MaaTouchBackend> {
    if backend.is_none() {
        let mut created = MaaTouchBackend::new(
            config.adb.clone(),
            config.target.clone(),
            config.maatouch.clone(),
        );
        let device = created.connect()?;
        journal.event(
            "maatouch_connected",
            json!({"serial": device.serial, "state": device.state, "screen_size": device.screen_size}),
        )?;
        *backend = Some(created);
    }
    backend
        .as_mut()
        .ok_or_else(|| DeviceError::fatal("failed to initialize MaaTouch backend"))
}

struct ArrivalResult {
    scene: Scene,
    frames: usize,
}

struct ArrivalPollContext<'a> {
    journal: &'a OperationJournal,
    detector: &'a PageDetector,
    evaluator: &'a RecognitionEvaluator,
    navigation: Option<&'a NavigationBridge>,
    page_id: &'a str,
    step_id: &'a str,
}

struct ArrivalPollTiming {
    timeout_ms: u64,
    interval_ms: u64,
}

fn poll_arrival(
    capture: &mut ScreencapBackend,
    context: ArrivalPollContext<'_>,
    timing: ArrivalPollTiming,
) -> DeviceResult<ArrivalResult> {
    let timeout = Duration::from_millis(timing.timeout_ms);
    let interval = Duration::from_millis(timing.interval_ms.max(1));
    let started = Instant::now();
    let mut frames = 0;
    let max_frames =
        ((timing.timeout_ms / timing.interval_ms.max(1)) as usize).clamp(1, MAX_POLL_FRAMES);

    loop {
        let frame_name = format!(
            "{:03}_after_click_{}.png",
            frames + 1,
            safe_file_part(context.step_id)
        );
        let png = capture_frame(capture, context.journal, &frame_name, "poll")?;
        frames += 1;
        let scene = Scene::from_png(&png).map_err(|err| DeviceError::fatal(err.to_string()))?;
        let matched = match context
            .navigation
            .and_then(|bridge| bridge.arrival_anchors.get(context.page_id))
        {
            Some(anchor) => match_arrival_anchor(&scene, anchor, context.journal)?,
            None => {
                context
                    .detector
                    .evaluate_page(context.evaluator, &scene, context.page_id)
                    .map_err(page_error)?
                    .matched
            }
        };
        context.journal.event(
            "poll_iter",
            json!({
                "step_id": context.step_id,
                "page_id": context.page_id,
                "frame": frame_name,
                "matched": matched,
                "elapsed_ms": started.elapsed().as_millis()
            }),
        )?;
        if matched {
            let arrived_name = format!(
                "{}{:03}_arrived_{}.png",
                "",
                frames,
                safe_file_part(context.page_id)
            );
            fs::write(context.journal.frames.join(&arrived_name), &png).map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to write arrived frame {}: {err}",
                    arrived_name
                ))
            })?;
            context.journal.event(
                "arrived",
                json!({"step_id": context.step_id, "page_id": context.page_id, "frame": arrived_name}),
            )?;
            return Ok(ArrivalResult { scene, frames });
        }
        if frames >= max_frames || started.elapsed() >= timeout {
            return Err(DeviceError::fatal(format!(
                "timed out waiting for expected page '{}' after {frames} frames",
                context.page_id
            )));
        }
        thread::sleep(interval);
    }
}

fn match_arrival_anchor(
    scene: &Scene,
    anchor: &ArrivalAnchor,
    journal: &OperationJournal,
) -> DeviceResult<bool> {
    let template_path = journal.pack_root().join(&anchor.template);
    let template = fs::read(&template_path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read arrival anchor template {}: {err}",
            template_path.display()
        ))
    })?;
    let matched = scene
        .match_template(&template, None)
        .map_err(|err| DeviceError::fatal(err.to_string()))?;
    Ok(matched.score >= anchor.threshold.unwrap_or(0.9))
}

fn capture_frame(
    capture: &mut ScreencapBackend,
    journal: &OperationJournal,
    name: &str,
    label: &str,
) -> DeviceResult<Vec<u8>> {
    journal.event("capture_started", json!({"label": label, "frame": name}))?;
    let started = Instant::now();
    let frame = capture.capture()?;
    fs::write(journal.frames.join(name), &frame.png)
        .map_err(|err| DeviceError::fatal(format!("failed to write frame {}: {err}", name)))?;
    journal.event(
        "capture_done",
        json!({
            "label": label,
            "frame": name,
            "width": frame.width,
            "height": frame.height,
            "elapsed_ms": started.elapsed().as_millis()
        }),
    )?;
    Ok(frame.png)
}

fn load_navigation_bridge(path: &Path) -> DeviceResult<NavigationBridge> {
    let json = fs::read_to_string(path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read navigation file {}: {err}",
            path.display()
        ))
    })?;
    let navigation: NavigationFile = serde_json::from_str(&json).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to parse navigation file {}: {err}",
            path.display()
        ))
    })?;
    if let Some(space) = navigation.coordinate_space
        && (space.width != CLICK_SPACE_WIDTH || space.height != CLICK_SPACE_HEIGHT)
    {
        return Err(DeviceError::fatal(format!(
            "navigation coordinate_space {}x{} does not match probe click space {}x{}",
            space.width, space.height, CLICK_SPACE_WIDTH, CLICK_SPACE_HEIGHT
        )));
    }

    let mut overrides = ProbeReferenceOverrides::new();
    let mut arrival_anchors = HashMap::new();
    for route in &navigation.navigation {
        let target_id = navigation_target_id(&route.id);
        overrides.insert_click_target(&target_id, point_rect(route.click_point));
        if let Some(anchor) = &route.arrive_anchor {
            let page_id = navigation_arrival_page_id(&route.id);
            overrides.insert_page(&page_id);
            arrival_anchors.insert(page_id, anchor.clone());
        }
    }
    for (name, point) in &navigation.control_points {
        overrides.insert_click_target(control_target_id(name), point_rect(point.point));
    }

    Ok(NavigationBridge {
        overrides,
        arrival_anchors,
        forbidden: navigation.forbidden_destructive_points,
    })
}

impl NavigationBridge {
    fn validate_not_forbidden(&self, x: i32, y: i32) -> DeviceResult<()> {
        for forbidden in &self.forbidden {
            let in_rect = forbidden.rect.is_some_and(|rect| rect_contains(rect, x, y));
            let in_radius = forbidden.point.is_some_and(|point| {
                point_in_radius(
                    x,
                    y,
                    point[0],
                    point[1],
                    forbidden.radius.unwrap_or(DEFAULT_FORBIDDEN_RADIUS),
                )
            });
            if in_rect || in_radius {
                return Err(DeviceError::fatal(format!(
                    "actual click point {x},{y} falls inside forbidden destructive point '{}'",
                    forbidden.id
                )));
            }
        }
        Ok(())
    }
}

impl OperationJournal {
    fn create(options: &ProbeRunOptions, probe_json: &str) -> DeviceResult<Self> {
        let run_id = format!("probe-{}", run_timestamp());
        let run_dir = options.run_root.join(&run_id);
        let frames = run_dir.join("frames");
        let observations = run_dir.join("observations");
        fs::create_dir_all(&frames).map_err(|err| {
            DeviceError::fatal(format!(
                "failed to create probe run directory {}: {err}",
                frames.display()
            ))
        })?;
        fs::create_dir_all(&observations).map_err(|err| {
            DeviceError::fatal(format!(
                "failed to create observations directory {}: {err}",
                observations.display()
            ))
        })?;
        fs::write(
            run_dir.join("command.txt"),
            env::args().collect::<Vec<_>>().join(" "),
        )
        .map_err(|err| DeviceError::fatal(format!("failed to write command.txt: {err}")))?;
        fs::write(run_dir.join("probe-plan.json"), probe_json)
            .map_err(|err| DeviceError::fatal(format!("failed to write probe-plan.json: {err}")))?;
        Ok(Self {
            run_id,
            events: run_dir.join("events.jsonl"),
            run_dir,
            frames,
            observations,
            pack_root: options.pack_root.clone(),
        })
    }

    fn event(&self, event: &str, payload: serde_json::Value) -> DeviceResult<()> {
        let value = json!({
            "timestamp_ms": now_millis(),
            "event": event,
            "payload": payload,
        });
        let line = serde_json::to_string(&value).map_err(|err| {
            DeviceError::fatal(format!("failed to serialize journal event: {err}"))
        })?;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events)
            .and_then(|mut file| {
                use std::io::Write;
                writeln!(file, "{line}")
            })
            .map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to append journal event {}: {err}",
                    self.events.display()
                ))
            })
    }

    fn record_inputs(&self, options: &ProbeRunOptions) -> DeviceResult<()> {
        let inputs = json!({
            "pack": file_entry(&options.pack)?,
            "pages": file_entry(&options.pages)?,
            "probe": file_entry(&options.probe)?,
            "navigation": match &options.navigation {
                Some(path) => Some(file_entry(path)?),
                None => None,
            },
            "pack_root": options.pack_root,
        });
        fs::write(
            self.run_dir.join("input-paths.json"),
            serde_json::to_vec_pretty(&inputs).map_err(|err| {
                DeviceError::fatal(format!("failed to serialize input-paths.json: {err}"))
            })?,
        )
        .map_err(|err| DeviceError::fatal(format!("failed to write input-paths.json: {err}")))
    }

    fn summary(
        &self,
        options: &ProbeRunOptions,
        result: &str,
        state: &ProbeRunState,
        error: Option<String>,
    ) -> DeviceResult<()> {
        let summary = json!({
            "run_id": self.run_id,
            "result": result,
            "executed": state.executed,
            "click_count": state.click_count,
            "destructive_allowed": false,
            "final_page": state.final_page,
            "frames": state.frames,
            "observations": state.observations,
            "paths": {
                "pack": options.pack,
                "pack_root": options.pack_root,
                "pages": options.pages,
                "probe": options.probe,
                "navigation": options.navigation,
                "run_dir": self.run_dir,
                "frames": self.frames,
                "observations": self.observations
            },
            "error": error,
        });
        fs::write(
            self.run_dir.join("summary.json"),
            serde_json::to_vec_pretty(&summary).map_err(|err| {
                DeviceError::fatal(format!("failed to serialize summary.json: {err}"))
            })?,
        )
        .map_err(|err| DeviceError::fatal(format!("failed to write summary.json: {err}")))
    }

    fn pack_root(&self) -> &Path {
        &self.pack_root
    }
}

fn actual_click_point(rect: PackRect, seed: u64) -> ActualClickPoint {
    let mut state = if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    };
    let x_offset = next_u64(&mut state) % rect.width as u64;
    let y_offset = next_u64(&mut state) % rect.height as u64;
    ActualClickPoint {
        seed,
        algorithm: "xorshift64_uniform_rect_v1",
        rect,
        x: rect.x + x_offset as i32,
        y: rect.y + y_offset as i32,
    }
}

fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn actual_click_json(actual: ActualClickPoint) -> serde_json::Value {
    json!({
        "seed": actual.seed,
        "algorithm": actual.algorithm,
        "rect": rect_json(actual.rect),
        "point": {"x": actual.x, "y": actual.y}
    })
}

fn rect_json(rect: PackRect) -> serde_json::Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn point_rect(point: [i32; 2]) -> PackRect {
    let x = (point[0] - DEFAULT_CLICK_RECT_RADIUS).max(0);
    let y = (point[1] - DEFAULT_CLICK_RECT_RADIUS).max(0);
    let right = (point[0] + DEFAULT_CLICK_RECT_RADIUS).min(CLICK_SPACE_WIDTH - 1);
    let bottom = (point[1] + DEFAULT_CLICK_RECT_RADIUS).min(CLICK_SPACE_HEIGHT - 1);
    PackRect {
        x,
        y,
        width: right - x + 1,
        height: bottom - y + 1,
    }
}

fn validate_rect_in_click_space(rect: PackRect) -> DeviceResult<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(DeviceError::fatal(format!(
            "click rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    validate_point_in_click_space(rect.x, rect.y)?;
    validate_point_in_click_space(rect.x + rect.width - 1, rect.y + rect.height - 1)
}

fn validate_point_in_click_space(x: i32, y: i32) -> DeviceResult<()> {
    if !(0..CLICK_SPACE_WIDTH).contains(&x) || !(0..CLICK_SPACE_HEIGHT).contains(&y) {
        return Err(DeviceError::fatal(format!(
            "click point {x},{y} is outside {}x{} click space",
            CLICK_SPACE_WIDTH, CLICK_SPACE_HEIGHT
        )));
    }
    Ok(())
}

fn rect_contains(rect: PackRect, x: i32, y: i32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

fn point_in_radius(x: i32, y: i32, cx: i32, cy: i32, radius: i32) -> bool {
    let dx = i64::from(x - cx);
    let dy = i64::from(y - cy);
    let radius = i64::from(radius.max(0));
    dx * dx + dy * dy <= radius * radius
}

fn navigation_target_id(id: &str) -> String {
    format!("navigation/{id}")
}

fn control_target_id(id: &str) -> String {
    format!("control/{id}")
}

fn navigation_arrival_page_id(id: &str) -> String {
    format!("navigation/{id}/arrive_anchor")
}

fn safe_file_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn file_entry(path: &Path) -> DeviceResult<serde_json::Value> {
    Ok(json!({
        "path": path,
        "fnv1a64": format!("{:016x}", hash_file(path)?),
    }))
}

fn hash_file(path: &Path) -> DeviceResult<u64> {
    let bytes = fs::read(path).map_err(|err| {
        DeviceError::fatal(format!(
            "failed to read {} for hashing: {err}",
            path.display()
        ))
    })?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    Ok(hash)
}

fn hash_text(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn run_seed() -> u64 {
    now_millis() ^ 0x517c_c1b7_2722_0a95
}

fn run_timestamp() -> String {
    now_millis().to_string()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_click_point_records_seed_algorithm_rect_and_point() {
        let rect = PackRect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        let point = actual_click_point(rect, 12345);

        assert_eq!(point.seed, 12345);
        assert_eq!(point.algorithm, "xorshift64_uniform_rect_v1");
        assert_eq!(point.rect, rect);
        assert!(rect_contains(rect, point.x, point.y));
    }

    #[test]
    fn point_rect_wraps_point_with_radius() {
        assert_eq!(
            point_rect([66, 237]),
            PackRect {
                x: 46,
                y: 217,
                width: 41,
                height: 41
            }
        );
    }

    #[test]
    fn forbidden_point_uses_radius_not_exact_equality() {
        let bridge = NavigationBridge {
            overrides: ProbeReferenceOverrides::new(),
            arrival_anchors: HashMap::new(),
            forbidden: vec![ForbiddenDestructivePoint {
                id: "collect".to_string(),
                point: Some([100, 100]),
                rect: None,
                radius: Some(10),
            }],
        };

        assert!(bridge.validate_not_forbidden(106, 100).is_err());
        assert!(bridge.validate_not_forbidden(111, 100).is_ok());
    }

    #[test]
    fn forbidden_rect_rejects_inside_point() {
        let bridge = NavigationBridge {
            overrides: ProbeReferenceOverrides::new(),
            arrival_anchors: HashMap::new(),
            forbidden: vec![ForbiddenDestructivePoint {
                id: "rect".to_string(),
                point: None,
                rect: Some(PackRect {
                    x: 40,
                    y: 50,
                    width: 10,
                    height: 20,
                }),
                radius: None,
            }],
        };

        assert!(bridge.validate_not_forbidden(49, 69).is_err());
        assert!(bridge.validate_not_forbidden(50, 69).is_ok());
    }

    #[test]
    fn navigation_bridge_adds_navigation_targets_and_arrival_pages() {
        let dir =
            std::env::temp_dir().join(format!("actingcommand-probe-nav-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("navigation.json");
        fs::write(
            &path,
            r#"{
                "schema_version": "0.1",
                "coordinate_space": {"width": 1280, "height": 720},
                "control_points": {"home": {"point": [1236, 25]}},
                "navigation": [
                  {
                    "id": "home_to_task",
                    "click_point": [66, 237],
                    "arrive_anchor": {"template": "page.png", "threshold": 0.9}
                  }
                ],
                "forbidden_destructive_points": [
                  {"id": "collect", "point": [1150, 671], "radius": 30}
                ]
            }"#,
        )
        .expect("nav");

        let bridge = load_navigation_bridge(&path).expect("bridge");
        assert!(
            bridge
                .overrides
                .click_target("navigation/home_to_task")
                .is_some()
        );
        assert!(bridge.overrides.click_target("control/home").is_some());
        assert!(
            bridge
                .overrides
                .contains_page("navigation/home_to_task/arrive_anchor")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
