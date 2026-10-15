// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 PKG-S3 (M-PKG §3.3, §3.4, D1, D18; `contracts/policy-suspension.md`,
//! "Lifting"): the content hash of a task package, and the lift rule of a paused pair.
//!
//! The content hash has one use left (option 丙): the live settlement of a scheduled failure
//! records the hash of each package its run went through in the failure identity, and a lift
//! check measures those packages again; a pause lifts once a package's hash differs from the
//! one recorded. Admission never computes it. A lift check measures one package at most once per
//! `scheduling.suspension.lift_check_min_interval_ms` and reuses that measurement until then. A
//! package that cannot be measured does not lift the pause: the check reports
//! `lift_check_hash_unmeasured` and measures it again at a later check (D18). Everything here is
//! memory; a restart rebuilds the recorded hashes from the failure identities and checks again.

use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::{
    FailureIdentity, IdentityLayer, IdentityPackage, MISSING_HASH, content_hash_prefix,
    package_id_hash, return_home_key_hash,
};
use crate::{ProcedureBinding, ProcedureManifest, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{RuntimeErrorCode, StandardPackRegistry};
use actingcommand_pack_containment::{Containment, ContainmentError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// How long one measurement may read a package (one bounded snapshot, as admission reads it).
const MEASURE_BUDGET: Duration = Duration::from_secs(60);
/// The current hash of a layer whose mapping changed (several package ids share its id hash,
/// or its return-home entry names another package): equal to no recorded hash.
const MAPPING_CHANGED: &str = "?";

/// The content hash of the package at `path` (M-PKG §3.3, D3), in lowercase hex, or why it
/// could not be measured: the containment refusal's code when the refusal names one. Called
/// only when a scheduled failure is settled and by a lift check.
pub(crate) fn measure_package_hash(path: &str) -> Result<String, Option<Code>> {
    Containment::new()
        .measure_content_hash(Path::new(path), Instant::now() + MEASURE_BUDGET)
        .map_err(|error| match error {
            ContainmentError::SourceTree { code } => Some(code),
            _ => None,
        })
}

/// What a paused pair's suspension is lifted against (§12.7): the main package and the
/// prerequisite layers its failure identity names, with the hashes measured at its
/// settlement. A pause recorded without a failure identity names no package (`main` is
/// `None`): its main package is compared with the hash its first lift check measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PausedPackages {
    pub(crate) main: Option<IdentityPackage>,
    pub(crate) layers: Vec<IdentityLayer>,
}

impl PausedPackages {
    /// The packages a recorded failure identity names; none for another record.
    pub(crate) fn of_identity(failure_identity: Option<&str>) -> Self {
        match failure_identity.and_then(FailureIdentity::parse) {
            Some(identity) => Self {
                main: Some(identity.main),
                layers: identity.layers,
            },
            None => Self {
                main: None,
                layers: Vec::new(),
            },
        }
    }
}

/// What lifted a suspension: the main package's content hash, or the layer at a one-based
/// index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuspensionLift {
    MainHash,
    Layer(usize),
}

impl SuspensionLift {
    pub(crate) fn label(self) -> String {
        match self {
            Self::MainHash => "main_hash".to_owned(),
            Self::Layer(index) => format!("layer:{index}"),
        }
    }
}

/// One lift check of a paused pair: what lifted it, if anything, and a
/// `lift_check_hash_unmeasured` warning for each package it could not measure.
#[derive(Debug, Default)]
pub(crate) struct LiftVerdict {
    pub(crate) lifted: Option<SuspensionLift>,
    pub(crate) unmeasured: Vec<Outcome>,
}

/// When a lift check runs: the Runtime's monotonic time and the least time between two
/// measurements of one package (`scheduling.suspension.lift_check_min_interval_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiftCheckTime {
    pub(crate) monotonic_ms: u64,
    pub(crate) min_interval_ms: u64,
}

impl LiftCheckTime {
    /// One read: every package is measured once and that measurement serves the whole read
    /// (`actingd suspended`).
    pub(crate) const ONE_READ: Self = Self {
        monotonic_ms: 0,
        min_interval_ms: u64::MAX,
    };
}

/// One package's latest measurement.
#[derive(Debug, Clone)]
struct Measured {
    at_monotonic_ms: u64,
    hash: Result<String, Option<Code>>,
}

#[derive(Debug, Default)]
struct LiftCheckState {
    /// By package location.
    measured: BTreeMap<String, Measured>,
    /// By the paused dispatch's decision id: the main package's hash prefix the first check of
    /// a pause recorded without a failure identity measured.
    baselines: BTreeMap<String, String>,
}

/// The lift checks' memory: each package's latest measurement and the baselines of pauses
/// recorded without a failure identity.
#[derive(Debug, Default)]
pub(crate) struct LiftChecks {
    state: Mutex<LiftCheckState>,
}

impl LiftChecks {
    fn state(&self) -> RuntimeHostResult<MutexGuard<'_, LiftCheckState>> {
        self.state.lock().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::RuntimeStatePoisoned,
                HostLocation::CheckSuspensionLift,
                RuntimeErrorCode::RuntimeHalted,
            )
        })
    }

    /// The content hash of the package at `path` as of `time`: the latest measurement while it
    /// is younger than the interval, else a new one. Workflow #386 SR-3: a suspended task
    /// package's lift check reads it too.
    pub(crate) fn current(
        &self,
        path: &str,
        time: LiftCheckTime,
    ) -> RuntimeHostResult<Result<String, Option<Code>>> {
        let mut state = self.state()?;
        if let Some(measured) = state.measured.get(path)
            && time.monotonic_ms.saturating_sub(measured.at_monotonic_ms) < time.min_interval_ms
        {
            return Ok(measured.hash.clone());
        }
        let hash = measure_package_hash(path);
        state.measured.insert(
            path.to_owned(),
            Measured {
                at_monotonic_ms: time.monotonic_ms,
                hash: hash.clone(),
            },
        );
        Ok(hash)
    }

    /// Whether the main package of a pause recorded without a failure identity changed: its
    /// first check keeps `current` as the baseline and lifts nothing.
    fn baseline_changed(&self, decision_id: &str, current: &str) -> RuntimeHostResult<bool> {
        let mut state = self.state()?;
        match state.baselines.get(decision_id) {
            Some(baseline) => Ok(baseline != current),
            None => {
                state
                    .baselines
                    .insert(decision_id.to_owned(), current.to_owned());
                Ok(false)
            }
        }
    }
}

/// A package's content hash prefix as a lift check reads it now, or the warning that it could
/// not be measured.
enum Current {
    Hash(String),
    Unmeasured(Outcome),
}

/// `lift_check_hash_unmeasured` (warning): the package at `path` (absent when its procedure
/// names no package location) could not be measured, with the containment refusal behind it.
fn unmeasured(path: Option<&str>, package_id: Option<&str>, cause: Option<Code>) -> Outcome {
    let mut outcome = Outcome::new(HostCode::LiftCheckHashUnmeasured);
    if let Some(path) = path {
        outcome = outcome.path(path);
    }
    if let Some(package_id) = package_id {
        outcome = outcome.package_id(package_id);
    }
    match cause {
        Some(cause) => outcome.caused_by(Outcome::new(cause)),
        None => outcome,
    }
}

/// One game root's registry as a lift check reads it: its game, its directory, its registry.
type GameRegistryRead = (String, PathBuf, StandardPackRegistry);

/// The configuration a suspension is judged against (§12.7): the procedure bindings with their
/// packages' locations, as read at startup (or by `actingd suspended`), the standard-pack root
/// whose registries name the prerequisite and return-home layers (Workflow #373 NAV-3a, M-373
/// §4.3: read when a layer is first looked up, once per view), and the lift checks' memory.
pub(crate) struct SuspensionLiftView<'a> {
    // Workflow #386 SR-3: the pack admission's deferral reads the bindings and checks a
    // suspended package's lift through the same memory and time.
    pub(crate) manifest: &'a ProcedureManifest,
    standard_packs_root: Option<&'a Path>,
    registries: OnceLock<Vec<GameRegistryRead>>,
    pub(crate) checks: &'a LiftChecks,
    pub(crate) time: LiftCheckTime,
}

impl<'a> SuspensionLiftView<'a> {
    pub(crate) fn new(
        manifest: &'a ProcedureManifest,
        standard_packs_root: Option<&'a Path>,
        checks: &'a LiftChecks,
        time: LiftCheckTime,
    ) -> Self {
        Self {
            manifest,
            standard_packs_root,
            registries: OnceLock::new(),
            checks,
            time,
        }
    }

    /// Every game root's registry, read on first use.
    fn registries(&self) -> &[GameRegistryRead] {
        self.registries
            .get_or_init(|| crate::host::read_registries(self.standard_packs_root))
    }

    /// The registry packages whose package id has `id_hash`: (game, package id, path).
    fn registered_with_hash(&self, id_hash: &str) -> Vec<(&str, &str, PathBuf)> {
        self.registries()
            .iter()
            .flat_map(|(game, directory, registry)| {
                registry
                    .package_ids()
                    .filter(move |package_id| package_id_hash(package_id) == id_hash)
                    .filter_map(move |package_id| {
                        registry.get(package_id).map(|entry| {
                            (game.as_str(), package_id, directory.join(&entry.directory))
                        })
                    })
            })
            .collect()
    }
}

impl SuspensionLiftView<'_> {
    fn measure(&self, path: &str, package_id: Option<&str>) -> RuntimeHostResult<Current> {
        Ok(match self.checks.current(path, self.time)? {
            Ok(hash) => Current::Hash(content_hash_prefix(&hash)),
            Err(cause) => Current::Unmeasured(unmeasured(Some(path), package_id, cause)),
        })
    }

    fn current_main(&self, binding: &ProcedureBinding) -> RuntimeHostResult<Current> {
        match binding.package_path() {
            Some(path) => self.measure(path, None),
            None => Ok(Current::Unmeasured(unmeasured(None, None, None))),
        }
    }

    /// The return-home package the registries name for the (game, server) key hash, if exactly
    /// one: its game, package id and path (`None` for the path when its game root does not list
    /// it).
    fn return_home_id(&self, key_hash: &str) -> Option<(&str, &str, Option<PathBuf>)> {
        let mut matching = self
            .registries()
            .iter()
            .flat_map(|(game, directory, registry)| {
                registry
                    .return_home_packages()
                    .iter()
                    .filter(move |(server, _)| return_home_key_hash(game, server) == key_hash)
                    .map(move |(_, package_id)| {
                        (
                            game.as_str(),
                            package_id.as_str(),
                            registry
                                .get(package_id)
                                .map(|entry| directory.join(&entry.directory)),
                        )
                    })
            });
        match (matching.next(), matching.next()) {
            (Some(found), None) => Some(found),
            _ => None,
        }
    }

    fn current_layer(&self, layer: &IdentityLayer) -> RuntimeHostResult<Current> {
        match layer {
            IdentityLayer::Declared(package) => {
                match self.registered_with_hash(&package.id_hash).as_slice() {
                    [] => Ok(Current::Hash(MISSING_HASH.to_owned())),
                    [(_, package_id, path)] => {
                        self.measure(&path.to_string_lossy(), Some(*package_id))
                    }
                    _ => Ok(Current::Hash(MAPPING_CHANGED.to_owned())),
                }
            }
            IdentityLayer::ReturnHome { key_hash, package } => {
                match self
                    .return_home_id(key_hash)
                    .filter(|(_, package_id, _)| package_id_hash(package_id) == package.id_hash)
                {
                    Some((_, package_id, Some(path))) => {
                        self.measure(&path.to_string_lossy(), Some(package_id))
                    }
                    Some((_, _, None)) => Ok(Current::Hash(MISSING_HASH.to_owned())),
                    None => Ok(Current::Hash(MAPPING_CHANGED.to_owned())),
                }
            }
        }
    }

    /// The main package's content hash prefix now, `None` when it cannot be measured or the
    /// procedure is not bound.
    pub(crate) fn current_main_hash(
        &self,
        procedure_ref: &str,
    ) -> RuntimeHostResult<Option<String>> {
        let Some(binding) = self.manifest.binding(procedure_ref) else {
            return Ok(None);
        };
        Ok(match self.current_main(binding)? {
            Current::Hash(hash) => Some(hash),
            Current::Unmeasured(_) => None,
        })
    }

    /// A layer's content hash prefix now (`x` without a mapped package, `?` when its mapping
    /// changed), `None` when it cannot be measured.
    pub(crate) fn current_layer_hash(
        &self,
        layer: &IdentityLayer,
    ) -> RuntimeHostResult<Option<String>> {
        Ok(match self.current_layer(layer)? {
            Current::Hash(hash) => Some(hash),
            Current::Unmeasured(_) => None,
        })
    }

    /// The package id a layer names in the current registries (Workflow #373 NAV-3a), if
    /// exactly one registry package has its hash.
    pub(crate) fn layer_package_id(&self, layer: &IdentityLayer) -> Option<String> {
        if let IdentityLayer::ReturnHome { key_hash, .. } = layer
            && let Some((_, package_id, _)) = self.return_home_id(key_hash)
            && package_id_hash(package_id) == layer.id_hash()
        {
            return Some(package_id.to_owned());
        }
        match self.registered_with_hash(layer.id_hash()).as_slice() {
            [(_, package_id, _)] => Some((*package_id).to_owned()),
            _ => None,
        }
    }

    /// §12.7: whether the suspension of the dispatch `decision_id` of `procedure_ref`, paused
    /// on `paused`, is lifted: the main package's hash or a layer's differs from the one
    /// recorded. A package that cannot be measured lifts nothing and is reported. An unbound
    /// procedure never lifts (its whole cycle fails anyway).
    pub(crate) fn lifted(
        &self,
        procedure_ref: &str,
        decision_id: &str,
        paused: &PausedPackages,
    ) -> RuntimeHostResult<LiftVerdict> {
        let mut verdict = LiftVerdict::default();
        let Some(binding) = self.manifest.binding(procedure_ref) else {
            return Ok(verdict);
        };
        match self.current_main(binding)? {
            Current::Unmeasured(warning) => verdict.unmeasured.push(warning),
            Current::Hash(current) => {
                let changed = match &paused.main {
                    Some(recorded) => current != recorded.hash,
                    None => self.checks.baseline_changed(decision_id, &current)?,
                };
                if changed {
                    verdict.lifted = Some(SuspensionLift::MainHash);
                    return Ok(verdict);
                }
            }
        }
        for (index, layer) in paused.layers.iter().enumerate() {
            match self.current_layer(layer)? {
                Current::Unmeasured(warning) => verdict.unmeasured.push(warning),
                Current::Hash(current) if current != layer.hash() => {
                    verdict.lifted = Some(SuspensionLift::Layer(index + 1));
                    return Ok(verdict);
                }
                Current::Hash(_) => {}
            }
        }
        Ok(verdict)
    }
}
