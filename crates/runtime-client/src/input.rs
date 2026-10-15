// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{ClientCode, ClientLocation};
use crate::{RuntimeClient, RuntimeClientError, RuntimeClientResult, RuntimeDebugSession};
use actingcommand_contract::{InputAction, InputFrameReference, LeaseToken, RuntimeReceipt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(feature = "test-observation")]
use crate::test_observation::{
    ObservationOperation, ObservationOutcome, ObservationStage, ObservationThreadRole,
    current_observation_owner, enter_observation_owner, record_active,
};

pub const DEFAULT_RUNTIME_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(2_500);
pub const MAX_RUNTIME_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(5_000);

struct ProxyState {
    token: LeaseToken,
    heartbeat_failure: Option<RuntimeClientError>,
}

/// What the heartbeat thread's channel carries: the proxy's stop and, in tests only, one beat of
/// the manual heartbeat timer (test plan v2.2 §3.10 C-4, CON-I2).
pub(crate) enum HeartbeatSignal {
    Stop,
    /// One heartbeat interval has passed on the test's manual timer.
    #[cfg(test)]
    Beat,
}

/// How the heartbeat thread waits out one heartbeat interval (test plan v2.2 §3.10 C-4, CON-I2).
/// Every production constructor waits on the wall clock.
#[derive(Clone, Copy)]
pub(crate) enum HeartbeatTimer {
    /// `stop.recv_timeout(heartbeat_interval)`: the interval passes on the wall clock.
    Wall,
    /// Tests only: no wall clock. A beat the test sends on the proxy's channel is one passed
    /// interval; the proxy's stop and a closed channel end the wait as on the wall clock.
    #[cfg(test)]
    Manual,
}

/// How one heartbeat wait ended.
enum HeartbeatWait {
    Stop,
    Closed,
    Elapsed,
}

impl HeartbeatTimer {
    fn wait(
        self,
        signals: &Receiver<HeartbeatSignal>,
        heartbeat_interval: Duration,
    ) -> HeartbeatWait {
        let signal = match self {
            Self::Wall => match signals.recv_timeout(heartbeat_interval) {
                Ok(signal) => signal,
                Err(RecvTimeoutError::Timeout) => return HeartbeatWait::Elapsed,
                Err(RecvTimeoutError::Disconnected) => return HeartbeatWait::Closed,
            },
            #[cfg(test)]
            Self::Manual => match signals.recv() {
                Ok(signal) => signal,
                Err(_) => return HeartbeatWait::Closed,
            },
        };
        match signal {
            HeartbeatSignal::Stop => HeartbeatWait::Stop,
            #[cfg(test)]
            HeartbeatSignal::Beat => HeartbeatWait::Elapsed,
        }
    }
}

/// Test plan v2.2 §3.10 C-4 (CON-I2): a lease authority whose answers the test scripts. Tests
/// only; neither the `test-support` feature nor a shipped build has it.
#[cfg(test)]
pub(crate) trait TestInputAuthority: Send + Sync {
    fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken>;
    fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken>;
    fn input(
        &self,
        token: &LeaseToken,
        action: InputAction,
        frame: Option<InputFrameReference>,
    ) -> RuntimeClientResult<RuntimeReceipt>;
    fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()>;
}

#[derive(Clone)]
pub(crate) enum RuntimeInputAuthority {
    Client(RuntimeClient),
    Debug(RuntimeDebugSession),
    /// Tests only: the test's scripted authority (test plan v2.2 §3.10 C-4).
    #[cfg(test)]
    Fake(Arc<dyn TestInputAuthority>),
}

impl RuntimeInputAuthority {
    fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken> {
        match self {
            Self::Client(client) => client.acquire_lease(instance_alias),
            Self::Debug(session) => session.acquire_lease(instance_alias),
            #[cfg(test)]
            Self::Fake(fake) => fake.acquire_lease(instance_alias),
        }
    }

    fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken> {
        match self {
            Self::Client(client) => client.renew_lease(token),
            Self::Debug(session) => session.renew_lease(token),
            #[cfg(test)]
            Self::Fake(fake) => fake.renew_lease(token),
        }
    }

    fn input(
        &self,
        token: &LeaseToken,
        action: InputAction,
        frame: Option<InputFrameReference>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        match self {
            Self::Client(client) => client.input_with_frame(token, action, frame),
            Self::Debug(session) => session.input_with_frame(token, action, frame),
            #[cfg(test)]
            Self::Fake(fake) => fake.input(token, action, frame),
        }
    }

    fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()> {
        match self {
            Self::Client(client) => client.release_lease(token),
            Self::Debug(session) => session.release_lease(token),
            #[cfg(test)]
            Self::Fake(fake) => fake.release_lease(token),
        }
    }
}

/// Tests only: the manual heartbeat timer's beats, sent on the proxy's own heartbeat channel
/// (test plan v2.2 §3.10 C-4).
#[cfg(test)]
pub(crate) struct ManualHeartbeat(SyncSender<HeartbeatSignal>);

#[cfg(test)]
impl ManualHeartbeat {
    /// One heartbeat interval passes. False once the heartbeat thread has ended and its channel
    /// is closed.
    pub(crate) fn beat(&self) -> bool {
        self.0.send(HeartbeatSignal::Beat).is_ok()
    }
}

/// Connection-scoped write lease that sends typed input commands to the resident Runtime.
///
/// This proxy owns no device backend. Device-specific adapters belong outside runtime-client.
pub struct RuntimeInputProxy {
    authority: RuntimeInputAuthority,
    state: Arc<Mutex<ProxyState>>,
    stop: Option<SyncSender<HeartbeatSignal>>,
    heartbeat: Option<JoinHandle<()>>,
    closed: bool,
}

impl RuntimeInputProxy {
    pub fn connect(client: RuntimeClient, instance_alias: &str) -> RuntimeClientResult<Self> {
        Self::connect_authority(
            RuntimeInputAuthority::Client(client),
            instance_alias,
            DEFAULT_RUNTIME_HEARTBEAT_INTERVAL,
            HeartbeatTimer::Wall,
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
            HeartbeatTimer::Wall,
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
            HeartbeatTimer::Wall,
        )
    }

    /// Tests only (test plan v2.2 §3.10 C-4): a proxy on `authority` whose heartbeat waits on
    /// `timer`.
    #[cfg(test)]
    pub(crate) fn connect_for_test(
        authority: RuntimeInputAuthority,
        instance_alias: &str,
        heartbeat_interval: Duration,
        timer: HeartbeatTimer,
    ) -> RuntimeClientResult<Self> {
        Self::connect_authority(authority, instance_alias, heartbeat_interval, timer)
    }

    /// Tests only: the beats of a proxy connected with `HeartbeatTimer::Manual`; `None` once the
    /// proxy is closed.
    #[cfg(test)]
    pub(crate) fn manual_heartbeat(&self) -> Option<ManualHeartbeat> {
        self.stop.clone().map(ManualHeartbeat)
    }

    fn connect_authority(
        authority: RuntimeInputAuthority,
        instance_alias: &str,
        heartbeat_interval: Duration,
        timer: HeartbeatTimer,
    ) -> RuntimeClientResult<Self> {
        if heartbeat_interval.is_zero() || heartbeat_interval > MAX_RUNTIME_HEARTBEAT_INTERVAL {
            return Err(RuntimeClientError::fatal(
                ClientCode::RuntimeInputProxyHeartbeatIntervalInvalid,
                ClientLocation::ConnectRuntimeInputProxy.location(),
            ));
        }
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ProxyAcquireStart,
            ObservationOperation::AcquireLease,
            ObservationThreadRole::Client,
            ObservationOutcome::Started,
            None,
            None,
            None,
        );
        let token = match authority.acquire_lease(instance_alias) {
            Ok(token) => {
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::ProxyAcquireResult,
                    ObservationOperation::AcquireLease,
                    ObservationThreadRole::Client,
                    ObservationOutcome::Success,
                    None,
                    None,
                    Some(&token),
                );
                token
            }
            Err(error) => {
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::ProxyAcquireResult,
                    ObservationOperation::AcquireLease,
                    ObservationThreadRole::Client,
                    ObservationOutcome::Failure,
                    None,
                    None,
                    None,
                );
                return Err(error);
            }
        };
        let state = Arc::new(Mutex::new(ProxyState {
            token,
            heartbeat_failure: None,
        }));
        let (stop, receiver) = mpsc::sync_channel(1);
        let thread_state = Arc::clone(&state);
        let thread_authority = authority.clone();
        #[cfg(feature = "test-observation")]
        let heartbeat_observation_owner = current_observation_owner();
        let heartbeat = thread::Builder::new()
            .name("actingcommand-runtime-heartbeat".to_string())
            .spawn(move || {
                #[cfg(feature = "test-observation")]
                let _observation_owner = enter_observation_owner(heartbeat_observation_owner);
                let result = catch_unwind(AssertUnwindSafe(|| {
                    heartbeat_loop(
                        &thread_authority,
                        &thread_state,
                        &receiver,
                        heartbeat_interval,
                        timer,
                    )
                }));
                if result.is_err() {
                    let mut state = match thread_state.lock() {
                        Ok(state) => state,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    state.heartbeat_failure = Some(RuntimeClientError::fatal(
                        ClientCode::RuntimeHeartbeatPanicked,
                        ClientLocation::RenewRuntimeLease.location(),
                    ));
                }
            });
        let heartbeat = match heartbeat {
            Ok(heartbeat) => heartbeat,
            Err(_) => {
                let spawn_error = RuntimeClientError::fatal(
                    ClientCode::RuntimeInputProxyHeartbeatSpawnFailed,
                    ClientLocation::StartRuntimeHeartbeat.location(),
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

    pub fn input(&mut self, action: InputAction) -> RuntimeClientResult<RuntimeReceipt> {
        self.input_with_frame(action, None)
    }

    pub fn input_with_frame(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameReference>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        if self.closed {
            return Err(RuntimeClientError::fatal(
                ClientCode::RuntimeInputProxyClosed,
                ClientLocation::ProxyInput.location(),
            ));
        }
        let heartbeat_finished = self.heartbeat.as_ref().is_none_or(JoinHandle::is_finished);
        let state = lock_proxy(&self.state)?;
        if let Some(error) = &state.heartbeat_failure {
            return Err(error.clone());
        }
        if heartbeat_finished {
            return Err(RuntimeClientError::fatal(
                ClientCode::RuntimeInputProxyHeartbeatStopped,
                ClientLocation::ProxyInput.location(),
            ));
        }
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ProxyInputStart,
            ObservationOperation::Input,
            ObservationThreadRole::Client,
            ObservationOutcome::Started,
            None,
            None,
            Some(&state.token),
        );
        let result = self.authority.input(&state.token, action, frame);
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ProxyInputResult,
            ObservationOperation::Input,
            ObservationThreadRole::Client,
            if result.is_ok() {
                ObservationOutcome::Success
            } else {
                ObservationOutcome::Failure
            },
            None,
            None,
            Some(&state.token),
        );
        result
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
        if let Some(stop) = self.stop.take() {
            let result = stop.send(HeartbeatSignal::Stop);
            #[cfg(feature = "test-observation")]
            record_active(
                ObservationStage::ProxyStopSend,
                ObservationOperation::Proxy,
                ObservationThreadRole::Client,
                if result.is_ok() {
                    ObservationOutcome::Stop
                } else {
                    ObservationOutcome::Failure
                },
                None,
                None,
                None,
            );
            if result.is_err() {
                merge_failure(
                    &mut failure,
                    RuntimeClientError::fatal(
                        ClientCode::RuntimeInputProxyHeartbeatStopFailed,
                        ClientLocation::CloseRuntimeInputProxy.location(),
                    ),
                );
            }
        }
        if let Some(heartbeat) = self.heartbeat.take() {
            #[cfg(feature = "test-observation")]
            record_active(
                ObservationStage::HeartbeatJoinStart,
                ObservationOperation::Proxy,
                ObservationThreadRole::Client,
                ObservationOutcome::Started,
                None,
                None,
                None,
            );
            let result = heartbeat.join();
            #[cfg(feature = "test-observation")]
            record_active(
                ObservationStage::HeartbeatJoinResult,
                ObservationOperation::Proxy,
                ObservationThreadRole::Client,
                if result.is_ok() {
                    ObservationOutcome::Success
                } else {
                    ObservationOutcome::Failure
                },
                None,
                None,
                None,
            );
            if result.is_err() {
                merge_failure(
                    &mut failure,
                    RuntimeClientError::fatal(
                        ClientCode::RuntimeInputProxyHeartbeatJoinFailed,
                        ClientLocation::CloseRuntimeInputProxy.location(),
                    ),
                );
            }
        }
        match lock_proxy(&self.state) {
            Ok(state) => {
                if let Some(error) = &state.heartbeat_failure {
                    merge_failure(&mut failure, error.clone());
                }
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::ProxyReleaseStart,
                    ObservationOperation::ReleaseLease,
                    ObservationThreadRole::Client,
                    ObservationOutcome::Started,
                    None,
                    None,
                    Some(&state.token),
                );
                let release = self.authority.release_lease(&state.token);
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::ProxyReleaseResult,
                    ObservationOperation::ReleaseLease,
                    ObservationThreadRole::Client,
                    if release.is_ok() {
                        ObservationOutcome::Success
                    } else {
                        ObservationOutcome::Failure
                    },
                    None,
                    None,
                    Some(&state.token),
                );
                if let Err(error) = release {
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
    stop: &Receiver<HeartbeatSignal>,
    heartbeat_interval: Duration,
    timer: HeartbeatTimer,
) {
    loop {
        match timer.wait(stop, heartbeat_interval) {
            HeartbeatWait::Stop => {
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::HeartbeatWaitResult,
                    ObservationOperation::Proxy,
                    ObservationThreadRole::Heartbeat,
                    ObservationOutcome::Stop,
                    None,
                    None,
                    None,
                );
                return;
            }
            HeartbeatWait::Closed => {
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::HeartbeatWaitResult,
                    ObservationOperation::Proxy,
                    ObservationThreadRole::Heartbeat,
                    ObservationOutcome::Closed,
                    None,
                    None,
                    None,
                );
                return;
            }
            HeartbeatWait::Elapsed => {
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::HeartbeatWaitResult,
                    ObservationOperation::Proxy,
                    ObservationThreadRole::Heartbeat,
                    ObservationOutcome::Timeout,
                    None,
                    None,
                    None,
                );
                let mut state = match state.lock() {
                    Ok(state) => state,
                    Err(_) => return,
                };
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::HeartbeatRenewStart,
                    ObservationOperation::RenewLease,
                    ObservationThreadRole::Heartbeat,
                    ObservationOutcome::Started,
                    None,
                    None,
                    Some(&state.token),
                );
                match authority.renew_lease(&state.token) {
                    Ok(token) => {
                        #[cfg(feature = "test-observation")]
                        record_active(
                            ObservationStage::HeartbeatRenewResult,
                            ObservationOperation::RenewLease,
                            ObservationThreadRole::Heartbeat,
                            ObservationOutcome::Success,
                            None,
                            None,
                            Some(&token),
                        );
                        state.token = token;
                    }
                    Err(error) => {
                        #[cfg(feature = "test-observation")]
                        record_active(
                            ObservationStage::HeartbeatRenewResult,
                            ObservationOperation::RenewLease,
                            ObservationThreadRole::Heartbeat,
                            ObservationOutcome::Failure,
                            None,
                            None,
                            Some(&state.token),
                        );
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
            ClientCode::RuntimeInputProxyStatePoisoned,
            ClientLocation::AccessRuntimeInputProxy.location(),
        )
    })
}

fn merge_failure(failure: &mut Option<RuntimeClientError>, error: RuntimeClientError) {
    *failure = Some(match failure.take() {
        Some(primary) => primary.with_related(error),
        None => error,
    });
}
