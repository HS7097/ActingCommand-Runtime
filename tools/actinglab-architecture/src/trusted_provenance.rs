// SPDX-License-Identifier: AGPL-3.0-only

//! Exact-head pull-request provenance verification built outside the candidate revision.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

pub const TARGET_REPOSITORY: &str = "HS7097/ActingCommand-Runtime";
pub const WORKFLOW_REPOSITORY: &str = "HS7097/ActingCommand-Workflow";
pub const DEFAULT_WORKFLOW_ISSUE: u64 = 65;
pub const TRUSTED_APPROVER_ID: u64 = 103_177_863;

const TRUSTED_APPROVER_LOGIN: &str = "HS7097";
const MARKER_PREFIX: &str = "<!-- actingcommand-trusted-provenance-v2";
const MARKER_END: &str = "-->";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProvenanceRequest {
    pub repository: String,
    pub base_ref: String,
    pub base_protected: bool,
    pub base_sha: String,
    pub head_sha: String,
    pub pull_request: u64,
    pub trusted_verifier_sha: String,
    pub workflow_issue: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProvenanceReport {
    pub approval_comment_id: u64,
    pub sequence: u64,
    pub head_sha: String,
    pub scopes: Vec<String>,
    pub changed_paths: Vec<String>,
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
struct MarkerHeader {
    target_repository: String,
    pull_request: u64,
    base_sha: String,
    sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderIdentityMatch {
    Indeterminate,
    Matches,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExactHeadBinding {
    header: MarkerHeader,
    head_sha: String,
    scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    mode: String,
    object_type: String,
    object_id: String,
    path: String,
}

/// Verifies one exact PR head using a verifier built from trusted history.
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
    ensure_ancestor(
        &root,
        &trusted_verifier_sha,
        &base,
        "trusted verifier",
        "base",
    )?;
    ensure_ancestor(&root, &base, &head, "base", "head")?;

    let comments = source.fetch_comments(WORKFLOW_REPOSITORY, request.workflow_issue)?;
    let (comment_id, binding) = select_target_binding(&comments, request, &base)?;
    if binding.head_sha != head {
        return Err(format!(
            "trusted provenance marker binds head {}, expected {head}",
            binding.head_sha
        ));
    }
    let changed_paths = validate_changed_objects(&root, &base, &head)?;

    Ok(TrustedProvenanceReport {
        approval_comment_id: comment_id,
        sequence: binding.header.sequence,
        head_sha: head,
        scopes: binding.scopes,
        changed_paths,
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
    if request.workflow_issue == 0 {
        return Err("trusted provenance Workflow issue must be non-zero".to_string());
    }
    Ok(())
}

fn select_target_binding(
    comments: &[GitHubIssueComment],
    request: &TrustedProvenanceRequest,
    base: &str,
) -> Result<(u64, ExactHeadBinding), String> {
    let expected_issue_url = format!(
        "https://api.github.com/repos/{WORKFLOW_REPOSITORY}/issues/{}",
        request.workflow_issue
    );
    let mut candidates = Vec::new();
    let mut malformed_targets = Vec::new();

    for comment in comments {
        if comment.user.id != TRUSTED_APPROVER_ID || comment.user.login != TRUSTED_APPROVER_LOGIN {
            continue;
        }
        for line in comment
            .body
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(MARKER_PREFIX))
        {
            let targets_request = header_targets_request(line, request, base);
            let header = match parse_marker_header(line) {
                Ok(header) => header,
                Err(error) if targets_request => {
                    malformed_targets.push((marker_sequence_hint(line), comment.id, error));
                    continue;
                }
                Err(_) => continue,
            };
            if header.target_repository != request.repository
                || header.pull_request != request.pull_request
                || header.base_sha != base
            {
                if targets_request {
                    malformed_targets.push((
                        Some(header.sequence),
                        comment.id,
                        "header identifies the current request with non-canonical identity values"
                            .to_string(),
                    ));
                }
                continue;
            }
            if comment.issue_url != expected_issue_url {
                return Err(format!(
                    "trusted provenance comment {} belongs to {}, expected {expected_issue_url}",
                    comment.id, comment.issue_url
                ));
            }
            candidates.push((header.sequence, comment.id, comment, header));
        }
    }

    let Some(highest_sequence) = candidates.iter().map(|(sequence, _, _, _)| *sequence).max()
    else {
        if let Some((_, comment_id, error)) = malformed_targets
            .iter()
            .max_by_key(|(sequence, id, _)| (sequence.unwrap_or(0), *id))
        {
            return Err(format!(
                "trusted provenance comment {comment_id} has malformed target header: {error}"
            ));
        }
        return Err(format!(
            "no trusted provenance marker matches repository {}, pull request {}, and base {base}",
            request.repository, request.pull_request
        ));
    };
    let mut selected = candidates
        .into_iter()
        .filter(|(sequence, _, _, _)| *sequence == highest_sequence)
        .collect::<Vec<_>>();
    if selected.len() != 1 {
        let ids = selected
            .iter()
            .map(|(_, id, _, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        return Err(format!(
            "trusted provenance sequence {highest_sequence} has conflicting target candidates: {ids}"
        ));
    }

    let (_, comment_id, comment, expected_header) = selected.pop().expect("length checked");
    if let Some((_, malformed_id, error)) = malformed_targets
        .iter()
        .filter(|(sequence, id, _)| {
            sequence.is_some_and(|sequence| sequence >= highest_sequence) || *id > comment_id
        })
        .max_by_key(|(sequence, id, _)| (sequence.unwrap_or(0), *id))
    {
        return Err(format!(
            "trusted provenance comment {malformed_id} has a newer malformed target header: {error}"
        ));
    }
    if comment.created_at != comment.updated_at {
        return Err(format!(
            "trusted provenance comment {comment_id} was edited; publish a higher immutable sequence"
        ));
    }
    let binding = parse_exact_head_binding(&comment.body, &expected_header)
        .map_err(|error| format!("trusted provenance comment {comment_id}: {error}"))?;
    Ok((comment_id, binding))
}

fn header_targets_request(line: &str, request: &TrustedProvenanceRequest, base: &str) -> bool {
    let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
    if tokens.first() != Some(&"<!--")
        || tokens.get(1) != Some(&"actingcommand-trusted-provenance-v2")
    {
        return false;
    }

    let identities = [
        classify_header_identity(&tokens, "target_repository", |value| {
            if value.is_empty() {
                HeaderIdentityMatch::Indeterminate
            } else if value.eq_ignore_ascii_case(&request.repository) {
                HeaderIdentityMatch::Matches
            } else {
                HeaderIdentityMatch::Other
            }
        }),
        classify_header_identity(&tokens, "pull_request", |value| {
            match value.parse::<u64>() {
                Ok(value) if value == request.pull_request => HeaderIdentityMatch::Matches,
                Ok(value) if value != 0 => HeaderIdentityMatch::Other,
                Ok(_) | Err(_) => HeaderIdentityMatch::Indeterminate,
            }
        }),
        classify_header_identity(&tokens, "base_sha", |value| {
            if value.eq_ignore_ascii_case(base) {
                HeaderIdentityMatch::Matches
            } else if is_full_sha(value) {
                HeaderIdentityMatch::Other
            } else {
                HeaderIdentityMatch::Indeterminate
            }
        }),
    ];

    // Two matching axes identify a target even when one required identity field is malformed or
    // missing; an explicit valid mismatch keeps markers for other requests isolated.
    !identities.contains(&HeaderIdentityMatch::Other)
        && identities
            .iter()
            .filter(|identity| **identity == HeaderIdentityMatch::Matches)
            .count()
            >= 2
}

fn classify_header_identity(
    tokens: &[&str],
    key: &str,
    classify: impl Fn(&str) -> HeaderIdentityMatch,
) -> HeaderIdentityMatch {
    let mut matched = false;
    let mut other = false;

    for token in tokens {
        if *token == key {
            continue;
        }
        let Some((candidate_key, value)) = token.split_once('=') else {
            continue;
        };
        if candidate_key != key {
            continue;
        }
        match classify(value) {
            HeaderIdentityMatch::Matches => matched = true,
            HeaderIdentityMatch::Other => other = true,
            HeaderIdentityMatch::Indeterminate => {}
        }
    }

    if matched {
        HeaderIdentityMatch::Matches
    } else if other {
        HeaderIdentityMatch::Other
    } else {
        HeaderIdentityMatch::Indeterminate
    }
}

fn marker_sequence_hint(line: &str) -> Option<u64> {
    line.split_ascii_whitespace()
        .filter_map(|token| token.split_once('='))
        .filter(|(key, _)| *key == "sequence")
        .filter_map(|(_, value)| value.parse::<u64>().ok())
        .filter(|value| *value != 0)
        .max()
}

fn parse_marker_header(line: &str) -> Result<MarkerHeader, String> {
    let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
    if tokens.len() != 6
        || tokens[0] != "<!--"
        || tokens[1] != "actingcommand-trusted-provenance-v2"
    {
        return Err("header must contain the normalized v2 marker and four fields".to_string());
    }

    let mut fields = BTreeMap::new();
    for token in &tokens[2..] {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("header token is not key=value: {token}"))?;
        if key.is_empty() || value.is_empty() {
            return Err(format!("header token has an empty key or value: {token}"));
        }
        if fields.insert(key, value).is_some() {
            return Err(format!("header repeats field {key}"));
        }
    }
    let expected = BTreeSet::from(["base_sha", "pull_request", "sequence", "target_repository"]);
    let actual = fields.keys().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "header fields are {actual:?}, expected {expected:?}"
        ));
    }

    let pull_request = parse_nonzero_u64(fields["pull_request"], "pull_request")?;
    let sequence = parse_nonzero_u64(fields["sequence"], "sequence")?;
    if !is_full_sha(fields["base_sha"]) {
        return Err("header base_sha must be a lowercase full commit SHA".to_string());
    }
    let header = MarkerHeader {
        target_repository: fields["target_repository"].to_string(),
        pull_request,
        base_sha: fields["base_sha"].to_string(),
        sequence,
    };
    let normalized = format_marker_header(&header);
    if line != normalized {
        return Err(format!("header is not normalized; expected {normalized}"));
    }
    Ok(header)
}

fn format_marker_header(header: &MarkerHeader) -> String {
    format!(
        "{MARKER_PREFIX} target_repository={} pull_request={} base_sha={} sequence={}",
        header.target_repository, header.pull_request, header.base_sha, header.sequence
    )
}

fn parse_nonzero_u64(value: &str, field: &str) -> Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|error| format!("header {field} is invalid: {error}"))?;
    if value == 0 {
        return Err(format!("header {field} must be non-zero"));
    }
    Ok(value)
}

fn parse_exact_head_binding(
    body: &str,
    expected_header: &MarkerHeader,
) -> Result<ExactHeadBinding, String> {
    let lines = body.lines().map(str::trim).collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with(MARKER_PREFIX).then_some(index))
        .collect::<Vec<_>>();
    if starts.len() != 1 {
        return Err(format!(
            "expected exactly one target marker block, found {}",
            starts.len()
        ));
    }
    let start = starts[0];
    let header = parse_marker_header(lines[start])?;
    if &header != expected_header {
        return Err("selected marker header changed during parsing".to_string());
    }
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| (*line == MARKER_END).then_some(index))
        .ok_or_else(|| "marker has no closing -->".to_string())?;

    let mut fields = BTreeMap::new();
    for line in &lines[start + 1..end] {
        if line.is_empty() {
            return Err("marker contains an empty field line".to_string());
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("marker line is not key=value: {line}"))?;
        if key.is_empty() || value.is_empty() {
            return Err(format!("marker line has an empty key or value: {line}"));
        }
        if fields.insert(key, value).is_some() {
            return Err(format!("marker repeats field {key}"));
        }
    }
    let expected = BTreeSet::from(["head_sha", "scopes"]);
    let actual = fields.keys().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "marker fields are {actual:?}, expected {expected:?}"
        ));
    }
    if !is_full_sha(fields["head_sha"]) {
        return Err("marker head_sha must be a lowercase full commit SHA".to_string());
    }
    let scopes = parse_scopes(fields["scopes"])?;

    Ok(ExactHeadBinding {
        header,
        head_sha: fields["head_sha"].to_string(),
        scopes,
    })
}

fn parse_scopes(value: &str) -> Result<Vec<String>, String> {
    let scopes = value
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
        return Err("marker scopes must be non-empty, unique, and sorted".to_string());
    }
    Ok(scopes)
}

fn validate_changed_objects(root: &Path, base: &str, head: &str) -> Result<Vec<String>, String> {
    let output = run_git(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "--no-renames",
            "-r",
            "-z",
            base,
            head,
        ],
        "inspect candidate tree delta",
    )?;
    let chunks = output
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>();
    if chunks.is_empty() {
        return Err("trusted provenance candidate has no changed paths".to_string());
    }
    if chunks.len() % 2 != 0 {
        return Err("candidate Git status output is malformed".to_string());
    }

    let mut paths = BTreeSet::new();
    for pair in chunks.chunks_exact(2) {
        let status = std::str::from_utf8(pair[0])
            .map_err(|error| format!("candidate Git status is not UTF-8: {error}"))?;
        let path = std::str::from_utf8(pair[1])
            .map_err(|error| format!("candidate path is not UTF-8: {error}"))?;
        if path.is_empty() {
            return Err("candidate Git status contains an empty path".to_string());
        }
        if !matches!(status, "A" | "M") {
            return Err(format!(
                "candidate path {path} has forbidden Git status {status}; only added or modified regular files are accepted"
            ));
        }
        if !paths.insert(path.to_string()) {
            return Err(format!("candidate Git status repeats path {path}"));
        }

        let head_entry = tree_entry(root, head, path)?
            .ok_or_else(|| format!("candidate head has no tree object for {path}"))?;
        validate_regular_blob(&head_entry, "candidate head")?;
        match status {
            "A" => {
                if tree_entry(root, base, path)?.is_some() {
                    return Err(format!(
                        "candidate path {path} is reported added but exists in the base tree"
                    ));
                }
            }
            "M" => {
                let base_entry = tree_entry(root, base, path)?
                    .ok_or_else(|| format!("candidate path {path} is absent from the base tree"))?;
                validate_regular_blob(&base_entry, "base")?;
                if base_entry.mode != head_entry.mode
                    || base_entry.object_type != head_entry.object_type
                {
                    return Err(format!(
                        "candidate path {path} changes Git mode/type from {}/{} to {}/{}",
                        base_entry.mode,
                        base_entry.object_type,
                        head_entry.mode,
                        head_entry.object_type
                    ));
                }
            }
            _ => unreachable!("status matched above"),
        }
    }
    Ok(paths.into_iter().collect())
}

fn tree_entry(root: &Path, revision: &str, path: &str) -> Result<Option<TreeEntry>, String> {
    let pathspec = format!(":(literal){path}");
    let output = run_git(
        root,
        &["ls-tree", "-z", revision, "--", &pathspec],
        "inspect candidate tree object",
    )?;
    let records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    if records.is_empty() {
        return Ok(None);
    }
    if records.len() != 1 {
        return Err(format!(
            "tree lookup for {path} returned {} records",
            records.len()
        ));
    }
    let record = std::str::from_utf8(records[0])
        .map_err(|error| format!("tree entry for {path} is not UTF-8: {error}"))?;
    let (metadata, actual_path) = record
        .split_once('\t')
        .ok_or_else(|| format!("tree entry for {path} is malformed"))?;
    let metadata = metadata.split_ascii_whitespace().collect::<Vec<_>>();
    if metadata.len() != 3 {
        return Err(format!("tree entry metadata for {path} is malformed"));
    }
    if actual_path != path {
        return Err(format!(
            "tree lookup for {path} returned unexpected path {actual_path}"
        ));
    }
    if !is_git_object_id(metadata[2]) {
        return Err(format!("tree entry object id for {path} is invalid"));
    }
    Ok(Some(TreeEntry {
        mode: metadata[0].to_string(),
        object_type: metadata[1].to_string(),
        object_id: metadata[2].to_string(),
        path: actual_path.to_string(),
    }))
}

fn validate_regular_blob(entry: &TreeEntry, label: &str) -> Result<(), String> {
    if entry.mode != "100644" || entry.object_type != "blob" {
        return Err(format!(
            "{label} path {} is {}/{}; trusted provenance requires 100644 blob objects",
            entry.path, entry.mode, entry.object_type
        ));
    }
    Ok(())
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
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|error| format!("failed to inspect commit ancestry: {error}"))?;
    if output.status.success() {
        Ok(())
    } else if output.status.code() == Some(1) {
        Err(format!(
            "trusted provenance {ancestor_label} {ancestor} is not an ancestor of {descendant_label} {descendant}"
        ))
    } else {
        Err(format!(
            "failed to inspect commit ancestry: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn run_git(root: &Path, args: &[&str], context: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
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

fn is_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::Stdio;
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
            assert_eq!(issue, DEFAULT_WORKFLOW_ISSUE);
            Ok(self.comments.clone())
        }
    }

    struct GitFixture {
        root: PathBuf,
        trusted: String,
        base: String,
        head: String,
    }

    impl GitFixture {
        fn regular() -> Self {
            let root = new_repository();
            let trusted = commit_file(&root, "trusted.txt", "verifier", "trusted verifier");
            let base = commit_file(&root, "src/lib.rs", "base", "base");
            let head = commit_file(&root, "src/lib.rs", "head", "candidate");
            git(&root, &["checkout", "--detach", &trusted]);
            Self {
                root,
                trusted,
                base,
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
                workflow_issue: DEFAULT_WORKFLOW_ISSUE,
            }
        }

        fn comment(&self, id: u64, sequence: u64) -> GitHubIssueComment {
            comment(id, sequence, &self.base, &self.head)
        }
    }

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn exact_head_binding_accepts_regular_blob_changes() {
        let fixture = GitFixture::regular();
        let source = FakeCommentSource {
            comments: vec![fixture.comment(10, 1)],
        };

        let report = verify_trusted_provenance(&fixture.root, &fixture.request(), &source).unwrap();

        assert_eq!(report.approval_comment_id, 10);
        assert_eq!(report.sequence, 1);
        assert_eq!(report.head_sha, fixture.head);
        assert_eq!(report.changed_paths, vec!["src/lib.rs"]);
    }

    #[test]
    fn request_and_exact_binding_drift_fail_loudly() {
        let fixture = GitFixture::regular();
        let source = FakeCommentSource {
            comments: vec![fixture.comment(10, 1)],
        };

        let mut request = fixture.request();
        request.repository = "other/repository".to_string();
        assert!(
            verify_trusted_provenance(&fixture.root, &request, &source)
                .unwrap_err()
                .contains("expected")
        );

        let mut wrong_head = fixture.comment(11, 2);
        wrong_head.body = marker_body(2, &fixture.base, &fixture.base);
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![wrong_head],
            },
        )
        .unwrap_err();
        assert!(error.contains("binds head"));

        let mut wrong_pr = fixture.comment(12, 3);
        wrong_pr.body = wrong_pr
            .body
            .replace("pull_request=123", "pull_request=124");
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![wrong_pr],
            },
        )
        .unwrap_err();
        assert!(error.contains("no trusted provenance marker"));

        let mut wrong_base = fixture.comment(13, 4);
        wrong_base.body = wrong_base.body.replace(&fixture.base, &fixture.trusted);
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![wrong_base],
            },
        )
        .unwrap_err();
        assert!(error.contains("no trusted provenance marker"));
    }

    #[test]
    fn unrelated_malformed_history_does_not_block_the_target() {
        let fixture = GitFixture::regular();
        let same_repository_other_pull_request = GitHubIssueComment {
            id: 8,
            issue_url: issue_url(),
            created_at: "2026-07-19T00:00:00Z".to_string(),
            updated_at: "2026-07-19T00:01:00Z".to_string(),
            body: format!(
                "{MARKER_PREFIX} target_repository={TARGET_REPOSITORY} pull_request=999 sequence=broken\nhead_sha=broken"
            ),
            user: trusted_user(),
        };
        let other_repository_same_pull_request = GitHubIssueComment {
            id: 9,
            issue_url: issue_url(),
            created_at: "2026-07-19T00:00:00Z".to_string(),
            updated_at: "2026-07-19T00:01:00Z".to_string(),
            body: format!(
                "{MARKER_PREFIX} target_repository=other/repo pull_request=999 base_sha=broken sequence=nope\nhead_sha=broken"
            ),
            user: trusted_user(),
        };
        let source = FakeCommentSource {
            comments: vec![
                same_repository_other_pull_request,
                other_repository_same_pull_request,
                fixture.comment(10, 2),
            ],
        };

        let report = verify_trusted_provenance(&fixture.root, &fixture.request(), &source).unwrap();

        assert_eq!(report.approval_comment_id, 10);
    }

    #[test]
    fn newer_target_header_missing_any_required_field_fails_loudly() {
        let fixture = GitFixture::regular();
        let valid = fixture.comment(10, 1);

        for field in ["target_repository", "pull_request", "base_sha", "sequence"] {
            let mut malformed = fixture.comment(11, 2);
            malformed.body = remove_header_field(&malformed.body, field);

            let error = verify_trusted_provenance(
                &fixture.root,
                &fixture.request(),
                &FakeCommentSource {
                    comments: vec![valid.clone(), malformed],
                },
            )
            .unwrap_err();

            assert!(
                error.contains("newer malformed target header"),
                "missing {field}: {error}"
            );
        }
    }

    #[test]
    fn duplicate_and_conflicting_target_header_fields_fail_loudly() {
        let fixture = GitFixture::regular();
        let valid = fixture.comment(10, 1);
        let repository = format!("target_repository={TARGET_REPOSITORY}");
        let pull_request = "pull_request=123".to_string();
        let base = format!("base_sha={}", fixture.base);
        let sequence = "sequence=2".to_string();
        let cases = vec![
            (
                "duplicate repository",
                repository.clone(),
                format!("{repository} {repository}"),
            ),
            (
                "conflicting repository",
                repository.clone(),
                format!("{repository} target_repository=other/repo"),
            ),
            (
                "duplicate pull request",
                pull_request.clone(),
                format!("{pull_request} {pull_request}"),
            ),
            (
                "conflicting pull request",
                pull_request.clone(),
                format!("{pull_request} pull_request=124"),
            ),
            ("duplicate base", base.clone(), format!("{base} {base}")),
            (
                "conflicting base",
                base.clone(),
                format!("{base} base_sha={}", fixture.trusted),
            ),
            (
                "duplicate sequence",
                sequence.clone(),
                format!("{sequence} {sequence}"),
            ),
            (
                "conflicting sequence",
                sequence.clone(),
                format!("{sequence} sequence=3"),
            ),
        ];

        for (label, needle, replacement) in cases {
            let mut malformed = fixture.comment(11, 2);
            malformed.body = malformed.body.replace(&needle, &replacement);

            let error = verify_trusted_provenance(
                &fixture.root,
                &fixture.request(),
                &FakeCommentSource {
                    comments: vec![valid.clone(), malformed],
                },
            )
            .unwrap_err();

            assert!(
                error.contains("newer malformed target header"),
                "{label}: {error}"
            );
        }
    }

    #[test]
    fn noncanonical_target_identity_values_fail_loudly() {
        let fixture = GitFixture::regular();
        let valid = fixture.comment(10, 1);
        let cases = [
            (
                "repository case",
                format!("target_repository={TARGET_REPOSITORY}"),
                "target_repository=hs7097/actingcommand-runtime".to_string(),
            ),
            (
                "pull request leading zeros",
                "pull_request=123".to_string(),
                "pull_request=00123".to_string(),
            ),
            (
                "base uppercase",
                format!("base_sha={}", fixture.base),
                format!("base_sha={}", fixture.base.to_ascii_uppercase()),
            ),
        ];

        for (label, needle, replacement) in cases {
            let mut malformed = fixture.comment(11, 2);
            malformed.body = malformed.body.replace(&needle, &replacement);

            let error = verify_trusted_provenance(
                &fixture.root,
                &fixture.request(),
                &FakeCommentSource {
                    comments: vec![valid.clone(), malformed],
                },
            )
            .unwrap_err();

            assert!(
                error.contains("newer malformed target header"),
                "{label}: {error}"
            );
        }
    }

    #[test]
    fn selected_target_malformed_edited_or_conflicting_fails_loudly() {
        let fixture = GitFixture::regular();
        let mut malformed = fixture.comment(10, 1);
        malformed.body = malformed.body.replace("head_sha=", "head_sha");
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![malformed],
            },
        )
        .unwrap_err();
        assert!(error.contains("not key=value"));

        let mut edited = fixture.comment(11, 2);
        edited.updated_at = "2026-07-19T00:01:00Z".to_string();
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![edited],
            },
        )
        .unwrap_err();
        assert!(error.contains("was edited"));

        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![fixture.comment(12, 3), fixture.comment(13, 3)],
            },
        )
        .unwrap_err();
        assert!(error.contains("conflicting target candidates"));
    }

    #[test]
    fn higher_sequence_recovers_from_an_older_malformed_body() {
        let fixture = GitFixture::regular();
        let mut old = fixture.comment(10, 1);
        old.body = old.body.replace("scopes=", "scopes");
        old.updated_at = "2026-07-19T00:01:00Z".to_string();
        let source = FakeCommentSource {
            comments: vec![old, fixture.comment(11, 2)],
        };

        let report = verify_trusted_provenance(&fixture.root, &fixture.request(), &source).unwrap();

        assert_eq!(report.approval_comment_id, 11);
        assert_eq!(report.sequence, 2);
    }

    #[test]
    fn newer_normalized_marker_recovers_from_an_older_malformed_target_header() {
        let fixture = GitFixture::regular();
        let mut old = fixture.comment(10, 1);
        old.body = old.body.replace("sequence=1", "sequence=broken");
        let source = FakeCommentSource {
            comments: vec![old, fixture.comment(11, 2)],
        };

        let report = verify_trusted_provenance(&fixture.root, &fixture.request(), &source).unwrap();

        assert_eq!(report.approval_comment_id, 11);

        let mut newer_malformed = fixture.comment(12, 3);
        newer_malformed.body = newer_malformed
            .body
            .replace("sequence=3", "sequence=broken");
        let error = verify_trusted_provenance(
            &fixture.root,
            &fixture.request(),
            &FakeCommentSource {
                comments: vec![fixture.comment(11, 2), newer_malformed],
            },
        )
        .unwrap_err();
        assert!(error.contains("newer malformed target header"));
    }

    #[test]
    fn highest_valid_monotonic_sequence_wins() {
        let fixture = GitFixture::regular();
        let source = FakeCommentSource {
            comments: vec![fixture.comment(10, 1), fixture.comment(11, 2)],
        };

        let report = verify_trusted_provenance(&fixture.root, &fixture.request(), &source).unwrap();

        assert_eq!(report.approval_comment_id, 11);
        assert_eq!(report.sequence, 2);
    }

    #[test]
    fn added_symlink_executable_and_gitlink_objects_fail_loudly() {
        for kind in ["symlink", "executable", "gitlink"] {
            let root = new_repository();
            let trusted = commit_file(&root, "trusted.txt", "verifier", "trusted verifier");
            let base = commit_file(&root, "src/lib.rs", "base", "base");
            let head = match kind {
                "symlink" => commit_index_entry(&root, "link", "120000", "src/lib.rs", "symlink"),
                "executable" => {
                    fs::write(root.join("tool.sh"), "exit 0\n").unwrap();
                    git(&root, &["add", "--", "tool.sh"]);
                    git(&root, &["update-index", "--chmod=+x", "tool.sh"]);
                    commit_index(&root, "executable")
                }
                "gitlink" => commit_gitlink(&root, "vendor/runtime", &base),
                _ => unreachable!(),
            };
            git(&root, &["checkout", "--detach", &trusted]);
            let request = request(&trusted, &base, &head);
            let source = FakeCommentSource {
                comments: vec![comment(10, 1, &base, &head)],
            };

            let error = verify_trusted_provenance(&root, &request, &source).unwrap_err();

            assert!(
                error.contains("requires 100644 blob objects"),
                "{kind}: {error}"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn deletion_rename_and_mode_change_fail_loudly() {
        for kind in ["deletion", "rename", "mode-change"] {
            let root = new_repository();
            let trusted = commit_file(&root, "trusted.txt", "verifier", "trusted verifier");
            let base = commit_file(&root, "src/lib.rs", "base", "base");
            match kind {
                "deletion" => git(&root, &["rm", "--", "src/lib.rs"]),
                "rename" => git(&root, &["mv", "src/lib.rs", "src/renamed.rs"]),
                "mode-change" => git(&root, &["update-index", "--chmod=+x", "src/lib.rs"]),
                _ => unreachable!(),
            }
            let head = commit_index(&root, kind);
            git(&root, &["checkout", "--detach", &trusted]);
            let request = request(&trusted, &base, &head);
            let source = FakeCommentSource {
                comments: vec![comment(10, 1, &base, &head)],
            };

            let error = verify_trusted_provenance(&root, &request, &source).unwrap_err();

            assert!(
                error.contains("forbidden Git status")
                    || error.contains("requires 100644 blob objects"),
                "{kind}: {error}"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn candidate_content_is_inspected_as_data_and_never_executed() {
        let root = new_repository();
        let trusted = commit_file(&root, "trusted.txt", "verifier", "trusted verifier");
        let base = commit_file(&root, "src/lib.rs", "base", "base");
        let sentinel = root.join("candidate-executed");
        let hostile = format!("touch {}\n", sentinel.display());
        let head = commit_file(&root, "candidate.sh", &hostile, "hostile candidate");
        git(&root, &["checkout", "--detach", &trusted]);
        let request = request(&trusted, &base, &head);
        let source = FakeCommentSource {
            comments: vec![comment(10, 1, &base, &head)],
        };

        verify_trusted_provenance(&root, &request, &source).unwrap();

        assert!(!sentinel.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn request(trusted: &str, base: &str, head: &str) -> TrustedProvenanceRequest {
        TrustedProvenanceRequest {
            repository: TARGET_REPOSITORY.to_string(),
            base_ref: "main".to_string(),
            base_protected: true,
            base_sha: base.to_string(),
            head_sha: head.to_string(),
            pull_request: 123,
            trusted_verifier_sha: trusted.to_string(),
            workflow_issue: DEFAULT_WORKFLOW_ISSUE,
        }
    }

    fn comment(id: u64, sequence: u64, base: &str, head: &str) -> GitHubIssueComment {
        let timestamp = format!("2026-07-19T00:{id:02}:00Z");
        GitHubIssueComment {
            id,
            issue_url: issue_url(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
            body: marker_body(sequence, base, head),
            user: trusted_user(),
        }
    }

    fn trusted_user() -> GitHubUser {
        GitHubUser {
            id: TRUSTED_APPROVER_ID,
            login: TRUSTED_APPROVER_LOGIN.to_string(),
        }
    }

    fn issue_url() -> String {
        format!(
            "https://api.github.com/repos/{WORKFLOW_REPOSITORY}/issues/{DEFAULT_WORKFLOW_ISSUE}"
        )
    }

    fn marker_body(sequence: u64, base: &str, head: &str) -> String {
        let header = MarkerHeader {
            target_repository: TARGET_REPOSITORY.to_string(),
            pull_request: 123,
            base_sha: base.to_string(),
            sequence,
        };
        format!(
            "Approved exact candidate.\n\n{}\nhead_sha={head}\nscopes=approval.provenance,surface.mapping\n{MARKER_END}",
            format_marker_header(&header)
        )
    }

    fn remove_header_field(body: &str, field: &str) -> String {
        let prefix = format!("{field}=");
        body.lines()
            .map(|line| {
                if line.starts_with(MARKER_PREFIX) {
                    line.split_ascii_whitespace()
                        .filter(|token| !token.starts_with(&prefix))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn new_repository() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "actingcommand-trusted-provenance-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        root
    }

    fn commit_file(root: &Path, path: &str, content: &str, message: &str) -> String {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
        git(root, &["add", "--", relative]);
        commit_index(root, message)
    }

    fn commit_index_entry(
        root: &Path,
        path: &str,
        mode: &str,
        content: &str,
        message: &str,
    ) -> String {
        let object = hash_blob(root, content.as_bytes());
        let cache_info = format!("{mode},{object},{path}");
        git(root, &["update-index", "--add", "--cacheinfo", &cache_info]);
        commit_index(root, message)
    }

    fn commit_gitlink(root: &Path, path: &str, commit: &str) -> String {
        let cache_info = format!("160000,{commit},{path}");
        git(root, &["update-index", "--add", "--cacheinfo", &cache_info]);
        commit_index(root, "gitlink")
    }

    fn hash_blob(root: &Path, bytes: &[u8]) -> String {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn commit_index(root: &Path, message: &str) -> String {
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
