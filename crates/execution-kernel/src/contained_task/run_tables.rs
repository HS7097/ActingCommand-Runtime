// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-2 (model M-357a v1.1 §2.4, §3.5 K1, K2, K4; register §1.65 TB7): the shared
//! tables of one run, read once in its preparation and answered from memory afterwards.
//!
//! Admission never comes here: a package's admission depends only on the package and the kernel,
//! so suspension and its lift do not meet a table (§2.7). The host's run preparation, and the
//! Lab's dry runs given a root, call [`RunTables::bind_package`] for each package of the run; a
//! run reads each path once and keeps that table to its end, so a table replaced in place is the
//! next run's. Evaluation reads no file: [`ContainedTaskRuntime::standard_pack_table`] answers
//! from what was bound here.

use super::{ContainedTaskRuntime, PreparedContainedTask};
use actingcommand_contract::{StandardPackTableReadFailure, read_standard_pack_table};
use actingcommand_selection_policy::{LoadedTable, SelectionTables, TableBindRefusal};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// The tables a run bound, by the path its documents declare (`tables/<table_id>.json`).
#[derive(Debug, Clone, Default)]
pub struct RunTables {
    tables: BTreeMap<String, Arc<LoadedTable>>,
}

/// A table a package's binding read for the run: what the run's record names (register §1.244).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTableRead {
    /// The path the documents declare, `tables/<table_id>.json`.
    pub path: String,
    /// `sha256:<lowercase hex>` of the file as it was read.
    pub sha256: String,
}

/// Why a package's tables could not be bound: the run is refused (§2.4).
#[derive(Debug)]
pub enum RunTableFailure {
    /// K1: the game root holds no such table (`standard_pack_table_missing`).
    Missing { path: String },
    /// K1: the file is refused or cannot be read.
    Read(StandardPackTableReadFailure),
    /// K2: the table differs from the document's declaration (`standard_pack_table_mismatch`).
    Mismatch(TableBindRefusal),
}

impl RunTables {
    /// The table the run bound at `path`.
    pub fn table(&self, path: &str) -> Option<Arc<LoadedTable>> {
        self.tables.get(path).cloned()
    }

    /// K1 and K2 for every select step of `package`, whose game root is `game_root`: a path this
    /// run has not read is read once and fixed at `now_unix_ms`, and every document that declares
    /// a table is bound against what the run holds. A package that declares no table reads
    /// nothing. The answer is the tables this call read, each once: a path the run holds already
    /// is not among them, and a failure answers none.
    pub fn bind_package(
        &mut self,
        game_root: &Path,
        package: &PreparedContainedTask,
        now_unix_ms: u64,
    ) -> Result<Vec<RunTableRead>, RunTableFailure> {
        let mut reads = Vec::new();
        for policy in package.selection_policies() {
            let mut loaded = Vec::with_capacity(policy.tables.len());
            for declaration in &policy.tables {
                let table = match self.tables.get(&declaration.path) {
                    Some(table) => Arc::clone(table),
                    None => {
                        let read = read_standard_pack_table(
                            game_root,
                            package.game(),
                            &declaration.table_id,
                        )
                        .map_err(RunTableFailure::Read)?;
                        let Some(read) = read else {
                            return Err(RunTableFailure::Missing {
                                path: declaration.path.clone(),
                            });
                        };
                        let table = Arc::new(LoadedTable::load(read.table, now_unix_ms));
                        self.tables
                            .insert(declaration.path.clone(), Arc::clone(&table));
                        reads.push(RunTableRead {
                            path: declaration.path.clone(),
                            sha256: read.sha256,
                        });
                        table
                    }
                };
                loaded.push(table);
            }
            SelectionTables::bind(policy, &loaded).map_err(RunTableFailure::Mismatch)?;
        }
        Ok(reads)
    }
}

/// The tables of one evaluation: each table `policy` declares as the runtime answers for it. One
/// it cannot answer for stays unbound, and the evaluator refuses it (`dangling_reference`).
pub(super) fn evaluation_tables<R: ContainedTaskRuntime>(
    runtime: &mut R,
    policy: &actingcommand_selection_policy::SelectionPolicy,
) -> Result<SelectionTables, TableBindRefusal> {
    let loaded = policy
        .tables
        .iter()
        .filter_map(|declaration| runtime.standard_pack_table(&declaration.path))
        .collect::<Vec<_>>();
    SelectionTables::bind(policy, &loaded)
}
