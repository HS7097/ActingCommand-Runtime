// SPDX-License-Identifier: AGPL-3.0-only

//! #411 B37, the entry part (slice A35-S4; design `rd-1021/train-0129/a35-design.md` §2.2 "Entry
//! extension", §5.1 S4 T1 and T2, §5.3; coordinator decision Q5 of 10-21): an entry layer that
//! completes on a scheduling outcome mapping its package marks `busy` ends the run with
//! `contained_task_entry_busy` (warning, ladder class `planned_stop`; `package_id`, `page_id`,
//! `outcome_key`). In a home entry it replaces `contained_task_home_recovery_terminal_non_home`,
//! and under the defaults no ladder is admitted; in a navigation recovery chain the next chain
//! step is neither admitted nor run.
//!
//! A synthetic `neutral` game; the fake device's first pixel is the screen, and it shows the
//! `activity` page throughout: an activity that ends by itself. The return-home package knows that
//! page and maps it to its busy key (`still_running`); its other key (`returned`) maps `home`. The
//! run's own package enters on `home`. This is A35's aspect seen from the entry: the instance is
//! busy when a run starts, and today the entry layer's busy end goes on to a ladder (home entry)
//! or to the chain's next step, which may restart the application. Existing tests missed it: no
//! entry layer of theirs declares a `busy` mapping (`busy` came with A35-S1, whose tests run
//! return-home only as a ladder rung).

use super::gate_373_nav3a::{ALIAS, Run, StandardRoot, screens, start};
use super::gate_387_d1c::ladder_manifest;
use super::instance_workers::{admit_ladder_run, ladder_setup_with_package, phases};
use super::*;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{RuntimeLifecyclePhase, TaskEntryRecoveryRole};
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

const MAIN: &str = "neutral.test.main";
const RETURN_HOME: &str = "neutral.test.return_home";
const RESTART: &str = "neutral.test.restart";
const FEATURES: &str = "neutral.test.features";

/// The return-home package's two outcome keys: home reached, and the activity still running.
const RETURNED: &str = "returned";
const STILL_RUNNING: &str = "still_running";

/// The code a busy entry layer ends the run with (design §5.3).
const ENTRY_BUSY: &str = "contained_task_entry_busy";
/// The page the return-home package ends on, as the run records it.
const ACTIVITY_PAGE: &str = "neutral/activity";
/// The screen of the activity page.
const ACTIVITY: [u8; 3] = [255, 0, 255];

fn colour(page: &str) -> [u8; 3] {
    match page {
        "away" => [255, 0, 0],
        "home" => [0, 0, 255],
        "shop" => [0, 255, 0],
        "done" => [255, 255, 255],
        "activity" => ACTIVITY,
        other => panic!("no screen {other}"),
    }
}

// ---- Packages.

/// A package's files: its control and its one task (schema `0.9`, control `Lab-1y.control.v2`)
/// with the task members `members` over `pages`, each page recognised by its first pixel.
fn package(
    package_id: &str,
    mode: &str,
    pages: &[&str],
    members: Value,
) -> Vec<(&'static str, Value)> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "A35-S4 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": pages
            .iter()
            .map(|page| json!({
                "id": format!("first_{page}"),
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                "expected": colour(page)
            }))
            .collect::<Vec<_>>(),
        "page_rules": pages
            .iter()
            .map(|page| ((*page).to_owned(), json!({"required": [format!("first_{page}")]})))
            .collect::<serde_json::Map<_, _>>()
    });
    for (member, value) in members.as_object().expect("task members") {
        task[member.as_str()] = value.clone();
    }
    vec![
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ]
}

/// One click from `from` to `to`.
fn click(id: &str, from: &str, to: &str) -> Value {
    json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    })
}

/// The run's own package: a page graph from its entry set {home} to `done`.
fn main_package() -> Vec<(&'static str, Value)> {
    package(
        MAIN,
        "navigable_route",
        &["home", "done"],
        json!({
            "entry_page": ["home"],
            "target_page": "done",
            "operations": [click("open", "home", "done")]
        }),
    )
}

/// The return-home package: from the screen as it is (`any`), it ends on `home` (`returned`) or
/// on `activity`, mapped `busy` (`still_running`); it leaves `away` for `home`.
fn busy_return_home() -> Vec<(&'static str, Value)> {
    package(
        RETURN_HOME,
        "navigable_route",
        &["away", "home", "activity"],
        json!({
            "entry_page": "any",
            "target_page": ["home", "activity"],
            "scheduling_outcome": {"mappings": [
                {"outcome_key": RETURNED, "effect": "no_designated_effect",
                 "terminal_pages": ["home"]},
                {"outcome_key": STILL_RUNNING, "effect": "no_designated_effect",
                 "terminal_pages": ["activity"], "busy": true}
            ]},
            "operations": [click("back", "away", "home")]
        }),
    )
}

/// An application entry package: it restarts the application from any screen, to `home`.
fn restart_package() -> Vec<(&'static str, Value)> {
    package(
        RESTART,
        "navigable_route",
        &["home"],
        json!({
            "entry_page": "any",
            "target_page": "home",
            "operations": [{
                "id": "restart",
                "from": "any",
                "to": "home",
                "application": {"action": "restart"}
            }]
        }),
    )
}

/// The feature package of `pages.json`: a `recognize_only` package with exactly its pages.
fn features_package() -> Vec<(&'static str, Value)> {
    package(
        FEATURES,
        "recognize_only",
        &["home", "shop"],
        json!({
            "entry_page": "home",
            "target_page": "home",
            "operations": [click("see_shop", "shop", "home")]
        }),
    )
}

fn write_files(directory: &Path, files: &[(&'static str, Value)]) {
    for (path, value) in files {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("package file parent")).expect("package dir");
        fs::write(
            path,
            serde_json::to_vec_pretty(value).expect("package JSON"),
        )
        .expect("write package file");
    }
}

/// The same files in one ZIP container (a source package), for a scheduled run.
fn zip_files(files: &[(&'static str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec_pretty(value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

// ---- Outcomes.

/// The outcome of the run's committed terminal among `facts`.
fn terminal_outcome<'a>(facts: impl IntoIterator<Item = &'a TaskSemanticFact>) -> Outcome {
    facts
        .into_iter()
        .find_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                outcome: Some(outcome),
                ..
            } => Some(outcome.clone()),
            _ => None,
        })
        .expect("the run's failed terminal")
}

/// Design §5.3: `contained_task_entry_busy` is a warning and names the busy layer's package, the
/// page it ended on and its busy key.
fn assert_entry_busy(outcome: &Outcome) {
    assert_eq!(outcome.code().as_str(), ENTRY_BUSY, "{outcome:?}");
    assert_eq!(outcome.category(), Category::Warning, "{outcome:?}");
    assert_eq!(
        json!({
            "package_id": outcome.value("package_id"),
            "page_id": outcome.value("page_id"),
            "outcome_key": outcome.value("outcome_key")
        }),
        json!({
            "package_id": RETURN_HOME,
            "page_id": ACTIVITY_PAGE,
            "outcome_key": STILL_RUNNING
        }),
        "{outcome:?}"
    );
}

// ---- T1: the home entry.

/// T1 (design §5.1 S4 T1, §2.2 "Entry extension"; Q5): a scheduled run under the shipped ladder
/// defaults, whose request binds no recovery package, starts away from its entry {home}; its home
/// entry runs the registry's return-home package, which completes on `activity`, its busy key. The
/// run fails `contained_task_entry_busy` (warning) with the return-home package, `activity` and
/// `still_running`, not `contained_task_home_recovery_terminal_non_home`; its ladder class matches
/// no default trigger item, so no ladder is admitted: no ladder record, and nothing holds or
/// waits for the instance as a ladder once the run returned.
#[test]
fn b37_entry_t1_a_busy_return_home_layer_ends_the_run_and_admits_no_ladder() {
    let temp = TempDir::new().expect("tempdir");
    let (host_config, _) = ladder_setup_with_package(temp.path(), &zip_files(&main_package()));
    let request = ContainedTaskRequest::new(
        temp.path()
            .join("physical-scheduled-task.zip")
            .to_string_lossy()
            .into_owned(),
    )
    .expect("scheduled request without a recovery binding");
    let mut root = StandardRoot::new(temp.path());
    write_files(&root.list(RETURN_HOME), &busy_return_home());
    root.return_home(RETURN_HOME);
    root.write_bundle();
    let state = Arc::new(FakeState::default());
    screens(&state, vec![ACTIVITY]);
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config
            .with_standard_packs_root(Some(root.root.clone()))
            .with_config_manifest(ladder_manifest(json!({}))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let context = admit_ladder_run(&host, &request);
    let run_id = context.run_id();

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    let (holder, queued) = host
        .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
        .expect("instance claims");
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close runtime host");

    assert!(!error.is_fatal(), "{}", error.code());
    let terminal = terminal_outcome(
        events
            .iter()
            .filter(|event| event.links().run_id() == Some(&run_id))
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
                _ => None,
            }),
    );
    assert_entry_busy(&terminal);
    let ladders = phases(&events)
        .into_iter()
        .filter(|(_, phase)| {
            matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                    | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
            )
        })
        .count();
    assert_eq!(ladders, 0, "no ladder is admitted under the defaults");
    assert!(
        holder != Some(ClaimKind::Ladder) && !queued.contains(&ClaimKind::Ladder),
        "no ladder holds or waits for the instance"
    );
}

// ---- T2: the navigation recovery chain.

/// The game root of T2: the feature package, the return-home package and the application entry
/// package, listed; `pages.json` ({home, shop}) and `navigation.json` (no edges; the recovery
/// chain the return-home package, then the application entry package).
fn navigation_root(base: &Path) -> StandardRoot {
    let mut root = StandardRoot::new(base);
    write_files(&root.list(FEATURES), &features_package());
    write_files(&root.list(RETURN_HOME), &busy_return_home());
    write_files(&root.list(RESTART), &restart_package());
    root.write_bundle();
    let game = root.root.join("neutral");
    for (file, value) in [
        (
            "pages.json",
            json!({
                "schema_version": "actingcommand.pages.v1",
                "game": "neutral",
                "features": [FEATURES],
                "pages": [
                    {"id": "home", "kind": "page"},
                    {"id": "shop", "kind": "page"}
                ]
            }),
        ),
        (
            "navigation.json",
            json!({
                "schema_version": "actingcommand.navigation.v1",
                "game": "neutral",
                "edges": [],
                "recovery_chain": [RETURN_HOME, RESTART]
            }),
        ),
    ] {
        fs::write(
            game.join(file),
            serde_json::to_vec_pretty(&value).expect("root file"),
        )
        .expect("write root file");
    }
    root
}

/// Runs the run's own package, written at `<temp>/main`, directly.
fn run_direct(host: &RuntimeHost, temp: &TempDir) -> Run {
    let path = temp.path().join("main");
    write_files(&path, &main_package());
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

/// T2 (design §5.1 S4 T2, §1.7 "Navigation recovery chain", §2.2 "Entry extension"): with
/// navigation data, the run's entry {home} does not pass and the first locate finds no listed
/// page, so the recovery chain's first step runs, the return-home package, which completes on
/// `activity`, its busy key. The run ends `contained_task_entry_busy` (warning) with the
/// return-home package, `activity` and `still_running`; the chain's next step, the application
/// entry package, is neither admitted (the only layer is the return-home package) nor run (no
/// application effect).
#[test]
fn b37_entry_t2_a_busy_recovery_chain_step_ends_the_run_before_the_next_step() {
    let state = Arc::new(FakeState::default());
    screens(&state, vec![ACTIVITY]);
    let temp = TempDir::new().expect("tempdir");
    let root = navigation_root(temp.path());
    let host = start(&temp, &state, Some(root.root.clone()));

    let run = run_direct(&host, &temp);
    host.close().expect("close runtime host");

    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        run.receipt
    );
    let facts = run.facts();
    assert_entry_busy(&terminal_outcome(facts.iter().copied()));
    let admitted = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_label,
                role,
                ..
            } => Some((package_label.clone(), *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        admitted,
        vec![(
            RETURN_HOME.to_owned(),
            Some(TaskEntryRecoveryRole::RecoveryChain)
        )],
        "the chain's next step is not admitted"
    );
    assert_eq!(
        state.application_count.load(Ordering::Acquire),
        0,
        "no application effect"
    );
}
