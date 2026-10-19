// SPDX-License-Identifier: AGPL-3.0-only

// #411 A15 (v0.12.1-rc.2; host gate key 106): a run that fails with
// `contained_task_step_unconfirmed` carries, on its terminal and on its receipt, the kernel's
// values (the operation, its destination pages, its attempts and the page its last frame
// matched) and `artifact_id`, the frame of the run's `Failure` pin that `capture.summary_committed`
// records. Every expected value comes from the requirement lines of #411 A15.
//
// The neutral package's one guarded step goes from `home` to `terminal` and is not retried; the
// screen stays on `home` after the click (the tap was ignored), so the step is unconfirmed.

use super::*;
use serde_json::json;

const ALIAS: &str = "neutral.instance";
const STEP_NOT_CONFIRMED: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskStepUnconfirmed.as_str();

#[test]
fn a15_the_terminal_and_the_receipt_carry_the_values_and_the_failure_frame() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("unconfirmed.zip");
    fs::write(&package, neutral_non_retryable_destination_package(false)).expect("write package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    drop(client);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("run events");
    host.close().expect("close runtime host");

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let failure_frames = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) => Some(summary),
            _ => None,
        })
        .flat_map(|summary| summary.summary().pinned())
        .filter(|pin| pin.reason() == PinnedFrameReason::Failure)
        .filter_map(|pin| pin.artifact())
        .map(|artifact| serde_json::to_value(artifact.artifact_id).expect("artifact id"))
        .collect::<Vec<_>>();
    let [failure_frame] = failure_frames.as_slice() else {
        panic!("one Failure pin with its frame: {failure_frames:?}");
    };
    let terminals = events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskFailed)
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted {
                    outcome: Some(outcome),
                    ..
                } => Some(outcome.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let [terminal] = terminals.as_slice() else {
        panic!("one failed terminal with its outcome: {terminals:?}");
    };

    for (carrier, outcome) in [("terminal", terminal), ("receipt", receipt.outcome())] {
        assert_eq!(outcome.code().as_str(), STEP_NOT_CONFIRMED, "{carrier}");
        let value = |key: &str| outcome.value(key).cloned();
        assert_eq!(
            value("pack_operation_id"),
            Some(json!("open_terminal")),
            "{carrier}"
        );
        assert_eq!(value("page_ids"), Some(json!(["terminal"])), "{carrier}");
        assert_eq!(value("attempts"), Some(json!(1)), "{carrier}");
        assert_eq!(value("page_id"), Some(json!("neutral/home")), "{carrier}");
        assert_eq!(
            value("artifact_id").as_ref(),
            Some(failure_frame),
            "{carrier}"
        );
    }
}
