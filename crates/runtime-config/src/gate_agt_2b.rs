// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #432 AGT-2b (`contracts/agent-wake.md`, "The daily summary"; register §1.263): a
// `daily_summary` rule names its kind alone, a minute of the day and an offset from UTC, no codes
// and a day cap of 1; any other shape is refused at start, in `check-config` and through an
// instruction, with `daily_summary_shape`. The names are made up.

use super::*;
use crate::agent_wake::*;
use actingcommand_contract::{ConfigSettingChange, RuntimeConfigManifest};
use serde_json::{Value, json};

const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

// A whole section with these rules.
fn whole(rules: Value) -> Value {
    json!({
        "enabled": true,
        "summary": {"max_chars": 100},
        "body": {"dir": "agent-wakes", "max_bytes": 65536, "retention_days": 14, "max_files": 512},
        "delivery": {
            "queue_capacity": 64, "coalesce_ms": 3000, "send_deadline_ms": 30000,
            "retry_interval_ms": 300000, "give_up_ms": 86400000, "channel_gap_ms": 5000,
            "history_rows": 32, "panic_backoff_ms": 60000, "panic_backoff_max_ms": 900000,
        },
        "quiet": {"windows": [], "hold_until_unix_ms": 0},
        "budget": {"daily_cap": 24},
        "rules": rules,
    })
}

// What a start and `check-config` make of a file with this section.
fn checked(section: Value) -> Result<EffectiveConfig, ConfigFailure> {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "agent_wake": section }),
        &|_| None,
    )?;
    annotate_settable(&mut parameters)?;
    let config = EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })?;
    config.check(&context())?;
    Ok(config)
}

fn daily() -> Value {
    json!({
        "id": "daily",
        "on": {"kinds": ["daily_summary"], "at_minute_of_day": 1260, "utc_offset_minutes": 540},
        "instances": "*", "to": "总览", "cooldown_ms": 0, "daily_cap": 1,
    })
}

fn fence() -> Value {
    json!({
        "id": "fence", "on": {"kinds": ["instance_fenced"]}, "instances": "*",
        "to": "总览", "cooldown_ms": 1800000, "daily_cap": 8,
    })
}

fn rules_in_effect(rules: Value) -> Vec<WakeRule> {
    checked(whole(rules))
        .expect("the section passes")
        .agent_wake()
        .expect("the section in effect")
        .expect("the configuration carries it")
        .rules
}

// `rule` with the member at `path` set to `value`, or removed with `None`.
fn reshaped(rule: Value, path: &str, value: Option<Value>) -> Value {
    let mut rule = rule;
    let (parent, name) = path.rsplit_once('.').unwrap_or(("", path));
    let holder = if parent.is_empty() {
        &mut rule
    } else {
        rule.pointer_mut(&format!("/{}", parent.replace('.', "/")))
            .expect("the member's parent")
    };
    let members = holder.as_object_mut().expect("an object");
    match value {
        Some(value) => {
            members.insert(name.to_owned(), value);
        }
        None => {
            members.remove(name).expect("the member");
        }
    }
    rule
}

#[test]
fn a_daily_summary_rule_of_another_shape_is_refused() {
    // The shape: the kind alone, its minute and its offset, no codes, a day cap of 1.
    let rules = rules_in_effect(json!([daily(), fence()]));
    assert_eq!(
        rules[0].summary_at,
        Some(SummaryAt {
            minute_of_day: 1260,
            utc_offset_minutes: 540,
        })
    );
    assert_eq!(
        rules[1].summary_at, None,
        "a rule of another kind has no time"
    );
    // The ends of both ranges pass.
    for (minute, offset) in [(0, -720), (1439, 840)] {
        let rule = reshaped(daily(), "on.at_minute_of_day", Some(json!(minute)));
        let rule = reshaped(rule, "on.utc_offset_minutes", Some(json!(offset)));
        let rules = rules_in_effect(json!([rule]));
        assert_eq!(
            rules[0].summary_at,
            Some(SummaryAt {
                minute_of_day: minute,
                utc_offset_minutes: offset,
            })
        );
    }

    // Every other shape is refused, naming the rules and the problem.
    let shapes = [
        reshaped(daily(), "on.at_minute_of_day", None),
        reshaped(daily(), "on.utc_offset_minutes", None),
        reshaped(daily(), "on.at_minute_of_day", Some(json!(1440))),
        reshaped(daily(), "on.at_minute_of_day", Some(json!(-1))),
        reshaped(daily(), "on.utc_offset_minutes", Some(json!(-721))),
        reshaped(daily(), "on.utc_offset_minutes", Some(json!(841))),
        reshaped(
            daily(),
            "on.codes",
            Some(json!(["capture_backend_operation_failed"])),
        ),
        reshaped(
            daily(),
            "on.kinds",
            Some(json!(["daily_summary", "instance_fenced"])),
        ),
        reshaped(daily(), "daily_cap", Some(json!(2))),
        reshaped(fence(), "on.at_minute_of_day", Some(json!(1260))),
        reshaped(fence(), "on.utc_offset_minutes", Some(json!(540))),
    ];
    for rule in &shapes {
        let refused = checked(whole(json!([rule]))).expect_err("a daily summary of another shape");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::AgentWakeConfigInvalid,
            "{rule}"
        );
        assert_eq!(refused.config_key(), RULES, "{rule}");
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::DailySummaryShape),
            "{rule}"
        );
    }

    // Through an instruction the same is `config_value_invalid` with the same key and problem,
    // and the configuration in effect stays.
    let config = checked(whole(json!([daily()]))).expect("the section passes");
    let stamp = InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    };
    for rule in &shapes {
        let refused = config
            .apply(
                &[ConfigSettingChange {
                    key: RULES.to_owned(),
                    value: Some(json!([rule])),
                }],
                &stamp,
                &context(),
            )
            .expect_err("a daily summary of another shape");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{rule}"
        );
        assert_eq!(refused.config_key(), RULES, "{rule}");
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::DailySummaryShape),
            "{rule}"
        );
    }
}
