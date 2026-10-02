// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2a evidence: the contained-task loader's own codes
//! for the linear packages (`PreparedContainedTask::load_path`, the path actingd admits a
//! package through) and the offline first decision of the linear packages that admit.
//! Every printed line starts with `KERNEL|`.

use actingcommand_contract::{InputAction, PackageRef};
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_execution_kernel::{
    ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTrace, InputFrameContext,
    ObservedFrame, PreparedContainedTask, simulate_contained_task,
};
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

/// Replays saved frames in order and counts inputs: the kernel's own result, code and detail.
struct FrameRuntime {
    frames: VecDeque<Frame>,
    inputs: u32,
    trace: Vec<String>,
}

impl ContainedTaskRuntime for FrameRuntime {
    type Error = String;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.frames
            .pop_front()
            .map(ObservedFrame::from)
            .ok_or_else(|| "frames exhausted".to_owned())
    }

    fn input(&mut self, _action: InputAction, _frame: Option<InputFrameContext>) -> Result<(), Self::Error> {
        self.inputs += 1;
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let line = match &trace {
            ContainedTaskTrace::StepStarted { step_index, from_page, .. } => {
                format!("step_started {step_index} {from_page}")
            }
            ContainedTaskTrace::StepFinished { step_index, page_label, .. } => {
                format!("step_finished {step_index} {page_label}")
            }
            ContainedTaskTrace::RecognitionCompleted { candidate_pages, page_label, .. } => {
                format!("recognition {candidate_pages:?} -> {page_label:?}")
            }
            ContainedTaskTrace::EffectIntent { step_index, .. } => format!("effect_intent {step_index}"),
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

fn main() {
    let cases_path = std::env::args().nth(1).expect("cases.json path");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cases_path).expect("read cases")).expect("cases JSON");
    let mut failures = 0_usize;
    for case in cases["admission"].as_array().expect("admission cases") {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let reference = PackageRef::parse_argument(case["reference"].as_str().expect("reference"))
            .expect("package reference");
        let result = PreparedContainedTask::load_path(
            "oneoff-336-l2a",
            Path::new(case["locator"].as_str().expect("locator")),
            &reference,
            None,
            Instant::now() + Duration::from_secs(60),
        );
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
        let pass = code == expect && detail.contains(expect_detail);
        failures += usize::from(!pass);
        println!(
            "KERNEL|load_path|{name}|expect={expect} {expect_detail}|code={code}|{}|{detail}",
            if pass { "PASS" } else { "FAIL" }
        );
        if let (Ok(task), Some(frame)) = (&result, case["first_frame"].as_str()) {
            let frame = read_frame(frame);
            match simulate_contained_task(task, vec![frame]) {
                Ok(simulation) => {
                    let decision = serde_json::to_string(&simulation.decision).expect("decision JSON");
                    let pass = decision.contains(case["expect_decision"].as_str().unwrap_or("would_click"));
                    failures += usize::from(!pass);
                    println!(
                        "KERNEL|offline_first_decision|{name}|{}|{decision}|recognition={}",
                        if pass { "PASS" } else { "FAIL" },
                        serde_json::to_string(&simulation.recognition).expect("recognition JSON")
                    );
                }
                Err(error) => {
                    failures += 1;
                    println!("KERNEL|offline_first_decision|{name}|FAIL|{error}");
                }
            }
        }
    }
    for case in cases["runs"].as_array().expect("run cases") {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let expect_inputs = case["expect_inputs"].as_u64().expect("expect_inputs");
        let reference = PackageRef::parse_argument(case["reference"].as_str().expect("reference"))
            .expect("package reference");
        let task = PreparedContainedTask::load_path(
            "oneoff-336-l2a",
            Path::new(case["locator"].as_str().expect("locator")),
            &reference,
            None,
            Instant::now() + Duration::from_secs(60),
        )
        .expect("admitted package");
        let mut runtime = FrameRuntime {
            frames: case["frames"]
                .as_array()
                .expect("frames")
                .iter()
                .map(|frame| read_frame(frame.as_str().expect("frame path")))
                .collect(),
            inputs: 0,
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
            Err(other) => ("runtime_error".to_owned(), format!("{other:?}")),
        };
        let pass = code == expect && detail.contains(expect_detail) && u64::from(runtime.inputs) == expect_inputs;
        failures += usize::from(!pass);
        println!(
            "KERNEL|run|{name}|expect={expect} {expect_detail} inputs={expect_inputs}|code={code}|inputs={}|seconds={:.2}|{}|{detail}",
            runtime.inputs,
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
