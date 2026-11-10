// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #428 S168-4 (`contracts/resource-readings.md`, "List readings"; register §1.291): a
//! `read_list` step reads a list a page shows, across as many viewports as it takes, and the host
//! writes it as one instance fact of value `record_list`, whole or not at all.
//!
//! - **The effect.** `read_list` is the fifth effect of a task schema `0.9` page-graph operation,
//!   beside `click`, `application`, `select` and `wait`. Its `to` is its `from`; it declares its
//!   `guard`; it is never retried; and it runs at most once per run, as an operation capped at one
//!   dispatch with `on_max_runs: next_candidate` (the static walk takes it as an edge from its
//!   page to the same page and follows the page's next operation).
//! - **Admission.** The operation's members first ([`TaskProgram::validate_list_reading_members`],
//!   before the common operation checks), then its references and its scan's static checks
//!   ([`prepare_list_readings`]); every refusal is `contained_task_resource_reading_invalid` with
//!   the detail `<operation id>:<reason>`, the reason the refused member below the operation. A
//!   `linear_steps` task refuses the step with `contained_task_linear_invalid`, `reason=read_list`.
//! - **The read.** The scan session of a select over a scan, its steps 1 and 2, with no policy:
//!   each position's first complete reading (its identity and every declared column's feature
//!   Known) is one row. A position read but never fixed, two positions of one key, or a 257th
//!   position is a miss: the warning `contained_task_resource_reading_unresolved` with its
//!   `reading_problem` and `count`, and no list. A failure of the scan itself ends the run with its
//!   code at the stage `read_list`.
//! - **The traces.** When the end check passes the step records [`ContainedTaskTrace::
//!   ListReadingFrame`] for the frame captured last; the step then records one
//!   [`ContainedTaskTrace::ListReading`], the list whole or its miss, which the host publishes at
//!   the run's successful end or writes as the warning row at once.

use super::scan::{RuntimeScanSource, ScanDeclaration, ScanEnd, ScanSession, ScanSource};
use super::{
    ContainedTaskError, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTimingContext,
    ContainedTaskTrace, PageObservation, PreparedContainedTask, ResourceReadingProblem,
    TaskControl, TaskOperation, TaskProgram, budget_observation_now, step_point_draw,
};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CandidateFeature, CandidateLayoutKind, FactScalar, MAX_FACT_TTL_MS, MAX_LIST_READING_COLUMNS,
    MAX_LIST_READINGS, MIN_FACT_TTL_MS, TaskSchema, list_reading_column_valid,
    list_reading_fact_key_valid,
};
use actingcommand_recognition_pack::{
    CandidateFeatureValue, RecognitionEvaluator, UnknownIdentityHandling,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// The operation member that declares a list reading. It shares its spelling with the stage
/// `read_list` the step's scan failures report, so it is spelled once, by the stage.
pub(super) const READ_LIST_MEMBER: &str = ExecutionKernelLocation::ReadList.as_str();

const READING_INVALID: ExecutionKernelCode =
    ExecutionKernelCode::ContainedTaskResourceReadingInvalid;

/// A `read_list` operation's effect, as the package writes it (`contracts/resource-readings.md`,
/// "List readings", "Declaration").
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskReadList {
    layout_id: String,
    key_feature: String,
    key_column: String,
    columns: Vec<TaskListColumn>,
    fact_key: String,
    valid_for_ms: u64,
    scan: ScanDeclaration,
}

/// One declared column: the row member `column` holds the fixed reading of the layout's
/// feature `feature`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskListColumn {
    column: String,
    feature: String,
}

/// An admitted list reading: its layout, its key and columns, the key it writes, how long the
/// list stays valid and its scan. Set by admission and never read from the package.
#[derive(Debug)]
pub(super) struct PreparedListReading {
    layout_id: String,
    key_column: String,
    /// `(column, feature)` in declaration order.
    columns: Vec<(String, String)>,
    fact_key: String,
    valid_for_ms: u64,
    scan: ScanDeclaration,
}

impl PreparedListReading {
    /// The step's own time limit, its scan's `max_ms`, which replaces the step timeout.
    pub(super) const fn max_ms(&self) -> u64 {
        self.scan.max_ms()
    }
}

/// One list reading's declaration, as the host publishes it: the operation that read it, the
/// key it writes and how long it stays valid. In memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListReadingDeclaration {
    /// The operation's id: the warning's `reading_id`, part of the record's `source_detector`
    /// and `source_snapshot_id`.
    pub operation_id: String,
    pub fact_key: String,
    pub valid_for_ms: u64,
}

/// A list read whole: one row per position, `{<key_column>: <business ID>, <column>: <value>,
/// …}`, in ascending byte order of the key, and the lowest confidence of the features the rows
/// read that carry one (1000 for an empty list). In memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListReadingRows {
    pub rows: Vec<BTreeMap<String, FactScalar>>,
    pub confidence_milli: u16,
}

/// A list that gave no reading: why, and `count` (the unread positions, the repeated keys, or
/// the position that exceeded the list). In memory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListReadingMiss {
    pub problem: ResourceReadingProblem,
    pub count: u32,
}

impl ListReadingMiss {
    /// The warning of this miss: `contained_task_resource_reading_unresolved` with the
    /// operation's id as `reading_id`, the list's `fact_key`, the `reading_problem` and `count`.
    pub fn outcome(&self, declaration: &ListReadingDeclaration) -> Outcome {
        Outcome::new(ExecutionKernelCode::ContainedTaskResourceReadingUnresolved)
            .reading_id(declaration.operation_id.as_str())
            .fact_key(declaration.fact_key.as_str())
            .reading_problem(self.problem)
            .count(self.count)
    }
}

/// What one `read_list` step read, before its trace: the list whole, or its miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ListRead {
    Whole(ListReadingRows),
    Missed(ListReadingMiss),
}

/// `contained_task_resource_reading_invalid` of the operation `operation_id` with `reason`.
fn refusal(operation_id: &str, reason: &str) -> ContainedTaskError {
    ContainedTaskError::with_detail(READING_INVALID.code(), format!("{operation_id}:{reason}"))
}

impl TaskOperation {
    /// Workflow #428 S168-4: whether the operation reads a list.
    pub(super) const fn declares_read_list(&self) -> bool {
        self.read_list.is_some()
    }

    /// The first member, as its JSON pointer below the operation, that a `read_list` step never
    /// takes (`contracts/resource-readings.md`, "Members of a `read_list` step"): another effect,
    /// a retry, a recovery, a trusted coordinate, a confirmation after its moves, a cap of its
    /// own, or a `linear_steps` member.
    fn refused_read_list_member(&self) -> Option<&'static str> {
        [
            ("click", self.click.is_some()),
            ("application", self.application.is_some()),
            ("select", self.select.is_some()),
            ("wait", self.wait.is_some()),
            ("max_attempts", self.max_attempts.is_some()),
            ("retry_interval_ms", self.retry_interval_ms.is_some()),
            ("on_error", self.on_error.is_some()),
            (
                "unguarded_trusted_coordinate",
                self.unguarded_trusted_coordinate,
            ),
            ("expect_after", self.expect_after.is_some()),
            ("post_delay_ms", self.post_delay_ms.is_some()),
            ("input_recheck", self.input_recheck.is_some()),
            ("observation_window", self.observation_window.is_some()),
            ("on_guard_refused", self.on_guard_refused.is_some()),
            ("max_runs", self.max_runs.is_some()),
            ("on_max_runs", self.on_max_runs.is_some()),
            ("transition", self.transition.is_some()),
            ("optional", self.optional.is_some()),
        ]
        .into_iter()
        .find_map(|(member, declared)| declared.then_some(member))
    }
}

impl TaskProgram {
    /// Whether the task declares a `read_list` step.
    pub(super) fn declares_list_readings(&self) -> bool {
        self.operations
            .iter()
            .any(TaskOperation::declares_read_list)
    }

    /// The page-graph admission of every `read_list` step's members, before the common
    /// operation checks: task schema `0.9`, a task that declares `scheduling_outcome`, at most
    /// 16 such steps, `to` its `from` page, a declared guard, no retry and none of the members
    /// [`TaskOperation::refused_read_list_member`] names. Each refusal is
    /// `contained_task_resource_reading_invalid`, `<operation id>:<reason>`.
    pub(super) fn validate_list_reading_members(&self) -> Result<(), ContainedTaskError> {
        let mut count = 0_usize;
        for operation in self
            .operations
            .iter()
            .filter(|operation| operation.declares_read_list())
        {
            let refuse = |reason: &str| Err(refusal(&operation.id, reason));
            count += 1;
            if !self.schema_at_least(TaskSchema::V0_9) {
                return refuse("schema_version");
            }
            if self.scheduling_outcome.is_none() {
                return refuse("scheduling_outcome");
            }
            if count > MAX_LIST_READINGS {
                return refuse("operations");
            }
            if let Some(member) = operation.refused_read_list_member() {
                return refuse(member);
            }
            if operation.retryable == Some(true) {
                return refuse("retryable");
            }
            if operation.guard.is_none() {
                return refuse("guard");
            }
            let to_from = operation
                .to
                .as_ref()
                .and_then(|to| to.normalized().ok())
                .is_some_and(|pages| pages == [operation.from.clone()]);
            if !to_from || operation.from == super::linear::ANY_PAGE {
                return refuse("to");
            }
        }
        Ok(())
    }
}

/// Contained task admission of every `read_list` step's references, after the program and the
/// page set are validated (`contracts/resource-readings.md`, "List readings", "Declaration"): its
/// layout is a `repeated_anchor` layout of the recognition pack on the step's `from` page whose
/// `unknown_identity` is `reject`; its key feature is an identity feature of the layout and its
/// scan's `identity_feature`; its key column and columns are tokens, distinct, at most 63, each
/// column's feature another feature of the layout, read once, of value `passed`, `ocr_integer` or
/// `identity`; its `fact_key` lies under `inventory.list.`, is a valid instance fact key and is
/// unique among every key the task writes; `valid_for_ms` is `1..=31_536_000_000`; and its scan
/// declares no `early_stop` and passes a select step's static checks. Each refusal is
/// `contained_task_resource_reading_invalid`, `<operation id>:<reason>`.
pub(super) fn prepare_list_readings(
    program: &mut TaskProgram,
    control: &TaskControl,
    evaluator: &RecognitionEvaluator,
) -> Result<(), ContainedTaskError> {
    let mut written = program
        .resource_readings
        .iter()
        .flatten()
        .flat_map(|reading| reading.fact_keys())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    for operation in &mut program.operations {
        let Some(declared) = &operation.read_list else {
            continue;
        };
        let id = operation.id.clone();
        let refuse = |reason: &str| refusal(&id, reason);
        let read: TaskReadList = serde_json::from_value(declared.clone())
            .map_err(|_| refuse(&format!("{READ_LIST_MEMBER}/declaration")))?;
        let member = |name: &str| format!("{READ_LIST_MEMBER}/{name}");
        let layout = evaluator
            .candidate_layout(&read.layout_id)
            .filter(|layout| {
                crate::canonical_page_anchor(&control.game, &layout.page_id)
                    == crate::canonical_page_anchor(&control.game, &operation.from)
                    && layout.kind == CandidateLayoutKind::RepeatedAnchor
                    && layout.unknown_identity == UnknownIdentityHandling::Reject
            })
            .ok_or_else(|| refuse(&member("layout_id")))?;
        let feature = |name: &str| layout.features.iter().find(|feature| feature.name == name);
        if feature(&read.key_feature).is_none_or(|feature| feature.identity.is_none())
            || read.key_feature != read.scan.identity_feature()
        {
            return Err(refuse(&member("key_feature")));
        }
        if !list_reading_column_valid(&read.key_column) {
            return Err(refuse(&member("key_column")));
        }
        if read.columns.len() > MAX_LIST_READING_COLUMNS {
            return Err(refuse(&member("columns")));
        }
        let mut column_names = BTreeSet::from([read.key_column.as_str()]);
        let mut features = BTreeSet::from([read.key_feature.as_str()]);
        for (index, column) in read.columns.iter().enumerate() {
            if !list_reading_column_valid(&column.column)
                || !column_names.insert(column.column.as_str())
            {
                return Err(refuse(&member(&format!("columns/{index}/column"))));
            }
            let readable = feature(&column.feature).is_some_and(|feature| {
                matches!(
                    feature.value,
                    CandidateFeatureValue::Passed
                        | CandidateFeatureValue::OcrInteger
                        | CandidateFeatureValue::Identity
                )
            });
            if !readable || !features.insert(column.feature.as_str()) {
                return Err(refuse(&member(&format!("columns/{index}/feature"))));
            }
        }
        if !list_reading_fact_key_valid(&read.fact_key) || !written.insert(read.fact_key.clone()) {
            return Err(refuse(&member("fact_key")));
        }
        if !(MIN_FACT_TTL_MS..=MAX_FACT_TTL_MS).contains(&read.valid_for_ms) {
            return Err(refuse(&member("valid_for_ms")));
        }
        if read.scan.declares_early_stop() {
            return Err(refuse(&member("scan/early_stop")));
        }
        if let Some(detail) = read.scan.admission_refusal(
            layout,
            None,
            evaluator,
            (control.resolution.width, control.resolution.height),
            control.task_timeout().milliseconds,
        ) {
            return Err(refuse(&format!("{} {detail}", member("scan"))));
        }
        operation.prepared_list = Some(Box::new(PreparedListReading {
            layout_id: read.layout_id,
            key_column: read.key_column,
            columns: read
                .columns
                .into_iter()
                .map(|column| (column.column, column.feature))
                .collect(),
            fact_key: read.fact_key,
            valid_for_ms: read.valid_for_ms,
            scan: read.scan,
        }));
    }
    Ok(())
}

/// The confidence of one fixed feature, when it carries one, as milli in 0..=1000.
fn feature_confidence(feature: &CandidateFeature) -> Option<u16> {
    let confidence = match feature {
        CandidateFeature::Integer { confidence, .. }
        | CandidateFeature::Boolean { confidence, .. }
        | CandidateFeature::Identity { confidence, .. } => *confidence,
        CandidateFeature::Unknown { .. } => None,
    }?;
    u16::try_from(confidence.clamp(0, 1000)).ok()
}

/// One fixed feature as a row value: a `passed` feature a boolean, an `ocr_integer` feature an
/// integer, an identity its business ID.
fn row_value(feature: &CandidateFeature) -> Option<FactScalar> {
    match feature {
        CandidateFeature::Boolean { value, .. } => Some(FactScalar::Boolean(*value)),
        CandidateFeature::Integer { value, .. } => Some(FactScalar::Integer(*value)),
        CandidateFeature::Identity { value, .. } => Some(FactScalar::String(value.clone())),
        CandidateFeature::Unknown { .. } => None,
    }
}

impl ScanSession<'_> {
    /// One list reading over this session (`contracts/resource-readings.md`, "List readings",
    /// "Reading"): the scan's steps 1 and 2 with no policy, then the rows. A failure of the scan
    /// itself returns as the error; a list that cannot be read whole is its miss.
    pub(super) fn read_list<S: ScanSource>(
        &mut self,
        source: &mut S,
        key_column: &str,
        columns: &[(String, String)],
    ) -> Result<ListRead, ContainedTaskRunError<S::Error>> {
        match self.run(source)? {
            ScanEnd::Covered => Ok(list_rows_of(
                self.read_positions(),
                self.identity_feature(),
                key_column,
                columns,
            )?),
            ScanEnd::RowsExceeded => Ok(ListRead::Missed(ListReadingMiss {
                problem: ResourceReadingProblem::ListRowsExceeded,
                count: u32::try_from(actingcommand_contract::MAX_LIST_READING_ROWS + 1)
                    .unwrap_or(u32::MAX),
            })),
            // A list reading evaluates nothing, so it neither aborts nor stops early.
            ScanEnd::Aborted { .. } | ScanEnd::EarlyStopped => {
                Err(ContainedTaskError::with_detail(
                    SharedCode::ContainedTaskStateInvalid.code(),
                    "list reading ended without its end check",
                )
                .into())
            }
        }
    }
}

/// Builds the rows of a list whose coverage is complete from its positions' fixed readings, in
/// first-seen order: a position read but never fixed is `list_position_unread`, two positions of
/// one key `list_duplicate_key`; otherwise the rows in ascending byte order of their key.
pub(super) fn list_rows_of<'p>(
    positions: impl Iterator<Item = Option<&'p BTreeMap<String, CandidateFeature>>>,
    key_feature: &str,
    key_column: &str,
    columns: &[(String, String)],
) -> Result<ListRead, ContainedTaskError> {
    let invariant = || {
        ContainedTaskError::with_detail(
            SharedCode::ContainedTaskStateInvalid.code(),
            "list reading: a fixed position lacks a declared feature",
        )
    };
    let mut unread = 0_u32;
    let mut rows: BTreeMap<String, BTreeMap<String, FactScalar>> = BTreeMap::new();
    let mut repeated = BTreeSet::new();
    let mut confidence: Option<u16> = None;
    for features in positions {
        let Some(features) = features else {
            unread = unread.saturating_add(1);
            continue;
        };
        let key_reading = features.get(key_feature).ok_or_else(invariant)?;
        let Some(FactScalar::String(key)) = row_value(key_reading) else {
            return Err(invariant());
        };
        let mut row = BTreeMap::from([(key_column.to_owned(), FactScalar::String(key.clone()))]);
        let mut lowest = feature_confidence(key_reading);
        for (column, feature) in columns {
            let reading = features.get(feature).ok_or_else(invariant)?;
            row.insert(column.clone(), row_value(reading).ok_or_else(invariant)?);
            lowest = match (lowest, feature_confidence(reading)) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, right) => left.or(right),
            };
        }
        confidence = match (confidence, lowest) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        if rows.insert(key.clone(), row).is_some() {
            repeated.insert(key);
        }
    }
    if unread > 0 {
        return Ok(ListRead::Missed(ListReadingMiss {
            problem: ResourceReadingProblem::ListPositionUnread,
            count: unread,
        }));
    }
    if !repeated.is_empty() {
        return Ok(ListRead::Missed(ListReadingMiss {
            problem: ResourceReadingProblem::ListDuplicateKey,
            count: u32::try_from(repeated.len()).unwrap_or(u32::MAX),
        }));
    }
    // An identity always carries a confidence; a list of rows without one cannot be published.
    let confidence_milli = match (rows.is_empty(), confidence) {
        (true, _) => 1000,
        (false, Some(confidence)) => confidence,
        (false, None) => {
            return Err(ContainedTaskError::with_detail(
                SharedCode::ContainedTaskStateInvalid.code(),
                "list reading: no row feature carries a confidence",
            ));
        }
    };
    Ok(ListRead::Whole(ListReadingRows {
        rows: rows.into_values().collect(),
        confidence_milli,
    }))
}

/// A failure of a list reading's scan, at the stage `read_list` (`contracts/resource-readings.md`,
/// "List readings", "Reading"); the runtime's own failures are unchanged.
fn at_read_list<E>(error: ContainedTaskRunError<E>) -> ContainedTaskRunError<E> {
    match error {
        ContainedTaskRunError::Task(error) => {
            let outcome = error.outcome().stage(ExecutionKernelLocation::ReadList);
            ContainedTaskRunError::Task(error.with_values(outcome))
        }
        other => other,
    }
}

impl PreparedContainedTask {
    /// Workflow #428 S168-4: whether the task declares a `read_list` step; offline simulation
    /// reads no list.
    pub fn has_list_readings(&self) -> bool {
        self.program.declares_list_readings()
    }

    /// One `read_list` step, whose `StepStarted` (index `step_index`) is written, from its
    /// dispatched frame `first` within `timing`'s deadline (its scan's `max_ms`, capped by the
    /// task's). The list, whole or its miss, is recorded as one [`ContainedTaskTrace::
    /// ListReading`]; the step's last viewport is returned, on which the step finishes. A failure
    /// of the scan ends the run with its code at the stage `read_list`, publishing nothing.
    pub(super) fn read_list_step<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        prepared: &PreparedListReading,
        first: &PageObservation,
        (step_index, timing): (u32, ContainedTaskTimingContext),
    ) -> Result<PageObservation, ContainedTaskRunError<R::Error>> {
        let layout = self
            .evaluator
            .candidate_layout(&prepared.layout_id)
            .ok_or_else(|| refusal(&operation.id, &format!("{READ_LIST_MEMBER}/layout_id")))?;
        let deadline = timing.deadline();
        let remaining =
            deadline.saturating_duration_since(budget_observation_now(runtime, timing.pace()));
        let mut session = ScanSession::for_list(
            &prepared.scan,
            layout,
            prepared
                .columns
                .iter()
                .map(|(_, feature)| feature.clone())
                .collect(),
            (first.scene.width(), first.scene.height()),
            remaining,
        )
        .map_err(|error| at_read_list(error.into()))?;
        let draw = step_point_draw(runtime, step_index, &operation.id)?;
        let mut source = RuntimeScanSource::new(
            self,
            runtime,
            operation,
            &prepared.scan,
            layout,
            first,
            (step_index, timing, deadline, draw),
            false,
        )
        .reading_list();
        let read = session.read_list(&mut source, &prepared.key_column, &prepared.columns);
        let end_captured_at = source.end_captured_at();
        let (latest, _) = source.into_parts();
        let read = read.map_err(at_read_list)?;
        let last = latest.ok_or_else(|| {
            ContainedTaskError::with_detail(
                SharedCode::ContainedTaskStateInvalid.code(),
                "list reading captured no viewport",
            )
        })?;
        let declaration = ListReadingDeclaration {
            operation_id: operation.id.clone(),
            fact_key: prepared.fact_key.clone(),
            valid_for_ms: prepared.valid_for_ms,
        };
        let (captured_at, list): (SystemTime, Result<ListReadingRows, ListReadingMiss>) = match read
        {
            ListRead::Whole(rows) => (
                end_captured_at.ok_or_else(|| {
                    ContainedTaskError::with_detail(
                        SharedCode::ContainedTaskStateInvalid.code(),
                        "list reading whole without an end frame",
                    )
                })?,
                Ok(rows),
            ),
            ListRead::Missed(miss) => (last.captured_at, Err(miss)),
        };
        runtime
            .record(ContainedTaskTrace::ListReading {
                declaration,
                captured_at,
                list,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        Ok(last)
    }
}
