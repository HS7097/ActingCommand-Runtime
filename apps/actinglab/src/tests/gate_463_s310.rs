// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-10 (register §1.417; contracts/standard-pack-trial.md; coordinator work order
// rd-1115/brief-463-s310-trial-plan.md, ruling 1): `actinglab standard-pack trial` on the command
// line.
// - `s310_without_plan_the_line_is_refused_before_anything_is_read`: a line without `--plan` is
//   `validation_failed` (exit 2) naming `--plan` `required`, whether it names a package that does
//   not exist or nothing at all, and nothing is made; a flag the row does not take is refused
//   naming it (`unknown`) first.
// - `s310_the_command_line_answers_the_plan_and_sends_nothing`: with `--plan` the line answers the
//   plan of a made-up package (`sent` false, the package id, the instance as `--instance` gives
//   it, the authorization, the variable given, the irreversible mark `not_judged`, the record's
//   directory under `--state-dir`) and makes nothing: neither the state directory nor any file of
//   the Runtime's state root it points at; `--variable` is parsed as the scan replay's (a value
//   that is not `<name>=<json>` `not_recognised`, a name given twice `repeated`), and an
//   `--authorization` without its value is `trial_authorization_missing`.
// No Runtime, no device, no clock read, no sleep: temporary directories and a made-up package.

use super::*;

/// The made-up package's id.
const PACKAGE_ID: &str = "neutral.test.trial";

/// A context with no configuration and no recording session, and a directory for the package,
/// the state directory and the Runtime's state root. The fields drop in order, the environment
/// lock last.
struct Isolated {
    files: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Isolated {
    fn new() -> Self {
        let guard = env_lock();
        let app_env = set_isolated_app_env();
        set_missing_config_env();
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
        }
        Self {
            _guard: guard,
            _app_env: app_env,
            files: TempDir::new().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.files.path().join(name)
    }

    /// The package [`PACKAGE_ID`] as a directory `packs/<id>`: a page-graph route from `home` (a
    /// blue first pixel) to `page_b` (a red one), its end `task_done`, and the variable `floor`, a
    /// whole number of 0 to 200 on a step of 10.
    fn package(&self) -> PathBuf {
        let directory = self.path("packs").join(PACKAGE_ID);
        let probe = |page: &str, expected: [u8; 3]| {
            json!({
                "id": format!("{page}_colour"),
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                "expected": expected
            })
        };
        let control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": PACKAGE_ID,
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2,
            "variables": [{"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
                           "min": 0, "max": 200, "step": 10, "default": 0}]
        });
        let task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
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
            "scheduling_outcome": {"mappings": [{
                "outcome_key": "task_done",
                "effect": "no_designated_effect",
                "terminal_pages": ["page_b"]
            }]},
            "operations": [{
                "id": "go", "from": "home", "to": "page_b",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true, "retryable": false
            }]
        });
        for (relative, value) in [
            ("control.json", control),
            (
                "resources/operations/resources.json",
                json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
            ),
            ("resources/operations/task/task.json", task),
        ] {
            let path = directory.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }
        directory
    }
}

/// `actinglab --json standard-pack trial`, then `words`.
fn trial(words: &[&str]) -> CliResult {
    let line = ["--json", "standard-pack", "trial"]
        .into_iter()
        .chain(words.iter().copied())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    run_cli(line, true)
}

/// The answer is the refusal `code` (exit 2) with no data; its details.
#[track_caller]
fn refused(result: &CliResult, code: &str) -> Value {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{shown}");
    assert!(result.envelope.data.is_none(), "{shown}");
    let error = result.envelope.error.as_ref().expect("a refusal");
    assert_eq!(error.code, code, "{shown}");
    error.details.clone().unwrap_or(Value::Null)
}

/// The answer is `validation_failed` naming `argument` as `problem`.
#[track_caller]
fn argument_refused(result: &CliResult, argument: &str, problem: &str) {
    let details = refused(result, "validation_failed");
    assert_eq!(details["argument"], argument, "{details}");
    assert_eq!(details["argument_problem"], problem, "{details}");
}

#[test]
fn s310_without_plan_the_line_is_refused_before_anything_is_read() {
    let context = Isolated::new();
    let missing = context.path("no-such-package");
    let state = context.path("state");
    let missing_text = missing.display().to_string();
    let state_text = state.display().to_string();

    argument_refused(
        &trial(&[
            "--instance",
            "node-a",
            "--package",
            &missing_text,
            "--authorization",
            "board item 463",
            "--state-dir",
            &state_text,
        ]),
        "--plan",
        "required",
    );
    // Nothing at all: still the plan first, before the instance, the package or the authorization.
    argument_refused(&trial(&[]), "--plan", "required");
    // `--plan false` is no plan.
    argument_refused(
        &trial(&["--plan", "false", "--state-dir", &state_text]),
        "--plan",
        "required",
    );
    // A flag the row does not take is refused by name before that.
    argument_refused(&trial(&["--frobnicate", "x"]), "--frobnicate", "unknown");
    assert!(!missing.exists());
    assert!(!state.exists(), "the refused line made the state directory");
}

#[test]
fn s310_the_command_line_answers_the_plan_and_sends_nothing() {
    let context = Isolated::new();
    let package = context.package();
    let package_text = package.display().to_string();
    let state = context.path("state");
    let state_text = state.display().to_string();
    let runtime_root = context.path("runtime-state");
    fs::create_dir_all(&runtime_root).unwrap();
    let _runtime = use_runtime_state_root(&runtime_root);
    let line = |extra: &[&str]| {
        let mut words = vec![
            "--instance",
            "node-a",
            "--package",
            package_text.as_str(),
            "--state-dir",
            state_text.as_str(),
            "--plan",
        ];
        words.extend_from_slice(extra);
        trial(&words)
    };

    let answered = line(&[
        "--authorization",
        "board item 463, comment 0000000001",
        "--variable",
        "floor=20",
    ]);
    let shown = answered.envelope_json();
    assert_eq!(answered.exit_code(), 0, "{shown}");
    let data = answered.envelope.data.clone().expect("the plan");
    assert_eq!(data["sent"], false, "{shown}");
    let plan = &data["plan"];
    assert_eq!(plan["package_id"], PACKAGE_ID, "{shown}");
    assert_eq!(plan["instance"], "node-a", "{shown}");
    assert_eq!(
        plan["authorization"], "board item 463, comment 0000000001",
        "{shown}"
    );
    assert_eq!(
        plan["package"],
        std::path::absolute(&package)
            .unwrap()
            .display()
            .to_string()
            .as_str(),
        "{shown}"
    );
    assert_eq!(
        plan["variables"],
        json!([{"name": "floor", "value": 20, "source": "given"}]),
        "{shown}"
    );
    assert_eq!(
        plan["ends"],
        json!([{"outcome_key": "task_done", "end": "target", "pages": ["page_b"]}]),
        "{shown}"
    );
    assert_eq!(plan["irreversible"], "not_judged", "{shown}");
    assert_eq!(
        data["record_directory"],
        std::path::absolute(&state)
            .unwrap()
            .join("trials")
            .join(PACKAGE_ID)
            .display()
            .to_string()
            .as_str(),
        "{shown}"
    );
    assert!(!state.exists(), "the plan made the state directory");
    assert_eq!(
        fs::read_dir(&runtime_root).unwrap().count(),
        0,
        "the plan touched the Runtime's state root"
    );

    // `--variable` as the scan replay parses it.
    let variable = |text: &str| line(&["--authorization", "board item 463", "--variable", text]);
    argument_refused(&variable("floor"), "--variable", "not_recognised");
    argument_refused(&variable("floor=ten"), "--variable", "not_recognised");
    argument_refused(
        &line(&[
            "--authorization",
            "board item 463",
            "--variable",
            "floor=20",
            "--variable",
            "floor=30",
        ]),
        "--variable",
        "repeated",
    );
    // An `--authorization` without its value names no authorization.
    refused(&line(&["--authorization"]), "trial_authorization_missing");
    refused(&line(&[]), "trial_authorization_missing");
    assert!(!state.exists());
}
