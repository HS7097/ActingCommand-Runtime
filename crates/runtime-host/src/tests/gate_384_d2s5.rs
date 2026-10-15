// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-S5 and D2-S4's remaining rows (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2
// §3.4, §3.9 item 6, §4 O-4, §5.1 MP-3, §5.3 A-5, §6 S5, §7, §8 rows T31, T32, T35, T36, T38,
// T39; host gate key 91).
//
// The fixture is D2-CHAIN's (`gate_384_d2s4`): one instance whose catalog task is triggered by
// `resource.k2`, which the instance does not know, and whose recognition maintenance package
// reads it on its terminal page. Every expected value comes from the model rows named at each
// test.

use super::gate_383_ao4q::ledger_head;
use super::gate_384_d2s4::{
    BUSINESS_TASK, READ_KEY, RECOGNITION_PACKAGE_ID, TerminalReadings, admit_refresh,
    chain_sources, intent_of, recognition_package,
};
use super::*;
use crate::fact_refresh_transitions::FactRefreshRecorder;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, RuntimeConfigManifest,
};
use actingcommand_policy::FACT_REFRESH_TASK_ID;
use actingcommand_runtime_config::scheduling::FACT_REFRESH_OVERDUE_MS;
use serde_json::json;

const PENDING: &str = "fact_refresh_pending";
const UNRESOLVED: &str = "fact_refresh_unresolved";
const RESOLVED: &str = "fact_refresh_resolved";
const OVERDUE: &str = "fact_reader_overdue";
const STALE: &str = "policy_facts_stale";
// Ten minutes, the overdue setting of T38.
const OVERDUE_MS: u64 = 600_000;

// TerminalReadings' text at a confidence of 0.5, below the marks' floor of 900 thousandths: the
// package runs and succeeds, and recognition cannot use its readings (D2 model §3.5).
#[derive(Debug, Default)]
struct LowConfidenceReadings;

impl VisionProvider for LowConfidenceReadings {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        TerminalReadings.require_ocr_model(model_ref, model_sha256)
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        TerminalReadings.require_nn_model(model_ref, model_sha256)
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        TerminalReadings.read_text(request)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<actingcommand_recognition_pack::OcrProviderObservation, VisionProviderError> {
        let mut observation = TerminalReadings.read_text_with_execution_evidence(request)?;
        observation.result.confidence = Some(0.5);
        for block in &mut observation.result.blocks {
            block.confidence = Some(0.5);
        }
        Ok(observation)
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        TerminalReadings.classify(request)
    }
}

// D2-CHAIN's host, reading with `vision`, with `manifest` as its configuration when given.
fn chain_host(
    root: &TempDir,
    vision: Arc<dyn VisionProvider>,
    manifest: Option<RuntimeConfigManifest>,
) -> RuntimeHost {
    let package = root.path().join("recognition.zip");
    fs::write(&package, recognition_package(false)).expect("write the recognition package");
    let state = Arc::new(FakeState::default());
    // The package starts on its home page; after its tap the device shows the terminal page.
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let mut host_config = config(root).with_recognition_packages(BTreeMap::from([(
        POLICY_INSTANCE_ALIAS.to_owned(),
        package,
    )]));
    if let Some(manifest) = manifest {
        host_config = host_config.with_config_manifest(manifest);
    }
    let host = RuntimeHost::start(
        host_config,
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), state)
                .fixture_simulation()
                .with_vision_provider(vision),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&chain_sources())
        .expect("activate the chain catalog");
    host
}

// A configuration that sets `scheduling.fact_refresh.overdue_ms` and nothing else.
fn overdue_manifest(overdue_ms: u64) -> RuntimeConfigManifest {
    let mut parameters = vec![ConfigParameter {
        key: FACT_REFRESH_OVERDUE_MS.to_owned(),
        value: FactScalar::DurationMs(overdue_ms),
        source: ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

// One full policy cycle at `unix_ms`.
fn cycle_at(host: &RuntimeHost, unix_ms: u64, seed: u64) -> PolicyCycle {
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms,
            monotonic_ms: unix_ms,
        },
        seed,
        PolicyTrigger::Recovery,
    )
    .expect("policy cycle")
}

fn now() -> u64 {
    unix_ms_now().expect("wall clock")
}

// The reading of `resource.k2` landing in the instance fact store at `observed_at_unix_ms`, fresh
// for a minute.
fn reading(observed_at_unix_ms: u64) -> FactRecord {
    let mut record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        READ_KEY,
        ContractFactValue::Integer(35),
        "snapshot:d2s5-reading",
        Vec::new(),
    );
    record.observed_at_unix_ms = observed_at_unix_ms;
    record.expires_at_unix_ms = Some(observed_at_unix_ms + 60_000);
    record
}

// The transitions of `cycle` as (cycle number, transition number, instance, key, code).
fn transitions(cycle: &PolicyCycle) -> Vec<(u64, u64, &str, &str, &str)> {
    cycle
        .fact_refresh_transitions
        .iter()
        .map(|transition| {
            (
                transition.cycle_seq,
                transition.transition_seq,
                transition.instance_id.as_str(),
                transition.fact_key.as_str(),
                transition.outcome.code().as_str(),
            )
        })
        .collect()
}

// The transition's outcome carries `values` (D2 model §7.1, §7.2: the code's required values and
// the R_I it names).
fn assert_values(transition: &crate::FactRefreshTransition, values: &[(&str, serde_json::Value)]) {
    for (key, value) in values {
        assert_eq!(
            transition.outcome.value(key),
            Some(value),
            "{key} of {:?}",
            transition.outcome
        );
    }
}

// T35 (D2M:615; §5.3 A-5, v1.3.1 N3): the wait for `resource.k2` goes pending, then unresolved
// (the package ran and succeeded, and recognition could not use its reading), then resolved (the
// reading landed): exactly one transition per change, numbered 1, 2, 3 across the owner epoch's
// cycles (never reset per cycle, no gap); evaluating the same input again adds none.
#[test]
fn d2s5_t35_each_change_of_a_wait_is_one_transition_numbered_through_the_epoch() {
    let root = TempDir::new().expect("tempdir");
    let host = chain_host(&root, Arc::new(LowConfidenceReadings), None);
    let key = json!(READ_KEY);
    let instance = json!(POLICY_INSTANCE_ALIAS);
    let package = json!(RECOGNITION_PACKAGE_ID);

    let started = now();
    let first = cycle_at(&host, started, 71);
    assert_eq!(
        transitions(&first),
        vec![(1, 1, POLICY_INSTANCE_ALIAS, READ_KEY, PENDING)]
    );
    assert_values(
        &first.fact_refresh_transitions[0],
        &[
            ("instance_id", instance.clone()),
            ("fact_key", key.clone()),
            ("unknown_reason", json!("fact_missing")),
            ("package_id", package.clone()),
        ],
    );
    let again = cycle_at(&host, started, 71);
    assert_eq!(transitions(&again), Vec::new(), "the same input again");

    let evaluation = first.evaluation.as_ref().expect("a full evaluation");
    let (refresh, refresh_chain) = intent_of(evaluation, FACT_REFRESH_TASK_ID);
    record_policy_approval(&host, &refresh);
    let (context, request) = admit_refresh(&host, &refresh, &refresh_chain);
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the recognition maintenance package runs");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "a reading recognition cannot use does not fail the run: {:?}",
        receipt.outcome()
    );
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("the fact refresh settles");

    let unresolved = cycle_at(&host, now(), 73);
    assert_eq!(
        transitions(&unresolved),
        vec![(3, 2, POLICY_INSTANCE_ALIAS, READ_KEY, UNRESOLVED)]
    );
    assert_values(
        &unresolved.fact_refresh_transitions[0],
        &[
            ("instance_id", instance.clone()),
            ("fact_key", key.clone()),
            ("unknown_reason", json!("fact_missing")),
            ("package_id", package.clone()),
        ],
    );

    host.publish_fact(reading(now()))
        .expect("the reading lands");
    let resolved = cycle_at(&host, now(), 74);
    assert_eq!(
        transitions(&resolved),
        vec![(4, 3, POLICY_INSTANCE_ALIAS, READ_KEY, RESOLVED)]
    );
    assert_values(
        &resolved.fact_refresh_transitions[0],
        &[
            ("instance_id", instance),
            ("fact_key", key),
            ("package_id", package),
        ],
    );
    host.close().expect("close the host");
}

// T36 (D2M:616; §6 S5 "只产出转换、不参与决策"): the same input evaluated with an empty recorder,
// with a recorder that already holds the wait, and with one whose wait has been reported overdue
// gives the same cycle, member by member, apart from the transitions it reports. Two cycles run
// first, so that every other state the cycles keep has settled.
#[test]
fn d2s5_t36_the_recorder_s_state_never_changes_an_evaluation() {
    let root = TempDir::new().expect("tempdir");
    let host = chain_host(&root, Arc::new(TerminalReadings), None);
    let at = now();
    let warm = cycle_at(&host, at, 61);
    cycle_at(&host, at, 61);
    let evaluation = warm.evaluation.as_ref().expect("a full evaluation");
    let free = |_: &str| false;

    let mut holding = FactRefreshRecorder::default();
    assert_eq!(holding.observe(evaluation, &free, at, None).len(), 1);
    let mut overdue = FactRefreshRecorder::default();
    overdue.observe(evaluation, &free, at - (OVERDUE_MS + 100_000), None);
    let reported = overdue.observe(evaluation, &free, at, Some(OVERDUE_MS));
    assert_eq!(
        reported
            .iter()
            .map(|transition| transition.outcome.code().as_str())
            .collect::<Vec<_>>(),
        vec![OVERDUE],
        "the third recorder's wait is reported overdue"
    );

    let mut cycles = Vec::new();
    for (name, recorder) in [
        ("empty", FactRefreshRecorder::default()),
        ("holding", holding),
        ("overdue", overdue),
    ] {
        host.replace_fact_refresh_recorder_for_test(recorder)
            .expect("set the recorder");
        cycles.push((name, cycle_at(&host, at, 61)));
    }
    let decided = |cycle: &PolicyCycle| {
        (
            cycle.directive.clone(),
            cycle.evaluation.clone(),
            cycle.pending_dispatch_intents.clone(),
            cycle.detection_planning_signals.clone(),
            cycle.clock_slot_signals.clone(),
        )
    };
    let (_, empty) = &cycles[0];
    for (name, cycle) in &cycles[1..] {
        assert_eq!(
            decided(cycle),
            decided(empty),
            "the {name} recorder changed the cycle"
        );
    }
    // Only the report differs: the empty recorder first meets the wait.
    assert_eq!(
        cycles
            .iter()
            .map(|(name, cycle)| (*name, cycle.fact_refresh_transitions.len()))
            .collect::<Vec<_>>(),
        vec![("empty", 1), ("holding", 0), ("overdue", 0)]
    );
    host.close().expect("close the host");
}

// T38 (D2M:618; §3.4 `overdue_ms`, §3.9 item 6): with `overdue_ms` ten minutes, a wait that has
// lasted eleven minutes is reported exactly once, `fact_reader_overdue` with `elapsed_ms` at
// least 600 000 and the package that reads the key; not a millisecond earlier, and not again.
// Without the setting nothing is reported.
#[test]
fn d2s5_t38_a_wait_past_overdue_ms_is_reported_once_and_only_when_set() {
    let root = TempDir::new().expect("tempdir");
    let host = chain_host(
        &root,
        Arc::new(TerminalReadings),
        Some(overdue_manifest(OVERDUE_MS)),
    );
    let started = now();
    let first = cycle_at(&host, started, 81);
    assert_eq!(
        transitions(&first),
        vec![(1, 1, POLICY_INSTANCE_ALIAS, READ_KEY, PENDING)]
    );
    let short = cycle_at(&host, started + OVERDUE_MS - 1, 81);
    assert_eq!(transitions(&short), Vec::new(), "one millisecond short");
    let late = cycle_at(&host, started + 660_000, 81);
    assert_eq!(
        transitions(&late),
        vec![(3, 2, POLICY_INSTANCE_ALIAS, READ_KEY, OVERDUE)]
    );
    let overdue = &late.fact_refresh_transitions[0];
    assert_values(
        overdue,
        &[
            ("instance_id", json!(POLICY_INSTANCE_ALIAS)),
            ("fact_key", json!(READ_KEY)),
            ("package_id", json!(RECOGNITION_PACKAGE_ID)),
        ],
    );
    let elapsed = overdue
        .outcome
        .value("elapsed_ms")
        .and_then(serde_json::Value::as_u64)
        .expect("elapsed_ms");
    assert!(elapsed >= OVERDUE_MS, "elapsed_ms {elapsed}");
    let later = cycle_at(&host, started + 720_000, 81);
    assert_eq!(transitions(&later), Vec::new(), "reported once");
    host.close().expect("close the host");

    let root = TempDir::new().expect("tempdir");
    let host = chain_host(&root, Arc::new(TerminalReadings), None);
    let started = now();
    cycle_at(&host, started, 82);
    let late = cycle_at(&host, started + 660_000, 82);
    assert_eq!(
        transitions(&late),
        Vec::new(),
        "without overdue_ms nothing is reported"
    );
    host.close().expect("close the host");
}

// T39 (D2M:619; §5.1 "S5 的只读查询不是必经点，不写账本"; Fail Loud): the read answers the instance's
// waiting key, its code, the recognition maintenance package and when the wait began, with the
// numbers of the latest cycle and transition; the ledger is the same length before and after it,
// and no record names its request.
#[test]
fn d2s5_t39_the_status_read_answers_the_wait_from_memory_and_appends_nothing() {
    let root = TempDir::new().expect("tempdir");
    let host = chain_host(&root, Arc::new(TerminalReadings), None);
    let started = now();
    cycle_at(&host, started, 91);

    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);
    let head = ledger_head(&mut reader);
    let request = client.request(RuntimeOperation::FactRefreshStatus);
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::FactRefreshStatus { status }) = receipt.result() else {
        panic!("expected a fact refresh status, got {:?}", receipt.result());
    };
    assert_eq!((status.cycle_seq, status.transition_seq), (1, 1));
    let [row] = status.instances.as_slice() else {
        panic!("one instance waits: {status:?}");
    };
    assert_eq!(row.instance_alias, POLICY_INSTANCE_ALIAS);
    let [demand] = row.demands.as_slice() else {
        panic!("one key is waited for: {row:?}");
    };
    assert_eq!(
        (
            demand.fact_key.as_str(),
            demand.outcome.code().as_str(),
            demand.package_id.as_deref(),
            demand.since_unix_ms,
            demand.overdue,
        ),
        (
            READ_KEY,
            PENDING,
            Some(RECOGNITION_PACKAGE_ID),
            started,
            false
        )
    );
    assert_eq!(ledger_head(&mut reader), head, "the read appended a record");
    assert!(
        projected_events(
            &mut reader,
            EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            },
        )
        .is_empty(),
        "the read left a record of its request"
    );
    drop(reader);
    drop(client);
    host.close().expect("close the host");
}

// T31 (D2M:611; §4 O-4): an intent evaluated before the reading lands is refused at admission
// with `policy_facts_stale`; the intent the next evaluation gives after the reading is admitted.
#[test]
fn d2s4_t31_an_intent_evaluated_before_the_reading_is_stale_and_the_next_one_is_admitted() {
    let root = TempDir::new().expect("tempdir");
    let host = chain_host(&root, Arc::new(TerminalReadings), None);
    let before = cycle_at(&host, now(), 31);
    let evaluation = before.evaluation.as_ref().expect("a full evaluation");
    let (refresh, refresh_chain) = intent_of(evaluation, FACT_REFRESH_TASK_ID);
    record_policy_approval(&host, &refresh);

    host.publish_fact(reading(now()))
        .expect("the reading lands");
    let request = host
        .fact_refresh_request(&refresh)
        .expect("the fact refresh request")
        .expect("a request for the fact refresh intent");
    let refused = host
        .admit_scheduled_policy_dispatch(
            &refresh,
            &refresh_chain,
            &policy_context(&host, &refresh),
            &request,
        )
        .expect_err("an intent evaluated before the reading is stale");
    assert_eq!(refused.code(), STALE);

    let after = cycle_at(&host, now(), 32);
    let evaluation = after.evaluation.as_ref().expect("a full evaluation");
    let (business, business_chain) = intent_of(evaluation, BUSINESS_TASK);
    let admission = host
        .admit_policy_dispatch(
            &business,
            &business_chain,
            &policy_context(&host, &business),
        )
        .expect("the intent evaluated after the reading is admitted");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    host.close().expect("close the host");
}

// T32, the run half (D2M:612; §5.1; v1.3.1 N3): through D2-CHAIN's cycles, the dispatch, the run
// and the reading (T18-T19), the recorder's transitions stay in memory: the ledger holds no
// `policy.fact_refresh_transition` (the ledger observer's G5 gate copies them from AO-4 on; D2's
// code writes none), and a cycle that only finds the wait, and reports its transition, adds no
// record of any kind. The code half is the architecture guard
// `d2_t32_the_fact_refresh_code_never_calls_the_ledger_write_interface`.
#[test]
fn d2s4_t32_the_transitions_stay_in_memory_and_the_ledger_gains_no_new_record_kind() {
    let root = TempDir::new().expect("tempdir");
    let host = chain_host(&root, Arc::new(TerminalReadings), None);
    let events = || {
        host.query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events")
    };
    let at = now();
    let first = cycle_at(&host, at, 21);
    assert_eq!(first.fact_refresh_transitions.len(), 1);
    // The same wait met by an empty recorder: one transition, no record.
    host.replace_fact_refresh_recorder_for_test(FactRefreshRecorder::default())
        .expect("set the recorder");
    let recorded = events().len();
    let again = cycle_at(&host, at, 21);
    assert_eq!(
        transitions(&again),
        vec![(1, 1, POLICY_INSTANCE_ALIAS, READ_KEY, PENDING)]
    );
    assert_eq!(
        events().len(),
        recorded,
        "the cycle and its transition added a record"
    );

    let evaluation = again.evaluation.as_ref().expect("a full evaluation");
    let (refresh, refresh_chain) = intent_of(evaluation, FACT_REFRESH_TASK_ID);
    record_policy_approval(&host, &refresh);
    let (context, request) = admit_refresh(&host, &refresh, &refresh_chain);
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the recognition maintenance package runs");
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("the fact refresh settles");
    let resolved = cycle_at(&host, now(), 22);
    assert_eq!(
        transitions(&resolved),
        vec![(2, 2, POLICY_INSTANCE_ALIAS, READ_KEY, RESOLVED)]
    );
    assert!(
        !events()
            .iter()
            .any(|event| event.event_type() == EventType::PolicyFactRefreshTransition),
        "D2's code wrote a transition record"
    );
    host.close().expect("close the host");
}
