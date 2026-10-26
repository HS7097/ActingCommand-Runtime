// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401, slice DA-4 (model 392 v1.2 §4.1, §12.1 row DA-4; test row DA4-T3 of
//! §13, and the identity test every slice carries, §12.2 and C16): a game root's display
//! declaration `display-layout.json` in the Runtime. At startup it is read with the other root
//! files, and a broken one is only recorded. A run of the game reads it once, in its preparation:
//! a broken one refuses the run before any lease, task record or input; what the preparation read
//! holds for the whole run; a file replaced in place is read by the next run. A game root without
//! the file is not read for the run (that a translated map then fails is DA-2a's to assert). And
//! merge is deploy (Alice 10-26): a 1280x720 package on a 1280x720 device runs byte for byte as it
//! runs without a declaration and without a root.
//!
//! A synthetic `neutral` game, server `test`. The device instance is bound to a neutral loopback
//! endpoint; its frames show `home` (the first pixel red) until the run's one input and
//! `terminal` (blue) after it.

use super::*;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{RuntimeLifecyclePhase, RuntimePayload};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const GAME: &str = "neutral";

/// A valid declaration of the game.
fn declaration() -> Value {
    json!({
        "schema_version": "actingcommand.display-layout.v1",
        "game": GAME,
        "scale": {"by": "height", "area": "screen"},
        "anchor_area": "screen",
        "correction": {"radius_px": 16}
    })
}

/// The declaration with its correction radius above the bound.
fn radius_65() -> Value {
    let mut declaration = declaration();
    declaration["correction"]["radius_px"] = json!(65);
    declaration
}

/// The standard-pack root `<temporary>/packs/` and its game root `neutral/`: a `bundle.json`
/// listing one package no run here uses, and `display-layout.json` when declared.
struct PackRoot {
    root: PathBuf,
    game: PathBuf,
}

impl PackRoot {
    fn new(temp: &TempDir, declared: Option<&Value>) -> Self {
        let root = temp.path().join("packs");
        let game = root.join(GAME);
        fs::create_dir_all(&game).expect("game root");
        fs::write(
            game.join("bundle.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": "actingcommand.bundle.v4",
                "game": GAME,
                "packs": [{
                    "package_id": "neutral.test.listed",
                    "server": "test",
                    "entry_task_id": "task",
                    "path": "packs/neutral.test.listed",
                    "file_count": 3,
                    "byte_count": 64
                }]
            }))
            .expect("bundle JSON"),
        )
        .expect("write bundle.json");
        let pack_root = Self { root, game };
        if let Some(declared) = declared {
            pack_root.declare(declared);
        }
        pack_root
    }

    /// Writes `display-layout.json` in place.
    fn declare(&self, declared: &Value) {
        fs::write(
            self.game.join("display-layout.json"),
            serde_json::to_vec_pretty(declared).expect("declaration JSON"),
        )
        .expect("write display-layout.json");
    }
}

/// A host whose one instance is bound to a device endpoint whose frames and display are
/// `width`x`height` with rotation `r0` observed, reading the standard packs of `root`.
fn device_host(
    temp: &TempDir,
    root: Option<&PackRoot>,
    width: u32,
    height: u32,
) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((width, height));
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let registered = instance_id();
    let resolved = ResolvedExecutionInstance::new(registered, "127.0.0.1:16384").with_adb_endpoint(
        actingcommand_execution_kernel::ResolvedAdbEndpoint::new("127.0.0.1", 16384, false),
    );
    let host = RuntimeHost::start(
        config(temp).with_standard_packs_root(root.map(|root| root.root.clone())),
        Arc::new(
            FakeProvider::one(ALIAS, registered, Arc::clone(&state))
                .with_resolved_override(Arc::new(std::sync::Mutex::new(resolved))),
        ),
    )
    .expect("runtime host with a device instance");
    (host, state)
}

/// A navigable route package of the game whose resolution and coordinate spaces are
/// `width`x`height`: the page colour at (0, 0) and one trusted tap at (1, 0) from `home` to
/// `terminal`. Its timeouts leave room for a full-size frame.
fn package_at(width: u32, height: u32) -> Vec<u8> {
    let space = json!({"width": width, "height": height});
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.test.task",
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 20_000,
        "timeout_ms": 40_000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": GAME,
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    let recognition = json!({
        "schema_version": "0.3",
        "game": GAME,
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]}
        ]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [], "forbidden": []}
        ]
    });
    let manifest = json!({"schema_version": "0.3", "entry_task_id": "task"});
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in [
        ("control.json", control),
        ("resources/manifest.json", manifest),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("package JSON"))
            .expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

struct Run {
    receipt: RuntimeReceipt,
    events: Vec<PersistedEvent>,
}

impl Run {
    fn facts(&self) -> Vec<&TaskSemanticFact> {
        self.events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
                _ => None,
            })
            .collect()
    }

    fn succeeded(&self) -> bool {
        self.receipt.state() == RuntimeReceiptState::Completed
            && matches!(
                self.receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    final_page: Some(page),
                    ..
                }) if page == "neutral/terminal"
            )
    }

    /// The run's records as they compare across two hosts: each record's type and severity, and
    /// each task fact in full but for what differs between any two runs (the response deadline
    /// instant, the timing observations and the geometry's observation, which carries its read
    /// time).
    fn comparable(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| {
                let fact = match event.payload() {
                    EventPayload::Task(TaskPayload::Semantic(payload)) => {
                        let mut fact = serde_json::to_value(payload.fact()).expect("fact JSON");
                        if let Some(fact) = fact.as_object_mut() {
                            fact.remove("response_deadline_monotonic_ms");
                            fact.remove("task_timing");
                            if fact.get("kind") == Some(&json!("geometry_observed")) {
                                fact.remove("observation");
                            }
                        }
                        fact
                    }
                    _ => Value::Null,
                };
                format!("{:?} {:?} {fact}", event.event_type(), event.severity())
            })
            .collect()
    }
}

/// Runs `package` (written as `<name>.zip`) on the instance and reads its records back.
fn run(host: &RuntimeHost, temp: &TempDir, name: &str, package: &[u8]) -> Run {
    let path = temp.path().join(format!("{name}.zip"));
    fs::write(&path, package).expect("write run package");
    let task_request = ContainedTaskRequest::new(path.display().to_string()).expect("task request");
    let mut client = TestClient::connect(host);
    // A full-size frame is read, kept and recognised on every capture of an unoptimised build.
    client
        .stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("receipt timeout");
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

/// The outcomes of the `standard_pack_read` lifecycle records, in ledger order.
fn standard_pack_reads(host: &RuntimeHost) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeLifecycleObserved),
        ..EventQuery::default()
    })
    .expect("lifecycle events")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => match value.phase() {
            RuntimeLifecyclePhase::StandardPackRead { outcome } => Some(outcome.clone()),
            _ => None,
        },
        _ => None,
    })
    .collect()
}

fn text<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

/// `outcome` is `standard_pack_root_invalid` of the game's `display-layout.json` with
/// `problem` at `pointer`.
fn assert_declaration_refused(outcome: &Outcome, problem: &str, pointer: &str) {
    assert_eq!(outcome.code().as_str(), "standard_pack_root_invalid");
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(text(outcome, "game"), Some(GAME));
    assert!(
        text(outcome, "path").is_some_and(|path| path.ends_with("display-layout.json")),
        "{outcome:?}"
    );
    assert_eq!(text(outcome, "standard_pack_problem"), Some(problem));
    assert_eq!(text(outcome, "json_pointer"), Some(pointer));
}

/// The run was refused in its preparation with the declaration's problem: before any lease,
/// with no task record and no input.
fn assert_run_refused(run: &Run, state: &FakeState, problem: &str, pointer: &str) {
    assert_eq!(run.receipt.state(), RuntimeReceiptState::Denied);
    assert_declaration_refused(run.receipt.outcome(), problem, pointer);
    assert!(run.facts().is_empty(), "no task record: {:?}", run.facts());
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
}

/// DA4-T3: a broken declaration is recorded at startup and the Runtime starts; a run of its game
/// is refused in its preparation (`standard_pack_root_invalid`, `display_layout_invalid` at
/// `/correction/radius_px`) before any lease, task record or input; the declaration repaired in
/// place is read by the next run, which goes on.
#[test]
fn da4_t3_a_broken_declaration_refuses_the_run_before_its_lease_and_a_repaired_one_is_read_next() {
    let temp = TempDir::new().expect("tempdir");
    let root = PackRoot::new(&temp, Some(&radius_65()));
    let (host, state) = device_host(&temp, Some(&root), 16, 9);
    let reads = standard_pack_reads(&host);
    let [startup] = reads.as_slice() else {
        panic!("one record of the game root at startup: {reads:?}");
    };
    assert_declaration_refused(startup, "display_layout_invalid", "/correction/radius_px");

    let refused = run(&host, &temp, "refused", &package_at(16, 9));
    assert_run_refused(
        &refused,
        &state,
        "display_layout_invalid",
        "/correction/radius_px",
    );

    root.declare(&declaration());
    let repaired = run(&host, &temp, "repaired", &package_at(16, 9));
    assert!(repaired.succeeded(), "{:?}", repaired.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    host.close().expect("close runtime host");
}

/// DA4-T3: the declaration a run's preparation read holds for the whole run: replaced in place
/// by a broken file after the run's input, the run still goes on to its end; the next run
/// reads the broken file and is refused (`malformed` at its first missing member).
#[test]
fn da4_t3_the_declaration_read_in_preparation_holds_for_the_whole_run() {
    let temp = TempDir::new().expect("tempdir");
    let root = PackRoot::new(&temp, Some(&declaration()));
    let (host, state) = device_host(&temp, Some(&root), 16, 9);
    let reads = standard_pack_reads(&host);
    assert_eq!(
        reads
            .iter()
            .map(|outcome| outcome.code().as_str())
            .collect::<Vec<_>>(),
        ["standard_pack_registered"],
        "the sound game root at startup"
    );
    let declared = root.game.join("display-layout.json");
    // Every capture after the run's input, which only the run sends, finds the file replaced.
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            if inputs > 0 {
                fs::write(&declared, b"{}").expect("replace display-layout.json");
            }
            Vec::new()
        },
    ));
    let first = run(&host, &temp, "first", &package_at(16, 9));
    assert!(first.succeeded(), "{:?}", first.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(
        fs::read(root.game.join("display-layout.json")).expect("the replaced file"),
        b"{}",
        "the file was replaced during the run"
    );

    *state.pixel_script.lock().expect("pixel script") = None;
    let next = run(&host, &temp, "next", &package_at(16, 9));
    assert_eq!(next.receipt.state(), RuntimeReceiptState::Denied);
    assert_declaration_refused(next.receipt.outcome(), "malformed", "/schema_version");
    assert!(next.facts().is_empty(), "no task record");
    assert_eq!(state.input_count.load(Ordering::Acquire), 1, "no new input");
    host.close().expect("close runtime host");
}

/// The identity test (model 392 v1.2 §12.2, C16; Alice 10-26, merge is deploy): a 1280x720
/// package on a 1280x720 device records the same task records and sends the same input with a
/// game root that holds a valid declaration as with a game root without one and as with no
/// standard-pack root at all.
#[test]
fn da4_identity_a_1280x720_package_on_a_1280x720_device_runs_byte_for_byte_as_before() {
    let scenario = |declared: Option<Option<Value>>| {
        let temp = TempDir::new().expect("tempdir");
        let root = declared.map(|declared| PackRoot::new(&temp, declared.as_ref()));
        let (host, state) = device_host(&temp, root.as_ref(), 1280, 720);
        let run = run(&host, &temp, "run", &package_at(1280, 720));
        host.close().expect("close runtime host");
        assert!(run.succeeded(), "{:?}", run.receipt);
        assert_eq!(state.input_count.load(Ordering::Acquire), 1);
        let inputs = serde_json::to_value(&*state.input_actions.lock().expect("input actions"))
            .expect("inputs JSON");
        (run.comparable(), inputs)
    };
    let without_root = scenario(None);
    assert_eq!(
        scenario(Some(None)),
        without_root,
        "a game root without a declaration"
    );
    assert_eq!(
        scenario(Some(Some(declaration()))),
        without_root,
        "a game root with a declaration"
    );
}
