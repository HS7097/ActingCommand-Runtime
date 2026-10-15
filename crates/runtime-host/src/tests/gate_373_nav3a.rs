// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-3a (model M-373 v1.1 §4.2, §4.3, §8.3; test rows N3a-2 to N3a-7, host
// key 90): a page-graph package declares a prerequisite package and gets the return-home
// fallback, and every prerequisite and return-home package is found through the standard-pack
// registry of its game (`<standard_packs.root>/<game>/bundle.json`), read when a run uses it.
//
// A synthetic `neutral` game. The fake device's first pixel is the screen: red `away`, blue
// `menu`, green `shop`, white `done`; each input moves the screen to the next one its test
// scripts. Every package is a source package directory of the game, server `test`, 16x9; one
// click per step. The standard pack's packages sit in the game root's directories that its
// `bundle.json` lists; a run's own package is requested by its path outside that root.

use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    RecoveryRung, RecoveryRungOutcome, RuntimePayload, TaskEntryRecoveryRole,
};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
pub(super) const AWAY: [u8; 3] = [255, 0, 0];
pub(super) const MENU: [u8; 3] = [0, 0, 255];
const SHOP: [u8; 3] = [0, 255, 0];
const DONE: [u8; 3] = [255, 255, 255];

const LEAD: &str = "neutral.test.lead";
const LEAD_NEXT: &str = "neutral.test.lead_next";
const MIDDLE: &str = "neutral.test.middle";
pub(super) const RETURN_HOME: &str = "neutral.test.return_home";
pub(super) const MAIN: &str = "neutral.test.main";

const UNBOUND: &str = "contained_task_prerequisite_unbound";
const PREREQUISITE: Option<TaskEntryRecoveryRole> = Some(TaskEntryRecoveryRole::Prerequisite);
const RETURN_HOME_ROLE: Option<TaskEntryRecoveryRole> = Some(TaskEntryRecoveryRole::ReturnHome);

// ---- The screens.

// The screen after `inputs` inputs is `screens[inputs]` (the last one after that).
pub(super) fn screens(state: &FakeState, screens: Vec<[u8; 3]>) {
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            vec![(0, screens[inputs.min(screens.len() - 1)])]
        }));
}

// `menu` until the input count passes `base`, then `done`; the test moves `base` to the count
// before each run, so every run starts on `menu`.
fn menu_then_done(state: &FakeState) -> Arc<AtomicUsize> {
    let base = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&base);
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            if inputs <= seen.load(Ordering::Acquire) {
                vec![(0, MENU)]
            } else {
                vec![(0, DONE)]
            }
        }));
    base
}

// ---- Packages.

fn probe(page: &str) -> Value {
    let expected = match page {
        "away" => AWAY,
        "menu" => MENU,
        "shop" => SHOP,
        "done" => DONE,
        other => panic!("no screen {other}"),
    };
    json!({
        "id": format!("first_{page}"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

// A package's control and its one task: `entry` (a page or a set of pages), `target`, one
// click per operation `(id, from, to)`; its pages are the ones these name.
pub(super) struct Pack<'a> {
    pub(super) package_id: &'a str,
    pub(super) mode: &'a str,
    pub(super) prerequisite: Option<&'a str>,
    pub(super) entry: Value,
    pub(super) target: &'a str,
    pub(super) operations: &'a [(&'a str, &'a str, &'a str)],
}

impl Pack<'_> {
    pub(super) fn write(&self, directory: &Path) {
        let mut pages = BTreeSet::new();
        if let Some(entry) = self.entry.as_str() {
            pages.insert(entry.to_owned());
        }
        for entry in self.entry.as_array().into_iter().flatten() {
            pages.insert(entry.as_str().expect("entry page").to_owned());
        }
        pages.insert(self.target.to_owned());
        for (_, from, to) in self.operations {
            pages.insert((*from).to_owned());
            pages.insert((*to).to_owned());
        }
        let mut control = json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": self.package_id,
            "execution_mode": self.mode,
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        });
        if let Some(prerequisite) = self.prerequisite {
            control["prerequisite_package_id"] = json!(prerequisite);
        }
        let task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "NAV-3a fixture",
            "coordinate_space": {"width": 16, "height": 9},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": pages.iter().map(|page| probe(page)).collect::<Vec<_>>(),
            "page_rules": pages
                .iter()
                .map(|page| (page.clone(), json!({"required": [format!("first_{page}")]})))
                .collect::<serde_json::Map<_, _>>(),
            "entry_page": self.entry,
            "target_page": self.target,
            "operations": self
                .operations
                .iter()
                .map(|(id, from, to)| json!({
                    "id": id,
                    "from": from,
                    "to": to,
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true,
                    "retryable": false
                }))
                .collect::<Vec<_>>()
        });
        for (path, value) in [
            ("control.json", control),
            (
                "resources/operations/resources.json",
                json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
            ),
            ("resources/operations/task/task.json", task),
        ] {
            let path = directory.join(path);
            fs::create_dir_all(path.parent().expect("package file parent")).expect("package dir");
            fs::write(
                path,
                serde_json::to_vec_pretty(&value).expect("package JSON"),
            )
            .expect("write package file");
        }
    }
}

// A `navigable_route` package from `from` to `to` (entry `from`), declaring `prerequisite`.
pub(super) fn route<'a>(
    package_id: &'a str,
    from: &'a str,
    to: &'a str,
    operations: &'a [(&'a str, &'a str, &'a str)],
    prerequisite: Option<&'a str>,
) -> Pack<'a> {
    Pack {
        package_id,
        mode: "navigable_route",
        prerequisite,
        entry: json!(from),
        target: to,
        operations,
    }
}

// ---- The standard-pack root.

// The game root of `neutral` below `<base>/packages`: its listed packages, each in its own
// directory named by its `package_id` (bundle v4, Workflow #336 PKG-S5), and the return-home
// package of the server `test` when one is named.
pub(super) struct StandardRoot {
    pub(super) root: PathBuf,
    game: PathBuf,
    listed: Vec<String>,
    return_home: Option<String>,
}

impl StandardRoot {
    pub(super) fn new(base: &Path) -> Self {
        let root = base.join("packages");
        let game = root.join("neutral");
        fs::create_dir_all(&game).expect("game root");
        Self {
            root,
            game,
            listed: Vec::new(),
            return_home: None,
        }
    }

    // Lists `package_id` in its own directory `<game root>/<package_id>` (created) and gives
    // that directory, where the caller writes the package.
    pub(super) fn list(&mut self, package_id: &str) -> PathBuf {
        self.listed.push(package_id.to_owned());
        let path = self.game.join(package_id);
        fs::create_dir_all(&path).expect("package directory");
        path
    }

    // No longer lists `package_id` (its directory stays).
    fn unlist(&mut self, package_id: &str) {
        self.listed.retain(|listed| listed != package_id);
    }

    // Names `package_id` (listed) the return-home package of the server `test`.
    pub(super) fn return_home(&mut self, package_id: &str) {
        self.return_home = Some(package_id.to_owned());
    }

    // Writes `bundle.json` (v4) with the packages listed now.
    pub(super) fn write_bundle(&self) {
        let maintenance = self
            .return_home
            .iter()
            .map(|package_id| {
                json!({"package_id": package_id, "server": "test", "uses": ["return_home"]})
            })
            .collect::<Vec<_>>();
        let bundle = json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": "neutral",
            "packs": self
                .listed
                .iter()
                .map(|package_id| json!({
                    "package_id": package_id,
                    "server": "test",
                    "entry_task_id": "task",
                    "path": format!("packs/{package_id}"),
                    "file_count": 3,
                    "byte_count": 64
                }))
                .collect::<Vec<_>>(),
            "maintenance": maintenance
        });
        fs::write(
            self.game.join("bundle.json"),
            serde_json::to_vec_pretty(&bundle).expect("bundle JSON"),
        )
        .expect("write bundle");
    }
}

// ---- Running.

fn start(temp: &TempDir, state: &Arc<FakeState>, root: Option<PathBuf>) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        config(temp).with_standard_packs_root(root),
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

    // The layers the gate opened: each admitted package with its role.
    fn admitted(&self) -> Vec<(String, Option<TaskEntryRecoveryRole>)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryPackageAdmitted {
                    package_label,
                    role,
                } => Some((package_label.clone(), *role)),
                _ => None,
            })
            .collect()
    }

    // The layers that completed: each package with its final page and its role.
    fn completed(&self) -> Vec<(String, String, Option<TaskEntryRecoveryRole>)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecoveryCompleted {
                    package_label,
                    final_page,
                    role,
                    ..
                } => Some((package_label.clone(), final_page.clone(), *role)),
                _ => None,
            })
            .collect()
    }

    // The entry checks: phase, required pages and the page that matched.
    fn recognitions(&self) -> Vec<(TaskEntryRecognitionPhase, Vec<String>, Option<String>)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::EntryRecognition {
                    phase,
                    required_pages,
                    matched_page,
                    ..
                } => Some((*phase, required_pages.clone(), matched_page.clone())),
                _ => None,
            })
            .collect()
    }
}

// Runs the package written by `pack` at `<temp>/<name>` directly.
fn run(host: &RuntimeHost, temp: &TempDir, name: &str, pack: &Pack<'_>) -> Run {
    let path = temp.path().join(name);
    pack.write(&path);
    let task_request = ContainedTaskRequest::new(path.display().to_string()).expect("task request");
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

// The outcomes of the `standard_pack_read` lifecycle records, in ledger order.
fn standard_pack_reads(host: &RuntimeHost) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeLifecycleObserved),
        ..EventQuery::default()
    })
    .expect("lifecycle events")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => match value.phase() {
            actingcommand_contract::RuntimeLifecyclePhase::StandardPackRead { outcome } => {
                Some(outcome.clone())
            }
            _ => None,
        },
        _ => None,
    })
    .collect()
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

// A run denied in its preparation with `contained_task_prerequisite_unbound` and the registry
// problem `problem`: no task record, no input.
fn assert_unbound(run: &Run, state: &FakeState, problem: &str) {
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Denied,
        "{:?}",
        run.receipt
    );
    assert_eq!(run.receipt.outcome().code().as_str(), UNBOUND);
    assert_eq!(
        run.receipt
            .outcome()
            .value("registry_problem")
            .and_then(Value::as_str),
        Some(problem),
        "{:?}",
        run.receipt
    );
    assert!(run.facts().is_empty(), "no task record");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
}

pub(super) const LEAD_STEPS: &[(&str, &str, &str)] = &[("go", "away", "menu")];
pub(super) const OPEN: &[(&str, &str, &str)] = &[("open", "menu", "done")];

// The run's own package: from `menu` to `done`, declaring `prerequisite`.
fn main_pack(prerequisite: &str) -> Pack<'_> {
    route(MAIN, "menu", "done", OPEN, Some(prerequisite))
}

// ---- N3a-2: a page-graph package with a declared prerequisite package.

// N3a-2: a `navigable_route` package with the entry page set {menu, shop} declares the
// prerequisite package `lead` of its game's standard pack. The run starts away: the entry set is
// not matched, `lead` runs as a layer (`role: prerequisite`) and ends on `menu`, the whole entry
// set is checked again and matches `menu`, and the package runs from there.
#[test]
fn n3a_2_a_page_graph_package_runs_its_declared_prerequisite_and_rechecks_its_entry_set() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(LEAD, "away", "menu", LEAD_STEPS, None).write(&root.list(LEAD));
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));
    let main = Pack {
        package_id: MAIN,
        mode: "navigable_route",
        prerequisite: Some(LEAD),
        entry: json!(["menu", "shop"]),
        target: "done",
        operations: &[("open", "menu", "done"), ("leave", "shop", "done")],
    };

    let run = run(&host, &temp, "main", &main);
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    // The gate's two checks of the whole entry set (the package's own check in its run follows).
    let entry_set = vec!["neutral/menu".to_owned(), "neutral/shop".to_owned()];
    let recognitions = run.recognitions();
    assert_eq!(
        recognitions.get(..2),
        Some(
            &[
                (TaskEntryRecognitionPhase::Initial, entry_set.clone(), None),
                (
                    TaskEntryRecognitionPhase::PostRecovery,
                    entry_set,
                    Some("neutral/menu".to_owned())
                ),
            ][..]
        ),
        "{recognitions:?}"
    );
    assert_eq!(run.admitted(), vec![(LEAD.to_owned(), PREREQUISITE)]);
    assert_eq!(
        run.completed(),
        vec![(LEAD.to_owned(), "neutral/menu".to_owned(), PREREQUISITE)]
    );
}

// ---- N3a-3: the chain goes on through a page-graph layer.

// N3a-3 (the run; the chain's limits are the kernel's table, exec key 67): the package declares
// `middle`, a page-graph package that declares `lead` itself. Away from both entries, `lead`
// runs first, then `middle`, each a `prerequisite` layer, and the package runs from `shop`.
#[test]
fn n3a_3_a_page_graph_prerequisite_layer_leads_the_chain_on() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU, SHOP, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(LEAD, "away", "menu", LEAD_STEPS, None).write(&root.list(LEAD));
    route(
        MIDDLE,
        "menu",
        "shop",
        &[("go", "menu", "shop")],
        Some(LEAD),
    )
    .write(&root.list(MIDDLE));
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));

    let run = run(
        &host,
        &temp,
        "main",
        &route(
            MAIN,
            "shop",
            "done",
            &[("open", "shop", "done")],
            Some(MIDDLE),
        ),
    );
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 3);
    assert_eq!(
        run.admitted(),
        vec![
            (MIDDLE.to_owned(), PREREQUISITE),
            (LEAD.to_owned(), PREREQUISITE)
        ]
    );
    assert_eq!(
        run.completed(),
        vec![
            (LEAD.to_owned(), "neutral/menu".to_owned(), PREREQUISITE),
            (MIDDLE.to_owned(), "neutral/shop".to_owned(), PREREQUISITE),
        ]
    );
}

// ---- N3a-4: the registry, not the configuration.

// N3a-4: the configuration names no prerequisite package at all; the game root's `bundle.json`
// lists the one a `linear_steps` package declares, so its chain resolves and the run completes.
#[test]
fn n3a_4_a_prerequisite_package_listed_in_the_bundle_resolves_without_configuration() {
    let state = Arc::new(FakeState::default());
    // The linear package's first step is red, its target blue.
    screens(&state, vec![AWAY, MENU]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    super::gate_e4::exec_i6::write_pack(
        &root.list(LEAD),
        &super::gate_e4::exec_i6::I6Pack {
            package_id: LEAD,
            prerequisite: None,
            resolution: super::gate_e4::exec_i6::RESOLUTION,
        },
    );
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));
    let main = temp.path().join("main");
    super::gate_e4::exec_i6::write_pack(
        &main,
        &super::gate_e4::exec_i6::I6Pack {
            package_id: MAIN,
            prerequisite: Some(LEAD),
            resolution: super::gate_e4::exec_i6::RESOLUTION,
        },
    );
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let request = client.request(RuntimeOperation::run_contained_task(
        ALIAS,
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(main.display().to_string()).expect("task request"),
    ));

    let receipt = client.send(&request);
    drop(client);
    host.close().expect("close runtime host");

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// ---- N3a-5: replaced in place, found by the next run.

// N3a-5: while the Runtime runs, the standard pack is replaced in place: `lead` leaves the
// bundle and `lead_next` joins it. The next run resolves `lead_next`; a run that still declares
// `lead` is denied with `contained_task_prerequisite_unbound` (`not_listed`); the registry
// change is recorded once, with what was added and removed; there is no restart.
#[test]
fn n3a_5_a_standard_pack_replaced_in_place_is_used_by_the_next_run() {
    let state = Arc::new(FakeState::default());
    let base = menu_then_done(&state);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(LEAD, "away", "menu", LEAD_STEPS, None).write(&root.list(LEAD));
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));

    let before = run(&host, &temp, "before", &main_pack(LEAD));
    assert!(before.succeeded(), "{:?}", before.receipt);

    route(LEAD_NEXT, "away", "menu", LEAD_STEPS, None).write(&root.list(LEAD_NEXT));
    root.unlist(LEAD);
    root.write_bundle();
    base.store(state.input_count.load(Ordering::Acquire), Ordering::Release);
    let after = run(&host, &temp, "after", &main_pack(LEAD_NEXT));
    let inputs = state.input_count.load(Ordering::Acquire);
    let removed = run(&host, &temp, "removed", &main_pack(LEAD));
    let reads = standard_pack_reads(&host);
    host.close().expect("close runtime host");

    assert!(after.succeeded(), "{:?}", after.receipt);
    assert_eq!(removed.receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(removed.receipt.outcome().code().as_str(), UNBOUND);
    assert_eq!(
        removed
            .receipt
            .outcome()
            .value("registry_problem")
            .and_then(Value::as_str),
        Some("not_listed")
    );
    assert!(removed.facts().is_empty(), "no task record");
    assert_eq!(state.input_count.load(Ordering::Acquire), inputs);
    let [started, replaced] = reads.as_slice() else {
        panic!("the start's record and one of the replacement: {reads:?}");
    };
    for registered in [started, replaced] {
        assert_eq!(registered.code().as_str(), "standard_pack_registered");
        assert_eq!(registered.value("count").and_then(Value::as_u64), Some(1));
    }
    assert_eq!(strings(started, "added_package_ids"), [LEAD]);
    assert_eq!(strings(replaced, "added_package_ids"), [LEAD_NEXT]);
    assert_eq!(strings(replaced, "removed_package_ids"), [LEAD]);
}

// ---- N3a-6: why a package is not bound.

// N3a-6: without `standard_packs.root` a declared prerequisite package is unbound with
// `registry_problem: root_unset`; with a root that holds no root of the package's game, with
// `game_root_missing`. Each run is denied in its preparation, with no task record and no input.
#[test]
fn n3a_6_an_unset_root_and_a_missing_game_root_leave_the_prerequisite_unbound() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let host = start(&temp, &state, None);
    let unset = run(&host, &temp, "main", &main_pack(LEAD));
    host.close().expect("close runtime host");
    assert_unbound(&unset, &state, "root_unset");

    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path().join("packages");
    fs::create_dir_all(root.join("other")).expect("another game's root");
    let host = start(&temp, &state, Some(root));
    let missing = run(&host, &temp, "main", &main_pack(LEAD));
    host.close().expect("close runtime host");
    assert_unbound(&missing, &state, "game_root_missing");
}

// ---- N3a-7: the return-home package of the registry.

// N3a-7, linear: a `linear_steps` package that declares no prerequisite package and starts away
// from its first step falls back to the return-home package the bundle names for its server
// (`maintenance[].uses` `return_home`), run as the `return_home` layer.
#[test]
fn n3a_7_a_linear_package_falls_back_to_the_registry_return_home_package() {
    let state = Arc::new(FakeState::default());
    // The linear package's first step is red, its target blue.
    screens(&state, vec![MENU, AWAY, MENU]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(
        RETURN_HOME,
        "menu",
        "away",
        &[("back", "menu", "away")],
        None,
    )
    .write(&root.list(RETURN_HOME));
    root.return_home(RETURN_HOME);
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));
    let main = temp.path().join("main");
    super::gate_e4::exec_i6::write_pack(
        &main,
        &super::gate_e4::exec_i6::I6Pack {
            package_id: MAIN,
            prerequisite: None,
            resolution: super::gate_e4::exec_i6::RESOLUTION,
        },
    );
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(main.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    drop(client);
    let run = Run {
        receipt,
        events: host
            .query_persisted_events_for_test(EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            })
            .expect("run events"),
    };
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        run.admitted(),
        vec![(RETURN_HOME.to_owned(), RETURN_HOME_ROLE)]
    );
    assert_eq!(
        run.completed(),
        vec![(
            RETURN_HOME.to_owned(),
            "neutral/away".to_owned(),
            RETURN_HOME_ROLE
        )]
    );
}

// N3a-7, page graph: a page-graph package with an entry page set and no prerequisite package,
// whose request binds no recovery package, starts away from its entry: its home entry runs the
// registry's return-home package (`role: return_home`) and the package runs from `menu`.
#[test]
fn n3a_7_a_page_graph_package_falls_back_to_the_registry_return_home_package() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = StandardRoot::new(temp.path());
    route(RETURN_HOME, "away", "menu", LEAD_STEPS, None).write(&root.list(RETURN_HOME));
    root.return_home(RETURN_HOME);
    root.write_bundle();
    let host = start(&temp, &state, Some(root.root.clone()));
    let main = Pack {
        package_id: MAIN,
        mode: "navigable_route",
        prerequisite: None,
        entry: json!(["menu"]),
        target: "done",
        operations: OPEN,
    };

    let run = run(&host, &temp, "main", &main);
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        run.admitted(),
        vec![(RETURN_HOME.to_owned(), RETURN_HOME_ROLE)]
    );
    assert_eq!(
        run.completed(),
        vec![(
            RETURN_HOME.to_owned(),
            "neutral/menu".to_owned(),
            RETURN_HOME_ROLE
        )]
    );
}

// N3a-7, ladder: a scheduled run that binds no recovery package fails and starts the recovery
// ladder; its first rung runs the return-home package the registry names for the failed
// package's game and server (the run of that package is the rung's).
#[test]
fn n3a_7_the_ladder_first_rung_runs_the_registry_return_home_package() {
    let temp = TempDir::new().expect("tempdir");
    let (host_config, _) = super::instance_workers::ladder_setup(temp.path());
    let request = ContainedTaskRequest::new(
        temp.path()
            .join("physical-scheduled-task.zip")
            .to_string_lossy()
            .into_owned(),
    )
    .expect("scheduled request without a recovery binding");
    let mut root = StandardRoot::new(temp.path());
    route(RETURN_HOME, "away", "menu", LEAD_STEPS, None).write(&root.list(RETURN_HOME));
    root.return_home(RETURN_HOME);
    root.write_bundle();
    let state = Arc::new(FakeState::default());
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
    state.unknown_capture.store(true, Ordering::Release);

    host.run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    super::instance_workers::wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close runtime host");

    let rungs = super::instance_workers::rung_outcomes(&events);
    assert!(
        rungs
            .iter()
            .any(|(rung, result, _)| *rung == RecoveryRung::ReturnHome
                && *result != RecoveryRungOutcome::Skipped),
        "{rungs:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Task(TaskPayload::Semantic(payload))
                if matches!(
                    payload.fact(),
                    TaskSemanticFact::PackageAdmitted { package_label, .. }
                        if package_label == RETURN_HOME
                )
        )),
        "the rung ran the registry's return-home package"
    );
}
