// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeClient, RuntimeClientError};
use actingcommand_contract::{InputAction, LeaseToken};
use actingcommand_device::{DeviceError, DeviceErrorSeverity, DeviceResult, InputBackend};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const DEFAULT_RUNTIME_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(2_500);
pub const MAX_RUNTIME_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(5_000);

struct ProxyState {
    token: LeaseToken,
    heartbeat_failure: Option<RuntimeClientError>,
}

/// `InputBackend` adapter that keeps device authority inside the resident Runtime.
pub struct RuntimeInputProxy {
    client: RuntimeClient,
    state: Arc<Mutex<ProxyState>>,
    stop: Option<SyncSender<()>>,
    heartbeat: Option<JoinHandle<()>>,
    closed: bool,
}

impl RuntimeInputProxy {
    pub fn connect(client: RuntimeClient, instance_alias: &str) -> DeviceResult<Self> {
        Self::connect_with_heartbeat(client, instance_alias, DEFAULT_RUNTIME_HEARTBEAT_INTERVAL)
    }

    pub fn connect_with_heartbeat(
        client: RuntimeClient,
        instance_alias: &str,
        heartbeat_interval: Duration,
    ) -> DeviceResult<Self> {
        if heartbeat_interval.is_zero() || heartbeat_interval > MAX_RUNTIME_HEARTBEAT_INTERVAL {
            return Err(DeviceError::fatal(
                "RuntimeInputProxy heartbeat interval must be within 1..=5000 ms",
            ));
        }
        let token = client.acquire_lease(instance_alias).map_err(device_error)?;
        let state = Arc::new(Mutex::new(ProxyState {
            token,
            heartbeat_failure: None,
        }));
        let (stop, receiver) = mpsc::sync_channel(1);
        let thread_state = Arc::clone(&state);
        let thread_client = client.clone();
        let heartbeat = thread::Builder::new()
            .name("actingcommand-runtime-heartbeat".to_string())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    heartbeat_loop(&thread_client, &thread_state, &receiver, heartbeat_interval)
                }));
                if result.is_err() {
                    let mut state = match thread_state.lock() {
                        Ok(state) => state,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    state.heartbeat_failure = Some(RuntimeClientError::fatal(
                        "runtime_heartbeat_panicked",
                        "renew_runtime_lease",
                    ));
                }
            });
        let heartbeat = match heartbeat {
            Ok(heartbeat) => heartbeat,
            Err(_) => {
                let release = client.release_lease(&lock_proxy(&state)?.token);
                return Err(match release {
                    Ok(()) => DeviceError::fatal("RuntimeInputProxy heartbeat thread failed"),
                    Err(error) => DeviceError::fatal(format!(
                        "RuntimeInputProxy heartbeat thread failed; lease release also failed: {error}"
                    )),
                });
            }
        };
        Ok(Self {
            client,
            state,
            stop: Some(stop),
            heartbeat: Some(heartbeat),
            closed: false,
        })
    }

    fn execute(&mut self, action: InputAction) -> DeviceResult<()> {
        if self.closed {
            return Err(DeviceError::fatal("RuntimeInputProxy is closed"));
        }
        let heartbeat_finished = self.heartbeat.as_ref().is_none_or(JoinHandle::is_finished);
        let state = lock_proxy(&self.state)?;
        if let Some(error) = &state.heartbeat_failure {
            return Err(device_error(error.clone()));
        }
        if heartbeat_finished {
            return Err(DeviceError::fatal(
                "RuntimeInputProxy heartbeat stopped unexpectedly",
            ));
        }
        self.client
            .input(&state.token, action)
            .map_err(device_error)
    }

    fn shutdown(&mut self) -> DeviceResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let stop_failed = self.stop.take().is_some_and(|stop| stop.send(()).is_err());
        let heartbeat_panicked = self
            .heartbeat
            .take()
            .is_some_and(|heartbeat| heartbeat.join().is_err());
        let state = lock_proxy(&self.state);
        let mut errors = Vec::new();
        match state {
            Ok(state) => {
                if let Some(error) = &state.heartbeat_failure {
                    errors.push(error.to_string());
                }
                if let Err(error) = self.client.release_lease(&state.token) {
                    errors.push(error.to_string());
                }
            }
            Err(error) => errors.push(error.to_string()),
        }
        if stop_failed && errors.is_empty() {
            errors.push("RuntimeInputProxy heartbeat stop channel failed".to_string());
        }
        if heartbeat_panicked {
            errors.push("RuntimeInputProxy heartbeat thread panicked".to_string());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(DeviceError::fatal(errors.join("; ")))
        }
    }
}

impl InputBackend for RuntimeInputProxy {
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
        self.shutdown()
    }
}

impl Drop for RuntimeInputProxy {
    fn drop(&mut self) {
        if self.closed || thread::panicking() {
            return;
        }
        if let Err(error) = self.shutdown() {
            panic!("{error}");
        }
    }
}

fn heartbeat_loop(
    client: &RuntimeClient,
    state: &Mutex<ProxyState>,
    stop: &Receiver<()>,
    heartbeat_interval: Duration,
) {
    loop {
        match stop.recv_timeout(heartbeat_interval) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                let mut state = match state.lock() {
                    Ok(state) => state,
                    Err(_) => return,
                };
                match client.renew_lease(&state.token) {
                    Ok(token) => state.token = token,
                    Err(error) => {
                        state.heartbeat_failure = Some(error);
                        return;
                    }
                }
            }
        }
    }
}

fn lock_proxy(state: &Mutex<ProxyState>) -> DeviceResult<MutexGuard<'_, ProxyState>> {
    state
        .lock()
        .map_err(|_| DeviceError::fatal("RuntimeInputProxy state is poisoned"))
}

fn device_error(error: RuntimeClientError) -> DeviceError {
    let severity = if error.is_fatal() {
        DeviceErrorSeverity::Fatal
    } else {
        DeviceErrorSeverity::Transient
    };
    DeviceError::with_severity(severity, error.to_string())
}
