// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-4 G4b (exec key 12), the Runtime half: the Runtime's admission of a run
// judges the shared fixture set EXEC-I6 (`tests/support/exec_i6.rs`) with the same shared
// `PrerequisiteChain` the offline `package bundle --maintenance` uses, so each set the bundle
// refuses refuses the run of its root under the same code, before any input
// (`contracts/package-reference.md`, "Maintenance declarations (bundle v3)": "Host retains
// loading, deadlines, failure facts and execution, consuming this same predicate
// incrementally"). The offline half is resource-tooling's `gate_e4`. Requirement basis:
// rd-1009/tests/req-basis-el.md section E-4, row "G4b".

// The one module of the shared fixture in this crate's tests; gate_336_pkgs4 uses it from here
// (clippy `duplicate_mod`).
#[allow(dead_code)]
#[path = "../../../../tests/support/exec_i6.rs"]
pub(super) mod exec_i6;

use super::*;
use actingcommand_contract::ContainedTaskRecoveryBinding;
use exec_i6::{I6Case, ROOT};
use std::path::Path;

const ALIAS: &str = "neutral.instance";

// A Runtime whose prerequisite packages are the packages of `case.bound`, each bound to its
// fixture directory, as actingd installs a bundle's `prerequisite` declarations.
fn start(temp: &TempDir, state: &Arc<FakeState>, packs_root: &Path, case: &I6Case) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    let bindings = case
        .bound
        .iter()
        .map(|package_id| {
            let binding = ContainedTaskRecoveryBinding::new(
                packs_root.join(package_id).display().to_string(),
            )
            .expect("prerequisite binding");
            ((*package_id).to_owned(), binding)
        })
        .collect::<BTreeMap<_, _>>();
    RuntimeHost::start(
        config(temp).with_prerequisite_packages(bindings),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host")
}

// Runs the root package of the set from its fixture directory.
fn run_root(host: &RuntimeHost, packs_root: &Path) -> RuntimeReceipt {
    let task_request = ContainedTaskRequest::new(packs_root.join(ROOT).display().to_string())
        .expect("task request");
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    client.send(&request)
}

// package-reference.md: the Runtime consumes the shared predicate that owns "game/server/
// resolution compatibility, cycle detection, three declared layers"; resource-tooling's
// `gate_e4` refuses the same sets offline under the same codes. Each refused set of EXEC-I6
// refuses the run of its root with the chain's code, as a denied request with no input.
#[test]
fn gate_e4_runtime_refuses_each_fixture_chain_with_the_bundles_code() {
    for case in exec_i6::refused_cases() {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let packs_root = temp.path().join("packs");
        exec_i6::write_case(&packs_root, &case);
        let host = start(&temp, &state, &packs_root, &case);

        let receipt = run_root(&host, &packs_root);

        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{}: {receipt:?}",
            case.name
        );
        assert_eq!(
            receipt.outcome().code().as_str(),
            case.code,
            "{}: {receipt:?}",
            case.name
        );
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            0,
            "{}: no input",
            case.name
        );
        host.close().expect("close runtime host");
    }
}
