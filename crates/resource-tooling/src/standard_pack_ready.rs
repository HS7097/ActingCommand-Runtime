// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #464 P3 (register §1.375; `contracts/standard-pack-schedule.md`, "Readiness"; the
//! design `rd-1114/464-pack-build-design.md` F.1 with the coordinator's rulings on its questions,
//! and fix round 1 after the independent review): for every pack of one game root, what the
//! scheduler can do with it, or every reason it waits. One function judges the thirteen items,
//! [`judge_standard_pack_readiness`]; `actinglab standard-pack ready --root <game root>` answers
//! it, and adopt's answer, the build's `build.json` and the placement plan are to call it with what
//! each of them holds.
//!
//! The game root is an installed one, `<standard_packs.root>/<game>/`, or a bundle laid out as one
//! (`package bundle`'s output in a directory named as its game). Its root files are read by the one
//! structure check of every reader (`read_standard_pack_root`), a refused or missing root file
//! refusing the whole answer as `standard-pack check` refuses it. Fix round 1: then
//! `standard-pack check` itself runs on the root (`check_standard_pack_root`, called as it is),
//! and each of its refusals is carried by the pack it names, or by every pack when it names none:
//! under S1 or S4 when it is of their kind, otherwise under S7; its warnings are the answer's
//! `warnings`. Then, per pack of the registry, in its order, each item is judged by the function
//! that judges it elsewhere, and answered `holds`, `fails` (with a `package_dispatch_waiting`
//! naming the item, the problem and its values, the other check's own outcome as its cause),
//! `not_judged` (with a `package_readiness_unjudged` naming why) or, item I3 only, `information`:
//! - S1 `admitted`: the Runtime's loader, `PreparedContainedTask::describe_navigation_path`, the
//!   admission `standard-pack check` and `package bundle` run for every pack of the registry;
//! - S2 `classified`: the classification of the composer (`classify_standard_pack_packages` over
//!   `standard_pack_maintenance_packages`), after the composer accepted `schedule.json`;
//! - S3 `valued`: the composer's coverage of each rule of the pack (`benefit_warnings`,
//!   `benefit_pending`, `benefit_followers`), the maintenance tier exempt; fix round 1: a pack whose
//!   every rule rides with another rule's chain waits while the pack of each head waits;
//! - S4 `composed`: the composer, `compose_standard_pack_rules`, with one instance per server
//!   (`crate::standard_pack_schedule::compose_per_server`, the composition `standard-pack check`
//!   runs), and the check's refusals of `schedule.json`;
//! - S5 `entry_reachable`: fix round 1: first the pack's prerequisite chain through the kernel's own
//!   predicate, [`PrerequisiteChain`] (`begin`, `admit`, `finish`), as the Runtime admits it before
//!   a run; then the admitted descriptor's entry (`entry_pages`, `prerequisite_package_id`,
//!   `navigation_entry_pages`) against the registry, the page list and the server's return-home
//!   package (`return_home_package`), as the Runtime's gate resolves it
//!   (`contracts/linear-steps.md`, "Return-home fallback"); fix round 2: the home entry of a
//!   page-graph pack, which takes that package through no chain (R23), holds only when the kernel
//!   accepts the package as the Runtime's entry asks it to (`require_configured_package_id`, then
//!   `prerequisite_incompatibility_with` for a configured return-home package);
//! - S6 `models_named`: the admitted descriptor's `recognition_models`, what `standard-pack check`
//!   answers as `models` (the loader refuses a target that names no model, so S6 fails only with
//!   S1);
//! - S7 `root_checked` (fix round 1): `standard-pack check`'s refusals of other kinds;
//! - I1 to I6 from [`ReadinessInstallation`], what the caller read of an installation: I1 its
//!   bindings, I2 its `package` range against the bundle's declared maximum (as `place` holds a
//!   bundle at the edge), I3 an information line (the coordinator's ruling 3: no revision tells
//!   it), I4 the composer again with the live build's subsystems (as check-config and the Runtime
//!   check `requires`), I5 its suspended pairs, I6 its verified models.
//!
//! Fix round 1: a pack answers one of four words ([`ReadinessState`]): `waiting` when an item
//! fails; `not_scheduled` when it plays a supporting part no rule dispatches; `dispatchable` when
//! every item of the root and of the installation holds; `ready_to_place` when every item of the
//! root holds and the installation's were not all judged. An item not judged never counts as
//! holding. It reads the game root only: it writes nothing, starts nothing and reads no
//! installation itself.

use crate::StandardPackCheckRequest;
use crate::codes::{
    ReadinessItem, ReadinessProblem, ReadinessRole, ReadinessState, ReadinessUnjudged,
    ResourceToolingCode,
};
use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{
    CHECK_LABEL, check_standard_pack_root, read_bounded, root_invalid, root_issue_outcome,
};
use crate::standard_pack_goals::root_irreversibility;
use crate::standard_pack_schedule::compose_per_server;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Outcome, OutcomeEnvelope};
use actingcommand_contract::{
    BundleIndex, BundleMaintenance, InterruptThen, LabError, LabResult, MaintenanceUse,
    RuntimeErrorCode, STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE,
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRoot, StandardPackRootBytes,
    StandardPackRootIssue, StandardPackSchedule, classify_standard_pack_packages,
    parse_standard_pack_schedule, read_standard_pack_root, standard_pack_maintenance_packages,
};
use actingcommand_execution_kernel::{
    ExecutionKernelCode, NavigationPackageDescriptor, PreparedContainedTask, PrerequisiteChain,
    PrerequisiteLink, TaskPackageDescriptor,
};
use actingcommand_policy::StandardPackComposeFailure;
use serde::{Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The thirteen items, in the order every pack answers them, each with its id in the design.
const ITEMS: [(ReadinessItem, &str); 13] = [
    (ReadinessItem::Admitted, "S1"),
    (ReadinessItem::Classified, "S2"),
    (ReadinessItem::Valued, "S3"),
    (ReadinessItem::Composed, "S4"),
    (ReadinessItem::EntryReachable, "S5"),
    (ReadinessItem::ModelsNamed, "S6"),
    (ReadinessItem::RootChecked, "S7"),
    (ReadinessItem::InstanceBound, "I1"),
    (ReadinessItem::PackageRevision, "I2"),
    (ReadinessItem::UnrecordedScreenCode, "I3"),
    (ReadinessItem::RequiresKnown, "I4"),
    (ReadinessItem::PairUnsuspended, "I5"),
    (ReadinessItem::ModelsInstalled, "I6"),
];

/// Whether `item` is one the game root tells (S1 to S7).
fn of_root(item: ReadinessItem) -> bool {
    matches!(
        item,
        ReadinessItem::Admitted
            | ReadinessItem::Classified
            | ReadinessItem::Valued
            | ReadinessItem::Composed
            | ReadinessItem::EntryReachable
            | ReadinessItem::ModelsNamed
            | ReadinessItem::RootChecked
    )
}

/// What [`judge_standard_pack_readiness`] judges: one game root, and what the caller holds of the
/// bundle and of an installation.
#[derive(Debug, Clone, Default)]
pub struct StandardPackReadyRequest {
    /// The game root: an installed one or a bundle laid out as one, its directory named as its
    /// game.
    pub root: PathBuf,
    /// I2: the `package` range the bundle declares, `[min, max]` (`speaks.package` of the zip's
    /// `interfaces.json`, or what the build declares); `None` where the caller holds none (an
    /// installed game root keeps no `interfaces.json`).
    pub declared_package: Option<[u64; 2]>,
    /// What the caller read of an installation; `None`: none was read, and the six items of an
    /// installation are not judged.
    pub installation: Option<ReadinessInstallation>,
}

/// What a caller read of an installation, each part `None` where it was not read (that item is
/// then not judged, never guessed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadinessInstallation {
    /// I1: every instance the installation binds, with its game and, where the read tells it, its
    /// server (the status read tells the game only; the configuration's `policy.facts.instances`
    /// both).
    pub bound: Option<Vec<ReadinessBinding>>,
    /// I2: the `package` range the installation holds a bundle to, `[min, max]` (the placing
    /// program's own declaration, as `place` reads it).
    pub package: Option<[u64; 2]>,
    /// I4: the subsystems the live build's configuration manifest names (check-config's
    /// `config_manifest.subsystems`, the Runtime's `config.subsystems`).
    pub subsystems: Option<BTreeSet<String>>,
    /// I5: the suspended pairs (`actingd suspended`'s `suspended` and `packs`).
    pub suspended: Option<Vec<ReadinessSuspension>>,
    /// I6: the models the installation's vision root holds with the identity the packs name
    /// (`vision check`'s `required_models` whose identity matches).
    pub models: Option<Vec<ReadinessModel>>,
}

/// I1: one instance an installation binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessBinding {
    pub instance_id: String,
    pub game: String,
    /// `None` where the read does not tell it.
    pub server: Option<String>,
}

/// I5: one suspended pair of an installation, by the package it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessSuspension {
    pub package_id: String,
    /// The pair's task, where the read names one.
    pub task_id: Option<String>,
    /// The pair's instance, where the read names one.
    pub instance_id: Option<String>,
}

/// One model: its folder name below the vision root's `models\` and its identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ReadinessModel {
    pub model_ref: String,
    pub model_sha256: String,
}

/// The answer: every pack of the game root, in the registry's order.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackReadiness {
    pub root: String,
    pub game: String,
    /// Whether an installation was read: without one only the seven items of the game root are
    /// judged.
    pub installation: InstallationRead,
    /// How many packs answer each of the four words (fix round 1).
    pub dispatchable: usize,
    pub ready_to_place: usize,
    pub waiting: usize,
    pub not_scheduled: usize,
    /// Fix round 1: the warnings `standard-pack check` gave when it accepted the root, in its
    /// order (a recorded entry page the page list does not name is accepted with its warning,
    /// ruling 7); empty when it refused the root.
    pub warnings: Vec<Outcome>,
    pub packs: Vec<PackReadiness>,
}

/// Whether the readiness was given an installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationRead {
    Read,
    NotRead,
}

/// One pack's readiness.
#[derive(Debug, Clone, Serialize)]
pub struct PackReadiness {
    pub package_id: String,
    pub server: String,
    /// Fix round 1: one of the four words.
    pub readiness: ReadinessState,
    /// Fix round 1: the supporting parts the root files give the pack (a `not_scheduled` pack's
    /// parts, which other packs' runs use); absent when it plays none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<ReadinessRole>,
    /// The thirteen items, S1 to S7, then I1 to I6.
    pub items: Vec<ReadinessAnswer>,
}

impl PackReadiness {
    /// The answer of `item`.
    pub fn item(&self, item: ReadinessItem) -> Option<&ReadinessAnswer> {
        self.items.iter().find(|answer| answer.item == item)
    }
}

impl Serialize for ReadinessState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for ReadinessRole {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One item's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Holds,
    Fails,
    NotJudged,
    /// Item I3 only: told, never a refusal.
    Information,
}

/// How an item holds, where more than one way exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessBasis {
    /// S2: named by a rule and not held.
    Scheduled,
    /// S2: a root file gives the pack a maintenance role.
    MaintenanceRole,
    /// S3: the pack's package is maintenance-role, so its rules are not judged.
    MaintenanceTier,
    /// S3: its rules declare themselves maintenance.
    MaintenanceDeclared,
    /// S3: a rule of it is complete.
    Complete,
    /// S3: its rules ride with a complete rule's chain.
    Follower,
    /// S4 and I4: the game root holds no `schedule.json`: nothing to refuse.
    NoSchedule,
    /// S5: an application entry.
    ApplicationEntry,
    /// S5: no page entry to reach.
    NoPageEntry,
    /// S5: its declared prerequisite package, a package of the root.
    Prerequisite,
    /// S5: navigation data that lists its entry.
    Navigation,
    /// S5: its server's return-home package.
    ReturnHome,
}

/// One item of one pack.
#[derive(Debug, Clone, Serialize)]
pub struct ReadinessAnswer {
    /// The item's id in the design: `S1` to `S7`, `I1` to `I6`.
    pub id: &'static str,
    #[serde(serialize_with = "item_token")]
    pub item: ReadinessItem,
    pub state: ItemState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basis: Option<ReadinessBasis>,
    /// S6 and I6: the models the pack needs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ReadinessModel>,
    /// `fails`: `package_dispatch_waiting`; `not_judged` and `information`:
    /// `package_readiness_unjudged`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

/// A readiness item as its token.
fn item_token<S: Serializer>(item: &ReadinessItem, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(item.as_str())
}

/// The design's id of `item`.
fn item_id(item: ReadinessItem) -> &'static str {
    ITEMS
        .iter()
        .find(|(listed, _)| *listed == item)
        .map_or("", |(_, id)| *id)
}

impl ReadinessAnswer {
    fn new(item: ReadinessItem, state: ItemState, outcome: Option<Outcome>) -> Self {
        Self {
            id: item_id(item),
            item,
            state,
            basis: None,
            models: Vec::new(),
            outcome,
        }
    }

    fn holds(item: ReadinessItem, basis: Option<ReadinessBasis>) -> Self {
        let mut answer = Self::new(item, ItemState::Holds, None);
        answer.basis = basis;
        answer
    }

    fn fails(item: ReadinessItem, outcome: Outcome) -> Self {
        Self::new(item, ItemState::Fails, Some(outcome))
    }

    fn not_judged(item: ReadinessItem, package_id: &str, why: ReadinessUnjudged) -> Self {
        Self::new(
            item,
            ItemState::NotJudged,
            Some(unjudged(package_id, item, why)),
        )
    }

    fn with_models(mut self, models: Vec<ReadinessModel>) -> Self {
        self.models = models;
        self
    }
}

/// `package_dispatch_waiting` of `package_id` for `item`.
fn waiting(package_id: &str, item: ReadinessItem, problem: ReadinessProblem) -> Outcome {
    Outcome::new(ResourceToolingCode::PackageDispatchWaiting)
        .package_id(package_id)
        .readiness_item(item)
        .readiness_problem(problem)
}

/// `package_readiness_unjudged` of `package_id` for `item`.
fn unjudged(package_id: &str, item: ReadinessItem, why: ReadinessUnjudged) -> Outcome {
    Outcome::new(ResourceToolingCode::PackageReadinessUnjudged)
        .package_id(package_id)
        .readiness_item(item)
        .readiness_unjudged(why)
}

/// Every pack of the registry as the loader admitted it, by package id: its descriptor, or
/// `package_invalid` (its directory) caused by the loader's refusal.
type Admitted = BTreeMap<String, Result<NavigationPackageDescriptor, Outcome>>;

/// Workflow #464 P3 (module doc): the readiness of every pack of the game root `request.root`. A
/// root that is no directory, that holds no `bundle.json` or whose root files the structure check
/// refuses is refused as `standard-pack check` refuses it; nothing else refuses.
pub fn judge_standard_pack_readiness(
    request: &StandardPackReadyRequest,
) -> LabResult<StandardPackReadiness> {
    let root = std::path::absolute(&request.root)
        .map_err(|error| LabError::usage(format!("{}: {error}", request.root.display())))?;
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
    let checked = read_root(&root, &game)?;
    // Fix round 1 (the coordinator's ruling): `standard-pack check` on the same root, called, not
    // copied; a refusal is carried by the packs, never the whole answer.
    let (warnings, refusals) =
        match check_standard_pack_root(StandardPackCheckRequest { root: root.clone() }) {
            Ok(response) => (response.warnings, Vec::new()),
            Err(error) => (Vec::new(), check_refusals(&error, &checked.root)),
        };
    let installation = request.installation.as_ref();
    let rules = Rules::of(&game, &checked, &root, installation)?;
    let mut admitted = Admitted::new();
    for package_id in checked.root.registry.package_ids() {
        let entry = checked.root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let directory = root.join(&entry.directory);
        let admission = PreparedContainedTask::describe_navigation_path(
            CHECK_LABEL,
            &directory,
            Instant::now() + PACK_DEADLINE,
        )
        .map_err(|error| {
            Outcome::new(RuntimeErrorCode::PackageInvalid)
                .path(&entry.directory)
                .caused_by(error.outcome())
        });
        admitted.insert(package_id.to_owned(), admission);
    }
    let context = Context {
        game: &game,
        root: &checked.root,
        maintenance: &checked.maintenance,
        rules: &rules,
        declared_package: request.declared_package,
        installation,
        admitted: &admitted,
        refusals: &refusals,
    };
    let mut packs = Vec::new();
    for (package_id, admission) in &admitted {
        let entry = checked.root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let pack = Pack {
            package_id: package_id.as_str(),
            server: entry.server.as_str(),
            admitted: admission,
        };
        packs.push(context.judge(&pack));
    }
    context.hold_followers(&mut packs);
    let count = |state: ReadinessState| packs.iter().filter(|pack| pack.readiness == state).count();
    Ok(StandardPackReadiness {
        root: root.display().to_string(),
        game,
        installation: if installation.is_some() {
            InstallationRead::Read
        } else {
            InstallationRead::NotRead
        },
        dispatchable: count(ReadinessState::Dispatchable),
        ready_to_place: count(ReadinessState::ReadyToPlace),
        waiting: count(ReadinessState::Waiting),
        not_scheduled: count(ReadinessState::NotScheduled),
        warnings,
        packs,
    })
}

/// The game root as every reader reads it, and its bundle's maintenance entries.
struct CheckedRoot {
    root: StandardPackRoot,
    maintenance: Vec<BundleMaintenance>,
}

/// Reads the root files of `root` (the game `game`) as `standard-pack check` reads them.
fn read_root(root: &Path, game: &str) -> LabResult<CheckedRoot> {
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
        game,
    )
    .map_err(|issue| root_invalid(game, &root.join(issue.file), &issue))?
    .ok_or_else(|| {
        let issue = StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        };
        root_invalid(game, &root.join(STANDARD_PACK_BUNDLE_FILE), &issue)
    })?;
    let maintenance = bytes(STANDARD_PACK_BUNDLE_FILE)
        .and_then(|bundle| serde_json::from_slice::<BundleIndex>(bundle).ok())
        .map(|index| index.maintenance().to_vec())
        .unwrap_or_default();
    Ok(CheckedRoot {
        root: checked,
        maintenance,
    })
}

/// Which item carries a refusal of `standard-pack check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefusalKind {
    /// S1: the admission of the pack it names (`package_invalid`).
    Admission,
    /// S4: `schedule.json` (`standard_pack_root_invalid` naming it).
    Composition,
    /// S7: anything else the check refuses.
    Root,
}

/// Fix round 1: one refusal of `standard-pack check`.
struct CheckRefusal {
    /// The pack it names; `None`: it names none, and every pack carries it.
    package_id: Option<String>,
    kind: RefusalKind,
    outcome: Outcome,
}

impl CheckRefusal {
    /// The refusal `outcome` of the root `root`: the pack its `package_id` names, else the pack
    /// whose directory its `path` is, else none; its kind by its code (`package_invalid` that
    /// names a pack is S1's; `standard_pack_root_invalid` whose `path` is `schedule.json` S4's; a
    /// `package_invalid` that names no pack, from a reader whose refusal carries no outcome,
    /// tells no pack's admission and is S7's like every other refusal).
    fn read(outcome: Outcome, root: &StandardPackRoot) -> Self {
        let text = |key: &str| outcome.value(key).and_then(serde_json::Value::as_str);
        let package_id = text("package_id")
            .filter(|package_id| root.registry.get(package_id).is_some())
            .map(str::to_owned)
            .or_else(|| {
                let path = Path::new(text("path")?);
                root.registry
                    .package_ids()
                    .find(|package_id| {
                        root.registry
                            .get(package_id)
                            .is_some_and(|entry| Path::new(&entry.directory) == path)
                    })
                    .map(str::to_owned)
            });
        let of_schedule = text("path")
            .and_then(|path| Path::new(path).file_name())
            .is_some_and(|name| name == STANDARD_PACK_SCHEDULE_FILE);
        let kind = if outcome.is(RuntimeErrorCode::PackageInvalid) && package_id.is_some() {
            RefusalKind::Admission
        } else if outcome.is(SharedCode::StandardPackRootInvalid) && of_schedule {
            RefusalKind::Composition
        } else {
            RefusalKind::Root
        };
        Self {
            package_id,
            kind,
            outcome,
        }
    }

    /// Whether the pack `package_id` carries it.
    fn carried_by(&self, package_id: &str) -> bool {
        self.package_id
            .as_deref()
            .is_none_or(|named| named == package_id)
    }
}

/// Fix round 1: the refusals `error` of `standard-pack check` holds: every issue it lists
/// (`details.issues`), else its outcome (`details.outcome`), else its code alone.
fn check_refusals(error: &LabError, root: &StandardPackRoot) -> Vec<CheckRefusal> {
    let details = error.details.as_ref();
    let listed = details
        .and_then(|details| details.get("issues"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .or_else(|| {
            details
                .and_then(|details| details.get("outcome"))
                .map(|outcome| vec![outcome.clone()])
        })
        .unwrap_or_default();
    let mut outcomes = listed
        .into_iter()
        .filter_map(|envelope| serde_json::from_value::<OutcomeEnvelope>(envelope).ok())
        .map(OutcomeEnvelope::into_outcome)
        .collect::<Vec<_>>();
    if outcomes.is_empty() {
        outcomes.push(Outcome::relayed(&error.code));
    }
    outcomes
        .into_iter()
        .map(|outcome| CheckRefusal::read(outcome, root))
        .collect()
}

/// What the game's `schedule.json` gives the items S2, S3, S4 and I4.
enum Rules {
    /// The game root holds no `schedule.json`: the game has no rules; the packages the root files
    /// give a maintenance role.
    Absent { maintenance: BTreeSet<String> },
    /// The composer refused the file: its first refusal, at the file's pointer.
    Refused(Outcome),
    /// The composer accepted the file.
    Composed(Box<ComposedRules>),
}

/// A `schedule.json` the composer accepted, as the items read it.
struct ComposedRules {
    schedule: StandardPackSchedule,
    maintenance: BTreeSet<String>,
    scheduled: BTreeSet<String>,
    held: BTreeSet<String>,
    /// The judged rules counted pending, by id.
    pending: BTreeSet<String>,
    /// The chain followers, by id, each with the complete rule it rides with.
    followers: BTreeMap<String, String>,
    /// The judged rules counted undeclared, by id, each with the composer's warning.
    undeclared: BTreeMap<String, Outcome>,
    /// I4: the composer again with the live build's subsystems: `None` without them, the refusal
    /// when it refuses.
    requires: Option<Result<(), Outcome>>,
}

impl Rules {
    /// The rules of the game root `checked` of `game` at `root`.
    fn of(
        game: &str,
        checked: &CheckedRoot,
        root: &Path,
        installation: Option<&ReadinessInstallation>,
    ) -> LabResult<Self> {
        let path = root.join(STANDARD_PACK_SCHEDULE_FILE);
        let maintenance = standard_pack_maintenance_packages(&checked.maintenance, &checked.root);
        let Some(bytes) = read_bounded(&path)? else {
            return Ok(Self::Absent { maintenance });
        };
        let refusal = |failure: &StandardPackComposeFailure| match failure.issues().first() {
            Some(first) => root_issue_outcome(game, &path, &first.issue),
            None => Outcome::new(SharedCode::StandardPackRootInvalid)
                .game(game)
                .path(&path)
                .standard_pack_problem(StandardPackProblem::ScheduleInvalid)
                .json_pointer(String::new()),
        };
        // Workflow #455 G-R2s: with every package of the root read as `standard-pack check` reads
        // it, so the composition is the one the check runs.
        let irreversible =
            root_irreversibility(&checked.root, &|directory: &str| root.join(directory));
        let compose = |subsystems: Option<&BTreeSet<String>>| {
            compose_per_server(
                game,
                &checked.root,
                &checked.maintenance,
                &bytes,
                &irreversible.packages,
                subsystems,
            )
        };
        let composition = match compose(None) {
            Ok(composition) => composition,
            Err(failure) => return Ok(Self::Refused(refusal(&failure))),
        };
        // The composer read the file; read again here, it is the reader the composer runs.
        let schedule = parse_standard_pack_schedule(&bytes)
            .map_err(|issue| root_invalid(game, &path, &issue))?;
        let classification =
            classify_standard_pack_packages(&checked.root.registry, &maintenance, &schedule);
        let of_game = |lists: &[(String, Vec<String>)]| {
            lists
                .iter()
                .filter(|(game_id, _)| game_id == game)
                .flat_map(|(_, ids)| ids.iter().cloned())
                .collect::<BTreeSet<_>>()
        };
        let pending = of_game(composition.benefit_pending());
        let followers = composition
            .benefit_followers()
            .iter()
            .filter(|(game_id, _)| game_id == game)
            .flat_map(|(_, followers)| {
                followers
                    .iter()
                    .map(|follower| (follower.task_id.clone(), follower.rides_with.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let undeclared = composition
            .benefit_warnings()
            .into_iter()
            .filter_map(|warning| {
                let rule = warning.value("task_id")?.as_str()?.to_owned();
                Some((rule, warning))
            })
            .collect::<BTreeMap<_, _>>();
        let requires = installation
            .and_then(|installation| installation.subsystems.as_ref())
            .map(|subsystems| {
                compose(Some(subsystems))
                    .map(|_| ())
                    .map_err(|failure| refusal(&failure))
            });
        Ok(Self::Composed(Box::new(ComposedRules {
            schedule,
            maintenance: classification.maintenance,
            scheduled: classification.scheduled,
            held: classification.held,
            pending,
            followers,
            undeclared,
            requires,
        })))
    }
}

impl ComposedRules {
    /// Fix round 1: when every judged rule of the pack `package_id` is a follower whose head's
    /// pack is another pack in `waiting`, the first such follower and its head's pack.
    fn behind(&self, package_id: &str, waiting: &BTreeSet<String>) -> Option<(String, String)> {
        let mut first = None;
        for rule in self
            .schedule
            .rules
            .iter()
            .filter(|rule| rule.package_id == package_id && !rule.maintenance)
        {
            let head_rule = self.followers.get(&rule.id)?;
            let head = self
                .schedule
                .rules
                .iter()
                .find(|candidate| candidate.id == *head_rule)?;
            if head.package_id == package_id || !waiting.contains(&head.package_id) {
                return None;
            }
            if first.is_none() {
                first = Some((rule.id.clone(), head.package_id.clone()));
            }
        }
        first
    }
}

/// One pack of the registry and what its admission gave.
struct Pack<'a> {
    package_id: &'a str,
    server: &'a str,
    /// Its descriptor, or `package_invalid` (its directory) caused by the loader's refusal.
    admitted: &'a Result<NavigationPackageDescriptor, Outcome>,
}

impl Pack<'_> {
    /// The models its recognition targets name, each once.
    fn models(descriptor: &NavigationPackageDescriptor) -> Vec<ReadinessModel> {
        descriptor
            .recognition_models()
            .iter()
            .map(|(model_ref, model_sha256)| ReadinessModel {
                model_ref: model_ref.clone(),
                model_sha256: model_sha256.clone(),
            })
            .collect()
    }
}

/// S5 (fix round 1): where the kernel's prerequisite chain refused a pack's chain.
struct ChainRefusal {
    /// The layer the chain was refused at.
    package_id: String,
    /// The layer is no package of the game root.
    unbound: bool,
    /// The kernel's refusal, or the layer's own admission refusal.
    cause: Outcome,
}

impl ChainRefusal {
    fn at(link: &PrerequisiteLink, unbound: bool, cause: Outcome) -> Self {
        Self {
            package_id: link.package_id.clone(),
            unbound,
            cause,
        }
    }
}

/// What every pack of one game root is judged against.
struct Context<'a> {
    game: &'a str,
    root: &'a StandardPackRoot,
    /// The bundle's maintenance entries.
    maintenance: &'a [BundleMaintenance],
    rules: &'a Rules,
    declared_package: Option<[u64; 2]>,
    installation: Option<&'a ReadinessInstallation>,
    /// Every pack of the registry as the loader admitted it.
    admitted: &'a Admitted,
    /// Fix round 1: the refusals of `standard-pack check`, empty when it accepted the root.
    refusals: &'a [CheckRefusal],
}

/// The part `read` of `installation`, or why the item that needs it is not judged.
fn part<'a, T>(
    installation: Option<&'a ReadinessInstallation>,
    read: impl FnOnce(&'a ReadinessInstallation) -> Option<&'a T>,
) -> Result<&'a T, ReadinessUnjudged> {
    let installation = installation.ok_or(ReadinessUnjudged::InstallationNotRead)?;
    read(installation).ok_or(ReadinessUnjudged::PartNotRead)
}

impl Context<'_> {
    /// The thirteen items of `pack` and its answer.
    fn judge(&self, pack: &Pack<'_>) -> PackReadiness {
        let items = vec![
            self.admitted(pack),
            self.classified(pack),
            self.valued(pack),
            self.composed(pack),
            self.entry_reachable(pack),
            self.models_named(pack),
            self.root_checked(pack),
            self.instance_bound(pack),
            self.package_revision(pack),
            self.unrecorded_screen_code(pack),
            self.requires_known(pack),
            self.pair_unsuspended(pack),
            self.models_installed(pack),
        ];
        PackReadiness {
            package_id: pack.package_id.to_owned(),
            server: pack.server.to_owned(),
            readiness: self.answer(pack.package_id, &items),
            roles: self.roles(pack.package_id),
            items,
        }
    }

    /// Fix round 1: the four answers. `waiting` when an item fails or an item of the root does
    /// not hold; `not_scheduled` for a supporting pack no rule dispatches; `dispatchable` when
    /// every item of the installation holds too (I3 is information); `ready_to_place` otherwise,
    /// an item not judged never counting as holding.
    fn answer(&self, package_id: &str, items: &[ReadinessAnswer]) -> ReadinessState {
        let root_holds = items
            .iter()
            .filter(|answer| of_root(answer.item))
            .all(|answer| answer.state == ItemState::Holds);
        if !root_holds || items.iter().any(|answer| answer.state == ItemState::Fails) {
            return ReadinessState::Waiting;
        }
        if self.supporting(package_id) {
            return ReadinessState::NotScheduled;
        }
        let installed = items
            .iter()
            .filter(|answer| !of_root(answer.item))
            .all(|answer| matches!(answer.state, ItemState::Holds | ItemState::Information));
        if installed {
            ReadinessState::Dispatchable
        } else {
            ReadinessState::ReadyToPlace
        }
    }

    /// Whether the pack `package_id` plays a supporting part a root file gives it and no rule
    /// dispatches it (it is maintenance-role and not scheduled).
    fn supporting(&self, package_id: &str) -> bool {
        match self.rules {
            Rules::Absent { maintenance } => maintenance.contains(package_id),
            Rules::Composed(rules) => {
                rules.maintenance.contains(package_id) && !rules.scheduled.contains(package_id)
            }
            Rules::Refused(_) => false,
        }
    }

    /// The supporting parts the root files give the pack `package_id`, in the order of
    /// `standard_pack_maintenance_packages`, each once.
    fn roles(&self, package_id: &str) -> Vec<ReadinessRole> {
        let mut roles = Vec::new();
        let mut add = |role: ReadinessRole| {
            if !roles.contains(&role) {
                roles.push(role);
            }
        };
        for entry in self
            .maintenance
            .iter()
            .filter(|entry| entry.package_id == package_id)
        {
            for purpose in &entry.uses {
                add(match purpose {
                    MaintenanceUse::Startup => ReadinessRole::Startup,
                    MaintenanceUse::ReturnHome => ReadinessRole::ReturnHome,
                    MaintenanceUse::Recognition => ReadinessRole::Recognition,
                });
            }
        }
        if let Some(pages) = &self.root.pages
            && pages.features.iter().any(|feature| feature == package_id)
        {
            add(ReadinessRole::Features);
        }
        if let Some(navigation) = &self.root.navigation {
            if navigation
                .edges
                .iter()
                .any(|edge| edge.package_id == package_id)
            {
                add(ReadinessRole::Edge);
            }
            if navigation
                .recovery_chain
                .iter()
                .any(|step| step == package_id)
            {
                add(ReadinessRole::Chain);
            }
        }
        if let Some(interrupts) = &self.root.interrupts {
            for interrupt in &interrupts.interrupts {
                if interrupt
                    .handlers
                    .iter()
                    .any(|handler| handler == package_id)
                {
                    add(ReadinessRole::Handler);
                }
                if let InterruptThen::Run(run) = &interrupt.then
                    && run.iter().any(|named| named == package_id)
                {
                    add(ReadinessRole::ThenRun);
                }
            }
        }
        roles
    }

    /// Fix round 1: the first refusal of `standard-pack check` of `kind` the pack `package_id`
    /// carries.
    fn carried(&self, package_id: &str, kind: RefusalKind) -> Option<&CheckRefusal> {
        self.refusals
            .iter()
            .find(|refusal| refusal.kind == kind && refusal.carried_by(package_id))
    }

    /// Fix round 1: a follower is not ahead of its head. A pack whose every rule rides with
    /// another rule's chain (S3 `follower`) waits while the pack of each head waits: its S3 fails
    /// `head_waiting`, naming its first follower and that follower's head's pack. Again until
    /// nothing changes, since a head may wait for its own head.
    fn hold_followers(&self, packs: &mut [PackReadiness]) {
        const ITEM: ReadinessItem = ReadinessItem::Valued;
        let Rules::Composed(rules) = self.rules else {
            return;
        };
        loop {
            let waiting_packs = packs
                .iter()
                .filter(|pack| pack.readiness == ReadinessState::Waiting)
                .map(|pack| pack.package_id.clone())
                .collect::<BTreeSet<_>>();
            let mut changed = false;
            for pack in packs.iter_mut() {
                let follows = pack.item(ITEM).and_then(|answer| answer.basis)
                    == Some(ReadinessBasis::Follower);
                if pack.readiness == ReadinessState::Waiting || !follows {
                    continue;
                }
                let Some((follower, head)) = rules.behind(&pack.package_id, &waiting_packs) else {
                    continue;
                };
                let outcome = waiting(&pack.package_id, ITEM, ReadinessProblem::HeadWaiting)
                    .game(self.game)
                    .task_id(follower.as_str())
                    .blocking_package_id(head.as_str());
                if let Some(answer) = pack.items.iter_mut().find(|answer| answer.item == ITEM) {
                    *answer = ReadinessAnswer::fails(ITEM, outcome);
                }
                pack.readiness = ReadinessState::Waiting;
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }

    /// S1: the loader admits the pack, and `standard-pack check` refuses no admission it carries.
    fn admitted(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::Admitted;
        let refused = |cause: Outcome| {
            ReadinessAnswer::fails(
                ITEM,
                waiting(pack.package_id, ITEM, ReadinessProblem::AdmissionRefused)
                    .game(self.game)
                    .caused_by(cause),
            )
        };
        match pack.admitted {
            Err(refusal) => refused(refusal.clone()),
            Ok(_) => match self.carried(pack.package_id, RefusalKind::Admission) {
                Some(refusal) => refused(refusal.outcome.clone()),
                None => ReadinessAnswer::holds(ITEM, None),
            },
        }
    }

    /// S2: a rule names the pack and it is not held, or a root file gives it a maintenance role.
    fn classified(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::Classified;
        let id = pack.package_id;
        let failed = |problem| waiting(id, ITEM, problem).game(self.game);
        match self.rules {
            Rules::Refused(_) => {
                ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::CompositionRefused)
            }
            Rules::Absent { maintenance } if maintenance.contains(id) => {
                ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::MaintenanceRole))
            }
            Rules::Absent { .. } => {
                ReadinessAnswer::fails(ITEM, failed(ReadinessProblem::ScheduleMissing))
            }
            Rules::Composed(rules) if rules.maintenance.contains(id) => {
                ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::MaintenanceRole))
            }
            Rules::Composed(rules) if rules.scheduled.contains(id) => {
                ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::Scheduled))
            }
            Rules::Composed(rules) if rules.held.contains(id) => {
                ReadinessAnswer::fails(ITEM, failed(ReadinessProblem::Held))
            }
            Rules::Composed(_) => ReadinessAnswer::fails(
                ITEM,
                failed(ReadinessProblem::Unclassified).caused_by(
                    Outcome::new(SharedCode::StandardPackPackageUnclassified)
                        .game(self.game)
                        .package_id(id),
                ),
            ),
        }
    }

    /// S3: every rule of the pack is in the maintenance tier or declares its value (a follower
    /// behind a waiting head is judged after every pack, [`Self::hold_followers`]).
    fn valued(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::Valued;
        let id = pack.package_id;
        let rules = match self.rules {
            Rules::Refused(_) => {
                return ReadinessAnswer::not_judged(
                    ITEM,
                    id,
                    ReadinessUnjudged::CompositionRefused,
                );
            }
            Rules::Absent { maintenance } if maintenance.contains(id) => {
                return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::MaintenanceTier));
            }
            Rules::Absent { .. } => {
                return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::NoRule);
            }
            Rules::Composed(rules) => rules,
        };
        if rules.maintenance.contains(id) {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::MaintenanceTier));
        }
        let own = rules
            .schedule
            .rules
            .iter()
            .filter(|rule| rule.package_id == id)
            .collect::<Vec<_>>();
        if rules.held.contains(id) || own.is_empty() {
            return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::NoRule);
        }
        let (mut complete, mut follower) = (false, false);
        for rule in own {
            if rule.maintenance {
                continue;
            }
            if let Some(warning) = rules.undeclared.get(&rule.id) {
                let mut outcome = waiting(id, ITEM, ReadinessProblem::ValueUndeclared)
                    .game(self.game)
                    .task_id(rule.id.as_str());
                if let Some(pool) = warning.value("pool_id").and_then(serde_json::Value::as_str) {
                    outcome = outcome.pool_id(pool);
                }
                return ReadinessAnswer::fails(ITEM, outcome.caused_by(warning.clone()));
            }
            if rules.pending.contains(&rule.id) {
                return ReadinessAnswer::fails(
                    ITEM,
                    waiting(id, ITEM, ReadinessProblem::ValuePending)
                        .game(self.game)
                        .task_id(rule.id.as_str()),
                );
            }
            if rules.followers.contains_key(&rule.id) {
                follower = true;
            } else {
                complete = true;
            }
        }
        let basis = if complete {
            ReadinessBasis::Complete
        } else if follower {
            ReadinessBasis::Follower
        } else {
            ReadinessBasis::MaintenanceDeclared
        };
        ReadinessAnswer::holds(ITEM, Some(basis))
    }

    /// S4: the composer accepts the game's `schedule.json` with one instance per server, and
    /// `standard-pack check` refuses no part of it the pack carries.
    fn composed(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::Composed;
        let refused = |cause: Outcome| {
            ReadinessAnswer::fails(
                ITEM,
                waiting(pack.package_id, ITEM, ReadinessProblem::CompositionRefused)
                    .game(self.game)
                    .caused_by(cause),
            )
        };
        let basis = match self.rules {
            Rules::Refused(refusal) => return refused(refusal.clone()),
            Rules::Absent { .. } => Some(ReadinessBasis::NoSchedule),
            Rules::Composed(_) => None,
        };
        match self.carried(pack.package_id, RefusalKind::Composition) {
            Some(refusal) => refused(refusal.outcome.clone()),
            None => ReadinessAnswer::holds(ITEM, basis),
        }
    }

    /// S5: the pack's prerequisite chain passes the kernel's predicate, and its entry is reached
    /// (module doc).
    fn entry_reachable(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::EntryReachable;
        let id = pack.package_id;
        let Ok(descriptor) = pack.admitted else {
            return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::NotAdmitted);
        };
        let task = descriptor.descriptor();
        let entry = task.entry_pages();
        let failed = |problem| {
            let outcome = waiting(id, ITEM, problem)
                .game(self.game)
                .server(pack.server);
            match entry {
                Some(pages) => outcome.page_ids(pages.iter().cloned()),
                None => outcome,
            }
        };
        let navigated = self.root.navigation.is_some()
            && match (&self.root.pages, descriptor.navigation_entry_pages()) {
                (Some(pages), Some(pages_of_entry)) => pages_of_entry
                    .iter()
                    .all(|page| pages.pages.iter().any(|listed| listed.id == *page)),
                _ => false,
            };
        // Fix round 1: the chain the Runtime admits before a run, through the kernel's predicate.
        let home_layer = match self.chain(task, navigated) {
            Ok(home_layer) => home_layer,
            Err(refusal) => {
                let problem = if refusal.unbound {
                    ReadinessProblem::PrerequisiteUnregistered
                } else {
                    ReadinessProblem::PrerequisiteRefused
                };
                return ReadinessAnswer::fails(
                    ITEM,
                    failed(problem)
                        .blocking_package_id(refusal.package_id.as_str())
                        .caused_by(refusal.cause),
                );
            }
        };
        if descriptor.has_application_entry() {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::ApplicationEntry));
        }
        if entry.is_none() {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::NoPageEntry));
        }
        if task.prerequisite_package_id().is_some() {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::Prerequisite));
        }
        if navigated {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::Navigation));
        }
        let Some(return_home) = self
            .root
            .registry
            .return_home_package(pack.server)
            .filter(|named| *named != id)
        else {
            return ReadinessAnswer::fails(ITEM, failed(ReadinessProblem::EntryUnreachable));
        };
        // Fix round 2: a pack whose chain did not take the return-home package as a layer is a
        // page-graph pack run as the run's own package, which keeps its home entry (R23); that
        // entry judges the package before it runs it.
        if !home_layer && let Err(cause) = self.home_entry(task, return_home) {
            return ReadinessAnswer::fails(
                ITEM,
                failed(ReadinessProblem::ReturnHomeRefused)
                    .blocking_package_id(return_home)
                    .caused_by(cause),
            );
        }
        ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::ReturnHome))
    }

    /// S5 (fix round 2): the return-home package `return_home` the home entry of the page-graph
    /// pack `task` takes, judged as the Runtime's entry judges it before it runs it (the host's
    /// `run_preflighted_contained_task`, Workflow #336 L2d and PKG-S4): the package the loader
    /// admitted here, then the kernel's `require_configured_package_id`, then the kernel's
    /// `prerequisite_incompatibility_with` for a configured return-home package (the host's
    /// `configured_return_home_incompatibility`: a chain layer's checks against the pack's game,
    /// server and resolution, and no prerequisite package of its own). The error is the cause:
    /// the package's admission refusal, the kernel's code, or, for a package the loader was not
    /// asked to admit, `contained_task_prerequisite_unbound` as the chain answers it.
    fn home_entry(&self, task: &TaskPackageDescriptor, return_home: &str) -> Result<(), Outcome> {
        let package = match self.admitted.get(return_home) {
            Some(Ok(admitted)) => admitted.descriptor(),
            Some(Err(admission)) => return Err(admission.clone()),
            None => {
                return Err(Outcome::new(
                    ExecutionKernelCode::ContainedTaskPrerequisiteUnbound,
                ));
            }
        };
        package
            .require_configured_package_id(return_home)
            .map_err(|error| error.outcome())?;
        if package
            .prerequisite_incompatibility_with(task.game(), task.server(), task.resolution(), true)
            .is_some()
        {
            return Err(Outcome::new(
                SharedCode::ContainedTaskPrerequisiteIncompatible,
            ));
        }
        Ok(())
    }

    /// S5 (fix round 1): the prerequisite chain of the pack `task` as the Runtime admits it before
    /// a run (the host's `resolve_prerequisite_chain`): none when it declares no prerequisite
    /// package and its entry is `navigated` or it may not fall back to the return-home package;
    /// otherwise every layer through the kernel's [`PrerequisiteChain`] (`next` with its server's
    /// return-home package, `begin`, `admit`, `finish`), each layer's descriptor the one the loader
    /// admitted here. Fix round 2: whether the chain admitted a return-home layer.
    fn chain(&self, task: &TaskPackageDescriptor, navigated: bool) -> Result<bool, ChainRefusal> {
        let mut chain = PrerequisiteChain::new(task.clone());
        if task.prerequisite_package_id().is_none() && (navigated || !chain.may_fall_back()) {
            return Ok(false);
        }
        let registry = &self.root.registry;
        let mut home_layer = false;
        while let Some(link) = chain.next(registry.return_home_package(chain.dependent().server()))
        {
            let layer = self.admitted.get(link.package_id.as_str());
            chain
                .begin(&link, layer.is_some())
                .map_err(|error| ChainRefusal::at(&link, layer.is_none(), error.outcome()))?;
            let descriptor = match layer {
                Some(Ok(descriptor)) => descriptor.descriptor().clone(),
                Some(Err(admission)) => {
                    return Err(ChainRefusal::at(&link, false, admission.clone()));
                }
                None => {
                    let unbound =
                        Outcome::new(ExecutionKernelCode::ContainedTaskPrerequisiteUnbound);
                    return Err(ChainRefusal::at(&link, true, unbound));
                }
            };
            chain
                .admit(&link, descriptor)
                .map_err(|error| ChainRefusal::at(&link, false, error.outcome()))?;
            home_layer |= link.return_home;
        }
        chain.finish().map_err(|error| ChainRefusal {
            package_id: chain.dependent().package_id().to_owned(),
            unbound: false,
            cause: error.outcome(),
        })?;
        Ok(home_layer)
    }

    /// S6: the models the pack needs are named.
    fn models_named(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::ModelsNamed;
        match pack.admitted {
            Ok(descriptor) => {
                ReadinessAnswer::holds(ITEM, None).with_models(Pack::models(descriptor))
            }
            Err(_) => {
                ReadinessAnswer::not_judged(ITEM, pack.package_id, ReadinessUnjudged::NotAdmitted)
            }
        }
    }

    /// S7 (fix round 1): `standard-pack check` accepts the root, beyond what S1 and S4 tell. A
    /// refusal of another kind the pack carries fails it (`check_refused`); one of S1's or S4's
    /// kind it carries leaves it not judged (`check_stopped`); a refusal only another pack
    /// carries fails it too (`root_refused`), as the check judged nothing after it.
    fn root_checked(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::RootChecked;
        let id = pack.package_id;
        let Some(first) = self.refusals.first() else {
            return ReadinessAnswer::holds(ITEM, None);
        };
        if let Some(own) = self.carried(id, RefusalKind::Root) {
            return ReadinessAnswer::fails(
                ITEM,
                waiting(id, ITEM, ReadinessProblem::CheckRefused)
                    .game(self.game)
                    .caused_by(own.outcome.clone()),
            );
        }
        if self.refusals.iter().any(|refusal| refusal.carried_by(id)) {
            return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::CheckStopped);
        }
        let mut outcome = waiting(id, ITEM, ReadinessProblem::RootRefused).game(self.game);
        if let Some(named) = &first.package_id {
            outcome = outcome.blocking_package_id(named.as_str());
        }
        ReadinessAnswer::fails(ITEM, outcome.caused_by(first.outcome.clone()))
    }

    /// I1: an instance is bound to the pack's game and server.
    fn instance_bound(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::InstanceBound;
        let id = pack.package_id;
        let bound = match part(self.installation, |installation| {
            installation.bound.as_ref()
        }) {
            Ok(bound) => bound,
            Err(why) => return ReadinessAnswer::not_judged(ITEM, id, why),
        };
        let of_game = bound
            .iter()
            .filter(|binding| binding.game == self.game)
            .collect::<Vec<_>>();
        let failed = |problem| {
            waiting(id, ITEM, problem)
                .game(self.game)
                .server(pack.server)
        };
        if of_game.is_empty() {
            return ReadinessAnswer::fails(ITEM, failed(ReadinessProblem::InstanceUnbound));
        }
        if of_game
            .iter()
            .any(|binding| binding.server.as_deref() == Some(pack.server))
        {
            return ReadinessAnswer::holds(ITEM, None);
        }
        if of_game.iter().any(|binding| binding.server.is_none()) {
            return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::ServerNotTold);
        }
        ReadinessAnswer::fails(ITEM, failed(ReadinessProblem::ServerUnbound))
    }

    /// I2: the installation's `package` range holds the bundle's declared maximum, as `place`
    /// holds a bundle at the edge.
    fn package_revision(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::PackageRevision;
        let id = pack.package_id;
        let [least, most] = match part(self.installation, |installation| {
            installation.package.as_ref()
        }) {
            Ok(range) => *range,
            Err(why) => return ReadinessAnswer::not_judged(ITEM, id, why),
        };
        let Some([_, declared]) = self.declared_package else {
            return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::PackageUndeclared);
        };
        if least <= declared && declared <= most {
            return ReadinessAnswer::holds(ITEM, None);
        }
        ReadinessAnswer::fails(
            ITEM,
            waiting(id, ITEM, ReadinessProblem::RevisionOutside)
                .required_revision(declared)
                .range_min(least)
                .range_max(most),
        )
    }

    /// I3: an information line, never a refusal (the coordinator's ruling 3).
    fn unrecorded_screen_code(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::UnrecordedScreenCode;
        ReadinessAnswer::new(
            ITEM,
            ItemState::Information,
            Some(unjudged(
                pack.package_id,
                ITEM,
                ReadinessUnjudged::NoRevisionTells,
            )),
        )
    }

    /// I4: the live build names every `requires` word of the game's rules.
    fn requires_known(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::RequiresKnown;
        let id = pack.package_id;
        if let Err(why) = part(self.installation, |installation| {
            installation.subsystems.as_ref()
        }) {
            return ReadinessAnswer::not_judged(ITEM, id, why);
        }
        match self.rules {
            Rules::Absent { .. } => ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::NoSchedule)),
            Rules::Refused(_) => {
                ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::CompositionRefused)
            }
            Rules::Composed(rules) => match &rules.requires {
                Some(Ok(())) => ReadinessAnswer::holds(ITEM, None),
                Some(Err(refusal)) => ReadinessAnswer::fails(
                    ITEM,
                    waiting(id, ITEM, ReadinessProblem::RequiresUnknown)
                        .game(self.game)
                        .caused_by(refusal.clone()),
                ),
                None => ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::PartNotRead),
            },
        }
    }

    /// I5: no pair of the pack is suspended.
    fn pair_unsuspended(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::PairUnsuspended;
        let id = pack.package_id;
        let suspended = match part(self.installation, |installation| {
            installation.suspended.as_ref()
        }) {
            Ok(suspended) => suspended,
            Err(why) => return ReadinessAnswer::not_judged(ITEM, id, why),
        };
        let Some(pair) = suspended.iter().find(|pair| pair.package_id == id) else {
            return ReadinessAnswer::holds(ITEM, None);
        };
        let mut outcome = waiting(id, ITEM, ReadinessProblem::PairSuspended);
        if let Some(task_id) = &pair.task_id {
            outcome = outcome.task_id(task_id.as_str());
        }
        if let Some(instance_id) = &pair.instance_id {
            outcome = outcome.instance_id(instance_id.as_str());
        }
        ReadinessAnswer::fails(ITEM, outcome)
    }

    /// I6: the installation holds the models of S6 with their identity.
    fn models_installed(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::ModelsInstalled;
        let id = pack.package_id;
        let installed = match part(self.installation, |installation| {
            installation.models.as_ref()
        }) {
            Ok(installed) => installed,
            Err(why) => return ReadinessAnswer::not_judged(ITEM, id, why),
        };
        let Ok(descriptor) = pack.admitted else {
            return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::NotAdmitted);
        };
        let needed = Pack::models(descriptor);
        if let Some(missing) = needed.iter().find(|model| !installed.contains(model)) {
            return ReadinessAnswer::fails(
                ITEM,
                waiting(id, ITEM, ReadinessProblem::ModelMissing)
                    .model_name(missing.model_ref.as_str()),
            );
        }
        ReadinessAnswer::holds(ITEM, None).with_models(needed)
    }
}
