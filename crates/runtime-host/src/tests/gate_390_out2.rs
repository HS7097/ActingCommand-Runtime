// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 OUT-2 (forced-compromises mapping rows O-01, O-02, S-01; FCM §2.4 row OUT-2): the
// terminal of a run of a designated effect set records how often each member completed and the
// cap the run held it to, so "done n times" and "there was nothing to do" are told apart, and
// maps the run's outcome by whether any member completed. A cap may be the value of an instance
// fact (S-01), read before the run's first step. Every expected value comes from the FCM rows
// and the declaration rules of the contract.

use super::*;

const OUT2_ALIAS: &str = "neutral.instance";
const CAP_FACT: &str = "session.example.collect.remaining";
const UNRESOLVED: &str = "contained_task_designated_effect_cap_unresolved";

// `collect` on `home` comes back to `home` or reaches `terminal` (a counted business step),
// `leave` goes to `terminal`. The set has `collect`, its cap the fact `CAP_FACT`; the outcome is
// "collected" when it completed and "nothing" when it did not.
fn out2_package() -> Vec<u8> {
    let tap = |id: &str, to: serde_json::Value| {
        serde_json::json!({
            "id": id,
            "from": "home",
            "to": to,
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        })
    };
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {
            "designated_effects": [
                {"operation_id": "collect", "max_repeats": {"fact_key": CAP_FACT}}
            ],
            "mappings": [
                {"outcome_key": "collected", "effect": "designated_effect_completed",
                 "terminal_pages": ["terminal"]},
                {"outcome_key": "nothing", "effect": "no_designated_effect",
                 "terminal_pages": ["terminal"]}
            ]
        },
        "operations": [
            tap("collect", serde_json::json!(["home", "terminal"])),
            tap("leave", serde_json::json!("terminal"))
        ]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.out2.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 10
    });
    let pack = serde_json::json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
        ]
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"}),
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

// The cap fact of the instance: `value`, observed a second ago, valid for an hour.
fn cap_fact(value: i64) -> FactRecord {
    let observed_at_unix_ms = unix_ms_now().expect("the clock") - 1_000;
    FactRecord {
        scope: FactScope::Instance {
            instance_id: OUT2_ALIAS.to_owned(),
        },
        key: CAP_FACT.to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(value),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: Some(observed_at_unix_ms + 3_600_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 3_600_000,
            maximum_ms: 3_600_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 1_000,
        source_detector: "fixture.decision".to_owned(),
        source_snapshot_id: "snapshot:decision".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "c".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// What one run left: its terminal's task result, scheduling disposition and outcome code, and
// the inputs it made.
struct Out2Run {
    task_result: TaskOutcome,
    disposition: Option<SchedulingDisposition>,
    failure: Option<String>,
    inputs: usize,
}

// One manual run of the package on a fresh host whose screen shows `home` until `inputs` inputs
// were made, then `terminal`, with the cap fact `cap` (none when absent).
fn run_out2(cap: Option<i64>, inputs: usize) -> Out2Run {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("out2.zip");
    let bytes = out2_package();
    fs::write(&package, &bytes).expect("write package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    state
        .transition_capture_after_inputs
        .store(inputs, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            OUT2_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    if let Some(cap) = cap {
        host.publish_fact(cap_fact(cap)).expect("publish the cap");
    }
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            OUT2_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(
                package.display().to_string(),
                actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string(),
            )
            .expect("task request"),
        ),
    );
    let _receipt = client.send(&request);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let terminal = events
        .iter()
        .find_map(|event| match projected_task_semantic_fact(event) {
            Some(TaskSemanticFact::TerminalCommitted {
                task_result,
                scheduling_disposition,
                outcome,
                ..
            }) => Some((
                *task_result,
                scheduling_disposition.clone(),
                outcome
                    .as_ref()
                    .map(|outcome| outcome.code().as_str().to_owned()),
            )),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "the run's terminal; the Runtime's fatal error: {:?}",
                host.fatal_error()
            )
        });
    let made = state.input_count.load(Ordering::Acquire);
    drop(client);
    host.close().expect("close runtime host");
    Out2Run {
        task_result: terminal.0,
        disposition: terminal.1,
        failure: terminal.2,
        inputs: made,
    }
}

// FCM O-01, O-02 and S-01, end to end: the cap is the instance fact's value; the terminal maps
// "collected" or "nothing" and records `collect`'s count beside its cap; a fact that gives no cap
// ends the run before any input.
#[test]
fn out2_a_run_of_a_designated_effect_set_records_each_count_and_its_cap() {
    for (case, cap, inputs, outcome_key, count) in [
        // The cap is reached on `home`: `leave` goes on, the third input.
        ("the cap reached", 2, 3, "collected", 2),
        // The page changes before the cap: done twice of five.
        ("done before the cap", 5, 2, "collected", 2),
        // A cap of 0: nothing to do, `leave` at once.
        ("nothing to do", 0, 1, "nothing", 0),
    ] {
        let run = run_out2(Some(cap), inputs);
        assert_eq!(
            run.task_result,
            TaskOutcome::Success,
            "{case}: {:?}",
            run.failure
        );
        assert_eq!(run.inputs, inputs, "{case}");
        let disposition = run
            .disposition
            .unwrap_or_else(|| panic!("{case}: a disposition"));
        assert_eq!(disposition.outcome_key(), outcome_key, "{case}");
        let SchedulingEffectEvidence::DesignatedEffects { effects } = disposition.effect() else {
            panic!("{case}: the set's tallies, got {:?}", disposition.effect());
        };
        let tallies = effects
            .iter()
            .map(|tally| {
                (
                    tally.operation_label().to_owned(),
                    tally.count(),
                    tally.max_repeats(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tallies,
            vec![(
                "collect".to_owned(),
                count,
                u32::try_from(cap).expect("a cap")
            )],
            "{case}"
        );
        // The ledger reads back what it was given.
        let encoded = serde_json::to_value(&disposition).expect("encode the disposition");
        let decoded: SchedulingDisposition =
            serde_json::from_value(encoded).expect("decode the disposition");
        assert_eq!(decoded, disposition, "{case}");
    }

    let run = run_out2(None, 1);
    assert_eq!(run.task_result, TaskOutcome::Failure);
    assert_eq!(run.disposition, None);
    assert_eq!(run.failure.as_deref(), Some(UNRESOLVED));
    assert_eq!(run.inputs, 0, "no input before the cap is known");
}
