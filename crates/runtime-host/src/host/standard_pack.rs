// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-6 (model M-373 v1.1 §3, §4.3, §6, §7, §8): the standard-pack root reader,
//! its registry, and the shared interrupt pages of a run.
//!
//! - **The registry** (§4.3; Workflow #373 NAV-3a). A game root's `bundle.json` registers its
//!   packages (package id → directory) and, per server, its return-home package. A run that
//!   resolves a prerequisite package or falls back to the return-home package reads the root of
//!   its package's game when it uses it ([`GameRegistry`]); a package installed or replaced in
//!   place is found by the next run, without a restart and without any configuration entry.
//!
//! - **The root.** `standard_packs.root` (actingd configuration, set once by the installer)
//!   holds one directory per game, `<root>/<game>/`, with the root files `bundle.json`,
//!   `pages.json` and `interrupts.json` beside the package directories. Nothing here names a
//!   game or a page: ids are opaque strings, compared as equal or not.
//! - **When it is read** (§7). At startup every game root is read (root files only, no package
//!   admitted) and recorded: `standard_pack_registered`, or `standard_pack_root_invalid` /
//!   `standard_pack_root_unreadable`; a broken root never stops the Runtime. Each run's
//!   preparation reads its own game's root when that root holds `interrupts.json` (the one
//!   consumer NAV-6 brings), refuses the run with the root's code when it is broken, records a
//!   registry that changed since it was last read, and keeps what it read for the whole run.
//!   Without `interrupts.json` nothing is read and the run is what it was before NAV-6.
//! - **Shared interrupt pages** (§6). The kernel offers each frame that did not give the running
//!   package what it awaited ([`SharedInterruptFrame`]). The run's feature package (admitted on
//!   first use) evaluates the interrupt pages the running package does not declare itself, in
//!   one scene evaluation. One match is handled: its handler (admitted on first use) runs as a
//!   layer of the run (`role: interrupt_handler`), then its `then.run` package, and the run
//!   resumes, or ends with `contained_task_interrupt_escalated` (`then: end_run`). Two or more
//!   matches are recorded and none is handled. A page handled `max_per_run` times, or a run that
//!   handled 16, ends the run with `contained_task_interrupt_limit_reached`. A handler never
//!   looks at the interrupt pages itself.

use super::contained_task::{EntryRecoveryRuntime, PackageIdentity, RuntimeContainedTask};
use super::*;
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_contract::{
    MAX_INTERRUPT_HANDLINGS_PER_RUN, NavigationProblem, NavigationRole, RegistryProblem,
    STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_PAGES_FILE,
    StandardPackInterrupts, StandardPackPages, StandardPackRegistry, StandardPackRootIssue,
    TaskEntryRecoveryRole, TaskInterruptThen,
};
use actingcommand_execution_kernel::{
    MAX_GATED_STEPS, SharedInterruptFrame, SharedInterruptVerdict, canonical_page_anchor,
};
use serde::Serialize;
use std::io;

/// What a game's registry held when it was last read: its (package id, server) set and
/// (Workflow #373 NAV-3a) its return-home package by server.
type Registered = (BTreeSet<(String, String)>, BTreeMap<String, String>);

/// The standard-pack root of the host and, per game, what its registry held when it was last
/// read (§4.3; memory only).
pub(super) struct StandardPackRoots {
    root: Option<PathBuf>,
    registered: Mutex<BTreeMap<String, Registered>>,
}

impl StandardPackRoots {
    pub(super) fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            registered: Mutex::new(BTreeMap::new()),
        }
    }

    /// `standard_packs.root`, when set.
    pub(super) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
}

/// Workflow #373 NAV-3a (M-373 §4.3): the registry of one game, as a run reads it when it
/// resolves a prerequisite package or a return-home package.
pub(super) enum GameRegistry {
    /// No `standard_packs.root` is set: nothing is registered.
    RootUnset,
    /// The root holds no root of the game (no directory, or none of the root files).
    GameRootMissing,
    Read {
        directory: PathBuf,
        registry: StandardPackRegistry,
    },
}

impl GameRegistry {
    /// Where the registry package `package_id` is, or why it is not bound (§4.3, the values of
    /// `contained_task_prerequisite_unbound`).
    pub(super) fn package_path(&self, package_id: &str) -> Result<PathBuf, RegistryProblem> {
        match self {
            Self::RootUnset => Err(RegistryProblem::RootUnset),
            Self::GameRootMissing => Err(RegistryProblem::GameRootMissing),
            Self::Read {
                directory,
                registry,
            } => registry
                .get(package_id)
                .map(|entry| directory.join(&entry.directory))
                .ok_or(RegistryProblem::NotListed),
        }
    }

    /// The return-home package of `server` (its `maintenance[]` use `return_home`), when the
    /// game has a registry that names one.
    pub(super) fn return_home_package(&self, server: &str) -> Option<&str> {
        match self {
            Self::Read { registry, .. } => registry.return_home_package(server),
            Self::RootUnset | Self::GameRootMissing => None,
        }
    }

    /// The registry's package ids; none without a registry.
    pub(super) fn package_ids(&self) -> BTreeSet<String> {
        match self {
            Self::Read { registry, .. } => registry.package_ids().map(str::to_owned).collect(),
            Self::RootUnset | Self::GameRootMissing => BTreeSet::new(),
        }
    }
}

/// Workflow #373 NAV-3a (M-373 §4.3, §8.5): every game root's registry below `root`, read as
/// the suspension lift checks read the packages of a failure identity: root files only, nothing
/// recorded. A root that cannot be listed, and a game root that is broken, register nothing, so
/// a layer they held is measured as missing.
pub(crate) fn read_registries(root: Option<&Path>) -> Vec<(String, PathBuf, StandardPackRegistry)> {
    let Some(root) = root else {
        return Vec::new();
    };
    let Ok(games) = game_directories(root) else {
        return Vec::new();
    };
    games
        .into_iter()
        .filter_map(
            |(game, directory)| match read_game_root(&directory, &game) {
                Ok(Some(read)) => Some((game, read.directory, read.registry)),
                Ok(None) | Err(_) => None,
            },
        )
        .collect()
}

/// One game root as read: where it is, its registry and, when present, its page list and its
/// shared interrupt pages.
struct GameRoot {
    directory: PathBuf,
    registry: StandardPackRegistry,
    pages: Option<StandardPackPages>,
    interrupts: Option<StandardPackInterrupts>,
}

/// Why a game root (or the root itself) cannot be used.
enum RootFailure {
    Invalid {
        path: PathBuf,
        issue: StandardPackRootIssue,
    },
    Unreadable {
        path: PathBuf,
        error: io::Error,
        op: IoOp,
    },
}

impl RootFailure {
    fn code(&self) -> Code {
        match self {
            Self::Invalid { .. } => SharedCode::StandardPackRootInvalid.code(),
            Self::Unreadable { .. } => HostCode::StandardPackRootUnreadable.code(),
        }
    }

    /// The failure's values on the outcome `base` of its code (§9.1).
    fn values(&self, game: Option<&str>, base: Outcome) -> Outcome {
        let base = match game {
            Some(game) => base.game(game),
            None => base,
        };
        match self {
            Self::Invalid { path, issue } => base
                .path(path)
                .standard_pack_problem(issue.problem)
                .json_pointer(issue.pointer.clone()),
            Self::Unreadable { path, error, op } => base
                .path(path)
                .caused_by(Outcome::from_io_error(error, *op)),
        }
    }

    fn outcome(&self, game: Option<&str>) -> Outcome {
        self.values(
            game,
            Outcome::new(self.code()).operation(HostLocation::StandardPackRead),
        )
    }

    /// The refusal of a run whose game root is broken: in preparation, before any lease and any
    /// task record (§7).
    fn refusal(&self, game: &str) -> RequestFailure {
        let error = RuntimeHostError::request(
            self.code(),
            HostLocation::StandardPackRead,
            RuntimeErrorCode::PackageInvalid,
        )
        .with_outcome_values(|outcome| self.values(Some(game), outcome));
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}

/// A root file's bytes; `None` when the file does not exist.
fn read_root_file(path: &Path) -> Result<Option<Vec<u8>>, RootFailure> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RootFailure::Unreadable {
            path: path.to_owned(),
            error,
            op: IoOp::Read,
        }),
    }
}

/// Reads one game root (§3, §7): `None` when the directory holds none of the root files.
/// `pages.json` or `interrupts.json` without `bundle.json`, and `interrupts.json` without
/// `pages.json`, are refused; every file is checked as the contract crate checks it, and the
/// packages the page list and the interrupts name must be packages of the registry.
fn read_game_root(directory: &Path, game: &str) -> Result<Option<GameRoot>, RootFailure> {
    let path_of = |file: &str| directory.join(file);
    let invalid = |file: &'static str, issue: StandardPackRootIssue| RootFailure::Invalid {
        path: path_of(file),
        issue,
    };
    let bundle = read_root_file(&path_of(STANDARD_PACK_BUNDLE_FILE))?;
    let pages = read_root_file(&path_of(STANDARD_PACK_PAGES_FILE))?;
    let interrupts = read_root_file(&path_of(STANDARD_PACK_INTERRUPTS_FILE))?;
    let Some(bundle) = bundle else {
        if pages.is_none() && interrupts.is_none() {
            return Ok(None);
        }
        return Err(invalid(
            STANDARD_PACK_BUNDLE_FILE,
            StandardPackRootIssue {
                file: STANDARD_PACK_BUNDLE_FILE,
                problem: actingcommand_contract::StandardPackProblem::BundleMissing,
                pointer: String::new(),
            },
        ));
    };
    let registry = StandardPackRegistry::from_bundle_bytes(&bundle, game)
        .map_err(|issue| invalid(STANDARD_PACK_BUNDLE_FILE, issue))?;
    let pages = pages
        .map(|bytes| actingcommand_contract::parse_standard_pack_pages(&bytes, Some(game)))
        .transpose()
        .map_err(|issue| invalid(STANDARD_PACK_PAGES_FILE, issue))?;
    let interrupts = match (interrupts, &pages) {
        (None, _) => None,
        (Some(_), None) => {
            return Err(invalid(
                STANDARD_PACK_INTERRUPTS_FILE,
                StandardPackRootIssue {
                    file: STANDARD_PACK_INTERRUPTS_FILE,
                    problem: actingcommand_contract::StandardPackProblem::PagesMissing,
                    pointer: String::new(),
                },
            ));
        }
        (Some(bytes), Some(pages)) => Some(
            actingcommand_contract::parse_standard_pack_interrupts(&bytes, Some(game), pages)
                .map_err(|issue| invalid(STANDARD_PACK_INTERRUPTS_FILE, issue))?,
        ),
    };
    if let Some(pages) = &pages {
        actingcommand_contract::validate_standard_pack_references(
            &registry,
            pages,
            interrupts.as_ref(),
        )
        .map_err(|issue| invalid(issue.file, issue))?;
    }
    Ok(Some(GameRoot {
        directory: directory.to_owned(),
        registry,
        pages,
        interrupts,
    }))
}

/// The game directories below the root, by name.
fn game_directories(root: &Path) -> Result<Vec<(String, PathBuf)>, RootFailure> {
    let unreadable = |error, op| RootFailure::Unreadable {
        path: root.to_owned(),
        error,
        op,
    };
    let mut games = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| unreadable(error, IoOp::ReadDir))? {
        let entry = entry.map_err(|error| unreadable(error, IoOp::ReadDir))?;
        let kind = entry
            .file_type()
            .map_err(|error| unreadable(error, IoOp::Metadata))?;
        let name = entry.file_name();
        if kind.is_dir()
            && let Some(name) = name.to_str()
        {
            games.push((name.to_owned(), entry.path()));
        }
    }
    games.sort();
    Ok(games)
}

/// Workflow #373 NAV-6 (§7): what check-config reports of the standard-pack root, read as
/// startup reads it (root files only; no package is admitted). A broken game root is listed with
/// its problem; it is no failure of the configuration.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPacksReport {
    /// The configured root; `None` when `standard_packs.root` is not set.
    pub root: Option<PathBuf>,
    /// When the root itself cannot be listed, why.
    pub root_problem: Option<Outcome>,
    pub games: Vec<StandardPackGameReport>,
}

/// One game root in [`StandardPacksReport`].
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackGameReport {
    pub game: String,
    /// The bundle index version, 2 or 3; `None` when the root is broken.
    pub bundle_version: Option<u8>,
    /// The registry's package count; `None` when the root is broken.
    pub pack_count: Option<usize>,
    /// The root files present (`bundle.json`, `pages.json`, `interrupts.json`).
    pub root_files: Vec<&'static str>,
    /// `standard_pack_root_invalid` or `standard_pack_root_unreadable` when the root is broken.
    pub problem: Option<Outcome>,
}

impl StandardPacksReport {
    /// The games whose root is broken, as check-config lists its warnings.
    pub fn problems(&self) -> impl Iterator<Item = (&str, &Outcome)> {
        self.games.iter().filter_map(|game| {
            game.problem
                .as_ref()
                .map(|problem| (game.game.as_str(), problem))
        })
    }
}

/// Workflow #373 NAV-6: reads every game root below `root` as startup does, for check-config.
pub fn inspect_standard_packs(root: Option<&Path>) -> StandardPacksReport {
    let Some(root) = root else {
        return StandardPacksReport {
            root: None,
            root_problem: None,
            games: Vec::new(),
        };
    };
    let games = match game_directories(root) {
        Ok(games) => games,
        Err(failure) => {
            return StandardPacksReport {
                root: Some(root.to_owned()),
                root_problem: Some(failure.outcome(None)),
                games: Vec::new(),
            };
        }
    };
    let games = games
        .into_iter()
        .filter_map(|(game, directory)| {
            let root_files = [
                STANDARD_PACK_BUNDLE_FILE,
                STANDARD_PACK_PAGES_FILE,
                STANDARD_PACK_INTERRUPTS_FILE,
            ]
            .into_iter()
            .filter(|file| directory.join(file).is_file())
            .collect::<Vec<_>>();
            match read_game_root(&directory, &game) {
                Ok(None) => None,
                Ok(Some(read)) => Some(StandardPackGameReport {
                    game,
                    bundle_version: Some(read.registry.bundle_version()),
                    pack_count: Some(read.registry.len()),
                    root_files,
                    problem: None,
                }),
                Err(failure) => Some(StandardPackGameReport {
                    problem: Some(failure.outcome(Some(&game))),
                    game,
                    bundle_version: None,
                    pack_count: None,
                    root_files,
                }),
            }
        })
        .collect();
    StandardPacksReport {
        root: Some(root.to_owned()),
        root_problem: None,
        games,
    }
}

/// A package of the registry, where the run finds it.
#[derive(Debug, Clone)]
struct RegisteredPack {
    package_id: String,
    path: PathBuf,
}

impl RegisteredPack {
    fn of(root: &GameRoot, package_id: &str) -> Option<Self> {
        root.registry.get(package_id).map(|entry| Self {
            package_id: package_id.to_owned(),
            path: root.directory.join(&entry.directory),
        })
    }
}

/// Where the run goes after one interrupt's handler, for the run's server.
#[derive(Debug, Clone)]
enum RunThen {
    Resume,
    EndRun,
    /// The list's package of the run's server (`None` when it names none) and the first
    /// package it names.
    Run {
        pack: Option<RegisteredPack>,
        listed: String,
    },
}

/// One interrupt that applies to the run: its handler is a package of the run's server.
#[derive(Debug, Clone)]
struct RunInterrupt {
    page: String,
    handler: RegisteredPack,
    then: RunThen,
    max_per_run: u8,
}

/// Workflow #373 NAV-6: a run's shared interrupt pages, fixed at its preparation (§7), and what
/// the run has done with them.
pub(super) struct RunInterrupts {
    /// The run's package: a feature, handler or `then.run` package must have its game, server
    /// and resolution.
    identity: PackageIdentity,
    features: RegisteredPack,
    /// The ids of `pages.json`, which the feature package's pages must be exactly.
    pages: BTreeSet<String>,
    entries: Vec<RunInterrupt>,
    admitted_features: Option<PreparedContainedTask>,
    admitted: BTreeMap<String, PreparedContainedTask>,
    counts: BTreeMap<String, u8>,
    total: u32,
    /// Layer packages whose effective configuration this run recorded (once each, §8.3).
    configured: BTreeSet<String>,
}

impl HostShared {
    /// Workflow #373 NAV-6 (§7): reads every game root once at startup and records each:
    /// `standard_pack_registered`, or the root's failure. A broken root never stops the Runtime;
    /// only a ledger failure does.
    pub(super) fn register_standard_packs_on_start(&self) -> RuntimeHostResult<()> {
        let Some(root) = self.standard_packs.root.clone() else {
            return Ok(());
        };
        let games = match game_directories(&root) {
            Ok(games) => games,
            Err(failure) => return self.record_standard_pack_read(failure.outcome(None)),
        };
        for (game, directory) in games {
            match read_game_root(&directory, &game) {
                Ok(None) => {}
                Ok(Some(read)) => self.note_standard_pack_registry(&game, &read.registry)?,
                Err(failure) => self.record_standard_pack_read(failure.outcome(Some(&game)))?,
            }
        }
        Ok(())
    }

    /// Records `standard_pack_registered` when `registry` differs from the one last read for
    /// `game` (the first read included), its return-home packages included (Workflow #373
    /// NAV-3a), and remembers it (§4.3).
    fn note_standard_pack_registry(
        &self,
        game: &str,
        registry: &StandardPackRegistry,
    ) -> RuntimeHostResult<()> {
        let listed = registry.listed();
        let current = (listed.clone(), registry.return_home_packages().clone());
        let (previous, _) = {
            let mut registered = lock(
                &self.standard_packs.registered,
                HostLocation::StandardPackRegister,
            )?;
            if registered.get(game) == Some(&current) {
                return Ok(());
            }
            registered.insert(game.to_owned(), current)
        }
        .unwrap_or_default();
        let ids = |set: &BTreeSet<(String, String)>| {
            set.iter()
                .map(|(package_id, _)| package_id.clone())
                .collect::<BTreeSet<_>>()
        };
        let (now, before) = (ids(&listed), ids(&previous));
        let outcome = Outcome::new(HostCode::StandardPackRegistered)
            .operation(HostLocation::StandardPackRegister)
            .game(game)
            .count(registry.len());
        let added = now.difference(&before).cloned().collect::<Vec<_>>();
        let removed = before.difference(&now).cloned().collect::<Vec<_>>();
        let outcome = if added.is_empty() {
            outcome
        } else {
            outcome.added_package_ids(added)
        };
        let outcome = if removed.is_empty() {
            outcome
        } else {
            outcome.removed_package_ids(removed)
        };
        self.record_standard_pack_read(outcome)
    }

    /// A `runtime.lifecycle_observed` `standard_pack_read` record carrying `outcome` (§8.4).
    pub(super) fn record_standard_pack_read(&self, outcome: Outcome) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.events.system_links()?,
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::StandardPackRead { outcome },
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// Workflow #373 NAV-3a (M-373 §4.3, §7): the registry of `game`, read now, when a run
    /// resolves a prerequisite package or a return-home package with it; a registry that changed
    /// since it was last read is recorded (`standard_pack_registered`). A broken game root
    /// refuses the run with the root's code, as a preparation refusal (no task record).
    pub(super) fn read_game_registry(&self, game: &str) -> Result<GameRegistry, RequestFailure> {
        let registry = self.peek_game_registry(game)?;
        if let GameRegistry::Read { registry, .. } = &registry {
            self.note_standard_pack_registry(game, registry)
                .map_err(RequestFailure::poison_without_terminal)?;
        }
        Ok(registry)
    }

    /// [`Self::read_game_registry`] without the record of a changed registry, for a replay that
    /// checks the layers its run recorded.
    pub(super) fn peek_game_registry(&self, game: &str) -> Result<GameRegistry, RequestFailure> {
        let Some(root) = &self.standard_packs.root else {
            return Ok(GameRegistry::RootUnset);
        };
        match read_game_root(&root.join(game), game) {
            Ok(None) => Ok(GameRegistry::GameRootMissing),
            Ok(Some(read)) => Ok(GameRegistry::Read {
                directory: read.directory,
                registry: read.registry,
            }),
            Err(failure) => Err(failure.refusal(game)),
        }
    }

    /// Workflow #373 NAV-6 (§7): the shared interrupt pages of a run of `prepared`, read in its
    /// preparation. `None` (nothing read, nothing recorded) without a root, without the game's
    /// directory, or when the game root holds no `interrupts.json`; also when the root names no
    /// feature package or no handler for the run's server. A broken root refuses the run.
    pub(super) fn prepare_run_interrupts(
        &self,
        prepared: &PreparedContainedTask,
    ) -> Result<Option<Box<RunInterrupts>>, RequestFailure> {
        let Some(root) = &self.standard_packs.root else {
            return Ok(None);
        };
        let game = prepared.game();
        let directory = root.join(game);
        let marker = directory.join(STANDARD_PACK_INTERRUPTS_FILE);
        match fs::symlink_metadata(&marker) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RootFailure::Unreadable {
                    path: marker,
                    error,
                    op: IoOp::Metadata,
                }
                .refusal(game));
            }
        }
        let read = match read_game_root(&directory, game) {
            Ok(Some(read)) => read,
            Ok(None) => return Ok(None),
            Err(failure) => return Err(failure.refusal(game)),
        };
        self.note_standard_pack_registry(game, &read.registry)
            .map_err(RequestFailure::poison_without_terminal)?;
        let (Some(pages), Some(interrupts)) = (&read.pages, &read.interrupts) else {
            return Ok(None);
        };
        let server = prepared.server();
        let for_server = |packages: &[String]| {
            read.registry
                .for_server(packages, server)
                .and_then(|package_id| RegisteredPack::of(&read, package_id))
        };
        let Some(features) = for_server(&pages.features) else {
            return Ok(None);
        };
        let entries = interrupts
            .interrupts
            .iter()
            .filter_map(|interrupt| {
                let handler = for_server(&interrupt.handlers)?;
                let then = match &interrupt.then {
                    actingcommand_contract::InterruptThen::Resume => RunThen::Resume,
                    actingcommand_contract::InterruptThen::EndRun => RunThen::EndRun,
                    actingcommand_contract::InterruptThen::Run(packages) => RunThen::Run {
                        pack: for_server(packages),
                        listed: packages.first().cloned().unwrap_or_default(),
                    },
                };
                Some(RunInterrupt {
                    page: interrupt.page.clone(),
                    handler,
                    then,
                    max_per_run: interrupt.max_per_run,
                })
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(Box::new(RunInterrupts {
            identity: PackageIdentity::of(prepared),
            features,
            pages: pages.pages.iter().map(|page| page.id.clone()).collect(),
            entries,
            admitted_features: None,
            admitted: BTreeMap::new(),
            counts: BTreeMap::new(),
            total: 0,
            configured: BTreeSet::new(),
        })))
    }
}

/// A run-ending failure the interrupt handling decides itself, with its full outcome: the run's
/// terminal carries it, for a direct and a scheduled run alike.
fn interrupt_failure(error: RuntimeHostError) -> ContainedTaskRunError<RequestFailure> {
    let outcome = error.outcome();
    let mut failure = RequestFailure::request(error, RuntimeReceiptState::Failed, None);
    failure.task_failure = Some(TaskFailureEvidence {
        outcome,
        severe: false,
    });
    ContainedTaskRunError::Boundary(failure)
}

/// `contained_task_navigation_package_invalid` for `package_id` in `role` (§9.1), with the
/// loader's refusal as its cause when admission failed.
fn package_invalid(
    package_id: &str,
    role: NavigationRole,
    problem: NavigationProblem,
    cause: Option<Outcome>,
) -> ContainedTaskRunError<RequestFailure> {
    let error = RuntimeHostError::request(
        HostCode::ContainedTaskNavigationPackageInvalid,
        HostLocation::InterruptHandle,
        RuntimeErrorCode::PackageInvalid,
    )
    .with_outcome_values(|outcome| {
        outcome
            .package_id(package_id)
            .navigation_role(role)
            .navigation_problem(problem)
    });
    interrupt_failure(match cause {
        Some(cause) => error.with_caused_by(cause),
        None => error,
    })
}

impl RunInterrupts {
    /// The interrupts the running package does not declare itself (§6.1: a page the package
    /// declares, its id without the game prefix, is its own to handle).
    fn candidates(&self, frame: &SharedInterruptFrame<'_>) -> Vec<usize> {
        let own = frame
            .own_pages
            .iter()
            .map(|page| canonical_page_anchor(frame.game, page))
            .collect::<BTreeSet<_>>();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !own.contains(&entry.page))
            .map(|(index, _)| index)
            .collect()
    }

    /// Why `package` cannot play `role` for this run (§6.2: as a prerequisite layer, same game,
    /// server and resolution, no prerequisite package of its own).
    fn ineligibility(&self, package: &PreparedContainedTask) -> Option<NavigationProblem> {
        let identity = &self.identity;
        match package
            .package_descriptor()
            .prerequisite_incompatibility_with(
                &identity.game,
                &identity.server,
                identity.resolution,
                true,
            ) {
            None => None,
            Some("game" | "server" | "resolution") => Some(NavigationProblem::IdentityMismatch),
            Some("return_home_declares_prerequisite") => {
                Some(NavigationProblem::PrerequisiteDeclared)
            }
            Some(_) => Some(NavigationProblem::RoleIneligible),
        }
    }
}

impl RuntimeContainedTask<'_> {
    /// The kernel's offer of a frame (§6.1). The run's interrupts are taken out while one is
    /// handled, so a handler or `then.run` package never looks at them itself (no nesting).
    pub(super) fn offered_shared_interrupt(
        &mut self,
        frame: SharedInterruptFrame<'_>,
    ) -> Result<SharedInterruptVerdict, ContainedTaskRunError<RequestFailure>> {
        let Some(mut interrupts) = self.interrupts.take() else {
            return Ok(SharedInterruptVerdict::NotHandled);
        };
        let verdict = self.check_shared_interrupt(&mut interrupts, &frame);
        self.interrupts = Some(interrupts);
        verdict
    }

    fn check_shared_interrupt(
        &mut self,
        interrupts: &mut RunInterrupts,
        frame: &SharedInterruptFrame<'_>,
    ) -> Result<SharedInterruptVerdict, ContainedTaskRunError<RequestFailure>> {
        let candidates = interrupts.candidates(frame);
        if candidates.is_empty() {
            return Ok(SharedInterruptVerdict::NotHandled);
        }
        self.admit_features(interrupts)?;
        let features = interrupts.admitted_features.as_ref().ok_or_else(|| {
            ContainedTaskRunError::task(SharedCode::ContainedTaskStateInvalid.as_str())
        })?;
        // The feature package's page of each candidate interrupt page.
        let feature_pages = features
            .page_ids()
            .into_iter()
            .map(|page| {
                (
                    canonical_page_anchor(features.game(), page),
                    page.to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let evaluated = candidates
            .iter()
            .filter_map(|index| feature_pages.get(&interrupts.entries[*index].page))
            .map(String::as_str)
            .collect::<Vec<_>>();
        let matched = features
            .matching_pages_on_scene(frame.scene, &evaluated)
            .map_err(ContainedTaskRunError::Task)?
            .into_iter()
            .map(|page| canonical_page_anchor(features.game(), &page))
            .collect::<BTreeSet<_>>();
        let matched = candidates
            .into_iter()
            .filter(|index| matched.contains(&interrupts.entries[*index].page))
            .collect::<Vec<_>>();
        match matched.as_slice() {
            [] => Ok(SharedInterruptVerdict::NotHandled),
            [index] => self.handle_shared_interrupt(interrupts, *index),
            _ => {
                let page_ids = matched
                    .iter()
                    .map(|index| interrupts.entries[*index].page.clone())
                    .collect::<Vec<_>>();
                self.record_entry_fact(TaskSemanticFact::InterruptAmbiguous {
                    outcome: Outcome::new(HostCode::ContainedTaskInterruptAmbiguous)
                        .operation(HostLocation::InterruptCheck)
                        .page_ids(page_ids.clone()),
                    page_ids,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
                Ok(SharedInterruptVerdict::NotHandled)
            }
        }
    }

    /// The run's feature package, admitted on its first use (§7) and checked for its part: a
    /// `recognize_only` package of the run's game, server and resolution whose pages are exactly
    /// the pages of `pages.json`.
    fn admit_features(
        &mut self,
        interrupts: &mut RunInterrupts,
    ) -> Result<(), ContainedTaskRunError<RequestFailure>> {
        if interrupts.admitted_features.is_some() {
            return Ok(());
        }
        let pack = interrupts.features.clone();
        let role = NavigationRole::Features;
        let admitted = self.admit_layer_package(&pack, role)?;
        let problem = if admitted.execution_mode() != "recognize_only" {
            Some(NavigationProblem::RoleIneligible)
        } else if (admitted.game(), admitted.server(), admitted.resolution())
            != (
                interrupts.identity.game.as_str(),
                interrupts.identity.server.as_str(),
                interrupts.identity.resolution,
            )
        {
            Some(NavigationProblem::IdentityMismatch)
        } else if admitted
            .page_ids()
            .into_iter()
            .map(|page| canonical_page_anchor(admitted.game(), page))
            .collect::<BTreeSet<_>>()
            != interrupts.pages
        {
            Some(NavigationProblem::PageSetMismatch)
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(package_invalid(&pack.package_id, role, problem, None));
        }
        interrupts.admitted_features = Some(admitted);
        Ok(())
    }

    /// Admits a registry package by its path (Workflow #336 PKG-S1: structurally); a refusal is
    /// `contained_task_navigation_package_invalid` with the loader's code as its cause.
    fn admit_layer_package(
        &self,
        pack: &RegisteredPack,
        role: NavigationRole,
    ) -> Result<PreparedContainedTask, ContainedTaskRunError<RequestFailure>> {
        let vision_provider = self
            .host
            .execution()
            .map_err(|error| ContainedTaskRunError::Boundary(RequestFailure::from(error)))?
            .vision_provider();
        let deadline = self
            .host
            .package_material_deadline(self.control.deadline())
            .map_err(ContainedTaskRunError::Boundary)?;
        PreparedContainedTask::load_path(self.instance_alias, &pack.path, vision_provider, deadline)
            .map_err(|error| {
                package_invalid(
                    &pack.package_id,
                    role,
                    NavigationProblem::AdmissionFailed,
                    Some(error.outcome()),
                )
            })
    }

    /// A handler or `then.run` package of the run, admitted once (§7) and checked for its part
    /// (§6.2); a handler's entry must hold the page it handles.
    fn admitted_layer<'r>(
        &mut self,
        interrupts: &'r mut RunInterrupts,
        pack: &RegisteredPack,
        role: NavigationRole,
        page: Option<&str>,
    ) -> Result<&'r PreparedContainedTask, ContainedTaskRunError<RequestFailure>> {
        if !interrupts.admitted.contains_key(&pack.package_id) {
            let admitted = self.admit_layer_package(pack, role)?;
            if let Some(problem) = interrupts.ineligibility(&admitted) {
                return Err(package_invalid(&pack.package_id, role, problem, None));
            }
            interrupts
                .admitted
                .insert(pack.package_id.clone(), admitted);
        }
        let admitted = interrupts.admitted.get(&pack.package_id).ok_or_else(|| {
            ContainedTaskRunError::task(SharedCode::ContainedTaskStateInvalid.as_str())
        })?;
        if let Some(page) = page
            && !admitted.entry_holds_page(page)
        {
            return Err(package_invalid(
                &pack.package_id,
                role,
                NavigationProblem::EntryMismatch,
                None,
            ));
        }
        Ok(admitted)
    }

    /// Handles the interrupt `index` (§6.2): its limits, its handler as a layer, its `then`.
    fn handle_shared_interrupt(
        &mut self,
        interrupts: &mut RunInterrupts,
        index: usize,
    ) -> Result<SharedInterruptVerdict, ContainedTaskRunError<RequestFailure>> {
        let entry = interrupts.entries[index].clone();
        let handled = interrupts.counts.get(&entry.page).copied().unwrap_or(0);
        let limit = if handled >= entry.max_per_run {
            Some((u32::from(handled) + 1, u32::from(entry.max_per_run)))
        } else if interrupts.total >= MAX_INTERRUPT_HANDLINGS_PER_RUN {
            Some((interrupts.total + 1, MAX_INTERRUPT_HANDLINGS_PER_RUN))
        } else {
            None
        };
        if let Some((count, limit_count)) = limit {
            return Err(interrupt_failure(
                RuntimeHostError::request(
                    HostCode::ContainedTaskInterruptLimitReached,
                    HostLocation::InterruptHandle,
                    RuntimeErrorCode::BackendOperationFailed,
                )
                .with_outcome_values(|outcome| {
                    outcome
                        .page_id(entry.page.as_str())
                        .count(count)
                        .limit_count(limit_count)
                }),
            ));
        }
        let handler = self
            .admitted_layer(
                interrupts,
                &entry.handler,
                NavigationRole::Handler,
                Some(&entry.page),
            )?
            .package_label()
            .to_owned();
        self.run_interrupt_layer(interrupts, &entry.handler.package_id)?;
        let then = match &entry.then {
            RunThen::Resume => TaskInterruptThen::Resume,
            RunThen::EndRun => TaskInterruptThen::EndRun,
            RunThen::Run { pack: None, listed } => {
                return Err(package_invalid(
                    listed,
                    NavigationRole::ThenRun,
                    NavigationProblem::ServerMissing,
                    None,
                ));
            }
            RunThen::Run {
                pack: Some(pack), ..
            } => {
                let label = self
                    .admitted_layer(interrupts, pack, NavigationRole::ThenRun, None)?
                    .package_label()
                    .to_owned();
                self.run_interrupt_layer(interrupts, &pack.package_id)?;
                TaskInterruptThen::Run(label)
            }
        };
        let count = handled + 1;
        interrupts.counts.insert(entry.page.clone(), count);
        interrupts.total += 1;
        self.record_entry_fact(TaskSemanticFact::InterruptHandled {
            page: entry.page.clone(),
            handler_package_label: handler,
            then: then.clone(),
            count,
            outcome: Outcome::new(HostCode::ContainedTaskInterruptHandled)
                .operation(HostLocation::InterruptHandle)
                .page_id(entry.page.as_str())
                .count(count),
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        if then == TaskInterruptThen::EndRun {
            return Err(interrupt_failure(
                RuntimeHostError::request(
                    HostCode::ContainedTaskInterruptEscalated,
                    HostLocation::InterruptHandle,
                    RuntimeErrorCode::BackendOperationFailed,
                )
                .with_outcome_values(|outcome| outcome.page_id(entry.page.as_str())),
            ));
        }
        Ok(SharedInterruptVerdict::Handled)
    }

    /// Runs the admitted package `package_id` as a layer of the run (`role: interrupt_handler`,
    /// §6.2, §8.3), as a prerequisite layer runs: its own steps after every step the run used so
    /// far, its failure carried out with its own code. The run's own step indices then go on
    /// after the layer's (the step it was inside keeps its index).
    fn run_interrupt_layer(
        &mut self,
        interrupts: &mut RunInterrupts,
        package_id: &str,
    ) -> Result<(), ContainedTaskRunError<RequestFailure>> {
        let package = interrupts.admitted.get(package_id).ok_or_else(|| {
            ContainedTaskRunError::task(SharedCode::ContainedTaskStateInvalid.as_str())
        })?;
        let role = Some(TaskEntryRecoveryRole::InterruptHandler);
        let package_label = package.package_label().to_owned();
        // The run's steps stay within 1000 (§3.10, `MAX_GATED_STEPS`).
        if u64::from(self.planned_steps)
            + u64::from(self.interrupt_steps)
            + u64::from(package.maximum_executed_steps())
            > MAX_GATED_STEPS
        {
            return Err(ContainedTaskRunError::task(
                SharedCode::ContainedTaskPrerequisiteStepLimit.as_str(),
            ));
        }
        self.record_entry_fact(TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: package_label.clone(),
            role,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
        if self.configuration_records > 0 && interrupts.configured.insert(package_id.to_owned()) {
            self.interrupt_configurations = self.interrupt_configurations.saturating_add(1);
            self.record_configuration(
                EffectiveConfigurationFacts::EntryRecovery {
                    package_label: package_label.clone(),
                    timing: package.effective_timing(),
                },
                None,
                None,
                None,
            )
            .map_err(ContainedTaskRunError::Boundary)?;
        }
        let layer = self.begin_interrupt_layer();
        let previous_timing = self.task_timing.context();
        let execution = {
            let mut layer_runtime = EntryRecoveryRuntime { inner: self };
            package.run_as_prerequisite(&mut layer_runtime)
        };
        if let Err(ContainedTaskRunError::Task(error)) = &execution {
            self.task_timing
                .task_failure(error.timing(), error.timing_check_position());
        }
        self.task_timing.replace_context(previous_timing);
        let executed = execution
            .as_ref()
            .map_or(0, |outcome| outcome.executed_steps);
        self.end_interrupt_layer(layer, executed);
        let outcome = match execution {
            Ok(outcome) => outcome,
            Err(error) => {
                // The layer's own failure code is carried out unchanged (§6.3).
                let code = match &error {
                    ContainedTaskRunError::Task(error) => error.outcome(),
                    ContainedTaskRunError::Boundary(failure)
                    | ContainedTaskRunError::NonfatalOperation(failure) => failure.error.outcome(),
                };
                if let ContainedTaskRunError::Boundary(failure) = &error
                    && (failure.poison_runtime || failure.error.is_fatal())
                {
                    return Err(error);
                }
                self.record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
                    package_label: Some(package_label),
                    outcome: code,
                    role,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
                return Err(error);
            }
        };
        let Some(final_page) = outcome.final_page else {
            return Err(ContainedTaskRunError::task(
                HostCode::ContainedTaskPrerequisiteFinalPageMissing.as_str(),
            ));
        };
        self.record_entry_fact(TaskSemanticFact::EntryRecoveryCompleted {
            package_label,
            final_page,
            executed_steps: outcome.executed_steps,
            role,
        })
        .map_err(ContainedTaskRunError::Boundary)
    }
}

/// The step state of the layer an interrupt layer interrupted, restored when it ends.
pub(super) struct InterruptedLayer {
    offset: u32,
    shift: u32,
}

impl RuntimeContainedTask<'_> {
    /// Workflow #373 NAV-6: an interrupt layer's steps take the indices after every step the run
    /// used so far.
    fn begin_interrupt_layer(&mut self) -> InterruptedLayer {
        let interrupted = InterruptedLayer {
            offset: self.step_index_offset,
            shift: self.step_shift,
        };
        self.step_index_offset = self.next_free_step_index;
        self.step_shift = 0;
        self.interrupt_layer = true;
        interrupted
    }

    /// Back in the interrupted layer after `executed` steps of an interrupt layer: the steps it
    /// starts from now on follow the layer's, and a step it was inside keeps its index.
    fn end_interrupt_layer(&mut self, interrupted: InterruptedLayer, executed: u32) {
        self.interrupt_layer = false;
        self.step_index_offset = interrupted.offset;
        if let Some(&open) = self.step_actions.keys().next()
            && self.pinned_step.is_none_or(|(_, pinned)| pinned != open)
            && let Some(local) = open
                .checked_sub(interrupted.offset)
                .and_then(|local| local.checked_sub(interrupted.shift))
        {
            self.pinned_step = Some((local, open));
        }
        self.step_shift = interrupted.shift.saturating_add(executed);
        self.interrupt_steps = self.interrupt_steps.saturating_add(executed);
    }

    /// The step index a trace of the running layer carries for its own index `step_index`: an
    /// interrupt layer's from its offset; otherwise the layer's offset plus the steps of the
    /// interrupt layers it was interrupted by, except the step it was inside, which keeps its
    /// index. Without shared interrupt pages this is the layer's offset plus `step_index`.
    pub(super) fn layer_step_index(&self, step_index: u32) -> Option<u32> {
        if !self.interrupt_layer {
            if let Some((local, absolute)) = self.pinned_step
                && local == step_index
            {
                return Some(absolute);
            }
            return self
                .step_index_offset
                .checked_add(step_index)?
                .checked_add(self.step_shift);
        }
        self.step_index_offset.checked_add(step_index)
    }

    /// Starts a layer of the run (a prerequisite or entry recovery package, or the run's package
    /// after them) whose steps follow `executed_before` steps of the layers before it, and every
    /// step of the interrupt layers that ran so far.
    pub(super) fn begin_layer(&mut self, executed_before: u32) -> Option<()> {
        self.step_index_offset = executed_before.checked_add(self.interrupt_steps)?;
        self.step_shift = 0;
        self.pinned_step = None;
        Some(())
    }

    /// The run's executed steps once its package ran `executed` steps: one more than the index
    /// of its last step (its offset when it ran none). Without shared interrupt pages this is the
    /// layer's offset plus `executed`.
    pub(super) fn layer_end(&self, executed: u32) -> Option<u32> {
        match executed.checked_sub(1) {
            None => self.step_index_offset.checked_add(self.step_shift),
            Some(last) => self.layer_step_index(last)?.checked_add(1),
        }
    }

    /// `outcome` of the run's package with the run's executed steps ([`Self::layer_end`]).
    pub(super) fn with_layer_end(
        &self,
        mut outcome: ContainedTaskOutcome,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>> {
        outcome.executed_steps = self.layer_end(outcome.executed_steps).ok_or_else(|| {
            ContainedTaskRunError::task(SharedCode::ContainedTaskPrerequisiteStepLimit.as_str())
        })?;
        Ok(outcome)
    }
}
