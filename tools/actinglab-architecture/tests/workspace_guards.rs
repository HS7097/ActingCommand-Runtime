// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use actingcommand_actinglab_architecture::{
    contract_dependency_violations, extract_command_inventory, inspect_contract_fact_matching,
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
    let run = fs::read_to_string(root.join("crates/lab/src/lab_run/api.rs"))
        .expect("read Lab run ingress source");
    let run_api = fs::read_to_string(root.join("crates/lab/src/lab_run_api.rs"))
        .expect("read Lab run API source");
    let cli = fs::read_to_string(root.join("apps/actinglab/src/lab_run.rs"))
        .expect("read ActingLab run CLI source");
    let production_loader = run
        .split_once("fn load_lab_package_for_run")
        .and_then(|(_, tail)| tail.split_once("fn containment_error"))
        .map(|(loader, _)| loader)
        .expect("locate production run loader");

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
        production_loader.contains("ExternallyVerifiedBundle::load"),
        "production run loader bypasses the execution bundle capability"
    );
    for forbidden in ["Sha256Hash::digest", "unwrap_or_else"] {
        assert!(
            !production_loader.contains(forbidden),
            "production run loader self-trusts its package via {forbidden}"
        );
    }
    assert!(
        run_api.contains("pub expected_input_sha256: ExternalExpectedSha256"),
        "LabRunRequest does not require an externally supplied hash type"
    );
    assert!(
        cli.contains("parse_required_external_sha256"),
        "ActingLab production run CLI does not require an external expected hash"
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
            violations.push(normalized);
        }
    }
    assert!(
        violations.is_empty(),
        "artifact issuer escaped contract/store boundary:\n{}",
        violations.join("\n")
    );
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
fn c5_online_lab_run_effects_are_instance_bound_and_runtime_owned() {
    let root = workspace_root();
    let app_environment = fs::read_to_string(root.join("apps/actinglab/src/env_detection.rs"))
        .expect("read ActingLab environment adapter");
    let app_run = fs::read_to_string(root.join("apps/actinglab/src/lab_run.rs"))
        .expect("read ActingLab run adapter");
    let app_readonly = fs::read_to_string(root.join("apps/actinglab/src/readonly_cli.rs"))
        .expect("read ActingLab read-only adapter");
    let lab_run = fs::read_to_string(root.join("crates/lab/src/lab_run/api.rs"))
        .expect("read Lab run ingress");
    let lab_execute = fs::read_to_string(root.join("crates/lab/src/lab_run/execute.rs"))
        .expect("read Lab run execution adapter");
    let lab_context = fs::read_to_string(root.join("crates/lab/src/lab_run/context.rs"))
        .expect("read Lab run context");
    let lab_output = fs::read_to_string(root.join("crates/lab/src/lab_run/output.rs"))
        .expect("read Lab run output");
    let lab_contract = fs::read_to_string(root.join("crates/lab/src/lab_run_api.rs"))
        .expect("read Lab run contract");
    let runtime_input = fs::read_to_string(root.join("crates/runtime-client/src/input.rs"))
        .expect("read Runtime input proxy");

    assert!(
        !root
            .join("apps/actinglab/src/legacy_control_capture.rs")
            .exists()
    );
    for (path, source) in [
        ("apps/actinglab/src/env_detection.rs", &app_environment),
        ("apps/actinglab/src/lab_run.rs", &app_run),
        ("apps/actinglab/src/readonly_cli.rs", &app_readonly),
    ] {
        for forbidden in ["LegacyControl", "legacy_control_capture"] {
            assert!(
                !source.contains(forbidden),
                "{path} regained legacy production authority via {forbidden}"
            );
        }
    }
    for forbidden in ["create_capture_backend", "create_touch_backend"] {
        assert!(
            !app_environment.contains(forbidden),
            "ActingLab Runtime port constructs a device backend via {forbidden}"
        );
    }
    for required in [
        "AppCaptureAuthority::RuntimeByInstance",
        "RuntimeInputBackend::connect",
        "request.instance_alias",
    ] {
        assert!(
            app_environment.contains(required),
            "ActingLab Runtime port lost {required}"
        );
    }
    assert!(lab_run.contains("instance_alias: Some(selected_id.clone())"));
    assert!(lab_execute.contains("instance_alias: Some(instance_alias.to_string())"));
    for (path, source) in [
        ("crates/lab/src/lab_run/api.rs", &lab_run),
        ("crates/lab/src/lab_run/context.rs", &lab_context),
        ("crates/lab/src/lab_run/output.rs", &lab_output),
        ("crates/lab/src/lab_run_api.rs", &lab_contract),
    ] {
        for forbidden in [
            "LabLeaseGuard",
            "lease_root",
            "lab_lease_acquired",
            "lab_lease_released",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} regained private lease authority via {forbidden}"
            );
        }
    }
    for required in [
        "client.acquire_lease",
        "client.release_lease",
        "self.client.input",
    ] {
        assert!(
            runtime_input.contains(required),
            "Runtime input proxy lost scheduler-fenced effect path {required}"
        );
    }
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
