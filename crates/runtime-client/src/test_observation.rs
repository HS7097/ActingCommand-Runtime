// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    CorrelationId, HolderId, InstanceId, LeaseId, LeaseToken, RuntimeOperation, RuntimeReceipt,
    RuntimeRequest, RuntimeResult,
};
use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

#[cfg(test)]
use std::sync::MutexGuard;

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationStage {
    ProxyAcquireStart,
    ProxyAcquireResult,
    HeartbeatWaitResult,
    HeartbeatRenewStart,
    HeartbeatRenewResult,
    ProxyInputStart,
    ProxyInputResult,
    ProxyStopSend,
    HeartbeatJoinStart,
    HeartbeatJoinResult,
    ProxyReleaseStart,
    ProxyReleaseResult,
    ClientAcquireStart,
    ClientAcquireResult,
    ClientRenewStart,
    ClientRenewResult,
    ClientInputStart,
    ClientInputResult,
    ClientReleaseStart,
    ClientReleaseResult,
    ClientRequestCreated,
    ClientRequestWriteStart,
    ClientRequestWriteResult,
    ReceiptHeaderWait,
    ReceiptHeaderResult,
    ReceiptBodyWait,
    ReceiptBodyResult,
    ReceiptValidationResult,
    ClientTerminalResult,
    HostFrameReceived,
    HostDispatchStart,
    HostDispatchResult,
    HostReceiptWriteStart,
    HostReceiptWriteResult,
    HostConnectionExit,
    HostCleanupResult,
    BackendOpenStart,
    BackendOpenResult,
    BackendInputStart,
    BackendInputResult,
    BackendLongInputStart,
    BackendLongInputResult,
    BackendClose,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationOperation {
    Proxy,
    AcquireLease,
    RenewLease,
    Input,
    ReleaseLease,
    Connection,
    Backend,
    Other,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationThreadRole {
    Client,
    Heartbeat,
    Host,
    Backend,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationOutcome {
    Started,
    Success,
    Failure,
    Timeout,
    Stop,
    Closed,
    Shutdown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObservationIdentities {
    pub(crate) request: Option<u64>,
    pub(crate) correlation: Option<u64>,
    pub(crate) instance: Option<u64>,
    pub(crate) lease: Option<u64>,
    pub(crate) holder: Option<u64>,
    pub(crate) token: Option<u64>,
}

impl fmt::Debug for ObservationIdentities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentityOrdinals")
            .field("request", &self.request)
            .field("correlation", &self.correlation)
            .field("instance", &self.instance)
            .field("lease", &self.lease)
            .field("holder", &self.holder)
            .field("token", &self.token)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ObservationRecord {
    pub(crate) sequence: u64,
    pub(crate) elapsed_micros: u64,
    pub(crate) stage: ObservationStage,
    pub(crate) operation: ObservationOperation,
    pub(crate) thread_role: ObservationThreadRole,
    pub(crate) thread_name: String,
    pub(crate) outcome: ObservationOutcome,
    pub(crate) identities: ObservationIdentities,
}

impl fmt::Debug for ObservationRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TestObservation")
            .field("sequence", &self.sequence)
            .field("elapsed_micros", &self.elapsed_micros)
            .field("stage", &self.stage)
            .field("operation", &self.operation)
            .field("thread_role", &self.thread_role)
            .field("thread_name", &self.thread_name)
            .field("outcome", &self.outcome)
            .field("identities", &self.identities)
            .finish()
    }
}

#[derive(Default)]
struct IdentityClasses {
    request: Vec<(actingcommand_contract::RequestId, u64)>,
    correlation: Vec<(CorrelationId, u64)>,
    instance: Vec<(InstanceId, u64)>,
    lease: Vec<(LeaseId, u64)>,
    holder: Vec<(HolderId, u64)>,
    token: Vec<(LeaseToken, u64)>,
}

impl IdentityClasses {
    fn ordinals(
        &mut self,
        request: Option<&RuntimeRequest>,
        receipt: Option<&RuntimeReceipt>,
        token: Option<&LeaseToken>,
    ) -> ObservationIdentities {
        let mut ordinals = ObservationIdentities {
            request: None,
            correlation: None,
            instance: None,
            lease: None,
            holder: None,
            token: None,
        };
        if let Some(request) = request {
            ordinals.request = Some(ordinal(&mut self.request, &request.request_id()));
            ordinals.correlation = Some(ordinal(&mut self.correlation, &request.correlation_id()));
            match request.operation() {
                RuntimeOperation::AcquireLease { holder_id, .. } => {
                    ordinals.holder = Some(ordinal(&mut self.holder, holder_id));
                }
                RuntimeOperation::RenewLease { token }
                | RuntimeOperation::ReleaseLease { token }
                | RuntimeOperation::Input { token, .. } => {
                    self.add_token(&mut ordinals, token);
                }
                _ => {}
            }
        }
        if let Some(receipt) = receipt {
            ordinals.request = Some(ordinal(&mut self.request, &receipt.request_id()));
            ordinals.correlation = Some(ordinal(&mut self.correlation, &receipt.correlation_id()));
            match receipt.result() {
                Some(
                    RuntimeResult::LeaseGranted { token } | RuntimeResult::LeaseRenewed { token },
                ) => {
                    self.add_token(&mut ordinals, token);
                }
                Some(RuntimeResult::LeaseReleased {
                    instance_id,
                    lease_id,
                }) => {
                    ordinals.instance = Some(ordinal(&mut self.instance, instance_id));
                    ordinals.lease = Some(ordinal(&mut self.lease, lease_id));
                }
                _ => {}
            }
        }
        if let Some(token) = token {
            self.add_token(&mut ordinals, token);
        }
        ordinals
    }

    fn add_token(&mut self, ordinals: &mut ObservationIdentities, token: &LeaseToken) {
        ordinals.instance = Some(ordinal(&mut self.instance, &token.instance_id()));
        ordinals.lease = Some(ordinal(&mut self.lease, &token.lease_id()));
        ordinals.holder = Some(ordinal(&mut self.holder, &token.holder_id()));
        ordinals.token = Some(ordinal(&mut self.token, token));
    }
}

fn ordinal<T: Clone + PartialEq>(classes: &mut Vec<(T, u64)>, value: &T) -> u64 {
    if let Some((_, ordinal)) = classes.iter().find(|(candidate, _)| candidate == value) {
        return *ordinal;
    }
    let ordinal =
        u64::try_from(classes.len()).expect("test observation identity class count fits u64") + 1;
    classes.push((value.clone(), ordinal));
    ordinal
}

struct RecorderState {
    started: Instant,
    next_sequence: u64,
    identities: IdentityClasses,
    records: Vec<ObservationRecord>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestObservationOwner(u64);

pub(crate) struct TestObservationOwnerScope {
    previous: Option<TestObservationOwner>,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for TestObservationOwnerScope {
    fn drop(&mut self) {
        CURRENT_OBSERVATION_OWNER.set(self.previous);
    }
}

std::thread_local! {
    static CURRENT_OBSERVATION_OWNER: Cell<Option<TestObservationOwner>> = const { Cell::new(None) };
}

#[cfg(test)]
static NEXT_OBSERVATION_OWNER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn current_observation_owner() -> Option<TestObservationOwner> {
    CURRENT_OBSERVATION_OWNER.get()
}

pub(crate) fn enter_observation_owner(
    owner: Option<TestObservationOwner>,
) -> TestObservationOwnerScope {
    let previous = CURRENT_OBSERVATION_OWNER.replace(owner);
    TestObservationOwnerScope {
        previous,
        _not_send: PhantomData,
    }
}

#[cfg(test)]
fn mint_observation_owner() -> Result<TestObservationOwner, &'static str> {
    NEXT_OBSERVATION_OWNER
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .map(TestObservationOwner)
        .map_err(|_| "test_observation_owner_overflow")
}

#[derive(Clone)]
pub(crate) struct TestObservationRecorder {
    state: Arc<Mutex<RecorderState>>,
}

impl TestObservationRecorder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &self,
        stage: ObservationStage,
        operation: ObservationOperation,
        thread_role: ObservationThreadRole,
        outcome: ObservationOutcome,
        request: Option<&RuntimeRequest>,
        receipt: Option<&RuntimeReceipt>,
        token: Option<&LeaseToken>,
    ) {
        self.try_record(
            stage,
            operation,
            thread_role,
            outcome,
            request,
            receipt,
            token,
        )
        .unwrap_or_else(|error| panic!("test observation recorder failed: {error}"));
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_sanitized(
        &self,
        stage: ObservationStage,
        operation: ObservationOperation,
        thread_role: ObservationThreadRole,
        outcome: ObservationOutcome,
        identities: ObservationIdentities,
    ) {
        self.try_record_sanitized(stage, operation, thread_role, outcome, identities)
            .unwrap_or_else(|error| panic!("test observation recorder failed: {error}"));
    }

    #[allow(clippy::too_many_arguments)]
    fn try_record(
        &self,
        stage: ObservationStage,
        operation: ObservationOperation,
        thread_role: ObservationThreadRole,
        outcome: ObservationOutcome,
        request: Option<&RuntimeRequest>,
        receipt: Option<&RuntimeReceipt>,
        token: Option<&LeaseToken>,
    ) -> Result<(), &'static str> {
        let current = thread::current();
        let thread_name = current.name().unwrap_or("unnamed").to_owned();
        let mut state = self
            .state
            .lock()
            .map_err(|_| "test_observation_recorder_lock_poisoned")?;
        let operation = request
            .map(|request| operation_for(request.operation()))
            .unwrap_or(operation);
        let identities = state.identities.ordinals(request, receipt, token);
        push_record(
            &mut state,
            stage,
            operation,
            thread_role,
            thread_name,
            outcome,
            identities,
        )
    }

    #[cfg(test)]
    fn try_record_sanitized(
        &self,
        stage: ObservationStage,
        operation: ObservationOperation,
        thread_role: ObservationThreadRole,
        outcome: ObservationOutcome,
        identities: ObservationIdentities,
    ) -> Result<(), &'static str> {
        let current = thread::current();
        let thread_name = current.name().unwrap_or("unnamed").to_owned();
        let mut state = self
            .state
            .lock()
            .map_err(|_| "test_observation_recorder_lock_poisoned")?;
        push_record(
            &mut state,
            stage,
            operation,
            thread_role,
            thread_name,
            outcome,
            identities,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn push_record(
    state: &mut RecorderState,
    stage: ObservationStage,
    operation: ObservationOperation,
    thread_role: ObservationThreadRole,
    thread_name: String,
    outcome: ObservationOutcome,
    identities: ObservationIdentities,
) -> Result<(), &'static str> {
    state.next_sequence = state
        .next_sequence
        .checked_add(1)
        .ok_or("test_observation_sequence_overflow")?;
    let sequence = state.next_sequence;
    let elapsed_micros = u64::try_from(state.started.elapsed().as_micros())
        .map_err(|_| "test_observation_elapsed_overflow")?;
    state.records.push(ObservationRecord {
        sequence,
        elapsed_micros,
        stage,
        operation,
        thread_role,
        thread_name,
        outcome,
        identities,
    });
    Ok(())
}

#[cfg(test)]
static TEST_SERIAL: Mutex<()> = Mutex::new(());

struct ActiveRecorder {
    owner: TestObservationOwner,
    state: Arc<Mutex<RecorderState>>,
}

static ACTIVE_RECORDER: Mutex<Option<ActiveRecorder>> = Mutex::new(None);

#[cfg(test)]
pub(crate) struct TestObservationCapture {
    serial: Option<MutexGuard<'static, ()>>,
    owner_scope: Option<TestObservationOwnerScope>,
    recorder: TestObservationRecorder,
    finished: bool,
}

#[cfg(test)]
impl TestObservationCapture {
    pub(crate) fn start() -> Result<Self, &'static str> {
        let serial = TEST_SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let owner = mint_observation_owner()?;
        let owner_scope = enter_observation_owner(Some(owner));
        let state = Arc::new(Mutex::new(RecorderState {
            started: Instant::now(),
            next_sequence: 0,
            identities: IdentityClasses::default(),
            records: Vec::new(),
        }));
        let mut active = ACTIVE_RECORDER
            .lock()
            .map_err(|_| "test_observation_active_lock_poisoned")?;
        if active.is_some() {
            return Err("test_observation_recorder_already_active");
        }
        *active = Some(ActiveRecorder {
            owner,
            state: Arc::clone(&state),
        });
        drop(active);
        Ok(Self {
            serial: Some(serial),
            owner_scope: Some(owner_scope),
            recorder: TestObservationRecorder { state },
            finished: false,
        })
    }

    pub(crate) fn recorder(&self) -> TestObservationRecorder {
        self.recorder.clone()
    }

    pub(crate) fn finish(mut self) -> Result<Vec<ObservationRecord>, &'static str> {
        if let Err(error) = clear_active(&self.recorder) {
            self.finished = true;
            return Err(error);
        }
        let records = match self.recorder.state.lock() {
            Ok(state) => state.records.clone(),
            Err(poisoned) => {
                let records = poisoned.into_inner().records.clone();
                emit_trace(&records);
                self.finished = true;
                self.owner_scope.take();
                self.serial.take();
                return Err("test_observation_recorder_lock_poisoned");
            }
        };
        emit_trace(&records);
        self.finished = true;
        self.owner_scope.take();
        self.serial.take();
        Ok(records)
    }
}

#[cfg(test)]
impl Drop for TestObservationCapture {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let records = match self.recorder.state.lock() {
            Ok(state) => state.records.clone(),
            Err(poisoned) => poisoned.into_inner().records.clone(),
        };
        emit_trace(&records);
        let _ = clear_active(&self.recorder);
        self.owner_scope.take();
        self.serial.take();
        if !thread::panicking() {
            panic!("test observation capture dropped without a sealed trace");
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_active(
    stage: ObservationStage,
    operation: ObservationOperation,
    thread_role: ObservationThreadRole,
    outcome: ObservationOutcome,
    request: Option<&RuntimeRequest>,
    receipt: Option<&RuntimeReceipt>,
    token: Option<&LeaseToken>,
) {
    let Some(owner) = current_observation_owner() else {
        return;
    };
    let recorder = ACTIVE_RECORDER
        .lock()
        .unwrap_or_else(|_| panic!("test observation active lock poisoned"))
        .as_ref()
        .filter(|active| active.owner == owner)
        .map(|active| TestObservationRecorder {
            state: Arc::clone(&active.state),
        });
    if let Some(recorder) = recorder {
        recorder.record(
            stage,
            operation,
            thread_role,
            outcome,
            request,
            receipt,
            token,
        );
    }
}

#[cfg(test)]
fn clear_active(recorder: &TestObservationRecorder) -> Result<(), &'static str> {
    let mut active = ACTIVE_RECORDER
        .lock()
        .map_err(|_| "test_observation_active_lock_poisoned")?;
    let Some(installed) = active.as_ref() else {
        return Err("test_observation_recorder_not_active");
    };
    if installed.owner != current_observation_owner().ok_or("test_observation_owner_not_active")?
        || !Arc::ptr_eq(&installed.state, &recorder.state)
    {
        return Err("test_observation_recorder_identity_mismatch");
    }
    *active = None;
    Ok(())
}

#[cfg(test)]
fn emit_trace(records: &[ObservationRecord]) {
    eprintln!("[test-observation trace begin count={}]", records.len());
    for record in records {
        eprintln!("{record:?}");
    }
    eprintln!("[test-observation trace end]");
}

fn operation_for(operation: &RuntimeOperation) -> ObservationOperation {
    match operation {
        RuntimeOperation::AcquireLease { .. } => ObservationOperation::AcquireLease,
        RuntimeOperation::RenewLease { .. } => ObservationOperation::RenewLease,
        RuntimeOperation::Input { .. } => ObservationOperation::Input,
        RuntimeOperation::ReleaseLease { .. } => ObservationOperation::ReleaseLease,
        _ => ObservationOperation::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::IdentifierIssuer;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[test]
    fn recorder_survives_caught_panic_and_redacts_identity() {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let token = LeaseToken::new(
            *ids.mint_owner_epoch().expect("owner epoch").transport(),
            *ids.mint_lease_id().expect("lease id").transport(),
            *ids.mint_instance_id().expect("instance id").transport(),
            *ids.mint_holder_id().expect("holder id").transport(),
            1,
        )
        .expect("lease token");
        let raw_lease = format!("{:?}", token.lease_id());
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let capture = TestObservationCapture::start().expect("start observation");
            let recorder = capture.recorder();
            recorder.record(
                ObservationStage::ClientInputStart,
                ObservationOperation::Input,
                ObservationThreadRole::Client,
                ObservationOutcome::Started,
                None,
                None,
                Some(&token),
            );
            panic!("controlled recorder unwind");
        }));
        assert!(unwind.is_err());

        let serial = TEST_SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            ACTIVE_RECORDER
                .lock()
                .expect("active recorder lock")
                .is_none()
        );
        drop(serial);
        let capture = TestObservationCapture::start().expect("restart observation after unwind");
        capture.recorder().record(
            ObservationStage::ClientInputStart,
            ObservationOperation::Input,
            ObservationThreadRole::Client,
            ObservationOutcome::Started,
            None,
            None,
            Some(&token),
        );
        let records = capture.finish().expect("seal observation");
        assert_eq!(records.len(), 1);
        let rendered = format!("{:?}", records[0]);
        assert!(rendered.contains("lease: Some(1)"));
        assert!(!rendered.contains(&raw_lease));

        let capture = TestObservationCapture::start().expect("start recorder failure control");
        let recorder = capture.recorder();
        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let _state = recorder.state.lock().expect("recorder state lock");
            panic!("controlled recorder lock poison");
        }));
        assert!(poisoned.is_err());
        assert_eq!(
            capture
                .finish()
                .expect_err("recorder failure must be visible"),
            "test_observation_recorder_lock_poisoned"
        );
    }

    #[test]
    fn recorder_admits_only_the_exact_capture_owner() {
        let capture = TestObservationCapture::start().expect("start observation");
        let owner = current_observation_owner().expect("capture owner");

        thread::spawn(|| {
            record_active(
                ObservationStage::ClientInputStart,
                ObservationOperation::Input,
                ObservationThreadRole::Client,
                ObservationOutcome::Failure,
                None,
                None,
                None,
            );
        })
        .join()
        .expect("join non-owner observation thread");

        thread::spawn(move || {
            let _owner_scope = enter_observation_owner(Some(owner));
            record_active(
                ObservationStage::ClientInputResult,
                ObservationOperation::Input,
                ObservationThreadRole::Client,
                ObservationOutcome::Success,
                None,
                None,
                None,
            );
        })
        .join()
        .expect("join owner observation thread");

        let records = capture.finish().expect("seal observation");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].sequence, 1);
        assert_eq!(records[0].stage, ObservationStage::ClientInputResult);
        assert_eq!(records[0].outcome, ObservationOutcome::Success);
    }
}
