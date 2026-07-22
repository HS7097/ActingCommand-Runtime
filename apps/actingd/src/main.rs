// SPDX-License-Identifier: AGPL-3.0-only

//! Thin process adapter for the resident ActingCommand Runtime.

#![forbid(unsafe_code)]

mod config;

use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalPayload, ApprovalTarget, EventActor,
    EventPayload, EventQuery, EventSource, EventType, ProjectionPayload, ProjectionProfile,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use actingcommand_runtime_host::{
    PolicyAdmissionContext, PolicyDispatchAdmission, PolicyTrigger, RuntimeHost, RuntimeHostError,
};
use config::{PolicyBootstrap, RuntimeAssembly};
use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL actingd: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
    let config_path = parse_arguments(arguments)?;
    let RuntimeAssembly {
        host,
        registry,
        policy,
    } = config::load(&config_path)
        .and_then(config::ActingdConfigFile::assemble)
        .map_err(ActingdError::config)?;
    let host = RuntimeHost::start(host, Arc::new(registry)).map_err(ActingdError::runtime)?;
    if let Some(policy) = policy
        && let Err(error) = initialize_policy(&host, &policy)
    {
        return match host.close() {
            Ok(()) => Err(error),
            Err(close_error) => Err(ActingdError::runtime(close_error)),
        };
    }
    println!(
        "actingd ready pid={} host={} port={}",
        host.runtime_info().pid(),
        host.runtime_info().host(),
        host.runtime_info().port()
    );
    monitor(host)
}

fn initialize_policy(host: &RuntimeHost, policy: &PolicyBootstrap) -> Result<(), ActingdError> {
    let generation = host
        .activate_policy_catalog(&policy.catalog)
        .map_err(ActingdError::runtime)?;
    let governance = RuntimeClient::connect(
        RuntimeClientConfig::new(&policy.state_root, EventActor::User, EventSource::Ui)
            .with_io_timeout(Duration::from_secs(5)),
    )
    .map_err(ActingdError::client)?;
    governance
        .authenticate_governance(&policy.governance_capability)
        .map_err(ActingdError::client)?;
    let approval_events = governance
        .query_events(
            EventQuery {
                event_type: Some(EventType::ApprovalDecision),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .map_err(ActingdError::client)?;
    for approval_id in &policy.catalog_approval_ids {
        let decision = ApprovalDecisionRecord::new(
            approval_id,
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: generation.catalog_hash().to_owned(),
                catalog_version: generation.catalog_version(),
            },
            "configured_catalog_approval",
        )
        .map_err(|_| ActingdError::process("policy_catalog_approval_invalid"))?;
        let existing = approval_events
            .iter()
            .filter_map(|event| match &event.payload {
                ProjectionPayload::Full(payload) => match payload.as_ref() {
                    EventPayload::Approval(ApprovalPayload::Decision(payload))
                        if payload.decision().approval_id() == approval_id =>
                    {
                        Some((event.sequence, payload.decision()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .max_by_key(|(sequence, _)| *sequence)
            .map(|(_, decision)| decision);
        if let Some(existing) = existing {
            // A persisted rejection or revocation cannot be silently replaced by startup config.
            if existing != &decision {
                return Err(ActingdError::process("policy_catalog_approval_conflict"));
            }
            continue;
        }
        governance
            .record_approval_decision(decision)
            .map_err(ActingdError::client)?;
    }
    drop(governance);
    let cycle = host
        .evaluate_policy_cycle(PolicyTrigger::Recovery)
        .map_err(ActingdError::runtime)?;
    let Some(evaluation) = cycle.evaluation.as_ref() else {
        if cycle.pending_dispatch_intents.is_empty() {
            return Ok(());
        }
        return Err(ActingdError::process(
            "policy_pending_dispatch_without_evaluation",
        ));
    };
    for intent in &cycle.pending_dispatch_intents {
        let reason_chain = evaluation
            .reason_chains
            .iter()
            .find(|reason_chain| reason_chain.id == intent.reason_chain_id)
            .ok_or_else(|| ActingdError::process("policy_reason_chain_missing"))?;
        let admission = host
            .admit_policy_dispatch(
                intent,
                reason_chain,
                &PolicyAdmissionContext {
                    fact_ledger_position: intent.input_ledger_position,
                    fact_snapshot_id: intent.fact_snapshot_id.clone(),
                    approval_fact_ids: intent
                        .approval_refs
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                    fencing_owner_epoch: host.runtime_info().owner_epoch(),
                    now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
                },
            )
            .map_err(ActingdError::runtime)?;
        let PolicyDispatchAdmission::Granted { context } = admission else {
            continue;
        };
        if let Some(task) = policy.scheduled_tasks.get(context.procedure_ref()) {
            let receipt = host
                .run_scheduled_contained_task(&context, task)
                .map_err(ActingdError::runtime)?;
            host.complete_scheduled_policy_run(&context, &receipt)
                .map_err(ActingdError::runtime)?;
        }
    }
    Ok(())
}

fn monitor(host: RuntimeHost) -> Result<(), ActingdError> {
    loop {
        thread::sleep(HEALTH_POLL_INTERVAL);
        match host.fatal_error().map_err(ActingdError::runtime)? {
            Some(error) => {
                let close_error = host.close().err();
                return Err(
                    close_error.map_or_else(|| ActingdError::runtime(error), ActingdError::runtime)
                );
            }
            None => continue,
        }
    }
}

fn parse_arguments(arguments: Vec<std::ffi::OsString>) -> Result<PathBuf, ActingdError> {
    let [flag, path] = arguments.as_slice() else {
        return Err(ActingdError::config("usage_invalid"));
    };
    if flag != "--config" || path.is_empty() {
        return Err(ActingdError::config("usage_invalid"));
    }
    Ok(PathBuf::from(path))
}

#[derive(Debug)]
struct ActingdError {
    code: &'static str,
    runtime: Option<Box<RuntimeHostError>>,
    client: Option<Box<RuntimeClientError>>,
}

impl ActingdError {
    const fn config(code: &'static str) -> Self {
        Self {
            code,
            runtime: None,
            client: None,
        }
    }

    const fn process(code: &'static str) -> Self {
        Self {
            code,
            runtime: None,
            client: None,
        }
    }

    fn runtime(error: RuntimeHostError) -> Self {
        Self {
            code: error.code(),
            runtime: Some(Box::new(error)),
            client: None,
        }
    }

    fn client(error: RuntimeClientError) -> Self {
        Self {
            code: error.code(),
            runtime: None,
            client: Some(Box::new(error)),
        }
    }
}

impl fmt::Display for ActingdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.runtime {
            Some(error) => error.fmt(formatter),
            None => match &self.client {
                Some(error) => error.fmt(formatter),
                None => formatter.write_str(self.code),
            },
        }
    }
}

impl Error for ActingdError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn process_adapter_requires_exact_config_argument() {
        assert!(parse_arguments(Vec::new()).is_err());
        assert!(parse_arguments(vec![OsString::from("--config")]).is_err());
        assert!(
            parse_arguments(vec![
                OsString::from("--config"),
                OsString::from("actingd.json")
            ])
            .is_ok()
        );
    }
}
