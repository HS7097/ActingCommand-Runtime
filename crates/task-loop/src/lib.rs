// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_page_detector::{PageDetector, PageEvaluation};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{PackRect, RecognitionEvaluator};
use serde::Deserialize;
use std::collections::HashSet;
use std::error::Error;
use std::fmt;

pub mod probe;

pub use probe::*;

pub type TaskLoopResult<T> = Result<T, TaskLoopError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLoopErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLoopError {
    severity: TaskLoopErrorSeverity,
    message: String,
}

impl TaskLoopError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            severity: TaskLoopErrorSeverity::Fatal,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> TaskLoopErrorSeverity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TaskLoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.severity {
            TaskLoopErrorSeverity::Fatal => {
                write!(f, "fatal task loop error: {}", self.message)
            }
        }
    }
}

impl Error for TaskLoopError {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaskPlan {
    pub schema_version: String,
    pub id: String,
    pub steps: Vec<TaskStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaskStep {
    pub id: String,
    pub page_id: String,
    pub on_match: TaskAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskAction {
    Complete,
    Click { target_id: String },
}

#[derive(Debug, Clone)]
pub struct DryRunTaskLoop {
    task_plan: TaskPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRunResult {
    pub task_id: String,
    pub matched_step_id: Option<String>,
    pub matched_page_id: Option<String>,
    pub status: DryRunStatus,
    pub action: Option<DryRunAction>,
    pub page_evaluations: Vec<PageEvaluation>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DryRunStatus {
    NoPageMatched,
    WouldComplete,
    WouldClick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DryRunAction {
    Complete,
    Click { target_id: String, click: PackRect },
}

pub fn load_task_plan_from_json_str(json: &str) -> TaskLoopResult<TaskPlan> {
    serde_json::from_str(json)
        .map_err(|err| TaskLoopError::fatal(format!("failed to parse task plan JSON: {err}")))
}

impl DryRunTaskLoop {
    pub fn new(task_plan: TaskPlan) -> TaskLoopResult<Self> {
        validate_task_plan_structure(&task_plan)?;
        Ok(Self { task_plan })
    }

    pub fn validate(
        &self,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
    ) -> TaskLoopResult<()> {
        for step in &self.task_plan.steps {
            if !detector.contains_page(&step.page_id) {
                return Err(TaskLoopError::fatal(format!(
                    "page id not found: {}",
                    step.page_id
                )));
            }

            if let TaskAction::Click { target_id } = &step.on_match {
                evaluator.get_click_target(target_id).map_err(pack_error)?;
            }
        }

        Ok(())
    }

    pub fn dry_run(
        &self,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
    ) -> TaskLoopResult<DryRunResult> {
        self.validate(detector, evaluator)?;

        let mut page_evaluations = Vec::new();
        for step in &self.task_plan.steps {
            let evaluation = detector
                .evaluate_page(evaluator, scene, &step.page_id)
                .map_err(page_error)?;
            let matched = evaluation.matched;
            page_evaluations.push(evaluation);

            if matched {
                return self.matched_result(step, evaluator, page_evaluations);
            }
        }

        Ok(DryRunResult {
            task_id: self.task_plan.id.clone(),
            matched_step_id: None,
            matched_page_id: None,
            status: DryRunStatus::NoPageMatched,
            action: None,
            page_evaluations,
            message: "no page matched".to_string(),
        })
    }

    fn matched_result(
        &self,
        step: &TaskStep,
        evaluator: &RecognitionEvaluator,
        page_evaluations: Vec<PageEvaluation>,
    ) -> TaskLoopResult<DryRunResult> {
        match &step.on_match {
            TaskAction::Complete => Ok(DryRunResult {
                task_id: self.task_plan.id.clone(),
                matched_step_id: Some(step.id.clone()),
                matched_page_id: Some(step.page_id.clone()),
                status: DryRunStatus::WouldComplete,
                action: Some(DryRunAction::Complete),
                page_evaluations,
                message: "step would complete task".to_string(),
            }),
            TaskAction::Click { target_id } => {
                let click = evaluator.get_click_target(target_id).map_err(pack_error)?;
                Ok(DryRunResult {
                    task_id: self.task_plan.id.clone(),
                    matched_step_id: Some(step.id.clone()),
                    matched_page_id: Some(step.page_id.clone()),
                    status: DryRunStatus::WouldClick,
                    action: Some(DryRunAction::Click {
                        target_id: target_id.clone(),
                        click,
                    }),
                    page_evaluations,
                    message: "step would click target".to_string(),
                })
            }
        }
    }
}

fn validate_task_plan_structure(task_plan: &TaskPlan) -> TaskLoopResult<()> {
    if task_plan.schema_version != "0.1" {
        return Err(TaskLoopError::fatal(format!(
            "unsupported schema_version '{}', expected '0.1'",
            task_plan.schema_version
        )));
    }
    if task_plan.id.is_empty() {
        return Err(TaskLoopError::fatal("task id is empty"));
    }
    if task_plan.steps.is_empty() {
        return Err(TaskLoopError::fatal("task steps must not be empty"));
    }

    let mut step_ids = HashSet::new();
    for step in &task_plan.steps {
        if step.id.is_empty() {
            return Err(TaskLoopError::fatal("step id is empty"));
        }
        if !step_ids.insert(step.id.clone()) {
            return Err(TaskLoopError::fatal(format!(
                "step id '{}' is duplicated",
                step.id
            )));
        }
        if step.page_id.is_empty() {
            return Err(TaskLoopError::fatal(format!(
                "step '{}' page_id is empty",
                step.id
            )));
        }
        if let TaskAction::Click { target_id } = &step.on_match
            && target_id.is_empty()
        {
            return Err(TaskLoopError::fatal(format!(
                "step '{}' click target_id is empty",
                step.id
            )));
        }
    }

    Ok(())
}

fn page_error(err: actingcommand_page_detector::PageDetectorError) -> TaskLoopError {
    TaskLoopError::fatal(err.to_string())
}

fn pack_error(err: actingcommand_recognition_pack::RecognitionPackError) -> TaskLoopError {
    TaskLoopError::fatal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_page_detector::{PageDefinition, PageDetector, PageSet};
    use actingcommand_recognition_pack::{
        ClickOnlyTarget, ColorTarget, PackCoordinateSpace, RecognitionDefaults, RecognitionPack,
        RecognitionTarget,
    };

    #[test]
    fn task_plan_json_parses() {
        let plan =
            load_task_plan_from_json_str(&task_plan_json("fixture/home_page", task_complete()))
                .expect("plan");

        assert_eq!(plan.id, "fixture.task");
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn unsupported_schema_is_fatal() {
        let err = DryRunTaskLoop::new(TaskPlan {
            schema_version: "9.9".to_string(),
            ..complete_plan("fixture/home_page")
        })
        .expect_err("unsupported schema");

        assert_fatal_contains(err, "unsupported schema_version");
    }

    #[test]
    fn task_id_empty_is_fatal() {
        let err = DryRunTaskLoop::new(TaskPlan {
            id: String::new(),
            ..complete_plan("fixture/home_page")
        })
        .expect_err("empty task id");

        assert_fatal_contains(err, "task id is empty");
    }

    #[test]
    fn steps_empty_is_fatal() {
        let err = DryRunTaskLoop::new(TaskPlan {
            steps: Vec::new(),
            ..complete_plan("fixture/home_page")
        })
        .expect_err("empty steps");

        assert_fatal_contains(err, "steps");
    }

    #[test]
    fn duplicate_step_id_is_fatal() {
        let err = DryRunTaskLoop::new(TaskPlan {
            steps: vec![
                TaskStep {
                    id: "same".to_string(),
                    page_id: "fixture/home_page".to_string(),
                    on_match: TaskAction::Complete,
                },
                TaskStep {
                    id: "same".to_string(),
                    page_id: "fixture/other_page".to_string(),
                    on_match: TaskAction::Complete,
                },
            ],
            ..complete_plan("fixture/home_page")
        })
        .expect_err("duplicate step");

        assert_fatal_contains(err, "duplicated");
    }

    #[test]
    fn page_id_empty_is_fatal_in_new() {
        let err = DryRunTaskLoop::new(complete_plan("")).expect_err("empty page id");

        assert_fatal_contains(err, "page_id is empty");
    }

    #[test]
    fn page_id_missing_is_fatal_in_validate() {
        let fixture = Fixture::new();
        let task_loop = DryRunTaskLoop::new(complete_plan("fixture/missing_page")).expect("loop");

        let err = task_loop
            .validate(&fixture.detector, &fixture.evaluator)
            .expect_err("missing page");

        assert_fatal_contains(err, "page id not found");
    }

    #[test]
    fn matched_page_complete_returns_would_complete() {
        let fixture = Fixture::new();
        let task_loop = DryRunTaskLoop::new(complete_plan("fixture/home_page")).expect("loop");

        let result = task_loop
            .dry_run(&fixture.detector, &fixture.evaluator, &scene(true, false))
            .expect("dry run");

        assert_eq!(result.status, DryRunStatus::WouldComplete);
        assert_eq!(result.action, Some(DryRunAction::Complete));
        assert_eq!(result.matched_step_id.as_deref(), Some("home_step"));
    }

    #[test]
    fn matched_page_click_returns_click_metadata() {
        let fixture = Fixture::new();
        let task_loop =
            DryRunTaskLoop::new(click_plan("fixture/home_page", "fixture/click")).expect("loop");

        let result = task_loop
            .dry_run(&fixture.detector, &fixture.evaluator, &scene(true, false))
            .expect("dry run");

        assert_eq!(result.status, DryRunStatus::WouldClick);
        assert_eq!(
            result.action,
            Some(DryRunAction::Click {
                target_id: "fixture/click".to_string(),
                click: PackRect {
                    x: 11,
                    y: 12,
                    width: 13,
                    height: 14
                }
            })
        );
    }

    #[test]
    fn no_page_matched_returns_no_page_matched() {
        let fixture = Fixture::new();
        let task_loop = DryRunTaskLoop::new(complete_plan("fixture/home_page")).expect("loop");

        let result = task_loop
            .dry_run(&fixture.detector, &fixture.evaluator, &scene(false, false))
            .expect("dry run");

        assert_eq!(result.status, DryRunStatus::NoPageMatched);
        assert_eq!(result.action, None);
        assert_eq!(result.matched_step_id, None);
        assert_eq!(result.page_evaluations.len(), 1);
    }

    #[test]
    fn matched_click_target_missing_is_fatal() {
        let fixture = Fixture::new();
        let task_loop =
            DryRunTaskLoop::new(click_plan("fixture/home_page", "fixture/missing")).expect("loop");

        let err = task_loop
            .validate(&fixture.detector, &fixture.evaluator)
            .expect_err("missing click target");

        assert_fatal_contains(err, "target id not found");
    }

    #[test]
    fn matched_click_target_without_click_is_fatal() {
        let fixture = Fixture::new();
        let task_loop =
            DryRunTaskLoop::new(click_plan("fixture/home_page", "fixture/no_click")).expect("loop");

        let err = task_loop
            .validate(&fixture.detector, &fixture.evaluator)
            .expect_err("missing click field");

        assert_fatal_contains(err, "has no click");
    }

    #[test]
    fn validate_catches_bad_target_in_non_matching_step() {
        let fixture = Fixture::new();
        let task_loop = DryRunTaskLoop::new(TaskPlan {
            steps: vec![
                TaskStep {
                    id: "home_step".to_string(),
                    page_id: "fixture/home_page".to_string(),
                    on_match: TaskAction::Complete,
                },
                TaskStep {
                    id: "other_step".to_string(),
                    page_id: "fixture/other_page".to_string(),
                    on_match: TaskAction::Click {
                        target_id: "fixture/missing".to_string(),
                    },
                },
            ],
            ..complete_plan("fixture/home_page")
        })
        .expect("loop");

        let err = task_loop
            .validate(&fixture.detector, &fixture.evaluator)
            .expect_err("bad target");

        assert_fatal_contains(err, "target id not found");
    }

    struct Fixture {
        evaluator: RecognitionEvaluator,
        detector: PageDetector,
    }

    impl Fixture {
        fn new() -> Self {
            let evaluator =
                RecognitionEvaluator::new(std::env::temp_dir(), fixture_pack()).expect("evaluator");
            let detector = PageDetector::new(PageSet {
                schema_version: "0.1".to_string(),
                pages: vec![
                    PageDefinition {
                        id: "fixture/home_page".to_string(),
                        required: vec!["fixture/home_anchor".to_string()],
                        optional: Vec::new(),
                        forbidden: Vec::new(),
                    },
                    PageDefinition {
                        id: "fixture/other_page".to_string(),
                        required: vec!["fixture/other_anchor".to_string()],
                        optional: Vec::new(),
                        forbidden: Vec::new(),
                    },
                ],
            })
            .expect("detector");
            detector.validate(&evaluator).expect("detector refs");
            Self {
                evaluator,
                detector,
            }
        }
    }

    fn complete_plan(page_id: &str) -> TaskPlan {
        TaskPlan {
            schema_version: "0.1".to_string(),
            id: "fixture.task".to_string(),
            steps: vec![TaskStep {
                id: "home_step".to_string(),
                page_id: page_id.to_string(),
                on_match: TaskAction::Complete,
            }],
        }
    }

    fn click_plan(page_id: &str, target_id: &str) -> TaskPlan {
        TaskPlan {
            schema_version: "0.1".to_string(),
            id: "fixture.task".to_string(),
            steps: vec![TaskStep {
                id: "home_step".to_string(),
                page_id: page_id.to_string(),
                on_match: TaskAction::Click {
                    target_id: target_id.to_string(),
                },
            }],
        }
    }

    fn task_plan_json(page_id: &str, action: &str) -> String {
        format!(
            r#"{{
                "schema_version": "0.1",
                "id": "fixture.task",
                "steps": [
                    {{
                        "id": "home_step",
                        "page_id": "{page_id}",
                        "on_match": {action}
                    }}
                ]
            }}"#
        )
    }

    fn task_complete() -> &'static str {
        r#"{ "type": "complete" }"#
    }

    fn fixture_pack() -> RecognitionPack {
        RecognitionPack {
            schema_version: "0.1".to_string(),
            game: Some("fixture".to_string()),
            server: Some("test".to_string()),
            locale: None,
            coordinate_space: Some(PackCoordinateSpace {
                width: 16,
                height: 16,
            }),
            defaults: RecognitionDefaults::default(),
            targets: vec![
                RecognitionTarget::Color(ColorTarget {
                    id: "fixture/home_anchor".to_string(),
                    region: rect(0, 0, 4, 4),
                    expected: [255, 0, 0],
                    click: None,
                }),
                RecognitionTarget::Color(ColorTarget {
                    id: "fixture/other_anchor".to_string(),
                    region: rect(4, 0, 4, 4),
                    expected: [0, 255, 0],
                    click: None,
                }),
                RecognitionTarget::Color(ColorTarget {
                    id: "fixture/no_click".to_string(),
                    region: rect(8, 0, 4, 4),
                    expected: [0, 0, 255],
                    click: None,
                }),
                RecognitionTarget::ClickOnly(ClickOnlyTarget {
                    id: "fixture/click".to_string(),
                    click: rect(11, 12, 13, 14),
                }),
            ],
        }
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
        PackRect {
            x,
            y,
            width,
            height,
        }
    }

    fn scene(home: bool, other: bool) -> Scene {
        let png = encode_png(16, 16, |x, y| {
            if home && x < 4 && y < 4 {
                [255, 0, 0]
            } else if other && (4..8).contains(&x) && y < 4 {
                [0, 255, 0]
            } else {
                [0, 0, 0]
            }
        });
        Scene::from_png(&png).expect("scene")
    }

    fn assert_fatal_contains(err: TaskLoopError, expected: &str) {
        assert_eq!(err.severity(), TaskLoopErrorSeverity::Fatal);
        assert!(
            err.message().contains(expected),
            "message was: {}",
            err.message()
        );
    }

    fn encode_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut scanlines = Vec::with_capacity(((width * 3 + 1) * height) as usize);
        for y in 0..height {
            scanlines.push(0);
            for x in 0..width {
                scanlines.extend_from_slice(&pixel(x, y));
            }
        }

        let len = u16::try_from(scanlines.len()).expect("test PNG fits one deflate block");
        let mut zlib = vec![0x78, 0x01, 0x01];
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(&scanlines);
        zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        append_chunk(&mut png, b"IHDR", &ihdr);
        append_chunk(&mut png, b"IDAT", &zlib);
        append_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn append_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let mut crc_data = Vec::with_capacity(kind.len() + data.len());
        crc_data.extend_from_slice(kind);
        crc_data.extend_from_slice(data);
        png.extend_from_slice(&crc32(&crc_data).to_be_bytes());
    }

    fn adler32(data: &[u8]) -> u32 {
        const MOD: u32 = 65_521;
        let mut a = 1_u32;
        let mut b = 0_u32;
        for byte in data {
            a = (a + u32::from(*byte)) % MOD;
            b = (b + a) % MOD;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0_u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }
}
