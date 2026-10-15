// SPDX-License-Identifier: AGPL-3.0-only

//! #398 R1 RD-3i: the floors a contained task's captures carry (model §5.2, test rows T3 and
//! T2's "no input after a stale capture").

use super::*;

fn run_neutral_task(state: &Arc<FakeState>) -> RuntimeReceipt {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package(true);
    fs::write(&package, &bytes).expect("write package");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::run_contained_task(
        "neutral.instance",
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
    ));
    client.send(&request)
}

#[test]
fn rd3i_t3_the_first_capture_after_an_input_has_its_completion_as_floor_and_polling_the_last_sequence()
 {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.stream_capture.store(true, Ordering::Release);

    let receipt = run_neutral_task(&state);

    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    let floors = state.capture_floors.lock().expect("capture floors").clone();
    let inputs = state
        .inputs_completed
        .lock()
        .expect("completed inputs")
        .clone();
    let [(input_completed_at, captures_before_input)] = inputs.as_slice() else {
        panic!("the task makes exactly one input: {inputs:?}");
    };
    assert!(
        floors.len() > *captures_before_input,
        "a capture follows the input: {floors:?}"
    );
    assert_eq!(
        floors[0], None,
        "the first capture has nothing to be newer than"
    );
    for (index, floor) in floors.iter().enumerate().skip(1) {
        let floor = floor.expect("every later capture has a floor");
        assert_eq!(
            floor.after_sequence,
            Some(index as u64),
            "capture {index} must be newer than the frame delivered before it"
        );
        if index == *captures_before_input {
            let after = floor
                .after
                .expect("the first capture after the input has the input's completion");
            assert!(after >= *input_completed_at, "capture {index}");
        } else {
            assert_eq!(floor.after, None, "capture {index}");
        }
    }
}

#[test]
fn rd3i_t2_a_stale_capture_after_an_input_is_followed_by_no_input() {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state.stream_capture.store(true, Ordering::Release);
    *state
        .capture_error_after_input
        .lock()
        .expect("capture error after input") = Some(DeviceError::transient(format!(
        "{}: no frame encoded after the capture floor arrived",
        actingcommand_device::StreamCaptureCode::CaptureFrameStale.as_str()
    )));

    let receipt = run_neutral_task(&state);

    assert!(
        !matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                ..
            })
        ),
        "a stale capture does not complete the task"
    );
    let inputs = state
        .inputs_completed
        .lock()
        .expect("completed inputs")
        .clone();
    assert_eq!(inputs.len(), 1, "no input after the stale capture");
    let (_, captures_before_input) = inputs[0];
    let floors = state.capture_floors.lock().expect("capture floors").clone();
    assert!(
        floors.len() > captures_before_input,
        "the capture after the input was attempted: {floors:?}"
    );
    assert!(
        floors[captures_before_input].is_some_and(|floor| floor.after.is_some()),
        "{floors:?}"
    );
}
