// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #464 P1 (`contracts/standard-pack-authoring.md`; register §1.377):
//! `actinglab standard-pack adopt` makes a recorded package a member of its game's standard pack
//! in a resource repository's checkout, with a rule the composer accepts and the Runtime
//! dispatches; its answer never says more than is true, and a call that cannot finish fails
//! before it writes.
//!
//! - **Inputs.** `--repo`, the game's working root in the source layout (`applications.json`,
//!   `maintenance.json`, `packs/`, the root files); the package, a stopped recording's
//!   (`--record-id`, its `lab/out/<package_id>.<zip|json>`) or any content directory or container
//!   (`--package`); the decision document `actingcommand.pack-decision.v1` ([`Decision`]), which
//!   holds only what the Lab cannot know and is required on a first adoption; the Lab's state
//!   directory, which holds the recordings, the kept copies and the check trees.
//! - **The checks**, every one before the first write, in this order: the repository, no leftover
//!   of an interrupted call; the recording; the package, admitted as `package preflight` admits
//!   it, its game and server those of `applications.json`; its `package_id` a directory name and
//!   not adopted before without `--replace`, with `--replace` no change below its directory the
//!   version control does not hold and the kept copy's place on the checkout's volume
//!   ([`AdoptWorktree`]); the decision, and one role for the package; the review (a class other
//!   than `held` needs a package run through); no absolute local path in the package's text; the
//!   root files it must change in their canonical form; every outcome predicate on the rule itself
//!   on a key the package declares; then the tree as it would be written, laid out in the
//!   checkout's own marked check tree `<state>/adopt-check/<checkout>/`, bundled by `package
//!   bundle` with every root file the repository holds, its installed layout checked by
//!   `standard-pack check`, both composing the schedule with one instance per server; last, a
//!   business rule's value (none only for a follower of a chain, never `pending` for one). Each
//!   refusal is `adopt_refused` naming the check, its member or its pointer, and writes nothing in
//!   the repository.
//! - **What is derived** ([`derive`]): every rule member the decision does not give, each answered
//!   with its value and where it came from (`package`, `file`, `neutral`, `neutral_floor`), so that
//!   nothing is chosen silently; `feedback_stop` only from the package's own declared keys, never
//!   from another rule; `expected_duration_ms` never below the neutral 60000.
//! - **One role.** The decision's class decides: the package ends with exactly one role in the
//!   files adopt writes (a business rule, a maintenance rule or entry, or a held entry), what it
//!   removed or replaced answered in `removed_entries` and `replaced_entries`.
//! - **What it writes**, only inside `--repo` and under the state directory, each part written whole
//!   and renamed, nothing deleted in the checkout or of a replaced directory: `packs/<package_id>/`
//!   (written as `packs/<package_id>.part/`, only `control.json` and `resources/**`, its JSON in the
//!   canonical form), the replaced directory first moved to `<state>/adopt/<package_id>.old-<ms>/`
//!   and moved back when the new one cannot take its place; `schedule.json` and `maintenance.json`
//!   written as `<name>.part`, read again and renamed only when it is still the file the call read,
//!   each only when it changes. A failure is `adopt_write_failed` naming where it stopped and what
//!   it had written. The check tree alone is the tool's own scratch, emptied at the start of each
//!   call when its marker is there.
//! - **The value's origin** (coordinator ruling on #464): adopt never writes
//!   `fixed_value_origin: "model"`. A decided value is `"pending"` or a number of the pack's own
//!   (`origin` absent or `pack`), written as `fixed_value_milli` without an origin; a decision that
//!   says `model` is refused `value_origin_model` before anything is written.
//! - **Repeating** (`--replace`): without a decision the rule, the held entry and the maintenance
//!   entry are kept byte for byte; a decision changes only the members it names; a fixed value
//!   whose origin is `model` is kept as it is whatever the decision names, and answered in
//!   `model_kept`; a derived member that would now differ is answered in `derived_drift` and never
//!   written.
//! - **`--dry-run`** runs every check, the check tree included, and writes nothing in the
//!   repository and keeps no copy.

use crate::codes::{AdoptProblem, ResourceToolingCode};
use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::root_issue_outcome;
use crate::{
    PackageBundleRequest, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackRootFiles, check_standard_pack_root, package_bundle,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, OutcomeEnvelope};
use actingcommand_contract::{
    BundleMaintenance, LabError, LabErrorClass, LabResult, MaintenanceUse,
    SCHEDULE_RULE_FIXED_VALUE_MEMBER, SCHEDULE_RULE_FIXED_VALUE_ORIGIN_MEMBER,
    SCHEDULE_RULE_MAINTENANCE_MEMBER, SCHEDULE_RULE_REALIZATION_MEMBER,
    SCHEDULE_VALUE_ORIGIN_MODEL, STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_GOALS_FILE,
    STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_LOADING_FILE, STANDARD_PACK_NAVIGATION_FILE,
    STANDARD_PACK_NOTICES_DIRECTORY, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    STANDARD_PACK_SCHEDULE_VERSION, ScheduleItem, ScheduleRule, StandardPackRegistry,
    StandardPackRoot, StandardPackSchedule, canonical_json_bytes,
    package_id_names_one_path_segment, parse_standard_pack_interrupts,
    parse_standard_pack_navigation, parse_standard_pack_pages, parse_standard_pack_schedule,
    reserved_package_id, standard_pack_maintenance_packages,
};
use actingcommand_execution_kernel::{PreparedContainedTask, TaskPackageDescriptor};
use actingcommand_pack_containment::Containment;
use actingcommand_policy::{
    BENEFIT_FIXED_VALUE_SUBSYSTEM, BenefitGap, CONSUMPTION_FACT_SUBSYSTEM,
    FACT_REFERENCE_SUBSYSTEM, FACT_REFERENCE_TYPE, FACT_UNKNOWN_PROBE_SUBSYSTEM,
    MAX_QUEUE_WAIT_MEMBER, POOL_FACT_REFERENCE_SUBSYSTEM, PolicyCode, QUEUE_WAIT_SUBSYSTEM,
    SCHEDULE_RULE_LIMITED_MEMBER,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The one version of the decision document.
pub const PACK_DECISION_SCHEMA: &str = "actingcommand.pack-decision.v1";
/// The label the package is admitted under: admission depends only on the package.
const ADOPT_LABEL: &str = "standard-pack-adopt";
const APPLICATIONS_FILE: &str = "applications.json";
const APPLICATIONS_SCHEMA: &str = "actingcommand.applications.v1";
const MAINTENANCE_FILE: &str = "maintenance.json";
const PACKS_DIRECTORY: &str = "packs";
/// Below the state directory: the kept copies of replaced pack directories, and the check trees.
const KEPT_DIRECTORY: &str = "adopt";
const CHECK_DIRECTORY: &str = "adopt-check";
/// The file adopt writes into the check tree of a checkout, `<state>/adopt-check/<checkout>/`, to
/// mark it as its own scratch: adopt empties a check tree only when this file is in it, and holds
/// it locked for the whole call, so that two calls on one checkout never share a tree at once.
pub(crate) const CHECK_TREE_MARKER: &str = ".adopt-check-tree";
/// Of a check tree's name: the characters of the checkout's last segment kept, then the digest.
const CHECK_TREE_STEM_CHARS: usize = 32;
const CHECK_TREE_DIGEST_CHARS: usize = 16;
/// A recording's file and the directory of the package its last stop generated.
const RECORDING_FILE: &str = "recording.json";
const RECORDING_SCHEMA: &str = "actingcommand.lab-recording.v1";
const RECORDING_OUT: &str = "out";
const RECORDING_STOPPED: &str = "stopped";
/// The provenance a recorded package's entry task carries (`contracts/lab-recording.md`).
const RECORDED_SOURCE: &str = "lab_record";
const CROSS_CHECK_PASSED: &str = "passed";
const FIRST_DECISION_NOT_EVALUATED: &str = "first_decision_not_evaluated";
const PART: &str = "part";
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";
/// The members of the decision document.
const DECISION_MEMBERS: [&str; 11] = [
    "schema_version",
    "class",
    "trigger",
    "value",
    "consumes",
    "produces",
    "limited",
    "realization",
    "uses",
    "held",
    "overrides",
];
/// The members of a decided `value` object.
const VALUE_MEMBERS: [&str; 2] = ["fixed_value_milli", "origin"];
/// The members of a decided `consumes` or `produces` entry.
const EFFECT_MEMBERS: [&str; 5] = [
    "pool_id",
    "amount",
    "expected_amount_milli",
    "observation_source",
    "confidence_milli",
];
/// The members of a decided `held` object.
const HELD_MEMBERS: [&str; 2] = ["reason", "reference"];
/// The longest `held` reason or reference, as the schedule reader bounds them.
const MAX_HELD_TEXT_BYTES: usize = 1_024;
const VALUE_PENDING: &str = "pending";
const ORIGIN_PACK: &str = "pack";
/// The members of a rule the composer requires that adopt derives (B.3), in the order a rule is
/// written; `overrides` may name each, and `id` and `loop_budget` besides.
const DERIVED_MEMBERS: [&str; 10] = [
    PRIORITY,
    FEEDBACK_STOP,
    ON_FAILURE,
    SENSITIVE,
    NEXT_RUN_CLAMP_MS,
    YIELD_POINTS,
    EXPECTED_DURATION_MS,
    COOLDOWN_MS,
    LOAD_PROFILE,
    STRATEGIC_WEIGHT_MILLI,
];
const OVERRIDE_MEMBERS: [&str; 12] = [
    ID,
    PRIORITY,
    FEEDBACK_STOP,
    ON_FAILURE,
    SENSITIVE,
    NEXT_RUN_CLAMP_MS,
    YIELD_POINTS,
    EXPECTED_DURATION_MS,
    COOLDOWN_MS,
    LOAD_PROFILE,
    STRATEGIC_WEIGHT_MILLI,
    LOOP_BUDGET,
];
const ID: &str = "id";
const PACKAGE_ID: &str = "package_id";
const PRIORITY: &str = "priority";
const TRIGGER: &str = "trigger";
const FEEDBACK_STOP: &str = "feedback_stop";
const CONSUMES: &str = "consumes";
const PRODUCES: &str = "produces";
const ON_FAILURE: &str = "on_failure";
const SENSITIVE: &str = "sensitive";
const NEXT_RUN_CLAMP_MS: &str = "next_run_clamp_ms";
const YIELD_POINTS: &str = "yield_points";
const EXPECTED_DURATION_MS: &str = "expected_duration_ms";
const COOLDOWN_MS: &str = "cooldown_ms";
const LOAD_PROFILE: &str = "load_profile";
const STRATEGIC_WEIGHT_MILLI: &str = "strategic_weight_milli";
const LOOP_BUDGET: &str = "loop_budget";
const RULES: &str = "rules";
const HELD: &str = "held";
const REQUIRES: &str = "requires";
const SCHEMA_VERSION: &str = "schema_version";
const HELD_REASON: &str = "reason";
const HELD_REFERENCE: &str = "reference";
const USES: &str = "uses";
const SERVER: &str = "server";
/// The neutral constants of the contract ("Defaults"): what a member takes when neither the
/// package nor the file's rules of its tier give it.
const NEUTRAL_COOLDOWN_MS: u64 = 60_000;
const NEUTRAL_NEXT_RUN_CLAMP_MS: u64 = 3_600_000;
const NEUTRAL_STRATEGIC_WEIGHT_MILLI: u64 = 1_000;
const NEUTRAL_EXPECTED_DURATION_MS: u64 = 60_000;
const NEUTRAL_OBSERVATION_SOURCE: &str = "self_reported";
const NEUTRAL_CONFIDENCE_MILLI: u64 = 1_000;
/// The repository's own items adopt does not write, each named when the checkout holds it.
const NEXT_FILES: [&str; 2] = ["docs/PACKS.md", "ours/coverage/business-v1.csv"];
const REGRESSION_DIRECTORY: &str = "regression";

/// Where the package to adopt comes from.
#[derive(Debug, Clone)]
pub enum AdoptSource {
    /// A stopped recording: its `record_id` and its Lab directory,
    /// `<state>/record-artifacts/<record>/lab`, which holds `recording.json` and `out/`.
    Recording { record_id: String, lab_dir: PathBuf },
    /// A package by its path: a content directory, a `.zip` or a `.json` container.
    Package(PathBuf),
}

/// Workflow #464 P1: `actinglab standard-pack adopt --repo <checkout> (--record-id <id> |
/// --package <path>) [--decide <decision.json>] [--replace] [--state-dir <dir>]` under the
/// global `--dry-run`.
#[derive(Debug, Clone)]
pub struct StandardPackAdoptRequest {
    pub repo: PathBuf,
    pub source: AdoptSource,
    pub decision: Option<PathBuf>,
    pub replace: bool,
    pub dry_run: bool,
    /// The Lab's state directory: the recordings, the kept copies and the check trees.
    pub state_dir: PathBuf,
    /// The operation's time, which names its kept copy.
    pub at_unix_ms: u64,
}

/// What adopt asks of the machine beyond the files it reads.
pub trait AdoptWorktree {
    /// The checkout's version control: the paths below `relative` (a `/`-separated directory of
    /// the checkout `repo`) that hold a change it has not committed, untracked files included; an
    /// error outcome when it cannot tell.
    fn changes_under(&self, repo: &Path, relative: &str) -> Result<Vec<String>, Outcome>;

    /// The volume `path` lies on, a name two paths share exactly when a directory can be renamed
    /// from one to the other ([`volume_of`]: the volume of the nearest part of `path` that
    /// exists, its links resolved). A test sets its own to stand for two volumes.
    fn volume(&self, path: &Path) -> io::Result<String> {
        volume_of(path)
    }
}

/// The decision's class: what the package becomes in its game's standard pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptClass {
    /// A business rule, judged by its value.
    Business,
    /// A maintenance rule (`"maintenance": true`), or a `maintenance.json` entry by its `uses`.
    Maintenance,
    /// A `held` entry: kept from scheduling, with its reason.
    Held,
}

impl AdoptClass {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "business" => Some(Self::Business),
            "maintenance" => Some(Self::Maintenance),
            "held" => Some(Self::Held),
            _ => None,
        }
    }
}

/// Where a derived member's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedFrom {
    /// The package itself (its `control.json` or its entry task).
    Package,
    /// The file's rules of the same tier.
    File,
    /// A neutral constant of the contract.
    Neutral,
    /// The contract's neutral floor, which the value the package gives falls below
    /// (`expected_duration_ms`: never below 60000).
    NeutralFloor,
}

/// One member adopt derived and wrote.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DerivedMember {
    /// The member, a JSON pointer inside the rule below its root without the leading `/`
    /// (`consumes/0/observation_source` for a member of an entry).
    pub member: String,
    pub value: Value,
    pub from: DerivedFrom,
    /// How it was derived, in words, where `from` alone does not say it (`feedback_stop`,
    /// `expected_duration_ms`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
}

/// One entry of a root file the adoption removed or replaced, so that the package has exactly one
/// role.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RootEntryChange {
    /// `schedule.json` or `maintenance.json`.
    pub file: String,
    /// The entry's JSON pointer in the file as the call read it.
    pub json_pointer: String,
    /// The entry as the call read it.
    pub before: Value,
    /// The entry as written; absent for an entry removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

/// Why the adopted business rule is not dispatched until a value is given.
#[derive(Debug, Clone, Serialize)]
pub struct ValueWait {
    /// The reason the Runtime's evaluator records for each of its candidates, a registered code:
    /// `benefit_unknown` with its `benefit_gap` (and `pool_id` where a pool is why).
    pub outcome: Outcome,
    /// The same in words, with what gives the value.
    pub text: String,
}

/// One member of the rule the decision gave and adopt wrote; the value as `fixed_value_milli`
/// with the origin `pack`, or as `value` `"pending"`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GivenMember {
    pub member: String,
    pub value: Value,
    /// For `fixed_value_milli`: `pack`, the one origin a caller gives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// One derived member a re-adoption would derive otherwise than the rule writes it: answered,
/// never written.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DerivedDrift {
    pub member: String,
    pub written: Value,
    pub derived: Value,
    pub from: DerivedFrom,
}

/// How far the adopted business rule declares its value, as the composer counts the file's judged
/// rules (`contracts/standard-pack-schedule.md`, "Judged rules").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptCoverage {
    Complete,
    Pending,
    Undeclared,
    Follower,
}

/// What the package's provenance says of its recording (ruling 2 on #464: a recording counts as
/// run through when its cross-check passed, its first decision was evaluated and no step needs
/// review).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordingReview {
    /// The entry task's provenance is a recording's (`source` `lab_record`).
    pub recorded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
    /// `cross_check.status` as the provenance records it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_check: Option<String>,
    /// The first decision was simulated: no `first_decision_not_evaluated` warning and no
    /// application step, which the offline simulation never passes.
    pub first_decision_evaluated: bool,
    /// The steps whose click the recording marked `needs_review`, by their step number.
    pub steps_needing_review: Vec<u64>,
    pub run_through: bool,
}

/// The answer of `standard-pack adopt`.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackAdoptResponse {
    /// `adopted`, or `validated` under `--dry-run`.
    pub status: String,
    pub dry_run: bool,
    pub repo: String,
    pub game: String,
    pub server: String,
    pub package_id: String,
    pub entry_task_id: String,
    pub class: AdoptClass,
    /// The package was adopted before, and this adoption replaces its directory.
    pub replaced: bool,
    pub review: RecordingReview,
    /// The package's rule as `schedule.json` holds it after the adoption.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<Value>,
    /// Its `held` entry, for the class `held`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<Value>,
    /// Its `maintenance.json` entry, for the class `maintenance` with `uses`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<Value>,
    /// The entries of the root files the adoption removed (a held entry a decision of another
    /// class lifts, a `maintenance.json` entry a decision of another class ends), in order.
    pub removed_entries: Vec<RootEntryChange>,
    /// The entries of the root files that were there and that the adoption changed (the rule,
    /// the held entry, the `maintenance.json` entry), each as it was and as written.
    pub replaced_entries: Vec<RootEntryChange>,
    /// The subsystems `requires` gained, in the order they were added.
    pub requires_added: Vec<String>,
    /// Every member of the rule the decision gave and adopt wrote, in the order written.
    pub given: Vec<GivenMember>,
    /// Every member adopt derived and wrote, in the order written.
    pub derived: Vec<DerivedMember>,
    /// The derived members a re-adoption would derive otherwise; never written.
    pub derived_drift: Vec<DerivedDrift>,
    /// The members of the package's rule the model wrote (`fixed_value_milli` and
    /// `fixed_value_origin` when its origin is `model`), kept as they are whatever the decision
    /// names.
    pub model_kept: Vec<String>,
    /// The coverage of the adopted business rule; absent for another class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<AdoptCoverage>,
    /// For a business rule whose value is not known (a `pending` value, no fixed value and no
    /// effect, a pool without its valuation): it is not dispatched until a value is given; absent
    /// otherwise, and for a follower, which is dispatched with its chain's head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waits_for_value: Option<ValueWait>,
    /// Every warning of the check, the game's included, each once.
    pub warnings: Vec<Outcome>,
    /// What it wrote in the repository, relative to it, in order; empty under `--dry-run`.
    pub written: Vec<String>,
    /// Under `--dry-run`: what it would write.
    pub would_write: Vec<String>,
    /// Where the replaced pack directory was moved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    /// The checkout's check tree, `<state>/adopt-check/<checkout>/`, kept until the next call on
    /// the checkout empties it.
    pub check_tree: String,
    /// The repository's own items adopt does not write (a regression case, the census row, the
    /// catalog section), each by its path when the checkout holds it.
    pub next: Vec<String>,
}

/// Workflow #464 P1: `actinglab standard-pack adopt` (module doc).
pub fn standard_pack_adopt(
    request: StandardPackAdoptRequest,
    worktree: &dyn AdoptWorktree,
) -> LabResult<StandardPackAdoptResponse> {
    let repo = absolute(&request.repo)?;
    let state = absolute(&request.state_dir)?;
    let root = Repository::read(&repo)?;
    let (locator, record_id) = match &request.source {
        AdoptSource::Recording { record_id, lab_dir } => (
            recorded_package(record_id, lab_dir)?,
            Some(record_id.clone()),
        ),
        AdoptSource::Package(path) => (absolute(path)?, None),
    };
    let package = Package::read(&locator)?;
    let game = package.descriptor.game().to_owned();
    let server = package.descriptor.server().to_owned();
    let package_id = package.descriptor.package_id().to_owned();
    if game != root.game {
        return Err(refused(
            adopt(AdoptProblem::PackageMismatch).member("game"),
            format!(
                "the package declares the game {game:?}, applications.json the game {:?}",
                root.game
            ),
        ));
    }
    if !root.servers.contains(&server) {
        return Err(refused(
            adopt(AdoptProblem::PackageMismatch).member(SERVER),
            format!(
                "the package declares the server {server:?}, none of applications.json's {:?}",
                root.servers
            ),
        ));
    }
    if !package_id_names_one_path_segment(&package_id) || reserved_package_id(&package_id) {
        return Err(refused(
            named_package(adopt(AdoptProblem::PackageInvalid), &package_id),
            format!(
                "the package_id {package_id:?} names no directory packs/<package_id> (one path \
                 segment, not a name the game root reserves)"
            ),
        ));
    }

    // The earlier adoption, if any, and `--replace`.
    let existing = Existing::of(&root, &package_id, &server);
    let pack_relative = format!("{PACKS_DIRECTORY}/{package_id}");
    let pack_dir = repo.join(PACKS_DIRECTORY).join(&package_id);
    let staged = repo
        .join(PACKS_DIRECTORY)
        .join(format!("{package_id}.{PART}"));
    if fs::symlink_metadata(&staged).is_ok() {
        return Err(refused(
            adopt(AdoptProblem::PackExists).entry(format!("{pack_relative}.{PART}")),
            interrupted(&root.leftovers),
        ));
    }
    let pack_exists = fs::symlink_metadata(&pack_dir).is_ok();
    if !request.replace && (pack_exists || existing.any()) {
        return Err(refused(
            existing.pointing(adopt(AdoptProblem::PackExists), pack_exists, &pack_relative),
            format!("{package_id} was adopted before; give --replace to adopt it again"),
        ));
    }
    let kept_place = state
        .join(KEPT_DIRECTORY)
        .join(format!("{package_id}.old-{}", request.at_unix_ms));
    if request.replace && pack_exists {
        if fs::symlink_metadata(&kept_place).is_ok() {
            return Err(refused(
                adopt(AdoptProblem::PackExists).path(&kept_place),
                format!("the kept copy's place {} is taken", kept_place.display()),
            ));
        }
        let changes = worktree
            .changes_under(&repo, &pack_relative)
            .map_err(|cause| {
                refused(
                    adopt(AdoptProblem::PackDirty)
                        .entry(pack_relative.clone())
                        .caused_by(cause),
                    format!("the version control cannot tell whether {pack_relative} is committed"),
                )
            })?;
        if !changes.is_empty() {
            return Err(refused(
                adopt(AdoptProblem::PackDirty)
                    .entry(pack_relative.clone())
                    .count(changes.len()),
                format!(
                    "{pack_relative} holds changes not committed ({}); commit them so that the \
                     old version stays a commit",
                    changes.join(", ")
                ),
            ));
        }
        // The replaced directory is moved, never copied and removed: its kept place must lie on
        // the checkout's volume, or the move fails after the new directory was written.
        let volume = |path: &Path| {
            worktree.volume(path).map_err(|error| {
                refused(
                    adopt(AdoptProblem::ReplaceAcrossVolumes)
                        .entry(&pack_relative)
                        .path(&kept_place)
                        .caused_by(Outcome::from_io_error(&error, IoOp::Inspect).path(path)),
                    format!(
                        "the volume of {} cannot be told: {error}; nothing was written",
                        path.display()
                    ),
                )
            })
        };
        let checkout_volume = volume(&pack_dir)?;
        let kept_volume = volume(&kept_place)?;
        if checkout_volume != kept_volume {
            let instead = match &request.source {
                AdoptSource::Recording { .. } => format!(
                    "; the recording lies in this state directory, so with another --state-dir \
                     give its package by its path instead of --record-id: --package {}",
                    locator.display()
                ),
                AdoptSource::Package(_) => String::new(),
            };
            return Err(refused(
                adopt(AdoptProblem::ReplaceAcrossVolumes)
                    .entry(&pack_relative)
                    .path(&kept_place),
                format!(
                    "--replace moves {} to its kept place {}, and the checkout's volume \
                     {checkout_volume} is not the state directory's {kept_volume}, where a \
                     directory cannot be renamed to; give a --state-dir on the checkout's volume \
                     {checkout_volume}{instead}; nothing was written",
                    pack_dir.display(),
                    kept_place.display()
                ),
            ));
        }
    }

    // The decision; without one, the class of the earlier adoption.
    let decision = match &request.decision {
        Some(path) => Some(Decision::read(&absolute(path)?, &root.schedule)?),
        None => None,
    };
    let class = match (
        &decision,
        existing.class(&root.schedule, &root.maintenance_role, &package_id),
    ) {
        (Some(decision), _) => decision.class,
        (None, Some(class)) => class,
        (None, None) => {
            return Err(refused(
                adopt(AdoptProblem::DecisionInvalid).json_pointer(""),
                "a first adoption needs --decide <decision.json>".to_owned(),
            ));
        }
    };
    // One role (the decision's class decides): adopt writes `schedule.json` and
    // `maintenance.json` only, so a maintenance role another root file gives cannot be ended here.
    if let Some(decision) = &decision
        && decision.class != AdoptClass::Maintenance
        && let Some(file) = root.other_role_file(&package_id)
    {
        return Err(decision.invalid(
            "/class",
            &format!(
                "{file} gives {package_id} a maintenance role, which adopt does not write and \
                 which would stay beside the class {:?}; take it out of {file} first, or adopt \
                 the package as maintenance",
                decision.class
            ),
        ));
    }

    // The review (ruling 2): a class other than `held` needs a package run through.
    let mut review = package.review();
    if review.record_id.is_none() {
        review.record_id = record_id;
    }
    if class != AdoptClass::Held && !review.run_through {
        let outcome = named_package(adopt(AdoptProblem::RecordingUnreviewed), &package_id);
        return Err(refused(
            outcome.clone(),
            format!(
                "{package_id} is not run through (cross-check {:?}, first decision evaluated \
                 {}, steps needing review {:?}); adopt it as held",
                review.cross_check, review.first_decision_evaluated, review.steps_needing_review
            ),
        )
        .with_details(json!({"outcome": outcome.envelope(Detail::Full), "review": review})));
    }

    // No absolute local path in the package's text.
    let needles = [state.display().to_string(), repo.display().to_string()];
    if let Some((entry, pointer)) = local_path(&package.files, &needles) {
        return Err(refused(
            adopt(AdoptProblem::LocalPath)
                .entry(entry.clone())
                .json_pointer(bounded_pointer(&pointer)),
            format!("{entry} holds an absolute local path at {pointer:?}"),
        ));
    }

    // The rule, the held entry and the maintenance entry as they would be written.
    let plan = Plan::make(&PlanInputs {
        root: &root,
        package: &package,
        package_id: &package_id,
        server: &server,
        class,
        decision: decision.as_ref(),
        existing: &existing,
    })?;
    // The Runtime refuses every dispatch of a rule that reads an outcome key of its own its
    // package does not declare: every such predicate, given, derived or kept, is checked here.
    own_outcome_keys_declared(&plan, &package, decision.as_ref(), &package_id)?;

    // The tree as it would be written, checked as the Runtime reads it.
    let check = check_tree(&CheckInputs {
        repo: &repo,
        root: &root,
        state: &state,
        package_id: &package_id,
        files: &package.files,
        plan: &plan,
    })?;
    let rule_id = plan.rule_id();
    if check.answer.warnings.iter().any(|warning| {
        warning.is(SharedCode::StandardPackPackageUnclassified)
            && text_value(warning, PACKAGE_ID) == Some(package_id.as_str())
    }) {
        return Err(refused(
            named_package(adopt(AdoptProblem::RuleRefused), &package_id),
            format!(
                "{package_id} comes out unclassified: no rule, held entry or maintenance role \
                 names it; the check tree is at {}",
                check.tree.display()
            ),
        ));
    }
    // A business rule's value: none only for a follower of a chain, which is dispatched with its
    // head whatever its own value, and so never `pending` for one.
    let follower = rule_id.as_deref().is_some_and(|rule_id| {
        check
            .answer
            .benefit_followers
            .iter()
            .any(|follower| follower.task_id == rule_id)
    });
    if class == AdoptClass::Business
        && plan.held.is_none()
        && let Some(decision) = &decision
    {
        if follower && decision.value == Some(DecidedValue::Pending) {
            return Err(decision.invalid(
                "/value",
                "the rule follows its chain's head, and a follower is dispatched with its head \
                 whatever its own value, so \"pending\" would be untrue: give the follower no \
                 value at all, or adopt it apart from the chain (a trigger of its own) with its \
                 value",
            ));
        }
        if !follower && decision.value.is_none() && existing.rule.is_none() {
            return Err(decision.invalid(
                "/value",
                "a business rule needs its value: {fixed_value_milli, origin} or \"pending\" \
                 (only a follower of a chain, dispatched with its head, gives none)",
            ));
        }
    }
    let coverage = match (&plan.rule, class) {
        (Some(rule), AdoptClass::Business) if plan.held.is_none() => {
            Some(rule_coverage(rule, &root.schedule.pools, &check.answer))
        }
        _ => None,
    };
    let waits_for_value = match (&plan.rule, coverage) {
        (Some(rule), Some(coverage)) if coverage != AdoptCoverage::Follower => {
            value_wait(rule, &root.schedule.pools)
        }
        _ => None,
    };
    let warnings = all_warnings(&check);

    // What it writes.
    let mut changed = vec![pack_relative.clone()];
    if plan.schedule_bytes.is_some() {
        changed.push(STANDARD_PACK_SCHEDULE_FILE.to_owned());
    }
    if plan.maintenance_bytes.is_some() {
        changed.push(MAINTENANCE_FILE.to_owned());
    }
    let replaced = request.replace && pack_exists;
    let (written, would_write, kept) = if request.dry_run {
        (Vec::new(), changed, None)
    } else {
        let moved = write_adoption(&WriteInputs {
            repo: &repo,
            files: &package.files,
            staged: &staged,
            pack_dir: &pack_dir,
            kept: replaced.then_some(kept_place.as_path()),
            plan: &plan,
            read_schedule: root.schedule_bytes.as_deref(),
            read_maintenance: &root.maintenance_bytes,
        })?;
        (
            changed,
            Vec::new(),
            moved.map(|path| path.display().to_string()),
        )
    };
    Ok(StandardPackAdoptResponse {
        status: if request.dry_run {
            "validated".to_owned()
        } else {
            "adopted".to_owned()
        },
        dry_run: request.dry_run,
        repo: repo.display().to_string(),
        game,
        server,
        package_id,
        entry_task_id: package.descriptor.entry_task_id().to_owned(),
        class,
        replaced,
        review,
        rule: plan.rule.clone().map(Value::Object),
        held: plan.held.clone(),
        maintenance: plan.maintenance_entry.clone(),
        removed_entries: plan.removed.clone(),
        replaced_entries: plan.replaced.clone(),
        requires_added: plan.requires_added.clone(),
        given: plan.given.clone(),
        derived: plan.derived.clone(),
        derived_drift: plan.drift.clone(),
        model_kept: plan.model_kept.clone(),
        coverage,
        waits_for_value,
        warnings,
        written,
        would_write,
        kept,
        check_tree: check.tree.display().to_string(),
        next: next_items(&repo, &root.game),
    })
}

/// The checkout as adopt reads it.
struct Repository {
    game: String,
    servers: BTreeSet<String>,
    maintenance_bytes: Vec<u8>,
    maintenance: Vec<BundleMaintenance>,
    schedule_bytes: Option<Vec<u8>>,
    schedule: StandardPackSchedule,
    /// The packages a role of the root files makes maintenance-role.
    maintenance_role: BTreeSet<String>,
    /// The packages each root file adopt does not write (`pages.json`, `navigation.json`,
    /// `interrupts.json`) gives a maintenance role, by the file's name.
    other_roles: Vec<(&'static str, BTreeSet<String>)>,
    /// What interrupted adoptions left below `packs/` ([`leftovers`]); a leftover root file is
    /// refused when the checkout is read.
    leftovers: Vec<String>,
}

impl Repository {
    fn read(repo: &Path) -> LabResult<Self> {
        let applications = required_file(repo, APPLICATIONS_FILE)?;
        let (game, servers) = applications_of(&applications)?;
        let maintenance_bytes = required_file(repo, MAINTENANCE_FILE)?;
        let maintenance =
            serde_json::from_slice::<Vec<BundleMaintenance>>(strip_bom(&maintenance_bytes))
                .map_err(|error| {
                    repository_invalid(MAINTENANCE_FILE, format!("maintenance.json: {error}"))
                })?;
        if !fs::metadata(repo.join(PACKS_DIRECTORY)).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(repository_invalid(
                PACKS_DIRECTORY,
                format!("{} is no directory", repo.join(PACKS_DIRECTORY).display()),
            ));
        }
        let leftovers = leftovers(repo)?;
        let pack_prefix = format!("{PACKS_DIRECTORY}/");
        if let Some(part) = leftovers
            .iter()
            .find(|left| !left.starts_with(&pack_prefix))
        {
            return Err(repository_invalid(part, interrupted(&leftovers)));
        }
        let schedule_path = repo.join(STANDARD_PACK_SCHEDULE_FILE);
        let schedule_bytes = optional_file(&schedule_path, STANDARD_PACK_SCHEDULE_FILE)?;
        let schedule = match &schedule_bytes {
            Some(bytes) => parse_standard_pack_schedule(bytes).map_err(|issue| {
                refused(
                    adopt(AdoptProblem::RepositoryInvalid)
                        .entry(STANDARD_PACK_SCHEDULE_FILE)
                        .json_pointer(bounded_pointer(&issue.pointer))
                        .caused_by(root_issue_outcome(&game, &schedule_path, &issue)),
                    format!("schedule.json: {issue}"),
                )
            })?,
            None => StandardPackSchedule::default(),
        };
        let roles = maintenance_roles(repo, &game, &maintenance);
        Ok(Self {
            game,
            servers,
            maintenance_bytes,
            maintenance,
            schedule_bytes,
            schedule,
            maintenance_role: roles.all,
            other_roles: roles.others,
            leftovers,
        })
    }

    /// The root file adopt does not write that gives `package_id` a maintenance role, if any.
    fn other_role_file(&self, package_id: &str) -> Option<&'static str> {
        self.other_roles
            .iter()
            .find(|(_, packages)| packages.contains(package_id))
            .map(|(file, _)| *file)
    }
}

/// What an interrupted adoption leaves in the checkout: `schedule.json.part`,
/// `maintenance.json.part` and every `packs/<name>.part`, by their `/`-separated paths relative
/// to it, in that order.
fn leftovers(repo: &Path) -> LabResult<Vec<String>> {
    let mut left = Vec::new();
    for name in [STANDARD_PACK_SCHEDULE_FILE, MAINTENANCE_FILE] {
        let part = format!("{name}.{PART}");
        if fs::symlink_metadata(repo.join(&part)).is_ok() {
            left.push(part);
        }
    }
    let packs = repo.join(PACKS_DIRECTORY);
    let unreadable = |error: io::Error| {
        refused(
            adopt(AdoptProblem::RepositoryInvalid)
                .entry(PACKS_DIRECTORY)
                .caused_by(Outcome::from_io_error(&error, IoOp::ReadDir).path(&packs)),
            format!("{}: {error}", packs.display()),
        )
    };
    let mut names = fs::read_dir(&packs)
        .map_err(unreadable)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(unreadable)?;
    names.sort();
    let suffix = format!(".{PART}");
    for name in names {
        if let Some(text) = name.to_str()
            && text.ends_with(&suffix)
        {
            left.push(format!("{PACKS_DIRECTORY}/{text}"));
        }
    }
    Ok(left)
}

/// The message of a refusal for what an interrupted adoption left: each leftover by name, and
/// what to do (`contracts/standard-pack-authoring.md`, "If a call was interrupted").
fn interrupted(leftovers: &[String]) -> String {
    format!(
        "an earlier adoption was interrupted and left {leftovers:?} in the checkout. When it \
         began, the pack directory it wrote, packs/<package_id>, was absent or committed (adopt \
         refuses a pack directory with changes), so `git status -- packs/<package_id>` shows \
         exactly what it left there; schedule.json and maintenance.json are each written whole \
         and renamed in one step, so each is as it was before that call or as that call \
         computed it (`git diff` shows which); a replaced pack directory it had moved away is \
         in the last commit and under the state directory's adopt/. Move the leftovers named \
         out of the checkout by hand (adopt deletes nothing), then run adopt again"
    )
}

/// The game and the servers `applications.json` declares; its full check is `package bundle`'s.
fn applications_of(bytes: &[u8]) -> LabResult<(String, BTreeSet<String>)> {
    let invalid = |problem: &str| {
        repository_invalid(APPLICATIONS_FILE, format!("applications.json: {problem}"))
    };
    let value = serde_json::from_slice::<Value>(strip_bom(bytes))
        .map_err(|error| invalid(&error.to_string()))?;
    if value.get(SCHEMA_VERSION).and_then(Value::as_str) != Some(APPLICATIONS_SCHEMA) {
        return Err(invalid(
            "schema_version is not actingcommand.applications.v1",
        ));
    }
    let game = value
        .get("game")
        .and_then(Value::as_str)
        .filter(|game| !game.is_empty())
        .ok_or_else(|| invalid("game is no non-empty string"))?;
    let servers = value
        .get("servers")
        .and_then(Value::as_object)
        .filter(|servers| !servers.is_empty())
        .ok_or_else(|| invalid("servers is no non-empty object"))?;
    Ok((game.to_owned(), servers.keys().cloned().collect()))
}

/// The packages the root files give a maintenance role.
struct Roles {
    /// Every one (`standard_pack_maintenance_packages`).
    all: BTreeSet<String>,
    /// Those each root file adopt does not write gives, by the file's name.
    others: Vec<(&'static str, BTreeSet<String>)>,
}

/// The packages the root files give a maintenance role (`standard_pack_maintenance_packages`):
/// `maintenance.json`'s, and those of `pages.json`, `navigation.json` and `interrupts.json` where
/// they read, the last three also each apart. A root file that does not read gives none here;
/// the check of the tree refuses it.
fn maintenance_roles(repo: &Path, game: &str, maintenance: &[BundleMaintenance]) -> Roles {
    let read = |name: &str| fs::read(repo.join(name)).ok();
    let pages = read(STANDARD_PACK_PAGES_FILE)
        .and_then(|bytes| parse_standard_pack_pages(&bytes, Some(game)).ok());
    let interrupts = match (&pages, read(STANDARD_PACK_INTERRUPTS_FILE)) {
        (Some(listed), Some(bytes)) => {
            parse_standard_pack_interrupts(&bytes, Some(game), listed).ok()
        }
        _ => None,
    };
    let navigation = match (&pages, read(STANDARD_PACK_NAVIGATION_FILE)) {
        (Some(listed), Some(bytes)) => {
            parse_standard_pack_navigation(&bytes, Some(game), listed).ok()
        }
        _ => None,
    };
    let empty = StandardPackRoot {
        registry: StandardPackRegistry::default(),
        pages: None,
        interrupts: None,
        navigation: None,
        display_layout: None,
    };
    let only = |root: StandardPackRoot| standard_pack_maintenance_packages(&[], &root);
    let others = vec![
        (
            STANDARD_PACK_PAGES_FILE,
            only(StandardPackRoot {
                pages: pages.clone(),
                ..empty.clone()
            }),
        ),
        (
            STANDARD_PACK_NAVIGATION_FILE,
            only(StandardPackRoot {
                navigation: navigation.clone(),
                ..empty.clone()
            }),
        ),
        (
            STANDARD_PACK_INTERRUPTS_FILE,
            only(StandardPackRoot {
                interrupts: interrupts.clone(),
                ..empty
            }),
        ),
    ];
    let root = StandardPackRoot {
        registry: StandardPackRegistry::default(),
        pages,
        interrupts,
        navigation,
        display_layout: None,
    };
    Roles {
        all: standard_pack_maintenance_packages(maintenance, &root),
        others,
    }
}

/// The package to adopt, admitted.
struct Package {
    descriptor: TaskPackageDescriptor,
    /// `control.json` and `resources/**`, each JSON file in the canonical form.
    files: BTreeMap<String, Vec<u8>>,
    /// The entry task's `task.json`, when it reads.
    task: Option<Value>,
}

impl Package {
    fn read(locator: &Path) -> LabResult<Self> {
        let deadline = Instant::now() + PACK_DEADLINE;
        let descriptor = PreparedContainedTask::describe_path(ADOPT_LABEL, locator, deadline)
            .map_err(|error| {
                refused(
                    adopt(AdoptProblem::PackageInvalid)
                        .path(locator)
                        .caused_by(error.outcome()),
                    format!("{}: {error}", locator.display()),
                )
            })?;
        let entries = Containment::for_metadata_validation()
            .snapshot_content_directory(locator, Instant::now() + PACK_DEADLINE)
            .map_err(|error| {
                refused(
                    adopt(AdoptProblem::PackageInvalid).path(locator),
                    format!("{}: {error}", locator.display()),
                )
            })?;
        let mut files = BTreeMap::new();
        for (relative, bytes) in entries {
            if relative != "control.json" && !relative.starts_with("resources/") {
                continue;
            }
            let bytes = if is_json(&relative) {
                let value =
                    serde_json::from_slice::<Value>(strip_bom(&bytes)).map_err(|error| {
                        refused(
                            adopt(AdoptProblem::PackageInvalid)
                                .path(locator)
                                .entry(relative.clone()),
                            format!("{relative} is no JSON document: {error}"),
                        )
                    })?;
                canonical(&value)?
            } else {
                bytes
            };
            files.insert(relative, bytes);
        }
        let task_path = format!(
            "resources/operations/{}/task.json",
            descriptor.entry_task_id()
        );
        let task = files
            .get(&task_path)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
        Ok(Self {
            descriptor,
            files,
            task,
        })
    }

    fn provenance(&self) -> Option<&Value> {
        self.task.as_ref()?.get("provenance")
    }

    /// What the entry task's provenance says of the recording.
    fn review(&self) -> RecordingReview {
        let provenance = self.provenance();
        let recorded = provenance
            .and_then(|provenance| provenance.get("source"))
            .and_then(Value::as_str)
            == Some(RECORDED_SOURCE);
        let record_id = provenance
            .and_then(|provenance| provenance.get("record_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let cross_check = provenance
            .and_then(|provenance| provenance.get("cross_check"))
            .and_then(|cross_check| cross_check.get("status"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let not_evaluated = provenance
            .and_then(|provenance| provenance.get("warnings"))
            .and_then(Value::as_array)
            .is_some_and(|warnings| {
                warnings.iter().any(|warning| {
                    warning.get("code").and_then(Value::as_str)
                        == Some(FIRST_DECISION_NOT_EVALUATED)
                })
            });
        let application = self
            .operations()
            .iter()
            .any(|operation| operation.get("application").is_some());
        let steps_needing_review = provenance
            .and_then(|provenance| provenance.get("steps"))
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter(|step| {
                        step.get("click")
                            .and_then(|click| click.get("needs_review"))
                            .and_then(Value::as_bool)
                            == Some(true)
                    })
                    .filter_map(|step| step.get("step").and_then(Value::as_u64))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let first_decision_evaluated = recorded && !not_evaluated && !application;
        let run_through = first_decision_evaluated
            && cross_check.as_deref() == Some(CROSS_CHECK_PASSED)
            && steps_needing_review.is_empty();
        RecordingReview {
            recorded,
            record_id,
            cross_check,
            first_decision_evaluated,
            steps_needing_review,
            run_through,
        }
    }

    fn operations(&self) -> Vec<&Value> {
        self.task
            .as_ref()
            .and_then(|task| task.get("operations"))
            .and_then(Value::as_array)
            .map(|operations| operations.iter().collect())
            .unwrap_or_default()
    }

    /// The entry task's `scheduling_outcome` mappings.
    fn mappings(&self) -> Vec<&Value> {
        self.task
            .as_ref()
            .and_then(|task| task.get("scheduling_outcome"))
            .and_then(|outcome| outcome.get("mappings"))
            .and_then(Value::as_array)
            .map(|mappings| mappings.iter().collect())
            .unwrap_or_default()
    }

    /// Every outcome key the entry task declares (`scheduling_outcome.mappings`), each once, in
    /// declaration order: the keys a scheduled run of the package may be read for.
    fn declared_outcome_keys(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut keys = Vec::new();
        for key in self
            .mappings()
            .into_iter()
            .filter_map(|mapping| mapping.get("outcome_key").and_then(Value::as_str))
        {
            if seen.insert(key) {
                keys.push(key.to_owned());
            }
        }
        keys
    }

    /// The declared keys of a normal end: every mapping's but a busy one's (an activity still in
    /// progress, `"busy": true`) and one with a `remedy` (the business result was not the expected
    /// one), each once, in declaration order.
    fn normal_outcome_keys(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut keys = Vec::new();
        for mapping in self.mappings() {
            let busy = mapping.get("busy").and_then(Value::as_bool) == Some(true);
            let remedy = mapping.get("remedy").is_some();
            if let Some(key) = mapping.get("outcome_key").and_then(Value::as_str)
                && !busy
                && !remedy
                && seen.insert(key)
            {
                keys.push(key.to_owned());
            }
        }
        keys
    }

    /// A lower bound of one run, in milliseconds: the sum over its operations of the wait after
    /// the effect and one arrival interval (the package's timeouts are upper bounds).
    fn lower_bound_ms(&self) -> u64 {
        self.operations().iter().fold(0_u64, |total, operation| {
            let post = operation
                .get("post_delay_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let interval = operation
                .get("expect_after")
                .and_then(|expect| expect.get("interval_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            total.saturating_add(post).saturating_add(interval)
        })
    }
}

/// The package a stopped recording's last stop generated, `<lab_dir>/out/<package_id>.<ext>`,
/// when `recording.json` says that stop is the last change: stopped, not marked again since, with
/// its artifact.
fn recorded_package(record_id: &str, lab_dir: &Path) -> LabResult<PathBuf> {
    let path = lab_dir.join(RECORDING_FILE);
    let not_current = |member: Option<&str>, message: String| {
        let outcome = adopt(AdoptProblem::RecordingNotCurrent).path(&path);
        let outcome = match member {
            Some(member) => outcome.member(member),
            None => outcome,
        };
        refused(outcome, message)
    };
    let bytes = fs::read(&path)
        .map_err(|error| not_current(None, format!("{}: {error}", path.display())))?;
    let recording = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| not_current(None, format!("{}: {error}", path.display())))?;
    let text = |member: &str| recording.get(member).and_then(Value::as_str);
    if text(SCHEMA_VERSION) != Some(RECORDING_SCHEMA) {
        return Err(not_current(
            Some(SCHEMA_VERSION),
            format!("{} is no {RECORDING_SCHEMA}", path.display()),
        ));
    }
    if text("record_id") != Some(record_id) {
        return Err(not_current(
            Some("record_id"),
            format!("{} is the recording of another record_id", path.display()),
        ));
    }
    if text("status") != Some(RECORDING_STOPPED) {
        return Err(not_current(
            Some("status"),
            format!("the recording {record_id} is not stopped; run record stop"),
        ));
    }
    if recording
        .get("amended_at_unix_ms")
        .is_some_and(|amended| !amended.is_null())
    {
        return Err(not_current(
            Some("amended_at_unix_ms"),
            format!(
                "the recording {record_id} was marked again after its package was generated; run \
                 record stop"
            ),
        ));
    }
    let artifact = recording.get("artifact");
    let package_id = artifact
        .and_then(|artifact| artifact.get(PACKAGE_ID))
        .and_then(Value::as_str)
        .filter(|package_id| package_id_names_one_path_segment(package_id));
    let container = artifact
        .and_then(|artifact| artifact.get("container"))
        .and_then(Value::as_str)
        .filter(|container| matches!(*container, "zip" | "json"));
    let (Some(package_id), Some(container)) = (package_id, container) else {
        return Err(not_current(
            Some("artifact"),
            format!("the recording {record_id} names no package of its last stop"),
        ));
    };
    let locator = lab_dir
        .join(RECORDING_OUT)
        .join(format!("{package_id}.{container}"));
    if !fs::metadata(&locator).is_ok_and(|metadata| metadata.is_file()) {
        return Err(not_current(
            Some("artifact"),
            format!("{} is missing", locator.display()),
        ));
    }
    Ok(locator)
}

/// What an earlier adoption of the package left in the checkout.
struct Existing {
    /// The index of its rule: the rule whose `package_id` is the package's.
    rule: Option<usize>,
    held: Option<usize>,
    /// The index of its `maintenance.json` entry for its server.
    maintenance: Option<usize>,
}

impl Existing {
    fn of(root: &Repository, package_id: &str, server: &str) -> Self {
        Self {
            rule: root
                .schedule
                .rules
                .iter()
                .position(|rule| rule.package_id == package_id),
            held: root
                .schedule
                .held
                .iter()
                .position(|held| held.package_id == package_id),
            maintenance: root
                .maintenance
                .iter()
                .position(|entry| entry.package_id == package_id && entry.server == server),
        }
    }

    fn any(&self) -> bool {
        self.rule.is_some() || self.held.is_some() || self.maintenance.is_some()
    }

    /// The class the earlier adoption gave the package, as the composer reads the files: held
    /// wins, as the composer leaves a held package's rule out; then a maintenance role of the root
    /// files, which leaves its rule unjudged whatever it declares.
    fn class(
        &self,
        schedule: &StandardPackSchedule,
        maintenance_role: &BTreeSet<String>,
        package_id: &str,
    ) -> Option<AdoptClass> {
        if self.held.is_some() {
            return Some(AdoptClass::Held);
        }
        if self.any() && maintenance_role.contains(package_id) {
            return Some(AdoptClass::Maintenance);
        }
        if let Some(index) = self.rule {
            return Some(if schedule.rules[index].maintenance {
                AdoptClass::Maintenance
            } else {
                AdoptClass::Business
            });
        }
        self.maintenance.map(|_| AdoptClass::Maintenance)
    }

    /// `outcome` naming what was adopted before: the directory, else the rule or the held entry
    /// in `schedule.json`, else the entry in `maintenance.json`.
    fn pointing(&self, outcome: Outcome, pack_exists: bool, pack_relative: &str) -> Outcome {
        if pack_exists {
            return outcome.entry(pack_relative);
        }
        if let Some(index) = self.rule {
            return outcome
                .entry(STANDARD_PACK_SCHEDULE_FILE)
                .json_pointer(format!("/{RULES}/{index}"));
        }
        if let Some(index) = self.held {
            return outcome
                .entry(STANDARD_PACK_SCHEDULE_FILE)
                .json_pointer(format!("/{HELD}/{index}"));
        }
        match self.maintenance {
            Some(index) => outcome
                .entry(MAINTENANCE_FILE)
                .json_pointer(format!("/{index}")),
            None => outcome,
        }
    }
}

/// A decided value: the pending marker, or a fixed value of the pack's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecidedValue {
    Pending,
    Fixed { milli: u64 },
}

/// A decided `held` entry's text.
#[derive(Debug, Clone)]
struct HeldDecision {
    rationale: String,
    reference: Option<String>,
}

/// The decision document `actingcommand.pack-decision.v1` (B.2): only what the Lab cannot know.
#[derive(Debug, Clone)]
struct Decision {
    path: PathBuf,
    class: AdoptClass,
    trigger: Option<Value>,
    value: Option<DecidedValue>,
    consumes: Option<Vec<Map<String, Value>>>,
    produces: Option<Vec<Map<String, Value>>>,
    limited: bool,
    realization: bool,
    uses: Option<Vec<MaintenanceUse>>,
    held: Option<HeldDecision>,
    overrides: Map<String, Value>,
}

impl Decision {
    /// The document at `path`, checked on its own and against the file's pools.
    fn read(path: &Path, schedule: &StandardPackSchedule) -> LabResult<Self> {
        let invalid = |pointer: &str, message: &str| decision_invalid(path, pointer, message);
        let bytes =
            fs::read(path).map_err(|error| invalid("", &format!("{}: {error}", path.display())))?;
        let document = serde_json::from_slice::<Value>(strip_bom(&bytes))
            .map_err(|error| invalid("", &format!("not JSON: {error}")))?;
        let Value::Object(document) = document else {
            return Err(invalid("", "the decision is no JSON object"));
        };
        if let Some(unknown) = document
            .keys()
            .find(|member| !DECISION_MEMBERS.contains(&member.as_str()))
        {
            return Err(invalid(
                &format!("/{}", segment(unknown)),
                &format!("{unknown} is no member of a decision"),
            ));
        }
        if document.get(SCHEMA_VERSION).and_then(Value::as_str) != Some(PACK_DECISION_SCHEMA) {
            return Err(invalid(
                "/schema_version",
                &format!("schema_version is not {PACK_DECISION_SCHEMA}"),
            ));
        }
        let class = document
            .get("class")
            .and_then(Value::as_str)
            .and_then(AdoptClass::parse)
            .ok_or_else(|| invalid("/class", "class is business, maintenance or held"))?;
        let trigger = match document.get(TRIGGER) {
            None => None,
            Some(Value::Object(trigger)) => Some(Value::Object(trigger.clone())),
            Some(_) => return Err(invalid("/trigger", "trigger is a predicate object")),
        };
        let value = document
            .get("value")
            .map(|value| decided_value(value, path, &invalid))
            .transpose()?;
        let pools = schedule
            .pools
            .iter()
            .map(|pool| pool.id.as_str())
            .collect::<BTreeSet<_>>();
        let consumes = document
            .get(CONSUMES)
            .map(|effects| decided_effects(effects, CONSUMES, &pools, &invalid))
            .transpose()?;
        let produces = document
            .get(PRODUCES)
            .map(|effects| decided_effects(effects, PRODUCES, &pools, &invalid))
            .transpose()?;
        let mark = |member: &str| match document.get(member) {
            None => Ok(false),
            Some(Value::Bool(true)) => Ok(true),
            Some(_) => Err(invalid(
                &format!("/{member}"),
                &format!("{member}, when written, is true"),
            )),
        };
        let limited = mark(SCHEDULE_RULE_LIMITED_MEMBER)?;
        let realization = mark(SCHEDULE_RULE_REALIZATION_MEMBER)?;
        let uses = document
            .get(USES)
            .map(|uses| decided_uses(uses, &invalid))
            .transpose()?;
        let held = document
            .get(HELD)
            .map(|held| decided_held(held, &invalid))
            .transpose()?;
        let overrides = match document.get("overrides") {
            None => Map::new(),
            Some(Value::Object(overrides)) => overrides.clone(),
            Some(_) => return Err(invalid("/overrides", "overrides is an object")),
        };
        if let Some(unknown) = overrides
            .keys()
            .find(|member| !OVERRIDE_MEMBERS.contains(&member.as_str()))
        {
            return Err(invalid(
                &format!("/overrides/{}", segment(unknown)),
                &format!("{unknown} is no member adopt derives"),
            ));
        }
        if overrides.get(ID).is_some_and(|id| !id.is_string()) {
            return Err(invalid("/overrides/id", "id is a string"));
        }
        // The first member the class does not take.
        let forbidden = match class {
            AdoptClass::Business => [(USES, uses.is_some()), (HELD, held.is_some())]
                .into_iter()
                .find(|(_, given)| *given),
            AdoptClass::Maintenance => [
                ("value", value.is_some()),
                (SCHEDULE_RULE_LIMITED_MEMBER, limited),
                (SCHEDULE_RULE_REALIZATION_MEMBER, realization),
                (HELD, held.is_some()),
            ]
            .into_iter()
            .find(|(_, given)| *given),
            AdoptClass::Held => [
                (TRIGGER, trigger.is_some()),
                ("value", value.is_some()),
                (CONSUMES, consumes.is_some()),
                (PRODUCES, produces.is_some()),
                (SCHEDULE_RULE_LIMITED_MEMBER, limited),
                (SCHEDULE_RULE_REALIZATION_MEMBER, realization),
                (USES, uses.is_some()),
                ("overrides", !overrides.is_empty()),
            ]
            .into_iter()
            .find(|(_, given)| *given),
        };
        if let Some((member, _)) = forbidden {
            return Err(invalid(
                &format!("/{member}"),
                &format!("the class {class:?} takes no {member}"),
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            class,
            trigger,
            value,
            consumes,
            produces,
            limited,
            realization,
            uses,
            held,
            overrides,
        })
    }

    fn invalid(&self, pointer: &str, message: &str) -> LabError {
        decision_invalid(&self.path, pointer, message)
    }
}

/// `value`: `"pending"`, or `{fixed_value_milli, origin?}` whose origin, when written, is `pack`.
/// The origin `model` is `value_origin_model` (the coordinator's ruling on #464): only the model's
/// export writes it.
fn decided_value(
    value: &Value,
    path: &Path,
    invalid: &dyn Fn(&str, &str) -> LabError,
) -> LabResult<DecidedValue> {
    match value {
        Value::String(text) if text == VALUE_PENDING => Ok(DecidedValue::Pending),
        Value::Object(members) => {
            if let Some(unknown) = members
                .keys()
                .find(|member| !VALUE_MEMBERS.contains(&member.as_str()))
            {
                return Err(invalid(
                    &format!("/value/{}", segment(unknown)),
                    &format!("{unknown} is no member of a value"),
                ));
            }
            let milli = members
                .get(SCHEDULE_RULE_FIXED_VALUE_MEMBER)
                .and_then(Value::as_u64)
                .filter(|milli| *milli >= 1)
                .ok_or_else(|| {
                    invalid(
                        "/value/fixed_value_milli",
                        "fixed_value_milli is a whole number of at least 1",
                    )
                })?;
            match members.get("origin") {
                None => {}
                Some(Value::String(origin)) if origin == ORIGIN_PACK => {}
                Some(Value::String(origin)) if origin == SCHEDULE_VALUE_ORIGIN_MODEL => {
                    return Err(refused(
                        adopt(AdoptProblem::ValueOriginModel)
                            .path(path)
                            .json_pointer("/value/origin"),
                        "the decision at \"/value/origin\": only the model's export writes the \
                         origin model; give the number with the origin pack, or \"pending\""
                            .to_owned(),
                    ));
                }
                Some(_) => {
                    return Err(invalid("/value/origin", "origin, when written, is pack"));
                }
            }
            Ok(DecidedValue::Fixed { milli })
        }
        _ => Err(invalid(
            "/value",
            "value is \"pending\" or {fixed_value_milli, origin: pack}",
        )),
    }
}

/// `consumes` or `produces` (`member`): entries `{pool_id, amount | expected_amount_milli,
/// observation_source?, confidence_milli?}` of pools the file declares.
fn decided_effects(
    effects: &Value,
    member: &str,
    pools: &BTreeSet<&str>,
    invalid: &dyn Fn(&str, &str) -> LabError,
) -> LabResult<Vec<Map<String, Value>>> {
    let Value::Array(entries) = effects else {
        return Err(invalid(
            &format!("/{member}"),
            &format!("{member} is a list"),
        ));
    };
    let mut decided = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let pointer = format!("/{member}/{index}");
        let Value::Object(entry) = entry else {
            return Err(invalid(&pointer, "an entry is an object"));
        };
        if let Some(unknown) = entry
            .keys()
            .find(|name| !EFFECT_MEMBERS.contains(&name.as_str()))
        {
            return Err(invalid(
                &format!("{pointer}/{}", segment(unknown)),
                &format!("{unknown} is no member of an entry"),
            ));
        }
        match entry.get("pool_id").and_then(Value::as_str) {
            Some(pool) if pools.contains(pool) => {}
            _ => {
                return Err(invalid(
                    &format!("{pointer}/pool_id"),
                    "pool_id names no pool schedule.json declares",
                ));
            }
        }
        if !entry.contains_key("amount") && !entry.contains_key("expected_amount_milli") {
            return Err(invalid(
                &pointer,
                "an entry gives one run's amount or expected_amount_milli",
            ));
        }
        decided.push(entry.clone());
    }
    Ok(decided)
}

/// `uses`: one of `startup`, `return_home`, `recognition`, or a list of them.
fn decided_uses(
    uses: &Value,
    invalid: &dyn Fn(&str, &str) -> LabError,
) -> LabResult<Vec<MaintenanceUse>> {
    let words = match uses {
        Value::String(_) => vec![uses.clone()],
        Value::Array(words) if !words.is_empty() => words.clone(),
        _ => return Err(invalid("/uses", "uses is a use or a list of uses")),
    };
    let mut decided = Vec::new();
    for (index, word) in words.into_iter().enumerate() {
        let used = serde_json::from_value::<MaintenanceUse>(word).map_err(|_| {
            invalid(
                &format!("/uses/{index}"),
                "a use is startup, return_home or recognition",
            )
        })?;
        if !decided.contains(&used) {
            decided.push(used);
        }
    }
    Ok(decided)
}

/// `held`: `{reason, reference?}`, each a text of at most 1024 bytes without a control character.
fn decided_held(held: &Value, invalid: &dyn Fn(&str, &str) -> LabError) -> LabResult<HeldDecision> {
    let Value::Object(members) = held else {
        return Err(invalid("/held", "held is {reason, reference?}"));
    };
    if let Some(unknown) = members
        .keys()
        .find(|member| !HELD_MEMBERS.contains(&member.as_str()))
    {
        return Err(invalid(
            &format!("/held/{}", segment(unknown)),
            &format!("{unknown} is no member of held"),
        ));
    }
    let text = |member: &str| -> LabResult<Option<String>> {
        match members.get(member) {
            None => Ok(None),
            Some(Value::String(text)) if bounded_text(text) => Ok(Some(text.clone())),
            Some(_) => Err(invalid(
                &format!("/held/{member}"),
                "a non-blank text of at most 1024 bytes without a control character",
            )),
        }
    };
    let rationale =
        text(HELD_REASON)?.ok_or_else(|| invalid("/held/reason", "held names its reason"))?;
    let reference = text(HELD_REFERENCE)?;
    Ok(HeldDecision {
        rationale,
        reference,
    })
}

/// What adopt writes, computed before anything is written.
struct Plan {
    /// The bytes of `schedule.json` the check composes, and those adopt writes when it changes.
    check_schedule: Option<Vec<u8>>,
    schedule_bytes: Option<Vec<u8>>,
    /// The entries the bundle declares, and the bytes of `maintenance.json` when it changes.
    check_maintenance: Vec<BundleMaintenance>,
    maintenance_bytes: Option<Vec<u8>>,
    /// The package's rule after the adoption, as written.
    rule: Option<Map<String, Value>>,
    held: Option<Value>,
    maintenance_entry: Option<Value>,
    /// The entries of the root files removed and replaced, so that the package has one role.
    removed: Vec<RootEntryChange>,
    replaced: Vec<RootEntryChange>,
    requires_added: Vec<String>,
    given: Vec<GivenMember>,
    derived: Vec<DerivedMember>,
    drift: Vec<DerivedDrift>,
    model_kept: Vec<String>,
}

/// What [`Plan::make`] reads.
struct PlanInputs<'a> {
    root: &'a Repository,
    package: &'a Package,
    package_id: &'a str,
    server: &'a str,
    class: AdoptClass,
    decision: Option<&'a Decision>,
    existing: &'a Existing,
}

impl Plan {
    fn make(inputs: &PlanInputs<'_>) -> LabResult<Self> {
        let root = inputs.root;
        let mut document = match &root.schedule_bytes {
            Some(bytes) => match serde_json::from_slice::<Value>(bytes) {
                Ok(Value::Object(document)) => document,
                _ => {
                    return Err(repository_invalid(
                        STANDARD_PACK_SCHEDULE_FILE,
                        "schedule.json is no JSON object".to_owned(),
                    ));
                }
            },
            None => {
                let mut document = Map::new();
                document.insert(
                    SCHEMA_VERSION.to_owned(),
                    json!(STANDARD_PACK_SCHEDULE_VERSION),
                );
                document
            }
        };
        let mut plan = Self {
            check_schedule: None,
            schedule_bytes: None,
            check_maintenance: root.maintenance.clone(),
            maintenance_bytes: None,
            rule: None,
            held: None,
            maintenance_entry: None,
            removed: Vec::new(),
            replaced: Vec::new(),
            requires_added: Vec::new(),
            given: Vec::new(),
            derived: Vec::new(),
            drift: Vec::new(),
            model_kept: Vec::new(),
        };
        let tier = match inputs.class {
            AdoptClass::Maintenance => Tier::Maintenance,
            _ => Tier::Business,
        };
        let inputs_of_rule = |rule_id: &str| DeriveInputs {
            rules: root
                .schedule
                .rules
                .iter()
                .filter(|rule| rule.package_id != inputs.package_id)
                .map(|rule| (rule, tier_of(rule, &root.maintenance_role)))
                .collect(),
            tier,
            rule_id: rule_id.to_owned(),
            normal_outcome_keys: inputs.package.normal_outcome_keys(),
            lower_bound_ms: inputs.package.lower_bound_ms(),
        };

        // The rule.
        let wants_rule = match (inputs.class, inputs.decision) {
            (AdoptClass::Business, _) => true,
            (AdoptClass::Maintenance, Some(decision)) => {
                decision.trigger.is_some() || inputs.existing.rule.is_some()
            }
            (AdoptClass::Maintenance, None) => inputs.existing.rule.is_some(),
            (AdoptClass::Held, _) => false,
        };
        // A maintenance package is a rule (its trigger) or a role (its uses).
        if let Some(decision) = inputs.decision
            && inputs.class == AdoptClass::Maintenance
            && !wants_rule
            && decision.uses.is_none()
            && inputs.existing.maintenance.is_none()
        {
            return Err(decision.invalid(
                "/trigger",
                "the class maintenance needs its trigger (a rule) or its uses (a role)",
            ));
        }
        if let Some(index) = inputs.existing.rule {
            let Some(Value::Object(mut rule)) =
                array_member(&mut document, RULES)?.get(index).cloned()
            else {
                return Err(repository_invalid(
                    STANDARD_PACK_SCHEDULE_FILE,
                    format!("schedule.json: /rules/{index} is no object"),
                ));
            };
            let before = rule.clone();
            // A fixed value the model wrote is kept as it is, and answered.
            if rule
                .get(SCHEDULE_RULE_FIXED_VALUE_ORIGIN_MEMBER)
                .and_then(Value::as_str)
                == Some(SCHEDULE_VALUE_ORIGIN_MODEL)
            {
                plan.model_kept = vec![
                    SCHEDULE_RULE_FIXED_VALUE_MEMBER.to_owned(),
                    SCHEDULE_RULE_FIXED_VALUE_ORIGIN_MEMBER.to_owned(),
                ];
            }
            if let Some(decision) = inputs.decision.filter(|_| wants_rule) {
                plan.named(&mut rule, decision);
            }
            if wants_rule {
                let rule_id = rule
                    .get(ID)
                    .and_then(Value::as_str)
                    .unwrap_or(inputs.package_id)
                    .to_owned();
                plan.drift = drift(&rule, &inputs_of_rule(&rule_id), inputs.decision);
            }
            if let Some(slot) = array_member(&mut document, RULES)?.get_mut(index) {
                *slot = Value::Object(rule.clone());
            }
            if rule != before {
                plan.replaced.push(RootEntryChange {
                    file: STANDARD_PACK_SCHEDULE_FILE.to_owned(),
                    json_pointer: format!("/{RULES}/{index}"),
                    before: Value::Object(before),
                    after: Some(Value::Object(rule.clone())),
                });
            }
            plan.rule = Some(rule);
        } else if wants_rule {
            let decision = inputs.decision.ok_or_else(|| {
                refused(
                    adopt(AdoptProblem::DecisionInvalid).json_pointer(""),
                    "a first adoption needs --decide <decision.json>".to_owned(),
                )
            })?;
            let rule_id = decision
                .overrides
                .get(ID)
                .and_then(Value::as_str)
                .unwrap_or(inputs.package_id)
                .to_owned();
            let rule = plan.new_rule(inputs, decision, &inputs_of_rule(&rule_id))?;
            array_member(&mut document, RULES)?.push(Value::Object(rule.clone()));
            plan.rule = Some(rule);
        }

        // The held entry.
        if inputs.class == AdoptClass::Held {
            let decided = inputs.decision.and_then(|decision| decision.held.as_ref());
            let held = array_member(&mut document, HELD)?;
            match (inputs.existing.held, decided) {
                (Some(index), decided) => {
                    let before = held.get(index).cloned();
                    if let (Some(decided), Some(Value::Object(entry))) =
                        (decided, held.get_mut(index))
                    {
                        entry.insert(HELD_REASON.to_owned(), json!(decided.rationale));
                        if let Some(reference) = &decided.reference {
                            entry.insert(HELD_REFERENCE.to_owned(), json!(reference));
                        }
                    }
                    plan.held = held.get(index).cloned();
                    if let Some(before) = before
                        && plan.held.as_ref() != Some(&before)
                    {
                        plan.replaced.push(RootEntryChange {
                            file: STANDARD_PACK_SCHEDULE_FILE.to_owned(),
                            json_pointer: format!("/{HELD}/{index}"),
                            before,
                            after: plan.held.clone(),
                        });
                    }
                }
                (None, Some(decided)) => {
                    let mut entry = Map::new();
                    entry.insert(PACKAGE_ID.to_owned(), json!(inputs.package_id));
                    entry.insert(HELD_REASON.to_owned(), json!(decided.rationale));
                    if let Some(reference) = &decided.reference {
                        entry.insert(HELD_REFERENCE.to_owned(), json!(reference));
                    }
                    held.push(Value::Object(entry.clone()));
                    plan.held = Some(Value::Object(entry));
                }
                (None, None) => {
                    return Err(match inputs.decision {
                        Some(decision) => decision.invalid("/held", "the class held names held"),
                        None => decision_invalid(Path::new(""), "", "held needs a decision"),
                    });
                }
            }
        } else if let Some(index) = inputs.existing.held
            && inputs.decision.is_some()
        {
            // A decision of another class lifts the hold: the rule is composed again.
            let held = array_member(&mut document, HELD)?;
            if index < held.len() {
                plan.removed.push(RootEntryChange {
                    file: STANDARD_PACK_SCHEDULE_FILE.to_owned(),
                    json_pointer: format!("/{HELD}/{index}"),
                    before: held.remove(index),
                    after: None,
                });
            }
        }

        // The words `requires` must name for the rule.
        let missing = plan
            .rule
            .as_ref()
            .map(required_words)
            .unwrap_or_default()
            .into_iter()
            .filter(|word| {
                !root
                    .schedule
                    .requires
                    .iter()
                    .any(|named| named.as_str() == *word)
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            plan.requires_added = missing.clone();
            let words = missing.into_iter().map(Value::String).collect::<Vec<_>>();
            if document.get(REQUIRES).is_some_and(Value::is_array) {
                array_member(&mut document, REQUIRES)?.extend(words);
            } else {
                document = after_schema_version(document, REQUIRES, Value::Array(words));
            }
        }

        // `schedule.json`: written in the canonical form only when it was in it.
        let schedule = canonical(&Value::Object(document))?;
        let schedule_changed = root.schedule_bytes.as_deref() != Some(schedule.as_slice());
        let any_schedule =
            root.schedule_bytes.is_some() || plan.rule.is_some() || plan.held.is_some();
        if schedule_changed && any_schedule {
            if let Some(original) = &root.schedule_bytes {
                require_canonical(original, STANDARD_PACK_SCHEDULE_FILE)?;
            }
            plan.schedule_bytes = Some(schedule.clone());
        }
        plan.check_schedule = any_schedule.then_some(schedule);

        // `maintenance.json`: an entry for the class maintenance with `uses`; a decision of
        // another class ends every entry of the package there, so that it has one role.
        let uses = inputs
            .decision
            .filter(|decision| decision.class == AdoptClass::Maintenance)
            .and_then(|decision| decision.uses.clone());
        let ends_entries = inputs
            .decision
            .is_some_and(|decision| decision.class != AdoptClass::Maintenance);
        let named_entries = root
            .maintenance
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.package_id == inputs.package_id)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if uses.is_some() || (ends_entries && !named_entries.is_empty()) {
            let mut entries = serde_json::from_slice::<Value>(strip_bom(&root.maintenance_bytes))
                .ok()
                .and_then(|value| match value {
                    Value::Array(entries) => Some(entries),
                    _ => None,
                })
                .ok_or_else(|| {
                    repository_invalid(MAINTENANCE_FILE, "maintenance.json is no list".to_owned())
                })?;
            match uses {
                Some(uses) => {
                    let uses_value = serde_json::to_value(&uses).map_err(|error| {
                        repository_invalid(
                            MAINTENANCE_FILE,
                            format!("the uses do not serialize: {error}"),
                        )
                    })?;
                    match inputs.existing.maintenance {
                        Some(index) => {
                            let before = entries.get(index).cloned();
                            if let Some(Value::Object(entry)) = entries.get_mut(index) {
                                entry.insert(USES.to_owned(), uses_value);
                            }
                            plan.maintenance_entry = entries.get(index).cloned();
                            if let Some(before) = before
                                && plan.maintenance_entry.as_ref() != Some(&before)
                            {
                                plan.replaced.push(RootEntryChange {
                                    file: MAINTENANCE_FILE.to_owned(),
                                    json_pointer: format!("/{index}"),
                                    before,
                                    after: plan.maintenance_entry.clone(),
                                });
                            }
                        }
                        None => {
                            let mut entry = Map::new();
                            entry.insert(PACKAGE_ID.to_owned(), json!(inputs.package_id));
                            entry.insert(SERVER.to_owned(), json!(inputs.server));
                            entry.insert(USES.to_owned(), uses_value);
                            entries.push(Value::Object(entry.clone()));
                            plan.maintenance_entry = Some(Value::Object(entry));
                        }
                    }
                }
                None => {
                    // From the last, so that each index is the one the file was read with.
                    let mut ended = Vec::new();
                    for index in named_entries.into_iter().rev() {
                        if index < entries.len() {
                            ended.push(RootEntryChange {
                                file: MAINTENANCE_FILE.to_owned(),
                                json_pointer: format!("/{index}"),
                                before: entries.remove(index),
                                after: None,
                            });
                        }
                    }
                    ended.reverse();
                    plan.removed.extend(ended);
                }
            }
            let value = Value::Array(entries);
            plan.check_maintenance = serde_json::from_value::<Vec<BundleMaintenance>>(
                value.clone(),
            )
            .map_err(|error| {
                repository_invalid(MAINTENANCE_FILE, format!("maintenance.json: {error}"))
            })?;
            let bytes = canonical(&value)?;
            if bytes != root.maintenance_bytes {
                require_canonical(&root.maintenance_bytes, MAINTENANCE_FILE)?;
                plan.maintenance_bytes = Some(bytes);
            }
        } else if let Some(index) = inputs.existing.maintenance {
            plan.maintenance_entry = root
                .maintenance
                .get(index)
                .and_then(|entry| serde_json::to_value(entry).ok());
        }
        Ok(plan)
    }

    /// The id of the package's rule after the adoption.
    fn rule_id(&self) -> Option<String> {
        self.rule
            .as_ref()
            .and_then(|rule| rule.get(ID))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// A first rule: the decision's members, the derived ones, in the order a rule is written.
    fn new_rule(
        &mut self,
        inputs: &PlanInputs<'_>,
        decision: &Decision,
        derive_inputs: &DeriveInputs<'_>,
    ) -> LabResult<Map<String, Value>> {
        let trigger = decision
            .trigger
            .clone()
            .ok_or_else(|| decision.invalid("/trigger", "a new rule needs its trigger"))?;
        // A business rule without its value is refused after the check of the tree, which tells
        // a follower of a chain (dispatched with its head, so given none) from any other rule.
        let mut rule = Map::new();
        match decision.overrides.get(ID) {
            Some(id) => self.give(ID, id.clone()),
            None => self.derive_member(ID, json!(derive_inputs.rule_id), DerivedFrom::Package),
        }
        rule.insert(ID.to_owned(), json!(derive_inputs.rule_id));
        self.derive_member(PACKAGE_ID, json!(inputs.package_id), DerivedFrom::Package);
        rule.insert(PACKAGE_ID.to_owned(), json!(inputs.package_id));
        if decision.class == AdoptClass::Maintenance {
            self.give(SCHEDULE_RULE_MAINTENANCE_MEMBER, Value::Bool(true));
            rule.insert(
                SCHEDULE_RULE_MAINTENANCE_MEMBER.to_owned(),
                Value::Bool(true),
            );
        }
        for member in DERIVED_MEMBERS {
            let value = match decision.overrides.get(member) {
                Some(value) => {
                    self.give(member, value.clone());
                    value.clone()
                }
                None => {
                    let (value, from) = derive(derive_inputs, member).ok_or_else(|| {
                        decision.invalid(
                            &format!("/overrides/{member}"),
                            &format!(
                                "nothing gives {member} (the package, the file's rules of its \
                                 tier and the contract's defaults): name it in overrides"
                            ),
                        )
                    })?;
                    self.derived.push(DerivedMember {
                        member: member.to_owned(),
                        value: value.clone(),
                        from,
                        how: derivation(derive_inputs, member, from),
                    });
                    value
                }
            };
            if member == PRIORITY {
                rule.insert(PRIORITY.to_owned(), value);
                self.give(TRIGGER, trigger.clone());
                rule.insert(TRIGGER.to_owned(), trigger.clone());
            } else if member == FEEDBACK_STOP {
                rule.insert(FEEDBACK_STOP.to_owned(), value);
                let consumes = self.effects(decision.consumes.as_deref(), CONSUMES);
                let produces = self.effects(decision.produces.as_deref(), PRODUCES);
                rule.insert(CONSUMES.to_owned(), consumes);
                rule.insert(PRODUCES.to_owned(), produces);
                match decision.value {
                    Some(DecidedValue::Fixed { milli }) => {
                        self.give_value(milli);
                        rule.insert(SCHEDULE_RULE_FIXED_VALUE_MEMBER.to_owned(), json!(milli));
                    }
                    Some(DecidedValue::Pending) => self.give_pending(),
                    None => {}
                }
                if decision.realization {
                    self.give(SCHEDULE_RULE_REALIZATION_MEMBER, Value::Bool(true));
                    rule.insert(
                        SCHEDULE_RULE_REALIZATION_MEMBER.to_owned(),
                        Value::Bool(true),
                    );
                }
                if decision.limited {
                    self.give(SCHEDULE_RULE_LIMITED_MEMBER, Value::Bool(true));
                    rule.insert(SCHEDULE_RULE_LIMITED_MEMBER.to_owned(), Value::Bool(true));
                }
            } else {
                rule.insert(member.to_owned(), value);
            }
        }
        if let Some(loop_budget) = decision.overrides.get(LOOP_BUDGET) {
            self.give(LOOP_BUDGET, loop_budget.clone());
            rule.insert(LOOP_BUDGET.to_owned(), loop_budget.clone());
        }
        Ok(rule)
    }

    /// `consumes` or `produces` as a rule writes them: each decided entry with its direction, and
    /// the evidence members it does not give derived from the contract's defaults; none when the
    /// decision names none.
    fn effects(&mut self, decided: Option<&[Map<String, Value>]>, member: &str) -> Value {
        let direction = if member == CONSUMES {
            "consume"
        } else {
            "produce"
        };
        let Some(decided) = decided else {
            self.derive_member(member, Value::Array(Vec::new()), DerivedFrom::Neutral);
            return Value::Array(Vec::new());
        };
        let mut effects = Vec::new();
        for (index, entry) in decided.iter().enumerate() {
            let mut effect = Map::new();
            if let Some(pool) = entry.get("pool_id") {
                effect.insert("pool_id".to_owned(), pool.clone());
            }
            effect.insert("direction".to_owned(), json!(direction));
            for name in ["amount", "expected_amount_milli"] {
                if let Some(value) = entry.get(name) {
                    effect.insert(name.to_owned(), value.clone());
                }
            }
            for (name, neutral) in [
                ("observation_source", json!(NEUTRAL_OBSERVATION_SOURCE)),
                ("confidence_milli", json!(NEUTRAL_CONFIDENCE_MILLI)),
            ] {
                let value = match entry.get(name) {
                    Some(value) => value.clone(),
                    None => {
                        self.derive_member(
                            &format!("{member}/{index}/{name}"),
                            neutral.clone(),
                            DerivedFrom::Neutral,
                        );
                        neutral
                    }
                };
                effect.insert(name.to_owned(), value);
            }
            effects.push(Value::Object(effect));
        }
        let written = Value::Array(effects);
        self.give(member, written.clone());
        written
    }

    /// A re-adoption's decision on the package's rule: only the members it names; a fixed value
    /// the model wrote stays as it is (answered in `model_kept`).
    fn named(&mut self, rule: &mut Map<String, Value>, decision: &Decision) {
        if let Some(trigger) = &decision.trigger {
            self.give(TRIGGER, trigger.clone());
            rule.insert(TRIGGER.to_owned(), trigger.clone());
        }
        if let Some(value) = decision.value
            && self.model_kept.is_empty()
        {
            match value {
                DecidedValue::Fixed { milli } => {
                    self.give_value(milli);
                    rule.insert(SCHEDULE_RULE_FIXED_VALUE_MEMBER.to_owned(), json!(milli));
                }
                DecidedValue::Pending => {
                    self.give_pending();
                    rule.shift_remove(SCHEDULE_RULE_FIXED_VALUE_MEMBER);
                    rule.shift_remove(SCHEDULE_RULE_FIXED_VALUE_ORIGIN_MEMBER);
                }
            }
        }
        if decision.consumes.is_some() {
            let consumes = self.effects(decision.consumes.as_deref(), CONSUMES);
            rule.insert(CONSUMES.to_owned(), consumes);
        }
        if decision.produces.is_some() {
            let produces = self.effects(decision.produces.as_deref(), PRODUCES);
            rule.insert(PRODUCES.to_owned(), produces);
        }
        if decision.limited {
            self.give(SCHEDULE_RULE_LIMITED_MEMBER, Value::Bool(true));
            rule.insert(SCHEDULE_RULE_LIMITED_MEMBER.to_owned(), Value::Bool(true));
        }
        if decision.realization {
            self.give(SCHEDULE_RULE_REALIZATION_MEMBER, Value::Bool(true));
            rule.insert(
                SCHEDULE_RULE_REALIZATION_MEMBER.to_owned(),
                Value::Bool(true),
            );
        }
        if decision.class == AdoptClass::Maintenance {
            self.give(SCHEDULE_RULE_MAINTENANCE_MEMBER, Value::Bool(true));
            rule.insert(
                SCHEDULE_RULE_MAINTENANCE_MEMBER.to_owned(),
                Value::Bool(true),
            );
        } else {
            rule.shift_remove(SCHEDULE_RULE_MAINTENANCE_MEMBER);
        }
        for (member, value) in &decision.overrides {
            self.give(member, value.clone());
            rule.insert(member.clone(), value.clone());
        }
    }

    fn derive_member(&mut self, member: &str, value: Value, from: DerivedFrom) {
        self.derived.push(DerivedMember {
            member: member.to_owned(),
            value,
            from,
            how: None,
        });
    }

    fn give(&mut self, member: &str, value: Value) {
        self.given.push(GivenMember {
            member: member.to_owned(),
            value,
            origin: None,
        });
    }

    /// The caller's fixed value: `fixed_value_milli`, the origin `pack`.
    fn give_value(&mut self, milli: u64) {
        self.given.push(GivenMember {
            member: SCHEDULE_RULE_FIXED_VALUE_MEMBER.to_owned(),
            value: json!(milli),
            origin: Some(ORIGIN_PACK.to_owned()),
        });
    }

    /// The caller's pending marker: `value` `"pending"`, no fixed value written.
    fn give_pending(&mut self) {
        self.give("value", json!(VALUE_PENDING));
    }
}

/// The tier a rule of the file runs in, for the derivation: maintenance when it declares itself
/// maintenance or its package has a maintenance role by the root files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Business,
    Maintenance,
}

fn tier_of(rule: &ScheduleRule, maintenance_role: &BTreeSet<String>) -> Tier {
    if rule.maintenance || maintenance_role.contains(&rule.package_id) {
        Tier::Maintenance
    } else {
        Tier::Business
    }
}

/// What the derivation of one rule reads.
struct DeriveInputs<'a> {
    /// The file's rules but the package's own, each with its tier.
    rules: Vec<(&'a ScheduleRule, Tier)>,
    tier: Tier,
    rule_id: String,
    /// The outcome keys of a normal end the package's entry task declares
    /// ([`Package::normal_outcome_keys`]).
    normal_outcome_keys: Vec<String>,
    /// The package's lower bound of one run, in milliseconds; 0 when it gives none.
    lower_bound_ms: u64,
}

/// The value of the derived member `member` and where it came from (B.3, "Defaults"); `None`
/// when nothing gives it, and the decision's `overrides` must.
fn derive(inputs: &DeriveInputs<'_>, member: &str) -> Option<(Value, DerivedFrom)> {
    let of_file = |neutral: Value| {
        Some(match most_frequent(inputs, member) {
            Some(value) => (value, DerivedFrom::File),
            None => (neutral, DerivedFrom::Neutral),
        })
    };
    match member {
        PRIORITY => priority(inputs).map(|priority| (json!(priority), DerivedFrom::File)),
        FEEDBACK_STOP => feedback_stop(inputs),
        ON_FAILURE => Some((
            json!({
                "action": "pause",
                "retry_limit": 1,
                "retry_backoff_ms": 0,
                "escalation_threshold": 2
            }),
            DerivedFrom::Neutral,
        )),
        SENSITIVE => Some((Value::Bool(false), DerivedFrom::Neutral)),
        YIELD_POINTS => Some((Value::Array(Vec::new()), DerivedFrom::Neutral)),
        // Never below the neutral floor: the package's waits are a lower bound only, and a value
        // far below a run's time would rank the task first among its instance's tasks.
        EXPECTED_DURATION_MS => Some(if inputs.lower_bound_ms >= NEUTRAL_EXPECTED_DURATION_MS {
            (json!(inputs.lower_bound_ms), DerivedFrom::Package)
        } else {
            (
                json!(NEUTRAL_EXPECTED_DURATION_MS),
                DerivedFrom::NeutralFloor,
            )
        }),
        NEXT_RUN_CLAMP_MS => of_file(json!(NEUTRAL_NEXT_RUN_CLAMP_MS)),
        COOLDOWN_MS => of_file(json!(NEUTRAL_COOLDOWN_MS)),
        LOAD_PROFILE => of_file(json!({"kind": "light"})),
        STRATEGIC_WEIGHT_MILLI => of_file(json!(NEUTRAL_STRATEGIC_WEIGHT_MILLI)),
        _ => None,
    }
}

/// The priority of the rule's tier: with exactly two values among the file's rules the business
/// tier's is the lower and the maintenance tier's the higher; otherwise the one value the rules of
/// its tier agree on; `None` when they give none.
fn priority(inputs: &DeriveInputs<'_>) -> Option<i64> {
    let values = |same_tier: bool| {
        inputs
            .rules
            .iter()
            .filter(|(_, tier)| !same_tier || *tier == inputs.tier)
            .filter_map(|(rule, _)| rule.body.get(PRIORITY).and_then(Value::as_i64))
            .collect::<BTreeSet<_>>()
    };
    let all = values(false);
    if all.len() == 2 {
        return match inputs.tier {
            Tier::Business => all.first().copied(),
            Tier::Maintenance => all.last().copied(),
        };
    }
    let same = values(true);
    if same.len() == 1 {
        same.first().copied()
    } else {
        None
    }
}

/// The most frequent value of `member` among the file's rules of the rule's tier, the first in
/// file order of equally frequent ones.
fn most_frequent(inputs: &DeriveInputs<'_>, member: &str) -> Option<Value> {
    frequent(
        inputs
            .rules
            .iter()
            .filter(|(_, tier)| *tier == inputs.tier)
            .filter_map(|(rule, _)| rule.body.get(member).cloned())
            .collect(),
    )
}

fn frequent(values: Vec<Value>) -> Option<Value> {
    let mut counted: Vec<(Value, usize)> = Vec::new();
    for value in values {
        let position = counted.iter().position(|(seen, _)| *seen == value);
        match position {
            Some(index) => counted[index].1 += 1,
            None => counted.push((value, 1)),
        }
    }
    let mut best: Option<(Value, usize)> = None;
    for (value, count) in counted {
        if best.as_ref().is_none_or(|(_, top)| count > *top) {
            best = Some((value, count));
        }
    }
    best.map(|(value, _)| value)
}

/// `feedback_stop`, never copied from another rule (a copied stop may never come true, so that the
/// task would be dispatched again at every cooldown): "this rule's own run ended with a key its
/// package declares for a normal end", an outcome predicate on the rule itself for each such key,
/// under `any` when there are several, so that the task runs once for each time its trigger comes
/// true. `None` when the package declares no such key, and the decision's `overrides` must give
/// it.
fn feedback_stop(inputs: &DeriveInputs<'_>) -> Option<(Value, DerivedFrom)> {
    let mut predicates = inputs
        .normal_outcome_keys
        .iter()
        .map(|key| {
            json!({
                "kind": "outcome",
                "task_id": inputs.rule_id,
                "outcome_key": key,
                "comparison": "eq",
                "value": {"type": "boolean", "value": true}
            })
        })
        .collect::<Vec<_>>();
    let stop = match predicates.len() {
        0 => return None,
        1 => predicates.remove(0),
        _ => json!({"kind": "any", "predicates": predicates}),
    };
    Some((stop, DerivedFrom::Package))
}

/// How a derived member was derived, in words, where `from` alone does not say it.
fn derivation(inputs: &DeriveInputs<'_>, member: &str, from: DerivedFrom) -> Option<String> {
    match (member, from) {
        (FEEDBACK_STOP, _) => Some(format!(
            "derived from the package: this rule's own run ended with a key its package declares \
             for a normal end ({:?}), so that the task runs once for each time its trigger comes \
             true",
            inputs.normal_outcome_keys
        )),
        (EXPECTED_DURATION_MS, DerivedFrom::NeutralFloor) => Some(format!(
            "derived, neutral floor: the package's lower bound of one run, {} ms (the sum of its \
             waits), is below the neutral {NEUTRAL_EXPECTED_DURATION_MS}; the measured runs \
             replace it",
            inputs.lower_bound_ms
        )),
        (EXPECTED_DURATION_MS, _) => Some(format!(
            "derived from the package: its lower bound of one run, {} ms (the sum of its waits); \
             the measured runs replace it",
            inputs.lower_bound_ms
        )),
        _ => None,
    }
}

/// The derived members of a re-adopted rule that the derivation now gives otherwise than the
/// rule writes them, but those the decision's `overrides` names.
fn drift(
    rule: &Map<String, Value>,
    inputs: &DeriveInputs<'_>,
    decision: Option<&Decision>,
) -> Vec<DerivedDrift> {
    let mut drift = Vec::new();
    for member in DERIVED_MEMBERS {
        if decision.is_some_and(|decision| decision.overrides.contains_key(member)) {
            continue;
        }
        let (Some(written), Some((derived, from))) = (rule.get(member), derive(inputs, member))
        else {
            continue;
        };
        if *written != derived {
            drift.push(DerivedDrift {
                member: member.to_owned(),
                written: written.clone(),
                derived,
                from,
            });
        }
    }
    drift
}

/// The subsystems `requires` must name for what `rule` writes
/// (`contracts/standard-pack-schedule.md`, "`requires`"), each once, in this order: a fixed
/// value, a fact reference, a pool fact reference, a probe, a consumption read from a fact, a
/// queue wait.
fn required_words(rule: &Map<String, Value>) -> Vec<&'static str> {
    let mut words = Vec::new();
    if rule.contains_key(SCHEDULE_RULE_FIXED_VALUE_MEMBER) {
        add_word(&mut words, BENEFIT_FIXED_VALUE_SUBSYSTEM);
    }
    for member in [TRIGGER, FEEDBACK_STOP] {
        if let Some(node) = rule.get(member) {
            predicate_words(node, &mut words);
        }
    }
    let consumption_fact = rule
        .get(CONSUMES)
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| is_fact_reference(entry.get("amount")))
        });
    if consumption_fact {
        add_word(&mut words, CONSUMPTION_FACT_SUBSYSTEM);
    }
    if rule.contains_key(MAX_QUEUE_WAIT_MEMBER) {
        add_word(&mut words, QUEUE_WAIT_SUBSYSTEM);
    }
    words
}

fn add_word(words: &mut Vec<&'static str>, word: &'static str) {
    if !words.contains(&word) {
        words.push(word);
    }
}

/// The words a predicate node needs, through `all`, `any` (`predicates`) and `not` (`predicate`).
fn predicate_words(node: &Value, words: &mut Vec<&'static str>) {
    let Value::Object(members) = node else {
        return;
    };
    let kind = members.get("kind").and_then(Value::as_str);
    let reference = is_fact_reference(members.get("value"));
    if kind == Some("fact") && reference {
        add_word(words, FACT_REFERENCE_SUBSYSTEM);
    }
    if kind == Some("resource_projection") && reference {
        add_word(words, POOL_FACT_REFERENCE_SUBSYSTEM);
    }
    if kind == Some("fact") && members.contains_key("on_unknown") {
        add_word(words, FACT_UNKNOWN_PROBE_SUBSYSTEM);
    }
    if let Some(Value::Array(children)) = members.get("predicates") {
        for child in children {
            predicate_words(child, words);
        }
    }
    if let Some(child) = members.get("predicate") {
        predicate_words(child, words);
    }
}

/// Whether `value` is a fact reference: an object whose `type` is `fact`.
fn is_fact_reference(value: Option<&Value>) -> bool {
    value
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        == Some(FACT_REFERENCE_TYPE)
}

/// The coverage of the adopted business rule: a follower or undeclared as the check answers it,
/// else complete when every entry has its quantity and every pool it reads its valuation (with a
/// fixed value, every pool it consumes), pending otherwise.
fn rule_coverage(
    rule: &Map<String, Value>,
    pools: &[ScheduleItem],
    check: &StandardPackCheckResponse,
) -> AdoptCoverage {
    let rule_id = rule.get(ID).and_then(Value::as_str).unwrap_or_default();
    if check
        .benefit_followers
        .iter()
        .any(|follower| follower.task_id == rule_id)
    {
        return AdoptCoverage::Follower;
    }
    if check.warnings.iter().any(|warning| {
        warning.is(SharedCode::StandardPackBenefitUndeclared)
            && text_value(warning, "task_id") == Some(rule_id)
    }) {
        return AdoptCoverage::Undeclared;
    }
    let valued = |pool_id: Option<&str>| {
        pools.iter().any(|pool| {
            Some(pool.id.as_str()) == pool_id
                && matches!(pool.body.get("valuation"), Some(Value::Object(_)))
        })
    };
    let entries = |member: &str| {
        rule.get(member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let complete = if rule.contains_key(SCHEDULE_RULE_FIXED_VALUE_MEMBER) {
        entries(CONSUMES)
            .iter()
            .all(|entry| valued(entry.get("pool_id").and_then(Value::as_str)))
    } else {
        let all = entries(CONSUMES)
            .into_iter()
            .chain(entries(PRODUCES))
            .collect::<Vec<_>>();
        !all.is_empty()
            && all.iter().all(|entry| {
                entry.get("amount").and_then(Value::as_str) != Some(VALUE_PENDING)
                    && valued(entry.get("pool_id").and_then(Value::as_str))
            })
    };
    if complete {
        AdoptCoverage::Complete
    } else {
        AdoptCoverage::Pending
    }
}

/// Workflow #464 P1 item 2: every outcome predicate of the schedule as it would be written that
/// reads the adopted rule's own outcome (`kind` `outcome`, `task_id` the rule's id: the rule's own
/// trigger and stop, given, derived or kept, and any other rule's or pool's that reads it) names a
/// key the package declares. The Runtime refuses every dispatch of a task whose outcome is read
/// for a key its package does not declare (`contracts/scheduling/README.md`, the outcome
/// declaration). A refusal is `decision_invalid` at the predicate's key: in the decision when the
/// decision gave the member, in `schedule.json` as it would be written otherwise.
fn own_outcome_keys_declared(
    plan: &Plan,
    package: &Package,
    decision: Option<&Decision>,
    package_id: &str,
) -> LabResult<()> {
    let (Some(rule_id), None, Some(bytes)) = (plan.rule_id(), &plan.held, &plan.check_schedule)
    else {
        return Ok(());
    };
    let document = serde_json::from_slice::<Value>(bytes).map_err(|error| {
        repository_invalid(
            STANDARD_PACK_SCHEDULE_FILE,
            format!("schedule.json as it would be written does not read: {error}"),
        )
    })?;
    let declared = package.declared_outcome_keys();
    let declared_set = declared.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut references = Vec::new();
    for member in [RULES, "pools"] {
        if let Some(items) = document.get(member) {
            own_outcome_references(items, &format!("/{member}"), &rule_id, &mut references);
        }
    }
    let own_index = document
        .get(RULES)
        .and_then(Value::as_array)
        .and_then(|rules| {
            rules
                .iter()
                .position(|rule| rule.get(PACKAGE_ID).and_then(Value::as_str) == Some(package_id))
        });
    for (pointer, key) in references {
        if key
            .as_deref()
            .is_some_and(|named| declared_set.contains(named))
        {
            continue;
        }
        let key_pointer = format!("{pointer}/outcome_key");
        // Where the predicate came from: the decision's trigger or stop, else the file.
        let given = own_index.zip(decision).and_then(|(index, decided)| {
            let rest = key_pointer.strip_prefix(&format!("/{RULES}/{index}"))?;
            if let Some(tail) = rest.strip_prefix(&format!("/{TRIGGER}"))
                && decided.trigger.is_some()
            {
                return Some((decided, format!("/{TRIGGER}{tail}")));
            }
            if let Some(tail) = rest.strip_prefix(&format!("/{FEEDBACK_STOP}"))
                && decided.overrides.contains_key(FEEDBACK_STOP)
            {
                return Some((decided, format!("/overrides/{FEEDBACK_STOP}{tail}")));
            }
            None
        });
        let message = format!(
            "the outcome predicate at {key_pointer:?} reads the rule {rule_id}'s own outcome on \
             the key {key:?}, which its package does not declare (it declares {declared:?}); the \
             Runtime refuses every dispatch of such a rule: name one of the declared keys"
        );
        let error = match given {
            Some((decided, decided_pointer)) => decided.invalid(&decided_pointer, &message),
            None => refused(
                adopt(AdoptProblem::DecisionInvalid)
                    .entry(STANDARD_PACK_SCHEDULE_FILE)
                    .json_pointer(bounded_pointer(&key_pointer)),
                format!("schedule.json as it would be written: {message}"),
            ),
        };
        let outcome = cause_of(&error);
        return Err(error.with_details(json!({
            "outcome": outcome.envelope(Detail::Full),
            "declared_outcome_keys": declared
        })));
    }
    Ok(())
}

/// Each object below `value` (at `pointer`) that reads `rule_id`'s own outcome, by its pointer,
/// with the key it names (`None` when it names none as a text).
fn own_outcome_references(
    value: &Value,
    pointer: &str,
    rule_id: &str,
    references: &mut Vec<(String, Option<String>)>,
) {
    match value {
        Value::Object(members) => {
            if members.get("kind").and_then(Value::as_str) == Some("outcome")
                && members.get("task_id").and_then(Value::as_str) == Some(rule_id)
            {
                references.push((
                    pointer.to_owned(),
                    members
                        .get("outcome_key")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                ));
            }
            for (name, member) in members {
                own_outcome_references(
                    member,
                    &format!("{pointer}/{}", segment(name)),
                    rule_id,
                    references,
                );
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                own_outcome_references(item, &format!("{pointer}/{index}"), rule_id, references);
            }
        }
        _ => {}
    }
}

/// Why the business rule `rule` is not dispatched until a value is given, as the evaluator judges
/// its net benefit before reading any pool (`contracts/scheduling/README.md`, "Net benefit within
/// a tier"): no fixed value and no effect, an entry whose amount is the pending marker, a pool it
/// touches without its valuation or with the marker. `None` when its value is declared (it is
/// then judged by it at each evaluation).
fn value_wait(rule: &Map<String, Value>, pools: &[ScheduleItem]) -> Option<ValueWait> {
    let rule_id = rule.get(ID).and_then(Value::as_str).unwrap_or_default();
    let fixed = rule.contains_key(SCHEDULE_RULE_FIXED_VALUE_MEMBER);
    let entries = |member: &str| {
        rule.get(member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    // With a fixed value its `produces` are not read.
    let produces = if fixed { Vec::new() } else { entries(PRODUCES) };
    let consumes = entries(CONSUMES);
    let give = "give it with --replace and a decision whose value is {fixed_value_milli, origin: \
                pack}, or by the model's export, or declare its produces and consumes on valued \
                pools";
    let (gap, pool_id, why, remedy) = if !fixed && produces.is_empty() && consumes.is_empty() {
        (
            BenefitGap::NoEffects,
            None,
            "it writes no fixed value and no produces or consumes entry".to_owned(),
            give.to_owned(),
        )
    } else {
        produces.iter().chain(&consumes).find_map(|entry| {
            let pool_id = entry.get("pool_id").and_then(Value::as_str)?.to_owned();
            if entry.get("amount").and_then(Value::as_str) == Some(VALUE_PENDING) {
                return Some((
                    BenefitGap::EntryPending,
                    Some(pool_id.clone()),
                    format!("its entry on the pool {pool_id} has the amount \"pending\""),
                    "write the entry's amount".to_owned(),
                ));
            }
            let valuation = pools
                .iter()
                .find(|pool| pool.id == pool_id)
                .and_then(|pool| pool.body.get("valuation"));
            match valuation {
                Some(Value::Object(_)) => None,
                Some(Value::String(marker)) if marker == VALUE_PENDING => Some((
                    BenefitGap::ValuationPending,
                    Some(pool_id.clone()),
                    format!("the pool {pool_id} has the valuation \"pending\""),
                    "write the pool's valuation in schedule.json".to_owned(),
                )),
                _ => Some((
                    BenefitGap::ValuationMissing,
                    Some(pool_id.clone()),
                    format!("the pool {pool_id} has no valuation"),
                    "write the pool's valuation in schedule.json".to_owned(),
                )),
            }
        })?
    };
    let outcome = Outcome::new(PolicyCode::BenefitUnknown).benefit_gap(gap);
    let outcome = match &pool_id {
        Some(pool_id) => outcome.pool_id(pool_id.as_str()),
        None => outcome,
    };
    Some(ValueWait {
        outcome,
        text: format!(
            "{rule_id} is not dispatched until a value is given: {why}; {remedy}. Until then the \
             Runtime's evaluator holds each of its candidates with this reason"
        ),
    })
}

/// The check tree and what its checks answered.
struct Checked {
    tree: PathBuf,
    answer: StandardPackCheckResponse,
    /// The bundle's warnings, which the check of the installed layout does not repeat (the
    /// recognition packages its maintenance declarations leave undeclared).
    bundle_warnings: Vec<Outcome>,
    /// The check tree's marker, open and locked until the call ends.
    _marker: File,
}

/// What [`check_tree`] reads.
struct CheckInputs<'a> {
    repo: &'a Path,
    root: &'a Repository,
    state: &'a Path,
    package_id: &'a str,
    files: &'a BTreeMap<String, Vec<u8>>,
    plan: &'a Plan,
}

/// The name of a checkout's check tree: the checkout's last segment (its characters that name a
/// file on every system, at most 32), `-`, and 16 hexadecimal digits of the SHA-256 of its
/// resolved path, so that one checkout has one tree whatever path names it.
fn check_tree_name(repo: &Path) -> LabResult<String> {
    let resolved = fs::canonicalize(repo).map_err(|error| {
        refused(
            adopt(AdoptProblem::RepositoryInvalid)
                .path(repo)
                .caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize).path(repo)),
            format!("{}: {error}", repo.display()),
        )
    })?;
    let digest = format!(
        "{:x}",
        Sha256::digest(resolved.to_string_lossy().as_bytes())
    );
    let stem = resolved
        .file_name()
        .map(|name| {
            name.to_string_lossy()
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                        character
                    } else {
                        '_'
                    }
                })
                .take(CHECK_TREE_STEM_CHARS)
                .collect::<String>()
        })
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "checkout".to_owned());
    Ok(format!("{stem}-{}", &digest[..CHECK_TREE_DIGEST_CHARS]))
}

/// The checkout's check tree, `<state>/adopt-check/<checkout>/` ([`check_tree_name`]), made ready
/// for this call: made and marked ([`CHECK_TREE_MARKER`]) when it is not there; when it is, refused
/// without its marker (it is then not the tool's own and nothing in it is touched) and while
/// another call holds the marker, and otherwise emptied of everything but the marker. The marker
/// is answered open and locked, and stays so until the call ends.
fn prepare_check_tree(state: &Path, repo: &Path) -> LabResult<(PathBuf, File)> {
    let parent = state.join(CHECK_DIRECTORY);
    fs::create_dir_all(&parent).map_err(|error| tree_failed(&parent, &error, IoOp::CreateDir))?;
    let tree = parent.join(check_tree_name(repo)?);
    let marker = tree.join(CHECK_TREE_MARKER);
    match fs::create_dir(&tree) {
        Ok(()) => {
            let text = format!(
                "actinglab standard-pack adopt keeps the check tree of the checkout {} in this \
                 directory. Each call on that checkout empties it, but for this file, before it \
                 lays its tree; without this file adopt leaves the directory alone.\n",
                repo.display()
            );
            write_new(&marker, text.as_bytes())
                .map_err(|error| tree_failed(&marker, &error, IoOp::Write))?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(tree_failed(&tree, &error, IoOp::CreateDir)),
    }
    if !fs::symlink_metadata(&marker).is_ok_and(|metadata| metadata.is_file()) {
        return Err(refused(
            adopt(AdoptProblem::CheckTreeUnmarked).path(&tree),
            format!(
                "{} is the place of the check tree of the checkout {} and holds no marker \
                 {CHECK_TREE_MARKER}: it is not the tool's own, and adopt touches nothing in it; \
                 move it away by hand or give another --state-dir; nothing was written",
                tree.display(),
                repo.display()
            ),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&marker)
        .map_err(|error| tree_failed(&marker, &error, IoOp::Open))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(refused(
                adopt(AdoptProblem::CheckTreeBusy).path(&tree),
                format!(
                    "another adoption of the checkout {} with this state directory is running (it \
                     holds the marker of {}); wait for it to end, then run adopt again; nothing \
                     was written",
                    repo.display(),
                    tree.display()
                ),
            ));
        }
        Err(TryLockError::Error(error)) => {
            return Err(tree_failed(&marker, &error, IoOp::Lock));
        }
    }
    // Everything in it but its marker is this tool's scratch of an earlier call.
    let entries = fs::read_dir(&tree)
        .map_err(|error| tree_failed(&tree, &error, IoOp::ReadDir))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|error| tree_failed(&tree, &error, IoOp::ReadDir))?;
    for entry in entries {
        if entry.file_name().and_then(|name| name.to_str()) == Some(CHECK_TREE_MARKER) {
            continue;
        }
        let metadata = fs::symlink_metadata(&entry)
            .map_err(|error| tree_failed(&entry, &error, IoOp::Metadata))?;
        let emptied = if metadata.is_dir() {
            fs::remove_dir_all(&entry)
        } else {
            fs::remove_file(&entry)
        };
        emptied.map_err(|error| tree_failed(&entry, &error, IoOp::Remove))?;
    }
    Ok((tree, file))
}

/// B.4 item 6: the tree as it would be written, in the checkout's check tree
/// ([`prepare_check_tree`]): `packs-root/` (the repository's pack directories, the package's own as
/// it would be written), `root/` (the root files adopt changes), `bundle/` (`package bundle` with
/// every root file the repository holds) and `installed/<game>/` (its installed layout, checked by
/// `standard-pack check`). Both compose the schedule with one instance per server; a composition
/// that refuses the file as it would be written is `rule_refused` with the composer's pointer, any
/// other refusal is answered as it is. Nothing is written in the repository.
fn check_tree(inputs: &CheckInputs<'_>) -> LabResult<Checked> {
    let (tree, marker) = prepare_check_tree(inputs.state, inputs.repo)?;
    let packs_root = tree.join("packs-root");
    create_dir(&packs_root)?;
    let packs = inputs.repo.join(PACKS_DIRECTORY);
    let mut names = fs::read_dir(&packs)
        .map_err(|error| tree_failed(&packs, &error, IoOp::ReadDir))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|error| tree_failed(&packs, &error, IoOp::ReadDir))?;
    names.sort();
    for name in names {
        let source = packs.join(&name);
        let is_directory = fs::symlink_metadata(&source)
            .map_err(|error| tree_failed(&source, &error, IoOp::Metadata))?
            .is_dir();
        if !is_directory || name.to_str() == Some(inputs.package_id) {
            continue;
        }
        copy_tree(&source, &packs_root.join(&name))?;
    }
    write_files(&packs_root.join(inputs.package_id), inputs.files)?;
    let staged_root = tree.join("root");
    create_dir(&staged_root)?;
    let schedule = match &inputs.plan.check_schedule {
        Some(bytes) => {
            let path = staged_root.join(STANDARD_PACK_SCHEDULE_FILE);
            write_new(&path, bytes).map_err(|error| tree_failed(&path, &error, IoOp::Write))?;
            Some(path)
        }
        None => None,
    };
    let present = |name: &str| {
        let path = inputs.repo.join(name);
        fs::metadata(&path)
            .is_ok_and(|metadata| metadata.is_file())
            .then_some(path)
    };
    let notices = inputs.repo.join(STANDARD_PACK_NOTICES_DIRECTORY);
    let bundle_dir = tree.join("bundle");
    let bundle = package_bundle(PackageBundleRequest {
        applications: inputs.repo.join(APPLICATIONS_FILE),
        packs_root,
        out: bundle_dir.clone(),
        source: None,
        maintenance: Some(inputs.plan.check_maintenance.clone()),
        root_files: StandardPackRootFiles {
            pages: present(STANDARD_PACK_PAGES_FILE),
            interrupts: present(STANDARD_PACK_INTERRUPTS_FILE),
            navigation: present(STANDARD_PACK_NAVIGATION_FILE),
            schedule,
            display_layout: present(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
            loading: present(STANDARD_PACK_LOADING_FILE),
            notices: fs::metadata(&notices)
                .is_ok_and(|metadata| metadata.is_dir())
                .then_some(notices),
            goals: present(STANDARD_PACK_GOALS_FILE),
        },
    })
    .map_err(|error| composed(error, &tree))?;
    let installed = tree.join("installed").join(&inputs.root.game);
    fs::create_dir_all(&installed)
        .map_err(|error| tree_failed(&installed, &error, IoOp::CreateDir))?;
    let entries = fs::read_dir(&bundle_dir)
        .map_err(|error| tree_failed(&bundle_dir, &error, IoOp::ReadDir))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|error| tree_failed(&bundle_dir, &error, IoOp::ReadDir))?;
    for source in entries {
        let Some(name) = source.file_name().map(ToOwned::to_owned) else {
            continue;
        };
        if !source.is_dir() {
            let bytes =
                fs::read(&source).map_err(|error| tree_failed(&source, &error, IoOp::Read))?;
            let target = installed.join(&name);
            write_new(&target, &bytes)
                .map_err(|error| tree_failed(&target, &error, IoOp::Write))?;
        } else if name.to_str() == Some(PACKS_DIRECTORY) {
            // Each `packs/<package_id>/` of the bundle is `<package_id>/` of the installed root.
            let packs = fs::read_dir(&source)
                .map_err(|error| tree_failed(&source, &error, IoOp::ReadDir))?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()
                .map_err(|error| tree_failed(&source, &error, IoOp::ReadDir))?;
            for pack in packs {
                if let Some(pack_name) = pack.file_name() {
                    copy_tree(&pack, &installed.join(pack_name))?;
                }
            }
        } else {
            copy_tree(&source, &installed.join(&name))?;
        }
    }
    let answer = check_standard_pack_root(StandardPackCheckRequest { root: installed })
        .map_err(|error| composed(error, &tree))?;
    Ok(Checked {
        tree,
        answer,
        bundle_warnings: bundle.warnings,
        _marker: marker,
    })
}

/// A refusal of the check tree: `rule_refused` with the composer's pointer when it refuses
/// `schedule.json`, the refusal as it is otherwise; either way nothing was written in the
/// repository.
fn composed(mut error: LabError, tree: &Path) -> LabError {
    let cause = cause_of(&error);
    let at_schedule = error.code == SharedCode::StandardPackRootInvalid.as_str()
        && text_value(&cause, "path")
            .and_then(|path| Path::new(path).file_name())
            .and_then(|name| name.to_str())
            == Some(STANDARD_PACK_SCHEDULE_FILE);
    if !at_schedule {
        error.message = format!(
            "{}; nothing was written in the repository; the check tree is at {}",
            error.message,
            tree.display()
        );
        return error;
    }
    let pointer = text_value(&cause, "json_pointer")
        .map(bounded_pointer)
        .unwrap_or_default();
    refused(
        adopt(AdoptProblem::RuleRefused)
            .entry(STANDARD_PACK_SCHEDULE_FILE)
            .json_pointer(pointer.clone())
            .caused_by(cause),
        format!(
            "the composer refuses schedule.json as it would be written at {pointer:?}: {}; the \
             check tree is at {}",
            error.message,
            tree.display()
        ),
    )
}

/// Every warning of the check, the game's included (those of the installed layout's check, then
/// the bundle's), each once.
fn all_warnings(check: &Checked) -> Vec<Outcome> {
    let mut seen = Vec::new();
    let mut all = Vec::new();
    for warning in check.answer.warnings.iter().chain(&check.bundle_warnings) {
        let envelope = warning.envelope(Detail::Full);
        if !seen.contains(&envelope) {
            seen.push(envelope);
            all.push(warning.clone());
        }
    }
    all
}

/// What [`write_adoption`] writes.
struct WriteInputs<'a> {
    repo: &'a Path,
    files: &'a BTreeMap<String, Vec<u8>>,
    staged: &'a Path,
    pack_dir: &'a Path,
    /// Where the replaced pack directory goes, when it is replaced.
    kept: Option<&'a Path>,
    plan: &'a Plan,
    /// `schedule.json` (absent: `None`) and `maintenance.json` as the call read them, which the
    /// plan was computed from.
    read_schedule: Option<&'a [u8]>,
    read_maintenance: &'a [u8],
}

/// B.5: the pack directory written whole as its `.part` and renamed, the replaced one first moved
/// to its kept place and moved back when the new one cannot take its place; `schedule.json` and
/// `maintenance.json` each as `<name>.part`, read again just before its rename and renamed only
/// while it is the file the call read (so that no call overwrites another's work). A failure names
/// where it stopped and what was done before; nothing is removed. The kept place, when a directory
/// was moved there.
fn write_adoption(inputs: &WriteInputs<'_>) -> LabResult<Option<PathBuf>> {
    let mut done: Vec<PathBuf> = Vec::new();
    let root_writes = [
        (
            STANDARD_PACK_SCHEDULE_FILE,
            inputs.plan.schedule_bytes.as_deref(),
            inputs.read_schedule,
        ),
        (
            MAINTENANCE_FILE,
            inputs.plan.maintenance_bytes.as_deref(),
            Some(inputs.read_maintenance),
        ),
    ];
    // Before the first write: each root file it will rename is still the one it read.
    for (name, bytes, read) in root_writes {
        if bytes.is_some() {
            unchanged(inputs.repo, name, read, &done)?;
        }
    }
    fs::create_dir(inputs.staged)
        .map_err(|error| write_failed(inputs.staged, &done, &error, IoOp::CreateDir))?;
    for (relative, bytes) in inputs.files {
        let target = join_relative(inputs.staged, relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| write_failed(parent, &done, &error, IoOp::CreateDir))?;
        }
        write_new(&target, bytes)
            .map_err(|error| write_failed(&target, &done, &error, IoOp::Write))?;
    }
    done.push(inputs.staged.to_path_buf());
    let mut moved = None;
    if let Some(kept) = inputs.kept {
        if let Some(parent) = kept.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| write_failed(parent, &done, &error, IoOp::CreateDir))?;
        }
        fs::rename(inputs.pack_dir, kept)
            .map_err(|error| write_failed(inputs.pack_dir, &done, &error, IoOp::Rename))?;
        done.push(kept.to_path_buf());
        moved = Some(kept.to_path_buf());
    }
    if let Err(error) = fs::rename(inputs.staged, inputs.pack_dir) {
        // The old directory goes back to its place, by a rename; nothing is removed.
        let Some(kept) = inputs.kept else {
            return Err(write_failed(inputs.pack_dir, &done, &error, IoOp::Rename));
        };
        let note = match fs::rename(kept, inputs.pack_dir) {
            Ok(()) => {
                done.retain(|done_path| done_path.as_path() != kept);
                format!(
                    "the old directory was moved back from {} to {}; the new one stays as {}",
                    kept.display(),
                    inputs.pack_dir.display(),
                    inputs.staged.display()
                )
            }
            Err(back) => format!(
                "moving the old directory back from {} failed too ({back}): it is there, {} is \
                 missing, and the new one stays as {}; move the old one back by hand",
                kept.display(),
                inputs.pack_dir.display(),
                inputs.staged.display()
            ),
        };
        return Err(noted(
            write_failed(inputs.pack_dir, &done, &error, IoOp::Rename),
            &note,
        ));
    }
    done.push(inputs.pack_dir.to_path_buf());
    for (name, bytes, read) in root_writes {
        let Some(bytes) = bytes else {
            continue;
        };
        let target = inputs.repo.join(name);
        let part = inputs.repo.join(format!("{name}.{PART}"));
        write_new(&part, bytes).map_err(|error| write_failed(&part, &done, &error, IoOp::Write))?;
        done.push(part.clone());
        unchanged(inputs.repo, name, read, &done)?;
        fs::rename(&part, &target)
            .map_err(|error| write_failed(&target, &done, &error, IoOp::Rename))?;
        done.push(target);
    }
    Ok(moved)
}

/// The root file `name` of `repo` is still `read` (`None`: absent), the bytes the call computed
/// from; otherwise a write refusal (`adopt_write_failed` at the file) that says it changed, after
/// `done`: the call renames nothing more.
fn unchanged(repo: &Path, name: &str, read: Option<&[u8]>, done: &[PathBuf]) -> LabResult<()> {
    let path = repo.join(name);
    let now = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(write_failed(&path, done, &error, IoOp::Read)),
    };
    if now.as_deref() == read {
        return Ok(());
    }
    let outcome = Outcome::new(ResourceToolingCode::AdoptWriteFailed).path(&path);
    let outcome = if done.is_empty() {
        outcome
    } else {
        outcome.paths(done.iter())
    };
    let listed = done
        .iter()
        .map(|done_path| done_path.display().to_string())
        .collect::<Vec<_>>();
    Err(LabError::new(
        LabErrorClass::UsageValidation,
        ResourceToolingCode::AdoptWriteFailed.as_str(),
        format!(
            "{} changed after this call read it (another adoption or another writer), so this \
             call stops here and renames nothing more, and does not overwrite that work; written \
             or moved before: {listed:?}; nothing is removed: compare with the version control, \
             move what this call left away by hand, and run adopt again",
            path.display()
        ),
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)})))
}

/// `error` with `note` added to its message.
fn noted(mut error: LabError, note: &str) -> LabError {
    error.message = format!("{}; {note}", error.message);
    error
}

/// The repository's own items adopt does not write, each by its path when the checkout holds it.
fn next_items(repo: &Path, game: &str) -> Vec<String> {
    let mut next = Vec::new();
    if repo.join(REGRESSION_DIRECTORY).is_dir() {
        next.push(format!("{REGRESSION_DIRECTORY}/{game}/"));
    }
    for file in NEXT_FILES {
        if repo.join(file).is_file() {
            next.push(file.to_owned());
        }
    }
    next
}

/// The first string of a text file of the package that holds an absolute local path, a drive or
/// network path, or one of `needles` (the state directory, the repository): its file and JSON
/// pointer (empty for a text file that is no JSON).
fn local_path(files: &BTreeMap<String, Vec<u8>>, needles: &[String]) -> Option<(String, String)> {
    let needles = needles
        .iter()
        .map(|needle| folded(needle.as_str()))
        .filter(|needle| !needle.is_empty())
        .collect::<Vec<_>>();
    let local = |text: &str| {
        let folded_text = folded(text);
        needles
            .iter()
            .any(|needle| folded_text.contains(needle.as_str()))
            || drive_or_network(&folded_text)
    };
    for (relative, bytes) in files {
        if is_json(relative) {
            let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
                continue;
            };
            if let Some(pointer) = local_string(&value, "", &local) {
                return Some((relative.clone(), pointer));
            }
        } else if let Ok(text) = std::str::from_utf8(bytes)
            && local(text)
        {
            return Some((relative.clone(), String::new()));
        }
    }
    None
}

/// The pointer of the first string (a member name too) of `value` at `pointer` that `local`
/// holds.
fn local_string(value: &Value, pointer: &str, local: &dyn Fn(&str) -> bool) -> Option<String> {
    match value {
        Value::String(text) if local(text) => Some(pointer.to_owned()),
        Value::Object(members) => members.iter().find_map(|(name, member)| {
            let child = format!("{pointer}/{}", segment(name));
            if local(name) {
                Some(child)
            } else {
                local_string(member, &child, local)
            }
        }),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(index, item)| local_string(item, &format!("{pointer}/{index}"), local)),
        _ => None,
    }
}

/// `text` in ASCII lower case with `\` as `/`.
fn folded(text: &str) -> String {
    text.to_ascii_lowercase().replace('\\', "/")
}

/// Whether a folded text holds a drive path (a letter, `:` and `/` where the letter does not
/// follow a letter or digit) or begins with a network path (`//` and a name).
fn drive_or_network(text: &str) -> bool {
    let bytes = text.as_bytes();
    let drive = bytes.windows(3).enumerate().any(|(index, window)| {
        window[0].is_ascii_alphabetic()
            && window[1] == b':'
            && window[2] == b'/'
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
    });
    let network = text
        .strip_prefix("//")
        .and_then(|rest| rest.bytes().next())
        .is_some_and(|first| first.is_ascii_alphanumeric());
    drive || network
}

/// `document` with `member` placed right after `schema_version`, every other member in its order.
fn after_schema_version(
    document: Map<String, Value>,
    member: &str,
    value: Value,
) -> Map<String, Value> {
    let mut pending = Some(value);
    let mut placed = Map::new();
    for (name, item) in document {
        let schema = name == SCHEMA_VERSION;
        placed.insert(name, item);
        if schema && let Some(value) = pending.take() {
            placed.insert(member.to_owned(), value);
        }
    }
    if let Some(value) = pending {
        placed.insert(member.to_owned(), value);
    }
    placed
}

/// The array `member` of `document`, an empty one added when absent.
fn array_member<'a>(
    document: &'a mut Map<String, Value>,
    member: &str,
) -> LabResult<&'a mut Vec<Value>> {
    document
        .entry(member.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            repository_invalid(
                STANDARD_PACK_SCHEDULE_FILE,
                format!("schedule.json: {member} is no list"),
            )
        })
}

/// A root file adopt must change is in the canonical form already.
fn require_canonical(bytes: &[u8], name: &str) -> LabResult<()> {
    let parsed = serde_json::from_slice::<Value>(bytes).ok();
    let canonical_bytes = parsed.as_ref().map(canonical).transpose()?;
    if canonical_bytes.as_deref() == Some(bytes) {
        return Ok(());
    }
    Err(refused(
        adopt(AdoptProblem::RootFileNotCanonical).entry(name),
        format!(
            "{name} must change and is not in the canonical form package fmt writes (two-space \
             indentation, one member per line, LF, a final newline); write it so once, in a \
             commit of its own"
        ),
    ))
}

fn canonical(value: &Value) -> LabResult<Vec<u8>> {
    canonical_json_bytes(value)
        .map_err(|error| LabError::usage(format!("a JSON document does not serialize: {error}")))
}

/// Writes `files` (by `/`-separated relative path) into the new directory `directory`.
fn write_files(directory: &Path, files: &BTreeMap<String, Vec<u8>>) -> LabResult<()> {
    create_dir(directory)?;
    for (relative, bytes) in files {
        let target = join_relative(directory, relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| tree_failed(parent, &error, IoOp::CreateDir))?;
        }
        write_new(&target, bytes).map_err(|error| tree_failed(&target, &error, IoOp::Write))?;
    }
    Ok(())
}

/// Copies the directory tree `from` into the new directory `to`; a link in it is refused, as the
/// loader refuses one.
fn copy_tree(from: &Path, to: &Path) -> LabResult<()> {
    create_dir(to)?;
    let entries = fs::read_dir(from)
        .map_err(|error| tree_failed(from, &error, IoOp::ReadDir))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|error| tree_failed(from, &error, IoOp::ReadDir))?;
    for source in entries {
        let Some(name) = source.file_name() else {
            continue;
        };
        let target = to.join(name);
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| tree_failed(&source, &error, IoOp::Metadata))?;
        if metadata.file_type().is_symlink() {
            return Err(refused(
                adopt(AdoptProblem::RepositoryInvalid).path(&source),
                format!("{} is a link", source.display()),
            ));
        }
        if metadata.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            let bytes =
                fs::read(&source).map_err(|error| tree_failed(&source, &error, IoOp::Read))?;
            write_new(&target, &bytes)
                .map_err(|error| tree_failed(&target, &error, IoOp::Write))?;
        }
    }
    Ok(())
}

fn create_dir(path: &Path) -> LabResult<()> {
    fs::create_dir(path).map_err(|error| tree_failed(path, &error, IoOp::CreateDir))
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// A required file of `--repo`: missing or unreadable is `repository_invalid`.
fn required_file(repo: &Path, name: &str) -> LabResult<Vec<u8>> {
    let path = repo.join(name);
    optional_file(&path, name)?
        .ok_or_else(|| repository_invalid(name, format!("{} is missing", path.display())))
}

/// An optional file of `--repo`: `None` when it is missing.
fn optional_file(path: &Path, name: &str) -> LabResult<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(refused(
            adopt(AdoptProblem::RepositoryInvalid)
                .entry(name)
                .caused_by(Outcome::from_io_error(&error, IoOp::Read).path(path)),
            format!("{}: {error}", path.display()),
        )),
    }
}

fn is_json(relative: &str) -> bool {
    Path::new(relative)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(bytes)
}

/// One JSON pointer segment (RFC 6901).
fn segment(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

/// A pointer within the key table's bound, cut at its last whole segment.
fn bounded_pointer(pointer: &str) -> String {
    const LIMIT: usize = 512;
    if pointer.len() <= LIMIT {
        return pointer.to_owned();
    }
    let mut end = LIMIT;
    while !pointer.is_char_boundary(end) {
        end -= 1;
    }
    match pointer[..end].rfind('/') {
        Some(cut) => pointer[..cut].to_owned(),
        None => String::new(),
    }
}

/// A `held` text: non-blank, at most 1024 bytes, no control character.
fn bounded_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= MAX_HELD_TEXT_BYTES
        && !text.chars().any(char::is_control)
}

fn text_value<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

/// The outcome a Lab error carries in its details, else the outcome of its code.
fn cause_of(error: &LabError) -> Outcome {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .and_then(|outcome| serde_json::from_value::<OutcomeEnvelope>(outcome.clone()).ok())
        .map(OutcomeEnvelope::into_outcome)
        .unwrap_or_else(|| Outcome::relayed(&error.code))
}

fn adopt(problem: AdoptProblem) -> Outcome {
    Outcome::new(ResourceToolingCode::AdoptRefused).adopt_problem(problem)
}

/// `outcome` with `package_id` where the key table's bound takes it.
fn named_package(outcome: Outcome, package_id: &str) -> Outcome {
    if package_id.len() <= 256 && !package_id.chars().any(char::is_control) {
        outcome.package_id(package_id)
    } else {
        outcome
    }
}

/// The Lab error of the refusal `outcome`.
fn refused(outcome: Outcome, message: String) -> LabError {
    LabError::new(
        LabErrorClass::UsageValidation,
        ResourceToolingCode::AdoptRefused.as_str(),
        message,
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

fn repository_invalid(entry: &str, message: String) -> LabError {
    refused(adopt(AdoptProblem::RepositoryInvalid).entry(entry), message)
}

fn decision_invalid(path: &Path, pointer: &str, message: &str) -> LabError {
    let outcome = adopt(AdoptProblem::DecisionInvalid).json_pointer(bounded_pointer(pointer));
    let outcome = if path.as_os_str().is_empty() {
        outcome
    } else {
        outcome.path(path)
    };
    refused(outcome, format!("the decision at {pointer:?}: {message}"))
}

/// A write of the check tree failed: nothing was written in the repository.
fn tree_failed(path: &Path, error: &io::Error, op: IoOp) -> LabError {
    write_failed(path, &[], error, op)
}

/// `adopt_write_failed`: the write at `path` failed after `done`.
fn write_failed(path: &Path, done: &[PathBuf], error: &io::Error, op: IoOp) -> LabError {
    let outcome = Outcome::new(ResourceToolingCode::AdoptWriteFailed).path(path);
    let outcome = if done.is_empty() {
        outcome
    } else {
        outcome.paths(done.iter())
    };
    let outcome = outcome.caused_by(Outcome::from_io_error(error, op).path(path));
    let listed = done
        .iter()
        .map(|done_path| done_path.display().to_string())
        .collect::<Vec<_>>();
    LabError::new(
        LabErrorClass::UsageValidation,
        ResourceToolingCode::AdoptWriteFailed.as_str(),
        format!(
            "{}: {error}; written or moved before: {listed:?}; nothing is removed",
            path.display()
        ),
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

fn absolute(path: &Path) -> LabResult<PathBuf> {
    std::path::absolute(path)
        .map_err(|error| LabError::usage(format!("{}: {error}", path.display())))
}

/// The volume `path` lies on ([`AdoptWorktree::volume`]): of the nearest part of `path` that
/// exists, its links resolved, the drive or network share on Windows and the device elsewhere. A
/// volume mounted on a folder of another is not told apart from it on Windows.
fn volume_of(path: &Path) -> io::Result<String> {
    let mut existing = path;
    while fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no part of {} exists", path.display()),
            )
        })?;
    }
    volume_name(&fs::canonicalize(existing)?)
}

#[cfg(windows)]
fn volume_name(resolved: &Path) -> io::Result<String> {
    use std::path::{Component, Prefix};
    let Some(Component::Prefix(prefix)) = resolved.components().next() else {
        return Err(io::Error::other(format!(
            "{} names no volume",
            resolved.display()
        )));
    };
    Ok(match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            format!("{}:", char::from(letter.to_ascii_uppercase()))
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
            r"\\{}\{}",
            server.to_string_lossy(),
            share.to_string_lossy()
        )
        .to_lowercase(),
        Prefix::DeviceNS(name) | Prefix::Verbatim(name) => name.to_string_lossy().to_lowercase(),
    })
}

#[cfg(unix)]
fn volume_name(resolved: &Path) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("device {}", fs::metadata(resolved)?.dev()))
}

#[cfg(not(any(windows, unix)))]
fn volume_name(resolved: &Path) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("the volume of {} cannot be told here", resolved.display()),
    ))
}
