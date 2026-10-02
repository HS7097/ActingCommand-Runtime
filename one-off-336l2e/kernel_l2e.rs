// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2e evidence: the contained-task loader's own codes
//! for the application packages (`PreparedContainedTask::load_path`, the path actingd admits a
//! package through), the offline first decisions, and in-process runs on saved frames with a
//! runtime that drives (or fails) the application effect. Every printed line starts with
//! `KERNEL|`.

use actingcommand_contract::{ApplicationLifecycleAction, InputAction, PackageRef};
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_execution_kernel::{
    ApplicationEffectSupport, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTrace,
    InputFrameContext, ObservedFrame, PreparedContainedTask, simulate_contained_task,
};
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

/// Replays saved frames in order, counts inputs and application calls, and keeps the trace.
struct FrameRuntime {
    frames: VecDeque<Frame>,
    inputs: u32,
    captures: u32,
    applications: Vec<ApplicationLifecycleAction>,
    supports_application: bool,
    fail_application: bool,
    trace: Vec<String>,
}

impl ContainedTaskRuntime for FrameRuntime {
    type Error = String;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.captures += 1;
        self.frames
            .pop_front()
            .map(ObservedFrame::from)
            .ok_or_else(|| "frames exhausted".to_owned())
    }

    fn input(&mut self, _action: InputAction, _frame: Option<InputFrameContext>) -> Result<(), Self::Error> {
        self.inputs += 1;
        self.trace.push("input".to_owned());
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
        if self.fail_application {
            Err("injected application failure".to_owned())
        } else {
            Ok(ApplicationEffectSupport::Performed)
        }
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let line = match &trace {
            ContainedTaskTrace::PackageAdmitted { .. } => "package_admitted".to_owned(),
            ContainedTaskTrace::RunStarted => "run_started".to_owned(),
            ContainedTaskTrace::CaptureCompleted { .. } => "capture_completed".to_owned(),
            ContainedTaskTrace::StepStarted { step_index, operation_label, from_page, .. } => {
                format!("step_started {step_index} {operation_label} from={from_page}")
            }
            ContainedTaskTrace::StepFinished { step_index, page_label, .. } => {
                format!("step_finished {step_index} {page_label}")
            }
            ContainedTaskTrace::RecognitionCompleted { candidate_pages, page_label, .. } => {
                format!("recognition {candidate_pages:?} -> {page_label:?}")
            }
            ContainedTaskTrace::EffectIntent { step_index, .. } => format!("effect_intent {step_index}"),
            ContainedTaskTrace::EffectCompleted { step_index, .. } => format!("effect_completed {step_index}"),
            ContainedTaskTrace::Finalizing { outcome } => format!("finalizing {outcome:?}"),
            _ => return Ok(()),
        };
        self.trace.push(line);
        Ok(())
    }
}

fn read_frame(path: &str) -> Frame {
    Frame::from_png(std::fs::read(path).expect("frame PNG"), CaptureBackendName::FixtureSimulation)
        .expect("frame")
}

fn load(case: &serde_json::Value) -> Result<PreparedContainedTask, actingcommand_execution_kernel::ContainedTaskError> {
    let reference = PackageRef::parse_argument(case["reference"].as_str().expect("reference"))
        .expect("package reference");
    PreparedContainedTask::load_path(
        "oneoff-336-l2e",
        Path::new(case["locator"].as_str().expect("locator")),
        &reference,
        None,
        Instant::now() + Duration::from_secs(60),
    )
}

fn main() {
    let cases_path = std::env::args().nth(1).expect("cases.json path");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cases_path).expect("read cases")).expect("cases JSON");
    let mut failures = 0_usize;
    for case in cases["admission"].as_array().expect("admission cases") {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let result = load(case);
        let (code, detail) = match &result {
            Ok(task) => (
                "ok".to_owned(),
                format!(
                    "package={} task={} mode={} maximum_executed_steps={} required_home_entry_page={:?}",
                    task.package_label(),
                    task.task_label(),
                    task.execution_mode(),
                    task.maximum_executed_steps(),
                    task.required_home_entry_page()
                ),
            ),
            Err(error) => (
                error.code().to_owned(),
                format!(
                    "{} declaration_issue={}",
                    error.detail().unwrap_or("").replace('\n', " "),
                    error
                        .declaration_issue()
                        .map(|issue| serde_json::to_string(issue).expect("issue JSON"))
                        .unwrap_or_default()
                ),
            ),
        };
        let accepted = if expect == "!ok" {
            code != "ok"
        } else {
            expect.split('|').any(|candidate| candidate == code)
        };
        let pass = accepted && detail.contains(expect_detail);
        failures += usize::from(!pass);
        println!(
            "KERNEL|load_path|{name}|expect={expect} {expect_detail}|code={code}|{}|{detail}",
            if pass { "PASS" } else { "FAIL" }
        );
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
                        let decision = serde_json::to_string(&simulation.decision).expect("decision JSON");
                        let wanted = if count == 0 {
                            case["expect_empty_decision"].as_str().unwrap_or("offline_fixture_missing")
                        } else {
                            case["expect_decision"].as_str().unwrap_or("would_click")
                        };
                        let wanted_captures = if count == 0 { 0 } else { case["expect_capture_count"].as_u64().unwrap_or(u64::MAX) };
                        let captures_ok = wanted_captures == u64::MAX || simulation.capture_count as u64 == wanted_captures;
                        let pass = decision.contains(wanted) && captures_ok;
                        failures += usize::from(!pass);
                        println!(
                            "KERNEL|offline_first_decision|{name}|frames={count}|{}|expect={wanted} capture_count={}|decision={decision}|capture_count={}|recognition={}",
                            if pass { "PASS" } else { "FAIL" },
                            if wanted_captures == u64::MAX { "any".to_owned() } else { wanted_captures.to_string() },
                            simulation.capture_count,
                            serde_json::to_string(&simulation.recognition).expect("recognition JSON")
                        );
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
        let expect_inputs = case["expect_inputs"].as_u64().expect("expect_inputs");
        let expect_applications = case["expect_applications"].as_u64().expect("expect_applications");
        let task = load(case).expect("admitted package");
        let mut runtime = FrameRuntime {
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
            fail_application: case["fail_application"].as_bool().unwrap_or(false),
            trace: Vec::new(),
        };
        let started = Instant::now();
        let (code, detail) = match task.run(&mut runtime) {
            Ok(outcome) => (
                "success".to_owned(),
                format!("final_page={:?} executed_steps={}", outcome.final_page, outcome.executed_steps),
            ),
            Err(ContainedTaskRunError::Task(error)) => (
                error.code().to_owned(),
                format!("{} timing={:?}", error.detail().unwrap_or(""), error.timing()),
            ),
            Err(other) => ("operation_error".to_owned(), format!("{other:?}")),
        };
        let pass = code == expect
            && detail.contains(expect_detail)
            && u64::from(runtime.inputs) == expect_inputs
            && runtime.applications.len() as u64 == expect_applications;
        failures += usize::from(!pass);
        println!(
            "KERNEL|run|{name}|expect={expect} {expect_detail} inputs={expect_inputs} applications={expect_applications}|code={code}|inputs={}|applications={:?}|captures={}|seconds={:.2}|{}|{detail}",
            runtime.inputs,
            runtime.applications,
            runtime.captures,
            started.elapsed().as_secs_f64(),
            if pass { "PASS" } else { "FAIL" }
        );
        for line in &runtime.trace {
            println!("KERNEL|run trace|{name}|{line}");
        }
    }
    println!("KERNEL|RESULT|failures={failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
