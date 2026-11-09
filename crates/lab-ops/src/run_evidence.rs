// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl run evidence` (Workflow #389 MCP-6; MCP-0 command table v1.1 §4.7), the CLI
//! original of MCP `ac_run_evidence`: what one contained run did, step by step, assembled online
//! from its own ledger events. The run is found as `ac_get_run` finds it (runtime-client
//! `contained_run_status`, by request_id or run_id); its events are read with one Forensic
//! QueryEvents by its request_id, which records nothing. Both sides parse their arguments into
//! one [`EvidenceRequest`], read it with [`read`] and answer [`Evidence::render`]: stdout is one
//! line, the same JSON as the `ac_run_evidence` result, with the outcome `run_evidence_reported`
//! (Workflow #378 A5). Only MCP cuts steps to its output budget (`truncated`, command table §2.2).
//!
//! The answer is `{request_id, run_id?, task_id?, steps, final, next_step?, overlay?}`. A step is
//! `{step_index, page, targets, action?, frames}` in the order the run started its steps:
//! - `page` is the page the step started from;
//! - `targets` are the targets of the recognition the step decided on (the frame its input was
//!   computed on, else the last frame recognised before it started), each `{target, matched,
//!   box?}` as the recognition fact records it (the ledger's recognition fact keeps no score; a
//!   template's score is in the overlay);
//! - `action` is the step's first input, `{kind, point?, rect?}`: `point` where a tap or long tap
//!   lands, `rect` the one box its point was drawn in;
//! - `frames` are the material refs (`{ref}`, for `ac_material`) of the frame the step decided
//!   on and of every frame captured while it ran.
//!
//! `final` is the terminal's page and the material ref of the run's last frame. `overlay` is the
//! material ref of the run's task diagnostic stream, the JSON with every evaluated target's box
//! and score per frame; `ac_material` reads it, and exports it whole when it is large.
//!
//! #411 A42 (a nine-minute run answered `runtime_event_query_response_too_large`, from_step and
//! limit alike):
//! - The query reads only the record types the evidence folds (`EVIDENCE_EVENT_TYPES`), page by
//!   page (runtime-client `visit_events`), and keeps no page: the run's other records never enter
//!   a page, each page is one QueryEvents answer the Runtime bounds by its response limit, and a
//!   long run has no record count bound.
//! - `--detail summary` (`detail: summary`) answers each step as a summary without its targets
//!   and frames: `{step_index, page, kind?, started_unix_ms, duration_ms?, frames_total,
//!   frame_sequence?, finished?, failure_code?}`, and a page holds up to 100 of them by default.
//! - MCP answers a step that alone is over its output budget with its heavy parts referenced, not
//!   inlined (`Evidence::render_referenced`).

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem, RunState};
use crate::material_export;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CodeStr, Outcome, RawSource};
use actingcommand_contract::{
    ArtifactKind, ArtifactProducer, ArtifactRedactionState, EventActor, EventPayload, EventQuery,
    EventSource, EventType, FrameId, InputAction, InputSamplingEvidence, ProjectedEvent,
    ProjectionPayload, ProjectionProfile, RecognizedTarget, RequestId, RunId, TaskPayload,
    TaskSemanticFact,
};
use actingcommand_runtime_client::{
    ContainedRunState, RunKey, RunStatusMode, RuntimeClient, RuntimeClientConfig,
    RuntimeClientError,
};
use serde_json::{Map, Value, json};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

pub(crate) const COMMAND: &str = "run";
pub(crate) const ACTION: &str = "evidence";
const USAGE: &str = "usage: actingctl run evidence --state-root <path> (--request-id <request_id> | --run-id <run_id>) [--from-step <n>] [--limit <1-100>] [--detail full|summary]";
pub(crate) const DEFAULT_LIMIT: u64 = 20;
pub(crate) const MAX_LIMIT: u64 = 100;
/// The largest `from_step`: a step position counts like a step index.
pub(crate) const MAX_FROM_STEP: u64 = u32::MAX as u64;
/// How long the CLI reads a run's records: the bound of the whole read it replaced (#411 A42).
const CLI_READ_BUDGET: Duration = Duration::from_secs(60);
/// The record types the evidence folds (#411 A42): a step's start, first input and finish, the
/// recognition it decided on, the run's terminal, and the verified frames and diagnostic stream.
/// Each is bounded far below the Runtime's response limit by its contract (pages and targets
/// are counted lists, an outcome envelope is at most 32 KiB, an `artifact.verified` record
/// carries one artifact), so every page of them is answerable.
const EVIDENCE_EVENT_TYPES: [EventType; 8] = [
    EventType::TaskStepStarted,
    EventType::TaskEffectIntent,
    EventType::TaskStepFinished,
    EventType::TaskRecognitionCompleted,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
    EventType::ArtifactVerified,
];

/// How each step is answered (#411 A42): whole, or as a summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepDetail {
    Full,
    Summary,
}

impl StepDetail {
    /// `full` or `summary`, as `--detail` and `detail` give it; `None` for any other word.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "full" => Some(Self::Full),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }

    /// The page length when none is given: a page of summaries holds as many steps as a page
    /// may, so that a long run fits in one page or a few.
    pub(crate) const fn default_limit(self) -> u64 {
        match self {
            Self::Full => DEFAULT_LIMIT,
            Self::Summary => MAX_LIMIT,
        }
    }
}

/// The arguments both sides take, typed (R2; `--request-id` ↔ `handle`, as for `ac_get_run`).
pub(crate) struct EvidenceRequest {
    pub(crate) run: RunKey,
    /// The position of the first step to answer, from 0, in the order the run started its steps.
    pub(crate) from_step: u64,
    pub(crate) limit: u64,
    /// #411 A42: whole steps, or step summaries.
    pub(crate) detail: StepDetail,
}

/// Why a run's evidence was refused or failed.
#[derive(Debug)]
pub(crate) enum Failure {
    /// A flag or argument is unknown, repeated, missing its value, malformed, out of its range
    /// (`range`) or at odds with another: the CLI flag as given, or the MCP argument's name.
    Argument {
        argument: String,
        problem: ArgumentProblem,
        range: Option<(u64, u64)>,
    },
    /// No run has this request_id (given) or run_id.
    RunUnknown {
        request_id: Option<RequestId>,
    },
    /// A ledger record of the run does not encode as JSON.
    Output(serde_json::Error),
    Runtime(RuntimeClientError),
}

impl Failure {
    pub(crate) fn argument(argument: impl AsRef<OsStr>, problem: ArgumentProblem) -> Self {
        Self::Argument {
            argument: argument.as_ref().to_string_lossy().into_owned(),
            problem,
            range: None,
        }
    }

    /// The failure as an outcome (Workflow #378 A5). An argument failure is the CLI's
    /// `actingctl_usage_invalid` with `argument`, `argument_problem` and, for a range,
    /// `range_min` and `range_max` (MCP answers `arguments_invalid` with the same values). A run
    /// nobody has is `run_unknown` with the request_id given and `run_state` `not_found`. A record
    /// that does not encode is `actingctl_output_failed`, caused by the encoder's failure.
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Argument {
                argument,
                problem,
                range,
            } => {
                let outcome = Outcome::new(ActingctlCode::ActingctlUsageInvalid)
                    .argument(argument.as_str())
                    .argument_problem(*problem);
                match range {
                    Some((minimum, maximum)) => outcome.range_min(*minimum).range_max(*maximum),
                    None => outcome,
                }
            }
            Self::RunUnknown { request_id } => {
                let outcome = Outcome::new(ActingctlCode::RunUnknown).run_state(RunState::NotFound);
                match request_id {
                    Some(request_id) => outcome.request_id(id_text(request_id).as_str()),
                    None => outcome,
                }
            }
            Self::Output(error) => Outcome::new(ActingctlCode::ActingctlOutputFailed).caused_by(
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(error.to_string()),
            ),
            Self::Runtime(error) => error.outcome(),
        }
    }
}

/// An identifier as its text (also the request and correlation ids of an expired `--wait`,
/// Workflow #389 MCP-21).
pub(crate) fn id_text(id: &impl serde::Serialize) -> String {
    match json!(id) {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// A request_id as `--request-id` or `handle` gives it; `None` when it is none.
pub(crate) fn parse_request_id(text: &str) -> Option<RequestId> {
    serde_json::from_value(json!(text)).ok()
}

/// A run_id as `--run-id` or `run_id` gives it; `None` when it is none.
pub(crate) fn parse_run_id(text: &str) -> Option<RunId> {
    serde_json::from_value(json!(text)).ok()
}

/// One run's evidence, before rendering: every step of the run in order, and the page asked
/// for.
pub(crate) struct Evidence {
    /// `request_id`, `run_id`, `task_id`.
    pub(crate) identity: Map<String, Value>,
    pub(crate) steps: Vec<StepEvidence>,
    pub(crate) final_: Value,
    pub(crate) overlay: Option<Value>,
    pub(crate) from_step: usize,
    pub(crate) limit: usize,
    /// #411 A42: how each step of the page is answered.
    pub(crate) detail: StepDetail,
}

/// What MCP leaves out of a step that alone is over its output budget (#411 A42): its frames
/// first, then its targets too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Omit {
    Frames,
    FramesAndTargets,
}

impl Evidence {
    /// How many steps the page asked for holds.
    pub(crate) fn page_len(&self) -> usize {
        self.steps
            .len()
            .saturating_sub(self.from_step)
            .min(self.limit)
    }

    /// The result with the first `kept` steps of the page, each answered as `detail` asks. With
    /// fewer steps than the page holds it is `truncated`; `next_step` is the position after the
    /// last step answered while steps remain after it.
    pub(crate) fn render(&self, kept: usize) -> Value {
        self.render_page(kept, None)
    }

    /// The result with the page's first step alone, its `omit` parts left out and referenced
    /// (#411 A42): MCP's answer when that step alone is over the output budget. A summary is
    /// never cut this way.
    pub(crate) fn render_referenced(&self, omit: Omit) -> Value {
        self.render_page(1, Some(omit))
    }

    fn render_page(&self, kept: usize, omit: Option<Omit>) -> Value {
        let page_len = self.page_len();
        let kept = kept.min(page_len);
        let start = self.from_step.min(self.steps.len());
        let end = start + kept;
        let rows = self.steps[start..end]
            .iter()
            .map(|step| match (self.detail, omit) {
                (StepDetail::Summary, _) => step.summary_row(),
                (StepDetail::Full, None) => step.full_row(),
                (StepDetail::Full, Some(omit)) => step.referenced_row(omit),
            })
            .collect();
        let mut result = self.identity.clone();
        result.insert("steps".to_owned(), Value::Array(rows));
        result.insert("final".to_owned(), self.final_.clone());
        if end < self.steps.len() {
            result.insert("next_step".to_owned(), json!(end));
        }
        if let Some(overlay) = &self.overlay {
            result.insert("overlay".to_owned(), overlay.clone());
        }
        if kept < page_len {
            result.insert("truncated".to_owned(), json!(true));
        }
        Value::Object(result)
    }
}

/// One step as the evidence answers it, in three forms (#411 A42): whole, with its heavy parts
/// referenced, and as a summary.
pub(crate) struct StepEvidence {
    pub(crate) step_index: u32,
    /// The page it started from.
    pub(crate) page: String,
    /// The targets of the recognition it decided on, each `{target, matched, box?}`.
    pub(crate) targets: Vec<Value>,
    /// Its first input, `{kind, point?, rect?}`.
    pub(crate) action: Option<Value>,
    /// The material refs of the frame it decided on, first when known (`decided_frame`), and of
    /// every frame captured while it ran, each with the sequence of its `artifact.verified`.
    pub(crate) frames: Vec<(u64, String)>,
    /// Whether the first of `frames` is the frame it decided on.
    pub(crate) decided_frame: bool,
    /// The ledger sequences of its first and last record: its recognition, its frames, its
    /// start, its input and its finish.
    pub(crate) first_sequence: u64,
    pub(crate) last_sequence: u64,
    /// When it started and, once it did, finished.
    pub(crate) started_unix_ms: u64,
    pub(crate) finished_unix_ms: Option<u64>,
    /// The run's terminal failure, on the step that was running when the run ended.
    pub(crate) failure: Option<CodeStr>,
}

impl StepEvidence {
    /// `{step_index, page, targets, action?, frames}` (command table §4.7).
    pub(crate) fn full_row(&self) -> Value {
        let mut row = Map::new();
        row.insert("step_index".to_owned(), json!(self.step_index));
        row.insert("page".to_owned(), json!(self.page));
        row.insert("targets".to_owned(), Value::Array(self.targets.clone()));
        if let Some(action) = &self.action {
            row.insert("action".to_owned(), action.clone());
        }
        row.insert("frames".to_owned(), self.frame_refs());
        Value::Object(row)
    }

    /// The step with its `omit` parts left out (#411 A42): `omitted` names them, `frames_total`
    /// and `targets_total` count what was left out, and `ledger_range` `{after_sequence,
    /// through_sequence}` is the run's ledger stretch that holds them (its
    /// `task.recognition_completed` and `artifact.verified` records, for `ac_events` with the
    /// run's request_id).
    pub(crate) fn referenced_row(&self, omit: Omit) -> Value {
        let mut row = Map::new();
        row.insert("step_index".to_owned(), json!(self.step_index));
        row.insert("page".to_owned(), json!(self.page));
        let mut omitted = vec!["frames"];
        if omit == Omit::FramesAndTargets {
            omitted.push("targets");
            row.insert("targets_total".to_owned(), json!(self.targets.len()));
        } else {
            row.insert("targets".to_owned(), Value::Array(self.targets.clone()));
        }
        if let Some(action) = &self.action {
            row.insert("action".to_owned(), action.clone());
        }
        row.insert("frames_total".to_owned(), json!(self.frames.len()));
        row.insert("omitted".to_owned(), json!(omitted));
        row.insert(
            "ledger_range".to_owned(),
            json!({
                "after_sequence": self.first_sequence.saturating_sub(1),
                "through_sequence": self.last_sequence,
            }),
        );
        Value::Object(row)
    }

    /// `{step_index, page, kind?, started_unix_ms, duration_ms?, frames_total, frame_sequence?,
    /// finished?, failure_code?}` (#411 A42): its first input's kind; when it started and how
    /// long it took to finish; how many frames it has and the ledger sequence of the frame it
    /// decided on (its `artifact.verified`; the step's whole answer names its material ref);
    /// `finished: false` when it never finished, and then the run's terminal failure code when
    /// the run ended in it.
    pub(crate) fn summary_row(&self) -> Value {
        let mut row = Map::new();
        row.insert("step_index".to_owned(), json!(self.step_index));
        row.insert("page".to_owned(), json!(self.page));
        if let Some(kind) = self.action.as_ref().and_then(|action| action.get("kind")) {
            row.insert("kind".to_owned(), kind.clone());
        }
        row.insert("started_unix_ms".to_owned(), json!(self.started_unix_ms));
        if let Some(finished) = self.finished_unix_ms {
            row.insert(
                "duration_ms".to_owned(),
                json!(finished.saturating_sub(self.started_unix_ms)),
            );
        }
        row.insert("frames_total".to_owned(), json!(self.frames.len()));
        if let Some((sequence, _)) = self.frames.first().filter(|_| self.decided_frame) {
            row.insert("frame_sequence".to_owned(), json!(sequence));
        }
        if self.finished_unix_ms.is_none() {
            row.insert("finished".to_owned(), json!(false));
        }
        // A code's name is not a `json!` key (outcome-code spec §12); it is inserted by name.
        if let Some(failure) = &self.failure {
            row.insert("failure_code".to_owned(), json!(failure));
        }
        Value::Object(row)
    }

    fn frame_refs(&self) -> Value {
        Value::Array(
            self.frames
                .iter()
                .map(|(_, reference)| json!({"ref": reference}))
                .collect(),
        )
    }
}

/// The evidence of the run `request` names, read by `client` until `deadline`: the run is found
/// as `ac_get_run` finds it, then its records of `EVIDENCE_EVENT_TYPES` are read page by page
/// and folded as each page arrives (#411 A42).
pub(crate) fn read(
    client: &RuntimeClient,
    request: &EvidenceRequest,
    deadline: Instant,
) -> Result<Evidence, Failure> {
    let status = client
        .contained_run_status(request.run, RunStatusMode::Brief)
        .map_err(Failure::Runtime)?;
    let given = match request.run {
        RunKey::RequestId(request_id) => Some(request_id),
        RunKey::RunId(_) => None,
    };
    let request_id = match (status.state, status.request_id) {
        (ContainedRunState::NotFound, _) | (_, None) => {
            return Err(Failure::RunUnknown { request_id: given });
        }
        (_, Some(request_id)) => request_id,
    };
    let mut assembly = Assembly::default();
    client
        .visit_events(
            EventQuery {
                request_id: Some(request_id),
                event_types: EVIDENCE_EVENT_TYPES.to_vec(),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
            deadline,
            |event| assembly.push(event),
        )
        .map_err(Failure::Runtime)?;
    let mut identity = Map::new();
    identity.insert("request_id".to_owned(), json!(request_id));
    if let Some(run_id) = status.run_id {
        identity.insert("run_id".to_owned(), json!(run_id));
    }
    if let Some(task_id) = status.task_id {
        identity.insert("task_id".to_owned(), json!(task_id));
    }
    let assembled = assembly.finish()?;
    Ok(Evidence {
        identity,
        steps: assembled.steps,
        final_: assembled.final_,
        overlay: assembled.overlay,
        from_step: usize::try_from(request.from_step).unwrap_or(usize::MAX),
        limit: usize::try_from(request.limit).unwrap_or(usize::MAX),
        detail: request.detail,
    })
}

/// What a run's events show, before paging.
pub(crate) struct Assembled {
    pub(crate) steps: Vec<StepEvidence>,
    pub(crate) final_: Value,
    pub(crate) overlay: Option<Value>,
}

/// One step as its facts record it.
struct Step {
    step_index: u32,
    page: String,
    started: u64,
    started_unix_ms: u64,
    finished: Option<u64>,
    finished_unix_ms: Option<u64>,
    /// The sequence of its first input, the frame that input was computed on, and the input.
    effect: Option<(u64, Option<FrameId>, Value)>,
}

/// One recognition fact: its sequence, its frame and its targets as the step rows show them.
struct Recognition {
    sequence: u64,
    frame: Option<FrameId>,
    targets: Vec<Value>,
}

/// A run's records folded as they arrive, oldest first (#411 A42): the steps, the recognitions
/// and frames the step rows are built from, the terminal and the diagnostic stream. The first
/// input that does not encode is kept and fails `finish`.
#[derive(Default)]
struct Assembly {
    steps: Vec<Step>,
    recognitions: Vec<Recognition>,
    /// Every verified capture frame: its sequence, frame and material ref.
    frames: Vec<(u64, Option<FrameId>, String)>,
    final_page: Option<String>,
    failure: Option<CodeStr>,
    overlay: Option<String>,
    unencodable: Option<serde_json::Error>,
}

impl Assembly {
    fn push(&mut self, event: &ProjectedEvent) {
        if event.event_type == EventType::ArtifactVerified {
            for artifact in &event.artifacts {
                if artifact.kind == ArtifactKind::CaptureFrame
                    && artifact.producer == ArtifactProducer::CaptureStore
                {
                    let frame = artifact
                        .frame_id
                        .or_else(|| event.links.frame_id().copied());
                    self.frames.push((
                        event.sequence,
                        frame,
                        material_export::material_ref(event, artifact),
                    ));
                }
                // The run's task diagnostic stream: sealed by the artifact store when the run
                // ends, with no frame of its own, and kept private until redacted.
                if artifact.kind == ArtifactKind::DiagnosticJson
                    && artifact.producer == ArtifactProducer::ArtifactStore
                    && artifact.redaction_state == ArtifactRedactionState::Pending
                    && event.links.frame_id().is_none()
                {
                    self.overlay = Some(material_export::material_ref(event, artifact));
                }
            }
        }
        let ProjectionPayload::Full(payload) = &event.payload else {
            return;
        };
        let EventPayload::Task(TaskPayload::Semantic(semantic)) = payload.as_ref() else {
            return;
        };
        match semantic.fact() {
            TaskSemanticFact::StepStarted {
                step_index,
                from_page,
                ..
            } => self.steps.push(Step {
                step_index: *step_index,
                page: from_page.clone(),
                started: event.sequence,
                started_unix_ms: event.timestamp_unix_ms,
                finished: None,
                finished_unix_ms: None,
                effect: None,
            }),
            TaskSemanticFact::EffectIntent {
                step_index, action, ..
            } => {
                if let Some(step) = self
                    .steps
                    .iter_mut()
                    .rev()
                    .find(|step| step.step_index == *step_index && step.finished.is_none())
                    && step.effect.is_none()
                {
                    match action_value(action, semantic.sampling()) {
                        Ok(value) => {
                            step.effect =
                                Some((event.sequence, event.links.frame_id().copied(), value));
                        }
                        Err(error) => {
                            if self.unencodable.is_none() {
                                self.unencodable = Some(error);
                            }
                        }
                    }
                }
            }
            TaskSemanticFact::StepFinished { step_index, .. } => {
                if let Some(step) = self
                    .steps
                    .iter_mut()
                    .rev()
                    .find(|step| step.step_index == *step_index && step.finished.is_none())
                {
                    step.finished = Some(event.sequence);
                    step.finished_unix_ms = Some(event.timestamp_unix_ms);
                }
            }
            TaskSemanticFact::RecognitionCompleted { targets, .. } => {
                self.recognitions.push(Recognition {
                    sequence: event.sequence,
                    frame: event.links.frame_id().copied(),
                    targets: targets.iter().map(target_value).collect(),
                });
            }
            TaskSemanticFact::TerminalCommitted {
                final_page: page,
                outcome,
                ..
            } => {
                self.final_page = page.clone();
                self.failure = outcome.as_ref().map(|outcome| outcome.code().clone());
            }
            _ => {}
        }
    }

    /// The steps, final and overlay of the folded records.
    fn finish(self) -> Result<Assembled, Failure> {
        if let Some(error) = self.unencodable {
            return Err(Failure::Output(error));
        }
        let last = self.steps.len().checked_sub(1);
        let steps = self
            .steps
            .iter()
            .enumerate()
            .map(|(position, step)| {
                // The step ends with its finish, else where the next step starts.
                let end = step.finished.unwrap_or_else(|| {
                    self.steps
                        .get(position + 1)
                        .map_or(u64::MAX, |next| next.started.saturating_sub(1))
                });
                let mut evidence = step_evidence(step, end, &self.recognitions, &self.frames);
                // The run ended in the step it never finished: its terminal failure is there.
                if Some(position) == last && step.finished.is_none() {
                    evidence.failure.clone_from(&self.failure);
                }
                evidence
            })
            .collect();
        let mut final_ = Map::new();
        if let Some(page) = self.final_page {
            final_.insert("page".to_owned(), json!(page));
        }
        if let Some((_, _, reference)) = self.frames.last() {
            final_.insert("frame_ref".to_owned(), json!(reference));
        }
        Ok(Assembled {
            steps,
            final_: Value::Object(final_),
            overlay: self.overlay.map(|reference| json!({"ref": reference})),
        })
    }
}

/// One step's evidence: the recognition it decided on, its input and its frames.
fn step_evidence(
    step: &Step,
    end: u64,
    recognitions: &[Recognition],
    frames: &[(u64, Option<FrameId>, String)],
) -> StepEvidence {
    // The recognition of the frame the step's input was computed on, else the last one before
    // the step started.
    let recognition = match &step.effect {
        Some((sequence, Some(frame), _)) => recognitions.iter().rev().find(|recognition| {
            recognition.sequence < *sequence && recognition.frame == Some(*frame)
        }),
        _ => recognitions
            .iter()
            .rev()
            .find(|recognition| recognition.sequence < step.started),
    };
    let decided_on = match &step.effect {
        Some((_, Some(frame), _)) => Some(*frame),
        _ => recognition.and_then(|recognition| recognition.frame),
    };
    let targets = recognition.map_or_else(Vec::new, |recognition| recognition.targets.clone());
    let mut references: Vec<(u64, String)> = Vec::new();
    if let Some(frame) = decided_on
        && let Some((sequence, _, reference)) = frames
            .iter()
            .rev()
            .find(|(sequence, candidate, _)| *candidate == Some(frame) && *sequence <= end)
    {
        references.push((*sequence, reference.clone()));
    }
    let decided_frame = !references.is_empty();
    for (sequence, _, reference) in frames
        .iter()
        .filter(|(sequence, _, _)| *sequence > step.started && *sequence <= end)
    {
        if !references.iter().any(|(_, known)| known == reference) {
            references.push((*sequence, reference.clone()));
        }
    }
    let effect = step.effect.as_ref().map(|(sequence, _, _)| *sequence);
    let parts = [
        Some(step.started),
        step.finished,
        effect,
        recognition.map(|recognition| recognition.sequence),
    ]
    .into_iter()
    .flatten()
    .chain(references.iter().map(|(sequence, _)| *sequence));
    let (first_sequence, last_sequence) = parts.fold((u64::MAX, 0), |(first, last), sequence| {
        (first.min(sequence), last.max(sequence))
    });
    StepEvidence {
        step_index: step.step_index,
        page: step.page.clone(),
        targets,
        action: step.effect.as_ref().map(|(_, _, action)| action.clone()),
        frames: references,
        decided_frame,
        first_sequence,
        last_sequence,
        started_unix_ms: step.started_unix_ms,
        finished_unix_ms: step.finished_unix_ms,
        failure: None,
    }
}

/// `{target, matched, box?}`: the target, whether it passed, and its region in frame pixels.
fn target_value(target: &RecognizedTarget) -> Value {
    let mut value = Map::new();
    value.insert("target".to_owned(), json!(target.target_id));
    value.insert("matched".to_owned(), json!(target.passed));
    if let Some(region) = &target.region {
        value.insert("box".to_owned(), json!(region));
    }
    Value::Object(value)
}

/// `{kind, point?, rect?}`: the input's kind; where a tap or long tap lands; the one box its
/// point was drawn in.
fn action_value(
    action: &InputAction,
    sampling: Option<&InputSamplingEvidence>,
) -> Result<Value, serde_json::Error> {
    let input = serde_json::to_value(action)?;
    let mut value = Map::new();
    value.insert("kind".to_owned(), input["kind"].clone());
    if let (Some(x), Some(y)) = (input["x"].as_i64(), input["y"].as_i64()) {
        value.insert("point".to_owned(), json!({"x": x, "y": y}));
    }
    if let Some([region]) = sampling.map(InputSamplingEvidence::source_regions) {
        value.insert(
            "rect".to_owned(),
            json!({
                "x": region.x(),
                "y": region.y(),
                "width": region.width(),
                "height": region.height(),
            }),
        );
    }
    Ok(Value::Object(value))
}

/// Runs `run evidence`; `None` leaves every other command to the CLI, unchanged. One result line
/// on stdout (Workflow #378 A5): the evidence with `run_evidence_reported`, or `{outcome}` with
/// the failure (an argument failure's developer line adds the usage) and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND || arguments.get(1)?.to_str()? != ACTION {
        return None;
    }
    Some(match run(&arguments[2..]) {
        Ok(result) => cli_line::success(result, &Outcome::new(ActingctlCode::RunEvidenceReported)),
        Err(failure @ Failure::Argument { .. }) => {
            cli_line::failure(&failure.outcome(), Some(USAGE))
        }
        Err(failure) => cli_line::failure(&failure.outcome(), None),
    })
}

fn run(arguments: &[OsString]) -> Result<Map<String, Value>, Failure> {
    let (state_root, request) = parse(arguments)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(Failure::Runtime)?;
    let evidence = read(&client, &request, Instant::now() + CLI_READ_BUDGET)?;
    Ok(cli_line::members(
        evidence.render(evidence.page_len()),
        ACTION,
    ))
}

/// The flags `flags_of` names for `run evidence`, each at most once and with a value; any other
/// word, a repeated flag or a malformed value is `actingctl_usage_invalid` with the word
/// (`argument`) and what is wrong with it (`argument_problem`). Exactly one of `--request-id`
/// and `--run-id`.
fn parse(arguments: &[OsString]) -> Result<(PathBuf, EvidenceRequest), Failure> {
    let takes = crate::flags_of(COMMAND, Some(ACTION)).unwrap_or_default();
    let mut given: Vec<(&str, String)> = Vec::new();
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        let Some(text) = word.to_str() else {
            return Err(Failure::argument(word, ArgumentProblem::NotUtf8));
        };
        let Some(flag) = takes.iter().copied().find(|flag| *flag == text) else {
            return Err(Failure::argument(text, ArgumentProblem::Unknown));
        };
        if given.iter().any(|(name, _)| *name == flag) {
            return Err(Failure::argument(flag, ArgumentProblem::Repeated));
        }
        let value = words
            .next()
            .ok_or_else(|| Failure::argument(flag, ArgumentProblem::ValueMissing))?
            .to_str()
            .ok_or_else(|| Failure::argument(flag, ArgumentProblem::NotUtf8))?;
        if value.trim().is_empty() {
            return Err(Failure::argument(flag, ArgumentProblem::ValueMissing));
        }
        given.push((flag, value.to_owned()));
    }
    let mut value = |flag: &str| {
        given
            .iter()
            .position(|(name, _)| *name == flag)
            .map(|index| given.swap_remove(index).1)
    };
    let state_root = value("--state-root")
        .ok_or_else(|| Failure::argument("--state-root", ArgumentProblem::Required))?;
    let run = match (value("--request-id"), value("--run-id")) {
        (Some(text), None) => RunKey::RequestId(
            parse_request_id(&text)
                .ok_or_else(|| Failure::argument("--request-id", ArgumentProblem::NotRecognised))?,
        ),
        (None, Some(text)) => RunKey::RunId(
            parse_run_id(&text)
                .ok_or_else(|| Failure::argument("--run-id", ArgumentProblem::NotRecognised))?,
        ),
        (Some(_), Some(_)) => {
            return Err(Failure::argument("--run-id", ArgumentProblem::Conflicting));
        }
        (None, None) => {
            return Err(Failure::argument("--request-id", ArgumentProblem::Required));
        }
    };
    let number = |flag: &str, text: Option<String>, minimum: u64, maximum: u64| {
        text.map(|text| match text.parse::<u64>() {
            Ok(number) if (minimum..=maximum).contains(&number) => Ok(number),
            Ok(_) => Err(Failure::Argument {
                argument: flag.to_owned(),
                problem: ArgumentProblem::OutOfRange,
                range: Some((minimum, maximum)),
            }),
            Err(_) => Err(Failure::argument(flag, ArgumentProblem::WrongType)),
        })
        .transpose()
    };
    let from_step = number("--from-step", value("--from-step"), 0, MAX_FROM_STEP)?.unwrap_or(0);
    // #411 A42: `--detail full|summary`, full by default.
    let detail = match value("--detail") {
        None => StepDetail::Full,
        Some(text) => StepDetail::parse(&text)
            .ok_or_else(|| Failure::argument("--detail", ArgumentProblem::NotRecognised))?,
    };
    let limit =
        number("--limit", value("--limit"), 1, MAX_LIMIT)?.unwrap_or(detail.default_limit());
    Ok((
        PathBuf::from(state_root),
        EvidenceRequest {
            run,
            from_step,
            limit,
            detail,
        },
    ))
}

// Test-only: Workflow #389 MCP-6, the refusals of `run evidence` (command table §4.7).
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::outcome::Category;

    fn words(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[track_caller]
    fn refused(arguments: &[&str]) -> Outcome {
        match parse(&words(arguments)) {
            Ok(_) => panic!("{arguments:?} was taken where a refusal is required"),
            Err(failure) => failure.outcome(),
        }
    }

    #[track_caller]
    fn usage(outcome: &Outcome, argument: &str, problem: &str) {
        assert!(
            outcome.is(ActingctlCode::ActingctlUsageInvalid),
            "{outcome}"
        );
        assert_eq!(outcome.category(), Category::Error);
        assert_eq!(
            (
                outcome.values().get("argument"),
                outcome.values().get("argument_problem")
            ),
            (Some(&json!(argument)), Some(&json!(problem))),
            "{outcome}"
        );
    }

    #[test]
    fn each_refusal_of_run_evidence_carries_its_registered_code() {
        let request_id = "request_0123456789abcdef0123456789abcdef";

        // Command table §4.7: exactly one of the request_id and the run_id, as for ac_get_run.
        usage(
            &refused(&["--state-root", "state"]),
            "--request-id",
            "required",
        );
        usage(
            &refused(&[
                "--state-root",
                "state",
                "--request-id",
                request_id,
                "--run-id",
                "run_0123456789abcdef0123456789abcdef",
            ]),
            "--run-id",
            "conflicting",
        );
        usage(
            &refused(&["--state-root", "state", "--request-id", "neutral"]),
            "--request-id",
            "not_recognised",
        );

        // §4.7: limit is 1-100; from_step counts from 0.
        let limit = refused(&[
            "--state-root",
            "state",
            "--request-id",
            request_id,
            "--limit",
            "101",
        ]);
        usage(&limit, "--limit", "out_of_range");
        assert_eq!(
            (
                limit.values().get("range_min"),
                limit.values().get("range_max")
            ),
            (Some(&json!(1)), Some(&json!(MAX_LIMIT))),
            "{limit}"
        );
        usage(
            &refused(&[
                "--state-root",
                "state",
                "--request-id",
                request_id,
                "--from-step",
                "-1",
            ]),
            "--from-step",
            "wrong_type",
        );

        // §4.7 (run_unknown): no run has the request_id given; the outcome names it and the
        // state not_found.
        let given = parse_request_id(request_id).expect("a request_id");
        let unknown = Failure::RunUnknown {
            request_id: Some(given),
        }
        .outcome();
        assert!(unknown.is(ActingctlCode::RunUnknown), "{unknown}");
        assert_eq!(unknown.category(), Category::Error);
        assert_eq!(
            (
                unknown.values().get("request_id"),
                unknown.values().get("run_state")
            ),
            (Some(&json!(request_id)), Some(&json!("not_found"))),
            "{unknown}"
        );
    }
}
