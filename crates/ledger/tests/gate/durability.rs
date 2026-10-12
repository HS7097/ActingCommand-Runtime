// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2, L-1 G7 (inventory-ledger key 3), on a SQLite candidate ledger:
//! `append_durable` refuses an observational draft, and an observational append leaves the
//! shared connection restored to `FULL` (`contracts/ledger-store.md`, "Durability classes").

use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventSeverity, EventSource,
    IdentifierIssuer, MonitorPayloadDraft, OriginModule, SanitizedEventDraft,
};
use actingcommand_ledger::critical::EventAppender;
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerError, Sha256SecretFingerprinter,
};
use actingcommand_runtime_database::{DatabaseLocation, RuntimeDatabase};
use std::sync::Arc;
use tempfile::TempDir;

/// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;

fn database(root: &TempDir) -> Arc<RuntimeDatabase> {
    Arc::new(
        RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
            root.path(),
            b"gate-l1-durability-seed",
            |_| Ok(()),
            |_| Ok(()),
        )
        .expect("candidate physical owner"),
    )
}

fn open(root: &TempDir, database: &Arc<RuntimeDatabase>) -> GlobalLedger {
    GlobalLedger::open_sqlite_candidate(
        GlobalLedgerConfig::new(root.path(), "gate-owner"),
        Arc::clone(database),
    )
    .expect("SQLite candidate ledger")
}

/// `monitor.probe_requested`, which the contract lists as observational.
fn observational(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        MonitorPayloadDraft::requested(AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"gate-l1-durability").expect("salt"))
    .expect("observational draft")
}

#[test]
fn gate_append_durable_refuses_an_observational_draft() {
    let root = TempDir::new().expect("root");
    let ledger = open(&root, &database(&root));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let head = ledger.latest_sequence().expect("head");

    let refused = ledger
        .append_durable(observational(&ids))
        .expect_err("an observational draft is refused");

    assert_eq!(refused.code(), "critical_event_not_durable");
    assert!(refused.is_fatal(), "{refused:?}");
    assert_eq!(ledger.latest_sequence().expect("head"), head);
}

#[test]
fn gate_observational_append_restores_full_sync() {
    let root = TempDir::new().expect("root");
    let database = database(&root);
    let ledger = open(&root, &database);
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let appended = ledger.append(observational(&ids));
    assert!(appended.is_ok(), "{appended:?}");

    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .unwrap_or_else(|error| panic!("the connection is refused after the append: {error:?}"));
    let synchronous = connection
        .pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))
        .expect("synchronous readback");
    assert_eq!(synchronous, 2, "PRAGMA synchronous is FULL");
}
