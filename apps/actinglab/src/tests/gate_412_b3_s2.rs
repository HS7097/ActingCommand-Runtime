// SPDX-License-Identifier: AGPL-3.0-only

// #412 Lab B-3 S2 (frozen design rd-1021/train-0129/lab-b3-design.md §1.2 and §5 "S2", T2-6;
// register §1.162; apps key 91): online `do --operation` refuses, before any Runtime connection,
// every form design §1.2 lists: an element id, `--tap`, `--swipe`, `--after-page`,
// `--after-timeout-ms`, `--record`, `--tap-rect`, `--projection-sequence`, `--projection-hash`,
// `--dry-run` (do's and the global one), `--scene` (the CLI form of MCP's sample scenes too) and
// `--operation` without `--capture`. Each is `validation_failed`, exit 2, its details naming the
// argument with `argument_problem` `conflicting`. No Runtime runs, and the package path names
// nothing, since a refusal comes before anything is opened. The exit table is
// `lab2_cli::operation::gate_412_b3_s2_exit`.

use super::*;

const INSTANCE: &str = "neutral.instance";
const OPERATION: &str = "op_a";

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

/// `actinglab --json --instance <instance> <global> do <before> --operation op_a <capture>
/// --package <absent> <after>`.
fn line(
    context: &Isolated,
    global: &[&str],
    before: &[&str],
    capture: bool,
    after: &[&str],
) -> Vec<String> {
    let package = context.absent("neutral-package");
    let mut words = vec!["--json", "--instance", INSTANCE];
    words.extend_from_slice(global);
    words.push("do");
    words.extend_from_slice(before);
    words.extend_from_slice(&["--operation", OPERATION]);
    if capture {
        words.push("--capture");
    }
    let mut line = words.into_iter().map(str::to_string).collect::<Vec<_>>();
    line.extend(["--package".to_string(), package]);
    line.extend(after.iter().map(|word| (*word).to_string()));
    line
}

/// The answer is the refusal naming `argument` as `conflicting`: `validation_failed`, exit 2, its
/// details' `argument` and `argument_problem` and `arguments` that one entry.
#[track_caller]
fn assert_conflicting(line: &[String], argument: &str) {
    let result = run_cli(line.to_vec(), true);
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{argument}: {shown}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("{argument} was not refused: {shown}"));
    assert_eq!(error.code, "validation_failed", "{argument}: {shown}");
    let details = error
        .details
        .as_ref()
        .unwrap_or_else(|| panic!("{argument}: the refusal names no argument: {shown}"));
    assert_eq!(details["argument"], argument, "{shown}");
    assert_eq!(details["argument_problem"], "conflicting", "{shown}");
    assert_eq!(
        details["arguments"],
        json!([{"argument": argument, "argument_problem": "conflicting"}]),
        "{shown}"
    );
}

#[test]
fn b3_s2_t2_6_do_operation_refuses_each_conflicting_form_before_connecting() {
    let context = Isolated::new();
    let scene = context.absent("neutral-scene.png");
    let hash = "a".repeat(64);
    let options: [(&str, Option<&str>); 10] = [
        ("--tap", Some("1,1")),
        ("--swipe", Some("1,1,2,2,100")),
        ("--after-page", Some("neutral/page_b")),
        ("--after-timeout-ms", Some("1000")),
        ("--record", None),
        ("--tap-rect", Some("1,1,2,2")),
        ("--projection-sequence", Some("3")),
        ("--projection-hash", Some(hash.as_str())),
        ("--dry-run", None),
        ("--scene", Some(scene.as_str())),
    ];
    for (option, value) in options {
        let mut after = vec![option];
        after.extend(value);
        assert_conflicting(&line(&context, &[], &[], true, &after), option);
    }
    assert_conflicting(
        &line(&context, &[], &["element_a"], true, &[]),
        "<element-id>",
    );
    assert_conflicting(&line(&context, &["--dry-run"], &[], true, &[]), "--dry-run");
    assert_conflicting(&line(&context, &[], &[], false, &[]), "--operation");
}
