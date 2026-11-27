// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 GOALS-PACK-1 (register §1.498; contracts/standard-pack-goals.md, "Who reads it";
//! contracts/resource-verify.md, the `bundle` step): a resource repository's `goals.json` held to
//! the game root that `package bundle` wrote into a directory, for the `bundle` step `resource
//! verify` and `standard-pack build` run on their one function. The file is read as `standard-pack
//! check` reads it ([`read_goals_file`], through its bound plus one byte) and judged by the one goal
//! check ([`goal_issues`]: the contract crate's reader, then an accepted file's references held to
//! the bundle's packages, the pools and rules of its composed `schedule.json` and the variables its
//! packs declare, as `package bundle --goals` holds them); each problem is given as `standard-pack
//! check` gives it ([`goal_warnings`]), every one, in file order. An accepted file's entries are
//! counted by type, read by the same reader. Nothing is written.
//!
//! Workflow #455 RR1 (register §1.535; contracts/standard-pack-goal-tables.md): the repository's
//! `goal-tables.json` the same way and by the same check, read through its own bound
//! ([`read_goal_tables_file`]); an accepted file's tables and items are counted.

use crate::StandardPackRootFiles;
use crate::standard_pack_check::RootFileInputs;
use crate::standard_pack_goals::{
    GoalFiles, goal_issues, goal_warnings, read_goal_tables_file, read_goals_file,
};
use crate::standard_pack_schedule::switched_catalog;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, GoalType, LabError, LabResult, STANDARD_PACK_BUNDLE_FILE,
    STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackGoalTables, StandardPackGoals, StandardPackRootIssue,
    parse_standard_pack_goal_tables, parse_standard_pack_goals,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The directory of a written bundle's packs, `packs/<package_id>/`.
const PACKS_DIRECTORY: &str = "packs";

/// What [`check_bundle_goals`] reads.
#[derive(Debug, Clone)]
pub struct BundleGoalsRequest {
    /// A directory `package bundle` wrote (its `--out`): `bundle.json`, the root files it was
    /// given and `packs/<package_id>/`.
    pub bundle: PathBuf,
    /// The `goals.json` to hold to the game root the bundle makes, when the repository holds one.
    pub goals: Option<PathBuf>,
    /// Workflow #455 RR1: the `goal-tables.json` to hold to it, when the repository holds one.
    pub goal_tables: Option<PathBuf>,
}

/// What [`check_bundle_goals`] found.
#[derive(Debug, Clone)]
pub struct BundleGoals {
    /// The bundle's game (`bundle.json` `game`), whose game root the files were held to.
    pub game: String,
    /// `goals.json`, when the request named one.
    pub goals: Option<BundleGoalFile>,
    /// Workflow #455 RR1: `goal-tables.json`, when the request named one.
    pub goal_tables: Option<BundleGoalFile>,
}

/// One file [`check_bundle_goals`] read: its bytes as read (the whole file when it is accepted)
/// and what the check judged of it.
#[derive(Debug, Clone)]
pub struct BundleGoalFile {
    pub bytes: Vec<u8>,
    pub verdict: BundleGoalsVerdict,
}

/// What [`check_bundle_goals`] judged of a file.
#[derive(Debug, Clone)]
pub enum BundleGoalsVerdict {
    /// The file keeps every rule. Of `goals.json`: how many entries of each type of version 2 it
    /// holds, by the type's word, every such type named, and (Workflow #455 RR1) `records` when it
    /// holds one, so that a file of version 2's types is counted as before. Of `goal-tables.json`:
    /// how many `tables` and `items` it declares.
    Accepted(BTreeMap<&'static str, usize>),
    /// The file is refused: every problem, `standard_pack_root_invalid` with `game`, `path` (the
    /// file), `standard_pack_problem` and `json_pointer`, in file order, as `standard-pack check`
    /// gives each.
    Refused(Vec<Outcome>),
}

/// Holds the `goals.json` and (Workflow #455 RR1) the `goal-tables.json` the request names to the
/// game root the bundle written into `request.bundle` makes: its `bundle.json` and the root files
/// beside it, read as `package bundle` read them, and its packs below `packs/`. A bundle or a root
/// file that cannot be read, a file the request names that does not exist, or a package of the
/// bundle the metadata reader refuses, is the error, not a problem of the file.
pub fn check_bundle_goals(request: &BundleGoalsRequest) -> LabResult<BundleGoals> {
    let bundle_path = request.bundle.join(STANDARD_PACK_BUNDLE_FILE);
    let unreadable = |error: String| LabError::usage(format!("{}: {error}", bundle_path.display()));
    let bundle = fs::read(&bundle_path).map_err(|error| unreadable(error.to_string()))?;
    let game = serde_json::from_slice::<BundleIndex>(&bundle)
        .map_err(|error| unreadable(error.to_string()))?
        .game;
    let present = |name: &str| {
        let path = request.bundle.join(name);
        fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file())
            .then_some(path)
    };
    let inputs = RootFileInputs::read(&StandardPackRootFiles {
        pages: present(STANDARD_PACK_PAGES_FILE),
        interrupts: present(STANDARD_PACK_INTERRUPTS_FILE),
        navigation: present(STANDARD_PACK_NAVIGATION_FILE),
        schedule: present(STANDARD_PACK_SCHEDULE_FILE),
        display_layout: present(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
        ..StandardPackRootFiles::default()
    })?;
    let root = inputs.root(&game, &bundle_path, &bundle)?;
    let goals = request
        .goals
        .as_deref()
        .map(|path| read_named(path, read_goals_file))
        .transpose()?;
    let tables = request
        .goal_tables
        .as_deref()
        .map(|path| read_named(path, read_goal_tables_file))
        .transpose()?;
    let packs = request.bundle.join(PACKS_DIRECTORY);
    // Workflow #477 CV-2 (register §1.508): a `schedule.json` that names `catalog_business` is held
    // with the `pack-catalog.json` of the root `goals.json` is read from (the checkout's, beside
    // it): `package bundle` lays no such file, which `standard-pack build` carries into the zip
    // itself. Workflow #455 RR1: beside either file the request names.
    let beside = request.goals.as_deref().or(request.goal_tables.as_deref());
    let catalog = match (inputs.schedule(), beside) {
        (Some(schedule), Some(beside)) => switched_catalog(&schedule.bytes, beside)?,
        _ => None,
    };
    let issues = goal_issues(
        &game,
        &root,
        inputs
            .schedule()
            .map(|schedule| (schedule.bytes.as_slice(), schedule.path.as_path())),
        catalog.as_deref(),
        GoalFiles {
            goals: goals.as_deref(),
            tables: tables.as_deref(),
        },
        &|directory: &str| packs.join(directory),
    )?;
    let goals = match (request.goals.as_deref(), goals) {
        (Some(path), Some(bytes)) => {
            // An accepted file is read again by the same reader, for its entries.
            let judgement = judged(&game, path, &issues.goals, || {
                parse_standard_pack_goals(&bytes, &game).map(|declared| entry_counts(&declared))
            });
            Some(BundleGoalFile {
                bytes,
                verdict: judgement,
            })
        }
        _ => None,
    };
    let goal_tables = match (request.goal_tables.as_deref(), tables) {
        (Some(path), Some(bytes)) => {
            let judgement = judged(&game, path, &issues.tables, || {
                parse_standard_pack_goal_tables(&bytes, &game).map(|read| table_counts(&read))
            });
            Some(BundleGoalFile {
                bytes,
                verdict: judgement,
            })
        }
        _ => None,
    };
    Ok(BundleGoals {
        game,
        goals,
        goal_tables,
    })
}

/// The bytes of the file at `path`, read by `read`; a file that does not exist is the error.
fn read_named(path: &Path, read: fn(&Path) -> LabResult<Option<Vec<u8>>>) -> LabResult<Vec<u8>> {
    read(path)?.ok_or_else(|| LabError::usage(format!("{}: no such root file", path.display())))
}

/// What an accepted file's counts are read as: the counts, or the reader's problems.
type Counted = Result<BTreeMap<&'static str, usize>, Vec<StandardPackRootIssue>>;

/// The verdict of the file at `path` of `game` whose problems are `issues`: refused with them, or
/// accepted with the counts `counted` reads (refused with the reader's problems should it refuse).
fn judged(
    game: &str,
    path: &Path,
    issues: &[StandardPackRootIssue],
    counted: impl FnOnce() -> Counted,
) -> BundleGoalsVerdict {
    if !issues.is_empty() {
        return BundleGoalsVerdict::Refused(goal_warnings(game, path, issues));
    }
    match counted() {
        Ok(counts) => BundleGoalsVerdict::Accepted(counts),
        Err(refused) => BundleGoalsVerdict::Refused(goal_warnings(game, path, &refused)),
    }
}

/// How many entries of each type of version 2 `declared` holds, by the type's word, every such
/// type named; Workflow #455 RR1: and `records` when it holds one.
fn entry_counts(declared: &StandardPackGoals) -> BTreeMap<&'static str, usize> {
    GoalType::ALL
        .into_iter()
        .filter_map(|goal_type| {
            let count = declared
                .entries
                .iter()
                .filter(|entry| entry.goal_type() == goal_type)
                .count();
            (goal_type != GoalType::Records || count > 0).then_some((goal_type.as_str(), count))
        })
        .collect()
}

/// Workflow #455 RR1: how many tables and items `tables` declares.
fn table_counts(tables: &StandardPackGoalTables) -> BTreeMap<&'static str, usize> {
    BTreeMap::from([
        ("tables", tables.tables.len()),
        ("items", tables.items.len()),
    ])
}
