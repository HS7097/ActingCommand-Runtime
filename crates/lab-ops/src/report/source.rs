// SPDX-License-Identifier: AGPL-3.0-only

//! What `report export` reads (Workflow #467 RPT-1; `contracts/problem-report.md`, "What is
//! read"): one trait, [`Source`], whose every method is a read, and the Runtime's
//! ([`RuntimeSource`]), which reads through the code of the rows `status`, `runs`, `run show`,
//! `run evidence`, `events` and `material`. Every read names a request id, or an instance with
//! the report's window; none reads without one. No method writes, captures, pins or confirms
//! anything: the trait has no such method. Fix round 1: a read that answers nothing says why
//! ([`Missed`], vocabulary `report_part_reason`), and a listing or a search that stopped before
//! its end says so ([`Listed::cut`], [`Errors::stopped`]).

use super::READ_STEP;
use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem, ReportPartReason, RunState};
use crate::events::{self, Detail as RowDetail, EventPages, EventsError};
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
    ClientCode, ContainedRunState, ListedContainedRun, RunListFilter, RuntimeClient,
    RuntimeClientError,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::cell::Cell;
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

/// A listing: its runs, newest first, and, when it was not read to its end, why (its read's
/// limit or the Runtime's read budget, `not_read_to_end`; the report's budget, `budget_spent`):
/// a run it does not hold may still be in the window.
#[derive(Debug, Clone, Default)]
pub struct Listed {
    pub runs: Vec<RunRow>,
    pub cut: Option<ReportPartReason>,
}

/// The newest rows of the `errors` view of one instance in the window, found by the search for
/// the newest records `events` has (fix round 1, item 7): oldest first, each as `events --view
/// errors --detail summary` answers it with its `module`. When fewer than were asked for were
/// found and the search did not reach the window's first record, `searched_from` is the lowest
/// sequence it searched. `stopped` says why older rows of the window were not searched (fix
/// round 2, item 5, one word for each cause): `not_read_to_end`, a read the Runtime cut short
/// ended the search; `search_windows_spent`, the search read its largest window and stopped
/// there.
#[derive(Debug, Clone, Default)]
pub struct Errors {
    pub rows: Vec<Value>,
    pub searched_from: Option<u64>,
    pub stopped: Option<ReportPartReason>,
}

impl Errors {
    /// Whether the search reached the window's first record (or found every row it was asked
    /// for, the newest of the window).
    pub fn searched_to_start(&self) -> bool {
        self.stopped.is_none()
    }
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

/// Why a read answered nothing (fix round 1): the reason, a token of `report_part_reason`, and
/// the read's own outcome when it has one (a fatal one ends the report).
#[derive(Debug, Clone)]
pub struct Missed {
    pub why: ReportPartReason,
    pub cause: Option<Outcome>,
}

impl Missed {
    /// A read that answered nothing for `why`, with its outcome `cause`.
    pub fn new(why: ReportPartReason, cause: Outcome) -> Self {
        Self {
            why,
            cause: Some(cause),
        }
    }

    /// A material of `actual` bytes, larger than the `limit` it was read with:
    /// `material_too_large`.
    pub fn too_large(actual: u64, limit: u64) -> Self {
        Self::new(
            ReportPartReason::TooLarge,
            Outcome::new(ActingctlCode::MaterialTooLarge)
                .actual_bytes(actual)
                .limit_bytes(limit),
        )
    }
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
    ) -> Result<Listed, Missed>;
    /// The rows of the `errors` view of `instance` in `window`: the newest `keep`.
    fn errors(&self, instance: &Instance, window: Window, keep: usize) -> Result<Errors, Missed>;
    /// The run `request_id` submitted, in full (`run show`).
    fn run(&self, request_id: &str) -> Result<Value, Missed>;
    /// The run's evidence, every step (`run evidence`).
    fn evidence(&self, request_id: &str) -> Result<Evidence, Missed>;
    /// The run's geometry observations, oldest first.
    fn geometry(&self, request_id: &str) -> Result<Vec<Geometry>, Missed>;
    /// The run's recognition records from the ledger sequence `from` (through `through`),
    /// oldest first.
    fn recognitions(
        &self,
        request_id: &str,
        from: u64,
        through: Option<u64>,
    ) -> Result<Vec<Recognition>, Missed>;
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

    /// When the next read stops; none starts once the deadline has passed (`budget_spent`,
    /// caused by `call_budget_exhausted`).
    fn step(&self) -> Result<Instant, Missed> {
        let now = Instant::now();
        if now >= self.deadline {
            return Err(Missed::new(
                ReportPartReason::BudgetSpent,
                Outcome::new(ActingctlCode::CallBudgetExhausted),
            ));
        }
        Ok(self.deadline.min(now + READ_STEP))
    }

    /// Why a read stopped before its end: the report's deadline once it has passed, else the
    /// read's own limit or the Runtime's read budget.
    fn cut(&self) -> ReportPartReason {
        if Instant::now() >= self.deadline {
            ReportPartReason::BudgetSpent
        } else {
            ReportPartReason::NotReadToEnd
        }
    }

    /// A read the client's `error` ended: the Runtime's refusal when the Runtime answered with
    /// its error; the client's time limit a read stopped before its end ([`Self::cut`]); anything
    /// else a failed read. Its outcome is the cause.
    fn failed(&self, error: &RuntimeClientError) -> Missed {
        let why = if error.projection().is_some() || error.receipt_outcome().is_some() {
            ReportPartReason::RuntimeRefused
        } else if error.code() == ClientCode::RuntimeEventQueryTimeLimitExceeded.code() {
            self.cut()
        } else {
            ReportPartReason::ReadFailed
        };
        Missed::new(why, error.outcome())
    }

    /// A material read that answered nothing (as `material`'s row answers it): a reference that
    /// is none is unreadable, the Runtime's answer without the material its refusal, the client's
    /// failure as [`Self::failed`] says.
    fn material_missed(&self, failure: material::Failure) -> Missed {
        match failure {
            material::Failure::Usage(outcome) | material::Failure::NotText(outcome) => {
                Missed::new(ReportPartReason::Unreadable, outcome)
            }
            material::Failure::Unavailable(outcome) => {
                Missed::new(ReportPartReason::RuntimeRefused, outcome)
            }
            material::Failure::OutOfRange {
                argument,
                minimum,
                maximum,
            } => Missed::new(
                ReportPartReason::ReadFailed,
                cli_line::usage_invalid(Some(argument), ArgumentProblem::OutOfRange)
                    .range_min(minimum)
                    .range_max(maximum),
            ),
            material::Failure::Runtime(error) => self.failed(&error),
            material::Failure::RuntimeNoted { error, reached } => {
                let missed = self.failed(&error);
                Missed::new(missed.why, error.outcome().caused_by(reached))
            }
        }
    }
}

/// A failed read whose outcome is `cause`, for `why`.
fn missed_for(why: ReportPartReason) -> impl Fn(Outcome) -> Missed {
    move |cause| Missed::new(why, cause)
}

/// The query of the errors view of `instance` in `window`: the view `errors`, the instance, the
/// window's time bound.
fn errors_query(instance: &Instance, window: Window) -> Result<EventQuery, Missed> {
    Ok(EventQuery {
        view: Some(LedgerView::Errors),
        instance_id: Some(id_of(instance).map_err(missed_for(ReportPartReason::ReadFailed))?),
        from_timestamp_unix_ms: Some(window.since_unix_ms),
        to_timestamp_unix_ms: window.until_unix_ms,
        ..EventQuery::default()
    })
}

/// The errors view of `instance` in `window`: its newest `keep` rows, found through `pages` by
/// the search for the newest records `events` has (fix round 1, item 7), every read of it
/// carrying the instance but the ledger head's, which selects no row (fix round 2, item 3).
/// Fix round 2, item 4: `late` says, once the time the read is held to is spent, the word for
/// that (`not_read_to_end` at the step's own limit, `budget_spent` at the report's deadline);
/// the search then starts no other read, and the part is missing with that word. A read the
/// Runtime cut short stops the search with `not_read_to_end`, the search's largest window with
/// `search_windows_spent` (fix round 2, item 5). A failure of the Runtime is what `failed` says.
pub(crate) fn errors_view(
    pages: &dyn EventPages,
    instance: &Instance,
    window: Window,
    keep: usize,
    late: &dyn Fn() -> Option<ReportPartReason>,
    failed: &dyn Fn(&RuntimeClientError) -> Missed,
) -> Result<Errors, Missed> {
    let query = errors_query(instance, window)?;
    let limit = u16::try_from(keep).unwrap_or(u16::MAX);
    // The word of the moment the time was found spent.
    let word = Cell::new(None);
    let spent = || {
        if word.get().is_none() {
            word.set(late());
        }
        word.get().is_some()
    };
    let found =
        events::newest_in_window(pages, &query, limit, &spent).map_err(|error| match &error {
            EventsError::Runtime(failure) => failed(failure),
            _ => Missed::new(ReportPartReason::ReadFailed, error.outcome()),
        })?;
    if found.late {
        // The search answers `late` only after `spent` said so, which kept the word.
        let why = word.get().unwrap_or(ReportPartReason::NotReadToEnd);
        return Err(Missed { why, cause: None });
    }
    let stopped = if found.cut {
        Some(ReportPartReason::NotReadToEnd)
    } else if found.searched_from.is_some() {
        Some(ReportPartReason::SearchWindowsSpent)
    } else {
        None
    };
    let envelope = |outcome: &Outcome| cli_line::outcome_value(outcome, Detail::Full);
    Ok(Errors {
        rows: found
            .events
            .iter()
            .map(|event| {
                let mut row = events::row(event, RowDetail::Summary, &envelope);
                row["module"] = json!(event.origin.module());
                row
            })
            .collect(),
        searched_from: found.searched_from,
        stopped,
    })
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
    ) -> Result<Listed, Missed> {
        let filter = RunListFilter {
            instance_id: Some(id_of(instance).map_err(missed_for(ReportPartReason::ReadFailed))?),
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
            .map_err(|error| self.failed(&error))?;
        Ok(Listed {
            runs: page
                .runs
                .iter()
                .map(|run| row_of(run, &instance.alias))
                .collect(),
            cut: page.incomplete.then(|| self.cut()),
        })
    }

    /// Fix round 2, item 4: the search is held to the step's time and the report's deadline,
    /// whichever comes first ([`errors_view`]): no read of it starts after that, and a search cut
    /// there is the part missing with the word [`Self::cut`] gives.
    fn errors(&self, instance: &Instance, window: Window, keep: usize) -> Result<Errors, Missed> {
        let until = self.step()?;
        let late = || (Instant::now() >= until).then(|| self.cut());
        errors_view(self.client, instance, window, keep, &late, &|failure| {
            self.failed(failure)
        })
    }

    fn run(&self, request_id: &str) -> Result<Value, Missed> {
        self.step()?;
        let key = run_show::by_request_id(request_id).ok_or_else(|| {
            Missed::new(
                ReportPartReason::Unreadable,
                Outcome::new(ActingctlCode::RunUnknown).request_id(request_id),
            )
        })?;
        run_show::read(self.client, key)
            .map(|status| run_show::answer(&status))
            .map_err(|error| self.failed(&error))
    }

    fn evidence(&self, request_id: &str) -> Result<Evidence, Missed> {
        let deadline = self.step()?;
        let run = run_show::by_request_id(request_id).ok_or_else(|| {
            Missed::new(
                ReportPartReason::Unreadable,
                Outcome::new(ActingctlCode::RunUnknown).request_id(request_id),
            )
        })?;
        let request = EvidenceRequest {
            run,
            from_step: 0,
            limit: u64::MAX,
            detail: StepDetail::Full,
        };
        // Fix round 2, item 7: a run the Runtime does not find is its refusal.
        run_evidence::read(self.client, &request, deadline).map_err(|failure| match &failure {
            run_evidence::Failure::Runtime(error) => self.failed(error),
            run_evidence::Failure::RunUnknown { .. } => {
                Missed::new(ReportPartReason::RuntimeRefused, failure.outcome())
            }
            run_evidence::Failure::Output(_) => {
                Missed::new(ReportPartReason::Unreadable, failure.outcome())
            }
            run_evidence::Failure::Argument { .. } => {
                Missed::new(ReportPartReason::ReadFailed, failure.outcome())
            }
        })
    }

    fn geometry(&self, request_id: &str) -> Result<Vec<Geometry>, Missed> {
        let query = EventQuery {
            request_id: Some(
                request_of(request_id).map_err(missed_for(ReportPartReason::Unreadable))?,
            ),
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
            .map_err(|error| self.failed(&error))?;
        Ok(found)
    }

    fn recognitions(
        &self,
        request_id: &str,
        from: u64,
        through: Option<u64>,
    ) -> Result<Vec<Recognition>, Missed> {
        let query = EventQuery {
            request_id: Some(
                request_of(request_id).map_err(missed_for(ReportPartReason::Unreadable))?,
            ),
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
            .map_err(|error| self.failed(&error))?;
        Ok(found)
    }

    fn material(&self, reference: &str, limit: u64) -> Result<Vec<u8>, Missed> {
        let deadline = self.step()?;
        let (position, artifact_id) =
            material::parse_ref(reference).map_err(|failure| self.material_missed(failure))?;
        let found = material::resolve(self.client, position, artifact_id)
            .map_err(|failure| self.material_missed(failure))?;
        if found.artifact.byte_count > limit {
            return Err(Missed::too_large(found.artifact.byte_count, limit));
        }
        material::read_whole(self.client, &found, deadline, &|| true)
            .map_err(|failure| self.material_missed(failure))
    }
}
