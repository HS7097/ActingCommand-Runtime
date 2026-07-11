// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::InputAction;
use actingcommand_device::{DeviceError, DeviceErrorSeverity, DeviceResult, InputBackend};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError, RuntimeInputProxy};

/// Lab compatibility adapter for the Runtime's typed input proxy.
///
/// The resident Runtime remains the only owner of the real device backend.
pub(super) struct RuntimeInputBackend {
    proxy: RuntimeInputProxy,
}

impl RuntimeInputBackend {
    pub(super) fn connect(client: RuntimeClient, instance_alias: &str) -> DeviceResult<Self> {
        RuntimeInputProxy::connect(client, instance_alias)
            .map(|proxy| Self { proxy })
            .map_err(device_error)
    }

    fn execute(&mut self, action: InputAction) -> DeviceResult<()> {
        self.proxy.input(action).map_err(device_error)
    }
}

impl InputBackend for RuntimeInputBackend {
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

    fn reset(&mut self) -> DeviceResult<()> {
        self.execute(InputAction::Reset)
    }

    fn close(&mut self) -> DeviceResult<()> {
        self.proxy.close().map_err(device_error)
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
