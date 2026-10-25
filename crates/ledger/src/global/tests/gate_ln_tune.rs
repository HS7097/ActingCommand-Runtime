// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-TUNE (brief rd-1026/brief-ln-tune.md item 2): a paged history read asks the store
//! for no more rows than its page can still return. The monitor registry's replay reads the
//! Runtime records in pages smaller than a history page (128 of 256 in production); here a
//! history page holds four rows and the replay's page two, the same proportion. Every row the
//! replay selects is read and authenticated once, and the pages hold exactly what one unpaged
//! read of the same query returns.
//!
//! Every record time is a fixed offset; no clock and no sleep.

use super::*;
use crate::global::{GlobalLedger, GlobalLedgerConfig, Sha256SecretFingerprinter};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventQuery, EventSeverity, EventSource, FactValue,
    IdentifierIssuer, OriginModule, RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft,
    SanitizedEventDraft,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

const AT_MS: u64 = 1_752_147_200_000;
/// The rows of one history page in this test.
const HISTORY_ROWS: usize = 4;
/// The replay's page: half a history page, as 128 is of 256.
const REPLAY_PAGE: usize = 2;
/// The Runtime records the replay selects.
const RUNTIME_RECORDS: u64 = 9;

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"ln-tune-fixture-salt").expect("salt")
}

fn draft(
    ids: &IdentifierIssuer,
    offset: u64,
    origin: EventOrigin,
    payload: EventPayloadDraft,
) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS + offset,
        EventSeverity::Info,
        origin,
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(&salt())
    .expect("a fixture draft")
}

fn runtime_fact(index: u64) -> (EventOrigin, EventPayloadDraft) {
    (
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        RuntimePayloadDraft::fact_recorded(
            RuntimeFactRecord {
                scope: RuntimeFactScope::Runtime,
                key: format!("backend.fixture_key_{index}"),
                value: FactValue::Integer(1),
                observed_at_unix_ms: AT_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
}

fn cli_command() -> (EventOrigin, EventPayloadDraft) {
    (
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
}

/// The monitor registry's replay (`MonitorRegistry::replay_runtime_records`): pages of
/// `REPLAY_PAGE` through one position, each after the last record of the one before.
fn replay(ledger: &GlobalLedger, query: &EventQuery, through: u64) -> Vec<PersistedEvent> {
    let mut events = Vec::new();
    let mut after = 0;
    while after < through {
        let page = ledger
            .query_page(query.clone(), after, through, REPLAY_PAGE)
            .expect("a replay page");
        let Some(last) = page.last() else {
            break;
        };
        after = last.sequence();
        events.extend(page);
    }
    events
}

#[test]
fn ln_tune_every_monitor_row_is_authenticated_once_per_read() {
    let root = TempDir::new().expect("state root");
    let rows_read = Arc::new(AtomicU64::new(0));
    let ledger = sqlite_contract::open(
        GlobalLedgerConfig::new(root.path(), "ln-tune-monitor-pages")
            .with_history_page_rows(HISTORY_ROWS)
            .with_history_rows_read(Arc::clone(&rows_read)),
    )
    .expect("a SQLite ledger");
    // Runtime records with a command of another origin after every third one, so the replay's
    // selection is not the whole history; the last record is another origin's.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let mut offset = 0;
    for index in 0..RUNTIME_RECORDS {
        let (origin, payload) = runtime_fact(index);
        ledger
            .append(draft(&ids, offset, origin, payload))
            .expect("a Runtime record");
        offset += 1;
        if index % 3 == 2 {
            let (origin, payload) = cli_command();
            ledger
                .append(draft(&ids, offset, origin, payload))
                .expect("a command record");
            offset += 1;
        }
    }
    let head = ledger.latest_sequence().expect("head");
    let query = EventQuery {
        origin_module: Some(OriginModule::Runtime),
        ..EventQuery::default()
    };

    rows_read.store(0, Ordering::SeqCst);
    let replayed = replay(&ledger, &query, head);
    let read = rows_read.load(Ordering::SeqCst);

    let whole = ledger.query(query).expect("one unpaged read");
    assert_eq!(
        whole.len(),
        usize::try_from(RUNTIME_RECORDS).expect("count"),
        "the fixture's Runtime records"
    );
    assert_eq!(
        replayed, whole,
        "the replay's pages hold what one read holds"
    );
    assert_eq!(
        read, RUNTIME_RECORDS,
        "each Runtime record is read and authenticated once by the replay"
    );
    ledger.close().expect("close");
}
