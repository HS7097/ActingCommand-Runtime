// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-1b (contracts/lab-recording.md, "What a recording still lacks: `record ready`"; register
// §1.434): `record ready` takes `--game`, `--server` and `--locale` as `record stop` takes them,
// and, in an installation, reads the instance's game and server from the installation's
// configuration, as `record stop` then builds with them; it says where each part came from
// (`sources`) and writes none of them on the recording. The installation is made up as S1-8's
// tests make theirs (gate_463_s18): a single-location root in a temporary folder,
// `runtime\BUILD-MANIFEST.json` beside the root's `actingd.config.json`, the program read as the
// root's `runtime\actinglab.exe` (`cli_module::authoring::test_place`); its configuration binds
// the instance to a game and a server in `policy.facts.instances`. The recording is made as
// gate_449_rec_arch makes it: three screens through the Lab's recording API (the first two with a
// tap), the session and every mark through the command line (a colour mark on each step). A
// refusal exits 1 on actinglab's own line, whatever its class (`cli_module::own_line`).
// - `s21b_an_installed_instance_takes_game_and_server_from_its_installation`: in the
//   installation, every step marked, no flag: only `locale` is missing, `sources` game and server
//   `installation_config` and locale `null`; with `--locale`: `ready`, nothing missing, every
//   self-check run, locale `command_line`.
// - `s21b_outside_an_installation_the_three_are_missing_until_given`: from no installation, no
//   flag: `game`, `server` and `locale` missing as before, each with the hint that names `record
//   ready` and `record stop`, `sources` all `null`; with the three flags: `ready`, `sources` all
//   `command_line`, on the command line and through `serve --stdio` (the row's parameters, which
//   the Lab's MCP server's `ac_record_ready` sends).
// - `s21b_a_flag_and_the_recording_win_over_the_installation`: in the installation, a recording
//   started with `--game` and `--server`: `sources` `recording` for both and `record stop
//   --dry-run` builds the package id of the recording's; `--game` on both commands: `command_line`
//   and the flag's game. (Fix round 1 moved its third case, a flag's server beside the row's game,
//   to the row taken whole or not at all.)
// - `s21b_a_malformed_value_is_refused_with_record_stops_code`: a game, a server and a locale out
//   of their form are each refused by `record ready` and by `record stop` with one code,
//   `validation_failed`, exit 1.
// - `s21b_record_ready_with_flags_writes_nothing_on_the_recording`: `record ready` with the three
//   flags answers `ready` and leaves every file of the session directory (but the recording lock's
//   own) byte for byte; a `record stop --dry-run` without them is then still refused
//   `record_locale_missing` (exit 1) with the three parts missing.
// - `s21b_record_stop_in_an_installation_builds_with_its_game_and_server`: in the installation,
//   `record stop --locale` without `--game` and `--server` writes the package of the
//   installation's game and server; with `--game` and `--server` the package of the flags.
// Fix round 1:
// - `s21b_an_installation_without_a_usable_row_leaves_game_and_server_missing`: an installation
//   whose configuration cannot be read (a folder in its place), is no JSON, has no row for the
//   instance, or whose row names a server out of its form: `record ready --locale` answers (exit
//   0), `draft`, game and server missing, `sources` game and server `null`.
// - `s21b_the_labs_instance_configuration_wins_over_the_installations`: the Lab's instance
//   configuration binds the instance to another game and server than the installation's row:
//   `sources` `lab_config` for both, and `record stop --dry-run` builds the package of the Lab's.
// - `s21b_the_installations_row_is_taken_whole_or_not_at_all`: the row's game is the recording's:
//   the server comes from the row; the row's server is the flag's: the game comes from it; the
//   recording's game is not the row's, or the flag's server is not the row's: nothing is taken from
//   the row, the other part is missing in `record ready` and `record stop` is refused
//   `record_locale_missing` (exit 1) naming it, as before this slice.
// - `s21b_the_installations_configuration_is_opened_only_when_needed`: with `--game` and
//   `--server`, `record ready` and `record stop --dry-run` call the installation's reader no time
//   (`session_record::installed_reads`); without them, once each.
// - `s21b_record_ready_in_an_installation_writes_nothing_there_or_on_the_recording`: in the
//   installation, `record ready` reading the row and `record ready` with the three flags leave the
//   session directory and the installation root byte for byte; `record stop --dry-run` without
//   flags is then refused `record_locale_missing` with the locale alone missing.
// - `s21b_a_packaged_recording_answers_what_its_package_holds`: a recording stopped with its
//   package: `record ready` without flags and with the package's own values answers `packaged`,
//   `sources` `recording` for the three and no warning; with another game and locale it still
//   answers `packaged` with the package's sources, and warns `record_stop_option_conflict` for each,
//   with the members `record stop --game` refuses it with (exit 1).
// No Runtime (the state root's variable names a folder that does not exist), no device, no clock,
// no sleep; made-up paths, aliases and values.

use super::gate_434_lab5b4::{Isolated, printed};
use super::{env_lock, set_config_env};
use crate::cli_module::{self, authoring, serve};
use crate::commands::session_record::installed_reads;
use crate::write_user_config;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{
    AttachFrameRequest, ClickEffect, CommitClickRequest, InstanceConfig, OpaqueJson,
    PlanClickRequest, RecordRect, RecordingLock, UserConfig, record_attach_frame,
    record_commit_click, record_plan_click,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const INSTANCE: &str = "neutral.a";
const TASK: &str = "neutral_task";
/// The game and the server the made-up installation binds the instance to.
const INSTALLED_GAME: &str = "neutral";
const INSTALLED_SERVER: &str = "test";
const LOCALE: &str = "en-US";
/// The made-up installation's root, below a test's base folder.
const INSTALL_ROOT: &str = "install-root";
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const GRAY: [u8; 3] = [60, 60, 60];
const TAP: RecordRect = RecordRect {
    x: 8,
    y: 2,
    width: 4,
    height: 4,
};
/// The self-checks of `record stop`, in the order they run.
const EVERY_CHECK: [&str; 7] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
];

/// What the made-up installation's configuration is.
#[derive(Debug, Clone, Copy)]
enum Config {
    /// The instance's row binds it to [`INSTALLED_GAME`] on [`INSTALLED_SERVER`].
    Bound,
    /// A folder where the file would be: it cannot be read.
    Unreadable,
    /// Bytes that are no JSON.
    Malformed,
    /// No row of `policy.facts.instances` for the instance (one for another).
    WithoutRow,
    /// The instance's row names a server out of its form.
    MalformedValue,
}

/// A made-up single-location installation root under `base`, as S1-8's tests make one: the program
/// placed as its `runtime\actinglab.exe` beside `runtime\BUILD-MANIFEST.json`, and its
/// `actingd.config.json` naming an absolute state root, the instance and the instance's row of
/// `policy.facts.instances`, which binds it to [`INSTALLED_GAME`] on [`INSTALLED_SERVER`].
fn installation(base: &Path) -> PathBuf {
    installation_with(base, Config::Bound)
}

/// [`installation`] whose configuration is `config`.
fn installation_with(base: &Path, config: Config) -> PathBuf {
    let root = base.join(INSTALL_ROOT);
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).expect("the installation's runtime folder");
    fs::write(runtime.join("BUILD-MANIFEST.json"), b"{}\n").expect("the build manifest");
    let state_root = base.join("installed-state");
    let configured = |row_instance: &str, server: &str| {
        json!({
            "state_root": state_root.to_str().expect("a UTF-8 path"),
            "instances": [{"alias": INSTANCE, "instance_id": "neutral-instance-a"}],
            "policy": {"facts": {"instances": [
                {"instance_id": row_instance, "game_id": INSTALLED_GAME, "server_id": server,
                 "host_id": "neutral-host", "available": true, "capability_operation_ids": [],
                 "preferred_task_ids": []}
            ]}}
        })
        .to_string()
    };
    let file = root.join("actingd.config.json");
    match config {
        Config::Bound => fs::write(&file, configured(INSTANCE, INSTALLED_SERVER)),
        Config::Unreadable => fs::create_dir_all(&file),
        Config::Malformed => fs::write(&file, b"not json"),
        Config::WithoutRow => fs::write(&file, configured("neutral.b", INSTALLED_SERVER)),
        Config::MalformedValue => fs::write(&file, configured(INSTANCE, "no/server")),
    }
    .unwrap_or_else(|error| panic!("the configuration {}: {error}", file.display()));
    runtime.join("actinglab.exe")
}

// A 16x9 frame: a 4x4 square of `color` at the top left on gray.
fn frame_png(color: [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..9 {
        for x in 0..16 {
            pixels.extend(if x < 4 && y < 4 { color } else { GRAY });
        }
    }
    Frame::from_pixels(
        16,
        9,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// What `actinglab --json --instance neutral.a record <action> --state-dir <state> <more>` prints:
// its exit code and its line.
fn record(state: &str, action: &str, more: &[&str]) -> (u8, Value) {
    let mut line = vec![
        "--json",
        "--instance",
        INSTANCE,
        "record",
        action,
        "--state-dir",
        state,
    ];
    line.extend(more);
    let exit = printed(&line);
    let answered = exit
        .line
        .clone()
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (exit.code, answered)
}

fn opaque(value: Value) -> Option<OpaqueJson> {
    Some(OpaqueJson::from_serializable(&value).expect("opaque value"))
}

// A frame showing `color` as `observe --capture --record` attaches it; with `request`, then a tap
// of `TAP` at its centre as `do --capture --record --tap-rect` records a Performed one.
fn step(state: &str, color: [u8; 3], request: Option<&str>) {
    let lock = RecordingLock::acquire(Path::new(state), INSTANCE, "gate_463_s21b", "Lab-CLI")
        .expect("recording lock");
    record_attach_frame(
        &lock,
        AttachFrameRequest {
            png: frame_png(color),
            source: "runtime_observation".to_owned(),
            runtime_artifact: None,
            capture_backend: None,
            freshness: None,
        },
    )
    .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    let Some(request) = request else {
        return;
    };
    let plan = record_plan_click(
        &lock,
        &PlanClickRequest {
            tap_rect: Some(TAP),
            ..PlanClickRequest::default()
        },
    )
    .unwrap_or_else(|error| panic!("plan: {}: {}", error.code, error.message));
    record_commit_click(
        &lock,
        &plan,
        &CommitClickRequest {
            effect: ClickEffect::Performed,
            has_failure: false,
            carrier_package: opaque(json!({"package_label": "neutral.package"})),
            req_id: opaque(json!(request)),
            correlation_id: opaque(json!(format!("{request}-correlation"))),
            action_id: opaque(json!(format!("{request}-action"))),
            lease_id: opaque(json!("lease-0463")),
            failure: None,
            before: opaque(
                json!({"frame_id": format!("{request}-before"), "page": "neutral_page"}),
            ),
            after: opaque(json!({"frame_id": format!("{request}-after"), "page": null})),
        },
    )
    .unwrap_or_else(|error| panic!("tap: {}: {}", error.code, error.message));
}

// A recording `record_id` under `state` with every step marked: started with `start` (the flags
// `record start` is given beside its task and id), three screens, the first two with a tap, and a
// colour mark on each step.
fn marked_recording(state: &str, record_id: &str, start: &[&str]) {
    let mut flags = vec!["--task-id", TASK, "--record-id", record_id];
    flags.extend(start);
    let (code, started) = record(state, "start", &flags);
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["lab_recording"]["status"], "active", "{started}");
    step(state, RED, Some("request-1"));
    step(state, GREEN, Some("request-2"));
    step(state, BLUE, None);
    for (number, id) in [("1", "red"), ("2", "green"), ("3", "blue")] {
        let color = format!("{id}_color=0,0,4,4");
        let (code, marked) = record(
            state,
            "mark",
            &["--step", number, "--color", color.as_str()],
        );
        assert_eq!(code, 0, "{marked}");
    }
}

// `record ready` with `more`, which answers.
fn ready(state: &str, more: &[&str]) -> Value {
    let (code, answered) = record(state, "ready", more);
    assert_eq!(code, 0, "{answered}");
    answered
}

// The `sources` of a `record ready` answer for `game`, `server` and `locale`.
fn sources(game: Option<&str>, server: Option<&str>, locale: Option<&str>) -> Value {
    json!({"game": game, "server": server, "locale": locale})
}

// The item of `missing` for the part `what` not known, `order` in the answer, when the parts
// `parts` are not known.
fn identity_item(order: u32, what: &str, flag: &str, parts: &[&str]) -> Value {
    json!({
        "order": order, "step": null, "artifact_step": null, "frame_id": null, "what": what,
        "code": "record_locale_missing",
        "hint": format!("record ready {flag} to check, record stop {flag} to build"),
        "details": {"missing": parts}
    })
}

// The package id `record stop --dry-run` with `more` would build.
fn dry_run_package_id(state: &str, more: &[&str]) -> Value {
    let mut flags = vec!["--dry-run"];
    flags.extend(more);
    let (code, stopped) = record(state, "stop", &flags);
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["lab"]["status"], "validated", "{stopped}");
    stopped["lab"]["package_id"].clone()
}

// `record stop` with `more`, refused `record_locale_missing` (exit 1, a refusal on actinglab's own
// line): the parts its details name missing.
fn stop_missing(state: &str, more: &[&str]) -> Value {
    let (code, refused) = record(state, "stop", more);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(
        refused["outcome"]["code"], "record_locale_missing",
        "{refused}"
    );
    refused["details"]["missing"].clone()
}

// Every file under `dir`, by its `/`-separated path relative to it, with its bytes, in name order,
// but the recording lock's own files (`record-<instance>.lock` and its holder file).
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = pending.pop() {
        for entry in fs::read_dir(&path).expect("directory") {
            let entry = entry.expect("entry");
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("file type").is_dir() {
                pending.push((entry.path(), format!("{name}/")));
            } else if !name.contains(".lock") {
                files.push((name, fs::read(entry.path()).expect("file")));
            }
        }
    }
    files.sort();
    files
}

// What `serve --stdio` answers to `record ready` with `params` beside the instance and the session
// directory `state`: its one line, as JSON.
fn served_ready(state: &str, mut params: Value) -> Value {
    let table = cli_module::table().expect("the operation table");
    params["instance"] = json!(INSTANCE);
    params["state_dir"] = json!(state);
    let request = json!({"op": "record ready", "params": params});
    let input = format!("{request}\n");
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut developer).expect("the loop ends");
    let output = String::from_utf8(output).expect("UTF-8 output");
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{output}");
    serde_json::from_str(lines[0]).expect("one JSON answer")
}

#[test]
fn s21b_an_installed_instance_takes_game_and_server_from_its_installation() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(installation(isolated.temp.path()));
    let state = isolated.path("session");
    marked_recording(&state, "rec_installed", &[]);

    let answered = ready(&state, &[]);
    assert_eq!(answered["status"], "draft", "{answered}");
    assert_eq!(
        answered["missing"],
        json!([identity_item(1, "locale", "--locale <l>", &["locale"])]),
        "{answered}"
    );
    assert_eq!(answered["missing_total"], 1, "{answered}");
    assert_eq!(answered["checks_after"], json!(EVERY_CHECK), "{answered}");
    assert_eq!(
        answered["sources"],
        sources(
            Some("installation_config"),
            Some("installation_config"),
            None
        ),
        "{answered}"
    );

    let answered = ready(&state, &["--locale", LOCALE]);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(answered["missing"], json!([]), "{answered}");
    assert_eq!(answered["missing_total"], 0, "{answered}");
    assert_eq!(answered["checks_after"], json!([]), "{answered}");
    assert_eq!(
        answered["sources"],
        sources(
            Some("installation_config"),
            Some("installation_config"),
            Some("command_line")
        ),
        "{answered}"
    );
}

#[test]
fn s21b_outside_an_installation_the_three_are_missing_until_given() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    marked_recording(&state, "rec_plain", &[]);

    let answered = ready(&state, &[]);
    let parts = ["game", "server", "locale"];
    assert_eq!(answered["status"], "draft", "{answered}");
    assert_eq!(
        answered["missing"],
        json!([
            identity_item(1, "game", "--game <g>", &parts),
            identity_item(2, "server", "--server <s>", &parts),
            identity_item(3, "locale", "--locale <l>", &parts)
        ]),
        "{answered}"
    );
    assert_eq!(answered["missing_total"], 3, "{answered}");
    assert_eq!(answered["checks_after"], json!(EVERY_CHECK), "{answered}");
    assert_eq!(answered["sources"], sources(None, None, None), "{answered}");

    let given = [
        "--game",
        INSTALLED_GAME,
        "--server",
        INSTALLED_SERVER,
        "--locale",
        LOCALE,
    ];
    let every = Some("command_line");
    let answered = ready(&state, &given);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(answered["missing"], json!([]), "{answered}");
    assert_eq!(answered["checks_after"], json!([]), "{answered}");
    assert_eq!(
        answered["sources"],
        sources(every, every, every),
        "{answered}"
    );

    // The row's parameters, as the Lab's MCP server's `ac_record_ready` sends them through its
    // `serve` child, answer the same.
    let served = served_ready(
        &state,
        json!({"game": INSTALLED_GAME, "server": INSTALLED_SERVER, "locale": LOCALE}),
    );
    assert_eq!(served["status"], "ready", "{served}");
    assert_eq!(served["missing"], json!([]), "{served}");
    assert_eq!(served["sources"], sources(every, every, every), "{served}");
}

#[test]
fn s21b_a_flag_and_the_recording_win_over_the_installation() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(installation(isolated.temp.path()));

    // The recording holds a game and a server: they win over the installation's.
    let held = isolated.path("held");
    marked_recording(&held, "rec_held", &["--game", "held", "--server", "kept"]);
    let answered = ready(&held, &["--locale", LOCALE]);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        answered["sources"],
        sources(Some("recording"), Some("recording"), Some("command_line")),
        "{answered}"
    );
    assert_eq!(
        dry_run_package_id(&held, &["--locale", LOCALE]),
        "held.kept.neutral_task"
    );

    // A flag wins over what the recording holds, and so over the installation's.
    let answered = ready(&held, &["--game", "given", "--locale", LOCALE]);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        answered["sources"],
        sources(
            Some("command_line"),
            Some("recording"),
            Some("command_line")
        ),
        "{answered}"
    );
    assert_eq!(
        dry_run_package_id(&held, &["--game", "given", "--locale", LOCALE]),
        "given.kept.neutral_task"
    );
}

#[test]
fn s21b_a_malformed_value_is_refused_with_record_stops_code() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    marked_recording(&state, "rec_malformed", &[]);

    // A refusal exits 1 on actinglab's own line, whatever its class (`cli_module::own_line`).
    for malformed in [
        ["--game", "no game"],
        ["--server", "no/server"],
        ["--locale", "no locale"],
    ] {
        let (ready_code, refused_ready) = record(&state, "ready", &malformed);
        let (stop_code, refused_stop) = record(&state, "stop", &malformed);
        assert_eq!(ready_code, 1, "{malformed:?}: {refused_ready}");
        assert_eq!(stop_code, 1, "{malformed:?}: {refused_stop}");
        assert_eq!(
            refused_ready["outcome"]["code"], "validation_failed",
            "{malformed:?}: {refused_ready}"
        );
        assert_eq!(
            refused_ready["outcome"]["code"], refused_stop["outcome"]["code"],
            "{malformed:?}: {refused_ready} {refused_stop}"
        );
    }
}

#[test]
fn s21b_record_ready_with_flags_writes_nothing_on_the_recording() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    marked_recording(&state, "rec_unwritten", &[]);
    let before = files_under(Path::new(&state));

    let answered = ready(
        &state,
        &[
            "--game",
            INSTALLED_GAME,
            "--server",
            INSTALLED_SERVER,
            "--locale",
            LOCALE,
        ],
    );
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(files_under(Path::new(&state)), before, "record ready wrote");

    // `record stop` still needs every part `record ready` was given.
    assert_eq!(
        stop_missing(&state, &["--dry-run"]),
        json!(["game", "server", "locale"])
    );
}

#[test]
fn s21b_record_stop_in_an_installation_builds_with_its_game_and_server() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(installation(isolated.temp.path()));

    let installed = isolated.path("installed");
    marked_recording(&installed, "rec_installed", &[]);
    let (code, stopped) = record(&installed, "stop", &["--locale", LOCALE]);
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    assert_eq!(stopped["lab"]["status"], "generated", "{stopped}");
    assert_eq!(
        stopped["lab"]["package_id"], "neutral.test.neutral_task",
        "{stopped}"
    );

    let flagged = isolated.path("flagged");
    marked_recording(&flagged, "rec_flagged", &[]);
    let (code, stopped) = record(
        &flagged,
        "stop",
        &["--game", "given", "--server", "flagged", "--locale", LOCALE],
    );
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["lab"]["status"], "generated", "{stopped}");
    assert_eq!(
        stopped["lab"]["package_id"], "given.flagged.neutral_task",
        "{stopped}"
    );
}

#[test]
fn s21b_an_installation_without_a_usable_row_leaves_game_and_server_missing() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    marked_recording(&state, "rec_unusable", &[]);
    let parts = ["game", "server"];

    for config in [
        Config::Unreadable,
        Config::Malformed,
        Config::WithoutRow,
        Config::MalformedValue,
    ] {
        let base = isolated.temp.path().join(format!("{config:?}"));
        let _place = authoring::test_place::set(installation_with(&base, config));
        // `ready` asserts the exit 0: nothing is refused.
        let answered = ready(&state, &["--locale", LOCALE]);
        assert_eq!(answered["status"], "draft", "{config:?}: {answered}");
        assert_eq!(
            answered["missing"],
            json!([
                identity_item(1, "game", "--game <g>", &parts),
                identity_item(2, "server", "--server <s>", &parts)
            ]),
            "{config:?}: {answered}"
        );
        assert_eq!(
            answered["sources"],
            sources(None, None, Some("command_line")),
            "{config:?}: {answered}"
        );
    }
}

#[test]
fn s21b_the_labs_instance_configuration_wins_over_the_installations() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(installation(isolated.temp.path()));
    let state = isolated.path("session");
    marked_recording(&state, "rec_configured", &[]);
    // The Lab's instance configuration, written after the recording started (which would
    // otherwise hold its game and server), binds the instance to another game and server than the
    // installation's row.
    set_config_env(isolated.temp.path().join("config.json"));
    let mut config = UserConfig::default();
    config.instances.insert(
        INSTANCE.to_owned(),
        InstanceConfig {
            game: Some("configured".to_owned()),
            server: Some("lab".to_owned()),
            ..InstanceConfig::default()
        },
    );
    write_user_config(&config).expect("the Lab's instance configuration");

    let answered = ready(&state, &["--locale", LOCALE]);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        answered["sources"],
        sources(Some("lab_config"), Some("lab_config"), Some("command_line")),
        "{answered}"
    );
    assert_eq!(
        dry_run_package_id(&state, &["--locale", LOCALE]),
        "configured.lab.neutral_task"
    );
}

#[test]
fn s21b_the_installations_row_is_taken_whole_or_not_at_all() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(installation(isolated.temp.path()));

    // The row's game is the recording's: the row gives the server.
    let agreeing = isolated.path("agreeing");
    marked_recording(&agreeing, "rec_agreeing", &["--game", INSTALLED_GAME]);
    let answered = ready(&agreeing, &["--locale", LOCALE]);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        answered["sources"],
        sources(
            Some("recording"),
            Some("installation_config"),
            Some("command_line")
        ),
        "{answered}"
    );
    assert_eq!(
        dry_run_package_id(&agreeing, &["--locale", LOCALE]),
        "neutral.test.neutral_task"
    );

    // The row's server is the flag's: the row gives the game.
    let plain = isolated.path("plain");
    marked_recording(&plain, "rec_plain", &[]);
    let given = ["--server", INSTALLED_SERVER, "--locale", LOCALE];
    let answered = ready(&plain, &given);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        answered["sources"],
        sources(
            Some("installation_config"),
            Some("command_line"),
            Some("command_line")
        ),
        "{answered}"
    );
    assert_eq!(
        dry_run_package_id(&plain, &given),
        "neutral.test.neutral_task"
    );

    // The row binds another game than the recording's: nothing is taken from it, the server is
    // missing, and `record stop` refuses as it did before this slice.
    let other = isolated.path("other");
    marked_recording(&other, "rec_other", &["--game", "held"]);
    let answered = ready(&other, &["--locale", LOCALE]);
    assert_eq!(answered["status"], "draft", "{answered}");
    assert_eq!(
        answered["missing"],
        json!([identity_item(1, "server", "--server <s>", &["server"])]),
        "{answered}"
    );
    assert_eq!(
        answered["sources"],
        sources(Some("recording"), None, Some("command_line")),
        "{answered}"
    );
    assert_eq!(
        stop_missing(&other, &["--locale", LOCALE]),
        json!(["server"])
    );

    // The row binds another server than the flag's: the game is missing.
    let given = ["--server", "given", "--locale", LOCALE];
    let answered = ready(&plain, &given);
    assert_eq!(answered["status"], "draft", "{answered}");
    assert_eq!(
        answered["missing"],
        json!([identity_item(1, "game", "--game <g>", &["game"])]),
        "{answered}"
    );
    assert_eq!(
        answered["sources"],
        sources(None, Some("command_line"), Some("command_line")),
        "{answered}"
    );
    assert_eq!(stop_missing(&plain, &given), json!(["game"]));
}

#[test]
fn s21b_the_installations_configuration_is_opened_only_when_needed() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(installation(isolated.temp.path()));
    let state = isolated.path("session");
    marked_recording(&state, "rec_lazy", &[]);
    let given = [
        "--game",
        INSTALLED_GAME,
        "--server",
        INSTALLED_SERVER,
        "--locale",
        LOCALE,
    ];

    // The flags give both: neither command opens the installation's configuration.
    let before = installed_reads::count();
    let every = Some("command_line");
    let answered = ready(&state, &given);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        answered["sources"],
        sources(every, every, every),
        "{answered}"
    );
    assert_eq!(
        dry_run_package_id(&state, &given),
        "neutral.test.neutral_task"
    );
    assert_eq!(
        installed_reads::count(),
        before,
        "the configuration was opened"
    );

    // Neither gives them: each opens it once.
    let answered = ready(&state, &["--locale", LOCALE]);
    assert_eq!(
        answered["sources"],
        sources(
            Some("installation_config"),
            Some("installation_config"),
            Some("command_line")
        ),
        "{answered}"
    );
    assert_eq!(installed_reads::count(), before + 1);
    assert_eq!(
        dry_run_package_id(&state, &["--locale", LOCALE]),
        "neutral.test.neutral_task"
    );
    assert_eq!(installed_reads::count(), before + 2);
}

#[test]
fn s21b_record_ready_in_an_installation_writes_nothing_there_or_on_the_recording() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(installation(isolated.temp.path()));
    let root = isolated.temp.path().join(INSTALL_ROOT);
    let state = isolated.path("session");
    marked_recording(&state, "rec_unwritten", &[]);
    let session_before = files_under(Path::new(&state));
    let root_before = files_under(&root);

    // The installation's row read (no flag for the game and the server), then every part given.
    let answered = ready(&state, &["--locale", LOCALE]);
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        answered["sources"]["game"], "installation_config",
        "{answered}"
    );
    let answered = ready(
        &state,
        &[
            "--game",
            INSTALLED_GAME,
            "--server",
            INSTALLED_SERVER,
            "--locale",
            LOCALE,
        ],
    );
    assert_eq!(answered["status"], "ready", "{answered}");
    assert_eq!(
        files_under(Path::new(&state)),
        session_before,
        "record ready wrote on the recording"
    );
    assert_eq!(
        files_under(&root),
        root_before,
        "record ready wrote in the installation"
    );

    // `record stop` still needs the locale `record ready` was given; the game and the server come
    // from the installation.
    assert_eq!(stop_missing(&state, &["--dry-run"]), json!(["locale"]));
}

#[test]
fn s21b_a_packaged_recording_answers_what_its_package_holds() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    marked_recording(&state, "rec_packaged", &[]);
    let built = [
        "--game",
        INSTALLED_GAME,
        "--server",
        INSTALLED_SERVER,
        "--locale",
        LOCALE,
    ];
    let (code, stopped) = record(&state, "stop", &built);
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["lab"]["status"], "generated", "{stopped}");
    let held = Some("recording");

    // Without a flag, and with the package's own values: what the package holds, no warning.
    let without: [&str; 0] = [];
    for more in [&without[..], &built[..]] {
        let answered = ready(&state, more);
        assert_eq!(answered["status"], "packaged", "{more:?}: {answered}");
        assert_eq!(answered["missing"], json!([]), "{more:?}: {answered}");
        assert_eq!(answered["warnings"], json!([]), "{more:?}: {answered}");
        assert_eq!(
            answered["sources"],
            sources(held, held, held),
            "{more:?}: {answered}"
        );
    }

    // Another game and another locale: still what the package holds, and each one a warning with
    // the code and the details `record stop` refuses it with.
    let answered = ready(&state, &["--game", "other", "--locale", "en-GB"]);
    assert_eq!(answered["status"], "packaged", "{answered}");
    assert_eq!(answered["sources"], sources(held, held, held), "{answered}");
    assert_eq!(
        answered["warnings"],
        json!([
            {"code": "record_stop_option_conflict", "field": "game",
             "recorded": INSTALLED_GAME, "given": "other"},
            {"code": "record_stop_option_conflict", "field": "locale",
             "recorded": LOCALE, "given": "en-GB"}
        ]),
        "{answered}"
    );
    let (code, refused) = record(&state, "stop", &["--game", "other"]);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(
        refused["outcome"]["code"], "record_stop_option_conflict",
        "{refused}"
    );
    assert_eq!(
        refused["details"],
        json!({"field": "game", "recorded": INSTALLED_GAME, "given": "other"}),
        "{refused}"
    );
}
