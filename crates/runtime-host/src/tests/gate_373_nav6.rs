// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-6 (model M-373 v1.1 §3.3, §3.6, §4.3, §6, §7, §8.3, §8.4, §9; test rows
//! N6-1 to N6-13 and N6-15; N6-14 is the kernel's, `contained_task/text_locate_tests.rs`): the
//! standard-pack root reader and the shared interrupt pages of a run.
//!
//! A synthetic `neutral` game. The fake device's first pixel is scripted: red is the run's page
//! `away`, blue (with the second pixel green) its page `home`. The overlays are shared
//! interrupt pages: `notice` turns the first pixel white, `alert` the third pixel white and
//! `banner` the second pixel white (both with the first pixel black). No page of the run's own
//! package matches an overlay. The game root's packages are source package directories.

use super::*;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{
    RuntimeLifecyclePhase, RuntimePayload, TaskEntryRecoveryRole, TaskInterruptThen,
};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

const FEATURES: &str = "neutral.test.features";
const CLOSE_NOTICE: &str = "neutral.test.close_notice";
const CLOSE_ALERT: &str = "neutral.test.close_alert";
const CLOSE_BANNER: &str = "neutral.test.close_banner";
const CLEANUP: &str = "neutral.test.cleanup";

/// An overlay over the first pixel `under` shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Notice,
    Alert,
    Banner,
}

impl Overlay {
    fn pixels(self) -> Vec<(usize, [u8; 3])> {
        match self {
            Self::Notice => vec![(0, WHITE)],
            Self::Alert => vec![(0, BLACK), (2, WHITE)],
            Self::Banner => vec![(0, BLACK), (1, WHITE)],
        }
    }
}

/// The first pixel `first` with `overlay` over it.
fn frame(first: [u8; 3], overlay: Option<Overlay>) -> Vec<(usize, [u8; 3])> {
    let mut pixels = vec![(0, first)];
    pixels.extend(overlay.map(Overlay::pixels).unwrap_or_default());
    pixels
}

fn script(
    state: &FakeState,
    script: impl Fn(usize, usize) -> Vec<(usize, [u8; 3])> + Send + Sync + 'static,
) {
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(script));
}

/// `overlay` until the device's input count passes `base`, then `away` until it passes
/// `base + 1`, then `home`.
fn overlay_then_away_then_home(state: &FakeState, base: usize, overlay: Overlay) {
    script(state, move |_, inputs| match inputs.saturating_sub(base) {
        0 => frame(RED, Some(overlay)),
        1 => frame(RED, None),
        _ => frame(BLUE, None),
    });
}

/// `overlay` on every capture but the first after each input, on which the run's `away`
/// shows; `kind` chooses the overlay from the input count.
fn recurring_overlay(state: &FakeState, kind: impl Fn(usize) -> Overlay + Send + Sync + 'static) {
    let seen = std::sync::Mutex::new((0_usize, 0_usize));
    script(state, move |capture, inputs| {
        let mut seen = seen.lock().expect("seen input");
        if seen.0 != inputs {
            *seen = (inputs, capture);
        }
        if inputs > 0 && seen.1 == capture {
            frame(RED, None)
        } else {
            frame(RED, Some(kind(inputs)))
        }
    });
}

// ---- The run's own package: an assembled package with the device's pixels as its pages.

const AWAY: &[&str] = &["pixel/first_red"];
const HOME: &[&str] = &["pixel/first_blue", "pixel/second_green"];
const OWN_NOTICE: &[&str] = &["pixel/first_white"];

fn operation(id: &str, from: &str, to: &str) -> Value {
    json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    })
}

/// A `navigable_route` package around `task` (control v2 for task schema 0.9, v1 otherwise).
fn run_package(task: Value, pages: &[(&str, &[&str])]) -> Vec<u8> {
    let control = json!({
        "schema_version": if task["schema_version"] == "0.9" {
            "Lab-1y.control.v2"
        } else {
            "Lab-1y.control.v1"
        },
        "package_id": "neutral.test.run",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 4
    });
    let recognition = json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "pixel/first_red", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "pixel/first_blue", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "pixel/first_white", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 255]},
            {"type": "color", "id": "pixel/second_green", "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}
        ]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": pages
            .iter()
            .map(|(page, required)| json!({
                "id": format!("neutral/{page}"),
                "required": required,
                "optional": [],
                "forbidden": []
            }))
            .collect::<Vec<_>>()
    });
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// From `away` to `home`, with no entry check.
fn away_to_home() -> Vec<u8> {
    run_package(
        json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 16, "height": 9},
            "entry_page": "away",
            "target_page": "home",
            "operations": [operation("go", "away", "home")]
        }),
        &[("away", AWAY), ("home", HOME)],
    )
}

/// From `away` to `home` with the entry set `{away}`, which the host checks first; `own_notice`
/// also declares the page `notice`, which the package leaves from itself.
fn entry_away_to_home(own_notice: bool) -> Vec<u8> {
    let mut operations = vec![operation("go", "away", "home")];
    let mut pages: Vec<(&str, &[&str])> = vec![("away", AWAY), ("home", HOME)];
    if own_notice {
        operations.push(operation("dismiss", "notice", "away"));
        pages.push(("notice", OWN_NOTICE));
    }
    run_package(
        json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 16, "height": 9},
            "entry_page": ["away"],
            "target_page": "home",
            "operations": operations
        }),
        &pages,
    )
}

// ---- The game root: source package directories, `bundle.json`, `pages.json`, `interrupts.json`.

/// One colour probe of a source task by its id.
fn probe(id: &str) -> Value {
    let (x, expected) = match id {
        "first_red" => (0, RED),
        "first_blue" => (0, BLUE),
        "first_white" => (0, WHITE),
        "first_black" => (0, BLACK),
        "second_green" => (1, [0, 255, 0]),
        "second_white" => (1, WHITE),
        "third_white" => (2, WHITE),
        other => panic!("no probe {other}"),
    };
    json!({
        "id": id,
        "region": {"mode": "rect", "rect": {"x": x, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// A source task: its pages by their probes, its entry and target page and its operations
/// (`post_delay_ms` on each when given).
fn source_task(
    rules: &[(&str, &[&str])],
    entry: &str,
    target: &str,
    operations: &[(&str, &str, &str)],
    post_delay_ms: Option<u64>,
    width: u32,
) -> Value {
    let probes = rules
        .iter()
        .flat_map(|(_, probes)| probes.iter().copied())
        .collect::<BTreeSet<_>>();
    let page_rules = rules
        .iter()
        .map(|(page, probes)| ((*page).to_owned(), json!({"required": probes})))
        .collect::<serde_json::Map<_, _>>();
    let height = width * 9 / 16;
    json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "neutral interrupt fixture",
        "coordinate_space": {"width": width, "height": height},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": probes.into_iter().map(probe).collect::<Vec<_>>(),
        "page_rules": page_rules,
        "entry_page": entry,
        "target_page": target,
        "operations": operations
            .iter()
            .map(|(id, from, to)| {
                let mut operation = operation(id, from, to);
                if let Some(delay) = post_delay_ms {
                    operation["post_delay_ms"] = json!(delay);
                }
                operation
            })
            .collect::<Vec<_>>()
    })
}

/// The feature package: the four pages of `pages.json`.
fn features_task() -> Value {
    source_task(
        &[
            ("home", &["first_blue", "second_green"]),
            ("notice", &["first_white"]),
            ("alert", &["third_white"]),
            ("banner", &["second_white"]),
        ],
        "home",
        "home",
        &[
            ("see_notice", "notice", "home"),
            ("see_alert", "alert", "home"),
            ("see_banner", "banner", "home"),
        ],
        None,
        16,
    )
}

/// A handler of the interrupt page `page` (its probe `probe`) that leaves it to `target`.
fn handler_task(page: &str, probe: &'static str, target: (&str, &'static [&'static str])) -> Value {
    source_task(
        &[(page, &[probe]), target],
        page,
        target.0,
        &[("close", page, target.0)],
        None,
        16,
    )
}

const TO_AWAY: (&str, &[&str]) = ("away", &["first_red"]);

struct GameRoot {
    root: PathBuf,
    game: PathBuf,
    packs: Vec<String>,
}

impl GameRoot {
    fn new(base: &Path) -> Self {
        let root = base.join("packages");
        let game = root.join("neutral");
        fs::create_dir_all(&game).expect("game root");
        Self {
            root,
            game,
            packs: Vec::new(),
        }
    }

    fn directory(index: usize) -> String {
        format!("{:064x}", index + 1)
    }

    /// A listed package whose directory holds `task` (16x9 unless `width` says otherwise).
    fn pack(&mut self, package_id: &str, mode: &str, task: Value) -> &mut Self {
        let width = task["coordinate_space"]["width"].as_u64().expect("width");
        let height = task["coordinate_space"]["height"].as_u64().expect("height");
        let directory = self.game.join(Self::directory(self.packs.len()));
        fs::create_dir_all(directory.join("resources/operations/task")).expect("package dirs");
        let control = json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": package_id,
            "execution_mode": mode,
            "game": "neutral",
            "server": "test",
            "resolution": {"width": width, "height": height},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 1000,
            "timeout_ms": 5000,
            "max_steps": 2
        });
        for (path, value) in [
            ("control.json", control),
            (
                "resources/operations/resources.json",
                json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
            ),
            ("resources/operations/task/task.json", task),
        ] {
            fs::write(
                directory.join(path),
                serde_json::to_vec_pretty(&value).expect("package file"),
            )
            .expect("write package file");
        }
        self.packs.push(package_id.to_owned());
        self
    }

    /// A package `bundle.json` lists whose directory is not there.
    fn listed_only(&mut self, package_id: &str) -> &mut Self {
        self.packs.push(package_id.to_owned());
        self
    }

    fn features(&mut self) -> &mut Self {
        self.pack(FEATURES, "recognize_only", features_task())
    }

    fn write(&self, file: &str, value: &Value) {
        fs::write(
            self.game.join(file),
            serde_json::to_vec_pretty(value).expect("root file"),
        )
        .expect("write root file");
    }

    fn bundle(&self) -> &Self {
        self.write(
            "bundle.json",
            &json!({
                "schema_version": "actingcommand.bundle.v2",
                "game": "neutral",
                "packs": self
                    .packs
                    .iter()
                    .enumerate()
                    .map(|(index, package_id)| json!({
                        "package_id": package_id,
                        "server": "test",
                        "entry_task_id": "task",
                        "digest": Self::directory(index),
                        "path": format!("packs/{}", Self::directory(index)),
                        "file_count": 3,
                        "byte_count": 64
                    }))
                    .collect::<Vec<_>>()
            }),
        );
        self
    }

    fn pages(&self) -> &Self {
        self.write(
            "pages.json",
            &json!({
                "schema_version": "actingcommand.pages.v1",
                "game": "neutral",
                "features": [FEATURES],
                "pages": [
                    {"id": "home", "kind": "page"},
                    {"id": "notice", "kind": "overlay"},
                    {"id": "alert", "kind": "overlay"},
                    {"id": "banner", "kind": "overlay"}
                ]
            }),
        );
        self
    }

    fn interrupts(&self, interrupts: Value) -> &Self {
        self.write(
            "interrupts.json",
            &json!({
                "schema_version": "actingcommand.interrupts.v1",
                "game": "neutral",
                "interrupts": interrupts
            }),
        );
        self
    }
}

fn interrupt(page: &str, handler: &str, then: Value, max_per_run: u8) -> Value {
    json!({"page": page, "handlers": [handler], "then": then, "max_per_run": max_per_run})
}

/// A game root with the feature package, `close_notice` (to `away`) and its notice interrupt.
fn notice_root(base: &Path, then: Value, max_per_run: u8) -> GameRoot {
    let mut root = GameRoot::new(base);
    root.features()
        .pack(
            CLOSE_NOTICE,
            "navigable_route",
            handler_task("notice", "first_white", TO_AWAY),
        )
        .bundle()
        .pages()
        .interrupts(json!([interrupt(
            "notice",
            CLOSE_NOTICE,
            then,
            max_per_run
        )]));
    root
}

// ---- Running.

fn start(temp: &TempDir, state: &Arc<FakeState>, root: Option<&GameRoot>) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        config(temp).with_standard_packs_root(root.map(|root| root.root.clone())),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host")
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

    /// The run's terminal outcome: a failed run's failure.
    fn failure(&self) -> Outcome {
        self.facts()
            .into_iter()
            .find_map(|fact| match fact {
                TaskSemanticFact::TerminalCommitted {
                    outcome: Some(outcome),
                    ..
                } => Some(outcome.clone()),
                _ => None,
            })
            .expect("a failed terminal")
    }

    fn handled(&self) -> Vec<(String, String, TaskInterruptThen, u8, Outcome)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::InterruptHandled {
                    page,
                    handler_package_label,
                    then,
                    count,
                    outcome,
                } => Some((
                    page.clone(),
                    handler_package_label.clone(),
                    then.clone(),
                    *count,
                    outcome.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    /// The `role: interrupt_handler` layers: each completed one's final page, `None` for a
    /// failed one with its failure.
    fn handler_layers(&self) -> (usize, Vec<String>, Vec<Outcome>) {
        let role = Some(TaskEntryRecoveryRole::InterruptHandler);
        let facts = self.facts();
        let admitted = facts
            .iter()
            .filter(|fact| {
                matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { role: admitted, .. } if *admitted == role)
            })
            .count();
        let completed = facts
            .iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryCompleted {
                    final_page,
                    role: completed,
                    ..
                } if *completed == role => Some(final_page.clone()),
                _ => None,
            })
            .collect();
        let failed = facts
            .iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryFailed {
                    outcome,
                    role: failed,
                    ..
                } if *failed == role => Some(outcome.clone()),
                _ => None,
            })
            .collect();
        (admitted, completed, failed)
    }

    fn succeeded(&self) -> bool {
        self.receipt.state() == RuntimeReceiptState::Completed
            && matches!(
                self.receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    ..
                })
            )
    }
}

fn run(host: &RuntimeHost, temp: &TempDir, name: &str, target: &[u8]) -> Run {
    let path = temp.path().join(format!("{name}.zip"));
    fs::write(&path, target).expect("write run package");
    let task_request = ContainedTaskRequest::new(
        path.display().to_string(),
        format!("{:x}", Sha256::digest(target)),
    )
    .expect("task request");
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

fn integer(outcome: &Outcome, key: &str) -> Option<u64> {
    outcome.value(key).and_then(Value::as_u64)
}

fn strings(outcome: &Outcome, key: &str) -> Vec<String> {
    outcome
        .value(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---- N6-1, N6-2: the reader.

/// N6-1 (with the contract's table, `standard_pack.rs`): without `standard_packs.root` nothing
/// is read; a root that cannot be listed is recorded as `standard_pack_root_unreadable`; a
/// broken game root a run needs refuses the run in preparation with
/// `standard_pack_root_invalid`, before any lease and any task record.
#[test]
fn n6_1_the_root_reader_records_its_problems_and_refuses_a_run_that_needs_a_broken_root() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let host = start(&temp, &state, None);
    assert!(
        standard_pack_reads(&host).is_empty(),
        "no root, no registry"
    );
    host.close().expect("close runtime host");

    let temp = TempDir::new().expect("tempdir");
    let missing = GameRoot {
        root: temp.path().join("absent"),
        game: temp.path().join("absent/neutral"),
        packs: Vec::new(),
    };
    let host = start(&temp, &state, Some(&missing));
    let reads = standard_pack_reads(&host);
    let [unreadable] = reads.as_slice() else {
        panic!("one record of the root: {reads:?}");
    };
    assert_eq!(unreadable.code().as_str(), "standard_pack_root_unreadable");
    assert_eq!(unreadable.category(), Category::Error);
    assert!(text(unreadable, "path").is_some_and(|path| path.ends_with("absent")));
    assert!(unreadable.value("game").is_none());
    assert_eq!(
        unreadable
            .causes()
            .iter()
            .map(|link| link.outcome().code().as_str())
            .collect::<Vec<_>>(),
        ["foreign_os_error"]
    );
    host.close().expect("close runtime host");

    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("resume"), 0);
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &away_to_home());
    assert_eq!(run.receipt.state(), RuntimeReceiptState::Denied);
    assert!(
        run.receipt.error_projection().is_some(),
        "refusal projection"
    );
    // Receipt v2 (Workflow #378 A4): the refusal is the top of the receipt's outcome.
    assert_eq!(
        run.receipt.outcome().code().as_str(),
        "standard_pack_root_invalid"
    );
    assert!(run.facts().is_empty(), "no task record");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let reads = standard_pack_reads(&host);
    let [invalid] = reads.as_slice() else {
        panic!("one record of the game root: {reads:?}");
    };
    assert_eq!(invalid.code().as_str(), "standard_pack_root_invalid");
    assert_eq!(invalid.category(), Category::Error);
    assert_eq!(text(invalid, "game"), Some("neutral"));
    assert!(text(invalid, "path").is_some_and(|path| path.ends_with("interrupts.json")));
    assert_eq!(
        text(invalid, "standard_pack_problem"),
        Some("max_per_run_invalid")
    );
    assert_eq!(
        text(invalid, "json_pointer"),
        Some("/interrupts/0/max_per_run")
    );
    host.close().expect("close runtime host");
}

/// N6-2: two game roots, one valid and one broken: the Runtime starts as usual, records the
/// valid one as `standard_pack_registered` and the broken one as `standard_pack_root_invalid`,
/// and a run of the valid game runs (check-config's half is in actingd's process tests).
#[test]
fn n6_2_a_broken_game_root_never_stops_the_runtime() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("resume"), 1);
    let other = root.root.join("other");
    fs::create_dir_all(&other).expect("other game root");
    fs::write(
        other.join("bundle.json"),
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.bundle.v2",
            "game": "other",
            "packs": [{"package_id": "other.test.task", "server": "test", "entry_task_id": "task",
                       "digest": format!("{:064x}", 9), "path": format!("packs/{:064x}", 9),
                       "file_count": 1, "byte_count": 1}]
        }))
        .expect("bundle JSON"),
    )
    .expect("write bundle");
    fs::write(
        other.join("interrupts.json"),
        br#"{"schema_version":"actingcommand.interrupts.v1","game":"other","interrupts":[]}"#,
    )
    .expect("write interrupts");
    let host = start(&temp, &state, Some(&root));
    let reads = standard_pack_reads(&host);
    let [registered, invalid] = reads.as_slice() else {
        panic!("one record per game root: {reads:?}");
    };
    assert_eq!(registered.code().as_str(), "standard_pack_registered");
    assert_eq!(registered.category(), Category::Info);
    assert_eq!(text(registered, "game"), Some("neutral"));
    assert_eq!(integer(registered, "count"), Some(2));
    assert_eq!(
        strings(registered, "added_package_ids"),
        [CLOSE_NOTICE, FEATURES]
    );
    assert_eq!(invalid.code().as_str(), "standard_pack_root_invalid");
    assert_eq!(text(invalid, "game"), Some("other"));
    assert_eq!(
        text(invalid, "standard_pack_problem"),
        Some("pages_missing")
    );
    assert_eq!(text(invalid, "json_pointer"), Some(""));

    script(&state, |_, inputs| {
        frame(if inputs == 0 { RED } else { BLUE }, None)
    });
    let run = run(&host, &temp, "run", &away_to_home());
    assert!(run.succeeded(), "{:?}", run.receipt);
    host.close().expect("close runtime host");
}

// ---- N6-3 to N6-12, N6-15: the shared interrupt pages.

/// N6-3: the run waits for its step's destination when a notice covers it: the handler runs as
/// a layer of the run (`role: interrupt_handler`), and the step goes on waiting with its own
/// budget: the handling (its 800 ms `post_delay_ms` alone is longer than the step's 600 ms
/// budget) is not counted, so the destination that shows later than the budget after the step's
/// input still confirms the step, after frames of the step's own wait that show no page of the
/// run.
///
/// The step's budget is wall-clock time and only the handling is paused, so every frame of the
/// step's own wait (each one captured, stored, recorded and checked against the interrupt pages)
/// spends it. The wait therefore sees a few such frames, not many: with ten, a loaded CI runner
/// spent the 600 ms on them alone and the step ended `contained_task_step_unconfirmed` (CI run
/// 38603013861), although nothing of the handling was counted.
#[test]
fn n6_3_a_notice_during_a_wait_is_handled_and_the_wait_keeps_its_own_budget() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let mut root = GameRoot::new(temp.path());
    root.features()
        .pack(
            CLOSE_NOTICE,
            "navigable_route",
            source_task(
                &[("notice", &["first_white"]), ("dim", &["first_black"])],
                "notice",
                "dim",
                &[("close", "notice", "dim")],
                Some(800),
                16,
            ),
        )
        .bundle()
        .pages()
        .interrupts(json!([interrupt(
            "notice",
            CLOSE_NOTICE,
            json!("resume"),
            1
        )]));
    // After the run's input the notice covers the page; after the handler's input the page is
    // dark (the handler's destination) for three captures, the handler's own frame and then the
    // step's own wait, then `home` shows. Captures are counted, the clock is not read (test plan
    // v2.2 §2.1 guard 3).
    let dark_from = std::sync::Mutex::new(None::<usize>);
    script(&state, move |capture, inputs| match inputs {
        0 => frame(RED, None),
        1 => frame(RED, Some(Overlay::Notice)),
        _ => {
            let first = *dark_from.lock().expect("dark from").get_or_insert(capture);
            if capture < first + 3 {
                frame(BLACK, None)
            } else {
                frame(BLUE, None)
            }
        }
    });
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &away_to_home());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    let handled = run.handled();
    let [(page, handler, then, count, outcome)] = handled.as_slice() else {
        panic!("one handled interrupt: {handled:?}");
    };
    assert_eq!(page, "notice");
    assert_eq!(handler, CLOSE_NOTICE);
    assert_eq!(*then, TaskInterruptThen::Resume);
    assert_eq!(*count, 1);
    assert_eq!(outcome.code().as_str(), "contained_task_interrupt_handled");
    assert_eq!(outcome.category(), Category::Info);
    assert_eq!(text(outcome, "page_id"), Some("notice"));
    assert_eq!(integer(outcome, "count"), Some(1));
    let (admitted, completed, failed) = run.handler_layers();
    assert_eq!(admitted, 1);
    assert_eq!(completed, ["neutral/dim"]);
    assert!(failed.is_empty());
    // The step went on waiting after the handling: its own frames after the handled record show
    // no page of the run (the handler's dark destination) before the first one on `home`.
    let after_handling = run
        .facts()
        .into_iter()
        .skip_while(|fact| !matches!(fact, TaskSemanticFact::InterruptHandled { .. }))
        .filter_map(|fact| match fact {
            TaskSemanticFact::RecognitionCompleted { matched_page, .. } => {
                Some(matched_page.as_deref())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let waited = after_handling
        .iter()
        .take_while(|page| page.is_none())
        .count();
    assert!(waited >= 1, "{after_handling:?}");
    assert_eq!(
        after_handling.get(waited),
        Some(&Some("neutral/home")),
        "{after_handling:?}"
    );
}

/// N6-4: no page of the page-graph run matches (it would end with `contained_task_page_unknown`)
/// because a notice covers the screen: the notice is handled and the run goes on by its pages.
#[test]
fn n6_4_an_unknown_page_that_is_a_notice_is_handled_and_the_run_goes_on() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("resume"), 1);
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &away_to_home());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    assert_eq!(run.handled().len(), 1);
    assert_eq!(run.handler_layers().1, ["neutral/away"]);
}

/// N6-5: the entry's first check ends unmatched on a notice: the notice is handled on that
/// frame, the first check is made again and passes, and the run starts with no prerequisite or
/// recovery package.
#[test]
fn n6_5_an_entry_check_unmatched_on_a_notice_handles_it_and_checks_again() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("resume"), 1);
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &entry_away_to_home(false));
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    assert_eq!(run.handled().len(), 1);
    let facts = run.facts();
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::EntryRecoveryPackageAdmitted { role: None, .. }
        )),
        "no prerequisite or recovery package ran"
    );
    let recognitions = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecognition { matched_page, .. } => Some(matched_page.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!recognitions.is_empty());
    assert!(
        recognitions
            .iter()
            .all(|matched| matched.as_deref() == Some("neutral/away")),
        "{recognitions:?}"
    );
}

/// N6-6: a package that declares the interrupt page itself keeps it: its entry check ends
/// unmatched on the notice and nothing intercepts it (no handler, no input); the run ends as it
/// would without shared interrupt pages.
#[test]
fn n6_6_a_page_the_package_declares_itself_is_not_intercepted() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("resume"), 1);
    script(&state, |_, _| frame(RED, Some(Overlay::Notice)));
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &entry_away_to_home(true));
    host.close().expect("close runtime host");

    assert_eq!(run.receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        run.failure().code().as_str(),
        "contained_task_home_recovery_binding_missing"
    );
    assert!(run.handled().is_empty());
    assert_eq!(run.handler_layers().0, 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
}

/// N6-7: a page seen once more than its `max_per_run` ends the run with
/// `contained_task_interrupt_limit_reached`, and so does the 17th handling of a run whatever
/// the pages; nothing is input after either.
#[test]
fn n6_7_handling_beyond_max_per_run_or_sixteen_in_a_run_ends_the_run() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("resume"), 1);
    recurring_overlay(&state, |_| Overlay::Notice);
    let host = start(&temp, &state, Some(&root));
    let per_page = run(&host, &temp, "per_page", &away_to_home());
    host.close().expect("close runtime host");

    assert_eq!(per_page.receipt.state(), RuntimeReceiptState::Failed);
    let failure = per_page.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_interrupt_limit_reached"
    );
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(text(&failure, "page_id"), Some("notice"));
    assert_eq!(integer(&failure, "count"), Some(2));
    assert_eq!(integer(&failure, "limit_count"), Some(1));
    assert_eq!(per_page.handled().len(), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);

    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let mut root = GameRoot::new(temp.path());
    root.features()
        .pack(
            CLOSE_NOTICE,
            "navigable_route",
            handler_task("notice", "first_white", TO_AWAY),
        )
        .pack(
            CLOSE_ALERT,
            "navigable_route",
            handler_task("alert", "third_white", TO_AWAY),
        )
        .pack(
            CLOSE_BANNER,
            "navigable_route",
            handler_task("banner", "second_white", TO_AWAY),
        )
        .bundle()
        .pages()
        .interrupts(json!([
            interrupt("notice", CLOSE_NOTICE, json!("resume"), 8),
            interrupt("alert", CLOSE_ALERT, json!("resume"), 8),
            interrupt("banner", CLOSE_BANNER, json!("resume"), 8)
        ]));
    recurring_overlay(&state, |inputs| match inputs % 3 {
        0 => Overlay::Notice,
        1 => Overlay::Alert,
        _ => Overlay::Banner,
    });
    let host = start(&temp, &state, Some(&root));
    let total = run(&host, &temp, "total", &away_to_home());
    host.close().expect("close runtime host");

    assert_eq!(total.receipt.state(), RuntimeReceiptState::Failed);
    let failure = total.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_interrupt_limit_reached"
    );
    assert_eq!(text(&failure, "page_id"), Some("alert"));
    assert_eq!(integer(&failure, "count"), Some(17));
    assert_eq!(integer(&failure, "limit_count"), Some(16));
    assert_eq!(total.handled().len(), 16);
    assert_eq!(state.input_count.load(Ordering::Acquire), 16);
}

/// N6-8: one frame matches two interrupt pages: an `interrupt_ambiguous` record, nothing is
/// handled, and the run goes on as it would.
#[test]
fn n6_8_two_interrupt_pages_on_one_frame_are_recorded_and_none_is_handled() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let mut root = GameRoot::new(temp.path());
    root.features()
        .pack(
            CLOSE_NOTICE,
            "navigable_route",
            handler_task("notice", "first_white", TO_AWAY),
        )
        .pack(
            CLOSE_ALERT,
            "navigable_route",
            handler_task("alert", "third_white", TO_AWAY),
        )
        .bundle()
        .pages()
        .interrupts(json!([
            interrupt("notice", CLOSE_NOTICE, json!("resume"), 1),
            interrupt("alert", CLOSE_ALERT, json!("resume"), 1)
        ]));
    script(&state, |capture, inputs| match (capture, inputs) {
        (1, _) => vec![(0, WHITE), (2, WHITE)],
        (_, 0) => frame(RED, None),
        _ => frame(BLUE, None),
    });
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &away_to_home());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert!(run.handled().is_empty());
    let ambiguous = run
        .facts()
        .into_iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::InterruptAmbiguous { page_ids, outcome } => {
                Some((page_ids.clone(), outcome.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(page_ids, outcome)] = ambiguous.as_slice() else {
        panic!("one ambiguous frame: {ambiguous:?}");
    };
    assert_eq!(page_ids, &["notice", "alert"]);
    assert_eq!(
        outcome.code().as_str(),
        "contained_task_interrupt_ambiguous"
    );
    assert_eq!(outcome.category(), Category::Warning);
    assert_eq!(strings(outcome, "page_ids"), ["notice", "alert"]);
}

/// N6-9: `then: end_run`: after its handler the run ends with
/// `contained_task_interrupt_escalated`; a direct run starts no recovery ladder (the default
/// ladder trigger of a routine run is D1c's).
#[test]
fn n6_9_end_run_ends_the_run_escalated_after_its_handler() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("end_run"), 1);
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &away_to_home());
    let ladders = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeLifecycleObserved),
            ..EventQuery::default()
        })
        .expect("lifecycle events")
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value))
                    if matches!(value.phase(), RuntimeLifecyclePhase::RecoveryLadderStarted { .. })
            )
        })
        .count();
    host.close().expect("close runtime host");

    assert_eq!(run.receipt.state(), RuntimeReceiptState::Failed);
    let failure = run.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_interrupt_escalated"
    );
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(text(&failure, "page_id"), Some("notice"));
    let handled = run.handled();
    let [(_, _, then, 1, _)] = handled.as_slice() else {
        panic!("one handled interrupt: {handled:?}");
    };
    assert_eq!(*then, TaskInterruptThen::EndRun);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(ladders, 0, "a direct run starts no ladder");
}

/// N6-9, the ladder half NAV-6 left to Workflow #387 D1c (coordinator ruling 2): a scheduled
/// run that ends `end_run` ends with `contained_task_interrupt_escalated`, class
/// `pack_logic_mismatch`, so its failed lease end starts the ladder by the default trigger item
/// `pack_logic` (M387 §3.3, §7.1; M373 §9.4).
#[test]
fn n6_9_a_scheduled_run_that_ends_end_run_starts_the_ladder_by_pack_logic() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("end_run"), 1);
    let (host_config, request) =
        super::instance_workers::ladder_setup_with_package(temp.path(), &away_to_home());
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config.with_standard_packs_root(Some(root.root.clone())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let context = super::instance_workers::admit_ladder_run(&host, &request);
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{}", error.code());
    super::instance_workers::wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let triggers = super::instance_workers::phases(&events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. } => Some(trigger),
            _ => None,
        })
        .collect::<Vec<_>>();
    host.close().expect("close runtime host");
    let [trigger] = triggers.as_slice() else {
        panic!("one ladder: {triggers:?}");
    };
    assert_eq!(
        trigger.stage,
        actingcommand_contract::RecoveryTriggerStage::Task
    );
    assert!(trigger.run_id.is_some(), "the failed run's ladder");
    assert_eq!(
        trigger.failure_code.as_str(),
        "contained_task_interrupt_escalated"
    );
    assert_eq!(trigger.rule.as_deref(), Some("pack_logic"));
    assert_eq!(
        trigger.matched,
        Some(actingcommand_contract::RecoveryTriggerMatch::Class {
            ladder_class: actingcommand_contract::outcome::LadderClass::PackLogicMismatch
        })
    );
}

/// N6-10: `then: {run}`: after the handler the named package runs as a layer too, and then the
/// run checks its page again.
#[test]
fn n6_10_then_run_runs_its_package_after_the_handler_and_resumes() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let mut root = GameRoot::new(temp.path());
    root.features()
        .pack(
            CLOSE_NOTICE,
            "navigable_route",
            handler_task("notice", "first_white", TO_AWAY),
        )
        .pack(
            CLEANUP,
            "navigable_route",
            source_task(
                &[
                    ("away", &["first_red"]),
                    ("home", &["first_blue", "second_green"]),
                ],
                "away",
                "home",
                &[("tidy", "away", "home")],
                None,
                16,
            ),
        )
        .bundle()
        .pages()
        .interrupts(json!([interrupt(
            "notice",
            CLOSE_NOTICE,
            json!({"run": [CLEANUP]}),
            1
        )]));
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &away_to_home());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    let handled = run.handled();
    let [(_, _, then, 1, _)] = handled.as_slice() else {
        panic!("one handled interrupt: {handled:?}");
    };
    assert_eq!(*then, TaskInterruptThen::Run(CLEANUP.to_owned()));
    let (admitted, completed, _) = run.handler_layers();
    assert_eq!(admitted, 2);
    assert_eq!(completed, ["neutral/away", "neutral/home"]);
}

/// N6-11 (M-373 §6.2 item 3, §6.3): a handler that fails ends the run with its own code,
/// recorded as the layer's `EntryRecoveryFailed` with `role: interrupt_handler`. The notice
/// never leaves: the handler's one step (`retryable: false`) sees its own page `notice` again
/// instead of its destination, which is the handler's `contained_task_step_unconfirmed`. The run
/// itself never started a step: without the notice it would have ended on its unknown page
/// (`contained_task_page_unknown`), and its only input is the handler's.
#[test]
fn n6_11_a_failed_handler_ends_the_run_with_its_own_code() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = notice_root(temp.path(), json!("resume"), 1);
    script(&state, |_, _| frame(RED, Some(Overlay::Notice)));
    let host = start(&temp, &state, Some(&root));
    let run = run(&host, &temp, "run", &away_to_home());
    host.close().expect("close runtime host");

    assert_eq!(run.receipt.state(), RuntimeReceiptState::Failed);
    let (admitted, completed, failed) = run.handler_layers();
    assert_eq!(admitted, 1);
    assert!(completed.is_empty());
    let [handler_failure] = failed.as_slice() else {
        panic!("one failed handler layer: {failed:?}");
    };
    assert_eq!(
        handler_failure.code().as_str(),
        "contained_task_step_unconfirmed"
    );
    assert_eq!(
        run.failure().code(),
        handler_failure.code(),
        "the run ends with the handler's own code"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert!(run.handled().is_empty());
}

/// How an N6-12 case lays out the handler `close_notice` in the game root.
type ArrangeHandler = Box<dyn Fn(&mut GameRoot)>;

/// N6-12: a handler that cannot be admitted, is not eligible as a layer of the run, or whose
/// entry does not hold its page ends the run with `contained_task_navigation_package_invalid`
/// (`navigation_role: handler`) before any input.
#[test]
fn n6_12_a_handler_unfit_for_its_part_ends_the_run_before_any_input() {
    let cases: [(&str, ArrangeHandler, &str); 4] = [
        (
            "listed, not installed",
            Box::new(|root: &mut GameRoot| {
                root.listed_only(CLOSE_NOTICE);
            }),
            "admission_failed",
        ),
        (
            "recognize only",
            Box::new(|root: &mut GameRoot| {
                root.pack(
                    CLOSE_NOTICE,
                    "recognize_only",
                    handler_task("notice", "first_white", TO_AWAY),
                );
            }),
            "role_ineligible",
        ),
        (
            "another resolution",
            Box::new(|root: &mut GameRoot| {
                root.pack(
                    CLOSE_NOTICE,
                    "navigable_route",
                    source_task(
                        &[("notice", &["first_white"]), TO_AWAY],
                        "notice",
                        "away",
                        &[("close", "notice", "away")],
                        None,
                        32,
                    ),
                );
            }),
            "identity_mismatch",
        ),
        (
            "an entry without the page",
            Box::new(|root: &mut GameRoot| {
                root.pack(
                    CLOSE_NOTICE,
                    "navigable_route",
                    source_task(
                        &[("notice", &["first_white"]), TO_AWAY],
                        "away",
                        "away",
                        &[("close", "notice", "away")],
                        None,
                        16,
                    ),
                );
            }),
            "entry_mismatch",
        ),
    ];
    for (case, handler, problem) in cases {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let mut root = GameRoot::new(temp.path());
        root.features();
        handler(&mut root);
        root.bundle().pages().interrupts(json!([interrupt(
            "notice",
            CLOSE_NOTICE,
            json!("resume"),
            1
        )]));
        script(&state, |_, _| frame(RED, Some(Overlay::Notice)));
        let host = start(&temp, &state, Some(&root));
        let run = run(&host, &temp, "run", &away_to_home());
        host.close().expect("close runtime host");

        assert_eq!(run.receipt.state(), RuntimeReceiptState::Failed, "{case}");
        let failure = run.failure();
        assert_eq!(
            failure.code().as_str(),
            "contained_task_navigation_package_invalid",
            "{case}"
        );
        assert_eq!(failure.category(), Category::Error, "{case}");
        assert_eq!(text(&failure, "package_id"), Some(CLOSE_NOTICE), "{case}");
        assert_eq!(text(&failure, "navigation_role"), Some("handler"), "{case}");
        assert_eq!(
            text(&failure, "navigation_problem"),
            Some(problem),
            "{case}"
        );
        assert_eq!(
            failure.causes().is_empty(),
            problem != "admission_failed",
            "{case}: the loader's refusal is the cause of an admission failure"
        );
        assert_eq!(state.input_count.load(Ordering::Acquire), 0, "{case}");
        assert!(run.handled().is_empty(), "{case}");
    }
}

// ---- N6-13: without `interrupts.json` nothing changes.

/// A run's records as they compare across two hosts: each record's type and severity, and each
/// task fact in full but for what differs between any two runs (the response deadline instant,
/// the timing observations and the geometry's frame).
fn comparable(run: &Run) -> Vec<String> {
    run.events
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

/// N6-13: a game root without `interrupts.json` (its `bundle.json` and `pages.json` there) leaves
/// a run's records exactly as without a root: a run that passes an unknown frame and succeeds,
/// and a run that fails on a capture.
#[test]
fn n6_13_without_interrupts_a_run_records_what_it_records_without_a_root() {
    let scenario = |root: bool, failing: bool| {
        let state = Arc::new(FakeState::default());
        let temp = TempDir::new().expect("tempdir");
        let game_root = root.then(|| {
            let mut game_root = GameRoot::new(temp.path());
            game_root
                .features()
                .pack(
                    CLOSE_NOTICE,
                    "navigable_route",
                    handler_task("notice", "first_white", TO_AWAY),
                )
                .bundle()
                .pages();
            game_root
        });
        script(&state, |capture, inputs| match (capture, inputs) {
            (1, _) => frame(RED, Some(Overlay::Notice)),
            (_, 0) => frame(RED, None),
            _ => frame(BLUE, None),
        });
        if failing {
            state.fail_capture_on.store(3, Ordering::Release);
            state
                .transient_capture_failure
                .store(true, Ordering::Release);
        }
        let host = start(&temp, &state, game_root.as_ref());
        let run = run(&host, &temp, "run", &away_to_home());
        host.close().expect("close runtime host");
        (comparable(&run), run.succeeded())
    };
    for failing in [false, true] {
        let (without, succeeded) = scenario(false, failing);
        assert_eq!(succeeded, !failing);
        let (with, _) = scenario(true, failing);
        assert_eq!(with, without, "failing: {failing}");
    }
}

// ---- N6-15: the root is read for every run.

/// N6-15: replacing `interrupts.json` (and `bundle.json`, with a new handler) in place is used
/// by the next run, with no restart; the changed registry is recorded as
/// `standard_pack_registered` with the package it gained.
#[test]
fn n6_15_root_files_replaced_in_place_take_effect_at_the_next_run() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let mut root = notice_root(temp.path(), json!("resume"), 1);
    overlay_then_away_then_home(&state, 0, Overlay::Notice);
    let host = start(&temp, &state, Some(&root));
    let first = run(&host, &temp, "first", &away_to_home());
    assert!(first.succeeded(), "{:?}", first.receipt);
    assert_eq!(
        first
            .handled()
            .iter()
            .map(|(page, handler, ..)| (page.as_str(), handler.as_str()))
            .collect::<Vec<_>>(),
        [("notice", CLOSE_NOTICE)]
    );

    root.pack(
        CLOSE_ALERT,
        "navigable_route",
        handler_task("alert", "third_white", TO_AWAY),
    )
    .bundle()
    .interrupts(json!([interrupt("alert", CLOSE_ALERT, json!("resume"), 1)]));
    overlay_then_away_then_home(&state, 2, Overlay::Alert);
    let second = run(&host, &temp, "second", &away_to_home());
    let reads = standard_pack_reads(&host);
    host.close().expect("close runtime host");

    assert!(second.succeeded(), "{:?}", second.receipt);
    assert_eq!(
        second
            .handled()
            .iter()
            .map(|(page, handler, ..)| (page.as_str(), handler.as_str()))
            .collect::<Vec<_>>(),
        [("alert", CLOSE_ALERT)]
    );
    let [startup, changed] = reads.as_slice() else {
        panic!("the startup record and the change: {reads:?}");
    };
    assert_eq!(startup.code().as_str(), "standard_pack_registered");
    assert_eq!(changed.code().as_str(), "standard_pack_registered");
    assert_eq!(integer(changed, "count"), Some(3));
    assert_eq!(strings(changed, "added_package_ids"), [CLOSE_ALERT]);
    assert!(changed.value("removed_package_ids").is_none());
}
