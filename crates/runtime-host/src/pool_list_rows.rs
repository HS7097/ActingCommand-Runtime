// SPDX-License-Identifier: AGPL-3.0-only

//! #455 P2 (register §1.505; `contracts/scheduling/README.md`, "A pool read from rows of a list";
//! the coordinator's rulings 1 to 4 of 11-22): the reading of a pool whose observation is
//! `list_rows`. Before each evaluation, under the fact write gate the fact pools are projected
//! under, the host reads the pool's instance's list fact and computes the reading with one pure
//! function ([`list_rows_reading`]); the evaluator is given it
//! ([`actingcommand_policy::GivenPoolReadings`]) and reads no list. The list itself enters neither
//! the projection nor the evaluation's facts.
//!
//! - **The sum.** For each declared row, the list's row whose key column holds the row's key
//!   gives its value column, a non-negative integer, times the row's factor; the reading is the
//!   sum. A key the list does not hold counts 0: a list is written whole or not at all, so a key
//!   it does not hold is not on the screen it was read from.
//! - **Unknown, never zero.** No list: `fact_missing`; a confidence below the pool's `ledger_fact`
//!   minimum: `fact_low_confidence`; a named row's value that is not a non-negative integer, or a
//!   sum beyond 2^53 − 1: `type_mismatch`. A list past its expiry is the evaluation's to judge
//!   (`fact_expired`), by the expiry given with the reading, as are a timeline reset's
//!   invalidation and a number a constant capacity cannot hold.
//! - **Freshness is the list's**: its observation time, its expiry, its confidence.

use actingcommand_contract::{FactContent, FactRecord, FactScalar, FactValue};
use actingcommand_policy::{
    CompiledCatalog, GivenPoolReading, GivenPoolReadings, GivenReadingState, ListRowsUnknown,
    ObservationRef, PoolListRow, PoolValueSource, ScopeSelector,
};

/// The largest reading a pool read from rows of a list holds: the canonical safe integer.
pub(crate) const MAX_LIST_ROWS_READING: u64 = 9_007_199_254_740_991;

/// The readings of every pool of `catalog` read from rows of a list, each from its instance's
/// list fact as `list_of` (instance alias, key) answers it. A pool the compile admits is of an
/// instance's scope and reads a `ledger_fact`; every such pool is given a reading.
pub(crate) fn pool_list_rows_readings<'a>(
    catalog: &CompiledCatalog,
    mut list_of: impl FnMut(&str, &str) -> Option<&'a FactRecord>,
) -> GivenPoolReadings {
    let mut readings = GivenPoolReadings::default();
    for pool in &catalog.catalog().pools.pools {
        let ObservationRef::ListRows {
            fact_key,
            key_column,
            value_column,
            rows,
        } = &pool.observation
        else {
            continue;
        };
        let ScopeSelector::Instance { instance_id } = &pool.scope else {
            continue;
        };
        let PoolValueSource::LedgerFact {
            minimum_confidence_milli,
        } = pool.value_source
        else {
            continue;
        };
        let state = list_rows_reading(
            key_column,
            value_column,
            rows,
            minimum_confidence_milli,
            list_of(instance_id.as_str(), fact_key.as_str()),
        );
        readings.give(
            pool.id.clone(),
            GivenPoolReading {
                instance_id: instance_id.clone(),
                fact_key: fact_key.clone(),
                state,
            },
        );
    }
    readings
}

/// The reading of a pool read from `rows` of `list` (its instance's list fact under the pool's
/// key, `None` when there is none) by `key_column` and `value_column`, with at least
/// `minimum_confidence_milli` (module doc). Pure.
pub(crate) fn list_rows_reading(
    key_column: &str,
    value_column: &str,
    rows: &[PoolListRow],
    minimum_confidence_milli: u16,
    list: Option<&FactRecord>,
) -> GivenReadingState {
    let unknown = GivenReadingState::Unknown;
    let Some(list) = list else {
        return unknown(ListRowsUnknown::FactMissing);
    };
    if list.confidence_milli < minimum_confidence_milli {
        return unknown(ListRowsUnknown::FactLowConfidence);
    }
    let FactContent::Inline {
        value: FactValue::RecordList(listed),
    } = &list.content
    else {
        return unknown(ListRowsUnknown::TypeMismatch);
    };
    let mut sum = 0_u128;
    for row in rows {
        // A key the whole list does not hold counts 0.
        let Some(found) = listed.iter().find(|entry| {
            matches!(
                entry.get(key_column),
                Some(FactScalar::String(key)) if *key == row.key
            )
        }) else {
            continue;
        };
        let Some(FactScalar::Integer(value)) = found.get(value_column) else {
            return unknown(ListRowsUnknown::TypeMismatch);
        };
        let Ok(value) = u64::try_from(*value) else {
            return unknown(ListRowsUnknown::TypeMismatch);
        };
        // A u64 times a u64 fits a u128, and the sum stops at the bound before it grows further.
        sum += u128::from(value) * u128::from(row.factor);
        if sum > u128::from(MAX_LIST_ROWS_READING) {
            return unknown(ListRowsUnknown::TypeMismatch);
        }
    }
    match u64::try_from(sum) {
        Ok(value) => GivenReadingState::Read {
            value,
            observed_at_unix_ms: list.observed_at_unix_ms,
            expires_at_unix_ms: list.expires_at_unix_ms,
        },
        Err(_) => unknown(ListRowsUnknown::TypeMismatch),
    }
}
