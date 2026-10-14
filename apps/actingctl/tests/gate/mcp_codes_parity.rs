// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-14 (command table v1.1 §4.15, §2.3): `actingctl codes` and `ac_outcome_code`
// read the outcome catalog the build embeds and, for the same parameters, answer the same
// document: the CLI's stdout JSON without its `outcome` equals the MCP `result`. One code answers
// its catalog entry as the catalog holds it, with its `code`; the code list answers
// `{code, category, ladder_class, status}` rows in name order, page by page, each page's
// `next_cursor` read on by both sides until the list ends. No Runtime runs, and the CLI gets no
// `--state-root` (D5). Workflow #378 A5: the CLI line carries the outcome `outcome_codes_reported`
// beside the document.

use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::outcome::catalog_json;
use serde_json::{Map, Value, json};
use std::process::Command;
use tempfile::TempDir;

// A code actingctl owns, looked up by itself.
const CODE: &str = "answer_exported";
// The list: actingctl's codes of one prefix, more of them than one page holds.
const OWNER: &str = "actingctl";
const PREFIX: &str = "watchdog_";
const LIMIT: usize = 20;

// `actingctl codes <arguments>`: its stdout JSON without the line's `outcome`, which is
// `outcome_codes_reported`.
fn cli_codes(arguments: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .arg("codes")
        .args(arguments)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl codes");
    assert!(
        output.status.success(),
        "actingctl codes {arguments:?} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl codes {arguments:?} printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    let Some(outcome) = outcome else {
        panic!("actingctl codes {arguments:?} printed no outcome: {line}")
    };
    assert_eq!(
        outcome["code"],
        json!("outcome_codes_reported"),
        "actingctl codes {arguments:?}: {outcome}"
    );
    line
}

// The codes table of the embedded catalog.
fn catalog_codes() -> &'static Map<String, Value> {
    catalog_json()
        .unwrap_or_else(|error| panic!("the embedded outcome catalog does not parse: {error}"))
        .get("codes")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("the embedded outcome catalog has no codes table"))
}

// A JSON object of these members, in this order.
fn object<const N: usize>(members: [(&str, Value); N]) -> Value {
    let mut object = Map::new();
    for (member, value) in members {
        object.insert(member.to_owned(), value);
    }
    Value::Object(object)
}

#[test]
fn gate_actingctl_codes_and_ac_outcome_code_answer_the_same_entry_and_list_for_the_same_arguments()
{
    let codes = catalog_codes();
    let root = TempDir::new().expect("tempdir");
    let mut mcp = McpServer::spawn_at_root(root.path());
    mcp.initialize();

    // One code: its catalog entry as the catalog holds it, with its code.
    let entry = codes
        .get(CODE)
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("the embedded catalog does not list {CODE}"));
    let mut expected = Map::new();
    expected.insert("code".to_owned(), json!(CODE));
    expected.extend(entry.clone());
    let cli = cli_codes(&[CODE]);
    let answer = mcp.call_tool(2, "ac_outcome_code", object([("code", json!(CODE))]));
    assert_eq!(
        cli,
        *accepted(&answer, "ac_outcome_code"),
        "actingctl codes {CODE} and ac_outcome_code answer differently: {answer}"
    );
    assert_eq!(cli, Value::Object(expected), "actingctl codes {CODE}");

    // The list: the codes of the owner and prefix in name order, LIMIT a page.
    let mut names = codes
        .iter()
        .filter(|(name, entry)| name.starts_with(PREFIX) && entry["owner"] == json!(OWNER))
        .collect::<Vec<_>>();
    names.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    let expected_rows = names
        .into_iter()
        .map(|(name, entry)| {
            object([
                ("code", json!(name)),
                ("category", entry["category"].clone()),
                ("ladder_class", entry["ladder_class"].clone()),
                ("status", entry["status"].clone()),
            ])
        })
        .collect::<Vec<_>>();
    assert!(
        expected_rows.len() > LIMIT,
        "{OWNER} has {} codes of {PREFIX}: fewer than two pages",
        expected_rows.len()
    );
    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    for id in 3_u64.. {
        let limit = LIMIT.to_string();
        let mut flags = vec![
            "--owner",
            OWNER,
            "--prefix",
            PREFIX,
            "--limit",
            limit.as_str(),
        ];
        let mut arguments = object([
            ("owner", json!(OWNER)),
            ("prefix", json!(PREFIX)),
            ("limit", json!(LIMIT)),
        ]);
        if let Some(cursor) = &cursor {
            flags.extend(["--cursor", cursor.as_str()]);
            arguments["cursor"] = json!(cursor);
        }
        let cli = cli_codes(&flags);
        let answer = mcp.call_tool(id, "ac_outcome_code", arguments.clone());
        assert_eq!(
            cli,
            *accepted(&answer, "ac_outcome_code"),
            "actingctl codes {flags:?} and ac_outcome_code {arguments} answer differently: {answer}"
        );
        let page = cli["codes"]
            .as_array()
            .unwrap_or_else(|| panic!("actingctl codes {flags:?} answered no list: {cli}"));
        rows.extend(page.iter().cloned());
        match cli.get("next_cursor").and_then(Value::as_str) {
            // Every page but the last is full.
            Some(next) => {
                assert_eq!(page.len(), LIMIT, "a page before the last: {cli}");
                cursor = Some(next.to_owned());
            }
            None => break,
        }
        assert!(
            rows.len() < expected_rows.len(),
            "the list goes on past its {} codes: {cli}",
            expected_rows.len()
        );
    }
    assert_eq!(
        rows, expected_rows,
        "the pages of actingctl codes --owner {OWNER} --prefix {PREFIX}"
    );
}
