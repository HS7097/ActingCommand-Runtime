// SPDX-License-Identifier: AGPL-3.0-only

//! Machine-readable generic-domain concepts and protected Runtime surfaces.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use quote::ToTokens;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use syn::parse::{Parse, ParseStream};
use syn::visit::Visit;
use syn::{
    Attribute, BinOp, Expr, ExprBinary, ExprMatch, Fields, Ident, ImplItem, Item, Lit, LitStr,
    Member, Token, Visibility, braced,
};

use crate::external_compat::{EXTERNAL_COMPAT_MANIFEST_PATH, load_and_validate_external_compat};
use crate::{inspect_generic_runtime_identity_with_allowances, known_project_identity_tokens};

pub const GENERIC_DOMAIN_SCHEMA_VERSION: &str = "actingcommand.generic-domain.v2";
pub const GENERIC_DOMAIN_REGISTRY_PATH: &str =
    "tools/actinglab-architecture/generic-domain-v2.toml";
pub const GENERIC_DOMAIN_SURFACE_SCHEMA_VERSION: &str = "actingcommand.generic-domain-surfaces.v2";
pub const GENERIC_DOMAIN_SURFACE_MANIFEST_PATH: &str =
    "tools/actinglab-architecture/generic-domain-surfaces-v2.jsonl";
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
    pub surface_manifest: Option<SurfaceManifestReference>,
    #[serde(default)]
    pub approval: Vec<SurfaceApproval>,
    #[serde(default)]
    pub concept: Vec<GenericConcept>,
    #[serde(default)]
    pub identity_allowance: Vec<IdentityAllowance>,
    #[serde(default)]
    pub surface: Vec<ProtectedSurface>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceManifestReference {
    pub path: String,
    pub sha256: String,
    pub approval_id: String,
    pub source_issue: u64,
    #[serde(default)]
    pub source_pr: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SurfaceManifestHeader {
    pub schema_version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceRecord {
    pub surface_id: String,
    pub kind: String,
    pub stable_path: String,
    pub selector: String,
    pub concept_ids: Vec<String>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceApproval {
    pub id: String,
    pub repository: String,
    pub issue: u64,
    pub comment_id: u64,
    pub author: String,
    pub content_sha256: String,
    pub scope: Vec<String>,
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
    pub selector: String,
    pub concept_ids: Vec<String>,
    pub fingerprint: String,
    pub approval_id: String,
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
    pub surface_id: String,
    pub kind: String,
    pub stable_path: String,
    pub selector: String,
    pub fingerprint: String,
}

pub fn parse_generic_domain_registry(source: &str) -> Result<GenericDomainRegistry, String> {
    toml::from_str(source).map_err(|error| format!("invalid generic-domain registry: {error}"))
}

pub fn load_generic_domain_registry(path: &Path) -> Result<GenericDomainRegistry, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut registry = parse_generic_domain_registry(&source)?;
    let Some(reference) = registry.surface_manifest.clone() else {
        return Ok(registry);
    };
    let workspace_root = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| format!("registry path {} is not under tools/<name>", path.display()))?;
    validate_stable_path(&reference.path)?;
    if reference.path != GENERIC_DOMAIN_SURFACE_MANIFEST_PATH {
        return Err(format!(
            "generic-domain registry references unexpected surface manifest {}",
            reference.path
        ));
    }
    let manifest_path = workspace_root.join(&reference.path);
    let bytes = fs::read(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != reference.sha256 {
        return Err(format!(
            "surface manifest hash drifted: registered {}, actual {actual}",
            reference.sha256
        ));
    }
    let source = std::str::from_utf8(&bytes)
        .map_err(|error| format!("surface manifest is not UTF-8: {error}"))?;
    let mut lines = source.lines();
    let header: SurfaceManifestHeader = serde_json::from_str(
        lines
            .next()
            .ok_or_else(|| "surface manifest is empty".to_string())?,
    )
    .map_err(|error| format!("invalid surface manifest header: {error}"))?;
    if header.schema_version != GENERIC_DOMAIN_SURFACE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported surface manifest schema_version {}; expected {GENERIC_DOMAIN_SURFACE_SCHEMA_VERSION}",
            header.schema_version
        ));
    }
    let mut records = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            return Err(format!(
                "surface manifest contains a blank record at line {}",
                index + 2
            ));
        }
        records.push(
            serde_json::from_str::<SurfaceRecord>(line).map_err(|error| {
                format!(
                    "invalid surface manifest record at line {}: {error}",
                    index + 2
                )
            })?,
        );
    }
    if !registry.surface.is_empty() {
        return Err("registry cannot combine inline and external surfaces".to_string());
    }
    registry.surface = records
        .into_iter()
        .map(|surface| ProtectedSurface {
            surface_id: surface.surface_id,
            kind: surface.kind,
            stable_path: surface.stable_path,
            selector: surface.selector,
            concept_ids: surface.concept_ids,
            fingerprint: surface.fingerprint,
            approval_id: reference.approval_id.clone(),
            source_issue: reference.source_issue,
            source_pr: reference.source_pr,
        })
        .collect();
    Ok(registry)
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
    if registry.approval.is_empty() {
        errors.push("generic-domain registry contains no surface approvals".to_string());
    }
    if registry.surface.is_empty() {
        errors.push("generic-domain registry contains no protected surfaces".to_string());
    }

    let mut approval_ids = HashSet::new();
    let mut previous_approval = None;
    for approval in &registry.approval {
        if !is_surface_id(&approval.id) {
            errors.push(format!("surface approval has invalid id {}", approval.id));
        }
        if !approval_ids.insert(approval.id.as_str()) {
            errors.push(format!("duplicate surface approval id {}", approval.id));
        }
        if previous_approval.is_some_and(|previous: &str| previous >= approval.id.as_str()) {
            errors.push(format!(
                "surface approval ids are not strictly sorted at {}",
                approval.id
            ));
        }
        previous_approval = Some(approval.id.as_str());
        if approval.repository != "HS7097/ActingCommand-Workflow" {
            errors.push(format!(
                "surface approval {} has untrusted repository {}",
                approval.id, approval.repository
            ));
        }
        if approval.author != "HS7097" {
            errors.push(format!(
                "surface approval {} has untrusted author {}",
                approval.id, approval.author
            ));
        }
        if approval.issue == 0 || approval.comment_id == 0 {
            errors.push(format!(
                "surface approval {} has no issue/comment source",
                approval.id
            ));
        }
        if !is_sha256(&approval.content_sha256) {
            errors.push(format!(
                "surface approval {} content_sha256 must be lowercase SHA-256",
                approval.id
            ));
        }
        if approval.scope.is_empty() {
            errors.push(format!("surface approval {} has no scope", approval.id));
        }
        let mut scopes = HashSet::new();
        let mut previous_scope = None;
        for scope in &approval.scope {
            if !is_surface_id(scope) {
                errors.push(format!(
                    "surface approval {} has invalid scope {scope}",
                    approval.id
                ));
            }
            if !scopes.insert(scope.as_str()) {
                errors.push(format!(
                    "surface approval {} repeats scope {scope}",
                    approval.id
                ));
            }
            if previous_scope.is_some_and(|previous: &str| previous >= scope.as_str()) {
                errors.push(format!(
                    "surface approval {} scopes are not strictly sorted at {scope}",
                    approval.id
                ));
            }
            previous_scope = Some(scope.as_str());
        }
        if !scopes.contains("surface.mapping") {
            errors.push(format!(
                "surface approval {} does not authorize surface.mapping",
                approval.id
            ));
        }
    }
    if let Some(reference) = &registry.surface_manifest {
        if reference.path != GENERIC_DOMAIN_SURFACE_MANIFEST_PATH {
            errors.push(format!(
                "surface manifest has unexpected path {}",
                reference.path
            ));
        }
        if !is_sha256(&reference.sha256) {
            errors.push("surface manifest sha256 must be lowercase SHA-256".to_string());
        }
        if !approval_ids.contains(reference.approval_id.as_str()) {
            errors.push(format!(
                "surface manifest references unknown approval {}",
                reference.approval_id
            ));
        }
        if reference.source_issue == 0 || reference.source_pr == Some(0) {
            errors.push("surface manifest has invalid source issue/PR".to_string());
        }
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
    let mut surface_keys = HashSet::new();
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
        if !matches!(
            surface.kind.as_str(),
            "rust_public_item"
                | "rust_wire_item"
                | "rust_public_field"
                | "rust_wire_field"
                | "rust_public_variant"
                | "rust_wire_variant"
                | "rust_public_impl_item"
                | "rust_wire_attribute"
                | "rust_cli_attribute"
                | "rust_match_literal"
                | "rust_macro_item"
                | "rust_macro_variant"
                | "rust_macro_wire_value"
                | "structured_key"
                | "structured_value"
                | "text_record"
        ) {
            errors.push(format!(
                "surface {} has invalid kind {}",
                surface.surface_id, surface.kind
            ));
        }
        if let Err(error) = validate_stable_path(&surface.stable_path) {
            errors.push(format!("surface {} {error}", surface.surface_id));
        }
        if surface.selector.trim().is_empty() {
            errors.push(format!(
                "surface {} has an empty selector",
                surface.surface_id
            ));
        }
        if !surface_keys.insert((
            surface.kind.as_str(),
            surface.stable_path.as_str(),
            surface.selector.as_str(),
        )) {
            errors.push(format!(
                "duplicate protected surface {} {} {}",
                surface.kind, surface.stable_path, surface.selector
            ));
        }
        let expected_surface_id =
            surface_id_for(&surface.kind, &surface.stable_path, &surface.selector);
        if surface.surface_id != expected_surface_id {
            errors.push(format!(
                "surface {} has unstable id; expected {expected_surface_id}",
                surface.surface_id
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
        if !approval_ids.contains(surface.approval_id.as_str()) {
            errors.push(format!(
                "surface {} references unknown approval {}",
                surface.surface_id, surface.approval_id
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
        collect_protected_files(&root.join(stable_path), &mut files)?;
    }
    files.sort();
    files.dedup();

    let mut snapshots = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped workspace root", file.display()))?;
        let relative = normalize_path(relative)?;
        if relative == GENERIC_DOMAIN_REGISTRY_PATH
            || relative == GENERIC_DOMAIN_SURFACE_MANIFEST_PATH
        {
            continue;
        }
        snapshots.extend(snapshot_for_file(&file, &relative)?);
    }
    snapshots.sort_by(|left, right| left.surface_id.cmp(&right.surface_id));
    if snapshots
        .windows(2)
        .any(|pair| pair[0].surface_id == pair[1].surface_id)
    {
        return Err("surface inventory produced duplicate stable ids".to_string());
    }
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
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<HashMap<_, _>>();
    let expected_ids = expected
        .iter()
        .map(|snapshot| snapshot.surface_id.clone())
        .collect::<HashSet<_>>();
    let mut errors = Vec::new();

    for snapshot in &expected {
        let Some(surface) = registered.get(snapshot.surface_id.as_str()) else {
            errors.push(format!(
                "unmapped protected surface {} {} {} ({})",
                snapshot.kind, snapshot.stable_path, snapshot.selector, snapshot.surface_id
            ));
            continue;
        };
        if surface.kind != snapshot.kind
            || surface.stable_path != snapshot.stable_path
            || surface.selector != snapshot.selector
        {
            errors.push(format!(
                "surface {} identity drifted: registered {} {} {}, actual {} {} {}",
                snapshot.surface_id,
                surface.kind,
                surface.stable_path,
                surface.selector,
                snapshot.kind,
                snapshot.stable_path,
                snapshot.selector
            ));
        }
        if surface.fingerprint != snapshot.fingerprint {
            errors.push(format!(
                "surface {} fingerprint drifted at {} {}: registered {}, actual {}",
                snapshot.surface_id,
                snapshot.stable_path,
                snapshot.selector,
                surface.fingerprint,
                snapshot.fingerprint
            ));
        }
    }
    for surface in &registry.surface {
        if !expected_ids.contains(&surface.surface_id) {
            errors.push(format!(
                "registered surface {} no longer exists at {} {}",
                surface.surface_id, surface.stable_path, surface.selector
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
            || relative == GENERIC_DOMAIN_SURFACE_MANIFEST_PATH
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSurface {
    kind: &'static str,
    selector: String,
    content: String,
}

fn snapshot_for_file(path: &Path, stable_path: &str) -> Result<Vec<SurfaceSnapshot>, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let raw = match extension {
        "rs" => rust_surface_inventory(stable_path, &source)?,
        "json" => structured_json_inventory(stable_path, &source)?,
        "toml" => structured_toml_inventory(stable_path, &source)?,
        _ => text_surface_inventory(&source),
    };
    Ok(raw
        .into_iter()
        .map(|item| {
            let surface_id = surface_id_for(item.kind, stable_path, &item.selector);
            let fingerprint = format!("{:x}", Sha256::digest(item.content.as_bytes()));
            SurfaceSnapshot {
                surface_id,
                kind: item.kind.to_string(),
                stable_path: stable_path.to_string(),
                selector: item.selector,
                fingerprint,
            }
        })
        .collect())
}

fn surface_id_for(kind: &str, stable_path: &str, selector: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{kind}\0{stable_path}\0{selector}").as_bytes())
    );
    format!("surface.{}", &digest[..40])
}

fn rust_surface_inventory(path: &str, source: &str) -> Result<Vec<RawSurface>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse {path}: {error}"))?;
    let mut collector = RustSurfaceCollector::default();
    collector.collect_items(&file.items, &[])?;
    let mut match_collector = MatchLiteralCollector::default();
    match_collector.visit_file(&file);
    collector.items.extend(match_collector.items);
    collector.items.sort_by(|left, right| {
        (left.kind, left.selector.as_str(), left.content.as_str()).cmp(&(
            right.kind,
            right.selector.as_str(),
            right.content.as_str(),
        ))
    });
    collector.items.dedup();
    Ok(collector.items)
}

#[derive(Default)]
struct RustSurfaceCollector {
    items: Vec<RawSurface>,
}

impl RustSurfaceCollector {
    fn collect_items(&mut self, items: &[Item], module: &[String]) -> Result<(), String> {
        for item in items {
            match item {
                Item::Const(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(module, &format!("const:{}", item.ident)),
                    item.to_token_stream(),
                ),
                Item::Enum(item) => {
                    let owner = qualified(module, &item.ident.to_string());
                    let wire = is_wire_type(&item.attrs);
                    self.attributes(&owner, &item.attrs);
                    if is_public(&item.vis) || wire {
                        let visibility = &item.vis;
                        let identifier = &item.ident;
                        let generics = &item.generics;
                        self.push(
                            if is_public(&item.vis) {
                                "rust_public_item"
                            } else {
                                "rust_wire_item"
                            },
                            format!("enum:{owner}"),
                            quote::quote!(#visibility enum #identifier #generics),
                        );
                        for variant in &item.variants {
                            let kind = if is_public(&item.vis) {
                                "rust_public_variant"
                            } else {
                                "rust_wire_variant"
                            };
                            let selector = format!("variant:{owner}::{}", variant.ident);
                            self.push(kind, selector.clone(), variant.to_token_stream());
                            self.attributes(&selector, &variant.attrs);
                            self.fields(&selector, &variant.fields, true);
                        }
                    }
                }
                Item::ExternCrate(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(module, &format!("extern:{}", item.ident)),
                    item.to_token_stream(),
                ),
                Item::Fn(item) => {
                    let owner = qualified(module, &format!("fn:{}", item.sig.ident));
                    self.attributes(&owner, &item.attrs);
                    if is_public(&item.vis) {
                        self.push("rust_public_item", owner, item.sig.to_token_stream());
                    }
                }
                Item::Macro(item) => self.macro_item(item, module)?,
                Item::Mod(item) => {
                    let owner = qualified(module, &format!("mod:{}", item.ident));
                    self.attributes(&owner, &item.attrs);
                    if is_public(&item.vis) {
                        self.push("rust_public_item", owner, item.ident.to_token_stream());
                    }
                    if let Some((_, nested)) = &item.content {
                        let mut next = module.to_vec();
                        next.push(item.ident.to_string());
                        self.collect_items(nested, &next)?;
                    }
                }
                Item::Static(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(module, &format!("static:{}", item.ident)),
                    item.to_token_stream(),
                ),
                Item::Struct(item) => {
                    let owner = qualified(module, &item.ident.to_string());
                    let wire = is_wire_type(&item.attrs);
                    self.attributes(&owner, &item.attrs);
                    if is_public(&item.vis) || wire {
                        let visibility = &item.vis;
                        let identifier = &item.ident;
                        let generics = &item.generics;
                        self.push(
                            if is_public(&item.vis) {
                                "rust_public_item"
                            } else {
                                "rust_wire_item"
                            },
                            format!("struct:{owner}"),
                            quote::quote!(#visibility struct #identifier #generics),
                        );
                        self.fields(&owner, &item.fields, wire);
                    }
                }
                Item::Trait(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(module, &format!("trait:{}", item.ident)),
                    item.to_token_stream(),
                ),
                Item::TraitAlias(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(module, &format!("trait_alias:{}", item.ident)),
                    item.to_token_stream(),
                ),
                Item::Type(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(module, &format!("type:{}", item.ident)),
                    item.to_token_stream(),
                ),
                Item::Union(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(module, &format!("union:{}", item.ident)),
                    item.to_token_stream(),
                ),
                Item::Use(item) if is_public(&item.vis) => self.push(
                    "rust_public_item",
                    qualified(
                        module,
                        &format!("use:{}", short_hash(&item.to_token_stream().to_string())),
                    ),
                    item.to_token_stream(),
                ),
                Item::Impl(item) => {
                    let owner = qualified(module, &item.self_ty.to_token_stream().to_string());
                    self.attributes(&owner, &item.attrs);
                    for member in &item.items {
                        match member {
                            ImplItem::Const(member) if is_public(&member.vis) => self.push(
                                "rust_public_impl_item",
                                format!("impl_const:{owner}::{}", member.ident),
                                member.to_token_stream(),
                            ),
                            ImplItem::Fn(member) if is_public(&member.vis) => self.push(
                                "rust_public_impl_item",
                                format!("impl_fn:{owner}::{}", member.sig.ident),
                                member.sig.to_token_stream(),
                            ),
                            ImplItem::Type(member) if is_public(&member.vis) => self.push(
                                "rust_public_impl_item",
                                format!("impl_type:{owner}::{}", member.ident),
                                member.to_token_stream(),
                            ),
                            _ => {}
                        }
                    }
                }
                _ => self.attributes(
                    &qualified(module, &format!("item: {}", item.to_token_stream())),
                    item_attrs(item),
                ),
            }
        }
        Ok(())
    }

    fn fields(&mut self, owner: &str, fields: &Fields, wire: bool) {
        for (index, field) in fields.iter().enumerate() {
            let name = field
                .ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| index.to_string());
            let selector = format!("field:{owner}::{name}");
            let kind = if wire && !is_public(&field.vis) {
                "rust_wire_field"
            } else {
                "rust_public_field"
            };
            self.push(kind, selector.clone(), field.to_token_stream());
            self.attributes(&selector, &field.attrs);
        }
    }

    fn attributes(&mut self, owner: &str, attributes: &[Attribute]) {
        let mut ordinal = HashMap::<String, usize>::new();
        for attribute in attributes {
            let Some(name) = attribute
                .path()
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            else {
                continue;
            };
            let kind = match name.as_str() {
                "serde" | "value" => "rust_wire_attribute",
                "arg" | "clap" | "command" => "rust_cli_attribute",
                _ => continue,
            };
            let index = ordinal.entry(name.clone()).or_default();
            self.push(
                kind,
                format!("attribute:{owner}:{name}:{}", *index),
                attribute.to_token_stream(),
            );
            *index += 1;
        }
    }

    fn macro_item(&mut self, item: &syn::ItemMacro, module: &[String]) -> Result<(), String> {
        let name = item
            .mac
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        if name == "closed_code" {
            let definition = syn::parse2::<ClosedCodeDefinition>(item.mac.tokens.clone())
                .map_err(|error| format!("invalid closed_code invocation: {error}"))?;
            let owner = qualified(module, &definition.name.to_string());
            self.push(
                "rust_macro_item",
                format!("macro_enum:{owner}"),
                definition.name.to_token_stream(),
            );
            for variant in definition.variant {
                let selector = format!("macro_variant:{owner}::{}", variant.name);
                self.push(
                    "rust_macro_variant",
                    selector,
                    variant.name.to_token_stream(),
                );
                self.push(
                    "rust_macro_wire_value",
                    format!("macro_wire:{owner}::{}", variant.name),
                    variant.wire.to_token_stream(),
                );
            }
        } else if item
            .attrs
            .iter()
            .any(|attribute| attribute.path().is_ident("macro_export"))
        {
            self.push(
                "rust_macro_item",
                qualified(module, &format!("macro:{name}")),
                item.to_token_stream(),
            );
        }
        Ok(())
    }

    fn push(&mut self, kind: &'static str, selector: String, content: impl ToTokens) {
        self.items.push(RawSurface {
            kind,
            selector,
            content: content.to_token_stream().to_string(),
        });
    }
}

fn qualified(module: &[String], name: &str) -> String {
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{}::{name}", module.join("::"))
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn is_wire_type(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("serde")
            || (attribute.path().is_ident("derive") && {
                let tokens = attribute.to_token_stream().to_string();
                tokens.contains("Serialize") || tokens.contains("Deserialize")
            })
    })
}

fn short_hash(value: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(value.as_bytes()));
    digest[..16].to_string()
}

struct ClosedCodeDefinition {
    name: Ident,
    variant: Vec<ClosedCodeVariant>,
}

struct ClosedCodeVariant {
    name: Ident,
    wire: LitStr,
}

impl Parse for ClosedCodeDefinition {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name = input.parse()?;
        let content;
        braced!(content in input);
        let mut variant = Vec::new();
        while !content.is_empty() {
            variant.push(ClosedCodeVariant {
                name: content.parse()?,
                wire: {
                    content.parse::<Token![=>]>()?;
                    content.parse()?
                },
            });
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
        Ok(Self { name, variant })
    }
}

#[derive(Default)]
struct MatchLiteralCollector {
    items: Vec<RawSurface>,
    ordinal: usize,
}

impl Visit<'_> for MatchLiteralCollector {
    fn visit_expr_match(&mut self, node: &ExprMatch) {
        for arm in &node.arms {
            let mut strings = PatternStringVisitor::default();
            strings.visit_pat(&arm.pat);
            for value in strings.values {
                self.items.push(RawSurface {
                    kind: "rust_match_literal",
                    selector: format!("match_literal:{:06}", self.ordinal),
                    content: value,
                });
                self.ordinal += 1;
            }
        }
        syn::visit::visit_expr_match(self, node);
    }
}

fn structured_json_inventory(path: &str, source: &str) -> Result<Vec<RawSurface>, String> {
    let value: serde_json::Value = serde_json::from_str(source)
        .map_err(|error| format!("failed to parse protected JSON {path}: {error}"))?;
    let mut items = Vec::new();
    collect_structured_value(&value, "", &mut items)?;
    Ok(items)
}

fn structured_toml_inventory(path: &str, source: &str) -> Result<Vec<RawSurface>, String> {
    let value: toml::Value = toml::from_str(source)
        .map_err(|error| format!("failed to parse protected TOML {path}: {error}"))?;
    let value = serde_json::to_value(value)
        .map_err(|error| format!("failed to normalize protected TOML {path}: {error}"))?;
    let mut items = Vec::new();
    collect_structured_value(&value, "", &mut items)?;
    Ok(items)
}

fn collect_structured_value(
    value: &serde_json::Value,
    pointer: &str,
    items: &mut Vec<RawSurface>,
) -> Result<(), String> {
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                items.push(RawSurface {
                    kind: "structured_value",
                    selector: format!("value:{pointer}"),
                    content: "{}".to_string(),
                });
            }
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (key, child) in entries {
                let child_pointer = format!("{pointer}/{}", escape_json_pointer(key));
                items.push(RawSurface {
                    kind: "structured_key",
                    selector: format!("key:{child_pointer}"),
                    content: key.clone(),
                });
                collect_structured_value(child, &child_pointer, items)?;
            }
        }
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                items.push(RawSurface {
                    kind: "structured_value",
                    selector: format!("value:{pointer}"),
                    content: "[]".to_string(),
                });
            }
            for (index, child) in values.iter().enumerate() {
                collect_structured_value(child, &format!("{pointer}/{index}"), items)?;
            }
        }
        _ => items.push(RawSurface {
            kind: "structured_value",
            selector: format!("value:{pointer}"),
            content: serde_json::to_string(value)
                .map_err(|error| format!("failed to normalize structured value: {error}"))?,
        }),
    }
    Ok(())
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn text_surface_inventory(source: &str) -> Vec<RawSurface> {
    source
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| RawSurface {
            kind: "text_record",
            selector: format!("line:{:06}", index + 1),
            content: line.to_string(),
        })
        .collect()
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
        let surface_id = surface_id_for(
            "rust_public_item",
            "crates/example/src/lib.rs",
            "struct:Summary",
        );
        format!(
            r#"
schema_version = "actingcommand.generic-domain.v2"

[[approval]]
id = "approval.issue44_r8"
repository = "HS7097/ActingCommand-Workflow"
issue = 54
comment_id = 5011264343
author = "HS7097"
content_sha256 = "{}"
scope = ["surface.mapping"]

[[concept]]
id = "identity.game"
status = "active"
approval_comment_id = 5010683904

[[concept]]
id = "structure.value"
status = "active"
approval_comment_id = 5010683904

[[surface]]
surface_id = "{surface_id}"
kind = "rust_public_item"
stable_path = "crates/example/src/lib.rs"
selector = "struct:Summary"
concept_ids = ["identity.game", "structure.value"]
fingerprint = "{}"
approval_id = "approval.issue44_r8"
source_issue = 44
source_pr = 108
"#,
            "a".repeat(64),
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
            .replace("crates/example/src/lib.rs", "crates/*/src/lib.rs");
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
        assert!(error.contains("unmapped protected surface rust_public_field"));
        assert!(error.contains("field:Summary::status"));

        fs::write(
            root.join("crates/example/src/lib.rs"),
            "#[arg(long, default_value = \"changed\")]\nstruct Cli;\n",
        )
        .unwrap();
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface rust_cli_attribute"));
        assert!(error.contains("registered surface"));

        fs::create_dir_all(root.join("crates/second/src")).unwrap();
        fs::write(root.join("crates/second/src/lib.rs"), "pub struct Added;\n").unwrap();
        write_workspace_manifest(&root, &["crates/example", "crates/second"]);
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface rust_public_item crates/second"));

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
        assert!(error.contains("unmapped protected surface structured_key"));
        assert!(error.contains("/properties/game/default"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_registry_rejects_unregistered_closed_code_variant_and_wire_value() {
        let root = temporary_workspace("closed-code-drift");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        let path = root.join("crates/example/src/lib.rs");
        fs::write(
            &path,
            r#"closed_code!(Status { Ready => "ready" });
"#,
        )
        .unwrap();
        create_required_roots(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let registry = registry_for_snapshots(&snapshots);
        validate_workspace_surface_registry(&root, &registry).unwrap();

        fs::write(
            &path,
            r#"closed_code!(Status {
    Ready => "ready",
    SyntheticFaction => "synthetic.faction",
});
"#,
        )
        .unwrap();
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface rust_macro_variant"));
        assert!(error.contains("macro_variant:Status::SyntheticFaction"));
        assert!(error.contains("unmapped protected surface rust_macro_wire_value"));
        assert!(error.contains("macro_wire:Status::SyntheticFaction"));

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
            surface_manifest: None,
            approval: vec![SurfaceApproval {
                id: "approval.issue44_r8".to_string(),
                repository: "HS7097/ActingCommand-Workflow".to_string(),
                issue: 54,
                comment_id: 5011264343,
                author: "HS7097".to_string(),
                content_sha256: "a".repeat(64),
                scope: vec!["surface.mapping".to_string()],
            }],
            concept: vec![GenericConcept {
                id: "structure.value".to_string(),
                status: "active".to_string(),
                approval_comment_id: 5010683904,
                replaced_by: None,
            }],
            identity_allowance: Vec::new(),
            surface: snapshots
                .iter()
                .map(|snapshot| ProtectedSurface {
                    surface_id: snapshot.surface_id.clone(),
                    kind: snapshot.kind.clone(),
                    stable_path: snapshot.stable_path.clone(),
                    selector: snapshot.selector.clone(),
                    concept_ids: vec!["structure.value".to_string()],
                    fingerprint: snapshot.fingerprint.clone(),
                    approval_id: "approval.issue44_r8".to_string(),
                    source_issue: 44,
                    source_pr: Some(108),
                })
                .collect(),
        }
    }
}
