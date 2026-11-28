// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.356; contracts/standard-pack-goals.md): the Tools' side of a
//! game root's goal declaration `goals.json`. `actinglab standard-pack check` and `package bundle
//! --goals` read it after the composition they already run, with the contract crate's reader
//! (`parse_standard_pack_goals`), and hold an accepted file's references to the game root
//! (`check_standard_pack_goal_references`): the pools and rules of `schedule.json`, which the
//! composition has accepted, each rule with the package it runs; and every package of
//! `bundle.json`, read by the Runtime's metadata reader, with (Workflow #463 V1) the variables it
//! declares, which a `variable` entry places. Workflow #455 G-SLx (register §1.501): what a
//! package provides and its `irreversible` marks are no reference of the goal check any more;
//! until 0.12.77 an entry that governed, reached or followed a package that can do something
//! irreversible was `goal_governs_irreversible`. Nothing here reads or writes a value.
//!
//! `standard-pack check` reports each problem as a `standard_pack_root_invalid` warning and never
//! fails the root for it ([`goal_warnings`]); `package bundle` refuses a refused file as it
//! refuses a refused root file ([`refuse_goals`]).
//!
//! Workflow #463 V1 (register §1.378; contracts/package-variables.md): `standard-pack check` also
//! runs the root check of the variables the root's packages declare ([`variable_report`]): each
//! problem is a `standard_pack_root_invalid` warning naming the package's `control.json`, the root
//! is checked as before, and the answer lists each variable that applies by its default's origin.
//! Its fix round 1 judged the packages by the rule of irreversibility over the whole root and
//! listed a variable of a package that counted as irreversible as not applying, until Workflow
//! #455 G-SLx (register §1.501), since which such a variable applies as any other.
//!
//! Workflow #455 RR1 (register §1.535; contracts/standard-pack-goal-tables.md): the root's
//! `goal-tables.json` is checked in the same place ([`goal_issues`]): read with the contract
//! crate's reader (`parse_standard_pack_goal_tables`), and an accepted file's pools held to the
//! composed schedule's (`check_standard_pack_goal_table_references`) with the same references the
//! goal declaration is held to, the pools' one judgement of whether a goal can be put on them; a
//! file both accept gives its tables to the references of `goals.json`, whose `records` entries
//! name them. Each file's problems are its own, and each is reported as `goals.json`'s are.

use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{CHECK_LABEL, root_invalid, root_issue_outcome};
use crate::standard_pack_schedule::issue_path;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    DeclaredCapabilities, GoalPool, GoalReferences, GoalRule, IrreversiblePackages, LabError,
    LabResult, MAX_STANDARD_PACK_GOAL_TABLES_BYTES, MAX_STANDARD_PACK_GOALS_BYTES,
    PACKAGE_CONTROL_FILE, RootGoalDocument, RootPackageVariables, StandardPackRoot,
    StandardPackRootIssue, VariableOrigins, check_standard_pack_goal_references,
    check_standard_pack_goal_table_references, goal_variables, judge_root_variables,
    parse_standard_pack_goal_tables, parse_standard_pack_goals, parse_standard_pack_schedule,
    take_catalog_business,
};
use actingcommand_execution_kernel::{ContainedTaskError, PreparedContainedTask};
use actingcommand_policy::{
    POOL_SPEND_LINE_MEMBER, POOL_TARGET_DEFAULTS_MEMBER, standard_pack_schedule_takes_catalog,
};
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
    read_bounded_file(path, MAX_STANDARD_PACK_GOALS_BYTES)
}

/// Workflow #455 RR1: the bytes of the game root's `goal-tables.json` at `path`, as
/// [`read_goals_file`] reads `goals.json`: `None` when the root holds none, through the file's
/// bound plus one byte (a file of any size is then the reader's `table_out_of_limits`).
pub(crate) fn read_goal_tables_file(path: &Path) -> LabResult<Option<Vec<u8>>> {
    read_bounded_file(path, MAX_STANDARD_PACK_GOAL_TABLES_BYTES)
}

/// The bytes of the file at `path` through `bound` plus one byte, `None` when it does not exist;
/// a file that cannot be read is a usage error naming it.
fn read_bounded_file(path: &Path, bound: usize) -> LabResult<Option<Vec<u8>>> {
    let unreadable = |error: io::Error| LabError::usage(format!("{}: {error}", path.display()));
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    let mut bytes = Vec::new();
    file.take(bound as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    Ok(Some(bytes))
}

/// Reads every package of `root` (`pack_directory` is where a registry entry's directory lies)
/// with the Runtime's metadata reader, in registry order: each package with (Workflow #463 V1)
/// the variables it declares and the fact keys its designated effects read their caps from; a
/// package it refuses is `package_invalid` naming its directory. Workflow #455 G-SLx (register
/// §1.501): until 0.12.77 also what the rule of irreversibility judged of each package, which no
/// reader here needs since.
fn package_variables(
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<Vec<RootPackageVariables>> {
    let mut variables = Vec::new();
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
        variables.push(RootPackageVariables {
            package_id: package_id.to_owned(),
            variables: read.variables,
            cap_fact_keys: read.cap_fact_keys,
        });
    }
    Ok(variables)
}

/// Workflow #455 RR1: the bytes of the game root's two goal files a check holds to the root, each
/// `None` when the root holds none.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GoalFiles<'a> {
    /// `goals.json`.
    pub(crate) goals: Option<&'a [u8]>,
    /// `goal-tables.json`.
    pub(crate) tables: Option<&'a [u8]>,
}

/// Workflow #455 RR1: the problems of each of a game root's two goal files, each in its file's
/// order; both empty for files that keep every rule.
#[derive(Debug, Clone, Default)]
pub(crate) struct GoalIssues {
    /// `goals.json`'s.
    pub(crate) goals: Vec<StandardPackRootIssue>,
    /// `goal-tables.json`'s.
    pub(crate) tables: Vec<StandardPackRootIssue>,
}

/// The problems of the goal declaration (the bytes of `goals.json`) and (Workflow #455 RR1) of
/// the table of rules for raising (the bytes of `goal-tables.json`) of the game root `root` of
/// `game`, each in file order: the reader's, or, for a file the reader accepts, its references'.
/// `schedule` is `schedule.json`'s bytes and path when the root holds one (the composition has
/// accepted it); each of its rules is held with the package it runs (Workflow #455 G-SLx, register
/// §1.501: no longer with the rules that follow it), and each of its pools is judged once, for
/// both files, by whether a goal can be put on it; `pack_directory` is where a registry entry's
/// directory lies. RR1: the tables file is held to the pools first; one that both its reader and
/// its references accept gives its tables to the references of `goals.json` (a `records` entry
/// names one), one that is refused leaves a records entry's table unjudged, and a root without it
/// has no table. A package that cannot be read is the caller's error, not a problem of a file; the
/// packages are read only for an accepted `goals.json`. Workflow #477 CV-2 (register §1.508): a
/// `schedule.json` that names `catalog_business` is held with its rules' business part taken from
/// `catalog`, the root's `pack-catalog.json`, as the composer takes it (a `tier` entry's rule has
/// its fixed value there); a refusal of it is the caller's error, at the file it names.
pub(crate) fn goal_issues(
    game: &str,
    root: &StandardPackRoot,
    schedule: Option<(&[u8], &Path)>,
    catalog: Option<&[u8]>,
    files: GoalFiles<'_>,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<GoalIssues> {
    let mut found = GoalIssues::default();
    let declared = match files
        .goals
        .map(|bytes| parse_standard_pack_goals(bytes, game))
    {
        None => None,
        Some(Ok(declared)) => Some(declared),
        Some(Err(issues)) => {
            found.goals = issues;
            None
        }
    };
    let tables = match files
        .tables
        .map(|bytes| parse_standard_pack_goal_tables(bytes, game))
    {
        None => None,
        Some(Ok(tables)) => Some(tables),
        Some(Err(issues)) => {
            found.tables = issues;
            None
        }
    };
    if declared.is_none() && tables.is_none() {
        return Ok(found);
    }
    let mut references =
        GoalReferences::new(root.registry.package_ids().map(str::to_owned).collect());
    if declared.is_some() {
        // G-R1 fix round 1: every package of the root, each read by the Runtime's metadata reader
        // in registry order as the remedy report reads them (one it refuses is the check's
        // error). Workflow #463 V1: every variable a package declares, which a placement names.
        let variables = package_variables(root, pack_directory)?;
        references.variables = goal_variables(&variables);
    }
    if let Some((bytes, path)) = schedule {
        let mut schedule = parse_standard_pack_schedule(bytes)
            .map_err(|issue| root_invalid(game, path, &issue))?;
        if standard_pack_schedule_takes_catalog(bytes) {
            take_catalog_business(&mut schedule, catalog, game, &root.registry)
                .map_err(|issue| root_invalid(game, &issue_path(path, &issue), &issue))?;
        }
        for pool in &schedule.pools {
            // A pool a goal may name has a valuation block with its step and its gap weight, or
            // (#455 B3-P, register §1.522) writes no valuation and `target_defaults` with both.
            // B3-P fix round 1: "no valuation" as the compiler reads the member, absent or `null`
            // (as `CatalogScheduleFacts` reads it); the pending marker is a valuation.
            let written = pool
                .body
                .get("valuation")
                .filter(|valuation| !valuation.is_null());
            let valued = match written {
                Some(Value::Object(block)) => {
                    block.contains_key("scale") && block.contains_key("gap")
                }
                Some(_) => false,
                None => matches!(
                    pool.body.get(POOL_TARGET_DEFAULTS_MEMBER),
                    Some(Value::Object(defaults))
                        if defaults.contains_key("scale") && defaults.contains_key("importance_milli")
                ),
            };
            // #455 SL2 (register §1.547): any other pool that writes `spend_line` takes its line
            // from a `resource` entry, and no need lands on it; the one map says which.
            let offered = if valued {
                GoalPool::Valued
            } else if matches!(
                pool.body.get(POOL_SPEND_LINE_MEMBER),
                Some(Value::Object(_))
            ) {
                GoalPool::Line
            } else {
                GoalPool::Unvalued
            };
            references.pools.insert(pool.id.clone(), offered);
        }
        for rule in &schedule.rules {
            // G-R1 fix round 1: with the package it runs, which a `tier` entry is held to.
            references.rules.insert(
                rule.id.clone(),
                GoalRule {
                    package_id: rule.package_id.clone(),
                    fixed_value: rule.fixed_value_milli.is_some(),
                },
            );
        }
    }
    // Workflow #455 RR1: the tables' pools, then the tables a records entry may name: none of a
    // root without the file (the references' default), those of a file that keeps every rule, and
    // no judgement while the root's file is refused.
    if files.tables.is_some() {
        references.tables = match &tables {
            Some(tables) => {
                found.tables = check_standard_pack_goal_table_references(tables, &references);
                found.tables.is_empty().then(|| tables.table_names())
            }
            None => None,
        };
    }
    if let Some(declared) = &declared {
        found.goals = check_standard_pack_goal_references(declared, &references);
    }
    Ok(found)
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
/// Workflow #463 V2 fix round 2: through the contract crate's one judgement of a root's variables
/// (`judge_root_variables`), which the Runtime's scheduler workbench calls too, with the goal
/// document absent, accepted or refused as read here. Workflow #455 G-SLx (register §1.501):
/// until 0.12.77 (since V1 fix round 1) the packages were also judged by the rule of
/// irreversibility over the whole root, and a variable of a package that counted as irreversible
/// was `variable_irreversible_by_root` and listed as not applying; neither since.
pub(crate) fn variable_report(
    game: &str,
    root: &StandardPackRoot,
    goals: Option<&[u8]>,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<(Vec<Outcome>, Option<VariableOrigins>)> {
    let variables = package_variables(root, pack_directory)?;
    let read = goals.map(|bytes| parse_standard_pack_goals(bytes, game));
    // Workflow #463 V2 fix round 2: the goal document as the contract crate's one judgement
    // takes it, the Runtime's own. A `goals.json` that cannot be read never reaches here: it is
    // the check's usage error naming the file (`read_goals_file`).
    let document = match &read {
        None => RootGoalDocument::Absent,
        Some(Ok(accepted)) => RootGoalDocument::Accepted(accepted),
        Some(Err(_)) => RootGoalDocument::Refused,
    };
    let checked = judge_root_variables(&variables, document).checked;
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
    let declares = variables
        .iter()
        .any(|package| !package.variables.is_empty());
    Ok((warnings, declares.then(|| checked.provenance())))
}
