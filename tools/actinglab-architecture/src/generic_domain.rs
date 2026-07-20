// SPDX-License-Identifier: AGPL-3.0-only

//! Machine-readable generic-domain concepts and protected Runtime surfaces.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use syn::parse::{Parse, ParseStream};
use syn::visit::Visit;
use syn::{
    Attribute, BinOp, Expr, ExprBinary, ExprMatch, Fields, FnArg, GenericArgument, Ident, ImplItem,
    Item, Lit, LitStr, Member, PathArguments, Token, Type, Visibility, braced,
};

use crate::external_compat::{EXTERNAL_COMPAT_MANIFEST_PATH, validated_external_compat_paths};
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
pub const TRACKED_INPUT_POLICY_PATH: &str =
    "tools/actinglab-architecture/tracked-input-policy-v1.toml";
const TRACKED_INPUT_POLICY_SCHEMA_VERSION: &str = "actingcommand.tracked-input-policy.v1";
const EXTERNAL_COMPAT_ROOT: &str = "tests/external-compat";
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
struct TrackedInputPolicy {
    schema_version: String,
    #[serde(default)]
    non_product: Vec<NonProductPath>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NonProductPath {
    path: String,
    kind: String,
    purpose: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrackedFileClass {
    Protected,
    ExternalCompat,
    NonProduct,
}

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
    #[serde(default)]
    pub author_id: Option<u64>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
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
        if approval.author_id == Some(0) {
            errors.push(format!(
                "surface approval {} has invalid numeric author id",
                approval.id
            ));
        }
        for (field, value) in [
            ("created_at", approval.created_at.as_deref()),
            ("updated_at", approval.updated_at.as_deref()),
        ] {
            if value.is_some_and(str::is_empty) {
                errors.push(format!(
                    "surface approval {} has empty {field}",
                    approval.id
                ));
            }
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
                | "rust_wire_impl"
                | "rust_wire_attribute"
                | "rust_cli_attribute"
                | "rust_derive_attribute"
                | "rust_ffi_attribute"
                | "rust_ffi_item"
                | "rust_attribute"
                | "rust_match_literal"
                | "rust_macro_item"
                | "rust_macro_invocation"
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
    if let Some(pair) = snapshots
        .windows(2)
        .find(|pair| pair[0].surface_id == pair[1].surface_id)
    {
        return Err(format!(
            "surface inventory produced duplicate stable id {} for {} {} {} and {} {} {}",
            pair[0].surface_id,
            pair[0].kind,
            pair[0].stable_path,
            pair[0].selector,
            pair[1].kind,
            pair[1].stable_path,
            pair[1].selector
        ));
    }
    Ok(snapshots)
}

pub fn workspace_identity_allowance_candidates(
    root: &Path,
) -> Result<Vec<IdentityAllowanceCandidate>, String> {
    let external_paths = validated_external_compat_paths(root)?
        .into_iter()
        .collect::<HashSet<_>>();
    let files = protected_files(root)?;
    let callables = collect_workspace_callable_semantics(&files)?;

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
        let file_callables = if file.extension().is_some_and(|extension| extension == "rs") {
            Some(collect_file_callable_semantics(&file, &callables)?)
        } else {
            None
        };
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
                inspect_identity_axis_branches_with_semantics(
                    &label,
                    &fragment.content,
                    file_callables.as_ref().unwrap_or(&callables),
                )?
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
    let external_paths = validated_external_compat_paths(root)?
        .into_iter()
        .collect::<HashSet<_>>();
    let allowance_by_fragment = validate_identity_allowance_fragments(root, registry)?;

    let files = protected_files(root)?;
    let callables = collect_workspace_callable_semantics(&files)?;

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
        let file_callables = if file.extension().is_some_and(|extension| extension == "rs") {
            Some(collect_file_callable_semantics(&file, &callables)?)
        } else {
            None
        };
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
                errors.extend(inspect_identity_axis_branches_with_semantics(
                    &label,
                    &fragment.content,
                    file_callables.as_ref().unwrap_or(&callables),
                )?);
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
    let callables = collect_callable_semantics(&file);
    inspect_identity_axis_branches_with_semantics(path, source, &callables)
}

fn inspect_identity_axis_branches_with_semantics(
    path: &str,
    source: &str,
    callables: &CallableSemantics,
) -> Result<Vec<String>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse {path}: {error}"))?;
    let macro_definitions = collect_macro_definitions(&file);
    let constants = collect_constant_flows(&file, &macro_definitions, callables);
    let mut visitor = IdentityBranchVisitor {
        path,
        violations: Vec::new(),
        errors: Vec::new(),
        comparisons: BTreeSet::new(),
        direct: BTreeSet::new(),
        summary_event_limit: None,
        summary_overflow: false,
        bindings: vec![HashMap::new()],
        constants,
        macro_definitions,
        callables,
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
    let classes = tracked_file_classes(root)?;
    validate_compile_input_closure(root, &classes)?;
    Ok(classes
        .into_iter()
        .filter_map(|(path, class)| {
            (class != TrackedFileClass::NonProduct).then(|| root.join(path))
        })
        .collect())
}

fn load_tracked_input_policy(root: &Path) -> Result<TrackedInputPolicy, String> {
    let path = root.join(TRACKED_INPUT_POLICY_PATH);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    toml::from_str(&source)
        .map_err(|error| format!("invalid tracked-input policy {}: {error}", path.display()))
}

fn tracked_file_classes(root: &Path) -> Result<Vec<(String, TrackedFileClass)>, String> {
    let policy = load_tracked_input_policy(root)?;
    let members = workspace_members(root)?;
    let tracked = git_tracked_files(root)?;
    let external_paths = validated_external_compat_paths(root)?
        .into_iter()
        .chain([EXTERNAL_COMPAT_MANIFEST_PATH.to_string()])
        .collect::<HashSet<_>>();
    let mut errors = validate_tracked_input_policy(&policy, &members);
    let mut policy_matches = vec![false; policy.non_product.len()];
    let mut classes = Vec::with_capacity(tracked.len());

    for path in tracked {
        let absolute = match resolve_exact_regular_file(root, &path) {
            Ok(path) => path,
            Err(error) => {
                errors.push(format!("tracked file {path} {error}"));
                continue;
            }
        };
        let class = if external_paths.contains(&path) {
            TrackedFileClass::ExternalCompat
        } else if path_is_within(&path, EXTERNAL_COMPAT_ROOT) {
            errors.push(format!(
                "external-compat file is not registered by the exact manifest: {path}"
            ));
            continue;
        } else if path == TRACKED_INPUT_POLICY_PATH
            || !path.contains('/')
            || members.iter().any(|member| path_is_within(&path, member))
            || REQUIRED_PROTECTED_ROOTS
                .iter()
                .any(|protected| path_is_within(&path, protected))
        {
            TrackedFileClass::Protected
        } else if let Some((index, _)) = policy
            .non_product
            .iter()
            .enumerate()
            .find(|(_, entry)| path_is_within(&path, &entry.path))
        {
            policy_matches[index] = true;
            TrackedFileClass::NonProduct
        } else {
            errors.push(format!("unclassified tracked file {path}"));
            continue;
        };
        if class != TrackedFileClass::NonProduct
            && let Err(error) = ensure_protected_text_file(&absolute)
        {
            errors.push(error);
            continue;
        }
        classes.push((path, class));
    }

    for (entry, matched) in policy.non_product.iter().zip(policy_matches) {
        if !matched {
            errors.push(format!(
                "non-product classification matches no tracked files: {}",
                entry.path
            ));
        }
    }
    for external in external_paths {
        if !classes.iter().any(|(path, _)| path == &external) {
            errors.push(format!("external-compat file is not tracked: {external}"));
        }
    }
    finish_errors(errors)?;
    classes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(classes)
}

fn validate_tracked_input_policy(policy: &TrackedInputPolicy, members: &[String]) -> Vec<String> {
    let mut errors = Vec::new();
    if policy.schema_version != TRACKED_INPUT_POLICY_SCHEMA_VERSION {
        errors.push(format!(
            "unsupported tracked-input policy schema_version {}; expected {TRACKED_INPUT_POLICY_SCHEMA_VERSION}",
            policy.schema_version
        ));
    }
    let mut previous = None;
    for entry in &policy.non_product {
        if previous.is_some_and(|path: &str| path >= entry.path.as_str()) {
            errors.push(format!(
                "non-product paths are not strictly sorted at {}",
                entry.path
            ));
        }
        previous = Some(entry.path.as_str());
        if let Err(error) = validate_stable_path(&entry.path) {
            errors.push(format!("non-product path {}", error));
        }
        if !matches!(entry.kind.as_str(), "documentation" | "external_tool") {
            errors.push(format!(
                "non-product path {} has invalid kind {}",
                entry.path, entry.kind
            ));
        }
        if entry.purpose.trim().is_empty() {
            errors.push(format!("non-product path {} has empty purpose", entry.path));
        }
        if members
            .iter()
            .any(|member| path_is_within(&entry.path, member))
            || REQUIRED_PROTECTED_ROOTS
                .iter()
                .any(|protected| path_is_within(&entry.path, protected))
            || path_is_within(&entry.path, EXTERNAL_COMPAT_ROOT)
        {
            errors.push(format!(
                "non-product path overlaps a protected boundary: {}",
                entry.path
            ));
        }
    }
    errors
}

fn git_tracked_files(root: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .map_err(|error| format!("failed to read trusted Git index: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to read trusted Git index: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(str::to_string)
                .map_err(|error| format!("trusted Git index contains non-UTF-8 path: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    if files.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("trusted Git index contains duplicate paths".to_string());
    }
    if !files.iter().any(|path| path == "Cargo.toml") {
        return Err("trusted Git index does not contain Cargo.toml".to_string());
    }
    for path in &files {
        validate_stable_path(path).map_err(|error| format!("trusted Git index path {error}"))?;
    }
    Ok(files)
}

fn path_is_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn validate_compile_input_closure(
    root: &Path,
    classes: &[(String, TrackedFileClass)],
) -> Result<(), String> {
    let class_by_path = classes
        .iter()
        .map(|(path, class)| (path.as_str(), *class))
        .collect::<HashMap<_, _>>();
    let mut errors = Vec::new();
    for (path, class) in classes {
        if *class != TrackedFileClass::Protected {
            continue;
        }
        let inputs = if path.ends_with(".rs") {
            rust_compile_inputs(root, path)
        } else if path.ends_with("Cargo.toml") {
            cargo_compile_inputs(root, path)
        } else {
            continue;
        };
        match inputs {
            Ok(inputs) => {
                for input in inputs {
                    match class_by_path.get(input.as_str()) {
                        Some(TrackedFileClass::Protected) => {}
                        Some(class) => errors.push(format!(
                            "compile input {input} referenced by {path} has forbidden class {class:?}"
                        )),
                        None => errors.push(format!(
                            "compile input {input} referenced by {path} is not tracked"
                        )),
                    }
                }
            }
            Err(error) => errors.push(error),
        }
    }
    finish_errors(errors)
}

fn rust_compile_inputs(root: &Path, stable_path: &str) -> Result<Vec<String>, String> {
    let absolute = root.join(stable_path);
    let source = fs::read_to_string(&absolute)
        .map_err(|error| format!("failed to read compile input owner {stable_path}: {error}"))?;
    let file = syn::parse_file(&source)
        .map_err(|error| format!("failed to parse compile input owner {stable_path}: {error}"))?;
    let source_dir = Path::new(stable_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let module_dir = rust_module_directory(stable_path)?;
    let mut inputs = Vec::new();
    collect_module_inputs(
        root,
        stable_path,
        &file.items,
        &module_dir,
        source_dir,
        &mut inputs,
    )?;
    let mut includes = IncludeMacroVisitor {
        owner: stable_path,
        base: source_dir,
        inputs: Vec::new(),
        errors: Vec::new(),
    };
    includes.visit_file(&file);
    inputs.extend(includes.inputs);
    if !includes.errors.is_empty() {
        return Err(includes.errors.join("\n"));
    }
    inputs.sort();
    inputs.dedup();
    Ok(inputs)
}

fn rust_module_directory(stable_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(stable_path);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("compile input owner has a non-UTF-8 file name: {stable_path}"))?;
    if matches!(name, "lib.rs" | "main.rs" | "mod.rs") {
        Ok(parent.to_path_buf())
    } else {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("compile input owner has a non-UTF-8 stem: {stable_path}"))?;
        Ok(parent.join(stem))
    }
}

fn collect_module_inputs(
    root: &Path,
    owner: &str,
    items: &[Item],
    module_dir: &Path,
    path_attribute_dir: &Path,
    inputs: &mut Vec<String>,
) -> Result<(), String> {
    for item in items {
        let Item::Mod(module) = item else {
            continue;
        };
        let path_attribute = module
            .attrs
            .iter()
            .find(|attribute| attribute.path().is_ident("path"));
        if let Some((_, nested)) = &module.content {
            if path_attribute.is_some() {
                return Err(format!(
                    "unsupported #[path] on inline module {} in {owner}",
                    module.ident
                ));
            }
            collect_module_inputs(
                root,
                owner,
                nested,
                &module_dir.join(module.ident.to_string()),
                &path_attribute_dir.join(module.ident.to_string()),
                inputs,
            )?;
            continue;
        }

        let target = if let Some(attribute) = path_attribute {
            let literal = attribute
                .meta
                .require_name_value()
                .ok()
                .and_then(|value| match &value.value {
                    Expr::Lit(expression) => match &expression.lit {
                        Lit::Str(value) => Some(value.value()),
                        _ => None,
                    },
                    _ => None,
                })
                .ok_or_else(|| {
                    format!(
                        "unsupported dynamic #[path] for module {} in {owner}",
                        module.ident
                    )
                })?;
            normalize_compile_input(path_attribute_dir, &literal)?
        } else {
            let file = module_dir.join(format!("{}.rs", module.ident));
            let nested = module_dir.join(module.ident.to_string()).join("mod.rs");
            let candidates = [file, nested]
                .into_iter()
                .map(|path| normalize_path(&path))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|path| root.join(path).exists())
                .collect::<Vec<_>>();
            match candidates.as_slice() {
                [target] => target.clone(),
                [] => {
                    return Err(format!(
                        "module {} in {owner} has no compile input file",
                        module.ident
                    ));
                }
                _ => {
                    return Err(format!(
                        "module {} in {owner} has ambiguous compile input files",
                        module.ident
                    ));
                }
            }
        };
        inputs.push(target);
    }
    Ok(())
}

struct IncludeMacroVisitor<'a> {
    owner: &'a str,
    base: &'a Path,
    inputs: Vec<String>,
    errors: Vec<String>,
}

impl Visit<'_> for IncludeMacroVisitor<'_> {
    fn visit_macro(&mut self, node: &syn::Macro) {
        let name = node
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        if matches!(name.as_str(), "include" | "include_bytes" | "include_str") {
            match syn::parse2::<LitStr>(node.tokens.clone()) {
                Ok(literal) => match normalize_compile_input(self.base, &literal.value()) {
                    Ok(path) => self.inputs.push(path),
                    Err(error) => self
                        .errors
                        .push(format!("compile input macro in {} {error}", self.owner)),
                },
                Err(_) => self.errors.push(format!(
                    "unsupported dynamic {name}! compile input in {}",
                    self.owner
                )),
            }
        }
        syn::visit::visit_macro(self, node);
    }
}

fn cargo_compile_inputs(root: &Path, stable_path: &str) -> Result<Vec<String>, String> {
    let source = fs::read_to_string(root.join(stable_path))
        .map_err(|error| format!("failed to read Cargo compile input {stable_path}: {error}"))?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("failed to parse Cargo compile input {stable_path}: {error}"))?;
    let manifest_dir = Path::new(stable_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut inputs = Vec::new();

    if let Some(package) = manifest.get("package").and_then(toml::Value::as_table) {
        match package.get("build") {
            Some(toml::Value::Boolean(false)) => {}
            Some(toml::Value::String(path)) => {
                let path = normalize_compile_input(manifest_dir, path)?;
                return Err(format!(
                    "Cargo package {stable_path} uses build script {path}; build scripts require an approved generated-input extractor"
                ));
            }
            Some(_) => {
                return Err(format!(
                    "Cargo package {stable_path} has unsupported dynamic build script declaration"
                ));
            }
            None => {
                let default = normalize_compile_input(manifest_dir, "build.rs")?;
                if root.join(&default).exists() {
                    return Err(format!(
                        "Cargo package {stable_path} uses build script {default}; build scripts require an approved generated-input extractor"
                    ));
                }
            }
        }
    }
    collect_manifest_dependency_inputs(&manifest, manifest_dir, &mut inputs)?;
    inputs.sort();
    inputs.dedup();
    Ok(inputs)
}

fn collect_manifest_dependency_inputs(
    manifest: &toml::Value,
    manifest_dir: &Path,
    inputs: &mut Vec<String>,
) -> Result<(), String> {
    let Some(root) = manifest.as_table() else {
        return Ok(());
    };
    for key in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "replace",
    ] {
        if let Some(table) = root.get(key).and_then(toml::Value::as_table) {
            collect_dependency_table(table, manifest_dir, inputs)?;
        }
    }
    if let Some(workspace) = root.get("workspace").and_then(toml::Value::as_table)
        && let Some(table) = workspace
            .get("dependencies")
            .and_then(toml::Value::as_table)
    {
        collect_dependency_table(table, manifest_dir, inputs)?;
    }
    if let Some(targets) = root.get("target").and_then(toml::Value::as_table) {
        for target in targets.values().filter_map(toml::Value::as_table) {
            for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(table) = target.get(key).and_then(toml::Value::as_table) {
                    collect_dependency_table(table, manifest_dir, inputs)?;
                }
            }
        }
    }
    if let Some(patches) = root.get("patch").and_then(toml::Value::as_table) {
        for patch in patches.values().filter_map(toml::Value::as_table) {
            collect_dependency_table(patch, manifest_dir, inputs)?;
        }
    }
    Ok(())
}

fn collect_dependency_table(
    dependencies: &toml::map::Map<String, toml::Value>,
    manifest_dir: &Path,
    inputs: &mut Vec<String>,
) -> Result<(), String> {
    for (name, dependency) in dependencies {
        if name == "clap"
            || dependency
                .as_table()
                .and_then(|table| table.get("package"))
                .and_then(toml::Value::as_str)
                == Some("clap")
        {
            return Err(
                "clap dependency requires an approved runtime CommandFactory extractor".to_string(),
            );
        }
        let Some(path) = dependency
            .as_table()
            .and_then(|table| table.get("path"))
            .and_then(toml::Value::as_str)
        else {
            continue;
        };
        let directory = normalize_compile_input(manifest_dir, path)?;
        inputs.push(normalize_compile_input(
            Path::new(&directory),
            "Cargo.toml",
        )?);
    }
    Ok(())
}

fn normalize_compile_input(base: &Path, relative: &str) -> Result<String, String> {
    if relative.is_empty() || relative.contains('\\') || Path::new(relative).is_absolute() {
        return Err(format!("has invalid compile input path {relative}"));
    }
    let mut parts = Vec::new();
    for component in base.join(relative).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(format!("compile input escapes workspace: {relative}"));
                }
            }
            Component::CurDir => {}
            _ => return Err(format!("has invalid compile input path {relative}")),
        }
    }
    let path = parts.into_iter().collect::<PathBuf>();
    normalize_path(&path)
}

fn finish_errors(mut errors: Vec<String>) -> Result<(), String> {
    if errors.is_empty() {
        Ok(())
    } else {
        errors.sort();
        errors.dedup();
        Err(errors.join("\n"))
    }
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
    let counts = collector.items.iter().fold(
        HashMap::<(&'static str, String), usize>::new(),
        |mut counts, item| {
            *counts
                .entry((item.kind, item.selector.clone()))
                .or_default() += 1;
            counts
        },
    );
    let mut ordinals = HashMap::<(&'static str, String, String), usize>::new();
    for item in &mut collector.items {
        let key = (item.kind, item.selector.clone());
        if counts.get(&key).copied().unwrap_or_default() > 1 {
            let digest = short_hash(&item.content);
            let ordinal = ordinals
                .entry((item.kind, item.selector.clone(), digest.clone()))
                .or_default();
            item.selector = format!("{}@{digest}:{}", item.selector, *ordinal);
            *ordinal += 1;
        }
    }
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
                    let emitted = is_public(&item.vis) || wire;
                    if emitted {
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
                    }
                    for variant in &item.variants {
                        let selector = format!("variant:{owner}::{}", variant.ident);
                        self.attributes(&selector, &variant.attrs);
                        if emitted {
                            let kind = if is_public(&item.vis) {
                                "rust_public_variant"
                            } else {
                                "rust_wire_variant"
                            };
                            self.push(kind, selector.clone(), variant.to_token_stream());
                        }
                        self.fields(&selector, &variant.fields, emitted, true);
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
                    if item.sig.abi.is_some() {
                        self.push("rust_ffi_item", owner.clone(), item.sig.to_token_stream());
                    }
                    if is_public(&item.vis) {
                        self.push("rust_public_item", owner, item.sig.to_token_stream());
                    }
                }
                Item::ForeignMod(item) => {
                    let owner = qualified(
                        module,
                        &format!(
                            "foreign:{}",
                            short_hash(&item.to_token_stream().to_string())
                        ),
                    );
                    self.attributes(&owner, &item.attrs);
                    self.push("rust_ffi_item", owner, item.to_token_stream());
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
                    let emitted = is_public(&item.vis) || wire;
                    if emitted {
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
                    }
                    self.fields(&owner, &item.fields, emitted, wire);
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
                    if item
                        .trait_
                        .as_ref()
                        .and_then(|(_, path, _)| path.segments.last())
                        .is_some_and(|segment| {
                            matches!(
                                segment.ident.to_string().as_str(),
                                "Serialize" | "Deserialize"
                            )
                        })
                    {
                        self.push("rust_wire_impl", owner.clone(), item.to_token_stream());
                    }
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

    fn fields(&mut self, owner: &str, fields: &Fields, emit: bool, wire: bool) {
        for (index, field) in fields.iter().enumerate() {
            let name = field
                .ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| index.to_string());
            let selector = format!("field:{owner}::{name}");
            self.attributes(&selector, &field.attrs);
            if emit {
                let kind = if wire && !is_public(&field.vis) {
                    "rust_wire_field"
                } else {
                    "rust_public_field"
                };
                self.push(kind, selector, field.to_token_stream());
            }
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
            let attribute_tokens = attribute.to_token_stream().to_string();
            let kind = match name.as_str() {
                "serde" | "value" => "rust_wire_attribute",
                "arg" | "clap" | "command" => "rust_cli_attribute",
                "derive" => "rust_derive_attribute",
                "export_name" | "link" | "link_name" | "no_mangle" | "repr" => "rust_ffi_attribute",
                "unsafe"
                    if attribute_tokens.contains("no_mangle")
                        || attribute_tokens.contains("export_name") =>
                {
                    "rust_ffi_attribute"
                }
                _ if is_inert_rust_attribute(&name) => continue,
                _ => "rust_attribute",
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
        }
        if let Some(identifier) = &item.ident {
            self.push(
                "rust_macro_item",
                qualified(module, &format!("macro:{identifier}")),
                item.to_token_stream(),
            );
        } else {
            self.push(
                "rust_macro_invocation",
                qualified(
                    module,
                    &format!(
                        "macro_call:{name}:{}",
                        short_hash(&item.to_token_stream().to_string())
                    ),
                ),
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

fn is_inert_rust_attribute(name: &str) -> bool {
    matches!(
        name,
        "allow"
            | "cold"
            | "cfg"
            | "cfg_attr"
            | "deny"
            | "deprecated"
            | "doc"
            | "forbid"
            | "ignore"
            | "inline"
            | "macro_export"
            | "must_use"
            | "path"
            | "should_panic"
            | "test"
            | "track_caller"
            | "warn"
    )
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

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct IdentityFlow {
    axes: BTreeSet<&'static str>,
    strings: BTreeSet<String>,
    unsupported: BTreeSet<String>,
    argument_sources: BTreeSet<usize>,
    named_fields: BTreeMap<String, IdentityFlow>,
    indexed_fields: Vec<IdentityFlow>,
    raw_string: bool,
    callable_sensitive: bool,
    mutable_sequence: bool,
}

impl IdentityFlow {
    fn for_axis(axis: &'static str) -> Self {
        Self {
            axes: BTreeSet::from([axis]),
            raw_string: true,
            ..Self::default()
        }
    }

    fn merge(&mut self, other: Self) {
        self.axes.extend(other.axes);
        self.strings.extend(other.strings);
        self.unsupported.extend(other.unsupported);
        self.argument_sources.extend(other.argument_sources);
        for (name, flow) in other.named_fields {
            self.named_fields.entry(name).or_default().merge(flow);
        }
        for (index, flow) in other.indexed_fields.into_iter().enumerate() {
            if let Some(current) = self.indexed_fields.get_mut(index) {
                current.merge(flow);
            } else {
                self.indexed_fields.push(flow);
            }
        }
        self.raw_string |= other.raw_string;
        self.callable_sensitive |= other.callable_sensitive;
        self.mutable_sequence |= other.mutable_sequence;
    }

    fn merged(mut self, other: Self) -> Self {
        self.merge(other);
        self
    }

    fn mark_unsupported(&mut self, construct: impl Into<String>) {
        self.unsupported.insert(construct.into());
    }
}

struct IdentityFlowEnvironment<'a> {
    bindings: &'a [HashMap<String, IdentityFlow>],
    constants: &'a HashMap<String, IdentityFlow>,
    macro_definitions: &'a HashMap<String, TokenStream>,
    callables: &'a CallableSemantics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CallableReturnFlow {
    RawString,
    Aggregate,
    NonIdentity,
    Unknown,
}

#[derive(Clone, Debug, Default)]
struct CallableSemantics {
    functions: HashMap<String, CallableReturnFlow>,
    methods: HashMap<String, CallableReturnFlow>,
    structured_functions: HashMap<String, IdentityFlow>,
    structured_methods: HashMap<String, IdentityFlow>,
    ambiguous_structured_functions: HashSet<String>,
    ambiguous_structured_methods: HashSet<String>,
    sink_functions: HashMap<String, CallableSinkSummary>,
    sink_methods: HashMap<String, CallableSinkSummary>,
    ambiguous_sink_functions: HashSet<String>,
    ambiguous_sink_methods: HashSet<String>,
    local_functions: HashSet<String>,
    local_methods: HashSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CallableSinkSummary {
    comparisons: BTreeSet<(IdentityFlow, IdentityFlow)>,
    unresolved_sources: BTreeSet<usize>,
    all_sources: BTreeSet<usize>,
}

const MAX_CALLABLE_SUMMARY_ITERATIONS: usize = 8;
const MAX_CALLABLE_SINK_EVENTS: usize = 256;
const MAX_CALLABLE_FLOW_DEPTH: usize = 32;
const MAX_CALLABLE_FLOW_NODES: usize = 256;

impl CallableSinkSummary {
    fn is_resolved_empty(&self) -> bool {
        self.comparisons.is_empty() && self.unresolved_sources.is_empty()
    }
}

struct StructuredCallableDefinition {
    names: Vec<String>,
    method: bool,
    return_flow: CallableReturnFlow,
    inputs: syn::punctuated::Punctuated<FnArg, Token![,]>,
    body: syn::Block,
}

struct CallableDefinitionGroup {
    definitions: Vec<StructuredCallableDefinition>,
    sink_definitions: Vec<StructuredCallableDefinition>,
    macro_definitions: HashMap<String, TokenStream>,
}

fn collect_callable_semantics(file: &syn::File) -> CallableSemantics {
    let (mut semantics, definitions) = collect_callable_inventory(file);
    semantics.local_functions = semantics.functions.keys().cloned().collect();
    semantics.local_methods = semantics.methods.keys().cloned().collect();
    derive_callable_summaries(&mut semantics, std::slice::from_ref(&definitions));
    derive_file_callable_sink_summaries(&mut semantics, &definitions);
    semantics
}

fn collect_callable_inventory(file: &syn::File) -> (CallableSemantics, CallableDefinitionGroup) {
    #[derive(Default)]
    struct Collector {
        semantics: CallableSemantics,
        definitions: Vec<StructuredCallableDefinition>,
        sink_definitions: Vec<StructuredCallableDefinition>,
        owner: Option<String>,
    }

    impl Visit<'_> for Collector {
        fn visit_item_fn(&mut self, node: &syn::ItemFn) {
            let name = node.sig.ident.to_string();
            let flow = callable_return_flow(&node.sig.output);
            insert_callable_flow(&mut self.semantics.functions, name.clone(), flow);
            if is_bool_or_unit_return(&node.sig.output) {
                self.sink_definitions.push(StructuredCallableDefinition {
                    names: vec![name.clone()],
                    method: false,
                    return_flow: flow,
                    inputs: node.sig.inputs.clone(),
                    body: (*node.block).clone(),
                });
            }
            if matches!(
                flow,
                CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
            ) {
                self.definitions.push(StructuredCallableDefinition {
                    names: vec![name],
                    method: false,
                    return_flow: flow,
                    inputs: node.sig.inputs.clone(),
                    body: (*node.block).clone(),
                });
            }
            syn::visit::visit_item_fn(self, node);
        }

        fn visit_item_impl(&mut self, node: &syn::ItemImpl) {
            let previous = self.owner.take();
            self.owner = type_owner_name(&node.self_ty);
            syn::visit::visit_item_impl(self, node);
            self.owner = previous;
        }

        fn visit_impl_item_fn(&mut self, node: &syn::ImplItemFn) {
            let name = node.sig.ident.to_string();
            let flow = callable_return_flow(&node.sig.output);
            insert_callable_flow(&mut self.semantics.methods, name.clone(), flow);
            let mut names = vec![name.clone()];
            if let Some(owner) = &self.owner {
                let qualified = format!("{owner}::{name}");
                insert_callable_flow(&mut self.semantics.methods, qualified.clone(), flow);
                names.push(qualified);
            }
            if is_bool_or_unit_return(&node.sig.output) {
                self.sink_definitions.push(StructuredCallableDefinition {
                    names: names.clone(),
                    method: true,
                    return_flow: flow,
                    inputs: node.sig.inputs.clone(),
                    body: node.block.clone(),
                });
            }
            if matches!(
                flow,
                CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
            ) {
                self.definitions.push(StructuredCallableDefinition {
                    names,
                    method: true,
                    return_flow: flow,
                    inputs: node.sig.inputs.clone(),
                    body: node.block.clone(),
                });
            }
            syn::visit::visit_impl_item_fn(self, node);
        }
    }

    let mut collector = Collector::default();
    collector.visit_file(file);
    (
        collector.semantics,
        CallableDefinitionGroup {
            definitions: collector.definitions,
            sink_definitions: collector.sink_definitions,
            macro_definitions: collect_macro_definitions(file),
        },
    )
}

fn derive_callable_summaries(
    semantics: &mut CallableSemantics,
    groups: &[CallableDefinitionGroup],
) {
    let definition_count = groups
        .iter()
        .map(|group| group.definitions.len())
        .sum::<usize>();
    if definition_count == 0 {
        return;
    }
    let local_function_names = groups
        .iter()
        .flat_map(|group| &group.definitions)
        .filter(|definition| !definition.method)
        .flat_map(|definition| &definition.names)
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let local_method_names = groups
        .iter()
        .flat_map(|group| &group.definitions)
        .filter(|definition| definition.method)
        .flat_map(|definition| &definition.names)
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let base_structured_functions = semantics
        .structured_functions
        .iter()
        .filter(|(name, _)| !local_function_names.contains(*name))
        .map(|(name, flow)| (name.clone(), flow.clone()))
        .collect::<HashMap<_, _>>();
    let base_structured_methods = semantics
        .structured_methods
        .iter()
        .filter(|(name, _)| !local_method_names.contains(*name))
        .map(|(name, flow)| (name.clone(), flow.clone()))
        .collect::<HashMap<_, _>>();
    let base_ambiguous_functions = semantics
        .ambiguous_structured_functions
        .iter()
        .filter(|name| !local_function_names.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    let base_ambiguous_methods = semantics
        .ambiguous_structured_methods
        .iter()
        .filter(|name| !local_method_names.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    let constants = HashMap::new();
    let mut converged = false;
    for _ in 0..=definition_count {
        let mut structured_functions = base_structured_functions.clone();
        let mut structured_methods = base_structured_methods.clone();
        let mut ambiguous_structured_functions = base_ambiguous_functions.clone();
        let mut ambiguous_structured_methods = base_ambiguous_methods.clone();

        for group in groups {
            for definition in &group.definitions {
                let bindings = vec![structured_callable_parameter_bindings(&definition.inputs)];
                let mut flow = block_flow(
                    &definition.body,
                    &IdentityFlowEnvironment {
                        bindings: &bindings,
                        constants: &constants,
                        macro_definitions: &group.macro_definitions,
                        callables: semantics,
                    },
                );
                if matches!(definition.return_flow, CallableReturnFlow::RawString) {
                    flow.raw_string = true;
                    flow.callable_sensitive = false;
                } else {
                    flow = aggregate_flow(flow);
                }
                let (summaries, ambiguous) = if definition.method {
                    (&mut structured_methods, &mut ambiguous_structured_methods)
                } else {
                    (
                        &mut structured_functions,
                        &mut ambiguous_structured_functions,
                    )
                };
                for name in &definition.names {
                    insert_structured_callable_flow(summaries, ambiguous, name, flow.clone());
                }
            }
        }

        if semantics.structured_functions == structured_functions
            && semantics.structured_methods == structured_methods
            && semantics.ambiguous_structured_functions == ambiguous_structured_functions
            && semantics.ambiguous_structured_methods == ambiguous_structured_methods
        {
            converged = true;
            break;
        }

        semantics.structured_functions = structured_functions;
        semantics.structured_methods = structured_methods;
        semantics.ambiguous_structured_functions = ambiguous_structured_functions;
        semantics.ambiguous_structured_methods = ambiguous_structured_methods;
    }

    if !converged {
        for flow in semantics
            .structured_functions
            .values_mut()
            .chain(semantics.structured_methods.values_mut())
        {
            flow.mark_unsupported("callable return summary did not converge");
        }
    }
}

fn derive_file_callable_sink_summaries(
    semantics: &mut CallableSemantics,
    group: &CallableDefinitionGroup,
) {
    semantics.sink_functions.clear();
    semantics.sink_methods.clear();
    semantics.ambiguous_sink_functions.clear();
    semantics.ambiguous_sink_methods.clear();
    if group.sink_definitions.is_empty() {
        return;
    }

    let function_name_counts = callable_definition_name_counts(&group.sink_definitions, false);
    let method_name_counts = callable_definition_name_counts(&group.sink_definitions, true);
    let iteration_limit = group
        .sink_definitions
        .len()
        .saturating_add(1)
        .min(MAX_CALLABLE_SUMMARY_ITERATIONS);
    let mut converged = false;

    for _ in 0..iteration_limit {
        let mut sink_functions = HashMap::new();
        let mut sink_methods = HashMap::new();
        let mut ambiguous_sink_functions = HashSet::new();
        let mut ambiguous_sink_methods = HashSet::new();

        for definition in &group.sink_definitions {
            let bindings = vec![structured_callable_parameter_bindings(&definition.inputs)];
            let summary = callable_sink_summary(
                &definition.body,
                bindings,
                &group.macro_definitions,
                semantics,
            );
            let (summaries, ambiguous, name_counts) = if definition.method {
                (
                    &mut sink_methods,
                    &mut ambiguous_sink_methods,
                    &method_name_counts,
                )
            } else {
                (
                    &mut sink_functions,
                    &mut ambiguous_sink_functions,
                    &function_name_counts,
                )
            };
            for name in &definition.names {
                let normalized_name = name.to_ascii_lowercase();
                if summary.is_resolved_empty() {
                    summaries.remove(&normalized_name);
                    continue;
                }
                if name_counts.get(&normalized_name).copied().unwrap_or(0) > 1 {
                    summaries.remove(&normalized_name);
                    ambiguous.insert(normalized_name);
                    continue;
                }
                insert_callable_sink_summary(summaries, ambiguous, name, summary.clone());
            }
        }

        if semantics.sink_functions == sink_functions
            && semantics.sink_methods == sink_methods
            && semantics.ambiguous_sink_functions == ambiguous_sink_functions
            && semantics.ambiguous_sink_methods == ambiguous_sink_methods
        {
            converged = true;
            break;
        }
        semantics.sink_functions = sink_functions;
        semantics.sink_methods = sink_methods;
        semantics.ambiguous_sink_functions = ambiguous_sink_functions;
        semantics.ambiguous_sink_methods = ambiguous_sink_methods;
    }

    if !converged {
        for summary in semantics
            .sink_functions
            .values_mut()
            .chain(semantics.sink_methods.values_mut())
        {
            summary.unresolved_sources = summary.all_sources.clone();
        }
    }
}

fn callable_definition_name_counts(
    definitions: &[StructuredCallableDefinition],
    methods: bool,
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for name in definitions
        .iter()
        .filter(|definition| definition.method == methods)
        .flat_map(|definition| &definition.names)
    {
        *counts.entry(name.to_ascii_lowercase()).or_default() += 1;
    }
    counts
}

fn structured_callable_parameter_bindings(
    inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
) -> HashMap<String, IdentityFlow> {
    let mut bindings = HashMap::new();
    for (index, input) in inputs.iter().enumerate() {
        match input {
            FnArg::Receiver(_) => {
                bindings.insert(
                    "self".to_string(),
                    IdentityFlow {
                        argument_sources: BTreeSet::from([index]),
                        ..IdentityFlow::default()
                    },
                );
            }
            FnArg::Typed(input) => {
                let Some((name, _)) = local_binding(&input.pat) else {
                    continue;
                };
                let mut flow = if binding_exposes_raw_identity(&name, Some(&input.ty)) {
                    IdentityFlow::for_axis(exact_identity_axis(&name).expect("identity parameter"))
                } else if type_is_raw_identity(&input.ty) {
                    IdentityFlow {
                        raw_string: true,
                        ..IdentityFlow::default()
                    }
                } else {
                    IdentityFlow::default()
                };
                flow.argument_sources.insert(index);
                bindings.insert(name, flow);
            }
        }
    }
    bindings
}

fn type_owner_name(kind: &Type) -> Option<String> {
    let Type::Path(path) = kind else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn collect_workspace_callable_semantics(files: &[PathBuf]) -> Result<CallableSemantics, String> {
    let mut semantics = CallableSemantics::default();
    let mut definitions = Vec::new();
    for file in files {
        if file.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let source = fs::read_to_string(file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let parsed = syn::parse_file(&source)
            .map_err(|error| format!("failed to parse {}: {error}", file.display()))?;
        let (inventory, mut group) = collect_callable_inventory(&parsed);
        merge_callable_semantics(&mut semantics, inventory);
        group.sink_definitions.clear();
        definitions.push(group);
    }
    derive_callable_summaries(&mut semantics, &definitions);
    Ok(semantics)
}

fn collect_file_callable_semantics(
    file: &Path,
    workspace: &CallableSemantics,
) -> Result<CallableSemantics, String> {
    let source = fs::read_to_string(file)
        .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
    let parsed = syn::parse_file(&source)
        .map_err(|error| format!("failed to parse {}: {error}", file.display()))?;
    Ok(specialize_callable_semantics(&parsed, workspace))
}

fn specialize_callable_semantics(
    file: &syn::File,
    workspace: &CallableSemantics,
) -> CallableSemantics {
    let (inventory, definitions) = collect_callable_inventory(file);
    let local_functions = inventory.functions.keys().cloned().collect();
    let local_methods = inventory.methods.keys().cloned().collect();
    let mut semantics = workspace.clone();
    overlay_callable_inventory(&mut semantics, inventory);
    semantics.local_functions = local_functions;
    semantics.local_methods = local_methods;
    derive_callable_summaries(&mut semantics, std::slice::from_ref(&definitions));
    derive_file_callable_sink_summaries(&mut semantics, &definitions);
    semantics
}

fn overlay_callable_inventory(target: &mut CallableSemantics, source: CallableSemantics) {
    for (name, flow) in source.functions {
        target.functions.insert(name, flow);
    }
    for (name, flow) in source.methods {
        target.methods.insert(name, flow);
    }
    for (name, summary) in source.sink_functions {
        target.sink_functions.insert(name, summary);
    }
    for (name, summary) in source.sink_methods {
        target.sink_methods.insert(name, summary);
    }
}

fn merge_callable_semantics(target: &mut CallableSemantics, source: CallableSemantics) {
    for (name, flow) in source.functions {
        insert_callable_flow(&mut target.functions, name, flow);
    }
    for (name, flow) in source.methods {
        insert_callable_flow(&mut target.methods, name, flow);
    }
    for name in source.ambiguous_structured_functions {
        target.structured_functions.remove(&name);
        target.ambiguous_structured_functions.insert(name);
    }
    for name in source.ambiguous_structured_methods {
        target.structured_methods.remove(&name);
        target.ambiguous_structured_methods.insert(name);
    }
    for (name, flow) in source.structured_functions {
        insert_structured_callable_flow(
            &mut target.structured_functions,
            &mut target.ambiguous_structured_functions,
            &name,
            flow,
        );
    }
    for (name, flow) in source.structured_methods {
        insert_structured_callable_flow(
            &mut target.structured_methods,
            &mut target.ambiguous_structured_methods,
            &name,
            flow,
        );
    }
    for name in source.ambiguous_sink_functions {
        target.sink_functions.remove(&name);
        target.ambiguous_sink_functions.insert(name);
    }
    for name in source.ambiguous_sink_methods {
        target.sink_methods.remove(&name);
        target.ambiguous_sink_methods.insert(name);
    }
    for (name, summary) in source.sink_functions {
        insert_callable_sink_summary(
            &mut target.sink_functions,
            &mut target.ambiguous_sink_functions,
            &name,
            summary,
        );
    }
    for (name, summary) in source.sink_methods {
        insert_callable_sink_summary(
            &mut target.sink_methods,
            &mut target.ambiguous_sink_methods,
            &name,
            summary,
        );
    }
}

fn insert_callable_flow(
    callables: &mut HashMap<String, CallableReturnFlow>,
    name: String,
    flow: CallableReturnFlow,
) {
    callables
        .entry(name.to_ascii_lowercase())
        .and_modify(|current| {
            if *current != flow {
                *current = CallableReturnFlow::Unknown;
            }
        })
        .or_insert(flow);
}

fn insert_structured_callable_flow(
    callables: &mut HashMap<String, IdentityFlow>,
    ambiguous: &mut HashSet<String>,
    name: &str,
    flow: IdentityFlow,
) {
    let name = name.to_ascii_lowercase();
    if ambiguous.contains(&name) {
        return;
    }
    if callables.get(&name).is_some_and(|current| *current != flow) {
        callables.remove(&name);
        ambiguous.insert(name);
    } else {
        callables.entry(name).or_insert(flow);
    }
}

fn insert_callable_sink_summary(
    callables: &mut HashMap<String, CallableSinkSummary>,
    ambiguous: &mut HashSet<String>,
    name: &str,
    summary: CallableSinkSummary,
) {
    let name = name.to_ascii_lowercase();
    if ambiguous.contains(&name) {
        return;
    }
    if callables
        .get(&name)
        .is_some_and(|current| *current != summary)
    {
        callables.remove(&name);
        ambiguous.insert(name);
    } else {
        callables.entry(name).or_insert(summary);
    }
}

fn callable_return_flow(output: &syn::ReturnType) -> CallableReturnFlow {
    match output {
        syn::ReturnType::Default => CallableReturnFlow::NonIdentity,
        syn::ReturnType::Type(_, kind) => type_return_flow(kind),
    }
}

fn is_bool_or_unit_return(output: &syn::ReturnType) -> bool {
    match output {
        syn::ReturnType::Default => true,
        syn::ReturnType::Type(_, kind) => is_bool_or_unit_type(kind),
    }
}

fn is_bool_or_unit_type(kind: &Type) -> bool {
    match kind {
        Type::Reference(reference) => is_bool_or_unit_type(&reference.elem),
        Type::Paren(paren) => is_bool_or_unit_type(&paren.elem),
        Type::Group(group) => is_bool_or_unit_type(&group.elem),
        Type::Tuple(tuple) => tuple.elems.is_empty(),
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "bool"),
        _ => false,
    }
}

fn type_return_flow(kind: &Type) -> CallableReturnFlow {
    match kind {
        Type::Reference(reference) => type_return_flow(&reference.elem),
        Type::Paren(paren) => type_return_flow(&paren.elem),
        Type::Group(group) => type_return_flow(&group.elem),
        Type::Array(array) => aggregate_return_flow(type_return_flow(&array.elem)),
        Type::Slice(slice) => aggregate_return_flow(type_return_flow(&slice.elem)),
        Type::Tuple(tuple) => tuple
            .elems
            .iter()
            .fold(CallableReturnFlow::NonIdentity, |current, element| {
                merge_return_flow(current, type_return_flow(element))
            }),
        Type::Never(_) => CallableReturnFlow::NonIdentity,
        Type::Path(path) => path_return_flow(path),
        _ => CallableReturnFlow::Unknown,
    }
}

fn path_return_flow(path: &syn::TypePath) -> CallableReturnFlow {
    let Some(segment) = path.path.segments.last() else {
        return CallableReturnFlow::Unknown;
    };
    let name = segment.ident.to_string();
    if ["String", "str"].contains(&name.as_str()) {
        return CallableReturnFlow::RawString;
    }
    if [
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16",
        "u32", "u64", "u128", "usize",
    ]
    .contains(&name.as_str())
    {
        return CallableReturnFlow::NonIdentity;
    }
    let inner = first_type_argument(&segment.arguments).map(type_return_flow);
    if ["ArrayVec", "BTreeSet", "HashSet", "Vec", "VecDeque"].contains(&name.as_str()) {
        return inner.map_or(CallableReturnFlow::Unknown, aggregate_return_flow);
    }
    if ["Arc", "Box", "Cow", "Option", "Rc"].contains(&name.as_str()) {
        return inner.unwrap_or(CallableReturnFlow::Unknown);
    }
    if !segment.arguments.is_empty()
        && (name == "Result" || name.ends_with("Result") || name.ends_with("Outcome"))
    {
        return inner.unwrap_or(CallableReturnFlow::Unknown);
    }
    if segment.arguments.is_empty() {
        if name.len() == 1 && name.as_bytes()[0].is_ascii_uppercase() {
            CallableReturnFlow::Unknown
        } else {
            CallableReturnFlow::Aggregate
        }
    } else {
        CallableReturnFlow::Unknown
    }
}

fn first_type_argument(arguments: &PathArguments) -> Option<&Type> {
    let PathArguments::AngleBracketed(arguments) = arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| {
        let GenericArgument::Type(kind) = argument else {
            return None;
        };
        Some(kind)
    })
}

fn aggregate_return_flow(flow: CallableReturnFlow) -> CallableReturnFlow {
    match flow {
        CallableReturnFlow::RawString | CallableReturnFlow::Aggregate => {
            CallableReturnFlow::Aggregate
        }
        other => other,
    }
}

fn merge_return_flow(left: CallableReturnFlow, right: CallableReturnFlow) -> CallableReturnFlow {
    if left == right {
        return left;
    }
    if matches!(left, CallableReturnFlow::Unknown) || matches!(right, CallableReturnFlow::Unknown) {
        return CallableReturnFlow::Unknown;
    }
    if matches!(
        left,
        CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
    ) || matches!(
        right,
        CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
    ) {
        CallableReturnFlow::Aggregate
    } else {
        CallableReturnFlow::NonIdentity
    }
}

struct IdentityBranchVisitor<'a> {
    path: &'a str,
    violations: Vec<String>,
    errors: Vec<String>,
    comparisons: BTreeSet<(IdentityFlow, IdentityFlow)>,
    direct: BTreeSet<IdentityFlow>,
    summary_event_limit: Option<usize>,
    summary_overflow: bool,
    bindings: Vec<HashMap<String, IdentityFlow>>,
    constants: HashMap<String, IdentityFlow>,
    macro_definitions: HashMap<String, TokenStream>,
    callables: &'a CallableSemantics,
    return_axis: Option<&'static str>,
}

impl Visit<'_> for IdentityBranchVisitor<'_> {
    fn visit_stmt(&mut self, node: &syn::Stmt) {
        if let syn::Stmt::Expr(Expr::Call(call), Some(_)) = node {
            let arguments = call
                .args
                .iter()
                .map(|argument| self.flow(argument))
                .collect::<Vec<_>>();
            if !callable_sink_is_registered(&call.func, self.callables)
                && !known_callable_path(&call.func, self.callables)
            {
                let name = callable_name(&call.func).unwrap_or_else(|| "<expression>".to_string());
                self.record_direct(opaque_flow(
                    merge_identity_flows(arguments),
                    format!("statement call {name} has unresolved sink semantics"),
                ));
            }
        }
        syn::visit::visit_stmt(self, node);
    }

    fn visit_block(&mut self, node: &syn::Block) {
        self.bindings.push(HashMap::new());
        syn::visit::visit_block(self, node);
        self.bindings.pop();
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if let Some(initializer) = &node.init {
            let initializer_flow = self.flow(&initializer.expr);
            if let Some((name, declared_type)) = local_binding(&node.pat)
                && binding_exposes_raw_identity(&name, declared_type)
                && (declared_type.is_some() || initializer_flow.raw_string)
            {
                self.record_expected(
                    exact_identity_axis(&name).expect("identity local"),
                    initializer_flow.clone(),
                );
            }
            let pattern_bindings = flows_for_pattern(&node.pat, initializer_flow);
            syn::visit::visit_local(self, node);
            self.bindings
                .last_mut()
                .expect("identity scope")
                .extend(pattern_bindings);
            return;
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &syn::ExprForLoop) {
        let iterator_flow = self.flow(&node.expr);
        self.visit_expr(&node.expr);
        self.bindings
            .push(flows_for_pattern(&node.pat, iterator_flow));
        self.visit_block(&node.body);
        self.bindings.pop();
    }

    fn visit_expr_binary(&mut self, node: &ExprBinary) {
        if matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_)) {
            self.record_comparison(self.flow(&node.left), self.flow(&node.right));
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &syn::ExprMethodCall) {
        let method = node.method.to_string();
        let receiver = self.flow(&node.receiver);
        if method == "push" && receiver.mutable_sequence && node.args.len() == 1 {
            let value = self.flow(node.args.first().expect("push argument"));
            let mut updated = receiver.clone();
            updated.merge(flow_summary(&value));
            updated.indexed_fields.push(value);
            write_assignment_flow(&node.receiver, updated, &mut self.bindings);
        }
        let mut method_arguments = vec![receiver.clone()];
        method_arguments.extend(node.args.iter().map(|argument| self.flow(argument)));
        let local_method = explicit_method_qualified_name(&node.receiver, &method)
            .is_some_and(|name| self.callables.local_methods.contains(&name));
        let higher_order = has_higher_order_semantics(&method);
        let reserved_semantic = is_identity_comparison(&method) || higher_order;

        if local_method && reserved_semantic {
            let mut flow = merge_identity_flows(method_arguments);
            flow.mark_unsupported(format!(
                "local method {method} collides with reserved semantic method"
            ));
            self.record_direct(flow);
            syn::visit::visit_expr_method_call(self, node);
            return;
        }
        if is_identity_comparison(&method) {
            for argument in &node.args {
                self.record_comparison(receiver.clone(), self.flow(argument));
            }
        }
        if !reserved_semantic {
            self.record_method_sinks(&node.receiver, &method, &method_arguments);
        }

        if higher_order {
            if !higher_order_call_shape_supported(&method, node.args.len()) {
                let mut flow = merge_identity_flows(method_arguments);
                flow.mark_unsupported(format!(
                    "method {method} has unsupported higher-order call shape"
                ));
                self.record_direct(flow);
                syn::visit::visit_expr_method_call(self, node);
                return;
            }
            self.visit_expr(&node.receiver);
            for (index, argument) in node.args.iter().enumerate() {
                let Some(inputs) = higher_order_closure_inputs(
                    &method,
                    index,
                    &node.receiver,
                    &receiver,
                    &node.args,
                    &self.environment(),
                ) else {
                    self.visit_expr(argument);
                    continue;
                };
                match argument {
                    Expr::Closure(closure) => {
                        self.visit_closure_with_arguments(closure, &inputs);
                    }
                    Expr::Path(_) => {
                        if !self.record_callable_sinks(argument, &inputs)
                            && !known_callable_path(argument, self.callables)
                        {
                            let flow = opaque_flow(
                                merge_identity_flows(inputs),
                                format!("method {method} has unresolved callable routing"),
                            );
                            self.record_direct(flow);
                        }
                        self.visit_expr(argument);
                    }
                    _ => {
                        self.visit_expr(argument);
                        let mut flow = merge_identity_flows(inputs);
                        flow.mark_unsupported(format!(
                            "method {method} has unresolved closure routing"
                        ));
                        self.record_direct(flow);
                    }
                }
            }
            return;
        }

        if node
            .args
            .iter()
            .any(|argument| matches!(argument, Expr::Closure(_)))
        {
            let mut flow = merge_identity_flows(method_arguments);
            flow.mark_unsupported(format!(
                "method {method} has unregistered closure semantics"
            ));
            self.record_direct(flow);
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &syn::ExprCall) {
        let callable = callable_name(&node.func);
        let callable_flow = self.callable_flow(&node.func);
        let arguments = node
            .args
            .iter()
            .map(|argument| self.flow(argument))
            .collect::<Vec<_>>();
        if callable.as_deref().is_some_and(is_identity_comparison)
            && !callable
                .as_deref()
                .is_some_and(|name| self.callables.local_functions.contains(name))
        {
            let mut arguments = arguments.iter();
            if let Some(first) = arguments.next() {
                for argument in arguments {
                    self.record_comparison(first.clone(), argument.clone());
                }
            }
        } else if callable_flow.callable_sensitive {
            for argument in &arguments {
                self.record_comparison(callable_flow.clone(), argument.clone());
            }
        }
        self.record_callable_sinks(&node.func, &arguments);
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_match(&mut self, node: &ExprMatch) {
        let subject = self.flow(&node.expr);
        let mut patterns = IdentityFlow::default();
        for arm in &node.arms {
            let mut strings = PatternStringVisitor::default();
            strings.visit_pat(&arm.pat);
            patterns.strings.extend(strings.values);
        }
        if !patterns.strings.is_empty() {
            patterns.raw_string = true;
            self.record_comparison(subject.clone(), patterns);
        }
        self.visit_expr(&node.expr);
        for arm in &node.arms {
            self.visit_pat(&arm.pat);
            self.bindings
                .push(flows_for_pattern(&arm.pat, subject.clone()));
            if let Some((_, guard)) = &arm.guard {
                self.visit_expr(guard);
            }
            self.visit_expr(&arm.body);
            self.bindings.pop();
        }
    }

    fn visit_expr_if(&mut self, node: &syn::ExprIf) {
        self.visit_expr(&node.cond);
        let baseline = self.bindings.clone();

        self.bindings = baseline.clone();
        let pattern_scope = if let Expr::Let(binding) = node.cond.as_ref() {
            Some(flows_for_pattern(&binding.pat, self.flow(&binding.expr)))
        } else {
            None
        };
        if let Some(pattern_scope) = pattern_scope {
            self.bindings.push(pattern_scope);
        }
        self.visit_block(&node.then_branch);
        if matches!(node.cond.as_ref(), Expr::Let(_)) {
            self.bindings.pop();
        }
        let then_bindings = self.bindings.clone();

        self.bindings = baseline.clone();
        if let Some((_, otherwise)) = &node.else_branch {
            self.visit_expr(otherwise);
        }
        self.bindings = merge_binding_states(then_bindings, std::mem::take(&mut self.bindings));
    }

    fn visit_expr_while(&mut self, node: &syn::ExprWhile) {
        let Expr::Let(binding) = node.cond.as_ref() else {
            syn::visit::visit_expr_while(self, node);
            return;
        };

        let subject = self.flow(&binding.expr);
        self.visit_expr(&node.cond);
        self.bindings.push(flows_for_pattern(&binding.pat, subject));
        self.visit_block(&node.body);
        self.bindings.pop();
    }

    fn visit_expr_let(&mut self, node: &syn::ExprLet) {
        let mut strings = PatternStringVisitor::default();
        strings.visit_pat(&node.pat);
        if !strings.values.is_empty() {
            self.record_comparison(
                self.flow(&node.expr),
                IdentityFlow {
                    strings: strings.values.into_iter().collect(),
                    raw_string: true,
                    ..IdentityFlow::default()
                },
            );
        }
        syn::visit::visit_expr_let(self, node);
    }

    fn visit_expr_macro(&mut self, node: &syn::ExprMacro) {
        if node.mac.path.is_ident("matches") {
            match syn::parse2::<MatchesExpression>(node.mac.tokens.clone()) {
                Ok(matches) => {
                    let subject = self.flow(&matches.expression);
                    let mut strings = PatternStringVisitor::default();
                    strings.visit_pat(&matches.pattern);
                    self.record_comparison(
                        subject.clone(),
                        IdentityFlow {
                            strings: strings.values.into_iter().collect(),
                            raw_string: true,
                            ..IdentityFlow::default()
                        },
                    );
                    self.visit_expr(&matches.expression);
                    if let Some(guard) = &matches.guard {
                        self.bindings
                            .push(flows_for_pattern(&matches.pattern, subject));
                        self.visit_expr(guard);
                        self.bindings.pop();
                    }
                }
                Err(error) => self.errors.push(format!(
                    "{}: failed to parse matches! identity expression: {error}",
                    self.path
                )),
            }
        } else {
            let flow = self.flow(&Expr::Macro(node.clone()));
            if flow.callable_sensitive {
                self.record_direct(flow);
            }
        }
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_expr_struct(&mut self, node: &syn::ExprStruct) {
        for field in &node.fields {
            if let Member::Named(identifier) = &field.member
                && let Some(axis) = exact_identity_axis(&identifier.to_string())
            {
                self.record_expected(axis, self.flow(&field.expr));
            }
        }
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_assign(&mut self, node: &syn::ExprAssign) {
        let right = self.flow(&node.right);
        for axis in assignment_identity_axes(&node.left, &self.environment()) {
            self.record_expected(axis, right.clone());
        }
        write_assignment_flow(&node.left, right, &mut self.bindings);
        syn::visit::visit_expr_assign(self, node);
    }

    fn visit_expr_return(&mut self, node: &syn::ExprReturn) {
        if let (Some(axis), Some(expression)) = (self.return_axis, &node.expr) {
            self.record_expected(axis, self.flow(expression));
        }
        syn::visit::visit_expr_return(self, node);
    }

    fn visit_item_const(&mut self, node: &syn::ItemConst) {
        if binding_exposes_raw_identity(&node.ident.to_string(), Some(&node.ty)) {
            self.record_expected(
                exact_identity_axis(&node.ident.to_string()).expect("identity const"),
                self.flow(&node.expr),
            );
        }
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &syn::ItemStatic) {
        if binding_exposes_raw_identity(&node.ident.to_string(), Some(&node.ty)) {
            self.record_expected(
                exact_identity_axis(&node.ident.to_string()).expect("identity static"),
                self.flow(&node.expr),
            );
        }
        syn::visit::visit_item_static(self, node);
    }

    fn visit_item_fn(&mut self, node: &syn::ItemFn) {
        self.push_parameter_scope(&node.sig.inputs);
        let previous = self.return_axis;
        self.return_axis = identity_return_axis(&node.sig.ident.to_string());
        if let Some(axis) = self.return_axis {
            self.record_expected(axis, self.flow_block(&node.block));
        }
        syn::visit::visit_item_fn(self, node);
        self.return_axis = previous;
        self.bindings.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &syn::ImplItemFn) {
        self.push_parameter_scope(&node.sig.inputs);
        let previous = self.return_axis;
        self.return_axis = identity_return_axis(&node.sig.ident.to_string());
        if let Some(axis) = self.return_axis {
            self.record_expected(axis, self.flow_block(&node.block));
        }
        syn::visit::visit_impl_item_fn(self, node);
        self.return_axis = previous;
        self.bindings.pop();
    }
}

impl IdentityBranchVisitor<'_> {
    fn environment(&self) -> IdentityFlowEnvironment<'_> {
        IdentityFlowEnvironment {
            bindings: &self.bindings,
            constants: &self.constants,
            macro_definitions: &self.macro_definitions,
            callables: self.callables,
        }
    }

    fn flow(&self, expression: &Expr) -> IdentityFlow {
        expression_flow(expression, &self.environment())
    }

    fn flow_block(&self, block: &syn::Block) -> IdentityFlow {
        block_flow(block, &self.environment())
    }

    fn callable_flow(&self, expression: &Expr) -> IdentityFlow {
        let Expr::Path(path) = expression else {
            return self.flow(expression);
        };
        let Some(name) = path.path.get_ident().map(ToString::to_string) else {
            return IdentityFlow::default();
        };
        lookup_binding(&name, &self.bindings).unwrap_or_default()
    }

    fn record_callable_sinks(&mut self, callable: &Expr, arguments: &[IdentityFlow]) -> bool {
        let name = callable_name(callable);
        let qualified = callable_qualified_name(callable);
        let (summary, ambiguous) = if callable_path_has_explicit_type_owner(callable) {
            (
                qualified
                    .as_deref()
                    .and_then(|name| self.callables.sink_methods.get(name))
                    .cloned(),
                qualified
                    .as_deref()
                    .is_some_and(|name| self.callables.ambiguous_sink_methods.contains(name)),
            )
        } else if callable_path_is_unqualified(callable) {
            (
                name.as_deref()
                    .and_then(|name| self.callables.sink_functions.get(name))
                    .cloned(),
                name.as_deref()
                    .is_some_and(|name| self.callables.ambiguous_sink_functions.contains(name)),
            )
        } else {
            (None, false)
        };

        if let Some(summary) = summary {
            self.apply_callable_sink_summary(&summary, arguments);
            true
        } else if ambiguous {
            let mut flow = merge_identity_flows(arguments.iter().cloned());
            flow.mark_unsupported("callable sink summary is ambiguous");
            self.record_direct(flow);
            true
        } else {
            false
        }
    }

    fn record_method_sinks(&mut self, receiver: &Expr, method: &str, arguments: &[IdentityFlow]) {
        let method = method.to_ascii_lowercase();
        let qualified = explicit_method_qualified_name(receiver, &method);
        let summary = qualified
            .as_deref()
            .and_then(|name| self.callables.sink_methods.get(name))
            .or_else(|| self.callables.sink_methods.get(&method))
            .cloned();
        let ambiguous = qualified
            .as_deref()
            .is_some_and(|name| self.callables.ambiguous_sink_methods.contains(name))
            || self.callables.ambiguous_sink_methods.contains(&method);
        if summary.is_none() && !ambiguous {
            return;
        }
        if let Some(summary) = summary {
            self.apply_callable_sink_summary(&summary, arguments);
        } else {
            let mut flow = merge_identity_flows(arguments.iter().cloned());
            flow.mark_unsupported("method sink summary is ambiguous");
            self.record_direct(flow);
        }
    }

    fn apply_callable_sink_summary(
        &mut self,
        summary: &CallableSinkSummary,
        arguments: &[IdentityFlow],
    ) {
        for (left, right) in &summary.comparisons {
            self.record_comparison(
                instantiate_structured_callable_flow(left, arguments),
                instantiate_structured_callable_flow(right, arguments),
            );
        }
        if !summary.unresolved_sources.is_empty() {
            let mut flow = merge_identity_flows(
                summary
                    .unresolved_sources
                    .iter()
                    .filter_map(|source| arguments.get(*source).cloned()),
            );
            flow.mark_unsupported("callable sink summary is unresolved");
            self.record_direct(flow);
        }
    }

    fn visit_closure_with_arguments(
        &mut self,
        closure: &syn::ExprClosure,
        arguments: &[IdentityFlow],
    ) {
        let mut bindings = HashMap::new();
        for (index, pattern) in closure.inputs.iter().enumerate() {
            if let Some(argument) = arguments.get(index) {
                bindings.extend(flows_for_pattern(pattern, argument.clone()));
            }
        }
        self.bindings.push(bindings);
        self.visit_expr(&closure.body);
        self.bindings.pop();
    }

    fn push_parameter_scope(&mut self, inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>) {
        let mut bindings = HashMap::new();
        for input in inputs {
            let FnArg::Typed(input) = input else {
                continue;
            };
            if let Some((name, _)) = local_binding(&input.pat) {
                let flow = if binding_exposes_raw_identity(&name, Some(&input.ty)) {
                    IdentityFlow::for_axis(exact_identity_axis(&name).expect("identity parameter"))
                } else if type_is_raw_identity(&input.ty) {
                    IdentityFlow {
                        raw_string: true,
                        ..IdentityFlow::default()
                    }
                } else {
                    IdentityFlow::default()
                };
                bindings.insert(name, flow);
            }
        }
        self.bindings.push(bindings);
    }

    fn store_comparison(&mut self, left: IdentityFlow, right: IdentityFlow) {
        let event = (left, right);
        if self.comparisons.contains(&event) {
            return;
        }
        if self
            .summary_event_limit
            .is_some_and(|limit| self.comparisons.len() + self.direct.len() >= limit)
        {
            self.summary_overflow = true;
            return;
        }
        self.comparisons.insert(event);
    }

    fn store_direct(&mut self, flow: IdentityFlow) {
        if self.direct.contains(&flow) {
            return;
        }
        if self
            .summary_event_limit
            .is_some_and(|limit| self.comparisons.len() + self.direct.len() >= limit)
        {
            self.summary_overflow = true;
            return;
        }
        self.direct.insert(flow);
    }

    fn record_comparison(&mut self, left: IdentityFlow, right: IdentityFlow) {
        self.store_comparison(left.clone(), right.clone());
        for axis in &left.axes {
            for value in &right.strings {
                self.violations.push(format!(
                    "{}: raw identity string {value:?} on axis {axis}",
                    self.path
                ));
            }
        }
        for axis in &right.axes {
            for value in &left.strings {
                self.violations.push(format!(
                    "{}: raw identity string {value:?} on axis {axis}",
                    self.path
                ));
            }
        }
        let axes = left.axes.union(&right.axes).copied().collect::<Vec<_>>();
        let unsupported = left
            .unsupported
            .union(&right.unsupported)
            .cloned()
            .collect::<Vec<_>>();
        self.record_unsupported(&axes, &unsupported);
    }

    fn record_expected(&mut self, axis: &'static str, value: IdentityFlow) {
        self.store_comparison(IdentityFlow::for_axis(axis), value.clone());
        for string in &value.strings {
            self.violations.push(format!(
                "{}: raw identity string {string:?} on axis {axis}",
                self.path
            ));
        }
        self.record_unsupported(&[axis], &value.unsupported.into_iter().collect::<Vec<_>>());
    }

    fn record_direct(&mut self, flow: IdentityFlow) {
        self.store_direct(flow.clone());
        for axis in &flow.axes {
            for value in &flow.strings {
                self.violations.push(format!(
                    "{}: raw identity string {value:?} on axis {axis}",
                    self.path
                ));
            }
        }
        self.record_unsupported(
            &flow.axes.iter().copied().collect::<Vec<_>>(),
            &flow.unsupported.iter().cloned().collect::<Vec<_>>(),
        );
    }

    fn record_unsupported(&mut self, axes: &[&str], unsupported: &[String]) {
        for axis in axes {
            for construct in unsupported {
                self.violations.push(format!(
                    "{}: incomplete identity analysis on axis {axis}: {construct}",
                    self.path
                ));
            }
        }
    }
}

fn callable_sink_summary(
    body: &syn::Block,
    bindings: Vec<HashMap<String, IdentityFlow>>,
    macro_definitions: &HashMap<String, TokenStream>,
    callables: &CallableSemantics,
) -> CallableSinkSummary {
    let all_sources = bindings
        .iter()
        .flat_map(|scope| scope.values())
        .flat_map(|flow| flow.argument_sources.iter().copied())
        .collect::<BTreeSet<_>>();
    let mut visitor = IdentityBranchVisitor {
        path: "<callable-summary>",
        violations: Vec::new(),
        errors: Vec::new(),
        comparisons: BTreeSet::new(),
        direct: BTreeSet::new(),
        summary_event_limit: Some(MAX_CALLABLE_SINK_EVENTS),
        summary_overflow: false,
        bindings,
        constants: HashMap::new(),
        macro_definitions: macro_definitions.clone(),
        callables,
        return_axis: None,
    };
    visitor.visit_block(body);
    let mut unresolved_sources = visitor
        .direct
        .iter()
        .flat_map(|flow| flattened_identity_flow(flow).argument_sources)
        .collect::<BTreeSet<_>>();
    if !visitor.errors.is_empty() || visitor.summary_overflow {
        unresolved_sources.extend(all_sources.iter().copied());
    }
    CallableSinkSummary {
        comparisons: visitor.comparisons,
        unresolved_sources,
        all_sources,
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
    guard: Option<Expr>,
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
            guard,
        })
    }
}

fn collect_macro_definitions(file: &syn::File) -> HashMap<String, TokenStream> {
    #[derive(Default)]
    struct MacroDefinitionCollector {
        definitions: HashMap<String, TokenStream>,
    }

    impl Visit<'_> for MacroDefinitionCollector {
        fn visit_item_macro(&mut self, node: &syn::ItemMacro) {
            if let Some(name) = &node.ident {
                self.definitions
                    .insert(name.to_string(), node.mac.tokens.clone());
            }
            syn::visit::visit_item_macro(self, node);
        }
    }

    let mut collector = MacroDefinitionCollector::default();
    collector.visit_file(file);
    collector.definitions
}

fn collect_constant_flows(
    file: &syn::File,
    macro_definitions: &HashMap<String, TokenStream>,
    callables: &CallableSemantics,
) -> HashMap<String, IdentityFlow> {
    #[derive(Default)]
    struct ConstantCollector {
        expressions: Vec<(String, Type, Expr)>,
    }

    impl Visit<'_> for ConstantCollector {
        fn visit_item_const(&mut self, node: &syn::ItemConst) {
            self.expressions.push((
                node.ident.to_string(),
                (*node.ty).clone(),
                (*node.expr).clone(),
            ));
            syn::visit::visit_item_const(self, node);
        }

        fn visit_item_static(&mut self, node: &syn::ItemStatic) {
            self.expressions.push((
                node.ident.to_string(),
                (*node.ty).clone(),
                (*node.expr).clone(),
            ));
            syn::visit::visit_item_static(self, node);
        }
    }

    let mut collector = ConstantCollector::default();
    collector.visit_file(file);
    let mut constants = HashMap::new();
    let bindings = vec![HashMap::new()];
    for _ in 0..=collector.expressions.len() {
        let previous = constants.clone();
        for (name, kind, expression) in &collector.expressions {
            let environment = IdentityFlowEnvironment {
                bindings: &bindings,
                constants: &previous,
                macro_definitions,
                callables,
            };
            let flow =
                flow_for_binding(name, Some(kind), expression_flow(expression, &environment));
            constants.insert(name.clone(), flow);
        }
        if constants == previous {
            break;
        }
    }
    constants
}

fn expression_flow(expression: &Expr, environment: &IdentityFlowEnvironment<'_>) -> IdentityFlow {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(value) => IdentityFlow {
                strings: BTreeSet::from([value.value()]),
                raw_string: true,
                ..IdentityFlow::default()
            },
            _ => IdentityFlow::default(),
        },
        Expr::Path(path) => path_flow(&path.path, environment),
        Expr::Field(field) => match &field.member {
            Member::Named(identifier) => {
                let name = identifier.to_string();
                let mut base = expression_flow(&field.base, environment);
                if let Some(flow) = base.named_fields.remove(&name) {
                    return flow;
                }
                if let Some(axis) = exact_identity_axis(&name) {
                    base.axes = BTreeSet::from([axis]);
                    base.strings.clear();
                    base.argument_sources.clear();
                    base.raw_string = false;
                    return base;
                }
                if base.unsupported.is_empty() {
                    IdentityFlow::default()
                } else {
                    base.strings.clear();
                    base.named_fields.clear();
                    base.indexed_fields.clear();
                    base
                }
            }
            Member::Unnamed(index) => {
                let base = expression_flow(&field.base, environment);
                base.indexed_fields
                    .get(index.index as usize)
                    .cloned()
                    .unwrap_or(base)
            }
        },
        Expr::Array(array) => sequence_flow(array.elems.iter(), environment),
        Expr::Tuple(tuple) => sequence_flow(tuple.elems.iter(), environment),
        Expr::Reference(reference) => expression_flow(&reference.expr, environment),
        Expr::Paren(paren) => expression_flow(&paren.expr, environment),
        Expr::Group(group) => expression_flow(&group.expr, environment),
        Expr::Try(expression) => expression_flow(&expression.expr, environment),
        Expr::Await(expression) => expression_flow(&expression.base, environment),
        Expr::Cast(expression) => expression_flow(&expression.expr, environment),
        Expr::Unary(expression) => expression_flow(&expression.expr, environment),
        Expr::Index(expression) => {
            let base = expression_flow(&expression.expr, environment);
            literal_index(&expression.index)
                .and_then(|index| base.indexed_fields.get(index).cloned())
                .unwrap_or(base)
        }
        Expr::Block(expression) => block_flow(&expression.block, environment),
        Expr::Const(expression) => block_flow(&expression.block, environment),
        Expr::TryBlock(expression) => block_flow(&expression.block, environment),
        Expr::Unsafe(expression) => block_flow(&expression.block, environment),
        Expr::If(expression) => if_expression_flow(expression, environment),
        Expr::Match(expression) => match_expression_flow(expression, environment),
        Expr::Let(_) => IdentityFlow::default(),
        Expr::Call(call) => call_flow(call, environment),
        Expr::MethodCall(call) => method_call_flow(call, environment),
        Expr::Macro(expression) => macro_flow(&expression.mac, environment),
        Expr::Closure(closure) => closure_flow(closure, environment),
        Expr::Struct(expression) => {
            let mut flow = IdentityFlow::default();
            for field in &expression.fields {
                let field_flow = expression_flow(&field.expr, environment);
                flow.merge(flow_summary(&field_flow));
                flow.named_fields
                    .entry(field.member.to_token_stream().to_string())
                    .or_default()
                    .merge(field_flow);
            }
            if let Some(rest) = &expression.rest {
                flow.merge(expression_flow(rest, environment));
            }
            flow.strings.clear();
            flow.raw_string = false;
            flow
        }
        Expr::Repeat(expression) => expression_flow(&expression.expr, environment),
        Expr::Range(expression) => {
            let mut flow = IdentityFlow::default();
            if let Some(start) = &expression.start {
                flow.merge(expression_flow(start, environment));
            }
            if let Some(end) = &expression.end {
                flow.merge(expression_flow(end, environment));
            }
            flow
        }
        Expr::Binary(binary) if matches!(binary.op, BinOp::Eq(_) | BinOp::Ne(_)) => {
            IdentityFlow::default()
        }
        Expr::Binary(binary) => opaque_flow(
            expression_flow(&binary.left, environment)
                .merged(expression_flow(&binary.right, environment)),
            "binary expression",
        ),
        Expr::Async(expression) => {
            opaque_flow(block_flow(&expression.block, environment), "async block")
        }
        Expr::Break(expression) => expression
            .expr
            .as_deref()
            .map(|value| expression_flow(value, environment))
            .unwrap_or_default(),
        Expr::Return(expression) => expression
            .expr
            .as_deref()
            .map(|value| expression_flow(value, environment))
            .unwrap_or_default(),
        Expr::Yield(expression) => expression
            .expr
            .as_deref()
            .map(|value| expression_flow(value, environment))
            .unwrap_or_default(),
        _ => opaque_flow(
            token_stream_flow(expression.to_token_stream(), environment),
            "unregistered expression construct",
        ),
    }
}

fn if_expression_flow(
    expression: &syn::ExprIf,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let mut flow = if let Expr::Let(binding) = expression.cond.as_ref() {
        let mut bindings = environment.bindings.to_vec();
        bindings.push(flows_for_pattern(
            &binding.pat,
            expression_flow(&binding.expr, environment),
        ));
        block_flow(
            &expression.then_branch,
            &IdentityFlowEnvironment {
                bindings: &bindings,
                constants: environment.constants,
                macro_definitions: environment.macro_definitions,
                callables: environment.callables,
            },
        )
    } else {
        block_flow(&expression.then_branch, environment)
    };
    if let Some((_, otherwise)) = &expression.else_branch {
        flow.merge(expression_flow(otherwise, environment));
    }
    flow
}

fn match_expression_flow(
    expression: &ExprMatch,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let subject = expression_flow(&expression.expr, environment);
    let mut result = IdentityFlow::default();
    for arm in &expression.arms {
        let mut bindings = environment.bindings.to_vec();
        bindings.push(flows_for_pattern(&arm.pat, subject.clone()));
        result.merge(expression_flow(
            &arm.body,
            &IdentityFlowEnvironment {
                bindings: &bindings,
                constants: environment.constants,
                macro_definitions: environment.macro_definitions,
                callables: environment.callables,
            },
        ));
    }
    result
}

fn assignment_identity_axes(
    expression: &Expr,
    environment: &IdentityFlowEnvironment<'_>,
) -> BTreeSet<&'static str> {
    match expression {
        Expr::Path(_) => expression_flow(expression, environment).axes,
        Expr::Field(field) => match &field.member {
            Member::Named(identifier) => exact_identity_axis(&identifier.to_string())
                .into_iter()
                .collect(),
            Member::Unnamed(_) => BTreeSet::new(),
        },
        Expr::Reference(reference) => assignment_identity_axes(&reference.expr, environment),
        Expr::Paren(paren) => assignment_identity_axes(&paren.expr, environment),
        Expr::Group(group) => assignment_identity_axes(&group.expr, environment),
        _ => BTreeSet::new(),
    }
}

#[derive(Clone, Debug)]
enum AssignmentProjection {
    Named(String),
    Indexed(usize),
}

fn write_assignment_flow(
    target: &Expr,
    value: IdentityFlow,
    bindings: &mut [HashMap<String, IdentityFlow>],
) {
    match target {
        Expr::Tuple(tuple) => {
            for (index, element) in tuple.elems.iter().enumerate() {
                let projected = value
                    .indexed_fields
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| projection_fallback(&value));
                write_assignment_flow(element, projected, bindings);
            }
        }
        Expr::Array(array) => {
            for (index, element) in array.elems.iter().enumerate() {
                let projected = value
                    .indexed_fields
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| projection_fallback(&value));
                write_assignment_flow(element, projected, bindings);
            }
        }
        _ => {
            let Some((name, projections)) = assignment_target(target) else {
                return;
            };
            let Some(binding) = bindings
                .iter_mut()
                .rev()
                .find_map(|scope| scope.get_mut(&name))
            else {
                return;
            };
            if projections.is_empty() {
                *binding = flow_for_binding(&name, None, value);
            } else {
                write_projected_flow(binding, &projections, value);
            }
        }
    }
}

fn assignment_target(expression: &Expr) -> Option<(String, Vec<AssignmentProjection>)> {
    match expression {
        Expr::Path(path) if path.path.segments.len() == 1 => {
            Some((path.path.segments[0].ident.to_string(), Vec::new()))
        }
        Expr::Field(field) => {
            let (name, mut projections) = assignment_target(&field.base)?;
            projections.push(match &field.member {
                Member::Named(identifier) => AssignmentProjection::Named(identifier.to_string()),
                Member::Unnamed(index) => AssignmentProjection::Indexed(index.index as usize),
            });
            Some((name, projections))
        }
        Expr::Index(index) => {
            let (name, mut projections) = assignment_target(&index.expr)?;
            projections.push(AssignmentProjection::Indexed(literal_index(&index.index)?));
            Some((name, projections))
        }
        Expr::Reference(reference) => assignment_target(&reference.expr),
        Expr::Paren(paren) => assignment_target(&paren.expr),
        Expr::Group(group) => assignment_target(&group.expr),
        Expr::Unary(unary) => assignment_target(&unary.expr),
        _ => None,
    }
}

fn write_projected_flow(
    target: &mut IdentityFlow,
    projections: &[AssignmentProjection],
    mut value: IdentityFlow,
) {
    let Some((projection, remaining)) = projections.split_first() else {
        *target = value;
        return;
    };
    match projection {
        AssignmentProjection::Named(name) => {
            if remaining.is_empty() {
                value = flow_for_binding(name, None, value);
            }
            write_projected_flow(
                target.named_fields.entry(name.clone()).or_default(),
                remaining,
                value,
            );
        }
        AssignmentProjection::Indexed(index) => {
            if target.indexed_fields.len() <= *index {
                target
                    .indexed_fields
                    .resize_with(index + 1, IdentityFlow::default);
            }
            write_projected_flow(&mut target.indexed_fields[*index], remaining, value);
        }
    }
}

fn call_flow(call: &syn::ExprCall, environment: &IdentityFlowEnvironment<'_>) -> IdentityFlow {
    let name = callable_name(&call.func);
    if name.as_deref().is_some_and(is_identity_comparison)
        && !name
            .as_deref()
            .is_some_and(|name| environment.callables.local_functions.contains(name))
    {
        return IdentityFlow::default();
    }
    if is_source_independent_string_call(&call.func) {
        return IdentityFlow {
            raw_string: true,
            ..IdentityFlow::default()
        };
    }
    if is_mutable_sequence_constructor(&call.func) {
        return IdentityFlow {
            mutable_sequence: true,
            ..IdentityFlow::default()
        };
    }
    let mut flow = callable_binding_flow(&call.func, environment);
    flow.merge(merge_expressions(call.args.iter(), environment));
    if is_intrinsic_transparent_callable_path(&call.func) {
        return flow;
    }
    let semantic_rule = callable_qualified_name(&call.func)
        .as_deref()
        .and_then(|name| environment.callables.methods.get(name))
        .or_else(|| {
            name.as_deref()
                .and_then(|name| environment.callables.functions.get(name))
        });
    if let Some(rule) = semantic_rule {
        if matches!(
            rule,
            CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
        ) && let Some(summary) =
            structured_callable_summary(&call.func, name.as_deref(), environment)
        {
            let arguments = call
                .args
                .iter()
                .map(|argument| expression_flow(argument, environment))
                .collect::<Vec<_>>();
            return instantiate_structured_callable_flow(summary, &arguments);
        }
        return apply_callable_return(flow, *rule);
    }
    if is_transparent_function_call(call, name.as_deref()) {
        return flow;
    }
    opaque_flow(
        flow,
        format!(
            "call {}",
            name.unwrap_or_else(|| "<expression>".to_string())
        ),
    )
}

fn method_call_flow(
    call: &syn::ExprMethodCall,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let method = call.method.to_string();
    let normalized_method = method.to_ascii_lowercase();
    let mut receiver = expression_flow(&call.receiver, environment);
    let local_method = explicit_method_qualified_name(&call.receiver, &method)
        .filter(|name| environment.callables.local_methods.contains(name));
    if let Some(local_method) = local_method {
        let mut arguments = vec![receiver.clone()];
        arguments.extend(
            call.args
                .iter()
                .map(|argument| expression_flow(argument, environment)),
        );
        let Some(rule) = environment.callables.methods.get(&local_method) else {
            return opaque_flow(
                merge_identity_flows(arguments),
                format!("local method {method} has no return semantics"),
            );
        };
        if matches!(
            rule,
            CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
        ) && let Some(summary) = environment.callables.structured_methods.get(&local_method)
        {
            return instantiate_structured_callable_flow(summary, &arguments);
        }
        return apply_callable_return(merge_identity_flows(arguments), *rule);
    }
    if environment
        .callables
        .local_methods
        .contains(&normalized_method)
        && matches!(method_flow_rule(&method), MethodFlowRule::NonIdentity)
        && environment
            .callables
            .methods
            .get(&normalized_method)
            .is_none_or(|flow| !matches!(flow, CallableReturnFlow::NonIdentity))
    {
        receiver.merge(merge_expressions(call.args.iter(), environment));
        return opaque_flow(
            receiver,
            format!("local method {method} conflicts with built-in nonidentity semantics"),
        );
    }
    if is_identity_comparison(&method) || is_higher_order_nonidentity_result(&method) {
        return IdentityFlow::default();
    }
    match method_flow_rule(&method) {
        MethodFlowRule::Receiver => receiver,
        MethodFlowRule::ReceiverAndArguments => {
            receiver.merge(merge_expressions(call.args.iter(), environment));
            receiver
        }
        MethodFlowRule::Aggregate => aggregate_flow(receiver),
        MethodFlowRule::Transform => transform_method_flow(call, receiver, environment),
        MethodFlowRule::MapOr => map_or_method_flow(call, receiver, environment),
        MethodFlowRule::Fold => fold_method_flow(call, receiver, environment),
        MethodFlowRule::Reduce => reduce_method_flow(call, receiver, environment),
        MethodFlowRule::NonIdentity => IdentityFlow::default(),
        MethodFlowRule::Opaque => {
            receiver.merge(merge_expressions(call.args.iter(), environment));
            if let Some(axis) = identity_accessor_axis(&method) {
                receiver.axes.insert(axis);
                return apply_callable_return(receiver, CallableReturnFlow::RawString);
            }
            if let Some(rule) = environment.callables.methods.get(&method) {
                if matches!(
                    rule,
                    CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
                ) && let Some(summary) = environment.callables.structured_methods.get(&method)
                {
                    let mut arguments = vec![expression_flow(&call.receiver, environment)];
                    arguments.extend(
                        call.args
                            .iter()
                            .map(|argument| expression_flow(argument, environment)),
                    );
                    return instantiate_structured_callable_flow(summary, &arguments);
                }
                return apply_callable_return(receiver, *rule);
            }
            opaque_flow(receiver, format!("method {method}"))
        }
    }
}

fn map_or_method_flow(
    call: &syn::ExprMethodCall,
    receiver: IdentityFlow,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    if call.args.len() != 2 {
        return opaque_flow(
            receiver.merged(merge_expressions(call.args.iter(), environment)),
            format!("method {} with unsupported arity", call.method),
        );
    }
    let mut flow = if call.method == "map_or_else" {
        callable_result_flow_with_arguments(&call.args[0], &[], environment)
    } else {
        expression_flow(&call.args[0], environment)
    };
    flow.merge(callable_result_flow_with_arguments(
        &call.args[1],
        &[receiver],
        environment,
    ));
    flow
}

fn fold_method_flow(
    call: &syn::ExprMethodCall,
    receiver: IdentityFlow,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    if call.args.len() != 2 {
        return opaque_flow(
            receiver.merged(merge_expressions(call.args.iter(), environment)),
            format!("method {} with unsupported arity", call.method),
        );
    }
    let initial = expression_flow(&call.args[0], environment);
    callable_result_flow_with_arguments(&call.args[1], &[initial, receiver], environment)
}

fn reduce_method_flow(
    call: &syn::ExprMethodCall,
    receiver: IdentityFlow,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    if call.args.len() != 1 {
        return opaque_flow(
            receiver.merged(merge_expressions(call.args.iter(), environment)),
            format!("method {} with unsupported arity", call.method),
        );
    }
    callable_result_flow_with_arguments(&call.args[0], &[receiver.clone(), receiver], environment)
}

fn transform_method_flow(
    call: &syn::ExprMethodCall,
    receiver: IdentityFlow,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let Some(transform) = call.args.first() else {
        return opaque_flow(
            receiver,
            format!("method {} without transform", call.method),
        );
    };
    if call.args.len() != 1 {
        return opaque_flow(
            receiver.merged(merge_expressions(call.args.iter(), environment)),
            format!("method {} with unsupported transform arity", call.method),
        );
    }
    callable_result_flow_with_arguments(transform, &[receiver], environment)
}

fn callable_result_flow_with_arguments(
    callable: &Expr,
    arguments: &[IdentityFlow],
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    if let Expr::Closure(closure) = callable {
        return closure_return_flow_with_arguments(closure, environment, arguments);
    }

    let mut flow = merge_identity_flows(arguments.iter().cloned());
    let name = callable_name(callable);
    if is_intrinsic_transparent_callable_path(callable) && arguments.len() == 1 {
        return flow;
    }
    let semantic_rule = callable_qualified_name(callable)
        .as_deref()
        .and_then(|name| environment.callables.methods.get(name))
        .or_else(|| {
            name.as_deref()
                .and_then(|name| environment.callables.functions.get(name))
        });
    if let Some(rule) = semantic_rule {
        if matches!(
            rule,
            CallableReturnFlow::RawString | CallableReturnFlow::Aggregate
        ) && let Some(summary) =
            structured_callable_summary(callable, name.as_deref(), environment)
        {
            return instantiate_structured_callable_flow(summary, arguments);
        }
        return apply_callable_return(flow, *rule);
    }
    if is_transparent_callable_path(callable, name.as_deref()) && arguments.len() == 1 {
        return flow;
    }
    flow.merge(expression_flow(callable, environment));
    opaque_flow(
        flow,
        format!(
            "transform callable {}",
            name.unwrap_or_else(|| "<expression>".to_string())
        ),
    )
}

fn structured_callable_summary<'a>(
    expression: &Expr,
    name: Option<&str>,
    environment: &'a IdentityFlowEnvironment<'_>,
) -> Option<&'a IdentityFlow> {
    callable_qualified_name(expression)
        .as_deref()
        .and_then(|name| environment.callables.structured_methods.get(name))
        .or_else(|| name.and_then(|name| environment.callables.structured_functions.get(name)))
}

fn instantiate_structured_callable_flow(
    summary: &IdentityFlow,
    arguments: &[IdentityFlow],
) -> IdentityFlow {
    let mut remaining_nodes = MAX_CALLABLE_FLOW_NODES;
    instantiate_structured_callable_flow_bounded(summary, arguments, 0, &mut remaining_nodes)
}

fn instantiate_structured_callable_flow_bounded(
    summary: &IdentityFlow,
    arguments: &[IdentityFlow],
    depth: usize,
    remaining_nodes: &mut usize,
) -> IdentityFlow {
    if depth >= MAX_CALLABLE_FLOW_DEPTH || *remaining_nodes == 0 {
        let mut flow = flattened_identity_flow(summary);
        let sources = std::mem::take(&mut flow.argument_sources);
        for source in sources {
            if let Some(argument) = arguments.get(source) {
                flow.merge(flattened_identity_flow(argument));
            } else {
                flow.mark_unsupported(format!(
                    "structured callable source {source} has no matching argument"
                ));
            }
        }
        if !flow.axes.is_empty() || !flow.strings.is_empty() || !flow.argument_sources.is_empty() {
            flow.mark_unsupported("structured callable flow exceeded analysis bounds");
        }
        return flow;
    }
    *remaining_nodes -= 1;

    let mut flow = flow_summary(summary);
    let mut direct_unsupported = summary.unsupported.clone();
    for nested in summary
        .named_fields
        .values()
        .chain(summary.indexed_fields.iter())
    {
        direct_unsupported.retain(|construct| !nested.unsupported.contains(construct));
    }
    flow.unsupported = direct_unsupported;
    flow.argument_sources.clear();
    for source in &summary.argument_sources {
        if let Some(argument) = arguments.get(*source) {
            flow.merge(clone_identity_flow_bounded(
                argument,
                depth,
                remaining_nodes,
            ));
        } else {
            flow.mark_unsupported(format!(
                "structured callable source {source} has no matching argument"
            ));
        }
    }
    flow.named_fields.clear();
    flow.indexed_fields.clear();
    for (name, field) in &summary.named_fields {
        let field = instantiate_structured_callable_flow_bounded(
            field,
            arguments,
            depth + 1,
            remaining_nodes,
        );
        if !field.axes.is_empty() || !field.argument_sources.is_empty() {
            flow.unsupported.extend(field.unsupported.iter().cloned());
        }
        flow.named_fields.insert(name.clone(), field);
    }
    for field in &summary.indexed_fields {
        let field = instantiate_structured_callable_flow_bounded(
            field,
            arguments,
            depth + 1,
            remaining_nodes,
        );
        if !field.axes.is_empty() || !field.argument_sources.is_empty() {
            flow.unsupported.extend(field.unsupported.iter().cloned());
        }
        flow.indexed_fields.push(field);
    }
    if flow.axes.is_empty() && flow.argument_sources.is_empty() {
        flow.unsupported.clear();
    }
    flow
}

fn clone_identity_flow_bounded(
    source: &IdentityFlow,
    depth: usize,
    remaining_nodes: &mut usize,
) -> IdentityFlow {
    if depth >= MAX_CALLABLE_FLOW_DEPTH || *remaining_nodes == 0 {
        let mut flow = flattened_identity_flow(source);
        if !flow.axes.is_empty() || !flow.argument_sources.is_empty() {
            flow.mark_unsupported("structured callable flow exceeded analysis bounds");
        }
        return flow;
    }
    *remaining_nodes -= 1;
    let mut flow = flow_summary(source);
    for (name, field) in &source.named_fields {
        flow.named_fields.insert(
            name.clone(),
            clone_identity_flow_bounded(field, depth + 1, remaining_nodes),
        );
    }
    for field in &source.indexed_fields {
        flow.indexed_fields.push(clone_identity_flow_bounded(
            field,
            depth + 1,
            remaining_nodes,
        ));
    }
    flow
}

fn flattened_identity_flow(source: &IdentityFlow) -> IdentityFlow {
    fn collect(source: &IdentityFlow, target: &mut IdentityFlow) {
        target.axes.extend(source.axes.iter().copied());
        target.strings.extend(source.strings.iter().cloned());
        target
            .unsupported
            .extend(source.unsupported.iter().cloned());
        target
            .argument_sources
            .extend(source.argument_sources.iter().copied());
        target.raw_string |= source.raw_string;
        target.callable_sensitive |= source.callable_sensitive;
        target.mutable_sequence |= source.mutable_sequence;
        for field in source
            .named_fields
            .values()
            .chain(source.indexed_fields.iter())
        {
            collect(field, target);
        }
    }

    let mut flow = IdentityFlow::default();
    collect(source, &mut flow);
    flow
}

fn apply_callable_return(mut flow: IdentityFlow, rule: CallableReturnFlow) -> IdentityFlow {
    match rule {
        CallableReturnFlow::RawString => {
            flow.strings.clear();
            flow.raw_string = true;
            flow
        }
        CallableReturnFlow::Aggregate => {
            flow.strings.clear();
            aggregate_flow(flow)
        }
        CallableReturnFlow::NonIdentity => IdentityFlow::default(),
        CallableReturnFlow::Unknown => opaque_flow(flow, "callable with unresolved return flow"),
    }
}

fn callable_qualified_name(expression: &Expr) -> Option<String> {
    let Expr::Path(path) = expression else {
        return None;
    };
    let mut segments = path.path.segments.iter().rev();
    let callable = segments.next()?.ident.to_string();
    let owner = segments.next()?.ident.to_string();
    Some(format!("{owner}::{callable}").to_ascii_lowercase())
}

fn explicit_method_qualified_name(receiver: &Expr, method: &str) -> Option<String> {
    let owner = match receiver {
        Expr::Path(path) => path.path.segments.last()?.ident.to_string(),
        Expr::Struct(structure) => structure.path.segments.last()?.ident.to_string(),
        Expr::Reference(reference) => {
            return explicit_method_qualified_name(&reference.expr, method);
        }
        Expr::Paren(paren) => return explicit_method_qualified_name(&paren.expr, method),
        Expr::Group(group) => return explicit_method_qualified_name(&group.expr, method),
        _ => return None,
    };
    if !owner.chars().next().is_some_and(char::is_uppercase) {
        return None;
    }
    Some(format!("{owner}::{method}").to_ascii_lowercase())
}

fn opaque_flow(mut flow: IdentityFlow, construct: impl Into<String>) -> IdentityFlow {
    if flow.axes.is_empty() && flow.argument_sources.is_empty() {
        return IdentityFlow::default();
    }
    flow.strings.clear();
    flow.named_fields.clear();
    flow.indexed_fields.clear();
    flow.mark_unsupported(construct);
    flow
}

fn closure_flow(
    closure: &syn::ExprClosure,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    closure_flow_with_arguments(closure, environment, &[])
}

fn closure_flow_with_arguments(
    closure: &syn::ExprClosure,
    environment: &IdentityFlowEnvironment<'_>,
    arguments: &[IdentityFlow],
) -> IdentityFlow {
    closure_flow_with_arguments_mode(closure, environment, arguments, true)
}

fn closure_return_flow_with_arguments(
    closure: &syn::ExprClosure,
    environment: &IdentityFlowEnvironment<'_>,
    arguments: &[IdentityFlow],
) -> IdentityFlow {
    closure_flow_with_arguments_mode(closure, environment, arguments, false)
}

fn closure_flow_with_arguments_mode(
    closure: &syn::ExprClosure,
    environment: &IdentityFlowEnvironment<'_>,
    arguments: &[IdentityFlow],
    include_comparison_context: bool,
) -> IdentityFlow {
    let mut bindings = environment.bindings.to_vec();
    let mut parameters = HashMap::new();
    for (index, pattern) in closure.inputs.iter().enumerate() {
        if let Some(argument) = arguments.get(index) {
            parameters.extend(flows_for_pattern(pattern, argument.clone()));
            continue;
        }
        if let Some((name, kind)) = local_binding(pattern) {
            parameters.insert(
                name.clone(),
                if binding_exposes_raw_identity(&name, kind) {
                    IdentityFlow::for_axis(exact_identity_axis(&name).expect("identity closure"))
                } else if kind.is_some_and(type_is_raw_identity) {
                    IdentityFlow {
                        raw_string: true,
                        ..IdentityFlow::default()
                    }
                } else {
                    IdentityFlow::default()
                },
            );
        }
    }
    bindings.push(parameters);
    let nested = IdentityFlowEnvironment {
        bindings: &bindings,
        constants: environment.constants,
        macro_definitions: environment.macro_definitions,
        callables: environment.callables,
    };
    let mut flow = expression_flow(&closure.body, &nested);
    let callable_sensitive =
        include_comparison_context && contains_identity_comparison(&closure.body);
    if callable_sensitive {
        flow.merge(lexical_expression_flow(&closure.body, &nested));
    }
    flow.callable_sensitive = callable_sensitive;
    flow
}

fn lexical_expression_flow(
    expression: &Expr,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    struct Collector<'a, 'b> {
        environment: &'a IdentityFlowEnvironment<'b>,
        flow: IdentityFlow,
    }

    impl Visit<'_> for Collector<'_, '_> {
        fn visit_expr(&mut self, node: &Expr) {
            self.flow.merge(expression_flow(node, self.environment));
            syn::visit::visit_expr(self, node);
        }
    }

    let mut collector = Collector {
        environment,
        flow: IdentityFlow::default(),
    };
    collector.visit_expr(expression);
    collector.flow
}

fn block_flow(block: &syn::Block, environment: &IdentityFlowEnvironment<'_>) -> IdentityFlow {
    let mut bindings = environment.bindings.to_vec();
    bindings.push(HashMap::new());
    let mut result = IdentityFlow::default();
    for statement in &block.stmts {
        let nested = IdentityFlowEnvironment {
            bindings: &bindings,
            constants: environment.constants,
            macro_definitions: environment.macro_definitions,
            callables: environment.callables,
        };
        match statement {
            syn::Stmt::Local(local) => {
                if let Some(initializer) = &local.init {
                    let flow = expression_flow(&initializer.expr, &nested);
                    bindings
                        .last_mut()
                        .expect("block scope")
                        .extend(flows_for_pattern(&local.pat, flow));
                }
            }
            syn::Stmt::Item(syn::Item::Const(item)) => {
                let flow = flow_for_binding(
                    &item.ident.to_string(),
                    Some(&item.ty),
                    expression_flow(&item.expr, &nested),
                );
                bindings
                    .last_mut()
                    .expect("block scope")
                    .insert(item.ident.to_string(), flow);
            }
            syn::Stmt::Expr(expression, terminator) => {
                if let Expr::Assign(assignment) = expression {
                    let value = expression_flow(&assignment.right, &nested);
                    write_assignment_flow(&assignment.left, value, &mut bindings);
                }
                if terminator.is_none() {
                    let nested = IdentityFlowEnvironment {
                        bindings: &bindings,
                        constants: environment.constants,
                        macro_definitions: environment.macro_definitions,
                        callables: environment.callables,
                    };
                    result = expression_flow(expression, &nested);
                }
            }
            syn::Stmt::Item(_) | syn::Stmt::Macro(_) => {}
        }
    }
    result
}

fn merge_expressions<'a>(
    expressions: impl IntoIterator<Item = &'a Expr>,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let mut flow = IdentityFlow::default();
    for expression in expressions {
        flow.merge(expression_flow(expression, environment));
    }
    flow
}

fn merge_identity_flows(flows: impl IntoIterator<Item = IdentityFlow>) -> IdentityFlow {
    let mut result = IdentityFlow::default();
    for flow in flows {
        result.merge(flow);
    }
    result
}

fn sequence_flow<'a>(
    expressions: impl IntoIterator<Item = &'a Expr>,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let mut aggregate = IdentityFlow::default();
    for expression in expressions {
        let flow = expression_flow(expression, environment);
        aggregate.merge(flow_summary(&flow));
        aggregate.indexed_fields.push(flow);
    }
    aggregate_flow(aggregate)
}

fn literal_index(expression: &Expr) -> Option<usize> {
    let Expr::Lit(literal) = expression else {
        return None;
    };
    let Lit::Int(value) = &literal.lit else {
        return None;
    };
    value.base10_parse().ok()
}

fn aggregate_flow(mut flow: IdentityFlow) -> IdentityFlow {
    flow.raw_string = false;
    flow
}

fn flow_summary(flow: &IdentityFlow) -> IdentityFlow {
    let mut summary = flow.clone();
    summary.named_fields.clear();
    summary.indexed_fields.clear();
    summary
}

fn path_flow(path: &syn::Path, environment: &IdentityFlowEnvironment<'_>) -> IdentityFlow {
    let Some(segment) = path.segments.last() else {
        return IdentityFlow::default();
    };
    let name = segment.ident.to_string();
    if path.segments.len() == 1
        && let Some(flow) = lookup_binding(&name, environment.bindings)
    {
        return flow;
    }
    if let Some(flow) = environment.constants.get(&name) {
        return flow.clone();
    }
    if path.segments.len() == 1 {
        exact_identity_axis(&name)
            .map(IdentityFlow::for_axis)
            .unwrap_or_default()
    } else {
        IdentityFlow::default()
    }
}

fn callable_binding_flow(
    expression: &Expr,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let Expr::Path(path) = expression else {
        return expression_flow(expression, environment);
    };
    let Some(name) = path.path.get_ident().map(ToString::to_string) else {
        return IdentityFlow::default();
    };
    lookup_binding(&name, environment.bindings).unwrap_or_default()
}

fn lookup_binding(name: &str, bindings: &[HashMap<String, IdentityFlow>]) -> Option<IdentityFlow> {
    bindings
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
}

fn merge_binding_states(
    mut left: Vec<HashMap<String, IdentityFlow>>,
    right: Vec<HashMap<String, IdentityFlow>>,
) -> Vec<HashMap<String, IdentityFlow>> {
    assert_eq!(
        left.len(),
        right.len(),
        "identity binding scopes must balance across control-flow branches"
    );
    for (left_scope, right_scope) in left.iter_mut().zip(right) {
        for (name, flow) in right_scope {
            left_scope.entry(name).or_default().merge(flow);
        }
    }
    left
}

fn flow_for_binding(
    name: &str,
    declared_type: Option<&Type>,
    mut initializer: IdentityFlow,
) -> IdentityFlow {
    if let Some(axis) = exact_identity_axis(name) {
        match declared_type {
            Some(kind) if type_is_raw_identity(kind) => {
                initializer.axes.insert(axis);
            }
            Some(_) => {
                initializer.axes.clear();
            }
            None if initializer.raw_string => {
                initializer.axes.insert(axis);
            }
            None => {}
        }
    }
    initializer
}

fn binding_exposes_raw_identity(name: &str, declared_type: Option<&Type>) -> bool {
    exact_identity_axis(name).is_some() && declared_type.is_none_or(type_is_raw_identity)
}

fn callable_name(expression: &Expr) -> Option<String> {
    let Expr::Path(path) = expression else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string().to_ascii_lowercase())
}

fn known_callable_path(expression: &Expr, callables: &CallableSemantics) -> bool {
    if is_intrinsic_transparent_callable_path(expression) {
        return true;
    }
    let name = callable_name(expression);
    if is_transparent_callable_path(expression, name.as_deref()) {
        return true;
    }
    if callable_path_has_explicit_type_owner(expression)
        && callable_qualified_name(expression)
            .as_deref()
            .is_some_and(|qualified| callables.methods.contains_key(qualified))
        || callable_path_is_unqualified(expression)
            && name
                .as_deref()
                .is_some_and(|name| callables.functions.contains_key(name))
    {
        return true;
    }
    let Expr::Path(path) = expression else {
        return false;
    };
    let normalized_segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if normalized_segments.ends_with(&["value".to_string(), "as_array".to_string()]) {
        return true;
    }
    let mut segments = path.path.segments.iter().rev();
    let Some(callable) = segments.next().map(|segment| segment.ident.to_string()) else {
        return false;
    };
    if callable.chars().next().is_some_and(char::is_uppercase) {
        return true;
    }
    segments.next().is_some_and(|owner| {
        owner
            .ident
            .to_string()
            .chars()
            .next()
            .is_some_and(char::is_uppercase)
    }) && !matches!(method_flow_rule(&callable), MethodFlowRule::Opaque)
}

fn callable_sink_is_registered(callable: &Expr, callables: &CallableSemantics) -> bool {
    let name = callable_name(callable);
    let qualified = callable_qualified_name(callable);
    if callable_path_has_explicit_type_owner(callable) {
        qualified.as_deref().is_some_and(|name| {
            callables.sink_methods.contains_key(name)
                || callables.ambiguous_sink_methods.contains(name)
        })
    } else if callable_path_is_unqualified(callable) {
        name.as_deref().is_some_and(|name| {
            callables.sink_functions.contains_key(name)
                || callables.ambiguous_sink_functions.contains(name)
        })
    } else {
        false
    }
}

fn callable_path_is_unqualified(expression: &Expr) -> bool {
    matches!(expression, Expr::Path(path) if path.path.segments.len() == 1)
}

fn callable_path_has_explicit_type_owner(expression: &Expr) -> bool {
    let Expr::Path(path) = expression else {
        return false;
    };
    path.path.segments.iter().rev().nth(1).is_some_and(|owner| {
        owner
            .ident
            .to_string()
            .chars()
            .next()
            .is_some_and(char::is_uppercase)
    })
}

fn is_identity_comparison(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cmp"
            | "contains"
            | "ends_with"
            | "eq"
            | "eq_ignore_ascii_case"
            | "ne"
            | "partial_cmp"
            | "starts_with"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClosureInputSemantics {
    NoInput,
    Receiver,
    InitialAndReceiver,
    ReceiverPair,
}

fn higher_order_closure_semantics(
    name: &str,
    argument_index: usize,
) -> Option<ClosureInputSemantics> {
    let name = name.to_ascii_lowercase();
    if argument_index == 0
        && [
            "get_or_insert_with",
            "map_err",
            "ok_or_else",
            "or_insert_with",
            "or_else",
            "resize_with",
            "then",
            "unwrap_or_else",
        ]
        .contains(&name.as_str())
    {
        return Some(ClosureInputSemantics::NoInput);
    }
    if argument_index == 0 && name == "map_or_else" {
        return Some(ClosureInputSemantics::NoInput);
    }
    if argument_index == 0
        && [
            "all",
            "and_then",
            "any",
            "filter",
            "filter_map",
            "find",
            "find_map",
            "flat_map",
            "for_each",
            "inspect",
            "is_none_or",
            "is_some_and",
            "map",
            "map_while",
            "max_by_key",
            "min_by_key",
            "partition",
            "position",
            "rposition",
            "skip_while",
            "sort_by_key",
            "take_while",
        ]
        .contains(&name.as_str())
    {
        return Some(ClosureInputSemantics::Receiver);
    }
    if argument_index == 1 && ["map_or", "map_or_else"].contains(&name.as_str()) {
        return Some(ClosureInputSemantics::Receiver);
    }
    if argument_index == 1 && ["fold", "scan", "try_fold"].contains(&name.as_str()) {
        return Some(ClosureInputSemantics::InitialAndReceiver);
    }
    if argument_index == 0 && ["max_by", "min_by", "reduce", "sort_by"].contains(&name.as_str()) {
        return Some(ClosureInputSemantics::ReceiverPair);
    }
    None
}

fn has_higher_order_semantics(name: &str) -> bool {
    (0..=1).any(|index| higher_order_closure_semantics(name, index).is_some())
}

fn higher_order_call_shape_supported(name: &str, argument_count: usize) -> bool {
    let expected = if higher_order_closure_semantics(name, 1).is_some() {
        2
    } else if higher_order_closure_semantics(name, 0).is_some() {
        1
    } else {
        return false;
    };
    argument_count == expected
}

fn higher_order_closure_inputs(
    method: &str,
    argument_index: usize,
    receiver_expression: &Expr,
    receiver: &IdentityFlow,
    arguments: &syn::punctuated::Punctuated<Expr, Token![,]>,
    environment: &IdentityFlowEnvironment<'_>,
) -> Option<Vec<IdentityFlow>> {
    if argument_index == 0
        && method.eq_ignore_ascii_case("map_err")
        && let Some(error) = direct_result_error_flow(receiver_expression, environment)
    {
        return Some(error.into_iter().collect());
    }
    match higher_order_closure_semantics(method, argument_index)? {
        ClosureInputSemantics::NoInput => Some(Vec::new()),
        ClosureInputSemantics::Receiver => Some(vec![receiver.clone()]),
        ClosureInputSemantics::InitialAndReceiver => Some(vec![
            arguments
                .first()
                .map(|argument| expression_flow(argument, environment))
                .unwrap_or_default(),
            receiver.clone(),
        ]),
        ClosureInputSemantics::ReceiverPair => Some(vec![receiver.clone(), receiver.clone()]),
    }
}

fn direct_result_error_flow(
    expression: &Expr,
    environment: &IdentityFlowEnvironment<'_>,
) -> Option<Option<IdentityFlow>> {
    let expression = match expression {
        Expr::Paren(paren) => return direct_result_error_flow(&paren.expr, environment),
        Expr::Group(group) => return direct_result_error_flow(&group.expr, environment),
        expression => expression,
    };
    let Expr::Call(call) = expression else {
        return None;
    };
    let Expr::Path(path) = call.func.as_ref() else {
        return None;
    };
    let constructor = path.path.segments.last()?.ident.to_string();
    if constructor == "Ok" {
        return Some(None);
    }
    if constructor != "Err" || call.args.len() != 1 {
        return None;
    }
    Some(
        call.args
            .first()
            .map(|value| expression_flow(value, environment)),
    )
}

fn is_higher_order_nonidentity_result(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "all"
            | "any"
            | "for_each"
            | "is_none_or"
            | "is_some_and"
            | "position"
            | "rposition"
            | "sort_by"
            | "sort_by_key"
    )
}

enum MethodFlowRule {
    Receiver,
    ReceiverAndArguments,
    Aggregate,
    Transform,
    MapOr,
    Fold,
    Reduce,
    NonIdentity,
    Opaque,
}

fn method_flow_rule(name: &str) -> MethodFlowRule {
    let name = name.to_ascii_lowercase();
    if [
        "as_deref",
        "as_ref",
        "as_str",
        "borrow",
        "clone",
        "deref",
        "expect",
        "filter",
        "find",
        "get",
        "inspect",
        "into",
        "last",
        "map_err",
        "max_by",
        "ok",
        "ok_or",
        "ok_or_else",
        "strip_prefix",
        "to_ascii_lowercase",
        "to_lowercase",
        "to_owned",
        "to_path_buf",
        "to_string",
        "to_uppercase",
        "to_vec",
        "transpose",
        "trim",
        "unwrap",
    ]
    .contains(&name.as_str())
    {
        return MethodFlowRule::Receiver;
    }
    if [
        "and_then",
        "get_or_insert_with",
        "join",
        "or_insert_with",
        "or_else",
        "replace",
        "unwrap_or",
        "unwrap_or_else",
    ]
    .contains(&name.as_str())
    {
        return MethodFlowRule::ReceiverAndArguments;
    }
    if [
        "filter_map",
        "find_map",
        "flat_map",
        "map",
        "map_while",
        "then",
    ]
    .contains(&name.as_str())
    {
        return MethodFlowRule::Transform;
    }
    if ["map_or", "map_or_else"].contains(&name.as_str()) {
        return MethodFlowRule::MapOr;
    }
    if ["fold", "scan", "try_fold"].contains(&name.as_str()) {
        return MethodFlowRule::Fold;
    }
    if name == "reduce" {
        return MethodFlowRule::Reduce;
    }
    if [
        "bytes",
        "chars",
        "cloned",
        "collect",
        "copied",
        "into_iter",
        "iter",
        "iter_mut",
    ]
    .contains(&name.as_str())
    {
        return MethodFlowRule::Aggregate;
    }
    if [
        "count",
        "resize_with",
        "is_empty",
        "is_err",
        "is_none",
        "is_none_or",
        "is_ok",
        "is_some",
        "is_some_and",
        "len",
        "position",
        "rposition",
    ]
    .contains(&name.as_str())
    {
        return MethodFlowRule::NonIdentity;
    }
    MethodFlowRule::Opaque
}

fn identity_accessor_axis(name: &str) -> Option<&'static str> {
    name.to_ascii_lowercase()
        .strip_suffix("_id")
        .and_then(exact_identity_axis)
}

fn is_transparent_function_call(call: &syn::ExprCall, name: Option<&str>) -> bool {
    is_transparent_callable_path(&call.func, name)
}

fn is_intrinsic_transparent_callable_path(expression: &Expr) -> bool {
    let Expr::Path(path) = expression else {
        return false;
    };
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    matches!(segments.as_slice(), [constructor] if constructor == "Ok" || constructor == "Some")
        || segments.as_slice() == ["std", "convert", "identity"]
        || segments.as_slice() == ["core", "convert", "identity"]
}

fn is_source_independent_string_call(expression: &Expr) -> bool {
    let Expr::Path(path) = expression else {
        return false;
    };
    let mut segments = path.path.segments.iter().rev();
    let callable = segments.next().map(|segment| segment.ident.to_string());
    let owner = segments.next().map(|segment| segment.ident.to_string());
    callable.as_deref() == Some("read_to_string") && owner.as_deref() == Some("fs")
}

fn is_mutable_sequence_constructor(expression: &Expr) -> bool {
    let Expr::Path(path) = expression else {
        return false;
    };
    let mut segments = path.path.segments.iter().rev();
    let callable = segments.next().map(|segment| segment.ident.to_string());
    let owner = segments.next().map(|segment| segment.ident.to_string());
    owner.as_deref() == Some("Vec") && matches!(callable.as_deref(), Some("new" | "with_capacity"))
}

fn is_transparent_callable_path(expression: &Expr, name: Option<&str>) -> bool {
    let Some(name) = name else {
        return false;
    };
    let Expr::Path(path) = expression else {
        return false;
    };
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let owner = segments
        .len()
        .checked_sub(2)
        .and_then(|index| segments.get(index))
        .map(String::as_str);

    matches!(
        (owner, name),
        (Some("asref"), "as_ref")
            | (Some("borrow"), "borrow")
            | (Some("clone"), "clone")
            | (Some("deref"), "deref")
            | (Some("from"), "from")
            | (Some("into"), "into")
            | (Some("str"), "to_owned")
            | (Some("str"), "to_string")
            | (Some("toowned"), "to_owned")
            | (Some("tostring"), "to_string")
    ) || name == "from" && owner.is_some_and(|owner| ["cow", "string"].contains(&owner))
        || name == "new" && owner.is_some_and(|owner| ["arc", "box", "rc"].contains(&owner))
}

fn contains_identity_comparison(expression: &Expr) -> bool {
    #[derive(Default)]
    struct ComparisonVisitor {
        found: bool,
    }

    impl Visit<'_> for ComparisonVisitor {
        fn visit_expr_binary(&mut self, node: &ExprBinary) {
            self.found |= matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_));
            syn::visit::visit_expr_binary(self, node);
        }

        fn visit_expr_call(&mut self, node: &syn::ExprCall) {
            self.found |= callable_name(&node.func)
                .as_deref()
                .is_some_and(is_identity_comparison);
            syn::visit::visit_expr_call(self, node);
        }

        fn visit_expr_method_call(&mut self, node: &syn::ExprMethodCall) {
            self.found |= is_identity_comparison(&node.method.to_string());
            syn::visit::visit_expr_method_call(self, node);
        }

        fn visit_expr_match(&mut self, node: &ExprMatch) {
            self.found = true;
            syn::visit::visit_expr_match(self, node);
        }

        fn visit_expr_macro(&mut self, node: &syn::ExprMacro) {
            self.found |= node.mac.path.is_ident("matches");
            syn::visit::visit_expr_macro(self, node);
        }
    }

    let mut visitor = ComparisonVisitor::default();
    visitor.visit_expr(expression);
    visitor.found
}

fn macro_flow(mac: &syn::Macro, environment: &IdentityFlowEnvironment<'_>) -> IdentityFlow {
    let name = mac
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_else(|| "<macro>".to_string());
    if name == "matches" {
        return IdentityFlow::default();
    }
    if matches!(name.as_str(), "format" | "format_args") {
        return format_macro_flow(mac.tokens.clone(), environment);
    }
    if name == "concat" {
        let mut flow = token_stream_flow(mac.tokens.clone(), environment);
        flow.raw_string = true;
        return flow;
    }
    if name == "vec" {
        let mut flow = aggregate_flow(token_stream_flow(mac.tokens.clone(), environment));
        flow.mutable_sequence = true;
        return flow;
    }
    if name == "json" {
        let mut flow = aggregate_flow(token_stream_flow(mac.tokens.clone(), environment));
        flow.callable_sensitive = false;
        return flow;
    }

    let mut flow = token_stream_flow(mac.tokens.clone(), environment);
    if let Some(definition) = environment.macro_definitions.get(&name) {
        let definition_flow = token_stream_flow(definition.clone(), environment);
        flow.callable_sensitive |= token_stream_contains_comparison(definition.clone());
        flow.merge(definition_flow);
    }
    if flow.callable_sensitive {
        flow.mark_unsupported(format!("macro {name}!"));
        return flow;
    }
    opaque_flow(flow, format!("macro {name}!"))
}

fn format_macro_flow(
    tokens: TokenStream,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let trees = tokens.into_iter().collect::<Vec<_>>();
    let mut flow = IdentityFlow::default();
    let mut start = 0;
    if let Some(TokenTree::Literal(literal)) = trees.first()
        && let Ok(template) = syn::parse_str::<LitStr>(&literal.to_string())
    {
        for placeholder in format_placeholders(&template.value()) {
            if let Some(binding) = lookup_binding(&placeholder, environment.bindings) {
                flow.merge(binding);
            } else if let Some(constant) = environment.constants.get(&placeholder) {
                flow.merge(constant.clone());
            } else if let Some(axis) = exact_identity_axis(&placeholder) {
                flow.axes.insert(axis);
            }
        }
        start = 1;
    }
    flow.merge(token_stream_flow(
        trees.into_iter().skip(start).collect(),
        environment,
    ));
    flow.raw_string = true;
    flow
}

fn format_placeholders(template: &str) -> Vec<String> {
    let bytes = template.as_bytes();
    let mut names = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'{' || bytes.get(index + 1) == Some(&b'{') {
            index += 1;
            continue;
        }
        let Some(end) = bytes[index + 1..].iter().position(|byte| *byte == b'}') else {
            break;
        };
        let value = &template[index + 1..index + 1 + end];
        let name = value.split(':').next().unwrap_or_default().trim();
        if !name.is_empty()
            && name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
        {
            names.push(name.to_string());
        }
        index += end + 2;
    }
    names
}

fn token_stream_flow(
    tokens: TokenStream,
    environment: &IdentityFlowEnvironment<'_>,
) -> IdentityFlow {
    let mut flow = IdentityFlow::default();
    let mut after_dollar = false;
    for token in tokens {
        match token {
            TokenTree::Ident(identifier) => {
                if !after_dollar {
                    let name = identifier.to_string();
                    if let Some(binding) = lookup_binding(&name, environment.bindings) {
                        flow.merge(binding);
                    } else if let Some(constant) = environment.constants.get(&name) {
                        flow.merge(constant.clone());
                    } else if let Some(axis) = exact_identity_axis(&name) {
                        flow.axes.insert(axis);
                    }
                }
                after_dollar = false;
            }
            TokenTree::Literal(literal) => {
                if let Ok(value) = syn::parse_str::<LitStr>(&literal.to_string()) {
                    flow.strings.insert(value.value());
                    flow.raw_string = true;
                }
                after_dollar = false;
            }
            TokenTree::Group(group) => {
                flow.merge(token_stream_flow(group.stream(), environment));
                after_dollar = false;
            }
            TokenTree::Punct(punctuation) => {
                after_dollar = punctuation.as_char() == '$';
            }
        }
    }
    flow
}

fn token_stream_contains_comparison(tokens: TokenStream) -> bool {
    let trees = tokens.into_iter().collect::<Vec<_>>();
    for (index, token) in trees.iter().enumerate() {
        match token {
            TokenTree::Ident(identifier) if is_identity_comparison(&identifier.to_string()) => {
                return true;
            }
            TokenTree::Group(group) if token_stream_contains_comparison(group.stream()) => {
                return true;
            }
            TokenTree::Punct(punctuation)
                if matches!(punctuation.as_char(), '=' | '!')
                    && trees.get(index + 1).is_some_and(
                        |next| matches!(next, TokenTree::Punct(next) if next.as_char() == '='),
                    ) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn local_binding(pattern: &syn::Pat) -> Option<(String, Option<&Type>)> {
    match pattern {
        syn::Pat::Ident(pattern) => Some((pattern.ident.to_string(), None)),
        syn::Pat::Type(pattern) => {
            let syn::Pat::Ident(identifier) = pattern.pat.as_ref() else {
                return None;
            };
            Some((identifier.ident.to_string(), Some(&pattern.ty)))
        }
        _ => None,
    }
}

fn flows_for_pattern(
    pattern: &syn::Pat,
    initializer: IdentityFlow,
) -> HashMap<String, IdentityFlow> {
    let mut bindings = HashMap::new();
    collect_pattern_flows(pattern, None, initializer, &mut bindings);
    bindings
}

fn collect_pattern_flows(
    pattern: &syn::Pat,
    declared_type: Option<&Type>,
    initializer: IdentityFlow,
    bindings: &mut HashMap<String, IdentityFlow>,
) {
    match pattern {
        syn::Pat::Ident(identifier) => {
            let name = identifier.ident.to_string();
            bindings.insert(
                name.clone(),
                flow_for_binding(&name, declared_type, initializer),
            );
        }
        syn::Pat::Type(pattern) => {
            collect_pattern_flows(&pattern.pat, Some(&pattern.ty), initializer, bindings);
        }
        syn::Pat::Reference(pattern) => {
            collect_pattern_flows(&pattern.pat, declared_type, initializer, bindings);
        }
        syn::Pat::Paren(pattern) => {
            collect_pattern_flows(&pattern.pat, declared_type, initializer, bindings);
        }
        syn::Pat::Struct(pattern) => {
            for field in &pattern.fields {
                let flow = match &field.member {
                    Member::Named(identifier) => initializer
                        .named_fields
                        .get(&identifier.to_string())
                        .cloned()
                        .or_else(|| {
                            exact_identity_axis(&identifier.to_string()).map(identity_field_flow)
                        })
                        .unwrap_or_else(|| projection_fallback(&initializer)),
                    Member::Unnamed(index) => initializer
                        .indexed_fields
                        .get(index.index as usize)
                        .cloned()
                        .unwrap_or_else(|| projection_fallback(&initializer)),
                };
                collect_pattern_flows(&field.pat, None, flow, bindings);
            }
        }
        syn::Pat::Tuple(pattern) => {
            for (index, element) in pattern.elems.iter().enumerate() {
                let flow = initializer
                    .indexed_fields
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| projection_fallback(&initializer));
                collect_pattern_flows(element, None, flow, bindings);
            }
        }
        syn::Pat::TupleStruct(pattern) => {
            if pattern.elems.len() == 1
                && pattern.path.segments.last().is_some_and(|segment| {
                    ["Some", "Ok", "Err"].contains(&segment.ident.to_string().as_str())
                })
            {
                collect_pattern_flows(
                    pattern.elems.first().expect("single transparent pattern"),
                    None,
                    initializer,
                    bindings,
                );
                return;
            }
            for (index, element) in pattern.elems.iter().enumerate() {
                let flow = initializer
                    .indexed_fields
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| projection_fallback(&initializer));
                collect_pattern_flows(element, None, flow, bindings);
            }
        }
        syn::Pat::Slice(pattern) => {
            for (index, element) in pattern.elems.iter().enumerate() {
                let flow = initializer
                    .indexed_fields
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| projection_fallback(&initializer));
                collect_pattern_flows(element, None, flow, bindings);
            }
        }
        syn::Pat::Or(pattern) => {
            for case in &pattern.cases {
                collect_pattern_flows(case, declared_type, initializer.clone(), bindings);
            }
        }
        _ => {}
    }
}

fn projection_fallback(initializer: &IdentityFlow) -> IdentityFlow {
    let mut flow = flow_summary(initializer);
    if !flow.axes.is_empty() || !flow.argument_sources.is_empty() {
        flow.mark_unsupported("projection without resolved member flow");
    }
    flow
}

fn identity_field_flow(axis: &'static str) -> IdentityFlow {
    IdentityFlow {
        axes: BTreeSet::from([axis]),
        ..IdentityFlow::default()
    }
}

fn type_is_raw_identity(kind: &Type) -> bool {
    match kind {
        Type::Reference(reference) => type_is_raw_identity(&reference.elem),
        Type::Paren(paren) => type_is_raw_identity(&paren.elem),
        Type::Group(group) => type_is_raw_identity(&group.elem),
        Type::Path(path) => {
            let Some(segment) = path.path.segments.last() else {
                return false;
            };
            if matches!(segment.ident.to_string().as_str(), "str" | "String") {
                return true;
            }
            if !matches!(
                segment.ident.to_string().as_str(),
                "Box" | "Cow" | "Option" | "Rc" | "Arc"
            ) {
                return false;
            }
            let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return false;
            };
            arguments.args.iter().any(|argument| {
                matches!(argument, GenericArgument::Type(kind) if type_is_raw_identity(kind))
            })
        }
        _ => false,
    }
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
    fn identity_branches_reject_unlisted_methods_wrappers_and_closures() {
        let source = r#"
            fn inspect(game: &str, resource: &str) {
                let resources = ["resource.fixed"];
                let predicate = |value: &str| value.eq_ignore_ascii_case("game.wrapped");
                let _ = game.eq_ignore_ascii_case("game.direct");
                let _ = resources.contains(&resource);
                let _ = predicate(game);
                let _ = ["game.closure"]
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(game));
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for value in [
            "game.direct",
            "resource.fixed",
            "game.wrapped",
            "game.closure",
        ] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
    }

    #[test]
    fn identity_branches_share_one_fail_closed_flow_kernel() {
        let source = r#"
            const EXAMPLIA_CONST: &str = "game.const";
            static EXAMPLIA_STATIC: &str = "game.static";

            fn pass<T, U>(_: T, value: U) -> U { value }
            fn first<T, U>(value: T, _: U) -> T { value }
            struct Holder<'a> { value: &'a str }

            fn inspect(game: &str, candidate: &str) {
                macro_rules! hidden_branch {
                    () => { game == "game.macro" };
                }

                let pair = (game, "game.tuple");
                let values = ["game.index"];
                let needle = "game.let";
                let holder = Holder { value: game };
                let wrapped = mystery_wrap(game);
                let _ = str::eq(game, "game.free");
                let _ = <str as PartialEq>::eq(game, "game.ufcs");
                let _ = PartialEq::eq(&game, &EXAMPLIA_CONST);
                let _ = pass(true, game) == "game.multi_argument";
                let _ = first(game.bytes(), true).eq("game.bytes_multi".bytes());
                let _ = { game } == "game.block";
                let _ = pair.0 == "game.tuple_projection";
                let _ = holder.value == "game.field_projection";
                let _ = game == values[0];
                let _ = game == EXAMPLIA_STATIC;
                let _ = game == needle;
                let _ = hidden_branch!();
                let _ = match format!("{game}").as_str() {
                    "game.match" => true,
                    _ => false,
                };
                let _ = matches!(game, "game.matches");
                if let "game.if_let" = game {}
                let _ = mystery_wrap(game) == candidate;
                let _ = wrapped.value == candidate;
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for value in [
            "game.free",
            "game.ufcs",
            "game.const",
            "game.multi_argument",
            "game.bytes_multi",
            "game.block",
            "game.tuple_projection",
            "game.field_projection",
            "game.index",
            "game.static",
            "game.let",
            "game.macro",
            "game.match",
            "game.matches",
            "game.if_let",
        ] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
        assert!(violations.iter().any(|violation| {
            violation.contains("incomplete identity analysis on axis game")
                && violation.contains("call mystery_wrap")
        }));
    }

    #[test]
    fn assignments_and_pattern_bindings_preserve_identity_flow() {
        let source = r#"
            fn inspect(game: &str) {
                let mut alias = "";
                alias = game;
                let _ = alias == "game.assigned";

                let mut pair = ("", "neutral");
                pair.0 = game;
                let _ = pair.0 == "game.projected_assignment";

                let (mut first, mut second) = ("", "");
                (first, second) = (game, "neutral");
                let _ = first == "game.destructured_assignment";
                let _ = second == "neutral.destructured_assignment";

                let _ = match game {
                    bound => bound == "game.match_binding",
                };
                if let Some(bound) = Some(game) {
                    let _ = bound == "game.if_let_binding";
                }
                while let Some(bound) = Some(game) {
                    let _ = bound == "game.while_let_binding";
                    break;
                }
                let _ = matches!(
                    Some(game),
                    Some(bound) if bound == "game.matches_guard"
                );
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for value in [
            "game.assigned",
            "game.projected_assignment",
            "game.destructured_assignment",
            "game.match_binding",
            "game.if_let_binding",
            "game.while_let_binding",
            "game.matches_guard",
        ] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("neutral.destructured_assignment"))
        );
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("projection without resolved member flow")),
            "transparent Some patterns must preserve the subject flow: {violations:#?}"
        );
    }

    #[test]
    fn mutable_sequences_preserve_pushed_identity_flow() {
        let source = r#"
            fn inspect(game: &str, mode: &str) {
                let mut values = Vec::new();
                values.push(game);
                let _ = values[0] == "game.pushed";

                let mut neutral = vec![];
                neutral.push(mode);
                let _ = neutral[0] == "neutral.pushed";
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.pushed")),
            "missing pushed identity flow: {violations:#?}"
        );
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("neutral.pushed")),
            "neutral pushed flow inherited identity: {violations:#?}"
        );
    }

    #[test]
    fn conditional_assignments_merge_all_reachable_identity_flows() {
        let source = r#"
            fn inspect(game: &str, mode: &str, enabled: bool) {
                let mut selected = mode;
                if enabled {
                    selected = game;
                } else {
                    selected = mode;
                }
                let _ = selected == "game.conditional_branch";

                let mut retained = game;
                if enabled {
                    retained = mode;
                }
                let _ = retained == "game.conditional_fallthrough";

                let mut neutral = mode;
                if enabled {
                    neutral = "neutral.then";
                } else {
                    neutral = "neutral.else";
                }
                let _ = neutral == "neutral.conditional";
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for value in ["game.conditional_branch", "game.conditional_fallthrough"] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("neutral.conditional")),
            "neutral conditional flow inherited identity: {violations:#?}"
        );
    }

    #[test]
    fn bool_and_unit_sink_helpers_bind_actual_arguments() {
        let source = r#"
            fn same(left: &str, right: &str) -> bool { left == right }
            fn selected(value: &str) -> bool { value == "game.helper_single" }
            fn check(left: &str, right: &str) { let _ = left == right; }
            fn same_length(left: &str, right: &str) -> bool { left.len() == right.len() }
            fn nested(left: &str, right: &str) -> bool { same(left, right) }
            fn recursive(left: &str, right: &str) -> bool {
                left == right || recursive(left, right)
            }
            fn independent(a: &str, b: &str, c: &str, d: &str) -> bool {
                a == b || c == d
            }

            struct Guard;
            impl Guard {
                fn same(&self, left: &str, right: &str) -> bool { left == right }
            }

            fn inspect(game: &str, mode: &str) {
                let _ = same(game, "game.helper_pair");
                let _ = selected(game);
                check(game, "game.helper_unit");
                let _ = nested(game, "game.helper_nested");
                let _ = recursive(game, "game.helper_recursive");
                let _ = Guard.same(game, "game.helper_method");

                let _ = same(mode, "neutral.helper_pair");
                let _ = same_length(game, "neutral.length_only");
                let _ = independent(
                    game,
                    mode,
                    "neutral.independent",
                    "game.not_compared_to_identity",
                );
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for value in [
            "game.helper_pair",
            "game.helper_single",
            "game.helper_unit",
            "game.helper_nested",
            "game.helper_recursive",
            "game.helper_method",
        ] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
        for value in [
            "neutral.helper_pair",
            "neutral.length_only",
            "game.not_compared_to_identity",
        ] {
            assert!(
                violations
                    .iter()
                    .all(|violation| !violation.contains(value)),
                "unexpected {value}: {violations:#?}"
            );
        }
    }

    #[test]
    fn unresolved_external_statement_sinks_fail_closed() {
        let source = r#"
            fn check(_: &str, _: &str) {}

            fn inspect(game: &str, mode: &str) {
                external::check(game, "game.external_sink");
                external::check(mode, "neutral.external_sink");
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(violations.iter().any(|violation| {
            violation.contains("incomplete identity analysis on axis game")
                && violation.contains("statement call check has unresolved sink semantics")
        }));
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("neutral.external_sink")),
            "neutral external call inherited identity: {violations:#?}"
        );
    }

    #[test]
    fn higher_order_closures_receive_identity_flow_by_semantics() {
        let source = r#"
            fn routed(value: &str) -> bool { value == "game.function_route" }

            fn inspect(game: &str, mode: &str) {
                let _ = Some(game).is_some_and(|value| value == "game.is_some_and");
                let _ = Some(game).is_none_or(|value| value == "game.is_none_or");
                let _ = [game]
                    .iter()
                    .filter(|value| **value == "game.filter")
                    .count();
                let _ = Some(game).map_or(false, |value| value == "game.map_or");
                let _ = [game]
                    .iter()
                    .fold(false, |_, value| *value == "game.fold");
                let _ = Some(game).is_some_and(routed);
                let _ = true
                    .then(|| game)
                    .is_some_and(|value| value == "game.then");
                let _ = Ok::<&str, &str>(game)
                    .map_err(|error| error == "neutral.map_err")
                    .unwrap()
                    .eq("game.map_err_success");

                let _ = Some(mode).is_some_and(|value| value == "neutral.some");
                let _ = [game]
                    .iter()
                    .filter(|_| mode == "neutral.captured")
                    .count();
                let _ = Some(game).unregistered_route(|_| mode == "neutral.unknown");
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for value in [
            "game.is_some_and",
            "game.is_none_or",
            "game.filter",
            "game.map_or",
            "game.fold",
            "game.function_route",
            "game.then",
            "game.map_err_success",
        ] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
        for value in ["neutral.some", "neutral.captured", "neutral.map_err"] {
            assert!(
                violations
                    .iter()
                    .all(|violation| !violation.contains(value)),
                "unexpected {value}: {violations:#?}"
            );
        }
        assert!(violations.iter().any(|violation| {
            violation.contains("incomplete identity analysis on axis game")
                && violation.contains("unregistered closure semantics")
        }));
    }

    #[test]
    fn unresolved_higher_order_function_paths_fail_closed() {
        let source = r#"
            fn inspect(game: &str) {
                let _ = Some(game).is_some_and(external::selected);
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(violations.iter().any(|violation| {
            violation.contains("incomplete identity analysis on axis game")
                && violation.contains("method is_some_and has unresolved callable routing")
        }));
    }

    #[test]
    fn map_err_routes_direct_result_error_identity_only() {
        let source = r#"
            fn inspect(game: &str) {
                let _ = Err::<&str, &str>(game)
                    .map_err(|error| error == "game.map_err_error");
                let _ = Ok::<&str, &str>(game)
                    .map_err(|error| error == "neutral.map_err_success");
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.map_err_error")),
            "missing Result error flow: {violations:#?}"
        );
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("neutral.map_err_success")),
            "Result success flow reached map_err: {violations:#?}"
        );
    }

    #[test]
    fn higher_order_routing_preserves_projected_nonidentity_fields() {
        let source = r#"
            struct Asset { template: String }
            struct Draft { game: String, assets: Vec<Asset> }

            fn build(game: &str) -> Draft {
                Draft {
                    game: game.to_owned(),
                    assets: vec![Asset { template: "neutral.template".to_owned() }],
                }
            }

            fn inspect(game: &str) {
                let draft = build(game);
                let _ = draft.assets
                    .iter()
                    .map(|asset| asset.template == "neutral.asset")
                    .collect::<Vec<_>>();
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("neutral.asset")),
            "projected nonidentity field inherited identity flow: {violations:#?}"
        );
    }

    #[test]
    fn reserved_method_names_do_not_whitelist_local_methods() {
        let source = r#"
            struct Custom;
            impl Custom {
                fn map<F>(&self, value: &str, predicate: F) -> bool
                where F: Fn(&str) -> bool
                {
                    predicate(value)
                }
            }

            fn inspect(game: &str) {
                let _ = Custom.map(game, |value| value == "game.custom_map");
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(violations.iter().any(|violation| {
            violation.contains("incomplete identity analysis on axis game")
                && violation.contains("collides with reserved semantic method")
        }));
    }

    #[test]
    fn local_methods_do_not_inherit_builtin_nonidentity_semantics() {
        let source = r#"
            struct Custom<'a> { value: &'a str }
            impl Custom<'_> {
                fn len(&self) -> &str { self.value }
            }

            fn inspect(game: &str) {
                let custom = Custom { value: game };
                let _ = custom.len() == "game.custom_method";
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(violations.iter().any(|violation| {
            violation.contains("incomplete identity analysis on axis game")
                && violation
                    .contains("local method len conflicts with built-in nonidentity semantics")
        }));
    }

    #[test]
    fn identity_branches_allow_proven_nonidentity_through_same_syntax() {
        let source = r#"
            const LABEL: &str = "neutral.const";

            fn mystery_wrap<T>(value: T) -> T { value }
            struct Holder<'a> { value: &'a str }

            fn inspect(mode: &str) -> bool {
                let values = ["neutral.index"];
                let pair = (mode, "neutral.tuple");
                let holder = Holder { value: mode };
                let _ = mystery_wrap(mode) == "neutral.call";
                let _ = pair.0 == values[0];
                let _ = holder.value == "neutral.field";
                let _ = match format!("{mode}").as_str() {
                    LABEL => true,
                    _ => false,
                };
                if let "neutral.if_let" = mode {}
                matches!(mode, "neutral.matches")
            }
        "#;
        assert!(
            inspect_identity_axis_branches("fixture.rs", source)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn inferred_identity_bindings_require_raw_string_flow() {
        let source = r#"
            struct TaskValue { game: String }

            fn build_task(game: &str) -> TaskValue {
                TaskValue { game: game.to_owned() }
            }

            fn inspect(game: &str) {
                let task = build_task(game);
                let _ = task.game == "game.aggregate_field";
                let task = "task.raw_string";
                let _ = task;
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.aggregate_field"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("task.raw_string"))
        );
        assert!(violations.iter().all(|violation| {
            !(violation.contains("axis task") && violation.contains("build_task"))
        }));
    }

    #[test]
    fn transparent_rules_propagate_identity_but_unknown_wrappers_fail_closed() {
        let source = r#"
            fn to_string(_value: &str) -> String {
                "neutral.local_to_string".to_owned()
            }

            fn inspect(game: &str, mode: &str, candidate: &str) {
                let identity_values = [game];
                let neutral_values = [mode];
                let mapped_identity_values = identity_values
                    .iter()
                    .map(|value| *value)
                    .collect::<Vec<_>>()
                    .to_vec();
                let mapped_neutral_values = neutral_values
                    .iter()
                    .map(|value| *value)
                    .collect::<Vec<_>>()
                    .to_vec();
                let mapped_identity_with_internal_comparison = identity_values
                    .iter()
                    .map(|value| {
                        let _ = mode == "neutral.map_internal";
                        *value
                    })
                    .collect::<Vec<_>>();
                let _ = game.as_ref().eq("game.transparent");
                let _ = game.trim().to_ascii_lowercase().eq("game.normalized");
                let _ = Some(game)
                    .filter(|value| !value.is_empty())
                    .unwrap()
                    .eq("game.filtered");
                let _ = identity_values.get(0).unwrap().eq(&"game.collection_get");
                let _ = identity_values.iter().last().unwrap().eq(&"game.collection_last");
                let _ = mapped_identity_values[0].eq("game.collection_map");
                let _ = mapped_identity_with_internal_comparison[0]
                    .eq("game.collection_map_internal");
                let _ = game.join("segment").eq("game.joined");
                let _ = Some(game)
                    .and_then(Some)
                    .ok_or_else(|| "diagnostic")
                    .unwrap()
                    .eq("game.option_transparent");
                let _ = mode.as_ref().eq("neutral.transparent");
                let _ = mode.trim().to_ascii_lowercase().eq("neutral.normalized");
                let _ = neutral_values.get(0).unwrap().eq(&"neutral.collection_get");
                let _ = neutral_values.iter().last().unwrap().eq(&"neutral.collection_last");
                let _ = mapped_neutral_values[0].eq("neutral.collection_map");
                let _ = Some(mode)
                    .and_then(Some)
                    .ok_or_else(|| "diagnostic")
                    .unwrap()
                    .eq("neutral.option_transparent");
                let _ = to_string(game).eq("neutral.local_to_string_call");
                let _ = game.scramble() == candidate;
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.transparent"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.option_transparent"))
        );
        for value in [
            "game.normalized",
            "game.filtered",
            "game.collection_get",
            "game.collection_last",
            "game.collection_map",
            "game.collection_map_internal",
            "game.joined",
        ] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
        assert!(violations.iter().all(|violation| {
            !violation.contains("neutral.transparent")
                && !violation.contains("neutral.option_transparent")
                && !violation.contains("neutral.normalized")
                && !violation.contains("neutral.collection_get")
                && !violation.contains("neutral.collection_last")
                && !violation.contains("neutral.collection_map")
                && !violation.contains("neutral.map_internal")
                && !violation.contains("neutral.local_to_string_call")
                && !violation.contains("neutral.if_let")
        }));
        assert!(violations.iter().any(|violation| {
            violation.contains("incomplete identity analysis on axis game")
                && violation.contains("method scramble")
        }));
    }

    #[test]
    fn callable_return_semantics_preserve_identity_or_prove_nonidentity() {
        let source = r#"
            struct Wrapped { value: String }

            fn has_value(game: &str) -> bool { !game.is_empty() }
            fn expose(game: &str) -> String { game.to_owned() }
            fn fixed() -> String { "game.fixed_return".to_owned() }
            fn wrap(value: &str) -> Wrapped {
                Wrapped { value: value.to_owned() }
            }

            fn inspect(game: &str) {
                let _ = has_value(game) == true;
                let _ = expose(game) == "game.string_return";
                let _ = game == fixed();
                let _ = wrap(game).value == "game.structured_return";
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.string_return"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.structured_return"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.fixed_return"))
        );
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("has_value"))
        );
    }

    #[test]
    fn file_local_callable_semantics_override_workspace_short_name_collisions() {
        let local_source = r#"
            fn detect_current_page(game: &str) -> String {
                game.to_owned()
            }

            fn inspect(game: &str) {
                let _ = detect_current_page(game) == "game.local_page";
            }
        "#;
        let local_file = syn::parse_file(local_source).unwrap();
        let other_file = syn::parse_file(
            r#"
                fn detect_current_page() -> bool { true }
            "#,
        )
        .unwrap();
        let (mut workspace, local_definitions) = collect_callable_inventory(&local_file);
        let (other_inventory, other_definitions) = collect_callable_inventory(&other_file);
        merge_callable_semantics(&mut workspace, other_inventory);
        derive_callable_summaries(&mut workspace, &[local_definitions, other_definitions]);
        assert_eq!(
            workspace.functions.get("detect_current_page"),
            Some(&CallableReturnFlow::Unknown)
        );

        let specialized = specialize_callable_semantics(&local_file, &workspace);
        assert_eq!(
            specialized.functions.get("detect_current_page"),
            Some(&CallableReturnFlow::RawString)
        );
        let violations =
            inspect_identity_axis_branches_with_semantics("fixture.rs", local_source, &specialized)
                .unwrap();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("game.local_page"))
        );
        assert!(
            violations
                .iter()
                .all(|violation| !violation.contains("incomplete identity analysis"))
        );
    }

    #[test]
    fn concrete_outcome_types_and_source_independent_reads_remain_precise() {
        let concrete: Type = syn::parse_str("PageDetectionOutcome").unwrap();
        let wrapped: Type = syn::parse_str("CliOutcome<PageDetectionOutcome>").unwrap();
        assert_eq!(type_return_flow(&concrete), CallableReturnFlow::Aggregate);
        assert_eq!(type_return_flow(&wrapped), CallableReturnFlow::Aggregate);

        let source = r#"
            struct Payload;
            struct Wrapped { task: String, neutral: String }

            fn mystery<T>(value: T) -> T { value }
            fn build(payload: &Payload) -> Wrapped {
                Wrapped {
                    task: payload.task_id().to_owned(),
                    neutral: mystery(payload.mode()),
                }
            }

            fn inspect(game: &str, payload: &Payload) {
                let text = std::fs::read_to_string(game).unwrap();
                let _ = text == "neutral.file_contents";
                let _ = build(payload) == build(payload);
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(violations.iter().all(|violation| {
            !violation.contains("neutral.file_contents")
                && !violation.contains("incomplete identity analysis on axis task")
        }));
    }

    #[test]
    fn projections_are_precise_and_qualified_variants_are_not_identity_axes() {
        let source = r#"
            struct Holder<'a> { value: &'a str }
            struct Entry { label: String }
            enum EventPayload { Task }

            fn inspect(game: &str, mode: &str, entries: &[Entry]) {
                let pair = (game, mode);
                let (selected, neutral) = pair;
                let Holder { value } = Holder { value: game };
                let _ = selected == "game.tuple_destructure";
                let _ = neutral == "neutral.tuple_destructure";
                let _ = value == "game.struct_destructure";
                let event = EventPayload::Task;
                let _ = event == "neutral.qualified_variant";
                for package in entries {
                    let _ = package.label == "neutral.loop_binding";
                }
            }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        for value in ["game.tuple_destructure", "game.struct_destructure"] {
            assert!(
                violations.iter().any(|violation| violation.contains(value)),
                "missing {value}: {violations:#?}"
            );
        }
        for value in [
            "neutral.tuple_destructure",
            "neutral.qualified_variant",
            "neutral.loop_binding",
        ] {
            assert!(
                violations
                    .iter()
                    .all(|violation| !violation.contains(value)),
                "unexpected {value}: {violations:#?}"
            );
        }
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
    fn rust_surface_inventory_closes_macro_serde_derive_and_ffi_boundaries() {
        let source = r#"
            macro_rules! typed_identity { ($name:ident) => { pub struct $name(String); }; }
            typed_identity!(OpaqueIdentity);

            #[derive(Serialize, Deserialize)]
            struct WireRecord { value: String }

            #[repr(C)]
            struct WireLayout { value: u32 }

            impl Serialize for CustomWire {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where S: Serializer { serializer.serialize_str("wire.v1") }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn exported_contract() {}
        "#;
        let surfaces = rust_surface_inventory("fixture.rs", source).unwrap();
        for kind in [
            "rust_macro_item",
            "rust_macro_invocation",
            "rust_derive_attribute",
            "rust_wire_impl",
            "rust_ffi_attribute",
            "rust_ffi_item",
        ] {
            assert!(
                surfaces.iter().any(|surface| surface.kind == kind),
                "missing {kind}: {surfaces:#?}"
            );
        }

        let changed = source.replace("wire.v1", "wire.v2");
        let changed = rust_surface_inventory("fixture.rs", &changed).unwrap();
        let original_impl = surfaces
            .iter()
            .find(|surface| surface.kind == "rust_wire_impl")
            .unwrap();
        let changed_impl = changed
            .iter()
            .find(|surface| surface.kind == "rust_wire_impl")
            .unwrap();
        assert_ne!(original_impl.content, changed_impl.content);
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
        track_workspace(&root);
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
        track_workspace(&root);

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
        track_workspace(&root);

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("unknown file type"));
        assert!(error.contains("opaque.random"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_snapshot_requires_a_trusted_git_index() {
        let root = temporary_workspace("missing-git-index");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Marker;\n",
        )
        .unwrap();
        create_required_roots(&root);
        fs::remove_dir_all(root.join(".git")).unwrap();

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("failed to read trusted Git index"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tracked_shadow_space_and_workspace_exclude_fail_closed() {
        let root = temporary_workspace("tracked-shadow-space");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "const SHADOW: &str = include_str!(\"../../../shadow-space/value.txt\");\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("shadow-space")).unwrap();
        fs::write(root.join("shadow-space/value.txt"), "hidden\n").unwrap();
        create_required_roots(&root);

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("unclassified tracked file shadow-space/value.txt"));
        fs::remove_dir_all(root).unwrap();

        let root = temporary_workspace("workspace-exclude");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/example\"]\nexclude = [\"crates/excluded\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Marker;\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/excluded/src")).unwrap();
        fs::write(
            root.join("crates/excluded/Cargo.toml"),
            "[package]\nname = \"excluded\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/excluded/src/lib.rs"),
            "pub struct Hidden;\n",
        )
        .unwrap();
        create_required_roots(&root);

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("unclassified tracked file crates/excluded/Cargo.toml"));
        assert!(error.contains("unclassified tracked file crates/excluded/src/lib.rs"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_compat_directory_does_not_grant_implicit_scope() {
        let root = temporary_workspace("external-compat-unregistered");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Marker;\n",
        )
        .unwrap();
        fs::create_dir_all(root.join(EXTERNAL_COMPAT_ROOT)).unwrap();
        fs::write(
            root.join(EXTERNAL_COMPAT_ROOT).join("unregistered.rs"),
            "pub struct HiddenIdentity;\n",
        )
        .unwrap();
        create_required_roots(&root);

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(
            error.contains(
                "unregistered external-compat file tests/external-compat/unregistered.rs"
            ),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compile_input_closure_rejects_untracked_and_dynamic_inputs() {
        let root = temporary_workspace("untracked-include");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        let source = root.join("crates/example/src/lib.rs");
        fs::write(&source, "pub struct Marker;\n").unwrap();
        create_required_roots(&root);
        fs::write(
            &source,
            "const DATA: &str = include_str!(\"payload.txt\");\n",
        )
        .unwrap();
        fs::write(root.join("crates/example/src/payload.txt"), "data\n").unwrap();

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains(
            "compile input crates/example/src/payload.txt referenced by crates/example/src/lib.rs is not tracked"
        ));
        fs::remove_dir_all(root).unwrap();

        let root = temporary_workspace("dynamic-include");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "const DATA: &str = include_str!(concat!(\"payload\", \".txt\"));\n",
        )
        .unwrap();
        create_required_roots(&root);

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("unsupported dynamic include_str! compile input"));
        fs::remove_dir_all(root).unwrap();

        let root = temporary_workspace("untracked-path-module");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        let source = root.join("crates/example/src/lib.rs");
        fs::write(&source, "pub struct Marker;\n").unwrap();
        create_required_roots(&root);
        fs::write(&source, "#[path = \"hidden.rs\"]\nmod hidden;\n").unwrap();
        fs::write(
            root.join("crates/example/src/hidden.rs"),
            "pub struct Hidden;\n",
        )
        .unwrap();

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains(
            "compile input crates/example/src/hidden.rs referenced by crates/example/src/lib.rs is not tracked"
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cargo_compile_inputs_rejects_unapproved_clap_extraction() {
        let root = temporary_workspace("clap-extractor");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n\n[dependencies]\nclap = \"4\"\n",
        )
        .unwrap();
        let error = cargo_compile_inputs(&root, "Cargo.toml").unwrap_err();
        assert!(error.contains("approved runtime CommandFactory extractor"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cargo_compile_inputs_rejects_unapproved_build_script_extraction() {
        let root = temporary_workspace("build-script-extractor");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("build.rs"), "fn main() {}\n").unwrap();
        let error = cargo_compile_inputs(&root, "Cargo.toml").unwrap_err();
        assert!(error.contains("approved generated-input extractor"));
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
        track_workspace(&root);

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
        write_external_compat_manifest(root);
        let policy = root.join(TRACKED_INPUT_POLICY_PATH);
        fs::create_dir_all(policy.parent().unwrap()).unwrap();
        fs::write(
            policy,
            format!(
                "schema_version = {:?}\nnon_product = []\n",
                TRACKED_INPUT_POLICY_SCHEMA_VERSION
            ),
        )
        .unwrap();
        run_git(root, &["init", "--quiet"]);
        track_workspace(root);
    }

    fn track_workspace(root: &Path) {
        run_git(root, &["add", "-A"]);
    }

    fn run_git(root: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_external_compat_manifest(root: &Path) {
        let path = root.join(EXTERNAL_COMPAT_MANIFEST_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "schema_version = {:?}\n",
                crate::external_compat::EXTERNAL_COMPAT_SCHEMA_VERSION
            ),
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
                    author_id: None,
                    created_at: None,
                    updated_at: None,
                    content_sha256: "a".repeat(64),
                    scope: vec!["surface.mapping".to_string()],
                },
                SurfaceApproval {
                    id: "approval.issue44_r8b".to_string(),
                    repository: "HS7097/ActingCommand-Workflow".to_string(),
                    issue: 54,
                    comment_id: 5011350539,
                    author: "HS7097".to_string(),
                    author_id: None,
                    created_at: None,
                    updated_at: None,
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
