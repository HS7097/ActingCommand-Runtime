// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 S3-10 (register §1.417; `contracts/standard-pack-trial.md`; the design
//! `rd-1115/pack-stage3-design.md` §2.4 and §4.2 "S3-10", with the coordinator's rulings): the
//! offline half of `actinglab standard-pack trial`, a trial of one package on a real application.
//! [`plan_standard_pack_trial`] answers the plan of the one run the trial would send and every
//! refusal before it; it reads the package directory, the game root `--root` names and the place
//! of the state directory, and nothing else: no Runtime, no device, no installation, and it
//! writes nothing. [`write_trial_record`] writes the record of a trial,
//! `actingcommand.pack-trial.v1` under `<state>/trials/<package_id>/<content12>-<ms>.json`, and
//! [`trial_records`] and [`latest_trial_record`] read them back. In this release nothing writes a
//! record outside the tests: the run, and the record it leaves, are the next slice's.
//!
//! - **The content.** The hash of the directory given, measured by the containment crate's own
//!   `measure_content_hash` (the one a scheduled run's break-in and its failure identity measure
//!   with); its first [`CONTENT_PREFIX_DIGITS`] hex digits are the prefix they keep. Only a
//!   directory is tried: adoption rewrites a package's JSON into the canonical form, so a
//!   container and the directory adopted from it may hash differently, and what is tried must be
//!   what is adopted.
//! - **The variables.** Each declared variable with the value given or its default; a given value
//!   is judged by the contract crate's `PackageVariable::judge`, the rule a dispatch resolves a
//!   value by, and must be taken exactly as given.
//! - **The irreversible mark.** Judged only over a game root (`--root`), by the contract crate's
//!   `IrreversiblePackages` over every package that root registers, the directory tried standing
//!   for the package of its id; without a root it is `not_judged` and nothing is refused for it.
//! - **The state directory** is never a repository's or an installed root's, nor inside one: a
//!   directory along its path that holds `.git` is a repository's; one that holds the markers by
//!   which the Lab's location finds an installed root (`runtime\BUILD-MANIFEST.json` of a
//!   single-location root, `install\active.json` of a root with a selection) is an installed
//!   root.

use crate::codes::{ResourceToolingCode, TrialStatePlace};
use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{read_bounded, root_invalid};
use crate::standard_pack_goals::root_irreversibility;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CodeStr, Detail, IoOp, Outcome, RawSource};
use actingcommand_contract::{
    INSTALL_SELECTION_PATH, LabError, LabErrorClass, LabResult, PackageVariable,
    STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, StandardPackProblem,
    StandardPackRootBytes, StandardPackRootIssue, TaskOutcome, VariableProblem,
    package_id_names_one_path_segment, read_standard_pack_root,
};
use actingcommand_execution_kernel::{PreparedContainedTask, canonical_page_anchor};
use actingcommand_pack_containment::Containment;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The one version of a trial record.
pub const PACK_TRIAL_SCHEMA: &str = "actingcommand.pack-trial.v1";
/// The directory of the state directory that holds the trial records, one directory per package.
pub const TRIALS_DIRECTORY: &str = "trials";
/// The longest authorization reference, in bytes.
pub const MAX_AUTHORIZATION_BYTES: usize = 1024;
/// The hex digits of the content hash a trial keeps: the prefix a scheduled run's break-in and
/// its failure identity keep of the same hash.
pub const CONTENT_PREFIX_DIGITS: usize = 12;
/// The largest trial record read.
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
/// The label the package is admitted under: admission depends only on the package.
const TRIAL_LABEL: &str = "standard-pack-trial";
/// The flags a refusal names, as the command line writes them.
const AUTHORIZATION_FLAG: &str = "--authorization";
const VARIABLE_FLAG: &str = "--variable";
/// A Git work tree's top holds this entry (a directory, or a file in a linked work tree).
const GIT_ENTRY: &str = ".git";
/// A single-location installed root holds its Runtime member's manifest here (the place the Lab's
/// location reads, `lab_ops::locate`).
const RUNTIME_MANIFEST: [&str; 2] = ["runtime", "BUILD-MANIFEST.json"];
/// The words of the argument problems a refusal names, as the command line's are.
const NOT_RECOGNISED: &str = "not_recognised";
const OUT_OF_RANGE: &str = "out_of_range";
const REPEATED: &str = "repeated";

/// What one trial is to try: `actinglab standard-pack trial <instance> --package <dir>
/// --authorization <reference> [--variable <name>=<json>]... [--expect <outcome_key>]
/// [--irreversible] [--root <game root>] [--state-dir <dir>] --plan`.
#[derive(Debug, Clone)]
pub struct StandardPackTrialRequest {
    /// The package directory: `packs/<id>` of a checkout, or a placed pack.
    pub package: PathBuf,
    /// The instance, as given; it is not contacted.
    pub instance: String,
    /// The reference of the owner's authorization; `None` when none was given.
    pub authorization: Option<String>,
    /// The values given, each `(name, value)`, in the order given.
    pub variables: Vec<(String, Value)>,
    /// The outcome key the trial is expected to end with.
    pub expect: Option<String>,
    /// `--irreversible`: the caller acknowledges a package the irreversible mark flags.
    pub irreversible: bool,
    /// The game root the irreversible mark is judged over.
    pub root: Option<PathBuf>,
    /// The Lab's state directory, where the trial's record is kept.
    pub state_dir: PathBuf,
}

/// The answer of `standard-pack trial --plan`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StandardPackTrialAnswer {
    /// Whether a run was sent: never in this release.
    pub sent: bool,
    pub plan: TrialPlan,
    /// Where the trial's record is to be kept: `<state>/trials/<package_id>`.
    pub record_directory: String,
}

/// The plan of one trial run, as `--plan` answers it and as the record keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialPlan {
    pub package_id: String,
    /// The first [`CONTENT_PREFIX_DIGITS`] hex digits of the content hash of [`Self::package`].
    pub content_hash_prefix: String,
    /// The package directory tried, absolute.
    pub package: String,
    /// The instance, as given.
    pub instance: String,
    /// The reference of the owner's authorization, as given; never interpreted.
    pub authorization: String,
    /// The ends the entry task declares, in mapping order.
    pub ends: Vec<TrialDeclaredEnd>,
    /// Every variable the package declares, in declaration order.
    pub variables: Vec<TrialVariable>,
    pub irreversible: TrialIrreversible,
    /// What the mark was judged over; absent when it was not judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irreversible_basis: Option<TrialIrreversibleBasis>,
    /// Whether `--irreversible` was given.
    pub irreversible_acknowledged: bool,
    /// The outcome key the trial is expected to end with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
}

/// One end the entry task declares: a `scheduling_outcome` mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialDeclaredEnd {
    pub outcome_key: String,
    pub end: TrialEndKind,
    /// Its terminal pages, as the task writes them.
    pub pages: Vec<String>,
}

/// Which end a mapping is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialEndKind {
    /// Its terminal pages hold the task's target page.
    Target,
    /// Its key is the `on_empty` key of a select step.
    OnEmpty,
    /// Any other: an ending page, a normal end after the last press.
    Ending,
}

/// One declared variable and the value the trial runs with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialVariable {
    pub name: String,
    pub value: Value,
    pub source: TrialValueSource,
}

/// Where a variable's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialValueSource {
    /// `--variable`, judged by the declaration.
    Given,
    /// The declared default.
    Default,
}

/// The irreversible mark of the package tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialIrreversible {
    /// The game root counts it as able to do something irreversible.
    Marked,
    /// The game root counts it as able to do nothing irreversible.
    Unmarked,
    /// No game root was given, so the mark was not judged.
    NotJudged,
}

/// What the irreversible mark was judged over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialIrreversibleBasis {
    /// The game root, absolute.
    pub root: String,
    /// Whether its `bundle.json` registers the package's id; a package it does not register
    /// counts as marked.
    pub registered: bool,
    /// The capabilities a package of the root marks irreversible that the package provides or
    /// its remedies list.
    pub capabilities: Vec<String>,
    /// The packages of the root whose declarations could not be read; while there is one, every
    /// package counts as marked.
    pub unreadable_packages: Vec<String>,
}

/// One trial as its record keeps it (`actingcommand.pack-trial.v1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackTrialRecord {
    /// [`PACK_TRIAL_SCHEMA`].
    pub schema_version: String,
    /// When the trial ended; it names the record's file.
    pub at_unix_ms: u64,
    /// The plan the trial ran by.
    pub plan: TrialPlan,
    /// The run's request id.
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub end: TrialRunEnd,
    /// Whether the end's outcome key is the plan's `expect`; absent without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<bool>,
}

/// How the trial's run ended, as its terminal says it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialRunEnd {
    pub outcome: TaskOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<CodeStr>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub failure_values: Map<String, Value>,
}

/// Workflow #463 S3-10 (module doc): the plan of the trial `request` asks for, or its refusal, in
/// this order: the authorization, the package path, the state directory's place, the package's
/// admission, its variables, `--expect`, the irreversible mark. It writes nothing.
pub fn plan_standard_pack_trial(
    request: &StandardPackTrialRequest,
) -> LabResult<StandardPackTrialAnswer> {
    let authorization = authorization(request.authorization.as_deref())?;
    let package = absolute(&request.package)?;
    match fs::metadata(&package) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(refused(
                Outcome::new(ResourceToolingCode::TrialPackageNotDirectory).path(&package),
                format!(
                    "{} is a file, not a package directory: a trial runs packs/<id> of a checkout \
                     or a placed pack, the form adoption writes, never a .zip or .json container, \
                     whose content may hash otherwise than the directory adopted from it",
                    package.display()
                ),
            ));
        }
        Err(error) => {
            return Err(LabError::package_invalid(format!(
                "{}: {error}",
                package.display()
            )));
        }
    }
    let state = absolute(&request.state_dir)?;
    check_trial_state_dir(&state)?;
    let invalid = |error: &dyn std::fmt::Display| {
        LabError::package_invalid(format!("{}: {error}", package.display()))
    };
    let descriptor =
        PreparedContainedTask::describe_path(TRIAL_LABEL, &package, Instant::now() + PACK_DEADLINE)
            .map_err(|error| invalid(&error))?;
    let package_id = descriptor.package_id().to_owned();
    if !names_directory(&package_id) {
        return Err(LabError::package_invalid(format!(
            "the package_id {package_id:?} names no directory {TRIALS_DIRECTORY}/<package_id>"
        )));
    }
    let declarations = PreparedContainedTask::read_remedy_declarations(
        TRIAL_LABEL,
        &package,
        Instant::now() + PACK_DEADLINE,
    )
    .map_err(|error| invalid(&error))?;
    let task = entry_task(&package, descriptor.entry_task_id())?;
    let ends = declared_ends(descriptor.game(), &task);
    let content_hash = Containment::new()
        .measure_content_hash(&package, Instant::now() + PACK_DEADLINE)
        .map_err(|error| invalid(&error))?;
    let content_hash_prefix = content_hash
        .chars()
        .take(CONTENT_PREFIX_DIGITS)
        .collect::<String>();
    let variables = trial_variables(&declarations.variables, &request.variables)?;
    if let Some(expected) = &request.expect
        && !ends.iter().any(|end| end.outcome_key == *expected)
    {
        let declared = ends
            .iter()
            .map(|end| end.outcome_key.as_str())
            .collect::<Vec<_>>();
        let outcome =
            Outcome::new(ResourceToolingCode::TrialExpectationUnknown).package_id(&package_id);
        return Err(LabError::new(
            LabErrorClass::UsageValidation,
            outcome.code().as_str(),
            format!(
                "--expect {expected:?} is no outcome key the package {package_id} declares \
                 (it declares {declared:?})"
            ),
            &[],
        )
        .with_details(json!({
            "outcome": outcome.envelope(Detail::Full),
            "expected": expected,
            "declared_outcome_keys": declared,
        })));
    }
    let (irreversible, irreversible_basis) = match &request.root {
        None => (TrialIrreversible::NotJudged, None),
        Some(root) => {
            let (mark, basis) = judge_irreversible(root, &package_id, &package)?;
            (mark, Some(basis))
        }
    };
    if irreversible == TrialIrreversible::Marked && !request.irreversible {
        let root = irreversible_basis
            .as_ref()
            .map(|basis| basis.root.clone())
            .unwrap_or_default();
        return Err(refused(
            Outcome::new(ResourceToolingCode::TrialIrreversibleUnacknowledged)
                .package_id(&package_id)
                .path(&root),
            format!(
                "the game root {root} counts the package {package_id} as able to do something \
                 irreversible; a trial of it needs --irreversible as well as the authorization"
            ),
        ));
    }
    Ok(StandardPackTrialAnswer {
        sent: false,
        record_directory: state
            .join(TRIALS_DIRECTORY)
            .join(&package_id)
            .display()
            .to_string(),
        plan: TrialPlan {
            package_id,
            content_hash_prefix,
            package: package.display().to_string(),
            instance: request.instance.clone(),
            authorization,
            ends,
            variables,
            irreversible,
            irreversible_basis,
            irreversible_acknowledged: request.irreversible,
            expect: request.expect.clone(),
        },
    })
}

/// The reference `given`: not blank (`trial_authorization_missing`), at most
/// [`MAX_AUTHORIZATION_BYTES`] bytes (`validation_failed`, `out_of_range`) and without a control
/// character (`validation_failed`, `not_recognised`).
fn authorization(given: Option<&str>) -> LabResult<String> {
    let Some(reference) = given.filter(|reference| !reference.trim().is_empty()) else {
        return Err(refused(
            Outcome::new(ResourceToolingCode::TrialAuthorizationMissing),
            "a trial on a real application names the owner's authorization: --authorization \
             <reference>"
                .to_owned(),
        ));
    };
    if reference.len() > MAX_AUTHORIZATION_BYTES {
        return Err(argument_refused(
            AUTHORIZATION_FLAG,
            OUT_OF_RANGE,
            format!(
                "--authorization holds {} bytes; a reference holds at most \
                 {MAX_AUTHORIZATION_BYTES}",
                reference.len()
            ),
        ));
    }
    if reference.chars().any(char::is_control) {
        return Err(argument_refused(
            AUTHORIZATION_FLAG,
            NOT_RECOGNISED,
            "--authorization holds a control character".to_owned(),
        ));
    }
    Ok(reference.to_owned())
}

/// Every variable `declared` declares, with the value `given` names for it (judged by the
/// declaration, [`PackageVariable::judge`], and taken only exactly as given) or its default. A
/// name given twice, a name the package does not declare and a value the declaration does not
/// take as given are refused (`validation_failed` naming `--variable`, its `variable_name` and,
/// for a value, the judgement's `variable_problem`).
fn trial_variables(
    declared: &[PackageVariable],
    given: &[(String, Value)],
) -> LabResult<Vec<TrialVariable>> {
    let mut seen = BTreeSet::new();
    let mut taken = BTreeMap::new();
    for (name, value) in given {
        if !seen.insert(name.as_str()) {
            return Err(variable_refused(
                name,
                REPEATED,
                None,
                format!("--variable {name} is given twice; a variable is given once"),
            ));
        }
        let Some(variable) = declared.iter().find(|variable| variable.name == *name) else {
            return Err(variable_refused(
                name,
                NOT_RECOGNISED,
                None,
                format!("the package declares no variable {name}"),
            ));
        };
        match variable.judge(value) {
            Ok(judged) if judged == *value => {
                taken.insert(name.as_str(), judged);
            }
            Ok(_) => {
                return Err(variable_refused(
                    name,
                    NOT_RECOGNISED,
                    Some(VariableProblem::KeysLeftOut),
                    format!(
                        "the declaration of {name} would leave keys of the value given out; a \
                         trial runs a value only as given"
                    ),
                ));
            }
            Err(problem) => {
                return Err(variable_refused(
                    name,
                    NOT_RECOGNISED,
                    Some(problem),
                    format!(
                        "the declaration of {name} does not take the value given ({})",
                        problem.as_str()
                    ),
                ));
            }
        }
    }
    Ok(declared
        .iter()
        .map(|variable| match taken.remove(variable.name.as_str()) {
            Some(value) => TrialVariable {
                name: variable.name.clone(),
                value,
                source: TrialValueSource::Given,
            },
            None => TrialVariable {
                name: variable.name.clone(),
                value: variable.default_value(),
                source: TrialValueSource::Default,
            },
        })
        .collect())
}

/// The entry task `entry_task_id` of the package directory `package`, as its files hold it.
fn entry_task(package: &Path, entry_task_id: &str) -> LabResult<Value> {
    let files = Containment::for_metadata_validation()
        .snapshot_content_directory(package, Instant::now() + PACK_DEADLINE)
        .map_err(|error| LabError::package_invalid(format!("{}: {error}", package.display())))?;
    let relative = format!("resources/operations/{entry_task_id}/task.json");
    let bytes = files.get(&relative).ok_or_else(|| {
        LabError::package_invalid(format!("{} holds no {relative}", package.display()))
    })?;
    let bytes = bytes
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(bytes.as_slice());
    serde_json::from_slice(bytes).map_err(|error| {
        LabError::package_invalid(format!("{}: {relative}: {error}", package.display()))
    })
}

/// The ends the task `task` of the game `game` declares, in mapping order: each mapping of its
/// `scheduling_outcome` with its terminal pages, the target page's `target`, a select step's
/// `on_empty` key's `on_empty`, every other `ending`.
fn declared_ends(game: &str, task: &Value) -> Vec<TrialDeclaredEnd> {
    let target = task
        .get("target_page")
        .and_then(Value::as_str)
        .map(|page| canonical_page_anchor(game, page));
    let on_empty = task
        .get("operations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|operation| {
            operation
                .get("select")?
                .get("on_empty")?
                .get("outcome_key")?
                .as_str()
        })
        .collect::<BTreeSet<_>>();
    task.get("scheduling_outcome")
        .and_then(|outcome| outcome.get("mappings"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mapping| {
            let outcome_key = mapping.get("outcome_key")?.as_str()?;
            let pages = mapping
                .get("terminal_pages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let reaches_target = target.as_deref().is_some_and(|target| {
                pages
                    .iter()
                    .any(|page| canonical_page_anchor(game, page) == target)
            });
            let end = if reaches_target {
                TrialEndKind::Target
            } else if on_empty.contains(outcome_key) {
                TrialEndKind::OnEmpty
            } else {
                TrialEndKind::Ending
            };
            Some(TrialDeclaredEnd {
                outcome_key: outcome_key.to_owned(),
                end,
                pages,
            })
        })
        .collect()
}

/// The irreversible mark of the package `package_id`, the directory `package`, over the game root
/// `root`: its root files read as every reader reads them (a refused or missing root file is
/// `standard_pack_root_invalid`), then the contract crate's `IrreversiblePackages` over every
/// package its `bundle.json` registers, read by the reader the goal check and the composer use,
/// `package` read in place of the registered directory of its id.
fn judge_irreversible(
    root: &Path,
    package_id: &str,
    package: &Path,
) -> LabResult<(TrialIrreversible, TrialIrreversibleBasis)> {
    let root = absolute(root)?;
    if !fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(LabError::usage(format!(
            "--root {} is no directory",
            root.display()
        )));
    }
    let game = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            LabError::usage(format!("--root {} names no game directory", root.display()))
        })?
        .to_owned();
    let mut files = BTreeMap::new();
    for name in [
        STANDARD_PACK_BUNDLE_FILE,
        STANDARD_PACK_PAGES_FILE,
        STANDARD_PACK_INTERRUPTS_FILE,
        STANDARD_PACK_NAVIGATION_FILE,
        STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    ] {
        if let Some(bytes) = read_bounded(&root.join(name))? {
            files.insert(name, bytes);
        }
    }
    let bytes = |name: &str| files.get(name).map(Vec::as_slice);
    let checked = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: bytes(STANDARD_PACK_BUNDLE_FILE),
            pages: bytes(STANDARD_PACK_PAGES_FILE),
            interrupts: bytes(STANDARD_PACK_INTERRUPTS_FILE),
            navigation: bytes(STANDARD_PACK_NAVIGATION_FILE),
            tables: false,
            display_layout: bytes(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
        },
        &game,
    )
    .map_err(|issue| root_invalid(&game, &root.join(issue.file), &issue))?
    .ok_or_else(|| {
        let issue = StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        };
        root_invalid(&game, &root.join(STANDARD_PACK_BUNDLE_FILE), &issue)
    })?;
    let registered = checked
        .registry
        .get(package_id)
        .map(|entry| entry.directory.clone());
    let read = root_irreversibility(&checked, &|directory: &str| {
        if registered.as_deref() == Some(directory) {
            package.to_path_buf()
        } else {
            root.join(directory)
        }
    });
    let packages = &read.packages;
    let mark = if packages.counts_irreversible(package_id) {
        TrialIrreversible::Marked
    } else {
        TrialIrreversible::Unmarked
    };
    let basis = TrialIrreversibleBasis {
        root: root.display().to_string(),
        registered: registered.is_some(),
        capabilities: packages
            .irreversible_capabilities(package_id)
            .map(|capabilities| capabilities.iter().cloned().collect())
            .unwrap_or_default(),
        unreadable_packages: packages.unreadable().iter().cloned().collect(),
    };
    Ok((mark, basis))
}

/// Workflow #463 S3-10: the state directory `state_dir` (made absolute, then also with its
/// nearest existing part's links resolved) is no repository's or installed root's, nor inside
/// one; otherwise `trial_state_dir_refused` with the word of the place. It reads whether those
/// entries exist along the path, and nothing else.
pub fn check_trial_state_dir(state_dir: &Path) -> LabResult<()> {
    let state = absolute(state_dir)?;
    let resolved = state
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .and_then(|existing| fs::canonicalize(existing).ok());
    for ancestor in state
        .ancestors()
        .chain(resolved.iter().flat_map(|path| path.ancestors()))
    {
        let place = if ancestor.join(GIT_ENTRY).exists() {
            TrialStatePlace::Repository
        } else if ancestor
            .join(RUNTIME_MANIFEST[0])
            .join(RUNTIME_MANIFEST[1])
            .is_file()
            || ancestor.join(INSTALL_SELECTION_PATH).is_file()
        {
            TrialStatePlace::InstalledRoot
        } else {
            continue;
        };
        return Err(refused(
            Outcome::new(ResourceToolingCode::TrialStateDirRefused)
                .path(&state)
                .trial_state_place(place),
            format!(
                "the state directory {} lies inside {} ({}); trial records are Lab authoring \
                 state, kept beside the recordings and never in a repository or an installed \
                 root: give --state-dir outside it",
                state.display(),
                ancestor.display(),
                place.as_str()
            ),
        ));
    }
    Ok(())
}

/// Where the record of a trial of the content `content_hash_prefix` of the package `package_id`
/// that ended at `at_unix_ms` is kept: `<state>/trials/<package_id>/<content12>-<ms>.json`.
pub fn trial_record_path(
    state_dir: &Path,
    package_id: &str,
    content_hash_prefix: &str,
    at_unix_ms: u64,
) -> PathBuf {
    state_dir
        .join(TRIALS_DIRECTORY)
        .join(package_id)
        .join(format!("{content_hash_prefix}-{at_unix_ms}.json"))
}

/// Workflow #463 S3-10: writes `record` once at its place ([`trial_record_path`]) under the state
/// directory `state_dir`, which [`check_trial_state_dir`] must accept, and answers that path. The
/// record is written whole under its name with `.part` added and renamed into place; a record of
/// that name, or a `.part` of it, that is there already is never replaced
/// (`trial_record_write_failed`, the operating system's refusal or `AlreadyExists` as its cause).
/// Nothing is removed.
pub fn write_trial_record(state_dir: &Path, record: &PackTrialRecord) -> LabResult<PathBuf> {
    check_trial_state_dir(state_dir)?;
    let state = absolute(state_dir)?;
    if record.schema_version != PACK_TRIAL_SCHEMA
        || !names_directory(&record.plan.package_id)
        || !content_prefix(&record.plan.content_hash_prefix)
    {
        return Err(LabError::usage(format!(
            "the record is not one of {PACK_TRIAL_SCHEMA} whose package_id names a directory and \
             whose content_hash_prefix is {CONTENT_PREFIX_DIGITS} hex digits"
        )));
    }
    let path = trial_record_path(
        &state,
        &record.plan.package_id,
        &record.plan.content_hash_prefix,
        record.at_unix_ms,
    );
    let directory = state.join(TRIALS_DIRECTORY).join(&record.plan.package_id);
    fs::create_dir_all(&directory)
        .map_err(|error| write_failed(&directory, &error, IoOp::CreateDir))?;
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let taken = io::Error::from(io::ErrorKind::AlreadyExists);
            return Err(write_failed(&path, &taken, IoOp::Create));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(write_failed(&path, &error, IoOp::Metadata)),
    }
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|error| LabError::usage(format!("the trial record does not encode: {error}")))?;
    let part = path.with_extension("json.part");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|error| write_failed(&part, &error, IoOp::Create))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| write_failed(&part, &error, IoOp::Write))?;
    drop(file);
    fs::rename(&part, &path).map_err(|error| write_failed(&part, &error, IoOp::Rename))?;
    Ok(path)
}

/// Workflow #463 S3-10: every trial record of the package `package_id` under the state directory
/// `state_dir`, in the order the trials ended (then by content); none without a record. A file of
/// a record's name (`<content12>-<ms>.json`) that does not read as the record of that package,
/// content and time is `trial_record_unreadable`, never passed over; another name (a `.part` a
/// write left) is no record.
pub fn trial_records(state_dir: &Path, package_id: &str) -> LabResult<Vec<PackTrialRecord>> {
    if !names_directory(package_id) {
        return Err(LabError::usage(format!(
            "the package_id {package_id:?} names no directory {TRIALS_DIRECTORY}/<package_id>"
        )));
    }
    let directory = absolute(state_dir)?.join(TRIALS_DIRECTORY).join(package_id);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(read_failed(&directory, &error, IoOp::ReadDir)),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| read_failed(&directory, &error, IoOp::ReadDir))?;
        let name = entry.file_name();
        let Some((prefix, at_unix_ms)) = name.to_str().and_then(record_name) else {
            continue;
        };
        let path = entry.path();
        let record = read_record(&path)?;
        let named = record.schema_version == PACK_TRIAL_SCHEMA
            && record.plan.package_id == package_id
            && record.plan.content_hash_prefix == prefix
            && record.at_unix_ms == at_unix_ms;
        if !named {
            return Err(unreadable(
                &path,
                None,
                format!(
                    "{} is no {PACK_TRIAL_SCHEMA} record of the package {package_id}, the content \
                     {prefix} and the time {at_unix_ms} its name says",
                    path.display()
                ),
            ));
        }
        records.push(record);
    }
    records.sort_by(|left, right| {
        left.at_unix_ms.cmp(&right.at_unix_ms).then_with(|| {
            left.plan
                .content_hash_prefix
                .cmp(&right.plan.content_hash_prefix)
        })
    });
    Ok(records)
}

/// Workflow #463 S3-10: the latest trial record of the content `content_hash_prefix` of the
/// package `package_id` ([`trial_records`]); none when that content was never tried.
pub fn latest_trial_record(
    state_dir: &Path,
    package_id: &str,
    content_hash_prefix: &str,
) -> LabResult<Option<PackTrialRecord>> {
    Ok(trial_records(state_dir, package_id)?
        .into_iter()
        .rev()
        .find(|record| record.plan.content_hash_prefix == content_hash_prefix))
}

/// The content prefix and the time a record's file name `name` says, `<content12>-<ms>.json`;
/// `None` for any other name.
fn record_name(name: &str) -> Option<(&str, u64)> {
    let (prefix, at) = name.strip_suffix(".json")?.split_once('-')?;
    if !content_prefix(prefix) || at.is_empty() || !at.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((prefix, at.parse::<u64>().ok()?))
}

/// One record file, read whole (at most [`MAX_RECORD_BYTES`]).
fn read_record(path: &Path) -> LabResult<PackTrialRecord> {
    let file = fs::File::open(path).map_err(|error| read_failed(path, &error, IoOp::Open))?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| read_failed(path, &error, IoOp::Read))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(unreadable(
            path,
            None,
            format!("{} is larger than {MAX_RECORD_BYTES} bytes", path.display()),
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        let cause = Outcome::new(ContractCode::ForeignDataError)
            .raw_source(RawSource::Json)
            .raw_text(error.to_string());
        unreadable(
            path,
            Some(cause),
            format!(
                "{} is no {PACK_TRIAL_SCHEMA} record: {error}",
                path.display()
            ),
        )
    })
}

/// Whether `text` is a content prefix: [`CONTENT_PREFIX_DIGITS`] lowercase hex digits.
fn content_prefix(text: &str) -> bool {
    text.len() == CONTENT_PREFIX_DIGITS
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `package_id` names one directory below `trials/`, and fits an outcome's name.
fn names_directory(package_id: &str) -> bool {
    package_id.len() <= 256 && package_id_names_one_path_segment(package_id)
}

fn absolute(path: &Path) -> LabResult<PathBuf> {
    std::path::absolute(path)
        .map_err(|error| LabError::usage(format!("{}: {error}", path.display())))
}

/// The Lab error of the refusal `outcome`.
fn refused(outcome: Outcome, message: String) -> LabError {
    LabError::new(
        LabErrorClass::UsageValidation,
        outcome.code().as_str(),
        message,
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

/// `validation_failed` naming the flag `flag` with the argument problem `problem`, as the command
/// line's refusals name it.
fn argument_refused(flag: &str, problem: &str, message: String) -> LabError {
    LabError::usage(message).with_details(json!({
        "argument": flag,
        "argument_problem": problem,
        "arguments": [{"argument": flag, "argument_problem": problem}],
    }))
}

/// A refused `--variable`: [`argument_refused`] with the variable's name and, for a value, the
/// judgement's word.
fn variable_refused(
    name: &str,
    problem: &str,
    judged: Option<VariableProblem>,
    message: String,
) -> LabError {
    let mut error = argument_refused(VARIABLE_FLAG, problem, message);
    if let Some(details) = error.details.as_mut() {
        details["variable_name"] = json!(name);
        if let Some(judged) = judged {
            details["variable_problem"] = json!(judged.as_str());
        }
    }
    error
}

/// `trial_record_write_failed` at `path`, the operating system's refusal `error` of `op` its
/// cause.
fn write_failed(path: &Path, error: &io::Error, op: IoOp) -> LabError {
    let outcome = Outcome::new(ResourceToolingCode::TrialRecordWriteFailed)
        .path(path)
        .caused_by(Outcome::from_io_error(error, op).path(path));
    refused(
        outcome,
        format!(
            "the trial record {} could not be written: {error}",
            path.display()
        ),
    )
}

/// `trial_record_unreadable` at `path`, the operating system's refusal `error` of `op` its cause.
fn read_failed(path: &Path, error: &io::Error, op: IoOp) -> LabError {
    unreadable(
        path,
        Some(Outcome::from_io_error(error, op).path(path)),
        format!("{} could not be read: {error}", path.display()),
    )
}

/// `trial_record_unreadable` at `path`, with `cause` when there is one.
fn unreadable(path: &Path, cause: Option<Outcome>, message: String) -> LabError {
    let outcome = Outcome::new(ResourceToolingCode::TrialRecordUnreadable).path(path);
    let outcome = match cause {
        Some(cause) => outcome.caused_by(cause),
        None => outcome,
    };
    refused(outcome, message)
}
