// SPDX-License-Identifier: AGPL-3.0-only

//! `outcome-guard merge [<workspace root>]` writes `contracts/outcome-codes.json` from the
//! catalog header and the owner fragments. CI runs it on every pushed head and uploads the
//! file as the artifact `outcome-codes-<sha>`; implementers commit that artifact unchanged.

use actingcommand_outcome_guard::catalog::MERGED_FILE;
use actingcommand_outcome_guard::merge;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let root = match arguments.as_slice() {
        [command] if command == "merge" => PathBuf::from("."),
        [command, root] if command == "merge" => PathBuf::from(root),
        _ => {
            eprintln!("ERROR outcome-guard: usage: outcome-guard merge [<workspace root>]");
            return ExitCode::from(1);
        }
    };
    let target = root.join(MERGED_FILE);
    let errors = match merge(&root) {
        Ok(text) => {
            let current = std::fs::read_to_string(&target).ok();
            let current = current.map(|current| current.replace("\r\n", "\n"));
            if current.as_deref() == Some(text.as_str()) {
                println!("outcome-guard: {MERGED_FILE} is current");
                return ExitCode::SUCCESS;
            }
            match std::fs::write(&target, text) {
                Ok(()) => {
                    println!("outcome-guard: wrote {MERGED_FILE}");
                    return ExitCode::SUCCESS;
                }
                Err(error) => vec![format!("cannot write {MERGED_FILE}: {error}")],
            }
        }
        Err(errors) => errors,
    };
    for error in errors {
        eprintln!("ERROR outcome-guard: {error}");
    }
    ExitCode::from(1)
}
