// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 OUT-1, forced compromises O-03 and O-04 (real-device failures) and O-05: a
// catalog may reference a subset of the outcomes its package declares, a declared outcome that a
// successful run did not produce evaluates false, and a zero-input fields package may declare
// several outcomes.

use super::*;

/// The outcome the mapped producer's run produces: its designated effect completes.
const PRODUCED: &str = "out1-produced";
/// The zero-input package's outcome on its page without fields.
const OTHER_PAGE_OUTCOME: &str = "out1-other-page";

/// A host whose producer `fixture.observe` is bound to `package`, at the manual clock's time.
fn start_producer_host(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    package: &[u8],
    state: &Arc<FakeState>,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(PolicyInputSnapshot::new(
                mapped_policy_facts_at(PRODUCED, POLICY_NOW_UNIX_MS, false),
                policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(state))
                .fixture_simulation(),
        ),
    )
    .expect("producer runtime host")
}

/// The mapped catalog whose consumer `fixture.followup` reads only `reads` of the producer: it is
/// due when every one of them has its value on the producer's instance and its stop fact is
/// false.
fn consumer_reading(reads: &[(&str, bool)]) -> CatalogSources {
    let keys = reads.iter().map(|(key, _)| *key).collect::<Vec<_>>();
    let mut sources = mapped_policy_sources_with_keys(1, &keys);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("consumer catalog tasks");
    let reads = reads
        .iter()
        .map(|(key, value)| {
            serde_json::json!({
                "kind": "outcome",
                "task_id": "fixture.observe",
                "outcome_key": key,
                "comparison": "eq",
                "value": {"type": "boolean", "value": value}
            })
        })
        .collect::<Vec<_>>();
    let stop = tasks["tasks"][1]["feedback_stop"].clone();
    tasks["tasks"][1]["trigger"] = serde_json::json!({
        "kind": "all",
        "predicates": [
            {"kind": "all", "predicates": reads},
            {"kind": "not", "predicate": stop}
        ]
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("consumer catalog bytes");
    sources
}

/// Evaluates at the producer's next eligible instant, with the consumer's stop fact false.
fn evaluate_after_the_producer(
    case: &str,
    host: &RuntimeHost,
    clock: &Arc<ManualRuntimeClock>,
    context: &PolicyRunContext,
) -> actingcommand_policy::PolicyEvaluation {
    advance_manual_clock_to(clock, context.admission().activity.next_eligible_unix_ms);
    let at = clock
        .sample()
        .expect("sample the consumer's evaluation time");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts_at(PRODUCED, at.unix_ms, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at.unix_ms,
                monotonic_ms: at.monotonic_ms,
            },
            39_951,
            PolicyTrigger::FactsChanged,
        )
        .unwrap_or_else(|error| {
            panic!(
                "{case}: evaluation after the producer: {} ({error})",
                error.code()
            )
        });
    let (kind, reason) = (cycle.directive.kind, cycle.directive.reason);
    cycle.evaluation.unwrap_or_else(|| {
        panic!("{case}: no evaluation after the producer; directive {kind:?} {reason:?}")
    })
}

#[test]
fn out1_a_catalog_reads_a_subset_of_the_declared_outcomes_and_an_unproduced_one_is_false() {
    // FCM O-03 and O-04 (real-device failures): a catalog had to list exactly the outcomes its
    // package declares, so a package said "no" by failing, and a declared outcome that a
    // successful run did not produce read as Unknown, so its consumer never ran.
    let unproduced = complementary_outcome_key(PRODUCED);
    for (case, reads, producer_fails) in [
        (
            "a subset of the outcomes",
            vec![(unproduced.as_str(), false)],
            false,
        ),
        (
            "every outcome",
            vec![(PRODUCED, true), (unproduced.as_str(), false)],
            false,
        ),
        (
            "a failed producer",
            vec![(unproduced.as_str(), false)],
            true,
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        // The package declares PRODUCED (its designated effect) and its alternate.
        let package =
            neutral_mapped_contained_task_package(PRODUCED, "designated_effect_completed");
        let package_path = root.path().join("out1-producer.zip");
        fs::write(&package_path, &package).expect("write producer package");
        let request = ContainedTaskRequest::new(
            package_path.to_string_lossy().into_owned(),
            format!("{:x}", Sha256::digest(&package)),
        )
        .expect("producer request");
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        let clock = Arc::new(ManualRuntimeClock::new(
            POLICY_NOW_UNIX_MS,
            POLICY_NOW_UNIX_MS,
        ));
        let host = start_producer_host(&root, &clock, &package, &state);
        host.activate_policy_catalog(&consumer_reading(&reads))
            .unwrap_or_else(|error| panic!("{case}: activate the consumer's catalog: {error}"));
        let (context, _) = admit_mapped_run_at(&host, PRODUCED, POLICY_NOW_UNIX_MS, 39_941);
        if producer_fails {
            host.complete_scheduled_policy_failure_without_terminal_for_test(&context)
                .unwrap_or_else(|error| panic!("{case}: producer failure: {error}"));
        } else {
            let receipt = host
                .run_scheduled_contained_task(&context, &request)
                .unwrap_or_else(|error| {
                    panic!(
                        "{case}: the producer runs under a catalog that reads its outcomes: {} \
                         ({error})",
                        error.code()
                    )
                });
            let (_, projection) = host
                .complete_scheduled_policy_run(&context, &receipt)
                .unwrap_or_else(|error| {
                    panic!("{case}: producer completion: {} ({error})", error.code())
                });
            assert_eq!(
                projection
                    .as_ref()
                    .map(|projection| projection.scheduling_result().disposition().outcome_key()),
                Some(PRODUCED),
                "{case}: the run produced its designated effect's outcome"
            );
        }
        assert!(
            host.fatal_error()
                .expect("read runtime fatal state")
                .is_none(),
            "{case}: nothing is fatal"
        );

        let evaluation = evaluate_after_the_producer(case, &host, &clock, &context);
        let unproduced_fact = format!("outcome.fixture.observe.{unproduced}");
        let consumer = evaluation
            .decisions
            .iter()
            .find(|decision| {
                decision.task_id == "fixture.followup"
                    && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
            })
            .unwrap_or_else(|| panic!("{case}: consumer decision missing"));
        let read_as_unknown = consumer.detection_suggestions.iter().any(|suggestion| {
            suggestion.fact_key == unproduced_fact && suggestion.reason == "outcome_missing"
        });
        let consumer_dispatched = evaluation.dispatch_intents.iter().any(|intent| {
            intent.task_id == "fixture.followup" && intent.instance_id == POLICY_INSTANCE_ALIAS
        });
        if producer_fails {
            assert!(
                read_as_unknown,
                "{case}: a failed run leaves the outcome Unknown; got {consumer:?}"
            );
            assert!(
                !consumer_dispatched,
                "{case}: the consumer cannot run on an Unknown outcome"
            );
        } else {
            assert!(
                !read_as_unknown,
                "{case}: the declared outcome the run did not produce is false, not Unknown; got \
                 {consumer:?}"
            );
            assert!(
                consumer_dispatched,
                "{case}: the consumer that waits for the unproduced outcome to be false runs; got \
                 {:?}",
                evaluation
                    .dispatch_intents
                    .iter()
                    .map(|intent| (intent.task_id.as_str(), intent.instance_id.as_str()))
                    .collect::<Vec<_>>()
            );
        }
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
    }
}

#[test]
fn out1_a_zero_input_fields_package_with_two_outcomes_ends_on_its_other_page_without_fields() {
    // FCM O-05: a zero-input package could declare only its fields outcome, and a fields package
    // whose run passed no field page failed with `observation_missing`.
    use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
    use serde_json::{Value, json};
    use std::io::Read;

    let mut source = zip::ZipArchive::new(Cursor::new(
        neutral_post_admission_ocr_contained_task_package(true),
    ))
    .expect("neutral OCR package");
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
    .expect("truth set");
    let truth_sha = format!("{:x}", Sha256::digest(&truth));
    let mut control: Value =
        serde_json::from_slice(&files["control.json"]).expect("package control");
    control["execution_mode"] = json!("navigable_route");
    control["capture_interval_ms"] = json!(3);
    control["step_timeout_ms"] = json!(27);
    control
        .as_object_mut()
        .expect("control object")
        .remove("stability_termination");
    let mut task: Value = serde_json::from_slice(&files["resources/operations/task/task.json"])
        .expect("package task");
    task["schema_version"] = json!("0.8");
    task["target_page"] = json!(["home", "terminal"]);
    task["operations"] = json!([]);
    task.as_object_mut()
        .expect("task object")
        .remove("stability_termination");
    // Fields are read on `home` only; `terminal` is the other outcome's page.
    task["post_admission_ocr"] = json!({"mode":"fields_v1","page_ids":["home"],"fields":[{
        "id":"location","group":"page","target_id":"fixture/ocr","required":true,"privacy":"public","trim":"whitespace_v1",
        "value":{"type":"dictionary_entry","dictionary":{"path":"truth.json","sha256":truth_sha}}}],
        "limits":{"max_frames":2,"max_items":16,"max_string_bytes":64,"max_total_bytes":4096,"max_truth_entries":16},"outcome_key":"fields_recorded"});
    task["scheduling_outcome"] = json!({"mappings":[
        {"outcome_key":"fields_recorded","effect":"no_designated_effect","terminal_pages":["home"]},
        {"outcome_key":OTHER_PAGE_OUTCOME,"effect":"no_designated_effect","terminal_pages":["terminal"]}
    ]});
    let mut manifest: Value =
        serde_json::from_slice(&files["resources/manifest.json"]).expect("package manifest");
    manifest["files"][0]["sha256"] = json!(truth_sha);
    files.insert(
        "control.json".into(),
        serde_json::to_vec(&control).expect("control bytes"),
    );
    files.insert(
        "resources/operations/task/task.json".into(),
        serde_json::to_vec(&task).expect("task bytes"),
    );
    files.insert("resources/operations/task/truth.json".into(), truth);
    files.insert(
        "resources/manifest.json".into(),
        serde_json::to_vec(&manifest).expect("manifest bytes"),
    );
    let mut package = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in files {
        package
            .start_file(
                path,
                FileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .expect("package entry");
        package.write_all(&bytes).expect("package entry bytes");
    }
    let bytes = package.finish().expect("package zip").into_inner();
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let root = TempDir::new().expect("tempdir");
    let package_path = root.path().join("zero-input-two-outcomes.zip");
    fs::write(&package_path, &bytes).expect("write zero-input package");

    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    // The first frame already shows `terminal`.
    state
        .transition_capture_after_capture
        .store(1, Ordering::Release);
    let vision = Arc::new(FakeVisionProvider::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one("neutral.instance", instance_id(), state.clone())
                .with_vision_provider(vision.clone()),
        ),
    )
    .expect("zero-input host");
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("official client");
    let output = client
        .run_contained_task(
            "neutral.instance",
            ContainedTaskRequest::new(package_path.display().to_string(), expected)
                .expect("zero-input request"),
        )
        .unwrap_or_else(|error| {
            panic!(
                "a zero-input package with two outcomes is admitted and its run succeeds: {} \
                 ({error})",
                error.code()
            )
        });
    assert!(
        matches!(output.receipt().result(), Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success, executed_steps: 0, final_page: Some(page), ..
        }) if page == "neutral/terminal"),
        "the run ends on the other outcome's page: {:?}",
        output.receipt().result()
    );
    assert!(
        output.official_ocr_fields_projection().is_none(),
        "a run that passed no field page has no fields report"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "zero inputs");
    assert_eq!(
        vision.ocr_calls.load(Ordering::Acquire),
        0,
        "no field is read"
    );
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("read the run's events");
    let dispositions = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    scheduling_disposition: Some(disposition),
                    ..
                } => Some(disposition.outcome_key().to_owned()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        dispositions,
        vec![OTHER_PAGE_OUTCOME.to_owned()],
        "the terminal carries the other page's outcome"
    );
    drop(client);
    host.close()
        .expect("the zero-input run leaves the host healthy");
}
