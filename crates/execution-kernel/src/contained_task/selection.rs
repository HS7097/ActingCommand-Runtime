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

use super::{
    ClickRect, ContainedTaskError, ContainedTaskGuardOutcome, ContainedTaskRunError,
    ContainedTaskRuntime, ContainedTaskRuntimeErrorClass, ContainedTaskTimingContext,
    ContainedTaskTrace, PageObservation, PreparedContainedTask, Resolution, TaskControl,
    TaskOperation, TaskProgram, safe_task_local_path, sampled_tap, scene_from_frame,
};
use actingcommand_contract::{
    CandidateFeature, CandidateProjection, InputAction, InputSamplingEvidence,
    InstanceFactSnapshot, TaskSelectionConfirmation, TaskSelectionPolicy, TaskSelectionRecord,
    validate_candidate_layout_id,
};
use actingcommand_device::Frame;
use actingcommand_pack_containment::{LoadedBundle, Sha256Hash};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    CandidateFeatureValue, CandidateLayout, RecognitionEvaluator, UnknownIdentityHandling,
};
use actingcommand_selection_policy::{
    Candidate, GateUnknownHandling, MAX_DOCUMENT_BYTES, ScalarValue, SelectionDecision,
    SelectionFactSnapshot, SelectionOutcome, SelectionPolicy, TermUnknownHandling, ValueRef,
    ValueType, evaluate, parse_canonical_json,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

const SELECT_INVALID: &str = "contained_task_select_invalid";
const SELECT_POLICY_MISSING: &str = "contained_task_select_policy_missing";
const SELECT_POLICY_HASH_MISMATCH: &str = "contained_task_select_policy_hash_mismatch";
const SELECT_POLICY_INVALID: &str = "contained_task_select_policy_invalid";
const SELECT_POLICY_MISMATCH: &str = "contained_task_select_policy_mismatch";
const SELECTION_STATE_UNAVAILABLE: &str = "selection_state_unavailable";
const SELECTION_FACT_CONTEXT_MISMATCH: &str = "selection_fact_context_mismatch";
const SELECTION_EVALUATION_FAILED: &str = "selection_evaluation_failed";
const SELECTION_NOT_SELECTED: &str = "selection_not_selected";
const SELECTION_PAGE_CHANGED: &str = "selection_page_changed";
const SELECTION_PROJECTION_MISMATCH: &str = "selection_projection_mismatch";
/// The confirmation code a record carries when the runtime's own capture of the confirmation
/// frame fails, nonfatally or fatally; the task then fails with the runtime's error.
const SELECTION_CONFIRMATION_CAPTURE_FAILED: &str = "selection_confirmation_capture_failed";
/// A dry run's frame is not the step's page.
const SELECT_PAGE_MISMATCH: &str = "select_page_mismatch";

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

/// The `select` of a task operation: `{layout_id, policy: {path, sha256}}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskSelect {
    layout_id: String,
    policy: TaskSelectPolicy,
}

/// The task-local policy document a select step names, sealed by the SHA-256 of its bytes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskSelectPolicy {
    path: String,
    sha256: String,
}

impl TaskSelect {
    /// The layout ID has the candidate layout grammar; the document is a `policies/<name>.json`
    /// path of the task directory with a lowercase hexadecimal SHA-256.
    pub(super) fn validate(&self, operation_id: &str) -> Result<(), ContainedTaskError> {
        let sealed = self.policy.sha256.len() == 64
            && self
                .policy
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if validate_candidate_layout_id(&self.layout_id).is_err()
            || !safe_task_local_path(&self.policy.path)
            || !self.policy.path.starts_with("policies/")
            || !self.policy.path.ends_with(".json")
            || !sealed
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
    /// The SHA-256 of the document's bytes, as declared and compared at admission.
    package_sha256: String,
    policy: SelectionPolicy,
}

/// A select step's confirmed candidate: the guard outcome on the confirmation frame, the
/// candidate's click rectangle and the confirmation frame, whose input context binds the tap.
pub(super) struct SelectedCandidate {
    pub(super) guard: ContainedTaskGuardOutcome,
    click: ClickRect,
    pub(super) frame: PageObservation,
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
        action
            .validate()
            .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))?;
        if let InputAction::Tap { x, y } = &action {
            resolution.validate_point(*x, *y)?;
        }
        Ok((action, sampling))
    }
}

/// Contained task admission of every select step, after the program and the page set are
/// validated: the layout is a candidate layout of the recognition pack on the step's `from`
/// page; the policy document is present in the package, its bytes hash to the declared
/// SHA-256 (directly, whatever the package's manifest form), it reads as a selection-policy
/// document, and it applies to the layout (`contracts/selection-graph.md`, section Select
/// step).
pub(super) fn prepare_select_steps(
    program: &mut TaskProgram,
    control: &TaskControl,
    bundle: &LoadedBundle,
    evaluator: &RecognitionEvaluator,
) -> Result<(), ContainedTaskError> {
    let task_id = program.task_id.clone();
    for operation in &mut program.operations {
        let Some(select) = &operation.select else {
            continue;
        };
        let failure = |code: &'static str, detail: String| {
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
        if layout.has_consensus() {
            let required_ms = layout
                .maximum_provider_and_wait_ms(evaluator)
                .saturating_mul(2);
            if required_ms
                > control
                    .step_timeout()
                    .milliseconds
                    .min(control.task_timeout().milliseconds)
            {
                return Err(failure(
                    SELECT_INVALID,
                    format!(
                        "layout={} H1/H2 declared provider and wait budget {required_ms}ms exceeds the original task/step budget",
                        layout.id
                    ),
                ));
            }
        }
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
        if Sha256Hash::digest(bytes).to_string() != select.policy.sha256 {
            return Err(failure(
                SELECT_POLICY_HASH_MISMATCH,
                format!("policy={path}"),
            ));
        }
        let policy = read_policy(bytes)
            .map_err(|reason| failure(SELECT_POLICY_INVALID, format!("policy={path} {reason}")))?;
        check_policy(&policy, layout)
            .map_err(|reason| failure(SELECT_POLICY_MISMATCH, format!("policy={path} {reason}")))?;
        let prepared = PreparedSelect {
            layout_id: select.layout_id.clone(),
            path,
            package_sha256: select.policy.sha256.clone(),
            policy,
        };
        operation.prepared_select = Some(Box::new(prepared));
    }
    Ok(())
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
/// exactly one candidate.
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
    if layout
        .features
        .iter()
        .any(|feature| feature.identity.is_some())
        && policy.gates.iter().any(|gate| {
            matches!(
                gate.on_unknown,
                GateUnknownHandling::SubstituteVerdict { passes: true }
            )
        })
    {
        return Err("identity selection requires fail-closed hard gates".into());
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
        projection: projection.clone(),
        policy: TaskSelectionPolicy {
            path: prepared.path.clone(),
            package_sha256: prepared.package_sha256.clone(),
            policy_sha256: decision.policy_sha256.clone(),
            policy_id: decision.policy_id.clone(),
        },
        fact_snapshot_id: decision.fact_snapshot_id.clone(),
        input_ledger_position: snapshot.ledger_position,
        now_unix_ms,
        input_sha256: decision.input_sha256.clone(),
        outcome: mirror(&decision.outcome)?,
        outcome_key: decision.outcome_key.clone(),
        selected: decision.selected.clone(),
        verdicts: mirror(&decision.candidates)?,
        reasons: mirror(&decision.reasons)?,
        confirmation: TaskSelectionConfirmation::NotAttempted,
    })
}

/// How a confirmation ended: the record's `confirmation`, and the candidate to tap or the
/// failure the attempt returns once the record is written.
type Confirmation<E> = (
    TaskSelectionConfirmation,
    Result<SelectedCandidate, ContainedTaskRunError<E>>,
);

impl PreparedContainedTask {
    fn project_transaction<R: ContainedTaskRuntime>(
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
        let check_deadline = || {
            if Instant::now() >= deadline {
                Err(ContainedTaskError::new(
                    "candidate_sample_deadline_exceeded",
                ))
            } else {
                Ok(())
            }
        };
        check_deadline()?;
        if layout.has_consensus() {
            if !runtime
                .candidate_sampling_checkpoint()
                .map_err(ContainedTaskRunError::operation::<R>)?
            {
                return Err(
                    ContainedTaskError::new("candidate_sampling_runtime_unsupported").into(),
                );
            }
            if Duration::from_millis(layout.maximum_provider_and_wait_ms(&self.evaluator))
                > deadline.saturating_duration_since(Instant::now())
            {
                return Err(ContainedTaskError::new("candidate_sample_budget_insufficient").into());
            }
        }
        let mut frames = Vec::with_capacity(layout.required_frames().saturating_sub(1));
        for _ in 1..layout.required_frames() {
            check_deadline()?;
            let interval = Duration::from_millis(u64::from(layout.sample_interval_ms));
            if interval > deadline.saturating_duration_since(Instant::now()) {
                return Err(ContainedTaskError::new("candidate_sample_budget_insufficient").into());
            }
            let boundary = actingcommand_contract::TaskTimingBoundary::PageRecognitionWait;
            let identity = runtime.task_boundary_identity(boundary);
            let started = Instant::now();
            std::thread::sleep(interval);
            let ended = Instant::now();
            runtime.observe_task_boundary(super::ContainedTaskBoundaryTiming {
                boundary,
                identity,
                context: timing,
                started,
                ended,
                succeeded: true,
            });
            check_deadline()?;
            let frame = self
                .capture_frame(runtime, None, None, timing)?
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
                return Err(
                    ContainedTaskError::new("candidate_sample_page_or_geometry_changed").into(),
                );
            }
            frames.push(frame);
        }
        let scenes = std::iter::once(&first.scene)
            .chain(frames.iter().map(|frame| &frame.scene))
            .collect::<Vec<_>>();
        let mut recording_error = None;
        let projection = self.evaluator.scene_context(scenes[scenes.len() - 1]).project_candidates_with_samples(
            &layout.id, &scenes, &mut |target, result, started, ended| {
                let measured = super::ContainedTaskEvaluationTiming {
                    elapsed_us: super::observe_instant_span(started, ended), budget_before: timing.budget_at(started),
                    result: if result.is_ok() { actingcommand_contract::TaskTimingResult::Ok } else { actingcommand_contract::TaskTimingResult::Err },
                };
                if let Err(error) = runtime.record_candidate_evaluation(target, result, measured) {
                    recording_error = Some(error);
                    return Err(actingcommand_recognition_pack::CandidateProjectionFailure::recording_failed("runtime rejected candidate sample recording"));
                }
                check_deadline().map_err(|error| actingcommand_recognition_pack::CandidateProjectionFailure::recording_failed(error.code()))
            });
        if let Some(error) = recording_error {
            return Err(ContainedTaskRunError::Boundary(error));
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
        check_deadline()?;
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
    pub(super) fn select_attempt<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        observation: &PageObservation,
        step_index: u32,
        timing: ContainedTaskTimingContext,
    ) -> Result<SelectedCandidate, ContainedTaskRunError<R::Error>> {
        let prepared = operation.prepared_select.as_deref().ok_or_else(|| {
            ContainedTaskError::with_detail(
                SELECT_INVALID,
                format!("operation={} has no admitted select step", operation.id),
            )
        })?;
        let layout = self
            .evaluator
            .candidate_layout(&prepared.layout_id)
            .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID))?;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(
                self.control.step_timeout().milliseconds,
            ))
            .ok_or_else(|| ContainedTaskError::new("candidate_sample_deadline_overflow"))?
            .min(timing.deadline());
        let (projection, _) =
            self.project_transaction(runtime, operation, observation, layout, timing, deadline)?;
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
        let facts = SelectionFactSnapshot::from_instance_snapshot(&snapshot, now_unix_ms);
        let decision = decide(&prepared.policy, layout, &projection, &facts, now_unix_ms)?;
        let mut record =
            selection_record(prepared, &projection, &snapshot, now_unix_ms, &decision)?;
        let (confirmation, result): Confirmation<R::Error> = match chosen(&decision)? {
            None => (
                TaskSelectionConfirmation::NotAttempted,
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
            Some(candidate_id) => self.confirm(
                runtime,
                operation,
                prepared,
                &projection,
                candidate_id,
                timing,
                deadline,
            )?,
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
        match recorded {
            Err(error) if !run_ending => Err(ContainedTaskRunError::Boundary(error)),
            Ok(()) if result.is_ok() && layout.has_consensus() && Instant::now() >= deadline => {
                Err(ContainedTaskError::new("candidate_sample_deadline_exceeded").into())
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
    fn confirm<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        prepared: &PreparedSelect,
        projection: &CandidateProjection,
        candidate_id: &str,
        timing: ContainedTaskTimingContext,
        deadline: Instant,
    ) -> Result<Confirmation<R::Error>, ContainedTaskRunError<R::Error>> {
        let frame = match self.capture_frame(runtime, None, None, timing) {
            Ok(frame) => frame,
            Err(ContainedTaskRunError::Task(error)) => {
                return Ok((
                    TaskSelectionConfirmation::CaptureFailed {
                        code: error.code().to_owned(),
                    },
                    Err(error.into()),
                ));
            }
            Err(ContainedTaskRunError::NonfatalOperation(error)) => {
                return Ok((
                    TaskSelectionConfirmation::CaptureFailed {
                        code: SELECTION_CONFIRMATION_CAPTURE_FAILED.to_owned(),
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
                        code: SELECTION_CONFIRMATION_CAPTURE_FAILED.to_owned(),
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
        let mut guard =
            match operation.guard_outcome(&self.control, &frame, &self.evaluator, runtime) {
                Ok((guard, _)) => guard,
                Err(ContainedTaskRunError::Task(error)) => {
                    return Ok((
                        TaskSelectionConfirmation::GuardFailed {
                            code: error.code().to_owned(),
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
                        ) {
                            Ok((guard, _)) => guard,
                            Err(ContainedTaskRunError::Task(error)) => {
                                return Ok((
                                    TaskSelectionConfirmation::GuardFailed {
                                        code: error.code().to_owned(),
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
                            code: error.code().to_owned(),
                        },
                        Err(error.into()),
                    ));
                }
                Err(ContainedTaskRunError::NonfatalOperation(error)) => {
                    return Ok((
                        TaskSelectionConfirmation::CaptureFailed {
                            code: SELECTION_CONFIRMATION_CAPTURE_FAILED.to_owned(),
                        },
                        Err(ContainedTaskRunError::NonfatalOperation(error)),
                    ));
                }
                Err(ContainedTaskRunError::Boundary(error))
                    if R::classify_error(&error) == ContainedTaskRuntimeErrorClass::Fatal =>
                {
                    return Ok((
                        TaskSelectionConfirmation::CaptureFailed {
                            code: SELECTION_CONFIRMATION_CAPTURE_FAILED.to_owned(),
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
            ContainedTaskError::with_detail("contained_task_operation_missing", operation_id)
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
            ContainedTaskError::with_detail("contained_task_recognition_failed", error.to_string())
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
