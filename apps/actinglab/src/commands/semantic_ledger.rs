use crate::{CliError, CliOutcome, GlobalOptions};
use actingcommand_contract::LedgerProjection;
use actingcommand_lab::{SemanticLedgerContext, SemanticRequestContext, project_semantic_payload};
use serde_json::{Value, json};

pub(crate) fn semantic_ledger_context(
    command: &'static str,
    global: &GlobalOptions,
    args: &[String],
) -> SemanticLedgerContext {
    SemanticLedgerContext::new(SemanticRequestContext {
        command: command.to_string(),
        instance: global
            .instance
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        arguments: args.to_vec(),
        dry_run: global.dry_run,
    })
}

pub(crate) fn finish_semantic_result_with_ledger(
    global: &GlobalOptions,
    ctx: SemanticLedgerContext,
    result: CliOutcome<Value>,
) -> CliOutcome<Value> {
    match result {
        Ok(payload) => finish_semantic_payload_with_ledger(global, ctx, payload),
        Err(error) => return_semantic_error_with_ledger(global, ctx, error),
    }
}

fn finish_semantic_payload_with_ledger(
    _global: &GlobalOptions,
    mut ctx: SemanticLedgerContext,
    mut payload: Value,
) -> CliOutcome<Value> {
    if let Some(object) = payload.as_object_mut() {
        object
            .entry("req_id")
            .or_insert_with(|| json!(ctx.req_id.clone()));
        object
            .entry("instance")
            .or_insert_with(|| json!(ctx.instance.clone()));
    }
    let records = ctx.take_records();
    payload["trace_record_count"] = json!(records.len());
    project_semantic_payload(
        payload,
        LedgerProjection::skipped("isolated_offline_projection"),
    )
}

fn return_semantic_error_with_ledger(
    _global: &GlobalOptions,
    mut ctx: SemanticLedgerContext,
    error: CliError,
) -> CliOutcome<Value> {
    let mut payload = json!({
        "req_id": ctx.req_id.clone(),
        "instance": ctx.instance.clone(),
        "command": ctx.command.clone(),
        "error": error.code.clone(),
        "state": "failed",
        "blocked_error": {
            "code": error.code.clone(),
            "message": error.message.clone(),
            "blocked_by": error.blocked_by.clone()
        },
        "details": error.details.clone().unwrap_or(Value::Null)
    });
    payload["trace_record_count"] = json!(ctx.take_records().len());
    payload = project_semantic_payload(
        payload,
        LedgerProjection::skipped("isolated_offline_projection"),
    )?;
    Err(error.with_details(payload))
}
