// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A24 (ledger key 50, register §1.139 A24-4): a ledger record's `timestamp_unix_ms` is its
//! producer's capture or occurrence time, not its append time, so it is not monotonic in
//! `sequence`: a run's diagnostic `artifact.created` carries the run's start and is appended at
//! its end. The offline `actingledger facts --at <position>` takes its snapshot at the latest time
//! through the position, the maximum timestamp of the events at or before it, not the time of the
//! event at it (`contracts/runtime-fact-store.md`, "Offline read"). The root is a formal SQLite
//! root (`missing_material.rs`) with its writer closed, as an offline reader finds it.

use super::missing_material::formal_root;
use crate::global::Sha256SecretFingerprinter;
use actingcommand_contract::{
    ArtifactPayloadDraft, AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventSeverity, EventSource, IdentifierIssuer, MonitorPayloadDraft,
    OriginModule, SanitizedEventDraft,
};
use actingcommand_ledger_forensics::{ForensicRuntimeFactsResult, runtime_facts_at_per_phase};
use std::time::Duration;

/// A run's start: its diagnostic `artifact.created` carries it.
const RUN_START_MS: u64 = 1_752_147_200_000;
/// The run's end, 329 s later: the record before the diagnostic one.
const RUN_END_MS: u64 = RUN_START_MS + 329_000;
/// A record after the position, later than both.
const AFTER_MS: u64 = RUN_END_MS + 60_000;
/// Each phase's budget: far more than this small root needs.
const ENOUGH: Duration = Duration::from_secs(60);

fn stamped(
    ids: &IdentifierIssuer,
    at_unix_ms: u64,
    payload: EventPayloadDraft,
) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        at_unix_ms,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"a24-forensic-fixture").expect("salt"))
    .expect("a stamped draft")
}

/// A record of the run's end at `RUN_END_MS`, then the run's diagnostic `artifact.created`
/// stamped `RUN_START_MS`, then a record at `AFTER_MS`. At the diagnostic record's position the
/// snapshot is taken at `RUN_END_MS`: the latest time through it, not the position's own time
/// and not the later record's.
#[test]
fn a24_forensic_taken_at_is_the_latest_timestamp() {
    let directory = tempfile::tempdir().expect("state root");
    let (database, ledger) = formal_root(directory.path());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    ledger
        .append(stamped(
            &ids,
            RUN_END_MS,
            MonitorPayloadDraft::requested(AuditInput::new()).into(),
        ))
        .expect("the run's end record");
    let diagnostic = ledger
        .append(stamped(
            &ids,
            RUN_START_MS,
            ArtifactPayloadDraft::created(AuditInput::new()).into(),
        ))
        .expect("the run's diagnostic record, stamped with its start");
    ledger
        .append(stamped(
            &ids,
            AFTER_MS,
            MonitorPayloadDraft::requested(AuditInput::new()).into(),
        ))
        .expect("a later record");
    ledger.close().expect("close the writer");
    drop(database);

    let position = diagnostic.sequence();
    let result = runtime_facts_at_per_phase(directory.path(), position, |_| ENOUGH);
    let ForensicRuntimeFactsResult::Available { facts, .. } = &result else {
        panic!("the facts at {position} are available: {result:?}");
    };
    assert_eq!(
        facts.taken_at_unix_ms, RUN_END_MS,
        "the snapshot is taken at the latest time through the position"
    );
}
