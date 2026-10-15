// SPDX-License-Identifier: AGPL-3.0-only

//! `RuntimeOperation::FactRefreshStatus` (Workflow #384 D2-S5, frozen model D2 v1.4.2 §5.1 MP-3,
//! §6 S5, §8 T39; register §1.68): what the scheduler's fact refresh is waiting for now, as the
//! transition recorder holds it after the latest policy cycle. One row per instance with at least
//! one waiting key; per key the reason the latest cycle gave its candidates (the code with its
//! values), the instance's recognition maintenance package R_I when one serves the key, and when
//! the wait began. The read is answered from memory; it reads and records nothing in the ledger,
//! and it takes no part in any decision.

use super::{RuntimeContractError, RuntimeContractResult};
use crate::codes::RuntimeContractCode;
use crate::outcome::{Category, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// The most instance rows one status answers.
pub const MAX_FACT_REFRESH_STATUS_INSTANCES: usize = 256;
/// The most waiting keys one instance row answers.
pub const MAX_FACT_REFRESH_STATUS_DEMANDS: usize = 256;

/// One key an instance's candidates wait for (D2 model §3.7): the latest cycle's reason, which is
/// `fact_refresh_pending`, `fact_refresh_unresolved`, `fact_refresh_exhausted` or
/// `fact_refresh_unbound` with its values; R_I's package id when a reader serves the key; when
/// the wait began (the cycle that first found the key unknown); and whether
/// `fact_reader_overdue` was reported for this wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactRefreshDemand {
    pub fact_key: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    pub since_unix_ms: u64,
    pub overdue: bool,
}

/// One instance and the keys its candidates wait for, by key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactRefreshInstance {
    pub instance_alias: String,
    pub demands: Vec<RuntimeFactRefreshDemand>,
}

/// The answer of `FactRefreshStatus`: the number of the latest policy cycle the recorder observed
/// in this owner epoch (0 before the first), the number of its latest transition (0 before the
/// first; numbered from 1 per owner epoch, one by one, never reset), and one row per instance
/// with a waiting key, by alias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFactRefreshStatus {
    pub cycle_seq: u64,
    pub transition_seq: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<RuntimeFactRefreshInstance>,
}

impl RuntimeFactRefreshStatus {
    /// The shape rules: no row before the first cycle; each instance once by a valid alias with
    /// one to [`MAX_FACT_REFRESH_STATUS_DEMANDS`] waiting keys, each key once and valid; every
    /// reason an info or a warning naming the row's instance and key, and exactly the row's
    /// package id (none when the row has none); and the bounds above.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let invalid = || RuntimeContractError::new(RuntimeContractCode::InvalidFactRefreshStatus);
        if self.instances.len() > MAX_FACT_REFRESH_STATUS_INSTANCES
            || (self.cycle_seq == 0 && !self.instances.is_empty())
        {
            return Err(invalid());
        }
        let mut aliases = BTreeSet::new();
        for row in &self.instances {
            if crate::validate_instance_alias(&row.instance_alias).is_err()
                || !aliases.insert(row.instance_alias.as_str())
                || row.demands.is_empty()
                || row.demands.len() > MAX_FACT_REFRESH_STATUS_DEMANDS
            {
                return Err(invalid());
            }
            let mut keys = BTreeSet::new();
            for demand in &row.demands {
                let names = |key: &str, expected: &str| {
                    demand.outcome.value(key).and_then(Value::as_str) == Some(expected)
                };
                if crate::fact::validate_fact_key(&demand.fact_key).is_err()
                    || !keys.insert(demand.fact_key.as_str())
                    || !matches!(
                        demand.outcome.category(),
                        Category::Info | Category::Warning
                    )
                    || !names("instance_id", &row.instance_alias)
                    || !names("fact_key", &demand.fact_key)
                    || demand.outcome.value("package_id").and_then(Value::as_str)
                        != demand.package_id.as_deref()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}
