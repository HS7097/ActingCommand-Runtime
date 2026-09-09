// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn runtime_requires_vision_provider_only_after_selected_vision_target() {
    use std::io::Read;
    let source = neutral_vision_contained_task_package();
    let mut archive = zip::ZipArchive::new(Cursor::new(source)).unwrap();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_owned();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        if name == "control.json" {
            let mut control: serde_json::Value = serde_json::from_slice(&content).unwrap();
            control["step_timeout_ms"] = 5000.into();
            control["timeout_ms"] = 20000.into();
            content = serde_json::to_vec(&control).unwrap();
        } else if name.ends_with("neutral.test.pack.json") {
            let mut pack: serde_json::Value = serde_json::from_slice(&content).unwrap();
            for target in pack["targets"].as_array_mut().unwrap() {
                if target["type"] == "ocr" {
                    target["region"]["width"] = 2.into();
                    target["expected"][0] =
                        format!("{}\nmarker", target["expected"][0].as_str().unwrap()).into();
                }
            }
            pack["targets"].as_array_mut().unwrap().push(
                serde_json::json!({"type":"nn", "id":"model/raw",
                "region":{"x":1,"y":0,"width":1,"height":1},
                "model_ref":"neutral-classifier", "model_sha256":"b".repeat(64),
                "candidate_labels":["ready"], "minimum_score":0.9,
                "selection":"best", "timeout_ms":1000}),
            );
            content = serde_json::to_vec(&pack).unwrap();
        } else if name.ends_with("neutral.test.pages.json") {
            let mut pages: serde_json::Value = serde_json::from_slice(&content).unwrap();
            pages["pages"][0]["optional"] = serde_json::json!(["model/raw"]);
            content = serde_json::to_vec(&pages).unwrap();
        }
        output.start_file(name, FileOptions::default()).unwrap();
        output.write_all(&content).unwrap();
    }
    let bytes = output.finish().unwrap().into_inner();
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();

    let missing_root = TempDir::new().expect("missing-provider tempdir");
    let missing_package = missing_root.path().join("neutral-vision-task.zip");
    fs::write(&missing_package, &bytes).expect("write missing-provider package");
    let missing_state = Arc::new(FakeState::default());
    let missing_host = RuntimeHost::start(
        config(&missing_root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&missing_state),
        )),
    )
    .expect("missing-provider runtime host");
    let mut missing_client = TestClient::connect(&missing_host);
    let missing_request = missing_client.request(RuntimeOperation::run_contained_task(
        "neutral.instance",
        missing_client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(missing_package.display().to_string(), expected.clone())
            .expect("missing-provider task request"),
    ));
    let missing_receipt = missing_client.send(&missing_request);
    assert_eq!(missing_receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        missing_receipt
            .error_projection()
            .expect("typed task failure")
            .code,
        RuntimeErrorCode::BackendOperationFailed
    );
    assert_eq!(missing_state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(missing_state.input_count.load(Ordering::Acquire), 0);
    drop(missing_client);
    missing_host.close().expect("close missing-provider host");

    let injected_root = TempDir::new().expect("injected-provider tempdir");
    let injected_package = injected_root.path().join("neutral-vision-task.zip");
    fs::write(&injected_package, &bytes).expect("write injected-provider package");
    let injected_state = Arc::new(FakeState::default());
    injected_state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let vision_provider = Arc::new(FakeVisionProvider {
        raw_evidence: true,
        ..FakeVisionProvider::default()
    });
    let injected_host = RuntimeHost::start(
        config(&injected_root),
        Arc::new(
            FakeProvider::one(
                "neutral.instance",
                instance_id(),
                Arc::clone(&injected_state),
            )
            .with_vision_provider(vision_provider.clone()),
        ),
    )
    .expect("injected-provider runtime host");
    let mut injected_client = TestClient::connect(&injected_host);
    injected_client.set_receipt_read_timeout();
    let injected_request = injected_client.request(RuntimeOperation::run_contained_task(
        "neutral.instance",
        injected_client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(injected_package.display().to_string(), expected)
            .expect("injected-provider task request"),
    ));
    let injected_receipt = injected_client.send(&injected_request);
    assert_eq!(injected_receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        injected_receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            final_page: Some(page),
            ..
        }) if page == "neutral/terminal"
    ));
    assert!(vision_provider.ocr_calls.load(Ordering::Acquire) >= 2);
    assert_eq!(vision_provider.ocr_calls.load(Ordering::Acquire), 6);
    assert_eq!(vision_provider.nn_calls.load(Ordering::Acquire), 2);
    assert_eq!(injected_state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(injected_state.capture_count.load(Ordering::Acquire), 2);
    let events = projected_events(
        &mut injected_client,
        EventQuery {
            request_id: Some(injected_request.request_id()),
            ..EventQuery::default()
        },
    );
    let diagnostics = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
        .filter(|artifact| {
            artifact.kind == ArtifactKind::DiagnosticJson
                && artifact.redaction_state
                    == actingcommand_contract::ArtifactRedactionState::Pending
        })
        .collect::<Vec<_>>();
    let [artifact] = diagnostics.as_slice() else {
        panic!("one streamed task artifact")
    };
    assert!(artifact.byte_count > 4 * 1024 * 1024);
    let mut reader =
        actingcommand_artifact_store::open_projected_stream(injected_root.path(), artifact)
            .unwrap();
    let document: serde_json::Value = serde_json::from_reader(&mut reader).unwrap();
    reader.finish().unwrap();
    assert_eq!(
        document["schema_version"],
        actingcommand_contract::TASK_DIAGNOSTIC_SCHEMA
    );
    let records = document["records"].as_array().unwrap();
    let ocr = records
        .iter()
        .filter(|record| record["kind"] == "ocr")
        .collect::<Vec<_>>();
    assert_eq!(ocr.len(), 6);
    assert_eq!(ocr[0]["data"]["raw_text"], "provider aggregate home");
    assert_eq!(ocr[0]["data"]["derived_text"], "home\nmarker");
    let blocks = records
        .iter()
        .filter(|record| record["kind"] == "ocr_block")
        .collect::<Vec<_>>();
    assert_eq!(blocks.len(), 12);
    assert_eq!(blocks[0]["data"]["source_index"], 0);
    assert_eq!(blocks[0]["data"]["derived_rank"], 1);
    assert_eq!(blocks[0]["data"]["raw"]["text"], "marker");
    assert_eq!(
        blocks[0]["data"]["raw"]["rect"],
        serde_json::json!({"x":1,"y":0,"width":1,"height":1})
    );
    assert_eq!(
        blocks[0]["data"]["raw"]["confidence"],
        serde_json::json!(0.75_f32)
    );
    let nn_results = records
        .iter()
        .filter(|record| record["kind"] == "nn")
        .collect::<Vec<_>>();
    assert_eq!(nn_results.len(), 2);
    for nn in nn_results {
        assert_eq!(
            nn["data"]["requested_region"],
            serde_json::json!({"x":1,"y":0,"width":1,"height":1})
        );
        assert_eq!(nn["data"]["selected_label"], "ready");
        let labels = records
            .iter()
            .filter(|record| record["kind"] == "nn_label" && record["parent_index"] == nn["index"])
            .collect::<Vec<_>>();
        assert_eq!(labels.len(), 1024);
        for (index, label) in labels.iter().enumerate() {
            assert_eq!(label["data"]["source_index"], index);
            assert_eq!(label["parent_index"], nn["index"]);
            assert_eq!(
                label["data"]["raw"]["label"],
                if index == 1023 {
                    "ready".into()
                } else {
                    format!("{index:04}{}", "x".repeat(4092))
                }
            );
            assert_eq!(
                serde_json::from_value::<f32>(label["data"]["raw"]["score"].clone())
                    .unwrap()
                    .to_bits(),
                (if index == 1023 { 0.98_f32 } else { 0.25_f32 }).to_bits()
            );
        }
        assert_eq!(labels[1023]["data"]["derived"]["rank"], 0);
    }
    drop(injected_client);
    injected_host.close().expect("close injected-provider host");
}
