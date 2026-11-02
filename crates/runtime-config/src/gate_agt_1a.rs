// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #432 AGT-1a (`contracts/agent-wake.md`; register §1.245): the `agent_wake` section's
// rows have no default and the section is whole or absent; its caps fit the rows `host.agent_wake`
// keeps; its quiet windows use the blackout entries' rules; and its settable keys are the design's
// table. The names are made up.

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

// A whole section, the shipped example's values.
fn whole() -> Value {
    json!({
        "enabled": true,
        "summary": {"max_chars": 100},
        "body": {"dir": "agent-wakes", "max_bytes": 65536, "retention_days": 14, "max_files": 512},
        "delivery": {
            "queue_capacity": 64, "coalesce_ms": 3000, "send_deadline_ms": 30000,
            "retry_interval_ms": 300000, "give_up_ms": 86400000, "channel_gap_ms": 5000,
            "history_rows": 32, "panic_backoff_ms": 60000, "panic_backoff_max_ms": 900000,
        },
        "quiet": {
            "windows": [{
                "id": "restart", "weekdays": [1, 4], "utc_offset_minutes": 540,
                "start_minute_of_day": 280, "end_minute_of_day": 480,
            }],
            "hold_until_unix_ms": 0,
        },
        "budget": {"daily_cap": 24},
        "rules": [{
            "id": "fence", "on": {"kinds": ["instance_fenced"]}, "instances": "*",
            "to": "总览", "cooldown_ms": 1800000, "daily_cap": 8,
        }],
    })
}

// The rows a start renders from a file with this section, over the shipped defaults.
fn rendered(section: Value) -> Result<EffectiveConfig, ConfigFailure> {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "agent_wake": section }),
        &|_| None,
    )?;
    annotate_settable(&mut parameters)?;
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
}

fn checked(section: Value) -> Result<EffectiveConfig, ConfigFailure> {
    let config = rendered(section)?;
    config.check(&context())?;
    Ok(config)
}

// The key a member path of the section writes.
fn set(section: &mut Value, path: &str, value: Value) {
    let pointer = format!("/{}", path.replace('.', "/"));
    *section.pointer_mut(&pointer).expect("the member exists") = value;
}

fn refusal(section: Value) -> ConfigFailure {
    checked(section).expect_err("the section is refused")
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

fn change(key: &str, value: Value) -> Vec<ConfigSettingChange> {
    vec![ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }]
}

fn section_rows() -> Vec<KeyRow> {
    rows()
        .filter(|row| matches!(row.name, KeyName::Exact(key) if key.starts_with("agent_wake.")))
        .copied()
        .collect()
}

fn key_of(row: &KeyRow) -> &'static str {
    match row.name {
        KeyName::Exact(key) => key,
        KeyName::PerInstance(member) => panic!("{member} is per instance"),
    }
}

#[test]
fn agent_wake_rows_have_no_default_and_the_section_must_be_whole() {
    let rows = section_rows();
    assert!(!rows.is_empty());
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    assert_eq!(
        member(&defaults, SECTION),
        None,
        "the defaults file holds none"
    );
    for row in &rows {
        assert_eq!(row.default, DefaultSource::None, "{:?}", row.name);
        assert_eq!(row.since, "0.12.37", "{:?}", row.name);
    }

    // The whole section passes and reads back as written.
    let section = checked(whole())
        .expect("the whole section passes")
        .agent_wake()
        .expect("the section in effect")
        .expect("the configuration carries it");
    assert!(section.enabled);
    assert_eq!(section.summary_max_chars, 100);
    assert_eq!(section.body.dir, "agent-wakes");
    assert_eq!(section.delivery.history_rows, 32);
    assert_eq!(section.quiet.windows.len(), 1);
    assert_eq!(section.rules[0].to, "总览");
    assert_eq!(section.rules[0].instances, WakeInstances::All);

    // Each key left out refuses the section, naming that key.
    for row in &rows {
        let key = key_of(row);
        let mut section = whole();
        let path = key.strip_prefix("agent_wake.").expect("the section's key");
        let (parent, name) = path.rsplit_once('.').unwrap_or(("", path));
        let holder = if parent.is_empty() {
            &mut section
        } else {
            section
                .pointer_mut(&format!("/{}", parent.replace('.', "/")))
                .expect("the member's parent")
        };
        holder
            .as_object_mut()
            .expect("an object")
            .remove(name)
            .expect("the member");
        let refused = refusal(section);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::AgentWakeConfigInvalid,
            "{key}"
        );
        assert_eq!(refused.config_key(), key);
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::RequiredMissing),
            "{key}"
        );
    }
}

#[test]
fn agent_wake_settable_keys_match_the_table() {
    let not_settable = [BODY_DIR, QUEUE_CAPACITY];
    let settable = [
        ENABLED,
        SUMMARY_MAX_CHARS,
        BODY_MAX_BYTES,
        BODY_RETENTION_DAYS,
        BODY_MAX_FILES,
        COALESCE_MS,
        SEND_DEADLINE_MS,
        RETRY_INTERVAL_MS,
        GIVE_UP_MS,
        CHANNEL_GAP_MS,
        HISTORY_ROWS,
        PANIC_BACKOFF_MS,
        PANIC_BACKOFF_MAX_MS,
        QUIET_WINDOWS,
        QUIET_HOLD_UNTIL_UNIX_MS,
        BUDGET_DAILY_CAP,
        RULES,
    ];
    let rows = section_rows();
    assert_eq!(rows.len(), not_settable.len() + settable.len());
    for row in &rows {
        let key = key_of(row);
        assert_eq!(row.settable.is_settable(), settable.contains(&key), "{key}");
        assert_eq!(
            !row.settable.is_settable(),
            not_settable.contains(&key),
            "{key}"
        );
    }

    let config = checked(whole()).expect("the whole section passes");
    // The two read at assembly are refused, and the configuration stays.
    for (key, value) in [(BODY_DIR, json!("other")), (QUEUE_CAPACITY, json!(8))] {
        let refused = config
            .apply(&change(key, value), &stamp(), &context())
            .expect_err("not settable");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigKeyNotSettable,
            "{key}"
        );
        assert_eq!(refused.config_key(), key);
    }
    // A settable key takes effect, and its section stays whole.
    let applied = config
        .apply(
            &change(QUIET_HOLD_UNTIL_UNIX_MS, json!(1_793_000_000_000_u64)),
            &stamp(),
            &context(),
        )
        .expect("the hold is settable");
    let section = applied
        .config
        .agent_wake()
        .expect("the section in effect")
        .expect("the configuration carries it");
    assert_eq!(section.quiet.hold_until_unix_ms, 1_793_000_000_000);
    // A broken rule through an instruction is `config_value_invalid`, with the same problem.
    let refused = config
        .apply(
            &change(
                RULES,
                json!([{
                    "id": "a", "on": {"kinds": ["neutral_kind"]}, "instances": "*",
                    "to": "总览", "cooldown_ms": 0, "daily_cap": 1,
                }]),
            ),
            &stamp(),
            &context(),
        )
        .expect_err("a kind outside the closed list");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), RULES);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::KindUnknown));
}

#[test]
fn agent_wake_caps_must_fit_the_history_rows() {
    // Equal at every step passes: rule cap = budget = history rows.
    let mut at_the_edge = whole();
    set(&mut at_the_edge, "delivery.history_rows", json!(8));
    set(&mut at_the_edge, "budget.daily_cap", json!(8));
    checked(at_the_edge).expect("each cap at the next one's value");

    // A rule above the budget.
    let mut over = whole();
    set(&mut over, "budget.daily_cap", json!(7));
    let refused = refusal(over);
    assert_eq!(refused.failure_code(), ConfigCode::AgentWakeConfigInvalid);
    assert_eq!(refused.config_key(), RULES);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::CapsExceed));

    // The budget above the rows kept.
    let mut over = whole();
    set(&mut over, "budget.daily_cap", json!(33));
    let refused = refusal(over);
    assert_eq!(refused.failure_code(), ConfigCode::AgentWakeConfigInvalid);
    assert_eq!(refused.config_key(), BUDGET_DAILY_CAP);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::CapsExceed));

    // The rows kept above what one record list holds: the key's own range.
    let mut over = whole();
    set(&mut over, "delivery.history_rows", json!(257));
    let refused = rendered(over).expect_err("outside the key's range");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), HISTORY_ROWS);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::OutOfRange));
    let mut most = whole();
    set(&mut most, "delivery.history_rows", json!(256));
    checked(most).expect("the most one record list holds");
}

#[test]
fn agent_wake_quiet_windows_use_the_blackout_rules() {
    let weekly = |member: &str, value: Value| {
        let mut entry = json!({
            "id": "a", "weekdays": [1, 4], "utc_offset_minutes": 540,
            "start_minute_of_day": 280, "end_minute_of_day": 480,
        });
        entry[member] = value;
        entry
    };
    let section_with = |entries: Value| {
        let mut section = whole();
        set(&mut section, "quiet.windows", entries);
        section
    };
    // A weekly and a once entry, the blackout list's two kinds.
    let config = checked(section_with(json!([
        weekly("id", json!("a")),
        {"kind": "once", "id": "b", "from_unix_ms": 1, "until_unix_ms": 2},
    ])))
    .expect("both kinds pass");
    let windows = config
        .agent_wake()
        .expect("the section in effect")
        .expect("the configuration carries it")
        .quiet
        .windows;
    assert_eq!(windows.len(), 2);
    assert!(matches!(windows[1].window, BlackoutWindow::Once { .. }));
    // Each broken rule is this section's code, with the entry's member as its pointer.
    let too_many = (0..65)
        .map(|index| weekly("id", json!(format!("w{index}"))))
        .collect::<Vec<_>>();
    for (entries, pointer) in [
        (
            json!([weekly("weekdays", json!([8]))]),
            "/agent_wake/quiet/windows/0/weekdays",
        ),
        (
            json!([weekly("utc_offset_minutes", json!(900))]),
            "/agent_wake/quiet/windows/0/utc_offset_minutes",
        ),
        (
            json!([weekly("start_minute_of_day", json!(1440))]),
            "/agent_wake/quiet/windows/0/start_minute_of_day",
        ),
        (
            json!([weekly("id", json!("a")), weekly("id", json!("a"))]),
            "/agent_wake/quiet/windows/1/id",
        ),
        (
            json!([{"kind": "once", "id": "a", "from_unix_ms": 2, "until_unix_ms": 2}]),
            "/agent_wake/quiet/windows/0/until_unix_ms",
        ),
        (Value::Array(too_many), "/agent_wake/quiet/windows/64"),
    ] {
        let refused = refusal(section_with(entries));
        assert_eq!(
            refused.failure_code(),
            ConfigCode::AgentWakeConfigInvalid,
            "{pointer}"
        );
        assert_eq!(refused.config_key(), QUIET_WINDOWS, "{pointer}");
        assert_eq!(refused.pointer(), Some(pointer));
    }
}
