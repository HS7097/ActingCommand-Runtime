// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #428 S168-5a (contracts/selection-policy.md, "Table maximum over a list"; register
//! §1.291): the value source `table_max_over_list`, the largest integer of one table column over
//! the union of the keys that one to sixteen record lists hold.
//!
//! The value does not depend on the candidate, so an evaluation reads each distinct reference
//! once, after the up-front check of the record lists and before any candidate ([`read_all`]).
//! One read ([`read_list_maximum`]) runs four steps: the lists, the pins, each key of the union
//! in ascending byte order, the value. A listed list that is not usable, a pin that stays
//! unavailable, a key no row of the table has and a null cell of the class each make the read
//! Unknown: a missing value is never read as 0 and never skipped. Only a key the table has and
//! places outside the class adds nothing. Like the rest of the crate, it reads no file and no
//! clock: the lists come from the snapshot and the table from the bound tables.

use std::collections::BTreeSet;

use crate::facts::{ScalarValue, SelectionFactSnapshot, UnknownReason};
use crate::schema::{ListSource, SelectionPolicy, ValueRef, ValueType};
use crate::tables::{SelectionTables, cell_value, pin_value};

/// What one read of one list maximum came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListMaxReading {
    /// The largest value of the class, the document's `empty`, or why the read is unknown.
    pub(crate) value: Result<ScalarValue, UnknownReason>,
    /// The number of distinct keys of the union. 0 when a listed list is unusable, which only a
    /// read of one reference alone shows: an evaluation's up-front check ends it first.
    pub(crate) count: usize,
    /// The smallest key, in byte order, that holds the largest value; none for `empty` and for
    /// an unknown read.
    pub(crate) maximum_identity: Option<String>,
    /// The columns of the pins that used their `otherwise`, in pin order.
    pub(crate) otherwise_columns: Vec<String>,
}

impl ListMaxReading {
    fn unknown(reason: UnknownReason, count: usize, otherwise_columns: Vec<String>) -> Self {
        Self {
            value: Err(reason),
            count,
            maximum_identity: None,
            otherwise_columns,
        }
    }
}

/// One distinct list maximum of a document, read once.
pub(crate) struct ListMaxRead<'a> {
    pub(crate) reference: &'a ValueRef,
    /// The gate or term that reads the reference first.
    pub(crate) rule_id: &'a str,
    pub(crate) reading: ListMaxReading,
}

/// Reads every distinct list maximum of `policy` once, in the order of its first read (the gates
/// in order, each predicate depth first and its left side before its right, then the terms).
pub(crate) fn read_all<'a>(
    policy: &'a SelectionPolicy,
    tables: &SelectionTables,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Vec<ListMaxRead<'a>> {
    policy
        .list_maxima()
        .into_iter()
        .map(|(reference, rule_id)| ListMaxRead {
            reference,
            rule_id,
            reading: read_list_maximum(policy, reference, tables, facts, now_unix_ms),
        })
        .collect()
}

/// Step 1: the keys of the union of `lists`, each list's rows after its own `where`, each key
/// once. A list that is not usable (missing, expired, stale, below its floor, of another type or
/// malformed, two kept rows of it sharing one key included, as the up-front check reads a list)
/// is the `Err`, the first such list in the order of `lists`; no later list is read.
pub(crate) fn union_keys(
    policy: &SelectionPolicy,
    lists: &[ListSource],
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<BTreeSet<String>, UnknownReason> {
    let mut keys = BTreeSet::new();
    for list in lists {
        let declaration = policy
            .facts
            .iter()
            .find(|declaration| declaration.fact_key == list.fact_key)
            .ok_or(UnknownReason::FactMissing)?;
        let ValueType::RecordList { key_column, .. } = &declaration.value_type else {
            return Err(UnknownReason::TypeMismatch);
        };
        let rows = facts.resolve_list(declaration, now_unix_ms)?;
        let mut listed = BTreeSet::new();
        for row in rows {
            if let Some(filter) = &list.row_filter
                && !matches!(row.get(&filter.column), Some(ScalarValue::String(value)) if *value == filter.equals)
            {
                continue;
            }
            let Some(ScalarValue::String(key)) = row.get(key_column) else {
                return Err(UnknownReason::TypeMismatch);
            };
            if !listed.insert(key.as_str()) {
                return Err(UnknownReason::TypeMismatch);
            }
        }
        keys.extend(listed.into_iter().map(str::to_owned));
    }
    Ok(keys)
}

/// One read of the list maximum `reference` (steps 1 to 4 of "One read"). It reads its lists
/// from `facts` itself, so a read of one reference alone, without the evaluation's up-front
/// check, shows step 1's own order, that of `lists`.
pub(crate) fn read_list_maximum(
    policy: &SelectionPolicy,
    reference: &ValueRef,
    tables: &SelectionTables,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> ListMaxReading {
    let ValueRef::TableMaxOverList {
        table_id,
        value_column,
        pins,
        lists,
        empty,
        ..
    } = reference
    else {
        return ListMaxReading::unknown(UnknownReason::TypeMismatch, 0, Vec::new());
    };
    // Step 1: the lists.
    let keys = match union_keys(policy, lists, facts, now_unix_ms) {
        Ok(keys) => keys,
        Err(reason) => return ListMaxReading::unknown(reason, 0, Vec::new()),
    };
    let count = keys.len();
    // Step 2: each pin's value; a pin still unavailable keeps the first fact's reason.
    let fact = |fact_key: &str| {
        policy
            .facts
            .iter()
            .find(|declaration| declaration.fact_key == fact_key)
            .ok_or(UnknownReason::FactMissing)
            .and_then(|declaration| facts.resolve(declaration, now_unix_ms).cloned())
    };
    let mut otherwise_columns = Vec::new();
    let mut pinned = Vec::with_capacity(pins.len());
    for pin in pins {
        let mut otherwise_column = None;
        match pin_value(pin, &fact, &mut otherwise_column) {
            Ok(value) => {
                pinned.push((pin.column.as_str(), value));
                otherwise_columns.extend(otherwise_column);
            }
            Err(reason) => return ListMaxReading::unknown(reason, count, otherwise_columns),
        }
    }
    // Step 3: each key of the union, in ascending byte order. A key no row has may hold the
    // largest value, and so may a null cell of the class: either is Unknown, whatever the other
    // keys hold. A key whose rows are all of another class adds nothing.
    let Some(table) = tables.loaded(table_id) else {
        return ListMaxReading::unknown(UnknownReason::TypeMismatch, count, otherwise_columns);
    };
    let mut largest: Option<(i64, &str)> = None;
    for key in &keys {
        if !table.has_key(key) {
            return ListMaxReading::unknown(UnknownReason::LookupMiss, count, otherwise_columns);
        }
        let Some(row) = table.row_of(key, &pinned) else {
            continue;
        };
        match table.cell(row, value_column).and_then(cell_value) {
            Some(ScalarValue::Integer(value)) => {
                // Ascending keys and a strict comparison keep the smallest key of the largest.
                if largest.is_none_or(|(best, _)| value > best) {
                    largest = Some((value, key.as_str()));
                }
            }
            Some(_) => {
                return ListMaxReading::unknown(
                    UnknownReason::TypeMismatch,
                    count,
                    otherwise_columns,
                );
            }
            None => {
                return ListMaxReading::unknown(
                    UnknownReason::LookupMiss,
                    count,
                    otherwise_columns,
                );
            }
        }
    }
    // Step 4: the largest cell of the class; with none, the document's `empty`.
    match largest {
        Some((value, key)) => ListMaxReading {
            value: Ok(ScalarValue::Integer(value)),
            count,
            maximum_identity: Some(key.to_owned()),
            otherwise_columns,
        },
        None => ListMaxReading {
            value: Ok(empty.clone()),
            count,
            maximum_identity: None,
            otherwise_columns,
        },
    }
}
