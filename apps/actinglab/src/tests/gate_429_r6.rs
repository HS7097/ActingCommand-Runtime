// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-6, first slice (register §1.301; contracts/lab-operations.json, the row
// `task stop`; coordinator work order rd-1111/brief-r6.md): the Lab's core row `task stop` reaches
// `StopInstanceRun`, as the neighbouring core rows reach their requests (`gate_434_lab2`).
// - `r6_task_stop_words_and_serve_line_reach_the_same_stop`: the row is offered with its one
//   sentence and names `StopInstanceRun`; its command line is the CLI module's; its short words
//   and a `serve --stdio` line with the same parameters give the same call, the run's request and
//   the wait the stop sends (`--wait 2s`: two seconds; without it the stop answers at once, a
//   zero wait).
// Reads only the table this build embeds: no state root, no Runtime, no device, no clock.

use crate::cli_module::{self, Route, words};
use actingcommand_contract::{RequestId, RuntimeOperationKind};
use actingcommand_lab_ops::core_ops;
use serde_json::{Value, json};
use std::time::Duration;

const RUN: &str = "request_0123456789abcdef0123456789abcdef";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn r6_task_stop_words_and_serve_line_reach_the_same_stop() {
    let table = cli_module::table().expect("the operation table");
    let row = table.operation("task stop").expect("the row task stop");
    assert!(words::offered(row), "task stop is offered");
    let help = row.help.as_deref().unwrap_or_default();
    assert!(!help.trim().is_empty(), "task stop says what it does");
    let kinds = &row.requests;
    assert!(
        kinds.contains(&RuntimeOperationKind::StopInstanceRun),
        "{kinds:?}"
    );

    let line = ["task", "stop", "node.a", RUN, "--wait", "2s"];
    match cli_module::route(words_of(&line)) {
        Route::Module(module) => assert_eq!(module, words_of(&line)),
        other => panic!("{line:?} is the CLI module's: {other:?}"),
    }
    let cli = words::parse(table, &words_of(&line)).expect("the words");
    let params = json!({"instance": "node.a", "request": RUN, "wait": "2s"});
    let request = json!({"op": "task stop", "params": params}).to_string();
    let served = words::from_json(table, &request).expect("the serve line");
    assert_eq!(cli.name, "task stop");
    assert_eq!(served, cli);
    assert_eq!(Value::Object(cli.params.clone()), params);

    let run: RequestId = serde_json::from_value(json!(RUN)).expect("a request id");
    let (request_id, wait) = core_ops::stop_target(&cli.params).expect("the stop's run and wait");
    assert_eq!((request_id, wait), (run, Duration::from_secs(2)));
    let at_once = words::parse(table, &words_of(&["task", "stop", "node.a", RUN]))
        .expect("the words without a wait");
    let (request_id, wait) = core_ops::stop_target(&at_once.params).expect("the stop's run");
    assert_eq!((request_id, wait), (run, Duration::ZERO));
}
