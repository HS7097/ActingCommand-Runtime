// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

// Authorized D01 callback regression: https://github.com/HS7097/ActingCommand-Runtime/pull/301#pullrequestreview-5121633182
// Zero-input Defect: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5553542252
// ZIF-D01: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5554095550
#[test]
fn fields_v1_callback_failures_keep_official_projection_and_fatal_boundaries() {
    use actingcommand_contract::{
        ArtifactRedactionState, EffectiveConfigurationFacts, EffectiveConfigurationRecord,
        EffectiveTimingSource,
    };
    use actingcommand_device::{
        AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureMumuContext,
        CaptureSelectionContext, DeviceTarget, InputSelectionContext, MaaTouchConfig,
        MumuInstallSource, TouchBackendChoice, TouchBackendConfig, TouchBackendName,
    };
    use actingcommand_recognition_pack::{
        OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
    };
    use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
    use serde_json::{Value, json};
    use std::io::Read;

    #[derive(Debug)]
    struct FieldEvidenceProvider(Arc<FakeVisionProvider>, Option<&'static str>);
    impl VisionProvider for FieldEvidenceProvider {
        fn require_ocr_model(
            &self,
            model_ref: &str,
            model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            self.0.require_ocr_model(model_ref, model_sha256)
        }

        fn require_nn_model(
            &self,
            model_ref: &str,
            model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            self.0.require_nn_model(model_ref, model_sha256)
        }

        fn read_text(
            &self,
            request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderResult, VisionProviderError> {
            self.0.read_text(request)
        }

        fn read_text_with_execution_evidence(
            &self,
            request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderObservation, VisionProviderError> {
            let model_ref = request.model_ref.to_owned();
            let model_sha256 = request.model_sha256.to_owned();
            let mut result = self.0.read_text(request)?;
            if let Some(text) = self.1 {
                result.text = text.to_owned();
                for block in &mut result.blocks {
                    block.text = text.to_owned();
                }
            }
            Ok(OcrProviderObservation {
                result,
                execution: Some(OcrProviderExecutionEvidence {
                    invocation_id: format!("ocr-{}", self.0.ocr_calls.load(Ordering::Acquire)),
                    session_id: "fields-session".to_owned(),
                    session_generation: 1,
                    requested_provider: OcrExecutionProviderKind::Cpu,
                    resolved_provider: OcrExecutionProviderKind::Cpu,
                    requested_cuda_ordinal: None,
                    requested_cuda_identity: None,
                    resolved_cuda_ordinal: None,
                    resolved_cuda_identity: None,
                    provider_implementation: "fixture-ocr".to_owned(),
                    provider_binary_sha256: "b".repeat(64),
                    runtime_version: "fixture-runtime".to_owned(),
                    model_ref,
                    model_sha256,
                    cpu_ep_registered: true,
                    cpu_fallback_disabled: false,
                    fallback_forbidden: true,
                    fallback_observed: None,
                    complete: true,
                }),
            })
        }

        fn classify(
            &self,
            request: NnProviderRequest<'_>,
        ) -> Result<NnProviderResult, VisionProviderError> {
            self.0.classify(request)
        }
    }

    // Specification criterion: Workflow #269 SAVED-ARTIFACT-OCR-v1.
    // One historical frame passes the native source owner and the same fields provider seam.
    {
        use actingcommand_contract::{
            SavedArtifactOcrRequest, SavedArtifactOcrSource, TerminalEvent,
        };
        let original = TempDir::new().unwrap();
        let source_host = RuntimeHost::start(
            config(&original),
            Arc::new(FakeProvider::one(
                "node.a",
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .unwrap();
        let source_client = RuntimeClient::connect(RuntimeClientConfig::new(
            original.path(),
            EventActor::Lab,
            EventSource::Lab,
        ))
        .unwrap();
        let flow = source_client.observe_readonly("node.a").unwrap();
        let locate = |kind| {
            flow.events()
                .iter()
                .find(|event| {
                    event.event_type == kind
                        && (kind == EventType::CaptureCompleted
                            || event
                                .artifacts
                                .iter()
                                .any(|artifact| artifact.kind == ArtifactKind::CaptureFrame))
                })
                .unwrap()
        };
        let created = locate(EventType::ArtifactCreated);
        let verified = locate(EventType::ArtifactVerified);
        let captured = locate(EventType::CaptureCompleted);
        let reference = created.artifacts[0].clone();
        let binding = SavedArtifactOcrSource {
            state_root: original.path().to_str().unwrap().into(),
            through_sequence: captured.sequence,
            frame_id: reference.frame_id.unwrap(),
            artifact: reference,
            created: TerminalEvent {
                sequence: created.sequence,
                event_id: created.event_id,
            },
            verified: TerminalEvent {
                sequence: verified.sequence,
                event_id: verified.event_id,
            },
            captured: TerminalEvent {
                sequence: captured.sequence,
                event_id: captured.event_id,
            },
        };
        drop(source_client);
        source_host.close().unwrap();
        let original_ledger = fs::read(original.path().join("runtime-state.sqlite")).unwrap();
        let original_image =
            fs::read(original.path().join(binding.artifact.object_key().unwrap())).unwrap();
        let package = neutral_post_admission_ocr_contained_task_package();
        let package_path = original.path().join("saved-ocr.zip");
        fs::write(&package_path, &package).unwrap();
        for mode in 0..7 {
            let target = TempDir::new().unwrap();
            let state = Arc::new(FakeState::default());
            let vision = Arc::new(FakeVisionProvider {
                ocr_failure_detail: (mode == 3).then_some("private-saved-ocr-failure"),
                ..FakeVisionProvider::default()
            });
            vision.block_ocr.store(mode == 4, Ordering::Release);
            let host = RuntimeHost::start(
                config(&target),
                Arc::new(
                    FakeProvider::one("node.a", instance_id(), state.clone()).with_vision_provider(
                        Arc::new(FieldEvidenceProvider(vision.clone(), None)),
                    ),
                ),
            )
            .unwrap();
            let client = RuntimeClient::connect(RuntimeClientConfig::new(
                target.path(),
                EventActor::Lab,
                EventSource::Lab,
            ))
            .unwrap();
            let mut request = SavedArtifactOcrRequest {
                source: binding.clone(),
                package_path: package_path.to_str().unwrap().into(),
                expected_sha256: format!("{:x}", Sha256::digest(&package)).into(),
                target_id: "fixture/ocr".into(),
            };
            if mode == 1 {
                request.source.captured.event_id = request.source.created.event_id;
            }
            if mode == 2 {
                request.source.artifact.created_at_unix_ms += 1;
            }
            if mode == 5 {
                request.source.state_root = target.path().to_str().unwrap().into();
            }
            if mode == 6 {
                request.source.through_sequence += 100_000;
            }
            let encoded = serde_json::to_vec(&request).unwrap();
            assert_eq!(
                serde_json::from_slice::<SavedArtifactOcrRequest>(&encoded).unwrap(),
                request
            );
            let operation = thread::spawn(move || client.recognize_artifact(request));
            if mode == 4 {
                let wait_until = Instant::now() + Duration::from_secs(3);
                while !vision.ocr_started.load(Ordering::Acquire) && Instant::now() < wait_until {
                    thread::sleep(Duration::from_millis(1));
                }
                assert!(vision.ocr_started.load(Ordering::Acquire));
                let (closed, receive) = mpsc::channel();
                let close = thread::spawn(move || {
                    let result = host.close();
                    closed.send(()).unwrap();
                    result
                });
                assert!(
                    receive.recv_timeout(Duration::from_millis(30)).is_err(),
                    "close waits for in-flight OCR"
                );
                vision.block_ocr.store(false, Ordering::Release);
                operation.join().unwrap().unwrap();
                close.join().unwrap().unwrap();
            } else {
                let result = operation.join().unwrap();
                if mode == 0 {
                    let receipt = result.unwrap();
                    receipt.validate().unwrap();
                    let RuntimeResult::ArtifactRecognized { result } = receipt.result().unwrap()
                    else {
                        panic!("saved OCR result missing")
                    };
                    assert_eq!(result.source, binding);
                    assert!(result.artifact.run_id.is_none() && result.artifact.frame_id.is_none());
                    let stored: Value = serde_json::from_slice(
                        &actingcommand_artifact_store::read_projected_verified(
                            target.path(),
                            &result.artifact,
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(stored["observation"]["raw_text"], "home");
                    assert_eq!(stored["observation"]["text"], "home");
                    assert_eq!(
                        stored["observation"]["confidence"].as_f64().unwrap() as f32,
                        0.99_f32
                    );
                    assert_eq!(stored["observation"]["blocks"], json!([]));
                    assert_eq!(
                        stored["observation"]["execution"]["provider_implementation"],
                        "fixture-ocr"
                    );
                    assert_eq!(
                        stored["source"]["artifact"]["sha256"],
                        binding.artifact.sha256
                    );
                } else {
                    assert!(result.is_err(), "source/provider failure is visible");
                }
                host.close().unwrap();
            }
            assert_eq!(
                vision.ocr_calls.load(Ordering::Acquire),
                u64::from(matches!(mode, 0 | 3 | 4))
            );
            assert_eq!(state.capture_open_count.load(Ordering::Acquire), 0);
            assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
            assert_eq!(state.input_count.load(Ordering::Acquire), 0);
            let ledger = GlobalLedger::open_evidence(
                actingcommand_ledger::GlobalLedgerEvidenceConfig::new(target.path()),
                |reference| {
                    actingcommand_artifact_store::verify_projected_read_only(
                        target.path(),
                        reference,
                    )
                    .ok()
                },
            )
            .unwrap();
            assert!(ledger.corrupt_tail().is_none());
            assert!(ledger.events().iter().all(|event| !matches!(
                event.event_type(),
                EventType::CaptureRequested
                    | EventType::LeaseGranted
                    | EventType::InputIntent
                    | EventType::InputCommitted
                    | EventType::FactPublished
                    | EventType::TaskRequested
            )));
            assert_eq!(
                ledger
                    .events()
                    .iter()
                    .filter(|event| event.event_type() == EventType::ArtifactVerified)
                    .count(),
                usize::from(mode == 0 || mode == 4)
            );
        }
        assert_eq!(
            fs::read(original.path().join("runtime-state.sqlite")).unwrap(),
            original_ledger
        );
        assert_eq!(
            fs::read(original.path().join(binding.artifact.object_key().unwrap())).unwrap(),
            original_image
        );
    }

    let mut source = zip::ZipArchive::new(Cursor::new(
        neutral_post_admission_ocr_contained_task_package(),
    ))
    .expect("existing neutral package");
    let mut files = BTreeMap::new();
    for index in 0..source.len() {
        let mut entry = source.by_index(index).expect("neutral entry");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("neutral entry bytes");
        files.insert(entry.name().to_string(), bytes);
    }
    let truth = serde_json::to_vec(
        &json!({"schema_version":"actingcommand.ocr-truth-set.v2","items":["home"],"aliases":[]}),
    )
    .unwrap();
    let truth_sha = format!("{:x}", Sha256::digest(&truth));
    let mut control: Value = serde_json::from_slice(&files["control.json"]).unwrap();
    control["execution_mode"] = json!("navigable_route");
    control["capture_interval_ms"] = json!(3);
    control["step_timeout_ms"] = json!(27);
    control
        .as_object_mut()
        .unwrap()
        .remove("stability_termination");
    let mut task: Value =
        serde_json::from_slice(&files["resources/operations/task/task.json"]).unwrap();
    task["schema_version"] = json!("0.8");
    task["target_page"] = json!("terminal");
    task["operations"][0]["to"] = json!("terminal");
    task["operations"][0]["expect_after"] =
        json!({"page_id":"terminal","timeout_ms":480_000,"interval_ms":7});
    task["operations"][0]["post_delay_ms"] = json!(1);
    task.as_object_mut()
        .unwrap()
        .remove("stability_termination");
    task["post_admission_ocr"] = json!({"mode":"fields_v1","page_ids":["home"],"fields":[{
        "id":"location","group":"page","target_id":"fixture/ocr","required":true,"privacy":"public","trim":"whitespace_v1",
        "value":{"type":"dictionary_entry","dictionary":{"path":"truth.json","sha256":truth_sha}}}],
        "limits":{"max_frames":2,"max_items":16,"max_string_bytes":64,"max_total_bytes":4096,"max_truth_entries":16},"outcome_key":"fields_recorded"});
    task["scheduling_outcome"] = json!({"mappings":[{"outcome_key":"fields_recorded","effect":"no_designated_effect","terminal_pages":["terminal"]}]});
    let mut manifest: Value = serde_json::from_slice(&files["resources/manifest.json"]).unwrap();
    manifest["files"][0]["sha256"] = json!(truth_sha);
    files.insert("control.json".into(), serde_json::to_vec(&control).unwrap());
    files.insert(
        "resources/operations/task/task.json".into(),
        serde_json::to_vec(&task).unwrap(),
    );
    files.insert("resources/operations/task/truth.json".into(), truth);
    files.insert(
        "resources/manifest.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    );
    let zero_input_files = files.clone();
    let mut package = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in files {
        package
            .start_file(
                path,
                FileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        package.write_all(&bytes).unwrap();
    }
    let bytes = package.finish().unwrap().into_inner();
    let expected_sha = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();

    for (capture, fatal, fail_report, successful) in [
        (true, false, false, false),
        (false, false, false, false),
        (true, true, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package_path = root.path().join("fields-callback.zip");
        fs::write(&package_path, &bytes).expect("inline fields package");
        let state = Arc::new(FakeState::default());
        *state.input_selection.lock().unwrap() = Some(InputSelectionContext {
            backend: TouchBackendName::AdbShellInput,
            serial: "neutral-selected-input".to_owned(),
        });
        *state.capture_selection.lock().unwrap() = Some(CaptureSelectionContext {
            requested: CaptureBackendChoice::NemuIpc,
            configured_adb: "neutral-configured-adb".to_owned(),
            configured_serial: Some("127.0.0.1:16384".to_owned()),
            resolved_adb: "neutral-resolved-adb".to_owned(),
            selected_serial: "neutral-selected-capture".to_owned(),
            mumu: Some(CaptureMumuContext {
                root: "neutral-installation".into(),
                adb_path: "neutral-installation/adb".into(),
                capture_dll_path: "neutral-installation/capture.dll".into(),
                source: MumuInstallSource::RunningProcess,
            }),
        });
        if capture {
            state.fail_capture_on.store(2, Ordering::Release);
            state
                .transient_capture_failure
                .store(!fatal, Ordering::Release);
        } else if !successful {
            let error = if fatal {
                DeviceError::fatal("synthetic input failure")
            } else {
                DeviceError::transient("synthetic input failure")
            };
            *state.input_error.lock().unwrap() = Some(error.with_diagnostic(
                DeviceErrorCategory::Native,
                "device_registry.input.operation",
            ));
        }
        state.block_input.store(fail_report, Ordering::Release);
        state
            .transition_capture_after_input
            .store(successful, Ordering::Release);
        let configured_instance = instance_id();
        let target = DeviceTarget {
            serial: Some("127.0.0.1:16384".to_owned()),
            ..DeviceTarget::default()
        };
        let adb = AdbConfig {
            adb_path: "neutral-configured-adb".to_owned(),
            command_timeout: Duration::from_millis(71),
        };
        let registry = ExecutionBackendRegistry::new([ExecutionBackendRegistration::new(
            "neutral.instance",
            configured_instance,
            "neutral.application",
            TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
                .with_requested(TouchBackendChoice::Minitouch),
            CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::NemuIpc),
        )
        .unwrap()])
        .unwrap();
        let resolved = registry.resolve("neutral.instance").unwrap();
        let expected_configuration = resolved.configuration().unwrap().clone();
        let vision = Arc::new(FakeVisionProvider::default());
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(
                FakeProvider::one("neutral.instance", configured_instance, state.clone())
                    .with_resolved_override(Arc::new(std::sync::Mutex::new(resolved)))
                    .with_vision_provider(Arc::new(FieldEvidenceProvider(vision.clone(), None))),
            ),
        )
        .expect("formal host with existing fake backends");
        let request =
            ContainedTaskRequest::new(package_path.display().to_string(), &expected_sha).unwrap();
        let client_root = root.path().to_path_buf();
        let execution = thread::spawn(move || {
            let client = RuntimeClient::connect(RuntimeClientConfig::new(
                client_root,
                EventActor::Cli,
                EventSource::Cli,
            ))
            .expect("official client");
            client.run_contained_task("neutral.instance", request)
        });
        let artifact_root = root.path().join("artifacts");
        let preserved_artifacts = root.path().join("preserved-artifacts");
        if fail_report {
            wait_until(Duration::from_secs(5), || {
                state.input_started.load(Ordering::Acquire)
            });
            // The existing fake input pause occurs after parsed observation persistence.
            // Refuse the next store write using this test's own filesystem boundary.
            let refusal = fs::rename(&artifact_root, &preserved_artifacts)
                .and_then(|()| fs::write(&artifact_root, b"synthetic store refusal"));
            state.block_input.store(false, Ordering::Release);
            refusal.expect("refuse report storage");
        }
        let result = execution.join().expect("official client execution");
        if fail_report {
            fs::remove_file(&artifact_root).expect("remove synthetic refusal");
            fs::rename(&preserved_artifacts, &artifact_root)
                .expect("restore original artifact evidence");
        }
        let expected_code = if fail_report {
            RuntimeErrorCode::RuntimeFatal
        } else if capture {
            RuntimeErrorCode::CaptureFailed
        } else {
            RuntimeErrorCode::BackendOperationFailed
        };
        let events = host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("product ledger facts");
        let diagnostics = events
            .iter()
            .filter(|event| event.event_type() == EventType::ArtifactVerified)
            .flat_map(|event| event.artifacts())
            .filter(|artifact| artifact.kind() == ArtifactKind::DiagnosticJson)
            .filter(|artifact| artifact.producer() == ArtifactProducer::CapturePipeline)
            .collect::<Vec<_>>();
        let configuration_records = events
            .iter()
            .filter(|event| event.event_type() == EventType::ArtifactVerified)
            .flat_map(|event| {
                event
                    .artifacts()
                    .iter()
                    .filter(|artifact| {
                        artifact.kind() == ArtifactKind::DiagnosticJson
                            && artifact.producer() == ArtifactProducer::ArtifactStore
                    })
                    .map(move |artifact| (event, artifact))
            })
            .filter_map(|(event, artifact)| {
                let bytes = read_projected_verified(root.path(), &artifact.project(true)).unwrap();
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if value
                    .get("schema_version")
                    .and_then(serde_json::Value::as_str)
                    != Some(actingcommand_contract::EFFECTIVE_CONFIGURATION_SCHEMA)
                {
                    return None;
                }
                assert!(
                    artifact.byte_count()
                        <= actingcommand_contract::MAX_EFFECTIVE_CONFIGURATION_BYTES
                );
                let record: EffectiveConfigurationRecord = serde_json::from_value(value).unwrap();
                assert_eq!(event.links().task_id(), Some(&record.task_id));
                assert_eq!(event.links().run_id(), Some(&record.run_id));
                assert_eq!(event.links().frame_id(), record.frame_id.as_ref());
                assert_eq!(event.links().action_id(), record.action_id.as_ref());
                assert_eq!(event.links().request_id(), Some(&record.request_id));
                Some((event, record))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            configuration_records.len(),
            if capture || successful { 3 } else { 2 }
        );
        assert!(configuration_records.len() <= 4);
        let (initial_event, initial) = &configuration_records[0];
        let EffectiveConfigurationFacts::Initial {
            device,
            timing,
            capture_observed,
            input_observed,
            host_deadline_monotonic_ms,
            observed_at_monotonic_ms,
            host_remaining_ms,
            ..
        } = &initial.facts
        else {
            panic!("initial effective configuration");
        };
        assert_eq!(device.as_ref(), Some(&expected_configuration));
        assert!(!capture_observed && !input_observed);
        assert_eq!(
            *host_remaining_ms,
            host_deadline_monotonic_ms.saturating_sub(*observed_at_monotonic_ms)
        );
        assert_eq!(timing.step_timeout.milliseconds, 27);
        assert_eq!(timing.step_timeout.source, EffectiveTimingSource::Control);
        assert_eq!(timing.capture_interval.milliseconds, 3);
        assert_eq!(timing.operations[0].timeout.milliseconds, 480_000);
        assert_eq!(timing.operations[0].interval.milliseconds, 7);
        assert_eq!(
            timing.operations[0].timeout.source,
            EffectiveTimingSource::ExpectAfter
        );
        assert_eq!(timing.operations[0].postdelay.milliseconds, 1);
        assert_eq!(
            timing.operations[0].postdelay.source,
            EffectiveTimingSource::Operation
        );
        assert!(timing.operations[0].expect_after);
        assert!(
            events
                .iter()
                .find(|event| event.event_type() == EventType::CaptureRequested)
                .unwrap()
                .sequence()
                > initial_event.sequence()
        );
        assert!(
            initial.frame_id.is_none()
                && initial.action_id.is_none()
                && initial.source_sequence.is_none()
        );
        let capture_record = &configuration_records[1].1;
        let EffectiveConfigurationFacts::Capture {
            backend,
            selection: Some(selection),
        } = &capture_record.facts
        else {
            panic!("first successful capture context");
        };
        assert_eq!(backend, "adb_screencap");
        assert_eq!(selection.requested_backend, "nemu_ipc");
        assert_eq!(selection.resolved_adb, "neutral-resolved-adb");
        assert_eq!(selection.selected_serial, "neutral-selected-capture");
        assert_eq!(selection.mumu.as_ref().unwrap().source, "running_process");
        assert_eq!(
            selection.mumu.as_ref().unwrap().capture_dll_path,
            std::path::PathBuf::from("neutral-installation/capture.dll")
        );
        let capture_source = events
            .iter()
            .find(|event| Some(event.sequence()) == capture_record.source_sequence)
            .unwrap();
        assert_eq!(capture_source.event_type(), EventType::CaptureRequested);
        assert_eq!(
            capture_source.links().frame_id(),
            capture_record.frame_id.as_ref()
        );
        if capture || successful {
            let input_record = &configuration_records[2].1;
            let EffectiveConfigurationFacts::Input {
                selection: Some(selection),
            } = &input_record.facts
            else {
                panic!("committed input context");
            };
            assert_eq!(selection.backend, "adb_shell_input");
            assert_eq!(selection.serial, "neutral-selected-input");
            let input_source = events
                .iter()
                .find(|event| Some(event.sequence()) == input_record.source_sequence)
                .unwrap();
            assert_eq!(input_source.event_type(), EventType::InputCommitted);
            assert_eq!(
                input_source.links().action_id(),
                input_record.action_id.as_ref()
            );
        }
        if successful {
            let output = result.expect("successful formal task with effective configuration");
            assert!(matches!(
                output.receipt().result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    outcome: TaskOutcome::Success,
                    ..
                })
            ));
            assert_eq!(state.input_count.load(Ordering::Acquire), 1);
            assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
            host.close().expect("successful close");
            continue;
        }
        assert_eq!(
            vision.ocr_calls.load(Ordering::Acquire),
            1,
            "resolved fields precede callback failure: {result:?}"
        );
        assert_eq!(
            state.capture_count.load(Ordering::Acquire),
            if capture { 2 } else { 1 }
        );
        assert_eq!(
            diagnostics.len(),
            if fatal || fail_report { 1 } else { 2 },
            "one raw observation and at most one report"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type() == EventType::ArtifactStoreFailed)
                .count(),
            usize::from(fail_report),
            "failed report persistence is not retried"
        );
        if fatal || fail_report {
            let error = result.expect_err("fatal boundary propagates");
            assert!(error.is_fatal());
            assert_eq!(
                error.projection().expect("typed fatal projection").code,
                expected_code
            );
            if capture || fail_report {
                assert!(
                    host.fatal_error()
                        .unwrap()
                        .expect("fatal host state")
                        .is_fatal()
                );
                assert!(host.close().expect_err("fatal host close").is_fatal());
            } else {
                assert!(host.fatal_error().unwrap().is_none());
                host.close().expect("input failure remains contained");
            }
        } else {
            let output = result.expect("ordinary callback failure keeps official fields output");
            assert_eq!(output.receipt().state(), RuntimeReceiptState::Failed);
            let error = output
                .receipt()
                .error_projection()
                .expect("original failed receipt");
            assert_eq!(error.code, expected_code);
            assert!(!error.fatal);
            let projection = output
                .official_ocr_fields_projection()
                .expect("official parsed fields");
            assert_eq!(projection.records().len(), 1);
            assert_eq!(projection.failure(), None);
            for artifact in &diagnostics {
                assert_eq!(artifact.run_id(), Some(&projection.run_id()));
                assert_eq!(
                    artifact.frame_id(),
                    Some(&projection.records()[0].frame_id())
                );
            }
            let value = serde_json::to_value(projection).unwrap();
            assert_eq!(value["records"][0]["group"], "page");
            assert_eq!(value["records"][0]["fields"][0]["raw_text"], "home");
            assert_eq!(value["records"][0]["fields"][0]["value"]["value"], "home");
            assert_eq!(
                value["records"][0]["frame_id"],
                value["observations"][0]["frame_id"]
            );
            let terminals = events.iter().filter(|event| matches!(event.payload(), EventPayload::Task(TaskPayload::Semantic(payload))
                if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted { outcome: TaskOutcome::Failure, .. }))).collect::<Vec<_>>();
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminals[0].links().run_id(), Some(&projection.run_id()));
            assert!(host.fatal_error().unwrap().is_none());
            host.close().expect("ordinary failure leaves host healthy");
        }
    }

    {
        let root = TempDir::new().unwrap();
        let target = explicit_home_contained_task_package(
            "fixture01.configuration-target",
            [0, 0, 255],
            [255, 0, 0],
        );
        let recovery_source = explicit_home_contained_task_package(
            "fixture01.configuration-recovery",
            [0, 0, 255],
            [255, 0, 0],
        );
        let mut archive = zip::ZipArchive::new(Cursor::new(recovery_source)).unwrap();
        let mut recovery = ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if entry.name() == "control.json" {
                let mut control: Value = serde_json::from_slice(&bytes).unwrap();
                for key in ["capture_interval_ms", "step_timeout_ms", "timeout_ms"] {
                    control.as_object_mut().unwrap().remove(key);
                }
                bytes = serde_json::to_vec(&control).unwrap();
            }
            recovery
                .start_file(
                    entry.name(),
                    FileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            recovery.write_all(&bytes).unwrap();
        }
        let recovery = recovery.finish().unwrap().into_inner();
        let target_path = root.path().join("configuration-target.zip");
        let recovery_path = root.path().join("configuration-recovery.zip");
        fs::write(&target_path, &target).unwrap();
        fs::write(&recovery_path, &recovery).unwrap();
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        *state.input_selection.lock().unwrap() = Some(InputSelectionContext {
            backend: TouchBackendName::AdbShellInput,
            serial: "neutral-recovery".to_owned(),
        });
        *state.capture_selection.lock().unwrap() = Some(CaptureSelectionContext {
            requested: CaptureBackendChoice::Adb,
            configured_adb: "neutral-adb".to_owned(),
            configured_serial: Some("neutral-recovery".to_owned()),
            resolved_adb: "neutral-adb".to_owned(),
            selected_serial: "neutral-recovery".to_owned(),
            mumu: None,
        });
        let configured_instance = instance_id();
        let target_config = DeviceTarget {
            serial: Some("neutral-recovery".to_owned()),
            ..DeviceTarget::default()
        };
        let adb = AdbConfig {
            adb_path: "neutral-adb".to_owned(),
            ..AdbConfig::default()
        };
        let registry = ExecutionBackendRegistry::new([ExecutionBackendRegistration::new(
            "fixture01.instance",
            configured_instance,
            "neutral.application",
            TouchBackendConfig::new(
                adb.clone(),
                target_config.clone(),
                MaaTouchConfig::default(),
            )
            .with_requested(TouchBackendChoice::AdbShellInput),
            CaptureBackendConfig::new(adb, target_config).with_requested(CaptureBackendChoice::Adb),
        )
        .unwrap()])
        .unwrap();
        let resolved = registry.resolve("fixture01.instance").unwrap();
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(
                FakeProvider::one("fixture01.instance", configured_instance, state.clone())
                    .with_resolved_override(Arc::new(std::sync::Mutex::new(resolved))),
            ),
        )
        .unwrap();
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            root.path(),
            EventActor::Cli,
            EventSource::Cli,
        ))
        .unwrap();
        let request = ContainedTaskRequest::new(
            target_path.display().to_string(),
            format!("{:x}", Sha256::digest(&target)),
        )
        .unwrap()
        .with_recovery(
            ContainedTaskRecoveryBinding::new(
                recovery_path.display().to_string(),
                format!("{:x}", Sha256::digest(&recovery)),
            )
            .unwrap(),
        )
        .unwrap();
        let output = client
            .run_contained_task("fixture01.instance", request)
            .unwrap();
        assert!(matches!(
            output.receipt().result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                outcome: TaskOutcome::Success,
                ..
            })
        ));
        let events = host
            .query_persisted_events_for_test(EventQuery::default())
            .unwrap();
        let configurations = events
            .iter()
            .filter(|event| event.event_type() == EventType::ArtifactVerified)
            .flat_map(|event| event.artifacts())
            .filter(|artifact| {
                artifact.kind() == ArtifactKind::DiagnosticJson
                    && artifact.producer() == ArtifactProducer::ArtifactStore
            })
            .filter_map(|artifact| {
                let value: serde_json::Value = serde_json::from_slice(
                    &read_projected_verified(root.path(), &artifact.project(true)).unwrap(),
                )
                .unwrap();
                (value
                    .get("schema_version")
                    .and_then(serde_json::Value::as_str)
                    == Some(actingcommand_contract::EFFECTIVE_CONFIGURATION_SCHEMA))
                .then(|| serde_json::from_value::<EffectiveConfigurationRecord>(value).unwrap())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            configurations.len(),
            4,
            "one initial, first capture, recovery, first committed input"
        );
        let recovery_record = configurations
            .iter()
            .find(|record| {
                matches!(
                    record.facts,
                    EffectiveConfigurationFacts::EntryRecovery { .. }
                )
            })
            .unwrap();
        let EffectiveConfigurationFacts::EntryRecovery {
            package_sha256,
            timing,
        } = &recovery_record.facts
        else {
            unreachable!()
        };
        assert_eq!(
            package_sha256,
            &actingcommand_contract::PackageRef::from(format!("{:x}", Sha256::digest(&recovery)))
        );
        assert_eq!(timing.task_timeout.milliseconds, 60_000);
        assert_eq!(timing.step_timeout.milliseconds, 5_000);
        assert_eq!(timing.capture_interval.milliseconds, 50);
        assert_eq!(
            timing.operations[0].timeout.source,
            EffectiveTimingSource::Default
        );
        assert_eq!(
            timing.operations[0].interval.source,
            EffectiveTimingSource::Default
        );
        assert_eq!(
            timing.operations[0].postdelay.source,
            EffectiveTimingSource::NotSpecified
        );
        assert!(!timing.operations[0].expect_after);
        assert_eq!(state.input_count.load(Ordering::Acquire), 1);
        assert!(state.capture_count.load(Ordering::Acquire) > 1);
        assert_eq!(events.iter().filter(|event| matches!(event.payload(), EventPayload::Task(TaskPayload::Semantic(payload)) if matches!(payload.fact(), TaskSemanticFact::RunStarted))).count(), 1);
        host.close().unwrap();
    }

    for (declared_page, explicit_home, starts_home, extracted_personal) in [
        ("home", false, true, None),
        ("terminal", false, true, None),
        ("home", true, true, None),
        ("home", true, false, None),
        ("home", false, true, Some(false)),
        ("home", false, true, Some(true)),
    ] {
        let mut files = zero_input_files.clone();
        let mut task = task.clone();
        task["operations"] = json!([]);
        task["target_page"] = json!(declared_page);
        task["post_admission_ocr"]["page_ids"] = json!([declared_page]);
        task["scheduling_outcome"]["mappings"][0]["terminal_pages"] = json!([declared_page]);
        if let Some(personal) = extracted_personal {
            let field = &mut task["post_admission_ocr"]["fields"][0];
            field["privacy"] = json!(if personal { "personal" } else { "public" });
            field["text_extraction"] = json!({"mode":"strip_declared_suffix_v1","suffix":[
                {"type":"ascii_digits","count":2}, {"type":"literal","value":"/"},
                {"type":"ascii_digits","count":4}, {"type":"literal","value":":"},
                {"type":"ascii_digits","count":2}, {"type":"literal","value":"期限"}
            ]});
        }
        files.insert(
            "resources/operations/task/task.json".into(),
            serde_json::to_vec(&task).unwrap(),
        );
        if explicit_home {
            let pack_path = "resources/recognition/neutral.test.pack.json";
            let mut pack: Value = serde_json::from_slice(&files[pack_path]).unwrap();
            for (id, color) in [
                ("home/green_anchor", [0, 255, 0]),
                ("home/alternate_anchor", [255, 255, 255]),
            ] {
                pack["targets"].as_array_mut().unwrap().push(json!({
                    "type":"color","id":id,"region":{"x":1,"y":0,"width":1,"height":1},
                    "expected":color
                }));
            }
            files.insert(pack_path.into(), serde_json::to_vec(&pack).unwrap());
            let pages_path = "resources/recognition/neutral.test.pages.json";
            let mut pages: Value = serde_json::from_slice(&files[pages_path]).unwrap();
            pages["pages"][0]["any_of"] = json!([["home/green_anchor", "home/alternate_anchor"]]);
            files.insert(pages_path.into(), serde_json::to_vec(&pages).unwrap());
        }
        let mut package = ZipWriter::new(Cursor::new(Vec::new()));
        for (path, bytes) in files {
            package
                .start_file(
                    path,
                    FileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            package.write_all(&bytes).unwrap();
        }
        let bytes = package.finish().unwrap().into_inner();
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let root = TempDir::new().unwrap();
        let package_path = root.path().join("zero-input-fields.zip");
        fs::write(&package_path, &bytes).unwrap();
        let state = Arc::new(FakeState::default());
        if explicit_home {
            state.fail_capture_on.store(2, Ordering::Release);
            state
                .transient_capture_failure
                .store(true, Ordering::Release);
            if !starts_home {
                state
                    .transition_capture_after_capture
                    .store(1, Ordering::Release);
            }
        }
        let vision = Arc::new(FakeVisionProvider::default());
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(
                FakeProvider::one("neutral.instance", instance_id(), state.clone())
                    .with_vision_provider(Arc::new(FieldEvidenceProvider(
                        vision.clone(),
                        extracted_personal.map(|_| " home09/2803:59期限 "),
                    ))),
            ),
        )
        .expect("formal zero-input host");
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            root.path(),
            EventActor::Cli,
            EventSource::Cli,
        ))
        .expect("official client");
        let result = client.run_contained_task(
            "neutral.instance",
            ContainedTaskRequest::new(package_path.display().to_string(), expected).unwrap(),
        );
        let events = host
            .query_persisted_events_for_test(EventQuery::default())
            .unwrap();
        assert_eq!(state.input_count.load(Ordering::Acquire), 0);
        assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
        assert_eq!(
            vision.ocr_calls.load(Ordering::Acquire),
            u64::from(declared_page == "home" && starts_home)
        );
        if explicit_home {
            let facts = events
                .iter()
                .filter_map(|event| match event.payload() {
                    EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                facts
                    .iter()
                    .filter(|fact| matches!(fact,
                        TaskSemanticFact::EntryRecognition {
                            phase: TaskEntryRecognitionPhase::Initial, required_page, matched,
                        } if required_page == "neutral/home" && *matched == starts_home
                    ))
                    .count(),
                1
            );
            assert!(facts.iter().any(|fact| matches!(
                fact,
                TaskSemanticFact::EntryRecoveryDecision { required: false }
            )));
            assert!(!facts.iter().any(|fact| matches!(
                fact,
                TaskSemanticFact::EntryRecoveryPackageAdmitted { .. }
                    | TaskSemanticFact::EntryRecoveryCompleted { .. }
                    | TaskSemanticFact::StepStarted { .. }
            )));
            assert!(facts.iter().any(|fact| matches!(fact,
                TaskSemanticFact::EntryTargetDisposition { disposition, failure_code }
                    if (*disposition == TaskEntryTargetDisposition::Started && starts_home && failure_code.is_none())
                        || (*disposition == TaskEntryTargetDisposition::FailClosed && !starts_home
                            && failure_code.as_deref() == Some("contained_task_home_entry_not_matched"))
            )));
        }
        let terminals = events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    fact @ TaskSemanticFact::TerminalCommitted { .. } => Some((event, fact)),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminals.len(), 1, "exactly one terminal: {result:?}");
        if declared_page == "home" && starts_home {
            let output = result.expect("official zero-input fields receipt");
            assert!(
                matches!(output.receipt().result(), Some(RuntimeResult::ContainedTaskCompleted {
                outcome: TaskOutcome::Success, executed_steps: 0, final_page: Some(page), ..
            }) if page == "neutral/home")
            );
            assert!(matches!(
                terminals[0].1,
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Success,
                    executed_steps: Some(0),
                    ..
                }
            ));
            let projection = output
                .official_ocr_fields_projection()
                .expect("verified report projection");
            assert_eq!(projection.failure(), None);
            assert_eq!(projection.records().len(), 1);
            assert_eq!(terminals[0].0.links().run_id(), Some(&projection.run_id()));
            let summary_event = events
                .iter()
                .find(|event| event.event_type() == EventType::CaptureSummaryCommitted)
                .expect("terminal capture summary");
            assert_eq!(
                summary_event.links().run_id(),
                terminals[0].0.links().run_id()
            );
            assert!(summary_event.sequence() < terminals[0].0.sequence());
            let EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) =
                summary_event.payload()
            else {
                panic!("typed terminal capture summary");
            };
            assert_eq!(summary.summary().frames().len(), 1);
            assert_eq!(
                summary.summary().frames()[0].artifact().frame_id(),
                Some(&projection.records()[0].frame_id())
            );
            assert!(summary.summary().pinned().iter().any(|pin| {
                pin.reason() == PinnedFrameReason::Terminal && pin.frame_index() == Some(0)
            }));
            let value = serde_json::to_value(projection).unwrap();
            if extracted_personal == Some(true) {
                let field = &value["records"][0]["fields"][0];
                assert_eq!(field["redacted"], true);
                assert_eq!(field["raw_text"], Value::Null);
                assert_eq!(field["normalized_text"], Value::Null);
                assert_eq!(field["value"], Value::Null);
                assert!(field.get("extraction").is_none());
                assert!(
                    !serde_json::to_string(projection)
                        .unwrap()
                        .contains("home09/2803:59")
                );
            } else {
                assert_eq!(value["records"][0]["fields"][0]["value"]["value"], "home");
            }
            if extracted_personal == Some(false) {
                let field = &value["records"][0]["fields"][0];
                assert_eq!(field["raw_text"], " home09/2803:59期限 ");
                assert_eq!(field["normalized_text"], "home09/2803:59期限");
                assert_eq!(
                    field["extraction"],
                    json!({
                        "rule_version":"strip_declared_suffix_v1",
                        "matched_suffix":{"start":4,"end":20},
                        "extracted_text":"home","extracted_range":{"start":0,"end":4}
                    })
                );
            }
            assert_eq!(
                value["records"][0]["frame_id"],
                value["observations"][0]["frame_id"]
            );
            let diagnostics = events
                .iter()
                .filter(|event| event.event_type() == EventType::ArtifactVerified)
                .flat_map(|event| event.artifacts())
                .filter(|artifact| {
                    artifact.kind() == ArtifactKind::DiagnosticJson
                        && artifact.producer() == ArtifactProducer::CapturePipeline
                })
                .collect::<Vec<_>>();
            assert_eq!(diagnostics.len(), 2, "one observation and one final report");
            for artifact in diagnostics {
                assert_eq!(artifact.run_id(), Some(&projection.run_id()));
                assert_eq!(
                    artifact.frame_id(),
                    Some(&projection.records()[0].frame_id())
                );
                if let Some(personal) = extracted_personal {
                    assert_eq!(
                        artifact.project(true).redaction_state,
                        if personal {
                            ArtifactRedactionState::Pending
                        } else {
                            ArtifactRedactionState::NotRequired
                        }
                    );
                    let stored: Value = serde_json::from_slice(
                        &read_projected_verified(root.path(), &artifact.project(true)).unwrap(),
                    )
                    .unwrap();
                    if let Some(report) = stored.get("report") {
                        let field = &report["records"][0]["fields"][0];
                        assert_eq!(field["raw_text"], " home09/2803:59期限 ");
                        assert_eq!(field["normalized_text"], "home09/2803:59期限");
                        assert_eq!(field["value"]["value"], "home");
                        assert_eq!(
                            field["extraction"]["matched_suffix"],
                            json!({"start":4,"end":20})
                        );
                        assert_eq!(field["extraction"]["extracted_text"], "home");
                        assert_eq!(report["declaration"], task["post_admission_ocr"]);
                    }
                }
            }
        } else {
            assert!(matches!(
                terminals[0].1,
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    executed_steps: Some(0),
                    ..
                }
            ));
            match result {
                Ok(output) => {
                    assert_eq!(output.receipt().state(), RuntimeReceiptState::Failed);
                    assert!(output.official_ocr_fields_projection().is_none());
                }
                Err(error) => assert!(!error.is_fatal(), "page mismatch is a contained failure"),
            }
            assert!(
                !events
                    .iter()
                    .any(|event| event.event_type() == EventType::TaskCompleted)
            );
        }
        drop(client);
        host.close().expect("zero-input run leaves host healthy");
    }
}
