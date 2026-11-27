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
//!   the detail `<operation id>:<reason>`, the reason the refused member below the operation.
//!   Workflow #428 S168-LK: a `linear_steps` task admits the step too, its members checked the
//!   same way first (`TaskProgram::validate_linear`), and a `key` beside `read_list` is another
//!   effect beside it (`read_list`), in both modes.
//! - **The read.** The scan session of a select over a scan, its steps 1 and 2, with no policy:
//!   each position's first complete reading (its identity and every declared column's feature
//!   Known) is one row. A position read but never fixed, two positions of one key, or a position
//!   past `MAX_LIST_READING_ROWS` (the 1025th since S168-LB1a, 0.12.55) is a miss: the warning
//!   `contained_task_resource_reading_unresolved` with its `reading_problem` and `count`, and no
//!   list. A failure of the scan itself ends the run with its code at the stage `read_list`.
//!   S168-LB1a: a list read whole whose record is larger than one observation holds is the miss
//!   `list_bytes_exceeded`, which the host finds when it takes the list (it alone knows the
//!   record the run's end would publish).
//! - **The traces.** When the end check passes the step records [`ContainedTaskTrace::
//!   ListReadingFrame`] for the frame captured last; the step then records one
//!   [`ContainedTaskTrace::ListReading`], the list whole or its miss, which the host publishes at
//!   the run's successful end or writes as the warning row at once.
//!
//! Workflow #428 S168-LK (`contracts/linear-steps.md`, "List reading steps"; register §1.500): the
//! scan and its list are one function (`PreparedContainedTask::scan_list`), which the run's step
//! of either mode calls and records, and which the dry run over saved viewports
//! (`dry_run_list_reading`) calls and answers ([`ListReadingDryRun`]).

use super::scan::{
    ListPosition, RuntimeScanSource, ScanDeclaration, ScanDryRunUnknown, ScanDryRunViewport,
    ScanEnd, ScanSession, ScanSource,
};
use super::{
    ContainedTaskError, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTimingContext,
    ContainedTaskTrace, PageObservation, PreparedContainedTask, ResourceReadingProblem,
    TaskControl, TaskOperation, TaskProgram, budget_observation_now, invariant_detailed,
    step_point_draw,
};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
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
    /// Workflow #503 LIST-A (`package` revision 27): what the list does with a card that is none
    /// of the keys its identity declares; absent (or `null`), every card is read as before.
    #[serde(default)]
    others: Option<TaskListOthers>,
}

/// Workflow #503 LIST-A: the closed vocabulary of `read_list.others`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskListOthers {
    /// A position the identity mapping marks none of the wanted keys on every read makes no row
    /// and does not make the list unread.
    PassOver,
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
    /// Workflow #503 LIST-A: the reading declares `others: pass_over`.
    pass_over: bool,
}

impl PreparedListReading {
    /// The step's own time limit, its scan's `max_ms`, which replaces the step timeout.
    pub(super) const fn max_ms(&self) -> u64 {
        self.scan.max_ms()
    }

    /// Workflow #448 SBUD-1: the layout the step projects on each viewport, whose candidate
    /// provider need admission compares with the step's limit.
    pub(super) fn layout_id(&self) -> &str {
        &self.layout_id
    }

    /// Workflow #450 FL-1b: the key the step writes, which a field list's key must differ from.
    pub(super) fn fact_key(&self) -> &str {
        &self.fact_key
    }

    /// Workflow #428 S168-LK: the list's declaration as the step `operation_id` records it with
    /// its list, and as the dry run answers it. Workflow #503 LIST-A: with the positions its scan
    /// passed over, `passed_over` (`None` for a reading that declares no `others`).
    pub(super) fn declaration(
        &self,
        operation_id: &str,
        passed_over: Option<u32>,
    ) -> ListReadingDeclaration {
        ListReadingDeclaration {
            operation_id: operation_id.to_owned(),
            fact_key: self.fact_key.clone(),
            valid_for_ms: self.valid_for_ms,
            frame_indices: Vec::new(),
            passed_over,
        }
    }
}

/// One list reading's declaration, as the host publishes it: the operation that read it, the
/// key it writes and how long it stays valid. In memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListReadingDeclaration {
    /// The operation's id: the warning's `reading_id`, part of the record's `source_detector`
    /// and `source_snapshot_id`. Workflow #450 FL-1b: a field list's id.
    pub operation_id: String,
    pub fact_key: String,
    pub valid_for_ms: u64,
    /// Workflow #450 FL-1b: for a field list's miss, the fields report's frame indices it names
    /// (the warning's `frame_indices`): the unresolved frames, the frames that hold a repeated
    /// key, or the first or the last frame. Empty for a `read_list` step and for a whole list.
    pub frame_indices: Vec<u32>,
    /// Workflow #503 LIST-A: for a `read_list` step that declares `others: pass_over`, how many
    /// positions its scan passed over, whole list or miss alike, which the host writes as the
    /// note [`Self::passed_over_outcome`]; `None` for every other list, a field list's included.
    pub passed_over: Option<u32>,
}

impl ListReadingDeclaration {
    /// Workflow #503 LIST-A (`contracts/resource-readings.md`, "List readings", "A list of the
    /// wanted keys only"): the note of a list that passes over, `contained_task_resource_reading_
    /// passed_over` (info) with the operation's id as `reading_id`, the list's `fact_key` and
    /// `count` the positions passed over (0 included); `None` for a list that declares no
    /// `others`. No card's text is in it.
    pub fn passed_over_outcome(&self) -> Option<Outcome> {
        self.passed_over.map(|count| {
            Outcome::new(ExecutionKernelCode::ContainedTaskResourceReadingPassedOver)
                .reading_id(self.operation_id.as_str())
                .fact_key(self.fact_key.as_str())
                .count(count)
        })
    }
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
    /// Workflow #450 FL-1b: and, for a field list's miss, the `frame_indices` its declaration
    /// carries (at most 64 kept, `frame_indices_total` past that).
    pub fn outcome(&self, declaration: &ListReadingDeclaration) -> Outcome {
        let outcome = Outcome::new(ExecutionKernelCode::ContainedTaskResourceReadingUnresolved)
            .reading_id(declaration.operation_id.as_str())
            .fact_key(declaration.fact_key.as_str())
            .reading_problem(self.problem)
            .count(self.count);
        if declaration.frame_indices.is_empty() {
            outcome
        } else {
            outcome.frame_indices(declaration.frame_indices.iter().copied())
        }
    }
}

/// What one `read_list` step read, before its trace: the list whole, or its miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ListRead {
    Whole(ListReadingRows),
    Missed(ListReadingMiss),
}

/// Workflow #428 S168-LK (`contracts/resource-readings.md`, "List readings", "Offline"): a
/// `read_list` step's scan dry-run over saved viewports (`dry_run_list_reading`), answering what
/// the step's run records: the list's declaration, the session's coverage viewports in order, each
/// with the end check it read, and the list whole or its miss, as the run's `ListReading` record
/// carries it. Beside it (round 2 of the slice), `unknown`: the positions the scan read but could
/// not read whole, in first-seen order, each as a select's dry run lists its unknown positions, so
/// that a self-check can name the place that was not read; empty for a list read whole, and for
/// `list_position_unread` the positions its count counts. In memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListReadingDryRun {
    pub declaration: ListReadingDeclaration,
    pub viewports: Vec<ScanDryRunViewport>,
    pub list: Result<ListReadingRows, ListReadingMiss>,
    pub unknown: Vec<ScanDryRunUnknown>,
}

impl ListReadingDryRun {
    /// The warning a run writes for the list's miss ([`ListReadingMiss::outcome`],
    /// `contained_task_resource_reading_unresolved`); `None` for a list read whole.
    pub fn warning(&self) -> Option<Outcome> {
        self.list
            .as_ref()
            .err()
            .map(|miss| miss.outcome(&self.declaration))
    }
}

/// Workflow #428 S168-LK: what one `read_list` step's scan read (`PreparedContainedTask::
/// scan_list`): the frame captured last, the device capture time the list is observed at (the
/// frame its end check passed on for a list read whole, the last frame for a miss), the list
/// whole or its miss, the session's coverage viewports and the positions it could not read.
pub(super) struct ScannedList {
    pub(super) last: PageObservation,
    pub(super) captured_at: SystemTime,
    pub(super) list: Result<ListReadingRows, ListReadingMiss>,
    pub(super) viewports: Vec<ScanDryRunViewport>,
    pub(super) unknown: Vec<ScanDryRunUnknown>,
    /// Workflow #503 LIST-A: the positions a list that passes over passed over; `None` for a
    /// reading that declares no `others`.
    pub(super) passed_over: Option<u32>,
}

/// `contained_task_resource_reading_invalid` of the operation `operation_id`, refused at `member`
/// (the refused member below the operation, or the task member a rule of the whole task names).
fn refusal(operation_id: &str, member: &str) -> ContainedTaskError {
    ContainedTaskError::with_detail(READING_INVALID.code(), format!("{operation_id}:{member}"))
}

/// Workflow #428 S168-LK: the dry run of a list reading asked of the operation `operation_id`,
/// which declares none: `contained_task_resource_reading_invalid`, `<operation id>:read_list`.
pub(super) fn not_a_list_reading(operation_id: &str) -> ContainedTaskError {
    refusal(operation_id, READ_LIST_MEMBER)
}

impl TaskOperation {
    /// Workflow #428 S168-4: whether the operation reads a list.
    pub(super) const fn declares_read_list(&self) -> bool {
        self.read_list.is_some()
    }

    /// The first member, as its JSON pointer below the operation, that a `read_list` step never
    /// takes (`contracts/resource-readings.md`, "Members of a `read_list` step"): another effect,
    /// a retry, a recovery, a trusted coordinate, a confirmation after its moves, a cap of its
    /// own, or a `linear_steps` member. Another effect is named `read_list`, the member the
    /// declaration gate refuses beside it. Workflow #428 S168-LK: a `key` is another effect.
    fn refused_read_list_member(&self) -> Option<&'static str> {
        [
            (
                READ_LIST_MEMBER,
                self.click.is_some()
                    || self.application.is_some()
                    || self.select.is_some()
                    || self.wait.is_some()
                    || self.key.is_some(),
            ),
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

    /// The admission of every `read_list` step's members, before the common operation checks,
    /// in a page graph and (Workflow #428 S168-LK) in a step plan, where it is the first check of
    /// `TaskProgram::validate_linear` after the task's own (`contracts/resource-readings.md`,
    /// "List readings", "Declaration"):
    /// task schema `0.9` (`read_list` refused before it), a task that declares
    /// `scheduling_outcome`, at most 16 such steps (`operations` for the 17th), none of the
    /// members [`TaskOperation::refused_read_list_member`] names, no retry, a declared guard and
    /// `to` its `from` page. Each refusal is `contained_task_resource_reading_invalid`,
    /// `<operation id>:<member>`, the member the declaration gate refuses at.
    pub(super) fn validate_list_reading_members(&self) -> Result<(), ContainedTaskError> {
        let mut count = 0_usize;
        for operation in self
            .operations
            .iter()
            .filter(|operation| operation.declares_read_list())
        {
            let refuse = |member: &str| Err(refusal(&operation.id, member));
            count += 1;
            if !self.schema_at_least(TaskSchema::V0_9) {
                return refuse(READ_LIST_MEMBER);
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
            // The step stays on its page: `to` names exactly its `from` page.
            let to_from = operation
                .to
                .as_ref()
                .and_then(|to| to.normalized().ok())
                .is_some_and(|pages| match pages.as_slice() {
                    [page] => {
                        crate::canonical_page_anchor(&self.game, page)
                            == crate::canonical_page_anchor(&self.game, &operation.from)
                    }
                    _ => false,
                });
            if !to_from {
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
/// declares no `early_stop`, declares `end` (Workflow #463 K-c) and passes a select step's static
/// checks. Each refusal is
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
        let read: TaskReadList =
            serde_json::from_value(declared.clone()).map_err(|_| refuse(READ_LIST_MEMBER))?;
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
        // Workflow #463 K-c: a select's scan may leave out `end` beside `early_stop`; a list
        // reading reads to its end, so its scan declares one (the declaration gate refuses it
        // first, `MissingField` at `read_list/scan/end`).
        if !read.scan.declares_end() {
            return Err(refuse(&member("scan/end")));
        }
        if let Some(detail) = read.scan.admission_refusal(
            layout,
            None,
            evaluator,
            (control.resolution.width, control.resolution.height),
            control.task_timeout().milliseconds,
        ) {
            // The detail of a select step's static check, below the step's `read_list`:
            // `read_list/scan <key>=<value>`.
            return Err(refuse(&format!("{READ_LIST_MEMBER}/{detail}")));
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
            pass_over: read.others == Some(TaskListOthers::PassOver),
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
            ScanEnd::Aborted { .. } | ScanEnd::EarlyStopped => Err(invariant_detailed(
                "list reading ended without its end check",
                |outcome| outcome,
            )
            .into()),
        }
    }
}

/// Builds the rows of a list whose coverage is complete from its positions' fixed readings, in
/// first-seen order: a position read but never fixed is `list_position_unread`, two positions of
/// one key `list_duplicate_key`; otherwise the rows in ascending byte order of their key.
/// Workflow #503 LIST-A: a position passed over makes no row and is not unread, so a list whose
/// every position was passed over is whole with no row.
pub(super) fn list_rows_of<'p>(
    positions: impl Iterator<Item = ListPosition<'p>>,
    key_feature: &str,
    key_column: &str,
    columns: &[(String, String)],
) -> Result<ListRead, ContainedTaskError> {
    let invariant = || {
        invariant_detailed(
            "list reading: a fixed position lacks a declared feature",
            |outcome| outcome,
        )
    };
    let mut unread = 0_u32;
    let mut rows: BTreeMap<String, BTreeMap<String, FactScalar>> = BTreeMap::new();
    let mut repeated = BTreeSet::new();
    let mut confidence: Option<u16> = None;
    for position in positions {
        let features = match position {
            ListPosition::Row(features) => features,
            ListPosition::Unread => {
                unread = unread.saturating_add(1);
                continue;
            }
            ListPosition::PassedOver => continue,
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
            return Err(invariant_detailed(
                "list reading: no row feature carries a confidence",
                |outcome| outcome,
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
pub(super) fn at_read_list<E>(error: ContainedTaskRunError<E>) -> ContainedTaskRunError<E> {
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
    /// reads no list. Workflow #450 FL-1b: or a field list.
    pub fn has_list_readings(&self) -> bool {
        self.program.declares_list_readings() || self.program.declares_field_lists()
    }

    /// One `read_list` step, whose `StepStarted` (index `step_index`) is written, from its
    /// dispatched frame `first` within `timing`'s deadline (its scan's `max_ms`, capped by the
    /// task's). The list, whole or its miss, is recorded as one [`ContainedTaskTrace::
    /// ListReading`]; the step's last viewport is returned, on which the step finishes. A failure
    /// of the scan ends the run with its code at the stage `read_list`, publishing nothing.
    /// Workflow #428 S168-LK: the step of a page graph and of a step plan alike; its scan and its
    /// list are [`Self::scan_list`]'s.
    pub(super) fn read_list_step<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        prepared: &PreparedListReading,
        first: &PageObservation,
        (step_index, timing): (u32, ContainedTaskTimingContext),
    ) -> Result<PageObservation, ContainedTaskRunError<R::Error>> {
        let scanned = self.scan_list(
            runtime,
            operation,
            prepared,
            first,
            (step_index, timing),
            false,
        )?;
        runtime
            .record(ContainedTaskTrace::ListReading {
                declaration: prepared.declaration(&operation.id, scanned.passed_over),
                captured_at: scanned.captured_at,
                list: scanned.list,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        Ok(scanned.last)
    }

    /// Workflow #428 S168-LK: the scan of one `read_list` step over `runtime` from its dispatched
    /// frame `first` within `timing`'s deadline, and the list it read: the one reading the run's
    /// step records ([`Self::read_list_step`]) and the dry run over saved viewports answers
    /// (`dry_run_list_reading`, whose `runtime` hands out the saved frames: `replay`, no wait
    /// between captures). The end check's frame is recorded as `ListReadingFrame` as it passes. A
    /// failure of the scan is the error, with the stage `read_list`.
    pub(super) fn scan_list<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        prepared: &PreparedListReading,
        first: &PageObservation,
        (step_index, timing): (u32, ContainedTaskTimingContext),
        replay: bool,
    ) -> Result<ScannedList, ContainedTaskRunError<R::Error>> {
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
        // Workflow #503 LIST-A: a reading that declares `others: pass_over` passes over what the
        // identity mapping marks none of the wanted keys.
        if prepared.pass_over {
            session.passing_over();
        }
        let draw = step_point_draw(runtime, step_index, &operation.id)?;
        let mut source = RuntimeScanSource::new(
            self,
            runtime,
            operation,
            &prepared.scan,
            layout,
            first,
            (step_index, timing, deadline, draw),
            replay,
        )
        .reading_list(prepared.pass_over);
        let read = session.read_list(&mut source, &prepared.key_column, &prepared.columns);
        let end_captured_at = source.end_captured_at();
        let (latest, _) = source.into_parts();
        let read = read.map_err(at_read_list)?;
        let last = latest.ok_or_else(|| {
            invariant_detailed("list reading captured no viewport", |outcome| {
                outcome.pack_operation_id(operation.id.as_str())
            })
        })?;
        let (captured_at, list): (SystemTime, Result<ListReadingRows, ListReadingMiss>) = match read
        {
            ListRead::Whole(rows) => (
                end_captured_at.ok_or_else(|| {
                    invariant_detailed("list reading whole without an end frame", |outcome| {
                        outcome.pack_operation_id(operation.id.as_str())
                    })
                })?,
                Ok(rows),
            ),
            ListRead::Missed(miss) => (last.captured_at, Err(miss)),
        };
        let summary = session.summary(None);
        Ok(ScannedList {
            last,
            captured_at,
            list,
            viewports: summary.viewports,
            unknown: summary.unknown,
            passed_over: session.passed_over(),
        })
    }
}
