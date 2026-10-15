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

use super::gate_373_nav3a::StandardRoot;
use super::*;
use exec_i6::{I6Case, ROOT};
use std::path::Path;

const ALIAS: &str = "neutral.instance";

// Workflow #373 NAV-3a (M-373 §4.3): the packages of `case.bound` are the packages the game's
// standard-pack registry lists (with the root package, so the bundle lists one at least); the
// others sit outside it. Gives the standard-pack root and the root package's directory.
fn write_case(temp: &TempDir, case: &I6Case) -> (PathBuf, PathBuf) {
    let mut registry = StandardRoot::new(temp.path());
    let mut root_package = None;
    for pack in &case.packs {
        let directory = if pack.package_id == ROOT || case.bound.contains(&pack.package_id) {
            registry.list(pack.package_id)
        } else {
            temp.path().join("packs").join(pack.package_id)
        };
        exec_i6::write_pack(&directory, pack);
        if pack.package_id == ROOT {
            root_package = Some(directory);
        }
    }
    registry.write_bundle();
    (registry.root, root_package.expect("the root package"))
}

// A Runtime whose standard-pack root is `registry`.
fn start(temp: &TempDir, state: &Arc<FakeState>, registry: PathBuf) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        config(temp).with_standard_packs_root(Some(registry)),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host")
}

// Runs the root package of the set from its fixture directory.
fn run_root(host: &RuntimeHost, root_package: &Path) -> RuntimeReceipt {
    let task_request =
        ContainedTaskRequest::new(root_package.display().to_string()).expect("task request");
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
        let (registry, root_package) = write_case(&temp, &case);
        let host = start(&temp, &state, registry);

        let receipt = run_root(&host, &root_package);

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
