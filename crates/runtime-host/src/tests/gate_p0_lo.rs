// SPDX-License-Identifier: AGPL-3.0-only

//! P0-LO (brief "Tests", host part): the start's ledger open runs under its configured deadline
//! `ledger_observer.open_timeout_ms`, and an open whose measured time, the sum of the phases
//! `ledger_opened` reports, exceeds `open_timeout_ms` × `open_warn_ratio_milli` / 1000 gives one
//! `ledger_open_slow` (warning; `elapsed_ms`, `timeout_ms`, `event_count`), which actingd writes
//! right after `ledger_opened`.
//!
//! A slow open is injected by the ledger's open delay gate: the open's SQL read starts as if the
//! delay had already passed, so its deadline comes that much sooner and its measured time is that
//! much longer; nothing waits the delay out.

use super::*;
use actingcommand_contract::RuntimeConfigManifest;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_ledger::LedgerOpenDelayGate;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

/// The configured deadline of a short open, in milliseconds.
const SHORT_TIMEOUT_MS: u64 = 10_000;

/// The phases of an open whose sum is its measured time.
const PHASES: [&str; 4] = ["sql_read_ms", "verify_ms", "material_ms", "restore_ms"];

// The configuration a start reads, with `section` as its `ledger_observer` section.
fn manifest(section: Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "ledger_observer": section }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn start(root: &TempDir, section: Option<Value>) -> RuntimeHostResult<RuntimeHost> {
    let host_config = match section {
        Some(section) => config(root).with_config_manifest(manifest(section)),
        None => config(root),
    };
    RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
}

fn integer(outcome: &Outcome, key: &str) -> u64 {
    outcome
        .value(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{key} is an integer: {outcome:?}"))
}

// An open slower than its configured deadline fails the start with the read budget's deadline
// failure, and the open's measures, its `ledger_open_measured` note, carry the configured
// deadline.
#[test]
fn p0_lo_an_open_past_its_configured_deadline_fails_and_reports_that_deadline() {
    let root = TempDir::new().expect("tempdir");
    let slow =
        LedgerOpenDelayGate::arm(root.path(), Duration::from_millis(SHORT_TIMEOUT_MS + 1_000))
            .expect("open delay gate");
    let failed = start(&root, Some(json!({"open_timeout_ms": SHORT_TIMEOUT_MS})))
        .err()
        .expect("an open past its deadline fails the start");
    drop(slow);
    let outcome = failed.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "ledger_read_budget_exceeded",
        "{outcome:?}"
    );
    let measured = outcome
        .causes()
        .iter()
        .find(|link| {
            link.relation().as_str() == "note"
                && link.outcome().code().as_str() == "ledger_open_measured"
        })
        .map(|link| link.outcome().clone())
        .unwrap_or_else(|| panic!("a ledger_open_measured note: {outcome:?}"));
    assert_eq!(integer(&measured, "timeout_ms"), SHORT_TIMEOUT_MS);
}

// An open under its warning line (the defaults: 600 000 ms and 500) gives no `ledger_open_slow`;
// its `ledger_opened` reports the configured deadline.
#[test]
fn p0_lo_an_open_under_its_warning_line_gives_no_ledger_open_slow() {
    let root = TempDir::new().expect("tempdir");
    let host = start(&root, None).expect("runtime host");
    let opened = host.ledger_opened().clone();
    let slow = host.ledger_open_slow().cloned();
    host.close().expect("close the host");
    assert_eq!(integer(&opened, "timeout_ms"), 600_000, "{opened:?}");
    assert_eq!(slow, None);
}

// An open over its warning line but within its deadline starts, and gives exactly one
// `ledger_open_slow` with the open's measured time, its configured deadline and its events.
#[test]
fn p0_lo_an_open_over_its_warning_line_gives_one_ledger_open_slow() {
    let root = TempDir::new().expect("tempdir");
    // 10 000 ms × 500 / 1000: the warning line is 5 000 ms; the open is 6 000 ms slower.
    let slow = LedgerOpenDelayGate::arm(root.path(), Duration::from_millis(6_000))
        .expect("open delay gate");
    let host = start(
        &root,
        Some(json!({"open_timeout_ms": SHORT_TIMEOUT_MS, "open_warn_ratio_milli": 500})),
    )
    .expect("an open within its deadline starts");
    drop(slow);
    let opened = host.ledger_opened().clone();
    let warning = host
        .ledger_open_slow()
        .cloned()
        .expect("one ledger_open_slow");
    host.close().expect("close the host");
    assert_eq!(warning.code().as_str(), "ledger_open_slow");
    assert_eq!(warning.category(), Category::Warning);
    let measured = PHASES
        .iter()
        .map(|phase| integer(&opened, phase))
        .sum::<u64>();
    assert!(measured > 5_000, "{opened:?}");
    assert_eq!(integer(&warning, "elapsed_ms"), measured, "{warning:?}");
    assert_eq!(integer(&warning, "timeout_ms"), SHORT_TIMEOUT_MS);
    assert_eq!(
        integer(&warning, "event_count"),
        integer(&opened, "event_count")
    );
    assert!(warning.causes().is_empty(), "{warning:?}");
}
