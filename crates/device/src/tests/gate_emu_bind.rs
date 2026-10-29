// SPDX-License-Identifier: AGPL-3.0-only

//! EMU-BIND A and C (0.12.27; production 10-29 after the 05:23 reboot): during a Start's
//! readiness wait a failed `info` read is no observation yet, and a failure carries the failing
//! read's own text. Replayed over the injected vendor surface (EXEC-I1) on the replay's own
//! clock: a read and a sleep advance it, nothing waits.

use crate::discovery::DeviceDiscoveryProcess;
use crate::mumu_manager::{
    InstanceState, MUMU_MANAGER_STATE_WAIT_START, MUMU_MANAGER_STATE_WAIT_STOP, MumuControlOutput,
    MumuManagerRunner, control_instance_with,
};
use crate::{DeviceError, DeviceErrorCategory, DeviceResult};
use actingcommand_contract::EmulatorInstanceAction;
use std::collections::VecDeque;
use std::time::Duration;

/// The text a failed `info` read carries: its exit status, stdout and stderr, as `run_json`
/// writes it.
const INFO_FAILURE: &str = "MuMuManager info -v 0 failed with exit code: 1 (no errcode envelope)\nstdout:\nfixture info stdout\nstderr:\nfixture info stderr";

/// The vendor surface: `control` answers exit 0 and `{"errcode": 0}` after 1 s; each `info`
/// read takes 1 s and answers the next scripted read, a non-zero exit (`None`) or a state;
/// with the script used up it answers `rest`.
struct Replay {
    clock: Duration,
    reads: VecDeque<Option<InstanceState>>,
    rest: Option<InstanceState>,
    controls: Vec<Vec<String>>,
    info_reads: usize,
}

impl Replay {
    fn new(reads: Vec<Option<InstanceState>>, rest: Option<InstanceState>) -> Self {
        Self {
            clock: Duration::ZERO,
            reads: reads.into(),
            rest,
            controls: Vec::new(),
            info_reads: 0,
        }
    }
}

fn running_state() -> InstanceState {
    InstanceState {
        process_started: true,
        android_started: true,
        adb_port: Some(40001),
        player_state: Some("start_finished".to_owned()),
        launch_err_code: 0,
        launch_err_msg: String::new(),
    }
}

impl MumuManagerRunner for Replay {
    fn control(&mut self, args: &[&str]) -> DeviceResult<MumuControlOutput> {
        self.controls
            .push(args.iter().map(|arg| (*arg).to_owned()).collect());
        self.clock += Duration::from_secs(1);
        Ok(MumuControlOutput {
            exit_code: Some(0),
            status: "exit code: 0".to_owned(),
            stdout: br#"{"errcode": 0, "errmsg": ""}"#.to_vec(),
            stderr: Vec::new(),
        })
    }

    fn read_state(&mut self, instance_index: u16) -> DeviceResult<InstanceState> {
        assert_eq!(instance_index, 0);
        self.info_reads += 1;
        self.clock += Duration::from_secs(1);
        let read = self.reads.pop_front().unwrap_or_else(|| self.rest.clone());
        read.ok_or_else(|| {
            DeviceError::fatal(INFO_FAILURE).with_diagnostic(
                DeviceErrorCategory::ChildExit,
                crate::codes::DeviceStage::MumuManagerExit.location(),
            )
        })
    }

    fn list_processes(&mut self, _bound: Duration) -> DeviceResult<Vec<DeviceDiscoveryProcess>> {
        Ok(Vec::new())
    }

    fn now(&mut self) -> Duration {
        self.clock
    }

    fn sleep(&mut self, duration: Duration) {
        self.clock += duration;
    }
}

fn stage(error: &DeviceError) -> Option<&str> {
    error
        .diagnostic()
        .map(|diagnostic| diagnostic.stage().as_str())
}

/// T1, the 10-29 case (A): the launch succeeds, one `info` read exits non-zero while the VM comes
/// up, then `info` reads the instance running: the Start completes, and the launch was sent once.
#[test]
fn emu_bind_t1_a_start_survives_one_failed_info_read_and_completes() {
    let mut replay = Replay::new(vec![None], Some(running_state()));
    let outcome = control_instance_with(
        &mut replay,
        0,
        EmulatorInstanceAction::Start,
        MUMU_MANAGER_STATE_WAIT_START,
    )
    .expect("one failed info read does not fail the Start");
    assert!(outcome.running);
    assert_eq!(outcome.adb_port, Some(40001));
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(
        replay.controls,
        [["control", "-v", "0", "launch"].map(str::to_owned).to_vec()],
        "the launch is sent once"
    );
    assert_eq!(replay.info_reads, 2);
}

/// A and C: a Start whose every `info` read fails waits to its deadline and ends
/// `mumu_manager_wait_timeout` carrying the last failed read's own text; a Stop's failed read
/// still ends the wait at once, and carries that read's text after the control output too.
#[test]
fn emu_bind_a_failed_reads_wait_to_the_deadline_and_a_failure_carries_the_reads_text() {
    let mut replay = Replay::new(Vec::new(), None);
    let failure = control_instance_with(
        &mut replay,
        0,
        EmulatorInstanceAction::Start,
        MUMU_MANAGER_STATE_WAIT_START,
    )
    .expect_err("no read answered");
    assert_eq!(stage(&failure.error), Some("mumu_manager_wait_timeout"));
    assert!(
        failure.elapsed_ms >= 120_000,
        "the wait went on to its deadline: {}",
        failure.elapsed_ms
    );
    assert!(replay.info_reads > 1, "{}", replay.info_reads);
    assert_eq!(failure.exit_code, Some(0), "the launch's own exit code");
    assert!(
        failure
            .output_summary
            .contains(r#"{"errcode": 0, "errmsg": ""}"#),
        "{}",
        failure.output_summary
    );
    assert!(
        failure
            .output_summary
            .contains("info: MuMuManager info -v 0 failed with exit code: 1"),
        "{}",
        failure.output_summary
    );
    assert!(
        failure.output_summary.contains("fixture info stderr"),
        "{}",
        failure.output_summary
    );

    let mut replay = Replay::new(Vec::new(), None);
    let failure = control_instance_with(
        &mut replay,
        0,
        EmulatorInstanceAction::Stop,
        MUMU_MANAGER_STATE_WAIT_STOP,
    )
    .expect_err("a Stop's failed read ends the wait");
    assert_eq!(stage(&failure.error), Some("mumu_manager_exit"));
    assert_eq!(replay.info_reads, 1, "the Stop's wait ended at once");
    assert!(
        failure
            .output_summary
            .contains("info: MuMuManager info -v 0 failed with exit code: 1"),
        "{}",
        failure.output_summary
    );
}
