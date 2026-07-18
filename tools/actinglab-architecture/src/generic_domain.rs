// SPDX-License-Identifier: AGPL-3.0-only

//! Machine-readable generic-domain concepts and protected Runtime surfaces.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use quote::ToTokens;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use syn::visit::Visit;
use syn::{BinOp, Expr, ExprBinary, ExprMatch, ImplItem, Item, Lit, Member, Visibility};

use crate::external_compat::{EXTERNAL_COMPAT_MANIFEST_PATH, load_and_validate_external_compat};
use crate::{inspect_generic_runtime_identity_with_allowances, known_project_identity_tokens};

pub const GENERIC_DOMAIN_SCHEMA_VERSION: &str = "actingcommand.generic-domain.v1";
pub const GENERIC_DOMAIN_REGISTRY_PATH: &str =
    "tools/actinglab-architecture/generic-domain-v1.toml";
pub const REQUIRED_PROTECTED_ROOTS: &[&str] = &[
    "benchmarks/workloads",
    "contracts",
    "ratchet",
    "resources",
    "tests",
];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GenericDomainRegistry {
    pub schema_version: String,
    #[serde(default)]
    pub concept: Vec<GenericConcept>,
    #[serde(default)]
    pub identity_allowance: Vec<IdentityAllowance>,
    #[serde(default)]
    pub surface: Vec<ProtectedSurface>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GenericConcept {
    pub id: String,
    pub status: String,
    pub approval_comment_id: u64,
    #[serde(default)]
    pub replaced_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtectedSurface {
    pub surface_id: String,
    pub kind: String,
    pub stable_path: String,
    pub concept_ids: Vec<String>,
    pub fingerprint: String,
    pub source_issue: u64,
    #[serde(default)]
    pub source_pr: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityAllowance {
    pub id: String,
    pub kind: String,
    pub exact_path: String,
    pub tokens: Vec<String>,
    pub sha256: String,
    pub purpose: String,
    pub approval_comment_id: u64,
    pub source_issue: u64,
    #[serde(default)]
    pub source_pr: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SurfaceSnapshot {
    pub kind: String,
    pub stable_path: String,
    pub fingerprint: String,
}

pub fn parse_generic_domain_registry(source: &str) -> Result<GenericDomainRegistry, String> {
    toml::from_str(source).map_err(|error| format!("invalid generic-domain registry: {error}"))
}

pub fn load_generic_domain_registry(path: &Path) -> Result<GenericDomainRegistry, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse_generic_domain_registry(&source)
}

pub fn validate_generic_domain_registry(registry: &GenericDomainRegistry) -> Result<(), String> {
    let mut errors = Vec::new();
    if registry.schema_version != GENERIC_DOMAIN_SCHEMA_VERSION {
        errors.push(format!(
            "unsupported schema_version {}; expected {GENERIC_DOMAIN_SCHEMA_VERSION}",
            registry.schema_version
        ));
    }
    if registry.concept.is_empty() {
        errors.push("generic-domain registry contains no concepts".to_string());
    }
    if registry.surface.is_empty() {
        errors.push("generic-domain registry contains no protected surfaces".to_string());
    }

    let mut concept_ids = HashSet::new();
    let mut previous_concept = None;
    for concept in &registry.concept {
        if !is_concept_id(&concept.id) {
            errors.push(format!("invalid concept id {}", concept.id));
        }
        if !concept_ids.insert(concept.id.as_str()) {
            errors.push(format!("duplicate concept id {}", concept.id));
        }
        if previous_concept.is_some_and(|previous: &str| previous >= concept.id.as_str()) {
            errors.push(format!(
                "concept ids are not strictly sorted at {}",
                concept.id
            ));
        }
        previous_concept = Some(concept.id.as_str());
        match concept.status.as_str() {
            "active" if concept.replaced_by.is_some() => errors.push(format!(
                "active concept {} cannot declare replaced_by",
                concept.id
            )),
            "active" => {}
            "deprecated" if concept.replaced_by.is_none() => errors.push(format!(
                "deprecated concept {} must declare replaced_by",
                concept.id
            )),
            "deprecated" => {}
            status => errors.push(format!(
                "concept {} has invalid status {status}",
                concept.id
            )),
        }
        if concept.approval_comment_id == 0 {
            errors.push(format!(
                "concept {} has no Alice approval_comment_id",
                concept.id
            ));
        }
    }
    for concept in &registry.concept {
        if let Some(replacement) = &concept.replaced_by
            && (!concept_ids.contains(replacement.as_str()) || replacement == &concept.id)
        {
            errors.push(format!(
                "concept {} has invalid replacement {replacement}",
                concept.id
            ));
        }
    }

    let known_tokens = known_project_identity_tokens();
    let mut allowance_ids = HashSet::new();
    let mut allowance_paths = HashSet::new();
    let mut previous_allowance = None;
    for allowance in &registry.identity_allowance {
        if !is_surface_id(&allowance.id) {
            errors.push(format!(
                "identity allowance has invalid id {}",
                allowance.id
            ));
        }
        if !allowance_ids.insert(allowance.id.as_str()) {
            errors.push(format!("duplicate identity allowance id {}", allowance.id));
        }
        if previous_allowance.is_some_and(|previous: &str| previous >= allowance.id.as_str()) {
            errors.push(format!(
                "identity allowance ids are not strictly sorted at {}",
                allowance.id
            ));
        }
        previous_allowance = Some(allowance.id.as_str());
        if !matches!(
            allowance.kind.as_str(),
            "guard_fixture" | "technical_adapter" | "upstream_metadata"
        ) {
            errors.push(format!(
                "identity allowance {} has invalid kind {}",
                allowance.id, allowance.kind
            ));
        }
        if let Err(error) = validate_stable_path(&allowance.exact_path) {
            errors.push(format!("identity allowance {} {error}", allowance.id));
        }
        if !allowance_paths.insert(allowance.exact_path.as_str()) {
            errors.push(format!(
                "duplicate identity allowance exact_path {}",
                allowance.exact_path
            ));
        }
        if allowance.tokens.is_empty() && allowance.kind != "guard_fixture" {
            errors.push(format!("identity allowance {} has no tokens", allowance.id));
        }
        let mut previous_token = None;
        let mut tokens = HashSet::new();
        for token in &allowance.tokens {
            if !known_tokens.contains(token) {
                errors.push(format!(
                    "identity allowance {} references unknown detector token {token}",
                    allowance.id
                ));
            }
            if !tokens.insert(token.as_str()) {
                errors.push(format!(
                    "identity allowance {} repeats token {token}",
                    allowance.id
                ));
            }
            if previous_token.is_some_and(|previous: &str| previous >= token.as_str()) {
                errors.push(format!(
                    "identity allowance {} tokens are not strictly sorted at {token}",
                    allowance.id
                ));
            }
            previous_token = Some(token.as_str());
        }
        if !is_sha256(&allowance.sha256) {
            errors.push(format!(
                "identity allowance {} sha256 must be lowercase SHA-256",
                allowance.id
            ));
        }
        if allowance.purpose.trim().is_empty() || allowance.purpose.len() > 256 {
            errors.push(format!(
                "identity allowance {} has invalid purpose",
                allowance.id
            ));
        }
        if allowance.approval_comment_id == 0 {
            errors.push(format!(
                "identity allowance {} has no Alice approval_comment_id",
                allowance.id
            ));
        }
        if allowance.source_issue == 0 {
            errors.push(format!(
                "identity allowance {} has no source_issue",
                allowance.id
            ));
        }
        if allowance.source_pr == Some(0) {
            errors.push(format!(
                "identity allowance {} has invalid source_pr",
                allowance.id
            ));
        }
    }

    let mut surface_ids = HashSet::new();
    let mut stable_paths = HashSet::new();
    let mut previous_surface = None;
    for surface in &registry.surface {
        if !is_surface_id(&surface.surface_id) {
            errors.push(format!("invalid surface id {}", surface.surface_id));
        }
        if !surface_ids.insert(surface.surface_id.as_str()) {
            errors.push(format!("duplicate surface id {}", surface.surface_id));
        }
        if previous_surface.is_some_and(|previous: &str| previous >= surface.surface_id.as_str()) {
            errors.push(format!(
                "surface ids are not strictly sorted at {}",
                surface.surface_id
            ));
        }
        previous_surface = Some(surface.surface_id.as_str());
        if !matches!(surface.kind.as_str(), "protected_root" | "workspace_member") {
            errors.push(format!(
                "surface {} has invalid kind {}",
                surface.surface_id, surface.kind
            ));
        }
        if let Err(error) = validate_stable_path(&surface.stable_path) {
            errors.push(format!("surface {} {error}", surface.surface_id));
        }
        if !stable_paths.insert(surface.stable_path.as_str()) {
            errors.push(format!(
                "duplicate protected stable_path {}",
                surface.stable_path
            ));
        }
        if surface.concept_ids.is_empty() {
            errors.push(format!(
                "surface {} has no concept mappings",
                surface.surface_id
            ));
        }
        let mut previous_mapping = None;
        let mut mappings = HashSet::new();
        for concept_id in &surface.concept_ids {
            if !concept_ids.contains(concept_id.as_str()) {
                errors.push(format!(
                    "surface {} references unknown concept {concept_id}",
                    surface.surface_id
                ));
            }
            if !mappings.insert(concept_id.as_str()) {
                errors.push(format!(
                    "surface {} repeats concept {concept_id}",
                    surface.surface_id
                ));
            }
            if previous_mapping.is_some_and(|previous: &str| previous >= concept_id.as_str()) {
                errors.push(format!(
                    "surface {} concept_ids are not strictly sorted at {concept_id}",
                    surface.surface_id
                ));
            }
            previous_mapping = Some(concept_id.as_str());
        }
        if !is_sha256(&surface.fingerprint) {
            errors.push(format!(
                "surface {} fingerprint must be lowercase SHA-256",
                surface.surface_id
            ));
        }
        if surface.source_issue == 0 {
            errors.push(format!(
                "surface {} has no source_issue",
                surface.surface_id
            ));
        }
        if surface.source_pr == Some(0) {
            errors.push(format!(
                "surface {} has invalid source_pr",
                surface.surface_id
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        errors.sort();
        Err(errors.join("\n"))
    }
}

pub fn workspace_surface_snapshot(root: &Path) -> Result<Vec<SurfaceSnapshot>, String> {
    let mut snapshots = workspace_members(root)?
        .into_iter()
        .map(|stable_path| snapshot_for_root(root, "workspace_member", &stable_path))
        .collect::<Result<Vec<_>, _>>()?;
    for stable_path in REQUIRED_PROTECTED_ROOTS {
        snapshots.push(snapshot_for_root(root, "protected_root", stable_path)?);
    }
    snapshots.sort_by(|left, right| left.stable_path.cmp(&right.stable_path));
    Ok(snapshots)
}

pub fn validate_workspace_surface_registry(
    root: &Path,
    registry: &GenericDomainRegistry,
) -> Result<(), String> {
    validate_generic_domain_registry(registry)?;
    let expected = workspace_surface_snapshot(root)?;
    let registered = registry
        .surface
        .iter()
        .map(|surface| (surface.stable_path.as_str(), surface))
        .collect::<HashMap<_, _>>();
    let expected_paths = expected
        .iter()
        .map(|snapshot| snapshot.stable_path.clone())
        .collect::<HashSet<_>>();
    let mut errors = Vec::new();

    for snapshot in &expected {
        let Some(surface) = registered.get(snapshot.stable_path.as_str()) else {
            errors.push(format!(
                "unmapped protected surface {}",
                snapshot.stable_path
            ));
            continue;
        };
        if surface.kind != snapshot.kind {
            errors.push(format!(
                "surface {} kind drifted: registered {}, actual {}",
                snapshot.stable_path, surface.kind, snapshot.kind
            ));
        }
        if surface.fingerprint != snapshot.fingerprint {
            errors.push(format!(
                "surface {} fingerprint drifted: registered {}, actual {}",
                snapshot.stable_path, surface.fingerprint, snapshot.fingerprint
            ));
        }
    }
    for surface in &registry.surface {
        if !expected_paths.contains(&surface.stable_path) {
            errors.push(format!(
                "registered surface {} no longer exists",
                surface.stable_path
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        errors.sort();
        Err(errors.join("\n"))
    }
}

/// Validates every registered Runtime surface against structural and identity boundaries.
pub fn validate_workspace_genericity(
    root: &Path,
    registry: &GenericDomainRegistry,
) -> Result<(), String> {
    validate_workspace_surface_registry(root, registry)?;
    let external = load_and_validate_external_compat(root)?;
    let external_paths = external
        .entry
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<HashSet<_>>();
    let allowance_by_path = validate_identity_allowance_files(root, registry)?;

    let mut roots = workspace_members(root)?;
    roots.extend(
        REQUIRED_PROTECTED_ROOTS
            .iter()
            .map(|path| (*path).to_string()),
    );
    roots.sort();
    roots.dedup();
    let mut files = Vec::new();
    for stable_path in roots {
        collect_protected_files(&root.join(&stable_path), &mut files)?;
    }
    files.sort();
    files.dedup();

    let mut errors = Vec::new();
    let mut covered_paths = HashSet::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped workspace root", file.display()))?;
        let relative = normalize_path(relative)?;
        covered_paths.insert(relative.clone());
        if relative == GENERIC_DOMAIN_REGISTRY_PATH
            || relative == EXTERNAL_COMPAT_MANIFEST_PATH
            || external_paths.contains(relative.as_str())
        {
            continue;
        }
        let source = fs::read_to_string(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let allowed_tokens = allowance_by_path
            .get(relative.as_str())
            .map_or_else(HashSet::new, |allowance| {
                allowance.tokens.iter().cloned().collect()
            });
        errors.extend(inspect_generic_runtime_identity_with_allowances(
            &relative,
            &source,
            &allowed_tokens,
        ));
        if file.extension().is_some_and(|extension| extension == "rs")
            && allowance_by_path
                .get(relative.as_str())
                .is_none_or(|allowance| allowance.kind != "guard_fixture")
        {
            errors.extend(inspect_identity_axis_branches(&relative, &source)?);
        }
    }
    for allowance in &registry.identity_allowance {
        if !covered_paths.contains(&allowance.exact_path) {
            errors.push(format!(
                "identity allowance {} path is outside registered Runtime surfaces: {}",
                allowance.id, allowance.exact_path
            ));
        }
    }

    errors.sort();
    errors.dedup();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn validate_identity_allowance_files<'a>(
    root: &Path,
    registry: &'a GenericDomainRegistry,
) -> Result<HashMap<&'a str, &'a IdentityAllowance>, String> {
    let mut errors = Vec::new();
    let mut by_path = HashMap::new();
    for allowance in &registry.identity_allowance {
        by_path.insert(allowance.exact_path.as_str(), allowance);
        match resolve_exact_regular_file(root, &allowance.exact_path) {
            Ok(path) => match fs::read(&path) {
                Ok(bytes) => {
                    let actual = format!("{:x}", Sha256::digest(bytes));
                    if actual != allowance.sha256 {
                        errors.push(format!(
                            "identity allowance {} content hash drifted: registered {}, actual {actual}",
                            allowance.id, allowance.sha256
                        ));
                    }
                }
                Err(error) => errors.push(format!(
                    "identity allowance {} failed to read {}: {error}",
                    allowance.id,
                    path.display()
                )),
            },
            Err(error) => errors.push(format!("identity allowance {} {error}", allowance.id)),
        }
    }
    if errors.is_empty() {
        Ok(by_path)
    } else {
        errors.sort();
        Err(errors.join("\n"))
    }
}

fn resolve_exact_regular_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_stable_path(relative)?;
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        format!(
            "failed to resolve workspace root {}: {error}",
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err(format!("has unsafe exact_path {relative}"));
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("failed to inspect {}: {error}", current.display()))?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "exact_path {relative} crosses a symlink or reparse point at {}",
                current.display()
            ));
        }
    }
    let canonical = fs::canonicalize(&current)
        .map_err(|error| format!("failed to resolve {}: {error}", current.display()))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(format!("exact_path {relative} escapes the workspace"));
    }
    if !canonical.is_file() {
        return Err(format!("exact_path {relative} is not a regular file"));
    }
    Ok(canonical)
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub fn inspect_identity_axis_branches(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse {path}: {error}"))?;
    let mut visitor = IdentityBranchVisitor {
        path,
        violations: Vec::new(),
    };
    visitor.visit_file(&file);
    visitor.violations.sort();
    visitor.violations.dedup();
    Ok(visitor.violations)
}

fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let manifest_path = root.join("Cargo.toml");
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", manifest_path.display()))?;
    let members = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "workspace members must be an array".to_string())?;
    let mut result = members
        .iter()
        .map(|member| {
            member
                .as_str()
                .ok_or_else(|| "workspace member must be a string".to_string())
                .and_then(|member| {
                    validate_stable_path(member)?;
                    Ok(member.to_string())
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    result.sort();
    if result.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("workspace contains duplicate members".to_string());
    }
    Ok(result)
}

fn snapshot_for_root(
    root: &Path,
    kind: &str,
    stable_path: &str,
) -> Result<SurfaceSnapshot, String> {
    validate_stable_path(stable_path)?;
    let target = root.join(stable_path);
    if !target.is_dir() {
        return Err(format!(
            "protected surface root {} is not a directory",
            target.display()
        ));
    }
    let mut files = Vec::new();
    collect_protected_files(&target, &mut files)?;
    files.sort();
    let mut records = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped workspace root", file.display()))?;
        let relative = normalize_path(relative)?;
        if relative == "tools/actinglab-architecture/generic-domain-v1.toml" {
            continue;
        }
        let source = fs::read_to_string(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        if file.extension().is_some_and(|extension| extension == "rs") {
            let inventory = rust_surface_inventory(&relative, &source)?;
            if !inventory.is_empty() {
                records.push(format!("{relative}\n{}", inventory.join("\n")));
            }
        } else {
            records.push(format!(
                "{relative}\n{}",
                source.replace("\r\n", "\n").replace('\r', "\n")
            ));
        }
    }
    records.sort();
    let fingerprint = format!("{:x}", Sha256::digest(records.join("\n--\n").as_bytes()));
    Ok(SurfaceSnapshot {
        kind: kind.to_string(),
        stable_path: stable_path.to_string(),
        fingerprint,
    })
}

fn collect_protected_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read directory {}: {error}", root.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("failed to read directory entry: {error}"))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "protected Runtime surface contains a symlink or reparse point: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_protected_files(&path, files)?;
        } else if is_protected_file(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_protected_file(path: &Path) -> bool {
    if path.file_name().is_some_and(|name| name == "Cargo.toml") {
        return true;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "json" | "md" | "rs" | "sql" | "stderr" | "toml" | "txt" | "yaml" | "yml"
            )
        })
}

fn rust_surface_inventory(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse {path}: {error}"))?;
    let mut visitor = ProtectedSurfaceVisitor::default();
    visitor.visit_file(&file);
    visitor.entries.sort();
    visitor.entries.dedup();
    Ok(visitor.entries)
}

#[derive(Default)]
struct ProtectedSurfaceVisitor {
    entries: Vec<String>,
}

impl<'ast> Visit<'ast> for ProtectedSurfaceVisitor {
    fn visit_item(&mut self, node: &'ast Item) {
        match node {
            Item::Const(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.const:{}", item.to_token_stream()));
            }
            Item::Enum(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.enum:{}", item.to_token_stream()));
            }
            Item::ExternCrate(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.extern:{}", item.to_token_stream()));
            }
            Item::Fn(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.fn:{}", item.sig.to_token_stream()));
            }
            Item::Macro(item)
                if item
                    .attrs
                    .iter()
                    .any(|attribute| attribute.path().is_ident("macro_export")) =>
            {
                self.entries
                    .push(format!("public.macro:{}", item.mac.path.to_token_stream()));
            }
            Item::Mod(item) if is_public(&item.vis) => {
                self.entries.push(format!("public.mod:{}", item.ident));
            }
            Item::Static(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.static:{}", item.to_token_stream()));
            }
            Item::Struct(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.struct:{}", item.to_token_stream()));
            }
            Item::Trait(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.trait:{}", item.to_token_stream()));
            }
            Item::TraitAlias(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.trait_alias:{}", item.to_token_stream()));
            }
            Item::Type(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.type:{}", item.to_token_stream()));
            }
            Item::Union(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.union:{}", item.to_token_stream()));
            }
            Item::Use(item) if is_public(&item.vis) => {
                self.entries
                    .push(format!("public.use:{}", item.to_token_stream()));
            }
            _ => {}
        }
        syn::visit::visit_item(self, node);
    }

    fn visit_impl_item(&mut self, node: &'ast ImplItem) {
        match node {
            ImplItem::Const(item) if is_public(&item.vis) => self
                .entries
                .push(format!("public.impl_const:{}", item.to_token_stream())),
            ImplItem::Fn(item) if is_public(&item.vis) => self
                .entries
                .push(format!("public.impl_fn:{}", item.sig.to_token_stream())),
            ImplItem::Type(item) if is_public(&item.vis) => self
                .entries
                .push(format!("public.impl_type:{}", item.to_token_stream())),
            _ => {}
        }
        syn::visit::visit_impl_item(self, node);
    }

    fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
        let protected = node.path().segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "arg" | "clap" | "command" | "serde" | "value"
            )
        });
        if protected {
            self.entries
                .push(format!("attribute:{}", node.to_token_stream()));
        }
        syn::visit::visit_attribute(self, node);
    }

    fn visit_expr_lit(&mut self, node: &'ast syn::ExprLit) {
        match &node.lit {
            Lit::Str(value) => self
                .entries
                .push(format!("literal.str:{:?}", value.value())),
            Lit::ByteStr(value) => self
                .entries
                .push(format!("literal.bytes:{:?}", value.value())),
            _ => {}
        }
        syn::visit::visit_expr_lit(self, node);
    }
}

struct IdentityBranchVisitor<'a> {
    path: &'a str,
    violations: Vec<String>,
}

impl Visit<'_> for IdentityBranchVisitor<'_> {
    fn visit_expr_binary(&mut self, node: &ExprBinary) {
        if matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_)) {
            if let (Some(axis), Some(value)) =
                (identity_axis(&node.left), expression_string(&node.right))
            {
                self.record(axis, &value);
            }
            if let (Some(axis), Some(value)) =
                (identity_axis(&node.right), expression_string(&node.left))
            {
                self.record(axis, &value);
            }
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        if let Some(axis) = identity_axis(&node.expr) {
            for arm in &node.arms {
                let mut strings = PatternStringVisitor::default();
                strings.visit_pat(&arm.pat);
                for value in strings.values {
                    self.record(axis, &value);
                }
            }
        }
        syn::visit::visit_expr_match(self, node);
    }
}

impl IdentityBranchVisitor<'_> {
    fn record(&mut self, axis: &str, value: &str) {
        self.violations.push(format!(
            "{}: built-in identity value {value:?} on axis {axis}",
            self.path
        ));
    }
}

#[derive(Default)]
struct PatternStringVisitor {
    values: Vec<String>,
}

impl Visit<'_> for PatternStringVisitor {
    fn visit_lit_str(&mut self, node: &syn::LitStr) {
        self.values.push(node.value());
    }
}

fn identity_axis(expression: &Expr) -> Option<&str> {
    match expression {
        Expr::Field(field) => match &field.member {
            Member::Named(identifier) => exact_identity_axis(&identifier.to_string()),
            Member::Unnamed(_) => None,
        },
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .and_then(|segment| exact_identity_axis(&segment.ident.to_string())),
        Expr::Paren(paren) => identity_axis(&paren.expr),
        Expr::Reference(reference) => identity_axis(&reference.expr),
        _ => None,
    }
}

fn expression_string(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(value) => Some(value.value()),
            _ => None,
        },
        Expr::Paren(paren) => expression_string(&paren.expr),
        Expr::Reference(reference) => expression_string(&reference.expr),
        _ => None,
    }
}

fn exact_identity_axis(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "game" => Some("game"),
        "package" => Some("package"),
        "profile" => Some("profile"),
        "project" => Some("project"),
        "server" => Some("server"),
        "theme" => Some("theme"),
        _ => None,
    }
}

fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

fn validate_stable_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains('\\')
        || path.contains('*')
        || path.contains('?')
        || path.starts_with('/')
        || path.ends_with('/')
    {
        return Err(format!("has invalid stable_path {path}"));
    }
    let parsed = Path::new(path);
    if parsed
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("has unsafe stable_path {path}"));
    }
    Ok(())
}

fn normalize_path(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("path {} is not UTF-8", path.display()))?
        .replace('\\', "/");
    validate_stable_path(&value)?;
    Ok(value)
}

fn is_concept_id(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(family), Some(term), None)
            if is_registry_segment(family) && is_registry_segment(term)
    )
}

fn is_surface_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('.')
            .all(|segment| is_registry_segment(segment) && !segment.is_empty())
}

fn is_registry_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn registry_source() -> String {
        format!(
            r#"
schema_version = "actingcommand.generic-domain.v1"

[[concept]]
id = "identity.game"
status = "active"
approval_comment_id = 5010683904

[[concept]]
id = "structure.value"
status = "active"
approval_comment_id = 5010683904

[[surface]]
surface_id = "workspace.member"
kind = "workspace_member"
stable_path = "crates/example"
concept_ids = ["identity.game", "structure.value"]
fingerprint = "{}"
source_issue = 44
source_pr = 108
"#,
            "0".repeat(64)
        )
    }

    #[test]
    fn registry_accepts_sorted_approved_concepts_and_exact_surface() {
        let registry = parse_generic_domain_registry(&registry_source()).unwrap();
        validate_generic_domain_registry(&registry).unwrap();
    }

    #[test]
    fn registry_rejects_unknown_concept_duplicate_and_wildcard_surface() {
        let source = registry_source()
            .replace(
                "concept_ids = [\"identity.game\", \"structure.value\"]",
                "concept_ids = [\"identity.unknown\", \"identity.unknown\"]",
            )
            .replace("crates/example", "crates/*");
        let registry = parse_generic_domain_registry(&source).unwrap();
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("unknown concept identity.unknown"));
        assert!(error.contains("repeats concept identity.unknown"));
        assert!(error.contains("invalid stable_path"));
    }

    #[test]
    fn registry_rejects_unapproved_and_invalid_status_transitions() {
        let source = registry_source()
            .replace("status = \"active\"", "status = \"retired\"")
            .replace(
                "approval_comment_id = 5010683904",
                "approval_comment_id = 0",
            );
        let registry = parse_generic_domain_registry(&source).unwrap();
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("invalid status retired"));
        assert!(error.contains("has no Alice approval_comment_id"));
    }

    #[test]
    fn registry_rejects_broad_or_unverifiable_identity_allowances() {
        let source = format!(
            "{}\n{}",
            registry_source(),
            identity_allowance_source(
                "allowance.technical",
                "technical_adapter",
                "crates/*/src/lib.rs",
                &["not-a-detector-token"],
                &"A".repeat(64),
            )
        );
        let registry = parse_generic_domain_registry(&source).unwrap();
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("invalid stable_path"));
        assert!(error.contains("unknown detector token"));
        assert!(error.contains("sha256 must be lowercase SHA-256"));
    }

    #[test]
    fn identity_branches_reject_unknown_concrete_values() {
        let source = r#"
            fn select(game: &str, profile: &str) -> bool {
                game == "unknown_project_code" || match profile {
                    "fixed_profile" => true,
                    _ => false,
                }
            }

        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert_eq!(violations.len(), 2);
        assert!(
            violations
                .iter()
                .any(|item| item.contains("unknown_project_code"))
        );
        assert!(violations.iter().any(|item| item.contains("fixed_profile")));
    }

    #[test]
    fn private_algorithm_helpers_are_not_source_word_dictionary_checked() {
        let source = r#"
            fn interpolate_weighted_samples(left: f64, right: f64) -> f64 {
                (left + right) / 2.0
            }
        "#;
        assert!(
            inspect_identity_axis_branches("fixture.rs", source)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn public_surface_inventory_changes_only_for_protected_semantics() {
        let original = r#"
            pub struct Summary { pub value: u64 }
            fn interpolate_weighted_samples(left: f64, right: f64) -> f64 { left + right }
        "#;
        let renamed_helper = r#"
            pub struct Summary { pub value: u64 }
            fn blend_observations(left: f64, right: f64) -> f64 { left + right }
        "#;
        let new_public_field = r#"
            pub struct Summary { pub value: u64, pub status: String }
            fn blend_observations(left: f64, right: f64) -> f64 { left + right }
        "#;

        let original = rust_surface_inventory("fixture.rs", original).unwrap();
        let renamed = rust_surface_inventory("fixture.rs", renamed_helper).unwrap();
        let expanded = rust_surface_inventory("fixture.rs", new_public_field).unwrap();
        assert_eq!(original, renamed);
        assert_ne!(original, expanded);
    }

    #[test]
    fn workspace_registry_rejects_unmapped_member_and_public_surface_drift() {
        let root = temporary_workspace("surface-drift");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Summary { pub value: u64 }\n",
        )
        .unwrap();
        create_required_roots(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let registry = registry_for_snapshots(&snapshots);
        validate_workspace_surface_registry(&root, &registry).unwrap();

        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Summary { pub value: u64, pub status: String }\n",
        )
        .unwrap();
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("crates/example fingerprint drifted"));

        fs::write(
            root.join("crates/example/src/lib.rs"),
            "#[arg(long, default_value = \"changed\")]\nstruct Cli;\n",
        )
        .unwrap();
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("crates/example fingerprint drifted"));

        fs::create_dir_all(root.join("crates/second/src")).unwrap();
        fs::write(root.join("crates/second/src/lib.rs"), "pub struct Added;\n").unwrap();
        write_workspace_manifest(&root, &["crates/example", "crates/second"]);
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface crates/second"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_genericity_scans_cfg_tests_private_helpers_and_fused_tokens() {
        let root = temporary_workspace("full-genericity");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            r#"
                #[cfg(test)]
                mod tests {
                    fn private_select(game: &str) -> bool {
                        game == "unknown_project_code"
                    }

                    fn BaasPvpLimit() {}
                }
            "#,
        )
        .unwrap();
        create_required_roots(&root);
        write_external_compat_manifest(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let registry = registry_for_snapshots(&snapshots);
        let error = validate_workspace_genericity(&root, &registry).unwrap_err();
        assert!(error.contains("unknown_project_code"));
        assert!(error.contains("project-specific word baas"));
        assert!(error.contains("project-specific word pvp"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_genericity_requires_exact_allowance_hash_and_registered_surface() {
        let root = temporary_workspace("identity-allowance");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        let path = "crates/example/src/lib.rs";
        fs::write(root.join(path), "fn compile_maa_tasks() {}\n").unwrap();
        create_required_roots(&root);
        write_external_compat_manifest(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let mut registry = registry_for_snapshots(&snapshots);
        let hash = format!("{:x}", Sha256::digest(fs::read(root.join(path)).unwrap()));
        registry.identity_allowance.push(IdentityAllowance {
            id: "allowance.technical".to_string(),
            kind: "technical_adapter".to_string(),
            exact_path: path.to_string(),
            tokens: vec!["maa".to_string()],
            sha256: hash,
            purpose: "Exact technical adapter boundary.".to_string(),
            approval_comment_id: 5010683904,
            source_issue: 44,
            source_pr: Some(111),
        });
        validate_generic_domain_registry(&registry).unwrap();
        validate_workspace_genericity(&root, &registry).unwrap();

        fs::write(root.join(path), "fn compile_maa_jobs() {}\n").unwrap();
        let error = validate_workspace_genericity(&root, &registry).unwrap_err();
        assert!(error.contains("content hash drifted"));

        let outside = "outside.rs";
        fs::write(root.join(outside), "fn compile_maa_tasks() {}\n").unwrap();
        let outside_hash = format!(
            "{:x}",
            Sha256::digest(fs::read(root.join(outside)).unwrap())
        );
        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let mut outside_registry = registry_for_snapshots(&snapshots);
        outside_registry.identity_allowance.push(IdentityAllowance {
            id: "allowance.outside".to_string(),
            kind: "technical_adapter".to_string(),
            exact_path: outside.to_string(),
            tokens: vec!["maa".to_string()],
            sha256: outside_hash,
            purpose: "Counterexample outside registered surfaces.".to_string(),
            approval_comment_id: 5010683904,
            source_issue: 44,
            source_pr: Some(111),
        });
        let error = validate_workspace_genericity(&root, &outside_registry).unwrap_err();
        assert!(error.contains("outside registered Runtime surfaces"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_registry_rejects_unregistered_schema_and_default_changes() {
        let root = temporary_workspace("schema-drift");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Summary;\n",
        )
        .unwrap();
        create_required_roots(&root);
        fs::write(
            root.join("contracts/example.schema.json"),
            r#"{"properties":{"game":{"type":"string"}}}"#,
        )
        .unwrap();

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let registry = registry_for_snapshots(&snapshots);
        validate_workspace_surface_registry(&root, &registry).unwrap();

        fs::write(
            root.join("contracts/example.schema.json"),
            r#"{"properties":{"game":{"type":"string","default":"synthetic_project_code"}}}"#,
        )
        .unwrap();
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("contracts fingerprint drifted"));

        fs::remove_dir_all(root).unwrap();
    }

    fn temporary_workspace(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "actingcommand-generic-domain-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_workspace_manifest(root: &Path, members: &[&str]) {
        let members = members
            .iter()
            .map(|member| format!("    {member:?},"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            root.join("Cargo.toml"),
            format!("[workspace]\nmembers = [\n{members}\n]\n"),
        )
        .unwrap();
    }

    fn create_required_roots(root: &Path) {
        for path in REQUIRED_PROTECTED_ROOTS {
            fs::create_dir_all(root.join(path)).unwrap();
        }
    }

    fn write_external_compat_manifest(root: &Path) {
        let path = root.join(EXTERNAL_COMPAT_MANIFEST_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            "schema_version = \"actingcommand.external-compat.v1\"\n",
        )
        .unwrap();
    }

    fn identity_allowance_source(
        id: &str,
        kind: &str,
        exact_path: &str,
        tokens: &[&str],
        sha256: &str,
    ) -> String {
        let tokens = tokens
            .iter()
            .map(|token| format!("{token:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"[[identity_allowance]]
id = {id:?}
kind = {kind:?}
exact_path = {exact_path:?}
tokens = [{tokens}]
sha256 = {sha256:?}
purpose = "Exact test allowance."
approval_comment_id = 5010683904
source_issue = 44
source_pr = 111
"#
        )
    }

    fn registry_for_snapshots(snapshots: &[SurfaceSnapshot]) -> GenericDomainRegistry {
        GenericDomainRegistry {
            schema_version: GENERIC_DOMAIN_SCHEMA_VERSION.to_string(),
            concept: vec![GenericConcept {
                id: "structure.value".to_string(),
                status: "active".to_string(),
                approval_comment_id: 5010683904,
                replaced_by: None,
            }],
            identity_allowance: Vec::new(),
            surface: snapshots
                .iter()
                .enumerate()
                .map(|(index, snapshot)| ProtectedSurface {
                    surface_id: format!("surface.item_{index:03}"),
                    kind: snapshot.kind.clone(),
                    stable_path: snapshot.stable_path.clone(),
                    concept_ids: vec!["structure.value".to_string()],
                    fingerprint: snapshot.fingerprint.clone(),
                    source_issue: 44,
                    source_pr: Some(108),
                })
                .collect(),
        }
    }
}
