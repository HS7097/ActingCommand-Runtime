// SPDX-License-Identifier: AGPL-3.0-only

//! The outcome-code drift guard (Workflow #378 model section 9).
//!
//! It reads the workspace as source and links no workspace crate. Its checks, and the slice
//! that turns each on:
//!
//! - G1 registries = catalog fragments, both ways (A1);
//! - G2 string channels, G3 `json!` keys, G4 construction, G5 stray literals: every workspace
//!   member starts on [`ALLOW_LIST`]; A2a-A2e remove members as they convert them;
//! - G6 persisted prose (A3) and G10 review (A8) are not implemented yet;
//! - G7 key table and vocabularies = catalog (A1);
//! - G8 naming and prefixes (A1);
//! - G9 the merged file and the released contract (A1).
//!
//! Crate roots come from each manifest (`[lib]`, `[[bin]]`) and Cargo's defaults. Code under
//! `#[cfg(test)]` or `#[cfg(all(test, ...))]`, `#[test]` functions and files reached only through
//! them are skipped. The guard cannot see source pulled in with `include!`, so a non-test
//! `include!` is itself a G2 finding.

pub mod catalog;
pub mod source;

pub use catalog::Catalog;
pub use source::Check;

use source::{ChannelScope, Finding, KeyLine, ModuleDeclaration};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One allow-list entry for G2-G5. A member entry (`item: None`) covers a whole workspace
/// member; an item entry covers one function of one file.
#[derive(Debug, Clone, Copy)]
pub struct AllowEntry {
    pub path: &'static str,
    pub item: Option<&'static str>,
    pub checks: &'static [Check],
    pub reason: &'static str,
}

const CHANNELS: &[Check] = &[Check::G2, Check::G3, Check::G4, Check::G5];

/// The G2-G5 allow list. A1 seeds every workspace member; each A2 part removes the members the
/// Owners line of model section 11 assigns to it when it converts them. The two tools and
/// MCP's JSON-RPC `error_response` are standing entries (kept after B6).
pub const ALLOW_LIST: &[AllowEntry] = &[
    member("apps/actingctl", "A2a converts actingctl"),
    member("apps/actingd", "A2a converts actingd"),
    member(
        "apps/actinglab",
        "A2d lists it until B4 converts the actinglab sites",
    ),
    member("apps/device-test", "A2a converts the device-test tool"),
    member("apps/ledger-forensics", "A2c converts actingledger"),
    member(
        "apps/vision-provider-check",
        "A2a converts the vision-provider-check tool",
    ),
    member(
        "crates/actingcommand-contract",
        "A2a converts the contract crate",
    ),
    member("crates/artifact-store", "A2c converts the artifact store"),
    member(
        "crates/device",
        "A2e converts the device crate (mechanical: registry, constructors, literals)",
    ),
    member(
        "crates/execution-kernel",
        "A2d converts the execution kernel",
    ),
    member("crates/host-metrics", "A2b converts host-metrics"),
    member("crates/lab", "A2d lists it until B4 converts the lab sites"),
    member("crates/ledger", "A2c converts the ledger"),
    member("crates/ledger-forensics", "A2c converts ledger-forensics"),
    member(
        "crates/onnx-provider-support",
        "A2d converts onnx-provider-support",
    ),
    member("crates/pack-containment", "A2d converts pack-containment"),
    member("crates/page-detector", "A2d converts the page detector"),
    member("crates/policy", "A2d converts policy"),
    member("crates/recognition", "A2d converts recognition"),
    member("crates/recognition-pack", "A2d converts recognition-pack"),
    member("crates/resource-tooling", "A2d converts resource-tooling"),
    member("crates/runtime-client", "A2a converts the runtime client"),
    member(
        "crates/runtime-database",
        "A2c converts the runtime database",
    ),
    member("crates/runtime-host", "A2b converts the runtime host"),
    member("crates/runtime-state", "A2c converts the runtime state"),
    member("crates/scheduler", "A2b converts the scheduler"),
    member("crates/selection-policy", "A2d converts selection-policy"),
    member("crates/vision-ffi", "A2d converts vision-ffi"),
    member(
        "providers/ppocr-onnx-json",
        "A2d converts the ppocr provider",
    ),
    member(
        "tools/actinglab-architecture",
        "standing: a test tool, not a result path",
    ),
    member(
        "tools/outcome-guard",
        "standing: the guard itself, not a result path",
    ),
    AllowEntry {
        path: "apps/actingctl/src/mcp/protocol.rs",
        item: Some("error_response"),
        checks: &[Check::G3],
        reason: "standing: JSON-RPC requires error.code and error.message (model section 7.3)",
    },
];

const fn member(path: &'static str, reason: &'static str) -> AllowEntry {
    AllowEntry {
        path,
        item: None,
        checks: CHANNELS,
        reason,
    }
}

/// The workspace root of a member at `tools/<name>`.
pub fn workspace_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.to_path_buf())
}

/// One scanned source file.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub member: String,
    /// Relative to the workspace root, with `/`.
    pub path: String,
    pub scan: source::FileScan,
}

/// The workspace as the checks see it: members, catalog and scanned sources.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub members: Vec<String>,
    pub catalog: Result<Catalog, Vec<String>>,
    pub files: Vec<ScannedFile>,
    /// Files that cannot be read, parsed or found.
    pub errors: Vec<String>,
}

/// Reads the catalog and scans every member's non-test sources once.
pub fn load_workspace(root: &Path) -> Workspace {
    let mut errors = Vec::new();
    let members = workspace_members(root).unwrap_or_else(|error| {
        errors.push(error);
        Vec::new()
    });
    let catalog = catalog::load(root, &members);
    let registered = catalog
        .as_ref()
        .map(Catalog::registered_names)
        .unwrap_or_default();
    let mut files = Vec::new();
    for member in &members {
        let channels = !member_fully_allowed(member);
        scan_member(root, member, channels, &registered, &mut files, &mut errors);
    }
    Workspace {
        root: root.to_path_buf(),
        members,
        catalog,
        files,
        errors,
    }
}

/// The merged catalog text, as `outcome-guard merge` writes it.
pub fn merge(root: &Path) -> Result<String, Vec<String>> {
    let members = workspace_members(root).map_err(|error| vec![error])?;
    let catalog = catalog::load(root, &members)?;
    catalog::merged_text(&catalog).map_err(|error| vec![error])
}

/// G1: every registry entry is an active fragment entry of the registry's own crate, with the
/// same category; every active fragment entry is registered; no spelling is registered twice.
pub fn check_registries(workspace: &Workspace) -> Vec<String> {
    let catalog = match &workspace.catalog {
        Ok(catalog) => catalog,
        Err(errors) => return prefixed("G1", errors),
    };
    let mut violations = Vec::new();
    for file in &workspace.files {
        for error in &file.scan.facts.errors {
            violations.push(format!("G1 {}: {error}", file.path));
        }
    }
    let mut places = BTreeMap::<String, String>::new();
    let mut codes = BTreeSet::new();
    let mut locations = BTreeSet::new();
    for file in &workspace.files {
        let owner = catalog.owner_of(&file.member);
        for registry in &file.scan.facts.registries {
            for entry in &registry.entries {
                let place = format!("{}:{}", file.path, entry.line);
                let spelling = &entry.spelling;
                if let Some(first) = places.insert(spelling.clone(), place.clone()) {
                    violations.push(format!(
                        "G1 {place}: {spelling} is registered twice (also at {first})"
                    ));
                }
                let (table, label) = if registry.locations {
                    locations.insert(spelling.clone());
                    (&catalog.locations, "location")
                } else {
                    codes.insert(spelling.clone());
                    (&catalog.codes, "code")
                };
                let Some(listed) = table.get(spelling) else {
                    violations.push(format!(
                        "G1 {place}: {label} {spelling} is in no fragment's {label}s"
                    ));
                    continue;
                };
                if catalog::text(listed, "status") != Some("active") {
                    violations.push(format!("G1 {place}: {label} {spelling} is retired"));
                }
                if catalog::text(listed, "owner") != owner {
                    violations.push(format!(
                        "G1 {place}: {label} {spelling} belongs to fragment owner {}, not the \
                         registry's crate {}",
                        catalog::text(listed, "owner").unwrap_or("?"),
                        file.member
                    ));
                }
                if !registry.locations
                    && catalog::text(listed, "category") != entry.category.as_deref()
                {
                    violations.push(format!(
                        "G1 {place}: code {spelling} has category {} in its registry and {} in \
                         its fragment",
                        entry.category.as_deref().unwrap_or("?"),
                        catalog::text(listed, "category").unwrap_or("?")
                    ));
                }
            }
        }
    }
    for (table, registered, label) in [
        (&catalog.codes, &codes, "code"),
        (&catalog.locations, &locations, "location"),
    ] {
        for (name, entry) in table {
            if catalog::text(entry, "status") == Some("active") && !registered.contains(name) {
                violations.push(format!(
                    "G1 {label} {name} (owner {}) has no registry entry",
                    catalog::text(entry, "owner").unwrap_or("?")
                ));
            }
        }
    }
    violations
}

/// G2-G5: findings outside the allow list.
pub fn check_channels(workspace: &Workspace) -> Vec<String> {
    let mut violations = Vec::new();
    for file in &workspace.files {
        for finding in &file.scan.findings {
            if !finding_allowed(&file.member, &file.path, finding) {
                violations.push(format!(
                    "{:?} {}:{}: {}",
                    finding.check, file.path, finding.line, finding.detail
                ));
            }
        }
    }
    violations
}

/// G7: `outcome_keys!` equals the catalog key table (names, types, record fields; UI-only
/// `setup_`/`console_` keys are ignored), and every vocabulary built from an enum equals its
/// catalog vocabulary.
pub fn check_keys(workspace: &Workspace) -> Vec<String> {
    let catalog = match &workspace.catalog {
        Ok(catalog) => catalog,
        Err(errors) => return prefixed("G7", errors),
    };
    let mut violations = Vec::new();
    let mut tables = Vec::new();
    for file in &workspace.files {
        for table in &file.scan.facts.key_tables {
            if file.member == CONTRACT_MEMBER {
                tables.push(table);
            } else {
                violations.push(format!(
                    "G7 {}: only {CONTRACT_MEMBER} holds the key table",
                    file.path
                ));
            }
        }
    }
    match tables.as_slice() {
        [table] => compare_key_table(catalog, table, &mut violations),
        _ => violations.push(format!(
            "G7 {CONTRACT_MEMBER} must hold exactly one outcome_keys! table, found {}",
            tables.len()
        )),
    }

    let mut built = BTreeMap::<String, String>::new();
    for file in &workspace.files {
        for vocabulary in &file.scan.facts.vocabularies {
            let place = format!("{} enum {}", file.path, vocabulary.name);
            if let Some(first) = built.insert(vocabulary.vocabulary.clone(), place.clone()) {
                violations.push(format!(
                    "G7 {place}: vocabulary {} is already built by {first}",
                    vocabulary.vocabulary
                ));
            }
            compare_tokens(
                catalog,
                &vocabulary.vocabulary,
                &vocabulary.tokens,
                &place,
                &mut violations,
            );
        }
    }
    for (name, entry) in &catalog.vocabularies {
        let Some(origin) = entry.get("source").and_then(|origin| origin.as_object()) else {
            continue;
        };
        let (Some(file), Some(enum_name)) =
            (catalog::text(origin, "file"), catalog::text(origin, "enum"))
        else {
            continue;
        };
        let place = format!("vocabulary {name} source {file} enum {enum_name}");
        let tokens = fs::read_to_string(workspace.root.join(file))
            .map_err(|error| error.to_string())
            .and_then(|text| source::enum_tokens(&text, enum_name));
        match tokens {
            Ok(tokens) => compare_tokens(catalog, name, &tokens, &place, &mut violations),
            Err(error) => violations.push(format!("G7 {place}: {error}")),
        }
    }
    violations
}

/// G8: names, prefixes and reserved prefixes.
pub fn check_naming(workspace: &Workspace) -> Vec<String> {
    match &workspace.catalog {
        Ok(catalog) => catalog::check_naming(catalog),
        Err(errors) => prefixed("G8", errors),
    }
}

/// G9: the committed merged file is the merge, and released settled entries hold.
pub fn check_contract(workspace: &Workspace) -> Vec<String> {
    match &workspace.catalog {
        Ok(catalog) => catalog::check_contract(&workspace.root, catalog),
        Err(errors) => prefixed("G9", errors),
    }
}

const CONTRACT_MEMBER: &str = "crates/actingcommand-contract";

fn compare_key_table(catalog: &Catalog, table: &[KeyLine], violations: &mut Vec<String>) {
    let lines = table
        .iter()
        .map(|line| (line.name.as_str(), line))
        .collect::<BTreeMap<_, _>>();
    if lines.len() != table.len() {
        violations.push("G7 outcome_keys! lists a key twice".to_owned());
    }
    for (name, entry) in &catalog.keys {
        if is_ui_key(name) {
            continue;
        }
        let Some(line) = lines.get(name.as_str()) else {
            violations.push(format!(
                "G7 catalog key {name} is missing from outcome_keys!"
            ));
            continue;
        };
        let kind = entry
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("");
        if kind != line.kind {
            violations.push(format!(
                "G7 key {name} is {kind} in the catalog and {} in outcome_keys!",
                line.kind
            ));
        }
        let fields = entry
            .get("fields")
            .and_then(|fields| fields.as_object())
            .map(|fields| {
                fields
                    .iter()
                    .map(|(field, presence)| (field.clone(), presence.as_str() == Some("required")))
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        if fields != line.fields.iter().cloned().collect::<BTreeSet<_>>() {
            violations.push(format!(
                "G7 key {name} has other record fields in the catalog than in outcome_keys!"
            ));
        }
    }
    for name in lines.keys() {
        if !is_ui_key(name) && !catalog.keys.contains_key(*name) {
            violations.push(format!("G7 outcome_keys! key {name} is not in the catalog"));
        }
    }
}

fn compare_tokens(
    catalog: &Catalog,
    vocabulary: &str,
    tokens: &[String],
    place: &str,
    violations: &mut Vec<String>,
) {
    let Some(listed) = catalog
        .vocabularies
        .get(vocabulary)
        .and_then(|entry| entry.get("tokens"))
        .and_then(|tokens| tokens.as_object())
    else {
        violations.push(format!("G7 {place}: no catalog vocabulary {vocabulary}"));
        return;
    };
    let built = tokens.iter().cloned().collect::<BTreeSet<_>>();
    let listed = listed.keys().cloned().collect::<BTreeSet<_>>();
    if built.len() != tokens.len() {
        violations.push(format!("G7 {place}: a token is listed twice"));
    }
    for token in built.difference(&listed) {
        violations.push(format!(
            "G7 {place}: token {token} is not in catalog vocabulary {vocabulary}"
        ));
    }
    for token in listed.difference(&built) {
        violations.push(format!(
            "G7 {place}: catalog vocabulary {vocabulary} token {token} is not in the enum"
        ));
    }
}

fn is_ui_key(name: &str) -> bool {
    name.starts_with("setup_") || name.starts_with("console_")
}

fn prefixed(check: &str, errors: &[String]) -> Vec<String> {
    errors
        .iter()
        .map(|error| format!("{check} catalog: {error}"))
        .collect()
}

fn member_fully_allowed(member: &str) -> bool {
    ALLOW_LIST.iter().any(|entry| {
        entry.item.is_none()
            && entry.path == member
            && CHANNELS.iter().all(|check| entry.checks.contains(check))
    })
}

fn finding_allowed(member: &str, path: &str, finding: &Finding) -> bool {
    ALLOW_LIST.iter().any(|entry| {
        entry.checks.contains(&finding.check)
            && match entry.item {
                None => entry.path == member,
                Some(item) => entry.path == path && finding.item.as_deref() == Some(item),
            }
    })
}

/// The `members` of the root `Cargo.toml`.
pub fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("Cargo.toml: cannot read: {error}"))?;
    let start = manifest
        .find("members = [")
        .ok_or_else(|| "Cargo.toml: no workspace members list".to_owned())?;
    let rest = &manifest[start + "members = [".len()..];
    let end = rest
        .find(']')
        .ok_or_else(|| "Cargo.toml: the members list does not end".to_owned())?;
    Ok(rest[..end]
        .split(',')
        .map(|member| member.trim().trim_matches('"').to_owned())
        .filter(|member| !member.is_empty())
        .collect())
}

fn is_outcome_module(path: &str) -> bool {
    path == "crates/actingcommand-contract/src/outcome.rs"
        || path.starts_with("crates/actingcommand-contract/src/outcome/")
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The crate roots of a member: the `[lib]` and `[[bin]]` paths its manifest names (else
/// `src/lib.rs`), plus `src/main.rs` and the `src/bin` targets Cargo discovers.
fn crate_roots(root: &Path, member: &str, errors: &mut Vec<String>) -> Vec<PathBuf> {
    let directory = root.join(member);
    let manifest = match fs::read_to_string(directory.join("Cargo.toml")) {
        Ok(manifest) => manifest,
        Err(error) => {
            errors.push(format!("{member}/Cargo.toml: cannot read: {error}"));
            return Vec::new();
        }
    };
    let (library, binaries) = manifest_target_paths(&manifest);
    let source = directory.join("src");
    let mut declared = binaries
        .iter()
        .map(|path| directory.join(path))
        .collect::<Vec<_>>();
    if let Some(path) = &library {
        declared.push(directory.join(path));
    }
    for path in &declared {
        if !path.is_file() {
            errors.push(format!(
                "{member}/Cargo.toml: target path {} is not a file",
                relative_path(root, path)
            ));
        }
    }
    let mut roots = declared
        .into_iter()
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    let defaults = if library.is_some() {
        vec![source.join("main.rs")]
    } else {
        vec![source.join("lib.rs"), source.join("main.rs")]
    };
    roots.extend(defaults.into_iter().filter(|path| path.is_file()));
    if let Ok(entries) = fs::read_dir(source.join("bin")) {
        let mut bins = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter_map(|path| {
                if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
                    Some(path)
                } else {
                    Some(path.join("main.rs")).filter(|main| main.is_file())
                }
            })
            .collect::<Vec<_>>();
        bins.sort();
        roots.extend(bins);
    }
    roots
}

/// The `path` of the `[lib]` section and of each `[[bin]]` section of a manifest.
fn manifest_target_paths(manifest: &str) -> (Option<String>, Vec<String>) {
    let mut section = "";
    let mut library = None;
    let mut binaries = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some(value) = line
            .strip_prefix("path")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
        else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_owned();
        match section {
            "[lib]" => library = Some(value),
            "[[bin]]" => binaries.push(value),
            _ => {}
        }
    }
    (library, binaries)
}

/// Walks one member's module tree from its crate roots, skipping test modules.
fn scan_member(
    root: &Path,
    member: &str,
    channels: bool,
    registered: &BTreeSet<String>,
    files: &mut Vec<ScannedFile>,
    errors: &mut Vec<String>,
) {
    let roots = crate_roots(root, member, errors);
    if roots.is_empty() {
        errors.push(format!("{member}: no crate root under src"));
        return;
    }
    let mut seen = BTreeSet::new();
    let mut queue = roots
        .into_iter()
        .map(|path| (path, true))
        .collect::<Vec<_>>();
    while let Some((path, crate_root)) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let relative = relative_path(root, &path);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{relative}: cannot read: {error}"));
                continue;
            }
        };
        let scope = channels.then_some(ChannelScope {
            registered,
            outcome_module: is_outcome_module(&relative),
        });
        let scan = match source::scan_file(&text, scope) {
            Ok(scan) => scan,
            Err(error) => {
                errors.push(format!("{relative}: does not parse: {error}"));
                continue;
            }
        };
        for declaration in scan.modules.iter().filter(|declaration| !declaration.test) {
            match module_file(&path, crate_root, declaration) {
                Some(child) => queue.push((child, false)),
                None => errors.push(format!(
                    "{relative}: no file for module {}",
                    declaration.names.join("::")
                )),
            }
        }
        files.push(ScannedFile {
            member: member.to_owned(),
            path: relative,
            scan,
        });
    }
}

/// The file of an out-of-line module declared in `file`.
fn module_file(file: &Path, crate_root: bool, declaration: &ModuleDeclaration) -> Option<PathBuf> {
    let directory = file.parent()?;
    let (name, inline) = declaration.names.split_last()?;
    let mod_rs = file
        .file_name()
        .is_some_and(|file_name| file_name == "mod.rs");
    if let Some(path) = &declaration.path {
        let mut base = directory.to_path_buf();
        if !inline.is_empty() {
            if !crate_root && !mod_rs {
                base.push(file.file_stem()?);
            }
            base.extend(inline);
        }
        let candidate = base.join(path);
        return candidate.is_file().then_some(candidate);
    }
    let mut base = directory.to_path_buf();
    if !crate_root && !mod_rs {
        base.push(file.file_stem()?);
    }
    base.extend(inline);
    [
        base.join(format!("{name}.rs")),
        base.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}
