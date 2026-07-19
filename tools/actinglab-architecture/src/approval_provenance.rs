// SPDX-License-Identifier: AGPL-3.0-only

//! External approval evidence and protected base-to-head surface verification.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::generic_domain::{
    ApprovalChangeBinding, ApprovalLifecycle, ApprovalLifecycleKind, GENERIC_DOMAIN_REGISTRY_PATH,
    GenericDomainRegistry, IdentityAllowance, SurfaceApproval, SurfaceSnapshot,
    approval_lifecycle_kind, load_generic_domain_registry, validate_workspace_genericity,
    workspace_surface_snapshot,
};

pub const TRUSTED_APPROVER_ID: u64 = 103_177_863;

const R8_SCOPES: &[&str] = &["surface.mapping"];
const R8B_SCOPES: &[&str] = &["identity.allowance", "surface.mapping"];
const R8C_SCOPES: &[&str] = &[
    "identity.allowance",
    "surface.mapping",
    "workspace.discovery",
];
const R8D_SCOPES: &[&str] = &["external.compat", "identity.allowance", "surface.mapping"];
const R8E_SCOPES: &[&str] = &[
    "compatibility.alias",
    "identity.allowance",
    "safety.effect",
    "surface.mapping",
];
const R9_SCOPES: &[&str] = &[
    "approval.provenance",
    "compatibility.alias",
    "external.compat",
    "identity.allowance",
    "safety.effect",
    "surface.mapping",
    "workspace.discovery",
];
const ISSUE65_S1_SCOPES: &[&str] = &[
    "approval.provenance",
    "identity.allowance",
    "surface.mapping",
];
const BINDING_START: &str = "<!-- actingcommand-approval-binding-v1";
const BINDING_END: &str = "-->";
const LEGACY_MIGRATION_APPROVAL_ID: &str = "approval.issue65_s1";

static WORKTREE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalProvenanceReport {
    pub approvals_verified: usize,
    pub surface_changes_verified: usize,
    pub tracked_files_added: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubIssueComment {
    pub id: u64,
    pub issue_url: String,
    pub created_at: String,
    pub updated_at: String,
    pub body: String,
    pub user: GitHubUser,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GitHubUser {
    pub id: u64,
    pub login: String,
}

pub trait ApprovalCommentSource {
    fn fetch_comment(
        &self,
        repository: &str,
        comment_id: u64,
    ) -> Result<GitHubIssueComment, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GhApprovalCommentSource;

impl ApprovalCommentSource for GhApprovalCommentSource {
    fn fetch_comment(
        &self,
        repository: &str,
        comment_id: u64,
    ) -> Result<GitHubIssueComment, String> {
        let endpoint = format!("repos/{repository}/issues/comments/{comment_id}");
        let output = Command::new("gh")
            .env("GH_PROMPT_DISABLED", "1")
            .args([
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                &endpoint,
            ])
            .output()
            .map_err(|error| {
                format!("approval provenance GitHub API client failed to start: {error}")
            })?;
        if !output.status.success() {
            return Err(format!(
                "approval provenance GitHub API request for comment {comment_id} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| {
            format!("approval provenance GitHub API returned invalid JSON: {error}")
        })
    }
}

#[derive(Debug)]
struct RevisionEvidence {
    registry: GenericDomainRegistry,
    surfaces: Vec<SurfaceSnapshot>,
    tracked_files: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalAuthority {
    Active,
    None,
}

#[derive(Debug, Clone)]
struct VerifiedApproval {
    approval: SurfaceApproval,
    authority: ApprovalAuthority,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ApprovalLifecycleDelta {
    change_count: usize,
    migrated_legacy: BTreeSet<String>,
    retired: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommentChangeBinding {
    binding: ApprovalChangeBinding,
    scopes: Vec<String>,
}

pub fn verify_approval_provenance(
    repository_root: &Path,
    base: &str,
    head: &str,
    source: &impl ApprovalCommentSource,
) -> Result<ApprovalProvenanceReport, String> {
    let root = canonical_repository_root(repository_root)?;
    let base = resolve_full_commit(&root, base)?;
    let head = resolve_full_commit(&root, head)?;
    ensure_ancestor(&root, &base, &head)?;

    let base_evidence = revision_evidence(&root, &base, false)?;
    let head_evidence = revision_evidence(&root, &head, true)?;
    let lifecycle_delta = validate_approval_lifecycle_delta(
        &base_evidence.registry.approval,
        &head_evidence.registry.approval,
    )?;
    let remotely_verified = verify_remote_approvals(&head_evidence.registry, source)?;
    let verified_approvals = bind_approval_authority(remotely_verified)?;
    validate_registry_delta(
        &base_evidence.registry,
        &head_evidence.registry,
        &verified_approvals,
        &lifecycle_delta,
    )?;
    let (surface_changes_verified, tracked_files_added) =
        validate_surface_delta(&base_evidence, &head_evidence, &verified_approvals)?;

    Ok(ApprovalProvenanceReport {
        approvals_verified: verified_approvals.len(),
        surface_changes_verified,
        tracked_files_added,
    })
}

fn verify_remote_approvals(
    registry: &GenericDomainRegistry,
    source: &impl ApprovalCommentSource,
) -> Result<HashMap<String, SurfaceApproval>, String> {
    let mut verified = HashMap::new();
    for approval in &registry.approval {
        let author_id = approval.author_id.ok_or_else(|| {
            format!(
                "approval {} does not record the trusted numeric author id",
                approval.id
            )
        })?;
        let created_at = approval
            .created_at
            .as_deref()
            .ok_or_else(|| format!("approval {} does not record created_at", approval.id))?;
        let updated_at = approval
            .updated_at
            .as_deref()
            .ok_or_else(|| format!("approval {} does not record updated_at", approval.id))?;
        if author_id != TRUSTED_APPROVER_ID {
            return Err(format!(
                "approval {} records untrusted numeric author id {author_id}",
                approval.id
            ));
        }

        let comment = source.fetch_comment(&approval.repository, approval.comment_id)?;
        let expected_issue_url = format!(
            "https://api.github.com/repos/{}/issues/{}",
            approval.repository, approval.issue
        );
        let actual_hash = format!("{:x}", Sha256::digest(comment.body.as_bytes()));
        let mut errors = Vec::new();
        if comment.id != approval.comment_id {
            errors.push(format!(
                "comment id is {}, expected {}",
                comment.id, approval.comment_id
            ));
        }
        if comment.issue_url != expected_issue_url {
            errors.push(format!(
                "comment issue is {}, expected {expected_issue_url}",
                comment.issue_url
            ));
        }
        if comment.user.id != TRUSTED_APPROVER_ID || comment.user.id != author_id {
            errors.push(format!(
                "comment numeric author id is {}, expected {TRUSTED_APPROVER_ID}",
                comment.user.id
            ));
        }
        if comment.user.login != approval.author {
            errors.push(format!(
                "comment author is {}, expected {}",
                comment.user.login, approval.author
            ));
        }
        if comment.created_at != created_at {
            errors.push(format!(
                "comment created_at is {}, expected {created_at}",
                comment.created_at
            ));
        }
        if comment.updated_at != updated_at {
            errors.push(format!(
                "comment updated_at is {}, expected {updated_at}",
                comment.updated_at
            ));
        }
        if actual_hash != approval.content_sha256 {
            errors.push(format!(
                "comment body SHA-256 is {actual_hash}, expected {}",
                approval.content_sha256
            ));
        }
        if !errors.is_empty() {
            return Err(format!(
                "approval {} failed external verification: {}",
                approval.id,
                errors.join("; ")
            ));
        }
        match approval_lifecycle_kind(approval)? {
            ApprovalLifecycleKind::UnmigratedLegacy => {
                return Err(format!(
                    "approval {} has no explicit lifecycle",
                    approval.id
                ));
            }
            ApprovalLifecycleKind::LegacyRetired => {
                let trusted_scopes = trusted_comment_scopes(approval.comment_id)?;
                if let Some(scope) = approval
                    .scope
                    .iter()
                    .find(|scope| !trusted_scopes.contains(&scope.as_str()))
                {
                    return Err(format!(
                        "legacy approval {} self-reports scope {scope} that its trusted migration binding does not authorize",
                        approval.id
                    ));
                }
            }
            ApprovalLifecycleKind::Active | ApprovalLifecycleKind::Retired => {
                let declared = approval.change_binding.as_ref().ok_or_else(|| {
                    format!("approval {} has no immutable change binding", approval.id)
                })?;
                let parsed = parse_comment_change_binding(&comment.body).map_err(|error| {
                    format!(
                        "approval {} has invalid external change binding: {error}",
                        approval.id
                    )
                })?;
                if &parsed.binding != declared {
                    return Err(format!(
                        "approval {} registry change binding does not match its trusted comment",
                        approval.id
                    ));
                }
                if parsed.scopes != approval.scope {
                    return Err(format!(
                        "approval {} registry scopes do not exactly match its trusted comment",
                        approval.id
                    ));
                }
            }
        }
        verified.insert(approval.id.clone(), approval.clone());
    }
    Ok(verified)
}

fn parse_comment_change_binding(body: &str) -> Result<CommentChangeBinding, String> {
    let lines = body.lines().map(str::trim).collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (*line == BINDING_START).then_some(index))
        .collect::<Vec<_>>();
    if starts.len() != 1 {
        return Err(format!(
            "expected exactly one {BINDING_START} block, found {}",
            starts.len()
        ));
    }
    let start = starts[0];
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| (*line == BINDING_END).then_some(index))
        .ok_or_else(|| "change binding block has no closing -->".to_string())?;

    let mut fields = BTreeMap::new();
    for line in &lines[start + 1..end] {
        if line.is_empty() {
            return Err("change binding block contains an empty line".to_string());
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("change binding line is not key=value: {line}"))?;
        if key.is_empty() || value.is_empty() {
            return Err(format!(
                "change binding line has an empty key or value: {line}"
            ));
        }
        if fields.insert(key, value).is_some() {
            return Err(format!("change binding repeats field {key}"));
        }
    }
    let expected = BTreeSet::from([
        "base_sha",
        "pull_request",
        "scopes",
        "subject_sha",
        "target_repository",
    ]);
    let actual = fields.keys().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "change binding fields are {:?}, expected {:?}",
            actual, expected
        ));
    }

    let pull_request = fields["pull_request"]
        .parse::<u64>()
        .map_err(|error| format!("change binding pull_request is invalid: {error}"))?;
    if pull_request == 0 {
        return Err("change binding pull_request must be non-zero".to_string());
    }
    let scopes = fields["scopes"]
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if scopes.iter().any(String::is_empty)
        || scopes
            .windows(2)
            .any(|pair| pair[0].as_str() >= pair[1].as_str())
    {
        return Err("change binding scopes must be non-empty, unique, and sorted".to_string());
    }

    Ok(CommentChangeBinding {
        binding: ApprovalChangeBinding {
            target_repository: fields["target_repository"].to_string(),
            pull_request,
            base_sha: fields["base_sha"].to_string(),
            subject_sha: fields["subject_sha"].to_string(),
        },
        scopes,
    })
}

fn trusted_comment_scopes(comment_id: u64) -> Result<&'static [&'static str], String> {
    match comment_id {
        5011264343 => Ok(R8_SCOPES),
        5011350539 => Ok(R8B_SCOPES),
        5011427079 => Ok(R8C_SCOPES),
        5011483079 => Ok(R8D_SCOPES),
        5011549303 => Ok(R8E_SCOPES),
        5011923710 => Ok(R9_SCOPES),
        5014804131 => Ok(ISSUE65_S1_SCOPES),
        _ => Err(format!(
            "approval comment {comment_id} has no trusted scope binding"
        )),
    }
}

fn validate_approval_lifecycle_delta(
    base: &[SurfaceApproval],
    head: &[SurfaceApproval],
) -> Result<ApprovalLifecycleDelta, String> {
    let base = base
        .iter()
        .map(|approval| (approval.id.as_str(), approval))
        .collect::<BTreeMap<_, _>>();
    let head = head
        .iter()
        .map(|approval| (approval.id.as_str(), approval))
        .collect::<BTreeMap<_, _>>();
    let mut delta = ApprovalLifecycleDelta::default();

    for (id, base_approval) in &base {
        let head_approval = head
            .get(id)
            .copied()
            .ok_or_else(|| format!("approval registry removed {id} without a tombstone"))?;
        if head_approval == *base_approval {
            continue;
        }
        delta.change_count += 1;
        match (
            approval_lifecycle_kind(base_approval)?,
            approval_lifecycle_kind(head_approval)?,
        ) {
            (ApprovalLifecycleKind::UnmigratedLegacy, ApprovalLifecycleKind::LegacyRetired) => {
                let mut expected = (*base_approval).clone();
                expected.lifecycle = Some(ApprovalLifecycle {
                    version: 0,
                    state: "retired".to_string(),
                    legacy_migration: true,
                });
                if head_approval != &expected {
                    return Err(format!(
                        "legacy approval {id} migration changed fields beyond its lifecycle marker"
                    ));
                }
                delta.migrated_legacy.insert((*id).to_string());
            }
            (ApprovalLifecycleKind::Active, ApprovalLifecycleKind::Retired) => {
                let mut expected = (*base_approval).clone();
                let lifecycle = expected.lifecycle.as_mut().ok_or_else(|| {
                    format!("active approval {id} has no lifecycle during retirement")
                })?;
                lifecycle.state = "retired".to_string();
                if head_approval != &expected {
                    return Err(format!(
                        "approval {id} retirement changed its immutable change binding or approval facts"
                    ));
                }
                delta.retired.insert((*id).to_string());
            }
            (ApprovalLifecycleKind::Retired, ApprovalLifecycleKind::Active) => {
                return Err(format!("retired approval {id} cannot be reactivated"));
            }
            (ApprovalLifecycleKind::Retired, ApprovalLifecycleKind::Retired) => {
                return Err(format!("retired approval {id} changed after retirement"));
            }
            (ApprovalLifecycleKind::LegacyRetired, _) => {
                return Err(format!(
                    "legacy retired approval {id} is immutable after migration"
                ));
            }
            (ApprovalLifecycleKind::Active, ApprovalLifecycleKind::Active) => {
                return Err(format!(
                    "active approval {id} changed its immutable approval facts"
                ));
            }
            (base_state, head_state) => {
                return Err(format!(
                    "approval {id} has invalid lifecycle transition {base_state:?} -> {head_state:?}"
                ));
            }
        }
    }

    for (id, approval) in &head {
        if base.contains_key(id) {
            continue;
        }
        delta.change_count += 1;
        match approval_lifecycle_kind(approval)? {
            ApprovalLifecycleKind::Active => {}
            ApprovalLifecycleKind::Retired => {
                return Err(format!("new approval {id} cannot enter already retired"));
            }
            ApprovalLifecycleKind::LegacyRetired => {
                return Err(format!(
                    "new approval {id} cannot enter through the legacy migration path"
                ));
            }
            ApprovalLifecycleKind::UnmigratedLegacy => {
                return Err(format!("new approval {id} has no explicit lifecycle"));
            }
        }
    }

    Ok(delta)
}

fn bind_approval_authority(
    approvals: HashMap<String, SurfaceApproval>,
) -> Result<HashMap<String, VerifiedApproval>, String> {
    approvals
        .into_iter()
        .map(|(id, approval)| {
            let authority = match approval_lifecycle_kind(&approval)? {
                ApprovalLifecycleKind::Active => ApprovalAuthority::Active,
                ApprovalLifecycleKind::LegacyRetired | ApprovalLifecycleKind::Retired => {
                    ApprovalAuthority::None
                }
                ApprovalLifecycleKind::UnmigratedLegacy => {
                    return Err(format!("approval {id} has no explicit lifecycle"));
                }
            };
            Ok((
                id,
                VerifiedApproval {
                    approval,
                    authority,
                },
            ))
        })
        .collect()
}

fn validate_registry_delta(
    base: &GenericDomainRegistry,
    head: &GenericDomainRegistry,
    verified: &HashMap<String, VerifiedApproval>,
    lifecycle_delta: &ApprovalLifecycleDelta,
) -> Result<(), String> {
    authorize_lifecycle_delta(verified, lifecycle_delta)?;

    let base_allowances = allowance_map(&base.identity_allowance);
    let head_allowances = allowance_map(&head.identity_allowance);
    for (id, allowance) in &head_allowances {
        if base_allowances.get(id).copied() == Some(*allowance) {
            continue;
        }
        require_approval_scope(
            verified,
            &allowance.approval_id,
            "identity.allowance",
            &format!("identity allowance {id}"),
        )?;
    }
    if base_allowances
        .keys()
        .any(|id| !head_allowances.contains_key(id))
    {
        require_any_scope(verified, "identity.allowance", "identity allowance removal")?;
    }
    Ok(())
}

fn authorize_lifecycle_delta(
    verified: &HashMap<String, VerifiedApproval>,
    delta: &ApprovalLifecycleDelta,
) -> Result<(), String> {
    if delta.change_count == 0 {
        return Ok(());
    }
    if verified.values().any(|entry| {
        entry.authority == ApprovalAuthority::Active
            && entry
                .approval
                .scope
                .iter()
                .any(|scope| scope == "approval.provenance")
    }) {
        return Ok(());
    }
    if delta.change_count == delta.migrated_legacy.len()
        && delta.migrated_legacy.contains(LEGACY_MIGRATION_APPROVAL_ID)
    {
        let migration = verified
            .get(LEGACY_MIGRATION_APPROVAL_ID)
            .ok_or_else(|| {
                format!(
                    "legacy lifecycle migration is missing externally verified approval {LEGACY_MIGRATION_APPROVAL_ID}"
                )
            })?;
        if migration
            .approval
            .scope
            .iter()
            .any(|scope| scope == "approval.provenance")
        {
            return Ok(());
        }
    }
    if delta.change_count == delta.retired.len() {
        for approval_id in &delta.retired {
            let retirement = verified.get(approval_id).ok_or_else(|| {
                format!("retirement is missing externally verified approval {approval_id}")
            })?;
            if !retirement
                .approval
                .scope
                .iter()
                .any(|scope| scope == "approval.provenance")
            {
                return Err(format!(
                    "approval {approval_id} cannot retire without its immutable approval.provenance scope"
                ));
            }
        }
        return Ok(());
    }
    Err("approval registry delta is not covered by an active approval or a permitted lifecycle-only transition".to_string())
}

fn allowance_map(allowances: &[IdentityAllowance]) -> BTreeMap<&str, &IdentityAllowance> {
    allowances
        .iter()
        .map(|allowance| (allowance.id.as_str(), allowance))
        .collect()
}

fn validate_surface_delta(
    base: &RevisionEvidence,
    head: &RevisionEvidence,
    verified: &HashMap<String, VerifiedApproval>,
) -> Result<(usize, usize), String> {
    let base_surfaces = base
        .surfaces
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<BTreeMap<_, _>>();
    let head_surfaces = head
        .surfaces
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<BTreeMap<_, _>>();
    let registered = head
        .registry
        .surface
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<HashMap<_, _>>();
    let added_files = head
        .tracked_files
        .difference(&base.tracked_files)
        .cloned()
        .collect::<BTreeSet<_>>();

    for (surface_id, surface) in &base_surfaces {
        if !head_surfaces.contains_key(surface_id) {
            return Err(format!(
                "surface removal {} at {} has no approved tombstone",
                surface.surface_id, surface.stable_path
            ));
        }
    }

    let mut changed = 0;
    let mut added_files_with_surface = BTreeSet::new();
    for (surface_id, surface) in &head_surfaces {
        if base_surfaces.get(surface_id).copied() == Some(*surface) {
            continue;
        }
        changed += 1;
        let registration = registered.get(surface_id).ok_or_else(|| {
            format!(
                "changed surface {} is absent from the candidate registry",
                surface.surface_id
            )
        })?;
        let context = format!(
            "surface {} at {} {}",
            surface.surface_id, surface.stable_path, surface.selector
        );
        require_approval_scope(
            verified,
            &registration.approval_id,
            "surface.mapping",
            &context,
        )?;
        if approval_provenance_surface(surface) {
            require_approval_scope(
                verified,
                &registration.approval_id,
                "approval.provenance",
                &context,
            )?;
        }
        if external_compat_surface(surface) {
            require_approval_scope(
                verified,
                &registration.approval_id,
                "external.compat",
                &context,
            )?;
        }
        if added_files.contains(&surface.stable_path) {
            added_files_with_surface.insert(surface.stable_path.clone());
            require_approval_scope(
                verified,
                &registration.approval_id,
                "workspace.discovery",
                &context,
            )?;
        }
    }

    for path in &added_files {
        if head
            .surfaces
            .iter()
            .any(|surface| &surface.stable_path == path)
            && !added_files_with_surface.contains(path)
        {
            return Err(format!(
                "new tracked protected file {path} has no approved changed surface"
            ));
        }
    }
    Ok((changed, added_files.len()))
}

fn approval_provenance_surface(surface: &SurfaceSnapshot) -> bool {
    surface.stable_path.contains("approval_provenance")
        || surface.selector.to_ascii_lowercase().contains("approval")
            && surface.stable_path.ends_with("generic_domain.rs")
}

fn external_compat_surface(surface: &SurfaceSnapshot) -> bool {
    surface.stable_path == "tests/external-compat/manifest.toml"
        || surface.stable_path.starts_with("tests/external-compat/")
}

fn require_any_scope(
    approvals: &HashMap<String, VerifiedApproval>,
    scope: &str,
    context: &str,
) -> Result<(), String> {
    if approvals.values().any(|verified| {
        verified.authority != ApprovalAuthority::None
            && verified.approval.scope.iter().any(|item| item == scope)
    }) {
        Ok(())
    } else {
        Err(format!(
            "{context} is not covered by any externally verified {scope} approval"
        ))
    }
}

fn require_approval_scope(
    approvals: &HashMap<String, VerifiedApproval>,
    approval_id: &str,
    scope: &str,
    context: &str,
) -> Result<(), String> {
    let verified = approvals
        .get(approval_id)
        .ok_or_else(|| format!("{context} references unverified approval {approval_id}"))?;
    if verified.authority == ApprovalAuthority::None {
        return Err(format!(
            "{context} references retired approval {approval_id}, which cannot authorize changes"
        ));
    }
    if verified.approval.scope.iter().any(|item| item == scope) {
        Ok(())
    } else {
        Err(format!(
            "{context} requires scope {scope}, but approval {approval_id} does not authorize it"
        ))
    }
}

fn revision_evidence(
    repository_root: &Path,
    revision: &str,
    validate_candidate: bool,
) -> Result<RevisionEvidence, String> {
    with_revision_worktree(repository_root, revision, |root| {
        let registry_path = root.join(GENERIC_DOMAIN_REGISTRY_PATH);
        let registry = load_generic_domain_registry(&registry_path)?;
        if validate_candidate {
            validate_workspace_genericity(root, &registry)?;
        }
        Ok(RevisionEvidence {
            registry,
            surfaces: workspace_surface_snapshot(root)?,
            tracked_files: tracked_files(root)?,
        })
    })
}

fn canonical_repository_root(path: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to resolve repository root {}: {error}",
            path.display()
        )
    })?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("failed to verify Git repository root: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to verify Git repository root: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let discovered = PathBuf::from(
        String::from_utf8(output.stdout)
            .map_err(|error| format!("Git repository root is not UTF-8: {error}"))?
            .trim(),
    );
    let discovered = fs::canonicalize(&discovered).map_err(|error| {
        format!(
            "failed to resolve discovered repository root {}: {error}",
            discovered.display()
        )
    })?;
    if discovered != root {
        return Err(format!(
            "approval provenance root {} is not the Git repository root {}",
            root.display(),
            discovered.display()
        ));
    }
    Ok(root)
}

fn resolve_full_commit(root: &Path, revision: &str) -> Result<String, String> {
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "approval provenance revision must be a full 40-character commit SHA: {revision}"
        ));
    }
    let specification = format!("{revision}^{{commit}}");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", &specification])
        .output()
        .map_err(|error| format!("failed to resolve commit {revision}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to resolve commit {revision}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let resolved = String::from_utf8(output.stdout)
        .map_err(|error| format!("resolved commit is not UTF-8: {error}"))?
        .trim()
        .to_ascii_lowercase();
    if resolved != revision.to_ascii_lowercase() {
        return Err(format!(
            "approval provenance revision {revision} resolved to unexpected commit {resolved}"
        ));
    }
    Ok(resolved)
}

fn ensure_ancestor(root: &Path, base: &str, head: &str) -> Result<(), String> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", base, head])
        .status()
        .map_err(|error| format!("failed to verify base/head ancestry: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "approval provenance base {base} is not an ancestor of head {head}"
        ))
    }
}

fn with_revision_worktree<T>(
    repository_root: &Path,
    revision: &str,
    inspect: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    let path = unique_worktree_path(revision)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(["worktree", "add", "--detach", "--quiet"])
        .arg(&path)
        .arg(revision)
        .output()
        .map_err(|error| format!("failed to create revision worktree {revision}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to create revision worktree {revision}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let result = inspect(&path);
    let cleanup = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(["worktree", "remove", "--force"])
        .arg(&path)
        .output()
        .map_err(|error| format!("failed to remove revision worktree {revision}: {error}"))
        .and_then(|output| {
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "failed to remove revision worktree {revision}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ))
            }
        });
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failure: {cleanup}")),
    }
}

fn unique_worktree_path(revision: &str) -> Result<PathBuf, String> {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before UNIX epoch: {error}"))?
        .as_nanos();
    let sequence = WORKTREE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!(
        "actingcommand-approval-{}-{epoch}-{sequence}-{}",
        std::process::id(),
        &revision[..12]
    ));
    if path.exists() {
        return Err(format!(
            "approval provenance temporary worktree already exists: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn tracked_files(root: &Path) -> Result<BTreeSet<String>, String> {
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
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(str::to_string)
                .map_err(|error| format!("trusted Git index contains non-UTF-8 path: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE_COMMENT_ID: u64 = 9_000_000_001;

    #[derive(Default)]
    struct FakeSource {
        comments: HashMap<u64, Result<GitHubIssueComment, String>>,
    }

    impl ApprovalCommentSource for FakeSource {
        fn fetch_comment(
            &self,
            _repository: &str,
            comment_id: u64,
        ) -> Result<GitHubIssueComment, String> {
            self.comments
                .get(&comment_id)
                .cloned()
                .unwrap_or_else(|| Err(format!("comment {comment_id} not found")))
        }
    }

    fn approval(scopes: &[&str]) -> SurfaceApproval {
        SurfaceApproval {
            id: "approval.issue44_r9".to_string(),
            repository: "HS7097/ActingCommand-Workflow".to_string(),
            issue: 54,
            comment_id: 5011923710,
            author: "HS7097".to_string(),
            author_id: Some(TRUSTED_APPROVER_ID),
            created_at: Some("2026-07-18T16:02:09Z".to_string()),
            updated_at: Some("2026-07-18T16:02:09Z".to_string()),
            content_sha256: format!("{:x}", Sha256::digest(b"approved body")),
            scope: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            lifecycle: Some(ApprovalLifecycle {
                version: 0,
                state: "retired".to_string(),
                legacy_migration: true,
            }),
            change_binding: None,
        }
    }

    fn active_approval(scopes: &[&str]) -> SurfaceApproval {
        let body = active_comment_body(scopes);
        SurfaceApproval {
            id: "approval.test_active".to_string(),
            repository: "HS7097/ActingCommand-Workflow".to_string(),
            issue: 65,
            comment_id: ACTIVE_COMMENT_ID,
            author: "HS7097".to_string(),
            author_id: Some(TRUSTED_APPROVER_ID),
            created_at: Some("2026-07-19T12:00:00Z".to_string()),
            updated_at: Some("2026-07-19T12:00:00Z".to_string()),
            content_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
            scope: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            lifecycle: Some(ApprovalLifecycle {
                version: crate::generic_domain::APPROVAL_LIFECYCLE_VERSION,
                state: "active".to_string(),
                legacy_migration: false,
            }),
            change_binding: Some(test_change_binding()),
        }
    }

    fn issue65_legacy_approval(explicit_lifecycle: bool) -> SurfaceApproval {
        let body = "issue 65 approval";
        SurfaceApproval {
            id: LEGACY_MIGRATION_APPROVAL_ID.to_string(),
            repository: "HS7097/ActingCommand-Workflow".to_string(),
            issue: 65,
            comment_id: 5014804131,
            author: "HS7097".to_string(),
            author_id: Some(TRUSTED_APPROVER_ID),
            created_at: Some("2026-07-19T07:06:09Z".to_string()),
            updated_at: Some("2026-07-19T07:06:09Z".to_string()),
            content_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
            scope: ISSUE65_S1_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            lifecycle: explicit_lifecycle.then(|| ApprovalLifecycle {
                version: 0,
                state: "retired".to_string(),
                legacy_migration: true,
            }),
            change_binding: None,
        }
    }

    fn retired(mut approval: SurfaceApproval) -> SurfaceApproval {
        approval.lifecycle.as_mut().unwrap().state = "retired".to_string();
        approval
    }

    fn test_change_binding() -> ApprovalChangeBinding {
        ApprovalChangeBinding {
            target_repository: "HS7097/ActingCommand-Runtime".to_string(),
            pull_request: 127,
            base_sha: "a".repeat(40),
            subject_sha: "b".repeat(40),
        }
    }

    fn active_comment_body(scopes: &[&str]) -> String {
        format!(
            "approved body\n{BINDING_START}\ntarget_repository=HS7097/ActingCommand-Runtime\npull_request=127\nbase_sha={}\nsubject_sha={}\nscopes={}\n{BINDING_END}",
            "a".repeat(40),
            "b".repeat(40),
            scopes.join(",")
        )
    }

    fn registry(approval: SurfaceApproval) -> GenericDomainRegistry {
        GenericDomainRegistry {
            schema_version: crate::generic_domain::GENERIC_DOMAIN_SCHEMA_VERSION.to_string(),
            surface_manifest: None,
            approval: vec![approval],
            concept: Vec::new(),
            identity_allowance: Vec::new(),
            surface: Vec::new(),
        }
    }

    fn comment() -> GitHubIssueComment {
        GitHubIssueComment {
            id: 5011923710,
            issue_url: "https://api.github.com/repos/HS7097/ActingCommand-Workflow/issues/54"
                .to_string(),
            created_at: "2026-07-18T16:02:09Z".to_string(),
            updated_at: "2026-07-18T16:02:09Z".to_string(),
            body: "approved body".to_string(),
            user: GitHubUser {
                id: TRUSTED_APPROVER_ID,
                login: "HS7097".to_string(),
            },
        }
    }

    fn active_comment(scopes: &[&str]) -> GitHubIssueComment {
        GitHubIssueComment {
            id: ACTIVE_COMMENT_ID,
            issue_url: "https://api.github.com/repos/HS7097/ActingCommand-Workflow/issues/65"
                .to_string(),
            created_at: "2026-07-19T12:00:00Z".to_string(),
            updated_at: "2026-07-19T12:00:00Z".to_string(),
            body: active_comment_body(scopes),
            user: GitHubUser {
                id: TRUSTED_APPROVER_ID,
                login: "HS7097".to_string(),
            },
        }
    }

    fn issue65_comment() -> GitHubIssueComment {
        GitHubIssueComment {
            id: 5014804131,
            issue_url: "https://api.github.com/repos/HS7097/ActingCommand-Workflow/issues/65"
                .to_string(),
            created_at: "2026-07-19T07:06:09Z".to_string(),
            updated_at: "2026-07-19T07:06:09Z".to_string(),
            body: "issue 65 approval".to_string(),
            user: GitHubUser {
                id: TRUSTED_APPROVER_ID,
                login: "HS7097".to_string(),
            },
        }
    }

    fn identity_allowance(approval_id: &str) -> IdentityAllowance {
        IdentityAllowance {
            id: "allowance.test".to_string(),
            kind: "identity_branch".to_string(),
            exact_path: "crates/test/src/lib.rs".to_string(),
            selector: "rust:fn:test".to_string(),
            scope: vec!["identity.token".to_string()],
            tokens: vec!["game".to_string()],
            sha256: "c".repeat(64),
            purpose: "Lifecycle authorization test.".to_string(),
            approval_id: approval_id.to_string(),
            source_issue: 65,
            source_pr: Some(127),
        }
    }

    #[test]
    fn remote_approval_verifies_external_identity_time_and_body() {
        let registry = registry(approval(&["approval.provenance", "surface.mapping"]));
        let source = FakeSource {
            comments: HashMap::from([(5011923710, Ok(comment()))]),
        };

        let verified = verify_remote_approvals(&registry, &source).unwrap();

        assert!(verified.contains_key("approval.issue44_r9"));
    }

    #[test]
    fn fake_comment_id_and_api_failure_are_fatal() {
        let mut forged = approval(&["approval.provenance"]);
        forged.comment_id = 9_999_999_999;
        let registry = registry(forged);
        let source = FakeSource::default();

        let error = verify_remote_approvals(&registry, &source).unwrap_err();

        assert!(error.contains("comment 9999999999 not found"));
    }

    #[test]
    fn api_failure_for_known_approval_is_fatal() {
        let registry = registry(approval(&["approval.provenance"]));
        let source = FakeSource {
            comments: HashMap::from([(5011923710, Err("network unavailable".to_string()))]),
        };

        let error = verify_remote_approvals(&registry, &source).unwrap_err();

        assert!(error.contains("network unavailable"));
    }

    #[test]
    fn wrong_numeric_author_and_body_hash_are_fatal() {
        let registry = registry(approval(&["approval.provenance"]));
        let mut forged = comment();
        forged.user.id = TRUSTED_APPROVER_ID + 1;
        forged.body = "different body".to_string();
        let source = FakeSource {
            comments: HashMap::from([(5011923710, Ok(forged))]),
        };

        let error = verify_remote_approvals(&registry, &source).unwrap_err();

        assert!(error.contains("numeric author id"));
        assert!(error.contains("body SHA-256"));
    }

    #[test]
    fn candidate_scope_cannot_expand_the_trusted_comment_binding() {
        let registry = registry(approval(&["surface.mapping", "unapproved.scope"]));
        let source = FakeSource {
            comments: HashMap::from([(5011923710, Ok(comment()))]),
        };

        let error = verify_remote_approvals(&registry, &source).unwrap_err();

        assert!(error.contains("self-reports scope unapproved.scope"));
    }

    #[test]
    fn issue_and_timestamp_drift_are_fatal() {
        let registry = registry(approval(&["approval.provenance"]));
        let mut forged = comment();
        forged.issue_url =
            "https://api.github.com/repos/HS7097/ActingCommand-Workflow/issues/44".to_string();
        forged.updated_at = "2026-07-18T16:03:00Z".to_string();
        let source = FakeSource {
            comments: HashMap::from([(5011923710, Ok(forged))]),
        };

        let error = verify_remote_approvals(&registry, &source).unwrap_err();

        assert!(error.contains("comment issue"));
        assert!(error.contains("updated_at"));
    }

    #[test]
    fn new_style_approval_can_activate_then_retire_without_legacy_scope_table() {
        let active = active_approval(&["approval.provenance", "identity.allowance"]);
        let mut empty_registry = registry(active.clone());
        empty_registry.approval.clear();
        let active_registry = registry(active.clone());
        let source = FakeSource {
            comments: HashMap::from([(
                ACTIVE_COMMENT_ID,
                Ok(active_comment(&[
                    "approval.provenance",
                    "identity.allowance",
                ])),
            )]),
        };

        let activation =
            validate_approval_lifecycle_delta(&empty_registry.approval, &active_registry.approval)
                .unwrap();
        let active_verified =
            bind_approval_authority(verify_remote_approvals(&active_registry, &source).unwrap())
                .unwrap();
        validate_registry_delta(
            &empty_registry,
            &active_registry,
            &active_verified,
            &activation,
        )
        .unwrap();

        let retired_registry = registry(retired(active));
        let retirement = validate_approval_lifecycle_delta(
            &active_registry.approval,
            &retired_registry.approval,
        )
        .unwrap();
        let retired_verified =
            bind_approval_authority(verify_remote_approvals(&retired_registry, &source).unwrap())
                .unwrap();
        validate_registry_delta(
            &active_registry,
            &retired_registry,
            &retired_verified,
            &retirement,
        )
        .unwrap();

        let mut changed_registry = retired_registry.clone();
        changed_registry
            .identity_allowance
            .push(identity_allowance("approval.test_active"));
        let unchanged_lifecycle = validate_approval_lifecycle_delta(
            &retired_registry.approval,
            &changed_registry.approval,
        )
        .unwrap();
        let error = validate_registry_delta(
            &retired_registry,
            &changed_registry,
            &retired_verified,
            &unchanged_lifecycle,
        )
        .unwrap_err();
        assert!(error.contains("retired approval approval.test_active"));
    }

    #[test]
    fn retirement_rejects_binding_drift_reactivation_and_post_retirement_mutation() {
        let active = active_approval(&["approval.provenance"]);
        let retired_approval = retired(active.clone());

        let mut binding_drift = retired_approval.clone();
        binding_drift.change_binding.as_mut().unwrap().subject_sha = "c".repeat(40);
        let error =
            validate_approval_lifecycle_delta(std::slice::from_ref(&active), &[binding_drift])
                .unwrap_err();
        assert!(error.contains("immutable change binding"));

        let error = validate_approval_lifecycle_delta(
            std::slice::from_ref(&retired_approval),
            std::slice::from_ref(&active),
        )
        .unwrap_err();
        assert!(error.contains("cannot be reactivated"));

        let mut mutated = retired_approval.clone();
        mutated.scope.push("surface.mapping".to_string());
        let error = validate_approval_lifecycle_delta(&[retired_approval], &[mutated]).unwrap_err();
        assert!(error.contains("changed after retirement"));
    }

    #[test]
    fn legacy_migration_is_one_time_and_cannot_authorize_new_changes() {
        let unmigrated = issue65_legacy_approval(false);
        let migrated = issue65_legacy_approval(true);
        let base = registry(unmigrated);
        let migrated_registry = registry(migrated.clone());
        let source = FakeSource {
            comments: HashMap::from([(5014804131, Ok(issue65_comment()))]),
        };

        let migration =
            validate_approval_lifecycle_delta(&base.approval, &migrated_registry.approval).unwrap();
        let verified =
            bind_approval_authority(verify_remote_approvals(&migrated_registry, &source).unwrap())
                .unwrap();
        validate_registry_delta(&base, &migrated_registry, &verified, &migration).unwrap();

        let mut changed_registry = migrated_registry.clone();
        changed_registry
            .identity_allowance
            .push(identity_allowance(LEGACY_MIGRATION_APPROVAL_ID));
        let unchanged_lifecycle = validate_approval_lifecycle_delta(
            &migrated_registry.approval,
            &changed_registry.approval,
        )
        .unwrap();
        let error = validate_registry_delta(
            &migrated_registry,
            &changed_registry,
            &verified,
            &unchanged_lifecycle,
        )
        .unwrap_err();
        assert!(error.contains("retired approval approval.issue65_s1"));

        let mut mutated_legacy = migrated;
        mutated_legacy.scope.push("workspace.discovery".to_string());
        let error =
            validate_approval_lifecycle_delta(&migrated_registry.approval, &[mutated_legacy])
                .unwrap_err();
        assert!(error.contains("immutable after migration"));
    }

    #[test]
    fn new_retired_and_new_legacy_approvals_are_rejected() {
        let active = active_approval(&["approval.provenance"]);
        let retired_approval = retired(active);
        let error = validate_approval_lifecycle_delta(&[], &[retired_approval]).unwrap_err();
        assert!(error.contains("cannot enter already retired"));

        let error =
            validate_approval_lifecycle_delta(&[], &[issue65_legacy_approval(true)]).unwrap_err();
        assert!(error.contains("cannot enter through the legacy migration path"));
    }

    #[test]
    fn changed_surface_must_be_within_verified_scope() {
        let surface_approval = active_approval(&["surface.mapping"]);
        let candidate_registry = registry(surface_approval.clone());
        let source = FakeSource {
            comments: HashMap::from([(
                ACTIVE_COMMENT_ID,
                Ok(active_comment(&["surface.mapping"])),
            )]),
        };
        let surface = SurfaceSnapshot {
            surface_id: "surface.provenance".to_string(),
            kind: "rust.item".to_string(),
            stable_path: "tools/actinglab-architecture/src/approval_provenance.rs".to_string(),
            selector: "function:verify".to_string(),
            fingerprint: "a".repeat(64),
        };
        let registered = crate::generic_domain::ProtectedSurface {
            surface_id: surface.surface_id.clone(),
            kind: surface.kind.clone(),
            stable_path: surface.stable_path.clone(),
            selector: surface.selector.clone(),
            concept_ids: vec!["decision.approval".to_string()],
            fingerprint: surface.fingerprint.clone(),
            approval_id: surface_approval.id.clone(),
            source_issue: 44,
            source_pr: Some(120),
        };
        let base = RevisionEvidence {
            registry: registry(surface_approval.clone()),
            surfaces: Vec::new(),
            tracked_files: BTreeSet::new(),
        };
        let mut head_registry = candidate_registry;
        head_registry.surface.push(registered);
        let verified =
            bind_approval_authority(verify_remote_approvals(&head_registry, &source).unwrap())
                .unwrap();
        let head = RevisionEvidence {
            registry: head_registry,
            surfaces: vec![surface],
            tracked_files: BTreeSet::from([
                "tools/actinglab-architecture/src/approval_provenance.rs".to_string(),
            ]),
        };

        let error = validate_surface_delta(&base, &head, &verified).unwrap_err();

        assert!(error.contains("approval.provenance"));
    }
}
