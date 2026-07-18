// SPDX-License-Identifier: AGPL-3.0-only

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use actingcommand_actinglab_architecture::approval_provenance::{
    GhApprovalCommentSource, verify_approval_provenance,
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
    let (base, head, pull_request) = parse_args(env::args().skip(1))?;
    let report = verify_approval_provenance(
        &workspace_root()?,
        &base,
        &head,
        pull_request,
        &GhApprovalCommentSource,
    )?;
    println!(
        "approval provenance verified: approvals={}, surface_changes={}, tracked_files_added={}",
        report.approvals_verified, report.surface_changes_verified, report.tracked_files_added
    );
    Ok(())
}

fn parse_args(
    arguments: impl IntoIterator<Item = String>,
) -> Result<(String, String, u64), String> {
    let mut base = None;
    let mut head = None;
    let mut pull_request = None;
    let mut arguments = arguments.into_iter().peekable();
    while let Some(flag) = arguments.next() {
        let target = match flag.as_str() {
            "--base" => &mut base,
            "--head" => &mut head,
            "--pull-request" => &mut pull_request,
            _ => return Err(format!("unknown argument {flag}")),
        };
        if target.is_some() {
            return Err(format!("duplicate argument {flag}"));
        }
        *target = Some(
            arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?,
        );
    }
    let pull_request = pull_request
        .ok_or_else(usage)?
        .parse::<u64>()
        .map_err(|error| format!("invalid --pull-request value: {error}"))?;
    if pull_request == 0 {
        return Err("--pull-request must be non-zero".to_string());
    }
    Ok((
        base.ok_or_else(usage)?,
        head.ok_or_else(usage)?,
        pull_request,
    ))
}

fn usage() -> String {
    "usage: approval-provenance-guard --base <full-commit-sha> --head <full-commit-sha> --pull-request <number>".to_string()
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
    fn arguments_require_unique_base_head_and_pull_request() {
        assert!(
            parse_args(["--base", "a", "--head", "b", "--pull-request", "123"].map(str::to_string))
                .is_ok()
        );
        assert!(
            parse_args(
                [
                    "--base",
                    "a",
                    "--base",
                    "b",
                    "--head",
                    "c",
                    "--pull-request",
                    "123",
                ]
                .map(str::to_string)
            )
            .unwrap_err()
            .contains("duplicate")
        );
        assert!(
            parse_args(["--base", "a"].map(str::to_string))
                .unwrap_err()
                .contains("usage")
        );
        assert!(
            parse_args(["--base", "a", "--head", "b", "--pull-request", "0"].map(str::to_string))
                .unwrap_err()
                .contains("non-zero")
        );
    }
}
