// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-2 fix 2 (register §1.287, "Slice LAB-2"; `contracts/lab-two-entries.md`
//! §9.3; the coordinator's ruling of 2026-11-10): the command lines actinglab's current consumers
//! send print what main `c76ce040` printed for them. Each case of `golden/consumers.json` is one
//! consumer's command line in its own shape (`--json` or none with stdout redirected, `--via mcp`,
//! a leading `--run-root`, `--verbose` before the words, `--package` naming a content directory),
//! run through the production `actinglab` binary against a sealed temporary fixture; its exit code
//! and its envelope are compared with the golden. The goldens were written from main's code (its
//! `main` printing `CliResult`'s envelope, and each command's own answer or refusal), not
//! recorded by running a binary. `<ROOT>` stands for the fixture's folder, and `\` is read as `/`.
//! A member whose golden value is `"<ANY>"` must be there, with any value: the text an admission
//! or the operating system writes (a refusal's message and details), which these lines do not
//! pin. The protocol goldens (`golden_protocol.rs`) pin the other shapes (global options before
//! the words, `--zip`).

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const GOLDEN_PATH: &str = "tests/golden/consumers.json";
/// A golden value that only requires the member to be there.
const ANY: &str = "<ANY>";
const ROOT: &str = "<ROOT>";

#[derive(Debug, Deserialize)]
struct ConsumerCase {
    name: String,
    consumer: String,
    args: Vec<String>,
    exit_code: i32,
    envelope: Value,
}

#[test]
fn consumer_command_lines_print_what_main_printed() {
    let cases: Vec<ConsumerCase> = serde_json::from_slice(
        &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_PATH))
            .expect("read the consumer goldens"),
    )
    .expect("the consumer goldens parse");
    assert!(!cases.is_empty(), "no consumer golden");
    for case in &cases {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        fixture(root);
        let args = case
            .args
            .iter()
            .map(|arg| arg.replace(ROOT, &root.to_string_lossy()))
            .collect::<Vec<_>>();
        let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
            .args(&args)
            .current_dir(root)
            .env("ACTINGLAB_CONFIG_PATH", root.join("missing-config.json"))
            .env("LOCALAPPDATA", root.join("appdata"))
            .env("APPDATA", root.join("appdata"))
            .env("ACTINGLAB_SESSION_STATE_DIR", root.join("session"))
            .env_remove("ACTINGCOMMAND_RUNTIME_STATE_ROOT")
            .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
            .env_remove("ACTINGCOMMAND_INITIATOR")
            .env_remove("ACTINGCOMMAND_DETAIL")
            .output()
            .expect("run actinglab");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut envelope: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
            panic!(
                "{} ({}): stdout is no envelope ({error}): {stdout}; stderr: {stderr}",
                case.name, case.consumer
            )
        });
        normalize(&mut envelope, root);
        assert_eq!(
            output.status.code(),
            Some(case.exit_code),
            "{} ({}): {envelope}; stderr: {stderr}",
            case.name,
            case.consumer
        );
        if let Some(difference) = differs(&case.envelope, &envelope, "") {
            panic!(
                "{} ({}): {difference}\nexpected {}\nprinted  {envelope}",
                case.name, case.consumer, case.envelope
            );
        }
    }
}

/// The fixture: a resource repository whose changed-path list names a workflow file, a file that
/// is no game root, a content directory that is a file, a maintenance list that ends early and
/// the run root; `packs/missing` is not there.
fn fixture(root: &Path) {
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).expect("the repository's Git directory");
    fs::create_dir_all(repo.join(".github").join("workflows")).expect("the workflows directory");
    fs::write(
        repo.join(".github").join("workflows").join("ci.yml"),
        b"name: neutral\n",
    )
    .expect("the workflow file");
    // Git's `--name-status -z` list: 27 bytes.
    fs::write(
        repo.join(".git").join("resource-changed-paths"),
        b"M\0.github/workflows/ci.yml\0",
    )
    .expect("the changed-path list");
    fs::write(root.join("not-a-root"), b"no game root\n").expect("the file that is no root");
    fs::create_dir_all(root.join("packs")).expect("the packs directory");
    fs::write(root.join("packs").join("neutral.json"), b"{}\n").expect("the package file");
    fs::write(root.join("maintenance.json"), b"[").expect("the maintenance list");
    fs::create_dir_all(root.join("runs")).expect("the run root");
}

/// Every string of `value` with the fixture's folder (as given and canonical) as `<ROOT>`, and
/// `\` as `/`.
fn normalize(value: &mut Value, root: &Path) {
    let mut names = vec![root.to_string_lossy().into_owned()];
    if let Ok(canonical) = fs::canonicalize(root) {
        let canonical = canonical.to_string_lossy().into_owned();
        if let Some(plain) = canonical.strip_prefix(r"\\?\") {
            names.push(plain.to_owned());
        }
        names.push(canonical);
    }
    let slashed = names
        .iter()
        .map(|name| name.replace('\\', "/"))
        .collect::<Vec<_>>();
    names.extend(slashed);
    // The longest first, so that a canonical form is replaced whole.
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    normalize_strings(value, &names);
}

fn normalize_strings(value: &mut Value, names: &[String]) {
    match value {
        Value::String(text) => {
            for name in names {
                *text = text.replace(name.as_str(), ROOT);
            }
            *text = text.replace('\\', "/");
        }
        Value::Array(items) => {
            for item in items {
                normalize_strings(item, names);
            }
        }
        Value::Object(members) => {
            for member in members.values_mut() {
                normalize_strings(member, names);
            }
        }
        _ => {}
    }
}

/// Where `printed` differs from `golden` (`"<ANY>"` matches any value), at the JSON pointer `at`.
fn differs(golden: &Value, printed: &Value, at: &str) -> Option<String> {
    match (golden, printed) {
        (Value::String(any), _) if any == ANY => None,
        (Value::Object(golden), Value::Object(printed)) => {
            let golden_names = golden.keys().collect::<BTreeSet<_>>();
            let printed_names = printed.keys().collect::<BTreeSet<_>>();
            if golden_names != printed_names {
                return Some(format!(
                    "{at}: members {printed_names:?}, golden {golden_names:?}"
                ));
            }
            golden.iter().find_map(|(name, value)| {
                differs(value, &printed[name.as_str()], &format!("{at}/{name}"))
            })
        }
        (Value::Array(golden), Value::Array(printed)) => {
            if golden.len() != printed.len() {
                return Some(format!(
                    "{at}: {} items, golden {}",
                    printed.len(),
                    golden.len()
                ));
            }
            golden
                .iter()
                .zip(printed)
                .enumerate()
                .find_map(|(index, (golden, printed))| {
                    differs(golden, printed, &format!("{at}/{index}"))
                })
        }
        _ if golden == printed => None,
        _ => Some(format!("{at}: {printed}, golden {golden}")),
    }
}
