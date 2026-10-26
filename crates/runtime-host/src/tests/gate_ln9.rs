// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-9 (LEAN-L model §5.1 row LN-9, §6.3; Alice's ruling 57): damage in an old row that no
//! reader reads stops the Runtime within one pass of the ledger's background scrub, as a Global
//! failure (ISO-M) with the code and operation the full verification (`ledger-maintenance
//! verify`, which was the start's open before LN-8) gives for the same damage.
//!
//! The host runs the shipped `ledger_observer` defaults (625 rows a second, chunks of 256 rows, a
//! pass every 2 400 000 ms) on a stepping scrub clock the test drives, so no wait follows the wall
//! clock. The damaged row is an instance fact no reader reads again, with later facts after it;
//! it is damaged after the open, so the open never reads it, before and after LN-8. `GATE_WAIT`
//! only bounds a broken run.

use super::*;
use crate::FailureScope;
use actingcommand_ledger::{LedgerCode, LedgerLocation, LedgerMaintenance, LedgerOpenTiming};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};

const GATE_WAIT: Duration = Duration::from_secs(30);
/// `ledger_observer.scrub_pass_interval_ms` of the defaults file.
const PASS_INTERVAL_MS: u64 = 2_400_000;
/// The instance facts the test publishes; the first is the damaged one.
const FACTS: u64 = 4;

fn fact(index: u64) -> FactRecord {
    FactRecord {
        scope: FactScope::Server {
            server_id: "fixture-server".to_owned(),
        },
        key: format!("env.ln9_fixture_{index}"),
        content: FactContent::Inline {
            value: ContractFactValue::Boolean(true),
        },
        observed_at_unix_ms: 1_000,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "detector.fixture".to_owned(),
        // One observation per fact: a second record under a snapshot id that names an active
        // record is refused (`fact_observation_version_conflict`).
        source_snapshot_id: format!("snapshot:ln9-{index}"),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

/// Changes the last digit of the keyed tag of the first fact's row: the row no longer equals its
/// projection over its stored bytes.
fn damage_first_fact(root: &TempDir) {
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("state database");
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("state database connection");
    let stored: i64 = connection
        .query_row(
            "SELECT sequence FROM ledger_events WHERE instr(canonical_record, 'env.ln9_fixture_0') > 0 ORDER BY sequence LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("the first fact's row");
    connection
        .execute_batch(&format!(
            "UPDATE ledger_events SET integrity_tag = substr(integrity_tag, 1, length(integrity_tag) - 1) || CASE substr(integrity_tag, -1) WHEN '0' THEN '1' ELSE '0' END WHERE sequence = {stored}"
        ))
        .expect("damage the row");
}

/// The full verification of the closed root (`ledger-maintenance verify`).
fn full_verification(root: &TempDir) -> (&'static str, &'static str) {
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("state database");
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        root.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner");
    let refused = maintenance
        .verify_formal(&database, &mut LedgerOpenTiming::default())
        .expect_err("the full verification refuses the damage");
    maintenance.close().expect("close the maintenance owner");
    (refused.code(), refused.operation())
}

#[test]
fn ln9_d_a_scrub_finding_stops_the_runtime_as_a_global_failure_with_the_full_verification_code() {
    let root = TempDir::new().expect("tempdir");
    let clock = actingcommand_ledger::SteppingScrubClock::new();
    let host = RuntimeHost::start(
        config(&root).with_ledger_scrub_clock_for_test(
            Arc::clone(&clock) as Arc<dyn actingcommand_ledger::LedgerScrubClock>
        ),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    assert!(
        clock.wait_idle(GATE_WAIT),
        "the scrub waits for its first pass"
    );
    for index in 0..FACTS {
        host.publish_fact(fact(index)).expect("publish a fact");
    }
    damage_first_fact(&root);
    assert!(
        host.fatal_error().expect("health").is_none(),
        "no reader reads the damaged row"
    );

    // One pass: the next one is due at the pass interval.
    clock.run_until(PASS_INTERVAL_MS - 1);
    assert!(clock.wait_idle(GATE_WAIT), "the scrub stops");
    assert!(clock.has_ended(), "the scrub ended at its finding");
    assert!(clock.now() < PASS_INTERVAL_MS, "found within one pass");

    let expected = (
        LedgerCode::LedgerRecordMismatch.as_str(),
        LedgerLocation::VerifySqliteRecord.as_str(),
    );
    let latched = host
        .fatal_error()
        .expect("health")
        .expect("the Runtime latched the scrub's finding");
    assert_eq!((latched.code(), latched.operation()), expected);
    assert!(latched.is_fatal());
    assert_eq!(latched.scope(), FailureScope::Global);
    assert!(latched.halts_runtime());
    assert!(
        host.is_shutdown_requested().expect("shutdown state"),
        "the latch stops the Runtime"
    );
    let closed = host
        .close()
        .expect_err("the Runtime ends with the scrub's finding");
    assert_eq!((closed.code(), closed.operation()), expected);

    assert_eq!(
        full_verification(&root),
        expected,
        "the code and operation of the full verification"
    );
}
