// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2b evidence on the existing fake physical provider:
//! the direct (`task-run`) path, which a fixture instance refuses. Item 2 with and without a
//! request recovery binding, item 5's refusals at preparation before any lease, and item 9's
//! replay of item 2's requests. Copied into the runtime-host test module by the one-off workflow
//! only; every printed line starts with `HOST|`.

use super::*;

/// The fake frame's first pixel: red before capture `k`, blue from `k`, yellow from `m`.
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const YELLOW: [u8; 3] = [255, 255, 0];

fn oneoff_package(
    package_id: &str,
    mode: &str,
    prerequisite: Option<&str>,
    task: serde_json::Value,
) -> Vec<u8> {
    let mut control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 500,
        "timeout_ms": 5000,
        "max_steps": 1
    });
    if let Some(prerequisite) = prerequisite {
        control["prerequisite_package_id"] = serde_json::json!(prerequisite);
    }
    let recognition = serde_json::json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/x", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": RED},
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": BLUE},
            {"type": "color", "id": "page/a2", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": YELLOW}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/x", "required": ["page/x"], "optional": [], "forbidden": []},
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/step_02_a2", "required": ["page/a2"], "optional": [], "forbidden": []}
        ]
    });
    let control = serde_json::to_vec(&control).expect("control JSON");
    let task = serde_json::to_vec(&task).expect("task JSON");
    let recognition = serde_json::to_vec(&recognition).expect("recognition JSON");
    let pages = serde_json::to_vec(&pages).expect("pages JSON");
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in [
        ("control.json", control.as_slice()),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#.as_slice(),
        ),
        ("resources/operations/task/task.json", task.as_slice()),
        ("resources/recognition/neutral.test.pack.json", recognition.as_slice()),
        ("resources/recognition/neutral.test.pages.json", pages.as_slice()),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn oneoff_task(from: &str, to: &str, entry: Option<&str>) -> serde_json::Value {
    let mut task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "timeout_ms": 5000,
        "max_steps": 1,
        "target_page": to,
        "operations": [{
            "id": "step_01_click",
            "from": from,
            "to": to,
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "expect_after": {"page_id": to, "timeout_ms": 300, "interval_ms": 10},
            "post_delay_ms": 10
        }]
    });
    if let Some(entry) = entry {
        task["entry_page"] = serde_json::json!(entry);
    }
    task
}

/// A linear package `home -> step_02_a2` declaring `prerequisite`.
fn oneoff_linear(package_id: &str, prerequisite: Option<&str>) -> Vec<u8> {
    oneoff_package(
        package_id,
        "linear_steps",
        prerequisite,
        oneoff_task("home", "step_02_a2", Some("home")),
    )
}

/// A page-graph package `x -> home`.
fn oneoff_return_home(package_id: &str, mode: &str) -> Vec<u8> {
    oneoff_package(package_id, mode, None, oneoff_task("x", "home", None))
}

struct OneoffPackage {
    path: std::path::PathBuf,
    sha256: String,
}

fn oneoff_write(root: &TempDir, name: &str, bytes: &[u8]) -> OneoffPackage {
    let path = root.path().join(format!("{name}.zip"));
    fs::write(&path, bytes).expect("write package");
    OneoffPackage {
        path,
        sha256: actingcommand_pack_containment::Sha256Hash::digest(bytes).to_string(),
    }
}

fn oneoff_binding(package: &OneoffPackage) -> ContainedTaskRecoveryBinding {
    ContainedTaskRecoveryBinding::new(package.path.display().to_string(), package.sha256.clone())
        .expect("binding")
}

fn oneoff_events(
    label: &str,
    client: &mut TestClient,
    correlation_id: CorrelationId,
) -> Vec<ProjectedEvent> {
    let events = projected_events(
        client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    for event in &events {
        // Shortened: a long line ends the job log's capture of the step output.
        let semantic = projected_task_semantic_fact(event)
            .map(|fact| format!("{fact:?}").chars().take(400).collect::<String>())
            .unwrap_or_default();
        println!(
            "HOST|{label}|event|{}|{:?}|{}",
            event.sequence, event.event_type, semantic
        );
    }
    events
}

static ONEOFF_FAILURES: AtomicUsize = AtomicUsize::new(0);

fn oneoff_check(label: &str, name: &str, condition: bool, detail: String) {
    println!(
        "HOST|{label}|CHECK|{name}|{}|{detail}",
        if condition { "PASS" } else { "FAIL" }
    );
    if !condition {
        ONEOFF_FAILURES.fetch_add(1, Ordering::AcqRel);
    }
}

/// Item 2 on the direct path: frames [X, X, HOME, HOME, HOME, A2] (blue from capture 3, yellow
/// from capture 6), H mapped as the prerequisite package; then the same request replayed (item 9).
fn oneoff_item_2(label: &str, with_binding: bool) {
    let root = TempDir::new().expect("tempdir");
    let main = oneoff_write(&root, "a", &oneoff_linear("neutral.prereq.a", Some("neutral.prereq.h")));
    let home = oneoff_write(&root, "h", &oneoff_return_home("neutral.prereq.h", "navigable_route"));
    let other = oneoff_write(&root, "other", &oneoff_return_home("neutral.prereq.other", "navigable_route"));
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state.transition_capture_after_capture.store(3, Ordering::Release);
    state.error_capture_after_capture.store(6, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_prerequisite_packages(BTreeMap::from([(
            "neutral.prereq.h".to_owned(),
            oneoff_binding(&home),
        )])),
        Arc::new(FakeProvider::one("neutral.instance", instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let mut task_request =
        ContainedTaskRequest::new(main.path.display().to_string(), main.sha256.clone()).expect("request");
    if with_binding {
        task_request = task_request.with_recovery(oneoff_binding(&other)).expect("binding");
    }
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let receipt = client.send(&request);
    let first = serde_json::to_value(&receipt).expect("receipt JSON");
    println!("HOST|{label}|receipt|{first}");
    println!(
        "HOST|{label}|counts|input_count={} capture_count={} prerequisite_sha256={} binding_sha256={}",
        state.input_count.load(Ordering::Acquire),
        state.capture_count.load(Ordering::Acquire),
        home.sha256,
        if with_binding { other.sha256.as_str() } else { "none" }
    );
    let events = oneoff_events(label, &mut client, correlation_id);
    oneoff_check(
        label,
        "completed_two_steps",
        receipt.state() == RuntimeReceiptState::Completed
            && matches!(receipt.result(), Some(RuntimeResult::ContainedTaskCompleted { executed_steps: 2, .. })),
        format!("{:?}", receipt.state()),
    );
    let opened = events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted { package_sha256 } => Some(package_sha256.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    oneoff_check(
        label,
        "opened_package_is_the_mapped_prerequisite",
        opened == vec![actingcommand_contract::PackageRef::from(&home.sha256)],
        format!("{opened:?}"),
    );
    oneoff_check(
        label,
        "one_task_requested",
        events.iter().filter(|event| event.event_type == EventType::TaskRequested).count() == 1,
        String::new(),
    );
    // Item 9: the same request again is answered from its ledger.
    let replayed = host
        .process_request_for_test(&request, ConnectionId::new(336).expect("connection"))
        .expect("replay");
    let second = serde_json::to_value(&replayed).expect("replay JSON");
    println!("HOST|{label}|replay receipt|{second}");
    oneoff_check(
        label,
        "replay_returns_the_original_receipt",
        first == second,
        format!("{:?}", replayed.state()),
    );
    drop(client);
    host.close().expect("close host");
}

/// Item 5 on the direct path: refused at preparation, before any lease or task record.
fn oneoff_refusal(
    label: &str,
    main: &[u8],
    packages: &[(&str, &str, Vec<u8>)],
    map: &[(&str, &str)],
    code: &str,
) {
    let root = TempDir::new().expect("tempdir");
    let main = oneoff_write(&root, "main", main);
    let written = packages
        .iter()
        .map(|(name, _id, bytes)| ((*name).to_owned(), oneoff_write(&root, name, bytes)))
        .collect::<BTreeMap<_, _>>();
    let prerequisites = map
        .iter()
        .map(|(key, name)| ((*key).to_owned(), oneoff_binding(&written[*name])))
        .collect::<BTreeMap<_, _>>();
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_prerequisite_packages(prerequisites),
        Arc::new(FakeProvider::one("neutral.instance", instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(main.path.display().to_string(), main.sha256.clone())
                .expect("request"),
        ),
    );
    let receipt = client.send(&request);
    let text = serde_json::to_string(&receipt).expect("receipt JSON");
    println!("HOST|{label}|receipt|{text}");
    let events = oneoff_events(label, &mut client, correlation_id);
    let types = events.iter().map(|event| event.event_type).collect::<Vec<_>>();
    println!("HOST|{label}|event types|{types:?}");
    oneoff_check(
        label,
        "denied_with_code",
        receipt.state() == RuntimeReceiptState::Denied && text.contains(code),
        format!("{:?}", receipt.state()),
    );
    oneoff_check(
        label,
        "no_lease_no_task_record_no_entry_fact",
        !types.iter().any(|event_type| {
            matches!(
                event_type,
                EventType::LeaseGranted | EventType::TaskRequested | EventType::TaskEntryPreflight
            )
        }) && state.capture_count.load(Ordering::Acquire) == 0,
        format!("captures={}", state.capture_count.load(Ordering::Acquire)),
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn oneoff_336l2b_direct_path_on_fake_physical_device() {
    oneoff_item_2("D2 item 2 without binding", false);
    oneoff_item_2("D2 item 2 with a request recovery binding", true);
    let linear = |id: &str, prerequisite: &str| oneoff_linear(id, Some(prerequisite));
    oneoff_refusal(
        "D5 cycle",
        &linear("neutral.prereq.p", "neutral.prereq.q"),
        &[("q", "neutral.prereq.q", linear("neutral.prereq.q", "neutral.prereq.p")),
          ("p", "neutral.prereq.p", linear("neutral.prereq.p", "neutral.prereq.q"))],
        &[("neutral.prereq.q", "q"), ("neutral.prereq.p", "p")],
        "contained_task_prerequisite_cycle",
    );
    oneoff_refusal(
        "D5 depth",
        &linear("neutral.prereq.l0", "neutral.prereq.l1"),
        &[("l1", "", linear("neutral.prereq.l1", "neutral.prereq.l2")),
          ("l2", "", linear("neutral.prereq.l2", "neutral.prereq.l3")),
          ("l3", "", linear("neutral.prereq.l3", "neutral.prereq.l4")),
          ("l4", "", oneoff_linear("neutral.prereq.l4", None))],
        &[("neutral.prereq.l1", "l1"), ("neutral.prereq.l2", "l2"), ("neutral.prereq.l3", "l3"), ("neutral.prereq.l4", "l4")],
        "contained_task_prerequisite_depth_exceeded",
    );
    oneoff_refusal(
        "D5 unbound",
        &linear("neutral.prereq.a", "neutral.prereq.h"),
        &[],
        &[],
        "contained_task_prerequisite_unbound",
    );
    oneoff_refusal(
        "D5 mismatch",
        &linear("neutral.prereq.a", "neutral.prereq.h"),
        &[("other", "", oneoff_return_home("neutral.prereq.other", "navigable_route"))],
        &[("neutral.prereq.h", "other")],
        "contained_task_prerequisite_mismatch",
    );
    oneoff_refusal(
        "D5 incompatible recognize_only",
        &linear("neutral.prereq.a", "neutral.prereq.r"),
        &[("r", "", oneoff_return_home("neutral.prereq.r", "recognize_only"))],
        &[("neutral.prereq.r", "r")],
        "contained_task_prerequisite_incompatible",
    );
    let failures = ONEOFF_FAILURES.load(Ordering::Acquire);
    println!("HOST|RESULT|failures={failures}");
    assert_eq!(failures, 0, "one-off checks failed");
}
