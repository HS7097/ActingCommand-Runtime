// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn direct_runtime_run_derives_sampling_seed_from_issued_run_identity() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_region_contained_task_package();
    let package_path = root.path().join("direct-region-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
                .expect("contained task request"),
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            executed_steps: 1,
            ..
        })
    ));
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let effect = events
        .iter()
        .find(|event| event.event_type == EventType::TaskEffectIntent)
        .expect("direct effect intent");
    let ProjectionPayload::Full(payload) = &effect.payload else {
        panic!("direct effect intent must retain full payload")
    };
    let EventPayload::Task(TaskPayload::Semantic(payload)) = payload.as_ref() else {
        panic!("direct effect intent must retain typed task payload")
    };
    let TaskSemanticFact::EffectIntent { action, .. } = payload.fact() else {
        panic!("direct effect intent must retain the sampled action")
    };
    let sampling = payload
        .sampling()
        .expect("direct production run sampling evidence");
    let run_id = effect.links.run_id().expect("direct effect run id");
    let action_id = effect.links.action_id().expect("direct effect action id");
    let expected_run_seed =
        expected_contained_task_sampling_seed(&("xorshift64_uniform_rect_v1/run", run_id));
    let expected_action_seed = expected_contained_task_sampling_seed(&(
        "xorshift64_uniform_rect_v1/action",
        expected_run_seed,
        action_id,
    ));
    assert_eq!(sampling.action_seed(), expected_action_seed);
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::Xorshift64UniformRectV1
    );
    assert_eq!(sampling.source_regions().len(), 1);
    assert_eq!(sampling.source_regions()[0].x(), 0);
    assert_eq!(sampling.source_regions()[0].y(), 0);
    assert_eq!(sampling.source_regions()[0].width(), 2);
    assert_eq!(sampling.source_regions()[0].height(), 1);
    let mut expected_state = if expected_action_seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        expected_action_seed
    };
    let mut next_expected = || {
        expected_state ^= expected_state << 13;
        expected_state ^= expected_state >> 7;
        expected_state ^= expected_state << 17;
        expected_state
    };
    let expected_x = (next_expected() % 2) as i32;
    next_expected();
    assert_eq!(
        action,
        &InputAction::Tap {
            x: expected_x,
            y: 0
        }
    );
    assert_eq!(
        state
            .input_actions
            .lock()
            .expect("fake input actions lock")
            .as_slice(),
        std::slice::from_ref(action),
        "the direct durable sampled action must equal the backend action"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn production_runtime_missing_sampling_seed_fails_before_input() {
    struct MissingSeedRuntime {
        frame: Frame,
        input_calls: usize,
    }

    impl ContainedTaskRuntime for MissingSeedRuntime {
        type Error = RuntimeHostError;

        fn capture(&mut self) -> Result<Frame, Self::Error> {
            Ok(self.frame.clone())
        }

        fn action_seed(
            &mut self,
            _step_index: u32,
            _operation_label: &str,
        ) -> Result<Option<u64>, Self::Error> {
            crate::host::require_contained_task_sampling_run_seed(None).map(Some)
        }

        fn input(&mut self, _action: InputAction) -> Result<(), Self::Error> {
            self.input_calls += 1;
            Ok(())
        }

        fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    let package = neutral_region_contained_task_package();
    let expected = ExternalExpectedSha256::parse_hex(
        &actingcommand_pack_containment::Sha256Hash::digest(&package).to_string(),
    )
    .expect("package hash");
    let prepared = PreparedContainedTask::load("neutral.instance", &package, expected)
        .expect("prepared contained task");
    let mut runtime = MissingSeedRuntime {
        frame: Frame::from_pixels(
            2,
            1,
            vec![255, 0, 0, 0, 255, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("home frame"),
        input_calls: 0,
    };

    let error = prepared
        .run(&mut runtime)
        .expect_err("production seed guard must reject a region effect before input");
    let ContainedTaskRunError::Boundary(error) = error else {
        panic!("missing production seed must surface as a typed boundary error")
    };
    assert_eq!(error.code(), "contained_task_sampling_seed_missing");
    assert_eq!(error.operation(), "derive_contained_task_action_seed");
    assert!(error.is_fatal());
    assert_eq!(
        runtime.input_calls, 0,
        "missing RuntimeHost seed must fail before backend input"
    );
}

#[test]
fn execution_kernel_no_seed_caller_retains_center_fallback() {
    struct NoSeedRuntime {
        frames: VecDeque<Frame>,
        last_frame: Frame,
        actions: Vec<InputAction>,
        traces: Vec<ContainedTaskTrace>,
    }

    impl ContainedTaskRuntime for NoSeedRuntime {
        type Error = &'static str;

        fn capture(&mut self) -> Result<Frame, Self::Error> {
            Ok(self
                .frames
                .pop_front()
                .unwrap_or_else(|| self.last_frame.clone()))
        }

        fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
            self.actions.push(action);
            Ok(())
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            self.traces.push(trace);
            Ok(())
        }
    }

    let package = neutral_region_contained_task_package();
    let expected = ExternalExpectedSha256::parse_hex(
        &actingcommand_pack_containment::Sha256Hash::digest(&package).to_string(),
    )
    .expect("package hash");
    let prepared = PreparedContainedTask::load("neutral.instance", &package, expected)
        .expect("prepared contained task");
    let home = Frame::from_pixels(
        2,
        1,
        vec![255, 0, 0, 0, 255, 0],
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("home frame");
    let terminal = Frame::from_pixels(
        2,
        1,
        vec![0, 0, 255, 0, 255, 0],
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("terminal frame");
    let mut runtime = NoSeedRuntime {
        frames: [home, terminal.clone()].into(),
        last_frame: terminal,
        actions: Vec::new(),
        traces: Vec::new(),
    };

    let outcome = prepared.run(&mut runtime).expect("offline no-seed run");
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    assert_eq!(runtime.actions, vec![InputAction::Tap { x: 1, y: 0 }]);
    assert!(runtime.traces.iter().any(|trace| matches!(
        trace,
        ContainedTaskTrace::EffectIntent {
            action: InputAction::Tap { x: 1, y: 0 },
            sampling: None,
            ..
        }
    )));
}
