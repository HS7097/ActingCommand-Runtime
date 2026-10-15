// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-S5 (frozen model D2 v1.4.2 §5.2, §7.1, §7.2, §8 row T37; apps gate 65): each
// transition of the fact refresh recorder is exactly one developer line, at its category's
// level, naming its code with every value the model requires of it, its cycle and its number,
// and, written in the same write (Workflow #378 A6a), one `OUTCOME actingd:` line carrying the
// same outcome. A line that cannot be written is counted and never stops the driver.

use super::*;
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const KEY: &str = "resource.neutral_count";
const PACKAGE: &str = "neutral.recognition.task";

// A sink whose every write fails, as a stderr nobody can write to.
struct FailingSink;

impl Write for FailingSink {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
    }
}

// An outcome as the Runtime hands it over: its code, category and values.
fn outcome(code: &str, category: &str, values: Value) -> Outcome {
    serde_json::from_value(json!({"code": code, "category": category, "values": values}))
        .expect("an outcome")
}

// A transition, the level of its developer line and the values it must carry.
type Row = (
    FactRefreshTransition,
    &'static str,
    Vec<(&'static str, Value)>,
);

// One transition of each code the recorder reports, with the values D2 model §7 requires of it
// (and the package it names where §7 allows one), and those required values.
fn transitions() -> Vec<Row> {
    let identity = || vec![("instance_id", json!(ALIAS)), ("fact_key", json!(KEY))];
    let rows = [
        (
            "fact_refresh_pending",
            "info",
            "INFO",
            vec![
                ("unknown_reason", json!("fact_missing")),
                ("package_id", json!(PACKAGE)),
            ],
        ),
        (
            "fact_refresh_unresolved",
            "warning",
            "WARN",
            vec![
                ("unknown_reason", json!("fact_expired")),
                ("package_id", json!(PACKAGE)),
            ],
        ),
        (
            "fact_refresh_exhausted",
            "warning",
            "WARN",
            vec![
                ("package_id", json!(PACKAGE)),
                ("refresh_limit", json!("daily_limit")),
            ],
        ),
        (
            "fact_refresh_unbound",
            "warning",
            "WARN",
            vec![("unknown_reason", json!("fact_missing"))],
        ),
        ("fact_refresh_resolved", "info", "INFO", Vec::new()),
        (
            "fact_reader_overdue",
            "warning",
            "WARN",
            vec![("elapsed_ms", json!(660_000))],
        ),
    ];
    rows.into_iter()
        .enumerate()
        .map(|(index, (code, category, level, extra))| {
            let required = identity().into_iter().chain(extra).collect::<Vec<_>>();
            let values = required
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<serde_json::Map<_, _>>();
            let number = u64::try_from(index).expect("a small index") + 1;
            (
                FactRefreshTransition {
                    cycle_seq: 7,
                    transition_seq: number,
                    instance_id: ALIAS.to_owned(),
                    fact_key: KEY.to_owned(),
                    outcome: outcome(code, category, Value::Object(values)),
                },
                level,
                required,
            )
        })
        .collect()
}

#[test]
fn d2s5_t37_each_transition_is_one_developer_line_and_one_outcome_line() {
    let rows = transitions();
    let transitions = rows
        .iter()
        .map(|(transition, _, _)| transition.clone())
        .collect::<Vec<_>>();

    let mut written = Vec::new();
    assert_eq!(
        write_fact_refresh_transitions(&transitions, &mut written),
        0
    );
    let text = String::from_utf8(written).expect("daemon lines are text");
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), rows.len() * 2, "{text}");
    for (pair, (transition, level, required)) in lines.chunks(2).zip(&rows) {
        let code = transition.outcome.code().as_str();
        let developer = pair[0];
        assert!(
            developer.starts_with(&format!("{level} actingd: {code} ")),
            "the developer line names {code} at {level}: {developer}"
        );
        for (key, value) in required {
            let shown = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            assert!(
                developer.contains(&format!(" {key}={shown}")),
                "the developer line of {code} carries {key}: {developer}"
            );
        }
        assert!(
            developer.ends_with(&format!(
                " cycle_seq=7 transition_seq={}",
                transition.transition_seq
            )),
            "the developer line names the cycle and the transition: {developer}"
        );
        let envelope: Value = serde_json::from_str(
            pair[1]
                .strip_prefix("OUTCOME actingd: ")
                .expect("the OUTCOME line"),
        )
        .expect("the envelope");
        assert_eq!(envelope["code"], json!(code), "{envelope}");
        for (key, value) in required {
            assert_eq!(&envelope["values"][*key], value, "{key} of {envelope}");
        }
    }

    // A transition that cannot be written is counted, and the pass goes on to the last one.
    let mut failing = FailingSink;
    assert_eq!(
        write_fact_refresh_transitions(&transitions, &mut failing),
        transitions.len()
    );
}
