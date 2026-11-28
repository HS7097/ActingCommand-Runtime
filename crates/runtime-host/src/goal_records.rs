// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR2 (register §1.537; `contracts/instance-fact-store.md`, "Goal records facts";
//! `contracts/standard-pack-goals.md`, "The records entry"): the instance fact
//! `session.goal_records`, the records a person or an agent gave the `records` entries of the
//! application's game root, whom to raise and to where, as pure functions of what the host read.
//!
//! - **The record** ([`StoredGoalRecords::read`]): one header row
//!   `{row: "header", schema_version: "actingcommand.goal-records.v1", instance}`, then one row
//!   for each record, `{row: "record", entry, key, to, set_unix_ms, channel, initiator?}`, at most
//!   [`MAX_INSTANCE_GOAL_RECORDS`] over all the instance's entries. `to` is the record's target in
//!   its canonical text (compact JSON, the members of every object in ascending byte order of
//!   their names, a position an array of whole numbers), at most [`MAX_GOAL_RECORD_TO_BYTES`]. The
//!   reader holds the rows to their form only: a record whose entry, key, column or position the
//!   root in force no longer declares is read back as it is.
//! - **The edit** ([`compose`]): a `set` of a records entry replaces that entry's records where its
//!   first one stood (else they are added after the others), an empty one and a `clear` remove
//!   them; every other record is kept as it is stored, with who set it. The identity is the records
//!   alone (each entry, key and `to`, in their order), so the same records set again is a replay.
//!   The record is read back by the reader before it is answered.
//!
//! Nothing here reads a clock, a file, a lock or the ledger, and nothing is computed from a
//! record: no evaluation reads one in this release.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    FactScalar, GoalSetBy, Initiator, MAX_GOAL_TABLE_ID_BYTES, RuntimeChannel, goal_name,
};
use actingcommand_policy::TargetSetter;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The version word of the record's header.
pub(crate) const GOAL_RECORDS_SCHEMA_VERSION: &str = "actingcommand.goal-records.v1";
/// The most records an instance holds over all its records entries: with the header, 129 rows,
/// inside the 256 rows of a record list the scheduler's input takes.
pub(crate) const MAX_INSTANCE_GOAL_RECORDS: usize = 128;
/// The longest canonical text of a record's `to`: the fact string bound.
pub(crate) const MAX_GOAL_RECORD_TO_BYTES: usize = 1_024;

const FIELD_ROW: &str = "row";
const ROW_HEADER: &str = "header";
const ROW_RECORD: &str = "record";
const FIELD_SCHEMA_VERSION: &str = "schema_version";
const FIELD_INSTANCE: &str = "instance";
const FIELD_ENTRY: &str = "entry";
const FIELD_KEY: &str = "key";
const FIELD_TO: &str = "to";
const FIELD_SET_AT: &str = "set_unix_ms";
const FIELD_CHANNEL: &str = "channel";
const FIELD_INITIATOR: &str = "initiator";
/// The fields of a record row without and with its `initiator`.
const RECORD_FIELDS: usize = 6;
/// The channels a record row names, as the receipt names them.
const CHANNELS: [RuntimeChannel; 4] = [
    RuntimeChannel::Cli,
    RuntimeChannel::Mcp,
    RuntimeChannel::Ui,
    RuntimeChannel::Lab,
];

/// One row of the record.
pub(crate) type GoalRecordsRow = BTreeMap<String, FactScalar>;

/// One record a `set` gives, as its check took it: a key of the entry's table and the canonical
/// text of its `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GivenRecord {
    pub(crate) key: String,
    pub(crate) to: String,
}

/// The one change an `EditGoal` makes to the record: the records of `entry` replaced by `records`
/// (none: removed), or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordsChange {
    Set {
        entry: String,
        records: Vec<GivenRecord>,
    },
    Clear {
        entry: String,
    },
}

/// One stored record: its entry, key and `to`, and its row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredRecordRow {
    pub(crate) entry: String,
    pub(crate) key: String,
    pub(crate) to: String,
    pub(crate) row: GoalRecordsRow,
}

impl StoredRecordRow {
    /// Workflow #455 RR4b-1: who set the record, as its row names it: the Runtime's clock at the
    /// request, the channel and the initiator. The reader takes a row only with these of their
    /// form, so `None` is a row it did not take.
    pub(crate) fn set_by(&self) -> Option<GoalSetBy> {
        let unix_ms = match self.row.get(FIELD_SET_AT) {
            Some(FactScalar::Integer(at)) => u64::try_from(*at).ok()?,
            _ => return None,
        };
        let channel = text(&self.row, FIELD_CHANNEL)?;
        let channel = CHANNELS
            .into_iter()
            .find(|known| known.as_str() == channel)?;
        let initiator = match self.row.get(FIELD_INITIATOR) {
            None => None,
            Some(FactScalar::String(word)) => Some(Initiator::from_word(word)?),
            Some(_) => return None,
        };
        Some(GoalSetBy {
            unix_ms,
            channel,
            initiator,
        })
    }

    /// Workflow #455 RR4b-1: the record's `to` as the object its canonical text writes. The
    /// reader takes a row only with `to` the text of an object, so `None` is a row it did not take.
    pub(crate) fn target(&self) -> Option<serde_json::Map<String, Value>> {
        match serde_json::from_str::<Value>(&self.to).ok()? {
            Value::Object(members) => Some(members),
            _ => None,
        }
    }
}

/// An instance's stored `session.goal_records` record, as the edit reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoredGoalRecords {
    /// The instance holds no record of the key.
    Absent,
    /// A record the reader reads: its records in their order (none for a header alone).
    Read(Vec<StoredRecordRow>),
    /// A record the reader does not read, which only a fault of the Runtime writes.
    Unreadable,
}

impl StoredGoalRecords {
    /// `instance`'s stored record `rows` (`None` without one), held to the form of its rows: one
    /// leading header of this version naming the instance, then at most
    /// [`MAX_INSTANCE_GOAL_RECORDS`] record rows of exactly their fields (an entry name's form, a
    /// key of 1 to 128 bytes without a control character, a `to` that is the text of a JSON object
    /// of at most [`MAX_GOAL_RECORD_TO_BYTES`] bytes, a `set_unix_ms` of at least 0, a channel's
    /// word, an optional initiator's word), no key named twice under one entry. Nothing is judged
    /// against the root in force.
    pub(crate) fn read(rows: Option<&[GoalRecordsRow]>, instance: &str) -> Self {
        let Some(rows) = rows else {
            return Self::Absent;
        };
        let Some((first, records)) = rows.split_first() else {
            return Self::Unreadable;
        };
        if !is_header(first, instance) || records.len() > MAX_INSTANCE_GOAL_RECORDS {
            return Self::Unreadable;
        }
        let mut taken = Vec::with_capacity(records.len());
        let mut named = BTreeSet::new();
        for row in records {
            let Some(record) = record_row(row) else {
                return Self::Unreadable;
            };
            if !named.insert((record.entry.clone(), record.key.clone())) {
                return Self::Unreadable;
            }
            taken.push(record);
        }
        Self::Read(taken)
    }

    /// Whether the record holds a record of `entry`.
    pub(crate) fn holds(&self, entry: &str) -> bool {
        match self {
            Self::Read(records) => records.iter().any(|record| record.entry == entry),
            Self::Absent | Self::Unreadable => false,
        }
    }
}

/// The text `field` of `row` holds, when it holds a text.
fn text<'a>(row: &'a GoalRecordsRow, field: &str) -> Option<&'a str> {
    match row.get(field) {
        Some(FactScalar::String(written)) => Some(written.as_str()),
        _ => None,
    }
}

/// Whether `row` is the header of `instance`'s record of this version, with exactly its fields.
fn is_header(row: &GoalRecordsRow, instance: &str) -> bool {
    row.len() == 3
        && text(row, FIELD_ROW) == Some(ROW_HEADER)
        && text(row, FIELD_SCHEMA_VERSION) == Some(GOAL_RECORDS_SCHEMA_VERSION)
        && text(row, FIELD_INSTANCE) == Some(instance)
}

/// Whether `key` has the form of a key of a table: 1 to 128 bytes without a control character.
fn record_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_GOAL_TABLE_ID_BYTES && !key.chars().any(char::is_control)
}

/// Whether `to` is the text of a JSON object within the bound.
fn record_to(to: &str) -> bool {
    to.len() <= MAX_GOAL_RECORD_TO_BYTES
        && serde_json::from_str::<Value>(to).is_ok_and(|value| value.is_object())
}

/// `row` as a record row, or `None` when it is not one of exactly its fields.
fn record_row(row: &GoalRecordsRow) -> Option<StoredRecordRow> {
    if text(row, FIELD_ROW) != Some(ROW_RECORD) {
        return None;
    }
    let entry = text(row, FIELD_ENTRY).filter(|entry| goal_name(entry))?;
    let key = text(row, FIELD_KEY).filter(|key| record_key(key))?;
    let to = text(row, FIELD_TO).filter(|to| record_to(to))?;
    if !matches!(row.get(FIELD_SET_AT), Some(FactScalar::Integer(at)) if *at >= 0) {
        return None;
    }
    let channel = text(row, FIELD_CHANNEL)?;
    if !CHANNELS.iter().any(|known| known.as_str() == channel) {
        return None;
    }
    let fields = match row.get(FIELD_INITIATOR) {
        None => RECORD_FIELDS,
        Some(FactScalar::String(word)) if Initiator::from_word(word).is_some() => RECORD_FIELDS + 1,
        Some(_) => return None,
    };
    (row.len() == fields).then(|| StoredRecordRow {
        entry: entry.to_owned(),
        key: key.to_owned(),
        to: to.to_owned(),
        row: row.clone(),
    })
}

/// Why the stored record cannot take an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordsRefusal {
    /// The stored record is no record the reader reads.
    Unreadable,
    /// The edited record would hold more records than [`MAX_INSTANCE_GOAL_RECORDS`].
    Full,
    /// The composed record is no record the reader reads back: an internal fault; nothing is
    /// written, and the host refuses the one request.
    Inconsistent,
}

/// The record an edit composed: its rows, and its identity (the lowercase hex SHA-256 of its
/// version, its instance and its records' entries, keys and `to` texts, in their order), which a
/// replay is told by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposedGoalRecords {
    pub(crate) rows: Vec<GoalRecordsRow>,
    pub(crate) identity: String,
}

/// The record `change` writes over `instance`'s stored record `stored`, the rows it writes set by
/// `setter`: the records of the change's entry replaced where the first of them stood, else added
/// after the others (a `set` of none and a `clear` remove them), every other record kept as it is
/// stored. More than [`MAX_INSTANCE_GOAL_RECORDS`] records is `Full`; a stored record the reader
/// does not read is `Unreadable`; the composed record is read back before it is answered.
pub(crate) fn compose(
    stored: &StoredGoalRecords,
    instance: &str,
    change: &RecordsChange,
    setter: TargetSetter,
) -> Result<ComposedGoalRecords, RecordsRefusal> {
    let kept: &[StoredRecordRow] = match stored {
        StoredGoalRecords::Absent => &[],
        StoredGoalRecords::Read(records) => records,
        // Fix round 1 (the review): a clear rewrites a record the reader does not read as the
        // header alone, the one way out of it; a set of it is refused, as before.
        StoredGoalRecords::Unreadable if matches!(change, RecordsChange::Clear { .. }) => &[],
        StoredGoalRecords::Unreadable => return Err(RecordsRefusal::Unreadable),
    };
    let entry = match change {
        RecordsChange::Set { entry, .. } | RecordsChange::Clear { entry } => entry.as_str(),
    };
    let given: &[GivenRecord] = match change {
        RecordsChange::Set { records, .. } => records,
        RecordsChange::Clear { .. } => &[],
    };
    let others = kept.iter().filter(|record| record.entry != entry).count();
    if others + given.len() > MAX_INSTANCE_GOAL_RECORDS {
        return Err(RecordsRefusal::Full);
    }
    let fresh = given
        .iter()
        .map(|record| StoredRecordRow {
            entry: entry.to_owned(),
            key: record.key.clone(),
            to: record.to.clone(),
            row: record_of(entry, record, setter),
        })
        .collect::<Vec<_>>();
    let mut records = Vec::with_capacity(others + fresh.len());
    let mut placed = false;
    for record in kept {
        if record.entry == entry {
            if !placed {
                records.extend(fresh.iter().cloned());
                placed = true;
            }
            continue;
        }
        records.push(record.clone());
    }
    if !placed {
        records.extend(fresh);
    }
    let identity = serde_json::to_vec(&(
        GOAL_RECORDS_SCHEMA_VERSION,
        instance,
        records
            .iter()
            .map(|record| (&record.entry, &record.key, &record.to))
            .collect::<Vec<_>>(),
    ))
    .map_err(|_| RecordsRefusal::Inconsistent)?;
    let mut rows = Vec::with_capacity(records.len() + 1);
    rows.push(header(instance));
    rows.extend(records.into_iter().map(|record| record.row));
    if !matches!(
        StoredGoalRecords::read(Some(rows.as_slice()), instance),
        StoredGoalRecords::Read(back) if back.len() + 1 == rows.len()
    ) {
        return Err(RecordsRefusal::Inconsistent);
    }
    Ok(ComposedGoalRecords {
        rows,
        identity: format!("{:x}", Sha256::digest(identity)),
    })
}

/// The header row of `instance`'s record.
fn header(instance: &str) -> GoalRecordsRow {
    BTreeMap::from([
        (
            FIELD_ROW.to_owned(),
            FactScalar::String(ROW_HEADER.to_owned()),
        ),
        (
            FIELD_SCHEMA_VERSION.to_owned(),
            FactScalar::String(GOAL_RECORDS_SCHEMA_VERSION.to_owned()),
        ),
        (
            FIELD_INSTANCE.to_owned(),
            FactScalar::String(instance.to_owned()),
        ),
    ])
}

/// The row of `record` of `entry`, set by `setter`: the Runtime's clock at the request, the
/// request's channel and, when its connection declared one, the initiator.
fn record_of(entry: &str, record: &GivenRecord, setter: TargetSetter) -> GoalRecordsRow {
    let mut row = BTreeMap::from([
        (
            FIELD_ROW.to_owned(),
            FactScalar::String(ROW_RECORD.to_owned()),
        ),
        (FIELD_ENTRY.to_owned(), FactScalar::String(entry.to_owned())),
        (FIELD_KEY.to_owned(), FactScalar::String(record.key.clone())),
        (FIELD_TO.to_owned(), FactScalar::String(record.to.clone())),
        (
            FIELD_SET_AT.to_owned(),
            FactScalar::Integer(i64::try_from(setter.set_unix_ms).unwrap_or(i64::MAX)),
        ),
        (
            FIELD_CHANNEL.to_owned(),
            FactScalar::String(setter.channel.as_str().to_owned()),
        ),
    ]);
    if let Some(initiator) = setter.initiator {
        row.insert(
            FIELD_INITIATOR.to_owned(),
            FactScalar::String(initiator.as_str().to_owned()),
        );
    }
    row
}
