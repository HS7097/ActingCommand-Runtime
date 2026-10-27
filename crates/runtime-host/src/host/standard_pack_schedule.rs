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
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_contract::{
    BundleIndex, BundleMaintenance, STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE,
    STANDARD_PACK_SCHEDULE_FILE, STANDARD_PACK_TABLES_DIRECTORY, StandardPackRoot,
    StandardPackRootBytes, read_standard_pack_root,
};
use actingcommand_policy::{
    CompiledCatalog, STANDARD_PACK_OPERATION_ID, STANDARD_PACKS_CATALOG_ID, StandardPackBinding,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    StandardPackRuleIssue, compose_standard_pack_rules,
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

/// The marks of a game root's `schedule.json` and `bundle.json`.
type GameMarks = (FileMark, FileMark);

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
/// bundle's maintenance entries, and `schedule.json` when it holds one (counted in `reads`).
/// `None` when the directory holds no game root.
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
    Ok(Some(ScheduleGame {
        root,
        maintenance,
        schedule,
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
            state.adopted = true;
            // The active generation is not the one the workbench last activated (a start, or a
            // switch by another writer): adopt its record when it is a composed one, and read
            // every game in use again.
            state.active = match &active {
                Some(generation) if generation.catalog_id() == STANDARD_PACKS_CATALOG_ID => {
                    let record = lock(self.policy()?, HostLocation::StandardPackRead)?
                        .composition_record(generation.catalog_hash())?;
                    record
                        .and_then(|bytes| serde_json::from_slice::<CompositionRecord>(&bytes).ok())
                        .filter(|record| record.schema_version == COMPOSITION_RECORD_SCHEMA)
                }
                _ => None,
            };
            state.active_hash = active_hash.clone();
            state.marks.clear();
            // Until a composition succeeds the cycle runs the active generation's own
            // procedures (none without a composed one): a broken root never leaves the cycle
            // without a manifest.
            let record = state.active.clone().unwrap_or_else(|| CompositionRecord {
                schema_version: COMPOSITION_RECORD_SCHEMA.to_owned(),
                rules_digest: String::new(),
                games: BTreeMap::new(),
                procedures: Vec::new(),
                pools: Vec::new(),
            });
            self.install_composed_manifest(&record, root.as_deref())?;
            state.procedures = record
                .procedures
                .iter()
                .map(|procedure| (procedure.procedure_ref.clone(), procedure.clone()))
                .collect();
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
                    Ok(game_marks) => {
                        marks.insert(game.clone(), game_marks);
                    }
                    Err(failure) => {
                        self.report_once(
                            state,
                            game,
                            "root:metadata",
                            failure.outcome(Some(game.as_str())),
                        )?;
                        return Ok(());
                    }
                }
            }
        }
        if state.active.is_some() && state.bound == bound && state.marks == marks {
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
            let inputs = read
                .iter()
                .filter_map(|(game, files)| {
                    schedules.get(game).map(|schedule| {
                        (
                            game.clone(),
                            StandardPackGameFiles {
                                schedule: schedule.as_slice(),
                                root: &files.root,
                                maintenance: files.maintenance.as_slice(),
                            },
                        )
                    })
                })
                .collect::<BTreeMap<_, _>>();
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
                let count = read
                    .get(&game.game_id)
                    .map_or(0, |files| files.root.registry.len());
                self.record_standard_pack_read(
                    Outcome::new(HostCode::StandardPackRegistered)
                        .operation(HostLocation::StandardPackRegister)
                        .game(game.game_id.as_str())
                        .count(count)
                        .schedule_sha256(sha.as_str())
                        .rules(game.tasks)
                        .held(game.held.len())
                        .unclassified(game.unclassified.len()),
                )?;
            }
            state.active_hash = Some(generation.catalog_hash().to_owned());
            state.active = Some(record.clone());
        }
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
        state.marks = marks;
        Ok(())
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
        let inputs = read
            .iter()
            .filter_map(|(game, files)| {
                schedules.get(game).map(|schedule| {
                    (
                        game.clone(),
                        StandardPackGameFiles {
                            schedule,
                            root: &files.root,
                            maintenance: files.maintenance.as_slice(),
                        },
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
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
    /// One `standard_pack_root_invalid` per refusal of a `schedule.json`.
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
    let inputs = read
        .iter()
        .filter_map(|(game, files)| {
            files.schedule.as_ref().map(|schedule| {
                (
                    game.clone(),
                    StandardPackGameFiles {
                        schedule: schedule.as_slice(),
                        root: &files.root,
                        maintenance: files.maintenance.as_slice(),
                    },
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
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
