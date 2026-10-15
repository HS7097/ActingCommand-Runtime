// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #369 S6c (model-369-queue v3.1 §7 row 8, test (10); apps gate 56): the configured
// emulator starts' report lines are daemon lines written through the A6a writer, one developer
// line and one `OUTCOME actingd:` line each; a report write that fails is counted and never
// stops actingd (P3 L2): the pass goes on with the next line.

use super::*;
use actingcommand_runtime_host::HostCode;
use serde_json::Value;

const ALIAS: &str = "neutral.instance";

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

fn reports() -> Vec<Outcome> {
    vec![
        Outcome::new(HostCode::EmulatorAutostartQueued).count(1_usize),
        Outcome::new(HostCode::EmulatorAutostartSuperseded).alias(ALIAS),
        Outcome::new(HostCode::EmulatorAutostartNetworkUnready)
            .alias(ALIAS)
            .elapsed_ms(600_000_u64),
    ]
}

#[test]
fn s6c_a_failed_report_write_does_not_stop_actingd() {
    let outcomes = reports();

    let mut failing = FailingSink;
    assert_eq!(
        write_daemon_reports(&outcomes, &mut failing),
        outcomes.len(),
        "every failed write is counted, and the pass reaches the last line"
    );

    let mut written = Vec::new();
    assert_eq!(write_daemon_reports(&outcomes, &mut written), 0);
    let text = String::from_utf8(written).expect("daemon lines are text");
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), outcomes.len() * 2, "{text}");
    for (pair, outcome) in lines.chunks(2).zip(&outcomes) {
        let code = outcome.code().as_str();
        assert!(
            pair[0].contains(&format!("actingd: {code}")),
            "the developer line names {code}: {}",
            pair[0]
        );
        let envelope: Value = serde_json::from_str(
            pair[1]
                .strip_prefix("OUTCOME actingd: ")
                .expect("the OUTCOME line"),
        )
        .expect("the envelope");
        assert_eq!(envelope["code"], code, "{envelope}");
    }
}
