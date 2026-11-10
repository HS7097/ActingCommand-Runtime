// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-2 (model M-357a v1.1 §3, §5; register §1.65 TB6, TB7, TB9): the shared tables
//! one selection document reads, as the evaluator and the upper bound read them.
//!
//! This crate still reads no file. [`LoadedTable::load`] takes a table its caller has read and
//! checked (the contract crate's `parse_standard_pack_table`) and fixes it at one instant: a
//! column past its `valid_until_unix_s` reads null from then on, and every row is indexed by its
//! key once. [`SelectionTables::bind`] is the one constructor of what a decision reads: it checks
//! each loaded table against the document's declaration (K2) and refuses with one word of the
//! vocabulary `table_mismatch`. The host's run preparation, the Lab's dry runs and the resource
//! tools all call it, so K2 is written once.
//!
//! A table is read by one rule (§3.4): the key, the pins, the row, the cell, the declared value.
//! The rule lives here so the evaluator and the upper bound cannot disagree about a cell.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use actingcommand_contract::{
    StandardPackTable, StandardPackTableCell, StandardPackTableColumnType, TableMismatch,
};
use serde_json::Value;

use crate::facts::{Candidate, ScalarValue, UnknownReason};
use crate::schema::{
    ListSource, SelectionError, SelectionErrorCode, SelectionPolicy, TableDeclaration, TablePin,
    ValueRef, ValueType,
};

/// One table as a run holds it: fixed at the instant it was loaded, its rows indexed by key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTable {
    table: StandardPackTable,
    /// Each column's position in a row, by name.
    positions: BTreeMap<String, usize>,
    /// The rows of each key, in file order.
    by_key: BTreeMap<String, Vec<usize>>,
}

impl LoadedTable {
    /// Fixes `table` at `now_unix_ms`: every cell of a column whose `valid_until_unix_s` has
    /// passed reads null (§2.2, D25), and the rows are indexed by key once, here, so no
    /// evaluation walks the rows of another key.
    pub fn load(mut table: StandardPackTable, now_unix_ms: u64) -> Self {
        let positions: BTreeMap<String, usize> = table
            .columns
            .iter()
            .enumerate()
            .map(|(position, column)| (column.name.clone(), position))
            .collect();
        for column in table.expired_columns(now_unix_ms) {
            if let Some(&position) = positions.get(&column) {
                for row in &mut table.rows {
                    if let Some(cell) = row.get_mut(position) {
                        *cell = StandardPackTableCell::Null;
                    }
                }
            }
        }
        let key_position = table
            .row_key
            .first()
            .and_then(|column| positions.get(column).copied());
        let mut by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        if let Some(key_position) = key_position {
            for (index, row) in table.rows.iter().enumerate() {
                if let Some(StandardPackTableCell::Text(key)) = row.get(key_position) {
                    by_key.entry(key.clone()).or_default().push(index);
                }
            }
        }
        Self {
            table,
            positions,
            by_key,
        }
    }

    /// The table's id, its file's.
    pub fn table_id(&self) -> &str {
        &self.table.table_id
    }

    /// The table's key column values, ascending: what the resource tools' coverage check reads.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.by_key.keys().map(String::as_str)
    }

    pub(crate) fn cell(&self, row: usize, column: &str) -> Option<&StandardPackTableCell> {
        let position = *self.positions.get(column)?;
        self.table.rows.get(row)?.get(position)
    }

    /// Whether some row has the key (Workflow #428 S168-5a: a key of a list no row has is a miss).
    pub(crate) fn has_key(&self, key: &str) -> bool {
        self.by_key.contains_key(key)
    }

    /// The row of `key` whose pinned columns hold `pins`; the row key is unique, so at most one.
    pub(crate) fn row_of(&self, key: &str, pins: &[(&str, ScalarValue)]) -> Option<usize> {
        self.by_key.get(key)?.iter().copied().find(|&row| {
            pins.iter().all(|(column, value)| {
                self.cell(row, column)
                    .is_some_and(|cell| cell_holds(cell, value))
            })
        })
    }
}

fn cell_holds(cell: &StandardPackTableCell, value: &ScalarValue) -> bool {
    match (cell, value) {
        (StandardPackTableCell::Integer(cell), ScalarValue::Integer(value)) => cell == value,
        (StandardPackTableCell::Boolean(cell), ScalarValue::Boolean(value)) => cell == value,
        (StandardPackTableCell::Text(cell), ScalarValue::String(value)) => cell == value,
        _ => false,
    }
}

pub(crate) fn cell_value(cell: &StandardPackTableCell) -> Option<ScalarValue> {
    match cell {
        StandardPackTableCell::Null => None,
        StandardPackTableCell::Integer(value) => Some(ScalarValue::Integer(*value)),
        StandardPackTableCell::Boolean(value) => Some(ScalarValue::Boolean(*value)),
        StandardPackTableCell::Text(value) => Some(ScalarValue::String(value.clone())),
    }
}

fn cell_json(cell: &StandardPackTableCell) -> Value {
    match cell {
        StandardPackTableCell::Null => Value::Null,
        StandardPackTableCell::Integer(value) => Value::from(*value),
        StandardPackTableCell::Boolean(value) => Value::from(*value),
        StandardPackTableCell::Text(value) => Value::from(value.clone()),
    }
}

/// Why [`SelectionTables::bind`] refused a table: which one, and the word of `table_mismatch`,
/// with the column when one column is at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBindRefusal {
    pub table_id: String,
    /// The declared path, `tables/<table_id>.json`.
    pub path: String,
    pub mismatch: TableMismatch,
    pub column: Option<String>,
}

impl fmt::Display for TableBindRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "table_id={} table_mismatch={}",
            self.table_id, self.mismatch
        )?;
        if let Some(column) = &self.column {
            write!(formatter, " table_column={column}")?;
        }
        Ok(())
    }
}

/// One table bound to one document: the loaded table and the declaration it was checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundTable {
    loaded: Arc<LoadedTable>,
    declaration: TableDeclaration,
}

/// The tables one evaluation reads, each checked against its document's declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionTables {
    tables: BTreeMap<String, BoundTable>,
}

static NO_TABLES: SelectionTables = SelectionTables {
    tables: BTreeMap::new(),
};

impl SelectionTables {
    /// No table at all: what a document that declares none reads, and what a declared table is
    /// unbound under (`dangling_reference`).
    pub fn none() -> &'static Self {
        &NO_TABLES
    }

    /// The one constructor (D15, K2): each table of `tables` that `policy` declares, compared
    /// with its declaration. The file's `row_key` equals the declared one, item by item; every
    /// declared column is in the file with the same type; a declared `allowed` is a subset of
    /// the file's. A table the document does not declare is ignored; a declared table not given
    /// stays unbound, and the evaluator refuses it with `dangling_reference`.
    pub fn bind(
        policy: &SelectionPolicy,
        tables: &[Arc<LoadedTable>],
    ) -> Result<Self, TableBindRefusal> {
        let mut bound = BTreeMap::new();
        for declaration in &policy.tables {
            let Some(loaded) = tables
                .iter()
                .find(|table| table.table_id() == declaration.table_id)
            else {
                continue;
            };
            check_declaration(declaration, loaded)?;
            bound.insert(
                declaration.table_id.clone(),
                BoundTable {
                    loaded: Arc::clone(loaded),
                    declaration: declaration.clone(),
                },
            );
        }
        Ok(Self { tables: bound })
    }

    /// The loaded table bound under `table_id` (Workflow #428 S168-5a: a list maximum reads it).
    pub(crate) fn loaded(&self, table_id: &str) -> Option<&LoadedTable> {
        self.tables.get(table_id).map(|bound| bound.loaded.as_ref())
    }

    /// `dangling_reference` for a table `policy` declares and nothing bound (D15).
    pub(crate) fn check_bound(&self, policy: &SelectionPolicy) -> Result<(), SelectionError> {
        match policy
            .tables
            .iter()
            .find(|declaration| !self.tables.contains_key(&declaration.table_id))
        {
            Some(declaration) => Err(SelectionError::new(
                SelectionErrorCode::DanglingReference,
                format!("table_id={} unbound", declaration.table_id),
            )),
            None => Ok(()),
        }
    }

    /// Reads one `table` reference for the candidate key `key` (§3.4). `fact` resolves a pinned
    /// fact in the pinned snapshot.
    pub(crate) fn read(
        &self,
        reference: &ValueRef,
        key: &str,
        fact: &dyn Fn(&str) -> Result<ScalarValue, UnknownReason>,
    ) -> TableReading {
        let ValueRef::Table {
            table_id,
            pins,
            value_column,
            fallback_column,
            absent,
            ..
        } = reference
        else {
            return TableReading::unknown(UnknownReason::TypeMismatch, false);
        };
        let Some(bound) = self.tables.get(table_id) else {
            return TableReading::unknown(UnknownReason::TypeMismatch, false);
        };
        let mut otherwise_column = None;
        let mut pinned = Vec::with_capacity(pins.len());
        for pin in pins {
            match pin_value(pin, fact, &mut otherwise_column) {
                Ok(value) => pinned.push((pin.column.as_str(), value)),
                // A pinned fact is instance data: it may refresh, so this is no static drop.
                Err(reason) => return TableReading::unknown(reason, false),
            }
        }
        let loaded = &bound.loaded;
        if !loaded.by_key.contains_key(key) {
            return TableReading {
                otherwise_column,
                ..TableReading::unknown(UnknownReason::LookupMiss, true)
            };
        }
        let Some(row) = loaded.row_of(key, &pinned) else {
            // The key has rows and none is of this class: a known value on a class gate. A read
            // that declares no `absent` stands behind one, so reaching it here means the class
            // gate let the candidate through; nothing can be read, and it is no table gap.
            return match absent {
                Some(absent) => TableReading {
                    otherwise_column,
                    ..TableReading::known(absent.clone())
                },
                None => TableReading {
                    otherwise_column,
                    ..TableReading::unknown(UnknownReason::LookupMiss, false)
                },
            };
        };
        let mut used_fallback = false;
        let mut read_column = value_column;
        let mut cell = loaded.cell(row, value_column);
        if matches!(cell, None | Some(StandardPackTableCell::Null))
            && let Some(fallback) = fallback_column
        {
            let other = loaded.cell(row, fallback);
            if !matches!(other, None | Some(StandardPackTableCell::Null)) {
                used_fallback = true;
                read_column = fallback;
                cell = other;
            }
        }
        let Some(value) = cell.and_then(cell_value) else {
            return TableReading {
                otherwise_column,
                ..TableReading::unknown(UnknownReason::LookupMiss, true)
            };
        };
        // A value the table allows and the document did not declare: the table is newer than the
        // document, and no transform's default may swallow it (§3.4 step 5).
        let declared = bound.declaration.columns.get(read_column);
        if !declared.is_some_and(|declared| value.matches(declared)) {
            return TableReading {
                otherwise_column,
                ..TableReading::unknown(UnknownReason::TypeMismatch, true)
            };
        }
        TableReading {
            used_fallback,
            otherwise_column,
            ..TableReading::known(value)
        }
    }

    /// The rows the evaluation's references read for `candidates`, for the input identity (§3.7):
    /// by `table_id`, each row an object of its key column and its declared columns, ordered by
    /// the row key's projection, each row once. `key_of` gives a candidate's key the way a read
    /// takes it, so a candidate whose key is unknown reads no row. Workflow #428 S168-5a: a list
    /// maximum adds the row of each key of its union (`union_of` gives it when every listed list
    /// is usable) that holds its pins, whether or not the row is of the largest value.
    pub(crate) fn read_rows(
        &self,
        policy: &SelectionPolicy,
        candidates: &[Candidate],
        key_of: &dyn Fn(&str, &Candidate) -> Option<String>,
        fact: &dyn Fn(&str) -> Result<ScalarValue, UnknownReason>,
        union_of: &dyn Fn(&[ListSource]) -> Option<BTreeSet<String>>,
    ) -> BTreeMap<String, Vec<BTreeMap<String, Value>>> {
        let mut read: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
        for reference in policy.value_refs() {
            if let ValueRef::TableMaxOverList {
                table_id,
                pins,
                lists,
                ..
            } = reference
            {
                let Some(bound) = self.tables.get(table_id) else {
                    continue;
                };
                let mut otherwise_column = None;
                let pinned = pins
                    .iter()
                    .map(|pin| {
                        pin_value(pin, fact, &mut otherwise_column)
                            .map(|value| (pin.column.as_str(), value))
                    })
                    .collect::<Result<Vec<_>, _>>();
                let (Ok(pinned), Some(keys)) = (pinned, union_of(lists.as_slice())) else {
                    continue;
                };
                for key in &keys {
                    if let Some(row) = bound.loaded.row_of(key, &pinned) {
                        read.entry(table_id.as_str()).or_default().insert(row);
                    }
                }
                continue;
            }
            let ValueRef::Table {
                table_id,
                key_field,
                pins,
                ..
            } = reference
            else {
                continue;
            };
            let Some(bound) = self.tables.get(table_id) else {
                continue;
            };
            let mut otherwise_column = None;
            let pinned = pins
                .iter()
                .map(|pin| {
                    pin_value(pin, fact, &mut otherwise_column)
                        .map(|value| (pin.column.as_str(), value))
                })
                .collect::<Result<Vec<_>, _>>();
            let Ok(pinned) = pinned else {
                continue;
            };
            for candidate in candidates {
                if let Some(row) =
                    key_of(key_field, candidate).and_then(|key| bound.loaded.row_of(&key, &pinned))
                {
                    read.entry(table_id.as_str()).or_default().insert(row);
                }
            }
        }
        policy
            .tables
            .iter()
            .filter_map(|declaration| {
                let bound = self.tables.get(&declaration.table_id)?;
                let mut rows = read
                    .get(declaration.table_id.as_str())
                    .map(|rows| rows.iter().copied().collect::<Vec<_>>())
                    .unwrap_or_default();
                rows.sort_by_key(|&row| projection(bound, row));
                let objects = rows
                    .into_iter()
                    .map(|row| row_object(bound, row))
                    .collect::<Vec<_>>();
                Some((declaration.table_id.clone(), objects))
            })
            .collect()
    }
}

/// A row's cells under the row key, the order of the input identity's rows.
fn projection(bound: &BoundTable, row: usize) -> Vec<StandardPackTableCell> {
    bound
        .declaration
        .row_key
        .iter()
        .map(|column| {
            bound
                .loaded
                .cell(row, column)
                .cloned()
                .unwrap_or(StandardPackTableCell::Null)
        })
        .collect()
}

/// A row's object in the input identity: its key column and every declared column; an expired
/// column is the null this run read.
fn row_object(bound: &BoundTable, row: usize) -> BTreeMap<String, Value> {
    bound
        .declaration
        .row_key
        .first()
        .into_iter()
        .chain(bound.declaration.columns.keys())
        .map(|column| {
            let value = bound
                .loaded
                .cell(row, column)
                .map_or(Value::Null, cell_json);
            (column.clone(), value)
        })
        .collect()
}

/// A pin's value (§3.4 step 2): the literal, the fact, or the fact's `otherwise`, which is noted.
/// Still unavailable, the read keeps the first fact's reason. A list maximum's pins are read the
/// same way (Workflow #428 S168-5a, step 2).
pub(crate) fn pin_value(
    pin: &TablePin,
    fact: &dyn Fn(&str) -> Result<ScalarValue, UnknownReason>,
    otherwise_column: &mut Option<String>,
) -> Result<ScalarValue, UnknownReason> {
    if let Some(literal) = &pin.equals {
        return Ok(literal.clone());
    }
    let Some(fact_key) = &pin.equals_fact else {
        return Err(UnknownReason::TypeMismatch);
    };
    let reason = match fact(fact_key) {
        Ok(value) => return Ok(value),
        Err(reason) => reason,
    };
    let other = match &pin.otherwise {
        Some(otherwise) => match (&otherwise.equals, &otherwise.equals_fact) {
            (Some(literal), _) => Ok(literal.clone()),
            (None, Some(other_key)) => fact(other_key),
            (None, None) => Err(reason),
        },
        None => Err(reason),
    };
    match other {
        Ok(value) => {
            otherwise_column.get_or_insert_with(|| pin.column.clone());
            Ok(value)
        }
        Err(_) => Err(reason),
    }
}

/// K2 for one table.
fn check_declaration(
    declaration: &TableDeclaration,
    loaded: &LoadedTable,
) -> Result<(), TableBindRefusal> {
    let refuse = |mismatch: TableMismatch, column: Option<&str>| TableBindRefusal {
        table_id: declaration.table_id.clone(),
        path: declaration.path.clone(),
        mismatch,
        column: column.map(str::to_owned),
    };
    if loaded.table.row_key != declaration.row_key {
        return Err(refuse(TableMismatch::RowKey, None));
    }
    for (name, declared) in &declaration.columns {
        let Some(column) = loaded
            .table
            .columns
            .iter()
            .find(|column| &column.name == name)
        else {
            return Err(refuse(TableMismatch::ColumnAbsent, Some(name.as_str())));
        };
        match (declared, &column.column_type) {
            (ValueType::Integer, StandardPackTableColumnType::Integer { .. })
            | (ValueType::Boolean, StandardPackTableColumnType::Boolean) => {}
            (
                ValueType::EnumString { allowed },
                StandardPackTableColumnType::EnumString { allowed: file },
            ) => {
                if !allowed.iter().all(|member| file.contains(member)) {
                    return Err(refuse(TableMismatch::AllowedNotSubset, Some(name.as_str())));
                }
            }
            _ => return Err(refuse(TableMismatch::ColumnType, Some(name.as_str()))),
        }
    }
    Ok(())
}

/// What one read of one `table` reference came to, with the two marks only the evaluator's own
/// reasons and verdicts use (§5).
pub(crate) struct TableReading {
    pub(crate) value: Result<ScalarValue, UnknownReason>,
    /// The value is the `fallback_column`'s.
    pub(crate) used_fallback: bool,
    /// A pin used its `otherwise`: the first such column.
    pub(crate) otherwise_column: Option<String>,
    /// The Unknown is the table's own static data (§3.4 steps 3 to 5): a key no row holds, a null
    /// cell or an undeclared value. Reading again cannot change it (D21).
    pub(crate) static_unknown: bool,
}

impl TableReading {
    fn known(value: ScalarValue) -> Self {
        Self {
            value: Ok(value),
            used_fallback: false,
            otherwise_column: None,
            static_unknown: false,
        }
    }

    fn unknown(reason: UnknownReason, static_unknown: bool) -> Self {
        Self {
            value: Err(reason),
            used_fallback: false,
            otherwise_column: None,
            static_unknown,
        }
    }
}
