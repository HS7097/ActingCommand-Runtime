// SPDX-License-Identifier: AGPL-3.0-only

//! Zero-device adapter for evaluating contained tasks against recorded frames.

use crate::codes::ExecutionKernelCode;
use crate::{
    ContainedTaskCode, ContainedTaskGuardOutcome, ContainedTaskRunError, ContainedTaskRunOptions,
    ContainedTaskRuntime, ContainedTaskTrace, InputFrameContext, ObservedFrame,
    PreparedContainedTask, ResourceReadingProblem, RunParameters, SelectionState,
    SelectionStateRequest, TouchSampling,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Detail, Outcome};
use actingcommand_contract::{
    InputAction, InputSamplingEvidence, InstanceFactContext, InstanceFactSnapshot, OcrFieldPrivacy,
    OcrFieldsReport,
};
use actingcommand_device::Frame;
use actingcommand_pack_containment::Sha256Hash;
use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

const FIXTURE_EXHAUSTED_CODE: Code = ExecutionKernelCode::OfflineFixtureExhausted.code();
/// The instance an offline select step's empty fact snapshot names.
const OFFLINE_INSTANCE: &str = "offline.simulation";
/// The empty fact snapshot of an offline select step. An offline run has no ledger; the
/// snapshot names the first position, and its record is never appended anywhere.
const OFFLINE_SNAPSHOT_ID: &str = "snapshot:offline:empty";
const OFFLINE_SNAPSHOT_POSITION: u64 = 1;
/// Workflow #406 T4: the domain of an offline action seed ([`offline_action_seed`]).
const OFFLINE_ACTION_SEED_DOMAIN: &[u8] = b"ActingCommand offline action seed v1\0";

/// Workflow #406 T4 (plan Q20, Q14; `contracts/resource-declarations.md`, "The Lab's draw"): how
/// an offline run draws the points of its boxes, as the Runtime draws a run's: from the run's
/// seed (`--seed` when given, else 0), each step from its own action seed
/// ([`offline_action_seed`]), with the touch keys of the root configuration the Lab reads. An
/// offline run has no landing history, so a single-region press draws one centre-hotspot point
/// (`center_hotspot_normal_v1`); without touch keys every box is drawn uniformly
/// (`xorshift64_uniform_rect_v1`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OfflineDraw {
    /// The run's seed.
    pub seed: u64,
    /// The touch keys in effect; `None` without a touch configuration.
    pub touch: Option<TouchSampling>,
}

impl OfflineDraw {
    /// The touch keys a step is handed: those in effect with one candidate, since an offline run
    /// keeps no landing history.
    fn step_touch(self) -> Option<TouchSampling> {
        self.touch.map(|touch| TouchSampling {
            candidate_count: 1,
            ..touch
        })
    }
}

/// Workflow #406 T4: the action seed of the step `step_index` with the operation
/// `operation_label` of an offline run with the seed `seed`, as the Runtime derives a step's from
/// its run seed: a domain-separated SHA-256 over the run seed, the step index and the operation
/// label, the first eight bytes of the digest read big-endian. The bytes hashed are the domain
/// `ActingCommand offline action seed v1` and a NUL, `seed` (eight bytes, big-endian),
/// `step_index` (four bytes, big-endian), the label's length in bytes (eight bytes, big-endian)
/// and the label's UTF-8 bytes. The Lab derives the seed of each press it draws the same way.
pub fn offline_action_seed(seed: u64, step_index: u32, operation_label: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(OFFLINE_ACTION_SEED_DOMAIN);
    hasher.update(seed.to_be_bytes());
    hasher.update(step_index.to_be_bytes());
    hasher.update((operation_label.len() as u64).to_be_bytes());
    hasher.update(operation_label.as_bytes());
    let digest = hasher.finalize();
    let mut first = [0_u8; 8];
    first.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(first)
}

/// Workflow #406 T4: whether an effect drew its points from the run's seed: it carries sampling
/// evidence, or it is a path drag, whose points the kernel generates from the step's seed.
fn drew_from_seed(action: &InputAction, sampling: Option<&InputSamplingEvidence>) -> bool {
    sampling.is_some() || matches!(action, InputAction::SingleTouchPathDragV1 { .. })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OfflineRecognitionResult {
    pub candidate_pages: Vec<String>,
    pub matched_page: Option<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OfflineDecision {
    WouldClick {
        operation_label: String,
        action: InputAction,
        /// Workflow #406 T4: how the click's points were drawn, the evidence as the ledger
        /// records an effect intent's (algorithm, action seed, regions, parameters); absent for
        /// a click that draws no box point (a `point` click, a path drag).
        #[serde(skip_serializing_if = "Option::is_none")]
        sampling: Option<InputSamplingEvidence>,
        guard: ContainedTaskGuardOutcome,
    },
    WouldComplete {
        final_page: Option<String>,
    },
    NoOp {
        final_page: Option<String>,
    },
    Refused {
        #[serde(serialize_with = "code_spelling")]
        code: ContainedTaskCode,
        detail: Option<String>,
    },
}

/// A refusal code in serialized output as its spelling.
fn code_spelling<S: Serializer>(
    code: &ContainedTaskCode,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(code.as_str())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OfflineSimulationResult {
    pub mode: &'static str,
    pub executed: bool,
    /// The package's identity, its `package_id` (Workflow #336 PKG-S1: no package digest).
    pub package_id: String,
    pub decision_fingerprint: String,
    pub task_id: String,
    pub entry_count: usize,
    pub task_count: usize,
    pub capture_count: usize,
    pub post_admission_ocr: OfflinePostAdmissionOcrStatus,
    pub recognition: Vec<OfflineRecognitionResult>,
    pub decision: OfflineDecision,
    /// The instant a select step evaluated at (Workflow #308): the caller's, read when the
    /// dry run started. Present only when a select step was evaluated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_now_unix_ms: Option<u64>,
    /// Workflow #406 T4: the run's seed, present when the decision's click drew its points from
    /// it; a run given this seed draws the same points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OfflinePostAdmissionOcrStatus {
    pub declared: bool,
    pub executed: bool,
    pub pending_real_execution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineSimulationError {
    code: Code,
    detail: Option<String>,
}

impl OfflineSimulationError {
    fn new(code: impl Into<Code>) -> Self {
        Self {
            code: code.into(),
            detail: None,
        }
    }

    fn with_detail(code: impl Into<Code>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: Some(detail.into()),
        }
    }

    pub const fn code(&self) -> Code {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for OfflineSimulationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "offline simulation error {}", self.code)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for OfflineSimulationError {}

/// Runs the production contained-task interpreter while intercepting its first effect intent.
/// No device backend can be injected through this boundary, and `input` never performs an effect.
/// Without an instant a select step is refused with `selection_state_unavailable`; see
/// [`simulate_contained_task_at`].
pub fn simulate_contained_task(
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
) -> Result<OfflineSimulationResult, OfflineSimulationError> {
    simulate(task, frames, None, OfflineDraw::default())
}

/// [`simulate_contained_task`] with the caller's instant for a select step (Workflow #308): the
/// step evaluates with an empty fact snapshot at `now_unix_ms`, confirms on the same saved
/// frame it was evaluated on, and the result carries the instant.
pub fn simulate_contained_task_at(
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
    now_unix_ms: u64,
) -> Result<OfflineSimulationResult, OfflineSimulationError> {
    simulate(task, frames, Some(now_unix_ms), OfflineDraw::default())
}

/// Workflow #406 T4: [`simulate_contained_task`] (with the caller's instant for a select step
/// when given) drawing every box point as `draw` says: the run's seed and the touch keys in
/// effect. The plain forms draw with the seed 0 and no touch keys.
pub fn simulate_contained_task_drawn(
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
    now_unix_ms: Option<u64>,
    draw: OfflineDraw,
) -> Result<OfflineSimulationResult, OfflineSimulationError> {
    simulate(task, frames, now_unix_ms, draw)
}

fn simulate(
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
    now_unix_ms: Option<u64>,
    draw: OfflineDraw,
) -> Result<OfflineSimulationResult, OfflineSimulationError> {
    let mut runtime = OfflineRuntime::new(
        frames,
        now_unix_ms,
        task.has_select_steps(),
        task.default_run_parameters(),
        draw,
    );
    let decision = if runtime.frames.is_empty() {
        OfflineDecision::Refused {
            code: SharedCode::OfflineFixtureMissing.code().into(),
            detail: None,
        }
    } else {
        match task.run_with_options(&mut runtime, ContainedTaskRunOptions::offline_simulation()) {
            Ok(outcome)
                if outcome.executed_steps == 0 && task.execution_mode() == "recognize_only" =>
            {
                OfflineDecision::NoOp {
                    final_page: outcome.final_page,
                }
            }
            Ok(outcome) if outcome.executed_steps == 0 => OfflineDecision::WouldComplete {
                final_page: outcome.final_page,
            },
            Ok(_) => {
                return Err(OfflineSimulationError::new(
                    ExecutionKernelCode::OfflineSimulationExecutedStepInvariant.code(),
                ));
            }
            Err(
                ContainedTaskRunError::Boundary(OfflineBoundary::EffectIntercepted)
                | ContainedTaskRunError::NonfatalOperation(OfflineBoundary::EffectIntercepted),
            ) => runtime
                .planned
                .take()
                .map(|planned| OfflineDecision::WouldClick {
                    operation_label: planned.operation_label,
                    action: planned.action,
                    sampling: planned.sampling,
                    guard: planned.guard,
                })
                .ok_or_else(|| {
                    OfflineSimulationError::new(
                        ExecutionKernelCode::OfflineSimulationEffectIntentMissing.code(),
                    )
                })?,
            Err(
                ContainedTaskRunError::Boundary(OfflineBoundary::FixtureExhausted)
                | ContainedTaskRunError::NonfatalOperation(OfflineBoundary::FixtureExhausted),
            ) => OfflineDecision::Refused {
                code: FIXTURE_EXHAUSTED_CODE.into(),
                detail: None,
            },
            Err(
                ContainedTaskRunError::Boundary(OfflineBoundary::Invariant(code))
                | ContainedTaskRunError::NonfatalOperation(OfflineBoundary::Invariant(code)),
            ) => {
                return Err(OfflineSimulationError::new(code));
            }
            Err(ContainedTaskRunError::Task(error)) => OfflineDecision::Refused {
                code: error.code(),
                detail: error.detail().map(str::to_string),
            },
        }
    };

    let seed = match &decision {
        OfflineDecision::WouldClick {
            action, sampling, ..
        } if drew_from_seed(action, sampling.as_ref()) => Some(draw.seed),
        _ => None,
    };
    let package_id = task.package_label().to_string();
    let task_id = task.task_label().to_string();
    let post_admission_ocr_declared = task.has_post_admission_ocr();
    let decision_fingerprint = fingerprint_decision(
        &package_id,
        &task_id,
        &runtime.recognition,
        &decision,
        runtime.selection_now_unix_ms,
    )?;
    Ok(OfflineSimulationResult {
        mode: "offline_simulation",
        executed: false,
        package_id,
        decision_fingerprint,
        task_id,
        entry_count: task.entry_count(),
        task_count: task.task_count(),
        capture_count: runtime.capture_count,
        post_admission_ocr: OfflinePostAdmissionOcrStatus {
            declared: post_admission_ocr_declared,
            executed: false,
            // Declared OCR fields, resource readings and resource adjustments (Workflow #384
            // D2-S3) are all read only by a real run.
            pending_real_execution: post_admission_ocr_declared
                || task.has_resource_readings()
                || task.has_resource_adjustments(),
        },
        recognition: runtime.recognition,
        decision,
        selection_now_unix_ms: runtime.selection_now_unix_ms,
        seed,
    })
}

#[derive(Serialize)]
struct DecisionFingerprintProjection<'a> {
    schema_version: &'static str,
    /// The package by its `package_id` only (Workflow #336 PKG-S1).
    package_id: &'a str,
    task_id: &'a str,
    recognition: &'a [OfflineRecognitionResult],
    decision: &'a OfflineDecision,
    /// Bound only when a select step evaluated, so every other fingerprint is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_now_unix_ms: Option<u64>,
}

fn fingerprint_decision(
    package_id: &str,
    task_id: &str,
    recognition: &[OfflineRecognitionResult],
    decision: &OfflineDecision,
    selection_now_unix_ms: Option<u64>,
) -> Result<String, OfflineSimulationError> {
    const DOMAIN: &[u8] = b"ActingCommand package offline first decision v1\0";
    let encoded = serde_json::to_vec(&DecisionFingerprintProjection {
        schema_version: "actingcommand.package-offline-decision.v1",
        package_id,
        task_id,
        recognition,
        decision,
        selection_now_unix_ms,
    })
    .map_err(|error| {
        OfflineSimulationError::with_detail(
            ExecutionKernelCode::OfflineDecisionFingerprintFailed.code(),
            error.to_string(),
        )
    })?;
    let mut framed = Vec::with_capacity(DOMAIN.len() + size_of::<u64>() + encoded.len());
    framed.extend_from_slice(DOMAIN);
    framed.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
    framed.extend_from_slice(&encoded);
    Ok(Sha256Hash::digest(&framed).to_string())
}

struct PlannedEffect {
    operation_label: String,
    action: InputAction,
    sampling: Option<InputSamplingEvidence>,
    guard: ContainedTaskGuardOutcome,
}

struct OfflineRuntime {
    frames: VecDeque<Frame>,
    capture_count: usize,
    recognition: Vec<OfflineRecognitionResult>,
    planned: Option<PlannedEffect>,
    /// The caller's instant for a select step; without one a select step is unavailable.
    now_unix_ms: Option<u64>,
    /// Set once a select step evaluated.
    selection_now_unix_ms: Option<u64>,
    /// A package with a select step keeps a copy of the last saved frame captured, which the
    /// step's confirmation capture repeats.
    retains_last_frame: bool,
    last_frame: Option<Frame>,
    confirmation_repeats_last_frame: bool,
    /// Workflow #411 B15 R5: a select step's snapshot also holds the package's declared
    /// parameter defaults (an offline run has no dispatch).
    run_parameters: RunParameters,
    /// Workflow #406 T4: how the run draws its box points.
    draw: OfflineDraw,
}

impl OfflineRuntime {
    fn new(
        frames: Vec<Frame>,
        now_unix_ms: Option<u64>,
        retains_last_frame: bool,
        run_parameters: RunParameters,
        draw: OfflineDraw,
    ) -> Self {
        Self {
            frames: frames.into(),
            capture_count: 0,
            recognition: Vec::new(),
            planned: None,
            now_unix_ms,
            selection_now_unix_ms: None,
            retains_last_frame,
            last_frame: None,
            confirmation_repeats_last_frame: false,
            run_parameters,
            draw,
        }
    }
}

enum OfflineBoundary {
    EffectIntercepted,
    FixtureExhausted,
    Invariant(Code),
}

impl ContainedTaskRuntime for OfflineRuntime {
    type Error = OfflineBoundary;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        // A select step's confirmation frame is the saved frame it was evaluated on.
        let frame = if std::mem::take(&mut self.confirmation_repeats_last_frame) {
            self.last_frame
                .as_ref()
                .ok_or(OfflineBoundary::Invariant(
                    ExecutionKernelCode::OfflineSimulationConfirmationFrameMissing.code(),
                ))?
                .try_clone()
                .map_err(|_| {
                    OfflineBoundary::Invariant(
                        ExecutionKernelCode::OfflineSimulationFrameCopyFailed.code(),
                    )
                })?
        } else {
            let frame = self
                .frames
                .pop_front()
                .ok_or(OfflineBoundary::FixtureExhausted)?;
            if self.retains_last_frame {
                self.last_frame = Some(frame.try_clone().map_err(|_| {
                    OfflineBoundary::Invariant(
                        ExecutionKernelCode::OfflineSimulationFrameCopyFailed.code(),
                    )
                })?);
            }
            frame
        };
        self.capture_count += 1;
        Ok(frame.into())
    }

    /// Workflow #406 T4: each step's seed from the run's ([`offline_action_seed`]).
    fn action_seed(
        &mut self,
        step_index: u32,
        operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        Ok(Some(offline_action_seed(
            self.draw.seed,
            step_index,
            operation_label,
        )))
    }

    /// Workflow #406 T4: the touch keys in effect, with one candidate (no landing history).
    fn touch_sampling(&mut self) -> Result<Option<TouchSampling>, Self::Error> {
        Ok(None)
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        let Some(now_unix_ms) = self.now_unix_ms else {
            return Ok(SelectionState::Unavailable);
        };
        self.selection_now_unix_ms = Some(now_unix_ms);
        self.confirmation_repeats_last_frame = true;
        let mut snapshot = empty_offline_snapshot(request);
        self.run_parameters.overlay(&mut snapshot, now_unix_ms);
        Ok(SelectionState::Snapshot {
            snapshot,
            now_unix_ms,
        })
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        let Some(planned) = &self.planned else {
            return Err(OfflineBoundary::Invariant(
                ExecutionKernelCode::OfflineSimulationEffectIntentMissing.code(),
            ));
        };
        if planned.action != action {
            return Err(OfflineBoundary::Invariant(
                ExecutionKernelCode::OfflineSimulationActionMismatch.code(),
            ));
        }
        Err(OfflineBoundary::EffectIntercepted)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match trace {
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                width,
                height,
                ..
            } => self.recognition.push(OfflineRecognitionResult {
                candidate_pages,
                matched_page: page_label,
                width,
                height,
            }),
            ContainedTaskTrace::EffectIntent {
                operation_label,
                action,
                sampling,
                guard,
                ..
            } => {
                if self.planned.is_some() {
                    return Err(OfflineBoundary::Invariant(
                        ExecutionKernelCode::OfflineSimulationMultipleEffects.code(),
                    ));
                }
                self.planned = Some(PlannedEffect {
                    operation_label,
                    action,
                    sampling,
                    guard,
                });
            }
            _ => {}
        }
        Ok(())
    }
}

/// The empty fact snapshot of an offline select step, in the package's game and server (also
/// the snapshot of a replayed scan without `--facts`, Workflow #308 SEL-L6b).
pub(crate) fn empty_offline_snapshot(request: SelectionStateRequest) -> InstanceFactSnapshot {
    InstanceFactSnapshot {
        snapshot_id: OFFLINE_SNAPSHOT_ID.to_owned(),
        ledger_position: OFFLINE_SNAPSHOT_POSITION,
        context: InstanceFactContext {
            instance_id: OFFLINE_INSTANCE.to_owned(),
            server_id: request.server,
            game_id: request.game,
        },
        records: Vec::new(),
    }
}

/// Workflow #308 SEL-L6a (FCM T-01, T-03): the facts a replayed select step evaluates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineReplayFacts {
    /// No snapshot was given: a select step evaluates the empty offline snapshot (the dry run's)
    /// at `now_unix_ms`.
    Empty { now_unix_ms: u64 },
    /// One instance's fact snapshot as a Runtime answered it (`actingctl facts --instance`, MCP
    /// `ac_facts`), evaluated at the instant the Runtime read it.
    Snapshot {
        snapshot: InstanceFactSnapshot,
        read_at_unix_ms: u64,
    },
}

/// The entry check of a replayed run: the entry pages it required and the one its frame matched
/// (`None` when it matched none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineReplayEntry {
    pub required_pages: Vec<String>,
    pub matched_page: Option<String>,
}

/// One input a replayed step would have sent, with the guard outcome it was sent under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineReplayEffect {
    pub action: InputAction,
    pub guard: ContainedTaskGuardOutcome,
    /// Workflow #406 T4: how the input's points were drawn, as `WouldClick`'s `sampling`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<InputSamplingEvidence>,
}

/// One step of a replayed run, in the order the interpreter started it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineReplayStep {
    pub step_index: u32,
    pub operation_label: String,
    pub from_page: String,
    /// The pages the operation declares it reaches, as the run waits for them: its `to` page
    /// set (more than one page for a multi-destination operation), else its `expect_after`
    /// page; empty when it declares neither.
    pub destinations: Vec<String>,
    /// A select step's decision record (Workflow #308), the last attempt's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Box<actingcommand_contract::TaskSelectionRecord>>,
    /// The inputs the step would have sent, in order.
    pub effects: Vec<OfflineReplayEffect>,
    /// The page the step finished on; `None` when the run ended inside the step.
    pub reached_page: Option<String>,
}

/// How a replayed run ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OfflineReplayEnd {
    /// The run completed on `final_page`; `scheduling_outcome_key` is the key the run selected
    /// for its scheduling outcome, when it selected one.
    Completed {
        final_page: Option<String>,
        executed_steps: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        scheduling_outcome_key: Option<String>,
    },
    /// The run ended with the outcome of its failure; a run that needed a frame after the last
    /// one ends with `offline_fixture_exhausted`.
    Refused {
        #[serde(serialize_with = "full_envelope")]
        outcome: Outcome,
    },
}

/// An outcome in its full interface form: the replay is read by the person or the CI job that
/// ran it.
fn full_envelope<S: Serializer>(outcome: &Outcome, serializer: S) -> Result<S::Ok, S::Error> {
    outcome.envelope(Detail::Full).serialize(serializer)
}

/// #411 Lab L6: the readings of one frame a replayed run handed to recognition (D2 model §3.5),
/// joined before the run ended, in hand-off order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineReplayReadings {
    /// The frame's position in the sequence, from 1: the frame the run had captured last when it
    /// handed the frame to recognition.
    pub frame: usize,
    /// The readings that held, in mark order (a mark with `capacity_fact_key` gives its value,
    /// then its capacity).
    pub readings: Vec<OfflineReplayReading>,
    /// The marks that gave no reading on the frame.
    pub unresolved: Vec<OfflineReplayReadingMiss>,
}

/// #411 Lab L6: one reading a replayed run took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineReplayReading {
    pub reading_id: String,
    pub fact_key: String,
    pub value: u64,
    pub confidence_milli: u16,
}

/// #411 Lab L6: one reading mark that gave no reading, with why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineReplayReadingMiss {
    pub reading_id: String,
    pub fact_key: String,
    /// Its token of the vocabulary `reading_problem`.
    #[serde(serialize_with = "token_spelling")]
    pub reading_problem: ResourceReadingProblem,
}

/// A vocabulary token in serialized output as its spelling.
fn token_spelling<S: Serializer>(
    problem: &ResourceReadingProblem,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(problem.as_str())
}

/// Workflow #308 SEL-L6a: a whole contained task replayed over a saved frame sequence.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OfflineReplayResult {
    pub mode: &'static str,
    pub executed: bool,
    /// The package's identity, its `package_id` (Workflow #336 PKG-S1: no package digest).
    pub package_id: String,
    pub task_id: String,
    /// The frames the run captured, from the first of the sequence on.
    pub frames_read: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<OfflineReplayEntry>,
    /// Every page recognition of the run, in order.
    pub recognition: Vec<OfflineRecognitionResult>,
    pub steps: Vec<OfflineReplayStep>,
    pub end: OfflineReplayEnd,
    /// The instant a select step evaluated at; present only when one was evaluated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_now_unix_ms: Option<u64>,
    /// #411 Lab L6: the readings of every frame the run handed to recognition, in hand-off
    /// order; absent when it handed none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub readings: Vec<OfflineReplayReadings>,
    /// #411 Lab L6: the run's fields report (`fields_v1`) as the Runtime records it, each field
    /// its declaration classifies `personal` redacted; absent when the run recorded none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<OcrFieldsReport>,
    /// Workflow #406 T4: the run's seed, present when an input drew its points from it; a
    /// replay given this seed draws every point again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Why a replay gave no result.
#[derive(Debug)]
pub enum OfflineReplayError<E> {
    /// The frame sequence failed to give its next frame; its error unchanged.
    Frames(E),
    /// The interpreter broke an invariant of the replay adapter.
    Invariant(OfflineSimulationError),
}

/// Workflow #308 SEL-L6a (FCM T-01, T-03): replays the whole of `task` with the production
/// contained-task interpreter over `frames`. Every capture takes the next frame of the
/// sequence, a select step's confirmation and every sample of a multi-frame consensus
/// included; every input is recorded and performs nothing, and the next capture shows what the
/// recording saw after it. A select step evaluates `facts`. No device backend can be injected
/// through this boundary.
pub fn replay_contained_task<I, E>(
    task: &PreparedContainedTask,
    frames: I,
    facts: OfflineReplayFacts,
) -> Result<OfflineReplayResult, OfflineReplayError<E>>
where
    I: Iterator<Item = Result<Frame, E>>,
{
    replay_contained_task_drawn(task, frames, facts, OfflineDraw::default())
}

/// Workflow #406 T4: [`replay_contained_task`] drawing every box point as `draw` says: the run's
/// seed and the touch keys in effect. The plain form draws with the seed 0 and no touch keys.
pub fn replay_contained_task_drawn<I, E>(
    task: &PreparedContainedTask,
    frames: I,
    facts: OfflineReplayFacts,
    draw: OfflineDraw,
) -> Result<OfflineReplayResult, OfflineReplayError<E>>
where
    I: Iterator<Item = Result<Frame, E>>,
{
    let mut runtime = ReplayRuntime {
        task,
        frames,
        facts,
        frames_read: 0,
        entry: None,
        recognition: Vec::new(),
        steps: Vec::new(),
        pending_action: None,
        selection_now_unix_ms: None,
        reading_frames: VecDeque::new(),
        readings: Vec::new(),
        fields: None,
        draw,
    };
    let end = match task.run(&mut runtime) {
        Ok(outcome) => OfflineReplayEnd::Completed {
            final_page: outcome.final_page,
            executed_steps: outcome.executed_steps,
            scheduling_outcome_key: outcome.selected_scheduling_outcome,
        },
        Err(ContainedTaskRunError::Task(error)) => OfflineReplayEnd::Refused {
            outcome: error.outcome(),
        },
        Err(
            ContainedTaskRunError::Boundary(boundary)
            | ContainedTaskRunError::NonfatalOperation(boundary),
        ) => match boundary {
            ReplayBoundary::FramesExhausted => OfflineReplayEnd::Refused {
                outcome: Outcome::new(FIXTURE_EXHAUSTED_CODE),
            },
            ReplayBoundary::Step(error) => OfflineReplayEnd::Refused {
                outcome: error.outcome(),
            },
            ReplayBoundary::Frames(error) => return Err(OfflineReplayError::Frames(error)),
            ReplayBoundary::Invariant(code) => {
                return Err(OfflineReplayError::Invariant(OfflineSimulationError::new(
                    code,
                )));
            }
        },
    };
    let seed = runtime
        .steps
        .iter()
        .flat_map(|step| &step.effects)
        .any(|effect| drew_from_seed(&effect.action, effect.sampling.as_ref()))
        .then_some(draw.seed);
    Ok(OfflineReplayResult {
        mode: "offline_replay",
        executed: false,
        package_id: task.package_label().to_string(),
        task_id: task.task_label().to_string(),
        frames_read: runtime.frames_read,
        entry: runtime.entry,
        recognition: runtime.recognition,
        steps: runtime.steps,
        end,
        selection_now_unix_ms: runtime.selection_now_unix_ms,
        readings: runtime.readings,
        fields: runtime.fields,
        seed,
    })
}

struct ReplayRuntime<'t, I> {
    task: &'t PreparedContainedTask,
    frames: I,
    facts: OfflineReplayFacts,
    frames_read: usize,
    entry: Option<OfflineReplayEntry>,
    recognition: Vec<OfflineRecognitionResult>,
    steps: Vec<OfflineReplayStep>,
    /// The action of the last effect intent, which the next input must send.
    pending_action: Option<InputAction>,
    selection_now_unix_ms: Option<u64>,
    /// #411 Lab L6: the positions of the frames handed to recognition whose readings the run has
    /// not joined yet, in hand-off order.
    reading_frames: VecDeque<usize>,
    readings: Vec<OfflineReplayReadings>,
    fields: Option<OcrFieldsReport>,
    /// Workflow #406 T4: how the run draws its box points.
    draw: OfflineDraw,
}

#[derive(Debug)]
enum ReplayBoundary<E> {
    /// The run needed a frame after the last one of the sequence.
    FramesExhausted,
    Frames(E),
    /// A started step names an operation the replay cannot describe; the run ends with it.
    Step(Box<crate::ContainedTaskError>),
    Invariant(Code),
}

impl<I> ReplayRuntime<'_, I> {
    /// The step `step_index` started last; `None` for a step that never started.
    fn step(&mut self, step_index: u32) -> Option<&mut OfflineReplayStep> {
        self.steps
            .iter_mut()
            .rev()
            .find(|step| step.step_index == step_index)
    }
}

/// A step trace for a step that never started.
fn step_not_started<E>() -> ReplayBoundary<E> {
    ReplayBoundary::Invariant(SharedCode::ContainedTaskStateInvalid.code())
}

impl<I, E> ContainedTaskRuntime for ReplayRuntime<'_, I>
where
    I: Iterator<Item = Result<Frame, E>>,
{
    type Error = ReplayBoundary<E>;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let frame = self
            .frames
            .next()
            .ok_or(ReplayBoundary::FramesExhausted)?
            .map_err(ReplayBoundary::Frames)?;
        self.frames_read += 1;
        Ok(frame.into())
    }

    /// A saved sequence holds every sample the recording took, so a multi-frame consensus takes
    /// its samples from it.
    fn sampling_supported(&self) -> bool {
        true
    }

    /// Workflow #406 T4: each step's seed from the run's ([`offline_action_seed`]).
    fn action_seed(
        &mut self,
        step_index: u32,
        operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        Ok(Some(offline_action_seed(
            self.draw.seed,
            step_index,
            operation_label,
        )))
    }

    /// Workflow #406 T4: the touch keys in effect, with one candidate (no landing history).
    fn touch_sampling(&mut self) -> Result<Option<TouchSampling>, Self::Error> {
        Ok(self.draw.step_touch())
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        let (mut snapshot, now_unix_ms) = match &self.facts {
            OfflineReplayFacts::Empty { now_unix_ms } => {
                (empty_offline_snapshot(request), *now_unix_ms)
            }
            OfflineReplayFacts::Snapshot {
                snapshot,
                read_at_unix_ms,
            } => (snapshot.clone(), *read_at_unix_ms),
        };
        // Workflow #411 B15 R5: a replayed run has no dispatch; it reads the package's declared
        // parameter defaults.
        self.task
            .default_run_parameters()
            .overlay(&mut snapshot, now_unix_ms);
        self.selection_now_unix_ms = Some(now_unix_ms);
        Ok(SelectionState::Snapshot {
            snapshot,
            now_unix_ms,
        })
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        match self.pending_action.take() {
            None => Err(ReplayBoundary::Invariant(
                ExecutionKernelCode::OfflineSimulationEffectIntentMissing.code(),
            )),
            Some(intended) if intended != action => Err(ReplayBoundary::Invariant(
                ExecutionKernelCode::OfflineSimulationActionMismatch.code(),
            )),
            Some(_) => Ok(()),
        }
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match trace {
            ContainedTaskTrace::EntryRecognition {
                required_pages,
                matched_page,
            } => {
                self.entry = Some(OfflineReplayEntry {
                    required_pages,
                    matched_page,
                });
            }
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                width,
                height,
                ..
            } => self.recognition.push(OfflineRecognitionResult {
                candidate_pages,
                matched_page: page_label,
                width,
                height,
            }),
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                ..
            } => {
                let destinations = self
                    .task
                    .operation_destinations(&operation_label)
                    .map_err(|error| ReplayBoundary::Step(Box::new(error)))?;
                self.steps.push(OfflineReplayStep {
                    step_index,
                    operation_label,
                    from_page,
                    destinations,
                    selection: None,
                    effects: Vec::new(),
                    reached_page: None,
                });
            }
            ContainedTaskTrace::SelectionEvaluated {
                step_index,
                selection,
                ..
            } => {
                self.step(step_index)
                    .ok_or_else(step_not_started::<E>)?
                    .selection = Some(selection);
            }
            ContainedTaskTrace::EffectIntent {
                step_index,
                action,
                sampling,
                guard,
                ..
            } => {
                if self.pending_action.is_some() {
                    return Err(ReplayBoundary::Invariant(
                        ExecutionKernelCode::OfflineSimulationMultipleEffects.code(),
                    ));
                }
                self.step(step_index)
                    .ok_or_else(step_not_started::<E>)?
                    .effects
                    .push(OfflineReplayEffect {
                        action: action.clone(),
                        guard,
                        sampling,
                    });
                self.pending_action = Some(action);
            }
            ContainedTaskTrace::StepFinished {
                step_index,
                page_label,
                ..
            } => {
                self.step(step_index)
                    .ok_or_else(step_not_started::<E>)?
                    .reached_page = Some(page_label);
            }
            // #411 Lab L6: the frame handed to recognition is the one the run captured last; its
            // readings are joined before the run ends, in hand-off order.
            ContainedTaskTrace::ResourceReadingFrame { .. } => {
                self.reading_frames.push_back(self.frames_read);
            }
            ContainedTaskTrace::ResourceReadings {
                readings,
                unresolved,
                ..
            } => {
                let frame = self.reading_frames.pop_front().ok_or_else(|| {
                    ReplayBoundary::Invariant(SharedCode::ContainedTaskStateInvalid.code())
                })?;
                self.readings.push(OfflineReplayReadings {
                    frame,
                    readings: readings
                        .into_iter()
                        .map(|reading| OfflineReplayReading {
                            reading_id: reading.declaration.id,
                            fact_key: reading.fact_key,
                            value: reading.value,
                            confidence_milli: reading.confidence_milli,
                        })
                        .collect(),
                    unresolved: unresolved
                        .into_iter()
                        .map(|miss| OfflineReplayReadingMiss {
                            reading_id: miss.declaration.id,
                            fact_key: miss.declaration.fact_key,
                            reading_problem: miss.problem,
                        })
                        .collect(),
                });
            }
            ContainedTaskTrace::PostAdmissionOcrFields { report } => {
                self.fields = Some(redact_personal_fields(report));
            }
            _ => {}
        }
        Ok(())
    }
}

/// #411 Lab L6: `report` with every field its declaration classifies `personal` redacted: no
/// text, value, detail or extraction evidence, and `redacted` true.
fn redact_personal_fields(mut report: OcrFieldsReport) -> OcrFieldsReport {
    let personal = report
        .declaration
        .fields
        .iter()
        .filter(|field| field.privacy == OcrFieldPrivacy::Personal)
        .map(|field| field.id.clone())
        .collect::<Vec<_>>();
    for result in report
        .records
        .iter_mut()
        .flat_map(|record| record.fields.iter_mut())
        .filter(|result| personal.contains(&result.field_id))
    {
        result.raw_text = None;
        result.normalized_text = None;
        result.value = None;
        result.detail = None;
        result.extraction = None;
        result.redacted = true;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PreparedContainedTask;
    use actingcommand_device::{CaptureBackendName, PixelFormat};
    use serde_json::{Value, json};
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::FileOptions};

    #[test]
    fn offline_and_effecting_runtime_share_the_first_decision() {
        let package_bytes = package(PackageOptions::default());
        let task = prepare(&package_bytes);
        let frames = vec![home_frame(true)];
        let offline = simulate_contained_task(
            &task,
            frames
                .iter()
                .map(|frame| frame.try_clone().expect("copy fixture frame"))
                .collect(),
        )
        .expect("offline simulation");
        let offline_action = match &offline.decision {
            OfflineDecision::WouldClick { action, .. } => action.clone(),
            other => panic!("expected would-click, got {other:?}"),
        };
        let effecting = effecting_decision(&task, frames);

        assert_eq!(effecting.decision, offline.decision);
        assert_eq!(effecting.decision_fingerprint, offline.decision_fingerprint);
        assert_eq!(effecting.actions, vec![offline_action]);
        let mut archive = zip::ZipArchive::new(Cursor::new(package_bytes)).unwrap();
        let mut entries = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_owned();
            let mut value: Value = serde_json::from_reader(&mut entry).unwrap();
            if name == "resources/recognition/neutral.test.pages.json" {
                value["pages"][0]["required"] = json!([]);
                value["pages"][0]["any_of"] = json!([["page/home"]]);
            }
            entries.push((name, value));
        }
        let bytes = zip_entries(
            &entries
                .iter()
                .map(|(name, value)| (name.as_str(), value.clone()))
                .collect::<Vec<_>>(),
        );
        let required = prepare(&bytes);
        assert_eq!(
            required.required_entry_pages(),
            Some(["neutral/home".to_owned()].as_slice())
        );
        let offline = simulate_contained_task(&required, vec![home_frame(true)]).unwrap();
        let effecting = effecting_decision(&required, vec![home_frame(true)]);
        assert_eq!(offline.decision, effecting.decision);
        assert_eq!(offline.decision_fingerprint, effecting.decision_fingerprint);
        assert_eq!(effecting.actions.len(), 1);
        for frame in [terminal_frame(), solid_frame([0, 0, 0], [0, 0, 0])] {
            assert_same_refusal(
                &required,
                vec![frame],
                "contained_task_home_entry_not_matched",
            );
        }
    }

    #[test]
    fn effecting_runtime_stops_at_the_typed_first_effect_boundary() {
        let package_bytes = package(PackageOptions::default());
        let task = prepare(&package_bytes);
        let mut runtime = EffectingRuntime::new(vec![home_frame(true)]);

        let outcome = task.run(&mut runtime);

        assert!(matches!(
            outcome,
            Err(ContainedTaskRunError::Boundary(
                EffectingBoundary::FirstEffectIntercepted
            ))
        ));
        assert!(runtime.frames.is_empty());
        assert_eq!(runtime.actions.len(), 1);
        assert!(runtime.first_effect.is_some());
        assert!(runtime.recognition_at_first_effect.is_some());
    }

    #[test]
    fn offline_and_effecting_runtime_share_completion_and_no_op() {
        let navigable = package(PackageOptions::default());
        let navigable = prepare(&navigable);
        let frames = vec![terminal_frame()];
        let offline = simulate_contained_task(
            &navigable,
            frames
                .iter()
                .map(|frame| frame.try_clone().expect("copy fixture frame"))
                .collect(),
        )
        .expect("offline completion");
        let effecting = effecting_decision(&navigable, frames);
        assert_eq!(effecting.decision, offline.decision);
        assert_eq!(effecting.decision_fingerprint, offline.decision_fingerprint);
        assert!(effecting.actions.is_empty());

        let recognize_only = package(PackageOptions {
            execution_mode: "recognize_only",
            ..PackageOptions::default()
        });
        let recognize_only = prepare(&recognize_only);
        let offline = simulate_contained_task(&recognize_only, vec![home_frame(true)])
            .expect("offline no-op");
        let effecting = effecting_decision(&recognize_only, vec![home_frame(true)]);
        assert_eq!(effecting.decision, offline.decision);
        assert_eq!(effecting.decision_fingerprint, offline.decision_fingerprint);
        assert!(effecting.actions.is_empty());
    }

    #[test]
    fn offline_and_effecting_runtime_share_all_typed_refusals() {
        let navigable = prepare(&package(PackageOptions::default()));
        assert_same_refusal(
            &navigable,
            vec![home_frame(false)],
            "contained_task_guard_refused",
        );

        let unknown = prepare(&package(PackageOptions {
            step_timeout_ms: 1,
            ..PackageOptions::default()
        }));
        let unknown_frame = solid_frame([0, 0, 0], [0, 0, 0]);
        assert_same_refusal(
            &unknown,
            (0..16)
                .map(|_| unknown_frame.try_clone().expect("copy fixture frame"))
                .collect(),
            "contained_task_page_unknown",
        );

        let conflict = prepare(&package(PackageOptions {
            conflicting_page: true,
            ..PackageOptions::default()
        }));
        assert_same_refusal(
            &conflict,
            vec![home_frame(true)],
            "contained_task_recognition_conflict",
        );

        let wrong_size = Frame::from_pixels(
            1,
            1,
            vec![255, 0, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame");
        assert_same_refusal(
            &navigable,
            vec![wrong_size],
            "contained_task_frame_resolution_mismatch",
        );

        let exhaustion = prepare(&package(PackageOptions {
            step_timeout_ms: 60_000,
            ..PackageOptions::default()
        }));
        assert_same_refusal(
            &exhaustion,
            vec![solid_frame([0, 0, 0], [0, 0, 0])],
            FIXTURE_EXHAUSTED_CODE.as_str(),
        );
    }

    #[test]
    fn simulation_reports_complete_and_legal_no_op_without_input() {
        let navigable = package(PackageOptions::default());
        let completed = simulate_contained_task(&prepare(&navigable), vec![terminal_frame()])
            .expect("complete simulation");
        assert!(matches!(
            completed.decision,
            OfflineDecision::WouldComplete { .. }
        ));

        let recognize_only = package(PackageOptions {
            execution_mode: "recognize_only",
            ..PackageOptions::default()
        });
        let no_op = simulate_contained_task(&prepare(&recognize_only), vec![home_frame(true)])
            .expect("no-op simulation");
        assert!(matches!(no_op.decision, OfflineDecision::NoOp { .. }));
    }

    #[test]
    fn simulation_receipts_bind_typed_refusals_to_decision_fingerprints() {
        let package_bytes = package(PackageOptions::default());
        let task = prepare(&package_bytes);
        let unknown_task = prepare(&package(PackageOptions {
            step_timeout_ms: 1,
            ..PackageOptions::default()
        }));
        let unknown_frame = solid_frame([0, 0, 0], [0, 0, 0]);
        let unknown = simulate_contained_task(
            &unknown_task,
            (0..16)
                .map(|_| unknown_frame.try_clone().expect("copy fixture frame"))
                .collect(),
        )
        .expect("unknown-page refusal receipt");
        assert_refusal(&unknown, "contained_task_page_unknown");
        let guard =
            simulate_contained_task(&task, vec![home_frame(false)]).expect("guard refusal receipt");
        assert_refusal(&guard, "contained_task_guard_refused");
        let guard_replay = simulate_contained_task(&task, vec![home_frame(false)])
            .expect("guard refusal replay receipt");
        assert_eq!(
            guard.decision_fingerprint,
            guard_replay.decision_fingerprint
        );
        let wrong_size = Frame::from_pixels(
            1,
            1,
            vec![255, 0, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame");
        let resolution =
            simulate_contained_task(&task, vec![wrong_size]).expect("resolution refusal receipt");
        assert_refusal(&resolution, "contained_task_frame_resolution_mismatch");

        let conflict = package(PackageOptions {
            conflicting_page: true,
            ..PackageOptions::default()
        });
        let conflict = simulate_contained_task(&prepare(&conflict), vec![home_frame(true)])
            .expect("conflict refusal receipt");
        assert_refusal(&conflict, "contained_task_recognition_conflict");
        let fingerprints = [
            &unknown.decision_fingerprint,
            &guard.decision_fingerprint,
            &resolution.decision_fingerprint,
            &conflict.decision_fingerprint,
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            fingerprints.len(),
            4,
            "distinct refusals must have distinct decision fingerprints"
        );
    }

    fn assert_refusal(result: &OfflineSimulationResult, expected: &str) {
        assert!(matches!(
            &result.decision,
            OfflineDecision::Refused { code, .. } if code.as_str() == expected
        ));
        assert_eq!(result.decision_fingerprint.len(), 64);
        assert!(
            result
                .decision_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    fn assert_same_refusal(task: &PreparedContainedTask, frames: Vec<Frame>, expected: &str) {
        let offline = simulate_contained_task(
            task,
            frames
                .iter()
                .map(|frame| frame.try_clone().expect("copy fixture frame"))
                .collect(),
        )
        .expect("offline refusal result");
        let effecting = effecting_decision(task, frames);
        assert_refusal(&offline, expected);
        assert!(matches!(
            &effecting.decision,
            OfflineDecision::Refused { code, .. } if code.as_str() == expected
        ));
        assert_eq!(effecting.decision, offline.decision);
        assert_eq!(effecting.decision_fingerprint, offline.decision_fingerprint);
        assert!(effecting.actions.is_empty());
    }

    #[test]
    fn admission_rejects_missing_guard_and_recovery_task() {
        let missing_guard = package(PackageOptions {
            include_guard: false,
            ..PackageOptions::default()
        });
        assert_eq!(
            prepare_error(&missing_guard).code().as_str(),
            "contained_task_guard_missing"
        );

        let missing_recovery = package(PackageOptions {
            recovery: true,
            ..PackageOptions::default()
        });
        assert_eq!(
            prepare_error(&missing_recovery).code().as_str(),
            "contained_task_recovery_missing"
        );
    }

    // Workflow #411 B15 R5 (host gate key 114; register §1.115; contracts/remedies.md section 4):
    // an offline Lab run has no dispatch, so its select step's snapshot holds every parameter the
    // package declares with its declared default. The package provides `cap_x` with `param_p`
    // (options `opt_1` and `opt_2`, default `opt_2`); its select step's policy reads
    // `session.dispatch.parameter.param_p`, admits the first slot for `opt_1` and the second for
    // `opt_2`, and aborts when the value is unknown. The dry run would tap the second slot and
    // the replay's decision selects it.
    #[test]
    fn b15r_an_offline_lab_run_reads_the_declared_defaults() {
        const NOW_UNIX_MS: u64 = 1_000_000;
        let task = prepare(&parameter_package());

        let simulated = simulate_contained_task_at(&task, vec![home_frame(true)], NOW_UNIX_MS)
            .expect("offline simulation");
        assert!(
            matches!(
                &simulated.decision,
                OfflineDecision::WouldClick {
                    action: InputAction::Tap { x: 1, y: 0 },
                    ..
                }
            ),
            "{:?}",
            simulated.decision
        );

        let frames = [home_frame(true), home_frame(true), result_frame()];
        let replayed = replay_contained_task(
            &task,
            frames.into_iter().map(Ok::<_, std::convert::Infallible>),
            OfflineReplayFacts::Empty {
                now_unix_ms: NOW_UNIX_MS,
            },
        )
        .expect("the replay gives a result");
        let selected = replayed
            .steps
            .iter()
            .filter_map(|step| step.selection.as_ref())
            .map(|selection| selection.selected.clone())
            .collect::<Vec<_>>();
        assert_eq!(selected, [vec!["choices#01".to_owned()]], "{replayed:?}");
    }

    // The provider package of `b15r_an_offline_lab_run_reads_the_declared_defaults`: frames are
    // 2x1, the first pixel the page (red `home`, white `result`), the second green; the feature
    // `first` of the two fixed slots passes on the first slot only.
    fn parameter_package() -> Vec<u8> {
        let fact = json!({"source": "fact", "fact_key": "session.dispatch.parameter.param_p"});
        let reads = |option: &str, first: bool| {
            json!({"kind": "all", "of": [
                {"kind": "string_in", "value": fact.clone(), "allowed": [option]},
                {"kind": "boolean_equals", "value": {"source": "field", "field": "first"},
                 "expected": first}
            ]})
        };
        let color = |id: &str, x: u32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let slot = |x: u32, target: &str| {
            json!({"rect": {"x": x, "y": 0, "width": 1, "height": 1},
                   "click": {"x": x, "y": 0, "width": 1, "height": 1},
                   "targets": {"first": target}})
        };
        let layout = json!({
            "id": "choices",
            "page_id": "neutral/home",
            "kind": "fixed_slots",
            "features": [{"name": "first", "value": "passed"}],
            "slots": [slot(0, "slot/first"), slot(1, "slot/second")]
        });
        zip_entries(&[
            (
                "control.json",
                json!({
                    "schema_version": "Lab-1y.control.v2",
                    "package_id": "neutral.test.param",
                    "execution_mode": "navigable_route",
                    "game": "neutral",
                    "server": "test",
                    "resolution": {"width": 2, "height": 1},
                    "entry_task_id": "task",
                    "capture_interval_ms": 1,
                    "step_timeout_ms": 5000,
                    "timeout_ms": 20000,
                    "max_steps": 2,
                    "provides": [{"capability": "cap_x", "parameters": [
                        {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_2"}
                    ]}]
                }),
            ),
            (
                "resources/manifest.json",
                json!({"schema_version": "0.3", "entry_task_id": "task"}),
            ),
            (
                "resources/operations/task/task.json",
                json!({
                    "schema_version": "0.9",
                    "task_id": "task",
                    "game": "neutral",
                    "server_scope": ["test"],
                    "coordinate_space": {"width": 2, "height": 1},
                    "target_page": "result",
                    "candidate_layouts": [layout.clone()],
                    "operations": [{
                        "id": "pick",
                        "from": "home",
                        "to": "result",
                        "expect_after": {"page_id": "result"},
                        "guard": {
                            "page_id": "home",
                            "target_id": "guard/ready",
                            "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                            "color_probe": "guard/ready"
                        },
                        "select": {"layout_id": "choices",
                                   "policy": {"path": "policies/by_parameter.json"}},
                        "retryable": false
                    }]
                }),
            ),
            (
                "resources/operations/task/policies/by_parameter.json",
                json!({
                    "schema_version": "actingcommand.selection-policy.v1",
                    "policy_id": "by-parameter",
                    "applies_to": {
                        "candidate_layout_id": "choices",
                        "outcome_keys": {"selected": "slot_selected", "empty": "slot_empty",
                                         "insufficient": "slot_insufficient",
                                         "ambiguous": "slot_ambiguous",
                                         "unknown": "slot_unknown"}
                    },
                    "fields": [{"name": "first", "value_type": {"type": "boolean"}}],
                    "facts": [{"fact_key": "session.dispatch.parameter.param_p",
                               "value_type": {"type": "enum_string",
                                              "allowed": ["opt_1", "opt_2"]},
                               "max_age_ms": 86_400_000, "minimum_confidence_milli": 1000}],
                    "gates": [{"gate_id": "by_parameter",
                               "predicate": {"kind": "any",
                                             "of": [reads("opt_1", true), reads("opt_2", false)]},
                               "on_unknown": {"kind": "abort_evaluation"}}],
                    "scoring": [],
                    "selection": {"mode": "exactly_one", "required_count": 1},
                    "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
                }),
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                json!({
                    "schema_version": "0.7",
                    "game": "neutral",
                    "server": "test",
                    "coordinate_space": {"width": 2, "height": 1},
                    "defaults": {"color_max_distance": 0.0},
                    "targets": [
                        color("page/home", 0, [255, 0, 0]),
                        color("page/result", 0, [255, 255, 255]),
                        color("guard/ready", 1, [0, 255, 0]),
                        color("slot/first", 1, [0, 255, 0]),
                        color("slot/second", 1, [0, 0, 255])
                    ],
                    "candidate_layouts": [layout]
                }),
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version": "0.3", "pages": [
                    {"id": "neutral/home", "required": ["page/home"], "optional": [],
                     "forbidden": []},
                    {"id": "neutral/result", "required": ["page/result"], "optional": [],
                     "forbidden": []}
                ]}),
            ),
        ])
    }

    fn result_frame() -> Frame {
        solid_frame([255, 255, 255], [0, 255, 0])
    }

    #[derive(Clone, Copy)]
    struct PackageOptions {
        execution_mode: &'static str,
        include_guard: bool,
        conflicting_page: bool,
        recovery: bool,
        step_timeout_ms: u64,
    }

    impl Default for PackageOptions {
        fn default() -> Self {
            Self {
                execution_mode: "navigable_route",
                include_guard: true,
                conflicting_page: false,
                recovery: false,
                step_timeout_ms: 10,
            }
        }
    }

    fn package(options: PackageOptions) -> Vec<u8> {
        let control = json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.semantic.task",
            "execution_mode": options.execution_mode,
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": options.step_timeout_ms,
            "timeout_ms": 100,
            "max_steps": 2
        });
        let mut operation = json!({
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0}
        });
        if options.include_guard {
            operation["guard"] = json!({
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            });
        }
        let mut task = json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "entry_page": "home",
            "target_page": "terminal",
            "operations": [operation]
        });
        if options.recovery {
            task["recovery"] = json!({"kind": "return_home", "task_id": "return_home"});
        }
        let mut pages = vec![
            json!({"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]}),
            json!({"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}),
        ];
        if options.conflicting_page {
            pages.push(json!({"id":"neutral/duplicate","required":["page/home"],"optional":[],"forbidden":[]}));
        }
        zip_entries(&[
            ("control.json", control),
            (
                "resources/manifest.json",
                json!({"schema_version":"0.3","entry_task_id":"task"}),
            ),
            ("resources/operations/task/task.json", task),
            (
                "resources/recognition/neutral.test.pack.json",
                json!({
                    "schema_version": "0.3",
                    "game": "neutral",
                    "server": "test",
                    "coordinate_space": {"width": 2, "height": 1},
                    "defaults": {"color_max_distance": 0.0},
                    "targets": [
                        {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                        {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
                        {"type":"color","id":"guard/ready","region":{"x":1,"y":0,"width":1,"height":1},"expected":[0,255,0]}
                    ]
                }),
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version":"0.3","pages":pages}),
            ),
        ])
    }

    fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (path, value) in entries {
            zip.start_file(*path, options).expect("zip entry");
            serde_json::to_writer(&mut zip, value).expect("zip JSON");
            zip.write_all(b"\n").expect("zip newline");
        }
        zip.finish().expect("finish zip").into_inner()
    }

    fn prepare(bytes: &[u8]) -> PreparedContainedTask {
        PreparedContainedTask::load("neutral.instance", bytes).expect("prepared task")
    }

    fn prepare_error(bytes: &[u8]) -> crate::ContainedTaskError {
        PreparedContainedTask::load("neutral.instance", bytes)
            .err()
            .expect("admission failure")
    }

    fn home_frame(guard_passes: bool) -> Frame {
        solid_frame(
            [255, 0, 0],
            if guard_passes { [0, 255, 0] } else { [0, 0, 0] },
        )
    }

    fn terminal_frame() -> Frame {
        solid_frame([0, 0, 255], [0, 0, 0])
    }

    fn solid_frame(left: [u8; 3], right: [u8; 3]) -> Frame {
        Frame::from_pixels(
            2,
            1,
            [left, right].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame")
    }

    struct EffectingRuntime {
        frames: VecDeque<Frame>,
        actions: Vec<InputAction>,
        recognition: Vec<OfflineRecognitionResult>,
        first_effect: Option<PlannedEffect>,
        recognition_at_first_effect: Option<Vec<OfflineRecognitionResult>>,
    }

    #[derive(Debug)]
    enum EffectingBoundary {
        FirstEffectIntercepted,
        FixtureExhausted,
        Invariant(&'static str),
    }

    impl EffectingRuntime {
        fn new(frames: Vec<Frame>) -> Self {
            Self {
                frames: frames.into(),
                actions: Vec::new(),
                recognition: Vec::new(),
                first_effect: None,
                recognition_at_first_effect: None,
            }
        }
    }

    impl ContainedTaskRuntime for EffectingRuntime {
        type Error = EffectingBoundary;

        fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
            self.frames
                .pop_front()
                .ok_or(EffectingBoundary::FixtureExhausted)
                .map(ObservedFrame::from)
        }

        fn input(
            &mut self,
            action: InputAction,
            _frame: Option<InputFrameContext>,
        ) -> Result<(), Self::Error> {
            let Some(planned) = &self.first_effect else {
                return Err(EffectingBoundary::Invariant(
                    "effecting_fake_effect_intent_missing",
                ));
            };
            if planned.action != action {
                return Err(EffectingBoundary::Invariant(
                    "effecting_fake_action_mismatch",
                ));
            }
            self.actions.push(action);
            Err(EffectingBoundary::FirstEffectIntercepted)
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            match trace {
                ContainedTaskTrace::RecognitionCompleted {
                    candidate_pages,
                    page_label,
                    width,
                    height,
                    ..
                } => self.recognition.push(OfflineRecognitionResult {
                    candidate_pages,
                    matched_page: page_label,
                    width,
                    height,
                }),
                ContainedTaskTrace::EffectIntent {
                    operation_label,
                    action,
                    sampling,
                    guard,
                    ..
                } => {
                    if self.first_effect.is_some() {
                        return Err(EffectingBoundary::Invariant(
                            "effecting_fake_multiple_effects",
                        ));
                    }
                    self.recognition_at_first_effect = Some(self.recognition.clone());
                    self.first_effect = Some(PlannedEffect {
                        operation_label,
                        action,
                        sampling,
                        guard,
                    });
                }
                _ => {}
            }
            Ok(())
        }
    }

    struct EffectingDecision {
        decision: OfflineDecision,
        decision_fingerprint: String,
        actions: Vec<InputAction>,
    }

    fn effecting_decision(task: &PreparedContainedTask, frames: Vec<Frame>) -> EffectingDecision {
        let mut runtime = EffectingRuntime::new(frames);
        let outcome = task.run(&mut runtime);
        let (decision, recognition) = if let Some(planned) = runtime.first_effect.take() {
            match outcome {
                Err(
                    ContainedTaskRunError::Boundary(EffectingBoundary::FirstEffectIntercepted)
                    | ContainedTaskRunError::NonfatalOperation(
                        EffectingBoundary::FirstEffectIntercepted,
                    ),
                ) => {}
                Err(
                    ContainedTaskRunError::Boundary(EffectingBoundary::Invariant(code))
                    | ContainedTaskRunError::NonfatalOperation(EffectingBoundary::Invariant(code)),
                ) => {
                    panic!("unexpected effecting invariant after first effect: {code}")
                }
                other => panic!("effecting runtime did not stop at the first effect: {other:?}"),
            }
            (
                OfflineDecision::WouldClick {
                    operation_label: planned.operation_label,
                    action: planned.action,
                    sampling: planned.sampling,
                    guard: planned.guard,
                },
                runtime
                    .recognition_at_first_effect
                    .take()
                    .expect("recognition snapshot at first effect"),
            )
        } else {
            let decision = match outcome {
                Ok(outcome)
                    if outcome.executed_steps == 0 && task.execution_mode() == "recognize_only" =>
                {
                    OfflineDecision::NoOp {
                        final_page: outcome.final_page,
                    }
                }
                Ok(outcome) if outcome.executed_steps == 0 => OfflineDecision::WouldComplete {
                    final_page: outcome.final_page,
                },
                Ok(outcome) => panic!(
                    "effecting runtime completed {} step(s) without a first-effect trace",
                    outcome.executed_steps
                ),
                Err(ContainedTaskRunError::Task(error)) => OfflineDecision::Refused {
                    code: error.code(),
                    detail: error.detail().map(str::to_string),
                },
                Err(
                    ContainedTaskRunError::Boundary(EffectingBoundary::FixtureExhausted)
                    | ContainedTaskRunError::NonfatalOperation(EffectingBoundary::FixtureExhausted),
                ) => OfflineDecision::Refused {
                    code: FIXTURE_EXHAUSTED_CODE.into(),
                    detail: None,
                },
                Err(
                    ContainedTaskRunError::Boundary(EffectingBoundary::FirstEffectIntercepted)
                    | ContainedTaskRunError::NonfatalOperation(
                        EffectingBoundary::FirstEffectIntercepted,
                    ),
                ) => {
                    panic!("effecting runtime intercepted input without an effect trace")
                }
                Err(
                    ContainedTaskRunError::Boundary(EffectingBoundary::Invariant(code))
                    | ContainedTaskRunError::NonfatalOperation(EffectingBoundary::Invariant(code)),
                ) => {
                    panic!("unexpected effecting invariant: {code}")
                }
            };
            (decision, runtime.recognition.clone())
        };
        let decision_fingerprint = fingerprint_decision(
            task.package_label(),
            task.task_label(),
            &recognition,
            &decision,
            None,
        )
        .expect("effecting decision fingerprint");
        EffectingDecision {
            decision,
            decision_fingerprint,
            actions: runtime.actions,
        }
    }
}
