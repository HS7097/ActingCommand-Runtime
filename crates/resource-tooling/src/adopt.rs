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
//!   directory, which holds the recordings, the kept copies and the check trees. Workflow #463
//!   S3-4: or, in place of a document, a held decision's reason and reference
//!   ([`AdoptDecision::Held`]), which are that document of the class `held` and are read as it is.
//! - **The checks**, every one before the first write, in this order: the repository, no leftover
//!   of an interrupted call; the recording; the package, admitted as `package preflight` admits
//!   it, its game and server those of `applications.json`; its `package_id` a directory name and
//!   not adopted before without `--replace`, with `--replace` no change below its directory the
//!   version control does not hold and the kept copy's place on the checkout's volume
//!   ([`AdoptWorktree`]); the decision, and one role for the package (no business rule beside a
//!   role another root file gives, no maintenance role by its uses alone beside its business
//!   rule, without a decision no two roles in the files adopt writes); the review (a class other
//!   than `held` needs a recording checked offline); no absolute local path in the package's
//!   text; the root files it must change in their canonical form; then the tree as it would be
//!   written, laid out in the checkout's own marked check tree `<state>/adopt-check/<checkout>/`
//!   (never through a link), bundled by `package bundle` with every root file the repository
//!   holds, its installed layout checked by `standard-pack check`, both composing the schedule
//!   with one instance per server; every outcome key the compiled schedule reads of the rule (the
//!   policy crate's `referenced_outcome_keys`: rules, pools, activity targets) and every outcome
//!   key the rule reads of another task declared by the package of the task read; last, a
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
//!   and moved back when the new one cannot take its place; `schedule.json` and `maintenance.json`,
//!   each only when it changes, both written as `<name>.part` first, then each read again and
//!   renamed only when it is still the file the call read, the file that gives the package its
//!   new role first, so that a call interrupted between the two renames leaves a `.part` the next
//!   call sees. A failure is `adopt_write_failed` naming where it stopped and what it had written.
//!   The check tree alone is the tool's own scratch, made whole under a temporary name of its
//!   call's own and renamed into place, and emptied at the start of each call when its marker is
//!   there, as is a temporary directory a crash left beside it that carries the marker.
//! - **One role as the composer reads it** (fix round 3): a rule is a business rule exactly when
//!   the composer judges it by its value (the policy crate's `is_judged_rule`), so a rule of a
//!   package a root file makes maintenance-role is part of its maintenance role, not a second one.
//! - **After an interruption** (fix round 3): each refusal that meets what an interrupted call
//!   left says what that call left and the steps that are taken from there; a root file's `.part`
//!   is trusted as that file whole only when it reads as it and is in the canonical form.
//! - **Fix round 4**: the advice to commit the call's new directory and run `--replace` says that
//!   `--replace` then needs a state directory on the checkout's volume, and names the way that
//!   needs none (the directory put back as it was); `pack_exists` beside a role another root file
//!   gives names that file and what the call accepts; every file renamed into place is flushed to
//!   the disk before the rename; a maintenance decision without a trigger adds no `maintenance`
//!   declaration to a rule the composer does not judge already.
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
//! - **The offline review and the trial** (Workflow #463 S3-12, register §1.445): the review's
//!   `recording_checked` (named `run_through` before) says that the recording was checked offline,
//!   nothing of a run on a device; `review.trial` is the latest trial record of the content the
//!   adoption writes (`passed`, `failed` or `none`), read by the trial's own reader from the state
//!   directory and matched by the trial's own content prefix. Two warnings refuse nothing:
//!   `enabling_needs_authorization` for a decision of a class that runs, `not_tried` while no
//!   passed trial of the content is on record. `next` names `ours/model/bindings.json` with the
//!   rule's two ways to a value while the adopted rule has none. The train of S3-12 and S3-14
//!   (register §1.445, "Train"): `review.trial` is the trial module's one summary
//!   (`trial_summary`, with `passed_on_record`), each record judged by `PackTrialRecord::passed`,
//!   which the status row answers too.
//! - **The cases** (Workflow #463 AC-1, `contracts/standard-pack-authoring.md`, "The cases"): with
//!   `--record-id`, the regression cases the recording's last stop wrote beside its package
//!   (`out/<package_id>.cases/<case>/`, #463 S2-10) become `regression/<game>/<package_id>.<case>/`
//!   of the checkout, byte for byte. Before the first write each is run by the one runner on the
//!   package as this adoption writes it (the check tree's copy of it, admitted once by the runner);
//!   a failing one refuses the adoption, a case not proven whole is taken and named; a destination
//!   that is no case an adoption took from a recording of this package refuses. With `--replace`
//!   the cases an earlier adoption took are moved as a set beside the replaced pack directory under
//!   the state directory. They are written last, each as its `.part` and renamed. The runner and
//!   the judgement below are the Lab's ([`AdoptLab`]), which this crate does not reach.
//! - **One judgement of the recording** (Workflow #463 S3-17): with `--record-id` the recording is
//!   judged by `record ready`'s one function ([`AdoptLab::recording_ready`]); a recording it does
//!   not call `packaged` is refused `recording_not_current` with its list of what is missing. The
//!   checks of the recording's own members stay beside it where the two differ: the form of its
//!   artifact and its package's file, and the members' check when that function cannot read the
//!   recording. The review (`recording_unreviewed`) stays whole: that function checks a packaged
//!   recording no further.
//! - **The recordings' own directory** (Workflow #463 AD-1, `contracts/standard-pack-authoring.md`,
//!   "The recordings' directory"): with `--record-id` the recording, its package, its cases and
//!   `record ready`'s judgement of it are read from the directory its source names
//!   ([`AdoptSource::Recording`]'s `session_state_dir`: `--session-state-dir`, or the state
//!   directory without it), and only read; everything adoption writes or moves (the check tree,
//!   the kept copies of a replaced pack directory and of a replaced case set) stays under the state
//!   directory, and the trial records are read there, as before.
//! - **The warnings of other packages** (Workflow #463 AD-1, "The warnings of other packages"):
//!   a warning that names another package of the root than the adopted one, by the one rule a
//!   readiness row's warnings are read by (`standard_pack_ready::with_packs_named`), is counted by
//!   its code in `other_packages_warnings`; every other warning (of the adopted package, or of no
//!   package) is answered whole in `warnings`. No check changes. Fix round 1: when the check
//!   tree's game root cannot be read again for it, the adoption is not refused, as the readiness
//!   is not by the same failure: every warning is answered whole, `other_packages_warnings` is
//!   absent and the warning `adopt_warnings_not_split` says why.
//! - **The stages and their times** (Workflow #463 S2b-1, `contracts/standard-pack-authoring.md`,
//!   "The stages and their times"): a call runs the stages of [`AdoptStage`], one after the other,
//!   in that order, under `--dry-run` too; the answer's `timings` gives the wall-clock milliseconds
//!   of each and the total, and a call that fails carries the same in its details with the stage it
//!   stopped in. The clock is read here, in the library (this crate reads `Instant` for its
//!   deadlines already), and [`AdoptWorktree::stage_started`] is told each stage as it starts.
//!   **One root check**: the readiness of the check tree's game root takes the warnings of the
//!   check of that root this call ran, after which nothing under the root was written, instead of
//!   checking it again (`standard_pack_ready::judge_checked_readiness`).
//! - **On a switched root** (Workflow #477 CV-2b, register §1.515,
//!   `contracts/standard-pack-authoring.md`, "On a switched root"): of a checkout whose
//!   `schedule.json` names `catalog_business`, the call reads its version 2 `pack-catalog.json`
//!   (missing or of version 1: `repository_invalid` caused by `catalog_missing` or
//!   `catalog_version_1`), reads the package's rule with the business part the catalog gives it,
//!   and writes every business member it would write into the rule into the catalog's part for
//!   that rule instead (`AdoptionCatalog::adopted`: the entry's
//!   own part for the package's one rule, else a per-rule part), the rule without them; a word
//!   `requires` gains goes before `catalog_business`. The check of the tree composes both files as
//!   they would be written, and the catalog is renamed into place right before `schedule.json`
//!   ([`root_order`]). A root that does not name `catalog_business` is adopted as before, byte for
//!   byte.

use crate::codes::{AdoptCasesState, AdoptProblem, ResourceToolingCode};
use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{check_standard_pack_root_with_catalog, root_issue_outcome};
use crate::standard_pack_ready::{judge_checked_readiness, with_packs_named};
use crate::standard_pack_trial::{
    TrialNotRead, TrialSummary, cause_of, content_hash_prefix_of, trial_summary,
};
use crate::{
    PackReadiness, PackageBundleRequest, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackReadyRequest, StandardPackRootFiles, package_bundle,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome};
use actingcommand_contract::{
    AdoptionCatalog, BundleMaintenance, CatalogAdoption, LabError, LabErrorClass, LabResult,
    MaintenanceUse, SCHEDULE_RULE_BUSINESS_MEMBERS, SCHEDULE_RULE_FIXED_VALUE_MEMBER,
    SCHEDULE_RULE_FIXED_VALUE_ORIGIN_MEMBER, SCHEDULE_RULE_MAINTENANCE_MEMBER,
    SCHEDULE_RULE_REALIZATION_MEMBER, SCHEDULE_VALUE_ORIGIN_MODEL, STANDARD_PACK_CATALOG_FILE,
    STANDARD_PACK_CATALOG_SCHEMA_VERSION_V2, STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    STANDARD_PACK_GOALS_FILE, STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_LOADING_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_NOTICES_DIRECTORY, STANDARD_PACK_PAGES_FILE,
    STANDARD_PACK_SCHEDULE_FILE, STANDARD_PACK_SCHEDULE_VERSION, ScheduleItem, ScheduleRule,
    StandardPackClassification, StandardPackProblem, StandardPackRegistry, StandardPackRoot,
    StandardPackRootIssue, StandardPackSchedule, canonical_json_bytes,
    package_id_names_one_path_segment, parse_standard_pack_interrupts,
    parse_standard_pack_navigation, parse_standard_pack_pages, parse_standard_pack_schedule,
    reserved_package_id, standard_pack_maintenance_packages,
};
use actingcommand_execution_kernel::{PreparedContainedTask, TaskPackageDescriptor};
use actingcommand_pack_containment::Containment;
use actingcommand_policy::{
    BENEFIT_FIXED_VALUE_SUBSYSTEM, BenefitGap, CATALOG_BUSINESS_SUBSYSTEM,
    CONSUMPTION_FACT_SUBSYSTEM, CompiledCatalog, FACT_REFERENCE_SUBSYSTEM, FACT_REFERENCE_TYPE,
    FACT_UNKNOWN_PROBE_SUBSYSTEM, MAX_QUEUE_WAIT_MEMBER, POOL_FACT_REFERENCE_SUBSYSTEM, PolicyCode,
    QUEUE_WAIT_SUBSYSTEM, SCHEDULE_RULE_LIMITED_MEMBER, is_judged_rule, is_manual_request,
    standard_pack_schedule_takes_catalog,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
/// In a check tree: the repository's pack directories with the adopted package's as it would be
/// written (Workflow #463 AC-1: the package its cases are run on).
const CHECK_PACKS_ROOT: &str = "packs-root";
/// The file adopt writes into the check tree of a checkout, `<state>/adopt-check/<checkout>/`, to
/// mark it as its own scratch: adopt empties a check tree only when this file is in it, and holds
/// it locked for the whole call, so that two calls on one checkout never share a tree at once.
pub(crate) const CHECK_TREE_MARKER: &str = ".adopt-check-tree";
/// Of a check tree's name: the characters of the checkout's last segment kept, then the digest.
const CHECK_TREE_STEM_CHARS: usize = 32;
const CHECK_TREE_DIGEST_CHARS: usize = 16;
/// A check tree is made whole beside its place, as `<checkout>.new-<process>-<ms>-<count>/` with
/// its marker, and renamed into place, so that no crash leaves it unmarked. The process, the
/// call's time and a count of the process name it, and a name a directory already holds is never
/// taken (the next count is), so that no two calls, of one process or of two, share one; a call
/// that holds the tree clears such a directory a crash left beside it when it carries the marker
/// ([`clear_staged_leftovers`]).
const CHECK_TREE_STAGING: &str = "new";
static CHECK_TREE_STAGED: AtomicU64 = AtomicU64::new(0);
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
const POOLS: &str = "pools";
const ACTIVITY: &str = "activity";
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
/// Workflow #463 AC-1: beside a recording's package, the directory of its regression cases,
/// `out/<package_id>.cases/` (#463 S2-10); the file of a case; and the members of a case's
/// `source` that `record stop` writes, `{record_id, step}`, `{record_id, final: true}` or
/// `{record_id, ending}`, by which a case is told to be a recording's.
const CASES: &str = "cases";
const CASE_FILE: &str = "case.json";
const CASE_SOURCE: &str = "source";
const SOURCE_RECORD_ID: &str = "record_id";
const SOURCE_STEP: &str = "step";
const SOURCE_FINAL: &str = "final";
const SOURCE_ENDING: &str = "ending";
/// Workflow #463 AC-1: the longest name of a directory entry a file system takes, in bytes; a
/// case's directory with its `.part` stays within it.
const NAME_MAX_BYTES: usize = 255;
/// Workflow #463 S3-12: where a rule's fixed value is registered, relative to `--repo` (the model's
/// `acmodel.bindings.v1`): by a value binding, which the model's writer `acmodel bindings apply`
/// writes into the rule with `fixed_value_origin: "model"`, or by a hand entry of a number written
/// by hand. Named in `next` whenever the adopted rule has no value yet, whether the checkout holds
/// it or not.
const BINDINGS_FILE: &str = "ours/model/bindings.json";

/// Where the package to adopt comes from.
#[derive(Debug, Clone)]
pub enum AdoptSource {
    /// A stopped recording: its `record_id` and its Lab directory,
    /// `<recordings>/record-artifacts/<record>/lab`, which holds `recording.json` and `out/`.
    /// Workflow #463 AD-1: `session_state_dir` is the directory the recordings are read from
    /// (`--session-state-dir`, or the state directory when it is not given), where `record ready`'s
    /// function reads the recording by its id; adoption only reads it.
    Recording {
        record_id: String,
        lab_dir: PathBuf,
        session_state_dir: PathBuf,
    },
    /// A package by its path: a content directory, a `.zip` or a `.json` container.
    Package(PathBuf),
}

/// Where the adoption's decision comes from (Workflow #463 S3-4).
#[derive(Debug, Clone)]
pub enum AdoptDecision {
    /// `--decide <decision.json>`: the decision document at this path.
    Document(PathBuf),
    /// `--held-reason <text> [--held-reference <text>]`: exactly the decision `{schema_version,
    /// class: "held", held: {reason, reference?}}`, built as that document and read by the
    /// reader of a document, so that every check and every byte written is the document's.
    Held {
        /// The held entry's `reason` text (the name is the one the decision's reader uses for
        /// it; the outcome guard keeps the word `reason` for typed reasons).
        rationale: String,
        reference: Option<String>,
    },
}

/// Workflow #464 P1: `actinglab standard-pack adopt --repo <checkout> (--record-id <id> |
/// --package <path>) [--decide <decision.json> | --held-reason <text> [--held-reference <text>]]
/// [--replace] [--state-dir <dir>]` under the global `--dry-run`. Workflow #463 AD-1: and
/// `[--session-state-dir <dir>]` with `--record-id`, carried by [`AdoptSource::Recording`].
#[derive(Debug, Clone)]
pub struct StandardPackAdoptRequest {
    pub repo: PathBuf,
    pub source: AdoptSource,
    pub decision: Option<AdoptDecision>,
    pub replace: bool,
    pub dry_run: bool,
    /// The Lab's state directory: the kept copies, the check trees and the trial records.
    /// Workflow #463 AD-1: a recording is read from its source's own directory, which is this one
    /// unless `--session-state-dir` names another.
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

    /// Renames `from` to `to` ([`fs::rename`]): every rename of the adoption's writes (the pack
    /// directory, a replaced directory moved away and back, the root files) goes through it. A
    /// test sets its own to make one fail, or to change a root file in between as another writer
    /// would.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    /// Workflow #463 AC-1: the Lab's own parts adopt calls ([`AdoptLab`]): `record ready`'s one
    /// function and the one runner of a regression case.
    fn lab(&self) -> &dyn AdoptLab;

    /// Workflow #463 S2b-1: told as each stage of the adoption starts, in the order they run
    /// ([`AdoptStage`]), once the stage before it has ended. The times are measured by adopt
    /// itself; the product's does nothing here. A test sets its own to see the stages, or to
    /// change the check tree between two of them as another writer would.
    fn stage_started(&self, _stage: AdoptStage) {}
}

/// Workflow #463 AC-1 (`contracts/standard-pack-authoring.md`, "The cases" and "One judgement of
/// the recording"): what adopt asks of the Lab, whose crate this one does not reach (the Lab's
/// crate depends on this one): the one judgement of what a recording still lacks (`record
/// ready`'s function, #463 S3-17) and the one runner of a regression case
/// (`contracts/resource-regression-case.md`). actinglab gives the Lab's own; nothing here judges
/// a recording or runs a case a second way.
pub trait AdoptLab {
    /// `record ready`'s answer for the recording `record_id` of the state directory `state_dir`
    /// (Workflow #463 AD-1: the directory the recordings are read from, the source's
    /// `session_state_dir`), whatever instance it is of, by its one function called with no
    /// option given; when that
    /// function answers none, its refusal, a recording the Lab cannot read whole told apart from
    /// every other one ([`ReadyRefused`], fix round 1).
    fn recording_ready(
        &self,
        state_dir: &Path,
        record_id: &str,
    ) -> Result<RecordingReady, ReadyRefused>;

    /// The cases `cases`, each run by the one runner on the package directory `package_dir`,
    /// which it admits once as the runner admits a package: each case's directory read as
    /// `resource regression` reads a case of a checkout (its `case.json` by the one reader, the
    /// scene it names), one run of each in the order given. Fix round 1: with the cases that did
    /// not pass as `resource regression` lists them, by its one rule (the failed ones first, at
    /// most its bound, each row without a check's `detail`); a refusal when that listing does not
    /// serialize.
    fn run_cases(&self, package_dir: &Path, cases: &[AdoptCaseAt]) -> LabResult<AdoptCasesRun>;
}

/// Workflow #463 AC-1 fix round 1 (the coordinator's ruling): why `record ready`'s function
/// answered nothing for the recording, as adopt decides by it.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadyRefused {
    /// The Lab cannot read the recording whole: its refusal carries the word
    /// `lab_recording_unreadable` (a `recording.json` that does not decode as the Lab writes it,
    /// made by hand or damaged). The check of the recording's own members judges it, as before
    /// #463 S3-17, so that what adoption accepts and refuses does not move for such a recording.
    Unreadable(LabError),
    /// Every other refusal of that function (the state not read, `lab_recording_mismatch`, an
    /// answer that does not serialize): the adoption is refused (`recording_not_judged`), that
    /// refusal its cause, nothing written. A failure on the Lab's side is never a pass.
    Failed(LabError),
}

/// Workflow #463 AC-1 fix round 1: a case of the recording's set to run ([`AdoptLab::run_cases`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptCaseAt {
    /// Where the adoption writes it, `regression/<game>/<package_id>.<case>` relative to `--repo`:
    /// the path its row names, as `resource regression` names a case of the checkout.
    pub path: String,
    /// Its directory in the recording's set, `out/<package_id>.cases/<case>/`.
    pub directory: PathBuf,
}

/// Workflow #463 AC-1 fix round 1: the cases as the one runner ran them
/// ([`AdoptLab::run_cases`]).
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptCasesRun {
    /// One run of each case, in the order given.
    pub runs: Vec<AdoptCaseRun>,
    /// How many of them did not pass (`failed` and `partial`), as `resource regression` counts
    /// its `failed_total`.
    pub not_passed: usize,
    /// The rows of the first of those, as `resource regression` lists them: the failed ones
    /// first, each group in the order given, at most its bound (`ROWS_LISTED_MAX`), each `{path,
    /// id, package_id, verdict, checks, warnings?}` without a check's `detail`.
    pub listed: Vec<Value>,
}

/// Workflow #463 S3-17: `record ready`'s answer for a recording, as adopt reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingReady {
    /// Its status is `packaged`: stopped, its package written and nothing changed since.
    pub packaged: bool,
    /// The answer as `record ready` serializes it: `status`, `missing` (each item as that function
    /// gives it), `missing_total`, `warnings`, `checks`, `checks_after`, `sources`.
    pub answer: Value,
}

/// Workflow #463 AC-1: the one runner's verdict of a case, as adopt decides by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptCaseVerdict {
    /// `passed`: every check ran and passed.
    Passed,
    /// `failed`: a check failed; the adoption is refused.
    Failed,
    /// `partial` or `not_run`: taken, and named with the runner's word for its verdict
    /// (`verdict`) and its words for the checks that did not run, each once (`reasons`).
    NotProven {
        verdict: String,
        reasons: Vec<String>,
    },
}

/// Workflow #463 AC-1: one case as the one runner ran it. Fix round 1: a failing case is listed
/// as `resource regression` lists it ([`AdoptCasesRun::listed`]), so the run carries no answer of
/// its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptCaseRun {
    pub verdict: AdoptCaseVerdict,
    /// The case's files as they were read and run, each by its name in the case's directory: its
    /// `case.json` and, when the reader read it, the scene it names. Written byte for byte.
    pub files: Vec<(String, Vec<u8>)>,
}

/// Workflow #463 S3-17: `record ready`'s function did not read the recording (its refusal), so the
/// check of the recording's own members judged it, as before that slice. Fix round 1: only for a
/// recording the Lab cannot read whole ([`ReadyRefused::Unreadable`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadyNotRead {
    /// The function's refusal, with its code.
    pub outcome: Outcome,
    /// The same in words.
    pub text: String,
}

/// Workflow #463 AC-1: a case taken that the one runner did not prove whole.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseNotProven {
    /// The case, by the name of its directory in the recording's set.
    pub case: String,
    /// `partial` or `not_run`, the runner's own word.
    pub verdict: String,
    /// The runner's words for the checks that did not run (`ocr_unavailable`, `frames_required`,
    /// `not_evaluated`), each once.
    pub reasons: Vec<String>,
}

/// Workflow #463 AC-1 (`contracts/standard-pack-authoring.md`, "The cases"): the regression cases
/// of the adoption.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdoptCases {
    /// `taken`, `none_found` or `package_path` (the vocabulary `adopt_cases`).
    pub state: AdoptCasesState,
    /// Where they were read from: the recording's `out/<package_id>.cases/`; with `taken` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The directories written, relative to `--repo`, in the order they were renamed into place
    /// (also in `written`); empty under `--dry-run`.
    pub written: Vec<String>,
    /// Under `--dry-run`: the directories it would write (also in `would_write`).
    pub would_write: Vec<String>,
    /// The cases an earlier adoption of the package took from a recording, relative to `--repo`,
    /// which this one moved (under `--dry-run`: would move) to `kept` before it wrote its own.
    pub replaced: Vec<String>,
    /// Where the replaced set was moved, `<state>/adopt/<package_id>.cases.old-<ms>/`; absent when
    /// none was, and under `--dry-run`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    /// The cases taken that the runner did not prove whole, with its words.
    pub not_proven: Vec<CaseNotProven>,
}

impl Serialize for AdoptCasesState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
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

/// Why the Runtime's evaluator dispatches the adopted business rule without judging its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotJudgedBecause {
    /// A follower of a chain: it rides with its chain's head, whatever its own value.
    Follower,
    /// A manual request: its trigger requires an instruction of the `session.` family, and it
    /// runs when the instruction is given, whatever its value.
    ManualRequest,
}

/// The adopted business rule is dispatched without its value being judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValueNotJudged {
    pub because: NotJudgedBecause,
    /// The same in words.
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
/// checked offline when its cross-check passed, its first decision was evaluated and no step needs
/// review), and (Workflow #463 S3-12) what the trial records say of the content adopted.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// The steps whose click the recording marked `needs_review`, by their step number; #463 KR-1
    /// fix round 1: a key step whose replaced press it marked so.
    pub steps_needing_review: Vec<u64>,
    /// Workflow #463 S3-12 (the rule of ruling 2 on #464, named for what it is): the recording was
    /// checked offline, a recording's, its cross-check passed, its first decision evaluated and no
    /// step needing review. It says nothing of a run on a device; that is [`Self::trial`].
    pub recording_checked: bool,
    /// Workflow #463 S3-12: the trials of the content this adoption writes on record in the state
    /// directory; absent only beside [`Self::trial_not_read`]. The train of S3-12 and S3-14
    /// (ruling R2): the trial module's one summary, which the status row answers too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial: Option<TrialSummary>,
    /// Workflow #463 S3-12: why the trial records of the package were not read; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial_not_read: Option<TrialNotRead>,
    /// Workflow #463 S3-17: with `--record-id`, why `record ready`'s function did not read the
    /// recording, which the check of its own members then judged; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_not_read: Option<ReadyNotRead>,
}

/// One of the repository's own items adopt does not write, by its path relative to `--repo`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdoptNext {
    pub path: String,
    /// Workflow #463 S3-12: for `ours/model/bindings.json`, how the adopted rule's fixed value
    /// gets there; absent for every other item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_value: Option<NextFixedValue>,
}

/// Workflow #463 S3-12: the two ways a fixed value of the adopted rule is registered in
/// `ours/model/bindings.json`, each the entry as that file writes it, the texts adopt cannot know
/// left blank.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextFixedValue {
    /// The adopted rule.
    pub rule_id: String,
    /// An entry of `bindings`: the model's writer `acmodel bindings apply` writes the value of the
    /// model's `exports.json` it names (`value`, blank) into the rule's `fixed_value_milli`, with
    /// `fixed_value_origin: "model"`.
    pub value_binding: NextValueBinding,
    /// An entry of `hand`: registers a number written by hand into the rule's
    /// `fixed_value_milli` (adopt `--replace --decide` with `{fixed_value_milli, origin: "pack"}`),
    /// how it was had (`status`) and where it comes from (`ref`), both blank.
    pub hand_entry: NextHandEntry,
}

/// Workflow #463 S3-12: a value binding of `bindings.json`, `{value, file, rule_id, path}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextValueBinding {
    /// The id of a value of the model's `exports.json`: blank, the model's to name.
    pub value: String,
    /// `schedule.json`.
    pub file: String,
    pub rule_id: String,
    /// `/fixed_value_milli`.
    pub path: String,
}

/// Workflow #463 S3-12: a hand entry of `bindings.json`, `{rule_id, status, ref}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextHandEntry {
    pub rule_id: String,
    /// How the number was had: blank, the writer's to say.
    pub status: String,
    /// Where it comes from (the member `ref`): blank, the writer's to say.
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Workflow #463 S3-5: the readiness function refused the check tree's game root (or answered no
/// row of the adopted pack), so the pack's readiness was not judged; the adoption is the same
/// without it.
#[derive(Debug, Clone, Serialize)]
pub struct ReadinessNotJudged {
    /// The function's refusal, with its code as the function answered it.
    pub outcome: Outcome,
    /// The same in words.
    pub text: String,
}

/// Workflow #463 AD-1 (`contracts/standard-pack-authoring.md`, "The warnings of other packages"):
/// the warnings of the check that name another package of the root than the adopted one, which
/// the answer counts instead of giving each whole.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OtherPackagesWarnings {
    /// How many there are.
    pub total: usize,
    /// How many there are of each code, by the code.
    pub counts: BTreeMap<String, usize>,
}

/// Workflow #463 S2b-1 (`contracts/standard-pack-authoring.md`, "The stages and their times"): the
/// stages of one adoption, a closed list, in the order a call runs them; a dry run runs each too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptStage {
    /// The checkout read: `applications.json`, `maintenance.json`, `schedule.json`, what an
    /// interrupted call left.
    Repository,
    /// With `--record-id`, the recording read and judged by `record ready`'s one function, its
    /// package and its cases found; with `--package`, nothing but its path.
    Recording,
    /// The package admitted as `package preflight` admits it, and its files read.
    Package,
    /// The checks before the check tree (the package's game, server and id, an earlier adoption,
    /// `--replace`'s version control and volumes, the decision and the role, the review and the
    /// trial records, local paths) and the plan of what is written.
    Checks,
    /// The checkout's check tree made, or locked and emptied of the earlier call's scratch.
    CheckTree,
    /// The checkout's other packages copied into the check tree, the package and the root files
    /// adopt changes written beside them.
    CopyPackages,
    /// `package bundle` of the check tree.
    Bundle,
    /// The bundle's installed layout copied into the check tree.
    InstalledTree,
    /// `standard-pack check` of that installed layout: the one root check of the call.
    RootCheck,
    /// The outcome keys the schedule reads and the business rule's value.
    RuleChecks,
    /// The regression cases run on the package as written, or, taking none, the checkout's cases
    /// of the package read.
    Cases,
    /// The coverage, the value's wait and the warnings, told apart by the package they name.
    Warnings,
    /// The writes; under `--dry-run`, the list of what would be written.
    Write,
    /// The adopted pack's readiness on the check tree's game root.
    Readiness,
}

/// Workflow #463 S2b-1: the wall-clock time of one stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdoptStageTime {
    pub stage: AdoptStage,
    /// From the stage's start to the next one's, or to the end of the call.
    pub ms: u64,
}

/// Workflow #463 S2b-1: the answer's `timings`, and a failed call's `details.timings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdoptTimings {
    /// Each stage that ran, in the order it ran.
    pub stages: Vec<AdoptStageTime>,
    /// The whole call, from its start to its answer.
    pub total_ms: u64,
    /// A failed call's: the stage it stopped in, the last of `stages`; absent in an answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_in: Option<AdoptStage>,
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
    /// The package's rule as `schedule.json` holds it after the adoption (Workflow #477 CV-2b: of
    /// a switched root without its business part, which `catalog_entry` holds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<Value>,
    /// Its `held` entry, for the class `held`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<Value>,
    /// Its `maintenance.json` entry, for the class `maintenance` with `uses`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<Value>,
    /// Workflow #477 CV-2b: of a switched root, the package's entry of `pack-catalog.json` after
    /// the adoption, its rules' business part in it; absent for any other root, and for a package
    /// the file lists no entry for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_entry: Option<Value>,
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
    /// For a business rule the evaluator judges whose value is not known (no fixed value and no
    /// effect, an entry or a pool it reads with the pending marker or without its valuation): it
    /// is not dispatched until a value is given; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waits_for_value: Option<ValueWait>,
    /// For a business rule the evaluator does not judge by its value (a follower of a chain, a
    /// manual request): why it is dispatched whatever its value; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_not_judged: Option<ValueNotJudged>,
    /// Every warning of the check, the game's included, each once. Workflow #463 AD-1: but each
    /// that names another package of the root, counted in `other_packages_warnings` instead. Fix
    /// round 1: every one, then `adopt_warnings_not_split`, when the root could not be read again
    /// to tell which package each names.
    pub warnings: Vec<Outcome>,
    /// Workflow #463 AD-1: the warnings of the check that name another package of the root than
    /// the adopted one, counted by code. Fix round 1: absent beside `adopt_warnings_not_split`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_packages_warnings: Option<OtherPackagesWarnings>,
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
    /// Workflow #463 S3-5: the adopted pack's row of the readiness of the check tree's game root
    /// `installed/<game>/`, as the readiness function answers it there with no installation (the
    /// row `standard-pack ready` answers for that root), judged after every check passed, under
    /// `--dry-run` too; absent only beside `readiness_not_judged`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness: Option<PackReadiness>,
    /// Workflow #463 S3-5: why the readiness was not judged, when the readiness function refused
    /// the check tree's game root or answered no row of the pack; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness_not_judged: Option<ReadinessNotJudged>,
    /// The repository's own items adopt does not write (a regression case, the census row, the
    /// catalog section), each by its path when the checkout holds it; Workflow #463 S3-12: and
    /// `ours/model/bindings.json` with the two ways of the rule's value when the adopted rule has
    /// no value yet. Workflow #463 AC-1: a regression case only when the adoption took none.
    pub next: Vec<AdoptNext>,
    /// Workflow #463 AC-1: the regression cases the adoption took from its recording, or the word
    /// why it took none.
    pub cases: AdoptCases,
    /// Workflow #463 S2b-1: the wall-clock milliseconds of each stage, in order, and the total.
    pub timings: AdoptTimings,
}

/// Workflow #463 S2b-1: the member of a failed call's details that holds its [`AdoptTimings`].
const TIMINGS_MEMBER: &str = "timings";

/// Workflow #463 S2b-1: the wall clock of one adoption's stages. They follow one another: each runs
/// from the moment it is entered to the moment the next one is, or the call ends.
struct Stages<'a> {
    worktree: &'a dyn AdoptWorktree,
    started: Instant,
    done: Vec<AdoptStageTime>,
    running: Option<(AdoptStage, Instant)>,
}

impl<'a> Stages<'a> {
    fn start(worktree: &'a dyn AdoptWorktree) -> Self {
        Self {
            worktree,
            started: Instant::now(),
            done: Vec::new(),
            running: None,
        }
    }

    /// Ends the running stage and starts `stage`, told to the worktree first.
    fn enter(&mut self, stage: AdoptStage) {
        self.end();
        self.worktree.stage_started(stage);
        self.running = Some((stage, Instant::now()));
    }

    fn end(&mut self) {
        if let Some((stage, since)) = self.running.take() {
            self.done.push(AdoptStageTime {
                stage,
                ms: millis_since(since),
            });
        }
    }

    /// The answer's `timings`: the running stage ended, every stage that ran and the total.
    fn answer(&mut self) -> AdoptTimings {
        self.end();
        AdoptTimings {
            stages: self.done.clone(),
            total_ms: millis_since(self.started),
            stopped_in: None,
        }
    }

    /// `error`, the failure of the running stage, with the timings in its details: the stages
    /// that ran, the last the one it stopped in. Every refusal of adoption carries its details as
    /// an object or none; details that are no object would be left as they are.
    fn stopped(&mut self, mut error: LabError) -> LabError {
        let stopped_in = self.running.map(|(stage, _)| stage);
        let mut timings = self.answer();
        timings.stopped_in = stopped_in;
        let mut details = match error.details.take() {
            Some(Value::Object(details)) => details,
            None => Map::new(),
            Some(other) => {
                error.details = Some(other);
                return error;
            }
        };
        details.insert(TIMINGS_MEMBER.to_owned(), json!(timings));
        error.details = Some(Value::Object(details));
        error
    }
}

/// The whole milliseconds since `since`.
fn millis_since(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Workflow #464 P1: `actinglab standard-pack adopt` (module doc). Workflow #463 S2b-1: each stage
/// timed, a failure answered with the times of the stages that ran.
pub fn standard_pack_adopt(
    request: StandardPackAdoptRequest,
    worktree: &dyn AdoptWorktree,
) -> LabResult<StandardPackAdoptResponse> {
    let mut stages = Stages::start(worktree);
    adopt_in_stages(request, worktree, &mut stages).map_err(|error| stages.stopped(error))
}

/// The adoption, each of its stages entered in `stages` as it starts.
fn adopt_in_stages(
    request: StandardPackAdoptRequest,
    worktree: &dyn AdoptWorktree,
    stages: &mut Stages<'_>,
) -> LabResult<StandardPackAdoptResponse> {
    stages.enter(AdoptStage::Repository);
    let repo = absolute(&request.repo)?;
    let state = absolute(&request.state_dir)?;
    let root = Repository::read(&repo)?;
    // Workflow #463 S3-17: a recording is judged by `record ready`'s one function; Workflow #463
    // AC-1: with the cases its last stop wrote beside its package. Workflow #463 AD-1: all of it
    // read from the directory the source names, which nothing here writes.
    stages.enter(AdoptStage::Recording);
    let (locator, record_id, recorded, recordings) = match &request.source {
        AdoptSource::Recording {
            record_id,
            lab_dir,
            session_state_dir,
        } => {
            let recordings = absolute(session_state_dir)?;
            let recorded = recorded_package(record_id, lab_dir, &recordings, worktree.lab())?;
            (
                recorded.package.clone(),
                Some(record_id.clone()),
                Some(recorded),
                Some(recordings),
            )
        }
        AdoptSource::Package(path) => (absolute(path)?, None, None, None),
    };
    stages.enter(AdoptStage::Package);
    let package = Package::read(&locator)?;
    stages.enter(AdoptStage::Checks);
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
            interrupted(&repo, &root.leftovers),
        ));
    }
    // Workflow #463 AC-1: when this adoption takes cases, what the checkout holds of the package's
    // cases: a case's `.part` an interrupted adoption left, and the set an earlier adoption took
    // from a recording.
    let regression_relative = format!("{REGRESSION_DIRECTORY}/{}", root.game);
    let regression_dir = repo.join(REGRESSION_DIRECTORY).join(&root.game);
    let taken = recorded.as_ref().and_then(|read| read.cases.as_ref());
    let checkout_cases = match taken {
        Some(_) => Some(CheckoutCases::read(
            &regression_dir,
            &regression_relative,
            &package_id,
        )?),
        None => None,
    };
    if let Some(part) = checkout_cases
        .as_ref()
        .and_then(|cases| cases.parts.first())
    {
        let entry = format!("{regression_relative}/{part}");
        return Err(refused(
            named_package(adopt(AdoptProblem::PackExists), &package_id).entry(&entry),
            case_part_left(&entry, &package_id),
        ));
    }
    let pack_exists = fs::symlink_metadata(&pack_dir).is_ok();
    if !request.replace && (pack_exists || existing.any()) {
        return Err(refused(
            existing.pointing(adopt(AdoptProblem::PackExists), pack_exists, &pack_relative),
            existing.adopted_before(&root, &package_id, pack_exists),
        ));
    }
    if !request.replace
        && let Some(earlier) = checkout_cases
            .as_ref()
            .and_then(|cases| cases.recorded.first())
    {
        let entry = format!("{regression_relative}/{earlier}");
        return Err(refused(
            named_package(adopt(AdoptProblem::PackExists), &package_id).entry(&entry),
            format!(
                "{entry} is a regression case of {package_id} that an earlier adoption took from a \
                 recording (its case.json names the package and its source a recording); give \
                 --replace to adopt the package again, which moves the earlier set of its cases \
                 to the state directory's adopt/; nothing was written"
            ),
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
                    "; the recording may stay where it is: name its directory by \
                     --session-state-dir (adopt only reads it) beside that --state-dir, or give \
                     its package by its path instead of --record-id: --package {}",
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
    // Workflow #463 AC-1: the earlier set of the package's cases is moved as the replaced pack
    // directory is, beside it under the state directory: its kept place free and on the
    // checkout's volume.
    let cases_kept = state
        .join(KEPT_DIRECTORY)
        .join(format!("{package_id}.{CASES}.old-{}", request.at_unix_ms));
    if request.replace
        && let Some(earlier) = checkout_cases
            .as_ref()
            .and_then(|cases| cases.recorded.first())
    {
        if fs::symlink_metadata(&cases_kept).is_ok() {
            return Err(refused(
                adopt(AdoptProblem::PackExists).path(&cases_kept),
                format!(
                    "the kept place of the earlier cases {} is taken",
                    cases_kept.display()
                ),
            ));
        }
        same_volume(
            worktree,
            &regression_dir.join(earlier),
            &cases_kept,
            &format!("{regression_relative}/{earlier}"),
        )?;
    }

    // The decision; without one, the class of the earlier adoption. Workflow #463 S3-4: a held
    // decision's two texts are that document, handed to the document's own reader.
    let decision = match &request.decision {
        Some(AdoptDecision::Document(path)) => {
            Some(Decision::read(&absolute(path)?, &root.schedule)?)
        }
        Some(AdoptDecision::Held {
            rationale,
            reference,
        }) => Some(Decision::of_document(
            held_document(rationale, reference.as_deref()),
            Path::new(""),
            &root.schedule,
        )?),
        None => None,
    };
    let class = match (
        &decision,
        existing.class(&root.schedule, &root.classification),
    ) {
        (Some(decision), _) => decision.class,
        (None, Some(class)) => class,
        (None, None) => {
            return Err(refused(
                adopt(AdoptProblem::DecisionInvalid).json_pointer(""),
                "a first adoption needs --decide <decision.json>, or --held-reason <text> to \
                 adopt the package as held"
                    .to_owned(),
            ));
        }
    };
    // One role (the decision's class decides): adopt writes `schedule.json` and
    // `maintenance.json` only, so a maintenance role another root file gives cannot be ended here.
    // A business rule beside it would never be judged; a hold beside it keeps the package from
    // scheduling, so a package that file names and that is not run through is adopted as held.
    if let Some(decision) = &decision
        && decision.class == AdoptClass::Business
        && let Some(file) = root.other_role_file(&package_id)
    {
        return Err(decision.invalid(
            "/class",
            &format!(
                "{file} gives {package_id} a maintenance role, which adopt does not write and \
                 beside which a business rule is never judged; take it out of {file} first, or \
                 adopt the package as maintenance, or as held"
            ),
        ));
    }
    // A maintenance role by its uses alone never turns the package's business rule into a
    // maintenance rule. A business rule is one the composer judges by its value (its own
    // `is_judged_rule`): a rule of a package a root file makes maintenance-role is not judged, and
    // is part of that role already.
    if let Some(decision) = &decision
        && decision.class == AdoptClass::Maintenance
        && decision.trigger.is_none()
        && decision.uses.is_some()
        && let Some(index) = existing.rule
        && let Some(rule) = root.schedule.rules.get(index)
        && is_judged_rule(rule, &root.classification)
    {
        return Err(refused(
            named_package(adopt(AdoptProblem::BusinessRuleInTheWay), &package_id)
                .entry(STANDARD_PACK_SCHEDULE_FILE)
                .json_pointer(format!("/{RULES}/{index}")),
            format!(
                "the decision gives {package_id} a maintenance role by its uses alone, and \
                 schedule.json holds its business rule {} at /{RULES}/{index} (the composer \
                 judges it by its value: it declares no maintenance and no root file gives the \
                 package a maintenance role), which adopt does not turn into a maintenance rule; \
                 take that rule out of schedule.json first, or give the decision the trigger of a \
                 maintenance rule; nothing was written",
                rule.id
            ),
        ));
    }
    // Without a decision nothing decides a role, so two roles in the files adopt writes (an
    // adoption interrupted between its two renames leaves them so) are told and not kept.
    if decision.is_none()
        && let Some((entry_index, message)) = existing.two_roles(&package_id)
    {
        return Err(refused(
            named_package(adopt(AdoptProblem::RepositoryInvalid), &package_id)
                .entry(MAINTENANCE_FILE)
                .json_pointer(format!("/{entry_index}")),
            message,
        ));
    }

    // The review (ruling 2): a class other than `held` needs a recording checked offline.
    // Workflow #463 S3-12: and the latest trial of the content this adoption writes, read from the
    // trial records of the state directory, which the trial keeps by the same content prefix; it
    // refuses nothing. The train of S3-12 and S3-14 (rulings R1, R2): the trial module's one
    // summary, and `not_tried` while no record of this content passed.
    let mut review = package.review();
    if review.record_id.is_none() {
        review.record_id = record_id;
    }
    // Workflow #463 S3-17: the recording that `record ready`'s function did not read, judged by the
    // check of its own members instead, says so.
    review.ready_not_read = recorded
        .as_ref()
        .and_then(|read| read.ready_not_read.clone());
    let content = content_hash_prefix_of(&package.files);
    let not_tried = match trial_summary(&state, &package_id, &content) {
        Ok(trial) => {
            let untried = !trial.passed_on_record;
            review.trial = Some(trial);
            untried
        }
        Err(not_read) => {
            review.trial_not_read = Some(not_read);
            false
        }
    };
    if class != AdoptClass::Held && !review.recording_checked {
        let outcome = named_package(adopt(AdoptProblem::RecordingUnreviewed), &package_id);
        return Err(refused(
            outcome.clone(),
            format!(
                "the recording of {package_id} was not checked offline (cross-check {:?}, first \
                 decision evaluated {}, steps needing review {:?}); a class other than held needs \
                 a recording checked offline: adopt it as held",
                review.cross_check, review.first_decision_evaluated, review.steps_needing_review
            ),
        )
        .with_details(json!({"outcome": outcome.envelope(Detail::Full), "review": review})));
    }

    // No absolute local path in the package's text. Workflow #463 AD-1: the recordings' own
    // directory is one too where it is not the state directory, as when both are one.
    let mut needles = vec![state.display().to_string(), repo.display().to_string()];
    if let Some(own) = recordings.as_ref().filter(|own| **own != state) {
        needles.push(own.display().to_string());
    }
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
        repo: &repo,
        root: &root,
        package: &package,
        package_id: &package_id,
        server: &server,
        class,
        decision: decision.as_ref(),
        existing: &existing,
    })?;

    // The tree as it would be written, checked as the Runtime reads it.
    let check = check_tree(
        &CheckInputs {
            repo: &repo,
            root: &root,
            state: &state,
            at_unix_ms: request.at_unix_ms,
            package_id: &package_id,
            files: &package.files,
            plan: &plan,
        },
        stages,
    )?;
    stages.enter(AdoptStage::RuleChecks);
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
    // The Runtime refuses every dispatch of a task whose outcome the schedule reads for a key its
    // package does not declare: every key the compiled schedule reads of the rule, and every key
    // the rule reads of another task, is checked here.
    outcome_keys_declared(&KeyInputs {
        repo: &repo,
        plan: &plan,
        package: &package,
        decision: decision.as_ref(),
        package_id: &package_id,
        compiled: check.compiled.as_ref(),
    })?;
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
    // Workflow #463 AC-1: the cases, checked last, before the first write: each names one
    // directory of the checkout that is free or the earlier set's, and each is run by the one
    // runner on the package as this adoption writes it, the check tree's copy of it.
    stages.enter(AdoptStage::Cases);
    let case_plan = match (taken, &checkout_cases) {
        (Some(recording_cases), Some(checkout)) => {
            let package_dir = check.tree.join(CHECK_PACKS_ROOT).join(&package_id);
            Some(plan_cases(
                &CaseInputs {
                    taken: recording_cases,
                    checkout,
                    regression_relative: &regression_relative,
                    package_id: &package_id,
                    package_dir: &package_dir,
                },
                worktree.lab(),
            )?)
        }
        _ => None,
    };
    // Fix round 1: an adoption that takes no case (`none_found`, `package_path`) leaves the cases
    // of the package the checkout holds as they are, neither moved nor run on this content: the
    // earlier set a recording gave and a case's `.part`; it warns of them.
    let cases_left = match taken {
        Some(_) => Vec::new(),
        None => CheckoutCases::read(&regression_dir, &regression_relative, &package_id)?
            .left(&regression_relative),
    };
    stages.enter(AdoptStage::Warnings);
    let coverage = match (&plan.rule, class) {
        (Some(rule), AdoptClass::Business) if plan.held.is_none() => {
            Some(rule_coverage(rule, &root.schedule.pools, &check.answer))
        }
        _ => None,
    };
    // Who is judged (the evaluator's placement): not a follower of a chain, which rides with its
    // head, and not a manual request, which runs when it is asked for; every other business rule
    // is judged by its value and waits while that value is not known.
    let not_judged = match (rule_id.as_deref(), coverage) {
        (Some(rule_id), Some(AdoptCoverage::Follower)) => {
            let head = check
                .answer
                .benefit_followers
                .iter()
                .find(|follower| follower.task_id == rule_id)
                .map(|follower| follower.rides_with.clone())
                .unwrap_or_default();
            Some(ValueNotJudged {
                because: NotJudgedBecause::Follower,
                text: format!(
                    "{rule_id} rides with its chain's head {head}: the Runtime's evaluator \
                     dispatches it with that head whatever its own value"
                ),
            })
        }
        (Some(rule_id), Some(_)) if manual_request(check.compiled.as_ref(), rule_id) => {
            Some(ValueNotJudged {
                because: NotJudgedBecause::ManualRequest,
                text: format!(
                    "{rule_id} is a manual request (its trigger requires an instruction of the \
                     session. family): the Runtime's evaluator dispatches it when the \
                     instruction is given, whatever its value, and never judges it by its value"
                ),
            })
        }
        _ => None,
    };
    let waits_for_value = match (&plan.rule, coverage, &not_judged) {
        (Some(rule), Some(_), None) => value_wait(rule, &root.schedule.pools),
        _ => None,
    };
    // Workflow #463 S3-12: the rule has no value yet when it writes no fixed value and the
    // evaluator holds it until a value is given; `next` then names where its value is registered.
    let unvalued_rule = match (&plan.rule, &waits_for_value) {
        (Some(rule), Some(_)) if !rule.contains_key(SCHEDULE_RULE_FIXED_VALUE_MEMBER) => rule_id,
        _ => None,
    };
    let mut warnings = all_warnings(&check);
    // Workflow #463 S3-12: the two things the owner's rules require before a pack runs, said at
    // adoption and never refused: a decision of a class that runs is enabled only by a placement
    // under a named authorization, and a content no trial passed is not tried.
    if decision
        .as_ref()
        .is_some_and(|decision| decision.class != AdoptClass::Held)
    {
        warnings.push(named_package(
            Outcome::new(ResourceToolingCode::EnablingNeedsAuthorization),
            &package_id,
        ));
    }
    if not_tried {
        warnings.push(named_package(
            Outcome::new(ResourceToolingCode::NotTried),
            &package_id,
        ));
    }
    if !cases_left.is_empty() {
        warnings.push(
            named_package(
                Outcome::new(ResourceToolingCode::AdoptCasesLeft),
                &package_id,
            )
            .paths(cases_left.iter()),
        );
    }
    // Workflow #463 AD-1: each warning with the pack of the check tree's game root it names, by
    // the one rule a readiness row's warnings are read by; those that name another package are
    // counted by code, every other one is answered whole. Fix round 1: a root that cannot be read
    // again for it refuses nothing, as the readiness's own reading of it does not: every warning is
    // answered whole, then `adopt_warnings_not_split` with that refusal as its cause, and no count.
    let (answered_warnings, other_packages_warnings) =
        match with_packs_named(&check.installed, &root.game, &warnings) {
            Ok(named) => {
                let (whole, others) = of_the_package(named, &package_id);
                (whole, Some(others))
            }
            Err(error) => {
                warnings.push(
                    Outcome::new(ResourceToolingCode::AdoptWarningsNotSplit)
                        .path(&check.installed)
                        .caused_by(cause_of(&error)),
                );
                (warnings, None)
            }
        };

    // What it writes, in the order it renames it into place.
    stages.enter(AdoptStage::Write);
    let mut changed = vec![pack_relative.clone()];
    for name in root_order(class == AdoptClass::Maintenance) {
        let changes = match name {
            STANDARD_PACK_SCHEDULE_FILE => plan.schedule_bytes.is_some(),
            // Workflow #477 CV-2b: only on a switched root.
            STANDARD_PACK_CATALOG_FILE => plan.catalog_bytes.is_some(),
            _ => plan.maintenance_bytes.is_some(),
        };
        if changes {
            changed.push(name.to_owned());
        }
    }
    let replaced = request.replace && pack_exists;
    // Workflow #463 AC-1: the cases' directories, relative to the checkout, in the order they are
    // renamed into place, after everything else the adoption writes.
    let case_directories = case_plan
        .as_ref()
        .map(|planned| {
            planned
                .cases
                .iter()
                .map(|case| format!("{regression_relative}/{}", case.name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (written, would_write, kept, cases_kept_at) = if request.dry_run {
        let mut would_write = changed;
        would_write.extend(case_directories.iter().cloned());
        (Vec::new(), would_write, None, None)
    } else {
        let (moved, done) = write_adoption(
            &WriteInputs {
                repo: &repo,
                files: &package.files,
                staged: &staged,
                pack_dir: &pack_dir,
                pack_relative: &pack_relative,
                kept: replaced.then_some(kept_place.as_path()),
                plan: &plan,
                read_schedule: root.schedule_bytes.as_deref(),
                read_maintenance: &root.maintenance_bytes,
                read_catalog: root
                    .catalog
                    .as_ref()
                    .map(|catalog| catalog.bytes.as_slice()),
                maintenance_first: class == AdoptClass::Maintenance,
            },
            worktree,
        )?;
        let cases_moved = match &case_plan {
            Some(planned) => write_cases(
                &CaseWrites {
                    directory: &regression_dir,
                    kept: &cases_kept,
                    plan: planned,
                },
                done,
                worktree,
            )?,
            None => None,
        };
        let mut written = changed;
        written.extend(case_directories.iter().cloned());
        (
            written,
            Vec::new(),
            moved.map(|path| path.display().to_string()),
            cases_moved.map(|path| path.display().to_string()),
        )
    };
    // Workflow #463 AC-1: the cases taken, or the word why none was; `next` names a regression
    // case only when none was taken.
    let none_taken = case_directories.is_empty();
    let (cases_written, cases_would_write) = if request.dry_run {
        (Vec::new(), case_directories)
    } else {
        (case_directories, Vec::new())
    };
    let cases = match &case_plan {
        Some(planned) => AdoptCases {
            state: AdoptCasesState::Taken,
            from: Some(planned.from.display().to_string()),
            written: cases_written,
            would_write: cases_would_write,
            replaced: planned
                .replaced
                .iter()
                .map(|name| format!("{regression_relative}/{name}"))
                .collect(),
            kept: cases_kept_at,
            not_proven: planned.not_proven.clone(),
        },
        None => AdoptCases {
            state: if recorded.is_some() {
                AdoptCasesState::NoneFound
            } else {
                AdoptCasesState::PackagePath
            },
            from: None,
            written: Vec::new(),
            would_write: Vec::new(),
            replaced: Vec::new(),
            kept: None,
            not_proven: Vec::new(),
        },
    };
    let next = next_items(&repo, &root.game, unvalued_rule.as_deref(), none_taken);
    // Workflow #463 S3-5: every check passed (a refused adoption answers no readiness); the
    // adopted pack's readiness on the game root the check tree laid out, by the same path under
    // `--dry-run`. It changes nothing of the adoption. Workflow #463 S2b-1: with the warnings of
    // the check of that root, which nothing has written under since.
    stages.enter(AdoptStage::Readiness);
    let (readiness, readiness_not_judged) =
        match adopted_readiness(&check.installed, &package_id, &check.answer.warnings) {
            Ok(row) => (Some(row), None),
            Err(why) => (None, Some(why)),
        };
    let timings = stages.answer();
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
        rule: plan.written_rule().map(Value::Object),
        held: plan.held.clone(),
        maintenance: plan.maintenance_entry.clone(),
        catalog_entry: plan.catalog_entry.clone(),
        removed_entries: plan.removed.clone(),
        replaced_entries: plan.replaced.clone(),
        requires_added: plan.requires_added.clone(),
        given: plan.given.clone(),
        derived: plan.derived.clone(),
        derived_drift: plan.drift.clone(),
        model_kept: plan.model_kept.clone(),
        coverage,
        waits_for_value,
        value_not_judged: not_judged,
        warnings: answered_warnings,
        other_packages_warnings,
        written,
        would_write,
        kept,
        check_tree: check.tree.display().to_string(),
        readiness,
        readiness_not_judged,
        next,
        cases,
        timings,
    })
}

/// Workflow #463 S3-5: the row of the adopted pack `package_id` of the readiness of the check
/// tree's game root `installed` (`<tree>/installed/<game>/`), judged by the readiness function,
/// called as it is, with no installation, so `ready_to_place` at best. When that function refuses
/// the root, or answers no row of the pack (the function's own `package_invalid`, "is no package
/// of the bundle"), why it was not judged: the refusal's outcome and the same in words. Workflow
/// #463 S2b-1: the root is not checked a second time: `checked` are the warnings of the check of
/// this root the call ran and that accepted it ([`check_tree`]), handed to the readiness as that
/// check's answer.
fn adopted_readiness(
    installed: &Path,
    package_id: &str,
    checked: &[Outcome],
) -> Result<PackReadiness, ReadinessNotJudged> {
    let not_judged = |error: &LabError| ReadinessNotJudged {
        outcome: cause_of(error),
        text: format!(
            "the readiness of {package_id} was not judged on the check tree's game root {}: \
             {error}; the adoption is answered without it",
            installed.display()
        ),
    };
    judge_checked_readiness(
        &StandardPackReadyRequest {
            root: installed.to_path_buf(),
            declared_package: None,
            installation: None,
        },
        checked,
    )
    .map_err(|error| not_judged(&error))?
    .packs
    .into_iter()
    .find(|pack| pack.package_id == package_id)
    .ok_or_else(|| {
        not_judged(&LabError::package_invalid(format!(
            "{package_id} is no package of the bundle"
        )))
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
    /// The packages a role of the root files makes maintenance-role, as the composer's
    /// classification holds them (`standard_pack_maintenance_packages`), so that a rule's role is
    /// read by the composer's own `is_judged_rule`; its other sets are not read and stay empty.
    classification: StandardPackClassification,
    /// The packages each root file adopt does not write (`pages.json`, `navigation.json`,
    /// `interrupts.json`) gives a maintenance role, by the file's name.
    other_roles: Vec<(&'static str, BTreeSet<String>)>,
    /// What interrupted adoptions left below `packs/` ([`leftovers`]); a leftover root file is
    /// refused when the checkout is read.
    leftovers: Vec<String>,
    /// Workflow #477 CV-2b: of a switched root (its `schedule.json` names `catalog_business`), its
    /// `pack-catalog.json`, of version 2, which holds its rules' business part; `None` otherwise.
    catalog: Option<SwitchedCatalog>,
}

/// Workflow #477 CV-2b (`contracts/standard-pack-authoring.md`, "On a switched root"): the
/// `pack-catalog.json` of a switched root as the call read it: its bytes, and the file as the
/// contract crate's one reader reads it for an adoption.
struct SwitchedCatalog {
    bytes: Vec<u8>,
    read: AdoptionCatalog,
}

impl SwitchedCatalog {
    /// The checkout `repo`'s `pack-catalog.json` (game `game`), which its switched `schedule.json`
    /// takes every rule's business part from. A root without it, or with one of version 1, is
    /// refused `repository_invalid` at it, its cause `standard_pack_root_invalid` with the words
    /// the composer and the writer use (`catalog_missing`, `catalog_version_1`); one its reader
    /// refuses, with the reader's first error: its business part lives there alone and is
    /// restored, never made again from the rules, which write none.
    fn read(repo: &Path, game: &str) -> LabResult<Self> {
        let path = repo.join(STANDARD_PACK_CATALOG_FILE);
        let refused_at = |issue: StandardPackRootIssue| {
            refused(
                adopt(AdoptProblem::RepositoryInvalid)
                    .entry(STANDARD_PACK_CATALOG_FILE)
                    .json_pointer(bounded_pointer(&issue.pointer))
                    .caused_by(root_issue_outcome(game, &path, &issue)),
                format!(
                    "{STANDARD_PACK_CATALOG_FILE}: {issue}; the root is switched: its \
                     schedule.json names {CATALOG_BUSINESS_SUBSYSTEM}, so its rules write no \
                     business part, which lives in {STANDARD_PACK_CATALOG_FILE} of version 2 \
                     alone and must be restored, not rebuilt; nothing was written"
                ),
            )
        };
        let Some(bytes) = optional_file(&path, STANDARD_PACK_CATALOG_FILE)? else {
            return Err(refused_at(StandardPackRootIssue {
                file: STANDARD_PACK_CATALOG_FILE,
                problem: StandardPackProblem::CatalogMissing,
                pointer: String::new(),
            }));
        };
        let read = AdoptionCatalog::read(&bytes, game).map_err(refused_at)?;
        Ok(Self { bytes, read })
    }
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
            return Err(repository_invalid(
                part,
                interrupted(repo, &with_catalog_part(repo, leftovers.clone())),
            ));
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
        // Workflow #477 CV-2b: a switched root's catalog, whose `.part` an interrupted call left is
        // that call's root file as the others are ("If a call was interrupted").
        let switched = schedule
            .requires
            .iter()
            .any(|name| name == CATALOG_BUSINESS_SUBSYSTEM);
        let catalog = if switched {
            let left = with_catalog_part(repo, leftovers.clone());
            if let Some(part) = left
                .iter()
                .find(|left_part| !left_part.starts_with(&pack_prefix))
            {
                return Err(repository_invalid(part, interrupted(repo, &left)));
            }
            Some(SwitchedCatalog::read(repo, &game)?)
        } else {
            None
        };
        let roles = maintenance_roles(repo, &game, &maintenance);
        Ok(Self {
            game,
            servers,
            maintenance_bytes,
            maintenance,
            schedule_bytes,
            schedule,
            classification: StandardPackClassification {
                maintenance: roles.all,
                ..StandardPackClassification::default()
            },
            other_roles: roles.others,
            leftovers,
            catalog,
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

/// Workflow #477 CV-2b: `leftovers` ([`leftovers`]) with `pack-catalog.json.part` first, when it is
/// there and the checkout's `schedule.json` names `catalog_business`: on a switched root adopt
/// writes the catalog as one of its root files, and an interrupted call leaves its `.part` as it
/// leaves theirs. On any other root adopt writes no catalog and its `.part` is not looked at.
fn with_catalog_part(repo: &Path, mut leftovers: Vec<String>) -> Vec<String> {
    let part = format!("{STANDARD_PACK_CATALOG_FILE}.{PART}");
    if switched_on_disk(repo) && fs::symlink_metadata(repo.join(&part)).is_ok() {
        leftovers.insert(0, part);
    }
    leftovers
}

/// Workflow #477 CV-2b: whether the checkout `repo`'s `schedule.json`, as it stands, names
/// `catalog_business`: a switched root, whose adoption writes `pack-catalog.json` as one of its
/// root files ([`with_catalog_part`], [`interrupted`]).
fn switched_on_disk(repo: &Path) -> bool {
    fs::read(repo.join(STANDARD_PACK_SCHEDULE_FILE))
        .is_ok_and(|bytes| standard_pack_schedule_takes_catalog(&bytes))
}

/// The message of a refusal for what an interrupted adoption left: each leftover by name, what it
/// tells of that call, and the steps to take from there, each of which a later call accepts
/// (`contracts/standard-pack-authoring.md`, "If a call was interrupted"). A root file's `.part` is
/// trusted as that file whole only when [`finished_part`] finds it so.
fn interrupted(repo: &Path, leftovers: &[String]) -> String {
    let pack_prefix = format!("{PACKS_DIRECTORY}/");
    let mut said = vec![format!(
        "an earlier adoption was interrupted and left {leftovers:?} in the checkout, and adopt \
         deletes nothing"
    )];
    if leftovers.iter().any(|left| left.starts_with(&pack_prefix)) {
        said.push(
            "packs/<package_id>.part is a pack directory that call had not put in place (whole, \
             or cut off where the call was interrupted), and that call had written no root file: \
             when it began, packs/<package_id> was absent or committed (adopt refuses a pack \
             directory with changes), so `git status -- packs/<package_id>` shows exactly what \
             it left there, and a directory it replaced and had moved away is in the last commit \
             and under the state directory's adopt/; move that .part out of the checkout by \
             hand, then run adopt again as that call was run"
                .to_owned(),
        );
    }
    // Workflow #477 CV-2b (fix round 1): on a switched root the catalog is a root file the call
    // writes, renamed right before `schedule.json`, whether its `.part` is left or not: the call
    // may have renamed it already ([`with_catalog_part`] takes its `.part` only there).
    let switched = switched_on_disk(repo);
    let root_files: &[&str] = if switched {
        &[
            STANDARD_PACK_CATALOG_FILE,
            STANDARD_PACK_SCHEDULE_FILE,
            MAINTENANCE_FILE,
        ]
    } else {
        &[STANDARD_PACK_SCHEDULE_FILE, MAINTENANCE_FILE]
    };
    let parts = root_files
        .iter()
        .copied()
        .filter(|name| leftovers.contains(&format!("{name}.{PART}")))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return said.join("; ");
    }
    let renames = if switched {
        "renames pack-catalog.json, schedule.json and maintenance.json one after the other \
         (pack-catalog.json right before schedule.json, maintenance.json first for the class \
         maintenance and last for the others)"
    } else {
        "renames schedule.json and maintenance.json one after the other"
    };
    said.push(format!(
        "that call put its packs/<package_id> in place before it wrote its first root .part, \
         writes every root .part before it renames the first, and {renames}, so {parts:?} were \
         not renamed and are as they were before that call"
    ));
    let mut trusted = Vec::new();
    let mut cut_off = false;
    for name in parts {
        match finished_part(repo, name) {
            Ok(()) => {
                said.push(format!(
                    "{name}.{PART} is trusted as {name} whole: it reads as {name} and is in the \
                     canonical form, its final newline included, as a finished write leaves it"
                ));
                trusted.push(name);
            }
            Err(not_whole) => {
                said.push(format!(
                    "{name}.{PART} is not trusted ({}): it is not whole, as a .part the call was \
                     interrupted while writing is not",
                    not_whole.message
                ));
                cut_off = true;
            }
        }
    }
    let finish = if cut_off {
        "a .part not trusted was being written when the call stopped, so it had renamed no root \
         file and every root file is as it was before that call; renaming does not finish that \
         adoption"
            .to_owned()
    } else if switched {
        // Workflow #477 CV-2b: the root files with no `.part` left, of the three.
        let others = root_files
            .iter()
            .copied()
            .filter(|name| !trusted.contains(name))
            .collect::<Vec<_>>();
        if others.is_empty() {
            "all three were written whole and none was renamed: renaming each over its file, \
             pack-catalog.json before schedule.json, finishes that adoption and nothing more is \
             run"
            .to_owned()
        } else {
            format!(
                "{others:?} were renamed by that call, or were not to change in it, or were to \
                 change and the call stopped before it wrote their .part: when each already is as \
                 that call decided it (the package's rule, held entry or maintenance.json entry \
                 there, or gone where the call ended one, and the business part of its rule in \
                 pack-catalog.json), renaming each .part left over its file, pack-catalog.json \
                 before schedule.json, finishes that adoption and nothing more is run; when one \
                 is not, renaming does not finish it"
            )
        }
    } else if let [one] = trusted.as_slice() {
        let other = if *one == STANDARD_PACK_SCHEDULE_FILE {
            MAINTENANCE_FILE
        } else {
            STANDARD_PACK_SCHEDULE_FILE
        };
        format!(
            "{other} was renamed by that call, or was not to change in it, or was to change and \
             the call stopped before it wrote its .part: when {other} already is as that call \
             decided it (the package's rule, held entry or maintenance.json entry there, or gone \
             where the call ended one), renaming {one}.{PART} over {one} finishes that adoption \
             and nothing more is run; when it is not, renaming does not finish it"
        )
    } else {
        "both were written whole and neither was renamed: renaming each over its file finishes \
         that adoption and nothing more is run"
            .to_owned()
    };
    said.push(finish);
    // CV-2b (fix round 1): on a switched root the undo restores the catalog too, which a call that
    // left `schedule.json.part` alone has replaced already.
    let restored = if switched {
        "a root file that call renamed, pack-catalog.json among them (on this switched root that \
         call renamed it right before schedule.json, so when schedule.json.part is left and \
         pack-catalog.json.part is not, pack-catalog.json is that call's already: restore it from \
         the repository's history too, or the rule as it was composes with the new business part \
         of pack-catalog.json)"
    } else {
        "a root file that call renamed"
    };
    said.push(format!(
        "to run that adoption again instead: move the root .part files out of the checkout; {}; \
         then run adopt with --replace and the decision that call was given (--decide, or \
         --held-reason, with --held-reference when that call had one), if any, which writes the \
         package's one role from the root files as they are; to undo it: move the root .part \
         files out, put packs/<package_id> back as it was, and restore with the version control \
         {restored} (to its last commit: a change not committed before that call is not in it)",
        new_directory_step("packs/<package_id>", "that call")
    ));
    said.join("; ")
}

/// The step that comes before adopt is run again on a package whose new directory an interrupted
/// or stopped call (`who`) put in place, `pack` by its path relative to the checkout: the
/// directory is not committed, so `--replace` refuses it `pack_dirty` until it is put back as it
/// was or committed (`contracts/standard-pack-authoring.md`, "If a call was interrupted"). Fix
/// round 4: the two are not alike across volumes. Put back as it was, `--replace` needs no state
/// directory on the checkout's volume: absent (`who` adopted the package first) it moves nothing,
/// committed (`who` replaced it) `who` passed the volume check with its `--state-dir` already.
/// Committed as `who` left it, `--replace` moves it to the kept place under the state directory,
/// which must lie on the checkout's volume (`replace_across_volumes` otherwise), a check `who`
/// never ran when it began without the directory.
fn new_directory_step(pack: &str, who: &str) -> String {
    format!(
        "{pack} is {who}'s new directory and not committed, which --replace refuses: either put \
         it back as it was before {who} (move it out of the checkout by hand and, where it was \
         committed, restore it with the version control; a directory {who} replaced is also \
         under the state directory's adopt/), which needs no state directory on the checkout's \
         volume (absent, --replace moves nothing; committed, {who} passed that check already \
         with its --state-dir); or commit it, after which --replace moves it to its kept place \
         under the state directory's adopt/, so the state directory must lie on the checkout's \
         volume: give a --state-dir there (actinglab's default session directory may lie on \
         another volume), and with another --state-dir name a recording's own directory by \
         --session-state-dir, or give its package by --package <its lab/out/ file> instead of \
         --record-id"
    )
}

/// Whether the `.part` of the root file `name` that an interrupted call left in `repo` is that
/// file whole, as a finished write of adopt leaves it: a file that reads as that root file
/// (`schedule.json` by the schedule reader, `maintenance.json` as its list of entries, Workflow
/// #477 CV-2b `pack-catalog.json` as a document of version 2) and is in the canonical form, its
/// final newline included. A `.part` cut off by a crash ends before its last bracket or its final
/// newline, so it is not. Fix round 4 (the outcome guard's G2): `Err` is
/// the module's own outcome-carrying refusal, `adopt_refused` `repository_invalid` at that `.part`
/// (`entry` `<name>.part`, the operating system's refusal as the cause where there is one), its
/// message saying why not, in words, which [`interrupted`] folds into its own.
fn finished_part(repo: &Path, name: &str) -> LabResult<()> {
    let part = format!("{name}.{PART}");
    let path = repo.join(&part);
    let unreadable = |error: &io::Error, op: IoOp| {
        refused(
            adopt(AdoptProblem::RepositoryInvalid)
                .entry(&part)
                .caused_by(Outcome::from_io_error(error, op).path(&path)),
            format!("it cannot be read: {error}"),
        )
    };
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(repository_invalid(&part, "it is no file".to_owned())),
        Err(error) => return Err(unreadable(&error, IoOp::Metadata)),
    }
    let bytes = fs::read(&path).map_err(|error| unreadable(&error, IoOp::Read))?;
    let value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        repository_invalid(&part, format!("it is no whole JSON document: {error}"))
    })?;
    if canonical_json_bytes(&value).ok().as_deref() != Some(bytes.as_slice()) {
        return Err(repository_invalid(
            &part,
            "it is not in the canonical form, its final newline included".to_owned(),
        ));
    }
    if name == STANDARD_PACK_SCHEDULE_FILE {
        parse_standard_pack_schedule(&bytes)
            .map(|_| ())
            .map_err(|issue| {
                repository_invalid(&part, format!("it does not read as schedule.json: {issue}"))
            })
    } else if name == STANDARD_PACK_CATALOG_FILE {
        // Workflow #477 CV-2b: a switched root's catalog, of version 2.
        if value.get(SCHEMA_VERSION).and_then(Value::as_str)
            == Some(STANDARD_PACK_CATALOG_SCHEMA_VERSION_V2)
        {
            Ok(())
        } else {
            Err(repository_invalid(
                &part,
                "it does not read as pack-catalog.json of version 2".to_owned(),
            ))
        }
    } else {
        serde_json::from_value::<Vec<BundleMaintenance>>(value)
            .map(|_| ())
            .map_err(|error| {
                repository_invalid(
                    &part,
                    format!("it does not read as maintenance.json: {error}"),
                )
            })
    }
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

    /// What the entry task's provenance says of the recording; the trial is the caller's to read.
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
                        // #463 KR-1 fix round 1: a key step by the press its key replaced, which
                        // the step's `key` entry says.
                        ["click", "key"].into_iter().any(|effect| {
                            step.get(effect)
                                .and_then(|entry| entry.get("needs_review"))
                                .and_then(Value::as_bool)
                                == Some(true)
                        })
                    })
                    .filter_map(|step| step.get("step").and_then(Value::as_u64))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let first_decision_evaluated = recorded && !not_evaluated && !application;
        let recording_checked = first_decision_evaluated
            && cross_check.as_deref() == Some(CROSS_CHECK_PASSED)
            && steps_needing_review.is_empty();
        RecordingReview {
            recorded,
            record_id,
            cross_check,
            first_decision_evaluated,
            steps_needing_review,
            recording_checked,
            trial: None,
            trial_not_read: None,
            ready_not_read: None,
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

/// What adopt reads of a stopped recording (`--record-id`).
struct Recorded {
    /// The package its last stop generated, `<lab_dir>/out/<package_id>.<ext>`.
    package: PathBuf,
    /// Workflow #463 AC-1: the cases that stop wrote beside it; `None` when it wrote none (a stop
    /// of a build before #463 S2-10).
    cases: Option<RecordedCases>,
    /// Workflow #463 S3-17: why `record ready`'s function did not read the recording, which the
    /// check of its own members judged instead.
    ready_not_read: Option<ReadyNotRead>,
}

/// Workflow #463 AC-1: the regression cases a recording's last stop wrote beside its package.
struct RecordedCases {
    /// `<lab_dir>/out/<package_id>.cases/`.
    directory: PathBuf,
    /// The names of its entries, each a case's directory named by the case's id, in the order of
    /// the names.
    names: Vec<String>,
}

/// The package a stopped recording's last stop generated, `<lab_dir>/out/<package_id>.<ext>`,
/// when that stop is the recording's last change. Workflow #463 S3-17: the recording is judged by
/// `record ready`'s one function (`lab`, on `recordings`, the directory the recordings are read
/// from, Workflow #463 AD-1): a status other than
/// `packaged` is refused `recording_not_current` with that function's answer in the details
/// (`record_ready`, its list of what is missing among it). Where the two differ, the check of the
/// recording's own members stays beside it: its schema and its `record_id`, first, as before; its
/// `status` and `amended_at_unix_ms` when that function cannot read the recording whole (its
/// refusal as the cause, or, when they pass, said as `ready_not_read`); and its artifact, which
/// names the package by a `package_id` of one path segment and a container `zip` or `json`, and
/// the package's file. Workflow #463 AC-1: with the cases that stop wrote beside the package. Fix
/// round 1: every other refusal of that function refuses the adoption, `recording_not_judged`.
fn recorded_package(
    record_id: &str,
    lab_dir: &Path,
    recordings: &Path,
    lab: &dyn AdoptLab,
) -> LabResult<Recorded> {
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
    // Workflow #463 S3-17: the one judgement of what the recording still lacks, `record ready`'s.
    let ready_not_read = match lab.recording_ready(recordings, record_id) {
        Ok(ready) if ready.packaged => None,
        Ok(ready) => {
            let status = ready
                .answer
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let missing = ready
                .answer
                .get("missing_total")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let outcome = adopt(AdoptProblem::RecordingNotCurrent).path(&path);
            return Err(refused(
                outcome.clone(),
                format!(
                    "the recording {record_id} is {status} by record ready, not packaged: its \
                     package is not the current one of its last record stop ({missing} missing, \
                     each as record ready lists it, in the details' record_ready); run record \
                     stop, or record ready --record-id {record_id} to see what it lacks; nothing \
                     was written"
                ),
            )
            .with_details(json!({
                "outcome": outcome.envelope(Detail::Full),
                "record_ready": ready.answer
            })));
        }
        // Fix round 1 (the coordinator's ruling): a failure on the Lab's side is never a pass. The
        // function failed otherwise than on a recording it cannot read whole: refused, its
        // refusal the cause.
        Err(ReadyRefused::Failed(error)) => {
            return Err(refused(
                adopt(AdoptProblem::RecordingNotJudged)
                    .path(&path)
                    .caused_by(cause_of(&error)),
                format!(
                    "record ready's function did not judge the recording {record_id} ({error}), \
                     and adopt takes no recording it did not judge; run record ready --record-id \
                     {record_id} to see why, then adopt again; nothing was written"
                ),
            ));
        }
        // The function cannot read the recording whole: the check of its own members judges it,
        // kept beside the function for this difference.
        Err(ReadyRefused::Unreadable(unread)) => {
            let cause = cause_of(&unread);
            let members = |member: &str, message: String| {
                refused(
                    adopt(AdoptProblem::RecordingNotCurrent)
                        .path(&path)
                        .member(member)
                        .caused_by(cause.clone()),
                    message,
                )
            };
            if text("status") != Some(RECORDING_STOPPED) {
                return Err(members(
                    "status",
                    format!("the recording {record_id} is not stopped ({unread}); run record stop"),
                ));
            }
            if recording
                .get("amended_at_unix_ms")
                .is_some_and(|amended| !amended.is_null())
            {
                return Err(members(
                    "amended_at_unix_ms",
                    format!(
                        "the recording {record_id} was marked again after its package was \
                         generated ({unread}); run record stop"
                    ),
                ));
            }
            Some(ReadyNotRead {
                outcome: cause,
                text: format!(
                    "record ready's function did not read the recording {record_id} ({unread}); \
                     its own members were checked instead: stopped, and not marked again since \
                     its package was generated"
                ),
            })
        }
    };
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
    // Workflow #463 AC-1: the cases that stop wrote beside the package (#463 S2-10); none when it
    // wrote none.
    let directory = lab_dir
        .join(RECORDING_OUT)
        .join(format!("{package_id}.{CASES}"));
    let cases = match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        _ => {
            let names = case_names(&directory)?;
            Some(RecordedCases { directory, names })
        }
    };
    Ok(Recorded {
        package: locator,
        cases,
        ready_not_read,
    })
}

/// Workflow #463 AC-1: the names of the entries of a recording's cases directory, each a case's
/// directory, in the order of their names. A directory that cannot be listed, or an entry whose
/// name is no text, is not as a stop leaves it: `recording_not_current` at it.
fn case_names(directory: &Path) -> LabResult<Vec<String>> {
    let unlisted = |error: io::Error| {
        refused(
            adopt(AdoptProblem::RecordingNotCurrent)
                .path(directory)
                .caused_by(Outcome::from_io_error(&error, IoOp::ReadDir).path(directory)),
            format!(
                "{}, the cases of the recording's last stop: {error}",
                directory.display()
            ),
        )
    };
    let mut names = fs::read_dir(directory)
        .map_err(unlisted)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(unlisted)?;
    names.sort();
    names
        .into_iter()
        .map(|name| {
            name.into_string().map_err(|name| {
                let entry = directory.join(&name);
                refused(
                    adopt(AdoptProblem::RecordingNotCurrent).path(&entry),
                    format!(
                        "{} is no case a record stop writes: its name is no text",
                        entry.display()
                    ),
                )
            })
        })
        .collect()
}

/// Workflow #463 AC-1: what `regression/<game>/` of the checkout holds of the adopted package's
/// cases.
struct CheckoutCases {
    /// The directories of the package's cases an earlier adoption took from a recording
    /// ([`recorded_case`]), by name, in the order of the names: replaced as a set.
    recorded: Vec<String>,
    /// Every other name there, in ASCII lower case, as a file system that does not tell the
    /// cases apart compares them: a case's destination named so is taken.
    others: BTreeSet<String>,
    /// The `.part` of a case of the package (`<package_id>.<case>.part`) an interrupted adoption
    /// left, by name.
    parts: Vec<String>,
}

impl CheckoutCases {
    /// Reads `directory`, `regression/<game>/` of the checkout (`relative`), for the cases of
    /// `package_id`; nothing when it is not there. A directory that cannot be listed is
    /// `repository_invalid` at it.
    fn read(directory: &Path, relative: &str, package_id: &str) -> LabResult<Self> {
        let mut found = Self {
            recorded: Vec::new(),
            others: BTreeSet::new(),
            parts: Vec::new(),
        };
        let unlisted = |error: io::Error| {
            refused(
                adopt(AdoptProblem::RepositoryInvalid)
                    .entry(relative)
                    .caused_by(Outcome::from_io_error(&error, IoOp::ReadDir).path(directory)),
                format!("{}: {error}", directory.display()),
            )
        };
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(found),
            Err(error) => return Err(unlisted(error)),
        };
        let mut names = entries
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<_>>>()
            .map_err(unlisted)?;
        names.sort();
        let prefix = format!("{package_id}.");
        let suffix = format!(".{PART}");
        for name in names {
            let Some(text) = name.to_str() else {
                found
                    .others
                    .insert(name.to_string_lossy().to_ascii_lowercase());
                continue;
            };
            let part_of_case = text
                .strip_prefix(prefix.as_str())
                .and_then(|rest| rest.strip_suffix(suffix.as_str()))
                .is_some_and(|case| !case.is_empty() && !case.contains('.'));
            if part_of_case {
                found.parts.push(text.to_owned());
            } else if recorded_case(&directory.join(text), package_id) {
                found.recorded.push(text.to_owned());
            } else {
                found.others.insert(text.to_ascii_lowercase());
            }
        }
        Ok(found)
    }

    /// Fix round 1: the directories of the package's cases that an adoption taking none leaves as
    /// they are (the earlier set a recording gave and each case's `.part`), relative to the
    /// checkout (`relative` is `regression/<game>`), in the order of their names.
    fn left(&self, relative: &str) -> Vec<String> {
        let mut names = self.recorded.iter().chain(&self.parts).collect::<Vec<_>>();
        names.sort();
        names
            .into_iter()
            .map(|name| format!("{relative}/{name}"))
            .collect()
    }
}

/// Workflow #463 AC-1 (`contracts/standard-pack-authoring.md`, "The cases"): whether `directory`
/// is a case an adoption took from a recording of the package `package_id`, which a later
/// adoption of the package replaces: a directory (no link) whose `case.json` is a JSON object
/// whose `package_id` is the package's and whose `source` is a recording's, the form `record stop`
/// writes (#463 S2-10): an object of `record_id` (a text) and exactly one of `step` (a whole
/// number), `final` (`true`) and `ending` (a text), and nothing else. The member `source.record_id`
/// in that form tells it; a case of another source, or of none, is never moved or rewritten.
fn recorded_case(directory: &Path, package_id: &str) -> bool {
    if !fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.is_dir()) {
        return false;
    }
    let Ok(bytes) = fs::read(directory.join(CASE_FILE)) else {
        return false;
    };
    let Ok(Value::Object(case)) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    if case.get(PACKAGE_ID).and_then(Value::as_str) != Some(package_id) {
        return false;
    }
    let Some(Value::Object(source)) = case.get(CASE_SOURCE) else {
        return false;
    };
    let pointers = [
        source.get(SOURCE_STEP).is_some_and(Value::is_u64),
        source.get(SOURCE_FINAL) == Some(&Value::Bool(true)),
        source.get(SOURCE_ENDING).is_some_and(Value::is_string),
    ];
    source.get(SOURCE_RECORD_ID).is_some_and(Value::is_string)
        && source.len() == 2
        && pointers.iter().filter(|held| **held).count() == 1
}

/// Workflow #463 AC-1: the message of `pack_exists` at a case's `.part`, `entry` relative to the
/// checkout, that an interrupted adoption of `package_id` left (`contracts/standard-pack-authoring.md`,
/// "If a call was interrupted").
fn case_part_left(entry: &str, package_id: &str) -> String {
    format!(
        "{entry} is a regression case's directory that an adoption of {package_id} wrote and had \
         not put in place when it was interrupted, and adopt deletes nothing; an adoption writes \
         its cases last, after it renamed packs/{package_id} and the root files into place, so that \
         call had done everything but its cases. Move the .part out of the checkout by hand; then \
         {}; then run adopt with --replace and the decision that call was given (--decide, or \
         --held-reason, with --held-reference when that call had one), if any, which moves the \
         cases of the package that call took to the state directory's adopt/ and writes the whole \
         set again",
        new_directory_step(&format!("{PACKS_DIRECTORY}/{package_id}"), "that call")
    )
}

/// Workflow #463 AC-1: `from`, a case directory of the checkout (`entry` relative to it), and its
/// kept place `kept` lie on one volume, so that the case can be moved there; otherwise
/// `replace_across_volumes`, as for the replaced pack directory.
fn same_volume(
    worktree: &dyn AdoptWorktree,
    from: &Path,
    kept: &Path,
    entry: &str,
) -> LabResult<()> {
    let volume = |path: &Path| {
        worktree.volume(path).map_err(|error| {
            refused(
                adopt(AdoptProblem::ReplaceAcrossVolumes)
                    .entry(entry)
                    .path(kept)
                    .caused_by(Outcome::from_io_error(&error, IoOp::Inspect).path(path)),
                format!(
                    "the volume of {} cannot be told: {error}; nothing was written",
                    path.display()
                ),
            )
        })
    };
    let checkout_volume = volume(from)?;
    let kept_volume = volume(kept)?;
    if checkout_volume == kept_volume {
        return Ok(());
    }
    Err(refused(
        adopt(AdoptProblem::ReplaceAcrossVolumes)
            .entry(entry)
            .path(kept),
        format!(
            "--replace moves the earlier cases of the package, {entry} among them, to their kept \
             place {}, and the checkout's volume {checkout_volume} is not the state directory's \
             {kept_volume}, where a directory cannot be renamed to; give a --state-dir on the \
             checkout's volume {checkout_volume} (the recording may stay where it is: name its \
             directory by --session-state-dir, which adopt only reads); nothing was written",
            kept.display()
        ),
    ))
}

/// Workflow #463 AC-1: what [`plan_cases`] reads.
struct CaseInputs<'a> {
    /// The cases of the recording's last stop.
    taken: &'a RecordedCases,
    /// What `regression/<game>/` of the checkout holds of the package's cases.
    checkout: &'a CheckoutCases,
    /// `regression/<game>`.
    regression_relative: &'a str,
    package_id: &'a str,
    /// The package as this adoption writes it: the check tree's `packs-root/<package_id>/`, the
    /// very files it writes into `packs/<package_id>/`.
    package_dir: &'a Path,
}

/// Workflow #463 AC-1: the cases an adoption writes, after every check passed.
struct CasePlan {
    /// The recording's `out/<package_id>.cases/`.
    from: PathBuf,
    /// The cases to write, in the order of their names.
    cases: Vec<PlannedCase>,
    /// The earlier set of the package's cases, by their names in `regression/<game>/`, moved
    /// before the new set is written (with `--replace` only; without it they refuse).
    replaced: Vec<String>,
    /// The cases taken that the runner did not prove whole.
    not_proven: Vec<CaseNotProven>,
}

/// Workflow #463 AC-1: one case to write.
struct PlannedCase {
    /// Its directory's name in `regression/<game>/`, `<package_id>.<case>`.
    name: String,
    /// Its files as the runner read and ran them, each by its name in the directory.
    files: Vec<(String, Vec<u8>)>,
}

/// Workflow #463 AC-1 (`contracts/standard-pack-authoring.md`, "The cases"): the cases of the
/// recording's set, each checked before the first write. Its directory `<package_id>.<case>` of
/// `regression/<game>/` is one path segment whose `.part` stays within a file name, the case's
/// name holding no dot, so that the name tells its package and its case apart
/// (`case_name_invalid` otherwise, at the case in the recording); it is free or holds a case of
/// the earlier set (`case_place_taken` at it otherwise). Then each case is run by the one runner
/// on the package as this adoption writes it, admitted once (`lab`'s runner): a case whose
/// verdict is `failed` refuses the adoption (`case_failed`; fix round 1: the details list the
/// cases that did not pass as `resource regression` lists them, by its one rule, with
/// `failed_total` and `listed`); `partial` and `not_run` cases are taken and named with the
/// runner's words.
fn plan_cases(inputs: &CaseInputs<'_>, lab: &dyn AdoptLab) -> LabResult<CasePlan> {
    let mut names = Vec::new();
    for case in &inputs.taken.names {
        let name = format!("{}.{case}", inputs.package_id);
        if case.contains('.')
            || !package_id_names_one_path_segment(&name)
            || name.len() + PART.len() + 1 > NAME_MAX_BYTES
        {
            let at = inputs.taken.directory.join(case);
            return Err(refused(
                named_package(adopt(AdoptProblem::CaseNameInvalid), inputs.package_id).path(&at),
                format!(
                    "the case {} of the recording names no directory {name} of {}: a case's name \
                     holds no dot, and {name} with its .{PART} is one path segment of at most \
                     {NAME_MAX_BYTES} bytes; nothing was written",
                    at.display(),
                    inputs.regression_relative
                ),
            ));
        }
        if inputs.checkout.others.contains(&name.to_ascii_lowercase()) {
            let entry = format!("{}/{name}", inputs.regression_relative);
            return Err(refused(
                named_package(adopt(AdoptProblem::CasePlaceTaken), inputs.package_id).entry(&entry),
                format!(
                    "{entry}, where the recording's case {case} goes, is there and is no case an \
                     adoption took from a recording of {} (no case.json naming the package with a \
                     recording's source): adopt neither moves nor overwrites it; move it out of \
                     the checkout or rename it, then run adopt again; nothing was written",
                    inputs.package_id
                ),
            ));
        }
        names.push(name);
    }
    let at = names
        .iter()
        .zip(&inputs.taken.names)
        .map(|(name, case)| AdoptCaseAt {
            path: format!("{}/{name}", inputs.regression_relative),
            directory: inputs.taken.directory.join(case),
        })
        .collect::<Vec<_>>();
    let ran = lab.run_cases(inputs.package_dir, &at)?;
    if ran.runs.len() != at.len() {
        return Err(LabError::device(format!(
            "the Lab answered {} runs for the {} regression cases of the recording; nothing was \
             written",
            ran.runs.len(),
            at.len()
        )));
    }
    let mut failed = 0_usize;
    let mut first_failed = None;
    let mut not_proven = Vec::new();
    let mut cases = Vec::new();
    for ((case, name), run) in inputs.taken.names.iter().zip(names).zip(ran.runs) {
        match run.verdict {
            AdoptCaseVerdict::Failed => {
                failed += 1;
                if first_failed.is_none() {
                    first_failed = Some(case);
                }
                continue;
            }
            AdoptCaseVerdict::Passed => {}
            AdoptCaseVerdict::NotProven { verdict, reasons } => {
                not_proven.push(CaseNotProven {
                    case: case.clone(),
                    verdict,
                    reasons,
                });
            }
        }
        cases.push(PlannedCase {
            name,
            files: run.files,
        });
    }
    // Fix round 1: the failing cases are listed by `resource regression`'s one rule, so that the
    // refusal fits the Lab's own line and the MCP server's answer budget whatever their number.
    if let Some(first) = first_failed {
        let outcome = named_package(adopt(AdoptProblem::CaseFailed), inputs.package_id)
            .path(&inputs.taken.directory)
            .count(failed);
        return Err(refused(
            outcome.clone(),
            format!(
                "{failed} of the {} regression cases of the recording fail on the package as this \
                 adoption writes it, the first {first}; the details' cases list the first {} of \
                 the {} cases that did not pass, the failed ones first, as resource regression \
                 lists them; nothing was written",
                inputs.taken.names.len(),
                ran.listed.len(),
                ran.not_passed
            ),
        )
        .with_details(json!({
            "outcome": outcome.envelope(Detail::Full),
            "failed_total": ran.not_passed,
            "listed": ran.listed.len(),
            "cases": ran.listed
        })));
    }
    Ok(CasePlan {
        from: inputs.taken.directory.clone(),
        cases,
        replaced: inputs.checkout.recorded.clone(),
        not_proven,
    })
}

/// Workflow #463 AC-1: what [`write_cases`] writes.
struct CaseWrites<'a> {
    /// `regression/<game>/` of the checkout.
    directory: &'a Path,
    /// Where the earlier set goes, `<state>/adopt/<package_id>.cases.old-<ms>/`.
    kept: &'a Path,
    plan: &'a CasePlan,
}

/// Workflow #463 AC-1 (`contracts/standard-pack-authoring.md`, "What it writes"): the cases,
/// written last, after `done`, what the adoption wrote and moved before, in order: the earlier
/// set moved one directory at a time into its kept place, then each case written whole as
/// `<name>.part/` (its files flushed to the disk) and renamed into place, in the order of the
/// plan. Every rename goes through `worktree`. A failure is `adopt_write_failed` naming where it
/// stopped and everything written or moved before; nothing is removed. The kept place, when the
/// earlier set was moved there.
fn write_cases(
    inputs: &CaseWrites<'_>,
    mut done: Vec<PathBuf>,
    worktree: &dyn AdoptWorktree,
) -> LabResult<Option<PathBuf>> {
    let mut kept = None;
    if !inputs.plan.replaced.is_empty() {
        fs::create_dir_all(inputs.kept)
            .map_err(|error| write_failed(inputs.kept, &done, &error, IoOp::CreateDir))?;
        for name in &inputs.plan.replaced {
            let from = inputs.directory.join(name);
            let to = inputs.kept.join(name);
            worktree
                .rename(&from, &to)
                .map_err(|error| write_failed(&from, &done, &error, IoOp::Rename))?;
            done.push(to);
        }
        kept = Some(inputs.kept.to_path_buf());
    }
    fs::create_dir_all(inputs.directory)
        .map_err(|error| write_failed(inputs.directory, &done, &error, IoOp::CreateDir))?;
    for case in &inputs.plan.cases {
        let part = inputs.directory.join(format!("{}.{PART}", case.name));
        fs::create_dir(&part)
            .map_err(|error| write_failed(&part, &done, &error, IoOp::CreateDir))?;
        for (file, bytes) in &case.files {
            let target = part.join(file);
            write_flushed(&target, bytes)
                .map_err(|error| write_failed(&target, &done, &error, IoOp::Write))?;
        }
        done.push(part.clone());
        let target = inputs.directory.join(&case.name);
        worktree
            .rename(&part, &target)
            .map_err(|error| write_failed(&target, &done, &error, IoOp::Rename))?;
        done.push(target);
    }
    Ok(kept)
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
    /// wins, as the composer leaves a held package's rule out; then its rule, business exactly
    /// when the composer judges it by its value (`is_judged_rule`: a maintenance role of the root
    /// files leaves it unjudged whatever it declares); then its `maintenance.json` entry.
    fn class(
        &self,
        schedule: &StandardPackSchedule,
        classification: &StandardPackClassification,
    ) -> Option<AdoptClass> {
        if self.held.is_some() {
            return Some(AdoptClass::Held);
        }
        if let Some(rule) = self.rule.and_then(|index| schedule.rules.get(index)) {
            return Some(if is_judged_rule(rule, classification) {
                AdoptClass::Business
            } else {
                AdoptClass::Maintenance
            });
        }
        self.maintenance.map(|_| AdoptClass::Maintenance)
    }

    /// The message of `pack_exists` without `--replace` (`pack_exists` the directory is there):
    /// what the checkout holds of the package, and what to do. When only `packs/<package_id>` is
    /// there and neither `schedule.json` nor `maintenance.json` names the package, the message
    /// tells two cases apart by what it reads (fix round 4): a root file adopt does not write
    /// (`pages.json`, `navigation.json`, `interrupts.json`) gives the package a maintenance role,
    /// and the message names that file and what the call accepts for such a package; or no root
    /// file names it, and an adoption was interrupted after it put the directory in place and
    /// before it wrote the root files, or the directory was put there by hand. Either way the
    /// version control tells whether the directory is committed, and each advice ends in a call
    /// the checks accept (`contracts/standard-pack-authoring.md`, "If a call was interrupted").
    fn adopted_before(&self, root: &Repository, package_id: &str, pack_exists: bool) -> String {
        let pack = format!("{PACKS_DIRECTORY}/{package_id}");
        if !self.any() {
            let committed = format!(
                "`git status -- {pack}` tells whether it is committed (an adoption begins only \
                 with {pack} absent or committed, since adopt refuses a pack directory with \
                 changes, so a change there was left by an interrupted adoption or by hand): when \
                 it shows a change, put the directory back as it was (move it out of the checkout \
                 by hand and, where it was committed, restore it with the version control)"
            );
            let replace = "--replace moves the committed directory to its kept place under the \
                           state directory's adopt/, so the state directory must lie on the \
                           checkout's volume (a --state-dir there, and with another --state-dir \
                           a recording's own directory by --session-state-dir, or its package \
                           by --package <its lab/out/ file> instead of --record-id)";
            return match root.other_role_file(package_id) {
                Some(file) => format!(
                    "{pack} is there and {file} gives {package_id} a maintenance role, while \
                     schedule.json holds no rule or held entry of it and maintenance.json no \
                     entry of it for its server: the directory belongs to the pack by {file}. \
                     Adopt takes such a package with a decision of the class held (its hold \
                     beside that role: --held-reason <text> [--held-reference <text>], or \
                     --decide) or maintenance (--decide: a rule by its trigger, or a \
                     maintenance.json entry by its uses), never business, which is refused beside \
                     that role. \
                     {committed}; then, with it absent, run adopt without --replace, or, with it \
                     committed, run adopt with --replace; {replace}; nothing was written"
                ),
                None => format!(
                    "{pack} is there and no root file names {package_id} (schedule.json holds \
                     no rule or held entry of it, maintenance.json no entry of it for its server, \
                     and pages.json, navigation.json and interrupts.json give it no role): an \
                     adoption of it was interrupted after it put that directory in place and \
                     before it wrote the root files, or the directory was put there by hand. \
                     {committed}; then, with it absent, run adopt without --replace with \
                     --decide or --held-reason, as a first adoption, or, with it committed, run \
                     adopt with --replace and --decide or --held-reason; {replace}; nothing was \
                     written"
                ),
            };
        }
        let mut held = Vec::new();
        if pack_exists {
            held.push(format!("its directory {pack}"));
        }
        if let Some(index) = self.rule {
            let id = root
                .schedule
                .rules
                .get(index)
                .map_or(package_id, |rule| rule.id.as_str());
            held.push(format!(
                "its rule {id} at /{RULES}/{index} of schedule.json"
            ));
        }
        if let Some(index) = self.held {
            held.push(format!(
                "its held entry at /{HELD}/{index} of schedule.json"
            ));
        }
        if let Some(index) = self.maintenance {
            held.push(format!("its entry at /{index} of maintenance.json"));
        }
        format!(
            "{package_id} was adopted before ({}); give --replace to adopt it again; nothing was \
             written",
            held.join(", ")
        )
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

    /// Two roles of the package in the files adopt writes, as the composer reads them, which no
    /// adoption that ended leaves (one interrupted between its two renames does): its held entry
    /// beside its `maintenance.json` entry. A rule beside that entry is never a second role: the
    /// composer judges no rule of a package such an entry names (`is_judged_rule`), so the rule is
    /// part of the package's one maintenance role (fix round 4: the branch that told a business
    /// rule there could not be reached and is gone). Only a call with `--replace` meets this check
    /// (without it the entry is refused `pack_exists` first), after `pack_dirty` passed, so
    /// `packs/<package_id>` is committed or absent and the message says so. The entry's index and
    /// the message that says what to do; `None` when the package has one role there.
    fn two_roles(&self, package_id: &str) -> Option<(usize, String)> {
        let entry = self.maintenance?;
        let held = self.held?;
        let pack = format!("{PACKS_DIRECTORY}/{package_id}");
        Some((
            entry,
            format!(
                "schedule.json holds its held entry at /{HELD}/{held} and maintenance.json its \
                 maintenance entry at /{entry}: two roles, which an adoption that writes both \
                 files leaves only when it was interrupted between its two renames (the .part of \
                 the file it did not rename is left, unless it was moved away). {pack} is \
                 committed or absent, as this call with --replace found it (it refuses a pack \
                 directory with changes, and moves a committed one only to a state directory on \
                 the checkout's volume), so nothing comes before: to finish that adoption, run \
                 adopt again with --replace, this --state-dir and the decision that call was given \
                 (--decide, or --held-reason, with --held-reference when that call had one), which \
                 writes one role. To undo it: put {pack} back as it was before \
                 that call (absent, or as its earlier commit) and restore schedule.json and \
                 maintenance.json with the version control. Nothing was written"
            ),
        ))
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
        Self::of_document(document, path, schedule)
    }

    /// The document `document` read from `path`, checked on its own and against the file's pools.
    /// Workflow #463 S3-4: also a document the call built ([`held_document`]), whose `path` is
    /// empty, as no file was read.
    fn of_document(
        document: Value,
        path: &Path,
        schedule: &StandardPackSchedule,
    ) -> LabResult<Self> {
        let invalid = |pointer: &str, message: &str| decision_invalid(path, pointer, message);
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

/// Workflow #463 S3-4: the decision document `{schema_version, class: "held", held: {reason,
/// reference?}}` of `--held-reason` and `--held-reference`, which [`Decision::of_document`] reads
/// as it reads a document given by its path; the texts as given, so that its reader checks them.
fn held_document(rationale: &str, reference: Option<&str>) -> Value {
    let mut held = Map::new();
    held.insert(HELD_REASON.to_owned(), json!(rationale));
    if let Some(reference) = reference {
        held.insert(HELD_REFERENCE.to_owned(), json!(reference));
    }
    let mut document = Map::new();
    document.insert(SCHEMA_VERSION.to_owned(), json!(PACK_DECISION_SCHEMA));
    document.insert("class".to_owned(), json!("held"));
    document.insert(HELD.to_owned(), Value::Object(held));
    Value::Object(document)
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
    /// The package's rule after the adoption, as written. Workflow #477 CV-2b: of a switched root,
    /// with its business part, which `pack-catalog.json` holds and `schedule.json` does not
    /// ([`Self::written_rule`]), as the composer reads the rule.
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
    /// Workflow #477 CV-2b: the root is switched and the rule's business part is written into
    /// `pack-catalog.json`, not into the rule.
    switched: bool,
    /// CV-2b: of a switched root, the bytes of `pack-catalog.json` the check composes (as read
    /// when the adoption leaves the file as it is), and those adopt writes when it changes.
    check_catalog: Option<Vec<u8>>,
    catalog_bytes: Option<Vec<u8>>,
    /// CV-2b: of a switched root, the package's entry of `pack-catalog.json` after the adoption;
    /// `None` when it has none.
    catalog_entry: Option<Value>,
}

/// What [`Plan::make`] reads.
struct PlanInputs<'a> {
    /// Workflow #477 CV-2b: the checkout, which a refusal of the root as it would be written names.
    repo: &'a Path,
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
        // Workflow #477 CV-2b (`contracts/standard-pack-authoring.md`, "On a switched root"): of a
        // switched root, every business member the adoption writes into its rule goes into the
        // catalog's part for that rule instead. A rule of the package that writes a business member
        // there itself is not touched: the check of the tree refuses it as the composer does
        // (`catalog_business_member`), as before this slice.
        let routing = root.catalog.as_ref().filter(|_| {
            inputs.existing.rule.is_none_or(|index| {
                document
                    .get(RULES)
                    .and_then(Value::as_array)
                    .and_then(|rules| rules.get(index))
                    .and_then(Value::as_object)
                    .is_none_or(|rule| !writes_business(rule))
            })
        });
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
            switched: routing.is_some(),
            check_catalog: root.catalog.as_ref().map(|catalog| catalog.bytes.clone()),
            catalog_bytes: None,
            catalog_entry: routing.and_then(|catalog| catalog.read.entry(inputs.package_id)),
        };
        // CV-2b: where the package's rule stands in the file as it would be written, and its id as
        // the call read it (none for a rule the call adds).
        let mut rule_index = None;
        let mut rule_id_read = None;
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
                .map(|rule| (rule, tier_of(rule, &root.classification)))
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
            rule_id_read = before.get(ID).and_then(Value::as_str).map(str::to_owned);
            // Workflow #477 CV-2b: of a switched root, the rule with its business part as the
            // composer gives it the catalog's, for this call's own reading of the rule (a value the
            // model wrote, what the decision changes, the words `requires` needs, the coverage and
            // the value's wait); `schedule.json` is written without it.
            if let Some(catalog) = routing {
                rule.extend(catalog.read.rule_members(
                    inputs.package_id,
                    rule_id_read.as_deref().unwrap_or_default(),
                ));
            }
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
                // Fix round 4: whether the composer would judge the rule by its value once the
                // call wrote the root files, were it to declare nothing (its own `is_judged_rule`;
                // the decision's uses make the package maintenance-role, as an entry the class
                // maintenance keeps does).
                let judged_undeclared = root.schedule.rules.get(index).is_some_and(|read| {
                    let mut after = root.classification.clone();
                    if decision.uses.is_some() {
                        after.maintenance.insert(inputs.package_id.to_owned());
                    }
                    let undeclared = ScheduleRule {
                        maintenance: false,
                        ..read.clone()
                    };
                    is_judged_rule(&undeclared, &after)
                });
                plan.named(&mut rule, decision, judged_undeclared);
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
            // CV-2b: the entry as `schedule.json` holds it, of a switched root without its
            // business part.
            let written = if routing.is_some() {
                without_business(&rule)
            } else {
                rule.clone()
            };
            if written != before {
                plan.replaced.push(RootEntryChange {
                    file: STANDARD_PACK_SCHEDULE_FILE.to_owned(),
                    json_pointer: format!("/{RULES}/{index}"),
                    before: Value::Object(before),
                    after: Some(Value::Object(written)),
                });
            }
            plan.rule = Some(rule);
            rule_index = Some(index);
        } else if wants_rule {
            let decision = inputs.decision.ok_or_else(|| {
                refused(
                    adopt(AdoptProblem::DecisionInvalid).json_pointer(""),
                    "a first adoption needs --decide <decision.json>, or --held-reason <text> \
                     to adopt the package as held"
                        .to_owned(),
                )
            })?;
            let rule_id = decision
                .overrides
                .get(ID)
                .and_then(Value::as_str)
                .unwrap_or(inputs.package_id)
                .to_owned();
            let rule = plan.new_rule(inputs, decision, &inputs_of_rule(&rule_id))?;
            let rules = array_member(&mut document, RULES)?;
            rule_index = Some(rules.len());
            rules.push(Value::Object(rule.clone()));
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
                // Workflow #477 CV-2b: before `catalog_business`, which a cut-over names last, so
                // that a switched root names them as adopting before the cut-over and cutting over
                // would; at the end of a file that does not name it, as before.
                let names = array_member(&mut document, REQUIRES)?;
                let at = names
                    .iter()
                    .position(|name| name.as_str() == Some(CATALOG_BUSINESS_SUBSYSTEM))
                    .unwrap_or(names.len());
                let tail = names.split_off(at);
                names.extend(words);
                names.extend(tail);
            } else {
                document = after_schema_version(document, REQUIRES, Value::Array(words));
            }
        }

        // Workflow #477 CV-2b (`contracts/standard-pack-authoring.md`, "On a switched root"): of a
        // switched root the rule's business part is written into the catalog's part for the rule
        // (`AdoptionCatalog::adopted`, from the rule as the file holds it with its business members,
        // read by the schedule's one reader, as the writer's `--refresh` reads a rule), and the rule
        // is written without it.
        if let Some(catalog) = routing
            && let Some(index) = rule_index
        {
            let full_bytes = canonical(&Value::Object(document.clone()))?;
            let as_written = parse_standard_pack_schedule(&full_bytes)
                .map_err(|issue| root_refused(inputs.repo, &root.game, &issue))?;
            let adoption = catalog
                .read
                .adopted(&as_written, index, rule_id_read.as_deref())
                .map_err(|issue| root_refused(inputs.repo, &root.game, &issue))?;
            if let Some(Value::Object(rule)) = array_member(&mut document, RULES)?.get_mut(index) {
                let stripped = without_business(rule);
                *rule = stripped;
            }
            if let Some(adoption) = adoption
                && adoption.bytes != catalog.bytes
            {
                require_canonical(&catalog.bytes, STANDARD_PACK_CATALOG_FILE)?;
                let CatalogAdoption {
                    bytes,
                    pointer,
                    before,
                    after,
                } = adoption;
                if let Some(before) = before {
                    plan.replaced.push(RootEntryChange {
                        file: STANDARD_PACK_CATALOG_FILE.to_owned(),
                        json_pointer: pointer,
                        before,
                        after: Some(after.clone()),
                    });
                }
                plan.catalog_entry = Some(after);
                plan.check_catalog = Some(bytes.clone());
                plan.catalog_bytes = Some(bytes);
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

    /// The package's rule as `schedule.json` holds it after the adoption: Workflow #477 CV-2b, of
    /// a switched root without its business part, which `pack-catalog.json` holds.
    fn written_rule(&self) -> Option<Map<String, Value>> {
        self.rule.as_ref().map(|rule| {
            if self.switched {
                without_business(rule)
            } else {
                rule.clone()
            }
        })
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
                        decision.invalid(&format!("/overrides/{member}"), &underivable(member))
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
    /// the model wrote stays as it is (answered in `model_kept`). `judged_undeclared`: the
    /// composer would judge the rule by its value after the call were it to declare nothing.
    fn named(
        &mut self,
        rule: &mut Map<String, Value>,
        decision: &Decision,
        judged_undeclared: bool,
    ) {
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
        // Fix round 4: a maintenance rule by its trigger declares itself maintenance. Without a
        // trigger (by its uses, or naming other members only) the rule declares it only where the
        // composer would otherwise judge it by its value; beside its package's maintenance.json
        // entry, or a role another root file gives, the composer does not judge it already, and
        // a declaration would only change what the composer checks of it (`limited` is refused on
        // a rule that declares maintenance, and its priority is compared with the maintenance
        // tier's), so its `maintenance` member stays as it was written.
        if decision.class != AdoptClass::Maintenance {
            rule.shift_remove(SCHEDULE_RULE_MAINTENANCE_MEMBER);
        } else if decision.trigger.is_some() || judged_undeclared {
            self.give(SCHEDULE_RULE_MAINTENANCE_MEMBER, Value::Bool(true));
            rule.insert(
                SCHEDULE_RULE_MAINTENANCE_MEMBER.to_owned(),
                Value::Bool(true),
            );
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

/// The tier of `rule`: business exactly when the composer judges it by its value
/// (`is_judged_rule`).
fn tier_of(rule: &ScheduleRule, classification: &StandardPackClassification) -> Tier {
    if is_judged_rule(rule, classification) {
        Tier::Business
    } else {
        Tier::Maintenance
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
/// under `any` when there are several: after one normal end inside the activity window the rule is
/// not dispatched again in that window. `None` when the package declares no such key, and the
/// decision's `overrides` must give it.
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

/// Why nothing gives the derived member `member`, which the decision's `overrides` must then name.
fn underivable(member: &str) -> String {
    match member {
        FEEDBACK_STOP => "nothing gives feedback_stop: the package declares no outcome key of a \
                          normal end, and a feedback_stop is never copied from another rule of \
                          the file; name it in overrides, on a key the package declares"
            .to_owned(),
        PRIORITY => "nothing gives priority: the file's rules hold neither exactly two \
                     priorities nor one their tier agrees on; name it in overrides"
            .to_owned(),
        _ => format!(
            "nothing gives {member} (the package, the file's rules of its tier and the \
             contract's defaults): name it in overrides"
        ),
    }
}

/// How a derived member was derived, in words, where `from` alone does not say it.
fn derivation(inputs: &DeriveInputs<'_>, member: &str, from: DerivedFrom) -> Option<String> {
    match (member, from) {
        (FEEDBACK_STOP, _) => Some(format!(
            "derived from the package: this rule's own run ended with a key its package declares \
             for a normal end ({:?}); after one such end inside the activity window the rule is \
             not dispatched again in that window",
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

/// What [`outcome_keys_declared`] reads.
struct KeyInputs<'a> {
    repo: &'a Path,
    plan: &'a Plan,
    package: &'a Package,
    decision: Option<&'a Decision>,
    package_id: &'a str,
    /// The schedule as the check of the tree compiled it, as the Runtime compiles it.
    compiled: Option<&'a CompiledCatalog>,
}

/// One object of `schedule.json` that reads a task's outcome (`kind` `outcome`): an outcome
/// predicate of a rule, a pool's observation, an activity target's measure.
struct OutcomeRead {
    /// Its pointer in the file as it would be written.
    pointer: String,
    task_id: String,
    /// The key it names; `None` when it names none as a text.
    key: Option<String>,
}

/// Workflow #464 P1 (fix rounds 1 and 2): the Runtime refuses every dispatch of a task whose
/// outcome the schedule reads for a key its package does not declare
/// (`contracts/scheduling/README.md`, the outcome declaration). So, before anything is written:
/// - every key the compiled schedule reads of the adopted rule's outcome is one its package
///   declares, as the policy crate's own `referenced_outcome_keys`, which the Runtime runs,
///   collects them: every composed rule's trigger and stop, every pool's observation and every
///   activity target's measure;
/// - every key an outcome predicate of the adopted rule as it would be written (given, derived or
///   kept) reads of another task is one the package of that task declares, or that task would be
///   refused at every dispatch and the rule would never come due.
///
/// A refusal is `decision_invalid` at the reading object's key: in the decision when the decision
/// gave the member it lies in (the trigger, `overrides.feedback_stop`), in `schedule.json` as it
/// would be written otherwise. Its message names who reads (the rule, the pool or the activity
/// profile, by its id) and its details carry the keys the package of the task read declares.
fn outcome_keys_declared(inputs: &KeyInputs<'_>) -> LabResult<()> {
    let plan = inputs.plan;
    let (Some(rule_id), None, Some(bytes), Some(compiled)) = (
        plan.rule_id(),
        &plan.held,
        &plan.check_schedule,
        inputs.compiled,
    ) else {
        return Ok(());
    };
    let document = serde_json::from_slice::<Value>(bytes).map_err(|error| {
        repository_invalid(
            STANDARD_PACK_SCHEDULE_FILE,
            format!("schedule.json as it would be written does not read: {error}"),
        )
    })?;
    let own_index = document
        .get(RULES)
        .and_then(Value::as_array)
        .and_then(|rules| {
            rules.iter().position(|rule| {
                rule.get(PACKAGE_ID).and_then(Value::as_str) == Some(inputs.package_id)
            })
        });
    let declared = inputs.package.declared_outcome_keys();
    let reads = outcome_reads(&document);

    // The rule's own outcome, every reader of it as the Runtime collects them.
    let undeclared = compiled
        .referenced_outcome_keys(&rule_id)
        .into_iter()
        .filter(|key| !declared.contains(key))
        .collect::<BTreeSet<_>>();
    if !undeclared.is_empty() {
        let read = reads.iter().find(|read| {
            read.task_id == rule_id
                && read
                    .key
                    .as_ref()
                    .is_some_and(|key| undeclared.contains(key))
        });
        let Some(read) = read else {
            let error = refused(
                adopt(AdoptProblem::DecisionInvalid)
                    .entry(STANDARD_PACK_SCHEDULE_FILE)
                    .json_pointer(""),
                format!(
                    "schedule.json as it would be written reads the rule {rule_id}'s outcome on \
                     the keys {undeclared:?}, which its package {} does not declare (it declares \
                     {declared:?}); the Runtime refuses every dispatch of {rule_id}: name one of \
                     the declared keys",
                    inputs.package_id
                ),
            );
            let outcome = cause_of(&error);
            return Err(error.with_details(json!({
                "outcome": outcome.envelope(Detail::Full),
                "declared_outcome_keys": declared
            })));
        };
        let key_pointer = format!("{}/outcome_key", read.pointer);
        let message = format!(
            "{} reads at {key_pointer:?} the rule {rule_id}'s outcome on the key {:?}, which its \
             package {} does not declare (it declares {declared:?}); the Runtime refuses every \
             dispatch of {rule_id}: name one of the declared keys",
            reader(&document, &read.pointer),
            read.key,
            inputs.package_id
        );
        return Err(key_refused(
            inputs, own_index, read, &message, &declared, None,
        ));
    }

    // What the rule's own predicates read of another task.
    let Some(own_index) = own_index else {
        return Ok(());
    };
    let own_prefix = format!("/{RULES}/{own_index}/");
    let mut declared_by = BTreeMap::<String, Vec<String>>::new();
    for read in reads
        .iter()
        .filter(|read| read.pointer.starts_with(&own_prefix) && read.task_id != rule_id)
    {
        let key_pointer = format!("{}/outcome_key", read.pointer);
        let Some(read_package) = package_of_rule(&document, &read.task_id) else {
            let message = format!(
                "the rule {rule_id} reads at {key_pointer:?} the outcome of the task {}, which no \
                 rule of schedule.json as it would be written composes",
                read.task_id
            );
            return Err(key_refused(
                inputs,
                Some(own_index),
                read,
                &message,
                &[],
                None,
            ));
        };
        let keys = match declared_by.get(&read_package) {
            Some(keys) => keys.clone(),
            None => {
                let keys = if read_package == inputs.package_id {
                    declared.clone()
                } else {
                    Package::read(&inputs.repo.join(PACKS_DIRECTORY).join(&read_package))?
                        .declared_outcome_keys()
                };
                declared_by.insert(read_package.clone(), keys.clone());
                keys
            }
        };
        if read.key.as_ref().is_some_and(|key| keys.contains(key)) {
            continue;
        }
        let message = format!(
            "the rule {rule_id} reads at {key_pointer:?} the outcome of the task {} on the key \
             {:?}, which its package {read_package} does not declare (it declares {keys:?}); the \
             Runtime refuses every dispatch of {}, so {rule_id} would never come due by it: name \
             one of the declared keys",
            read.task_id, read.key, read.task_id
        );
        return Err(key_refused(
            inputs,
            Some(own_index),
            read,
            &message,
            &keys,
            Some((read.task_id.as_str(), read_package.as_str())),
        ));
    }
    Ok(())
}

/// The refusal of `read`, whose key the package of the task it reads does not declare
/// (`declared`), `message` in words: `decision_invalid` in the decision at the pointer of the
/// member it gave when `read` lies in that member of the adopted rule (at `own_index`), in
/// `schedule.json` as it would be written otherwise. The details carry `declared` and, for a read
/// of another task, that task and its package.
fn key_refused(
    inputs: &KeyInputs<'_>,
    own_index: Option<usize>,
    read: &OutcomeRead,
    message: &str,
    declared: &[String],
    other: Option<(&str, &str)>,
) -> LabError {
    let key_pointer = format!("{}/outcome_key", read.pointer);
    // Where the predicate came from: the decision's trigger or stop, else the file.
    let given = own_index.zip(inputs.decision).and_then(|(index, decided)| {
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
    let error = match given {
        Some((decided, decided_pointer)) => decided.invalid(&decided_pointer, message),
        None => refused(
            adopt(AdoptProblem::DecisionInvalid)
                .entry(STANDARD_PACK_SCHEDULE_FILE)
                .json_pointer(bounded_pointer(&key_pointer)),
            format!("schedule.json as it would be written: {message}"),
        ),
    };
    let outcome = cause_of(&error);
    let details = match other {
        Some((task_id, package_id)) => json!({
            "outcome": outcome.envelope(Detail::Full),
            "declared_outcome_keys": declared,
            "task_id": task_id,
            "package_id": package_id
        }),
        None => json!({
            "outcome": outcome.envelope(Detail::Full),
            "declared_outcome_keys": declared
        }),
    };
    error.with_details(details)
}

/// Every object of `document` (`schedule.json` as it would be written) that reads a task's
/// outcome, in file order, of what the Runtime compiles: the rules the composer composes (not a
/// held package's), the pools and the activity profiles.
fn outcome_reads(document: &Value) -> Vec<OutcomeRead> {
    let held = document
        .get(HELD)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get(PACKAGE_ID).and_then(Value::as_str))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut reads = Vec::new();
    for member in [RULES, POOLS, ACTIVITY] {
        let Some(items) = document.get(member).and_then(Value::as_array) else {
            continue;
        };
        for (index, item) in items.iter().enumerate() {
            let held_rule = member == RULES
                && item
                    .get(PACKAGE_ID)
                    .and_then(Value::as_str)
                    .is_some_and(|package_id| held.contains(package_id));
            if !held_rule {
                collect_outcome_reads(item, &format!("/{member}/{index}"), &mut reads);
            }
        }
    }
    reads
}

/// Each object below `value` (at `pointer`) that reads a task's outcome, by its pointer.
fn collect_outcome_reads(value: &Value, pointer: &str, reads: &mut Vec<OutcomeRead>) {
    match value {
        Value::Object(members) => {
            if members.get("kind").and_then(Value::as_str) == Some("outcome")
                && let Some(task_id) = members.get("task_id").and_then(Value::as_str)
            {
                reads.push(OutcomeRead {
                    pointer: pointer.to_owned(),
                    task_id: task_id.to_owned(),
                    key: members
                        .get("outcome_key")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
            for (name, member) in members {
                collect_outcome_reads(member, &format!("{pointer}/{}", segment(name)), reads);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_outcome_reads(item, &format!("{pointer}/{index}"), reads);
            }
        }
        _ => {}
    }
}

/// Who holds the object at `pointer` of `document`: the rule, the pool or the activity profile, by
/// its id and its pointer.
fn reader(document: &Value, pointer: &str) -> String {
    let mut segments = pointer.split('/').skip(1);
    let (Some(member), Some(index)) = (segments.next(), segments.next()) else {
        return format!("the object at {pointer:?}");
    };
    let what = match member {
        RULES => "the rule",
        POOLS => "the pool",
        _ => "the activity profile",
    };
    let id = index
        .parse::<usize>()
        .ok()
        .and_then(|index| document.get(member)?.get(index)?.get(ID)?.as_str())
        .unwrap_or("without an id");
    format!("{what} {id} at /{member}/{index}")
}

/// The package of the rule `task_id` of `document` that the composer composes (not a held
/// package's).
fn package_of_rule(document: &Value, task_id: &str) -> Option<String> {
    let held = |package_id: &str| {
        document
            .get(HELD)
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get(PACKAGE_ID).and_then(Value::as_str) == Some(package_id))
            })
    };
    document
        .get(RULES)?
        .as_array()?
        .iter()
        .filter(|rule| rule.get(ID).and_then(Value::as_str) == Some(task_id))
        .filter_map(|rule| rule.get(PACKAGE_ID).and_then(Value::as_str))
        .find(|package_id| !held(package_id))
        .map(str::to_owned)
}

/// Whether the compiled schedule's task `rule_id` is a manual request, as the evaluator's
/// placement reads it (the policy crate's `is_manual_request`).
fn manual_request(compiled: Option<&CompiledCatalog>, rule_id: &str) -> bool {
    compiled
        .and_then(|compiled| {
            compiled
                .catalog()
                .tasks
                .tasks
                .iter()
                .find(|task| task.id == rule_id)
        })
        .is_some_and(is_manual_request)
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
    /// Workflow #463 S3-5: its game root `installed/<game>/`, the bundle's installed layout the
    /// check read.
    installed: PathBuf,
    answer: StandardPackCheckResponse,
    /// The bundle's warnings, which the check of the installed layout does not repeat (the
    /// recognition packages its maintenance declarations leave undeclared).
    bundle_warnings: Vec<Outcome>,
    /// The catalog the check's composition of `schedule.json` compiled, as the Runtime compiles
    /// it; `None` without the file.
    compiled: Option<CompiledCatalog>,
    /// The check tree's marker, open and locked until the call ends.
    _marker: File,
}

/// What [`check_tree`] reads.
struct CheckInputs<'a> {
    repo: &'a Path,
    root: &'a Repository,
    state: &'a Path,
    /// The operation's time, which names the directory a first call makes the tree whole in.
    at_unix_ms: u64,
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
/// for this call: made whole and marked ([`CHECK_TREE_MARKER`], [`make_check_tree`]) when it is
/// not there; when it is, refused when it is a link (a symbolic link or a junction: nothing is
/// cleared through it), without its marker (it is then not the tool's own and nothing in it is
/// touched) and while another call holds the marker, and otherwise, the marker locked, cleared of
/// the marked directories a crash left beside it ([`clear_staged_leftovers`]) and emptied of
/// everything but the marker. The marker is answered open and locked, and stays so until the call
/// ends.
fn prepare_check_tree(state: &Path, repo: &Path, at_unix_ms: u64) -> LabResult<(PathBuf, File)> {
    let parent = state.join(CHECK_DIRECTORY);
    fs::create_dir_all(&parent).map_err(|error| tree_failed(&parent, &error, IoOp::CreateDir))?;
    let name = check_tree_name(repo)?;
    let tree = parent.join(&name);
    let marker = tree.join(CHECK_TREE_MARKER);
    let metadata = match fs::symlink_metadata(&tree) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            make_check_tree(&parent, &name, repo, at_unix_ms)?;
            fs::symlink_metadata(&tree)
                .map_err(|error| tree_failed(&tree, &error, IoOp::Metadata))?
        }
        Err(error) => return Err(tree_failed(&tree, &error, IoOp::Metadata)),
    };
    // On Windows a junction is a link to the standard library too (a name-surrogate reparse
    // point), so neither kind is ever followed to clear what it points at.
    if metadata.file_type().is_symlink() {
        return Err(refused(
            adopt(AdoptProblem::CheckTreeLink).path(&tree),
            format!(
                "{} is the place of the check tree of the checkout {} and is a link (a symbolic \
                 link or a junction): adopt clears nothing through a link and touches nothing \
                 where it points; move it away by hand or give another --state-dir; nothing was \
                 written",
                tree.display(),
                repo.display()
            ),
        ));
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
    // This call holds the tree: a directory a crash left beside it, in which a call was making it
    // whole, goes when it carries the marker.
    clear_staged_leftovers(&parent, &name)?;
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

/// Makes the check tree `<parent>/<name>/` whole, so that no crash leaves it unmarked: this call's
/// own directory `<name>.new-<process>-<ms>-<count>/` beside it (a name no directory holds: a
/// taken one is passed over for the next count) with the marker written, renamed into place.
/// When another call put its tree there first (two first calls at once), this call's own
/// directory, which holds only the marker this call wrote, is removed (unless that call, which
/// holds the tree, cleared it already) and the other's tree is used. A crash before the rename
/// leaves only that directory of this call, marked, which no call reads and the next call that
/// holds the tree clears ([`clear_staged_leftovers`]).
fn make_check_tree(parent: &Path, name: &str, repo: &Path, at_unix_ms: u64) -> LabResult<()> {
    let tree = parent.join(name);
    let staged = loop {
        let staged = parent.join(format!(
            "{name}.{CHECK_TREE_STAGING}-{}-{at_unix_ms}-{}",
            std::process::id(),
            CHECK_TREE_STAGED.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&staged) {
            Ok(()) => break staged,
            // The name is another directory's (a leftover of an earlier process): the next count.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(tree_failed(&staged, &error, IoOp::CreateDir)),
        }
    };
    let marker = staged.join(CHECK_TREE_MARKER);
    let text = format!(
        "actinglab standard-pack adopt keeps the check tree of the checkout {} in this \
         directory. Each call on that checkout empties it, but for this file, before it lays its \
         tree; without this file adopt leaves the directory alone.\n",
        repo.display()
    );
    write_flushed(&marker, text.as_bytes())
        .map_err(|error| tree_failed(&marker, &error, IoOp::Write))?;
    let Err(error) = fs::rename(&staged, &tree) else {
        return Ok(());
    };
    if fs::symlink_metadata(&tree).is_err() {
        return Err(noted(
            tree_failed(&tree, &error, IoOp::Rename),
            &format!(
                "the check tree made beside it stays at {}, with its marker: no call reads it, \
                 and the next call on the checkout that holds the tree clears it; nothing was \
                 written in the repository; run adopt again",
                staged.display()
            ),
        ));
    }
    // Another call's tree took the place first: this call's own directory goes, unless that call,
    // which holds the tree, cleared it already.
    gone(&marker, fs::remove_file(&marker))?;
    gone(&staged, fs::remove_dir(&staged))
}

/// Clears what calls made the checkout's check tree `<parent>/<name>/` whole in and a crash left
/// beside it ([`make_check_tree`]: `<name>.new-<process>-<ms>-<count>/`), each only when it
/// carries the marker: a directory of that name without it is not the tool's and is left alone,
/// and a link of that name is refused `check_tree_link` before anything is cleared. Called while
/// this call holds the tree: the tree is in place, so no call renames such a directory into it any
/// more, and a call still making its own finds the tree there and goes on with it.
fn clear_staged_leftovers(parent: &Path, name: &str) -> LabResult<()> {
    let prefix = format!("{name}.{CHECK_TREE_STAGING}-");
    let names = fs::read_dir(parent)
        .map_err(|error| tree_failed(parent, &error, IoOp::ReadDir))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|error| tree_failed(parent, &error, IoOp::ReadDir))?;
    let mut marked = Vec::new();
    for entry_name in names {
        let Some(rest) = entry_name
            .to_str()
            .and_then(|text| text.strip_prefix(prefix.as_str()))
        else {
            continue;
        };
        if !staged_suffix(rest) {
            continue;
        }
        let path = parent.join(&entry_name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(tree_failed(&path, &error, IoOp::Metadata)),
        };
        if metadata.file_type().is_symlink() {
            return Err(refused(
                adopt(AdoptProblem::CheckTreeLink).path(&path),
                format!(
                    "{} is named as a directory adopt makes the check tree {} whole in, and is a \
                     link (a symbolic link or a junction): adopt clears nothing through a link and \
                     touches nothing where it points; move it away by hand; nothing was written",
                    path.display(),
                    parent.join(name).display()
                ),
            ));
        }
        if metadata.is_dir()
            && fs::symlink_metadata(path.join(CHECK_TREE_MARKER))
                .is_ok_and(|marker| marker.is_file())
        {
            marked.push(path);
        }
    }
    for path in marked {
        gone(&path, fs::remove_dir_all(&path))?;
    }
    Ok(())
}

/// Whether `rest`, what follows `<name>.new-` in a name beside the check tree, is
/// `<process>-<ms>-<count>` as [`make_check_tree`] writes it: three decimal numbers.
fn staged_suffix(rest: &str) -> bool {
    let fields = rest.split('-').collect::<Vec<_>>();
    fields.len() == 3
        && fields
            .iter()
            .all(|field| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()))
}

/// `removed`, the removal of `path`: done, or found done already (`NotFound`, another call that
/// holds the check tree cleared it); any other failure is `adopt_write_failed` at `path`.
fn gone(path: &Path, removed: io::Result<()>) -> LabResult<()> {
    match removed {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(tree_failed(path, &error, IoOp::Remove)),
    }
}

/// B.4 item 6: the tree as it would be written, in the checkout's check tree
/// ([`prepare_check_tree`]): `packs-root/` (the repository's pack directories, the package's own as
/// it would be written), `root/` (the root files adopt changes), `bundle/` (`package bundle` with
/// every root file the repository holds) and `installed/<game>/` (its installed layout, checked by
/// `standard-pack check`). Both compose the schedule with one instance per server; a composition
/// that refuses the file as it would be written is `rule_refused` with the composer's pointer, any
/// other refusal is answered as it is. Nothing is written in the repository. Workflow #463 S2b-1:
/// its five stages entered in `stages` (`check_tree` to `root_check`).
fn check_tree(inputs: &CheckInputs<'_>, stages: &mut Stages<'_>) -> LabResult<Checked> {
    stages.enter(AdoptStage::CheckTree);
    let (tree, marker) = prepare_check_tree(inputs.state, inputs.repo, inputs.at_unix_ms)?;
    stages.enter(AdoptStage::CopyPackages);
    let packs_root = tree.join(CHECK_PACKS_ROOT);
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
    // Workflow #477 CV-2b: of a switched root, the catalog as it would be written beside
    // `schedule.json`, which the bundle's composition takes the rules' business part from.
    if let Some(bytes) = &inputs.plan.check_catalog {
        let path = staged_root.join(STANDARD_PACK_CATALOG_FILE);
        write_new(&path, bytes).map_err(|error| tree_failed(&path, &error, IoOp::Write))?;
    }
    stages.enter(AdoptStage::Bundle);
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
    stages.enter(AdoptStage::InstalledTree);
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
    // Workflow #477 CV-2b: of a switched root, the catalog in the installed layout too, as the
    // build carries it into the zip and `place` lays it beside `bundle.json`.
    if let Some(bytes) = &inputs.plan.check_catalog {
        let target = installed.join(STANDARD_PACK_CATALOG_FILE);
        write_new(&target, bytes).map_err(|error| tree_failed(&target, &error, IoOp::Write))?;
    }
    stages.enter(AdoptStage::RootCheck);
    let (answer, compiled) = check_standard_pack_root_with_catalog(StandardPackCheckRequest {
        root: installed.clone(),
    })
    .map_err(|error| composed(error, &tree))?;
    Ok(Checked {
        tree,
        installed,
        answer,
        bundle_warnings: bundle.warnings,
        compiled,
        _marker: marker,
    })
}

/// A refusal of the check tree: `rule_refused` with the composer's pointer when it refuses
/// `schedule.json`, the refusal as it is otherwise; either way nothing was written in the
/// repository.
fn composed(mut error: LabError, tree: &Path) -> LabError {
    let cause = cause_of(&error);
    // Workflow #477 CV-2b: or `pack-catalog.json`, which of a switched root the check composes
    // beside it, as it would be written; no other root's check tree holds one.
    let file = text_value(&cause, "path")
        .and_then(|path| Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .and_then(|name| {
            [STANDARD_PACK_SCHEDULE_FILE, STANDARD_PACK_CATALOG_FILE]
                .into_iter()
                .find(|root_file| *root_file == name)
        })
        .filter(|_| error.code == SharedCode::StandardPackRootInvalid.as_str());
    let Some(file) = file else {
        error.message = format!(
            "{}; nothing was written in the repository; the check tree is at {}",
            error.message,
            tree.display()
        );
        return error;
    };
    let pointer = text_value(&cause, "json_pointer")
        .map(bounded_pointer)
        .unwrap_or_default();
    refused(
        adopt(AdoptProblem::RuleRefused)
            .entry(file)
            .json_pointer(pointer.clone())
            .caused_by(cause),
        format!(
            "the composer refuses {file} as it would be written at {pointer:?}: {}; the \
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

/// Workflow #463 AD-1 (`contracts/standard-pack-authoring.md`, "The warnings of other packages"):
/// the warnings `named`, each beside the pack it names ([`with_packs_named`]), as the answer gives
/// them: whole and in their order, each that names the adopted package `package_id` or no package
/// of the root; counted by code, each that names another package. None is left out.
fn of_the_package(
    named: Vec<(Outcome, Option<String>)>,
    package_id: &str,
) -> (Vec<Outcome>, OtherPackagesWarnings) {
    let mut whole = Vec::new();
    let mut others = OtherPackagesWarnings::default();
    for (warning, pack) in named {
        match pack {
            Some(pack) if pack != package_id => {
                others.total += 1;
                *others
                    .counts
                    .entry(warning.code().as_str().to_owned())
                    .or_default() += 1;
            }
            _ => whole.push(warning),
        }
    }
    (whole, others)
}

/// What [`write_adoption`] writes.
struct WriteInputs<'a> {
    repo: &'a Path,
    files: &'a BTreeMap<String, Vec<u8>>,
    staged: &'a Path,
    pack_dir: &'a Path,
    /// `packs/<package_id>`, relative to the checkout, as a message names it.
    pack_relative: &'a str,
    /// Where the replaced pack directory goes, when it is replaced.
    kept: Option<&'a Path>,
    plan: &'a Plan,
    /// `schedule.json` (absent: `None`) and `maintenance.json` as the call read them, which the
    /// plan was computed from.
    read_schedule: Option<&'a [u8]>,
    read_maintenance: &'a [u8],
    /// Workflow #477 CV-2b: of a switched root, `pack-catalog.json` as the call read it.
    read_catalog: Option<&'a [u8]>,
    /// The class is maintenance: `maintenance.json` is renamed before `schedule.json`
    /// ([`root_order`]).
    maintenance_first: bool,
}

/// The order the root files are renamed in: first the file that gives the package its new role
/// (`maintenance.json` for the class maintenance, `schedule.json` for the others), then the file
/// that ends its old one, so that a call interrupted between the two renames leaves the `.part` of
/// the file not renamed, which the next call tells, and never ends the old role before the new one
/// is written. Workflow #477 CV-2b: `pack-catalog.json`, which only a switched root's adoption
/// changes, right before `schedule.json`: a call interrupted after it renamed the catalog and
/// before it renamed `schedule.json` leaves a catalog whose new part stands for no rule that runs
/// yet (a package's first rule) or that the old `requires` does not cover (the composer refuses
/// it), never a new rule without its business part, which would compose as a rule with none; and
/// it leaves `schedule.json.part`, which the next call tells.
fn root_order(maintenance_first: bool) -> [&'static str; 3] {
    if maintenance_first {
        [
            MAINTENANCE_FILE,
            STANDARD_PACK_CATALOG_FILE,
            STANDARD_PACK_SCHEDULE_FILE,
        ]
    } else {
        [
            STANDARD_PACK_CATALOG_FILE,
            STANDARD_PACK_SCHEDULE_FILE,
            MAINTENANCE_FILE,
        ]
    }
}

/// B.5: the pack directory written whole as its `.part` and renamed, the replaced one first moved
/// to its kept place and moved back when the new one cannot take its place; then `schedule.json`
/// and `maintenance.json`, each only when it changes, both written whole as `<name>.part` before
/// the first is renamed, then renamed one after the other ([`root_order`]), each read again just
/// before its rename and renamed only while it is the file the call read (so that no call
/// overwrites another's work). Every file it renames into place, or whose directory it renames
/// into place, is flushed to the disk first ([`write_flushed`]). Every rename goes through
/// `worktree`. A failure names where it stopped and what was done before; nothing is removed. The
/// kept place, when a directory was moved there; Workflow #463 AC-1: and what it wrote and moved,
/// in order, after which the cases are written ([`write_cases`]).
fn write_adoption(
    inputs: &WriteInputs<'_>,
    worktree: &dyn AdoptWorktree,
) -> LabResult<(Option<PathBuf>, Vec<PathBuf>)> {
    let mut done: Vec<PathBuf> = Vec::new();
    let root_writes = root_order(inputs.maintenance_first).map(|name| match name {
        STANDARD_PACK_SCHEDULE_FILE => (
            name,
            inputs.plan.schedule_bytes.as_deref(),
            inputs.read_schedule,
        ),
        // Workflow #477 CV-2b: written only on a switched root.
        STANDARD_PACK_CATALOG_FILE => (
            name,
            inputs.plan.catalog_bytes.as_deref(),
            inputs.read_catalog,
        ),
        _ => (
            name,
            inputs.plan.maintenance_bytes.as_deref(),
            Some(inputs.read_maintenance),
        ),
    });
    // Before the first write: each root file it will rename is still the one it read.
    for (name, bytes, read) in root_writes {
        if bytes.is_some() {
            unchanged(inputs.repo, name, read, &done, inputs.pack_relative)?;
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
        write_flushed(&target, bytes)
            .map_err(|error| write_failed(&target, &done, &error, IoOp::Write))?;
    }
    done.push(inputs.staged.to_path_buf());
    let mut moved = None;
    if let Some(kept) = inputs.kept {
        if let Some(parent) = kept.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| write_failed(parent, &done, &error, IoOp::CreateDir))?;
        }
        worktree
            .rename(inputs.pack_dir, kept)
            .map_err(|error| write_failed(inputs.pack_dir, &done, &error, IoOp::Rename))?;
        done.push(kept.to_path_buf());
        moved = Some(kept.to_path_buf());
    }
    if let Err(error) = worktree.rename(inputs.staged, inputs.pack_dir) {
        // The old directory goes back to its place, by a rename; nothing is removed.
        let Some(kept) = inputs.kept else {
            return Err(write_failed(inputs.pack_dir, &done, &error, IoOp::Rename));
        };
        let note = match worktree.rename(kept, inputs.pack_dir) {
            Ok(()) => {
                done.retain(|done_path| done_path.as_path() != kept);
                format!(
                    "the old directory was moved back from {} to {}; the new one stays as {}: \
                     move it out of the checkout by hand, then run adopt again as this call was \
                     run",
                    kept.display(),
                    inputs.pack_dir.display(),
                    inputs.staged.display()
                )
            }
            Err(back) => format!(
                "moving the old directory back from {} failed too ({back}): it is there, {} is \
                 missing, and the new one stays as {}; move the old one back by hand and the new \
                 one out of the checkout, then run adopt again as this call was run",
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
    // Both root files written whole, and flushed to the disk, before the first is renamed.
    let mut parts = Vec::new();
    for (name, bytes, read) in root_writes {
        let Some(bytes) = bytes else {
            continue;
        };
        let part = inputs.repo.join(format!("{name}.{PART}"));
        write_flushed(&part, bytes)
            .map_err(|error| write_failed(&part, &done, &error, IoOp::Write))?;
        done.push(part.clone());
        parts.push((name, part, read));
    }
    for (name, part, read) in parts {
        let target = inputs.repo.join(name);
        unchanged(inputs.repo, name, read, &done, inputs.pack_relative)?;
        worktree
            .rename(&part, &target)
            .map_err(|error| write_failed(&target, &done, &error, IoOp::Rename))?;
        done.push(target);
    }
    Ok((moved, done))
}

/// The root file `name` of `repo` is still `read` (`None`: absent), the bytes the call computed
/// from; otherwise a write refusal (`adopt_write_failed` at the file) that says it changed, after
/// `done`: the call renames nothing more. Its message says what to do: before anything was
/// written, run adopt again; after the call put its new directory `pack` in place, move the root
/// `.part` files it wrote out, commit `pack` or put it back as it was, and run adopt again with
/// `--replace` ([`new_directory_step`]).
fn unchanged(
    repo: &Path,
    name: &str,
    read: Option<&[u8]>,
    done: &[PathBuf],
    pack: &str,
) -> LabResult<()> {
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
    let next = if done.is_empty() {
        "nothing was written: run adopt again, which reads the file as it is now".to_owned()
    } else {
        format!(
            "written or moved before: {listed:?}; nothing is removed: compare with the version \
             control; move the root .part files this call wrote out of the checkout; {}; then run \
             adopt again with --replace and this call's decision (--decide, or --held-reason, with \
             --held-reference when this call had one), if any, which reads the root files as they \
             are now",
            new_directory_step(pack, "this call")
        )
    };
    Err(LabError::new(
        LabErrorClass::UsageValidation,
        ResourceToolingCode::AdoptWriteFailed.as_str(),
        format!(
            "{} changed after this call read it (another adoption or another writer), so this \
             call stops here and renames nothing more, and does not overwrite that work; {next}",
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
/// Workflow #463 S3-12: then, when the adopted rule `unvalued_rule` has no value yet,
/// `ours/model/bindings.json` with the two ways its value is registered there. Workflow #463
/// AC-1: a regression case only when the adoption took none (`none_taken`).
fn next_items(
    repo: &Path,
    game: &str,
    unvalued_rule: Option<&str>,
    none_taken: bool,
) -> Vec<AdoptNext> {
    let item = |path: String| AdoptNext {
        path,
        fixed_value: None,
    };
    let mut next = Vec::new();
    if none_taken && repo.join(REGRESSION_DIRECTORY).is_dir() {
        next.push(item(format!("{REGRESSION_DIRECTORY}/{game}/")));
    }
    for file in NEXT_FILES {
        if repo.join(file).is_file() {
            next.push(item(file.to_owned()));
        }
    }
    if let Some(rule_id) = unvalued_rule {
        next.push(AdoptNext {
            path: BINDINGS_FILE.to_owned(),
            fixed_value: Some(NextFixedValue {
                rule_id: rule_id.to_owned(),
                value_binding: NextValueBinding {
                    value: String::new(),
                    file: STANDARD_PACK_SCHEDULE_FILE.to_owned(),
                    rule_id: rule_id.to_owned(),
                    path: format!("/{SCHEDULE_RULE_FIXED_VALUE_MEMBER}"),
                },
                hand_entry: NextHandEntry {
                    rule_id: rule_id.to_owned(),
                    status: String::new(),
                    reference: String::new(),
                },
            }),
        });
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

/// Workflow #477 CV-2b: whether `rule` writes one of a rule's business members
/// (`SCHEDULE_RULE_BUSINESS_MEMBERS`), which a rule of a switched root writes none of.
fn writes_business(rule: &Map<String, Value>) -> bool {
    SCHEDULE_RULE_BUSINESS_MEMBERS
        .iter()
        .any(|member| rule.contains_key(*member))
}

/// Workflow #477 CV-2b: `rule` without its business members (`SCHEDULE_RULE_BUSINESS_MEMBERS`),
/// every other member as it stands, in its order: the rule as a switched `schedule.json` holds it.
fn without_business(rule: &Map<String, Value>) -> Map<String, Value> {
    rule.iter()
        .filter(|(name, _)| !SCHEDULE_RULE_BUSINESS_MEMBERS.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// Workflow #477 CV-2b: the refusal of the root of the checkout `repo` (game `game`) as it would
/// be written: `rule_refused`, at `issue`'s member of the root file it names, the issue
/// (`standard_pack_root_invalid`) its cause. Nothing was written.
fn root_refused(repo: &Path, game: &str, issue: &StandardPackRootIssue) -> LabError {
    refused(
        adopt(AdoptProblem::RuleRefused)
            .entry(issue.file)
            .json_pointer(bounded_pointer(&issue.pointer))
            .caused_by(root_issue_outcome(game, &repo.join(issue.file), issue)),
        format!(
            "{} as it would be written: {issue}; nothing was written",
            issue.file
        ),
    )
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

/// Writes the new file `path` whole: a file of the check tree's scratch, which is never renamed
/// into place.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
}

/// Fix round 4: writes the new file `path` whole and flushes it to the disk (`sync_all`) before it
/// returns, as the Runtime's own writers of state do. Every file adopt renames into place, or whose
/// directory it renames into place, is written so before that rename (the files of
/// `packs/<package_id>.part/`, both root `.part` files, the check tree's marker), so that a rename
/// a crash keeps never names a file whose bytes the crash lost, and a root `.part` that is not
/// whole after a crash was still being written before the first root rename.
fn write_flushed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
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
