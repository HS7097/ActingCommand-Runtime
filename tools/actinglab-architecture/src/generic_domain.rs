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
use crate::{
    inspect_generic_runtime_identity, inspect_generic_runtime_identity_with_allowances,
    known_project_identity_tokens,
};

pub const GENERIC_DOMAIN_SCHEMA_VERSION: &str = "actingcommand.generic-domain.v2";
pub const GENERIC_DOMAIN_REGISTRY_PATH: &str =
    "tools/actinglab-architecture/generic-domain-v2.toml";
pub const GENERIC_DOMAIN_SURFACE_SCHEMA_VERSION: &str = "actingcommand.generic-domain-surfaces.v2";
pub const GENERIC_DOMAIN_SURFACE_MANIFEST_PATH: &str =
    "tools/actinglab-architecture/generic-domain-surfaces-v2.jsonl";
pub const REQUIRED_PROTECTED_ROOTS: &[&str] = &[
    ".github",
    "benchmarks/workloads",
    "contracts",
    "ratchet",
    "resources",
    "scripts",
    "tests",
];
const WORKSPACE_PACKAGE_ROOTS: &[&str] = &["apps", "benchmarks", "crates", "providers", "tools"];

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
    #[serde(default)]
    pub approval_id: Option<String>,
    #[serde(default)]
    pub source_issue: Option<u64>,
    #[serde(default)]
    pub source_pr: Option<u64>,
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
    pub selector: String,
    pub scope: Vec<String>,
    pub tokens: Vec<String>,
    pub sha256: String,
    pub purpose: String,
    pub approval_id: String,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IdentityAllowanceCandidate {
    pub stable_path: String,
    pub selector: String,
    pub fingerprint: String,
    pub detector_tokens: Vec<String>,
    pub branch_violations: Vec<String>,
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
            approval_id: surface
                .approval_id
                .unwrap_or_else(|| reference.approval_id.clone()),
            source_issue: surface.source_issue.unwrap_or(reference.source_issue),
            source_pr: surface.source_pr.or(reference.source_pr),
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
    }
    let approval_by_id = registry
        .approval
        .iter()
        .map(|approval| (approval.id.as_str(), approval))
        .collect::<HashMap<_, _>>();
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
        if approval_by_id
            .get(reference.approval_id.as_str())
            .is_some_and(|approval| {
                !approval
                    .scope
                    .iter()
                    .any(|scope| scope == "surface.mapping")
            })
        {
            errors.push(format!(
                "surface manifest approval {} does not authorize surface.mapping",
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
    let mut allowance_targets = HashSet::new();
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
            "documentation"
                | "guard_fixture"
                | "guard_source"
                | "technical_adapter"
                | "test_fixture"
                | "upstream_metadata"
        ) {
            errors.push(format!(
                "identity allowance {} has invalid kind {}",
                allowance.id, allowance.kind
            ));
        }
        if allowance.kind == "documentation"
            && !(allowance.exact_path.ends_with(".md")
                || allowance.exact_path == "LICENSE"
                || allowance.exact_path == "NOTICE")
        {
            errors.push(format!(
                "identity allowance {} documentation target is not a documentation file",
                allowance.id
            ));
        }
        if let Err(error) = validate_stable_path(&allowance.exact_path) {
            errors.push(format!("identity allowance {} {error}", allowance.id));
        }
        if allowance.kind == "guard_fixture"
            && !allowance
                .exact_path
                .starts_with("tools/actinglab-architecture/tests/")
        {
            errors.push(format!(
                "identity allowance {} guard_fixture is outside the guard test directory",
                allowance.id
            ));
        }
        if allowance.kind == "guard_source"
            && !allowance
                .exact_path
                .starts_with("tools/actinglab-architecture/src/")
        {
            errors.push(format!(
                "identity allowance {} guard_source is outside the guard source directory",
                allowance.id
            ));
        }
        if allowance.kind == "test_fixture"
            && !is_test_identity_fragment(&allowance.exact_path, &allowance.selector)
        {
            errors.push(format!(
                "identity allowance {} test_fixture does not target a test fragment",
                allowance.id
            ));
        }
        if allowance.selector.trim().is_empty()
            || allowance.selector.contains('*')
            || allowance.selector.contains('?')
        {
            errors.push(format!(
                "identity allowance {} has invalid selector {}",
                allowance.id, allowance.selector
            ));
        }
        if !allowance_targets.insert((allowance.exact_path.as_str(), allowance.selector.as_str())) {
            errors.push(format!(
                "duplicate identity allowance target {} {}",
                allowance.exact_path, allowance.selector
            ));
        }
        if allowance.scope.is_empty() {
            errors.push(format!("identity allowance {} has no scope", allowance.id));
        }
        let mut previous_scope = None;
        let mut scopes = HashSet::new();
        for scope in &allowance.scope {
            if !matches!(scope.as_str(), "identity.branch" | "identity.token") {
                errors.push(format!(
                    "identity allowance {} has invalid scope {scope}",
                    allowance.id
                ));
            }
            if !scopes.insert(scope.as_str()) {
                errors.push(format!(
                    "identity allowance {} repeats scope {scope}",
                    allowance.id
                ));
            }
            if previous_scope.is_some_and(|previous: &str| previous >= scope.as_str()) {
                errors.push(format!(
                    "identity allowance {} scopes are not strictly sorted at {scope}",
                    allowance.id
                ));
            }
            previous_scope = Some(scope.as_str());
        }
        if allowance.tokens.is_empty() && scopes.contains("identity.token") {
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
        let allowance_approval = approval_by_id.get(allowance.approval_id.as_str());
        if allowance_approval.is_none() {
            errors.push(format!(
                "identity allowance {} references unknown approval {}",
                allowance.id, allowance.approval_id
            ));
        } else if allowance_approval.is_some_and(|approval| {
            !approval
                .scope
                .iter()
                .any(|scope| scope == "identity.allowance")
        }) {
            errors.push(format!(
                "identity allowance {} approval {} does not authorize identity.allowance",
                allowance.id, allowance.approval_id
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
        if approval_by_id
            .get(surface.approval_id.as_str())
            .is_some_and(|approval| {
                !approval
                    .scope
                    .iter()
                    .any(|scope| scope == "surface.mapping")
            })
        {
            errors.push(format!(
                "surface {} approval {} does not authorize surface.mapping",
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
    let files = protected_files(root)?;

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

pub fn workspace_identity_allowance_candidates(
    root: &Path,
) -> Result<Vec<IdentityAllowanceCandidate>, String> {
    let external = load_and_validate_external_compat(root)?;
    let external_paths = external
        .entry
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<HashSet<_>>();
    let files = protected_files(root)?;

    let mut candidates = Vec::new();
    for file in files {
        let relative = normalize_path(
            file.strip_prefix(root)
                .map_err(|_| format!("{} escaped workspace root", file.display()))?,
        )?;
        if relative == GENERIC_DOMAIN_REGISTRY_PATH
            || relative == GENERIC_DOMAIN_SURFACE_MANIFEST_PATH
            || relative == EXTERNAL_COMPAT_MANIFEST_PATH
            || external_paths.contains(relative.as_str())
        {
            continue;
        }
        for fragment in identity_fragments_for_file(&file, &relative)? {
            let label = format!("{}#{}", relative, fragment.selector);
            let mut detector_tokens = inspect_generic_runtime_identity(&label, &fragment.content)
                .into_iter()
                .filter_map(|violation| {
                    violation.split_whitespace().last().map(ToString::to_string)
                })
                .collect::<Vec<_>>();
            detector_tokens.sort();
            detector_tokens.dedup();
            let branch_violations = if file.extension().is_some_and(|extension| extension == "rs") {
                inspect_identity_axis_branches(&label, &fragment.content)?
            } else {
                Vec::new()
            };
            if detector_tokens.is_empty() && branch_violations.is_empty() {
                continue;
            }
            candidates.push(IdentityAllowanceCandidate {
                stable_path: relative.clone(),
                selector: fragment.selector,
                fingerprint: format!("{:x}", Sha256::digest(fragment.content.as_bytes())),
                detector_tokens,
                branch_violations,
            });
        }
    }
    candidates.sort_by(|left, right| {
        (&left.stable_path, &left.selector).cmp(&(&right.stable_path, &right.selector))
    });
    Ok(candidates)
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
    let allowance_by_fragment = validate_identity_allowance_fragments(root, registry)?;

    let files = protected_files(root)?;

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
        for fragment in identity_fragments_for_file(&file, &relative)? {
            let key = (relative.clone(), fragment.selector.clone());
            let allowance = allowance_by_fragment.get(&key).copied();
            let allowed_tokens = allowance
                .filter(|allowance| {
                    allowance
                        .scope
                        .iter()
                        .any(|scope| scope == "identity.token")
                })
                .map_or_else(HashSet::new, |allowance| {
                    allowance.tokens.iter().cloned().collect()
                });
            let label = format!("{}#{}", relative, fragment.selector);
            errors.extend(inspect_generic_runtime_identity_with_allowances(
                &label,
                &fragment.content,
                &allowed_tokens,
            ));
            let branch_allowed = allowance.is_some_and(|allowance| {
                allowance
                    .scope
                    .iter()
                    .any(|scope| scope == "identity.branch")
            });
            if file.extension().is_some_and(|extension| extension == "rs") && !branch_allowed {
                errors.extend(inspect_identity_axis_branches(&label, &fragment.content)?);
            }
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

fn validate_identity_allowance_fragments<'a>(
    root: &Path,
    registry: &'a GenericDomainRegistry,
) -> Result<HashMap<(String, String), &'a IdentityAllowance>, String> {
    let mut errors = Vec::new();
    let mut by_fragment = HashMap::new();
    let mut fragment_cache = HashMap::<String, Vec<IdentityFragment>>::new();
    for allowance in &registry.identity_allowance {
        match resolve_exact_regular_file(root, &allowance.exact_path) {
            Ok(path) => {
                let fragments = if let Some(fragments) = fragment_cache.get(&allowance.exact_path) {
                    fragments
                } else {
                    match identity_fragments_for_file(&path, &allowance.exact_path) {
                        Ok(fragments) => {
                            fragment_cache.insert(allowance.exact_path.clone(), fragments);
                            fragment_cache
                                .get(&allowance.exact_path)
                                .expect("inserted fragment inventory")
                        }
                        Err(error) => {
                            errors.push(format!("identity allowance {} {error}", allowance.id));
                            continue;
                        }
                    }
                };
                let Some(fragment) = fragments
                    .iter()
                    .find(|fragment| fragment.selector == allowance.selector)
                else {
                    errors.push(format!(
                        "identity allowance {} references missing selector {} in {}",
                        allowance.id, allowance.selector, allowance.exact_path
                    ));
                    continue;
                };
                let actual = format!("{:x}", Sha256::digest(fragment.content.as_bytes()));
                if actual != allowance.sha256 {
                    errors.push(format!(
                        "identity allowance {} fragment hash drifted: registered {}, actual {actual}",
                        allowance.id, allowance.sha256
                    ));
                }
                by_fragment.insert(
                    (allowance.exact_path.clone(), allowance.selector.clone()),
                    allowance,
                );
            }
            Err(error) => errors.push(format!("identity allowance {} {error}", allowance.id)),
        }
    }
    if errors.is_empty() {
        Ok(by_fragment)
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
        errors: Vec::new(),
        aliases: vec![HashMap::new()],
        collections: vec![HashMap::new()],
        return_axis: None,
    };
    visitor.visit_file(&file);
    if !visitor.errors.is_empty() {
        visitor.errors.sort();
        return Err(visitor.errors.join("\n"));
    }
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
    let workspace = manifest
        .get("workspace")
        .ok_or_else(|| "workspace table is missing".to_string())?;
    let members = workspace
        .get("members")
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
    let excluded = workspace
        .get("exclude")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| "workspace exclude must be an array".to_string())?
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .ok_or_else(|| "workspace exclude entry must be a string".to_string())
                        .and_then(|entry| {
                            validate_stable_path(entry)?;
                            Ok(entry.to_string())
                        })
                })
                .collect::<Result<HashSet<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let declared = result.iter().cloned().collect::<HashSet<_>>();
    let discovered = discover_workspace_packages(root)?;
    let mut errors = Vec::new();
    for package in &discovered {
        if !declared.contains(package) && !excluded.contains(package) {
            errors.push(format!(
                "workspace package manifest is not declared or excluded: {package}"
            ));
        }
    }
    for member in &result {
        if !discovered.contains(member) {
            errors.push(format!(
                "workspace member has no discoverable Cargo.toml: {member}"
            ));
        }
    }
    if !errors.is_empty() {
        errors.sort();
        return Err(errors.join("\n"));
    }
    Ok(result)
}

fn discover_workspace_packages(root: &Path) -> Result<HashSet<String>, String> {
    let mut packages = HashSet::new();
    for package_root in WORKSPACE_PACKAGE_ROOTS {
        let path = root.join(package_root);
        if !path.exists() {
            continue;
        }
        collect_package_manifests(root, &path, &mut packages)?;
    }
    Ok(packages)
}

fn collect_package_manifests(
    workspace_root: &Path,
    directory: &Path,
    packages: &mut HashSet<String>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("failed to read directory {}: {error}", directory.display()))?
    {
        let path = entry
            .map_err(|error| format!("failed to read directory entry: {error}"))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "workspace package discovery encountered a symlink or reparse point: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == "target" || name == ".git")
            {
                continue;
            }
            collect_package_manifests(workspace_root, &path, packages)?;
        } else if path.file_name().is_some_and(|name| name == "Cargo.toml") {
            let parent = path
                .parent()
                .ok_or_else(|| format!("package manifest has no parent: {}", path.display()))?;
            let relative =
                normalize_path(parent.strip_prefix(workspace_root).map_err(|_| {
                    format!("package manifest escaped workspace: {}", path.display())
                })?)?;
            packages.insert(relative);
        }
    }
    Ok(())
}

fn protected_files(root: &Path) -> Result<Vec<PathBuf>, String> {
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
    collect_workspace_root_files(root, &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_workspace_root_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(root)
        .map_err(|error| format!("failed to read workspace root {}: {error}", root.display()))?
    {
        let path = entry
            .map_err(|error| format!("failed to read workspace root entry: {error}"))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if metadata.is_dir() || path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "protected workspace root contains a symlink or reparse point: {}",
                path.display()
            ));
        }
        ensure_protected_text_file(&path)?;
        files.push(path);
    }
    Ok(())
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
        } else {
            ensure_protected_text_file(&path)?;
            files.push(path);
        }
    }
    Ok(())
}

fn ensure_protected_text_file(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("protected file name is not UTF-8: {}", path.display()))?;
    if matches!(
        name,
        ".gitattributes" | ".gitignore" | "Cargo.lock" | "Cargo.toml" | "LICENSE"
    ) {
        return Ok(());
    }
    let known = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "bat"
                    | "cfg"
                    | "cmd"
                    | "ini"
                    | "json"
                    | "jsonl"
                    | "lock"
                    | "md"
                    | "proto"
                    | "ps1"
                    | "ron"
                    | "rs"
                    | "sh"
                    | "sql"
                    | "stderr"
                    | "toml"
                    | "txt"
                    | "yaml"
                    | "yml"
            )
        });
    if known {
        Ok(())
    } else {
        Err(format!(
            "protected Runtime surface has an unknown file type: {}",
            path.display()
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSurface {
    kind: &'static str,
    selector: String,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityFragment {
    selector: String,
    content: String,
}

fn identity_fragments_for_file(
    path: &Path,
    stable_path: &str,
) -> Result<Vec<IdentityFragment>, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let mut fragments = match extension {
        "rs" => rust_identity_fragments(stable_path, &source)?,
        "json" => structured_json_inventory(stable_path, &source)?
            .into_iter()
            .map(identity_fragment_from_raw)
            .collect(),
        "toml" => structured_toml_inventory(stable_path, &source)?
            .into_iter()
            .map(identity_fragment_from_raw)
            .collect(),
        _ => text_surface_inventory(&source)
            .into_iter()
            .map(identity_fragment_from_raw)
            .collect(),
    };
    fragments.sort_by(|left, right| left.selector.cmp(&right.selector));
    if fragments
        .windows(2)
        .any(|pair| pair[0].selector == pair[1].selector)
    {
        return Err(format!(
            "identity inventory produced duplicate selectors for {stable_path}"
        ));
    }
    Ok(fragments)
}

fn identity_fragment_from_raw(surface: RawSurface) -> IdentityFragment {
    IdentityFragment {
        selector: format!("{}:{}", surface.kind, surface.selector),
        content: surface.content,
    }
}

fn rust_identity_fragments(path: &str, source: &str) -> Result<Vec<IdentityFragment>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse {path}: {error}"))?;
    let mut collector = RustIdentityFragmentCollector::default();
    if !file.attrs.is_empty() {
        collector.fragments.push(IdentityFragment {
            selector: "rust:file_attributes".to_string(),
            content: file
                .attrs
                .iter()
                .map(ToTokens::to_token_stream)
                .collect::<proc_macro2::TokenStream>()
                .to_string(),
        });
    }
    collector.collect_items(&file.items, &[]);
    let counts =
        collector
            .fragments
            .iter()
            .fold(HashMap::<String, usize>::new(), |mut counts, fragment| {
                *counts.entry(fragment.selector.clone()).or_default() += 1;
                counts
            });
    let mut duplicate_ordinals = HashMap::<String, usize>::new();
    for fragment in &mut collector.fragments {
        if counts.get(&fragment.selector).copied().unwrap_or_default() > 1 {
            let base = fragment.selector.clone();
            let digest = short_hash(&fragment.content);
            let ordinal = duplicate_ordinals
                .entry(format!("{base}@{digest}"))
                .or_default();
            fragment.selector = format!("{base}@{digest}:{}", *ordinal);
            *ordinal += 1;
        }
    }
    collector
        .fragments
        .sort_by(|left, right| left.selector.cmp(&right.selector));
    Ok(collector.fragments)
}

#[derive(Default)]
struct RustIdentityFragmentCollector {
    fragments: Vec<IdentityFragment>,
}

impl RustIdentityFragmentCollector {
    fn collect_items(&mut self, items: &[Item], module: &[String]) {
        for item in items {
            match item {
                Item::Mod(item) if item.content.is_some() => {
                    let selector = qualified(module, &format!("mod:{}", item.ident));
                    let attributes = &item.attrs;
                    let visibility = &item.vis;
                    let identifier = &item.ident;
                    self.push(
                        format!("rust:{selector}"),
                        quote::quote!(#(#attributes)* #visibility mod #identifier;),
                    );
                    let mut next = module.to_vec();
                    next.push(item.ident.to_string());
                    self.collect_items(
                        &item.content.as_ref().expect("checked inline module").1,
                        &next,
                    );
                }
                Item::Impl(item) => {
                    let trait_name = item
                        .trait_
                        .as_ref()
                        .map(|(_, path, _)| path.to_token_stream().to_string())
                        .unwrap_or_else(|| "inherent".to_string());
                    let owner = qualified(
                        module,
                        &format!(
                            "impl:{}:{}",
                            short_hash(&trait_name),
                            item.self_ty.to_token_stream()
                        ),
                    );
                    for member in &item.items {
                        let name = match member {
                            ImplItem::Const(member) => format!("const:{}", member.ident),
                            ImplItem::Fn(member) => format!("fn:{}", member.sig.ident),
                            ImplItem::Type(member) => format!("type:{}", member.ident),
                            ImplItem::Macro(member) => format!(
                                "macro:{}",
                                short_hash(&member.to_token_stream().to_string())
                            ),
                            ImplItem::Verbatim(member) => {
                                format!("verbatim:{}", short_hash(&member.to_string()))
                            }
                            _ => format!(
                                "item:{}",
                                short_hash(&member.to_token_stream().to_string())
                            ),
                        };
                        self.push(format!("rust:{owner}::{name}"), member.to_token_stream());
                    }
                }
                _ => self.push(
                    format!("rust:{}", identity_item_selector(item, module)),
                    item.to_token_stream(),
                ),
            }
        }
    }

    fn push(&mut self, selector: String, content: impl ToTokens) {
        self.fragments.push(IdentityFragment {
            selector,
            content: content.to_token_stream().to_string(),
        });
    }
}

fn identity_item_selector(item: &Item, module: &[String]) -> String {
    let label = match item {
        Item::Const(item) => format!("const:{}", item.ident),
        Item::Enum(item) => format!("enum:{}", item.ident),
        Item::ExternCrate(item) => format!("extern:{}", item.ident),
        Item::Fn(item) => format!("fn:{}", item.sig.ident),
        Item::ForeignMod(item) => format!(
            "foreign:{}",
            short_hash(&item.to_token_stream().to_string())
        ),
        Item::Macro(item) => format!(
            "macro:{}",
            item.ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| short_hash(&item.to_token_stream().to_string()))
        ),
        Item::Mod(item) => format!("mod:{}", item.ident),
        Item::Static(item) => format!("static:{}", item.ident),
        Item::Struct(item) => format!("struct:{}", item.ident),
        Item::Trait(item) => format!("trait:{}", item.ident),
        Item::TraitAlias(item) => format!("trait_alias:{}", item.ident),
        Item::Type(item) => format!("type:{}", item.ident),
        Item::Union(item) => format!("union:{}", item.ident),
        Item::Use(item) => format!("use:{}", short_hash(&item.to_token_stream().to_string())),
        Item::Verbatim(item) => format!("verbatim:{}", short_hash(&item.to_string())),
        _ => format!("item:{}", short_hash(&item.to_token_stream().to_string())),
    };
    qualified(module, &label)
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
    errors: Vec<String>,
    aliases: Vec<HashMap<String, &'static str>>,
    collections: Vec<HashMap<String, Vec<String>>>,
    return_axis: Option<&'static str>,
}

impl Visit<'_> for IdentityBranchVisitor<'_> {
    fn visit_block(&mut self, node: &syn::Block) {
        self.aliases.push(HashMap::new());
        self.collections.push(HashMap::new());
        syn::visit::visit_block(self, node);
        self.aliases.pop();
        self.collections.pop();
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if let syn::Pat::Ident(pattern) = &node.pat
            && let Some(initializer) = &node.init
        {
            let name = pattern.ident.to_string();
            if let Some(axis) = self.axis(&initializer.expr) {
                self.aliases
                    .last_mut()
                    .expect("identity scope")
                    .insert(name.clone(), axis);
            }
            let values = expression_strings(&initializer.expr);
            if !values.is_empty() {
                self.collections
                    .last_mut()
                    .expect("collection scope")
                    .insert(name.clone(), values.clone());
                if let Some(axis) = exact_identity_axis(&name) {
                    self.record_values(axis, values);
                }
            }
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_binary(&mut self, node: &ExprBinary) {
        if matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_)) {
            if let Some(axis) = self.axis(&node.left) {
                self.record_values(axis, expression_strings(&node.right));
            }
            if let Some(axis) = self.axis(&node.right) {
                self.record_values(axis, expression_strings(&node.left));
            }
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &syn::ExprMethodCall) {
        let method = node.method.to_string();
        if matches!(
            method.as_str(),
            "eq" | "ne" | "starts_with" | "ends_with" | "strip_prefix" | "strip_suffix"
        ) {
            if let Some(axis) = self.axis(&node.receiver) {
                self.record_values(
                    axis,
                    node.args.iter().flat_map(expression_strings).collect(),
                );
            }
            for argument in &node.args {
                if let Some(axis) = self.axis(argument) {
                    self.record_values(axis, expression_strings(&node.receiver));
                }
            }
        }
        if matches!(
            method.as_str(),
            "contains" | "get" | "get_mut" | "binary_search" | "binary_search_by_key"
        ) {
            for argument in &node.args {
                if let Some(axis) = self.axis(argument) {
                    self.record_values(axis, self.collection_values(&node.receiver));
                }
            }
        }
        if matches!(
            method.as_str(),
            "unwrap_or" | "get_or_insert" | "replace" | "then_some"
        ) && let Some(axis) = self.axis(&node.receiver)
        {
            self.record_values(
                axis,
                node.args.iter().flat_map(expression_strings).collect(),
            );
        }
        if method == "unwrap_or_else"
            && let Some(axis) = self.axis(&node.receiver)
        {
            self.record_values(
                axis,
                node.args.iter().flat_map(expression_strings).collect(),
            );
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        if let Some(axis) = self.axis(&node.expr) {
            for arm in &node.arms {
                let mut strings = PatternStringVisitor::default();
                strings.visit_pat(&arm.pat);
                self.record_values(axis, strings.values);
            }
        }
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_macro(&mut self, node: &syn::ExprMacro) {
        if node.mac.path.is_ident("matches") {
            match syn::parse2::<MatchesExpression>(node.mac.tokens.clone()) {
                Ok(matches) => {
                    if let Some(axis) = self.axis(&matches.expression) {
                        let mut strings = PatternStringVisitor::default();
                        strings.visit_pat(&matches.pattern);
                        self.record_values(axis, strings.values);
                    }
                }
                Err(error) => self.errors.push(format!(
                    "{}: failed to parse matches! identity expression: {error}",
                    self.path
                )),
            }
        }
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_expr_struct(&mut self, node: &syn::ExprStruct) {
        for field in &node.fields {
            if let Member::Named(identifier) = &field.member
                && let Some(axis) = exact_identity_axis(&identifier.to_string())
            {
                self.record_values(axis, expression_strings(&field.expr));
            }
        }
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_assign(&mut self, node: &syn::ExprAssign) {
        if let Some(axis) = self.axis(&node.left) {
            self.record_values(axis, expression_strings(&node.right));
        }
        syn::visit::visit_expr_assign(self, node);
    }

    fn visit_expr_return(&mut self, node: &syn::ExprReturn) {
        if let (Some(axis), Some(expression)) = (self.return_axis, &node.expr) {
            self.record_values(axis, expression_strings(expression));
        }
        syn::visit::visit_expr_return(self, node);
    }

    fn visit_item_const(&mut self, node: &syn::ItemConst) {
        if let Some(axis) = exact_identity_axis(&node.ident.to_string()) {
            self.record_values(axis, expression_strings(&node.expr));
        }
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &syn::ItemStatic) {
        if let Some(axis) = exact_identity_axis(&node.ident.to_string()) {
            self.record_values(axis, expression_strings(&node.expr));
        }
        syn::visit::visit_item_static(self, node);
    }

    fn visit_item_fn(&mut self, node: &syn::ItemFn) {
        let previous = self.return_axis;
        self.return_axis = identity_return_axis(&node.sig.ident.to_string());
        if let Some(axis) = self.return_axis {
            self.record_values(axis, block_tail_strings(&node.block));
        }
        syn::visit::visit_item_fn(self, node);
        self.return_axis = previous;
    }

    fn visit_impl_item_fn(&mut self, node: &syn::ImplItemFn) {
        let previous = self.return_axis;
        self.return_axis = identity_return_axis(&node.sig.ident.to_string());
        if let Some(axis) = self.return_axis {
            self.record_values(axis, block_tail_strings(&node.block));
        }
        syn::visit::visit_impl_item_fn(self, node);
        self.return_axis = previous;
    }
}

impl IdentityBranchVisitor<'_> {
    fn axis(&self, expression: &Expr) -> Option<&'static str> {
        identity_axis(expression, &self.aliases)
    }

    fn collection_values(&self, expression: &Expr) -> Vec<String> {
        let direct = expression_strings(expression);
        if !direct.is_empty() {
            return direct;
        }
        let Expr::Path(path) = expression else {
            return Vec::new();
        };
        let Some(name) = path.path.get_ident().map(ToString::to_string) else {
            return Vec::new();
        };
        self.collections
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).cloned())
            .unwrap_or_default()
    }

    fn record_values(&mut self, axis: &str, values: Vec<String>) {
        for value in values {
            self.record(axis, &value);
        }
    }

    fn record(&mut self, axis: &str, value: &str) {
        self.violations.push(format!(
            "{}: raw identity string {value:?} on axis {axis}",
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

struct MatchesExpression {
    expression: Expr,
    pattern: syn::Pat,
    _guard: Option<Expr>,
}

impl Parse for MatchesExpression {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let expression = input.parse()?;
        input.parse::<Token![,]>()?;
        let pattern = syn::Pat::parse_multi_with_leading_vert(input)?;
        let guard = if input.peek(Token![if]) {
            input.parse::<Token![if]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        Ok(Self {
            expression,
            pattern,
            _guard: guard,
        })
    }
}

fn identity_axis(
    expression: &Expr,
    aliases: &[HashMap<String, &'static str>],
) -> Option<&'static str> {
    match expression {
        Expr::Field(field) => match &field.member {
            Member::Named(identifier) => exact_identity_axis(&identifier.to_string()),
            Member::Unnamed(_) => None,
        },
        Expr::Path(path) => path.path.segments.last().and_then(|segment| {
            let name = segment.ident.to_string();
            exact_identity_axis(&name).or_else(|| {
                aliases
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(&name).copied())
            })
        }),
        Expr::Paren(paren) => identity_axis(&paren.expr, aliases),
        Expr::Group(group) => identity_axis(&group.expr, aliases),
        Expr::Reference(reference) => identity_axis(&reference.expr, aliases),
        Expr::Try(expression) => identity_axis(&expression.expr, aliases),
        Expr::Await(expression) => identity_axis(&expression.base, aliases),
        Expr::Cast(expression) => identity_axis(&expression.expr, aliases),
        Expr::Unary(expression) => identity_axis(&expression.expr, aliases),
        Expr::MethodCall(call)
            if matches!(
                call.method.to_string().as_str(),
                "as_deref"
                    | "as_ref"
                    | "as_str"
                    | "borrow"
                    | "clone"
                    | "deref"
                    | "to_owned"
                    | "trim"
            ) =>
        {
            identity_axis(&call.receiver, aliases)
        }
        Expr::Call(call) if call.args.len() == 1 => identity_axis(&call.args[0], aliases),
        _ => None,
    }
}

fn expression_strings(expression: &Expr) -> Vec<String> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(value) => vec![value.value()],
            _ => Vec::new(),
        },
        Expr::Array(array) => array.elems.iter().flat_map(expression_strings).collect(),
        Expr::Tuple(tuple) => tuple.elems.iter().flat_map(expression_strings).collect(),
        Expr::Paren(paren) => expression_strings(&paren.expr),
        Expr::Group(group) => expression_strings(&group.expr),
        Expr::Reference(reference) => expression_strings(&reference.expr),
        Expr::Call(call) => call.args.iter().flat_map(expression_strings).collect(),
        Expr::MethodCall(call)
            if matches!(
                call.method.to_string().as_str(),
                "into" | "to_owned" | "to_string"
            ) =>
        {
            expression_strings(&call.receiver)
        }
        Expr::Closure(closure) => expression_strings(&closure.body),
        Expr::Block(block) => block_tail_strings(&block.block),
        Expr::If(expression) => {
            let mut values = block_tail_strings(&expression.then_branch);
            if let Some((_, otherwise)) = &expression.else_branch {
                values.extend(expression_strings(otherwise));
            }
            values
        }
        Expr::Match(expression) => expression
            .arms
            .iter()
            .flat_map(|arm| expression_strings(&arm.body))
            .collect(),
        _ => Vec::new(),
    }
}

fn block_tail_strings(block: &syn::Block) -> Vec<String> {
    block
        .stmts
        .last()
        .and_then(|statement| match statement {
            syn::Stmt::Expr(expression, None) => Some(expression_strings(expression)),
            _ => None,
        })
        .unwrap_or_default()
}

fn identity_return_axis(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        "choose_",
        "current_",
        "default_",
        "effective_",
        "fallback_",
        "resolve_",
        "select_",
    ];
    PREFIXES
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix).and_then(exact_identity_axis))
}

fn exact_identity_axis(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "game" => Some("game"),
        "package" => Some("package"),
        "profile" => Some("profile"),
        "project" => Some("project"),
        "provider" => Some("provider"),
        "resource" => Some("resource"),
        "server" => Some("server"),
        "task" => Some("task"),
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

fn is_test_identity_fragment(path: &str, selector: &str) -> bool {
    path.contains("/tests/")
        || path.ends_with("/tests.rs")
        || selector.contains("::tests::")
        || selector.starts_with("rust:tests::")
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

[[approval]]
id = "approval.issue44_r8b"
repository = "HS7097/ActingCommand-Workflow"
issue = 54
comment_id = 5011350539
author = "HS7097"
content_sha256 = "{}"
scope = ["identity.allowance", "surface.mapping"]

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
            "b".repeat(64),
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
    fn identity_branches_cover_nine_axes_and_propagation_forms() {
        let source = r#"
            struct Context {
                game: String,
                server: String,
                project: String,
                profile: String,
                package: String,
                resource: String,
                task: String,
                theme: String,
                provider: String,
            }

            struct Defaults {
                theme: String,
                provider: String,
            }

            fn inspect(context: &Context) {
                let selected = context.game.as_ref();
                let servers = ["server.alpha", "server.beta"];
                let package = "package.alpha";
                let resource = "resource.alpha";
                let task = "task.alpha";
                let _ = selected.eq("game.alpha");
                let _ = servers.contains(&context.server.as_str());
                let _ = context.resource == "resource.direct";
                let _ = context.task == "task.direct";
                let _ = context.provider == "provider.direct";
                let _ = matches!(context.project.as_ref(), "project.alpha" | "project.beta");
                let _ = match context.profile.as_str() {
                    "profile.alpha" => true,
                    _ => false,
                };
                let _ = Defaults {
                    theme: "theme.alpha".to_string(),
                    provider: "provider.alpha".to_string(),
                };
            }

            fn default_game() -> &'static str { "game.default" }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for axis in [
            "game", "server", "project", "profile", "package", "resource", "task", "theme",
            "provider",
        ] {
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains(&format!("axis {axis}"))),
                "missing axis {axis}: {violations:#?}"
            );
        }
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.default"))
        );
    }

    #[test]
    fn typed_identity_and_neighboring_names_remain_allowed() {
        let source = r#"
            enum GameIdentity { Alpha, Beta }
            struct Context { game: GameIdentity }

            fn inspect(context: &Context) -> bool {
                let provider_count = "neutral";
                context.game == GameIdentity::Alpha && provider_count == "neutral"
            }
        "#;
        assert!(
            inspect_identity_axis_branches("fixture.rs", source)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn guard_fixture_allowance_cannot_target_product_code() {
        let source = format!(
            "{}\n{}",
            registry_source(),
            identity_allowance_source(
                "allowance.fixture",
                "guard_fixture",
                "crates/example/src/lib.rs",
                &["maa"],
                &"a".repeat(64),
            )
        );
        let registry = parse_generic_domain_registry(&source).unwrap();
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("guard_fixture is outside the guard test directory"));
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
        let fragment = identity_fragments_for_file(&root.join(path), path)
            .unwrap()
            .into_iter()
            .find(|fragment| fragment.selector == "rust:fn:compile_maa_tasks")
            .unwrap();
        let hash = format!("{:x}", Sha256::digest(fragment.content.as_bytes()));
        registry.identity_allowance.push(IdentityAllowance {
            id: "allowance.technical".to_string(),
            kind: "technical_adapter".to_string(),
            exact_path: path.to_string(),
            selector: "rust:fn:compile_maa_tasks".to_string(),
            scope: vec!["identity.token".to_string()],
            tokens: vec!["maa".to_string()],
            sha256: hash,
            purpose: "Exact technical adapter boundary.".to_string(),
            approval_id: "approval.issue44_r8b".to_string(),
            source_issue: 44,
            source_pr: Some(111),
        });
        validate_generic_domain_registry(&registry).unwrap();
        validate_workspace_genericity(&root, &registry).unwrap();

        fs::write(root.join(path), "fn compile_maa_jobs() {}\n").unwrap();
        let error = validate_workspace_genericity(&root, &registry).unwrap_err();
        assert!(error.contains("missing selector") || error.contains("fragment hash drifted"));

        fs::create_dir_all(root.join("scratch")).unwrap();
        let outside = "scratch/outside.rs";
        fs::write(root.join(outside), "fn compile_maa_tasks() {}\n").unwrap();
        let outside_fragment = identity_fragments_for_file(&root.join(outside), outside)
            .unwrap()
            .into_iter()
            .find(|fragment| fragment.selector == "rust:fn:compile_maa_tasks")
            .unwrap();
        let outside_hash = format!("{:x}", Sha256::digest(outside_fragment.content.as_bytes()));
        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let mut outside_registry = registry_for_snapshots(&snapshots);
        outside_registry.identity_allowance.push(IdentityAllowance {
            id: "allowance.outside".to_string(),
            kind: "technical_adapter".to_string(),
            exact_path: outside.to_string(),
            selector: "rust:fn:compile_maa_tasks".to_string(),
            scope: vec!["identity.token".to_string()],
            tokens: vec!["maa".to_string()],
            sha256: outside_hash,
            purpose: "Counterexample outside registered surfaces.".to_string(),
            approval_id: "approval.issue44_r8b".to_string(),
            source_issue: 44,
            source_pr: Some(111),
        });
        let error = validate_workspace_genericity(&root, &outside_registry).unwrap_err();
        assert!(error.contains("outside registered Runtime surfaces"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn identity_allowance_is_limited_to_one_exact_ast_fragment() {
        let root = temporary_workspace("fragment-allowance");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        let path = "crates/example/src/lib.rs";
        fs::write(
            root.join(path),
            "fn compile_maa_tasks() {}\nfn compile_maa_jobs() {}\n",
        )
        .unwrap();
        create_required_roots(&root);
        write_external_compat_manifest(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let mut registry = registry_for_snapshots(&snapshots);
        let fragment = identity_fragments_for_file(&root.join(path), path)
            .unwrap()
            .into_iter()
            .find(|fragment| fragment.selector == "rust:fn:compile_maa_tasks")
            .unwrap();
        registry.identity_allowance.push(IdentityAllowance {
            id: "allowance.fragment".to_string(),
            kind: "technical_adapter".to_string(),
            exact_path: path.to_string(),
            selector: fragment.selector,
            scope: vec!["identity.token".to_string()],
            tokens: vec!["maa".to_string()],
            sha256: format!("{:x}", Sha256::digest(fragment.content.as_bytes())),
            purpose: "Exact AST fragment counterexample.".to_string(),
            approval_id: "approval.issue44_r8b".to_string(),
            source_issue: 44,
            source_pr: Some(112),
        });

        let error = validate_workspace_genericity(&root, &registry).unwrap_err();
        assert!(error.contains("rust:fn:compile_maa_jobs"));
        assert!(error.contains("project-specific word maa"));
        assert!(!error.contains("rust:fn:compile_maa_tasks"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn branch_allowance_is_limited_to_one_exact_test_fragment() {
        let root = temporary_workspace("branch-allowance");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/tests")).unwrap();
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Marker;\n",
        )
        .unwrap();
        let path = "crates/example/tests/identity.rs";
        fs::write(
            root.join(path),
            r#"
                fn first(game: &str) -> bool { game == "identity.alpha" }
                fn second(game: &str) -> bool { game == "identity.beta" }
            "#,
        )
        .unwrap();
        create_required_roots(&root);
        write_external_compat_manifest(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let mut registry = registry_for_snapshots(&snapshots);
        let fragment = identity_fragments_for_file(&root.join(path), path)
            .unwrap()
            .into_iter()
            .find(|fragment| fragment.selector == "rust:fn:first")
            .unwrap();
        registry.identity_allowance.push(IdentityAllowance {
            id: "allowance.fragment".to_string(),
            kind: "test_fixture".to_string(),
            exact_path: path.to_string(),
            selector: fragment.selector,
            scope: vec!["identity.branch".to_string()],
            tokens: Vec::new(),
            sha256: format!("{:x}", Sha256::digest(fragment.content.as_bytes())),
            purpose: "Exact branch fragment counterexample.".to_string(),
            approval_id: "approval.issue44_r8b".to_string(),
            source_issue: 44,
            source_pr: Some(112),
        });

        let error = validate_workspace_genericity(&root, &registry).unwrap_err();
        assert!(error.contains("rust:fn:second"));
        assert!(error.contains("identity.beta"));
        assert!(!error.contains("identity.alpha"));
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

    #[test]
    fn workspace_discovery_rejects_undeclared_package_manifest() {
        let root = temporary_workspace("undeclared-package");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::write(
            root.join("crates/example/Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.0.0\"\n\n[dependencies]\nhidden = { path = \"../hidden\" }\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/hidden/src")).unwrap();
        fs::write(
            root.join("crates/hidden/Cargo.toml"),
            "[package]\nname = \"hidden\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        create_required_roots(&root);

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("workspace package manifest is not declared or excluded"));
        assert!(error.contains("crates/hidden"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_roots_reject_unknown_file_types() {
        let root = temporary_workspace("unknown-file-type");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Marker;\n",
        )
        .unwrap();
        create_required_roots(&root);
        fs::write(root.join("contracts/opaque.random"), b"opaque").unwrap();

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("unknown file type"));
        assert!(error.contains("opaque.random"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proto_and_root_script_are_itemized_protected_surfaces() {
        let root = temporary_workspace("new-text-surfaces");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Marker;\n",
        )
        .unwrap();
        create_required_roots(&root);
        fs::write(
            root.join("contracts/example.proto"),
            "message Example { string provider = 1; }\n",
        )
        .unwrap();
        fs::write(root.join("verify.ps1"), "Write-Output 'verify'\n").unwrap();

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        assert!(
            snapshots
                .iter()
                .any(|surface| surface.stable_path == "contracts/example.proto")
        );
        assert!(
            snapshots
                .iter()
                .any(|surface| surface.stable_path == "verify.ps1")
        );
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
        for (index, member) in members.iter().enumerate() {
            fs::create_dir_all(root.join(member)).unwrap();
            fs::write(
                root.join(member).join("Cargo.toml"),
                format!(
                    "[package]\nname = \"fixture-{index}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n"
                ),
            )
            .unwrap();
        }
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
selector = "rust:fn:compile_maa_tasks"
scope = ["identity.token"]
tokens = [{tokens}]
sha256 = {sha256:?}
purpose = "Exact test allowance."
approval_id = "approval.issue44_r8b"
source_issue = 44
source_pr = 111
"#
        )
    }

    fn registry_for_snapshots(snapshots: &[SurfaceSnapshot]) -> GenericDomainRegistry {
        GenericDomainRegistry {
            schema_version: GENERIC_DOMAIN_SCHEMA_VERSION.to_string(),
            surface_manifest: None,
            approval: vec![
                SurfaceApproval {
                    id: "approval.issue44_r8".to_string(),
                    repository: "HS7097/ActingCommand-Workflow".to_string(),
                    issue: 54,
                    comment_id: 5011264343,
                    author: "HS7097".to_string(),
                    content_sha256: "a".repeat(64),
                    scope: vec!["surface.mapping".to_string()],
                },
                SurfaceApproval {
                    id: "approval.issue44_r8b".to_string(),
                    repository: "HS7097/ActingCommand-Workflow".to_string(),
                    issue: 54,
                    comment_id: 5011350539,
                    author: "HS7097".to_string(),
                    content_sha256: "b".repeat(64),
                    scope: vec![
                        "identity.allowance".to_string(),
                        "surface.mapping".to_string(),
                    ],
                },
            ],
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
