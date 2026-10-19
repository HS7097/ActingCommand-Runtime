// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A23 (host key 118; register §1.125 A23-1): a run's scheduling outcome is judged on its own
//! package's records. When the package dispatched no step of its own after its prerequisite
//! layer, its last recognition decides, never the page the prerequisite's last step finished on;
//! when it dispatched one, its own last step's `StepFinished` decides. `executed_steps` stays the
//! run's total, the prerequisite's steps included (`contracts/linear-steps.md`, the prerequisite
//! gate; `contracts/ocr-fields.md`, zero-input fields tasks).
//!
//! The fixtures of `gate_373_nav3a`: a synthetic `neutral` game whose fake device's first pixel
//! is the screen (red `away`, blue `menu`, white `done`), each input moving it to the next screen
//! its test scripts; the prerequisite package `lead` is listed in the game's standard-pack root.
//! `fin` is a page of `lead` that shows the `done` screen under `lead`'s own page id.

use super::gate_373_nav3a::{
    ALIAS, AWAY, DONE, LEAD, LEAD_STEPS, MAIN, MENU, Pack, Run, StandardRoot, route, screens, start,
};
use super::*;
use actingcommand_contract::TaskEntryRecoveryRole;
use serde_json::{Value, json};

const PREREQUISITE: Option<TaskEntryRecoveryRole> = Some(TaskEntryRecoveryRole::Prerequisite);
/// The dependent package's outcome on its terminal page `done`.
const DONE_REACHED: &str = "done_reached";
/// The zero-input fields package's outcome on its field page.
const FIELDS_RECORDED: &str = "fields_recorded";
/// Every code of the host's scheduling-outcome check starts so.
const OUTCOME_CHECK_CODES: &str = "\"contained_task_outcome_";

// The dependent package: a page graph whose entry is the set {menu, done}, from `menu` to `done`,
// declaring `lead`.
fn dependent() -> Pack<'static> {
    Pack {
        package_id: MAIN,
        mode: "navigable_route",
        prerequisite: Some(LEAD),
        entry: json!(["menu", "done"]),
        target: "done",
        operations: &[("open", "menu", "done")],
    }
}

// Writes `pack` at `<temp>/main` and declares its outcome `done_reached` on its terminal page
// `done` in its written `task.json`; gives its directory.
fn write_with_outcome(temp: &TempDir, pack: &Pack<'_>) -> PathBuf {
    let directory = temp.path().join("main");
    pack.write(&directory);
    let task_path = directory.join("resources/operations/task/task.json");
    let mut task: Value =
        serde_json::from_slice(&fs::read(&task_path).expect("task")).expect("task JSON");
    task["scheduling_outcome"] = json!({"mappings": [{
        "outcome_key": DONE_REACHED,
        "effect": "no_designated_effect",
        "terminal_pages": ["done"]
    }]});
    fs::write(
        &task_path,
        serde_json::to_vec_pretty(&task).expect("task JSON"),
    )
    .expect("write task");
    directory
}

// Runs the package written at `directory` directly, as `gate_373_nav3a`'s `run` does.
fn run_written(host: &RuntimeHost, directory: &Path) -> Run {
    let task_request =
        ContainedTaskRequest::new(directory.display().to_string()).expect("task request");
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let receipt = client.send(&request);
    drop(client);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("run events");
    Run { receipt, events }
}

// The receipt's final page and executed steps.
fn receipt_end(receipt: &RuntimeReceipt) -> (Option<String>, u32) {
    match receipt.result() {
        Some(RuntimeResult::ContainedTaskCompleted {
            final_page,
            executed_steps,
            ..
        }) => (final_page.clone(), *executed_steps),
        other => panic!("a completed contained task: {other:?}"),
    }
}

// The successful terminals' executed steps and scheduling outcome keys.
fn terminals(facts: &[&TaskSemanticFact]) -> Vec<(Option<u32>, Option<String>)> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Success,
                executed_steps,
                scheduling_disposition,
                ..
            } => Some((
                *executed_steps,
                scheduling_disposition
                    .as_ref()
                    .map(|disposition| disposition.outcome_key().to_owned()),
            )),
            _ => None,
        })
        .collect()
}

// No record names a code of the host's scheduling-outcome check.
fn assert_no_outcome_check_code(events: &[PersistedEvent]) {
    for event in events {
        let payload = serde_json::to_string(event.payload()).expect("payload JSON");
        assert!(
            !payload.contains(OUTCOME_CHECK_CODES),
            "no contained_task_outcome_* code: {payload}"
        );
    }
}

// T1: the run starts away from the dependent's entry {menu, done}; `lead` (one step from `away`
// to its own page `fin`, the `done` screen) runs as the prerequisite layer, the recheck matches
// `done`, and the dependent, already on its terminal page, dispatches no step. Its own
// recognition decides: the run succeeds on `done` with `done_reached`, and `executed_steps` is
// the prerequisite's one step.
#[test]
fn a23_a_package_already_on_its_terminal_page_after_its_prerequisite_is_judged_on_its_own_recognition()
 {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(LEAD, "away", "fin", &[("go", "away", "fin")], None).write(&root.list(LEAD));
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));
    let directory = write_with_outcome(&temp, &dependent());

    let run = run_written(&host, &directory);
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        receipt_end(&run.receipt),
        (Some("neutral/done".to_owned()), 1)
    );
    let facts = run.facts();
    assert_eq!(
        terminals(&facts),
        vec![(Some(1), Some(DONE_REACHED.to_owned()))]
    );
    assert_eq!(
        run.completed(),
        vec![(LEAD.to_owned(), "neutral/fin".to_owned(), PREREQUISITE)]
    );
    let admitted = facts
        .iter()
        .rposition(|fact| {
            matches!(
                fact,
                TaskSemanticFact::PackageAdmitted { package_label, .. } if package_label == MAIN
            )
        })
        .expect("the dependent's PackageAdmitted");
    assert!(
        !facts[admitted..]
            .iter()
            .any(|fact| matches!(fact, TaskSemanticFact::StepStarted { .. })),
        "the dependent dispatches no step: {facts:?}"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_no_outcome_check_code(&run.events);
}

// The zero-input fields package of `gate_390_out1` (schema 0.8, no operation), its entry and
// terminal page its field page `home` (red), with the outcome `fields_recorded` there, declaring
// `lead` as its prerequisite package.
fn fields_package() -> Vec<u8> {
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
    control["prerequisite_package_id"] = json!(LEAD);
    control
        .as_object_mut()
        .expect("control object")
        .remove("stability_termination");
    let mut task: Value = serde_json::from_slice(&files["resources/operations/task/task.json"])
        .expect("package task");
    task["schema_version"] = json!("0.8");
    task["target_page"] = json!("home");
    task["operations"] = json!([]);
    task.as_object_mut()
        .expect("task object")
        .remove("stability_termination");
    task["post_admission_ocr"] = json!({"mode":"fields_v1","page_ids":["home"],"fields":[{
        "id":"location","group":"page","target_id":"fixture/ocr","required":true,"privacy":"public","trim":"whitespace_v1",
        "value":{"type":"dictionary_entry","dictionary":{"path":"truth.json","sha256":truth_sha}}}],
        "limits":{"max_frames":2,"max_items":16,"max_string_bytes":64,"max_total_bytes":4096,"max_truth_entries":16},"outcome_key":FIELDS_RECORDED});
    task["scheduling_outcome"] = json!({"mappings":[
        {"outcome_key":FIELDS_RECORDED,"effect":"no_designated_effect","terminal_pages":["home"]}
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
    package.finish().expect("package zip").into_inner()
}

// The fixture vision provider with the complete execution evidence a fields read needs (the
// recognition pack refuses an OCR observation without it), as `ocr_fields`'
// `FieldEvidenceProvider` gives it: one CPU stream, one invocation per read.
#[derive(Debug, Default)]
struct FieldsVision(FakeVisionProvider);

impl VisionProvider for FieldsVision {
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
    ) -> Result<actingcommand_recognition_pack::OcrProviderObservation, VisionProviderError> {
        use actingcommand_recognition_pack::{
            OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
        };

        let model_ref = request.model_ref.to_owned();
        let model_sha256 = request.model_sha256.to_owned();
        let result = self.0.read_text(request)?;
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

// T2 (the device shape): the run starts on the `done` screen, which no page of the fields package
// matches; `lead` (one step from `done` to its own page `away`, the red screen) runs as the
// prerequisite layer, the recheck matches the field page `home` on the same red screen, and the
// zero-input fields package reads its field and dispatches no step. Its own recognition decides:
// the run succeeds on `home` with `fields_recorded` and its fields report, and `executed_steps`
// is the prerequisite's one step.
#[test]
fn a23_a_zero_input_fields_package_after_its_prerequisite_records_its_fields_outcome() {
    use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};

    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    screens(&state, vec![DONE, AWAY]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(LEAD, "done", "away", &[("go", "done", "away")], None).write(&root.list(LEAD));
    root.write_bundle();
    let package_path = temp.path().join("fields.zip");
    fs::write(&package_path, fields_package()).expect("write fields package");
    let host = RuntimeHost::start(
        config(&temp).with_standard_packs_root(Some(root.root.clone())),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(FieldsVision::default())),
        ),
    )
    .expect("runtime host");
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        temp.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("official client");

    let output = client
        .run_contained_task(
            ALIAS,
            ContainedTaskRequest::new(package_path.display().to_string()).expect("fields request"),
        )
        .unwrap_or_else(|error| {
            panic!(
                "the fields package's run after its prerequisite succeeds: {} ({error})",
                error.code()
            )
        });
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("run events");
    drop(client);
    host.close().expect("close runtime host");

    assert!(
        matches!(
            output.receipt().result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                executed_steps: 1,
                final_page: Some(page),
                ..
            }) if page == "neutral/home"
        ),
        "{:?}",
        output.receipt()
    );
    let facts = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminals(&facts),
        vec![(Some(1), Some(FIELDS_RECORDED.to_owned()))]
    );
    let completed = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryCompleted {
                package_label,
                final_page,
                role,
                ..
            } => Some((package_label.as_str(), final_page.as_str(), *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed, vec![(LEAD, "neutral/away", PREREQUISITE)]);
    let projection = output
        .official_ocr_fields_projection()
        .expect("the fields report");
    assert_eq!(projection.failure(), None);
    assert_eq!(projection.records().len(), 1);
    let report = serde_json::to_value(projection).expect("fields report JSON");
    assert_eq!(report["records"][0]["fields"][0]["value"]["value"], "home");
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_no_outcome_check_code(&events);
}

// T3: as T1, but `lead` ends on `menu` (one step from `away`), so the dependent dispatches its
// own step `open` from `menu` to `done` as step 1. That step decides: its `StepFinished` names
// `done`, the run succeeds with `done_reached`, and `executed_steps` is 2.
#[test]
fn a23_a_package_that_runs_its_own_step_after_its_prerequisite_is_judged_on_that_step() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(LEAD, "away", "menu", LEAD_STEPS, None).write(&root.list(LEAD));
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));
    let directory = write_with_outcome(&temp, &dependent());

    let run = run_written(&host, &directory);
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        receipt_end(&run.receipt),
        (Some("neutral/done".to_owned()), 2)
    );
    let facts = run.facts();
    assert_eq!(
        terminals(&facts),
        vec![(Some(2), Some(DONE_REACHED.to_owned()))]
    );
    let last_step = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::StepFinished {
                step_index: 1,
                operation_label,
                page_label,
                ..
            } => Some((operation_label.as_str(), page_label.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(last_step, vec![("open", "neutral/done")]);
    assert_no_outcome_check_code(&run.events);
}
