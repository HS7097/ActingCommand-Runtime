// SPDX-License-Identifier: AGPL-3.0-only

//! Machine-readable generic-domain concepts and protected Runtime surfaces.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

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

pub const GENERIC_DOMAIN_SCHEMA_VERSION: &str = "actingcommand.generic-domain.v1";
pub const GENERIC_DOMAIN_REGISTRY_PATH: &str =
    "tools/actinglab-architecture/generic-domain-v1.toml";
pub const GENERIC_DOMAIN_SURFACE_SCHEMA_VERSION: &str = "actingcommand.generic-domain-surfaces.v1";
pub const GENERIC_DOMAIN_SURFACE_MANIFEST_PATH: &str =
    "tools/actinglab-architecture/generic-domain-surfaces-v1.jsonl";
const EXTERNAL_COMPAT_ROOT: &str = "tests/external-compat";
const MAX_PROTECTED_FILE_BYTES: u64 = 32 * 1024 * 1024;
const INITIAL_CONCEPT_APPROVAL_COMMENT_ID: u64 = 5_010_683_904;
const APPROVED_CONCEPT_FAMILIES: &[&str] = &[
    "agent",
    "artifact",
    "catalog",
    "coordination",
    "decision",
    "device",
    "execution",
    "fact",
    "identity",
    "interface",
    "ledger",
    "network",
    "operation",
    "performance",
    "recognition",
    "release",
    "schedule",
    "state_value",
    "structure",
    "time",
    "unit_qualifier",
];

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

fn read_bounded_regular_file(path: &Path) -> Result<Vec<u8>, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_PROTECTED_FILE_BYTES {
        return Err(format!(
            "{} is {} bytes, exceeding the {MAX_PROTECTED_FILE_BYTES}-byte protected-file limit",
            path.display(),
            metadata.len()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_PROTECTED_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_PROTECTED_FILE_BYTES {
        return Err(format!(
            "{} exceeded the {MAX_PROTECTED_FILE_BYTES}-byte protected-file limit while reading",
            path.display()
        ));
    }
    Ok(bytes)
}

fn read_bounded_regular_text(path: &Path) -> Result<String, String> {
    String::from_utf8(read_bounded_regular_file(path)?)
        .map_err(|error| format!("{} is not UTF-8: {error}", path.display()))
}

pub fn load_generic_domain_registry(path: &Path) -> Result<GenericDomainRegistry, String> {
    let source = read_bounded_regular_text(path)?;
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
    let bytes = read_bounded_regular_file(&manifest_path)?;
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
    if registry.surface.is_empty() {
        errors.push("generic-domain registry contains no protected surfaces".to_string());
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
    }

    let mut concept_ids = HashSet::new();
    let mut previous_concept = None;
    for concept in &registry.concept {
        if !is_concept_id(&concept.id) {
            errors.push(format!("invalid concept id {}", concept.id));
        }
        let family = concept.id.split_once('.').map(|(family, _)| family);
        if family.is_none_or(|family| !APPROVED_CONCEPT_FAMILIES.contains(&family)) {
            errors.push(format!("concept {} uses an unknown family", concept.id));
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
        if concept.approval_comment_id != INITIAL_CONCEPT_APPROVAL_COMMENT_ID {
            errors.push(format!(
                "concept {} approval_comment_id must be {INITIAL_CONCEPT_APPROVAL_COMMENT_ID}",
                concept.id
            ));
        }
    }
    let concept_by_id = registry
        .concept
        .iter()
        .map(|concept| (concept.id.as_str(), concept))
        .collect::<HashMap<_, _>>();
    for concept in &registry.concept {
        let Some(replacement) = &concept.replaced_by else {
            continue;
        };
        let Some(target) = concept_by_id.get(replacement.as_str()) else {
            errors.push(format!(
                "concept {} has unknown replacement {replacement}",
                concept.id
            ));
            continue;
        };
        if replacement == &concept.id {
            errors.push(format!("concept {} cannot replace itself", concept.id));
        }
        if concept.id.split_once('.').map(|(family, _)| family)
            != replacement.split_once('.').map(|(family, _)| family)
        {
            errors.push(format!(
                "concept {} replacement {replacement} crosses families",
                concept.id
            ));
        }
        if target.status != "active" {
            errors.push(format!(
                "concept {} replacement {replacement} is not active",
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
        if scopes.contains("identity.token") && allowance.tokens.is_empty() {
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
                | "rust_default_impl"
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
            let Some(concept) = concept_by_id.get(concept_id.as_str()) else {
                errors.push(format!(
                    "surface {} references unknown concept {concept_id}",
                    surface.surface_id
                ));
                continue;
            };
            if concept.status != "active" {
                errors.push(format!(
                    "surface {} maps deprecated concept {concept_id}",
                    surface.surface_id
                ));
            }
        }
        if !is_sha256(&surface.fingerprint) {
            errors.push(format!(
                "surface {} fingerprint must be lowercase SHA-256",
                surface.surface_id
            ));
        }
    }

    finish_errors(errors)
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
        let identity_source = read_bounded_regular_text(&file)?;
        let rust_identity_contexts = if file.extension().is_some_and(|extension| extension == "rs")
        {
            Some(rust_identity_branch_contexts(&relative, &identity_source)?)
        } else {
            None
        };
        for fragment in identity_fragments_for_source(&relative, &identity_source)? {
            let label = format!("{}#{}", relative, fragment.selector);
            let mut detector_tokens = inspect_generic_runtime_identity(&label, &fragment.content)
                .into_iter()
                .filter_map(|violation| {
                    violation.split_whitespace().last().map(ToString::to_string)
                })
                .collect::<Vec<_>>();
            detector_tokens.sort();
            detector_tokens.dedup();
            let branch_violations = if let Some(rust_identity_contexts) = &rust_identity_contexts {
                inspect_rust_identity_fragment(&label, &fragment, rust_identity_contexts)?
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
                "surface {} {} fingerprint drifted at {} {}: registered {}, actual {}",
                snapshot.surface_id,
                snapshot.kind,
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
        let identity_source = read_bounded_regular_text(&file)?;
        let rust_identity_contexts = if file.extension().is_some_and(|extension| extension == "rs")
        {
            Some(rust_identity_branch_contexts(&relative, &identity_source)?)
        } else {
            None
        };
        for fragment in identity_fragments_for_source(&relative, &identity_source)? {
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
            if let Some(rust_identity_contexts) = &rust_identity_contexts
                && !branch_allowed
            {
                errors.extend(inspect_rust_identity_fragment(
                    &label,
                    &fragment,
                    rust_identity_contexts,
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
    let contexts = rust_identity_branch_contexts(path, source)?;
    let mut violations = Vec::new();
    for fragment in rust_identity_fragments(path, source)? {
        violations.extend(inspect_rust_identity_fragment(path, &fragment, &contexts)?);
    }
    violations.sort();
    violations.dedup();
    Ok(violations)
}

fn inspect_rust_identity_fragment(
    path: &str,
    fragment: &IdentityFragment,
    contexts: &HashMap<String, syn::File>,
) -> Result<Vec<String>, String> {
    let context_key = fragment.rust_context_key.as_ref().ok_or_else(|| {
        format!(
            "Rust identity fragment {} has no context",
            fragment.selector
        )
    })?;
    let context = contexts.get(context_key).ok_or_else(|| {
        format!(
            "Rust identity fragment {} references missing context {context_key}",
            fragment.selector
        )
    })?;
    inspect_identity_axis_fragment_with_context(path, &fragment.content, context)
}

fn inspect_identity_axis_fragment_with_context(
    path: &str,
    fragment_source: &str,
    file: &syn::File,
) -> Result<Vec<String>, String> {
    let fragment = syn::parse_file(fragment_source)
        .map_err(|error| format!("failed to parse {path}: {error}"))?;
    inspect_identity_axis_branches_with_context(path, &fragment, file)
}

fn inspect_identity_axis_branches_with_context(
    path: &str,
    inspected: &syn::File,
    context: &syn::File,
) -> Result<Vec<String>, String> {
    let inferred_parameters = infer_identity_parameter_axes(context);
    let inferred_returns = infer_function_return_strings(context);
    let type_facts = identity_type_facts(context);
    let mut visitor = IdentityBranchVisitor {
        path,
        violations: Vec::new(),
        errors: Vec::new(),
        aliases: vec![HashMap::new()],
        non_identity: vec![HashSet::new()],
        types: vec![HashMap::new()],
        collections: vec![HashMap::new()],
        return_axis: None,
        inferred_parameters,
        inferred_returns,
        type_facts,
    };
    visitor.visit_file(inspected);
    if !visitor.errors.is_empty() {
        visitor.errors.sort();
        return Err(visitor.errors.join("\n"));
    }
    visitor.violations.sort();
    visitor.violations.dedup();
    Ok(visitor.violations)
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    id: String,
    manifest_path: String,
}

fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let mut command = Command::new("cargo");
    command.current_dir(root).args([
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
        "--offline",
    ]);
    if root.join("Cargo.lock").is_file() {
        command.arg("--locked");
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run Cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid Cargo metadata output: {error}"))?;
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<HashMap<_, _>>();
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        format!(
            "failed to resolve workspace root {}: {error}",
            root.display()
        )
    })?;
    let mut members = Vec::with_capacity(metadata.workspace_members.len());
    for id in metadata.workspace_members {
        let package = packages
            .get(id.as_str())
            .ok_or_else(|| format!("Cargo metadata omitted workspace member package {id}"))?;
        let manifest = fs::canonicalize(&package.manifest_path).map_err(|error| {
            format!(
                "failed to resolve Cargo metadata manifest {}: {error}",
                package.manifest_path
            )
        })?;
        let relative = manifest.strip_prefix(&canonical_root).map_err(|_| {
            format!(
                "Cargo metadata workspace member escaped repository: {}",
                package.manifest_path
            )
        })?;
        let relative = normalize_path(relative)?;
        if !relative.ends_with("/Cargo.toml") {
            return Err(format!(
                "Cargo metadata workspace member has unexpected manifest path {relative}"
            ));
        }
        let member = relative
            .strip_suffix("/Cargo.toml")
            .expect("checked manifest suffix")
            .to_string();
        validate_stable_path(&member)?;
        resolve_exact_regular_file(root, &relative)?;
        members.push(member);
    }
    members.sort();
    if members.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("Cargo metadata contains duplicate workspace member paths".to_string());
    }
    if members.is_empty() {
        return Err("Cargo metadata contains no workspace members".to_string());
    }
    Ok(members)
}

fn protected_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let classes = tracked_file_classes(root)?;
    validate_compile_input_closure(root, &classes)?;
    Ok(classes
        .into_iter()
        .filter_map(|(path, class)| (class == TrackedFileClass::Protected).then(|| root.join(path)))
        .collect())
}

fn tracked_file_classes(root: &Path) -> Result<Vec<(String, TrackedFileClass)>, String> {
    let members = workspace_members(root)?;
    let tracked = git_tracked_files(root)?;
    let external_paths = validated_external_compat_paths(root)?
        .into_iter()
        .chain([EXTERNAL_COMPAT_MANIFEST_PATH.to_string()])
        .collect::<HashSet<_>>();
    let mut errors = Vec::new();
    let mut classes = Vec::with_capacity(tracked.len());

    for (path, mode) in tracked {
        if !matches!(mode.as_str(), "100644" | "100755") {
            errors.push(format!(
                "tracked file {path} has unsupported Git index mode {mode}"
            ));
            continue;
        }
        if let Err(error) = resolve_exact_regular_file(root, &path) {
            errors.push(format!("tracked file {path} {error}"));
            continue;
        }
        let member = members.iter().find(|member| path_is_within(&path, member));
        let class = if external_paths.contains(&path) {
            TrackedFileClass::ExternalCompat
        } else if path_is_within(&path, EXTERNAL_COMPAT_ROOT) {
            errors.push(format!(
                "external-compat file is not registered by the exact manifest: {path}"
            ));
            continue;
        } else if member.is_some()
            || matches!(path.as_str(), "Cargo.lock" | "Cargo.toml")
            || is_repo_wide_protected_text(&path)
            || is_test_asset_path(&path)
        {
            TrackedFileClass::Protected
        } else if is_non_product_document(&path) || path == "external-tools/maatouch/maatouch" {
            TrackedFileClass::NonProduct
        } else {
            errors.push(format!("unclassified tracked file {path}"));
            continue;
        };

        if path.ends_with("Cargo.toml") && path != "Cargo.toml" {
            let parent = path
                .strip_suffix("/Cargo.toml")
                .expect("checked Cargo manifest suffix");
            if !members.iter().any(|member| member == parent) {
                errors.push(format!(
                    "tracked Cargo package is not a Cargo metadata workspace member: {parent}"
                ));
            }
        }
        if class == TrackedFileClass::Protected
            && let Err(error) = ensure_protected_text_path(&path)
        {
            errors.push(error);
            continue;
        }
        classes.push((path, class));
    }

    for member in &members {
        let manifest = format!("{member}/Cargo.toml");
        if !classes
            .iter()
            .any(|(path, class)| path == &manifest && *class == TrackedFileClass::Protected)
        {
            errors.push(format!(
                "Cargo metadata workspace member manifest is not tracked and protected: {manifest}"
            ));
        }
    }
    for external in external_paths {
        if !classes
            .iter()
            .any(|(path, class)| path == &external && *class == TrackedFileClass::ExternalCompat)
        {
            errors.push(format!("external-compat file is not tracked: {external}"));
        }
    }

    finish_errors(errors)?;
    classes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(classes)
}

fn is_repo_wide_protected_text(path: &str) -> bool {
    Path::new(path)
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
        })
}

fn is_test_asset_path(path: &str) -> bool {
    Path::new(path).components().any(|component| {
        component.as_os_str().to_str().is_some_and(|component| {
            matches!(
                component,
                "test" | "tests" | "fixture" | "fixtures" | "golden"
            )
        })
    })
}

fn is_non_product_document(path: &str) -> bool {
    matches!(
        Path::new(path).file_name().and_then(|name| name.to_str()),
        Some(".gitattributes" | ".gitignore" | "LICENSE")
    ) || path.ends_with(".md")
}

fn git_tracked_files(root: &Path) -> Result<Vec<(String, String)>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--stage", "-z"])
        .output()
        .map_err(|error| format!("failed to read trusted Git index: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to read trusted Git index: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut entries = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(parse_git_tracked_entry)
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("trusted Git index contains duplicate paths or merge stages".to_string());
    }
    if !entries.iter().any(|(path, _)| path == "Cargo.toml") {
        return Err("trusted Git index does not contain Cargo.toml".to_string());
    }
    Ok(entries)
}

fn parse_git_tracked_entry(record: &[u8]) -> Result<(String, String), String> {
    let separator = record
        .iter()
        .position(|byte| *byte == b'\t')
        .ok_or_else(|| "trusted Git index contains a malformed stage record".to_string())?;
    let metadata = std::str::from_utf8(&record[..separator])
        .map_err(|error| format!("trusted Git index contains non-UTF-8 metadata: {error}"))?;
    let path = std::str::from_utf8(&record[separator + 1..])
        .map_err(|error| format!("trusted Git index contains non-UTF-8 path: {error}"))?;
    let fields = metadata.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 3 || fields[2] != "0" || !is_git_object_id(fields[1]) {
        return Err(format!(
            "trusted Git index has invalid stage metadata for {path}"
        ));
    }
    validate_stable_path(path).map_err(|error| format!("trusted Git index path {error}"))?;
    Ok((path.to_string(), fields[0].to_string()))
}

fn is_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    let tracked_paths = class_by_path.keys().copied().collect::<HashSet<_>>();
    let mut errors = Vec::new();
    for (path, class) in classes {
        if *class != TrackedFileClass::Protected {
            continue;
        }
        let inputs = if path.ends_with(".rs") {
            read_bounded_regular_text(&root.join(path))
                .map_err(|error| format!("failed to read compile input owner {path}: {error}"))
                .and_then(|source| rust_compile_inputs(path, &source, &tracked_paths))
        } else if path.ends_with("Cargo.toml") {
            read_bounded_regular_text(&root.join(path))
                .map_err(|error| format!("failed to read Cargo compile input {path}: {error}"))
                .and_then(|source| cargo_compile_inputs(path, &source, &tracked_paths))
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

fn rust_compile_inputs(
    stable_path: &str,
    source: &str,
    tracked_paths: &HashSet<&str>,
) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source)
        .map_err(|error| format!("failed to parse compile input owner {stable_path}: {error}"))?;
    let source_dir = Path::new(stable_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let module_dir = rust_module_directory(stable_path)?;
    let mut inputs = Vec::new();
    collect_module_inputs(
        tracked_paths,
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
    tracked_paths: &HashSet<&str>,
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
                tracked_paths,
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
                .filter(|path| tracked_paths.contains(path.as_str()))
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

fn cargo_compile_inputs(
    stable_path: &str,
    source: &str,
    tracked_paths: &HashSet<&str>,
) -> Result<Vec<String>, String> {
    let manifest: toml::Value = toml::from_str(source)
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
                inputs.push(path);
            }
            Some(_) => {
                return Err(format!(
                    "Cargo package {stable_path} has unsupported dynamic build script declaration"
                ));
            }
            None => {
                let default = normalize_compile_input(manifest_dir, "build.rs")?;
                if tracked_paths.contains(default.as_str()) {
                    inputs.push(default);
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

fn ensure_protected_text_path(stable_path: &str) -> Result<(), String> {
    let path = Path::new(stable_path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("protected file name is not UTF-8: {stable_path}"))?;
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
            "protected Runtime surface has an unknown file type: {stable_path}"
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
    rust_context_key: Option<String>,
}

fn identity_fragments_for_file(
    path: &Path,
    stable_path: &str,
) -> Result<Vec<IdentityFragment>, String> {
    let source = read_bounded_regular_text(path)?;
    identity_fragments_for_source(stable_path, &source)
}

fn identity_fragments_for_source(
    stable_path: &str,
    source: &str,
) -> Result<Vec<IdentityFragment>, String> {
    let extension = Path::new(stable_path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let mut fragments = match extension {
        "rs" => rust_identity_fragments(stable_path, source)?,
        "json" => structured_json_inventory(stable_path, source)?
            .into_iter()
            .map(identity_fragment_from_raw)
            .collect(),
        "toml" => structured_toml_inventory(stable_path, source)?
            .into_iter()
            .map(identity_fragment_from_raw)
            .collect(),
        _ => text_surface_inventory(source)
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
        rust_context_key: None,
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
            rust_context_key: Some(rust_module_context_key(&[])),
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

fn rust_module_context_key(module: &[String]) -> String {
    format!("module:{}", module.join("::"))
}

fn rust_impl_context_key(owner: &str) -> String {
    format!("impl:{owner}")
}

fn rust_impl_owner(item: &syn::ItemImpl, module: &[String]) -> String {
    let trait_name = item
        .trait_
        .as_ref()
        .map(|(_, path, _)| path.to_token_stream().to_string())
        .unwrap_or_else(|| "inherent".to_string());
    qualified(
        module,
        &format!(
            "impl:{}:{}",
            short_hash(&trait_name),
            item.self_ty.to_token_stream()
        ),
    )
}

impl RustIdentityFragmentCollector {
    fn collect_items(&mut self, items: &[Item], module: &[String]) {
        let module_context_key = rust_module_context_key(module);
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
                        module_context_key.clone(),
                    );
                    let mut next = module.to_vec();
                    next.push(item.ident.to_string());
                    self.collect_items(
                        &item.content.as_ref().expect("checked inline module").1,
                        &next,
                    );
                }
                Item::Impl(item) => {
                    let owner = rust_impl_owner(item, module);
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
                        self.push(
                            format!("rust:{owner}::{name}"),
                            member.to_token_stream(),
                            rust_impl_context_key(&owner),
                        );
                    }
                }
                _ => self.push(
                    format!("rust:{}", identity_item_selector(item, module)),
                    item.to_token_stream(),
                    module_context_key.clone(),
                ),
            }
        }
    }

    fn push(&mut self, selector: String, content: impl ToTokens, rust_context_key: String) {
        self.fragments.push(IdentityFragment {
            selector,
            content: content.to_token_stream().to_string(),
            rust_context_key: Some(rust_context_key),
        });
    }
}

fn rust_identity_branch_contexts(
    path: &str,
    source: &str,
) -> Result<HashMap<String, syn::File>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse {path}: {error}"))?;
    let mut contexts = HashMap::new();
    collect_rust_identity_branch_contexts(&file.items, &[], &mut contexts);
    Ok(contexts)
}

fn collect_rust_identity_branch_contexts(
    items: &[Item],
    module: &[String],
    contexts: &mut HashMap<String, syn::File>,
) {
    let module_context = contexts
        .entry(rust_module_context_key(module))
        .or_insert_with(|| syn::File {
            shebang: None,
            attrs: Vec::new(),
            items: Vec::new(),
        });
    module_context.items.extend(
        items
            .iter()
            .filter(|item| matches!(item, Item::Fn(_) | Item::Struct(_)))
            .cloned(),
    );

    for item in items {
        match item {
            Item::Mod(item) => {
                if let Some((_, nested)) = &item.content {
                    let mut next = module.to_vec();
                    next.push(item.ident.to_string());
                    collect_rust_identity_branch_contexts(nested, &next, contexts);
                }
            }
            Item::Impl(item) => {
                contexts
                    .entry(rust_impl_context_key(&rust_impl_owner(item, module)))
                    .or_insert_with(|| syn::File {
                        shebang: None,
                        attrs: Vec::new(),
                        items: Vec::new(),
                    })
                    .items
                    .push(Item::Impl(item.clone()));
            }
            _ => {}
        }
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
    let source = read_bounded_regular_text(path)?;
    snapshot_for_source(stable_path, &source)
}

fn snapshot_for_source(stable_path: &str, source: &str) -> Result<Vec<SurfaceSnapshot>, String> {
    let extension = Path::new(stable_path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let raw = match extension {
        "rs" => rust_surface_inventory(stable_path, source)?,
        "json" => structured_json_inventory(stable_path, source)?,
        "toml" => structured_toml_inventory(stable_path, source)?,
        _ => text_surface_inventory(source),
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
                    let trait_name = item
                        .trait_
                        .as_ref()
                        .and_then(|(_, path, _)| path.segments.last())
                        .map(|segment| segment.ident.to_string());
                    if trait_name.as_deref() == Some("Default") {
                        self.push(
                            "rust_default_impl",
                            format!("impl_default:{owner}"),
                            item.to_token_stream(),
                        );
                    }
                    if matches!(trait_name.as_deref(), Some("Serialize" | "Deserialize")) {
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

struct IdentityFunction<'a> {
    name: String,
    inputs: &'a syn::punctuated::Punctuated<FnArg, Token![,]>,
    output: &'a syn::ReturnType,
    block: &'a syn::Block,
}

type TrackedIdentityType = String;

struct IdentityTypeFacts {
    function_returns: HashMap<String, TrackedIdentityType>,
    struct_fields: HashMap<(String, String), TrackedIdentityType>,
}

fn identity_type_facts(file: &syn::File) -> IdentityTypeFacts {
    let mut functions = Vec::new();
    collect_identity_functions(&file.items, &mut functions);
    let mut facts = IdentityTypeFacts {
        function_returns: HashMap::new(),
        struct_fields: HashMap::new(),
    };
    let mut ambiguous_returns = HashSet::new();
    for function in functions {
        let syn::ReturnType::Type(_, kind) = function.output else {
            continue;
        };
        let Some(kind) = tracked_identity_type(kind) else {
            continue;
        };
        if ambiguous_returns.contains(&function.name) {
            continue;
        }
        match facts.function_returns.get(&function.name) {
            Some(existing) if existing != &kind => {
                facts.function_returns.remove(&function.name);
                ambiguous_returns.insert(function.name);
            }
            Some(_) => {}
            None => {
                facts.function_returns.insert(function.name, kind);
            }
        }
    }
    collect_identity_struct_fields(&file.items, &mut facts.struct_fields);
    facts
}

fn collect_identity_struct_fields(
    items: &[Item],
    fields: &mut HashMap<(String, String), TrackedIdentityType>,
) {
    for item in items {
        match item {
            Item::Struct(item) => {
                let owner = item.ident.to_string();
                for field in &item.fields {
                    let Some(name) = &field.ident else {
                        continue;
                    };
                    if let Some(kind) = tracked_identity_type(&field.ty) {
                        fields.insert((owner.clone(), name.to_string()), kind);
                    }
                }
            }
            Item::Mod(item) => {
                if let Some((_, nested)) = &item.content {
                    collect_identity_struct_fields(nested, fields);
                }
            }
            _ => {}
        }
    }
}

fn tracked_identity_type(kind: &Type) -> Option<TrackedIdentityType> {
    if type_is_raw_identity(kind) {
        return Some(String::new());
    }
    match kind {
        Type::Reference(reference) => tracked_identity_type(&reference.elem),
        Type::Paren(paren) => tracked_identity_type(&paren.elem),
        Type::Group(group) => tracked_identity_type(&group.elem),
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

fn infer_identity_parameter_axes(file: &syn::File) -> HashMap<String, Vec<Option<&'static str>>> {
    let mut functions = Vec::new();
    collect_identity_functions(&file.items, &mut functions);
    let type_facts = identity_type_facts(file);
    let mut axes = functions
        .iter()
        .map(|function| {
            function
                .inputs
                .iter()
                .filter_map(|input| match input {
                    FnArg::Typed(input) => Some(input),
                    FnArg::Receiver(_) => None,
                })
                .map(|input| {
                    let syn::Pat::Ident(pattern) = input.pat.as_ref() else {
                        return None;
                    };
                    exact_identity_axis(&pattern.ident.to_string())
                        .filter(|_| type_is_raw_identity(&input.ty))
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let targets = functions.iter().enumerate().fold(
        HashMap::<String, Vec<usize>>::new(),
        |mut targets, (index, function)| {
            targets
                .entry(function.name.clone())
                .or_default()
                .push(index);
            targets
        },
    );
    let propagation_budget = axes.iter().map(Vec::len).sum::<usize>().saturating_add(1);
    for _ in 0..propagation_budget {
        let mut inferred_calls = Vec::new();
        for (function, parameters) in functions.iter().zip(&axes) {
            let mut visitor =
                IdentityCallPropagation::new(function.inputs, parameters, &type_facts);
            visitor.visit_block(function.block);
            inferred_calls.extend(visitor.calls);
        }
        let mut changed = false;
        for (callee, parameter_index, axis) in inferred_calls {
            let Some(candidates) = targets.get(&callee) else {
                continue;
            };
            for candidate in candidates {
                let Some(slot) = axes[*candidate].get_mut(parameter_index) else {
                    continue;
                };
                let raw_parameter = functions[*candidate]
                    .inputs
                    .iter()
                    .filter_map(|input| match input {
                        FnArg::Typed(input) => Some(input),
                        FnArg::Receiver(_) => None,
                    })
                    .nth(parameter_index)
                    .is_some_and(|input| type_is_raw_identity(&input.ty));
                if raw_parameter && slot.is_none() {
                    *slot = Some(axis);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut merged = HashMap::<String, Vec<Option<&'static str>>>::new();
    for (function, parameters) in functions.iter().zip(axes) {
        let entry = merged
            .entry(function.name.clone())
            .or_insert_with(|| vec![None; parameters.len()]);
        if entry.len() < parameters.len() {
            entry.resize(parameters.len(), None);
        }
        for (target, axis) in entry.iter_mut().zip(parameters) {
            if target.is_none() {
                *target = axis;
            }
        }
    }
    merged
}

fn collect_identity_functions<'a>(items: &'a [Item], functions: &mut Vec<IdentityFunction<'a>>) {
    for item in items {
        match item {
            Item::Fn(function) => functions.push(IdentityFunction {
                name: function.sig.ident.to_string(),
                inputs: &function.sig.inputs,
                output: &function.sig.output,
                block: &function.block,
            }),
            Item::Impl(implementation) => {
                for item in &implementation.items {
                    if let ImplItem::Fn(function) = item {
                        functions.push(IdentityFunction {
                            name: function.sig.ident.to_string(),
                            inputs: &function.sig.inputs,
                            output: &function.sig.output,
                            block: &function.block,
                        });
                    }
                }
            }
            Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect_identity_functions(items, functions);
                }
            }
            _ => {}
        }
    }
}

fn infer_function_return_strings(file: &syn::File) -> HashMap<String, Vec<String>> {
    let mut functions = Vec::new();
    collect_identity_functions(&file.items, &mut functions);
    let type_facts = identity_type_facts(file);
    let mut returns = HashMap::<String, Vec<String>>::new();
    for _ in 0..functions.len().saturating_add(1) {
        let mut changed = false;
        for function in &functions {
            let mut values =
                return_strings_for_block(function.inputs, function.block, &returns, &type_facts);
            values.sort();
            values.dedup();
            let entry = returns.entry(function.name.clone()).or_default();
            let before = entry.len();
            entry.extend(values);
            entry.sort();
            entry.dedup();
            changed |= entry.len() != before;
        }
        if !changed {
            break;
        }
    }
    returns
}

fn return_strings_for_block(
    inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
    block: &syn::Block,
    inferred_returns: &HashMap<String, Vec<String>>,
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    struct ReturnStringVisitor<'a> {
        inferred_returns: &'a HashMap<String, Vec<String>>,
        type_facts: &'a IdentityTypeFacts,
        types: Vec<HashMap<String, TrackedIdentityType>>,
        values: Vec<String>,
    }

    impl Visit<'_> for ReturnStringVisitor<'_> {
        fn visit_block(&mut self, node: &syn::Block) {
            self.types.push(HashMap::new());
            syn::visit::visit_block(self, node);
            self.values.extend(block_return_strings_with_returns(
                node,
                self.inferred_returns,
                &self.types,
                self.type_facts,
            ));
            self.types.pop();
        }

        fn visit_local(&mut self, node: &syn::Local) {
            if let Some((name, declared_type)) = local_binding(&node.pat)
                && let Some(initializer) = &node.init
                && let Some(kind) = declared_type.and_then(tracked_identity_type).or_else(|| {
                    expression_tracked_type(&initializer.expr, &self.types, self.type_facts)
                })
            {
                self.types
                    .last_mut()
                    .expect("return-string type scope")
                    .insert(name, kind);
            }
            syn::visit::visit_local(self, node);
        }

        fn visit_expr_return(&mut self, node: &syn::ExprReturn) {
            if let Some(expression) = &node.expr {
                self.values.extend(returned_strings_with_returns(
                    expression,
                    self.inferred_returns,
                    &self.types,
                    self.type_facts,
                ));
            }
            syn::visit::visit_expr_return(self, node);
        }

        fn visit_expr_closure(&mut self, _node: &syn::ExprClosure) {}

        fn visit_item_fn(&mut self, _node: &syn::ItemFn) {}

        fn visit_impl_item_fn(&mut self, _node: &syn::ImplItemFn) {}
    }

    let parameter_types = inputs
        .iter()
        .filter_map(|input| match input {
            FnArg::Typed(input) => Some(input),
            FnArg::Receiver(_) => None,
        })
        .filter_map(|input| {
            let syn::Pat::Ident(pattern) = input.pat.as_ref() else {
                return None;
            };
            tracked_identity_type(&input.ty).map(|kind| (pattern.ident.to_string(), kind))
        })
        .collect::<HashMap<_, _>>();
    let mut visitor = ReturnStringVisitor {
        inferred_returns,
        type_facts,
        types: vec![parameter_types],
        values: Vec::new(),
    };
    visitor.visit_block(block);
    let mut values = visitor.values;
    values.sort();
    values.dedup();
    values
}

struct IdentityCallPropagation<'a> {
    aliases: Vec<HashMap<String, &'static str>>,
    non_identity: Vec<HashSet<String>>,
    types: Vec<HashMap<String, TrackedIdentityType>>,
    type_facts: &'a IdentityTypeFacts,
    calls: Vec<(String, usize, &'static str)>,
}

impl<'a> IdentityCallPropagation<'a> {
    fn new(
        inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
        parameters: &[Option<&'static str>],
        type_facts: &'a IdentityTypeFacts,
    ) -> Self {
        let mut aliases = HashMap::new();
        let mut non_identity = HashSet::new();
        let mut types = HashMap::new();
        let mut typed_index = 0;
        for input in inputs {
            let FnArg::Typed(input) = input else {
                continue;
            };
            let parameter_axis = parameters.get(typed_index).copied().flatten();
            typed_index += 1;
            let syn::Pat::Ident(pattern) = input.pat.as_ref() else {
                continue;
            };
            let name = pattern.ident.to_string();
            if let Some(axis) = parameter_axis {
                aliases.insert(name, axis);
            } else if exact_identity_axis(&name).is_some() && !type_is_raw_identity(&input.ty) {
                non_identity.insert(name);
            }
            if let Some(kind) = tracked_identity_type(&input.ty) {
                types.insert(pattern.ident.to_string(), kind);
            }
        }
        Self {
            aliases: vec![aliases],
            non_identity: vec![non_identity],
            types: vec![types],
            type_facts,
            calls: Vec::new(),
        }
    }

    fn axis(&self, expression: &Expr) -> Option<&'static str> {
        identity_axis(
            expression,
            &self.aliases,
            &self.non_identity,
            &self.types,
            self.type_facts,
        )
    }

    fn record_call<'expr>(&mut self, callee: &str, arguments: impl Iterator<Item = &'expr Expr>) {
        for (index, argument) in arguments.enumerate() {
            if let Some(axis) = self.axis(argument) {
                self.calls.push((callee.to_string(), index, axis));
            }
        }
    }
}

impl Visit<'_> for IdentityCallPropagation<'_> {
    fn visit_block(&mut self, node: &syn::Block) {
        self.aliases.push(HashMap::new());
        self.non_identity.push(HashSet::new());
        self.types.push(HashMap::new());
        syn::visit::visit_block(self, node);
        self.aliases.pop();
        self.non_identity.pop();
        self.types.pop();
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if let Some((name, declared_type)) = local_binding(&node.pat)
            && let Some(initializer) = &node.init
        {
            let declared_axis = exact_identity_axis(&name);
            let initializer_axis = self.axis(&initializer.expr);
            let inferred_type = declared_type.and_then(tracked_identity_type).or_else(|| {
                expression_tracked_type(&initializer.expr, &self.types, self.type_facts)
            });
            let axis = match (declared_axis, declared_type) {
                (Some(axis), Some(kind)) if type_is_raw_identity(kind) => Some(axis),
                (Some(_), Some(_)) => None,
                (Some(axis), None) => Some(axis),
                (None, _) => initializer_axis,
            };
            if let Some(kind) = inferred_type {
                self.types
                    .last_mut()
                    .expect("identity propagation type scope")
                    .insert(name.clone(), kind);
            }
            if let Some(axis) = axis {
                self.aliases
                    .last_mut()
                    .expect("identity propagation scope")
                    .insert(name.clone(), axis);
            } else if declared_axis.is_some() {
                self.non_identity
                    .last_mut()
                    .expect("identity propagation scope")
                    .insert(name);
            }
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_call(&mut self, node: &syn::ExprCall) {
        if let Expr::Path(path) = node.func.as_ref()
            && let Some(segment) = path.path.segments.last()
        {
            self.record_call(&segment.ident.to_string(), node.args.iter());
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &syn::ExprMethodCall) {
        self.record_call(&node.method.to_string(), node.args.iter());
        syn::visit::visit_expr_method_call(self, node);
    }
}

struct IdentityBranchVisitor<'a> {
    path: &'a str,
    violations: Vec<String>,
    errors: Vec<String>,
    aliases: Vec<HashMap<String, &'static str>>,
    non_identity: Vec<HashSet<String>>,
    types: Vec<HashMap<String, TrackedIdentityType>>,
    collections: Vec<HashMap<String, Vec<String>>>,
    return_axis: Option<&'static str>,
    inferred_parameters: HashMap<String, Vec<Option<&'static str>>>,
    inferred_returns: HashMap<String, Vec<String>>,
    type_facts: IdentityTypeFacts,
}

impl Visit<'_> for IdentityBranchVisitor<'_> {
    fn visit_block(&mut self, node: &syn::Block) {
        self.aliases.push(HashMap::new());
        self.non_identity.push(HashSet::new());
        self.types.push(HashMap::new());
        self.collections.push(HashMap::new());
        syn::visit::visit_block(self, node);
        self.aliases.pop();
        self.non_identity.pop();
        self.types.pop();
        self.collections.pop();
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if let Some((name, declared_type)) = local_binding(&node.pat)
            && let Some(initializer) = &node.init
        {
            let declared_axis = exact_identity_axis(&name);
            let initializer_axis = self.axis(&initializer.expr);
            let inferred_type = declared_type.and_then(tracked_identity_type).or_else(|| {
                expression_tracked_type(&initializer.expr, &self.types, &self.type_facts)
            });
            let axis = match (declared_axis, declared_type) {
                (Some(axis), Some(kind)) if type_is_raw_identity(kind) => Some(axis),
                (Some(_), Some(_)) => None,
                (Some(axis), None) => Some(axis),
                (None, _) => initializer_axis,
            };
            if let Some(kind) = inferred_type {
                self.types
                    .last_mut()
                    .expect("identity type scope")
                    .insert(name.clone(), kind);
            }
            if let Some(axis) = axis {
                self.aliases
                    .last_mut()
                    .expect("identity scope")
                    .insert(name.clone(), axis);
            } else if declared_axis.is_some() {
                self.non_identity
                    .last_mut()
                    .expect("identity scope")
                    .insert(name.clone());
            }
            let values = self.strings(&initializer.expr);
            if !values.is_empty() {
                self.collections
                    .last_mut()
                    .expect("collection scope")
                    .insert(name.clone(), values.clone());
                if let Some(axis) = axis {
                    self.record_values(axis, values);
                }
            }
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_binary(&mut self, node: &ExprBinary) {
        if matches!(node.op, BinOp::Eq(_) | BinOp::Ne(_)) {
            if let Some(axis) = self.axis(&node.left) {
                self.record_values(axis, self.strings(&node.right));
            }
            if let Some(axis) = self.axis(&node.right) {
                self.record_values(axis, self.strings(&node.left));
            }
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &syn::ExprMethodCall) {
        if let Some(axis) = self.axis(&node.receiver) {
            self.record_values(
                axis,
                node.args.iter().flat_map(|arg| self.strings(arg)).collect(),
            );
        }
        let receiver_values = self.collection_values(&node.receiver);
        for argument in &node.args {
            if let Some(axis) = self.axis(argument) {
                self.record_values(axis, receiver_values.clone());
                self.record_values(axis, self.strings(&node.receiver));
            }
            if !matches!(argument, Expr::Closure(_)) {
                continue;
            }
            for axis in identity_axes_in_expression(
                argument,
                &self.aliases,
                &self.non_identity,
                &self.types,
                &self.type_facts,
            ) {
                let mut values = self.strings(argument);
                values.extend(receiver_values.iter().cloned());
                self.record_values(axis, values);
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &syn::ExprCall) {
        let callable_values = self.collection_values(&node.func);
        for argument in &node.args {
            if let Some(axis) = self.axis(argument) {
                self.record_values(axis, callable_values.clone());
            }
        }
        syn::visit::visit_expr_call(self, node);
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
                self.record_values(axis, self.strings(&field.expr));
            }
        }
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_assign(&mut self, node: &syn::ExprAssign) {
        if let Some(axis) = self.axis(&node.left) {
            self.record_values(axis, self.strings(&node.right));
        }
        syn::visit::visit_expr_assign(self, node);
    }

    fn visit_expr_return(&mut self, node: &syn::ExprReturn) {
        if let (Some(axis), Some(expression)) = (self.return_axis, &node.expr) {
            self.record_values(axis, self.strings(expression));
        }
        syn::visit::visit_expr_return(self, node);
    }

    fn visit_item_const(&mut self, node: &syn::ItemConst) {
        if let Some(axis) = exact_identity_axis(&node.ident.to_string()) {
            self.record_values(axis, self.strings(&node.expr));
        }
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &syn::ItemStatic) {
        if let Some(axis) = exact_identity_axis(&node.ident.to_string()) {
            self.record_values(axis, self.strings(&node.expr));
        }
        syn::visit::visit_item_static(self, node);
    }

    fn visit_item_fn(&mut self, node: &syn::ItemFn) {
        self.push_parameter_scope(&node.sig.ident.to_string(), &node.sig.inputs);
        let previous = self.return_axis;
        self.return_axis = identity_return_axis(&node.sig.ident.to_string());
        if let Some(axis) = self.return_axis {
            self.record_values(axis, self.block_strings(&node.block));
        }
        syn::visit::visit_item_fn(self, node);
        self.return_axis = previous;
        self.pop_parameter_scope();
    }

    fn visit_impl_item_fn(&mut self, node: &syn::ImplItemFn) {
        self.push_parameter_scope(&node.sig.ident.to_string(), &node.sig.inputs);
        let previous = self.return_axis;
        self.return_axis = identity_return_axis(&node.sig.ident.to_string());
        if let Some(axis) = self.return_axis {
            self.record_values(axis, self.block_strings(&node.block));
        }
        syn::visit::visit_impl_item_fn(self, node);
        self.return_axis = previous;
        self.pop_parameter_scope();
    }
}

impl IdentityBranchVisitor<'_> {
    fn strings(&self, expression: &Expr) -> Vec<String> {
        expression_strings_with_returns(
            expression,
            &self.inferred_returns,
            &self.types,
            &self.type_facts,
        )
    }

    fn block_strings(&self, block: &syn::Block) -> Vec<String> {
        block_tail_strings_with_returns(
            block,
            &self.inferred_returns,
            &self.types,
            &self.type_facts,
        )
    }

    fn axis(&self, expression: &Expr) -> Option<&'static str> {
        identity_axis(
            expression,
            &self.aliases,
            &self.non_identity,
            &self.types,
            &self.type_facts,
        )
    }

    fn push_parameter_scope(
        &mut self,
        function_name: &str,
        inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
    ) {
        let mut aliases = HashMap::new();
        let mut non_identity = HashSet::new();
        let mut types = HashMap::new();
        let inferred = self.inferred_parameters.get(function_name);
        let mut typed_index = 0;
        for input in inputs {
            let FnArg::Typed(input) = input else {
                continue;
            };
            let parameter_index = typed_index;
            typed_index += 1;
            let syn::Pat::Ident(pattern) = input.pat.as_ref() else {
                continue;
            };
            let name = pattern.ident.to_string();
            let declared_axis = exact_identity_axis(&name);
            let inferred_axis = inferred
                .and_then(|parameters| parameters.get(parameter_index))
                .copied()
                .flatten();
            if let Some(axis) = declared_axis.or(inferred_axis)
                && type_is_raw_identity(&input.ty)
            {
                aliases.insert(name, axis);
            } else if declared_axis.is_some() {
                non_identity.insert(name);
            }
            if let Some(kind) = tracked_identity_type(&input.ty) {
                types.insert(pattern.ident.to_string(), kind);
            }
        }
        self.aliases.push(aliases);
        self.non_identity.push(non_identity);
        self.types.push(types);
        self.collections.push(HashMap::new());
    }

    fn pop_parameter_scope(&mut self) {
        self.aliases.pop();
        self.non_identity.pop();
        self.types.pop();
        self.collections.pop();
    }

    fn collection_values(&self, expression: &Expr) -> Vec<String> {
        let direct = self.strings(expression);
        if !direct.is_empty() {
            return direct;
        }
        match expression {
            Expr::Path(path) => path
                .path
                .get_ident()
                .map(ToString::to_string)
                .and_then(|name| {
                    self.collections
                        .iter()
                        .rev()
                        .find_map(|scope| scope.get(&name).cloned())
                })
                .unwrap_or_default(),
            Expr::MethodCall(call) => self.collection_values(&call.receiver),
            Expr::Reference(reference) => self.collection_values(&reference.expr),
            Expr::Paren(paren) => self.collection_values(&paren.expr),
            Expr::Group(group) => self.collection_values(&group.expr),
            _ => Vec::new(),
        }
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

fn tracked_type_for_name(
    name: &str,
    types: &[HashMap<String, TrackedIdentityType>],
) -> Option<TrackedIdentityType> {
    types
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
}

fn expression_tracked_type(
    expression: &Expr,
    types: &[HashMap<String, TrackedIdentityType>],
    facts: &IdentityTypeFacts,
) -> Option<TrackedIdentityType> {
    match expression {
        Expr::Lit(literal) if matches!(literal.lit, Lit::Str(_)) => Some(String::new()),
        Expr::Path(path) => {
            let segments = path.path.segments.iter().collect::<Vec<_>>();
            let last = segments.last()?;
            tracked_type_for_name(&last.ident.to_string(), types).or_else(|| {
                (segments.len() > 1).then(|| segments[segments.len() - 2].ident.to_string())
            })
        }
        Expr::Struct(expression) => expression
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        Expr::Field(field) => {
            let owner = expression_tracked_type(&field.base, types, facts)?;
            if owner.is_empty() {
                return None;
            }
            let Member::Named(field) = &field.member else {
                return None;
            };
            facts
                .struct_fields
                .get(&(owner, field.to_string()))
                .cloned()
        }
        Expr::Call(call) => {
            let Expr::Path(path) = call.func.as_ref() else {
                return None;
            };
            path.path
                .segments
                .last()
                .and_then(|segment| facts.function_returns.get(&segment.ident.to_string()))
                .cloned()
        }
        Expr::MethodCall(call)
            if identity_passthrough_method(&call.method.to_string())
                && expression_tracked_type(&call.receiver, types, facts) == Some(String::new()) =>
        {
            Some(String::new())
        }
        Expr::Paren(expression) => expression_tracked_type(&expression.expr, types, facts),
        Expr::Group(expression) => expression_tracked_type(&expression.expr, types, facts),
        Expr::Reference(expression) => expression_tracked_type(&expression.expr, types, facts),
        Expr::Try(expression) => expression_tracked_type(&expression.expr, types, facts),
        Expr::Await(expression) => expression_tracked_type(&expression.base, types, facts),
        Expr::Cast(expression) => tracked_identity_type(&expression.ty),
        _ => None,
    }
}

fn identity_axis(
    expression: &Expr,
    aliases: &[HashMap<String, &'static str>],
    non_identity: &[HashSet<String>],
    types: &[HashMap<String, TrackedIdentityType>],
    facts: &IdentityTypeFacts,
) -> Option<&'static str> {
    match expression {
        Expr::Field(field) => {
            if expression_tracked_type(expression, types, facts)
                .is_some_and(|kind| !kind.is_empty())
            {
                return None;
            }
            match &field.member {
                Member::Named(identifier) => exact_identity_axis(&identifier.to_string()),
                Member::Unnamed(_) => None,
            }
        }
        Expr::Path(path) => path.path.segments.last().and_then(|segment| {
            let name = segment.ident.to_string();
            if non_identity.iter().rev().any(|scope| scope.contains(&name)) {
                return None;
            }
            if tracked_type_for_name(&name, types).is_some_and(|kind| !kind.is_empty()) {
                return None;
            }
            aliases
                .iter()
                .rev()
                .find_map(|scope| scope.get(&name).copied())
                .or_else(|| exact_identity_axis(&name))
        }),
        Expr::Paren(paren) => identity_axis(&paren.expr, aliases, non_identity, types, facts),
        Expr::Group(group) => identity_axis(&group.expr, aliases, non_identity, types, facts),
        Expr::Reference(reference) => {
            identity_axis(&reference.expr, aliases, non_identity, types, facts)
        }
        Expr::Try(expression) => {
            identity_axis(&expression.expr, aliases, non_identity, types, facts)
        }
        Expr::Await(expression) => {
            identity_axis(&expression.base, aliases, non_identity, types, facts)
        }
        Expr::Cast(expression) => {
            identity_axis(&expression.expr, aliases, non_identity, types, facts)
        }
        Expr::Unary(expression) => {
            identity_axis(&expression.expr, aliases, non_identity, types, facts)
        }
        Expr::MethodCall(call)
            if expression_tracked_type(&call.receiver, types, facts) == Some(String::new())
                && identity_passthrough_method(&call.method.to_string()) =>
        {
            identity_axis(&call.receiver, aliases, non_identity, types, facts)
        }
        Expr::MethodCall(_) => None,
        Expr::Call(call)
            if expression_tracked_type(expression, types, facts) == Some(String::new()) =>
        {
            call.args
                .iter()
                .find_map(|argument| identity_axis(argument, aliases, non_identity, types, facts))
        }
        _ => None,
    }
}

fn identity_passthrough_method(method: &str) -> bool {
    matches!(
        method,
        "as_deref"
            | "as_ref"
            | "as_str"
            | "clone"
            | "copied"
            | "deref"
            | "expect"
            | "to_owned"
            | "to_string"
            | "trim"
            | "trim_end"
            | "trim_start"
            | "unwrap"
            | "unwrap_or"
            | "unwrap_or_else"
    )
}

fn identity_axes_in_expression(
    expression: &Expr,
    aliases: &[HashMap<String, &'static str>],
    non_identity: &[HashSet<String>],
    types: &[HashMap<String, TrackedIdentityType>],
    facts: &IdentityTypeFacts,
) -> Vec<&'static str> {
    struct AxisUseVisitor<'a> {
        aliases: &'a [HashMap<String, &'static str>],
        non_identity: &'a [HashSet<String>],
        types: &'a [HashMap<String, TrackedIdentityType>],
        facts: &'a IdentityTypeFacts,
        axes: HashSet<&'static str>,
    }

    impl Visit<'_> for AxisUseVisitor<'_> {
        fn visit_expr(&mut self, node: &Expr) {
            if let Some(axis) = identity_axis(
                node,
                self.aliases,
                self.non_identity,
                self.types,
                self.facts,
            ) {
                self.axes.insert(axis);
            }
            syn::visit::visit_expr(self, node);
        }
    }

    let mut visitor = AxisUseVisitor {
        aliases,
        non_identity,
        types,
        facts,
        axes: HashSet::new(),
    };
    visitor.visit_expr(expression);
    let mut axes = visitor.axes.into_iter().collect::<Vec<_>>();
    axes.sort();
    axes
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

fn returned_strings_with_returns(
    expression: &Expr,
    inferred_returns: &HashMap<String, Vec<String>>,
    types: &[HashMap<String, TrackedIdentityType>],
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(value) => vec![value.value()],
            _ => Vec::new(),
        },
        Expr::Array(array) => array
            .elems
            .iter()
            .flat_map(|item| {
                returned_strings_with_returns(item, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Tuple(tuple) => tuple
            .elems
            .iter()
            .flat_map(|item| {
                returned_strings_with_returns(item, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Paren(paren) => {
            returned_strings_with_returns(&paren.expr, inferred_returns, types, type_facts)
        }
        Expr::Group(group) => {
            returned_strings_with_returns(&group.expr, inferred_returns, types, type_facts)
        }
        Expr::Reference(reference) => {
            returned_strings_with_returns(&reference.expr, inferred_returns, types, type_facts)
        }
        Expr::Try(expression) => {
            returned_strings_with_returns(&expression.expr, inferred_returns, types, type_facts)
        }
        Expr::Await(expression) => {
            returned_strings_with_returns(&expression.base, inferred_returns, types, type_facts)
        }
        Expr::Call(call) => {
            let callee = if let Expr::Path(path) = call.func.as_ref() {
                path.path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
            } else {
                None
            };
            if let Some(values) = callee
                .as_ref()
                .and_then(|callee| inferred_returns.get(callee))
            {
                values.clone()
            } else if callee
                .as_deref()
                .is_some_and(|callee| matches!(callee, "Some" | "Ok" | "Borrowed" | "Owned"))
            {
                call.args
                    .iter()
                    .flat_map(|argument| {
                        returned_strings_with_returns(argument, inferred_returns, types, type_facts)
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        Expr::MethodCall(call) => {
            let method = call.method.to_string();
            if identity_passthrough_method(&method)
                && expression_tracked_type(&call.receiver, types, type_facts) == Some(String::new())
            {
                let mut values = returned_strings_with_returns(
                    &call.receiver,
                    inferred_returns,
                    types,
                    type_facts,
                );
                if matches!(method.as_str(), "unwrap_or" | "unwrap_or_else") {
                    values.extend(call.args.iter().flat_map(|argument| {
                        returned_strings_with_returns(argument, inferred_returns, types, type_facts)
                    }));
                }
                values
            } else {
                Vec::new()
            }
        }
        Expr::Block(block) => {
            block_return_strings_with_returns(&block.block, inferred_returns, types, type_facts)
        }
        Expr::If(expression) => {
            let mut values = block_return_strings_with_returns(
                &expression.then_branch,
                inferred_returns,
                types,
                type_facts,
            );
            if let Some((_, otherwise)) = &expression.else_branch {
                values.extend(returned_strings_with_returns(
                    otherwise,
                    inferred_returns,
                    types,
                    type_facts,
                ));
            }
            values
        }
        Expr::Match(expression) => expression
            .arms
            .iter()
            .flat_map(|arm| {
                returned_strings_with_returns(&arm.body, inferred_returns, types, type_facts)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn block_return_strings_with_returns(
    block: &syn::Block,
    inferred_returns: &HashMap<String, Vec<String>>,
    types: &[HashMap<String, TrackedIdentityType>],
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    block
        .stmts
        .last()
        .and_then(|statement| match statement {
            syn::Stmt::Expr(expression, None) => Some(returned_strings_with_returns(
                expression,
                inferred_returns,
                types,
                type_facts,
            )),
            _ => None,
        })
        .unwrap_or_default()
}

fn expression_strings_with_returns(
    expression: &Expr,
    inferred_returns: &HashMap<String, Vec<String>>,
    types: &[HashMap<String, TrackedIdentityType>],
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(value) => vec![value.value()],
            _ => Vec::new(),
        },
        Expr::Array(array) => array
            .elems
            .iter()
            .flat_map(|item| {
                expression_strings_with_returns(item, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Tuple(tuple) => tuple
            .elems
            .iter()
            .flat_map(|item| {
                expression_strings_with_returns(item, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Paren(paren) => {
            expression_strings_with_returns(&paren.expr, inferred_returns, types, type_facts)
        }
        Expr::Group(group) => {
            expression_strings_with_returns(&group.expr, inferred_returns, types, type_facts)
        }
        Expr::Reference(reference) => {
            expression_strings_with_returns(&reference.expr, inferred_returns, types, type_facts)
        }
        Expr::Call(call) => {
            let known = if let Expr::Path(path) = call.func.as_ref() {
                path.path
                    .segments
                    .last()
                    .and_then(|segment| inferred_returns.get(&segment.ident.to_string()))
                    .cloned()
            } else {
                None
            };
            known.unwrap_or_else(|| {
                call.args
                    .iter()
                    .flat_map(|argument| {
                        expression_strings_with_returns(
                            argument,
                            inferred_returns,
                            types,
                            type_facts,
                        )
                    })
                    .collect()
            })
        }
        Expr::MethodCall(call) => {
            let method = call.method.to_string();
            if identity_passthrough_method(&method)
                && expression_tracked_type(&call.receiver, types, type_facts) == Some(String::new())
            {
                let mut values = expression_strings_with_returns(
                    &call.receiver,
                    inferred_returns,
                    types,
                    type_facts,
                );
                if matches!(method.as_str(), "unwrap_or" | "unwrap_or_else") {
                    values.extend(call.args.iter().flat_map(|argument| {
                        expression_strings_with_returns(
                            argument,
                            inferred_returns,
                            types,
                            type_facts,
                        )
                    }));
                }
                values
            } else {
                call.args
                    .iter()
                    .flat_map(|argument| {
                        expression_strings_with_returns(
                            argument,
                            inferred_returns,
                            types,
                            type_facts,
                        )
                    })
                    .collect()
            }
        }
        Expr::Closure(closure) => literal_strings_in_expression(&closure.body),
        Expr::Block(block) => {
            block_tail_strings_with_returns(&block.block, inferred_returns, types, type_facts)
        }
        Expr::If(expression) => {
            let mut values = block_tail_strings_with_returns(
                &expression.then_branch,
                inferred_returns,
                types,
                type_facts,
            );
            if let Some((_, otherwise)) = &expression.else_branch {
                values.extend(expression_strings_with_returns(
                    otherwise,
                    inferred_returns,
                    types,
                    type_facts,
                ));
            }
            values
        }
        Expr::Match(expression) => expression
            .arms
            .iter()
            .flat_map(|arm| {
                expression_strings_with_returns(&arm.body, inferred_returns, types, type_facts)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn literal_strings_in_expression(expression: &Expr) -> Vec<String> {
    #[derive(Default)]
    struct LiteralStringVisitor {
        values: Vec<String>,
    }

    impl Visit<'_> for LiteralStringVisitor {
        fn visit_lit_str(&mut self, node: &LitStr) {
            self.values.push(node.value());
        }
    }

    let mut visitor = LiteralStringVisitor::default();
    visitor.visit_expr(expression);
    visitor.values.sort();
    visitor.values.dedup();
    visitor.values
}

fn block_tail_strings_with_returns(
    block: &syn::Block,
    inferred_returns: &HashMap<String, Vec<String>>,
    types: &[HashMap<String, TrackedIdentityType>],
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    block
        .stmts
        .last()
        .and_then(|statement| match statement {
            syn::Stmt::Expr(expression, None) => Some(expression_strings_with_returns(
                expression,
                inferred_returns,
                types,
                type_facts,
            )),
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

    #[test]
    fn bounded_reader_rejects_oversized_regular_file_before_content_read() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "actingcommand-generic-domain-oversized-{}-{nonce}",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        file.set_len(MAX_PROTECTED_FILE_BYTES + 1).unwrap();

        let error = read_bounded_regular_text(&path).unwrap_err();
        assert!(error.contains("protected-file limit"));

        fs::remove_file(path).unwrap();
    }

    fn registry_source() -> String {
        let surface_id = surface_id_for(
            "rust_public_item",
            "crates/example/src/lib.rs",
            "struct:Summary",
        );
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
surface_id = "{surface_id}"
kind = "rust_public_item"
stable_path = "crates/example/src/lib.rs"
selector = "struct:Summary"
concept_ids = ["identity.game", "structure.value"]
fingerprint = "{}"
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
        assert!(error.contains("approval_comment_id must be 5010683904"));
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
    fn identity_branches_follow_multilevel_private_helper_returns() {
        let source = r#"
            fn hidden() -> &'static str { "synthetic_project_code" }
            fn neutral_wrapper() -> &'static str { hidden() }
            fn default_game() -> &'static str { neutral_wrapper() }
            fn route(game: &str) -> bool { game == default_game() }
        "#;
        let violations = inspect_identity_axis_branches("fixture.rs", source).unwrap();
        assert!(
            violations
                .iter()
                .any(|item| item.contains("synthetic_project_code") && item.contains("game")),
            "{violations:#?}"
        );
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
                let owned = game.trim().to_owned();
                let _ = game.eq_ignore_ascii_case("game.direct");
                let _ = owned.as_str().trim() == "game.raw-chain";
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
            "game.raw-chain",
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
    fn identity_data_flow_propagates_into_neutrally_named_private_helpers() {
        let source = r#"
            fn private_select(candidate: &str) -> bool {
                candidate == "synthetic_project_code"
            }

            fn route(game: &str) -> bool {
                private_select(game)
            }
        "#;
        let violations = inspect_identity_axis_branches("private-helper.rs", source).unwrap();
        assert!(violations.iter().any(|violation| {
            violation.contains("synthetic_project_code") && violation.contains("axis game")
        }));
    }

    #[test]
    fn typed_identity_and_neighboring_names_remain_allowed() {
        let source = r#"
            enum GameIdentity { Alpha, Beta }
            impl GameIdentity {
                fn content_sha256(&self) -> &'static str { "sha256:neutral" }
                fn as_str(&self) -> &'static str { "custom-as-str" }
                fn trim(&self) -> &'static str { "custom-trim" }
                fn to_owned(&self) -> &'static str { "custom-to-owned" }
            }
            struct Context { game: GameIdentity }

            fn prepare() -> GameIdentity {
                Some(GameIdentity::Alpha).expect("prepared task")
            }

            fn consume(value: &str) {
                assert!(value.starts_with("sha256:"), "stage release");
            }

            fn inspect(context: &Context) -> bool {
                let provider_count = "neutral";
                let task = prepare();
                let resource = GameIdentity::Beta;
                consume(resource.content_sha256());
                consume(resource.as_str());
                consume(resource.trim());
                consume(resource.to_owned());
                context.game == GameIdentity::Alpha
                    && task == GameIdentity::Alpha
                    && provider_count == "neutral"
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
        run_git(&root, &["add", "--", "crates/example/src/lib.rs"]);
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface rust_public_field"));
        assert!(error.contains("field:Summary::status"));

        fs::write(
            root.join("crates/example/src/lib.rs"),
            "#[arg(long, default_value = \"changed\")]\nstruct Cli;\n",
        )
        .unwrap();
        run_git(&root, &["add", "--", "crates/example/src/lib.rs"]);
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
        let fragment =
            identity_fragments_for_source(path, &fs::read_to_string(root.join(path)).unwrap())
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
        });
        validate_generic_domain_registry(&registry).unwrap();
        validate_workspace_genericity(&root, &registry).unwrap();

        fs::write(root.join(path), "fn compile_maa_jobs() {}\n").unwrap();
        run_git(&root, &["add", "--", path]);
        let error = validate_workspace_genericity(&root, &registry).unwrap_err();
        assert!(error.contains("missing selector") || error.contains("fragment hash drifted"));

        fs::create_dir_all(root.join("scratch")).unwrap();
        let outside = "scratch/outside.rs";
        fs::write(root.join(outside), "fn compile_maa_tasks() {}\n").unwrap();
        let outside_fragment = identity_fragments_for_source(
            outside,
            &fs::read_to_string(root.join(outside)).unwrap(),
        )
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
        let fragment =
            identity_fragments_for_source(path, &fs::read_to_string(root.join(path)).unwrap())
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
        let fragment =
            identity_fragments_for_source(path, &fs::read_to_string(root.join(path)).unwrap())
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
        run_git(&root, &["add", "--", "contracts/example.schema.json"]);
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface structured_key"));
        assert!(error.contains("/properties/game/default"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_registry_rejects_unregistered_public_rust_default_change() {
        let root = temporary_workspace("rust-default-drift");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        let path = root.join("crates/example/src/lib.rs");
        fs::write(
            &path,
            r#"
                pub struct Settings { pub enabled: bool }
                impl Default for Settings {
                    fn default() -> Self { Self { enabled: true } }
                }
            "#,
        )
        .unwrap();
        create_required_roots(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let registry = registry_for_snapshots(&snapshots);
        validate_workspace_surface_registry(&root, &registry).unwrap();

        fs::write(
            &path,
            r#"
                pub struct Settings { pub enabled: bool }
                impl Default for Settings {
                    fn default() -> Self { Self { enabled: false } }
                }
            "#,
        )
        .unwrap();
        run_git(&root, &["add", "--", "crates/example/src/lib.rs"]);
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("rust_default_impl"));
        assert!(error.contains("fingerprint drifted"));

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
        run_git(&root, &["add", "--", "crates/example/src/lib.rs"]);
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface rust_macro_variant"));
        assert!(error.contains("macro_variant:Status::SyntheticFaction"));
        assert!(error.contains("unmapped protected surface rust_macro_wire_value"));
        assert!(error.contains("macro_wire:Status::SyntheticFaction"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cargo_metadata_auto_covers_path_dependency_workspace_member() {
        let root = temporary_workspace("metadata-path-member");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::write(
            root.join("crates/example/Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\nhidden = { path = \"../hidden\" }\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "pub struct Example;\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/hidden/src")).unwrap();
        fs::write(
            root.join("crates/hidden/Cargo.toml"),
            "[package]\nname = \"hidden\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/hidden/src/lib.rs"),
            "pub struct Hidden;\n",
        )
        .unwrap();
        create_required_roots(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        assert!(
            snapshots
                .iter()
                .any(|surface| surface.stable_path == "crates/hidden/src/lib.rs")
        );
        let registry = registry_for_snapshots(&snapshots);
        fs::write(
            root.join("crates/hidden/src/lib.rs"),
            "pub struct Hidden;\npub struct Added;\n",
        )
        .unwrap();
        run_git(&root, &["add", "--", "crates/hidden/src/lib.rs"]);
        let error = validate_workspace_surface_registry(&root, &registry).unwrap_err();
        assert!(error.contains("unmapped protected surface rust_public_item"));
        assert!(error.contains("crates/hidden/src/lib.rs"));
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
        assert!(error.contains("unclassified tracked file"));
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
            "const SHADOW: &str = include_str!(\"../../../shadow-space/value.random\");\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("shadow-space")).unwrap();
        fs::write(root.join("shadow-space/value.random"), "hidden\n").unwrap();
        create_required_roots(&root);

        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("unclassified tracked file shadow-space/value.random"));
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
        assert!(error.contains(
            "tracked Cargo package is not a Cargo metadata workspace member: crates/excluded"
        ));
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
        run_git(&root, &["add", "--", "crates/example/src/lib.rs"]);

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
        run_git(&root, &["add", "--", "crates/example/src/lib.rs"]);

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
        let source = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let error = cargo_compile_inputs("Cargo.toml", &source, &HashSet::new()).unwrap_err();
        assert!(error.contains("approved runtime CommandFactory extractor"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cargo_compile_inputs_registers_static_build_script() {
        let root = temporary_workspace("build-script-extractor");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("build.rs"), "fn main() {}\n").unwrap();
        let source = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let tracked = HashSet::from(["Cargo.toml", "build.rs"]);
        let inputs = cargo_compile_inputs("Cargo.toml", &source, &tracked).unwrap();
        assert_eq!(inputs, ["build.rs"]);
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
        fs::create_dir_all(root.join("contracts")).unwrap();
        write_external_compat_manifest(root);
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
"#
        )
    }

    fn registry_for_snapshots(snapshots: &[SurfaceSnapshot]) -> GenericDomainRegistry {
        GenericDomainRegistry {
            schema_version: GENERIC_DOMAIN_SCHEMA_VERSION.to_string(),
            surface_manifest: None,
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
                })
                .collect(),
        }
    }
}
