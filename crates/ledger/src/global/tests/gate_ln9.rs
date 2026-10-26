// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-9 (LEAN-L model §5.1 row LN-9, §6; Alice's ruling 57): the paced background scrub of
//! the Runtime's writer.
//!
//! - Damage in an old row (the row's bytes, its link row, a missing row) is found within one
//!   pass, with the code and operation the full verification (`ledger-maintenance verify`, which
//!   was the start's open before LN-8) gives for the same damage, and the scrub's finding is fatal.
//! - The rate is respected: a chunk starts no earlier than the one before plus its rows at the
//!   rate in effect, a new rate takes effect at the next chunk, and a pass starts once per pass
//!   interval.
//! - The writer does not wait for the scrub: a whole pass runs while the test holds the writer's
//!   shared connection, and the writer commits while the scrub holds its chunk's snapshot.
//!
//! The formal root is a small fixture; the scrub runs on a stepping clock the test drives
//! (`SteppingScrubClock`), so no wait follows the wall clock. `GATE_WAIT` only bounds a broken run.
//! The damage is made after the writer opened, so the open never reads it, before and after LN-8.

use crate::global::{
    GlobalLedger, GlobalLedgerError, LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming,
    LedgerScrubPolicy, LedgerWritePolicy, Sha256SecretFingerprinter, SteppingScrubClock,
};
use crate::{LedgerCode, LedgerLocation};
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventSeverity, EventSource,
    IdentifierIssuer, MonitorPayloadDraft, OriginModule, OwnerEpoch, SanitizedEventDraft,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);
// The State integrity key and the writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ln9-fixture";
const OWNER: &str = "ledger-ln9-fixture";
const AT_MS: u64 = 1_752_147_200_000;
/// The fixture's records; with the scrub's chunks of `CHUNK_ROWS`, a pass reads several chunks.
const RECORDS: u64 = 40;
const CHUNK_ROWS: usize = 8;
/// Four rows a second: a chunk of eight rows every 2 000 ms of the stepping clock.
const ROWS_PER_SECOND: u64 = 4;
const CHUNK_MS: u64 = 2_000;
const PASS_INTERVAL_MS: u64 = 3_600_000;

/// One damage case: its name, the damage, and the code and operation that find it.
type DamageCase<'a> = (&'a str, &'a dyn Fn(&Root), (&'a str, &'a str));

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

// `monitor.probe_requested`, an observational record without links.
fn draft(ids: &IdentifierIssuer) -> SanitizedEventDraft {
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
    .sanitize(&Sha256SecretFingerprinter::new(b"ledger-ln9-fixture").expect("salt"))
    .expect("an observational draft")
}

/// A stored sequence (`ordered-u64-v1`).
fn stored(sequence: u64) -> i64 {
    (sequence ^ (1_u64 << 63)) as i64
}

/// The time by which a pass over `head` rows has read its last chunk on the stepping clock.
fn pass_ms(head: u64) -> u64 {
    head.div_ceil(CHUNK_ROWS as u64).saturating_sub(1) * CHUNK_MS
}

// A formal root as acsetup leaves it.
struct Root {
    directory: TempDir,
    database: Arc<RuntimeDatabase>,
}

impl Root {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("state root");
        let database = Arc::new(
            RuntimeStateStore::open_database(directory.path(), STATE_KEY).expect("State database"),
        );
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            directory.path(),
            true,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner");
        maintenance
            .initialize_empty(&database)
            .expect("formal ledger");
        maintenance
            .open_writer(Arc::clone(&database), OWNER.into())
            .expect("formal writer")
            .close()
            .expect("close the formal writer");
        Self {
            directory,
            database,
        }
    }

    fn state_root(&self) -> PathBuf {
        self.directory.path().canonicalize().expect("state root")
    }

    /// A start: the Runtime's writer with a scrub paced on `clock`, then `RECORDS` records.
    fn open(&self, clock: &Arc<SteppingScrubClock>) -> GlobalLedger {
        let scrub = LedgerScrubPolicy::new(ROWS_PER_SECOND, CHUNK_ROWS, PASS_INTERVAL_MS)
            .expect("scrub policy")
            .with_clock(Arc::clone(clock) as Arc<dyn crate::global::LedgerScrubClock>);
        let plan = LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], 120_000, 1_000, 1_073_741_824)
                .expect("write policy"),
            &self.state_root(),
            None,
            epoch(),
            AT_MS,
        )
        .with_scrub(scrub);
        let limits = MaintenanceLimits::default();
        let ledger = LedgerMaintenance::acquire(
            self.directory.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner")
        .open_writer_timed(
            Arc::clone(&self.database),
            OWNER.into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
        .expect("the Runtime's writer");
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        for _ in 0..RECORDS {
            ledger.append(draft(&ids)).expect("a record");
        }
        ledger
    }

    /// Changes the root through the writer's own connection, between its commits.
    fn damage(&self, sql: &str) {
        self.database
            .connection(DatabaseLocation::OpenExistingDatabase.location())
            .expect("the shared connection")
            .execute_batch(sql)
            .expect("damage the fixture");
    }

    /// Changes one digit inside the stored record's event id: the JSON stays valid and decodable,
    /// so only the record's hash, tag and index columns tell.
    fn flip_event_id_digit(&self, sequence: u64) {
        let connection = self
            .database
            .connection(DatabaseLocation::OpenExistingDatabase.location())
            .expect("the shared connection");
        let mut record: Vec<u8> = connection
            .query_row(
                "SELECT canonical_record FROM ledger_events WHERE sequence=?1",
                [stored(sequence)],
                |row| row.get(0),
            )
            .expect("stored record");
        let digit = record
            .windows(5)
            .position(|window| window == b"\"evt_")
            .expect("event id string")
            + 5;
        record[digit] = if record[digit] == b'0' { b'1' } else { b'0' };
        connection
            .execute(
                "UPDATE ledger_events SET canonical_record=?1 WHERE sequence=?2",
                rusqlite::params![record, stored(sequence)],
            )
            .expect("damage the record");
    }

    /// The full verification of the closed root (`ledger-maintenance verify`).
    fn verify(&self) -> GlobalLedgerError {
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            self.directory.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner");
        let refused = maintenance
            .verify_formal(&self.database, &mut LedgerOpenTiming::default())
            .err()
            .expect("the full verification refuses the damage");
        maintenance.close().expect("close the maintenance owner");
        refused
    }
}

#[test]
fn ln9_a_damage_in_an_old_row_is_found_within_one_pass_with_the_full_verification_code() {
    let cases: [DamageCase<'_>; 3] = [
        (
            "the bytes of row 3",
            &|root: &Root| root.flip_event_id_digit(3),
            (
                LedgerCode::LedgerRecordMismatch.as_str(),
                LedgerLocation::VerifySqliteRecord.as_str(),
            ),
        ),
        (
            "the link row of row 5",
            &|root: &Root| {
                root.damage(&format!(
                    "UPDATE ledger_links SET run_id='invalid-run' WHERE sequence={}",
                    stored(5)
                ));
            },
            (
                LedgerCode::LedgerIndexMismatch.as_str(),
                LedgerLocation::VerifySqliteRelations.as_str(),
            ),
        ),
        (
            "row 4 missing",
            &|root: &Root| {
                root.damage(&format!(
                    "DELETE FROM ledger_links WHERE sequence={0}; DELETE FROM ledger_events WHERE sequence={0}",
                    stored(4)
                ));
            },
            (
                LedgerCode::SequenceDiscontinuity.as_str(),
                LedgerLocation::RecoverSequence.as_str(),
            ),
        ),
    ];
    for (case, damage, expected) in cases {
        let root = Root::new();
        let clock = SteppingScrubClock::new();
        let ledger = root.open(&clock);
        let head = ledger.latest_sequence().expect("head");
        assert!(head >= RECORDS, "{case}: the fixture's records");
        assert!(
            clock.wait_idle(GATE_WAIT),
            "{case}: the scrub waits for its first pass"
        );
        assert_eq!(
            clock.waits(),
            vec![0],
            "{case}: the first pass is due at the open"
        );
        assert!(
            ledger.scrub_finding().is_none(),
            "{case}: nothing found yet"
        );
        damage(&root);

        // One pass: every chunk is due by `pass_ms(head)`; the next pass is not.
        clock.run_until(pass_ms(head));
        assert!(clock.wait_idle(GATE_WAIT), "{case}: the scrub stops");
        assert!(clock.has_ended(), "{case}: the scrub ended at its finding");
        assert!(
            clock.now() <= pass_ms(head),
            "{case}: found within one pass"
        );
        let finding = ledger.scrub_finding().expect("the scrub's finding");
        assert!(finding.is_fatal(), "{case}: a finding is fatal");
        assert_eq!((finding.code(), finding.operation()), expected, "{case}");
        assert!(
            finding
                .detail()
                .is_some_and(|detail| detail.contains("phase=scrub")),
            "{case}: {:?}",
            finding.detail()
        );
        ledger.close().expect("close the writer");

        let full = root.verify();
        assert_eq!(
            (full.code(), full.operation()),
            (finding.code(), finding.operation()),
            "{case}: the code and operation of the full verification"
        );
    }
}

#[test]
fn ln9_b_the_scrub_keeps_its_rate_and_takes_a_new_rate_at_its_next_chunk() {
    let root = Root::new();
    let clock = SteppingScrubClock::new();
    let ledger = root.open(&clock);
    let head = ledger.latest_sequence().expect("head");
    let chunks = head.div_ceil(CHUNK_ROWS as u64);
    assert!(
        chunks >= 4,
        "the pass reads several chunks after the change"
    );
    assert!(clock.wait_idle(GATE_WAIT), "the first pass is due");

    // The first three chunks at four rows a second; the fourth is due 2 000 ms after the third.
    clock.run_until(2 * CHUNK_MS);
    assert!(clock.wait_idle(GATE_WAIT), "parked before the fourth chunk");
    assert_eq!(
        clock.waits(),
        vec![0, 0, CHUNK_MS, 2 * CHUNK_MS, 3 * CHUNK_MS]
    );

    // A rate outside its range is refused; eight rows a second takes effect at the next chunk.
    for refused in [0, 100_001] {
        let error = ledger
            .set_scrub_rows_per_second(refused)
            .expect_err("a rate out of range");
        assert_eq!(error.code(), LedgerCode::InvalidLedgerConfig.as_str());
    }
    ledger
        .set_scrub_rows_per_second(2 * ROWS_PER_SECOND)
        .expect("a new rate");
    clock.run_until(PASS_INTERVAL_MS - 1);
    assert!(clock.wait_idle(GATE_WAIT), "parked before the next pass");
    assert!(!clock.has_ended(), "the pass found nothing");
    assert!(ledger.scrub_finding().is_none());

    // The pass's wait, then one wait per chunk: chunk k starts after the rows of the chunks
    // before it at the rate in effect when the one before started; then the next pass's wait.
    let mut expected = vec![0, 0];
    let mut due = 0;
    for chunk in 1..chunks {
        due += if chunk <= 3 { CHUNK_MS } else { CHUNK_MS / 2 };
        expected.push(due);
    }
    expected.push(PASS_INTERVAL_MS);
    assert_eq!(clock.waits(), expected);
    // Never faster than the rate: the rows read before a chunk starts fit its start at the
    // highest rate in effect.
    for (index, start) in expected[1..expected.len() - 1].iter().enumerate() {
        let rows_before = index as u64 * CHUNK_ROWS as u64;
        assert!(
            rows_before * 1_000 <= start * 2 * ROWS_PER_SECOND,
            "chunk {index} at {start} ms"
        );
    }
    ledger.close().expect("close the writer");
}

#[test]
fn ln9_c_the_writer_does_not_wait_for_the_scrub() {
    let root = Root::new();
    let clock = SteppingScrubClock::new();
    let ledger = root.open(&clock);
    let head = ledger.latest_sequence().expect("head");
    assert!(clock.wait_idle(GATE_WAIT), "the first pass is due");

    // A whole pass, its whole-file check included, while the test holds the writer's connection:
    // the scrub never takes it.
    let held = root
        .database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("the shared connection");
    clock.run_until(PASS_INTERVAL_MS - 1);
    assert!(clock.wait_idle(GATE_WAIT), "the pass ends");
    drop(held);
    assert!(!clock.has_ended(), "the pass found nothing");
    assert_eq!(
        clock.waits().last(),
        Some(&PASS_INTERVAL_MS),
        "the scrub waits for its next pass"
    );

    // The writer commits while the scrub holds the snapshot of its next pass's first chunk.
    clock.hold_chunks(true);
    clock.run_until(PASS_INTERVAL_MS + pass_ms(head));
    assert!(clock.wait_held(GATE_WAIT), "the scrub holds its snapshot");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for _ in 0..RECORDS {
        ledger
            .append(draft(&ids))
            .expect("a commit while the scrub reads");
    }
    assert_eq!(
        ledger.latest_sequence().expect("head"),
        head + RECORDS,
        "every record committed while the scrub held its snapshot"
    );
    clock.hold_chunks(false);
    assert!(clock.wait_idle(GATE_WAIT), "the second pass ends");
    assert!(!clock.has_ended(), "the second pass found nothing");
    assert!(ledger.scrub_finding().is_none());
    ledger.close().expect("close the writer");
}
