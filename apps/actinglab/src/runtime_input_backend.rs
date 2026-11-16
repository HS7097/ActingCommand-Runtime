// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{InputAction, InputFrameReference, LeaseToken, RuntimeReceipt};
use actingcommand_device::{DeviceError, DeviceErrorSeverity, DeviceResult};
use actingcommand_lab::LabInputPort;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError, RuntimeInputProxy};

/// Lab compatibility adapter for the Runtime's typed input proxy.
///
/// The resident Runtime remains the only owner of the real device backend.
pub(super) struct RuntimeInputBackend {
    lease: RuntimeInputLease,
}

enum RuntimeInputLease {
    /// The proxy acquires, renews and releases its own connection-scoped lease.
    Acquired(RuntimeInputProxy),
    /// Inputs run under the caller's lease on this connection. Its holder renews and
    /// releases it, so closing the port releases nothing.
    Held {
        client: RuntimeClient,
        token: LeaseToken,
        closed: bool,
    },
}

impl RuntimeInputBackend {
    pub(super) fn connect(client: RuntimeClient, instance_alias: &str) -> DeviceResult<Self> {
        RuntimeInputProxy::connect(client, instance_alias)
            .map(|proxy| Self {
                lease: RuntimeInputLease::Acquired(proxy),
            })
            .map_err(device_error)
    }

    pub(super) fn with_held_lease(client: RuntimeClient, token: LeaseToken) -> Self {
        Self {
            lease: RuntimeInputLease::Held {
                client,
                token,
                closed: false,
            },
        }
    }

    /// The lease step this port reports: its own acquisition or the caller's held lease.
    pub(super) fn lease_action(&self) -> &'static str {
        match self.lease {
            RuntimeInputLease::Acquired(_) => "lease_acquire",
            RuntimeInputLease::Held { .. } => "lease_held",
        }
    }

    pub(super) fn input_receipt(&mut self, action: InputAction) -> DeviceResult<RuntimeReceipt> {
        self.input_receipt_on(action, None)
    }

    /// Workflow #434 LW-1 (register §1.435): [`Self::input_receipt`] sent on `frame`, when given:
    /// the reference of the frame the Runtime committed for the capture the press was planned on.
    /// An instance whose input is the emulator's own (Nemu) presses a tap, a long tap or a swipe
    /// only on such a frame; every other input takes it and resolves it the same way.
    pub(super) fn input_receipt_on(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameReference>,
    ) -> DeviceResult<RuntimeReceipt> {
        match &mut self.lease {
            RuntimeInputLease::Acquired(proxy) => {
                proxy.input_with_frame(action, frame).map_err(device_error)
            }
            RuntimeInputLease::Held { closed: true, .. } => Err(DeviceError::fatal(
                "Runtime input port under the caller's lease is closed",
            )),
            RuntimeInputLease::Held { client, token, .. } => client
                .input_with_frame(token, action, frame)
                .map_err(device_error),
        }
    }

    fn execute(&mut self, action: InputAction) -> DeviceResult<()> {
        self.input_receipt(action).map(|_| ())
    }
}

impl LabInputPort for RuntimeInputBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.execute(InputAction::Tap { x, y })
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.execute(InputAction::LongTap { x, y, duration_ms })
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        self.execute(InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        })
    }

    fn key(&mut self, key: &str) -> DeviceResult<()> {
        self.execute(InputAction::Key {
            key: key.to_string(),
        })
    }

    fn text(&mut self, text: &str) -> DeviceResult<()> {
        self.execute(InputAction::Text {
            text: text.to_string(),
        })
    }

    fn close(&mut self) -> DeviceResult<()> {
        match &mut self.lease {
            RuntimeInputLease::Acquired(proxy) => proxy.close().map_err(device_error),
            RuntimeInputLease::Held { closed, .. } => {
                *closed = true;
                Ok(())
            }
        }
    }
}

fn device_error(error: RuntimeClientError) -> DeviceError {
    let severity = if error.is_fallback_eligible() {
        DeviceErrorSeverity::Transient
    } else {
        DeviceErrorSeverity::Fatal
    };
    DeviceError::with_severity(severity, error.to_string())
}
