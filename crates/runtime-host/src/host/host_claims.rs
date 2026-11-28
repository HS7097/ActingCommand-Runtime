// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #369 W-2, Q-5: the per-instance host workers and the Runtime-held keys they run
//! under (model-369-queue v3.1).
//!
//! Every registered instance has one long-lived worker thread, `actingd-instance-<alias>`, fed
//! by a channel and created before the daemon-start preparation. A host claim (a startup
//! package, a stuck-recovery ladder) is registered with its payload before it can be granted;
//! whoever grants it (the pump, or a hand-off at a lease end) posts the payload with the
//! claim's key to the instance's worker. Nothing starts a thread on a grant, and no shared
//! thread waits on one instance's device. A worker counts as lifecycle work while it runs a
//! payload, so an install drain waits for it, and the key is released once at the end,
//! whatever the payload did with it.
//!
//! A worker panic follows spec §7: the thread root catches it and records `panic_caught` under
//! the payload's own failure category; the payload is dropped, not retried (device actions are
//! not idempotent); its key is left to lapse at its TTL, where the sweep's expiry hands it on;
//! and the worker carries on with fresh state.
//!
//! #503 ISO-A (board #419, the owner's rule of 10-24: one instance's fault does not stop the
//! whole Runtime): the worker is the containment boundary of its payloads (ISO-M R0.5).
//! - A payload that ends with an error goes to the one decision point (`contain_failure`) with
//!   the payload's own instance and links: a failure of the whole latches the Runtime as before;
//!   one of the instance fences it with the fence's own records; one of the operation is
//!   recorded once (`runtime.failed` with its own code). Then the ends a panic runs for its kind
//!   run too (a policy run's lane, a probe's unavailability with the error's code, an entry
//!   recovery), and the worker goes on.
//! - At a panic, first of all, the poison the unwind left on the payload's own two locks (its
//!   device's admission guard and queue-order lock, which guard no data) is cleared without
//!   blocking; when either was poisoned the instance is fenced with `instance_lock_poisoned`
//!   after the panic's ends and its record. A second panic before the rebuilt worker finished a
//!   payload fences the payload's instance (cause `panic_caught`) instead of asking for a
//!   restart of the whole Runtime, and the worker goes on.
//!
//! Workflow #427-A: a worker also runs the scheduled policy runs the driver hands it
//! (`policy_lanes.rs`), one at a time, under the same panic rule; a panicked policy run's
//! registration is left to the reconciliation, which settles the run once its lease ended (fix 1,
//! review M1), never stuck.

use super::device_link::PendingLinkRestore;
use super::emulator_autostart::PendingAutostart;
use super::idle::PendingIdleAction;
use super::recovery_ladder::PendingRecoveryLadder;
use super::resource_close::PendingHeldStartPreparation;
use super::resource_fence::PendingFenceRecheck;
use super::startup_package::PendingStartupPackage;
use super::unavailable_probe::PendingUnavailableProbe;
use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;

/// How often a granted claim retries a lifecycle admission that is still closed (a daemon
/// start that is still preparing, an install hold) before its payload runs; a handed-off policy
/// run waits the same way (Workflow #427-A fix 1, `policy_lanes.rs`).
pub(super) const HOST_CLAIM_ADMISSION_POLL: Duration = Duration::from_millis(100);

/// What a granted host claim runs on its instance's worker.
pub(super) enum HostClaimWork {
    StartupPackage(PendingStartupPackage),
    RecoveryLadder(Box<PendingRecoveryLadder>),
    /// Workflow #398 RD-2 (model-398 v1.1 §4.3): the preparation of a handset whose link came
    /// back to `device`, on a self-check claim.
    LinkRestore(PendingLinkRestore),
    /// Workflow #369 S6c (H-5): a configured emulator start at daemon start, on its autostart
    /// claim.
    Autostart(PendingAutostart),
    /// #411 A38: one periodic self-check of a fence that waits for a person, on a self-check
    /// claim.
    FenceRecheck(PendingFenceRecheck),
    /// IDLE-1: one instance's idle action, on an emulator-control claim.
    IdleAction(Box<PendingIdleAction>),
    /// Workflow #437 G1 fix round 2: the start's preparation of a member of a device group of
    /// several that the device's hold for another member's unconfirmed close refused, queued by
    /// the lease sweep once that hold has ended, on a self-check claim.
    HeldStartPreparation(PendingHeldStartPreparation),
    /// Workflow #489 AVAIL-1: one probe (the device self-check only) of an instance its device
    /// self-check made unavailable, on a self-check claim.
    UnavailableProbe(PendingUnavailableProbe),
}

impl HostClaimWork {
    const fn failure_category(&self) -> &'static str {
        match self {
            Self::StartupPackage(_) => "startup_package",
            Self::RecoveryLadder(_) => "recovery_ladder",
            // The payload runs the registered host operation of a link's restore.
            Self::LinkRestore(_) => HostLocation::DeviceLinkRestore.as_str(),
            // The payload runs the registered host operation of a configured start.
            Self::Autostart(_) => HostLocation::RunEmulatorAutostart.as_str(),
            // The payload runs the registered host operation of a fence's recheck.
            Self::FenceRecheck(_) => HostLocation::RunResourceFenceRecheck.as_str(),
            // The payload runs the registered host operation of an idle action.
            Self::IdleAction(_) => crate::codes::IdleLocation::Action.as_str(),
            // The payload runs the registered host operation of a held start preparation.
            Self::HeldStartPreparation(_) => HostLocation::RunHeldStartPreparation.as_str(),
            // The payload runs the registered host operation of an unavailable instance's probe.
            Self::UnavailableProbe(_) => HostLocation::RunUnavailableProbe.as_str(),
        }
    }
}

/// Workflow #369 Q-5: a Runtime holder's key: its claim's own request (its links carry the
/// holder's records), the current token, which every renewal and continuation grant replaces,
/// and its synthetic connection.
pub(super) struct HostKey {
    pub(super) request: RuntimeRequest,
    pub(super) token: LeaseToken,
    pub(super) connection_id: ConnectionId,
}

// The channel moves each job once, by value; boxing the host claim's payload as well gains
// nothing.
#[allow(clippy::large_enum_variant)]
pub(super) enum WorkerJob {
    HostClaim {
        work: HostClaimWork,
        key: HostKey,
    },
    /// Workflow #427-A: a scheduled policy run the driver handed to the instance.
    PolicyRun(Box<super::policy_lanes::PolicyRunJob>),
    /// #503 ISO-A, tests: a payload of `instance` that does what `act` says
    /// (`RuntimeHost::post_worker_payload_for_test`).
    #[cfg(test)]
    Test {
        instance: InstanceId,
        act: failure_scope::WorkerActForTest,
    },
}

impl WorkerJob {
    fn failure_category(&self) -> &'static str {
        match self {
            Self::HostClaim { work, .. } => work.failure_category(),
            Self::PolicyRun(_) => HostLocation::RunHandedPolicyRun.as_str(),
            #[cfg(test)]
            Self::Test { .. } => HostLocation::RunInstanceWorker.as_str(),
        }
    }

    /// #503 ISO-A: the instance the payload is for, which its failure is contained against and
    /// whose two locks a panic of it is cleared on: its key's for a host claim, its lease's for
    /// a policy run.
    fn instance(&self) -> InstanceId {
        match self {
            Self::HostClaim { key, .. } => key.token.instance_id(),
            Self::PolicyRun(run) => run.context().lease_token().instance_id(),
            #[cfg(test)]
            Self::Test { instance, .. } => *instance,
        }
    }
}

/// The instance workers' channels and threads. Shutdown takes both: dropping the senders ends
/// every worker after its current payload, and the threads are joined before the host closes.
/// Workflow #437 MAPP-1b: the senders are keyed by the device key, one worker per device.
#[derive(Default)]
pub(super) struct InstanceWorkers {
    senders: BTreeMap<InstanceId, mpsc::Sender<WorkerJob>>,
    threads: Vec<JoinHandle<RuntimeHostResult<()>>>,
    /// #503 ISO-A, tests: by device key, the failure the next host claim of that device's worker
    /// ends with instead of its work (`RuntimeHost::fail_next_host_claim_for_test`).
    #[cfg(test)]
    failures_for_test: BTreeMap<InstanceId, RuntimeHostError>,
}

/// W-2: creates one worker per registered device. It runs at host start, before the
/// daemon-start preparation, so a preparation's ladder already has a worker. Workflow #437
/// MAPP-1b (`contracts/device-group.md`, "One queue, one worker, one lease per device"): the
/// workers are keyed by the device key; an instance alone on its device has its own worker,
/// `actingd-instance-<alias>`, as before, and a device group of several has one worker,
/// `actingd-device-<device_group>`, which takes every member's host claims and handed-off policy
/// runs, one at a time. A group of several whose name the host was not given cannot be named so
/// and stops the start (`instance_worker_spawn_failed`).
pub(super) fn spawn_instance_workers(shared: &Arc<HostShared>) -> RuntimeHostResult<()> {
    let instances = lock(
        &shared.registered_instances,
        HostLocation::ListWorkerInstances,
    )?
    .values()
    .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
    .collect::<Vec<_>>();
    let mut devices = BTreeMap::<InstanceId, Vec<String>>::new();
    for (instance_id, instance_alias) in instances {
        devices
            .entry(shared.device_key(instance_id))
            .or_default()
            .push(instance_alias);
    }
    let spawn_failed = || {
        RuntimeHostError::fatal(
            HostCode::InstanceWorkerSpawnFailed,
            HostLocation::StartRuntimeHost,
            RuntimeErrorCode::RuntimeHalted,
        )
    };
    let mut workers = lock(&shared.instance_workers, HostLocation::SpawnInstanceWorkers)?;
    for (device, aliases) in devices {
        let name = match aliases.as_slice() {
            [instance_alias] => format!("actingd-instance-{instance_alias}"),
            members => {
                let group = members
                    .iter()
                    .find_map(|alias| shared.device_group_names.get(alias))
                    .ok_or_else(spawn_failed)?;
                format!("actingd-device-{group}")
            }
        };
        let (sender, receiver) = mpsc::channel();
        let worker_shared = Arc::clone(shared);
        let thread = thread::Builder::new()
            .name(name)
            .spawn(move || instance_worker_loop(&worker_shared, &receiver))
            .map_err(|_| spawn_failed())?;
        workers.senders.insert(device, sender);
        workers.threads.push(thread);
    }
    Ok(())
}

/// W-2: one worker's thread root. A closed channel (shutdown) ends it normally; a panic follows
/// spec §7. #503 ISO-A: a payload's error goes to the one decision point against the payload's
/// instance; only a failure of the whole latches the Runtime and ends the worker, which every
/// other end leaves running (the module documentation).
fn instance_worker_loop(
    shared: &HostShared,
    receiver: &mpsc::Receiver<WorkerJob>,
) -> RuntimeHostResult<()> {
    // Spec §7: whether the worker was rebuilt after a panic and has not finished a payload since.
    let mut rebuilt = false;
    while let Ok(job) = receiver.recv() {
        let category = job.failure_category();
        let instance = job.instance();
        // Workflow #427-A: what a panicked policy run leaves behind (its registration).
        let lane = match &job {
            WorkerJob::PolicyRun(run) => Some(run.lane()),
            WorkerJob::HostClaim { .. } => None,
            #[cfg(test)]
            WorkerJob::Test { .. } => None,
        };
        // Workflow #489 AVAIL-1 fix round 1 (item 4): what a panicked probe of an unavailable
        // instance leaves behind (its instance's probe step, queued or running).
        let probe = match &job {
            WorkerJob::HostClaim {
                work: HostClaimWork::UnavailableProbe(pending),
                ..
            } => Some((pending.instance_id, pending.probe)),
            _ => None,
        };
        // Workflow #490 ENTRY-A fix round 1: a recovery ladder whose worker panics ended without
        // recovering, which an entry block it was the recovery of tells a person.
        let ladder = match &job {
            WorkerJob::HostClaim {
                work: HostClaimWork::RecoveryLadder(pending),
                ..
            } => Some(PendingRecoveryLadder::clone(pending)),
            _ => None,
        };
        // Fix 2 (re-review L-3): the lane is taken out first, so a failure to issue the failure
        // links (a global fault) still ends a policy run's registration, left abandoned for the
        // reconciliation, never working, and the Runtime is marked fatal.
        let links = match shared.host_claim_failure_links(&job) {
            Ok(links) => links,
            Err(error) => {
                let lane_ended = match lane {
                    Some(lane) => shared.drop_panicked_policy_run(lane),
                    None => Ok(()),
                };
                shared.fatal.mark(error.clone())?;
                lane_ended?;
                return Err(error);
            }
        };
        match catch_unwind(AssertUnwindSafe(|| shared.run_worker_job(job))) {
            Ok(Ok(())) => rebuilt = false,
            Ok(Err(error)) => {
                // #503 ISO-A (R-A1-1, R-A1-2): the payload's error is contained against its own
                // instance, and a payload that ended with an error is a finished one. Only a
                // failure of the whole (latched there) or of the containment's own records and
                // ends stops the Runtime, as a failed clean-up of a panic does.
                let ends = PayloadEnds {
                    lane,
                    probe,
                    ladder,
                };
                if let Err(error) = shared.contain_worker_failure(error, instance, links, ends) {
                    let error = error.into_fatal();
                    shared.fatal.mark(error.clone())?;
                    return Err(error);
                }
                rebuilt = false;
            }
            Err(panic) => {
                // #503 ISO-A (R-A1-3): first of all, before any record, the poison the unwind
                // left on the payload's own two locks is cleared, without blocking; whether
                // either was poisoned decides the fence below.
                let cleared = shared.clear_instance_lock_poison(instance);
                // The payload is dropped, not retried; its lease lapses at its TTL. A policy
                // run's registration stays as abandoned (its instance still occupied) until the
                // reconciliation settles the run once its lease ended, and the driver is woken.
                // Workflow #427-A fix 1 (review L2): before the panic's record, so a ledger
                // failure there cannot leave the registration working.
                if let Some(lane) = lane {
                    shared.drop_panicked_policy_run(lane)?;
                }
                // Workflow #489 AVAIL-1 fix round 2 (item 2): a panicked probe ends its
                // unavailability's probes and tells a person (`end_panicked_unavailable_probe`),
                // with the code the panic's own record carries. A failure of that clean-up is
                // kept, and returned only after the panic's own record and its fence, which it
                // never swallows.
                let panic_caught = actingcommand_contract::outcome::CodeStr::from(
                    actingcommand_contract::codes::ContractCode::PanicCaught.code(),
                );
                let probe_ended = probe.map_or(Ok(()), |(instance_id, probe)| {
                    shared.end_panicked_unavailable_probe(instance_id, probe, &panic_caught)
                });
                // Workflow #490 ENTRY-A fix round 1: a panicked recovery ladder ended without
                // recovering; an entry block it was a recovery of tells a person
                // (`note_entry_recovery_failed`). Its failure is kept as the probe's is.
                let ladder_ended = ladder.as_ref().map_or(Ok(()), |ladder| {
                    shared.note_entry_recovery_failed(
                        ladder,
                        crate::codes::EntryRecoveryEnd::WorkerPanicked,
                    )
                });
                let ended = probe_ended.and(ladder_ended);
                let words = WorkerPanic::of(panic.as_ref());
                shared.record_worker_panic(category, links.clone(), &words)?;
                // #503 ISO-A: after the panic's ends and its record, a lock of the instance the
                // panic left poisoned (R-A1-3), else a second panic before the rebuilt worker
                // finished a payload (R-A1-5, the owner's rule of 10-24: an instance that fails
                // twice in a row stops only that instance), fences the payload's instance; the
                // worker goes on.
                let fault = match &cleared {
                    Ok(true) => Some(RuntimeHostError::instance_fault(
                        HostCode::InstanceLockPoisoned,
                        HostLocation::RunInstanceWorker,
                        RuntimeErrorCode::RuntimeHalted,
                        instance,
                    )),
                    Ok(false) if rebuilt => Some(
                        RuntimeHostError::instance_fault(
                            actingcommand_contract::codes::ContractCode::PanicCaught,
                            HostLocation::RunInstanceWorker,
                            RuntimeErrorCode::RuntimeHalted,
                            instance,
                        )
                        .with_outcome_values(|outcome| words.values(outcome)),
                    ),
                    Ok(false) | Err(_) => None,
                };
                let fenced = fault.map_or(Ok(()), |fault| {
                    shared.fence_panicked_instance(fault, instance, links)
                });
                if let Err(error) = cleared.map(|_poisoned| ()).and(ended).and(fenced) {
                    let error = error.into_fatal();
                    shared.fatal.mark(error.clone())?;
                    return Err(error);
                }
                rebuilt = true;
            }
        }
    }
    Ok(())
}

/// #503 ISO-A (R-A1-2): what a payload its worker did not finish leaves to end, by its kind, as
/// a panic ends it: a policy run's lane, a probe's step of an unavailable instance, a recovery
/// ladder an entry block may wait for.
struct PayloadEnds {
    lane: Option<super::policy_lanes::PolicyLane>,
    probe: Option<(InstanceId, u32)>,
    ladder: Option<PendingRecoveryLadder>,
}

/// Spec §7: a worker's panic as its `panic_caught` carries it: the worker thread and the panic
/// payload, each as words only, bounded.
struct WorkerPanic {
    thread: String,
    raw_text: String,
}

impl WorkerPanic {
    fn of(panic: &(dyn std::any::Any + Send)) -> Self {
        let raw_text = panic
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-text panic payload".to_owned());
        let thread_name = thread::current()
            .name()
            .unwrap_or("actingd-instance")
            .to_owned();
        // A diagnostic message carries no path, control character or credential shape: the
        // panic text is kept as words only, bounded.
        let detail_text = |text: &str| {
            text.chars()
                .take(256)
                .map(|character| {
                    if character.is_ascii_alphanumeric()
                        || matches!(character, ' ' | '_' | '-' | '.')
                    {
                        character
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        };
        Self {
            thread: detail_text(&thread_name),
            raw_text: detail_text(&raw_text),
        }
    }

    /// `outcome` with the panic's values (ledger revision 3): the worker thread, the boundary,
    /// the raw source and the panic payload.
    fn values(
        &self,
        outcome: actingcommand_contract::outcome::Outcome,
    ) -> actingcommand_contract::outcome::Outcome {
        outcome
            .thread(self.thread.clone())
            .boundary(HostLocation::RunInstanceWorker)
            .raw_source(actingcommand_contract::outcome::RawSource::Panic)
            .raw_text(self.raw_text.clone())
    }
}

impl HostShared {
    /// One payload of an instance's worker.
    fn run_worker_job(&self, job: WorkerJob) -> RuntimeHostResult<()> {
        match job {
            WorkerJob::HostClaim { work, key } => self.run_host_claim(work, key),
            WorkerJob::PolicyRun(run) => self.run_handed_policy_run(*run),
            #[cfg(test)]
            WorkerJob::Test { instance, act } => self.act_for_test(instance, act),
        }
    }

    /// Runs one granted host claim on its instance's worker, as lifecycle work. A grant that
    /// arrives while the host is stopping releases at once; one that arrives while lifecycle
    /// admission is still closed (a daemon start that is preparing) waits for it, holding the
    /// key, as the old host-work thread kept its queue.
    fn run_host_claim(&self, work: HostClaimWork, key: HostKey) -> RuntimeHostResult<()> {
        #[cfg(test)]
        if self.worker_panic_for_test.swap(false, Ordering::AcqRel) {
            panic!("injected instance worker panic");
        }
        // #503 ISO-A, tests: the failure this device's next host claim ends with instead of its
        // work, its key released first, as a payload releases it at its end.
        #[cfg(test)]
        {
            let failure = lock(&self.instance_workers, HostLocation::PostHostClaim)?
                .failures_for_test
                .remove(&self.device_key(key.token.instance_id()));
            if let Some(error) = failure {
                self.release_host_key(&key)?;
                return Err(error);
            }
        }
        let _work = loop {
            if self.fatal.is_shutdown_requested() {
                return self.release_host_key(&key);
            }
            if let Some(admitted) = self.begin_continuation()? {
                break admitted;
            }
            thread::sleep(HOST_CLAIM_ADMISSION_POLL);
        };
        match work {
            HostClaimWork::StartupPackage(pending) => self.run_startup_claim(&pending, key),
            HostClaimWork::RecoveryLadder(pending) => self.run_recovery_ladder(&pending, key),
            HostClaimWork::LinkRestore(pending) => self.run_link_restore(&pending, key),
            HostClaimWork::Autostart(pending) => self.run_autostart_claim(&pending, key),
            HostClaimWork::FenceRecheck(pending) => self.run_fence_recheck(&pending, key),
            HostClaimWork::IdleAction(pending) => self.run_idle_action(&pending, key),
            HostClaimWork::HeldStartPreparation(pending) => {
                self.run_held_start_preparation(&pending, key)
            }
            HostClaimWork::UnavailableProbe(pending) => self.run_unavailable_probe(&pending, key),
        }
    }

    /// The links a worker's panic is recorded under: the instance and the payload's causation.
    /// #503 ISO-A: and the links a payload's contained failure is recorded and fenced under.
    fn host_claim_failure_links(&self, job: &WorkerJob) -> RuntimeHostResult<EventLinksDraft> {
        let work = match job {
            WorkerJob::HostClaim { work, .. } => work,
            // Workflow #427-A: a policy run's panic is recorded under its run's links.
            WorkerJob::PolicyRun(run) => return self.policy_run_event_links(run.context()),
            // #503 ISO-A, tests: a test payload's are its instance's.
            #[cfg(test)]
            WorkerJob::Test { instance, .. } => {
                return Ok(self
                    .events
                    .system_links()?
                    .with_instance_id(self.events.issuer().issue_registered_instance(*instance)));
            }
        };
        Ok(match work {
            HostClaimWork::StartupPackage(pending) => self
                .events
                .system_links()?
                .with_instance_id(
                    self.events
                        .issuer()
                        .issue_registered_instance(pending.instance_id),
                )
                .with_causation_id(pending.causation_id),
            HostClaimWork::RecoveryLadder(pending) => pending.links().clone(),
            HostClaimWork::LinkRestore(pending) => pending.links.clone(),
            HostClaimWork::Autostart(pending) => self.events.system_links()?.with_instance_id(
                self.events
                    .issuer()
                    .issue_registered_instance(pending.instance_id),
            ),
            HostClaimWork::FenceRecheck(pending) => pending.links.clone(),
            HostClaimWork::IdleAction(pending) => self.events.system_links()?.with_instance_id(
                self.events
                    .issuer()
                    .issue_registered_instance(pending.instance_id),
            ),
            HostClaimWork::HeldStartPreparation(pending) => pending.links.clone(),
            HostClaimWork::UnavailableProbe(pending) => pending.links.clone(),
        })
    }

    /// Spec §7: `panic_caught` (warning) under the payload's own failure category, with the
    /// worker thread, the boundary and the panic payload as raw text.
    fn record_worker_panic(
        &self,
        category: &'static str,
        links: EventLinksDraft,
        panic: &WorkerPanic,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            // Ledger revision 3: `panic_caught` is the record's outcome, with the worker thread,
            // the boundary and the panic payload (words only, bounded) as its values.
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                panic.values(actingcommand_contract::outcome::Outcome::new(
                    actingcommand_contract::codes::ContractCode::PanicCaught,
                )),
                DiagnosticDetailDraft::new(
                    category,
                    RuntimeLifecycleFailureStage::OperationCleanup.location(),
                    "runtime_host",
                    HostLocation::RunInstanceWorker.as_str(),
                ),
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// #503 ISO-A (R-A1-1, R-A1-2): `error`, the error a payload of `instance` ended with, at the
    /// one decision point against that instance, under the payload's `links`. A failure of the
    /// whole latches the Runtime and is the `Err`, as before. One of the instance fences it: the
    /// fence's own records say so. One of the operation is recorded once, a `runtime.failed` with
    /// the error's own code at stage `operation_cleanup` (nothing more when its path recorded it
    /// already). Then `ends` run as a panic runs them: a policy run's lane is ended and its sink
    /// told the run was dropped, a probe's unavailability tells a person with the error's code
    /// and probes no more, and a recovery ladder of an entry block is noted ended as a panicked
    /// worker's is (`worker_panicked`: nothing climbs again in that block). Any other `Err` is a
    /// failure of those records or ends, which its caller marks fatal.
    fn contain_worker_failure(
        &self,
        error: RuntimeHostError,
        instance: InstanceId,
        links: EventLinksDraft,
        ends: PayloadEnds,
    ) -> RuntimeHostResult<()> {
        let cause = error.outcome().code().clone();
        match self.contain_failure(error, Some(instance), links.clone())? {
            failure_scope::Contained::Latched(error) => return Err(error),
            failure_scope::Contained::Fenced(_) => {}
            failure_scope::Contained::Failed(error) => self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&error),
                links,
                None,
            )?,
        }
        if let Some(lane) = ends.lane {
            self.drop_panicked_policy_run(lane)?;
        }
        let probe_ended = ends.probe.map_or(Ok(()), |(instance_id, probe)| {
            self.end_panicked_unavailable_probe(instance_id, probe, &cause)
        });
        let ladder_ended = ends.ladder.as_ref().map_or(Ok(()), |ladder| {
            self.note_entry_recovery_failed(ladder, crate::codes::EntryRecoveryEnd::WorkerPanicked)
        });
        probe_ended.and(ladder_ended)
    }

    /// #503 ISO-A (R-A1-3, R-A1-5): fences `instance` after its worker's panic for `fault`, an
    /// `instance_fault` of the instance (`instance_lock_poisoned` or `panic_caught`), through the
    /// one decision point under the payload's `links`. `Err` when the fence latched instead (it
    /// failed, or no registered instance owns it).
    fn fence_panicked_instance(
        &self,
        fault: RuntimeHostError,
        instance: InstanceId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        match self.contain_failure(fault, Some(instance), links)? {
            failure_scope::Contained::Latched(error) => Err(error),
            failure_scope::Contained::Fenced(_) | failure_scope::Contained::Failed(_) => Ok(()),
        }
    }

    /// #503 ISO-A, tests: a test payload of `instance` doing `act`.
    #[cfg(test)]
    fn act_for_test(
        &self,
        instance: InstanceId,
        act: failure_scope::WorkerActForTest,
    ) -> RuntimeHostResult<()> {
        match act {
            failure_scope::WorkerActForTest::Finish(done) => {
                done.store(true, Ordering::Release);
                Ok(())
            }
            failure_scope::WorkerActForTest::Fail(error) => Err(error),
            failure_scope::WorkerActForTest::Panic => panic!("injected test payload panic"),
            failure_scope::WorkerActForTest::PanicHoldingInstanceLocks => {
                // As a device step holds them: the admission guard, then the queue-order lock.
                let guard = self
                    .instance_guard(instance)
                    .map_err(|failure| *failure.error)?;
                let order = self
                    .queue_order_lock(instance)
                    .map_err(|failure| *failure.error)?;
                let _admission = lock(&guard, HostLocation::LockInstanceAdmission)?;
                let _order = lock(&order, HostLocation::LockInstanceQueueOrder)?;
                panic!("injected test payload panic holding its instance's locks");
            }
            failure_scope::WorkerActForTest::PanicHoldingSharedLock => {
                let _scheduler = lock(&self.scheduler, HostLocation::ReadHostKey)?;
                panic!("injected test payload panic holding a shared lock");
            }
        }
    }

    /// W-2: posts a granted host claim's payload with its key to the instance's worker (Workflow
    /// #437 MAPP-1b: its device's). Only a stopping host has no worker left; its close releases
    /// the key.
    pub(super) fn post_host_claim(
        &self,
        instance_id: InstanceId,
        work: HostClaimWork,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let device = self.device_key(instance_id);
        let workers = lock(&self.instance_workers, HostLocation::PostHostClaim)?;
        let posted = workers
            .senders
            .get(&device)
            .is_some_and(|sender| sender.send(WorkerJob::HostClaim { work, key }).is_ok());
        if posted || self.fatal.is_shutdown_requested() {
            return Ok(());
        }
        Err(RuntimeHostError::fatal(
            HostCode::InstanceWorkerUnavailable,
            HostLocation::PostHostClaim,
            RuntimeErrorCode::RuntimeHalted,
        ))
    }

    /// Workflow #427-A: posts a handed-off policy run to the instance's worker (Workflow #437
    /// MAPP-1b: its device's). `Ok(Err(run))` gives the run back when no worker took it: the
    /// host is stopping (its workers' senders are gone) or the instance's worker ended.
    pub(super) fn post_policy_run(
        &self,
        instance_id: InstanceId,
        run: Box<super::policy_lanes::PolicyRunJob>,
    ) -> RuntimeHostResult<Result<(), Box<super::policy_lanes::PolicyRunJob>>> {
        let device = self.device_key(instance_id);
        let workers = lock(
            &self.instance_workers,
            HostLocation::HandOffScheduledPolicyRun,
        )?;
        let Some(sender) = workers.senders.get(&device) else {
            return Ok(Err(run));
        };
        match sender.send(WorkerJob::PolicyRun(run)) {
            Ok(()) => Ok(Ok(())),
            Err(mpsc::SendError(WorkerJob::PolicyRun(run))) => Ok(Err(run)),
            Err(mpsc::SendError(_)) => Err(RuntimeHostError::fatal(
                HostCode::InstanceWorkerUnavailable,
                HostLocation::HandOffScheduledPolicyRun,
                RuntimeErrorCode::RuntimeHalted,
            )),
        }
    }

    /// Registers `work` before its claim can be granted (Q-6), then requests the claim: a
    /// claim granted at once goes straight to the worker; a queued one goes there from the
    /// pump or the transfer that grants it.
    pub(super) fn request_host_claim_work(
        &self,
        claim: super::lease::HostClaim<'_>,
        work: HostClaimWork,
    ) -> Result<(), RequestFailure> {
        let request = claim.request.clone();
        let request_id = request.request_id();
        let connection_id = claim.connection_id;
        let instance_id = self.resolve_instance(claim.instance_alias)?.instance_id();
        lock(&self.host_claim_work, HostLocation::RegisterHostClaimWork)?.insert(request_id, work);
        match self.request_host_claim(claim) {
            Ok(super::lease::HostClaimAdmission::Granted(token)) => {
                let work = lock(&self.host_claim_work, HostLocation::TakeHostClaimWork)?
                    .remove(&request_id);
                if let Some(work) = work {
                    self.post_host_claim(
                        instance_id,
                        work,
                        HostKey {
                            request,
                            token,
                            connection_id,
                        },
                    )
                    .map_err(RequestFailure::poison_without_terminal)?;
                }
                Ok(())
            }
            Ok(super::lease::HostClaimAdmission::Queued { .. }) => Ok(()),
            Err(failure) => {
                lock(&self.host_claim_work, HostLocation::ForgetHostClaimWork)?.remove(&request_id);
                Err(failure)
            }
        }
    }

    /// W-2: the stopping host's worker shutdown, after the queues were cancelled: dropping the
    /// senders ends every worker after its current payload, and each thread is joined.
    pub(super) fn join_instance_workers(&self) -> RuntimeHostResult<()> {
        let threads = {
            let mut workers = lock(&self.instance_workers, HostLocation::StopInstanceWorkers)?;
            workers.senders.clear();
            std::mem::take(&mut workers.threads)
        };
        let mut failure = None;
        for thread in threads {
            if let Err(error) = join_runtime_thread(Some(thread), HostLocation::JoinInstanceWorker)
                && failure.is_none()
            {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Q-5: whether `key` is still its instance's unexpired lease, compared by lease id and
    /// holder id under the instance's queue-order lock.
    pub(super) fn holds_key(&self, key: &HostKey) -> RuntimeHostResult<bool> {
        let instance_id = key.token.instance_id();
        let order_lock = self
            .queue_order_lock(instance_id)
            .map_err(|failure| *failure.error)?;
        let _order = lock(&order_lock, HostLocation::LockInstanceQueueOrder)?;
        let now = self.monotonic_ms()?;
        Ok(lock(&self.scheduler, HostLocation::ReadHostKey)?
            .active_lease(instance_id)
            .is_some_and(|lease| {
                let token = lease.token();
                token.lease_id() == key.token.lease_id()
                    && token.holder_id() == key.token.holder_id()
                    && token.expires_at_monotonic_ms() > now
            }))
    }

    /// Q-5 (B1): renews `key` to `lease_ttl_ms` (capped like every claim TTL) when less than
    /// that remains, so a hold renews only before a step whose bound exceeds what is left. A key
    /// within `host_claims.renewal_slack_ms` (Workflow #387 D1c, the configuration in effect) of
    /// the bound is not renewed, so a claim granted a moment ago (its TTL already that bound)
    /// does not renew at once. A refusal (`lease_expired`, `lease_missing`) ends the hold at the
    /// caller.
    pub(super) fn renew_host_key(
        &self,
        key: &mut HostKey,
        lease_ttl_ms: u64,
    ) -> Result<(), RequestFailure> {
        let reserve = lock(&self.scheduler, HostLocation::ReadHostKeyTtlBound)?
            .config()
            .maximum_client_heartbeat_interval_ms;
        let lease_ttl_ms = lease_ttl_ms
            .min(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS.saturating_add(reserve));
        let renewal_slack_ms = self
            .renewal_slack_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        if key
            .token
            .expires_at_monotonic_ms()
            .saturating_sub(now)
            .saturating_add(renewal_slack_ms)
            >= lease_ttl_ms
        {
            return Ok(());
        }
        let validated = key.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::RenewHostKey,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        key.token =
            self.renew_held_lease(&validated, &key.token, key.connection_id, lease_ttl_ms)?;
        Ok(())
    }

    /// Releases `key` once when it is still its instance's lease, which hands the key on. A
    /// key that is already gone (a run released it, it lapsed) is left as it is.
    pub(super) fn release_host_key(&self, key: &HostKey) -> RuntimeHostResult<()> {
        let current = lock(&self.scheduler, HostLocation::ReadHostKeyRelease)?
            .active_lease(key.token.instance_id())
            .filter(|lease| {
                lease.token().lease_id() == key.token.lease_id()
                    && lease.token().holder_id() == key.token.holder_id()
                    && lease.connection_id() == key.connection_id
            })
            .map(|lease| lease.token().clone());
        let Some(token) = current else {
            return Ok(());
        };
        let validated = key.request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::ReleaseHostKey,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        match self.release_lease(
            &validated,
            key.request.request_id(),
            &token,
            key.connection_id,
            None,
        ) {
            Ok(_) => Ok(()),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&failure.error),
                self.events.request_links(
                    &validated,
                    Some(token.instance_id()),
                    Some(token.lease_id()),
                    None,
                ),
                None,
            ),
        }
    }
}

#[cfg(test)]
impl RuntimeHost {
    /// #503 ISO-A, tests: posts a payload doing `act` to the worker of `instance_alias`'s
    /// device, as a granted claim's payload is posted; `instance_worker_unavailable` when that
    /// worker takes no more payloads.
    pub(crate) fn post_worker_payload_for_test(
        &self,
        instance_alias: &str,
        act: failure_scope::WorkerActForTest,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref(
            crate::codes::test_spellings::TestLocation::InjectInstanceWorkerPanicForTest,
        )?;
        let instance = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let device = shared.device_key(instance);
        let workers = lock(&shared.instance_workers, HostLocation::PostHostClaim)?;
        let posted = workers
            .senders
            .get(&device)
            .is_some_and(|sender| sender.send(WorkerJob::Test { instance, act }).is_ok());
        if posted {
            return Ok(());
        }
        Err(RuntimeHostError::fatal(
            HostCode::InstanceWorkerUnavailable,
            HostLocation::PostHostClaim,
            RuntimeErrorCode::RuntimeHalted,
        ))
    }

    /// #503 ISO-A, tests: the next host claim the worker of `instance_alias`'s device takes ends
    /// with `error` instead of its work, after releasing its key.
    pub(crate) fn fail_next_host_claim_for_test(
        &self,
        instance_alias: &str,
        error: RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref(
            crate::codes::test_spellings::TestLocation::InjectInstanceWorkerPanicForTest,
        )?;
        let instance = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let device = shared.device_key(instance);
        lock(&shared.instance_workers, HostLocation::PostHostClaim)?
            .failures_for_test
            .insert(device, error);
        Ok(())
    }
}
