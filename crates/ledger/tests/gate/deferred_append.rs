// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2, L-1 G10: deferred append confirmed at boundaries
//! (`contracts/ledger-store.md`, "Deferred append"; a duplicate EventId is the nonfatal
//! `duplicate_event_id / append_event`), on a SQLite candidate ledger.

use crate::GATE_WAIT;
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventSeverity, EventSource, IdentifierIssuer, IssuedEventId, OriginModule,
    SanitizedEventDraft,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerError, Sha256SecretFingerprinter,
};
use actingcommand_runtime_database::RuntimeDatabase;
use std::sync::Arc;
use tempfile::TempDir;

/// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;

fn open(root: &TempDir) -> GlobalLedger {
    let database = RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
        root.path(),
        b"gate-l1-deferred-append-seed",
        |_| Ok(()),
        |_| Ok(()),
    )
    .expect("candidate physical owner");
    GlobalLedger::open_sqlite_candidate(
        GlobalLedgerConfig::new(root.path(), "gate-owner"),
        Arc::new(database),
    )
    .expect("SQLite candidate ledger")
}

/// An ordinary durable fact (`command.received`).
fn draft(event_id: IssuedEventId, timestamp_unix_ms: u64) -> SanitizedEventDraft {
    EventDraft::new(
        event_id,
        timestamp_unix_ms,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"gate-l1-deferred-append").expect("salt"))
    .expect("draft")
}

#[test]
fn gate_deferred_appends_confirm_in_acceptance_order_with_contiguous_sequences() {
    let root = TempDir::new().expect("root");
    let ledger = open(&root);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let head = ledger.latest_sequence().expect("head");

    let mut accepted = Vec::new();
    for offset in 1..=3 {
        let fact = draft(ids.mint_event_id().expect("event id"), AT_MS + offset);
        let event_type = fact.event_type();
        let ticket = ledger.append_deferred(fact).expect("accepted");
        accepted.push((*ticket.event_id(), event_type));
    }
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("confirmation");

    assert_eq!(summary.committed.len(), accepted.len(), "{summary:?}");
    for ((committed, (event_id, event_type)), sequence) in summary
        .committed
        .iter()
        .zip(&accepted)
        .zip(head + 1..=head + 3)
    {
        assert_eq!(committed.event_id(), event_id, "acceptance order");
        assert_eq!(committed.event_type(), *event_type);
        assert_eq!(committed.sequence(), sequence, "contiguous sequences");
    }
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    assert_eq!(summary.pending, 0);
    let closed = ledger.close();
    assert!(closed.is_ok(), "{closed:?}");
}

#[test]
fn gate_deferred_append_failure_is_returned_verbatim_at_confirmation() {
    let root = TempDir::new().expect("root");
    let ledger = open(&root);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let reused = ids.mint_event_id().expect("event id");

    let first = ledger
        .append_deferred(draft(reused, AT_MS + 1))
        .expect("A accepted");
    ledger
        .append_deferred(draft(reused, AT_MS + 2))
        .expect("B, with A's event id, accepted");
    let third = ledger
        .append_deferred(draft(ids.mint_event_id().expect("event id"), AT_MS + 3))
        .expect("C accepted");
    let summary = ledger.confirm_deferred(GATE_WAIT).expect("confirmation");

    let committed = summary
        .committed
        .iter()
        .map(|reference| *reference.event_id())
        .collect::<Vec<_>>();
    assert_eq!(committed, vec![*first.event_id(), *third.event_id()]);
    assert_eq!(
        summary.committed[1].sequence(),
        summary.committed[0].sequence() + 1,
        "the refused append consumed no sequence"
    );
    assert_eq!(summary.failed.len(), 1, "{:?}", summary.failed);
    let failure = &summary.failed[0];
    assert_eq!(failure.code(), "duplicate_event_id");
    assert_eq!(failure.operation(), "append_event");
    assert!(!failure.is_fatal(), "{failure:?}");
    assert_eq!(summary.pending, 0);

    let following = ledger.append(draft(ids.mint_event_id().expect("event id"), AT_MS + 4));
    assert!(following.is_ok(), "{following:?}");
}
