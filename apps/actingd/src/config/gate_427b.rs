// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #427-B (#427 plan r2 point 5; review C4, comment 6459928838; approved whole by Alice
// 10-28 12:4x; `contracts/runtime-config.md`, "The performance section"): the start-up assembly
// hands the controller the heavy-dispatch limits and the one optional key of the shared run
// limits, `performance.pacing.max_concurrent_runs`, in each pacing mode alike: `observe`,
// `pace` and `full` change neither. Without the key there is no limit and no parameter row; a
// file that names it carries it, explicit; 0 is refused.

use super::*;
use actingcommand_contract::{ConfigParameterSource, FactScalar, IdentifierIssuer};
use serde_json::{Value, json};

/// A configuration of one fixture instance with this `performance` section.
fn configuration(performance: Value) -> Value {
    let id = IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id");
    json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": "state",
        "bind_host": "127.0.0.1",
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": [{
            "alias": "neutral.fixture", "instance_id": id.transport(),
            "fixture_backend": {
                "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}], "max_inputs": 1
            }
        }],
        "performance": performance
    })
}

#[test]
fn gate_427b_the_assembly_hands_the_limits_and_the_max_key_over_in_every_pacing_mode() {
    for mode in ["observe", "pace", "full"] {
        for max in [None, Some(3), Some(1)] {
            let mut pacing = json!({
                "mode": mode,
                "heavy_dispatch_limit": {"normal": 2, "paused": 1}
            });
            if let Some(max) = max {
                pacing["max_concurrent_runs"] = json!(max);
            }
            let file = serde_json::from_value::<ActingdConfigFile>(configuration(
                json!({ "pacing": pacing }),
            ))
            .expect("typed config");
            let assembly = file.assemble().expect("runtime assembly");
            let control = assembly.host.performance_control();
            assert_eq!(
                (
                    control.normal_heavy_dispatch_limit(),
                    control.pressured_heavy_dispatch_limit(),
                    control.max_concurrent_runs(),
                ),
                (2, 1, max),
                "{mode} {max:?}"
            );
            let row = assembly
                .manifest
                .parameters
                .iter()
                .find(|parameter| parameter.key == pacing::MAX_CONCURRENT_RUNS);
            match max {
                None => assert!(row.is_none(), "{mode}: {row:?}"),
                Some(max) => {
                    let row = row.unwrap_or_else(|| panic!("{mode}: the named key's row"));
                    assert_eq!(row.value, FactScalar::Integer(i64::from(max)), "{mode}");
                    assert_eq!(row.source, ConfigParameterSource::Explicit, "{mode}");
                }
            }
        }
    }

    let file = serde_json::from_value::<ActingdConfigFile>(configuration(
        json!({ "pacing": {"max_concurrent_runs": 0} }),
    ))
    .expect("typed config");
    assert!(
        file.assemble().is_err(),
        "a limit of 0 concurrent runs is refused"
    );
}
