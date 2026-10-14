// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl facts --program | --instance <selector>` and `actingctl status --config`
//! (Workflow #389 MCP-4; MCP-0 command table v1.1 §4.4), the CLI originals of MCP `ac_facts` with
//! `scope` program, instance and config. Both sides build one `FactsRequest`, read it with
//! `read` and answer the same object, `{scope, read_at_unix_ms, snapshot | records,
//! records_total, records_kept}`: the CLI prints it as its line beside the outcome, MCP as its
//! result, exported whole past the output budget (§2.2). No read records anything in the
//! ledger: `RuntimeFactSnapshot` for program and config, `InstanceFactSnapshot` for one instance,
//! whose selector one status read resolves (§2.5).

use crate::instance_selector::{self, SelectorMiss};
use actingcommand_contract::{
    CONFIG_PARAMETERS_FACT_PREFIX, CONFIG_SELECTION_FACT_KEY, CONFIG_SUBSYSTEMS_FACT_KEY,
    RuntimeFactSnapshot,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// What one read answers: the CLI form, the MCP `scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// `facts --program`: the Runtime's own fact store.
    Program,
    /// `status --config`: only the store's `config.*` records.
    Config,
    /// `facts --instance <selector>`: one instance's facts.
    Instance,
}

impl Scope {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        [Self::Program, Self::Config, Self::Instance]
            .into_iter()
            .find(|scope| scope.as_str() == text)
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::Config => "config",
            Self::Instance => "instance",
        }
    }
}

/// One read, as either side parsed it.
pub(crate) enum FactsRequest {
    Program {
        key_prefix: Option<String>,
    },
    Config,
    /// `selector`: the instance's alias, instance_id or ADB port.
    Instance {
        selector: String,
        key_prefix: Option<String>,
    },
}

impl FactsRequest {
    const fn scope(&self) -> Scope {
        match self {
            Self::Program { .. } => Scope::Program,
            Self::Config => Scope::Config,
            Self::Instance { .. } => Scope::Instance,
        }
    }
}

/// Why a read answered nothing.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The selector names no instance of the Runtime's status, or more than one.
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    /// `status --config`: the snapshot carries no `config.subsystems` or no
    /// `config.parameters.<section>` record (`config_facts_missing`).
    ConfigFactsMissing,
    Runtime(RuntimeClientError),
    /// The answer does not encode as JSON.
    Output(serde_json::Error),
}

/// The answer to `request`: `{scope, read_at_unix_ms, snapshot | records, records_total,
/// records_kept}`. `read_at_unix_ms` is the Runtime's clock at the read (the snapshot's
/// `taken_at_unix_ms`, the instance read's sampled interval end), against which each record's
/// `observed_at_unix_ms` and `expires_at_unix_ms` tell its freshness. `records_total` counts
/// the records the read returned, `records_kept` those the answer holds.
pub(crate) fn read(client: &RuntimeClient, request: &FactsRequest) -> Result<Value, Failure> {
    let kept =
        |prefix: Option<&str>, key: &str| prefix.is_none_or(|prefix| key.starts_with(prefix));
    let (read_at_unix_ms, member, body, records_total, records_kept) = match request {
        FactsRequest::Program { key_prefix } => {
            let mut snapshot = client.runtime_fact_snapshot().map_err(Failure::Runtime)?;
            let total = snapshot.records.len();
            snapshot
                .records
                .retain(|record| kept(key_prefix.as_deref(), &record.key));
            let answered = snapshot.records.len();
            (
                snapshot.taken_at_unix_ms,
                "snapshot",
                encode(&snapshot)?,
                total,
                answered,
            )
        }
        FactsRequest::Config => {
            let snapshot = client.runtime_fact_snapshot().map_err(Failure::Runtime)?;
            let records = config_records(&snapshot)?;
            (
                snapshot.taken_at_unix_ms,
                "records",
                encode(&records)?,
                snapshot.records.len(),
                records.len(),
            )
        }
        FactsRequest::Instance {
            selector,
            key_prefix,
        } => {
            let status = client.status().map_err(Failure::Runtime)?;
            let alias = instance_selector::select(status.instances(), selector)
                .map_err(|miss| Failure::Selector {
                    miss,
                    selector: selector.clone(),
                })?
                .instance_alias()
                .to_owned();
            let (mut snapshot, source) = client
                .instance_fact_snapshot(&alias)
                .map_err(Failure::Runtime)?;
            let total = snapshot.records.len();
            snapshot
                .records
                .retain(|record| kept(key_prefix.as_deref(), &record.key));
            let answered = snapshot.records.len();
            (
                source.sampled_completed_at_unix_ms,
                "snapshot",
                encode(&snapshot)?,
                total,
                answered,
            )
        }
    };
    let mut answer = Map::new();
    answer.insert("scope".to_owned(), json!(request.scope().as_str()));
    answer.insert("read_at_unix_ms".to_owned(), json!(read_at_unix_ms));
    answer.insert(member.to_owned(), body);
    answer.insert("records_total".to_owned(), json!(records_total));
    answer.insert("records_kept".to_owned(), json!(records_kept));
    Ok(Value::Object(answer))
}

/// `status --config` (Workflow #387 D1b): `config.subsystems`, `config.selection` when recorded,
/// and every `config.parameters.<section>` in key order; a snapshot without `config.subsystems`
/// or without any section is `config_facts_missing`, never an empty list.
fn config_records(
    snapshot: &RuntimeFactSnapshot,
) -> Result<Vec<&actingcommand_contract::RuntimeFactRecord>, Failure> {
    let record = |key: &str| snapshot.records.iter().find(|record| record.key == key);
    let mut sections = snapshot
        .records
        .iter()
        .filter(|record| record.key.starts_with(CONFIG_PARAMETERS_FACT_PREFIX))
        .collect::<Vec<_>>();
    if sections.is_empty() {
        return Err(Failure::ConfigFactsMissing);
    }
    sections.sort_by(|left, right| left.key.cmp(&right.key));
    let mut records = vec![record(CONFIG_SUBSYSTEMS_FACT_KEY).ok_or(Failure::ConfigFactsMissing)?];
    records.extend(record(CONFIG_SELECTION_FACT_KEY));
    records.extend(sections);
    Ok(records)
}

fn encode(value: &impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(Failure::Output)
}
