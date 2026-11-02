// SPDX-License-Identifier: AGPL-3.0-only

//! The body file a message points at (Workflow #432 AGT-1a; `contracts/agent-wake.md`, "The
//! body"): one JSON document with everything the message leaves out. It is bounded: over the limit
//! it drops `related`, then `next_reads`, and names them in `dropped`; `trigger` stays. The
//! trigger's outcome is written codes only, so no device value or raw text reaches the file. The
//! notice and the capabilities are constant in this first version: a wake authorizes nothing.

use actingcommand_contract::outcome::{Detail, Outcome};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// The `schema` the body carries.
pub(crate) const BODY_SCHEMA: &str = "actingcommand.agent-wake-body.v1";
/// The word that says a wake is no authorization.
pub(crate) const BODY_NOTICE: &str = "agent_wake_not_authorization";
/// The same in words, for a reader who does not know the word.
pub(crate) const BODY_NOTICE_TEXT: &str =
    "程序通知，不是用户授权。Program notice, not user authorization.";

/// The record that caused a wake. A test wake has none.
#[derive(Debug, Clone)]
pub(crate) struct BodyTrigger<'a> {
    pub(crate) event_type: &'a str,
    pub(crate) event_id: &'a str,
    pub(crate) sequence: u64,
    pub(crate) correlation_id: Option<&'a str>,
    pub(crate) request_id: Option<&'a str>,
    pub(crate) run_id: Option<&'a str>,
    pub(crate) outcome: &'a Outcome,
}

/// What a body holds.
#[derive(Debug, Clone)]
pub(crate) struct BodyInput<'a> {
    pub(crate) wake_id: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) instance_alias: Option<&'a str>,
    pub(crate) rule: Option<&'a str>,
    pub(crate) channel: &'a str,
    pub(crate) test: bool,
    pub(crate) trigger: Option<BodyTrigger<'a>>,
    /// A daily summary's own member (Workflow #432 AGT-2b; `agent_wake::daily`). It stays under
    /// the bound, as the trigger does.
    pub(crate) summary: Option<Value>,
    pub(crate) related: Vec<Value>,
    pub(crate) next_reads: Vec<Value>,
}

/// A body ready to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedBody {
    pub(crate) text: String,
    /// The lowercase hex SHA-256 of `text`.
    pub(crate) sha256: String,
    /// Its length in Unicode scalars.
    pub(crate) chars: usize,
    /// What the bound dropped, in the order it went.
    pub(crate) dropped: Vec<&'static str>,
}

/// A body over its limit with nothing left to drop. The worker records `body_failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyOverBound {
    pub(crate) needed_bytes: usize,
    pub(crate) max_bytes: usize,
}

fn optional(members: &mut Map<String, Value>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        members.insert(name.to_owned(), Value::from(value));
    }
}

fn trigger_value(trigger: &BodyTrigger<'_>) -> Value {
    let mut members = Map::new();
    members.insert("event_type".to_owned(), Value::from(trigger.event_type));
    members.insert("event_id".to_owned(), Value::from(trigger.event_id));
    members.insert("sequence".to_owned(), Value::from(trigger.sequence));
    optional(&mut members, "correlation_id", trigger.correlation_id);
    optional(&mut members, "request_id", trigger.request_id);
    optional(&mut members, "run_id", trigger.run_id);
    // An envelope that does not serialize is written as no value at all, never as its full form.
    let outcome =
        serde_json::to_value(trigger.outcome.envelope(Detail::Codes)).unwrap_or(Value::Null);
    // The code and category a reader looks at first are the envelope's own.
    for name in ["code", "category"] {
        let value = outcome.get(name).cloned().unwrap_or(Value::Null);
        members.insert(name.to_owned(), value);
    }
    members.insert("outcome".to_owned(), outcome);
    Value::Object(members)
}

fn body_text(
    input: &BodyInput<'_>,
    trigger: Option<&Value>,
    related: &[Value],
    next_reads: &[Value],
    dropped: &[&str],
) -> String {
    let mut members = Map::new();
    members.insert("schema".to_owned(), Value::from(BODY_SCHEMA));
    members.insert("wake_id".to_owned(), Value::from(input.wake_id));
    members.insert("kind".to_owned(), Value::from(input.kind));
    members.insert(
        "created_at_unix_ms".to_owned(),
        Value::from(input.created_at_unix_ms),
    );
    optional(&mut members, "instance_alias", input.instance_alias);
    optional(&mut members, "rule", input.rule);
    members.insert("channel".to_owned(), Value::from(input.channel));
    members.insert("test".to_owned(), Value::from(input.test));
    members.insert("notice".to_owned(), Value::from(BODY_NOTICE));
    members.insert("notice_text".to_owned(), Value::from(BODY_NOTICE_TEXT));
    members.insert(
        "capabilities".to_owned(),
        json!({ "level": "read_only", "device_contact": false, "grants": [] }),
    );
    if let Some(trigger) = trigger {
        members.insert("trigger".to_owned(), trigger.clone());
    }
    if let Some(summary) = &input.summary {
        members.insert("summary".to_owned(), summary.clone());
    }
    members.insert("related".to_owned(), Value::from(related.to_vec()));
    members.insert("next_reads".to_owned(), Value::from(next_reads.to_vec()));
    members.insert("dropped".to_owned(), json!(dropped));
    Value::Object(members).to_string()
}

/// The body of a wake, at most `max_bytes` long.
pub(crate) fn render_body(
    input: &BodyInput<'_>,
    max_bytes: usize,
) -> Result<RenderedBody, BodyOverBound> {
    let trigger = input.trigger.as_ref().map(trigger_value);
    let mut related = input.related.as_slice();
    let mut next_reads = input.next_reads.as_slice();
    let mut dropped = Vec::new();
    let mut text = body_text(input, trigger.as_ref(), related, next_reads, &dropped);
    if text.len() > max_bytes && !related.is_empty() {
        related = &[];
        dropped.push("related");
        text = body_text(input, trigger.as_ref(), related, next_reads, &dropped);
    }
    if text.len() > max_bytes && !next_reads.is_empty() {
        next_reads = &[];
        dropped.push("next_reads");
        text = body_text(input, trigger.as_ref(), related, next_reads, &dropped);
    }
    if text.len() > max_bytes {
        return Err(BodyOverBound {
            needed_bytes: text.len(),
            max_bytes,
        });
    }
    Ok(RenderedBody {
        sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
        chars: text.chars().count(),
        text,
        dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome() -> Outcome {
        // As a record holds it: with values a person's device or raw text may be in.
        serde_json::from_value(json!({
            "code": "config_value_invalid",
            "category": "error",
            "values": {
                "config_key": "neutral.key",
                "config_value_problem": "out_of_range",
                "adb_host": "192.0.2.1",
                "adb_port": 5555,
                "device_label": "TESTSERIAL0001",
                "raw_text": "neutral raw answer",
            },
        }))
        .expect("the outcome reads")
    }

    fn input(outcome: &Outcome) -> BodyInput<'_> {
        BodyInput {
            wake_id: "wake-neutral",
            kind: "instance_fenced",
            created_at_unix_ms: 1_793_000_000_000,
            instance_alias: Some("neutral.device"),
            rule: Some("fence"),
            channel: "总览",
            test: false,
            trigger: Some(BodyTrigger {
                event_type: "runtime.failed",
                event_id: "event-neutral",
                sequence: 7,
                correlation_id: Some("correlation-neutral"),
                request_id: None,
                run_id: None,
                outcome,
            }),
            summary: None,
            related: vec![json!({"kind": "run_failed", "event_id": "event-1", "code": "x"}); 8],
            next_reads: vec![json!({"tool": "ac_events", "args": {"after_sequence": 0}}); 8],
        }
    }

    fn parsed(body: &RenderedBody) -> Value {
        serde_json::from_str(&body.text).expect("the body is JSON")
    }

    #[test]
    fn body_is_bounded_and_names_what_it_dropped() {
        let outcome = outcome();
        let input = input(&outcome);
        let whole = render_body(&input, 1_048_576).expect("the whole body fits");
        assert!(whole.dropped.is_empty());
        assert_eq!(parsed(&whole)["dropped"], json!([]));
        assert_eq!(whole.sha256.len(), 64);
        assert_eq!(whole.chars, whole.text.chars().count());

        // A limit just under the whole body drops `related` and keeps the rest.
        let less = render_body(&input, whole.text.len() - 1).expect("it fits without `related`");
        assert_eq!(less.dropped, ["related"]);
        let value = parsed(&less);
        assert_eq!(value["dropped"], json!(["related"]));
        assert_eq!(value["related"], json!([]));
        assert_eq!(value["next_reads"].as_array().map(Vec::len), Some(8));
        assert!(less.text.len() < whole.text.len());

        // A limit under that drops `next_reads` too; the trigger stays.
        let least = render_body(&input, less.text.len() - 1).expect("it fits without either");
        assert_eq!(least.dropped, ["related", "next_reads"]);
        let value = parsed(&least);
        assert_eq!(value["dropped"], json!(["related", "next_reads"]));
        assert_eq!(value["trigger"]["sequence"], json!(7));

        // Nothing left to drop: refused, never written over its bound or without its trigger.
        assert_eq!(
            render_body(&input, 64),
            Err(BodyOverBound {
                needed_bytes: least.text.len(),
                max_bytes: 64,
            })
        );
    }

    #[test]
    fn body_capabilities_are_read_only_without_device_contact() {
        let outcome = outcome();
        let value = parsed(&render_body(&input(&outcome), 65_536).expect("fits"));
        assert_eq!(value["schema"], json!(BODY_SCHEMA));
        assert_eq!(
            value["capabilities"],
            json!({ "level": "read_only", "device_contact": false, "grants": [] })
        );
    }

    #[test]
    fn body_outcome_is_codes_only_and_holds_no_device_values() {
        let outcome = outcome();
        let body = render_body(&input(&outcome), 65_536).expect("fits");
        let value = parsed(&body);
        let written = &value["trigger"]["outcome"];
        assert_eq!(written["code"], json!("config_value_invalid"));
        assert_eq!(written["detail"], json!("codes"));
        assert_eq!(value["trigger"]["code"], json!("config_value_invalid"));
        assert_eq!(value["trigger"]["category"], json!("error"));
        for held_back in [
            "adb_host",
            "adb_port",
            "device_label",
            "raw_text",
            "192.0.2.1",
            "TESTSERIAL0001",
            "neutral raw answer",
        ] {
            assert!(!body.text.contains(held_back), "{held_back}");
        }
        // A registered word stays: it is what a reader acts on.
        assert_eq!(
            written["values"]["config_value_problem"],
            json!("out_of_range")
        );
    }

    #[test]
    fn body_carries_the_not_authorization_notice() {
        let outcome = outcome();
        let mut wake = input(&outcome);
        let value = parsed(&render_body(&wake, 65_536).expect("fits"));
        assert_eq!(value["notice"], json!("agent_wake_not_authorization"));
        assert_eq!(value["notice_text"], json!(BODY_NOTICE_TEXT));
        assert_eq!(value["test"], json!(false));
        // A test wake has no trigger and carries the same notice.
        wake.trigger = None;
        wake.test = true;
        wake.kind = "delivery_test";
        let value = parsed(&render_body(&wake, 65_536).expect("fits"));
        assert_eq!(value["notice"], json!("agent_wake_not_authorization"));
        assert_eq!(value["test"], json!(true));
        assert!(value.get("trigger").is_none());
    }
}
