// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use actingcommand_actinglab_architecture::trusted_provenance::{
    DEFAULT_WORKFLOW_ISSUE, GhApprovalCommentSource, TrustedProvenanceReport,
    TrustedProvenanceRequest, verify_trusted_provenance,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL: {}", public_failure_message(&error));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let request = parse_args(env::args().skip(1))?;
    let report = verify_trusted_provenance(&workspace_root()?, &request, &GhApprovalCommentSource)?;
    println!("{}", success_message(&report));
    Ok(())
}

fn success_message(report: &TrustedProvenanceReport) -> String {
    format!(
        "trusted provenance verified: head={}, changed_paths={}",
        report.head_sha,
        report.changed_paths.len()
    )
}

fn public_failure_message(error: &str) -> &'static str {
    let code = error.split_once(':').map_or(error, |(code, _)| code);
    match code {
        "TP_API_FAILED" => "TP_API_FAILED: Workflow comment verification failed",
        "TP_MARKER_HEADER_INVALID" => "TP_MARKER_HEADER_INVALID: trusted marker header is invalid",
        "TP_MARKER_BASE_MISMATCH" => {
            "TP_MARKER_BASE_MISMATCH: trusted marker base_sha does not bind the requested base"
        }
        "TP_MARKER_NOT_FOUND" => {
            "TP_MARKER_NOT_FOUND: no trusted marker matches the current request"
        }
        "TP_MARKER_CONFLICT" => "TP_MARKER_CONFLICT: trusted marker candidates conflict",
        "TP_MARKER_EDITED" => "TP_MARKER_EDITED: trusted marker must be immutable",
        "TP_MARKER_BODY_INVALID" => "TP_MARKER_BODY_INVALID: trusted marker body is invalid",
        "TP_MARKER_HEAD_MISMATCH" => {
            "TP_MARKER_HEAD_MISMATCH: trusted marker does not bind the requested head"
        }
        "TP_MARKER_SOURCE_INVALID" => "TP_MARKER_SOURCE_INVALID: trusted marker source is invalid",
        _ => "TP_VERIFY_FAILED: trusted provenance verification failed",
    }
}

fn parse_args(
    arguments: impl IntoIterator<Item = String>,
) -> Result<TrustedProvenanceRequest, String> {
    let mut values = BTreeMap::new();
    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        if ![
            "--repository",
            "--base-ref",
            "--base-protected",
            "--base",
            "--head",
            "--pull-request",
            "--trusted-verifier-sha",
            "--workflow-issue",
        ]
        .contains(&flag.as_str())
        {
            return Err(format!("unknown argument {flag}"));
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        if values.insert(flag.clone(), value).is_some() {
            return Err(format!("duplicate argument {flag}"));
        }
    }

    let get = |flag: &str| {
        values
            .get(flag)
            .cloned()
            .ok_or_else(|| format!("missing {flag}; {}", usage()))
    };
    let base_protected = match get("--base-protected")?.as_str() {
        "true" => true,
        "false" => false,
        value => return Err(format!("invalid --base-protected value {value:?}")),
    };
    let pull_request = get("--pull-request")?
        .parse::<u64>()
        .map_err(|error| format!("invalid --pull-request value: {error}"))?;
    let workflow_issue = match values.get("--workflow-issue") {
        Some(value) => value
            .parse::<u64>()
            .map_err(|error| format!("invalid --workflow-issue value: {error}"))?,
        None => DEFAULT_WORKFLOW_ISSUE,
    };

    Ok(TrustedProvenanceRequest {
        repository: get("--repository")?,
        base_ref: get("--base-ref")?,
        base_protected,
        base_sha: get("--base")?,
        head_sha: get("--head")?,
        pull_request,
        trusted_verifier_sha: get("--trusted-verifier-sha")?,
        workflow_issue,
    })
}

fn usage() -> &'static str {
    "usage: trusted-provenance-guard --repository <owner/repo> --base-ref <name> --base-protected <true|false> --base <full-sha> --head <full-sha> --pull-request <number> --trusted-verifier-sha <full-sha> [--workflow-issue <number>]"
}

fn workspace_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "architecture tool must live at tools/<name>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_require_every_unique_boundary_value() {
        let request = parse_args(
            [
                "--repository",
                "owner/repo",
                "--base-ref",
                "main",
                "--base-protected",
                "true",
                "--base",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "--head",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "--pull-request",
                "123",
                "--trusted-verifier-sha",
                "cccccccccccccccccccccccccccccccccccccccc",
                "--workflow-issue",
                "65",
            ]
            .map(str::to_string),
        )
        .unwrap();
        assert!(request.base_protected);
        assert_eq!(request.pull_request, 123);
        assert_eq!(request.workflow_issue, 65);

        let error = parse_args(
            ["--repository", "owner/repo", "--repository", "other/repo"].map(str::to_string),
        )
        .unwrap_err();
        assert!(error.contains("duplicate"));
    }

    #[test]
    fn public_messages_never_include_private_marker_metadata() {
        const PRIVATE_SENTINEL: &str = "PRIVATE_MARKER_SENTINEL_DO_NOT_LOG";
        let report = TrustedProvenanceReport {
            approval_comment_id: 987_654_321,
            sequence: 42,
            head_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            scopes: vec![PRIVATE_SENTINEL.to_string()],
            changed_paths: vec!["src/lib.rs".to_string()],
        };

        let success = success_message(&report);
        assert!(success.contains(&report.head_sha));
        assert!(success.contains("changed_paths=1"));
        assert!(!success.contains(PRIVATE_SENTINEL));
        assert!(!success.contains("987654321"));
        assert!(!success.contains("sequence"));
        assert!(!success.contains("scopes"));

        for error in [
            format!("TP_MARKER_BODY_INVALID: {PRIVATE_SENTINEL}"),
            format!("TP_MARKER_HEADER_INVALID: {PRIVATE_SENTINEL}"),
            PRIVATE_SENTINEL.to_string(),
        ] {
            let public = public_failure_message(&error);
            assert!(!public.contains(PRIVATE_SENTINEL));
        }
    }
}
