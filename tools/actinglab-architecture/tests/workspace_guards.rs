// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use actingcommand_actinglab_architecture::{
    DeclaredVisibility, FunctionReference, GlueCaller, GlueDeclaration, GlueInvariant, GlueText,
    LedgerOwnerModule, OwnerSymbol, RefusalBranch, SourceFacts, SymbolKind,
    contract_dependency_violations, discover_ledger_owners, extract_command_inventory,
    function_source, inspect_admission_handle_uses, inspect_artifact_byte_writes,
    inspect_call_sites, inspect_contract_fact_matching, inspect_disallowed_lint_escapes,
    inspect_dispatch_arm_calls, inspect_enum_variants, inspect_envelope_sites,
    inspect_field_accesses, inspect_function_origin_terms, inspect_generic_authoring_identity,
    inspect_generic_runtime_identity, inspect_glue_declarations, inspect_lab_source,
    inspect_ledger_append_ingress, inspect_ledger_forbidden_sources, inspect_ledger_public_api,
    inspect_non_reexport_items, inspect_persisted_event_ownership,
    inspect_producer_event_capabilities, inspect_provider_symbol_literals, inspect_public_api,
    inspect_pure_decision_source, inspect_refusal_branches, inspect_source_facts,
    inspect_stderr_writes, inspect_store_write_api, inspect_store_writes,
    inspect_type_constructions, lab_removability_violations, ledger_owns_query_matching,
    reference_calls, reference_matches, resource_tooling_removability_violations,
    workspace_dependency_allow_list_violations, workspace_dependency_violations,
};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("architecture tool must live at tools/<name>")
        .to_path_buf()
}

fn ledger_owners(root: &Path) -> Vec<LedgerOwnerModule> {
    discover_ledger_owners(&root.join("crates/ledger/src/lib.rs"))
        .expect("discover production Ledger owners from module declarations")
}

const GENERIC_NON_CARGO_ROOTS: &[&str] = &["contracts", "tests"];

const GENERIC_AUTHORING_MEMBER_ROOTS: &[&str] = &[
    "apps/actinglab",
    "apps/device-test",
    "crates/lab",
    "crates/resource-tooling",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenericityDomain {
    Runtime,
    Authoring,
    Architecture,
}

fn workspace_genericity_roots(root: &Path) -> BTreeMap<PathBuf, GenericityDomain> {
    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata for genericity");
    let canonical_root = root
        .canonicalize()
        .unwrap_or_else(|error| panic!("resolve workspace root {}: {error}", root.display()));
    let declared_root = metadata["workspace_root"]
        .as_str()
        .expect("cargo metadata workspace_root");
    let declared_root = Path::new(declared_root)
        .canonicalize()
        .unwrap_or_else(|error| panic!("resolve metadata workspace root: {error}"));
    assert_eq!(
        declared_root, canonical_root,
        "cargo metadata belongs to another workspace"
    );
    let members = metadata["workspace_members"]
        .as_array()
        .expect("cargo metadata workspace_members");
    assert!(!members.is_empty(), "cargo workspace has no members");
    let packages = metadata["packages"]
        .as_array()
        .expect("cargo metadata packages");
    let mut member_ids = BTreeSet::new();
    let mut roots = BTreeMap::new();
    for member in members {
        let id = member.as_str().expect("cargo workspace member id");
        assert!(!id.is_empty(), "cargo workspace member id is empty");
        assert!(member_ids.insert(id), "duplicate workspace member {id}");
        let mut matching = packages
            .iter()
            .filter(|package| package["id"].as_str().expect("cargo package id") == id);
        let package = matching
            .next()
            .unwrap_or_else(|| panic!("workspace member {id} has no package"));
        assert!(
            matching.next().is_none(),
            "workspace member {id} resolves to multiple packages"
        );
        let manifest = Path::new(
            package["manifest_path"]
                .as_str()
                .expect("workspace member manifest_path"),
        );
        assert!(
            manifest.is_absolute(),
            "member {id} manifest is not absolute"
        );
        let manifest = manifest
            .canonicalize()
            .unwrap_or_else(|error| panic!("resolve member {id} manifest: {error}"));
        assert!(manifest.is_file(), "member {id} manifest is not a file");
        let directory = manifest.parent().expect("member manifest has a parent");
        let relative = directory
            .strip_prefix(&canonical_root)
            .unwrap_or_else(|_| panic!("member {id} directory is outside the workspace"))
            .to_path_buf();
        let domain = if GENERIC_AUTHORING_MEMBER_ROOTS
            .iter()
            .any(|known| relative == Path::new(known))
        {
            GenericityDomain::Authoring
        } else if relative == Path::new("tools/actinglab-architecture") {
            GenericityDomain::Architecture
        } else {
            GenericityDomain::Runtime
        };
        assert!(
            roots.insert(relative.clone(), domain).is_none(),
            "multiple workspace members share directory {}",
            relative.display()
        );
    }
    assert_eq!(
        roots.len(),
        member_ids.len(),
        "member classification is incomplete"
    );
    roots
}

fn genericity_check_inputs(
    root: &Path,
    roots: &BTreeMap<PathBuf, GenericityDomain>,
    domain: GenericityDomain,
) -> BTreeMap<PathBuf, Vec<PathBuf>> {
    let mut inputs = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (member, actual_domain) in roots {
        if *actual_domain != domain {
            continue;
        }
        let mut files = Vec::new();
        match domain {
            GenericityDomain::Runtime => {
                collect_generic_runtime_files(&root.join(member), &mut files)
            }
            GenericityDomain::Authoring => {
                collect_rust_files(&root.join(member).join("src"), &mut files)
            }
            GenericityDomain::Architecture => {
                panic!("architecture owns counterexamples, not a neutral-source input")
            }
        }
        assert!(
            !files.is_empty(),
            "classified member {} has no check inputs",
            member.display()
        );
        files.sort();
        for file in &files {
            assert!(
                seen.insert(file.clone()),
                "duplicate genericity input {}",
                file.display()
            );
        }
        inputs.insert(member.clone(), files);
    }
    assert_eq!(
        inputs.keys().collect::<BTreeSet<_>>(),
        roots
            .iter()
            .filter_map(|(member, actual)| (*actual == domain).then_some(member))
            .collect(),
        "classified members and checker inputs differ"
    );
    inputs
}

#[test]
fn lab_source_obeys_dependency_law() {
    let root = workspace_root();
    let lab_root = root.join("crates/lab");
    let workspace_manifest =
        fs::read_to_string(root.join("Cargo.toml")).expect("read workspace Cargo.toml");
    assert!(
        workspace_manifest.contains("\"crates/lab\""),
        "workspace must register the required crates/lab member"
    );

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
    let roots = workspace_genericity_roots(&root);
    let mut files = genericity_check_inputs(&root, &roots, GenericityDomain::Runtime)
        .into_values()
        .flatten()
        .collect::<Vec<_>>();
    for owned_root in GENERIC_NON_CARGO_ROOTS {
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
    let root = workspace_root();
    let roots = workspace_genericity_roots(&root);
    let runtime = genericity_check_inputs(&root, &roots, GenericityDomain::Runtime);
    let authoring = genericity_check_inputs(&root, &roots, GenericityDomain::Authoring);
    let mut classified = runtime
        .keys()
        .chain(authoring.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(classified.insert(PathBuf::from("tools/actinglab-architecture")));
    assert_eq!(
        classified,
        roots.keys().cloned().collect(),
        "actual member classification/checker coverage differs"
    );
    for (member, domain) in &roots {
        println!("genericity member {}: {domain:?}", member.display());
    }
    println!("genericity non-Cargo roots: {GENERIC_NON_CARGO_ROOTS:?}");
    for required_root in [
        "crates/host-metrics",
        "crates/policy",
        "crates/runtime-database",
        "crates/runtime-state",
        "crates/selection-policy",
    ] {
        assert_eq!(
            roots.get(Path::new(required_root)),
            Some(&GenericityDomain::Runtime),
            "C2 generic Runtime guard does not cover {required_root}"
        );
    }
    for required_root in GENERIC_AUTHORING_MEMBER_ROOTS {
        assert_eq!(
            roots.get(Path::new(required_root)),
            Some(&GenericityDomain::Authoring),
            "R2-F generic authoring guard does not cover {required_root}"
        );
    }
    assert_eq!(
        roots.get(Path::new("tools/actinglab-architecture")),
        Some(&GenericityDomain::Architecture),
        "the architecture member owns the policy counterexamples"
    );
    for (member_root, domain) in roots {
        if domain != GenericityDomain::Runtime {
            continue;
        }
        let counterexample = "const SERVER_BA: &str = \"neutral\";";
        let violations = inspect_generic_runtime_identity(
            &format!("{}/src/lib.rs", member_root.display()),
            counterexample,
        );
        assert!(
            !violations.is_empty(),
            "C2 counterexample escaped in {}",
            member_root.display()
        );
    }
}

#[test]
fn r2f_product_and_authoring_paths_have_no_builtin_game_identity() {
    let root = workspace_root();
    let roots = workspace_genericity_roots(&root);
    let files = genericity_check_inputs(&root, &roots, GenericityDomain::Authoring)
        .into_values()
        .flatten();

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
                "actingcommand-device"
                    | "actingcommand-recognition"
                    | "actingcommand-runtime-database"
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
    // Workflow #314 fw2: its own lease goes through the proxy, a caller's held lease
    // through the Runtime client; both reach input only through the Runtime.
    assert!(
        input.contains("Acquired(RuntimeInputProxy)")
            && input.contains("proxy.input(action)")
            && input.contains("client.input(token, action)")
            && !input.contains("AdbConfig")
            && !input.contains("ExecutionBackendProvider"),
        "ActingLab input adapter must remain a Runtime proxy without provider authority"
    );
    assert!(
        capture_production.contains("client: Option<RuntimeClient>")
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
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host/contained_task.rs"))
        .expect("read Runtime contained-task adapter");
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
        host.contains(
            "ExternalExpectedSha256::parse_hex(request.expected_sha256().legacy_sha256()"
        ) && host.contains("PreparedContainedTask::load_path(")
            && host.contains("request.expected_sha256(),")
            && host.contains("PreparedContainedTask::load(instance_alias, &bytes, expected)"),
        "Runtime host must bind the client package reference to contained package admission"
    );
    for forbidden in ["Sha256Hash::digest", "unwrap_or_else"] {
        assert!(
            !contained.contains(forbidden),
            "contained task ingress self-trusts its package via {forbidden}"
        );
    }
    assert!(
        cli.contains("required_package_reference")
            && cli.contains("PackageInput::open_declared(&flags, reference)")
            && cli.contains("PackageInput::declared_reference(flags)")
            && cli.contains("package_reader.reference.clone()")
            && cli.contains("ContainedTaskRequest::new")
            && cli.contains("run_contained_task(&instance, request)"),
        "ActingLab production run CLI does not require an external package reference"
    );
}

#[test]
fn c5_offline_package_simulation_reuses_the_contained_task_kernel_without_device_authority() {
    let root = workspace_root();
    let lab_package_control =
        fs::read_to_string(root.join("apps/actinglab/src/lab_package_control.rs"))
            .expect("read ActingLab Lab/package control source");
    let capabilities = fs::read_to_string(root.join("apps/actinglab/src/commands/capabilities.rs"))
        .expect("read ActingLab capability source");
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
    let contained_task =
        fs::read_to_string(root.join("crates/execution-kernel/src/contained_task.rs"))
            .expect("read production contained-task interpreter source");
    let lab_run = fs::read_to_string(root.join("apps/actinglab/src/lab_run.rs"))
        .expect("read production Lab run CLI source");

    for (source, required) in [
        (
            &lab_package_control,
            "\"dry-run\" => package_cli::run_offline(global, &flags)",
        ),
        (&capabilities, "package_cli::offline_capability()"),
    ] {
        assert!(
            source.contains(required),
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
        "task.run_with_options(&mut runtime, ContainedTaskRunOptions::offline_simulation())",
        "OfflineBoundary::EffectIntercepted",
        "executed: false",
    ] {
        assert!(
            offline_kernel_production.contains(required),
            "offline adapter stopped delegating to the production contained-task kernel via {required}"
        );
    }
    for required in [
        "pub fn run<R: ContainedTaskRuntime>",
        "self.run_with_options(runtime, ContainedTaskRunOptions::default())",
        "pub(crate) fn run_with_options<R: ContainedTaskRuntime>",
    ] {
        assert!(
            contained_task.contains(required),
            "production and offline paths stopped sharing one contained-task interpreter via {required}"
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
        lab_package_control.contains("\"package run requires an exclusive_drain LabLease")
            && lab_package_control.contains("\"lab_lease_required\""),
        "package run must remain a blocked compatibility boundary"
    );
    assert!(
        lab_package_control.contains("\"operation run requires Runtime scheduler admission")
            && lab_package_control.contains("\"lab_lease_required\""),
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
    let contained = fs::read_to_string(root.join("crates/execution-kernel/src/contained_task.rs"))
        .expect("read contained task execution source");

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
        ".next_directive(&candidates)",
        ".operation_succeeded(",
        ".operation_needs_recovery(",
        "RunTerminal::SuccessorSuggested",
    ] {
        assert!(
            contained.contains(required),
            "contained task no longer consumes execution-owned transition {required}"
        );
    }
    for forbidden in [
        "run_recovery_bundle(",
        ".load_operation_bundle(",
        "recovery_started",
        "recovery_result",
    ] {
        assert!(
            !lab_api.contains(forbidden) && !contained.contains(forbidden),
            "task consumer gained direct recovery chaining via {forbidden}"
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
    let root = workspace_root()
        .canonicalize()
        .expect("resolve checked workspace root");
    let owners = ledger_owners(&root);
    for owner in &owners {
        println!(
            "Ledger owner {}: {}",
            owner.module,
            owner
                .path
                .strip_prefix(&root)
                .expect("owner inside workspace")
                .display()
        );
    }
    let append_violations =
        inspect_ledger_append_ingress(&owners).expect("inspect global append ingress");
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

// Workflow #314 FENCED-CLOSE-v1: approved issuer/close invariant.
// Source first red: 14b7addc crates/device/src/error.rs DeviceCloseAuthority
// had a zero-field FencedDeviceWrite; Workflow #314 issuecomment-5766924100.
// Workflow #314 fw2: the issuer inventory covers both purposes, naming the scheduler
// admission that mints Business and the one that mints ResourceClose.
#[test]
fn fenced_close_requires_scheduler_witness() {
    let sources = workspace_sources(&["crates", "apps", "providers", "tools"]);
    let violations = actingcommand_actinglab_architecture::inspect_fenced_write_issuers(&sources)
        .expect("parse fenced-write issuer source inventory");
    assert!(
        violations.is_empty(),
        "fenced-write issuer violations:\n{}",
        violations.join("\n")
    );
}

fn workspace_sources(directories: &[&str]) -> Vec<(String, String)> {
    let root = workspace_root();
    let mut files = Vec::new();
    for directory in directories {
        collect_rust_files(&root.join(directory), &mut files);
    }
    files.sort();
    files
        .into_iter()
        .map(|file| {
            let path = file
                .strip_prefix(&root)
                .expect("workspace source")
                .to_string_lossy()
                .replace('\\', "/");
            let source =
                fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
            (path, source)
        })
        .collect()
}

// Workflow #314 goal 6: device and kernel input writes take `FencedWrite` and closes take
// `DeviceCloseAuthority`. The table is printed so its dry-run record stays reproducible.
#[test]
fn fenced_write_shape_table_carries_witness_and_close_authority() {
    let sources = workspace_sources(&["crates/device/src", "crates/execution-kernel/src"]);
    let report = actingcommand_actinglab_architecture::inspect_fenced_write_shapes(&sources)
        .expect("parse device and kernel shape table");
    for row in &report.rows {
        println!("{row}");
    }
    assert!(
        report.violations.is_empty(),
        "fenced write shape violations:\n{}",
        report.violations.join("\n")
    );
}

// Workflow #314 goal 5: `SemanticInputExecutor` is Lab's only input face, and `crates/lab`
// names no device input backend.
#[test]
fn lab_input_face_is_semantic_input_executor() {
    let lab = workspace_sources(&["crates/lab"]);
    let device = workspace_sources(&["crates/device/src"]);
    let report = actingcommand_actinglab_architecture::inspect_lab_input_face(&lab, &device)
        .expect("parse Lab input face inventory");
    println!("LabInputPort writes: {:?}", report.port_writes);
    println!("input faces: {:?}", report.faces);
    println!("forbidden device names: {:?}", report.forbidden);
    assert!(
        report.violations.is_empty(),
        "Lab input face violations:\n{}",
        report.violations.join("\n")
    );
}

/// Runtime-domain crates never write to stderr: diagnostics go to the ledger and failures
/// return as explicit errors; the process shells under `apps/` are the only exception. The scan
/// covers every `.rs` file under `crates/` and `providers/`, skipping test files (`tests.rs` and
/// anything under a `tests/` directory) and, inside a file, `#[cfg(test)]` items and `mod tests`.
/// Fail-closed: a stderr site missing from `STDERR_ALLOW_LIST` fails with its `path:line`. A new
/// site must be added to the list in the same PR that introduces it, with its reason; a listed
/// entry that no longer matches any site fails too, so the list stays exact.
#[test]
fn runtime_domain_never_writes_stderr() {
    /// Workspace-relative file (matched as the `path:` prefix of a reported violation) and reason.
    const STDERR_ALLOW_LIST: &[(&str, &str)] = &[];

    let root = workspace_root();
    let mut files = Vec::new();
    for directory in ["crates", "providers"] {
        collect_rust_files(&root.join(directory), &mut files);
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "no Rust sources found under crates/ or providers/"
    );
    let mut violations = Vec::new();
    for file in files {
        let path = file
            .strip_prefix(&root)
            .expect("workspace source")
            .to_string_lossy()
            .replace('\\', "/");
        if path
            .split('/')
            .any(|component| component == "tests" || component == "tests.rs")
        {
            continue;
        }
        let source =
            fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
        violations.extend(
            inspect_stderr_writes(&path, &source)
                .unwrap_or_else(|error| panic!("scan {path} for stderr writes: {error}")),
        );
    }
    let mut unused_allowances = STDERR_ALLOW_LIST
        .iter()
        .map(|(path, _)| *path)
        .collect::<BTreeSet<_>>();
    let mut unexpected = Vec::new();
    for violation in violations {
        match STDERR_ALLOW_LIST
            .iter()
            .find(|(path, _)| violation.starts_with(&format!("{path}:")))
        {
            Some((path, _)) => {
                unused_allowances.remove(path);
            }
            None => unexpected.push(violation),
        }
    }
    assert!(
        unexpected.is_empty(),
        "stderr writes in Runtime-domain crates (a site may only join STDERR_ALLOW_LIST in the PR that adds it):\n{}",
        unexpected.join("\n")
    );
    assert!(
        unused_allowances.is_empty(),
        "stale STDERR_ALLOW_LIST entries without a matching site:\n{}",
        unused_allowances.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn contract_has_no_public_value_payload_or_persisted_fact() {
    let root = workspace_root();
    let owners = ledger_owners(&root);
    let mut files = vec![root.join("crates/actingcommand-contract/src/event.rs")];
    collect_rust_files(
        &root.join("crates/actingcommand-contract/src/event"),
        &mut files,
    );
    let mut violations =
        inspect_ledger_public_api(&owners).expect("inspect all formal Ledger public surfaces");
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
    let owners = ledger_owners(&root);
    let forbidden = [
        "ClassifiedField",
        "StructuredPayloadDraft",
        "ErasedSanitizedEventDraft",
        "take_hook",
        "set_hook",
        "catch_unwind",
        "events_after(",
    ];
    let mut violations =
        inspect_ledger_forbidden_sources(&owners).expect("inspect complete C1 Ledger owner set");
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
    assert!(host.contains("LedgerMaintenance::acquire"));
    assert!(host.contains(".open_writer("));
    assert!(host.contains("verify_recovery_reference(reference)"));
    assert!(store.contains("pub fn verify_recovery_reference"));
}

#[test]
fn forensic_leaf_dependency_boundary_is_narrow_and_production_free() {
    let root = workspace_root();
    let leaf_manifest = root.join("crates/ledger-forensics/Cargo.toml");
    let app_manifest = root.join("apps/ledger-forensics/Cargo.toml");
    assert!(
        leaf_manifest.is_file(),
        "forensic leaf manifest is absent from the workspace"
    );
    assert!(
        app_manifest.is_file(),
        "actingledger application manifest is absent from the workspace"
    );

    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    let leaf = packages
        .iter()
        .find(|package| package["name"] == "actingcommand-ledger-forensics")
        .expect("forensic leaf package");
    let app = packages
        .iter()
        .find(|package| package["name"] == "actingledger")
        .expect("actingledger package");
    let checker = packages
        .iter()
        .find(|package| package["name"] == "actingcommand-vision-provider-check")
        .expect("Provider ledger consumer package");
    let device_test = packages
        .iter()
        .find(|package| package["name"] == "actingcommand-device-test")
        .expect("non-production device ledger consumer package");

    let internal_dependencies = |package: &serde_json::Value| {
        let mut names = package["dependencies"]
            .as_array()
            .expect("package dependencies")
            .iter()
            .filter(|dependency| dependency["kind"].is_null())
            .filter_map(|dependency| dependency["name"].as_str())
            .filter(|name| name.starts_with("actingcommand-") || *name == "actingledger")
            .map(str::to_owned)
            .collect::<Vec<_>>();
        names.sort_unstable();
        names
    };
    assert_eq!(
        internal_dependencies(leaf),
        vec![
            "actingcommand-artifact-store".to_owned(),
            "actingcommand-contract".to_owned(),
            "actingcommand-ledger".to_owned()
        ],
        "forensic leaf internal dependency boundary changed"
    );
    assert_eq!(
        internal_dependencies(app),
        vec![
            "actingcommand-contract".to_owned(),
            "actingcommand-ledger-forensics".to_owned()
        ],
        "actingledger consumes the shared installation occupancy and forensic leaf only"
    );
    // F2: the checker consumes B and retains mechanical artifact parsing only.
    // First red: https://github.com/HS7097/ActingCommand-Runtime/actions/runs/34145595031
    assert_eq!(
        internal_dependencies(checker),
        vec![
            "actingcommand-contract".to_owned(),
            "actingcommand-ledger-forensics".to_owned(),
            "actingcommand-vision-ffi".to_owned(),
        ],
        "Provider checker internal dependency boundary changed"
    );
    // DEVICE-TEST-B-READ-v1: the named non-production tool also consumes B.
    // Its capture factory uses FrameStore and the existing memory source for prime admission.
    // First red: https://github.com/HS7097/ActingCommand-Runtime/actions/runs/35676181277/attempts/1
    assert_eq!(
        internal_dependencies(device_test),
        vec![
            "actingcommand-artifact-store".to_owned(),
            "actingcommand-contract".to_owned(),
            "actingcommand-device".to_owned(),
            "actingcommand-execution-kernel".to_owned(),
            "actingcommand-host-metrics".to_owned(),
            "actingcommand-ledger-forensics".to_owned(),
            "actingcommand-page-detector".to_owned(),
            "actingcommand-recognition".to_owned(),
            "actingcommand-recognition-pack".to_owned(),
            "actingcommand-scheduler".to_owned(),
        ],
        "device-test internal dependency boundary changed"
    );

    let artifact_dependency = leaf["dependencies"]
        .as_array()
        .expect("leaf dependencies")
        .iter()
        .find(|dependency| {
            dependency["kind"].is_null() && dependency["name"] == "actingcommand-artifact-store"
        })
        .expect("forensic leaf artifact-store dependency");
    assert_eq!(
        artifact_dependency["uses_default_features"],
        serde_json::Value::Bool(false),
        "forensic leaf must disable artifact-store default capture features"
    );

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "tree",
            "-p",
            "actingcommand-ledger-forensics",
            "--edges",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(&root)
        .output()
        .expect("run cargo tree for forensic leaf");
    assert!(
        output.status.success(),
        "cargo tree for forensic leaf failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dependency_tree = String::from_utf8(output.stdout).expect("cargo tree must emit UTF-8");
    for forbidden in [
        "actingcommand-runtime-host",
        "actingcommand-runtime-client",
        "actingcommand-device",
        "actingcommand-scheduler",
    ] {
        assert!(
            !dependency_tree
                .lines()
                .any(|line| line.starts_with(forbidden)),
            "forensic leaf normal dependency closure reaches {forbidden}"
        );
    }

    let production_dependants = packages
        .iter()
        .filter(|package| {
            !matches!(
                package["name"].as_str(),
                Some(
                    "actingledger"
                        | "actingcommand-vision-provider-check"
                        | "actingcommand-device-test"
                )
            )
        })
        .filter(|package| {
            package["dependencies"]
                .as_array()
                .expect("package dependencies")
                .iter()
                .any(|dependency| {
                    dependency["kind"].is_null()
                        && matches!(
                            dependency["name"].as_str(),
                            Some(
                                "actingcommand-ledger-forensics"
                                    | "actingcommand-vision-provider-check"
                                    | "actingcommand-device-test"
                            )
                        )
                })
        })
        .filter_map(|package| package["name"].as_str())
        .collect::<Vec<_>>();
    assert!(
        production_dependants.is_empty(),
        "production packages depend on forensic leaf or its tool consumers: {}",
        production_dependants.join(", ")
    );
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
fn c5_runtime_status_registry_is_owned_by_the_resident_control_plane() {
    let root = workspace_root();
    let contract = fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
        .expect("read Runtime contract");
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
        .expect("read Runtime host");
    let client = fs::read_to_string(root.join("crates/runtime-client/src/client.rs"))
        .expect("read Runtime client");
    let lab = fs::read_to_string(root.join("crates/lab/src/lib.rs")).expect("read Lab facade");

    // The host-split owner of control_plane_status is a HOST_SPLIT entry (read_events).
    assert!(contract.contains("RuntimeControlPlaneStatus"));
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
    let monitor_control_path = "crates/runtime-host/src/host/monitor_control.rs";
    let monitor_control = fs::read_to_string(root.join(monitor_control_path))
        .expect("read Runtime host monitor control");
    let client = fs::read_to_string(root.join("crates/runtime-client/src/client.rs"))
        .expect("read Runtime client");
    let lab = fs::read_to_string(root.join("crates/lab/src/lib.rs")).expect("read Lab facade");

    assert!(contract.contains("ConfigureMonitor"));
    assert!(contract.contains("MonitorStatus"));
    assert!(registry.contains("struct MonitorRegistry"));
    assert!(registry.contains("struct DueMonitorProbe"));
    assert!(registry.contains("prepare_completion"));
    assert!(registry.contains("prepare_failure"));
    assert!(registry.contains("MONITOR_FILE_NAME"));
    assert!(host.contains("monitor_registry: OnceLock<Mutex<MonitorRegistry>>"));
    // The monitor_control definitions, their visibility, callers and delegation (the probe's
    // artifact store, capture pipeline and frame persistence; the completed / recovery
    // payloads) are HOST_SPLIT entries; the probe's error mapping stays here.
    let monitor_probe = function_source(
        monitor_control_path,
        &monitor_control,
        Some("HostShared"),
        "run_monitor_probe",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(monitor_probe.contains("let error = RuntimeHostError::artifact(error)"));
    let coordination_start = monitor_control
        .find("    fn record_monitor_recovery_coordination(")
        .expect("monitor recovery coordination start");
    let coordination_end = monitor_control[coordination_start..]
        .find("    fn finish_monitor_failure(")
        .map(|offset| coordination_start + offset)
        .expect("monitor recovery coordination end");
    let coordination = &monitor_control[coordination_start..coordination_end];
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
                        | "actingcommand-resource-tooling"
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
fn c3b_selection_policy_is_a_pure_decision_crate() {
    let root = workspace_root();
    let metadata: serde_json::Value =
        serde_json::from_str(&workspace_metadata()).expect("parse cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    let selection_policy = packages
        .iter()
        .find(|package| package["name"] == "actingcommand-selection-policy")
        .expect("selection-policy package");
    let mut dependency_names = selection_policy["dependencies"]
        .as_array()
        .expect("selection-policy dependencies")
        .iter()
        .filter_map(|dependency| dependency["name"].as_str())
        .collect::<Vec<_>>();
    dependency_names.sort_unstable();
    dependency_names.dedup();
    assert_eq!(
        dependency_names,
        ["actingcommand-contract", "serde", "serde_json", "sha2"],
        "selection-policy takes exactly the pure decision dependencies"
    );

    let mut sources = Vec::new();
    collect_rust_files(&root.join("crates/selection-policy/src"), &mut sources);
    assert!(
        !sources.is_empty(),
        "crates/selection-policy contains no Rust source files"
    );
    let bin_root = root.join("crates/selection-policy/src/bin");
    for path in sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        // The crate's own purity test names these tokens, so only production source counts.
        let source = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source")
            .to_owned();
        for forbidden in [
            "TcpStream",
            "GlobalLedger",
            "LeaseToken",
            "SeedScheduler",
            "RuntimeClient",
            "actingcommand_device",
            "actingcommand_lab",
            "actingcommand_ledger",
            "actingcommand_runtime_host",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} contains forbidden decision-crate token {forbidden}",
                path.display()
            );
        }
        // The offline debugging binary is the crate's declared IO shell; the library half
        // reads no clock and no file, and the walk covers every module it gains later.
        if path.starts_with(&bin_root) {
            continue;
        }
        for forbidden in [
            "std::fs",
            "std::net",
            "std::process",
            "std::thread::sleep",
            "SystemTime::now",
            "Instant::now",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} contains forbidden decision-crate token {forbidden}",
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
    let host_path = "crates/runtime-host/src/host/observation.rs";
    let host = fs::read_to_string(root.join(host_path)).expect("read Runtime observation adapter");
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
    assert!(contract.contains("Self::Input {\n                token,\n                action,\n                frame,\n            }"));

    // The owner, visibility, callers and delegation of capture_sequence (to
    // capture_readonly_observation, thread::sleep and CaptureSequence::new) are HOST_SPLIT
    // entries; its read-only seam stays here.
    let host_sequence = function_source(host_path, &host, Some("HostShared"), "capture_sequence")
        .unwrap_or_else(|error| panic!("{error}"));
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
    let session_management =
        fs::read_to_string(root.join("apps/actinglab/src/session_management.rs"))
            .expect("read ActingLab session management source");
    let adapter = fs::read_to_string(root.join("apps/actinglab/src/runtime_session_adapter.rs"))
        .expect("read Runtime session adapter");

    for required in [
        "runtime_session_adapter::run_status",
        "runtime_session_adapter::run_monitor_policy",
    ] {
        assert!(
            session_management.contains(required),
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
    let monitor_stream = fs::read_to_string(root.join("apps/actinglab/src/monitor_stream.rs"))
        .expect("read ActingLab monitor stream source");
    let lab2 = fs::read_to_string(root.join("apps/actinglab/src/lab2_cli.rs"))
        .expect("read ActingLab Lab2 adapter");
    let session = fs::read_to_string(root.join("apps/actinglab/src/runtime_session_adapter.rs"))
        .expect("read Runtime session adapter");
    let stream = fs::read_to_string(root.join("apps/actinglab/src/runtime_stream_adapter.rs"))
        .expect("read Runtime stream adapter");

    for (source, required) in [
        (
            &monitor_stream,
            "runtime_session_adapter::retired_authority(\"monitor\", args)",
        ),
        (
            &main,
            "\"daemon\" => runtime_session_adapter::retired_authority(sub, args)",
        ),
        (
            &main,
            "\"request\" => runtime_session_adapter::retired_authority(sub, args)",
        ),
        (
            &main,
            "\"journal\" => runtime_session_adapter::retired_authority(sub, args)",
        ),
        (
            &main,
            "\"events\" => runtime_session_adapter::retired_authority(sub, args)",
        ),
        (
            &main,
            "\"response\" => runtime_session_adapter::retired_authority(sub, args)",
        ),
        (
            &main,
            "\"request-state\" => runtime_session_adapter::retired_authority(sub, args)",
        ),
        (
            &main,
            "\"lease\" => runtime_session_adapter::retired_authority(sub, args)",
        ),
    ] {
        assert!(
            source.contains(required),
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
    let host = fs::read_to_string(root.join("crates/runtime-host/src/host/contained_task.rs"))
        .expect("read Runtime contained-task adapter");
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
    let mut exemptions = BTreeSet::new();
    for exemption in snapshot["pipeline_exemptions"]
        .as_array()
        .expect("snapshot pipeline_exemptions must be an array")
    {
        let command = exemption["command"]
            .as_str()
            .expect("pipeline exemption command must be a string");
        assert!(
            exemptions.insert(command),
            "duplicate pipeline exemption {command}"
        );
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
    assert_eq!(
        exemptions,
        BTreeSet::from([
            "help",
            "version",
            "doctor",
            "scheduler status",
            "scheduler pause",
            "scheduler resume",
            "scheduler start",
            "scheduler stop",
        ]),
        "pipeline exemptions must match their named command scope"
    );
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
    let metadata = workspace_metadata();
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
    let document: serde_json::Value =
        serde_json::from_str(&metadata).expect("parse cargo metadata");
    assert!(
        document["packages"].as_array().is_some_and(|packages| {
            packages
                .iter()
                .any(|package| package["name"] == "actingcommand-lab")
        }),
        "the required crates/lab member must provide the actingcommand-lab package"
    );
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
    // Workflow #349: resource generation shares kernel's offline admission/qualification.
    // Device types are transitively present through that owner; tooling has no direct device
    // dependency. Production-to-tooling removability above remains mandatory.
    let tooling = document["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "actingcommand-resource-tooling")
        .unwrap();
    assert!(
        !tooling["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| dependency["name"] == "actingcommand-device")
    );
}

// Workflow #310 work packages C and E in one slice (C+E, frozen model: the #310 coordinator
// comment "C+E 冻模型"). C1: the Lab environment bridge and re-export stay in crates/lab; this
// guard names their owners and checks definitions, the bridge-to-entry call relationship and the
// bridge's callers. The dependency-graph guards above stay as they are.

const PACK_CONTAINMENT_CRATE: &str = "actingcommand_pack_containment";
const RESOURCE_TOOLING_CRATE: &str = "actingcommand_resource_tooling";
const LAB_CRATE: &str = "actingcommand_lab";

/// C1: one definition of a Lab bridge-chain owner.
struct BridgeDefinition {
    /// `Owner::name`, or `name` for a free function.
    name: &'static str,
    /// For a Lab bridge, its resource-tooling entry: the reference its body must make, as
    /// `reference_matches` reads it once the file's `use` bindings resolve the paths.
    /// `self.inner.<method>` is the wrapped resource-tooling catalog, which only
    /// each Lab forwarding method reaches its resource-tooling entry. Owner definitions
    /// have no entry.
    entry: Option<&'static str>,
}

const fn owner_entry(name: &'static str) -> BridgeDefinition {
    BridgeDefinition { name, entry: None }
}

const fn lab_bridge(name: &'static str, entry: &'static str) -> BridgeDefinition {
    BridgeDefinition {
        name,
        entry: Some(entry),
    }
}

/// C1: one owner module of the Lab bridge chain and its production definitions.
struct BridgeOwner {
    module: &'static str,
    file: &'static str,
    definitions: &'static [BridgeDefinition],
}

/// pack-containment owns pure source parsing; resource-tooling owns content-directory
/// digest/bundle and MAA compilation. Lab forwards to its declared resource-tooling entry.
const LAB_BRIDGE_OWNERS: &[BridgeOwner] = &[
    BridgeOwner {
        module: "pack-containment",
        file: "crates/pack-containment/src/source/mod.rs",
        definitions: &[
            owner_entry("OperationParser::build_all"),
            owner_entry("OperationParser::build_selected"),
            owner_entry("OperationParser::canonical_task"),
            owner_entry("source_file_requests"),
            owner_entry("validate_phases_bundle"),
            owner_entry("validate_post_admission_ocr_bundle"),
        ],
    },
    BridgeOwner {
        module: "resource-tooling",
        file: "crates/resource-tooling/src/package_directory.rs",
        definitions: &[owner_entry("package_digest"), owner_entry("package_bundle")],
    },
    BridgeOwner {
        module: "resource-tooling",
        file: "crates/resource-tooling/src/maa_task_graph.rs",
        definitions: &[owner_entry("compile_maa_task_graph")],
    },
    BridgeOwner {
        module: "lab",
        file: "crates/lab/src/package_build.rs",
        definitions: &[
            lab_bridge(
                "Lab::package_digest",
                "actingcommand_resource_tooling::package_digest",
            ),
            lab_bridge(
                "Lab::package_bundle",
                "actingcommand_resource_tooling::package_bundle",
            ),
        ],
    },
];
/// C1: Lab's re-export module holds only `pub use` items, each from resource-tooling, and
/// re-exports these names.
const LAB_REEXPORT_MODULE: (&str, &[&str]) = (
    "crates/lab/src/maa_task_graph.rs",
    &["compile_maa_task_graph"],
);

/// C1: the only production files that call the Lab bridge; their test items are exempt through
/// their cfg(test) scope.
const LAB_BRIDGE_CALLERS: &[&str] = &[
    "apps/actinglab/src/maa_task_graph.rs",
    "apps/actinglab/src/package_build.rs",
];

fn bridge_problem(module: &str, rule: &str, file: &str, reason: impl std::fmt::Display) -> String {
    format!("[{module}] {rule}: {file}: {reason}")
}

fn reference_path(reference: &FunctionReference) -> Option<&[String]> {
    match reference {
        FunctionReference::Call(path)
        | FunctionReference::Struct(path)
        | FunctionReference::Value(path)
        | FunctionReference::Pattern(path) => Some(path),
        _ => None,
    }
}

/// `reference` with its path resolved through the file's production `use` bindings.
fn resolved_reference(source: &SourceFacts, reference: &FunctionReference) -> FunctionReference {
    match reference {
        FunctionReference::Call(path) => FunctionReference::Call(source.resolved_path(path)),
        FunctionReference::Struct(path) => FunctionReference::Struct(source.resolved_path(path)),
        FunctionReference::Value(path) => FunctionReference::Value(source.resolved_path(path)),
        FunctionReference::Pattern(path) => FunctionReference::Pattern(source.resolved_path(path)),
        other => other.clone(),
    }
}

/// C1: resolves the typed digest/bundle and MAA compilation bridges into Lab.
fn lab_bridge_path(source: &SourceFacts, path: &[String]) -> Option<String> {
    let names = path.iter().map(String::as_str).collect::<Vec<_>>();
    let first = *names.first()?;
    let into_lab = first == LAB_CRATE
        || (source.path.starts_with("crates/lab/src/")
            && matches!(
                first,
                "crate" | "super" | "self" | "Lab" | "compile_maa_task_graph"
            ));
    if !into_lab {
        return None;
    }
    match names.as_slice() {
        [.., "compile_maa_task_graph"] => Some("compile_maa_task_graph".to_string()),
        [.., "Lab", method @ ("package_digest" | "package_bundle")] => {
            Some(format!("Lab::{method}"))
        }
        _ => None,
    }
}

fn lab_bridge_reference(source: &SourceFacts, reference: &FunctionReference) -> Option<String> {
    if let FunctionReference::Method(_, method) = reference {
        return (matches!(method.as_str(), "package_digest" | "package_bundle")
            && source
                .uses
                .iter()
                .any(|binding| binding.path.first().map(String::as_str) == Some(LAB_CRATE)))
        .then(|| format!("Lab::{method}"));
    }
    lab_bridge_path(source, &source.resolved_path(reference_path(reference)?))
}
/// C1: whether a resolved reference of a Lab bridge reaches pack-containment directly: a path
/// into that crate, or a call of one of its source-parsing core entries.
fn reaches_pack_containment(reference: &FunctionReference, core_entries: &[&str]) -> bool {
    reference_path(reference)
        .and_then(|path| path.first())
        .is_some_and(|first| first == PACK_CONTAINMENT_CRATE)
        || core_entries
            .iter()
            .any(|entry| reference_calls(reference, entry))
}

fn bare_name(name: &str) -> &str {
    name.rsplit("::").next().expect("definition name")
}

#[test]
fn c1_lab_bridge_and_reexport_follow_their_owner_and_call_table() {
    let root = workspace_root();
    let mut problems = Vec::new();
    let lab = LAB_BRIDGE_OWNERS
        .iter()
        .find(|owner| owner.module == "lab")
        .expect("LAB_BRIDGE_OWNERS names the Lab bridge file");
    let lab_rows = lab
        .definitions
        .iter()
        .map(|definition| definition.name)
        .collect::<BTreeSet<_>>();
    let core_entries = LAB_BRIDGE_OWNERS
        .iter()
        .filter(|owner| owner.module == "pack-containment")
        .flat_map(|owner| owner.definitions)
        .map(|definition| bare_name(definition.name))
        .collect::<Vec<_>>();
    let owner_entries = LAB_BRIDGE_OWNERS
        .iter()
        .filter(|owner| owner.module != "lab")
        .flat_map(|owner| owner.definitions)
        .map(|definition| bare_name(definition.name))
        .collect::<BTreeSet<_>>();
    assert!(
        !core_entries.is_empty() && !owner_entries.is_empty(),
        "LAB_BRIDGE_OWNERS names no owner entries"
    );

    for owner in LAB_BRIDGE_OWNERS {
        let rule = if owner.module == "lab" {
            "bridge-definition"
        } else {
            "owner-definition"
        };
        let sources = production_facts(&root, &[owner.file]);
        let [facts] = sources.as_slice() else {
            panic!(
                "{} resolves to {} production files",
                owner.file,
                sources.len()
            );
        };
        if facts.functions.is_empty() {
            problems.push(bridge_problem(
                owner.module,
                rule,
                owner.file,
                "the file has no production functions; the table lost its target",
            ));
        }
        for definition in owner.definitions {
            let found = facts
                .functions
                .iter()
                .filter(|function| function.qualified_name() == definition.name)
                .collect::<Vec<_>>();
            let [function] = found.as_slice() else {
                problems.push(bridge_problem(
                    owner.module,
                    rule,
                    owner.file,
                    format!(
                        "must define {} once, found {}",
                        definition.name,
                        found.len()
                    ),
                ));
                continue;
            };
            let Some(entry) = definition.entry else {
                continue;
            };
            let resolved = function
                .references
                .iter()
                .map(|(reference, line)| (resolved_reference(facts, reference), *line))
                .collect::<Vec<_>>();
            if !resolved
                .iter()
                .any(|(reference, _)| reference_matches(reference, entry))
            {
                problems.push(bridge_problem(
                    owner.module,
                    "bridge-entry",
                    owner.file,
                    format!(
                        "{} bypasses its resource-tooling entry {entry}",
                        definition.name
                    ),
                ));
            }
            for (reference, line) in &resolved {
                if reaches_pack_containment(reference, &core_entries) {
                    problems.push(bridge_problem(
                        owner.module,
                        "no-pack-containment",
                        owner.file,
                        format!(
                            "{}:{line} calls pack-containment directly ({reference:?})",
                            definition.name
                        ),
                    ));
                }
            }
        }
        if owner.module != "lab" {
            continue;
        }
        for function in &facts.functions {
            if !lab_rows.contains(function.qualified_name().as_str()) {
                problems.push(bridge_problem(
                    owner.module,
                    rule,
                    owner.file,
                    format!(
                        "{}:{} is defined outside the bridge table",
                        function.qualified_name(),
                        function.line
                    ),
                ));
            }
        }
        if !facts.types.is_empty() || !facts.fields.is_empty() {
            problems.push(bridge_problem(
                owner.module,
                rule,
                owner.file,
                format!(
                    "the forwarding bridge must not own state: types {:?}, fields {:?}",
                    facts.types, facts.fields
                ),
            ));
        }
        for binding in &facts.uses {
            if binding.path.first().map(String::as_str) == Some(PACK_CONTAINMENT_CRATE) {
                problems.push(bridge_problem(
                    owner.module,
                    "no-pack-containment",
                    owner.file,
                    format!("line {} imports {}", binding.line, binding.path.join("::")),
                ));
            }
        }
    }

    for function in production_facts(&root, &["crates/lab/src"])
        .iter()
        .flat_map(|source| &source.functions)
    {
        if owner_entries.contains(function.name.as_str())
            && !(function.path == lab.file && lab_rows.contains(function.qualified_name().as_str()))
        {
            problems.push(bridge_problem(
                "lab",
                "bridge-definition",
                &function.path,
                format!(
                    "{}:{} defines an owner entry outside the bridge table",
                    function.qualified_name(),
                    function.line
                ),
            ));
        }
    }

    let (reexport_file, reexported) = LAB_REEXPORT_MODULE;
    let source = fs::read_to_string(root.join(reexport_file))
        .unwrap_or_else(|error| panic!("read {reexport_file}: {error}"));
    for row in
        inspect_non_reexport_items(reexport_file, &source).unwrap_or_else(|error| panic!("{error}"))
    {
        problems.push(bridge_problem(
            "lab",
            "reexport-only",
            reexport_file,
            format!("{row} is not a `pub use` re-export"),
        ));
    }
    let facts =
        inspect_source_facts(reexport_file, &source).unwrap_or_else(|error| panic!("{error}"));
    if facts.uses.is_empty() {
        problems.push(bridge_problem(
            "lab",
            "reexport-only",
            reexport_file,
            "re-exports nothing; the table lost its target",
        ));
    }
    for binding in &facts.uses {
        if binding.path.first().map(String::as_str) != Some(RESOURCE_TOOLING_CRATE) {
            problems.push(bridge_problem(
                "lab",
                "reexport-only",
                reexport_file,
                format!(
                    "line {} re-exports {} from outside resource-tooling",
                    binding.line,
                    binding.path.join("::")
                ),
            ));
        }
    }
    for name in reexported {
        if !facts.uses.iter().any(|binding| {
            binding.local == *name
                && binding.visibility == DeclaredVisibility::Public
                && binding.path == [RESOURCE_TOOLING_CRATE, *name]
        }) {
            problems.push(bridge_problem(
                "lab",
                "reexport-only",
                reexport_file,
                format!("no longer re-exports {RESOURCE_TOOLING_CRATE}::{name}"),
            ));
        }
    }

    let mut callers = BTreeMap::<String, Vec<String>>::new();
    for source in workspace_production_facts() {
        for binding in &source.uses {
            if binding.local == "*" && binding.path.first().map(String::as_str) == Some(LAB_CRATE) {
                callers
                    .entry(source.path.clone())
                    .or_default()
                    .push(format!(
                        "{}:{} -> a glob import of {LAB_CRATE} hides which bridge names it calls",
                        source.path, binding.line
                    ));
            }
        }
        for function in &source.functions {
            for (reference, line) in &function.references {
                if let Some(symbol) = lab_bridge_reference(source, reference) {
                    callers
                        .entry(source.path.clone())
                        .or_default()
                        .push(format!("{}:{line} -> {symbol}", function.site()));
                }
            }
        }
    }
    for rows in callers.values() {
        for row in rows {
            println!("Lab bridge call: {row}");
        }
    }
    let allowed = row_set(LAB_BRIDGE_CALLERS.iter().copied());
    for (file, rows) in &callers {
        if !allowed.contains(file) {
            for row in rows {
                problems.push(bridge_problem(
                    "lab",
                    "bridge-caller",
                    file,
                    format!("{row} is not an allowed caller of the Lab bridge"),
                ));
            }
        }
    }
    for file in &allowed {
        if !callers.contains_key(file) {
            problems.push(bridge_problem(
                "lab",
                "bridge-caller",
                file,
                "is an allowed caller but calls no Lab bridge; the table lost its target",
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "the Lab bridge chain differs from LAB_BRIDGE_OWNERS / LAB_REEXPORT_MODULE / \
         LAB_BRIDGE_CALLERS ([module] rule: file: reason):\n{}",
        problems.join("\n")
    );
}

fn workspace_relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .expect("workspace source")
        .to_string_lossy()
        .replace('\\', "/")
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// Workflow #310 work package B item 1: the workspace dependency faces of the pure decision
/// crates, checked on the resolve graph of the shared `--all-features` metadata call.
const PURE_DECISION_DEPENDENCY_ALLOW_LISTS: &[(&str, &[&str])] = &[
    ("actingcommand-scheduler", &["actingcommand-contract"]),
    (
        "actingcommand-policy",
        &["actingcommand-contract", "actingcommand-selection-policy"],
    ),
    (
        "actingcommand-selection-policy",
        &["actingcommand-contract"],
    ),
];

#[test]
fn b1_pure_decision_crates_stay_within_their_dependency_allow_lists() {
    let violations = workspace_dependency_allow_list_violations(
        &workspace_metadata(),
        PURE_DECISION_DEPENDENCY_ALLOW_LISTS,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        violations.is_empty(),
        "pure decision crate dependency allow-list violations:\n{}",
        violations.join("\n")
    );
}

/// The modules meant to be pure: every `src/*.rs` of policy and selection-policy (the
/// selection-policy `src/bin` shell is outside), and scheduler's `facts.rs` and `lib.rs`.
fn pure_decision_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for directory in ["crates/policy/src", "crates/selection-policy/src"] {
        let before = files.len();
        let entries = fs::read_dir(root.join(directory))
            .unwrap_or_else(|error| panic!("read {directory}: {error}"));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|error| panic!("read {directory} entry: {error}"))
                .path();
            if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
        assert!(files.len() > before, "{directory} has no Rust sources");
    }
    for file in [
        "crates/scheduler/src/facts.rs",
        "crates/scheduler/src/lib.rs",
    ] {
        let path = root.join(file);
        assert!(path.is_file(), "{file} is missing");
        files.push(path);
    }
    files.sort();
    files
}

#[test]
fn b1_pure_decision_modules_hold_no_side_effect_authority_in_production_items() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for file in pure_decision_sources(&root) {
        let path = workspace_relative(&root, &file);
        let source =
            fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
        violations.extend(
            inspect_pure_decision_source(&path, &source)
                .unwrap_or_else(|error| panic!("scan {path} for side effects: {error}")),
        );
    }
    assert!(
        violations.is_empty(),
        "side-effect authority in production items of pure decision code:\n{}",
        violations.join("\n")
    );
}

/// The resolved-path clippy bans each pure decision crate's `clippy.toml` must carry. Clippy
/// reads the configuration nearest the package manifest, so the ban reaches every target of that
/// package and no other package.
const PURE_DECISION_DISALLOWED_METHODS: &[&str] = &[
    "std::fs::canonicalize",
    "std::fs::copy",
    "std::fs::create_dir",
    "std::fs::create_dir_all",
    "std::fs::exists",
    "std::fs::hard_link",
    "std::fs::metadata",
    "std::fs::read",
    "std::fs::read_dir",
    "std::fs::read_link",
    "std::fs::read_to_string",
    "std::fs::remove_dir",
    "std::fs::remove_dir_all",
    "std::fs::remove_file",
    "std::fs::rename",
    "std::fs::set_permissions",
    "std::fs::symlink_metadata",
    "std::fs::write",
    "std::time::Instant::now",
    "std::time::SystemTime::now",
];
const PURE_DECISION_DISALLOWED_TYPES: &[&str] = &[
    "std::fs::DirBuilder",
    "std::fs::DirEntry",
    "std::fs::File",
    "std::fs::OpenOptions",
    "std::fs::ReadDir",
    "std::net::TcpListener",
    "std::net::TcpStream",
    "std::net::UdpSocket",
    "std::time::SystemTime",
];
/// Named exemptions from those bans: (crate directory, banned path, reason).
const PURE_DECISION_CLIPPY_EXEMPTIONS: &[(&str, &str, &str)] = &[(
    "crates/selection-policy",
    "std::fs::read",
    "src/bin/selection-eval.rs is the crate's declared offline file-reading shell and clippy.toml \
     reaches every target of the package; the library half's std::fs stays banned by the \
     production-item scan",
)];
/// Named `allow` / `expect` escapes of the banning lints: (workspace file, reason). A site may
/// only join this list in the PR that adds it; an entry without a matching site fails too.
const PURE_DECISION_LINT_ESCAPES: &[(&str, &str)] = &[];

fn configured_disallowed_paths(document: &toml::Value, key: &str, file: &str) -> BTreeSet<String> {
    document
        .get(key)
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("{file} has no {key} list"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .or_else(|| entry.get("path").and_then(toml::Value::as_str))
                .unwrap_or_else(|| panic!("{file}: {key} entry without a path"))
                .to_string()
        })
        .collect()
}

/// Manifest lint tables that set a banning lint (or a group containing it) to `allow`.
fn manifest_lint_allowances(document: &toml::Value, table: &[&str], file: &str) -> Vec<String> {
    let mut value = document;
    for key in table {
        match value.get(key) {
            Some(next) => value = next,
            None => return Vec::new(),
        }
    }
    [
        "disallowed_methods",
        "disallowed_types",
        "style",
        "all",
        "warnings",
    ]
    .into_iter()
    .filter(|lint| {
        value.get(*lint).and_then(|level| {
            level
                .as_str()
                .or_else(|| level.get("level").and_then(toml::Value::as_str))
        }) == Some("allow")
    })
    .map(|lint| format!("{file}: {}.{lint} = allow", table.join(".")))
    .collect()
}

#[test]
fn b1_pure_decision_crates_ban_side_effects_in_clippy_and_name_every_escape() {
    let root = workspace_root();
    let mut problems = Vec::new();
    for (crate_directory, _, _) in PURE_DECISION_CLIPPY_EXEMPTIONS {
        assert!(
            [
                "crates/policy",
                "crates/scheduler",
                "crates/selection-policy"
            ]
            .contains(crate_directory),
            "exemption names an unknown crate {crate_directory}"
        );
    }
    for (_, banned, _) in PURE_DECISION_CLIPPY_EXEMPTIONS {
        assert!(
            PURE_DECISION_DISALLOWED_METHODS.contains(banned)
                || PURE_DECISION_DISALLOWED_TYPES.contains(banned),
            "exemption names {banned}, which is not a required ban"
        );
    }
    let workspace_manifest = fs::read_to_string(root.join("Cargo.toml"))
        .unwrap_or_else(|error| panic!("read Cargo.toml: {error}"));
    let workspace_manifest = toml::from_str::<toml::Value>(&workspace_manifest)
        .unwrap_or_else(|error| panic!("parse Cargo.toml: {error}"));
    for table in [
        ["workspace", "lints", "clippy"],
        ["workspace", "lints", "rust"],
    ] {
        problems.extend(manifest_lint_allowances(
            &workspace_manifest,
            &table,
            "Cargo.toml",
        ));
    }
    let mut escapes = Vec::new();
    for crate_directory in [
        "crates/policy",
        "crates/scheduler",
        "crates/selection-policy",
    ] {
        let config_path = format!("{crate_directory}/clippy.toml");
        let config = fs::read_to_string(root.join(&config_path))
            .unwrap_or_else(|error| panic!("read {config_path}: {error}"));
        let config = toml::from_str::<toml::Value>(&config)
            .unwrap_or_else(|error| panic!("parse {config_path}: {error}"));
        for (key, required) in [
            ("disallowed-methods", PURE_DECISION_DISALLOWED_METHODS),
            ("disallowed-types", PURE_DECISION_DISALLOWED_TYPES),
        ] {
            let configured = configured_disallowed_paths(&config, key, &config_path);
            for banned in required {
                let exempt = PURE_DECISION_CLIPPY_EXEMPTIONS
                    .iter()
                    .any(|(directory, path, _)| directory == &crate_directory && path == banned);
                if !exempt && !configured.contains(*banned) {
                    problems.push(format!("{config_path}: {key} lacks {banned}"));
                }
            }
        }
        let manifest_path = format!("{crate_directory}/Cargo.toml");
        let manifest = fs::read_to_string(root.join(&manifest_path))
            .unwrap_or_else(|error| panic!("read {manifest_path}: {error}"));
        let manifest = toml::from_str::<toml::Value>(&manifest)
            .unwrap_or_else(|error| panic!("parse {manifest_path}: {error}"));
        for table in [["lints", "clippy"], ["lints", "rust"]] {
            problems.extend(manifest_lint_allowances(&manifest, &table, &manifest_path));
        }
        let mut files = Vec::new();
        collect_rust_files(&root.join(crate_directory), &mut files);
        files.sort();
        assert!(!files.is_empty(), "{crate_directory} has no Rust sources");
        for file in files {
            let path = workspace_relative(&root, &file);
            let source =
                fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
            escapes.extend(
                inspect_disallowed_lint_escapes(&path, &source)
                    .unwrap_or_else(|error| panic!("scan {path} for lint escapes: {error}")),
            );
        }
    }
    let mut unused = PURE_DECISION_LINT_ESCAPES
        .iter()
        .map(|(path, _)| *path)
        .collect::<BTreeSet<_>>();
    for escape in escapes {
        match PURE_DECISION_LINT_ESCAPES
            .iter()
            .find(|(path, _)| escape.starts_with(&format!("{path}:")))
        {
            Some((path, _)) => {
                unused.remove(path);
            }
            None => problems.push(format!("unnamed lint escape {escape}")),
        }
    }
    problems.extend(
        unused
            .into_iter()
            .map(|path| format!("stale PURE_DECISION_LINT_ESCAPES entry {path}")),
    );
    assert!(
        problems.is_empty(),
        "pure decision clippy bans are incomplete or escaped without a name:\n{}",
        problems.join("\n")
    );
}

/// Workflow #310 work package B item 3: every refusal branch of `RuntimeRequest::validate`,
/// as (classified operations, refusal codes, origin terms). A change to any origin category is
/// a change to this table.
const RUNTIME_REQUEST_ORIGIN_BRANCHES: &[(&[&str], &[&str], &[&str])] = &[
    (
        &["InstallTransition"],
        &["invalid_install_transition_origin"],
        &[
            "EventActor::Cli",
            "EventActor::User",
            "EventSource::Cli",
            "EventSource::Ui",
        ],
    ),
    (
        &[],
        &["unsupported_request_schema"],
        &["self.schema_version"],
    ),
    (
        &[],
        &["invalid_request_timestamp"],
        &["self.submitted_at_unix_ms"],
    ),
    (&[], &["invalid_client_origin"], &["valid_client_origin"]),
    (
        &["RequestShutdown"],
        &["invalid_shutdown_origin"],
        &[
            "EventActor::Cli",
            "EventActor::User",
            "EventSource::Cli",
            "EventSource::Ui",
        ],
    ),
    (
        &["RecordAuthoringEvent"],
        &["invalid_resource_authoring_origin"],
        &["EventActor::Lab", "EventSource::Lab"],
    ),
    (
        &[
            "MatchDiagnosticSignatures",
            "RegisterDiagnosticSignature",
            "RetireDiagnosticSignature",
        ],
        &["invalid_signature_origin"],
        &["EventActor::Lab", "EventSource::Lab"],
    ),
    (
        &[
            "RecognizeArtifact",
            "RecordDebugEvent",
            "ReleaseLabPin",
            "RunContainedLabOperation",
        ],
        &["invalid_runtime_debug_origin"],
        &["EventActor::Lab", "EventSource::Lab"],
    ),
    (
        &["DebugPackage", "ExportEvidence"],
        &["invalid_runtime_debug_origin"],
        &["EventActor::Lab", "EventSource::Lab"],
    ),
    // Governance after slice cfg4: a card is declared by the person or the operator; an approval
    // is the person's only. The host's accepted-connection half is checked by
    // b3_governance_origin_is_person_or_operator_and_approval_needs_an_accepted_connection.
    (
        &["DeclareGovernanceIdentity"],
        &["invalid_governance_origin"],
        &["valid_governance_declaration_origin"],
    ),
    (
        &["RecordApprovalDecision"],
        &["invalid_governance_origin"],
        &["EventActor::User", "EventSource::Ui"],
    ),
    (
        &[
            "ControlEmulatorInstance",
            "DiscoverInstances",
            "SelfCheckInstance",
        ],
        &["invalid_emulator_control_origin"],
        &[
            "EventActor::Cli",
            "EventActor::User",
            "EventSource::Cli",
            "EventSource::Ui",
        ],
    ),
    (
        &["PauseScheduling", "ResumeScheduling"],
        &["invalid_scheduling_pause_origin"],
        &[
            "EventActor::Cli",
            "EventActor::User",
            "EventSource::Cli",
            "EventSource::Ui",
        ],
    ),
    (
        &["PublishFact", "PublishFacts"],
        &["fact_origin_mixed", "invalid_agent_dispatcher_origin"],
        &[
            "EventActor::Agent",
            "EventActor::Cli",
            "EventActor::User",
            "EventSource::Adapter",
            "EventSource::Cli",
            "EventSource::Ui",
        ],
    ),
    (
        &[
            "AgentSessionStatus",
            "ApplyResourceTargets",
            "AssessPredictiveMaintenance",
            "CompileProposal",
            "PrepareStrategicReport",
            "ProjectPolicyForward",
            "ProjectPolicyInputIdentity",
            "PromoteProposal",
            "RecordAgentResponse",
            "ResumeAgentSession",
            "StartAgentSession",
        ],
        &["invalid_agent_dispatcher_origin"],
        &["EventActor::Agent", "EventSource::Adapter"],
    ),
];

/// Operations `RuntimeRequest::validate` admits from every origin `valid_client_origin`
/// accepts, with no narrower category. They are classified here explicitly, so a new operation
/// fails the guard until it joins a refusal branch or this list.
const RUNTIME_OPEN_ORIGIN_OPERATIONS: &[&str] = &[
    "AcquireLease",
    "ApplicationLifecycle",
    "CancelContainedTask",
    "CancelQueuedLease",
    "CaptureSequence",
    "ClearMonitor",
    "ConfigureMonitor",
    "Health",
    "Input",
    "MonitorStatus",
    "ObserveContainedPage",
    "ObserveReadonly",
    "PollQueuedLease",
    "ProjectInterface",
    "QueryEvents",
    "QueueLease",
    "ReadMaterial",
    "RecordClientAction",
    "ReleaseLease",
    "RenewLease",
    "ResourceTargetView",
    "RunContainedTask",
    "RuntimeFactSnapshot",
    "SafeReset",
    "Status",
    "SubscribeEvents",
];

const RUNTIME_CONTRACT_SOURCE: &str = "crates/actingcommand-contract/src/runtime.rs";

fn runtime_request_origin_branches(root: &Path) -> Vec<RefusalBranch> {
    let source = fs::read_to_string(root.join(RUNTIME_CONTRACT_SOURCE))
        .unwrap_or_else(|error| panic!("read {RUNTIME_CONTRACT_SOURCE}: {error}"));
    inspect_refusal_branches(
        RUNTIME_CONTRACT_SOURCE,
        &source,
        "RuntimeRequest",
        "validate",
        "RuntimeOperation",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn b3_every_runtime_operation_has_an_explicit_origin_category() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join(RUNTIME_CONTRACT_SOURCE))
        .unwrap_or_else(|error| panic!("read {RUNTIME_CONTRACT_SOURCE}: {error}"));
    let variants = inspect_enum_variants(RUNTIME_CONTRACT_SOURCE, &source, "RuntimeOperation")
        .unwrap_or_else(|error| panic!("{error}"));
    let branches = runtime_request_origin_branches(&root);

    let actual = branches.iter().cloned().collect::<BTreeSet<_>>();
    let expected = RUNTIME_REQUEST_ORIGIN_BRANCHES
        .iter()
        .map(|(variants, codes, terms)| RefusalBranch {
            variants: owned(variants),
            codes: owned(codes),
            terms: owned(terms),
        })
        .collect::<BTreeSet<_>>();
    let unlisted = actual.difference(&expected).collect::<Vec<_>>();
    let vanished = expected.difference(&actual).collect::<Vec<_>>();
    assert!(
        unlisted.is_empty() && vanished.is_empty(),
        "origin categories of RuntimeRequest::validate differ from RUNTIME_REQUEST_ORIGIN_BRANCHES:\n\
         branches not in the table:\n{unlisted:#?}\ntable entries without a branch:\n{vanished:#?}"
    );

    let classified = branches
        .iter()
        .flat_map(|branch| branch.variants.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let open = RUNTIME_OPEN_ORIGIN_OPERATIONS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let declared = variants.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let doubly = classified.intersection(&open).collect::<Vec<_>>();
    assert!(
        doubly.is_empty(),
        "operations both in a refusal branch and in RUNTIME_OPEN_ORIGIN_OPERATIONS: {doubly:?}"
    );
    let unclassified = declared
        .iter()
        .filter(|variant| !classified.contains(*variant) && !open.contains(*variant))
        .collect::<Vec<_>>();
    assert!(
        unclassified.is_empty(),
        "RuntimeOperation variants without an explicit origin category (name them in a refusal \
         branch of RuntimeRequest::validate or in RUNTIME_OPEN_ORIGIN_OPERATIONS): {unclassified:?}"
    );
    let stale = open.difference(&declared).collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "RUNTIME_OPEN_ORIGIN_OPERATIONS names operations RuntimeOperation no longer declares: {stale:?}"
    );
}

/// Production accesses to the accepted governance connections in crates/runtime-host/src: the
/// set grows only when `declare_governance_identity` accepts an identity card, the approval gate
/// and installation gate read it, and connection cleanup shrinks it.
const GOVERNANCE_CONNECTION_ACCESSES: &[&str] = &[
    "crates/runtime-host/src/host/governance.rs::HostShared::declare_governance_identity -> governance_connections.contains",
    "crates/runtime-host/src/host/governance.rs::HostShared::declare_governance_identity -> governance_connections.insert",
    "crates/runtime-host/src/host/governance.rs::HostShared::record_approval_decision -> governance_connections.contains",
    "crates/runtime-host/src/host/installation.rs::HostShared::install_transition -> governance_connections.contains",
    "crates/runtime-host/src/host/lease.rs::HostShared::cleanup_connection -> governance_connections.remove",
];

/// Production Rust sources of crates/runtime-host/src (test files and `tests` trees excluded).
fn runtime_host_production_sources(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    collect_rust_files(&root.join("crates/runtime-host/src"), &mut files);
    files.sort();
    let sources = files
        .into_iter()
        .map(|file| (workspace_relative(root, &file), file))
        .filter(|(path, _)| {
            !path
                .split('/')
                .any(|component| component == "tests" || component == "tests.rs")
        })
        .map(|(path, file)| {
            let source =
                fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
            (path, source)
        })
        .collect::<Vec<_>>();
    assert!(
        !sources.is_empty(),
        "crates/runtime-host/src has no production Rust sources"
    );
    sources
}

#[test]
fn b3_governance_origin_is_person_or_operator_and_approval_needs_an_accepted_connection() {
    let root = workspace_root();
    // Slice cfg4 in the contract gate: a card is declared by User + Ui or Cli + Cli, and an
    // approval decision is User + Ui only.
    let contract = fs::read_to_string(root.join(RUNTIME_CONTRACT_SOURCE))
        .unwrap_or_else(|error| panic!("read {RUNTIME_CONTRACT_SOURCE}: {error}"));
    assert_eq!(
        inspect_function_origin_terms(
            RUNTIME_CONTRACT_SOURCE,
            &contract,
            "valid_governance_declaration_origin"
        )
        .unwrap_or_else(|error| panic!("{error}")),
        owned(&[
            "EventActor::Cli",
            "EventActor::User",
            "EventSource::Cli",
            "EventSource::Ui",
        ]),
        "a governance identity card is declared by the person or the operator only"
    );
    let branches = runtime_request_origin_branches(&root);
    for (operation, terms) in [
        (
            "DeclareGovernanceIdentity",
            &["valid_governance_declaration_origin"][..],
        ),
        (
            "RecordApprovalDecision",
            &["EventActor::User", "EventSource::Ui"][..],
        ),
    ] {
        let branch = branches
            .iter()
            .find(|branch| branch.variants.iter().any(|variant| variant == operation))
            .unwrap_or_else(|| panic!("{operation} has no governance refusal branch"));
        assert_eq!(
            (&branch.variants, &branch.codes, &branch.terms),
            (
                &owned(&[operation]),
                &owned(&["invalid_governance_origin"]),
                &owned(terms)
            ),
            "{operation} left its own governance origin branch"
        );
    }

    // The host refuses an approval first unless it comes from User + Ui on a connection whose
    // identity card was accepted.
    let governance_path = "crates/runtime-host/src/host/governance.rs";
    let governance = fs::read_to_string(root.join(governance_path))
        .unwrap_or_else(|error| panic!("read {governance_path}: {error}"));
    let approval = inspect_refusal_branches(
        governance_path,
        &governance,
        "HostShared",
        "record_approval_decision",
        "RuntimeOperation",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        approval.first(),
        Some(&RefusalBranch {
            variants: Vec::new(),
            codes: owned(&["governance_authority_required"]),
            terms: owned(&[
                "EventActor::User",
                "EventSource::Ui",
                "self.governance_connections",
            ]),
        }),
        "record_approval_decision must first refuse anything but User + Ui on an accepted connection"
    );
    let accesses = runtime_host_production_sources(&root)
        .iter()
        .flat_map(|(path, source)| {
            inspect_field_accesses(path, source, "governance_connections")
                .unwrap_or_else(|error| panic!("scan {path}: {error}"))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        accesses,
        GOVERNANCE_CONNECTION_ACCESSES
            .iter()
            .map(|row| (*row).to_string())
            .collect::<BTreeSet<_>>(),
        "accepted governance connections changed hands"
    );
}

/// Production sites in crates/runtime-host/src that write instance fact store state (the
/// store's mutating methods and constructor, and every fact payload construction), with
/// whether a `fact_write_gate` guard is held in scope: (row, reason).
const INSTANCE_FACT_WRITE_SITES: &[(&str, &str)] = &[
    (
        "crates/runtime-host/src/host.rs::RuntimeHost::start_with_provider -> InstanceFactStore::recover [no fact_write_gate]",
        "startup replays the ledger into the store before the host is shared",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::publish_facts -> FactPayloadDraft::observation [fact_write_gate held]",
        "the single fact publication entry, for PublishFact and PublishFacts alike",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::synchronize_fact_store_under_gate -> FactPayloadDraft::invalidated [no fact_write_gate]",
        "event-driven invalidation during ledger replay; its callers hold the gate",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::synchronize_fact_store_under_gate -> acknowledge_generated_invalidation [no fact_write_gate]",
        "event-driven invalidation during ledger replay; its callers hold the gate",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::synchronize_fact_store_under_gate -> synchronize [no fact_write_gate]",
        "ledger replay into the shared store; its callers hold the gate",
    ),
    (
        "crates/runtime-host/src/host/policy_dispatch.rs::HostShared::project_policy_forward -> synchronize [no fact_write_gate]",
        "replays a private clone for a forward projection; the shared store is not written",
    ),
];

#[test]
fn b3_fact_publication_reaches_the_store_through_one_gated_entry() {
    let root = workspace_root();
    let store_path = "crates/runtime-host/src/fact_store.rs";
    let store = fs::read_to_string(root.join(store_path))
        .unwrap_or_else(|error| panic!("read {store_path}: {error}"));
    let api = inspect_store_write_api(store_path, &store, "InstanceFactStore")
        .unwrap_or_else(|error| panic!("{error}"));
    let rows = runtime_host_production_sources(&root)
        .iter()
        .flat_map(|(path, source)| {
            inspect_store_writes(
                path,
                source,
                "InstanceFactStore",
                &api,
                "FactPayloadDraft",
                "fact_write_gate",
            )
            .unwrap_or_else(|error| panic!("scan {path}: {error}"))
        })
        .collect::<BTreeSet<_>>();
    let expected = INSTANCE_FACT_WRITE_SITES
        .iter()
        .map(|(row, _)| (*row).to_string())
        .collect::<BTreeSet<_>>();
    let unlisted = rows.difference(&expected).collect::<Vec<_>>();
    let vanished = expected.difference(&rows).collect::<Vec<_>>();
    assert!(
        unlisted.is_empty() && vanished.is_empty(),
        "instance fact store write sites differ from INSTANCE_FACT_WRITE_SITES:\n\
         sites not in the table:\n{}\ntable entries without a site:\n{}",
        unlisted
            .iter()
            .map(|row| row.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        vanished
            .iter()
            .map(|row| row.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );

    let requests_path = "crates/runtime-host/src/host/requests.rs";
    let requests = fs::read_to_string(root.join(requests_path))
        .unwrap_or_else(|error| panic!("read {requests_path}: {error}"));
    assert_eq!(
        inspect_dispatch_arm_calls(
            requests_path,
            &requests,
            "HostShared",
            "process_validated",
            "RuntimeOperation",
            &["PublishFact", "PublishFacts"],
        )
        .unwrap_or_else(|error| panic!("{error}")),
        vec![
            ("PublishFact".to_string(), owned(&["publish_facts"])),
            ("PublishFacts".to_string(), owned(&["publish_facts"])),
        ],
        "PublishFact and PublishFacts must both dispatch only to the single publish_facts entry"
    );
}

#[test]
fn bplus_vision_provider_check_takes_provider_symbols_from_vision_ffi() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rust_files(&root.join("apps/vision-provider-check"), &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "apps/vision-provider-check has no Rust sources"
    );
    let mut violations = Vec::new();
    for file in files {
        let path = workspace_relative(&root, &file);
        let source =
            fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
        violations.extend(
            inspect_provider_symbol_literals(&path, &source)
                .unwrap_or_else(|error| panic!("scan {path}: {error}")),
        );
    }
    assert!(
        violations.is_empty(),
        "apps/vision-provider-check spells provider ABI symbols; take them from actingcommand-vision-ffi:\n{}",
        violations.join("\n")
    );
}

// Workflow #310 work package B, second half (slice gB2, frozen model: the #310 coordinator
// comment "gB2 冻模型"): the capacity admission structure (B2), the planning document decoding
// responsibility (B4), the database / forensic / vendor stdio ownership (B5) and the typed host
// split (B6).

/// Production Rust sources under `directories` (a directory or one file), workspace-relative
/// and sorted; test files and `tests` trees are left out. A directory without production
/// sources is an error.
fn production_sources(root: &Path, directories: &[&str]) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    for directory in directories {
        let base = root.join(directory);
        let mut files = Vec::new();
        if base.is_file() {
            files.push(base);
        } else {
            collect_rust_files(&base, &mut files);
        }
        files.sort();
        let before = sources.len();
        for file in files {
            let path = workspace_relative(root, &file);
            if path
                .split('/')
                .any(|component| component == "tests" || component == "tests.rs")
            {
                continue;
            }
            let source =
                fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
            sources.push((path, source));
        }
        assert!(
            sources.len() > before,
            "{directory} has no production Rust sources"
        );
    }
    sources
}

fn production_facts(root: &Path, directories: &[&str]) -> Vec<SourceFacts> {
    production_sources(root, directories)
        .iter()
        .map(|(path, source)| {
            inspect_source_facts(path, source).unwrap_or_else(|error| panic!("{error}"))
        })
        .collect()
}

/// The production facts of every workspace source tree outside the architecture tool, read
/// once per test binary.
fn workspace_production_facts() -> &'static [SourceFacts] {
    static FACTS: OnceLock<Vec<SourceFacts>> = OnceLock::new();
    FACTS.get_or_init(|| production_facts(&workspace_root(), &["apps", "crates", "providers"]))
}

/// The rows that differ from a named table, as an explicit report; `None` when they agree.
fn table_difference(actual: &BTreeSet<String>, expected: &BTreeSet<String>) -> Option<String> {
    let unlisted = actual.difference(expected).cloned().collect::<Vec<_>>();
    let vanished = expected.difference(actual).cloned().collect::<Vec<_>>();
    (!unlisted.is_empty() || !vanished.is_empty()).then(|| {
        format!(
            "rows not in the table:\n{}\ntable rows without a site:\n{}",
            unlisted.join("\n"),
            vanished.join("\n")
        )
    })
}

fn row_set<'a>(rows: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    rows.into_iter().map(str::to_string).collect()
}

/// B2 (a): the single capacity predicate: `admit_bytes`, reached directly or through
/// `ArtifactStore::admit_new_bytes`.
const CAPACITY_PREDICATES: &[&str] = &["admit_bytes", "admit_new_bytes"];

/// B2 (a): the artifact-store write entries: the `ArtifactStore` / `ArtifactStream` writes, the
/// evidence export and the `FrameStore` overflow segment, whose spilled frames publish through
/// `CapturePipeline::publish_candidate`.
const ARTIFACT_WRITE_ENTRIES: &[&str] = &[
    "ArtifactStore::begin_stream",
    "ArtifactStore::commit_prepared",
    "ArtifactStore::put",
    "ArtifactStream::append",
    "CapturePipeline::publish_candidate",
    "EvidenceExporter::export",
];

/// B2 (a): every byte sink and write entry of crates/artifact-store with the admission that
/// covers it: (row, reason). A sink outside admission is listed only with the reason it writes
/// no new artifact bytes.
const ARTIFACT_BYTE_WRITES: &[(&str, &str)] = &[
    (
        "crates/artifact-store/src/exporter.rs::CapacityArchiveWriter::write -> Write::write [admitted in place]",
        "evidence archive bytes, admitted chunk by chunk through the exporter's store",
    ),
    (
        "crates/artifact-store/src/exporter.rs::EvidenceExporter::export [reaches admit_new_bytes via EvidenceExporter::export -> EvidenceExporter::export_inner]",
        "the evidence export entry",
    ),
    (
        "crates/artifact-store/src/exporter.rs::create_export_temp -> OpenOptions(write+create_new).open [admitted by EvidenceExporter::export_inner]",
        "the export temporary, created after its zero-byte admission",
    ),
    (
        "crates/artifact-store/src/pipeline.rs::CapturePipeline::publish_candidate [reaches admit_bytes via CapturePipeline::publish_candidate -> ArtifactStore::put -> ArtifactStore::commit_prepared]",
        "the FrameStore overflow segment: spilled frames publish as ordinary artifacts",
    ),
    (
        "crates/artifact-store/src/store.rs::ArtifactStore::begin_stream -> OpenOptions(write+create_new).open [admitted in place]",
        "the stream staging file, created after its zero-byte admission",
    ),
    (
        "crates/artifact-store/src/store.rs::ArtifactStore::begin_stream [reaches admit_bytes via ArtifactStore::begin_stream]",
        "the streaming write entry",
    ),
    (
        "crates/artifact-store/src/store.rs::ArtifactStore::commit_prepared [reaches admit_bytes via ArtifactStore::commit_prepared]",
        "the prepared-artifact write entry",
    ),
    (
        "crates/artifact-store/src/store.rs::ArtifactStore::put [reaches admit_bytes via ArtifactStore::put -> ArtifactStore::commit_prepared]",
        "the one-shot write entry",
    ),
    (
        "crates/artifact-store/src/store.rs::ArtifactStore::restore_recovery_reference -> OpenOptions(write+create_new).open [NOT admitted]",
        "restores the exact bytes of an already persisted Ledger reference under the offline maintenance owner, not a new artifact; its capacity exception is #287's open offline-restore item",
    ),
    (
        "crates/artifact-store/src/store.rs::ArtifactStream::append [reaches admit_bytes via ArtifactStream::append -> ArtifactStream::write]",
        "the streaming append entry",
    ),
    (
        "crates/artifact-store/src/store.rs::ArtifactStream::write -> Write::write [admitted in place]",
        "streamed artifact bytes, admitted chunk by chunk",
    ),
    (
        "crates/artifact-store/src/store.rs::write_synced_with -> OpenOptions(write+create_new).open [admitted by ArtifactStore::commit_prepared]",
        "the one-shot artifact temporary",
    ),
    (
        "crates/artifact-store/src/usage.rs::open_lock -> OpenOptions(write+create).open [NOT admitted]",
        "a zero-byte OS lock file for use and delete coordination; it holds no artifact bytes",
    ),
];

/// B2 (a): the only functions that ask the capacity owner for a decision directly: the
/// predicate itself, and stream publication, which records a zero-byte decision for bytes the
/// stream's writes already admitted.
const CAPACITY_DECISION_CALLERS: &[&str] = &[
    "crates/artifact-store/src/store.rs::ArtifactStore::seal_stream -> decide",
    "crates/artifact-store/src/store.rs::admit_bytes -> decide",
];

#[test]
fn b2_new_artifact_bytes_pass_the_single_capacity_predicate() {
    let root = workspace_root();
    let sources = production_facts(&root, &["crates/artifact-store/src"]);
    let predicate = sources
        .iter()
        .flat_map(|source| &source.functions)
        .filter(|function| function.qualified_name() == "admit_bytes")
        .collect::<Vec<_>>();
    assert_eq!(
        predicate.len(),
        1,
        "crates/artifact-store must define exactly one production admit_bytes predicate"
    );
    let new_bytes = sources
        .iter()
        .flat_map(|source| &source.functions)
        .filter(|function| function.qualified_name() == "ArtifactStore::admit_new_bytes")
        .collect::<Vec<_>>();
    assert!(
        matches!(new_bytes.as_slice(), [function] if function.calls("admit_bytes")),
        "ArtifactStore::admit_new_bytes must exist once and delegate to admit_bytes"
    );
    let rows = inspect_artifact_byte_writes(&sources, CAPACITY_PREDICATES, ARTIFACT_WRITE_ENTRIES)
        .unwrap_or_else(|error| panic!("{error}"));
    if let Some(difference) = table_difference(
        &rows.into_iter().collect(),
        &row_set(ARTIFACT_BYTE_WRITES.iter().map(|(row, _)| *row)),
    ) {
        panic!(
            "artifact byte writes and their capacity admission differ from ARTIFACT_BYTE_WRITES \
             (a sink outside admit_bytes / admit_new_bytes may only be listed with the reason it \
             writes no new artifact bytes):\n{difference}"
        );
    }
    let decisions = inspect_call_sites(&sources, &["decide"]);
    if let Some(difference) = table_difference(
        &decisions.into_iter().collect(),
        &row_set(CAPACITY_DECISION_CALLERS.iter().copied()),
    ) {
        panic!(
            "direct capacity decisions outside the single predicate differ from \
             CAPACITY_DECISION_CALLERS:\n{difference}"
        );
    }
}

/// B2 (b): the capacity admission handles as (file, scope, expression): the evidence archive
/// writer's admission, the store's installed owner, a stream's retained owner, the owner an
/// evidence store inherits, the predicate's parameter, and the host's lease / business capacity
/// owner.
const CAPACITY_ADMISSION_HANDLES: &[(&str, &str, &str)] = &[
    (
        "crates/artifact-store/src/exporter.rs",
        "*",
        "self.admission",
    ),
    (
        "crates/artifact-store/src/exporter.rs",
        "write_archive",
        "file.admission",
    ),
    ("crates/artifact-store/src/store.rs", "*", "self.capacity"),
    ("crates/artifact-store/src/store.rs", "*", "stream.capacity"),
    ("crates/artifact-store/src/store.rs", "*", "source.capacity"),
    (
        "crates/artifact-store/src/store.rs",
        "admit_bytes",
        "admission",
    ),
    (
        "crates/runtime-host/src/performance/capacity.rs",
        "PerformanceMonitor::admit_capacity",
        "self.capacity",
    ),
];

/// B2 (b): every production use of those handles. A use that decides an absent handle must
/// refuse with the distinct code `capacity_owner_missing`; the others pass the handle on.
const CAPACITY_ADMISSION_HANDLE_USES: &[&str] = &[
    "crates/artifact-store/src/exporter.rs::CapacityArchiveWriter::io_error -> self.admission: projection .2",
    "crates/artifact-store/src/exporter.rs::CapacityArchiveWriter::write -> self.admission: bound to a pattern",
    "crates/artifact-store/src/exporter.rs::write_archive -> file.admission: projection .2",
    "crates/artifact-store/src/store.rs::ArtifactStore::admit_new_bytes -> self.capacity: argument of admit_bytes",
    "crates/artifact-store/src/store.rs::ArtifactStore::begin_stream -> self.capacity: bound to capacity",
    "crates/artifact-store/src/store.rs::ArtifactStore::commit_prepared -> self.capacity: argument of admit_bytes",
    "crates/artifact-store/src/store.rs::ArtifactStore::inherit_capacity -> source.capacity: ok_or Err(capacity_owner_missing)",
    "crates/artifact-store/src/store.rs::ArtifactStore::install_capacity_admission -> self.capacity: receiver of .set",
    "crates/artifact-store/src/store.rs::ArtifactStore::seal_stream -> stream.capacity: let-else Err(capacity_owner_missing)",
    "crates/artifact-store/src/store.rs::ArtifactStream::write -> self.capacity: argument of admit_bytes",
    "crates/artifact-store/src/store.rs::admit_bytes -> admission: let-else Err(capacity_owner_missing)",
    "crates/artifact-store/src/store.rs::admit_bytes -> admission: receiver of .decide",
    "crates/runtime-host/src/performance/capacity.rs::PerformanceMonitor::admit_capacity -> self.capacity: ok_or Err(capacity_owner_missing)",
];

#[test]
fn b2_an_absent_admission_handle_refuses_with_a_distinct_code() {
    let root = workspace_root();
    let mut files = CAPACITY_ADMISSION_HANDLES
        .iter()
        .map(|(file, _, _)| *file)
        .collect::<Vec<_>>();
    files.dedup();
    let mut rows = BTreeSet::new();
    for file in files {
        let source = fs::read_to_string(root.join(file))
            .unwrap_or_else(|error| panic!("read {file}: {error}"));
        let handles = CAPACITY_ADMISSION_HANDLES
            .iter()
            .filter(|(handle_file, _, _)| *handle_file == file)
            .map(|(_, scope, expression)| (*scope, *expression))
            .collect::<Vec<_>>();
        rows.extend(
            inspect_admission_handle_uses(file, &source, &handles)
                .unwrap_or_else(|error| panic!("{error}")),
        );
    }
    let deciding = rows
        .iter()
        .filter(|row| {
            ["let-else", "if-let", "match ", "ok_or", "None decided"]
                .iter()
                .any(|use_label| row.contains(&format!(": {use_label}")))
        })
        .filter(|row| {
            !row.ends_with(": let-else Err(capacity_owner_missing)")
                && !row.ends_with(": ok_or Err(capacity_owner_missing)")
                && !row.ends_with(": if-let else Err(capacity_owner_missing)")
                && !row.ends_with(": match None Err(capacity_owner_missing)")
        })
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        deciding.is_empty(),
        "an absent capacity admission handle is admitted or decided without the distinct \
         capacity_owner_missing refusal:\n{}",
        deciding.join("\n")
    );
    if let Some(difference) = table_difference(
        &rows,
        &row_set(CAPACITY_ADMISSION_HANDLE_USES.iter().copied()),
    ) {
        panic!(
            "capacity admission handle uses differ from CAPACITY_ADMISSION_HANDLE_USES:\n{difference}"
        );
    }
}

/// B2 (c): the production callers of the lease / business capacity admission in
/// crates/runtime-host/src (`admit_capacity`, `require_business_capacity`).
const BUSINESS_CAPACITY_ADMISSION_CALLERS: &[&str] = &[
    "crates/runtime-host/src/host/contained_task.rs::HostShared::run_contained_task -> require_business_capacity",
    "crates/runtime-host/src/host/contained_task.rs::HostShared::run_scheduled_contained_task -> require_business_capacity",
    "crates/runtime-host/src/host/contained_task.rs::HostShared::run_startup_package -> require_business_capacity",
    "crates/runtime-host/src/host/lease.rs::HostShared::grant_prepared_lease_with_links -> require_business_capacity",
    "crates/runtime-host/src/host/monitor_control.rs::HostShared::run_monitor_probe -> admit_capacity",
    "crates/runtime-host/src/host/observation.rs::HostShared::capture_observation_with_links -> require_business_capacity",
    "crates/runtime-host/src/host/performance.rs::HostShared::admit_capacity -> admit_capacity",
    "crates/runtime-host/src/host/performance.rs::HostShared::capacity_allows_transfer -> admit_capacity",
    "crates/runtime-host/src/host/performance.rs::HostShared::require_business_capacity -> admit_capacity",
    "crates/runtime-host/src/host/policy_dispatch.rs::HostShared::admit_policy_dispatch -> admit_capacity",
    "crates/runtime-host/src/host/recovery_ladder.rs::HostShared::recovery_admitted -> require_business_capacity",
    "crates/runtime-host/src/performance/capacity.rs::PerformanceMonitor::preflight_capacity -> admit_capacity",
];

#[test]
fn b2_business_capacity_admission_has_a_named_caller_set() {
    let root = workspace_root();
    let sources = production_facts(&root, &["crates/runtime-host/src"]);
    let rows = inspect_call_sites(&sources, &["admit_capacity", "require_business_capacity"]);
    if let Some(difference) = table_difference(
        &rows.into_iter().collect(),
        &row_set(BUSINESS_CAPACITY_ADMISSION_CALLERS.iter().copied()),
    ) {
        panic!(
            "callers of the lease / business capacity admission differ from \
             BUSINESS_CAPACITY_ADMISSION_CALLERS:\n{difference}"
        );
    }
}

/// B4: who handles a planning document envelope (a `RuntimePlanningDocument`, or a policy
/// catalog generation record of the five scheduling documents), by responsibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvelopeResponsibility {
    /// Builds an envelope (schema version, content hash and kind) through the owner's encoder.
    Construct,
    /// Opens an envelope only through its owner's single decoding entry.
    Decode,
    /// Hands raw catalog document bytes to the policy compiler; never builds or opens an envelope.
    Source,
}

/// B4: the envelope calls the site scan reports: the contract envelope's encoder and decoding
/// entry (a method call counts inside a function whose signature names the envelope), the host
/// catalog store's generation record builder and verified loader, and raw catalog sources.
const PLANNING_ENVELOPE_API: &[&str] = &[
    "RuntimePlanningDocument::encode",
    "RuntimePlanningDocument::decode",
    "RuntimePlanningDocument::validate",
    "RuntimePlanningDocument::validate_kind",
    ".decode@RuntimePlanningDocument",
    ".validate@RuntimePlanningDocument",
    ".validate_kind@RuntimePlanningDocument",
    "source_record",
    ".load_source",
    "CatalogDocumentSource::new",
];

/// B4: every production envelope site outside the contract (the envelope's owner), with its
/// responsibility. The fifth catalog document, `selection`, is recorded and loaded like the
/// other four.
const PLANNING_ENVELOPE_SITES: &[(&str, EnvelopeResponsibility)] = &[
    (
        "apps/actingd/src/config.rs::read_catalog_document -> CatalogDocumentSource::new(_, _)",
        EnvelopeResponsibility::Source,
    ),
    (
        "apps/actinglab/src/resource_declarations.rs::DeclarationReader::validate -> CatalogDocumentSource::new(_, _)",
        EnvelopeResponsibility::Source,
    ),
    (
        "crates/lab/src/scheduling.rs::read_source -> CatalogDocumentSource::new(_, _)",
        EnvelopeResponsibility::Source,
    ),
    (
        "crates/policy/src/resource_targets.rs::parse_resource_targets -> CatalogDocumentSource::new(_, _)",
        EnvelopeResponsibility::Source,
    ),
    (
        "crates/runtime-client/src/client.rs::PredictiveMaintenanceRequest::new -> encode_policy_document(RuntimePlanningDocumentKind::MaintenanceTrendPolicy, _, \"build_predictive_maintenance_request\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::assess_predictive_maintenance -> .decode_policy_document(_, RuntimePlanningDocumentKind::MaintenanceAssessmentV2, \"assess_predictive_maintenance\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::decode_policy_document -> .decode(_)",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::prepare_strategic_report -> .decode_policy_document(_, RuntimePlanningDocumentKind::StrategicProjection, \"prepare_strategic_report\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::prepare_strategic_report -> encode_policy_document(RuntimePlanningDocumentKind::StrategicReport, _, \"prepare_strategic_report\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::project_policy_forward -> .decode_policy_document(_, RuntimePlanningDocumentKind::ForwardProjection, \"project_policy_forward\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::project_policy_forward -> encode_policy_document(RuntimePlanningDocumentKind::EvaluationFacts, _, \"project_policy_forward\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::project_policy_forward -> encode_policy_document(RuntimePlanningDocumentKind::EvaluationResources, _, \"project_policy_forward\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::project_policy_forward -> encode_policy_document(RuntimePlanningDocumentKind::EvaluationTime, _, \"project_policy_forward\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-client/src/client.rs::RuntimeClient::project_policy_forward -> encode_policy_document(RuntimePlanningDocumentKind::ForwardProjectionConfig, _, \"project_policy_forward\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-client/src/client.rs::encode_policy_document -> RuntimePlanningDocument::encode(_, _)",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::assess_predictive_maintenance_ipc -> decode_planning_document(_, RuntimePlanningDocumentKind::MaintenanceTrendPolicy, \"assess_predictive_maintenance\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::assess_predictive_maintenance_ipc -> encode_planning_document(RuntimePlanningDocumentKind::MaintenanceAssessmentV2, _, \"assess_predictive_maintenance\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::prepare_strategic_report_ipc -> decode_planning_document(_, RuntimePlanningDocumentKind::StrategicReport, \"prepare_strategic_report\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::prepare_strategic_report_with -> encode_planning_document(RuntimePlanningDocumentKind::StrategicProjection, _, \"prepare_strategic_report\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::project_policy_forward_ipc -> decode_planning_document(_, RuntimePlanningDocumentKind::EvaluationFacts, \"project_policy_forward\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::project_policy_forward_ipc -> decode_planning_document(_, RuntimePlanningDocumentKind::EvaluationResources, \"project_policy_forward\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::project_policy_forward_ipc -> decode_planning_document(_, RuntimePlanningDocumentKind::EvaluationTime, \"project_policy_forward\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::project_policy_forward_ipc -> decode_planning_document(_, RuntimePlanningDocumentKind::ForwardProjectionConfig, \"project_policy_forward\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::project_policy_forward_ipc -> encode_planning_document(RuntimePlanningDocumentKind::ForwardProjection, _, \"project_policy_forward\")",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::decode_planning_document -> .decode(_)",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/host/planning.rs::encode_planning_document -> RuntimePlanningDocument::encode(_, _)",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::CatalogStore::load_source -> CatalogDocumentSource::new(_, _)",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::CatalogStore::read_generation -> .load_source(_, _, \"activity\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::CatalogStore::read_generation -> .load_source(_, _, \"pools\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::CatalogStore::read_generation -> .load_source(_, _, \"selection\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::CatalogStore::read_generation -> .load_source(_, _, \"tasks\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::CatalogStore::read_generation -> .load_source(_, _, \"timeline\")",
        EnvelopeResponsibility::Decode,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::generation_from -> source_record(\"activity\", _)",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::generation_from -> source_record(\"pools\", _)",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::generation_from -> source_record(\"selection\", _)",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::generation_from -> source_record(\"tasks\", _)",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/policy_host.rs::generation_from -> source_record(\"timeline\", _)",
        EnvelopeResponsibility::Construct,
    ),
    (
        "crates/runtime-host/src/proposal.rs::encode_document -> CatalogDocumentSource::new(_, _)",
        EnvelopeResponsibility::Source,
    ),
];

/// B4: each envelope's single decoding entry and the validation it must make, as (file,
/// function, required references): the contract envelope checks schema version, content hash
/// and kind before it decodes; the catalog store checks the generation's schema version and
/// hash and each source record's kind and hash before a document reaches the compiler.
const PLANNING_ENVELOPE_ENTRIES: &[(&str, &str, &[&str])] = &[
    (
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimePlanningDocument::validate",
        &[
            "self.schema_version",
            "RUNTIME_PLANNING_DOCUMENT_SCHEMA_VERSION",
            "\"unsupported_planning_document_schema\"",
            "self.sha256",
            "Sha256::digest",
            "\"planning_document_hash_mismatch\"",
        ],
    ),
    (
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimePlanningDocument::validate_kind",
        &[
            "self.validate",
            "self.kind",
            "\"planning_document_kind_mismatch\"",
        ],
    ),
    (
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimePlanningDocument::decode",
        &[
            "self.validate_kind",
            "serde_json::from_value",
            "self.document",
        ],
    ),
    (
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimePlanningDocument::encode",
        &[
            "RUNTIME_PLANNING_DOCUMENT_SCHEMA_VERSION",
            "Sha256::digest",
            "envelope.validate",
        ],
    ),
    (
        "crates/runtime-host/src/policy_host.rs",
        "CatalogStore::read_generation",
        &[
            "CATALOG_STATE_SCHEMA",
            "\"catalog_generation_identity_mismatch\"",
            "self.load_source",
            "compile_catalog",
            "\"catalog_generation_content_mismatch\"",
        ],
    ),
    (
        "crates/runtime-host/src/policy_host.rs",
        "CatalogStore::load_source",
        &[
            "\"catalog_source_missing\"",
            "sha256",
            "\"catalog_source_hash_mismatch\"",
            "CatalogDocumentSource::new",
        ],
    ),
];

#[test]
fn b4_planning_envelopes_are_validated_by_their_single_decoding_entry() {
    let facts = workspace_production_facts();
    let mut problems = Vec::new();
    for (file, function, required) in PLANNING_ENVELOPE_ENTRIES {
        let source = facts
            .iter()
            .find(|source| source.path == *file)
            .unwrap_or_else(|| panic!("{file} is not a workspace production source"));
        let entries = source
            .functions
            .iter()
            .filter(|candidate| candidate.qualified_name() == *function)
            .collect::<Vec<_>>();
        let [entry] = entries.as_slice() else {
            problems.push(format!(
                "{file}: expected one production {function}, found {}",
                entries.len()
            ));
            continue;
        };
        for reference in *required {
            if !entry.references_target(reference) {
                problems.push(format!(
                    "{file}::{function} no longer references {reference}"
                ));
            }
        }
    }
    let contract = "crates/actingcommand-contract/src/runtime.rs";
    let contract_facts = facts
        .iter()
        .find(|source| source.path == contract)
        .unwrap_or_else(|| panic!("{contract} is not a workspace production source"));
    let decoders = contract_facts
        .functions
        .iter()
        .filter(|function| {
            function.references_target("self.document")
                && [
                    "serde_json::from_value",
                    "serde_json::from_str",
                    "serde_json::from_slice",
                ]
                .iter()
                .any(|decoder| function.calls(decoder))
        })
        .map(|function| function.qualified_name())
        .collect::<Vec<_>>();
    if decoders != ["RuntimePlanningDocument::decode"] {
        problems.push(format!(
            "the envelope document must be decoded by RuntimePlanningDocument::decode alone, found {decoders:?}"
        ));
    }
    let envelope_fields = contract_facts
        .fields
        .iter()
        .filter(|(owner, _, _)| owner == "RuntimePlanningDocument")
        .collect::<Vec<_>>();
    if envelope_fields.is_empty()
        || envelope_fields
            .iter()
            .any(|(_, _, visibility)| *visibility != DeclaredVisibility::Private)
    {
        problems.push(format!(
            "RuntimePlanningDocument fields must exist and stay private to the contract: {envelope_fields:?}"
        ));
    }
    assert!(
        problems.is_empty(),
        "planning document envelope validation lost its single decoding entry:\n{}",
        problems.join("\n")
    );
}

#[test]
fn b4_planning_envelope_sites_match_the_responsibility_table() {
    assert!(
        PLANNING_ENVELOPE_SITES.iter().all(|(row, responsibility)| {
            *responsibility != EnvelopeResponsibility::Construct
                || !(row.starts_with("crates/lab/") || row.starts_with("apps/actinglab/"))
        }),
        "Lab may only decode or hand raw sources to the compiler; it never builds an envelope"
    );
    let root = workspace_root();
    let mut rows = BTreeSet::new();
    for (path, source) in production_sources(&root, &["apps", "crates", "providers"]) {
        if path.starts_with("crates/actingcommand-contract/") {
            continue;
        }
        rows.extend(
            inspect_envelope_sites(
                &path,
                &source,
                PLANNING_ENVELOPE_API,
                &["RuntimePlanningDocumentKind"],
            )
            .unwrap_or_else(|error| panic!("{error}")),
        );
    }
    if let Some(difference) = table_difference(
        &rows,
        &row_set(PLANNING_ENVELOPE_SITES.iter().map(|(row, _)| *row)),
    ) {
        panic!(
            "planning document envelope sites differ from PLANNING_ENVELOPE_SITES (who may \
             construct an envelope and who may only decode):\n{difference}"
        );
    }
}

/// B5 (a): the production constructors of the Runtime database, which owns the SQLite file
/// and its integrity key, and of its backup material: (row, owner and reason). The frozen
/// model places construction in the ledger crate; main also constructs it in runtime-state and
/// runtime-host, recorded here by owner, and any other site fails.
const RUNTIME_DATABASE_CONSTRUCTORS: &[(&str, &str)] = &[
    (
        "crates/ledger/src/global/evidence.rs::GlobalLedger::open_evidence -> RuntimeDatabase",
        "ledger: forensic evidence opens the existing database read-only",
    ),
    (
        "crates/ledger/src/global/evidence.rs::GlobalLedger::open_metadata -> RuntimeDatabase",
        "ledger: forensic metadata opens the existing database read-only",
    ),
    (
        "crates/runtime-host/src/host.rs::RuntimeHost::start_with_provider -> RuntimeDatabase",
        "runtime-host: startup opens existing storage (fresh storage goes through runtime-state)",
    ),
    (
        "crates/runtime-host/src/ledger_maintenance.rs::restore -> RuntimeDatabase",
        "runtime-host: offline restore verifies the restored database",
    ),
    (
        "crates/runtime-host/src/ledger_maintenance.rs::restore -> restore_backup",
        "runtime-host: offline restore writes the backup material into the target root",
    ),
    (
        "crates/runtime-host/src/ledger_maintenance.rs::run_locked -> RuntimeDatabase",
        "runtime-host: offline backup opens the source database",
    ),
    (
        "crates/runtime-host/src/ledger_maintenance.rs::run_locked -> backup",
        "runtime-host: offline backup writes the backup material",
    ),
    (
        "crates/runtime-host/src/ledger_maintenance.rs::verify_backup_binding -> RuntimeDatabase",
        "runtime-host: offline backup verifies the archived database read-only",
    ),
    (
        "crates/runtime-host/src/owner_unlock.rs::record -> RuntimeDatabase",
        "runtime-host: the owner unlock record opens the existing database",
    ),
    (
        "crates/runtime-state/src/store.rs::RuntimeStateStore::open_database -> RuntimeDatabase",
        "runtime-state: fresh storage opens with the state-owned schema",
    ),
];

#[test]
fn b5_runtime_database_file_and_key_have_a_named_constructor_table() {
    let facts = workspace_production_facts();
    let mut rows = inspect_type_constructions(facts, &["RuntimeDatabase"])
        .into_iter()
        .filter(|row| !row.starts_with("crates/runtime-database/"))
        .collect::<BTreeSet<_>>();
    rows.extend(
        inspect_call_sites(facts, &["backup", "restore_backup"])
            .into_iter()
            .filter(|row| !row.starts_with("crates/runtime-database/")),
    );
    if let Some(difference) = table_difference(
        &rows,
        &row_set(RUNTIME_DATABASE_CONSTRUCTORS.iter().map(|(row, _)| *row)),
    ) {
        panic!(
            "Runtime database (file and integrity key) constructors differ from \
             RUNTIME_DATABASE_CONSTRUCTORS:\n{difference}"
        );
    }
}

/// B5 (b): the forensic read chain: the ledger's read-only snapshot module and the forensic
/// leaf crate.
const FORENSIC_READ_CHAIN: &[&str] = &[
    "crates/ledger/src/global/read_only.rs",
    "crates/ledger-forensics/src",
];

/// B5 (b): the write entries the forensic read chain never calls: Ledger, Runtime database and
/// artifact writers. File-system mutations are refused separately.
const FORENSIC_FORBIDDEN_WRITE_ENTRIES: &[&str] = &[
    "GlobalLedger::open",
    "GlobalLedger::open_with_artifact_verifier",
    "GlobalLedger::open_sqlite_candidate",
    "GlobalLedger::open_sqlite_candidate_with_artifact_verifier",
    "append",
    "append_transaction",
    "append_with_observation",
    "append_deferred",
    "append_event",
    "confirm_deferred",
    "LedgerMaintenance::acquire",
    "initialize_empty",
    "open_writer",
    "import",
    "LabLedger::create",
    "LabLedger::open_or_create",
    "LabLedger::create_runtime_shard",
    "commit_then_record",
    "enforce_retention",
    "RuntimeDatabase::open",
    "RuntimeDatabase::open_with_initializer",
    "backup",
    "restore_backup",
    "borrow_transaction",
    "put",
    "commit_prepared",
    "begin_stream",
    "seal_stream",
    "restore_recovery_reference",
    "install_capacity_admission",
    "EvidenceExporter::open",
    "EvidenceExporter::open_with_admission",
    "try_artifact_delete_guard",
    "remove_after_durable_intent",
];

/// B5 (b): the read entries through which the chain opens the ledger.
const FORENSIC_READ_ENTRIES: &[&str] =
    &["GlobalLedger::open_evidence", "GlobalLedger::open_metadata"];

#[test]
fn b5_forensic_read_chain_reaches_no_write_entry() {
    let root = workspace_root();
    let chain = production_facts(&root, FORENSIC_READ_CHAIN);
    for file in FORENSIC_READ_CHAIN {
        assert!(
            chain
                .iter()
                .any(|source| source.path.starts_with(file) && !source.functions.is_empty()),
            "forensic read chain part {file} has no production functions"
        );
    }
    let mut violations = inspect_call_sites(&chain, FORENSIC_FORBIDDEN_WRITE_ENTRIES);
    for function in chain.iter().flat_map(|source| &source.functions) {
        for (write, line) in &function.file_writes {
            violations.push(format!("{}:{line} -> {write}", function.site()));
        }
    }
    assert!(
        violations.is_empty(),
        "the forensic read chain reaches a write entry:\n{}",
        violations.join("\n")
    );
    for entry in FORENSIC_READ_ENTRIES {
        assert!(
            !inspect_call_sites(&chain, &[entry]).is_empty(),
            "the forensic read chain no longer opens the ledger through {entry}"
        );
    }
}

/// B5 (c): the vendor stdio diagnostic types, in their device and ledger contract forms.
const VENDOR_STDIO_TYPES: &[&str] = &[
    "DeviceStdioObservation",
    "ExecutionStdioObservation",
    "StdioApi",
    "StdioFact",
    "StdioFileIdentity",
    "StdioNativeError",
    "StdioPathFact",
    "StdioPathRemoval",
    "StdioPhase",
    "StdioReference",
    "StdioReferenceFact",
    "StdioRmApi",
    "StdioRmAvailability",
    "StdioRmCall",
    "StdioRmFacts",
    "StdioRmProcess",
    "StdioStep",
    "StdioTargetRetirement",
    "StdioUnknown",
    "VendorStdioCapture",
    "VendorStdioFacts",
];

/// B5 (c): where vendor stdio diagnostics may be constructed: (workspace path prefix, reason).
/// The host and every other crate only consume them.
const VENDOR_STDIO_CONSTRUCTION_OWNERS: &[(&str, &str)] = &[
    (
        "crates/device/src/",
        "the device domain: the vendor stdio session and its native facts",
    ),
    (
        "crates/execution-kernel/src/error.rs",
        "the device backend shell: a device close observation joins the execution error",
    ),
    (
        "crates/execution-kernel/src/error/vendor_stdio.rs",
        "the device backend shell: device facts convert one-to-one into the contract form",
    ),
    (
        "crates/execution-kernel/src/session.rs",
        "the device backend shell: device close observations join the execution outcome",
    ),
];

#[test]
fn b5_vendor_stdio_diagnostics_are_constructed_in_the_device_domain_only() {
    let rows = inspect_type_constructions(workspace_production_facts(), VENDOR_STDIO_TYPES);
    assert!(
        !rows.is_empty(),
        "no vendor stdio diagnostic construction found; the scan lost its target"
    );
    let outside = rows
        .iter()
        .filter(|row| {
            !VENDOR_STDIO_CONSTRUCTION_OWNERS
                .iter()
                .any(|(prefix, _)| row.starts_with(prefix))
        })
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        outside.is_empty(),
        "vendor stdio diagnostics constructed outside the device domain (the host only consumes \
         them):\n{}",
        outside.join("\n")
    );
    let stale = VENDOR_STDIO_CONSTRUCTION_OWNERS
        .iter()
        .filter(|(prefix, _)| !rows.iter().any(|row| row.starts_with(prefix)))
        .map(|(prefix, _)| *prefix)
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "VENDOR_STDIO_CONSTRUCTION_OWNERS entries without a construction: {stale:?}"
    );
}

/// B6: one definition a host-split module owns, as the existing ownership checks named it.
struct HostDefinition {
    /// `Owner::name`, or `name` for a free function.
    name: &'static str,
    visibility: DeclaredVisibility,
    /// The crates/runtime-host/src files whose production code calls it.
    callers: &'static [&'static str],
    /// References its body must make (see `reference_matches`): its delegation path.
    delegates: &'static [&'static str],
}

/// B6: one `crates/runtime-host/src/host/*.rs` module of the #161 host split.
struct HostModule {
    /// The file stem, declared `mod <module>;` in host.rs.
    module: &'static str,
    /// The types outside the module that its `impl` blocks extend.
    owners: &'static [&'static str],
    definitions: &'static [HostDefinition],
    /// References some production function of the module must make.
    delegates: &'static [&'static str],
}

const fn host_module(module: &'static str, owners: &'static [&'static str]) -> HostModule {
    HostModule {
        module,
        owners,
        definitions: &[],
        delegates: &[],
    }
}

/// B6: every module of the host split with its owner types; the ownership, visibility, caller
/// and delegation facts the former text checks asserted sit on the modules they concern.
const HOST_SPLIT: &[HostModule] = &[
    host_module("agent_control", &["HostShared"]),
    host_module("backend_open", &["HostShared"]),
    host_module("client_events", &["HostShared"]),
    HostModule {
        module: "contained_task",
        owners: &["HostShared"],
        definitions: &[],
        delegates: &[
            "TaskSemanticFact::RecognitionStarted",
            "TaskSemanticFact::EffectIntent",
            "TaskSemanticFact::TerminalCommitted",
            "TaskSemanticFact::TerminalRejected",
        ],
    },
    host_module("device_diagnostic", &["HostShared"]),
    host_module("emulator_instance", &["HostShared"]),
    host_module("evidence_export", &["HostShared"]),
    host_module("facts", &["HostShared"]),
    host_module("failure_settlement", &["HostShared"]),
    host_module("foreground_gate", &["HostShared"]),
    host_module("frame_retention", &["HostShared"]),
    host_module("governance", &["HostShared"]),
    host_module("input", &["HostShared"]),
    host_module("instance_discovery", &["HostShared"]),
    host_module("installation", &["HostShared"]),
    host_module("lab_operation", &["HostShared"]),
    host_module("lease", &["HostShared"]),
    host_module("lifecycle", &["HostShared", "RuntimeLifecycleFailureStage"]),
    host_module("material_read", &["HostShared"]),
    HostModule {
        module: "monitor_control",
        owners: &["HostShared"],
        definitions: &[
            HostDefinition {
                name: "monitor_probe_loop",
                visibility: DeclaredVisibility::Super,
                callers: &["host.rs"],
                delegates: &[],
            },
            HostDefinition {
                name: "HostShared::run_monitor_probe",
                visibility: DeclaredVisibility::Private,
                callers: &["host/monitor_control.rs"],
                delegates: &[
                    "self.artifacts",
                    "CapturePipeline::open_with_frame_store",
                    "pipeline.persist_frame",
                ],
            },
            HostDefinition {
                name: "HostShared::record_monitor_recovery_coordination",
                visibility: DeclaredVisibility::Private,
                callers: &["host/monitor_control.rs"],
                delegates: &[],
            },
            HostDefinition {
                name: "HostShared::monitor_recovery_admission",
                visibility: DeclaredVisibility::Super,
                callers: &["host/emulator_instance.rs", "host/monitor_control.rs"],
                delegates: &[],
            },
        ],
        delegates: &[
            "MonitorPayloadDraft::completed",
            "MonitorPayloadDraft::recovery_admitted",
            "MonitorPayloadDraft::recovery_deferred",
        ],
    },
    host_module("nemu_input", &["HostShared"]),
    HostModule {
        module: "observation",
        owners: &["HostShared"],
        definitions: &[
            HostDefinition {
                name: "HostShared::capture_sequence",
                visibility: DeclaredVisibility::Super,
                callers: &["host/requests.rs"],
                delegates: &[
                    "capture_readonly_observation",
                    "thread::sleep",
                    "CaptureSequence::new",
                ],
            },
            HostDefinition {
                name: "HostShared::capture_readonly_observation",
                visibility: DeclaredVisibility::Super,
                callers: &["host/observation.rs", "host/online_observation.rs"],
                delegates: &[],
            },
        ],
        delegates: &[],
    },
    host_module("online_observation", &["HostShared"]),
    host_module("package_debug", &["HostShared"]),
    host_module("performance", &["HostShared"]),
    host_module("planning", &["HostShared"]),
    host_module("policy_catalog", &["HostShared"]),
    host_module("policy_dispatch", &["HostShared"]),
    host_module("policy_outcome", &["HostShared"]),
    host_module("ppocr_diagnostic", &["HostShared", "RuntimeContainedTask"]),
    host_module("prerequisite", &["HostShared"]),
    HostModule {
        module: "read_events",
        owners: &["HostShared"],
        definitions: &[HostDefinition {
            name: "HostShared::control_plane_status",
            visibility: DeclaredVisibility::Super,
            callers: &["host/requests.rs"],
            delegates: &[],
        }],
        delegates: &[],
    },
    host_module("recovery_ladder", &["HostShared"]),
    host_module(
        "requests",
        &["HostShared", "OperationSuccess", "RuntimeRunLinks"],
    ),
    host_module("resource_close", &["HostShared"]),
    host_module("resource_targets", &["HostShared"]),
    host_module("runtime_facts", &["HostShared"]),
    host_module("saved_artifact_ocr", &["HostShared"]),
    host_module("signatures", &["HostShared"]),
    host_module("startup_package", &["HostShared"]),
    host_module("state_control", &["HostShared"]),
    host_module("task_diagnostic", &["RuntimeContainedTask"]),
    host_module("task_timing", &[]),
];

#[test]
fn b6_host_split_modules_match_their_typed_owner_and_delegation_table() {
    let root = workspace_root();
    let host_root = "crates/runtime-host/src";
    let host_file = format!("{host_root}/host.rs");
    let host = inspect_source_facts(
        &host_file,
        &fs::read_to_string(root.join(&host_file))
            .unwrap_or_else(|error| panic!("read {host_file}: {error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mut problems = Vec::new();
    for (module, visibility) in &host.modules {
        if *visibility != DeclaredVisibility::Private {
            problems.push(format!(
                "host.rs declares module {module} as {visibility:?}, not private"
            ));
        }
    }
    let declared = host
        .modules
        .iter()
        .map(|(module, _)| module.clone())
        .collect::<BTreeSet<_>>();
    let directory = root.join(host_root).join("host");
    let files = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("read host module entry: {error}"))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect::<BTreeSet<_>>();
    assert!(
        !files.is_empty() && !declared.is_empty(),
        "the host split scan found no host/*.rs modules ({} files, {} declarations)",
        files.len(),
        declared.len()
    );
    let tabled = HOST_SPLIT
        .iter()
        .map(|module| module.module.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        tabled.len(),
        HOST_SPLIT.len(),
        "HOST_SPLIT names a module twice"
    );
    for missing in tabled.difference(&files) {
        problems.push(format!(
            "host module {missing} has no file {host_root}/host/{missing}.rs"
        ));
    }
    for missing in declared.difference(&files) {
        problems.push(format!("host.rs declares module {missing} without a file"));
    }
    for unlisted in files.difference(&tabled) {
        problems.push(format!(
            "{host_root}/host/{unlisted}.rs is not in HOST_SPLIT"
        ));
    }
    for undeclared in files.difference(&declared) {
        problems.push(format!(
            "{host_root}/host/{undeclared}.rs is not declared in host.rs"
        ));
    }
    let crate_facts = production_facts(&root, &[host_root]);
    for module in HOST_SPLIT {
        let path = format!("{host_root}/host/{}.rs", module.module);
        let Some(facts) = crate_facts.iter().find(|source| source.path == path) else {
            problems.push(format!(
                "{path} is missing from the runtime-host production sources"
            ));
            continue;
        };
        if facts.functions.is_empty() {
            problems.push(format!("{path} has no production functions"));
        }
        let owners = module
            .owners
            .iter()
            .map(|owner| (*owner).to_string())
            .collect::<BTreeSet<_>>();
        if facts.extended_types() != owners {
            problems.push(format!(
                "{path} extends {:?}, HOST_SPLIT names {owners:?}",
                facts.extended_types()
            ));
        }
        for definition in module.definitions {
            let found = facts
                .functions
                .iter()
                .filter(|function| function.qualified_name() == definition.name)
                .collect::<Vec<_>>();
            let [function] = found.as_slice() else {
                problems.push(format!(
                    "{path} must define {} once, found {}",
                    definition.name,
                    found.len()
                ));
                continue;
            };
            if function.visibility != definition.visibility {
                problems.push(format!(
                    "{path}::{} is {:?}, HOST_SPLIT names {:?}",
                    definition.name, function.visibility, definition.visibility
                ));
            }
            let callee = definition
                .name
                .rsplit("::")
                .next()
                .expect("definition name");
            let callers = crate_facts
                .iter()
                .filter(|source| source.functions.iter().any(|caller| caller.calls(callee)))
                .map(|source| {
                    source
                        .path
                        .strip_prefix(&format!("{host_root}/"))
                        .expect("runtime-host source")
                        .to_string()
                })
                .collect::<BTreeSet<_>>();
            let expected = definition
                .callers
                .iter()
                .map(|caller| (*caller).to_string())
                .collect::<BTreeSet<_>>();
            if callers != expected {
                problems.push(format!(
                    "{path}::{} is called from {callers:?}, HOST_SPLIT allows {expected:?}",
                    definition.name
                ));
            }
            for delegate in definition.delegates {
                if !function.references_target(delegate) {
                    problems.push(format!(
                        "{path}::{} no longer delegates to {delegate}",
                        definition.name
                    ));
                }
            }
        }
        for delegate in module.delegates {
            if !facts
                .functions
                .iter()
                .any(|function| function.references_target(delegate))
            {
                problems.push(format!("{path} no longer delegates to {delegate}"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the host split differs from HOST_SPLIT (owner types, allowed callers and delegation \
         paths per host/*.rs module):\n{}",
        problems.join("\n")
    );
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
    assert!(environment.contains("finish_semantic_result_with_ledger"));
    assert!(environment.contains("semantic_ledger_context"));
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
    // The Runtime semantic owner (host/contained_task.rs constructing the RecognitionStarted,
    // EffectIntent, TerminalCommitted and TerminalRejected task facts) is a HOST_SPLIT entry.
    assert!(
        !runtime_contract.contains("TaskSemanticFact"),
        "clients must not be able to submit Runtime task semantic facts"
    );
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
    for required in ["package_path: String", "expected_sha256: crate::PackageRef"] {
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
    let host = [
        fs::read_to_string(root.join("crates/runtime-host/src/host.rs"))
            .expect("read Runtime host"),
        fs::read_to_string(root.join("crates/runtime-host/src/host/observation.rs"))
            .expect("read Runtime observation adapter"),
        fs::read_to_string(root.join("crates/runtime-host/src/host/contained_task.rs"))
            .expect("read Runtime contained-task adapter"),
        fs::read_to_string(root.join("crates/runtime-host/src/host/input.rs"))
            .expect("read Runtime input adapter"),
    ]
    .join("\n");
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
    static METADATA: OnceLock<Result<String, String>> = OnceLock::new();
    METADATA
        .get_or_init(dependency_metadata)
        .as_ref()
        .unwrap_or_else(|error| panic!("{error}"))
        .clone()
}

fn dependency_metadata() -> Result<String, String> {
    let root = workspace_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(cargo_metadata_args())
        .current_dir(&root)
        .output()
        .map_err(|error| format!("run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let metadata = String::from_utf8(output.stdout)
        .map_err(|error| format!("cargo metadata must emit UTF-8 JSON: {error}"))?;
    serde_json::from_str::<serde_json::Value>(&metadata)
        .map_err(|error| format!("parse cargo metadata: {error}"))?;
    Ok(metadata)
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

// Workflow #310 work package D, first half (slice D-1, frozen model: the #310 coordinator comment
// "D 冻模型"): the ActingLab glue modules main.rs delegates to. Each owner module is one typed
// declaration that the library checker `inspect_glue_declarations` reads against every Rust file
// of apps/actinglab/src; the entries replace the 30 `actinglab_*_glue_stays_out_of_main` tests,
// named above each entry. Exact call expressions and import lines, the precedence / error-text /
// parameter-mapping invariants and the behaviour test names stay verbatim. Per #325 the owner
// body line / byte / sha256 freezes and the caller-serialization sha256 values are gone; their
// purpose is carried by the declared owner, visibility, forbidden file and caller set.

const ACTINGLAB_SRC: &str = "apps/actinglab/src";
const ACTINGLAB_MAIN: &str = "apps/actinglab/src/main.rs";
const MAIN_ONLY: &[&str] = &[ACTINGLAB_MAIN];
const CLI_INFORMATION: &str = "apps/actinglab/src/cli_information.rs";
const FLAG_VALUES: &str = "apps/actinglab/src/flag_values.rs";
const SESSION_RECORD: &str = "apps/actinglab/src/commands/session_record.rs";
const DEVICE_COMMANDS: &str = "apps/actinglab/src/commands/device_commands.rs";

const fn glue_owner(
    name: &'static str,
    kind: SymbolKind,
    visibility: DeclaredVisibility,
) -> OwnerSymbol {
    OwnerSymbol {
        name,
        kind,
        visibility,
        test_only: false,
    }
}

/// A `pub(super) fn` owner.
const fn glue_fn(name: &'static str) -> OwnerSymbol {
    glue_owner(name, SymbolKind::Fn, DeclaredVisibility::Super)
}

/// A private `fn` owner.
const fn glue_private_fn(name: &'static str) -> OwnerSymbol {
    glue_owner(name, SymbolKind::Fn, DeclaredVisibility::Private)
}

const fn glue_field(name: &'static str, visibility: DeclaredVisibility) -> OwnerSymbol {
    glue_owner(name, SymbolKind::Field, visibility)
}

/// An owner defined under a `cfg(test)` scope.
const fn glue_test_owner(
    name: &'static str,
    kind: SymbolKind,
    visibility: DeclaredVisibility,
) -> OwnerSymbol {
    OwnerSymbol {
        name,
        kind,
        visibility,
        test_only: true,
    }
}

/// `text` appears in `file` exactly `count` times.
const fn exact(file: &'static str, text: &'static str, count: usize) -> GlueText {
    GlueText {
        file,
        text,
        count: Some(count),
    }
}

/// `text` appears in `file`.
const fn present(file: &'static str, text: &'static str) -> GlueText {
    GlueText {
        file,
        text,
        count: None,
    }
}

/// `text` never appears in `file`.
const fn absent(file: &'static str, text: &'static str) -> GlueText {
    GlueText {
        file,
        text,
        count: Some(0),
    }
}

const fn glue_invariant(symbol: &'static str, text: &'static str) -> GlueInvariant {
    GlueInvariant { symbol, text }
}

/// A caller file with its pinned number of references.
const fn glue_caller(file: &'static str, count: usize) -> GlueCaller {
    GlueCaller {
        file,
        count: Some(count),
    }
}

/// A caller file without a pinned count.
const fn glue_caller_file(file: &'static str) -> GlueCaller {
    GlueCaller { file, count: None }
}

const GLUE_DECLARATIONS: &[GlueDeclaration] = &[
    // actinglab_runtime_endpoint_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/runtime_endpoint.rs",
        rule: "runtime_endpoint_glue_stays_out_of_main",
        owner_symbols: &[
            glue_owner(
                "RuntimeEndpointPolicy",
                SymbolKind::Struct,
                DeclaredVisibility::Super,
            ),
            glue_field("RuntimeEndpointPolicy::scheme", DeclaredVisibility::Super),
            glue_field("RuntimeEndpointPolicy::host", DeclaredVisibility::Super),
            glue_field("RuntimeEndpointPolicy::port", DeclaredVisibility::Super),
            glue_field("RuntimeEndpointPolicy::channel", DeclaredVisibility::Super),
            glue_field(
                "RuntimeEndpointPolicy::auth_material",
                DeclaredVisibility::Super,
            ),
            glue_owner(
                "RuntimeEndpointChannel",
                SymbolKind::Enum,
                DeclaredVisibility::Super,
            ),
            glue_private_fn("RuntimeEndpointChannel::as_str"),
            glue_fn("runtime_endpoint_check"),
            glue_fn("runtime_endpoint_policy"),
            glue_fn("runtime_endpoint_policy_json"),
            glue_private_fn("trusted_remote_auth_material"),
            glue_fn("env_var_non_empty"),
            glue_fn("runtime_tcp_available"),
            glue_private_fn("parse_endpoint_host_port"),
            glue_private_fn("parse_endpoint_parts"),
            glue_private_fn("is_loopback_host"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller_file(CLI_INFORMATION),
            glue_caller_file("apps/actinglab/src/commands/session_transport.rs"),
            glue_caller_file("apps/actinglab/src/resource_runtime_support.rs"),
            glue_caller_file("apps/actinglab/src/runtime_endpoint.rs"),
        ],
        required_calls: &[],
        invariants: &[],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "the Runtime endpoint policy (channel, trusted-remote auth material, TCP probe) \
                 is owned by runtime_endpoint.rs; main.rs only declares the private module",
    },
    // actinglab_cli_result_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/cli_result.rs",
        rule: "cli_result_glue_stays_out_of_main",
        owner_symbols: &[
            glue_fn("human_summary"),
            glue_private_fn("with_input_outcome"),
            glue_owner("CliResult", SymbolKind::Struct, DeclaredVisibility::Super),
            glue_field("CliResult::print_json", DeclaredVisibility::Super),
            glue_field("CliResult::envelope", DeclaredVisibility::Super),
            glue_fn("CliResult::ok"),
            glue_fn("CliResult::err"),
            glue_fn("CliResult::exit_code"),
            glue_fn("CliResult::envelope_json"),
            glue_fn("CliResult::human_text"),
            glue_owner(
                "CliErrorExitCode",
                SymbolKind::Trait,
                DeclaredVisibility::Super,
            ),
            glue_private_fn("CliError::exit_code"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[glue_caller_file(ACTINGLAB_MAIN)],
        required_calls: &[
            exact(ACTINGLAB_MAIN, "use cli_result::human_summary;", 1),
            exact(
                ACTINGLAB_MAIN,
                "let human = human_summary(&invocation.command_name, &data);",
                1,
            ),
            present(
                "apps/actinglab/src/cli_result.rs",
                "impl CliErrorExitCode for CliError",
            ),
            absent(ACTINGLAB_MAIN, "Value::String(text) => text.clone(),"),
            absent(
                ACTINGLAB_MAIN,
                r#"_ => with_input_outcome(format!("{command} ok"), data),"#,
            ),
            absent(ACTINGLAB_MAIN, "ErrorKind::UsageValidation => 2,"),
            absent(ACTINGLAB_MAIN, "ErrorKind::SafetyBlocked => 3,"),
            absent(ACTINGLAB_MAIN, "ErrorKind::DeviceInstance => 4,"),
            absent(ACTINGLAB_MAIN, "ErrorKind::RuntimeUnavailable => 5,"),
            absent(ACTINGLAB_MAIN, "ErrorKind::NotImplemented => 6,"),
        ],
        invariants: &[
            glue_invariant(
                "human_summary",
                "fn human_summary(command: &str, data: &Value) -> String",
            ),
            glue_invariant("human_summary", "Value::String(text) => text.clone(),"),
            glue_invariant(
                "human_summary",
                r#"_ => with_input_outcome(format!("{command} ok"), data),"#,
            ),
            glue_invariant(
                "with_input_outcome",
                "fn with_input_outcome(summary: String, data: &Value) -> String",
            ),
            glue_invariant(
                "CliResult::ok",
                "fn ok(command: String, data: Value, print_json: bool, human: String) -> Self",
            ),
            glue_invariant(
                "CliResult::err",
                "fn err(command: String, err: CliError, print_json: bool) -> Self",
            ),
            glue_invariant("CliResult::exit_code", "fn exit_code(&self) -> i32"),
            glue_invariant(
                "CliResult::envelope_json",
                "fn envelope_json(&self) -> String",
            ),
            glue_invariant("CliResult::human_text", "fn human_text(&self) -> String"),
            glue_invariant("CliError::exit_code", "ErrorKind::UsageValidation => 2,"),
            glue_invariant("CliError::exit_code", "ErrorKind::SafetyBlocked => 3,"),
            glue_invariant("CliError::exit_code", "ErrorKind::DeviceInstance => 4,"),
            glue_invariant("CliError::exit_code", "ErrorKind::RuntimeUnavailable => 5,"),
            glue_invariant("CliError::exit_code", "ErrorKind::NotImplemented => 6,"),
        ],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "the CLI result envelope, human summary and exit-code mapping are owned by \
                 cli_result.rs; main.rs imports human_summary privately and calls it once",
    },
    // actinglab_flag_args_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/flag_args.rs",
        rule: "flag_args_glue_stays_out_of_main",
        owner_symbols: &[
            glue_owner("FlagArgs", SymbolKind::Struct, DeclaredVisibility::Super),
            glue_field("FlagArgs::flags", DeclaredVisibility::Super),
            glue_field("FlagArgs::positionals", DeclaredVisibility::Super),
            glue_fn("FlagArgs::parse"),
            glue_fn("FlagArgs::parse_values"),
            glue_fn("FlagArgs::bool"),
            glue_fn("FlagArgs::optional"),
            glue_fn("FlagArgs::values"),
            glue_fn("FlagArgs::without_first_positional"),
            glue_fn("FlagArgs::required"),
            glue_fn("FlagArgs::optional_path"),
            glue_fn("FlagArgs::required_path"),
            glue_fn("FlagArgs::reject_flags"),
            glue_fn("FlagArgs::expect_positionals"),
            glue_fn("FlagArgs::required_positional"),
            glue_fn("FlagArgs::required_i32"),
            glue_fn("FlagArgs::required_u64"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller_file(ACTINGLAB_MAIN),
            glue_caller_file(CLI_INFORMATION),
            glue_caller_file(DEVICE_COMMANDS),
            glue_caller_file("apps/actinglab/src/commands/navigation_recovery.rs"),
            glue_caller_file("apps/actinglab/src/commands/session_contracts.rs"),
            glue_caller_file(SESSION_RECORD),
            glue_caller_file("apps/actinglab/src/commands/session_transport.rs"),
            glue_caller_file("apps/actinglab/src/drive_cli.rs"),
            glue_caller_file("apps/actinglab/src/env_detection.rs"),
            glue_caller_file("apps/actinglab/src/lab2_cli.rs"),
            glue_caller_file("apps/actinglab/src/lab_package_control.rs"),
            glue_caller_file("apps/actinglab/src/lab_run.rs"),
            glue_caller_file("apps/actinglab/src/readonly_cli.rs"),
            glue_caller_file("apps/actinglab/src/resource_restore.rs"),
            glue_caller_file("apps/actinglab/src/resource_runtime_support.rs"),
            glue_caller_file("apps/actinglab/src/run_summary.rs"),
            glue_caller_file("apps/actinglab/src/runtime_debug.rs"),
            glue_caller_file("apps/actinglab/src/runtime_session_adapter.rs"),
            glue_caller_file("apps/actinglab/src/runtime_slice_cli.rs"),
            glue_caller_file("apps/actinglab/src/runtime_stream_adapter.rs"),
            glue_caller_file("apps/actinglab/src/scheduling_cli.rs"),
            glue_caller_file("apps/actinglab/src/session_management.rs"),
            glue_caller_file("apps/actinglab/src/signature_cli.rs"),
        ],
        required_calls: &[
            absent(ACTINGLAB_MAIN, "flags: BTreeMap<String, Vec<String>>"),
            absent(ACTINGLAB_MAIN, "positionals: Vec<String>"),
        ],
        invariants: &[
            glue_invariant("FlagArgs", "flags: BTreeMap<String, Vec<String>>"),
            glue_invariant("FlagArgs", "positionals: Vec<String>"),
            glue_invariant(
                "FlagArgs::parse",
                "fn parse(args: &[String]) -> CliOutcome<Self>",
            ),
            glue_invariant("FlagArgs::bool", "fn bool(&self, name: &str) -> bool"),
            glue_invariant(
                "FlagArgs::optional",
                "fn optional(&self, name: &str) -> Option<String>",
            ),
            glue_invariant(
                "FlagArgs::values",
                "fn values(&self, name: &str) -> Vec<String>",
            ),
            glue_invariant(
                "FlagArgs::without_first_positional",
                "fn without_first_positional(&self) -> Self",
            ),
            glue_invariant(
                "FlagArgs::required",
                "fn required(&self, name: &str) -> CliOutcome<String>",
            ),
            glue_invariant(
                "FlagArgs::optional_path",
                "fn optional_path(&self, name: &str) -> Option<PathBuf>",
            ),
            glue_invariant(
                "FlagArgs::required_path",
                "fn required_path(&self, name: &str) -> CliOutcome<PathBuf>",
            ),
            glue_invariant(
                "FlagArgs::reject_flags",
                "fn reject_flags(&self, command: &str) -> CliOutcome<()>",
            ),
            glue_invariant(
                "FlagArgs::expect_positionals",
                "fn expect_positionals(&self, command: &str, expected: usize) -> CliOutcome<()>",
            ),
            glue_invariant(
                "FlagArgs::required_positional",
                "fn required_positional(&self, index: usize, name: &str) -> CliOutcome<&str>",
            ),
            glue_invariant(
                "FlagArgs::required_i32",
                "fn required_i32(&self, index: usize, name: &str) -> CliOutcome<i32>",
            ),
            glue_invariant(
                "FlagArgs::required_u64",
                "fn required_u64(&self, index: usize, name: &str) -> CliOutcome<u64>",
            ),
        ],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "flag and positional parsing is owned by flag_args.rs; main.rs only declares \
                 the private module",
    },
    // actinglab_device_runtime_config_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/device_runtime_config.rs",
        rule: "device_runtime_config_glue_stays_out_of_main",
        owner_symbols: &[
            glue_fn("device_config"),
            glue_private_fn("device_config_for_instance"),
            glue_owner(
                "DeviceRuntimeConfig",
                SymbolKind::Struct,
                DeclaredVisibility::Super,
            ),
            glue_field(
                "DeviceRuntimeConfig::instance_alias",
                DeclaredVisibility::Private,
            ),
            glue_field(
                "DeviceRuntimeConfig::runtime_state_root",
                DeclaredVisibility::Private,
            ),
            glue_test_owner(
                "DeviceRuntimeConfig::target",
                SymbolKind::Field,
                DeclaredVisibility::Super,
            ),
            glue_field("DeviceRuntimeConfig::adb_source", DeclaredVisibility::Super),
            glue_field(
                "DeviceRuntimeConfig::adb_warning",
                DeclaredVisibility::Super,
            ),
            glue_field(
                "DeviceRuntimeConfig::capture_backend",
                DeclaredVisibility::Super,
            ),
            glue_test_owner(
                "DeviceRuntimeConfig::touch_backend",
                SymbolKind::Field,
                DeclaredVisibility::Super,
            ),
            glue_fn("DeviceRuntimeConfig::runtime_capture_endpoint"),
            glue_fn("effective_capture_backend_choice"),
            glue_test_owner(
                "effective_touch_backend_choice",
                SymbolKind::Fn,
                DeclaredVisibility::Private,
            ),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller_file(DEVICE_COMMANDS),
            glue_caller_file("apps/actinglab/src/commands/navigation_recovery.rs"),
            glue_caller_file(SESSION_RECORD),
            glue_caller_file("apps/actinglab/src/device_runtime_config.rs"),
        ],
        required_calls: &[],
        invariants: &[
            glue_invariant("DeviceRuntimeConfig", "instance_alias: String"),
            glue_invariant("DeviceRuntimeConfig", "runtime_state_root: PathBuf"),
            glue_invariant("DeviceRuntimeConfig", "target: DeviceTarget"),
            glue_invariant("DeviceRuntimeConfig", "adb_source: AdbPathSource"),
            glue_invariant("DeviceRuntimeConfig", "adb_warning: Option<String>"),
            glue_invariant(
                "DeviceRuntimeConfig",
                "capture_backend: CaptureBackendChoice",
            ),
            glue_invariant("DeviceRuntimeConfig", "touch_backend: TouchBackendChoice"),
            glue_invariant(
                "device_config_for_instance",
                "enforce_path_adb_target_boundary(&resolved_adb, instance, capture_backend)?;",
            ),
        ],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "the per-instance device Runtime configuration and backend choice are owned by \
                 device_runtime_config.rs; main.rs only declares the private module",
    },
    // actinglab_instance_resolution_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/instance_resolution.rs",
        rule: "instance_resolution_glue_stays_out_of_main",
        owner_symbols: &[
            glue_fn("resolve_instance_id"),
            glue_fn("resolve_instance_id_for_flags"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller_file(DEVICE_COMMANDS),
            glue_caller_file(SESSION_RECORD),
            glue_caller_file("apps/actinglab/src/device_runtime_config.rs"),
            glue_caller_file("apps/actinglab/src/drive_cli.rs"),
            glue_caller_file("apps/actinglab/src/instance_resolution.rs"),
            glue_caller_file("apps/actinglab/src/lab_run.rs"),
            glue_caller_file("apps/actinglab/src/readonly_cli.rs"),
            glue_caller_file("apps/actinglab/src/runtime_stream_adapter.rs"),
            glue_caller_file("apps/actinglab/src/session_management.rs"),
        ],
        required_calls: &[],
        invariants: &[
            glue_invariant("resolve_instance_id", "game_match && server_match"),
            glue_invariant(
                "resolve_instance_id_for_flags",
                "flags.optional(\"--instance\").filter(|value| value != \"true\")",
            ),
        ],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "instance id resolution is owned by instance_resolution.rs, declared once as a \
                 private out-of-line module",
    },
    // actinglab_user_config_store_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/user_config_store.rs",
        rule: "user_config_store_glue_stays_out_of_main",
        owner_symbols: &[
            glue_fn("read_user_config"),
            glue_fn("write_user_config"),
            glue_fn("config_path"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller_file(CLI_INFORMATION),
            glue_caller_file("apps/actinglab/src/commands/capabilities.rs"),
            glue_caller_file(DEVICE_COMMANDS),
            glue_caller_file("apps/actinglab/src/commands/navigation_recovery.rs"),
            glue_caller_file(SESSION_RECORD),
            glue_caller_file("apps/actinglab/src/drive_cli.rs"),
            glue_caller_file("apps/actinglab/src/env_detection.rs"),
            glue_caller_file("apps/actinglab/src/lab2_cli.rs"),
            glue_caller_file("apps/actinglab/src/lab_run.rs"),
            glue_caller_file("apps/actinglab/src/readonly_cli.rs"),
            glue_caller_file("apps/actinglab/src/resource_runtime_support.rs"),
            glue_caller_file("apps/actinglab/src/run_summary.rs"),
            glue_caller_file("apps/actinglab/src/runtime_stream_adapter.rs"),
            glue_caller_file("apps/actinglab/src/session_management.rs"),
            glue_caller_file("apps/actinglab/src/user_config_store.rs"),
        ],
        required_calls: &[
            exact(
                CLI_INFORMATION,
                "use crate::user_config_store::{config_path, read_user_config, write_user_config};",
                1,
            ),
            absent(
                ACTINGLAB_MAIN,
                "use user_config_store::{config_path, read_user_config, write_user_config};",
            ),
            absent(
                "apps/actinglab/src/user_config_store.rs",
                "unwrap_or_default",
            ),
        ],
        invariants: &[glue_invariant(
            "read_user_config",
            "serde_json::from_str(&text).map_err(",
        )],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "reading, writing and locating the user config file is owned by \
                 user_config_store.rs; cli_information.rs imports it privately",
    },
    // actinglab_user_config_keys_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/user_config_keys.rs",
        rule: "user_config_keys_glue_stays_out_of_main",
        owner_symbols: &[
            glue_fn("config_get"),
            glue_fn("config_set"),
            glue_private_fn("get_instance_value"),
            glue_private_fn("set_instance_value"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[glue_caller_file(CLI_INFORMATION)],
        required_calls: &[
            exact(
                CLI_INFORMATION,
                "use crate::user_config_keys::{config_get, config_set};",
                1,
            ),
            absent(
                ACTINGLAB_MAIN,
                "use user_config_keys::{config_get, config_set};",
            ),
        ],
        invariants: &[],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "the user config key table (get / set, instance keys) is owned by \
                 user_config_keys.rs; cli_information.rs imports it privately",
    },
    // actinglab_safe_file_stem_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/safe_file_stem.rs",
        rule: "safe_file_stem_glue_stays_out_of_main",
        owner_symbols: &[glue_fn("safe_file_stem")],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[glue_caller_file(SESSION_RECORD)],
        required_calls: &[exact(
            ACTINGLAB_MAIN,
            "use safe_file_stem::safe_file_stem;",
            1,
        )],
        invariants: &[glue_invariant(
            "safe_file_stem",
            "fn safe_file_stem(value: &str) -> String {",
        )],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "file-stem sanitising is owned by safe_file_stem.rs; main.rs imports it privately",
    },
    // actinglab_sha256_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/sha256.rs",
        rule: "sha256_glue_stays_out_of_main",
        owner_symbols: &[glue_fn("file_sha256"), glue_fn("hex_sha256")],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller_file(SESSION_RECORD),
            glue_caller_file("apps/actinglab/src/lab_package_control.rs"),
            glue_caller_file("apps/actinglab/src/sha256.rs"),
        ],
        required_calls: &[
            exact(ACTINGLAB_MAIN, "use sha256::{file_sha256, hex_sha256};", 1),
            exact(
                "apps/actinglab/src/sha256.rs",
                concat!(
                    "use super::{CliError, CliOutcome};\n",
                    "use sha2::{Digest, Sha256};\n",
                    "use std::{fs, path::Path};\n\n",
                ),
                1,
            ),
        ],
        invariants: &[],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "file and byte SHA-256 hashing is owned by sha256.rs; main.rs imports it \
                 privately",
    },
    // actinglab_zip_error_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/zip_error.rs",
        rule: "zip_error_glue_stays_out_of_main",
        owner_symbols: &[glue_fn("zip_write_error"), glue_fn("zip_io_error")],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[glue_caller_file(
            "apps/actinglab/src/resource_runtime_support.rs",
        )],
        required_calls: &[
            exact(
                "apps/actinglab/src/resource_runtime_support.rs",
                "use super::zip_error::{zip_io_error, zip_write_error};",
                1,
            ),
            absent(
                ACTINGLAB_MAIN,
                "use zip_error::{zip_io_error, zip_write_error};",
            ),
            exact(
                "apps/actinglab/src/zip_error.rs",
                "use super::CliError;\nuse std::io;\n\n",
                1,
            ),
        ],
        invariants: &[],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "ZIP write error mapping is owned by zip_error.rs; resource_runtime_support.rs \
                 imports it privately",
    },
    // actinglab_state_roots_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/state_roots.rs",
        rule: "state_roots_glue_stays_out_of_main",
        owner_symbols: &[
            glue_fn("app_state_root"),
            glue_fn("runtime_state_root"),
            glue_fn("session_state_dir_from_flags"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller_file(DEVICE_COMMANDS),
            glue_caller_file(SESSION_RECORD),
            glue_caller_file("apps/actinglab/src/device_runtime_config.rs"),
            glue_caller_file("apps/actinglab/src/env_detection.rs"),
            glue_caller_file("apps/actinglab/src/lab2_cli.rs"),
            glue_caller_file("apps/actinglab/src/lab_run.rs"),
            glue_caller_file("apps/actinglab/src/readonly_cli.rs"),
            glue_caller_file("apps/actinglab/src/runtime_debug.rs"),
            glue_caller_file("apps/actinglab/src/runtime_session_adapter.rs"),
            glue_caller_file("apps/actinglab/src/runtime_stream_adapter.rs"),
            glue_caller_file("apps/actinglab/src/session_management.rs"),
            glue_caller_file("apps/actinglab/src/state_roots.rs"),
            glue_caller_file("apps/actinglab/src/user_config_store.rs"),
        ],
        required_calls: &[
            exact(
                ACTINGLAB_MAIN,
                "use state_roots::{app_state_root, runtime_state_root, session_state_dir_from_flags};",
                1,
            ),
            exact(
                "apps/actinglab/src/state_roots.rs",
                concat!(
                    "use super::{CliError, CliOutcome, FlagArgs, RUNTIME_STATE_ROOT_ENV, SESSION_STATE_ENV};\n",
                    "use std::{env, path::PathBuf};\n\n",
                ),
                1,
            ),
        ],
        invariants: &[],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "the application, Runtime and session state roots are owned by state_roots.rs; \
                 main.rs imports them privately",
    },
    // actinglab_unix_time_glue_stays_out_of_main
    GlueDeclaration {
        module: "apps/actinglab/src/unix_time.rs",
        rule: "unix_time_glue_stays_out_of_main",
        owner_symbols: &[glue_fn("current_unix_ms")],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller("apps/actinglab/src/commands/session_contracts.rs", 4),
            glue_caller(SESSION_RECORD, 12),
            glue_caller("apps/actinglab/src/env_detection.rs", 1),
            glue_caller("apps/actinglab/src/runtime_session_adapter.rs", 1),
            glue_caller("apps/actinglab/src/runtime_stream_adapter.rs", 1),
        ],
        required_calls: &[
            exact(ACTINGLAB_MAIN, "use unix_time::current_unix_ms;", 1),
            exact(
                "apps/actinglab/src/unix_time.rs",
                "use std::time::{SystemTime, UNIX_EPOCH};\n\n",
                1,
            ),
        ],
        invariants: &[glue_invariant(
            "current_unix_ms",
            "fn current_unix_ms() -> u64 {",
        )],
        required_tests: &[],
        tolerated_elsewhere: &[],
        reason: "the Unix millisecond clock is owned by unix_time.rs; main.rs imports it \
                 privately and its callers reach it through the crate root",
    },
    // actinglab_flag_values_glue_stays_out_of_main,
    // actinglab_required_non_empty_flag_glue_stays_out_of_main,
    // actinglab_optional_unit_f64_glue_stays_out_of_main,
    // actinglab_record_duration_flag_glue_stays_out_of_main,
    // actinglab_record_amend_step_id_glue_stays_out_of_main,
    // actinglab_split_csv_glue_stays_out_of_main,
    // actinglab_stream_check_requested_glue_stays_out_of_main,
    // actinglab_target_argument_glue_stays_out_of_main,
    // actinglab_session_record_drift_diagnostics_path_glue_stays_out_of_main,
    // actinglab_parse_touch_backend_override_glue_stays_out_of_main,
    // actinglab_parse_match_metric_flag_glue_stays_out_of_main,
    // actinglab_record_candidates_step_id_glue_stays_out_of_main,
    // actinglab_stream_input_relay_action_glue_stays_out_of_main,
    // actinglab_parse_session_record_region_glue_stays_out_of_main,
    // actinglab_parse_session_record_rect_glue_stays_out_of_main,
    // actinglab_parse_session_record_swipe_rects_glue_stays_out_of_main,
    // actinglab_parse_session_record_candidate_index_glue_stays_out_of_main
    GlueDeclaration {
        module: FLAG_VALUES,
        rule: "flag_values_glue_stays_out_of_main",
        owner_symbols: &[
            glue_fn("parse_optional_duration_ms"),
            glue_fn("parse_optional_usize"),
            glue_fn("parse_optional_string_value"),
            glue_fn("required_non_empty_flag"),
            glue_fn("parse_optional_unit_f64"),
            glue_fn("parse_record_duration_ms"),
            glue_fn("record_amend_step_id"),
            glue_fn("record_candidates_step_id"),
            glue_fn("stream_input_relay_action"),
            glue_fn("stream_check_requested"),
            glue_fn("target_argument"),
            glue_fn("session_record_drift_diagnostics_path"),
            glue_fn("parse_touch_backend_override"),
            glue_fn("parse_match_metric_flag"),
            glue_fn("parse_session_record_region"),
            glue_fn("parse_session_record_rect"),
            glue_fn("parse_session_record_swipe_rects"),
            glue_fn("parse_session_record_candidate_index"),
            glue_fn("split_csv"),
        ],
        forbidden_in: MAIN_ONLY,
        allowed_callers: &[
            glue_caller("apps/actinglab/src/cli_parse.rs", 1),
            glue_caller(DEVICE_COMMANDS, 4),
            glue_caller("apps/actinglab/src/commands/navigation_recovery.rs", 8),
            glue_caller(SESSION_RECORD, 26),
            glue_caller("apps/actinglab/src/commands/session_transport.rs", 1),
            glue_caller("apps/actinglab/src/drive_cli.rs", 3),
            glue_caller("apps/actinglab/src/env_detection.rs", 1),
            glue_caller(FLAG_VALUES, 2),
            glue_caller("apps/actinglab/src/lab2_cli.rs", 9),
            glue_caller("apps/actinglab/src/readonly_cli.rs", 2),
            glue_caller("apps/actinglab/src/runtime_session_adapter.rs", 1),
            glue_caller("apps/actinglab/src/runtime_stream_adapter.rs", 8),
        ],
        required_calls: &[
            exact(
                FLAG_VALUES,
                concat!(
                    "use super::{\n",
                    "    CliError, CliOutcome, FlagArgs, MatchMetric, SessionRecordRect, SessionRecordRegion,\n",
                    "    TouchBackendChoice,\n",
                    "};\n",
                    "use std::path::PathBuf;\n",
                    "use std::time::Duration;\n\n",
                ),
                1,
            ),
            exact(
                FLAG_VALUES,
                concat!(
                    "use super::{\n",
                    "    CliError, CliOutcome, FlagArgs, MatchMetric, SessionRecordRect, ",
                    "SessionRecordRegion,\n",
                    "    TouchBackendChoice,\n",
                    "};",
                ),
                1,
            ),
            exact(FLAG_VALUES, "use std::path::PathBuf;", 1),
            // The region and rectangle types the parsers return stay owned by the session-record
            // commands, crate-visible, and main.rs declares neither more widely.
            exact(SESSION_RECORD, "pub(crate) enum SessionRecordRegion {", 1),
            exact(SESSION_RECORD, "pub(crate) struct SessionRecordRect {", 1),
            absent(ACTINGLAB_MAIN, "pub enum SessionRecordRegion"),
            absent(ACTINGLAB_MAIN, "pub(crate) enum SessionRecordRegion"),
            absent(ACTINGLAB_MAIN, "pub struct SessionRecordRect"),
            absent(ACTINGLAB_MAIN, "pub(crate) struct SessionRecordRect"),
            exact(
                SESSION_RECORD,
                "let id = required_non_empty_flag(flags, \"--id\")?;",
                3,
            ),
            exact(
                SESSION_RECORD,
                "let from = required_non_empty_flag(flags, \"--from\")?;",
                1,
            ),
            exact(
                SESSION_RECORD,
                "let threshold = parse_optional_unit_f64(flags, \"--threshold\")?;",
                2,
            ),
            exact(
                SESSION_RECORD,
                "*target.threshold = parse_optional_unit_f64(flags, \"--threshold\")?;",
                2,
            ),
            exact(
                SESSION_RECORD,
                "duration_ms: parse_record_duration_ms(flags, 500)?,",
                1,
            ),
            exact(
                SESSION_RECORD,
                "duration_ms: parse_record_duration_ms(flags, 700)?,",
                1,
            ),
            exact(
                SESSION_RECORD,
                "let step_id = record_amend_step_id(&flags)?;",
                1,
            ),
            exact(
                SESSION_RECORD,
                concat!(
                    "            let step_id = record_amend_step_id(&flags)?;\n",
                    "            let Some(step) = record.steps.iter_mut().find(|step| step.step_id == step_id) else {",
                ),
                1,
            ),
            present(
                "apps/actinglab/src/cli_parse.rs",
                "global.instances = Some(split_csv(&require_raw(&raw, index, \"--instances\")?));",
            ),
            exact(
                "apps/actinglab/src/lab2_cli.rs",
                ".flat_map(|value| split_csv(&value))",
                2,
            ),
            exact(
                DEVICE_COMMANDS,
                "if parse_touch_backend_override(&flags)?.is_some() || global.touch_backend.is_some() {",
                1,
            ),
            exact(
                "apps/actinglab/src/commands/navigation_recovery.rs",
                "let metric = parse_match_metric_flag(&flags)?;",
                1,
            ),
            exact(
                SESSION_RECORD,
                "let metric = parse_match_metric_flag(flags)?;",
                1,
            ),
            exact(SESSION_RECORD, "Some(parse_match_metric_flag(flags)?)", 1),
            exact(
                SESSION_RECORD,
                "let step_id = record_candidates_step_id(&flags)?;",
                1,
            ),
            exact(
                DEVICE_COMMANDS,
                "if let Some((action, action_args)) = stream_input_relay_action(flags)? {",
                1,
            ),
            exact(
                SESSION_RECORD,
                "let region = parse_session_record_region(&flags.required(\"--region\")?)?;",
                3,
            ),
            exact(
                SESSION_RECORD,
                "*target.region = parse_session_record_region(&value)?;",
                3,
            ),
            exact(
                FLAG_VALUES,
                "parse_session_record_rect(from, \"--swipe from\")?,",
                1,
            ),
            exact(
                FLAG_VALUES,
                "parse_session_record_rect(to, \"--swipe to\")?,",
                1,
            ),
            exact(
                SESSION_RECORD,
                "let (from, to) = parse_session_record_swipe_rects(&swipe)?;",
                1,
            ),
            exact(
                SESSION_RECORD,
                "if let Some(candidate_index) = parse_session_record_candidate_index(flags)? {",
                3,
            ),
        ],
        invariants: &[
            glue_invariant(
                "parse_record_duration_ms",
                "failed to parse --duration-ms '{value}': {err}",
            ),
            glue_invariant("parse_record_duration_ms", "--duration-ms must be positive"),
            glue_invariant("record_amend_step_id", ".optional(\"--step-id\")"),
            glue_invariant("record_amend_step_id", ".filter(|value| value != \"true\")"),
            glue_invariant(
                "record_amend_step_id",
                ".or_else(|| flags.positionals.first().cloned())",
            ),
            glue_invariant(
                "record_amend_step_id",
                "session record amend requires <step-id> or --step-id",
            ),
            glue_invariant("record_amend_step_id", "if value.trim().is_empty()"),
            glue_invariant(
                "record_amend_step_id",
                "record amend step id must not be empty",
            ),
            glue_invariant("record_amend_step_id", "Ok(value)"),
            glue_invariant("target_argument", ".optional(\"--target\")"),
            glue_invariant("target_argument", ".filter(|value| value != \"true\")"),
            glue_invariant("target_argument", "return Ok(target);"),
            glue_invariant("target_argument", ".positionals"),
            glue_invariant("target_argument", ".first()"),
            glue_invariant("target_argument", ".cloned()"),
            glue_invariant(
                "target_argument",
                "{command} requires <target> or --target <id>",
            ),
            glue_invariant(
                "session_record_drift_diagnostics_path",
                ".optional(\"--from-drift-diagnostics\")",
            ),
            glue_invariant("session_record_drift_diagnostics_path", "return Ok(None);"),
            glue_invariant(
                "session_record_drift_diagnostics_path",
                "if value == \"true\"",
            ),
            glue_invariant(
                "session_record_drift_diagnostics_path",
                "session record amend --from-drift-diagnostics requires <path>",
            ),
            glue_invariant(
                "session_record_drift_diagnostics_path",
                "Ok(Some(PathBuf::from(value)))",
            ),
            glue_invariant(
                "parse_touch_backend_override",
                ".optional(\"--touch-backend\")",
            ),
            glue_invariant("parse_touch_backend_override", "return Ok(None);"),
            glue_invariant("parse_touch_backend_override", "if value == \"true\""),
            glue_invariant(
                "parse_touch_backend_override",
                "--touch-backend expects auto, auto-fastest, maatouch, minitouch, or adb_shell_input",
            ),
            glue_invariant(
                "parse_touch_backend_override",
                "TouchBackendChoice::parse(&value)",
            ),
            glue_invariant("parse_touch_backend_override", ".map(Some)"),
            glue_invariant(
                "parse_touch_backend_override",
                ".map_err(|err| CliError::usage(err.to_string()))",
            ),
            glue_invariant("parse_match_metric_flag", ".optional(\"--metric\")"),
            glue_invariant(
                "parse_match_metric_flag",
                ".unwrap_or_else(|| \"ccorr_normed\".to_string())",
            ),
            glue_invariant(
                "parse_match_metric_flag",
                "\"ccorr_normed\" => Ok(MatchMetric::CrossCorrelationNormalized)",
            ),
            glue_invariant(
                "parse_match_metric_flag",
                "\"ccoeff_normed\" => Ok(MatchMetric::CorrelationCoefficientNormalized)",
            ),
            glue_invariant(
                "parse_match_metric_flag",
                "unsupported --metric '{other}', expected ccorr_normed or ccoeff_normed",
            ),
            glue_invariant(
                "record_candidates_step_id",
                concat!(
                    ".optional(\"--step-id\")\n",
                    "        .filter(|value| value != \"true\")\n",
                    "        .or_else(|| flags.positionals.first().cloned())"
                ),
            ),
            glue_invariant(
                "record_candidates_step_id",
                "session record candidates requires <step-id> or --step-id",
            ),
            glue_invariant("record_candidates_step_id", "if value.trim().is_empty()"),
            glue_invariant(
                "record_candidates_step_id",
                "record candidates step id must not be empty",
            ),
            glue_invariant("record_candidates_step_id", "Ok(value)"),
            glue_invariant(
                "stream_input_relay_action",
                concat!(
                    ".optional(\"--input-relay\")\n",
                    "        .or_else(|| flags.optional(\"--interactive-input\"))"
                ),
            ),
            glue_invariant("stream_input_relay_action", "return Ok(None);"),
            glue_invariant("stream_input_relay_action", "if value == \"true\" {"),
            glue_invariant(
                "stream_input_relay_action",
                "stream --input-relay expects an action: tap|swipe|long-tap|key|text",
            ),
            glue_invariant(
                "stream_input_relay_action",
                "flags.positionals.iter().skip(1).cloned().collect(),",
            ),
            glue_invariant(
                "stream_input_relay_action",
                "Ok(Some((value, flags.positionals.clone())))",
            ),
            glue_invariant("parse_session_record_region", "if value == \"auto\""),
            glue_invariant(
                "parse_session_record_region",
                "return Ok(SessionRecordRegion::Auto);",
            ),
            glue_invariant(
                "parse_session_record_region",
                "value.split(',').map(str::trim).collect::<Vec<_>>()",
            ),
            glue_invariant("parse_session_record_region", "if parts.len() != 4"),
            glue_invariant(
                "parse_session_record_region",
                "record anchor region must be auto or x,y,width,height: {value}",
            ),
            glue_invariant(
                "parse_session_record_region",
                "failed to parse record anchor region {name} '{}': {err}",
            ),
            glue_invariant("parse_session_record_region", "x: parse_part(0, \"x\")?"),
            glue_invariant("parse_session_record_region", "y: parse_part(1, \"y\")?"),
            glue_invariant(
                "parse_session_record_region",
                "width: parse_part(2, \"width\")?",
            ),
            glue_invariant(
                "parse_session_record_region",
                "height: parse_part(3, \"height\")?",
            ),
            glue_invariant(
                "parse_session_record_region",
                "if rect.width <= 0 || rect.height <= 0",
            ),
            glue_invariant(
                "parse_session_record_region",
                "record anchor region width and height must be positive",
            ),
            glue_invariant(
                "parse_session_record_region",
                "Ok(SessionRecordRegion::Rect { rect })",
            ),
            glue_invariant(
                "parse_session_record_rect",
                "value.split(',').map(str::trim).collect::<Vec<_>>()",
            ),
            glue_invariant("parse_session_record_rect", "if parts.len() != 4"),
            glue_invariant(
                "parse_session_record_rect",
                "{label} must be formatted as x,y,width,height: {value}",
            ),
            glue_invariant(
                "parse_session_record_rect",
                "failed to parse {label} {name} '{}': {err}",
            ),
            glue_invariant("parse_session_record_rect", "x: parse(0, \"x\")?"),
            glue_invariant("parse_session_record_rect", "y: parse(1, \"y\")?"),
            glue_invariant("parse_session_record_rect", "width: parse(2, \"width\")?"),
            glue_invariant("parse_session_record_rect", "height: parse(3, \"height\")?"),
            glue_invariant(
                "parse_session_record_rect",
                "if rect.width <= 0 || rect.height <= 0",
            ),
            glue_invariant(
                "parse_session_record_rect",
                "{label} dimensions must be positive: {}x{}",
            ),
            glue_invariant("parse_session_record_rect", "Ok(rect)"),
            glue_invariant("parse_session_record_swipe_rects", ".split_once(\"->\")"),
            glue_invariant(
                "parse_session_record_swipe_rects",
                "--swipe must be formatted as x,y,w,h->x,y,w,h",
            ),
            glue_invariant(
                "parse_session_record_swipe_rects",
                "parse_session_record_rect(from, \"--swipe from\")?",
            ),
            glue_invariant(
                "parse_session_record_swipe_rects",
                "parse_session_record_rect(to, \"--swipe to\")?",
            ),
            glue_invariant("parse_session_record_swipe_rects", "Ok(("),
        ],
        required_tests: &[
            "match_metric_flag_preserves_default_values_and_rejection",
            "record_candidates_step_id_preserves_precedence_fallback_errors_and_original_value",
            "stream_input_relay_action_preserves_precedence_fallback_absence_literal_true_errors_and_arguments",
            "retired_record_production_refuses_all_resolution_forms",
            "parse_session_record_region_preserves_auto_rect_whitespace_parse_errors_and_positive_dimensions",
            "parse_session_record_rect_preserves_whitespace_parse_order_labels_errors_and_positive_dimensions",
            "parse_session_record_swipe_rects_preserves_first_arrow_order_labels_and_tuple",
        ],
        tolerated_elsewhere: &[],
        reason: "the flag-value parsers of the ActingLab commands (durations, thresholds, step \
                 ids, targets, backends, metrics, resolutions, regions, rectangles, swipes, \
                 candidate indexes, CSV lists) are owned by flag_values.rs; main.rs imports them \
                 privately for the command modules",
    },
];

#[test]
fn actinglab_glue_declarations_hold() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rust_files(&root.join(ACTINGLAB_SRC), &mut files);
    files.sort();
    let sources = files
        .iter()
        .map(|file| {
            let path = workspace_relative(&root, file);
            let source =
                fs::read_to_string(file).unwrap_or_else(|error| panic!("read {path}: {error}"));
            (path, source)
        })
        .collect::<Vec<_>>();
    assert!(!sources.is_empty(), "{ACTINGLAB_SRC} has no Rust sources");

    let violations = inspect_glue_declarations(&sources, GLUE_DECLARATIONS);
    println!(
        "glue declarations: {} modules, {} owner symbols, {} caller files, {} required calls, \
         {} invariants, {} required tests over {} sources: {} violations",
        GLUE_DECLARATIONS.len(),
        GLUE_DECLARATIONS
            .iter()
            .map(|declaration| declaration.owner_symbols.len())
            .sum::<usize>(),
        GLUE_DECLARATIONS
            .iter()
            .map(|declaration| declaration.allowed_callers.len())
            .sum::<usize>(),
        GLUE_DECLARATIONS
            .iter()
            .map(|declaration| declaration.required_calls.len())
            .sum::<usize>(),
        GLUE_DECLARATIONS
            .iter()
            .map(|declaration| declaration.invariants.len())
            .sum::<usize>(),
        GLUE_DECLARATIONS
            .iter()
            .map(|declaration| declaration.required_tests.len())
            .sum::<usize>(),
        sources.len(),
        violations.len()
    );
    assert!(
        violations.is_empty(),
        "ActingLab glue differs from GLUE_DECLARATIONS ([module] rule: file: reason):\n{}",
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
