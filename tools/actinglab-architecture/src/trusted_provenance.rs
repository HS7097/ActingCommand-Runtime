// SPDX-License-Identifier: AGPL-3.0-only

//! Trusted pull-request provenance verification built outside the candidate revision.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

pub const TARGET_REPOSITORY: &str = "HS7097/ActingCommand-Runtime";
pub const WORKFLOW_REPOSITORY: &str = "HS7097/ActingCommand-Workflow";
pub const WORKFLOW_ISSUE: u64 = 54;
pub const TRUSTED_APPROVER_ID: u64 = 103_177_863;

const TRUSTED_APPROVER_LOGIN: &str = "HS7097";
const BINDING_START: &str = "<!-- actingcommand-approval-binding-v1";
const BINDING_END: &str = "-->";
const POST_SUBJECT_METADATA_PATHS: &[&str] = &[
    "tools/actinglab-architecture/generic-domain-v2.toml",
    "tools/actinglab-architecture/generic-domain-surfaces-v2.jsonl",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProvenanceRequest {
    pub repository: String,
    pub base_ref: String,
    pub base_protected: bool,
    pub base_sha: String,
    pub head_sha: String,
    pub pull_request: u64,
    pub trusted_verifier_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProvenanceReport {
    pub approval_comment_id: u64,
    pub subject_sha: String,
    pub scopes: Vec<String>,
    pub post_subject_paths: Vec<String>,
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
    fn fetch_comments(
        &self,
        repository: &str,
        issue: u64,
    ) -> Result<Vec<GitHubIssueComment>, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GhApprovalCommentSource;

impl ApprovalCommentSource for GhApprovalCommentSource {
    fn fetch_comments(
        &self,
        repository: &str,
        issue: u64,
    ) -> Result<Vec<GitHubIssueComment>, String> {
        let endpoint = format!("repos/{repository}/issues/{issue}/comments?per_page=100");
        let output = Command::new("gh")
            .env("GH_PROMPT_DISABLED", "1")
            .args([
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                "--paginate",
                "--slurp",
                &endpoint,
            ])
            .output()
            .map_err(|error| format!("trusted provenance API client failed to start: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "trusted provenance API request failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let pages = serde_json::from_slice::<Vec<Vec<GitHubIssueComment>>>(&output.stdout)
            .map_err(|error| format!("trusted provenance API returned invalid JSON: {error}"))?;
        Ok(pages.into_iter().flatten().collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangeBinding {
    target_repository: String,
    pull_request: u64,
    base_sha: String,
    subject_sha: String,
    scopes: Vec<String>,
}

/// Verifies a PR against a protected main base and an immutable Workflow approval marker.
pub fn verify_trusted_provenance(
    repository_root: &Path,
    request: &TrustedProvenanceRequest,
    source: &impl ApprovalCommentSource,
) -> Result<TrustedProvenanceReport, String> {
    validate_request_boundary(request)?;
    let root = canonical_repository_root(repository_root)?;
    let trusted_verifier_sha = resolve_full_commit(&root, &request.trusted_verifier_sha)?;
    let current_head = current_head(&root)?;
    if current_head != trusted_verifier_sha {
        return Err(format!(
            "trusted verifier checkout is {current_head}, expected {trusted_verifier_sha}"
        ));
    }

    let base = resolve_full_commit(&root, &request.base_sha)?;
    let head = resolve_full_commit(&root, &request.head_sha)?;
    ensure_ancestor(&root, &base, &head, "base", "head")?;

    let comments = source.fetch_comments(WORKFLOW_REPOSITORY, WORKFLOW_ISSUE)?;
    let (comment_id, binding) = select_latest_binding(&comments, request, &base)?;
    let subject = resolve_full_commit(&root, &binding.subject_sha)?;
    ensure_ancestor(&root, &base, &subject, "base", "approved subject")?;
    ensure_ancestor(&root, &subject, &head, "approved subject", "head")?;

    let post_subject_paths = if subject == head {
        Vec::new()
    } else {
        validate_post_subject_metadata(&root, &subject, &head)?
    };

    Ok(TrustedProvenanceReport {
        approval_comment_id: comment_id,
        subject_sha: subject,
        scopes: binding.scopes,
        post_subject_paths,
    })
}

fn validate_request_boundary(request: &TrustedProvenanceRequest) -> Result<(), String> {
    if request.repository != TARGET_REPOSITORY {
        return Err(format!(
            "trusted provenance repository is {}, expected {TARGET_REPOSITORY}",
            request.repository
        ));
    }
    if request.base_ref != "main" {
        return Err(format!(
            "trusted provenance base ref is {}, expected main",
            request.base_ref
        ));
    }
    if !request.base_protected {
        return Err("trusted provenance requires a protected main base".to_string());
    }
    if request.pull_request == 0 {
        return Err("trusted provenance pull request must be non-zero".to_string());
    }
    Ok(())
}

fn select_latest_binding(
    comments: &[GitHubIssueComment],
    request: &TrustedProvenanceRequest,
    base: &str,
) -> Result<(u64, ChangeBinding), String> {
    let expected_issue_url =
        format!("https://api.github.com/repos/{WORKFLOW_REPOSITORY}/issues/{WORKFLOW_ISSUE}");
    let mut matches = Vec::new();
    for comment in comments {
        if !comment
            .body
            .lines()
            .any(|line| line.trim() == BINDING_START)
        {
            continue;
        }
        if comment.user.id != TRUSTED_APPROVER_ID || comment.user.login != TRUSTED_APPROVER_LOGIN {
            continue;
        }
        if comment.issue_url != expected_issue_url {
            return Err(format!(
                "trusted approval comment {} belongs to {}, expected {expected_issue_url}",
                comment.id, comment.issue_url
            ));
        }
        if comment.created_at != comment.updated_at {
            return Err(format!(
                "trusted approval comment {} was edited; publish a new immutable binding comment",
                comment.id
            ));
        }
        let binding = parse_change_binding(&comment.body)
            .map_err(|error| format!("trusted approval comment {}: {error}", comment.id))?;
        if binding.target_repository == request.repository
            && binding.pull_request == request.pull_request
            && binding.base_sha == base
        {
            matches.push((comment.created_at.as_str(), comment.id, binding));
        }
    }
    matches.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    matches
        .pop()
        .map(|(_, id, binding)| (id, binding))
        .ok_or_else(|| {
            format!(
                "no trusted approval binding matches repository {}, pull request {}, and base {base}",
                request.repository, request.pull_request
            )
        })
}

fn parse_change_binding(body: &str) -> Result<ChangeBinding, String> {
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
        .ok_or_else(|| "approval binding has no closing -->".to_string())?;
    if lines[end + 1..].contains(&BINDING_END) {
        return Err("approval binding contains an extra closing -->".to_string());
    }

    let mut fields = BTreeMap::new();
    for line in &lines[start + 1..end] {
        if line.is_empty() {
            return Err("approval binding contains an empty line".to_string());
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("approval binding line is not key=value: {line}"))?;
        if key.is_empty() || value.is_empty() {
            return Err(format!(
                "approval binding line has an empty key or value: {line}"
            ));
        }
        if fields.insert(key, value).is_some() {
            return Err(format!("approval binding repeats field {key}"));
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
            "approval binding fields are {actual:?}, expected {expected:?}"
        ));
    }

    let pull_request = fields["pull_request"]
        .parse::<u64>()
        .map_err(|error| format!("approval binding pull_request is invalid: {error}"))?;
    if pull_request == 0 {
        return Err("approval binding pull_request must be non-zero".to_string());
    }
    for field in ["base_sha", "subject_sha"] {
        if !is_full_sha(fields[field]) {
            return Err(format!(
                "approval binding {field} must be a lowercase full commit SHA"
            ));
        }
    }
    let scopes = fields["scopes"]
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if scopes.is_empty()
        || scopes.iter().any(String::is_empty)
        || scopes
            .windows(2)
            .any(|pair| pair[0].as_str() >= pair[1].as_str())
    {
        return Err("approval binding scopes must be non-empty, unique, and sorted".to_string());
    }

    Ok(ChangeBinding {
        target_repository: fields["target_repository"].to_string(),
        pull_request,
        base_sha: fields["base_sha"].to_string(),
        subject_sha: fields["subject_sha"].to_string(),
        scopes,
    })
}

fn validate_post_subject_metadata(
    root: &Path,
    subject: &str,
    head: &str,
) -> Result<Vec<String>, String> {
    let output = run_git(
        root,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            "-z",
            subject,
            head,
            "--",
        ],
        "inspect post-subject changes",
    )?;
    let chunks = output
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>();
    if chunks.len() % 2 != 0 {
        return Err("post-subject Git status output is malformed".to_string());
    }

    let mut paths = Vec::new();
    for pair in chunks.chunks_exact(2) {
        let status = std::str::from_utf8(pair[0])
            .map_err(|error| format!("post-subject status is not UTF-8: {error}"))?;
        let path = std::str::from_utf8(pair[1])
            .map_err(|error| format!("post-subject path is not UTF-8: {error}"))?;
        if status != "M" {
            return Err(format!(
                "post-subject provenance path {path} has forbidden Git status {status}"
            ));
        }
        if !POST_SUBJECT_METADATA_PATHS.contains(&path) {
            return Err(format!(
                "non-provenance path {path} changed after the approved subject {subject}"
            ));
        }
        paths.push(path.to_string());
    }
    paths.sort();
    paths.dedup();
    if !paths
        .iter()
        .any(|path| path == "tools/actinglab-architecture/generic-domain-v2.toml")
    {
        return Err("post-subject changes do not include the provenance registry".to_string());
    }
    Ok(paths)
}

fn canonical_repository_root(path: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to resolve repository root {}: {error}",
            path.display()
        )
    })?;
    let output = run_git(
        &root,
        &["rev-parse", "--show-toplevel"],
        "verify repository root",
    )?;
    let actual = PathBuf::from(
        std::str::from_utf8(&output)
            .map_err(|error| format!("repository root is not UTF-8: {error}"))?
            .trim(),
    );
    let actual = fs::canonicalize(&actual)
        .map_err(|error| format!("failed to resolve Git repository root: {error}"))?;
    if actual != root {
        return Err(format!(
            "trusted provenance must run at repository root {}; received {}",
            actual.display(),
            root.display()
        ));
    }
    Ok(root)
}

fn current_head(root: &Path) -> Result<String, String> {
    let output = run_git(
        root,
        &["rev-parse", "HEAD"],
        "resolve trusted verifier HEAD",
    )?;
    String::from_utf8(output)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("trusted verifier HEAD is not UTF-8: {error}"))
}

fn resolve_full_commit(root: &Path, revision: &str) -> Result<String, String> {
    if !is_full_sha(revision) {
        return Err(format!(
            "commit {revision:?} must be a lowercase full 40-character SHA"
        ));
    }
    let object = format!("{revision}^{{commit}}");
    let output = run_git(root, &["rev-parse", "--verify", &object], "resolve commit")?;
    let resolved = String::from_utf8(output)
        .map_err(|error| format!("resolved commit is not UTF-8: {error}"))?
        .trim()
        .to_string();
    if resolved != revision {
        return Err(format!(
            "commit {revision} resolved unexpectedly to {resolved}"
        ));
    }
    Ok(resolved)
}

fn ensure_ancestor(
    root: &Path,
    ancestor: &str,
    descendant: &str,
    ancestor_label: &str,
    descendant_label: &str,
) -> Result<(), String> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .status()
        .map_err(|error| format!("failed to inspect commit ancestry: {error}"))?;
    if status.success() {
        Ok(())
    } else if status.code() == Some(1) {
        Err(format!(
            "trusted provenance {ancestor_label} {ancestor} is not an ancestor of {descendant_label} {descendant}"
        ))
    } else {
        Err(format!(
            "failed to inspect commit ancestry with exit status {status}"
        ))
    }
}

fn run_git(root: &Path, args: &[&str], context: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("failed to {context}: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "failed to {context}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone)]
    struct FakeCommentSource {
        comments: Vec<GitHubIssueComment>,
    }

    impl ApprovalCommentSource for FakeCommentSource {
        fn fetch_comments(
            &self,
            repository: &str,
            issue: u64,
        ) -> Result<Vec<GitHubIssueComment>, String> {
            assert_eq!(repository, WORKFLOW_REPOSITORY);
            assert_eq!(issue, WORKFLOW_ISSUE);
            Ok(self.comments.clone())
        }
    }

    struct GitFixture {
        root: PathBuf,
        trusted: String,
        base: String,
        subject: String,
        head: String,
    }

    impl GitFixture {
        fn new(post_subject_path: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "actingcommand-trusted-provenance-{}-{}",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "-b", "main"]);
            let trusted = commit_file(&root, "trusted.txt", "verifier", "trusted verifier");
            let base = commit_file(&root, post_subject_path, "baseline", "base");
            let subject = commit_file(&root, "src/lib.rs", "subject", "approved subject");
            let head = commit_file(&root, post_subject_path, "metadata", "metadata");
            git(&root, &["checkout", "--detach", &trusted]);
            Self {
                root,
                trusted,
                base,
                subject,
                head,
            }
        }

        fn request(&self) -> TrustedProvenanceRequest {
            TrustedProvenanceRequest {
                repository: TARGET_REPOSITORY.to_string(),
                base_ref: "main".to_string(),
                base_protected: true,
                base_sha: self.base.clone(),
                head_sha: self.head.clone(),
                pull_request: 123,
                trusted_verifier_sha: self.trusted.clone(),
            }
        }

        fn comment(&self, id: u64, created_at: &str, subject: &str) -> GitHubIssueComment {
            GitHubIssueComment {
                id,
                issue_url: format!(
                    "https://api.github.com/repos/{WORKFLOW_REPOSITORY}/issues/{WORKFLOW_ISSUE}"
                ),
                created_at: created_at.to_string(),
                updated_at: created_at.to_string(),
                body: binding_body(&self.base, subject),
                user: GitHubUser {
                    id: TRUSTED_APPROVER_ID,
                    login: TRUSTED_APPROVER_LOGIN.to_string(),
                },
            }
        }
    }

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn exact_binding_accepts_only_metadata_after_the_latest_subject() {
        let fixture = GitFixture::new("tools/actinglab-architecture/generic-domain-v2.toml");
        let source = FakeCommentSource {
            comments: vec![
                fixture.comment(10, "2026-07-18T20:00:00Z", &fixture.base),
                fixture.comment(11, "2026-07-18T20:01:00Z", &fixture.subject),
            ],
        };

        let report = verify_trusted_provenance(&fixture.root, &fixture.request(), &source).unwrap();

        assert_eq!(report.approval_comment_id, 11);
        assert_eq!(report.subject_sha, fixture.subject);
        assert_eq!(
            report.post_subject_paths,
            vec!["tools/actinglab-architecture/generic-domain-v2.toml"]
        );
    }

    #[test]
    fn non_main_and_unprotected_bases_fail_before_external_access() {
        let fixture = GitFixture::new("tools/actinglab-architecture/generic-domain-v2.toml");
        let source = FakeCommentSource { comments: vec![] };
        let mut request = fixture.request();
        request.base_ref = "feature".to_string();
        assert!(
            verify_trusted_provenance(&fixture.root, &request, &source)
                .unwrap_err()
                .contains("expected main")
        );
        request.base_ref = "main".to_string();
        request.base_protected = false;
        assert!(
            verify_trusted_provenance(&fixture.root, &request, &source)
                .unwrap_err()
                .contains("protected main")
        );
    }

    #[test]
    fn product_changes_after_the_subject_fail_loudly() {
        let fixture = GitFixture::new("src/after_subject.rs");
        let source = FakeCommentSource {
            comments: vec![fixture.comment(11, "2026-07-18T20:01:00Z", &fixture.subject)],
        };

        let error =
            verify_trusted_provenance(&fixture.root, &fixture.request(), &source).unwrap_err();

        assert!(error.contains("non-provenance path src/after_subject.rs"));
    }

    #[test]
    fn edited_and_malformed_trusted_markers_fail_loudly() {
        let fixture = GitFixture::new("tools/actinglab-architecture/generic-domain-v2.toml");
        let mut edited = fixture.comment(11, "2026-07-18T20:01:00Z", &fixture.subject);
        edited.updated_at = "2026-07-18T20:02:00Z".to_string();
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![edited],
            },
        )
        .unwrap_err();
        assert!(error.contains("was edited"));

        let mut malformed = fixture.comment(12, "2026-07-18T20:03:00Z", &fixture.subject);
        malformed.body = malformed.body.replace("scopes=", "scopes");
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![malformed],
            },
        )
        .unwrap_err();
        assert!(error.contains("not key=value"));
    }

    fn binding_body(base: &str, subject: &str) -> String {
        format!(
            "{BINDING_START}\n\
             target_repository={TARGET_REPOSITORY}\n\
             pull_request=123\n\
             base_sha={base}\n\
             subject_sha={subject}\n\
             scopes=approval.provenance,surface.mapping\n\
             {BINDING_END}"
        )
    }

    fn commit_file(root: &Path, path: &str, content: &str, message: &str) -> String {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        git(root, &["add", "--", path.to_str().unwrap()]);
        git(
            root,
            &[
                "-c",
                "user.name=ActingCommand Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                message,
            ],
        );
        git_output(root, &["rev-parse", "HEAD"])
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
