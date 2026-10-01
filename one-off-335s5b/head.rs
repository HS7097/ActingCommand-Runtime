// SPDX-License-Identifier: AGPL-3.0-only

//! one-off (to be reverted): Workflow #335 S5b checks on the PR head (model section 6, H1-H6,
//! H9, the T3 sequence and the T4 details). Lines start with the check name. The H1 state root
//! is copied to `ONE_OFF_335S5B_STATE` for H7 (the merge-base build opens it).

use super::one_off_335s5b_common::*;
use super::*;
use serde_json::{Value, json};

const TARGET_AMOUNT: u64 = 300_000_000;

fn target_document(valid_until_unix_ms: u64) -> String {
    json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": POLICY_INSTANCE_ALIAS,
        "valid_until_unix_ms": valid_until_unix_ms,
        "targets": [{
            "id": "primary-floor",
            "resource": "fixture-pool-a",
            "condition": {"kind": "at_least", "amount": TARGET_AMOUNT},
            "scale": 1000,
            "importance_milli": 100,
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    })
    .to_string()
}

fn apply_targets(tag: &str, label: &str, bench: &Bench, document: &str) {
    let mut client = TestClient::connect(&bench.host);
    let request = client.agent_request(RuntimeOperation::ApplyResourceTargets {
        document_json: document.to_owned(),
    });
    let receipt = client.send(&request);
    match receipt.result() {
        Some(RuntimeResult::ResourceTargetsApplied { applied }) => {
            let value = serde_json::to_value(applied).unwrap_or_default();
            out(
                tag,
                format!(
                    "{label}: state={:?} replayed={} conditions={}",
                    receipt.state(),
                    value["replayed"],
                    value["conditions"]
                ),
            );
        }
        _ => out(tag, format!("{label}: {}", receipt_text(&receipt))),
    }
}

fn now(bench: &Bench) -> u64 {
    bench.clock.unix_ms.load(Ordering::SeqCst)
}

fn evaluate(
    tag: &str,
    label: &str,
    bench: &Bench,
    resources: &EvaluationResources,
    seed: u64,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let at = now(bench);
    match bench.host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        resources,
        EvaluationTime {
            unix_ms: at,
            monotonic_ms: at,
        },
        seed,
        PolicyTrigger::FactsChanged,
    ) {
        Ok(cycle) => {
            let Some(evaluation) = cycle.evaluation else {
                out(tag, format!("{label}: no evaluation ({:?})", cycle.directive));
                return None;
            };
            for decision in evaluation
                .decisions
                .iter()
                .filter(|decision| decision.task_id == "fixture.observe")
            {
                out(tag, format!("{label}: decision state={:?}", decision.state));
                for reason in decision.reasons.iter().filter(|reason| {
                    reason.code.starts_with("resource_target")
                        || reason.code == "resource_weights"
                        || reason.code == "scored"
                }) {
                    out(tag, format!("{label}: {} {}", reason.code, reason.detail));
                }
            }
            let intent = evaluation
                .dispatch_intents
                .iter()
                .find(|intent| intent.task_id == "fixture.observe")
                .cloned();
            out(tag, format!("{label}: dispatch_intent={}", intent.is_some()));
            intent.map(|intent| {
                let chain = evaluation
                    .reason_chains
                    .iter()
                    .find(|chain| chain.id == intent.reason_chain_id)
                    .expect("reason chain")
                    .clone();
                (intent, chain)
            })
        }
        Err(error) => {
            out(tag, format!("{label}: evaluation error code={}", error.code()));
            None
        }
    }
}

/// The geometry frame of the run (frame id text, capture time in Unix ms).
fn geometry_frames(events: &[PersistedEvent]) -> Vec<(String, u64)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::GeometryObserved { observation } => {
                    observation.frame.as_ref().map(|frame| {
                        (
                            serde_json::to_value(frame.frame_id)
                                .ok()
                                .and_then(|value| value.as_str().map(str::to_owned))
                                .unwrap_or_default(),
                            u64::try_from(
                                frame
                                    .captured_at
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .expect("capture time")
                                    .as_millis(),
                            )
                            .expect("capture ms"),
                        )
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn check(tag: &str, name: &str, ok: bool) {
    out(tag, format!("CHECK {name} ok={ok}"));
}

/// Prints each record published during the run with its event origin and links, and checks the
/// T2 fields.
fn report_facts(tag: &str, label: &str, run: &ManualRun, package: &[u8], task_label: &str) {
    // The ledger keeps a capture time only for the initial geometry frame; the run's last
    // `capture.completed` names its terminal frame.
    let frames = geometry_frames(&run.events);
    let captures = run
        .events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::CaptureCompleted
                && event.links().correlation_id() == Some(&run.correlation_id)
        })
        .filter_map(|event| event.links().frame_id())
        .map(|frame| {
            serde_json::to_value(frame)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    out(
        tag,
        format!("{label}: geometry frames {frames:?} captured frames {captures:?}"),
    );
    let bundle = format!("{:x}", Sha256::digest(package));
    for (event, record) in published_records(&run.events) {
        out(
            tag,
            format!(
                "{label}: record {}",
                serde_json::to_string(record).unwrap_or_default()
            ),
        );
        out(
            tag,
            format!(
                "{label}: event origin source={:?} actor={:?} module={:?} links={} severity={:?}",
                event.origin().source(),
                event.origin().actor(),
                event.origin().module(),
                serde_json::to_string(event.links()).unwrap_or_default(),
                event.severity()
            ),
        );
        let reading_id = record
            .source_snapshot_id
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        let frame_part = record
            .source_snapshot_id
            .split('/')
            .nth(1)
            .unwrap_or_default()
            .trim_start_matches("frame:")
            .to_owned();
        if captures.len() == 1 {
            check(
                tag,
                &format!(
                    "{label} {reading_id} observed_at==the single (terminal) frame's captured_at"
                ),
                frames.first().is_some_and(|(id, ms)| {
                    *ms == record.observed_at_unix_ms && Some(id) == captures.last()
                }),
            );
        } else {
            out(
                tag,
                format!(
                    "{label} {reading_id}: {} frames; observed_at={} initial frame captured_at={:?} (later capture times are not in the ledger)",
                    captures.len(),
                    record.observed_at_unix_ms,
                    frames.first().map(|(_, ms)| *ms)
                ),
            );
        }
        check(
            tag,
            &format!("{label} {reading_id} snapshot frame==last captured (terminal) frame id"),
            captures.last().is_some_and(|id| *id == frame_part),
        );
        check(
            tag,
            &format!("{label} {reading_id} expires==observed+21600000"),
            record.expires_at_unix_ms == Some(record.observed_at_unix_ms + 21_600_000),
        );
        check(
            tag,
            &format!("{label} {reading_id} ttl detector_contract 21600000"),
            record.ttl_policy
                == Some(FactTtlPolicy {
                    minimum_ms: 21_600_000,
                    maximum_ms: 21_600_000,
                    source: FactTtlSource::DetectorContract,
                }),
        );
        check(
            tag,
            &format!("{label} {reading_id} scope instance alias"),
            record.scope
                == FactScope::Instance {
                    instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                },
        );
        check(
            tag,
            &format!("{label} {reading_id} source_detector"),
            record.source_detector == format!("resource_reading:{task_label}/{reading_id}"),
        );
        check(
            tag,
            &format!("{label} {reading_id} snapshot run:<run>/frame:<frame>/<id>"),
            record.source_snapshot_id.starts_with("run:run_")
                && record.source_snapshot_id.contains("/frame:frame_"),
        );
        check(
            tag,
            &format!("{label} {reading_id} bundle hash == package sha256"),
            record.resource_bundle_hash == bundle,
        );
        check(
            tag,
            &format!("{label} {reading_id} confidence 970, fact.v1, invalidate_on []"),
            record.confidence_milli == 970
                && record.schema_version == "fact.v1"
                && record.invalidate_on.is_empty(),
        );
        check(
            tag,
            &format!("{label} {reading_id} origin runtime/runtime/fact-store, no correlation"),
            event.origin().source() == EventSource::Runtime
                && event.origin().actor() == EventActor::Runtime
                && event.origin().module() == OriginModule::FactStore
                && event.links().correlation_id().is_none(),
        );
    }
}

fn context() -> InstanceFactContext {
    InstanceFactContext {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        server_id: "fixture-server-a".to_owned(),
        game_id: "fixture-game-a".to_owned(),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create copy directory");
    for entry in fs::read_dir(from).expect("read directory") {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

#[test]
fn one_off_335s5b_head_h1_h2_h3_t3_h7() {
    let single = [reading("primary", "resource.primary", "home", "ocr/primary")];
    let double = [
        reading("primary", "resource.primary", "home", "ocr/primary"),
        reading("secondary", "inventory.secondary", "home", "ocr/secondary"),
    ];
    let single_package = observation_package(&single);
    let double_package = observation_package(&double);
    let claim_with_reading = claim_package(&[reading(
        "primary",
        "resource.primary",
        "done",
        "ocr/primary",
    )]);
    let primary_package = single_package.clone();
    let bench = bench(move |config| {
        config.with_procedure_manifest(procedure_manifest_with_primary(
            &primary_package,
            vec!["after_observation".to_owned()],
        ))
    });
    bench
        .host
        .activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate catalog");
    let valid_until = now(&bench) + 86_400_000;
    let document = target_document(valid_until);

    apply_targets("H2", "before any reading", &bench, &document);
    evaluate("H2", "round before reading", &bench, &policy_resources(), 7);

    let request = write_package(&bench, "observation-double", &double_package);
    let ocr_before = bench.vision.calls.load(Ordering::Acquire);
    let run = run_manual(&bench, request);
    report_run("H1", "two readings", &run);
    out(
        "H1",
        format!(
            "two readings: ocr_calls={} captures={} inputs={}",
            bench.vision.calls.load(Ordering::Acquire) - ocr_before,
            bench.state.capture_count.load(Ordering::Acquire),
            bench.state.input_count.load(Ordering::Acquire)
        ),
    );
    report_facts("H1", "two readings", &run, &double_package, "task");
    out(
        "H3",
        format!(
            "two readings: fact.published events={} distinct snapshots={}",
            run.events
                .iter()
                .filter(|event| event.event_type() == EventType::FactPublished)
                .count(),
            published_records(&run.events)
                .iter()
                .map(|(_, record)| record.source_snapshot_id.clone())
                .collect::<BTreeSet<_>>()
                .len()
        ),
    );

    apply_targets("H2", "after the reading (same document)", &bench, &document);
    bench.clock.advance(1_000);
    evaluate("H2", "round after reading", &bench, &policy_resources(), 8);

    let request = write_package(&bench, "observation-single", &single_package);
    let run = run_manual(&bench, request);
    report_run("H3", "one reading after two", &run);
    for (_, record) in published_records(&run.events) {
        out(
            "H3",
            format!(
                "one reading after two: record key={} content={} observed_at={}",
                record.key,
                serde_json::to_string(&record.content).unwrap_or_default(),
                record.observed_at_unix_ms
            ),
        );
    }

    bench.clock.advance(1_000);
    match evaluate("T2", "policy round to dispatch", &bench, &policy_resources(), 9) {
        Some((intent, chain)) => {
            record_policy_approval(&bench.host, &intent);
            match bench
                .host
                .admit_policy_dispatch(&intent, &chain, &policy_context(&bench.host, &intent))
            {
                Ok(PolicyDispatchAdmission::Granted { context }) => {
                    let request = write_package(&bench, "observation-single", &single_package);
                    let before = latest_sequence(&bench.host);
                    let receipt = bench.host.run_scheduled_contained_task(&context, &request);
                    let events = events_after(&bench.host, before);
                    match &receipt {
                        Ok(receipt) => {
                            out(
                                "T2",
                                format!("policy dispatch: receipt {}", receipt_text(receipt)),
                            );
                            match bench.host.complete_scheduled_policy_run(&context, receipt) {
                                Ok(_) => out("T2", "policy dispatch: completion recorded"),
                                Err(error) => out(
                                    "T2",
                                    format!("policy dispatch: completion code={}", error.code()),
                                ),
                            }
                        }
                        Err(error) => {
                            out("T2", format!("policy dispatch: error code={}", error.code()))
                        }
                    }
                    for (event, record) in published_records(&events) {
                        out(
                            "T2",
                            format!(
                                "policy dispatch: record key={} content={} snapshot={} links={}",
                                record.key,
                                serde_json::to_string(&record.content).unwrap_or_default(),
                                record.source_snapshot_id,
                                serde_json::to_string(event.links()).unwrap_or_default()
                            ),
                        );
                    }
                    let sequence = events
                        .iter()
                        .filter(|event| {
                            matches!(
                                event.event_type(),
                                EventType::TaskTerminalIntent
                                    | EventType::FactPublished
                                    | EventType::CaptureSummaryCommitted
                                    | EventType::TaskCompleted
                                    | EventType::TaskFailed
                            )
                        })
                        .map(|event| format!("{:?}", event.event_type()))
                        .collect::<Vec<_>>();
                    out("T2", format!("policy dispatch: sequence {}", sequence.join(",")));
                }
                Ok(_) => out("T2", "policy dispatch: admission not granted"),
                Err(error) => out("T2", format!("policy dispatch: admission code={}", error.code())),
            }
        }
        None => out("T2", "policy dispatch: no intent"),
    }

    bench
        .state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let request = write_package(&bench, "claim-reading", &claim_with_reading);
    let run = run_manual(&bench, request);
    report_run("H1", "claim then reading on done", &run);
    report_facts("H1", "claim then reading on done", &run, &claim_with_reading, "task");

    let latest_observed = bench
        .host
        .instance_fact_snapshot(context())
        .expect("instance facts")
        .records
        .iter()
        .filter(|record| record.key == "resource.primary")
        .map(|record| record.observed_at_unix_ms)
        .max()
        .expect("a primary reading");
    bench
        .clock
        .set_unix_ms(latest_observed + 21_600_000 + 1_000);
    apply_targets("T3", "after valid_for_ms (same document)", &bench, &document);
    evaluate("T3", "round after valid_for_ms", &bench, &policy_resources(), 10);

    let snapshot = bench
        .host
        .instance_fact_snapshot(context())
        .expect("instance facts");
    for record in &snapshot.records {
        out(
            "H7",
            format!(
                "record {}",
                serde_json::to_string(record).unwrap_or_default()
            ),
        );
    }
    let latest = latest_sequence(&bench.host);
    let clock_at = now(&bench);
    out(
        "H1",
        format!(
            "fatal={:?}",
            bench
                .host
                .fatal_error()
                .expect("health")
                .map(|error| error.code())
        ),
    );
    let Bench {
        root,
        instance,
        host,
        ..
    } = bench;
    host.close().expect("close host");
    if let Ok(target) = std::env::var("ONE_OFF_335S5B_STATE") {
        let target = PathBuf::from(target);
        copy_tree(root.path(), &target.join("state"));
        fs::write(
            target.join("instance_id.json"),
            serde_json::to_vec(&instance).expect("instance id JSON"),
        )
        .expect("write instance id");
        fs::write(target.join("latest_sequence.txt"), latest.to_string())
            .expect("write sequence");
        fs::write(target.join("clock_unix_ms.txt"), clock_at.to_string()).expect("write clock");
        out("H7", format!("state copied latest_sequence={latest}"));
    }
}

fn manual_record(observed_at_unix_ms: u64) -> FactRecord {
    FactRecord {
        scope: FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        key: "resource.primary".to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(1),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: Some(observed_at_unix_ms + 600_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 600_000,
            maximum_ms: 600_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 1_000,
        source_detector: "agent.manual".to_owned(),
        source_snapshot_id: "snapshot:manual-newer".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

/// The host's own run entry (the one-off helper): the failure's code and native detail.
fn run_direct(tag: &str, label: &str, bench: &Bench, request: ContainedTaskRequest) {
    let client = TestClient::connect(&bench.host);
    let message = client.request(RuntimeOperation::run_contained_task(
        POLICY_INSTANCE_ALIAS,
        client.ids.mint_holder_id().expect("holder"),
        request,
    ));
    match bench.host.one_off_335s5b_run_contained_task(&message) {
        Ok(state) => out(tag, format!("{label}: direct run state={state}")),
        Err((code, detail)) => out(
            tag,
            format!("{label}: direct run failure code={code} native_detail={detail:?}"),
        ),
    }
}

fn count_published(events: &[PersistedEvent]) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == EventType::FactPublished)
        .count()
}

#[test]
fn one_off_335s5b_head_h4_newer_observation() {
    let bench = bench(|config| config);
    let manual = manual_record(now(&bench));
    bench.host.publish_fact(manual).expect("manual newer record");
    let package = observation_package(&[reading(
        "primary",
        "resource.primary",
        "home",
        "ocr/primary",
    )]);
    let run = run_manual(&bench, write_package(&bench, "h4", &package));
    report_run("H4", "reading older than the manual record", &run);
    let before = latest_sequence(&bench.host);
    run_direct(
        "H4",
        "reading older than the manual record",
        &bench,
        write_package(&bench, "h4", &package),
    );
    out(
        "H4",
        format!(
            "direct run fact.published={}",
            count_published(&events_after(&bench.host, before))
        ),
    );
    let active = bench
        .host
        .instance_fact_snapshot(context())
        .expect("instance facts")
        .records
        .into_iter()
        .filter(|record| record.key == "resource.primary")
        .map(|record| record.source_snapshot_id)
        .collect::<Vec<_>>();
    out("H4", format!("active resource.primary snapshot={active:?}"));
    bench.host.close().expect("close host");
}

#[test]
fn one_off_335s5b_head_h5_live_pool() {
    let without_pools = EvaluationResources {
        pools: Vec::new(),
        hosts: policy_resources().hosts,
    };
    let inputs = without_pools.clone();
    let bench = bench(move |config| {
        config.with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), inputs))
    });
    let mut sources = budget_policy_sources(1);
    let mut pools: Value = serde_json::from_slice(&sources.pools.bytes).expect("pools");
    pools["pools"][0]["value_source"] =
        json!({"kind": "ledger_fact", "minimum_confidence_milli": 900});
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("pools bytes");
    match bench.host.activate_policy_catalog(&sources) {
        Ok(_) => out("H5", "catalog with a ledger_fact pool on resource.primary active"),
        Err(error) => out("H5", format!("catalog activation code={}", error.code())),
    }
    let package = observation_package(&[reading(
        "primary",
        "resource.primary",
        "home",
        "ocr/primary",
    )]);
    let run = run_manual(&bench, write_package(&bench, "h5", &package));
    report_run("H5", "reading of a live pool key", &run);
    let before = latest_sequence(&bench.host);
    run_direct(
        "H5",
        "reading of a live pool key",
        &bench,
        write_package(&bench, "h5", &package),
    );
    out(
        "H5",
        format!(
            "direct run fact.published={}",
            count_published(&events_after(&bench.host, before))
        ),
    );
    bench.clock.advance(1_000);
    evaluate("H5", "policy round after the refusal", &bench, &without_pools, 11);
    bench.host.close().expect("close host");
}

#[test]
fn one_off_335s5b_head_h6_page_confirmation_failed() {
    let bench = bench(|config| config);
    let package = claim_package(&[reading("primary", "resource.primary", "done", "ocr/primary")]);
    // The tap does not change the screen: the claim's page confirmation fails.
    let run = run_manual(&bench, write_package(&bench, "h6", &package));
    report_run("H6", "claim whose page confirmation fails", &run);
    out(
        "H6",
        format!(
            "inputs={} fatal={:?}",
            bench.state.input_count.load(Ordering::Acquire),
            bench
                .host
                .fatal_error()
                .expect("health")
                .map(|error| error.code())
        ),
    );
    bench.host.close().expect("close host");
}

#[test]
fn one_off_335s5b_head_h9_startup_package() {
    let bench = bench(|config| config);
    let package = observation_package(&[reading(
        "primary",
        "resource.primary",
        "home",
        "ocr/primary",
    )]);
    let before = latest_sequence(&bench.host);
    let captures = bench.state.capture_count.load(Ordering::Acquire);
    let result = bench
        .host
        .one_off_335s5b_run_startup_package(
            POLICY_INSTANCE_ALIAS,
            write_package(&bench, "h9", &package),
        )
        .expect("startup package run");
    out("H9", format!("startup package with readings: result={result:?}"));
    let events = events_after(&bench.host, before);
    out(
        "H9",
        format!(
            "inputs={} captures={} fact.published={} events={}",
            bench.state.input_count.load(Ordering::Acquire),
            bench.state.capture_count.load(Ordering::Acquire) - captures,
            count_published(&events),
            events
                .iter()
                .map(|event| format!("{:?}", event.event_type()))
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    for event in events
        .iter()
        .filter(|event| event.event_type() == EventType::RuntimeFailed)
    {
        let text = serde_json::to_string(event.payload()).unwrap_or_default();
        let start = text.find("host_code=").unwrap_or(0);
        let end = (start + 120).min(text.len());
        out(
            "H9",
            format!("runtime.failed {}", text.get(start..end).unwrap_or("<cut>")),
        );
    }
    bench.host.close().expect("close host");
}

#[test]
fn one_off_335s5b_head_t4_details() {
    let bench = bench(|config| config);
    let package = observation_package(&[reading(
        "primary",
        "resource.primary",
        "home",
        "ocr/primary",
    )]);
    bench.vision.set_confidence(0.85);
    let run = run_manual(&bench, write_package(&bench, "t4-low", &package));
    report_run("T4", "confidence 0.85 below 900", &run);
    run_direct(
        "T4",
        "confidence 0.85 below 900",
        &bench,
        write_package(&bench, "t4-low", &package),
    );
    bench.vision.set_confidence(0.97);

    let mut pages = claim_pages();
    pages[1]["optional"] = json!(["ocr/primary"]);
    let gated = claim_package_with_pages(
        &[reading("primary", "resource.primary", "done", "ocr/primary")],
        &pages,
    );
    let inputs = bench.state.input_count.load(Ordering::Acquire);
    let run = run_manual(&bench, write_package(&bench, "t4-gated", &gated));
    report_run("T4", "reading target in a page gate", &run);
    run_direct(
        "T4",
        "reading target in a page gate",
        &bench,
        write_package(&bench, "t4-gated", &gated),
    );
    out(
        "T4",
        format!(
            "reading target in a page gate: inputs={} fatal={:?}",
            bench.state.input_count.load(Ordering::Acquire) - inputs,
            bench
                .host
                .fatal_error()
                .expect("health")
                .map(|error| error.code())
        ),
    );
    bench.host.close().expect("close host");
}
