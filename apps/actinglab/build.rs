// SPDX-License-Identifier: AGPL-3.0-only

use std::env::{self, VarError};
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
    let repository_head = git_head();
    let head = match env::var("ACTINGCOMMAND_RUNTIME_HEAD") {
        Ok(value) if valid_head(&value) => {
            if let Some(repository_head) = &repository_head
                && !value.eq_ignore_ascii_case(repository_head)
            {
                panic!(
                    "ACTINGCOMMAND_RUNTIME_HEAD {value} does not match repository HEAD {repository_head}"
                );
            }
            repository_head.unwrap_or_else(|| value.to_ascii_lowercase())
        }
        Ok(_) | Err(VarError::NotUnicode(_)) => {
            panic!("ACTINGCOMMAND_RUNTIME_HEAD must be a 40-character hexadecimal commit")
        }
        Err(VarError::NotPresent) => repository_head.unwrap_or_else(|| {
            panic!(
                "ACTINGCOMMAND_RUNTIME_HEAD must be a 40-character hexadecimal commit when Git metadata is unavailable"
            )
        }),
    };
    println!("cargo:rustc-env=ACTINGCOMMAND_RUNTIME_HEAD={head}");
}

fn git_head() -> Option<String> {
    let value = git_output(&["rev-parse", "HEAD"])?;
    valid_head(&value).then_some(value)
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
