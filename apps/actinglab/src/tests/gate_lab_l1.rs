// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L1 (G7; plan-lab-backends §4 L1 R1, R2; coordinator decision 11 of 2026-10-19; register
// §1.118; exec gate key 96): a Lab2 verb never accepts and ignores an option. Online `do --capture`
// refuses the options it does not read (`--no-wait`, `--recovery-timeout-ms`,
// `--recovery-poll-ms`, `--priority`, `--lease-id`, `--destructive`, `--allow-destructive`) with
// `validation_failed` (exit 2) before any Runtime connection, its details naming them. The options
// no path of a verb reads are gone from its declaration (`do`: `--no-wait`, `--recovery-*`,
// `--priority`, `--lease-id`; `ensure`: `--no-wait`, `--recovery-*`, `--lease-id`; `observe`:
// `--require-fresh`) and refused on every path. Every path of `observe`, `do`, `ensure` and `wait`
// refuses an option it does not read: `unknown` when no path of the verb reads it, `conflicting`
// when another path does. Every expected value comes from those requirement lines; no Runtime
// runs, and every package path names nothing, since a refusal comes before anything is opened.

use super::*;

const INSTANCE: &str = "neutral.instance";

/// One option as it is typed: its name and, for a valued option, its value.
type Typed = (&'static str, Option<&'static str>);

/// The options `do` declared and no path of it reads (R1, R2).
const DO_REMOVED: [Typed; 5] = [
    ("--no-wait", None),
    ("--recovery-timeout-ms", Some("1000")),
    ("--recovery-poll-ms", Some("100")),
    ("--priority", Some("high")),
    ("--lease-id", Some("lease_a")),
];
/// The destructive guard, which online `do --capture` does not read (R1).
const DESTRUCTIVE_GUARD: [Typed; 2] = [("--allow-destructive", None), ("--destructive", None)];
/// The options `ensure` declared and no path of it reads (R2; decision 11 for `--lease-id`).
const ENSURE_REMOVED: [Typed; 4] = [
    ("--no-wait", None),
    ("--recovery-timeout-ms", Some("1000")),
    ("--recovery-poll-ms", Some("100")),
    ("--lease-id", Some("lease_a")),
];

/// A context with no configuration, no recording session and an empty Runtime state root. The
/// fields drop in order, the environment lock last.
struct Isolated {
    files: TempDir,
    _runtime_env: RuntimeStateEnvGuard,
    _runtime: TempDir,
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
        let runtime = TempDir::new().unwrap();
        let runtime_env = use_runtime_state_root(runtime.path());
        Self {
            _guard: guard,
            _app_env: app_env,
            _runtime_env: runtime_env,
            _runtime: runtime,
            files: TempDir::new().unwrap(),
        }
    }

    /// An absolute path naming nothing.
    fn absent(&self, name: &str) -> String {
        self.files.path().join(name).display().to_string()
    }
}

/// The command lines of each path of the four verbs, before the option under test: global
/// options, the verb and what the path needs.
fn path_lines(context: &Isolated) -> Vec<(&'static str, Vec<String>)> {
    let package = context.absent("neutral-package");
    let scene = context.absent("neutral-scene.png");
    let line = |words: &[&str]| -> Vec<String> {
        ["--json", "--instance", INSTANCE]
            .iter()
            .chain(words)
            .map(|word| (*word).to_string())
            .chain(["--package".to_string(), package.clone()])
            .collect()
    };
    let with_scene = |words: &[&str]| {
        let mut words = line(words);
        words.extend(["--scene".to_string(), scene.clone()]);
        words
    };
    vec![
        ("observe --scene", with_scene(&["observe"])),
        ("observe --capture", line(&["observe", "--capture"])),
        ("do --scene", with_scene(&["--dry-run", "do", "element_a"])),
        (
            "do --capture --dry-run",
            line(&["--dry-run", "do", "element_a", "--capture"]),
        ),
        ("do --capture", line(&["do", "--tap", "1,1", "--capture"])),
        (
            "ensure --scene",
            with_scene(&["--dry-run", "ensure", "page_a"]),
        ),
        (
            "ensure --capture --dry-run",
            line(&["--dry-run", "ensure", "page_a", "--capture"]),
        ),
        ("ensure --capture", line(&["ensure", "page_a", "--capture"])),
        ("wait --scene", with_scene(&["wait", "--page", "page_a"])),
        (
            "wait --capture",
            line(&["wait", "--page", "page_a", "--capture"]),
        ),
    ]
}

fn line_of(paths: &[(&'static str, Vec<String>)], path: &str) -> Vec<String> {
    paths
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, line)| line.clone())
        .unwrap_or_else(|| panic!("no command line for {path}"))
}

fn with_options(mut line: Vec<String>, options: &[Typed]) -> Vec<String> {
    for (name, value) in options {
        line.push((*name).to_string());
        line.extend(value.map(str::to_string));
    }
    line
}

/// The answer is the Lab L1 refusal naming exactly `refused` (option, argument_problem):
/// `validation_failed`, exit 2, its details' `argument` and `argument_problem` one of them and
/// `arguments` all of them.
#[track_caller]
fn assert_refused(line: &[String], refused: &[(&str, &str)], what: &str) {
    let result = run_cli(line.to_vec(), true);
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{what}: {shown}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("{what} was not refused: {shown}"));
    assert_eq!(error.code, "validation_failed", "{what}: {shown}");
    let details = error
        .details
        .as_ref()
        .unwrap_or_else(|| panic!("{what}: the refusal names no option: {shown}"));
    let pair = |entry: &Value| {
        (
            entry["argument"].as_str().unwrap_or_default().to_string(),
            entry["argument_problem"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    };
    let expected = refused
        .iter()
        .map(|(argument, problem)| ((*argument).to_string(), (*problem).to_string()))
        .collect::<BTreeSet<_>>();
    let entries = details["arguments"]
        .as_array()
        .unwrap_or_else(|| panic!("{what}: no arguments list: {shown}"));
    let named = entries.iter().map(pair).collect::<BTreeSet<_>>();
    assert_eq!(entries.len(), refused.len(), "{what}: {shown}");
    assert_eq!(named, expected, "{what}: {shown}");
    assert!(expected.contains(&pair(details)), "{what}: {shown}");
}

#[test]
fn l1_online_do_refuses_each_option_it_does_not_read_before_any_runtime_connection() {
    let context = Isolated::new();
    let online = line_of(&path_lines(&context), "do --capture");
    for option in DO_REMOVED {
        assert_refused(
            &with_options(online.clone(), &[option]),
            &[(option.0, "unknown")],
            &format!("do --capture {}", option.0),
        );
    }
    for option in DESTRUCTIVE_GUARD {
        assert_refused(
            &with_options(online.clone(), &[option]),
            &[(option.0, "conflicting")],
            &format!("do --capture {}", option.0),
        );
    }
    // All seven at once: the details name every one.
    let every = DO_REMOVED
        .iter()
        .chain(&DESTRUCTIVE_GUARD)
        .copied()
        .collect::<Vec<_>>();
    let mut refused = DO_REMOVED
        .iter()
        .map(|(name, _)| (*name, "unknown"))
        .collect::<Vec<_>>();
    refused.extend(
        DESTRUCTIVE_GUARD
            .iter()
            .map(|(name, _)| (*name, "conflicting")),
    );
    assert_refused(
        &with_options(online, &every),
        &refused,
        "do --capture with all seven",
    );
}

#[test]
fn l1_removed_options_are_refused_on_every_path_of_their_verb() {
    let context = Isolated::new();
    let paths = path_lines(&context);
    for path in ["do --scene", "do --capture --dry-run", "do --capture"] {
        for option in DO_REMOVED {
            assert_refused(
                &with_options(line_of(&paths, path), &[option]),
                &[(option.0, "unknown")],
                &format!("{path} {}", option.0),
            );
        }
    }
    for path in [
        "ensure --scene",
        "ensure --capture --dry-run",
        "ensure --capture",
    ] {
        for option in ENSURE_REMOVED {
            assert_refused(
                &with_options(line_of(&paths, path), &[option]),
                &[(option.0, "unknown")],
                &format!("{path} {}", option.0),
            );
        }
    }
    for path in ["observe --scene", "observe --capture"] {
        assert_refused(
            &with_options(line_of(&paths, path), &[("--require-fresh", None)]),
            &[("--require-fresh", "unknown")],
            &format!("{path} --require-fresh"),
        );
    }
}

#[test]
fn l1_schema_and_capabilities_no_longer_declare_the_removed_options() {
    let _context = Isolated::new();
    let removed: [(&str, &[&str]); 3] = [
        (
            "do",
            &[
                "--no-wait",
                "--recovery-timeout-ms",
                "--recovery-poll-ms",
                "--priority",
                "--lease-id",
            ],
        ),
        (
            "ensure",
            &[
                "--no-wait",
                "--recovery-timeout-ms",
                "--recovery-poll-ms",
                "--lease-id",
            ],
        ),
        ("observe", &["--require-fresh"]),
    ];
    let declares = |options: &Value, flag: &str| {
        options.as_array().is_some_and(|options| {
            options
                .iter()
                .filter_map(Value::as_str)
                .any(|option| option == flag || option.starts_with(&format!("{flag} ")))
        })
    };
    let capabilities = run_cli(["--json", "capabilities"], true);
    assert_eq!(
        capabilities.exit_code(),
        0,
        "{}",
        capabilities.envelope_json()
    );
    let verbs = capabilities
        .envelope
        .data
        .as_ref()
        .and_then(|data| data.pointer("/lab2_cli/verbs"))
        .and_then(Value::as_array)
        .cloned()
        .expect("lab2 verbs");
    for (verb, flags) in removed {
        let schema = run_cli(["--json", "schema", verb], true);
        assert_eq!(schema.exit_code(), 0, "{}", schema.envelope_json());
        let schema = schema.envelope.data.clone().expect("schema");
        let summary = verbs
            .iter()
            .find(|summary| summary["command"] == verb)
            .unwrap_or_else(|| panic!("capabilities lists no {verb}"));
        for &flag in flags {
            for (place, declared) in [
                ("schema", &schema["parameters"]["optional"]),
                ("schema", &schema["parameters"]["required"]),
                ("capabilities", &summary["optional"]),
                ("capabilities", &summary["required"]),
            ] {
                assert!(!declares(declared, flag), "{place} {verb} declares {flag}");
            }
        }
    }
}

#[test]
fn l1_every_path_refuses_an_option_no_path_of_its_verb_reads() {
    let context = Isolated::new();
    let pages = context.absent("neutral.pages.json");
    for (path, mut line) in path_lines(&context) {
        line.extend(["--pages".to_string(), pages.clone()]);
        assert_refused(&line, &[("--pages", "unknown")], &format!("{path} --pages"));
    }
}

#[test]
fn l1_a_path_refuses_an_option_only_another_path_of_its_verb_reads() {
    let context = Isolated::new();
    let paths = path_lines(&context);
    for (path, option) in [
        ("observe --capture", ("--test-capture-delay-ms", Some("25"))),
        ("do --scene", ("--tap", Some("1,1"))),
        (
            "do --capture --dry-run",
            ("--projection-sequence", Some("3")),
        ),
        ("do --capture", ("--test-capture-delay-ms", Some("25"))),
        ("ensure --scene", ("--step-timeout-ms", Some("100"))),
        ("ensure --capture --dry-run", ("--poll-ms", Some("100"))),
        ("wait --capture", ("--test-capture-delay-ms", Some("25"))),
    ] {
        assert_refused(
            &with_options(line_of(&paths, path), &[option]),
            &[(option.0, "conflicting")],
            &format!("{path} {}", option.0),
        );
    }
}
