// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 B (test plan HOST-I1, EXEC-I2): the scripted emulator controller behind the
//! fake providers of execution-kernel, runtime-host and actingctl's gate binary, one script
//! format for all three. Each action takes its scripted results in order; an action with no
//! result left ends as a confirmed Stop (no device process left) or a Start running on
//! `FAKE_EMULATOR_PORT`. A state read takes its scripted states in order and fails when none is
//! left. Every call is logged and, while a test listens, sent to it, so a test waits for a call
//! instead of for time.

use actingcommand_contract::EmulatorInstanceAction;
use actingcommand_device::{
    DeviceError, DeviceErrorCategory, DeviceProcessPresence, DeviceResult, EmulatorControlFailure,
    EmulatorControlOutcome, EmulatorControlResult, InstanceState,
};
use std::collections::VecDeque;
use std::sync::{Mutex, mpsc};

/// The port a fresh controller is bound to and an unscripted Start reports.
pub const FAKE_EMULATOR_PORT: u16 = 16384;
/// The scripted provider time of a confirmed action and of a failed one.
pub const SCRIPTED_CONFIRMED_MS: u64 = 1_000;
pub const SCRIPTED_FAILED_MS: u64 = 2_000;

/// One call the controller saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmulatorCall {
    Control(EmulatorInstanceAction),
    ReadState,
    Rebind(Option<u16>),
    Discover,
    ObserveForeground,
}

/// One scripted result of a control action.
#[derive(Debug, Clone)]
pub enum ScriptedControl {
    /// A confirmed Stop with its process check.
    Stopped {
        device_process_present: DeviceProcessPresence,
    },
    /// A confirmed Start, running on `adb_port` (`None`: running without a reported port).
    Running { adb_port: Option<u16> },
    /// A failure at the device stage `stage`, with the vendor exit code (`None`: the vendor
    /// command did not run), the last observation and a Stop's process check.
    Failed {
        stage: &'static str,
        exit_code: Option<i32>,
        last_state: Option<InstanceState>,
        device_process_present: Option<DeviceProcessPresence>,
    },
}

struct Script {
    stop: VecDeque<ScriptedControl>,
    start: VecDeque<ScriptedControl>,
    restart: VecDeque<ScriptedControl>,
    reads: VecDeque<InstanceState>,
    calls: Vec<EmulatorCall>,
    bound_port: Option<u16>,
    listener: Option<mpsc::Sender<EmulatorCall>>,
}

pub struct FakeEmulatorController {
    script: Mutex<Script>,
}

impl Default for FakeEmulatorController {
    fn default() -> Self {
        Self::bound(FAKE_EMULATOR_PORT)
    }
}

/// A stopped instance as `info` reports it: no process, no Android, no port.
pub fn stopped_state() -> InstanceState {
    InstanceState {
        process_started: false,
        android_started: false,
        adb_port: None,
        player_state: None,
        launch_err_code: 0,
        launch_err_msg: String::new(),
    }
}

/// A running instance as `info` reports it, on `adb_port`.
pub fn running_state(adb_port: u16) -> InstanceState {
    InstanceState {
        process_started: true,
        android_started: true,
        adb_port: Some(adb_port),
        player_state: None,
        launch_err_code: 0,
        launch_err_msg: String::new(),
    }
}

/// The device stage's error category, as the device crate types it.
fn stage_category(stage: &str) -> DeviceErrorCategory {
    match stage {
        "mumu_manager.control_exit" | "mumu_manager.exit" => DeviceErrorCategory::ChildExit,
        "mumu_manager.run" | "mumu_manager.timeout" | "mumu_manager.path" => {
            DeviceErrorCategory::BackendLaunch
        }
        _ => DeviceErrorCategory::Response,
    }
}

impl FakeEmulatorController {
    /// A controller whose instance is bound to `adb_port`.
    pub fn bound(adb_port: u16) -> Self {
        Self::with_binding(Some(adb_port))
    }

    /// A controller whose instance is pending (stopped).
    pub fn pending() -> Self {
        Self::with_binding(None)
    }

    fn with_binding(bound_port: Option<u16>) -> Self {
        Self {
            script: Mutex::new(Script {
                stop: VecDeque::new(),
                start: VecDeque::new(),
                restart: VecDeque::new(),
                reads: VecDeque::new(),
                calls: Vec::new(),
                bound_port,
                listener: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().expect("emulator controller script")
    }

    /// Appends one result to `action`'s script.
    pub fn script_control(&self, action: EmulatorInstanceAction, result: ScriptedControl) {
        let mut script = self.lock();
        match action {
            EmulatorInstanceAction::Stop => script.stop.push_back(result),
            EmulatorInstanceAction::Start => script.start.push_back(result),
            EmulatorInstanceAction::Restart => script.restart.push_back(result),
        }
    }

    /// Appends one state to the reads' script.
    pub fn script_read(&self, state: InstanceState) {
        self.lock().reads.push_back(state);
    }

    /// Every later call is also sent to the returned receiver.
    pub fn listen(&self) -> mpsc::Receiver<EmulatorCall> {
        let (sender, receiver) = mpsc::channel();
        self.lock().listener = Some(sender);
        receiver
    }

    pub fn calls(&self) -> Vec<EmulatorCall> {
        self.lock().calls.clone()
    }

    /// The port the instance is bound to; `None` while it is pending.
    pub fn bound_port(&self) -> Option<u16> {
        self.lock().bound_port
    }

    /// Logs a call that has no scripted result.
    pub fn note(&self, call: EmulatorCall) {
        let mut script = self.lock();
        script.calls.push(call);
        let gone = script
            .listener
            .as_ref()
            .is_some_and(|listener| listener.send(call).is_err());
        if gone {
            script.listener = None;
        }
    }

    pub fn control(
        &self,
        instance_index: u16,
        action: EmulatorInstanceAction,
    ) -> EmulatorControlResult<EmulatorControlOutcome> {
        self.note(EmulatorCall::Control(action));
        let scripted = {
            let mut script = self.lock();
            match action {
                EmulatorInstanceAction::Stop => script.stop.pop_front(),
                EmulatorInstanceAction::Start => script.start.pop_front(),
                EmulatorInstanceAction::Restart => script.restart.pop_front(),
            }
        };
        let result = scripted.unwrap_or(match action {
            EmulatorInstanceAction::Stop => ScriptedControl::Stopped {
                device_process_present: DeviceProcessPresence::Absent,
            },
            EmulatorInstanceAction::Start | EmulatorInstanceAction::Restart => {
                ScriptedControl::Running {
                    adb_port: Some(FAKE_EMULATOR_PORT),
                }
            }
        });
        match result {
            ScriptedControl::Stopped {
                device_process_present,
            } => Ok(EmulatorControlOutcome {
                exit_code: Some(0),
                output_summary: "scripted emulator control".to_owned(),
                instance_index,
                process_started: false,
                running: false,
                adb_port: None,
                player_state: None,
                elapsed_ms: SCRIPTED_CONFIRMED_MS,
                device_process_present: Some(device_process_present),
            }),
            ScriptedControl::Running { adb_port } => Ok(EmulatorControlOutcome {
                exit_code: Some(0),
                output_summary: "scripted emulator control".to_owned(),
                instance_index,
                process_started: true,
                running: true,
                adb_port,
                player_state: None,
                elapsed_ms: SCRIPTED_CONFIRMED_MS,
                device_process_present: None,
            }),
            ScriptedControl::Failed {
                stage,
                exit_code,
                last_state,
                device_process_present,
            } => Err(Box::new(EmulatorControlFailure {
                error: DeviceError::fatal("scripted emulator control failure")
                    .with_diagnostic(stage_category(stage), stage),
                exit_code,
                output_summary: "scripted vendor output".to_owned(),
                last_state,
                elapsed_ms: SCRIPTED_FAILED_MS,
                device_process_present,
            })),
        }
    }

    pub fn read_state(&self) -> DeviceResult<InstanceState> {
        self.note(EmulatorCall::ReadState);
        self.lock().reads.pop_front().ok_or_else(|| {
            DeviceError::fatal("scripted emulator state read: no answer")
                .with_diagnostic(DeviceErrorCategory::BackendLaunch, "mumu_manager.timeout")
        })
    }

    pub fn rebind(&self, adb_port: Option<u16>) {
        self.note(EmulatorCall::Rebind(adb_port));
        self.lock().bound_port = adb_port;
    }
}
