// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.356; contracts/standard-pack-goals.md): the Tools' side of a
//! game root's goal declaration `goals.json`. `actinglab standard-pack check` and `package bundle
//! --goals` read it after the composition they already run, with the contract crate's reader
//! (`parse_standard_pack_goals`), and hold an accepted file's references to the game root
//! (`check_standard_pack_goal_references`): the pools and rules of `schedule.json`, which the
//! composition has accepted, each rule with the package it runs; and every package of
//! `bundle.json` with the capabilities its control declaration provides and their `irreversible`
//! marks, read by the Runtime's metadata reader (G-R1 fix round 1: the whole root, so that a
//! capability any package marks irreversible makes every package that provides it irreversible),
//! and (Workflow #463 V1) the variables it declares, which a `variable` entry places. Nothing here
//! reads or writes a value.
//!
//! `standard-pack check` reports each problem as a `standard_pack_root_invalid` warning and never
//! fails the root for it ([`goal_warnings`]); `package bundle` refuses a refused file as it
//! refuses a refused root file ([`refuse_goals`]).
//!
//! Workflow #463 V1 (register §1.378; contracts/package-variables.md): `standard-pack check` also
//! runs the root check of the variables the root's packages declare ([`variable_report`]): each
//! problem is a `standard_pack_root_invalid` warning naming the package's `control.json`, the root
//! is checked as before, and the answer lists each variable that applies by its default's origin.
//! Its fix round 1: the packages are judged by the rule of irreversibility over the whole root, the
//! one the goal check uses, and a variable of a package that counts as irreversible is listed as
//! not applying.

use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{CHECK_LABEL, root_invalid, root_issue_outcome};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    DeclaredCapabilities, GoalReferences, GoalRule, IrreversiblePackages, LabError, LabResult,
    MAX_STANDARD_PACK_GOALS_BYTES, PACKAGE_CONTROL_FILE, RootPackageVariables, StandardPackRoot,
    StandardPackRootIssue, VariableOrigins, check_root_variables,
    check_standard_pack_goal_references, goal_variables, parse_standard_pack_goals,
    parse_standard_pack_schedule,
};
use actingcommand_execution_kernel::{ContainedTaskError, PreparedContainedTask};
use actingcommand_policy::standard_pack_rule_followers;
use serde_json::Value;
use std::collections::BTreeMap;
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

/// What every package of a game root declares, as the Runtime's metadata reader reads it, in
/// registry order.
struct PackageDeclarations {
    /// Each package with what the rule of irreversibility judges of it (G-R1 fix round 1;
    /// Workflow #455 G-R2s fix round 3: and every capability its remedies list).
    capabilities: BTreeMap<String, DeclaredCapabilities>,
    /// Workflow #463 V1: each package with the variables it declares and the fact keys its
    /// designated effects read their caps from.
    variables: Vec<RootPackageVariables>,
}

/// Reads every package of `root` (`pack_directory` is where a registry entry's directory lies)
/// with the Runtime's metadata reader; a package it refuses is `package_invalid` naming its
/// directory.
fn package_declarations(
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<PackageDeclarations> {
    let mut declarations = PackageDeclarations {
        capabilities: BTreeMap::new(),
        variables: Vec::new(),
    };
    for package_id in root.registry.package_ids() {
        let entry = root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let path = pack_directory(&entry.directory);
        let read = PreparedContainedTask::read_remedy_declarations(
            CHECK_LABEL,
            &path,
            Instant::now() + PACK_DEADLINE,
        )
        .map_err(|error| LabError::package_invalid(format!("{}: {error}", path.display())))?;
        declarations
            .capabilities
            .insert(package_id.to_owned(), read.declared_capabilities());
        declarations.variables.push(RootPackageVariables {
            package_id: package_id.to_owned(),
            variables: read.variables,
            cap_fact_keys: read.cap_fact_keys,
        });
    }
    Ok(declarations)
}

/// The problems of the goal declaration `goals` (the bytes of `goals.json`) of the game root
/// `root` of `game`, in file order: the reader's, or, for a file the reader accepts, its
/// references'. `schedule` is `schedule.json`'s bytes and path when the root holds one (the
/// composition has accepted it); each of its rules is held with the rules that follow it
/// (`standard_pack_rule_followers`, G-R1 fix round 2, Workflow #455 G-R2s fix round 1);
/// `pack_directory` is where a registry entry's directory lies. Workflow #463 V1: no entry takes
/// a table any more, so no table is read. Empty for a file that keeps every rule. A package that
/// cannot be read is the caller's error, not a problem of the file.
pub(crate) fn goal_issues(
    game: &str,
    root: &StandardPackRoot,
    schedule: Option<(&[u8], &Path)>,
    goals: &[u8],
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<Vec<StandardPackRootIssue>> {
    let declared = match parse_standard_pack_goals(goals, game) {
        Ok(declared) => declared,
        Err(issues) => return Ok(issues),
    };
    // G-R1 fix round 1: what every package of the root provides, each capability with its
    // `irreversible` mark, in registry order as the remedy report reads them. The references
    // judge irreversibility per capability across the whole root (contracts/remedies.md §2.3).
    // Workflow #455 G-R2s fix round 3: and every capability its remedies list.
    let declarations = package_declarations(root, pack_directory)?;
    let mut references = GoalReferences::new(declarations.capabilities);
    // Workflow #463 V1: every variable a package declares, which a placement names.
    references.variables = goal_variables(&declarations.variables);
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
        // G-R1 fix round 2: each rule's followers; Workflow #455 G-R2s fix round 1: every rule
        // that follows it by a chain link, the lists the composer's irreversible mark reads.
        let mut followers_of = standard_pack_rule_followers(&schedule);
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
    Ok(check_standard_pack_goal_references(&declared, &references))
}

/// [`root_irreversibility`]'s reading of the package at `path`: what the Runtime's metadata reader
/// (`read_remedy_declarations`) reads of it for the rule of irreversibility (each capability of
/// its control declaration's `provides`, `true` when an entry marks it `irreversible`, and,
/// Workflow #455 G-R2s fix round 3, every capability its remedies list,
/// `RemedyDeclarations::declared_capabilities`), or the reader's refusal.
fn read_provides(path: &Path) -> Result<DeclaredCapabilities, ContainedTaskError> {
    PreparedContainedTask::read_remedy_declarations(
        CHECK_LABEL,
        path,
        Instant::now() + PACK_DEADLINE,
    )
    .map(|declarations| declarations.declared_capabilities())
}

/// Workflow #455 G-R2s: one game root's packages as the composer takes them
/// ([`root_irreversibility`]), with the reader's refusal of each package it could not read.
pub(crate) struct RootIrreversibility {
    /// The composer's required input.
    pub(crate) packages: IrreversiblePackages,
    /// The reader's refusal of each package it could not read, by package id.
    refusals: BTreeMap<String, Outcome>,
}

impl RootIrreversibility {
    /// G-R2s fix round 1 (the independent review, point 4): `warnings` with the reader's refusal
    /// linked as the cause (`caused_by`) of each `standard_pack_package_unreadable` that names a
    /// package of `game` the reader refused, so the warning says why the package could not be
    /// read; every other warning as it is.
    pub(crate) fn with_causes(&self, game: &str, warnings: Vec<Outcome>) -> Vec<Outcome> {
        warnings
            .into_iter()
            .map(|warning| {
                let unreadable = warning.is(SharedCode::StandardPackPackageUnreadable)
                    && warning.value("game").and_then(Value::as_str) == Some(game);
                let refusal = if unreadable {
                    warning
                        .value("package_id")
                        .and_then(Value::as_str)
                        .and_then(|package_id| self.refusals.get(package_id))
                } else {
                    None
                };
                match refusal {
                    Some(refusal) => warning.caused_by(refusal.clone()),
                    None => warning,
                }
            })
            .collect()
    }
}

/// Workflow #455 G-R2s (register §1.369; contracts/standard-pack-schedule.md, "The irreversible
/// mark"): the composer's required input for the game root `root`, every package its registry
/// lists read below `pack_directory` by the reader the goal check uses ([`read_provides`]),
/// through the one copy of the rule ([`IrreversiblePackages::new`]), with the reader's refusal of
/// each package it refused. A package the reader refuses could not be read: the composition then
/// marks every task of the game and warns `standard_pack_package_unreadable`, which the tools
/// list with that refusal as its cause ([`RootIrreversibility::with_causes`]).
pub(crate) fn root_irreversibility(
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> RootIrreversibility {
    let mut declared = BTreeMap::new();
    let mut refusals = BTreeMap::new();
    for package_id in root.registry.package_ids() {
        let capabilities = match root.registry.get(package_id) {
            Some(entry) => match read_provides(&pack_directory(&entry.directory)) {
                Ok(capabilities) => Some(capabilities),
                Err(error) => {
                    refusals.insert(package_id.to_owned(), error.outcome());
                    None
                }
            },
            None => None,
        };
        declared.insert(package_id.to_owned(), capabilities);
    }
    RootIrreversibility {
        packages: IrreversiblePackages::new(declared),
        refusals,
    }
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

/// Workflow #463 V1 (contracts/package-variables.md, "The root check"): what `standard-pack check`
/// says of the variables the packages of the game root `root` of `game` declare: the warnings,
/// one `standard_pack_root_invalid` per problem of the contract crate's root check
/// (`check_root_variables`), each naming the declaring package's `control.json`, in the check's
/// order; and the provenance listing of the variables that apply, `None` when no package declares
/// one. `goals` is the root's `goals.json` when it holds one: the names an accepted file uses for
/// anything but a placement are no variable's (a refused file applies no entry, so it takes none).
/// Fix round 1: the packages are judged by the rule of irreversibility over the whole root, from
/// what the reader read of every package (`IrreversiblePackages::new`, the one copy the goal
/// check holds a placement to), so a variable of a package that counts as irreversible is
/// `variable_irreversible_by_root` and the listing marks it as not applying.
pub(crate) fn variable_report(
    game: &str,
    root: &StandardPackRoot,
    goals: Option<&[u8]>,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<(Vec<Outcome>, Option<VariableOrigins>)> {
    let declarations = package_declarations(root, pack_directory)?;
    let accepted = goals.and_then(|goals| parse_standard_pack_goals(goals, game).ok());
    let goal_names = accepted
        .as_ref()
        .map(|goals| goals.names_besides_placements())
        .unwrap_or_default();
    // Every package of the root was read (one the reader refuses is the check's error).
    let irreversible = IrreversiblePackages::new(
        declarations
            .capabilities
            .into_iter()
            .map(|(package_id, declared)| (package_id, Some(declared)))
            .collect(),
    );
    let checked = check_root_variables(&declarations.variables, &goal_names, &irreversible);
    let mut warnings = Vec::with_capacity(checked.issues.len());
    for found in &checked.issues {
        let entry = root
            .registry
            .get(found.package_id.as_str())
            .ok_or_else(|| {
                LabError::package_invalid(format!(
                    "{} is no package of the bundle",
                    found.package_id
                ))
            })?;
        let path = pack_directory(&entry.directory).join(PACKAGE_CONTROL_FILE);
        warnings.push(root_issue_outcome(game, &path, &found.issue));
    }
    let declares = declarations
        .variables
        .iter()
        .any(|package| !package.variables.is_empty());
    Ok((warnings, declares.then(|| checked.provenance())))
}
