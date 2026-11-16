// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 PC-1 (register §1.438; contracts/pack-catalog.md, "Who writes it"; coordinator work
// order `rd-1116/brief-477-pc1-pack-catalog.md`, item 6): `actinglab standard-pack catalog`, the
// writer of a game root's `pack-catalog.json`, on both fronts of the Lab.
// - `pc1_the_writer_answers_on_the_command_line_and_through_serve`: under the global `--dry-run` a
//   line without `--plan` is `dry_run_unsupported` and makes nothing; a flag the row does not take
//   is refused; with `--plan` the command line answers the document it would write (`planned`,
//   one entry per package of `bundle.json`) and writes nothing, and `serve --stdio` (the front the
//   Lab's MCP server's child runs) answers the same plan for the row's parameters; without
//   `--plan` the file is written, and a second line finds it holding its bytes (`unchanged`).
// A working root of made-up files in a temporary directory: no Runtime, no device, no clock read,
// no sleep.

use super::env_lock;
use super::gate_434_lab5b4::Isolated;
use crate::cli_module::{self, serve};
use crate::{CliResult, run_cli};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const GAME: &str = "neutral";
const FIRST: &str = "neutral.catalog.first";
const SECOND: &str = "neutral.catalog.second";
const POOL: &str = "pool.neutral.a";

/// A game root of a working copy below `base`: `repo/neutral/` with `bundle.json` (two packages)
/// and `schedule.json` (one rule of the first producing one unit of [`POOL`]).
fn working_root(base: &Path) -> PathBuf {
    let root = base.join("repo").join(GAME);
    fs::create_dir_all(&root).unwrap();
    let packs = [FIRST, SECOND]
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": "test", "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    let files = [
        (
            "bundle.json",
            json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": packs}),
        ),
        (
            "schedule.json",
            json!({
                "schema_version": "actingcommand.schedule.v1",
                "rules": [{"id": "rule.first", "package_id": FIRST,
                           "produces": [{"pool_id": POOL, "amount": 1}]}],
                "pools": [{"id": POOL}]
            }),
        ),
    ];
    for (name, value) in files {
        fs::write(root.join(name), serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
    root
}

/// `actinglab --json`, then `words`.
fn catalog(words: &[&str]) -> CliResult {
    run_cli(
        std::iter::once("--json")
            .chain(words.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        true,
    )
}

/// The first value of the member `name` anywhere in `value`.
fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Object(members) => members
            .get(name)
            .or_else(|| members.values().find_map(|inner| member(inner, name))),
        Value::Array(items) => items.iter().find_map(|inner| member(inner, name)),
        _ => None,
    }
}

#[test]
fn pc1_the_writer_answers_on_the_command_line_and_through_serve() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let root = working_root(isolated.temp.path());
    let root_text = root.display().to_string();
    let file = root.join("pack-catalog.json");

    let dry_run = catalog(&[
        "--dry-run",
        "standard-pack",
        "catalog",
        "--root",
        &root_text,
    ]);
    assert_eq!(dry_run.exit_code(), 2, "{}", dry_run.envelope_json());
    assert_eq!(
        dry_run
            .envelope
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("dry_run_unsupported")
    );
    let unknown = catalog(&[
        "standard-pack",
        "catalog",
        "--root",
        &root_text,
        "--frobnicate",
        "x",
    ]);
    assert_eq!(unknown.exit_code(), 2, "{}", unknown.envelope_json());
    assert!(!file.exists(), "a refused line wrote the file");

    let planned = catalog(&["standard-pack", "catalog", "--root", &root_text, "--plan"]);
    assert_eq!(planned.exit_code(), 0, "{}", planned.envelope_json());
    let data = planned.envelope.data.clone().expect("the plan");
    assert_eq!(data["status"], "planned");
    assert_eq!(data["added"], json!([FIRST, SECOND]));
    assert_eq!(
        data["document"]["packs"][0],
        json!({"package_id": FIRST, "consumes": [],
               "produces": [{"pool_id": POOL, "quantity": {"kind": "constant", "amount": 1}}]})
    );
    assert!(!file.exists(), "a plan wrote the file");

    let input = json!({"op": "standard-pack catalog", "params": {"root": root_text, "plan": true}})
        .to_string();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    serve::serve(
        cli_module::table().expect("the operation table"),
        input.as_bytes(),
        &mut output,
        &mut errors,
    )
    .expect("the loop ends");
    let output = String::from_utf8(output).unwrap();
    let line: Value = serde_json::from_str(output.lines().next().expect("one line")).unwrap();
    assert_eq!(member(&line, "status"), Some(&json!("planned")), "{line}");
    assert_eq!(
        member(&line, "added"),
        Some(&json!([FIRST, SECOND])),
        "{line}"
    );
    assert!(!file.exists(), "a plan through serve wrote the file");

    let written = catalog(&["standard-pack", "catalog", "--root", &root_text]);
    assert_eq!(written.exit_code(), 0, "{}", written.envelope_json());
    assert_eq!(
        written.envelope.data.clone().expect("the answer")["status"],
        "written"
    );
    assert!(file.exists());
    let again = catalog(&["standard-pack", "catalog", "--root", &root_text]);
    assert_eq!(again.exit_code(), 0, "{}", again.envelope_json());
    assert_eq!(
        again.envelope.data.clone().expect("the answer")["status"],
        "unchanged"
    );
}
