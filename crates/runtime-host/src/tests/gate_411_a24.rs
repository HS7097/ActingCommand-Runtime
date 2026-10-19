// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A24 (host key 125, register §1.139 A24-1): a ledger record's `timestamp_unix_ms` is its
//! producer's capture or occurrence time, not its append time, so it is not monotonic in
//! `sequence`: a run's diagnostic `artifact.created` carries the run's start and is appended at
//! its end. `actingd suspended` judges its clock slots against the ledger's latest time, the
//! maximum timestamp, not the time of the last record by sequence
//! (`contracts/policy-suspension.md`, `clock_slots`; `contracts/ledger-observer.md` §8.1).

use super::clock_slot_report::{ledger_events, record_sequence, report};
use super::gate_372k2::{SLOT_TASK, at, slot_sources, slot_task};
use super::*;
use crate::codes::HostCode;
use crate::policy_host::clock_slot::{SlotRecord, SlotValues, clock_slot_signal};
use actingcommand_contract::{
    ArtifactPayloadDraft, AuditInput, EventDraft, EventLinksDraft, EventOrigin,
};
use actingcommand_ledger::{LedgerMaintenance, Sha256SecretFingerprinter};
use actingcommand_runtime_database::MaintenanceLimits;
use serde_json::json;

/// The State seed of `config` and the offline writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"runtime-host-test-salt";
const WRITER: &str = "a24-offline-writer";

/// Appends to the closed state root `root` one `artifact.created` stamped `at_unix_ms`, as the
/// artifact store records a run's diagnostic stream: stamped with the run's start, appended when
/// the run ends.
fn append_diagnostic_created(root: &TempDir, at_unix_ms: u64) {
    let database = Arc::new(
        RuntimeStateStore::open_database(root.path(), STATE_KEY).expect("the state database"),
    );
    let limits = MaintenanceLimits::default();
    let ledger = LedgerMaintenance::acquire(
        root.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("the ledger's maintenance owner")
    .open_writer(database, WRITER.to_owned())
    .expect("the offline writer");
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let draft = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        at_unix_ms,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::System,
            OriginModule::ArtifactStore,
            EventActor::System,
        ),
        EventLinksDraft::default(),
        ArtifactPayloadDraft::created(AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(WRITER.as_bytes()).expect("fingerprinter"))
    .expect("the artifact.created draft");
    ledger.append(draft).expect("append the artifact.created");
    ledger.close().expect("close the offline writer");
}

/// A deferred record of the 10:00 slot, whose next occurrence is 10:00 the next day, is settled
/// by the ledger's latest time, 10:30 the next day, when the Runtime stopped. A run that started
/// at 10:05 the first day is sealed last: its diagnostic `artifact.created`, stamped 10:05, is the
/// last record by sequence. The slot is `collapsed`, settled, not `pending`.
#[test]
fn a24_suspension_reference_time_is_the_latest_timestamp() {
    let sources = slot_sources(1, &[slot_task(&[600], 1)], None);
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(at(1, 10, 0), 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("start the host");
    host.activate_policy_catalog(&sources)
        .expect("activate the catalog");
    let deferred = clock_slot_signal(
        SlotRecord::Deferred,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        &SlotValues {
            gate_code: Some(HostCode::DispatchPausedInstance.code()),
            occurrence_unix_ms: at(1, 10, 0),
            window_end_unix_ms: Some(at(1, 23, 30)),
            next_occurrence_unix_ms: Some(at(2, 10, 0)),
            ..SlotValues::default()
        },
        at(1, 10, 0),
    );
    let signal_id = deferred.signal_id.clone();
    host.record_policy_planning_signal(deferred)
        .expect("record the deferred slot");
    // The Runtime runs on past the next occurrence and stops at 10:30 the next day.
    clock.set_unix_ms(at(2, 10, 30));
    host.close().expect("close the host");
    append_diagnostic_created(&root, at(1, 10, 5));

    let events = ledger_events(&root);
    let last = events
        .iter()
        .max_by_key(|event| event.sequence())
        .expect("a ledger record");
    assert_eq!(
        (last.event_type(), last.timestamp_unix_ms()),
        (EventType::ArtifactCreated, at(1, 10, 5)),
        "the last record by sequence is the diagnostic artifact.created, stamped with its run's start"
    );
    assert_eq!(
        events.iter().map(PersistedEvent::timestamp_unix_ms).max(),
        Some(at(2, 10, 30)),
        "the ledger's latest time is the stop"
    );

    let report = report(&root, &sources);
    let sequence = record_sequence(&events, &signal_id);
    let entries = report["clock_slots"]
        .as_array()
        .unwrap_or_else(|| panic!("clock_slots is a list: {report:#}"));
    let entry = entries
        .iter()
        .find(|entry| entry["recorded_sequence"] == json!(sequence))
        .unwrap_or_else(|| panic!("the deferred slot is listed: {report:#}"));
    assert_eq!(
        (&entry["slot_fate"], &entry["slot_fate_settled"]),
        (&json!("collapsed"), &json!(true)),
        "the slot is judged against the latest time: {entry:#}"
    );
}
