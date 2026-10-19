// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #372 K5 (K372 addendum of 2026-10-17, K5.6 rows K5-T1 to K5-T6 and K5-T8; the
// coordinator's rulings of 2026-10-17, Q1 and Q6): under the active catalog a task accounts as
// itself and the ids its `continues_from` declares. Budgets are summed over that group; the
// slot, outcome and failure state come from its latest run; the clock-slot records of a
// continued id count for the task. A catalog switch is a host restart onto a second generation,
// as `initialize_policy` does it. The scenarios run generic tasks on the fixture instance of
// `gate_372k2` (activity window 06:00-23:30 UTC, UTC clocks, a manual Runtime clock), except the
// outcome row, which runs the mapped producer of `mapped_runs`. Every expected value comes from
// the frozen model row.

use super::clock_slot_report::{ledger_events, record_sequence, report};
use super::gate_372k2::{SlotTask, Slots, at, slot_sources};
use super::*;
use crate::codes::HostCode;
use crate::policy_host::clock_slot::{SlotRecord, signal_id};
use actingcommand_contract::PolicyAdmissionRecord;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_policy::{EligibilityState, PolicyCode, SchedulingDecisionState, TaskDecision};
use serde_json::{Value, json};

const TASK_A: &str = "fixture.a";
const TASK_B: &str = "fixture.b";
const TASK_C: &str = "fixture.c";
const TASK_E: &str = "fixture.e";
const TASK_F: &str = "fixture.f";
// The second generation's approval reference: an approval id names one catalog generation.
const SECOND_APPROVAL: &str = "approval:fixture-k5";
// The grace of the one `missed` rule of these scenarios: two hours.
const GRACE_MS: u64 = 7_200_000;

// #411 A24 (register §1.139): `hour:minute` UTC on scenario day `day`, 1 364 whole weeks after
// `at`'s (day 0 is 2050-01-05, a Wednesday as 2023-11-15 is). A host restart writes the ledger's
// own prior-epoch close records, stamped with the machine clock, not the manual Runtime clock,
// and `actingd suspended` judges its clock slots against the ledger's latest time, its maximum
// timestamp (`contracts/ledger-observer.md`). A scenario that restarts and reads that report
// therefore runs after the machine clock, as a Runtime's own clock does, and before the fixture
// catalog's 2100 horizon.
const fn after_the_machine_clock(day: u64, hour: u64, minute: u64) -> u64 {
    at(day, hour, minute) + 1_364 * 7 * 24 * 3_600_000
}

// A UTC `daily` clock at `minutes`, with `missed: skip_after` of `grace_ms` when given.
fn daily(minutes: &[u16], grace_ms: Option<u64>) -> Value {
    let mut clock = json!({
        "kind": "clock",
        "schedule": {
            "kind": "daily",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "minutes_of_day": minutes
        }
    });
    if let Some(grace_ms) = grace_ms {
        clock["missed"] = json!({"kind": "skip_after", "grace_ms": grace_ms});
    }
    clock
}

// A trigger with no clock that is always true: the fixture pool's projection never exceeds its
// capacity of 120.
fn unclocked() -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": "fixture-pool-a",
        "comparison": "less_than",
        "value": 1000
    })
}

// A trigger that waits for `task_id` to have ended in `state`.
fn after(task_id: &str, state: &str) -> Value {
    json!({
        "kind": "dependency_completed",
        "task_id": task_id,
        "terminal_states": [state]
    })
}

fn task(id: &'static str, trigger: Value, daily_limit: u32) -> SlotTask {
    SlotTask {
        id,
        priority: 100,
        trigger,
        daily_limit,
    }
}

// Generation `version`: the `gate_372k2` catalog of `tasks`, declared as
// `actingcommand.scheduling.v2` (each timeline event with its explicit unbounded validity), with
// each `(task, member, value)` of `members` set on that task. A generation after the first names
// its own approval reference.
fn generation(version: u64, tasks: &[SlotTask], members: &[(&str, &str, Value)]) -> CatalogSources {
    let mut sources = slot_sources(version, tasks, None);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value = serde_json::from_slice(&source.bytes).expect("fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if version > 1 {
            document["catalog"]["approval_refs"] = json!([SECOND_APPROVAL]);
        }
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if let Some(specs) = document.get_mut("tasks").and_then(Value::as_array_mut) {
            for spec in specs {
                for (task_id, member, value) in members {
                    if spec["id"] == *task_id {
                        spec[*member] = value.clone();
                    }
                }
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("V2 fixture bytes");
    }
    sources
}

// A clean stop, a new start on the same state root at `unix_ms`, and the activation of `sources`
// there: a catalog switch.
fn switch(slots: Slots, unix_ms: u64, sources: &CatalogSources) -> Slots {
    let slots = slots.restart(unix_ms);
    slots
        .host
        .activate_policy_catalog(sources)
        .expect("activate the second generation");
    slots
}

// A person approves the catalog generation `intent` was evaluated under.
fn approve(host: &RuntimeHost, intent: &DispatchIntent) {
    let mut client = TestClient::connect(host);
    client.declare_governance_identity();
    for approval_id in &intent.approval_refs {
        let decision = ApprovalDecisionRecord::new(
            approval_id.as_str(),
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: intent.catalog_hash.clone(),
                catalog_version: intent.catalog_version,
            },
            actingcommand_contract::ApprovalReasonCode::new("user_confirmed")
                .expect("approval reason"),
        )
        .expect("approval decision");
        let request =
            client.governance_request(RuntimeOperation::RecordApprovalDecision { decision });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }
}

// Evaluates at `unix_ms`, admits the dispatch the cycle selected for `task_id` and records its
// run: a success, or a failure of `failed`. Returns the run's admission.
fn run(
    slots: &mut Slots,
    unix_ms: u64,
    task_id: &str,
    failed: Option<PolicyFailureClass>,
) -> PolicyAdmissionRecord {
    let cycle = slots.evaluate(unix_ms);
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| panic!("{task_id} is selected at {unix_ms}: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("dispatch reason chain")
        .clone();
    approve(&slots.host, &intent);
    let PolicyDispatchAdmission::Granted { context } = slots
        .host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&slots.host, &intent))
        .unwrap_or_else(|error| panic!("admit {task_id} at {unix_ms}: {error}"))
    else {
        panic!("{task_id} is granted at {unix_ms}");
    };
    let input = match failed {
        None => PolicyExecutionInput::Succeeded,
        Some(class) => PolicyExecutionInput::Failed {
            outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
            failure_identity: None,
            class,
        },
    };
    slots
        .host
        .record_policy_dispatch_outcome(&intent.decision_id, &input)
        .expect("record the run");
    slots
        .host
        .release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    context.admission().clone()
}

// The cycle's decision for `task_id` on the fixture instance.
fn decision<'a>(cycle: &'a PolicyCycle, task_id: &str) -> &'a TaskDecision {
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .unwrap_or_else(|| panic!("{task_id} has a decision: {evaluation:#?}"))
}

// Whether the cycle selected `task_id`.
fn selects(cycle: &PolicyCycle, task_id: &str) -> bool {
    cycle
        .pending_dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id)
}

// Whether the decision was deferred with a reason of `code` whose detail contains `detail`.
fn deferred_by(decision: &TaskDecision, code: impl Into<Code> + Copy, detail: &str) -> bool {
    decision.state == SchedulingDecisionState::Deferred
        && decision
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(code) && reason.detail.contains(detail))
}

// The budget denial of the task daily count with the group's `used` count of a limit of 2, as
// the deferral's detail prints it.
fn task_daily_spent(used: u32) -> String {
    format!("dimension: TaskDaily, used: {used}, requested: 1, limit: 2")
}

// The `clock_slots` entry of `signal_id` in an `actingd suspended` report.
fn slot_entry(report: &Value, events: &[PersistedEvent], signal_id: &str) -> Value {
    let sequence = record_sequence(events, signal_id);
    report["clock_slots"]
        .as_array()
        .expect("clock_slots")
        .iter()
        .find(|entry| entry["recorded_sequence"] == sequence)
        .cloned()
        .unwrap_or_else(|| panic!("an entry for {signal_id}: {report:#}"))
}

// `next_dispatch` naming the run admitted at `unix_ms`: its admission's sequence and time.
fn admitted_at(events: &[PersistedEvent], unix_ms: u64) -> Value {
    let found = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::PolicyDispatchAdmitted
                && event.timestamp_unix_ms() == unix_ms
        })
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "one admission at {unix_ms}: {found:?}");
    json!({"sequence": found[0], "admitted_at_unix_ms": unix_ms})
}

// K5-T1, a rename mid-day: generation 1 runs `fixture.a` (`daily [06:30]`, limit 1) at 06:31.
// At 12:00 a restart activates generation 2, where `fixture.b` with the same clock and limit
// continues `fixture.a`: `fixture.b` is not due at 12:01 or at any cycle through 23:30, as 06:31
// served its slot. Without the declaration `fixture.b` is a new task and is dispatched at 12:01,
// the day's second run.
#[test]
fn k5_t1_a_task_renamed_mid_day_is_not_due_again_that_day() {
    for declared in [true, false] {
        let first = generation(1, &[task(TASK_A, daily(&[390], None), 1)], &[]);
        let mut slots = Slots::start(&first, at(1, 6, 0));
        run(&mut slots, at(1, 6, 31), TASK_A, None);
        let members = if declared {
            vec![(TASK_B, "continues_from", json!([TASK_A]))]
        } else {
            Vec::new()
        };
        let second = generation(2, &[task(TASK_B, daily(&[390], None), 1)], &members);
        let mut slots = switch(slots, at(1, 12, 0), &second);
        if declared {
            for unix_ms in [
                at(1, 12, 1),
                at(1, 15, 0),
                at(1, 18, 0),
                at(1, 21, 0),
                at(1, 23, 29),
            ] {
                let cycle = slots.evaluate(unix_ms);
                assert!(
                    !selects(&cycle, TASK_B),
                    "the renamed task is not dispatched again at {unix_ms}"
                );
                assert_eq!(
                    decision(&cycle, TASK_B).eligibility,
                    EligibilityState::False,
                    "its slot was served at 06:31, so it is not due at {unix_ms}"
                );
            }
        } else {
            let admission = run(&mut slots, at(1, 12, 1), TASK_B, None);
            assert_eq!(
                admission.budget.task_daily_used, 1,
                "without the declaration the new id counts from zero"
            );
        }
        slots.close();
    }
}

// K5-T2, budget, receipt and replay: a trigger with no clock and `daily_limit` 2. `fixture.a`
// ran once under generation 1; under generation 2 `fixture.b`, which continues it, is admitted
// with `task_daily_used` 2, and the next candidate is refused `policy_budget_exhausted` on
// `task_daily`. A restart replays both admissions without `policy_budget_receipt_mismatch`, and
// as the own entries are a = 1 and b = 1 the lookup after it is 2, not 3.
#[test]
fn k5_t2_the_receipt_counts_the_group_and_replays_without_a_mismatch() {
    let first = generation(1, &[task(TASK_A, unclocked(), 2)], &[]);
    let mut slots = Slots::start(&first, at(1, 6, 0));
    let admission = run(&mut slots, at(1, 6, 10), TASK_A, None);
    assert_eq!(admission.budget.task_daily_used, 1);
    let second = generation(
        2,
        &[task(TASK_B, unclocked(), 2)],
        &[(TASK_B, "continues_from", json!([TASK_A]))],
    );
    let mut slots = switch(slots, at(1, 7, 0), &second);
    let admission = run(&mut slots, at(1, 7, 1), TASK_B, None);
    assert_eq!(
        admission.budget.task_daily_used, 2,
        "the receipt counts fixture.a's run and fixture.b's"
    );
    let cycle = slots.evaluate(at(1, 7, 2));
    assert!(!selects(&cycle, TASK_B));
    assert!(
        deferred_by(
            decision(&cycle, TASK_B),
            HostCode::PolicyBudgetExhausted,
            &task_daily_spent(2)
        ),
        "{:#?}",
        decision(&cycle, TASK_B)
    );

    let mut slots = slots.restart(at(1, 7, 3));
    assert!(
        slots
            .host
            .fatal_error()
            .expect("read the fatal state")
            .is_none(),
        "the replay of both admissions matches their receipts"
    );
    let cycle = slots.evaluate(at(1, 7, 4));
    assert!(
        deferred_by(
            decision(&cycle, TASK_B),
            HostCode::PolicyBudgetExhausted,
            &task_daily_spent(2)
        ),
        "after the restart the group still counts 2: {:#?}",
        decision(&cycle, TASK_B)
    );
    slots.close();
}

// K5-T3, a follower of a renamed head: `fixture.a` (`daily [06:30]`) succeeded at 06:31 under
// generation 1. Under generation 2, `fixture.f` waits for `fixture.b`, which continues
// `fixture.a`, to have succeeded: `fixture.f` is due at 12:01 without the head running again.
// Without the declaration `fixture.f` stays `trigger_unknown` (`dependency_state_missing`).
#[test]
fn k5_t3_a_follower_of_a_renamed_head_reads_the_head_s_terminal_state() {
    for declared in [true, false] {
        let first = generation(1, &[task(TASK_A, daily(&[390], None), 1)], &[]);
        let mut slots = Slots::start(&first, at(1, 6, 0));
        run(&mut slots, at(1, 6, 31), TASK_A, None);
        let members = if declared {
            vec![(TASK_B, "continues_from", json!([TASK_A]))]
        } else {
            Vec::new()
        };
        let second = generation(
            2,
            &[
                task(TASK_B, daily(&[390], None), 1),
                task(TASK_F, after(TASK_B, "succeeded"), 1),
            ],
            &members,
        );
        let mut slots = switch(slots, at(1, 12, 0), &second);
        let cycle = slots.evaluate(at(1, 12, 1));
        let follower = decision(&cycle, TASK_F);
        if declared {
            assert_eq!(
                follower.eligibility,
                EligibilityState::True,
                "{follower:#?}"
            );
            assert!(selects(&cycle, TASK_F), "the follower is dispatched");
            assert!(!selects(&cycle, TASK_B), "the head does not run again");
        } else {
            assert!(
                follower
                    .reasons
                    .iter()
                    .any(|reason| reason.outcome.is(PolicyCode::TriggerUnknown)),
                "{follower:#?}"
            );
            assert!(
                follower.detection_suggestions.iter().any(|suggestion| {
                    suggestion.fact_key == format!("task.{TASK_B}.terminal_state")
                        && suggestion.reason == "dependency_state_missing"
                }),
                "{follower:#?}"
            );
            assert!(!selects(&cycle, TASK_F));
        }
        slots.close();
    }
}

// The mapped producer's outcome key.
const PRODUCED: &str = "k5-produced";
// The id generation 2 gives the mapped producer `fixture.observe`.
const RENAMED: &str = "fixture.renamed";

// A host whose producer is bound to `package`, at the manual clock's time `facts_unix_ms`.
fn start_mapped_host(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    package: &[u8],
    state: &Arc<FakeState>,
    registered: InstanceId,
    facts_unix_ms: u64,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(PolicyInputSnapshot::new(
                mapped_policy_facts_at(PRODUCED, facts_unix_ms, false),
                policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, Arc::clone(state))
                .fixture_simulation(),
        ),
    )
    .expect("mapped producer host")
}

// Generation 2 of the mapped catalog: declared as V2 with its own approval reference, the
// producer renamed `fixture.renamed` (continuing `fixture.observe` when `declared`), and the
// consumer reading the renamed producer's outcome.
fn renamed_producer_sources(declared: bool) -> CatalogSources {
    let mut sources = mapped_policy_sources_with_keys(2, &[PRODUCED]);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let text = String::from_utf8(source.bytes.clone()).expect("fixture UTF-8");
        let text = text.replace("\"fixture.observe\"", &format!("\"{RENAMED}\""));
        let mut document: Value = serde_json::from_str(&text).expect("fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        document["catalog"]["approval_refs"] = json!([SECOND_APPROVAL]);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if declared && let Some(specs) = document.get_mut("tasks") {
            specs[0]["continues_from"] = json!(["fixture.observe"]);
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("V2 fixture bytes");
    }
    sources
}

// K5-T3, the outcome half: the mapped producer `fixture.observe` succeeds with its outcome under
// generation 1, whose consumer reads it. After a restart, generation 2 renames the producer
// `fixture.renamed`, which continues it, and its consumer reads `outcome(fixture.renamed, key)`:
// the consumer sees the earlier run's outcome and is due. Without the declaration the outcome is
// unknown (`outcome_missing`).
#[test]
fn k5_t3_a_renamed_producer_s_outcome_is_read_under_the_task_that_continues_it() {
    for declared in [true, false] {
        let root = TempDir::new().expect("tempdir");
        let package =
            neutral_mapped_contained_task_package(PRODUCED, "designated_effect_completed");
        let package_path = root.path().join("k5-producer.zip");
        fs::write(&package_path, &package).expect("write the producer package");
        let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
            .expect("producer request");
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        let registered = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(
            POLICY_NOW_UNIX_MS,
            POLICY_NOW_UNIX_MS,
        ));
        let host = start_mapped_host(
            &root,
            &clock,
            &package,
            &state,
            registered,
            POLICY_NOW_UNIX_MS,
        );
        host.activate_policy_catalog(&mapped_policy_sources_with_keys(1, &[PRODUCED]))
            .expect("activate generation 1");
        let (context, _) = admit_mapped_run_at(&host, PRODUCED, POLICY_NOW_UNIX_MS, 41_201);
        let receipt = host
            .run_scheduled_contained_task(&context, &request)
            .expect("the producer runs");
        let (_, projection) = host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the producer completes");
        assert_eq!(
            projection
                .as_ref()
                .map(|projection| projection.scheduling_result().disposition().outcome_key()),
            Some(PRODUCED),
            "the producer's run gives its outcome"
        );
        host.close().expect("close generation 1");

        advance_manual_clock_to(&clock, POLICY_RESTART_UNIX_MS);
        let restarted = start_mapped_host(
            &root,
            &clock,
            &package,
            &state,
            registered,
            POLICY_RESTART_UNIX_MS,
        );
        restarted
            .activate_policy_catalog(&renamed_producer_sources(declared))
            .expect("activate generation 2");
        let cycle = restarted
            .evaluate_policy_cycle(PolicyTrigger::Recovery)
            .unwrap_or_else(|error| panic!("evaluate generation 2: {} ({error})", error.code()));
        assert!(
            restarted
                .fatal_error()
                .expect("read the fatal state")
                .is_none()
        );
        let consumer = decision(&cycle, "fixture.followup");
        let unknown = consumer.detection_suggestions.iter().any(|suggestion| {
            suggestion.fact_key == format!("outcome.{RENAMED}.{PRODUCED}")
                && suggestion.reason == "outcome_missing"
        });
        if declared {
            assert!(!unknown, "{consumer:#?}");
            assert_eq!(
                consumer.eligibility,
                EligibilityState::True,
                "{consumer:#?}"
            );
        } else {
            assert!(unknown, "{consumer:#?}");
        }
        restarted.close().expect("close generation 2");
    }
}

// K5-T4, K2, K4 and the view. Generation 1's `fixture.a` (`daily [06:30]`, limit 1) runs at
// 06:10 for the evening before and is refused at 06:30 (one `clock_slot_refused`). After a 07:00
// switch to `fixture.b`, which continues it with the same clock, the next refused cycle writes no
// second refused record of 06:30; `fixture.b`'s first selection, the next morning, writes an
// owed record without `first_run`; and `actingd suspended` settles `fixture.a`'s record with
// `fixture.b`'s admission as `next_dispatch`. With `missed` on the continuing clock, a K4 lapse
// of `fixture.b` is recorded, as the pair has its group's history.
#[test]
fn k5_t4_clock_slot_records_of_a_continued_id_count_for_the_task() {
    // #411 A24: it restarts and reads `actingd suspended`.
    let at = after_the_machine_clock;
    let first = generation(1, &[task(TASK_A, daily(&[390], None), 1)], &[]);
    let mut slots = Slots::start(&first, at(0, 23, 40));
    slots.evaluate(at(0, 23, 40));
    run(&mut slots, at(1, 6, 10), TASK_A, None);
    slots.evaluate(at(1, 6, 30));
    let rows = slots.new_rows();
    let refused = rows
        .iter()
        .filter(|row| row.kind == "refused")
        .collect::<Vec<_>>();
    assert_eq!(refused.len(), 1, "{rows:#?}");
    assert_eq!(refused[0].task_id, TASK_A);
    assert_eq!(refused[0].time("occurrence_unix_ms"), Some(at(1, 6, 30)));

    let second = generation(
        2,
        &[task(TASK_B, daily(&[390], None), 1)],
        &[(TASK_B, "continues_from", json!([TASK_A]))],
    );
    let mut slots = switch(slots, at(1, 7, 0), &second);
    let cycle = slots.evaluate(at(1, 7, 1));
    assert!(!selects(&cycle, TASK_B));
    assert!(
        slots.new_rows().is_empty(),
        "fixture.a's refused record of 06:30 stands for fixture.b"
    );
    run(&mut slots, at(2, 6, 5), TASK_B, None);
    let rows = slots.new_rows();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(
        (rows[0].kind.as_str(), rows[0].task_id.as_str()),
        ("owed", TASK_B)
    );
    assert_eq!(rows[0].time("occurrence_unix_ms"), Some(at(1, 6, 30)));
    assert_eq!(
        rows[0].text("first_run"),
        None,
        "fixture.b has its group's history"
    );
    let root = slots.close_keeping_root();
    let events = ledger_events(&root);
    let report = report(&root, &second);
    let entry = slot_entry(
        &report,
        &events,
        &signal_id(
            SlotRecord::Refused,
            TASK_A,
            POLICY_INSTANCE_ALIAS,
            at(1, 6, 30),
        ),
    );
    assert_eq!(
        entry["next_dispatch"],
        admitted_at(&events, at(2, 6, 5)),
        "{entry:#}"
    );
    assert_eq!(entry["slot_fate"], "next_window", "{entry:#}");
    assert_eq!(entry["slot_fate_settled"], true, "{entry:#}");

    // The K4 lapse: the continuing clock skips an occurrence two hours after it.
    let mut slots = Slots::start(&first, at(0, 23, 40));
    slots.evaluate(at(0, 23, 40));
    run(&mut slots, at(1, 6, 10), TASK_A, None);
    slots.evaluate(at(1, 6, 30));
    slots.new_rows();
    let lapsing = generation(
        2,
        &[task(TASK_B, daily(&[390], Some(GRACE_MS)), 1)],
        &[(TASK_B, "continues_from", json!([TASK_A]))],
    );
    let mut slots = switch(slots, at(1, 7, 0), &lapsing);
    slots.evaluate(at(1, 8, 31));
    let rows = slots.new_rows();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(
        (rows[0].kind.as_str(), rows[0].task_id.as_str()),
        ("expired", TASK_B)
    );
    assert_eq!(rows[0].time("occurrence_unix_ms"), Some(at(1, 6, 30)));
    assert_eq!(rows[0].time("grace_ms"), Some(GRACE_MS));
    slots.close();
}

// K5-T5, a merge: `fixture.a` succeeded at 06:10 and `fixture.b` failed at 06:20 under
// generation 1. Under generation 2 `fixture.c` continues both, with `daily_limit` 2 and a cooldown
// of one hour: its last dispatch is 06:20, so it is in its cooldown at 07:15; its terminal state
// is failed, so `fixture.f`, which waits for it to have failed, is due; and after the cooldown it
// is refused on `task_daily`, its group having run twice that day.
#[test]
fn k5_t5_a_merged_task_sums_its_budgets_and_reads_the_later_run() {
    let mut first_a = task(TASK_A, unclocked(), 1);
    first_a.priority = 200;
    let first = generation(1, &[first_a, task(TASK_B, unclocked(), 1)], &[]);
    let mut slots = Slots::start(&first, at(1, 6, 0));
    run(&mut slots, at(1, 6, 10), TASK_A, None);
    run(
        &mut slots,
        at(1, 6, 20),
        TASK_B,
        Some(PolicyFailureClass::Recoverable),
    );
    let mut merged = task(TASK_C, unclocked(), 2);
    merged.priority = 200;
    let second = generation(
        2,
        &[merged, task(TASK_F, after(TASK_C, "failed"), 1)],
        &[
            (TASK_C, "continues_from", json!([TASK_A, TASK_B])),
            (TASK_C, "cooldown_ms", json!(3_600_000)),
        ],
    );
    let mut slots = switch(slots, at(1, 7, 0), &second);
    let cycle = slots.evaluate(at(1, 7, 15));
    let merged = decision(&cycle, TASK_C);
    assert!(
        merged.state == SchedulingDecisionState::Deferred
            && merged
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(PolicyCode::TaskCooldownActive)),
        "the last dispatch is fixture.b's 06:20: {merged:#?}"
    );
    let follower = decision(&cycle, TASK_F);
    assert_eq!(
        follower.eligibility,
        EligibilityState::True,
        "the terminal state is fixture.b's failure: {follower:#?}"
    );
    let cycle = slots.evaluate(at(1, 7, 21));
    assert!(
        deferred_by(
            decision(&cycle, TASK_C),
            HostCode::PolicyBudgetExhausted,
            &task_daily_spent(2)
        ),
        "{:#?}",
        decision(&cycle, TASK_C)
    );
    slots.close();
}

// K5-T6, removed and added: generation 1's `fixture.a` (`daily [06:30]`) is paused by a severe
// failure at 06:10 and its 06:30 slot refused. Generation 2 drops it with no continuation and adds
// `fixture.e` (`daily [05:00]`): `fixture.e` is a first run, its owed record carries `first_run`
// and its counters start at 0. In `actingd suspended`, `fixture.a`'s refused record settles
// `collapsed` once its next occurrence has passed, and the paused pair is listed under
// `task_not_in_catalog`.
#[test]
fn k5_t6_a_removed_task_keeps_its_records_and_an_added_task_starts_fresh() {
    // #411 A24: it restarts and reads `actingd suspended`.
    let at = after_the_machine_clock;
    let first = generation(1, &[task(TASK_A, daily(&[390], None), 2)], &[]);
    let mut slots = Slots::start(&first, at(0, 23, 40));
    slots.evaluate(at(0, 23, 40));
    run(
        &mut slots,
        at(1, 6, 10),
        TASK_A,
        Some(PolicyFailureClass::Severe),
    );
    slots.evaluate(at(1, 6, 30));
    let rows = slots.new_rows();
    assert!(
        rows.iter().any(|row| row.kind == "refused"
            && row.task_id == TASK_A
            && row.text("gate_code") == Some("policy_task_paused")),
        "{rows:#?}"
    );
    let second = generation(2, &[task(TASK_E, daily(&[300], None), 1)], &[]);
    let mut slots = switch(slots, at(1, 7, 0), &second);
    let admission = run(&mut slots, at(1, 7, 1), TASK_E, None);
    assert_eq!(admission.budget.task_daily_used, 1);
    assert_eq!(admission.budget.task_window_used, 1);
    let rows = slots.new_rows();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(
        (rows[0].kind.as_str(), rows[0].task_id.as_str()),
        ("owed", TASK_E)
    );
    assert_eq!(rows[0].text("first_run"), Some("true"));
    run(&mut slots, at(2, 6, 40), TASK_E, None);
    let root = slots.close_keeping_root();
    let events = ledger_events(&root);
    let report = report(&root, &second);
    let entry = slot_entry(
        &report,
        &events,
        &signal_id(
            SlotRecord::Refused,
            TASK_A,
            POLICY_INSTANCE_ALIAS,
            at(1, 6, 30),
        ),
    );
    assert_eq!(entry["slot_fate"], "collapsed", "{entry:#}");
    assert_eq!(entry["slot_fate_settled"], true, "{entry:#}");
    assert!(entry.get("next_dispatch").is_none(), "{entry:#}");
    let pair = format!("task_not_in_catalog:{TASK_A}/{POLICY_INSTANCE_ALIAS}");
    assert!(
        report["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| *warning == pair),
        "{report:#}"
    );
}

// K5-T8, failure state (ruling Q1): generation 1's `fixture.a` is paused by a severe failure.
// Under generation 2 `fixture.b` continues it with the same procedure: `fixture.b` is deferred
// `policy_task_paused`, at every cycle until its package changes. Without the declaration
// `fixture.b` is admitted, today's silent lift.
#[test]
fn k5_t8_a_pause_of_the_continued_id_holds_the_task_that_continues_it() {
    for declared in [true, false] {
        let first = generation(1, &[task(TASK_A, daily(&[390], None), 2)], &[]);
        let mut slots = Slots::start(&first, at(0, 23, 40));
        slots.evaluate(at(0, 23, 40));
        run(
            &mut slots,
            at(1, 6, 10),
            TASK_A,
            Some(PolicyFailureClass::Severe),
        );
        let members = if declared {
            vec![(TASK_B, "continues_from", json!([TASK_A]))]
        } else {
            Vec::new()
        };
        let second = generation(2, &[task(TASK_B, daily(&[390], None), 2)], &members);
        let mut slots = switch(slots, at(1, 7, 0), &second);
        if declared {
            for unix_ms in [at(1, 7, 1), at(1, 12, 0), at(1, 23, 0)] {
                let cycle = slots.evaluate(unix_ms);
                assert!(!selects(&cycle, TASK_B), "paused at {unix_ms}");
                assert!(
                    deferred_by(decision(&cycle, TASK_B), HostCode::PolicyTaskPaused, ""),
                    "{:#?}",
                    decision(&cycle, TASK_B)
                );
            }
        } else {
            run(&mut slots, at(1, 7, 1), TASK_B, None);
        }
        slots.close();
    }
}
