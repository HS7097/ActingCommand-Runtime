// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for the Workflow #308 G4 host fix (to be reverted): a fatal failure of the
//! confirmation capture of a select step, on a manual task run and on a scheduled policy run,
//! through the runtime host with the test fixture device. Every printed line starts with
//! `G4H|`.
//!
//! normal:  the confirmation capture fails fatally; the decision is recorded.
//! refused: the same, and the task deadline passes while the failing capture is in the device
//!          (the manual runtime clock is advanced once the confirmation capture's
//!          `capture.requested` is in the ledger), so the host refuses the select record.

use super::*;

const SELECT_GAME: &str = "fixture-game-a";
const SELECT_SERVER: &str = "fixture-server-a";
const SELECT_DEADLINE_MS: u64 = 20_000;
const SLOW_CAPTURE_MS: u64 = 1_500;

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "G4H|{}", line.replace('\n', " | ")).expect("write stdout");
    stdout.flush().expect("flush stdout");
}

fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| format!("<json {error}>"))
}

/// A package whose only step is a select step on `home`: one slot over the guard pixel, a
/// policy that selects an open slot, and the step's guard on the same pixel.
fn select_package() -> Vec<u8> {
    let (width, height) = (16, 9);
    let policy = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "slot_pick",
        "applies_to": {
            "candidate_layout_id": "layout/slots",
            "outcome_keys": {
                "selected": "slot_selected",
                "empty": "slot_none",
                "insufficient": "slot_none_open",
                "ambiguous": "slot_ambiguous",
                "unknown": "slot_unknown"
            }
        },
        "fields": [{"name": "open", "value_type": {"type": "boolean"}}],
        "facts": [],
        "gates": [{
            "gate_id": "open",
            "predicate": {"kind": "boolean_equals", "value": {"source": "field", "field": "open"}, "expected": true},
            "on_unknown": {"kind": "drop_candidate"}
        }],
        "scoring": [{
            "term_id": "open",
            "value": {"source": "field", "field": "open"},
            "transform": {"kind": "lookup", "entries": [
                {"key": {"type": "boolean", "value": true}, "value_milli": 1000},
                {"key": {"type": "boolean", "value": false}, "value_milli": 0}]},
            "weight_milli": 1000,
            "on_unknown": {"kind": "drop_candidate"}
        }],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("policy JSON");
    let policy_sha256 = format!("{:x}", Sha256::digest(&policy));
    let control = serde_json::to_vec(&serde_json::json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "fixture.select.task",
        "execution_mode": "navigable_route",
        "game": SELECT_GAME,
        "server": SELECT_SERVER,
        "resolution": {"width": width, "height": height},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 20_000,
        "timeout_ms": 30_000,
        "max_steps": 2
    }))
    .expect("control JSON");
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.3",
        "entry_task_id": "task",
        "files": [{"path": "operations/task/policies/pick.json", "sha256": policy_sha256.clone()}]
    }))
    .expect("manifest JSON");
    let task = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.7",
        "task_id": "task",
        "game": SELECT_GAME,
        "server_scope": [SELECT_SERVER],
        "coordinate_space": {"width": width, "height": height},
        "target_page": "terminal",
        "candidate_layouts": [{
            "id": "layout/slots",
            "page_id": "home",
            "kind": "fixed_slots",
            "features": [{"name": "open", "value": "passed"}],
            "slots": [{
                "rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "click": {"x": 1, "y": 0, "width": 1, "height": 1},
                "targets": {"open": "guard/ready"}
            }]
        }],
        "operations": [{
            "id": "choose_slot",
            "from": "home",
            "to": "terminal",
            "select": {
                "layout_id": "layout/slots",
                "policy": {"path": "policies/pick.json", "sha256": policy_sha256}
            },
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "expect_after": {"page_id": "terminal", "timeout_ms": 1000, "interval_ms": 1},
            "retryable": false
        }]
    }))
    .expect("task JSON");
    let pack = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.7",
        "game": SELECT_GAME,
        "server": SELECT_SERVER,
        "coordinate_space": {"width": width, "height": height},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "page/error", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 0]},
            {"type": "color", "id": "guard/ready", "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ],
        "candidate_layouts": [{
            "id": "layout/slots",
            "page_id": format!("{SELECT_GAME}/home"),
            "kind": "fixed_slots",
            "features": [{"name": "open", "value": "passed"}],
            "slots": [{
                "rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "click": {"x": 1, "y": 0, "width": 1, "height": 1},
                "targets": {"open": "guard/ready"}
            }]
        }]
    }))
    .expect("pack JSON");
    let pages = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": format!("{SELECT_GAME}/home"), "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": format!("{SELECT_GAME}/terminal"), "required": ["page/terminal"], "optional": [], "forbidden": []},
            {"id": format!("{SELECT_GAME}/error"), "required": ["page/error"], "optional": [], "forbidden": []}
        ]
    }))
    .expect("pages JSON");
    let pack_path = format!("resources/recognition/{SELECT_GAME}.{SELECT_SERVER}.pack.json");
    let pages_path = format!("resources/recognition/{SELECT_GAME}.{SELECT_SERVER}.pages.json");
    let files: [(&str, &[u8]); 6] = [
        ("control.json", &control),
        ("resources/manifest.json", &manifest),
        ("resources/operations/task/task.json", &task),
        ("resources/operations/task/policies/pick.json", &policy),
        (&pack_path, &pack),
        (&pages_path, &pages),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn describe(event: &PersistedEvent) -> Option<String> {
    let kind = format!("{:?}", event.event_type());
    match event.payload() {
        EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
            TaskSemanticFact::SelectionEvaluated {
                step_index,
                operation_label,
                selection,
            } => Some(format!(
                "{kind} step={step_index} op={operation_label} outcome={} selected={:?} confirmation={}",
                to_json(&selection.outcome),
                selection.selected,
                to_json(&selection.confirmation)
            )),
            TaskSemanticFact::TerminalCommitted {
                outcome,
                failure_code,
                executed_steps,
                ..
            } => Some(format!(
                "{kind} outcome={outcome:?} failure_code={failure_code:?} executed_steps={executed_steps:?}"
            )),
            _ => None,
        },
        EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(payload)) => {
            Some(match payload.lifecycle_failure() {
                Some(failure) => format!(
                    "{kind} stage={} code={} operation={:?} fatal={:?} native_detail={:?}",
                    failure.stage(),
                    failure.code(),
                    failure.operation(),
                    failure.fatal(),
                    failure.native_detail().map(|detail| detail.text())
                ),
                None => kind,
            })
        }
        _ if matches!(
            event.event_type(),
            EventType::CaptureFailed
                | EventType::InputIntent
                | EventType::InputCommitted
                | EventType::InputFailed
                | EventType::TaskEffectIntent
                | EventType::TaskCancelled
                | EventType::LeaseReleased
                | EventType::PolicyExecutionRecorded
        ) =>
        {
            Some(kind)
        }
        _ => None,
    }
}

struct Run {
    events: Vec<PersistedEvent>,
    outcome: String,
    primary: String,
    detail: String,
    inputs: usize,
    captures: usize,
    advanced: bool,
}

impl Run {
    fn selection_evaluated(&self) -> Vec<String> {
        self.events
            .iter()
            .filter(|event| event.event_type() == EventType::TaskSelectionEvaluated)
            .filter_map(describe)
            .collect()
    }

    fn lifecycle_failures(&self, selection_record: bool) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(payload)) => {
                    payload.lifecycle_failure()
                }
                _ => None,
            })
            .filter(|failure| {
                (failure.stage() == "runtime.lifecycle.selection_record") == selection_record
            })
            .map(|failure| format!("{}:{}", failure.stage(), failure.code()))
            .collect()
    }

    fn task_failed(&self) -> Vec<String> {
        self.events
            .iter()
            .filter(|event| event.event_type() == EventType::TaskFailed)
            .filter_map(describe)
            .collect()
    }

    fn input_events(&self) -> usize {
        self.events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type(),
                    EventType::InputIntent
                        | EventType::InputCommitted
                        | EventType::InputFailed
                        | EventType::TaskEffectIntent
                )
            })
            .count()
    }

    fn print(&self, label: &str) {
        emit(&format!(
            "{label} outcome={} primary={} detail={} inputs_device={} input_events={} captures_device={} clock_advanced={}",
            self.outcome,
            self.primary,
            self.detail,
            self.inputs,
            self.input_events(),
            self.captures,
            self.advanced
        ));
        emit(&format!(
            "{label} sequence={}",
            self.events
                .iter()
                .map(|event| format!("{:?}", event.event_type()))
                .collect::<Vec<_>>()
                .join(",")
        ));
        for line in self.events.iter().filter_map(describe) {
            emit(&format!("{label} event {line}"));
        }
    }
}

fn watch_confirmation_capture(
    host: &RuntimeHost,
    query: EventQuery,
    confirmation_capture: usize,
    clock: &ManualRuntimeClock,
    done: &AtomicBool,
    advanced: &AtomicBool,
) {
    let started = Instant::now();
    while !done.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(120) {
        let requested = host
            .query_persisted_events_for_test(query.clone())
            .map(|events| {
                events
                    .iter()
                    .filter(|event| event.event_type() == EventType::CaptureRequested)
                    .count()
            })
            .unwrap_or(0);
        if requested >= confirmation_capture {
            clock.advance(SELECT_DEADLINE_MS + 1);
            advanced.store(true, Ordering::Release);
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn close(host: RuntimeHost) -> String {
    let fatal = match host.fatal_error() {
        Ok(Some(error)) => format!("fatal:{}", error.code()),
        Ok(None) => "healthy".to_owned(),
        Err(error) => format!("health_error:{}", error.code()),
    };
    let closed = match host.close() {
        Ok(()) => "closed".to_owned(),
        Err(error) => format!("close_error:{}", error.code()),
    };
    format!("{fatal} {closed}")
}

fn manual_run(confirmation_capture: usize, refuse: bool) -> (Run, String) {
    let root = TempDir::new().expect("tempdir");
    let bytes = select_package();
    let package = root.path().join("select-task.zip");
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .fail_capture_on
        .store(confirmation_capture, Ordering::Release);
    if refuse {
        state
            .capture_delay_ms
            .store(SLOW_CAPTURE_MS, Ordering::Release);
    }
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let task_request = ContainedTaskRequest::new(package.display().to_string(), expected)
        .expect("contained task request")
        .with_response_deadline_ms(SELECT_DEADLINE_MS)
        .expect("bounded task deadline");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let query = EventQuery {
        request_id: Some(request.request_id()),
        ..EventQuery::default()
    };
    let done = AtomicBool::new(false);
    let advanced = AtomicBool::new(false);
    let result = thread::scope(|scope| {
        if refuse {
            let (host, query, clock, done, advanced) =
                (&host, query.clone(), &clock, &done, &advanced);
            scope.spawn(move || {
                watch_confirmation_capture(
                    host,
                    query,
                    confirmation_capture,
                    clock,
                    done,
                    advanced,
                )
            });
        }
        let result = host
            .process_request_for_test(&request, ConnectionId::new(901).expect("connection"));
        done.store(true, Ordering::Release);
        result
    });
    let (outcome, primary, detail) = match &result {
        Ok(receipt) => (
            format!("receipt:{:?}", receipt.state()),
            format!(
                "{:?}",
                receipt.error_projection().map(|projection| projection.code)
            ),
            format!("{:?}", receipt.error_projection()),
        ),
        Err(error) => (
            "host_error".to_owned(),
            error.code().to_owned(),
            format!(
                "declaration={:?} native={:?}",
                error.resource_declaration(),
                error.diagnostics().native_detail().map(|detail| detail.text())
            ),
        ),
    };
    let events = host
        .query_persisted_events_for_test(query)
        .expect("run events");
    let run = Run {
        events,
        outcome,
        primary,
        detail,
        inputs: state.input_count.load(Ordering::Acquire),
        captures: state.capture_count.load(Ordering::Acquire),
        advanced: advanced.load(Ordering::Acquire),
    };
    (run, close(host))
}

fn scheduled_run(confirmation_capture: usize, refuse: bool) -> (Run, String) {
    let root = TempDir::new().expect("tempdir");
    let package = select_package();
    let package_path = root.path().join("scheduled-select-task.zip");
    fs::write(&package_path, &package).expect("write scheduled package");
    let request = ContainedTaskRequest::new(
        package_path.to_string_lossy().into_owned(),
        format!("{:x}", Sha256::digest(&package)),
    )
    .expect("scheduled package request")
    .with_response_deadline_ms(SELECT_DEADLINE_MS)
    .expect("bounded scheduled deadline");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .fail_capture_on
        .store(confirmation_capture, Ordering::Release);
    if refuse {
        state
            .capture_delay_ms
            .store(SLOW_CAPTURE_MS, Ordering::Release);
    }
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("scheduled runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate scheduled catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(
            &intent,
            &reasons,
            &policy_context(&host, &intent),
            &request,
        )
        .expect("scheduled admission")
    else {
        panic!("expected scheduled context")
    };
    let query = EventQuery {
        run_id: Some(context.run_id()),
        ..EventQuery::default()
    };
    let done = AtomicBool::new(false);
    let advanced = AtomicBool::new(false);
    let result = thread::scope(|scope| {
        if refuse {
            let (host, query, clock, done, advanced) =
                (&host, query.clone(), &clock, &done, &advanced);
            scope.spawn(move || {
                watch_confirmation_capture(
                    host,
                    query,
                    confirmation_capture,
                    clock,
                    done,
                    advanced,
                )
            });
        }
        let result = host.run_scheduled_contained_task(&context, &request);
        done.store(true, Ordering::Release);
        result
    });
    let (outcome, primary, detail) = match &result {
        Ok(receipt) => (
            format!("receipt:{:?}", receipt.state()),
            format!(
                "{:?}",
                receipt.error_projection().map(|projection| projection.code)
            ),
            format!("{:?}", receipt.error_projection()),
        ),
        Err(error) => (
            "run_error".to_owned(),
            error.code().to_owned(),
            format!(
                "declaration={:?} native={:?}",
                error.resource_declaration(),
                error.diagnostics().native_detail().map(|detail| detail.text())
            ),
        ),
    };
    let events = host
        .query_persisted_events_for_test(query)
        .expect("scheduled run events");
    let run = Run {
        events,
        outcome,
        primary,
        detail,
        inputs: state.input_count.load(Ordering::Acquire),
        captures: state.capture_count.load(Ordering::Acquire),
        advanced: advanced.load(Ordering::Acquire),
    };
    (run, close(host))
}

fn find_confirmation_capture(
    path: &str,
    run: impl Fn(usize, bool) -> (Run, String),
) -> Option<(usize, Run, String)> {
    for confirmation_capture in 2..=4 {
        let (attempt, closed) = run(confirmation_capture, false);
        let records = attempt.selection_evaluated();
        emit(&format!(
            "{path} search fail_capture_on={confirmation_capture} outcome={} primary={} selection_evaluated={records:?} host={closed} detail={}",
            attempt.outcome, attempt.primary, attempt.detail
        ));
        if records
            .iter()
            .any(|record| record.contains("\"capture_failed\""))
        {
            return Some((confirmation_capture, attempt, closed));
        }
    }
    None
}

fn compare(path: &str, normal: &Run, refused: &Run) {
    let normal_task_failed = normal
        .task_failed()
        .iter()
        .map(|line| line.split(" executed_steps").next().unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    let refused_task_failed = refused
        .task_failed()
        .iter()
        .map(|line| line.split(" executed_steps").next().unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    emit(&format!(
        "{path} CHECK normal: selection_evaluated={:?} inputs_device={} input_events={}",
        normal.selection_evaluated(),
        normal.inputs,
        normal.input_events()
    ));
    emit(&format!(
        "{path} CHECK refused: selection_evaluated={} selection_record_failures={:?} clock_advanced={} inputs_device={} input_events={}",
        refused.selection_evaluated().len(),
        refused.lifecycle_failures(true),
        refused.advanced,
        refused.inputs,
        refused.input_events()
    ));
    emit(&format!(
        "{path} CHECK primary unchanged: outcome {} / {} primary {} / {} other_lifecycle {:?} / {:?} task_failed {:?} / {:?} equal={}",
        normal.outcome,
        refused.outcome,
        normal.primary,
        refused.primary,
        normal.lifecycle_failures(false),
        refused.lifecycle_failures(false),
        normal_task_failed,
        refused_task_failed,
        normal.outcome == refused.outcome
            && normal.primary == refused.primary
            && normal.lifecycle_failures(false) == refused.lifecycle_failures(false)
            && normal_task_failed == refused_task_failed
    ));
}

#[test]
fn one_off_308g4_host_fatal_confirmation_capture() {
    let package = select_package();
    emit(&format!(
        "package sha256={:x} bytes={}",
        Sha256::digest(&package),
        package.len()
    ));
    for (path, run) in [
        ("manual", manual_run as fn(usize, bool) -> (Run, String)),
        ("scheduled", scheduled_run as fn(usize, bool) -> (Run, String)),
    ] {
        let Some((confirmation_capture, normal, closed)) = find_confirmation_capture(path, run)
        else {
            emit(&format!(
                "{path} no capture number between 2 and 4 failed the confirmation capture"
            ));
            continue;
        };
        normal.print(&format!("{path} normal"));
        emit(&format!("{path} normal host={closed}"));
        let (refused, closed) = run(confirmation_capture, true);
        refused.print(&format!("{path} refused"));
        emit(&format!("{path} refused host={closed}"));
        compare(path, &normal, &refused);
    }
}
