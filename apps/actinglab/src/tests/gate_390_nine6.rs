// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 nine6 (Alice's nine additions of 2026-10-10, item 6; wave-18 ruling 5; register
// §1.92; exec gate key 81): `resource validate`, the tool that reads a scheduling pools document,
// warns about a resource pool that nothing produces, and the warning is not a refusal. A live
// (`ledger_fact`) pool is produced only by a reading mark or an adjustment of its key, and both
// write an instance fact in the run's instance scope (contracts/resource-readings.md, "Host",
// "Live pools"), while the pool reads its key in its own scope (contracts/scheduling/README.md,
// "Pool Projection"). Archived material is no declaration (contracts/resource-declarations.md).
// Every expected value comes from the ruling and those contracts.

use super::*;

const POOLS: &str = "scheduling/pools.json";
const WARNING: &str = "resource_pool_producer_missing";

// One live pool of the given scope, observing `fact_key`.
fn live_pool(id: &str, scope: Value, fact_key: &str) -> Value {
    json!({
        "id": id,
        "scope": scope,
        "capacity": 999,
        "projection": {"amount": 1, "per_ms": 360000},
        "observation": {"kind": "fact", "fact_key": fact_key},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 900},
        "group_delay": null
    })
}

fn instance_scope() -> Value {
    json!({"kind": "instance", "instance_id": "instance-a"})
}

// A repository with the pools document and the operation task files given, by path.
fn repository(pools: Vec<Value>, operations: &[(&str, Value)]) -> TempDir {
    let temp = TempDir::new().unwrap();
    let mut files = vec![(
        POOLS.to_owned(),
        json!({
            "schema_version": "actingcommand.scheduling.v1",
            "catalog": {
                "catalog_id": "neutral.catalog",
                "catalog_version": 1,
                "approval_refs": ["approval:neutral"]
            },
            "pools": pools
        }),
    )];
    files.extend(
        operations
            .iter()
            .map(|(path, task)| ((*path).to_owned(), task.clone())),
    );
    for (path, document) in files {
        let file = temp.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    }
    temp
}

// A reading task: one `current_capacity` mark that writes `fact_key` and `capacity_fact_key`.
fn reading_task(fact_key: &str, capacity_fact_key: &str) -> Value {
    json!({
        "task_id": "reader",
        "resource_readings": [{
            "id": "storage",
            "fact_key": fact_key,
            "capacity_fact_key": capacity_fact_key,
            "page_id": "home",
            "target_id": "ocr/storage",
            "trim": "whitespace_v1",
            "value": {"type": "unsigned_integer", "min": 0, "max": 999, "format": "current_capacity"},
            "minimum_confidence_milli": 900,
            "valid_for_ms": 21600000
        }]
    })
}

fn validate_pools(repo: &Path) -> CliResult {
    run_cli(
        [
            "--json",
            "resource",
            "validate",
            "--repo",
            repo.to_str().unwrap(),
            "--changed-path",
            POOLS,
        ],
        true,
    )
}

// The validation succeeds with the document valid, and its warnings are exactly these pools'.
fn assert_warned(result: &CliResult, warned: &[(usize, &str)]) {
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let report = result.envelope.data.as_ref().unwrap();
    assert_eq!(report["status"], "valid", "{report}");
    assert_eq!(
        report["entries"],
        json!([{"path": POOLS, "status": "valid", "family": "scheduling"}]),
        "{report}"
    );
    let expected = warned
        .iter()
        .map(|(index, fact_key)| {
            json!({
                "code": WARNING,
                "path": POOLS,
                "json_pointer": format!("/pools/{index}"),
                "fact_key": fact_key,
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(report["warnings"], json!(expected), "{report}");
}

// Of four live instance pools, the one whose key no reading mark and no adjustment of the
// repository writes gets the warning: a mark's key and its capacity key are produced, an
// adjustment's key is produced wherever its operation lies, and a reading in archived material
// produces nothing. The pools document stays valid and the command succeeds.
#[test]
fn nine6_a_live_pool_no_reading_or_adjustment_writes_gets_a_warning_not_a_refusal() {
    let temp = repository(
        vec![
            live_pool("pool.stock", instance_scope(), "resource.stock"),
            live_pool("pool.storage", instance_scope(), "resource.stock_capacity"),
            live_pool("pool.parts", instance_scope(), "inventory.parts"),
            live_pool("pool.tokens", instance_scope(), "resource.tokens"),
        ],
        &[
            (
                "packs/neutral.reader/resources/operations/reader/task.json",
                reading_task("resource.stock", "resource.stock_capacity"),
            ),
            (
                "ours/operations/exchange/task.json",
                json!({
                    "task_id": "exchange",
                    "resource_adjustments": [
                        {"id": "parts", "fact_key": "inventory.parts", "operation_id": "buy"}
                    ]
                }),
            ),
            (
                "packages/archive/resources/operations/reader/task.json",
                reading_task("resource.tokens", "resource.tokens_capacity"),
            ),
        ],
    );

    assert_warned(&validate_pools(temp.path()), &[(3, "resource.tokens")]);
}

// A reading and an adjustment write an instance fact, so a live pool of server or game scope is
// never published even though a reading of the repository writes its key; the instance pool of
// the same key is.
#[test]
fn nine6_a_live_pool_of_server_or_game_scope_gets_the_warning_though_a_reading_writes_its_key() {
    let temp = repository(
        vec![
            live_pool(
                "pool.server",
                json!({"kind": "server", "server_id": "server-a"}),
                "resource.stock",
            ),
            live_pool(
                "pool.game",
                json!({"kind": "game", "game_id": "neutral"}),
                "resource.stock",
            ),
            live_pool("pool.instance", instance_scope(), "resource.stock"),
        ],
        &[(
            "packs/neutral.reader/resources/operations/reader/task.json",
            reading_task("resource.stock", "resource.stock_capacity"),
        )],
    );

    assert_warned(
        &validate_pools(temp.path()),
        &[(0, "resource.stock"), (1, "resource.stock")],
    );
}
