// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 SH-1 (register L4): a stored record may carry its capture, the envelope member
// AO-2 writes; the ledger's readers take it and refuse a capture that names another time.

use super::{PersistedEvent, StoredEventRecord};
use crate::Sha256SecretFingerprinter;
use actingcommand_contract::{
    ArtifactPayloadDraft, AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventSeverity, EventSource, IdentifierIssuer, OriginModule,
};
use serde_json::{Value, json};

const AT_MS: u64 = 1_752_147_200_000;

fn stored_record() -> (IdentifierIssuer, Value) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let draft = EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::GlobalLedger,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        ArtifactPayloadDraft::created(AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"sh1-capture").expect("fingerprinter"))
    .expect("a sanitized record");
    let event = PersistedEvent::from_sanitized(1, draft).expect("a persisted record");
    let stored = serde_json::to_value(StoredEventRecord::from_event(&event)).expect("JSON");
    (ids, stored)
}

#[test]
fn l4_a_stored_record_reads_its_capture_and_refuses_another_time() {
    let (ids, written) = stored_record();
    // The writers before AO-2 write no capture.
    assert!(written.get("capture").is_none());
    let lineage = serde_json::to_value(*ids.mint_owner_epoch().expect("owner epoch").transport())
        .expect("a lineage");

    let mut captured = written.clone();
    captured["capture"] = json!({
        "capture_seq": 9,
        "captured_at_unix_ms": AT_MS,
        "source": {"source": "runtime_facts", "lineage": lineage, "revision": 4},
    });
    let record: StoredEventRecord =
        serde_json::from_value(captured.clone()).expect("the record decodes");
    assert_eq!(
        serde_json::to_value(&record).expect("the record encodes"),
        captured
    );
    record.clone().into_metadata().expect("the ledger reads it");
    record.into_event().expect("the record restores");

    let unchanged: StoredEventRecord =
        serde_json::from_value(written).expect("a record without a capture decodes");
    unchanged
        .into_metadata()
        .expect("a record without a capture reads");

    for capture in [
        json!({"capture_seq": 9, "captured_at_unix_ms": AT_MS + 1}),
        json!({
            "capture_seq": 9,
            "captured_at_unix_ms": AT_MS,
            "source": {"source": "runtime_facts", "lineage": lineage, "revision": 0},
        }),
    ] {
        let mut other = captured.clone();
        other["capture"] = capture;
        let record: StoredEventRecord = serde_json::from_value(other).expect("the record decodes");
        assert!(record.clone().into_metadata().is_err());
        assert!(record.into_event().is_err());
    }
}
