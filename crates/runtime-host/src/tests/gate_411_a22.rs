// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A22 (gate key host 117, register §1.124): a direct run (`run_contained_task`, as CLI or
//! console task-run and MCP `ac_run_pack` send it) that binds a recovery package no path of the
//! run can use is denied in its preparation with `contained_task_recovery_binding_unused`: no
//! `task.*` record, no lease, no input. A request's recovery package is used only by (a) the home
//! entry of a package whose entry is checked and that has no prerequisite chain, and (b) the
//! stuck-recovery ladder, which a failed direct run starts only under
//! `recovery_ladder.rules.direct_runs = trigger` with the instance's stuck recovery on. A request
//! without a binding and a binding (a) or (b) can use run as before. Every expected value comes
//! from the slice's requirement lines (`contracts/linear-steps.md`, "Prerequisite packages").
//!
//! The page-graph packages are those of NAV-3a (`gate_373_nav3a`: the fake device's first pixel
//! is the screen, red `away`, blue `menu`, white `done`); the unchecked entry is the support
//! package `neutral_non_home_start_contained_task_package` (no entry page).

use super::gate_373_nav3a::{
    AWAY, LEAD_STEPS, MAIN, MENU, OPEN, Pack, StandardRoot, route, screens,
};
use super::gate_387_d1c::ladder_manifest;
use super::*;
use crate::host::{UnusedRecoveryBinding, recovery_binding_unused_error, recovery_binding_use};
use actingcommand_contract::ConfigSettingChange;
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
/// NAV-3a's `done` screen (white).
const DONE: [u8; 3] = [255, 255, 255];
const LEAD: &str = "neutral.test.lead";
const RECOVERY: &str = "neutral.test.recovery";
/// The support package's `package_id` (`neutral_non_home_start_contained_task_package`).
const UNCHECKED: &str = "neutral.semantic.task";
const UNUSED: &str = "contained_task_recovery_binding_unused";

struct DirectRun {
    receipt: RuntimeReceipt,
    events: Vec<ProjectedEvent>,
}

impl DirectRun {
    fn count(&self, event_type: EventType) -> usize {
        self.events
            .iter()
            .filter(|event| event.event_type == event_type)
            .count()
    }

    fn facts(&self) -> Vec<&TaskSemanticFact> {
        self.events
            .iter()
            .filter_map(projected_task_semantic_fact)
            .collect()
    }

    fn succeeded(&self) -> bool {
        self.receipt.state() == RuntimeReceiptState::Completed
            && matches!(
                self.receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    ..
                })
            )
    }
}

/// A host with `config` and the one fake physical instance `ALIAS` on `state`.
fn start(state: &Arc<FakeState>, config: RuntimeHostConfig) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        config,
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host")
}

/// Runs the package at `package` directly, binding the recovery package at `recovery` when
/// given; the receipt and the records of the request's correlation.
fn run_direct(host: &RuntimeHost, package: &Path, recovery: Option<&Path>) -> DirectRun {
    let mut task_request =
        ContainedTaskRequest::new(package.display().to_string()).expect("task request");
    if let Some(recovery) = recovery {
        task_request = task_request
            .with_recovery(
                ContainedTaskRecoveryBinding::new(recovery.display().to_string())
                    .expect("recovery binding"),
            )
            .expect("task request with recovery");
    }
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let receipt = client.send(&request);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    drop(client);
    DirectRun { receipt, events }
}

/// The refusal of requirement 1: denied, `contained_task_recovery_binding_unused` at
/// `check_recovery_binding` over `invalid_request`, `package_id` the run's package; no
/// `task.requested`, no `lease.requested` or `lease.granted`, no task record and no input.
fn assert_refused(run: &DirectRun, state: &FakeState, package_id: &str) {
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Denied,
        "{:?}",
        run.receipt
    );
    let outcome = run.receipt.outcome();
    assert_eq!(outcome.code().as_str(), UNUSED, "{outcome:?}");
    assert_eq!(
        outcome.value("package_id").and_then(Value::as_str),
        Some(package_id),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("operation").and_then(Value::as_str),
        Some("check_recovery_binding"),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("runtime_code"),
        Some(&json!("invalid_request")),
        "{outcome:?}"
    );
    assert_eq!(run.count(EventType::TaskRequested), 0);
    assert_eq!(run.count(EventType::LeaseRequested), 0);
    assert_eq!(run.count(EventType::LeaseGranted), 0);
    assert!(run.facts().is_empty(), "no task record: {:?}", run.facts());
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
}

/// The unchecked support package at `<root>/unchecked.zip`, on a fake device whose capture moves
/// on after the input (`home` then `terminal`).
fn unchecked_package(root: &TempDir, state: &FakeState) -> PathBuf {
    let path = root.path().join("unchecked.zip");
    fs::write(&path, neutral_non_home_start_contained_task_package(true))
        .expect("write unchecked package");
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    path
}

/// Requirement 1, reason `entry_not_checked`: the unchecked package with a binding under the
/// default ladder rules (`direct_runs = exempt`) is refused before any lease.
#[test]
fn a22_an_unchecked_entry_with_a_recovery_binding_is_refused_before_any_lease() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = unchecked_package(&root, &state);
    let host = start(&state, config(&root));

    let unused = root.path().join("unused.zip");
    let run = run_direct(&host, &package, Some(unused.as_path()));
    host.close().expect("close runtime host");

    assert_refused(&run, &state, UNCHECKED);
}

/// Requirement 2, (a): an entry-set package started off its entry, with a binding, is not
/// refused; its home entry admits the bound package (no role) and the run goes on from `menu`.
#[test]
fn a22_a_checked_entry_still_uses_its_recovery_binding() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU, DONE]);
    let recovery = root.path().join("recovery");
    route(RECOVERY, "away", "menu", LEAD_STEPS, None).write(&recovery);
    let package = root.path().join("main");
    Pack {
        package_id: MAIN,
        mode: "navigable_route",
        prerequisite: None,
        entry: json!(["menu"]),
        target: "done",
        operations: OPEN,
    }
    .write(&package);
    let host = start(&state, config(&root));

    let run = run_direct(&host, &package, Some(recovery.as_path()));
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    let admitted = run
        .facts()
        .into_iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_label,
                role,
            } => Some((package_label.clone(), *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(admitted, vec![(RECOVERY.to_owned(), None)]);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
}

/// Requirement 1, reason `prerequisite_gate`: a page-graph package that declares a prerequisite
/// package of its game's standard pack, with a binding, is refused before any lease.
#[test]
fn a22_a_prerequisite_gate_refuses_a_recovery_binding() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU, DONE]);
    let mut packs = StandardRoot::new(root.path());
    route(LEAD, "away", "menu", LEAD_STEPS, None).write(&packs.list(LEAD));
    packs.write_bundle();
    let recovery = root.path().join("recovery");
    route(RECOVERY, "away", "menu", LEAD_STEPS, None).write(&recovery);
    let package = root.path().join("main");
    route(MAIN, "menu", "done", OPEN, Some(LEAD)).write(&package);
    let host = start(
        &state,
        config(&root).with_standard_packs_root(Some(packs.root.clone())),
    );

    let run = run_direct(&host, &package, Some(recovery.as_path()));
    host.close().expect("close runtime host");

    assert_refused(&run, &state, MAIN);
}

/// Requirement 2, (b): the request of the first test after `ConfigureSettings` sets
/// `recovery_ladder.rules.direct_runs` to `trigger`, on the fake physical instance whose stuck
/// recovery is on (`recovery_ladder.enabled`, shipped `true`): not refused, the run goes on and
/// completes as without the binding.
#[test]
fn a22_the_ladder_rule_trigger_accepts_the_binding() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = unchecked_package(&root, &state);
    let host = start(
        &state,
        config(&root).with_config_manifest(ladder_manifest(json!({}))),
    );
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let configured = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ConfigureSettings {
                    changes: vec![ConfigSettingChange {
                        key: "recovery_ladder.rules.direct_runs".to_owned(),
                        value: Some(json!("trigger")),
                    }],
                },
            ),
            ConnectionId::new(411).expect("connection"),
        )
        .expect("configure receipt");
    assert_eq!(
        configured.state(),
        RuntimeReceiptState::Completed,
        "{configured:?}"
    );

    let unused = root.path().join("unused.zip");
    let run = run_direct(&host, &package, Some(unused.as_path()));
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(run.count(EventType::TaskRequested), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
}

/// Requirement 2: the unchecked package without a binding completes as before, with no entry
/// check.
#[test]
fn a22_a_run_without_a_binding_is_unchanged() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = unchecked_package(&root, &state);
    let host = start(&state, config(&root));

    let run = run_direct(&host, &package, None);
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(run.count(EventType::TaskEntryPreflight), 0);
}

/// Requirement 1, the named function: (a) is "no prerequisite chain and a checked entry", (b) is
/// "a failed direct run may start a ladder"; the binding is refused only when both are false, by
/// `prerequisite_gate` when the chain is not empty and `entry_not_checked` otherwise. Each reason
/// travels as the native detail `reason=<why>` of the refusal, with `package_id`, at
/// `check_recovery_binding` over `invalid_request`.
#[test]
fn a22_the_binding_is_refused_only_when_neither_the_entry_nor_the_ladder_can_use_it() {
    use UnusedRecoveryBinding::{EntryNotChecked, PrerequisiteGate};
    // (prerequisite chain, entry checked, ladder) -> the refusal's reason, `None` when used.
    let rows = [
        ((false, true, false), None),
        ((false, true, true), None),
        ((false, false, true), None),
        ((true, true, true), None),
        ((true, false, true), None),
        ((false, false, false), Some(EntryNotChecked)),
        ((true, true, false), Some(PrerequisiteGate)),
        ((true, false, false), Some(PrerequisiteGate)),
    ];
    for ((chain, checked, ladder), expected) in rows {
        assert_eq!(
            recovery_binding_use(chain, checked, ladder),
            expected,
            "chain {chain}, entry checked {checked}, ladder {ladder}"
        );
    }
    for (unused, detail) in [
        (EntryNotChecked, "reason=entry_not_checked"),
        (PrerequisiteGate, "reason=prerequisite_gate"),
    ] {
        let error = recovery_binding_unused_error(MAIN, unused);
        assert_eq!(error.code(), UNUSED);
        assert!(!error.is_fatal());
        assert_eq!(error.native_detail(), Some(detail));
        let outcome = error.receipt_outcome();
        assert_eq!(outcome.code().as_str(), UNUSED);
        assert_eq!(
            outcome.value("package_id").and_then(Value::as_str),
            Some(MAIN)
        );
        assert_eq!(
            outcome.value("operation").and_then(Value::as_str),
            Some("check_recovery_binding")
        );
        assert_eq!(
            outcome.value("runtime_code"),
            Some(&json!("invalid_request"))
        );
    }
}
