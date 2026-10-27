// SPDX-License-Identifier: AGPL-3.0-only

//! #422 IDLE-OV (v0.12.19; brief rd-1027/brief-idle-overview.md, investigation
//! rd-1027/q-idle-overview-budget.md §2). On production v0.12.18, 10-27 18:55, an overview read
//! and an `ac_observe` read-back were refused with `ledger_read_budget_exceeded`, relayed by
//! QueryEvents as `protocol_invalid` / `denied`: a read the writer did not finish was labelled a
//! protocol error. The required state, the host's part:
//! - I-5: a QueryEvents page whose read its budget stopped outside the page's selection answers
//!   `failed` with the ledger's code and `runtime_code` `runtime_unavailable`, as every other
//!   Runtime read relays an unanswered read (#411 A51 R3, `ledger_read_failure`); nothing is
//!   latched, and the same read answers once the writer's clock is calm again.
//!
//! The writer's clock is the test's: it moves 1 ms each time it is read, and two read budgets a
//! look while the refused read runs, so that read's budget is spent at its first look, inside
//! its first statement; the bound looks every 100 SQLite steps
//! (`ledger_observer.read_check_vm_steps`'s least value). Nothing sleeps.

use super::*;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, RuntimeConfigManifest,
};
use actingcommand_ledger::{LedgerCode, LedgerMonotonicClock};

/// The read budget (`ledger_observer.read_budget_ms`'s default).
const READ_BUDGET_MS: u64 = 1_000;
/// `ledger_observer.read_check_vm_steps`'s least value.
const CHECK_VM_STEPS: i64 = 100;

/// A start as actingd makes one, with `ledger_observer.read_check_vm_steps` set.
fn manifest() -> RuntimeConfigManifest {
    let mut parameters = vec![
        ConfigParameter {
            key: "bind_port".to_owned(),
            value: FactScalar::Integer(0),
            source: ConfigParameterSource::Default,
            settable: false,
        },
        ConfigParameter {
            key: "ledger_observer.read_check_vm_steps".to_owned(),
            value: FactScalar::Integer(CHECK_VM_STEPS),
            source: ConfigParameterSource::Explicit,
            settable: false,
        },
    ];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn query_events(ids: &IdentifierIssuer) -> RuntimeRequest {
    runtime_request(
        ids,
        RuntimeOperation::QueryEvents {
            query: EventQuery::default(),
            profile: ProjectionProfile::Forensic,
            page: RuntimeEventQueryPageRequest::new(10, None).expect("a first page"),
        },
    )
}

#[test]
fn idle_ov_i5_a_budget_overrun_on_query_events_answers_ledger_read_failure() {
    let root = TempDir::new().expect("tempdir");
    let now = Arc::new(AtomicU64::new(0));
    let step = Arc::new(AtomicU64::new(1));
    let clock: LedgerMonotonicClock = {
        let now = Arc::clone(&now);
        let step = Arc::clone(&step);
        Arc::new(move || now.fetch_add(step.load(Ordering::SeqCst), Ordering::SeqCst))
    };
    let host = RuntimeHost::start(
        config(&root)
            .with_config_manifest(manifest())
            .with_frame_retention_enabled(false)
            .with_ledger_read_budget_for_test(Duration::from_millis(READ_BUDGET_MS), clock),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    // The page's read spends its budget at its first look at the clock.
    step.store(2 * READ_BUDGET_MS, Ordering::SeqCst);
    let refused = host.process_request_for_test(
        &query_events(&ids),
        ConnectionId::new(1).expect("connection"),
    );
    step.store(1, Ordering::SeqCst);
    let refused = refused.expect("a receipt");
    assert_eq!(refused.state(), RuntimeReceiptState::Failed, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        LedgerCode::LedgerReadBudgetExceeded.as_str(),
        "{refused:?}"
    );
    assert_eq!(
        refused.error_projection().map(|projection| projection.code),
        Some(RuntimeErrorCode::RuntimeUnavailable),
        "{refused:?}"
    );
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );

    // The same read, its budget unspent, answers.
    let answered = host
        .process_request_for_test(
            &query_events(&ids),
            ConnectionId::new(2).expect("connection"),
        )
        .expect("a receipt");
    assert_eq!(
        answered.state(),
        RuntimeReceiptState::Completed,
        "{answered:?}"
    );
    host.close().expect("close host");
}
