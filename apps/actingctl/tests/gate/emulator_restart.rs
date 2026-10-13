// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 B, test plan A-4 G4: `actingctl emulator restart` against a Runtime in this
//! process whose emulator is the scripted controller (HOST-I1). The Stop is confirmed and the
//! Start's readiness wait expires: actingctl exits non-zero, the restart's failure record names
//! `restart_phase=start`, `actingctl emulator status` shows the instance pending, no stale
//! `device.connected=true` is left, and the vendor's `restart` is never dispatched.

use crate::emulator_controller::{
    EmulatorCall, FakeEmulatorController, ScriptedControl, stopped_state,
};
use actingcommand_artifact_store::ArtifactStore;
use actingcommand_contract::{
    ApplicationLifecycleAction, BackendOpenEntry, EmulatorInstanceAction, EventPayload, EventQuery,
    FactValue, FencedWrite, IdentifierIssuer, InstanceId, InstanceStuckRecovery,
    RuntimeFactSnapshot, RuntimePayload,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceCloseAuthority, DeviceResourceCloseOutcome,
    DeviceResult, Frame, FrameMemoryBudget, InputBackend, InstanceState, OpenedBackend,
    PixelFormat,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig, PersistedEvent};
use actingcommand_runtime_host::{
    DiscoveredInstanceBinding, EmulatorControlOutcome, EmulatorControlResult,
    ExecutionBackendProvider, PendingAdbEndpoint, ResolvedAdbEndpoint, ResolvedExecutionInstance,
    RuntimeHost, RuntimeHostConfig,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use tempfile::TempDir;

const ALIAS: &str = "gate.emulator";
const SALT: &[u8] = b"emulator-restart-gate-salt";

/// A device session that accepts every input and captures one small frame.
struct GateBackend;

impl InputBackend for GateBackend {
    fn tap(&mut self, _witness: &FencedWrite, _x: i32, _y: i32) -> DeviceResult<()> {
        Ok(())
    }

    fn long_tap(
        &mut self,
        _witness: &FencedWrite,
        _x: i32,
        _y: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn swipe(
        &mut self,
        _witness: &FencedWrite,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn key(&mut self, _witness: &FencedWrite, _key: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn reset(&mut self, _witness: &FencedWrite) -> DeviceResult<()> {
        Ok(())
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(1))
    }
}

impl CaptureBackend for GateBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        Frame::from_pixels(
            2,
            1,
            vec![0; 6],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(1))
    }
}

/// One physical, discovery-bound instance whose emulator is the scripted controller.
struct ScriptedEmulator {
    instance_id: InstanceId,
    controller: Arc<FakeEmulatorController>,
}

impl ExecutionBackendProvider for ScriptedEmulator {
    fn instance_aliases(&self) -> Vec<String> {
        vec![ALIAS.to_owned()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == ALIAS).then(|| {
            let binding =
                DiscoveredInstanceBinding::new(0, "fixture", "1.0.0", "C:/fixture/MuMuManager.exe");
            let resolved = ResolvedExecutionInstance::new(self.instance_id, "127.0.0.1:16384");
            match self.controller.bound_port() {
                Some(port) => resolved.with_adb_endpoint(
                    ResolvedAdbEndpoint::new("127.0.0.1", port, false)
                        .with_discovered_binding(binding),
                ),
                None => {
                    resolved.with_pending_endpoint(PendingAdbEndpoint::new("127.0.0.1", binding))
                }
            }
        })
    }

    fn open_input(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<OpenedBackend<Box<dyn InputBackend>>> {
        Ok(OpenedBackend::unobserved(
            Box::new(GateBackend) as Box<dyn InputBackend>,
            BackendOpenEntry::Input,
        ))
    }

    fn open_capture(
        &self,
        _instance_alias: &str,
        _memory: Option<&FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        Ok(OpenedBackend::unobserved(
            Box::new(GateBackend) as Box<dyn CaptureBackend>,
            BackendOpenEntry::Capture,
        ))
    }

    fn control_application(
        &self,
        _witness: &FencedWrite,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn control_instance(
        &self,
        _instance_alias: &str,
        action: EmulatorInstanceAction,
    ) -> EmulatorControlResult<EmulatorControlOutcome> {
        self.controller.control(0, action)
    }

    fn read_instance_state(&self, _instance_alias: &str) -> DeviceResult<InstanceState> {
        self.controller.read_state()
    }

    fn rebind_discovered_endpoint(
        &self,
        _instance_alias: &str,
        adb_port: Option<u16>,
    ) -> DeviceResult<()> {
        self.controller.rebind(adb_port);
        Ok(())
    }
}

fn actingctl(state_root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(arguments)
        .arg("--state-root")
        .arg(state_root)
        .output()
        .expect("run actingctl")
}

fn closed_ledger_events(root: &Path) -> Vec<PersistedEvent> {
    let artifacts = ArtifactStore::open(root).expect("open the artifact store");
    GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root), |reference| {
        artifacts.verify_recovery_reference(reference).ok()
    })
    .expect("read the closed ledger")
    .query(&EventQuery::default())
}

fn describe(output: &Output) -> String {
    format!(
        "{}; stdout: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn gate_emulator_restart_whose_start_times_out_exits_non_zero_and_leaves_no_stale_connection() {
    let root = TempDir::new().expect("tempdir");
    let controller = Arc::new(FakeEmulatorController::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), SALT).with_stuck_recovery(BTreeMap::from([(
            ALIAS.to_owned(),
            InstanceStuckRecovery {
                enabled: false,
                cooldown_secs: InstanceStuckRecovery::DEFAULT_COOLDOWN_SECS,
            },
        )])),
        Arc::new(ScriptedEmulator {
            instance_id,
            controller: Arc::clone(&controller),
        }),
    )
    .expect("start the Runtime");
    // The instance runs and is connected before the restart.
    let started = actingctl(root.path(), &["emulator", "start", "--instance", ALIAS]);
    controller.script_control(
        EmulatorInstanceAction::Start,
        ScriptedControl::Failed {
            stage: actingcommand_device::codes::DeviceStage::MumuManagerWaitTimeout.location(),
            exit_code: Some(0),
            last_state: Some(stopped_state()),
            device_process_present: None,
        },
    );
    controller.script_read(stopped_state());
    let restarted = actingctl(root.path(), &["emulator", "restart", "--instance", ALIAS]);
    let status = actingctl(root.path(), &["emulator", "status", "--instance", ALIAS]);
    let facts = actingctl(root.path(), &["facts", "--program"]);
    host.close().expect("close the Runtime");
    let events = closed_ledger_events(root.path());

    assert!(started.status.success(), "{}", describe(&started));
    assert!(!restarted.status.success(), "{}", describe(&restarted));
    let controls = controller
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            EmulatorCall::Control(action) => Some(action),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        controls,
        [
            EmulatorInstanceAction::Start,
            EmulatorInstanceAction::Stop,
            EmulatorInstanceAction::Start,
        ],
        "the vendor's restart is never dispatched"
    );
    // Ledger revision 3: the restart's failure record carries its failure as its outcome, and
    // the restart's tokens as the outcome's values.
    let restart = events
        .iter()
        .find_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.lifecycle_failure().is_some()
                    && record.outcome().code().as_str() == "emulator_restart_start_failed" =>
            {
                Some(record.outcome().clone())
            }
            _ => None,
        })
        .expect("the restart's failure record");
    assert_eq!(
        restart.value("restart_phase").and_then(Value::as_str),
        Some("start"),
        "{restart}"
    );
    assert!(status.status.success(), "{}", describe(&status));
    let status: Value = serde_json::from_slice(&status.stdout).expect("status JSON");
    assert!(
        status.get("adb_port").is_none_or(Value::is_null),
        "the instance is pending: {status}"
    );
    assert!(facts.status.success(), "{}", describe(&facts));
    let facts: RuntimeFactSnapshot = serde_json::from_slice(&facts.stdout).expect("facts JSON");
    assert!(
        !facts.records.iter().any(|record| {
            record.key == "device.connected" && record.value == FactValue::Boolean(true)
        }),
        "{:?}",
        facts.records
    );
}
