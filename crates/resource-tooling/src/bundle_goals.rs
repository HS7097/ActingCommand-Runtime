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

use crate::StandardPackRootFiles;
use crate::standard_pack_check::RootFileInputs;
use crate::standard_pack_goals::{goal_issues, goal_warnings, read_goals_file};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, GoalType, LabError, LabResult, STANDARD_PACK_BUNDLE_FILE,
    STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackGoals, parse_standard_pack_goals,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// The directory of a written bundle's packs, `packs/<package_id>/`.
const PACKS_DIRECTORY: &str = "packs";

/// What [`check_bundle_goals`] reads.
#[derive(Debug, Clone)]
pub struct BundleGoalsRequest {
    /// A directory `package bundle` wrote (its `--out`): `bundle.json`, the root files it was
    /// given and `packs/<package_id>/`.
    pub bundle: PathBuf,
    /// The `goals.json` to hold to the game root the bundle makes.
    pub goals: PathBuf,
}

/// What [`check_bundle_goals`] found.
#[derive(Debug, Clone)]
pub struct BundleGoals {
    /// The bundle's game (`bundle.json` `game`), whose game root the file was held to.
    pub game: String,
    /// The file's bytes as read: the whole file when it is accepted.
    pub bytes: Vec<u8>,
    pub verdict: BundleGoalsVerdict,
}

/// What [`check_bundle_goals`] judged of the file.
#[derive(Debug, Clone)]
pub enum BundleGoalsVerdict {
    /// The file keeps every rule: how many entries of each type of version 2 it holds, by the
    /// type's word, every type named.
    Accepted(BTreeMap<&'static str, usize>),
    /// The file is refused: every problem, `standard_pack_root_invalid` with `game`, `path` (the
    /// file), `standard_pack_problem` and `json_pointer`, in file order, as `standard-pack check`
    /// gives each.
    Refused(Vec<Outcome>),
}

/// Holds the `goals.json` at `request.goals` to the game root the bundle written into
/// `request.bundle` makes: its `bundle.json` and the root files beside it, read as `package bundle`
/// read them, and its packs below `packs/`. A bundle or a root file that cannot be read, or a
/// package of the bundle the metadata reader refuses, is the error, not a problem of the file.
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
    let goals = read_goals_file(&request.goals)?.ok_or_else(|| {
        LabError::usage(format!("{}: no such root file", request.goals.display()))
    })?;
    let packs = request.bundle.join(PACKS_DIRECTORY);
    let issues = goal_issues(
        &game,
        &root,
        inputs
            .schedule()
            .map(|schedule| (schedule.bytes.as_slice(), schedule.path.as_path())),
        &goals,
        &|directory: &str| packs.join(directory),
    )?;
    // An accepted file is read again by the same reader, for its entries.
    let verdict = if issues.is_empty() {
        match parse_standard_pack_goals(&goals, &game) {
            Ok(declared) => BundleGoalsVerdict::Accepted(entry_counts(&declared)),
            Err(refused) => {
                BundleGoalsVerdict::Refused(goal_warnings(&game, &request.goals, &refused))
            }
        }
    } else {
        BundleGoalsVerdict::Refused(goal_warnings(&game, &request.goals, &issues))
    };
    Ok(BundleGoals {
        game,
        bytes: goals,
        verdict,
    })
}

/// How many entries of each type of version 2 `declared` holds, by the type's word.
fn entry_counts(declared: &StandardPackGoals) -> BTreeMap<&'static str, usize> {
    GoalType::ALL
        .into_iter()
        .map(|goal_type| {
            let count = declared
                .entries
                .iter()
                .filter(|entry| entry.goal_type() == goal_type)
                .count();
            (goal_type.as_str(), count)
        })
        .collect()
}
