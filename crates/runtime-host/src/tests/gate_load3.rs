// SPDX-License-Identifier: AGPL-3.0-only

//! LOAD-3 (Workflow #407, register §1.260 LD3-3; design `rd-1031/check-speed/model-loading.md` §6
//! and §7; coordinator brief `brief-load-3.md`): under `execution.loading.mode` `on` a run's
//! diagnostics hold its loading pages' rows. Each frame of a page-graph wait carries page rows of
//! the phase `loading`; a stretch of transition frames is entered on its first frame and left on the
//! first frame after it, with the page on that frame and the time since the stretch's last
//! transition frame; a stretch that outlasts the allowance records `limit_reached` on the frame that
//! ends the wait, whose failure carries `loading_page_id`, `loading_ms` and `loading_limit_ms`. Each
//! transition is one `contained_task_loading_recorded` row of the phase `loading` on its frame.
//!
//! A synthetic `neutral` game, server `test`, 2x1 frames: the first pixel names the page (`home`
//! red, `terminal` blue, dark none), a white second pixel is the package's loading page `spinner`.
//! Before the run's one input the device shows `home`; after it, the spinner over no page for a
//! number of captures, then `terminal`. The outcome of each run does not depend on the clock: the
//! first run's allowance is far longer than its three loading frames, the second's (1 ms) is spent
//! by its second loading frame, a capture interval later, and its loading never ends.

use super::*;
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const RECORDED: &str = "contained_task_loading_recorded";
const HOME: [u8; 3] = [255, 0, 0];
const TERMINAL: [u8; 3] = [0, 0, 255];
const DARK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

// The package of the one tap `open` from `home` to `terminal`, whose loading pages have the
// allowance `max_ms`.
fn package(max_ms: u64) -> Vec<u8> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.loading.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5000,
        "timeout_ms": 20000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "expect_after": {"page_id": "terminal", "timeout_ms": 5000}
        }]
    });
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/terminal", 0, TERMINAL),
            colour("loading/spinner", 1, WHITE)
        ]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
        ],
        "loading": {"max_ms": max_ms, "pages": [
            {"id": "neutral/spinner", "required": ["loading/spinner"], "optional": [],
             "forbidden": []}
        ]}
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("package JSON"))
            .expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

// A host whose one instance shows `home` before its input and, after it, the spinner over no page
// for `loading_frames` captures, then `terminal`.
fn host(root: &TempDir, loading_frames: usize) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    // The number of the first capture after the input, once there is one.
    let first_after = Arc::new(AtomicUsize::new(0));
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            if inputs == 0 {
                return vec![(0, HOME), (1, DARK)];
            }
            let first =
                match first_after.compare_exchange(0, capture, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => capture,
                    Err(first) => first,
                };
            if capture - first < loading_frames {
                vec![(0, DARK), (1, WHITE)]
            } else {
                vec![(0, TERMINAL), (1, DARK)]
            }
        },
    ));
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), state)),
    )
    .expect("runtime host")
}

// `execution.loading.mode` `on`, by an instruction.
fn loading_on(host: &RuntimeHost) {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ConfigureSettings {
        changes: vec![actingcommand_contract::ConfigSettingChange {
            key: actingcommand_runtime_config::execution::LOADING_MODE.to_owned(),
            value: Some(json!("on")),
        }],
        expected_version: None,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// One run of the package of `max_ms`; its receipt and correlation.
fn run(
    host: &RuntimeHost,
    root: &TempDir,
    max_ms: u64,
) -> (RuntimeReceipt, actingcommand_contract::CorrelationId) {
    let path = root.path().join(format!("loading-{max_ms}.zip"));
    fs::write(&path, package(max_ms)).expect("write package");
    let mut client = TestClient::connect(host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(path.display().to_string()).expect("task request"),
        ),
    );
    (client.send(&request), correlation_id)
}

// The task diagnostic records of the run `correlation_id`.
fn records(
    host: &RuntimeHost,
    root: &TempDir,
    correlation_id: actingcommand_contract::CorrelationId,
) -> Vec<Value> {
    let mut client = TestClient::connect(host);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let stream = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
        .find(|artifact| {
            artifact.kind == ArtifactKind::DiagnosticJson
                && artifact.redaction_state
                    == actingcommand_contract::ArtifactRedactionState::Pending
        })
        .expect("the run's task diagnostic stream");
    let document: Value = serde_json::from_slice(
        &read_projected_verified(root.path(), stream).expect("verified stream bytes"),
    )
    .expect("task diagnostic JSON");
    document["records"]
        .as_array()
        .expect("diagnostic records")
        .clone()
}

// The `contained_task_loading_recorded` rows, in order: each one's `loading_event`, its frame and
// its outcome values.
fn transitions(records: &[Value]) -> Vec<(String, Value, Value)> {
    records
        .iter()
        .filter(|record| {
            record["kind"] == "error"
                && record["data"]["phase"] == "loading"
                && record["data"]["error"]["outcome"]["code"] == RECORDED
        })
        .map(|record| {
            let outcome = &record["data"]["error"]["outcome"];
            assert_eq!(outcome["category"], "info");
            (
                outcome["values"]["loading_event"]
                    .as_str()
                    .expect("loading_event")
                    .to_owned(),
                record["frame_id"].clone(),
                outcome["values"].clone(),
            )
        })
        .collect()
}

#[test]
fn loading_rows_record_entry_exit_and_limit_frames() {
    let root = TempDir::new().expect("tempdir");
    let host = host(&root, 3);
    loading_on(&host);

    // Three spinner frames, then `terminal`: the run completes; `entered` on the first spinner
    // frame with nothing spent yet, `left` on the destination frame, which it names.
    let (receipt, correlation_id) = run(&host, &root, 600_000);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    let diagnostics = records(&host, &root, correlation_id);
    assert!(
        diagnostics.iter().any(|record| record["kind"] == "page"
            && record["data"]["phase"] == "loading"
            && record["data"]["page_id"] == "neutral/spinner"),
        "the frames' loading page rows"
    );
    let rows = transitions(&diagnostics);
    let [
        (entered, entered_frame, entered_values),
        (left, left_frame, left_values),
    ] = rows.as_slice()
    else {
        panic!("an entered and a left row: {rows:?}");
    };
    assert_eq!((entered.as_str(), left.as_str()), ("entered", "left"));
    assert!(!entered_frame.is_null() && !left_frame.is_null());
    assert_ne!(entered_frame, left_frame, "each row is on its own frame");
    for values in [entered_values, left_values] {
        assert_eq!(values["loading_page_id"], "neutral/spinner");
        assert_eq!(values["loading_limit_ms"], 600_000);
        assert_eq!(values["pack_operation_id"], "open");
    }
    assert_eq!(entered_values["loading_ms"], 0);
    assert_eq!(entered_values["page_id"], Value::Null);
    assert_eq!(left_values["page_id"], "neutral/terminal");
    assert!(left_values["loading_exit_gap_ms"].is_u64());
    host.close().expect("close the host");

    // A loading screen that never ends and an allowance of 1 ms: `entered`, then `limit_reached` on
    // the frame that ends the wait, and the failure's loading values.
    let root_b = TempDir::new().expect("tempdir");
    let host_b = self::host(&root_b, usize::MAX);
    loading_on(&host_b);
    let (receipt, correlation_id) = run(&host_b, &root_b, 1);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        receipt
    );
    let outcome = receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "page_confirmation_failed",
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("loading_page_id"),
        Some(&json!("neutral/spinner"))
    );
    assert_eq!(outcome.value("loading_ms"), Some(&json!(1)));
    assert_eq!(outcome.value("loading_limit_ms"), Some(&json!(1)));
    let rows = transitions(&records(&host_b, &root_b, correlation_id));
    assert_eq!(
        rows.iter()
            .map(|(event, _, _)| event.as_str())
            .collect::<Vec<_>>(),
        ["entered", "limit_reached"]
    );
    assert_eq!(rows[1].2["loading_ms"], 1);
    assert!(!rows[1].1.is_null(), "the limit row is on its frame");
    host_b.close().expect("close the second host");
}
