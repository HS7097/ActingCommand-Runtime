// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15, slice R7 (host gate key 116; register §1.117; model-b15 §7.5, §9.1, §13;
// contracts/policy-suspension.md, `remedies`; contracts/remedies.md section 10): a stopped
// remedy is visible where a person looks, and check-config finds the remedies that cannot be used.
//
// `actingd suspended` lists under `remedies` one entry per `signal:remedy:stopped:` record still
// in effect or lifted within 48 h of the ledger's latest event, newest first, with `lifted` once a
// later `signal:remedy:lifted:` record of the same round exists; a record whose outcome does not
// read back is an unparsed entry. The records are appended as the R4 host records them: a
// `goal_missed` planning signal on the fixture instance (its alias), the round's root task, the
// stop code with its values. check-config's notes are R4's table findings over the bound
// packages and the configuration rows whose names those packages do not declare.
//
// Needs, capabilities, parameters and options are neutral names. Every expected value is the
// brief's requirement, the contract's member list or model-b15's.

use super::clock_slot_report::{ledger_events, record_sequence, report};
use super::*;
use crate::codes::RemedyHostCode;
use actingcommand_contract::outcome::{Outcome, RemedyProblem};
use actingcommand_policy::PolicyCode;
use actingcommand_runtime_config::RemedyChoiceRow;
use serde_json::{Value, json};

const MAIN_TASK: &str = "t_main";
const OTHER_TASK: &str = "t_other";
const SINK_TASK: &str = "t_sink";
const BLOCKED_KEY: &str = "blocked_k";
const NEED: &str = "need_a";
const HOUR_MS: u64 = 3_600_000;
// The contract's span for a lifted stop: within 48 h of the ledger's latest event.
const LIFT_WINDOW_MS: u64 = 48 * HOUR_MS;

// ---- `actingd suspended`: the stop, lift and other records as R4 appends them.

// The round's `<hex>`: the SHA-256 of `task \0 instance \0 decision_id` (register B15-4).
fn round_hex(task_id: &str, decision_id: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{task_id}\0{POLICY_INSTANCE_ALIAS}\0{decision_id}").as_bytes())
    )
}

fn signal_id(kind: &str, task_id: &str, decision_id: &str) -> String {
    format!("signal:remedy:{kind}:{}", round_hex(task_id, decision_id))
}

// A `signal:remedy:<kind>:` record of the round `decision_id` of `task_id` on the fixture
// instance, observed at `at`.
fn signal(
    kind: &str,
    task_id: &str,
    decision_id: &str,
    outcome: Outcome,
    at: u64,
) -> PolicyPlanningSignalEventData {
    PolicyPlanningSignalEventData {
        signal_id: signal_id(kind, task_id, decision_id),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: Some(task_id.to_owned()),
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome,
        observed_at_unix_ms: at,
        detection_budget: None,
    }
}

// A stop's outcome as the plan writes it: the root task, the instance, the round, the layer's
// key and need, the problem and the inputs digest.
fn stop_outcome(
    code: PolicyCode,
    problem: RemedyProblem,
    task_id: &str,
    decision_id: &str,
) -> Outcome {
    Outcome::new(code)
        .task_id(task_id)
        .instance_id(POLICY_INSTANCE_ALIAS)
        .decision_id(decision_id)
        .outcome_key(BLOCKED_KEY)
        .need(NEED)
        .remedy_problem(problem)
        .remedy_inputs_digest(format!("sha256:{}", "a".repeat(64)))
}

// The lift of the round `decision_id` of `task_id`.
fn lift(task_id: &str, decision_id: &str, at: u64) -> PolicyPlanningSignalEventData {
    signal(
        "lifted",
        task_id,
        decision_id,
        Outcome::new(PolicyCode::PolicyRemedyLifted)
            .task_id(task_id)
            .instance_id(POLICY_INSTANCE_ALIAS)
            .decision_id(decision_id)
            .remedy_inputs_digest(format!("sha256:{}", "b".repeat(64))),
        at,
    )
}

// A closed state root whose ledger holds `signals`, each appended with the Runtime clock at its
// time: its `actingd suspended` report and its records.
fn reported(signals: &[(u64, PolicyPlanningSignalEventData)]) -> (Value, Vec<PersistedEvent>) {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let sources = policy_sources(1);
    host.activate_policy_catalog(&sources)
        .expect("activate the catalog");
    for (at, record) in signals {
        advance_manual_clock_to(&clock, *at);
        host.record_policy_planning_signal(record.clone())
            .expect("append the planning signal");
    }
    host.close().expect("close the runtime host");
    (report(&root, &sources), ledger_events(&root))
}

// The `remedies` entry of a stop of `task_id` read back: the contract's members.
fn entry(
    events: &[PersistedEvent],
    stop: &PolicyPlanningSignalEventData,
    code: &str,
    problem: &str,
    at: u64,
) -> Value {
    json!({
        "code": code,
        "remedy_problem": problem,
        "task_id": stop.task_id,
        "instance_id": POLICY_INSTANCE_ALIAS,
        "outcome_key": BLOCKED_KEY,
        "need": NEED,
        "recorded_sequence": record_sequence(events, &stop.signal_id),
        "recorded_at_unix_ms": at,
    })
}

// Requirement 1: a stop is listed while it is in effect; once a later lift of its round is
// recorded it is listed with `lifted` (the lift's sequence and time) while the lift is within
// 48 h of the ledger's latest event, then no longer; a stop still in effect stays listed whatever
// its age. Newest first; no `remedies_total` below 200.
#[test]
fn b15w2_suspended_lists_a_stopped_remedy_until_it_is_lifted() {
    let unchosen_at = POLICY_NOW_UNIX_MS + 60_000;
    let exhausted_at = POLICY_NOW_UNIX_MS + 120_000;
    let lifted_at = POLICY_NOW_UNIX_MS + HOUR_MS;
    let later_at = lifted_at + LIFT_WINDOW_MS + 60_000;
    let unchosen = signal(
        "stopped",
        MAIN_TASK,
        "round-1",
        stop_outcome(
            PolicyCode::PolicyRemedyUnchosen,
            RemedyProblem::NoChoice,
            MAIN_TASK,
            "round-1",
        ),
        unchosen_at,
    );
    let exhausted = signal(
        "stopped",
        OTHER_TASK,
        "round-2",
        stop_outcome(
            PolicyCode::PolicyRemedyExhausted,
            RemedyProblem::RoundLimit,
            OTHER_TASK,
            "round-2",
        )
        .count(2_u32)
        .limit_count(2_u32)
        .remedy_depth(1_u32),
        exhausted_at,
    );
    let lifted = lift(MAIN_TASK, "round-1", lifted_at);
    let unavailable = signal(
        "stopped",
        MAIN_TASK,
        "round-3",
        stop_outcome(
            PolicyCode::PolicyRemedyUnavailable,
            RemedyProblem::CapabilityUnbound,
            MAIN_TASK,
            "round-3",
        )
        .remedy_source(actingcommand_contract::outcome::RemedySource::Package)
        .capability("cap_x"),
        later_at,
    );

    // Two stops in effect.
    let (stopped, events) = reported(&[
        (unchosen_at, unchosen.clone()),
        (exhausted_at, exhausted.clone()),
    ]);
    assert_eq!(
        stopped["remedies"],
        json!([
            entry(
                &events,
                &exhausted,
                "policy_remedy_exhausted",
                "round_limit",
                exhausted_at
            ),
            entry(
                &events,
                &unchosen,
                "policy_remedy_unchosen",
                "no_choice",
                unchosen_at
            ),
        ]),
        "{stopped:#}"
    );
    assert!(stopped.get("remedies_total").is_none(), "{stopped:#}");

    // The first round lifted: listed with its lift.
    let (after_lift, events) = reported(&[
        (unchosen_at, unchosen.clone()),
        (exhausted_at, exhausted.clone()),
        (lifted_at, lifted.clone()),
    ]);
    let mut lifted_entry = entry(
        &events,
        &unchosen,
        "policy_remedy_unchosen",
        "no_choice",
        unchosen_at,
    );
    lifted_entry["lifted"] = json!({
        "sequence": record_sequence(&events, &lifted.signal_id),
        "at_unix_ms": lifted_at,
    });
    assert_eq!(
        after_lift["remedies"],
        json!([
            entry(
                &events,
                &exhausted,
                "policy_remedy_exhausted",
                "round_limit",
                exhausted_at
            ),
            lifted_entry,
        ]),
        "{after_lift:#}"
    );

    // More than 48 h after the lift: the lifted stop is gone, the stop still in effect stays.
    let (later, events) = reported(&[
        (unchosen_at, unchosen),
        (exhausted_at, exhausted.clone()),
        (lifted_at, lifted),
        (later_at, unavailable.clone()),
    ]);
    assert_eq!(
        later["remedies"],
        json!([
            entry(
                &events,
                &unavailable,
                "policy_remedy_unavailable",
                "capability_unbound",
                later_at
            ),
            entry(
                &events,
                &exhausted,
                "policy_remedy_exhausted",
                "round_limit",
                exhausted_at
            ),
        ]),
        "{later:#}"
    );
}

// Requirement 1, the clock-slot rule: a stop record whose outcome does not read back (here its
// code is not a stop code) is listed as `{parse_state: "unparsed", signal_id, code}` and adds no
// report warning.
#[test]
fn b15w2_an_unreadable_remedy_signal_is_listed_unparsed() {
    let at = POLICY_NOW_UNIX_MS + 60_000;
    let unreadable = signal(
        "stopped",
        MAIN_TASK,
        "round-1",
        Outcome::new(PolicyCode::PolicyRemedyExpired)
            .task_id(MAIN_TASK)
            .instance_id(POLICY_INSTANCE_ALIAS)
            .decision_id("round-1")
            .outcome_key(BLOCKED_KEY)
            .need(NEED)
            .window_end_unix_ms(POLICY_NOW_UNIX_MS + 10 * HOUR_MS),
        at,
    );

    let (without, _) = reported(&[]);
    let (report, _) = reported(&[(at, unreadable.clone())]);
    assert_eq!(
        report["remedies"],
        json!([{
            "parse_state": "unparsed",
            "signal_id": unreadable.signal_id,
            "code": "policy_remedy_expired",
        }]),
        "{report:#}"
    );
    assert_eq!(report["warnings"], without["warnings"], "{report:#}");
}

// ---- check-config: the table over the bound packages and the configuration rows.

const MAIN_PROCEDURE: &str = "procedure.main";
const SINK_PROCEDURE: &str = "procedure.sink";
const MAIN_PACKAGE: &str = "neutral.test.main";
const SINK_PACKAGE: &str = "neutral.test.sink";
// The registry id of an instance with its own configuration list.
const REGISTRY_ID: &str = "instance_0192f3a4-5b6c-7d8e-9f01-23456789abcd";

// One source package directory: from `home` to `page_b` or `result`, `page_b` mapped to
// `blocked_k` with `remedy`, when one is given; otherwise from `home` to `result`; with
// `provides` when one is given.
fn write_package(
    directory: &Path,
    package_id: &str,
    remedy: Option<Value>,
    provides: Option<Value>,
) {
    let pages: &[(&str, [u8; 3])] = if remedy.is_some() {
        &[
            ("home", [255, 0, 0]),
            ("page_b", [0, 0, 255]),
            ("result", [255, 255, 255]),
        ]
    } else {
        &[("home", [255, 0, 0]), ("result", [255, 255, 255])]
    };
    let ends = if remedy.is_some() {
        json!(["page_b", "result"])
    } else {
        json!("result")
    };
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(provides) = provides {
        control["provides"] = provides;
    }
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "B15 R7 fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": pages
            .iter()
            .map(|(page, colour)| json!({
                "id": format!("{page}_colour"),
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                "expected": colour
            }))
            .collect::<Vec<_>>(),
        "page_rules": pages
            .iter()
            .map(|(page, _)| ((*page).to_owned(), json!({"required": [format!("{page}_colour")]})))
            .collect::<serde_json::Map<_, _>>(),
        "entry_page": "home",
        "target_page": ends.clone(),
        "operations": [{
            "id": "go",
            "from": "home",
            "to": ends,
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    if let Some(remedy) = remedy {
        task["scheduling_outcome"] = json!({"mappings": [
            {"outcome_key": BLOCKED_KEY, "effect": "no_designated_effect",
             "terminal_pages": ["page_b"], "remedy": remedy},
            {"outcome_key": "done", "effect": "no_designated_effect",
             "terminal_pages": ["result"]}
        ]});
    }
    for (file, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(file);
        fs::create_dir_all(path.parent().expect("a package file's directory"))
            .expect("package directory");
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
        .expect("package file");
    }
}

// The fixture catalog with `t_main` and `t_sink`, each on the fixture instance, bound to its
// procedure.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let mut main = tasks["tasks"][0].clone();
    main["id"] = json!(MAIN_TASK);
    main["procedure_ref"] = json!(MAIN_PROCEDURE);
    let mut sink = main.clone();
    sink["id"] = json!(SINK_TASK);
    sink["procedure_ref"] = json!(SINK_PROCEDURE);
    tasks["tasks"] = json!([main, sink]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    sources
}

// Each task's procedure bound at its package's location, as actingd binds a scheduled procedure.
fn manifest(main: &Path, sink: &Path) -> ProcedureManifest {
    ProcedureManifest::new([(MAIN_PROCEDURE, main), (SINK_PROCEDURE, sink)].map(
        |(procedure_ref, path)| {
            ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding")
            .with_package_path(path.to_string_lossy())
        },
    ))
    .expect("procedure manifest")
}

// Requirement 3: check-config builds R4's table over the bound packages: `t_main`'s remedy of
// `need_a` lists `cap_x`, which `t_sink` provides on the fixture instance, and `cap_z`, which no
// task provides there: one `capability_unbound` note naming the task, key, need, capability,
// instance and package. Then the configuration rows: the section's `allow` of an undeclared need
// is `choice_invalid` naming `need` only; an instance's own `parameter` row whose value is not one
// of the declared options is `parameter_invalid` naming the capability and that instance; rows
// the packages declare give nothing. Each row's note carries a developer detail naming the row.
#[test]
fn b15w2_check_config_notes_an_unbound_capability_and_an_invalid_configuration_row() {
    let root = TempDir::new().expect("tempdir");
    let (main, sink) = (root.path().join("main"), root.path().join("sink"));
    write_package(
        &main,
        MAIN_PACKAGE,
        Some(json!({"need": NEED, "capabilities": ["cap_x", "cap_z"]})),
        None,
    );
    write_package(
        &sink,
        SINK_PACKAGE,
        None,
        Some(json!([{"capability": "cap_x", "parameters": [
            {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}
        ]}])),
    );
    let host = config(&root).with_procedure_manifest(manifest(&main, &sink));
    let lists = [
        RemedyChoiceList {
            instance_id: None,
            rows: vec![
                RemedyChoiceRow::Allow {
                    need: NEED.to_owned(),
                    capability: "cap_x".to_owned(),
                },
                RemedyChoiceRow::Allow {
                    need: "need_b".to_owned(),
                    capability: "cap_x".to_owned(),
                },
            ],
        },
        RemedyChoiceList {
            instance_id: Some(REGISTRY_ID.to_owned()),
            rows: vec![
                RemedyChoiceRow::Parameter {
                    capability: "cap_x".to_owned(),
                    parameter: "param_p".to_owned(),
                    value: "opt_9".to_owned(),
                },
                RemedyChoiceRow::Parameter {
                    capability: "cap_x".to_owned(),
                    parameter: "param_p".to_owned(),
                    value: "opt_2".to_owned(),
                },
            ],
        },
    ];

    let notes = crate::remedy_declaration_notes(&host, &catalog(), &lists).notes;
    let unusable = || Outcome::new(RemedyHostCode::PolicyRemedyDeclarationUnusable);
    assert_eq!(
        notes
            .iter()
            .map(|noted| noted.outcome.clone())
            .collect::<Vec<_>>(),
        vec![
            unusable()
                .remedy_problem(RemedyProblem::CapabilityUnbound)
                .task_id(MAIN_TASK)
                .outcome_key(BLOCKED_KEY)
                .need(NEED)
                .capability("cap_z")
                .instance_id(POLICY_INSTANCE_ALIAS)
                .package_id(MAIN_PACKAGE),
            unusable()
                .remedy_problem(RemedyProblem::ChoiceInvalid)
                .need("need_b"),
            unusable()
                .remedy_problem(RemedyProblem::ParameterInvalid)
                .capability("cap_x")
                .instance_id(REGISTRY_ID),
        ],
        "{notes:#?}"
    );
    assert_eq!(notes[0].detail, None, "{notes:#?}");
    for (row_note, named) in [
        (&notes[1], ["need_b", "cap_x"]),
        (&notes[2], ["param_p", "opt_9"]),
    ] {
        let detail = row_note.detail.as_deref().unwrap_or_default();
        assert!(
            named.iter().all(|name| detail.contains(name)),
            "the developer detail names the row: {notes:#?}"
        );
    }
}

// Fail Loud (coordinator acceptance of #989): a bound package whose remedy declarations cannot be
// read is never silent in check-config. `t_main`'s source package provides `cap_x`, the
// capability its own remedy names, which the reader's source parser refuses
// (`resource_declaration_invalid`, remedies.md section 2.4 rules 1 and 2). It gives the developer
// line the running Runtime writes for it,
// `actingd remedy_table unreadable code=<code> tasks=<tasks> path=<path>`, and no note: the
// configuration stays valid.
#[test]
fn b15w2_check_config_names_an_unreadable_package() {
    let root = TempDir::new().expect("tempdir");
    let (main, sink) = (root.path().join("main"), root.path().join("sink"));
    write_package(
        &main,
        MAIN_PACKAGE,
        Some(json!({"need": NEED, "capabilities": ["cap_x"]})),
        Some(json!([{"capability": "cap_x"}])),
    );
    write_package(&sink, SINK_PACKAGE, None, None);
    let host = config(&root).with_procedure_manifest(manifest(&main, &sink));

    let checked = crate::remedy_declaration_notes(&host, &catalog(), &[]);
    assert_eq!(
        checked.unreadable,
        vec![format!(
            "actingd remedy_table unreadable code=resource_declaration_invalid tasks={MAIN_TASK} path={}",
            main.to_string_lossy()
        )],
        "{checked:#?}"
    );
    assert!(checked.notes.is_empty(), "{checked:#?}");
}
