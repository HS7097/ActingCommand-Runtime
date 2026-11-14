// SPDX-License-Identifier: AGPL-3.0-only

//! ROUT-L RL-1b (frozen model v1.0 §3.3–§3.6, §3.8, §3.9; Alice's ruling 51
//! 「直接动态加载，用到的时候再加载，不用了直接从内存卸载，这个应该是要放到调度器工作台的」): the
//! scheduler workbench of the host. When the configuration names no catalog (composer mode),
//! the workbench composes the schedule rules from the standard packs of the games in use with the
//! one pure composer of the policy crate, `compose_standard_pack_rules`, and activates the
//! composition without any approval.
//!
//! - **When it checks.** At the start of a policy cycle, at most once per
//!   `standard_packs.check_interval_ms` (and at the first cycle). A check `stat`s `schedule.json`
//!   and `bundle.json` of each game in use and reads a game root only when one of them changed
//!   or the bound instances changed: 「用到的时候再加载」.
//! - **At start** (Workflow #442 REM-1, register §1.326). Before the start's remedy recovery the
//!   workbench adopts the active generation the policy host loaded: its composition's procedure
//!   bindings are the host's procedure manifest, so the remedy table built at start reads the
//!   composed packages. The first check finds that generation adopted.
//! - **Games in use.** The games of the configured instances ([`games_in_use`]). A game root
//!   installed without a bound instance is never read. Pending decision 58 takes its recommended
//!   answer A: a paused instance still uses its rules; [`games_in_use`] is the one function to
//!   change for B.
//! - **Unload.** A game whose instances are gone, or whose root holds no `schedule.json`, is left
//!   out of the next composition; activating it drops the old generation, and once the memo
//!   (`standard_packs.cached_generations` older generations) lets it go its memory is freed:
//!   「不用了直接从内存卸载」. A composed generation keeps no source bytes in memory.
//! - **Activation (§3.5).** A composition whose rules digest differs from the active one's is
//!   staged with `fragments/<game>.json` and its composition record, and activated with the
//!   existing `catalog.transition_intent` and `catalog.activated` records, without a promotion
//!   and without any approval decision. The host's procedure manifest is the composition's.
//! - **Fail Loud (§3.6).** A `schedule.json` the composer refuses is recorded once per (game,
//!   sha256) as `standard_pack_root_invalid` with its problem and pointer; that game keeps its last
//!   good `schedule.json` (the active generation's fragment), and a game that never had one has no
//!   rules. The other games are composed as they are. An unclassified package is recorded once
//!   per (game, package, sha256). A broken game root (`bundle.json` and its siblings) is recorded
//!   once per fingerprint and keeps the active generation as it is until it is fixed. No rules is
//!   a valid state.
//! - **Static pool readings** (RL-1a review, #386 6399836824, item 1). A reading the configuration
//!   gives for a pool no composed game declares is left out of the evaluation (policy dispatch),
//!   and the workbench records `policy_pool_reading_unused` once per pool.
//! - **Readers outside the Runtime** (RL-1c, #411; board 6432236193, 6435964241).
//!   [`compose_configured_standard_packs`] composes what a composer-mode configuration schedules
//!   with the same composer, for check-config and `actingd suspended`; the task package admission
//!   after start waits for the workbench's first check (`await_first_standard_pack_check`), so it
//!   admits the composed procedures.

use super::standard_pack::{RootFailure, read_root_file};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{IoOp, Outcome, Record};
use actingcommand_contract::{
    BundleIndex, BundleMaintenance, CONFIG_CATALOG_PROVENANCE_FACT_KEY,
    CONFIG_CATALOG_WARNINGS_FACT_KEY, CatalogProvenanceEntry, CatalogValueKind, CatalogValueOrigin,
    ConfigCatalogProvenance, ConfigCatalogWarnings, IrreversiblePackages,
    STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    STANDARD_PACK_TABLES_DIRECTORY, StandardPackRoot, StandardPackRootBytes,
    read_standard_pack_root,
};
use actingcommand_policy::{
    BenefitProvenance, CompiledCatalog, STANDARD_PACK_OPERATION_ID, STANDARD_PACKS_CATALOG_ID,
    StandardPackBinding, StandardPackCompileWarning, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, StandardPackRuleIssue,
    compose_standard_pack_rules,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::time::SystemTime;

/// The composition record's schema, kept in each composed generation's directory.
const COMPOSITION_RECORD_SCHEMA: &str = "actingcommand.standard-pack-composition.v1";
/// RL-1c: how long the task package admission after start waits at a time for the workbench's
/// first check before it looks at the shutdown request again. It bounds only how soon a stop ends
/// a wait; the first check itself wakes the admission at once.
const FIRST_CHECK_WAIT_TICK: Duration = Duration::from_millis(50);

/// The size and modification time of one root file, `None` when it does not exist.
type FileMark = Option<(u64, Option<SystemTime>)>;

/// Workflow #455 G-R2s (register §1.365; `contracts/standard-pack-schedule.md`, "The irreversible
/// mark"): the files of a package, below its directory, the workbench marks for its declaration.
/// The declaration reader (`PreparedContainedTask::read_remedy_declarations`) reads the whole
/// content directory of the package, so the set of files it opens follows the directory's listing
/// and is not fixed; the coordinator's ruling of 11-14 marks the two it opens by name before any
/// file says anything: the control file `control.json` and the source package's own manifest
/// `resources/operations/resources.json`.
const DECLARATION_FILES: [&[&str]; 2] = [
    &["control.json"],
    &["resources", "operations", "resources.json"],
];

/// G-R2s: the instance label the workbench reads a package's declaration under; the declaration
/// does not depend on one.
const DECLARATIONS_LABEL: &str = "standard-pack-workbench";

/// G-R2s: how long the workbench and its offline readers may read one package's declaration, as
/// the remedy table may.
const DECLARATIONS_BUDGET: Duration = Duration::from_secs(60);

/// G-R2s: the mark of one declaration file of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclarationMark {
    /// Its size and modification time, `None` when it does not exist ([`file_mark`]).
    Read(FileMark),
    /// Its metadata could not be read for another reason than its absence: a mark of its own,
    /// unequal to every readable one, so the root is read again once when it appears or goes and
    /// the package counts as unreadable meanwhile; it never stops the check.
    Failed,
}

/// The marks of a game root the workbench compares at each check: `schedule.json`,
/// `bundle.json` and, G-R2s, the declaration files of each package the root registered when the
/// workbench last read it, by package id.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GameMarks {
    schedule: FileMark,
    bundle: FileMark,
    packages: BTreeMap<String, Vec<DeclarationMark>>,
}

/// G-R2s fix round 1, for the gate tests: the declaration files whose metadata reads fail as a
/// file the process may not stat fails (another error than its absence), by path. Each test
/// names files below its own temporary root, so tests running at once do not meet.
#[cfg(test)]
pub(crate) static DECLARATION_METADATA_FAILS_FOR_TEST: std::sync::Mutex<BTreeSet<PathBuf>> =
    std::sync::Mutex::new(BTreeSet::new());

/// G-R2s fix round 1: whether a gate test makes the metadata of the declaration file at `path`
/// fail ([`DECLARATION_METADATA_FAILS_FOR_TEST`]).
#[cfg(test)]
fn metadata_fails_for_test(path: &Path) -> bool {
    DECLARATION_METADATA_FAILS_FOR_TEST
        .lock()
        .is_ok_and(|failing| failing.contains(path))
}

/// G-R2s fix round 1: outside the gate tests no declaration file's metadata is made to fail.
#[cfg(not(test))]
const fn metadata_fails_for_test(_: &Path) -> bool {
    false
}

/// G-R2s: the marks of the declaration files ([`DECLARATION_FILES`]) of the package whose
/// directory is `package`, in that order.
fn declaration_marks(package: &Path) -> Vec<DeclarationMark> {
    DECLARATION_FILES
        .iter()
        .map(|parts| {
            let path = parts
                .iter()
                .fold(package.to_path_buf(), |path, part| path.join(part));
            if metadata_fails_for_test(&path) {
                return DeclarationMark::Failed;
            }
            match file_mark(&path) {
                Ok(mark) => DeclarationMark::Read(mark),
                Err(_) => DeclarationMark::Failed,
            }
        })
        .collect()
}

/// The scheduler workbench of one host (composer mode).
pub(super) struct StandardPackWorkbench {
    /// The configuration manifest's subsystems each `requires` must name; `None` when the host
    /// has no manifest (then `requires` is not checked).
    subsystems: Option<BTreeSet<String>>,
    state: Mutex<WorkbenchState>,
    /// RL-1c: notified when a check has ended, for the admission waiting for the first one.
    checked: Condvar,
}

impl StandardPackWorkbench {
    pub(super) fn new(manifest: Option<&RuntimeConfigManifest>) -> Self {
        Self {
            subsystems: enabled_subsystems(manifest),
            state: Mutex::new(WorkbenchState::default()),
            checked: Condvar::new(),
        }
    }
}

/// The enabled subsystems of the configuration manifest, which each `requires` must name; `None`
/// without a manifest.
fn enabled_subsystems(manifest: Option<&RuntimeConfigManifest>) -> Option<BTreeSet<String>> {
    manifest.map(|manifest| {
        manifest
            .subsystems
            .iter()
            .filter(|subsystem| subsystem.enabled)
            .map(|subsystem| subsystem.name.clone())
            .collect()
    })
}

/// What the workbench remembers between two checks (memory only).
#[derive(Default)]
struct WorkbenchState {
    /// The monotonic time of the last check.
    checked_at_ms: Option<u64>,
    /// The bound instances the last composition used.
    bound: BTreeMap<String, StandardPackBinding>,
    /// The marks of each game in use when the last composition read it.
    marks: BTreeMap<String, GameMarks>,
    /// G-R2s: the packages each game root registered when the workbench last read it, each with
    /// its directory: whose declaration files the next check marks.
    packages: BTreeMap<String, BTreeMap<String, PathBuf>>,
    /// Whether the workbench has read the active generation once (at its first check).
    adopted: bool,
    /// The active generation the workbench last saw, and its composition record when it is a
    /// composed one.
    active_hash: Option<String>,
    active: Option<CompositionRecord>,
    /// The composed procedures of the active composition, by `procedure_ref`.
    procedures: BTreeMap<String, ComposedProcedureRecord>,
    /// Each (game, sha256 or root marks) whose refusal is recorded.
    reported: BTreeSet<(String, String)>,
    /// Each (game, package, sha256) whose `standard_pack_package_unclassified` is recorded.
    unclassified: BTreeSet<(String, String, String)>,
    /// Each static pool reading recorded as unused.
    unused_pools: BTreeSet<String>,
    /// How many times each game's `schedule.json` was opened (acceptance A4).
    schedule_reads: BTreeMap<String, u64>,
    /// Workflow #455 G-R2s round 2: the games in use whose root a check could not compose, set
    /// where the check keeps the active generation ([`note_uncomposed`]) and cleared by the
    /// first composition that succeeds. Every instance of such a game has no goals in effect.
    uncomposed: BTreeSet<String>,
}

/// Workflow #455 G-R2s round 2 (register §1.365; `contracts/resource-targets.md`, "A task that
/// can do something irreversible"): at a check that keeps the active generation (the root's
/// metadata cannot be read, the root is damaged, the composition is refused as a whole, or no
/// refused game can be taken out), notes as uncomposed `failed`, the game whose root failed, and
/// every game in use whose marks this check did not find equal to those of its last composed read
/// (`state.marks`): a game whose files have not changed since is composed in the active generation
/// as it is now, its marks those of its declarations; any other may have changed into something
/// the active generation does not mark. A game this check did not reach has no marks here, so it
/// is noted too.
fn note_uncomposed(
    state: &mut WorkbenchState,
    games: &BTreeSet<String>,
    marks: &BTreeMap<String, GameMarks>,
    failed: Option<&str>,
) {
    for game in games {
        let current = marks.get(game);
        if failed == Some(game.as_str()) || current.is_none() || current != state.marks.get(game) {
            state.uncomposed.insert(game.clone());
        }
    }
}

/// The composition record of a composed generation (`standard-packs.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompositionRecord {
    schema_version: String,
    rules_digest: String,
    /// Each game in use, with the sha256 of the `schedule.json` it composed (`None`: no rules).
    games: BTreeMap<String, Option<String>>,
    procedures: Vec<ComposedProcedureRecord>,
    /// The composed pool ids.
    pools: Vec<String>,
}

/// One composed procedure: the package a `package.<package_id>` procedure runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposedProcedureRecord {
    procedure_ref: String,
    game_id: String,
    package_id: String,
    directory: String,
}

/// One game root as the composer reads it.
struct ScheduleGame {
    root: StandardPackRoot,
    maintenance: Vec<BundleMaintenance>,
    /// `schedule.json`'s bytes; `None` when the root holds none.
    schedule: Option<Vec<u8>>,
    /// G-R2s: the declarations of every package the root registers, read when the root holds
    /// `schedule.json` (the composer's required input); `None` without it.
    declarations: Option<RootDeclarations>,
}

/// G-R2s (register §1.365): one game root's package declarations as the composer takes them.
struct RootDeclarations {
    /// Which packages can do something irreversible, by the whole root, and which could not be
    /// read.
    irreversible: IrreversiblePackages,
    /// The reader's refusal of each package it could not read, by package id.
    refusals: BTreeMap<String, Outcome>,
    /// Each package's directory and the marks of its declaration files, taken just before the
    /// reader read it, by package id: what the workbench's next check compares.
    marks: BTreeMap<String, PackageMarks>,
}

/// G-R2s: one package's directory and the marks of its declaration files.
struct PackageMarks {
    directory: PathBuf,
    marks: Vec<DeclarationMark>,
}

/// G-R2s (register §1.365; `contracts/standard-pack-schedule.md`, "The irreversible mark"): what
/// every package `root`'s registry lists provides, each read below the game root `directory`
/// through the reader the goal check uses (`PreparedContainedTask::read_remedy_declarations`), into
/// the one copy of the rule ([`IrreversiblePackages::new`]). Each package's declaration files are
/// marked first ([`declaration_marks`]), so a file that changes after its mark is seen by the next
/// check. A package one of whose marked files could not be stat'ed, or that the reader refuses,
/// could not be read: never "reversible".
fn read_declarations(directory: &Path, root: &StandardPackRoot) -> RootDeclarations {
    let mut declared = BTreeMap::new();
    let mut refusals = BTreeMap::new();
    let mut marks = BTreeMap::new();
    for package_id in root.registry.package_ids() {
        let Some(entry) = root.registry.get(package_id) else {
            declared.insert(package_id.to_owned(), None);
            continue;
        };
        let package = directory.join(&entry.directory);
        let package_marks = declaration_marks(&package);
        let capabilities = if package_marks.contains(&DeclarationMark::Failed) {
            None
        } else {
            match PreparedContainedTask::read_remedy_declarations(
                DECLARATIONS_LABEL,
                &package,
                Instant::now() + DECLARATIONS_BUDGET,
            ) {
                Ok(declarations) => {
                    let mut capabilities = BTreeMap::new();
                    for provided in &declarations.provides {
                        *capabilities
                            .entry(provided.capability().to_owned())
                            .or_insert(false) |= provided.irreversible();
                    }
                    Some(capabilities)
                }
                Err(error) => {
                    refusals.insert(package_id.to_owned(), error.outcome());
                    None
                }
            }
        };
        declared.insert(package_id.to_owned(), capabilities);
        marks.insert(
            package_id.to_owned(),
            PackageMarks {
                directory: package,
                marks: package_marks,
            },
        );
    }
    RootDeclarations {
        irreversible: IrreversiblePackages::new(declared),
        refusals,
        marks,
    }
}

/// G-R2s: the `standard_pack_package_unreadable` of the package `package_id` of `game` (keys
/// `game`, `package_id`), with the reader's refusal as its cause when `files`' reader refused it
/// (none when one of its declaration files could not be stat'ed): the workbench records it, and
/// check-config shows it (fix round 1).
fn unreadable_package(game: &str, package_id: &str, files: Option<&ScheduleGame>) -> Outcome {
    let outcome = Outcome::new(SharedCode::StandardPackPackageUnreadable)
        .game(game)
        .package_id(package_id);
    match files
        .and_then(|files| files.declarations.as_ref())
        .and_then(|declarations| declarations.refusals.get(package_id))
    {
        Some(refusal) => outcome.caused_by(refusal.clone()),
        None => outcome,
    }
}

/// ROUT-L RL-1b, pending decision 58 (recommended A): the bound instances whose games are in
/// use. Every configured instance is; an operator pause does not unload a game's rules. Answer B
/// would leave out an instance under a whole-instance pause here.
fn games_in_use(identities: &[(String, String, String)]) -> BTreeMap<String, StandardPackBinding> {
    identities
        .iter()
        .map(|(alias, game_id, server_id)| {
            (
                alias.clone(),
                StandardPackBinding {
                    game_id: game_id.clone(),
                    server_id: server_id.clone(),
                },
            )
        })
        .collect()
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// The mark of the file at `path`.
fn file_mark(path: &Path) -> Result<FileMark, RootFailure> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some((metadata.len(), metadata.modified().ok()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RootFailure::Unreadable {
            path: path.to_owned(),
            error,
            op: IoOp::Metadata,
        }),
    }
}

/// Reads one game root for the composer: its root files by the contract crate's one check, the
/// bundle's maintenance entries, and `schedule.json` when it holds one (counted in `reads`);
/// G-R2s: with `schedule.json`, the declarations of every package its registry lists
/// ([`read_declarations`]). `None` when the directory holds no game root.
fn read_schedule_game(
    directory: &Path,
    game: &str,
    reads: &mut BTreeMap<String, u64>,
) -> Result<Option<ScheduleGame>, RootFailure> {
    let path_of = |file: &str| directory.join(file);
    let bundle = read_root_file(&path_of(STANDARD_PACK_BUNDLE_FILE))?;
    let pages = read_root_file(&path_of(STANDARD_PACK_PAGES_FILE))?;
    let interrupts = read_root_file(&path_of(STANDARD_PACK_INTERRUPTS_FILE))?;
    let navigation = read_root_file(&path_of(STANDARD_PACK_NAVIGATION_FILE))?;
    // Workflow #392 DA-4: checked as every reader checks it, so no reader accepts what another
    // refuses.
    let display_layout = read_root_file(&path_of(STANDARD_PACK_DISPLAY_LAYOUT_FILE))?;
    let tables_path = path_of(STANDARD_PACK_TABLES_DIRECTORY);
    let tables = match fs::symlink_metadata(&tables_path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(RootFailure::Unreadable {
                path: tables_path,
                error,
                op: IoOp::Metadata,
            });
        }
    };
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: bundle.as_deref(),
            pages: pages.as_deref(),
            interrupts: interrupts.as_deref(),
            navigation: navigation.as_deref(),
            tables,
            display_layout: display_layout.as_deref(),
        },
        game,
    )
    .map_err(|issue| RootFailure::Invalid {
        path: path_of(issue.file),
        issue,
    })?;
    let Some(root) = root else {
        return Ok(None);
    };
    let maintenance = bundle
        .as_deref()
        .and_then(|bundle| serde_json::from_slice::<BundleIndex>(bundle).ok())
        .map(|index| index.maintenance().to_vec())
        .unwrap_or_default();
    let schedule_path = path_of(STANDARD_PACK_SCHEDULE_FILE);
    let schedule = if schedule_path.exists() {
        *reads.entry(game.to_owned()).or_default() += 1;
        read_root_file(&schedule_path)?
    } else {
        None
    };
    let declarations = schedule
        .as_ref()
        .map(|_| read_declarations(directory, &root));
    Ok(Some(ScheduleGame {
        root,
        maintenance,
        schedule,
        declarations,
    }))
}

impl CompositionRecord {
    fn of(composition: &StandardPackComposition) -> Self {
        Self {
            schema_version: COMPOSITION_RECORD_SCHEMA.to_owned(),
            rules_digest: composition.rules_digest().to_owned(),
            games: composition
                .games()
                .iter()
                .map(|game| (game.game_id.clone(), game.schedule_sha256.clone()))
                .collect(),
            procedures: composition
                .manifest()
                .iter()
                .map(|procedure| ComposedProcedureRecord {
                    procedure_ref: procedure.procedure_ref.clone(),
                    game_id: procedure.game_id.clone(),
                    package_id: procedure.package_id.clone(),
                    directory: procedure.directory.clone(),
                })
                .collect(),
            pools: composition
                .compiled()
                .catalog()
                .pools
                .pools
                .iter()
                .map(|pool| pool.id.clone())
                .collect(),
        }
    }
}

/// #428 SCHED-C1b: one game's provenance as the composer gives it, as the entries of
/// `config.catalog_provenance`: the fixed values, then the valuations, each by the pack's hand,
/// then by the model's export, each list in file order.
fn provenance_entries(game: &str, provenance: &BenefitProvenance) -> Vec<CatalogProvenanceEntry> {
    let kinds = [
        (CatalogValueKind::FixedValue, &provenance.fixed_value),
        (CatalogValueKind::Valuation, &provenance.valuation),
    ];
    let mut entries = Vec::new();
    for (kind, origins) in kinds {
        for (origin, ids) in [
            (CatalogValueOrigin::Pack, &origins.pack),
            (CatalogValueOrigin::Model, &origins.model),
        ] {
            for id in ids {
                entries.push(CatalogProvenanceEntry {
                    game: game.to_owned(),
                    kind,
                    origin,
                    id: id.clone(),
                });
            }
        }
    }
    entries
}

/// The composer's per-game files of every game of `read` whose rules `schedule` gives (`None`
/// leaves the game out), each with its root, its maintenance entries and, G-R2s, the
/// irreversibility of its packages as [`read_declarations`] read them: a game whose root holds
/// `schedule.json` always has them, so a game without them is left out too.
fn composer_inputs<'a>(
    read: &'a BTreeMap<String, ScheduleGame>,
    schedule: impl Fn(&str) -> Option<&'a [u8]>,
) -> BTreeMap<String, StandardPackGameFiles<'a>> {
    read.iter()
        .filter_map(|(game, files)| {
            let schedule = schedule(game.as_str())?;
            let declarations = files.declarations.as_ref()?;
            Some((
                game.clone(),
                StandardPackGameFiles {
                    schedule,
                    root: &files.root,
                    maintenance: files.maintenance.as_slice(),
                    irreversible: &declarations.irreversible,
                },
            ))
        })
        .collect()
}

/// The refusal record of one issue of `game`'s `schedule.json` at `path`.
fn schedule_refusal(game: &str, path: &Path, issue: &StandardPackRuleIssue) -> Outcome {
    RootFailure::Invalid {
        path: path.to_owned(),
        issue: issue.issue.clone(),
    }
    .outcome(Some(game))
}

impl HostShared {
    /// ROUT-L RL-1b: the scheduler workbench's keys in effect.
    fn standard_pack_workbench_settings(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::StandardPackWorkbenchConfig> {
        lock(&self.runtime_config, HostLocation::StandardPackRead)?
            .standard_pack_workbench()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    HostLocation::StandardPackRead,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// ROUT-L RL-1b (model §3.3): the workbench's check at the start of a policy cycle at
    /// `monotonic_ms`; nothing in configured mode, and nothing until `check_interval_ms` has
    /// passed since the last check. A refused file or root is recorded and never fails the
    /// cycle; only a ledger, store or state failure does.
    pub(super) fn check_standard_pack_rules(&self, monotonic_ms: u64) -> RuntimeHostResult<()> {
        let Some(workbench) = self.standard_packs.schedule.as_ref() else {
            return Ok(());
        };
        let settings = self.standard_pack_workbench_settings()?;
        let mut state = lock(&workbench.state, HostLocation::StandardPackRead)?;
        if state.checked_at_ms.is_some_and(|checked| {
            monotonic_ms.saturating_sub(checked) < settings.check_interval_ms
        }) {
            return Ok(());
        }
        state.checked_at_ms = Some(monotonic_ms);
        let checked = self
            .policy()
            .and_then(|policy| lock(policy, HostLocation::StandardPackRead))
            .and_then(|policy| policy.set_cached_generations(settings.cached_generations))
            .and_then(|()| self.refresh_standard_pack_rules(workbench, &mut state));
        // RL-1c: the admission after start waits for the first check; whatever this check's
        // result, the procedure bindings it leaves are the ones to admit.
        drop(state);
        workbench.checked.notify_all();
        checked
    }

    /// ROUT-L RL-1c (#411; `pack_admission.rs`, the admission after start): in composer mode the
    /// procedure bindings are the composition's, which the workbench installs at its first check
    /// (the active composed generation's, then the composition of the files as they are). Waits
    /// until that check has ended and answers `true`, or answers `false` once a stop is
    /// requested first. Without a workbench (configured mode) it answers `true` at once.
    pub(super) fn await_first_standard_pack_check(&self) -> RuntimeHostResult<bool> {
        let Some(workbench) = self.standard_packs.schedule.as_ref() else {
            return Ok(true);
        };
        let mut state = lock(&workbench.state, HostLocation::CheckPackAdmission)?;
        // A check holds the state for its whole run, so a set `checked_at_ms` seen here is a
        // check that has ended.
        while state.checked_at_ms.is_none() {
            if self.fatal.is_shutdown_requested() {
                return Ok(false);
            }
            state = workbench
                .checked
                .wait_timeout(state, FIRST_CHECK_WAIT_TICK)
                .map_err(|_| lock_poison_error(HostLocation::CheckPackAdmission))?
                .0;
        }
        Ok(true)
    }

    /// Workflow #442 REM-1 (register §1.326; `contracts/remedies.md` section 7.1): at start, in
    /// composer mode, the workbench adopts the active generation `policy` loaded, as its first
    /// check would: the procedure bindings of its composition record are the host's procedure
    /// manifest before the start's remedy recovery builds the table from it. The first check then
    /// finds the generation adopted and does not adopt it again; it still reads every game in use
    /// and composes as before. Nothing in configured mode. A generation that is not a composed
    /// one, or whose record cannot be used, gives the manifest that binds no procedure, as the
    /// check gives it.
    pub(super) fn adopt_standard_pack_generation_on_start(
        &self,
        policy: &PolicyHost,
    ) -> RuntimeHostResult<()> {
        let Some(workbench) = self.standard_packs.schedule.as_ref() else {
            return Ok(());
        };
        let active = policy.active_generation();
        let record = match &active {
            Some(generation) if generation.catalog_id() == STANDARD_PACKS_CATALOG_ID => {
                policy.composition_record(generation.catalog_hash())?
            }
            _ => None,
        };
        let mut state = lock(&workbench.state, HostLocation::StandardPackRead)?;
        self.adopt_active_generation(
            &mut state,
            active.map(|generation| generation.catalog_hash().to_owned()),
            record,
            self.standard_packs.root(),
        )
    }

    /// Adopts the active generation `active_hash` with `record`, its composition record as read
    /// (`None` when it is no composed one or has none): its record when it is one of this schema,
    /// and every game in use read again at the next check. Until a composition succeeds the
    /// cycle runs the active generation's own procedures (none without a composed one): a broken
    /// root never leaves the cycle without a manifest.
    fn adopt_active_generation(
        &self,
        state: &mut WorkbenchState,
        active_hash: Option<String>,
        record: Option<Vec<u8>>,
        root: Option<&Path>,
    ) -> RuntimeHostResult<()> {
        state.adopted = true;
        state.active = record
            .and_then(|bytes| serde_json::from_slice::<CompositionRecord>(&bytes).ok())
            .filter(|record| record.schema_version == COMPOSITION_RECORD_SCHEMA);
        state.active_hash = active_hash;
        state.marks.clear();
        let record = state.active.clone().unwrap_or_else(|| CompositionRecord {
            schema_version: COMPOSITION_RECORD_SCHEMA.to_owned(),
            rules_digest: String::new(),
            games: BTreeMap::new(),
            procedures: Vec::new(),
            pools: Vec::new(),
        });
        self.install_composed_manifest(&record, root)?;
        state.procedures = record
            .procedures
            .iter()
            .map(|procedure| (procedure.procedure_ref.clone(), procedure.clone()))
            .collect();
        Ok(())
    }

    fn refresh_standard_pack_rules(
        &self,
        workbench: &StandardPackWorkbench,
        state: &mut WorkbenchState,
    ) -> RuntimeHostResult<()> {
        let identities = lock(&self.policy_inputs, HostLocation::ReadPolicyInputs)?
            .as_ref()
            .map(|inputs| {
                inputs
                    .instance_identities()
                    .map(|identity| {
                        (
                            identity.instance_id.to_owned(),
                            identity.game_id.to_owned(),
                            identity.server_id.to_owned(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let bound = games_in_use(&identities);
        let root = self.standard_packs.root().map(Path::to_path_buf);
        let active = lock(self.policy()?, HostLocation::StandardPackRead)?.active_generation();
        let active_hash = active
            .as_ref()
            .map(|generation| generation.catalog_hash().to_owned());
        if !state.adopted || state.active_hash != active_hash {
            // The active generation is not the one the workbench last adopted or activated (a
            // switch by another writer; #442 REM-1: the start adopts the one it loads, so its
            // first check does not adopt it again).
            let record = match &active {
                Some(generation) if generation.catalog_id() == STANDARD_PACKS_CATALOG_ID => {
                    lock(self.policy()?, HostLocation::StandardPackRead)?
                        .composition_record(generation.catalog_hash())?
                }
                _ => None,
            };
            self.adopt_active_generation(state, active_hash, record, root.as_deref())?;
        }
        let games = bound
            .values()
            .map(|binding| binding.game_id.clone())
            .collect::<BTreeSet<_>>();
        let mut marks = BTreeMap::new();
        if let Some(root) = &root {
            for game in &games {
                let directory = root.join(game);
                let read =
                    file_mark(&directory.join(STANDARD_PACK_SCHEDULE_FILE)).and_then(|schedule| {
                        file_mark(&directory.join(STANDARD_PACK_BUNDLE_FILE))
                            .map(|bundle| (schedule, bundle))
                    });
                match read {
                    Ok((schedule, bundle)) => {
                        // G-R2s: and the declaration files of each package the root registered
                        // when the workbench last read it; a metadata failure there is that
                        // package's own mark and never stops the check.
                        let packages = state
                            .packages
                            .get(game)
                            .map(|packages| {
                                packages
                                    .iter()
                                    .map(|(package_id, package)| {
                                        (package_id.clone(), declaration_marks(package))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        marks.insert(
                            game.clone(),
                            GameMarks {
                                schedule,
                                bundle,
                                packages,
                            },
                        );
                    }
                    Err(failure) => {
                        self.report_once(
                            state,
                            game,
                            "root:metadata",
                            failure.outcome(Some(game.as_str())),
                        )?;
                        // G-R2s round 2: the game, and every game not found unchanged, has no
                        // goals in effect until a composition succeeds.
                        note_uncomposed(state, &games, &marks, Some(game.as_str()));
                        return Ok(());
                    }
                }
            }
        }
        // G-R2s round 2: while a game is noted uncomposed, the check reads and composes again
        // even when nothing changed, so a root that reads again ends it.
        if state.active.is_some()
            && state.bound == bound
            && state.marks == marks
            && state.uncomposed.is_empty()
        {
            return Ok(());
        }

        let mut read = BTreeMap::new();
        if let Some(root) = &root {
            for game in &games {
                match read_schedule_game(&root.join(game), game, &mut state.schedule_reads) {
                    Ok(Some(files)) => {
                        read.insert(game.clone(), files);
                    }
                    Ok(None) => {}
                    Err(failure) => {
                        // A broken game root keeps the active generation as it is until it is
                        // fixed: its registry is what every rule of the game is placed by.
                        let key = format!("root:{:?}", marks.get(game));
                        self.report_once(state, game, &key, failure.outcome(Some(game.as_str())))?;
                        // G-R2s round 2: as at a metadata failure.
                        note_uncomposed(state, &games, &marks, Some(game.as_str()));
                        return Ok(());
                    }
                }
            }
        }
        let mut schedules = read
            .iter()
            .filter_map(|(game, files)| {
                files
                    .schedule
                    .as_ref()
                    .map(|bytes| (game.clone(), bytes.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let catalog_version = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error(HostLocation::StandardPackRead))?
            .saturating_add(1);
        let mut fallen_back = BTreeSet::new();
        let composition = loop {
            let inputs = composer_inputs(&read, |game| schedules.get(game).map(Vec::as_slice));
            let failure = match compose_standard_pack_rules(&StandardPackComposeInput {
                instances: &bound,
                games: &inputs,
                subsystems: workbench.subsystems.as_ref(),
                catalog_version,
            }) {
                Ok(composition) => break composition,
                Err(failure) => failure,
            };
            let mut failed = BTreeMap::<String, &StandardPackRuleIssue>::new();
            let mut whole = Vec::new();
            for issue in failure.issues() {
                match &issue.game_id {
                    Some(game) => {
                        failed.entry(game.clone()).or_insert(issue);
                    }
                    None => whole.push(issue),
                }
            }
            if failed.is_empty() {
                // A refusal of the composition as a whole (its size) keeps the active
                // generation; it is recorded once per set of files composed.
                let key = format!(
                    "composition:{:?}",
                    schedules
                        .values()
                        .map(|bytes| sha256(bytes.as_slice()))
                        .collect::<Vec<_>>()
                );
                for issue in whole {
                    let game = issue.game_id.clone().unwrap_or_default();
                    let outcome = RootFailure::Invalid {
                        path: root.clone().unwrap_or_default(),
                        issue: issue.issue.clone(),
                    }
                    .outcome(None);
                    self.report_once(state, &game, &key, outcome)?;
                }
                // G-R2s round 2: every game not found unchanged has no goals in effect until a
                // composition succeeds.
                note_uncomposed(state, &games, &marks, None);
                return Ok(());
            }
            // Each round takes out at least one refused game, so the loop ends; a refusal that
            // names no game this round composed keeps the active generation.
            let mut progressed = false;
            for (game, issue) in failed {
                let Some(bytes) = schedules.remove(&game) else {
                    if let Some(root) = &root {
                        let path = root.join(&game).join(STANDARD_PACK_SCHEDULE_FILE);
                        let refusal = schedule_refusal(&game, &path, issue);
                        self.report_once(state, &game, "unplaced", refusal)?;
                    }
                    continue;
                };
                progressed = true;
                let sha = sha256(&bytes);
                if let Some(root) = &root {
                    let path = root.join(&game).join(STANDARD_PACK_SCHEDULE_FILE);
                    self.report_once(state, &game, &sha, schedule_refusal(&game, &path, issue))?;
                }
                // §3.6: the game keeps its last good `schedule.json`, the active generation's
                // fragment; a game that never had one has no rules.
                if fallen_back.insert(game.clone())
                    && let (Some(hash), Some(Some(good))) = (
                        state.active_hash.as_deref(),
                        state
                            .active
                            .as_ref()
                            .and_then(|record| record.games.get(&game)),
                    )
                    && *good != sha
                    && let Some(fragment) = lock(self.policy()?, HostLocation::StandardPackRead)?
                        .composed_fragment(hash, &game)?
                {
                    schedules.insert(game, fragment);
                }
            }
            if !progressed {
                // G-R2s round 2: as at a refusal as a whole.
                note_uncomposed(state, &games, &marks, None);
                return Ok(());
            }
        };

        for game in composition.games() {
            let Some(sha) = &game.schedule_sha256 else {
                continue;
            };
            for package_id in &game.unclassified {
                if state.unclassified.insert((
                    game.game_id.clone(),
                    package_id.clone(),
                    sha.clone(),
                )) {
                    self.record_standard_pack_read(
                        Outcome::new(SharedCode::StandardPackPackageUnclassified)
                            .operation(HostLocation::StandardPackRead)
                            .game(game.game_id.as_str())
                            .package_id(package_id.as_str()),
                    )?;
                }
            }
        }
        // G-R2s (register §1.365): a package of a composed game whose declaration could not be
        // read marks every task of its game; it is recorded once while it stays unreadable, with
        // the reader's refusal as its cause when the reader refused it, and again only after it
        // has read once.
        let unreadable = composition
            .unreadable_packages()
            .iter()
            .flat_map(|(game, packages)| {
                packages
                    .iter()
                    .map(move |package| (game.clone(), package.clone()))
            })
            .collect::<BTreeSet<_>>();
        for (game, files) in &read {
            if files.declarations.is_none() {
                continue;
            }
            for package_id in files.root.registry.package_ids() {
                let key = format!("declaration:{package_id}");
                if !unreadable.contains(&(game.clone(), package_id.to_owned())) {
                    state.reported.remove(&(game.clone(), key));
                    continue;
                }
                let outcome = unreadable_package(game, package_id, Some(files))
                    .operation(HostLocation::StandardPackRead);
                self.report_once(state, game, &key, outcome)?;
            }
        }
        let record = CompositionRecord::of(&composition);
        let unchanged = state
            .active
            .as_ref()
            .is_some_and(|active| active.rules_digest == record.rules_digest);
        if !unchanged {
            let fragments = composition
                .games()
                .iter()
                .filter_map(|game| {
                    schedules
                        .get(&game.game_id)
                        .map(|bytes| (game.game_id.clone(), bytes.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            let record_bytes = serde_json::to_vec_pretty(&record).map_err(|_| {
                RuntimeHostError::fatal(
                    HostCode::CatalogManifestEncodeFailed,
                    HostLocation::StageCatalogGeneration,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })?;
            let (catalog, previous) = {
                let policy = lock(self.policy()?, HostLocation::StagePolicyCatalog)?;
                if let Some(error) = self.fatal.answer()? {
                    return Err(error);
                }
                (
                    policy.stage_composed(&composition, &fragments, &record_bytes)?,
                    policy.active_generation(),
                )
            };
            // §3.5: the existing transition records, with no promotion and no approval.
            let generation = self.switch_policy_catalog(
                catalog,
                previous,
                EventAction::CatalogActivate,
                CatalogTransitionTarget::Activated,
                None,
            )?;
            let before = state.active.as_ref().map(|active| &active.games);
            for game in composition.games() {
                let previous_sha = before.and_then(|games| games.get(&game.game_id)).cloned();
                let Some(sha) = &game.schedule_sha256 else {
                    continue;
                };
                if previous_sha.as_ref() == Some(&Some(sha.clone())) {
                    continue;
                }
                // #428 CW-1 (fix 1, the coordinator's ruling: no new registration record): the
                // record the activation writes for a game whose file changed carries the compile
                // warnings that file gave. A game whose file did not change gets no record; its
                // warnings are in `config.catalog_warnings` with the rest of the generation's.
                let warnings = composition
                    .compile_warnings()
                    .iter()
                    .filter(|warning| warning.game_id.as_deref() == Some(game.game_id.as_str()))
                    .map(|warning| {
                        Record::new()
                            .warning_code(warning.code.code())
                            .json_pointer(warning.pointer.as_str())
                    })
                    .collect::<Vec<_>>();
                let count = read
                    .get(&game.game_id)
                    .map_or(0, |files| files.root.registry.len());
                let registered = Outcome::new(HostCode::StandardPackRegistered)
                    .operation(HostLocation::StandardPackRegister)
                    .game(game.game_id.as_str())
                    .count(count)
                    .schedule_sha256(sha.as_str())
                    .rules(game.tasks)
                    .held(game.held.len())
                    .unclassified(game.unclassified.len());
                self.record_standard_pack_read(if warnings.is_empty() {
                    registered
                } else {
                    registered.compile_warnings(warnings)
                })?;
            }
            state.active_hash = Some(generation.catalog_hash().to_owned());
            state.active = Some(record.clone());
        }
        // #428 CW-1: the active generation's compile warnings, as the overview shows them.
        self.record_catalog_warnings(&composition)?;
        // #428 SCHED-C1b: who wrote its fixed values and valuations, in the same place.
        self.record_catalog_provenance(&composition)?;
        // Item 1 of the RL-1a review: a configured reading of a pool no composed game declares
        // is left out of the evaluation; it is recorded once per pool.
        let configured_pools = lock(&self.policy_inputs, HostLocation::ReadPolicyInputs)?
            .as_ref()
            .map(|inputs| {
                inputs
                    .resources()
                    .pools
                    .iter()
                    .map(|pool| pool.pool_id.clone())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        for pool_id in configured_pools {
            if !record.pools.contains(&pool_id) && state.unused_pools.insert(pool_id.clone()) {
                self.record_standard_pack_read(
                    Outcome::new(HostCode::PolicyPoolReadingUnused)
                        .operation(HostLocation::StandardPackRead)
                        .pool_id(pool_id.as_str()),
                )?;
            }
        }
        self.install_composed_manifest(&record, root.as_deref())?;
        state.procedures = record
            .procedures
            .iter()
            .map(|procedure| (procedure.procedure_ref.clone(), procedure.clone()))
            .collect();
        state.bound = bound;
        // G-R2s: each composed root's packages as read now, with the marks of their declaration
        // files taken just before each was read, so the next check marks exactly these files and
        // compares them with what this read saw; a root without `schedule.json` marks none.
        state.packages.clear();
        for (game, files) in &read {
            let mut read_marks = BTreeMap::new();
            let mut directories = BTreeMap::new();
            for (package_id, package) in files
                .declarations
                .iter()
                .flat_map(|declarations| &declarations.marks)
            {
                read_marks.insert(package_id.clone(), package.marks.clone());
                directories.insert(package_id.clone(), package.directory.clone());
            }
            if let Some(game_marks) = marks.get_mut(game) {
                game_marks.packages = read_marks;
            }
            state.packages.insert(game.clone(), directories);
        }
        state.marks = marks;
        // G-R2s round 2: this composition succeeded, so every game in use is composed as it is
        // now; the goals of a game noted uncomposed are in effect again.
        state.uncomposed.clear();
        Ok(())
    }

    /// Workflow #455 G-R2s round 2 (register §1.365): the games in use whose root the
    /// workbench's checks could not compose since the last composition that succeeded
    /// ([`note_uncomposed`]); none in configured mode. The policy cycle reads them right after
    /// the check and leaves out the resource target record of each of their instances
    /// (`LeftOutFacts::void_targets`).
    pub(super) fn uncomposed_standard_pack_games(&self) -> RuntimeHostResult<BTreeSet<String>> {
        let Some(workbench) = self.standard_packs.schedule.as_ref() else {
            return Ok(BTreeSet::new());
        };
        Ok(lock(&workbench.state, HostLocation::StandardPackRead)?
            .uncomposed
            .clone())
    }

    /// Records `outcome` once per (`game`, `key`).
    fn report_once(
        &self,
        state: &mut WorkbenchState,
        game: &str,
        key: &str,
        outcome: Outcome,
    ) -> RuntimeHostResult<()> {
        if state.reported.insert((game.to_owned(), key.to_owned())) {
            self.record_standard_pack_read(outcome)?;
        }
        Ok(())
    }

    /// #428 CW-1 (register §1.290): records `config.catalog_warnings`, the compile warnings of
    /// `composition`, which the active generation holds (it was just activated, or its rules
    /// digest is the active one's): every game's, a game whose file did not change and so has no
    /// new `standard_pack_registered` record included (fix 1). Nothing is recorded when the stored value is the same, nor
    /// while the fact is absent and the composition has no warning, so a Runtime whose composed
    /// generations never had one records nothing new; one that had some records the fact empty.
    fn record_catalog_warnings(
        &self,
        composition: &StandardPackComposition,
    ) -> RuntimeHostResult<()> {
        let warnings = composition
            .compile_warnings()
            .iter()
            .map(StandardPackCompileWarning::to_contract)
            .collect::<Vec<_>>();
        let warnings = ConfigCatalogWarnings::of(&warnings);
        let record = warnings.to_fact_record(0, OriginModule::Policy);
        let unchanged = match lock(&self.runtime_facts, HostLocation::StandardPackRead)?
            .get(&RuntimeFactScope::Runtime, CONFIG_CATALOG_WARNINGS_FACT_KEY)
        {
            Some(stored) => stored.value == record.value,
            None => warnings.is_empty(),
        };
        if unchanged {
            return Ok(());
        }
        self.record_config_fact(record)
    }

    /// #428 SCHED-C1b (register §1.299; `contracts/standard-pack-schedule.md`, "Provenance"):
    /// records `config.catalog_provenance`, who wrote the fixed values and the valuation blocks of
    /// `composition`, which the active generation holds, every game's, as the composer gives them
    /// ([`StandardPackComposition::benefit_provenance`]). As `record_catalog_warnings` does:
    /// after the same compositions, so also when only an origin changed (no digest changes),
    /// nothing when the stored value is the same nor while the fact is absent and the composition
    /// has no such value, and the fact recorded empty once the active generation has none. No
    /// record of its own: the fact's `runtime.fact_recorded` is all the ledger gets.
    fn record_catalog_provenance(
        &self,
        composition: &StandardPackComposition,
    ) -> RuntimeHostResult<()> {
        let entries = composition
            .benefit_provenance()
            .iter()
            .flat_map(|(game, provenance)| provenance_entries(game, provenance))
            .collect::<Vec<_>>();
        let provenance = ConfigCatalogProvenance::of(&entries);
        let record = provenance.to_fact_record(0, OriginModule::Policy);
        let unchanged = match lock(&self.runtime_facts, HostLocation::StandardPackRead)?.get(
            &RuntimeFactScope::Runtime,
            CONFIG_CATALOG_PROVENANCE_FACT_KEY,
        ) {
            Some(stored) => stored.value == record.value,
            None => provenance.is_empty(),
        };
        if unchanged {
            return Ok(());
        }
        self.record_config_fact(record)
    }

    /// ROUT-L RL-1b (model §3.4): the host's procedure manifest is the composition's: one
    /// binding per composed procedure, the Runtime's package operation, no yield points, and its
    /// package's place as the binding's location.
    fn install_composed_manifest(
        &self,
        record: &CompositionRecord,
        root: Option<&Path>,
    ) -> RuntimeHostResult<()> {
        let manifest = composed_manifest(
            record.procedures.iter().map(|procedure| {
                (
                    procedure.procedure_ref.as_str(),
                    procedure.game_id.as_str(),
                    procedure.directory.as_str(),
                )
            }),
            root,
        )?;
        *lock(
            &self.procedure_manifest,
            HostLocation::ReadProcedureManifest,
        )? = Some(manifest);
        Ok(())
    }

    /// [`RuntimeHost::standard_pack_request`].
    pub(super) fn standard_pack_request(
        &self,
        procedure_ref: &str,
    ) -> RuntimeHostResult<Option<ContainedTaskRequest>> {
        let Some(workbench) = self.standard_packs.schedule.as_ref() else {
            return Ok(None);
        };
        let Some(procedure) = lock(&workbench.state, HostLocation::StandardPackRead)?
            .procedures
            .get(procedure_ref)
            .cloned()
        else {
            return Ok(None);
        };
        let Some(root) = self.standard_packs.root() else {
            return Ok(None);
        };
        // The registry as it is now: a package replaced or moved in place is found by the next
        // run. Without a usable registry the composed place is named, and the run's preparation
        // refuses a missing package with that package's own code.
        let path = match self.peek_game_registry(&procedure.game_id) {
            Ok(registry) => registry.package_path(&procedure.package_id).ok(),
            Err(_) => None,
        }
        .unwrap_or_else(|| root.join(&procedure.game_id).join(&procedure.directory));
        ContainedTaskRequest::new(path.to_string_lossy().into_owned())
            .and_then(|request| {
                request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
            })
            .map(Some)
            .map_err(|_| {
                RuntimeHostError::request(
                    HostCode::StandardPackRootUnreadable,
                    HostLocation::StandardPackRead,
                    RuntimeErrorCode::InvalidRequest,
                )
            })
    }

    /// ROUT-L RL-1b, acceptance A4: how many times the workbench opened `game`'s
    /// `schedule.json`.
    #[cfg(test)]
    pub(super) fn standard_pack_schedule_reads(&self, game: &str) -> RuntimeHostResult<u64> {
        Ok(match self.standard_packs.schedule.as_ref() {
            Some(workbench) => lock(&workbench.state, HostLocation::StandardPackRead)?
                .schedule_reads
                .get(game)
                .copied()
                .unwrap_or(0),
            None => 0,
        })
    }
}

/// ROUT-L RL-1b (model §3.4): the procedure bindings of a composition, each `(procedure_ref,
/// game, directory)`: the Runtime's package operation, no yield points, and its package's place
/// below `root` as the binding's location. It may bind no procedure.
fn composed_manifest<'a>(
    procedures: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
    root: Option<&Path>,
) -> RuntimeHostResult<ProcedureManifest> {
    let bindings = procedures
        .into_iter()
        .map(|(procedure_ref, game_id, directory)| {
            crate::ProcedureBinding::new(procedure_ref, STANDARD_PACK_OPERATION_ID, Vec::new()).map(
                |binding| match root {
                    Some(root) => binding.with_package_path(
                        root.join(game_id)
                            .join(directory)
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    None => binding,
                },
            )
        })
        .collect::<RuntimeHostResult<Vec<_>>>()?;
    ProcedureManifest::composed(bindings)
}

/// ROUT-L RL-1c (#411; board 6432236193, 6435964241): what a composer-mode configuration
/// schedules, for the readers outside the running Runtime: the composition the scheduler workbench
/// makes for the configured instances from the game roots below `standard_packs.root`, with the
/// same composer and the same games in use, and the configuration with the composition's
/// procedure bindings installed, as the workbench installs them. check-config computes over it
/// what catalog mode computes over `policy.catalog` (the missing fact readers, the remedy
/// declarations and the task package admission); `actingd suspended` reads its report against
/// it. Read only; nothing is recorded.
pub struct ConfiguredStandardPackSchedule {
    composition: Option<StandardPackComposition>,
    host: crate::RuntimeHostConfig,
    problems: Vec<Outcome>,
    warnings: Vec<String>,
}

impl ConfiguredStandardPackSchedule {
    /// RL-1c fix 1 (#411 M1): each of [`Self::problems`] as `<code>:<game>:sha256:<hex>` (the
    /// game's `schedule.json` as read, `unread` when none was read): why a game has no rows, or
    /// why its paused pairs are `task_not_in_catalog`, in `actingd suspended`'s warnings.
    pub fn problem_warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The composed catalog; `None` when the composition as a whole is refused (its problems
    /// say why).
    pub fn compiled(&self) -> Option<&CompiledCatalog> {
        self.composition
            .as_ref()
            .map(StandardPackComposition::compiled)
    }

    /// The configuration with the composition's procedure bindings (none without a
    /// composition): the bindings check-config admits and the report reads.
    pub fn host(&self) -> &crate::RuntimeHostConfig {
        &self.host
    }

    /// Each refusal met while composing, in order: a game root that cannot be read
    /// (`standard_pack_root_unreadable`) or is broken, and each refused `schedule.json`
    /// (`standard_pack_root_invalid`). The game is left out and the others are composed, as the
    /// workbench composes them; offline there is no last good file to keep.
    pub fn problems(&self) -> &[Outcome] {
        &self.problems
    }
}

/// ROUT-L RL-1c: [`ConfiguredStandardPackSchedule`] of `config`: its policy inputs' instances,
/// its `standard_packs.root` and its configuration manifest's subsystems, as the workbench of a
/// Runtime started from it reads them. A refused game is left out and each round of the
/// composition takes out at least one, so it ends; a refusal of the composition as a whole leaves
/// no composition. Only a binding the composition cannot make fails.
pub fn compose_configured_standard_packs(
    config: &crate::RuntimeHostConfig,
) -> RuntimeHostResult<ConfiguredStandardPackSchedule> {
    let identities = config
        .policy_inputs()
        .map(|inputs| {
            inputs
                .instance_identities()
                .map(|identity| {
                    (
                        identity.instance_id.to_owned(),
                        identity.game_id.to_owned(),
                        identity.server_id.to_owned(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let bound = games_in_use(&identities);
    let subsystems = enabled_subsystems(config.config_manifest.as_ref());
    let root = config.standard_packs_root();
    let mut problems = Vec::new();
    let mut read = BTreeMap::new();
    if let Some(root) = root {
        let games = bound
            .values()
            .map(|binding| binding.game_id.clone())
            .collect::<BTreeSet<_>>();
        let mut reads = BTreeMap::new();
        for game in &games {
            match read_schedule_game(&root.join(game), game, &mut reads) {
                Ok(Some(files)) => {
                    read.insert(game.clone(), files);
                }
                Ok(None) => {}
                Err(failure) => problems.push(failure.outcome(Some(game.as_str()))),
            }
        }
    }
    let mut schedules = read
        .iter()
        .filter_map(|(game, files)| {
            files
                .schedule
                .as_ref()
                .map(|bytes| (game.clone(), bytes.as_slice()))
        })
        .collect::<BTreeMap<_, _>>();
    let composition = loop {
        // G-R2s: with each game's package declarations, read as the workbench reads them.
        let inputs = composer_inputs(&read, |game| schedules.get(game).copied());
        let failure = match compose_standard_pack_rules(&StandardPackComposeInput {
            instances: &bound,
            games: &inputs,
            subsystems: subsystems.as_ref(),
            catalog_version: 1,
        }) {
            Ok(composition) => break Some(composition),
            Err(failure) => failure,
        };
        let mut refused = BTreeSet::new();
        for issue in failure.issues() {
            // A refusal of the composition as a whole is each composed game's problem: every
            // `standard_pack_root_invalid` names its game and file.
            let games = match &issue.game_id {
                Some(game) => vec![game.clone()],
                None => schedules.keys().cloned().collect(),
            };
            if let Some(root) = root {
                for game in &games {
                    problems.push(
                        RootFailure::Invalid {
                            path: root.join(game).join(STANDARD_PACK_SCHEDULE_FILE),
                            issue: issue.issue.clone(),
                        }
                        .outcome(Some(game.as_str())),
                    );
                }
            }
            if let Some(game) = &issue.game_id {
                refused.insert(game.clone());
            }
        }
        let before = schedules.len();
        schedules.retain(|game, _| !refused.contains(game));
        if schedules.len() == before {
            // A refusal of the composition as a whole, or one naming no game this round
            // composed: no composition.
            break None;
        }
    };
    let manifest = composed_manifest(
        composition
            .iter()
            .flat_map(StandardPackComposition::manifest)
            .map(|procedure| {
                (
                    procedure.procedure_ref.as_str(),
                    procedure.game_id.as_str(),
                    procedure.directory.as_str(),
                )
            }),
        root,
    )?;
    // RL-1c fix 1 (#411 M1): each problem as an offline reader's warning: its code, its game and
    // the sha256 of the `schedule.json` read for that game (`unread` when none was read).
    let warnings = problems
        .iter()
        .map(|problem| {
            let game = problem.value("game").and_then(serde_json::Value::as_str);
            let digest = game
                .and_then(|game| read.get(game))
                .and_then(|files| files.schedule.as_deref())
                .map_or_else(|| "unread".to_owned(), sha256);
            format!("{}:{}:{digest}", problem.code(), game.unwrap_or("-"))
        })
        .collect();
    Ok(ConfiguredStandardPackSchedule {
        host: config.clone().with_procedure_manifest(manifest),
        composition,
        problems,
        warnings,
    })
}

/// ROUT-L RL-1b (model §6.4 step 1, §7.2): what check-config shows of the composition the
/// workbench would make: the games in use with their rules and classification, the composed
/// counts, and each problem. Read only; nothing is recorded.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackSchedulePreview {
    /// Whether the configuration names no catalog, so the workbench composes the rules.
    pub composer_mode: bool,
    pub games: Vec<actingcommand_policy::ComposedGame>,
    pub rules: usize,
    pub pools: usize,
    pub profiles: usize,
    pub events: usize,
    pub unclassified: usize,
    /// One `standard_pack_root_invalid` per refusal of a `schedule.json`; Workflow #455 G-R2s fix
    /// round 1: and one `standard_pack_package_unreadable` per package of a composed game whose
    /// declaration could not be read (every task of that game carries the irreversible mark, so
    /// no goal weighs it), with the reader's refusal as its cause, by game, then package id.
    pub problems: Vec<Outcome>,
    /// The games in use whose game root is broken (listed with their problem in check-config's
    /// `standard_packs`), left out of the preview.
    pub left_out: Vec<String>,
}

/// ROUT-L RL-1b: check-config's preview of the composition for the bound instances
/// `(alias, game, server)` below `root`, with the `subsystems` the configuration names.
pub fn preview_standard_pack_schedule(
    root: Option<&Path>,
    instances: &[(String, String, String)],
    subsystems: Option<&BTreeSet<String>>,
    composer_mode: bool,
) -> StandardPackSchedulePreview {
    let mut preview = StandardPackSchedulePreview {
        composer_mode,
        games: Vec::new(),
        rules: 0,
        pools: 0,
        profiles: 0,
        events: 0,
        unclassified: 0,
        problems: Vec::new(),
        left_out: Vec::new(),
    };
    let Some(root) = root else {
        return preview;
    };
    let bound = games_in_use(instances);
    let games = bound
        .values()
        .map(|binding| binding.game_id.clone())
        .collect::<BTreeSet<_>>();
    let mut reads = BTreeMap::new();
    let mut read = BTreeMap::new();
    for game in &games {
        match read_schedule_game(&root.join(game), game, &mut reads) {
            Ok(Some(files)) => {
                read.insert(game.clone(), files);
            }
            Ok(None) => {}
            // A broken game root is check-config's `standard_packs` problem (and its note)
            // already; here the game is left out, as the workbench keeps its rules.
            Err(_) => preview.left_out.push(game.clone()),
        }
    }
    // G-R2s: with each game's package declarations, read as the workbench reads them.
    let inputs = composer_inputs(&read, |game| {
        read.get(game).and_then(|files| files.schedule.as_deref())
    });
    match compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &bound,
        games: &inputs,
        subsystems,
        catalog_version: 1,
    }) {
        Ok(composition) => {
            let catalog = composition.compiled().catalog();
            preview.rules = catalog.tasks.tasks.len();
            preview.pools = catalog.pools.pools.len();
            preview.profiles = catalog.activity.profiles.len();
            preview.events = catalog.timeline.events.len();
            preview.unclassified = composition
                .games()
                .iter()
                .map(|game| game.unclassified.len())
                .sum();
            preview.games = composition.games().to_vec();
            // G-R2s fix round 1: a package of a composed game whose declaration could not be
            // read marks every task of its game, so no goal weighs any of them; check-config
            // shows each such package, with the reader's refusal as its cause.
            for (game, packages) in composition.unreadable_packages() {
                for package_id in packages {
                    preview
                        .problems
                        .push(unreadable_package(game, package_id, read.get(game)));
                }
            }
        }
        Err(failure) => {
            for issue in failure.issues() {
                let game = issue.game_id.as_deref();
                let path = match game {
                    Some(game) => root.join(game).join(STANDARD_PACK_SCHEDULE_FILE),
                    None => root.to_owned(),
                };
                preview.problems.push(
                    RootFailure::Invalid {
                        path,
                        issue: issue.issue.clone(),
                    }
                    .outcome(game),
                );
            }
        }
    }
    preview
}
