// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-8 (contracts/lab-recording.md, "A recording holds its instance"; register §1.428): the
// state root actinglab's own commands reach (`runtime_state_root`) is the variable's when it is set,
// else the state root of the installation the program runs from, located as the CLI module's rows
// locate it, else (from no installation) the default. The installation is made up as the CLI
// module's tests make theirs (gate_434_lab5f, gate_434_lab5g4c): a single-location root in a
// temporary folder, `runtime\BUILD-MANIFEST.json` beside the root's `actingd.config.json`, the
// program read as the root's `runtime\actinglab.exe` (`cli_module::authoring::test_place`).
// - `s18_from_no_installation_without_the_variable_the_default`: the variable not set, the program
//   outside any installation root (a made-up unpacked place, and the test's own place): the default
//   `<LOCALAPPDATA>\ActingCommand\runtime`.
// - `s18_the_variable_wins_wherever_the_program_runs`: the variable set: its path, from no
//   installation, from an installation whose state root is located and from one whose state root
//   cannot be located.
// - `s18_from_an_installation_without_the_variable_its_state_root`: the installation's configured
//   state root.
// - `s18_an_installation_whose_state_root_cannot_be_located_is_refused`: a configuration without
//   `state_root`, and none at all: refused with the located outcome's code
//   (`install_state_root_unresolved`), the CLI module's text and the outcome in the details; the
//   default is not answered.
// - `s18_record_start_from_an_installation_holds_its_instance`: `record start` without the variable
//   from an installation, against the runtime client's scripted Runtime on its state root (as
//   gate_463_s17 runs it): one connection, the handshake, `status`, `pause_scheduling`; the answer's
//   `hold.state` is `own`, with no warning.
// No device, no clock, no sleep; made-up paths and aliases.

use super::gate_434_lab5b4::{Isolated, printed};
use super::*;
use crate::cli_module::{authoring, place};
use actingcommand_contract::{
    OwnerEpoch, RuntimeControlPlaneStatus, RuntimeInstanceStatus, RuntimeRequest, RuntimeResult,
    SchedulingDrainSummary, SchedulingPauseScope,
};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};

/// The made-up instance of the recording.
const INSTANCE: &str = "neutral.a";
const TASK: &str = "neutral_task";
const RECORD_ID: &str = "record_s18";
/// The variable the default state root is read from first.
const LOCAL_APP_DATA: &str = "LOCALAPPDATA";

/// What the made-up installation's configuration says of its state root.
#[derive(Debug, Clone, Copy)]
enum Config {
    /// An absolute `state_root`.
    Located,
    /// A configuration without `state_root`.
    WithoutStateRoot,
    /// No configuration at all.
    Missing,
}

fn text_of(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// A made-up single-location installation root under `base` whose configuration is `config`: the
/// program placed as its `runtime\actinglab.exe`, and the state root a located configuration names.
fn installation(base: &Path, config: Config) -> (PathBuf, PathBuf) {
    let root = base.join("install-root");
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("BUILD-MANIFEST.json"), b"{}\n").unwrap();
    let state_root = base.join("installed-state");
    let document = match config {
        Config::Located => Some(json!({"state_root": text_of(&state_root)})),
        Config::WithoutStateRoot => Some(json!({})),
        Config::Missing => None,
    };
    if let Some(document) = document {
        fs::write(root.join("actingd.config.json"), document.to_string()).unwrap();
    }
    (runtime.join("actinglab.exe"), state_root)
}

/// A program outside any installation root.
fn unrooted(base: &Path) -> PathBuf {
    let unpacked = base.join("unpacked-tools");
    fs::create_dir_all(&unpacked).unwrap();
    unpacked.join("actinglab.exe")
}

/// The state root's variable not set until the guard drops (then as it was).
fn without_variable() -> RuntimeStateEnvGuard {
    let previous = env::var_os(crate::RUNTIME_STATE_ROOT_ENV);
    unsafe {
        env::remove_var(crate::RUNTIME_STATE_ROOT_ENV);
    }
    RuntimeStateEnvGuard { previous }
}

/// `LOCALAPPDATA` named by a test until the guard drops (then as it was).
struct LocalAppData {
    previous: Option<OsString>,
}

impl LocalAppData {
    fn set(path: &Path) -> Self {
        let previous = env::var_os(LOCAL_APP_DATA);
        unsafe {
            env::set_var(LOCAL_APP_DATA, path);
        }
        Self { previous }
    }
}

impl Drop for LocalAppData {
    fn drop(&mut self) {
        unsafe {
            match self.previous.take() {
                Some(previous) => env::set_var(LOCAL_APP_DATA, previous),
                None => env::remove_var(LOCAL_APP_DATA),
            }
        }
    }
}

#[test]
fn s18_from_no_installation_without_the_variable_the_default() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let _unset = without_variable();
    let local = temp.path().join("local");
    let _local = LocalAppData::set(&local);
    let default = local.join("ActingCommand").join("runtime");

    // The test's own place: a build directory, no installation.
    assert_eq!(
        crate::runtime_state_root().expect("the default state root"),
        default
    );
    // A made-up place outside any installation root.
    let _place = authoring::test_place::set(unrooted(temp.path()));
    assert_eq!(
        crate::runtime_state_root().expect("the default state root"),
        default
    );
}

#[test]
fn s18_the_variable_wins_wherever_the_program_runs() {
    let _guard = env_lock();
    let named = TempDir::new().unwrap();
    let _variable = use_runtime_state_root(named.path());
    for config in [None, Some(Config::Located), Some(Config::WithoutStateRoot)] {
        let temp = TempDir::new().unwrap();
        let program = match config {
            Some(made) => installation(temp.path(), made).0,
            None => unrooted(temp.path()),
        };
        let _place = authoring::test_place::set(program);
        assert_eq!(
            crate::runtime_state_root().expect("the variable's state root"),
            named.path(),
            "{config:?}"
        );
    }
}

#[test]
fn s18_from_an_installation_without_the_variable_its_state_root() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let _unset = without_variable();
    let _local = LocalAppData::set(&temp.path().join("local"));
    let (program, state_root) = installation(temp.path(), Config::Located);
    let _place = authoring::test_place::set(program);

    assert_eq!(
        crate::runtime_state_root().expect("the installation's state root"),
        state_root
    );
}

#[test]
fn s18_an_installation_whose_state_root_cannot_be_located_is_refused() {
    let _guard = env_lock();
    let _unset = without_variable();
    for config in [Config::WithoutStateRoot, Config::Missing] {
        let temp = TempDir::new().unwrap();
        let _local = LocalAppData::set(&temp.path().join("local"));
        let (program, _) = installation(temp.path(), config);
        let _place = authoring::test_place::set(program);

        let refused = crate::runtime_state_root()
            .expect_err("an installation whose state root cannot be located is refused");
        assert_eq!(refused.code, "install_state_root_unresolved", "{config:?}");
        assert_eq!(refused.message, place::UNLOCATED, "{config:?}");
        let details = refused.details.expect("the located outcome");
        assert_eq!(
            details["outcome"]["code"], "install_state_root_unresolved",
            "{config:?}: {details}"
        );
    }
}

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

/// The scripted answer to a `Status`: the instance not paused, in `owner_epoch`.
fn status(
    owner_epoch: OwnerEpoch,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let instance =
            RuntimeInstanceStatus::new(INSTANCE, instance_id(), false, 0, false, false, false)
                .expect("instance status");
        let status =
            RuntimeControlPlaneStatus::new(owner_epoch, vec![instance]).expect("scripted status");
        ScriptedAnswer::completed(request, RuntimeResult::Status { status })
    }
}

/// The scripted answer to the instance's `PauseScheduling`: paused at revision 3, nothing drained.
fn paused(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::completed(
        request,
        RuntimeResult::SchedulingPaused {
            scope: SchedulingPauseScope::Instance {
                instance_alias: INSTANCE.to_owned(),
            },
            revision: 3,
            drained: Some(SchedulingDrainSummary {
                finished: 0,
                cancelled: 0,
            }),
        },
    )
}

/// The operation of each request a connection received, as the wire spells it.
fn operations(record: &ScriptedConnectionRecord) -> Vec<String> {
    record
        .requests
        .iter()
        .map(|request| {
            json!(request.operation())["operation"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

#[test]
fn s18_record_start_from_an_installation_holds_its_instance() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _unset = without_variable();
    let (program, state_root) = installation(isolated.temp.path(), Config::Located);
    fs::create_dir_all(&state_root).expect("the installation's state root");
    let _place = authoring::test_place::set(program);
    let runtime = ScriptedRuntime::start(&state_root, |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch))
                .request(paused),
        ]
    });
    let session = isolated.path("session");
    let line = [
        "--json",
        "--instance",
        INSTANCE,
        "record",
        "start",
        "--state-dir",
        session.as_str(),
        "--task-id",
        TASK,
        "--record-id",
        RECORD_ID,
    ];
    let exit = printed(&line);
    let record = runtime.finish();
    let started = exit
        .line
        .clone()
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));

    assert_eq!(exit.code, 0, "{started}");
    assert_eq!(started["hold"]["state"], "own", "{started}");
    assert!(started["hold"].get("warnings").is_none(), "{started}");
    assert!(started["hold"].get("runtime_error").is_none(), "{started}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        ["health", "bind_channel", "status", "pause_scheduling"]
    );
}
