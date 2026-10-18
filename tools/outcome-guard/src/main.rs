// SPDX-License-Identifier: AGPL-3.0-only

//! `outcome-guard merge [<workspace root>]` writes `contracts/outcome-codes.json` from the
//! catalog header and the owner fragments. CI runs it on every pushed head and uploads the
//! file as the artifact `outcome-codes-<sha>`; implementers commit that artifact unchanged.
//!
//! `outcome-guard release <version> [<workspace root>]` is G10 for one release (`X.Y.Z` or
//! `X.Y.Z-rc.N`, without the tag's `v`): the exact-SHA build of a release runs it on the commit
//! it releases, before it builds, and stops the release on any violation (Workflow #378 A8b).

use actingcommand_outcome_guard::catalog::MERGED_FILE;
use actingcommand_outcome_guard::{merge, release_gate};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: outcome-guard merge [<workspace root>] | outcome-guard release <version> [<workspace root>]";

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let errors = match arguments.as_slice() {
        [command] if command == "merge" => run_merge(Path::new(".")),
        [command, root] if command == "merge" => run_merge(Path::new(root)),
        [command, release] if command == "release" => run_release(Path::new("."), release),
        [command, release, root] if command == "release" => run_release(Path::new(root), release),
        _ => vec![USAGE.to_owned()],
    };
    if errors.is_empty() {
        return ExitCode::SUCCESS;
    }
    for error in errors {
        eprintln!("ERROR outcome-guard: {error}");
    }
    ExitCode::from(1)
}

fn run_merge(root: &Path) -> Vec<String> {
    let target: PathBuf = root.join(MERGED_FILE);
    match merge(root) {
        Ok(text) => {
            let current = std::fs::read_to_string(&target).ok();
            let current = current.map(|current| current.replace("\r\n", "\n"));
            if current.as_deref() == Some(text.as_str()) {
                println!("outcome-guard: {MERGED_FILE} is current");
                return Vec::new();
            }
            match std::fs::write(&target, text) {
                Ok(()) => {
                    println!("outcome-guard: wrote {MERGED_FILE}");
                    Vec::new()
                }
                Err(error) => vec![format!("cannot write {MERGED_FILE}: {error}")],
            }
        }
        Err(errors) => errors,
    }
}

fn run_release(root: &Path, release: &str) -> Vec<String> {
    let violations = release_gate(root, release);
    if violations.is_empty() {
        println!("outcome-guard: G10 holds for release {release}");
    }
    violations
}
