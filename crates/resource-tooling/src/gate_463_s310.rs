// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-10 (register §1.417; contracts/standard-pack-trial.md; the design
// rd-1115/pack-stage3-design.md §2.4 and §4.2 "S3-10", with the coordinator's rulings in the work
// order rd-1115/brief-463-s310-trial-plan.md): the offline half of `actinglab standard-pack trial`.
// - `s310_the_plan_of_a_package_with_two_endings_lists_three_keys`: the plan of a `linear_steps`
//   package whose task names its target page and two ending pages lists the three outcome keys in
//   mapping order (`target`, then the two `ending`s, each with its page), the package id, the
//   first 12 hex digits of containment's own content hash of the directory, the instance and the
//   authorization as given, each declared variable with the value given (`given`) or its default
//   (`default`), the irreversible mark `not_judged` without a root, `--expect`, and the record's
//   directory `<state>/trials/<package_id>`; `sent` is false;
// - `s310_the_plan_writes_nothing`: after a plan every file and directory of the temporary tree is
//   as it was, the state directory included, which is not made;
// - `s310_the_authorization_is_required_bounded_and_shown_back`: absent or blank is
//   `trial_authorization_missing`, before the package is read (a path that does not exist); 1025
//   bytes `validation_failed` `out_of_range`, a control character `not_recognised`; 1024 bytes are
//   taken and shown back;
// - `s310_only_a_directory_is_tried`: a `.zip` and a `.json` container are
//   `trial_package_not_directory` naming the path;
// - `s310_a_given_value_is_judged_by_its_declaration`: a value off its step, of another type, an
//   option not offered, a name the package does not declare and a name given twice are
//   `validation_failed` naming `--variable`, with the variable and the judgement's word where the
//   declaration judged it; a value the declaration takes is `given`;
// - `s310_an_expectation_the_package_does_not_declare_is_refused`: `trial_expectation_unknown`
//   naming the package, the declared keys in the details;
// - `s310_the_irreversible_mark_is_judged_only_over_a_root`: over a game root whose package
//   provides a capability it marks irreversible the plan of that package is
//   `trial_irreversible_unacknowledged` (the package and the root) and, with `--irreversible`,
//   answers `marked` with the capability; a package that provides nothing such is `unmarked`; one
//   the root does not register counts as marked (the type's safe side); without a root nothing
//   is refused for it;
// - `s310_a_state_directory_in_a_repository_or_an_installed_root_is_refused`: a state directory
//   below a directory holding `.git` is `trial_state_dir_refused` `repository`, below one holding
//   `install\active.json` or `runtime\BUILD-MANIFEST.json` `installed_root`; the plan and the
//   record's writer both refuse it and write nothing;
// - `s310_a_record_is_written_once_and_read_back_by_content_and_by_package`: the writer puts a
//   record at `<state>/trials/<package_id>/<content12>-<ms>.json` and never replaces one; the
//   reader answers every record of a package in the order of their times, as written, and the
//   latest of one content; a `.part` is no record; a file of a record's name that is no record, or
//   not the one its name says, is `trial_record_unreadable`.
// No device, no Runtime, no clock read, no sleep: temporary directories and made-up packages.

use crate::codes::ResourceToolingCode;
use crate::package_directory::admission_deadline;
use crate::{
    CONTENT_PREFIX_DIGITS, MAX_AUTHORIZATION_BYTES, PACK_TRIAL_SCHEMA, PackTrialRecord,
    StandardPackTrialAnswer, StandardPackTrialRequest, TrialDeclaredEnd, TrialEndKind,
    TrialIrreversible, TrialIrreversibleBasis, TrialRunEnd, TrialValueSource, TrialVariable,
    latest_trial_record, plan_standard_pack_trial, trial_record_path, trial_records,
    write_trial_record,
};
use actingcommand_contract::{LabError, TaskOutcome};
use actingcommand_pack_containment::Containment;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
/// The `linear_steps` package tried, with two endings and two variables.
const TRIED: &str = "neutral.test.trial";
/// A page-graph package that provides an irreversible capability.
const SPEND: &str = "neutral.test.spend";
/// The capability [`SPEND`] marks irreversible.
const CAPABILITY: &str = "cap_spend";
/// The keys of the target page and of the two ending pages.
const DONE_KEY: &str = "task_done";
const SHORT_KEY: &str = "task_short";
const SPARE_KEY: &str = "task_spare";
const INSTANCE: &str = "node-a";
const AUTHORIZATION: &str = "board item 463, comment 0000000001";
const VALIDATION_FAILED: &str = "validation_failed";

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

/// A colour probe of the first pixel that tells `page`.
fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// A `scheduling_outcome` mapping of `key` on `page`.
fn mapping(key: &str, page: &str) -> Value {
    json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]})
}

/// Writes [`TRIED`] at `directory`: a `linear_steps` package of one trusted click from `home` (a
/// blue first pixel) to its target page `done` (white), its ending pages `short` (green) and
/// `spare` (red), the keys [`DONE_KEY`], [`SHORT_KEY`] and [`SPARE_KEY`] in that order, and the
/// variables `floor` (a whole number of 0 to 200 on a step of 10, default 0) and `pick` (`item_a`
/// or `item_b`, default `item_a`).
fn write_trial_package(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": TRIED,
        "execution_mode": "linear_steps",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 64, "height": 24},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 1,
        "variables": [
            {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
             "min": 0, "max": 200, "step": 10, "default": 0},
            {"name": "pick", "type": "choice", "title": {"en": "Item"},
             "options": [{"value": "item_a", "title": {"en": "Item A"}},
                         {"value": "item_b", "title": {"en": "Item B"}}],
             "default": "item_a"}
        ]
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "S3-10 fixture",
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("home", [0, 0, 255]),
            probe("done", [255, 255, 255]),
            probe("short", [0, 255, 0]),
            probe("spare", [255, 0, 0])
        ],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "done": {"required": ["done_colour"]},
            "short": {"required": ["short_colour"]},
            "spare": {"required": ["spare_colour"]}
        },
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 1,
        "scheduling_outcome": {"mappings": [
            mapping(DONE_KEY, "done"),
            mapping(SHORT_KEY, "short"),
            mapping(SPARE_KEY, "spare")
        ]},
        "operations": [{
            "id": "go", "from": "home", "to": "done",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    write_files(directory, control, task);
}

/// Writes the page-graph package `package_id` at `directory`, from `home` to `page_b`, which
/// provides [`CAPABILITY`] marked irreversible when `irreversible`.
fn write_graph_package(directory: &Path, package_id: &str, irreversible: bool) {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if irreversible {
        control["provides"] = json!([{"capability": CAPABILITY, "irreversible": true,
            "parameters": [{"name": "param_p", "options": ["opt_1", "opt_2"],
                            "default": "opt_1"}]}]);
    }
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "S3-10 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    write_files(directory, control, task);
}

fn write_files(directory: &Path, control: Value, task: Value) {
    write_json(&directory.join("control.json"), &control);
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    write_json(
        &directory.join("resources/operations/task/task.json"),
        &task,
    );
}

/// An installed game root `<temp>/<name>/neutral` whose `bundle.json` registers `packs`, each
/// written at `<root>/<package_id>` (the tried package as [`write_trial_package`] writes it, any
/// other as a page-graph package that provides [`CAPABILITY`] irreversibly).
fn game_root(temp: &TempDir, name: &str, packs: &[&str]) -> PathBuf {
    let root = temp.path().join(name).join(GAME);
    let mut listed = Vec::new();
    for package_id in packs {
        if *package_id == TRIED {
            write_trial_package(&root.join(package_id));
        } else {
            write_graph_package(&root.join(package_id), package_id, true);
        }
        listed.push(json!({
            "package_id": package_id, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
        }));
    }
    write_json(
        &root.join("bundle.json"),
        &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed}),
    );
    root
}

/// A trial of the package at `package` on [`INSTANCE`] with [`AUTHORIZATION`], its records kept
/// under `state`; nothing else given.
fn request(package: &Path, state: &Path) -> StandardPackTrialRequest {
    StandardPackTrialRequest {
        package: package.to_path_buf(),
        instance: INSTANCE.to_owned(),
        authorization: Some(AUTHORIZATION.to_owned()),
        variables: Vec::new(),
        expect: None,
        irreversible: false,
        root: None,
        state_dir: state.to_path_buf(),
    }
}

fn plan(request: &StandardPackTrialRequest) -> StandardPackTrialAnswer {
    plan_standard_pack_trial(request)
        .unwrap_or_else(|error| panic!("the plan: {}: {}", error.code, error.message))
}

fn refusal(request: &StandardPackTrialRequest) -> LabError {
    plan_standard_pack_trial(request).expect_err("the plan is refused")
}

/// The values of the refusal `error`, which must be `code` with its outcome in the details.
#[track_caller]
fn refused_with(error: &LabError, code: ResourceToolingCode) -> Value {
    assert_eq!(error.code, code.as_str(), "{}", error.message);
    let details = error.details.clone().expect("the refusal's details");
    assert_eq!(details["outcome"]["code"], code.as_str(), "{details}");
    details["outcome"]["values"].clone()
}

/// The refusal `error` is `validation_failed` naming `flag` with `problem`; its details.
#[track_caller]
fn argument_refused(error: &LabError, flag: &str, problem: &str) -> Value {
    assert_eq!(error.code, VALIDATION_FAILED, "{}", error.message);
    let details = error.details.clone().expect("the refusal's details");
    assert_eq!(details["argument"], flag, "{details}");
    assert_eq!(details["argument_problem"], problem, "{details}");
    details
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).expect("an absolute path")
}

/// Every directory and file below `directory`, each file with its bytes.
fn tree(directory: &Path, into: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
    for entry in fs::read_dir(directory).expect("a directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            tree(&path, into);
            into.insert(path, None);
        } else {
            let bytes = fs::read(&path).expect("a file");
            into.insert(path, Some(bytes));
        }
    }
}

#[test]
fn s310_the_plan_of_a_package_with_two_endings_lists_three_keys() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("checkout").join("packs").join(TRIED);
    write_trial_package(&package);
    let state = temp.path().join("state");
    let answer = plan(&StandardPackTrialRequest {
        variables: vec![("floor".to_owned(), json!(20))],
        expect: Some(SHORT_KEY.to_owned()),
        ..request(&package, &state)
    });

    assert!(!answer.sent);
    let hash = Containment::new()
        .measure_content_hash(&package, admission_deadline())
        .expect("the content hash");
    let trial = &answer.plan;
    assert_eq!(trial.package_id, TRIED);
    assert_eq!(
        trial.content_hash_prefix,
        hash[..CONTENT_PREFIX_DIGITS].to_owned()
    );
    assert_eq!(trial.package, absolute(&package).display().to_string());
    assert_eq!(trial.instance, INSTANCE);
    assert_eq!(trial.authorization, AUTHORIZATION);
    let end = |outcome_key: &str, end: TrialEndKind, page: &str| TrialDeclaredEnd {
        outcome_key: outcome_key.to_owned(),
        end,
        pages: vec![page.to_owned()],
    };
    assert_eq!(
        trial.ends,
        vec![
            end(DONE_KEY, TrialEndKind::Target, "done"),
            end(SHORT_KEY, TrialEndKind::Ending, "short"),
            end(SPARE_KEY, TrialEndKind::Ending, "spare"),
        ]
    );
    assert_eq!(
        trial.variables,
        vec![
            TrialVariable {
                name: "floor".to_owned(),
                value: json!(20),
                source: TrialValueSource::Given,
            },
            TrialVariable {
                name: "pick".to_owned(),
                value: json!("item_a"),
                source: TrialValueSource::Default,
            },
        ]
    );
    assert_eq!(trial.irreversible, TrialIrreversible::NotJudged);
    assert_eq!(trial.irreversible_basis, None);
    assert!(!trial.irreversible_acknowledged);
    assert_eq!(trial.expect.as_deref(), Some(SHORT_KEY));
    assert_eq!(
        answer.record_directory,
        absolute(&state)
            .join("trials")
            .join(TRIED)
            .display()
            .to_string()
    );
    let shown = serde_json::to_value(&answer).expect("the answer as JSON");
    assert_eq!(shown["sent"], false);
    assert_eq!(shown["plan"]["irreversible"], "not_judged");
    assert_eq!(shown["plan"]["ends"][1]["end"], "ending");
    assert_eq!(shown["plan"]["variables"][0]["source"], "given");
}

#[test]
fn s310_the_plan_writes_nothing() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("checkout").join("packs").join(TRIED);
    write_trial_package(&package);
    let root = game_root(&temp, "packages", &[TRIED, SPEND]);
    let state = temp.path().join("state");
    let mut before = BTreeMap::new();
    tree(temp.path(), &mut before);

    let answer = plan(&StandardPackTrialRequest {
        root: Some(root),
        ..request(&package, &state)
    });

    assert!(!answer.sent);
    let mut after = BTreeMap::new();
    tree(temp.path(), &mut after);
    assert_eq!(before, after, "the plan wrote or changed a file");
    assert!(!state.exists(), "the plan made the state directory");
}

#[test]
fn s310_the_authorization_is_required_bounded_and_shown_back() {
    let temp = TempDir::new().expect("a temporary directory");
    let state = temp.path().join("state");
    // Absent or blank, before anything is read: the package path does not exist.
    let missing = temp.path().join("no-such-package");
    for authorization in [None, Some(String::new()), Some(" \t ".to_owned())] {
        let error = refusal(&StandardPackTrialRequest {
            authorization: authorization.clone(),
            ..request(&missing, &state)
        });
        let values = refused_with(&error, ResourceToolingCode::TrialAuthorizationMissing);
        assert_eq!(values, json!({}), "{authorization:?}");
    }
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let too_long = "r".repeat(MAX_AUTHORIZATION_BYTES + 1);
    let error = refusal(&StandardPackTrialRequest {
        authorization: Some(too_long),
        ..request(&package, &state)
    });
    argument_refused(&error, "--authorization", "out_of_range");
    let error = refusal(&StandardPackTrialRequest {
        authorization: Some("board item\u{7}463".to_owned()),
        ..request(&package, &state)
    });
    argument_refused(&error, "--authorization", "not_recognised");

    let longest = "r".repeat(MAX_AUTHORIZATION_BYTES);
    let answer = plan(&StandardPackTrialRequest {
        authorization: Some(longest.clone()),
        ..request(&package, &state)
    });
    assert_eq!(answer.plan.authorization, longest);
}

#[test]
fn s310_only_a_directory_is_tried() {
    let temp = TempDir::new().expect("a temporary directory");
    let state = temp.path().join("state");
    for name in [format!("{TRIED}.zip"), format!("{TRIED}.json")] {
        let container = temp.path().join(&name);
        fs::write(&container, b"a container").expect("the container");
        let error = refusal(&request(&container, &state));
        let values = refused_with(&error, ResourceToolingCode::TrialPackageNotDirectory);
        assert_eq!(
            values,
            json!({"path": absolute(&container).display().to_string()}),
            "{name}"
        );
    }
}

#[test]
fn s310_a_given_value_is_judged_by_its_declaration() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let state = temp.path().join("state");
    let given = |variables: Vec<(&str, Value)>| StandardPackTrialRequest {
        variables: variables
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
        ..request(&package, &state)
    };
    for (what, variables, problem, word) in [
        (
            "a value off its step",
            vec![("floor", json!(25))],
            "not_recognised",
            Some("off_step"),
        ),
        (
            "a value of another type",
            vec![("floor", json!("20"))],
            "not_recognised",
            Some("type_changed"),
        ),
        (
            "an option the declaration does not offer",
            vec![("pick", json!("item_c"))],
            "not_recognised",
            Some("not_an_option"),
        ),
        (
            "a name the package does not declare",
            vec![("other", json!(1))],
            "not_recognised",
            None,
        ),
        (
            "a name given twice",
            vec![("floor", json!(10)), ("floor", json!(20))],
            "repeated",
            None,
        ),
    ] {
        let name = variables[0].0;
        let details = argument_refused(&refusal(&given(variables)), "--variable", problem);
        assert_eq!(details["variable_name"], name, "{what}: {details}");
        match word {
            Some(word) => assert_eq!(details["variable_problem"], word, "{what}: {details}"),
            None => assert!(
                details.get("variable_problem").is_none(),
                "{what}: {details}"
            ),
        }
    }
    let answer = plan(&given(vec![("pick", json!("item_b"))]));
    assert_eq!(
        answer.plan.variables,
        vec![
            TrialVariable {
                name: "floor".to_owned(),
                value: json!(0),
                source: TrialValueSource::Default,
            },
            TrialVariable {
                name: "pick".to_owned(),
                value: json!("item_b"),
                source: TrialValueSource::Given,
            },
        ]
    );
}

#[test]
fn s310_an_expectation_the_package_does_not_declare_is_refused() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let state = temp.path().join("state");
    let error = refusal(&StandardPackTrialRequest {
        expect: Some("task_unknown".to_owned()),
        ..request(&package, &state)
    });
    let values = refused_with(&error, ResourceToolingCode::TrialExpectationUnknown);
    assert_eq!(values, json!({"package_id": TRIED}));
    let details = error.details.expect("the details");
    assert_eq!(details["expected"], "task_unknown");
    assert_eq!(
        details["declared_outcome_keys"],
        json!([DONE_KEY, SHORT_KEY, SPARE_KEY])
    );
}

#[test]
fn s310_the_irreversible_mark_is_judged_only_over_a_root() {
    let temp = TempDir::new().expect("a temporary directory");
    let state = temp.path().join("state");
    let root = game_root(&temp, "packages", &[TRIED, SPEND]);
    let shown_root = absolute(&root).display().to_string();

    // The package that provides the capability it marks irreversible.
    let spend = temp.path().join("checkout").join("packs").join(SPEND);
    write_graph_package(&spend, SPEND, true);
    let over_root = StandardPackTrialRequest {
        root: Some(root.clone()),
        ..request(&spend, &state)
    };
    let values = refused_with(
        &refusal(&over_root),
        ResourceToolingCode::TrialIrreversibleUnacknowledged,
    );
    assert_eq!(values, json!({"package_id": SPEND, "path": shown_root}));
    let acknowledged = plan(&StandardPackTrialRequest {
        irreversible: true,
        ..over_root
    });
    assert_eq!(acknowledged.plan.irreversible, TrialIrreversible::Marked);
    assert!(acknowledged.plan.irreversible_acknowledged);
    assert_eq!(
        acknowledged.plan.irreversible_basis,
        Some(TrialIrreversibleBasis {
            root: shown_root.clone(),
            registered: true,
            capabilities: vec![CAPABILITY.to_owned()],
            unreadable_packages: Vec::new(),
        })
    );
    // Without a root the mark is not judged, and nothing is refused for it.
    let unjudged = plan(&request(&spend, &state));
    assert_eq!(unjudged.plan.irreversible, TrialIrreversible::NotJudged);

    // A package that provides nothing marked, over the same root.
    let tried = temp.path().join("checkout").join("packs").join(TRIED);
    write_trial_package(&tried);
    let unmarked = plan(&StandardPackTrialRequest {
        root: Some(root),
        ..request(&tried, &state)
    });
    assert_eq!(unmarked.plan.irreversible, TrialIrreversible::Unmarked);
    assert_eq!(
        unmarked.plan.irreversible_basis,
        Some(TrialIrreversibleBasis {
            root: shown_root,
            registered: true,
            capabilities: Vec::new(),
            unreadable_packages: Vec::new(),
        })
    );

    // A root that does not register the package counts it as marked.
    let other = game_root(&temp, "other", &[SPEND]);
    let values = refused_with(
        &refusal(&StandardPackTrialRequest {
            root: Some(other.clone()),
            ..request(&tried, &state)
        }),
        ResourceToolingCode::TrialIrreversibleUnacknowledged,
    );
    assert_eq!(
        values,
        json!({"package_id": TRIED, "path": absolute(&other).display().to_string()})
    );
}

#[test]
fn s310_a_state_directory_in_a_repository_or_an_installed_root_is_refused() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let repository = temp.path().join("repository");
    fs::create_dir_all(repository.join(".git")).expect("the repository's Git directory");
    let selected = temp.path().join("selected");
    write_json(&selected.join("install").join("active.json"), &json!({}));
    let single = temp.path().join("single");
    write_json(
        &single.join("runtime").join("BUILD-MANIFEST.json"),
        &json!({}),
    );
    let accepted = plan(&request(&package, &temp.path().join("state")));
    let record = PackTrialRecord {
        schema_version: PACK_TRIAL_SCHEMA.to_owned(),
        at_unix_ms: 1_000,
        plan: accepted.plan,
        request_id: "req-1".to_owned(),
        run_id: None,
        end: TrialRunEnd {
            outcome: TaskOutcome::Success,
            outcome_key: Some(DONE_KEY.to_owned()),
            final_page: Some("done".to_owned()),
            failure_code: None,
            failure_values: Map::new(),
        },
        matched: None,
    };
    for (state, place) in [
        (repository.join("lab").join("state"), "repository"),
        (selected.join("state"), "installed_root"),
        (single.join("tools").join("state"), "installed_root"),
    ] {
        let expected = json!({
            "path": absolute(&state).display().to_string(),
            "trial_state_place": place
        });
        let values = refused_with(
            &refusal(&request(&package, &state)),
            ResourceToolingCode::TrialStateDirRefused,
        );
        assert_eq!(values, expected, "the plan");
        let values = refused_with(
            &write_trial_record(&state, &record).expect_err("the writer refuses"),
            ResourceToolingCode::TrialStateDirRefused,
        );
        assert_eq!(values, expected, "the writer");
        assert!(!state.exists(), "{} was made", state.display());
    }
}

#[test]
fn s310_a_record_is_written_once_and_read_back_by_content_and_by_package() {
    let temp = TempDir::new().expect("a temporary directory");
    let package = temp.path().join("packs").join(TRIED);
    write_trial_package(&package);
    let state = temp.path().join("state");
    let accepted = plan(&StandardPackTrialRequest {
        expect: Some(SHORT_KEY.to_owned()),
        ..request(&package, &state)
    });
    let content = accepted.plan.content_hash_prefix.clone();
    let other_content = "0123456789ab".to_owned();
    assert_ne!(content, other_content);
    let record = |at_unix_ms: u64, prefix: &str, outcome: TaskOutcome| {
        let mut plan = accepted.plan.clone();
        plan.content_hash_prefix = prefix.to_owned();
        let succeeded = outcome == TaskOutcome::Success;
        PackTrialRecord {
            schema_version: PACK_TRIAL_SCHEMA.to_owned(),
            at_unix_ms,
            plan,
            request_id: format!("req-{at_unix_ms}"),
            run_id: Some(format!("run-{at_unix_ms}")),
            end: TrialRunEnd {
                outcome,
                outcome_key: succeeded.then(|| SHORT_KEY.to_owned()),
                final_page: succeeded.then(|| "short".to_owned()),
                failure_code: None,
                failure_values: Map::new(),
            },
            matched: Some(succeeded),
        }
    };
    let first = record(1_000, &content, TaskOutcome::Success);
    let other = record(1_500, &other_content, TaskOutcome::Success);
    let latest = record(2_000, &content, TaskOutcome::Failure);

    // Nothing tried yet.
    assert_eq!(trial_records(&state, TRIED).expect("no record"), Vec::new());
    assert_eq!(
        latest_trial_record(&state, TRIED, &content).expect("no record"),
        None
    );

    let written = write_trial_record(&state, &first).expect("the first record");
    let place = trial_record_path(&absolute(&state), TRIED, &content, 1_000);
    assert_eq!(written, place);
    assert_eq!(
        place.file_name().and_then(|name| name.to_str()),
        Some(format!("{content}-1000.json").as_str())
    );
    let bytes = fs::read(&place).expect("the record file");
    let document: Value = serde_json::from_slice(&bytes).expect("the record reads");
    assert_eq!(document["schema_version"], PACK_TRIAL_SCHEMA);
    assert_eq!(document["plan"]["authorization"], AUTHORIZATION);
    assert_eq!(document["plan"]["content_hash_prefix"], content.as_str());
    assert_eq!(document["end"]["outcome"], "success");

    // Never replaced.
    let values = refused_with(
        &write_trial_record(&state, &first).expect_err("the same record again"),
        ResourceToolingCode::TrialRecordWriteFailed,
    );
    assert_eq!(values, json!({"path": place.display().to_string()}));
    assert_eq!(fs::read(&place).expect("the record file"), bytes);

    write_trial_record(&state, &latest).expect("the latest record");
    write_trial_record(&state, &other).expect("a record of another content");
    // A `.part` a write left is no record.
    let directory = absolute(&state).join("trials").join(TRIED);
    fs::write(
        directory.join(format!("{content}-3000.json.part")),
        b"half a record",
    )
    .expect("a .part");

    assert_eq!(
        trial_records(&state, TRIED).expect("the records"),
        vec![first, other.clone(), latest.clone()]
    );
    assert_eq!(
        latest_trial_record(&state, TRIED, &content).expect("the latest of the content"),
        Some(latest)
    );
    assert_eq!(
        latest_trial_record(&state, TRIED, &other_content).expect("the other content's"),
        Some(other)
    );
    assert_eq!(
        latest_trial_record(&state, TRIED, "ffffffffffff").expect("a content never tried"),
        None
    );
    assert_eq!(
        trial_records(&state, SPEND).expect("a package never tried"),
        Vec::new()
    );

    // A file of a record's name that is no record, and one that is not the record its name says.
    for (name, bytes) in [
        (format!("{content}-4000.json"), b"not a record".to_vec()),
        (format!("{content}-5000.json"), bytes),
    ] {
        let unread = temp.path().join(format!("unread-{name}"));
        let unread_directory = unread.join("trials").join(TRIED);
        fs::create_dir_all(&unread_directory).expect("the trials directory");
        let path = unread_directory.join(&name);
        fs::write(&path, bytes).expect("the file");
        let error = trial_records(&unread, TRIED).expect_err("the reader refuses");
        let values = refused_with(&error, ResourceToolingCode::TrialRecordUnreadable);
        assert_eq!(
            values,
            json!({"path": absolute(&path).display().to_string()}),
            "{name}"
        );
        let error =
            latest_trial_record(&unread, TRIED, &content).expect_err("the latest refuses too");
        refused_with(&error, ResourceToolingCode::TrialRecordUnreadable);
    }
}
