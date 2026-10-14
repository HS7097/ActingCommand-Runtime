// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-21: `actingctl emulator start|stop|restart --wait <s>` (MCP-0 command table
// v1.1 §4.22, FCM:213) against a Runtime in this process whose emulator is the scripted
// controller (HOST-I1).
// - Parity (§2.3): a control that ends within its wait prints what the same command prints
//   without `--wait`, and that is what MCP `ac_emulator` answers as `controlled`; MCP takes no
//   new argument, its wait is presentation (R4).
// - Expiry (§4.22, D11 as verified): a control that has not ended when its wait does leaves the
//   CLI with one line `{correlation_id, state: "in_progress"}` and the outcome
//   `command_wait_expired` (operation, waited_ms), exit 1; the Runtime goes on with the control
//   after the CLI has gone, and the vendor control is reached once the test lets it.

use crate::emulator_controller::{EmulatorCall, FakeEmulatorController};
use crate::held_runtime::GATE_WAIT;
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{
    ApplicationLifecycleAction, BackendOpenEntry, EmulatorInstanceAction, FencedWrite,
    IdentifierIssuer, InstanceId, InstanceStuckRecovery,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceCloseAuthority, DeviceResourceCloseOutcome,
    DeviceResult, Frame, FrameMemoryBudget, InputBackend, InstanceState, OpenedBackend,
    PixelFormat,
};
use actingcommand_runtime_host::{
    DiscoveredInstanceBinding, EmulatorControlOutcome, EmulatorControlResult,
    ExecutionBackendProvider, PendingAdbEndpoint, ResolvedAdbEndpoint, ResolvedExecutionInstance,
    RuntimeHost, RuntimeHostConfig,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, mpsc};
use tempfile::TempDir;

const ALIAS: &str = "gate.emulator.wait";
const SALT: &[u8] = b"emulator-wait-gate-salt";

// A device session that accepts every input and captures one small frame.
struct WaitBackend;

impl InputBackend for WaitBackend {
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

impl CaptureBackend for WaitBackend {
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

// One physical, discovery-bound instance whose emulator is the scripted controller. While
// `hold` is set, the next control waits until the test sends on it.
struct WaitedEmulator {
    instance_id: InstanceId,
    controller: Arc<FakeEmulatorController>,
    hold: Mutex<Option<mpsc::Receiver<()>>>,
}

impl ExecutionBackendProvider for WaitedEmulator {
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
            Box::new(WaitBackend) as Box<dyn InputBackend>,
            BackendOpenEntry::Input,
        ))
    }

    fn open_capture(
        &self,
        _instance_alias: &str,
        _memory: Option<&FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        Ok(OpenedBackend::unobserved(
            Box::new(WaitBackend) as Box<dyn CaptureBackend>,
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
        let held = self.hold.lock().expect("the hold").take();
        if let Some(release) = held {
            // GATE_WAIT only bounds a broken run; the test releases the hold.
            let _ = release.recv_timeout(GATE_WAIT);
        }
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

// A Runtime on `root` whose instance is `emulator`.
fn start_runtime(root: &Path, emulator: WaitedEmulator) -> RuntimeHost {
    RuntimeHost::start(
        RuntimeHostConfig::new(root, SALT).with_stuck_recovery(BTreeMap::from([(
            ALIAS.to_owned(),
            // The ladder is off; any cool-down in range.
            InstanceStuckRecovery {
                enabled: false,
                cooldown_secs: InstanceStuckRecovery::MAX_COOLDOWN_SECS,
            },
        )])),
        Arc::new(emulator),
    )
    .expect("start the Runtime")
}

fn emulator(
    controller: &Arc<FakeEmulatorController>,
    hold: Option<mpsc::Receiver<()>>,
) -> WaitedEmulator {
    WaitedEmulator {
        instance_id: *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport(),
        controller: Arc::clone(controller),
        hold: Mutex::new(hold),
    }
}

fn actingctl(state_root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(arguments)
        .arg("--state-root")
        .arg(state_root)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl")
}

fn describe(output: &Output) -> String {
    format!(
        "{}; stdout: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// The one stdout line of `output`, and its outcome taken out of it.
#[track_caller]
fn line(output: &Output) -> (Value, Value) {
    let mut line: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("no JSON line ({error}): {}", describe(output)));
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"))
        .unwrap_or_else(|| panic!("a line without its outcome: {}", describe(output)));
    (line, outcome)
}

#[test]
fn gate_an_emulator_control_that_ends_within_its_wait_answers_what_emulator_and_ac_emulator_answer()
{
    let root = TempDir::new().expect("tempdir");
    let controller = Arc::new(FakeEmulatorController::default());
    let host = start_runtime(root.path(), emulator(&controller, None));

    // Each stop is compared from the same state: the instance started and bound.
    let start = || actingctl(root.path(), &["emulator", "start", "--instance", ALIAS]);
    let first = start();
    let plain = actingctl(root.path(), &["emulator", "stop", "--instance", ALIAS]);
    let restarted = start();
    let waited = actingctl(
        root.path(),
        &["emulator", "stop", "--instance", ALIAS, "--wait", "60"],
    );
    let started_again = start();
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();
    let answer = mcp.call_tool(
        2,
        "ac_emulator",
        json!({"instance": ALIAS, "action": "stop"}),
    );
    let tool = accepted(&answer, "ac_emulator")["controlled"].clone();
    mcp.close_input();
    drop(mcp);
    host.close().expect("close the Runtime");

    for started in [&first, &restarted, &started_again] {
        assert!(started.status.success(), "{}", describe(started));
    }
    assert!(plain.status.success(), "{}", describe(&plain));
    assert!(waited.status.success(), "{}", describe(&waited));
    let (plain, plain_outcome) = line(&plain);
    let (waited, waited_outcome) = line(&waited);
    for outcome in [&plain_outcome, &waited_outcome] {
        assert_eq!(
            outcome["code"],
            json!("instance_emulator_controlled"),
            "{outcome}"
        );
    }
    assert_eq!(
        waited, plain,
        "--wait changed the answer of a control that ended within it"
    );
    assert_eq!(
        waited, tool,
        "actingctl emulator stop --wait and ac_emulator answer differently"
    );
}

#[test]
fn gate_an_emulator_wait_that_ends_first_exits_1_with_command_wait_expired_and_the_control_goes_on()
{
    let root = TempDir::new().expect("tempdir");
    let controller = Arc::new(FakeEmulatorController::default());
    let (release, hold) = mpsc::channel();
    let host = start_runtime(root.path(), emulator(&controller, Some(hold)));
    let calls = controller.listen();

    let expired = actingctl(
        root.path(),
        &["emulator", "stop", "--instance", ALIAS, "--wait", "1"],
    );
    // The CLI has gone; the Runtime still holds the control. Let it reach the vendor.
    release.send(()).expect("release the held control");
    // The first control call after the release; state reads and rebinds are not controls.
    // GATE_WAIT only bounds a broken run.
    let reached = std::iter::from_fn(|| calls.recv_timeout(GATE_WAIT).ok())
        .find(|call| matches!(call, EmulatorCall::Control(_)));
    host.close().expect("close the Runtime");

    assert_eq!(expired.status.code(), Some(1), "{}", describe(&expired));
    let (members, outcome) = line(&expired);
    assert_eq!(
        (
            &outcome["code"],
            &outcome["category"],
            &outcome["values"]["waited_ms"]
        ),
        (
            &json!("command_wait_expired"),
            &json!("error"),
            &json!(1000)
        ),
        "{outcome}"
    );
    assert!(
        outcome["values"]["operation"].is_string(),
        "the expired wait names its operation: {outcome}"
    );
    assert_eq!(members["state"], json!("in_progress"), "{members}");
    assert!(
        members["correlation_id"]
            .as_str()
            .is_some_and(|correlation| correlation.starts_with("correlation_")),
        "the expired wait names its request's correlation: {members}"
    );
    assert_eq!(
        reached,
        Some(EmulatorCall::Control(EmulatorInstanceAction::Stop)),
        "the control did not go on after the CLI stopped waiting"
    );
}
