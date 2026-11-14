// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #464 P3 (register §1.375; `contracts/standard-pack-schedule.md`, "Readiness"; the
//! design `rd-1114/464-pack-build-design.md` F.1 with the coordinator's rulings on its questions):
//! for every pack of one game root, whether the scheduler can dispatch it, or every reason it
//! waits. One function judges the twelve items, [`judge_standard_pack_readiness`]; `actinglab
//! standard-pack ready --root <game root>` answers it, and adopt's answer, the build's
//! `build.json` and the placement plan are to call it with what each of them holds.
//!
//! The game root is an installed one, `<standard_packs.root>/<game>/`, or a bundle laid out as one
//! (`package bundle`'s output in a directory named as its game). It is read as `standard-pack
//! check` reads it: the root files by the one structure check of every reader
//! (`read_standard_pack_root`), a refused or missing root file refusing the whole answer as that
//! check refuses it. Then, per pack of the registry, in its order, each item is judged by the
//! function that judges it elsewhere, and answered `holds`, `fails` (with a
//! `package_dispatch_waiting` naming the item, the problem and its values, the other check's own
//! outcome as its cause), `not_judged` (with a `package_readiness_unjudged` naming why) or, item
//! I3 only, `information`:
//! - S1 `admitted`: the Runtime's loader, `PreparedContainedTask::describe_navigation_path`, the
//!   admission `standard-pack check` and `package bundle` run for every pack of the registry;
//! - S2 `classified`: the classification of the composer (`classify_standard_pack_packages` over
//!   `standard_pack_maintenance_packages`), after the composer accepted `schedule.json`;
//! - S3 `valued`: the composer's coverage of each rule of the pack (`benefit_warnings`,
//!   `benefit_pending`, `benefit_followers`), the maintenance tier exempt;
//! - S4 `composed`: the composer, `compose_standard_pack_rules`, with one instance per server
//!   (`crate::standard_pack_schedule::compose_per_server`, the composition `standard-pack check`
//!   runs);
//! - S5 `entry_reachable`: the admitted descriptor's entry (`entry_pages`,
//!   `prerequisite_package_id`, `navigation_entry_pages`) against the registry, the page list and
//!   the server's return-home package (`return_home_package`), as the Runtime's gate resolves it
//!   (`contracts/linear-steps.md`, "Return-home fallback");
//! - S6 `models_named`: the admitted descriptor's `recognition_models`, what `standard-pack check`
//!   answers as `models` (the loader refuses a target that names no model, so S6 fails only with
//!   S1);
//! - I1 to I6 from [`ReadinessInstallation`], what the caller read of an installation: I1 its
//!   bindings, I2 its `package` range against the bundle's declared maximum (as `place` holds a
//!   bundle at the edge), I3 an information line (the coordinator's ruling 3: no revision tells
//!   it), I4 the composer again with the live build's subsystems (as check-config and the Runtime
//!   check `requires`), I5 its suspended pairs, I6 its verified models.
//!
//! A pack is `dispatchable` when every item but I3 holds, `waiting` when one fails, and
//! `not_judged` otherwise. It reads the game root only: it writes nothing, starts nothing and reads
//! no installation itself.

use crate::codes::{ReadinessItem, ReadinessProblem, ReadinessUnjudged, ResourceToolingCode};
use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::{CHECK_LABEL, read_bounded, root_invalid, root_issue_outcome};
use crate::standard_pack_goals::root_irreversibility;
use crate::standard_pack_schedule::compose_per_server;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, BundleMaintenance, LabError, LabResult, RuntimeErrorCode,
    STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackProblem, StandardPackRoot, StandardPackRootBytes, StandardPackRootIssue,
    StandardPackSchedule, classify_standard_pack_packages, parse_standard_pack_schedule,
    read_standard_pack_root, standard_pack_maintenance_packages,
};
use actingcommand_execution_kernel::{NavigationPackageDescriptor, PreparedContainedTask};
use actingcommand_policy::StandardPackComposeFailure;
use serde::{Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The twelve items, in the order every pack answers them, each with its id in the design.
const ITEMS: [(ReadinessItem, &str); 12] = [
    (ReadinessItem::Admitted, "S1"),
    (ReadinessItem::Classified, "S2"),
    (ReadinessItem::Valued, "S3"),
    (ReadinessItem::Composed, "S4"),
    (ReadinessItem::EntryReachable, "S5"),
    (ReadinessItem::ModelsNamed, "S6"),
    (ReadinessItem::InstanceBound, "I1"),
    (ReadinessItem::PackageRevision, "I2"),
    (ReadinessItem::UnrecordedScreenCode, "I3"),
    (ReadinessItem::RequiresKnown, "I4"),
    (ReadinessItem::PairUnsuspended, "I5"),
    (ReadinessItem::ModelsInstalled, "I6"),
];

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
    /// Whether an installation was read: without one only the six items of the game root are
    /// judged.
    pub installation: InstallationRead,
    /// How many packs are `dispatchable`, `waiting` and `not_judged`.
    pub dispatchable: usize,
    pub waiting: usize,
    pub not_judged: usize,
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
    pub readiness: PackState,
    /// The twelve items, S1 to S6, then I1 to I6.
    pub items: Vec<ReadinessAnswer>,
}

impl PackReadiness {
    /// The answer of `item`.
    pub fn item(&self, item: ReadinessItem) -> Option<&ReadinessAnswer> {
        self.items.iter().find(|answer| answer.item == item)
    }
}

/// A pack's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackState {
    /// Every item but I3 holds.
    Dispatchable,
    /// At least one item fails.
    Waiting,
    /// None fails, and at least one was not judged.
    NotJudged,
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
    /// The item's id in the design: `S1` to `S6`, `I1` to `I6`.
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
    let installation = request.installation.as_ref();
    let rules = Rules::of(&game, &checked, &root, installation)?;
    let context = Context {
        game: &game,
        root: &checked.root,
        rules: &rules,
        declared_package: request.declared_package,
        installation,
    };
    let mut packs = Vec::new();
    for package_id in checked.root.registry.package_ids() {
        let entry = checked.root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let directory = root.join(&entry.directory);
        let admitted = PreparedContainedTask::describe_navigation_path(
            CHECK_LABEL,
            &directory,
            Instant::now() + PACK_DEADLINE,
        )
        .map_err(|error| {
            Outcome::new(RuntimeErrorCode::PackageInvalid)
                .path(&entry.directory)
                .caused_by(error.outcome())
        });
        let pack = Pack {
            package_id,
            server: &entry.server,
            admitted,
        };
        packs.push(context.judge(&pack));
    }
    let count = |state: PackState| packs.iter().filter(|pack| pack.readiness == state).count();
    Ok(StandardPackReadiness {
        root: root.display().to_string(),
        game,
        installation: if installation.is_some() {
            InstallationRead::Read
        } else {
            InstallationRead::NotRead
        },
        dispatchable: count(PackState::Dispatchable),
        waiting: count(PackState::Waiting),
        not_judged: count(PackState::NotJudged),
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
    /// The chain followers, by id.
    followers: BTreeSet<String>,
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
        // Workflow #455 G-R2s (merge of main): with every package of the root read as
        // `standard-pack check` reads it, so the composition is the one the check runs.
        let irreversible =
            root_irreversibility(&checked.root, &|directory: &str| root.join(directory));
        let composition = match compose_per_server(
            game,
            &checked.root,
            &checked.maintenance,
            &bytes,
            &irreversible.packages,
            None,
        ) {
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
            .flat_map(|(_, followers)| followers.iter().map(|follower| follower.task_id.clone()))
            .collect::<BTreeSet<_>>();
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
                compose_per_server(
                    game,
                    &checked.root,
                    &checked.maintenance,
                    &bytes,
                    &irreversible.packages,
                    Some(subsystems),
                )
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

/// One pack of the registry and what its admission gave.
struct Pack<'a> {
    package_id: &'a str,
    server: &'a str,
    /// Its descriptor, or `package_invalid` (its directory) caused by the loader's refusal.
    admitted: Result<NavigationPackageDescriptor, Outcome>,
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

/// What every pack of one game root is judged against.
struct Context<'a> {
    game: &'a str,
    root: &'a StandardPackRoot,
    rules: &'a Rules,
    declared_package: Option<[u64; 2]>,
    installation: Option<&'a ReadinessInstallation>,
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
    /// The twelve items of `pack` and its verdict.
    fn judge(&self, pack: &Pack<'_>) -> PackReadiness {
        let items = vec![
            self.admitted(pack),
            self.classified(pack),
            self.valued(pack),
            self.composed(pack),
            self.entry_reachable(pack),
            self.models_named(pack),
            self.instance_bound(pack),
            self.package_revision(pack),
            self.unrecorded_screen_code(pack),
            self.requires_known(pack),
            self.pair_unsuspended(pack),
            self.models_installed(pack),
        ];
        let readiness = if items.iter().any(|item| item.state == ItemState::Fails) {
            PackState::Waiting
        } else if items.iter().any(|item| item.state == ItemState::NotJudged) {
            PackState::NotJudged
        } else {
            PackState::Dispatchable
        };
        PackReadiness {
            package_id: pack.package_id.to_owned(),
            server: pack.server.to_owned(),
            readiness,
            items,
        }
    }

    /// S1: the loader admits the pack.
    fn admitted(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::Admitted;
        match &pack.admitted {
            Ok(_) => ReadinessAnswer::holds(ITEM, None),
            Err(refusal) => ReadinessAnswer::fails(
                ITEM,
                waiting(pack.package_id, ITEM, ReadinessProblem::AdmissionRefused)
                    .game(self.game)
                    .caused_by(refusal.clone()),
            ),
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

    /// S3: every rule of the pack is in the maintenance tier or declares its value.
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
            if rules.followers.contains(&rule.id) {
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

    /// S4: the composer accepts the game's `schedule.json` with one instance per server.
    fn composed(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::Composed;
        match self.rules {
            Rules::Absent { .. } => ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::NoSchedule)),
            Rules::Refused(refusal) => ReadinessAnswer::fails(
                ITEM,
                waiting(pack.package_id, ITEM, ReadinessProblem::CompositionRefused)
                    .game(self.game)
                    .caused_by(refusal.clone()),
            ),
            Rules::Composed(_) => ReadinessAnswer::holds(ITEM, None),
        }
    }

    /// S5: the pack's entry is reached (module doc).
    fn entry_reachable(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::EntryReachable;
        let id = pack.package_id;
        let Ok(descriptor) = &pack.admitted else {
            return ReadinessAnswer::not_judged(ITEM, id, ReadinessUnjudged::NotAdmitted);
        };
        if descriptor.has_application_entry() {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::ApplicationEntry));
        }
        let Some(entry) = descriptor.descriptor().entry_pages() else {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::NoPageEntry));
        };
        let failed = |problem| {
            waiting(id, ITEM, problem)
                .game(self.game)
                .server(pack.server)
                .page_ids(entry.iter().cloned())
        };
        if let Some(prerequisite) = descriptor.descriptor().prerequisite_package_id() {
            return if self.root.registry.get(prerequisite).is_some() {
                ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::Prerequisite))
            } else {
                ReadinessAnswer::fails(ITEM, failed(ReadinessProblem::PrerequisiteUnregistered))
            };
        }
        let listed = self.root.navigation.is_some()
            && match (&self.root.pages, descriptor.navigation_entry_pages()) {
                (Some(pages), Some(pages_of_entry)) => pages_of_entry
                    .iter()
                    .all(|page| pages.pages.iter().any(|listed| listed.id == *page)),
                _ => false,
            };
        if listed {
            return ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::Navigation));
        }
        match self.root.registry.return_home_package(pack.server) {
            Some(return_home) if return_home != id => {
                ReadinessAnswer::holds(ITEM, Some(ReadinessBasis::ReturnHome))
            }
            _ => ReadinessAnswer::fails(ITEM, failed(ReadinessProblem::EntryUnreachable)),
        }
    }

    /// S6: the models the pack needs are named.
    fn models_named(&self, pack: &Pack<'_>) -> ReadinessAnswer {
        const ITEM: ReadinessItem = ReadinessItem::ModelsNamed;
        match &pack.admitted {
            Ok(descriptor) => {
                ReadinessAnswer::holds(ITEM, None).with_models(Pack::models(descriptor))
            }
            Err(_) => {
                ReadinessAnswer::not_judged(ITEM, pack.package_id, ReadinessUnjudged::NotAdmitted)
            }
        }
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
        let Ok(descriptor) = &pack.admitted else {
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
