// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    ApplicationLifecycleAction, IdentifierIssuer, InputAction, InstanceId, Sensitivity,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct FakeState {
    input_opens: usize,
    capture_opens: usize,
    input_calls: usize,
    capture_calls: usize,
    capture_closes: usize,
    input_closes: usize,
    application_calls: usize,
    application_observed_input_closes: usize,
    application_observed_capture_closes: usize,
    fail_input_open: bool,
    fail_capture_open: bool,
    fail_input: bool,
    fail_capture: bool,
    fail_application: bool,
    fail_close: bool,
    panic_input: bool,
    panic_capture: bool,
}

struct FakeProvider {
    state: Arc<Mutex<FakeState>>,
    instances: BTreeMap<String, ResolvedExecutionInstance>,
}

impl FakeProvider {
    fn new(state: Arc<Mutex<FakeState>>, instances: &[(&str, InstanceId, &str)]) -> Self {
        Self {
            state,
            instances: instances
                .iter()
                .map(|(alias, instance_id, endpoint)| {
                    (
                        (*alias).to_string(),
                        ResolvedExecutionInstance::new(*instance_id, *endpoint),
                    )
                })
                .collect(),
        }
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        self.instances.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        self.instances.get(instance_alias).cloned()
    }

    fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        let mut state = self.state.lock().expect("state");
        state.input_opens += 1;
        if state.fail_input_open {
            return Err(DeviceError::fatal("private input open detail"));
        }
        drop(state);
        Ok(Box::new(FakeInput {
            state: Arc::clone(&self.state),
        }))
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        let mut state = self.state.lock().expect("state");
        state.capture_opens += 1;
        if state.fail_capture_open {
            return Err(DeviceError::fatal("private capture open detail"));
        }
        drop(state);
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&self.state),
        }))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        let mut state = self.state.lock().expect("state");
        state.application_calls += 1;
        state.application_observed_input_closes = state.input_closes;
        state.application_observed_capture_closes = state.capture_closes;
        if state.fail_application {
            Err(DeviceError::fatal("private application failure"))
        } else {
            Ok(())
        }
    }
}

struct FakeInput {
    state: Arc<Mutex<FakeState>>,
}

impl FakeInput {
    fn execute(&mut self) -> DeviceResult<()> {
        let mut state = self.state.lock().expect("state");
        state.input_calls += 1;
        if state.panic_input {
            drop(state);
            panic!("private input panic detail");
        }
        if state.fail_input {
            Err(DeviceError::transient("private input failure detail"))
        } else {
            Ok(())
        }
    }
}

impl InputBackend for FakeInput {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.execute()
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        self.execute()
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.execute()
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        self.execute()
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        self.execute()
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.execute()
    }

    fn close(&mut self) -> DeviceResult<()> {
        let mut state = self.state.lock().expect("state");
        state.input_closes += 1;
        if state.fail_close {
            Err(DeviceError::fatal("private close failure detail"))
        } else {
            Ok(())
        }
    }
}

struct FakeCapture {
    state: Arc<Mutex<FakeState>>,
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let mut state = self.state.lock().expect("state");
        state.capture_calls += 1;
        assert!(!state.panic_capture, "private capture panic detail");
        if state.fail_capture {
            return Err(DeviceError::transient("private capture failure detail"));
        }
        drop(state);
        Frame::from_pixels(
            2,
            1,
            vec![1, 2, 3, 4, 5, 6],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        self.state.lock().expect("state").capture_closes += 1;
    }
}

fn instance() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance")
        .transport()
}

fn kernel(state: Arc<Mutex<FakeState>>, instances: &[(&str, InstanceId, &str)]) -> ExecutionKernel {
    ExecutionKernel::new(Arc::new(FakeProvider::new(state, instances)))
}

#[test]
fn input_and_capture_open_lazily_once_and_share_one_daemon_session() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let instance_id = instance();
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance_id, "private-a")]);
    assert_eq!(
        kernel.resolve("node.a").expect("resolve").instance_id(),
        instance_id
    );
    assert_eq!(state.lock().expect("state").input_opens, 0);

    kernel
        .input("node.a", InputAction::Reset)
        .expect("first input");
    kernel
        .input("node.a", InputAction::Tap { x: 1, y: 2 })
        .expect("second input");
    let first = kernel.capture("node.a").expect("first capture");
    let second = kernel.capture("node.a").expect("second capture");
    assert_eq!((first.width, first.height), (2, 1));
    assert_eq!((second.width, second.height), (2, 1));

    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_opens, 1);
    assert_eq!(snapshot.capture_opens, 1);
    assert_eq!(snapshot.input_calls, 2);
    assert_eq!(snapshot.capture_calls, 2);
    drop(snapshot);
    kernel.close().expect("close");
    kernel.close().expect("idempotent close");
    assert_eq!(state.lock().expect("state").input_closes, 1);
}

#[test]
fn application_lifecycle_is_serialized_by_the_daemon_session_and_invalidates_backends() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let kernel = kernel(
        Arc::clone(&state),
        &[("neutral.instance", instance(), "private-endpoint")],
    );
    kernel
        .input("neutral.instance", InputAction::Reset)
        .expect("open input");
    kernel.capture("neutral.instance").expect("open capture");

    kernel
        .control_application("neutral.instance", ApplicationLifecycleAction::Restart)
        .expect("application restart");

    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.application_calls, 1);
    assert_eq!(snapshot.application_observed_input_closes, 1);
    assert_eq!(snapshot.application_observed_capture_closes, 1);
    drop(snapshot);
    kernel.close().expect("close");
}

#[test]
fn instance_sessions_are_partitioned_and_identity_mismatch_is_fatal() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let first = instance();
    let second = instance();
    let kernel = kernel(
        Arc::clone(&state),
        &[
            ("node.a", first, "private-a"),
            ("node.b", second, "private-b"),
            ("node.a.shadow", first, "different-private-endpoint"),
        ],
    );
    kernel
        .input("node.a", InputAction::Reset)
        .expect("first instance");
    kernel
        .input("node.b", InputAction::Reset)
        .expect("second instance");
    assert_eq!(state.lock().expect("state").input_opens, 2);
    assert_eq!(
        kernel
            .capture("node.a.shadow")
            .expect_err("identity mismatch")
            .code(),
        "execution_instance_identity_mismatch"
    );
    assert_eq!(
        kernel.capture("missing").expect_err("unknown").code(),
        "execution_instance_unknown"
    );
    kernel.close().expect("close");
}

#[test]
fn input_failure_returns_original_error_then_later_input_uses_fresh_session() {
    let state = Arc::new(Mutex::new(FakeState {
        fail_input: true,
        ..FakeState::default()
    }));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    let error = kernel
        .input("node.a", InputAction::Reset)
        .expect_err("input failure");
    assert_eq!(error.code(), "input_backend_operation_failed");
    assert!(!error.is_fatal());
    assert!(!format!("{error:?} {error}").contains("private input failure detail"));
    {
        let mut snapshot = state.lock().expect("state");
        snapshot.fail_input = false;
        assert_eq!(snapshot.input_opens, 1);
        assert_eq!(snapshot.input_calls, 1);
        assert_eq!(snapshot.input_closes, 1);
    }
    kernel
        .input("node.a", InputAction::Reset)
        .expect("separate input uses a fresh session");
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_opens, 2);
    assert_eq!(snapshot.input_calls, 2);
    assert_eq!(snapshot.input_closes, 1);
    drop(snapshot);
    kernel.close().expect("close replacement session");
    assert_eq!(state.lock().expect("state").input_closes, 2);
}

#[test]
fn segmented_swipe_capability_failure_preserves_device_detail() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    let error = kernel
        .input(
            "node.a",
            InputAction::SingleTouchDragWithVerticalBrakeV1 {
                x1: 10,
                y1: 200,
                x2: 100,
                y2: 200,
                x3: 100,
                y3: 100,
                horizontal_duration_ms: 200,
                corner_hold_ms: 150,
                brake_distance_px: 100,
                brake_duration_ms: 200,
                slope_in: 2,
                slope_out: 0,
            },
        )
        .expect_err("segmented swipe capability rejection");

    assert_eq!(error.code(), "input_backend_operation_failed");
    let detail = error.diagnostic_detail().expect("capability detail");
    assert_eq!(detail.category(), "protocol");
    assert_eq!(detail.stage(), "input.segmented_swipe.capability");
    assert_eq!(detail.backend(), "input_backend");
    assert_eq!(detail.operation(), "segmented_swipe");
    assert_eq!(
        detail.message(),
        "selected input backend does not support segmented swipe"
    );
    assert_eq!(detail.declared_sensitivity(), Sensitivity::Internal);
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_calls, 0);
    assert_eq!(snapshot.input_closes, 1);
}

#[test]
fn failed_input_open_is_retired_before_separate_capture() {
    let state = Arc::new(Mutex::new(FakeState {
        fail_input_open: true,
        ..FakeState::default()
    }));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);

    let error = kernel
        .input("node.a", InputAction::Reset)
        .expect_err("input open failure");
    assert_eq!(error.code(), "input_backend_open_failed");
    assert!(error.is_fatal());
    assert!(!format!("{error:?} {error}").contains("private input open detail"));
    {
        let mut snapshot = state.lock().expect("state");
        assert_eq!(snapshot.input_opens, 1);
        assert_eq!(snapshot.input_calls, 0);
        assert_eq!(snapshot.capture_opens, 0);
        snapshot.fail_input_open = false;
    }

    let frame = kernel
        .capture("node.a")
        .expect("separate capture uses a fresh session");
    assert_eq!((frame.width, frame.height), (2, 1));
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_opens, 1);
    assert_eq!(snapshot.input_calls, 0);
    assert_eq!(snapshot.capture_opens, 1);
    assert_eq!(snapshot.capture_calls, 1);
    drop(snapshot);
    kernel.close().expect("close replacement session");
}

#[test]
fn capture_failure_returns_original_error_then_later_capture_uses_fresh_session() {
    let state = Arc::new(Mutex::new(FakeState {
        fail_capture: true,
        ..FakeState::default()
    }));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    kernel
        .input("node.a", InputAction::Reset)
        .expect("open input");
    let error = kernel.capture("node.a").expect_err("capture failure");
    assert_eq!(error.code(), "capture_backend_operation_failed");
    assert!(!error.is_fatal());
    {
        let mut snapshot = state.lock().expect("state");
        assert_eq!(snapshot.input_closes, 1);
        assert_eq!(snapshot.capture_opens, 1);
        assert_eq!(snapshot.capture_calls, 1);
        snapshot.fail_capture = false;
    }
    let frame = kernel
        .capture("node.a")
        .expect("separate capture uses a fresh session");
    assert_eq!((frame.width, frame.height), (2, 1));
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_opens, 1);
    assert_eq!(snapshot.input_closes, 1);
    assert_eq!(snapshot.capture_opens, 2);
    assert_eq!(snapshot.capture_calls, 2);
    drop(snapshot);
    kernel.close().expect("close replacement session");
    assert_eq!(state.lock().expect("state").capture_closes, 2);
}

#[test]
fn application_failure_returns_original_error_then_later_input_uses_fresh_session() {
    let state = Arc::new(Mutex::new(FakeState {
        fail_application: true,
        ..FakeState::default()
    }));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    kernel
        .input("node.a", InputAction::Reset)
        .expect("open input");
    kernel.capture("node.a").expect("open capture");

    let error = kernel
        .control_application("node.a", ApplicationLifecycleAction::Restart)
        .expect_err("application failure");
    assert_eq!(error.code(), "application_backend_operation_failed");
    assert!(error.is_fatal());
    {
        let mut snapshot = state.lock().expect("state");
        assert_eq!(snapshot.application_calls, 1);
        assert_eq!(snapshot.application_observed_input_closes, 1);
        assert_eq!(snapshot.application_observed_capture_closes, 1);
        snapshot.fail_application = false;
    }

    kernel
        .input("node.a", InputAction::Reset)
        .expect("separate input uses a fresh session");
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.application_calls, 1);
    assert_eq!(snapshot.input_opens, 2);
    assert_eq!(snapshot.input_calls, 2);
    drop(snapshot);
    kernel.close().expect("close replacement session");
}

#[test]
fn backend_open_and_close_failures_surface_without_private_details() {
    let input_state = Arc::new(Mutex::new(FakeState {
        fail_input_open: true,
        ..FakeState::default()
    }));
    let input_kernel = kernel(
        Arc::clone(&input_state),
        &[("node.a", instance(), "private-a")],
    );
    let error = input_kernel
        .input("node.a", InputAction::Reset)
        .expect_err("input open");
    assert_eq!(error.code(), "input_backend_open_failed");
    assert!(error.is_fatal());
    assert!(!format!("{error:?} {error}").contains("private input open detail"));
    input_kernel.close().expect("close failed-open kernel");

    let capture_state = Arc::new(Mutex::new(FakeState {
        fail_capture_open: true,
        ..FakeState::default()
    }));
    let capture_kernel = kernel(
        Arc::clone(&capture_state),
        &[("node.a", instance(), "private-a")],
    );
    assert_eq!(
        capture_kernel
            .capture("node.a")
            .expect_err("capture open")
            .code(),
        "capture_backend_open_failed"
    );
    capture_kernel.close().expect("close failed-open kernel");

    let close_state = Arc::new(Mutex::new(FakeState {
        fail_close: true,
        ..FakeState::default()
    }));
    let close_kernel = kernel(
        Arc::clone(&close_state),
        &[("node.a", instance(), "private-a")],
    );
    close_kernel
        .input("node.a", InputAction::Reset)
        .expect("open input");
    let error = close_kernel.close().expect_err("close failure");
    assert_eq!(error.code(), "input_backend_close_failed");
    assert!(!format!("{error:?} {error}").contains("private close failure detail"));
}

#[test]
fn backend_panic_is_caught_then_later_input_uses_fresh_session() {
    let state = Arc::new(Mutex::new(FakeState {
        panic_input: true,
        ..FakeState::default()
    }));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    let error = kernel
        .input("node.a", InputAction::Reset)
        .expect_err("panic must surface");
    assert_eq!(error.code(), "execution_session_response_lost");
    assert_eq!(error.secondary_code(), Some("execution_session_panicked"));
    assert!(error.is_fatal());
    assert!(!format!("{error:?} {error}").contains("private input panic detail"));
    {
        let mut snapshot = state.lock().expect("state");
        assert_eq!(snapshot.input_opens, 1);
        assert_eq!(snapshot.input_calls, 1);
        snapshot.panic_input = false;
    }
    kernel
        .input("node.a", InputAction::Reset)
        .expect("separate input uses a fresh session");
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_opens, 2);
    assert_eq!(snapshot.input_calls, 2);
    drop(snapshot);
    kernel.close().expect("close replacement session");
}

#[test]
fn stale_failed_session_cannot_evict_same_instance_replacement() {
    let state = Arc::new(Mutex::new(FakeState {
        fail_input: true,
        ..FakeState::default()
    }));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    let failed = kernel.session_for_test("node.a").expect("failed session");

    let error = kernel
        .input("node.a", InputAction::Reset)
        .expect_err("input failure");
    assert_eq!(error.code(), "input_backend_operation_failed");
    state.lock().expect("state").fail_input = false;

    let replacement = kernel
        .session_for_test("node.a")
        .expect("replacement session");
    assert!(!Arc::ptr_eq(&failed, &replacement));
    kernel
        .retire_failed_session_for_test(&failed)
        .expect("stale retirement is a no-op");
    let current = kernel.session_for_test("node.a").expect("current session");
    assert!(Arc::ptr_eq(&replacement, &current));

    kernel
        .input("node.a", InputAction::Reset)
        .expect("replacement remains usable");
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_opens, 2);
    assert_eq!(snapshot.input_calls, 2);
    drop(snapshot);
    kernel.close().expect("close replacement session");
}

#[test]
fn kernel_retirement_failure_keeps_the_operation_error_primary_and_visible() {
    let state = Arc::new(Mutex::new(FakeState {
        fail_input: true,
        ..FakeState::default()
    }));
    let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
    let failed = kernel.session_for_test("node.a").expect("failed session");
    let primary = failed
        .input(InputAction::Reset)
        .expect_err("direct session failure");
    assert_eq!(primary.code(), "input_backend_operation_failed");
    assert_eq!(primary.secondary_code(), None);
    let snapshot = state.lock().expect("state");
    assert_eq!(snapshot.input_opens, 1);
    assert_eq!(snapshot.input_calls, 1);
    assert_eq!(snapshot.input_closes, 1);
    drop(snapshot);

    let poison = catch_unwind(AssertUnwindSafe(|| kernel.poison_state_for_test()));
    assert!(poison.is_err());
    let error = kernel
        .finish_failed_session_for_test(&failed, primary)
        .expect_err("retirement lock failure");
    assert_eq!(error.code(), "input_backend_operation_failed");
    assert_eq!(
        error.secondary_code(),
        Some("execution_kernel_state_poisoned")
    );

    kernel.clear_state_poison_for_test();
    kernel.close().expect("close terminal session");
}

#[test]
fn drop_closes_daemon_owned_input_session() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    {
        let kernel = kernel(Arc::clone(&state), &[("node.a", instance(), "private-a")]);
        kernel
            .input("node.a", InputAction::Reset)
            .expect("open input");
    }
    assert_eq!(state.lock().expect("state").input_closes, 1);
}
