// SPDX-License-Identifier: AGPL-3.0-only

//! SCH-M slice ED-2 (frozen model v1.0 §3.4 D-13, D-14; Alice 10-22: 「每个任务执行所需的时间可以
//! 动态的从账本读，近5次该任务包的平均时间加5分钟」; ruling 39 R6.2, R6.3): the instance store
//! record `health.run_duration`, one per instance, which holds the latest durations of the
//! instance's successful scheduled runs, per task package.
//!
//! - One `record_list` row per package: `{package_id, n, d1 … dn, at}`, `d1` the newest duration
//!   and `at` the terminal time of the run that added it. A row keeps at most the instance's
//!   `scheduling.expected_runtime.runs` durations (1 to 16).
//! - At most [`MAX_RUN_DURATION_ROWS`] rows: a new package's row beyond them drops the row with
//!   the oldest `at`.
//! - Package version, content hash and Runtime version are not part of a row (R6.2): a package
//!   update and a Runtime upgrade keep it.
//!
//! This module only reads and builds the value; the host writes it at a settlement
//! (`host/run_duration.rs`) and reads it in its projection into the evaluation.

use actingcommand_contract::{FactScalar, FactValue};
use std::collections::BTreeMap;

/// The record's key: an instance-scoped key of the `health.` family that only the Runtime
/// writes (`run_duration_key_reserved` refuses it from any client).
pub(crate) const RUN_DURATION_FACT_KEY: &str = "health.run_duration";
/// The record's `source_detector`.
pub(crate) const RUN_DURATION_DETECTOR: &str = "runtime.run-duration";
/// The record's `schema_version`.
pub(crate) const RUN_DURATION_SCHEMA: &str = "fact.v1";
/// The `source_snapshot_id` prefix of a publication (`run_duration:<digest>`).
pub(crate) const RUN_DURATION_SNAPSHOT_PREFIX: &str = "run_duration:";
/// The most rows one record holds: the `record_list` bound.
pub(crate) const MAX_RUN_DURATION_ROWS: usize = 256;
/// The most durations one row holds (`d1` … `d16`): the range of
/// `scheduling.expected_runtime.runs`.
pub(crate) const MAX_RUN_DURATION_SAMPLES: usize =
    actingcommand_policy::MAX_EXPECTED_RUNTIME_RUNS as usize;

const PACKAGE_ID: &str = "package_id";
const COUNT: &str = "n";
const AT: &str = "at";

/// One package's latest durations, newest first, and the terminal time of the run that added the
/// newest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunDurationRow {
    pub(crate) samples: Vec<u64>,
    pub(crate) at_unix_ms: u64,
}

/// A record's rows by package, and what of the stored value could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunDurations {
    rows: BTreeMap<String, RunDurationRow>,
    /// One entry per stored row that does not have the row's shape, with its `package_id` when
    /// that member could be read. Only the Runtime writes the record, so this stays empty unless
    /// the ledger holds a row another writer made; the next write drops such rows and reports
    /// each one.
    unreadable: Vec<Option<String>>,
}

impl RunDurations {
    /// The rows of a stored value: an absent record has none; a value that is not a
    /// `record_list` is one unreadable entry.
    pub(crate) fn read(value: Option<&FactValue>) -> Self {
        let mut durations = Self::default();
        match value {
            None => {}
            Some(FactValue::RecordList(rows)) => {
                for row in rows {
                    match read_row(row) {
                        Some((package_id, row)) if !durations.rows.contains_key(&package_id) => {
                            durations.rows.insert(package_id, row);
                        }
                        _ => durations.unreadable.push(match row.get(PACKAGE_ID) {
                            Some(FactScalar::String(package_id)) => Some(package_id.clone()),
                            _ => None,
                        }),
                    }
                }
            }
            Some(_) => durations.unreadable.push(None),
        }
        durations
    }

    /// A stored record whose content is not an inline value: one unreadable entry, which the next
    /// write drops and reports.
    pub(crate) fn unreadable_record() -> Self {
        Self {
            rows: BTreeMap::new(),
            unreadable: vec![None],
        }
    }

    /// The package's durations, newest first; empty when it has no row.
    pub(crate) fn samples(&self, package_id: &str) -> &[u64] {
        self.rows
            .get(package_id)
            .map_or(&[], |row| row.samples.as_slice())
    }

    /// The package's row, if it has one.
    #[cfg(test)]
    pub(crate) fn row(&self, package_id: &str) -> Option<&RunDurationRow> {
        self.rows.get(package_id)
    }

    /// How many rows the record holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Adds one successful run of `package_id` that took `duration_ms` and ended at
    /// `at_unix_ms`: the duration becomes the row's `d1` and the row keeps the newest `keep`
    /// (`1..=16`). A new row beyond [`MAX_RUN_DURATION_ROWS`] drops the row with the oldest `at`
    /// (the smallest package id among equals), never the one just added. Returns what the write
    /// drops: the rows of the stored value that could not be read, then the row the bound
    /// dropped, each by its `package_id` when it has a readable one.
    pub(crate) fn add(
        &mut self,
        package_id: &str,
        duration_ms: u64,
        at_unix_ms: u64,
        keep: u8,
    ) -> Vec<Option<String>> {
        let keep = usize::from(keep).clamp(1, MAX_RUN_DURATION_SAMPLES);
        let mut dropped = std::mem::take(&mut self.unreadable);
        let row = self
            .rows
            .entry(package_id.to_owned())
            .or_insert_with(|| RunDurationRow {
                samples: Vec::new(),
                at_unix_ms,
            });
        row.samples.insert(0, duration_ms);
        row.samples.truncate(keep);
        row.at_unix_ms = at_unix_ms;
        if self.rows.len() > MAX_RUN_DURATION_ROWS {
            let oldest = self
                .rows
                .iter()
                .filter(|(package, _)| package.as_str() != package_id)
                .min_by(|(left_id, left), (right_id, right)| {
                    left.at_unix_ms
                        .cmp(&right.at_unix_ms)
                        .then_with(|| left_id.cmp(right_id))
                })
                .map(|(package, _)| package.clone());
            if let Some(oldest) = oldest {
                self.rows.remove(&oldest);
                dropped.push(Some(oldest));
            }
        }
        dropped
    }

    /// The record's value: one row per package, in package order.
    pub(crate) fn value(&self) -> FactValue {
        FactValue::RecordList(
            self.rows
                .iter()
                .map(|(package_id, row)| {
                    let mut members = BTreeMap::from([
                        (
                            PACKAGE_ID.to_owned(),
                            FactScalar::String(package_id.clone()),
                        ),
                        (
                            COUNT.to_owned(),
                            // At most 16 durations.
                            FactScalar::Integer(row.samples.len() as i64),
                        ),
                        (AT.to_owned(), FactScalar::TimestampMs(row.at_unix_ms)),
                    ]);
                    for (index, sample) in row.samples.iter().enumerate() {
                        members.insert(sample_member(index), FactScalar::DurationMs(*sample));
                    }
                    members
                })
                .collect(),
        )
    }
}

/// `d1` for the newest duration, `d16` for the sixteenth.
fn sample_member(index: usize) -> String {
    format!("d{}", index + 1)
}

/// One stored row: exactly `package_id` (a non-empty string), `n` (1 to 16), `d1` … `dn`
/// (durations) and `at` (a timestamp).
fn read_row(row: &BTreeMap<String, FactScalar>) -> Option<(String, RunDurationRow)> {
    let Some(FactScalar::String(package_id)) = row.get(PACKAGE_ID) else {
        return None;
    };
    let Some(FactScalar::Integer(count)) = row.get(COUNT) else {
        return None;
    };
    let Some(FactScalar::TimestampMs(at_unix_ms)) = row.get(AT) else {
        return None;
    };
    let count = usize::try_from(*count)
        .ok()
        .filter(|count| (1..=MAX_RUN_DURATION_SAMPLES).contains(count))?;
    if package_id.is_empty() || row.len() != count + 3 {
        return None;
    }
    let samples = (0..count)
        .map(|index| match row.get(&sample_member(index)) {
            Some(FactScalar::DurationMs(sample)) => Some(*sample),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((
        package_id.clone(),
        RunDurationRow {
            samples,
            at_unix_ms: *at_unix_ms,
        },
    ))
}
