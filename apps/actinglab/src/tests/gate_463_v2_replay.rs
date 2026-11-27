// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V2 (0.12.61; register §1.382; contracts/package-variables.md, "Runs without a
// dispatch"; contracts/remedies.md section 4): `actinglab replay --select <operation> --scan
// <file>` takes `--variable <name>=<json>`, the counterpart of `--parameter`, one flag per
// distinct name, whose value the declaration of a variable the package declares must take as
// given. Anything else is refused with `validation_failed` (exit 2) naming `--variable` and its
// `argument_problem`, never ignored: a name the package does not declare (a package that declares
// none declares no name), a value the declaration does not take (an option it does not offer,
// another type) and a value that is not `<name>=<json>` are `not_recognised`, a name given twice
// is `repeated`; until Workflow #455 TK-1 (0.12.89, register §1.532) `--variable` on a whole
// replay was `conflicting`, and since it the whole replay reads it by the same parser and check. A
// value the declaration takes passes the argument check. Packages and frames are synthetic and
// neutral (`gate_lab_rp`'s, with a variable in place of the parameter), and no Runtime or device
// is involved.

use super::*;
use actingcommand_contract::{FactContent, FactValue, InstanceFactContext, InstanceFactSnapshot};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

/// A context with no configuration and no recording session, and a directory for the packages,
/// the frame and the scan declaration. The fields drop in order, the environment lock last.
struct Isolated {
    files: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Isolated {
    fn new() -> Self {
        let guard = env_lock();
        let app_env = set_isolated_app_env();
        set_missing_config_env();
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
        }
        Self {
            _guard: guard,
            _app_env: app_env,
            files: TempDir::new().unwrap(),
        }
    }

    /// A directory holding one 2 x 1 frame of the `home` page.
    fn frames(&self) -> String {
        let directory = self.files.path().join("frames");
        fs::create_dir(&directory).unwrap();
        let png = Frame::from_pixels(
            2,
            1,
            [[255, 0, 0], [0, 255, 0]].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG");
        fs::write(directory.join("0001.png"), png).unwrap();
        directory.display().to_string()
    }

    /// A scan declaration of one column of cards scrolled vertically, as `gate_lab_rp`'s.
    fn scan(&self) -> String {
        let path = self.files.path().join("scan.json");
        let scan = json!({
            "identity_feature": "member",
            "axis": "vertical",
            "advance": {"kind": "drag",
                        "from": {"x": 20, "y": 60, "width": 20, "height": 10},
                        "to": {"x": 20, "y": 20, "width": 20, "height": 10},
                        "duration_ms": 300, "min_content_step_px": 12,
                        "max_content_step_px": 40},
            "reposition": {"kind": "drag",
                           "from": {"x": 20, "y": 70, "width": 20, "height": 10},
                           "to": {"x": 20, "y": 10, "width": 20, "height": 10},
                           "duration_ms": 300, "max_content_step_px": 60},
            "settle": {"interval_ms": 1, "timeout_ms": 1000},
            "track": {"area": {"x": 0, "y": 4, "width": 64, "height": 92},
                      "edge_margin_px": 2,
                      "signature": {"rect": {"x": 8, "y": 0, "width": 4, "height": 4},
                                    "columns": 1, "rows": 1},
                      "signature_max_distance_milli": 500,
                      "position_tolerance_px": 1,
                      "min_overlap_instances": 2,
                      "spacing": {"max_axis_gap_px": 20, "columns": 1,
                                  "max_trailing_gap_px": 100, "max_cross_lead_px": 10,
                                  "max_cross_gap_px": 10}},
            "start": {"check": "page/home", "rewind": false},
            "end": {"check": "page/result"},
            "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 60_000}
        });
        fs::write(&path, serde_json::to_vec(&scan).unwrap()).unwrap();
        path.display().to_string()
    }

    /// The package `name`, whose select step `pick` chooses one of two slots on `home`. With
    /// `declares` it declares the variable `pick` (a `choice` of `opt_1` and `opt_2`, default
    /// `opt_2`) and its policy reads `session.dispatch.parameter.pick`; without, it declares no
    /// variable and its policy reads none.
    fn package(&self, name: &str, declares: bool) -> String {
        let colour = |id: &str, x: u32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let slot = |x: u32, target: &str| {
            json!({"rect": {"x": x, "y": 0, "width": 1, "height": 1},
                   "click": {"x": x, "y": 0, "width": 1, "height": 1},
                   "targets": {"first": target}})
        };
        let layout = json!({
            "id": "choices",
            "page_id": "neutral/home",
            "kind": "fixed_slots",
            "features": [{"name": "first", "value": "passed"}],
            "slots": [slot(0, "slot/first"), slot(1, "slot/second")]
        });
        let first = json!({"kind": "boolean_equals",
                           "value": {"source": "field", "field": "first"}, "expected": true});
        let (facts, predicate) = if declares {
            let variable = json!({"source": "fact",
                                  "fact_key": "session.dispatch.parameter.pick"});
            (
                json!([{"fact_key": "session.dispatch.parameter.pick",
                        "value_type": {"type": "enum_string", "allowed": ["opt_1", "opt_2"]},
                        "max_age_ms": 86_400_000, "minimum_confidence_milli": 1000}]),
                json!({"kind": "all", "of": [
                    {"kind": "string_in", "value": variable, "allowed": ["opt_1", "opt_2"]},
                    first
                ]}),
            )
        } else {
            (json!([]), first)
        };
        let mut control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": "neutral.test.variable",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 5000,
            "timeout_ms": 20000,
            "max_steps": 2
        });
        if declares {
            control["variables"] = json!([{
                "name": "pick", "type": "choice", "title": {"en": "Option"},
                "options": [{"value": "opt_1", "title": {"en": "Option 1"}},
                            {"value": "opt_2", "title": {"en": "Option 2"}}],
                "default": "opt_2"
            }]);
        }
        let bytes = zip_entries(&[
            ("control.json", control),
            (
                "resources/manifest.json",
                json!({"schema_version": "0.3", "entry_task_id": "task"}),
            ),
            (
                "resources/operations/task/task.json",
                json!({
                    "schema_version": "0.9",
                    "task_id": "task",
                    "game": "neutral",
                    "server_scope": ["test"],
                    "coordinate_space": {"width": 2, "height": 1},
                    "target_page": "result",
                    "candidate_layouts": [layout.clone()],
                    "operations": [{
                        "id": "pick",
                        "from": "home",
                        "to": "result",
                        "expect_after": {"page_id": "result"},
                        "guard": {
                            "page_id": "home",
                            "target_id": "guard/ready",
                            "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                            "color_probe": "guard/ready"
                        },
                        "select": {"layout_id": "choices",
                                   "policy": {"path": "policies/choice.json"}},
                        "retryable": false
                    }]
                }),
            ),
            (
                "resources/operations/task/policies/choice.json",
                json!({
                    "schema_version": "actingcommand.selection-policy.v1",
                    "policy_id": "choice",
                    "applies_to": {
                        "candidate_layout_id": "choices",
                        "outcome_keys": {"selected": "slot_selected", "empty": "slot_empty",
                                         "insufficient": "slot_insufficient",
                                         "ambiguous": "slot_ambiguous",
                                         "unknown": "slot_unknown"}
                    },
                    "fields": [{"name": "first", "value_type": {"type": "boolean"}}],
                    "facts": facts,
                    "gates": [{"gate_id": "choice", "predicate": predicate,
                               "on_unknown": {"kind": "abort_evaluation"}}],
                    "scoring": [],
                    "selection": {"mode": "exactly_one", "required_count": 1},
                    "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
                }),
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                json!({
                    "schema_version": "0.7",
                    "game": "neutral",
                    "server": "test",
                    "coordinate_space": {"width": 2, "height": 1},
                    "defaults": {"color_max_distance": 0.0},
                    "targets": [
                        colour("page/home", 0, [255, 0, 0]),
                        colour("page/result", 0, [255, 255, 255]),
                        colour("guard/ready", 1, [0, 255, 0]),
                        colour("slot/first", 1, [0, 255, 0]),
                        colour("slot/second", 1, [0, 0, 255])
                    ],
                    "candidate_layouts": [layout]
                }),
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version": "0.3", "pages": [
                    {"id": "neutral/home", "required": ["page/home"], "optional": [],
                     "forbidden": []},
                    {"id": "neutral/result", "required": ["page/result"], "optional": [],
                     "forbidden": []}
                ]}),
            ),
        ]);
        let path = self.files.path().join(name);
        fs::write(&path, bytes).unwrap();
        path.display().to_string()
    }
}

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// `actinglab --json replay --package <package> --frames <frames>`, then `words`.
fn replay(package: &str, frames: &str, words: &[String]) -> CliResult {
    let mut line = ["--json", "replay", "--package", package, "--frames", frames]
        .map(str::to_string)
        .to_vec();
    line.extend(words.iter().cloned());
    run_cli(line, true)
}

/// `--select pick --scan <scan>`, then `--variable <v>` for each of `variables`.
fn scan_replay(scan: &str, variables: &[&str]) -> Vec<String> {
    let mut words = ["--select", "pick", "--scan", scan]
        .map(str::to_string)
        .to_vec();
    for variable in variables {
        words.extend(["--variable".to_string(), (*variable).to_string()]);
    }
    words
}

/// The answer's error, refused or not.
fn error_of(result: &CliResult) -> Option<(String, Value)> {
    result.envelope.error.as_ref().map(|error| {
        (
            error.code.clone(),
            error.details.clone().unwrap_or(Value::Null),
        )
    })
}

/// The answer is `validation_failed` (exit 2) naming `--variable` as `problem`, with no data.
#[track_caller]
fn variable_refused(result: &CliResult, problem: &str, what: &str) {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 2, "{what}: {shown}");
    assert!(result.envelope.data.is_none(), "{what}: {shown}");
    let (code, details) =
        error_of(result).unwrap_or_else(|| panic!("{what} was not refused: {shown}"));
    assert_eq!(code, "validation_failed", "{what}: {shown}");
    assert_eq!(details["argument"], "--variable", "{what}: {shown}");
    assert_eq!(details["argument_problem"], problem, "{what}: {shown}");
}

#[test]
fn v2_replay_refuses_an_undeclared_or_unfit_variable_and_takes_a_fit_one() {
    let context = Isolated::new();
    let declaring = context.package("declaring.zip", true);
    let declaring_none = context.package("declaring_none.zip", false);
    let frames = context.frames();
    let scan = context.scan();

    for (what, package, variables, problem) in [
        (
            "a name the package does not declare",
            &declaring,
            vec!["other=\"opt_1\""],
            "not_recognised",
        ),
        (
            "an option the declaration does not offer",
            &declaring,
            vec!["pick=\"opt_3\""],
            "not_recognised",
        ),
        (
            "a value of another type",
            &declaring,
            vec!["pick=1"],
            "not_recognised",
        ),
        (
            "a value that is not <name>=<json>",
            &declaring,
            vec!["pick"],
            "not_recognised",
        ),
        (
            "a right side that is no JSON",
            &declaring,
            vec!["pick=opt_1"],
            "not_recognised",
        ),
        (
            "a name given twice",
            &declaring,
            vec!["pick=\"opt_1\"", "pick=\"opt_2\""],
            "repeated",
        ),
        (
            "a variable of a package that declares none",
            &declaring_none,
            vec!["pick=\"opt_1\""],
            "not_recognised",
        ),
    ] {
        variable_refused(
            &replay(package, &frames, &scan_replay(&scan, &variables)),
            problem,
            what,
        );
    }

    // Workflow #455 TK-1 (register §1.532; the ruling changes what this pinned): a whole replay
    // reads `--variable` by the same parser and check, so an option the declaration does not
    // offer is refused there too, and a value it takes is no refusal of `--variable`.
    variable_refused(
        &replay(
            &declaring,
            &frames,
            &["--variable".to_string(), "pick=\"opt_3\"".to_string()],
        ),
        "not_recognised",
        "an option not offered, given to a whole replay",
    );
    let whole = replay(
        &declaring,
        &frames,
        &["--variable".to_string(), "pick=\"opt_1\"".to_string()],
    );
    if let Some((code, details)) = error_of(&whole) {
        assert!(
            code != "validation_failed" || details["argument"] != "--variable",
            "{}",
            whole.envelope_json()
        );
    }

    // A value the declaration takes passes the argument check: whatever the scan replay then
    // answers, it is not a refusal of `--variable`.
    let taken = replay(
        &declaring,
        &frames,
        &scan_replay(&scan, &["pick=\"opt_1\""]),
    );
    if let Some((code, details)) = error_of(&taken) {
        assert!(
            code != "validation_failed" || details["argument"] != "--variable",
            "{}",
            taken.envelope_json()
        );
    }

    // Fix round 1: the value given replaces the default in what the scan's snapshot is overlaid
    // with: `opt_1` given, `opt_2` (the default) without it.
    assert_eq!(
        overlaid_pick(&declaring, &[("pick".to_owned(), json!("opt_1"))]),
        Some(FactValue::String("opt_1".to_owned()))
    );
    assert_eq!(
        overlaid_pick(&declaring, &[]),
        Some(FactValue::String("opt_2".to_owned()))
    );
}

/// Fix round 1: the value of `session.dispatch.parameter.pick` in a snapshot overlaid with what
/// the scan replay of `package` reads when it is given `variables` (`None` when it holds none).
fn overlaid_pick(package: &str, variables: &[(String, Value)]) -> Option<FactValue> {
    let parameters =
        crate::replay_cli::scan_replay_parameters(std::path::Path::new(package), &[], variables)
            .unwrap_or_else(|_| panic!("the scan replay takes {variables:?}"));
    let mut snapshot = InstanceFactSnapshot {
        snapshot_id: "snapshot-replay".to_owned(),
        ledger_position: 1,
        context: InstanceFactContext {
            instance_id: "offline.replay".to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: Vec::new(),
    };
    parameters.overlay(&mut snapshot, 1_000);
    snapshot
        .records
        .iter()
        .find(|record| record.key == "session.dispatch.parameter.pick")
        .map(|record| match &record.content {
            FactContent::Inline { value } => value.clone(),
            other => panic!("an overlay record is inline: {other:?}"),
        })
}
