// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 S3-10 (register §1.417; `contracts/standard-pack-trial.md`; the design
//! `rd-1115/pack-stage3-design.md` §2.4 and §4.2 "S3-10", with the coordinator's rulings): the
//! offline half of `actinglab standard-pack trial`, a trial of one package on a real application.
//! [`plan_standard_pack_trial`] answers the plan of the one run the trial would send and every
//! refusal before it; it reads the package directory once (and, over a root, once more for the
//! irreversible mark), the game root `--root` names and the place of the state directory, and
//! nothing else: no Runtime, no device, no installation, and it writes nothing. A plan prepares
//! and shows (fix round 2, the coordinator's ruling 1): it needs no authorization and refuses
//! nothing only the run needs; what the run will require that the plan was not given, it says.
//! [`write_trial_record`] writes the record of a trial,
//! `actingcommand.pack-trial.v1` under `<state>/trials/<package_id>/<content12>-<ms>.json`, and
//! [`trial_records`] and [`latest_trial_record`] read them back; S3-11 fix round 2:
//! [`trial_record_of_run`] finds the record of one run, whatever content it names.
//!
//! Workflow #463 S3-11 (register §1.418; `contracts/standard-pack-trial.md`, "The run"): the run
//! itself is actinglab's (it sends one request through the Lab's operation layer and reads the
//! run's end as `run show` reads it); this module gives it the offline parts. [`check_trial_run`]
//! answers the refusals only the run needs, after the plan and before anything is sent or written,
//! and the plan as the record keeps it; [`PackTrialRecord::of_run`] makes the record of a run whose
//! end was read, `matched` judged against `--expect`, and [`PackTrialRecord::unexpected_end`] the
//! warning of a run that ended normally on another end than the one expected. The writer is the
//! record's only writer, called once per run whose end was read. Three sentences stay true: a
//! passed trial is not an authorization to enable; a trial counts nothing toward a break-in; a
//! trial is one run, sent once.
//!
//! - **One reading** (fix round 2). The package directory is read once, one bounded snapshot by
//!   the containment crate's `snapshot_content_directory`, and every member of the plan but the
//!   irreversible mark comes from those bytes: the content hash, the kernel's admission of them
//!   (`PreparedContainedTask::load_content_entries`), the declared ends and the declared
//!   variables. Fix round 3: over a root the mark is judged by the resource tooling's own
//!   `root_irreversibility`, called, not copied; the reader it calls of a package's declarations
//!   (`PreparedContainedTask::read_remedy_declarations`) takes only a path, so it reads the
//!   directory tried once more, given in place of the registered directory of its id.
//! - **A record holds an authorization** (fix round 3). Its plan is a [`TrialRecordPlan`], whose
//!   `authorization` is a text, made from a [`TrialPlan`] only by a conversion that refuses a plan
//!   without one; the reader decodes into it, so a stored record without one does not decode.
//! - **The content.** The hash of the directory given: the contract crate's
//!   `content_directory_digest` of each file's SHA-256, what containment's `measure_content_hash`
//!   computes for a directory (the hash a scheduled run's break-in and its failure identity
//!   measure); its first [`CONTENT_PREFIX_DIGITS`] hex digits are the prefix they keep. Only a
//!   directory is tried: adoption rewrites a package's JSON into the canonical form, so a
//!   container and the directory adopted from it may hash differently, and what is tried must be
//!   what was adopted.
//! - **The variables.** Each declared variable with the value given or its default; a given value
//!   is judged by the contract crate's `PackageVariable::judge`, the rule a dispatch resolves a
//!   value by, and must be taken exactly as given.
//! - **The irreversible mark.** Judged only over a game root (`--root`), by the contract crate's
//!   `IrreversiblePackages` over every package that root registers, the directory tried standing
//!   for the package of its id; without a root it is `not_judged`. It is shown, never refused.
//! - **The state directory** is never a repository's or an installed root's, nor inside one: a
//!   directory along its path that holds `.git` is a repository's; one whose `install\state.json`
//!   is a file is an installed root ([`installed_root_containing`]).
//! - **The writer's steps** (S3-11, the review of S3-10's third round): the hard link of the
//!   `.part` and the write and flush of a file created anew are taken as parameters inside the
//!   module (`WriteSteps`), as the Runtime host's `place_once` takes its link, so that the tests
//!   drive the paths the operating system's refusals take.

use crate::codes::{ResourceToolingCode, TrialRecordProblem, TrialStatePlace};
use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{read_bounded, root_invalid};
use crate::standard_pack_goals::root_irreversibility;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CauseRelation, CodeStr, Detail, IoOp, Outcome, RawSource};
use actingcommand_contract::{
    LabError, LabErrorClass, LabResult, PackageVariable, ProvidedCapability,
    STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, StandardPackProblem,
    StandardPackRootBytes, StandardPackRootIssue, TaskOutcome, VariableProblem,
    content_directory_digest, package_id_names_one_path_segment, parse_package_variables,
    read_standard_pack_root,
};
use actingcommand_execution_kernel::{PreparedContainedTask, canonical_page_anchor};
use actingcommand_pack_containment::{Containment, Sha256Hash};
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
/// A package's control declaration, at the top of its directory.
const CONTROL_FILE: &str = "control.json";
/// The `execution_mode` whose other ends are ending pages (`contracts/linear-steps.md`,
/// "Endings").
const LINEAR_STEPS: &str = "linear_steps";
/// The flags a refusal names, as the command line writes them.
const AUTHORIZATION_FLAG: &str = "--authorization";
const PACKAGE_FLAG: &str = "--package";
const VARIABLE_FLAG: &str = "--variable";
const EXPECT_FLAG: &str = "--expect";
/// A Git work tree's top holds this entry (a directory, or a file in a linked work tree).
const GIT_ENTRY: &str = ".git";
/// The installer's folder of an installed root and its own state file in it, in both layouts
/// (`contracts/install-cli.md`, "Layout"). The Runtime host recognises an installation root of one
/// directory by the same file (`installation_root_at`); the resource tooling may not reach the
/// host's crate (the architecture guard `production_packages_cannot_reach_resource_tooling`), so
/// only the file's name is repeated here.
const INSTALL_FOLDER: &str = "install";
const INSTALLER_STATE_FILE: &str = "state.json";
/// The words of the argument problems a refusal names, as the command line's are (the vocabulary
/// `argument_problem`).
const NOT_RECOGNISED: &str = "not_recognised";
const OUT_OF_RANGE: &str = "out_of_range";
const REPEATED: &str = "repeated";
const UNUSABLE: &str = "unusable";
const VALUE_MISSING: &str = "value_missing";

/// What one trial is to try: `actinglab standard-pack trial <instance> --package <dir>
/// [--authorization <reference>] [--variable <name>=<json>]... [--expect <outcome_key>]
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
    /// What the run will require that this plan was not given, in this order; empty when
    /// nothing is missing.
    pub run_requires: Vec<TrialRunRequirement>,
    /// Where the trial's record is to be kept: `<state>/trials/<package_id>`.
    pub record_directory: String,
}

/// Something the run will require that a plan may be made without.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialRunRequirement {
    /// `--authorization`: the run is sent only under the owner's authorization.
    Authorization,
}

/// The plan of one trial run, as `--plan` answers it; a record keeps it as a [`TrialRecordPlan`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrialPlan {
    pub package_id: String,
    /// The first [`CONTENT_PREFIX_DIGITS`] hex digits of the content hash of [`Self::package`].
    pub content_hash_prefix: String,
    /// The package directory tried, absolute.
    pub package: String,
    /// The instance, as given.
    pub instance: String,
    /// The reference of the owner's authorization, as given and never interpreted; `null` in a
    /// plan made without one. A record's plan always holds one ([`TrialRecordPlan`]).
    pub authorization: Option<String>,
    /// The ends the entry task declares, in mapping order.
    pub ends: Vec<TrialDeclaredEnd>,
    /// Every variable the package declares, in declaration order.
    pub variables: Vec<TrialVariable>,
    pub irreversible: TrialIrreversible,
    /// What the mark was judged over; absent when it was not judged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub irreversible_basis: Option<TrialIrreversibleBasis>,
    /// Whether `--irreversible` was given.
    pub irreversible_acknowledged: bool,
    /// The outcome key the trial is expected to end with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
}

/// Workflow #463 S3-10 fix round 3: the plan a trial record keeps, the [`TrialPlan`] the trial ran
/// by with its `authorization` as a text: a record without an authorization cannot be built, and
/// one stored without it (the member absent or `null`) does not decode, so the reader refuses it
/// (`trial_record_unreadable` `undecodable`). Outside this crate it is made only from a plan by
/// its conversion (`TrialRecordPlan::try_from`), which refuses a plan without an authorization,
/// and read back by the reader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TrialRecordPlan {
    pub package_id: String,
    /// The first [`CONTENT_PREFIX_DIGITS`] hex digits of the content hash of [`Self::package`].
    pub content_hash_prefix: String,
    /// The package directory tried, absolute.
    pub package: String,
    /// The instance, as given.
    pub instance: String,
    /// The reference of the owner's authorization the run was sent under, as given and never
    /// interpreted.
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

impl TryFrom<TrialPlan> for TrialRecordPlan {
    type Error = LabError;

    /// The plan `plan` as a record keeps it. A plan without an authorization makes no record
    /// plan: `validation_failed`, as [`write_trial_record`] refuses another malformed record.
    /// Workflow #463 S3-11 (the review of S3-10's third round, item 3): the authorization is held
    /// to the bounds the plan's own check holds it to, by calling that check: blank
    /// (`value_missing`), longer than [`MAX_AUTHORIZATION_BYTES`] (`out_of_range`) or with a
    /// control character (`not_recognised`), each `validation_failed` naming `--authorization`.
    fn try_from(plan: TrialPlan) -> Result<Self, Self::Error> {
        let Some(authorization) = authorization(plan.authorization.as_deref())? else {
            return Err(LabError::usage(format!(
                "the plan of the package {} names no authorization; a trial record keeps the \
                 authorization its run was sent under",
                plan.package_id
            )));
        };
        Ok(Self {
            package_id: plan.package_id,
            content_hash_prefix: plan.content_hash_prefix,
            package: plan.package,
            instance: plan.instance,
            authorization,
            ends: plan.ends,
            variables: plan.variables,
            irreversible: plan.irreversible,
            irreversible_basis: plan.irreversible_basis,
            irreversible_acknowledged: plan.irreversible_acknowledged,
            expect: plan.expect,
        })
    }
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

/// Which end a mapping is, named as the package's own declarations name it (fix round 2), in the
/// order they are told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialEndKind {
    /// Its terminal pages hold a target page of the task.
    Target,
    /// Its key is the `on_empty` key of a select step.
    OnEmpty,
    /// Its key is the key of an operation's keyed `on_max_runs`.
    OnMaxRuns,
    /// Its key is the key of an operation's keyed `on_guard_refused`.
    OnGuardRefused,
    /// Its key is a wait step's `on_timeout`.
    OnTimeout,
    /// Its terminal pages hold a page a stage lists in its `end_pages`.
    EndPage,
    /// In a `linear_steps` task, any other: an ending page, a normal end after the last press.
    Ending,
    /// In a page graph, any other: a mapping none of the above names.
    Other,
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
    /// The plan the trial ran by, with the authorization its run was sent under.
    pub plan: TrialRecordPlan,
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

/// What [`write_trial_record`] answers (fix round 3): the record, in place, and what the write left
/// beside it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WrittenTrialRecord {
    /// The record's file, `<state>/trials/<package_id>/<content12>-<ms>.json`.
    pub path: PathBuf,
    /// The `.part` the write created that is left beside the record because the operating system
    /// refused its removal once the record was in place; absent when none is left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_left: Option<PathBuf>,
    /// One `trial_record_part_left` naming [`Self::part_left`] when there is one, in the shape the
    /// Lab's answers carry warnings in; empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Outcome>,
}

/// The package directory tried, read once ([`read_tried_package`]): everything a plan says of it
/// but the irreversible mark.
struct TriedPackage {
    package_id: String,
    content_hash_prefix: String,
    ends: Vec<TrialDeclaredEnd>,
    variables: Vec<PackageVariable>,
}

/// Workflow #463 S3-10 (module doc): the plan of the trial `request` asks for, or its refusal, in
/// this order: the authorization's bounds, the package path, the state directory's place, the
/// package's reading and admission, its variables, `--expect`, then `--root`'s own (a root that
/// is no directory or names no game, a root file that cannot be read or is too large, a root file
/// missing or refused). Fix round 2: nothing is refused for an authorization not given or for the
/// irreversible mark; the plan shows both. It writes nothing.
pub fn plan_standard_pack_trial(
    request: &StandardPackTrialRequest,
) -> LabResult<StandardPackTrialAnswer> {
    let authorization = authorization(request.authorization.as_deref())?;
    let package = absolute(&request.package)?;
    match fs::metadata(&package) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(argument_refused(
                PACKAGE_FLAG,
                UNUSABLE,
                format!(
                    "--package {} is a file, not a package directory: a trial runs packs/<id> of a \
                     checkout or a placed pack, the form adoption writes, because what is tried \
                     must be what was adopted; adoption rewrites a package's JSON, so a .zip or \
                     .json container may hash otherwise than the directory adopted from it",
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
    let tried = read_tried_package(&package)?;
    let variables = trial_variables(&tried.variables, &request.variables)?;
    if let Some(expected) = &request.expect
        && !tried.ends.iter().any(|end| end.outcome_key == *expected)
    {
        let declared = tried
            .ends
            .iter()
            .map(|end| end.outcome_key.as_str())
            .collect::<Vec<_>>();
        let mut error = argument_refused(
            EXPECT_FLAG,
            NOT_RECOGNISED,
            format!(
                "--expect {expected:?} is no outcome key the package {} declares (it declares \
                 {declared:?})",
                tried.package_id
            ),
        );
        if let Some(details) = error.details.as_mut() {
            details["expected"] = json!(expected);
            details["declared_outcome_keys"] = json!(declared);
        }
        return Err(error);
    }
    let (irreversible, irreversible_basis) = match &request.root {
        None => (TrialIrreversible::NotJudged, None),
        Some(root) => {
            let (mark, basis) = judge_irreversible(root, &tried.package_id, &package)?;
            (mark, Some(basis))
        }
    };
    let run_requires = if authorization.is_none() {
        vec![TrialRunRequirement::Authorization]
    } else {
        Vec::new()
    };
    let record_directory = state
        .join(TRIALS_DIRECTORY)
        .join(&tried.package_id)
        .display()
        .to_string();
    Ok(StandardPackTrialAnswer {
        sent: false,
        plan: TrialPlan {
            package_id: tried.package_id,
            content_hash_prefix: tried.content_hash_prefix,
            package: package.display().to_string(),
            instance: request.instance.clone(),
            authorization,
            ends: tried.ends,
            variables,
            irreversible,
            irreversible_basis,
            irreversible_acknowledged: request.irreversible,
            expect: request.expect.clone(),
        },
        run_requires,
        record_directory,
    })
}

/// Workflow #463 S3-11 (`contracts/standard-pack-trial.md`, "The run"): the refusals only the run
/// needs, after the plan `answer` that [`plan_standard_pack_trial`] gave `request` (its refusals come
/// first), in this order, each before anything is sent and before anything is written:
/// - no authorization: `trial_authorization_missing`;
/// - a package the irreversible mark flags (`marked`) and no `--irreversible`:
///   `trial_irreversible_unacknowledged`; a mark not judged (no `--root`) refuses nothing, as the
///   plan says;
/// - any `--variable`: `trial_variables_not_carried`, since a direct run carries no variable value
///   yet (the plan keeps showing them).
///
/// Then the plan as the record keeps it ([`TrialRecordPlan`]). It reads and writes nothing, and it
/// judges no authorization: the reference is recorded as given.
pub fn check_trial_run(
    answer: &StandardPackTrialAnswer,
    request: &StandardPackTrialRequest,
) -> LabResult<TrialRecordPlan> {
    let plan = &answer.plan;
    if plan.authorization.is_none() {
        return Err(refused(
            Outcome::new(ResourceToolingCode::TrialAuthorizationMissing)
                .package_id(&plan.package_id),
            format!(
                "a trial of {} on a real application is sent only under the owner's \
                 authorization: give --authorization <reference> (the reference of the owner's \
                 word); a plan (--plan) needs none",
                plan.package_id
            ),
        ));
    }
    if plan.irreversible == TrialIrreversible::Marked && !plan.irreversible_acknowledged {
        return Err(refused(
            Outcome::new(ResourceToolingCode::TrialIrreversibleUnacknowledged)
                .package_id(&plan.package_id),
            format!(
                "the irreversible mark flags {} over the root given (see the plan's \
                 irreversible_basis): its trial is sent only with --irreversible",
                plan.package_id
            ),
        ));
    }
    if let Some((name, _)) = request.variables.first() {
        return Err(refused(
            Outcome::new(ResourceToolingCode::TrialVariablesNotCarried)
                .package_id(&plan.package_id)
                .variable_name(name),
            format!(
                "--variable {name} is given, but a direct run carries no variable value yet: a \
                 trial runs every declared default (the plan shows them); give no --variable"
            ),
        ));
    }
    TrialRecordPlan::try_from(plan.clone())
}

impl PackTrialRecord {
    /// Workflow #463 S3-11: the record of the trial run sent as `request_id` (its run `run_id`)
    /// under `plan`, whose end `end` was read, the run's terminal recorded at `at_unix_ms`;
    /// `matched` is whether the end's outcome key is the plan's `expect` (`false` for a run that
    /// failed, was cancelled or ended with another key or with none), absent without one.
    pub fn of_run(
        plan: TrialRecordPlan,
        at_unix_ms: u64,
        request_id: String,
        run_id: Option<String>,
        end: TrialRunEnd,
    ) -> Self {
        let matched = plan
            .expect
            .as_ref()
            .map(|expected| end.outcome_key.as_ref() == Some(expected));
        Self {
            schema_version: PACK_TRIAL_SCHEMA.to_owned(),
            at_unix_ms,
            plan,
            request_id,
            run_id,
            end,
            matched,
        }
    }

    /// Workflow #463 S3-11: the warning `trial_end_unexpected` of a run that ended normally on
    /// another end than the one expected (the business ended another way, which is what a trial is
    /// for: the command succeeds); `None` for a run that ended as expected, without `--expect`, or
    /// that did not end normally (a failed or cancelled run is answered with its own code).
    pub fn unexpected_end(&self) -> Option<Outcome> {
        if self.matched != Some(false) || self.end.outcome != TaskOutcome::Success {
            return None;
        }
        let warning =
            Outcome::new(ResourceToolingCode::TrialEndUnexpected).package_id(&self.plan.package_id);
        Some(match &self.end.outcome_key {
            Some(key) => warning.outcome_key(key),
            None => warning,
        })
    }
}

/// The reference `given`, `None` when none is given; given, it is not blank (`validation_failed`,
/// `value_missing`, as the command line answers an option without its value), at most
/// [`MAX_AUTHORIZATION_BYTES`] bytes (`out_of_range`) and without a control character
/// (`not_recognised`).
fn authorization(given: Option<&str>) -> LabResult<Option<String>> {
    let Some(reference) = given else {
        return Ok(None);
    };
    if reference.trim().is_empty() {
        return Err(argument_refused(
            AUTHORIZATION_FLAG,
            VALUE_MISSING,
            "--authorization is given without a reference".to_owned(),
        ));
    }
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
    Ok(Some(reference.to_owned()))
}

/// Fix round 2: the package directory `package`, read once, and everything a plan says of it from
/// those bytes but the irreversible mark ([`judge_irreversible`]). The snapshot is the containment
/// crate's bounded read; its hash is what `measure_content_hash` computes for a directory; the
/// kernel admits the same bytes (`PreparedContainedTask::load_content_entries`, offline without a
/// vision provider); the control declaration's `provides` (which the variables are parsed with)
/// and `variables` and the entry task are read from the snapshot after that admission. A refusal
/// of any of these is `package_invalid`.
fn read_tried_package(package: &Path) -> LabResult<TriedPackage> {
    let deadline = Instant::now() + PACK_DEADLINE;
    let invalid = |error: &dyn std::fmt::Display| {
        LabError::package_invalid(format!("{}: {error}", package.display()))
    };
    let entries = Containment::new()
        .snapshot_content_directory(package, deadline)
        .map_err(|error| invalid(&error))?;
    let content_hash = content_directory_digest(
        entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), *Sha256Hash::digest(bytes).as_bytes())),
    );
    let prepared =
        PreparedContainedTask::load_content_entries(TRIAL_LABEL, entries.clone(), None, deadline)
            .map_err(|error| invalid(&error))?;
    let package_id = prepared.package_label().to_owned();
    if !names_directory(&package_id) {
        return Err(LabError::package_invalid(format!(
            "the package_id {package_id:?} names no directory {TRIALS_DIRECTORY}/<package_id>"
        )));
    }
    let control = json_entry(package, &entries, CONTROL_FILE)?;
    let provides = match control.get("provides") {
        None => Vec::new(),
        Some(provides) => serde_json::from_value::<Vec<ProvidedCapability>>(provides.clone())
            .map_err(|error| invalid(&format!("{CONTROL_FILE}: provides: {error}")))?,
    };
    let variables = match control.get("variables") {
        None => Vec::new(),
        Some(variables) => parse_package_variables(variables, &provides).map_err(|violation| {
            invalid(&format!(
                "{CONTROL_FILE}: {} at {}",
                violation.problem.as_str(),
                violation.pointer
            ))
        })?,
    };
    let task = json_entry(
        package,
        &entries,
        &format!("resources/operations/{}/task.json", prepared.task_label()),
    )?;
    Ok(TriedPackage {
        package_id,
        content_hash_prefix: content_hash.chars().take(CONTENT_PREFIX_DIGITS).collect(),
        ends: declared_ends(&prepared, &task),
        variables,
    })
}

/// The JSON of the file `relative` of the snapshot `entries` of the package `package`, a leading
/// byte-order mark taken off; a file that is missing or no JSON is `package_invalid`.
fn json_entry(
    package: &Path,
    entries: &BTreeMap<String, Vec<u8>>,
    relative: &str,
) -> LabResult<Value> {
    let bytes = entries.get(relative).ok_or_else(|| {
        LabError::package_invalid(format!("{} holds no {relative}", package.display()))
    })?;
    let bytes = bytes
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(bytes.as_slice());
    serde_json::from_slice(bytes).map_err(|error| {
        LabError::package_invalid(format!("{}: {relative}: {error}", package.display()))
    })
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

/// The ends the entry task `task` of the admitted package `prepared` declares, in mapping order:
/// each mapping of its `scheduling_outcome` with its terminal pages as the task writes them, named
/// by the declaration that makes it an end, told apart in [`TrialEndKind`]'s order: a target page
/// of the task (the kernel's own target set), then the keyed end of an operation whose key it is,
/// then a stage's end page, then the rest (`ending` in a `linear_steps` task, `other` in a page
/// graph).
fn declared_ends(prepared: &PreparedContainedTask, task: &Value) -> Vec<TrialDeclaredEnd> {
    let game = prepared.game();
    let keyed = keyed_ends(task);
    let end_pages = task
        .get("phases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|phase| phase.get("end_pages").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(|page| canonical_page_anchor(game, page))
        .collect::<BTreeSet<_>>();
    let linear = prepared.execution_mode() == LINEAR_STEPS;
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
            let anchors = pages
                .iter()
                .map(|page| canonical_page_anchor(game, page))
                .collect::<Vec<_>>();
            let end = if anchors
                .iter()
                .any(|anchor| prepared.terminal_holds_page(anchor))
            {
                TrialEndKind::Target
            } else if let Some(kind) = keyed.get(outcome_key) {
                *kind
            } else if anchors.iter().any(|anchor| end_pages.contains(anchor)) {
                TrialEndKind::EndPage
            } else if linear {
                TrialEndKind::Ending
            } else {
                TrialEndKind::Other
            };
            Some(TrialDeclaredEnd {
                outcome_key: outcome_key.to_owned(),
                end,
                pages,
            })
        })
        .collect()
}

/// The keys the operations of the entry task `task` end a run on, each with the member that
/// declares it, in operation order and, within one operation, as the package contract
/// (`contracts/resource-declarations.md`) lists the keyed ends: a select step's `on_empty`, a
/// keyed `on_max_runs`, a keyed `on_guard_refused`, a wait step's `on_timeout`. The first
/// declaration of a key names it.
fn keyed_ends(task: &Value) -> BTreeMap<&str, TrialEndKind> {
    let mut keyed = BTreeMap::new();
    for operation in task
        .get("operations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let declared = [
            (
                operation
                    .get("select")
                    .and_then(|select| select.get("on_empty"))
                    .and_then(|end| end.get("outcome_key")),
                TrialEndKind::OnEmpty,
            ),
            (
                operation
                    .get("on_max_runs")
                    .and_then(|end| end.get("outcome_key")),
                TrialEndKind::OnMaxRuns,
            ),
            (
                operation
                    .get("on_guard_refused")
                    .and_then(|end| end.get("outcome_key")),
                TrialEndKind::OnGuardRefused,
            ),
            (
                operation
                    .get("wait")
                    .and_then(|wait| wait.get("on_timeout")),
                TrialEndKind::OnTimeout,
            ),
        ];
        for (key, kind) in declared {
            if let Some(key) = key.and_then(Value::as_str) {
                keyed.entry(key).or_insert(kind);
            }
        }
    }
    keyed
}

/// The irreversible mark of the package `package_id`, the directory `package`, over the game root
/// `root`: its root files read as every reader reads them (a root that is no directory or names no
/// game, and a root file that cannot be read or is too large, are `validation_failed`; a refused
/// or missing root file is `standard_pack_root_invalid`), then the resource tooling's
/// `root_irreversibility` over it (fix round 3: called, not copied), the contract crate's
/// `IrreversiblePackages` over every package its `bundle.json` registers, each read by the
/// Runtime's metadata reader at its registered directory, the registered directory of
/// `package_id` read at `package` in its place. That reader takes only a path: `package` is read
/// here once more, beside the plan's one snapshot of it.
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
    let tried = checked
        .registry
        .get(package_id)
        .map(|entry| entry.directory.clone());
    let packages = root_irreversibility(&checked, &|directory: &str| {
        if tried.as_deref() == Some(directory) {
            package.to_path_buf()
        } else {
            root.join(directory)
        }
    })
    .packages;
    let mark = if packages.counts_irreversible(package_id) {
        TrialIrreversible::Marked
    } else {
        TrialIrreversible::Unmarked
    };
    let basis = TrialIrreversibleBasis {
        root: root.display().to_string(),
        registered: tried.is_some(),
        capabilities: packages
            .irreversible_capabilities(package_id)
            .map(|capabilities| capabilities.iter().cloned().collect())
            .unwrap_or_default(),
        unreadable_packages: packages.unreadable().iter().cloned().collect(),
    };
    Ok((mark, basis))
}

/// Workflow #463 S3-10 fix round 2 (the review's item 2): the installed root `path` is or lies
/// inside: the nearest directory along it whose `install\state.json` is a file, the installer's own
/// state file in both layouts (`contracts/install-cli.md`, "Layout"); `None` outside every
/// installed root. It looks along `path` made absolute and then along its nearest existing part
/// with its links resolved. A check that cannot be made is never read as "absent": a part whose
/// existence or kind the operating system does not tell, or that cannot be resolved, is
/// `foreign_os_error` with that `path` and its `io_op`. It reads only whether those files exist,
/// and writes nothing. Workflow #463 S3-11 (the review of S3-10's third round): it is the
/// resource tooling's test of an installed root along a path, which the trial's plan and writer
/// call; it is not the only one. The Lab's recordings keep their own walk (`installed_root` of
/// `crates/lab/src/recording/container.rs`, #464 P4) by the same file along the same two paths,
/// with two different answers (`contracts/standard-pack-trial.md`, "The state directory's
/// place"): a check the operating system refuses is read there as "absent" and the walk goes on,
/// where this function answers `foreign_os_error`; and a path that cannot be made absolute is
/// walked there as given, where this function refuses it. The Runtime host's
/// `installation_root_at` asks the same file of one directory.
pub fn installed_root_containing(path: &Path) -> LabResult<Option<PathBuf>> {
    let given = absolute(path)?;
    let resolved = resolved_existing_part(&given)?;
    for ancestor in given
        .ancestors()
        .chain(resolved.iter().flat_map(|existing| existing.ancestors()))
    {
        let marker = ancestor.join(INSTALL_FOLDER).join(INSTALLER_STATE_FILE);
        match fs::metadata(&marker) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(ancestor.to_path_buf())),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(check_failed(&marker, &error, IoOp::Metadata)),
        }
    }
    Ok(None)
}

/// The nearest part of the absolute path `path` that exists, with its links resolved; `None` when
/// no part exists. Whether a part exists is asked with `try_exists`: a part the operating system
/// does not tell, and one that cannot be resolved, are `foreign_os_error`.
fn resolved_existing_part(path: &Path) -> LabResult<Option<PathBuf>> {
    for ancestor in path.ancestors() {
        match ancestor.try_exists() {
            Ok(true) => {
                return fs::canonicalize(ancestor)
                    .map(Some)
                    .map_err(|error| check_failed(ancestor, &error, IoOp::Canonicalize));
            }
            Ok(false) => {}
            Err(error) => return Err(check_failed(ancestor, &error, IoOp::Exists)),
        }
    }
    Ok(None)
}

/// The Git work tree the absolute path `state` is or lies inside, along `state` and then along
/// `resolved` (its nearest existing part with its links resolved): the nearest directory that
/// holds `.git`. An entry whose existence the operating system does not tell is
/// `foreign_os_error`.
fn repository_containing(state: &Path, resolved: Option<&Path>) -> LabResult<Option<PathBuf>> {
    for ancestor in state
        .ancestors()
        .chain(resolved.into_iter().flat_map(Path::ancestors))
    {
        let entry = ancestor.join(GIT_ENTRY);
        match entry.try_exists() {
            Ok(true) => return Ok(Some(ancestor.to_path_buf())),
            Ok(false) => {}
            Err(error) => return Err(check_failed(&entry, &error, IoOp::Exists)),
        }
    }
    Ok(None)
}

/// Workflow #463 S3-10: the state directory `state_dir` (made absolute, then also with its
/// nearest existing part's links resolved) is no installed root's or repository's, nor inside
/// one; otherwise `trial_state_dir_refused` with the word of the place: an installed root by
/// [`installed_root_containing`], then a repository by the `.git` it holds. A check that cannot
/// be made is `foreign_os_error`, never passed. It reads whether those entries exist along the
/// path, and nothing else.
pub fn check_trial_state_dir(state_dir: &Path) -> LabResult<()> {
    let state = absolute(state_dir)?;
    let (place, holder) = match installed_root_containing(&state)? {
        Some(root) => (TrialStatePlace::InstalledRoot, root),
        None => {
            let resolved = resolved_existing_part(&state)?;
            match repository_containing(&state, resolved.as_deref())? {
                Some(repository) => (TrialStatePlace::Repository, repository),
                None => return Ok(()),
            }
        }
    };
    Err(refused(
        Outcome::new(ResourceToolingCode::TrialStateDirRefused)
            .path(&state)
            .trial_state_place(place),
        format!(
            "the state directory {} lies inside {} ({}); trial records are Lab authoring state, \
             kept beside the recordings and never in a repository or an installed root: give \
             --state-dir outside it",
            state.display(),
            holder.display(),
            place.as_str()
        ),
    ))
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
/// directory `state_dir`, which [`check_trial_state_dir`] must accept. Fix round 2 (the review's
/// item 3): the record is written whole, and flushed, under its name with `.part` added, a file
/// created anew, and then hard-linked to its name, which adds that name in one operation and fails
/// when the name is taken, so a record is never replaced. A record of that name, or a `.part` of
/// it, that is there already is `trial_record_exists` naming that file (the `.part` this call
/// wrote stays beside a record the link or the exclusive create found there).
///
/// Fix round 3: a link that fails for another reason than the name being taken (a file system
/// without hard links: FAT32, exFAT, some network shares) falls back to an exclusive create of the
/// record's own name (`place_exclusive`). A step the operating system refuses is
/// `trial_record_write_failed` with its refusal as the cause, and no record is written: the files
/// this call created anew (its `.part`, and the record's file of an exclusive create) are removed,
/// so that writing the record again is not blocked by them, and one whose removal is refused too
/// is named in the failure (`failed_removing`). Once the record is in place its `.part` is
/// removed; a removal the operating system refuses then leaves the record written: the answer is
/// its path, with the `.part` as [`WrittenTrialRecord::part_left`] and the warning
/// `trial_record_part_left`. A record holds an authorization by its type ([`TrialRecordPlan`]).
///
/// Workflow #463 S3-11: the run's one call of the writer, once its end was read; the steps are
/// the operating system's (`WriteSteps`).
pub fn write_trial_record(
    state_dir: &Path,
    record: &PackTrialRecord,
) -> LabResult<WrittenTrialRecord> {
    write_trial_record_with(
        state_dir,
        record,
        &WriteSteps {
            link: &hard_link,
            write: &write_whole,
        },
    )
}

/// Workflow #463 S3-11: the writer's link step: the hard link of a `.part` (the first path) to its
/// record's name (the second), which fails when the name is taken.
pub(crate) type LinkStep<'a> = dyn Fn(&Path, &Path) -> io::Result<()> + 'a;
/// Workflow #463 S3-11: the writer's write step: writes bytes whole to a file created anew,
/// flushes them and closes it; the refusal of the step that failed with its `io_op`.
pub(crate) type WriteStep<'a> = dyn Fn(fs::File, &[u8]) -> Result<(), (io::Error, IoOp)> + 'a;

/// Workflow #463 S3-11 (the review of S3-10's third round, item 1): the writer's two steps whose
/// refusals a test cannot ask of the operating system, taken as parameters inside the module, as
/// the Runtime host's `place_once` takes its link (`state_root_installation.rs`): in production
/// [`fs::hard_link`] and [`write_whole`].
pub(crate) struct WriteSteps<'a> {
    pub(crate) link: &'a LinkStep<'a>,
    pub(crate) write: &'a WriteStep<'a>,
}

/// The production link step: [`fs::hard_link`].
fn hard_link(part: &Path, path: &Path) -> io::Result<()> {
    fs::hard_link(part, path)
}

/// [`write_trial_record`] with the steps `steps`.
pub(crate) fn write_trial_record_with(
    state_dir: &Path,
    record: &PackTrialRecord,
    steps: &WriteSteps<'_>,
) -> LabResult<WrittenTrialRecord> {
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
        .map_err(|error| write_failed(&path, &directory, &error, IoOp::CreateDir))?;
    match fs::symlink_metadata(&path) {
        Ok(_) => return Err(record_exists(&path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(write_failed(&path, &path, &error, IoOp::Metadata)),
    }
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|error| LabError::usage(format!("the trial record does not encode: {error}")))?;
    let part = path.with_extension("json.part");
    let file = match OpenOptions::new().write(true).create_new(true).open(&part) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(record_exists(&part));
        }
        Err(error) => return Err(write_failed(&path, &part, &error, IoOp::Create)),
    };
    if let Err((error, op)) = (steps.write)(file, &bytes) {
        return Err(failed_removing(
            write_failure(&path, &part, &error, op),
            &[part.as_path()],
        ));
    }
    match (steps.link)(&part, &path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(record_exists_beside(&path, &part));
        }
        Err(unlinked) => place_exclusive(&path, &part, &bytes, &unlinked, steps.write)?,
    }
    match fs::remove_file(&part) {
        Ok(()) => Ok(WrittenTrialRecord {
            path,
            part_left: None,
            warnings: Vec::new(),
        }),
        Err(error) => {
            let warning = Outcome::new(ResourceToolingCode::TrialRecordPartLeft)
                .path(&part)
                .caused_by(Outcome::from_io_error(&error, IoOp::Remove).path(&part));
            Ok(WrittenTrialRecord {
                path,
                part_left: Some(part),
                warnings: vec![warning],
            })
        }
    }
}

/// Fix round 3: writes `bytes` whole to `file`, a file created anew, flushes them to disk and
/// closes it; the refusal of the step that failed, `write` or (the flush) `sync`.
fn write_whole(mut file: fs::File, bytes: &[u8]) -> Result<(), (io::Error, IoOp)> {
    file.write_all(bytes)
        .map_err(|error| (error, IoOp::Write))?;
    file.sync_all().map_err(|error| (error, IoOp::Sync))
}

/// Fix round 3: the record `bytes` placed at `path` where the hard link of its `.part` `part` was
/// refused (`unlinked`) for another reason than the name being taken, as the Runtime host places a
/// file once where no hard link can be made (`state_root_installation.rs`, `create_exclusive`;
/// `contracts/install-cli.md`, `install\identity.json`): an exclusive create of `path` itself,
/// which fails when the name is taken (`trial_record_exists`, the `.part` left beside it), then
/// the bytes written whole and flushed. What it keeps of "never replaced": no file of that name is
/// ever written over. What it gives up against the link: the name is there before the bytes are,
/// so a reader at that moment, or a crash while they are written, finds an incomplete record,
/// which the reader refuses (`undecodable`). A step the operating system refuses is
/// `trial_record_write_failed` with the link's refusal as a `note` link; after a refused create
/// the `.part` is removed, after a refused write or flush the file this call created and the
/// `.part` ([`failed_removing`]). Workflow #463 S3-11: the bytes are written by the writer's
/// write step `write`.
fn place_exclusive(
    path: &Path,
    part: &Path,
    bytes: &[u8],
    unlinked: &io::Error,
    write: &WriteStep<'_>,
) -> LabResult<()> {
    let file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(record_exists_beside(path, part));
        }
        Err(error) => {
            return Err(unlinked_failed(
                path,
                part,
                unlinked,
                &error,
                IoOp::Create,
                &[part],
            ));
        }
    };
    write(file, bytes)
        .map_err(|(error, op)| unlinked_failed(path, part, unlinked, &error, op, &[path, part]))
}

/// Fix round 3: the step `op` of [`place_exclusive`] at the record `path` that the operating
/// system refused (`error`), the hard link of `part` it stood in for refused before (`unlinked`,
/// a `note` link), after the files `created` are removed ([`failed_removing`]).
fn unlinked_failed(
    path: &Path,
    part: &Path,
    unlinked: &io::Error,
    error: &io::Error,
    op: IoOp,
    created: &[&Path],
) -> LabError {
    let (outcome, message) = write_failure(path, path, error, op);
    let note = Outcome::from_io_error(unlinked, IoOp::Create).path(path);
    failed_removing(
        (
            outcome.link(CauseRelation::Note, note),
            format!(
                "{message} (it stood in for the hard link of {}, refused: {unlinked})",
                part.display()
            ),
        ),
        created,
    )
}

/// Fix round 3: the failed step of a write (its outcome and its message) after the files
/// `created`, which this call created anew, are removed, so that writing the record again is not
/// blocked by this call's own files. One the operating system refuses to remove is left and
/// named, by a `cleanup` link (`foreign_os_error`, `io_op` `remove`, `path` the file left) and in
/// the message; the answer is still the failed step.
fn failed_removing((mut outcome, mut message): (Outcome, String), created: &[&Path]) -> LabError {
    for file in created {
        if let Err(error) = fs::remove_file(file) {
            outcome = outcome.link(
                CauseRelation::Cleanup,
                Outcome::from_io_error(&error, IoOp::Remove).path(file),
            );
            message = format!(
                "{message}; {} that this call created is left, its removal refused: {error}",
                file.display()
            );
        }
    }
    refused(outcome, message)
}

/// Workflow #463 S3-10: every trial record of the package `package_id` under the state directory
/// `state_dir`, in the order the trials ended (then by content); none without a record. A file of
/// a record's name (`<content12>-<ms>.json`) that does not read as the record of that package,
/// content and time is `trial_record_unreadable` with its word, never passed over; another name
/// (a `.part` a write left) is no record.
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
                TrialRecordProblem::NameMismatch,
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

/// Workflow #463 S3-11 fix round 2 (one run, one record): the file of the record of the run
/// `request_id` among the trial records of the package `package_id` under `state_dir`
/// ([`trial_records`], whose refusal of a file it answers), whatever content that record names;
/// `None` when the run has none. The run reads it before it calls the writer, so that a run whose
/// package directory changed since it ran is not recorded a second time under another content.
pub fn trial_record_of_run(
    state_dir: &Path,
    package_id: &str,
    request_id: &str,
) -> LabResult<Option<PathBuf>> {
    let state = absolute(state_dir)?;
    Ok(trial_records(&state, package_id)?
        .into_iter()
        .find(|record| record.request_id == request_id)
        .map(|record| {
            trial_record_path(
                &state,
                package_id,
                &record.plan.content_hash_prefix,
                record.at_unix_ms,
            )
        }))
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
            TrialRecordProblem::TooLarge,
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
            TrialRecordProblem::Undecodable,
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

/// `foreign_os_error` for a check of whether a path lies inside an installed root or a repository
/// that the operating system refused at `path` (`op`): the answer is unknown, so the path is not
/// taken.
fn check_failed(path: &Path, error: &io::Error, op: IoOp) -> LabError {
    refused(
        Outcome::from_io_error(error, op).path(path),
        format!(
            "{} cannot be inspected ({error}), so whether the path lies inside an installed root \
             or a repository is unknown",
            path.display()
        ),
    )
}

/// `trial_record_exists`: the file `path`, a record or the `.part` of one, is there and is never
/// replaced.
fn record_exists(path: &Path) -> LabError {
    refused(
        Outcome::new(ResourceToolingCode::TrialRecordExists).path(path),
        format!(
            "{} is there already and a trial record is never replaced: look at that file (a \
             record of the same content and millisecond, or the .part an interrupted write left)",
            path.display()
        ),
    )
}

/// [`record_exists`] of the record `path` a link or an exclusive create found there; the `.part`
/// `part` this call wrote, holding the record it was given, is left beside it.
fn record_exists_beside(path: &Path, part: &Path) -> LabError {
    let mut exists = record_exists(path);
    exists.message = format!(
        "{}; the {} this call wrote, holding the record it was given, is left beside it",
        exists.message,
        part.display()
    );
    exists
}

/// `trial_record_write_failed` of the record `record` at `path`, the operating system's refusal
/// `error` of `op` there its cause, and its message.
fn write_failure(record: &Path, path: &Path, error: &io::Error, op: IoOp) -> (Outcome, String) {
    let outcome = Outcome::new(ResourceToolingCode::TrialRecordWriteFailed)
        .path(path)
        .caused_by(Outcome::from_io_error(error, op).path(path));
    let message = format!(
        "the trial record {} could not be written: {} {} refused: {error}",
        record.display(),
        path.display(),
        op.as_str()
    );
    (outcome, message)
}

/// [`write_failure`] as its Lab error, for a step before this call created a file.
fn write_failed(record: &Path, path: &Path, error: &io::Error, op: IoOp) -> LabError {
    let (outcome, message) = write_failure(record, path, error, op);
    refused(outcome, message)
}

/// `trial_record_unreadable` at `path` with the word `problem`, the operating system's refusal
/// `error` of `op` its cause.
fn read_failed(path: &Path, error: &io::Error, op: IoOp) -> LabError {
    unreadable(
        path,
        TrialRecordProblem::ReadFailed,
        Some(Outcome::from_io_error(error, op).path(path)),
        format!("{} could not be read: {error}", path.display()),
    )
}

/// `trial_record_unreadable` at `path` with the word `problem`, and `cause` when there is one.
fn unreadable(
    path: &Path,
    problem: TrialRecordProblem,
    cause: Option<Outcome>,
    message: String,
) -> LabError {
    let outcome = Outcome::new(ResourceToolingCode::TrialRecordUnreadable)
        .path(path)
        .trial_record_problem(problem);
    let outcome = match cause {
        Some(cause) => outcome.caused_by(cause),
        None => outcome,
    };
    refused(outcome, message)
}
