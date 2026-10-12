// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A (HOST-I3, I7): a held Runtime start in this process, stopped at the
//! runtime-host `test-hooks` preparation checkpoints, for tests outside runtime-host. Every wait
//! returns as soon as its message arrives; `GATE_WAIT` only bounds a broken run.

use actingcommand_contract::{
    ApplicationLifecycleAction, EventActor, EventSource, FencedWrite, GovernanceIdentityCard,
    IdentifierIssuer, InstallHeldStartup, InstallTransitionAction, InstallTransitionTicket,
};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, FrameMemoryBudget, InputBackend, OpenedBackend,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, PreparationCheckpoint, PreparationProbe, PreparationTestAction,
    PreparationTestHook, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
    RuntimeHostError,
};
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const GATE_WAIT: Duration = Duration::from_secs(120);
pub const HELD_SALT: &[u8] = b"held-runtime-test-salt";
pub const HELD_TRANSITION_ID: &str = "held-381a";
const HELD_TIMEOUT_MS: u64 = 60_000;

/// A provider without instances: a held start needs no device.
pub struct NoInstances;

impl ExecutionBackendProvider for NoInstances {
    fn instance_aliases(&self) -> Vec<String> {
        Vec::new()
    }

    fn resolve(&self, _instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        None
    }

    fn open_input(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<OpenedBackend<Box<dyn InputBackend>>> {
        Err(DeviceError::fatal("the held test Runtime has no instance"))
    }

    fn open_capture(
        &self,
        _instance_alias: &str,
        _memory: Option<&FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        Err(DeviceError::fatal("the held test Runtime has no instance"))
    }

    fn control_application(
        &self,
        _witness: &FencedWrite,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal("the held test Runtime has no instance"))
    }
}

/// A held start (`--install-held`) on its own thread. It reports each checkpoint and waits at
/// `Preparing` until `go_on`.
pub struct HeldStart {
    held: InstallHeldStartup,
    checkpoints: mpsc::Receiver<PreparationCheckpoint>,
    at_preparing: mpsc::Sender<PreparationTestAction>,
    start: JoinHandle<Result<RuntimeHost, RuntimeHostError>>,
}

impl HeldStart {
    pub fn spawn(state_root: &Path) -> Self {
        let (checkpoint_sender, checkpoints) = mpsc::channel();
        let (at_preparing, action_receiver) = mpsc::channel::<PreparationTestAction>();
        let hook_ends = Mutex::new((checkpoint_sender, action_receiver));
        let hook: PreparationTestHook = Arc::new(
            move |checkpoint: PreparationCheckpoint,
                  _probe: &PreparationProbe<'_>|
                  -> PreparationTestAction {
                let ends = hook_ends.lock().expect("hook channels");
                let (checkpoint_sender, action_receiver) = &*ends;
                checkpoint_sender
                    .send(checkpoint)
                    .expect("report the checkpoint");
                if checkpoint == PreparationCheckpoint::Held {
                    return PreparationTestAction::Continue;
                }
                action_receiver
                    .recv_timeout(GATE_WAIT)
                    .expect("the test lets the start go on")
            },
        );
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let held = InstallHeldStartup {
            transition_id: HELD_TRANSITION_ID.to_owned(),
            request_id: *issuer.mint_request_id().expect("request id").transport(),
            timeout_ms: HELD_TIMEOUT_MS,
            previous: None,
        };
        let start_root = state_root.to_path_buf();
        let start_held = held.clone();
        let start = thread::spawn(move || {
            RuntimeHost::start(
                RuntimeHostConfig::new(start_root, HELD_SALT)
                    .with_install_held(start_held)
                    .with_preparation_test_hook(hook),
                Arc::new(NoInstances),
            )
        });
        Self {
            held,
            checkpoints,
            at_preparing,
            start,
        }
    }

    pub fn reached(&self, expected: PreparationCheckpoint) {
        assert_eq!(
            self.checkpoints
                .recv_timeout(GATE_WAIT)
                .expect("the held start reaches its next checkpoint"),
            expected
        );
    }

    /// The installer's release through a client that declared its governance identity; the
    /// caller keeps the client while it needs the connection.
    pub fn release(&self, state_root: &Path) -> RuntimeClient {
        let installer = RuntimeClient::connect(RuntimeClientConfig::new(
            state_root,
            EventActor::Cli,
            EventSource::Cli,
        ))
        .expect("connect to the held Runtime");
        installer
            .declare_governance_identity(&GovernanceIdentityCard {
                client: "held-runtime-test".to_owned(),
                client_version: None,
                instance: None,
            })
            .expect("declare the installer identity");
        let ticket = InstallTransitionTicket {
            target: installer.runtime_info().shutdown_target(),
            transition_id: HELD_TRANSITION_ID.to_owned(),
            request_id: self.held.request_id,
        };
        installer
            .install_transition(InstallTransitionAction::Release {
                ticket,
                timeout_ms: HELD_TIMEOUT_MS,
            })
            .expect("release the held Runtime");
        installer
    }

    pub fn go_on(&self, action: PreparationTestAction) {
        self.at_preparing
            .send(action)
            .expect("let the held start go on");
    }

    pub fn finish(self) -> Result<RuntimeHost, RuntimeHostError> {
        self.start.join().expect("held start thread")
    }
}
