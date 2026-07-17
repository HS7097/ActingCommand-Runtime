// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{RecognitionEvaluator, TargetKind};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

pub type PageDetectorResult<T> = Result<T, PageDetectorError>;
pub type PageBatchResult = Result<Vec<PageOutcome>, BatchLevelError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDetectorErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDetectorError {
    severity: PageDetectorErrorSeverity,
    message: String,
}

impl PageDetectorError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            severity: PageDetectorErrorSeverity::Fatal,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> PageDetectorErrorSeverity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PageDetectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.severity {
            PageDetectorErrorSeverity::Fatal => {
                write!(f, "fatal page detector error: {}", self.message)
            }
        }
    }
}

impl Error for PageDetectorError {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PageSet {
    pub schema_version: String,
    pub pages: Vec<PageDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PageDefinition {
    pub id: String,
    pub required: Vec<String>,
    #[serde(default)]
    pub any_of: Vec<Vec<String>>,
    #[serde(default)]
    pub optional: Vec<String>,
    #[serde(default)]
    pub forbidden: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PageDetector {
    page_set: PageSet,
    page_indexes: HashMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEvaluation {
    pub page_id: String,
    pub matched: bool,
    pub required_passed: usize,
    pub required_total: usize,
    pub any_of_passed: usize,
    pub any_of_total: usize,
    pub optional_passed: usize,
    pub optional_total: usize,
    pub forbidden_passed: usize,
    pub forbidden_total: usize,
    pub target_results: Vec<PageTargetEvaluation>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageOutcome {
    pub index: usize,
    pub page_id: String,
    pub result: PageDetectorResult<PageEvaluation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageUnexecutedReason {
    BatchValidationFailed,
    BatchTerminated,
}

impl fmt::Display for PageUnexecutedReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BatchValidationFailed => formatter.write_str("batch_validation_failed"),
            Self::BatchTerminated => formatter.write_str("batch_terminated"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnexecutedPage {
    pub index: usize,
    pub page_id: String,
    pub reason: PageUnexecutedReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchLevelError {
    pub cause: PageDetectorError,
    pub completed: Vec<PageOutcome>,
    pub unexecuted: Vec<UnexecutedPage>,
}

impl fmt::Display for BatchLevelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "page evaluation batch terminated after {} completed page(s), leaving {} unexecuted page(s): {}",
            self.completed.len(),
            self.unexecuted.len(),
            self.cause
        )
    }
}

impl Error for BatchLevelError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageTargetEvaluation {
    pub target_id: String,
    pub role: PageTargetRole,
    pub passed: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTargetRole {
    Required,
    AnyOf,
    Optional,
    Forbidden,
}

pub fn load_page_set_from_json_str(json: &str) -> PageDetectorResult<PageSet> {
    serde_json::from_str(json)
        .map_err(|err| PageDetectorError::fatal(format!("failed to parse page set JSON: {err}")))
}

pub fn require_all_page_evaluations(
    outcomes: Vec<PageOutcome>,
) -> PageDetectorResult<Vec<PageEvaluation>> {
    let mut evaluations = Vec::with_capacity(outcomes.len());
    let mut failures = Vec::new();

    for outcome in outcomes {
        match outcome.result {
            Ok(evaluation) => evaluations.push(evaluation),
            Err(error) => failures.push(format!(
                "[{}] '{}': {}",
                outcome.index, outcome.page_id, error
            )),
        }
    }

    if failures.is_empty() {
        Ok(evaluations)
    } else {
        Err(PageDetectorError::fatal(format!(
            "page evaluation batch completed with {} successful page(s) and {} failed page(s): {}",
            evaluations.len(),
            failures.len(),
            failures.join("; ")
        )))
    }
}

impl PageDetector {
    pub fn new(page_set: PageSet) -> PageDetectorResult<Self> {
        validate_page_set(&page_set)?;
        let page_indexes = page_set
            .pages
            .iter()
            .enumerate()
            .map(|(index, page)| (page.id.clone(), index))
            .collect();

        Ok(Self {
            page_set,
            page_indexes,
        })
    }

    pub fn validate(&self, evaluator: &RecognitionEvaluator) -> PageDetectorResult<()> {
        for page in &self.page_set.pages {
            for target_id in page_target_ids(page) {
                match evaluator.target_kind(target_id).map_err(pack_error)? {
                    TargetKind::Template | TargetKind::Color => {}
                    TargetKind::ClickOnly => {
                        return Err(PageDetectorError::fatal(format!(
                            "page definition references click-only target: {target_id}"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn contains_page(&self, page_id: &str) -> bool {
        self.page_indexes.contains_key(page_id)
    }

    pub fn page_ids(&self) -> impl Iterator<Item = &str> {
        self.page_set.pages.iter().map(|page| page.id.as_str())
    }

    pub fn evaluate_page(
        &self,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        page_id: &str,
    ) -> PageDetectorResult<PageEvaluation> {
        self.validate(evaluator)?;
        let page = self.page(page_id)?;
        self.evaluate_page_definition(evaluator, scene, page)
    }

    pub fn evaluate_all(
        &self,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
    ) -> PageDetectorResult<Vec<PageEvaluation>> {
        let outcomes = self
            .evaluate_all_outcomes(evaluator, scene)
            .map_err(|error| PageDetectorError::fatal(error.to_string()))?;
        require_all_page_evaluations(outcomes)
    }

    pub fn evaluate_all_outcomes(
        &self,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
    ) -> PageBatchResult {
        self.evaluate_all_outcomes_with(evaluator, scene, |_, page| {
            self.evaluate_page_definition(evaluator, scene, page)
        })
    }

    fn evaluate_all_outcomes_with(
        &self,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        mut evaluate: impl FnMut(usize, &PageDefinition) -> PageDetectorResult<PageEvaluation>,
    ) -> PageBatchResult {
        if let Err(cause) = validate_batch_request(evaluator, scene) {
            return Err(self.batch_error(
                cause,
                Vec::new(),
                0,
                PageUnexecutedReason::BatchValidationFailed,
            ));
        }

        let mut completed = Vec::with_capacity(self.page_set.pages.len());
        for (index, page) in self.page_set.pages.iter().enumerate() {
            match evaluate(index, page) {
                Ok(evaluation) if evaluation.page_id == page.id => completed.push(PageOutcome {
                    index,
                    page_id: page.id.clone(),
                    result: Ok(evaluation),
                }),
                Ok(evaluation) => {
                    let cause = PageDetectorError::fatal(format!(
                        "page evaluation invariant failed at index {index}: expected page_id '{}', got '{}'",
                        page.id, evaluation.page_id
                    ));
                    completed.push(PageOutcome {
                        index,
                        page_id: page.id.clone(),
                        result: Err(cause.clone()),
                    });
                    return Err(self.batch_error(
                        cause,
                        completed,
                        index + 1,
                        PageUnexecutedReason::BatchTerminated,
                    ));
                }
                Err(error) => completed.push(PageOutcome {
                    index,
                    page_id: page.id.clone(),
                    result: Err(error),
                }),
            }
        }
        Ok(completed)
    }

    fn batch_error(
        &self,
        cause: PageDetectorError,
        completed: Vec<PageOutcome>,
        first_unexecuted: usize,
        reason: PageUnexecutedReason,
    ) -> BatchLevelError {
        let unexecuted = self
            .page_set
            .pages
            .iter()
            .enumerate()
            .skip(first_unexecuted)
            .map(|(index, page)| UnexecutedPage {
                index,
                page_id: page.id.clone(),
                reason,
            })
            .collect();
        BatchLevelError {
            cause,
            completed,
            unexecuted,
        }
    }

    fn page(&self, page_id: &str) -> PageDetectorResult<&PageDefinition> {
        let index = self
            .page_indexes
            .get(page_id)
            .ok_or_else(|| PageDetectorError::fatal(format!("page id not found: {page_id}")))?;
        Ok(&self.page_set.pages[*index])
    }

    fn evaluate_page_definition(
        &self,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        page: &PageDefinition,
    ) -> PageDetectorResult<PageEvaluation> {
        let required_results =
            evaluate_targets(evaluator, scene, &page.required, PageTargetRole::Required)?;
        let any_of_results = evaluate_any_of_groups(evaluator, scene, &page.any_of)?;
        let optional_results =
            evaluate_targets(evaluator, scene, &page.optional, PageTargetRole::Optional)?;
        let forbidden_results =
            evaluate_targets(evaluator, scene, &page.forbidden, PageTargetRole::Forbidden)?;

        let required_passed = count_passed(&required_results);
        let any_of_passed = count_passed_groups(&any_of_results);
        let optional_passed = count_passed(&optional_results);
        let forbidden_passed = count_passed(&forbidden_results);
        let matched = required_passed == page.required.len()
            && any_of_passed == page.any_of.len()
            && forbidden_passed == 0;
        let message = page_message(
            matched,
            &required_results,
            &any_of_results,
            &forbidden_results,
        );

        let mut target_results = required_results;
        target_results.extend(any_of_results.iter().flatten().cloned());
        target_results.extend(optional_results);
        target_results.extend(forbidden_results);

        Ok(PageEvaluation {
            page_id: page.id.clone(),
            matched,
            required_passed,
            required_total: page.required.len(),
            any_of_passed,
            any_of_total: page.any_of.len(),
            optional_passed,
            optional_total: page.optional.len(),
            forbidden_passed,
            forbidden_total: page.forbidden.len(),
            target_results,
            message,
        })
    }
}

fn validate_batch_request(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
) -> PageDetectorResult<()> {
    let expected =
        evaluator.pack().coordinate_space.as_ref().ok_or_else(|| {
            PageDetectorError::fatal("recognition pack coordinate_space is required")
        })?;
    if scene.width() != expected.width || scene.height() != expected.height {
        return Err(PageDetectorError::fatal(format!(
            "scene dimensions {}x{} do not match pack coordinate_space {}x{}",
            scene.width(),
            scene.height(),
            expected.width,
            expected.height
        )));
    }
    Ok(())
}

fn validate_page_set(page_set: &PageSet) -> PageDetectorResult<()> {
    if !matches!(
        page_set.schema_version.as_str(),
        "0.1" | "0.3" | "0.4" | "0.5"
    ) {
        return Err(PageDetectorError::fatal(format!(
            "unsupported schema_version '{}', expected one of '0.1', '0.3', '0.4', '0.5'",
            page_set.schema_version
        )));
    }

    let mut page_ids = HashSet::new();
    for (index, page) in page_set.pages.iter().enumerate() {
        if page.id.is_empty() {
            return Err(PageDetectorError::fatal(format!(
                "page[{index}] id is empty"
            )));
        }
        if !page_ids.insert(page.id.clone()) {
            return Err(PageDetectorError::fatal(format!(
                "page id '{}' is duplicated",
                page.id
            )));
        }
        validate_page_definition(page)?;
    }
    Ok(())
}

fn validate_page_definition(page: &PageDefinition) -> PageDetectorResult<()> {
    if page.required.is_empty() && page.any_of.is_empty() {
        return Err(PageDetectorError::fatal(format!(
            "page '{}' must have required targets or any_of groups",
            page.id
        )));
    }

    let mut seen = HashMap::<&str, PageTargetRole>::new();
    for (targets, role) in [
        (&page.required, PageTargetRole::Required),
        (&page.optional, PageTargetRole::Optional),
        (&page.forbidden, PageTargetRole::Forbidden),
    ] {
        for target_id in targets {
            if target_id.is_empty() {
                return Err(PageDetectorError::fatal(format!(
                    "page '{}' target id is empty",
                    page.id
                )));
            }
            match seen.insert(target_id, role) {
                None => {}
                Some(previous) if previous == role => {
                    return Err(PageDetectorError::fatal(format!(
                        "page '{}' target '{}' is duplicated",
                        page.id, target_id
                    )));
                }
                Some(previous) => {
                    return Err(PageDetectorError::fatal(format!(
                        "page '{}' target '{}' appears in both {} and {}",
                        page.id,
                        target_id,
                        role_name(previous),
                        role_name(role)
                    )));
                }
            }
        }
    }
    for (group_index, group) in page.any_of.iter().enumerate() {
        if group.is_empty() {
            return Err(PageDetectorError::fatal(format!(
                "page '{}' any_of[{group_index}] must not be empty",
                page.id
            )));
        }
        for target_id in group {
            if target_id.is_empty() {
                return Err(PageDetectorError::fatal(format!(
                    "page '{}' target id is empty",
                    page.id
                )));
            }
            match seen.insert(target_id, PageTargetRole::AnyOf) {
                None => {}
                Some(PageTargetRole::AnyOf) => {
                    return Err(PageDetectorError::fatal(format!(
                        "page '{}' target '{}' is duplicated",
                        page.id, target_id
                    )));
                }
                Some(previous) => {
                    return Err(PageDetectorError::fatal(format!(
                        "page '{}' target '{}' appears in both {} and any_of",
                        page.id,
                        target_id,
                        role_name(previous)
                    )));
                }
            }
        }
    }

    Ok(())
}

fn page_target_ids(page: &PageDefinition) -> impl Iterator<Item = &str> {
    page.required
        .iter()
        .chain(page.any_of.iter().flatten())
        .chain(page.optional.iter())
        .chain(page.forbidden.iter())
        .map(String::as_str)
}

fn evaluate_targets(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    target_ids: &[String],
    role: PageTargetRole,
) -> PageDetectorResult<Vec<PageTargetEvaluation>> {
    target_ids
        .iter()
        .map(|target_id| {
            let evaluation = evaluator
                .evaluate_target(scene, target_id)
                .map_err(pack_error)?;
            Ok(PageTargetEvaluation {
                target_id: target_id.clone(),
                role,
                passed: evaluation.passed,
                message: evaluation.message,
            })
        })
        .collect()
}

fn evaluate_any_of_groups(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    groups: &[Vec<String>],
) -> PageDetectorResult<Vec<Vec<PageTargetEvaluation>>> {
    groups
        .iter()
        .map(|group| evaluate_targets(evaluator, scene, group, PageTargetRole::AnyOf))
        .collect()
}

fn count_passed(results: &[PageTargetEvaluation]) -> usize {
    results.iter().filter(|result| result.passed).count()
}

fn count_passed_groups(groups: &[Vec<PageTargetEvaluation>]) -> usize {
    groups
        .iter()
        .filter(|group| group.iter().any(|result| result.passed))
        .count()
}

fn page_message(
    matched: bool,
    required_results: &[PageTargetEvaluation],
    any_of_results: &[Vec<PageTargetEvaluation>],
    forbidden_results: &[PageTargetEvaluation],
) -> String {
    if matched {
        return "page matched".to_string();
    }
    if let Some(result) = required_results.iter().find(|result| !result.passed) {
        return format!("required target failed: {}", result.target_id);
    }
    if let Some(result) = forbidden_results.iter().find(|result| result.passed) {
        return format!("forbidden target passed: {}", result.target_id);
    }
    if let Some(group) = any_of_results
        .iter()
        .find(|group| !group.iter().any(|result| result.passed))
    {
        let targets = group
            .iter()
            .map(|result| result.target_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return format!("any_of group failed: {targets}");
    }
    "page not matched".to_string()
}

fn role_name(role: PageTargetRole) -> &'static str {
    match role {
        PageTargetRole::Required => "required",
        PageTargetRole::AnyOf => "any_of",
        PageTargetRole::Optional => "optional",
        PageTargetRole::Forbidden => "forbidden",
    }
}

fn pack_error(err: actingcommand_recognition_pack::RecognitionPackError) -> PageDetectorError {
    PageDetectorError::fatal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_recognition_pack::{
        ClickOnlyTarget, ColorTarget, PackCoordinateSpace, PackRect, PackRegion,
        RecognitionDefaults, RecognitionPack, RecognitionTarget, TemplateTarget,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn page_set_parses() {
        let page_set = load_page_set_from_json_str(
            r#"{
                "schema_version": "0.1",
                "pages": [
                    {
                        "id": "fixture/home_page",
                        "required": ["fixture/home_anchor"],
                        "optional": ["fixture/settings_anchor"],
                        "forbidden": ["fixture/forbidden_popup"]
                    }
                ]
            }"#,
        )
        .expect("page set");

        assert_eq!(page_set.schema_version, "0.1");
        assert_eq!(page_set.pages.len(), 1);
    }

    #[test]
    fn page_set_parse_failure_is_fatal() {
        let err = load_page_set_from_json_str("{").expect_err("invalid JSON should fail");

        assert_fatal_contains(err, "failed to parse page set JSON");
    }

    #[test]
    fn new_accepts_valid_page_set() {
        PageDetector::new(home_page_set()).expect("detector");
    }

    #[test]
    fn schema_0_3_page_set_is_supported() {
        PageDetector::new(PageSet {
            schema_version: "0.3".to_string(),
            pages: vec![home_page()],
        })
        .expect("schema 0.3 detector");
    }

    #[test]
    fn contains_page_reports_known_pages() {
        let detector = PageDetector::new(home_page_set()).expect("detector");

        assert!(detector.contains_page("fixture/home_page"));
        assert!(!detector.contains_page("fixture/missing"));
    }

    #[test]
    fn unsupported_schema_is_fatal() {
        let err = PageDetector::new(PageSet {
            schema_version: "9.9".to_string(),
            pages: vec![home_page()],
        })
        .expect_err("unsupported schema");

        assert_fatal_contains(err, "unsupported schema_version");
    }

    #[test]
    fn empty_page_id_is_fatal() {
        let err = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: String::new(),
                required: vec!["fixture/home_anchor".to_string()],
                any_of: Vec::new(),
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
            ..base_page_set()
        })
        .expect_err("empty page id");

        assert_fatal_contains(err, "id is empty");
    }

    #[test]
    fn duplicate_page_id_is_fatal() {
        let err = PageDetector::new(PageSet {
            pages: vec![home_page(), home_page()],
            ..base_page_set()
        })
        .expect_err("duplicate page id");

        assert_fatal_contains(err, "duplicated");
    }

    #[test]
    fn required_empty_is_fatal() {
        let err = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/home_page".to_string(),
                required: Vec::new(),
                any_of: Vec::new(),
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
            ..base_page_set()
        })
        .expect_err("required empty");

        assert_fatal_contains(err, "required");
    }

    #[test]
    fn any_of_group_can_satisfy_page_without_required_targets() {
        let fixture = Fixture::new();
        let detector = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/menu_page".to_string(),
                required: Vec::new(),
                any_of: vec![vec![
                    "fixture/home_anchor".to_string(),
                    "fixture/settings_anchor".to_string(),
                ]],
                optional: Vec::new(),
                forbidden: vec!["fixture/forbidden_popup".to_string()],
            }],
            ..base_page_set()
        })
        .expect("detector");

        let evaluation = detector
            .evaluate_page(
                &fixture.evaluator,
                &scene_colors(false, true, false),
                "fixture/menu_page",
            )
            .expect("evaluate any_of page");

        assert!(evaluation.matched);
        assert_eq!(evaluation.required_total, 0);
        assert_eq!(evaluation.any_of_passed, 1);
        assert_eq!(evaluation.any_of_total, 1);
        assert!(evaluation.target_results.iter().any(|result| {
            result.role == PageTargetRole::AnyOf
                && result.target_id == "fixture/settings_anchor"
                && result.passed
        }));
    }

    #[test]
    fn any_of_group_failed_does_not_match() {
        let fixture = Fixture::new();
        let detector = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/menu_page".to_string(),
                required: Vec::new(),
                any_of: vec![vec![
                    "fixture/home_anchor".to_string(),
                    "fixture/settings_anchor".to_string(),
                ]],
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
            ..base_page_set()
        })
        .expect("detector");

        let evaluation = detector
            .evaluate_page(
                &fixture.evaluator,
                &scene_colors(false, false, false),
                "fixture/menu_page",
            )
            .expect("evaluate any_of page");

        assert!(!evaluation.matched);
        assert_eq!(evaluation.any_of_passed, 0);
        assert_eq!(
            evaluation.message,
            "any_of group failed: fixture/home_anchor, fixture/settings_anchor"
        );
    }

    #[test]
    fn any_of_empty_group_is_fatal() {
        let err = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/menu_page".to_string(),
                required: Vec::new(),
                any_of: vec![Vec::new()],
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
            ..base_page_set()
        })
        .expect_err("empty any_of group");

        assert_fatal_contains(err, "any_of[0]");
    }

    #[test]
    fn any_of_conflict_with_required_is_fatal() {
        let err = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/menu_page".to_string(),
                required: vec!["fixture/home_anchor".to_string()],
                any_of: vec![vec!["fixture/home_anchor".to_string()]],
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
            ..base_page_set()
        })
        .expect_err("any_of conflict");

        assert_fatal_contains(err, "appears in both required and any_of");
    }

    #[test]
    fn duplicate_target_in_same_role_is_fatal() {
        let err = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/home_page".to_string(),
                required: vec![
                    "fixture/home_anchor".to_string(),
                    "fixture/home_anchor".to_string(),
                ],
                any_of: Vec::new(),
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
            ..base_page_set()
        })
        .expect_err("duplicate target");

        assert_fatal_contains(err, "duplicated");
    }

    #[test]
    fn target_conflict_across_roles_is_fatal() {
        let err = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/home_page".to_string(),
                required: vec!["fixture/home_anchor".to_string()],
                any_of: Vec::new(),
                optional: Vec::new(),
                forbidden: vec!["fixture/home_anchor".to_string()],
            }],
            ..base_page_set()
        })
        .expect_err("target conflict");

        assert_fatal_contains(err, "appears in both");
    }

    #[test]
    fn required_all_passed_matches() {
        let fixture = Fixture::new();
        let evaluation = fixture.evaluate_home_page(scene_colors(true, false, false));

        assert!(evaluation.matched);
        assert_eq!(evaluation.required_passed, 1);
        assert_eq!(evaluation.message, "page matched");
    }

    #[test]
    fn required_failed_does_not_match() {
        let fixture = Fixture::new();
        let evaluation = fixture.evaluate_home_page(scene_colors(false, false, false));

        assert!(!evaluation.matched);
        assert_eq!(evaluation.required_passed, 0);
        assert_eq!(
            evaluation.message,
            "required target failed: fixture/home_anchor"
        );
    }

    #[test]
    fn optional_failed_does_not_affect_match() {
        let fixture = Fixture::new();
        let evaluation = fixture.evaluate_home_page(scene_colors(true, false, false));

        assert!(evaluation.matched);
        assert_eq!(evaluation.optional_passed, 0);
    }

    #[test]
    fn optional_passed_is_reported() {
        let fixture = Fixture::new();
        let evaluation = fixture.evaluate_home_page(scene_colors(true, true, false));

        assert!(evaluation.matched);
        assert_eq!(evaluation.optional_passed, 1);
        assert!(evaluation.target_results.iter().any(|result| {
            result.role == PageTargetRole::Optional
                && result.target_id == "fixture/settings_anchor"
                && result.passed
        }));
    }

    #[test]
    fn forbidden_passed_does_not_match() {
        let fixture = Fixture::new();
        let evaluation = fixture.evaluate_home_page(scene_colors(true, false, true));

        assert!(!evaluation.matched);
        assert_eq!(evaluation.forbidden_passed, 1);
        assert_eq!(
            evaluation.message,
            "forbidden target passed: fixture/forbidden_popup"
        );
    }

    #[test]
    fn forbidden_failed_does_not_affect_match() {
        let fixture = Fixture::new();
        let evaluation = fixture.evaluate_home_page(scene_colors(true, true, false));

        assert!(evaluation.matched);
        assert_eq!(evaluation.forbidden_passed, 0);
    }

    #[test]
    fn missing_page_id_is_fatal() {
        let fixture = Fixture::new();
        let err = fixture
            .detector
            .evaluate_page(
                &fixture.evaluator,
                &scene_colors(true, false, false),
                "missing",
            )
            .expect_err("missing page");

        assert_fatal_contains(err, "page id not found");
    }

    #[test]
    fn missing_target_id_is_fatal() {
        let fixture = Fixture::new();
        let detector = PageDetector::new(PageSet {
            pages: vec![PageDefinition {
                id: "fixture/home_page".to_string(),
                required: vec!["fixture/missing".to_string()],
                any_of: Vec::new(),
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
            ..base_page_set()
        })
        .expect("detector");

        let err = detector
            .validate(&fixture.evaluator)
            .expect_err("missing target");

        assert_fatal_contains(err, "target id not found");
    }

    #[test]
    fn click_only_required_is_fatal() {
        assert_click_only_role_is_fatal(PageTargetRole::Required);
    }

    #[test]
    fn click_only_optional_is_fatal() {
        assert_click_only_role_is_fatal(PageTargetRole::Optional);
    }

    #[test]
    fn click_only_forbidden_is_fatal() {
        assert_click_only_role_is_fatal(PageTargetRole::Forbidden);
    }

    #[test]
    fn click_only_any_of_is_fatal() {
        assert_click_only_role_is_fatal(PageTargetRole::AnyOf);
    }

    #[test]
    fn evaluate_all_returns_all_pages() {
        let fixture = Fixture::new();
        let evaluations = fixture
            .detector
            .evaluate_all(&fixture.evaluator, &scene_colors(true, true, false))
            .expect("evaluate all");

        assert_eq!(evaluations.len(), 2);
        assert!(
            evaluations.iter().any(|evaluation| {
                evaluation.page_id == "fixture/home_page" && evaluation.matched
            })
        );
        assert!(evaluations.iter().any(|evaluation| {
            evaluation.page_id == "fixture/settings_page" && evaluation.matched
        }));
    }

    #[test]
    fn evaluate_all_keeps_order_and_continues_after_page_error() {
        let fixture = Fixture::new();
        let detector = PageDetector::new(PageSet {
            pages: vec![
                home_page(),
                PageDefinition {
                    id: "fixture/broken_page".to_string(),
                    required: vec!["fixture/missing".to_string()],
                    any_of: Vec::new(),
                    optional: Vec::new(),
                    forbidden: Vec::new(),
                },
                settings_page(),
            ],
            ..base_page_set()
        })
        .expect("detector");

        let outcomes = detector
            .evaluate_all_outcomes(&fixture.evaluator, &scene_colors(true, true, false))
            .expect("page failures do not terminate the batch");

        assert_eq!(outcomes.len(), 3);
        assert_eq!(
            outcomes
                .iter()
                .map(|outcome| (outcome.index, outcome.page_id.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (0, "fixture/home_page"),
                (1, "fixture/broken_page"),
                (2, "fixture/settings_page"),
            ]
        );
        assert!(outcomes[0].result.as_ref().expect("home result").matched);
        assert!(outcomes[1].result.is_err());
        assert!(
            outcomes[2]
                .result
                .as_ref()
                .expect("settings result")
                .matched
        );

        let error = require_all_page_evaluations(outcomes).expect_err("partial batch");
        assert_fatal_contains(error, "1 failed page(s)");

        let compatibility_error = detector
            .evaluate_all(&fixture.evaluator, &scene_colors(true, true, false))
            .expect_err("compatibility entry point remains fail-loud");
        assert_fatal_contains(compatibility_error, "1 failed page(s)");
    }

    #[test]
    fn evaluate_all_reports_batch_validation_before_any_page_runs() {
        let fixture = Fixture::new();
        let error = fixture
            .detector
            .evaluate_all_outcomes(
                &fixture.evaluator,
                &scene_from_colors(12, 8, true, true, false),
            )
            .expect_err("coordinate mismatch terminates the batch");

        assert!(error.completed.is_empty());
        assert_eq!(
            error
                .unexecuted
                .iter()
                .map(|page| (page.index, page.page_id.as_str(), page.reason))
                .collect::<Vec<_>>(),
            vec![
                (
                    0,
                    "fixture/home_page",
                    PageUnexecutedReason::BatchValidationFailed,
                ),
                (
                    1,
                    "fixture/settings_page",
                    PageUnexecutedReason::BatchValidationFailed,
                ),
            ]
        );
        assert_fatal_contains(error.cause, "coordinate_space");
    }

    #[test]
    fn evaluate_all_preserves_completed_and_unexecuted_on_mid_batch_invariant_failure() {
        let fixture = Fixture::new();
        let detector = PageDetector::new(PageSet {
            pages: vec![
                home_page(),
                settings_page(),
                PageDefinition {
                    id: "fixture/final_page".to_string(),
                    required: vec!["fixture/home_anchor".to_string()],
                    any_of: Vec::new(),
                    optional: Vec::new(),
                    forbidden: Vec::new(),
                },
            ],
            ..base_page_set()
        })
        .expect("detector");
        let scene = scene_colors(true, true, false);

        let error = detector
            .evaluate_all_outcomes_with(&fixture.evaluator, &scene, |index, page| {
                let mut evaluation =
                    detector.evaluate_page_definition(&fixture.evaluator, &scene, page)?;
                if index == 1 {
                    evaluation.page_id = "fixture/wrong_page".to_string();
                }
                Ok(evaluation)
            })
            .expect_err("invariant failure terminates the batch");

        assert_eq!(error.completed.len(), 2);
        assert_eq!(error.completed[0].index, 0);
        assert!(error.completed[0].result.is_ok());
        assert_eq!(error.completed[1].index, 1);
        assert_eq!(error.completed[1].page_id, "fixture/settings_page");
        let attempted_error = error.completed[1]
            .result
            .as_ref()
            .expect_err("attempted invariant failure is preserved");
        assert_fatal_contains(
            attempted_error.clone(),
            "expected page_id 'fixture/settings_page'",
        );
        assert_eq!(
            error
                .unexecuted
                .iter()
                .map(|page| (page.index, page.page_id.as_str(), page.reason))
                .collect::<Vec<_>>(),
            vec![(
                2,
                "fixture/final_page",
                PageUnexecutedReason::BatchTerminated,
            ),]
        );
        assert_fatal_contains(error.cause, "invariant failed");
    }

    #[test]
    fn evaluate_page_does_not_evaluate_unrequested_page_targets() {
        let root = temp_fixture_dir("single-page-only");
        fs::write(
            root.join("too-large.png"),
            encode_png(32, 32, |_, _| [255, 255, 255]),
        )
        .expect("write template");
        let mut pack = fixture_pack();
        pack.targets
            .push(RecognitionTarget::Template(TemplateTarget {
                id: "fixture/too_large_template".to_string(),
                template_path: "too-large.png".to_string(),
                region: PackRegion::Keyword("full_frame".to_string()),
                threshold: Some(0.9),
                method: actingcommand_recognition_pack::RecognitionMethod::Ncc,
                mask: None,
                rect_move: None,
                color_check: None,
                click: None,
            }));
        let evaluator = RecognitionEvaluator::new(root.clone(), pack).expect("evaluator");
        let detector = PageDetector::new(PageSet {
            pages: vec![
                home_page(),
                PageDefinition {
                    id: "fixture/unrequested_page".to_string(),
                    required: vec!["fixture/too_large_template".to_string()],
                    any_of: Vec::new(),
                    optional: Vec::new(),
                    forbidden: Vec::new(),
                },
            ],
            ..base_page_set()
        })
        .expect("detector");

        let evaluation = detector
            .evaluate_page(
                &evaluator,
                &scene_colors(true, false, false),
                "fixture/home_page",
            )
            .expect("evaluate requested page only");

        assert!(evaluation.matched);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn coordinate_mismatch_is_fatal() {
        let fixture = Fixture::new();
        let err = fixture
            .detector
            .evaluate_page(
                &fixture.evaluator,
                &scene_from_colors(12, 8, true, false, false),
                "fixture/home_page",
            )
            .expect_err("coordinate mismatch");

        assert_fatal_contains(err, "coordinate_space");
    }

    fn assert_click_only_role_is_fatal(role: PageTargetRole) {
        let fixture = Fixture::new();
        let mut page = PageDefinition {
            id: "fixture/click_page".to_string(),
            required: vec!["fixture/home_anchor".to_string()],
            any_of: Vec::new(),
            optional: Vec::new(),
            forbidden: Vec::new(),
        };

        match role {
            PageTargetRole::Required => page.required = vec!["fixture/click_only".to_string()],
            PageTargetRole::AnyOf => page.any_of = vec![vec!["fixture/click_only".to_string()]],
            PageTargetRole::Optional => page.optional = vec!["fixture/click_only".to_string()],
            PageTargetRole::Forbidden => page.forbidden = vec!["fixture/click_only".to_string()],
        }

        let detector = PageDetector::new(PageSet {
            pages: vec![page],
            ..base_page_set()
        })
        .expect("detector");

        let err = detector
            .validate(&fixture.evaluator)
            .expect_err("click-only evidence");

        assert_fatal_contains(err, "click-only target");
    }

    struct Fixture {
        root: PathBuf,
        evaluator: RecognitionEvaluator,
        detector: PageDetector,
    }

    impl Fixture {
        fn new() -> Self {
            let root = temp_fixture_dir("page-detector");
            let evaluator = RecognitionEvaluator::new(root.clone(), fixture_pack()).expect("eval");
            let detector = PageDetector::new(PageSet {
                pages: vec![home_page(), settings_page()],
                ..base_page_set()
            })
            .expect("detector");

            Self {
                root,
                evaluator,
                detector,
            }
        }

        fn evaluate_home_page(&self, scene: Scene) -> PageEvaluation {
            self.detector
                .evaluate_page(&self.evaluator, &scene, "fixture/home_page")
                .expect("evaluate page")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn base_page_set() -> PageSet {
        PageSet {
            schema_version: "0.1".to_string(),
            pages: Vec::new(),
        }
    }

    fn home_page_set() -> PageSet {
        PageSet {
            pages: vec![home_page()],
            ..base_page_set()
        }
    }

    fn home_page() -> PageDefinition {
        PageDefinition {
            id: "fixture/home_page".to_string(),
            required: vec!["fixture/home_anchor".to_string()],
            any_of: Vec::new(),
            optional: vec!["fixture/settings_anchor".to_string()],
            forbidden: vec!["fixture/forbidden_popup".to_string()],
        }
    }

    fn settings_page() -> PageDefinition {
        PageDefinition {
            id: "fixture/settings_page".to_string(),
            required: vec!["fixture/settings_anchor".to_string()],
            any_of: Vec::new(),
            optional: Vec::new(),
            forbidden: vec!["fixture/forbidden_popup".to_string()],
        }
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
                    id: "fixture/settings_anchor".to_string(),
                    region: rect(4, 0, 4, 4),
                    expected: [0, 255, 0],
                    click: None,
                }),
                RecognitionTarget::Color(ColorTarget {
                    id: "fixture/forbidden_popup".to_string(),
                    region: rect(0, 4, 4, 4),
                    expected: [0, 0, 255],
                    click: None,
                }),
                RecognitionTarget::ClickOnly(ClickOnlyTarget {
                    id: "fixture/click_only".to_string(),
                    click: rect(1, 2, 3, 4),
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

    fn scene_colors(home: bool, settings: bool, forbidden: bool) -> Scene {
        scene_from_colors(16, 16, home, settings, forbidden)
    }

    fn scene_from_colors(
        width: u32,
        height: u32,
        home: bool,
        settings: bool,
        forbidden: bool,
    ) -> Scene {
        let png = encode_png(width, height, |x, y| {
            if home && x < 4 && y < 4 {
                [255, 0, 0]
            } else if settings && (4..8).contains(&x) && y < 4 {
                [0, 255, 0]
            } else if forbidden && x < 4 && (4..8).contains(&y) {
                [0, 0, 255]
            } else {
                [0, 0, 0]
            }
        });
        Scene::from_png(&png).expect("scene")
    }

    fn temp_fixture_dir(label: &str) -> PathBuf {
        let index = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "actingcommand-page-detector-{label}-{}-{index}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    fn assert_fatal_contains(err: PageDetectorError, expected: &str) {
        assert_eq!(err.severity(), PageDetectorErrorSeverity::Fatal);
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
