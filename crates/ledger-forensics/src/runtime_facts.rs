// SPDX-License-Identifier: AGPL-3.0-only

use super::{ForensicError, ForensicResult, map_ledger_error, query_view_page};
use actingcommand_contract::{
    EventPayload, EventQuery, EventType, LedgerReadScope, MAX_RUNTIME_EVENT_QUERY_EVENTS,
    MAX_RUNTIME_FACTS, OriginModule, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RUNTIME_FACT_SCHEMA_VERSION, RuntimeEventQueryPageRequest, RuntimeFactRecord, RuntimeFactScope,
    RuntimeFactSnapshot, RuntimePayload,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerEvidenceConfig, GlobalLedgerMetadata, LedgerIoKind,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

const OPERATION: &str = "read_runtime_facts_at";

/// The offline face of `RuntimeOperation::RuntimeFactSnapshot` at one ledger position.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[must_use = "inspect availability and failure before consuming the facts"]
pub enum ForensicRuntimeFactsResult {
    /// The store once every event through `position` was applied.
    Available {
        source: LedgerReadScope,
        position: u64,
        facts: RuntimeFactSnapshot,
    },
    NotAvailable {
        position: u64,
        latest_sequence: u64,
        reason: ForensicRuntimeFactsUnavailable,
    },
    /// Reading or replaying failed; no partial store is returned.
    Failed {
        position: u64,
        code: &'static str,
        operation: &'static str,
        detail: String,
        /// Present only when the failure came from a `std::io::Error`.
        #[serde(skip_serializing_if = "Option::is_none")]
        io_kind: Option<LedgerIoKind>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForensicRuntimeFactsUnavailable {
    /// The opened ledger holds no event.
    LedgerEmpty,
    /// The position lies beyond the opened ledger's last sequence.
    PositionBeyondSnapshot,
}

/// Replays the runtime fact store at `position` (inclusive) from one read-only metadata
/// snapshot: the latest `runtime.fact_snapshot` at or before it (or an empty store), then
/// every `runtime.fact_recorded` / `runtime.fact_invalidated` after it in ledger order,
/// under the same acceptance rules as the Runtime's startup replay. Any refusal fails.
pub fn runtime_facts_at(
    state_root: impl AsRef<Path>,
    position: u64,
    deadline: Instant,
) -> ForensicRuntimeFactsResult {
    read(state_root.as_ref(), position, deadline).unwrap_or_else(|error| {
        ForensicRuntimeFactsResult::Failed {
            position,
            code: error.code,
            operation: error.operation,
            detail: error.detail,
            io_kind: error.io_kind,
        }
    })
}

fn read(
    state_root: &Path,
    position: u64,
    deadline: Instant,
) -> ForensicResult<ForensicRuntimeFactsResult> {
    if state_root.as_os_str().is_empty() {
        return Err(facts_error("invalid_state_root", "state root is empty"));
    }
    if position == 0 {
        return Err(facts_error(
            "runtime_fact_ledger_position_invalid",
            "ledger positions start at sequence 1",
        ));
    }
    let snapshot = GlobalLedger::open_metadata(
        GlobalLedgerEvidenceConfig::new(state_root).with_deadline(deadline),
    )
    .map_err(map_ledger_error)?;
    let latest_sequence = snapshot.latest_sequence();
    if position > latest_sequence {
        return Ok(ForensicRuntimeFactsResult::NotAvailable {
            position,
            latest_sequence,
            reason: if latest_sequence == 0 {
                ForensicRuntimeFactsUnavailable::LedgerEmpty
            } else {
                ForensicRuntimeFactsUnavailable::PositionBeyondSnapshot
            },
        });
    }
    let mut taken_at_unix_ms = None;
    let position_query = EventQuery {
        from_sequence: Some(position),
        to_sequence: Some(position),
        ..EventQuery::default()
    };
    visit(
        &snapshot,
        &position_query,
        ProjectionProfile::Cli,
        position,
        deadline,
        |event| {
            taken_at_unix_ms = Some(event.timestamp_unix_ms);
            Ok(())
        },
    )?;
    let taken_at_unix_ms = taken_at_unix_ms
        .ok_or_else(|| facts_error("runtime_facts_position_missing", "no event at the position"))?;
    let mut sealed = None;
    let snapshot_query = EventQuery {
        event_type: Some(EventType::RuntimeFactSnapshot),
        ..EventQuery::default()
    };
    visit(
        &snapshot,
        &snapshot_query,
        ProjectionProfile::Cli,
        position,
        deadline,
        |event| {
            sealed = Some(event.sequence);
            Ok(())
        },
    )?;
    let mut store = BTreeMap::new();
    let facts_query = EventQuery {
        from_sequence: Some(sealed.unwrap_or(1)),
        origin_module: Some(OriginModule::RuntimeFacts),
        ..EventQuery::default()
    };
    let source = visit(
        &snapshot,
        &facts_query,
        ProjectionProfile::Forensic,
        position,
        deadline,
        |event| apply(&mut store, event, sealed),
    )?;
    Ok(ForensicRuntimeFactsResult::Available {
        source,
        position,
        facts: RuntimeFactSnapshot {
            schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
            ledger_position: position,
            taken_at_unix_ms,
            records: store.into_values().collect(),
        },
    })
}

/// One event, applied exactly as `RuntimeFactStore::replay` / `record` / `invalidate`.
fn apply(
    store: &mut BTreeMap<(RuntimeFactScope, String), RuntimeFactRecord>,
    event: &ProjectedEvent,
    sealed: Option<u64>,
) -> ForensicResult<()> {
    let refused = |detail: &str| {
        ForensicError::new(
            "runtime_fact_replay_failed",
            OPERATION,
            format!("sequence {}: {detail}", event.sequence),
        )
    };
    let ProjectionPayload::Full(payload) = &event.payload else {
        return Err(refused("payload is not the full projection"));
    };
    match payload.as_ref() {
        EventPayload::Runtime(RuntimePayload::FactSnapshot(payload))
            if sealed == Some(event.sequence) =>
        {
            let snapshot = payload.snapshot();
            snapshot
                .validate()
                .map_err(|error| refused(&format!("runtime fact rejected: {}", error.code())))?;
            store.clear();
            for record in &snapshot.records {
                store.insert((record.scope.clone(), record.key.clone()), record.clone());
            }
        }
        EventPayload::Runtime(RuntimePayload::FactRecorded(payload)) => {
            let record = payload.record();
            record
                .validate()
                .map_err(|error| refused(&format!("runtime fact rejected: {}", error.code())))?;
            let key = (record.scope.clone(), record.key.clone());
            match store.get(&key) {
                Some(existing) if existing == record => {}
                Some(existing) if existing.observed_at_unix_ms >= record.observed_at_unix_ms => {
                    return Err(refused(&format!(
                        "runtime fact is older than the stored observation at {}",
                        existing.observed_at_unix_ms
                    )));
                }
                None if store.len() >= MAX_RUNTIME_FACTS => {
                    return Err(refused(&format!(
                        "runtime fact store holds {MAX_RUNTIME_FACTS} records"
                    )));
                }
                _ => {
                    store.insert(key, record.clone());
                }
            }
        }
        EventPayload::Runtime(RuntimePayload::FactInvalidated(payload)) => {
            let invalidation = payload.invalidation();
            store
                .remove(&(invalidation.scope.clone(), invalidation.key.clone()))
                .ok_or_else(|| refused("runtime fact is not present"))?;
        }
        _ => {
            return Err(refused(
                "unexpected event under origin module runtime-facts",
            ));
        }
    }
    Ok(())
}

/// Pages one query at the fixed snapshot position through the `views` page read and
/// returns the final page's read scope. An incomplete source or an expired deadline fails.
fn visit(
    snapshot: &GlobalLedgerMetadata,
    query: &EventQuery,
    profile: ProjectionProfile,
    position: u64,
    deadline: Instant,
    mut each: impl FnMut(&ProjectedEvent) -> ForensicResult<()>,
) -> ForensicResult<LedgerReadScope> {
    let invalid_page = |error: actingcommand_contract::RuntimeContractError| {
        ForensicError::new(error.code(), OPERATION, "invalid runtime fact page request")
    };
    let mut request = RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, None)
        .and_then(|request| request.at_snapshot(position))
        .map_err(invalid_page)?;
    loop {
        if Instant::now() >= deadline {
            return Err(facts_error(
                "runtime_facts_read_budget_exceeded",
                "cooperative runtime fact read deadline expired",
            ));
        }
        let page = query_view_page(snapshot, query, profile, &request)?;
        let scope = page
            .read_scope()
            .filter(|scope| scope.read_complete)
            .cloned()
            .ok_or_else(|| {
                facts_error(
                    "runtime_facts_source_incomplete",
                    "runtime fact events are not completely readable through the position",
                )
            })?;
        page.events().iter().try_for_each(&mut each)?;
        match page.next_cursor() {
            Some(cursor) => {
                request = RuntimeEventQueryPageRequest::new(
                    MAX_RUNTIME_EVENT_QUERY_EVENTS,
                    Some(cursor.clone()),
                )
                .map_err(invalid_page)?;
            }
            None => return Ok(scope),
        }
    }
}

fn facts_error(code: &'static str, detail: &str) -> ForensicError {
    ForensicError::new(code, OPERATION, detail)
}
