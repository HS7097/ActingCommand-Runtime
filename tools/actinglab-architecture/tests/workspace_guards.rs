// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use actingcommand_actinglab_architecture::{
    contract_dependency_violations, extract_command_inventory, inspect_contract_fact_matching,
    inspect_generic_authoring_identity, inspect_generic_runtime_identity,
    inspect_global_append_ingress, inspect_lab_source, inspect_persisted_event_ownership,
    inspect_producer_event_capabilities, inspect_public_api, lab_removability_violations,
    ledger_owns_query_matching, resource_tooling_removability_violations, validate_line_ratchet,
    workspace_dependency_violations,
};
use sha2::{Digest, Sha256};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("architecture tool must live at tools/<name>")
        .to_path_buf()
}

const GENERIC_RUNTIME_OWNED_ROOTS: &[&str] = &[
    "apps/actingctl",
    "apps/actingd",
    "benchmarks/workloads",
    "contracts",
    "crates/actingcommand-contract",
    "crates/artifact-store",
    "crates/device",
    "crates/execution-kernel",
    "crates/host-metrics",
    "crates/ledger",
    "crates/onnx-provider-support",
    "crates/pack-containment",
    "crates/page-detector",
    "crates/policy",
    "crates/recognition",
    "crates/recognition-pack",
    "crates/runtime-client",
    "crates/runtime-host",
    "crates/runtime-state",
    "crates/scheduler",
    "crates/vision-ffi",
    "tests",
];

#[test]
fn a7_interface_amendment_matches_declared_freeze() {
    assert_frozen_payload(
        "docs/architecture/actinglab-a7-interface-amendment.md",
        "<!-- A7-INTERFACE-FREEZE-BEGIN -->\n",
        "<!-- A7-INTERFACE-FREEZE-END -->",
        "A7 interface amendment",
    );
}

#[test]
fn issue33_chain_amendment_matches_declared_freeze() {
    assert_frozen_payload(
        "docs/architecture/actinglab-chain-amendment-20260710.md",
        "<!-- ISSUE33-CHAIN-FREEZE-BEGIN -->\n",
        "<!-- ISSUE33-CHAIN-FREEZE-END -->",
        "issue 33 chain amendment",
    );
}

#[test]
fn issue35_c0_architecture_matches_declared_freeze() {
    assert_frozen_payload(
        "docs/architecture/runtime-ledger-v3-c0-freeze.md",
        "<!-- RUNTIME-LEDGER-V3-C0-FREEZE-BEGIN -->\n",
        "<!-- RUNTIME-LEDGER-V3-C0-FREEZE-END -->",
        "issue 35 C0 architecture",
    );
}

fn assert_frozen_payload(path: &str, begin: &str, end: &str, label: &str) {
    let source = fs::read_to_string(workspace_root().join(path))
        .unwrap_or_else(|error| panic!("read {label}: {error}"));
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let declared = normalized
        .lines()
        .find_map(|line| {
            line.strip_prefix("Frozen payload SHA-256: `")
                .and_then(|value| value.strip_suffix('`'))
        })
        .unwrap_or_else(|| panic!("{label} declares frozen payload SHA-256"));
    let payload = normalized
        .split_once(begin)
        .and_then(|(_, tail)| tail.split_once(end).map(|(payload, _)| payload))
        .unwrap_or_else(|| panic!("{label} contains freeze markers"));
    let actual = format!("{:x}", Sha256::digest(payload.as_bytes()));

    assert_eq!(actual, declared, "{label} freeze drifted");
}

#[test]
fn lab_source_obeys_dependency_law_or_placeholder_is_consistent() {
    let root = workspace_root();
    let lab_root = root.join("crates/lab");
    if !lab_root.exists() {
        let workspace_manifest =
            fs::read_to_string(root.join("Cargo.toml")).expect("read workspace Cargo.toml");
        assert!(
            !workspace_manifest.contains("\"crates/lab\""),
            "workspace registers crates/lab before the crate exists"
        );
        return;
    }

    let mut files = Vec::new();
    collect_rust_files(&lab_root, &mut files);
    assert!(
        !files.is_empty(),
        "crates/lab contains no Rust source files"
    );
    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        violations.extend(inspect_lab_source(&display, &source).unwrap());
        violations.extend(inspect_public_api(&display, &source).unwrap());
    }
    assert!(
        violations.is_empty(),
        "crates/lab dependency-law violations:\n{}",
        violations.join("\n")
    );
}

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(root).unwrap_or_else(|err| panic!("read directory {}: {err}", root.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("read {} entry: {err}", root.display()));
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn c2_runtime_code_contracts_defaults_and_fixtures_are_project_neutral() {
    let root = workspace_root();
    let mut files = Vec::new();
    for owned_root in GENERIC_RUNTIME_OWNED_ROOTS {
        collect_generic_runtime_files(&root.join(owned_root), &mut files);
    }

    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        violations.extend(inspect_generic_runtime_identity(&display, &source));
    }

    assert!(
        violations.is_empty(),
        "C2 generic Runtime boundary violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn c2_runtime_guard_covers_policy_and_runtime_owned_core_siblings() {
    for required_root in [
        "crates/host-metrics",
        "crates/policy",
        "crates/runtime-state",
    ] {
        assert!(
            GENERIC_RUNTIME_OWNED_ROOTS.contains(&required_root),
            "C2 generic Runtime guard does not cover {required_root}"
        );
        let counterexample = "const SERVER_BA: &str = \"neutral\";";
        let violations = inspect_generic_runtime_identity(
            &format!("{required_root}/src/lib.rs"),
            counterexample,
        );
        assert!(
            !violations.is_empty(),
            "C2 counterexample escaped in {required_root}"
        );
    }
}

#[test]
fn r2f_product_and_authoring_paths_have_no_builtin_game_identity() {
    let root = workspace_root();
    let owned_roots = [
        "apps/actinglab/src",
        "apps/device-test/src",
        "crates/lab/src",
        "crates/resource-tooling/src",
    ];
    let mut files = Vec::new();
    for owned_root in owned_roots {
        collect_rust_files(&root.join(owned_root), &mut files);
    }

    let mut violations = Vec::new();
    for path in files {
        if path.file_name().is_some_and(|name| name == "tests.rs")
            || path
                .components()
                .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        violations.extend(inspect_generic_authoring_identity(&display, &source).unwrap());
    }

    assert!(
        violations.is_empty(),
        "R2-F generic authoring boundary violations:\n{}",
        violations.join("\n")
    );
}

fn collect_generic_runtime_files(root: &Path, files: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(root).unwrap_or_else(|error| panic!("read {}: {error}", root.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("read {} entry: {error}", root.display()));
        let path = entry.path();
        if path.is_dir() {
            collect_generic_runtime_files(&path, files);
            continue;
        }
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension,
                    "rs" | "json" | "toml" | "yaml" | "yml" | "sql" | "md"
                )
            })
        {
            files.push(path);
        }
    }
}

#[test]
fn c3b_client_device_authority_stays_behind_runtime() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rust_files(&root.join("apps/actingctl/src"), &mut files);
    collect_rust_files(&root.join("crates/runtime-client/src"), &mut files);
    files.push(root.join("apps/actinglab/src/runtime_slice_cli.rs"));
    let forbidden = [
        "create_touch_backend",
        "create_capture_backend",
        "touch_probe_report",
        "MaaTouchBackend",
        "MinitouchBackend",
        "AdbShellInputBackend",
        "ScreencapBackend",
        "CaptureBackend",
        "DeviceTarget",
    ];
    let mut violations = Vec::new();
    for path in files {
        if path.file_name().is_some_and(|name| name == "tests.rs") {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for token in forbidden {
            if source.contains(token) {
                violations.push(format!(
                    "{display}: client constructs device authority via {token}"
                ));
            }
        }
    }
    let manifest = fs::read_to_string(root.join("apps/actinglab/Cargo.toml"))
        .expect("read ActingLab manifest");
    assert!(
        manifest.contains("actingcommand-runtime-client"),
        "ActingLab must depend on the typed Runtime client"
    );

    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    for package_name in ["actingcommand-runtime-client", "actingcommand-actingctl"] {
        let package = packages
            .iter()
            .find(|package| package["name"] == package_name)
            .unwrap_or_else(|| panic!("missing package {package_name}"));
        for dependency in package["dependencies"]
            .as_array()
            .expect("package dependencies")
            .iter()
            .filter(|dependency| dependency["kind"].is_null())
        {
            let dependency_name = dependency["name"].as_str().expect("dependency name");
            if matches!(
                dependency_name,
                "actingcommand-device" | "actingcommand-recognition"
            ) {
                violations.push(format!(
                    "{package_name}: production dependency reaches {dependency_name}"
                ));
            }
        }
    }

    let runtime_contract =
        fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
            .expect("read Runtime contract");
    for retired in [
        "AdmitReadonly",
        "BeginReadonlyObservation",
        "FinishReadonlyObservation",
        "ReadOnlyAdmitted",
        "ReadonlyObservationBegun",
    ] {
        if runtime_contract.contains(retired) {
            violations.push(format!(
                "runtime contract still exposes retired client capture capability {retired}"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "C3b client authority violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn c6_actinglab_does_not_construct_live_device_backends() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rust_files(&root.join("apps/actinglab/src"), &mut files);
    let mut violations = Vec::new();
    for path in files {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for authority in [
            "Adb::new(",
            "create_capture_backend(",
            "create_touch_backend(",
            "touch_probe_report(",
            "MaaTouchBackend",
            "MinitouchBackend",
            "AdbShellInputBackend",
            "ScreencapBackend::",
            "DroidcastRawBackend",
            "NemuIpcBackend",
            ".launch_package(",
            ".force_stop(",
            "Command::new(\"adb\")",
        ] {
            if source.contains(authority) {
                violations.push(format!(
                    "{display}: ActingLab reaches live device authority via {authority}"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "ActingLab live-backend ownership violations:\n{}",
        violations.join("\n")
    );
    let input = fs::read_to_string(root.join("apps/actinglab/src/runtime_input_backend.rs"))
        .expect("read ActingLab Runtime input adapter");
    let capture = fs::read_to_string(root.join("apps/actinglab/src/runtime_capture_backend.rs"))
        .expect("read ActingLab Runtime capture adapter");
    let capture_production = capture
        .split_once("#[cfg(test)]")
        .map_or(capture.as_str(), |(production, _)| production);
    assert!(
        input.contains("proxy: RuntimeInputProxy")
            && input.contains("self.proxy.input(action)")
            && !input.contains("AdbConfig")
            && !input.contains("ExecutionBackendProvider"),
        "ActingLab input adapter must remain a Runtime proxy without provider authority"
    );
    assert!(
        capture_production.contains("client: RuntimeClient")
            && capture_production.contains("observe_readonly")
            && !capture_production.contains("ExecutionBackendProvider"),
        "ActingLab capture adapter must consume Runtime observations without provider authority"
    );
}

#[test]
fn c5_drive_decisions_are_owned_by_execution_kernel() {
    let root = workspace_root();
    let kernel = fs::read_to_string(root.join("crates/execution-kernel/src/drive.rs"))
        .expect("read execution-kernel drive source");
    let lab = fs::read_to_string(root.join("crates/lab/src/drive.rs"))
        .expect("read Lab drive adapter source");

    for required in [
        "pub struct DriveNavigationGraph",
        "pub enum DriveSemanticInput",
        "pub fn find_route",
        "pub fn validate_route",
        "pub fn validate_resolved_input",
    ] {
        assert!(
            kernel.contains(required),
            "execution-kernel lost drive decision owner {required}"
        );
    }
    for forbidden in [
        "std::fs",
        "RuntimeClient",
        "LabPorts",
        "InputBackend",
        "TouchBackend",
    ] {
        assert!(
            !kernel.contains(forbidden),
            "execution-kernel drive decision module reaches effect owner {forbidden}"
        );
    }
    for retired in [
        "struct NavigationGraph",
        "enum SemanticInput",
        "fn parse_navigation_edge",
        "fn find_navigation_route",
        "fn rects_intersect",
    ] {
        assert!(
            !lab.contains(retired),
            "Lab still duplicates migrated drive decision {retired}"
        );
    }
    assert!(
        lab.contains("DriveNavigationGraph as NavigationGraph"),
        "Lab adapter no longer consumes execution-kernel drive decisions"
    );
}

#[test]
fn c5_drive_effects_cross_only_runtime_ports() {
    let root = workspace_root();
    let kernel = fs::read_to_string(root.join("crates/execution-kernel/src/drive.rs"))
        .expect("read execution-kernel drive source");
    let lab = fs::read_to_string(root.join("crates/lab/src/drive.rs"))
        .expect("read Lab drive adapter source");
    let cli = fs::read_to_string(root.join("apps/actinglab/src/drive_cli.rs"))
        .expect("read ActingLab drive CLI source");
    let ports = fs::read_to_string(root.join("apps/actinglab/src/env_detection.rs"))
        .expect("read ActingLab Runtime port source");

    assert!(
        kernel.contains("pub fn resolved_input_action"),
        "execution-kernel must own semantic-to-runtime input planning"
    );
    for forbidden in [
        "input_factory()",
        "InputBackendRequest",
        "TouchBackendConfig",
        "combine_operation_and_close",
    ] {
        assert!(
            !lab.contains(forbidden),
            "Lab drive still opens or configures a production input backend via {forbidden}"
        );
    }
    for forbidden in [
        "device_config",
        "build_control_lab",
        "legacy_control_capture",
    ] {
        assert!(
            !cli.contains(forbidden),
            "ActingLab drive CLI still reaches legacy device authority via {forbidden}"
        );
    }
    for required in [
        "build_drive_lab",
        "AppSemanticInputExecutor",
        "RuntimeInputProxy::connect",
        "AppCaptureAuthority::Runtime",
    ] {
        assert!(
            ports.contains(required),
            "ActingLab drive Runtime port lost {required}"
        );
    }
}

#[test]
fn c5_production_run_ingress_requires_external_loaded_bundle() {
    let root = workspace_root();
    let bundle = fs::read_to_string(root.join("crates/execution-kernel/src/bundle.rs"))
        .expect("read execution bundle source");
    let contained = fs::read_to_string(root.join("crates/execution-kernel/src/contained_task.rs"))
        .expect("read contained task source");
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host source");
    let cli = fs::read_to_string(root.join("apps/actinglab/src/lab_run.rs"))
        .expect("read ActingLab run CLI source");

    for required in [
        "pub struct ExternalExpectedSha256",
        "pub struct ExternallyVerifiedBundle",
        "Containment::new()",
    ] {
        assert!(
            bundle.contains(required),
            "execution bundle ingress lost {required}"
        );
    }
    for forbidden in ["std::fs", "Sha256Hash::digest"] {
        assert!(
            !bundle.contains(forbidden),
            "execution bundle ingress can discover or self-trust resources via {forbidden}"
        );
    }
    assert!(
        contained.contains("ExternallyVerifiedBundle::load(instance_label, zip_bytes, expected)"),
        "Runtime contained task bypasses the externally verified bundle capability"
    );
    assert!(
        host.contains("ExternalExpectedSha256::parse_hex(request.expected_sha256())")
            && host.contains("PreparedContainedTask::load(instance_alias, &bytes, expected)"),
        "Runtime host must bind the client hash to contained package admission"
    );
    for forbidden in ["Sha256Hash::digest", "unwrap_or_else"] {
        assert!(
            !contained.contains(forbidden),
            "contained task ingress self-trusts its package via {forbidden}"
        );
    }
    assert!(
        cli.contains("required_expected_sha256")
            && cli.contains("ContainedTaskRequest::new")
            && cli.contains("run_contained_task(&instance, request)"),
        "ActingLab production run CLI does not require an external expected hash"
    );
}

#[test]
fn c5_offline_package_simulation_reuses_the_contained_task_kernel_without_device_authority() {
    let root = workspace_root();
    let main = fs::read_to_string(root.join("apps/actinglab/src/main.rs"))
        .expect("read ActingLab CLI source");
    let package_cli = fs::read_to_string(root.join("apps/actinglab/src/package_cli.rs"))
        .expect("read package CLI router source");
    let offline_cli = fs::read_to_string(root.join("apps/actinglab/src/package_offline.rs"))
        .expect("read package offline CLI source");
    let offline_kernel = fs::read_to_string(root.join("crates/execution-kernel/src/offline.rs"))
        .expect("read offline execution adapter source");
    let offline_kernel_production = offline_kernel
        .split("#[cfg(test)]")
        .next()
        .expect("offline execution adapter production source");
    let lab_run = fs::read_to_string(root.join("apps/actinglab/src/lab_run.rs"))
        .expect("read production Lab run CLI source");

    for required in [
        "\"dry-run\" => package_cli::run_offline(global, &flags)",
        "package_cli::offline_capability()",
    ] {
        assert!(
            main.contains(required),
            "ActingLab lost offline package route or capability {required}"
        );
    }
    for required in [
        "#[path = \"package_offline.rs\"]",
        "offline::run_dry_run(global, flags)",
        "offline::capability()",
    ] {
        assert!(
            package_cli.contains(required),
            "package CLI router lost offline route {required}"
        );
    }
    for required in [
        "validate_lab_package_bytes",
        "PreparedContainedTask::load",
        "simulate_contained_task",
        "mode: \"offline_simulation\"",
        "executed: false",
        "production_global_ledger_written: false",
    ] {
        assert!(
            offline_cli.contains(required),
            "offline package entry lost required binding {required}"
        );
    }
    for required in [
        "task.run(&mut runtime)",
        "OfflineBoundary::EffectIntercepted",
        "executed: false",
    ] {
        assert!(
            offline_kernel_production.contains(required),
            "offline adapter stopped delegating to the production contained-task kernel via {required}"
        );
    }
    for forbidden in [
        "RuntimeClient",
        "run_contained_task",
        "InputBackend",
        "DeviceTarget",
        "ScreencapBackend",
        "MaaTouchBackend",
        "MinitouchBackend",
        "NemuIpc",
        "Droidcast",
        "GlobalLedger",
        "LabLease",
        "actingcommand_scheduler",
    ] {
        assert!(
            !offline_cli.contains(forbidden),
            "offline package entry gained production authority via {forbidden}"
        );
    }
    for forbidden in [
        "actingcommand_runtime_client",
        "actingcommand_runtime_host",
        "actingcommand_scheduler",
        "actingcommand_ledger",
        "InputBackend",
        "CaptureBackend",
        "DeviceTarget",
    ] {
        assert!(
            !offline_kernel_production.contains(forbidden),
            "offline execution adapter gained external authority via {forbidden}"
        );
    }
    assert!(
        lab_run.contains("if global.dry_run")
            && lab_run.contains("explicit_offline_entry_required")
            && lab_run.contains("use package dry-run"),
        "production lab run must fail loud when the global dry-run flag is present"
    );
    assert!(
        main.contains("\"package run requires an exclusive_drain LabLease")
            && main.contains("\"lab_lease_required\""),
        "package run must remain a blocked compatibility boundary"
    );
    assert!(
        main.contains("\"operation run requires Runtime scheduler admission")
            && main.contains("\"lab_lease_required\""),
        "operation run must remain behind Runtime scheduler admission"
    );
}

#[test]
fn c5_recovery_state_machine_is_execution_owned() {
    let root = workspace_root();
    let recovery = fs::read_to_string(root.join("crates/execution-kernel/src/recovery.rs"))
        .expect("read execution recovery source");
    let lab_facade =
        fs::read_to_string(root.join("crates/lab/src/lib.rs")).expect("read Lab facade source");
    let compatibility = fs::read_to_string(root.join("apps/actinglab/src/recovery_exec.rs"))
        .expect("read ActingLab recovery compatibility source");

    for required in [
        "pub struct RecoveryGraph",
        "pub trait RecoveryRuntime",
        "pub fn execute_recovery_graph",
    ] {
        assert!(
            recovery.contains(required),
            "execution-kernel lost recovery owner {required}"
        );
    }
    for forbidden in [
        "actingcommand_lab",
        "actingcommand_runtime_client",
        "actingcommand_device::",
        "std::fs",
    ] {
        assert!(
            !recovery.contains(forbidden),
            "execution recovery core reached effect owner via {forbidden}"
        );
    }
    assert!(
        lab_facade.contains("pub use actingcommand_execution_kernel"),
        "Lab facade no longer re-exports execution-owned recovery primitives"
    );
    assert!(
        compatibility.contains("pub use actingcommand_lab"),
        "ActingLab recovery compatibility no longer delegates through the Lab facade"
    );
    for forbidden in [
        "pub struct RecoveryGraph",
        "pub trait RecoveryRuntime",
        "pub fn execute_recovery_graph",
        "fn validate_graph",
    ] {
        assert!(
            !compatibility.contains(forbidden),
            "ActingLab regained recovery state-machine ownership via {forbidden}"
        );
    }
}

#[test]
fn c5_run_state_machine_returns_data_only_successors() {
    let root = workspace_root();
    let run = fs::read_to_string(root.join("crates/execution-kernel/src/run.rs"))
        .expect("read execution run source");
    let lab_api = fs::read_to_string(root.join("crates/lab/src/lab_run/api.rs"))
        .expect("read Lab run adapter source");
    let lab_execute = fs::read_to_string(root.join("crates/lab/src/lab_run/execute.rs"))
        .expect("read Lab operation adapter source");
    let lab_bundle = fs::read_to_string(root.join("crates/lab/src/lab_run/bundle.rs"))
        .expect("read Lab run bundle source");

    for required in [
        "pub struct RunStateMachine",
        "pub enum RunOperationFailureDecision",
        "pub struct RunSuccessorSuggestion",
        "SuccessorSuggested",
        "PausedNeedsHuman",
    ] {
        assert!(run.contains(required), "execution run core lost {required}");
    }
    for forbidden in [
        "actingcommand_lab",
        "actingcommand_runtime_client",
        "actingcommand_scheduler",
        "actingcommand_ledger",
        "actingcommand_device",
        "std::fs",
        "InputBackend",
        "CaptureBackend",
        "enqueue(",
        "start_task(",
        "submit_task(",
    ] {
        assert!(
            !run.contains(forbidden),
            "execution run decisions gained side-effect authority via {forbidden}"
        );
    }
    for required in [
        "RunStateMachine::new",
        ".next_directive(&run_operations)",
        ".operation_succeeded(",
        ".operation_needs_recovery(",
        "successor_suggested",
    ] {
        assert!(
            lab_api.contains(required),
            "Lab run adapter no longer consumes execution-owned transition {required}"
        );
    }
    for forbidden in [
        "run_recovery_bundle(",
        ".load_operation_bundle(",
        "recovery_started",
        "recovery_result",
    ] {
        assert!(
            !lab_api.contains(forbidden),
            "Lab run adapter regained direct recovery chaining via {forbidden}"
        );
    }
    for forbidden in [
        "enum OperationFailureDecision",
        "fn operation_failure_decision",
        "fn pre_execution_guard_failure_decision",
        "fn select_operation_for_page",
    ] {
        assert!(
            !lab_execute.contains(forbidden),
            "Lab operation adapter regained run decision ownership via {forbidden}"
        );
    }
    assert!(
        !lab_bundle.contains("fn load_operation_bundle"),
        "Lab bundle adapter can still load and directly chain successor tasks"
    );
}

#[test]
fn ledger_ingress_accepts_only_sanitized_event_v2() {
    let root = workspace_root();
    let global_path = root.join("crates/ledger/src/global.rs");
    let global = fs::read_to_string(&global_path).expect("read global ledger source");
    let append_violations = inspect_global_append_ingress("crates/ledger/src/global.rs", &global)
        .expect("inspect global append ingress");
    assert!(
        append_violations.is_empty(),
        "global append ingress violations:\n{}",
        append_violations.join("\n")
    );

    let event_root = root.join("crates/actingcommand-contract/src/event");
    let mut capability_files = Vec::new();
    collect_rust_files(&event_root, &mut capability_files);
    capability_files.sort();
    let mut capability_source =
        fs::read_to_string(root.join("crates/actingcommand-contract/src/event.rs"))
            .expect("read event root source");
    for file in capability_files {
        capability_source.push('\n');
        capability_source.push_str(
            &fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("read {}: {error}", file.display())),
        );
    }
    let capability_violations = inspect_producer_event_capabilities(
        "crates/actingcommand-contract/src/event.rs and event/**/*.rs",
        &capability_source,
    )
    .expect("inspect producer capabilities");
    assert!(
        capability_violations.is_empty(),
        "producer capability violations:\n{}",
        capability_violations.join("\n")
    );
}

#[test]
fn contract_has_no_public_value_payload_or_persisted_fact() {
    let root = workspace_root();
    let mut files = vec![
        root.join("crates/actingcommand-contract/src/event.rs"),
        root.join("crates/ledger/src/fact.rs"),
        root.join("crates/ledger/src/global.rs"),
        root.join("crates/ledger/src/global/projection.rs"),
    ];
    collect_rust_files(
        &root.join("crates/actingcommand-contract/src/event"),
        &mut files,
    );
    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        violations.extend(inspect_public_api(&display, &source).expect("inspect public API"));
    }
    assert!(
        violations.is_empty(),
        "event v2 public Value violations:\n{}",
        violations.join("\n")
    );

    let fact = fs::read_to_string(root.join("crates/ledger/src/fact.rs"))
        .expect("read persisted fact source");
    let ownership = inspect_persisted_event_ownership("crates/ledger/src/fact.rs", &fact)
        .expect("inspect persisted fact");
    assert!(
        ownership.is_empty(),
        "persisted fact ownership violations:\n{}",
        ownership.join("\n")
    );
}

#[test]
fn c1_hardening_forbidden_source_surfaces_are_absent() {
    let root = workspace_root();
    let mut files = vec![root.join("crates/actingcommand-contract/src/event.rs")];
    collect_rust_files(
        &root.join("crates/actingcommand-contract/src/event"),
        &mut files,
    );
    files.extend([
        root.join("crates/ledger/src/critical.rs"),
        root.join("crates/ledger/src/fact.rs"),
        root.join("crates/ledger/src/global.rs"),
        root.join("crates/ledger/src/global/projection.rs"),
        root.join("crates/ledger/src/global/storage.rs"),
    ]);
    let forbidden = [
        "ClassifiedField",
        "StructuredPayloadDraft",
        "ErasedSanitizedEventDraft",
        "take_hook",
        "set_hook",
        "catch_unwind",
        "events_after(",
    ];
    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for token in forbidden {
            if source.contains(token) {
                violations.push(format!("{display}: forbidden source token {token}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "C1 hardening source violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn c2_artifact_store_authority_and_dependency_boundary_are_narrow() {
    let root = workspace_root();
    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let artifact_package = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .find(|package| package["name"] == "actingcommand-artifact-store")
        .expect("artifact-store package");
    let dependency_names = artifact_package["dependencies"]
        .as_array()
        .expect("artifact-store dependencies")
        .iter()
        .filter_map(|dependency| dependency["name"].as_str())
        .collect::<Vec<_>>();
    for forbidden in [
        "actingcommand-lab",
        "actingcommand-runtime-host",
        "actingcommand-scheduler",
        "actingcommand-runtime-client",
    ] {
        assert!(
            !dependency_names.contains(&forbidden),
            "artifact-store must not depend on {forbidden}"
        );
    }

    let mut artifact_sources = Vec::new();
    collect_rust_files(
        &root.join("crates/artifact-store/src"),
        &mut artifact_sources,
    );
    for path in artifact_sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for forbidden in [
            "create_touch_backend",
            "create_capture_backend",
            "MaaTouchBackend",
            "MinitouchBackend",
            "AdbInputBackend",
            "CaptureBackendFactory",
            "dyn CaptureBackend",
            "impl CaptureBackend",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} contains forbidden device authority token {forbidden}",
                path.display()
            );
        }
    }

    let mut workspace_sources = Vec::new();
    for directory in ["apps", "crates", "providers", "benchmarks"] {
        collect_rust_files(&root.join(directory), &mut workspace_sources);
    }
    let mut violations = Vec::new();
    for path in workspace_sources {
        let normalized = path.to_string_lossy().replace('\\', "/");
        if normalized.contains("/crates/actingcommand-contract/")
            || normalized.contains("/crates/artifact-store/")
        {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        if source.contains("ArtifactStoreIssuer") {
            violations.push(normalized.clone());
        }
        if source.contains("VerifiedArtifactReference") && !normalized.contains("/crates/ledger/") {
            violations.push(format!(
                "{normalized}: verified artifact recovery authority escaped store/ledger boundary"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "artifact issuer escaped contract/store boundary:\n{}",
        violations.join("\n")
    );

    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    let store = fs::read_to_string(root.join("crates/artifact-store/src/store.rs"))
        .expect("read artifact store");
    assert!(host.contains("GlobalLedger::open_with_artifact_verifier"));
    assert!(store.contains("pub fn verify_recovery_reference"));
}

#[test]
fn c5_lab_consumes_artifact_frame_store_without_an_ownership_wrapper() {
    let root = workspace_root();
    assert!(
        !root.join("crates/lab/src/frame_store.rs").exists(),
        "Lab frame-store ownership wrapper returned"
    );

    let facade =
        fs::read_to_string(root.join("crates/lab/src/lib.rs")).expect("read Lab facade source");
    let run =
        fs::read_to_string(root.join("crates/lab/src/lab_run.rs")).expect("read Lab run source");

    assert!(facade.contains("pub use actingcommand_artifact_store"));
    assert!(run.contains("use actingcommand_artifact_store"));
    for forbidden in [
        "mod frame_store;",
        "frame_store::{",
        "pub(crate) struct FrameStore",
    ] {
        assert!(
            !facade.contains(forbidden) && !run.contains(forbidden),
            "Lab regained frame-store ownership via {forbidden}"
        );
    }
}
#[test]
fn c5_portable_output_archive_is_owned_by_artifact_store() {
    let root = workspace_root();
    let artifact = fs::read_to_string(root.join("crates/artifact-store/src/portable_archive.rs"))
        .expect("read portable archive source");
    let artifact_frame_store =
        fs::read_to_string(root.join("crates/artifact-store/src/frame_store.rs"))
            .expect("read artifact frame store source");
    let lab_output = fs::read_to_string(root.join("crates/lab/src/lab_run/output.rs"))
        .expect("read Lab run output");
    let lab_context = fs::read_to_string(root.join("crates/lab/src/lab_run/context.rs"))
        .expect("read Lab run context");

    assert!(artifact.contains("pub fn write_portable_projection_archive"));
    assert!(artifact_frame_store.contains("PortableFrameEvidenceProjection"));
    assert!(artifact_frame_store.contains("pub fn portable_evidence_projection"));
    assert!(lab_context.contains("write_portable_projection_archive"));
    assert!(lab_context.contains("portable_evidence_projection"));
    assert!(lab_context.contains("frame_evidence.json"));
    assert!(lab_context.contains("ScreenshotNameAllocator"));
    assert!(!lab_context.contains("timestamp_file_stem"));
    assert!(!lab_context.contains("HashMap<String, usize>"));
    for forbidden in [
        "fn write_output_zip",
        "ZipWriter",
        "add_zip_dir",
        "path_to_zip_name",
    ] {
        assert!(
            !lab_output.contains(forbidden),
            "Lab regained portable archive mechanics via {forbidden}"
        );
    }
}

#[test]
fn c5_runtime_status_registry_is_owned_by_the_resident_control_plane() {
    let root = workspace_root();
    let contract = fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
        .expect("read Runtime contract");
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    let client = fs::read_to_string(root.join("crates/runtime-client/src/client.rs"))
        .expect("read Runtime client");
    let lab = fs::read_to_string(root.join("crates/lab/src/lib.rs")).expect("read Lab facade");

    assert!(contract.contains("RuntimeControlPlaneStatus"));
    assert!(host.contains("fn control_plane_status"));
    assert!(host.contains("initial_registered_instances"));
    assert!(client.contains("pub fn status"));
    assert!(!lab.contains("RuntimeControlPlaneStatus"));
}

#[test]
fn c5_monitor_decisions_are_pure_and_execution_owned() {
    let root = workspace_root();
    let monitor = fs::read_to_string(root.join("crates/execution-kernel/src/monitor.rs"))
        .expect("read execution monitor decisions");
    let lab = fs::read_to_string(root.join("crates/lab/src/lib.rs")).expect("read Lab facade");

    assert!(monitor.contains("pub fn decide_monitor"));
    assert!(monitor.contains("MonitorRecoveryKind"));
    for forbidden in [
        "actingcommand_device",
        "std::fs",
        "std::thread",
        "thread::sleep",
        "RuntimeHost",
        "RuntimeClient",
    ] {
        assert!(
            !monitor.contains(forbidden),
            "pure monitor decisions must not contain {forbidden}"
        );
    }
    assert!(!lab.contains("pub fn decide_monitor"));
}

#[test]
fn c5_monitor_policy_and_state_are_owned_by_runtime() {
    let root = workspace_root();
    let contract = fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
        .expect("read Runtime contract");
    let registry = fs::read_to_string(root.join("crates/runtime-host/src/monitor.rs"))
        .expect("read Runtime monitor registry");
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    let client = fs::read_to_string(root.join("crates/runtime-client/src/client.rs"))
        .expect("read Runtime client");
    let lab = fs::read_to_string(root.join("crates/lab/src/lib.rs")).expect("read Lab facade");

    assert!(contract.contains("ConfigureMonitor"));
    assert!(contract.contains("MonitorStatus"));
    assert!(registry.contains("struct MonitorRegistry"));
    assert!(registry.contains("struct DueMonitorProbe"));
    assert!(registry.contains("complete_probe"));
    assert!(registry.contains("fail_probe"));
    assert!(registry.contains("MONITOR_FILE_NAME"));
    assert!(host.contains("monitor_registry: Mutex<MonitorRegistry>"));
    assert!(host.contains("fn monitor_probe_loop"));
    assert!(host.contains("fn run_monitor_probe"));
    assert!(host.contains("MonitorPayloadDraft::completed"));
    assert!(host.contains("persist_monitor_observation"));
    assert!(host.contains("fn record_monitor_recovery_coordination"));
    assert!(host.contains("fn monitor_recovery_admission"));
    assert!(host.contains("MonitorPayloadDraft::recovery_admitted"));
    assert!(host.contains("MonitorPayloadDraft::recovery_deferred"));
    let coordination_start = host
        .find("    fn record_monitor_recovery_coordination(")
        .expect("monitor recovery coordination start");
    let coordination_end = host[coordination_start..]
        .find("    fn finish_monitor_failure(")
        .map(|offset| coordination_start + offset)
        .expect("monitor recovery coordination end");
    let coordination = &host[coordination_start..coordination_end];
    for forbidden in [
        "RuntimeOperation::",
        "TaskPayloadDraft",
        "InputPayloadDraft",
        "self.execution.input",
        "self.execution.run",
        ".put(",
    ] {
        assert!(
            !coordination.contains(forbidden),
            "monitor recovery coordination must not execute effects through {forbidden}"
        );
    }
    assert!(client.contains("pub fn configure_monitor"));
    assert!(client.contains("pub fn clear_monitor"));
    assert!(!lab.contains("RuntimeMonitorRegistryStatus"));
}

#[test]
fn c3b_execution_kernel_is_a_daemon_only_backend_shell() {
    let root = workspace_root();
    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    let kernel = packages
        .iter()
        .find(|package| package["name"] == "actingcommand-execution-kernel")
        .expect("execution-kernel package");
    let dependency_names = kernel["dependencies"]
        .as_array()
        .expect("execution-kernel dependencies")
        .iter()
        .filter_map(|dependency| dependency["name"].as_str())
        .collect::<Vec<_>>();
    for forbidden in [
        "actingcommand-lab",
        "actingcommand-runtime-client",
        "actingcommand-runtime-host",
        "actingcommand-scheduler",
        "actingcommand-ledger",
        "actingcommand-artifact-store",
    ] {
        assert!(
            !dependency_names.contains(&forbidden),
            "execution-kernel must not depend on {forbidden}"
        );
    }

    for package in packages {
        let name = package["name"].as_str().expect("package name");
        let reaches_kernel = package["dependencies"]
            .as_array()
            .expect("package dependencies")
            .iter()
            .any(|dependency| dependency["name"] == "actingcommand-execution-kernel");
        if reaches_kernel {
            assert!(
                matches!(
                    name,
                    "actingcommand-runtime-host"
                        | "actingcommand-actingd"
                        | "actingcommand-device-test"
                        | "actingcommand-lab"
                ),
                "package {name} must not access execution-kernel"
            );
        }
    }

    let mut sources = Vec::new();
    collect_rust_files(&root.join("crates/execution-kernel/src"), &mut sources);
    for path in sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for forbidden in [
            "TcpStream",
            "GlobalLedger",
            "SeedScheduler",
            "RuntimeClient",
            "actingcommand_lab",
            "CaptureBackendConfig",
            "CaptureBackendFactory",
            "InputBackendFactory",
            "std::fs",
            "create_touch_backend",
            "create_capture_backend",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} contains forbidden control-plane token {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn c5_readonly_recognition_is_pure_and_execution_owned() {
    let root = workspace_root();
    let source_path = root.join("crates/execution-kernel/src/readonly.rs");
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", source_path.display()));
    for forbidden in [
        "actingcommand_lab",
        "CaptureBackendConfig",
        "CaptureBackendFactory",
        "InputBackendFactory",
        "RuntimeClient",
        "std::fs",
        "create_capture_backend",
    ] {
        assert!(
            !source.contains(forbidden),
            "{} contains forbidden read-only ownership token {forbidden}",
            source_path.display()
        );
    }

    let lab_source_path = root.join("crates/lab/src/readonly.rs");
    let lab_source = fs::read_to_string(&lab_source_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", lab_source_path.display()));
    assert!(lab_source.contains("ReadonlyRecognitionEngine"));
    assert!(!lab_source.contains("evaluate_target("));
    assert!(!lab_source.contains("evaluate_all("));
}

#[test]
fn c5_environment_state_is_pure_and_execution_owned() {
    let root = workspace_root();
    let source_path = root.join("crates/execution-kernel/src/environment.rs");
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", source_path.display()));
    for forbidden in [
        "actingcommand_lab",
        "CaptureBackend",
        "InputBackend",
        "RuntimeClient",
        "std::fs",
        "create_capture_backend",
        "create_touch_backend",
    ] {
        assert!(
            !source.contains(forbidden),
            "{} contains forbidden environment ownership token {forbidden}",
            source_path.display()
        );
    }

    let lab_source_path = root.join("crates/lab/src/env_detection.rs");
    let lab_source = fs::read_to_string(&lab_source_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", lab_source_path.display()));
    assert!(lab_source.contains("EnvironmentStateEngine"));
    assert!(lab_source.contains("EnvironmentDetectionEngine::decide"));
    assert!(!lab_source.contains("pub struct EnvDetectionResult"));
    assert!(!lab_source.contains("struct EnvDetectionCatalog"));
    assert!(!lab_source.contains("struct EnvDetector"));
    assert!(!lab_source.contains("fn normalize_flat_env_catalog("));
    assert!(!lab_source.contains("fn validate_detection_key("));
    assert!(!lab_source.contains("fn validate_resolved_value("));
    assert!(!lab_source.contains("fn resolve_env_markers_in_value_inner("));
    assert!(!lab_source.contains("fn evaluate_detection_key("));
    assert!(!lab_source.contains("fn evaluate_candidate("));
    assert!(!lab_source.contains("let mut best"));
}

#[test]
fn c5_online_readonly_capture_is_runtime_owned() {
    let root = workspace_root();
    let app_environment = fs::read_to_string(root.join("apps/actinglab/src/env_detection.rs"))
        .expect("read ActingLab environment adapter");
    let app_readonly = fs::read_to_string(root.join("apps/actinglab/src/readonly_cli.rs"))
        .expect("read ActingLab read-only adapter");
    let runtime_capture =
        fs::read_to_string(root.join("apps/actinglab/src/runtime_capture_backend.rs"))
            .expect("read Runtime capture adapter");
    let lab_environment = fs::read_to_string(root.join("crates/lab/src/env_detection.rs"))
        .expect("read Lab environment adapter");

    for (path, source) in [
        ("apps/actinglab/src/env_detection.rs", &app_environment),
        ("apps/actinglab/src/readonly_cli.rs", &app_readonly),
        (
            "apps/actinglab/src/runtime_capture_backend.rs",
            &runtime_capture,
        ),
    ] {
        assert!(
            !source.contains("create_capture_backend"),
            "{path} must not construct a production capture backend"
        );
    }
    assert!(app_environment.contains("open_runtime_capture"));
    assert!(app_readonly.contains("build_readonly_lab_for_capture"));
    assert!(runtime_capture.contains("observe_readonly"));
    assert!(runtime_capture.contains("read_projected_verified"));
    assert!(!lab_environment.contains("CaptureBackendChoice::NemuIpc"));
    assert!(!lab_environment.contains("CaptureBackendChoice::DroidcastRaw"));
    assert!(!lab_environment.contains("CaptureBackendChoice::Adb"));
}

#[test]
fn c5_bounded_capture_sequences_are_runtime_owned_and_input_free() {
    let root = workspace_root();
    let contract = fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
        .expect("read Runtime contract");
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    let client = fs::read_to_string(root.join("crates/runtime-client/src/client.rs"))
        .expect("read Runtime client");

    for required in [
        "MAX_RUNTIME_CAPTURE_SEQUENCE_FRAMES: u16 = 60",
        "MAX_RUNTIME_CAPTURE_SEQUENCE_INTERVAL_MS: u64 = 5_000",
        "MAX_RUNTIME_CAPTURE_SEQUENCE_WAIT_MS: u64 = 60_000",
        "pub struct CaptureSequenceSpec",
        "pub struct CaptureSequence",
        "CaptureSequenceCompleted",
    ] {
        assert!(
            contract.contains(required),
            "capture sequence lost {required}"
        );
    }
    let operation = contract
        .split_once("    CaptureSequence {")
        .and_then(|(_, tail)| tail.split_once("SafeReset {").map(|(value, _)| value))
        .expect("capture sequence operation slice");
    for forbidden in ["LeaseToken", "InputAction", "holder_id", "action:"] {
        assert!(
            !operation.contains(forbidden),
            "capture sequence operation gained input authority via {forbidden}"
        );
    }
    assert!(contract.contains("Self::Input { token, action }"));

    let host_sequence = host
        .split_once("    fn capture_sequence(")
        .and_then(|(_, tail)| {
            tail.split_once("    fn capture_readonly_observation(")
                .map(|(value, _)| value)
        })
        .expect("Runtime capture sequence implementation");
    for required in [
        "capture_readonly_observation",
        "thread::sleep",
        "CaptureSequence::new",
    ] {
        assert!(
            host_sequence.contains(required),
            "Runtime capture sequence lost {required}"
        );
    }
    for forbidden in [
        "InputAction",
        "RuntimeOperation::Input",
        "LeaseToken",
        "TcpListener",
        "WebSocket",
        "Tls",
        "remote_stream",
    ] {
        assert!(
            !host_sequence.contains(forbidden),
            "Runtime capture sequence crossed its bounded read-only seam via {forbidden}"
        );
    }

    let client_sequence = client
        .split_once("    pub fn capture_sequence(")
        .and_then(|(_, tail)| {
            tail.split_once("    pub fn safe_reset(")
                .map(|(value, _)| value)
        })
        .expect("Runtime client capture sequence method");
    for forbidden in ["LeaseToken", "InputAction", "WebSocket", "Tls", "listener"] {
        assert!(
            !client_sequence.contains(forbidden),
            "Runtime client capture sequence gained forbidden surface {forbidden}"
        );
    }
}

#[test]
fn c5_session_status_and_monitor_clients_use_runtime_without_legacy_file_authority() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let adapter = fs::read_to_string(root.join("apps/actinglab/src/runtime_session_adapter.rs"))
        .expect("read Runtime session adapter");

    for required in [
        "runtime_session_adapter::run_status",
        "runtime_session_adapter::run_monitor_policy",
    ] {
        assert!(
            main.contains(required),
            "ActingLab client cutover lost {required}"
        );
    }
    for required in [
        "RuntimeClient::connect",
        ".status()",
        ".monitor_status()",
        ".configure_monitor(",
        ".clear_monitor(",
    ] {
        assert!(
            adapter.contains(required),
            "Runtime session adapter lost {required}"
        );
    }
    for forbidden in [
        "session_info_path",
        "session_heartbeat_path",
        "session_monitor_policy_path",
        "session_monitor_state_path",
        "write_json_file_atomic",
        "submit_session_command_request",
        "TcpListener",
    ] {
        assert!(
            !adapter.contains(forbidden),
            "Runtime session adapter regained legacy authority via {forbidden}"
        );
    }
}

#[test]
fn c5_bounded_stream_client_uses_runtime_without_local_capture_or_session_queues() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let adapter = fs::read_to_string(root.join("apps/actinglab/src/runtime_stream_adapter.rs"))
        .expect("read Runtime stream adapter");

    assert!(main.contains("runtime_stream_adapter::run_stream"));
    assert!(!main.contains("fn run_stream_legacy"));
    for required in [
        "RuntimeClient::connect",
        ".capture_sequence(",
        "CaptureSequenceSpec::new",
        "run_stream_input_relay",
        "runtime_artifact_verified",
    ] {
        assert!(
            adapter.contains(required),
            "Runtime stream adapter lost {required}"
        );
    }
    for forbidden in [
        "create_capture_backend",
        "capture_for_command",
        "stream_capture_frames",
        "submit_session_command_request",
        "write_json_file_atomic",
        "SESSION_REQUESTS_DIR",
        "SESSION_RUNNING_DIR",
        "SESSION_RESPONSES_DIR",
        "TcpListener",
        "WebSocket",
        "Tls",
        "remote_stream",
    ] {
        assert!(
            !adapter.contains(forbidden),
            "Runtime stream adapter regained forbidden authority via {forbidden}"
        );
    }
}

#[test]
fn c5_legacy_session_live_authority_is_retired() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let lab2 = fs::read_to_string(root.join("apps/actinglab/src/lab2_cli.rs"))
        .expect("read ActingLab Lab2 adapter");
    let session = fs::read_to_string(root.join("apps/actinglab/src/runtime_session_adapter.rs"))
        .expect("read Runtime session adapter");
    let stream = fs::read_to_string(root.join("apps/actinglab/src/runtime_stream_adapter.rs"))
        .expect("read Runtime stream adapter");

    for required in [
        "runtime_session_adapter::retired_authority(\"monitor\", args)",
        "\"daemon\" => runtime_session_adapter::retired_authority(sub, args)",
        "\"request\" => runtime_session_adapter::retired_authority(sub, args)",
        "\"journal\" => runtime_session_adapter::retired_authority(sub, args)",
        "\"events\" => runtime_session_adapter::retired_authority(sub, args)",
        "\"response\" => runtime_session_adapter::retired_authority(sub, args)",
        "\"request-state\" => runtime_session_adapter::retired_authority(sub, args)",
        "\"lease\" => runtime_session_adapter::retired_authority(sub, args)",
    ] {
        assert!(
            main.contains(required),
            "ActingLab lost retirement route {required}"
        );
    }
    for forbidden in [
        "struct SessionInfo",
        "struct SessionHeartbeat",
        "struct SessionLease",
        "fn session_info_path",
        "fn session_heartbeat_path",
        "fn session_lease_path",
        "fn run_session_daemon",
        "fn submit_session_command_request",
        "fn run_monitor_loop",
        "fn run_monitor_once",
        "SessionLayerRecoveryThroat",
        "SESSION_REQUESTS_DIR",
        "SESSION_RUNNING_DIR",
        "SESSION_RESPONSES_DIR",
        "SESSION_JOURNAL_FILE",
        "ACTINGLAB_TEST_SESSION_CRASH_POINT",
    ] {
        assert!(
            !main.contains(forbidden),
            "ActingLab retained legacy Session authority via {forbidden}"
        );
    }
    for forbidden in [
        "SessionLease",
        "session_lease_path",
        "project_lab2_lease_to_session",
        "remove_projected_session_lease",
        "lab2_session_lease_gate",
    ] {
        assert!(
            !lab2.contains(forbidden),
            "Lab2 can reactivate Session file authority via {forbidden}"
        );
    }
    for adapter in [&session, &stream] {
        assert!(adapter.contains("legacy_session_authority_retired"));
        assert!(adapter.contains("--via-daemon"));
        assert!(adapter.contains("--local"));
        assert!(adapter.contains("--state-dir"));
    }
}

#[test]
fn c5_online_lab_run_effects_are_instance_bound_and_runtime_owned() {
    let root = workspace_root();
    let app_environment = fs::read_to_string(root.join("apps/actinglab/src/env_detection.rs"))
        .expect("read ActingLab environment adapter");
    let app_run = fs::read_to_string(root.join("apps/actinglab/src/lab_run.rs"))
        .expect("read ActingLab run adapter");
    let runtime_capture =
        fs::read_to_string(root.join("apps/actinglab/src/runtime_capture_backend.rs"))
            .expect("read Runtime capture adapter");
    let runtime_input = fs::read_to_string(root.join("crates/runtime-client/src/input.rs"))
        .expect("read Runtime input proxy");
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    let contained = fs::read_to_string(root.join("crates/execution-kernel/src/contained_task.rs"))
        .expect("read contained task engine");

    assert!(
        !root
            .join("apps/actinglab/src/legacy_control_capture.rs")
            .exists()
    );
    for (path, source) in [
        ("apps/actinglab/src/env_detection.rs", &app_environment),
        ("apps/actinglab/src/lab_run.rs", &app_run),
    ] {
        for forbidden in [
            "LegacyControl",
            "legacy_control_capture",
            "Adb::new(",
            "create_capture_backend(",
            "create_touch_backend(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} regained legacy production authority via {forbidden}"
            );
        }
    }
    for forbidden in ["LabRunRequest", ".lab_run(", "RuntimeDebugSession"] {
        assert!(
            !app_run.contains(forbidden),
            "ActingLab production run regained semantic execution authority via {forbidden}"
        );
    }
    for required in [
        "RuntimeClient::connect",
        "ContainedTaskRequest::new",
        "run_contained_task(&instance, request)",
        "runtime_global_ledger",
    ] {
        assert!(
            app_run.contains(required),
            "ActingLab Runtime task adapter lost {required}"
        );
    }
    assert!(app_environment.contains("AppCaptureAuthority::Runtime("));
    assert!(app_environment.contains("RuntimeInputBackend::connect("));
    assert!(runtime_capture.contains("observe_readonly"));
    for required in [
        "authority.acquire_lease",
        "authority.release_lease",
        "self.authority.input",
    ] {
        assert!(
            runtime_input.contains(required),
            "Runtime input proxy lost scheduler-fenced effect path {required}"
        );
    }
    assert!(
        host.contains("let execution = prepared.run(&mut runtime);")
            && contained.contains("pub struct PreparedContainedTask")
            && contained.contains("pub trait ContainedTaskRuntime"),
        "Runtime and execution-kernel must own the contained task run loop"
    );
}

#[test]
fn c5_task_planning_is_owned_by_execution_kernel_and_legacy_crate_is_retired() {
    let root = workspace_root();
    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    assert!(
        packages
            .iter()
            .all(|package| package["name"] != "actingcommand-task-loop"),
        "retired actingcommand-task-loop package returned to the workspace"
    );
    let dependencies = |package_name: &str| {
        packages
            .iter()
            .find(|package| package["name"] == package_name)
            .unwrap_or_else(|| panic!("missing package {package_name}"))["dependencies"]
            .as_array()
            .expect("package dependencies")
            .iter()
            .filter_map(|dependency| dependency["name"].as_str())
            .collect::<Vec<_>>()
    };

    let kernel_dependencies = dependencies("actingcommand-execution-kernel");
    for required in [
        "actingcommand-page-detector",
        "actingcommand-recognition",
        "actingcommand-recognition-pack",
    ] {
        assert!(
            kernel_dependencies.contains(&required),
            "execution-kernel must own task planning dependency {required}"
        );
    }

    let device_test_dependencies = dependencies("actingcommand-device-test");
    assert!(
        device_test_dependencies.contains(&"actingcommand-execution-kernel"),
        "device-test must consume planning from execution-kernel"
    );
    assert!(
        !device_test_dependencies.contains(&"actingcommand-task-loop"),
        "device-test must not retain the legacy task-loop dependency"
    );

    let mut planning_sources = Vec::new();
    collect_rust_files(
        &root.join("crates/execution-kernel/src/planning"),
        &mut planning_sources,
    );
    for path in planning_sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for forbidden in [
            "actingcommand_device",
            "ExecutionKernel",
            "ExecutionBackendProvider",
            "InputBackend",
            "CaptureBackend",
            "std::process::Command",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} contains forbidden planning side-effect token {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn persisted_event_is_opaque_and_query_matching_is_ledger_owned() {
    let root = workspace_root();
    let fact = fs::read_to_string(root.join("crates/ledger/src/fact.rs"))
        .expect("read persisted fact source");
    let ownership = inspect_persisted_event_ownership("crates/ledger/src/fact.rs", &fact)
        .expect("inspect persisted fact");
    assert!(
        ownership.is_empty(),
        "persisted fact ownership violations:\n{}",
        ownership.join("\n")
    );

    let mut contract_files = vec![root.join("crates/actingcommand-contract/src/event.rs")];
    collect_rust_files(
        &root.join("crates/actingcommand-contract/src/event"),
        &mut contract_files,
    );
    let mut matching_violations = Vec::new();
    for path in contract_files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        matching_violations.extend(
            inspect_contract_fact_matching(&display, &source)
                .expect("inspect contract fact matching"),
        );
    }
    assert!(
        matching_violations.is_empty(),
        "contract-owned fact matching violations:\n{}",
        matching_violations.join("\n")
    );

    let projection = fs::read_to_string(root.join("crates/ledger/src/global/projection.rs"))
        .expect("read ledger projection source");
    assert!(
        ledger_owns_query_matching("crates/ledger/src/global/projection.rs", &projection)
            .expect("inspect ledger query matching"),
        "ledger projection must own EventQuery-to-PersistedEvent matching"
    );
}

#[test]
fn command_inventory_matches_checked_in_snapshot() {
    let root = workspace_root();
    let mut paths = Vec::new();
    collect_rust_files(&root.join("apps/actinglab/src"), &mut paths);
    paths.sort();
    let owned_sources = paths
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string();
            let source = fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
            (relative, source)
        })
        .collect::<Vec<_>>();
    let sources = owned_sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let actual = extract_command_inventory(&sources).unwrap();

    let snapshot_text = fs::read_to_string(root.join("ratchet/actinglab_commands.json"))
        .expect("read ratchet/actinglab_commands.json");
    let snapshot: serde_json::Value =
        serde_json::from_str(&snapshot_text).expect("parse actinglab command snapshot");
    assert_eq!(
        snapshot["schema_version"],
        "actingcommand.command-inventory.v1"
    );
    assert_eq!(snapshot["source"], "apps/actinglab/src/**/*.rs");
    assert_eq!(snapshot["dispatch_function"], "execute");
    assert_eq!(snapshot["denominator_kind"], "top_level_dispatch_arm");
    assert_eq!(
        snapshot["dispatch_arm_count"].as_u64(),
        Some(actual.dispatch_arm_count as u64)
    );
    let expected_dispatch_arms = snapshot["dispatch_arms"]
        .as_array()
        .expect("snapshot dispatch_arms must be an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("snapshot dispatch arm must be a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(expected_dispatch_arms, actual.dispatch_arms);
    assert_eq!(
        snapshot["command_count"].as_u64(),
        Some(actual.commands.len() as u64)
    );
    let expected_commands = snapshot["commands"]
        .as_array()
        .expect("snapshot commands must be an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("snapshot command must be a string")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(expected_commands, actual.commands);
    for exemption in snapshot["pipeline_exemptions"]
        .as_array()
        .expect("snapshot pipeline_exemptions must be an array")
    {
        let command = exemption["command"]
            .as_str()
            .expect("pipeline exemption command must be a string");
        assert!(
            actual.commands.iter().any(|candidate| candidate == command),
            "pipeline exemption references unknown command {command}"
        );
        assert!(
            exemption["reason"]
                .as_str()
                .is_some_and(|reason| !reason.trim().is_empty()),
            "pipeline exemption {command} must explain its reason"
        );
    }
}

#[test]
fn contract_dependencies_stay_within_budget() {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("crates/actingcommand-contract/Cargo.toml"))
        .expect("read contract Cargo.toml");
    let violations = contract_dependency_violations(&manifest).unwrap();

    assert!(
        violations.is_empty(),
        "contract dependency budget violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn workspace_packages_do_not_depend_on_apps() {
    let root = workspace_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1"])
        .current_dir(&root)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = String::from_utf8(output.stdout).expect("cargo metadata must emit UTF-8 JSON");
    let violations = workspace_dependency_violations(&metadata).unwrap();

    assert!(
        violations.is_empty(),
        "workspace dependency-law violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn actingcommand_contract_has_no_dependency_path_to_actingcommand_ledger() {
    let metadata = workspace_metadata();
    let path = dependency_path(&metadata, "actingcommand-contract", "actingcommand-ledger");
    assert!(
        path.is_none(),
        "actingcommand-contract must not reach actingcommand-ledger: {}",
        path.as_ref()
            .map(|path| path.join(" -> "))
            .unwrap_or_else(|| "no path".to_string())
    );
}

#[test]
fn dependency_metadata_requests_all_features() {
    assert_eq!(
        cargo_metadata_args(),
        ["metadata", "--format-version", "1", "--all-features"]
    );
}

#[test]
fn feature_gated_forbidden_dependency_paths_are_detected() {
    let contract_path = dependency_path(
        FEATURE_GATED_FORBIDDEN_PATH_METADATA,
        "actingcommand-contract",
        "actingcommand-ledger",
    );
    assert_eq!(
        contract_path,
        Some(vec![
            "actingcommand-contract".to_string(),
            "contract-feature-bridge".to_string(),
            "actingcommand-ledger".to_string(),
        ])
    );
    let lab_violations = lab_removability_violations(
        FEATURE_GATED_FORBIDDEN_PATH_METADATA,
        &["actingcommand-lab", "actingcommand-actinglab"],
    )
    .expect("inspect feature-gated Lab path");
    assert_eq!(
        lab_violations,
        vec![
            "production package actingcommand-runtime-core reaches actingcommand-lab: actingcommand-runtime-core -> runtime-feature-bridge -> actingcommand-lab"
        ]
    );
}

#[test]
fn all_non_lab_packages_remain_lab_free_with_all_features() {
    let metadata = workspace_metadata();
    let violations =
        lab_removability_violations(&metadata, &["actingcommand-lab", "actingcommand-actinglab"])
            .unwrap();

    assert!(
        violations.is_empty(),
        "production-to-Lab dependency violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn production_packages_cannot_reach_resource_tooling() {
    let metadata = workspace_metadata();
    let document: serde_json::Value =
        serde_json::from_str(&metadata).expect("parse cargo metadata");
    assert!(
        document["packages"].as_array().is_some_and(|packages| {
            packages
                .iter()
                .any(|package| package["name"] == "actingcommand-resource-tooling")
        }),
        "C5 requires the actingcommand-resource-tooling package"
    );
    let violations = resource_tooling_removability_violations(
        &metadata,
        &[
            "actingcommand-resource-tooling",
            "actingcommand-lab",
            "actingcommand-actinglab",
        ],
    )
    .unwrap();

    assert!(
        violations.is_empty(),
        "production-to-resource-tooling dependency violations:\n{}",
        violations.join("\n")
    );
    for forbidden in [
        "actingcommand-lab",
        "actingcommand-runtime-host",
        "actingcommand-scheduler",
        "actingcommand-execution-kernel",
        "actingcommand-device",
    ] {
        let path = dependency_path(&metadata, "actingcommand-resource-tooling", forbidden);
        assert!(
            path.is_none(),
            "resource-tooling must not reach {forbidden}: {}",
            path.as_ref()
                .map(|path| path.join(" -> "))
                .unwrap_or_else(|| "no path".to_string())
        );
    }
}

#[test]
fn c5_disconnected_runtime_core_prototype_is_retired() {
    let root = workspace_root();
    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");

    assert!(
        packages
            .iter()
            .all(|package| package["name"] != "actingcommand-runtime-core"),
        "the disconnected runtime-core prototype must not remain in the workspace"
    );
    assert!(
        !root.join("crates/runtime-core/Cargo.toml").exists(),
        "the disconnected runtime-core prototype manifest must be removed"
    );
}

#[test]
fn c6_local_lab2_arbitrator_is_retired() {
    let root = workspace_root();
    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    assert!(
        packages
            .iter()
            .all(|package| package["name"] != "actingcommand-arbitrator"),
        "the legacy Lab2 arbitrator must not re-enter the workspace"
    );
    assert!(
        !root.join("crates/arbitrator/Cargo.toml").exists(),
        "the legacy Lab2 arbitrator manifest must remain removed"
    );

    let lab_state = fs::read_to_string(root.join("crates/lab/src/state.rs"))
        .expect("read crates/lab/src/state.rs");
    let lab2_cli = fs::read_to_string(root.join("apps/actinglab/src/lab2_cli.rs"))
        .expect("read apps/actinglab/src/lab2_cli.rs");
    for forbidden in [
        "ArbitratorStore",
        "DegradedArbitrator",
        "lab2-arbitrator",
        "lab2-recovery-state.json",
    ] {
        assert!(
            !lab_state.contains(forbidden) && !lab2_cli.contains(forbidden),
            "legacy Lab2 authority symbol '{forbidden}' must remain absent"
        );
    }
    for forbidden in [
        "ScreencapBackend",
        "MaaTouchBackend",
        "actingcommand_device",
    ] {
        assert!(
            !lab2_cli.contains(forbidden),
            "Lab2 must use Runtime IPC rather than opening production device authority: {forbidden}"
        );
    }
}

#[test]
fn c7_lab_has_no_production_ledger_writer_authority() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rust_files(&root.join("apps/actinglab/src"), &mut files);
    collect_rust_files(&root.join("crates/lab/src"), &mut files);
    let mut violations = Vec::new();
    for path in files {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for constructor in ["LabLedger::create(", "LabLedger::create_runtime_shard("] {
            if source.contains(constructor) {
                violations.push(format!(
                    "{display}: Lab regained a durable local ledger writer via {constructor}"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Lab local-ledger authority violations:\n{}",
        violations.join("\n")
    );

    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let lab2 = fs::read_to_string(root.join("apps/actinglab/src/lab2_cli.rs"))
        .expect("read ActingLab Lab2 adapter");
    let lab_run = fs::read_to_string(root.join("apps/actinglab/src/lab_run.rs"))
        .expect("read ActingLab run adapter");
    let environment = fs::read_to_string(root.join("apps/actinglab/src/env_detection.rs"))
        .expect("read ActingLab environment adapter");
    let runtime_contract =
        fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
            .expect("read Runtime contract");
    let runtime_host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    assert!(
        main.contains("local_ledger_retired"),
        "legacy local ledger command must remain a fail-loud tombstone"
    );
    assert!(
        lab2.contains("RuntimeDebugEvent::requested")
            && lab2.contains("RuntimeDebugEvent::completed")
            && lab2.contains("RuntimeDebugEvent::failed"),
        "online Lab2 operations must project typed lifecycle events through Runtime"
    );
    assert!(
        lab_run.contains("run_contained_task(&instance, request)")
            && lab_run.contains("runtime_global_ledger")
            && !lab_run.contains("record_event(RuntimeDebugEvent::")
            && !lab_run.contains(".lab_run("),
        "Lab run must submit one Runtime task request and render its GlobalLedger projection"
    );
    for required in [
        "pub(super) struct AppLedgerSink;",
        "pub(super) struct AppRunLedgerSession;",
        "retired_run_ledger_error",
    ] {
        assert!(
            environment.contains(required),
            "ActingLab disposable ledger adapter lost {required}"
        );
    }
    for forbidden in [
        "Vec<LedgerRecord>",
        "Vec<LightEvent>",
        "records:",
        "events:",
    ] {
        assert!(
            !environment.contains(forbidden),
            "ActingLab environment adapter regained an in-memory semantic source via {forbidden}"
        );
    }
    assert!(
        !runtime_contract.contains("TaskSemanticFact"),
        "clients must not be able to submit Runtime task semantic facts"
    );
    for required in [
        "TaskSemanticFact::RecognitionStarted",
        "TaskSemanticFact::EffectIntent",
        "TaskSemanticFact::TerminalCommitted",
        "TaskSemanticFact::TerminalRejected",
    ] {
        assert!(
            runtime_host.contains(required),
            "Runtime semantic owner lost {required}"
        );
    }
}

#[test]
fn r35_contained_task_boundary_is_generic_and_has_a_neutral_process_fixture() {
    let root = workspace_root();
    let contract = fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
        .expect("read Runtime contract");
    let request = contract
        .split_once("pub struct ContainedTaskRequest {")
        .and_then(|(_, tail)| tail.split_once("}\n\nimpl ContainedTaskRequest"))
        .map(|(body, _)| body)
        .expect("locate ContainedTaskRequest fields");
    for required in ["package_path: String", "expected_sha256: String"] {
        assert!(
            request.contains(required),
            "contained task request lost {required}"
        );
    }
    for forbidden in ["game", "server", "package_name", "TaskSemanticFact"] {
        assert!(
            !request.contains(forbidden),
            "contained task client contract embeds application identity via {forbidden}"
        );
    }
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    let contained = fs::read_to_string(root.join("crates/execution-kernel/src/contained_task.rs"))
        .expect("read contained task engine");
    for forbidden in ["arknights", "azurlane", "bluearchive", "com.YoStar"] {
        assert!(
            !host.contains(forbidden) && !contained.contains(forbidden),
            "contained Runtime path hard-codes application identity {forbidden}"
        );
    }
    let process = fs::read_to_string(root.join("apps/actingctl/tests/c4_process.rs"))
        .expect("read actingctl process tests");
    for required in [
        "actingctl_runs_neutral_contained_task_without_lab_and_runtime_survives_client_exit",
        "process_replay_cannot_duplicate_or_conflict_a_contained_task_terminal",
        "\"game\":\"neutral\"",
        "\"neutral.instance\"",
    ] {
        assert!(
            process.contains(required),
            "neutral contained-task process fixture lost {required}"
        );
    }
    let actinglab_process =
        fs::read_to_string(root.join("apps/actinglab/tests/runtime_input_proxy.rs"))
            .expect("read ActingLab Runtime process tests");
    let assert_neutral_fixture = |fixture: &str, label: &str| {
        assert!(
            fixture.contains("neutral.instance"),
            "{label} lost neutral instance identity"
        );
        for forbidden in ["ak.cn", "arknights", "\"ark\"", "\"cn\""] {
            assert!(
                !fixture.contains(forbidden),
                "{label} regained application-specific identity {forbidden}"
            );
        }
    };
    let application_fixture = actinglab_process
        .split_once(
            "fn session_app_routes_application_lifecycle_through_runtime_without_client_package_identity()",
        )
        .and_then(|(_, tail)| {
            tail.split_once(
                "fn session_status_and_monitor_policy_project_resident_runtime_without_legacy_state()",
            )
        })
        .map(|(body, _)| body)
        .expect("locate ActingLab application lifecycle Runtime evidence");
    assert_neutral_fixture(
        application_fixture,
        "ActingLab application lifecycle fixture",
    );
    let client_kill_fixture = actinglab_process
        .split_once("fn runtime_finishes_and_rebuilds_lab_run_after_actinglab_client_is_killed()")
        .and_then(|(_, tail)| tail.split_once("fn wait_until("))
        .map(|(body, _)| body)
        .expect("locate ActingLab client-kill Runtime recovery evidence");
    assert_neutral_fixture(client_kill_fixture, "ActingLab client-kill fixture");
    for required in ["neutral/terminal", "\"neutral\"", "\"test\""] {
        assert!(
            client_kill_fixture.contains(required),
            "ActingLab client-kill fixture lost neutral task identity {required}"
        );
    }
}

fn cargo_metadata_args() -> [&'static str; 4] {
    ["metadata", "--format-version", "1", "--all-features"]
}

fn workspace_metadata() -> String {
    let root = workspace_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(cargo_metadata_args())
        .current_dir(&root)
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("cargo metadata must emit UTF-8 JSON")
}

const FEATURE_GATED_FORBIDDEN_PATH_METADATA: &str = r#"{
    "packages": [
        {"id": "contract", "name": "actingcommand-contract"},
        {"id": "contract-bridge", "name": "contract-feature-bridge"},
        {"id": "ledger", "name": "actingcommand-ledger"},
        {"id": "runtime", "name": "actingcommand-runtime-core"},
        {"id": "runtime-bridge", "name": "runtime-feature-bridge"},
        {"id": "lab", "name": "actingcommand-lab"}
    ],
    "workspace_members": ["contract", "ledger", "runtime", "lab"],
    "resolve": {
        "nodes": [
            {"id": "contract", "dependencies": ["contract-bridge"]},
            {"id": "contract-bridge", "dependencies": ["ledger"]},
            {"id": "ledger", "dependencies": []},
            {"id": "runtime", "dependencies": ["runtime-bridge"]},
            {"id": "runtime-bridge", "dependencies": ["lab"]},
            {"id": "lab", "dependencies": []}
        ]
    }
}"#;

fn dependency_path(metadata: &str, from_name: &str, to_name: &str) -> Option<Vec<String>> {
    let metadata: serde_json::Value = serde_json::from_str(metadata).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    let package_names = packages
        .iter()
        .map(|package| {
            (
                package["id"].as_str().expect("package id"),
                package["name"].as_str().expect("package name"),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let from = package_names
        .iter()
        .find_map(|(id, name)| (*name == from_name).then_some(*id))
        .expect("source package");
    let to = package_names
        .iter()
        .find_map(|(id, name)| (*name == to_name).then_some(*id))
        .expect("target package");
    let dependencies = metadata["resolve"]["nodes"]
        .as_array()
        .expect("metadata resolve nodes")
        .iter()
        .map(|node| {
            (
                node["id"].as_str().expect("node id"),
                node["dependencies"]
                    .as_array()
                    .expect("node dependencies")
                    .iter()
                    .map(|dependency| dependency.as_str().expect("dependency id"))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut pending = std::collections::VecDeque::from([vec![from]]);
    let mut visited = std::collections::BTreeSet::from([from]);

    while let Some(path) = pending.pop_front() {
        let current = path.last().expect("non-empty dependency path");
        if *current == to {
            return Some(
                path.iter()
                    .map(|id| package_names[id].to_string())
                    .collect(),
            );
        }
        for dependency in dependencies.get(current).into_iter().flatten() {
            if visited.insert(dependency) {
                let mut next = path.clone();
                next.push(dependency);
                pending.push_back(next);
            }
        }
    }
    None
}

#[test]
fn actinglab_runtime_endpoint_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let runtime_endpoint = fs::read_to_string(root.join("apps/actinglab/src/runtime_endpoint.rs"))
        .expect("read ActingLab Runtime endpoint module");

    assert!(
        main.contains("mod runtime_endpoint;"),
        "ActingLab main lost the private Runtime endpoint module"
    );
    for definition in [
        "struct RuntimeEndpointPolicy",
        "enum RuntimeEndpointChannel",
        "impl RuntimeEndpointChannel",
        "fn runtime_endpoint_check(",
        "fn runtime_endpoint_policy(",
        "fn runtime_endpoint_policy_json(",
        "fn trusted_remote_auth_material(",
        "fn env_var_non_empty(",
        "fn runtime_tcp_available(",
        "fn parse_endpoint_host_port(",
        "fn parse_endpoint_parts(",
        "fn is_loopback_host(",
    ] {
        assert!(
            runtime_endpoint.contains(definition),
            "Runtime endpoint module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained Runtime endpoint owner definition {definition}"
        );
    }
}

#[test]
fn actinglab_cli_result_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let cli_result = fs::read_to_string(root.join("apps/actinglab/src/cli_result.rs"))
        .expect("read ActingLab CLI result module");

    assert!(
        main.contains("mod cli_result;"),
        "ActingLab main lost the private CLI result module"
    );
    for definition in [
        "struct CliResult",
        "impl CliResult",
        "fn ok(command: String, data: Value, print_json: bool, human: String) -> Self",
        "fn err(command: String, err: CliError, print_json: bool) -> Self",
        "fn exit_code(&self) -> i32",
        "fn envelope_json(&self) -> String",
        "fn human_text(&self) -> String",
        "trait CliErrorExitCode",
        "impl CliErrorExitCode for CliError",
        "ErrorKind::UsageValidation => 2,",
        "ErrorKind::SafetyBlocked => 3,",
        "ErrorKind::DeviceInstance => 4,",
        "ErrorKind::RuntimeUnavailable => 5,",
        "ErrorKind::NotImplemented => 6,",
    ] {
        assert!(
            cli_result.contains(definition),
            "CLI result module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained CLI result owner definition {definition}"
        );
    }
    assert_eq!(
        cli_result.matches("pub(super) ").count(),
        9,
        "CLI result owner visibility changed"
    );
    for line in cli_result.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "CLI result owner exposed broader visibility: {line}"
        );
    }
}

#[test]
fn actinglab_flag_args_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_args = fs::read_to_string(root.join("apps/actinglab/src/flag_args.rs"))
        .expect("read ActingLab flag args module");

    assert!(
        main.contains("mod flag_args;"),
        "ActingLab main lost the private flag args module"
    );
    for definition in [
        "struct FlagArgs",
        "flags: BTreeMap<String, Vec<String>>",
        "positionals: Vec<String>",
        "impl FlagArgs",
        "fn parse(args: &[String]) -> CliOutcome<Self>",
        "fn bool(&self, name: &str) -> bool",
        "fn optional(&self, name: &str) -> Option<String>",
        "fn values(&self, name: &str) -> Vec<String>",
        "fn without_first_positional(&self) -> Self",
        "fn required(&self, name: &str) -> CliOutcome<String>",
        "fn optional_path(&self, name: &str) -> Option<PathBuf>",
        "fn required_path(&self, name: &str) -> CliOutcome<PathBuf>",
        "fn reject_flags(&self, command: &str) -> CliOutcome<()>",
        "fn expect_positionals(&self, command: &str, expected: usize) -> CliOutcome<()>",
        "fn required_positional(&self, index: usize, name: &str) -> CliOutcome<&str>",
        "fn required_i32(&self, index: usize, name: &str) -> CliOutcome<i32>",
        "fn required_u64(&self, index: usize, name: &str) -> CliOutcome<u64>",
    ] {
        assert!(
            flag_args.contains(definition),
            "flag args module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained flag args owner definition {definition}"
        );
    }

    assert_eq!(
        flag_args.matches("pub(super) ").count(),
        16,
        "flag args owner visibility changed"
    );
    for line in flag_args.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "flag args owner exposed broader visibility: {line}"
        );
    }

    let marker = "#[derive(Debug, Clone, Default)]";
    let (_, owner_tail) = flag_args
        .split_once(marker)
        .expect("flag args module contains owner marker");
    let normalized_owner = format!("{marker}{owner_tail}").replace("pub(super) ", "");
    assert_eq!(
        normalized_owner.lines().count(),
        120,
        "flag args owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        3_791,
        "flag args owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "16bb08d7468541b06df651ee3169ee14066ad98620c060b2ae60344262326a30",
        "flag args owner body changed"
    );
}

#[test]
fn actinglab_device_runtime_config_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let device_runtime_config =
        fs::read_to_string(root.join("apps/actinglab/src/device_runtime_config.rs"))
            .expect("read ActingLab device Runtime config module");

    assert!(
        main.contains("mod device_runtime_config;"),
        "ActingLab main lost the private device Runtime config module"
    );
    for definition in [
        "fn device_config(",
        "fn device_config_for_instance(",
        "struct DeviceRuntimeConfig",
        "impl DeviceRuntimeConfig",
        "fn runtime_capture_endpoint(",
        "fn effective_capture_backend_choice(",
        "fn effective_touch_backend_choice(",
    ] {
        assert!(
            device_runtime_config.contains(definition),
            "device Runtime config module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained device Runtime config owner definition {definition}"
        );
    }
    for field in [
        "instance_alias: String",
        "runtime_state_root: PathBuf",
        "target: DeviceTarget",
        "adb_source: AdbPathSource",
        "adb_warning: Option<String>",
        "capture_backend: CaptureBackendChoice",
        "touch_backend: TouchBackendChoice",
    ] {
        assert!(
            device_runtime_config.contains(field),
            "device Runtime config module lost owner field {field}"
        );
    }

    assert_eq!(
        device_runtime_config.matches("pub(super) ").count(),
        9,
        "device Runtime config owner visibility changed"
    );
    for line in device_runtime_config.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "device Runtime config owner exposed broader visibility: {line}"
        );
    }

    let marker = "pub(super) fn device_config(";
    let (_, owner_tail) = device_runtime_config
        .split_once(marker)
        .expect("device Runtime config module contains owner marker");
    let normalized_owner = format!("fn device_config({owner_tail}").replace("pub(super) ", "");
    assert_eq!(
        normalized_owner.lines().count(),
        98,
        "device Runtime config owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        3_325,
        "device Runtime config owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "33f417f67615e680b48a520e7ce547cedd924b19305f1586df7c133f9a4a4541",
        "device Runtime config owner body changed"
    );
}

fn actinglab_instance_resolution_root_wiring_is_frozen(main: &str) -> bool {
    const DECLARATION: &str = "#[rustfmt::skip] mod instance_resolution;";

    let lines = main.lines().collect::<Vec<_>>();
    let declarations = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("mod instance_resolution;"))
        .map(|(index, line)| (index, *line))
        .collect::<Vec<_>>();
    if declarations.len() != 1 {
        return false;
    }

    let (index, declaration) = declarations[0];
    declaration == DECLARATION
        && index > 1
        && index + 1 < lines.len()
        && lines[index - 2] == "mod flag_args;"
        && lines[index - 1] == "mod flag_values;"
        && lines[index + 1] == "mod lab2_cli;"
}

#[test]
fn actinglab_instance_resolution_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let instance_resolution =
        fs::read_to_string(root.join("apps/actinglab/src/instance_resolution.rs"))
            .expect("read ActingLab instance resolution module");

    assert!(
        actinglab_instance_resolution_root_wiring_is_frozen(&main),
        "ActingLab main lost the exact private instance resolution module placement"
    );
    let declaration = "#[rustfmt::skip] mod instance_resolution;";
    let frozen_placement = concat!(
        "mod flag_args;\n",
        "mod flag_values;\n",
        "#[rustfmt::skip] mod instance_resolution;\n",
        "mod lab2_cli;",
    );
    let counterexamples = [
        (
            "plain pub declaration",
            main.replacen(
                declaration,
                "#[rustfmt::skip] pub mod instance_resolution;",
                1,
            ),
        ),
        (
            "pub(crate) declaration",
            main.replacen(
                declaration,
                "#[rustfmt::skip] pub(crate) mod instance_resolution;",
                1,
            ),
        ),
        (
            "duplicate declaration",
            main.replacen(
                declaration,
                "#[rustfmt::skip] mod instance_resolution;\n#[rustfmt::skip] mod instance_resolution;",
                1,
            ),
        ),
        ("missing declaration", main.replacen(declaration, "", 1)),
        (
            "moved declaration",
            main.replacen(
                frozen_placement,
                concat!(
                    "mod flag_args;\n",
                    "mod flag_values;\n",
                    "mod lab2_cli;\n",
                    "#[rustfmt::skip] mod instance_resolution;",
                ),
                1,
            ),
        ),
    ];
    for (label, counterexample) in counterexamples {
        assert_ne!(
            counterexample, main,
            "instance resolution guard counterexample was not constructed: {label}"
        );
        assert!(
            !actinglab_instance_resolution_root_wiring_is_frozen(&counterexample),
            "instance resolution guard accepted counterexample: {label}"
        );
    }
    for definition in [
        "fn resolve_instance_id(",
        "fn resolve_instance_id_for_flags(",
    ] {
        assert!(
            instance_resolution.contains(definition),
            "instance resolution module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained instance resolution owner definition {definition}"
        );
    }

    assert_eq!(
        instance_resolution.matches("pub(super) ").count(),
        2,
        "instance resolution owner visibility changed"
    );
    for line in instance_resolution.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "instance resolution owner exposed broader visibility: {line}"
        );
    }

    let marker = "pub(super) fn resolve_instance_id(";
    let (_, owner_tail) = instance_resolution
        .split_once(marker)
        .expect("instance resolution module contains owner marker");
    let normalized_owner =
        format!("fn resolve_instance_id({owner_tail}").replace("pub(super) ", "");
    assert_eq!(
        normalized_owner.lines().count(),
        32,
        "instance resolution owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        1_076,
        "instance resolution owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "c279bb198a5c289604faa8659299ff42b995b071ce85b8150b056cc5c19b794d",
        "instance resolution owner body changed"
    );
}

#[test]
fn actinglab_user_config_store_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let user_config_store =
        fs::read_to_string(root.join("apps/actinglab/src/user_config_store.rs"))
            .expect("read ActingLab user config store module");

    const ROOT_DECLARATION: &str = "mod user_config_store;";
    const ROOT_IMPORT: &str =
        "use user_config_store::{config_path, read_user_config, write_user_config};";
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod user_config_store;"))
        .collect::<Vec<_>>();
    let imports = main
        .lines()
        .filter(|line| line.contains("user_config_store::"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private user config store module declaration"
    );
    assert_eq!(
        imports,
        vec![ROOT_IMPORT],
        "ActingLab main lost the one private user config store import"
    );

    for definition in [
        "fn read_user_config(",
        "fn write_user_config(",
        "fn config_path(",
    ] {
        assert!(
            user_config_store.contains(definition),
            "user config store module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained user config store owner definition {definition}"
        );
    }

    assert_eq!(
        user_config_store.matches("pub(super) ").count(),
        3,
        "user config store owner visibility changed"
    );
    for line in user_config_store.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "user config store owner exposed broader visibility: {line}"
        );
    }

    let marker = "pub(super) fn read_user_config() -> CliOutcome<UserConfig> {";
    let (_, owner_tail) = user_config_store
        .split_once(marker)
        .expect("user config store module contains owner marker");
    let normalized_owner =
        format!("fn read_user_config() -> CliOutcome<UserConfig> {{{owner_tail}")
            .replace("pub(super) ", "");
    assert_eq!(
        normalized_owner.lines().count(),
        41,
        "user config store owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        1_322,
        "user config store owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "87dbf54442842ff2307fa3f60f8c05f9e3be503f32b988643de6544b8b7dd97c",
        "user config store owner body changed"
    );
}

#[test]
fn actinglab_user_config_keys_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let user_config_keys = fs::read_to_string(root.join("apps/actinglab/src/user_config_keys.rs"))
        .expect("read ActingLab user config keys module");

    const ROOT_DECLARATION: &str = "mod user_config_keys;";
    const ROOT_IMPORT: &str = "use user_config_keys::{config_get, config_set};";
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod user_config_keys;"))
        .collect::<Vec<_>>();
    let imports = main
        .lines()
        .filter(|line| line.contains("user_config_keys::"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private user config keys module declaration"
    );
    assert_eq!(
        imports,
        vec![ROOT_IMPORT],
        "ActingLab main lost the one private user config keys import"
    );

    for definition in [
        "fn config_get(",
        "fn config_set(",
        "fn get_instance_value(",
        "fn set_instance_value(",
    ] {
        assert!(
            user_config_keys.contains(definition),
            "user config keys module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained user config keys owner definition {definition}"
        );
    }

    assert_eq!(
        user_config_keys.matches("pub(super) ").count(),
        2,
        "user config keys owner visibility changed"
    );
    for line in user_config_keys.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "user config keys owner exposed broader visibility: {line}"
        );
    }

    let marker = "pub(super) fn config_get(config: &UserConfig, key: &str) -> CliOutcome<Value> {";
    let (_, owner_tail) = user_config_keys
        .split_once(marker)
        .expect("user config keys module contains owner marker");
    let normalized_owner = format!(
        "fn config_get(config: &UserConfig, key: &str) -> CliOutcome<Value> {{{owner_tail}"
    )
    .replace("pub(super) ", "");
    assert_eq!(
        normalized_owner.lines().count(),
        70,
        "user config keys owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        3_405,
        "user config keys owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "23ce53deda5cc13d6d3c44b2a312c94a60d2dbc68db7d34877a49b878d3463c0",
        "user config keys owner body changed"
    );
}

#[test]
fn actinglab_safe_file_stem_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let safe_file_stem = fs::read_to_string(root.join("apps/actinglab/src/safe_file_stem.rs"))
        .expect("read ActingLab safe file stem module");

    const ROOT_DECLARATION: &str = "mod safe_file_stem;";
    const ROOT_IMPORT: &str = "use safe_file_stem::safe_file_stem;";
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod safe_file_stem;"))
        .collect::<Vec<_>>();
    let imports = main
        .lines()
        .filter(|line| line.contains("safe_file_stem::"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private safe file stem module declaration"
    );
    assert_eq!(
        imports,
        vec![ROOT_IMPORT],
        "ActingLab main lost the one private safe file stem import"
    );

    const DEFINITION: &str = "fn safe_file_stem(value: &str) -> String {";
    assert!(
        safe_file_stem.contains(DEFINITION),
        "safe file stem module lost owner definition"
    );
    assert!(
        !main.contains(DEFINITION),
        "ActingLab main regained safe file stem owner definition"
    );

    assert_eq!(
        safe_file_stem.matches("pub(super) ").count(),
        1,
        "safe file stem owner visibility changed"
    );
    for line in safe_file_stem.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "safe file stem owner exposed broader visibility: {line}"
        );
    }

    let normalized_owner = safe_file_stem.replacen("pub(super) ", "", 1);
    assert_eq!(
        normalized_owner.lines().count(),
        12,
        "safe file stem owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        273,
        "safe file stem owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "2dfd834d20ddb6ec165c3084c8ea002daa4232c1e270559962c9ecad6e0bdcb4",
        "safe file stem owner body changed"
    );
}

#[test]
fn actinglab_sha256_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let sha256 = fs::read_to_string(root.join("apps/actinglab/src/sha256.rs"))
        .expect("read ActingLab SHA-256 module");

    const ROOT_DECLARATION: &str = "mod sha256;";
    const ROOT_IMPORT: &str = "use sha256::{file_sha256, hex_sha256};";
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod sha256;"))
        .collect::<Vec<_>>();
    let imports = main
        .lines()
        .filter(|line| line.contains("sha256::"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private SHA-256 module declaration"
    );
    assert_eq!(
        imports,
        vec![ROOT_IMPORT],
        "ActingLab main lost the one private SHA-256 import"
    );

    for definition in ["fn file_sha256(", "fn hex_sha256("] {
        assert!(
            sha256.contains(definition),
            "SHA-256 module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained SHA-256 owner definition {definition}"
        );
    }

    assert_eq!(
        sha256.matches("pub(super) ").count(),
        2,
        "SHA-256 owner visibility changed"
    );
    for line in sha256.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "SHA-256 owner exposed broader visibility: {line}"
        );
    }

    const CHILD_IMPORTS: &str = concat!(
        "use super::{CliError, CliOutcome};\n",
        "use sha2::{Digest, Sha256};\n",
        "use std::{fs, path::Path};\n\n",
    );
    let raw_owner = sha256
        .strip_prefix(CHILD_IMPORTS)
        .expect("SHA-256 module imports changed");
    let normalized_owner = raw_owner.replacen("pub(super) ", "", 2);
    assert_eq!(
        normalized_owner.lines().count(),
        9,
        "SHA-256 owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        293,
        "SHA-256 owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "f5e673a72156180e77e61ac7a711f741d80da0c6bb84328376b94a5038417748",
        "SHA-256 owner body changed"
    );
}

#[test]
fn actinglab_zip_error_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let zip_error = fs::read_to_string(root.join("apps/actinglab/src/zip_error.rs"))
        .expect("read ActingLab ZIP error module");

    const ROOT_DECLARATION: &str = "mod zip_error;";
    const ROOT_IMPORT: &str = "use zip_error::{zip_io_error, zip_write_error};";
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod zip_error;"))
        .collect::<Vec<_>>();
    let imports = main
        .lines()
        .filter(|line| line.contains("zip_error::"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private ZIP error module declaration"
    );
    assert_eq!(
        imports,
        vec![ROOT_IMPORT],
        "ActingLab main lost the one private ZIP error import"
    );

    for definition in ["fn zip_write_error(", "fn zip_io_error("] {
        assert!(
            zip_error.contains(definition),
            "ZIP error module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained ZIP error owner definition {definition}"
        );
    }

    assert_eq!(
        zip_error.matches("pub(super) ").count(),
        2,
        "ZIP error owner visibility changed"
    );
    for line in zip_error.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "ZIP error owner exposed broader visibility: {line}"
        );
    }

    const CHILD_IMPORTS: &str = "use super::CliError;\nuse std::io;\n\n";
    let raw_owner = zip_error
        .strip_prefix(CHILD_IMPORTS)
        .expect("ZIP error module imports changed");
    let normalized_owner = raw_owner.replacen("pub(super) ", "", 2);
    assert_eq!(
        normalized_owner.lines().count(),
        7,
        "ZIP error owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        244,
        "ZIP error owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "b15204ccc3490b7f5a81a792d7a13496b0809e6931644c4f27b1395cad758889",
        "ZIP error owner body changed"
    );
}

#[test]
fn actinglab_state_roots_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let state_roots = fs::read_to_string(root.join("apps/actinglab/src/state_roots.rs"))
        .expect("read ActingLab state roots module");

    const ROOT_DECLARATION: &str = "mod state_roots;";
    const ROOT_IMPORT: &str =
        "use state_roots::{app_state_root, runtime_state_root, session_state_dir_from_flags};";
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod state_roots;"))
        .collect::<Vec<_>>();
    let imports = main
        .lines()
        .filter(|line| line.contains("state_roots::"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private state roots module declaration"
    );
    assert_eq!(
        imports,
        vec![ROOT_IMPORT],
        "ActingLab main lost the one private state roots import"
    );

    for definition in [
        "fn app_state_root(",
        "fn runtime_state_root(",
        "fn session_state_dir_from_flags(",
    ] {
        assert!(
            state_roots.contains(definition),
            "state roots module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained state roots owner definition {definition}"
        );
    }

    assert_eq!(
        state_roots.matches("pub(super) ").count(),
        3,
        "state roots owner visibility changed"
    );
    for line in state_roots.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "state roots owner exposed broader visibility: {line}"
        );
    }

    const CHILD_IMPORTS: &str = concat!(
        "use super::{CliError, CliOutcome, FlagArgs, RUNTIME_STATE_ROOT_ENV, SESSION_STATE_ENV};\n",
        "use std::{env, path::PathBuf};\n\n",
    );
    let raw_owner = state_roots
        .strip_prefix(CHILD_IMPORTS)
        .expect("state roots module imports changed");
    let normalized_owner = raw_owner.replacen("pub(super) ", "", 3);
    assert_eq!(
        normalized_owner.lines().count(),
        31,
        "state roots owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        1_178,
        "state roots owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "eb07db2e6e1f6cc8384ccf06dad1a45e5b887f425ab0d50d670612c247534783",
        "state roots owner body changed"
    );
}

#[test]
fn actinglab_flag_values_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    const ROOT_DECLARATION: &str = "mod flag_values;";
    const ROOT_IMPORT: &str = concat!(
        "use flag_values::{\n",
        "    parse_match_metric_flag, parse_optional_duration_ms, ",
        "parse_optional_string_value,\n",
        "    parse_optional_unit_f64, parse_optional_usize, parse_record_duration_ms,\n",
        "    parse_touch_backend_override, record_amend_step_id, required_non_empty_flag,\n",
        "    session_record_drift_diagnostics_path, split_csv, stream_check_requested, ",
        "target_argument,\n",
        "};",
    );
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod flag_values;"))
        .collect::<Vec<_>>();
    let imports = main
        .lines()
        .filter(|line| line.contains("flag_values::"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private flag values module declaration"
    );
    assert_eq!(
        imports.len(),
        1,
        "ActingLab main gained another flag values import"
    );
    assert_eq!(
        main.matches(ROOT_IMPORT).count(),
        1,
        "ActingLab main lost the exact private flag values import"
    );

    for definition in [
        "fn parse_optional_duration_ms(",
        "fn parse_optional_usize(",
        "fn parse_optional_string_value(",
    ] {
        assert!(
            flag_values.contains(definition),
            "flag values module lost owner definition {definition}"
        );
        assert!(
            !main.contains(definition),
            "ActingLab main regained flag values owner definition {definition}"
        );
    }

    for line in flag_values.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "flag values owner exposed broader visibility: {line}"
        );
    }

    const CHILD_IMPORTS: &str = concat!(
        "use super::{CliError, CliOutcome, FlagArgs, MatchMetric, TouchBackendChoice};\n",
        "use std::path::PathBuf;\n",
        "use std::time::Duration;\n\n",
    );
    let raw_owner = flag_values
        .strip_prefix(CHILD_IMPORTS)
        .expect("flag values module imports changed");
    let (parser_owner, _) = raw_owner
        .rsplit_once("\npub(super) fn required_non_empty_flag(")
        .expect("flag values module lost the appended required-value owner");
    assert_eq!(
        parser_owner.matches("pub(super) ").count(),
        3,
        "flag values parser-trio visibility changed"
    );
    let normalized_owner = parser_owner
        .replacen("pub(super) ", "", 3)
        .replace(
            concat!(
                "fn parse_optional_usize(\n",
                "    flags: &FlagArgs,\n",
                "    name: &str,\n",
                "    default_value: usize,\n",
                ") -> CliOutcome<usize> {\n",
            ),
            concat!(
                "fn parse_optional_usize(flags: &FlagArgs, name: &str, ",
                "default_value: usize) -> CliOutcome<usize> {\n",
            ),
        )
        .replace(
            concat!(
                "fn parse_optional_string_value(\n",
                "    flags: &FlagArgs,\n",
                "    name: &str,\n",
                ") -> CliOutcome<Option<String>> {\n",
            ),
            concat!(
                "fn parse_optional_string_value(flags: &FlagArgs, name: &str) ",
                "-> CliOutcome<Option<String>> {\n",
            ),
        );
    assert_eq!(
        normalized_owner.lines().count(),
        33,
        "flag values owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        1_219,
        "flag values owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "bf4488b5477458012436cbf5f8e8258bebeb01a184c311b9bfa6413680c6284c",
        "flag values owner body changed"
    );
}

#[test]
fn actinglab_required_non_empty_flag_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    const ROOT_IMPORT: &str = concat!(
        "use flag_values::{\n",
        "    parse_match_metric_flag, parse_optional_duration_ms, ",
        "parse_optional_string_value,\n",
        "    parse_optional_unit_f64, parse_optional_usize, parse_record_duration_ms,\n",
        "    parse_touch_backend_override, record_amend_step_id, required_non_empty_flag,\n",
        "    session_record_drift_diagnostics_path, split_csv, stream_check_requested, ",
        "target_argument,\n",
        "};",
    );
    assert_eq!(
        main.matches(ROOT_IMPORT).count(),
        1,
        "ActingLab main lost the exact required-value root import"
    );
    assert_eq!(
        flag_values.matches("fn required_non_empty_flag(").count(),
        1,
        "flag values module lost the one required-value definition"
    );
    assert!(
        flag_values.contains("pub(super) fn required_non_empty_flag("),
        "required-value owner visibility changed"
    );
    assert!(
        !main.contains("fn required_non_empty_flag("),
        "ActingLab main regained the required-value owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "ActingLab main publicly re-exported flag-value glue"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    const ID_CALL: &str = "let id = required_non_empty_flag(flags, \"--id\")?;";
    const FROM_CALL: &str = "let from = required_non_empty_flag(flags, \"--from\")?;";
    assert_eq!(
        main.matches("required_non_empty_flag(").count(),
        4,
        "ActingLab main required-value caller set changed"
    );
    assert_eq!(
        main.matches(ID_CALL).count(),
        3,
        "ActingLab main lost an exact --id required-value caller"
    );
    assert_eq!(
        main.matches(FROM_CALL).count(),
        1,
        "ActingLab main lost the exact --from required-value caller"
    );

    let marker = "\npub(super) fn required_non_empty_flag(";
    let (_, owner_and_unit_f64) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended required-value owner");
    let (owner_tail, _) = owner_and_unit_f64
        .split_once("\npub(super) fn parse_optional_unit_f64(")
        .expect("flag values module lost the following unit-f64 owner");
    let normalized_owner = format!("fn required_non_empty_flag({owner_tail}");
    assert_eq!(
        normalized_owner.lines().count(),
        7,
        "required-value owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        249,
        "required-value owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "0b7facbdfec294aeec998cc3b628950ed082dd36be61ace9703356fb8bb572cd",
        "required-value owner body changed"
    );
}

#[test]
fn actinglab_optional_unit_f64_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    const ROOT_IMPORT: &str = concat!(
        "use flag_values::{\n",
        "    parse_match_metric_flag, parse_optional_duration_ms, ",
        "parse_optional_string_value,\n",
        "    parse_optional_unit_f64, parse_optional_usize, parse_record_duration_ms,\n",
        "    parse_touch_backend_override, record_amend_step_id, required_non_empty_flag,\n",
        "    session_record_drift_diagnostics_path, split_csv, stream_check_requested, ",
        "target_argument,\n",
        "};",
    );
    assert_eq!(
        main.matches(ROOT_IMPORT).count(),
        1,
        "ActingLab main lost the exact unit-f64 root import"
    );
    assert_eq!(
        flag_values.matches("fn parse_optional_unit_f64(").count(),
        1,
        "flag values module lost the one unit-f64 definition"
    );
    assert!(
        flag_values.contains("pub(super) fn parse_optional_unit_f64("),
        "unit-f64 owner visibility changed"
    );
    assert!(
        !main.contains("fn parse_optional_unit_f64("),
        "ActingLab main regained the unit-f64 owner"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    const DEFAULT_CALL: &str = "\"template_threshold\": parse_optional_unit_f64(flags, \"--default-threshold\")?.unwrap_or(0.95),";
    const NEW_STEP_CALL: &str = "let threshold = parse_optional_unit_f64(flags, \"--threshold\")?;";
    const AMEND_CALL: &str =
        "*target.threshold = parse_optional_unit_f64(flags, \"--threshold\")?;";
    assert_eq!(
        main.matches("parse_optional_unit_f64(").count(),
        5,
        "ActingLab main unit-f64 caller set changed"
    );
    assert_eq!(
        main.matches(DEFAULT_CALL).count(),
        1,
        "ActingLab main lost the exact default-threshold caller"
    );
    assert_eq!(
        main.matches(NEW_STEP_CALL).count(),
        2,
        "ActingLab main lost an exact new-step threshold caller"
    );
    assert_eq!(
        main.matches(AMEND_CALL).count(),
        2,
        "ActingLab main lost an exact amend threshold caller"
    );

    let marker = "\npub(super) fn parse_optional_unit_f64(";
    let (_, owner_and_record_duration) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended unit-f64 owner");
    let (owner_tail, _) = owner_and_record_duration
        .split_once("\npub(super) fn parse_record_duration_ms(")
        .expect("flag values module lost the following record-duration owner");
    let normalized_owner = format!("fn parse_optional_unit_f64({owner_tail}");
    assert_eq!(
        normalized_owner.lines().count(),
        17,
        "unit-f64 owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        623,
        "unit-f64 owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "ce0a6e73bf99c9b59f12410e320d77f65408b7686af02a5dff09bacc194af261",
        "unit-f64 owner body changed"
    );
}

#[test]
fn actinglab_record_duration_flag_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    const ROOT_IMPORT: &str = concat!(
        "use flag_values::{\n",
        "    parse_match_metric_flag, parse_optional_duration_ms, ",
        "parse_optional_string_value,\n",
        "    parse_optional_unit_f64, parse_optional_usize, parse_record_duration_ms,\n",
        "    parse_touch_backend_override, record_amend_step_id, required_non_empty_flag,\n",
        "    session_record_drift_diagnostics_path, split_csv, stream_check_requested, ",
        "target_argument,\n",
        "};",
    );
    assert_eq!(
        main.matches(ROOT_IMPORT).count(),
        1,
        "ActingLab main lost the exact record-duration root import"
    );
    assert_eq!(
        flag_values.matches("fn parse_record_duration_ms(").count(),
        1,
        "flag values module lost the one record-duration definition"
    );
    assert!(
        flag_values.contains("pub(super) fn parse_record_duration_ms("),
        "record-duration owner visibility changed"
    );
    assert!(
        !main.contains("fn parse_record_duration_ms("),
        "ActingLab main regained the record-duration owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "ActingLab main publicly re-exported flag-value glue"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    const SWIPE_CALL: &str = "duration_ms: parse_record_duration_ms(flags, 500)?,";
    const LONG_PRESS_CALL: &str = "duration_ms: parse_record_duration_ms(flags, 700)?,";
    assert_eq!(
        main.matches("parse_record_duration_ms(").count(),
        2,
        "ActingLab main record-duration caller set changed"
    );
    assert_eq!(
        main.matches(SWIPE_CALL).count(),
        1,
        "ActingLab main lost the exact swipe/drag duration caller"
    );
    assert_eq!(
        main.matches(LONG_PRESS_CALL).count(),
        1,
        "ActingLab main lost the exact long-press/long-tap duration caller"
    );

    let marker = "\npub(super) fn parse_record_duration_ms(";
    let (_, owner_and_record_amend_step_id) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended record-duration owner");
    let (owner_tail, _) = owner_and_record_amend_step_id
        .split_once("pub(super) fn record_amend_step_id(")
        .expect("flag values module lost the following record-amend step-id owner");
    let normalized_owner = format!("fn parse_record_duration_ms({owner_tail}");
    assert_eq!(
        normalized_owner.lines().count(),
        17,
        "record-duration owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        557,
        "record-duration owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "629a0606c83110e452458eab128ae1ac5f5531ca519db8b0814fd036447616fe",
        "record-duration owner body changed"
    );
    assert!(
        normalized_owner.contains("failed to parse --duration-ms '{value}': {err}"),
        "record-duration parse error text changed"
    );
    assert!(
        normalized_owner.contains("--duration-ms must be positive"),
        "record-duration zero-value error text changed"
    );
}

#[test]
fn actinglab_record_amend_step_id_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    const ROOT_DECLARATION: &str = "mod flag_values;";
    const ROOT_IMPORT: &str = concat!(
        "use flag_values::{\n",
        "    parse_match_metric_flag, parse_optional_duration_ms, ",
        "parse_optional_string_value,\n",
        "    parse_optional_unit_f64, parse_optional_usize, parse_record_duration_ms,\n",
        "    parse_touch_backend_override, record_amend_step_id, required_non_empty_flag,\n",
        "    session_record_drift_diagnostics_path, split_csv, stream_check_requested, ",
        "target_argument,\n",
        "};",
    );
    let declarations = main
        .lines()
        .filter(|line| line.contains("mod flag_values;"))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations,
        vec![ROOT_DECLARATION],
        "ActingLab main lost the one private flag values module declaration"
    );
    assert_eq!(
        main.matches("flag_values::").count(),
        1,
        "ActingLab main gained another flag values import"
    );
    assert_eq!(
        main.matches(ROOT_IMPORT).count(),
        1,
        "ActingLab main lost the exact record-amend step-id root import"
    );

    assert_eq!(
        flag_values.matches("fn record_amend_step_id(").count(),
        1,
        "flag values module lost the one record-amend step-id definition"
    );
    assert!(
        flag_values.contains("pub(super) fn record_amend_step_id("),
        "record-amend step-id owner visibility changed"
    );
    assert!(
        !main.contains("fn record_amend_step_id("),
        "ActingLab main regained the record-amend step-id owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "ActingLab main publicly re-exported flag-value glue"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );
    for line in flag_values.lines() {
        let trimmed = line.trim_start();
        assert!(
            !trimmed.starts_with("pub ") && !trimmed.starts_with("pub(crate) "),
            "flag values owner exposed broader visibility: {line}"
        );
    }

    const CALL: &str = "let step_id = record_amend_step_id(&flags)?;";
    const CALL_AND_LOOKUP: &str = concat!(
        "            let step_id = record_amend_step_id(&flags)?;\n",
        "            let Some(step) = record.steps.iter_mut().find(|step| step.step_id == step_id) else {",
    );
    assert_eq!(
        main.matches("record_amend_step_id(").count(),
        1,
        "ActingLab main record-amend step-id caller set changed"
    );
    assert_eq!(
        main.matches(CALL).count(),
        1,
        "ActingLab main lost the exact record-amend step-id caller"
    );
    assert_eq!(
        main.matches(CALL_AND_LOOKUP).count(),
        1,
        "ActingLab main changed record-amend step-id caller order"
    );

    let marker = "\npub(super) fn record_amend_step_id(";
    let (_, owner_and_stream_check_requested) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended record-amend step-id owner");
    let (owner_tail, _) = owner_and_stream_check_requested
        .split_once("pub(super) fn stream_check_requested(")
        .expect("flag values module lost the following stream-check owner");
    let normalized_owner = format!("fn record_amend_step_id({owner_tail}");
    assert_eq!(
        normalized_owner.matches('\n').count(),
        12,
        "record-amend step-id owner LF line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        449,
        "record-amend step-id owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "07cbf7af6b9ae384308d5961a7f3226c9aab6226f983d434aa0d098dd62d5009",
        "record-amend step-id owner body changed"
    );
    for invariant in [
        ".optional(\"--step-id\")",
        ".filter(|value| value != \"true\")",
        ".or_else(|| flags.positionals.first().cloned())",
        "session record amend requires <step-id> or --step-id",
        "if value.trim().is_empty()",
        "record amend step id must not be empty",
        "Ok(value)",
    ] {
        assert!(
            normalized_owner.contains(invariant),
            "record-amend step-id invariant changed: {invariant}"
        );
    }
}

#[test]
fn actinglab_split_csv_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let lab2 = fs::read_to_string(root.join("apps/actinglab/src/lab2_cli.rs"))
        .expect("read ActingLab lab2 CLI");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    const ROOT_IMPORT: &str = concat!(
        "use flag_values::{\n",
        "    parse_match_metric_flag, parse_optional_duration_ms, ",
        "parse_optional_string_value,\n",
        "    parse_optional_unit_f64, parse_optional_usize, parse_record_duration_ms,\n",
        "    parse_touch_backend_override, record_amend_step_id, required_non_empty_flag,\n",
        "    session_record_drift_diagnostics_path, split_csv, stream_check_requested, ",
        "target_argument,\n",
        "};",
    );
    assert_eq!(
        main.matches(ROOT_IMPORT).count(),
        1,
        "ActingLab main lost the exact split CSV root import"
    );
    assert_eq!(
        flag_values.matches("fn split_csv(").count(),
        1,
        "flag values module lost the one split CSV definition"
    );
    assert!(
        flag_values.contains("pub(super) fn split_csv("),
        "split CSV owner visibility changed"
    );
    assert!(
        !main.contains("fn split_csv("),
        "ActingLab main regained the split CSV owner"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    const MAIN_CALL: &str =
        "global.instances = Some(split_csv(&require_raw(&raw, index, \"--instances\")?));";
    const TARGETS_CALL: &str = ".flat_map(|value| split_csv(&value))";
    assert_eq!(
        main.matches("split_csv(").count(),
        1,
        "ActingLab main split CSV caller set changed"
    );
    assert!(
        main.contains(MAIN_CALL),
        "ActingLab main lost the exact --instances split CSV caller"
    );
    assert_eq!(
        lab2.matches("split_csv(").count(),
        2,
        "ActingLab lab2 split CSV caller set changed"
    );
    assert_eq!(
        lab2.matches(TARGETS_CALL).count(),
        2,
        "ActingLab lab2 lost the exact targets/fields split CSV callers"
    );

    let marker = "\npub(super) fn split_csv(";
    let (_, owner_and_tests) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended split CSV owner");
    let (owner_tail, _) = owner_and_tests
        .split_once("\n#[cfg(test)]")
        .expect("flag values module lost the following bounded behavior tests");
    let normalized_owner = format!("fn split_csv({owner_tail}");
    assert_eq!(
        normalized_owner.lines().count(),
        8,
        "split CSV owner line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        193,
        "split CSV owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "edc4c9a543d64723f7428067ad6e63668d51c50e882b90f135599fe5ee9a5f1a",
        "split CSV owner body changed"
    );
}

#[test]
fn actinglab_stream_check_requested_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");
    let runtime_stream_adapter =
        fs::read_to_string(root.join("apps/actinglab/src/runtime_stream_adapter.rs"))
            .expect("read ActingLab runtime stream adapter");

    assert_eq!(
        main.matches("stream_check_requested,").count(),
        1,
        "ActingLab main lost the private stream-check root import"
    );
    assert_eq!(
        flag_values.matches("fn stream_check_requested(").count(),
        1,
        "flag values module lost the one stream-check definition"
    );
    assert!(
        flag_values.contains("pub(super) fn stream_check_requested("),
        "stream-check owner visibility changed"
    );
    assert!(
        !main.contains("fn stream_check_requested("),
        "ActingLab main regained the stream-check owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "flag values owner became a public root re-export"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    let caller_serialization = runtime_stream_adapter
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("stream_check_requested("))
        .map(|(index, line)| {
            format!(
                "apps/actinglab/src/runtime_stream_adapter.rs:{}:{}\n",
                index + 1,
                line.trim()
            )
        })
        .collect::<String>();
    assert_eq!(
        format!("{:x}", Sha256::digest(caller_serialization.as_bytes())),
        "b82b22dfc6dd82e6216d6c0c778c2363444bf420d8dd811baa6dcbafd89af532",
        "runtime stream adapter caller serialization changed"
    );

    let marker = "\npub(super) fn stream_check_requested(";
    let (_, owner_and_target_argument) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended stream-check owner");
    let (owner_tail, _) = owner_and_target_argument
        .split_once("pub(super) fn target_argument(")
        .expect("flag values module lost the following target-argument owner");
    let normalized_owner = format!("fn stream_check_requested({owner_tail}");
    assert_eq!(
        normalized_owner.matches('\n').count(),
        4,
        "stream-check owner LF line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        124,
        "stream-check owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "01a8a647eca7a375059814b233c5825df9dbcfcd16213d270c6762ef62cd7684",
        "stream-check owner body changed"
    );
}

#[test]
fn actinglab_target_argument_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");
    let drive_cli = fs::read_to_string(root.join("apps/actinglab/src/drive_cli.rs"))
        .expect("read ActingLab drive CLI");
    let lab2_cli = fs::read_to_string(root.join("apps/actinglab/src/lab2_cli.rs"))
        .expect("read ActingLab lab2 CLI");
    let readonly_cli = fs::read_to_string(root.join("apps/actinglab/src/readonly_cli.rs"))
        .expect("read ActingLab readonly CLI");

    assert_eq!(
        main.matches("target_argument,").count(),
        1,
        "ActingLab main lost the private target-argument root import"
    );
    assert_eq!(
        flag_values.matches("fn target_argument(").count(),
        1,
        "flag values module lost the one target-argument definition"
    );
    assert!(
        flag_values.contains("pub(super) fn target_argument("),
        "target-argument owner visibility changed"
    );
    assert!(
        !main.contains("fn target_argument("),
        "ActingLab main regained the target-argument owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "flag values owner became a public root re-export"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    let mut caller_rows = Vec::new();
    for (path, source) in [
        ("apps/actinglab/src/drive_cli.rs", drive_cli.as_str()),
        ("apps/actinglab/src/lab2_cli.rs", lab2_cli.as_str()),
        ("apps/actinglab/src/readonly_cli.rs", readonly_cli.as_str()),
    ] {
        caller_rows.extend(
            source
                .lines()
                .enumerate()
                .filter(|(_, line)| line.contains("target_argument("))
                .map(|(index, line)| format!("{path}:{}:{}\n", index + 1, line.trim())),
        );
    }
    caller_rows.sort();
    assert_eq!(
        caller_rows.len(),
        3,
        "target-argument production caller set changed"
    );
    let caller_serialization = caller_rows.concat();
    assert_eq!(
        format!("{:x}", Sha256::digest(caller_serialization.as_bytes())),
        "3ede13a2fd78b57945a62411e14898882a7cf6e22d5a59f157e50d82af2fa90d",
        "target-argument caller serialization changed"
    );

    let marker = "\npub(super) fn target_argument(";
    let (_, owner_and_split_csv) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended target-argument owner");
    let (owner_tail, _) = owner_and_split_csv
        .split_once("#[rustfmt::skip]\npub(super) fn session_record_drift_diagnostics_path(")
        .expect("flag values module lost the following drift-diagnostics path owner");
    let normalized_owner = format!("fn target_argument({owner_tail}");
    assert_eq!(
        normalized_owner.matches('\n').count(),
        11,
        "target-argument owner LF line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        362,
        "target-argument owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "3fcd7683f2b4bdf9b74b85170ddd335d7616b040e47b0228685b9ba50e40a6d8",
        "target-argument owner body changed"
    );
    for invariant in [
        ".optional(\"--target\")",
        ".filter(|value| value != \"true\")",
        "return Ok(target);",
        ".positionals",
        ".first()",
        ".cloned()",
        "{command} requires <target> or --target <id>",
    ] {
        assert!(
            normalized_owner.contains(invariant),
            "target-argument invariant changed: {invariant}"
        );
    }
}

#[test]
fn actinglab_session_record_drift_diagnostics_path_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    assert_eq!(
        main.matches("session_record_drift_diagnostics_path,")
            .count(),
        1,
        "ActingLab main lost the private drift-diagnostics path root import"
    );
    assert_eq!(
        flag_values
            .matches("fn session_record_drift_diagnostics_path(")
            .count(),
        1,
        "flag values module lost the one drift-diagnostics path definition"
    );
    assert_eq!(
        flag_values
            .matches(concat!(
                "#[rustfmt::skip]\n",
                "pub(super) fn session_record_drift_diagnostics_path("
            ))
            .count(),
        1,
        "drift-diagnostics path owner lost its exact private visibility or format guard"
    );
    assert!(
        !main.contains("fn session_record_drift_diagnostics_path("),
        "ActingLab main regained the drift-diagnostics path owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "flag values owner became a public root re-export"
    );
    assert_eq!(
        flag_values.matches("use std::path::PathBuf;").count(),
        1,
        "drift-diagnostics path owner lost its exact PathBuf import"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    let caller_rows = main
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("session_record_drift_diagnostics_path("))
        .map(|(index, line)| format!("apps/actinglab/src/main.rs:{}:{}\n", index + 1, line.trim()))
        .collect::<Vec<_>>();
    assert_eq!(
        caller_rows.len(),
        1,
        "drift-diagnostics path production caller set changed"
    );
    let caller_serialization = caller_rows.concat();
    assert_eq!(
        format!("{:x}", Sha256::digest(caller_serialization.as_bytes())),
        "89562bf202efd837ac412907377a479206d5018b729323d3ecb953518011f2d1",
        "drift-diagnostics path caller serialization changed"
    );

    let marker = concat!(
        "\n#[rustfmt::skip]\n",
        "pub(super) fn session_record_drift_diagnostics_path("
    );
    let (_, owner_and_touch_backend) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended drift-diagnostics path owner");
    let (owner_tail, _) = owner_and_touch_backend
        .split_once("pub(super) fn parse_touch_backend_override(")
        .expect("flag values module lost the following touch-backend owner");
    let normalized_owner = format!("fn session_record_drift_diagnostics_path({owner_tail}");
    assert_eq!(
        normalized_owner.matches('\n').count(),
        12,
        "drift-diagnostics path owner LF line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        390,
        "drift-diagnostics path owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "1e969a434fe92b75824078b03b5facd48d2de473455fefc2015cd94bc2add7b0",
        "drift-diagnostics path owner body changed"
    );
    for invariant in [
        ".optional(\"--from-drift-diagnostics\")",
        "return Ok(None);",
        "if value == \"true\"",
        "session record amend --from-drift-diagnostics requires <path>",
        "Ok(Some(PathBuf::from(value)))",
    ] {
        assert!(
            normalized_owner.contains(invariant),
            "drift-diagnostics path invariant changed: {invariant}"
        );
    }

    let ratchet = fs::read_to_string(root.join("ratchet/main_rs_lines.txt"))
        .expect("read ratchet/main_rs_lines.txt")
        .trim()
        .parse::<usize>()
        .expect("ratchet/main_rs_lines.txt must contain one integer");
    assert_eq!(ratchet, 20_732, "drift-diagnostics path ratchet changed");
    assert_eq!(
        main.lines().count(),
        ratchet,
        "drift-diagnostics path move and main.rs ratchet diverged"
    );
}

#[test]
fn actinglab_parse_touch_backend_override_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    assert_eq!(
        main.matches("parse_touch_backend_override,").count(),
        1,
        "ActingLab main lost the private touch-backend root import"
    );
    assert_eq!(
        flag_values
            .matches("fn parse_touch_backend_override(")
            .count(),
        1,
        "flag values module lost the one touch-backend definition"
    );
    assert_eq!(
        flag_values
            .matches("pub(super) fn parse_touch_backend_override(")
            .count(),
        1,
        "touch-backend owner visibility changed"
    );
    assert!(
        !main.contains("fn parse_touch_backend_override("),
        "ActingLab main regained the touch-backend owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "flag values owner became a public root re-export"
    );
    assert_eq!(
        flag_values
            .matches(
                "use super::{CliError, CliOutcome, FlagArgs, MatchMetric, TouchBackendChoice};",
            )
            .count(),
        1,
        "touch-backend owner lost its exact private dependency import"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    const CALL: &str =
        "if parse_touch_backend_override(&flags)?.is_some() || global.touch_backend.is_some() {";
    assert_eq!(
        main.matches(CALL).count(),
        1,
        "ActingLab main lost the exact touch-backend caller expression"
    );
    let caller_rows = main
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("parse_touch_backend_override("))
        .map(|(index, line)| format!("apps/actinglab/src/main.rs:{}:{}\n", index + 1, line.trim()))
        .collect::<Vec<_>>();
    assert_eq!(
        caller_rows.len(),
        1,
        "touch-backend production caller set changed"
    );
    let caller_serialization = caller_rows.concat();
    assert_eq!(
        format!("{:x}", Sha256::digest(caller_serialization.as_bytes())),
        "432a91e7711513861cffd674b652d973a9983e872c5d6f0536cb8070483de036",
        "touch-backend caller serialization changed"
    );

    let marker = "\npub(super) fn parse_touch_backend_override(";
    let (_, owner_and_match_metric) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended touch-backend owner");
    let (owner_tail, _) = owner_and_match_metric
        .split_once("pub(super) fn parse_match_metric_flag(")
        .expect("flag values module lost the following match-metric owner");
    let normalized_owner = format!("fn parse_touch_backend_override({owner_tail}").replace(
        concat!(
            "fn parse_touch_backend_override(\n",
            "    flags: &FlagArgs,\n",
            ") -> CliOutcome<Option<TouchBackendChoice>> {\n",
        ),
        concat!(
            "fn parse_touch_backend_override(flags: &FlagArgs) ",
            "-> CliOutcome<Option<TouchBackendChoice>> {\n",
        ),
    );
    assert_eq!(
        normalized_owner.matches('\n').count(),
        14,
        "touch-backend owner LF line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        484,
        "touch-backend owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "0d9772f83ec6bbf884476c9532941a33e58af14dfae5a99e34f360eea3a4a226",
        "touch-backend owner body changed"
    );
    for invariant in [
        ".optional(\"--touch-backend\")",
        "return Ok(None);",
        "if value == \"true\"",
        "--touch-backend expects auto, auto-fastest, maatouch, minitouch, or adb_shell_input",
        "TouchBackendChoice::parse(&value)",
        ".map(Some)",
        ".map_err(|err| CliError::usage(err.to_string()))",
    ] {
        assert!(
            normalized_owner.contains(invariant),
            "touch-backend invariant changed: {invariant}"
        );
    }

    let ratchet = fs::read_to_string(root.join("ratchet/main_rs_lines.txt"))
        .expect("read ratchet/main_rs_lines.txt")
        .trim()
        .parse::<usize>()
        .expect("ratchet/main_rs_lines.txt must contain one integer");
    assert_eq!(ratchet, 20_732, "touch-backend ratchet changed");
    assert_eq!(
        main.lines().count(),
        ratchet,
        "touch-backend move and main.rs ratchet diverged"
    );
}

#[test]
fn actinglab_parse_match_metric_flag_glue_stays_out_of_main() {
    let root = workspace_root();
    let main =
        fs::read_to_string(root.join("apps/actinglab/src/main.rs")).expect("read ActingLab main");
    let flag_values = fs::read_to_string(root.join("apps/actinglab/src/flag_values.rs"))
        .expect("read ActingLab flag values module");

    assert_eq!(
        main.matches("parse_match_metric_flag,").count(),
        1,
        "ActingLab main lost the private match-metric root import"
    );
    assert_eq!(
        flag_values.matches("fn parse_match_metric_flag(").count(),
        1,
        "flag values module lost the one match-metric definition"
    );
    assert_eq!(
        flag_values
            .matches("pub(super) fn parse_match_metric_flag(")
            .count(),
        1,
        "match-metric owner visibility changed"
    );
    assert!(
        !main.contains("fn parse_match_metric_flag("),
        "ActingLab main regained the match-metric owner"
    );
    assert!(
        !main.contains("pub use flag_values::"),
        "flag values owner became a public root re-export"
    );
    assert_eq!(
        flag_values
            .matches(
                "use super::{CliError, CliOutcome, FlagArgs, MatchMetric, TouchBackendChoice};",
            )
            .count(),
        1,
        "match-metric owner lost its exact private dependency import"
    );
    assert_eq!(
        flag_values.matches("pub(super) ").count(),
        13,
        "flag values module visibility changed"
    );

    const LOCATE_CALL: &str = "let metric = parse_match_metric_flag(&flags)?;";
    const BACKTEST_CALL: &str = "let metric = parse_match_metric_flag(flags)?;";
    const AUTO_REGION_CALL: &str = "Some(parse_match_metric_flag(flags)?)";
    for (call, expected) in [(LOCATE_CALL, 1), (BACKTEST_CALL, 1), (AUTO_REGION_CALL, 1)] {
        assert_eq!(
            main.matches(call).count(),
            expected,
            "ActingLab main changed an exact match-metric caller: {call}"
        );
    }
    let caller_rows = main
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("parse_match_metric_flag("))
        .map(|(index, line)| format!("apps/actinglab/src/main.rs:{}:{}\n", index + 1, line.trim()))
        .collect::<Vec<_>>();
    assert_eq!(caller_rows.len(), 3, "match-metric caller set changed");
    let caller_serialization = caller_rows.concat();
    assert_eq!(
        format!("{:x}", Sha256::digest(caller_serialization.as_bytes())),
        "8283544c5912cf81452cdeee99a1b98eda4379d1b8b3b8597db45461254259f5",
        "match-metric caller serialization changed"
    );

    let marker = "\npub(super) fn parse_match_metric_flag(";
    let (_, owner_and_split_csv) = flag_values
        .rsplit_once(marker)
        .expect("flag values module lost the appended match-metric owner");
    let (owner_tail, _) = owner_and_split_csv
        .split_once("pub(super) fn split_csv(")
        .expect("flag values module lost the following split CSV owner");
    let normalized_owner = format!("fn parse_match_metric_flag({owner_tail}");
    assert_eq!(
        normalized_owner.matches('\n').count(),
        14,
        "match-metric owner LF line count changed"
    );
    assert_eq!(
        normalized_owner.len(),
        501,
        "match-metric owner byte count changed"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized_owner.as_bytes())),
        "ac1654d1a2a1b042afd6192d564ccfd9b699c47ffb981af8a7c47f6eef3526a1",
        "match-metric owner body changed"
    );
    for invariant in [
        ".optional(\"--metric\")",
        ".unwrap_or_else(|| \"ccorr_normed\".to_string())",
        "\"ccorr_normed\" => Ok(MatchMetric::CrossCorrelationNormalized)",
        "\"ccoeff_normed\" => Ok(MatchMetric::CorrelationCoefficientNormalized)",
        "unsupported --metric '{other}', expected ccorr_normed or ccoeff_normed",
    ] {
        assert!(
            normalized_owner.contains(invariant),
            "match-metric invariant changed: {invariant}"
        );
    }
    assert_eq!(
        flag_values
            .matches("fn match_metric_flag_preserves_default_values_and_rejection(")
            .count(),
        1,
        "match-metric behavior test coverage changed"
    );

    let mut actinglab_sources = Vec::new();
    collect_rust_files(&root.join("apps/actinglab/src"), &mut actinglab_sources);
    let definition_count = actinglab_sources
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
                .matches("fn parse_match_metric_flag(")
                .count()
        })
        .sum::<usize>();
    assert_eq!(
        definition_count, 1,
        "ActingLab gained a second match-metric parser or authority"
    );

    let ratchet = fs::read_to_string(root.join("ratchet/main_rs_lines.txt"))
        .expect("read ratchet/main_rs_lines.txt")
        .trim()
        .parse::<usize>()
        .expect("ratchet/main_rs_lines.txt must contain one integer");
    assert_eq!(ratchet, 20_732, "match-metric ratchet changed");
    assert_eq!(
        main.lines().count(),
        ratchet,
        "match-metric move and main.rs ratchet diverged"
    );
}

#[test]
fn main_rs_line_ratchet_matches_checked_in_baseline() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join("apps/actinglab/src/main.rs"))
        .expect("read apps/actinglab/src/main.rs");
    let baseline = fs::read_to_string(root.join("ratchet/main_rs_lines.txt"))
        .expect("read ratchet/main_rs_lines.txt")
        .trim()
        .parse::<usize>()
        .expect("ratchet/main_rs_lines.txt must contain one integer");

    validate_line_ratchet(baseline, source.lines().count()).unwrap();
}
