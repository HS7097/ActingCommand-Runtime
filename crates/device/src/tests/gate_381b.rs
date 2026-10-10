// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 B, test plan E-2 (device): a Stop is confirmed only when `info` reports the
//! instance's process stopped and no vendor device process of the instance is left (F2, review
//! M2), replayed over the injected vendor surface (EXEC-I1); and the two tables the readiness
//! wait reads (`contracts/emulator-control.md`, the `info` document and the errcode envelope).
//! Time is the replay's own clock: a poll, a listing and a sleep advance it, nothing waits.

use crate::discovery::DeviceDiscoveryProcess;
use crate::mumu_manager::{
    DeviceProcessPresence, InstanceState, MUMU_MANAGER_COMMAND_TIMEOUT,
    MUMU_MANAGER_PROCESS_LIST_INTERVAL, MUMU_MANAGER_PROCESS_LIST_TIMEOUT,
    MUMU_MANAGER_STATE_WAIT_STOP, MUMU_MANAGER_TIMEOUT_STAGE, MumuControlOutput, MumuManagerRunner,
    control_instance_with, errcode_envelope, parse_instance_document,
};
use crate::{DeviceError, DeviceErrorCategory, DeviceResult};
use actingcommand_contract::EmulatorInstanceAction;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

const fn seconds(value: u64) -> Duration {
    Duration::from_secs(value)
}

/// What the replay saw, in order, each at its own time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Control(Vec<String>),
    /// An `info` poll: `Some(true)` when it answered "stopped", `None` when its bound expired.
    Info(Option<bool>),
    /// A process listing with its bound.
    Listing(Duration),
}

/// The vendor surface of one Stop: `control` answers `stdout` with `exit_code` after 1 s;
/// every `info` poll expires its 10 s bound until `info_silent_for` has passed since `control`
/// returned, then answers "stopped"; a listing takes 2 s and shows the device process with
/// `-v 0` while the clock is before `device_process_until` (with the device process of another
/// instance and an unrelated process always), or fails when that is `None`.
struct Replay {
    clock: Duration,
    stdout: &'static str,
    exit_code: Option<i32>,
    info_silent_until: Option<Duration>,
    info_silent_for: Duration,
    device_process_until: Option<Duration>,
    seen: Vec<(Duration, Seen)>,
}

impl Replay {
    fn new(info_silent_for: Duration, device_process_until: Option<Duration>) -> Self {
        Self {
            clock: Duration::ZERO,
            stdout: r#"{"errcode": 0, "errmsg": ""}"#,
            exit_code: Some(0),
            info_silent_until: None,
            info_silent_for,
            device_process_until,
            seen: Vec::new(),
        }
    }

    fn control_answer(mut self, stdout: &'static str, exit_code: Option<i32>) -> Self {
        self.stdout = stdout;
        self.exit_code = exit_code;
        self
    }

    fn controls(&self) -> Vec<Vec<String>> {
        self.seen
            .iter()
            .filter_map(|(_, seen)| match seen {
                Seen::Control(args) => Some(args.clone()),
                _ => None,
            })
            .collect()
    }

    /// Each listing's time and bound.
    fn listings(&self) -> Vec<(Duration, Duration)> {
        self.seen
            .iter()
            .filter_map(|(at, seen)| match seen {
                Seen::Listing(bound) => Some((*at, *bound)),
                _ => None,
            })
            .collect()
    }

    /// The position in `seen` of the first `info` answer "stopped".
    fn first_stopped_answer(&self) -> Option<usize> {
        self.seen
            .iter()
            .position(|(_, seen)| *seen == Seen::Info(Some(true)))
    }
}

fn device_process(process_id: u32, instance_index: u16) -> DeviceDiscoveryProcess {
    DeviceDiscoveryProcess {
        process_id,
        name: "MuMuNxDevice.exe".to_owned(),
        executable_path: Some(PathBuf::from("C:/fixture/shell/MuMuNxDevice.exe")),
        command_line: Some(format!(
            "\"C:/fixture/shell/MuMuNxDevice.exe\" -v {instance_index}"
        )),
    }
}

fn unrelated_process() -> DeviceDiscoveryProcess {
    DeviceDiscoveryProcess {
        process_id: 7,
        name: "neutral.exe".to_owned(),
        executable_path: Some(PathBuf::from("C:/fixture/neutral.exe")),
        command_line: Some("neutral.exe -v 0".to_owned()),
    }
}

fn stopped_state() -> InstanceState {
    InstanceState {
        process_started: false,
        android_started: false,
        adb_port: None,
        player_state: None,
        launch_err_code: 0,
        launch_err_msg: String::new(),
    }
}

impl MumuManagerRunner for Replay {
    fn control(&mut self, args: &[&str]) -> DeviceResult<MumuControlOutput> {
        self.seen.push((
            self.clock,
            Seen::Control(args.iter().map(|arg| (*arg).to_owned()).collect()),
        ));
        self.clock += seconds(1);
        self.info_silent_until = Some(self.clock + self.info_silent_for);
        Ok(MumuControlOutput {
            exit_code: self.exit_code,
            status: format!("exit code {:?}", self.exit_code),
            stdout: self.stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }

    fn read_state(&mut self, instance_index: u16) -> DeviceResult<InstanceState> {
        assert_eq!(instance_index, 0);
        if self
            .info_silent_until
            .is_some_and(|silent_until| self.clock < silent_until)
        {
            self.seen.push((self.clock, Seen::Info(None)));
            self.clock += MUMU_MANAGER_COMMAND_TIMEOUT;
            return Err(
                DeviceError::fatal("MuMuManager info -v 0 timed out").with_diagnostic(
                    DeviceErrorCategory::BackendLaunch,
                    MUMU_MANAGER_TIMEOUT_STAGE,
                ),
            );
        }
        self.seen.push((self.clock, Seen::Info(Some(true))));
        Ok(stopped_state())
    }

    fn list_processes(&mut self, bound: Duration) -> DeviceResult<Vec<DeviceDiscoveryProcess>> {
        let at = self.clock;
        self.seen.push((at, Seen::Listing(bound)));
        self.clock += seconds(2);
        let Some(until) = self.device_process_until else {
            return Err(DeviceError::fatal("the process listing is unavailable"));
        };
        let mut processes = vec![device_process(31, 1), unrelated_process()];
        if at < until {
            processes.push(device_process(30, 0));
        }
        Ok(processes)
    }

    fn now(&mut self) -> Duration {
        self.clock
    }

    fn sleep(&mut self, duration: Duration) {
        self.clock += duration;
    }
}

fn stage(error: &DeviceError) -> Option<&str> {
    error.diagnostic().map(|diagnostic| diagnostic.stage())
}

/// RB-D2, the 10-09 replay: `info` does not answer for 30 s while the instance stops, and the
/// vendor's device process with `-v 0` stays until 40 s. The Stop is confirmed only after both
/// conditions hold; the process listing runs only after `info` reported the process stopped,
/// at most once per `MUMU_MANAGER_PROCESS_LIST_INTERVAL`, each within
/// `MUMU_MANAGER_PROCESS_LIST_TIMEOUT`.
#[test]
fn gate_a_stop_is_confirmed_only_when_info_reports_it_stopped_and_no_device_process_is_left() {
    let mut replay = Replay::new(seconds(30), Some(seconds(40)));
    let outcome = control_instance_with(
        &mut replay,
        0,
        EmulatorInstanceAction::Stop,
        MUMU_MANAGER_STATE_WAIT_STOP,
    )
    .expect("the Stop is confirmed");
    assert_eq!(
        outcome.device_process_present,
        Some(DeviceProcessPresence::Absent),
        "{:?}",
        replay.seen
    );
    assert!(!outcome.process_started);
    assert_eq!(
        replay.controls(),
        [["control", "-v", "0", "shutdown"]
            .map(str::to_owned)
            .to_vec()]
    );
    let first_stopped = replay
        .first_stopped_answer()
        .expect("info reported the process stopped");
    let first_listing = replay
        .seen
        .iter()
        .position(|(_, seen)| matches!(seen, Seen::Listing(_)))
        .expect("a process listing ran");
    assert!(
        first_listing > first_stopped,
        "the listing runs only after info reported the process stopped: {:?}",
        replay.seen
    );
    let listings = replay.listings();
    assert!(
        listings
            .iter()
            .all(|(_, bound)| *bound == MUMU_MANAGER_PROCESS_LIST_TIMEOUT),
        "{listings:?}"
    );
    assert!(
        listings
            .windows(2)
            .all(|pair| pair[1].0.saturating_sub(pair[0].0) >= MUMU_MANAGER_PROCESS_LIST_INTERVAL),
        "{listings:?}"
    );
    let (confirming, _) = *listings.last().expect("the confirming listing");
    assert!(
        confirming >= seconds(40),
        "confirmed only once the device process left: {listings:?}"
    );
    assert!(outcome.elapsed_ms >= 40_000, "{}", outcome.elapsed_ms);
}

/// RB-D6: when the process listing is unavailable, `info` confirms the Stop and the outcome
/// records `device_process_present=unknown`; a failed listing never becomes a timeout.
#[test]
fn gate_a_stop_whose_process_listing_fails_is_confirmed_by_info_with_the_presence_unknown() {
    let mut replay = Replay::new(seconds(30), None);
    let outcome = control_instance_with(
        &mut replay,
        0,
        EmulatorInstanceAction::Stop,
        MUMU_MANAGER_STATE_WAIT_STOP,
    )
    .expect("info confirms the Stop; a failed listing is never a timeout");
    assert_eq!(
        outcome.device_process_present,
        Some(DeviceProcessPresence::Unknown),
        "{:?}",
        replay.seen
    );
    assert!(!outcome.process_started);
    assert_eq!(replay.listings().len(), 1, "{:?}", replay.seen);
}

/// `contracts/emulator-control.md` "Tool dispatch, timeouts and wait criteria" and the 6.5.7.0
/// observations: `info -v <index>` is read in the flat and in the map shape; `adb_port` and
/// `player_state` may be absent; the booleans and a consistent `index` are required;
/// `launch_err_code` is 0 when absent; a stopped instance's flat object carries `error_code`,
/// which is not an envelope.
#[test]
fn gate_the_info_document_is_read_in_both_shapes_with_its_required_and_optional_fields() {
    let running = json!({
        "index": "0",
        "name": "fixture",
        "is_process_started": true,
        "is_android_started": true,
        "adb_host_ip": "127.0.0.1",
        "adb_port": 16384,
        "player_state": "start_finished"
    });
    let running_state = InstanceState {
        process_started: true,
        android_started: true,
        adb_port: Some(16384),
        player_state: Some("start_finished".to_owned()),
        launch_err_code: 0,
        launch_err_msg: String::new(),
    };
    let stopped = json!({
        "android_version": "12",
        "created_timestamp": 1,
        "disk_size_bytes": 1,
        "error_code": 0,
        "hyperv_enabled": false,
        "index": "0",
        "info_source": "rpc",
        "is_android_started": false,
        "is_main": true,
        "is_process_started": false,
        "name": "fixture"
    });
    let rows: Vec<(serde_json::Value, Result<InstanceState, &str>)> = vec![
        (running.clone(), Ok(running_state.clone())),
        (json!({ "0": running }), Ok(running_state)),
        (stopped.clone(), Ok(stopped_state())),
        (json!({ "0": stopped }), Ok(stopped_state())),
        (
            json!({
                "index": "0",
                "is_process_started": true,
                "is_android_started": false,
                "adb_port": null,
                "player_state": null
            }),
            Ok(InstanceState {
                process_started: true,
                ..stopped_state()
            }),
        ),
        (
            json!({ "index": "0", "is_android_started": false }),
            Err("mumu_manager.shape"),
        ),
        (
            json!({ "index": "0", "is_process_started": false }),
            Err("mumu_manager.shape"),
        ),
        (
            json!({ "is_process_started": false, "is_android_started": false }),
            Err("mumu_manager.shape"),
        ),
        (
            json!({ "index": "1", "is_process_started": false, "is_android_started": false }),
            Err("mumu_manager.shape"),
        ),
        (
            json!({ "index": "0", "is_process_started": "false", "is_android_started": false }),
            Err("mumu_manager.shape"),
        ),
    ];
    for (document, expected) in rows {
        let read = parse_instance_document(&document, 0);
        match expected {
            Ok(state) => assert_eq!(read.expect("a readable document"), state, "{document}"),
            Err(expected_stage) => {
                let error = read.expect_err("a refused document");
                assert_eq!(stage(&error), Some(expected_stage), "{document}");
            }
        }
    }
}

/// `contracts/emulator-control.md` "Tool dispatch, timeouts and wait criteria": an
/// `{"errcode","errmsg"}` envelope with `errcode != 0` is a failure; `{"errcode": 0}` is the
/// success acknowledgment and never a failure, in `info` and in `control` alike.
#[test]
fn gate_errcode_zero_is_the_acknowledgment_and_any_other_errcode_fails() {
    for (object, fails) in [
        (json!({ "errcode": 0, "errmsg": "" }), false),
        (json!({ "errcode": 1, "errmsg": "refused" }), true),
        (json!({ "errcode": -1 }), true),
        (json!({ "error_code": 0, "index": "0" }), false),
        (json!({}), false),
    ] {
        let map = object.as_object().expect("an object");
        assert_eq!(errcode_envelope(map).is_some(), fails, "{object}");
    }
    let refused = parse_instance_document(&json!({ "errcode": 3, "errmsg": "refused" }), 0)
        .expect_err("an info envelope with errcode 3");
    assert_eq!(stage(&refused), Some("mumu_manager.errcode"));
    parse_instance_document(
        &json!({
            "errcode": 0,
            "errmsg": "",
            "index": "0",
            "is_process_started": false,
            "is_android_started": false
        }),
        0,
    )
    .expect("errcode 0 in an info answer is no failure");
    for (stdout, exit_code, expected_stage) in [
        (r#"{"errcode": 0, "errmsg": ""}"#, Some(0), None),
        (
            r#"{"errcode": 2, "errmsg": "refused"}"#,
            Some(0),
            Some("mumu_manager.control_errcode"),
        ),
        (
            r#"{"errcode": 1, "errmsg": "refused"}"#,
            Some(1),
            Some("mumu_manager.control_exit"),
        ),
    ] {
        let mut replay =
            Replay::new(Duration::ZERO, Some(Duration::ZERO)).control_answer(stdout, exit_code);
        let result = control_instance_with(
            &mut replay,
            0,
            EmulatorInstanceAction::Stop,
            MUMU_MANAGER_STATE_WAIT_STOP,
        );
        match expected_stage {
            None => {
                result.expect("the acknowledgment lets the wait proceed");
            }
            Some(expected_stage) => {
                let failure = result.expect_err("a refused control");
                assert_eq!(stage(&failure.error), Some(expected_stage), "{stdout}");
                assert_eq!(failure.exit_code, exit_code, "{stdout}");
            }
        }
    }
}
