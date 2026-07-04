// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_arbitrator::{
    ArbitrationDecision, DegradedArbitrator, InstanceArbitration, LeaseGrant, RequestEnvelope,
    RequestPriority, RequestSource, RequestVerb,
};
use actingcommand_ledger::{
    EvidenceStore, IdIssuer, IdKind, LabLedger, LedgerRecord, LedgerRecordKind, LightEvent,
    ProjectionRequest, ProjectionVerbosity, SessionHeader, enforce_retention, error_projection,
    forbidden_target_suspicion, guard_reject_suspicion, low_margin_suspicion, project_record,
    stale_frame_suspicion,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use std::{env, fs};

use super::*;

struct Lab2Ids {
    issuer: IdIssuer,
    req_id: String,
}

impl Lab2Ids {
    fn new() -> Self {
        let issuer = IdIssuer::new();
        let req_id = issuer.issue(IdKind::Req).value;
        Self { issuer, req_id }
    }

    fn issue(&self, kind: IdKind) -> String {
        self.issuer.issue(kind).value
    }
}

struct Lab2Scene {
    scene: Scene,
    backend: String,
    source: Value,
    frame_age_ms: u64,
    png: Option<Vec<u8>>,
}

struct WaitIds<'a> {
    req_id: &'a str,
    wf_id: &'a str,
}

#[derive(Clone, Copy)]
struct WaitTiming {
    timeout: Duration,
    poll: Duration,
}

struct Lab2ClickRect {
    kind: &'static str,
    rect: PackRect,
    derivation: Value,
}

const LAB2_RECOVERY_STATE_FILE: &str = "lab2-recovery-state.json";
const LAB2_ARBITRATOR_STATE_FILE: &str = "lab2-arbitrator-state.json";
const LAB2_ARBITRATOR_STATE_VERSION: &str = "actingcommand.lab2.arbitrator_state.v0.1";
const LAB2_LEDGER_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const LAB2_LEDGER_PROTECTED_DAYS: u64 = 7;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Lab2ArbitratorState {
    schema_version: String,
    updated_at_unix_ms: u64,
    #[serde(default)]
    instances: BTreeMap<String, InstanceArbitration>,
}

#[derive(Clone, Copy)]
struct Lab2CommandContract {
    name: &'static str,
    summary: &'static str,
    required: &'static [&'static str],
    optional: &'static [&'static str],
    output_fields: &'static [&'static str],
    requires_lease: bool,
}

pub(crate) fn run_observe(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let instance = lab2_instance(global, &flags);
    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Observe,
        json!({"targets": target_list(&flags)}),
        &flags,
    )?;
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let loaded_scene = load_lab2_scene(global, &flags)?;
    let outcome = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
    let frame_path = write_frame_if_requested(&flags, &loaded_scene)?;
    let targets = observe_targets(&evaluator, &loaded_scene.scene, &flags, &outcome)?;
    let actions = observe_actions(global, &config, &flags, &outcome)?;
    let mut payload = json!({
        "req_id": ids.req_id,
        "state": if outcome.matched { "observed" } else { "unknown" },
        "instance": instance,
        "page": if outcome.matched { outcome.page.clone() } else { "unknown".to_string() },
        "matched": outcome.matched,
        "standby": outcome.standby,
        "frame_age_ms": loaded_scene.frame_age_ms,
        "backend": loaded_scene.backend,
        "frame_source": loaded_scene.source,
        "targets": targets,
        "actions": actions,
        "arbitration": arbitration_json(&arbitration.decision),
    });
    if !outcome.matched {
        payload["candidates"] = json!(lab2_page_candidates(&outcome));
    }
    if let Some(suspicion) = lab2_observation_suspicion(&outcome, loaded_scene.frame_age_ms) {
        payload["suspicion"] = suspicion;
    }
    if let Some(recovery) = active_lab2_recovery_state(&flags, &ids.req_id, &instance)? {
        payload["state"] = json!("recovering");
        payload["recovery"] = recovery;
    }
    if let Some(path) = frame_path {
        payload["frame_path"] = json!(path.display().to_string());
    }
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
}

pub(crate) fn run_do(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let target = target_argument(&flags, "do")?;
    let instance = lab2_instance(global, &flags);
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let allow_destructive = flags.bool("--allow-destructive");
    if flags.bool("--destructive") && !allow_destructive {
        let details = lab2_error_details_for_flags(
            &flags,
            &ids.req_id,
            "resource_drift",
            "blocked",
            "rerun-with---allow-destructive-if-this-action-is-intended",
        )?;
        return Err(CliError::safety_blocked(
            "destructive_action_requires_allow_destructive",
            "do --destructive requires --allow-destructive",
            &["allow_destructive"],
        )
        .with_details(details));
    }
    if !allow_destructive {
        reject_dangerous_semantic_id("target", &target)?;
    }
    if !dry_run && !flags.bool("--capture") {
        return Err(CliError::usage(
            "do real execution requires --capture; use --dry-run with --scene for offline planning",
        ));
    }
    let recovery_wait = wait_for_lab2_recovery_clear(
        &flags,
        &ids.req_id,
        &instance,
        "do",
        json!({"verb": "do", "target": target.clone(), "dry_run": dry_run}),
    )?;

    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Do,
        json!({"target": target, "dry_run": dry_run}),
        &flags,
    )?;
    let write_lease =
        match ensure_lab2_write_admitted(&flags, &ids.req_id, &instance, &arbitration.decision) {
            Ok(lease) => lease,
            Err(error) => {
                let details = cli_error_details_or_projection(
                    &error,
                    &ids.req_id,
                    "lease_held",
                    "blocked",
                    "inspect-lab2-arbitrator-state",
                );
                return return_lab2_error_with_ledger(
                    global,
                    &flags,
                    &ids.req_id,
                    &instance,
                    details,
                    error,
                    arbitration.ledger_records.clone(),
                );
            }
        };
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    guard_evaluable_target(&evaluator, &target, "do")?;
    let loaded_scene = load_lab2_scene(global, &flags)?;
    let before = detect_current_page(&evaluator, &detector, &loaded_scene.scene)?;
    let reco_id = ids.issue(IdKind::Reco);
    let evaluation = evaluator
        .evaluate_target(&loaded_scene.scene, &target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if !evaluation.passed {
        let details = lab2_error_payload_for_flags(
            &flags,
            &ids.req_id,
            "resource_drift",
            before.page,
            "observe-current-page-and-refresh-resource-or-target",
            Some(guard_reject_suspicion(&target, &evaluation.message)),
        )?;
        let error = CliError::safety_blocked(
            "target_not_visible",
            format!(
                "target '{target}' did not pass guard recognition: {}",
                evaluation.message
            ),
            &["guard_target"],
        );
        return return_lab2_error_with_ledger(
            global,
            &flags,
            &ids.req_id,
            &instance,
            details,
            error,
            arbitration.ledger_records.clone(),
        );
    }
    let click = evaluator
        .get_click_target(&target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    let actual_click = derive_lab2_click_rect(&evaluator, &target, click, &evaluation)?;
    if !allow_destructive {
        let graph = load_navigation_graph(global, &config, &flags)?;
        if let Err(error) =
            reject_lab2_destructive_click_overlap(&target, &before.page, actual_click.rect, &graph)
        {
            let details = lab2_error_payload_for_flags(
                &flags,
                &ids.req_id,
                "resource_drift",
                "blocked",
                "rerun-with---allow-destructive-if-this-action-is-intended",
                Some(forbidden_target_suspicion(vec![target.clone()])),
            )?;
            return return_lab2_error_with_ledger(
                global,
                &flags,
                &ids.req_id,
                &instance,
                details,
                error,
                arbitration.ledger_records.clone(),
            );
        }
    }
    let point = rect_center(actual_click.rect)?;
    let action_id = ids.issue(IdKind::Action);
    let device = if dry_run {
        json!({"executed": false, "mode": "dry_run"})
    } else {
        authorize_lab2_device_drive(global, &flags, &ids.req_id, &instance, &write_lease)?;
        send_semantic_tap(global, &config, point)?
    };
    let after = if dry_run {
        before
    } else {
        let after_scene = load_lab2_scene(global, &flags)?;
        detect_current_page(&evaluator, &detector, &after_scene.scene)?
    };
    let payload = json!({
        "req_id": ids.req_id,
        "reco_id": reco_id,
        "action_id": action_id,
        "state": if dry_run { "planned" } else { "sent" },
        "instance": instance,
        "executed": !dry_run,
        "target": target,
        "page": after.page,
        "frame_age_ms": loaded_scene.frame_age_ms,
        "backend": loaded_scene.backend,
        "actual_click": {
            "kind": actual_click.kind,
            "declared_rect": rect_json(click),
            "rect": rect_json(actual_click.rect),
            "point": point_json(point),
            "coordinate_derivation": actual_click.derivation
        },
        "guard_result": {
            "reco_id": reco_id,
            "target": target,
            "passed": true,
            "evaluation": target_eval_json(&evaluation)
        },
        "observation": page_detection_json(&after),
        "device": device,
        "arbitration": arbitration_json(&arbitration.decision),
    });
    let mut payload = payload;
    if let Some(recovery_wait) = recovery_wait {
        payload["recovery_wait"] = recovery_wait;
    }
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
}

pub(crate) fn run_ensure(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let to = ensure_target_page(&flags)?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let allow_destructive = flags.bool("--allow-destructive");
    if !dry_run && !flags.bool("--capture") {
        return Err(CliError::usage(
            "ensure real execution requires --capture; use --dry-run with --scene for route planning",
        ));
    }
    let instance = lab2_instance(global, &flags);
    let recovery_wait = wait_for_lab2_recovery_clear(
        &flags,
        &ids.req_id,
        &instance,
        "ensure",
        json!({"verb": "ensure", "to": to.clone(), "dry_run": dry_run}),
    )?;
    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Ensure,
        json!({"to": to, "dry_run": dry_run}),
        &flags,
    )?;
    let write_lease =
        match ensure_lab2_write_admitted(&flags, &ids.req_id, &instance, &arbitration.decision) {
            Ok(lease) => lease,
            Err(error) => {
                let details = cli_error_details_or_projection(
                    &error,
                    &ids.req_id,
                    "lease_held",
                    "blocked",
                    "inspect-lab2-arbitrator-state",
                );
                return return_lab2_error_with_ledger(
                    global,
                    &flags,
                    &ids.req_id,
                    &instance,
                    details,
                    error,
                    arbitration.ledger_records.clone(),
                );
            }
        };
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let graph = load_navigation_graph(global, &config, &flags)?;
    let scene = load_lab2_scene(global, &flags)?;
    let start = detect_current_page(&evaluator, &detector, &scene.scene)?;
    let target_page = canonical_navigation_page(&graph, &to);
    if start.matched && start.page == target_page {
        let payload = json!({
            "req_id": ids.req_id,
            "state": "already_at_target",
            "instance": instance,
            "executed": false,
            "page": start.page,
            "to": target_page,
            "route": [],
            "frame_age_ms": scene.frame_age_ms,
            "backend": scene.backend,
            "arbitration": arbitration_json(&arbitration.decision),
        });
        let mut payload = payload;
        if let Some(recovery_wait) = recovery_wait {
            payload["recovery_wait"] = recovery_wait;
        }
        return finish_lab2_response(
            global,
            &flags,
            &ids.req_id,
            &instance,
            payload,
            arbitration.ledger_records,
        );
    }
    if !start.matched {
        let details = lab2_error_payload_for_flags(
            &flags,
            &ids.req_id,
            "resource_drift",
            start.page,
            "observe-current-page-or-route-home-before-ensure",
            None,
        )?;
        let error = CliError::safety_blocked(
            "current_page_unknown",
            "ensure requires a matched current page before navigation",
            &["current_page"],
        );
        return return_lab2_error_with_ledger(
            global,
            &flags,
            &ids.req_id,
            &instance,
            details,
            error,
            arbitration.ledger_records.clone(),
        );
    }
    let route =
        find_navigation_route(&graph.edges, &start.page, &target_page).ok_or_else(|| {
            CliError::usage(format!(
                "no navigation route from '{}' to '{}'",
                start.page, target_page
            ))
        })?;
    for edge in &route {
        if !allow_destructive {
            reject_dangerous_semantic_id("navigation edge", &edge.id)?;
            reject_destructive_overlap(edge, &graph.destructive_clicks)?;
        }
    }
    let route_json = route.iter().map(navigation_edge_json).collect::<Vec<_>>();
    if dry_run {
        let payload = json!({
            "req_id": ids.req_id,
            "state": "planned",
            "instance": instance,
            "executed": false,
            "page": start.page,
            "to": target_page,
            "route": route_json,
            "frame_age_ms": scene.frame_age_ms,
            "backend": scene.backend,
            "arbitration": arbitration_json(&arbitration.decision),
        });
        let mut payload = payload;
        if let Some(recovery_wait) = recovery_wait {
            payload["recovery_wait"] = recovery_wait;
        }
        return finish_lab2_response(
            global,
            &flags,
            &ids.req_id,
            &instance,
            payload,
            arbitration.ledger_records,
        );
    }

    let step_timeout = parse_optional_duration_ms(&flags, "--step-timeout-ms", 5_000)?;
    let poll = parse_optional_duration_ms(&flags, "--poll-ms", 500)?;
    let execution = NavigationExecutionContext {
        global,
        flags: &flags,
        config: &config,
        evaluator: &evaluator,
        detector: &detector,
        step_timeout,
        poll,
    };
    authorize_lab2_device_drive(global, &flags, &ids.req_id, &instance, &write_lease)?;
    let (steps, arrived) = execute_navigation_route(&execution, start.page.clone(), route)?;
    let payload = json!({
        "req_id": ids.req_id,
        "state": "arrived",
        "instance": instance,
        "executed": true,
        "from": start.page,
        "page": arrived,
        "to": target_page,
        "route": route_json,
        "steps": steps,
        "arbitration": arbitration_json(&arbitration.decision),
    });
    let mut payload = payload;
    if let Some(recovery_wait) = recovery_wait {
        payload["recovery_wait"] = recovery_wait;
    }
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
}

pub(crate) fn run_wait(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let ids = Lab2Ids::new();
    let wf_id = ids.issue(IdKind::Wf);
    let instance = lab2_instance(global, &flags);
    let arbitration = admit_lab2_request(
        &ids,
        instance.clone(),
        RequestVerb::Wait,
        json!({"page": flags.optional("--page"), "stable": flags.optional("--stable")}),
        &flags,
    )?;
    let config = read_user_config()?;
    let (evaluator, detector) = load_semantic_detector(global, &config, &flags)?;
    let timeout = parse_optional_duration_ms(&flags, "--timeout-ms", 5_000)?;
    let poll = parse_optional_duration_ms(&flags, "--poll-ms", 200)?;
    let wait_ids = WaitIds {
        req_id: &ids.req_id,
        wf_id: &wf_id,
    };
    let timing = WaitTiming { timeout, poll };
    let payload = if let Some(page) = wait_page_target(&flags) {
        wait_for_page(
            global, &flags, &evaluator, &detector, wait_ids, &page, timing,
        )?
    } else if let Some(target) = flags.optional("--stable").filter(|value| value != "true") {
        wait_for_stable_target(global, &flags, &evaluator, wait_ids, &target, timing)?
    } else {
        return Err(CliError::usage(
            "wait requires --page <page> or --stable <target>",
        ));
    };
    let mut payload = payload;
    payload["instance"] = json!(instance);
    payload["arbitration"] = arbitration_json(&arbitration.decision);
    finish_lab2_response(
        global,
        &flags,
        &ids.req_id,
        &instance,
        payload,
        arbitration.ledger_records,
    )
}

pub(crate) fn run_receipt(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let req_id = flags
        .optional("--req")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("lab receipt requires --req <req_id>"))?;
    let config = read_user_config()?;
    let run_root = effective_run_root(global, &config)
        .ok_or_else(|| CliError::usage("lab receipt requires --run-root or config run_root"))?;
    let records = read_receipt_chain(&run_root, &req_id)?;
    if records.is_empty() {
        return Err(CliError::usage(format!(
            "no lab ledger records found for req_id '{req_id}'"
        )));
    }
    Ok(json!({
        "req_id": req_id,
        "ledger_count": distinct_ledger_count(&records),
        "record_count": records.len(),
        "records": records
    }))
}

pub(crate) fn run_evidence(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let evidence_id = flags
        .optional("--id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("lab evidence requires --id <evidence_id>"))?;
    let config = read_user_config()?;
    let run_root = effective_run_root(global, &config)
        .ok_or_else(|| CliError::usage("lab evidence requires --run-root or config run_root"))?;
    let refs = EvidenceStore::new(&run_root, true)
        .list_by_id(&evidence_id)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "evidence_id": evidence_id,
        "count": refs.len(),
        "evidence": refs
    }))
}

pub(crate) fn run_arbitrator(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let command = flags
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or("status");
    let instance = lab2_instance(global, &flags);
    let (state_path, mut arbitrator) = load_lab2_arbitrator(&flags)?;
    let mut records = Vec::new();
    let data = match command {
        "status" => json!({
            "state": "status",
            "instance": instance,
            "state_file": state_path.as_ref().map(|path| path.display().to_string()),
            "arbitration": arbitrator.snapshot(&instance)
        }),
        "release" => {
            let lease_id = flags
                .optional("--lease-id")
                .filter(|value| value != "true")
                .or_else(|| flags.positionals.get(1).cloned())
                .ok_or_else(|| {
                    CliError::usage("lab arbitrator release requires --lease-id <id>")
                })?;
            let outcome = arbitrator
                .release(&instance, &lease_id, current_unix_ms())
                .map_err(|err| CliError::device(err.to_string()))?;
            records.extend(outcome.ledger_records.clone());
            json!({
                "state": outcome.decision.as_str(),
                "instance": instance,
                "arbitration": arbitration_json(&outcome.decision)
            })
        }
        "cancel" => {
            let req_id = flags
                .optional("--req")
                .filter(|value| value != "true")
                .or_else(|| flags.positionals.get(1).cloned())
                .ok_or_else(|| CliError::usage("lab arbitrator cancel requires --req <req_id>"))?;
            let reason = flags
                .optional("--reason")
                .filter(|value| value != "true")
                .unwrap_or_else(|| "operator_cancelled".to_string());
            let outcome = arbitrator
                .cancel_queued(&instance, &req_id, reason)
                .map_err(|err| CliError::device(err.to_string()))?;
            records.extend(outcome.ledger_records.clone());
            json!({
                "state": outcome.decision.as_str(),
                "instance": instance,
                "arbitration": arbitration_json(&outcome.decision)
            })
        }
        "reclaim-dead" => {
            arbitrator.mark_holder_dead(&instance);
            let outcome = arbitrator
                .reclaim_dead_holder(&instance, current_unix_ms())
                .map_err(|err| CliError::device(err.to_string()))?;
            records.extend(outcome.ledger_records.clone());
            json!({
                "state": outcome.decision.as_str(),
                "instance": instance,
                "arbitration": arbitration_json(&outcome.decision)
            })
        }
        "mark-destructive" => {
            let active = flags
                .optional("--active")
                .filter(|value| value != "true")
                .map(|value| parse_bool_flag_value("--active", &value))
                .transpose()?
                .unwrap_or(true);
            arbitrator.mark_holder_destructive_step(&instance, active);
            json!({
                "state": "holder_destructive_step_marked",
                "instance": instance,
                "active": active,
                "arbitration": arbitrator.snapshot(&instance)
            })
        }
        other => {
            return Err(CliError::usage(format!(
                "unknown lab arbitrator command: {other}"
            )));
        }
    };
    save_lab2_arbitrator(state_path.as_deref(), &arbitrator)?;
    if records.is_empty() {
        return Ok(data);
    }
    let req_id = records
        .iter()
        .find_map(|record| record.req_id.clone())
        .unwrap_or_else(|| "lab2-arbitrator".to_string());
    let mut data = data;
    data["ledger"] = write_lab2_ledger(global, &instance, &req_id, &data, &mut records)?;
    Ok(data)
}

pub(crate) fn capability_summary(config: &UserConfig) -> Value {
    let commands = lab2_command_contracts()
        .into_iter()
        .map(command_contract_summary)
        .collect::<Vec<_>>();
    json!({
        "schema_version": "actingcommand.lab2.capabilities.v0.1",
        "verbs": commands,
        "click_kinds": ["target_rect_center"],
        "schema_versions": {
            "min": "0.3",
            "max": "0.5",
            "supported": ["0.3", "0.4", "0.5"]
        },
        "engine_capabilities": recognition_engine_capabilities(),
        "instances": lab2_config_instances(config),
        "recovery_transparency": {
            "state_file": LAB2_RECOVERY_STATE_FILE,
            "event_type": "recovery.state.changed",
            "observe": "allowed_with_state_recovering",
            "write_verbs": "wait_by_default_or_fail_fast_with_--no-wait",
            "wait_flags": ["--recovery-timeout-ms <ms>", "--recovery-poll-ms <ms>"]
        },
        "error_codes": lab2_error_code_table(),
        "exit_codes": exit_code_table(),
        "escape_toolbox": escape_toolbox_groups()
    })
}

pub(crate) fn command_schema(command: &str) -> Option<Value> {
    let normalized = command.trim();
    lab2_command_contracts()
        .into_iter()
        .find(|contract| contract.name == normalized)
        .map(command_contract_schema)
}

fn admit_lab2_request(
    ids: &Lab2Ids,
    instance: String,
    verb: RequestVerb,
    payload: Value,
    flags: &FlagArgs,
) -> CliOutcome<actingcommand_arbitrator::ArbitrationOutcome> {
    let mut request = RequestEnvelope::new(
        ids.req_id.clone(),
        RequestSource::Cli,
        instance.clone(),
        verb,
        payload,
        current_unix_ms(),
    );
    request.allow_destructive = flags.bool("--allow-destructive");
    request.priority = match flags.optional("--priority").as_deref() {
        Some("high") => RequestPriority::High,
        Some("normal") | None => RequestPriority::Normal,
        Some(other) => {
            return Err(CliError::usage(format!(
                "unsupported --priority '{other}', expected normal or high"
            )));
        }
    };
    if let Some(deadline) = flags
        .optional("--queue-deadline-ms")
        .filter(|value| value != "true")
    {
        request.queue_deadline_ms = Some(deadline.parse::<u64>().map_err(|err| {
            CliError::usage(format!("invalid --queue-deadline-ms '{deadline}': {err}"))
        })?);
    }
    let request_record = LedgerRecord::new(
        LedgerRecordKind::Dispatch,
        Some(ids.req_id.clone()),
        json!({
            "stage": "request",
            "request": request
        }),
    );
    let (state_path, mut arbitrator) = match load_lab2_arbitrator(flags) {
        Ok(loaded) => loaded,
        Err(err) => {
            let details = lab2_error_details_for_flags(
                flags,
                &ids.req_id,
                "fatal",
                "arbitrator_state_unavailable",
                "inspect-or-remove-corrupt-lab2-arbitrator-state",
            )?;
            return Err(err.with_details(details));
        }
    };
    let now_ms = current_unix_ms();
    let expired_queued = arbitrator
        .snapshot(&instance)
        .queued
        .filter(|queued| now_ms > queued.deadline_ms);
    let mut outcome = arbitrator
        .admit(request, now_ms)
        .map_err(|err| CliError::device(err.to_string()))?;
    save_lab2_arbitrator(state_path.as_deref(), &arbitrator)?;
    outcome.ledger_records.insert(0, request_record);
    if let Some(queued) = expired_queued {
        outcome.ledger_records.push(LedgerRecord::new(
            LedgerRecordKind::Dispatch,
            Some(queued.request.req_id.clone()),
            json!({
                "stage": "queue_deadline_expired",
                "req_id": queued.request.req_id,
                "instance": queued.request.instance,
                "deadline_ms": queued.deadline_ms,
                "state": "queue_deadline_expired",
                "hint": "resubmit-or-escalate-priority"
            }),
        ));
    }
    Ok(outcome)
}

fn load_lab2_arbitrator(flags: &FlagArgs) -> CliOutcome<(Option<PathBuf>, DegradedArbitrator)> {
    let Some(state_dir) = explicit_lab2_state_dir(flags)? else {
        return Ok((None, DegradedArbitrator::new(IdIssuer::new())));
    };
    fs::create_dir_all(&state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to create Lab-2 arbitrator state dir {}: {err}",
            state_dir.display()
        ))
    })?;
    let path = state_dir.join(LAB2_ARBITRATOR_STATE_FILE);
    let Some(state) = read_json_file::<Lab2ArbitratorState>(&path)? else {
        return Ok((Some(path), DegradedArbitrator::new(IdIssuer::new())));
    };
    if state.schema_version != LAB2_ARBITRATOR_STATE_VERSION {
        return Err(CliError::device(format!(
            "unsupported Lab-2 arbitrator state schema {} in {}",
            state.schema_version,
            path.display()
        )));
    }
    Ok((
        Some(path),
        DegradedArbitrator::from_instances(IdIssuer::new(), state.instances),
    ))
}

fn save_lab2_arbitrator(path: Option<&Path>, arbitrator: &DegradedArbitrator) -> CliOutcome<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let state = Lab2ArbitratorState {
        schema_version: LAB2_ARBITRATOR_STATE_VERSION.to_string(),
        updated_at_unix_ms: current_unix_ms(),
        instances: arbitrator.instances().clone(),
    };
    write_json_file_atomic(path, &state)
}

fn explicit_lab2_state_dir(flags: &FlagArgs) -> CliOutcome<Option<PathBuf>> {
    if let Some(path) = flags.optional_path("--state-dir") {
        return Ok(Some(path));
    }
    if let Ok(path) = env::var(SESSION_STATE_ENV) {
        return Ok(Some(PathBuf::from(path)));
    }
    Ok(None)
}

fn ensure_lab2_write_admitted(
    flags: &FlagArgs,
    req_id: &str,
    instance: &str,
    decision: &ArbitrationDecision,
) -> CliOutcome<LeaseGrant> {
    match decision {
        ArbitrationDecision::LeaseGranted { lease, .. } => Ok(lease.clone()),
        other => {
            let mut details = lab2_error_payload_for_flags(
                flags,
                req_id,
                match other {
                    ArbitrationDecision::Queued { .. } => "lease_held",
                    ArbitrationDecision::PreemptRequested { .. } => "lease_held",
                    ArbitrationDecision::Rejected { error, .. } => error,
                    ArbitrationDecision::DeviceDenied { error, .. } => error,
                    _ => "fatal",
                },
                other.as_str(),
                "wait-for-current-holder-or-release-lab2-arbitrator-state",
                None,
            )?;
            details["arbitration"] = arbitration_json(other);
            Err(CliError::safety_blocked(
                "lab2_write_not_admitted",
                format!(
                    "Lab-2 write request for {instance} was not admitted: {}",
                    other.as_str()
                ),
                &["lab2_arbitrator", "lease"],
            )
            .with_details(details))
        }
    }
}

fn authorize_lab2_device_drive(
    global: &GlobalOptions,
    flags: &FlagArgs,
    req_id: &str,
    instance: &str,
    lease: &LeaseGrant,
) -> CliOutcome<Value> {
    let session_gate = lab2_session_lease_gate(global, flags)?;
    if !session_gate
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let details = lab2_error_payload_for_flags(
            flags,
            req_id,
            session_gate
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("lease_held"),
            "blocked",
            "acquire-session-lease-and-rerun-with---lease-holder---lease-id",
            None,
        )?;
        return Err(CliError::safety_blocked(
            "lab_session_lease_required",
            "Lab-2 real device drive requires a matching SessionLease",
            &["session_lease", "lab2_arbitrator"],
        )
        .with_details(details));
    }

    let (_, arbitrator) = match load_lab2_arbitrator(flags) {
        Ok(loaded) => loaded,
        Err(err) => {
            let details = lab2_error_details_for_flags(
                flags,
                req_id,
                "fatal",
                "arbitrator_state_unavailable",
                "inspect-or-remove-corrupt-lab2-arbitrator-state",
            )?;
            return Err(err.with_details(details));
        }
    };
    let outcome = arbitrator.authorize_device_drive(instance, req_id.to_string(), &lease.lease_id);
    match outcome.decision {
        ArbitrationDecision::ReadonlyAccepted { .. } => Ok(json!({
            "session_lease": session_gate,
            "arbitrator": arbitration_json(&outcome.decision)
        })),
        decision => {
            let details = lab2_error_payload_for_flags(
                flags,
                req_id,
                "lease_held",
                decision.as_str(),
                "acquire-matching-lab2-arbitrator-lease-before-device-io",
                None,
            )?;
            Err(CliError::safety_blocked(
                "lab2_device_drive_not_authorized",
                "Lab-2 device drive was denied by the persistent arbitrator",
                &["lab2_arbitrator", "lease"],
            )
            .with_details(details))
        }
    }
}

fn lab2_session_lease_gate(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let state_dir = session_state_dir_from_flags(flags)?;
    let mut scoped_global = global.clone();
    if let Some(instance) = flags.optional("--instance").filter(|value| value != "true") {
        scoped_global.instance = Some(instance);
    }
    session_command_check_lease_gate(&state_dir, &scoped_global, flags, true)
}

fn finish_lab2_response(
    global: &GlobalOptions,
    flags: &FlagArgs,
    req_id: &str,
    instance: &str,
    payload: Value,
    mut records: Vec<LedgerRecord>,
) -> CliOutcome<Value> {
    let mut payload = payload;
    payload["ledger"] = write_lab2_ledger(global, instance, req_id, &payload, &mut records)?;
    project_lab2_payload(&payload, flags)
}

fn write_lab2_ledger(
    global: &GlobalOptions,
    instance: &str,
    req_id: &str,
    payload: &Value,
    records: &mut Vec<LedgerRecord>,
) -> CliOutcome<Value> {
    let config = read_user_config()?;
    let Some(run_root) = effective_run_root(global, &config) else {
        return Ok(json!({"written": false, "reason": "run_root_not_configured"}));
    };
    let game = global.game.clone().unwrap_or_else(|| "unknown".to_string());
    let server = global
        .server
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let mut ledger = LabLedger::create(
        &run_root,
        &format!("lab2-{req_id}"),
        SessionHeader::new(RUNTIME_VERSION, game, server, instance),
    )
    .map_err(|err| CliError::device(err.to_string()))?;
    records.extend(lab2_drive_records_from_payload(req_id, payload));
    for record in records.drain(..) {
        let record = with_lab2_id_chain(record, req_id, &[payload]);
        ledger
            .append(record)
            .map_err(|err| CliError::device(err.to_string()))?;
    }
    let receipt = with_lab2_id_chain(
        LedgerRecord::new(
            LedgerRecordKind::Receipt,
            Some(req_id.to_string()),
            payload.clone(),
        ),
        req_id,
        &[payload],
    );
    ledger
        .append(receipt)
        .map_err(|err| CliError::device(err.to_string()))?;
    let retention = enforce_retention(
        run_root.join("sessions"),
        LAB2_LEDGER_MAX_BYTES,
        Duration::from_secs(LAB2_LEDGER_PROTECTED_DAYS * 24 * 60 * 60),
    )
    .map_err(|err| CliError::device(err.to_string()))?;
    Ok(json!({
        "written": true,
        "path": ledger.ledger_path().display().to_string(),
        "retention": retention
    }))
}

fn return_lab2_error_with_ledger(
    global: &GlobalOptions,
    _flags: &FlagArgs,
    req_id: &str,
    instance: &str,
    details: Value,
    error: CliError,
    mut records: Vec<LedgerRecord>,
) -> CliOutcome<Value> {
    let mut payload = json!({
        "req_id": req_id,
        "instance": instance,
        "error": details.get("error").cloned().unwrap_or_else(|| json!(error.code)),
        "state": details.get("state").cloned().unwrap_or_else(|| json!("blocked")),
        "hint": details.get("hint").cloned().unwrap_or_else(|| json!("inspect-ledger")),
        "blocked_error": {
            "code": error.code.clone(),
            "message": error.message.clone(),
            "blocked_by": error.blocked_by.clone()
        },
        "details": details
    });
    payload["ledger"] = write_lab2_ledger(global, instance, req_id, &payload, &mut records)?;
    Err(error.with_details(payload))
}

fn cli_error_details_or_projection(
    error: &CliError,
    req_id: &str,
    default_error: &str,
    default_state: &str,
    default_hint: &str,
) -> Value {
    error
        .details
        .as_deref()
        .cloned()
        .unwrap_or_else(|| error_projection(req_id, default_error, default_state, default_hint))
}

fn lab2_drive_records_from_payload(req_id: &str, payload: &Value) -> Vec<LedgerRecord> {
    let mut records = Vec::new();
    if let Some(guard) = payload.get("guard_result") {
        records.push(with_lab2_id_chain(
            LedgerRecord::new(
                LedgerRecordKind::Drive,
                Some(req_id.to_string()),
                json!({
                    "stage": "recognition",
                    "target": payload.get("target").cloned().unwrap_or(Value::Null),
                    "guard_result": guard
                }),
            ),
            req_id,
            &[payload],
        ));
    }
    if let Some(click) = payload.get("actual_click") {
        records.push(with_lab2_id_chain(
            LedgerRecord::new(
                LedgerRecordKind::Drive,
                Some(req_id.to_string()),
                json!({
                    "stage": "action",
                    "executed": payload.get("executed").cloned().unwrap_or(Value::Null),
                    "actual_click": click,
                    "device": payload.get("device").cloned().unwrap_or(Value::Null)
                }),
            ),
            req_id,
            &[payload],
        ));
    }
    if payload.get("wf_id").is_some() {
        records.push(with_lab2_id_chain(
            LedgerRecord::new(
                LedgerRecordKind::Drive,
                Some(req_id.to_string()),
                json!({
                    "stage": "wait",
                    "state": payload.get("state").cloned().unwrap_or(Value::Null),
                    "page": payload.get("page").cloned().unwrap_or(Value::Null),
                    "target": payload.get("target").cloned().unwrap_or(Value::Null)
                }),
            ),
            req_id,
            &[payload],
        ));
    }
    records
}

fn with_lab2_id_chain(
    mut record: LedgerRecord,
    req_id: &str,
    extra_values: &[&Value],
) -> LedgerRecord {
    let mut values = Vec::with_capacity(extra_values.len() + 1);
    values.push(&record.payload);
    values.extend_from_slice(extra_values);
    for (key, value) in lab2_id_chain(req_id, &values) {
        record = record.with_id(key, value);
    }
    record
}

fn lab2_id_chain(req_id: &str, values: &[&Value]) -> Vec<(&'static str, String)> {
    let mut chain = Vec::new();
    chain.push(("req_id", req_id.to_string()));
    for (key, paths) in [
        (
            "lease_id",
            &[
                "/lease_id",
                "/lease/lease_id",
                "/details/lease/lease_id",
                "/arbitration/details/lease/lease_id",
            ][..],
        ),
        ("reco_id", &["/reco_id", "/guard_result/reco_id"][..]),
        ("action_id", &["/action_id"][..]),
        ("wf_id", &["/wf_id"][..]),
    ] {
        if let Some(value) = first_string_at_paths(values, paths) {
            chain.push((key, value));
        }
    }
    chain
}

fn first_string_at_paths(values: &[&Value], paths: &[&str]) -> Option<String> {
    values.iter().find_map(|value| {
        paths.iter().find_map(|path| {
            value
                .pointer(path)
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
    })
}

fn read_receipt_chain(run_root: &Path, req_id: &str) -> CliOutcome<Vec<Value>> {
    let sessions = run_root.join("sessions");
    let mut records = Vec::new();
    for path in collect_ledger_paths(&sessions)? {
        let read = LabLedger::read(&path)
            .map_err(|err| CliError::device(format!("failed to read {}: {err}", path.display())))?;
        for record in read.records {
            if record_matches_req(&record, req_id) {
                records.push(json!({
                    "ledger_path": path.display().to_string(),
                    "kind": record.kind.as_str(),
                    "record": record
                }));
            }
        }
    }
    Ok(records)
}

fn collect_ledger_paths(root: &Path) -> CliOutcome<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_ledger_paths_inner(root, &mut paths)?;
    Ok(paths)
}

fn collect_ledger_paths_inner(root: &Path, paths: &mut Vec<PathBuf>) -> CliOutcome<()> {
    for entry in fs::read_dir(root)
        .map_err(|err| CliError::device(format!("failed to read {}: {err}", root.display())))?
    {
        let entry = entry.map_err(|err| CliError::device(err.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_ledger_paths_inner(&path, paths)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("ledger.jsonl") {
            paths.push(path);
        }
    }
    Ok(())
}

fn record_matches_req(record: &LedgerRecord, req_id: &str) -> bool {
    record.req_id.as_deref() == Some(req_id)
        || record
            .id_chain
            .get("req_id")
            .is_some_and(|value| value == req_id)
}

fn distinct_ledger_count(records: &[Value]) -> usize {
    records
        .iter()
        .filter_map(|record| record.get("ledger_path").and_then(Value::as_str))
        .collect::<BTreeSet<_>>()
        .len()
}

fn project_lab2_payload(payload: &Value, flags: &FlagArgs) -> CliOutcome<Value> {
    let request = lab2_projection_request(
        flags,
        payload
            .get("req_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    );
    project_record(payload, &request).map_err(|err| CliError::device(err.to_string()))
}

fn lab2_projection_request(flags: &FlagArgs, evidence_id: Option<String>) -> ProjectionRequest {
    ProjectionRequest {
        verbosity: if flags.bool("--pretty") {
            ProjectionVerbosity::Debug
        } else if flags.bool("--verbose") {
            ProjectionVerbosity::Normal
        } else {
            ProjectionVerbosity::Min
        },
        fields: fields_from_flags(flags),
        evidence_id,
    }
}

fn parse_bool_flag_value(flag: &str, value: &str) -> CliOutcome<bool> {
    match value {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(CliError::usage(format!(
            "{flag} expects true or false, got {other}"
        ))),
    }
}

fn lab2_error_payload_for_flags(
    flags: &FlagArgs,
    req_id: &str,
    error: &str,
    state: impl Into<String>,
    hint: &str,
    suspicion: Option<Value>,
) -> CliOutcome<Value> {
    let mut payload = error_projection(req_id, error, state, hint);
    if let Some(suspicion) = suspicion {
        payload["suspicion"] = suspicion;
    }
    project_record(
        &payload,
        &lab2_projection_request(flags, Some(req_id.to_string())),
    )
    .map_err(|err| CliError::device(err.to_string()))
}

fn lab2_error_details_for_flags(
    flags: &FlagArgs,
    req_id: &str,
    error: &str,
    state: &str,
    hint: &str,
) -> CliOutcome<Value> {
    lab2_error_payload_for_flags(flags, req_id, error, state, hint, None)
}

fn wait_for_lab2_recovery_clear(
    flags: &FlagArgs,
    req_id: &str,
    instance: &str,
    verb: &str,
    planned_action: Value,
) -> CliOutcome<Option<Value>> {
    let Some(recovery) = active_lab2_recovery_state(flags, req_id, instance)? else {
        return Ok(None);
    };
    if flags.bool("--no-wait") {
        return Err(recovery_in_progress_error(
            req_id,
            verb,
            recovery,
            planned_action,
        ));
    }
    let timeout = parse_optional_duration_ms(flags, "--recovery-timeout-ms", 5_000)?;
    let poll = parse_optional_duration_ms(flags, "--recovery-poll-ms", 200)?;
    if poll.is_zero() || poll > Duration::from_millis(5_000) {
        return Err(CliError::usage(
            "--recovery-poll-ms must be between 1 and 5000",
        ));
    }
    let started = Instant::now();
    loop {
        if active_lab2_recovery_state(flags, req_id, instance)?.is_none() {
            return Ok(Some(json!({
                "waited": true,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "timeout_ms": timeout.as_millis() as u64
            })));
        }
        if started.elapsed() >= timeout {
            return Err(recovery_in_progress_error(
                req_id,
                verb,
                recovery,
                planned_action,
            ));
        }
        thread::sleep(poll.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn active_lab2_recovery_state(
    flags: &FlagArgs,
    req_id: &str,
    instance: &str,
) -> CliOutcome<Option<Value>> {
    let state_dir = session_state_dir_from_flags(flags)?;
    let path = state_dir.join(LAB2_RECOVERY_STATE_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CliError::device(format!(
                "failed to read Lab-2 recovery state {}: {err}",
                path.display()
            )));
        }
    };
    let mut value = serde_json::from_str::<Value>(&text).map_err(|err| {
        CliError::device(format!(
            "failed to parse Lab-2 recovery state {}: {err}",
            path.display()
        ))
    })?;
    if !lab2_recovery_state_is_active(&value) {
        return Ok(None);
    }
    value["state_dir"] = json!(state_dir.display().to_string());
    value["state_file"] = json!(path.display().to_string());
    value["event"] = lab2_recovery_light_event(req_id, instance, &value)?;
    Ok(Some(value))
}

fn lab2_recovery_state_is_active(value: &Value) -> bool {
    value
        .get("active")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            matches!(
                value.get("state").and_then(Value::as_str),
                Some("recovering" | "running" | "in_progress")
            )
        })
}

fn lab2_recovery_light_event(req_id: &str, instance: &str, recovery: &Value) -> CliOutcome<Value> {
    let mut ids = BTreeMap::new();
    ids.insert("req_id".to_string(), req_id.to_string());
    ids.insert("instance".to_string(), instance.to_string());
    LightEvent::new(
        "recovery.state.changed",
        ids,
        json!({
            "state": recovery.get("state").cloned().unwrap_or_else(|| json!("recovering")),
            "reason": recovery.get("reason").cloned().unwrap_or(Value::Null),
            "progress": recovery.get("progress").cloned().unwrap_or(Value::Null)
        }),
    )
    .map(|event| json!(event))
    .map_err(|err| CliError::device(err.to_string()))
}

fn recovery_in_progress_error(
    req_id: &str,
    verb: &str,
    recovery: Value,
    planned_action: Value,
) -> CliError {
    let mut details = error_projection(
        req_id,
        "recovering",
        "recovering",
        "wait-for-recovery-or-rerun-with---no-wait-to-fail-fast",
    );
    details["recovery"] = recovery;
    details["planned_action"] = planned_action;
    CliError::safety_blocked(
        "recovery_in_progress",
        format!("{verb} is deferred because recovery is in progress"),
        &["recovery", "lab2"],
    )
    .with_details(details)
}

fn lab2_command_contracts() -> Vec<Lab2CommandContract> {
    vec![
        Lab2CommandContract {
            name: "observe",
            summary: "read one frame, detect page, evaluate visible targets, and list navigation actions",
            required: &["--scene <png> or --capture"],
            optional: &[
                "--targets <id,id>",
                "--with-frame <path>",
                "--require-fresh",
                "--fields <field,field>",
                "--verbose",
                "--pretty",
            ],
            output_fields: &[
                "req_id",
                "state",
                "page",
                "targets",
                "actions",
                "frame_age_ms",
                "backend",
            ],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "do",
            summary: "guard a semantic target, derive the click point, optionally execute it, and observe once",
            required: &["<target>", "--scene <png> or --capture"],
            optional: &[
                "--dry-run",
                "--allow-destructive",
                "--destructive",
                "--priority <normal|high>",
                "--fields <field,field>",
                "--no-wait",
                "--recovery-timeout-ms <ms>",
                "--recovery-poll-ms <ms>",
            ],
            output_fields: &[
                "req_id",
                "reco_id",
                "action_id",
                "actual_click",
                "guard_result",
                "observation",
                "ledger",
            ],
            requires_lease: true,
        },
        Lab2CommandContract {
            name: "ensure",
            summary: "route from the current page to a requested page through guarded actions",
            required: &[
                "<page> or --page <page> or --to <page>",
                "--scene <png> or --capture",
            ],
            optional: &[
                "--dry-run",
                "--allow-destructive",
                "--step-timeout-ms <ms>",
                "--poll-ms <ms>",
                "--no-wait",
                "--recovery-timeout-ms <ms>",
                "--recovery-poll-ms <ms>",
            ],
            output_fields: &["req_id", "state", "page", "to", "route", "steps", "ledger"],
            requires_lease: true,
        },
        Lab2CommandContract {
            name: "wait",
            summary: "wait for a page or target stability using the shared ROI stability comparator",
            required: &[
                "--page <page> or --stable <target>",
                "--scene <png> or --capture",
            ],
            optional: &[
                "--timeout-ms <ms>",
                "--poll-ms <ms>",
                "--fields <field,field>",
            ],
            output_fields: &[
                "req_id",
                "wf_id",
                "state",
                "page",
                "target",
                "elapsed_ms",
                "ledger",
            ],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "lab receipt",
            summary: "load all ledger records tied to a req_id",
            required: &["--req <req_id>", "--run-root <path> or config run_root"],
            optional: &[],
            output_fields: &["req_id", "ledger_count", "record_count", "records"],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "lab evidence",
            summary: "list debug evidence refs tied to an evidence id",
            required: &["--id <evidence_id>", "--run-root <path> or config run_root"],
            optional: &[],
            output_fields: &["evidence_id", "count", "evidence"],
            requires_lease: false,
        },
        Lab2CommandContract {
            name: "lab arbitrator",
            summary: "inspect or maintain the persistent degraded arbitrator state",
            required: &["status|release|cancel|reclaim-dead|mark-destructive"],
            optional: &[
                "--state-dir <path>",
                "--instance <name>",
                "--lease-id <id>",
                "--req <id>",
            ],
            output_fields: &["state", "instance", "arbitration", "ledger"],
            requires_lease: false,
        },
    ]
}

fn command_contract_summary(contract: Lab2CommandContract) -> Value {
    json!({
        "command": contract.name,
        "summary": contract.summary,
        "requires_lease": contract.requires_lease,
        "required": contract.required,
        "optional": contract.optional,
        "output_fields": contract.output_fields
    })
}

fn command_contract_schema(contract: Lab2CommandContract) -> Value {
    json!({
        "schema_version": "actingcommand.lab2.command_shape.v0.1",
        "command": contract.name,
        "summary": contract.summary,
        "stdout": {
            "format": "single_line_json",
            "ansi": false,
            "image_bytes": false
        },
        "requires_lease": contract.requires_lease,
        "parameters": {
            "required": contract.required,
            "optional": contract.optional
        },
        "output": {
            "shape": "json_object",
            "fields": contract.output_fields
        }
    })
}

fn recognition_engine_capabilities() -> Value {
    let metrics = [
        MatchMetric::CrossCorrelationNormalized,
        MatchMetric::CorrelationCoefficientNormalized,
    ]
    .into_iter()
    .map(match_metric_name)
    .collect::<Vec<_>>();
    json!({
        "template_matching": {
            "supported_metrics": metrics,
            "families": [
                {"id": "ncc", "implemented_by": "ccorr_normed score normalization"},
                {"id": "ccoeff", "implemented_by": "ccoeff_normed"}
            ],
            "unsupported": [
                {"id": "masked_template_match", "reason": "not implemented in current recognition engine"},
                {"id": "count_match", "reason": "resource-level count semantics are not implemented in Lab-2 CLI"}
            ]
        },
        "color": {"rgb_mean_distance": true},
        "ocr": {"default_screen_state": false, "explicit_command_required": true}
    })
}

fn lab2_config_instances(config: &UserConfig) -> Vec<Value> {
    config
        .instances
        .iter()
        .map(|(id, instance)| {
            json!({
                "id": id,
                "game": instance.game,
                "server": instance.server,
                "serial_configured": instance.serial.is_some(),
                "package_configured": instance.package.is_some(),
                "capture_backend": instance.capture_backend,
                "touch_backend": instance.touch_backend
            })
        })
        .collect()
}

fn lab2_error_code_table() -> Value {
    json!([
        {"code": "transient", "meaning": "retryable issue; retry may be appropriate if bounded and logged"},
        {"code": "recovering", "meaning": "runtime recovery is active; write actions should wait or fail visibly"},
        {"code": "resource_drift", "meaning": "resource or page expectation drift; stop and inspect rather than retry blindly"},
        {"code": "lease_held", "meaning": "another holder owns the write lease"},
        {"code": "queue_full", "meaning": "single-slot degraded queue is full"},
        {"code": "fatal", "meaning": "non-recoverable fault; fail loud"},
        {"code": "target_not_visible", "meaning": "guard target did not pass recognition"},
        {"code": "current_page_unknown", "meaning": "current page was not matched before routing"}
    ])
}

fn escape_toolbox_groups() -> Value {
    json!([
        {
            "group": "evidence",
            "commands": ["capture --with-frame", "detect-page --all", "locate --template", "read-text --region", "session journal"]
        },
        {
            "group": "recovery",
            "commands": ["session recover", "session self-heal-plan", "session capture-policy", "session instance app restart"]
        },
        {
            "group": "lease_and_arbitration",
            "commands": ["lab lease", "lab preempt", "lab release", "session request status"]
        }
    ])
}

fn load_lab2_scene(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Lab2Scene> {
    if let Some(scene_path) = flags.optional_path("--scene") {
        let test_delay = flags
            .optional("--test-capture-delay-ms")
            .filter(|value| value != "true")
            .map(|value| {
                value
                    .parse::<u64>()
                    .map(Duration::from_millis)
                    .map_err(|err| {
                        CliError::usage(format!("invalid --test-capture-delay-ms '{value}': {err}"))
                    })
            })
            .transpose()?;
        if let Some(delay) = test_delay {
            thread::sleep(delay);
        }
        let png = fs::read(&scene_path).map_err(|err| {
            CliError::device(format!("failed to read {}: {err}", scene_path.display()))
        })?;
        let scene = Scene::from_png(&png).map_err(|err| CliError::device(err.to_string()))?;
        return Ok(Lab2Scene {
            scene,
            backend: if test_delay.is_some() {
                "test_stub_capture".to_string()
            } else {
                "scene_file".to_string()
            },
            source: json!({
                "kind": if test_delay.is_some() { "test_stub_capture" } else { "scene" },
                "path": scene_path.display().to_string(),
                "delay_ms": test_delay.map(|delay| delay.as_millis() as u64)
            }),
            frame_age_ms: 0,
            png: Some(png),
        });
    }
    if flags.bool("--capture") {
        let config = read_user_config()?;
        let device_config = device_config(global, &config)?;
        let requested = device_config.capture_backend;
        let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
        let captured = capture_for_command(
            &device_config,
            requested,
            flags.bool("--require-fresh"),
            fresh_delay,
        )?;
        let frame = captured.frame;
        let frame_age_ms = system_time_age_ms(frame.captured_at);
        let backend = frame.backend_name.as_str().to_string();
        let png = frame.original_png.clone();
        let scene = scene_from_frame(&frame)?;
        return Ok(Lab2Scene {
            scene,
            backend,
            source: json!({
                "kind": "capture",
                "capture_backend_used": frame.backend_name.as_str(),
                "capture_backend_attempts": captured.attempts,
                "freshness": captured.freshness
            }),
            frame_age_ms,
            png,
        });
    }
    Err(CliError::usage(
        "Lab-2 CLI verbs require --scene <png> or --capture",
    ))
}

fn write_frame_if_requested(flags: &FlagArgs, scene: &Lab2Scene) -> CliOutcome<Option<PathBuf>> {
    let Some(path) = flags.optional_path("--with-frame") else {
        return Ok(None);
    };
    let png = scene.png.as_ref().ok_or_else(|| {
        CliError::device(
            "--with-frame requires a PNG-backed frame; selected capture backend returned raw pixels",
        )
    })?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::device(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    fs::write(&path, png)
        .map_err(|err| CliError::device(format!("failed to write {}: {err}", path.display())))?;
    Ok(Some(path))
}

fn observe_targets(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    flags: &FlagArgs,
    outcome: &PageDetectionOutcome,
) -> CliOutcome<Vec<Value>> {
    let requested = target_list(flags);
    if !requested.is_empty() {
        return requested
            .into_iter()
            .map(|target| {
                guard_evaluable_target(evaluator, &target, "observe --targets")?;
                let evaluation = evaluator
                    .evaluate_target(scene, &target)
                    .map_err(|err| CliError::usage(err.to_string()))?;
                Ok(json!({
                    "id": target,
                    "passed": evaluation.passed,
                    "score": target_score(&evaluation),
                    "evaluation": target_eval_json(&evaluation)
                }))
            })
            .collect();
    }
    let mut targets = Vec::new();
    if let Some(page) = outcome
        .evaluations
        .iter()
        .find(|evaluation| evaluation.page_id == outcome.page)
    {
        for target in &page.target_results {
            targets.push(json!({
                "id": target.target_id,
                "passed": target.passed,
                "score": Value::Null,
                "message": target.message
            }));
        }
    }
    Ok(targets)
}

fn observe_actions(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    outcome: &PageDetectionOutcome,
) -> CliOutcome<Vec<Value>> {
    let graph = load_navigation_graph(global, config, flags)?;
    Ok(graph
        .edges
        .iter()
        .filter(|edge| outcome.matched && edge.from_page == outcome.page)
        .map(navigation_edge_json)
        .collect::<Vec<_>>())
}

fn lab2_page_candidates(outcome: &PageDetectionOutcome) -> Vec<Value> {
    let mut candidates = outcome
        .evaluations
        .iter()
        .map(|evaluation| {
            let passed = evaluation.required_passed
                + evaluation.any_of_passed
                + evaluation.optional_passed
                + evaluation.forbidden_passed;
            let total = evaluation.required_total
                + evaluation.any_of_total
                + evaluation.optional_total
                + evaluation.forbidden_total;
            json!({
                "id": evaluation.page_id,
                "matched": evaluation.matched,
                "passed": evaluation.matched,
                "score": if total == 0 { 0.0 } else { passed as f64 / total as f64 },
                "required_passed": evaluation.required_passed,
                "required_total": evaluation.required_total,
                "forbidden_passed": evaluation.forbidden_passed,
                "message": evaluation.message
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .partial_cmp(&left.get("score").and_then(Value::as_f64).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(4);
    candidates
}

fn lab2_observation_suspicion(outcome: &PageDetectionOutcome, frame_age_ms: u64) -> Option<Value> {
    let candidates = lab2_page_candidates(outcome);
    if !outcome.matched {
        return Some(low_margin_suspicion(candidates));
    }
    if candidates.len() >= 2 {
        let top = candidates[0]
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let runner_up = candidates[1]
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if (top - runner_up).abs() < 0.05 {
            return Some(low_margin_suspicion(candidates));
        }
    }
    if frame_age_ms > 1_000 {
        return Some(stale_frame_suspicion(frame_age_ms));
    }
    None
}

fn wait_for_page(
    global: &GlobalOptions,
    flags: &FlagArgs,
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    ids: WaitIds<'_>,
    page: &str,
    timing: WaitTiming,
) -> CliOutcome<Value> {
    let started = Instant::now();
    loop {
        let scene = load_lab2_scene(global, flags)?;
        let outcome = detect_current_page(evaluator, detector, &scene.scene)?;
        if outcome.matched && outcome.page == page {
            return Ok(json!({
                "req_id": ids.req_id,
                "wf_id": ids.wf_id,
                "state": "arrived",
                "page": outcome.page,
                "matched": true,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "frame_age_ms": scene.frame_age_ms,
                "backend": scene.backend
            }));
        }
        if started.elapsed() >= timing.timeout {
            let details = lab2_error_payload_for_flags(
                flags,
                ids.req_id,
                "transient",
                outcome.page,
                "retry-or-observe-current-page",
                None,
            )?;
            return Err(CliError::safety_blocked(
                "wait_timeout",
                format!("wait timed out before page '{page}' became current"),
                &["page_wait"],
            )
            .with_details(details));
        }
        thread::sleep(timing.poll);
    }
}

fn wait_for_stable_target(
    global: &GlobalOptions,
    flags: &FlagArgs,
    evaluator: &RecognitionEvaluator,
    ids: WaitIds<'_>,
    target: &str,
    timing: WaitTiming,
) -> CliOutcome<Value> {
    guard_evaluable_target(evaluator, target, "wait --stable")?;
    let started = Instant::now();
    let first = load_lab2_scene(global, flags)?;
    let mut previous = evaluator
        .evaluate_target(&first.scene, target)
        .map_err(|err| CliError::usage(err.to_string()))?;
    if !previous.passed {
        let details = lab2_error_payload_for_flags(
            flags,
            ids.req_id,
            "resource_drift",
            "unstable",
            "observe-current-page-and-refresh-stable-target",
            Some(guard_reject_suspicion(target, &previous.message)),
        )?;
        return Err(CliError::safety_blocked(
            "stable_target_not_visible",
            format!("stable target '{target}' did not pass baseline guard"),
            &["stable_target"],
        )
        .with_details(details));
    }
    loop {
        let next_scene = load_lab2_scene(global, flags)?;
        let current = evaluator
            .evaluate_target(&next_scene.scene, target)
            .map_err(|err| CliError::usage(err.to_string()))?;
        if lab_run::target_evaluations_stable_for_wait(&previous, &current) {
            return Ok(json!({
                "req_id": ids.req_id,
                "wf_id": ids.wf_id,
                "state": "stable",
                "target": target,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "frame_age_ms": next_scene.frame_age_ms,
                "backend": next_scene.backend,
                "evaluation": target_eval_json(&current)
            }));
        }
        if started.elapsed() >= timing.timeout {
            let details = lab2_error_details_for_flags(
                flags,
                ids.req_id,
                "transient",
                "unstable",
                "retry-or-switch-backend",
            )?;
            return Err(CliError::safety_blocked(
                "wait_timeout",
                format!("wait timed out before target '{target}' became stable"),
                &["stable_target"],
            )
            .with_details(details));
        }
        previous = current;
        thread::sleep(timing.poll);
    }
}

fn derive_lab2_click_rect(
    evaluator: &RecognitionEvaluator,
    target: &str,
    declared: PackRect,
    evaluation: &TargetEvaluation,
) -> CliOutcome<Lab2ClickRect> {
    let Some(anchor) = evaluator
        .get_template_anchor_rect(target)
        .map_err(|err| CliError::usage(err.to_string()))?
    else {
        return Ok(Lab2ClickRect {
            kind: "target_rect_center",
            rect: declared,
            derivation: json!({"mode": "declared_rect"}),
        });
    };
    let Some(template) = evaluation.template else {
        return Ok(Lab2ClickRect {
            kind: "target_rect_center",
            rect: declared,
            derivation: json!({"mode": "declared_rect", "reason": "non_template_evaluation"}),
        });
    };
    let matched = PackRect {
        x: template.x,
        y: template.y,
        width: template.width,
        height: template.height,
    };
    let rect = derive_absolute_coordinate_rect_from_match("lab2_do", declared, anchor, matched)?;
    Ok(Lab2ClickRect {
        kind: "target_rect_center_live_match",
        rect,
        derivation: json!({
            "mode": "matched_template_delta",
            "expected_rect": rect_json(anchor),
            "matched_rect": rect_json(matched)
        }),
    })
}

fn reject_lab2_destructive_click_overlap(
    target: &str,
    page: &str,
    click: PackRect,
    graph: &NavigationGraph,
) -> CliOutcome<()> {
    if graph.destructive_clicks.iter().any(|destructive| {
        destructive
            .page
            .as_deref()
            .is_none_or(|expected| expected == page)
            && rects_intersect(click, destructive.rect)
    }) {
        return Err(CliError::safety_blocked(
            "semantic_action_requires_destructive_opt_in",
            format!(
                "target '{target}' click rect overlaps a destructive_actions region and requires --allow-destructive"
            ),
            &["destructive_actions", "allow_destructive"],
        ));
    }
    Ok(())
}

fn target_list(flags: &FlagArgs) -> Vec<String> {
    flags
        .optional("--targets")
        .filter(|value| value != "true")
        .into_iter()
        .flat_map(|value| split_csv(&value))
        .collect()
}

fn fields_from_flags(flags: &FlagArgs) -> BTreeSet<String> {
    flags
        .optional("--fields")
        .filter(|value| value != "true")
        .into_iter()
        .flat_map(|value| split_csv(&value))
        .collect()
}

fn wait_page_target(flags: &FlagArgs) -> Option<String> {
    flags
        .optional("--page")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned())
}

fn ensure_target_page(flags: &FlagArgs) -> CliOutcome<String> {
    flags
        .optional("--to")
        .filter(|value| value != "true")
        .or_else(|| flags.optional("--page").filter(|value| value != "true"))
        .or_else(|| flags.positionals.first().cloned())
        .ok_or_else(|| CliError::usage("ensure requires <page>, --page <page>, or --to <page>"))
}

fn guard_evaluable_target(
    evaluator: &RecognitionEvaluator,
    target: &str,
    command: &str,
) -> CliOutcome<()> {
    if evaluator
        .target_kind(target)
        .map_err(|err| CliError::usage(err.to_string()))?
        == TargetKind::ClickOnly
    {
        return Err(CliError::usage(format!(
            "{command} requires a visually evaluatable target; '{target}' is click-only"
        )));
    }
    Ok(())
}

fn target_score(evaluation: &TargetEvaluation) -> Value {
    if let Some(template) = evaluation.template {
        json!(template.score)
    } else if let Some(color) = evaluation.color {
        json!(1.0_f32 - color.distance)
    } else {
        Value::Null
    }
}

fn lab2_instance(global: &GlobalOptions, flags: &FlagArgs) -> String {
    flags
        .optional("--instance")
        .filter(|value| value != "true")
        .or_else(|| global.instance.clone())
        .unwrap_or_else(|| "default".to_string())
}

fn system_time_age_ms(timestamp: SystemTime) -> u64 {
    timestamp
        .elapsed()
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn arbitration_json(decision: &ArbitrationDecision) -> Value {
    json!({
        "decision": decision.as_str(),
        "details": decision
    })
}
