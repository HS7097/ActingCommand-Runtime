// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.356; contracts/standard-pack-goals.md): the Tools' side of a
//! game root's goal declaration `goals.json`. `actinglab standard-pack check` and `package bundle
//! --goals` read it after the composition they already run, with the contract crate's reader
//! (`parse_standard_pack_goals`), and hold an accepted file's references to the game root
//! (`check_standard_pack_goal_references`): the pools and rules of `schedule.json`, which the
//! composition has accepted, each rule with the package it runs; every package of `bundle.json`
//! with the capabilities its control declaration provides and their `irreversible` marks, read by
//! the Runtime's metadata reader (G-R1 fix round 1: the whole root, so that a capability any
//! package marks irreversible makes every package that provides it irreversible); and, for
//! `standard-pack check` only, the tables of the game root an `ordered` entry takes its values from
//! (`package bundle` holds no table, so it judges no table reference). Nothing here reads or writes
//! a value.
//!
//! `standard-pack check` reports each problem as a `standard_pack_root_invalid` warning and never
//! fails the root for it ([`goal_warnings`]); `package bundle` refuses a refused file as it
//! refuses a refused root file ([`refuse_goals`]).

use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{CHECK_LABEL, root_invalid, root_issue_outcome};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    GoalReferences, GoalRule, IrreversiblePackages, LabError, LabResult,
    MAX_STANDARD_PACK_GOALS_BYTES, StandardPackRoot,
    StandardPackRootIssue, StandardPackTable, StandardPackTableCell, StandardPackTableReadFailure,
    check_standard_pack_goal_references, parse_standard_pack_goals, parse_standard_pack_schedule,
    read_standard_pack_table,
};
use actingcommand_execution_kernel::PreparedContainedTask;
use actingcommand_policy::BenefitFollower;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The bytes of the game root's `goals.json` at `path`, `None` when the root holds none, read
/// through the file's bound plus one byte: a file of any size is then the reader's
/// `goal_out_of_limits` (G-R1 fix round 1), never an error of `standard-pack check`. A file that
/// cannot be read is a usage error naming it.
pub(crate) fn read_goals_file(path: &Path) -> LabResult<Option<Vec<u8>>> {
    let unreadable = |error: io::Error| LabError::usage(format!("{}: {error}", path.display()));
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_STANDARD_PACK_GOALS_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    Ok(Some(bytes))
}

/// The problems of the goal declaration `goals` (the bytes of `goals.json`) of the game root
/// `root` of `game`, in file order: the reader's, or, for a file the reader accepts, its
/// references'. `schedule` is `schedule.json`'s bytes and path when the root holds one (the
/// composition has accepted it), and `followers` the chain followers that composition listed
/// (`benefit_followers`, G-R1 fix round 2; none without it); `pack_directory` is where a registry
/// entry's directory lies;
/// `game_root` is the directory whose `tables/` the references are held to, `None` when the caller
/// holds no tables. Empty for a file that keeps every rule. A package or a table that cannot be
/// read is the caller's error, not a problem of the file.
pub(crate) fn goal_issues(
    game: &str,
    root: &StandardPackRoot,
    schedule: Option<(&[u8], &Path)>,
    followers: &[BenefitFollower],
    goals: &[u8],
    pack_directory: &dyn Fn(&str) -> PathBuf,
    game_root: Option<&Path>,
) -> LabResult<Vec<StandardPackRootIssue>> {
    let declared = match parse_standard_pack_goals(goals, game) {
        Ok(declared) => declared,
        Err(issues) => return Ok(issues),
    };
    // G-R1 fix round 1: what every package of the root provides, each capability with its
    // `irreversible` mark, in registry order as the remedy report reads them. The references
    // judge irreversibility per capability across the whole root (contracts/remedies.md §2.3).
    let mut provides = BTreeMap::new();
    for package_id in root.registry.package_ids() {
        let entry = root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let capabilities = package_provides(&pack_directory(&entry.directory))?;
        provides.insert(package_id.to_owned(), capabilities);
    }
    let mut references = GoalReferences::new(provides);
    if let Some((bytes, path)) = schedule {
        let schedule = parse_standard_pack_schedule(bytes)
            .map_err(|issue| root_invalid(game, path, &issue))?;
        for pool in &schedule.pools {
            // A pool a goal may name has a valuation block with its step and its gap weight.
            let valued = matches!(
                pool.body.get("valuation"),
                Some(Value::Object(valuation))
                    if valuation.contains_key("scale") && valuation.contains_key("gap")
            );
            references.pools.insert(pool.id.clone(), valued);
        }
        // G-R1 fix round 2: each rule's followers, as the composition lists them: a follower with
        // the rule it rides with (a follower of a follower rides with the same rule).
        let mut followers_of = BTreeMap::<&str, Vec<String>>::new();
        for follower in followers {
            followers_of
                .entry(follower.rides_with.as_str())
                .or_default()
                .push(follower.task_id.clone());
        }
        for rule in &schedule.rules {
            // G-R1 fix round 1: with the package it runs, which a `tier` entry is held to; fix
            // round 2: and with the rules that follow it, whose packages it is held to as well.
            references.rules.insert(
                rule.id.clone(),
                GoalRule {
                    package_id: rule.package_id.clone(),
                    fixed_value: rule.fixed_value_milli.is_some(),
                    followers: followers_of.remove(rule.id.as_str()).unwrap_or_default(),
                },
            );
        }
    }
    if let Some(game_root) = game_root {
        let mut tables = BTreeMap::new();
        for table_id in declared.tables() {
            match read_standard_pack_table(game_root, game, table_id) {
                Ok(Some(read)) => {
                    tables.insert(table_id.to_owned(), table_keys(&read.table));
                }
                // A refused table is no table the root offers; the table's own check names why.
                Ok(None) | Err(StandardPackTableReadFailure::Invalid { .. }) => {}
                Err(StandardPackTableReadFailure::Unreadable { path, error, .. }) => {
                    return Err(LabError::usage(format!("{}: {error}", path.display())));
                }
            }
        }
        references.tables = Some(tables);
    }
    Ok(check_standard_pack_goal_references(&declared, &references))
}

/// What the package at `path` provides, read by the Runtime's metadata reader
/// (`read_remedy_declarations`): each capability of its control declaration's `provides`, `true`
/// when an entry marks it `irreversible`. A package the reader refuses is `package_invalid`
/// naming its path.
fn package_provides(path: &Path) -> LabResult<BTreeMap<String, bool>> {
    let declarations = PreparedContainedTask::read_remedy_declarations(
        CHECK_LABEL,
        path,
        Instant::now() + PACK_DEADLINE,
    )
    .map_err(|error| LabError::package_invalid(format!("{}: {error}", path.display())))?;
    let mut capabilities = BTreeMap::new();
    for provided in &declarations.provides {
        *capabilities
            .entry(provided.capability().to_owned())
            .or_insert(false) |= provided.irreversible();
    }
    Ok(capabilities)
}

/// Workflow #455 G-R2s (register §1.363; contracts/standard-pack-schedule.md, "The irreversible
/// mark"): the composer's required input for the game root `root`, every package its registry
/// lists read below `pack_directory` by the reader the goal check uses ([`package_provides`]),
/// through the one copy of the rule ([`IrreversiblePackages::new`]). A package the reader refuses
/// could not be read: the composition then marks every task of the game and warns
/// `standard_pack_package_unreadable`, which the tools list.
pub(crate) fn root_irreversibility(
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> IrreversiblePackages {
    IrreversiblePackages::new(
        root.registry
            .package_ids()
            .map(|package_id| {
                let capabilities = root
                    .registry
                    .get(package_id)
                    .and_then(|entry| package_provides(&pack_directory(&entry.directory)).ok());
                (package_id.to_owned(), capabilities)
            })
            .collect(),
    )
}

/// The distinct keys of `table` (its key column, `row_key[0]`), in row order.
fn table_keys(table: &StandardPackTable) -> Vec<String> {
    let Some(column) = table
        .row_key
        .first()
        .and_then(|key| table.columns.iter().position(|column| &column.name == key))
    else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut keys = Vec::new();
    for row in &table.rows {
        if let Some(StandardPackTableCell::Text(key)) = row.get(column)
            && seen.insert(key.as_str())
        {
            keys.push(key.clone());
        }
    }
    keys
}

/// `standard-pack check`: one `standard_pack_root_invalid` warning per problem of the file at
/// `path`, in file order.
pub(crate) fn goal_warnings(
    game: &str,
    path: &Path,
    issues: &[StandardPackRootIssue],
) -> Vec<Outcome> {
    issues
        .iter()
        .map(|issue| root_issue_outcome(game, path, issue))
        .collect()
}

/// `package bundle --goals`: a refused file refuses the bundle as a refused root file does, with
/// its first problem; the message names every one.
pub(crate) fn refuse_goals(
    game: &str,
    path: &Path,
    issues: &[StandardPackRootIssue],
) -> LabResult<()> {
    let Some(first) = issues.first() else {
        return Ok(());
    };
    let mut error = root_invalid(game, path, first);
    error.message = format!(
        "{}; {} problem(s) in all: {}",
        error.message,
        issues.len(),
        issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    );
    Err(error)
}
