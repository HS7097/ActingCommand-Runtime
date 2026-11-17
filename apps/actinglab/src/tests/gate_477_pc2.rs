// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 PC-2 fix round 1 (register §1.462; contracts/pack-catalog.md, "Who writes it"; the
// coordinator's fix brief, item 7): `actinglab standard-pack catalog --refresh` on both fronts of
// the Lab.
// - `pc2_refresh_is_repeated_on_the_command_line_and_a_list_through_serve`: on the command line
//   `--refresh` given twice names two packages, answered in `refreshed` in the order named; the
//   same id twice is refused `standard_pack_catalog_refresh_refused` (`package_repeated`, exit 2
//   on this line) and nothing is written; through `serve --stdio --front mcp` (the front the Lab's
//   MCP server's child runs) the parameter `refresh` is a list: two ids are answered both, exit
//   0, and the same id twice is refused with its word, exit 1, nothing written.
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

/// The lines `serve --stdio --front mcp` answers for `requests`, one per request: each the
/// envelope `{exit_code, line}`.
fn served(requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve_as(
        cli_module::table().expect("the operation table"),
        Some(serve::Front::Mcp),
        input.as_bytes(),
        &mut output,
        &mut developer,
    )
    .expect("the loop ends");
    let output = String::from_utf8(output).expect("UTF-8 lines");
    let lines = output
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("one JSON line ({error}): {line}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        requests.len(),
        "one line per request: {output}"
    );
    lines
}

#[test]
fn pc2_refresh_is_repeated_on_the_command_line_and_a_list_through_serve() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let root = working_root(isolated.temp.path());
    let root_text = root.display().to_string();
    let file = root.join("pack-catalog.json");
    let both = json!([
        {"package_id": FIRST, "state": "added",
         "after": {"consumes": [],
                   "produces": [{"pool_id": POOL, "quantity": {"kind": "constant", "amount": 1}}]}},
        {"package_id": SECOND, "state": "added", "after": {"consumes": [], "produces": []}}
    ]);

    let planned = catalog(&[
        "standard-pack",
        "catalog",
        "--root",
        &root_text,
        "--refresh",
        FIRST,
        "--refresh",
        SECOND,
        "--plan",
    ]);
    assert_eq!(planned.exit_code(), 0, "{}", planned.envelope_json());
    let data = planned.envelope.data.clone().expect("the plan");
    assert_eq!(data["status"], "planned");
    assert_eq!(
        data["refreshed"], both,
        "--refresh given twice names both, in order"
    );
    assert!(!file.exists(), "a plan wrote the file");

    let repeated = catalog(&[
        "standard-pack",
        "catalog",
        "--root",
        &root_text,
        "--refresh",
        FIRST,
        "--refresh",
        FIRST,
    ]);
    assert_eq!(repeated.exit_code(), 2, "{}", repeated.envelope_json());
    let error = repeated.envelope.error.as_ref().expect("the refusal");
    assert_eq!(error.code, "standard_pack_catalog_refresh_refused");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| member(details, "catalog_refresh_problem")),
        Some(&json!("package_repeated")),
        "{}",
        repeated.envelope_json()
    );
    assert!(!file.exists(), "a refused line wrote the file");

    let lines = served(&[
        json!({"op": "standard-pack catalog",
               "params": {"root": root_text, "plan": true, "refresh": [FIRST, SECOND]}}),
        json!({"op": "standard-pack catalog",
               "params": {"root": root_text, "refresh": [FIRST, FIRST]}}),
    ]);
    assert_eq!(lines[0]["exit_code"], json!(0), "{}", lines[0]);
    assert_eq!(
        member(&lines[0]["line"], "refreshed"),
        Some(&both),
        "a list through MCP names both, in order: {}",
        lines[0]
    );
    assert_eq!(lines[1]["exit_code"], json!(1), "{}", lines[1]);
    assert_eq!(
        lines[1]["line"]["outcome"]["code"], "standard_pack_catalog_refresh_refused",
        "{}",
        lines[1]
    );
    assert_eq!(
        member(&lines[1]["line"], "catalog_refresh_problem"),
        Some(&json!("package_repeated")),
        "{}",
        lines[1]
    );
    assert!(
        !file.exists(),
        "a refused line through serve wrote the file"
    );
}
