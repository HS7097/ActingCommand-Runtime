// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use actingcommand_actinglab_architecture::trusted_provenance::{
    GhApprovalCommentSource, TrustedProvenanceRequest, verify_trusted_provenance,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let request = parse_args(env::args().skip(1))?;
    let report = verify_trusted_provenance(&workspace_root()?, &request, &GhApprovalCommentSource)?;
    println!(
        "trusted provenance verified: comment={}, subject={}, scopes={}, post_subject_paths={}",
        report.approval_comment_id,
        report.subject_sha,
        report.scopes.join(","),
        report.post_subject_paths.len()
    );
    Ok(())
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

    Ok(TrustedProvenanceRequest {
        repository: get("--repository")?,
        base_ref: get("--base-ref")?,
        base_protected,
        base_sha: get("--base")?,
        head_sha: get("--head")?,
        pull_request,
        trusted_verifier_sha: get("--trusted-verifier-sha")?,
    })
}

fn usage() -> &'static str {
    "usage: trusted-provenance-guard --repository <owner/repo> --base-ref <name> --base-protected <true|false> --base <full-sha> --head <full-sha> --pull-request <number> --trusted-verifier-sha <full-sha>"
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
            ]
            .map(str::to_string),
        )
        .unwrap();
        assert!(request.base_protected);
        assert_eq!(request.pull_request, 123);

        let error = parse_args(
            ["--repository", "owner/repo", "--repository", "other/repo"].map(str::to_string),
        )
        .unwrap_err();
        assert!(error.contains("duplicate"));
    }
}
