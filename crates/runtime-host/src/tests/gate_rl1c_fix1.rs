// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c fix round 1 (#411; coordinator work order `rd-1027/brief-rl1c-fix1.md`; review
// `rd-1027/v01220-review/rl1c.md` H1 and M1): `actingd suspended`'s report.
//
// - H1: the report's ledger read is bounded. On a large fake ledger (LN-1's synthetic ledger) a
//   report whose budget holds fewer records than the ledger's records of the types it selects
//   answers, incomplete: no rows and the warning `ledger_read_incomplete`; it never holds the
//   whole ledger. The same report without that bound answers complete. No clock is read: the
//   bound is the budget's most records.
// - M1: in composer mode a game whose `schedule.json` the composition refuses is a warning
//   `<code>:<game>:sha256:<hex>` of the report, the refused file by its sha256, which says why
//   that game has no rows.

use super::gate_372k2::{slot_sources, slot_task};
use super::gate_rl1b::{GAME_A, SERVER_A, bundle, schedule_a, write_json};
use super::*;
use crate::SuspendedReadBudget;
use actingcommand_ledger::{SyntheticInstance, SyntheticLedgerSpec, write_synthetic_ledger};
use actingcommand_runtime_database::RuntimeDatabase;
use serde_json::{Value, json};

const GAME_B: &str = "fixture-game-b";
const SERVER_B: &str = "fixture-server-b";

fn warnings(report: &Value) -> Vec<String> {
    report["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("warnings is a list: {report:#}"))
        .iter()
        .map(|warning| warning.as_str().expect("a text warning").to_owned())
        .collect()
}

/// H1 (work order item 1): a report on a large fake ledger whose budget holds fewer records than
/// its read must hold answers incomplete within that budget, with no rows and the warning
/// `ledger_read_incomplete`; the budget is far below the ledger's records, so the report never
/// holds them all. Without that bound the same report answers complete. RL-1c fix 2 (#411 M2)
/// adapts the budget: the read no longer holds the fake ledger's `runtime.fact_recorded` rows,
/// so half of them is no longer below what it holds; one record is (the read holds the head row).
#[test]
fn rl1c_fix1_the_suspended_report_answers_incomplete_within_its_budget() {
    let root = TempDir::new().expect("the state root");
    super::lean_scale::formal_root(root.path());
    let generated = {
        let database =
            RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
        write_synthetic_ledger(
            &database,
            &SyntheticLedgerSpec::new(
                20_000,
                vec![SyntheticInstance {
                    alias: "synthetic-instance-a".to_owned(),
                    instance_id: instance_id(),
                }],
            ),
        )
        .expect("the synthetic ledger")
    };
    let max_events = 1_usize;
    assert!(
        (max_events as u64) * 10 < generated.records,
        "the budget is far below the ledger's records: {generated:?}"
    );

    let host = config(&root);
    let sources = slot_sources(1, &[slot_task(&[600], 1)], None);
    let request = SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &host,
        catalog: &sources,
    };
    let bounded = crate::suspension_report_within(
        &request,
        SuspendedReadBudget {
            max_events,
            ..SuspendedReadBudget::until(None)
        },
    )
    .unwrap_or_else(|error| panic!("the bounded report answers: {error}"));
    assert_eq!(
        warnings(&bounded).last().map(String::as_str),
        Some("ledger_read_incomplete"),
        "{bounded:#}"
    );
    for list in [
        "suspended",
        "lifted",
        "repeating",
        "clock_slots",
        "packs",
        "remedies",
    ] {
        assert_eq!(bounded[list], json!([]), "{list}: {bounded:#}");
    }
    assert_eq!(bounded["through_sequence"], Value::Null, "{bounded:#}");

    let whole = crate::suspension_report_within(&request, SuspendedReadBudget::until(None))
        .unwrap_or_else(|error| panic!("the unbounded report answers: {error}"));
    assert!(
        !warnings(&whole).contains(&"ledger_read_incomplete".to_owned()),
        "{whole:#}"
    );
    assert_eq!(
        whole["through_sequence"],
        json!(generated.head_sequence),
        "{whole:#}"
    );
}

/// M1 (work order item 3): in composer mode game B's `schedule.json`, refused by the composition,
/// is the report's warning `standard_pack_root_invalid:<game>:sha256:<hex>` of the refused file,
/// first among its warnings; game A's rules still compose.
#[test]
fn rl1c_fix1_the_suspended_report_carries_the_composition_problems() {
    let state = TempDir::new().expect("the state root");
    super::lean_scale::formal_root(state.path());
    let packs = TempDir::new().expect("the standard-pack root");
    write_json(
        &packs.path().join(GAME_A).join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &["fixture.read"]),
    );
    write_json(
        &packs.path().join(GAME_A).join("schedule.json"),
        &schedule_a(&[("fixture.read", "fixture.read", 100)]),
    );
    write_json(
        &packs.path().join(GAME_B).join("bundle.json"),
        &bundle(GAME_B, SERVER_B, &["fixture.other"]),
    );
    let refused = b"{";
    fs::write(packs.path().join(GAME_B).join("schedule.json"), refused).expect("schedule.json");
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = [
        (POLICY_INSTANCE_ALIAS, GAME_A, SERVER_A),
        (POLICY_INSTANCE_ALIAS_B, GAME_B, SERVER_B),
    ]
    .into_iter()
    .map(|(instance, game, server)| InstanceSnapshot {
        instance_id: instance.to_owned(),
        server_id: server.to_owned(),
        game_id: game.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    })
    .collect();
    let host = config(&state)
        .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
        .with_standard_packs_root(Some(packs.path().to_owned()))
        .with_standard_pack_schedule(true);

    let schedule = crate::compose_configured_standard_packs(&host)
        .unwrap_or_else(|error| panic!("compose the configuration's rules: {error}"));
    assert!(schedule.compiled().is_some(), "game A's rules compose");
    let report = crate::composed_suspension_report(&crate::ComposedSuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        schedule: &schedule,
    })
    .unwrap_or_else(|error| panic!("actingd suspended in composer mode: {error}"));
    let expected = format!(
        "standard_pack_root_invalid:{GAME_B}:sha256:{:x}",
        Sha256::digest(refused)
    );
    assert_eq!(
        warnings(&report).first(),
        Some(&expected),
        "the refused game is a warning, first: {report:#}"
    );
}
