// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 AO-1: the commit actingd is built from, for the header of the ledger's dump file
// (`contracts/ledger-observer.md` §9.3): `ACTINGCOMMAND_RUNTIME_HEAD` when it is a commit,
// otherwise the repository's HEAD. A build without either leaves the header member out.

use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=ACTINGCOMMAND_RUNTIME_HEAD");
    if let Some(path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(reference) = git_output(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git_output(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }
    let head = env::var("ACTINGCOMMAND_RUNTIME_HEAD")
        .ok()
        .filter(|value| valid_head(value))
        .or_else(|| git_output(&["rev-parse", "HEAD"]).filter(|value| valid_head(value)))
        .map(|value| value.to_ascii_lowercase());
    if let Some(head) = head {
        println!("cargo:rustc-env=ACTINGCOMMAND_RUNTIME_HEAD={head}");
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_string())
}

fn valid_head(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
