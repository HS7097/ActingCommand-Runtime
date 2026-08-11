// SPDX-License-Identifier: AGPL-3.0-only

//! Test-only observation sink for RuntimeHost request boundaries.
//!
//! This module is intentionally available only through the non-default
//! `test-observation` feature. It owns no production state or behavior.

use actingcommand_contract::{RuntimeReceipt, RuntimeRequest};
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

#[derive(Clone)]
pub struct HostTestObservation {
    point: HostTestObservationPoint,
    outcome: HostTestObservationOutcome,
    request: Option<RuntimeRequest>,
    receipt: Option<RuntimeReceipt>,
}

impl HostTestObservation {
    pub(crate) fn request(
        point: HostTestObservationPoint,
        outcome: HostTestObservationOutcome,
        request: &RuntimeRequest,
    ) -> Self {
        Self {
            point,
            outcome,
            request: Some(request.clone()),
            receipt: None,
        }
    }

    pub(crate) fn receipt(
        point: HostTestObservationPoint,
        outcome: HostTestObservationOutcome,
        request: &RuntimeRequest,
        receipt: &RuntimeReceipt,
    ) -> Self {
        Self {
            point,
            outcome,
            request: Some(request.clone()),
            receipt: Some(receipt.clone()),
        }
    }

    pub(crate) fn connection(
        point: HostTestObservationPoint,
        outcome: HostTestObservationOutcome,
    ) -> Self {
        Self {
            point,
            outcome,
            request: None,
            receipt: None,
        }
    }

    pub const fn point(&self) -> HostTestObservationPoint {
        self.point
    }

    pub const fn outcome(&self) -> HostTestObservationOutcome {
        self.outcome
    }

    pub const fn request_value(&self) -> Option<&RuntimeRequest> {
        self.request.as_ref()
    }

    pub const fn receipt_value(&self) -> Option<&RuntimeReceipt> {
        self.receipt.as_ref()
    }
}

type ObservationSink = Arc<dyn Fn(HostTestObservation) + Send + Sync + 'static>;

static INSTALL_LOCK: Mutex<()> = Mutex::new(());
static ACTIVE_SINK: OnceLock<Mutex<Option<ObservationSink>>> = OnceLock::new();

pub struct HostTestObservationGuard {
    _serial: MutexGuard<'static, ()>,
}

pub fn install(sink: ObservationSink) -> Result<HostTestObservationGuard, &'static str> {
    let serial = INSTALL_LOCK
        .lock()
        .map_err(|_| "host_test_observation_install_lock_poisoned")?;
    let mut active = active_sink()
        .lock()
        .map_err(|_| "host_test_observation_sink_lock_poisoned")?;
    if active.is_some() {
        return Err("host_test_observation_sink_already_installed");
    }
    *active = Some(sink);
    drop(active);
    Ok(HostTestObservationGuard { _serial: serial })
}

impl Drop for HostTestObservationGuard {
    fn drop(&mut self) {
        match active_sink().lock() {
            Ok(mut active) => *active = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }
}

pub(crate) fn emit(event: HostTestObservation) {
    let sink = active_sink()
        .lock()
        .unwrap_or_else(|_| panic!("host test observation sink lock poisoned"))
        .clone();
    if let Some(sink) = sink {
        sink(event);
    }
}

fn active_sink() -> &'static Mutex<Option<ObservationSink>> {
    ACTIVE_SINK.get_or_init(|| Mutex::new(None))
}
