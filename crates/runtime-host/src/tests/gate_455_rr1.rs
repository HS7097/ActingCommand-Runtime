// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR1 (0.12.92; register §1.535; contracts/standard-pack-goal-tables.md, "Who reads
// it"; contracts/standard-pack-goals.md, "The records entry"; coordinator work order
// rd-1123/brief-455-rr1.md), the host half. The host runs, as `gate_463_v3` does, in composer mode
// over one game root on disk, a manual Runtime clock and one fake instance; the root's one rule is
// due at every evaluation. Its `goals.json` (version 3) holds a heading and a `records` entry that
// names the table `heroes` of its `goal-tables.json`. The scheduler workbench reads the tables
// with the goal declaration, once per content, and keeps them for the generation in force; a
// tables file the reader refuses applies no records entry and the root runs, recorded with the
// file's own code (fix round 1, rd-1123/brief-455-rr1-fix1.md, ruling 5); the goal page lists
// no records entry, and a set or a clear of its name answers `holds_no_value`. Every name, pool
// and amount is made up; no clock is read and nothing sleeps.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use crate::RootGoalTables;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    DeclaredInitiator, GoalDeclarationRead, GoalEdit, GoalEntryType, GoalProblem, GoalView,
    Initiator, RuntimeChannel, RuntimeLifecyclePhase, RuntimePayload, StandardPackProblem,
};
use serde_json::{Value, json};
use std::fs;

const TASK: &str = "fixture.tables";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const RESULT: [u8; 3] = [255, 255, 255];
const HOME: [u8; 3] = [255, 0, 0];
const VIEWED: &str = "request_goal_view_completed";
const INVALID: &str = "goal_value_invalid";
/// The code of a `goals.json` that is not applied, `goals.json`'s alone (fix round 1).
const GOALS_NOT_APPLIED: &str = "standard_pack_goals_not_applied";
/// Fix round 1: the code of a `goal-tables.json` that is not applied, the file's own.
const TABLES_NOT_APPLIED: &str = "standard_pack_goal_tables_not_applied";
/// The default `standard_packs.check_interval_ms`: a check reads the roots again after it.
const CHECK_INTERVAL_MS: u64 = 60_000;
/// The records entry's name.
const RECORDS: &str = "raise";

/// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule (due at
/// every evaluation, producing the pool) and the package, as `gate_463_v3` places its own.
fn place_game_root(packs: &Path) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[TASK]),
    );
    let mut due = rule(TASK, TASK, 100, Some(POOL_A));
    due["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    due["loop_budget"]["window_iteration_limit"] = json!(16);
    due["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(16);
    }
    let mut valued = pool(POOL_A);
    valued["valuation"]["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 100});
    write_json(
        &game.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [],
            "rules": [due],
            "pools": [valued],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
    write_package(&game.join(TASK), TASK, None, None);
}

fn goals_path(packs: &Path) -> PathBuf {
    packs.join(GAME_A).join("goals.json")
}

fn tables_path(packs: &Path) -> PathBuf {
    packs.join(GAME_A).join("goal-tables.json")
}

/// The game's `goals.json`, version 3: a heading and, with `records`, the records entry.
fn goals(records: bool) -> Value {
    let mut entries =
        vec![json!({"name": "group_a", "type": "heading", "title": {"en": "Group A"}})];
    if records {
        entries.push(
            json!({"name": RECORDS, "type": "records", "table": "heroes",
                            "max_records": 8, "title": {"en": "Whom to raise"}}),
        );
    }
    json!({"schema_version": "actingcommand.goals.v3", "game": GAME_A, "entries": entries})
}

/// The game's `goal-tables.json`: the table `heroes`, whose one class's one step uses `amount` of
/// the item on the pool; `class` is the class its one key names (`common` is the table's).
fn tables(amount: u64, class: &str) -> Value {
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": GAME_A,
        "items": {"coin": {"pool": POOL_A}},
        "tables": [{
            "name": "heroes",
            "progress": [{"fact_key": "inventory.list.fixture_heroes", "key_column": "hero",
                          "held_by": true, "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"common": {"grade": {"positions": [[0], [1]],
                                              "steps": [{"coin": amount}]}}},
            "keys": {"hero_a": {"class": class}}
        }]
    })
}

/// Rewrites the package's control declaration one byte longer (its `timeout_ms` 8000 becomes
/// 80000, which nothing here reads), as `gate_463_v2` does, so the next check reads the root
/// again for certain whatever the file times say.
fn one_byte_longer(packs: &Path) {
    let control_path = packs.join(GAME_A).join(TASK).join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    assert_eq!(
        control["timeout_ms"],
        json!(8_000),
        "setup: the fixture's timeout"
    );
    control["timeout_ms"] = json!(80_000);
    write_json(&control_path, &control);
}

fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = vec![InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: SERVER_A.to_owned(),
        game_id: GAME_A.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    }];
    facts
}

struct Scene {
    _state_root: TempDir,
    packs: TempDir,
    device: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Scene {
    /// The host over the game root `place` places below its standard-pack root.
    fn with(place: impl FnOnce(&Path)) -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place(packs.path());
        let device = Arc::new(FakeState::default());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config(&state_root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(
                FakeProvider::one(INSTANCE, instance_id(), Arc::clone(&device))
                    .fixture_simulation(),
            ),
        )
        .expect("start the composer-mode host");
        Self {
            _state_root: state_root,
            packs,
            device,
            clock,
            host,
        }
    }

    fn sample_capacity(&self) {
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
    }

    /// One policy cycle at `at` (the workbench's check of the game root runs in it); its full
    /// evaluation admitted, the rule's dispatch run (`home`, one tap, `result`) and settled: the
    /// root runs. The time its next dispatch may come.
    fn evaluate_and_run(&self, at: u64, seed: u64) -> u64 {
        advance_manual_clock_to(&self.clock, at);
        self.sample_capacity();
        let evaluation = self
            .host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                seed,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy at {at}: {error}"))
            .evaluation
            .expect("a full evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK && intent.instance_id == INSTANCE)
            .unwrap_or_else(|| panic!("the root runs: a dispatch of {TASK}: {evaluation:#?}"))
            .clone();
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the intent's reason chain")
            .clone();
        self.sample_capacity();
        let admission = PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: BTreeSet::new(),
            fencing_owner_epoch: self.host.runtime_info().owner_epoch(),
            now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
        };
        let request = ContainedTaskRequest::new(
            self.packs
                .path()
                .join(GAME_A)
                .join(TASK)
                .to_string_lossy()
                .into_owned(),
        )
        .expect("contained task request");
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(&intent, &chain, &admission, &request)
            .unwrap_or_else(|error| panic!("admit {TASK}: {error}"))
        else {
            panic!("expected one policy run context")
        };
        let taps = self.device.input_count.load(Ordering::Acquire);
        *self.device.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                vec![(0, if inputs <= taps { HOME } else { RESULT })]
            }));
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &request)
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        context.admission().activity.next_eligible_unix_ms
    }

    /// The `goal-tables.json` the workbench keeps of the game root.
    fn kept_tables(&self) -> RootGoalTables {
        self.host
            .standard_pack_goal_roots()
            .expect("the workbench's roots")
            .expect("composer mode")
            .roots
            .get(GAME_A)
            .unwrap_or_else(|| panic!("the workbench keeps {GAME_A}"))
            .goal_tables
            .clone()
    }

    /// The instance's goal page, read on a connection of the CLI.
    fn view(&self) -> GoalView {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::GoalView {
            instance_alias: INSTANCE.to_owned(),
        });
        let receipt = client.send(&request);
        assert_eq!(receipt.outcome().code().as_str(), VIEWED, "{receipt:?}");
        let Some(RuntimeResult::GoalViewed { view }) = receipt.result() else {
            panic!("a goal view: {receipt:?}");
        };
        (**view).clone()
    }

    /// `EditGoal` of `op`, on a connection bound `lab` with `Lab-CLI` and the Lab's envelope.
    fn edit(&self, op: GoalEdit) -> RuntimeReceipt {
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator: Some(DeclaredInitiator::from(Initiator::LabCli)),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        let request = RuntimeRequest::new(
            client.ids.mint_request_id().expect("request id"),
            client.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Lab,
            EventSource::Lab,
            1,
            RuntimeOperation::EditGoal {
                instance_alias: INSTANCE.to_owned(),
                expected_version: None,
                op,
            },
        )
        .expect("lab runtime request");
        client.send(&request)
    }

    /// Fix round 1: the outcomes spelled `spelling` of the workbench's `standard_pack_read`
    /// lifecycle records, in ledger order, each with the file its `path` names.
    fn read_outcomes(&self, spelling: &str) -> Vec<(String, Outcome)> {
        let mut events = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::RuntimeLifecycleObserved),
                ..EventQuery::default()
            })
            .expect("the lifecycle records");
        events.sort_by_key(PersistedEvent::sequence);
        events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => {
                    match value.phase() {
                        RuntimeLifecyclePhase::StandardPackRead { outcome }
                            if outcome.code().as_str() == spelling =>
                        {
                            let file = outcome
                                .value("path")
                                .and_then(Value::as_str)
                                .and_then(|path| Path::new(path).file_name())
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            Some((file, outcome.clone()))
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// The `standard_pack_goal_tables_not_applied` outcomes (fix round 1: the file's own code),
    /// in ledger order; each names `goal-tables.json`.
    fn tables_not_applied(&self) -> Vec<Outcome> {
        self.read_outcomes(TABLES_NOT_APPLIED)
            .into_iter()
            .map(|(file, outcome)| {
                assert_eq!(file, "goal-tables.json", "{outcome}");
                outcome
            })
            .collect()
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

/// The name and the type of each entry the page lists, in its order.
fn listed(view: &GoalView) -> Vec<(String, GoalEntryType)> {
    view.entries
        .iter()
        .map(|entry| (entry.name.clone(), entry.entry_type))
        .collect()
}

/// A refused `EditGoal`: Denied `goal_value_invalid`, no result; its `goal_problem`.
#[track_caller]
fn refused_word(receipt: &RuntimeReceipt) -> Option<String> {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), INVALID, "{receipt:?}");
    assert!(receipt.result().is_none(), "{receipt:?}");
    receipt
        .outcome()
        .value("goal_problem")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

// Rulings 3, 4 and 5: after the first check the workbench keeps the root's accepted tables with
// the sha256 of their bytes, and every cycle's dispatch runs. The root read again (the package's
// declaration one byte longer), the same tables: what it keeps is the same parse (the same shared
// value, not a second one). Another content (one byte longer): parsed again, with its amount. A
// file the reader refuses (its key names a class the table does not have): the workbench keeps the
// refusal at its pointer, one record names `goal-tables.json` (`refused`, caused by the reader's
// `standard_pack_root_invalid` with `table_class_unknown`), the root runs and `goals.json` still
// applies (accepted, its heading listed); the root read again with the same refused file adds no
// second record. Fix round 1 (ruling 5): that record is the file's own code,
// `standard_pack_goal_tables_not_applied`, and no `standard_pack_goals_not_applied` is recorded
// for it; a `goals.json` refused afterwards is `standard_pack_goals_not_applied` naming
// `goals.json`, and the tables' code still names only `goal-tables.json`, once.
#[test]
fn rr1_h1_the_workbench_reads_the_tables_once_per_content_and_a_refused_file_applies_no_records() {
    let scene = Scene::with(|packs| {
        place_game_root(packs);
        write_json(&goals_path(packs), &goals(true));
        write_json(&tables_path(packs), &tables(10, "common"));
    });
    let next = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 6);
    let first = scene.kept_tables();
    let RootGoalTables::Accepted {
        sha256: first_sha,
        tables: first_tables,
    } = &first
    else {
        panic!("accepted tables: {first:?}");
    };
    assert!(first_sha.starts_with("sha256:"), "{first_sha}");
    assert_eq!(
        first_tables.table_names(),
        BTreeSet::from(["heroes".to_owned()])
    );

    one_byte_longer(scene.packs.path());
    let again_at = next.max(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS);
    let next = scene.evaluate_and_run(again_at, 7);
    let again = scene.kept_tables();
    let RootGoalTables::Accepted {
        sha256: again_sha,
        tables: again_tables,
    } = &again
    else {
        panic!("accepted tables: {again:?}");
    };
    assert_eq!(again_sha, first_sha);
    assert!(
        Arc::ptr_eq(first_tables, again_tables),
        "the same content is not parsed again"
    );

    write_json(&tables_path(scene.packs.path()), &tables(200, "common"));
    let other_at = next.max(again_at + CHECK_INTERVAL_MS);
    let next = scene.evaluate_and_run(other_at, 8);
    let other = scene.kept_tables();
    let RootGoalTables::Accepted {
        sha256: other_sha,
        tables: other_tables,
    } = &other
    else {
        panic!("accepted tables: {other:?}");
    };
    assert_ne!(other_sha, first_sha);
    assert!(!Arc::ptr_eq(first_tables, other_tables));
    let step = &other_tables
        .table("heroes")
        .and_then(|table| table.class("common"))
        .and_then(|class| class.ladder("grade"))
        .expect("the class's ladder")
        .steps[0];
    assert_eq!(step.first().map(|(_, amount)| *amount), Some(200));
    assert!(scene.tables_not_applied().is_empty());

    write_json(&tables_path(scene.packs.path()), &tables(200, "rare"));
    let refused_at = next.max(other_at + CHECK_INTERVAL_MS);
    let next = scene.evaluate_and_run(refused_at, 9);
    let RootGoalTables::Refused { sha256, issues } = scene.kept_tables() else {
        panic!("refused tables: {:?}", scene.kept_tables());
    };
    assert!(sha256.starts_with("sha256:"), "{sha256}");
    assert_eq!(
        issues
            .iter()
            .map(|issue| (issue.problem, issue.pointer.as_str()))
            .collect::<Vec<_>>(),
        [(
            StandardPackProblem::TableClassUnknown,
            "/tables/0/keys/hero_a/class"
        )]
    );
    let recorded = scene.tables_not_applied();
    assert_eq!(recorded.len(), 1, "{recorded:#?}");
    assert_eq!(
        recorded[0].value("goals_failure").and_then(Value::as_str),
        Some("refused")
    );
    let cause = recorded[0]
        .causes()
        .first()
        .unwrap_or_else(|| panic!("caused by the reader's refusal: {}", recorded[0]))
        .outcome();
    assert_eq!(cause.code().as_str(), "standard_pack_root_invalid");
    assert_eq!(
        cause.value("standard_pack_problem").and_then(Value::as_str),
        Some("table_class_unknown")
    );
    // Fix round 1: `goals.json`'s code says nothing of the tables file.
    assert!(
        scene.read_outcomes(GOALS_NOT_APPLIED).is_empty(),
        "{:#?}",
        scene.read_outcomes(GOALS_NOT_APPLIED)
    );
    let view = scene.view();
    assert_eq!(view.declaration.state, GoalDeclarationRead::Accepted);
    assert_eq!(
        listed(&view),
        [("group_a".to_owned(), GoalEntryType::Heading)]
    );

    // The root read again, the same refused file: no second record.
    one_byte_longer_again(scene.packs.path());
    let same_at = next.max(refused_at + CHECK_INTERVAL_MS);
    let next = scene.evaluate_and_run(same_at, 10);
    assert_eq!(scene.tables_not_applied().len(), 1, "once per content");

    // Fix round 1: a `goals.json` the reader refuses (version 2, retired) is named by its own
    // code, `standard_pack_goals_not_applied`, naming `goals.json`; the tables file, refused as
    // before, adds no record, and no record of the tables' code names `goals.json`.
    write_json(
        &goals_path(scene.packs.path()),
        &json!({"schema_version": "actingcommand.goals.v2", "game": GAME_A,
                "entries": [{"name": "group_a", "type": "heading", "title": {"en": "Group A"}}]}),
    );
    scene.evaluate_and_run(next.max(same_at + CHECK_INTERVAL_MS), 11);
    let goals_records = scene.read_outcomes(GOALS_NOT_APPLIED);
    assert_eq!(goals_records.len(), 1, "{goals_records:#?}");
    let (file, goals_record) = &goals_records[0];
    assert_eq!(file, "goals.json");
    assert_eq!(
        goals_record.value("goals_failure").and_then(Value::as_str),
        Some("refused")
    );
    assert_eq!(scene.tables_not_applied().len(), 1, "the tables as before");
    scene.close();
}

/// Rewrites the package's control declaration one byte longer again (its `timeout_ms` 80000
/// becomes 800000), after [`one_byte_longer`].
fn one_byte_longer_again(packs: &Path) {
    let control_path = packs.join(GAME_A).join(TASK).join("control.json");
    let mut control: Value =
        serde_json::from_slice(&fs::read(&control_path).expect("the control declaration"))
            .expect("control JSON");
    assert_eq!(
        control["timeout_ms"],
        json!(80_000),
        "setup: rewritten once"
    );
    control["timeout_ms"] = json!(800_000);
    write_json(&control_path, &control);
}

// Rulings 6 and 7: the goal page of a root whose `goals.json` holds a records entry lists no such
// entry, for today's client the page without it; a set and a clear of the records entry's name
// are refused `goal_value_invalid` with the existing word `holds_no_value`, nothing stored (the
// record's version unchanged). The same root's `goals.json` rewritten without the records entry:
// after the next check its page is the page it was, member for member (the target view aside).
#[test]
fn rr1_h2_the_page_lists_no_records_entry_and_a_set_of_its_name_holds_no_value() {
    let scene = Scene::with(|packs| {
        place_game_root(packs);
        write_json(&goals_path(packs), &goals(true));
        write_json(&tables_path(packs), &tables(10, "common"));
    });
    let next = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 6);
    let with_records = scene.view();
    assert_eq!(
        (
            with_records.declaration.state,
            with_records.declaration.sha256.as_deref()
        ),
        (GoalDeclarationRead::Accepted, None)
    );
    assert_eq!(
        listed(&with_records),
        [("group_a".to_owned(), GoalEntryType::Heading)]
    );
    let set = scene.edit(GoalEdit::Set {
        name: RECORDS.to_owned(),
        value_json: json!([{"key": "hero_a", "to": {"grade": [1]}}]).to_string(),
    });
    assert_eq!(
        refused_word(&set).as_deref(),
        Some(GoalProblem::HoldsNoValue.as_str())
    );
    let clear = scene.edit(GoalEdit::Clear {
        name: RECORDS.to_owned(),
    });
    assert_eq!(
        refused_word(&clear).as_deref(),
        Some(GoalProblem::HoldsNoValue.as_str())
    );
    assert_eq!(scene.view().version, with_records.version, "nothing stored");

    write_json(&goals_path(scene.packs.path()), &goals(false));
    scene.evaluate_and_run(next.max(POLICY_NOW_UNIX_MS + CHECK_INTERVAL_MS), 7);
    let without = scene.view();
    assert_eq!(without.declaration, with_records.declaration);
    assert_eq!(without.entries, with_records.entries);
    assert_eq!(without.orphans, with_records.orphans);
    assert_eq!(without.remedy_parameters, with_records.remedy_parameters);
    assert_eq!(without.other_targets, with_records.other_targets);
    assert_eq!(without.version, with_records.version);
    scene.close();
}
