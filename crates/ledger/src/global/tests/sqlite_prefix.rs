// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #363: an SQLite evidence opening reads and authenticates only its declared prefix.
use super::*;
use actingcommand_runtime_database::MaintenanceLimits;

/// A closed formal SQLite root whose writer appended `count` events.
pub(super) fn formal_root(count: usize) -> TempDir {
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
        .open_writer(database, "formal".into())
        .expect("formal writer");
    for index in 0..count {
        ledger
            .append(event(&format!("prefix-{index}")))
            .expect("append");
    }
    ledger.close().expect("close writer");
    root
}

/// A closed root migrated from a two-event segment ledger, with one event after the
/// cutover completion. Returns the cutover sequence.
pub(super) fn migrated_root() -> (TempDir, u64) {
    let root = TempDir::new().expect("root");
    let database = sqlite_contract::database(root.path());
    let legacy = GlobalLedger::open(GlobalLedgerConfig::new(
        root.path().join("ledger"),
        "source",
    ))
    .expect("segment source");
    legacy.append(event("first")).expect("source first");
    legacy.append(event("second")).expect("source second");
    legacy.close().expect("source closed");
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        root.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("closed source");
    let source = maintenance.source().expect("complete source");
    let record = source
        .migration_record(
            &actingcommand_runtime_database::digest(b"backup fixture"),
            &actingcommand_runtime_database::digest(b"state fixture"),
        )
        .expect("migration identity");
    let completion = EventDraft::new(
        event_id(),
        1_752_147_200_001,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::System,
            OriginModule::GlobalLedger,
            EventActor::System,
        ),
        EventLinksDraft::default(),
        actingcommand_contract::LedgerPayloadDraft::migrated(record.clone(), AuditInput::new())
            .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"test-private-salt").unwrap())
    .expect("typed completion");
    maintenance
        .import(&database, &source, &record, completion, false)
        .expect("import");
    let ledger = maintenance
        .open_writer(database, "formal".into())
        .expect("formal writer");
    ledger
        .append(event("after-cutover"))
        .expect("after cutover");
    ledger.close().expect("close writer");
    (root, record.cutover_sequence)
}

fn open_records(root: &TempDir, prefix: Option<u64>) -> GlobalLedgerEvidence {
    let config = GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read();
    let config = match prefix {
        Some(through) => config.sqlite_prefix(through),
        None => config,
    };
    GlobalLedger::open_evidence(config, |_| None).expect("record-path evidence")
}

#[test]
fn prefix_open_reads_only_declared_events() {
    let root = formal_root(20);
    let full = open_records(&root, None);
    let prefix = open_records(&root, Some(7));
    assert_eq!(prefix.events(), &full.events()[..7]);
    assert!(!prefix.material_checked());
    let extent = prefix.read_extent();
    assert_eq!(
        (
            extent.through_sequence,
            extent.head_sequence,
            extent.event_count
        ),
        (7, 20, 7)
    );
    assert!(extent.phases.is_some());
}

#[test]
fn prefix_read_stays_within_its_own_bytes() {
    let root = formal_root(20);
    let prefix_bytes = open_records(&root, Some(7))
        .read_extent()
        .ledger_bytes
        .expect("counted prefix bytes");
    let bounded = |prefix: Option<u64>| {
        let config = GlobalLedgerEvidenceConfig::new(root.path())
            .sqlite_material_not_read()
            .with_budget(prefix_bytes, 7, Instant::now() + Duration::from_secs(5));
        let config = match prefix {
            Some(through) => config.sqlite_prefix(through),
            None => config,
        };
        GlobalLedger::open_evidence(config, |_| None)
    };
    let within = bounded(Some(7)).expect("the prefix fits its own bytes and events");
    assert_eq!(within.read_extent().ledger_bytes, Some(prefix_bytes));
    // The same budget cannot hold the whole ledger, so the prefix read never reached row 8.
    let whole = bounded(None).err().expect("whole ledger exceeds it");
    assert_eq!(whole.code(), "ledger_read_budget_exceeded");
}

#[test]
fn prefix_at_head_equals_full_open() {
    let root = formal_root(20);
    let full = open_records(&root, None);
    let at_head = open_records(&root, Some(20));
    assert_eq!(at_head.events(), full.events());
    let (prefix, whole) = (at_head.read_extent(), full.read_extent());
    assert_eq!(
        (
            prefix.through_sequence,
            prefix.head_sequence,
            prefix.event_count,
            prefix.ledger_bytes
        ),
        (
            whole.through_sequence,
            whole.head_sequence,
            whole.event_count,
            whole.ledger_bytes
        )
    );
    assert_eq!((whole.through_sequence, whole.head_sequence), (20, 20));
}

#[test]
fn prefix_beyond_head_is_a_request_error() {
    let root = formal_root(20);
    let error = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path())
            .sqlite_material_not_read()
            .sqlite_prefix(21),
        |_| None,
    )
    .err()
    .expect("prefix beyond the head");
    assert_eq!(error.code(), "ledger_prefix_beyond_head");
    assert!(!error.is_fatal());
    assert_eq!(error.detail(), Some("through_sequence 21 exceeds head 20"));
}

#[test]
fn migrated_prefix_reads_through_cutover() {
    let (root, cutover) = migrated_root();
    let evidence = open_records(&root, Some(1));
    let extent = evidence.read_extent();
    assert_eq!(extent.through_sequence, cutover);
    assert_eq!(extent.head_sequence, cutover + 1);
    assert_eq!(evidence.events().len() as u64, cutover);
    assert_eq!(
        evidence.events(),
        &open_records(&root, None).events()[..evidence.events().len()]
    );
}

#[test]
fn prefix_requires_record_path() {
    let root = formal_root(3);
    let error = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_prefix(2),
        |_| None,
    )
    .err()
    .expect("required material with a prefix");
    assert_eq!(
        (error.code(), error.is_fatal()),
        ("invalid_evidence_prefix", false)
    );
}

#[test]
fn metadata_refuses_prefix() {
    let root = formal_root(3);
    let error =
        GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(root.path()).sqlite_prefix(2))
            .err()
            .expect("metadata with a prefix");
    assert_eq!(
        (error.code(), error.is_fatal()),
        ("ledger_prefix_unsupported", false)
    );
}

#[test]
fn budget_error_names_the_bound() {
    let root = formal_root(3);
    let later = Instant::now() + Duration::from_secs(5);
    for ((bytes, events, deadline), expected) in [
        ((1, usize::MAX, later), "bytes "),
        ((u64::MAX, 1, later), "events 2 > 1"),
        ((u64::MAX, usize::MAX, Instant::now()), "deadline reached"),
    ] {
        let error = GlobalLedger::open_evidence(
            GlobalLedgerEvidenceConfig::new(root.path())
                .sqlite_material_not_read()
                .with_budget(bytes, events, deadline),
            |_| None,
        )
        .err()
        .expect("exhausted budget");
        assert_eq!(
            (error.code(), error.operation(), error.is_fatal()),
            ("ledger_read_budget_exceeded", "read_only_snapshot", false)
        );
        assert!(
            error
                .detail()
                .is_some_and(|detail| detail.starts_with(expected)),
            "{:?}",
            error.detail()
        );
    }
}
