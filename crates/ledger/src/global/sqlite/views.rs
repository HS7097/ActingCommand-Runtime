// SPDX-License-Identifier: AGPL-3.0-only

use super::super::projection::{LAB_ANCHOR_SOURCE, LAB_ANCHOR_TYPE, LAB_RELATIONS, LabAxis};
use super::*;
use actingcommand_contract::{EventSeverity, LedgerView, LedgerViewDefinition};
use serde::Serialize;

// Ledger revision 3 (Workflow #378 A3): a record's severity is its category, so `success` is
// stored too; the list keeps the category order.
const SEVERITIES: [EventSeverity; 6] = [
    EventSeverity::Debug,
    EventSeverity::Success,
    EventSeverity::Info,
    EventSeverity::Warning,
    EventSeverity::Error,
    EventSeverity::Fatal,
];
const FAMILY: &str = "json_extract(CAST(canonical_record AS TEXT),'$.payload.family')";

pub(super) fn key(value: impl Serialize) -> GlobalLedgerResult<String> {
    match serde_json::to_value(value).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::EventSerializationFailed.code(),
            LedgerLocation::EncodeLedgerQuery.location(),
            &error,
        )
    })? {
        Value::String(value) => Ok(value),
        _ => Err(failure(
            LedgerCode::InvalidEventQueryValue.code(),
            LedgerLocation::EncodeLedgerQuery.location(),
        )),
    }
}

/// LN-5 (#422): the stored severities within `minimum` and `maximum`, for a binding.
pub(super) fn severities(
    minimum: Option<EventSeverity>,
    maximum: Option<EventSeverity>,
) -> GlobalLedgerResult<Vec<SqlValue>> {
    SEVERITIES
        .into_iter()
        .filter(|severity| {
            minimum.is_none_or(|minimum| *severity >= minimum)
                && maximum.is_none_or(|maximum| *severity <= maximum)
        })
        .map(|severity| key(severity).map(SqlValue::Text))
        .collect()
}

// Only closed, product-owned enum values enter DDL. Query values use bindings below.
fn literal(value: impl Serialize) -> GlobalLedgerResult<String> {
    Ok(format!("'{}'", key(value)?.replace('\'', "''")))
}

fn name(view: LedgerView) -> GlobalLedgerResult<String> {
    Ok(format!("ledger_view_{}_v1", key(view)?))
}

fn select(view: LedgerView) -> GlobalLedgerResult<String> {
    let Some(predicate) = predicate(view)? else {
        return lab_select();
    };
    Ok(format!(
        "SELECT e.*,e.sequence AS available_from_sequence FROM ledger_events e WHERE {predicate}"
    ))
}

/// LN-6 (#422, LEAN-L §3.3): the condition the stored view of `view` puts on a `ledger_events`
/// row, over its own columns; `None` for the Lab view, whose condition is a relation.
pub(super) fn predicate(view: LedgerView) -> GlobalLedgerResult<Option<String>> {
    Ok(Some(match view.definition() {
        LedgerViewDefinition::All => "1".to_owned(),
        LedgerViewDefinition::Types { families, events } => {
            let families = families
                .iter()
                .map(literal)
                .collect::<GlobalLedgerResult<Vec<_>>>()?;
            let events = events
                .iter()
                .map(literal)
                .collect::<GlobalLedgerResult<Vec<_>>>()?;
            format!(
                "({FAMILY} IN ({}) OR event_type IN ({}))",
                families.join(","),
                events.join(",")
            )
        }
        LedgerViewDefinition::MinimumSeverity(minimum) => {
            let values = SEVERITIES
                .into_iter()
                .filter(|severity| *severity >= minimum)
                .map(literal)
                .collect::<GlobalLedgerResult<Vec<_>>>()?;
            format!("severity IN ({})", values.join(","))
        }
        LedgerViewDefinition::LabContext => return Ok(None),
    }))
}

fn lab_select() -> GlobalLedgerResult<String> {
    let anchor = format!(
        "(a.origin_source={} OR a.event_type={})",
        literal(LAB_ANCHOR_SOURCE)?,
        literal(LAB_ANCHOR_TYPE)?
    );
    let mut paths = vec![format!(
        "SELECT e.sequence AS since WHERE e.origin_source={} OR e.event_type={}",
        literal(LAB_ANCHOR_SOURCE)?,
        literal(LAB_ANCHOR_TYPE)?
    )];
    for (axis, via_run) in LAB_RELATIONS {
        let column = match axis {
            LabAxis::Request => "request_id",
            LabAxis::Correlation => "correlation_id",
        };
        paths.push(if via_run {
            format!("SELECT MIN(MAX(e.sequence,a.sequence,r.sequence)) AS since FROM ledger_links r JOIN ledger_links al ON al.{column}=r.{column} JOIN ledger_events a ON a.sequence=al.sequence WHERE r.run_id=l.run_id AND {anchor}")
        } else {
            format!("SELECT MIN(MAX(e.sequence,a.sequence)) AS since FROM ledger_links al JOIN ledger_events a ON a.sequence=al.sequence WHERE al.{column}=l.{column} AND {anchor}")
        });
    }
    Ok(format!(
        "SELECT * FROM (SELECT e.*,(SELECT MIN(since) FROM ({})) AS available_from_sequence FROM ledger_events e JOIN ledger_links l ON l.sequence=e.sequence) WHERE available_from_sequence IS NOT NULL",
        paths.join(" UNION ALL ")
    ))
}

/// Versioned derived objects leave the authenticated v1 fact format unchanged.
fn definitions() -> GlobalLedgerResult<Vec<(String, String)>> {
    let mut definitions = Vec::new();
    for view in LedgerView::ALL {
        let name = name(view)?;
        definitions.push((
            name.clone(),
            format!("CREATE VIEW {name} AS {}", select(view)?),
        ));
    }
    for (suffix, column) in [
        ("type", "event_type"),
        ("module", "origin_module"),
        ("source", "origin_source"),
        ("time", "timestamp_unix_ms"),
        ("severity", "severity"),
    ] {
        let name = format!("ledger_view_index_{suffix}_v1");
        definitions.push((
            name.clone(),
            format!("CREATE INDEX {name} ON ledger_events({column},sequence)"),
        ));
    }
    definitions.sort();
    Ok(definitions)
}

pub(super) fn installed(connection: &Connection) -> GlobalLedgerResult<bool> {
    let mut statement = connection
        .prepare("SELECT name,sql FROM sqlite_schema WHERE name GLOB 'ledger_view_*' ORDER BY name")
        .map_err(|error| sql_error(error, LedgerLocation::InspectLedgerViews.location()))?;
    let actual = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| sql_error(error, ContractLocation::ReadLedgerViews.location()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error(error, ContractLocation::ReadLedgerViews.location()))?;
    if actual.is_empty() {
        return Ok(false);
    }
    if actual != definitions()? {
        return Err(failure(
            LedgerCode::LedgerViewSchemaMismatch.code(),
            LedgerLocation::VerifyLedgerViews.location(),
        ));
    }
    Ok(true)
}

pub(super) fn deploy(connection: &Connection) -> GlobalLedgerResult<()> {
    if !installed(connection)? {
        for (_, sql) in definitions()? {
            connection
                .execute_batch(&sql)
                .map_err(|error| sql_error(error, LedgerLocation::CreateLedgerViews.location()))?;
        }
    }
    Ok(())
}
