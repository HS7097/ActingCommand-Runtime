// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401, slice DA-4b-D in the host (Alice's ruling 69 of 2026-10-26: 「只有安全区适配
//! 不做，但是需要留出来声明式的安全区接口，也就是说可以依照声明的文件直接切换安全区形态」, 「一份文件对应一个机型或多个机型，
//! 并且修改设置后实时生效」; coordinator work order `rd-1026/brief-da4bd.md`, tests 1 to 4;
//! `contracts/display-adapter.md` section 9): the declared safe area.
//!
//! Declaration files in the state root's `display-safe-area/` (the default
//! `display_safe_area.directory`) declare invented device models; the instance declares its model
//! by `instance.<id>.display_safe_area.model`. Every host reads the directory at every frame
//! (`display_safe_area.check_interval_ms` 0). The insets a frame takes are those its geometry
//! record's frame carries (`frame.producer.safe_insets`), and a map they move names its insets'
//! source `declared`. Test 4's handset without a declaration is DA-2a's adapted test
//! `da2a_a_safe_area_declaration_finds_no_insets_before_da_4b`.
//!
//! The device's frames and display are 720x1280 at rotation `r0` and show `home` (red) until the
//! run's one input and `terminal` (blue) after it, in the frame's 2x2 top-left block; the package
//! is DA-2a's 720x1280 navigable route.

use super::gate_392_da2a::{ALIAS, HOME, Run, TERMINAL, number, pack_root, package_at, run, text};
use super::gate_398_rd1b::{bound, resolved_handset};
use super::*;
use actingcommand_contract::{
    CaptureGeometryObservation, CaptureInsets, ConfigSettingChange, DisplayArea,
    DisplayInsetsSource, DisplayMapKind, DisplayScaleBy, RuntimeConfigManifest,
    TaskGeometryConclusion, TaskGeometryDisplayMap, TaskGeometryObservation, TaskGeometryPhase,
    TaskGeometryRecheckTrigger,
};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

/// The handset's neutral fake serial, distinct from the other test modules'.
/// How long a test waits for a step of the run it drives (test plan v2.2 §2.1 guard 3).
const GATE_WAIT: Duration = Duration::from_secs(60);
const SERIAL: &str = "TESTSERIAL0392";
const MODEL_A: &str = "neutral-model-a";
const MODEL_B: &str = "neutral-model-b";
const MODEL_C: &str = "neutral-model-c";
/// The device-state sampler's interval and fact lifetime: an hour, so no sample reads the
/// handset while a test runs.
const QUIET_MS: u64 = 3_600_000;
const WIDTH: u32 = 720;
const HEIGHT: u32 = 1280;

/// What drives the host's one instance.
#[derive(Clone, Copy)]
enum Device {
    /// An emulator's endpoint.
    Emulator,
    /// A bound handset.
    Handset,
}

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("an instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The instance's own model key, `instance.<id>.display_safe_area.model`.
fn model_key(instance: InstanceId) -> String {
    format!(
        "instance.{}.display_safe_area.model",
        instance_text(instance)
    )
}

/// The configuration manifest actingd assembles over the defaults file for the one instance
/// `instance`, whose entry declares `model` when given: the directory read at every frame and a
/// quiet device-state sampler.
fn manifest(instance: InstanceId, model: Option<&str>) -> RuntimeConfigManifest {
    let id = instance_text(instance);
    let mut entry = json!({ "instance_id": id.clone() });
    if let Some(model) = model {
        entry["display_safe_area"] = json!({ "model": model });
    }
    let file = json!({
        "device_state": {"sample_interval_ms": QUIET_MS, "fact_ttl_ms": QUIET_MS},
        "display_safe_area": {"check_interval_ms": 0},
        "instances": [entry],
    });
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[id], &defaults, &file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A host whose one instance is `device`, declaring `model` when given, reading the standard
/// packs of `root`. Its frames show the page (see the module header); `after_input` runs once,
/// inside the first capture after the run's input, before that frame is delivered.
fn device_host(
    temp: &TempDir,
    root: &Path,
    device: Device,
    model: Option<&str>,
    after_input: Box<dyn Fn() + Send + Sync>,
) -> (RuntimeHost, Arc<FakeState>, InstanceId) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((WIDTH, HEIGHT));
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let row = WIDTH as usize;
    let hooked = AtomicBool::new(false);
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            let page = if inputs > 0 {
                if !hooked.swap(true, Ordering::AcqRel) {
                    after_input();
                }
                TERMINAL
            } else {
                HOME
            };
            [0, 1, row, row + 1]
                .into_iter()
                .map(|index| (index, page))
                .collect()
        },
    ));
    let registered = instance_id();
    let provider = FakeProvider::one(ALIAS, registered, Arc::clone(&state));
    let provider = match device {
        Device::Handset => {
            state.observed_open.store(true, Ordering::Release);
            provider.with_resolved_for(
                ALIAS,
                resolved_handset(ALIAS, registered, bound(ALIAS, SERIAL)),
            )
        }
        Device::Emulator => {
            let resolved = ResolvedExecutionInstance::new(registered, "127.0.0.1:16384")
                .with_adb_endpoint(actingcommand_execution_kernel::ResolvedAdbEndpoint::new(
                    "127.0.0.1",
                    16384,
                    false,
                ));
            provider.with_resolved_override(Arc::new(std::sync::Mutex::new(resolved)))
        }
    };
    let host = RuntimeHost::start(
        config(temp)
            .with_standard_packs_root(Some(root.to_path_buf()))
            .with_config_manifest(manifest(registered, model)),
        Arc::new(provider),
    )
    .expect("runtime host with a device instance");
    (host, state, registered)
}

/// The game's display declaration: the scale by the width of `scale_area`, the anchors on
/// `anchor_area`.
fn layout(scale_area: &str, anchor_area: &str) -> Value {
    json!({
        "schema_version": "actingcommand.display-layout.v1",
        "game": "neutral",
        "scale": {"by": "width", "area": scale_area},
        "anchor_area": anchor_area,
        "correction": {"radius_px": 16}
    })
}

/// A declaration file of `models` with one form: the display `(width, height)` at `r0` and the
/// insets `[left, top, right, bottom]`.
fn declaration(models: &[&str], (width, height): (u32, u32), insets: [u32; 4]) -> Value {
    let [left, top, right, bottom] = insets;
    json!({
        "schema_version": "actingcommand.display-safe-area.v1",
        "models": models,
        "forms": [{
            "display": {"width": width, "height": height},
            "rotation": "r0",
            "insets": {"left": left, "top": top, "right": right, "bottom": bottom}
        }]
    })
}

/// Writes `declaration` as `<state root>/display-safe-area/<name>` and returns its path.
fn write_declaration(temp: &TempDir, name: &str, declaration: &Value) -> PathBuf {
    let directory = temp.path().join("display-safe-area");
    fs::create_dir_all(&directory).expect("the declaration directory");
    let path = directory.join(name);
    fs::write(
        &path,
        serde_json::to_vec_pretty(declaration).expect("declaration JSON"),
    )
    .expect("write the declaration");
    path
}

fn configure(host: &RuntimeHost, key: String, value: Value) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ConfigureSettings {
                    expected_version: None,
                    changes: vec![ConfigSettingChange {
                        key,
                        value: Some(value),
                    }],
                },
            ),
            ConnectionId::new(392).expect("connection"),
        )
        .expect("configure receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

const fn insets(left: u32, top: u32, right: u32, bottom: u32) -> CaptureInsets {
    CaptureInsets {
        left,
        top,
        right,
        bottom,
    }
}

/// The insets the frame of a geometry record carried: its producer's geometry.
fn frame_insets(record: &TaskGeometryObservation) -> Option<CaptureInsets> {
    match record.frame.as_ref().map(|frame| &frame.producer) {
        Some(CaptureGeometryObservation::Observed(geometry)) => geometry.safe_insets,
        _ => None,
    }
}

fn assert_completed(run: &Run, case: &str) {
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Completed,
        "{case}: {:?}",
        run.receipt
    );
    assert!(
        matches!(
            run.receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                final_page: Some(page),
                ..
            }) if page == "neutral/terminal"
        ),
        "{case}: {:?}",
        run.receipt
    );
}

/// Test 1 (Alice's ruling 69: one file for one or several device models): a file that declares
/// two models gives its insets to an instance that declares either. On a handset whose game
/// measures the scale on the safe area (by width; the insets leave the width, so the map stays
/// the identity), each instance's every frame carries the file's insets, top 40, and its run
/// completes on its target page.
#[test]
fn da4bd_t1_a_file_of_two_models_applies_to_an_instance_that_declares_either() {
    for model in [MODEL_A, MODEL_B] {
        let temp = TempDir::new().expect("tempdir");
        let root = pack_root(&temp, &layout("safe_area", "screen"));
        write_declaration(
            &temp,
            "phones.json",
            &declaration(&[MODEL_A, MODEL_B], (WIDTH, HEIGHT), [0, 40, 0, 0]),
        );
        let (host, state, _) =
            device_host(&temp, &root, Device::Handset, Some(model), Box::new(|| {}));
        let run = run(&host, &temp, "two-models", &package_at(WIDTH, HEIGHT));
        host.close().expect("close runtime host");
        assert_completed(&run, model);
        assert_eq!(state.input_count.load(Ordering::Acquire), 1, "{model}");
        let geometry = run.geometry();
        assert!(!geometry.is_empty(), "{model}: a geometry record");
        for record in &geometry {
            assert_eq!(record.conclusion, TaskGeometryConclusion::Pass, "{model}");
            assert_eq!(
                frame_insets(record),
                Some(insets(0, 40, 0, 0)),
                "{model}: {record:?}"
            );
        }
    }
}

/// Test 2's expected records: the run's one input on the first frame's map (insets 0, the plain
/// identity, so the record names no map), then the first frame after the input recorded `changed`
/// with trigger `insets_changed`: its frame carries top 40 and its rebuilt map is the translation
/// they make (the anchors on the safe area, 1/1, the 720x1280 scale space and base, the insets
/// declared). The run's later end (a translation waits for DA-2b) is not what this pins.
fn assert_changed_at_the_next_frame(run: &Run, state: &FakeState) {
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        1,
        "the run's one input, on the first map"
    );
    let geometry = run.geometry();
    let [initial, changed, ..] = geometry.as_slice() else {
        panic!("an initial and a changed record: {geometry:?}");
    };
    assert_eq!(initial.phase, TaskGeometryPhase::Initial);
    assert_eq!(initial.conclusion, TaskGeometryConclusion::Pass);
    assert_eq!(frame_insets(initial), Some(insets(0, 0, 0, 0)));
    assert_eq!(initial.display_map, None, "the plain identity");
    assert_eq!(changed.phase, TaskGeometryPhase::Changed);
    assert_eq!(
        changed.trigger,
        Some(TaskGeometryRecheckTrigger::InsetsChanged)
    );
    assert_eq!(changed.conclusion, TaskGeometryConclusion::Pass);
    assert_eq!(frame_insets(changed), Some(insets(0, 40, 0, 0)));
    assert_eq!(
        changed.display_map,
        Some(TaskGeometryDisplayMap {
            kind: DisplayMapKind::Translate,
            scale_num: 1,
            scale_den: 1,
            scale_width: WIDTH,
            scale_height: HEIGHT,
            base_width: WIDTH,
            base_height: HEIGHT,
            scale_by: DisplayScaleBy::Width,
            scale_area: DisplayArea::Screen,
            anchor_area: DisplayArea::SafeArea,
            insets_source: Some(DisplayInsetsSource::Declared),
            declared: true,
        })
    );
}

/// Test 2, the file (ruling 69: 修改设置后实时生效): editing the instance's declaration file while
/// its run runs changes the map from the next frame, with a `changed` record, and no restart. The
/// file declares insets 0 for the run's first frames; inside the first capture after the input it
/// is rewritten to top 40.
#[test]
fn da4bd_t2_editing_the_declaration_changes_the_map_at_the_next_frame() {
    let temp = TempDir::new().expect("tempdir");
    let root = pack_root(&temp, &layout("screen", "safe_area"));
    let path = write_declaration(
        &temp,
        "phone.json",
        &declaration(&[MODEL_A], (WIDTH, HEIGHT), [0, 0, 0, 0]),
    );
    let edited =
        serde_json::to_vec_pretty(&declaration(&[MODEL_A], (WIDTH, HEIGHT), [0, 40, 0, 0]))
            .expect("declaration JSON");
    let (host, state, _) = device_host(
        &temp,
        &root,
        Device::Handset,
        Some(MODEL_A),
        Box::new(move || fs::write(&path, &edited).expect("edit the declaration")),
    );
    let run = run(&host, &temp, "edited", &package_at(WIDTH, HEIGHT));
    host.close().expect("close runtime host");
    assert_changed_at_the_next_frame(&run, &state);
}

/// Test 2, the setting: changing the instance's model setting while its run runs changes the map
/// from the next frame, with a `changed` record, and no restart. Model A's file declares insets
/// 0, model B's top 40; the first capture after the input waits while the test sets the
/// instance's model to B through `ConfigureSettings`.
#[test]
fn da4bd_t2_changing_the_model_setting_changes_the_map_at_the_next_frame() {
    let temp = TempDir::new().expect("tempdir");
    let root = pack_root(&temp, &layout("screen", "safe_area"));
    write_declaration(
        &temp,
        "a.json",
        &declaration(&[MODEL_A], (WIDTH, HEIGHT), [0, 0, 0, 0]),
    );
    write_declaration(
        &temp,
        "b.json",
        &declaration(&[MODEL_B], (WIDTH, HEIGHT), [0, 40, 0, 0]),
    );
    let (reached, reached_rx) = mpsc::channel::<()>();
    let (proceed, proceed_rx) = mpsc::channel::<()>();
    let proceed_rx = std::sync::Mutex::new(proceed_rx);
    let (host, state, instance) = device_host(
        &temp,
        &root,
        Device::Handset,
        Some(MODEL_A),
        Box::new(move || {
            reached
                .send(())
                .expect("the test waits for the first capture after the input");
            proceed_rx
                .lock()
                .expect("the setting's signal")
                .recv_timeout(GATE_WAIT)
                .expect("the test set the model");
        }),
    );
    let switched = thread::scope(|scope| {
        let running = scope.spawn(|| run(&host, &temp, "switched", &package_at(WIDTH, HEIGHT)));
        reached_rx
            .recv_timeout(GATE_WAIT)
            .expect("the first capture after the input");
        configure(&host, model_key(instance), json!(MODEL_B));
        proceed.send(()).expect("the capture waits for the setting");
        running.join().expect("the run's thread")
    });
    host.close().expect("close runtime host");
    assert_changed_at_the_next_frame(&switched, &state);
}

/// One refusal of test 3: the run fails `display_map_unresolved` with `display_problem` `problem`
/// at stage `display_map_build`, with the frame's size; its outcome is returned.
fn refused<'a>(run: &'a Run, problem: &str) -> &'a actingcommand_contract::outcome::Outcome {
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{problem}: {:?}",
        run.receipt
    );
    let outcome = run.receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "display_map_unresolved",
        "{problem}: {outcome:?}"
    );
    assert_eq!(
        text(outcome, "display_problem"),
        Some(problem),
        "{outcome:?}"
    );
    assert_eq!(
        text(outcome, "stage"),
        Some("display_map_build"),
        "{outcome:?}"
    );
    assert_eq!(number(outcome, "frame_width"), Some(u64::from(WIDTH)));
    assert_eq!(number(outcome, "frame_height"), Some(u64::from(HEIGHT)));
    outcome
}

fn ends_with(outcome: &actingcommand_contract::outcome::Outcome, key: &str, tail: &str) -> bool {
    text(outcome, key).is_some_and(|value| value.ends_with(tail))
}

/// Test 3 (fail loud, #420 S4: one domain code with values): on a handset whose game measures on
/// the safe area, each of these ends the run at its first frame with no input,
/// `display_map_unresolved` at stage `display_map_build` with its `display_problem`, and the next
/// run, after the files or the setting change, reads them again without a restart:
///
/// - a malformed file: `safe_area_declaration_invalid`, its path and the JSON pointer of the
///   missing member;
/// - one model in two files: `safe_area_model_repeated`, the later file and the earlier one;
/// - a declared model no file covers: `safe_area_model_undeclared`, the setting and the directory;
/// - a file that covers no size or orientation the device shows: `safe_area_form_undeclared`, the
///   file.
#[test]
fn da4bd_t3_each_broken_declaration_refuses_the_run_with_its_problem() {
    let temp = TempDir::new().expect("tempdir");
    let root = pack_root(&temp, &layout("safe_area", "screen"));
    let mut malformed = declaration(&[MODEL_A], (WIDTH, HEIGHT), [0, 40, 0, 0]);
    assert!(
        malformed["forms"][0]["insets"]
            .as_object_mut()
            .expect("the insets")
            .remove("bottom")
            .is_some(),
        "the malformed file leaves out the bottom inset"
    );
    write_declaration(&temp, "a.json", &malformed);
    let (host, state, instance) = device_host(
        &temp,
        &root,
        Device::Handset,
        Some(MODEL_A),
        Box::new(|| {}),
    );
    let package = package_at(WIDTH, HEIGHT);

    let broken = run(&host, &temp, "malformed", &package);

    write_declaration(
        &temp,
        "a.json",
        &declaration(&[MODEL_A], (WIDTH, HEIGHT), [0, 40, 0, 0]),
    );
    write_declaration(
        &temp,
        "b.json",
        &declaration(&[MODEL_A], (WIDTH, HEIGHT), [0, 40, 0, 0]),
    );
    let repeated = run(&host, &temp, "repeated", &package);

    write_declaration(
        &temp,
        "b.json",
        &declaration(&[MODEL_B], (1000, 2000), [0, 80, 0, 0]),
    );
    configure(&host, model_key(instance), json!(MODEL_C));
    let undeclared = run(&host, &temp, "undeclared", &package);

    configure(&host, model_key(instance), json!(MODEL_B));
    let uncovered = run(&host, &temp, "uncovered", &package);
    host.close().expect("close runtime host");

    let outcome = refused(&broken, "safe_area_declaration_invalid");
    assert!(ends_with(outcome, "path", "a.json"), "{outcome:?}");
    assert_eq!(
        text(outcome, "json_pointer"),
        Some("/forms/0/insets/bottom"),
        "{outcome:?}"
    );
    let outcome = refused(&repeated, "safe_area_model_repeated");
    assert!(ends_with(outcome, "path", "b.json"), "{outcome:?}");
    assert!(ends_with(outcome, "other_path", "a.json"), "{outcome:?}");
    let outcome = refused(&undeclared, "safe_area_model_undeclared");
    assert_eq!(
        text(outcome, "config_key"),
        Some(model_key(instance).as_str()),
        "{outcome:?}"
    );
    assert!(
        ends_with(outcome, "path", "display-safe-area"),
        "{outcome:?}"
    );
    let outcome = refused(&uncovered, "safe_area_form_undeclared");
    assert!(ends_with(outcome, "path", "b.json"), "{outcome:?}");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
}

/// Test 4 (ruling 69: an emulator needs no file; a `screen` layout is never affected by any
/// declaration). An emulator that declares no model, whose game measures on the safe area, takes
/// insets 0: its every frame carries them and its run completes. A handset whose model is declared
/// (top 40), whose game measures on the screen, takes none: no frame carries insets, no record
/// names a map, and its run completes.
#[test]
fn da4bd_t4_an_emulator_takes_insets_0_and_a_screen_layout_no_declaration() {
    let temp = TempDir::new().expect("tempdir");
    let root = pack_root(&temp, &layout("safe_area", "screen"));
    let (host, state, _) = device_host(&temp, &root, Device::Emulator, None, Box::new(|| {}));
    let emulator = run(&host, &temp, "emulator", &package_at(WIDTH, HEIGHT));
    host.close().expect("close runtime host");
    assert_completed(&emulator, "emulator");
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let geometry = emulator.geometry();
    assert!(!geometry.is_empty(), "a geometry record");
    for record in &geometry {
        assert_eq!(record.conclusion, TaskGeometryConclusion::Pass);
        assert_eq!(frame_insets(record), Some(insets(0, 0, 0, 0)), "{record:?}");
    }

    let temp = TempDir::new().expect("tempdir");
    let root = pack_root(&temp, &layout("screen", "screen"));
    write_declaration(
        &temp,
        "phone.json",
        &declaration(&[MODEL_A], (WIDTH, HEIGHT), [0, 40, 0, 0]),
    );
    let (host, state, _) = device_host(
        &temp,
        &root,
        Device::Handset,
        Some(MODEL_A),
        Box::new(|| {}),
    );
    let screen = run(&host, &temp, "screen", &package_at(WIDTH, HEIGHT));
    host.close().expect("close runtime host");
    assert_completed(&screen, "screen");
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let geometry = screen.geometry();
    assert!(!geometry.is_empty(), "a geometry record");
    for record in &geometry {
        assert_eq!(record.conclusion, TaskGeometryConclusion::Pass);
        assert_eq!(frame_insets(record), None, "{record:?}");
        assert_eq!(record.display_map, None, "{record:?}");
    }
}
