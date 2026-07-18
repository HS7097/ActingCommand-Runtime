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
    GENERIC_DOMAIN_REGISTRY_PATH, GenericDomainRegistry, IdentityAllowance, SurfaceApproval,
    SurfaceSnapshot, load_generic_domain_registry, validate_workspace_genericity,
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
const R10_SCOPES: &[&str] = &[
    "approval.provenance",
    "identity.allowance",
    "surface.mapping",
    "workspace.discovery",
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

#[derive(Debug)]
struct RevisionEvidence {
    registry: GenericDomainRegistry,
    surfaces: Vec<SurfaceSnapshot>,
    tracked_files: BTreeSet<String>,
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
    let verified_approvals = verify_remote_approvals(&head_evidence.registry, source)?;
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
        verified.insert(approval.id.clone(), approval.clone());
    }
    Ok(verified)
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

fn validate_registry_delta(
    base: &GenericDomainRegistry,
    head: &GenericDomainRegistry,
    verified: &HashMap<String, SurfaceApproval>,
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
    for id in base_approvals.keys() {
        if !head_approvals.contains_key(id) {
            return Err(format!(
                "approval registry removed {id} without an approved tombstone"
            ));
        }
    }

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

fn allowance_map(allowances: &[IdentityAllowance]) -> BTreeMap<&str, &IdentityAllowance> {
    allowances
        .iter()
        .map(|allowance| (allowance.id.as_str(), allowance))
        .collect()
}

fn validate_surface_delta(
    base: &RevisionEvidence,
    head: &RevisionEvidence,
    verified: &HashMap<String, SurfaceApproval>,
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
    approvals: &HashMap<String, SurfaceApproval>,
    scope: &str,
    context: &str,
) -> Result<(), String> {
    if approvals
        .values()
        .any(|approval| approval.scope.iter().any(|item| item == scope))
    {
        Ok(())
    } else {
        Err(format!(
            "{context} is not covered by any externally verified {scope} approval"
        ))
    }
}

fn require_approval_scope(
    approvals: &HashMap<String, SurfaceApproval>,
    approval_id: &str,
    scope: &str,
    context: &str,
) -> Result<(), String> {
    let approval = approvals
        .get(approval_id)
        .ok_or_else(|| format!("{context} references unverified approval {approval_id}"))?;
    if approval.scope.iter().any(|item| item == scope) {
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
    fn changed_surface_must_be_within_verified_scope() {
        let surface_approval = approval(&["surface.mapping"]);
        let candidate_registry = registry(surface_approval.clone());
        let source = FakeSource {
            comments: HashMap::from([(5011923710, Ok(comment()))]),
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
            approval_id: surface_approval.id,
            source_issue: 44,
            source_pr: Some(120),
        };
        let base = RevisionEvidence {
            registry: registry(approval(&["surface.mapping"])),
            surfaces: Vec::new(),
            tracked_files: BTreeSet::new(),
        };
        let mut head_registry = candidate_registry;
        head_registry.surface.push(registered);
        let verified = verify_remote_approvals(&head_registry, &source).unwrap();
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
