// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeClient, RuntimeClientError, RuntimeClientResult, RuntimeDebugSession};
use actingcommand_contract::{InputAction, LeaseToken};
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

#[derive(Clone)]
enum RuntimeInputAuthority {
    Client(RuntimeClient),
    Debug(RuntimeDebugSession),
}

impl RuntimeInputAuthority {
    fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken> {
        match self {
            Self::Client(client) => client.acquire_lease(instance_alias),
            Self::Debug(session) => session.acquire_lease(instance_alias),
        }
    }

    fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken> {
        match self {
            Self::Client(client) => client.renew_lease(token),
            Self::Debug(session) => session.renew_lease(token),
        }
    }

    fn input(&self, token: &LeaseToken, action: InputAction) -> RuntimeClientResult<()> {
        match self {
            Self::Client(client) => client.input(token, action),
            Self::Debug(session) => session.input(token, action).map(|_| ()),
        }
    }

    fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()> {
        match self {
            Self::Client(client) => client.release_lease(token),
            Self::Debug(session) => session.release_lease(token),
        }
    }
}

/// Connection-scoped write lease that sends typed input commands to the resident Runtime.
///
/// This proxy owns no device backend. Device-specific adapters belong outside runtime-client.
pub struct RuntimeInputProxy {
    authority: RuntimeInputAuthority,
    state: Arc<Mutex<ProxyState>>,
    stop: Option<SyncSender<()>>,
    heartbeat: Option<JoinHandle<()>>,
    closed: bool,
}

impl RuntimeInputProxy {
    pub fn connect(client: RuntimeClient, instance_alias: &str) -> RuntimeClientResult<Self> {
        Self::connect_authority(
            RuntimeInputAuthority::Client(client),
            instance_alias,
            DEFAULT_RUNTIME_HEARTBEAT_INTERVAL,
        )
    }

    pub fn connect_debug(
        session: RuntimeDebugSession,
        instance_alias: &str,
    ) -> RuntimeClientResult<Self> {
        Self::connect_authority(
            RuntimeInputAuthority::Debug(session),
            instance_alias,
            DEFAULT_RUNTIME_HEARTBEAT_INTERVAL,
        )
    }

    pub fn connect_with_heartbeat(
        client: RuntimeClient,
        instance_alias: &str,
        heartbeat_interval: Duration,
    ) -> RuntimeClientResult<Self> {
        Self::connect_authority(
            RuntimeInputAuthority::Client(client),
            instance_alias,
            heartbeat_interval,
        )
    }

    fn connect_authority(
        authority: RuntimeInputAuthority,
        instance_alias: &str,
        heartbeat_interval: Duration,
    ) -> RuntimeClientResult<Self> {
        if heartbeat_interval.is_zero() || heartbeat_interval > MAX_RUNTIME_HEARTBEAT_INTERVAL {
            return Err(RuntimeClientError::fatal(
                "runtime_input_proxy_heartbeat_interval_invalid",
                "connect_runtime_input_proxy",
            ));
        }
        let token = authority.acquire_lease(instance_alias)?;
        let state = Arc::new(Mutex::new(ProxyState {
            token,
            heartbeat_failure: None,
        }));
        let (stop, receiver) = mpsc::sync_channel(1);
        let thread_state = Arc::clone(&state);
        let thread_authority = authority.clone();
        let heartbeat = thread::Builder::new()
            .name("actingcommand-runtime-heartbeat".to_string())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    heartbeat_loop(
                        &thread_authority,
                        &thread_state,
                        &receiver,
                        heartbeat_interval,
                    )
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
                let spawn_error = RuntimeClientError::fatal(
                    "runtime_input_proxy_heartbeat_spawn_failed",
                    "start_runtime_heartbeat",
                );
                return Err(match authority.release_lease(&lock_proxy(&state)?.token) {
                    Ok(()) => spawn_error,
                    Err(release_error) => spawn_error.with_related(release_error),
                });
            }
        };
        Ok(Self {
            authority,
            state,
            stop: Some(stop),
            heartbeat: Some(heartbeat),
            closed: false,
        })
    }

    pub fn input(&mut self, action: InputAction) -> RuntimeClientResult<()> {
        if self.closed {
            return Err(RuntimeClientError::fatal(
                "runtime_input_proxy_closed",
                "proxy_input",
            ));
        }
        let heartbeat_finished = self.heartbeat.as_ref().is_none_or(JoinHandle::is_finished);
        let state = lock_proxy(&self.state)?;
        if let Some(error) = &state.heartbeat_failure {
            return Err(error.clone());
        }
        if heartbeat_finished {
            return Err(RuntimeClientError::fatal(
                "runtime_input_proxy_heartbeat_stopped",
                "proxy_input",
            ));
        }
        self.authority.input(&state.token, action)
    }

    pub fn close(&mut self) -> RuntimeClientResult<()> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> RuntimeClientResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut failure = None;
        if self.stop.take().is_some_and(|stop| stop.send(()).is_err()) {
            merge_failure(
                &mut failure,
                RuntimeClientError::fatal(
                    "runtime_input_proxy_heartbeat_stop_failed",
                    "close_runtime_input_proxy",
                ),
            );
        }
        if self
            .heartbeat
            .take()
            .is_some_and(|heartbeat| heartbeat.join().is_err())
        {
            merge_failure(
                &mut failure,
                RuntimeClientError::fatal(
                    "runtime_input_proxy_heartbeat_join_failed",
                    "close_runtime_input_proxy",
                ),
            );
        }
        match lock_proxy(&self.state) {
            Ok(state) => {
                if let Some(error) = &state.heartbeat_failure {
                    merge_failure(&mut failure, error.clone());
                }
                if let Err(error) = self.authority.release_lease(&state.token) {
                    merge_failure(&mut failure, error);
                }
            }
            Err(error) => merge_failure(&mut failure, error),
        }
        failure.map_or(Ok(()), Err)
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
    authority: &RuntimeInputAuthority,
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
                match authority.renew_lease(&state.token) {
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

fn lock_proxy(state: &Mutex<ProxyState>) -> RuntimeClientResult<MutexGuard<'_, ProxyState>> {
    state.lock().map_err(|_| {
        RuntimeClientError::fatal(
            "runtime_input_proxy_state_poisoned",
            "access_runtime_input_proxy",
        )
    })
}

fn merge_failure(failure: &mut Option<RuntimeClientError>, error: RuntimeClientError) {
    *failure = Some(match failure.take() {
        Some(primary) => primary.with_related(error),
        None => error,
    });
}
