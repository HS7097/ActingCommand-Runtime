// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-LK (register §1.500; contracts/linear-steps.md, "List reading steps";
// contracts/resource-readings.md, "List readings", "Publication" and "Capacity and reserved keys"),
// the host's half, with no change to the host: a list a step of a `linear_steps` task reads is
// published at the run's successful end as a page-graph task's list is, and a store with no room
// for it refuses it as it refuses a page-graph task's. The packages are the S168-4 host gate's
// (`gate_428_s168_4`): its page-graph package, whose step `read_members` reads the one-screen list
// of `member-00` to `member-05` and whose tap `leave` goes on to the detail page, and the same
// package as a step plan, its control naming `linear_steps` and its task the list as its entry page
// and its two steps (the zip rewritten here, every other entry byte for byte). A fake device paints
// the screens the test plans, one more after each input. The ids are made up; the keys are
// `inventory.list.example_kind_a` and `inventory.list.example_kind_b`.

use super::gate_308_l4b::ALIAS;
use super::gate_428_s168_4::{
    Device, completed, detail, list, publications, run, start, write_package,
};
use super::*;
use crate::fact_store::MAX_INSTANCE_FACTS;
use std::io::Read;

const LIST_KEY: &str = "inventory.list.example_kind_a";
const OTHER_KEY: &str = "inventory.list.example_kind_b";
const VALID_FOR_MS: u64 = 86_400_000;
const READING_REJECTED: &str = "contained_task_resource_reading_rejected";
const CAPACITY_EXCEEDED: &str = "fact_store_capacity_exceeded";

// The S168-4 gate's package of `fact_key` as a step plan, written under `root` as `name`: its
// `control.json` names `linear_steps`, its task the entry page `list` and `max_steps` 2 (the
// control's already); every other entry is the page-graph package's, byte for byte.
fn write_linear_package(root: &TempDir, name: &str, fact_key: &str) -> PathBuf {
    let graph = write_package(root, &format!("graph-{name}"), fact_key);
    let bytes = fs::read(&graph).expect("read the page-graph package");
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("the package's zip");
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("a zip entry");
        let path = entry.name().to_owned();
        let mut read = Vec::new();
        entry.read_to_end(&mut read).expect("the entry's bytes");
        let written = match path.as_str() {
            "control.json" => {
                let mut control: serde_json::Value =
                    serde_json::from_slice(&read).expect("the control's JSON");
                control["execution_mode"] = serde_json::json!("linear_steps");
                serde_json::to_vec(&control).expect("the control's bytes")
            }
            "resources/operations/task/task.json" => {
                let mut task: serde_json::Value =
                    serde_json::from_slice(&read).expect("the task's JSON");
                task["entry_page"] = serde_json::json!("list");
                task["max_steps"] = serde_json::json!(2);
                serde_json::to_vec(&task).expect("the task's bytes")
            }
            _ => read,
        };
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&written).expect("zip bytes");
    }
    let path = root.path().join(name);
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write the package");
    path
}

fn context(alias: &str) -> InstanceFactContext {
    InstanceFactContext {
        instance_id: alias.to_owned(),
        server_id: "test".to_owned(),
        game_id: "neutral".to_owned(),
    }
}

// The instance's active records.
fn records(host: &RuntimeHost, alias: &str) -> Vec<FactRecord> {
    host.instance_fact_snapshot(context(alias))
        .expect("instance facts")
        .records
}

// The step records of the run `correlation_id`: each step's start, effect intent and finish with
// its operation, and the page a finish names.
fn steps(host: &RuntimeHost, correlation_id: CorrelationId) -> Vec<(&'static str, String, String)> {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("the run's records");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::StepStarted {
                    operation_label, ..
                } => Some(("step_started", operation_label.clone(), String::new())),
                TaskSemanticFact::EffectIntent {
                    operation_label, ..
                } => Some(("effect_intent", operation_label.clone(), String::new())),
                TaskSemanticFact::StepFinished {
                    operation_label,
                    page_label,
                    ..
                } => Some(("step_finished", operation_label.clone(), page_label.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

// Whether the receipt is the run's normal end on the detail page after both steps.
fn ended_on_the_detail_page(receipt: &RuntimeReceipt) -> bool {
    matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            final_page: Some(page),
            executed_steps: 2,
            ..
        }) if page == "neutral/detail"
    )
}

// One made-up fact of `ALIAS`, observed at `now`.
fn fill(index: usize, now: u64) -> FactRecord {
    FactRecord {
        scope: FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        key: format!("session.example.fill_{index:03}"),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(1),
        },
        observed_at_unix_ms: now,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "detector.fixture".to_owned(),
        source_snapshot_id: format!("snapshot:s168-lk-fill-{index:03}"),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// Test 6, the publication (rulings 3 and 7 of the work order): on one host, the page-graph package
// and then the step plan read the same list, its members out of their key order, each run ending
// normally on the detail page after its two steps. The step plan's run publishes one more
// `fact.published` of the key: the same scope, the same six rows in ascending byte order of the
// key, the same confidence, lifetime and lifetime policy, `source_detector`, schema, bundle hash and
// no invalidation trigger, a snapshot of its own run observed later, and it is the key's active
// record. Its steps are recorded as a page-graph run's: the reading started and finished on the
// list page with no effect intent, then `leave` with its one.
#[test]
fn lk_a_linear_tasks_list_is_published_at_the_run_end_as_a_page_graph_tasks() {
    let root = TempDir::new().expect("tempdir");
    let graph = write_package(&root, "graph.zip", LIST_KEY);
    let linear = write_linear_package(&root, "linear.zip", LIST_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);

    let mut correlations = Vec::new();
    for package in [&graph, &linear] {
        device.shows(vec![list([3, 0, 5, 1, 4, 2]), detail()]);
        let (receipt, correlation_id) = run(&host, ALIAS, package);
        completed(&host, &receipt);
        assert!(
            ended_on_the_detail_page(&receipt),
            "{}: {:?}",
            package.display(),
            receipt.result()
        );
        correlations.push(correlation_id);
    }

    let published = publications(&host, LIST_KEY);
    let [by_graph, by_linear] = published.as_slice() else {
        panic!("two publications of the list, got {published:#?}");
    };
    assert_eq!(by_linear.scope, by_graph.scope);
    assert_eq!(by_linear.content, by_graph.content);
    assert_eq!(by_linear.confidence_milli, by_graph.confidence_milli);
    assert_eq!(by_linear.ttl_policy, by_graph.ttl_policy);
    assert_eq!(by_linear.source_detector, by_graph.source_detector);
    assert_eq!(by_linear.schema_version, by_graph.schema_version);
    assert_eq!(
        by_linear.resource_bundle_hash,
        by_graph.resource_bundle_hash
    );
    assert_eq!(by_linear.invalidate_on, by_graph.invalidate_on);
    assert_eq!(
        by_linear.expires_at_unix_ms,
        Some(by_linear.observed_at_unix_ms + VALID_FOR_MS)
    );
    assert_ne!(by_linear.source_snapshot_id, by_graph.source_snapshot_id);
    assert!(by_linear.observed_at_unix_ms > by_graph.observed_at_unix_ms);
    assert_eq!(
        records(&host, ALIAS)
            .into_iter()
            .find(|record| record.key == LIST_KEY)
            .as_ref(),
        Some(by_linear),
        "the key's record is the step plan's"
    );

    let expected = [
        ("step_started", "read_members", ""),
        ("step_finished", "read_members", "neutral/list"),
        ("step_started", "leave", ""),
        ("effect_intent", "leave", ""),
        ("step_finished", "leave", "neutral/detail"),
    ]
    .map(|(kind, operation, page)| (kind, operation.to_owned(), page.to_owned()));
    for correlation_id in correlations {
        assert_eq!(steps(&host, correlation_id), expected);
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 6, the full store: after the step plan read its list once, the instance's own facts are
// filled with made-up keys until the store refuses one (not fatal), at the instance's bound. A run
// of the step plan whose list is a new key then fails as a run of the page-graph package of that
// key fails on the same store: `contained_task_resource_reading_rejected` with the native detail
// `read_members:fact_store_capacity_exceeded`, nothing published under the key and no active fact
// evicted.
#[test]
fn lk_a_full_store_refuses_a_linear_tasks_list_as_a_page_graph_tasks() {
    let root = TempDir::new().expect("tempdir");
    let first = write_linear_package(&root, "linear-a.zip", LIST_KEY);
    let linear = write_linear_package(&root, "linear-b.zip", OTHER_KEY);
    let graph = write_package(&root, "graph-b.zip", OTHER_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    device.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, ALIAS, &first);
    completed(&host, &receipt);

    let now = unix_ms_now().expect("wall clock");
    let refusal = (0..=MAX_INSTANCE_FACTS)
        .find_map(|index| host.publish_fact(fill(index, now)).err())
        .expect("the instance's bound refuses a key");
    assert_eq!(refusal.code(), CAPACITY_EXCEEDED, "{refusal:?}");
    assert!(!refusal.is_fatal(), "{refusal:?}");
    let before = records(&host, ALIAS);

    for (mode, package) in [("linear_steps", &linear), ("navigable_route", &graph)] {
        device.shows(vec![list(0..6), detail()]);
        let (receipt, _) = run(&host, ALIAS, package);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Failed,
            "{mode}: {:?}",
            receipt.outcome()
        );
        assert_eq!(
            receipt.outcome().code().as_str(),
            READING_REJECTED,
            "{mode}: {:?}",
            receipt.outcome()
        );
        assert_eq!(
            host.failure_detail_for_test(receipt.request_id())
                .expect("the failure's detail"),
            Some(format!("read_members:{CAPACITY_EXCEEDED}")),
            "{mode}"
        );
        assert!(publications(&host, OTHER_KEY).is_empty(), "{mode}");
        assert_eq!(records(&host, ALIAS), before, "{mode}: nothing is evicted");
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
