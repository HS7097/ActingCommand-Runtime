// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #378 A6a (model sections 2.3, 7.4; host gate 65): the latch rule a later request is
// answered by, the `ledger_opened` a start reports with its open's phases, and the previous
// owner a start released, as the outcomes actingd writes as daemon lines.

use super::*;
use crate::error::FatalState;
use actingcommand_contract::outcome::Category;

// The values `ledger_opened` carries (model section 3.3, "Ledger open", N1).
const OPEN_KEYS: [&str; 9] = [
    "event_count",
    "artifact_count",
    "artifact_bytes",
    "worker_count",
    "sql_read_ms",
    "verify_ms",
    "material_ms",
    "restore_ms",
    "timeout_ms",
];

// M1a: a latched error condition answers under `runtime_halted`, the condition its `caused_by`
// link, with the halt and the condition's Runtime error code; a fatal condition answers as
// itself, so the fatal-link rule holds.
#[test]
fn a6a_a_latched_host_answers_a_later_request_by_its_condition() {
    let latched = FatalState::default();
    let condition = RuntimeHostError::fatal(
        HostCode::InstallStartupStopped,
        actingcommand_contract::codes::ContractLocation::InstallTransition,
        RuntimeErrorCode::LedgerFailure,
    );
    latched
        .mark(condition.clone())
        .expect("latch the condition");
    let answer = latched
        .answer()
        .expect("read the latch")
        .expect("a latched condition");
    let outcome = answer.receipt_outcome();
    assert_eq!(outcome.code().as_str(), "runtime_halted");
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(
        outcome.value("runtime_code"),
        Some(&serde_json::json!("ledger_failure"))
    );
    assert_eq!(outcome.value("host_halted"), Some(&serde_json::json!(true)));
    let [link] = outcome.causes() else {
        panic!("one link: {outcome:?}")
    };
    assert_eq!(link.relation().as_str(), "caused_by");
    assert_eq!(link.outcome(), &condition.outcome());

    let latched = FatalState::default();
    let fatal = RuntimeHostError::relayed_fatal(
        "ledger_format_unsupported",
        "open_global_ledger",
        RuntimeErrorCode::LedgerFailure,
    );
    latched
        .mark(fatal.clone())
        .expect("latch the fatal condition");
    let outcome = latched
        .answer()
        .expect("read the latch")
        .expect("a latched condition")
        .receipt_outcome();
    assert_eq!(outcome.code().as_str(), "ledger_format_unsupported");
    assert_eq!(outcome.category(), Category::Fatal);
    assert_eq!(outcome, fatal.receipt_outcome());
}

// N1: a start's ledger open is `ledger_opened` (info) with every phase as an integer value and
// the open's deadline as `timeout_ms`; the host prints no line of it.
#[test]
fn a6a_a_start_reports_its_ledger_open_as_ledger_opened() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let opened = host.ledger_opened().clone();
    host.close().expect("close the host");
    assert_eq!(opened.code().as_str(), "ledger_opened");
    assert_eq!(opened.category(), Category::Info);
    for key in OPEN_KEYS {
        assert!(
            opened.value(key).is_some_and(serde_json::Value::is_u64),
            "{key}: {opened:?}"
        );
    }
    assert!(
        opened
            .value("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|timeout_ms| timeout_ms > 0),
        "{opened:?}"
    );
    assert!(opened.causes().is_empty(), "{opened:?}");
}

// The previous owner a start released is `owner_released` (info) with its process and when it
// started.
#[test]
fn a6a_a_released_previous_owner_is_owner_released_with_its_pid() {
    let released = crate::PriorOwnerReleasedByExit {
        pid: 4_242,
        started_at_unix_ms: 1_800_000_000_000,
        last_disposition: actingcommand_contract::OwnerResourceDisposition::InUse,
    };
    let outcome = released.outcome();
    assert_eq!(outcome.code().as_str(), "owner_released");
    assert_eq!(outcome.category(), Category::Info);
    assert_eq!(outcome.value("owner_pid"), Some(&serde_json::json!(4_242)));
    assert_eq!(
        outcome.value("at_unix_ms"),
        Some(&serde_json::json!(1_800_000_000_000_u64))
    );
}
