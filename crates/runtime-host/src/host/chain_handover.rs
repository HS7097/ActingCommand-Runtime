// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 B41: what the scheduler's chain handover reads from the Runtime
//! (`contracts/scheduling/README.md`, "Chain Handover"): the page each instance rests on, its
//! `task.page` runtime fact (the page the instance's latest recognition matched) with the time it
//! was recorded, and whether a task's package entry holds a page, read from the package its
//! procedure binding locates. Both are read at the cycle and kept nowhere; a takeover, an
//! unreachable ADB or an emulator restart drops `task.page`, so after them no instance has a
//! handover until a run recognizes a page again.

use super::runtime_facts::TASK_PAGE_FACT_KEY;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::{ProcedureBinding, ProcedureManifest};
use actingcommand_contract::FactValue;
use actingcommand_execution_kernel::{PreparedContainedTask, canonical_page_anchor};
use actingcommand_policy::{ChainHandoverInputs, InstanceSnapshot, RestingPage, TaskSpec};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long the entry read of one package may take (one bounded snapshot, as the task package
/// admission check reads one).
const ENTRY_READ_BUDGET: Duration = Duration::from_secs(60);
/// The instance label a package's entry is read under: the entry depends only on the package.
const ENTRY_READ_LABEL: &str = "chain-handover";

impl HostShared {
    /// The page each instance of `instances` rests on: its `task.page` runtime fact, when it has
    /// one. A successor waiting for its random delay (`dispatch_random_delay_waiting`, position 9
    /// of the order of deferrals) still goes first: the delay is part of its dispatch.
    pub(super) fn chain_handover_inputs(
        &self,
        instances: &[InstanceSnapshot],
    ) -> RuntimeHostResult<ChainHandoverInputs> {
        let registered = lock(
            &self.registered_instances,
            HostLocation::EvaluatePolicyCycle,
        )?
        .values()
        .map(|instance| (instance.instance_alias.clone(), instance.instance_id))
        .collect::<BTreeMap<_, _>>();
        let facts = lock(&self.runtime_facts, HostLocation::EvaluatePolicyCycle)?;
        let mut resting_pages = BTreeMap::new();
        for instance in instances {
            let Some(instance_id) = registered.get(&instance.instance_id) else {
                continue;
            };
            let scope = RuntimeFactScope::Instance {
                instance_id: *instance_id,
            };
            if let Some(record) = facts.get(&scope, TASK_PAGE_FACT_KEY)
                && let FactValue::String(page_id) = &record.value
            {
                resting_pages.insert(
                    instance.instance_id.clone(),
                    RestingPage {
                        page_id: page_id.clone(),
                        observed_at_unix_ms: record.observed_at_unix_ms,
                    },
                );
            }
        }
        Ok(ChainHandoverInputs {
            resting_pages,
            successor_wait_codes: vec![HostCode::DispatchRandomDelayWaiting.code()],
        })
    }
}

/// Whether the entry page set of `task`'s package holds `page_id`: the package its procedure
/// binding locates, admitted as preparation admits it, with its declared entry page or entry
/// page set and the entry its entry gate checks, each page without the game prefix. A task whose
/// procedure locates no package, or whose package cannot be read now, holds no page: the round
/// keeps the ranked order, and the run's own admission refuses the package with its code.
pub(crate) fn chain_entry_holds_page(
    manifest: &ProcedureManifest,
    task: &TaskSpec,
    page_id: &str,
) -> bool {
    let Some(path) = manifest
        .binding(&task.procedure_ref)
        .and_then(ProcedureBinding::package_path)
    else {
        return false;
    };
    let Ok(package) = PreparedContainedTask::describe_navigation_path(
        ENTRY_READ_LABEL,
        Path::new(path),
        Instant::now() + ENTRY_READ_BUDGET,
    ) else {
        return false;
    };
    let page = canonical_page_anchor(package.descriptor().game(), page_id);
    package.entry_holds_page(&page)
        || package
            .navigation_entry_pages()
            .is_some_and(|pages| pages.contains(&page))
}
