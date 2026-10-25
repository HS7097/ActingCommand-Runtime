// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #422 LN-2 (`contracts/ledger-observer.md` §11.5 step 1): the `state` rows of the
//! index table under key prefixes, each with the stored hash of the record its sequence names.

use super::super::GlobalLedgerResult;
use super::super::tagged::StateIndexRow;
use super::{decode, sql_error};
use crate::codes::LedgerLocation;
use actingcommand_runtime_database::RuntimeDatabase;

pub(super) fn state_index_rows(
    database: &RuntimeDatabase,
    prefixes: &[String],
) -> GlobalLedgerResult<Vec<StateIndexRow>> {
    let operation = LedgerLocation::LedgerIndexRows.location();
    let connection = database.connection(operation)?;
    let mut statement = connection
        .prepare(
            "SELECT i.sequence, i.key, i.event_id, i.value_sha256, e.record_sha256 FROM ledger_index i LEFT JOIN ledger_events e ON e.sequence = i.sequence WHERE i.role = 'state' AND substr(i.key, 1, length(?1)) = ?1",
        )
        .map_err(|error| sql_error(error, operation))?;
    let mut rows = Vec::new();
    for prefix in prefixes {
        let selected = statement
            .query_map(rusqlite::params![prefix], |row| {
                Ok(StateIndexRow {
                    sequence: decode(row.get(0)?),
                    key: row.get(1)?,
                    event_id: row.get(2)?,
                    value_sha256: row.get(3)?,
                    record_sha256: row.get(4)?,
                })
            })
            .map_err(|error| sql_error(error, operation))?;
        for row in selected {
            rows.push(row.map_err(|error| sql_error(error, operation))?);
        }
    }
    Ok(rows)
}
