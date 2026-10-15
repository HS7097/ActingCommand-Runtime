// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-3 (host gate 92, policy gate 48), frozen scheduling model v1.4.2 §2.4, §10,
// §11, §12 and test rows T10, T11, T13 and T31; M-PKG v1.1 §3.3, §3.4, §5.2; coordinator ruling
// 4 of wave 14. A task package that admission refuses with a code of
// `scheduling.pack_admission.suspend_codes` (by default only
// `recognition_sample_budget_invalid`) is suspended by its `package_id` under this Runtime
// build, with its content hash measured right after the refusal. Every pair whose task's
// procedure locates it, on every instance, is deferred with `pack_admission_suspended` and not
// prepared again; other task packages dispatch as usual. A changed content hash or another build
// lifts it and the package is tried once; the same refusal suspends it again. The evaluation
// copies a suspension once per instance as a `goal_missed` planning signal (no direct write),
// and `actingd suspended` lists the suspended packages under `packs`.

use super::*;
use crate::pack_admission::PackAdmissionHold;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::PolicyEvaluation;
use serde_json::{Value, json};

// The build of the Runtime under test, and another build.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";
// The package admission refuses, and its task on each instance.
const REFUSED_ID: &str = "neutral.suspended.task";
const OBSERVE: &str = "fixture.observe";
const OBSERVE_B: &str = "fixture.observe-b";
// A task of another task package (the neutral fixture package, which admission admits).
const OTHER: &str = "fixture.other";
const OTHER_PROCEDURE: &str = "procedure.detect";
const SUSPEND_CODE: &str = "recognition_sample_budget_invalid";
const SUSPENDED_CODE: &str = "pack_admission_suspended";
const LIFTED_CODE: &str = "pack_admission_lifted";
// The default least time between two measurements of one package.
const LIFT_INTERVAL_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
// Bounds only a broken run: the background admission answers on a channel.
const GATE_WAIT: Duration = Duration::from_secs(30);

// The package the kernel refuses at admission with `recognition_sample_budget_invalid`: page
// `home` declares two frames `consensus_interval_ms` apart while the step timeout is 50 ms (the
// SR-1 T5 package). Another interval is the same package changed in place.
fn refused_package(consensus_interval_ms: u64) -> Vec<u8> {
    let entries = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": REFUSED_ID,
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 2, "height": 1},
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 50,
                "timeout_ms": 5000
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": 2, "height": 1},
                "target_page": "terminal",
                "operations": [{
                    "id": "open_terminal",
                    "from": "home",
                    "to": "terminal",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true,
                    "retryable": false
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    {"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "expected": [255, 0, 0]},
                    {"type": "color", "id": "page/terminal",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                     "expected": [0, 0, 255]},
                    {"type": "color", "id": "guard/ready",
                     "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                     "expected": [0, 255, 0]}
                ],
                "target_consensus": {
                    "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                                  "sample_interval_ms": consensus_interval_ms}
                }
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": [
                {"id": "neutral/home", "required": ["page/home"], "optional": [],
                 "forbidden": []},
                {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
                 "forbidden": []}
            ]}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, &value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    zip.finish().expect("finish zip").into_inner()
}

// Writes an assembled package and gives its content hash: the SHA-256 of the whole file (M-PKG
// D3), in lowercase hex.
fn write_package(path: &Path, bytes: &[u8]) -> String {
    fs::write(path, bytes).expect("write the package");
    format!("{:x}", Sha256::digest(bytes))
}

// The procedure bindings, each at its package's location (`scheduled_execution.package_path`).
fn manifest(bound: &[(&str, &Path)]) -> ProcedureManifest {
    ProcedureManifest::new(bound.iter().map(|(procedure_ref, path)| {
        ProcedureBinding::new(
            *procedure_ref,
            "operation.observe",
            vec!["after_observation".to_owned()],
        )
        .expect("procedure binding")
        .with_package_path(path.to_string_lossy())
    }))
    .expect("procedure manifest")
}

// The fixture catalog with `tasks`: the fixture task's copies, each `(id, instance, procedure,
// priority)`. A failure never escalates to a pause of its pair here, so the package suspension
// alone keeps a pair back.
fn catalog(tasks: &[(&str, &str, &str, u64)]) -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let template = document["tasks"][0].clone();
    document["tasks"] = Value::Array(
        tasks
            .iter()
            .map(|(id, instance, procedure_ref, priority)| {
                let mut task = template.clone();
                task["id"] = json!(id);
                task["scope"] = json!({"kind": "instance", "instance_id": instance});
                task["procedure_ref"] = json!(procedure_ref);
                task["priority"] = json!(priority);
                task["on_failure"]["escalation_threshold"] = json!(1000);
                if *id != OBSERVE {
                    task["produces"] = json!([]);
                    task["instance_overrides"] = json!([]);
                }
                task
            })
            .collect(),
    );
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("fixture task bytes");
    sources
}

fn start(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    commit: &str,
    manifest: ProcedureManifest,
    hold: Arc<PackAdmissionHold>,
    aliases: &[&str],
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_runtime_commit(commit)
            .with_procedure_manifest(manifest)
            .with_pack_admission_hold_for_test(hold),
        Arc::new(
            FakeProvider::from_entries(aliases.iter().map(|alias| {
                (
                    (*alias).to_owned(),
                    instance_id(),
                    Arc::new(FakeState::default()),
                )
            }))
            .fixture_simulation(),
        ),
    )
    .expect("runtime host")
}

// A hold whose release is dropped at once: the background admission after start does nothing,
// so only a scheduled run's preparation finds a suspension.
fn no_background_admission() -> Arc<PackAdmissionHold> {
    let (hold, _release, _finished) = PackAdmissionHold::open();
    hold
}

// Evaluates at `at` (the Runtime clock moved there first).
fn evaluate(
    host: &RuntimeHost,
    clock: &Arc<ManualRuntimeClock>,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    at: u64,
    seed: u64,
) -> PolicyEvaluation {
    advance_manual_clock_to(clock, at);
    host.evaluate_policy_cycle_with_test_inputs(
        facts,
        resources,
        EvaluationTime {
            unix_ms: at,
            monotonic_ms: at,
        },
        seed,
        PolicyTrigger::FactsChanged,
    )
    .expect("evaluate the policy")
    .evaluation
    .expect("an evaluation")
}

fn intent_of<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
    instance: &str,
) -> Option<&'a DispatchIntent> {
    evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == instance)
}

// The `pack_admission_suspended` reason that defers the pair, if any.
fn suspension_of(evaluation: &PolicyEvaluation, task_id: &str, instance: &str) -> Option<Outcome> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(instance)
        })
        .flat_map(|decision| decision.reasons.iter())
        .find(|reason| reason.outcome.code().as_str() == SUSPENDED_CODE)
        .map(|reason| reason.outcome.clone())
}

// Admits the evaluated dispatch of the pair (after recording the catalog's approval, once per
// host, when `approve`) and runs it; its preparation refuses the package.
fn dispatch_refused(
    host: &RuntimeHost,
    evaluation: &PolicyEvaluation,
    (task_id, instance): (&str, &str),
    package: &Path,
    approve: bool,
) {
    let intent = intent_of(evaluation, task_id, instance)
        .unwrap_or_else(|| panic!("{task_id} on {instance} is dispatched: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the dispatch reason chain")
        .clone();
    if approve {
        record_policy_approval(host, &intent);
    }
    // A capacity sample is fresh for twice the monitor's interval on the Runtime clock, and
    // admission refuses a stale one (contracts/capacity-admission.md "capacity freshness"). The
    // evaluation moved the Runtime clock, past the lift interval after an hour in T11, which
    // leaves the monitor's last sample stale, so capacity is sampled at the admission time, as
    // the monitor does while time passes.
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(host, &intent))
        .expect("the dispatch is admitted")
    else {
        panic!("expected a granted dispatch")
    };
    let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .expect("contained task request");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the preparation refuses the package");
    assert_eq!(error.code(), SUSPEND_CODE);
}

// The suspension's values: the package, its content hash, the refusal's code and the build.
fn assert_suspension(outcome: &Outcome, hash: &str, commit: &str, case: &str) {
    assert_eq!(
        outcome.value("package_id"),
        Some(&json!(REFUSED_ID)),
        "{case}"
    );
    assert_eq!(
        outcome.value("actual_digest"),
        Some(&json!(format!("sha256:{hash}"))),
        "{case}"
    );
    assert_eq!(
        outcome.value("failure_code"),
        Some(&json!(SUSPEND_CODE)),
        "{case}"
    );
    assert_eq!(
        outcome.value("runtime_commit"),
        Some(&json!(commit)),
        "{case}"
    );
}

// The planning signals that copy a task package suspension or lift: (code, instance, task,
// outcome).
fn copies(host: &RuntimeHost) -> Vec<(String, String, Option<String>, Outcome)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        ..EventQuery::default()
    })
    .expect("planning signals")
    .into_iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
            if payload.signal_id().starts_with("signal:pack_admission:") =>
        {
            assert_eq!(payload.kind(), PolicyPlanningSignalKind::GoalMissed);
            Some((
                payload.outcome().code().as_str().to_owned(),
                payload.instance_id().to_owned(),
                payload.task_id().map(str::to_owned),
                payload.outcome().clone(),
            ))
        }
        _ => None,
    })
    .collect()
}

// T10 (L726 "挂起并且不再加载"; L748 Q4 "只能按任务包挂起"; §2.4 item 3): the first preparation
// that refuses the package with `recognition_sample_budget_invalid` suspends the task package by
// its `package_id`, with its content hash; every task bound to it, on every instance, is then
// deferred with `pack_admission_suspended` and no longer dispatched, so it is not prepared
// again; a task of another task package dispatches as usual.
#[test]
fn sr3_t10_the_first_refusal_suspends_the_task_package_on_every_instance() {
    let root = TempDir::new().expect("tempdir");
    let refused = root.path().join("refused.zip");
    let admitted = root.path().join("admitted.zip");
    let hash = write_package(&refused, &refused_package(100));
    write_package(&admitted, &neutral_contained_task_package(false));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(
        &root,
        &clock,
        COMMIT,
        manifest(&[
            ("procedure.observe", refused.as_path()),
            ("procedure.observe-b", refused.as_path()),
            (OTHER_PROCEDURE, admitted.as_path()),
        ]),
        no_background_admission(),
        &[POLICY_INSTANCE_ALIAS, POLICY_INSTANCE_ALIAS_B],
    );
    host.activate_policy_catalog(&catalog(&[
        (OBSERVE, POLICY_INSTANCE_ALIAS, "procedure.observe", 100),
        (
            OBSERVE_B,
            POLICY_INSTANCE_ALIAS_B,
            "procedure.observe-b",
            100,
        ),
        (OTHER, POLICY_INSTANCE_ALIAS_B, OTHER_PROCEDURE, 50),
    ]))
    .expect("activate the catalog");
    let (facts, resources) = (pending_policy_facts(), pending_policy_resources());

    let first = evaluate(&host, &clock, &facts, &resources, POLICY_NOW_UNIX_MS, 101);
    assert!(suspension_of(&first, OBSERVE, POLICY_INSTANCE_ALIAS).is_none());
    dispatch_refused(
        &host,
        &first,
        (OBSERVE, POLICY_INSTANCE_ALIAS),
        &refused,
        true,
    );

    let due = POLICY_NOW_UNIX_MS + HOUR_MS;
    let next = evaluate(&host, &clock, &facts, &resources, due, 102);
    for (task_id, instance) in [
        (OBSERVE, POLICY_INSTANCE_ALIAS),
        (OBSERVE_B, POLICY_INSTANCE_ALIAS_B),
    ] {
        let case = format!("{task_id} on {instance}");
        assert!(intent_of(&next, task_id, instance).is_none(), "{case}");
        let suspension = suspension_of(&next, task_id, instance)
            .unwrap_or_else(|| panic!("{case} is deferred by the suspension: {next:#?}"));
        assert_suspension(&suspension, &hash, COMMIT, &case);
    }
    assert!(
        intent_of(&next, OTHER, POLICY_INSTANCE_ALIAS_B).is_some(),
        "another task package of the instance dispatches: {next:#?}"
    );
    host.close().expect("close the host");
}

// T11 (§2.4 item 4): the package changed in place (one file) lifts the suspension at the pair's
// first due evaluation after the lift interval, and the package is tried once; refused again
// under the new content hash it is suspended again and not tried in a loop. Another Runtime
// build starts without the suspension: the package is tried once and, refused again, suspended
// again under that build.
#[test]
fn sr3_t11_a_changed_hash_or_build_lifts_and_tries_once_and_the_same_refusal_suspends_again() {
    let root = TempDir::new().expect("tempdir");
    let refused = root.path().join("refused.zip");
    let first_hash = write_package(&refused, &refused_package(100));
    let bound = || manifest(&[("procedure.observe", refused.as_path())]);
    let tasks = [(OBSERVE, POLICY_INSTANCE_ALIAS, "procedure.observe", 100)];
    let (facts, resources) = (policy_facts(), policy_resources());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(
        &root,
        &clock,
        COMMIT,
        bound(),
        no_background_admission(),
        &[POLICY_INSTANCE_ALIAS],
    );
    host.activate_policy_catalog(&catalog(&tasks))
        .expect("activate the catalog");

    let at = POLICY_NOW_UNIX_MS;
    let evaluation = evaluate(&host, &clock, &facts, &resources, at, 111);
    dispatch_refused(
        &host,
        &evaluation,
        (OBSERVE, POLICY_INSTANCE_ALIAS),
        &refused,
        true,
    );
    let at = at + HOUR_MS;
    let evaluation = evaluate(&host, &clock, &facts, &resources, at, 112);
    let suspension = suspension_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS)
        .expect("suspended after the refusal");
    assert_suspension(&suspension, &first_hash, COMMIT, "first suspension");

    let changed_hash = write_package(&refused, &refused_package(200));
    let at = at + LIFT_INTERVAL_MS + 2_000;
    let evaluation = evaluate(&host, &clock, &facts, &resources, at, 113);
    assert!(
        suspension_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS).is_none(),
        "a changed content hash lifts the suspension: {evaluation:#?}"
    );
    let lifted = copies(&host)
        .into_iter()
        .filter(|(code, ..)| code == LIFTED_CODE)
        .collect::<Vec<_>>();
    assert_eq!(lifted.len(), 1, "{lifted:?}");
    assert_eq!(lifted[0].1, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        lifted[0].3.value("actual_digest"),
        Some(&json!(format!("sha256:{changed_hash}")))
    );
    assert_eq!(lifted[0].3.value("runtime_commit"), Some(&json!(COMMIT)));
    dispatch_refused(
        &host,
        &evaluation,
        (OBSERVE, POLICY_INSTANCE_ALIAS),
        &refused,
        false,
    );
    let at = at + HOUR_MS;
    let evaluation = evaluate(&host, &clock, &facts, &resources, at, 114);
    assert!(intent_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS).is_none());
    let suspension = suspension_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS)
        .expect("the same refusal under the new hash suspends again");
    assert_suspension(&suspension, &changed_hash, COMMIT, "suspended again");
    host.close().expect("close before the build changes");

    let at = at + HOUR_MS;
    let clock = Arc::new(ManualRuntimeClock::new(at, 0));
    let host = start(
        &root,
        &clock,
        OTHER_COMMIT,
        bound(),
        no_background_admission(),
        &[POLICY_INSTANCE_ALIAS],
    );
    let evaluation = evaluate(&host, &clock, &facts, &resources, at, 115);
    assert!(
        suspension_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS).is_none(),
        "another build starts without the suspension: {evaluation:#?}"
    );
    dispatch_refused(
        &host,
        &evaluation,
        (OBSERVE, POLICY_INSTANCE_ALIAS),
        &refused,
        true,
    );
    let at = at + HOUR_MS;
    let evaluation = evaluate(&host, &clock, &facts, &resources, at, 116);
    assert!(intent_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS).is_none());
    let suspension = suspension_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS)
        .expect("refused again under the other build");
    assert_suspension(&suspension, &changed_hash, OTHER_COMMIT, "other build");
    host.close().expect("close the host");
}

// T13 (L726 "向外部例如MCP/CLI/人类发出提示"; SPEC §9), the `actingd suspended` half: the report's
// `packs` lists the suspended package with the refusal's code, the build, the content hash
// prefix measured at the suspension and now, and the instances it defers; every member is a
// code, an id or a hash, no sentence.
#[test]
fn sr3_t13_actingd_suspended_lists_the_suspended_package_under_packs() {
    let root = TempDir::new().expect("tempdir");
    let refused = root.path().join("refused.zip");
    let hash = write_package(&refused, &refused_package(100));
    let bound = || manifest(&[("procedure.observe", refused.as_path())]);
    let (facts, resources) = (policy_facts(), policy_resources());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(
        &root,
        &clock,
        COMMIT,
        bound(),
        no_background_admission(),
        &[POLICY_INSTANCE_ALIAS],
    );
    let catalog = catalog(&[(OBSERVE, POLICY_INSTANCE_ALIAS, "procedure.observe", 100)]);
    host.activate_policy_catalog(&catalog)
        .expect("activate the catalog");
    let evaluation = evaluate(&host, &clock, &facts, &resources, POLICY_NOW_UNIX_MS, 131);
    dispatch_refused(
        &host,
        &evaluation,
        (OBSERVE, POLICY_INSTANCE_ALIAS),
        &refused,
        true,
    );
    let evaluation = evaluate(
        &host,
        &clock,
        &facts,
        &resources,
        POLICY_NOW_UNIX_MS + HOUR_MS,
        132,
    );
    assert!(suspension_of(&evaluation, OBSERVE, POLICY_INSTANCE_ALIAS).is_some());
    host.close().expect("close the host");

    let report = crate::suspension_report(&SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &config(&root).with_procedure_manifest(bound()),
        catalog: &catalog,
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"));
    let prefix = hash.chars().take(12).collect::<String>();
    assert_eq!(
        report["packs"],
        json!([{
            "package_id": REFUSED_ID,
            "failure_code": SUSPEND_CODE,
            "runtime_commit": COMMIT,
            "paused_hash_prefix": prefix,
            "current_hash_prefix": prefix,
            "instances": [POLICY_INSTANCE_ALIAS],
        }]),
        "{report:#}"
    );
    for (member, value) in report["packs"][0].as_object().expect("a packs row") {
        let texts = match value {
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
            other => other.as_str().into_iter().collect(),
        };
        assert!(
            texts.iter().all(|text| !text.contains(char::is_whitespace)),
            "{member} carries a sentence: {value}"
        );
    }
}

// T31 (§2.4 item 2, review L4; §11; v1.3 item 4): the start does not admit the task packages;
// the admission after start runs in the background and suspends the package it refuses with a
// suspend code; that package's pair is deferred, a task of another package dispatches, and the
// ledger has one `pack_admission_suspended`, copied by the evaluation as a planning signal, and
// no direct write of it.
#[test]
fn sr3_t31_the_admission_after_start_suspends_in_the_background_and_the_evaluation_copies_it() {
    let root = TempDir::new().expect("tempdir");
    let refused = root.path().join("refused.zip");
    let admitted = root.path().join("admitted.zip");
    let hash = write_package(&refused, &refused_package(100));
    write_package(&admitted, &neutral_contained_task_package(false));
    let (facts, resources) = (policy_facts(), policy_resources());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let (hold, release, finished) = PackAdmissionHold::open();
    let host = start(
        &root,
        &clock,
        COMMIT,
        manifest(&[
            ("procedure.observe", refused.as_path()),
            (OTHER_PROCEDURE, admitted.as_path()),
        ]),
        hold,
        &[POLICY_INSTANCE_ALIAS],
    );
    host.activate_policy_catalog(&catalog(&[
        (OBSERVE, POLICY_INSTANCE_ALIAS, "procedure.observe", 100),
        (OTHER, POLICY_INSTANCE_ALIAS, OTHER_PROCEDURE, 50),
    ]))
    .expect("activate the catalog");

    // The start has finished and has not admitted the packages.
    let before = evaluate(&host, &clock, &facts, &resources, POLICY_NOW_UNIX_MS, 311);
    assert!(
        suspension_of(&before, OBSERVE, POLICY_INSTANCE_ALIAS).is_none(),
        "{before:#?}"
    );
    assert!(copies(&host).is_empty());

    release.send(()).expect("release the background admission");
    finished
        .recv_timeout(GATE_WAIT)
        .expect("the background admission finishes");
    let after = evaluate(
        &host,
        &clock,
        &facts,
        &resources,
        POLICY_NOW_UNIX_MS + 1_000,
        312,
    );
    assert!(intent_of(&after, OBSERVE, POLICY_INSTANCE_ALIAS).is_none());
    let suspension = suspension_of(&after, OBSERVE, POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("the background suspension defers the pair: {after:#?}"));
    assert_suspension(&suspension, &hash, COMMIT, "background");
    assert!(
        intent_of(&after, OTHER, POLICY_INSTANCE_ALIAS).is_some(),
        "another task package dispatches: {after:#?}"
    );

    let copied = copies(&host);
    assert_eq!(copied.len(), 1, "{copied:?}");
    let (code, instance, task_id, outcome) = &copied[0];
    assert_eq!(code, SUSPENDED_CODE);
    assert_eq!(instance, POLICY_INSTANCE_ALIAS);
    assert_eq!(task_id.as_deref(), Some(OBSERVE));
    assert_suspension(outcome, &hash, COMMIT, "the copy");
    let direct = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("runtime failures")
        .into_iter()
        .filter(|event| {
            serde_json::to_string(event.payload())
                .expect("payload JSON")
                .contains(SUSPENDED_CODE)
        })
        .count();
    assert_eq!(direct, 0, "no direct write of the suspension");
    host.close().expect("close the host");
}
