// SPDX-License-Identifier: AGPL-3.0-only

//! What `report export` reads (Workflow #467 RPT-1; `contracts/problem-report.md`, "What is
//! read"): one trait, [`Source`], whose every method is a read, and the Runtime's
//! ([`RuntimeSource`]), which reads through the code of the rows `status`, `runs`, `run show`,
//! `run evidence`, `events` and `material`. Every read names a request id, or an instance with
//! the report's window; none reads without one. No method writes, captures, pins or confirms
//! anything: the trait has no such method.

use super::READ_STEP;
use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem, RunState};
use crate::events::{self, Detail as RowDetail};
use crate::instance_selector::{self, SelectorMiss};
use crate::material;
use crate::run_evidence::{self, Evidence, EvidenceRequest, StepDetail};
use crate::run_show;
use actingcommand_contract::outcome::{CodeStr, Detail, Outcome};
use actingcommand_contract::{
    EventPayload, EventQuery, EventType, InstanceId, LedgerView, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, RequestId, TaskGeometryConclusion, TaskGeometryObservation, TaskPayload,
    TaskSemanticFact,
};
use actingcommand_runtime_client::{
    ContainedRunState, ListedContainedRun, RunListFilter, RuntimeClient,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::Instant;

/// One instance of the Runtime's status: its alias, and its `instance_id` (a made-up source
/// may leave it out).
#[derive(Debug, Clone)]
pub struct Instance {
    pub alias: String,
    pub id: Option<InstanceId>,
}

/// The report's window: the runs first recorded, and the error rows recorded, from
/// `since_unix_ms` (and up to `until_unix_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub since_unix_ms: u64,
    pub until_unix_ms: Option<u64>,
}

/// Which runs a listing keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// Failed, open (admitted or running) and interrupted without a terminal.
    Unfinished,
    /// Every state.
    Any,
}

impl Which {
    /// The states the listing filter keeps; none: every state.
    fn states(self) -> Vec<ContainedRunState> {
        match self {
            Self::Unfinished => vec![
                ContainedRunState::Failed,
                ContainedRunState::Admitted,
                ContainedRunState::Running,
                ContainedRunState::InterruptedUnterminated,
            ],
            Self::Any => Vec::new(),
        }
    }
}

/// One run of a listing, as the report uses it.
#[derive(Debug, Clone)]
pub struct RunRow {
    pub request_id: String,
    pub run_id: Option<String>,
    pub alias: String,
    pub state: RunState,
    /// The admitted main package's `package_id`.
    pub package: Option<String>,
    /// The code of the run's terminal failure, when it has one.
    pub ended_with: Option<CodeStr>,
    pub final_page: Option<String>,
    pub first_unix_ms: u64,
    pub ended_unix_ms: Option<u64>,
}

/// A listing: its runs, newest first, and whether its read budget ended it early.
#[derive(Debug, Clone, Default)]
pub struct Listed {
    pub runs: Vec<RunRow>,
    pub incomplete: bool,
}

/// The rows of the `errors` view of one instance in the window: the newest that were kept,
/// oldest first, each as `events --view errors --detail summary` answers it with its `module`,
/// and how many the window held.
#[derive(Debug, Clone, Default)]
pub struct Errors {
    pub rows: Vec<Value>,
    pub total: u64,
}

/// The display map a geometry observation used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayFacts {
    /// `identity`, `scale` or `translate`.
    pub kind: String,
    pub scale_num: u32,
    pub scale_den: u32,
    pub scale_width: u32,
    pub scale_height: u32,
    pub base_width: u32,
    pub base_height: u32,
}

/// One `task.geometry_observed` of a run: the device frame's size as stored, the display map
/// when it is not the plain identity, and the observation's conclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Geometry {
    pub device: Option<(u32, u32)>,
    pub map: Option<DisplayFacts>,
    pub conclusion: String,
    /// The conclusion is `pass` (or the fixture's `fixture_not_applicable`).
    pub passed: bool,
}

/// One target of a recognition record: matched or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetVerdict {
    pub target: String,
    pub matched: bool,
}

/// One candidate page of a recognition record: matched or not, the best target score on it
/// in milli, and the first target that failed on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageVerdict {
    pub page: String,
    pub matched: bool,
    pub best_milli: Option<u32>,
    pub first_failed: Option<String>,
}

/// One `task.recognition_completed` record, as far as it holds scores: its targets matched or
/// not, and each candidate page's verdict (`candidate_scores`, ledger revision 4); no target
/// score and no threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognition {
    pub sequence: u64,
    pub matched_page: Option<String>,
    pub frame: (u32, u32),
    pub targets: Vec<TargetVerdict>,
    pub pages: Vec<PageVerdict>,
    /// The candidate pages after those listed.
    pub omitted: u32,
}

/// Why a material was not read.
#[derive(Debug, Clone)]
pub enum Missed {
    /// It is larger than the bound it was read with: its size.
    TooLarge(u64),
    /// The read failed: its outcome.
    Failed(Outcome),
}

/// What `report export` reads. Every method is a read; none writes, captures, pins or
/// confirms.
pub trait Source {
    /// The instances of the Runtime's status: the one `selector` names (alias, `instance_id` or
    /// ADB port), else every one.
    fn instances(&self, selector: Option<&str>) -> Result<Vec<Instance>, Outcome>;
    /// The runs of `instance` first recorded in `window`, newest first, at most `limit`.
    fn runs(
        &self,
        instance: &Instance,
        window: Window,
        which: Which,
        limit: usize,
    ) -> Result<Listed, Outcome>;
    /// The rows of the `errors` view of `instance` in `window`: the newest `keep`.
    fn errors(&self, instance: &Instance, window: Window, keep: usize) -> Result<Errors, Outcome>;
    /// The run `request_id` submitted, in full (`run show`).
    fn run(&self, request_id: &str) -> Result<Value, Outcome>;
    /// The run's evidence, every step (`run evidence`).
    fn evidence(&self, request_id: &str) -> Result<Evidence, Outcome>;
    /// The run's geometry observations, oldest first.
    fn geometry(&self, request_id: &str) -> Result<Vec<Geometry>, Outcome>;
    /// The run's recognition records from the ledger sequence `from` (through `through`),
    /// oldest first.
    fn recognitions(
        &self,
        request_id: &str,
        from: u64,
        through: Option<u64>,
    ) -> Result<Vec<Recognition>, Outcome>;
    /// The material `reference` names, whole and verified, when it is at most `limit` bytes.
    fn material(&self, reference: &str, limit: u64) -> Result<Vec<u8>, Missed>;
}

/// The Runtime's reads, on one connection, within one deadline: each read stops at the
/// deadline or [`READ_STEP`] after it starts, whichever comes first, and once the deadline has
/// passed none starts (`call_budget_exhausted`).
pub struct RuntimeSource<'c> {
    client: &'c RuntimeClient,
    deadline: Instant,
}

impl<'c> RuntimeSource<'c> {
    pub fn new(client: &'c RuntimeClient, deadline: Instant) -> Self {
        Self { client, deadline }
    }

    /// When the next read stops.
    fn step(&self) -> Result<Instant, Outcome> {
        let now = Instant::now();
        if now >= self.deadline {
            return Err(Outcome::new(ActingctlCode::CallBudgetExhausted));
        }
        Ok(self.deadline.min(now + READ_STEP))
    }
}

/// A value's JSON text: a string as it is, anything else as JSON.
pub(super) fn text_of<T: Serialize>(value: &T) -> String {
    match json!(value) {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// The request id `text` names, else `run_unknown` with it.
fn request_of(text: &str) -> Result<RequestId, Outcome> {
    run_evidence::parse_request_id(text)
        .ok_or_else(|| Outcome::new(ActingctlCode::RunUnknown).request_id(text))
}

/// The instance's id; a source that read the status always has it.
fn id_of(instance: &Instance) -> Result<InstanceId, Outcome> {
    instance
        .id
        .ok_or_else(|| SelectorMiss::Unknown.outcome(&instance.alias))
}

/// The semantic task fact a record carries, when it carries one.
fn semantic(event: &ProjectedEvent) -> Option<&TaskSemanticFact> {
    let ProjectionPayload::Full(payload) = &event.payload else {
        return None;
    };
    let EventPayload::Task(TaskPayload::Semantic(semantic)) = payload.as_ref() else {
        return None;
    };
    Some(semantic.fact())
}

fn row_of(run: &ListedContainedRun, alias: &str) -> RunRow {
    let status = &run.status;
    let terminal = status.terminal.as_ref();
    RunRow {
        request_id: status.request_id.as_ref().map(text_of).unwrap_or_default(),
        run_id: status.run_id.as_ref().map(text_of),
        alias: alias.to_owned(),
        state: RunState::of(status.state),
        package: status.package_label.clone(),
        ended_with: terminal.and_then(|terminal| terminal.failure_code.clone()),
        final_page: terminal.and_then(|terminal| terminal.final_page.clone()),
        first_unix_ms: run.first_fact_unix_ms,
        ended_unix_ms: run.terminal_unix_ms,
    }
}

fn geometry_of(observation: &TaskGeometryObservation) -> Geometry {
    Geometry {
        device: observation
            .frame
            .as_ref()
            .map(|frame| (frame.extent.width(), frame.extent.height())),
        map: observation.display_map.as_ref().map(|map| DisplayFacts {
            kind: text_of(&map.kind),
            scale_num: map.scale_num,
            scale_den: map.scale_den,
            scale_width: map.scale_width,
            scale_height: map.scale_height,
            base_width: map.base_width,
            base_height: map.base_height,
        }),
        conclusion: text_of(&observation.conclusion),
        passed: matches!(
            observation.conclusion,
            TaskGeometryConclusion::Pass | TaskGeometryConclusion::FixtureNotApplicable
        ),
    }
}

/// A material read that answered nothing, as its outcome (as `material`'s row answers it).
fn missed(failure: material::Failure) -> Missed {
    Missed::Failed(match failure {
        material::Failure::Usage(outcome)
        | material::Failure::NotText(outcome)
        | material::Failure::Unavailable(outcome) => outcome,
        material::Failure::OutOfRange {
            argument,
            minimum,
            maximum,
        } => cli_line::usage_invalid(Some(argument), ArgumentProblem::OutOfRange)
            .range_min(minimum)
            .range_max(maximum),
        material::Failure::Runtime(error) => error.outcome(),
        material::Failure::RuntimeNoted { error, reached } => error.outcome().caused_by(reached),
    })
}

impl Source for RuntimeSource<'_> {
    fn instances(&self, selector: Option<&str>) -> Result<Vec<Instance>, Outcome> {
        let status = self.client.status().map_err(|error| error.outcome())?;
        let instances = status.instances();
        let chosen = match selector {
            Some(selector) => std::slice::from_ref(
                instance_selector::select(instances, selector)
                    .map_err(|miss| miss.outcome(selector))?,
            ),
            None => instances,
        };
        Ok(chosen
            .iter()
            .map(|instance| Instance {
                alias: instance.instance_alias().to_owned(),
                id: Some(instance.instance_id()),
            })
            .collect())
    }

    fn runs(
        &self,
        instance: &Instance,
        window: Window,
        which: Which,
        limit: usize,
    ) -> Result<Listed, Outcome> {
        let filter = RunListFilter {
            instance_id: Some(id_of(instance)?),
            states: which.states(),
            ..RunListFilter::default()
        };
        let page = self
            .client
            .list_contained_runs(
                window.since_unix_ms,
                window.until_unix_ms,
                &filter,
                None,
                limit,
                self.step()?,
            )
            .map_err(|error| error.outcome())?;
        Ok(Listed {
            runs: page
                .runs
                .iter()
                .map(|run| row_of(run, &instance.alias))
                .collect(),
            incomplete: page.incomplete,
        })
    }

    fn errors(&self, instance: &Instance, window: Window, keep: usize) -> Result<Errors, Outcome> {
        let query = EventQuery {
            view: Some(LedgerView::Errors),
            instance_id: Some(id_of(instance)?),
            from_timestamp_unix_ms: Some(window.since_unix_ms),
            to_timestamp_unix_ms: window.until_unix_ms,
            ..EventQuery::default()
        };
        let envelope = |outcome: &Outcome| cli_line::outcome_value(outcome, Detail::Full);
        let mut kept = VecDeque::new();
        let mut total = 0_u64;
        self.client
            .visit_events(query, events::PROFILE, self.step()?, |event| {
                total += 1;
                let mut row = events::row(event, RowDetail::Summary, &envelope);
                row["module"] = json!(event.origin.module());
                kept.push_back(row);
                if kept.len() > keep {
                    kept.pop_front();
                }
            })
            .map_err(|error| error.outcome())?;
        Ok(Errors {
            rows: Vec::from(kept),
            total,
        })
    }

    fn run(&self, request_id: &str) -> Result<Value, Outcome> {
        self.step()?;
        let key = run_show::by_request_id(request_id)
            .ok_or_else(|| Outcome::new(ActingctlCode::RunUnknown).request_id(request_id))?;
        run_show::read(self.client, key)
            .map(|status| run_show::answer(&status))
            .map_err(|error| error.outcome())
    }

    fn evidence(&self, request_id: &str) -> Result<Evidence, Outcome> {
        let deadline = self.step()?;
        let run = run_show::by_request_id(request_id)
            .ok_or_else(|| Outcome::new(ActingctlCode::RunUnknown).request_id(request_id))?;
        let request = EvidenceRequest {
            run,
            from_step: 0,
            limit: u64::MAX,
            detail: StepDetail::Full,
        };
        run_evidence::read(self.client, &request, deadline).map_err(|failure| failure.outcome())
    }

    fn geometry(&self, request_id: &str) -> Result<Vec<Geometry>, Outcome> {
        let query = EventQuery {
            request_id: Some(request_of(request_id)?),
            event_types: vec![EventType::TaskGeometryObserved],
            ..EventQuery::default()
        };
        let mut found = Vec::new();
        self.client
            .visit_events(query, ProjectionProfile::Forensic, self.step()?, |event| {
                if let Some(TaskSemanticFact::GeometryObserved { observation }) = semantic(event) {
                    found.push(geometry_of(observation));
                }
            })
            .map_err(|error| error.outcome())?;
        Ok(found)
    }

    fn recognitions(
        &self,
        request_id: &str,
        from: u64,
        through: Option<u64>,
    ) -> Result<Vec<Recognition>, Outcome> {
        let query = EventQuery {
            request_id: Some(request_of(request_id)?),
            event_types: vec![EventType::TaskRecognitionCompleted],
            from_sequence: Some(from),
            to_sequence: through,
            ..EventQuery::default()
        };
        let mut found = Vec::new();
        self.client
            .visit_events(query, ProjectionProfile::Forensic, self.step()?, |event| {
                if let Some(TaskSemanticFact::RecognitionCompleted {
                    matched_page,
                    frame_width,
                    frame_height,
                    targets,
                    candidate_scores,
                    ..
                }) = semantic(event)
                {
                    found.push(Recognition {
                        sequence: event.sequence,
                        matched_page: matched_page.clone(),
                        frame: (*frame_width, *frame_height),
                        targets: targets
                            .iter()
                            .map(|target| TargetVerdict {
                                target: target.target_id.clone(),
                                matched: target.passed,
                            })
                            .collect(),
                        pages: candidate_scores
                            .as_ref()
                            .map(|scores| {
                                scores
                                    .pages
                                    .iter()
                                    .map(|page| PageVerdict {
                                        page: page.page_id.clone(),
                                        matched: page.passed,
                                        best_milli: page.best_score_milli,
                                        first_failed: page.first_failed_target_id.clone(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        omitted: candidate_scores.as_ref().map_or(0, |scores| scores.omitted),
                    });
                }
            })
            .map_err(|error| error.outcome())?;
        Ok(found)
    }

    fn material(&self, reference: &str, limit: u64) -> Result<Vec<u8>, Missed> {
        let deadline = self.step().map_err(Missed::Failed)?;
        let (position, artifact_id) = material::parse_ref(reference).map_err(missed)?;
        let found = material::resolve(self.client, position, artifact_id).map_err(missed)?;
        if found.artifact.byte_count > limit {
            return Err(Missed::TooLarge(found.artifact.byte_count));
        }
        material::read_whole(self.client, &found, deadline, &|| true).map_err(missed)
    }
}
