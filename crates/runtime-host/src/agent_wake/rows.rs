// SPDX-License-Identifier: AGPL-3.0-only

//! The rows of the runtime fact `host.agent_wake` (Workflow #432 AGT-1b;
//! `contracts/agent-wake.md`, "Delivery states"): one row per wake, the newest
//! `delivery.history_rows`. A row holds names, a digest and counts, never a body's or a message's
//! text and never a channel's address. [`kept_rows`] reads a record back strictly: a row of another
//! shape makes the whole record unreadable, which the start says and goes on from.

use crate::codes::HostCode;
use actingcommand_agent_forward::DeferReason;
use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::{
    AgentDeliveryState, FactScalar, FactValue, OriginModule, RuntimeFactRecord,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// The runtime fact (scope Runtime, a record list, tagged so a start loads it).
pub(crate) const WAKE_FACT_KEY: &str = "host.agent_wake";

const WAKE_ID: &str = "wake_id";
const KIND: &str = "kind";
const RULE: &str = "rule";
const TEST: &str = "test";
const INSTANCE_ALIAS: &str = "instance_alias";
const CODE: &str = "code";
const CHANNEL: &str = "channel";
const CREATED_AT: &str = "created_at_unix_ms";
const UPDATED_AT: &str = "updated_at_unix_ms";
const STATE: &str = "state";
const REASON: &str = "reason";
const PROOF: &str = "proof";
const PROBLEM: &str = "problem";
const BODY_FILE: &str = "body_file";
const BODY_SHA256: &str = "body_sha256";
const BODY_CHARS: &str = "body_chars";
const MESSAGE_CHARS: &str = "message_chars";
const ALIAS_CUT: &str = "alias_cut";
const TRIGGER_EVENT_ID: &str = "trigger_event_id";
const CORRELATION_ID: &str = "correlation_id";

/// Why a row waits: a transport's reason, or the feature off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitReason {
    Forward(DeferReason),
    Disabled,
}

impl WaitReason {
    const DISABLED: &'static str = "disabled";
    const FORWARD: [DeferReason; 7] = [
        DeferReason::TargetOffline,
        DeferReason::TargetNotLoaded,
        DeferReason::TargetAwaitingInput,
        DeferReason::IpcUnavailable,
        DeferReason::SidecarOffline,
        DeferReason::RegistryUnreadable,
        DeferReason::TargetUnresponsive,
    ];

    /// The word the row records.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Forward(reason) => reason.as_str(),
            Self::Disabled => Self::DISABLED,
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        if token == Self::DISABLED {
            return Some(Self::Disabled);
        }
        Self::FORWARD
            .into_iter()
            .find(|reason| reason.as_str() == token)
            .map(Self::Forward)
    }
}

/// One wake's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WakeRow {
    pub(crate) wake_id: String,
    pub(crate) kind: String,
    pub(crate) rule: Option<String>,
    pub(crate) test: bool,
    pub(crate) instance_alias: Option<String>,
    pub(crate) code: Option<CodeStr>,
    pub(crate) channel: String,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) updated_at_unix_ms: u64,
    pub(crate) state: AgentDeliveryState,
    pub(crate) reason: Option<WaitReason>,
    pub(crate) proof: Option<String>,
    pub(crate) problem: Option<String>,
    pub(crate) body_file: Option<String>,
    pub(crate) body_sha256: Option<String>,
    pub(crate) body_chars: Option<u64>,
    pub(crate) message_chars: Option<u64>,
    pub(crate) alias_cut: bool,
    pub(crate) trigger_event_id: Option<String>,
    pub(crate) correlation_id: Option<String>,
}

fn put_text(fields: &mut BTreeMap<String, FactScalar>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        fields.insert(key.to_owned(), FactScalar::String(value.to_owned()));
    }
}

fn put_count(fields: &mut BTreeMap<String, FactScalar>, key: &str, value: Option<u64>) {
    if let Some(value) = value.and_then(|value| i64::try_from(value).ok()) {
        fields.insert(key.to_owned(), FactScalar::Integer(value));
    }
}

impl WakeRow {
    /// The row as the record list holds it.
    pub(crate) fn fields(&self) -> BTreeMap<String, FactScalar> {
        let mut fields = BTreeMap::from([
            (WAKE_ID.to_owned(), FactScalar::String(self.wake_id.clone())),
            (KIND.to_owned(), FactScalar::String(self.kind.clone())),
            (TEST.to_owned(), FactScalar::Boolean(self.test)),
            (CHANNEL.to_owned(), FactScalar::String(self.channel.clone())),
            (
                CREATED_AT.to_owned(),
                FactScalar::TimestampMs(self.created_at_unix_ms),
            ),
            (
                UPDATED_AT.to_owned(),
                FactScalar::TimestampMs(self.updated_at_unix_ms),
            ),
            (
                STATE.to_owned(),
                FactScalar::String(self.state.as_str().to_owned()),
            ),
            (ALIAS_CUT.to_owned(), FactScalar::Boolean(self.alias_cut)),
        ]);
        put_text(&mut fields, RULE, self.rule.as_deref());
        put_text(&mut fields, INSTANCE_ALIAS, self.instance_alias.as_deref());
        put_text(&mut fields, CODE, self.code.as_ref().map(CodeStr::as_str));
        put_text(&mut fields, REASON, self.reason.map(WaitReason::as_str));
        put_text(&mut fields, PROOF, self.proof.as_deref());
        put_text(&mut fields, PROBLEM, self.problem.as_deref());
        put_text(&mut fields, BODY_FILE, self.body_file.as_deref());
        put_text(&mut fields, BODY_SHA256, self.body_sha256.as_deref());
        put_count(&mut fields, BODY_CHARS, self.body_chars);
        put_count(&mut fields, MESSAGE_CHARS, self.message_chars);
        put_text(
            &mut fields,
            TRIGGER_EVENT_ID,
            self.trigger_event_id.as_deref(),
        );
        put_text(&mut fields, CORRELATION_ID, self.correlation_id.as_deref());
        fields
    }
}

fn required_text(row: &BTreeMap<String, FactScalar>, key: &str) -> Option<String> {
    match row.get(key)? {
        FactScalar::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// `Some(None)` for an absent member, `None` for one of another type.
fn optional_text(row: &BTreeMap<String, FactScalar>, key: &str) -> Option<Option<String>> {
    match row.get(key) {
        None => Some(None),
        Some(FactScalar::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    }
}

fn optional_recorded_code(row: &BTreeMap<String, FactScalar>) -> Option<Option<CodeStr>> {
    match row.get(CODE) {
        None => Some(None),
        Some(FactScalar::String(text)) => serde_json::from_value(Value::from(text.as_str()))
            .ok()
            .map(Some),
        Some(_) => None,
    }
}

fn optional_wait(row: &BTreeMap<String, FactScalar>) -> Option<Option<WaitReason>> {
    match row.get(REASON) {
        None => Some(None),
        Some(FactScalar::String(text)) => WaitReason::from_token(text).map(Some),
        Some(_) => None,
    }
}

fn optional_count(row: &BTreeMap<String, FactScalar>, key: &str) -> Option<Option<u64>> {
    match row.get(key) {
        None => Some(None),
        Some(FactScalar::Integer(count)) => u64::try_from(*count).ok().map(Some),
        Some(_) => None,
    }
}

fn required_time(row: &BTreeMap<String, FactScalar>, key: &str) -> Option<u64> {
    match row.get(key)? {
        FactScalar::TimestampMs(at) => Some(*at),
        _ => None,
    }
}

fn required_flag(row: &BTreeMap<String, FactScalar>, key: &str) -> Option<bool> {
    match row.get(key)? {
        FactScalar::Boolean(flag) => Some(*flag),
        _ => None,
    }
}

fn kept_row(row: &BTreeMap<String, FactScalar>) -> Option<WakeRow> {
    const MEMBERS: [&str; 20] = [
        WAKE_ID,
        KIND,
        RULE,
        TEST,
        INSTANCE_ALIAS,
        CODE,
        CHANNEL,
        CREATED_AT,
        UPDATED_AT,
        STATE,
        REASON,
        PROOF,
        PROBLEM,
        BODY_FILE,
        BODY_SHA256,
        BODY_CHARS,
        MESSAGE_CHARS,
        ALIAS_CUT,
        TRIGGER_EVENT_ID,
        CORRELATION_ID,
    ];
    if row.keys().any(|key| !MEMBERS.contains(&key.as_str())) {
        return None;
    }
    Some(WakeRow {
        wake_id: required_text(row, WAKE_ID)?,
        kind: required_text(row, KIND)?,
        rule: optional_text(row, RULE)?,
        test: required_flag(row, TEST)?,
        instance_alias: optional_text(row, INSTANCE_ALIAS)?,
        code: optional_recorded_code(row)?,
        channel: required_text(row, CHANNEL)?,
        created_at_unix_ms: required_time(row, CREATED_AT)?,
        updated_at_unix_ms: required_time(row, UPDATED_AT)?,
        state: AgentDeliveryState::from_token(&required_text(row, STATE)?)?,
        reason: optional_wait(row)?,
        proof: optional_text(row, PROOF)?,
        problem: optional_text(row, PROBLEM)?,
        body_file: optional_text(row, BODY_FILE)?,
        body_sha256: optional_text(row, BODY_SHA256)?,
        body_chars: optional_count(row, BODY_CHARS)?,
        message_chars: optional_count(row, MESSAGE_CHARS)?,
        alias_cut: required_flag(row, ALIAS_CUT)?,
        trigger_event_id: optional_text(row, TRIGGER_EVENT_ID)?,
        correlation_id: optional_text(row, CORRELATION_ID)?,
    })
}

/// The rows of a `host.agent_wake` record; `None` when the record is not of the shape this build
/// writes (not the Runtime's own record list, a row with a member it does not know, one without a
/// member it needs, or one of another type).
pub(crate) fn kept_rows(record: &RuntimeFactRecord) -> Option<Vec<WakeRow>> {
    if record.source != OriginModule::Runtime {
        return None;
    }
    let FactValue::RecordList(rows) = &record.value else {
        return None;
    };
    rows.iter().map(kept_row).collect()
}

/// Whether a record's code can never be a candidate for a wake: a delivery failure and a used-up
/// budget cannot make a wake. The tap (Workflow #432 AGT-2) reads it.
pub(crate) fn never_a_candidate(spelling: &str) -> bool {
    spelling.starts_with("agent_wake_") || spelling == HostCode::AgentBudgetExhausted.as_str()
}

/// The 16 bytes of a wake id (`agent_wake_<32 hex digits>`), the message's id at a transport.
pub(crate) fn wake_id_bytes(wake_id: &str) -> Option<[u8; 16]> {
    let hex = wake_id.strip_prefix("agent_wake_")?;
    if hex.len() != 32 || !hex.is_ascii() {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (index, slot) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *slot = u8::from_str_radix(hex.get(start..start + 2)?, 16).ok()?;
    }
    Some(bytes)
}
