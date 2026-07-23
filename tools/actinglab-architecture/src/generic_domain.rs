// SPDX-License-Identifier: AGPL-3.0-only

//! Machine-readable generic-domain concepts and protected Runtime surfaces.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

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
    pub concept: Vec<GenericConcept>,
    #[serde(default)]
    pub mapping_source: Vec<MappingSource>,
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
    pub semantic_role: SemanticRole,
    pub stable_path: String,
    pub selector: String,
    pub concept_ids: Vec<String>,
    pub fingerprint: String,
    pub mapping_source_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRole {
    Contract,
    Wire,
    Schema,
    Cli,
    Default,
    Template,
    TaskDefinition,
    IdentityBranch,
    TestFixtureGolden,
}

impl SemanticRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Contract => "contract",
            Self::Wire => "wire",
            Self::Schema => "schema",
            Self::Cli => "cli",
            Self::Default => "default",
            Self::Template => "template",
            Self::TaskDefinition => "task_definition",
            Self::IdentityBranch => "identity_branch",
            Self::TestFixtureGolden => "test_fixture_golden",
        }
    }

    fn anchor_concept(self) -> &'static str {
        match self {
            Self::Contract => "interface.contract",
            Self::Wire => "interface.payload",
            Self::Schema => "catalog.schema",
            Self::Cli => "interface.cli",
            Self::Default => "decision.policy",
            Self::Template => "catalog.template",
            Self::TaskDefinition => "catalog.task",
            Self::IdentityBranch => "identity.scope",
            Self::TestFixtureGolden => "catalog.validation",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MappingSource {
    pub id: String,
    pub task_issue: u64,
    pub implementation_pr: u64,
    pub source_kind: String,
    pub change_kind: String,
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
    pub semantic_role: SemanticRole,
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
    reject_external_generic_domain_registries(path)?;
    parse_generic_domain_registry(&source)
}

fn reject_external_generic_domain_registries(path: &Path) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("generic-domain registry has no parent: {}", path.display()))?;
    let expected = path
        .file_name()
        .ok_or_else(|| format!("generic-domain registry has no file name: {}", path.display()))?;
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("failed to inspect {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| {
            format!(
                "failed to inspect generic-domain registry directory {}: {error}",
                directory.display()
            )
        })?;
        let file_name = entry.file_name();
        if file_name == expected {
            continue;
        }
        let file_name = file_name.to_string_lossy();
        let extension = entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if file_name.starts_with("generic-domain-")
            && matches!(extension.as_str(), "json" | "jsonl" | "toml")
        {
            return Err(format!(
                "generic-domain surfaces must be inline in {}; external registry {} is forbidden",
                path.display(),
                entry.path().display()
            ));
        }
    }
    Ok(())
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
    if registry.mapping_source.is_empty() {
        errors.push("generic-domain registry contains no mapping sources".to_string());
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
        match semantic_role_for_kind(&surface.kind) {
            Some(expected) if expected != surface.semantic_role => errors.push(format!(
                "surface {} role {} is incompatible with kind {}; expected {}",
                surface.surface_id,
                surface.semantic_role.as_str(),
                surface.kind,
                expected.as_str()
            )),
            Some(_) => {}
            None => errors.push(format!(
                "surface {} has unknown protected kind {}",
                surface.surface_id, surface.kind
            )),
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
        let anchor = surface.semantic_role.anchor_concept();
        if !surface
            .concept_ids
            .iter()
            .any(|concept_id| concept_id == anchor)
        {
            errors.push(format!(
                "surface {} role {} requires anchor concept {anchor}",
                surface.surface_id,
                surface.semantic_role.as_str()
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
            if !concept_is_applicable(surface.semantic_role, concept_id) {
                errors.push(format!(
                    "surface {} role {} cannot map concept {concept_id}",
                    surface.surface_id,
                    surface.semantic_role.as_str()
                ));
            }
        }
        if !is_sha256(&surface.fingerprint) {
            errors.push(format!(
                "surface {} fingerprint must be lowercase SHA-256",
                surface.surface_id
            ));
        }
        if !is_surface_id(&surface.mapping_source_id) {
            errors.push(format!(
                "surface {} has invalid mapping_source_id {}",
                surface.surface_id, surface.mapping_source_id
            ));
        }
    }

    validate_mapping_sources(registry, &surface_ids, &mut errors);

    finish_errors(errors)
}

fn semantic_role_for_kind(kind: &str) -> Option<SemanticRole> {
    match kind {
        "rust_public_item"
        | "rust_public_field"
        | "rust_public_variant"
        | "rust_public_impl_item"
        | "rust_ffi_attribute"
        | "rust_ffi_item"
        | "rust_macro_item"
        | "rust_macro_invocation"
        | "rust_macro_variant"
        | "rust_contract_attribute"
        | "rust_contract_carrier" => Some(SemanticRole::Contract),
        "rust_wire_item"
        | "rust_wire_field"
        | "rust_wire_variant"
        | "rust_wire_impl"
        | "rust_wire_attribute"
        | "rust_macro_wire_value"
        | "rust_wire_carrier" => Some(SemanticRole::Wire),
        "schema_key" | "schema_value" | "schema_carrier" => Some(SemanticRole::Schema),
        "rust_cli_attribute" | "rust_cli_carrier" => Some(SemanticRole::Cli),
        "rust_default_impl" | "rust_default_attribute" | "rust_default_carrier" => {
            Some(SemanticRole::Default)
        }
        "rust_template_carrier" => Some(SemanticRole::Template),
        "rust_task_definition_carrier" => Some(SemanticRole::TaskDefinition),
        "rust_identity_branch_carrier" => Some(SemanticRole::IdentityBranch),
        "rust_test_fixture_carrier" => Some(SemanticRole::TestFixtureGolden),
        _ => None,
    }
}

fn concept_is_applicable(role: SemanticRole, concept_id: &str) -> bool {
    let family = concept_id
        .split_once('.')
        .map(|(family, _)| family)
        .unwrap_or_default();
    match role {
        SemanticRole::Contract | SemanticRole::Wire | SemanticRole::TestFixtureGolden => {
            APPROVED_CONCEPT_FAMILIES.contains(&family)
        }
        SemanticRole::Schema => matches!(
            family,
            "catalog"
                | "decision"
                | "identity"
                | "interface"
                | "release"
                | "state_value"
                | "structure"
                | "time"
                | "unit_qualifier"
        ),
        SemanticRole::Cli => matches!(
            family,
            "artifact"
                | "catalog"
                | "decision"
                | "device"
                | "execution"
                | "identity"
                | "interface"
                | "operation"
                | "release"
                | "schedule"
                | "state_value"
                | "structure"
                | "unit_qualifier"
        ),
        SemanticRole::Default => matches!(
            family,
            "catalog"
                | "decision"
                | "execution"
                | "fact"
                | "identity"
                | "interface"
                | "release"
                | "schedule"
                | "state_value"
                | "structure"
                | "time"
                | "unit_qualifier"
        ),
        SemanticRole::Template => matches!(
            family,
            "artifact"
                | "catalog"
                | "decision"
                | "device"
                | "execution"
                | "identity"
                | "interface"
                | "recognition"
                | "release"
                | "schedule"
                | "state_value"
                | "structure"
        ),
        SemanticRole::TaskDefinition => matches!(
            family,
            "artifact"
                | "catalog"
                | "decision"
                | "device"
                | "execution"
                | "identity"
                | "interface"
                | "recognition"
                | "schedule"
                | "state_value"
                | "structure"
                | "time"
        ),
        SemanticRole::IdentityBranch => matches!(
            family,
            "catalog" | "decision" | "identity" | "interface" | "operation" | "structure"
        ),
    }
}

fn validate_mapping_sources(
    registry: &GenericDomainRegistry,
    surface_ids: &HashSet<&str>,
    errors: &mut Vec<String>,
) {
    let mut source_ids = HashSet::new();
    let mut previous = None;
    for source in &registry.mapping_source {
        if !is_surface_id(&source.id) || !source_ids.insert(source.id.as_str()) {
            errors.push(format!(
                "mapping source has invalid or duplicate id {}",
                source.id
            ));
        }
        if previous.is_some_and(|left: &str| left >= source.id.as_str()) {
            errors.push(format!(
                "mapping source ids are not strictly sorted at {}",
                source.id
            ));
        }
        previous = Some(source.id.as_str());
        if source.task_issue == 0 || source.implementation_pr == 0 {
            errors.push(format!(
                "mapping source {} must bind nonzero task_issue and implementation_pr",
                source.id
            ));
        }
        if source.source_kind != "workflow_task" {
            errors.push(format!(
                "mapping source {} has invalid source_kind {}",
                source.id, source.source_kind
            ));
        }
        if !matches!(
            source.change_kind.as_str(),
            "initial_import" | "repair" | "extension" | "correction"
        ) {
            errors.push(format!(
                "mapping source {} has invalid change_kind {}",
                source.id, source.change_kind
            ));
        }
    }

    for surface in &registry.surface {
        if !source_ids.contains(surface.mapping_source_id.as_str()) {
            errors.push(format!(
                "surface {} references missing mapping source {}",
                surface.surface_id, surface.mapping_source_id
            ));
        }
    }
    for source in &registry.mapping_source {
        let mut mapped = registry
            .surface
            .iter()
            .filter(|surface| surface.mapping_source_id == source.id)
            .collect::<Vec<_>>();
        mapped.sort_by(|left, right| left.surface_id.cmp(&right.surface_id));
        if mapped.is_empty() {
            errors.push(format!("mapping source {} is unreferenced", source.id));
            continue;
        }
        let expected = mapping_source_id(source, &mapped);
        if source.id != expected {
            errors.push(format!(
                "mapping source {} is not content-bound; expected {expected}",
                source.id
            ));
        }
        for surface in mapped {
            if !surface_ids.contains(surface.surface_id.as_str()) {
                errors.push(format!(
                    "mapping source {} references unknown surface {}",
                    source.id, surface.surface_id
                ));
            }
        }
    }
}

fn mapping_source_id(source: &MappingSource, surfaces: &[&ProtectedSurface]) -> String {
    let mut canonical = format!(
        "{}\0{}\0{}\0{}\n",
        source.task_issue, source.implementation_pr, source.source_kind, source.change_kind
    );
    for surface in surfaces {
        canonical.push_str(&surface.surface_id);
        canonical.push('\0');
        canonical.push_str(surface.semantic_role.as_str());
        canonical.push('\0');
        canonical.push_str(&surface.concept_ids.join(","));
        canonical.push('\n');
    }
    let digest = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    format!(
        "mapping_source.issue{}_pr{}_{}_{}",
        source.task_issue,
        source.implementation_pr,
        source.change_kind,
        &digest[..24]
    )
}

pub fn workspace_surface_snapshot(root: &Path) -> Result<Vec<SurfaceSnapshot>, String> {
    let profile_started = Instant::now();
    let files = protected_files(root)?;
    eprintln!(
        "PERF workspace_surface_snapshot protected_files={} elapsed_ms={}",
        files.len(),
        profile_started.elapsed().as_millis()
    );

    let mut snapshots = Vec::new();
    let mut rust_sources = HashMap::<String, syn::File>::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped workspace root", file.display()))?;
        let relative = normalize_path(relative)?;
        if relative == GENERIC_DOMAIN_REGISTRY_PATH {
            continue;
        }
        if file.extension().is_some_and(|extension| extension == "rs") {
            let source = read_bounded_regular_text(&file)?;
            let parsed = syn::parse_file(&source)
                .map_err(|error| format!("failed to parse {relative}: {error}"))?;
            snapshots.extend(snapshots_from_raw(
                &relative,
                rust_base_surface_inventory(&parsed)?,
            )?);
            rust_sources.insert(relative, parsed);
        } else {
            snapshots.extend(snapshot_for_file(&file, &relative)?);
        }
    }
    eprintln!(
        "PERF workspace_surface_snapshot base_surfaces={} rust_sources={} elapsed_ms={}",
        snapshots.len(),
        rust_sources.len(),
        profile_started.elapsed().as_millis()
    );
    for (stable_path, surface) in workspace_rust_carrier_inventory(root, &rust_sources)? {
        snapshots.extend(snapshots_from_raw(&stable_path, vec![surface])?);
    }
    eprintln!(
        "PERF workspace_surface_snapshot with_carriers={} elapsed_ms={}",
        snapshots.len(),
        profile_started.elapsed().as_millis()
    );
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

fn workspace_rust_carrier_inventory(
    root: &Path,
    sources: &HashMap<String, syn::File>,
) -> Result<Vec<(String, RawSurface)>, String> {
    let graph = build_workspace_rust_graph(root, sources)?;
    finish_rust_carrier_catalog(graph.catalog, &graph.type_facts)
}

struct RustWorkspaceGraph {
    catalog: RustCarrierCatalog,
    type_facts: IdentityTypeFacts,
    assignments: Vec<RustSourceAssignment>,
}

fn build_workspace_rust_graph(
    root: &Path,
    sources: &HashMap<String, syn::File>,
) -> Result<RustWorkspaceGraph, String> {
    let profile_started = Instant::now();
    let metadata = load_cargo_metadata(root)?;
    let workspace_ids = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let tracked_paths = git_tracked_files(root)?
        .into_iter()
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    let tracked = tracked_paths.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut catalog = RustCarrierCatalog::default();
    let mut assignments = Vec::<RustSourceAssignment>::new();
    let mut ownership = HashMap::<String, Vec<String>>::new();
    let mut errors = Vec::new();

    for package in metadata
        .packages
        .iter()
        .filter(|package| workspace_ids.contains(package.id.as_str()))
    {
        let manifest = metadata_relative_path(root, &package.manifest_path)?;
        for target in &package.targets {
            let target_root = metadata_relative_path(root, &target.src_path)?;
            if !sources.contains_key(&target_root) {
                errors.push(format!(
                    "Cargo target {} ({}) root {target_root} is not a protected tracked Rust source",
                    target.name,
                    target.kind.join(",")
                ));
                continue;
            }
            let unit_id = short_hash(&format!(
                "{manifest}\0{}\0{}\0{target_root}",
                target.name,
                target.kind.join(",")
            ));
            let unit_root = format!("$unit${unit_id}");
            let mut inherited_roles = BTreeSet::new();
            if target
                .kind
                .iter()
                .any(|kind| matches!(kind.as_str(), "test" | "bench"))
            {
                inherited_roles.insert(SemanticRole::TestFixtureGolden);
            }
            let mut external_crates = package
                .dependencies
                .iter()
                .map(|dependency| {
                    dependency
                        .rename
                        .as_deref()
                        .unwrap_or(&dependency.name)
                        .replace('-', "_")
                })
                .collect::<HashSet<_>>();
            external_crates.insert(package.name.replace('-', "_"));
            collect_rust_compile_unit_assignments(
                &target.name,
                RustSourceAssignment {
                stable_path: target_root.clone(),
                module: vec![unit_root.clone()],
                inherited_roles: inherited_roles.clone(),
                include_stack: vec![target_root.clone()],
                external_crates: external_crates.clone(),
                },
                &tracked,
                sources,
                &mut assignments,
                &mut ownership,
                &mut errors,
            );
        }
    }
    let unowned_test_fixtures = sources
        .keys()
        .filter(|path| !ownership.contains_key(*path) && is_test_source_path(path))
        .cloned()
        .collect::<Vec<_>>();
    for stable_path in unowned_test_fixtures {
        let candidates = metadata
            .packages
            .iter()
            .filter(|package| workspace_ids.contains(package.id.as_str()))
            .filter_map(|package| {
                let manifest = metadata_relative_path(root, &package.manifest_path).ok()?;
                let package_root = manifest.strip_suffix("/Cargo.toml")?;
                path_is_within(&stable_path, package_root)
                    .then_some((package, manifest))
            })
            .collect::<Vec<_>>();
        let [(package, manifest)] = candidates.as_slice() else {
            errors.push(format!(
                "test fixture Rust source {stable_path} belongs to {} workspace package roots; expected exactly one",
                candidates.len()
            ));
            continue;
        };
        let mut external_crates = package
            .dependencies
            .iter()
            .map(|dependency| {
                dependency
                    .rename
                    .as_deref()
                    .unwrap_or(&dependency.name)
                    .replace('-', "_")
            })
            .collect::<HashSet<_>>();
        external_crates.insert(package.name.replace('-', "_"));
        let mut bound_crates = external_crates.iter().cloned().collect::<Vec<_>>();
        bound_crates.sort();
        let unit_id = short_hash(&format!(
            "{manifest}\0{}\0{}\0test-fixture\0{stable_path}\0{}",
            package.name,
            package.edition,
            bound_crates.join(",")
        ));
        collect_rust_compile_unit_assignments(
            &format!("test-fixture:{stable_path}"),
            RustSourceAssignment {
                stable_path: stable_path.clone(),
                module: vec![format!("$unit${unit_id}")],
                inherited_roles: BTreeSet::from([SemanticRole::TestFixtureGolden]),
                include_stack: vec![stable_path],
                external_crates,
            },
            &tracked,
            sources,
            &mut assignments,
            &mut ownership,
            &mut errors,
        );
    }
    for stable_path in sources.keys() {
        if !ownership.contains_key(stable_path) {
            errors.push(format!(
                "protected Rust source {stable_path} is not owned by any Cargo compile unit"
            ));
        }
    }
    if !errors.is_empty() {
        errors.sort();
        errors.dedup();
        return Err(errors.join("\n"));
    }
    eprintln!(
        "PERF build_workspace_rust_graph assignments={} owned_sources={} elapsed_ms={}",
        assignments.len(),
        ownership.len(),
        profile_started.elapsed().as_millis()
    );
    let mut type_facts = IdentityTypeFacts::default();
    for assignment in &assignments {
        let file = sources
            .get(&assignment.stable_path)
            .expect("validated Rust source assignment");
        type_facts.register_external_crates(&assignment.module, &assignment.external_crates);
        type_facts.collect_symbols(&file.items, &assignment.module)?;
    }
    eprintln!(
        "PERF build_workspace_rust_graph collected_symbols modules={} items={} pending_imports={} pending_fields={} elapsed_ms={}",
        type_facts.modules.len(),
        type_facts.module_items.len(),
        type_facts.pending_imports.len(),
        type_facts.pending_struct_fields.len(),
        profile_started.elapsed().as_millis()
    );
    type_facts.finish_symbols()?;
    eprintln!(
        "PERF build_workspace_rust_graph modules={} items={} imports={} prefixes={} elapsed_ms={}",
        type_facts.modules.len(),
        type_facts.module_items.len(),
        type_facts.imports.len(),
        type_facts.local_path_prefixes.len(),
        profile_started.elapsed().as_millis()
    );
    for assignment in &assignments {
        let file = sources
            .get(&assignment.stable_path)
            .expect("validated Rust source assignment");
        catalog.collect_items(
            &file.items,
            &assignment.module,
            &assignment.inherited_roles,
            &assignment.stable_path,
            &type_facts,
        )?;
    }
    type_facts.ensure_resolution_succeeded()?;
    let type_facts = carrier_type_facts(&catalog, type_facts);
    eprintln!(
        "PERF build_workspace_rust_graph nodes={} functions={} impl_index={} elapsed_ms={}",
        catalog.nodes.len(),
        type_facts.functions.len(),
        type_facts.impl_functions.len(),
        profile_started.elapsed().as_millis()
    );
    Ok(RustWorkspaceGraph {
        catalog,
        type_facts,
        assignments,
    })
}

#[derive(Clone)]
struct RustSourceAssignment {
    stable_path: String,
    module: Vec<String>,
    inherited_roles: BTreeSet<SemanticRole>,
    include_stack: Vec<String>,
    external_crates: HashSet<String>,
}

fn collect_rust_compile_unit_assignments(
    unit_name: &str,
    root_assignment: RustSourceAssignment,
    tracked: &HashSet<&str>,
    sources: &HashMap<String, syn::File>,
    assignments: &mut Vec<RustSourceAssignment>,
    ownership: &mut HashMap<String, Vec<String>>,
    errors: &mut Vec<String>,
) {
    let mut queue = VecDeque::from([root_assignment]);
    let mut assigned = HashMap::<String, Vec<String>>::new();
    while let Some(assignment) = queue.pop_front() {
        let RustSourceAssignment {
            stable_path,
            module,
            inherited_roles,
            include_stack,
            external_crates,
        } = assignment;
        if let Some(existing) = assigned.get(&stable_path) {
            if existing != &module {
                errors.push(format!(
                    "Cargo compile unit {unit_name} maps {stable_path} to ambiguous modules {} and {}",
                    module_label(existing),
                    module_label(&module)
                ));
            }
            continue;
        }
        assigned.insert(stable_path.clone(), module.clone());
        ownership
            .entry(stable_path.clone())
            .or_default()
            .push(module_label(&module));
        let Some(file) = sources.get(&stable_path) else {
            errors.push(format!(
                "Cargo compile unit {unit_name} module {} references missing protected source {stable_path}",
                module_label(&module)
            ));
            continue;
        };
        assignments.push(RustSourceAssignment {
            stable_path: stable_path.clone(),
            module: module.clone(),
            inherited_roles: inherited_roles.clone(),
            include_stack: include_stack.clone(),
            external_crates: external_crates.clone(),
        });
        match rust_source_bindings(
            tracked,
            &stable_path,
            &file.items,
            &module,
            &inherited_roles,
        ) {
            Ok(bindings) => {
                for binding in bindings {
                    let next_stack = if binding.is_include {
                        if include_stack.contains(&binding.stable_path) {
                            errors.push(format!(
                                "static include cycle in Cargo compile unit {unit_name}: {} -> {}",
                                include_stack.join(" -> "),
                                binding.stable_path
                            ));
                            continue;
                        }
                        let mut stack = include_stack.clone();
                        stack.push(binding.stable_path.clone());
                        stack
                    } else {
                        vec![binding.stable_path.clone()]
                    };
                    queue.push_back(RustSourceAssignment {
                        stable_path: binding.stable_path,
                        module: binding.module,
                        inherited_roles: binding.inherited_roles,
                        include_stack: next_stack,
                        external_crates: external_crates.clone(),
                    });
                }
            }
            Err(error) => errors.push(error),
        }
    }
}

struct RustSourceBinding {
    stable_path: String,
    module: Vec<String>,
    inherited_roles: BTreeSet<SemanticRole>,
    is_include: bool,
}

fn metadata_relative_path(root: &Path, path: &str) -> Result<String, String> {
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        format!(
            "failed to resolve workspace root {}: {error}",
            root.display()
        )
    })?;
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve Cargo metadata path {path}: {error}"))?;
    let relative = canonical.strip_prefix(&canonical_root).map_err(|_| {
        format!("Cargo metadata path escaped workspace: {}", canonical.display())
    })?;
    normalize_path(relative)
}

fn rust_source_bindings(
    tracked_paths: &HashSet<&str>,
    stable_path: &str,
    items: &[Item],
    module: &[String],
    inherited_roles: &BTreeSet<SemanticRole>,
) -> Result<Vec<RustSourceBinding>, String> {
    let source_dir = Path::new(stable_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let module_dir = rust_module_directory(stable_path)?;
    let mut bindings = Vec::new();
    collect_rust_source_bindings(
        tracked_paths,
        stable_path,
        items,
        &module_dir,
        source_dir,
        module,
        inherited_roles,
        &mut bindings,
    )?;
    Ok(bindings)
}

#[allow(clippy::too_many_arguments)]
fn collect_rust_source_bindings(
    tracked_paths: &HashSet<&str>,
    owner: &str,
    items: &[Item],
    module_dir: &Path,
    path_attribute_dir: &Path,
    lexical_module: &[String],
    inherited_roles: &BTreeSet<SemanticRole>,
    bindings: &mut Vec<RustSourceBinding>,
) -> Result<(), String> {
    for item in items {
        if let Item::Macro(item) = item
            && item.mac.path.is_ident("include")
        {
            let literal = syn::parse2::<LitStr>(item.mac.tokens.clone()).map_err(|_| {
                format!("unsupported dynamic include! compile input in {owner}")
            })?;
            let target = normalize_compile_input(path_attribute_dir, &literal.value())?;
            if !tracked_paths.contains(target.as_str()) {
                return Err(format!(
                    "include! compile input {target} referenced by {owner} is not tracked"
                ));
            }
            let mut roles = inherited_roles.clone();
            roles.extend(semantic_roles_for_attributes(&item.attrs));
            bindings.push(RustSourceBinding {
                stable_path: target,
                module: lexical_module.to_vec(),
                inherited_roles: roles,
                is_include: true,
            });
            continue;
        }
        let Item::Mod(module) = item else { continue };
        let path_attribute = module
            .attrs
            .iter()
            .find(|attribute| attribute.path().is_ident("path"));
        let mut nested_module = lexical_module.to_vec();
        nested_module.push(module.ident.to_string());
        let mut nested_roles = inherited_roles.clone();
        nested_roles.extend(semantic_roles_for_attributes(&module.attrs));
        if let Some((_, nested)) = &module.content {
            if path_attribute.is_some() {
                return Err(format!(
                    "unsupported #[path] on inline module {} in {owner}",
                    module.ident
                ));
            }
            collect_rust_source_bindings(
                tracked_paths,
                owner,
                nested,
                &module_dir.join(module.ident.to_string()),
                &path_attribute_dir.join(module.ident.to_string()),
                &nested_module,
                &nested_roles,
                bindings,
            )?;
            continue;
        }
        let target = resolve_external_module_input(
            tracked_paths,
            owner,
            module,
            module_dir,
            path_attribute_dir,
        )?;
        bindings.push(RustSourceBinding {
            stable_path: target,
            module: nested_module,
            inherited_roles: nested_roles,
            is_include: false,
        });
    }
    Ok(())
}

pub fn workspace_identity_allowance_candidates(
    root: &Path,
) -> Result<Vec<IdentityAllowanceCandidate>, String> {
    let external_paths = validated_external_compat_paths(root)?
        .into_iter()
        .collect::<HashSet<_>>();
    let files = protected_files(root)?;
    let rust_identity_contexts = workspace_rust_identity_contexts(root)?;

    let mut candidates = Vec::new();
    for file in files {
        let relative = normalize_path(
            file.strip_prefix(root)
                .map_err(|_| format!("{} escaped workspace root", file.display()))?,
        )?;
        if relative == GENERIC_DOMAIN_REGISTRY_PATH
            || relative == EXTERNAL_COMPAT_MANIFEST_PATH
            || external_paths.contains(relative.as_str())
        {
            continue;
        }
        let identity_source = read_bounded_regular_text(&file)?;
        let is_rust = file.extension().is_some_and(|extension| extension == "rs");
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
            let branch_violations = if is_rust {
                inspect_workspace_rust_identity_fragment(
                    &label,
                    &relative,
                    &fragment,
                    &rust_identity_contexts,
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
            || surface.semantic_role != snapshot.semantic_role
            || surface.stable_path != snapshot.stable_path
            || surface.selector != snapshot.selector
        {
            errors.push(format!(
                "surface {} identity drifted: registered {} {} {} {}, actual {} {} {} {}",
                snapshot.surface_id,
                surface.kind,
                surface.semantic_role.as_str(),
                surface.stable_path,
                surface.selector,
                snapshot.kind,
                snapshot.semantic_role.as_str(),
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
    let rust_identity_contexts = workspace_rust_identity_contexts(root)?;

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
        let identity_source = read_bounded_regular_text(&file)?;
        let is_rust = file.extension().is_some_and(|extension| extension == "rs");
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
            if is_rust && !branch_allowed
            {
                errors.extend(inspect_workspace_rust_identity_fragment(
                    &label,
                    &relative,
                    &fragment,
                    &rust_identity_contexts,
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
    contexts: &RustIdentityBranchContexts,
) -> Result<Vec<String>, String> {
    let context_key = fragment.rust_context_key.as_ref().ok_or_else(|| {
        format!(
            "Rust identity fragment {} has no context",
            fragment.selector
        )
    })?;
    let owner = contexts.owners.get(context_key).ok_or_else(|| {
        format!(
            "Rust identity fragment {} references missing context {context_key}",
            fragment.selector
        )
    })?;
    inspect_identity_axis_fragment_with_context(path, &fragment.content, &contexts.file, owner)
}

fn inspect_workspace_rust_identity_fragment(
    path: &str,
    stable_path: &str,
    fragment: &IdentityFragment,
    contexts: &RustWorkspaceIdentityContexts,
) -> Result<Vec<String>, String> {
    let context_key = fragment.rust_context_key.as_ref().ok_or_else(|| {
        format!("Rust identity fragment {} has no context", fragment.selector)
    })?;
    let owners = contexts
        .owners
        .get(&(stable_path.to_string(), context_key.clone()))
        .ok_or_else(|| {
            format!(
                "Rust identity fragment {} in {stable_path} references missing compile-unit context {context_key}",
                fragment.selector
            )
        })?;
    let fragment_file = syn::parse_file(&fragment.content)
        .map_err(|error| format!("failed to parse {path}: {error}"))?;
    let mut violations = Vec::new();
    for owner in owners {
        violations.extend(inspect_identity_axis_branches_with_analysis(
            path,
            &fragment_file,
            owner,
            &contexts.inferred_parameters,
            &contexts.inferred_returns,
            &contexts.type_facts,
        )?);
    }
    violations.sort();
    violations.dedup();
    Ok(violations)
}

fn inspect_identity_axis_fragment_with_context(
    path: &str,
    fragment_source: &str,
    file: &syn::File,
    owner: &IdentityOwner,
) -> Result<Vec<String>, String> {
    let fragment = syn::parse_file(fragment_source)
        .map_err(|error| format!("failed to parse {path}: {error}"))?;
    inspect_identity_axis_branches_with_context(path, &fragment, file, owner)
}

fn inspect_identity_axis_branches_with_context(
    path: &str,
    inspected: &syn::File,
    context: &syn::File,
    owner: &IdentityOwner,
) -> Result<Vec<String>, String> {
    let inferred_parameters = infer_identity_parameter_axes(context)?;
    let inferred_returns = infer_function_return_strings(context)?;
    let type_facts = identity_type_facts(context)?;
    inspect_identity_axis_branches_with_analysis(
        path,
        inspected,
        owner,
        &inferred_parameters,
        &inferred_returns,
        &type_facts,
    )
}

fn inspect_identity_axis_branches_with_analysis(
    path: &str,
    inspected: &syn::File,
    owner: &IdentityOwner,
    inferred_parameters: &HashMap<IdentityFunctionKey, Vec<Option<&'static str>>>,
    inferred_returns: &HashMap<IdentityFunctionKey, Vec<String>>,
    type_facts: &IdentityTypeFacts,
) -> Result<Vec<String>, String> {
    let mut visitor = IdentityBranchVisitor {
        path,
        violations: Vec::new(),
        errors: Vec::new(),
        aliases: vec![HashMap::new()],
        non_identity: vec![HashSet::new()],
        types: vec![HashMap::new()],
        collections: vec![HashMap::new()],
        return_axis: None,
        owner: owner.clone(),
        inferred_parameters: inferred_parameters.clone(),
        inferred_returns: inferred_returns.clone(),
        type_facts: type_facts.clone(),
    };
    visitor.visit_file(inspected);
    visitor
        .errors
        .extend(visitor.type_facts.resolution_error_messages());
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
    name: String,
    edition: String,
    manifest_path: String,
    targets: Vec<CargoMetadataTarget>,
    #[serde(default)]
    dependencies: Vec<CargoMetadataDependency>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDependency {
    name: String,
    rename: Option<String>,
}

fn load_cargo_metadata(root: &Path) -> Result<CargoMetadata, String> {
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
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid Cargo metadata output: {error}"))
}

fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let metadata = load_cargo_metadata(root)?;
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

        let target = resolve_external_module_input(
            tracked_paths,
            owner,
            module,
            module_dir,
            path_attribute_dir,
        )?;
        inputs.push(target);
    }
    Ok(())
}

fn resolve_external_module_input(
    tracked_paths: &HashSet<&str>,
    owner: &str,
    module: &syn::ItemMod,
    module_dir: &Path,
    path_attribute_dir: &Path,
) -> Result<String, String> {
    let path_attribute = module
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("path"));
    if let Some(attribute) = path_attribute {
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
        return normalize_compile_input(path_attribute_dir, &literal);
    }

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
        [target] => Ok(target.clone()),
        [] => Err(format!(
            "module {} in {owner} has no compile input file",
            module.ident
        )),
        _ => Err(format!(
            "module {} in {owner} has ambiguous compile input files",
            module.ident
        )),
    }
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum IdentityOwner {
    Module(Vec<String>),
    Impl {
        module: Vec<String>,
        self_ty: String,
        trait_name: Option<String>,
    },
}

impl IdentityOwner {
    fn module(&self) -> &[String] {
        match self {
            Self::Module(module) | Self::Impl { module, .. } => module,
        }
    }
}

fn identity_impl_owner(item: &syn::ItemImpl, module: &[String]) -> IdentityOwner {
    IdentityOwner::Impl {
        module: module.to_vec(),
        self_ty: item.self_ty.to_token_stream().to_string(),
        trait_name: item
            .trait_
            .as_ref()
            .map(|(_, path, _)| path.to_token_stream().to_string()),
    }
}

fn identity_impl_owner_with_facts(
    item: &syn::ItemImpl,
    module: &[String],
    facts: &IdentityTypeFacts,
) -> IdentityOwner {
    let lexical_owner = IdentityOwner::Module(module.to_vec());
    IdentityOwner::Impl {
        module: module.to_vec(),
        self_ty: tracked_identity_type_with_facts(&item.self_ty, &lexical_owner, facts)
            .unwrap_or_else(|| item.self_ty.to_token_stream().to_string()),
        trait_name: item
            .trait_
            .as_ref()
            .map(|(_, path, _)| {
                resolve_type_path_with_facts(path, &lexical_owner, facts)
                    .unwrap_or_else(|| path.to_token_stream().to_string())
            }),
    }
}

fn identity_trait_owner(item: &syn::ItemTrait, module: &[String]) -> IdentityOwner {
    let trait_name = canonical_symbol_label(module, &item.ident.to_string());
    IdentityOwner::Impl {
        module: module.to_vec(),
        self_ty: trait_name.clone(),
        trait_name: Some(trait_name),
    }
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

struct RustIdentityBranchContexts {
    file: syn::File,
    owners: HashMap<String, IdentityOwner>,
}

struct RustWorkspaceIdentityContexts {
    owners: HashMap<(String, String), Vec<IdentityOwner>>,
    inferred_parameters: HashMap<IdentityFunctionKey, Vec<Option<&'static str>>>,
    inferred_returns: HashMap<IdentityFunctionKey, Vec<String>>,
    type_facts: IdentityTypeFacts,
}

fn workspace_rust_identity_contexts(
    root: &Path,
) -> Result<RustWorkspaceIdentityContexts, String> {
    let mut sources = HashMap::<String, syn::File>::new();
    for file in protected_files(root)? {
        if !file.extension().is_some_and(|extension| extension == "rs") {
            continue;
        }
        let stable_path = normalize_path(
            file.strip_prefix(root)
                .map_err(|_| format!("{} escaped workspace root", file.display()))?,
        )?;
        let source = read_bounded_regular_text(&file)?;
        let parsed = syn::parse_file(&source)
            .map_err(|error| format!("failed to parse {stable_path}: {error}"))?;
        sources.insert(stable_path, parsed);
    }
    let graph = build_workspace_rust_graph(root, &sources)?;
    let mut functions = Vec::new();
    let mut owners = HashMap::<(String, String), Vec<IdentityOwner>>::new();
    for assignment in &graph.assignments {
        let file = sources
            .get(&assignment.stable_path)
            .expect("validated workspace Rust assignment");
        collect_identity_functions(
            &file.items,
            &assignment.module,
            &graph.type_facts,
            &mut functions,
        );
        collect_workspace_identity_owners(
            &file.items,
            &assignment.stable_path,
            &[],
            &assignment.module,
            &graph.type_facts,
            &mut owners,
        );
    }
    for owner_set in owners.values_mut() {
        owner_set.sort();
        owner_set.dedup();
    }
    let inferred_parameters =
        infer_identity_parameter_axes_for_functions(&functions, &graph.type_facts)?;
    let inferred_returns =
        infer_function_return_strings_for_functions(&functions, &graph.type_facts)?;
    graph.type_facts.ensure_resolution_succeeded()?;
    Ok(RustWorkspaceIdentityContexts {
        owners,
        inferred_parameters,
        inferred_returns,
        type_facts: graph.type_facts,
    })
}

fn collect_workspace_identity_owners(
    items: &[Item],
    stable_path: &str,
    relative_module: &[String],
    absolute_module: &[String],
    facts: &IdentityTypeFacts,
    owners: &mut HashMap<(String, String), Vec<IdentityOwner>>,
) {
    owners
        .entry((
            stable_path.to_string(),
            rust_module_context_key(relative_module),
        ))
        .or_default()
        .push(IdentityOwner::Module(absolute_module.to_vec()));
    for item in items {
        match item {
            Item::Impl(item) => {
                let relative_owner = rust_impl_owner(item, relative_module);
                owners
                    .entry((
                        stable_path.to_string(),
                        rust_impl_context_key(&relative_owner),
                    ))
                    .or_default()
                    .push(identity_impl_owner_with_facts(item, absolute_module, facts));
            }
            Item::Mod(item) => {
                if let Some((_, nested)) = &item.content {
                    let mut relative = relative_module.to_vec();
                    relative.push(item.ident.to_string());
                    let mut absolute = absolute_module.to_vec();
                    absolute.push(item.ident.to_string());
                    collect_workspace_identity_owners(
                        nested,
                        stable_path,
                        &relative,
                        &absolute,
                        facts,
                        owners,
                    );
                }
            }
            _ => {}
        }
    }
}

fn rust_identity_branch_contexts(
    path: &str,
    source: &str,
) -> Result<RustIdentityBranchContexts, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse {path}: {error}"))?;
    let mut owners = HashMap::new();
    collect_rust_identity_branch_contexts(&file.items, &[], &mut owners);
    Ok(RustIdentityBranchContexts { file, owners })
}

fn collect_rust_identity_branch_contexts(
    items: &[Item],
    module: &[String],
    contexts: &mut HashMap<String, IdentityOwner>,
) {
    contexts
        .entry(rust_module_context_key(module))
        .or_insert_with(|| IdentityOwner::Module(module.to_vec()));

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
                    .or_insert_with(|| identity_impl_owner(item, module));
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
        "json" => structured_json_schema_inventory(stable_path, source)?,
        _ => Vec::new(),
    };
    snapshots_from_raw(stable_path, raw)
}

fn snapshots_from_raw(
    stable_path: &str,
    raw: Vec<RawSurface>,
) -> Result<Vec<SurfaceSnapshot>, String> {
    raw.into_iter()
        .map(|item| {
            let semantic_role = semantic_role_for_kind(item.kind).ok_or_else(|| {
                format!(
                    "protected surface extractor emitted unknown kind {} for {stable_path}",
                    item.kind
                )
            })?;
            let surface_id = surface_id_for(item.kind, stable_path, &item.selector);
            let fingerprint = format!("{:x}", Sha256::digest(item.content.as_bytes()));
            Ok(SurfaceSnapshot {
                surface_id,
                kind: item.kind.to_string(),
                semantic_role,
                stable_path: stable_path.to_string(),
                selector: item.selector,
                fingerprint,
            })
        })
        .collect()
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
    let mut items = rust_base_surface_inventory(&file)?;
    items.extend(rust_carrier_inventory(path, &file)?);
    Ok(normalize_raw_surfaces(items))
}

fn rust_base_surface_inventory(file: &syn::File) -> Result<Vec<RawSurface>, String> {
    let mut collector = RustSurfaceCollector::default();
    collector.collect_items(&file.items, &[])?;
    Ok(normalize_raw_surfaces(collector.items))
}

fn normalize_raw_surfaces(mut items: Vec<RawSurface>) -> Vec<RawSurface> {
    items.sort_by(|left, right| {
        (left.kind, left.selector.as_str(), left.content.as_str()).cmp(&(
            right.kind,
            right.selector.as_str(),
            right.content.as_str(),
        ))
    });
    items.dedup();
    let counts = items.iter().fold(
        HashMap::<(&'static str, String), usize>::new(),
        |mut counts, item| {
            *counts
                .entry((item.kind, item.selector.clone()))
                .or_default() += 1;
            counts
        },
    );
    let mut ordinals = HashMap::<(&'static str, String, String), usize>::new();
    for item in &mut items {
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
    items
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
            let kinds = match name.as_str() {
                "serde" | "value" => vec!["rust_wire_attribute"],
                "arg" | "clap" | "command" => vec!["rust_cli_attribute"],
                "derive" => {
                    let mut kinds = Vec::new();
                    if attribute_tokens.contains("Serialize")
                        || attribute_tokens.contains("Deserialize")
                    {
                        kinds.push("rust_wire_attribute");
                    }
                    if attribute_tokens.contains("Default") {
                        kinds.push("rust_default_attribute");
                    }
                    if ["Parser", "Args", "Subcommand", "ValueEnum", "CommandFactory"]
                        .iter()
                        .any(|derive| attribute_tokens.contains(derive))
                    {
                        kinds.push("rust_cli_attribute");
                    }
                    kinds
                }
                "export_name" | "link" | "link_name" | "no_mangle" | "repr" => {
                    vec!["rust_ffi_attribute"]
                }
                "unsafe"
                    if attribute_tokens.contains("no_mangle")
                        || attribute_tokens.contains("export_name") =>
                {
                    vec!["rust_ffi_attribute"]
                }
                _ if is_inert_rust_attribute(&name) => Vec::new(),
                _ => vec!["rust_contract_attribute"],
            };
            let index = ordinal.entry(name.clone()).or_default();
            for kind in kinds {
                self.push(
                    kind,
                    format!("attribute:{owner}:{name}:{}:{kind}", *index),
                    attribute.to_token_stream(),
                );
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum RustCarrierItemKind {
    Function,
    Const,
    Static,
    Macro,
}

impl RustCarrierItemKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Const => "const",
            Self::Static => "static",
            Self::Macro => "macro",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RustCarrierKey {
    owner: IdentityOwner,
    kind: RustCarrierItemKind,
    name: String,
}

impl RustCarrierKey {
    fn function(key: IdentityFunctionKey) -> Self {
        Self {
            owner: key.owner,
            kind: RustCarrierItemKind::Function,
            name: key.name,
        }
    }

    fn selector(&self) -> String {
        format!(
            "carrier:{}:{}:{}",
            identity_owner_label(&self.owner),
            self.kind.as_str(),
            self.name
        )
    }
}

#[derive(Clone)]
enum RustCarrierBody {
    Block(syn::Block),
    Expression(Expr),
    None,
}

struct RustCarrierNode {
    key: RustCarrierKey,
    stable_path: String,
    content: String,
    body: RustCarrierBody,
    parameter_types: HashMap<String, TrackedIdentityType>,
    return_type: Option<TrackedIdentityType>,
    root_roles: BTreeSet<SemanticRole>,
}

struct PendingCarrierRoot {
    owner: IdentityOwner,
    path: syn::Path,
    roles: BTreeSet<SemanticRole>,
    source: String,
}

#[derive(Default)]
struct RustCarrierCatalog {
    nodes: HashMap<RustCarrierKey, RustCarrierNode>,
    pending_roots: Vec<PendingCarrierRoot>,
    errors: Vec<String>,
}

impl RustCarrierCatalog {
    fn collect_items(
        &mut self,
        items: &[Item],
        module: &[String],
        inherited_roles: &BTreeSet<SemanticRole>,
        stable_path: &str,
        facts: &IdentityTypeFacts,
    ) -> Result<(), String> {
        for item in items {
            let mut roles = inherited_roles.clone();
            roles.extend(semantic_roles_for_attributes(item_attrs(item)));
            match item {
                Item::Const(item) => {
                    if is_public(&item.vis) {
                        roles.insert(SemanticRole::Contract);
                    }
                    self.insert_node(RustCarrierNode {
                        key: RustCarrierKey {
                            owner: IdentityOwner::Module(module.to_vec()),
                            kind: RustCarrierItemKind::Const,
                            name: item.ident.to_string(),
                        },
                        stable_path: stable_path.to_string(),
                        content: item.to_token_stream().to_string(),
                        body: RustCarrierBody::Expression((*item.expr).clone()),
                        parameter_types: HashMap::new(),
                        return_type: tracked_identity_type_with_facts(
                            &item.ty,
                            &IdentityOwner::Module(module.to_vec()),
                            facts,
                        ),
                        root_roles: roles,
                    });
                    self.collect_attribute_roots(
                        &item.attrs,
                        IdentityOwner::Module(module.to_vec()),
                        &format!("const {}", item.ident),
                    )?;
                }
                Item::Fn(item) => {
                    if is_public(&item.vis) {
                        roles.insert(SemanticRole::Contract);
                    }
                    if item.sig.ident == "main" {
                        roles.insert(SemanticRole::Cli);
                    }
                    self.insert_node(RustCarrierNode {
                        key: RustCarrierKey {
                            owner: IdentityOwner::Module(module.to_vec()),
                            kind: RustCarrierItemKind::Function,
                            name: item.sig.ident.to_string(),
                        },
                        stable_path: stable_path.to_string(),
                        content: item.to_token_stream().to_string(),
                        body: RustCarrierBody::Block((*item.block).clone()),
                        parameter_types: tracked_parameter_types_with_facts(
                            &item.sig.inputs,
                            &IdentityOwner::Module(module.to_vec()),
                            facts,
                        ),
                        return_type: tracked_return_type_with_facts(
                            &item.sig.output,
                            &IdentityOwner::Module(module.to_vec()),
                            facts,
                        ),
                        root_roles: roles,
                    });
                    self.collect_attribute_roots(
                        &item.attrs,
                        IdentityOwner::Module(module.to_vec()),
                        &format!("fn {}", item.sig.ident),
                    )?;
                }
                Item::Static(item) => {
                    if is_public(&item.vis) {
                        roles.insert(SemanticRole::Contract);
                    }
                    self.insert_node(RustCarrierNode {
                        key: RustCarrierKey {
                            owner: IdentityOwner::Module(module.to_vec()),
                            kind: RustCarrierItemKind::Static,
                            name: item.ident.to_string(),
                        },
                        stable_path: stable_path.to_string(),
                        content: item.to_token_stream().to_string(),
                        body: RustCarrierBody::Expression((*item.expr).clone()),
                        parameter_types: HashMap::new(),
                        return_type: tracked_identity_type_with_facts(
                            &item.ty,
                            &IdentityOwner::Module(module.to_vec()),
                            facts,
                        ),
                        root_roles: roles,
                    });
                    self.collect_attribute_roots(
                        &item.attrs,
                        IdentityOwner::Module(module.to_vec()),
                        &format!("static {}", item.ident),
                    )?;
                }
                Item::Macro(item) => {
                    if let Some(identifier) = &item.ident {
                        self.insert_node(RustCarrierNode {
                            key: RustCarrierKey {
                                owner: IdentityOwner::Module(module.to_vec()),
                                kind: RustCarrierItemKind::Macro,
                                name: identifier.to_string(),
                            },
                            stable_path: stable_path.to_string(),
                            content: item.to_token_stream().to_string(),
                            body: RustCarrierBody::None,
                            parameter_types: HashMap::new(),
                            return_type: None,
                            root_roles: roles,
                        });
                    }
                }
                Item::Mod(item) => {
                    self.collect_attribute_roots(
                        &item.attrs,
                        IdentityOwner::Module(module.to_vec()),
                        &format!("mod {}", item.ident),
                    )?;
                    if let Some((_, nested)) = &item.content {
                        let mut nested_roles = inherited_roles.clone();
                        nested_roles.extend(semantic_roles_for_attributes(&item.attrs));
                        let mut next = module.to_vec();
                        next.push(item.ident.to_string());
                        self.collect_items(nested, &next, &nested_roles, stable_path, facts)?;
                    }
                }
                Item::Impl(item) => {
                    let owner = identity_impl_owner_with_facts(item, module, facts);
                    let trait_name = item
                        .trait_
                        .as_ref()
                        .and_then(|(_, path, _)| path.segments.last())
                        .map(|segment| segment.ident.to_string());
                    let mut impl_roles = roles;
                    match trait_name.as_deref() {
                        Some("Default") => {
                            impl_roles.insert(SemanticRole::Default);
                        }
                        Some("Serialize" | "Deserialize") => {
                            impl_roles.insert(SemanticRole::Wire);
                        }
                        Some(_) => {
                            impl_roles.insert(SemanticRole::Contract);
                        }
                        None => {}
                    }
                    self.collect_attribute_roots(
                        &item.attrs,
                        owner.clone(),
                        &format!("impl {}", identity_owner_label(&owner)),
                    )?;
                    for member in &item.items {
                        match member {
                            ImplItem::Const(member) => {
                                let mut member_roles = impl_roles.clone();
                                member_roles.extend(semantic_roles_for_attributes(&member.attrs));
                                if is_public(&member.vis) {
                                    member_roles.insert(SemanticRole::Contract);
                                }
                                self.insert_node(RustCarrierNode {
                                    key: RustCarrierKey {
                                        owner: owner.clone(),
                                        kind: RustCarrierItemKind::Const,
                                        name: member.ident.to_string(),
                                    },
                                    stable_path: stable_path.to_string(),
                                    content: member.to_token_stream().to_string(),
                                    body: RustCarrierBody::Expression(member.expr.clone()),
                                    parameter_types: HashMap::new(),
                                    return_type: tracked_identity_type_with_facts(
                                        &member.ty,
                                        &owner,
                                        facts,
                                    ),
                                    root_roles: member_roles,
                                });
                                self.collect_attribute_roots(
                                    &member.attrs,
                                    owner.clone(),
                                    &format!("impl const {}", member.ident),
                                )?;
                            }
                            ImplItem::Fn(member) => {
                                let mut member_roles = impl_roles.clone();
                                member_roles.extend(semantic_roles_for_attributes(&member.attrs));
                                if is_public(&member.vis) {
                                    member_roles.insert(SemanticRole::Contract);
                                }
                                self.insert_node(RustCarrierNode {
                                    key: RustCarrierKey {
                                        owner: owner.clone(),
                                        kind: RustCarrierItemKind::Function,
                                        name: member.sig.ident.to_string(),
                                    },
                                    stable_path: stable_path.to_string(),
                                    content: member.to_token_stream().to_string(),
                                    body: RustCarrierBody::Block(member.block.clone()),
                                    parameter_types: tracked_parameter_types_with_facts(
                                        &member.sig.inputs,
                                        &owner,
                                        facts,
                                    ),
                                    return_type: tracked_return_type_with_facts(
                                        &member.sig.output,
                                        &owner,
                                        facts,
                                    ),
                                    root_roles: member_roles,
                                });
                                self.collect_attribute_roots(
                                    &member.attrs,
                                    owner.clone(),
                                    &format!("impl fn {}", member.sig.ident),
                                )?;
                            }
                            _ => {}
                        }
                    }
                }
                Item::Enum(item) => {
                    self.collect_attribute_roots(
                        &item.attrs,
                        IdentityOwner::Module(module.to_vec()),
                        &format!("enum {}", item.ident),
                    )?;
                    for variant in &item.variants {
                        self.collect_attribute_roots(
                            &variant.attrs,
                            IdentityOwner::Module(module.to_vec()),
                            &format!("variant {}::{}", item.ident, variant.ident),
                        )?;
                        for field in &variant.fields {
                            self.collect_attribute_roots(
                                &field.attrs,
                                IdentityOwner::Module(module.to_vec()),
                                &format!("variant field {}::{}", item.ident, variant.ident),
                            )?;
                        }
                    }
                }
                Item::Struct(item) => {
                    self.collect_attribute_roots(
                        &item.attrs,
                        IdentityOwner::Module(module.to_vec()),
                        &format!("struct {}", item.ident),
                    )?;
                    for field in &item.fields {
                        self.collect_attribute_roots(
                            &field.attrs,
                            IdentityOwner::Module(module.to_vec()),
                            &format!("field {}", item.ident),
                        )?;
                    }
                }
                Item::Trait(item) => {
                    let owner = identity_trait_owner(item, module);
                    let mut trait_roles = roles;
                    if is_public(&item.vis) {
                        trait_roles.insert(SemanticRole::Contract);
                    }
                    self.collect_attribute_roots(
                        &item.attrs,
                        owner.clone(),
                        &format!("trait {}", item.ident),
                    )?;
                    for member in &item.items {
                        let syn::TraitItem::Fn(member) = member else {
                            continue;
                        };
                        self.collect_attribute_roots(
                            &member.attrs,
                            owner.clone(),
                            &format!("trait fn {}", member.sig.ident),
                        )?;
                        let Some(block) = &member.default else {
                            continue;
                        };
                        let mut member_roles = trait_roles.clone();
                        member_roles.extend(semantic_roles_for_attributes(&member.attrs));
                        self.insert_node(RustCarrierNode {
                            key: RustCarrierKey {
                                owner: owner.clone(),
                                kind: RustCarrierItemKind::Function,
                                name: member.sig.ident.to_string(),
                            },
                            stable_path: stable_path.to_string(),
                            content: member.to_token_stream().to_string(),
                            body: RustCarrierBody::Block(block.clone()),
                            parameter_types: tracked_parameter_types_with_facts(
                                &member.sig.inputs,
                                &owner,
                                facts,
                            ),
                            return_type: tracked_return_type_with_facts(
                                &member.sig.output,
                                &owner,
                                facts,
                            ),
                            root_roles: member_roles,
                        });
                    }
                }
                _ => {
                    self.collect_attribute_roots(
                        item_attrs(item),
                        IdentityOwner::Module(module.to_vec()),
                        &identity_item_selector(item, module),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn insert_node(&mut self, node: RustCarrierNode) {
        if self.nodes.insert(node.key.clone(), node).is_some() {
            self.errors
                .push("duplicate fully-qualified Rust carrier item".to_string());
        }
    }

    fn collect_attribute_roots(
        &mut self,
        attributes: &[Attribute],
        owner: IdentityOwner,
        source: &str,
    ) -> Result<(), String> {
        for attribute in attributes {
            let name = attribute
                .path()
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
                .unwrap_or_default();
            let roles = match name.as_str() {
                "serde" => BTreeSet::from([SemanticRole::Wire, SemanticRole::Default]),
                "arg" | "clap" | "command" => BTreeSet::from([SemanticRole::Cli]),
                _ => continue,
            };
            for path in attribute_reference_paths(attribute)? {
                self.pending_roots.push(PendingCarrierRoot {
                    owner: owner.clone(),
                    path,
                    roles: roles.clone(),
                    source: source.to_string(),
                });
            }
        }
        Ok(())
    }
}

fn rust_carrier_inventory(path: &str, file: &syn::File) -> Result<Vec<RawSurface>, String> {
    let mut inherited_roles = BTreeSet::new();
    if is_test_source_path(path) {
        inherited_roles.insert(SemanticRole::TestFixtureGolden);
    }
    let mut type_facts = IdentityTypeFacts::default();
    type_facts.collect_symbols(&file.items, &[])?;
    type_facts.finish_symbols()?;
    let mut catalog = RustCarrierCatalog::default();
    catalog.collect_items(&file.items, &[], &inherited_roles, path, &type_facts)?;
    type_facts.ensure_resolution_succeeded()?;
    let type_facts = carrier_type_facts(&catalog, type_facts);
    finish_rust_carrier_catalog(catalog, &type_facts).map(|surfaces| {
        surfaces
            .into_iter()
            .map(|(_, surface)| surface)
            .collect()
    })
}

fn finish_rust_carrier_catalog(
    mut catalog: RustCarrierCatalog,
    type_facts: &IdentityTypeFacts,
) -> Result<Vec<(String, RawSurface)>, String> {
    let profile_started = Instant::now();
    for pending in std::mem::take(&mut catalog.pending_roots) {
        match resolve_call_path(&pending.path, &pending.owner, type_facts) {
            RustCallResolution::Local(function) => {
                let key = RustCarrierKey::function(function);
                let Some(node) = catalog.nodes.get_mut(&key) else {
                    catalog.errors.push(format!(
                        "attribute {} resolved missing local carrier {}",
                        pending.source,
                        key.selector()
                    ));
                    continue;
                };
                node.root_roles.extend(pending.roles);
            }
            RustCallResolution::LocalConstructor(kind) => {
                catalog.errors.push(format!(
                    "attribute {} resolved constructor {kind} instead of a carrier function for {}",
                    pending.source,
                    pending.path.to_token_stream()
                ));
            }
            RustCallResolution::Ambiguous(error)
            | RustCallResolution::UnsupportedLocal(error) => {
                catalog.errors.push(format!(
                    "attribute {} has unresolved local carrier reference {}: {error}",
                    pending.source,
                    pending.path.to_token_stream()
                ));
            }
            RustCallResolution::ProvenExternal
            | RustCallResolution::GeneratedByTrackedDerive => {}
        }
    }

    let mut queue = VecDeque::new();
    for node in catalog.nodes.values() {
        for role in &node.root_roles {
            queue.push_back((node.key.clone(), *role));
        }
    }
    eprintln!(
        "PERF finish_rust_carrier_catalog roots={} nodes={} elapsed_ms={}",
        queue.len(),
        catalog.nodes.len(),
        profile_started.elapsed().as_millis()
    );
    let mut reached = BTreeSet::<(RustCarrierKey, SemanticRole)>::new();
    let mut edge_cache = HashMap::<RustCarrierKey, Result<Vec<RustCarrierKey>, String>>::new();
    while let Some((key, role)) = queue.pop_front() {
        if !reached.insert((key.clone(), role)) {
            continue;
        }
        if reached.len() % 1_000 == 0 {
            eprintln!(
                "PERF finish_rust_carrier_catalog reached={} queued={} edge_cache={} elapsed_ms={}",
                reached.len(),
                queue.len(),
                edge_cache.len(),
                profile_started.elapsed().as_millis()
            );
        }
        let edges = edge_cache.entry(key.clone()).or_insert_with(|| {
            let node = catalog
                .nodes
                .get(&key)
                .ok_or_else(|| format!("missing carrier node {}", key.selector()))?;
            rust_carrier_edges(node, &catalog.nodes, &type_facts)
        });
        match edges {
            Ok(edges) => {
                for edge in edges {
                    queue.push_back((edge.clone(), role));
                }
            }
            Err(error) => catalog.errors.push(error.clone()),
        }
    }
    catalog
        .errors
        .extend(type_facts.resolution_error_messages());
    if !catalog.errors.is_empty() {
        catalog.errors.sort();
        catalog.errors.dedup();
        return Err(catalog.errors.join("\n"));
    }

    let mut surfaces = reached
        .into_iter()
        .filter_map(|(key, role)| {
            let node = catalog.nodes.get(&key)?;
            Some(RawSurface {
                kind: carrier_kind(role),
                selector: key.selector(),
                content: node.content.clone(),
            })
            .map(|surface| (node.stable_path.clone(), surface))
        })
        .collect::<Vec<_>>();
    surfaces.sort_by(|left, right| {
        (&left.0, left.1.kind, &left.1.selector).cmp(&(&right.0, right.1.kind, &right.1.selector))
    });
    surfaces.dedup();
    Ok(surfaces)
}

fn carrier_type_facts(
    catalog: &RustCarrierCatalog,
    mut facts: IdentityTypeFacts,
) -> IdentityTypeFacts {
    let functions = catalog
        .nodes
        .keys()
        .filter(|key| key.kind == RustCarrierItemKind::Function)
        .map(|key| IdentityFunctionKey {
            owner: key.owner.clone(),
            name: key.name.clone(),
        })
        .collect::<HashSet<_>>();
    let function_returns = catalog
        .nodes
        .values()
        .filter(|node| node.key.kind == RustCarrierItemKind::Function)
        .filter_map(|node| {
            node.return_type.clone().map(|return_type| {
                (
                    IdentityFunctionKey {
                        owner: node.key.owner.clone(),
                        name: node.key.name.clone(),
                    },
                    return_type,
                )
            })
        })
        .collect();
    facts.set_functions(functions);
    for key in catalog.nodes.keys() {
        if let IdentityOwner::Impl { self_ty, .. } = &key.owner {
            facts
                .impl_owners
                .entry(self_ty.clone())
                .or_default()
                .push(key.owner.clone());
        }
    }
    for owners in facts.impl_owners.values_mut() {
        owners.sort();
        owners.dedup();
    }
    facts.function_returns = function_returns;
    facts
}

fn carrier_kind(role: SemanticRole) -> &'static str {
    match role {
        SemanticRole::Contract => "rust_contract_carrier",
        SemanticRole::Wire => "rust_wire_carrier",
        SemanticRole::Schema => "schema_carrier",
        SemanticRole::Cli => "rust_cli_carrier",
        SemanticRole::Default => "rust_default_carrier",
        SemanticRole::Template => "rust_template_carrier",
        SemanticRole::TaskDefinition => "rust_task_definition_carrier",
        SemanticRole::IdentityBranch => "rust_identity_branch_carrier",
        SemanticRole::TestFixtureGolden => "rust_test_fixture_carrier",
    }
}

fn identity_owner_label(owner: &IdentityOwner) -> String {
    match owner {
        IdentityOwner::Module(module) if module.is_empty() => "crate".to_string(),
        IdentityOwner::Module(module) => module_label(module),
        IdentityOwner::Impl {
            module,
            self_ty,
            trait_name,
        } => format!(
            "{}::impl:{}:{}",
            module_label(module),
            self_ty,
            trait_name.as_deref().unwrap_or("inherent")
        ),
    }
}

fn module_label(module: &[String]) -> String {
    let root = compile_unit_root(module);
    let visible = &module[root.len()..];
    let prefix = root
        .first()
        .and_then(|value| value.strip_prefix("$unit$"))
        .map_or_else(|| "crate".to_string(), |id| format!("crate@{id}"));
    if visible.is_empty() {
        prefix
    } else {
        format!("{prefix}::{}", visible.join("::"))
    }
}

fn compile_unit_root(module: &[String]) -> &[String] {
    if module
        .first()
        .is_some_and(|segment| segment.starts_with("$unit$"))
    {
        &module[..1]
    } else {
        &[]
    }
}

fn is_test_source_path(path: &str) -> bool {
    path.starts_with("tests/") || path.contains("/tests/") || path.ends_with("/tests.rs")
}

fn semantic_roles_for_attributes(attributes: &[Attribute]) -> BTreeSet<SemanticRole> {
    let mut roles = BTreeSet::new();
    for attribute in attributes {
        let name = attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        let tokens = attribute.to_token_stream().to_string();
        match name.as_str() {
            "test" => {
                roles.insert(SemanticRole::TestFixtureGolden);
            }
            "cfg" | "cfg_attr" if meta_mentions_test(&attribute.meta) => {
                roles.insert(SemanticRole::TestFixtureGolden);
            }
            "serde" | "value" => {
                roles.insert(SemanticRole::Wire);
            }
            "arg" | "clap" | "command" => {
                roles.insert(SemanticRole::Cli);
            }
            "derive" => {
                if tokens.contains("Serialize") || tokens.contains("Deserialize") {
                    roles.insert(SemanticRole::Wire);
                }
                if tokens.contains("Default") {
                    roles.insert(SemanticRole::Default);
                }
                if ["Parser", "Args", "Subcommand", "ValueEnum", "CommandFactory"]
                    .iter()
                    .any(|derive| tokens.contains(derive))
                {
                    roles.insert(SemanticRole::Cli);
                }
            }
            _ => {}
        }
    }
    roles
}

fn meta_mentions_test(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::NameValue(_) => false,
        syn::Meta::List(list) => list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, Token![,]>::parse_terminated,
            )
            .map(|nested| nested.iter().any(meta_mentions_test))
            .unwrap_or(false),
    }
}

fn tracked_parameter_types_with_facts(
    inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> HashMap<String, TrackedIdentityType> {
    inputs
        .iter()
        .filter_map(|input| match input {
            FnArg::Typed(input) => {
                let syn::Pat::Ident(pattern) = input.pat.as_ref() else {
                    return None;
                };
                tracked_identity_type_with_facts(&input.ty, owner, facts)
                    .map(|kind| (pattern.ident.to_string(), kind))
            }
            FnArg::Receiver(_) => match owner {
                IdentityOwner::Impl { self_ty, .. } => {
                    Some(("self".to_string(), self_ty.clone()))
                }
                IdentityOwner::Module(_) => None,
            },
        })
        .collect()
}

fn tracked_return_type_with_facts(
    output: &syn::ReturnType,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Option<TrackedIdentityType> {
    match output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, kind) => tracked_identity_type_with_facts(kind, owner, facts),
    }
}

fn attribute_reference_paths(attribute: &Attribute) -> Result<Vec<syn::Path>, String> {
    let mut paths = Vec::new();
    collect_meta_reference_paths(&attribute.meta, &mut paths)?;
    paths.sort_by_key(|path| path.to_token_stream().to_string());
    paths.dedup_by_key(|path| path.to_token_stream().to_string());
    Ok(paths)
}

fn collect_meta_reference_paths(
    meta: &syn::Meta,
    paths: &mut Vec<syn::Path>,
) -> Result<(), String> {
    match meta {
        syn::Meta::Path(_) => {}
        syn::Meta::NameValue(value) => {
            let terminal = value.path.segments.last().map(|segment| segment.ident.to_string());
            if terminal.as_deref().is_some_and(|name| {
                matches!(
                    name,
                    "default"
                        | "default_value_t"
                        | "deserialize_with"
                        | "serialize_with"
                        | "skip_serializing_if"
                        | "value_parser"
                        | "with"
                )
            }) && let Expr::Lit(literal) = &value.value
                && let Lit::Str(literal) = &literal.lit
            {
                if terminal.as_deref() == Some("with") {
                    for method in ["serialize", "deserialize"] {
                        let path = syn::parse_str::<syn::Path>(&format!(
                            "{}::{method}",
                            literal.value()
                        ))
                        .map_err(|error| {
                            format!(
                                "failed to parse serde with path {:?}: {error}",
                                literal.value()
                            )
                        })?;
                        paths.push(path);
                    }
                } else {
                    let path = syn::parse_str::<syn::Path>(&literal.value()).map_err(|error| {
                        format!(
                            "failed to parse attribute reference {:?}: {error}",
                            literal.value()
                        )
                    })?;
                    paths.push(path);
                }
            }
            let mut collector = AttributeExpressionPathCollector { paths };
            collector.visit_expr(&value.value);
        }
        syn::Meta::List(list) => {
            let nested = list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, Token![,]>::parse_terminated,
                )
                .map_err(|error| {
                    format!(
                        "failed to parse structured attribute {}: {error}",
                        list.path.to_token_stream()
                    )
                })?;
            for meta in nested {
                collect_meta_reference_paths(&meta, paths)?;
            }
        }
    }
    Ok(())
}

struct AttributeExpressionPathCollector<'a> {
    paths: &'a mut Vec<syn::Path>,
}

impl Visit<'_> for AttributeExpressionPathCollector<'_> {
    fn visit_expr_call(&mut self, node: &syn::ExprCall) {
        if let Expr::Path(path) = node.func.as_ref() {
            self.paths.push(path.path.clone());
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_path(&mut self, node: &syn::ExprPath) {
        self.paths.push(node.path.clone());
        syn::visit::visit_expr_path(self, node);
    }
}

fn rust_carrier_edges(
    node: &RustCarrierNode,
    nodes: &HashMap<RustCarrierKey, RustCarrierNode>,
    facts: &IdentityTypeFacts,
) -> Result<Vec<RustCarrierKey>, String> {
    let mut visitor = RustCarrierEdgeVisitor {
        owner: &node.key.owner,
        nodes,
        facts,
        types: vec![node.parameter_types.clone()],
        edges: BTreeSet::new(),
        errors: Vec::new(),
    };
    match &node.body {
        RustCarrierBody::Block(block) => visitor.visit_block(block),
        RustCarrierBody::Expression(expression) => visitor.visit_expr(expression),
        RustCarrierBody::None => {}
    }
    if visitor.errors.is_empty() {
        Ok(visitor.edges.into_iter().collect())
    } else {
        visitor.errors.sort();
        visitor.errors.dedup();
        Err(format!(
            "carrier {} has unresolved protected edges: {}",
            node.key.selector(),
            visitor.errors.join("; ")
        ))
    }
}

struct RustCarrierEdgeVisitor<'a> {
    owner: &'a IdentityOwner,
    nodes: &'a HashMap<RustCarrierKey, RustCarrierNode>,
    facts: &'a IdentityTypeFacts,
    types: Vec<HashMap<String, TrackedIdentityType>>,
    edges: BTreeSet<RustCarrierKey>,
    errors: Vec<String>,
}

impl Visit<'_> for RustCarrierEdgeVisitor<'_> {
    fn visit_block(&mut self, node: &syn::Block) {
        self.types.push(HashMap::new());
        syn::visit::visit_block(self, node);
        self.types.pop();
    }

    fn visit_local(&mut self, node: &syn::Local) {
        if let Some((name, declared_type)) = local_binding(&node.pat)
            && let Some(initializer) = &node.init
            && let Some(kind) = declared_type
                .and_then(|kind| tracked_identity_type_with_facts(kind, self.owner, self.facts))
                .or_else(|| {
                    expression_tracked_type(
                        &initializer.expr,
                        self.owner,
                        &self.types,
                        self.facts,
                    )
                })
        {
            self.types
                .last_mut()
                .expect("carrier type scope")
                .insert(name, kind);
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_call(&mut self, node: &syn::ExprCall) {
        if let Expr::Path(path) = node.func.as_ref() {
            match resolve_call_path(&path.path, self.owner, self.facts) {
                RustCallResolution::Local(function) => {
                    self.edges.insert(RustCarrierKey::function(function));
                }
                RustCallResolution::LocalConstructor(_)
                | RustCallResolution::ProvenExternal
                | RustCallResolution::GeneratedByTrackedDerive => {}
                RustCallResolution::Ambiguous(error)
                | RustCallResolution::UnsupportedLocal(error) => self.errors.push(error),
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &syn::ExprMethodCall) {
        match resolve_method_call(node, self.owner, &self.types, self.facts) {
            RustCallResolution::Local(function) => {
                self.edges.insert(RustCarrierKey::function(function));
            }
            RustCallResolution::LocalConstructor(_)
            | RustCallResolution::ProvenExternal
            | RustCallResolution::GeneratedByTrackedDerive => {}
            RustCallResolution::Ambiguous(error)
            | RustCallResolution::UnsupportedLocal(error) => self.errors.push(error),
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_path(&mut self, node: &syn::ExprPath) {
        match resolve_named_carrier_path(
            &node.path,
            self.owner,
            self.nodes,
            self.facts,
            &[RustCarrierItemKind::Const, RustCarrierItemKind::Static],
        ) {
            Ok(Some(key)) => {
                self.edges.insert(key);
            }
            Ok(None) => {}
            Err(error) => self.errors.push(error),
        }
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_expr_macro(&mut self, node: &syn::ExprMacro) {
        match resolve_named_carrier_path(
            &node.mac.path,
            self.owner,
            self.nodes,
            self.facts,
            &[RustCarrierItemKind::Macro],
        ) {
            Ok(Some(key)) => {
                self.edges.insert(key);
            }
            Ok(None) => {}
            Err(error) => self.errors.push(error),
        }
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_item_fn(&mut self, _node: &syn::ItemFn) {}

    fn visit_impl_item_fn(&mut self, _node: &syn::ImplItemFn) {}
}

fn resolve_named_carrier_path(
    path: &syn::Path,
    owner: &IdentityOwner,
    nodes: &HashMap<RustCarrierKey, RustCarrierNode>,
    facts: &IdentityTypeFacts,
    kinds: &[RustCarrierItemKind],
) -> Result<Option<RustCarrierKey>, String> {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    let Some((name, prefix)) = segments.split_last() else {
        return Ok(None);
    };
    let mut candidates = Vec::new();
    if prefix == ["Self"] {
        for kind in kinds {
            let key = RustCarrierKey {
                owner: owner.clone(),
                kind: *kind,
                name: name.clone(),
            };
            if nodes.contains_key(&key) {
                candidates.push(key);
            }
        }
    } else {
        for target in expanded_symbol_paths(&segments, owner, facts)? {
            let RustImportTarget::Local(path) = target else {
                continue;
            };
            let Some((name, module)) = path.split_last() else {
                continue;
            };
            for kind in kinds {
                let key = RustCarrierKey {
                    owner: IdentityOwner::Module(module.to_vec()),
                    kind: *kind,
                    name: name.clone(),
                };
                if nodes.contains_key(&key) {
                    candidates.push(key);
                }
            }
            let type_label = module.join("::");
            if facts.local_types.contains(&type_label) {
                for kind in kinds {
                    for impl_owner in facts
                        .impl_owners
                        .get(&type_label)
                        .into_iter()
                        .flatten()
                    {
                        let key = RustCarrierKey {
                            owner: impl_owner.clone(),
                            kind: *kind,
                            name: name.clone(),
                        };
                        if nodes.contains_key(&key) {
                            candidates.push(key);
                        }
                    }
                }
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [] => Ok(None),
        [candidate] => Ok(Some(candidate.clone())),
        _ => Err(format!(
            "path {} resolved to {} local carrier items",
            path.to_token_stream(),
            candidates.len()
        )),
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

fn structured_json_inventory(path: &str, source: &str) -> Result<Vec<RawSurface>, String> {
    let value: serde_json::Value = serde_json::from_str(source)
        .map_err(|error| format!("failed to parse protected JSON {path}: {error}"))?;
    let mut items = Vec::new();
    collect_structured_value_as(
        &value,
        "",
        &mut items,
        "structured_key",
        "structured_value",
    )?;
    Ok(items)
}

fn structured_json_schema_inventory(
    path: &str,
    source: &str,
) -> Result<Vec<RawSurface>, String> {
    let value: serde_json::Value = serde_json::from_str(source)
        .map_err(|error| format!("failed to parse protected JSON {path}: {error}"))?;
    let Some(object) = value.as_object() else {
        return Ok(Vec::new());
    };
    let declared_schema = object
        .get("$schema")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|schema| !schema.trim().is_empty());
    let structural_schema = [
        "$defs",
        "allOf",
        "anyOf",
        "definitions",
        "oneOf",
        "properties",
    ]
    .iter()
    .any(|key| object.contains_key(*key));
    if !declared_schema && !structural_schema {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    collect_structured_value_as(
        &value,
        "",
        &mut items,
        "schema_key",
        "schema_value",
    )?;
    Ok(items)
}

fn structured_toml_inventory(path: &str, source: &str) -> Result<Vec<RawSurface>, String> {
    let value: toml::Value = toml::from_str(source)
        .map_err(|error| format!("failed to parse protected TOML {path}: {error}"))?;
    let value = serde_json::to_value(value)
        .map_err(|error| format!("failed to normalize protected TOML {path}: {error}"))?;
    let mut items = Vec::new();
    collect_structured_value_as(
        &value,
        "",
        &mut items,
        "structured_key",
        "structured_value",
    )?;
    Ok(items)
}

fn collect_structured_value_as(
    value: &serde_json::Value,
    pointer: &str,
    items: &mut Vec<RawSurface>,
    key_kind: &'static str,
    value_kind: &'static str,
) -> Result<(), String> {
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                items.push(RawSurface {
                    kind: value_kind,
                    selector: format!("value:{pointer}"),
                    content: "{}".to_string(),
                });
            }
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (key, child) in entries {
                let child_pointer = format!("{pointer}/{}", escape_json_pointer(key));
                items.push(RawSurface {
                    kind: key_kind,
                    selector: format!("key:{child_pointer}"),
                    content: key.clone(),
                });
                collect_structured_value_as(
                    child,
                    &child_pointer,
                    items,
                    key_kind,
                    value_kind,
                )?;
            }
        }
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                items.push(RawSurface {
                    kind: value_kind,
                    selector: format!("value:{pointer}"),
                    content: "[]".to_string(),
                });
            }
            for (index, child) in values.iter().enumerate() {
                collect_structured_value_as(
                    child,
                    &format!("{pointer}/{index}"),
                    items,
                    key_kind,
                    value_kind,
                )?;
            }
        }
        _ => items.push(RawSurface {
            kind: value_kind,
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
    key: IdentityFunctionKey,
    inputs: &'a syn::punctuated::Punctuated<FnArg, Token![,]>,
    output: &'a syn::ReturnType,
    block: &'a syn::Block,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct IdentityFunctionKey {
    owner: IdentityOwner,
    name: String,
}

type TrackedIdentityType = String;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum RustImportTarget {
    Local(Vec<String>),
    External(Vec<String>),
}

type RustImportKey = (Vec<String>, String);

#[derive(Clone)]
struct PendingRustImport {
    module: Vec<String>,
    alias: Option<String>,
    path: Vec<String>,
    glob: bool,
}

#[derive(Clone)]
struct PendingStructField {
    owner: String,
    module: Vec<String>,
    name: String,
    kind: Type,
}

#[derive(Clone)]
struct PendingTypeAlias {
    owner: String,
    module: Vec<String>,
    kind: Type,
}

#[derive(Clone, Default)]
struct IdentityTypeFacts {
    functions: HashSet<IdentityFunctionKey>,
    function_returns: HashMap<IdentityFunctionKey, TrackedIdentityType>,
    impl_functions: HashMap<(String, String), Vec<IdentityFunctionKey>>,
    impl_owners: HashMap<String, Vec<IdentityOwner>>,
    unit_impl_method_names: HashSet<(Vec<String>, String)>,
    struct_fields: HashMap<(String, String), TrackedIdentityType>,
    modules: HashSet<Vec<String>>,
    module_items: HashSet<Vec<String>>,
    local_path_prefixes: HashSet<Vec<String>>,
    local_types: HashSet<String>,
    local_constructors: HashSet<String>,
    enum_variants: HashSet<String>,
    generated_methods: HashSet<(String, String)>,
    type_alias_targets: HashMap<String, TrackedIdentityType>,
    imports: HashMap<RustImportKey, Vec<RustImportTarget>>,
    glob_imports: HashMap<Vec<String>, Vec<RustImportTarget>>,
    external_crates: HashMap<Vec<String>, HashSet<String>>,
    pending_imports: Vec<PendingRustImport>,
    pending_struct_fields: Vec<PendingStructField>,
    pending_type_aliases: Vec<PendingTypeAlias>,
    resolution_errors: RefCell<BTreeSet<String>>,
}

impl IdentityTypeFacts {
    fn record_resolution_error(&self, error: String) {
        self.resolution_errors.borrow_mut().insert(error);
    }

    fn resolution_error_messages(&self) -> Vec<String> {
        self.resolution_errors.borrow().iter().cloned().collect()
    }

    fn ensure_resolution_succeeded(&self) -> Result<(), String> {
        let errors = self.resolution_error_messages();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    fn set_functions(&mut self, functions: HashSet<IdentityFunctionKey>) {
        self.functions = functions;
        self.impl_functions.clear();
        self.unit_impl_method_names.clear();
        for function in &self.functions {
            if let IdentityOwner::Impl { self_ty, .. } = &function.owner {
                self.unit_impl_method_names.insert((
                    compile_unit_root(function.owner.module()).to_vec(),
                    function.name.clone(),
                ));
                self.impl_functions
                    .entry((self_ty.clone(), function.name.clone()))
                    .or_default()
                    .push(function.clone());
            }
        }
        for functions in self.impl_functions.values_mut() {
            functions.sort();
            functions.dedup();
        }
    }

    fn register_external_crates(&mut self, module: &[String], crates: &HashSet<String>) {
        self.external_crates
            .entry(compile_unit_root(module).to_vec())
            .or_default()
            .extend(crates.iter().cloned());
    }

    fn collect_symbols(&mut self, items: &[Item], module: &[String]) -> Result<(), String> {
        self.modules.insert(module.to_vec());
        for item in items {
            match item {
                Item::Const(item) => self.register_module_item(module, &item.ident.to_string()),
                Item::Enum(item) => {
                    let owner = self.register_type(module, &item.ident.to_string());
                    self.register_derive_methods(&owner, &item.attrs)?;
                    for variant in &item.variants {
                        let variant = format!("{owner}::{}", variant.ident);
                        self.enum_variants.insert(variant.clone());
                        self.local_constructors.insert(variant);
                    }
                }
                Item::ExternCrate(item) => {
                    let alias = item
                        .rename
                        .as_ref()
                        .map_or_else(|| item.ident.to_string(), |(_, alias)| alias.to_string());
                    self.imports
                        .entry((module.to_vec(), alias))
                        .or_default()
                        .push(RustImportTarget::External(vec![item.ident.to_string()]));
                }
                Item::Fn(item) => self.register_module_item(module, &item.sig.ident.to_string()),
                Item::Macro(item) => {
                    if let Some(identifier) = &item.ident {
                        self.register_module_item(module, &identifier.to_string());
                    }
                }
                Item::Mod(item) => {
                    self.register_module_item(module, &item.ident.to_string());
                    if let Some((_, nested)) = &item.content {
                        let mut next = module.to_vec();
                        next.push(item.ident.to_string());
                        self.collect_symbols(nested, &next)?;
                    }
                }
                Item::Static(item) => self.register_module_item(module, &item.ident.to_string()),
                Item::Struct(item) => {
                    let owner = self.register_type(module, &item.ident.to_string());
                    self.register_derive_methods(&owner, &item.attrs)?;
                    if matches!(item.fields, Fields::Unnamed(_) | Fields::Unit) {
                        self.local_constructors.insert(owner.clone());
                    }
                    for field in &item.fields {
                        if let Some(name) = &field.ident {
                            self.pending_struct_fields.push(PendingStructField {
                                owner: owner.clone(),
                                module: module.to_vec(),
                                name: name.to_string(),
                                kind: field.ty.clone(),
                            });
                        }
                    }
                }
                Item::Type(item) => {
                    let owner = self.register_type(module, &item.ident.to_string());
                    self.pending_type_aliases.push(PendingTypeAlias {
                        owner,
                        module: module.to_vec(),
                        kind: (*item.ty).clone(),
                    });
                }
                Item::Trait(item) => {
                    self.register_type(module, &item.ident.to_string());
                }
                Item::TraitAlias(item) => {
                    self.register_type(module, &item.ident.to_string());
                }
                Item::Union(item) => {
                    let owner = self.register_type(module, &item.ident.to_string());
                    for field in &item.fields.named {
                        if let Some(name) = &field.ident {
                            self.pending_struct_fields.push(PendingStructField {
                                owner: owner.clone(),
                                module: module.to_vec(),
                                name: name.to_string(),
                                kind: field.ty.clone(),
                            });
                        }
                    }
                }
                Item::Use(item) => {
                    collect_pending_use_tree(
                        &item.tree,
                        module,
                        Vec::new(),
                        &mut self.pending_imports,
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn register_module_item(&mut self, module: &[String], name: &str) {
        let mut path = module.to_vec();
        path.push(name.to_string());
        self.module_items.insert(path);
    }

    fn register_type(&mut self, module: &[String], name: &str) -> String {
        self.register_module_item(module, name);
        let owner = canonical_symbol_label(module, name);
        self.local_types.insert(owner.clone());
        owner
    }

    fn register_derive_methods(
        &mut self,
        owner: &str,
        attributes: &[Attribute],
    ) -> Result<(), String> {
        for attribute in attributes
            .iter()
            .filter(|attribute| attribute.path().is_ident("derive"))
        {
            let derives = attribute
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Path, Token![,]>::parse_terminated,
                )
                .map_err(|error| {
                    format!("failed to parse derive list for {owner}: {error}")
                })?;
            for derive in derives {
                let normalized = derive
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>();
                let method = match normalized.as_slice() {
                    [name] if name == "Clone" => Some("clone"),
                    [name] if name == "Default" => Some("default"),
                    [root, module, name]
                        if matches!(root.as_str(), "std" | "core")
                            && module == "clone"
                            && name == "Clone" =>
                    {
                        Some("clone")
                    }
                    [root, module, name]
                        if matches!(root.as_str(), "std" | "core")
                            && module == "default"
                            && name == "Default" =>
                    {
                        Some("default")
                    }
                    _ => None,
                };
                if let Some(method) = method {
                    self.generated_methods
                        .insert((owner.to_string(), method.to_string()));
                }
            }
        }
        Ok(())
    }

    fn finish_symbols(&mut self) -> Result<(), String> {
        let profile_started = Instant::now();
        let pending_imports = std::mem::take(&mut self.pending_imports);
        let declared_aliases = pending_imports
            .iter()
            .filter_map(|pending| {
                pending
                    .alias
                    .as_ref()
                    .map(|alias| (pending.module.clone(), alias.clone()))
            })
            .collect::<HashSet<_>>();
        for pending in &pending_imports {
            if let Some(alias) = &pending.alias {
                self.register_module_item(&pending.module, alias);
            }
        }
        self.rebuild_local_path_prefixes();
        eprintln!(
            "PERF finish_symbols prefixes={} pending_imports={} elapsed_ms={}",
            self.local_path_prefixes.len(),
            pending_imports.len(),
            profile_started.elapsed().as_millis()
        );
        let mut errors = Vec::new();
        for (index, pending) in pending_imports.into_iter().enumerate() {
            match self.resolve_use_target(
                &pending.module,
                &pending.path,
                &declared_aliases,
            ) {
                Ok(target) => {
                    if pending.glob {
                        self.glob_imports
                            .entry(pending.module)
                            .or_default()
                            .push(target);
                    } else if let Some(alias) = pending.alias {
                        self.imports
                            .entry((pending.module, alias))
                            .or_default()
                            .push(target);
                    }
                }
                Err(error) => errors.push(error),
            }
            if (index + 1) % 250 == 0 {
                eprintln!(
                    "PERF finish_symbols imports={} elapsed_ms={}",
                    index + 1,
                    profile_started.elapsed().as_millis()
                );
            }
        }
        for targets in self.imports.values_mut() {
            targets.sort();
            targets.dedup();
        }
        for targets in self.glob_imports.values_mut() {
            targets.sort();
            targets.dedup();
        }
        if !errors.is_empty() {
            errors.sort();
            errors.dedup();
            return Err(errors.join("\n"));
        }
        self.imports = resolve_import_aliases(
            &self.imports,
            &self.modules,
            &self.local_types,
        )?;
        let pending_globs = std::mem::take(&mut self.glob_imports);
        for (module, targets) in pending_globs {
            let mut resolved = Vec::new();
            for target in targets {
                resolved.extend(expand_import_targets(vec![target], self)?);
            }
            resolved.sort();
            resolved.dedup();
            self.glob_imports.insert(module, resolved);
        }
        let pending_aliases = std::mem::take(&mut self.pending_type_aliases);
        let mut raw_type_aliases = HashMap::new();
        for pending in pending_aliases {
            let lexical_owner = IdentityOwner::Module(pending.module);
            if let Some(target) =
                tracked_identity_type_with_facts_checked(&pending.kind, &lexical_owner, self)?
            {
                raw_type_aliases.insert(pending.owner, target);
            }
        }
        self.type_alias_targets = resolve_type_alias_targets(&raw_type_aliases)?;
        let pending_fields = std::mem::take(&mut self.pending_struct_fields);
        eprintln!(
            "PERF finish_symbols imports_done={} glob_modules={} pending_fields={} elapsed_ms={}",
            self.imports.len(),
            self.glob_imports.len(),
            pending_fields.len(),
            profile_started.elapsed().as_millis()
        );
        for (index, pending) in pending_fields.into_iter().enumerate() {
            if (5_240..=5_260).contains(&index) {
                eprintln!(
                    "PERF finish_symbols field_index={} owner={} name={} type={}",
                    index,
                    pending.owner,
                    pending.name,
                    pending.kind.to_token_stream()
                );
            }
            let owner = IdentityOwner::Module(pending.module.clone());
            match tracked_identity_type_with_facts_checked(&pending.kind, &owner, self) {
                Ok(Some(kind)) => {
                    self.struct_fields
                        .insert((pending.owner, pending.name), kind);
                }
                Ok(None) => {}
                Err(error) => errors.push(format!(
                    "failed to resolve field {}.{}: {error}",
                    pending.owner, pending.name
                )),
            }
            if (index + 1) % 250 == 0 {
                eprintln!(
                    "PERF finish_symbols fields={} elapsed_ms={}",
                    index + 1,
                    profile_started.elapsed().as_millis()
                );
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            errors.sort();
            errors.dedup();
            Err(errors.join("\n"))
        }
    }

    fn resolve_use_target(
        &self,
        module: &[String],
        path: &[String],
        declared_aliases: &HashSet<RustImportKey>,
    ) -> Result<RustImportTarget, String> {
        let first = path
            .first()
            .ok_or_else(|| format!("empty use path in {}", module_label(module)))?;
        let root = compile_unit_root(module);
        if first == "crate" {
            let mut target = root.to_vec();
            target.extend(path.iter().skip(1).cloned());
            return Ok(RustImportTarget::Local(target));
        }
        if first == "self" || first == "super" {
            let mut target = module.to_vec();
            let mut index = usize::from(first == "self");
            while path.get(index).is_some_and(|part| part == "super") {
                if target.len() == root.len() {
                    return Err(format!(
                        "use path {} escapes compile unit {}",
                        path.join("::"),
                        module_label(module)
                    ));
                }
                target.pop();
                index += 1;
            }
            target.extend(path[index..].iter().cloned());
            return Ok(RustImportTarget::Local(target));
        }
        let mut lexical_local = module.to_vec();
        lexical_local.extend(path.iter().cloned());
        if self.has_local_path_prefix(&lexical_local) {
            return Ok(RustImportTarget::Local(lexical_local));
        }
        if path_contains_declared_import_alias(&lexical_local, declared_aliases) {
            return Ok(RustImportTarget::Local(lexical_local));
        }
        let mut root_local = root.to_vec();
        root_local.extend(path.iter().cloned());
        if root_local != lexical_local && self.has_local_path_prefix(&root_local) {
            return Ok(RustImportTarget::Local(root_local));
        }
        if root_local != lexical_local
            && path_contains_declared_import_alias(&root_local, declared_aliases)
        {
            return Ok(RustImportTarget::Local(root_local));
        }
        if self
            .external_crates
            .get(root)
            .is_some_and(|crates| crates.contains(first))
            || is_rust_external_crate(first)
        {
            return Ok(RustImportTarget::External(path.to_vec()));
        }
        Err(format!(
            "use path {} in {} is neither a local compile-unit symbol nor a declared external crate",
            path.join("::"),
            module_label(module)
        ))
    }

    fn has_local_path_prefix(&self, path: &[String]) -> bool {
        self.local_path_prefixes.contains(path)
    }

    fn rebuild_local_path_prefixes(&mut self) {
        self.local_path_prefixes.clear();
        for path in self.modules.iter().chain(self.module_items.iter()) {
            for length in 1..=path.len() {
                self.local_path_prefixes.insert(path[..length].to_vec());
            }
        }
    }
}

fn collect_pending_use_tree(
    tree: &syn::UseTree,
    module: &[String],
    prefix: Vec<String>,
    pending: &mut Vec<PendingRustImport>,
) -> Result<(), String> {
    match tree {
        syn::UseTree::Path(path) => {
            let mut prefix = prefix;
            prefix.push(path.ident.to_string());
            collect_pending_use_tree(&path.tree, module, prefix, pending)
        }
        syn::UseTree::Name(name) => {
            let mut path = prefix;
            let identifier = name.ident.to_string();
            if identifier == "self" {
                let alias = path.last().cloned().ok_or_else(|| {
                    format!("use self has no owner in {}", module_label(module))
                })?;
                pending.push(PendingRustImport {
                    module: module.to_vec(),
                    alias: Some(alias),
                    path,
                    glob: false,
                });
            } else {
                path.push(identifier.clone());
                pending.push(PendingRustImport {
                    module: module.to_vec(),
                    alias: Some(identifier),
                    path,
                    glob: false,
                });
            }
            Ok(())
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix;
            if rename.ident != "self" {
                path.push(rename.ident.to_string());
            }
            if path.is_empty() {
                return Err(format!(
                    "use self as {} has no owner in {}",
                    rename.rename,
                    module_label(module)
                ));
            }
            pending.push(PendingRustImport {
                module: module.to_vec(),
                alias: Some(rename.rename.to_string()),
                path,
                glob: false,
            });
            Ok(())
        }
        syn::UseTree::Glob(_) => {
            pending.push(PendingRustImport {
                module: module.to_vec(),
                alias: None,
                path: prefix,
                glob: true,
            });
            Ok(())
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                collect_pending_use_tree(tree, module, prefix.clone(), pending)?;
            }
            Ok(())
        }
    }
}

fn rust_import_key_label(key: &RustImportKey) -> String {
    let (module, alias) = key;
    if module.is_empty() {
        alias.clone()
    } else {
        format!("{}::{alias}", module_label(module))
    }
}

fn resolve_import_aliases(
    imports: &HashMap<RustImportKey, Vec<RustImportTarget>>,
    modules: &HashSet<Vec<String>>,
    local_types: &HashSet<String>,
) -> Result<HashMap<RustImportKey, Vec<RustImportTarget>>, String> {
    let mut keys = imports.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    let edge_budget = imports
        .values()
        .map(Vec::len)
        .sum::<usize>()
        .saturating_add(imports.len())
        .max(1);
    let mut memo = HashMap::<RustImportKey, Arc<[RustImportTarget]>>::new();
    for key in &keys {
        let mut visiting = Vec::<RustImportKey>::new();
        resolve_import_alias(
            key,
            imports,
            &mut memo,
            &mut visiting,
            edge_budget,
            modules,
            local_types,
        )?;
    }
    Ok(keys
        .into_iter()
        .map(|key| {
            let targets = memo
                .remove(&key)
                .expect("resolved import alias")
                .as_ref()
                .to_vec();
            (key, targets)
        })
        .collect())
}

fn resolve_import_alias(
    key: &RustImportKey,
    imports: &HashMap<RustImportKey, Vec<RustImportTarget>>,
    memo: &mut HashMap<RustImportKey, Arc<[RustImportTarget]>>,
    visiting: &mut Vec<RustImportKey>,
    edge_budget: usize,
    modules: &HashSet<Vec<String>>,
    local_types: &HashSet<String>,
) -> Result<Arc<[RustImportTarget]>, String> {
    if let Some(resolved) = memo.get(key) {
        return Ok(Arc::clone(resolved));
    }
    if let Some(index) = visiting.iter().position(|candidate| candidate == key) {
        let mut cycle = visiting[index..]
            .iter()
            .map(rust_import_key_label)
            .collect::<Vec<_>>();
        cycle.push(rust_import_key_label(key));
        return Err(format!("local import alias cycle: {}", cycle.join(" -> ")));
    }
    let targets = imports.get(key).ok_or_else(|| {
        format!("missing local import alias {}", rust_import_key_label(key))
    })?;
    visiting.push(key.clone());
    let mut resolved = BTreeSet::new();
    for target in targets {
        resolved.extend(resolve_import_target(
            target.clone(),
            imports,
            memo,
            visiting,
            edge_budget,
            modules,
            local_types,
        )?);
        if resolved.len() > edge_budget {
            return Err(format!(
                "import alias {} expansion exceeded deterministic edge budget {edge_budget}",
                rust_import_key_label(key)
            ));
        }
    }
    visiting.pop();
    let resolved: Arc<[RustImportTarget]> =
        resolved.into_iter().collect::<Vec<_>>().into();
    memo.insert(key.clone(), Arc::clone(&resolved));
    Ok(resolved)
}

fn resolve_import_target(
    target: RustImportTarget,
    imports: &HashMap<RustImportKey, Vec<RustImportTarget>>,
    memo: &mut HashMap<RustImportKey, Arc<[RustImportTarget]>>,
    visiting: &mut Vec<RustImportKey>,
    edge_budget: usize,
    modules: &HashSet<Vec<String>>,
    local_types: &HashSet<String>,
) -> Result<BTreeSet<RustImportTarget>, String> {
    let RustImportTarget::Local(path) = target else {
        return Ok(BTreeSet::from([target]));
    };
    let Some((key, suffix)) = first_import_alias(
        &path,
        imports,
        modules,
        local_types,
    ) else {
        return Ok(BTreeSet::from([RustImportTarget::Local(path)]));
    };
    let bases = resolve_import_alias(
        &key,
        imports,
        memo,
        visiting,
        edge_budget,
        modules,
        local_types,
    )?;
    let mut resolved = BTreeSet::new();
    for base in bases.iter() {
        let combined = append_import_suffix(base.clone(), suffix);
        resolved.extend(resolve_import_target(
            combined,
            imports,
            memo,
            visiting,
            edge_budget,
            modules,
            local_types,
        )?);
        if resolved.len() > edge_budget {
            return Err(format!(
                "import path expansion exceeded deterministic edge budget {edge_budget}"
            ));
        }
    }
    Ok(resolved)
}

fn first_import_alias<'a>(
    path: &'a [String],
    imports: &HashMap<RustImportKey, Vec<RustImportTarget>>,
    modules: &HashSet<Vec<String>>,
    local_types: &HashSet<String>,
) -> Option<(RustImportKey, &'a [String])> {
    for index in 1..=path.len() {
        if index < path.len()
            && (modules.contains(&path[..index])
                || local_types.contains(&path[..index].join("::")))
        {
            continue;
        }
        let key = (path[..index - 1].to_vec(), path[index - 1].clone());
        if imports.contains_key(&key) {
            return Some((key, &path[index..]));
        }
    }
    None
}

fn path_contains_declared_import_alias(
    path: &[String],
    aliases: &HashSet<RustImportKey>,
) -> bool {
    (1..=path.len()).any(|index| {
        aliases.contains(&(path[..index - 1].to_vec(), path[index - 1].clone()))
    })
}

fn append_import_suffix(target: RustImportTarget, suffix: &[String]) -> RustImportTarget {
    match target {
        RustImportTarget::Local(mut path) => {
            path.extend(suffix.iter().cloned());
            RustImportTarget::Local(path)
        }
        RustImportTarget::External(mut path) => {
            path.extend(suffix.iter().cloned());
            RustImportTarget::External(path)
        }
    }
}

fn resolve_type_alias_targets(
    aliases: &HashMap<String, TrackedIdentityType>,
) -> Result<HashMap<String, TrackedIdentityType>, String> {
    fn resolve(
        owner: &str,
        aliases: &HashMap<String, TrackedIdentityType>,
        memo: &mut HashMap<String, TrackedIdentityType>,
        visiting: &mut Vec<String>,
    ) -> Result<TrackedIdentityType, String> {
        if let Some(target) = memo.get(owner) {
            return Ok(target.clone());
        }
        if let Some(index) = visiting.iter().position(|candidate| candidate == owner) {
            let mut cycle = visiting[index..].to_vec();
            cycle.push(owner.to_string());
            return Err(format!("local type alias cycle: {}", cycle.join(" -> ")));
        }
        let target = aliases
            .get(owner)
            .ok_or_else(|| format!("missing local type alias {owner}"))?;
        visiting.push(owner.to_string());
        let resolved = if aliases.contains_key(target) {
            resolve(target, aliases, memo, visiting)?
        } else {
            target.clone()
        };
        visiting.pop();
        memo.insert(owner.to_string(), resolved.clone());
        Ok(resolved)
    }

    let mut keys = aliases.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    let mut resolved = HashMap::new();
    for key in keys {
        resolve(&key, aliases, &mut resolved, &mut Vec::new())?;
    }
    Ok(resolved)
}

fn canonical_symbol_label(module: &[String], name: &str) -> String {
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{}::{name}", module.join("::"))
    }
}

fn is_rust_external_crate(name: &str) -> bool {
    matches!(name, "std" | "core" | "alloc" | "proc_macro" | "test")
}

fn identity_type_facts(file: &syn::File) -> Result<IdentityTypeFacts, String> {
    let mut facts = IdentityTypeFacts::default();
    facts.collect_symbols(&file.items, &[])?;
    facts.finish_symbols()?;
    let mut functions = Vec::new();
    collect_identity_functions(&file.items, &[], &facts, &mut functions);
    facts.set_functions(functions
        .iter()
        .map(|function| function.key.clone())
        .collect());
    for function in functions {
        let syn::ReturnType::Type(_, kind) = function.output else {
            continue;
        };
        let Some(kind) = tracked_identity_type_with_facts(kind, &function.key.owner, &facts) else {
            continue;
        };
        facts.function_returns.insert(function.key, kind);
    }
    facts.ensure_resolution_succeeded()?;
    Ok(facts)
}

fn tracked_identity_type_with_facts(
    kind: &Type,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Option<TrackedIdentityType> {
    match tracked_identity_type_with_facts_checked(kind, owner, facts) {
        Ok(kind) => kind,
        Err(error) => {
            facts.record_resolution_error(error);
            None
        }
    }
}

fn tracked_identity_type_with_facts_checked(
    kind: &Type,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Result<Option<TrackedIdentityType>, String> {
    if type_is_raw_identity(kind) {
        return Ok(Some(String::new()));
    }
    match kind {
        Type::Reference(reference) => {
            tracked_identity_type_with_facts_checked(&reference.elem, owner, facts)
        }
        Type::Paren(paren) => tracked_identity_type_with_facts_checked(&paren.elem, owner, facts),
        Type::Group(group) => tracked_identity_type_with_facts_checked(&group.elem, owner, facts),
        Type::Path(path) => {
            let resolved = resolve_type_path_with_facts_checked(&path.path, owner, facts)?
                .or_else(|| {
                path.path
                    .segments
                    .last()
                    .map(|segment| format!("$unknown$::{}", segment.ident))
                });
            Ok(resolved.map(|kind| {
                facts
                    .type_alias_targets
                    .get(&kind)
                    .cloned()
                    .unwrap_or(kind)
            }))
        }
        _ => Ok(None),
    }
}

fn resolve_type_path_with_facts(
    path: &syn::Path,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Option<TrackedIdentityType> {
    match resolve_type_path_with_facts_checked(path, owner, facts) {
        Ok(kind) => kind,
        Err(error) => {
            facts.record_resolution_error(error);
            None
        }
    }
}

fn resolve_type_path_with_facts_checked(
    path: &syn::Path,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Result<Option<TrackedIdentityType>, String> {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    if segments == ["Self"] {
        return Ok(match owner {
            IdentityOwner::Impl { self_ty, .. } => Some(self_ty.clone()),
            IdentityOwner::Module(_) => None,
        });
    }
    let candidates = expanded_symbol_paths(&segments, owner, facts)?;
    let mut local = candidates
        .iter()
        .filter_map(|target| match target {
            RustImportTarget::Local(path) => {
                let label = path.join("::");
                facts.local_types.contains(&label).then_some(label)
            }
            RustImportTarget::External(_) => None,
        })
        .collect::<Vec<_>>();
    local.sort();
    local.dedup();
    if local.len() == 1 {
        let kind = local.into_iter().next().expect("one local type");
        return Ok(Some(
            facts
                .type_alias_targets
                .get(&kind)
                .cloned()
                .unwrap_or(kind),
        ));
    }
    if local.len() > 1 {
        return Err(format!(
            "type path {} resolved to {} local types",
            path.to_token_stream(),
            local.len()
        ));
    }
    let mut external = candidates
        .iter()
        .filter_map(|target| match target {
            RustImportTarget::External(path) => Some(format!("$external$::{}", path.join("::"))),
            RustImportTarget::Local(_) => None,
        })
        .collect::<Vec<_>>();
    external.sort();
    external.dedup();
    if external.len() == 1 {
        return Ok(external.into_iter().next());
    }
    if external.len() > 1 {
        return Err(format!(
            "type path {} resolved to {} external types",
            path.to_token_stream(),
            external.len()
        ));
    }
    Ok(segments.last().and_then(|name| {
        is_rust_prelude_type(name).then(|| format!("$rust$::{name}"))
    }))
}

fn expanded_symbol_paths(
    segments: &[String],
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Result<Vec<RustImportTarget>, String> {
    let Some(first) = segments.first() else {
        return Ok(Vec::new());
    };
    let module = owner.module();
    let root = compile_unit_root(module);
    if first == "crate" || first == "self" || first == "super" {
        let mut target = if first == "crate" {
            root.to_vec()
        } else {
            module.to_vec()
        };
        let mut index = usize::from(first == "crate" || first == "self");
        while segments.get(index).is_some_and(|part| part == "super") {
            if target.len() == root.len() {
                return Ok(Vec::new());
            }
            target.pop();
            index += 1;
        }
        target.extend(segments[index..].iter().cloned());
        return expand_import_targets(vec![RustImportTarget::Local(target)], facts);
    }
    let mut lexical_namespace = module.to_vec();
    lexical_namespace.push(first.clone());
    let first_is_local_namespace = segments.len() > 1
        && (facts.modules.contains(&lexical_namespace)
            || facts
                .local_types
                .contains(&lexical_namespace.join("::")));
    if !first_is_local_namespace
        && let Some(targets) = facts.imports.get(&(module.to_vec(), first.clone()))
    {
        let targets = targets
            .iter()
            .map(|target| match target {
                RustImportTarget::Local(path) => {
                    let mut path = path.clone();
                    path.extend(segments.iter().skip(1).cloned());
                    RustImportTarget::Local(path)
                }
                RustImportTarget::External(path) => {
                    let mut path = path.clone();
                    path.extend(segments.iter().skip(1).cloned());
                    RustImportTarget::External(path)
                }
            })
            .collect();
        return expand_import_targets(targets, facts);
    }
    let mut candidates = Vec::new();
    let mut current = module.to_vec();
    current.extend(segments.iter().cloned());
    candidates.push(RustImportTarget::Local(current));
    if let Some(globs) = facts.glob_imports.get(module) {
        for glob in globs {
            match glob {
                RustImportTarget::Local(path) => {
                    let mut path = path.clone();
                    path.extend(segments.iter().cloned());
                    candidates.push(RustImportTarget::Local(path));
                }
                RustImportTarget::External(path) => {
                    let mut path = path.clone();
                    path.extend(segments.iter().cloned());
                    candidates.push(RustImportTarget::External(path));
                }
            }
        }
    }
    if facts
        .external_crates
        .get(root)
        .is_some_and(|crates| crates.contains(first))
        || is_rust_external_crate(first)
    {
        candidates.push(RustImportTarget::External(segments.to_vec()));
    }
    expand_import_targets(candidates, facts)
}

fn expand_import_targets(
    targets: Vec<RustImportTarget>,
    facts: &IdentityTypeFacts,
) -> Result<Vec<RustImportTarget>, String> {
    let mut expanded = Vec::new();
    for target in targets {
        expand_normalized_import_target(target, facts, &mut Vec::new(), &mut expanded)?;
    }
    expanded.sort();
    expanded.dedup();
    Ok(expanded)
}

fn expand_normalized_import_target(
    target: RustImportTarget,
    facts: &IdentityTypeFacts,
    visiting: &mut Vec<RustImportKey>,
    expanded: &mut Vec<RustImportTarget>,
) -> Result<(), String> {
    let RustImportTarget::Local(path) = target else {
        expanded.push(target);
        return Ok(());
    };
    let Some((key, suffix)) = first_import_alias(
        &path,
        &facts.imports,
        &facts.modules,
        &facts.local_types,
    ) else {
        expanded.push(RustImportTarget::Local(path));
        return Ok(());
    };
    if let Some(index) = visiting.iter().position(|candidate| candidate == &key) {
        let mut cycle = visiting[index..]
            .iter()
            .map(rust_import_key_label)
            .collect::<Vec<_>>();
        cycle.push(rust_import_key_label(&key));
        return Err(format!("local import alias cycle: {}", cycle.join(" -> ")));
    }
    let replacements = facts.imports.get(&key).ok_or_else(|| {
        format!("missing normalized import alias {}", rust_import_key_label(&key))
    })?;
    if replacements.len() != 1 {
        return Err(format!(
            "import alias {} resolves ambiguously to {} targets",
            rust_import_key_label(&key),
            replacements.len()
        ));
    }
    visiting.push(key);
    expand_normalized_import_target(
        append_import_suffix(replacements[0].clone(), suffix),
        facts,
        visiting,
        expanded,
    )?;
    visiting.pop();
    Ok(())
}

fn is_rust_prelude_type(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "char"
            | "f32"
            | "f64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "Box"
            | "Option"
            | "Result"
            | "String"
            | "Vec"
    )
}

fn infer_identity_parameter_axes(
    file: &syn::File,
) -> Result<HashMap<IdentityFunctionKey, Vec<Option<&'static str>>>, String> {
    let type_facts = identity_type_facts(file)?;
    let mut functions = Vec::new();
    collect_identity_functions(&file.items, &[], &type_facts, &mut functions);
    infer_identity_parameter_axes_for_functions(&functions, &type_facts)
}

fn infer_identity_parameter_axes_for_functions(
    functions: &[IdentityFunction<'_>],
    type_facts: &IdentityTypeFacts,
) -> Result<HashMap<IdentityFunctionKey, Vec<Option<&'static str>>>, String> {
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
    let mut targets = HashMap::<IdentityFunctionKey, Vec<usize>>::new();
    for (index, function) in functions.iter().enumerate() {
        targets
            .entry(function.key.clone())
            .or_default()
            .push(index);
    }
    let propagation_budget = axes.iter().map(Vec::len).sum::<usize>().saturating_add(1);
    let mut resolution_errors = Vec::new();
    for _ in 0..propagation_budget {
        let mut inferred_calls = Vec::new();
        for (function, parameters) in functions.iter().zip(&axes) {
            let mut visitor = IdentityCallPropagation::new(
                function.inputs,
                parameters,
                &function.key.owner,
                &type_facts,
            );
            visitor.visit_block(function.block);
            inferred_calls.extend(visitor.calls);
            resolution_errors.extend(visitor.errors);
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

    if !resolution_errors.is_empty() {
        resolution_errors.sort();
        resolution_errors.dedup();
        return Err(resolution_errors.join("\n"));
    }

    let mut merged = HashMap::<IdentityFunctionKey, Vec<Option<&'static str>>>::new();
    for (function, parameters) in functions.iter().zip(axes) {
        let entry = merged
            .entry(function.key.clone())
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
    Ok(merged)
}

fn collect_identity_functions<'a>(
    items: &'a [Item],
    module: &[String],
    facts: &IdentityTypeFacts,
    functions: &mut Vec<IdentityFunction<'a>>,
) {
    for item in items {
        match item {
            Item::Fn(function) => functions.push(IdentityFunction {
                key: IdentityFunctionKey {
                    owner: IdentityOwner::Module(module.to_vec()),
                    name: function.sig.ident.to_string(),
                },
                inputs: &function.sig.inputs,
                output: &function.sig.output,
                block: &function.block,
            }),
            Item::Impl(implementation) => {
                let owner = identity_impl_owner_with_facts(implementation, module, facts);
                for item in &implementation.items {
                    if let ImplItem::Fn(function) = item {
                        functions.push(IdentityFunction {
                            key: IdentityFunctionKey {
                                owner: owner.clone(),
                                name: function.sig.ident.to_string(),
                            },
                            inputs: &function.sig.inputs,
                            output: &function.sig.output,
                            block: &function.block,
                        });
                    }
                }
            }
            Item::Trait(item) => {
                let owner = identity_trait_owner(item, module);
                for member in &item.items {
                    let syn::TraitItem::Fn(function) = member else {
                        continue;
                    };
                    let Some(block) = &function.default else {
                        continue;
                    };
                    functions.push(IdentityFunction {
                        key: IdentityFunctionKey {
                            owner: owner.clone(),
                            name: function.sig.ident.to_string(),
                        },
                        inputs: &function.sig.inputs,
                        output: &function.sig.output,
                        block,
                    });
                }
            }
            Item::Mod(item_module) => {
                if let Some((_, items)) = &item_module.content {
                    let mut nested = module.to_vec();
                    nested.push(item_module.ident.to_string());
                    collect_identity_functions(items, &nested, facts, functions);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RustCallResolution {
    Local(IdentityFunctionKey),
    LocalConstructor(TrackedIdentityType),
    ProvenExternal,
    GeneratedByTrackedDerive,
    Ambiguous(String),
    UnsupportedLocal(String),
}

fn resolve_call_path(
    path: &syn::Path,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> RustCallResolution {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    let Some((name, prefix)) = segments.split_last() else {
        return RustCallResolution::UnsupportedLocal("empty call path".to_string());
    };
    if segments == ["Self"] {
        return match owner {
            IdentityOwner::Impl { self_ty, .. } => {
                RustCallResolution::LocalConstructor(self_ty.clone())
            }
            IdentityOwner::Module(_) => RustCallResolution::UnsupportedLocal(
                "Self constructor outside an impl".to_string(),
            ),
        };
    }
    if prefix == ["Self"] {
        let key = IdentityFunctionKey {
            owner: owner.clone(),
            name: name.clone(),
        };
        if facts.functions.contains(&key) {
            return RustCallResolution::Local(key);
        }
        if let IdentityOwner::Impl { self_ty, .. } = owner
            && facts
                .generated_methods
                .contains(&(self_ty.clone(), name.clone()))
        {
            return RustCallResolution::GeneratedByTrackedDerive;
        }
        return RustCallResolution::UnsupportedLocal(format!(
            "unresolved Self call {}",
            path.to_token_stream()
        ));
    }

    let expanded = match expanded_symbol_paths(&segments, owner, facts) {
        Ok(expanded) => expanded,
        Err(error) => return RustCallResolution::UnsupportedLocal(error),
    };
    let mut local = Vec::<IdentityFunctionKey>::new();
    let mut constructors = Vec::<String>::new();
    let mut generated = false;
    let mut external = false;
    let mut local_evidence = false;
    for target in expanded {
        match target {
            RustImportTarget::External(_) => external = true,
            RustImportTarget::Local(path) => {
                let label = path.join("::");
                if facts.local_constructors.contains(&label)
                    || facts.enum_variants.contains(&label)
                {
                    constructors.push(label.clone());
                }
                if let Some((function, module)) = path.split_last() {
                    let key = IdentityFunctionKey {
                        owner: IdentityOwner::Module(module.to_vec()),
                        name: function.clone(),
                    };
                    if facts.functions.contains(&key) {
                        local.push(key);
                    }
                    let type_label = module.join("::");
                    if facts.local_types.contains(&type_label) {
                        if let Some(associated) = facts
                            .impl_functions
                            .get(&(type_label.clone(), function.clone()))
                        {
                            local.extend(associated.iter().cloned());
                        }
                        generated |= facts
                            .generated_methods
                            .contains(&(type_label, function.clone()));
                    }
                }
                let first_visible = compile_unit_root(&path).len().saturating_add(1);
                local_evidence |= (first_visible..=path.len()).any(|length| {
                    facts.has_local_path_prefix(&path[..length])
                        || facts
                            .local_types
                            .contains(&path[..length].join("::"))
                });
            }
        }
    }
    local.sort();
    local.dedup();
    constructors.sort();
    constructors.dedup();
    match (local.as_slice(), constructors.as_slice()) {
        ([function], []) => return RustCallResolution::Local(function.clone()),
        ([], [constructor]) => {
            return RustCallResolution::LocalConstructor(constructor.clone());
        }
        ([], []) => {}
        _ => {
            return RustCallResolution::Ambiguous(format!(
                "call {} resolved to {} functions and {} constructors",
                path.to_token_stream(),
                local.len(),
                constructors.len()
            ));
        }
    }
    if generated {
        return RustCallResolution::GeneratedByTrackedDerive;
    }
    if external {
        return RustCallResolution::ProvenExternal;
    }
    if local_evidence
        || matches!(segments.first().map(String::as_str), Some("crate" | "self" | "super"))
    {
        RustCallResolution::UnsupportedLocal(format!(
            "unresolved local call {}",
            path.to_token_stream()
        ))
    } else if is_rust_prelude_call(&segments) {
        RustCallResolution::ProvenExternal
    } else {
        RustCallResolution::UnsupportedLocal(format!(
            "unproven external call {}",
            path.to_token_stream()
        ))
    }
}

fn resolve_function_path(
    path: &syn::Path,
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Option<IdentityFunctionKey> {
    match resolve_call_path(path, owner, facts) {
        RustCallResolution::Local(key) => Some(key),
        _ => None,
    }
}

fn resolve_method_call(
    call: &syn::ExprMethodCall,
    owner: &IdentityOwner,
    types: &[HashMap<String, TrackedIdentityType>],
    facts: &IdentityTypeFacts,
) -> RustCallResolution {
    if matches!(call.receiver.as_ref(), Expr::Path(path) if path.path.is_ident("self")) {
        let key = IdentityFunctionKey {
            owner: owner.clone(),
            name: call.method.to_string(),
        };
        if matches!(owner, IdentityOwner::Impl { .. }) && facts.functions.contains(&key) {
            return RustCallResolution::Local(key);
        }
    }
    let receiver_type = expression_tracked_type(&call.receiver, owner, types, facts);
    if receiver_type.as_deref() == Some("")
        || receiver_type
            .as_deref()
            .is_some_and(|kind| kind.starts_with("$external$::") || kind.starts_with("$rust$::"))
    {
        return RustCallResolution::ProvenExternal;
    }
    let candidates = receiver_type
        .as_ref()
        .and_then(|receiver| {
            facts
                .impl_functions
                .get(&(receiver.clone(), call.method.to_string()))
        })
        .cloned()
        .unwrap_or_default();
    let mut inherent = candidates
        .iter()
        .filter(|candidate| {
            matches!(candidate.owner, IdentityOwner::Impl { trait_name: None, .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut trait_methods = candidates
        .into_iter()
        .filter(|candidate| {
            matches!(candidate.owner, IdentityOwner::Impl { trait_name: Some(_), .. })
        })
        .collect::<Vec<_>>();
    inherent.sort();
    inherent.dedup();
    trait_methods.sort();
    trait_methods.dedup();
    if let [candidate] = inherent.as_slice() {
        return RustCallResolution::Local(candidate.clone());
    }
    if inherent.len() > 1 {
        return RustCallResolution::Ambiguous(format!(
            "method {} resolved to {} inherent impl items",
            call.method,
            inherent.len()
        ));
    }
    if let Some(receiver) = receiver_type.as_ref()
        && facts
            .generated_methods
            .contains(&(receiver.clone(), call.method.to_string()))
    {
        return RustCallResolution::GeneratedByTrackedDerive;
    }
    match trait_methods.as_slice() {
        [candidate] => RustCallResolution::Local(candidate.clone()),
        [] => {
            let same_name_local = facts.unit_impl_method_names.contains(&(
                compile_unit_root(owner.module()).to_vec(),
                call.method.to_string(),
            ));
            if receiver_type
                .as_ref()
                .is_some_and(|kind| facts.local_types.contains(kind))
            {
                RustCallResolution::UnsupportedLocal(format!(
                    "unresolved local method {} for receiver {}",
                    call.method,
                    receiver_type.unwrap_or_default()
                ))
            } else if same_name_local {
                RustCallResolution::Ambiguous(format!(
                    "method {} has local candidates but receiver type is {}",
                    call.method,
                    receiver_type.as_deref().unwrap_or("unresolved")
                ))
            } else {
                RustCallResolution::UnsupportedLocal(format!(
                    "method {} has unproven receiver type {}",
                    call.method,
                    receiver_type.as_deref().unwrap_or("unresolved")
                ))
            }
        }
        _ => RustCallResolution::Ambiguous(format!(
            "method {} resolved to {} local trait impl items",
            call.method,
            trait_methods.len()
        )),
    }
}

fn resolve_method_key(
    call: &syn::ExprMethodCall,
    owner: &IdentityOwner,
    types: &[HashMap<String, TrackedIdentityType>],
    facts: &IdentityTypeFacts,
) -> Option<IdentityFunctionKey> {
    match resolve_method_call(call, owner, types, facts) {
        RustCallResolution::Local(key) => Some(key),
        _ => None,
    }
}

fn is_rust_prelude_call(segments: &[String]) -> bool {
    matches!(segments, [name] if matches!(name.as_str(), "drop" | "Some" | "None" | "Ok" | "Err"))
        || matches!(segments, [owner, _] if is_rust_prelude_type(owner))
}

fn is_builtin_rust_macro_path(path: &syn::Path) -> bool {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    matches!(
        segments.as_slice(),
        [name]
            if matches!(
                name.as_str(),
                "assert"
                    | "assert_eq"
                    | "assert_ne"
                    | "cfg"
                    | "column"
                    | "compile_error"
                    | "concat"
                    | "dbg"
                    | "debug_assert"
                    | "debug_assert_eq"
                    | "debug_assert_ne"
                    | "eprint"
                    | "eprintln"
                    | "env"
                    | "file"
                    | "format"
                    | "format_args"
                    | "include"
                    | "include_bytes"
                    | "include_str"
                    | "line"
                    | "matches"
                    | "module_path"
                    | "option_env"
                    | "panic"
                    | "print"
                    | "println"
                    | "stringify"
                    | "todo"
                    | "unimplemented"
                    | "unreachable"
                    | "vec"
                    | "write"
                    | "writeln"
            )
    )
}

fn infer_function_return_strings(
    file: &syn::File,
) -> Result<HashMap<IdentityFunctionKey, Vec<String>>, String> {
    let type_facts = identity_type_facts(file)?;
    let mut functions = Vec::new();
    collect_identity_functions(&file.items, &[], &type_facts, &mut functions);
    infer_function_return_strings_for_functions(&functions, &type_facts)
}

fn infer_function_return_strings_for_functions(
    functions: &[IdentityFunction<'_>],
    type_facts: &IdentityTypeFacts,
) -> Result<HashMap<IdentityFunctionKey, Vec<String>>, String> {
    let mut returns = HashMap::<IdentityFunctionKey, Vec<String>>::new();
    for _ in 0..functions.len().saturating_add(1) {
        let mut changed = false;
        for function in functions {
            let mut values =
                return_strings_for_block(
                    function.inputs,
                    function.block,
                    &function.key.owner,
                    &returns,
                    &type_facts,
                );
            values.sort();
            values.dedup();
            let entry = returns.entry(function.key.clone()).or_default();
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
    Ok(returns)
}

fn return_strings_for_block(
    inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
    block: &syn::Block,
    owner: &IdentityOwner,
    inferred_returns: &HashMap<IdentityFunctionKey, Vec<String>>,
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    struct ReturnStringVisitor<'a> {
        owner: &'a IdentityOwner,
        inferred_returns: &'a HashMap<IdentityFunctionKey, Vec<String>>,
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
                self.owner,
                self.inferred_returns,
                &self.types,
                self.type_facts,
            ));
            self.types.pop();
        }

        fn visit_local(&mut self, node: &syn::Local) {
            if let Some((name, declared_type)) = local_binding(&node.pat)
                && let Some(initializer) = &node.init
                && let Some(kind) = declared_type
                    .and_then(|kind| {
                        tracked_identity_type_with_facts(kind, self.owner, self.type_facts)
                    })
                    .or_else(|| {
                        expression_tracked_type(
                            &initializer.expr,
                            self.owner,
                            &self.types,
                            self.type_facts,
                        )
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
                    self.owner,
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
            tracked_identity_type_with_facts(&input.ty, owner, type_facts)
                .map(|kind| (pattern.ident.to_string(), kind))
        })
        .collect::<HashMap<_, _>>();
    let mut visitor = ReturnStringVisitor {
        owner,
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
    owner: &'a IdentityOwner,
    type_facts: &'a IdentityTypeFacts,
    calls: Vec<(IdentityFunctionKey, usize, &'static str)>,
    errors: Vec<String>,
}

impl<'a> IdentityCallPropagation<'a> {
    fn new(
        inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
        parameters: &[Option<&'static str>],
        owner: &'a IdentityOwner,
        type_facts: &'a IdentityTypeFacts,
    ) -> Self {
        let mut aliases = HashMap::new();
        let mut non_identity = HashSet::new();
        let mut types = HashMap::new();
        let mut typed_index = 0;
        for input in inputs {
            let FnArg::Typed(input) = input else {
                if let IdentityOwner::Impl { self_ty, .. } = owner {
                    types.insert("self".to_string(), self_ty.clone());
                }
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
            if let Some(kind) = tracked_identity_type_with_facts(&input.ty, owner, type_facts) {
                types.insert(pattern.ident.to_string(), kind);
            }
        }
        Self {
            aliases: vec![aliases],
            non_identity: vec![non_identity],
            types: vec![types],
            owner,
            type_facts,
            calls: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn axis(&self, expression: &Expr) -> Option<&'static str> {
        identity_axis(
            expression,
            &self.aliases,
            &self.non_identity,
            &self.types,
            self.owner,
            self.type_facts,
        )
    }

    fn record_call<'expr>(
        &mut self,
        callee: IdentityFunctionKey,
        arguments: impl Iterator<Item = &'expr Expr>,
    ) {
        for (index, argument) in arguments.enumerate() {
            if let Some(axis) = self.axis(argument) {
                self.calls.push((callee.clone(), index, axis));
            }
        }
    }

    fn arguments_carry_identity<'expr>(
        &self,
        arguments: impl Iterator<Item = &'expr Expr>,
    ) -> bool {
        arguments.into_iter().any(|argument| self.axis(argument).is_some())
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
            let inferred_type = declared_type
                .and_then(|kind| {
                    tracked_identity_type_with_facts(kind, self.owner, self.type_facts)
                })
                .or_else(|| {
                    expression_tracked_type(
                        &initializer.expr,
                        self.owner,
                        &self.types,
                        self.type_facts,
                    )
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
        if let Expr::Path(path) = node.func.as_ref() {
            match resolve_call_path(&path.path, self.owner, self.type_facts) {
                RustCallResolution::Local(callee) => {
                    self.record_call(callee, node.args.iter());
                }
                RustCallResolution::Ambiguous(error)
                | RustCallResolution::UnsupportedLocal(error)
                    if self.arguments_carry_identity(node.args.iter()) =>
                {
                    self.errors.push(error);
                }
                RustCallResolution::LocalConstructor(_)
                | RustCallResolution::ProvenExternal
                | RustCallResolution::GeneratedByTrackedDerive
                | RustCallResolution::Ambiguous(_)
                | RustCallResolution::UnsupportedLocal(_) => {}
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &syn::ExprMethodCall) {
        match resolve_method_call(node, self.owner, &self.types, self.type_facts) {
            RustCallResolution::Local(callee) => {
                self.record_call(callee, node.args.iter());
            }
            RustCallResolution::Ambiguous(error)
            | RustCallResolution::UnsupportedLocal(error)
                if self.axis(&node.receiver).is_some()
                    || self.arguments_carry_identity(node.args.iter()) =>
            {
                self.errors.push(error);
            }
            RustCallResolution::LocalConstructor(_)
            | RustCallResolution::ProvenExternal
            | RustCallResolution::GeneratedByTrackedDerive
            | RustCallResolution::Ambiguous(_)
            | RustCallResolution::UnsupportedLocal(_) => {}
        }
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
    owner: IdentityOwner,
    inferred_parameters: HashMap<IdentityFunctionKey, Vec<Option<&'static str>>>,
    inferred_returns: HashMap<IdentityFunctionKey, Vec<String>>,
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
            let inferred_type = declared_type
                .and_then(|kind| {
                    tracked_identity_type_with_facts(kind, &self.owner, &self.type_facts)
                })
                .or_else(|| {
                    expression_tracked_type(
                        &initializer.expr,
                        &self.owner,
                        &self.types,
                        &self.type_facts,
                    )
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
                &self.owner,
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
        let key = IdentityFunctionKey {
            owner: self.owner.clone(),
            name: node.sig.ident.to_string(),
        };
        self.push_parameter_scope(&key, &node.sig.inputs);
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
        let key = IdentityFunctionKey {
            owner: self.owner.clone(),
            name: node.sig.ident.to_string(),
        };
        self.push_parameter_scope(&key, &node.sig.inputs);
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
            &self.owner,
            &self.inferred_returns,
            &self.types,
            &self.type_facts,
        )
    }

    fn block_strings(&self, block: &syn::Block) -> Vec<String> {
        block_tail_strings_with_returns(
            block,
            &self.owner,
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
            &self.owner,
            &self.type_facts,
        )
    }

    fn push_parameter_scope(
        &mut self,
        function_key: &IdentityFunctionKey,
        inputs: &syn::punctuated::Punctuated<FnArg, Token![,]>,
    ) {
        let mut aliases = HashMap::new();
        let mut non_identity = HashSet::new();
        let mut types = HashMap::new();
        let inferred = self.inferred_parameters.get(function_key);
        let mut typed_index = 0;
        for input in inputs {
            let FnArg::Typed(input) = input else {
                if let IdentityOwner::Impl { self_ty, .. } = &self.owner {
                    types.insert("self".to_string(), self_ty.clone());
                }
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
            if let Some(kind) = tracked_identity_type_with_facts(
                &input.ty,
                &self.owner,
                &self.type_facts,
            ) {
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
    owner: &IdentityOwner,
    types: &[HashMap<String, TrackedIdentityType>],
    facts: &IdentityTypeFacts,
) -> Option<TrackedIdentityType> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(_) => Some(String::new()),
            Lit::Bool(_) => Some("$rust$::bool".to_string()),
            Lit::Char(_) => Some("$rust$::char".to_string()),
            Lit::Byte(_) => Some("$rust$::u8".to_string()),
            Lit::ByteStr(_) | Lit::CStr(_) => Some("$rust$::bytes".to_string()),
            Lit::Int(value) => Some(format!(
                "$rust$::{}",
                if value.suffix().is_empty() { "i32" } else { value.suffix() }
            )),
            Lit::Float(value) => Some(format!(
                "$rust$::{}",
                if value.suffix().is_empty() { "f64" } else { value.suffix() }
            )),
            _ => None,
        },
        Expr::Path(path) => {
            let segments = path.path.segments.iter().collect::<Vec<_>>();
            let last = segments.last()?;
            tracked_type_for_name(&last.ident.to_string(), types).or_else(|| {
                resolve_type_path_with_facts(&path.path, owner, facts)
            })
        }
        Expr::Struct(expression) => {
            resolve_type_path_with_facts(&expression.path, owner, facts)
        }
        Expr::Field(field) => {
            let field_owner = expression_tracked_type(&field.base, owner, types, facts)?;
            if field_owner.is_empty() {
                return None;
            }
            if field_owner.starts_with("$external$::")
                || field_owner.starts_with("$rust$::")
            {
                return Some("$external$::opaque".to_string());
            }
            let Member::Named(field) = &field.member else {
                return None;
            };
            facts
                .struct_fields
                .get(&(field_owner, field.to_string()))
                .cloned()
        }
        Expr::Call(call) => {
            let Expr::Path(path) = call.func.as_ref() else {
                return None;
            };
            match resolve_call_path(&path.path, owner, facts) {
                RustCallResolution::Local(key) => facts.function_returns.get(&key).cloned(),
                RustCallResolution::LocalConstructor(kind) => Some(kind),
                RustCallResolution::ProvenExternal => {
                    Some("$external$::opaque".to_string())
                }
                RustCallResolution::GeneratedByTrackedDerive
                | RustCallResolution::Ambiguous(_)
                | RustCallResolution::UnsupportedLocal(_) => None,
            }
        }
        Expr::MethodCall(call)
            if identity_passthrough_method(&call.method.to_string())
                && expression_tracked_type(&call.receiver, owner, types, facts)
                    == Some(String::new()) =>
        {
            Some(String::new())
        }
        Expr::MethodCall(call) => match resolve_method_call(call, owner, types, facts) {
            RustCallResolution::Local(key) => facts.function_returns.get(&key).cloned(),
            RustCallResolution::LocalConstructor(kind) => Some(kind),
            RustCallResolution::ProvenExternal => {
                Some("$external$::opaque".to_string())
            }
            RustCallResolution::GeneratedByTrackedDerive => {
                expression_tracked_type(&call.receiver, owner, types, facts)
            }
            RustCallResolution::Ambiguous(_)
            | RustCallResolution::UnsupportedLocal(_) => None,
        },
        Expr::Array(_) | Expr::Repeat(_) => Some("$rust$::array".to_string()),
        Expr::Tuple(_) => Some("$rust$::tuple".to_string()),
        Expr::Closure(_) => Some("$rust$::closure".to_string()),
        Expr::Range(_) => Some("$rust$::range".to_string()),
        Expr::Macro(expression)
            if is_builtin_rust_macro_path(&expression.mac.path) =>
        {
            Some("$external$::macro-output".to_string())
        }
        Expr::Unary(expression) => {
            expression_tracked_type(&expression.expr, owner, types, facts)
        }
        Expr::Binary(expression) => {
            if matches!(
                expression.op,
                BinOp::Eq(_)
                    | BinOp::Lt(_)
                    | BinOp::Le(_)
                    | BinOp::Ne(_)
                    | BinOp::Ge(_)
                    | BinOp::Gt(_)
                    | BinOp::And(_)
                    | BinOp::Or(_)
            ) {
                Some("$rust$::bool".to_string())
            } else {
                expression_tracked_type(&expression.left, owner, types, facts)
            }
        }
        Expr::Index(expression) => {
            let base = expression_tracked_type(&expression.expr, owner, types, facts)?;
            (base.starts_with("$external$::") || base.starts_with("$rust$::"))
                .then(|| "$external$::opaque".to_string())
        }
        Expr::Paren(expression) => expression_tracked_type(&expression.expr, owner, types, facts),
        Expr::Group(expression) => expression_tracked_type(&expression.expr, owner, types, facts),
        Expr::Reference(expression) => {
            expression_tracked_type(&expression.expr, owner, types, facts)
        }
        Expr::Try(expression) => expression_tracked_type(&expression.expr, owner, types, facts),
        Expr::Await(expression) => expression_tracked_type(&expression.base, owner, types, facts),
        Expr::Cast(expression) => {
            tracked_identity_type_with_facts(&expression.ty, owner, facts)
        }
        _ => None,
    }
}

fn identity_axis(
    expression: &Expr,
    aliases: &[HashMap<String, &'static str>],
    non_identity: &[HashSet<String>],
    types: &[HashMap<String, TrackedIdentityType>],
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Option<&'static str> {
    match expression {
        Expr::Field(field) => {
            if expression_tracked_type(expression, owner, types, facts)
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
        Expr::Paren(paren) => identity_axis(&paren.expr, aliases, non_identity, types, owner, facts),
        Expr::Group(group) => identity_axis(&group.expr, aliases, non_identity, types, owner, facts),
        Expr::Reference(reference) => {
            identity_axis(&reference.expr, aliases, non_identity, types, owner, facts)
        }
        Expr::Try(expression) => {
            identity_axis(&expression.expr, aliases, non_identity, types, owner, facts)
        }
        Expr::Await(expression) => {
            identity_axis(&expression.base, aliases, non_identity, types, owner, facts)
        }
        Expr::Cast(expression) => {
            identity_axis(&expression.expr, aliases, non_identity, types, owner, facts)
        }
        Expr::Unary(expression) => {
            identity_axis(&expression.expr, aliases, non_identity, types, owner, facts)
        }
        Expr::MethodCall(call)
            if expression_tracked_type(&call.receiver, owner, types, facts) == Some(String::new())
                && identity_passthrough_method(&call.method.to_string()) =>
        {
            identity_axis(&call.receiver, aliases, non_identity, types, owner, facts)
        }
        Expr::MethodCall(_) => None,
        Expr::Call(call)
            if expression_tracked_type(expression, owner, types, facts) == Some(String::new()) =>
        {
            call.args
                .iter()
                .find_map(|argument| {
                    identity_axis(argument, aliases, non_identity, types, owner, facts)
                })
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
    owner: &IdentityOwner,
    facts: &IdentityTypeFacts,
) -> Vec<&'static str> {
    struct AxisUseVisitor<'a> {
        aliases: &'a [HashMap<String, &'static str>],
        non_identity: &'a [HashSet<String>],
        types: &'a [HashMap<String, TrackedIdentityType>],
        owner: &'a IdentityOwner,
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
                self.owner,
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
        owner,
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
    owner: &IdentityOwner,
    inferred_returns: &HashMap<IdentityFunctionKey, Vec<String>>,
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
                returned_strings_with_returns(item, owner, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Tuple(tuple) => tuple
            .elems
            .iter()
            .flat_map(|item| {
                returned_strings_with_returns(item, owner, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Paren(paren) => {
            returned_strings_with_returns(&paren.expr, owner, inferred_returns, types, type_facts)
        }
        Expr::Group(group) => {
            returned_strings_with_returns(&group.expr, owner, inferred_returns, types, type_facts)
        }
        Expr::Reference(reference) => {
            returned_strings_with_returns(
                &reference.expr,
                owner,
                inferred_returns,
                types,
                type_facts,
            )
        }
        Expr::Try(expression) => {
            returned_strings_with_returns(
                &expression.expr,
                owner,
                inferred_returns,
                types,
                type_facts,
            )
        }
        Expr::Await(expression) => {
            returned_strings_with_returns(
                &expression.base,
                owner,
                inferred_returns,
                types,
                type_facts,
            )
        }
        Expr::Call(call) => {
            let path = if let Expr::Path(path) = call.func.as_ref() {
                Some(&path.path)
            } else {
                None
            };
            if let Some(values) = path
                .and_then(|path| resolve_function_path(path, owner, type_facts))
                .and_then(|callee| inferred_returns.get(&callee))
            {
                values.clone()
            } else if path.is_some_and(language_string_wrapper)
            {
                call.args
                    .iter()
                    .flat_map(|argument| {
                        returned_strings_with_returns(
                            argument,
                            owner,
                            inferred_returns,
                            types,
                            type_facts,
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        Expr::MethodCall(call) => {
            let method = call.method.to_string();
            if let Some(values) = resolve_method_key(call, owner, types, type_facts)
                .and_then(|callee| inferred_returns.get(&callee))
            {
                values.clone()
            } else if identity_passthrough_method(&method)
                && expression_tracked_type(&call.receiver, owner, types, type_facts)
                    == Some(String::new())
            {
                let mut values = returned_strings_with_returns(
                    &call.receiver,
                    owner,
                    inferred_returns,
                    types,
                    type_facts,
                );
                if matches!(method.as_str(), "unwrap_or" | "unwrap_or_else") {
                    values.extend(call.args.iter().flat_map(|argument| {
                        returned_strings_with_returns(
                            argument,
                            owner,
                            inferred_returns,
                            types,
                            type_facts,
                        )
                    }));
                }
                values
            } else {
                Vec::new()
            }
        }
        Expr::Block(block) => {
            block_return_strings_with_returns(
                &block.block,
                owner,
                inferred_returns,
                types,
                type_facts,
            )
        }
        Expr::If(expression) => {
            let mut values = block_return_strings_with_returns(
                &expression.then_branch,
                owner,
                inferred_returns,
                types,
                type_facts,
            );
            if let Some((_, otherwise)) = &expression.else_branch {
                values.extend(returned_strings_with_returns(
                    otherwise,
                    owner,
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
                returned_strings_with_returns(
                    &arm.body,
                    owner,
                    inferred_returns,
                    types,
                    type_facts,
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn block_return_strings_with_returns(
    block: &syn::Block,
    owner: &IdentityOwner,
    inferred_returns: &HashMap<IdentityFunctionKey, Vec<String>>,
    types: &[HashMap<String, TrackedIdentityType>],
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    block
        .stmts
        .last()
        .and_then(|statement| match statement {
            syn::Stmt::Expr(expression, None) => Some(returned_strings_with_returns(
                expression,
                owner,
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
    owner: &IdentityOwner,
    inferred_returns: &HashMap<IdentityFunctionKey, Vec<String>>,
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
                expression_strings_with_returns(item, owner, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Tuple(tuple) => tuple
            .elems
            .iter()
            .flat_map(|item| {
                expression_strings_with_returns(item, owner, inferred_returns, types, type_facts)
            })
            .collect(),
        Expr::Paren(paren) => {
            expression_strings_with_returns(&paren.expr, owner, inferred_returns, types, type_facts)
        }
        Expr::Group(group) => {
            expression_strings_with_returns(&group.expr, owner, inferred_returns, types, type_facts)
        }
        Expr::Reference(reference) => {
            expression_strings_with_returns(
                &reference.expr,
                owner,
                inferred_returns,
                types,
                type_facts,
            )
        }
        Expr::Call(call) => {
            let known = if let Expr::Path(path) = call.func.as_ref() {
                resolve_function_path(&path.path, owner, type_facts)
                    .and_then(|callee| inferred_returns.get(&callee))
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
                            owner,
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
            if let Some(values) = resolve_method_key(call, owner, types, type_facts)
                .and_then(|callee| inferred_returns.get(&callee))
            {
                values.clone()
            } else if identity_passthrough_method(&method)
                && expression_tracked_type(&call.receiver, owner, types, type_facts)
                    == Some(String::new())
            {
                let mut values = expression_strings_with_returns(
                    &call.receiver,
                    owner,
                    inferred_returns,
                    types,
                    type_facts,
                );
                if matches!(method.as_str(), "unwrap_or" | "unwrap_or_else") {
                    values.extend(call.args.iter().flat_map(|argument| {
                        expression_strings_with_returns(
                            argument,
                            owner,
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
                            owner,
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
            block_tail_strings_with_returns(
                &block.block,
                owner,
                inferred_returns,
                types,
                type_facts,
            )
        }
        Expr::If(expression) => {
            let mut values = block_tail_strings_with_returns(
                &expression.then_branch,
                owner,
                inferred_returns,
                types,
                type_facts,
            );
            if let Some((_, otherwise)) = &expression.else_branch {
                values.extend(expression_strings_with_returns(
                    otherwise,
                    owner,
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
                expression_strings_with_returns(
                    &arm.body,
                    owner,
                    inferred_returns,
                    types,
                    type_facts,
                )
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
    owner: &IdentityOwner,
    inferred_returns: &HashMap<IdentityFunctionKey, Vec<String>>,
    types: &[HashMap<String, TrackedIdentityType>],
    type_facts: &IdentityTypeFacts,
) -> Vec<String> {
    block
        .stmts
        .last()
        .and_then(|statement| match statement {
            syn::Stmt::Expr(expression, None) => Some(expression_strings_with_returns(
                expression,
                owner,
                inferred_returns,
                types,
                type_facts,
            )),
            _ => None,
        })
        .unwrap_or_default()
}

fn language_string_wrapper(path: &syn::Path) -> bool {
    path.leading_colon.is_none()
        && path.segments.len() == 1
        && path.segments.first().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "Some" | "Ok" | "Borrowed" | "Owned"
            )
        })
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
        let mut surface = ProtectedSurface {
            surface_id: surface_id.clone(),
            kind: "rust_public_item".to_string(),
            semantic_role: SemanticRole::Contract,
            stable_path: "crates/example/src/lib.rs".to_string(),
            selector: "struct:Summary".to_string(),
            concept_ids: vec![
                "identity.game".to_string(),
                "interface.contract".to_string(),
                "structure.value".to_string(),
            ],
            fingerprint: "0".repeat(64),
            mapping_source_id: String::new(),
        };
        let mut mapping_source = MappingSource {
            id: String::new(),
            task_issue: 75,
            implementation_pr: 137,
            source_kind: "workflow_task".to_string(),
            change_kind: "initial_import".to_string(),
        };
        mapping_source.id = mapping_source_id(&mapping_source, &[&surface]);
        surface.mapping_source_id = mapping_source.id.clone();
        format!(
            r#"
schema_version = "actingcommand.generic-domain.v1"

[[concept]]
id = "identity.game"
status = "active"
approval_comment_id = 5010683904

[[concept]]
id = "interface.contract"
status = "active"
approval_comment_id = 5010683904

[[concept]]
id = "structure.value"
status = "active"
approval_comment_id = 5010683904

[[mapping_source]]
id = "{}"
task_issue = 75
implementation_pr = 137
source_kind = "workflow_task"
change_kind = "initial_import"

[[surface]]
surface_id = "{surface_id}"
kind = "rust_public_item"
semantic_role = "contract"
stable_path = "crates/example/src/lib.rs"
selector = "struct:Summary"
concept_ids = ["identity.game", "interface.contract", "structure.value"]
fingerprint = "{}"
mapping_source_id = "{}"
"#,
            mapping_source.id,
            surface.fingerprint,
            surface.mapping_source_id,
        )
    }

    #[test]
    fn registry_accepts_sorted_approved_concepts_and_exact_surface() {
        let registry = parse_generic_domain_registry(&registry_source()).unwrap();
        validate_generic_domain_registry(&registry).unwrap();
    }

    #[test]
    fn registry_rejects_external_or_hybrid_surface_tables() {
        let hybrid = registry_source().replace(
            "schema_version = \"actingcommand.generic-domain.v1\"",
            "schema_version = \"actingcommand.generic-domain.v1\"\n\n[surface_manifest]\npath = \"generic-domain-extra.jsonl\"\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"",
        );
        let error = parse_generic_domain_registry(&hybrid).unwrap_err();
        assert!(error.contains("unknown field `surface_manifest`"));

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "actingcommand-generic-domain-single-registry-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let registry_path = directory.join("generic-domain-v1.toml");
        fs::write(&registry_path, registry_source()).unwrap();
        fs::write(directory.join("generic-domain-extra.jsonl"), "{}\n").unwrap();
        let error = load_generic_domain_registry(&registry_path).unwrap_err();
        assert!(error.contains("external registry"));
        assert!(error.contains("must be inline"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn registry_rejects_unknown_concept_duplicate_and_wildcard_surface() {
        let source = registry_source()
            .replace(
                "concept_ids = [\"identity.game\", \"interface.contract\", \"structure.value\"]",
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
    fn registry_enforces_closed_roles_applicability_and_non_catch_all_mappings() {
        let mut registry = parse_generic_domain_registry(&registry_source()).unwrap();
        registry.surface[0].semantic_role = SemanticRole::Wire;
        rebind_mapping_source(&mut registry);
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("role wire is incompatible with kind rust_public_item"));

        let mut registry = parse_generic_domain_registry(&registry_source()).unwrap();
        registry.surface[0].concept_ids = vec!["structure.value".to_string()];
        rebind_mapping_source(&mut registry);
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("requires anchor concept interface.contract"));

        let snapshot = SurfaceSnapshot {
            surface_id: surface_id_for("schema_key", "contracts/example.json", "key:/properties"),
            kind: "schema_key".to_string(),
            semantic_role: SemanticRole::Schema,
            stable_path: "contracts/example.json".to_string(),
            selector: "key:/properties".to_string(),
            fingerprint: "a".repeat(64),
        };
        let mut registry = registry_for_snapshots(&[snapshot]);
        registry.concept.insert(
            0,
            GenericConcept {
                id: "agent.agent".to_string(),
                status: "active".to_string(),
                approval_comment_id: INITIAL_CONCEPT_APPROVAL_COMMENT_ID,
                replaced_by: None,
            },
        );
        registry.surface[0]
            .concept_ids
            .insert(0, "agent.agent".to_string());
        rebind_mapping_source(&mut registry);
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("role schema cannot map concept agent.agent"));
    }

    #[test]
    fn registry_requires_exact_nonzero_mapping_sources_and_rejects_reuse() {
        let mut registry = parse_generic_domain_registry(&registry_source()).unwrap();
        registry.mapping_source[0].task_issue = 0;
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("must bind nonzero task_issue and implementation_pr"));

        let mut registry = parse_generic_domain_registry(&registry_source()).unwrap();
        registry.mapping_source[0].source_kind = "candidate_claim".to_string();
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("invalid source_kind candidate_claim"));

        let mut registry = parse_generic_domain_registry(&registry_source()).unwrap();
        registry.surface[0].mapping_source_id = "mapping_source.missing".to_string();
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("references missing mapping source"));

        let mut registry = parse_generic_domain_registry(&registry_source()).unwrap();
        let mut added = registry.surface[0].clone();
        added.selector = "struct:Added".to_string();
        added.surface_id = surface_id_for(&added.kind, &added.stable_path, &added.selector);
        registry.surface.push(added);
        registry
            .surface
            .sort_by(|left, right| left.surface_id.cmp(&right.surface_id));
        let error = validate_generic_domain_registry(&registry).unwrap_err();
        assert!(error.contains("is not content-bound"));
    }

    #[test]
    fn registry_accepts_all_nine_closed_semantic_roles_with_exact_anchors() {
        let kinds = [
            ("rust_public_item", SemanticRole::Contract),
            ("rust_wire_item", SemanticRole::Wire),
            ("schema_key", SemanticRole::Schema),
            ("rust_cli_attribute", SemanticRole::Cli),
            ("rust_default_impl", SemanticRole::Default),
            ("rust_template_carrier", SemanticRole::Template),
            ("rust_task_definition_carrier", SemanticRole::TaskDefinition),
            ("rust_identity_branch_carrier", SemanticRole::IdentityBranch),
            (
                "rust_test_fixture_carrier",
                SemanticRole::TestFixtureGolden,
            ),
        ];
        let snapshots = kinds
            .into_iter()
            .enumerate()
            .map(|(index, (kind, semantic_role))| {
                let selector = format!("role:{index}");
                SurfaceSnapshot {
                    surface_id: surface_id_for(kind, "crates/example/src/lib.rs", &selector),
                    kind: kind.to_string(),
                    semantic_role,
                    stable_path: "crates/example/src/lib.rs".to_string(),
                    selector,
                    fingerprint: format!("{index:064x}"),
                }
            })
            .collect::<Vec<_>>();
        validate_generic_domain_registry(&registry_for_snapshots(&snapshots)).unwrap();
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
    fn identity_flow_uses_qualified_module_owners_without_same_name_pollution() {
        let source = r#"
            mod accepted {
                pub fn helper(candidate: &str) -> bool {
                    candidate == "synthetic_project_code"
                }
            }
            mod neutral {
                pub fn helper(candidate: &str) -> bool {
                    candidate == "ordinary-neutral-value"
                }
                pub fn ordinary(value: &str) -> bool { helper(value) }
            }
            fn route(game: &str) -> bool { accepted::helper(game) }
        "#;
        let violations = inspect_identity_axis_branches("qualified-owner.rs", source).unwrap();
        assert!(violations.iter().any(|violation| {
            violation.contains("synthetic_project_code") && violation.contains("axis game")
        }));
        assert!(
            !violations
                .iter()
                .any(|violation| violation.contains("ordinary-neutral-value")),
            "{violations:#?}"
        );
    }

    #[test]
    fn identity_flow_resolves_same_name_methods_by_receiver_owner() {
        let source = r#"
            struct Accepted;
            struct Neutral;
            impl Accepted {
                fn helper(&self, candidate: &str) -> bool {
                    candidate == "synthetic_method_project"
                }
            }
            impl Neutral {
                fn helper(&self, candidate: &str) -> bool {
                    candidate == "ordinary-method-value"
                }
            }
            fn route(owner: &Accepted, game: &str) -> bool { owner.helper(game) }
            fn ordinary(owner: &Neutral, value: &str) -> bool { owner.helper(value) }
        "#;
        let violations = inspect_identity_axis_branches("qualified-method.rs", source).unwrap();
        assert!(violations.iter().any(|violation| {
            violation.contains("synthetic_method_project") && violation.contains("axis game")
        }));
        assert!(
            !violations
                .iter()
                .any(|violation| violation.contains("ordinary-method-value")),
            "{violations:#?}"
        );
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
    fn carrier_closure_tracks_private_defaults_cli_tasks_and_templates_transitively() {
        let source = r#"
            const TASKS: [&str; 1] = ["task.default"];
            static TEMPLATE: &str = "template.default";

            fn task_value() -> &'static str { TASKS[0] }
            fn intermediate() -> &'static str { task_value() }
            fn serde_default() -> String { TEMPLATE.to_owned() }

            #[derive(Deserialize)]
            struct WireRecord {
                #[serde(default = "serde_default")]
                value: String,
            }

            fn select(argument: &str) -> &'static str {
                if argument == "--task" { intermediate() } else { "none" }
            }
            fn main() { let _ = select("--task"); }

            fn unrelated_numeric(left: u64, right: u64) -> u64 { left + right }
        "#;
        let surfaces = rust_surface_inventory("crates/example/src/main.rs", source).unwrap();
        for expected in [
            "carrier:crate:fn:main",
            "carrier:crate:fn:select",
            "carrier:crate:fn:intermediate",
            "carrier:crate:fn:task_value",
            "carrier:crate:const:TASKS",
            "carrier:crate:fn:serde_default",
            "carrier:crate:static:TEMPLATE",
        ] {
            assert!(
                surfaces.iter().any(|surface| surface.selector == expected),
                "missing {expected}: {surfaces:#?}"
            );
        }
        assert!(surfaces.iter().any(|surface| {
            surface.kind == "rust_cli_carrier" && surface.selector.ends_with(":fn:select")
        }));
        assert!(surfaces.iter().any(|surface| {
            surface.kind == "rust_default_carrier"
                && surface.selector.ends_with(":fn:serde_default")
        }));
        assert!(surfaces.iter().any(|surface| {
            surface.kind == "rust_wire_carrier"
                && surface.selector.ends_with(":fn:serde_default")
        }));
        assert!(!surfaces.iter().any(|surface| {
            surface.selector.ends_with(":fn:unrelated_numeric")
        }));

        let changed = rust_surface_inventory(
            "crates/example/src/main.rs",
            &source.replace("template.default", "template.changed"),
        )
        .unwrap();
        let original = surfaces
            .iter()
            .find(|surface| surface.selector.ends_with(":static:TEMPLATE"))
            .unwrap();
        let changed = changed
            .iter()
            .find(|surface| {
                surface.kind == original.kind && surface.selector == original.selector
            })
            .unwrap();
        assert_ne!(original.content, changed.content);
    }

    #[test]
    fn carrier_closure_resolves_modules_and_impl_receivers_without_cross_pollution() {
        let source = r#"
            mod accepted {
                pub fn start() -> &'static str { helper() }
                fn helper() -> &'static str { "accepted" }
            }
            mod neutral {
                fn helper() -> &'static str { "neutral" }
            }
            struct Accepted;
            struct Neutral;
            impl Accepted {
                pub fn start(&self) -> &'static str { self.helper() }
                fn helper(&self) -> &'static str { "accepted-method" }
            }
            impl Neutral {
                fn helper(&self) -> &'static str { "neutral-method" }
            }
        "#;
        let surfaces = rust_surface_inventory("fixture.rs", source).unwrap();
        assert!(surfaces.iter().any(|surface| {
            surface.selector == "carrier:crate::accepted:fn:helper"
        }));
        assert!(!surfaces.iter().any(|surface| {
            surface.selector == "carrier:crate::neutral:fn:helper"
        }));
        assert!(surfaces.iter().any(|surface| {
            surface.selector.contains("impl:Accepted:inherent:fn:helper")
        }));
        assert!(!surfaces.iter().any(|surface| {
            surface.selector.contains("impl:Neutral:inherent:fn:helper")
        }));
    }

    #[test]
    fn carrier_closure_fails_closed_on_unresolved_local_edges() {
        let source = r#"
            mod local { fn available() {} }
            pub fn root() { local::missing(); }
        "#;
        let error = rust_surface_inventory("fixture.rs", source).unwrap_err();
        assert!(error.contains("unresolved local call local :: missing"), "{error}");
    }

    #[test]
    fn import_alias_resolution_rejects_cycles_without_path_growth() {
        for source in [
            r#"
                use crate::looped as looped;
                pub struct Root { pub value: looped::Value }
            "#,
            r#"
                use crate::second as first;
                use crate::first as second;
                pub struct Root { pub value: first::Value }
            "#,
            r#"
                mod facade {
                    pub use crate::alias::next as next;
                    pub struct Value;
                }
                use crate::facade as alias;
                pub struct Root { pub value: alias::next::Value }
            "#,
        ] {
            let error = rust_surface_inventory("fixture.rs", source).unwrap_err();
            assert!(error.contains("local import alias cycle"), "{error}");
        }
    }

    #[test]
    fn import_alias_resolution_accepts_bounded_transitive_aliases() {
        let surfaces = rust_surface_inventory(
            "fixture.rs",
            r#"
                mod backend { pub struct Value; }
                mod facade { pub use crate::backend as api; }
                use crate::facade::api as selected;
                pub struct Root { pub value: selected::Value }
            "#,
        )
        .unwrap();
        assert!(!surfaces.is_empty());
    }

    #[test]
    fn import_alias_resolution_is_independent_of_declaration_order() {
        for source in [
            r#"
                use platform::string::String as Text;
                use std as platform;
                pub struct Root { pub value: Text }
            "#,
            r#"
                use std as platform;
                use platform::string::String as Text;
                pub struct Root { pub value: Text }
            "#,
        ] {
            rust_surface_inventory("fixture.rs", source).unwrap();
        }
    }

    #[test]
    fn import_alias_resolution_preserves_module_value_namespace_shadowing() {
        let surfaces = rust_surface_inventory(
            "fixture.rs",
            r#"
                mod worker {
                    pub fn worker() {}
                    pub fn helper() {}
                }
                pub use worker::worker;
                pub fn root() {
                    worker::helper();
                    worker();
                }
            "#,
        )
        .unwrap();
        assert!(surfaces.iter().any(|surface| {
            surface.selector.ends_with(":fn:helper")
        }), "{surfaces:#?}");
        assert!(surfaces.iter().any(|surface| {
            surface.selector.ends_with(":fn:worker")
        }), "{surfaces:#?}");
    }

    #[test]
    fn import_resolution_prefers_lexical_modules_and_rejects_missing_bare_paths() {
        rust_surface_inventory(
            "fixture.rs",
            r#"
                mod event {
                    mod artifact { pub struct Value; }
                    use artifact::Value;
                    pub struct Root { pub value: Value }
                }
            "#,
        )
        .unwrap();

        let error = rust_surface_inventory(
            "fixture.rs",
            r#"
                mod event {
                    use missing::Value;
                    pub struct Root { pub value: Value }
                }
            "#,
        )
        .unwrap_err();
        assert!(error.contains("use path missing::Value"), "{error}");
        assert!(error.contains("neither a local compile-unit symbol"), "{error}");
    }

    #[test]
    fn glob_resolution_filters_candidates_by_actual_symbol_kind() {
        rust_surface_inventory(
            "fixture.rs",
            r#"
                mod parent {
                    pub struct Value;
                    mod child {
                        use super::*;
                        pub struct Root { pub value: Value }
                    }
                }
            "#,
        )
        .unwrap();

        let error = rust_surface_inventory(
            "fixture.rs",
            r#"
                mod left { pub struct Value; }
                mod right { pub struct Value; }
                mod consumer {
                    use crate::left::*;
                    use crate::right::*;
                    pub struct Root { pub value: Value }
                }
            "#,
        )
        .unwrap_err();
        assert!(error.contains("resolved to 2 local types"), "{error}");
    }

    #[test]
    fn carrier_closure_catalogs_local_trait_default_bodies() {
        let surfaces = rust_surface_inventory(
            "fixture.rs",
            r#"
                fn private_default() -> &'static str { "default" }
                pub trait Policy {
                    fn selected() -> &'static str { private_default() }
                }
            "#,
        )
        .unwrap();
        assert!(surfaces.iter().any(|surface| {
            surface.selector.ends_with(":fn:private_default")
        }), "{surfaces:#?}");
        assert!(surfaces.iter().any(|surface| {
            surface.selector.contains("Policy")
                && surface.selector.ends_with(":fn:selected")
        }), "{surfaces:#?}");
    }

    #[test]
    fn carrier_closure_treats_prelude_attribute_helpers_as_external() {
        rust_surface_inventory(
            "fixture.rs",
            r#"
                #[derive(serde::Serialize)]
                pub struct Wire {
                    #[serde(skip_serializing_if = "Option::is_none")]
                    pub optional: Option<String>,
                    #[serde(skip_serializing_if = "Vec::is_empty")]
                    pub values: Vec<String>,
                }
            "#,
        )
        .unwrap();
    }

    #[test]
    fn carrier_closure_covers_integration_and_cfg_test_roots() {
        let integration = rust_surface_inventory(
            "crates/example/tests/integration.rs",
            "fn private_fixture() -> &'static str { \"fixture\" }",
        )
        .unwrap();
        assert!(integration.iter().any(|surface| {
            surface.kind == "rust_test_fixture_carrier"
                && surface.selector.ends_with(":fn:private_fixture")
        }));

        let cfg_test = rust_surface_inventory(
            "crates/example/src/lib.rs",
            "#[cfg(test)] mod tests { fn private_fixture() -> &'static str { \"fixture\" } }",
        )
        .unwrap();
        assert!(cfg_test.iter().any(|surface| {
            surface.kind == "rust_test_fixture_carrier"
                && surface.selector == "carrier:crate::tests:fn:private_fixture"
        }));
    }

    #[test]
    fn workspace_carrier_graph_preserves_out_of_line_roles_includes_and_imports() {
        let root = temporary_workspace("carrier-workspace-graph");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            r#"
                mod backend;
                mod caller;
                #[cfg(test)] mod tests;
                include!("included.rs");

                pub fn public_root() -> &'static str {
                    caller::route()
                }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/backend.rs"),
            r#"
                pub struct RuntimeInputBackend(&'static str);
                pub struct NeutralBackend(&'static str);
                pub enum Endpoint { Named(&'static str) }

                impl RuntimeInputBackend {
                    pub(super) fn connect() -> Self { Self("protected") }
                    pub(super) fn value(&self) -> &'static str { self.0 }
                }
                impl NeutralBackend {
                    fn connect() -> Self { Self("neutral") }
                    fn value(&self) -> &'static str { self.0 }
                }

                pub(super) fn helper() -> &'static str {
                    let endpoint = Endpoint::Named("endpoint");
                    let backend = RuntimeInputBackend::connect();
                    match endpoint { Endpoint::Named(_) => backend.value() }
                }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/caller.rs"),
            r#"
                use super::backend::helper as selected;
                pub(super) fn route() -> &'static str { selected() }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/tests.rs"),
            "fn out_of_line_fixture() -> &'static str { \"fixture\" }\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/included.rs"),
            r#"
                pub fn included_root() -> &'static str { included_helper() }
                fn included_helper() -> &'static str { "included" }
            "#,
        )
        .unwrap();
        create_required_roots(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        for expected in [
            "fn:public_root",
            "fn:route",
            "fn:helper",
            "RuntimeInputBackend:inherent:fn:connect",
            "fn:included_helper",
        ] {
            assert!(
                snapshots
                    .iter()
                    .any(|surface| surface.selector.contains(expected)),
                "missing {expected}: {snapshots:#?}"
            );
        }
        assert!(snapshots.iter().any(|surface| {
            surface.stable_path == "crates/example/src/included.rs"
                && surface.selector.ends_with(":fn:included_helper")
        }));
        assert!(snapshots.iter().any(|surface| {
            surface.stable_path == "crates/example/src/tests.rs"
                && surface.semantic_role == SemanticRole::TestFixtureGolden
                && surface.selector.ends_with(":fn:out_of_line_fixture")
        }));
        assert!(!snapshots.iter().any(|surface| {
            surface.selector.contains("NeutralBackend")
                && surface.selector.ends_with(":fn:connect")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_carrier_graph_isolates_same_names_by_compile_unit() {
        let root = temporary_workspace("carrier-compile-units");
        write_workspace_manifest(&root, &["crates/alpha", "crates/beta"]);
        for (member, value) in [("alpha", "alpha"), ("beta", "beta")] {
            fs::create_dir_all(root.join(format!("crates/{member}/src"))).unwrap();
            fs::write(
                root.join(format!("crates/{member}/src/lib.rs")),
                format!(
                    "fn helper() -> &'static str {{ \"{value}\" }}\npub fn root() -> &'static str {{ crate::helper() }}\n"
                ),
            )
            .unwrap();
        }
        create_required_roots(&root);

        let snapshots = workspace_surface_snapshot(&root).unwrap();
        let helpers = snapshots
            .iter()
            .filter(|surface| surface.selector.ends_with(":fn:helper"))
            .collect::<Vec<_>>();
        assert_eq!(helpers.len(), 2, "{helpers:#?}");
        assert_ne!(helpers[0].selector, helpers[1].selector);
        assert_ne!(helpers[0].stable_path, helpers[1].stable_path);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_carrier_graph_rejects_include_cycles_and_unowned_sources() {
        let root = temporary_workspace("carrier-include-cycle");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            "include!(\"cycle.rs\");\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/cycle.rs"),
            "include!(\"cycle.rs\");\n",
        )
        .unwrap();
        create_required_roots(&root);
        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("static include cycle"), "{error}");
        fs::remove_dir_all(root).unwrap();

        let root = temporary_workspace("carrier-unowned-source");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(root.join("crates/example/src/lib.rs"), "pub struct Root;\n").unwrap();
        fs::write(
            root.join("crates/example/src/orphan.rs"),
            "pub struct Orphan;\n",
        )
        .unwrap();
        create_required_roots(&root);
        let error = workspace_surface_snapshot(&root).unwrap_err();
        assert!(error.contains("is not owned by any Cargo compile unit"), "{error}");
        assert!(error.contains("orphan.rs"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_identity_flow_reuses_compile_unit_types_imports_and_owners() {
        let root = temporary_workspace("identity-workspace-graph");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        fs::write(
            root.join("crates/example/src/lib.rs"),
            r#"
                mod backend;
                mod accepted;
                mod neutral;
                pub(crate) use backend::RuntimeInputBackend as SelectedBackend;
                pub fn route(game: &str) -> bool { accepted::route(game) }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/backend.rs"),
            r#"
                pub(crate) struct RuntimeInputBackend;
                impl RuntimeInputBackend {
                    pub(crate) fn select(candidate: &str) -> bool {
                        candidate == "synthetic_project_code"
                    }
                }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/accepted.rs"),
            r#"
                use super::SelectedBackend;
                struct Request { game: String }
                fn select_request(request: &Request) -> bool {
                    request.game == "synthetic_request_game"
                }
                pub(super) fn route(game: &str) -> bool {
                    SelectedBackend::select(game)
                }
            "#,
        )
        .unwrap();
        fs::write(
            root.join("crates/example/src/neutral.rs"),
            r#"
                struct NeutralValue;
                struct Request { game: NeutralValue }
                struct RuntimeInputBackend;
                impl RuntimeInputBackend {
                    fn select(candidate: &str) -> bool {
                        candidate == "ordinary-neutral-value"
                    }
                }
                fn inspect(request: &Request) -> bool {
                    let _ = &request.game;
                    false
                }
            "#,
        )
        .unwrap();
        create_required_roots(&root);

        let contexts = workspace_rust_identity_contexts(&root).unwrap();
        let mut violations = Vec::new();
        for stable_path in [
            "crates/example/src/lib.rs",
            "crates/example/src/backend.rs",
            "crates/example/src/accepted.rs",
            "crates/example/src/neutral.rs",
        ] {
            let source = fs::read_to_string(root.join(stable_path)).unwrap();
            for fragment in rust_identity_fragments(stable_path, &source).unwrap() {
                violations.extend(
                    inspect_workspace_rust_identity_fragment(
                        stable_path,
                        stable_path,
                        &fragment,
                        &contexts,
                    )
                    .unwrap(),
                );
            }
        }
        assert!(violations.iter().any(|violation| {
            violation.contains("synthetic_project_code") && violation.contains("axis game")
        }), "{violations:#?}");
        assert!(!violations.iter().any(|violation| {
            violation.contains("ordinary-neutral-value")
        }), "{violations:#?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn identity_flow_fails_closed_for_ambiguous_local_method_with_identity() {
        let source = r#"
            struct Backend;
            trait First { fn select(&self, value: &str) -> bool; }
            trait Second { fn select(&self, value: &str) -> bool; }
            impl First for Backend {
                fn select(&self, value: &str) -> bool { value == "first" }
            }
            impl Second for Backend {
                fn select(&self, value: &str) -> bool { value == "second" }
            }
            fn route(backend: &Backend, game: &str) -> bool { backend.select(game) }
        "#;
        let error = inspect_identity_axis_branches("ambiguous-method.rs", source).unwrap_err();
        assert!(error.contains("resolved to 2 local impl items"), "{error}");
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
            "rust_wire_attribute",
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
        assert!(error.contains("unknown_project_code"), "{error}");
        assert!(error.contains("project-specific word baas"), "{error}");
        assert!(error.contains("project-specific word pvp"), "{error}");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_genericity_requires_exact_allowance_hash_and_registered_surface() {
        let root = temporary_workspace("identity-allowance");
        write_workspace_manifest(&root, &["crates/example"]);
        fs::create_dir_all(root.join("crates/example/src")).unwrap();
        let path = "crates/example/src/lib.rs";
        fs::write(
            root.join(path),
            "pub struct Marker;\nfn compile_maa_tasks() {}\n",
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

        fs::write(
            root.join(path),
            "pub struct Marker;\nfn compile_maa_jobs() {}\n",
        )
        .unwrap();
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
            "pub struct Marker;\nfn compile_maa_tasks() {}\nfn compile_maa_jobs() {}\n",
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
        assert!(error.contains("unmapped protected surface schema_key"));
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
    fn nonsemantic_proto_and_root_script_do_not_gain_approved_mappings() {
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
        assert!(!snapshots.iter().any(|surface| {
            matches!(
                surface.stable_path.as_str(),
                "contracts/example.proto" | "verify.ps1"
            )
        }));
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
        let mut concept_ids = snapshots
            .iter()
            .map(|snapshot| snapshot.semantic_role.anchor_concept().to_string())
            .collect::<Vec<_>>();
        concept_ids.sort();
        concept_ids.dedup();
        let mut surfaces = snapshots
            .iter()
            .map(|snapshot| ProtectedSurface {
                surface_id: snapshot.surface_id.clone(),
                kind: snapshot.kind.clone(),
                semantic_role: snapshot.semantic_role,
                stable_path: snapshot.stable_path.clone(),
                selector: snapshot.selector.clone(),
                concept_ids: vec![snapshot.semantic_role.anchor_concept().to_string()],
                fingerprint: snapshot.fingerprint.clone(),
                mapping_source_id: String::new(),
            })
            .collect::<Vec<_>>();
        surfaces.sort_by(|left, right| left.surface_id.cmp(&right.surface_id));
        let mut source = MappingSource {
            id: String::new(),
            task_issue: 75,
            implementation_pr: 137,
            source_kind: "workflow_task".to_string(),
            change_kind: "initial_import".to_string(),
        };
        let mapped = surfaces.iter().collect::<Vec<_>>();
        source.id = mapping_source_id(&source, &mapped);
        for surface in &mut surfaces {
            surface.mapping_source_id = source.id.clone();
        }
        GenericDomainRegistry {
            schema_version: GENERIC_DOMAIN_SCHEMA_VERSION.to_string(),
            concept: concept_ids
                .into_iter()
                .map(|id| GenericConcept {
                    id,
                    status: "active".to_string(),
                    approval_comment_id: INITIAL_CONCEPT_APPROVAL_COMMENT_ID,
                    replaced_by: None,
                })
                .collect(),
            mapping_source: vec![source],
            identity_allowance: Vec::new(),
            surface: surfaces,
        }
    }

    fn rebind_mapping_source(registry: &mut GenericDomainRegistry) {
        assert_eq!(registry.mapping_source.len(), 1);
        let source = &mut registry.mapping_source[0];
        let mapped = registry.surface.iter().collect::<Vec<_>>();
        source.id = mapping_source_id(source, &mapped);
        for surface in &mut registry.surface {
            surface.mapping_source_id = source.id.clone();
        }
    }
}
