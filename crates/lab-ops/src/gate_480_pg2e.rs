// SPDX-License-Identifier: AGPL-3.0-only

//! #480 PG-2e (0.12.75; register §1.485; `contracts/scheduling-pause.md`, "Status"): the instance
//! row of the overview, one function (`overview::instance_row`) for `actinglab status` and the
//! Lab's `ac_status`, `actingctl overview` and its MCP's `ac_overview`, passes the instance pause's
//! `pause_mode` and `in_the_way` through as the status answers them and leaves each out when the
//! status does; the operation table's `status` and `pause` rows keep their parameters. No Runtime,
//! no clock.
//!
//! - `pg2e_an_overview_row_passes_the_pauses_mode_and_what_is_in_its_way`: a standing pause in
//!   `paused` with two words, a tool's hold in `draining` with one, a `released` pause with its
//!   mode and nothing in its way, and a pause the status answers without either member (a Runtime
//!   before 0.12.75): the row's `pause` is exactly the status's members, the three it had before
//!   and each of the two the status carries.
//! - `pg2e_the_status_and_pause_rows_keep_their_parameters`: `status` takes `instance` (a
//!   positional instance) and `sections` (an option, a list of strings), `pause` `instance`,
//!   `reason` (required in MCP) and `drain` (default `300s`), nothing more.

use crate::overview::instance_row;
use crate::table::{self, ParamForm, ParamType};
use actingcommand_contract::{
    IdentifierIssuer, InstancePauseStage, InstancePauseState, PauseObstacle, RuntimeInstanceStatus,
    SchedulingPauseMode, SchedulingPauseReason,
};
use serde_json::{Value, json};

/// The status of one instance paused in `stage`, its pause answered with `pause_mode` and
/// `in_the_way`.
fn paused(
    stage: InstancePauseStage,
    pause_mode: Option<SchedulingPauseMode>,
    in_the_way: Vec<PauseObstacle>,
) -> RuntimeInstanceStatus {
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    RuntimeInstanceStatus::new("neutral.paused", instance_id, false, 0, false, false, false)
        .expect("instance status")
        .with_pause(Some(InstancePauseState {
            revision: 2,
            pause_reason: SchedulingPauseReason::new("operator").expect("pause reason"),
            since_unix_ms: 1_000,
            stage,
            pause_mode,
            in_the_way,
        }))
        .expect("a paused instance's status")
}

/// The `pause` member of the overview row of `instance`.
fn row_pause(instance: &RuntimeInstanceStatus) -> Value {
    instance_row(instance, None)["pause"].clone()
}

/// The three members the row's pause had before 0.12.75, with `extra` added.
fn expected(stage: &str, extra: &[(&str, Value)]) -> Value {
    let mut members = serde_json::Map::new();
    members.insert("revision".to_owned(), json!(2));
    members.insert("stage".to_owned(), json!(stage));
    members.insert("pause_reason".to_owned(), json!("operator"));
    for (name, value) in extra {
        members.insert((*name).to_owned(), value.clone());
    }
    Value::Object(members)
}

#[test]
fn pg2e_an_overview_row_passes_the_pauses_mode_and_what_is_in_its_way() {
    let standing = paused(
        InstancePauseStage::Paused,
        Some(SchedulingPauseMode::Stand),
        vec![PauseObstacle::Lease, PauseObstacle::QueuedEntry],
    );
    assert_eq!(
        row_pause(&standing),
        expected(
            "paused",
            &[
                ("pause_mode", json!("stand")),
                ("in_the_way", json!(["lease", "queued_entry"])),
            ]
        )
    );
    let draining = paused(
        InstancePauseStage::Draining,
        Some(SchedulingPauseMode::QuietOrLift),
        vec![PauseObstacle::ContainedRun],
    );
    assert_eq!(
        row_pause(&draining),
        expected(
            "draining",
            &[
                ("pause_mode", json!("quiet_or_lift")),
                ("in_the_way", json!(["contained_run"])),
            ]
        )
    );
    let released = paused(
        InstancePauseStage::Released,
        Some(SchedulingPauseMode::Stand),
        Vec::new(),
    );
    assert_eq!(
        row_pause(&released),
        expected("released", &[("pause_mode", json!("stand"))])
    );
    // A Runtime before 0.12.75 answers neither member: the row is what it was.
    let unanswered = paused(InstancePauseStage::Paused, None, Vec::new());
    assert_eq!(row_pause(&unanswered), expected("paused", &[]));
}

#[test]
fn pg2e_the_status_and_pause_rows_keep_their_parameters() {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    let params_of = |name: &str| {
        table
            .operation(name)
            .unwrap_or_else(|| panic!("the row {name}"))
            .params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param.form,
                    param.value_type.clone(),
                    param.required,
                    param.repeat,
                    param.default.clone(),
                    param.required_in.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        params_of("status"),
        [
            (
                "instance".to_owned(),
                ParamForm::Positional,
                ParamType::Instance,
                false,
                false,
                None,
                None
            ),
            (
                "sections".to_owned(),
                ParamForm::Option,
                ParamType::ListOfStrings,
                false,
                false,
                None,
                None
            ),
        ]
    );
    assert_eq!(
        params_of("pause"),
        [
            (
                "instance".to_owned(),
                ParamForm::Positional,
                ParamType::Instance,
                false,
                false,
                None,
                None
            ),
            (
                "reason".to_owned(),
                ParamForm::Option,
                ParamType::String,
                false,
                false,
                None,
                Some(vec!["mcp".to_owned()])
            ),
            (
                "drain".to_owned(),
                ParamForm::Option,
                ParamType::Duration,
                false,
                false,
                Some("300s".to_owned()),
                None
            ),
        ]
    );
}
