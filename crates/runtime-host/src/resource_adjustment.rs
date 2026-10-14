// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #384 D2-S3 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.6): what a successful
//! run's confirmed resource adjustments do to its instance's facts. The plan is made before
//! `Finalizing`, from the run's joined readings and adjustments and each adjusted key's record
//! before the run; the terminal's publication stage carries it out, each write and each drop
//! under `fact_write_gate`.
//!
//! - An adjustment read after its operation (no `amount`) writes nothing itself: a reading of
//!   its key that held on the confirmation frame or later is the new value. Without one, the
//!   key's record is dropped (`fact_adjustment_skipped`, `after_read_missing`).
//! - An adjustment that reads its amount adds it to or subtracts it from the key's current
//!   record: the latest of the key's record before the run, the run's readings of it before the
//!   confirmation and the run's earlier adjustments of it. The new record keeps the current
//!   record's expiry and the lower of the two confidences, and is observed at the confirmation.
//!   It writes nothing when its amount gave no reading (`amount_unresolved`, the record is
//!   dropped), when a reading at or after the confirmation already holds the change or the
//!   current record is newer (`newer_observation`), when there is no integer current record
//!   (`base_missing`) or it has expired (`base_expired`), and when the result would be negative
//!   (`fact_adjustment_underflow`, the record is dropped).
//! - A dropped key keeps none of the run's records of it; only its record from before the run
//!   is dropped, and only when that record is older than the confirmation that dropped it.

use crate::codes::{AdjustmentReason, HostCode};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EventId, FactContent, FactRecord, FactValue, ResourceAdjustmentDirection,
};
use actingcommand_execution_kernel::{ResourceAdjustmentResult, ResourceReadingValue};
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// One handed-off frame whose readings held, as the run joined it.
pub(crate) struct ReadingFrameInput<'a> {
    pub(crate) captured_at: SystemTime,
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) readings: &'a [ResourceReadingValue],
}

/// One joined confirmation of an operation that declares adjustments.
pub(crate) struct ConfirmationInput<'a> {
    pub(crate) captured_at: SystemTime,
    pub(crate) observed_at_unix_ms: u64,
    /// The ledger sequence of the confirmed step's `task.step_finished`.
    pub(crate) step_sequence: u64,
    pub(crate) adjustments: &'a [ResourceAdjustmentResult],
}

/// A key's active record before the run, with the sequence and the id of its
/// `fact.published`.
#[derive(Debug, Clone)]
pub(crate) struct PreRunRecord {
    pub(crate) record: FactRecord,
    pub(crate) sequence: u64,
    pub(crate) event_id: EventId,
}

/// The record an adjustment write adjusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlannedBase {
    /// The key's record before the run.
    PreRun { sequence: u64, event_id: EventId },
    /// An earlier publication of the plan.
    Run(usize),
}

/// One adjustment write: the new record's value, confidence and lifetime, and the record it
/// adjusts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedAdjustment {
    /// The index of its confirmation.
    pub(crate) confirmation: usize,
    pub(crate) adjustment_id: String,
    pub(crate) fact_key: String,
    /// `None` when the adjusted value does not fit an instance fact's integer.
    pub(crate) value: Option<i64>,
    pub(crate) confidence_milli: u16,
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) expires_at_unix_ms: Option<u64>,
    pub(crate) base: PlannedBase,
}

/// One record the run publishes, in publication order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedPublication {
    /// The `reading`-th reading that held on the `frame`-th frame.
    Reading {
        frame: usize,
        reading: usize,
    },
    Adjustment(PlannedAdjustment),
}

/// A key's record from before the run, dropped because a confirmed change of the key was not
/// read: the `fact.published` at `base_sequence`, dropped by the step at `step_sequence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedInvalidation {
    pub(crate) adjustment_id: String,
    pub(crate) fact_key: String,
    pub(crate) base_sequence: u64,
    pub(crate) step_sequence: u64,
}

/// One adjustment's outcome: a row of the run's task diagnostic on its confirmation frame.
#[derive(Debug, Clone)]
pub(crate) struct PlannedOutcome {
    /// The index of its confirmation.
    pub(crate) confirmation: usize,
    /// The amount's OCR target, for an adjustment that reads its amount.
    pub(crate) target_id: Option<String>,
    pub(crate) outcome: Outcome,
}

/// What the run's confirmed adjustments do, and the records the run publishes.
#[derive(Debug, Clone, Default)]
pub(crate) struct AdjustmentPlan {
    pub(crate) outcomes: Vec<PlannedOutcome>,
    pub(crate) invalidations: Vec<PlannedInvalidation>,
    pub(crate) publications: Vec<PlannedPublication>,
}

/// A record of the run that an adjustment can adjust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RunRecord {
    Reading(usize, usize),
    Write(usize),
}

#[derive(Debug, Clone, Copy)]
enum BaseSource {
    PreRun { sequence: u64, event_id: EventId },
    Run(RunRecord),
}

/// The key's current record at a confirmation.
struct Base {
    value: Option<i64>,
    observed_at_unix_ms: u64,
    expires_at_unix_ms: Option<u64>,
    confidence_milli: u16,
    source: BaseSource,
}

/// One adjustment write before its base is placed in the publication order.
struct Write {
    adjustment: PlannedAdjustment,
    base: BaseSource,
}

fn integer(content: &FactContent) -> Option<i64> {
    match content {
        FactContent::Inline {
            value: FactValue::Integer(value),
        } => Some(*value),
        _ => None,
    }
}

/// Plans the run's adjustments (see the module documentation). `frames` are in hand-off order
/// and `confirmations` in confirmation order, both in the order of their capture times; `now`
/// decides whether a current record has expired.
pub(crate) fn plan_resource_adjustments(
    instance_alias: &str,
    task_label: &str,
    now_unix_ms: u64,
    frames: &[ReadingFrameInput<'_>],
    confirmations: &[ConfirmationInput<'_>],
    pre_run: &BTreeMap<String, PreRunRecord>,
) -> AdjustmentPlan {
    let skipped = |key: &str, reason: AdjustmentReason| {
        Outcome::new(HostCode::FactAdjustmentSkipped)
            .instance_id(instance_alias)
            .fact_key(key)
            .adjustment_reason(reason)
            .task_id(task_label)
    };
    let mut outcomes = Vec::new();
    let mut writes: Vec<Write> = Vec::new();
    // The first confirmation that dropped each key: its adjustment, its step and its time.
    let mut dropped: BTreeMap<String, (String, u64, u64)> = BTreeMap::new();
    for (index, confirmation) in confirmations.iter().enumerate() {
        for adjustment in confirmation.adjustments {
            let declaration = &adjustment.declaration;
            let key = declaration.fact_key.as_str();
            let target_id = declaration
                .amount
                .as_ref()
                .map(|amount| amount.target_id.clone());
            // A reading of the key that held on the confirmation frame or a later one.
            let read_after = frames.iter().any(|frame| {
                frame.captured_at >= confirmation.captured_at
                    && frame.readings.iter().any(|reading| reading.fact_key == key)
            });
            let (outcome, drops_key) = match (&declaration.amount, &adjustment.amount) {
                (None, _) => (
                    (!read_after).then(|| skipped(key, AdjustmentReason::AfterReadMissing)),
                    !read_after,
                ),
                (Some(_), Some(Ok(_))) if read_after => (
                    Some(skipped(key, AdjustmentReason::NewerObservation)),
                    false,
                ),
                (Some(declared), Some(Ok(amount))) => {
                    let base = if dropped.contains_key(key) {
                        None
                    } else {
                        current_base(key, confirmation.captured_at, frames, &writes, pre_run)
                    };
                    match base {
                        None => (Some(skipped(key, AdjustmentReason::BaseMissing)), false),
                        Some(base)
                            if matches!(
                                base.source,
                                BaseSource::PreRun { sequence, .. }
                                    if sequence >= confirmation.step_sequence
                            ) || base.observed_at_unix_ms > confirmation.observed_at_unix_ms =>
                        {
                            (
                                Some(skipped(key, AdjustmentReason::NewerObservation)),
                                false,
                            )
                        }
                        Some(base)
                            if base
                                .expires_at_unix_ms
                                .is_some_and(|expires| now_unix_ms > expires) =>
                        {
                            (Some(skipped(key, AdjustmentReason::BaseExpired)), false)
                        }
                        Some(Base { value: None, .. }) => {
                            (Some(skipped(key, AdjustmentReason::BaseMissing)), false)
                        }
                        Some(Base {
                            value: Some(current),
                            expires_at_unix_ms,
                            confidence_milli,
                            source,
                            ..
                        }) => {
                            let change = i64::try_from(amount.value).ok();
                            let value = change.and_then(|change| match declared.direction {
                                ResourceAdjustmentDirection::Consume => current.checked_sub(change),
                                ResourceAdjustmentDirection::Gain => current.checked_add(change),
                            });
                            if value.is_some_and(|value| value < 0) {
                                (
                                    Some(
                                        Outcome::new(HostCode::FactAdjustmentUnderflow)
                                            .instance_id(instance_alias)
                                            .fact_key(key)
                                            .task_id(task_label),
                                    ),
                                    true,
                                )
                            } else {
                                writes.push(Write {
                                    adjustment: PlannedAdjustment {
                                        confirmation: index,
                                        adjustment_id: declaration.id.clone(),
                                        fact_key: key.to_owned(),
                                        value,
                                        confidence_milli: confidence_milli
                                            .min(amount.confidence_milli),
                                        observed_at_unix_ms: confirmation.observed_at_unix_ms,
                                        expires_at_unix_ms,
                                        base: PlannedBase::Run(0),
                                    },
                                    base: source,
                                });
                                (None, false)
                            }
                        }
                    }
                }
                (Some(_), _) => (
                    Some(skipped(key, AdjustmentReason::AmountUnresolved)),
                    !read_after,
                ),
            };
            if let Some(outcome) = outcome {
                outcomes.push(PlannedOutcome {
                    confirmation: index,
                    target_id,
                    outcome,
                });
            }
            if drops_key {
                dropped.entry(key.to_owned()).or_insert((
                    declaration.id.clone(),
                    confirmation.step_sequence,
                    confirmation.observed_at_unix_ms,
                ));
            }
        }
    }
    let invalidations = dropped
        .iter()
        .filter_map(
            |(key, (adjustment_id, step_sequence, observed_at_unix_ms))| {
                let before = pre_run.get(key)?;
                (before.sequence < *step_sequence
                    && before.record.observed_at_unix_ms <= *observed_at_unix_ms)
                    .then(|| PlannedInvalidation {
                        adjustment_id: adjustment_id.clone(),
                        fact_key: key.clone(),
                        base_sequence: before.sequence,
                        step_sequence: *step_sequence,
                    })
            },
        )
        .collect();
    AdjustmentPlan {
        outcomes,
        invalidations,
        publications: publication_order(frames, writes, &dropped.into_keys().collect()),
    }
}

/// The key's current record at `confirmed_at`: the latest of its record before the run, the
/// run's readings of it before the confirmation and the run's earlier adjustments of it.
fn current_base(
    key: &str,
    confirmed_at: SystemTime,
    frames: &[ReadingFrameInput<'_>],
    writes: &[Write],
    pre_run: &BTreeMap<String, PreRunRecord>,
) -> Option<Base> {
    let mut base = pre_run.get(key).map(|before| Base {
        value: integer(&before.record.content),
        observed_at_unix_ms: before.record.observed_at_unix_ms,
        expires_at_unix_ms: before.record.expires_at_unix_ms,
        confidence_milli: before.record.confidence_milli,
        source: BaseSource::PreRun {
            sequence: before.sequence,
            event_id: before.event_id,
        },
    });
    let newer = |base: &Option<Base>, observed_at_unix_ms: u64| {
        base.as_ref()
            .is_none_or(|base| observed_at_unix_ms >= base.observed_at_unix_ms)
    };
    for (frame_index, frame) in frames.iter().enumerate() {
        if frame.captured_at >= confirmed_at {
            continue;
        }
        for (reading_index, reading) in frame.readings.iter().enumerate() {
            if reading.fact_key == key && newer(&base, frame.observed_at_unix_ms) {
                base = Some(Base {
                    value: i64::try_from(reading.value).ok(),
                    observed_at_unix_ms: frame.observed_at_unix_ms,
                    expires_at_unix_ms: frame
                        .observed_at_unix_ms
                        .checked_add(reading.declaration.valid_for_ms),
                    confidence_milli: reading.confidence_milli,
                    source: BaseSource::Run(RunRecord::Reading(frame_index, reading_index)),
                });
            }
        }
    }
    for (write_index, write) in writes.iter().enumerate() {
        let adjustment = &write.adjustment;
        if adjustment.fact_key == key && newer(&base, adjustment.observed_at_unix_ms) {
            base = Some(Base {
                value: adjustment.value,
                observed_at_unix_ms: adjustment.observed_at_unix_ms,
                expires_at_unix_ms: adjustment.expires_at_unix_ms,
                confidence_milli: adjustment.confidence_milli,
                source: BaseSource::Run(RunRecord::Write(write_index)),
            });
        }
    }
    base
}

/// The run's records in publication order: by observation time, readings before writes of the
/// same millisecond, each kind in its own order. A dropped key's records are left out.
fn publication_order(
    frames: &[ReadingFrameInput<'_>],
    writes: Vec<Write>,
    dropped: &BTreeSet<String>,
) -> Vec<PlannedPublication> {
    let mut order = Vec::new();
    for (frame_index, frame) in frames.iter().enumerate() {
        for (reading_index, reading) in frame.readings.iter().enumerate() {
            if !dropped.contains(&reading.fact_key) {
                order.push((
                    frame.observed_at_unix_ms,
                    RunRecord::Reading(frame_index, reading_index),
                ));
            }
        }
    }
    for (write_index, write) in writes.iter().enumerate() {
        if !dropped.contains(&write.adjustment.fact_key) {
            order.push((
                write.adjustment.observed_at_unix_ms,
                RunRecord::Write(write_index),
            ));
        }
    }
    // `RunRecord` orders every reading before every write, each kind in its own order.
    order.sort();
    let positions = order
        .iter()
        .enumerate()
        .map(|(position, (_, record))| (*record, position))
        .collect::<BTreeMap<_, _>>();
    let mut writes = writes.into_iter().map(Some).collect::<Vec<_>>();
    order
        .into_iter()
        .filter_map(|(_, record)| match record {
            RunRecord::Reading(frame, reading) => {
                Some(PlannedPublication::Reading { frame, reading })
            }
            // Each write stands once in the order.
            RunRecord::Write(index) => writes.get_mut(index).and_then(Option::take).map(
                |Write {
                     mut adjustment,
                     base,
                 }| {
                    adjustment.base = match base {
                        BaseSource::PreRun { sequence, event_id } => {
                            PlannedBase::PreRun { sequence, event_id }
                        }
                        // The record a write adjusts is a record of its own key, which is in
                        // the order whenever the write is, and earlier. A position past the
                        // plan is refused when the plan is carried out.
                        BaseSource::Run(record) => {
                            PlannedBase::Run(positions.get(&record).copied().unwrap_or(usize::MAX))
                        }
                    };
                    PlannedPublication::Adjustment(adjustment)
                },
            ),
        })
        .collect()
}
