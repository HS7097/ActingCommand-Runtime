// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L6 evidence on the existing fake physical provider:
//! the scheduled settlement of the failures a fixture instance cannot produce, an application
//! effect and an adb (input or application) backend failure (R25-1 restart segment, R25-2).
//! Copied into the runtime-host test module by the one-off workflow only; every printed line
//! starts with `HOST|`.

use super::*;

/// pixel (0, 0): red before the first input, blue after it when transitions are on.
const L6_RED: [u8; 3] = [255, 0, 0];
const L6_BLUE: [u8; 3] = [0, 0, 255];

fn l6_package(
    mode: &str,
    operations: serde_json::Value,
    entry_page: &str,
    target_page: &str,
    pages: &[(&str, [u8; 3])],
) -> Vec<u8> {
    let steps = operations.as_array().expect("operations").len();
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.l6.oneoff",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 300,
        "timeout_ms": 5000,
        "max_steps": steps
    });
    let mut task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "timeout_ms": 5000,
        "max_steps": steps,
        "target_page": target_page,
        "operations": operations
    });
    if mode == "linear_steps" {
        task["entry_page"] = serde_json::json!(entry_page);
    }
    let recognition = serde_json::json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": pages.iter().map(|(name, color)| serde_json::json!({
            "type": "color",
            "id": format!("page/{name}"),
            "region": {"x": 0, "y": 0, "width": 1, "height": 1},
            "expected": color
        })).collect::<Vec<_>>()
    });
    let page_set = serde_json::json!({
        "schema_version": "0.3",
        "pages": pages.iter().map(|(name, _)| serde_json::json!({
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

fn l6_click(id: &str, from: &str, to: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "expect_after": {"page_id": to, "timeout_ms": 300, "interval_ms": 10},
        "post_delay_ms": 10
    })
}

fn l6_restart(to: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "step_01_app",
        "from": "any",
        "to": to,
        "application": {"action": "restart"},
        "expect_after": {"page_id": to, "timeout_ms": 300, "interval_ms": 10},
        "post_delay_ms": 10
    })
}

struct L6Outcome {
    error_code: String,
    original_class: PolicyFailureClass,
    disposition: PolicyFailureDisposition,
    consecutive_same_error: u16,
}

/// One scheduled run of `package` on the fake physical provider; returns its execution record.
fn l6_scheduled_run(
    label: &str,
    package: &[u8],
    fail_input: bool,
    fail_application: bool,
    transition: bool,
) -> L6Outcome {
    let root = TempDir::new().expect("tempdir");
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(package));
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(transition, Ordering::Release);
    state.fail_input.store(fail_input, Ordering::Release);
    state
        .fail_application
        .store(fail_application, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_procedure_manifest(procedure_manifest_with_primary(
            package,
            vec!["after_observation".to_owned()],
        )),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");
    match host.run_scheduled_contained_task(&context, &request) {
        Ok(receipt) => {
            println!("HOST|{label}|run|ok|{:?}", receipt.state());
            host.complete_scheduled_policy_run(&context, &receipt)
                .expect("complete scheduled run");
        }
        Err(error) => println!(
            "HOST|{label}|run|err|{}|fatal={}",
            error.code(),
            error.is_fatal()
        ),
    }
    println!(
        "HOST|{label}|counts|application_count={} input_count={} capture_count={}",
        state.application_count.load(Ordering::Acquire),
        state.input_count.load(Ordering::Acquire),
        state.capture_count.load(Ordering::Acquire)
    );
    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    for event in &events {
        let semantic = projected_task_semantic_fact(event)
            .map(|fact| format!("{fact:?}"))
            .unwrap_or_default();
        println!(
            "HOST|{label}|event|{}|{:?}|{:?}|{}",
            event.sequence, event.event_type, event.severity, semantic
        );
        if matches!(
            event.event_type,
            EventType::ApplicationIntent
                | EventType::ApplicationFailed
                | EventType::InputFailed
                | EventType::TaskFailed
                | EventType::PolicyExecutionRecorded
        ) {
            println!(
                "HOST|{label}|event json|{}|{}",
                event.sequence,
                serde_json::to_string(event).expect("event JSON")
            );
        }
    }
    let outcome = events
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
                    match payload.outcome() {
                        PolicyExecutionOutcome::Failed { failure } => Some(L6Outcome {
                            error_code: failure.error_code.clone(),
                            original_class: failure.original_class,
                            disposition: failure.disposition,
                            consecutive_same_error: failure.consecutive_same_error,
                        }),
                        PolicyExecutionOutcome::Succeeded { .. } => None,
                    }
                }
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_else(|| panic!("{label}: no failed execution record"));
    println!(
        "HOST|{label}|settlement|error_code={}|original_class={:?}|disposition={:?}|consecutive_same_error={}",
        outcome.error_code,
        outcome.original_class,
        outcome.disposition,
        outcome.consecutive_same_error
    );
    drop(client);
    let _ = host.close();
    outcome
}

fn l6_frame_mark(error_code: &str) -> &str {
    error_code.rsplit('~').next().unwrap_or_default()
}

#[test]
fn oneoff_336l6_scheduled_settlement_on_fake_physical_device() {
    let mut failures = Vec::new();
    let mut check = |label: &str, ok: bool, detail: String| {
        println!(
            "HOST|CHECK|{label}|{}|{detail}",
            if ok { "PASS" } else { "FAIL" }
        );
        if !ok {
            failures.push(label.to_owned());
        }
    };
    let home_terminal = [("home", L6_RED), ("terminal", L6_BLUE)];

    // R25-2: a linear task's input adb failure (fatal device error, no poisoning).
    let linear_click = l6_package(
        "linear_steps",
        serde_json::json!([l6_click("step_01_click", "neutral/home", "neutral/terminal")]),
        "neutral/home",
        "neutral/terminal",
        &home_terminal,
    );
    let outcome = l6_scheduled_run("R25-2 linear input failure", &linear_click, true, false, true);
    check(
        "R25-2.linear_input.identity_rerun_only",
        outcome
            .error_code
            .starts_with("input_backend_operation_failed~v1~k")
            && l6_frame_mark(&outcome.error_code).starts_with("fu"),
        outcome.error_code.clone(),
    );
    check(
        "R25-2.linear_input.recoverable_retry",
        outcome.original_class == PolicyFailureClass::Recoverable
            && outcome.disposition == PolicyFailureDisposition::RetryScheduled,
        format!("{:?} {:?}", outcome.original_class, outcome.disposition),
    );

    // The same failure of a page-graph task keeps its code and its severe class.
    let graph_click = l6_package(
        "navigable_route",
        serde_json::json!([l6_click("step_01_click", "neutral/home", "neutral/terminal")]),
        "neutral/home",
        "neutral/terminal",
        &home_terminal,
    );
    let outcome = l6_scheduled_run("R25-2 page-graph input failure", &graph_click, true, false, true);
    check(
        "R25-2.page_graph_input.original_code_severe_paused",
        outcome.error_code == "input_backend_operation_failed"
            && outcome.original_class == PolicyFailureClass::Severe
            && outcome.disposition == PolicyFailureDisposition::PausedTask,
        format!(
            "{} {:?} {:?}",
            outcome.error_code, outcome.original_class, outcome.disposition
        ),
    );

    // R25-2: a linear task's application adb failure.
    let restart_home = l6_package(
        "linear_steps",
        serde_json::json!([
            l6_restart("neutral/home"),
            l6_click("step_02_click", "neutral/home", "neutral/terminal")
        ]),
        "any",
        "neutral/terminal",
        &home_terminal,
    );
    let outcome = l6_scheduled_run(
        "R25-2 linear application failure",
        &restart_home,
        false,
        true,
        true,
    );
    check(
        "R25-2.linear_application.identity_rerun_only_recoverable",
        outcome
            .error_code
            .starts_with("application_backend_operation_failed~v1~k")
            && l6_frame_mark(&outcome.error_code).starts_with("fu")
            && outcome.original_class == PolicyFailureClass::Recoverable
            && outcome.disposition == PolicyFailureDisposition::RetryScheduled,
        format!(
            "{} {:?} {:?}",
            outcome.error_code, outcome.original_class, outcome.disposition
        ),
    );

    // R25-1: after the restart, the click's next page (the main interface) never shows:
    // the failure is inside the restart segment and does not accumulate.
    let restart_title = l6_package(
        "linear_steps",
        serde_json::json!([
            l6_restart("neutral/title"),
            l6_click("step_02_click", "neutral/title", "neutral/home")
        ]),
        "any",
        "neutral/home",
        &[("title", L6_RED), ("home", L6_BLUE)],
    );
    let outcome = l6_scheduled_run(
        "R25-1 restart segment page confirmation",
        &restart_title,
        false,
        false,
        false,
    );
    check(
        "R25-1.restart_segment.rerun_only",
        outcome.error_code.starts_with("page_confirmation_failed~v1~k")
            && l6_frame_mark(&outcome.error_code).starts_with("fu")
            && outcome.original_class == PolicyFailureClass::Recoverable
            && outcome.disposition == PolicyFailureDisposition::RetryScheduled,
        outcome.error_code.clone(),
    );

    // Control: the restart reached the main interface first, so the same failure code
    // accumulates and records its error frame.
    let outcome = l6_scheduled_run(
        "R25-1 after the main interface page confirmation",
        &restart_home,
        false,
        false,
        false,
    );
    let mark = l6_frame_mark(&outcome.error_code).to_owned();
    check(
        "R25-1.after_main_interface.accumulates_with_frame",
        outcome.error_code.starts_with("page_confirmation_failed~v1~k")
            && mark.len() == 13
            && mark.starts_with('f')
            && mark[1..].bytes().all(|byte| byte.is_ascii_hexdigit()),
        outcome.error_code.clone(),
    );
    println!("HOST|RESULT|failures|{}|{failures:?}", failures.len());
    assert!(failures.is_empty(), "one-off L6 checks failed: {failures:?}");
}
