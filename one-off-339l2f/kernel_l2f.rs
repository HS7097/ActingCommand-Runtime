// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #339 L2f evidence: the contained-task loader's own codes
//! (`PreparedContainedTask::load_path`, the path actingd admits a package through), the offline
//! first decisions, and in-process runs on saved frames that record the trace, the timing
//! boundaries, the run progress and the capture times. Every printed line starts with
//! `KERNEL|`; one JSON line per case is also written to the output file for comparisons.

use actingcommand_contract::{ApplicationLifecycleAction, InputAction, PackageRef};
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_execution_kernel::{
    ApplicationEffectSupport, ContainedTaskBoundaryTiming, ContainedTaskRunError,
    ContainedTaskRuntime, ContainedTaskTrace, InputFrameContext, ObservedFrame,
    PreparedContainedTask, simulate_contained_task,
};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

/// Replays saved frames in order and keeps what the kernel did with them.
struct FrameRuntime {
    started: Instant,
    frames: VecDeque<Frame>,
    inputs: u32,
    captures: u32,
    applications: Vec<ApplicationLifecycleAction>,
    supports_application: bool,
    /// Kernel facts and waits without times, for comparisons between builds.
    trace: Vec<String>,
    /// The same with the capture start times, in ms since the run started.
    timeline: Vec<String>,
    boundaries: BTreeMap<String, u32>,
    progress: Vec<u32>,
    last_capture_us: u128,
}

impl ContainedTaskRuntime for FrameRuntime {
    type Error = String;

    fn update_run_progress(&mut self, executed_steps: u32) {
        self.progress.push(executed_steps);
    }

    fn observe_task_boundary(&mut self, timing: ContainedTaskBoundaryTiming) {
        let name = format!("{:?}", timing.boundary);
        *self.boundaries.entry(name.clone()).or_default() += 1;
        if name.ends_with("Wait") {
            self.trace.push(format!("wait {name}"));
            self.timeline.push(format!(
                "wait {name} {}ms",
                timing.ended.saturating_duration_since(timing.started).as_millis()
            ));
        }
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        self.last_capture_us = self.started.elapsed().as_micros();
        self.trace.push("capture".to_owned());
        self.timeline
            .push(format!("capture #{} at {}us", self.captures, self.last_capture_us));
        self.frames
            .pop_front()
            .map(ObservedFrame::from)
            .ok_or_else(|| "frames exhausted".to_owned())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        self.trace.push("input".to_owned());
        self.timeline.push("input".to_owned());
        Ok(())
    }

    fn supports_application_effect(&self) -> bool {
        self.supports_application
    }

    fn control_application(
        &mut self,
        action: ApplicationLifecycleAction,
    ) -> Result<ApplicationEffectSupport, Self::Error> {
        self.applications.push(action);
        self.trace.push(format!("control_application {action:?}"));
        self.timeline.push(format!("control_application {action:?}"));
        Ok(ApplicationEffectSupport::Performed)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let line = match &trace {
            ContainedTaskTrace::PackageAdmitted { .. } => "package_admitted".to_owned(),
            ContainedTaskTrace::RunStarted => "run_started".to_owned(),
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                ..
            } => format!("step_started {step_index} {operation_label} from={from_page}"),
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => format!("step_finished {step_index} {operation_label} {page_label}"),
            ContainedTaskTrace::RecognitionStarted {
                candidate_pages, ..
            } => format!("recognition_started {candidate_pages:?}"),
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                targets,
                ..
            } => format!(
                "recognition {candidate_pages:?} -> {page_label:?} targets={}",
                targets
                    .iter()
                    .map(|target| target.target_id.clone())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                ..
            } => format!("effect_intent {step_index} {operation_label}"),
            ContainedTaskTrace::EffectCompleted {
                step_index,
                operation_label,
            } => format!("effect_completed {step_index} {operation_label}"),
            ContainedTaskTrace::Finalizing { outcome } => format!("finalizing {outcome:?}"),
            _ => return Ok(()),
        };
        if line.starts_with("recognition ") {
            self.timeline
                .push(format!("{line} (capture at {}us)", self.last_capture_us));
        } else {
            self.timeline.push(line.clone());
        }
        self.trace.push(line);
        Ok(())
    }
}

fn read_frame(path: &str) -> Frame {
    Frame::from_png(
        std::fs::read(path).expect("frame PNG"),
        CaptureBackendName::FixtureSimulation,
    )
    .expect("frame")
}

fn load(
    case: &serde_json::Value,
) -> Result<PreparedContainedTask, actingcommand_execution_kernel::ContainedTaskError> {
    let reference = PackageRef::parse_argument(case["reference"].as_str().expect("reference"))
        .expect("package reference");
    PreparedContainedTask::load_path(
        "oneoff-339-l2f",
        Path::new(case["locator"].as_str().expect("locator")),
        &reference,
        None,
        Instant::now() + Duration::from_secs(60),
    )
}

fn main() {
    let cases_path = std::env::args().nth(1).expect("cases.json path");
    let out_path = std::env::args().nth(2).expect("output JSON lines path");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cases_path).expect("read cases"))
            .expect("cases JSON");
    let mut out = std::fs::File::create(&out_path).expect("create output");
    let mut failures = 0_usize;
    for case in cases["admission"].as_array().into_iter().flatten() {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let result = load(case);
        let (code, detail, issue) = match &result {
            Ok(task) => (
                "ok".to_owned(),
                format!(
                    "package={} task={} mode={} maximum_executed_steps={}",
                    task.package_label(),
                    task.task_label(),
                    task.execution_mode(),
                    task.maximum_executed_steps(),
                ),
                serde_json::Value::Null,
            ),
            Err(error) => (
                error.code().to_owned(),
                error.detail().unwrap_or("").replace('\n', " "),
                error
                    .declaration_issue()
                    .map(|issue| serde_json::to_value(issue).expect("issue JSON"))
                    .unwrap_or(serde_json::Value::Null),
            ),
        };
        let issue_text = if issue.is_null() {
            String::new()
        } else {
            issue.to_string()
        };
        let pass = expect == code
            && expect_detail
                .split(" && ")
                .all(|part| detail.contains(part) || issue_text.contains(part));
        failures += usize::from(!pass);
        println!(
            "KERNEL|load_path|{name}|expect={expect} {expect_detail}|code={code}|{}|detail={detail}|declaration_issue={issue_text}",
            if pass { "PASS" } else { "FAIL" }
        );
        writeln!(
            out,
            "{}",
            serde_json::json!({"kind": "admission", "name": name, "code": code, "detail": detail, "issue": issue})
        )
        .expect("write output");
        if let Ok(task) = &result {
            for frames in case["simulate"].as_array().into_iter().flatten() {
                let frames: Vec<Frame> = frames
                    .as_array()
                    .expect("frame list")
                    .iter()
                    .map(|frame| read_frame(frame.as_str().expect("frame path")))
                    .collect();
                let count = frames.len();
                match simulate_contained_task(task, frames) {
                    Ok(simulation) => {
                        let decision =
                            serde_json::to_string(&simulation.decision).expect("decision JSON");
                        let wanted = case["expect_decision"].as_str().unwrap_or("would_click");
                        let pass = decision.contains(wanted);
                        failures += usize::from(!pass);
                        println!(
                            "KERNEL|offline_first_decision|{name}|frames={count}|{}|expect={wanted}|decision={decision}|capture_count={}|recognition={}",
                            if pass { "PASS" } else { "FAIL" },
                            simulation.capture_count,
                            serde_json::to_string(&simulation.recognition)
                                .expect("recognition JSON")
                        );
                        writeln!(
                            out,
                            "{}",
                            serde_json::json!({"kind": "offline", "name": name, "decision": decision, "capture_count": simulation.capture_count})
                        )
                        .expect("write output");
                    }
                    Err(error) => {
                        failures += 1;
                        println!("KERNEL|offline_first_decision|{name}|frames={count}|FAIL|{error}");
                    }
                }
            }
        }
    }
    for case in cases["runs"].as_array().into_iter().flatten() {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let task = match load(case) {
            Ok(task) => task,
            Err(error) => {
                failures += 1;
                println!(
                    "KERNEL|run|{name}|FAIL|not admitted: {} {}",
                    error.code(),
                    error.detail().unwrap_or("")
                );
                continue;
            }
        };
        let mut runtime = FrameRuntime {
            started: Instant::now(),
            frames: case["frames"]
                .as_array()
                .expect("frames")
                .iter()
                .map(|frame| read_frame(frame.as_str().expect("frame path")))
                .collect(),
            inputs: 0,
            captures: 0,
            applications: Vec::new(),
            supports_application: case["supports_application"].as_bool().unwrap_or(true),
            trace: Vec::new(),
            timeline: Vec::new(),
            boundaries: BTreeMap::new(),
            progress: Vec::new(),
            last_capture_us: 0,
        };
        let frames_given = runtime.frames.len();
        let started = Instant::now();
        let (code, detail, timing) = match task.run(&mut runtime) {
            Ok(outcome) => (
                "success".to_owned(),
                format!(
                    "final_page={:?} executed_steps={}",
                    outcome.final_page, outcome.executed_steps
                ),
                String::new(),
            ),
            Err(ContainedTaskRunError::Task(error)) => (
                error.code().to_owned(),
                error.detail().unwrap_or("").to_owned(),
                format!("{:?}", error.timing()),
            ),
            Err(other) => ("operation_error".to_owned(), format!("{other:?}"), String::new()),
        };
        let mut pass = code == expect && detail.contains(expect_detail);
        if let Some(inputs) = case["expect_inputs"].as_u64() {
            pass &= u64::from(runtime.inputs) == inputs;
        }
        if let Some(suffix) = case["expect_detail_suffix"].as_str() {
            pass &= detail.ends_with(suffix);
        }
        failures += usize::from(!pass);
        println!(
            "KERNEL|run|{name}|{}|expect={expect} {expect_detail} inputs={}|code={code}|inputs={}|applications={:?}|captures={} of {frames_given}|seconds={:.2}|progress={:?}|boundaries={:?}|detail={detail}|timing={timing}",
            if pass { "PASS" } else { "FAIL" },
            case["expect_inputs"],
            runtime.inputs,
            runtime.applications,
            runtime.captures,
            started.elapsed().as_secs_f64(),
            runtime.progress,
            runtime.boundaries,
        );
        for line in &runtime.timeline {
            println!("KERNEL|run timeline|{name}|{line}");
        }
        writeln!(
            out,
            "{}",
            serde_json::json!({
                "kind": "run", "name": name, "code": code, "detail": detail, "timing": timing,
                "inputs": runtime.inputs, "captures": runtime.captures, "frames_given": frames_given,
                "progress": runtime.progress, "boundaries": runtime.boundaries,
                "trace": runtime.trace, "timeline": runtime.timeline,
            })
        )
        .expect("write output");
    }
    println!("KERNEL|RESULT|failures={failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
