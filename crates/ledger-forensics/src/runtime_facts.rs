// SPDX-License-Identifier: AGPL-3.0-only

use super::{ForensicError, ForensicResult, map_ledger_error, query_view_page};
use crate::codes::{ForensicsCode, ForensicsLocation};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Location, Outcome};
use actingcommand_contract::{
    EventPayload, EventQuery, EventType, LedgerReadScope, MAX_RUNTIME_EVENT_QUERY_EVENTS,
    MAX_RUNTIME_FACTS, OriginModule, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RUNTIME_FACT_SCHEMA_VERSION, RuntimeEventQueryPageRequest, RuntimeFactRecord, RuntimeFactScope,
    RuntimeFactSnapshot, RuntimePayload, newest_complete_runtime_fact_snapshot_set,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerEvidenceConfig, GlobalLedgerMetadata, LedgerCode, LedgerIoKind,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

const OPERATION: Location = ForensicsLocation::ReadRuntimeFactsAt.location();

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
    /// Reading or replaying failed; no partial store is returned. `outcome` is the failure: the
    /// relayed ledger or forensic code with its `operation` (ledger revision 3, Workflow #378
    /// A3; was `code` and `operation`).
    Failed {
        position: u64,
        outcome: Outcome,
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

// Workflow #395 (LEDG-395): the phases of one offline `actingledger facts --at` read.
/// The phases of one offline runtime fact read, in order (Workflow #395).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForensicFactsPhase {
    /// Opens the read-only metadata snapshot and reads the event at the position, and the
    /// latest time through it (#411 A24).
    Position,
    /// Finds the newest complete runtime fact snapshot set at or before the position.
    SnapshotSet,
    /// Applies the runtime fact events after that set through the position.
    Changes,
}

impl ForensicFactsPhase {
    /// The phase's registered location.
    pub const fn location(self) -> Location {
        match self {
            Self::Position => ForensicsLocation::ReadRuntimeFactsPosition.location(),
            Self::SnapshotSet => ForensicsLocation::ReadRuntimeFactSnapshotSet.location(),
            Self::Changes => ForensicsLocation::ReadRuntimeFactChanges.location(),
        }
    }
}

/// `runtime_facts_at` with the budget of each phase taken from `budget` (Workflow #395). A
/// phase's deadline is its budget from the moment the phase starts, and the read-only snapshot
/// serves each later phase under that phase's deadline, so a slow phase leaves the next one its
/// whole budget. A deadline that runs out names its phase as the failure's operation.
pub fn runtime_facts_at_per_phase(
    state_root: impl AsRef<Path>,
    position: u64,
    mut budget: impl FnMut(ForensicFactsPhase) -> Duration,
) -> ForensicRuntimeFactsResult {
    facts_at(state_root.as_ref(), position, |phase| {
        Instant::now() + budget(phase)
    })
}

/// Replays the runtime fact store at `position` (inclusive) from one read-only metadata
/// snapshot: the newest complete `runtime.fact_snapshot` set at or before it (or an empty
/// store), then every `runtime.fact_recorded` / `runtime.fact_invalidated` after its last
/// part in ledger order, under the same acceptance rules as the Runtime's startup replay.
/// Any refusal fails. `deadline` bounds all three phases together; when it runs out, the
/// failure names the phase it ran out in (Workflow #395).
pub fn runtime_facts_at(
    state_root: impl AsRef<Path>,
    position: u64,
    deadline: Instant,
) -> ForensicRuntimeFactsResult {
    facts_at(state_root.as_ref(), position, |_| deadline)
}

/// Workflow #395: the one implementation behind both entries; `deadline_of` gives a phase its
/// deadline when the phase starts. A deadline that runs out (the ledger's
/// `ledger_read_budget_exceeded` or this reader's `runtime_facts_read_budget_exceeded`) names
/// its phase as the failure's operation.
fn facts_at(
    state_root: &Path,
    position: u64,
    mut deadline_of: impl FnMut(ForensicFactsPhase) -> Instant,
) -> ForensicRuntimeFactsResult {
    let mut phase = ForensicFactsPhase::Position;
    read(state_root, position, &mut phase, &mut deadline_of).unwrap_or_else(|mut error| {
        let deadline_ran_out = error.code == LedgerCode::LedgerReadBudgetExceeded.as_str()
            || error.code == ForensicsCode::RuntimeFactsReadBudgetExceeded.as_str();
        // Workflow #395: a deadline that ran out names its phase as the outcome's operation.
        if deadline_ran_out {
            error.operation = phase.location().as_str();
        }
        ForensicRuntimeFactsResult::Failed {
            position,
            outcome: error.outcome(),
            detail: error.detail,
            io_kind: error.io_kind,
        }
    })
}

fn read(
    state_root: &Path,
    position: u64,
    phase: &mut ForensicFactsPhase,
    deadline_of: &mut impl FnMut(ForensicFactsPhase) -> Instant,
) -> ForensicResult<ForensicRuntimeFactsResult> {
    if state_root.as_os_str().is_empty() {
        return Err(facts_error(
            ForensicsCode::InvalidStateRoot.code(),
            "state root is empty",
        ));
    }
    if position == 0 {
        return Err(facts_error(
            SharedCode::RuntimeFactLedgerPositionInvalid.code(),
            "ledger positions start at sequence 1",
        ));
    }
    *phase = ForensicFactsPhase::Position;
    let deadline = deadline_of(*phase);
    let mut snapshot = GlobalLedger::open_metadata(
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
    let mut position_unix_ms = None;
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
            position_unix_ms = Some(event.timestamp_unix_ms);
            Ok(())
        },
    )?;
    let position_unix_ms = position_unix_ms.ok_or_else(|| {
        facts_error(
            ForensicsCode::RuntimeFactsPositionMissing.code(),
            "no event at the position",
        )
    })?;
    // #411 A24: a record's time is its producer's, not monotonic in sequence (a run's diagnostic
    // `artifact.created` carries the run's start and is appended at its end), so the snapshot is
    // taken at the latest time through the position, the maximum timestamp: the events through
    // it stamped after the position's own are read too. The time bound filters; the scan ends
    // only at the position.
    let later_query = EventQuery {
        to_sequence: Some(position),
        from_timestamp_unix_ms: Some(position_unix_ms.saturating_add(1)),
        ..EventQuery::default()
    };
    let mut taken_at_unix_ms = position_unix_ms;
    visit(
        &snapshot,
        &later_query,
        ProjectionProfile::Cli,
        position,
        deadline,
        |event| {
            taken_at_unix_ms = taken_at_unix_ms.min(event.timestamp_unix_ms);
            Ok(())
        },
    )?;
    *phase = ForensicFactsPhase::SnapshotSet;
    let deadline = deadline_of(*phase);
    snapshot.set_read_deadline(deadline);
    let mut sealed = Vec::new();
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
            sealed.push(event.sequence);
            Ok(())
        },
    )?;
    // Workflow #308 5d-1: the newest complete snapshot set, its parts read newest first.
    let search = newest_complete_runtime_fact_snapshot_set(|| {
        sealed
            .pop()
            .map(|sequence| snapshot_part(&snapshot, sequence, position, deadline))
            .transpose()
    })?;
    let mut store = BTreeMap::new();
    let mut from_sequence = 1;
    if let Some(set) = search.set {
        for (_, part) in &set {
            for record in &part.records {
                store.insert((record.scope.clone(), record.key.clone()), record.clone());
            }
        }
        if let Some((last, _)) = set.last() {
            from_sequence = last.saturating_add(1);
        }
    }
    *phase = ForensicFactsPhase::Changes;
    let deadline = deadline_of(*phase);
    snapshot.set_read_deadline(deadline);
    let facts_query = EventQuery {
        from_sequence: Some(from_sequence),
        origin_module: Some(OriginModule::RuntimeFacts),
        ..EventQuery::default()
    };
    let source = visit(
        &snapshot,
        &facts_query,
        ProjectionProfile::Forensic,
        position,
        deadline,
        |event| apply(&mut store, event),
    )?;
    Ok(ForensicRuntimeFactsResult::Available {
        source,
        position,
        facts: RuntimeFactSnapshot {
            schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
            ledger_position: position,
            taken_at_unix_ms,
            snapshot_id: position,
            part: 1,
            parts: 1,
            records: store.into_values().collect(),
        },
    })
}

/// The validated snapshot part at `sequence`, read with its full payload.
fn snapshot_part(
    snapshot: &GlobalLedgerMetadata,
    sequence: u64,
    position: u64,
    deadline: Instant,
) -> ForensicResult<(u64, RuntimeFactSnapshot)> {
    let refused = |detail: &str| {
        ForensicError::new(
            SharedCode::RuntimeFactReplayFailed.code(),
            OPERATION,
            format!("sequence {sequence}: {detail}"),
        )
    };
    let query = EventQuery {
        from_sequence: Some(sequence),
        to_sequence: Some(sequence),
        event_type: Some(EventType::RuntimeFactSnapshot),
        ..EventQuery::default()
    };
    let mut part = None;
    visit(
        snapshot,
        &query,
        ProjectionProfile::Forensic,
        position,
        deadline,
        |event| {
            let ProjectionPayload::Full(payload) = &event.payload else {
                return Err(refused("payload is not the full projection"));
            };
            let EventPayload::Runtime(RuntimePayload::FactSnapshot(payload)) = payload.as_ref()
            else {
                return Err(refused("payload is not runtime.fact_snapshot"));
            };
            part = Some(payload.snapshot().clone());
            Ok(())
        },
    )?;
    let part = part.ok_or_else(|| refused("snapshot part is not readable"))?;
    part.validate()
        .map_err(|error| refused(&format!("runtime fact rejected: {}", error.code())))?;
    Ok((sequence, part))
}

/// One event after the snapshot set, applied exactly as `RuntimeFactStore::record` /
/// `invalidate`; a part of an incomplete set passed over changes nothing.
fn apply(
    store: &mut BTreeMap<(RuntimeFactScope, String), RuntimeFactRecord>,
    event: &ProjectedEvent,
) -> ForensicResult<()> {
    let refused = |detail: &str| {
        ForensicError::new(
            SharedCode::RuntimeFactReplayFailed.code(),
            OPERATION,
            format!("sequence {}: {detail}", event.sequence),
        )
    };
    let ProjectionPayload::Full(payload) = &event.payload else {
        return Err(refused("payload is not the full projection"));
    };
    match payload.as_ref() {
        EventPayload::Runtime(RuntimePayload::FactSnapshot(_)) => {}
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
                ForensicsCode::RuntimeFactsReadBudgetExceeded.code(),
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
                    ForensicsCode::RuntimeFactsSourceIncomplete.code(),
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

fn facts_error(code: Code, detail: &str) -> ForensicError {
    ForensicError::new(code, OPERATION, detail)
}
