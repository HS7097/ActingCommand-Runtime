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

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem, RunState};
use crate::material_export;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Outcome, RawSource};
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

pub(crate) const COMMAND: &str = "run";
pub(crate) const ACTION: &str = "evidence";
const USAGE: &str = "usage: actingctl run evidence --state-root <path> (--request-id <request_id> | --run-id <run_id>) [--from-step <n>] [--limit <1-100>]";
pub(crate) const DEFAULT_LIMIT: u64 = 20;
pub(crate) const MAX_LIMIT: u64 = 100;
/// The largest `from_step`: a step position counts like a step index.
pub(crate) const MAX_FROM_STEP: u64 = u32::MAX as u64;

/// The arguments both sides take, typed (R2; `--request-id` ↔ `handle`, as for `ac_get_run`).
pub(crate) struct EvidenceRequest {
    pub(crate) run: RunKey,
    /// The position of the first step to answer, from 0, in the order the run started its steps.
    pub(crate) from_step: u64,
    pub(crate) limit: u64,
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

/// An identifier as its text.
fn id_text(id: &impl serde::Serialize) -> String {
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

/// One run's evidence, before rendering: every step row of the run in order, and the page asked
/// for.
pub(crate) struct Evidence {
    /// `request_id`, `run_id`, `task_id`.
    pub(crate) identity: Map<String, Value>,
    pub(crate) steps: Vec<Value>,
    pub(crate) final_: Value,
    pub(crate) overlay: Option<Value>,
    pub(crate) from_step: usize,
    pub(crate) limit: usize,
}

impl Evidence {
    /// How many steps the page asked for holds.
    pub(crate) fn page_len(&self) -> usize {
        self.steps
            .len()
            .saturating_sub(self.from_step)
            .min(self.limit)
    }

    /// The result with the first `kept` steps of the page. With fewer steps than the page holds
    /// it is `truncated`; `next_step` is the position after the last step answered while steps
    /// remain after it.
    pub(crate) fn render(&self, kept: usize) -> Value {
        let page_len = self.page_len();
        let kept = kept.min(page_len);
        let start = self.from_step.min(self.steps.len());
        let end = start + kept;
        let mut result = self.identity.clone();
        result.insert(
            "steps".to_owned(),
            Value::Array(self.steps[start..end].to_vec()),
        );
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

/// The evidence of the run `request` names, read by `client`: the run is found as `ac_get_run`
/// finds it, then every event of its request is read at once.
pub(crate) fn read(client: &RuntimeClient, request: &EvidenceRequest) -> Result<Evidence, Failure> {
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
    let events = client
        .query_events(
            EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
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
    let assembled = assemble(&events)?;
    Ok(Evidence {
        identity,
        steps: assembled.steps,
        final_: assembled.final_,
        overlay: assembled.overlay,
        from_step: usize::try_from(request.from_step).unwrap_or(usize::MAX),
        limit: usize::try_from(request.limit).unwrap_or(usize::MAX),
    })
}

/// What a run's events show, before paging.
pub(crate) struct Assembled {
    pub(crate) steps: Vec<Value>,
    pub(crate) final_: Value,
    pub(crate) overlay: Option<Value>,
}

/// One step as its facts record it.
struct Step {
    step_index: u32,
    page: String,
    started: u64,
    finished: Option<u64>,
    /// The sequence of its first input, the frame that input was computed on, and the input.
    effect: Option<(u64, Option<FrameId>, Value)>,
}

/// One recognition fact: its sequence, its frame and its targets.
struct Recognition<'e> {
    sequence: u64,
    frame: Option<FrameId>,
    targets: &'e [RecognizedTarget],
}

/// The steps, final and overlay of a run's events (oldest first).
pub(crate) fn assemble(events: &[ProjectedEvent]) -> Result<Assembled, Failure> {
    let mut steps: Vec<Step> = Vec::new();
    let mut recognitions = Vec::new();
    // Every verified capture frame: its sequence, frame and material ref.
    let mut frames: Vec<(u64, Option<FrameId>, String)> = Vec::new();
    let mut final_page = None;
    let mut overlay = None;
    for event in events {
        if event.event_type == EventType::ArtifactVerified {
            for artifact in &event.artifacts {
                if artifact.kind == ArtifactKind::CaptureFrame
                    && artifact.producer == ArtifactProducer::CaptureStore
                {
                    let frame = artifact
                        .frame_id
                        .or_else(|| event.links.frame_id().copied());
                    frames.push((
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
                    overlay = Some(material_export::material_ref(event, artifact));
                }
            }
        }
        let ProjectionPayload::Full(payload) = &event.payload else {
            continue;
        };
        let EventPayload::Task(TaskPayload::Semantic(semantic)) = payload.as_ref() else {
            continue;
        };
        match semantic.fact() {
            TaskSemanticFact::StepStarted {
                step_index,
                from_page,
                ..
            } => steps.push(Step {
                step_index: *step_index,
                page: from_page.clone(),
                started: event.sequence,
                finished: None,
                effect: None,
            }),
            TaskSemanticFact::EffectIntent {
                step_index, action, ..
            } => {
                if let Some(step) = steps
                    .iter_mut()
                    .rev()
                    .find(|step| step.step_index == *step_index && step.finished.is_none())
                    && step.effect.is_none()
                {
                    step.effect = Some((
                        event.sequence,
                        event.links.frame_id().copied(),
                        action_value(action, semantic.sampling())?,
                    ));
                }
            }
            TaskSemanticFact::StepFinished { step_index, .. } => {
                if let Some(step) = steps
                    .iter_mut()
                    .rev()
                    .find(|step| step.step_index == *step_index && step.finished.is_none())
                {
                    step.finished = Some(event.sequence);
                }
            }
            TaskSemanticFact::RecognitionCompleted { targets, .. } => {
                recognitions.push(Recognition {
                    sequence: event.sequence,
                    frame: event.links.frame_id().copied(),
                    targets,
                });
            }
            TaskSemanticFact::TerminalCommitted {
                final_page: page, ..
            } => {
                final_page = page.clone();
            }
            _ => {}
        }
    }
    let rows: Vec<Value> = steps
        .iter()
        .enumerate()
        .map(|(position, step)| {
            // The step ends with its finish, else where the next step starts.
            let end = step.finished.unwrap_or_else(|| {
                steps
                    .get(position + 1)
                    .map_or(u64::MAX, |next| next.started.saturating_sub(1))
            });
            step_row(step, end, &recognitions, &frames)
        })
        .collect();
    let mut final_ = Map::new();
    if let Some(page) = final_page {
        final_.insert("page".to_owned(), json!(page));
    }
    if let Some((_, _, reference)) = frames.last() {
        final_.insert("frame_ref".to_owned(), json!(reference));
    }
    Ok(Assembled {
        steps: rows,
        final_: Value::Object(final_),
        overlay: overlay.map(|reference| json!({"ref": reference})),
    })
}

/// One step's row: the recognition it decided on, its input and its frames.
fn step_row(
    step: &Step,
    end: u64,
    recognitions: &[Recognition<'_>],
    frames: &[(u64, Option<FrameId>, String)],
) -> Value {
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
    let targets = recognition.map_or_else(Vec::new, |recognition| {
        recognition.targets.iter().map(target_value).collect()
    });
    let mut references: Vec<&str> = Vec::new();
    if let Some(frame) = decided_on
        && let Some((_, _, reference)) = frames
            .iter()
            .rev()
            .find(|(sequence, candidate, _)| *candidate == Some(frame) && *sequence <= end)
    {
        references.push(reference);
    }
    for (_, _, reference) in frames
        .iter()
        .filter(|(sequence, _, _)| *sequence > step.started && *sequence <= end)
    {
        if !references.contains(&reference.as_str()) {
            references.push(reference);
        }
    }
    let mut row = Map::new();
    row.insert("step_index".to_owned(), json!(step.step_index));
    row.insert("page".to_owned(), json!(step.page));
    row.insert("targets".to_owned(), Value::Array(targets));
    if let Some((_, _, action)) = &step.effect {
        row.insert("action".to_owned(), action.clone());
    }
    row.insert(
        "frames".to_owned(),
        Value::Array(
            references
                .into_iter()
                .map(|reference| json!({"ref": reference}))
                .collect(),
        ),
    );
    Value::Object(row)
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
) -> Result<Value, Failure> {
    let input = serde_json::to_value(action).map_err(Failure::Output)?;
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
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(Failure::Runtime)?;
    let evidence = read(&client, &request)?;
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
    let limit = number("--limit", value("--limit"), 1, MAX_LIMIT)?.unwrap_or(DEFAULT_LIMIT);
    Ok((
        PathBuf::from(state_root),
        EvidenceRequest {
            run,
            from_step,
            limit,
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
