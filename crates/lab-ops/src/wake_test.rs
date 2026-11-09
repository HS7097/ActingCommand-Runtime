// SPDX-License-Identifier: AGPL-3.0-only

//! `wake-test` (Workflow #434 LAB-1b; register §1.287), moved from actingctl's
//! `agent-wake-test` (Workflow #432 AGT-1b): a person's test of the delivery to a registered
//! channel, `AgentWakeTest`, and with a wait the wake's row in `host.agent_wake`.

use actingcommand_contract::{AgentWakeTestQueued, FactScalar, FactValue, RuntimeFactScope};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde_json::{Map, Value};
use std::time::{Duration, Instant};

/// The program fact holding one row per wake.
const AGENT_WAKE_FACT: &str = "host.agent_wake";
/// How long a wait for the wake's row pauses between two reads.
const ROW_POLL: Duration = Duration::from_millis(200);

/// `wake-test`: a test wake of the channel `to`, naming the instance `instance_alias` in its
/// message when given; the queued wake.
pub fn queue(
    client: &RuntimeClient,
    to: &str,
    instance_alias: Option<&str>,
) -> Result<AgentWakeTestQueued, RuntimeClientError> {
    client.agent_wake_test(to, instance_alias)
}

/// The row of the wake `wake_id` in `host.agent_wake`, read until it has left `prepared` and
/// `sending` or `wait` is over; null while it has no row. A read records nothing.
pub fn row(
    client: &RuntimeClient,
    wake_id: &str,
    wait: Duration,
) -> Result<Value, RuntimeClientError> {
    let deadline = Instant::now() + wait;
    loop {
        let snapshot = client.runtime_fact_snapshot()?;
        let row = snapshot
            .records
            .iter()
            .find(|record| {
                record.scope == RuntimeFactScope::Runtime && record.key == AGENT_WAKE_FACT
            })
            .and_then(|record| {
                match &record.value {
                FactValue::RecordList(rows) => rows.iter().find(|row| {
                    matches!(row.get("wake_id"), Some(FactScalar::String(id)) if id == wake_id)
                }),
                _ => None,
            }
            });
        let settled = row.is_some_and(|row| {
            !matches!(
                row.get("state"),
                Some(FactScalar::String(state)) if state == "prepared" || state == "sending"
            )
        });
        if settled || Instant::now() >= deadline {
            let Some(row) = row else {
                return Ok(Value::Null);
            };
            let members = row
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        FactScalar::Boolean(flag) => Value::from(*flag),
                        FactScalar::Integer(number) => Value::from(*number),
                        FactScalar::String(text) => Value::from(text.as_str()),
                        FactScalar::TimestampMs(ms) | FactScalar::DurationMs(ms) => {
                            Value::from(*ms)
                        }
                    };
                    (key.clone(), value)
                })
                .collect::<Map<String, Value>>();
            return Ok(Value::Object(members));
        }
        std::thread::sleep(ROW_POLL);
    }
}
