// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S10 (register §1.346; contracts/emulator-control.md, "Offline instance discovery";
// coordinator work orders rd-1113/brief-453-s10-offline-discovery.md and, fix 1 and fix 2,
// rd-1113/brief-1681-fix1.md and rd-1113/brief-1681-fix2.md): `actingd discover` on its reads
// injected (`MumuManagerReads`: the vendor tool's two and the process listing), so no program is
// spawned:
// - `s10_discover_lists_the_instances_with_the_files_bound_aliases`: the listing's fixture parsed
//   into the host's instance shape, each instance with the alias of the configuration entry whose
//   binding key names it (an index; a name only one listed instance carries; none for a name two
//   carry), the configured folder as written as `mumu_root` (`source` `config`), the version, and
//   the two reads asked once each, in order, of the tool the resolver found; the report line;
// - `s10_discover_resolves_the_named_emulator_root`: `--mumu-root` on a temporary emulator root,
//   without a file: the folder as given, `source` `person`, no bound alias; a root without the
//   vendor tool is `instance_discovery_unavailable` caused by `candidate_absent`, and nothing is
//   read;
// - `s10_discover_a_read_past_its_bound_is_unavailable`: an `info -v all` whose bound expires is
//   `instance_discovery_unavailable` caused by `provider_native_failed` (`backend_launch`);
// - `s10_fix1_discover_a_process_listing_past_its_bound_is_unavailable` (fix 1): with no folder
//   named, a process listing whose bound expires is `instance_discovery_unavailable` caused by
//   `provider_native_failed`, the listing asked once and the vendor tool never;
// - `s10_fix2_several_entries_on_one_instance_the_smallest_instance_id_gives_the_alias` (fix 2):
//   two entries bind one instance, the one with the larger instance id carrying the smaller alias
//   and standing first in the file; the alias is the other's, the smallest instance id's.
// Every root is a temporary folder with neutral names; the tool's files are empty and never run.

use super::*;
use actingcommand_device::codes::DeviceStage;
use actingcommand_device::{
    DeviceDiscoveryProcess, DeviceError, DeviceErrorCategory, DeviceResult, MumuManagerReads,
};
use serde_json::{Value, json};
use std::fs;

fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
    values.iter().map(std::ffi::OsString::from).collect()
}

/// The reads, answered from fixtures: `version` 6.5.7.0, and `info -v all` the listing, or,
/// without one, a read whose bound expires as the real one's does. Each read of the vendor tool
/// asked is kept with the tool it was asked of. Fix 1: the process listing always runs past its
/// bound, as the bounded one fails there (only a discovery that names no folder asks it); `listed`
/// counts the listings asked.
struct FixtureReads {
    listing: Option<Value>,
    asked: Vec<(&'static str, PathBuf)>,
    listed: usize,
}

impl FixtureReads {
    fn new(listing: Option<Value>) -> Self {
        Self {
            listing,
            asked: Vec::new(),
            listed: 0,
        }
    }
}

impl MumuManagerReads for FixtureReads {
    fn version(&mut self, mumu_manager_path: &Path) -> DeviceResult<Value> {
        self.asked
            .push(("version", mumu_manager_path.to_path_buf()));
        Ok(json!({"version": "6.5.7.0"}))
    }

    fn instances(&mut self, mumu_manager_path: &Path) -> DeviceResult<Value> {
        self.asked
            .push(("info -v all", mumu_manager_path.to_path_buf()));
        self.listing.clone().ok_or_else(|| {
            DeviceError::fatal("MuMuManager info -v all timed out after 10s").with_diagnostic(
                DeviceErrorCategory::BackendLaunch,
                DeviceStage::MumuManagerTimeout,
            )
        })
    }

    /// The bounded listing's failure at its bound: the device error its PowerShell run answers,
    /// untyped (`system_processes_within`).
    fn processes(&mut self) -> DeviceResult<Vec<DeviceDiscoveryProcess>> {
        self.listed += 1;
        Err(DeviceError::fatal(
            "powershell -NoProfile -ExecutionPolicy Bypass -Command <the listing> timed out after 10s",
        ))
    }
}

/// An emulator installation root in `base`: `nx_main\` with the vendor tool and its adb (empty
/// files, never run) when `with_tool`, else empty.
fn emulator_root(base: &Path, name: &str, with_tool: bool) -> PathBuf {
    let root = base.join(name);
    let main = root.join("nx_main");
    fs::create_dir_all(&main).expect("the tool folder");
    if with_tool {
        fs::write(main.join("MuMuManager.exe"), b"").expect("the vendor tool");
        fs::write(main.join("adb.exe"), b"").expect("the adb");
    }
    root
}

/// The tool the resolver finds in `root`.
fn tool_of(root: &Path) -> PathBuf {
    fs::canonicalize(root)
        .expect("the canonical root")
        .join("nx_main")
        .join("MuMuManager.exe")
}

/// A folder as written, the way the report names a folder the file or the person gave (fix 1).
fn written_text(root: &Path) -> Value {
    Value::from(root.to_string_lossy().into_owned())
}

/// `info -v all` as the vendor tool answers it: two running instances and two stopped ones (a
/// stopped one carries no endpoint), the last two sharing a name.
fn listing() -> Value {
    json!({
        "0": {"index": "0", "name": "neutral-a", "is_process_started": true,
            "is_android_started": true, "adb_host_ip": "127.0.0.1", "adb_port": 16384,
            "player_state": "start_finished", "android_version": "12"},
        "1": {"index": "1", "name": "neutral-b", "is_process_started": false,
            "is_android_started": false},
        "2": {"index": "2", "name": "neutral-dup", "is_process_started": true,
            "is_android_started": true, "adb_host_ip": "127.0.0.1", "adb_port": 16448,
            "player_state": "start_finished", "android_version": "12"},
        "3": {"index": "3", "name": "neutral-dup", "is_process_started": false,
            "is_android_started": false}
    })
}

/// One discovery-bound entry of a configuration file, its binding key `key`.
fn entry(alias: &str, application: &str, key: Value) -> Value {
    let id = actingcommand_contract::IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id");
    let mut entry = json!({
        "alias": alias, "instance_id": id.transport(), "application_id": application,
        "connect": false, "touch_backend": "adb_shell_input", "capture_backend": "adb"
    });
    for (name, value) in key.as_object().expect("the binding key") {
        entry[name] = value.clone();
    }
    entry
}

#[test]
fn s10_discover_lists_the_instances_with_the_files_bound_aliases() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = emulator_root(temp.path(), "emulator", true);
    let config = temp.path().join("actingd.config.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": temp.path().join("state"),
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "mumu_root": root,
            "instances": [
                entry("neutral.first", "neutral.application.first", json!({"instance_index": 0})),
                entry("neutral.second", "neutral.application.second",
                    json!({"instance_name": "neutral-b"})),
                entry("neutral.shared", "neutral.application.shared",
                    json!({"instance_name": "neutral-dup"})),
            ]
        }))
        .expect("the configuration JSON"),
    )
    .expect("write the configuration");
    let mut reads = FixtureReads::new(Some(listing()));

    let reported = discover::run_with(
        &arguments(&[
            "discover",
            "--config",
            config.to_str().expect("a UTF-8 path"),
        ]),
        &mut reads,
    )
    .unwrap_or_else(|error| panic!("discover answers: {}", error.outcome()));
    let (outcome, developer, line) = report_line(
        discover::SCHEMA_VERSION,
        SharedCode::SerializationFailed,
        Ok(reported),
        Detail::Full,
    );

    assert!(outcome.is(ActingdCode::DiscoverCompleted), "{outcome}");
    assert!(developer.is_none(), "{developer:?}");
    let line: Value = serde_json::from_str(&line).expect("one JSON line");
    assert_eq!(line["schema_version"], discover::SCHEMA_VERSION, "{line}");
    assert_eq!(
        line["outcome"]["code"], "actingd_discover_completed",
        "{line}"
    );
    let report = &line["report"];
    assert_eq!(
        report["mumu_root"],
        json!({"path": written_text(&root), "source": "config"}),
        "the configured folder as written, as check-config reports it: {line}"
    );
    assert_eq!(report["provider_version"], "6.5.7.0", "{line}");
    assert_eq!(
        report["instances"],
        json!([
            {"instance_index": 0, "instance_name": "neutral-a", "adb_host": "127.0.0.1",
                "adb_port": 16384, "running": true, "bound_alias": "neutral.first",
                "android_version": "12"},
            {"instance_index": 1, "instance_name": "neutral-b", "running": false,
                "bound_alias": "neutral.second"},
            {"instance_index": 2, "instance_name": "neutral-dup", "adb_host": "127.0.0.1",
                "adb_port": 16448, "running": true, "android_version": "12"},
            {"instance_index": 3, "instance_name": "neutral-dup", "running": false}
        ]),
        "{line}"
    );
    let tool = tool_of(&root);
    assert_eq!(
        reads.asked,
        [("version", tool.clone()), ("info -v all", tool)],
        "the two reads, once each, in order, of the tool the resolver found"
    );
    assert!(
        !temp.path().join("state").exists(),
        "no state root is made or read"
    );
}

#[test]
fn s10_discover_resolves_the_named_emulator_root() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = emulator_root(temp.path(), "emulator", true);
    let mut reads = FixtureReads::new(Some(json!({
        "0": {"index": "0", "name": "neutral-a", "is_process_started": false,
            "is_android_started": false}
    })));

    let reported = discover::run_with(
        &arguments(&[
            "discover",
            "--mumu-root",
            root.to_str().expect("a UTF-8 path"),
        ]),
        &mut reads,
    )
    .unwrap_or_else(|error| panic!("discover answers: {}", error.outcome()));

    assert!(reported.outcome.is(ActingdCode::DiscoverCompleted));
    let report = reported.data.get("report").expect("the report");
    assert_eq!(
        report["mumu_root"],
        json!({"path": written_text(&root), "source": "person"}),
        "the folder as the person gave it: {report}"
    );
    assert_eq!(
        report["instances"],
        json!([{"instance_index": 0, "instance_name": "neutral-a", "running": false}]),
        "without a file no instance is bound: {report}"
    );
    let tool = tool_of(&root);
    assert_eq!(
        reads.asked,
        [("version", tool.clone()), ("info -v all", tool)]
    );

    // A root without the vendor tool: refused before anything is run.
    let bare = emulator_root(temp.path(), "bare", false);
    let mut reads = FixtureReads::new(Some(listing()));
    let refused = discover::run_with(
        &arguments(&[
            "discover",
            "--mumu-root",
            bare.to_str().expect("a UTF-8 path"),
        ]),
        &mut reads,
    )
    .expect_err("a root without the tool is refused");
    let outcome = refused.outcome();
    assert!(
        outcome.is(SharedCode::InstanceDiscoveryUnavailable),
        "{outcome}"
    );
    let cause = outcome
        .causes()
        .first()
        .expect("the device failure")
        .outcome();
    assert_eq!(cause.code().as_str(), "candidate_absent", "{outcome}");
    assert_eq!(
        cause.value("install_source"),
        Some(&Value::from("explicit_folder")),
        "{outcome}"
    );
    assert_eq!(
        cause.value("mumu_root"),
        Some(&Value::from(bare.to_string_lossy().into_owned())),
        "{outcome}"
    );
    assert!(reads.asked.is_empty(), "nothing read: {:?}", reads.asked);
}

#[test]
fn s10_discover_a_read_past_its_bound_is_unavailable() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = emulator_root(temp.path(), "emulator", true);
    let mut reads = FixtureReads::new(None);

    let refused = discover::run_with(
        &arguments(&[
            "discover",
            "--mumu-root",
            root.to_str().expect("a UTF-8 path"),
        ]),
        &mut reads,
    )
    .expect_err("a listing past its bound is refused");

    let outcome = refused.outcome();
    assert!(
        outcome.is(SharedCode::InstanceDiscoveryUnavailable),
        "{outcome}"
    );
    assert_eq!(outcome.category(), Category::Error, "{outcome}");
    let cause = outcome
        .causes()
        .first()
        .expect("the device failure")
        .outcome();
    assert_eq!(cause.code().as_str(), "provider_native_failed", "{outcome}");
    assert_eq!(
        cause.value("device_category"),
        Some(&Value::from("backend_launch")),
        "{outcome}"
    );
    assert_eq!(
        cause.value("mumu_root"),
        Some(&Value::from(root.to_string_lossy().into_owned())),
        "{outcome}"
    );
    let tool = tool_of(&root);
    assert_eq!(
        reads.asked,
        [("version", tool.clone()), ("info -v all", tool)],
        "the listing is asked once and not again"
    );
}

/// Fix 1 (review, the process listing without a bound): with no folder named, the resolver's
/// process listing is a read of `MumuManagerReads`, bounded in `actingd discover`; one past its
/// bound is a discovery that could not be made, `instance_discovery_unavailable` of the existing
/// refusals, caused by the device failure (`provider_native_failed`), and nothing else is run.
#[test]
fn s10_fix1_discover_a_process_listing_past_its_bound_is_unavailable() {
    let mut reads = FixtureReads::new(Some(listing()));

    let refused = discover::run_with(&arguments(&["discover"]), &mut reads)
        .expect_err("a process listing past its bound is refused");

    let outcome = refused.outcome();
    assert!(
        outcome.is(SharedCode::InstanceDiscoveryUnavailable),
        "{outcome}"
    );
    assert_eq!(outcome.category(), Category::Error, "{outcome}");
    let cause = outcome
        .causes()
        .first()
        .expect("the device failure")
        .outcome();
    assert_eq!(cause.code().as_str(), "provider_native_failed", "{outcome}");
    assert_eq!(
        cause.value("mumu_root"),
        None,
        "no folder was named: {outcome}"
    );
    assert_eq!(reads.listed, 1, "the processes are listed once");
    assert!(
        reads.asked.is_empty(),
        "the vendor tool is never run: {:?}",
        reads.asked
    );
}

/// Fix 2 (the review of fix 1): of several entries on one instance, the entry with the smallest
/// `instance_id` gives the alias, as the Runtime's answer takes it. The entry with the larger id
/// carries the smaller alias and stands first in the file, so neither the least alias nor the
/// file's order gives this answer.
#[test]
fn s10_fix2_several_entries_on_one_instance_the_smallest_instance_id_gives_the_alias() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = emulator_root(temp.path(), "emulator", true);
    let issuer = actingcommand_contract::IdentifierIssuer::new().expect("issuer");
    let first = *issuer.mint_instance_id().expect("instance id").transport();
    let second = *issuer.mint_instance_id().expect("instance id").transport();
    let (smaller, larger) = (first.min(second), first.max(second));
    assert!(smaller < larger, "two distinct instance ids");
    let mut larger_id = entry(
        "neutral.alpha",
        "neutral.application.alpha",
        json!({"instance_index": 0}),
    );
    larger_id["instance_id"] = json!(larger);
    let mut smaller_id = entry(
        "neutral.zeta",
        "neutral.application.zeta",
        json!({"instance_index": 0}),
    );
    smaller_id["instance_id"] = json!(smaller);
    let config = temp.path().join("actingd.config.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": temp.path().join("state"),
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "mumu_root": root,
            "instances": [larger_id, smaller_id]
        }))
        .expect("the configuration JSON"),
    )
    .expect("write the configuration");
    let mut reads = FixtureReads::new(Some(json!({
        "0": {"index": "0", "name": "neutral-a", "is_process_started": false,
            "is_android_started": false}
    })));

    let reported = discover::run_with(
        &arguments(&[
            "discover",
            "--config",
            config.to_str().expect("a UTF-8 path"),
        ]),
        &mut reads,
    )
    .unwrap_or_else(|error| panic!("discover answers: {}", error.outcome()));

    assert!(reported.outcome.is(ActingdCode::DiscoverCompleted));
    let report = reported.data.get("report").expect("the report");
    assert_eq!(
        report["instances"],
        json!([{"instance_index": 0, "instance_name": "neutral-a", "running": false,
            "bound_alias": "neutral.zeta"}]),
        "the entry with the smallest instance id, not the least alias nor the first in the \
         file: {report}"
    );
}
