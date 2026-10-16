// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-5 (model M-373 v1.1 §4.4, §5, §8.2, §8.3, §8.5, §9; test rows N5-1, N5-3
// to N5-12 and N5-14 to N5-20, host key 97; N5-1's table is the contract crate's, N5-2 is
// `host/navigation.rs`'s own test, N5-13 the kernel's `text_locate_tests.rs`): the entry gate's
// full-match routing. Wave-17 ruling 1: a recovery chain step is a run the host starts itself,
// and NAV-5's real `contained_task_navigation_recovery_exhausted` starts the configured ladder
// profile (host key 96 beside RD-4's T5-T7).
//
// A synthetic `neutral` game. The fake device's first three pixels are the screen; each input
// moves it to the next screen the test scripts. The page list `pages.json` names `home`, `shop`,
// `menu`, `notice` and `alert`; the feature package recognises them by their pixels:
// - `home`: the first pixel blue; `shop`: the first red; `menu`: the second white;
// - `notice`: the first white; `alert`: the third white.
// A frame with a red first and a white second pixel is both `shop` and `menu`; a black first
// pixel is no listed page. The edges of `navigation.json` lead `menu` to `shop` and `shop` to
// `home`, each with its own package; the recovery chain's package leaves the dark screen for
// `shop`. The run's own package starts on `home` and ends on `done` (a yellow first pixel). Every
// package is a source package directory of the game, server `test`, 16x9, one click per step.

use super::gate_398_rd1b::{bound, resolved_handset};
use super::instance_workers::{admit_ladder_run, phases, wait_for_ladder_end, wait_until};
use super::*;
use crate::codes::HostCode;
use crate::failure_identity::{
    FailureIdentity, IdentityFrame, IdentityLayer, IdentityPackage, failure_accumulates,
    navigation_failure_code,
};
use crate::suspension_lift::{
    LiftCheckTime, LiftChecks, PausedPackages, SuspensionLift, SuspensionLiftView,
    standard_pack_root_hash,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Category, CodeStr, Outcome};
use actingcommand_contract::{
    RecoveryLadderOutcome, RecoveryRung, RecoveryRungPlan, RecoveryRungSkipReason,
    RecoveryRungState, RecoveryTriggerMatch, RuntimeConfigManifest, RuntimeLifecyclePhase,
    RuntimePayload, TaskEntryRecoveryRole, TaskNavigationHop,
};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const SERIAL: &str = "TESTSERIAL0097";
// The device-state sampler's interval and fact lifetime of the handset host: an hour, so no
// sample reads the handset while a test runs.
const QUIET_MS: u64 = 3_600_000;

const BLACK: [u8; 3] = [0, 0, 0];
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];
const YELLOW: [u8; 3] = [255, 255, 0];

// A screen: the first three pixels.
type Screen = [[u8; 3]; 3];
const HOME: Screen = [BLUE, BLACK, BLACK];
const SHOP: Screen = [RED, BLACK, BLACK];
const MENU: Screen = [GREEN, WHITE, BLACK];
// Both `shop` and `menu`.
const SHOP_MENU: Screen = [RED, WHITE, BLACK];
// `alert` over `shop`.
const ALERT_SHOP: Screen = [RED, BLACK, WHITE];
// No listed page.
const DARK: Screen = [BLACK, BLACK, BLACK];
const DONE: Screen = [YELLOW, BLACK, BLACK];

const MAIN: &str = "neutral.test.main";
const FEATURES: &str = "neutral.test.features";
const SHOP_TO_HOME: &str = "neutral.test.shop_to_home";
const MENU_TO_SHOP: &str = "neutral.test.menu_to_shop";
const WAKE: &str = "neutral.test.wake";
const NAP: &str = "neutral.test.nap";
const CLOSE_NOTICE: &str = "neutral.test.close_notice";
const CLOSE_ALERT: &str = "neutral.test.close_alert";
const LEAD: &str = "neutral.test.lead";
const RETURN_HOME: &str = "neutral.test.return_home";
const LIAR: &str = "neutral.test.liar";
const BIG: &str = "neutral.test.big";
const BAD: &str = "neutral.test.bad";
const OTHER_EDGE: &str = "neutral.test.other_edge";
const OTHER_WAKE: &str = "neutral.test.other_wake";

const HOP: Option<TaskEntryRecoveryRole> = Some(TaskEntryRecoveryRole::NavigationHop);
const CHAIN: Option<TaskEntryRecoveryRole> = Some(TaskEntryRecoveryRole::RecoveryChain);

// After `n` inputs the screen is `screens[n]` (the last one after that).
fn screens(state: &FakeState, screens: Vec<Screen>) {
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            screens[inputs.min(screens.len() - 1)]
                .iter()
                .copied()
                .enumerate()
                .collect()
        }));
}

// ---- Packages.

// Every pixel probe: its id, its pixel and its colour.
const PROBES: &[(&str, usize, [u8; 3])] = &[
    ("first_blue", 0, BLUE),
    ("first_red", 0, RED),
    ("first_green", 0, GREEN),
    ("first_white", 0, WHITE),
    ("first_black", 0, BLACK),
    ("first_yellow", 0, YELLOW),
    ("second_white", 1, WHITE),
    ("second_black", 1, BLACK),
    ("third_white", 2, WHITE),
    ("third_black", 2, BLACK),
];

// A page's probes, unless a package declares its own rule for it.
fn rule(page: &str) -> &'static [&'static str] {
    match page {
        "home" => &["first_blue"],
        "shop" => &["first_red"],
        "menu" => &["second_white"],
        "notice" => &["first_white"],
        "alert" => &["third_white"],
        "under" => &["first_red", "third_black"],
        "dark" => &["first_black"],
        "stale" => &["first_green"],
        "done" => &["first_yellow"],
        other => panic!("no page {other}"),
    }
}

fn probe(id: &str) -> Value {
    let (_, x, expected) = PROBES
        .iter()
        .find(|(probe, _, _)| *probe == id)
        .unwrap_or_else(|| panic!("no probe {id}"));
    json!({
        "id": id,
        "region": {"mode": "rect", "rect": {"x": x, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

// A package's control and its one task (schema `0.9`, so control `Lab-1y.control.v2`): its
// entry (a page, a set of pages or `any`), its target and one click per operation
// `(id, from, to)`; its pages are the ones these name, each by `rule` unless `rules` says
// otherwise.
#[derive(Clone)]
struct Pack<'a> {
    package_id: &'a str,
    mode: &'a str,
    server: &'a str,
    prerequisite: Option<&'a str>,
    entry: Value,
    target: &'a str,
    operations: Vec<(&'a str, &'a str, &'a str)>,
    rules: Vec<(&'a str, Vec<&'a str>)>,
    step_timeout_ms: u64,
    max_steps: u32,
    width: u32,
}

impl Pack<'_> {
    fn write(&self, directory: &Path) {
        let mut pages = BTreeSet::new();
        match &self.entry {
            Value::String(page) => {
                pages.insert(page.clone());
            }
            Value::Array(entries) => {
                pages.extend(entries.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            _ => {}
        }
        pages.insert(self.target.to_owned());
        for (_, from, to) in &self.operations {
            pages.insert((*from).to_owned());
            pages.insert((*to).to_owned());
        }
        pages.remove("any");
        let rule_of = |page: &str| -> Vec<String> {
            let probes = self
                .rules
                .iter()
                .find(|(name, _)| *name == page)
                .map_or(rule(page), |(_, probes)| probes.as_slice());
            probes.iter().map(|probe| (*probe).to_owned()).collect()
        };
        let probes = pages
            .iter()
            .flat_map(|page| rule_of(page.as_str()))
            .collect::<BTreeSet<_>>();
        let height = self.width * 9 / 16;
        let mut control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": self.package_id,
            "execution_mode": self.mode,
            "game": "neutral",
            "server": self.server,
            "resolution": {"width": self.width, "height": height},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": self.step_timeout_ms,
            "timeout_ms": 8000,
            "max_steps": self.max_steps
        });
        if let Some(prerequisite) = self.prerequisite {
            control["prerequisite_package_id"] = json!(prerequisite);
        }
        let linear = self.mode == "linear_steps";
        let mut task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": [self.server],
            "locale": "en-US",
            "goal": "NAV-5 fixture",
            "coordinate_space": {"width": self.width, "height": height},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": probes.iter().map(|id| probe(id)).collect::<Vec<_>>(),
            "page_rules": pages
                .iter()
                .map(|page| (page.clone(), json!({"required": rule_of(page.as_str())})))
                .collect::<serde_json::Map<_, _>>(),
            "entry_page": self.entry,
            "target_page": self.target,
            "operations": self
                .operations
                .iter()
                .map(|(id, from, to)| {
                    let mut operation = json!({
                        "id": id,
                        "from": from,
                        "to": to,
                        "click": {"kind": "point", "x": 1, "y": 0},
                        "unguarded_trusted_coordinate": true
                    });
                    if !linear {
                        operation["retryable"] = json!(false);
                    }
                    operation
                })
                .collect::<Vec<_>>()
        });
        // A linear task declares its step count in the task as well as the control.
        if linear {
            task["max_steps"] = json!(self.max_steps);
        }
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

// A `navigable_route` package from `entry` to `target`.
fn graph<'a>(
    package_id: &'a str,
    entry: &'a str,
    target: &'a str,
    operations: &[(&'a str, &'a str, &'a str)],
) -> Pack<'a> {
    Pack {
        package_id,
        mode: "navigable_route",
        server: "test",
        prerequisite: None,
        entry: json!(entry),
        target,
        operations: operations.to_vec(),
        rules: Vec::new(),
        step_timeout_ms: 600,
        max_steps: 2,
        width: 16,
    }
}

// The feature package: a `recognize_only` package with exactly the pages of `pages.json`.
fn features() -> Pack<'static> {
    Pack {
        mode: "recognize_only",
        ..graph(
            FEATURES,
            "home",
            "home",
            &[
                ("see_shop", "shop", "home"),
                ("see_menu", "menu", "home"),
                ("see_notice", "notice", "home"),
                ("see_alert", "alert", "home"),
            ],
        )
    }
}

// The run's own package: a page graph from `home` to `done`, a short entry window.
fn graph_main() -> Pack<'static> {
    Pack {
        step_timeout_ms: 200,
        max_steps: 4,
        ..graph(MAIN, "home", "done", &[("finish", "home", "done")])
    }
}

// The run's own package as a `linear_steps` package, its first step on `home`; its step count
// is its one operation.
fn linear_main() -> Pack<'static> {
    Pack {
        mode: "linear_steps",
        max_steps: 1,
        ..graph_main()
    }
}

// ---- The standard-pack root.

struct Root {
    root: PathBuf,
    game: PathBuf,
    // Each listed package and its server.
    packs: Vec<(String, String)>,
    return_home: Option<String>,
}

impl Root {
    fn new(base: &Path) -> Self {
        let root = base.join("packages");
        let game = root.join("neutral");
        fs::create_dir_all(&game).expect("game root");
        Self {
            root,
            game,
            packs: Vec::new(),
            return_home: None,
        }
    }

    // Lists `pack` (replacing a listed package of its id) in its directory, written now.
    fn add(&mut self, pack: &Pack<'_>) -> &mut Self {
        pack.write(&self.game.join(pack.package_id));
        self.packs.retain(|(listed, _)| listed != pack.package_id);
        self.packs
            .push((pack.package_id.to_owned(), pack.server.to_owned()));
        self
    }

    // Lists `package_id` of `server` with no directory.
    fn list_only(&mut self, package_id: &str, server: &str) -> &mut Self {
        self.packs.retain(|(listed, _)| listed != package_id);
        self.packs.push((package_id.to_owned(), server.to_owned()));
        self
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
                "schema_version": "actingcommand.bundle.v4",
                "game": "neutral",
                "packs": self
                    .packs
                    .iter()
                    .map(|(package_id, server)| json!({
                        "package_id": package_id,
                        "server": server,
                        "entry_task_id": "task",
                        "path": format!("packs/{package_id}"),
                        "file_count": 3,
                        "byte_count": 64
                    }))
                    .collect::<Vec<_>>(),
                "maintenance": self
                    .return_home
                    .iter()
                    .map(|package_id| json!({
                        "package_id": package_id, "server": "test", "uses": ["return_home"]
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
                    {"id": "shop", "kind": "page"},
                    {"id": "menu", "kind": "page"},
                    {"id": "notice", "kind": "overlay"},
                    {"id": "alert", "kind": "overlay"}
                ]
            }),
        );
        self
    }

    fn navigation(&self, edges: &[(&str, &str, &str)], chain: &[&str]) -> &Self {
        self.write(
            "navigation.json",
            &json!({
                "schema_version": "actingcommand.navigation.v1",
                "game": "neutral",
                "edges": edges
                    .iter()
                    .map(|(from, to, package_id)| json!({
                        "from": from, "to": to, "package_id": package_id
                    }))
                    .collect::<Vec<_>>(),
                "recovery_chain": chain
            }),
        );
        self
    }

    // The edges `menu` to `shop` and `shop` to `home`, and the chain that leaves the dark screen.
    fn default_navigation(&self) -> &Self {
        self.navigation(
            &[
                ("shop", "home", SHOP_TO_HOME),
                ("menu", "shop", MENU_TO_SHOP),
            ],
            &[WAKE],
        )
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

// The game root with the feature package, the edges' and the chain's packages and the two
// interrupt handlers, its `bundle.json` and `pages.json`; no `navigation.json` yet.
fn world(base: &Path) -> Root {
    let mut root = Root::new(base);
    root.add(&features())
        .add(&graph(
            SHOP_TO_HOME,
            "shop",
            "home",
            &[("go", "shop", "home")],
        ))
        .add(&graph(
            MENU_TO_SHOP,
            "menu",
            "shop",
            &[("go", "menu", "shop")],
        ))
        .add(&graph(WAKE, "dark", "shop", &[("wake", "dark", "shop")]))
        .add(&graph(
            CLOSE_NOTICE,
            "notice",
            "shop",
            &[("close", "notice", "shop")],
        ))
        .add(&graph(
            CLOSE_ALERT,
            "alert",
            "under",
            &[("close", "alert", "under")],
        ));
    root.bundle().pages();
    root
}

// ---- Running.

fn start(temp: &TempDir, state: &Arc<FakeState>, root: &Root) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        config(temp).with_standard_packs_root(Some(root.root.clone())),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host")
}

struct Run {
    receipt: Option<RuntimeReceipt>,
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
        self.facts().into_iter().any(|fact| {
            matches!(
                fact,
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    ..
                }
            )
        })
    }

    // The run's terminal outcome: a failed run's failure.
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
            .unwrap_or_else(|| panic!("a failed terminal: {:?}", self.receipt))
    }

    // Each locate: its attempt, the code of its outcome, the page the outcome names and the
    // interrupt page it handed on.
    fn located(&self) -> Vec<(u32, String, Option<String>, Option<String>)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::NavigationLocated {
                    attempt,
                    interrupt_page,
                    outcome,
                    ..
                } => Some((
                    *attempt,
                    outcome.code().as_str().to_owned(),
                    text(outcome, "page_id").map(str::to_owned),
                    interrupt_page.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    // Each layer the gate opened, with its role.
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

    // Each planned route: its located page and its hops.
    fn routes(&self) -> Vec<(String, Vec<TaskNavigationHop>)> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::NavigationRoutePlanned { from_page, hops } => {
                    Some((from_page.clone(), hops.clone()))
                }
                _ => None,
            })
            .collect()
    }

    // The outcomes of the navigation's warnings.
    fn warnings(&self) -> Vec<Outcome> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                TaskSemanticFact::NavigationWarning { outcome } => Some(outcome.clone()),
                _ => None,
            })
            .collect()
    }

    // The navigation's own records (located, planned, warnings), as values.
    fn navigation_records(&self) -> Vec<Value> {
        self.facts()
            .into_iter()
            .filter(|fact| {
                matches!(
                    fact,
                    TaskSemanticFact::NavigationLocated { .. }
                        | TaskSemanticFact::NavigationRoutePlanned { .. }
                        | TaskSemanticFact::NavigationWarning { .. }
                )
            })
            .map(|fact| serde_json::to_value(fact).expect("fact JSON"))
            .collect()
    }
}

// Runs the package `pack` writes at `<temp>/<name>` directly on the instance.
fn run_direct(host: &RuntimeHost, temp: &TempDir, name: &str, pack: &Pack<'_>) -> Run {
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
    Run {
        receipt: Some(receipt),
        events,
    }
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

fn sorted(mut items: Vec<String>) -> Vec<String> {
    items.sort();
    items
}

fn hop(from: &str, to: &str, package_label: &str) -> TaskNavigationHop {
    TaskNavigationHop {
        from: from.to_owned(),
        to: to.to_owned(),
        package_label: package_label.to_owned(),
    }
}

fn located(
    attempt: u32,
    code: &str,
    page: Option<&str>,
) -> (u32, String, Option<String>, Option<String>) {
    (attempt, code.to_owned(), page.map(str::to_owned), None)
}

fn inputs(state: &FakeState) -> usize {
    state.input_count.load(Ordering::Acquire)
}

// ---- Scheduled runs (the ladder fixture of `instance_workers`, without a request's own
// recovery package).

// The configuration manifest of the defaults (the trigger item `navigation_exhausted` at rung 3,
// the `handset` profile of two rungs, `exhaust`), with a quiet device-state sampler.
fn quiet_manifest() -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let file = json!({"device_state": {"sample_interval_ms": QUIET_MS, "fact_ttl_ms": QUIET_MS}});
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, &file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

// A host whose policy runs the package at `main` on the fixture instance, over `root`, on an
// emulator or (`handset`) a bound handset.
fn scheduled_host(
    temp: &TempDir,
    state: &Arc<FakeState>,
    root: &Root,
    main: &Path,
    handset: bool,
) -> (RuntimeHost, ContainedTaskRequest) {
    let request =
        ContainedTaskRequest::new(main.to_string_lossy().into_owned()).expect("scheduled request");
    let startup =
        ContainedTaskRequest::new(main.to_string_lossy().into_owned()).expect("startup package");
    let mut host_config = RuntimeHostConfig::new(temp.path(), b"runtime-host-test-salt")
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest_with_primary(
            &[],
            vec!["after_observation".to_owned()],
        ))
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
        .with_startup_packages(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            startup,
        )]))
        .with_scheduled_procedures(["procedure.observe".to_owned()])
        .with_standard_packs_root(Some(root.root.clone()));
    state.physical_task_geometry.store(true, Ordering::Release);
    let instance = instance_id();
    let mut provider = FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, Arc::clone(state));
    if handset {
        state.observed_open.store(true, Ordering::Release);
        provider = provider.with_resolved_for(
            POLICY_INSTANCE_ALIAS,
            resolved_handset(
                POLICY_INSTANCE_ALIAS,
                instance,
                bound(POLICY_INSTANCE_ALIAS, SERIAL),
            ),
        );
        host_config = host_config.with_config_manifest(quiet_manifest());
    }
    let host = RuntimeHost::start(host_config, Arc::new(provider)).expect("runtime host");
    (host, request)
}

// Admits and runs the scheduled run; its records by its run id.
fn scheduled_run(
    host: &RuntimeHost,
    request: &ContainedTaskRequest,
) -> (Box<PolicyRunContext>, Result<RuntimeReceipt, String>, Run) {
    let context = admit_ladder_run(host, request);
    let result = host
        .run_scheduled_contained_task(&context, request)
        .map_err(|error| error.code().to_owned());
    let events = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        })
        .expect("run events");
    (
        context,
        result,
        Run {
            receipt: None,
            events,
        },
    )
}

// The failure identity the settlement of the scheduled run recorded.
fn recorded_identity(run: &Run) -> FailureIdentity {
    let identity = run
        .events
        .iter()
        .find_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
                match payload.policy_result() {
                    PolicyExecutionOutcome::Failed { failure } => failure.failure_identity.clone(),
                    PolicyExecutionOutcome::Succeeded { .. } => None,
                }
            }
            _ => None,
        })
        .expect("the failure's identity");
    FailureIdentity::parse(&identity)
        .unwrap_or_else(|| panic!("a format v2 failure identity: {identity}"))
}

fn id8(package_id: &str) -> String {
    format!("{:x}", Sha256::digest(package_id.as_bytes()))
        .chars()
        .take(8)
        .collect()
}

// ---- N5-1: the reader.

// N5-1 (the reader's half; the table is the contract crate's `navigation_tests`): an edge that
// names a page `pages.json` does not list makes the game root invalid; startup records
// `standard_pack_root_invalid` with its problem and pointer, and the run that needs the root is
// refused before any task record and any input.
#[test]
fn n5_1_a_broken_navigation_file_makes_the_game_root_invalid() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.navigation(&[("stage", "home", SHOP_TO_HOME)], &[WAKE]);
    screens(&state, vec![SHOP]);
    let host = start(&temp, &state, &root);
    let reads = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeLifecycleObserved),
            ..EventQuery::default()
        })
        .expect("lifecycle events")
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => {
                match value.phase() {
                    RuntimeLifecyclePhase::StandardPackRead { outcome } => Some(outcome.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    let [invalid] = reads.as_slice() else {
        panic!("one record of the game root: {reads:?}");
    };
    assert_eq!(invalid.code().as_str(), "standard_pack_root_invalid");
    assert_eq!(invalid.category(), Category::Error);
    assert_eq!(text(invalid, "game"), Some("neutral"));
    assert!(text(invalid, "path").is_some_and(|path| path.ends_with("navigation.json")));
    assert_eq!(
        text(invalid, "standard_pack_problem"),
        Some("edge_page_unlisted")
    );
    assert_eq!(text(invalid, "json_pointer"), Some("/edges/0/from"));
    let receipt = run.receipt.as_ref().expect("the receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.outcome().code().as_str(),
        "standard_pack_root_invalid"
    );
    assert!(run.facts().is_empty(), "no task record");
    assert_eq!(inputs(&state), 0);
}

// ---- N5-3 to N5-12, N5-14 to N5-16: the gate.

// N5-3: a `linear_steps` package that declares no prerequisite package and starts away from its
// entry, with the registry's return-home package of its server and navigation data: the gate
// navigates instead of running the return-home package (R16), along the route the edges give.
#[test]
fn n5_3_without_a_prerequisite_the_gate_navigates_instead_of_returning_home() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![MENU, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.add(&graph(
        RETURN_HOME,
        "menu",
        "home",
        &[("back", "menu", "home")],
    ));
    root.return_home = Some(RETURN_HOME.to_owned());
    root.bundle().default_navigation();
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &linear_main());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        run.admitted(),
        [
            (MENU_TO_SHOP.to_owned(), HOP),
            (SHOP_TO_HOME.to_owned(), HOP),
        ]
    );
    assert_eq!(
        run.routes(),
        [(
            "menu".to_owned(),
            vec![
                hop("menu", "shop", MENU_TO_SHOP),
                hop("shop", "home", SHOP_TO_HOME),
            ]
        )]
    );
    assert_eq!(
        run.located(),
        [located(
            1,
            "contained_task_navigation_located",
            Some("menu")
        )]
    );
    assert_eq!(inputs(&state), 3);
}

// N5-4: a page-graph package that declares a prerequisite package: the prerequisite runs, the
// entry still does not pass, and the gate then navigates when the game has navigation data;
// without it the run ends with today's `contained_task_prerequisite_entry_unmatched`.
#[test]
fn n5_4_after_a_declared_prerequisite_the_gate_navigates_only_with_navigation_data() {
    for navigation in [true, false] {
        let state = Arc::new(FakeState::default());
        screens(&state, vec![MENU, SHOP, HOME, DONE]);
        let temp = TempDir::new().expect("tempdir");
        let mut root = world(temp.path());
        root.add(&graph(LEAD, "menu", "shop", &[("lead", "menu", "shop")]));
        root.bundle();
        if navigation {
            root.default_navigation();
        }
        let host = start(&temp, &state, &root);
        let main = Pack {
            prerequisite: Some(LEAD),
            ..graph_main()
        };
        let run = run_direct(&host, &temp, "main", &main);
        host.close().expect("close runtime host");

        if navigation {
            assert!(run.succeeded(), "{:?}", run.receipt);
            assert_eq!(
                run.admitted(),
                [
                    (LEAD.to_owned(), Some(TaskEntryRecoveryRole::Prerequisite)),
                    (SHOP_TO_HOME.to_owned(), HOP),
                ]
            );
            assert_eq!(
                run.routes(),
                [("shop".to_owned(), vec![hop("shop", "home", SHOP_TO_HOME)])]
            );
            assert_eq!(inputs(&state), 3);
        } else {
            assert_eq!(
                run.failure().code().as_str(),
                "contained_task_prerequisite_entry_unmatched"
            );
            assert!(run.located().is_empty());
            assert_eq!(inputs(&state), 1);
        }
    }
}

// N5-5: no listed page on the first frame: the recovery chain's first step runs (a layer the
// host starts itself, `role: recovery_chain`), the next locate finds `shop`, and the route
// leads to the entry.
#[test]
fn n5_5_an_unrecognised_frame_runs_the_chain_then_locates_and_routes() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        run.located(),
        [
            located(1, "contained_task_navigation_page_unrecognized", None),
            located(2, "contained_task_navigation_located", Some("shop")),
        ]
    );
    assert_eq!(
        run.admitted(),
        [(WAKE.to_owned(), CHAIN), (SHOP_TO_HOME.to_owned(), HOP)]
    );
    assert_eq!(inputs(&state), 3);
}

// N5-6: one frame matches two listed pages: the record lists both, nothing is picked, no hop is
// planned or run, and the recovery chain's next step runs.
#[test]
fn n5_6_a_frame_of_two_pages_is_ambiguous_and_goes_to_the_chain() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![SHOP_MENU]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    let facts = run.facts();
    let (first, outcome) = facts
        .iter()
        .find_map(|fact| match fact {
            TaskSemanticFact::NavigationLocated {
                attempt: 1,
                matched_pages,
                matched_total,
                outcome,
                ..
            } => Some(((matched_pages.clone(), *matched_total), outcome.clone())),
            _ => None,
        })
        .expect("the first locate");
    assert_eq!(sorted(first.0), ["menu", "shop"]);
    assert_eq!(first.1, 2);
    assert_eq!(
        outcome.code().as_str(),
        "contained_task_navigation_page_ambiguous"
    );
    assert_eq!(outcome.category(), Category::Warning);
    assert_eq!(sorted(strings(&outcome, "page_ids")), ["menu", "shop"]);
    // After the ambiguous frame the chain's step is the next layer; no route was planned.
    let after = facts
        .iter()
        .skip_while(|fact| !matches!(fact, TaskSemanticFact::NavigationLocated { .. }))
        .find_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_label,
                role,
            } => Some((package_label.clone(), *role)),
            TaskSemanticFact::NavigationRoutePlanned { .. } => {
                panic!("no route after an ambiguous frame")
            }
            _ => None,
        });
    assert_eq!(after, Some((WAKE.to_owned(), CHAIN)));
    assert_eq!(inputs(&state), 0);
}

// N5-7: the recovery chain runs out: the run ends with
// `contained_task_navigation_recovery_exhausted`, `count` the steps run, caused by the last
// locate's code (the second frame's ambiguity, not the first's unrecognised page), and nothing is
// input after it.
#[test]
fn n5_7_a_spent_chain_ends_the_run_exhausted_by_the_last_locate() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK, SHOP_MENU]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    let failure = run.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_navigation_recovery_exhausted"
    );
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(integer(&failure, "count"), Some(1));
    assert_eq!(
        failure
            .causes()
            .iter()
            .map(|link| link.outcome().code().as_str())
            .collect::<Vec<_>>(),
        ["contained_task_navigation_page_ambiguous"]
    );
    assert_eq!(
        run.located()
            .into_iter()
            .map(|(attempt, code, ..)| (attempt, code))
            .collect::<Vec<_>>(),
        [
            (1, "contained_task_navigation_page_unrecognized".to_owned()),
            (2, "contained_task_navigation_page_ambiguous".to_owned()),
        ]
    );
    assert_eq!(inputs(&state), 1);
}

// N5-8: a hop whose package ends where it says, but where the feature package does not see the
// edge's `to`, is `contained_task_navigation_hop_unconfirmed` and the run locates again; the
// 17th locate of the gate ends the run with `contained_task_navigation_limit_reached`
// (`locates`). The edge's package calls the screen `shop` its `home`, so it is done at once.
#[test]
fn n5_8_an_unconfirmed_hop_locates_again_until_sixteen_locates_are_spent() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![SHOP]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.add(&Pack {
        rules: vec![
            ("shop", vec!["first_red", "second_white"]),
            ("home", vec!["first_red", "second_black"]),
        ],
        step_timeout_ms: 50,
        ..graph(LIAR, "shop", "home", &[("go", "shop", "home")])
    });
    root.bundle().navigation(&[("shop", "home", LIAR)], &[WAKE]);
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    let failure = run.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_navigation_limit_reached"
    );
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(text(&failure, "navigation_limit"), Some("locates"));
    assert_eq!(integer(&failure, "count"), Some(17));
    assert_eq!(integer(&failure, "limit_count"), Some(16));
    let warnings = run.warnings();
    assert_eq!(warnings.len(), 16, "{warnings:?}");
    for warning in &warnings {
        assert_eq!(
            warning.code().as_str(),
            "contained_task_navigation_hop_unconfirmed"
        );
        assert_eq!(warning.category(), Category::Warning);
        assert_eq!(text(warning, "package_id"), Some(LIAR));
        assert_eq!(text(warning, "page_id"), Some("home"));
    }
    assert_eq!(
        run.located(),
        (1..=16)
            .map(|attempt| located(attempt, "contained_task_navigation_located", Some("shop")))
            .collect::<Vec<_>>()
    );
    // Each hop's check of its `to` page is recorded as a navigation target recognition.
    let targets = run
        .facts()
        .into_iter()
        .filter(|fact| {
            matches!(
                fact,
                TaskSemanticFact::EntryRecognition {
                    phase: TaskEntryRecognitionPhase::NavigationTarget,
                    matched: false,
                    ..
                }
            )
        })
        .count();
    assert_eq!(targets, 16);
    assert_eq!(inputs(&state), 0);
}

// N5-9: a package whose entry page `pages.json` does not list: a
// `contained_task_navigation_entry_unlisted` record, and otherwise the records of a run without
// navigation data.
#[test]
fn n5_9_an_entry_the_page_list_does_not_hold_is_recorded_and_not_navigated() {
    let scenario = |navigation: bool| {
        let state = Arc::new(FakeState::default());
        screens(&state, vec![DARK, DONE]);
        let temp = TempDir::new().expect("tempdir");
        let root = world(temp.path());
        if navigation {
            root.default_navigation();
        }
        let host = start(&temp, &state, &root);
        let main = Pack {
            step_timeout_ms: 200,
            max_steps: 4,
            ..graph(MAIN, "dark", "done", &[("finish", "dark", "done")])
        };
        let run = run_direct(&host, &temp, "main", &main);
        host.close().expect("close runtime host");
        run
    };
    let without = scenario(false);
    let with = scenario(true);
    assert!(without.succeeded(), "{:?}", without.receipt);
    assert!(with.succeeded(), "{:?}", with.receipt);
    let warnings = with.warnings();
    let [unlisted] = warnings.as_slice() else {
        panic!("one warning: {warnings:?}");
    };
    assert_eq!(
        unlisted.code().as_str(),
        "contained_task_navigation_entry_unlisted"
    );
    assert_eq!(unlisted.category(), Category::Warning);
    assert_eq!(strings(unlisted, "page_ids"), ["dark"]);
    assert!(with.located().is_empty() && with.routes().is_empty());
    assert!(with.admitted().is_empty());
    let comparable = |run: &Run| {
        run.events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::NavigationWarning { .. } => None,
                    fact => {
                        let mut fact = serde_json::to_value(fact).expect("fact JSON");
                        if let Some(fact) = fact.as_object_mut() {
                            fact.remove("response_deadline_monotonic_ms");
                            fact.remove("task_timing");
                            if fact.get("kind") == Some(&json!("geometry_observed")) {
                                fact.remove("observation");
                            }
                        }
                        Some(fact)
                    }
                },
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(comparable(&with), comparable(&without));
}

// How an N5-10 case lays out the edge's package `bad` in the game root.
type ArrangeEdge = Box<dyn Fn(&mut Root)>;

// N5-10: an edge's package whose entry does not hold the edge's `from`, that does not end on its
// `to`, that is an application entry, that is not eligible as a layer, of another resolution or
// that cannot be admitted ends the run with `contained_task_navigation_package_invalid`
// (`navigation_role: edge`) before any input.
#[test]
fn n5_10_an_edge_package_unfit_for_its_edge_ends_the_run_before_any_input() {
    let cases: [(&str, ArrangeEdge, &str); 6] = [
        (
            "entry without from",
            Box::new(|root: &mut Root| {
                root.add(&graph(BAD, "menu", "home", &[("go", "menu", "home")]));
            }),
            "entry_mismatch",
        ),
        (
            "terminal without to",
            Box::new(|root: &mut Root| {
                root.add(&graph(BAD, "shop", "menu", &[("go", "shop", "menu")]));
            }),
            "terminal_mismatch",
        ),
        (
            "an application entry",
            Box::new(|root: &mut Root| {
                root.add(&graph(BAD, "any", "home", &[("go", "shop", "home")]));
            }),
            "application_entry",
        ),
        (
            "recognize only",
            Box::new(|root: &mut Root| {
                root.add(&Pack {
                    mode: "recognize_only",
                    ..graph(BAD, "shop", "home", &[("see", "shop", "home")])
                });
            }),
            "role_ineligible",
        ),
        (
            "another resolution",
            Box::new(|root: &mut Root| {
                root.add(&Pack {
                    width: 32,
                    ..graph(BAD, "shop", "home", &[("go", "shop", "home")])
                });
            }),
            "identity_mismatch",
        ),
        (
            "listed, not installed",
            Box::new(|root: &mut Root| {
                root.list_only(BAD, "test");
            }),
            "admission_failed",
        ),
    ];
    for (case, arrange, problem) in cases {
        let state = Arc::new(FakeState::default());
        screens(&state, vec![SHOP]);
        let temp = TempDir::new().expect("tempdir");
        let mut root = world(temp.path());
        arrange(&mut root);
        root.bundle().navigation(&[("shop", "home", BAD)], &[WAKE]);
        let host = start(&temp, &state, &root);
        let run = run_direct(&host, &temp, "main", &graph_main());
        host.close().expect("close runtime host");

        let failure = run.failure();
        assert_eq!(
            failure.code().as_str(),
            "contained_task_navigation_package_invalid",
            "{case}"
        );
        assert_eq!(failure.category(), Category::Error, "{case}");
        assert_eq!(text(&failure, "package_id"), Some(BAD), "{case}");
        assert_eq!(text(&failure, "navigation_role"), Some("edge"), "{case}");
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
        assert!(run.admitted().is_empty(), "{case}");
        assert_eq!(inputs(&state), 0, "{case}");
    }
}

// N5-11: the feature package locates the run's entry page, and the run's package's own check of
// it does not pass (its `home` asks for a white second pixel): the run ends with
// `contained_task_navigation_entry_unconfirmed`, naming its entry pages.
#[test]
fn n5_11_an_entry_the_package_does_not_confirm_ends_the_run_unconfirmed() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![HOME]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let main = Pack {
        rules: vec![("home", vec!["first_blue", "second_white"])],
        ..graph_main()
    };
    let run = run_direct(&host, &temp, "main", &main);
    host.close().expect("close runtime host");

    let failure = run.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_navigation_entry_unconfirmed"
    );
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(strings(&failure, "page_ids"), ["home"]);
    assert_eq!(
        run.located(),
        [located(
            1,
            "contained_task_navigation_located",
            Some("home")
        )]
    );
    assert_eq!(run.routes(), [("home".to_owned(), Vec::new())]);
    assert_eq!(inputs(&state), 0);
}

// N5-12: the first locate takes the entry check's last frame (no capture between the entry's
// decision and the first locate); every later locate takes one new frame (one capture between
// the chain step's end and the second locate).
#[test]
fn n5_12_the_first_locate_takes_the_entry_check_s_frame_and_each_later_one_a_new_frame() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    let facts = run.facts();
    let position = |predicate: &dyn Fn(&TaskSemanticFact) -> bool| {
        facts
            .iter()
            .position(|fact| predicate(fact))
            .expect("the record")
    };
    let decided = position(&|fact| matches!(fact, TaskSemanticFact::EntryRecoveryDecision { .. }));
    let first =
        position(&|fact| matches!(fact, TaskSemanticFact::NavigationLocated { attempt: 1, .. }));
    let chain_done = position(
        &|fact| matches!(fact, TaskSemanticFact::EntryRecoveryCompleted { role, .. } if *role == CHAIN),
    );
    let second =
        position(&|fact| matches!(fact, TaskSemanticFact::NavigationLocated { attempt: 2, .. }));
    let captures = |from: usize, to: usize| {
        facts[from..to]
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::EvidenceIndexed { .. }))
            .count()
    };
    assert!(decided < first && first < chain_done && chain_done < second);
    assert_eq!(
        captures(decided, first),
        0,
        "the first locate captures nothing"
    );
    assert_eq!(
        captures(chain_done, second),
        1,
        "a later locate captures one frame"
    );
}

// N5-14: a shared interrupt page and a listed page on one frame: the interrupt page is handled
// first (NAV-6's handling, `interrupt_handled`), and the next locate finds the page under it.
#[test]
fn n5_14_an_interrupt_page_on_a_located_frame_is_handled_before_the_run_locates_again() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK, ALERT_SHOP, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation().interrupts(json!([
        {"page": "alert", "handlers": [CLOSE_ALERT], "then": "resume", "max_per_run": 1}
    ]));
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        run.located(),
        [
            located(1, "contained_task_navigation_page_unrecognized", None),
            (
                2,
                "contained_task_interrupt_handled".to_owned(),
                Some("alert".to_owned()),
                Some("alert".to_owned())
            ),
            located(3, "contained_task_navigation_located", Some("shop")),
        ]
    );
    let facts = run.facts();
    let handled = facts
        .iter()
        .position(|fact| matches!(fact, TaskSemanticFact::InterruptHandled { page, .. } if page == "alert"))
        .expect("the alert was handled");
    let planned = facts
        .iter()
        .position(|fact| matches!(fact, TaskSemanticFact::NavigationRoutePlanned { .. }))
        .expect("the route");
    assert!(handled < planned);
    assert_eq!(
        run.admitted(),
        [
            (WAKE.to_owned(), CHAIN),
            (
                CLOSE_ALERT.to_owned(),
                Some(TaskEntryRecoveryRole::InterruptHandler)
            ),
            (SHOP_TO_HOME.to_owned(), HOP),
        ]
    );
    assert_eq!(inputs(&state), 4);
}

// The values of every `task.page` runtime fact recorded from the first entry record of the
// run on, with the position of each among the run's task records (how many came before it).
fn task_pages(host_events: &[PersistedEvent], run: &Run) -> Vec<(usize, String)> {
    let task_sequences = run
        .events
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Task(TaskPayload::Semantic(_))
            )
        })
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
    let first = *task_sequences.first().expect("the run's records");
    host_events
        .iter()
        .filter(|event| event.sequence() > first)
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == "task.page" =>
            {
                match &recorded.record().value {
                    ContractFactValue::String(page) => Some((
                        task_sequences
                            .iter()
                            .filter(|sequence| **sequence < event.sequence())
                            .count(),
                        page.clone(),
                    )),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

// N5-15: a locate that found one page records it as `task.page`, as the page list names it,
// before any layer runs; an unrecognised or ambiguous frame records none.
#[test]
fn n5_15_task_page_is_the_located_page_and_unchanged_by_a_frame_not_located() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![MENU, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("events");
    host.close().expect("close runtime host");
    assert!(run.succeeded(), "{:?}", run.receipt);
    let facts = run.facts();
    let located_at = facts
        .iter()
        .position(|fact| matches!(fact, TaskSemanticFact::NavigationLocated { .. }))
        .expect("the locate");
    let first_layer = facts
        .iter()
        .position(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { .. }))
        .expect("the first hop");
    let pages = task_pages(&events, &run);
    let (at, first) = pages.first().expect("a task.page record").clone();
    assert_eq!(first, "neutral/menu");
    assert!(
        located_at < at && at <= first_layer,
        "recorded with the locate, before any layer: {pages:?}"
    );

    // Unrecognised, then (after the chain's step) ambiguous: no `task.page` outside the layer.
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK, SHOP_MENU]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("events");
    host.close().expect("close runtime host");
    let facts = run.facts();
    let opened = facts
        .iter()
        .position(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { .. }))
        .expect("the chain step");
    let closed = facts
        .iter()
        .position(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryCompleted { .. }))
        .expect("the chain step's end");
    let pages = task_pages(&events, &run);
    assert!(!pages.is_empty(), "the chain step's own pages");
    assert!(
        pages.iter().all(|(at, _)| opened < *at && *at <= closed),
        "only the chain step's own records: {pages:?}"
    );
}

// N5-16: the next layer's `max_steps` would take the run past 1000 steps: the run ends with
// `contained_task_navigation_limit_reached` (`steps`) and the layer does not run.
#[test]
fn n5_16_a_layer_past_a_thousand_steps_is_not_run() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![MENU, SHOP]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.add(&graph(LEAD, "menu", "shop", &[("lead", "menu", "shop")]))
        .add(&Pack {
            max_steps: 1000,
            ..graph(BIG, "shop", "home", &[("go", "shop", "home")])
        });
    root.bundle().navigation(&[("shop", "home", BIG)], &[WAKE]);
    let host = start(&temp, &state, &root);
    let main = Pack {
        prerequisite: Some(LEAD),
        ..graph_main()
    };
    let run = run_direct(&host, &temp, "main", &main);
    host.close().expect("close runtime host");

    let failure = run.failure();
    assert_eq!(
        failure.code().as_str(),
        "contained_task_navigation_limit_reached"
    );
    assert_eq!(text(&failure, "navigation_limit"), Some("steps"));
    assert_eq!(integer(&failure, "count"), Some(1001));
    assert_eq!(integer(&failure, "limit_count"), Some(1000));
    assert_eq!(
        run.admitted(),
        [(LEAD.to_owned(), Some(TaskEntryRecoveryRole::Prerequisite))]
    );
    assert_eq!(inputs(&state), 1);
}

// ---- N5-17 to N5-20.

// N5-17: a run the host starts itself (here its startup package) never navigates, and no layer
// of a navigating run (a chain step, an interrupt handler, an edge) does either.
#[test]
fn n5_17_host_started_runs_and_layers_never_navigate() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![MENU]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let main = temp.path().join("main");
    linear_main().write(&main);
    let (host, _) = scheduled_host(&temp, &state, &root, &main, false);
    host.schedule_startup_package_for_test(POLICY_INSTANCE_ALIAS)
        .expect("schedule the startup package");
    wait_until("the startup run's end", || {
        host.query_persisted_events_for_test(EventQuery::default())
            .expect("events")
            .iter()
            .any(|event| {
                matches!(
                    event.event_type(),
                    EventType::TaskCompleted | EventType::TaskFailed
                )
            })
    });
    let startup = Run {
        receipt: None,
        events: host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("events"),
    };
    host.close().expect("close runtime host");
    assert!(
        startup.navigation_records().is_empty(),
        "the startup package's gate does not navigate"
    );
    assert!(startup.admitted().is_empty());

    // A navigating run's layers: no navigation record between a layer's admission and its end.
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK, ALERT_SHOP, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation().interrupts(json!([
        {"page": "alert", "handlers": [CLOSE_ALERT], "then": "resume", "max_per_run": 1}
    ]));
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");
    assert!(run.succeeded(), "{:?}", run.receipt);
    let mut open = 0_usize;
    let mut layers = 0_usize;
    for fact in run.facts() {
        match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted { .. } => {
                open += 1;
                layers += 1;
            }
            TaskSemanticFact::EntryRecoveryCompleted { .. }
            | TaskSemanticFact::EntryRecoveryFailed { .. } => open -= 1,
            TaskSemanticFact::NavigationLocated { .. }
            | TaskSemanticFact::NavigationRoutePlanned { .. }
            | TaskSemanticFact::NavigationWarning { .. } => {
                assert_eq!(open, 0, "a navigation record inside a layer");
            }
            _ => {}
        }
    }
    assert_eq!(layers, 3, "the chain step, the handler and the edge");
}

// N5-18, the identity: a navigation failure's identity names the layer it happened in (`n`) and
// the game root's data files (`g`), after the main package and its prerequisite layers, and
// reads back; another order is not an identity. Navigation and shared interrupt failures
// accumulate in every scope but the preparation's, a restart segment included.
#[test]
fn n5_18_a_navigation_failure_identity_names_its_layer_and_data_and_accumulates() {
    let package = |package_id: &str, hash: &str| IdentityPackage {
        id_hash: id8(package_id),
        hash: hash.to_owned(),
    };
    let identity = FailureIdentity {
        base: "contained_task_navigation_package_invalid".to_owned(),
        key: "0".repeat(12),
        main: package(MAIN, "0123456789ab"),
        layers: vec![
            IdentityLayer::Declared(package(LEAD, "x")),
            IdentityLayer::Navigation(package(SHOP_TO_HOME, "456789abcdef")),
            IdentityLayer::StandardPackRoot("fedcba987654".to_owned()),
        ],
        frame: IdentityFrame::None,
    };
    let encoded = identity.encode();
    assert_eq!(
        encoded,
        format!(
            "contained_task_navigation_package_invalid~v2~k{}~m{}0123456789ab~p{}x~n{}456789abcdef~gfedcba987654~fna",
            "0".repeat(12),
            id8(MAIN),
            id8(LEAD),
            id8(SHOP_TO_HOME)
        )
    );
    assert_eq!(FailureIdentity::parse(&encoded), Some(identity));
    let misordered = encoded.replace(
        &format!("~n{}456789abcdef~gfedcba987654", id8(SHOP_TO_HOME)),
        &format!("~gfedcba987654~n{}456789abcdef", id8(SHOP_TO_HOME)),
    );
    assert_eq!(FailureIdentity::parse(&misordered), None);

    let exhausted = CodeStr::from(HostCode::ContainedTaskNavigationRecoveryExhausted.code());
    let escalated = CodeStr::from(HostCode::ContainedTaskInterruptEscalated.code());
    let unknown = CodeStr::from(SharedCode::ContainedTaskPageUnknown.code());
    assert!(navigation_failure_code(&exhausted) && navigation_failure_code(&escalated));
    assert!(!navigation_failure_code(&unknown));
    for scope in ["gate", "pre:0123abcd", "main"] {
        for restart_segment in [false, true] {
            assert!(
                failure_accumulates(&exhausted, scope, restart_segment),
                "{scope}"
            );
            assert!(
                failure_accumulates(&escalated, scope, restart_segment),
                "{scope}"
            );
        }
    }
    assert!(!failure_accumulates(&exhausted, "prepare", false));
    assert!(!failure_accumulates(&unknown, "gate", false));
}

// The fixture manifest with `procedure.observe` bound at `main`.
fn manifest_at(main: &Path) -> ProcedureManifest {
    ProcedureManifest::new(
        [
            "procedure.observe",
            "procedure.observe-b",
            "procedure.detect",
        ]
        .map(|procedure_ref| {
            let binding = ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding");
            if procedure_ref == "procedure.observe" {
                binding.with_package_path(main.to_string_lossy())
            } else {
                binding
            }
        }),
    )
    .expect("procedure manifest")
}

// A lift check of the bindings `manifest` over the registries of `root`, measuring now.
fn lift_view<'a>(
    manifest: &'a ProcedureManifest,
    root: &'a Root,
    checks: &'a LiftChecks,
) -> SuspensionLiftView<'a> {
    SuspensionLiftView::new(
        manifest,
        Some(root.root.as_path()),
        checks,
        LiftCheckTime::ONE_READ,
    )
}

// N5-18, the lift: a pause on a navigation failure lifts when the game root's data files change
// or the layer it names changes, and not when a hop's package that ran without failing changes;
// data files back as they were keep it.
#[test]
fn n5_18_a_navigation_pause_lifts_on_its_data_or_its_layer_not_on_a_hop_that_ran() {
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let main = temp.path().join("main");
    graph_main().write(&main);
    let manifest = manifest_at(&main);
    let edge = IdentityLayer::Navigation(IdentityPackage {
        id_hash: id8(SHOP_TO_HOME),
        hash: String::new(),
    });
    let checks = LiftChecks::default();
    let main_hash = lift_view(&manifest, &root, &checks)
        .current_main_hash("procedure.observe")
        .expect("the view reads the main package")
        .expect("the main package's hash");
    let edge_hash = lift_view(&manifest, &root, &checks)
        .current_layer_hash(&edge)
        .expect("the view reads the registry")
        .expect("the edge's hash");
    let root_hash = standard_pack_root_hash(&root.game).expect("the data files' hash");
    let paused = PausedPackages {
        main: Some(IdentityPackage {
            id_hash: id8(MAIN),
            hash: main_hash,
        }),
        layers: vec![
            IdentityLayer::Navigation(IdentityPackage {
                id_hash: id8(SHOP_TO_HOME),
                hash: edge_hash,
            }),
            IdentityLayer::StandardPackRoot(root_hash),
        ],
    };
    let lifted = || {
        lift_view(&manifest, &root, &LiftChecks::default())
            .lifted("procedure.observe", "decision", &paused)
            .expect("the lift check")
            .lifted
    };
    assert_eq!(lifted(), None, "nothing changed");
    // A hop's package the failure did not happen in.
    let mut changed = graph(
        MENU_TO_SHOP,
        "menu",
        "shop",
        &[("go_again", "menu", "shop")],
    );
    changed.step_timeout_ms = 500;
    changed.write(&root.game.join(MENU_TO_SHOP));
    assert_eq!(lifted(), None, "a hop that ran changed");
    // The data files.
    root.navigation(
        &[
            ("menu", "shop", MENU_TO_SHOP),
            ("shop", "home", SHOP_TO_HOME),
        ],
        &[WAKE],
    );
    assert_eq!(
        lifted(),
        Some(SuspensionLift::Layer(2)),
        "navigation.json changed"
    );
    root.default_navigation();
    assert_eq!(lifted(), None, "navigation.json as it was");
    // The layer the failure happened in.
    let mut changed = graph(
        SHOP_TO_HOME,
        "shop",
        "home",
        &[("go_again", "shop", "home")],
    );
    changed.step_timeout_ms = 500;
    changed.write(&root.game.join(SHOP_TO_HOME));
    assert_eq!(
        lifted(),
        Some(SuspensionLift::Layer(1)),
        "the failure's layer changed"
    );
}

// N5-18, the settlement: a scheduled run's navigation failure records its identity with the game
// root's data files (`g`, the hash of `pages.json`, `navigation.json` and `interrupts.json`);
// an edge package refused for its edge also names that package (`n`).
#[test]
fn n5_18_a_scheduled_navigation_failure_records_its_layer_and_data_in_its_identity() {
    // The chain runs out.
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.add(&graph(NAP, "dark", "dark", &[("nap", "stale", "dark")]));
    root.bundle()
        .navigation(&[("shop", "home", SHOP_TO_HOME)], &[NAP]);
    let main = temp.path().join("main");
    graph_main().write(&main);
    let (host, request) = scheduled_host(&temp, &state, &root, &main, false);
    let (_, result, run) = scheduled_run(&host, &request);
    let root_hash = standard_pack_root_hash(&root.game).expect("the data files' hash");
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    host.close().expect("close runtime host");
    assert_eq!(
        result.err().as_deref(),
        Some("contained_task_navigation_recovery_exhausted")
    );
    let identity = recorded_identity(&run);
    assert_eq!(
        identity.base,
        "contained_task_navigation_recovery_exhausted"
    );
    assert_eq!(
        identity.layers,
        vec![IdentityLayer::StandardPackRoot(root_hash)]
    );
    assert!(identity.accumulates(), "a navigation failure accumulates");

    // An edge's package refused for its edge.
    let state = Arc::new(FakeState::default());
    screens(&state, vec![SHOP]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.add(&graph(BAD, "menu", "home", &[("go", "menu", "home")]));
    root.bundle().navigation(&[("shop", "home", BAD)], &[WAKE]);
    let main = temp.path().join("main");
    graph_main().write(&main);
    let (host, request) = scheduled_host(&temp, &state, &root, &main, false);
    let (_, result, run) = scheduled_run(&host, &request);
    let root_hash = standard_pack_root_hash(&root.game).expect("the data files' hash");
    host.close().expect("close runtime host");
    assert_eq!(
        result.err().as_deref(),
        Some("contained_task_navigation_package_invalid")
    );
    let identity = recorded_identity(&run);
    let [
        IdentityLayer::Navigation(layer),
        IdentityLayer::StandardPackRoot(data),
    ] = identity.layers.as_slice()
    else {
        panic!("the edge and the data files: {:?}", identity.layers);
    };
    assert_eq!(layer.id_hash, id8(BAD));
    assert_ne!(layer.hash, "x", "the edge's package is measured");
    assert_eq!(*data, root_hash);
}

// N5-19: a direct run (task-run) routes as a scheduled run of the same package does: the same
// locates, route and warnings, and both complete.
#[test]
fn n5_19_a_direct_run_routes_as_a_scheduled_run_does() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![MENU, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let host = start(&temp, &state, &root);
    let direct = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    let state = Arc::new(FakeState::default());
    screens(&state, vec![MENU, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let root = world(temp.path());
    root.default_navigation();
    let main = temp.path().join("main");
    graph_main().write(&main);
    let (host, request) = scheduled_host(&temp, &state, &root, &main, false);
    let (_, result, scheduled) = scheduled_run(&host, &request);
    host.close().expect("close runtime host");

    assert!(direct.succeeded(), "{:?}", direct.receipt);
    assert!(result.is_ok(), "{result:?}");
    assert!(scheduled.succeeded());
    assert!(!direct.navigation_records().is_empty());
    assert_eq!(direct.navigation_records(), scheduled.navigation_records());
    assert_eq!(direct.admitted(), scheduled.admitted());
}

// N5-20: edges and chain steps of another server are not used: the run's server's chain step
// and edge run, though another server's are listed first.
#[test]
fn n5_20_only_the_run_s_server_s_edges_and_chain_steps_are_used() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK, SHOP, HOME, DONE]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.list_only(OTHER_EDGE, "other")
        .list_only(OTHER_WAKE, "other");
    root.bundle().navigation(
        &[
            ("shop", "home", OTHER_EDGE),
            ("shop", "home", SHOP_TO_HOME),
            ("menu", "shop", MENU_TO_SHOP),
        ],
        &[OTHER_WAKE, WAKE],
    );
    let host = start(&temp, &state, &root);
    let run = run_direct(&host, &temp, "main", &graph_main());
    host.close().expect("close runtime host");

    assert!(run.succeeded(), "{:?}", run.receipt);
    assert_eq!(
        run.admitted(),
        [(WAKE.to_owned(), CHAIN), (SHOP_TO_HOME.to_owned(), HOP)]
    );
    assert_eq!(
        run.routes(),
        [("shop".to_owned(), vec![hop("shop", "home", SHOP_TO_HOME)])]
    );
}

// ---- Wave-17 ruling 1: the ladder (host key 96, beside RD-4's T5-T7).

// NAV-5's real `contained_task_navigation_recovery_exhausted` of a scheduled run on a handset
// starts the ladder by the default trigger item `navigation_exhausted` (its code, start rung 3)
// with the instance's configured profile `handset`: under the default `exhaust` both of the
// profile's rungs are skipped before the start rung and the climb is exhausted.
#[test]
fn nav5_a_real_navigation_exhaustion_starts_the_configured_ladder_profile() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![DARK]);
    let temp = TempDir::new().expect("tempdir");
    let mut root = world(temp.path());
    root.add(&graph(NAP, "dark", "dark", &[("nap", "stale", "dark")]));
    root.bundle()
        .navigation(&[("shop", "home", SHOP_TO_HOME)], &[NAP]);
    let main = temp.path().join("main");
    graph_main().write(&main);
    let (host, request) = scheduled_host(&temp, &state, &root, &main, true);
    let (context, result, _) = scheduled_run(&host, &request);
    assert_eq!(
        result.err().as_deref(),
        Some("contained_task_navigation_recovery_exhausted")
    );
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");

    let ladders = phases(&events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted {
                trigger,
                rungs,
                profile,
                ..
            } => Some((trigger, rungs, profile)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(trigger, rungs, profile)] = ladders.as_slice() else {
        panic!("one ladder: {ladders:?}");
    };
    assert_eq!(
        trigger.run_id,
        Some(context.run_id()),
        "the failed run's ladder"
    );
    assert_eq!(
        trigger.failure_code.as_str(),
        "contained_task_navigation_recovery_exhausted"
    );
    assert_eq!(trigger.rule.as_deref(), Some("navigation_exhausted"));
    assert_eq!(trigger.matched, Some(RecoveryTriggerMatch::Code));
    assert_eq!(profile.as_deref(), Some("handset"));
    let skipped = |rung| RecoveryRungPlan {
        rung,
        state: RecoveryRungState::Skipped,
        reason: Some(RecoveryRungSkipReason::BeforeStartRung),
    };
    assert_eq!(
        *rungs,
        vec![
            skipped(RecoveryRung::ReturnHome),
            skipped(RecoveryRung::ApplicationRestart),
        ]
    );
    let finished = phases(&events)
        .into_iter()
        .find_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderFinished { ladder_result, .. } => {
                Some(ladder_result)
            }
            _ => None,
        });
    assert_eq!(finished, Some(RecoveryLadderOutcome::Exhausted));
}
