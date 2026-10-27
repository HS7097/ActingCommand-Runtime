// SPDX-License-Identifier: AGPL-3.0-only

//! `outcome-guard merge [<workspace root>]` prints the merged catalog, the header and every owner
//! fragment, on stdout. Nothing commits it (Workflow #420 S2): the contract crate's `build.rs`
//! embeds the same text, generated from the same fragments at build time.
//!
//! `outcome-guard release <version> [<workspace root>]` is G10 for one release (`X.Y.Z` or
//! `X.Y.Z-rc.N`, without the tag's `v`): the exact-SHA build of a release runs it on the commit
//! it releases, before it builds, and stops the release on any violation (Workflow #378 A8b).

use actingcommand_outcome_guard::{merge, release_gate};
use std::io::Write;
use std::path::Path;
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
    match merge(root) {
        Ok(text) => {
            let mut stdout = std::io::stdout().lock();
            match stdout
                .write_all(text.as_bytes())
                .and_then(|()| stdout.flush())
            {
                Ok(()) => Vec::new(),
                Err(error) => vec![format!(
                    "cannot write the merged catalog to stdout: {error}"
                )],
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
