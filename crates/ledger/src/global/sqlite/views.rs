// SPDX-License-Identifier: AGPL-3.0-only

use super::super::projection::{LAB_ANCHOR_SOURCE, LAB_ANCHOR_TYPE, LAB_RELATIONS, LabAxis};
use super::*;
use actingcommand_contract::{EventSeverity, LedgerView, LedgerViewDefinition};
use serde::Serialize;

const SEVERITIES: [EventSeverity; 5] = [
    EventSeverity::Debug,
    EventSeverity::Info,
    EventSeverity::Warning,
    EventSeverity::Error,
    EventSeverity::Fatal,
];
const FAMILY: &str = "json_extract(CAST(canonical_record AS TEXT),'$.payload.family')";

fn key(value: impl Serialize) -> GlobalLedgerResult<String> {
    match serde_json::to_value(value).map_err(|error| {
        GlobalLedgerError::json("event_serialization_failed", "encode_ledger_query", &error)
    })? {
        Value::String(value) => Ok(value),
        _ => Err(failure("invalid_event_query_value", "encode_ledger_query")),
    }
}

// Only closed, product-owned enum values enter DDL. Query values use bindings below.
fn literal(value: impl Serialize) -> GlobalLedgerResult<String> {
    Ok(format!("'{}'", key(value)?.replace('\'', "''")))
}

fn name(view: LedgerView) -> GlobalLedgerResult<String> {
    Ok(format!("ledger_view_{}_v1", key(view)?))
}

fn select(view: LedgerView) -> GlobalLedgerResult<String> {
    let predicate = match view.definition() {
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
        LedgerViewDefinition::LabContext => return lab_select(),
    };
    Ok(format!(
        "SELECT e.*,e.sequence AS available_from_sequence FROM ledger_events e WHERE {predicate}"
    ))
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
        .map_err(|error| sql_error(error, "inspect_ledger_views"))?;
    let actual = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| sql_error(error, "read_ledger_views"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error(error, "read_ledger_views"))?;
    if actual.is_empty() {
        return Ok(false);
    }
    if actual != definitions()? {
        return Err(failure(
            "ledger_view_schema_mismatch",
            "verify_ledger_views",
        ));
    }
    Ok(true)
}

pub(super) fn deploy(connection: &Connection) -> GlobalLedgerResult<()> {
    if !installed(connection)? {
        for (_, sql) in definitions()? {
            connection
                .execute_batch(&sql)
                .map_err(|error| sql_error(error, "create_ledger_views"))?;
        }
    }
    Ok(())
}

pub(super) fn select_sequences(
    connection: &Connection,
    events: &[LedgerEventMetadata],
    query: &EventQuery,
    after: u64,
    snapshot: u64,
    limit: usize,
    budget: ReadBudget,
) -> GlobalLedgerResult<Vec<u64>> {
    let view = query.view.unwrap_or(LedgerView::Events);
    let view_name = name(view)?;
    // Supported pre-view offline roots remain read-only and use the same definition.
    let prefix = if installed(connection)? {
        String::new()
    } else {
        format!("WITH {view_name} AS ({}) ", select(view)?)
    };
    let mut sql = format!(
        "{prefix}SELECT e.sequence FROM {view_name} e JOIN ledger_links l ON l.sequence=e.sequence WHERE e.sequence>? AND e.sequence<=? AND e.available_from_sequence<=?"
    );
    let mut values = vec![
        SqlValue::Integer(encode(after)),
        SqlValue::Integer(encode(snapshot)),
        SqlValue::Integer(encode(snapshot)),
    ];
    for (clause, value) in [
        ("e.sequence>=?", query.from_sequence),
        ("e.sequence<=?", query.to_sequence),
        ("e.timestamp_unix_ms>=?", query.from_timestamp_unix_ms),
        ("e.timestamp_unix_ms<?", query.to_timestamp_unix_ms),
    ] {
        if let Some(value) = value {
            sql.push_str(&format!(" AND {clause}"));
            values.push(SqlValue::Integer(encode(value)));
        }
    }
    let query_value = serde_json::to_value(query).map_err(|error| {
        GlobalLedgerError::json("event_serialization_failed", "encode_ledger_query", &error)
    })?;
    for (field, column) in [
        ("event_type", "e.event_type"),
        ("source", "e.origin_source"),
        ("origin_module", "e.origin_module"),
        ("instance_id", "l.instance_id"),
        ("request_id", "l.request_id"),
        ("correlation_id", "l.correlation_id"),
        ("causation_id", "l.causation_id"),
        ("task_id", "l.task_id"),
        ("run_id", "l.run_id"),
        ("lease_id", "l.lease_id"),
        ("frame_id", "l.frame_id"),
        ("action_id", "l.action_id"),
        ("recognition_id", "l.recognition_id"),
    ] {
        if let Some(value) = query_value.get(field).filter(|value| !value.is_null()) {
            let Some(value) = value.as_str() else {
                return Err(failure("invalid_event_query_value", "encode_ledger_query"));
            };
            sql.push_str(&format!(" AND {column}=?"));
            values.push(SqlValue::Text(value.to_owned()));
        }
    }
    if query.minimum_severity.is_some() || query.maximum_severity.is_some() {
        let allowed: Vec<_> = SEVERITIES
            .into_iter()
            .filter(|severity| {
                query
                    .minimum_severity
                    .is_none_or(|minimum| *severity >= minimum)
                    && query
                        .maximum_severity
                        .is_none_or(|maximum| *severity <= maximum)
            })
            .collect();
        sql.push_str(&format!(
            " AND e.severity IN ({})",
            vec!["?"; allowed.len()].join(",")
        ));
        values.extend(
            allowed
                .into_iter()
                .map(|severity| SqlValue::Text(severity.as_str().to_owned())),
        );
    }
    if let Some(code) = query.diagnostic_code {
        // DiagnosticCode is a typed payload projection, including computed semantic codes.
        let matching: Vec<_> = events
            .iter()
            .filter(|event| event.payload().diagnostic_code() == Some(code))
            .map(|event| encode(event.sequence()))
            .collect();
        let matching = serde_json::to_string(&matching).map_err(|error| {
            GlobalLedgerError::json("event_serialization_failed", "encode_ledger_query", &error)
        })?;
        sql.push_str(" AND e.sequence IN (SELECT value FROM json_each(?))");
        values.push(SqlValue::Text(matching));
    }
    sql.push_str(" ORDER BY e.sequence LIMIT ?");
    values.push(SqlValue::Integer(i64::try_from(limit).map_err(|_| {
        failure("invalid_page_limit", "encode_ledger_query")
    })?));
    check_read_budget(budget, 0, events.len())?;
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| sql_error(error, "prepare_ledger_view_query"))?;
    let mut rows = statement
        .query(params_from_iter(values.iter()))
        .map_err(|error| sql_error(error, "query_ledger_view"))?;
    let mut sequences = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| sql_error(error, "read_ledger_view"))?
    {
        check_read_budget(budget, 0, events.len())?;
        sequences
            .push(decode(row.get(0).map_err(|error| {
                sql_error(error, "read_ledger_view_sequence")
            })?));
    }
    check_read_budget(budget, 0, events.len())?;
    Ok(sequences)
}
