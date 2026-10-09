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
//! and the worker carries on with fresh state. A second panic before the rebuilt worker
//! finished one payload marks the Runtime fatal with `runtime_restart_required`.

use super::recovery_ladder::PendingRecoveryLadder;
use super::startup_package::PendingStartupPackage;
use super::*;
use crate::codes::HostCode;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;

/// How often a granted claim retries a lifecycle admission that is still closed (a daemon
/// start that is still preparing, an install hold) before its payload runs.
const HOST_CLAIM_ADMISSION_POLL: Duration = Duration::from_millis(100);

/// What a granted host claim runs on its instance's worker.
pub(super) enum HostClaimWork {
    StartupPackage(PendingStartupPackage),
    RecoveryLadder(Box<PendingRecoveryLadder>),
}

impl HostClaimWork {
    const fn failure_category(&self) -> &'static str {
        match self {
            Self::StartupPackage(_) => "startup_package",
            Self::RecoveryLadder(_) => "recovery_ladder",
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

pub(super) struct WorkerJob {
    work: HostClaimWork,
    key: HostKey,
}

/// The instance workers' channels and threads. Shutdown takes both: dropping the senders ends
/// every worker after its current payload, and the threads are joined before the host closes.
#[derive(Default)]
pub(super) struct InstanceWorkers {
    senders: BTreeMap<InstanceId, mpsc::Sender<WorkerJob>>,
    threads: Vec<JoinHandle<RuntimeHostResult<()>>>,
}

/// W-2: creates one worker per registered instance. It runs at host start, before the
/// daemon-start preparation, so a preparation's ladder already has a worker.
pub(super) fn spawn_instance_workers(shared: &Arc<HostShared>) -> RuntimeHostResult<()> {
    let instances = lock(&shared.registered_instances, "list_worker_instances")?
        .values()
        .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
        .collect::<Vec<_>>();
    let mut workers = lock(&shared.instance_workers, "spawn_instance_workers")?;
    for (instance_id, instance_alias) in instances {
        let (sender, receiver) = mpsc::channel();
        let worker_shared = Arc::clone(shared);
        let thread = thread::Builder::new()
            .name(format!("actingd-instance-{instance_alias}"))
            .spawn(move || instance_worker_loop(&worker_shared, &receiver))
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "instance_worker_spawn_failed",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        workers.senders.insert(instance_id, sender);
        workers.threads.push(thread);
    }
    Ok(())
}

/// W-2: one worker's thread root. A closed channel (shutdown) ends it normally; a fatal
/// payload error marks the Runtime fatal and ends it; a panic follows spec §7.
fn instance_worker_loop(
    shared: &HostShared,
    receiver: &mpsc::Receiver<WorkerJob>,
) -> RuntimeHostResult<()> {
    // Spec §7: whether the worker was rebuilt after a panic and has not finished a payload since.
    let mut rebuilt = false;
    while let Ok(job) = receiver.recv() {
        let category = job.work.failure_category();
        let links = shared.host_claim_failure_links(&job)?;
        match catch_unwind(AssertUnwindSafe(|| shared.run_host_claim(job))) {
            Ok(Ok(())) => rebuilt = false,
            Ok(Err(error)) => {
                shared.fatal.mark(error.clone())?;
                return Err(error);
            }
            Err(panic) => {
                shared.record_worker_panic(category, links, panic.as_ref())?;
                if rebuilt {
                    let error = RuntimeHostError::fatal(
                        HostCode::RuntimeRestartRequired.as_str(),
                        "run_instance_worker",
                        RuntimeErrorCode::RuntimeFatal,
                    );
                    shared.fatal.mark(error.clone())?;
                    return Err(error);
                }
                rebuilt = true;
            }
        }
    }
    Ok(())
}

impl HostShared {
    /// Runs one granted host claim on its instance's worker, as lifecycle work. A grant that
    /// arrives while the host is stopping releases at once; one that arrives while lifecycle
    /// admission is still closed (a daemon start that is preparing) waits for it, holding the
    /// key, as the old host-work thread kept its queue.
    fn run_host_claim(&self, job: WorkerJob) -> RuntimeHostResult<()> {
        #[cfg(test)]
        if self.worker_panic_for_test.swap(false, Ordering::AcqRel) {
            panic!("injected instance worker panic");
        }
        let WorkerJob { work, key } = job;
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
        }
    }

    /// The links a worker's panic is recorded under: the instance and the payload's causation.
    fn host_claim_failure_links(&self, job: &WorkerJob) -> RuntimeHostResult<EventLinksDraft> {
        Ok(match &job.work {
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
        })
    }

    /// Spec §7: `panic_caught` (warning) under the payload's own failure category, with the
    /// worker thread, the boundary and the panic payload as raw text.
    fn record_worker_panic(
        &self,
        category: &'static str,
        links: EventLinksDraft,
        panic: &(dyn std::any::Any + Send),
    ) -> RuntimeHostResult<()> {
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
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                DiagnosticDetailDraft::new(
                    category,
                    RuntimeLifecycleFailureStage::OperationCleanup.as_str(),
                    "runtime_host",
                    "run_instance_worker",
                    format!(
                        "code={} thread={} boundary=instance_worker raw_source=panic raw_text={}",
                        actingcommand_contract::codes::ContractCode::PanicCaught.as_str(),
                        detail_text(&thread_name),
                        detail_text(&raw_text)
                    ),
                    Sensitivity::Internal,
                ),
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// W-2: posts a granted host claim's payload with its key to the instance's worker. Only a
    /// stopping host has no worker left; its close releases the key.
    pub(super) fn post_host_claim(
        &self,
        instance_id: InstanceId,
        work: HostClaimWork,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let workers = lock(&self.instance_workers, "post_host_claim")?;
        let posted = workers
            .senders
            .get(&instance_id)
            .is_some_and(|sender| sender.send(WorkerJob { work, key }).is_ok());
        if posted || self.fatal.is_shutdown_requested() {
            return Ok(());
        }
        Err(RuntimeHostError::fatal(
            "instance_worker_unavailable",
            "post_host_claim",
            RuntimeErrorCode::RuntimeFatal,
        ))
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
        lock(&self.host_claim_work, "register_host_claim_work")?.insert(request_id, work);
        match self.request_host_claim(claim) {
            Ok(super::lease::HostClaimAdmission::Granted(token)) => {
                let work = lock(&self.host_claim_work, "take_host_claim_work")?.remove(&request_id);
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
                lock(&self.host_claim_work, "forget_host_claim_work")?.remove(&request_id);
                Err(failure)
            }
        }
    }

    /// W-2: the stopping host's worker shutdown, after the queues were cancelled: dropping the
    /// senders ends every worker after its current payload, and each thread is joined.
    pub(super) fn join_instance_workers(&self) -> RuntimeHostResult<()> {
        let threads = {
            let mut workers = lock(&self.instance_workers, "stop_instance_workers")?;
            workers.senders.clear();
            std::mem::take(&mut workers.threads)
        };
        let mut failure = None;
        for thread in threads {
            if let Err(error) = join_runtime_thread(Some(thread), "join_instance_worker")
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
        let _order = lock(&order_lock, "lock_instance_queue_order")?;
        let now = self.monotonic_ms()?;
        Ok(lock(&self.scheduler, "read_host_key")?
            .active_lease(instance_id)
            .is_some_and(|lease| {
                let token = lease.token();
                token.lease_id() == key.token.lease_id()
                    && token.holder_id() == key.token.holder_id()
                    && token.expires_at_monotonic_ms() > now
            }))
    }

    /// Q-5 (B1): renews `key` to `lease_ttl_ms` (capped like every claim TTL) when less than
    /// that remains, so a hold renews only before a step whose bound exceeds what is left. A
    /// refusal (`lease_expired`, `lease_missing`) ends the hold at the caller.
    pub(super) fn renew_host_key(
        &self,
        key: &mut HostKey,
        lease_ttl_ms: u64,
    ) -> Result<(), RequestFailure> {
        let reserve = lock(&self.scheduler, "read_host_key_ttl_bound")?
            .config()
            .maximum_client_heartbeat_interval_ms;
        let lease_ttl_ms = lease_ttl_ms
            .min(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS.saturating_add(reserve));
        let now = self
            .monotonic_ms()
            .map_err(RequestFailure::poison_without_terminal)?;
        if key.token.expires_at_monotonic_ms().saturating_sub(now) >= lease_ttl_ms {
            return Ok(());
        }
        let validated = key.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "host_key_request_invalid",
                "renew_host_key",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        key.token =
            self.renew_held_lease(&validated, &key.token, key.connection_id, lease_ttl_ms)?;
        Ok(())
    }

    /// Releases `key` once when it is still its instance's lease, which hands the key on. A
    /// key that is already gone (a run released it, it lapsed) is left as it is.
    pub(super) fn release_host_key(&self, key: &HostKey) -> RuntimeHostResult<()> {
        let current = lock(&self.scheduler, "read_host_key_release")?
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
                "host_key_request_invalid",
                "release_host_key",
                RuntimeErrorCode::RuntimeFatal,
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
