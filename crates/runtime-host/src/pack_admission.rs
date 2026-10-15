// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-3 (scheduling model v1.4.2 §2.4, §10, §11; M-PKG v1.1 §3.3, §3.4, §5.2;
//! `contracts/policy-suspension.md`, "Task package suspension"): a task package that admission
//! refuses with a code of `scheduling.pack_admission.suspend_codes` is suspended, identified by
//! its `package_id` under this Runtime build.
//!
//! One rule, three places: `check-config` admits every bound package and lists each refusal of
//! the rule as a note; the daemon admits them once in the background after its start; a
//! scheduled run's preparation suspends its package at the first such refusal. Right after the
//! refusal the package's content hash is measured once; admission itself never measures it.
//! While a package is suspended, every pair whose task's procedure locates it is deferred at
//! position 3 of the evaluation with `pack_admission_suspended` and never prepared. A due
//! pair's evaluation checks the lift at most once per package per
//! `scheduling.suspension.lift_check_min_interval_ms` (PKG-S3's measurements): a content hash
//! that differs lifts it, and the next preparation tries the package once; the same refusal
//! suspends it again. The suspension is memory only: a restart (or another build) starts
//! without it and the background admission judges the packages again.
//!
//! The ledger (§11, coordinator ruling 4 of wave 14): the evaluation copies a suspension once
//! for each instance it defers a pair of, and a lift once for each instance the suspension was
//! copied for, as a `goal_missed` planning signal (the policy-decision records the G5 probe
//! copies) whose outcome is `pack_admission_suspended` or `pack_admission_lifted` and whose
//! `signal_id` starts with `signal:pack_admission:`. No record type and no member is new.

use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::content_hash_prefix;
use crate::suspension_lift::{SuspensionLiftView, measure_package_hash};
use crate::{
    ProcedureBinding, ProcedureManifest, RuntimeHostConfig, RuntimeHostError, RuntimeHostResult,
};
use actingcommand_contract::outcome::{CauseRelation, Code, Outcome};
use actingcommand_contract::{
    PackageRef, PolicyPlanningSignalEventData, PolicyPlanningSignalKind, RuntimeErrorCode,
};
use actingcommand_execution_kernel::PreparedContainedTask;
use actingcommand_policy::{DecisionReason, DispatchIntent};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long one admission of the check may read a package (one bounded snapshot, as a lift
/// check reads it).
const ADMISSION_BUDGET: Duration = Duration::from_secs(60);
/// The instance label the check admits a package under: admission depends only on the package
/// and the kernel, not on an instance.
const ADMISSION_LABEL: &str = "pack-admission";
/// The `signal_id` prefix of the planning signals that copy a suspension or a lift.
pub(crate) const SIGNAL_PREFIX: &str = "signal:pack_admission:";
/// The `signal_id` segment of a copied suspension and of a copied lift.
const SUSPENDED_WORD: &str = "suspended";
const LIFTED_WORD: &str = "lifted";

/// Whether `code` is one of the suspend codes in effect.
pub(crate) fn suspends(suspend_codes: &[String], code: Code) -> bool {
    suspend_codes.iter().any(|listed| listed == code.as_str())
}

/// The package locations the procedure bindings name, each once, in order.
pub(crate) fn bound_paths(manifest: &ProcedureManifest) -> BTreeSet<String> {
    manifest
        .bindings()
        .filter_map(ProcedureBinding::package_path)
        .map(str::to_owned)
        .collect()
}

/// One package location's admission by this Runtime's kernel, as the suspension rule reads it.
#[derive(Debug)]
pub(crate) enum Admission {
    /// Admitted: the `package_id` its control declares.
    Admitted { package_id: String },
    /// Refused with a suspend code, naming its `package_id`.
    Suspend { package_id: String, code: Code },
    /// Refused otherwise (handled where it is dispatched), or without a readable `package_id`.
    Refused,
}

/// Admits the package at `path` in full, as preparation admits it (the kernel's preparation with
/// the metadata-only vision capability), and reads the verdict for the suspension rule.
pub(crate) fn admit(path: &str, suspend_codes: &[String]) -> Admission {
    match PreparedContainedTask::describe_path(
        ADMISSION_LABEL,
        Path::new(path),
        &PackageRef::default(),
        Instant::now() + ADMISSION_BUDGET,
    ) {
        Ok(descriptor) => Admission::Admitted {
            package_id: descriptor.package_id().to_owned(),
        },
        Err(error) => match (Code::relayed(error.code().as_str()), error.package_id()) {
            (Some(code), Some(package_id)) if suspends(suspend_codes, code) => Admission::Suspend {
                package_id: package_id.to_owned(),
                code,
            },
            _ => Admission::Refused,
        },
    }
}

/// `pack_admission_suspended` with its values: the package, its content hash measured after the
/// refusal (left out when it could not be measured), the refusal's code and the build (left out
/// when the build names no commit).
fn suspended_outcome(
    package_id: &str,
    hash: Option<&str>,
    failure_code: Code,
    runtime_commit: Option<&str>,
) -> Outcome {
    let mut outcome = Outcome::new(HostCode::PackAdmissionSuspended)
        .package_id(package_id)
        .failure_code(failure_code);
    if let Some(hash) = hash {
        outcome = outcome.actual_digest(format!("sha256:{hash}"));
    }
    if let Some(commit) = runtime_commit {
        outcome = outcome.runtime_commit(commit);
    }
    outcome
}

/// `pack_admission_lifted` with its values: the package, its content hash measured now and the
/// build.
fn lifted_outcome(package_id: &str, hash: &str, runtime_commit: Option<&str>) -> Outcome {
    let outcome = Outcome::new(HostCode::PackAdmissionLifted)
        .package_id(package_id)
        .actual_digest(format!("sha256:{hash}"));
    match runtime_commit {
        Some(commit) => outcome.runtime_commit(commit),
        None => outcome,
    }
}

/// Workflow #386 SR-3 (§2.4 item 2, `check-config`): admits every package location the
/// configuration's procedure bindings name with this Runtime's kernel and gives one
/// `pack_admission_suspended` per package refused with a suspend code, its content hash measured
/// right after the refusal. Any other refusal gives nothing here (it is handled at dispatch).
/// Nothing is written.
pub fn pack_admission_notes(
    config: &RuntimeHostConfig,
    suspend_codes: &[String],
    runtime_commit: Option<&str>,
) -> Vec<Outcome> {
    let Some(manifest) = config.procedure_manifest() else {
        return Vec::new();
    };
    let mut notes = BTreeMap::new();
    for path in bound_paths(manifest) {
        if let Admission::Suspend { package_id, code } = admit(&path, suspend_codes) {
            let hash = measure_package_hash(&path).ok();
            notes.entry(package_id.clone()).or_insert_with(|| {
                suspended_outcome(&package_id, hash.as_deref(), code, runtime_commit)
            });
        }
    }
    notes.into_values().collect()
}

/// One suspended task package (memory only).
#[derive(Debug, Clone)]
struct Suspension {
    /// The code admission refused it with.
    failure_code: Code,
    /// Its content hash measured right after the refusal; `None` when it could not be measured
    /// (M-PKG D18), so any later measurement lifts it.
    hash: Option<String>,
    /// The instances the evaluation copied it for.
    recorded: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct State {
    /// The `package_id` admission read at each package location.
    ids: BTreeMap<String, String>,
    /// The suspended task packages by `package_id`.
    suspended: BTreeMap<String, Suspension>,
    /// The planning signals the evaluation produced and the recorder has not taken yet.
    signals: Vec<PolicyPlanningSignalEventData>,
    /// Makes each produced signal's identity unique within this process.
    produced: u64,
}

/// The task package suspensions of this process and the build they are judged under.
#[derive(Debug, Default)]
pub(crate) struct PackAdmission {
    state: Mutex<State>,
    /// The commit this Runtime was built from, when the build names one.
    runtime_commit: Option<String>,
}

impl PackAdmission {
    pub(crate) fn new(runtime_commit: Option<String>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            runtime_commit,
        }
    }

    fn state(&self) -> RuntimeHostResult<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::RuntimeStatePoisoned,
                HostLocation::CheckPackAdmission,
                RuntimeErrorCode::RuntimeHalted,
            )
        })
    }

    /// Remembers the `package_id` admission read at `path`.
    pub(crate) fn learn(&self, path: &str, package_id: &str) -> RuntimeHostResult<()> {
        self.state()?
            .ids
            .insert(path.to_owned(), package_id.to_owned());
        Ok(())
    }

    /// Suspends the task package `package_id`, which admission at `path` refused with `code`:
    /// its content hash is measured once, right after the refusal (M-PKG §3.3). A package already
    /// suspended on the same content hash keeps its suspension and what was copied of it.
    pub(crate) fn suspend(
        &self,
        path: &str,
        package_id: &str,
        code: Code,
    ) -> RuntimeHostResult<()> {
        let hash = measure_package_hash(path).ok();
        let mut state = self.state()?;
        state.ids.insert(path.to_owned(), package_id.to_owned());
        if state
            .suspended
            .get(package_id)
            .is_some_and(|suspension| suspension.hash.is_some() && suspension.hash == hash)
        {
            return Ok(());
        }
        state.suspended.insert(
            package_id.to_owned(),
            Suspension {
                failure_code: code,
                hash,
                recorded: BTreeSet::new(),
            },
        );
        Ok(())
    }

    /// The `package_id` admission read at `path`, when that task package is suspended.
    fn suspended_at(&self, path: &str) -> RuntimeHostResult<Option<String>> {
        let state = self.state()?;
        Ok(state
            .ids
            .get(path)
            .filter(|package_id| state.suspended.contains_key(*package_id))
            .cloned())
    }

    /// The planning signals produced since the last call, in order, for the recorder.
    pub(crate) fn take_signals(&self) -> RuntimeHostResult<Vec<PolicyPlanningSignalEventData>> {
        Ok(std::mem::take(&mut self.state()?.signals))
    }

    /// Position 3 of the evaluation (`scheduling-blackout.md`, "Order of the deferrals"): the
    /// deferral of `intent` when its procedure locates a suspended task package, after the lift
    /// check of that package (at most once per interval, PKG-S3's measurements); `None` when the
    /// package is not suspended or the check lifted it. A package that cannot be measured stays
    /// suspended and its reason carries `lift_check_hash_unmeasured` as a note. The first
    /// deferral on an instance copies the suspension for it; a lift copies itself for each
    /// instance the suspension was copied for.
    pub(crate) fn deferral(
        &self,
        lift: &SuspensionLiftView<'_>,
        intent: &DispatchIntent,
        at_unix_ms: u64,
    ) -> RuntimeHostResult<Option<DecisionReason>> {
        let Some(path) = lift
            .manifest
            .binding(&intent.procedure_ref)
            .and_then(ProcedureBinding::package_path)
        else {
            return Ok(None);
        };
        let Some(package_id) = self.suspended_at(path)? else {
            return Ok(None);
        };
        // Measured outside this memory's lock: a read of the package can take a while.
        let current = lift.checks.current(path, lift.time)?;
        let commit = self.runtime_commit.as_deref();
        let mut guard = self.state()?;
        let state = &mut *guard;
        let Some(suspension) = state.suspended.get_mut(&package_id) else {
            return Ok(None);
        };
        let unmeasured = match &current {
            Ok(hash) if suspension.hash.as_deref() != Some(hash.as_str()) => {
                let recorded = std::mem::take(&mut suspension.recorded);
                state.suspended.remove(&package_id);
                for instance_id in recorded {
                    state.produced += 1;
                    let signal = copy_signal(
                        LIFTED_WORD,
                        &package_id,
                        &instance_id,
                        None,
                        lifted_outcome(&package_id, hash, commit),
                        at_unix_ms,
                        state.produced,
                    );
                    state.signals.push(signal);
                }
                return Ok(None);
            }
            Ok(_) => None,
            Err(cause) => {
                let warning = Outcome::new(HostCode::LiftCheckHashUnmeasured)
                    .path(path)
                    .package_id(package_id.as_str());
                Some(match cause {
                    Some(cause) => warning.caused_by(Outcome::new(*cause)),
                    None => warning,
                })
            }
        };
        let outcome = suspended_outcome(
            &package_id,
            suspension.hash.as_deref(),
            suspension.failure_code,
            commit,
        );
        if suspension.recorded.insert(intent.instance_id.clone()) {
            state.produced += 1;
            let signal = copy_signal(
                SUSPENDED_WORD,
                &package_id,
                &intent.instance_id,
                Some(&intent.task_id),
                outcome.clone(),
                at_unix_ms,
                state.produced,
            );
            state.signals.push(signal);
        }
        let detail = format!(
            "task package suspended: package_id={package_id}; hash_prefix={}",
            suspension
                .hash
                .as_deref()
                .map_or_else(|| "x".to_owned(), content_hash_prefix)
        );
        Ok(Some(DecisionReason {
            outcome: match unmeasured {
                Some(warning) => outcome.joined(CauseRelation::Note, warning),
                None => outcome,
            },
            detail,
        }))
    }
}

/// One copy of a suspension (`suspended`) or a lift (`lifted`) for `instance_id`: a `goal_missed`
/// planning signal whose outcome is the copied code. Its `signal_id` is
/// `signal:pack_admission:<word>:<hex>`, `<hex>` the lowercase SHA-256 of the package, the
/// instance, the evaluation time and the process's count of produced signals, so no two copies
/// share one.
fn copy_signal(
    word: &str,
    package_id: &str,
    instance_id: &str,
    task_id: Option<&str>,
    outcome: Outcome,
    observed_at_unix_ms: u64,
    produced: u64,
) -> PolicyPlanningSignalEventData {
    let mut digest = Sha256::new();
    digest.update(package_id.as_bytes());
    digest.update([0]);
    digest.update(instance_id.as_bytes());
    digest.update([0]);
    digest.update(observed_at_unix_ms.to_string().as_bytes());
    digest.update([0]);
    digest.update(produced.to_string().as_bytes());
    PolicyPlanningSignalEventData {
        signal_id: format!("{SIGNAL_PREFIX}{word}:{:x}", digest.finalize()),
        instance_id: instance_id.to_owned(),
        task_id: task_id.map(str::to_owned),
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome,
        observed_at_unix_ms,
        detection_budget: None,
    }
}

/// Workflow #386 SR-3, test only: holds the task package admission after start until the test
/// releases it, and tells the test when it has finished.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PackAdmissionHold {
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    finished: Mutex<std::sync::mpsc::Sender<()>>,
}

#[cfg(test)]
impl PackAdmissionHold {
    /// The hold, the sender that releases it and the receiver told when the admission finished.
    pub(crate) fn open() -> (
        std::sync::Arc<Self>,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    ) {
        let (release, released) = std::sync::mpsc::channel();
        let (finished, finished_seen) = std::sync::mpsc::channel();
        (
            std::sync::Arc::new(Self {
                release: Mutex::new(released),
                finished: Mutex::new(finished),
            }),
            release,
            finished_seen,
        )
    }

    /// Waits for the release: `false` when the test dropped its sender or the Runtime shuts down
    /// first, and the admission then does nothing.
    pub(crate) fn await_release(&self, shutting_down: impl Fn() -> bool) -> bool {
        let Ok(release) = self.release.lock() else {
            return false;
        };
        loop {
            match release.recv_timeout(Duration::from_millis(20)) {
                Ok(()) => return true,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if shutting_down() => {
                    return false;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// Tells the test that the admission finished.
    pub(crate) fn finished(&self) {
        if let Ok(finished) = self.finished.lock() {
            // The test may have stopped listening; nothing waits on this answer then.
            let _ = finished.send(());
        }
    }
}
