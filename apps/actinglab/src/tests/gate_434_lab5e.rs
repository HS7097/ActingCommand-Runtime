// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5E (register §1.287, "Slice LAB-5E"; contracts/lab-two-entries.md §9.1 and
// §9.2; coordinator work order rd-1108/arch-013/brief-lab5e.md): every offered row of the
// operation table says in one sentence what it does.
// - `lab5e_offered_rows_have_help`: every row actinglab offers (`words::offered`; the rescue
//   module's on Windows, where it runs) and every planned row of slice LAB-5, which LAB-5's MCP
//   server offers itself (`record draft`, `mcp config`, `mcp entries`), has a `help` that is not
//   blank. The form of every `help` (ASCII, 1 to 160 bytes on one line, no MCP tool name, no two
//   rows alike) is `lab_ops_table_complete`'s, in `crates/lab-ops`.
// Reads only the table this build embeds: no state root, no Runtime, no device, no clock.

use crate::cli_module::{self, words};
use actingcommand_lab_ops::table::{OperationStatus, OperationTable};

/// The slice of the MCP server that offers its planned rows itself (§9.1).
const LAB5_SLICE: &str = "LAB-5";

#[test]
fn lab5e_offered_rows_have_help() {
    let table: &OperationTable = cli_module::table().expect("the operation table");
    let offered = table
        .operations
        .iter()
        .filter(|row| {
            words::offered(row)
                || (row.status == OperationStatus::Planned && row.slice == LAB5_SLICE)
        })
        .collect::<Vec<_>>();
    assert!(
        offered.iter().any(|row| row.slice == LAB5_SLICE),
        "LAB-5's rows are among the offered rows"
    );
    let missing = offered
        .iter()
        .filter(|row| {
            row.help
                .as_deref()
                .is_none_or(|help| help.trim().is_empty())
        })
        .map(|row| row.name.as_str())
        .collect::<Vec<_>>();
    assert!(missing.is_empty(), "offered rows without help: {missing:?}");
}
