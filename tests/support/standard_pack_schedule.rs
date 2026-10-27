// SPDX-License-Identifier: AGPL-3.0-only

//! ROUT-L RL-1c (#411): neutral standard-pack game root fixtures shared by actingd's and
//! actingctl's composer-mode tests: rules, a `schedule.json` and a bundle index v4, built from the
//! scheduling examples' catalog-a as runtime-host's `gate_rl1b` builds them.

use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture JSON")
}

/// Removes every `scope` member at any depth: `schedule.json` names no instance.
fn strip_scopes(value: &mut Value) {
    match value {
        Value::Object(members) => {
            members.shift_remove("scope");
            for (_, member) in members.iter_mut() {
                strip_scopes(member);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_scopes),
        _ => {}
    }
}

/// catalog-a's task as the rule `id` of `package_id`: no scope, entrypoint, procedure or instance
/// override, no yield point, no product, and a feedback stop at a far clock time. Its trigger
/// reads the clock and the realistic fact `resource.primary` in its instance's scope, or the
/// clock only when `clock_only`.
pub fn rule(id: &str, package_id: &str, clock_only: bool) -> Value {
    let tasks = parse(include_str!(
        "../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    let mut rule = tasks["tasks"][0].clone();
    strip_scopes(&mut rule);
    let fields = rule.as_object_mut().expect("a task object");
    for member in ["entrypoint", "procedure_ref", "instance_overrides"] {
        fields.shift_remove(member);
    }
    fields.insert("id".to_owned(), json!(id));
    fields.insert("package_id".to_owned(), json!(package_id));
    fields.insert("priority".to_owned(), json!(100));
    fields.insert("yield_points".to_owned(), json!([]));
    fields.insert("produces".to_owned(), json!([]));
    fields.insert(
        "feedback_stop".to_owned(),
        json!({
            "kind": "clock",
            "schedule": {
                "kind": "at",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "etc/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "at_ms": 4_102_444_800_000_u64
            }
        }),
    );
    if clock_only {
        let clock = rule["trigger"]["predicates"][0].clone();
        assert_eq!(clock["kind"], json!("clock"), "catalog-a's first predicate");
        rule["trigger"] = clock;
    }
    rule
}

/// A game's `schedule.json`: `rules`, no pool, catalog-a's profiles without goals, no event.
pub fn schedule(rules: &[Value]) -> Value {
    let activity = parse(include_str!(
        "../../contracts/scheduling/examples/catalog-a/activity.json"
    ));
    let profiles = activity["profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .map(|profile| {
            let mut profile = profile.clone();
            strip_scopes(&mut profile);
            profile["goals"] = json!([]);
            profile
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": [],
        "activity": profiles,
        "timeline": [],
        "held": []
    })
}

/// A bundle index v4 of `game` registering `packages` on `server`.
pub fn bundle(game: &str, server: &str, packages: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": game,
        "packs": packages
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": server,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>()
    })
}

/// Writes `value` at `path`, its folders first.
pub fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent folder")).expect("create the folder");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON bytes")).expect("write a file");
}
