// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2c evidence on the existing fake physical provider:
//! the direct (`task-run`) path, which a fixture instance refuses. Item 1 on the direct path
//! (the return-home fallback with no declaration) and item 7's full chain (three declared layers
//! and one return-home layer), each replayed with the same request. Copied into the runtime-host
//! test module by the one-off workflow only; every printed line starts with `HOST|`.

use super::*;

/// The fake frame's pixel (0, 0): red before capture `k`, blue from `k`, yellow from `m`; its
/// pixel (1, 0) stays green.
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const YELLOW: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];

/// A page: its name (the page id without `neutral/`), the pixel column it reads and the color.
type OneoffPage<'a> = (&'a str, u32, [u8; 3]);

fn oneoff_package(
    package_id: &str,
    mode: &str,
    prerequisite: Option<&str>,
    steps: &[(&str, &str)],
    pages: &[OneoffPage<'_>],
) -> Vec<u8> {
    let linear = mode == "linear_steps";
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
        "max_steps": steps.len()
    });
    if let Some(prerequisite) = prerequisite {
        control["prerequisite_package_id"] = serde_json::json!(prerequisite);
    }
    let operations = steps
        .iter()
        .enumerate()
        .map(|(index, (from, to))| {
            serde_json::json!({
                "id": format!("step_{:02}_click", index + 1),
                "from": from,
                "to": to,
                "click": {"kind": "point", "x": 2, "y": 0},
                "unguarded_trusted_coordinate": true,
                "expect_after": {"page_id": to, "timeout_ms": 300, "interval_ms": 10},
                "post_delay_ms": 10
            })
        })
        .collect::<Vec<_>>();
    let mut task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "timeout_ms": 5000,
        "max_steps": steps.len(),
        "target_page": steps.last().expect("one step").1,
        "operations": operations
    });
    if linear {
        task["entry_page"] = serde_json::json!(steps[0].0);
    }
    let recognition = serde_json::json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": pages.iter().map(|(name, column, color)| serde_json::json!({
            "type": "color",
            "id": format!("page/{name}"),
            "region": {"x": column, "y": 0, "width": 1, "height": 1},
            "expected": color
        })).collect::<Vec<_>>()
    });
    let page_set = serde_json::json!({
        "schema_version": "0.3",
        "pages": pages.iter().map(|(name, _, _)| serde_json::json!({
            "id": format!("neutral/{name}"),
            "required": [format!("page/{name}")],
            "optional": [],
            "forbidden": []
        })).collect::<Vec<_>>()
    });
    let control = serde_json::to_vec(&control).expect("control JSON");
    let task = serde_json::to_vec(&task).expect("task JSON");
    let recognition = serde_json::to_vec(&recognition).expect("recognition JSON");
    let page_set = serde_json::to_vec(&page_set).expect("pages JSON");
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
        ("resources/recognition/neutral.test.pages.json", page_set.as_slice()),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// The page-graph return-home package `x -> home` (red, then blue).
fn oneoff_return_home() -> Vec<u8> {
    oneoff_package(
        "neutral.l2c.h",
        "navigable_route",
        None,
        &[("x", "home")],
        &[("x", 0, RED), ("home", 0, BLUE)],
    )
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

/// Runs `main` once on the direct path with `prerequisites` mapped and the return-home package
/// `neutral.l2c.h` configured for (neutral, test), then replays the same request.
fn oneoff_direct(
    label: &str,
    main: &OneoffPackage,
    prerequisites: BTreeMap<String, ContainedTaskRecoveryBinding>,
    root: &TempDir,
    blue_from: usize,
    yellow_from: usize,
    executed_steps: u32,
    opened: Vec<String>,
) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_capture
        .store(blue_from, Ordering::Release);
    state
        .error_capture_after_capture
        .store(yellow_from, Ordering::Release);
    let host = RuntimeHost::start(
        config(root)
            .with_prerequisite_packages(prerequisites)
            .with_return_home_packages(BTreeMap::from([(
                ("neutral".to_owned(), "test".to_owned()),
                "neutral.l2c.h".to_owned(),
            )])),
        Arc::new(FakeProvider::one("neutral.instance", instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let task_request =
        ContainedTaskRequest::new(main.path.display().to_string(), main.sha256.clone())
            .expect("request");
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
        "HOST|{label}|counts|input_count={} capture_count={}",
        state.input_count.load(Ordering::Acquire),
        state.capture_count.load(Ordering::Acquire),
    );
    let events = oneoff_events(label, &mut client, correlation_id);
    oneoff_check(
        label,
        "completed",
        receipt.state() == RuntimeReceiptState::Completed
            && matches!(receipt.result(), Some(RuntimeResult::ContainedTaskCompleted { executed_steps: steps, .. }) if *steps == executed_steps),
        format!("{:?} expected executed_steps {executed_steps}", receipt.state()),
    );
    let admitted = events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted { package_sha256 } => {
                Some(package_sha256.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected = opened
        .iter()
        .map(actingcommand_contract::PackageRef::from)
        .collect::<Vec<_>>();
    oneoff_check(
        label,
        "opened_packages_outermost_first",
        admitted == expected,
        format!("{admitted:?}"),
    );
    oneoff_check(
        label,
        "one_task_requested",
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskRequested)
            .count()
            == 1,
        String::new(),
    );
    oneoff_check(
        label,
        "no_configuration_limit_failure",
        !first
            .to_string()
            .contains("effective_configuration_limit_exceeded"),
        String::new(),
    );
    // The same request again is answered from its ledger.
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

/// Item 3 on the direct path: H runs and reaches its home, but A's first step (yellow) never
/// passes; the receipt and the run's lifecycle failure record carry the detail.
fn oneoff_direct_failure(label: &str) {
    let root = TempDir::new().expect("tempdir");
    let main = oneoff_write(
        &root,
        "a",
        &oneoff_package(
            "neutral.l2c.a",
            "linear_steps",
            None,
            &[("home", "step_02_a2")],
            &[("home", 0, YELLOW), ("step_02_a2", 1, BLUE)],
        ),
    );
    let home = oneoff_write(&root, "h", &oneoff_return_home());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_capture
        .store(3, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root)
            .with_prerequisite_packages(BTreeMap::from([(
                "neutral.l2c.h".to_owned(),
                oneoff_binding(&home),
            )]))
            .with_return_home_packages(BTreeMap::from([(
                ("neutral".to_owned(), "test".to_owned()),
                "neutral.l2c.h".to_owned(),
            )])),
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
    let wanted = "layer=0 package_id=neutral.l2c.a required_page=neutral/home return_home=neutral.l2c.h";
    oneoff_check(
        label,
        "failed_with_return_home_code",
        receipt.state() == RuntimeReceiptState::Failed
            && text.contains("contained_task_return_home_entry_unmatched"),
        format!("{:?}", receipt.state()),
    );
    println!(
        "HOST|{label}|receipt carries the detail|{}",
        text.contains(wanted)
    );
    let failed = events
        .iter()
        .filter(|event| event.event_type == EventType::RuntimeFailed)
        .map(|event| serde_json::to_string(event).expect("event JSON"))
        .collect::<Vec<_>>();
    for event in &failed {
        println!("HOST|{label}|runtime.failed|{event}");
    }
    oneoff_check(
        label,
        "one_lifecycle_record_with_the_detail",
        failed.iter().filter(|event| event.contains(wanted)).count() == 1,
        format!("{} runtime.failed", failed.len()),
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn oneoff_336l2c_direct_path_on_fake_physical_device() {
    oneoff_direct_failure("D3 item 3 direct path");
    // Item 1 on the direct path: A declares nothing; frames [x, x, home, home, home, a2].
    {
        let root = TempDir::new().expect("tempdir");
        let main = oneoff_write(
            &root,
            "a",
            &oneoff_package(
                "neutral.l2c.a",
                "linear_steps",
                None,
                &[("home", "step_02_a2")],
                &[("home", 0, BLUE), ("step_02_a2", 0, YELLOW)],
            ),
        );
        let home = oneoff_write(&root, "h", &oneoff_return_home());
        oneoff_direct(
            "D1 item 1 direct path",
            &main,
            BTreeMap::from([("neutral.l2c.h".to_owned(), oneoff_binding(&home))]),
            &root,
            3,
            6,
            2,
            vec![home.sha256.clone()],
        );
    }
    // Item 7 on the direct path: X0 -> X1 -> X2 -> X3 declared, X3 falls back to H. Every
    // first step reads pixel 0 (blue from capture 6), every second step pixel 1 (green).
    {
        let root = TempDir::new().expect("tempdir");
        let layer = |id: &str, prerequisite: Option<&str>, from: &str, to: &str| {
            oneoff_package(
                id,
                "linear_steps",
                prerequisite,
                &[(from, to)],
                &[(from, 0, BLUE), (to, 1, GREEN)],
            )
        };
        let x0 = oneoff_write(
            &root,
            "x0",
            &layer("neutral.l2c.x0", Some("neutral.l2c.x1"), "step_01_n1", "step_02_fin"),
        );
        let x1 = oneoff_write(
            &root,
            "x1",
            &layer("neutral.l2c.x1", Some("neutral.l2c.x2"), "step_01_n2", "step_02_n1"),
        );
        let x2 = oneoff_write(
            &root,
            "x2",
            &layer("neutral.l2c.x2", Some("neutral.l2c.x3"), "step_01_n3", "step_02_n2"),
        );
        let x3 = oneoff_write(&root, "x3", &layer("neutral.l2c.x3", None, "home", "step_02_n3"));
        let home = oneoff_write(&root, "h", &oneoff_return_home());
        oneoff_direct(
            "D7 item 7 full chain direct path",
            &x0,
            BTreeMap::from([
                ("neutral.l2c.x1".to_owned(), oneoff_binding(&x1)),
                ("neutral.l2c.x2".to_owned(), oneoff_binding(&x2)),
                ("neutral.l2c.x3".to_owned(), oneoff_binding(&x3)),
                ("neutral.l2c.h".to_owned(), oneoff_binding(&home)),
            ]),
            &root,
            6,
            0,
            5,
            vec![
                x1.sha256.clone(),
                x2.sha256.clone(),
                x3.sha256.clone(),
                home.sha256.clone(),
            ],
        );
    }
    let failures = ONEOFF_FAILURES.load(Ordering::Acquire);
    println!("HOST|RESULT|failures={failures}");
    assert_eq!(failures, 0, "one-off checks failed");
}
