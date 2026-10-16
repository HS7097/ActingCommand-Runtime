// SPDX-License-Identifier: AGPL-3.0-only

//! The in-task select step (Workflow #308; `contracts/selection-graph.md`, section Select step).
//!
//! A select step names a candidate layout of its page and a selection-policy document of its
//! own task. Admission seals the document by the SHA-256 of its bytes in the package, reads it
//! with the selection-policy crate's own rules and checks it against the layout. One attempt
//! projects the layout on the step's frame, asks the runtime for the instance's fact snapshot
//! and the evaluation instant, evaluates the policy, and confirms a selected candidate on a
//! fresh frame: the same page, the step's guard, and a candidate set that hashes like the
//! evaluated one. Only then is one tap sampled inside the candidate's click rectangle, bound to
//! the confirmation frame. An attempt that reaches a decision records it exactly once, before
//! any input and before any failure return; every selection failure comes before the input.
//!
//! Workflow #308 slice SEL-L4b (frozen selector model S0 v2.1 §1.2-§1.4, §2.1, §3.2): a step may
//! declare `scan`, a select over a list read across viewports (the scan session's `select`), and
//! `on_empty`, which ends the run normally when nobody listed is present and nothing is unknown.
//! Admission checks both declarations; a single-frame select that declares `on_empty` also
//! applies the unknown-instance rules before its confirmation.

use super::scan::{
    Bound, RuntimeScanSource, ScanDeclaration, ScanSelectEnd, ScanSelection, ScanSession,
    bound_inputs, decision_blocking, record_bound, result_blocked,
};
use super::{
    ClickRect, ContainedTaskError, ContainedTaskGuardOutcome, ContainedTaskRunError,
    ContainedTaskRuntime, ContainedTaskRuntimeErrorClass, ContainedTaskTimingContext,
    ContainedTaskTrace, PageObservation, PreparedContainedTask, Resolution, SampleRetries,
    TaskControl, TaskIdentityTemplate, TaskOperation, TaskProgram, safe_task_local_path,
    sample_budget_insufficient, sampled_tap, scene_from_frame,
};
use crate::codes::ExecutionKernelCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    CandidateFeature, CandidateLayoutKind, CandidateProjection, CandidateRect, InputAction,
    InputSamplingEvidence, InstanceFactSnapshot, PolicyReasonRecord, TaskSelectionBlocking,
    TaskSelectionConfirmation, TaskSelectionGateResult, TaskSelectionOutcome, TaskSelectionPolicy,
    TaskSelectionRecord, TaskSelectionTermResult, TaskSelectionUnknownCheck, TaskSelectionVerdict,
    validate_candidate_layout_id,
};
use actingcommand_device::Frame;
use actingcommand_pack_containment::LoadedBundle;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    CandidateFeatureValue, CandidateIdentityDeclaration, CandidateIdentityRecognition,
    CandidateLayout, MAX_IDENTITY_TEMPLATES, PackRegion, RecognitionEvaluator, RecognitionTarget,
    TargetKind, UnknownIdentityHandling,
};
use actingcommand_selection_policy::{
    Candidate, CandidateStatus, CandidateVerdict, DecisionReason, GateUnknownHandling,
    MAX_DOCUMENT_BYTES, ScalarValue, SelectionDecision, SelectionFactSnapshot, SelectionMode,
    SelectionOutcome, SelectionPolicy, TermUnknownHandling, ValueRef, ValueType, evaluate,
    parse_canonical_json, upper_bound,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const SELECT_INVALID: Code = ExecutionKernelCode::ContainedTaskSelectInvalid.code();
const SELECT_POLICY_MISSING: Code = ExecutionKernelCode::ContainedTaskSelectPolicyMissing.code();
const SELECT_POLICY_INVALID: Code = ExecutionKernelCode::ContainedTaskSelectPolicyInvalid.code();
const SELECT_POLICY_MISMATCH: Code = ExecutionKernelCode::ContainedTaskSelectPolicyMismatch.code();
const SELECTION_STATE_UNAVAILABLE: Code = ExecutionKernelCode::SelectionStateUnavailable.code();
const SELECTION_FACT_CONTEXT_MISMATCH: Code =
    ExecutionKernelCode::SelectionFactContextMismatch.code();
const SELECTION_EVALUATION_FAILED: Code = ExecutionKernelCode::SelectionEvaluationFailed.code();
const SELECTION_NOT_SELECTED: Code = ExecutionKernelCode::SelectionNotSelected.code();
const SELECTION_PAGE_CHANGED: Code = ExecutionKernelCode::SelectionPageChanged.code();
const SELECTION_PROJECTION_MISMATCH: Code = ExecutionKernelCode::SelectionProjectionMismatch.code();
/// The confirmation code a record carries when the runtime's own capture of the confirmation
/// frame fails, nonfatally or fatally; the task then fails with the runtime's error.
const SELECTION_CONFIRMATION_CAPTURE_FAILED: Code =
    ExecutionKernelCode::SelectionConfirmationCaptureFailed.code();
/// A dry run's frame is not the step's page.
const SELECT_PAGE_MISMATCH: Code = ExecutionKernelCode::SelectPageMismatch.code();

/// What a select step asks its runtime for (Workflow #308).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionStateRequest {
    pub step_index: u32,
    pub operation_label: String,
    /// The package's declared game and server (`control.json`). The snapshot's context must
    /// name the same game and server, or the step fails with `selection_fact_context_mismatch`.
    pub game: String,
    pub server: String,
}

/// The instance facts and the instant one select step evaluates with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionState {
    /// The runtime has no fact snapshot or clock for a select step.
    Unavailable,
    /// One ledger-pinned instance fact snapshot and the evaluation instant; the decision
    /// records the snapshot's ID, its ledger position and the instant.
    Snapshot {
        snapshot: InstanceFactSnapshot,
        now_unix_ms: u64,
    },
}

/// One select step evaluated on a saved frame by [`dry_run_select`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionDryRun {
    pub candidate_projection: CandidateProjection,
    pub decision: SelectionDecision,
    /// The decision used an author's draft policy instead of the package's sealed document.
    pub policy_override: bool,
}

/// The `select` of a task operation: `{layout_id, policy: {path}, scan?, on_empty?}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskSelect {
    layout_id: String,
    policy: TaskSelectPolicy,
    /// Workflow #308 SEL-L4b (S0 §2.1): the list is read across viewports by a scan session.
    #[serde(default)]
    scan: Option<ScanDeclaration>,
    /// Workflow #308 SEL-L4b (S0 §1.4): nobody listed is present, and nothing is unknown, ends
    /// the run normally with this outcome key.
    #[serde(default)]
    on_empty: Option<TaskSelectOnEmpty>,
}

/// A select step's `on_empty` (Workflow #308 SEL-L4b, S0 §1.4): `{outcome_key}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskSelectOnEmpty {
    outcome_key: String,
}

/// The task-local policy document a select step names. Workflow #336 PKG-S2 (option 丙, D5):
/// a `sha256` the step may still declare is an `IgnoredPin`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskSelectPolicy {
    path: String,
    #[serde(default, rename = "sha256")]
    _ignored_sha256: actingcommand_contract::IgnoredPin,
}

impl TaskSelect {
    /// The outcome key of the step's `on_empty`, when it declares one (Workflow #308 SEL-L4b).
    pub(super) fn on_empty_key(&self) -> Option<&str> {
        self.on_empty
            .as_ref()
            .map(|on_empty| on_empty.outcome_key.as_str())
    }

    /// The step's `scan`, when it declares one (Workflow #308 SEL-L4b).
    pub(super) fn scan(&self) -> Option<&ScanDeclaration> {
        self.scan.as_ref()
    }

    /// The layout ID has the candidate layout grammar; the document is a `policies/<name>.json`
    /// path of the task directory.
    pub(super) fn validate(&self, operation_id: &str) -> Result<(), ContainedTaskError> {
        if validate_candidate_layout_id(&self.layout_id).is_err()
            || !safe_task_local_path(&self.policy.path)
            || !self.policy.path.starts_with("policies/")
            || !self.policy.path.ends_with(".json")
        {
            return Err(ContainedTaskError::with_detail(
                SELECT_INVALID,
                format!("operation={operation_id} select declaration is malformed"),
            ));
        }
        Ok(())
    }
}

/// An admitted select step: its layout and its parsed, sealed policy document.
#[derive(Debug)]
pub(super) struct PreparedSelect {
    layout_id: String,
    /// The document's package path, `operations/<task>/<declared path>`, below the resource root.
    path: String,
    policy: SelectionPolicy,
    /// Workflow #308 SEL-L4b: the admitted `scan`.
    scan: Option<ScanDeclaration>,
    /// Workflow #308 SEL-L4b: the admitted `on_empty` outcome key.
    on_empty_key: Option<String>,
    /// Workflow #308 SEL-L4b: the feature whose identity is the step's choice (the scan's
    /// `identity_feature`, or the layout's one identity feature); `expect_identity` maps its
    /// reading through it.
    identity_feature: Option<String>,
}

impl PreparedSelect {
    /// The step's candidate layout (Workflow #308 SEL-L3: a scan session's layout).
    pub(super) fn layout_id(&self) -> &str {
        &self.layout_id
    }

    /// The step's sealed policy document (Workflow #308 SEL-L3: a scan session evaluates every
    /// viewport with it).
    pub(super) fn policy(&self) -> &SelectionPolicy {
        &self.policy
    }

    /// The feature whose identity is the step's choice (Workflow #308 SEL-L4b).
    pub(super) fn identity_feature(&self) -> Option<&str> {
        self.identity_feature.as_deref()
    }
}

#[cfg(test)]
impl PreparedSelect {
    /// A prepared select step for the gate tests outside this module (Workflow #386 SR-1): the
    /// layout `layout_id` and the sealed document `policy`.
    pub(super) fn for_test(layout_id: &str, policy: SelectionPolicy) -> Self {
        Self {
            layout_id: layout_id.to_owned(),
            path: "operations/task/policies/select.json".to_owned(),
            policy,
            scan: None,
            on_empty_key: None,
            identity_feature: None,
        }
    }
}

/// A select step's confirmed candidate: the guard outcome on the confirmation frame, the
/// candidate's click rectangle and the confirmation frame, whose input context binds the tap.
/// Workflow #308 SEL-L4b: also the business ID of the step's identity feature, when the chosen
/// candidate read one (`expect_identity` compares with it).
pub(super) struct SelectedCandidate {
    pub(super) guard: ContainedTaskGuardOutcome,
    click: ClickRect,
    pub(super) frame: PageObservation,
    pub(super) identity: Option<String>,
    /// The step's action seed when the attempt already drew it (a scan's moves are sampled with
    /// it); the runtime gives a step's seed once, so the selecting tap reuses it.
    pub(super) action_seed: Option<Option<u64>>,
}

/// How one attempt of a select step ended without a failure (Workflow #308 SEL-L4b).
pub(super) enum SelectAttempt {
    /// A candidate was confirmed; its tap follows. Boxed: it holds the confirmation frame, and
    /// inline it would be the one large variant (clippy `large_enum_variant`).
    Selected(Box<SelectedCandidate>),
    /// Nobody listed is present and nothing is unknown, and the step declares `on_empty` (S0
    /// §1.4, path P4): the run ends Completed with `outcome_key`, on the page `page_label` of
    /// the step's last frame.
    Empty {
        outcome_key: String,
        page_label: String,
    },
}

impl SelectedCandidate {
    /// One tap sampled inside the candidate's click rectangle, as a `rect` click samples one.
    pub(super) fn input_action(
        &self,
        resolution: &Resolution,
        action_seed: Option<u64>,
    ) -> Result<(InputAction, Option<InputSamplingEvidence>), ContainedTaskError> {
        self.click.validate(resolution)?;
        let (action, sampling) = sampled_tap(self.click, action_seed)?;
        action.validate().map_err(|_| {
            ContainedTaskError::new(ExecutionKernelCode::ContainedTaskOperationInvalid.code())
        })?;
        if let InputAction::Tap { x, y } = &action {
            resolution.validate_point(*x, *y)?;
        }
        Ok((action, sampling))
    }
}

/// Contained task admission of every select step, after the program and the page set are
/// validated: the layout is a candidate layout of the recognition pack on the step's `from`
/// page; the policy document is present in the package, it reads as a selection-policy
/// document, and it applies to the layout (`contracts/selection-graph.md`, section Select
/// step). A `sha256` it may still declare is ignored (Workflow #336 PKG-S2).
pub(super) fn prepare_select_steps(
    program: &mut TaskProgram,
    control: &TaskControl,
    bundle: &LoadedBundle,
    evaluator: &RecognitionEvaluator,
) -> Result<(), ContainedTaskError> {
    let task_id = program.task_id.clone();
    let scheduled = program.scheduling_outcome.is_some();
    for operation in &mut program.operations {
        let Some(select) = &operation.select else {
            continue;
        };
        let failure = |code: Code, detail: String| {
            ContainedTaskError::with_detail(code, format!("operation={} {detail}", operation.id))
        };
        let layout = evaluator
            .candidate_layout(&select.layout_id)
            .ok_or_else(|| {
                failure(
                    SELECT_INVALID,
                    format!(
                        "layout={} is not a candidate layout of the recognition pack",
                        select.layout_id
                    ),
                )
            })?;
        if crate::canonical_page_anchor(&control.game, &layout.page_id)
            != crate::canonical_page_anchor(&control.game, &operation.from)
        {
            return Err(failure(
                SELECT_INVALID,
                format!(
                    "layout={} belongs to page {}, the step runs from {}",
                    layout.id, layout.page_id, operation.from
                ),
            ));
        }
        let path = format!("operations/{task_id}/{}", select.policy.path);
        let bytes = bundle
            .resource_entry(&path)
            .map_err(|_| failure(SELECT_POLICY_MISSING, format!("policy={path}")))?;
        let policy = read_policy(bytes)
            .map_err(|reason| failure(SELECT_POLICY_INVALID, format!("policy={path} {reason}")))?;
        check_policy(&policy, layout)
            .map_err(|reason| failure(SELECT_POLICY_MISMATCH, format!("policy={path} {reason}")))?;
        // Workflow #308 SEL-L4b: the static checks of a scan (S0 §2.1) and the admission of
        // `on_empty` (§1.4 (a), (b) and §1.2); the route's terminal for `on_empty` is the
        // coverage or phase check's, and the post-admission refusals are the task's.
        let identity_feature = match &select.scan {
            Some(scan) => {
                if let Some(detail) = scan.admission_refusal(
                    layout,
                    &policy,
                    evaluator,
                    (control.resolution.width, control.resolution.height),
                    control.task_timeout().milliseconds,
                ) {
                    return Err(failure(SELECT_INVALID, detail));
                }
                Some(scan.identity_feature().to_owned())
            }
            None => single_identity_feature(layout),
        };
        if select.on_empty.is_some() {
            if policy.selection.mode != SelectionMode::NoneAllowed {
                return Err(failure(
                    SELECT_INVALID,
                    "on_empty mode=not_none_allowed".to_owned(),
                ));
            }
            if !scheduled {
                return Err(failure(
                    SELECT_INVALID,
                    "on_empty scheduling_outcome=absent".to_owned(),
                ));
            }
            if select.scan.is_none() && layout.kind != CandidateLayoutKind::FixedSlots {
                return Err(failure(
                    SELECT_INVALID,
                    "on_empty layout=not_fixed_slots".to_owned(),
                ));
            }
        }
        if select.scan.is_some() || select.on_empty.is_some() {
            bound_inputs(layout, &policy, identity_feature.as_deref())
                .map_err(|detail| failure(SELECT_INVALID, format!("upper_bound {detail}")))?;
        }
        let prepared = PreparedSelect {
            layout_id: select.layout_id.clone(),
            path,
            policy,
            scan: select.scan.clone(),
            on_empty_key: select.on_empty_key().map(str::to_owned),
            identity_feature,
        };
        operation.prepared_select = Some(Box::new(prepared));
    }
    validate_expect_identities(program, control, evaluator)
}

/// The layout's one identity feature, when it declares exactly one (Workflow #308 SEL-L4b): the
/// identity a single-frame select chooses.
fn single_identity_feature(layout: &CandidateLayout) -> Option<String> {
    let mut identities = layout
        .features
        .iter()
        .filter(|feature| feature.identity.is_some());
    match (identities.next(), identities.next()) {
        (Some(feature), None) => Some(feature.name.clone()),
        _ => None,
    }
}

/// Workflow #308 SEL-L4b (S0 §1.3, admission of `expect_identity`): `choice_of` names a select
/// operation of the same task that is the declaring one or precedes it (in phase order, or in
/// declaration order without phases), and that step has an identity feature; the declaring
/// operation is not retryable, because a retry would tap again after a mismatch; and exactly
/// one of the two paths is declared. The text path (`target_id`) needs a text-based identity,
/// because the read text is mapped through it, and an OCR target. The template path
/// (SEL-L4b-T, `templates`) is admitted for any identity kind and is checked by
/// [`validate_identity_templates`]. Every refusal is `contained_task_select_invalid`.
fn validate_expect_identities(
    program: &TaskProgram,
    control: &TaskControl,
    evaluator: &RecognitionEvaluator,
) -> Result<(), ContainedTaskError> {
    let phase_of = |id: &str| {
        program.phases.as_ref().and_then(|phases| {
            phases
                .iter()
                .position(|phase| phase.operations.iter().any(|operation| operation == id))
        })
    };
    for (index, operation) in program.operations.iter().enumerate() {
        let Some(expect) = operation
            .expect_after
            .as_ref()
            .and_then(|expectation| expectation.expect_identity.as_ref())
        else {
            continue;
        };
        let refuse = |detail: &str| {
            ContainedTaskError::with_detail(
                SELECT_INVALID,
                format!("operation={} expect_identity {detail}", operation.id),
            )
        };
        let Some((choice_index, choice)) = program
            .operations
            .iter()
            .enumerate()
            .find(|(_, candidate)| candidate.id == expect.choice_of)
        else {
            return Err(refuse("choice_of=undeclared"));
        };
        let Some(prepared) = choice.prepared_select.as_deref() else {
            return Err(refuse("choice_of=not_a_select_step"));
        };
        let precedes = match &program.phases {
            Some(_) => matches!(
                (phase_of(&choice.id), phase_of(&operation.id)),
                (Some(choice), Some(current)) if choice <= current
            ),
            None => choice_index <= index,
        };
        if !precedes {
            return Err(refuse("choice_of=after_the_step"));
        }
        // SEL-L4b-T: a step without an identity feature has no identity to read back.
        let Some(identity) = evaluator
            .candidate_layout(&prepared.layout_id)
            .zip(prepared.identity_feature())
            .and_then(|(layout, name)| layout.features.iter().find(|feature| feature.name == name))
            .and_then(|feature| feature.identity.as_ref())
        else {
            return Err(refuse("choice_of=no_identity"));
        };
        match (&expect.target_id, &expect.templates) {
            (Some(_), Some(_)) => return Err(refuse("expect_identity=target_and_templates")),
            (None, None) => return Err(refuse("expect_identity=neither")),
            (Some(_), None) | (None, Some(_)) => {}
        }
        if expect.target_id.is_some()
            && !matches!(
                identity.recognition,
                CandidateIdentityRecognition::OcrAliases { .. }
            )
        {
            return Err(refuse("choice_of=identity_not_text"));
        }
        if operation
            .retry_policy(program.defaults, control.task_timeout().milliseconds)?
            .is_some_and(|policy| policy.retryable())
        {
            return Err(refuse("operation=retryable"));
        }
        if let Some(target_id) = &expect.target_id {
            if !matches!(evaluator.target_kind(target_id), Ok(TargetKind::Ocr)) {
                return Err(refuse("target=not_ocr"));
            }
        } else if let Some(pool) = &expect.templates
            && let Err(detail) = validate_identity_templates(pool, identity, evaluator)
        {
            return Err(refuse(detail));
        }
    }
    Ok(())
}

/// Workflow #308 SEL-L4b-T (L4b-T.3): the template pool of an `expect_identity`. It holds
/// 1..=16 entries (`templates=count`). Each entry names a template target of the pack
/// (`template=not_template`) once (`template=duplicate`), an `id` that is an entry ID of the
/// `choice_of` identity (`template=out_of_domain`), and a target whose effective threshold is
/// above 0 (`template=threshold_zero`: a target that always passes, which REC-7 skips) and whose
/// region is a `rect` (`template=region_not_rect`); every entry searches the same rect
/// (`template=regions_differ`), the destination's one identity region. REC-7's flat-field floor
/// and mask minimums hold for every template target at pack admission already. The error is the
/// refusal's detail token.
fn validate_identity_templates(
    pool: &[TaskIdentityTemplate],
    identity: &CandidateIdentityDeclaration,
    evaluator: &RecognitionEvaluator,
) -> Result<(), &'static str> {
    if !(1..=MAX_IDENTITY_TEMPLATES).contains(&pool.len()) {
        return Err("templates=count");
    }
    let pack = evaluator.pack();
    let mut targets = BTreeSet::new();
    let mut region = None;
    for entry in pool {
        let Some(target) = pack.targets.iter().find_map(|target| match target {
            RecognitionTarget::Template(template) if template.id == entry.target_id => {
                Some(template)
            }
            _ => None,
        }) else {
            return Err("template=not_template");
        };
        if !targets.insert(entry.target_id.as_str()) {
            return Err("template=duplicate");
        }
        if !identity.entries.iter().any(|known| known.id == entry.id) {
            return Err("template=out_of_domain");
        }
        if target.threshold.unwrap_or(pack.defaults.template_threshold) <= 0.0 {
            return Err("template=threshold_zero");
        }
        let PackRegion::Rect(rect) = target.region else {
            return Err("template=region_not_rect");
        };
        match region {
            None => region = Some(rect),
            Some(first) if first == rect => {}
            Some(_) => return Err("template=regions_differ"),
        }
    }
    Ok(())
}

pub(super) fn page_recognition_budget_ms(
    evaluator: &RecognitionEvaluator,
    detector: &actingcommand_page_detector::PageDetector,
    candidates: Option<&[&str]>,
) -> Result<u64, ContainedTaskError> {
    let targets = detector
        .page_definitions()
        .iter()
        .filter(|page| candidates.is_none_or(|ids| ids.contains(&page.id.as_str())))
        .flat_map(|page| {
            page.required
                .iter()
                .chain(page.any_of.iter().flatten())
                .chain(page.optional.iter())
                .chain(page.forbidden.iter())
                .map(String::as_str)
        });
    evaluator
        .maximum_provider_ms(targets)
        .map(|provider| provider.saturating_add(evaluator.target_sample_wait_ms()))
        .map_err(|error| ContainedTaskError::with_detail(SELECT_INVALID, error.to_string()))
}

/// Known calls and declared waits on one complete pre-input path. Initial page recognition
/// precedes StepStarted; each additional candidate frame and H2 capture recognizes pages.
pub(super) fn sampling_budget_ms(
    operation: &TaskOperation,
    evaluator: &RecognitionEvaluator,
    detector: &actingcommand_page_detector::PageDetector,
) -> Result<Option<u64>, ContainedTaskError> {
    let layout = operation
        .select
        .as_ref()
        .map(|select| {
            evaluator
                .candidate_layout(&select.layout_id)
                .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID))
        })
        .transpose()?;
    if evaluator.pack().target_consensus.is_empty()
        && layout.is_none_or(|layout| !layout.has_consensus())
    {
        return Ok(None);
    }
    let guard = if operation.unguarded_trusted_coordinate {
        0
    } else {
        evaluator
            .maximum_provider_ms(operation.guard.iter().map(|guard| guard.target_id.as_str()))
            .map_err(|error| ContainedTaskError::with_detail(SELECT_INVALID, error.to_string()))?
    };
    let required = if let Some(layout) = layout {
        let frames = layout.required_frames() as u64;
        let pages = page_recognition_budget_ms(evaluator, detector, None)?;
        // H1/H2 each add F-1 frames; H2 starts with another fresh page capture. Its guard
        // runs before projection and again on the last frame when the projection adds frames.
        layout
            .maximum_provider_and_wait_ms(evaluator)
            .saturating_mul(2)
            .saturating_add(pages.saturating_mul(2 * frames - 1))
            .saturating_add(guard.saturating_mul(if frames > 1 { 2 } else { 1 }))
    } else {
        guard
    };
    Ok(Some(required))
}

/// The selection-policy crate's own reading: the byte limit, the typed decode, the canonical
/// form (no floats, no unsafe integers, no duplicate keys) and the document validation.
fn read_policy(bytes: &[u8]) -> Result<SelectionPolicy, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "{} bytes exceed the {MAX_DOCUMENT_BYTES}-byte document limit",
            bytes.len()
        ));
    }
    let policy: SelectionPolicy =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    parse_canonical_json(bytes).map_err(|error| error.to_string())?;
    policy.validate().map_err(|error| error.to_string())?;
    Ok(policy)
}

/// The document applies to `layout`: it names the layout, reads only features the layout
/// declares with their type (`passed` as boolean, `measure_milli` as integer), and requires
/// exactly one candidate. A document that reads a record list through `keyed_fact` needs a
/// layout that rejects unknown identities, so the evaluator never meets an unknown key.
fn check_policy(policy: &SelectionPolicy, layout: &CandidateLayout) -> Result<(), String> {
    if policy.applies_to.candidate_layout_id != layout.id {
        return Err(format!(
            "applies_to={} differs from layout={}",
            policy.applies_to.candidate_layout_id, layout.id
        ));
    }
    for field in &policy.fields {
        let feature = layout
            .features
            .iter()
            .find(|feature| feature.name == field.name);
        match (feature.map(|feature| feature.value), &field.value_type) {
            (Some(CandidateFeatureValue::Passed), ValueType::Boolean)
            | (
                Some(CandidateFeatureValue::MeasureMilli | CandidateFeatureValue::OcrInteger),
                ValueType::Integer,
            ) => {}
            (Some(CandidateFeatureValue::Identity), ValueType::EnumString { allowed })
                if feature
                    .and_then(|feature| feature.identity.as_ref())
                    .is_some_and(|identity| {
                        let mut allowed = allowed.clone();
                        allowed.sort();
                        identity.domain() == allowed
                    }) => {}
            (None, _) => {
                return Err(format!(
                    "field={} is not a feature of layout={}",
                    field.name, layout.id
                ));
            }
            (Some(_), _) => {
                return Err(format!(
                    "field={} is typed unlike its feature of layout={}",
                    field.name, layout.id
                ));
            }
        }
    }
    if layout.features.iter().any(|feature| {
        feature.identity.is_some() || feature.integer.is_some() || feature.consensus.is_some()
    }) && policy.gates.iter().any(|gate| {
        matches!(
            gate.on_unknown,
            GateUnknownHandling::SubstituteVerdict { passes: true }
        )
    }) {
        return Err(
            "declared identity, integer or consensus selection requires fail-closed hard gates"
                .into(),
        );
    }
    if layout.unknown_identity == UnknownIdentityHandling::ReadableAttributes {
        for term in &policy.scoring {
            if let TermUnknownHandling::SubstituteMilli { value_milli } = term.on_unknown {
                let identity_term = matches!(&term.value, ValueRef::Field { field }
                    if layout.features.iter().any(|feature| feature.name == *field && feature.identity.is_some()));
                if !identity_term || i128::from(value_milli) * i128::from(term.weight_milli) > 0 {
                    return Err("readable_attributes permits only a nonpositive identity substitution; other scoring inputs must be known".into());
                }
            }
        }
    }
    if policy.uses_record_lists() && layout.unknown_identity != UnknownIdentityHandling::Reject {
        return Err(format!(
            "layout={} unknown_identity=readable_attributes record_list=declared",
            layout.id
        ));
    }
    if policy.selection.required_count != 1 {
        return Err(format!(
            "required_count={} where a select step chooses one candidate",
            policy.selection.required_count
        ));
    }
    Ok(())
}

/// The layout's candidate projection on one scene; a projection failure keeps its code.
fn project(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    layout_id: &str,
) -> Result<CandidateProjection, ContainedTaskError> {
    evaluator
        .scene_context(scene)
        .project_candidates(layout_id)
        .map_err(|failure| {
            ContainedTaskError::with_detail(failure.code(), failure.detail())
                .with_ppocr_diagnostics(
                    failure
                        .cause()
                        .map(|cause| cause.ppocr_diagnostics().clone())
                        .unwrap_or_default(),
                )
        })
}

/// The evaluator's decision over the actionable candidates of `projection`, each feature a
/// field of its name: a boolean or an integer as the projection carries it.
fn decide(
    policy: &SelectionPolicy,
    layout: &CandidateLayout,
    projection: &CandidateProjection,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<SelectionDecision, ContainedTaskError> {
    let candidates = projection
        .candidates()
        .iter()
        .filter(|candidate| candidate.actionable)
        .filter(|candidate| {
            layout
                .features
                .iter()
                .filter(|feature| feature.consensus.is_some())
                .all(|feature| {
                    candidate
                        .features
                        .get(&feature.name)
                        .is_some_and(|value| !matches!(value, CandidateFeature::Unknown { .. }))
                })
        })
        .filter(|candidate| {
            layout.unknown_identity == UnknownIdentityHandling::ReadableAttributes
                || layout
                    .features
                    .iter()
                    .filter(|feature| feature.identity.is_some())
                    .all(|feature| {
                        matches!(
                            candidate.features.get(&feature.name),
                            Some(CandidateFeature::Identity { .. })
                        )
                    })
        })
        .map(|candidate| Candidate {
            candidate_id: candidate.id.clone(),
            fields: candidate
                .features
                .iter()
                .filter_map(|(name, feature)| {
                    let value = match feature {
                        CandidateFeature::Boolean { value, .. } => ScalarValue::Boolean(*value),
                        CandidateFeature::Integer { value, .. } => ScalarValue::Integer(*value),
                        CandidateFeature::Identity { value, .. } => {
                            ScalarValue::String(value.clone())
                        }
                        CandidateFeature::Unknown { .. } => return None,
                    };
                    Some((name.clone(), value))
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    evaluate(policy, &candidates, facts, now_unix_ms).map_err(|error| {
        ContainedTaskError::with_detail(SELECTION_EVALUATION_FAILED, error.to_string())
    })
}

/// The one chosen candidate of a `selected` decision; `None` for every other outcome.
fn chosen(decision: &SelectionDecision) -> Result<Option<&str>, ContainedTaskError> {
    match (&decision.outcome, decision.selected.as_slice()) {
        (SelectionOutcome::Selected { .. }, [candidate_id]) => Ok(Some(candidate_id.as_str())),
        (SelectionOutcome::Selected { count }, _) => Err(ContainedTaskError::with_detail(
            SELECTION_EVALUATION_FAILED,
            format!("selected count={count} where one candidate is required"),
        )),
        _ => Ok(None),
    }
}

/// The `kind` of a decision outcome, as its serde tag names it.
fn outcome_kind(outcome: &SelectionOutcome) -> &'static str {
    match outcome {
        SelectionOutcome::Selected { .. } => "selected",
        SelectionOutcome::Empty => "empty",
        SelectionOutcome::Insufficient { .. } => "insufficient",
        SelectionOutcome::Ambiguous { .. } => "ambiguous",
        SelectionOutcome::Unknown { .. } => "unknown",
    }
}

/// The contract's mirror of one evaluator value, through their shared serde shape
/// (`contracts/selection-graph.md`, section Evaluator mirror).
fn mirror<T: Serialize, U: DeserializeOwned>(value: &T) -> Result<U, ContainedTaskError> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|error| {
            ContainedTaskError::with_detail(
                SELECTION_EVALUATION_FAILED,
                format!("decision record: {error}"),
            )
        })
}

/// The record's outcome of the decision (ledger revision 3): an unknown outcome keeps its
/// reason, and the evaluator's `detail` text stays out of the record.
fn record_outcome(outcome: &SelectionOutcome) -> Result<TaskSelectionOutcome, ContainedTaskError> {
    match outcome {
        SelectionOutcome::Unknown { reason, .. } => Ok(TaskSelectionOutcome::Unknown {
            reason: mirror(reason)?,
        }),
        other => mirror(other),
    }
}

/// The record's reason chain (ledger revision 3): each reason's outcome, its code with its
/// values; the evaluator's English `detail` stays out of the record.
fn record_reasons(reasons: &[DecisionReason]) -> Vec<PolicyReasonRecord> {
    reasons
        .iter()
        .map(|reason| PolicyReasonRecord(reason.outcome.clone()))
        .collect()
}

/// The record's per-candidate breakdown: each gate's and term's result as its
/// `selection_result` (ledger revision 3), each reason as [`record_reasons`] keeps it.
fn record_verdicts(
    verdicts: &[CandidateVerdict],
) -> Result<Vec<TaskSelectionVerdict>, ContainedTaskError> {
    verdicts
        .iter()
        .map(|verdict| {
            Ok(TaskSelectionVerdict {
                candidate_id: verdict.candidate_id.clone(),
                status: mirror(&verdict.status)?,
                score_milli: verdict.score_milli,
                rank: verdict.rank,
                gates: verdict
                    .gates
                    .iter()
                    .map(|gate| {
                        Ok(TaskSelectionGateResult {
                            gate_id: gate.gate_id.clone(),
                            selection_result: mirror(&gate.outcome)?,
                        })
                    })
                    .collect::<Result<_, ContainedTaskError>>()?,
                terms: verdict
                    .terms
                    .iter()
                    .map(|term| {
                        Ok(TaskSelectionTermResult {
                            term_id: term.term_id.clone(),
                            selection_result: mirror(&term.outcome)?,
                            weight_milli: term.weight_milli,
                            contribution_milli: term.contribution_milli,
                        })
                    })
                    .collect::<Result<_, ContainedTaskError>>()?,
                reasons: record_reasons(&verdict.reasons),
            })
        })
        .collect()
}

/// The decision's record, mirrored before the confirmation frame is captured; the attempt
/// sets its `confirmation`.
fn selection_record(
    prepared: &PreparedSelect,
    projection: &CandidateProjection,
    snapshot: &InstanceFactSnapshot,
    now_unix_ms: u64,
    decision: &SelectionDecision,
) -> Result<TaskSelectionRecord, ContainedTaskError> {
    Ok(TaskSelectionRecord {
        layout_id: projection.layout_id().to_owned(),
        page_id: projection.page_id().to_owned(),
        projection: projection.clone().into(),
        policy: TaskSelectionPolicy {
            path: prepared.path.clone(),
            policy_sha256: decision.policy_sha256.clone(),
            policy_id: decision.policy_id.clone(),
        },
        fact_snapshot_id: decision.fact_snapshot_id.clone(),
        input_ledger_position: snapshot.ledger_position,
        input_fact_revisions: None,
        now_unix_ms,
        input_sha256: decision.input_sha256.clone(),
        selection_result: record_outcome(&decision.outcome)?,
        outcome_key: decision.outcome_key.clone(),
        selected: decision.selected.clone(),
        verdicts: record_verdicts(&decision.candidates)?,
        reasons: record_reasons(&decision.reasons),
        confirmation: TaskSelectionConfirmation::NotAttempted,
        unknown_check: None,
        multi_select: None,
    })
}

/// How a confirmation ended: the record's `confirmation`, and the candidate to tap or the
/// failure the attempt returns once the record is written.
type Confirmation<E> = (
    TaskSelectionConfirmation,
    Result<SelectedCandidate, ContainedTaskRunError<E>>,
);

/// How an attempt ended once its decision existed: the record's `confirmation`, and the attempt's
/// end or the failure it returns once the record is written (Workflow #308 SEL-L4b).
type Attempted<E> = (
    TaskSelectionConfirmation,
    Result<SelectAttempt, ContainedTaskRunError<E>>,
);

impl PreparedContainedTask {
    /// The layout's projection on `first` as a select step takes it: candidate sampling and
    /// its budget against `deadline`, every candidate evaluation recorded. A scan session
    /// (Workflow #308 SEL-L3) projects each viewport frame with it.
    pub(super) fn project_transaction<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        first: &PageObservation,
        layout: &CandidateLayout,
        timing: ContainedTaskTimingContext,
        deadline: Instant,
    ) -> Result<(CandidateProjection, Option<PageObservation>), ContainedTaskRunError<R::Error>>
    {
        let enhanced = layout.has_consensus()
            || layout
                .features
                .iter()
                .any(|feature| feature.identity.is_some() || feature.integer.is_some());
        if !enhanced {
            return Ok((project(&self.evaluator, &first.scene, &layout.id)?, None));
        }
        // Workflow #386 SR-1 (scheduling model §2.1): the deadline is compared on the
        // observation clock; past it the candidate sampling timed out.
        let check_deadline = |now: Instant| {
            if now >= deadline {
                Err(ContainedTaskError::new(
                    ExecutionKernelCode::CandidateSampleDeadlineExceeded.code(),
                ))
            } else {
                Ok(())
            }
        };
        let insufficient = |required_ms: u64, now: Instant| {
            sample_budget_insufficient(
                ExecutionKernelCode::CandidateSampleBudgetInsufficient,
                required_ms,
                deadline.saturating_duration_since(now),
            )
        };
        check_deadline(runtime.observation_now())?;
        if layout.has_consensus() {
            // Workflow #386 SR-1 (§2.2): whether the runtime samples is its own answer; the
            // checkpoint only answers cancellation.
            if !runtime.sampling_supported() {
                return Err(ContainedTaskError::new(
                    ExecutionKernelCode::CandidateSamplingRuntimeUnsupported.code(),
                )
                .into());
            }
            runtime
                .candidate_sampling_checkpoint()
                .map_err(ContainedTaskRunError::operation::<R>)?;
            let transaction_ms = layout
                .maximum_provider_and_wait_ms(&self.evaluator)
                .saturating_add(
                    page_recognition_budget_ms(&self.evaluator, &self.detector, None)?
                        .saturating_mul(layout.required_frames().saturating_sub(1) as u64),
                );
            let now = runtime.observation_now();
            if Duration::from_millis(transaction_ms) > deadline.saturating_duration_since(now) {
                return Err(insufficient(transaction_ms, now).into());
            }
        }
        let mut frames = Vec::with_capacity(layout.required_frames().saturating_sub(1));
        for _ in 1..layout.required_frames() {
            check_deadline(runtime.observation_now())?;
            let interval = Duration::from_millis(u64::from(layout.sample_interval_ms));
            let now = runtime.observation_now();
            if interval > deadline.saturating_duration_since(now) {
                return Err(insufficient(u64::from(layout.sample_interval_ms), now).into());
            }
            let boundary = actingcommand_contract::TaskTimingBoundary::PageRecognitionWait;
            let identity = runtime.task_boundary_identity(boundary);
            let started = runtime.monotonic_now();
            runtime.pause(interval);
            let ended = runtime.monotonic_now();
            runtime.observe_task_boundary(super::ContainedTaskBoundaryTiming {
                boundary,
                identity,
                context: timing,
                started,
                ended,
                succeeded: true,
            });
            check_deadline(runtime.observation_now())?;
            let frame = self
                .capture_frame(runtime, None, None, timing, None, Some(deadline))?
                .ok_or_else(|| ContainedTaskError::new(SELECTION_PAGE_CHANGED))?;
            if !crate::page_anchor_matches(&self.control.game, &frame.page_label, &operation.from)
                || frame.scene.width() != first.scene.width()
                || frame.scene.height() != first.scene.height()
                || match (&frame.input_context, &first.input_context) {
                    (Some(current), Some(first)) => !current.same_geometry(first),
                    (None, None) => false,
                    _ => true,
                }
            {
                return Err(ContainedTaskError::new(
                    ExecutionKernelCode::CandidateSamplePageOrGeometryChanged.code(),
                )
                .into());
            }
            frames.push(frame);
        }
        let scenes = std::iter::once(&first.scene)
            .chain(frames.iter().map(|frame| &frame.scene))
            .collect::<Vec<_>>();
        let provider_ms = layout
            .maximum_provider_and_wait_ms(&self.evaluator)
            .saturating_sub(
                layout.required_frames().saturating_sub(1) as u64
                    * u64::from(layout.sample_interval_ms),
            );
        let now = runtime.observation_now();
        if Duration::from_millis(provider_ms) > deadline.saturating_duration_since(now) {
            return Err(insufficient(provider_ms, now).into());
        }
        let mut recording_error = None;
        // Workflow #386 SR-1 (§2.1): a candidate recognition that ran past the deadline is the
        // candidate sampling's timeout, not a projection failure.
        let mut deadline_passed = false;
        let projection = self.evaluator.scene_context(scenes[scenes.len() - 1]).with_sample_deadline(deadline).project_candidates_with_samples(
            &layout.id, &scenes, &mut |target, result, started, ended| {
                let measured = super::ContainedTaskEvaluationTiming {
                    elapsed_us: super::observe_instant_span(started, ended), budget_before: timing.budget_at(started),
                    result: if result.is_ok() { actingcommand_contract::TaskTimingResult::Ok } else { actingcommand_contract::TaskTimingResult::Err },
                };
                if let Err(error) = runtime.record_candidate_evaluation(target, result, measured) {
                    recording_error = Some(error);
                    return Err(actingcommand_recognition_pack::CandidateProjectionFailure::recording_failed("runtime rejected candidate sample recording"));
                }
                check_deadline(runtime.observation_now()).map_err(|error| {
                    deadline_passed = true;
                    actingcommand_recognition_pack::CandidateProjectionFailure::recording_failed(error.code().as_str())
                })
            });
        if let Some(error) = recording_error {
            return Err(ContainedTaskRunError::Boundary(error));
        }
        if deadline_passed {
            return Err(ContainedTaskError::new(
                ExecutionKernelCode::CandidateSampleDeadlineExceeded.code(),
            )
            .into());
        }
        let projection = projection.map_err(|failure| {
            ContainedTaskError::with_detail(failure.code(), failure.detail())
                .with_ppocr_diagnostics(
                    failure
                        .cause()
                        .map(|cause| cause.ppocr_diagnostics().clone())
                        .unwrap_or_default(),
                )
        })?;
        check_deadline(runtime.observation_now())?;
        Ok((projection, frames.pop()))
    }

    /// One attempt of a select step on its step frame `observation`. A failure before the
    /// decision (projection, snapshot, context, evaluation) returns without a record: no
    /// decision exists to record, and the task fails with that code. Once a decision exists,
    /// exactly one `SelectionEvaluated` trace precedes the return, whether a candidate is
    /// confirmed or the attempt fails. Only a decision that cannot be recorded
    /// (`selection_record_too_large`, an invalid record) and a record boundary failure, which
    /// forbids further records, return without one; neither reaches an input. After a fatal
    /// failure of the runtime's confirmation capture the record goes to the runtime as
    /// `run_ending` and the capture's error is returned unchanged: a failure to write that
    /// record, including the runtime's own validation of it, stays with the runtime.
    ///
    /// Workflow #386 SR-1: `sampling` is the step's observe-only retries and its window; a
    /// confirmation that ended by a sampling timeout records that timeout with the retries made
    /// so far.
    pub(super) fn select_attempt<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        observation: &PageObservation,
        step_index: u32,
        timing: ContainedTaskTimingContext,
        sampling: (&SampleRetries, Duration),
    ) -> Result<SelectAttempt, ContainedTaskRunError<R::Error>> {
        let prepared = operation.prepared_select.as_deref().ok_or_else(|| {
            ContainedTaskError::with_detail(
                SELECT_INVALID,
                format!("operation={} has no admitted select step", operation.id),
            )
        })?;
        // Workflow #308 SEL-L4b: a select over a scan (S0 §2.2).
        if let Some(scan) = &prepared.scan {
            return self.scan_select_attempt(
                runtime,
                operation,
                observation,
                (step_index, timing),
                (prepared, scan),
            );
        }
        let layout = self
            .evaluator
            .candidate_layout(&prepared.layout_id)
            .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID))?;
        let deadline = timing.deadline();
        let sampling_budget = sampling_budget_ms(operation, &self.evaluator, &self.detector)?;
        if let Some(required_ms) = sampling_budget {
            // Workflow #386 SR-1 (§2.3): too little time left for the selection ends the run
            // before any capture, with what it needs and what is left.
            let remaining = deadline.saturating_duration_since(runtime.observation_now());
            if remaining.is_zero() || Duration::from_millis(required_ms) > remaining {
                return Err(sample_budget_insufficient(
                    ExecutionKernelCode::CandidateSampleBudgetInsufficient,
                    required_ms,
                    remaining,
                )
                .into());
            }
        }
        let (projection, _) =
            self.project_transaction(runtime, operation, observation, layout, timing, deadline)?;
        let (snapshot, now_unix_ms) = self.pinned_state(runtime, operation, step_index)?;
        // The rows of the record lists the document declares, which `keyed_fact` reads; every
        // other record list stays unusable, so a document without one hashes as before.
        let facts = SelectionFactSnapshot::from_instance_snapshot_with_lists(
            &snapshot,
            now_unix_ms,
            &prepared.policy.record_list_keys(),
        );
        let decision = decide(&prepared.policy, layout, &projection, &facts, now_unix_ms)?;
        let mut record =
            selection_record(prepared, &projection, &snapshot, now_unix_ms, &decision)?;
        // Workflow #308 SEL-L4b: rules 2 and 3 of S0 §3.2 apply to a single-frame select that
        // declares `on_empty`, whose record then carries `unknown_check` (§1.9).
        let unknown_check = match &prepared.on_empty_key {
            Some(_) => Some(single_frame_unknown_check(
                prepared,
                layout,
                &projection,
                &decision,
                (&facts, now_unix_ms),
            )?),
            None => None,
        };
        record.unknown_check = unknown_check;
        let not_attempted = TaskSelectionConfirmation::NotAttempted;
        let (confirmation, result): Attempted<R::Error> = match (chosen(&decision)?, unknown_check)
        {
            (_, Some(check)) if check.blocking != TaskSelectionBlocking::None => (
                not_attempted,
                Err(result_blocked(&decision, check.blocking, check.unknown_instances).into()),
            ),
            (None, Some(_)) if matches!(decision.outcome, SelectionOutcome::Empty) => (
                not_attempted,
                Ok(SelectAttempt::Empty {
                    outcome_key: prepared.on_empty_key.clone().unwrap_or_default(),
                    page_label: observation.page_label.clone(),
                }),
            ),
            (None, _) => (
                not_attempted,
                Err(ContainedTaskError::with_detail(
                    SELECTION_NOT_SELECTED,
                    format!(
                        "{}:{}",
                        outcome_kind(&decision.outcome),
                        decision.outcome_key
                    ),
                )
                .into()),
            ),
            (Some(candidate_id), _) => {
                let (confirmation, selected) = self.confirm(
                    runtime,
                    operation,
                    &projection,
                    candidate_id,
                    timing,
                    deadline,
                    sampling,
                )?;
                (
                    confirmation,
                    selected.map(|selected| SelectAttempt::Selected(Box::new(selected))),
                )
            }
        };
        record.confirmation = confirmation;
        let run_ending = matches!(result, Err(ContainedTaskRunError::Boundary(_)));
        if !run_ending {
            record.validate_for_append().map_err(|error| {
                ContainedTaskError::with_detail(error.code(), format!("field={}", error.field()))
            })?;
        }
        let recorded = runtime.record(ContainedTaskTrace::SelectionEvaluated {
            step_index,
            operation_label: operation.id.clone(),
            selection: Box::new(record),
            run_ending,
        });
        let now = runtime.observation_now();
        match recorded {
            Err(error) if !run_ending => Err(ContainedTaskRunError::Boundary(error)),
            Ok(()) if result.is_ok() && sampling_budget.is_some() && now >= deadline => {
                Err(ContainedTaskError::new(
                    ExecutionKernelCode::CandidateSampleDeadlineExceeded.code(),
                )
                .into())
            }
            // A `run_ending` record's failure stays with the runtime; the capture's error is
            // returned unchanged.
            _ => result,
        }
    }

    /// The confirmation of `candidate_id` on a fresh frame, captured and recognized like any
    /// frame but feeding neither stability sampling nor post-admission OCR: it is the step's
    /// page, the step's guard passes on it, and its candidate set hashes like the evaluated
    /// `projection`. A failure of the runtime's capture that it classifies nonfatal or fatal
    /// is the confirmation `capture_failed`, returned unchanged after the record; any other
    /// boundary failure returns as the outer error.
    #[allow(clippy::too_many_arguments)]
    fn confirm<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        projection: &CandidateProjection,
        candidate_id: &str,
        timing: ContainedTaskTimingContext,
        deadline: Instant,
        sampling: (&SampleRetries, Duration),
    ) -> Result<Confirmation<R::Error>, ContainedTaskRunError<R::Error>> {
        // Workflow #386 SR-1: a failure's outcome as the record names it, a sampling timeout with
        // the retries made so far.
        let (retries, window) = sampling;
        let outcome_of = |error: &ContainedTaskError| retries.outcome(error, window);
        let prepared = operation
            .prepared_select
            .as_deref()
            .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID))?;
        let scoped = sampling_budget_ms(operation, &self.evaluator, &self.detector)?.is_some();
        let frame = match self.capture_frame(
            runtime,
            None,
            None,
            timing,
            None,
            scoped.then_some(deadline),
        ) {
            Ok(frame) => frame,
            Err(ContainedTaskRunError::Task(error)) => {
                return Ok((
                    TaskSelectionConfirmation::CaptureFailed {
                        outcome: outcome_of(&error),
                    },
                    Err(error.into()),
                ));
            }
            Err(ContainedTaskRunError::NonfatalOperation(error)) => {
                return Ok((
                    TaskSelectionConfirmation::CaptureFailed {
                        outcome: Outcome::new(SELECTION_CONFIRMATION_CAPTURE_FAILED),
                    },
                    Err(ContainedTaskRunError::NonfatalOperation(error)),
                ));
            }
            // A fatal capture failure ends the run; the decision is still recorded.
            Err(ContainedTaskRunError::Boundary(error))
                if R::classify_error(&error) == ContainedTaskRuntimeErrorClass::Fatal =>
            {
                return Ok((
                    TaskSelectionConfirmation::CaptureFailed {
                        outcome: Outcome::new(SELECTION_CONFIRMATION_CAPTURE_FAILED),
                    },
                    Err(ContainedTaskRunError::Boundary(error)),
                ));
            }
            Err(error) => return Err(error),
        };
        let mut frame = match frame {
            Some(frame)
                if crate::page_anchor_matches(
                    &self.control.game,
                    &frame.page_label,
                    &operation.from,
                ) =>
            {
                frame
            }
            other => {
                let observed =
                    other.map_or_else(|| "<unrecognized>".to_owned(), |frame| frame.page_label);
                return Ok((
                    TaskSelectionConfirmation::PageChanged,
                    Err(ContainedTaskError::with_detail(
                        SELECTION_PAGE_CHANGED,
                        format!(
                            "operation={} from={} observed_page={observed}",
                            operation.id, operation.from
                        ),
                    )
                    .into()),
                ));
            }
        };
        let mut guard = match operation.guard_outcome(
            &self.control,
            &frame,
            &self.evaluator,
            runtime,
            Some(timing),
        ) {
            Ok((guard, _)) => guard,
            Err(ContainedTaskRunError::Task(error)) => {
                return Ok((
                    TaskSelectionConfirmation::GuardFailed {
                        outcome: outcome_of(&error),
                    },
                    Err(error.into()),
                ));
            }
            Err(error) => return Err(error),
        };
        let layout = self
            .evaluator
            .candidate_layout(&prepared.layout_id)
            .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID))?;
        let confirmed =
            match self.project_transaction(runtime, operation, &frame, layout, timing, deadline) {
                Ok((confirmed, last)) => {
                    if let Some(last) = last {
                        frame = last;
                        guard = match operation.guard_outcome(
                            &self.control,
                            &frame,
                            &self.evaluator,
                            runtime,
                            Some(timing),
                        ) {
                            Ok((guard, _)) => guard,
                            Err(ContainedTaskRunError::Task(error)) => {
                                return Ok((
                                    TaskSelectionConfirmation::GuardFailed {
                                        outcome: outcome_of(&error),
                                    },
                                    Err(error.into()),
                                ));
                            }
                            Err(error) => return Err(error),
                        };
                    }
                    confirmed
                }
                Err(ContainedTaskRunError::Task(error)) => {
                    return Ok((
                        TaskSelectionConfirmation::CaptureFailed {
                            outcome: outcome_of(&error),
                        },
                        Err(error.into()),
                    ));
                }
                Err(ContainedTaskRunError::NonfatalOperation(error)) => {
                    return Ok((
                        TaskSelectionConfirmation::CaptureFailed {
                            outcome: Outcome::new(SELECTION_CONFIRMATION_CAPTURE_FAILED),
                        },
                        Err(ContainedTaskRunError::NonfatalOperation(error)),
                    ));
                }
                Err(ContainedTaskRunError::Boundary(error))
                    if R::classify_error(&error) == ContainedTaskRuntimeErrorClass::Fatal =>
                {
                    return Ok((
                        TaskSelectionConfirmation::CaptureFailed {
                            outcome: Outcome::new(SELECTION_CONFIRMATION_CAPTURE_FAILED),
                        },
                        Err(ContainedTaskRunError::Boundary(error)),
                    ));
                }
                Err(error) => return Err(error),
            };
        let candidate_set_sha256 = confirmed.candidate_set_sha256().to_owned();
        if candidate_set_sha256 != projection.candidate_set_sha256() {
            return Ok((
                TaskSelectionConfirmation::Mismatched {
                    candidate_set_sha256: candidate_set_sha256.clone(),
                    projection: (!confirmed.recognition_evidence().is_empty())
                        .then(|| Box::new(confirmed.clone())),
                },
                Err(ContainedTaskError::with_detail(
                    SELECTION_PROJECTION_MISMATCH,
                    format!(
                        "operation={} evaluated={} confirmation={candidate_set_sha256}",
                        operation.id,
                        projection.candidate_set_sha256()
                    ),
                )
                .into()),
            ));
        }
        // Equal hashes cover every candidate's ID and click rectangle, so the confirmation
        // frame's rectangle is the evaluated one.
        let selected: Result<SelectedCandidate, ContainedTaskRunError<R::Error>> = confirmed
            .candidate(candidate_id)
            .map(|candidate| SelectedCandidate {
                guard,
                click: ClickRect {
                    x: candidate.click.x,
                    y: candidate.click.y,
                    width: candidate.click.width,
                    height: candidate.click.height,
                },
                frame,
                action_seed: None,
                identity: prepared.identity_feature().and_then(|name| {
                    match candidate.features.get(name) {
                        Some(CandidateFeature::Identity { value, .. }) => Some(value.clone()),
                        _ => None,
                    }
                }),
            })
            .ok_or_else(|| {
                ContainedTaskError::with_detail(
                    SELECTION_EVALUATION_FAILED,
                    format!("selected candidate {candidate_id} is not in the projection"),
                )
                .into()
            });
        Ok((
            TaskSelectionConfirmation::Matched {
                candidate_set_sha256,
                projection: (!confirmed.recognition_evidence().is_empty())
                    .then(|| Box::new(confirmed.clone())),
            },
            selected,
        ))
    }
}

impl PreparedContainedTask {
    /// The instance fact snapshot and the instant one select step evaluates with: a valid
    /// snapshot of the package's own game and server, else `selection_state_unavailable` or
    /// `selection_fact_context_mismatch`.
    fn pinned_state<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        step_index: u32,
    ) -> Result<(InstanceFactSnapshot, u64), ContainedTaskRunError<R::Error>> {
        let state = runtime
            .selection_state(SelectionStateRequest {
                step_index,
                operation_label: operation.id.clone(),
                game: self.control.game.clone(),
                server: self.control.server.clone(),
            })
            .map_err(ContainedTaskRunError::operation::<R>)?;
        let SelectionState::Snapshot {
            snapshot,
            now_unix_ms,
        } = state
        else {
            return Err(ContainedTaskError::with_detail(
                SELECTION_STATE_UNAVAILABLE,
                format!("operation={}", operation.id),
            )
            .into());
        };
        if now_unix_ms == 0 || snapshot.validate().is_err() {
            return Err(ContainedTaskError::with_detail(
                SELECTION_STATE_UNAVAILABLE,
                format!(
                    "operation={} the runtime's fact snapshot or instant is invalid",
                    operation.id
                ),
            )
            .into());
        }
        if snapshot.context.game_id != self.control.game
            || snapshot.context.server_id != self.control.server
        {
            return Err(ContainedTaskError::with_detail(
                SELECTION_FACT_CONTEXT_MISMATCH,
                format!(
                    "operation={} snapshot game={} server={}, package game={} server={}",
                    operation.id,
                    snapshot.context.game_id,
                    snapshot.context.server_id,
                    self.control.game,
                    self.control.server
                ),
            )
            .into());
        }
        Ok((snapshot, now_unix_ms))
    }

    /// Workflow #308 SEL-L4b: one attempt of a select step that declares `scan` (S0 §2.2), from
    /// the step's dispatched frame `first`. `Pin` takes one fact snapshot and one instant for the
    /// whole attempt; the session then scans, decides, returns and confirms (`ScanSession::select`)
    /// within the scan's deadline, `timing`'s. A failure before the decision exists returns
    /// without a record (S0 §3.4 P1); once it exists, exactly one record precedes the return,
    /// with the v2 projection and `unknown_check`. A failure of the runtime itself after the
    /// decision is recorded only when the runtime classifies it fatal (`run_ending`, as a
    /// single-frame confirmation records it); any other returns without a record. A confirmed
    /// member is tapped in its click rectangle, bound to F2.
    fn scan_select_attempt<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        first: &PageObservation,
        (step_index, timing): (u32, ContainedTaskTimingContext),
        (prepared, scan): (&PreparedSelect, &ScanDeclaration),
    ) -> Result<SelectAttempt, ContainedTaskRunError<R::Error>> {
        let layout = self
            .evaluator
            .candidate_layout(&prepared.layout_id)
            .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID))?;
        let (snapshot, now_unix_ms) = self.pinned_state(runtime, operation, step_index)?;
        let facts = SelectionFactSnapshot::from_instance_snapshot_with_lists(
            &snapshot,
            now_unix_ms,
            &prepared.policy.record_list_keys(),
        );
        let deadline = timing.deadline();
        let remaining = deadline.saturating_duration_since(runtime.observation_now());
        let mut session = ScanSession::new(
            scan,
            layout,
            &prepared.policy,
            &facts,
            now_unix_ms,
            (first.scene.width(), first.scene.height()),
            remaining,
        )?;
        let action_seed = runtime
            .action_seed(step_index, &operation.id)
            .map_err(ContainedTaskRunError::operation::<R>)?;
        let mut source = RuntimeScanSource::new(
            self,
            runtime,
            operation,
            scan,
            layout,
            first,
            (step_index, timing, deadline, action_seed),
            false,
        );
        let selection = session.select(&mut source, prepared.on_empty_key.is_some());
        let (latest, guard) = source.into_parts();
        let ScanSelection {
            decision,
            coverage,
            unknown_check,
            confirmation,
            end,
        } = selection?;
        let end = match end {
            ScanSelectEnd::Failed(ContainedTaskRunError::Boundary(error))
                if R::classify_error(&error) != ContainedTaskRuntimeErrorClass::Fatal =>
            {
                return Err(ContainedTaskRunError::Boundary(error));
            }
            end => end,
        };
        let record = scan_selection_record(
            &prepared.path,
            (snapshot.ledger_position, now_unix_ms),
            &decision,
            session.projection(coverage)?,
            unknown_check,
            confirmation,
        )?;
        let result: Result<SelectAttempt, ContainedTaskRunError<R::Error>> = match end {
            ScanSelectEnd::Confirmed { identity, rect } => match (latest, guard) {
                (Some(frame), Some(guard)) => scan_click(layout, rect)
                    .map(|click| {
                        SelectAttempt::Selected(Box::new(SelectedCandidate {
                            guard,
                            click,
                            frame,
                            identity: Some(identity),
                            action_seed: Some(action_seed),
                        }))
                    })
                    .map_err(ContainedTaskRunError::Task),
                _ => Err(ContainedTaskError::with_detail(
                    SELECTION_EVALUATION_FAILED,
                    "scan confirmation frame missing",
                )
                .into()),
            },
            ScanSelectEnd::Empty => Ok(SelectAttempt::Empty {
                outcome_key: prepared.on_empty_key.clone().unwrap_or_default(),
                page_label: latest
                    .map_or_else(|| first.page_label.clone(), |frame| frame.page_label),
            }),
            ScanSelectEnd::Failed(error) => Err(error),
        };
        let run_ending = matches!(result, Err(ContainedTaskRunError::Boundary(_)));
        if !run_ending {
            record.validate_for_append().map_err(|error| {
                ContainedTaskError::with_detail(error.code(), format!("field={}", error.field()))
            })?;
        }
        let recorded = runtime.record(ContainedTaskTrace::SelectionEvaluated {
            step_index,
            operation_label: operation.id.clone(),
            selection: Box::new(record),
            run_ending,
        });
        match recorded {
            Err(error) if !run_ending => Err(ContainedTaskRunError::Boundary(error)),
            _ => result,
        }
    }
}

/// The record of a select over a scan (Workflow #308 SEL-L4b, S0 §1.9, §3.4): the policy at
/// `path`, the pinned ledger position and instant, the decision verbatim, the v2 `projection`,
/// `unknown_check` and `confirmation`. An abort keeps no verdict, because it has no complete
/// breakdown (path rule 6).
pub(super) fn scan_selection_record(
    path: &str,
    (input_ledger_position, now_unix_ms): (u64, u64),
    decision: &SelectionDecision,
    projection: actingcommand_contract::ScanCandidateProjection,
    unknown_check: TaskSelectionUnknownCheck,
    confirmation: TaskSelectionConfirmation,
) -> Result<TaskSelectionRecord, ContainedTaskError> {
    let aborted = matches!(decision.outcome, SelectionOutcome::Unknown { .. });
    Ok(TaskSelectionRecord {
        layout_id: projection.layout_id().to_owned(),
        page_id: projection.page_id().to_owned(),
        projection: projection.into(),
        policy: TaskSelectionPolicy {
            path: path.to_owned(),
            policy_sha256: decision.policy_sha256.clone(),
            policy_id: decision.policy_id.clone(),
        },
        fact_snapshot_id: decision.fact_snapshot_id.clone(),
        input_ledger_position,
        input_fact_revisions: None,
        now_unix_ms,
        input_sha256: decision.input_sha256.clone(),
        selection_result: record_outcome(&decision.outcome)?,
        outcome_key: decision.outcome_key.clone(),
        selected: decision.selected.clone(),
        verdicts: if aborted {
            Vec::new()
        } else {
            record_verdicts(&decision.candidates)?
        },
        reasons: record_reasons(&decision.reasons),
        confirmation,
        unknown_check: Some(unknown_check),
        multi_select: None,
    })
}

/// The click rectangle of a scan's confirmed instance (Workflow #308 SEL-L4b): the layout's
/// `click`, placed by the instance rectangle `rect` on F2 (both relative to the anchor match).
fn scan_click(
    layout: &CandidateLayout,
    rect: CandidateRect,
) -> Result<ClickRect, ContainedTaskError> {
    let placed = layout
        .instance_rect
        .zip(layout.click)
        .and_then(|(instance, click)| {
            Some(ClickRect {
                x: rect.x.checked_sub(instance.x)?.checked_add(click.x)?,
                y: rect.y.checked_sub(instance.y)?.checked_add(click.y)?,
                width: click.width,
                height: click.height,
            })
        });
    placed.ok_or_else(|| {
        ContainedTaskError::with_detail(
            SELECT_INVALID,
            format!("layout={} scan click=unplaced", layout.id),
        )
    })
}

/// Rules 2 and 3 of S0 §3.2 for a single-frame select that declares `on_empty` (Workflow #308
/// SEL-L4b, §1.4, §3.4): its unknown instances are the actionable candidates the kernel kept
/// from the evaluator (an unresolved consensus feature, an identity feature that read no
/// identity) and those the evaluator dropped as unknown, each bounded at its Known identity, or
/// over the whole domain without one, with the pinned `facts` at their instant.
fn single_frame_unknown_check(
    prepared: &PreparedSelect,
    layout: &CandidateLayout,
    projection: &CandidateProjection,
    decision: &SelectionDecision,
    (facts, now_unix_ms): (&SelectionFactSnapshot, u64),
) -> Result<TaskSelectionUnknownCheck, ContainedTaskError> {
    let evaluated = decision
        .candidates
        .iter()
        .filter(|verdict| !matches!(verdict.status, CandidateStatus::UnknownDropped))
        .map(|verdict| verdict.candidate_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let unknown = projection
        .candidates()
        .iter()
        .filter(|candidate| candidate.actionable && !evaluated.contains(candidate.id.as_str()))
        .collect::<Vec<_>>();
    let upper = if unknown.is_empty() {
        None
    } else {
        let (field_domains, domain) =
            bound_inputs(layout, &prepared.policy, prepared.identity_feature()).map_err(
                |detail| {
                    ContainedTaskError::with_detail(SELECT_INVALID, format!("upper_bound {detail}"))
                },
            )?;
        let mut upper = Bound::None;
        for candidate in &unknown {
            let identities = prepared
                .identity_feature()
                .and_then(|name| match candidate.features.get(name) {
                    Some(CandidateFeature::Identity { value, .. }) => Some(vec![value.clone()]),
                    _ => None,
                })
                .unwrap_or_else(|| domain.clone());
            let bound = upper_bound(
                &prepared.policy,
                facts,
                now_unix_ms,
                &identities,
                &field_domains,
            )
            .map_err(|error| {
                ContainedTaskError::with_detail(SELECTION_EVALUATION_FAILED, error.to_string())
            })?;
            upper = upper.max(Bound::from(bound));
        }
        Some(upper)
    };
    let unknown_instances = u32::try_from(unknown.len()).unwrap_or(u32::MAX);
    Ok(TaskSelectionUnknownCheck {
        unknown_instances,
        blocking: decision_blocking(decision, upper, unknown_instances)?,
        upper_bound: record_bound(upper),
    })
}

/// One select step evaluated offline on a saved frame (Workflow #308; the `actinglab select`
/// tool of a later slice uses it). The frame must be recognized as the step's `from` page
/// (`select_page_mismatch` otherwise). The step's layout is projected on it and the step's
/// sealed policy, or `policy_override` checked against the same layout, is evaluated with
/// `facts` at `now_unix_ms`. A dry run's confirmation frame is the same frame, so it matches.
pub fn dry_run_select(
    task: &PreparedContainedTask,
    operation_id: &str,
    frame: &Frame,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
    policy_override: Option<&SelectionPolicy>,
) -> Result<SelectionDryRun, ContainedTaskError> {
    let operation = task
        .program
        .operations
        .iter()
        .find(|operation| operation.id == operation_id)
        .ok_or_else(|| {
            ContainedTaskError::with_detail(
                ExecutionKernelCode::ContainedTaskOperationMissing.code(),
                operation_id,
            )
        })?;
    let prepared = operation.prepared_select.as_deref().ok_or_else(|| {
        ContainedTaskError::with_detail(
            SELECT_INVALID,
            format!("operation={operation_id} is not a select step"),
        )
    })?;
    task.control.resolution.validate_frame(frame)?;
    let scene = scene_from_frame(frame)?;
    let evaluations = task
        .detector
        .evaluate_all(&task.evaluator, &scene)
        .map_err(|error| {
            ContainedTaskError::with_detail(
                SharedCode::ContainedTaskRecognitionFailed.code(),
                error.to_string(),
            )
            .with_ppocr_diagnostics(error.ppocr_diagnostics())
        })?;
    let matched = evaluations
        .iter()
        .filter(|evaluation| evaluation.matched)
        .map(|evaluation| evaluation.page_id.as_str())
        .collect::<Vec<_>>();
    if !matches!(
        matched.as_slice(),
        [page] if crate::page_anchor_matches(&task.control.game, page, &operation.from)
    ) {
        return Err(ContainedTaskError::with_detail(
            SELECT_PAGE_MISMATCH,
            format!(
                "operation={operation_id} from={} matched_pages={}",
                operation.from,
                matched.join(",")
            ),
        ));
    }
    let policy = match policy_override {
        Some(policy) => {
            let layout = task
                .evaluator
                .candidate_layout(&prepared.layout_id)
                .ok_or_else(|| {
                    ContainedTaskError::with_detail(
                        SELECT_INVALID,
                        format!(
                            "layout={} is not in the recognition pack",
                            prepared.layout_id
                        ),
                    )
                })?;
            policy.validate().map_err(|error| {
                ContainedTaskError::with_detail(SELECT_POLICY_INVALID, error.to_string())
            })?;
            check_policy(policy, layout).map_err(|reason| {
                ContainedTaskError::with_detail(SELECT_POLICY_MISMATCH, reason)
            })?;
            policy
        }
        None => &prepared.policy,
    };
    let candidate_projection = project(&task.evaluator, &scene, &prepared.layout_id)?;
    let layout = task
        .evaluator
        .candidate_layout(&prepared.layout_id)
        .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID))?;
    let decision = decide(policy, layout, &candidate_projection, facts, now_unix_ms)?;
    Ok(SelectionDryRun {
        candidate_projection,
        decision,
        policy_override: policy_override.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::CandidateRect;
    use actingcommand_recognition_pack::{AssetResolver, RecognitionPackResult};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[derive(Debug)]
    struct MarkerAssets(Vec<u8>);

    impl AssetResolver for MarkerAssets {
        fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
            assert_eq!(path, "assets/marker.png");
            Ok(self.0.clone())
        }
    }

    /// Deterministic pixel noise: a shifted copy of it correlates with it near zero.
    fn noise(x: u32, y: u32) -> [u8; 3] {
        let mut state = u64::from(y * 64 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
        let mut next = || {
            state ^= state >> 33;
            state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
            state ^= state >> 33;
            (state & 0xff) as u8
        };
        [next(), next(), next()]
    }

    fn frame(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
        let pixel = &pixel;
        Frame::from_pixels(
            width,
            height,
            (0..height)
                .flat_map(|y| (0..width).flat_map(move |x| pixel(x, y)))
                .collect(),
            actingcommand_device::PixelFormat::Rgb8,
            actingcommand_device::CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
    }

    /// Workflow #308 list selector slice L1: a single-frame select step's existing path (policy
    /// admission, projection, decision) over the rows of a `repeated_anchor` layout.
    #[test]
    fn a_single_frame_select_decides_over_the_actionable_rows_of_a_repeated_anchor_layout() {
        let pack = actingcommand_recognition_pack::load_pack_from_json_str(
            &json!({
                "schema_version": "0.7",
                "coordinate_space": {"width": 96, "height": 72},
                "defaults": {"match_metric": "ccoeff_normed"},
                "targets": [
                    {"type": "template", "id": "ui/row_marker", "template_path": "assets/marker.png",
                     "region": "full_frame", "threshold": 0.95},
                    {"type": "color", "id": "state/row_open",
                     "region": {"x": 0, "y": 0, "width": 4, "height": 4}, "expected": [0, 200, 0]}
                ],
                "candidate_layouts": [{
                    "id": "layout/rows", "page_id": "list_page", "kind": "repeated_anchor",
                    "anchor": "ui/row_marker", "max_instances": 8, "order": "top_to_bottom",
                    "instance_rect": {"x": -2, "y": -2, "width": 40, "height": 12},
                    "click": {"x": 10, "y": 0, "width": 20, "height": 6},
                    "features": [{"name": "open", "value": "passed", "target": "state/row_open",
                                  "offset": {"x": 30, "y": 1}}]
                }]
            })
            .to_string(),
        )
        .expect("pack");
        let marker = frame(8, 6, noise).png_for_artifact().expect("marker PNG");
        let evaluator =
            RecognitionEvaluator::with_asset_resolver(pack, Arc::new(MarkerAssets(marker)))
                .expect("evaluator");
        // The first row is cut by the frame's top edge although its open square is drawn; the
        // second row is closed and the third is open.
        let markers = [(4_u32, 1_u32), (4, 30), (50, 30)];
        let open_squares = [(34_u32, 2_u32), (80, 31)];
        let rows = frame(96, 72, |x, y| {
            for (left, top) in markers {
                if (left..left + 8).contains(&x) && (top..top + 6).contains(&y) {
                    return noise(x - left, y - top);
                }
            }
            if open_squares
                .iter()
                .any(|(left, top)| (*left..left + 4).contains(&x) && (*top..top + 4).contains(&y))
            {
                return [0, 200, 0];
            }
            [30, 31, 32]
        });
        let scene = scene_from_frame(&rows).expect("scene");
        let policy: SelectionPolicy = serde_json::from_value(json!({
            "schema_version": "actingcommand.selection-policy.v1",
            "policy_id": "rows-open",
            "applies_to": {
                "candidate_layout_id": "layout/rows",
                "outcome_keys": {"selected": "row_selected", "empty": "row_empty",
                                 "insufficient": "row_insufficient",
                                 "ambiguous": "row_ambiguous", "unknown": "row_unknown"}
            },
            "fields": [{"name": "open", "value_type": {"type": "boolean"}}],
            "facts": [],
            "gates": [{"gate_id": "open",
                       "predicate": {"kind": "boolean_equals",
                                     "value": {"source": "field", "field": "open"},
                                     "expected": true},
                       "on_unknown": {"kind": "drop_candidate"}}],
            "scoring": [],
            "selection": {"mode": "exactly_one", "required_count": 1},
            "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
        }))
        .expect("policy");
        policy.validate().expect("valid policy");
        let layout = evaluator
            .candidate_layout("layout/rows")
            .expect("declared layout");
        check_policy(&policy, layout).expect("the policy applies to the rows");

        let projection = project(&evaluator, &scene, "layout/rows").expect("projection");
        assert_eq!(
            projection
                .candidates()
                .iter()
                .map(|candidate| candidate.actionable)
                .collect::<Vec<_>>(),
            [false, true, true]
        );
        let facts = SelectionFactSnapshot {
            snapshot_id: "empty".into(),
            snapshot_at_unix_ms: 0,
            facts: BTreeMap::new(),
        };
        let decision = decide(&policy, layout, &projection, &facts, 0).expect("decision");
        // Only actionable rows are evaluated: the cut first row, which would read open and win
        // the lowest-ID tie-break, is neither a verdict nor the choice.
        assert_eq!(
            decision
                .candidates
                .iter()
                .map(|verdict| verdict.candidate_id.as_str())
                .collect::<Vec<_>>(),
            ["layout/rows#01", "layout/rows#02"]
        );
        assert_eq!(
            chosen(&decision).expect("one choice"),
            Some("layout/rows#02")
        );
        assert_eq!(
            projection
                .candidate("layout/rows#02")
                .expect("chosen row")
                .click,
            CandidateRect {
                x: 60,
                y: 30,
                width: 20,
                height: 6
            }
        );
    }

    fn identity_layout(unknown_identity: &str) -> CandidateLayout {
        serde_json::from_value(serde_json::json!({
            "id": "layout-list", "page_id": "neutral/home", "kind": "fixed_slots",
            "unknown_identity": unknown_identity,
            "features": [{"name": "member", "value": "identity", "identity": {
                "entries": [
                    {"id": "member-a", "aliases": ["a"]},
                    {"id": "member-b", "aliases": ["b"]},
                    {"id": "member-c", "aliases": ["c"]}
                ],
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}
            }}],
            "slots": [{"rect": {"x": 0, "y": 0, "width": 2, "height": 1},
                       "click": {"x": 0, "y": 0, "width": 2, "height": 1},
                       "targets": {"member": "candidate/name"}}]
        }))
        .expect("layout decodes")
    }

    /// A document that gates and scores on a record list read through `keyed_fact`.
    fn keyed_policy() -> SelectionPolicy {
        let list = serde_json::json!({
            "source": "keyed_fact", "fact_key": "session.example.list.targets",
            "key_field": "member", "value_column": "rank",
            "absent": {"type": "integer", "value": 0}
        });
        serde_json::from_value(serde_json::json!({
            "schema_version": "actingcommand.selection-policy.v1",
            "policy_id": "policy-list",
            "applies_to": {"candidate_layout_id": "layout-list", "outcome_keys": {
                "selected": "list-selected", "empty": "list-empty",
                "insufficient": "list-insufficient", "ambiguous": "list-ambiguous",
                "unknown": "list-unknown"}},
            "fields": [{"name": "member", "value_type": {
                "type": "enum_string", "allowed": ["member-a", "member-b", "member-c"]}}],
            "facts": [{"fact_key": "session.example.list.targets",
                       "value_type": {"type": "record_list", "key_column": "id",
                                      "columns": {"rank": {"type": "integer"}}},
                       "max_age_ms": 3_600_000, "minimum_confidence_milli": 1_000}],
            "gates": [{"gate_id": "gate-listed",
                       "predicate": {"kind": "integer_at_least", "value": list.clone(),
                                     "threshold": 1},
                       "on_unknown": {"kind": "abort_evaluation"}}],
            "scoring": [{"term_id": "term-rank", "value": list,
                         "transform": {"kind": "identity"}, "weight_milli": 1_000,
                         "on_unknown": {"kind": "abort_evaluation"}}],
            "selection": {"mode": "none_allowed", "required_count": 1},
            "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
        }))
        .expect("keyed policy decodes")
    }

    #[test]
    fn a_keyed_fact_document_needs_a_layout_that_rejects_unknown_identities() {
        let policy = keyed_policy();
        policy.validate().expect("keyed policy validates");
        check_policy(&policy, &identity_layout("reject")).expect("a rejecting layout admits it");
        let refusal = check_policy(&policy, &identity_layout("readable_attributes"))
            .expect_err("readable attributes would hand the evaluator an unknown key");
        assert!(
            refusal.contains("unknown_identity=readable_attributes"),
            "{refusal}"
        );

        // The kernel reports the refusal under its code: a dry run checks the same document,
        // given as an override, against the step's layout.
        let task = select_task(identity_layout("readable_attributes"));
        let facts = SelectionFactSnapshot::from_fact_records("snapshot-a", 1_000_000, &[]);
        let error = dry_run_select(
            &task,
            "pick",
            &home_frame(),
            &facts,
            1_000_000,
            Some(&policy),
        )
        .expect_err("the document does not apply to the layout");
        assert_eq!(
            error.code().as_str(),
            "contained_task_select_policy_mismatch"
        );
    }

    /// A prepared task whose one select step `pick` runs from the home page on `layout`, with
    /// the keyed document as its sealed policy.
    fn select_task(layout: CandidateLayout) -> PreparedContainedTask {
        let control: TaskControl = serde_json::from_value(serde_json::json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.semantic.task",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 2
        }))
        .expect("task control");
        let mut program: TaskProgram = serde_json::from_value(serde_json::json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "coordinate_space": {"width": 2, "height": 1},
            "operations": [{
                "id": "pick", "from": "home",
                "select": {"layout_id": "layout-list",
                           "policy": {"path": "policies/list.json", "sha256": "0".repeat(64)}}
            }]
        }))
        .expect("task program");
        program.operations[0].prepared_select = Some(Box::new(PreparedSelect {
            layout_id: "layout-list".to_owned(),
            path: "operations/task/policies/list.json".to_owned(),
            policy: keyed_policy(),
            scan: None,
            on_empty_key: None,
            identity_feature: None,
        }));
        let mut pack: actingcommand_recognition_pack::RecognitionPack =
            serde_json::from_value(serde_json::json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    {"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "expected": [255, 0, 0]},
                    {"type": "ocr", "id": "candidate/name",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
                     "expected": ["a"], "case_sensitive": true, "minimum_confidence": 0.8,
                     "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
                ]
            }))
            .expect("recognition pack");
        pack.candidate_layouts = vec![layout];
        let evaluator = RecognitionEvaluator::new(std::path::PathBuf::new(), pack)
            .expect("recognition evaluator");
        let page_set: actingcommand_page_detector::PageSet =
            serde_json::from_value(serde_json::json!({
                "schema_version": "0.3",
                "pages": [{"id": "neutral/home", "required": ["page/home"]}]
            }))
            .expect("page set");
        let detector =
            actingcommand_page_detector::PageDetector::new(page_set).expect("page detector");
        PreparedContainedTask {
            control,
            program,
            evaluator,
            detector,
            entry_page: None,
            scheduling_outcome: None,
            post_admission_ocr: None,
            post_admission_fields: None,
            entry_count: 1,
            task_count: 1,
        }
    }

    /// The home page of [`select_task`].
    fn home_frame() -> Frame {
        Frame::from_pixels(
            2,
            1,
            [255, 0, 0, 0, 0, 0].to_vec(),
            actingcommand_device::PixelFormat::Rgb8,
            actingcommand_device::CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
    }
}
