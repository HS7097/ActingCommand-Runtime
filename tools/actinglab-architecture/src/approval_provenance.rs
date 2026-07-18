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
    ApprovalChangeBinding, GENERIC_DOMAIN_REGISTRY_PATH, GENERIC_DOMAIN_SURFACE_MANIFEST_PATH,
    GenericDomainRegistry, IdentityAllowance, SurfaceApproval, SurfaceSnapshot,
    load_generic_domain_registry, validate_workspace_genericity, workspace_surface_snapshot,
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
const R10_SCOPES: &[&str] = &[
    "approval.provenance",
    "identity.allowance",
    "surface.mapping",
    "workspace.discovery",
];
const TARGET_REPOSITORY: &str = "HS7097/ActingCommand-Runtime";
const BINDING_START: &str = "<!-- actingcommand-approval-binding-v1";
const BINDING_END: &str = "-->";
const POST_SUBJECT_METADATA_PATHS: &[&str] = &[
    GENERIC_DOMAIN_REGISTRY_PATH,
    GENERIC_DOMAIN_SURFACE_MANIFEST_PATH,
];

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

#[derive(Debug, Clone)]
struct RevisionEvidence {
    registry: GenericDomainRegistry,
    surfaces: Vec<SurfaceSnapshot>,
    tracked_files: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct VerifiedApproval {
    approval: SurfaceApproval,
    subject: Option<RevisionEvidence>,
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
    pull_request: u64,
    source: &impl ApprovalCommentSource,
) -> Result<ApprovalProvenanceReport, String> {
    if pull_request == 0 {
        return Err("approval provenance pull request must be non-zero".to_string());
    }
    let root = canonical_repository_root(repository_root)?;
    let base = resolve_full_commit(&root, base)?;
    let head = resolve_full_commit(&root, head)?;
    ensure_ancestor(&root, &base, &head)?;

    let base_evidence = revision_evidence(&root, &base, false)?;
    let head_evidence = revision_evidence(&root, &head, true)?;
    let remotely_verified = verify_remote_approvals(&head_evidence.registry, source)?;
    let verified_approvals =
        bind_approvals_to_revisions(&root, &base, &head, pull_request, remotely_verified)?;
    validate_registry_delta(
        &base_evidence.registry,
        &head_evidence.registry,
        &verified_approvals,
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
        if approval.retired {
            let trusted_scopes = trusted_comment_scopes(approval.comment_id)?;
            if let Some(scope) = approval
                .scope
                .iter()
                .find(|scope| !trusted_scopes.contains(&scope.as_str()))
            {
                return Err(format!(
                    "approval {} self-reports scope {scope} that its trusted comment binding does not authorize",
                    approval.id
                ));
            }
        } else {
            let declared = approval
                .change_binding
                .as_ref()
                .ok_or_else(|| format!("active approval {} has no change binding", approval.id))?;
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
        5012656084 => Ok(R10_SCOPES),
        _ => Err(format!(
            "approval comment {comment_id} has no trusted scope binding"
        )),
    }
}

fn bind_approvals_to_revisions(
    root: &Path,
    base: &str,
    head: &str,
    pull_request: u64,
    approvals: HashMap<String, SurfaceApproval>,
) -> Result<HashMap<String, VerifiedApproval>, String> {
    let active = approvals
        .values()
        .filter(|approval| !approval.retired)
        .count();
    if active != 1 {
        return Err(format!(
            "approval provenance requires exactly one active change-bound approval, found {active}"
        ));
    }

    let mut verified = HashMap::new();
    for (id, approval) in approvals {
        if approval.retired {
            verified.insert(
                id,
                VerifiedApproval {
                    approval,
                    subject: None,
                },
            );
            continue;
        }

        let binding = approval
            .change_binding
            .as_ref()
            .ok_or_else(|| format!("active approval {} has no change binding", approval.id))?;
        if binding.target_repository != TARGET_REPOSITORY {
            return Err(format!(
                "approval {} targets {}, expected {TARGET_REPOSITORY}",
                approval.id, binding.target_repository
            ));
        }
        if binding.pull_request != pull_request {
            return Err(format!(
                "approval {} binds pull request {}, but verifier is evaluating pull request {pull_request}",
                approval.id, binding.pull_request
            ));
        }
        if binding.base_sha != base {
            return Err(format!(
                "approval {} binds base {}, but verifier is evaluating base {base}",
                approval.id, binding.base_sha
            ));
        }
        let subject = resolve_full_commit(root, &binding.subject_sha)?;
        ensure_ancestor(root, base, &subject)?;
        ensure_ancestor(root, &subject, head)?;
        if subject == head {
            return Err(format!(
                "approval {} subject must precede its provenance metadata commit",
                approval.id
            ));
        }
        let post_subject_paths = changed_paths(root, &subject, head)?;
        if !post_subject_paths.contains(GENERIC_DOMAIN_REGISTRY_PATH) {
            return Err(format!(
                "approval {} has no post-subject registry binding commit",
                approval.id
            ));
        }
        if let Some(path) = post_subject_paths
            .iter()
            .find(|path| !POST_SUBJECT_METADATA_PATHS.contains(&path.as_str()))
        {
            return Err(format!(
                "approval {} has non-provenance change {path} after approved subject {subject}",
                approval.id
            ));
        }

        let subject_evidence = revision_evidence(root, &subject, false)?;
        verified.insert(
            id,
            VerifiedApproval {
                approval,
                subject: Some(subject_evidence),
            },
        );
    }
    Ok(verified)
}

fn changed_paths(root: &Path, base: &str, head: &str) -> Result<BTreeSet<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--name-only", "-z", base, head, "--"])
        .output()
        .map_err(|error| format!("failed to inspect post-subject paths: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to inspect post-subject paths: {}",
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
                .map_err(|error| format!("post-subject path is not UTF-8: {error}"))
        })
        .collect()
}

fn validate_registry_delta(
    base: &GenericDomainRegistry,
    head: &GenericDomainRegistry,
    verified: &HashMap<String, VerifiedApproval>,
) -> Result<(), String> {
    let base_approvals = base
        .approval
        .iter()
        .map(|approval| (approval.id.as_str(), approval))
        .collect::<BTreeMap<_, _>>();
    let head_approvals = head
        .approval
        .iter()
        .map(|approval| (approval.id.as_str(), approval))
        .collect::<BTreeMap<_, _>>();
    let approval_changed = base_approvals
        .iter()
        .any(|(id, approval)| head_approvals.get(id).copied() != Some(*approval))
        || head_approvals
            .keys()
            .any(|id| !base_approvals.contains_key(id));
    if approval_changed {
        require_any_scope(verified, "approval.provenance", "approval registry delta")?;
    }
    for (id, base_approval) in &base_approvals {
        let head_approval = head_approvals.get(id).copied().ok_or_else(|| {
            format!("approval registry removed {id} without an approved tombstone")
        })?;
        if head_approval == *base_approval {
            continue;
        }
        let mut expected = (*base_approval).clone();
        expected.retired = true;
        if head_approval != &expected {
            return Err(format!(
                "approval registry changed {id} beyond the one-way retirement transition"
            ));
        }
    }
    for (id, approval) in &head_approvals {
        if !base_approvals.contains_key(id) && approval.retired {
            return Err(format!("new approval {id} cannot enter already retired"));
        }
    }

    if base.surface_manifest != head.surface_manifest {
        let (Some(base_manifest), Some(head_manifest)) =
            (&base.surface_manifest, &head.surface_manifest)
        else {
            return Err(
                "surface manifest reference cannot be added or removed in-place".to_string(),
            );
        };
        let mut expected = base_manifest.clone();
        expected.sha256.clone_from(&head_manifest.sha256);
        if &expected != head_manifest {
            return Err("surface manifest reference changed beyond its content hash".to_string());
        }
        require_any_scope(verified, "surface.mapping", "surface manifest hash delta")?;
    }

    let base_allowances = allowance_map(&base.identity_allowance);
    let head_allowances = allowance_map(&head.identity_allowance);
    for (id, allowance) in &head_allowances {
        if base_allowances.get(id).copied() == Some(*allowance) {
            continue;
        }
        let approval = require_approval_scope(
            verified,
            &allowance.approval_id,
            "identity.allowance",
            &format!("identity allowance {id}"),
        )?;
        require_source_pr(
            approval,
            allowance.source_pr,
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
    let base_registered = base
        .registry
        .surface
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<HashMap<_, _>>();
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

    for (surface_id, registration) in &registered {
        if base_registered.get(surface_id).copied() == Some(*registration) {
            continue;
        }
        if base_surfaces.get(surface_id).copied() == head_surfaces.get(surface_id).copied() {
            return Err(format!(
                "surface registration {surface_id} changed without a corresponding approved subject change"
            ));
        }
    }

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
        let approval = require_approval_scope(
            verified,
            &registration.approval_id,
            "surface.mapping",
            &context,
        )?;
        require_source_pr(approval, registration.source_pr, &context)?;
        require_subject_surface(
            approval,
            base_surfaces.get(surface_id).copied(),
            surface,
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
        if added_files_with_surface.contains(path) {
            continue;
        }
        require_bound_added_file(verified, base, path)?;
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
    if approvals.values().any(|approval| {
        !approval.approval.retired && approval.approval.scope.iter().any(|item| item == scope)
    }) {
        Ok(())
    } else {
        Err(format!(
            "{context} is not covered by any externally verified {scope} approval"
        ))
    }
}

fn require_approval_scope<'a>(
    approvals: &'a HashMap<String, VerifiedApproval>,
    approval_id: &str,
    scope: &str,
    context: &str,
) -> Result<&'a VerifiedApproval, String> {
    let approval = approvals
        .get(approval_id)
        .ok_or_else(|| format!("{context} references unverified approval {approval_id}"))?;
    if approval.approval.retired {
        return Err(format!(
            "{context} references retired approval {approval_id}; scope-only approvals cannot authorize new changes"
        ));
    }
    if approval.approval.scope.iter().any(|item| item == scope) {
        Ok(approval)
    } else {
        Err(format!(
            "{context} requires scope {scope}, but approval {approval_id} does not authorize it"
        ))
    }
}

fn require_source_pr(
    approval: &VerifiedApproval,
    source_pr: Option<u64>,
    context: &str,
) -> Result<(), String> {
    let binding = approval
        .approval
        .change_binding
        .as_ref()
        .ok_or_else(|| format!("{context} approval has no active change binding"))?;
    if source_pr == Some(binding.pull_request) {
        Ok(())
    } else {
        Err(format!(
            "{context} source PR {:?} does not match approved PR {}",
            source_pr, binding.pull_request
        ))
    }
}

fn require_subject_surface(
    approval: &VerifiedApproval,
    base: Option<&SurfaceSnapshot>,
    head: &SurfaceSnapshot,
    context: &str,
) -> Result<(), String> {
    let subject = approval
        .subject
        .as_ref()
        .ok_or_else(|| format!("{context} approval has no verified subject evidence"))?;
    let subject_surface = subject
        .surfaces
        .iter()
        .find(|surface| surface.surface_id == head.surface_id)
        .ok_or_else(|| {
            format!("{context} is absent from the externally approved subject commit")
        })?;
    if subject_surface != head {
        return Err(format!(
            "{context} differs from the externally approved subject commit"
        ));
    }
    if base == Some(subject_surface) {
        return Err(format!(
            "{context} was not changed by the externally approved subject commit"
        ));
    }
    if !subject.tracked_files.contains(&head.stable_path) {
        return Err(format!(
            "{context} file is absent from the approved subject Git index"
        ));
    }
    Ok(())
}

fn require_bound_added_file(
    approvals: &HashMap<String, VerifiedApproval>,
    base: &RevisionEvidence,
    path: &str,
) -> Result<(), String> {
    let approved = approvals.values().any(|approval| {
        !approval.approval.retired
            && approval
                .approval
                .scope
                .iter()
                .any(|scope| scope == "workspace.discovery")
            && approval.subject.as_ref().is_some_and(|subject| {
                !base.tracked_files.contains(path) && subject.tracked_files.contains(path)
            })
    });
    if approved {
        Ok(())
    } else {
        Err(format!(
            "new tracked file {path} is absent from every externally approved subject with workspace.discovery scope"
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
            retired: true,
            change_binding: None,
        }
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

    fn test_sha(byte: char) -> String {
        std::iter::repeat_n(byte, 40).collect()
    }

    fn bound_body(scopes: &[&str]) -> String {
        format!(
            "approved change\n\n{BINDING_START}\ntarget_repository={TARGET_REPOSITORY}\npull_request=123\nbase_sha={}\nsubject_sha={}\nscopes={}\n{BINDING_END}",
            test_sha('a'),
            test_sha('b'),
            scopes.join(",")
        )
    }

    fn bound_approval(scopes: &[&str]) -> SurfaceApproval {
        let body = bound_body(scopes);
        SurfaceApproval {
            id: "approval.issue44_r10b".to_string(),
            repository: "HS7097/ActingCommand-Workflow".to_string(),
            issue: 54,
            comment_id: 6_000_000_001,
            author: "HS7097".to_string(),
            author_id: Some(TRUSTED_APPROVER_ID),
            created_at: Some("2026-07-19T00:00:00Z".to_string()),
            updated_at: Some("2026-07-19T00:00:00Z".to_string()),
            content_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
            scope: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            retired: false,
            change_binding: Some(ApprovalChangeBinding {
                target_repository: TARGET_REPOSITORY.to_string(),
                pull_request: 123,
                base_sha: test_sha('a'),
                subject_sha: test_sha('b'),
            }),
        }
    }

    fn bound_comment(scopes: &[&str]) -> GitHubIssueComment {
        GitHubIssueComment {
            id: 6_000_000_001,
            issue_url: "https://api.github.com/repos/HS7097/ActingCommand-Workflow/issues/54"
                .to_string(),
            created_at: "2026-07-19T00:00:00Z".to_string(),
            updated_at: "2026-07-19T00:00:00Z".to_string(),
            body: bound_body(scopes),
            user: GitHubUser {
                id: TRUSTED_APPROVER_ID,
                login: "HS7097".to_string(),
            },
        }
    }

    fn verified_bound(
        approval: SurfaceApproval,
        subject: RevisionEvidence,
    ) -> HashMap<String, VerifiedApproval> {
        HashMap::from([(
            approval.id.clone(),
            VerifiedApproval {
                approval,
                subject: Some(subject),
            },
        )])
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
    fn active_approval_requires_exact_comment_change_binding() {
        let scopes = ["approval.provenance", "surface.mapping"];
        let approval = bound_approval(&scopes);
        let valid_registry = registry(approval.clone());
        let source = FakeSource {
            comments: HashMap::from([(approval.comment_id, Ok(bound_comment(&scopes)))]),
        };

        let verified = verify_remote_approvals(&valid_registry, &source).unwrap();
        assert!(verified.contains_key("approval.issue44_r10b"));

        let mut mismatched_comment = bound_comment(&scopes);
        mismatched_comment.body = mismatched_comment
            .body
            .replace("pull_request=123", "pull_request=124");
        let mut mismatched_approval = approval;
        mismatched_approval.content_sha256 =
            format!("{:x}", Sha256::digest(mismatched_comment.body.as_bytes()));
        let source = FakeSource {
            comments: HashMap::from([(mismatched_approval.comment_id, Ok(mismatched_comment))]),
        };

        let error = verify_remote_approvals(&registry(mismatched_approval), &source).unwrap_err();
        assert!(error.contains("does not match its trusted comment"));
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
    fn changed_surface_must_be_within_verified_scope() {
        let surface_approval = bound_approval(&["surface.mapping"]);
        let candidate_registry = registry(surface_approval.clone());
        let surface = SurfaceSnapshot {
            surface_id: "surface.provenance".to_string(),
            kind: "rust.item".to_string(),
            stable_path: "tools/actinglab-architecture/src/approval_provenance.rs".to_string(),
            selector: "function:verify".to_string(),
            fingerprint: "a".repeat(64),
        };
        let path = surface.stable_path.clone();
        let registered = crate::generic_domain::ProtectedSurface {
            surface_id: surface.surface_id.clone(),
            kind: surface.kind.clone(),
            stable_path: surface.stable_path.clone(),
            selector: surface.selector.clone(),
            concept_ids: vec!["decision.approval".to_string()],
            fingerprint: surface.fingerprint.clone(),
            approval_id: surface_approval.id.clone(),
            source_issue: 44,
            source_pr: Some(123),
        };
        let base = RevisionEvidence {
            registry: registry(approval(&["surface.mapping"])),
            surfaces: Vec::new(),
            tracked_files: BTreeSet::new(),
        };
        let mut head_registry = candidate_registry;
        head_registry.surface.push(registered);
        let subject = RevisionEvidence {
            registry: registry(surface_approval.clone()),
            surfaces: vec![surface.clone()],
            tracked_files: BTreeSet::from([path.clone()]),
        };
        let verified = verified_bound(surface_approval, subject);
        let head = RevisionEvidence {
            registry: head_registry,
            surfaces: vec![surface],
            tracked_files: BTreeSet::from([path]),
        };

        let error = validate_surface_delta(&base, &head, &verified).unwrap_err();

        assert!(error.contains("approval.provenance"));
    }

    #[test]
    fn scope_only_approval_reuse_cannot_authorize_poison_c() {
        let old_approval = approval(&["surface.mapping"]);
        let base_surface = SurfaceSnapshot {
            surface_id: "surface.readme".to_string(),
            kind: "text_record".to_string(),
            stable_path: "README.md".to_string(),
            selector: "line:trust".to_string(),
            fingerprint: "a".repeat(64),
        };
        let mut head_surface = base_surface.clone();
        head_surface.fingerprint = "b".repeat(64);
        let registration = |surface: &SurfaceSnapshot| crate::generic_domain::ProtectedSurface {
            surface_id: surface.surface_id.clone(),
            kind: surface.kind.clone(),
            stable_path: surface.stable_path.clone(),
            selector: surface.selector.clone(),
            concept_ids: vec!["structure.value".to_string()],
            fingerprint: surface.fingerprint.clone(),
            approval_id: old_approval.id.clone(),
            source_issue: 44,
            source_pr: Some(112),
        };
        let mut base_registry = registry(old_approval.clone());
        base_registry.surface.push(registration(&base_surface));
        let mut head_registry = registry(old_approval.clone());
        head_registry.surface.push(registration(&head_surface));
        let base = RevisionEvidence {
            registry: base_registry,
            surfaces: vec![base_surface],
            tracked_files: BTreeSet::from(["README.md".to_string()]),
        };
        let head = RevisionEvidence {
            registry: head_registry,
            surfaces: vec![head_surface],
            tracked_files: BTreeSet::from(["README.md".to_string()]),
        };
        let verified = HashMap::from([(
            old_approval.id.clone(),
            VerifiedApproval {
                approval: old_approval,
                subject: None,
            },
        )]);

        let error = validate_surface_delta(&base, &head, &verified).unwrap_err();

        assert!(error.contains("retired approval"));
        assert!(error.contains("scope-only approvals cannot authorize new changes"));
    }
}
