// SPDX-License-Identifier: AGPL-3.0-only

//! Test-only observation sink for RuntimeHost request boundaries.
//!
//! This module is intentionally available only through the non-default
//! `test-observation` feature. It owns no production state or behavior. Raw
//! Runtime requests and receipts are reduced to equality ordinals before an
//! installed sink receives an event.

use actingcommand_contract::{
    CorrelationId, HolderId, InstanceId, LeaseId, LeaseToken, RuntimeOperation, RuntimeReceipt,
    RuntimeRequest, RuntimeResult,
};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostTestObservationPoint {
    FrameReceived,
    DispatchStart,
    DispatchResult,
    ReceiptWriteStart,
    ReceiptWriteResult,
    ConnectionExit,
    ConnectionCleanupResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostTestObservationOutcome {
    Started,
    Success,
    Error,
    Closed,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostTestObservationOperation {
    AcquireLease,
    RenewLease,
    Input,
    ReleaseLease,
    Connection,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostTestObservationIdentityOrdinals {
    request: Option<u64>,
    correlation: Option<u64>,
    instance: Option<u64>,
    lease: Option<u64>,
    holder: Option<u64>,
    token: Option<u64>,
}

impl HostTestObservationIdentityOrdinals {
    pub const fn request(self) -> Option<u64> {
        self.request
    }

    pub const fn correlation(self) -> Option<u64> {
        self.correlation
    }

    pub const fn instance(self) -> Option<u64> {
        self.instance
    }

    pub const fn lease(self) -> Option<u64> {
        self.lease
    }

    pub const fn holder(self) -> Option<u64> {
        self.holder
    }

    pub const fn token(self) -> Option<u64> {
        self.token
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostTestObservation {
    point: HostTestObservationPoint,
    outcome: HostTestObservationOutcome,
    operation: HostTestObservationOperation,
    identities: HostTestObservationIdentityOrdinals,
}

impl HostTestObservation {
    pub const fn point(self) -> HostTestObservationPoint {
        self.point
    }

    pub const fn outcome(self) -> HostTestObservationOutcome {
        self.outcome
    }

    pub const fn operation(self) -> HostTestObservationOperation {
        self.operation
    }

    pub const fn identities(self) -> HostTestObservationIdentityOrdinals {
        self.identities
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
    ) -> HostTestObservationIdentityOrdinals {
        let mut ordinals = HostTestObservationIdentityOrdinals {
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
        ordinals
    }

    fn add_token(
        &mut self,
        ordinals: &mut HostTestObservationIdentityOrdinals,
        token: &LeaseToken,
    ) {
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
        u64::try_from(classes.len()).expect("host test observation identity count fits u64") + 1;
    classes.push((value.clone(), ordinal));
    ordinal
}

type ObservationSink = Arc<dyn Fn(HostTestObservation) + Send + Sync + 'static>;

struct ActiveObservation {
    sink: ObservationSink,
    identities: IdentityClasses,
}

static INSTALL_LOCK: Mutex<()> = Mutex::new(());
static ACTIVE_OBSERVATION: OnceLock<Mutex<Option<ActiveObservation>>> = OnceLock::new();

pub struct HostTestObservationGuard {
    _serial: MutexGuard<'static, ()>,
}

pub fn install(sink: ObservationSink) -> Result<HostTestObservationGuard, &'static str> {
    let serial = INSTALL_LOCK
        .lock()
        .map_err(|_| "host_test_observation_install_lock_poisoned")?;
    let mut active = active_observation()
        .lock()
        .map_err(|_| "host_test_observation_sink_lock_poisoned")?;
    if active.is_some() {
        return Err("host_test_observation_sink_already_installed");
    }
    *active = Some(ActiveObservation {
        sink,
        identities: IdentityClasses::default(),
    });
    drop(active);
    Ok(HostTestObservationGuard { _serial: serial })
}

impl Drop for HostTestObservationGuard {
    fn drop(&mut self) {
        match active_observation().lock() {
            Ok(mut active) => *active = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }
}

pub(crate) fn emit_request(
    point: HostTestObservationPoint,
    outcome: HostTestObservationOutcome,
    request: &RuntimeRequest,
) {
    emit_sanitized(point, outcome, Some(request), None);
}

pub(crate) fn emit_receipt(
    point: HostTestObservationPoint,
    outcome: HostTestObservationOutcome,
    request: &RuntimeRequest,
    receipt: &RuntimeReceipt,
) {
    emit_sanitized(point, outcome, Some(request), Some(receipt));
}

pub(crate) fn emit_connection(
    point: HostTestObservationPoint,
    outcome: HostTestObservationOutcome,
) {
    emit_sanitized(point, outcome, None, None);
}

fn emit_sanitized(
    point: HostTestObservationPoint,
    outcome: HostTestObservationOutcome,
    request: Option<&RuntimeRequest>,
    receipt: Option<&RuntimeReceipt>,
) {
    let event = {
        let mut active = active_observation()
            .lock()
            .unwrap_or_else(|_| panic!("host test observation sink lock poisoned"));
        let Some(active) = active.as_mut() else {
            return;
        };
        let operation = request
            .map(|request| operation_for(request.operation()))
            .unwrap_or(HostTestObservationOperation::Connection);
        let identities = active.identities.ordinals(request, receipt);
        (
            Arc::clone(&active.sink),
            HostTestObservation {
                point,
                outcome,
                operation,
                identities,
            },
        )
    };
    (event.0)(event.1);
}

fn active_observation() -> &'static Mutex<Option<ActiveObservation>> {
    ACTIVE_OBSERVATION.get_or_init(|| Mutex::new(None))
}

fn operation_for(operation: &RuntimeOperation) -> HostTestObservationOperation {
    match operation {
        RuntimeOperation::AcquireLease { .. } => HostTestObservationOperation::AcquireLease,
        RuntimeOperation::RenewLease { .. } => HostTestObservationOperation::RenewLease,
        RuntimeOperation::Input { .. } => HostTestObservationOperation::Input,
        RuntimeOperation::ReleaseLease { .. } => HostTestObservationOperation::ReleaseLease,
        _ => HostTestObservationOperation::Other,
    }
}
