// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-2b (register §1.287, "Slice LAB-2b"; contracts/lab-two-entries.md §8.2, §9.3
// and open point 10; coordinator work order rd-1108/arch-013/brief-lab2b.md and the coordinator's
// ruling of 2026-11-10 on the two results that name `outcome`): actinglab's own commands print the
// CLI module's line.
// - `own_commands_print_the_cli_line`: with `--json` an own command prints one line: its result's
//   members flat beside `outcome` (`lab_operation_completed`, success), or `{outcome (its own code,
//   error, the values the catalog registers for it), message, details (whole)}`; warnings stay a
//   member of the result; the global options before or after the words print the same line; the
//   exit codes are 0, 1 and 2 only. Fix 1 (the review of PR #1556): a failure whose details carry
//   an outcome of its own code gives the line that outcome's values, every value the catalog
//   requires among them, for every such failure found; what the hotplace kit and the resource
//   repositories read of `standard-pack check` and `standard-pack compose` is at the top level of
//   the line; `capabilities` reports the exit codes of the line it answers.
// - `own_commands_print_for_people_into_a_pipe`: without `--json`, with stdout redirected (the
//   hotplace kit's `standard-pack check`), the text for people, never JSON; fix 1: a missing
//   option value too, while a `--via` line keeps its envelope.
// - `mcp_child_lines_keep_the_envelope`: a `--json --via mcp …` line prints through `main`'s path
//   the envelope the parse-and-run path prints, byte for byte, with its exit codes 2 to 6 (the
//   consumer goldens pin it against main's code, `tests/consumer_goldens.rs`).
// - `status_and_run_show_take_their_core_rows`: the two words reach the CLI module's core rows on
//   the command line, actinglab's old implementations are gone, and the command line answers what
//   `serve --stdio` answers, `status` with its `watchdog` block. Fix 2 (the two cases of the old
//   `status` tests): with no Runtime running `status` exits 0 with its `daemon` block offline,
//   class `runtime`, `runtime_unavailable`, no connection opened; `--runtime-endpoint` with an
//   untrusted remote endpoint is refused before anything is sent, exit 1, after the words by the
//   core row (`actingctl_usage_invalid`) and before them by actinglab's own dispatch.
// - `own_results_never_name_outcome`: no own command's line carries a top-level `outcome` other
//   than its own; `resource verify` and `run summary` carry their documents whole under `report`
//   and `summary`; a result that names `outcome` without a declared member is refused, never
//   flattened.
// - `do_check_failure_exits_one`: `do`'s `lab_operation_check_failed` exits 1 on the command line
//   and 5 with `--via mcp`.
// Fakes only: an empty configuration in a temporary folder, offline fixtures; no Runtime, no
// device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, own_line, serve};
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::outcome::OUTCOME_SCHEMA_VERSION;
use actingcommand_contract::{CorrelationId, EffectDisposition, RuntimeReceiptState};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::LabOperationAnswer;
use actingcommand_runtime_client::test_support::lab_operation_check_failure;

const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
const CORRELATION: &str = "correlation_fedcba9876543210fedcba9876543210";
const COMPLETED: &str = "lab_operation_completed";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The paths of the offline fixture, as command-line words.
struct Fixture {
    repo: String,
    root: String,
    not_a_root: String,
    missing: String,
    runs: String,
}

/// A resource repository with one declaration and a changed-path list, NAV-T's installed game
/// root, a file that is no root, a package that is not there and a run root.
fn fixture(temp: &TempDir) -> Fixture {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        repo.join("declaration-exclusions.json"),
        br#"{"schema_version":"actingcommand.declaration-exclusions.v1","directories":["regression"]}"#,
    )
    .unwrap();
    let root = crate::standard_pack_cli::gate_373_navt::game_root(
        temp,
        crate::standard_pack_cli::gate_373_navt::HOME_TO_SHOP,
    );
    let not_a_root = temp.path().join("not-a-root");
    fs::write(&not_a_root, b"no game root\n").unwrap();
    let runs = temp.path().join("runs");
    fs::create_dir_all(&runs).unwrap();
    // The packs directory is there; its package `missing` is not.
    fs::create_dir_all(temp.path().join("packs")).unwrap();
    let text = |path: &Path| path.to_str().expect("a UTF-8 path").to_owned();
    Fixture {
        repo: text(repo.as_path()),
        root: text(root.as_path()),
        not_a_root: text(not_a_root.as_path()),
        missing: text(temp.path().join("packs").join("missing").as_path()),
        runs: text(runs.as_path()),
    }
}

/// What `main` prints for actinglab's own command line `line` (no `--via`), stdout redirected:
/// its exit and its stdout text.
fn printed_own(line: &[String]) -> (Exit, String) {
    assert!(!own_line::via_line(line), "{line:?}");
    let invocation = match entry(line.to_vec(), own_line::envelope_by_default(line, true)) {
        Ok(invocation) => invocation,
        Err(module) => {
            panic!("{line:?} is actinglab's own command line, not the module's {module:?}")
        }
    };
    let (exit, json) = own_line::answer(invocation);
    let text = cli_module::stdout_text(&exit, json)
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (exit, text)
}

/// The one JSON line `text` is.
fn one_line(text: &str) -> Value {
    assert!(!text.contains('\n'), "one line: {text}");
    serde_json::from_str(text).unwrap_or_else(|error| panic!("no JSON line ({error}): {text}"))
}

/// The members of `line`, in order.
fn names(line: &Value) -> Vec<String> {
    line.as_object()
        .expect("a line is an object")
        .keys()
        .cloned()
        .collect()
}

/// The values the outcome catalog requires for `code`.
fn required_values(code: &str) -> Vec<String> {
    actingcommand_contract::outcome::catalog_json()
        .expect("the embedded outcome catalog")
        .get("codes")
        .and_then(|codes| codes.get(code))
        .and_then(|entry| entry.get("values"))
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter(|(_, presence)| presence.as_str() == Some("required"))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Fix 1: the failure line `line` of `case`, whose details carry an outcome of the line's own code
/// (the details are one, or their member `outcome` is one): the line's values are that outcome's,
/// every value the catalog requires for the code among them, and the details are whole.
fn assert_nested_values(case: &str, line: &Value) {
    let code = line["outcome"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("{case}: no code: {line}"));
    assert_eq!(line["outcome"]["category"], "error", "{case}: {line}");
    let details = &line["details"];
    let nested = if details["code"] == code {
        details
    } else {
        &details["outcome"]
    };
    assert_eq!(
        nested["code"], code,
        "{case}: the details carry no outcome of {code}: {line}"
    );
    assert!(nested["values"].is_object(), "{case}: {line}");
    assert_eq!(
        line["outcome"]["values"], nested["values"],
        "{case}: {line}"
    );
    for name in required_values(code) {
        assert!(
            line["outcome"]["values"].get(name.as_str()).is_some(),
            "{case}: {code} requires {name}: {line}"
        );
    }
}

/// `bytes` at `path`, its folders made first.
fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("a parent folder")).unwrap();
    fs::write(path, bytes).unwrap();
}

/// An installed game root `<directory>/neutral/` whose only root file is `bundle.json`, listing
/// `packs`, each a package directory below the root (the resource tooling's E1 fixture).
fn listing_root(directory: &Path, packs: &[&str]) -> PathBuf {
    let root = directory.join("neutral");
    fs::create_dir_all(&root).unwrap();
    let listed = packs
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": "test", "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    write_file(
        &root.join("bundle.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.bundle.v4", "game": "neutral", "packs": listed
        }))
        .unwrap(),
    );
    root
}

/// The package `package_id` below `root` whose `task.json` breaks off inside its third line, so
/// its admission refuses it (the resource tooling's E1 T3 fixture).
fn refused_package(root: &Path, package_id: &str) {
    let pack = root.join(package_id);
    write_file(
        &pack.join("control.json"),
        &serde_json::to_vec_pretty(&json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": package_id,
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        }))
        .unwrap(),
    );
    write_file(
        &pack.join("resources/operations/resources.json"),
        &serde_json::to_vec_pretty(
            &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        )
        .unwrap(),
    );
    write_file(
        &pack.join("resources/operations/task/task.json"),
        b"{\n  \"schema_version\": \"0.9\",\n  \"task_id\": \"ta",
    );
}

#[test]
fn own_commands_print_the_cli_line() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let paths = fixture(&temp);
    let mut codes = BTreeSet::new();

    // A success, the global options before and after the words: the result's members flat
    // beside the line's own outcome, as the result's data holds them.
    let validate = words_of(&[
        "--json",
        "resource",
        "validate",
        "--repo",
        paths.repo.as_str(),
        "--changed-path",
        "declaration-exclusions.json",
    ]);
    let validate_after = words_of(&[
        "resource",
        "validate",
        "--repo",
        paths.repo.as_str(),
        "--changed-path",
        "declaration-exclusions.json",
        "--json",
    ]);
    let (exit, text) = printed_own(&validate);
    assert_eq!(printed_own(&validate_after).1, text);
    codes.insert(exit.code);
    assert_eq!(exit.code, 0, "{text}");
    assert!(exit.developer.is_none(), "{text}");
    let line = one_line(&text);
    assert_eq!(line["outcome"]["schema_version"], OUTCOME_SCHEMA_VERSION);
    assert_eq!(line["outcome"]["code"], COMPLETED);
    assert_eq!(line["outcome"]["category"], "success");
    assert_eq!(line["entries"][0]["status"], "valid", "{line}");
    let data = run_invocation(parse_invocation(validate.iter().cloned(), false))
        .envelope
        .data
        .expect("the result");
    let mut flat = data.as_object().expect("an object result").clone();
    flat.insert("outcome".to_owned(), line["outcome"].clone());
    assert_eq!(line, Value::Object(flat));

    // A failure, `--run-root` before and after the words: its outcome, its message and its
    // details nested whole; exit 1 and the developer line.
    let preflight = words_of(&[
        "--run-root",
        paths.runs.as_str(),
        "--json",
        "package",
        "preflight",
        "--package",
        paths.missing.as_str(),
    ]);
    let preflight_after = words_of(&[
        "package",
        "preflight",
        "--package",
        paths.missing.as_str(),
        "--run-root",
        paths.runs.as_str(),
        "--json",
    ]);
    let (exit, text) = printed_own(&preflight);
    assert_eq!(printed_own(&preflight_after).1, text);
    codes.insert(exit.code);
    assert_eq!(exit.code, 1, "{text}");
    let line = one_line(&text);
    let error = run_invocation(parse_invocation(preflight.iter().cloned(), false))
        .envelope
        .error
        .expect("the refusal");
    assert_eq!(names(&line), ["message", "details", "outcome"]);
    assert_eq!(line["outcome"]["code"], json!(error.code));
    assert_eq!(line["outcome"]["code"], "package_invalid");
    assert_eq!(line["outcome"]["category"], "error");
    assert_eq!(line["message"], json!(error.message));
    assert_eq!(Some(&line["details"]), error.details.as_ref());
    let details = &line["details"];
    for (name, value) in line["outcome"]["values"].as_object().expect("values") {
        assert_eq!(&details[name.as_str()], value, "{name}: {line}");
    }
    let developer = exit.developer.expect("the failure's developer line");
    assert!(
        developer
            .line(cli_module::PROGRAM)
            .starts_with("ERROR actinglab: package_invalid"),
        "{}",
        developer.line(cli_module::PROGRAM)
    );

    // An unknown word and refused short words: actinglab's own usage error, exit 1.
    for words in [
        &["--json", "frobnicate"][..],
        &["--json", "app", "launch"][..],
    ] {
        let (exit, text) = printed_own(&words_of(words));
        codes.insert(exit.code);
        assert_eq!(exit.code, 1, "{words:?}: {text}");
        let line = one_line(&text);
        assert_eq!(line["outcome"]["code"], "validation_failed", "{words:?}");
        assert_eq!(line["outcome"]["values"], json!({}), "{words:?}");
        assert!(line["message"].is_string(), "{words:?}");
    }
    let (_, refused) = printed_own(&words_of(&["--json", "app", "launch"]));
    assert_eq!(
        one_line(&refused)["details"],
        json!({"argument": "<instance>", "argument_problem": "required"})
    );

    // Warnings a result carries stay its member in their own shape; the category stays success
    // and no developer line is written. A result that is no object goes under `result`.
    let warned = json!({
        "status": "written",
        "warnings": [{"code": "resource_pool_producer_missing", "values": {"path": "pools.json"}}]
    });
    let exit = own_line::success("package bundle", warned.clone());
    codes.insert(exit.code);
    let line = exit.line.clone().expect("the line");
    assert_eq!(line["warnings"], warned["warnings"]);
    assert_eq!(line["outcome"]["category"], "success");
    assert!(exit.developer.is_none());
    let text = own_line::success("schema", json!("neutral text"));
    assert_eq!(
        names(text.line.as_ref().expect("the line")),
        ["result", "outcome"]
    );
    assert_eq!(text.line.unwrap()["result"], "neutral text");

    // Fix 1: what the hotplace kit and the resource repositories read of `standard-pack check
    // --json`, and the hotplace kit of `standard-pack compose --json`, at the top level of the line
    // (the check's members `gate_434_lab2::consumer_command_lines_take_the_old_path_unchanged` read
    // from the envelope's `data` before LAB-2b).
    let (exit, text) = printed_own(&words_of(&[
        "--json",
        "standard-pack",
        "check",
        "--root",
        paths.root.as_str(),
    ]));
    codes.insert(exit.code);
    assert_eq!(exit.code, 0, "{text}");
    let line = one_line(&text);
    assert_eq!(line["outcome"]["code"], COMPLETED, "{line}");
    assert_eq!(line["status"], "checked", "{line}");
    assert_eq!(line["pack_count"], 4, "{line}");
    assert_eq!(
        line["root_files"],
        json!(["bundle.json", "pages.json", "navigation.json"]),
        "{line}"
    );
    let warnings = line["warnings"].as_array().expect("the check's warnings");
    assert_eq!(warnings.len(), 1, "{line}");
    assert_eq!(
        warnings[0]["code"], "package_bundle_entry_page_unlisted",
        "{line}"
    );
    let composing = TempDir::new().unwrap();
    let packs = composing.path().join("standard-packs");
    let game = packs.join(super::gate_386_rlt::GAME);
    write_file(
        &game.join("bundle.json"),
        &serde_json::to_vec_pretty(&super::gate_386_rlt::bundle()).unwrap(),
    );
    write_file(
        &game.join("schedule.json"),
        &serde_json::to_vec_pretty(&super::gate_386_rlt::schedule()).unwrap(),
    );
    let binding = |instance: &str| {
        format!(
            "{instance}={}/{}",
            super::gate_386_rlt::GAME,
            super::gate_386_rlt::SERVER
        )
    };
    let (binding_a, binding_b) = (
        binding(super::gate_386_rlt::INSTANCE_A),
        binding(super::gate_386_rlt::INSTANCE_B),
    );
    let packs_text = packs.to_str().expect("a UTF-8 path").to_owned();
    let out = composing.path().join("composed");
    let out_text = out.to_str().expect("a UTF-8 path").to_owned();
    let (exit, text) = printed_own(&words_of(&[
        "--json",
        "standard-pack",
        "compose",
        "--root",
        packs_text.as_str(),
        "--instance",
        binding_a.as_str(),
        "--instance",
        binding_b.as_str(),
        "--out",
        out_text.as_str(),
    ]));
    codes.insert(exit.code);
    assert_eq!(exit.code, 0, "{text}");
    let line = one_line(&text);
    assert_eq!(line["outcome"]["code"], COMPLETED, "{line}");
    assert_eq!(line["status"], "composed", "{line}");
    assert_eq!(
        [
            &line["tasks"],
            &line["pools"],
            &line["profiles"],
            &line["events"]
        ],
        [&json!(2), &json!(2), &json!(4), &json!(2)],
        "{line}"
    );
    let games = line["games"].as_array().expect("the bound games");
    assert_eq!(games.len(), 1, "{line}");
    assert_eq!(games[0]["game_id"], super::gate_386_rlt::GAME, "{line}");
    assert_eq!(
        games[0]["instances"],
        json!([
            super::gate_386_rlt::INSTANCE_A,
            super::gate_386_rlt::INSTANCE_B
        ]),
        "{line}"
    );
    assert_eq!(
        line["manifest"][0]["package_id"],
        super::gate_386_rlt::PACKAGE,
        "{line}"
    );
    assert!(
        line["rules_digest"]
            .as_str()
            .is_some_and(|digest| !digest.is_empty()),
        "{line}"
    );
    assert_eq!(line["warnings"], json!([]), "{line}");

    // Fix 1: every failure of actinglab's own commands whose details carry an outcome of its own
    // code (the details are one, or their member `outcome` is one) gives the line that outcome's
    // values, every value the catalog requires among them, with the details whole. Each is run
    // where an offline fixture reaches it, else built by the code that builds it, else as its
    // site builds it.
    let mut nested: Vec<(&str, u8, Value)> = Vec::new();
    let verify = |repo: &str, work: &str, report: &str| {
        words_of(&[
            "--json",
            "resource",
            "verify",
            "--repo",
            repo,
            "--source-repository",
            "neutral-owner/neutral-resources",
            "--work",
            work,
            "--report",
            report,
        ])
    };
    let path_text = |path: PathBuf| path.to_str().expect("a UTF-8 path").to_owned();
    let report = path_text(temp.path().join("verify-report.json"));
    // `resource verify` (`resource_verify.rs`, `outcome_error`): an existing `--work`, refused
    // before anything is read; the details are the outcome itself.
    let (exit, text) = printed_own(&verify(&paths.repo, &paths.runs, &report));
    nested.push((
        "resource verify of an existing --work",
        exit.code,
        one_line(&text),
    ));
    // `resource verify`: a `--repo` that is no Git checkout fails the checkout precondition; the
    // details are the report, whose own `outcome` is the precondition's.
    let not_a_checkout = temp.path().join("not-a-checkout");
    fs::create_dir_all(&not_a_checkout).unwrap();
    let (exit, text) = printed_own(&verify(
        &path_text(not_a_checkout),
        &path_text(temp.path().join("verify-work")),
        &report,
    ));
    let line = one_line(&text);
    assert_eq!(
        line["details"]["schema_version"], "actinglab.resource-verify.v1",
        "{line}"
    );
    nested.push(("resource verify of no checkout", exit.code, line));
    // `standard-pack check` (resource tooling `standard_pack_check.rs`): an edge to the feature
    // package (`navigation_package_invalid`), a package its admission refuses
    // (`package_refused`), a folder with no root file (`root_invalid`) and a `bundle.json` that
    // lists no pack (`package_directory.rs`, `packs_empty`): each `{outcome}`.
    let navigation = TempDir::new().unwrap();
    let roots = TempDir::new().unwrap();
    let refused_root = listing_root(&roots.path().join("refused"), &["neutral.test.task"]);
    refused_package(&refused_root, "neutral.test.task");
    let no_root = roots.path().join("missing").join("neutral");
    fs::create_dir_all(&no_root).unwrap();
    for (case, root) in [
        (
            "standard-pack check of an edge to a feature package",
            crate::standard_pack_cli::gate_373_navt::game_root(
                &navigation,
                crate::standard_pack_cli::gate_373_navt::FEATURES,
            ),
        ),
        ("standard-pack check of a refused package", refused_root),
        ("standard-pack check of no root file", no_root),
        (
            "standard-pack check of no pack",
            listing_root(&roots.path().join("empty"), &[]),
        ),
    ] {
        let root = path_text(root);
        let (exit, text) = printed_own(&words_of(&[
            "--json",
            "standard-pack",
            "check",
            "--root",
            root.as_str(),
        ]));
        nested.push((case, exit.code, one_line(&text)));
    }
    // An offline replay without an OCR model (crates/lab `ocr.rs`, `unavailable_error`, as
    // `offline_ocr` refuses with it; here the provider of no installation): `{outcome}` with the
    // OCR target.
    let ocr = actingcommand_lab::LabOcr::from_installation(None)
        .unavailable_error(Some("fixture/ocr"))
        .expect("no OCR model without an installation");
    let exit = own_line::exit(&CliResult::err("replay".to_owned(), ocr, true));
    let code = exit.code;
    let line = exit.line.expect("the line");
    assert_eq!(
        line["outcome"]["values"]["target_id"], "fixture/ocr",
        "{line}"
    );
    nested.push(("replay without an OCR model", code, line));
    // A draw whose touch keys cannot be read (crates/lab `touch.rs`, `unreadable`, as `lab_draw`
    // refuses with it): `{outcome}` with the key.
    let touch = actingcommand_lab::LabTouch::from_runtime_configuration(
        br#"{"touch": {"sampling": {"sigma_ratio_milli": 0}}}"#,
    )
    .sampling()
    .expect_err("a touch key outside its range");
    let exit = own_line::exit(&CliResult::err("replay".to_owned(), touch, true));
    let code = exit.code;
    let line = exit.line.expect("the line");
    assert_eq!(
        line["outcome"]["values"]["config_key"], "touch.sampling.sigma_ratio_milli",
        "{line}"
    );
    nested.push(("replay with an unreadable touch key", code, line));
    // Built as their sites build them, which need a Lab recording, a loading table or a failing
    // disk: `record mark`'s click that follows no template (crates/lab `recording/follow.rs`,
    // `follow_invalid`: `{step, outcome}`), `package loading`'s refused table (resource tooling
    // `package_loading.rs`, `refused`: `{outcome, issues}`) and `standard-pack compose`'s failed
    // write (resource tooling `standard_pack_schedule.rs`, `write_failed`: `{outcome}`).
    let follow = json!({
        "schema_version": OUTCOME_SCHEMA_VERSION,
        "code": "record_click_follow_invalid",
        "category": "error",
        "values": {
            "mark_id": "neutral_mark",
            "follow_problem": "not_template",
            "stage": "record_click_follow_check"
        }
    });
    let issue = json!({
        "schema_version": OUTCOME_SCHEMA_VERSION,
        "code": "standard_pack_root_invalid",
        "category": "error",
        "values": {
            "game": "neutral",
            "path": "loading.json",
            "standard_pack_problem": "loading_invalid",
            "json_pointer": "/packages/0"
        }
    });
    let written = actingcommand_contract::outcome::Outcome::from_io_error(
        &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        actingcommand_contract::outcome::IoOp::Write,
    )
    .envelope(actingcommand_contract::outcome::Detail::Full);
    for (case, command, code, details) in [
        (
            "record mark of a click that follows no template",
            "record mark",
            "record_click_follow_invalid",
            json!({"step": 1, "outcome": follow}),
        ),
        (
            "package loading of a refused table",
            "package loading",
            "standard_pack_root_invalid",
            json!({"outcome": issue.clone(), "issues": [issue]}),
        ),
        (
            "standard-pack compose of a failed write",
            "standard-pack compose",
            "foreign_os_error",
            json!({"outcome": written}),
        ),
    ] {
        let error = actingcommand_contract::LabError::new(
            actingcommand_contract::LabErrorClass::UsageValidation,
            code,
            format!("{case} (fixture)"),
            &[],
        )
        .with_details(details);
        let exit = own_line::exit(&CliResult::err(command.to_owned(), error, true));
        nested.push((case, exit.code, exit.line.expect("the line")));
    }
    let mut covered = BTreeSet::new();
    for (case, code, line) in &nested {
        codes.insert(*code);
        assert_eq!(*code, 1, "{case}: {line}");
        assert_nested_values(case, line);
        covered.insert(
            line["outcome"]["code"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        );
    }
    for code in [
        "resource_verify_output_exists",
        "package_bundle_navigation_package_invalid",
        "package_invalid",
        "standard_pack_root_invalid",
        "package_bundle_packs_empty",
        "lab_ocr_unavailable",
        "config_value_invalid",
        "record_click_follow_invalid",
        "foreign_os_error",
    ] {
        assert!(covered.contains(code), "{code}: {covered:?}");
    }

    // Fix 1, the coordinator's ruling: `capabilities` reports the exit codes of the line it
    // answers: 0, 1 and 2 on this line; with `--via` (MCP's child) the envelope's classes, as
    // before.
    let (exit, text) = printed_own(&words_of(&["--json", "capabilities"]));
    codes.insert(exit.code);
    assert_eq!(exit.code, 0, "{text}");
    let line = one_line(&text);
    let line_codes = json!([
        {"exit_code": 0, "meaning": "ok"},
        {"exit_code": 1, "meaning": "error"},
        {"exit_code": 2, "meaning": "fatal"}
    ]);
    assert_eq!(line["exit_codes"], line_codes, "{line}");
    assert_eq!(line["lab2_cli"]["exit_codes"], line_codes, "{line}");
    let via = run_cli(["--json", "--via", "mcp", "capabilities"], true);
    let data = via
        .envelope
        .data
        .as_ref()
        .expect("capabilities through --via");
    let envelope_codes = json!([
        {"exit_code": 0, "meaning": "ok"},
        {"exit_code": 2, "meaning": "usage_or_validation"},
        {"exit_code": 3, "meaning": "safety_blocked"},
        {"exit_code": 4, "meaning": "device_or_instance"},
        {"exit_code": 5, "meaning": "runtime_not_running"},
        {"exit_code": 6, "meaning": "not_implemented_or_scheduler_not_available"}
    ]);
    assert_eq!(data["exit_codes"], envelope_codes);
    assert_eq!(data["lab2_cli"]["exit_codes"], envelope_codes);

    assert!(codes.iter().all(|code| *code <= 2), "{codes:?}");
    set_missing_config_env();
}

#[test]
fn own_commands_print_for_people_into_a_pipe() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let paths = fixture(&temp);

    // The hotplace kit's line: no `--json`, stdout redirected. No envelope by default any more.
    let check = words_of(&["standard-pack", "check", "--root", paths.root.as_str()]);
    assert!(!own_line::envelope_by_default(&check, true));
    let (exit, text) = printed_own(&check);
    assert_eq!(exit.code, 0, "{text}");
    assert_eq!(text, cli_module::people(exit.line.as_ref().unwrap()));
    assert!(text.starts_with("lab_operation_completed\n"), "{text}");
    assert!(
        text.lines()
            .any(|row| row.trim_start().starts_with("status") && row.ends_with("checked")),
        "{text}"
    );
    assert!(serde_json::from_str::<Value>(&text).is_err(), "{text}");

    // A failure for people: the code, then its message.
    let failing = words_of(&[
        "standard-pack",
        "check",
        "--root",
        paths.not_a_root.as_str(),
    ]);
    let (exit, text) = printed_own(&failing);
    assert_eq!(exit.code, 1, "{text}");
    assert_eq!(
        text,
        format!(
            "validation_failed\n  message  --root {} is no directory",
            paths.not_a_root
        )
    );
    assert!(exit.developer.is_some());

    // A `--via` line keeps its envelope by default when stdout is no terminal.
    let via = words_of(&[
        "--via",
        "mcp",
        "standard-pack",
        "check",
        "--root",
        paths.root.as_str(),
    ]);
    assert!(own_line::envelope_by_default(&via, true));
    assert!(!own_line::envelope_by_default(&via, false));

    // Fix 1: a missing option value follows the rule too: the text for people without `--json`,
    // the line with it; a `--via` line keeps its envelope, at a terminal too, as before.
    let missing = words_of(&["standard-pack", "check", "--run-root"]);
    let (exit, text) = printed_own(&missing);
    assert_eq!(exit.code, 1, "{text}");
    assert!(serde_json::from_str::<Value>(&text).is_err(), "{text}");
    assert!(text.starts_with("validation_failed"), "{text}");
    assert!(text.contains("missing value for --run-root"), "{text}");
    let (exit, text) = printed_own(&words_of(&[
        "--json",
        "standard-pack",
        "check",
        "--run-root",
    ]));
    assert_eq!(exit.code, 1, "{text}");
    let line = one_line(&text);
    assert_eq!(line["outcome"]["code"], "validation_failed", "{line}");
    assert_eq!(line["message"], "missing value for --run-root", "{line}");
    let via_missing = words_of(&["--via", "mcp", "standard-pack", "check", "--run-root"]);
    let (text, code) = match entry(
        via_missing.clone(),
        own_line::envelope_by_default(&via_missing, false),
    ) {
        Ok(invocation) => enveloped(&run_invocation(invocation)),
        Err(module) => panic!("{via_missing:?} is actinglab's own command line: {module:?}"),
    };
    assert_eq!(code, 2, "{text}");
    let envelope: Value = serde_json::from_str(&text).expect("the envelope");
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(envelope["error"]["code"], "validation_failed", "{envelope}");
    assert_eq!(
        envelope["error"]["message"], "missing value for --run-root",
        "{envelope}"
    );
    set_missing_config_env();
}

#[test]
fn mcp_child_lines_keep_the_envelope() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let paths = fixture(&temp);
    // `record mark`'s answer through `--json --via mcp` (its `initiator`) is pinned by
    // `gate_434_lab2::recording_steps_and_marks_carry_the_initiator`, `do`'s exit 5 by
    // `do_check_failure_exits_one`.
    let lines: [(&[&str], i32); 3] = [
        (
            &[
                "--json",
                "--via",
                "mcp",
                "resource",
                "validate",
                "--repo",
                paths.repo.as_str(),
                "--changed-path",
                "declaration-exclusions.json",
            ],
            0,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "package",
                "check",
                "--package",
                paths.missing.as_str(),
            ],
            2,
        ),
        // Without `--json` into a pipe, the envelope as before.
        (
            &[
                "--via",
                "mcp",
                "package",
                "check",
                "--package",
                paths.missing.as_str(),
            ],
            2,
        ),
    ];
    for (line, code) in lines {
        let line = words_of(line);
        assert!(own_line::via_line(&line), "{line:?}");
        let piped = own_line::envelope_by_default(&line, true);
        assert!(piped, "{line:?}");
        let through_main = match entry(line.clone(), piped) {
            Ok(invocation) => enveloped(&run_invocation(invocation)),
            Err(module) => panic!("{line:?} is actinglab's own command line: {module:?}"),
        };
        let without_route = enveloped(&run_invocation(parse_invocation(
            line.iter().cloned(),
            true,
        )));
        assert_eq!(through_main, without_route, "{line:?}");
        assert_eq!(
            i32::from(through_main.1),
            code,
            "{line:?}: {}",
            through_main.0
        );
        let envelope: Value = serde_json::from_str(&through_main.0)
            .unwrap_or_else(|error| panic!("{line:?}: no envelope ({error}): {}", through_main.0));
        assert_eq!(envelope["schema_version"], CLI_SCHEMA_VERSION, "{envelope}");
        let ok = envelope["ok"].as_bool().expect("ok");
        assert_eq!(ok, code == 0, "{envelope}");
        assert!(
            envelope.get(if ok { "data" } else { "error" }).is_some(),
            "{envelope}"
        );
        assert!(envelope.get("outcome").is_none(), "{envelope}");
    }
    set_missing_config_env();
}

#[test]
fn status_and_run_show_take_their_core_rows() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    // `main`'s route gives both words to the CLI module, `--json` anywhere.
    for line in [
        &["status"][..],
        &["--json", "status"][..],
        &["status", "--json"][..],
        &["run", "show", REQUEST][..],
        &["--json", "run", "show", REQUEST][..],
    ] {
        match entry(words_of(line), false) {
            Err(module) => assert_eq!(module, words_of(line)),
            Ok(own) => panic!("{line:?} is the CLI module's core row, not {own:?}"),
        }
    }
    assert!(!own_command(&words_of(&["status"])));
    assert!(!own_command(&words_of(&["run", "show"])));
    assert!(own_command(&words_of(&["run", "summary"])));
    // actinglab's old implementations are gone: its own dispatch knows neither word.
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    for (line, message) in [
        (
            &["--json", "status"][..],
            "unknown actinglab command: status",
        ),
        (
            &["--json", "run", "show", REQUEST][..],
            "unknown run command: show",
        ),
    ] {
        let old = run_cli(line.iter().copied(), false);
        let error = old.envelope.error.as_ref().expect("no own command");
        assert_eq!(
            (error.code.as_str(), error.message.as_str()),
            ("validation_failed", message)
        );
    }

    // The command line answers what `serve --stdio` answers for the same row, `status` with its
    // `watchdog` block (null without an install root). No Runtime runs on the state root.
    let state_root = temp.path().join("state");
    fs::create_dir_all(&state_root).unwrap();
    unsafe {
        env::set_var(RUNTIME_STATE_ROOT_ENV, &state_root);
    }
    let table = cli_module::table().expect("the operation table");
    let input = format!(
        "{}\n{}\n",
        json!({"op": "status", "params": {}}),
        json!({"op": "run show", "params": {"id": REQUEST}})
    );
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut developer).expect("the loop ends");
    let output = String::from_utf8(output).unwrap();
    let served = output.lines().collect::<Vec<_>>();
    assert_eq!(served.len(), 2, "{output}");
    let status = cli_module::run_line(&words_of(&["status"]));
    assert_eq!(
        cli_module::stdout_text(&status, true).expect("the status line"),
        served[0]
    );
    let line: Value = serde_json::from_str(served[0]).unwrap();
    assert_eq!(
        line["outcome"]["code"], "runtime_overview_reported",
        "{line}"
    );
    assert!(line.get("watchdog").is_some(), "{line}");
    // Fix 2, `actinglab status` with no Runtime running (the case of actinglab's old
    // `status_without_runtime_is_exit_five`): the row's Runtime is optional, so the line succeeds
    // with exit 0 and its `daemon` block names the missing Runtime: offline, the client's class
    // `runtime` and `runtime_unavailable`, caused by `runtime_info_unavailable`. The client found
    // no runtime-info.json under the state root and opened no connection.
    assert_eq!(status.code, 0, "{line}");
    assert_eq!(line["outcome"]["category"], "success", "{line}");
    let daemon = &line["daemon"];
    assert_eq!(daemon["online"], false, "{line}");
    assert_eq!(daemon["error"]["class"], "runtime", "{line}");
    assert_eq!(
        daemon["error"]["outcome"]["code"], "runtime_unavailable",
        "{line}"
    );
    assert!(
        daemon["error"]["outcome"]
            .to_string()
            .contains("\"runtime_info_unavailable\""),
        "{line}"
    );
    // Fix 2, `--runtime-endpoint` with an untrusted remote endpoint (the case of actinglab's old
    // `status_blocks_untrusted_remote_runtime_endpoint`). After the words the line is the core
    // row's, which takes no endpoint: the option is refused by name while the words are read,
    // before `core_ops::answer`, the one place the row connects (`actingctl_usage_invalid`, exit
    // 1, no overview read). Before the words the line is no core row's: actinglab's own dispatch
    // refuses it as a usage error before anything runs, exit 1. The row reaches a Runtime only
    // through the state root's runtime-info.json, whose host must be loopback.
    let untrusted = "http://example.invalid:4317";
    for line in [
        &["status", "--runtime-endpoint", untrusted][..],
        &["--json", "status", "--runtime-endpoint", untrusted][..],
    ] {
        let line = words_of(line);
        match entry(line.clone(), false) {
            Err(module) => assert_eq!(module, line),
            Ok(own) => panic!("{line:?} is the CLI module's core row, not {own:?}"),
        }
        let (_, words) = cli_module::front_end(line.clone());
        assert!(cli_module::words::parse(table, &words).is_err(), "{line:?}");
        let refused = cli_module::run_line(&words);
        assert_eq!(refused.code, 1, "{line:?}");
        let refusal = one_line(&cli_module::stdout_text(&refused, true).expect("the refusal"));
        assert_eq!(
            refusal["outcome"]["code"], "actingctl_usage_invalid",
            "{refusal}"
        );
        assert_eq!(
            refusal["outcome"]["values"],
            json!({"argument": "--runtime-endpoint", "argument_problem": "unknown"}),
            "{refusal}"
        );
        assert!(refusal.get("daemon").is_none(), "{refusal}");
    }
    let (exit, text) = printed_own(&words_of(&[
        "--json",
        "--runtime-endpoint",
        untrusted,
        "status",
    ]));
    assert_eq!(exit.code, 1, "{text}");
    let refusal = one_line(&text);
    assert_eq!(refusal["outcome"]["category"], "error", "{refusal}");
    assert_eq!(refusal["outcome"]["code"], "validation_failed", "{refusal}");
    assert!(refusal.get("daemon").is_none(), "{refusal}");
    let shown = cli_module::run_line(&words_of(&["run", "show", REQUEST]));
    assert_eq!(
        cli_module::stdout_text(&shown, true).expect("the run show line"),
        served[1]
    );
    let line: Value = serde_json::from_str(served[1]).unwrap();
    assert!(line["outcome"]["code"].is_string(), "{line}");
    unsafe {
        env::remove_var(RUNTIME_STATE_ROOT_ENV);
    }
    set_missing_config_env();
}

#[test]
fn own_results_never_name_outcome() {
    let _guard = env_lock();
    let snapshot: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ratchet/actinglab_commands.json"),
        )
        .expect("the command snapshot"),
    )
    .expect("the command snapshot parses");
    let commands = snapshot["commands"]
        .as_array()
        .expect("its commands")
        .iter()
        .map(|command| command.as_str().expect("a command").to_owned())
        .collect::<Vec<_>>();
    assert!(!commands.is_empty());
    // The two declared commands are commands of actinglab's dispatch.
    assert_eq!(
        own_line::WRAPPED,
        [("resource verify", "report"), ("run summary", "summary")]
    );
    for (command, _) in own_line::WRAPPED {
        assert!(commands.iter().any(|own| own == command), "{command}");
    }

    // Every own command: a result is flattened beside the line's own outcome only, and a result
    // that names `outcome` is never flattened: its declared member carries it whole, and without
    // one the line is refused (nothing on stdout, exit 1).
    let ordinary = json!({"status": "neutral"});
    let colliding = json!({"status": "neutral", "outcome": {"kind": "neutral"}});
    for command in &commands {
        let exit = own_line::success(command, ordinary.clone());
        let line = exit.line.expect("an ordinary result's line");
        match own_line::wrapping_member(command) {
            Some(member) => assert_eq!(line[member], ordinary, "{command}"),
            None => assert_eq!(names(&line), ["status", "outcome"], "{command}"),
        }
        assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
        let exit = own_line::success(command, colliding.clone());
        match own_line::wrapping_member(command) {
            Some(member) => {
                let line = exit.line.expect("a declared result's line");
                assert_eq!(names(&line), [member, "outcome"], "{command}");
                assert_eq!(line[member], colliding, "{command}");
                assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
            }
            None => {
                assert!(
                    exit.line.is_err(),
                    "{command}: a second outcome was flattened"
                );
                assert!(cli_module::stdout_text(&exit, true).is_err(), "{command}");
                assert_eq!(exit.code, 1, "{command}");
            }
        }
    }

    // `resource verify` and `run summary` carry their documents unchanged under `report` and
    // `summary`, whose own `outcome` stays inside.
    let report = json!({
        "schema_version": "actinglab.resource-verify.v1",
        "result": "passed",
        "outcome": {
            "schema_version": OUTCOME_SCHEMA_VERSION,
            "code": "resource_verify_passed",
            "category": "success",
            "values": {"step_count": 7}
        },
        "steps": [],
        "failed_steps": []
    });
    let summary = json!({
        "schema_version": "actingcommand.run-summary.v1",
        "status": "succeeded",
        "outcome": {"kind": "fixture_simulation", "policy": "completed", "result": "no_op"},
        "effect": "no_op"
    });
    for (command, member, document) in [
        ("resource verify", "report", report),
        ("run summary", "summary", summary),
    ] {
        let result = CliResult::ok(command.to_owned(), document.clone(), true, String::new());
        let exit = own_line::exit(&result);
        assert_eq!(exit.code, 0, "{command}");
        let text = cli_module::stdout_text(&exit, true).expect("the line");
        let line = one_line(&text);
        assert_eq!(names(&line), [member, "outcome"], "{command}");
        assert_eq!(line[member], document, "{command}");
        assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
    }

    // The own commands that run offline with nothing to read answer no `outcome` of their own.
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    for command in [
        "help",
        "version",
        "paths",
        "schema",
        "list",
        "capabilities",
        "doctor",
    ] {
        let result = run_cli(["--json", command], false);
        let data = result
            .envelope
            .data
            .clone()
            .unwrap_or_else(|| panic!("{command}: {}", result.envelope_json()));
        assert!(data.get("outcome").is_none(), "{command}");
        let line = own_line::exit(&result).line.expect("the line");
        assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
    }
    set_missing_config_env();
}

#[test]
fn do_check_failure_exits_one() {
    let answer = LabOperationAnswer {
        request_id: serde_json::from_value(json!(REQUEST)).expect("a canonical request id"),
        correlation_id: serde_json::from_value::<CorrelationId>(json!(CORRELATION))
            .expect("a canonical correlation id"),
        receipt_state: RuntimeReceiptState::Completed,
        effect: Some(EffectDisposition::Performed),
        failure: None,
    };
    let client_code = RuntimeContractCode::RuntimeLabDiagnosticBudgetMismatch.as_str();
    let error = crate::lab2_cli::lab_operation_call_error(lab_operation_check_failure(
        RuntimeContractCode::RuntimeLabDiagnosticBudgetMismatch,
        answer,
    ));
    assert_eq!(error.code, "lab_operation_check_failed");
    let details = error.details.clone().expect("the details");

    // The command line: the CLI module's line, exit 1, the catalog's values of the code beside
    // the message and the details whole.
    let result = CliResult::err("do".to_owned(), error.clone(), true);
    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 1);
    let line = one_line(&cli_module::stdout_text(&exit, true).expect("the line"));
    assert_eq!(names(&line), ["message", "details", "outcome"]);
    assert_eq!(line["outcome"]["code"], "lab_operation_check_failed");
    assert_eq!(line["outcome"]["category"], "error");
    assert_eq!(
        line["outcome"]["values"],
        json!({"req_id": REQUEST, "receipt_state": "completed", "client_code": client_code})
    );
    assert_eq!(line["message"], json!(error.message));
    assert_eq!(line["details"], details);
    assert_eq!(line["details"]["effect"], "performed");
    assert_eq!(line["details"]["executed"], true);

    // MCP's actinglab child (`--json --via mcp … do …`): the envelope, exit 5.
    let (text, code) = enveloped(&CliResult::err("do".to_owned(), error, true));
    assert_eq!(code, 5);
    let envelope: Value = serde_json::from_str(&text).expect("the envelope");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "lab_operation_check_failed");
    assert_eq!(envelope["error"]["details"], details);
}
