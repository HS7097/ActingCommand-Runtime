// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-2 (register §1.270; design `rd-1031/backend-switch/model-backend-switch.md` §2.2; the
//! coordinator's answer to its Q6): an `ac_config_set` whose changes name an instance's backend
//! key switches that device's backends, so it needs the operator tier; every other change keeps
//! the tool's own tier (observer, always on). A session without the operator tier is refused
//! `tier_not_enabled` naming `operator`, before anything reaches the Runtime. The instance id is
//! minted.

use super::super::protocol::Era;
use super::super::tools::{Tier, TierSet, call_result, call_tier, find, tier_not_enabled_at};
use actingcommand_contract::IdentifierIssuer;
use serde_json::{Map, Value, json};

fn arguments(keys: &[String]) -> Map<String, Value> {
    let changes = keys
        .iter()
        .map(|key| json!({"key": key, "value": "maatouch"}))
        .collect::<Vec<_>>();
    match json!({ "changes": changes }) {
        Value::Object(arguments) => arguments,
        other => panic!("arguments are an object: {other}"),
    }
}

#[test]
fn hs2_mcp_backend_key_needs_operator_tier() {
    let tool = find("ac_config_set").expect("ac_config_set is in the tool table");
    assert_eq!(tool.tier, Tier::Observer, "the tool itself stays observer");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let instance = *ids.mint_instance_id().expect("instance").transport();
    let instance = match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    };
    let observer = TierSet::default();
    let operator = TierSet::parse("operator").expect("the operator tier");

    // A backend key, alone or with the instance's other backend key: operator.
    for keys in [
        vec![format!("instance.{instance}.touch_backend")],
        vec![format!("instance.{instance}.capture_backend")],
        vec![
            format!("instance.{instance}.touch_backend"),
            format!("instance.{instance}.capture_backend"),
        ],
    ] {
        let required = call_tier(tool, &arguments(&keys));
        assert_eq!(required, Tier::Operator, "{keys:?}");
        assert!(!observer.contains(required), "{keys:?}");
        assert!(operator.contains(required), "{keys:?}");
    }
    // Any other change, and a call whose changes are malformed: the tool's own tier.
    for keys in [
        vec![format!("instance.{instance}.stuck_recovery")],
        vec!["scheduling.parallel.enabled".to_owned()],
    ] {
        let required = call_tier(tool, &arguments(&keys));
        assert_eq!(required, Tier::Observer, "{keys:?}");
        assert!(observer.contains(required), "{keys:?}");
    }
    let malformed = match json!({"changes": "touch_backend"}) {
        Value::Object(arguments) => arguments,
        other => panic!("arguments are an object: {other}"),
    };
    assert_eq!(call_tier(tool, &malformed), Tier::Observer);
    // Another tool never needs more than its own tier.
    let other = find("ac_facts").expect("ac_facts is in the tool table");
    assert_eq!(
        call_tier(
            other,
            &arguments(&[format!("instance.{instance}.touch_backend")])
        ),
        other.tier
    );

    // The refusal an observer session answers: tier_not_enabled, naming the operator tier and
    // the tool, blocked by the server's tiers.
    let answer = call_result(Err(tier_not_enabled_at(tool, Tier::Operator)), Era::Modern);
    assert_eq!(answer["isError"], json!(true), "{answer}");
    let outcome = &answer["structuredContent"]["error"]["outcome"];
    assert_eq!(outcome["code"], json!("tier_not_enabled"), "{answer}");
    assert_eq!(
        outcome["values"]["required_tier"],
        json!("operator"),
        "{answer}"
    );
    assert_eq!(
        outcome["values"]["mcp_tool"],
        json!("ac_config_set"),
        "{answer}"
    );
}
