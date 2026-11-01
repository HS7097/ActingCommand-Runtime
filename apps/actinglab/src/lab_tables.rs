// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-2 (model M-357a v1.1 §2.4; register §1.65 TB7): the shared tables of a Lab dry
//! run. An offline runtime answers a select step's table from the root it is given
//! (`--standard-pack-root`), read and checked once here with the Runtime's own K1 and K2, and it
//! reads no file during the run. A package that declares no table needs no root.

use super::{CliError, CliOutcome, ErrorKind};
use actingcommand_contract::StandardPackTableReadFailure;
use actingcommand_lab::{PreparedContainedTask, RunTableFailure, RunTables};
use std::path::Path;

/// The flag that names the root of the game directories, `<root>/<game>/tables/`.
pub(super) const STANDARD_PACK_ROOT_FLAG: &str = "--standard-pack-root";

/// The tables `task` declares, from the game root below `root`, fixed at `now_unix_ms`.
pub(super) fn run_tables(
    root: Option<&Path>,
    task: &PreparedContainedTask,
    now_unix_ms: u64,
) -> CliOutcome<RunTables> {
    let mut tables = RunTables::default();
    if !task.declares_tables() {
        return Ok(tables);
    }
    let Some(root) = root else {
        return Err(refusal(
            "standard_pack_table_missing",
            format!(
                "a select step of this package reads a shared table: {STANDARD_PACK_ROOT_FLAG} <root> names where"
            ),
        ));
    };
    tables
        .bind_package(&root.join(task.game()), task, now_unix_ms)
        .map_err(|failure| match failure {
            RunTableFailure::Missing { path } => refusal(
                "standard_pack_table_missing",
                format!("{path} is not in the game root of {}", root.display()),
            ),
            RunTableFailure::Read(StandardPackTableReadFailure::Invalid { path, issue }) => {
                refusal(
                    "standard_pack_table_invalid",
                    format!("{}: {issue}", path.display()),
                )
            }
            RunTableFailure::Read(StandardPackTableReadFailure::Unreadable {
                path, error, ..
            }) => refusal(
                "standard_pack_table_unreadable",
                format!("{}: {error}", path.display()),
            ),
            RunTableFailure::Mismatch(mismatch) => {
                refusal("standard_pack_table_mismatch", mismatch.to_string())
            }
        })?;
    Ok(tables)
}

fn refusal(code: &str, message: String) -> CliError {
    CliError::new(ErrorKind::UsageValidation, code, message, &[])
}
