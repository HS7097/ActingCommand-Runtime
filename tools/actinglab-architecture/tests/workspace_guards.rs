// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use actingcommand_actinglab_architecture::{
    FunctionReference, LedgerOwnerModule, RefusalBranch, SourceFacts,
    contract_dependency_violations, discover_ledger_owners, extract_command_inventory,
    inspect_call_sites, inspect_contract_fact_matching, inspect_disallowed_lint_escapes,
    inspect_enum_variants, inspect_field_accesses, inspect_function_origin_terms,
    inspect_generic_authoring_identity, inspect_generic_runtime_identity, inspect_lab_source,
    inspect_ledger_append_ingress, inspect_ledger_forbidden_sources, inspect_ledger_public_api,
    inspect_persisted_event_ownership, inspect_producer_event_capabilities,
    inspect_provider_symbol_literals, inspect_public_api, inspect_pure_decision_source,
    inspect_refusal_branches, inspect_source_facts, inspect_stderr_writes,
    lab_removability_violations, ledger_owns_query_matching,
    resource_tooling_removability_violations, workspace_dependency_allow_list_violations,
    workspace_dependency_violations,
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

const GENERIC_AUTHORING_MEMBER_ROOTS: &[&str] =
    &["apps/actinglab", "crates/lab", "crates/resource-tooling"];

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
        // Workflow #392 DA-1 (model 392 v1.2 §13): the display adapter is Runtime-owned core.
        "crates/display-adapter",
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
                Some("actingledger" | "actingcommand-vision-provider-check")
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
        // Workflow #382 P-3 (test plan v2.2 §4 #27): the tokens the retired evaluator purity
        // tests of policy and selection-policy banned beyond the item scan. The lease
        // authority is the scheduler's, so the policy crates name no lease token, and no
        // pure decision step sleeps.
        if !path.starts_with("crates/scheduler/") {
            let production = source
                .split("#[cfg(test)]")
                .next()
                .expect("production source");
            for forbidden in ["LeaseToken", "std::thread::sleep"] {
                if production.contains(forbidden) {
                    violations.push(format!("{path}: {forbidden}"));
                }
            }
        }
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
            // Workflow #390 DRILL-1: a ladder drill may restart the emulator.
            "DrillRecoveryLadder",
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
    // Workflow #429 R-7a (register §1.233, R-7a §3.1 step 2): the player hold's source gate takes
    // the four counters' three envelopes, the Lab's included (Alice's ruling 135).
    (
        &["AcquirePlayerHold", "ReleasePlayerHold"],
        &["invalid_player_hold_origin"],
        &[
            "EventActor::Cli",
            "EventActor::Lab",
            "EventActor::User",
            "EventSource::Cli",
            "EventSource::Lab",
            "EventSource::Ui",
        ],
    ),
];

/// Operations `RuntimeRequest::validate` admits from every origin `valid_client_origin`
/// accepts, with no narrower category. They are classified here explicitly, so a new operation
/// fails the guard until it joins a refusal branch or this list.
const RUNTIME_OPEN_ORIGIN_OPERATIONS: &[&str] = &[
    "AcquireLease",
    "ApplicationLifecycle",
    // Workflow #389 U-1: the new request kinds take no origin gate here; the channel decides
    // at the door (U-6), and LoadPack and UnloadPack take the Lab-only gate with U-3.
    "BindChannel",
    "CancelContainedTask",
    "CancelQueuedLease",
    "CaptureSequence",
    "ClearMonitor",
    "ConfigureMonitor",
    // Workflow #387 D1b (M387 §5.6): a general instruction the three counters send; no origin
    // gate beyond the client sources the host checks.
    "ConfigureSettings",
    // Workflow #429 R-0 (register §1.213): the workbench's kinds take no origin gate here; their
    // slices add the behaviour and the channel decides at the door (U-6).
    "EditPackSchedule",
    "EditResourceTarget",
    // Workflow #384 D2-S5 (register §1.68): the fact refresh's waiting keys, a general read from
    // memory like Status.
    "FactRefreshStatus",
    "Health",
    "Input",
    // Workflow #389 MCP-4 (register W2): a general read from memory, like Status.
    "InstanceFactSnapshot",
    "LoadPack",
    "MonitorStatus",
    "ObserveContainedPage",
    "ObserveReadonly",
    // Workflow #429 R-0c (register §1.216): the pack schedule's read view, a general read like
    // ResourceTargetView.
    "PackScheduleView",
    // Workflow #385 P-m (register §1.61 W15): the shadow measurement's status, a general read
    // from memory like Status.
    "PerformanceStatus",
    "PollQueuedLease",
    "ProjectInterface",
    "QueryEvents",
    "QueueLease",
    "ReadMaterial",
    "RecordClientAction",
    "ReleaseLease",
    "RenewLease",
    "ResourceTargetView",
    // Workflow #429 R-0: a general read from memory like Status.
    "RoutineSummary",
    "RunContainedTask",
    // Workflow #429 R-0: a general instruction of the workbench (R-5).
    "RunInstanceTask",
    "RuntimeFactSnapshot",
    "SafeReset",
    // Workflow #386 SR-6b, MCP-19a (register §1.143): the latest evaluation's not-run
    // explanation, a general read from memory like Status.
    "ScheduleExplanation",
    // Workflow #421 INST-2 (#421 deploy design §4): the Runtime's report of its own start, a
    // general read from memory like Status.
    "StartReport",
    "Status",
    // Workflow #429 R-0: a general instruction of the workbench (R-6).
    "StopInstanceRun",
    "SubscribeEvents",
    "UnloadPack",
    "UpgradeState",
    "UpgradeStateStatus",
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
    // Only an accepted identity card grows the accepted governance connections: any production
    // access other than a read (`contains`) or a cleanup (`remove`) sits in
    // `HostShared::declare_governance_identity`, whichever host file holds it.
    let growing = runtime_host_production_sources(&root)
        .iter()
        .flat_map(|(path, source)| {
            inspect_field_accesses(path, source, "governance_connections")
                .unwrap_or_else(|error| panic!("scan {path}: {error}"))
        })
        .filter(|access| {
            !access.ends_with("-> governance_connections.contains")
                && !access.ends_with("-> governance_connections.remove")
        })
        .collect::<Vec<_>>();
    assert!(
        !growing.is_empty()
            && growing
                .iter()
                .all(|access| access.contains("::HostShared::declare_governance_identity -> ")),
        "only HostShared::declare_governance_identity may grow the accepted governance \
         connections: {growing:?}"
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
    "open_writer_timed",
    "import",
    "LabLedger::create",
    "LabLedger::open_or_create",
    "LabLedger::create_runtime_shard",
    "commit_then_record",
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

    let runtime_contract =
        fs::read_to_string(root.join("crates/actingcommand-contract/src/runtime.rs"))
            .expect("read Runtime contract");
    assert!(
        !runtime_contract.contains("TaskSemanticFact"),
        "clients must not be able to submit Runtime task semantic facts"
    );
}

/// The client contract's contained task request names its package by its path alone: its fields
/// carry no application identity and, since Workflow #336 PKG-S4 (option 丙, M-PKG v1.1 §4.5), no
/// package digest. Read with a parser, so the struct may move within the contract file.
#[test]
fn r35_the_contained_task_request_carries_no_application_identity() {
    struct Identifiers(Vec<String>);
    impl<'ast> syn::visit::Visit<'ast> for Identifiers {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            self.0.push(ident.to_string());
        }
    }

    let root = workspace_root();
    let contract = fs::read_to_string(root.join(RUNTIME_CONTRACT_SOURCE))
        .unwrap_or_else(|error| panic!("read {RUNTIME_CONTRACT_SOURCE}: {error}"));
    let file = syn::parse_file(&contract)
        .unwrap_or_else(|error| panic!("parse {RUNTIME_CONTRACT_SOURCE}: {error}"));
    let request = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(structure) if structure.ident == "ContainedTaskRequest" => {
                Some(structure)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{RUNTIME_CONTRACT_SOURCE} defines no ContainedTaskRequest"));
    assert!(
        request.fields.iter().any(|field| {
            field
                .ident
                .as_ref()
                .is_some_and(|ident| ident == "package_path")
                && matches!(&field.ty, syn::Type::Path(path) if path.path.is_ident("String"))
        }),
        "contained task request lost package_path: String"
    );
    let mut identifiers = Identifiers(Vec::new());
    for field in &request.fields {
        syn::visit::Visit::visit_field(&mut identifiers, field);
    }
    for identifier in &identifiers.0 {
        for forbidden in [
            "game",
            "server",
            "package_name",
            "TaskSemanticFact",
            "expected_sha256",
            "PackageRef",
            "sha256",
        ] {
            assert!(
                !identifier.contains(forbidden),
                "contained task client contract embeds application identity via {identifier}"
            );
        }
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

// Workflow #398 RD-1b, test row T10 (model-398 v1.1 §3.2, §12): the private binding file
// `<state root>\private\device-bindings.json` holds the device serials, so it is read only by
// the binding reader and never exported, archived or put into a diagnostic. Its path constants
// and its reader are named only in the contract's `device_binding` module, actingd's binding
// reader and its one writer, `actingctl devices bind|unbind` (Workflow #398 RD-1c, model-398
// §3.2: "only `actingctl devices bind|unbind` and a person write it"); the evidence export, the
// ledger maintenance archive, the kept-frame folders, the forensic reads and the diagnostic paths
// name none of them.
#[test]
fn rd1b_t10_export_archive_and_diagnostics_never_read_the_private_bindings() {
    const NAMES: &[&str] = &[
        "PRIVATE_DIRECTORY",
        "DEVICE_BINDINGS_FILE",
        "device_bindings_path",
        "read_device_bindings",
        "device-bindings.json",
    ];
    const READERS: &[&str] = &[
        "crates/actingcommand-contract/src/device_binding.rs",
        "apps/actingd/src/config/device_binding.rs",
        "apps/actingctl/src/devices.rs",
    ];
    const NEVER_READING: &[&str] = &[
        "crates/runtime-host/src/host/evidence_export.rs",
        "crates/artifact-store/src/",
        "crates/runtime-host/src/ledger_maintenance.rs",
        "apps/actingd/src/ledger_maintenance.rs",
        "crates/ledger-forensics/src/",
        "apps/ledger-forensics/src/",
        "crates/runtime-host/src/host/device_diagnostic.rs",
        "crates/runtime-host/src/host/task_diagnostic.rs",
        "crates/runtime-host/src/host/ppocr_diagnostic.rs",
    ];
    let sources = workspace_sources(&["crates", "apps", "providers", "tools"]);
    let mut violations = Vec::new();
    for (path, source) in &sources {
        if READERS.contains(&path.as_str()) || path.starts_with("tools/actinglab-architecture/") {
            continue;
        }
        for name in NAMES {
            if source.contains(name) {
                violations.push(format!("{path} names {name}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "the private binding file is named outside its reader:\n{}",
        violations.join("\n")
    );
    for reader in READERS {
        let (_, source) = sources
            .iter()
            .find(|(path, _)| path == reader)
            .unwrap_or_else(|| panic!("the binding reader {reader} exists"));
        assert!(
            NAMES.iter().any(|name| source.contains(name)),
            "{reader} reads the private binding file through its constants"
        );
    }
    for place in NEVER_READING {
        let named = sources
            .iter()
            .filter(|(path, _)| path.starts_with(place))
            .collect::<Vec<_>>();
        assert!(!named.is_empty(), "{place} exists");
        for (path, source) in named {
            assert!(
                !source.contains("device_binding")
                    && !NAMES.iter().any(|name| source.contains(name)),
                "{path} reads the private binding file"
            );
        }
    }
}

/// Workflow #384 D2-S4, test row T32 (frozen model D2 v1.4.2 §5.1, §8 T32; Alice 2026-10-09
/// 22:1x and the ledger observer's exception table, contracts/ledger-observer.md §2 and §6): the
/// fact refresh's code never calls the ledger's write interface. A direct write is allowed only
/// where no probe can see the record, and every record of the D2 path is on a choke point a probe
/// sees, so the exception table has no D2 row: a ledger write in D2's code fails here. The
/// scheduler's half lives in the policy crate, which has no dependency path to the ledger at all
/// (the pure-decision allow lists of B1); the Runtime's half is read below without its tests: the
/// D2 modules whole, and the transition recorder's read in the policy dispatch module.
#[test]
fn d2_t32_the_fact_refresh_code_never_calls_the_ledger_write_interface() {
    // The D2 modules of the scheduler (S4) and the Runtime (S2 bindings, S3 adjustments, S4
    // dispatch, S5 transitions).
    const D2_MODULES: [&str; 5] = [
        "crates/policy/src/evaluator/fact_refresh.rs",
        "crates/runtime-host/src/recognition_binding.rs",
        "crates/runtime-host/src/resource_adjustment.rs",
        "crates/runtime-host/src/fact_refresh.rs",
        "crates/runtime-host/src/fact_refresh_transitions.rs",
    ];
    // D2-S5's read of the recorder, beside the policy dispatch it does not belong to.
    const D2_FUNCTIONS: [(&str, &str, &str); 1] = [(
        "crates/runtime-host/src/host/policy_dispatch.rs",
        "HostShared",
        "fact_refresh_status_read",
    )];
    // The ledger's write interface (contracts/ledger-observer.md §2): the `append*` family,
    // `execute_critical`, `reconcile_*` and the maintenance writer's `import`, and the types that
    // hold it.
    let writes = |reference: &FunctionReference| match reference {
        FunctionReference::Method(_, method) => {
            method.starts_with("append")
                || method == "execute_critical"
                || method.starts_with("reconcile_")
                || method == "import"
        }
        FunctionReference::Call(path)
        | FunctionReference::Struct(path)
        | FunctionReference::Value(path)
        | FunctionReference::Pattern(path) => path.iter().any(|segment| {
            segment.starts_with("append")
                || segment == "execute_critical"
                || segment.starts_with("reconcile_")
                || segment == "GlobalLedger"
                || segment == "LedgerMaintenance"
                || segment == "EventAppender"
        }),
        FunctionReference::SelfField(_) | FunctionReference::Literal(_) => false,
    };
    let root = workspace_root();
    let mut violations = Vec::new();
    let mut functions = Vec::new();
    for facts in production_facts(&root, &D2_MODULES) {
        functions.extend(facts.functions);
    }
    for (path, owner, name) in D2_FUNCTIONS {
        let found = production_facts(&root, &[path])
            .into_iter()
            .flat_map(|facts| facts.functions)
            .filter(|function| function.owner.as_deref() == Some(owner) && function.name == name)
            .collect::<Vec<_>>();
        assert_eq!(
            found.len(),
            1,
            "{path}: {owner}::{name} is not one production function"
        );
        functions.extend(found);
    }
    for function in &functions {
        for (reference, line) in &function.references {
            if writes(reference) {
                violations.push(format!("{}:{line}: {reference:?}", function.site()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "the fact refresh's code calls the ledger's write interface; D2 has no row in the \
         direct-write exception table:\n{}",
        violations.join("\n")
    );
}
