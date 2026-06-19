// SPDX-License-Identifier: AGPL-3.0-only

use crate::{TaskLoopError, TaskLoopResult};
use actingcommand_page_detector::{PageDetector, PageEvaluation};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{PackRect, RecognitionEvaluator, TargetEvaluation};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

const MAX_PROBE_STEPS: usize = 10;
const MAX_NAVIGATION_CLICKS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProbePlan {
    pub schema_version: String,
    pub id: String,
    pub steps: Vec<ProbeStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProbeStep {
    pub id: String,
    #[serde(default)]
    pub page_id: Option<String>,
    pub action: ProbeAction,
    #[serde(default)]
    pub expect_after: Option<ProbeExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProbeAction {
    DetectPage {
        page_id: String,
    },
    Click {
        target_id: String,
        effect: ProbeClickEffect,
    },
    ObserveTargets {
        target_ids: Vec<String>,
    },
    ObservePage {
        page_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeClickEffect {
    NavigationOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProbeExpectation {
    pub page_id: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ProbeReferenceOverrides {
    pages: HashSet<String>,
    click_targets: HashMap<String, PackRect>,
}

#[derive(Debug, Clone)]
pub struct ProbeDecisionLoop {
    plan: ProbePlan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProbeStepDecision {
    SkippedPageGuard {
        step_id: String,
        page_id: String,
        evaluation: PageEvaluation,
    },
    SkippedExternalPageGuard {
        step_id: String,
        page_id: String,
        current_page_id: Option<String>,
    },
    DetectPage {
        step_id: String,
        page_id: String,
        evaluation: PageEvaluation,
    },
    ObservePage {
        step_id: String,
        page_id: String,
        evaluation: PageEvaluation,
    },
    ObserveTargets {
        step_id: String,
        evaluations: Vec<TargetEvaluation>,
    },
    Click {
        step_id: String,
        target_id: String,
        click: PackRect,
        expect_after: ProbeExpectation,
    },
}

pub fn load_probe_plan_from_json_str(json: &str) -> TaskLoopResult<ProbePlan> {
    serde_json::from_str(json)
        .map_err(|err| TaskLoopError::fatal(format!("failed to parse probe plan JSON: {err}")))
}

impl ProbeReferenceOverrides {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_page(&mut self, page_id: impl Into<String>) {
        self.pages.insert(page_id.into());
    }

    pub fn insert_click_target(&mut self, target_id: impl Into<String>, rect: PackRect) {
        self.click_targets.insert(target_id.into(), rect);
    }

    pub fn contains_page(&self, page_id: &str) -> bool {
        self.pages.contains(page_id)
    }

    pub fn click_target(&self, target_id: &str) -> Option<PackRect> {
        self.click_targets.get(target_id).copied()
    }
}

impl ProbeDecisionLoop {
    pub fn new(plan: ProbePlan) -> TaskLoopResult<Self> {
        validate_probe_plan_structure(&plan)?;
        Ok(Self { plan })
    }

    pub fn plan(&self) -> &ProbePlan {
        &self.plan
    }

    pub fn max_navigation_clicks(&self) -> usize {
        MAX_NAVIGATION_CLICKS
    }

    pub fn validate(
        &self,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
    ) -> TaskLoopResult<()> {
        self.validate_with_overrides(detector, evaluator, &ProbeReferenceOverrides::default())
    }

    pub fn validate_with_overrides(
        &self,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
        overrides: &ProbeReferenceOverrides,
    ) -> TaskLoopResult<()> {
        for step in &self.plan.steps {
            if let Some(page_id) = &step.page_id {
                validate_page_reference(detector, overrides, page_id)?;
            }

            match &step.action {
                ProbeAction::DetectPage { page_id } | ProbeAction::ObservePage { page_id } => {
                    validate_page_reference(detector, overrides, page_id)?;
                }
                ProbeAction::ObserveTargets { target_ids } => {
                    for target_id in target_ids {
                        evaluator.target_kind(target_id).map_err(pack_error)?;
                    }
                }
                ProbeAction::Click { target_id, .. } => {
                    validate_click_name_safety(&self.plan.id, &step.id, target_id)?;
                    click_target_rect(evaluator, overrides, target_id)?;
                    let expectation = step.expect_after.as_ref().ok_or_else(|| {
                        TaskLoopError::fatal(format!(
                            "step '{}' click requires expect_after.page_id",
                            step.id
                        ))
                    })?;
                    validate_page_reference(detector, overrides, &expectation.page_id)?;
                }
            }
        }

        Ok(())
    }

    pub fn decide_step(
        &self,
        step: &ProbeStep,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        overrides: &ProbeReferenceOverrides,
    ) -> TaskLoopResult<ProbeStepDecision> {
        self.decide_step_with_known_page(step, detector, evaluator, scene, overrides, None)
    }

    pub fn decide_step_with_known_page(
        &self,
        step: &ProbeStep,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        overrides: &ProbeReferenceOverrides,
        known_page_id: Option<&str>,
    ) -> TaskLoopResult<ProbeStepDecision> {
        self.validate_with_overrides(detector, evaluator, overrides)?;
        if let Some(page_id) = &step.page_id {
            if overrides.contains_page(page_id) && !detector.contains_page(page_id) {
                if known_page_id == Some(page_id.as_str()) {
                    return self.decide_action(step, detector, evaluator, scene, overrides);
                }
                return Ok(ProbeStepDecision::SkippedExternalPageGuard {
                    step_id: step.id.clone(),
                    page_id: page_id.clone(),
                    current_page_id: known_page_id.map(str::to_string),
                });
            }
            let evaluation = detector
                .evaluate_page(evaluator, scene, page_id)
                .map_err(page_error)?;
            if !evaluation.matched {
                return Ok(ProbeStepDecision::SkippedPageGuard {
                    step_id: step.id.clone(),
                    page_id: page_id.clone(),
                    evaluation,
                });
            }
        }

        self.decide_action(step, detector, evaluator, scene, overrides)
    }

    fn decide_action(
        &self,
        step: &ProbeStep,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        overrides: &ProbeReferenceOverrides,
    ) -> TaskLoopResult<ProbeStepDecision> {
        match &step.action {
            ProbeAction::DetectPage { page_id } => {
                let evaluation = detector
                    .evaluate_page(evaluator, scene, page_id)
                    .map_err(page_error)?;
                Ok(ProbeStepDecision::DetectPage {
                    step_id: step.id.clone(),
                    page_id: page_id.clone(),
                    evaluation,
                })
            }
            ProbeAction::ObservePage { page_id } => {
                let evaluation = detector
                    .evaluate_page(evaluator, scene, page_id)
                    .map_err(page_error)?;
                Ok(ProbeStepDecision::ObservePage {
                    step_id: step.id.clone(),
                    page_id: page_id.clone(),
                    evaluation,
                })
            }
            ProbeAction::ObserveTargets { target_ids } => {
                let evaluations = target_ids
                    .iter()
                    .map(|target_id| {
                        evaluator
                            .evaluate_target(scene, target_id)
                            .map_err(pack_error)
                    })
                    .collect::<TaskLoopResult<Vec<_>>>()?;
                Ok(ProbeStepDecision::ObserveTargets {
                    step_id: step.id.clone(),
                    evaluations,
                })
            }
            ProbeAction::Click { target_id, .. } => {
                validate_click_name_safety(&self.plan.id, &step.id, target_id)?;
                let expect_after = step.expect_after.clone().ok_or_else(|| {
                    TaskLoopError::fatal(format!("step '{}' click requires expect_after", step.id))
                })?;
                Ok(ProbeStepDecision::Click {
                    step_id: step.id.clone(),
                    target_id: target_id.clone(),
                    click: click_target_rect(evaluator, overrides, target_id)?,
                    expect_after,
                })
            }
        }
    }
}

fn validate_probe_plan_structure(plan: &ProbePlan) -> TaskLoopResult<()> {
    if plan.schema_version != "0.1" {
        return Err(TaskLoopError::fatal(format!(
            "unsupported schema_version '{}', expected '0.1'",
            plan.schema_version
        )));
    }
    if plan.id.is_empty() {
        return Err(TaskLoopError::fatal("probe id is empty"));
    }
    if plan.steps.is_empty() {
        return Err(TaskLoopError::fatal("probe steps must not be empty"));
    }
    if plan.steps.len() > MAX_PROBE_STEPS {
        return Err(TaskLoopError::fatal(format!(
            "probe steps must not exceed {MAX_PROBE_STEPS}"
        )));
    }

    let mut step_ids = HashSet::new();
    for step in &plan.steps {
        if step.id.is_empty() {
            return Err(TaskLoopError::fatal("probe step id is empty"));
        }
        if !step_ids.insert(step.id.clone()) {
            return Err(TaskLoopError::fatal(format!(
                "probe step id '{}' is duplicated",
                step.id
            )));
        }
        if let Some(page_id) = &step.page_id
            && page_id.is_empty()
        {
            return Err(TaskLoopError::fatal(format!(
                "probe step '{}' page_id is empty",
                step.id
            )));
        }
        validate_probe_action_structure(plan, step)?;
    }
    Ok(())
}

fn validate_probe_action_structure(plan: &ProbePlan, step: &ProbeStep) -> TaskLoopResult<()> {
    match &step.action {
        ProbeAction::DetectPage { page_id } | ProbeAction::ObservePage { page_id } => {
            if page_id.is_empty() {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' page_id is empty",
                    step.id
                )));
            }
            if step.expect_after.is_some() {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' non-click action must not set expect_after",
                    step.id
                )));
            }
        }
        ProbeAction::ObserveTargets { target_ids } => {
            if target_ids.is_empty() {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' observe_targets must not be empty",
                    step.id
                )));
            }
            if target_ids.iter().any(String::is_empty) {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' observe target id is empty",
                    step.id
                )));
            }
            if step.expect_after.is_some() {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' non-click action must not set expect_after",
                    step.id
                )));
            }
        }
        ProbeAction::Click { target_id, effect } => {
            if target_id.is_empty() {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' click target_id is empty",
                    step.id
                )));
            }
            if *effect != ProbeClickEffect::NavigationOnly {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' click effect must be navigation_only",
                    step.id
                )));
            }
            let expectation = step.expect_after.as_ref().ok_or_else(|| {
                TaskLoopError::fatal(format!(
                    "probe step '{}' click requires expect_after",
                    step.id
                ))
            })?;
            if expectation.page_id.is_empty() {
                return Err(TaskLoopError::fatal(format!(
                    "probe step '{}' expect_after.page_id is empty",
                    step.id
                )));
            }
            validate_click_name_safety(&plan.id, &step.id, target_id)?;
        }
    }
    Ok(())
}

fn validate_page_reference(
    detector: &PageDetector,
    overrides: &ProbeReferenceOverrides,
    page_id: &str,
) -> TaskLoopResult<()> {
    if detector.contains_page(page_id) || overrides.contains_page(page_id) {
        Ok(())
    } else {
        Err(TaskLoopError::fatal(format!(
            "page id not found: {page_id}"
        )))
    }
}

fn click_target_rect(
    evaluator: &RecognitionEvaluator,
    overrides: &ProbeReferenceOverrides,
    target_id: &str,
) -> TaskLoopResult<PackRect> {
    match evaluator.get_click_target(target_id) {
        Ok(rect) => Ok(rect),
        Err(err) if overrides.click_target(target_id).is_some() => {
            let _ = err;
            Ok(overrides
                .click_target(target_id)
                .expect("checked by is_some"))
        }
        Err(err) => Err(pack_error(err)),
    }
}

fn validate_click_name_safety(
    probe_id: &str,
    step_id: &str,
    target_id: &str,
) -> TaskLoopResult<()> {
    for (label, value) in [
        ("probe id", probe_id),
        ("probe step id", step_id),
        ("click target id", target_id),
    ] {
        if let Some(word) = dangerous_word(value) {
            return Err(TaskLoopError::fatal(format!(
                "{label} '{value}' contains destructive word '{word}'"
            )));
        }
    }
    Ok(())
}

fn dangerous_word(value: &str) -> Option<&'static str> {
    let lower = value.to_lowercase();
    DANGEROUS_WORDS
        .iter()
        .copied()
        .find(|word| lower.contains(word))
}

fn page_error(err: actingcommand_page_detector::PageDetectorError) -> TaskLoopError {
    TaskLoopError::fatal(err.to_string())
}

fn pack_error(err: actingcommand_recognition_pack::RecognitionPackError) -> TaskLoopError {
    TaskLoopError::fatal(err.to_string())
}

const DANGEROUS_WORDS: &[&str] = &[
    "claim",
    "collect",
    "receive",
    "reward",
    "buy",
    "purchase",
    "confirm",
    "ok",
    "delete",
    "retire",
    "scrap",
    "consume",
    "enhance",
    "awaken",
    "build",
    "construct",
    "sortie",
    "battle",
    "fight",
    "start",
    "finish",
    "complete",
    "exchange",
    "decompose",
    "mail",
    "一括",
    "受取",
    "领取",
    "購入",
    "確認",
    "出撃",
    "戦闘",
    "建造",
    "退役",
    "強化",
];

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_page_detector::{PageDefinition, PageSet};
    use actingcommand_recognition_pack::{
        ClickOnlyTarget, ColorTarget, PackCoordinateSpace, RecognitionDefaults, RecognitionPack,
        RecognitionTarget,
    };

    #[test]
    fn probe_plan_json_parses() {
        let plan = load_probe_plan_from_json_str(&probe_plan_json()).expect("probe");

        assert_eq!(plan.id, "fixture.probe");
        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn unsupported_schema_is_fatal() {
        let err = ProbeDecisionLoop::new(ProbePlan {
            schema_version: "9.9".to_string(),
            ..valid_probe_plan()
        })
        .expect_err("schema");

        assert_fatal_contains(err, "unsupported schema_version");
    }

    #[test]
    fn empty_probe_id_is_fatal() {
        let err = ProbeDecisionLoop::new(ProbePlan {
            id: String::new(),
            ..valid_probe_plan()
        })
        .expect_err("id");

        assert_fatal_contains(err, "probe id is empty");
    }

    #[test]
    fn empty_steps_is_fatal() {
        let err = ProbeDecisionLoop::new(ProbePlan {
            steps: Vec::new(),
            ..valid_probe_plan()
        })
        .expect_err("steps");

        assert_fatal_contains(err, "steps");
    }

    #[test]
    fn too_many_steps_is_fatal() {
        let mut plan = valid_probe_plan();
        plan.steps = (0..11)
            .map(|index| ProbeStep {
                id: format!("observe_{index}"),
                page_id: None,
                action: ProbeAction::ObserveTargets {
                    target_ids: vec!["fixture/home_anchor".to_string()],
                },
                expect_after: None,
            })
            .collect();

        let err = ProbeDecisionLoop::new(plan).expect_err("too many");
        assert_fatal_contains(err, "must not exceed");
    }

    #[test]
    fn duplicate_step_id_is_fatal() {
        let err = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![click_step("same"), observe_step("same")],
            ..valid_probe_plan()
        })
        .expect_err("duplicate");

        assert_fatal_contains(err, "duplicated");
    }

    #[test]
    fn click_without_expect_after_is_fatal() {
        let err = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![ProbeStep {
                expect_after: None,
                ..click_step("click_home")
            }],
            ..valid_probe_plan()
        })
        .expect_err("expect after");

        assert_fatal_contains(err, "expect_after");
    }

    #[test]
    fn observe_targets_empty_is_fatal() {
        let err = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![ProbeStep {
                action: ProbeAction::ObserveTargets {
                    target_ids: Vec::new(),
                },
                ..observe_step("observe")
            }],
            ..valid_probe_plan()
        })
        .expect_err("targets");

        assert_fatal_contains(err, "observe_targets");
    }

    #[test]
    fn invalid_click_effect_json_is_fatal_at_parse() {
        let err = load_probe_plan_from_json_str(
            r#"{
                "schema_version": "0.1",
                "id": "fixture.probe",
                "steps": [
                  {
                    "id": "click_home",
                    "action": { "type": "click", "target_id": "fixture/click", "effect": "destructive" },
                    "expect_after": { "page_id": "fixture/home_page" }
                  }
                ]
            }"#,
        )
        .expect_err("bad effect");

        assert_fatal_contains(err, "failed to parse probe plan JSON");
    }

    #[test]
    fn validate_catches_missing_page_in_non_matching_step() {
        let fixture = Fixture::new();
        let probe = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![
                click_step("click_home"),
                ProbeStep {
                    id: "observe_missing".to_string(),
                    page_id: None,
                    action: ProbeAction::ObservePage {
                        page_id: "fixture/missing".to_string(),
                    },
                    expect_after: None,
                },
            ],
            ..valid_probe_plan()
        })
        .expect("probe");

        let err = probe
            .validate(&fixture.detector, &fixture.evaluator)
            .expect_err("missing");

        assert_fatal_contains(err, "page id not found");
    }

    #[test]
    fn validate_accepts_override_click_and_page_references() {
        let fixture = Fixture::new();
        let probe = ProbeDecisionLoop::new(override_probe_plan()).expect("probe");
        let mut overrides = ProbeReferenceOverrides::new();
        overrides.insert_click_target("navigation/home_to_task", rect(46, 217, 40, 40));
        overrides.insert_page("bluearchive/task_center");

        probe
            .validate_with_overrides(&fixture.detector, &fixture.evaluator, &overrides)
            .expect("overrides");
    }

    #[test]
    fn known_external_page_guard_allows_followup_decision() {
        let fixture = Fixture::new();
        let probe = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![ProbeStep {
                id: "return_home".to_string(),
                page_id: Some("bluearchive/task_center".to_string()),
                action: ProbeAction::Click {
                    target_id: "fixture/click".to_string(),
                    effect: ProbeClickEffect::NavigationOnly,
                },
                expect_after: Some(ProbeExpectation {
                    page_id: "fixture/home_page".to_string(),
                    timeout_ms: None,
                    interval_ms: None,
                }),
            }],
            ..valid_probe_plan()
        })
        .expect("probe");
        let mut overrides = ProbeReferenceOverrides::new();
        overrides.insert_page("bluearchive/task_center");

        let decision = probe
            .decide_step_with_known_page(
                &probe.plan().steps[0],
                &fixture.detector,
                &fixture.evaluator,
                &scene(true),
                &overrides,
                Some("bluearchive/task_center"),
            )
            .expect("external guard");

        assert!(matches!(decision, ProbeStepDecision::Click { .. }));
    }

    #[test]
    fn external_page_guard_skips_without_known_page_match() {
        let fixture = Fixture::new();
        let probe = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![ProbeStep {
                id: "return_home".to_string(),
                page_id: Some("bluearchive/task_center".to_string()),
                action: ProbeAction::Click {
                    target_id: "fixture/click".to_string(),
                    effect: ProbeClickEffect::NavigationOnly,
                },
                expect_after: Some(ProbeExpectation {
                    page_id: "fixture/home_page".to_string(),
                    timeout_ms: None,
                    interval_ms: None,
                }),
            }],
            ..valid_probe_plan()
        })
        .expect("probe");
        let mut overrides = ProbeReferenceOverrides::new();
        overrides.insert_page("bluearchive/task_center");

        let decision = probe
            .decide_step_with_known_page(
                &probe.plan().steps[0],
                &fixture.detector,
                &fixture.evaluator,
                &scene(true),
                &overrides,
                Some("fixture/home_page"),
            )
            .expect("skip");

        assert!(matches!(
            decision,
            ProbeStepDecision::SkippedExternalPageGuard { .. }
        ));
    }

    #[test]
    fn dangerous_click_target_is_fatal() {
        let err = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![ProbeStep {
                id: "click_home".to_string(),
                page_id: None,
                action: ProbeAction::Click {
                    target_id: "fixture/collect_all".to_string(),
                    effect: ProbeClickEffect::NavigationOnly,
                },
                expect_after: Some(ProbeExpectation {
                    page_id: "fixture/home_page".to_string(),
                    timeout_ms: None,
                    interval_ms: None,
                }),
            }],
            ..valid_probe_plan()
        })
        .expect_err("danger");

        assert_fatal_contains(err, "destructive word");
    }

    #[test]
    fn observe_target_with_dangerous_name_is_allowed() {
        let probe = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![ProbeStep {
                id: "observe".to_string(),
                page_id: None,
                action: ProbeAction::ObserveTargets {
                    target_ids: vec!["fixture/reward_indicator".to_string()],
                },
                expect_after: None,
            }],
            ..valid_probe_plan()
        });

        assert!(probe.is_ok());
    }

    #[test]
    fn click_decision_returns_rect_without_executing() {
        let fixture = Fixture::new();
        let probe = ProbeDecisionLoop::new(valid_probe_plan()).expect("probe");
        probe
            .validate(&fixture.detector, &fixture.evaluator)
            .expect("validate");

        let decision = probe
            .decide_step(
                &probe.plan().steps[0],
                &fixture.detector,
                &fixture.evaluator,
                &scene(true),
                &ProbeReferenceOverrides::new(),
            )
            .expect("decision");

        assert_eq!(
            decision,
            ProbeStepDecision::Click {
                step_id: "click_home".to_string(),
                target_id: "fixture/click".to_string(),
                click: rect(11, 12, 13, 14),
                expect_after: ProbeExpectation {
                    page_id: "fixture/other_page".to_string(),
                    timeout_ms: Some(3000),
                    interval_ms: Some(100)
                }
            }
        );
    }

    #[test]
    fn click_decision_skips_when_page_guard_does_not_match() {
        let fixture = Fixture::new();
        let probe = ProbeDecisionLoop::new(valid_probe_plan()).expect("probe");
        let decision = probe
            .decide_step(
                &probe.plan().steps[0],
                &fixture.detector,
                &fixture.evaluator,
                &scene(false),
                &ProbeReferenceOverrides::new(),
            )
            .expect("decision");

        assert!(matches!(
            decision,
            ProbeStepDecision::SkippedPageGuard { .. }
        ));
    }

    #[test]
    fn observe_targets_evaluates_without_click() {
        let fixture = Fixture::new();
        let probe = ProbeDecisionLoop::new(ProbePlan {
            steps: vec![observe_step("observe")],
            ..valid_probe_plan()
        })
        .expect("probe");

        let decision = probe
            .decide_step(
                &probe.plan().steps[0],
                &fixture.detector,
                &fixture.evaluator,
                &scene(true),
                &ProbeReferenceOverrides::new(),
            )
            .expect("decision");

        match decision {
            ProbeStepDecision::ObserveTargets { evaluations, .. } => {
                assert_eq!(evaluations.len(), 1);
                assert!(evaluations[0].passed);
            }
            other => panic!("unexpected decision: {other:?}"),
        }
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
            Self {
                evaluator,
                detector,
            }
        }
    }

    fn valid_probe_plan() -> ProbePlan {
        ProbePlan {
            schema_version: "0.1".to_string(),
            id: "fixture.probe".to_string(),
            steps: vec![click_step("click_home"), observe_step("observe_status")],
        }
    }

    fn override_probe_plan() -> ProbePlan {
        ProbePlan {
            schema_version: "0.1".to_string(),
            id: "fixture.probe".to_string(),
            steps: vec![ProbeStep {
                id: "click_home".to_string(),
                page_id: Some("fixture/home_page".to_string()),
                action: ProbeAction::Click {
                    target_id: "navigation/home_to_task".to_string(),
                    effect: ProbeClickEffect::NavigationOnly,
                },
                expect_after: Some(ProbeExpectation {
                    page_id: "bluearchive/task_center".to_string(),
                    timeout_ms: Some(3000),
                    interval_ms: Some(100),
                }),
            }],
        }
    }

    fn click_step(id: &str) -> ProbeStep {
        ProbeStep {
            id: id.to_string(),
            page_id: Some("fixture/home_page".to_string()),
            action: ProbeAction::Click {
                target_id: "fixture/click".to_string(),
                effect: ProbeClickEffect::NavigationOnly,
            },
            expect_after: Some(ProbeExpectation {
                page_id: "fixture/other_page".to_string(),
                timeout_ms: Some(3000),
                interval_ms: Some(100),
            }),
        }
    }

    fn observe_step(id: &str) -> ProbeStep {
        ProbeStep {
            id: id.to_string(),
            page_id: None,
            action: ProbeAction::ObserveTargets {
                target_ids: vec!["fixture/home_anchor".to_string()],
            },
            expect_after: None,
        }
    }

    fn probe_plan_json() -> String {
        r#"{
            "schema_version": "0.1",
            "id": "fixture.probe",
            "steps": [
              {
                "id": "click_home",
                "page_id": "fixture/home_page",
                "action": {
                  "type": "click",
                  "target_id": "fixture/click",
                  "effect": "navigation_only"
                },
                "expect_after": {
                  "page_id": "fixture/other_page",
                  "timeout_ms": 3000,
                  "interval_ms": 100
                }
              },
              {
                "id": "observe_status",
                "action": {
                  "type": "observe_targets",
                  "target_ids": ["fixture/home_anchor"]
                }
              }
            ]
        }"#
        .to_string()
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

    fn scene(home: bool) -> Scene {
        let png = encode_png(16, 16, |x, y| {
            if home && x < 4 && y < 4 {
                [255, 0, 0]
            } else {
                [0, 0, 0]
            }
        });
        Scene::from_png(&png).expect("scene")
    }

    fn assert_fatal_contains(err: TaskLoopError, expected: &str) {
        assert!(
            err.message().contains(expected),
            "expected '{expected}' in '{}'",
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
        let mut crc_data = Vec::with_capacity(kind.len() + data.len());
        crc_data.extend_from_slice(kind);
        crc_data.extend_from_slice(data);
        png.extend_from_slice(data);
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
