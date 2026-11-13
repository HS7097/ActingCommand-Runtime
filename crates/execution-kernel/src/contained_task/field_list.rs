// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #450 FL-1a and FL-1b (`contracts/resource-readings.md`, "Lists from fields"; register
//! §1.343): a task's field list groups the OCR fields of one `group` of its `fields_v1`
//! declaration, read one position per collected frame, into one list that the host writes as one
//! `record_list` fact, as it writes a `read_list` step's list.
//!
//! - **The declaration.** A top-level `field_lists` of task schema `0.8` or `0.9`, one entry in
//!   this revision (`FieldListDeclaration` in the contract crate). Admission ([`prepare_field_lists`])
//!   checks it by the contract crate's rule ([`validate_field_lists`]) against the task's declared
//!   fields, then its conditions' targets against the recognition pack (template or colour targets)
//!   and its fields against the projection metadata (`target_personal`). Every refusal is
//!   `contained_task_resource_reading_invalid` with the detail `<id>:<member>`, or the task's
//!   member alone for a rule of the task.
//! - **The frames.** The list's frames are the frames the fields collector collects, in collection
//!   order, each named by its fields report `frame_index`, but for the frame captured before an
//!   input under an action gap, which shows the item of its step's frame and is no list frame (the
//!   fields report still takes it). The collector keeps each frame's key and column readings with
//!   their confidences ([`FieldListRelay`]); the report does not change.
//! - **Its judgement.** On its first frame the list judges `start`; on every frame `end`. The first
//!   frame that meets `end` decides the list ([`field_list_of`]): whole, or its one miss, in the
//!   order start, unread, repeated key, rows. A frame collected after that fails the run. A
//!   successful run that collected frames and met no `end` gives `list_end_unseen` on the frame
//!   captured last.
//! - **The traces.** A whole list records [`ContainedTaskTrace::ListReadingFrame`] for its end
//!   frame, then [`ContainedTaskTrace::ListReading`]; a miss records only the latter, with the
//!   frames it names (`frame_indices`). The host takes them as a `read_list` step's.
//!
//! The core never folds, sums or picks: a key read on two frames is `list_duplicate_key`.

use super::list_reading::{ListReadingDeclaration, ListReadingMiss, ListReadingRows};
use super::{
    ContainedTaskError, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTrace,
    PreparedContainedTask, PreparedOcrFields, ResourceReadingProblem, Scene, TaskProgram,
    invariant_detailed,
};
use crate::codes::ExecutionKernelCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    FactScalar, FieldListCondition, FieldListDeclaration, FieldListTask, MAX_LIST_READING_ROWS,
    OcrFieldPrivacy, OcrFieldReason, OcrFieldRecord, OcrFieldType, OcrFieldValue,
    OcrFieldsDeclaration, ocr_confidence_milli, validate_field_lists,
};
use actingcommand_recognition_pack::{RecognitionEvaluator, TargetKind};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

/// The task member that declares field lists, the reason a startup or layer run refuses one.
pub(super) const FIELD_LISTS_MEMBER: &str = "field_lists";

/// `contained_task_resource_reading_invalid` with `detail`.
fn reading_invalid(detail: String) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        ExecutionKernelCode::ContainedTaskResourceReadingInvalid.code(),
        detail,
    )
}

/// A task's `field_lists`: as the package writes them, as admission prepares them, and the
/// device capture time of the run's latest capture, which a list that saw no end is reported on.
/// Admission sets `prepared`; the package never does.
#[derive(Debug, Default)]
pub(super) struct TaskFieldLists {
    declared: Option<serde_json::Value>,
    prepared: Vec<PreparedFieldList>,
    latest_capture: Mutex<Option<SystemTime>>,
}

impl<'de> Deserialize<'de> for TaskFieldLists {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde_json::Value::deserialize(deserializer).map(|declared| Self {
            declared: Some(declared),
            ..Self::default()
        })
    }
}

impl TaskFieldLists {
    /// The admitted lists, none before admission or without the member.
    pub(super) fn prepared(&self) -> &[PreparedFieldList] {
        &self.prepared
    }

    /// A run starts: no capture yet. Nothing to forget for a task that declares no list.
    pub(super) fn run_started(&self) {
        if self.prepared.is_empty() {
            return;
        }
        *self
            .latest_capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The run captured a frame at `captured_at` (its device capture time). Kept only for a task
    /// that declares a list: every capture of a run passes here, so the latest is the frame the
    /// host binds as the one captured last.
    pub(super) fn note_capture(&self, captured_at: SystemTime) {
        if self.prepared.is_empty() {
            return;
        }
        *self
            .latest_capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(captured_at);
    }

    fn latest_capture(&self) -> Option<SystemTime> {
        *self
            .latest_capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl TaskProgram {
    /// Workflow #450 FL-1b: whether the task declares `field_lists`.
    pub(super) fn declares_field_lists(&self) -> bool {
        self.field_lists.declared.is_some()
    }
}

/// One admitted column: the row member `column` holds the value of the field `field`, an integer
/// when the field is an `unsigned_integer`, its dictionary entry otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedFieldColumn {
    column: String,
    field: String,
    integer: bool,
}

/// An admitted field list. Set by admission and never read from the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedFieldList {
    id: String,
    group: String,
    key_field: String,
    key_column: String,
    columns: Vec<PreparedFieldColumn>,
    minimum_confidence_milli: u16,
    fact_key: String,
    valid_for_ms: u64,
    start: FieldListCondition,
    end: FieldListCondition,
}

impl PreparedFieldList {
    /// The list `declaration` reads from `fields`, the fields declaration it was checked against.
    pub(super) fn of(declaration: &FieldListDeclaration, fields: &OcrFieldsDeclaration) -> Self {
        let integer = |id: &str| {
            fields.fields.iter().any(|field| {
                field.id == id && matches!(field.value, OcrFieldType::UnsignedInteger { .. })
            })
        };
        Self {
            id: declaration.id.clone(),
            group: declaration.group.clone(),
            key_field: declaration.key_field.clone(),
            key_column: declaration.key_column.clone(),
            columns: declaration
                .columns
                .iter()
                .map(|column| PreparedFieldColumn {
                    column: column.column.clone(),
                    field: column.field.clone(),
                    integer: integer(column.field.as_str()),
                })
                .collect(),
            minimum_confidence_milli: declaration.minimum_confidence_milli,
            fact_key: declaration.fact_key.clone(),
            valid_for_ms: declaration.valid_for_ms,
            start: declaration.start.clone(),
            end: declaration.end.clone(),
        }
    }

    /// The declaration the host publishes the list by, with the frames a miss names.
    fn declaration(&self, frame_indices: Vec<u32>) -> ListReadingDeclaration {
        ListReadingDeclaration {
            operation_id: self.id.clone(),
            fact_key: self.fact_key.clone(),
            valid_for_ms: self.valid_for_ms,
            frame_indices,
        }
    }

    /// Every field the list reads: its key field, then its columns' fields.
    fn fields(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.key_field.as_str())
            .chain(self.columns.iter().map(|column| column.field.as_str()))
    }
}

/// Contained task admission of the task's field lists, after its fields are prepared
/// (`contracts/resource-readings.md`, "Lists from fields", "Declaration"): the contract crate's
/// rule against the fields the task declares, its operation and mark ids, and every key its marks
/// and its `read_list` steps write; then each condition's targets, each a template or colour
/// target of the recognition pack (`<id>:start/required/<k>`, …); then each field the list reads,
/// whose target the projection metadata does not mark personal (`<id>:target_personal`). Each
/// refusal is `contained_task_resource_reading_invalid`.
pub(super) fn prepare_field_lists(
    program: &mut TaskProgram,
    game: &str,
    evaluator: &RecognitionEvaluator,
    fields: Option<&PreparedOcrFields>,
) -> Result<(), ContainedTaskError> {
    let Some(declared) = &program.field_lists.declared else {
        return Ok(());
    };
    let lists: Vec<FieldListDeclaration> = serde_json::from_value(declared.clone())
        .map_err(|_| reading_invalid(FIELD_LISTS_MEMBER.to_owned()))?;
    // The rule reads the fields as declared; a target the projection marks personal is refused
    // below, by its own reason.
    let declared_fields = program
        .post_admission_ocr
        .as_ref()
        .and_then(|value| serde_json::from_value::<OcrFieldsDeclaration>(value.clone()).ok());
    let taken_ids = program
        .operations
        .iter()
        .map(|operation| operation.id.clone())
        .chain(
            program
                .resource_readings
                .iter()
                .flatten()
                .map(|reading| reading.id.clone()),
        )
        .collect::<BTreeSet<_>>();
    let written_keys = program
        .resource_readings
        .iter()
        .flatten()
        .flat_map(|reading| reading.fact_keys())
        .map(str::to_owned)
        .chain(
            program
                .operations
                .iter()
                .filter_map(|operation| operation.prepared_list.as_deref())
                .map(|list| list.fact_key().to_owned()),
        )
        .collect::<BTreeSet<_>>();
    validate_field_lists(
        &lists,
        FieldListTask {
            game,
            schema_version: &program.schema_version,
            fields: declared_fields.as_ref(),
            scheduling: program.scheduling_outcome.as_ref(),
            taken_ids: &taken_ids,
            written_keys: &written_keys,
        },
    )
    .map_err(|refusal| reading_invalid(refusal.detail()))?;
    let fields = fields.ok_or_else(|| reading_invalid("post_admission_ocr".to_owned()))?;
    let mut prepared = Vec::with_capacity(lists.len());
    for list in &lists {
        let refuse = |member: &str| reading_invalid(format!("{}:{member}", list.id));
        for (name, condition) in [("start", &list.start), ("end", &list.end)] {
            for (member, targets) in [
                ("required", &condition.required),
                ("forbidden", &condition.forbidden),
            ] {
                for (index, target) in targets.iter().enumerate() {
                    if !matches!(
                        evaluator.target_kind(target),
                        Ok(TargetKind::Template | TargetKind::Color)
                    ) {
                        return Err(refuse(&format!("{name}/{member}/{index}")));
                    }
                }
            }
        }
        let list = PreparedFieldList::of(list, &fields.declaration);
        let personal = list.fields().any(|id| {
            fields
                .declaration
                .fields
                .iter()
                .any(|field| field.id == id && field.privacy == OcrFieldPrivacy::Personal)
        });
        if personal {
            return Err(reading_invalid(format!("{}:target_personal", list.id)));
        }
        prepared.push(list);
    }
    program.field_lists.prepared = prepared;
    Ok(())
}

/// One field a field list reads on one frame: the field's value and reason in the fields report,
/// and the confidence of its observation in milli, when the provider gave one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FieldListReading {
    pub(super) value: Option<OcrFieldValue>,
    pub(super) reason: OcrFieldReason,
    pub(super) confidence_milli: Option<u16>,
}

/// One frame of a field list: the fields report's `frame_index`, the key field's reading and each
/// column's, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FieldListFrame {
    pub(super) frame_index: u32,
    pub(super) key: FieldListReading,
    pub(super) columns: Vec<FieldListReading>,
}

/// A decided field list: whole, or its one miss with the fields report's frames it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FieldListRead {
    Whole(ListReadingRows),
    Missed {
        miss: ListReadingMiss,
        frame_indices: Vec<u32>,
    },
}

fn missed(problem: ResourceReadingProblem, count: usize, frame_indices: Vec<u32>) -> FieldListRead {
    FieldListRead::Missed {
        miss: ListReadingMiss {
            problem,
            count: u32::try_from(count).unwrap_or(u32::MAX),
        },
        frame_indices,
    }
}

/// A reading resolved at the list's floor: its value and confidence; `None` for one that is not
/// resolved, has no value, or whose confidence is missing or below `floor`.
fn resolved(reading: &FieldListReading, floor: u16) -> Option<(&OcrFieldValue, u16)> {
    if reading.reason != OcrFieldReason::Resolved {
        return None;
    }
    let value = reading.value.as_ref()?;
    let confidence = reading.confidence_milli.filter(|milli| *milli >= floor)?;
    Some((value, confidence))
}

/// One resolved frame of a list: its key, its row `{<key_column>: <key>, <column>: <value>, …}`
/// and the lowest confidence of the fields it read.
struct FrameRow {
    key: String,
    row: BTreeMap<String, FactScalar>,
    lowest: u16,
}

/// One frame's row; `None` for an unresolved frame. A resolved value of another type than its
/// field's is an invariant failure.
fn frame_row(
    list: &PreparedFieldList,
    frame: &FieldListFrame,
) -> Result<Option<FrameRow>, ContainedTaskError> {
    let invariant = || {
        invariant_detailed(
            "field list: a reading of another type than its field",
            |outcome| outcome,
        )
    };
    let floor = list.minimum_confidence_milli;
    if frame.columns.len() != list.columns.len() {
        return Err(invariant());
    }
    let Some((key, mut lowest)) = resolved(&frame.key, floor) else {
        return Ok(None);
    };
    let OcrFieldValue::DictionaryEntry(key) = key else {
        return Err(invariant());
    };
    let mut row = BTreeMap::from([(list.key_column.clone(), FactScalar::String(key.clone()))]);
    for (column, reading) in list.columns.iter().zip(&frame.columns) {
        let Some((value, confidence)) = resolved(reading, floor) else {
            return Ok(None);
        };
        let value = match (column.integer, value) {
            (true, OcrFieldValue::UnsignedInteger(value)) => {
                FactScalar::Integer(i64::try_from(*value).map_err(|_| invariant())?)
            }
            (false, OcrFieldValue::DictionaryEntry(value)) => FactScalar::String(value.clone()),
            _ => return Err(invariant()),
        };
        row.insert(column.column.clone(), value);
        lowest = lowest.min(confidence);
    }
    Ok(Some(FrameRow {
        key: key.clone(),
        row,
        lowest,
    }))
}

/// Workflow #450 FL-1a (`contracts/resource-readings.md`, "Lists from fields", "When the list is
/// whole"): the list `list` decided on the last of `frames`, its frames in collection order, whose
/// first frame met `start` when `start_met`. Its one miss is the first that applies:
///
/// 1. `list_start_unseen`: the first frame did not meet `start` (`count` the frames,
///    `frame_indices` the first frame);
/// 2. `list_position_unread`: a frame whose key or a column is not resolved at the list's floor
///    (`count` and `frame_indices` those frames);
/// 3. `list_duplicate_key`: a key read on two frames or more, adjacent or not, with equal or
///    different columns (`count` the repeated keys, `frame_indices` every frame holding one);
/// 4. `list_rows_exceeded`: more rows than one fact holds (`count` the position past the bound,
///    `frame_indices` its frame).
///
/// Otherwise the list is whole: one row per frame in ascending byte order of the key, with the
/// lowest confidence of the fields the rows read.
pub(super) fn field_list_of(
    list: &PreparedFieldList,
    frames: &[FieldListFrame],
    start_met: bool,
) -> Result<FieldListRead, ContainedTaskError> {
    let Some(first) = frames.first() else {
        return Err(invariant_detailed(
            "field list decided with no frame",
            |outcome| outcome,
        ));
    };
    if !start_met {
        return Ok(missed(
            ResourceReadingProblem::ListStartUnseen,
            frames.len(),
            vec![first.frame_index],
        ));
    }
    let mut unread = Vec::new();
    let mut keyed: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut rows = BTreeMap::new();
    let mut confidence: Option<u16> = None;
    for frame in frames {
        let Some(FrameRow { key, row, lowest }) = frame_row(list, frame)? else {
            unread.push(frame.frame_index);
            continue;
        };
        keyed
            .entry(key.clone())
            .or_default()
            .push(frame.frame_index);
        confidence = Some(confidence.map_or(lowest, |confidence| confidence.min(lowest)));
        rows.insert(key, row);
    }
    if !unread.is_empty() {
        return Ok(missed(
            ResourceReadingProblem::ListPositionUnread,
            unread.len(),
            unread,
        ));
    }
    let repeated = keyed
        .values()
        .filter(|frames| frames.len() > 1)
        .collect::<Vec<_>>();
    if !repeated.is_empty() {
        let mut frame_indices = repeated
            .iter()
            .flat_map(|frames| frames.iter().copied())
            .collect::<Vec<_>>();
        frame_indices.sort_unstable();
        return Ok(missed(
            ResourceReadingProblem::ListDuplicateKey,
            repeated.len(),
            frame_indices,
        ));
    }
    if rows.len() > MAX_LIST_READING_ROWS {
        return Ok(missed(
            ResourceReadingProblem::ListRowsExceeded,
            MAX_LIST_READING_ROWS + 1,
            frames
                .get(MAX_LIST_READING_ROWS)
                .map(|frame| frame.frame_index)
                .into_iter()
                .collect(),
        ));
    }
    let confidence_milli = confidence
        .ok_or_else(|| invariant_detailed("field list whole with no row", |outcome| outcome))?;
    Ok(FieldListRead::Whole(ListReadingRows {
        rows: rows.into_values().collect(),
        confidence_milli,
    }))
}

/// Workflow #450 FL-1b: the miss of a list whose run succeeded after it collected `frames` and
/// met no `end`: `list_end_unseen`, `count` the frames, `frame_indices` the last frame.
pub(super) fn field_list_end_unseen(frames: &[FieldListFrame]) -> Option<FieldListRead> {
    let last = frames.last()?;
    Some(missed(
        ResourceReadingProblem::ListEndUnseen,
        frames.len(),
        vec![last.frame_index],
    ))
}

/// Workflow #450 FL-1a: whether a frame recognized as `page_label` meets `condition`: on one of
/// its `page_ids` when it names any, every `required` target passing and no `forbidden` one, as
/// `passes` judges a target on the frame.
pub(super) fn condition_met(
    game: &str,
    condition: &FieldListCondition,
    page_label: &str,
    passes: &mut dyn FnMut(&str) -> Result<bool, ContainedTaskError>,
) -> Result<bool, ContainedTaskError> {
    if let Some(pages) = &condition.page_ids
        && !pages
            .iter()
            .any(|page| crate::page_anchor_matches(game, page_label, page))
    {
        return Ok(false);
    }
    for target in &condition.required {
        if !passes(target.as_str())? {
            return Ok(false);
        }
    }
    for target in &condition.forbidden {
        if passes(target.as_str())? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The state of one field list in a run.
#[derive(Debug)]
struct FieldListState<'a> {
    list: &'a PreparedFieldList,
    frames: Vec<FieldListFrame>,
    /// Whether its first frame met `start`; `None` before its first frame.
    start_met: Option<bool>,
    /// Whether the list was decided (on its end frame, or with `list_end_unseen`).
    decided: bool,
    /// Whether it collected a frame that awaits its judgement.
    awaiting: bool,
}

/// Workflow #450 FL-1b: the fields collector's bookkeeping of a run's field lists: each list's
/// collected frames, with the key and column readings and their confidences. In memory only; the
/// fields report does not change.
#[derive(Debug)]
pub(super) struct FieldListRelay<'a> {
    lists: Vec<FieldListState<'a>>,
}

impl<'a> FieldListRelay<'a> {
    pub(super) fn new(lists: &'a [PreparedFieldList]) -> Self {
        Self {
            lists: lists
                .iter()
                .map(|list| FieldListState {
                    list,
                    frames: Vec::new(),
                    start_met: None,
                    decided: false,
                    awaiting: false,
                })
                .collect(),
        }
    }

    /// The collector collected the frame `frame_index`: `records` are its fields, one record per
    /// group, and `confidences` the confidence of each field's observation. Each list keeps its
    /// key's and columns' readings, to be judged once the frame's fields are recorded.
    pub(super) fn collect(
        &mut self,
        frame_index: u32,
        records: &[OcrFieldRecord],
        confidences: &BTreeMap<&str, Option<f32>>,
    ) {
        for state in &mut self.lists {
            let reading = |field: &str| {
                let result = records
                    .iter()
                    .filter(|record| record.group == state.list.group)
                    .flat_map(|record| &record.fields)
                    .find(|result| result.field_id == field);
                FieldListReading {
                    value: result.and_then(|result| result.value.clone()),
                    reason: result.map_or(OcrFieldReason::NotCollected, |result| result.reason),
                    confidence_milli: ocr_confidence_milli(
                        confidences.get(field).copied().flatten(),
                    ),
                }
            };
            let frame = FieldListFrame {
                frame_index,
                key: reading(state.list.key_field.as_str()),
                columns: state
                    .list
                    .columns
                    .iter()
                    .map(|column| reading(column.field.as_str()))
                    .collect(),
            };
            state.frames.push(frame);
            state.awaiting = true;
        }
    }
}

impl PreparedContainedTask {
    /// Workflow #450 FL-1b: the run notes each frame it captures, so that a list that saw no end
    /// is reported on the frame captured last.
    pub(super) fn note_capture(&self, captured_at: SystemTime) {
        self.program.field_lists.note_capture(captured_at);
    }

    /// Workflow #450 FL-1b: the judgement of the frame the run's field lists collected last, the
    /// frame captured last (`captured_at`), recognized as `page_label` on `scene`, once its fields
    /// are recorded and before any reading hand-off. On a list's first frame its `start` is judged;
    /// on every frame its `end`. A frame that meets `end` decides the list: a whole list records
    /// `ListReadingFrame` and `ListReading` on this frame, a miss `ListReading` alone. A frame
    /// collected after the list was decided fails the run (`contained_task_state_invalid`).
    pub(super) fn judge_field_list_frame<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        relay: Option<&mut FieldListRelay<'_>>,
        (page_label, scene): (&str, &Scene),
        captured_at: SystemTime,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let Some(relay) = relay else {
            return Ok(());
        };
        let game = self.control.game.as_str();
        let mut passes = |target: &str| {
            self.evaluator
                .evaluate_target(scene, target)
                .map(|evaluation| evaluation.passed)
                .map_err(|error| {
                    ContainedTaskError::with_detail(
                        SharedCode::ContainedTaskRecognitionFailed.code(),
                        error.to_string(),
                    )
                })
        };
        for state in &mut relay.lists {
            if !std::mem::take(&mut state.awaiting) {
                continue;
            }
            if state.decided {
                return Err(invariant_detailed(
                    "field list: a frame of its group was collected after its end",
                    |outcome| outcome,
                )
                .into());
            }
            if state.start_met.is_none() {
                state.start_met = Some(condition_met(
                    game,
                    &state.list.start,
                    page_label,
                    &mut passes,
                )?);
            }
            if !condition_met(game, &state.list.end, page_label, &mut passes)? {
                continue;
            }
            state.decided = true;
            let read = field_list_of(state.list, &state.frames, state.start_met.unwrap_or(false))?;
            record_field_list(runtime, state.list, read, captured_at)?;
        }
        Ok(())
    }

    /// Workflow #450 FL-1b: at a successful end, before the readings join, each list that
    /// collected frames and was not decided records its miss `list_end_unseen` on the frame
    /// captured last.
    pub(super) fn record_field_lists_end_unseen<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        relay: Option<&mut FieldListRelay<'_>>,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let Some(relay) = relay else {
            return Ok(());
        };
        for state in &mut relay.lists {
            if state.decided {
                continue;
            }
            let Some(read) = field_list_end_unseen(&state.frames) else {
                continue;
            };
            state.decided = true;
            let captured_at = self.program.field_lists.latest_capture().ok_or_else(|| {
                invariant_detailed("field list: frames without a capture", |outcome| outcome)
            })?;
            record_field_list(runtime, state.list, read, captured_at)?;
        }
        Ok(())
    }
}

/// The traces of a decided list on the frame captured last (`captured_at`): a whole list binds
/// that frame (`ListReadingFrame`) and records its rows; a miss records itself.
fn record_field_list<R: ContainedTaskRuntime>(
    runtime: &mut R,
    list: &PreparedFieldList,
    read: FieldListRead,
    captured_at: SystemTime,
) -> Result<(), ContainedTaskRunError<R::Error>> {
    let (declaration, list) = match read {
        FieldListRead::Whole(rows) => {
            runtime
                .record(ContainedTaskTrace::ListReadingFrame { captured_at })
                .map_err(ContainedTaskRunError::Boundary)?;
            (list.declaration(Vec::new()), Ok(rows))
        }
        FieldListRead::Missed {
            miss,
            frame_indices,
        } => (list.declaration(frame_indices), Err(miss)),
    };
    runtime
        .record(ContainedTaskTrace::ListReading {
            declaration,
            captured_at,
            list,
        })
        .map_err(ContainedTaskRunError::Boundary)
}
