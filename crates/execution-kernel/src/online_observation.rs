// SPDX-License-Identifier: AGPL-3.0-only

//! One contained, read-only observation over the existing recognition owners.

use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use crate::{
    DriveNavigationClick, DriveSemanticInput, ExternallyVerifiedBundle, drive_rect_center,
};
use actingcommand_contract::codes::{ContractLocation, PageProjectionCode};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::page_projection::{
    self as projection, ActionKey, ElementInput, ElementResolution, ElementRole, FieldInput,
    FrameIdentity, Geometry, MissingTarget, NotEvaluatedReason, PageProjection, Privacy,
    ProjectionInput,
};
use actingcommand_contract::{
    ObservationFacts, PageObservationStatus, PpocrDiagnostics, RuntimeContractResult,
};
use actingcommand_page_detector::{PageTargetEvaluation, PageTargetRole};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    CandidateProjectionFailure, PackRect, RecognitionEvaluator, RecognitionTarget,
    TargetEvaluation, TargetKind, VisionProvider,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct OnlineObservationError {
    code: Code,
    stage: Location,
    cause: String,
    ppocr_diagnostics: PpocrDiagnostics,
}
impl std::fmt::Debug for OnlineObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl OnlineObservationError {
    fn new(code: Code, stage: Location, cause: impl ToString) -> Self {
        Self {
            code,
            stage,
            cause: cause.to_string(),
            ppocr_diagnostics: Vec::new(),
        }
    }
    pub const fn code(&self) -> Code {
        self.code
    }
    pub const fn stage(&self) -> Location {
        self.stage
    }
    pub fn cause(&self) -> &str {
        &self.cause
    }

    pub fn ppocr_diagnostics(&self) -> &PpocrDiagnostics {
        &self.ppocr_diagnostics
    }

    pub fn with_ppocr_diagnostics(mut self, diagnostics: PpocrDiagnostics) -> Self {
        self.ppocr_diagnostics.extend(diagnostics);
        self
    }
}
impl std::fmt::Display for OnlineObservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} during {}", self.code, self.stage)
    }
}
impl std::error::Error for OnlineObservationError {}

struct ObservationElement {
    action: ActionKey,
    purpose: String,
    source: String,
    input: DriveNavigationClick,
}

pub struct PreparedPageObservation {
    bundle: ExternallyVerifiedBundle,
    elements: Vec<ObservationElement>,
    targets: Vec<String>,
}

pub struct EvaluatedPageObservation {
    pub projection: PageProjection,
    pub status: PageObservationStatus,
    pub rgb8_sha256: String,
    pub facts: ObservationFacts,
    pub private_facts: ObservationFacts,
    pub ppocr_diagnostics: PpocrDiagnostics,
}

impl PreparedPageObservation {
    /// Workflow #336 PKG-S1: admits the package `locator` names, the loader chosen by the path
    /// (since PKG-S5 no reference is taken).
    pub fn load_path(
        instance: &str,
        locator: &std::path::Path,
        targets: &[String],
        provider: Option<Arc<dyn VisionProvider>>,
        deadline: std::time::Instant,
    ) -> Result<Self, OnlineObservationError> {
        let bundle =
            ExternallyVerifiedBundle::load_path(instance, locator, true, provider, deadline)
                .map_err(|error| {
                    OnlineObservationError::new(
                        ExecutionKernelCode::ObservationContainmentFailed.code(),
                        ContractLocation::AdmitContainedObservation.location(),
                        error,
                    )
                })?;
        Self::from_bundle(bundle, targets)
    }

    pub fn load(
        instance: &str,
        bytes: &[u8],
        targets: &[String],
        provider: Option<Arc<dyn VisionProvider>>,
    ) -> Result<Self, OnlineObservationError> {
        let bundle = ExternallyVerifiedBundle::load_observation(instance, bytes, provider)
            .map_err(|error| {
                OnlineObservationError::new(
                    ExecutionKernelCode::ObservationContainmentFailed.code(),
                    ContractLocation::AdmitContainedObservation.location(),
                    error,
                )
            })?;
        Self::from_bundle(bundle, targets)
    }

    fn from_bundle(
        bundle: ExternallyVerifiedBundle,
        targets: &[String],
    ) -> Result<Self, OnlineObservationError> {
        let loaded = bundle.loaded_bundle();
        let invalid = |cause: &str| {
            OnlineObservationError::new(
                ExecutionKernelCode::ObservationResourcesInvalid.code(),
                ExecutionKernelLocation::LoadObservationResources.location(),
                cause,
            )
        };
        let metadata = loaded
            .projection_metadata()
            .ok_or_else(|| invalid("verified catalog missing"))?;
        let evaluator = loaded
            .evaluator()
            .ok_or_else(|| invalid("recognition pack missing"))?;
        let detector = loaded.detector().ok_or_else(|| invalid("pages missing"))?;
        detector
            .validate(evaluator)
            .map_err(|error| invalid(error.message()))?;
        evaluator
            .pack()
            .validate_candidate_layout_pages(|page_id| detector.contains_page(page_id))
            .map_err(|error| invalid(error.message()))?;
        for target in targets {
            if evaluator
                .target_kind(target)
                .map_err(|error| invalid(error.message()))?
                == TargetKind::ClickOnly
            {
                return Err(invalid("explicit target is not evaluable"));
            }
        }
        let navigation = loaded
            .navigation()
            .ok_or_else(|| invalid("navigation missing"))?;
        let mut elements = Vec::new();
        for action in &metadata.catalog().actions {
            let (array, id) = match action.role {
                ElementRole::Navigate => ("navigation", "id"),
                ElementRole::PageOp => ("page_operations", "id"),
                ElementRole::ControlPoint => ("control_points", "name"),
            };
            let row = navigation[array]
                .as_array()
                .and_then(|rows| {
                    rows.iter().find(|row| {
                        row[id].as_str() == Some(&action.resource_id)
                            && (action.role != ElementRole::PageOp
                                || row["task_id"].as_str() == Some(&action.task_id))
                            && (action.role == ElementRole::ControlPoint
                                || row[if action.role == ElementRole::Navigate {
                                    "from_page"
                                } else {
                                    "page"
                                }]
                                .as_str()
                                    == action.page.as_deref())
                    })
                })
                .ok_or_else(|| invalid("catalog action source missing"))?;
            // #411 Lab L2: a navigation edge's or page operation's click may be a segmented drag;
            // a control point's is a tap, as before.
            let input = match (row.get("click"), action.role) {
                (Some(click), ElementRole::ControlPoint) => DriveNavigationClick::Input(
                    crate::drive::parse_navigation_input(click)
                        .map_err(|error| invalid(error.message()))?,
                ),
                (Some(click), _) => crate::drive::parse_navigation_click(click)
                    .map_err(|error| invalid(error.message()))?,
                (None, ElementRole::ControlPoint) => {
                    let rect = crate::drive::parse_control_point_rect(row)
                        .map_err(|error| invalid(error.message()))?;
                    DriveNavigationClick::Input(DriveSemanticInput::Tap {
                        rect,
                        point: drive_rect_center(rect).map_err(|error| invalid(error.message()))?,
                    })
                }
                (None, _) => return Err(invalid("action input missing")),
            };
            match &input {
                DriveNavigationClick::Input(
                    DriveSemanticInput::TargetCenter { target_id }
                    | DriveSemanticInput::TargetOffset { target_id, .. },
                ) => {
                    if !metadata.catalog().targets.contains(target_id) {
                        return Err(invalid("action target is not declared"));
                    }
                }
                DriveNavigationClick::Input(input) => geometry(input)?
                    .validate(metadata.catalog().width, metadata.catalog().height)
                    .map_err(|_| invalid("declared action geometry invalid"))?,
                // #411 Lab L2: its two declared boxes lie in the coordinate space, as a drag's do.
                DriveNavigationClick::SegmentedDrag(drag) => {
                    for rect in [drag.from_rect, drag.corner_rect] {
                        geometry(&DriveSemanticInput::Tap {
                            rect,
                            point: drive_rect_center(rect)
                                .map_err(|error| invalid(error.message()))?,
                        })?
                        .validate(metadata.catalog().width, metadata.catalog().height)
                        .map_err(|_| invalid("declared action geometry invalid"))?;
                    }
                }
            }
            elements.push(ObservationElement {
                action: action.clone(),
                purpose: row["purpose"]
                    .as_str()
                    .unwrap_or(&action.resource_id)
                    .to_string(),
                source: row["source"].as_str().unwrap_or(array).to_string(),
                input,
            });
        }
        Ok(Self {
            bundle,
            elements,
            targets: targets.to_vec(),
        })
    }

    /// Workflow #336 PKG-S2 (option 丙): the package's `package_id` as its control declares
    /// it, `None` when the control declares none; the evidence's `package_label`.
    pub fn package_label(&self) -> Option<String> {
        self.bundle
            .loaded_bundle()
            .control()
            .and_then(|control| control.get("package_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }

    pub fn contains_page(&self, page_id: &str) -> bool {
        self.bundle
            .loaded_bundle()
            .projection_metadata()
            .expect("admitted metadata")
            .catalog()
            .pages
            .contains(page_id)
    }

    pub fn evaluate(
        &self,
        png: &[u8],
        frame: FrameIdentity,
    ) -> Result<EvaluatedPageObservation, OnlineObservationError> {
        let loaded = self.bundle.loaded_bundle();
        let metadata = loaded.projection_metadata().expect("admitted metadata");
        let evaluator = loaded.evaluator().expect("admitted evaluator");
        let detector = loaded.detector().expect("admitted detector");
        let scene = Scene::from_png(png).map_err(|error| {
            OnlineObservationError::new(
                ExecutionKernelCode::ObservationFrameInvalid.code(),
                ExecutionKernelLocation::DecodeObservationFrame.location(),
                error,
            )
        })?;
        let sample_facts = std::cell::RefCell::new(ObservationFacts::default());
        let sample_reports = std::cell::RefCell::new(PpocrDiagnostics::new());
        let mut sample_recorder =
            |target: &str,
             result: &actingcommand_recognition_pack::RecognitionPackResult<TargetEvaluation>,
             _started: std::time::Instant,
             _ended: std::time::Instant| {
                let reports = match result {
                    Ok(value) => value.ppocr_diagnostics(),
                    Err(error) => error.ppocr_diagnostics(),
                };
                sample_reports.borrow_mut().extend_from_slice(reports);
                let row = json!({"kind":"recognition_sample","target_id":target,"result":result});
                let mut facts = sample_facts.borrow_mut();
                facts.push(row, 1).map_err(|error| {
                    actingcommand_recognition_pack::RecognitionPackError::fatal(error.to_string())
                })?;
                if facts.omitted_count > 0 {
                    return Err(actingcommand_recognition_pack::RecognitionPackError::fatal(
                        "recognition sample evidence exceeds observation budget",
                    ));
                }
                Ok(())
            };
        let context = evaluator
            .scene_context(&scene)
            .with_sample_recorder(&mut sample_recorder);
        let (pages, batch_error) = match detector.evaluate_all_outcomes_in_context(&context) {
            Ok(pages) => (pages, None),
            Err(error) => (error.completed.clone(), Some(error)),
        };
        // Reports travel separately from the bounded, serialized observation facts.
        let mut ppocr_diagnostics: PpocrDiagnostics = match &batch_error {
            Some(error) => error.ppocr_diagnostics(),
            None => pages
                .iter()
                .flat_map(|page| match &page.result {
                    Ok(value) => value.ppocr_diagnostics(),
                    Err(error) => error.ppocr_diagnostics(),
                })
                .collect(),
        };
        let mut complete = batch_error.is_none() && pages.iter().all(|page| page.result.is_ok());
        // #411 A21: both collections admit the required evidence first, then the rows of the
        // explicitly requested targets, then the others ([`ObservationFactRows`]).
        let mut rows = ObservationFactRows::default();
        // The deferred `target_evaluation` row of each actual page target evaluation, by its
        // evaluation index; an explicit target that reuses the evaluation requests the row.
        let mut evaluation_rows = Vec::new();
        let consensus = !evaluator.pack().target_consensus.is_empty();
        if consensus {
            let supported = evaluator.target_sample_frames() == 1;
            complete &= supported;
            let row = json!({"kind":"recognition_coverage","provided_frames":1,"required_frames":evaluator.target_sample_frames(),"aggregation":if supported { "production_owner" } else { "not_completed" }});
            if !rows.require(row.clone(), row, 0).map_err(fact_error)? {
                return Err(fact_error(
                    "recognition sample evidence exceeds observation budget",
                ));
            }
        }
        let mut actual: Vec<(Option<String>, PageTargetEvaluation)> = Vec::new();
        let mut matched_pages = Vec::new();
        for page in &pages {
            let values = match &page.result {
                Ok(value) => {
                    if value.matched {
                        matched_pages.push(value.page_id.clone());
                    }
                    &value.target_results
                }
                Err(error) => &error.completed_targets,
            };
            actual.extend(
                values
                    .iter()
                    .cloned()
                    .map(|value| (Some(page.page_id.clone()), value)),
            );
            let mut summary = serde_json::to_value(page).map_err(|error| {
                fact_error(error).with_ppocr_diagnostics(ppocr_diagnostics.clone())
            })?;
            for branch in ["Ok", "Err"] {
                if let Some(result) = summary["result"][branch].as_object_mut() {
                    result.remove("target_results");
                    result.remove("completed_targets");
                }
            }
            let row = json!({"kind":"page_evaluation", "stage":ExecutionKernelLocation::EvaluatePage.as_str(), "outcome":summary, "target_evaluation_count":values.len()});
            let public = redact_row(row.clone(), metadata);
            rows.defer(DeferredTier::Other, row, public, 0);
            for (index, value) in values.iter().enumerate() {
                let row = json!({"kind":"target_evaluation", "stage":ExecutionKernelLocation::EvaluatePageTarget.as_str(), "page_id":page.page_id, "page_index":page.index,
                    "evaluation_index":actual.len() - values.len() + index, "target":value});
                let public = redact_row(row.clone(), metadata);
                evaluation_rows.push(rows.defer(DeferredTier::Other, row, public, 1));
            }
            if let Err(error) = &page.result {
                let definition = &detector.page_definitions()[page.index];
                let mut declared = Vec::new();
                let mut groups = vec![(PageTargetRole::Required, None, &definition.required)];
                groups.extend(
                    definition
                        .any_of
                        .iter()
                        .enumerate()
                        .map(|(index, targets)| (PageTargetRole::AnyOf, Some(index), targets)),
                );
                groups.push((PageTargetRole::Optional, None, &definition.optional));
                groups.push((PageTargetRole::Forbidden, None, &definition.forbidden));
                for (role, group, targets) in groups {
                    for (index, target) in targets.iter().enumerate() {
                        declared.push((role, group, index, target));
                    }
                }
                let attempted = values.len() + usize::from(error.failed_target.is_some());
                for (role, group, index, target) in declared.into_iter().skip(attempted) {
                    let row = json!({"kind":PageProjectionCode::TargetNotEvaluated.as_str(), "stage":ExecutionKernelLocation::EvaluatePageTarget.as_str(), "page_id":page.page_id,
                        "page_index":page.index, "target_id":target, "role":role, "group_index":group, "target_index":index,
                        "state":"not_evaluated", "reason":ExecutionKernelCode::EarlierTargetFailed.as_str()});
                    rows.defer(DeferredTier::Other, row.clone(), row, 0);
                }
            }
        }
        if let Some(error) = batch_error {
            let row = json!({"kind":"page_batch_failure", "stage":ExecutionKernelLocation::EvaluatePages.as_str(), "cause":error.cause, "unexecuted":error.unexecuted});
            let public = redact_row(row.clone(), metadata);
            rows.defer(DeferredTier::Other, row, public, 0);
        }
        for target in &self.targets {
            let uses = actual
                .iter()
                .enumerate()
                .filter(|(_, (_, value))| value.target_id == *target)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if !uses.is_empty() {
                for evaluation in &uses {
                    if let Some(deferred) = evaluation_rows.get(*evaluation) {
                        rows.request(*deferred);
                    }
                }
                let row = json!({"kind":"explicit_target_uses", "target_id":target, "evaluation_uses":uses});
                rows.defer(DeferredTier::Requested, row.clone(), row, 0);
                continue;
            }
            // A target which failed in this frame is not called again for projection.
            if pages.iter().any(|page| {
                page.result
                    .as_ref()
                    .err()
                    .and_then(|error| error.failed_target.as_ref())
                    .is_some_and(|failed| failed.target_id == *target)
            }) {
                let row = json!({"kind":"explicit_target_uses_failure", "target_id":target});
                rows.defer(DeferredTier::Requested, row.clone(), row, 0);
                continue;
            }
            match context.evaluate_target(target) {
                Ok(value) => {
                    ppocr_diagnostics.extend(value.ppocr_diagnostics().iter().cloned());
                    let evaluated = PageTargetEvaluation {
                        target_id: target.clone(),
                        role: PageTargetRole::Optional,
                        passed: value.passed,
                        message: value.message.clone(),
                        group_index: None,
                        target_index: 0,
                        evaluation: value,
                        skipped: None,
                    };
                    let row = json!({"kind":"explicit_target", "stage":ExecutionKernelLocation::EvaluateRequestedTarget.as_str(), "target":evaluated});
                    let public = redact_row(row.clone(), metadata);
                    rows.defer(DeferredTier::Requested, row, public, 1);
                    actual.push((None, evaluated));
                }
                Err(error) => {
                    ppocr_diagnostics.extend(error.ppocr_diagnostics().iter().cloned());
                    complete = false;
                    let row = json!({"kind":"target_failure", "stage":ExecutionKernelLocation::EvaluateRequestedTarget.as_str(), "target_id":target, "cause":error});
                    let public = redact_row(row.clone(), metadata);
                    rows.defer(DeferredTier::Requested, row, public, 0);
                }
            }
        }
        if complete && matched_pages.len() == 1 {
            let mut public_sets = Vec::new();
            for layout in evaluator
                .pack()
                .candidate_layouts
                .iter()
                .filter(|layout| layout.page_id == matched_pages[0])
            {
                let failure_row = |error: &CandidateProjectionFailure| json!({"kind":"candidate_projection_failure", "layout_id":layout.id, "code":error.code().as_str(), "item":error.item(), "coverage":{"provided_frames":1,"required_frames":layout.required_frames(),"aggregation":"not_completed"}});
                // #411 A21: a sample is admitted only while the row this projection ends with when
                // a sample does not fit still fits after it, so such a projection is recorded.
                let reserve = failure_row(&CandidateProjectionFailure::recording_failed(""));
                let projected = context.project_candidates_with_samples(&layout.id, &[&scene], &mut |target, result, started, ended| {
                    match result {
                        Ok(value) => ppocr_diagnostics.extend(value.ppocr_diagnostics().iter().cloned()),
                        Err(error) => ppocr_diagnostics.extend(error.ppocr_diagnostics().iter().cloned()),
                    }
                    let row = json!({"kind":"candidate_sample", "target_id":target, "evaluation":result, "elapsed_us":crate::observe_instant_span(started, ended)});
                    let public = redact_row(row.clone(), metadata);
                    if !rows.require_before(row, public, 1, &reserve).map_err(|error| CandidateProjectionFailure::recording_failed(error.to_string()))? {
                        return Err(CandidateProjectionFailure::recording_failed("candidate sample evidence exceeds observation budget"));
                    }
                    Ok(())
                });
                let (private, public) = match projected {
                    Ok(projected) => {
                        let (public, _) = projected
                            .split(
                                &layout.candidate_privacy(metadata, projected.candidates().len()),
                            )
                            .map_err(fact_error)?;
                        public_sets.push(public.clone());
                        actingcommand_contract::validate_candidate_sets_budget(&public_sets)
                            .map_err(fact_error)?;
                        (
                            json!({"kind":"candidate_projection", "projection":projected}),
                            json!({"kind":"candidate_projection", "projection":public}),
                        )
                    }
                    Err(error) => {
                        complete = false;
                        let row = failure_row(&error);
                        (row.clone(), row)
                    }
                };
                if !rows.require(private, public, 0).map_err(fact_error)? {
                    return Err(fact_error(
                        "candidate projection evidence exceeds observation budget",
                    )
                    .with_ppocr_diagnostics(ppocr_diagnostics));
                }
            }
        }
        ppocr_diagnostics.extend(sample_reports.borrow().iter().cloned());
        // With target consensus the recognition samples are required evidence; without it no
        // recognition requires them.
        let mut samples_fit = sample_facts.borrow().omitted_count == 0;
        for row in &sample_facts.borrow().rows {
            let public = redact_row(row.clone(), metadata);
            if consensus {
                samples_fit &= rows.require(row.clone(), public, 1).map_err(fact_error)?;
            } else {
                rows.defer(DeferredTier::Other, row.clone(), public, 1);
            }
        }
        if consensus && !samples_fit {
            return Err(
                fact_error("recognition sample evidence exceeds observation budget")
                    .with_ppocr_diagnostics(ppocr_diagnostics),
            );
        }
        let (private_facts, facts) = rows
            .finish()
            .map_err(|error| fact_error(error).with_ppocr_diagnostics(ppocr_diagnostics.clone()))?;
        let status = if !complete {
            PageObservationStatus::Partial
        } else {
            match matched_pages.len() {
                0 => PageObservationStatus::NoMatch,
                1 => PageObservationStatus::Recognized,
                _ => PageObservationStatus::Conflict,
            }
        };
        let mut elements = Vec::new();
        let selected_page =
            (complete && matched_pages.len() == 1).then(|| matched_pages[0].as_str());
        for element in &self.elements {
            if element
                .action
                .page
                .as_deref()
                .is_some_and(|page| Some(page) != selected_page)
            {
                continue;
            }
            let resolution = match &element.input {
                // #411 Lab L2: a segmented drag is listed from the page declaration with no input.
                DriveNavigationClick::SegmentedDrag(_) => ElementResolution::GestureNotProjectable,
                DriveNavigationClick::Input(input) => match input {
                    DriveSemanticInput::TargetCenter { target_id }
                    | DriveSemanticInput::TargetOffset { target_id, .. } => {
                        let values = actual
                            .iter()
                            .filter(|(page, value)| {
                                value.target_id == *target_id
                                    && (page.as_deref() == selected_page || page.is_none())
                            })
                            .map(|(_, value)| &value.evaluation)
                            .collect::<Vec<_>>();
                        match values.as_slice() {
                            [value] if element.action.role != ElementRole::ControlPoint => {
                                let geometry = match input {
                                    DriveSemanticInput::TargetOffset { offset, .. } => {
                                        offset_target_geometry(value, *offset)
                                    }
                                    _ => target_geometry(value, evaluator),
                                };
                                ElementResolution::Target {
                                    target_id: target_id.clone(),
                                    passed: value.passed,
                                    geometry: geometry.map_err(|error| {
                                        error.with_ppocr_diagnostics(ppocr_diagnostics.clone())
                                    })?,
                                }
                            }
                            [_, _, ..] => ElementResolution::Ambiguous {
                                target_id: target_id.clone(),
                            },
                            _ => ElementResolution::NotEvaluated {
                                target_id: target_id.clone(),
                                reason: NotEvaluatedReason::Unscoped,
                            },
                        }
                    }
                    input => ElementResolution::Declared(geometry(input).map_err(|error| {
                        error.with_ppocr_diagnostics(ppocr_diagnostics.clone())
                    })?),
                },
            };
            elements.push(ElementInput {
                action: element.action.clone(),
                purpose: element.purpose.clone(),
                source: element.source.clone(),
                resolution,
            });
        }
        let mut missing = Vec::new();
        for page in pages
            .iter()
            .filter_map(|page| page.result.as_ref().ok())
            .filter(|page| Some(page.page_id.as_str()) == selected_page)
        {
            for target in &page.target_results {
                missing.push(MissingTarget {
                    passed: target.passed,
                    id: target.target_id.clone(),
                    role: role_name(target.role).to_string(),
                    group_index: target.group_index,
                    group_satisfied: target.group_index.map(|index| {
                        page.target_results.iter().any(|value| {
                            value.role == PageTargetRole::AnyOf
                                && value.group_index == Some(index)
                                && value.passed
                        })
                    }),
                });
            }
        }
        let mut fields = Vec::new();
        for target in &metadata.catalog().targets {
            let values = actual.iter().enumerate().filter(|(_, (_, value))| value.target_id == *target)
                .map(|(index, (page, value))| json!({"evaluation_index":index,"page":page,"group_index":value.group_index,"target_index":value.target_index,"role":value.role,"evaluation":value.evaluation})).collect::<Vec<_>>();
            if values.is_empty() {
                continue;
            }
            let input = FieldInput {
                target_id: target.clone(),
                field: None,
                parsed: false,
                raw_text: None,
                value: Some(json!({"evaluations":values})),
                detail: None,
                privacy: metadata.target_privacy(target),
            };
            fields.push(input.clone());
            for field in metadata
                .catalog()
                .fields
                .iter()
                .filter(|field| field.target_id == *target)
            {
                let mut bound = input.clone();
                bound.field = Some(field.clone());
                bound.privacy = metadata.field_privacy(field);
                fields.push(bound);
            }
        }
        let projection = projection::project_observation(
            ProjectionInput {
                frame,
                matched_pages,
                elements,
                missing,
                fields,
            },
            metadata,
            complete,
        )
        .map_err(|error| {
            OnlineObservationError::new(
                ExecutionKernelCode::ObservationProjectionFailed.code(),
                ExecutionKernelLocation::ProjectObservation.location(),
                error,
            )
            .with_ppocr_diagnostics(ppocr_diagnostics.clone())
        })?;
        Ok(EvaluatedPageObservation {
            projection,
            status,
            rgb8_sha256: format!("{:x}", Sha256::digest(scene.rgb8_pixels())),
            facts,
            private_facts,
            ppocr_diagnostics,
        })
    }
}

fn fact_error(error: impl ToString) -> OnlineObservationError {
    OnlineObservationError::new(
        ExecutionKernelCode::ObservationFactFailed.code(),
        ExecutionKernelLocation::EncodeObservationFact.location(),
        error,
    )
}

/// #411 A21: the tier of a row that waits for the required evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DeferredTier {
    /// A row of an explicitly requested target, or a page target evaluation such a target
    /// reuses.
    Requested,
    /// Every other row no recognition requires.
    Other,
}

/// #411 A21: one fact collection whose rows are admitted out of production order and answered
/// in it.
#[derive(Default)]
struct TieredFacts {
    /// The admitted rows, in admission order, with the collection's counts.
    facts: ObservationFacts,
    /// The production position of each admitted row.
    positions: Vec<usize>,
}

impl TieredFacts {
    /// Tries `row` as [`ObservationFacts::push`] does; whether it was admitted.
    fn admit(
        &mut self,
        position: usize,
        row: Value,
        target_count: usize,
    ) -> RuntimeContractResult<bool> {
        let admitted = self.facts.rows.len();
        self.facts.push(row, target_count)?;
        let kept = self.facts.rows.len() > admitted;
        if kept {
            self.positions.push(position);
        }
        Ok(kept)
    }

    /// Whether `row` fits and `reserve` still fits after it once one more row of
    /// `target_count` evaluations was counted as omitted.
    fn fits_before(
        &self,
        row: &Value,
        target_count: usize,
        reserve: &Value,
    ) -> RuntimeContractResult<bool> {
        let mut trial = self.facts.clone();
        let admitted = trial.rows.len();
        trial.push(row.clone(), target_count)?;
        trial.omit(target_count);
        trial.push(reserve.clone(), 0)?;
        Ok(trial.rows.len() == admitted + 2)
    }

    /// The collection, its admitted rows in production order.
    fn finish(self) -> ObservationFacts {
        let Self { facts, positions } = self;
        let ObservationFacts {
            rows,
            item_count,
            omitted_count,
            target_evaluation_count,
            omitted_target_evaluation_count,
            truncated,
        } = facts;
        let mut ordered = positions.into_iter().zip(rows).collect::<Vec<_>>();
        ordered.sort_by_key(|(position, _)| *position);
        ObservationFacts {
            rows: ordered.into_iter().map(|(_, row)| row).collect(),
            item_count,
            omitted_count,
            target_evaluation_count,
            omitted_target_evaluation_count,
            truncated,
        }
    }
}

/// A row that waits for the required evidence, with its controlled and public forms.
struct DeferredFact {
    tier: DeferredTier,
    position: usize,
    private: Value,
    public: Value,
    target_count: usize,
}

/// #411 A21: the controlled (`private_facts`) and public (`facts`) collections of one
/// observation. A row a recognition requires (the matched page's candidate evidence and, with
/// target consensus, the recognition samples and their coverage) is admitted when it is
/// produced, ahead of every other row; the other rows wait, and are admitted after it: first
/// those of the explicitly requested targets, then the rest, each in production order, while
/// each still fits. Both collections keep their admitted rows in production order and count
/// the omitted ones as [`ObservationFacts::push`] does, so when every row fits they are those
/// of a collection filled in production order.
#[derive(Default)]
struct ObservationFactRows {
    private: TieredFacts,
    public: TieredFacts,
    deferred: Vec<DeferredFact>,
    produced: usize,
}

impl ObservationFactRows {
    fn next_position(&mut self) -> usize {
        self.produced += 1;
        self.produced - 1
    }

    /// A required row; whether both collections hold it.
    fn require(
        &mut self,
        private: Value,
        public: Value,
        target_count: usize,
    ) -> RuntimeContractResult<bool> {
        let position = self.next_position();
        let private = self.private.admit(position, private, target_count)?;
        let public = self.public.admit(position, public, target_count)?;
        Ok(private && public)
    }

    /// A required row that is admitted only while `reserve`, a later required row, still fits
    /// after it in both collections; otherwise both count it as omitted. Whether it was
    /// admitted.
    fn require_before(
        &mut self,
        private: Value,
        public: Value,
        target_count: usize,
        reserve: &Value,
    ) -> RuntimeContractResult<bool> {
        if self.private.fits_before(&private, target_count, reserve)?
            && self.public.fits_before(&public, target_count, reserve)?
        {
            return self.require(private, public, target_count);
        }
        self.private.facts.omit(target_count);
        self.public.facts.omit(target_count);
        Ok(false)
    }

    /// A row of `tier`, admitted by [`Self::finish`]; its index for [`Self::request`].
    fn defer(
        &mut self,
        tier: DeferredTier,
        private: Value,
        public: Value,
        target_count: usize,
    ) -> usize {
        let position = self.next_position();
        self.deferred.push(DeferredFact {
            tier,
            position,
            private,
            public,
            target_count,
        });
        self.deferred.len() - 1
    }

    /// Moves the waiting row `index` to the requested tier.
    fn request(&mut self, index: usize) {
        if let Some(row) = self.deferred.get_mut(index) {
            row.tier = DeferredTier::Requested;
        }
    }

    /// Admits the waiting rows, the requested ones first, and answers
    /// `(private_facts, facts)`.
    fn finish(mut self) -> RuntimeContractResult<(ObservationFacts, ObservationFacts)> {
        let mut deferred = std::mem::take(&mut self.deferred);
        // A stable sort: each tier keeps its production order.
        deferred.sort_by_key(|row| row.tier);
        for row in deferred {
            self.private
                .admit(row.position, row.private, row.target_count)?;
            self.public
                .admit(row.position, row.public, row.target_count)?;
        }
        Ok((self.private.finish(), self.public.finish()))
    }
}
fn role_name(role: PageTargetRole) -> &'static str {
    match role {
        PageTargetRole::Required => "required",
        PageTargetRole::AnyOf => "any_of",
        PageTargetRole::Optional => "optional",
        PageTargetRole::Forbidden => "forbidden",
    }
}

fn geometry(input: &DriveSemanticInput) -> Result<Geometry, OnlineObservationError> {
    let integer = |value: i32| u32::try_from(value).map_err(|_| fact_error("negative geometry"));
    let rect = |value: PackRect| -> Result<projection::Rect, OnlineObservationError> {
        Ok(projection::Rect {
            x: integer(value.x)?,
            y: integer(value.y)?,
            width: integer(value.width)?,
            height: integer(value.height)?,
        })
    };
    let point = |value: crate::DrivePoint| -> Result<projection::Point, OnlineObservationError> {
        Ok(projection::Point {
            x: integer(value.x)?,
            y: integer(value.y)?,
        })
    };
    match input {
        DriveSemanticInput::Tap {
            rect: value,
            point: position,
        } => Ok(Geometry::Tap {
            rect: rect(*value)?,
            point: point(*position)?,
        }),
        DriveSemanticInput::Drag {
            from_rect,
            to_rect,
            from,
            to,
            duration_ms,
        } => Ok(Geometry::Drag {
            from_rect: rect(*from_rect)?,
            to_rect: rect(*to_rect)?,
            from: point(*from)?,
            to: point(*to)?,
            duration_ms: *duration_ms,
        }),
        DriveSemanticInput::TargetCenter { .. } | DriveSemanticInput::TargetOffset { .. } => {
            Err(fact_error("target geometry unresolved"))
        }
    }
}

/// Workflow #390 LAB-2 item 7: an `offset` click's box on this frame, from the box its template
/// matched ([`crate::drive_offset_rect`]); a target that did not pass, or matched no template
/// box, gives none.
fn offset_target_geometry(
    value: &TargetEvaluation,
    offset: PackRect,
) -> Result<Option<Geometry>, OnlineObservationError> {
    let Some(template) = value.template.filter(|_| value.passed) else {
        return Ok(None);
    };
    let rect = crate::drive_offset_rect(
        PackRect {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height,
        },
        template.scale_milli,
        offset,
    )
    .map_err(fact_error)?;
    geometry(&DriveSemanticInput::Tap {
        rect,
        point: drive_rect_center(rect).map_err(fact_error)?,
    })
    .map(Some)
}
fn target_geometry(
    value: &TargetEvaluation,
    evaluator: &RecognitionEvaluator,
) -> Result<Option<Geometry>, OnlineObservationError> {
    if !value.passed {
        return Ok(None);
    }
    let rect = if let Some(template) = value.template {
        PackRect {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height,
        }
    } else {
        let declared = evaluator
            .pack()
            .targets
            .iter()
            .find_map(|target| {
                let (id, click) = match target {
                    RecognitionTarget::Template(target) => (&target.id, target.click),
                    RecognitionTarget::Color(target) => (&target.id, target.click),
                    RecognitionTarget::Ocr(target) => (&target.id, target.click),
                    RecognitionTarget::Nn(target) => (&target.id, target.click),
                    RecognitionTarget::ClickOnly(target) => (&target.id, Some(target.click)),
                    // A digest verifies a place and a composite is a check; neither is clicked.
                    RecognitionTarget::ColorDigest(target) => (&target.id, None),
                    RecognitionTarget::Composite(target) => (&target.id, None),
                    // Workflow #396 REC-6: a value test compares read values; it is not clicked.
                    RecognitionTarget::ValueTest(target) => (&target.id, None),
                };
                (id == &value.id).then_some(click)
            })
            .ok_or_else(|| fact_error("actual target identity missing from admitted pack"))?;
        let Some(rect) = declared else {
            return Ok(None);
        };
        rect
    };
    geometry(&DriveSemanticInput::Tap {
        rect,
        point: drive_rect_center(rect).map_err(fact_error)?,
    })
    .map(Some)
}

fn redact_row(mut value: Value, metadata: &projection::VerifiedProjectionMetadata) -> Value {
    fn visit(
        value: &mut Value,
        metadata: &projection::VerifiedProjectionMetadata,
        restricted: bool,
    ) {
        match value {
            Value::Array(values) => {
                for value in values {
                    visit(value, metadata, restricted);
                }
            }
            Value::Object(object) => {
                let local = object
                    .get("target_id")
                    .and_then(Value::as_str)
                    .map(|id| metadata.target_privacy(id) != Some(Privacy::Public));
                let restricted = local.unwrap_or(restricted);
                if restricted {
                    for key in ["message", "evaluation", "text", "value", "detail"] {
                        if object.contains_key(key) {
                            object.insert(key.to_string(), Value::Null);
                        }
                    }
                    if local.is_some() {
                        object.insert("redacted".to_string(), Value::Bool(true));
                    }
                }
                // A page/batch error message can contain nested target details; typed cause stays.
                if object.contains_key("completed_targets") {
                    object.insert("message".to_string(), Value::Null);
                }
                for value in object.values_mut() {
                    visit(value, metadata, restricted);
                }
            }
            _ => {}
        }
    }
    visit(&mut value, metadata, true);
    value
}
