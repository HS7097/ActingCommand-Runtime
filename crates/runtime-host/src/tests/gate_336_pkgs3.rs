// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S3 (option 丙; host gate 75), model M-PKG v1.1 §3.3, §3.4 and §4.4 S3-T1,
// S3-T3 to S3-T5, S3-T8 and S3-T9. A procedure binding names no package content; a paused pair
// keeps the content hashes its failure identity recorded, and a lift check measures the
// packages again, at most once per `scheduling.suspension.lift_check_min_interval_ms` (60 s by
// default): a package changed in place lifts the pause at the pair's next due evaluation,
// without a restart; a package that cannot be measured keeps the pause and is reported with
// `lift_check_hash_unmeasured`.

use super::*;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityLayer, IdentityPackage};
use actingcommand_contract::ContainedTaskRecoveryBinding;
use actingcommand_contract::outcome::Outcome;
use serde_json::Value;

// The paused task and its instance (the fixture catalog's one task).
const TASK: &str = "fixture.observe";
// The bound procedure and the package ids its failure identity names.
const PROCEDURE: &str = "procedure.observe";
const MAIN_ID: &str = "neutral.main";
const LAYER_ID: &str = "neutral.layer";
// The default least time between two measurements of one package.
const LIFT_INTERVAL_MS: u64 = 60_000;
// The first lift check: an hour after the paused dispatch, when the task is due again.
const FIRST_CHECK_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000;

// The packages of a pause: the main package the binding locates and a declared layer.
struct Packages {
    main: PathBuf,
    layer: PathBuf,
}

impl Packages {
    fn new(root: &TempDir) -> Self {
        let packages = Self {
            main: root.path().join("main.zip"),
            layer: root.path().join("layer.zip"),
        };
        write_package(&packages.main, &neutral_contained_task_package(false));
        write_package(&packages.layer, &neutral_contained_task_package(true));
        packages
    }
}

// Writes an assembled package and gives the first twelve hex digits of its content hash: the
// SHA-256 of the whole file (M-PKG D3).
fn write_package(path: &Path, bytes: &[u8]) -> String {
    fs::write(path, bytes).expect("write the package");
    format!("{:x}", Sha256::digest(bytes))
        .chars()
        .take(12)
        .collect()
}

fn id8(package_id: &str) -> String {
    format!("{:x}", Sha256::digest(package_id.as_bytes()))
        .chars()
        .take(8)
        .collect()
}

// The fixture manifest with `procedure.observe` bound at `main`, as actingd binds a scheduled
// procedure (`scheduled_execution.package_path`).
fn manifest_at(main: &Path) -> ProcedureManifest {
    ProcedureManifest::new([PROCEDURE, "procedure.observe-b", "procedure.detect"].map(
        |procedure_ref| {
            let binding = ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding");
            if procedure_ref == PROCEDURE {
                binding.with_package_path(main.to_string_lossy())
            } else {
                binding
            }
        },
    ))
    .expect("procedure manifest")
}

// The host configuration: the binding at the main package and the layer's mapping.
fn suspension_config(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    packages: &Packages,
) -> RuntimeHostConfig {
    config(root)
        .with_runtime_clock(clock.clone())
        .with_procedure_manifest(manifest_at(&packages.main))
        .with_prerequisite_packages(BTreeMap::from([(
            LAYER_ID.to_owned(),
            ContainedTaskRecoveryBinding::new(packages.layer.to_string_lossy())
                .expect("layer mapping"),
        )]))
}

fn start(root: &TempDir, clock: &Arc<ManualRuntimeClock>, packages: &Packages) -> RuntimeHost {
    RuntimeHost::start(
        suspension_config(root, clock, packages),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

// The failure identity of a main-scope failure whose run went through the main package and
// the layer, with the hash prefixes (or `x`) its settlement measured.
fn identity(main_hash: &str, layer_hash: &str) -> String {
    FailureIdentity {
        base: "page_confirmation_failed".to_owned(),
        key: "0".repeat(12),
        main: IdentityPackage {
            id_hash: id8(MAIN_ID),
            hash: main_hash.to_owned(),
        },
        layers: vec![IdentityLayer::Declared(IdentityPackage {
            id_hash: id8(LAYER_ID),
            hash: layer_hash.to_owned(),
        })],
        frame: IdentityFrame::None,
    }
    .encode()
}

// Dispatches the task at POLICY_NOW_UNIX_MS and settles it with a severe failure carrying
// `failure_identity`, which pauses the pair at once.
fn pause(host: &RuntimeHost, clock: &Arc<ManualRuntimeClock>, failure_identity: String) {
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate the catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(host, PolicyTrigger::FactsChanged);
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { .. } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(host, &intent))
        .expect("the dispatch is admitted")
    else {
        panic!("expected a granted dispatch")
    };
    clock.advance(100);
    let execution = host
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            &PolicyExecutionInput::Failed {
                outcome: Outcome::new(
                    actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed,
                ),
                failure_identity: Some(failure_identity),
                class: PolicyFailureClass::Severe,
            },
        )
        .expect("the failure is recorded");
    let PolicyExecutionOutcome::Failed { failure } = &execution.outcome else {
        panic!("expected a failed execution")
    };
    assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
}

// What one evaluation decided for the paused pair.
struct PairState {
    dispatched: bool,
    // The pause that kept it, with its links.
    paused: Option<Outcome>,
}

// Evaluates at `at` (the Runtime clock moved there first) and reads the pair's decision.
fn evaluate(host: &RuntimeHost, clock: &Arc<ManualRuntimeClock>, at: u64, seed: u64) -> PairState {
    advance_manual_clock_to(clock, at);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy");
    let evaluation = cycle.evaluation.expect("an evaluation");
    let dispatched = evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == TASK && intent.instance_id == POLICY_INSTANCE_ALIAS);
    let paused = evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .find(|reason| reason.outcome.code().as_str() == "policy_task_paused")
        .map(|reason| reason.outcome.clone());
    PairState { dispatched, paused }
}

fn assert_paused(state: &PairState, case: &str) {
    assert!(!state.dispatched, "{case}: the pair is not dispatched");
    assert!(state.paused.is_some(), "{case}: the pause keeps the pair");
}

fn assert_lifted(state: &PairState, case: &str) {
    assert!(state.dispatched, "{case}: the lifted pair is dispatched");
    assert!(state.paused.is_none(), "{case}: no pause keeps the pair");
}

// The `lift_check_hash_unmeasured` links of a pause.
fn unmeasured(pause: &Outcome) -> Vec<Outcome> {
    pause
        .causes()
        .iter()
        .map(|link| link.outcome().clone())
        .filter(|link| link.code().as_str() == "lift_check_hash_unmeasured")
        .collect()
}

// `actingd suspended` on the closed state root, with the host's configuration.
fn report(root: &TempDir, clock: &Arc<ManualRuntimeClock>, packages: &Packages) -> Value {
    crate::suspension_report(&SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &suspension_config(root, clock, packages),
        catalog: &policy_sources(1),
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"))
}

// S3-T1: a binding is its procedure, operation and yield points; its digest does not change
// when its package moves or changes in place, the evaluated intent and the dispatch record name
// the binding and no package digest, and an alias rebound to another location still admits the
// decision evaluated before.
#[test]
fn s3_t1_a_binding_names_no_package_and_a_dispatch_records_no_digest() {
    let root = TempDir::new().expect("tempdir");
    let packages = Packages::new(&root);
    let manifest = manifest_at(&packages.main);
    let binding_digest = manifest
        .binding(PROCEDURE)
        .expect("bound procedure")
        .binding_digest()
        .to_owned();
    let moved = manifest_at(&root.path().join("elsewhere.zip"));
    assert_eq!(
        moved
            .binding(PROCEDURE)
            .expect("rebound procedure")
            .binding_digest(),
        binding_digest
    );
    write_package(
        &packages.main,
        &neutral_non_home_start_contained_task_package(false),
    );
    assert_eq!(
        manifest_at(&packages.main)
            .binding(PROCEDURE)
            .expect("bound procedure")
            .binding_digest(),
        binding_digest
    );

    let host = RuntimeHost::start(
        config(&root).with_procedure_manifest(manifest),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate the catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert_eq!(
        intent.procedure_binding_digest.as_deref(),
        Some(binding_digest.as_str())
    );
    let intent_json = serde_json::to_value(&intent).expect("intent JSON");
    assert!(intent_json.get("package_digest").is_none(), "{intent_json}");

    host.replace_procedure_manifest_for_test(moved)
        .expect("rebind the alias to another location");
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the decision is admitted after the rebind");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));

    let mut client = TestClient::connect(&host);
    let recorded = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyDispatchIntent),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .filter_map(|event| match event.payload {
        ProjectionPayload::Full(payload) => match *payload {
            EventPayload::Policy(PolicyPayload::DispatchIntent(payload)) => {
                Some(serde_json::to_value(&payload).expect("dispatch record JSON"))
            }
            _ => None,
        },
        _ => None,
    })
    .collect::<Vec<_>>();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(
        recorded[0].get("procedure_binding_digest"),
        Some(&Value::String(binding_digest))
    );
    assert!(recorded[0].get("package_digest").is_none(), "{recorded:?}");
    drop(client);
    host.close().expect("close the host");
}

// S3-T3: the main package changed in place lifts the pause once the interval has passed since
// the package was last measured, at the pair's next due evaluation, without a restart; within
// the interval the earlier measurement stands.
#[test]
fn s3_t3_a_main_package_changed_in_place_lifts_after_the_interval_without_a_restart() {
    let root = TempDir::new().expect("tempdir");
    let packages = Packages::new(&root);
    let main_hash = write_package(&packages.main, &neutral_contained_task_package(false));
    let layer_hash = write_package(&packages.layer, &neutral_contained_task_package(true));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(&root, &clock, &packages);
    pause(&host, &clock, identity(&main_hash, &layer_hash));

    assert_paused(
        &evaluate(&host, &clock, FIRST_CHECK_UNIX_MS, 31),
        "unchanged",
    );
    write_package(
        &packages.main,
        &neutral_non_home_start_contained_task_package(false),
    );
    assert_paused(
        &evaluate(&host, &clock, FIRST_CHECK_UNIX_MS + 2_000, 32),
        "changed within the interval",
    );
    assert_lifted(
        &evaluate(
            &host,
            &clock,
            FIRST_CHECK_UNIX_MS + LIFT_INTERVAL_MS + 2_000,
            33,
        ),
        "changed, the interval passed",
    );
    host.close().expect("close the host");
}

// S3-T4: an unchanged package and a change to a package the pause does not name keep the
// pause; a changed prerequisite layer lifts it.
#[test]
fn s3_t4_only_a_package_the_pause_names_lifts_it() {
    for (case, change, lifts) in [
        ("nothing changed", None, false),
        ("another package changed", Some("other.zip"), false),
        ("the layer changed", Some("layer.zip"), true),
    ] {
        let root = TempDir::new().expect("tempdir");
        let packages = Packages::new(&root);
        let main_hash = write_package(&packages.main, &neutral_contained_task_package(false));
        let layer_hash = write_package(&packages.layer, &neutral_contained_task_package(true));
        let other = root.path().join("other.zip");
        write_package(&other, &neutral_contained_task_package(false));
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = start(&root, &clock, &packages);
        pause(&host, &clock, identity(&main_hash, &layer_hash));
        assert_paused(&evaluate(&host, &clock, FIRST_CHECK_UNIX_MS, 41), case);
        if let Some(file) = change {
            write_package(
                &root.path().join(file),
                &neutral_non_home_start_contained_task_package(true),
            );
        }
        let state = evaluate(
            &host,
            &clock,
            FIRST_CHECK_UNIX_MS + LIFT_INTERVAL_MS + 2_000,
            42,
        );
        if lifts {
            assert_lifted(&state, case);
        } else {
            assert_paused(&state, case);
        }
        host.close().expect("close the host");
    }
}

// S3-T5: `actingd suspended` shows the hash prefixes the pause recorded and the ones measured
// now, and that a content hash change lifts it; once the main package changed in place, the
// pair is listed as lifted by it, effective at the daemon's next evaluation.
#[test]
fn s3_t5_actingd_suspended_reports_the_paused_and_current_hash_prefixes() {
    let root = TempDir::new().expect("tempdir");
    let packages = Packages::new(&root);
    let main_hash = write_package(&packages.main, &neutral_contained_task_package(false));
    let layer_hash = write_package(&packages.layer, &neutral_contained_task_package(true));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(&root, &clock, &packages);
    pause(&host, &clock, identity(&main_hash, &layer_hash));
    host.close().expect("close the host");

    let suspended = report(&root, &clock, &packages);
    let entry = &suspended["suspended"][0];
    assert_eq!(entry["task_id"], TASK, "{suspended:#}");
    assert_eq!(entry["lifts_when"], "content_hash_changes");
    assert_eq!(entry["package"]["paused_hash_prefix"], main_hash.as_str());
    assert_eq!(entry["package"]["current_hash_prefix"], main_hash.as_str());
    assert_eq!(
        entry["package"]["layers"],
        serde_json::json!([{
            "kind": "declared",
            "package_id": LAYER_ID,
            "paused_hash_prefix": layer_hash,
            "current_hash_prefix": layer_hash,
            "changed": false,
        }])
    );
    for member in [
        "paused_digest_prefix",
        "current_digest_prefix",
        "paused",
        "current",
    ] {
        assert!(entry["package"].get(member).is_none(), "{member}");
    }
    assert_eq!(suspended["lifted"], serde_json::json!([]));

    let changed_hash = write_package(
        &packages.main,
        &neutral_non_home_start_contained_task_package(false),
    );
    let lifted = report(&root, &clock, &packages);
    assert_eq!(lifted["suspended"], serde_json::json!([]), "{lifted:#}");
    let entry = &lifted["lifted"][0];
    assert_eq!(entry["task_id"], TASK);
    assert_eq!(entry["lifted_by"], "main_hash");
    assert_eq!(entry["effective"], "next_evaluation");
    assert_eq!(entry["package"]["paused_hash_prefix"], main_hash.as_str());
    assert_eq!(
        entry["package"]["current_hash_prefix"],
        changed_hash.as_str()
    );
}

// S3-T8 (lift half, D18): a main package that cannot be measured at a check keeps the pause and
// reports `lift_check_hash_unmeasured` with its path on the pause; once it is back with other
// content, the next check after the interval lifts the pause. A pause whose settlement could
// not measure the main package (`x`) lifts at the first check that measures it.
#[test]
fn s3_t8_an_unmeasured_package_keeps_the_pause_and_a_measured_x_lifts_it() {
    let root = TempDir::new().expect("tempdir");
    let packages = Packages::new(&root);
    let main_hash = write_package(&packages.main, &neutral_contained_task_package(false));
    let layer_hash = write_package(&packages.layer, &neutral_contained_task_package(true));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(&root, &clock, &packages);
    pause(&host, &clock, identity(&main_hash, &layer_hash));
    fs::remove_file(&packages.main).expect("remove the main package");
    let state = evaluate(&host, &clock, FIRST_CHECK_UNIX_MS, 81);
    assert_paused(&state, "removed");
    let warnings = unmeasured(state.paused.as_ref().expect("the pause"));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        warnings[0].value("path").and_then(Value::as_str),
        Some(packages.main.to_string_lossy().as_ref())
    );
    write_package(
        &packages.main,
        &neutral_non_home_start_contained_task_package(false),
    );
    assert_lifted(
        &evaluate(
            &host,
            &clock,
            FIRST_CHECK_UNIX_MS + LIFT_INTERVAL_MS + 2_000,
            82,
        ),
        "back with other content",
    );
    host.close().expect("close the host");

    let root = TempDir::new().expect("tempdir");
    let packages = Packages::new(&root);
    let layer_hash = write_package(&packages.layer, &neutral_contained_task_package(true));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(&root, &clock, &packages);
    pause(&host, &clock, identity("x", &layer_hash));
    assert_lifted(
        &evaluate(&host, &clock, FIRST_CHECK_UNIX_MS, 83),
        "unmeasured at the settlement, measured now",
    );
    host.close().expect("close the host");
}

// S3-T9: the hashes a pause recorded come back from its failure identity after a restart: an
// unchanged package keeps the pair paused across a normal restart, and a package changed while
// the Runtime was stopped lifts it after the next start.
#[test]
fn s3_t9_a_restart_rebuilds_the_recorded_hashes_from_the_failure_identity() {
    let root = TempDir::new().expect("tempdir");
    let packages = Packages::new(&root);
    let main_hash = write_package(&packages.main, &neutral_contained_task_package(false));
    let layer_hash = write_package(&packages.layer, &neutral_contained_task_package(true));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(&root, &clock, &packages);
    pause(&host, &clock, identity(&main_hash, &layer_hash));
    host.close().expect("close before the restart");

    let restarted = start(&root, &clock, &packages);
    assert_paused(
        &evaluate(&restarted, &clock, FIRST_CHECK_UNIX_MS, 91),
        "unchanged across a restart",
    );
    restarted.close().expect("close before the second restart");

    write_package(
        &packages.main,
        &neutral_non_home_start_contained_task_package(false),
    );
    let restarted = start(&root, &clock, &packages);
    assert_lifted(
        &evaluate(&restarted, &clock, FIRST_CHECK_UNIX_MS + 2_000, 92),
        "changed while stopped",
    );
    restarted.close().expect("close the host");
}
