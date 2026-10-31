// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-4 (A.6 rule 2; register §1.239; since 0.12.36), test 7, the counters' read
//! points of the settings' version: after a change through `ac_config_set`, and again after one
//! through `actingctl config set`, each of whose answers carries the version after it, MCP
//! `ac_facts` with scope `config` and `actingctl status --config` answer that same version beside
//! the `config.*` records. The Runtime runs in-process on a fixed clock.

use super::super::jobs::Jobs;
use super::super::protocol::Era;
use super::super::runtime::RuntimeAccess;
use super::super::tools::{ToolContext, find};
use crate::held_runtime::{HELD_SALT, NoInstances};
use actingcommand_contract::outcome::Detail;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, RuntimeConfigManifest,
};
use actingcommand_runtime_config::{STUCK_RECOVERY_COOLDOWN_SECS, instance_key};
use actingcommand_runtime_host::{
    RuntimeClock, RuntimeClockSample, RuntimeHost, RuntimeHostConfig, RuntimeHostResult,
};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "instance_0123456789abcdef0123456789abcdef";
/// The fixed clock's wall-clock reading before any wait.
const START_UNIX_MS: u64 = 1_800_000_000_000;

/// The Runtime's clock: fixed, moved only by the waits the Runtime itself takes.
struct FixedClock {
    elapsed_ms: AtomicU64,
}

impl RuntimeClock for FixedClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        let elapsed_ms = self.elapsed_ms.load(Ordering::SeqCst);
        Ok(RuntimeClockSample {
            unix_ms: START_UNIX_MS + elapsed_ms,
            monotonic_ms: elapsed_ms,
        })
    }

    fn pause(&self, duration: Duration) {
        let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.elapsed_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

/// The result of the tool `name` with `arguments`; a refusal fails the test.
fn tool(context: &ToolContext<'_>, name: &str, arguments: Value) -> Value {
    let tool = find(name).unwrap_or_else(|| panic!("{name} is in the tool table"));
    let Value::Object(arguments) = arguments else {
        panic!("the arguments are an object");
    };
    match (tool.run)(context, &arguments) {
        Ok(success) => success.result,
        Err(error) => panic!("{name} was refused: {:?}", error.outcome_ref()),
    }
}

/// The members of the line `actingctl <words> --state-root <root>` prints beside its outcome.
fn cli(root: &Path, words: &[&str]) -> Map<String, Value> {
    let mut arguments = words.iter().map(OsString::from).collect::<Vec<_>>();
    arguments.push(OsString::from("--state-root"));
    arguments.push(root.as_os_str().to_owned());
    match crate::run(arguments, Detail::Full) {
        Ok(done) => done.members,
        Err(error) => panic!("actingctl {words:?}: {error:?}"),
    }
}

#[test]
fn r4_t7_each_read_point_answers_the_version_the_last_receipt_carried() {
    let root = TempDir::new().expect("tempdir");
    let cooldown = instance_key(INSTANCE, STUCK_RECOVERY_COOLDOWN_SECS);
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), HELD_SALT)
            .with_runtime_clock(Arc::new(FixedClock {
                elapsed_ms: AtomicU64::new(0),
            }))
            .with_config_manifest(RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters: vec![ConfigParameter {
                    key: cooldown.clone(),
                    value: FactScalar::Integer(600),
                    source: ConfigParameterSource::Default,
                    settable: true,
                }],
            }),
        Arc::new(NoInstances),
    )
    .expect("the Runtime starts");
    let runtime = RuntimeAccess::new(None, Some(root.path().to_path_buf()));
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: super::gate_mcpt1::unreached_deadline(),
        era: Era::Modern,
        session: 1,
        jobs: &jobs,
    };

    // A change through MCP, then each read point.
    let through_mcp = tool(
        &context,
        "ac_config_set",
        json!({"changes": [{"key": cooldown, "value": 120}]}),
    );
    let first = version_of("ac_config_set", through_mcp.as_object());
    assert_read_points(&context, root.path(), first, "ac_config_set");

    // A change through the CLI, then each read point again.
    let through_cli = cli(root.path(), &["config", "set", &format!("{cooldown}=90")]);
    let second = version_of("actingctl config set", Some(&through_cli));
    assert_ne!(second, first, "each committed change has its own version");
    assert_read_points(&context, root.path(), second, "actingctl config set");
    host.close().expect("close the Runtime");
}

/// The `config_version` a write's answer carries.
#[track_caller]
fn version_of(writer: &str, answer: Option<&Map<String, Value>>) -> u64 {
    answer
        .and_then(|members| members.get("config_version"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{writer} answers no config_version: {answer:?}"))
}

/// MCP `ac_facts` with scope `config` and `actingctl status --config` answer `version`, the one
/// `writer`'s answer carried, beside their records.
#[track_caller]
fn assert_read_points(context: &ToolContext<'_>, root: &Path, version: u64, writer: &str) {
    let facts = tool(context, "ac_facts", json!({"scope": "config"}));
    assert!(
        facts.get("records").is_some(),
        "ac_facts after {writer}: {facts}"
    );
    assert_eq!(
        facts.get("config_version"),
        Some(&json!(version)),
        "ac_facts after {writer}: {facts}"
    );
    let status = cli(root, &["status", "--config"]);
    assert!(
        status.get("records").is_some(),
        "status --config after {writer}: {status:?}"
    );
    assert_eq!(
        status.get("config_version"),
        Some(&json!(version)),
        "status --config after {writer}: {status:?}"
    );
}
