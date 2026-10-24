// SPDX-License-Identifier: AGPL-3.0-only

//! LN-1 (Workflow #422, LEAN-L model §7.2): the scale harness. For each history size it fills a
//! fresh state root with the ledger crate's synthetic ledger (no device, neutral words), then
//! starts the Runtime host on it twice, each start in a child process of its own, as the test
//! binary re-run on one ignored entry, so every measure has its own baseline. The first start
//! carries the one-time work of a new build; the second is the steady start. Each start reports:
//! - the time from the process's launch to ready (`RuntimeHost::start` returned, so
//!   `runtime-info.json` is published) and the start call's own time;
//! - the ledger open's phases (`ledger_opened`: `sql_read_ms`, `verify_ms`, `material_ms`,
//!   `restore_ms`, `event_count`);
//! - the recovery steps of the `actingd startup_recovery` line, with the writer's peak, its
//!   whole-ledger-query counter among it;
//! - private bytes, peak private bytes, working set and peak working set at ready and again
//!   `ACTINGCOMMAND_LEAN_SCALE_SETTLE_MS` (10 s by default) later.
//!
//! LN-1 only reports: the numbers are printed, and written as `lean-scale-report.json` and
//! `lean-scale-report.md` to `ACTINGCOMMAND_LEAN_SCALE_REPORT_DIR` when it is set. LN-8 makes them
//! a gate with the model's thresholds (§7.1). A start that fails, a child that ends without its
//! report or a measure that is missing fails the harness.
//!
//! The sizes are `ACTINGCOMMAND_LEAN_SCALE_SIZES` (records, comma separated; 10,000 and
//! 1,000,000 by default). The `Lean scale` workflow runs it on a release build, as production
//! runs: `cargo test --release -p actingcommand-runtime-host --lib -- --ignored --exact
//! tests::lean_scale::lean_scale_reports_start_measures --nocapture`.

use super::*;
use actingcommand_ledger::{
    LedgerMaintenance, ProcessMemory, SyntheticInstance, SyntheticLedgerSpec, process_age_ms,
    process_memory, write_synthetic_ledger,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use serde_json::{Value, json};

const SIZES_ENV: &str = "ACTINGCOMMAND_LEAN_SCALE_SIZES";
const SETTLE_ENV: &str = "ACTINGCOMMAND_LEAN_SCALE_SETTLE_MS";
const REPORT_DIR_ENV: &str = "ACTINGCOMMAND_LEAN_SCALE_REPORT_DIR";
// The child's state root and the file it writes its report to.
const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_LEAN_SCALE_ROOT";
const CHILD_REPORT_ENV: &str = "ACTINGCOMMAND_LEAN_SCALE_CHILD_REPORT";
const CHILD_TEST: &str = "tests::lean_scale::lean_scale_start_child";
const DEFAULT_SIZES: [u64; 2] = [10_000, 1_000_000];
const DEFAULT_SETTLE_MS: u64 = 10_000;
// The host test configuration's salt (`config`), also the State database's bootstrap seed here.
const SALT: &[u8] = b"runtime-host-test-salt";
const OWNER: &str = "lean-scale-fixture";
// The fixture instance: the host registers it under `POLICY_INSTANCE_ALIAS`, and the synthetic
// history's runs, inputs and facts name it.
const INSTANCE_ID: &str = "\"instance_000000000000000000000000000000a1\"";
// The two starts on each history.
const STARTS: [&str; 2] = ["first", "second"];
// The ledger open's phases, whose sum is its measured time.
const OPEN_PHASES: [&str; 4] = ["sql_read_ms", "verify_ms", "material_ms", "restore_ms"];

fn scale_instance_id() -> InstanceId {
    serde_json::from_str(INSTANCE_ID).expect("the fixture instance id")
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn settle_ms() -> u64 {
    match std::env::var(SETTLE_ENV) {
        Ok(value) => value
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{SETTLE_ENV}={value:?} is not milliseconds")),
        Err(_) => DEFAULT_SETTLE_MS,
    }
}

fn sizes() -> Vec<u64> {
    match std::env::var(SIZES_ENV) {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|size| !size.is_empty())
            .map(|size| {
                size.parse().unwrap_or_else(|_| {
                    panic!("{SIZES_ENV}={value:?}: {size:?} is no record count")
                })
            })
            .collect(),
        Err(_) => DEFAULT_SIZES.to_vec(),
    }
}

// The configuration `config` builds, for a root the parent made.
fn scale_config(root: &Path) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root, SALT)
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest())
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
}

// A closed formal root as acsetup leaves it: the State database, the formal empty ledger, and
// one writer open and close, whose owner metadata every later open reads.
fn formal_root(root: &Path) {
    let database =
        Arc::new(RuntimeStateStore::open_database(root, SALT).expect("the State database"));
    let limits = MaintenanceLimits::default();
    let maintenance =
        LedgerMaintenance::acquire(root, true, limits, limits.deadline().expect("deadline"))
            .expect("the maintenance owner");
    maintenance
        .initialize_empty(&database)
        .expect("the formal ledger");
    maintenance
        .open_writer(Arc::clone(&database), OWNER.into())
        .expect("the formal writer")
        .close()
        .expect("close the formal writer");
}

// The bytes of the root's database and its write-ahead log.
fn database_bytes(root: &Path) -> u64 {
    [
        RUNTIME_STATE_DATABASE_FILE.to_owned(),
        format!("{RUNTIME_STATE_DATABASE_FILE}-wal"),
    ]
    .iter()
    .filter_map(|name| fs::metadata(root.join(name)).ok())
    .map(|metadata| metadata.len())
    .sum()
}

fn memory_json(memory: ProcessMemory) -> Value {
    json!({
        "private_bytes": memory.private_bytes,
        "peak_private_bytes": memory.peak_private_bytes,
        "working_set_bytes": memory.working_set_bytes,
        "peak_working_set_bytes": memory.peak_working_set_bytes,
    })
}

// The child: one start of the Runtime host on the root the parent filled, measured and closed.
#[test]
#[ignore = "child process entry: lean_scale_reports_start_measures runs it with --ignored --exact"]
fn lean_scale_start_child() {
    let (Ok(root), Ok(report_path)) = (
        std::env::var(CHILD_ROOT_ENV),
        std::env::var(CHILD_REPORT_ENV),
    ) else {
        return;
    };
    let settle = Duration::from_millis(settle_ms());
    let launch_to_call_ms = process_age_ms().expect("the process's age before the start");
    let started = Instant::now();
    let host = RuntimeHost::start(
        scale_config(Path::new(&root)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            scale_instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the Runtime host starts on the synthetic ledger");
    let start_ms = milliseconds(started.elapsed());
    let launch_to_ready_ms = process_age_ms().expect("the process's age at ready");
    let at_ready = process_memory().expect("the memory at ready");
    let opened = host.ledger_opened().clone();
    let recovery = host.startup_recovery_for_test();
    thread::sleep(settle);
    let settled = process_memory().expect("the memory after settling");
    host.close().expect("close the Runtime host");
    let open = OPEN_PHASES
        .iter()
        .chain(["event_count", "timeout_ms"].iter())
        .map(|key| {
            let value = opened
                .value(key)
                .and_then(Value::as_u64)
                .unwrap_or_else(|| panic!("ledger_opened has {key}: {opened:?}"));
            ((*key).to_owned(), json!(value))
        })
        .collect::<serde_json::Map<_, _>>();
    let ledger_open_ms = OPEN_PHASES
        .iter()
        .map(|key| opened.value(key).and_then(Value::as_u64).unwrap_or(0))
        .sum::<u64>();
    let writer = &recovery.writer;
    let writer_longest_ms = writer
        .longest
        .as_ref()
        .map(|(_, duration)| milliseconds(*duration));
    let writer_longest_command = writer
        .longest
        .as_ref()
        .map(|(command, _)| format!("{command:?}"));
    let startup_recovery = json!({
        "fact_store_ms": recovery.fact_store_ms,
        "monitor_registry_ms": recovery.monitor_registry_ms,
        "policy_host_ms": recovery.policy_host_ms,
        "policy_dispatches_ms": recovery.policy_dispatches_ms,
        "policy_outcomes_ms": recovery.policy_outcomes_ms,
        "approvals_ms": recovery.approvals_ms,
        "runtime_state_ms": recovery.runtime_state_ms,
        "agent_dispatcher_ms": recovery.agent_dispatcher_ms,
        "recovery_ms": recovery.recovery_ms,
        "writer_commands": writer.commands,
        "writer_longest_ms": writer_longest_ms,
        "writer_longest_command": writer_longest_command,
        "writer_largest_read_events": writer.largest_read_events,
        "writer_whole_ledger_queries": writer.whole_ledger_queries,
        "connection_longest_wait_ms": recovery.connection_longest_wait_ms,
    });
    let startup_recovery_line = format!("actingd startup_recovery {recovery}");
    let report = json!({
        "launch_to_call_ms": launch_to_call_ms,
        "launch_to_ready_ms": launch_to_ready_ms,
        "start_ms": start_ms,
        "ledger_open": open,
        "ledger_open_ms": ledger_open_ms,
        "startup_recovery": startup_recovery,
        "startup_recovery_line": startup_recovery_line,
        "settle_ms": milliseconds(settle),
        "memory_at_ready": memory_json(at_ready),
        "memory_settled": memory_json(settled),
    });
    fs::write(
        report_path,
        serde_json::to_vec_pretty(&report).expect("the child report's bytes"),
    )
    .expect("write the child report");
}

// The last lines of a child's output, for a failure message.
fn tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_else(|error| format!("<{error}>"));
    let lines = text.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

// One start in its own child process; its report, with the parent's view of it.
fn start_child(root: &Path, report_dir: &Path, records: u64, start: &str) -> Value {
    let stem = format!("lean-scale-{records}-{start}");
    let report_path = report_dir.join(format!("{stem}.json"));
    let stdout_path = report_dir.join(format!("{stem}.stdout.txt"));
    let stderr_path = report_dir.join(format!("{stem}.stderr.txt"));
    let stdout = fs::File::create(&stdout_path).expect("the child's stdout file");
    let stderr = fs::File::create(&stderr_path).expect("the child's stderr file");
    let spawned = Instant::now();
    let status = Command::new(std::env::current_exe().expect("the test executable"))
        .args(["--ignored", "--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ROOT_ENV, root)
        .env(CHILD_REPORT_ENV, &report_path)
        .env(SETTLE_ENV, settle_ms().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .status()
        .expect("run the start's child process");
    let spawn_to_exit_ms = milliseconds(spawned.elapsed());
    assert!(
        status.success() && report_path.is_file(),
        "the {start} start on {records} records failed ({status}); stdout:\n{}\nstderr:\n{}",
        tail(&stdout_path),
        tail(&stderr_path)
    );
    let mut report: Value =
        serde_json::from_slice(&fs::read(report_path).expect("the child's report"))
            .expect("the child's report is JSON");
    let printed = fs::read_to_string(stdout_path)
        .expect("the child's stdout")
        .lines()
        .find(|line| line.contains("actingd startup_recovery "))
        .map(str::to_owned);
    assert!(
        printed.is_some(),
        "the {start} start on {records} records printed no startup_recovery line"
    );
    report["startup_recovery_printed"] = json!(printed);
    report["spawn_to_exit_ms"] = json!(spawn_to_exit_ms);
    let event_count = report["ledger_open"]["event_count"]
        .as_u64()
        .expect("the open's event count");
    assert!(
        event_count >= records,
        "the {start} start opened {event_count} records of {records}: {report}"
    );
    assert!(
        report["startup_recovery"]["recovery_ms"].is_u64(),
        "every recovery step of the {start} start on {records} records ran: {report}"
    );
    for memory in ["memory_at_ready", "memory_settled"] {
        assert!(
            report[memory]["private_bytes"]
                .as_u64()
                .is_some_and(|bytes| bytes > 0),
            "the {start} start on {records} records has {memory}: {report}"
        );
    }
    report
}

// One history size: the generated ledger and its two starts.
fn measure(records: u64, report_dir: &Path) -> Value {
    let root = TempDir::new().expect("the state root");
    formal_root(root.path());
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
    let spec = SyntheticLedgerSpec::new(
        records,
        vec![SyntheticInstance {
            alias: POLICY_INSTANCE_ALIAS.to_owned(),
            instance_id: scale_instance_id(),
        }],
    );
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    drop(database);
    assert_eq!(generated.records, records, "{generated:?}");
    let bytes = database_bytes(root.path());
    println!(
        "lean_scale records={records} generated_ms={} canonical_bytes={} database_bytes={bytes} runs={} frames={}",
        generated.elapsed_ms, generated.canonical_bytes, generated.runs, generated.frames
    );
    let mut starts = serde_json::Map::new();
    for start in STARTS {
        let report = start_child(root.path(), report_dir, records, start);
        println!(
            "lean_scale records={records} start={start} launch_to_ready_ms={} ledger_open_ms={} recovery_ms={} whole_ledger_queries={} private_bytes_settled={} peak_private_bytes={}",
            report["launch_to_ready_ms"],
            report["ledger_open_ms"],
            report["startup_recovery"]["recovery_ms"],
            report["startup_recovery"]["writer_whole_ledger_queries"],
            report["memory_settled"]["private_bytes"],
            report["memory_settled"]["peak_private_bytes"],
        );
        starts.insert(start.to_owned(), report);
    }
    let generation = serde_json::to_value(generated).expect("the generation report");
    json!({
        "records": records,
        "generation": generation,
        "database_bytes": bytes,
        "starts": starts,
    })
}

fn mib(value: &Value) -> String {
    value.as_u64().map_or_else(
        || "-".to_owned(),
        |bytes| format!("{:.1}", bytes as f64 / (1024.0 * 1024.0)),
    )
}

fn number(value: &Value) -> String {
    value
        .as_u64()
        .map_or_else(|| "-".to_owned(), |value| value.to_string())
}

// The report as a Markdown table, one row per start.
fn markdown(cases: &[Value]) -> String {
    let mut text = String::from(
        "## LN-1 scale harness (reports only; LN-8 gates)\n\n\
         | records | start | launch to ready ms | start ms | ledger open ms (sql read / verify / restore) | recovery ms | fact store ms | monitor ms | policy host ms | runtime state ms | agent ms | whole-ledger queries | largest read events | private MiB at ready | private MiB settled | peak private MiB | peak working set MiB |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for case in cases {
        for start in STARTS {
            let report = &case["starts"][start];
            let recovery = &report["startup_recovery"];
            let open = &report["ledger_open"];
            text.push_str(&format!(
                "| {} | {start} | {} | {} | {} ({} / {} / {}) | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                number(&case["records"]),
                number(&report["launch_to_ready_ms"]),
                number(&report["start_ms"]),
                number(&report["ledger_open_ms"]),
                number(&open["sql_read_ms"]),
                number(&open["verify_ms"]),
                number(&open["restore_ms"]),
                number(&recovery["recovery_ms"]),
                number(&recovery["fact_store_ms"]),
                number(&recovery["monitor_registry_ms"]),
                number(&recovery["policy_host_ms"]),
                number(&recovery["runtime_state_ms"]),
                number(&recovery["agent_dispatcher_ms"]),
                number(&recovery["writer_whole_ledger_queries"]),
                number(&recovery["writer_largest_read_events"]),
                mib(&report["memory_at_ready"]["private_bytes"]),
                mib(&report["memory_settled"]["private_bytes"]),
                mib(&report["memory_settled"]["peak_private_bytes"]),
                mib(&report["memory_settled"]["peak_working_set_bytes"]),
            ));
        }
    }
    text.push_str("\n| records | generated ms | canonical MiB | database MiB | runs | frames |\n|---|---|---|---|---|---|\n");
    for case in cases {
        let generation = &case["generation"];
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            number(&case["records"]),
            number(&generation["elapsed_ms"]),
            mib(&generation["canonical_bytes"]),
            mib(&case["database_bytes"]),
            number(&generation["runs"]),
            number(&generation["frames"]),
        ));
    }
    text
}

// The harness: every size, both starts; the report printed and, with the report folder set,
// written there.
#[test]
#[ignore = "LN-1 scale harness: the Lean scale workflow runs it with --ignored --exact; it reports, LN-8 gates"]
fn lean_scale_reports_start_measures() {
    let sizes = sizes();
    assert!(!sizes.is_empty(), "{SIZES_ENV} names no size");
    let scratch = TempDir::new().expect("the report scratch folder");
    let report_dir = match std::env::var(REPORT_DIR_ENV) {
        Ok(folder) => {
            let folder = PathBuf::from(folder);
            fs::create_dir_all(&folder).expect("the report folder");
            folder
        }
        Err(_) => scratch.path().to_path_buf(),
    };
    let cases = sizes
        .iter()
        .map(|records| measure(*records, &report_dir))
        .collect::<Vec<_>>();
    let report = json!({
        "format": "actingcommand-lean-scale/1",
        "settle_ms": settle_ms(),
        "cases": cases,
    });
    let table = markdown(&cases);
    println!("{table}");
    fs::write(
        report_dir.join("lean-scale-report.json"),
        serde_json::to_vec_pretty(&report).expect("the report's bytes"),
    )
    .expect("write the report");
    fs::write(report_dir.join("lean-scale-report.md"), table).expect("write the summary");
}
