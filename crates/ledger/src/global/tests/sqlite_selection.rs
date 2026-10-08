// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #375 R375-3: a selected opening reads and authenticates only the rows of its types,
// the head row and the contiguity of the stored sequences.
use super::sqlite_prefix::{formal_root, migrated_root};
use super::*;
use crate::codes::{LedgerCode, LedgerLocation};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalPayloadDraft, ApprovalTarget,
    CatalogPayloadDraft, CatalogTransitionEventData, LedgerView, outcome::Category,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};

/// The check-config preview's types: the catalog replay's and the approval decisions.
const SELECTED: [EventType; 6] = [
    EventType::StateMigrated,
    EventType::CatalogTransitionIntent,
    EventType::CatalogActivated,
    EventType::CatalogRolledBack,
    EventType::CatalogTransitionFailed,
    EventType::ApprovalDecision,
];

fn fingerprinter() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"test-private-salt").expect("fingerprinter")
}

fn system_links() -> EventLinksDraft {
    EventLinksDraft::default()
        .with_request_id(request_id())
        .with_correlation_id(correlation_id())
        .with_action_id(action_id())
}

fn catalog_intent(version: u64) -> actingcommand_contract::SanitizedEventDraft {
    EventDraft::new(
        event_id(),
        1_752_147_200_000 + version,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Policy,
            EventActor::Runtime,
        ),
        system_links(),
        CatalogPayloadDraft::transition_intent(
            EventAction::CatalogActivate,
            CatalogTransitionEventData {
                catalog_id: "catalog:fixture-a".to_owned(),
                catalog_version: version,
                catalog_hash: format!("sha256:{}", "c".repeat(64)),
                previous_catalog_hash: Some(format!("sha256:{}", "b".repeat(64))),
                promotion: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("catalog intent")
}

fn approval(index: u64) -> actingcommand_contract::SanitizedEventDraft {
    EventDraft::new(
        event_id(),
        1_752_147_300_000 + index,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Ui, OriginModule::Governance, EventActor::User),
        system_links(),
        ApprovalPayloadDraft::decision(
            ApprovalDecisionRecord::new(
                format!("approval:fixture-{index}"),
                ApprovalDisposition::Approved,
                ApprovalTarget::Catalog {
                    catalog_hash: format!("sha256:{}", "d".repeat(64)),
                    catalog_version: 1,
                },
                "user_confirmed",
            )
            .expect("approval record"),
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("approval decision")
}

/// Command events at 1-3, 5, 7 and 8 (the head); a catalog intent at 4; an approval at 6.
fn mixed_drafts() -> Vec<actingcommand_contract::SanitizedEventDraft> {
    vec![
        event("one"),
        event("two"),
        event("three"),
        catalog_intent(1),
        event("five"),
        approval(6),
        event("seven"),
        event("head"),
    ]
}

/// A closed formal SQLite root whose writer appended `drafts`, as `formal_root` builds one.
fn selection_root(drafts: Vec<actingcommand_contract::SanitizedEventDraft>) -> TempDir {
    let root = TempDir::new().expect("root");
    let database = sqlite_contract::database(root.path());
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        root.path(),
        true,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("fresh root");
    maintenance
        .initialize_empty(&database)
        .expect("formal empty ledger");
    let ledger = maintenance
        .open_writer(database, "selection".into(), |_| None)
        .expect("formal writer");
    for draft in drafts {
        ledger.append(draft).expect("append");
    }
    ledger.close().expect("close writer");
    root
}

fn open_selected(root: &TempDir, types: &[EventType]) -> GlobalLedgerResult<GlobalLedgerSelection> {
    GlobalLedger::open_selected(root.path(), types, Instant::now() + Duration::from_secs(30))
}

/// Each selected type's answer, in the order of `types`.
fn answers(selection: &GlobalLedgerSelection, types: &[EventType]) -> Vec<Vec<PersistedEvent>> {
    types
        .iter()
        .map(|event_type| {
            selection
                .query(&EventQuery {
                    event_type: Some(*event_type),
                    ..EventQuery::default()
                })
                .expect("selected type")
        })
        .collect()
}

/// The complete record-path evidence opening's answers for the same types, and its head.
fn evidence_answers(root: &TempDir, types: &[EventType]) -> (Vec<Vec<PersistedEvent>>, u64) {
    let evidence = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read(),
        |_| None,
    )
    .expect("complete evidence");
    let answers = types
        .iter()
        .map(|event_type| {
            evidence.query(&EventQuery {
                event_type: Some(*event_type),
                ..EventQuery::default()
            })
        })
        .collect();
    (answers, evidence.latest_sequence())
}

/// The stored form of a sequence (the ledger's ordered-u64 encoding).
fn stored(sequence: u64) -> i64 {
    (sequence ^ (1_u64 << 63)) as i64
}

/// Changes the closed root through SQL, as the SQLite integrity matrix does.
fn tamper(root: &TempDir, sql: &str) {
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("fixture database");
    database
        .connection("tamper selection fixture")
        .expect("fixture connection")
        .execute_batch(sql)
        .expect("tamper fixture");
}

fn refusal(result: GlobalLedgerResult<GlobalLedgerSelection>) -> (&'static str, &'static str) {
    let error = result.err().expect("refused opening");
    (error.code(), error.operation())
}

#[test]
fn selected_open_returns_exactly_the_selected_events() {
    let root = selection_root(mixed_drafts());
    let selection = open_selected(&root, &SELECTED).expect("selected opening");
    let (expected, head) = evidence_answers(&root, &SELECTED);
    assert_eq!(answers(&selection, &SELECTED), expected);
    assert_eq!(expected.iter().map(Vec::len).sum::<usize>(), 2);
    assert_eq!((selection.head_sequence(), head), (8, 8));
    assert!(selection.is_complete());
}

#[test]
fn selected_query_refuses_unselected_types_and_views() {
    let root = selection_root(mixed_drafts());
    let selection = open_selected(&root, &SELECTED).expect("selected opening");
    for query in [
        EventQuery {
            event_type: Some(EventType::CommandReceived),
            ..EventQuery::default()
        },
        EventQuery::default(),
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            view: Some(LedgerView::Events),
            ..EventQuery::default()
        },
    ] {
        let error = selection.query(&query).expect_err("refused query");
        assert_eq!(
            (error.code(), error.operation(), error.is_fatal()),
            (
                LedgerCode::SelectionQueryUnsupported.as_str(),
                LedgerLocation::QuerySelection.as_str(),
                false
            )
        );
    }
    assert_eq!(
        LedgerCode::SelectionQueryUnsupported.category(),
        Category::Error
    );
}

#[test]
fn selected_rows_and_their_relations_are_authenticated() {
    let row = selection_root(mixed_drafts());
    tamper(
        &row,
        &format!(
            "UPDATE ledger_events SET severity='warning' WHERE sequence={}",
            stored(4)
        ),
    );
    assert_eq!(
        refusal(open_selected(&row, &SELECTED)),
        ("ledger_record_mismatch", "verify_sqlite_record")
    );
    let link = selection_root(mixed_drafts());
    tamper(
        &link,
        &format!(
            "UPDATE ledger_links SET run_id='invalid-run' WHERE sequence={}",
            stored(6)
        ),
    );
    assert_eq!(
        refusal(open_selected(&link, &SELECTED)),
        ("ledger_index_mismatch", "verify_sqlite_relations")
    );
}

#[test]
fn contiguity_and_the_unselected_head_are_authenticated() {
    let gap = selection_root(mixed_drafts());
    tamper(
        &gap,
        &format!(
            "DELETE FROM ledger_links WHERE sequence={0}; DELETE FROM ledger_events WHERE sequence={0}",
            stored(2)
        ),
    );
    assert_eq!(
        refusal(open_selected(&gap, &SELECTED)),
        ("sequence_discontinuity", "recover_sequence")
    );
    let head = selection_root(mixed_drafts());
    tamper(
        &head,
        &format!(
            "UPDATE ledger_events SET severity='warning' WHERE sequence={}",
            stored(8)
        ),
    );
    assert_eq!(
        refusal(open_selected(&head, &SELECTED)),
        ("ledger_record_mismatch", "verify_sqlite_record")
    );
}

#[test]
fn rows_of_other_types_are_not_read() {
    // Q4: row 2 is neither the head nor the predecessor of a selected row.
    let root = selection_root(mixed_drafts());
    let before = answers(
        &open_selected(&root, &SELECTED).expect("original"),
        &SELECTED,
    );
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("fixture database");
    {
        let connection = database
            .connection("tamper selection fixture")
            .expect("fixture connection");
        let mut record: Vec<u8> = connection
            .query_row(
                "SELECT canonical_record FROM ledger_events WHERE sequence=?1",
                [stored(2)],
                |row| row.get(0),
            )
            .expect("stored record");
        // One byte inside the event id string; the JSON stays valid and decodable.
        let digit = record
            .windows(5)
            .position(|window| window == b"\"evt_")
            .expect("event id string")
            + 5;
        record[digit] = if record[digit] == b'0' { b'1' } else { b'0' };
        connection
            .execute(
                "UPDATE ledger_events SET canonical_record=?1 WHERE sequence=?2",
                rusqlite::params![record, stored(2)],
            )
            .expect("tamper record");
    }
    drop(database);
    let after = open_selected(&root, &SELECTED).expect("selected opening skips row 2");
    assert_eq!(answers(&after, &SELECTED), before);
    let error = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read(),
        |_| None,
    )
    .err()
    .expect("the complete read detects row 2");
    assert_eq!(
        (error.code(), error.operation()),
        ("ledger_record_mismatch", "verify_sqlite_record")
    );
}

#[test]
fn selection_without_derived_views_scans_the_same_rows() {
    let root = selection_root(mixed_drafts());
    let with_views = answers(&open_selected(&root, &SELECTED).expect("views"), &SELECTED);
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("fixture database");
    {
        let connection = database
            .connection("drop derived views")
            .expect("fixture connection");
        let objects = connection
            .prepare("SELECT type,name FROM sqlite_schema WHERE name GLOB 'ledger_view_*'")
            .expect("derived schema")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("derived objects")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("read derived objects");
        assert!(!objects.is_empty());
        for (kind, name) in objects {
            let kind = if kind == "view" { "VIEW" } else { "INDEX" };
            connection
                .execute_batch(&format!("DROP {kind} IF EXISTS {name}"))
                .expect("drop derived object");
        }
    }
    drop(database);
    let without = open_selected(&root, &SELECTED).expect("no views");
    assert_eq!(answers(&without, &SELECTED), with_views);
    assert_eq!(without.head_sequence(), 8);
}

#[test]
fn selected_open_honours_its_deadline() {
    let root = formal_root(3);
    let error = GlobalLedger::open_selected(root.path(), &SELECTED, Instant::now())
        .err()
        .expect("deadline reached");
    assert_eq!(
        (error.code(), error.operation(), error.is_fatal()),
        ("ledger_read_budget_exceeded", "read_only_snapshot", false)
    );
}

#[test]
fn segment_root_is_read_whole_and_filtered() {
    let root = TempDir::new().expect("root");
    let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
        root.path().join("ledger"),
        "segment",
    ))
    .expect("segment writer");
    for draft in mixed_drafts() {
        ledger.append(draft).expect("append");
    }
    ledger.close().expect("close segment writer");
    let selection = open_selected(&root, &SELECTED).expect("segment selection");
    let (expected, head) = evidence_answers(&root, &SELECTED);
    assert_eq!(answers(&selection, &SELECTED), expected);
    assert_eq!(expected.iter().map(Vec::len).sum::<usize>(), 2);
    assert_eq!((selection.head_sequence(), head), (8, 8));
    assert!(selection.is_complete());
}

#[test]
fn a_selected_head_is_read_once() {
    let root = selection_root(vec![
        event("one"),
        catalog_intent(1),
        event("three"),
        approval(4),
    ]);
    let selection = open_selected(&root, &SELECTED).expect("selected head");
    assert_eq!(selection.head_sequence(), 4);
    let (expected, head) = evidence_answers(&root, &SELECTED);
    assert_eq!(head, 4);
    let selected = answers(&selection, &SELECTED);
    assert_eq!(selected, expected);
    let approvals = &selected[5];
    assert_eq!(
        approvals
            .iter()
            .map(PersistedEvent::sequence)
            .collect::<Vec<_>>(),
        vec![4]
    );
}

#[test]
fn migrated_root_opens_without_its_prefix_digest() {
    // The cutover completion is the `ledger.recovered` event at the cutover sequence.
    let (root, cutover) = migrated_root();
    let types = [EventType::LedgerRecovered];
    let selection = open_selected(&root, &types).expect("migrated selection");
    let (expected, head) = evidence_answers(&root, &types);
    let selected = answers(&selection, &types);
    assert_eq!(selected, expected);
    assert_eq!(
        selected[0]
            .iter()
            .map(PersistedEvent::sequence)
            .collect::<Vec<_>>(),
        vec![cutover]
    );
    assert_eq!(
        (selection.head_sequence(), head),
        (cutover + 1, cutover + 1)
    );
}
