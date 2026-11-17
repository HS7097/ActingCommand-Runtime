// SPDX-License-Identifier: AGPL-3.0-only

// #463 RM-1 (contracts/lab-recording.md, "Self-checks", item 11, "Near a threshold"; register
// §1.466): a recording's self-checks warn of a template mark whose score lies nearer its threshold
// than a bound (`--template-margin-bound`, default 0.03), on the side a self-check needs it on:
// `record_template_near_threshold`, the fact only. It refuses nothing, proposes nothing and is not
// written into the package.
// - `rm1_a_template_that_passes_its_frames_by_less_than_the_bound_is_warned_and_changes_nothing`
//   (the work order's test 1): step 1's button passes its sample by about 0.01: the stop's dry run
//   and the stop answer the warning with its numbers (the sample's frame, its score, the threshold,
//   the margin the cross-check's `margins` gives, the bound 0.03); `record ready` lists it and is
//   otherwise what it is with the bound 0, still `ready`; `record review` shows it at step 1 alone;
//   the package written is byte for byte the package the self-checks build for the same plan with
//   the bound 0, which answer no warning.
// - `rm1_a_template_that_passes_by_more_than_the_bound_is_not_warned` (test 2): passing by about
//   0.05, no warning from the stop's dry run or `record ready`.
// - `rm1_the_bound_given_another_value_is_the_bound_the_warning_follows` (test 3): with
//   `--template-margin-bound 0.06` the recording of test 2 warns of both its marks, each with that
//   bound, in `record ready` and in each step's `record review`; with 0.005 the recording of test 1
//   warns of none; 1.5 is `validation_failed` for `record stop` and `record ready`.
// - `rm1_a_template_that_must_not_pass_and_misses_by_less_than_the_bound_is_warned` (test 4): step
//   2 forbids step 1's button, which misses on step 2's frame by about 0.01 (`misses`,
//   `record_step_self_mismatch`); step 2's page is told from step 1's screen by its one template,
//   which misses there by about 0.01 (`misses`, `arrival_unconfirmed`); each the one warning of its
//   recording, none with the bound 0.
// - `rm1_a_recording_without_template_marks_answers_as_with_any_bound` (test 5): colour marks only:
//   the stop's dry run and `record ready` with the bound 1 are what they are with the default, no
//   warning, and the package is the same with the bounds 0 and 1.
// Frames are synthetic 96x64 screens of flat gray with neutral noise patches, as the gates of
// Workflow #390 LAB-2 make them; a perturbed patch has its top-left 6x6 block replaced by other
// noise, and its score is read with the function the admitted evaluator reads it with. Steps are
// recorded offline by `record mark` with their frames; no OCR model, no Runtime, no device; no
// clock is read.

use crate::recording::generate::{
    DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS, DEFAULT_ARRIVAL_TIMEOUT_MS, Plan, StopSettings,
    plan as generate_plan,
};
use crate::recording::model::LabRecording;
use crate::recording::store::{old_record_path, recording_path};
use crate::recording_api::package_with_bound;
use crate::{
    ClickSpec, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkOutcome, MarkRequest, MarkSpec,
    RecordRect, RecordReviewRequest, RecordStartOptions, RecordStopOptions, RecordingLock,
    record_mark, record_ready, record_review, record_start, record_stop,
};
use actingcommand_contract::LabError;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition::{MatchMetric, Rect, Scene};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_rm1";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
/// The warning's word.
const NEAR: &str = "record_template_near_threshold";
/// The bound when none is given.
const DEFAULT_BOUND: f64 = 0.03;

// The button of step 1 and the mark of step 2, each searched at its crop.
const BUTTON: RecordRect = rect(20, 20, 12, 12);
const NEXT: RecordRect = rect(40, 46, 12, 12);
// The click of step 1, on no patch.
const PRESS: RecordRect = rect(60, 10, 8, 8);
// Test 5's colour squares.
const SQUARE_1: RecordRect = rect(20, 20, 12, 12);
const SQUARE_2: RecordRect = rect(40, 46, 12, 12);

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    // A value in `low..=high`.
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// A gray image, row-major; it is drawn with equal red, green and blue.
#[derive(Clone)]
struct Gray {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Gray {
    fn from_fn(width: u32, height: u32, mut value: impl FnMut(u32, u32) -> u8) -> Self {
        let mut values = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                values.push(value(x, y));
            }
        }
        Self {
            width,
            height,
            values,
        }
    }

    fn frame() -> Self {
        Self::from_fn(WIDTH, HEIGHT, |_, _| BACKGROUND)
    }

    fn noise(width: u32, height: u32, seed: u64) -> Self {
        let mut noise = Noise(seed);
        Self::from_fn(width, height, |_, _| noise.between(1, 255))
    }

    fn flat(width: u32, height: u32, value: u8) -> Self {
        Self::from_fn(width, height, |_, _| value)
    }

    // `image` drawn with its top-left corner at `(x, y)`.
    fn draw(mut self, x: u32, y: u32, image: &Gray) -> Self {
        for row in 0..image.height {
            for column in 0..image.width {
                self.values[((y + row) * self.width + x + column) as usize] =
                    image.values[(row * image.width + column) as usize];
            }
        }
        self
    }

    // `image` drawn with its top-left corner at the corner of `at`.
    fn draw_at(self, at: RecordRect, image: &Gray) -> Self {
        let x = u32::try_from(at.x).expect("a corner in the frame");
        let y = u32::try_from(at.y).expect("a corner in the frame");
        self.draw(x, y, image)
    }

    fn png(&self) -> Vec<u8> {
        let pixels = self
            .values
            .iter()
            .flat_map(|value| [*value; 3])
            .collect::<Vec<_>>();
        Frame::from_pixels(
            self.width,
            self.height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG")
    }
}

fn button() -> Gray {
    Gray::noise(12, 12, 0x0463_0101)
}

fn next() -> Gray {
    Gray::noise(12, 12, 0x0463_0102)
}

// `patch` with its top-left 6x6 block replaced by other noise.
fn perturbed(patch: &Gray, seed: u64) -> Gray {
    patch.clone().draw(0, 0, &Gray::noise(6, 6, seed))
}

// The score of `patch` searched at `at` on `image`, as the admitted evaluator reads a template
// target there (the recording's metric, `ccoeff_normed`).
fn score(image: &Gray, patch: &Gray, at: RecordRect) -> f32 {
    Scene::from_png(&image.png())
        .expect("scene")
        .match_template_with_metric(
            &patch.png(),
            Some(Rect {
                x: at.x,
                y: at.y,
                width: at.width,
                height: at.height,
            }),
            MatchMetric::CorrelationCoefficientNormalized,
        )
        .expect("template match")
        .score
}

// The fixture's assumption on a perturbed patch's score: low enough below 1 for a threshold a
// hundredth beside it to leave the exact copy more than the bound, high enough to be a near copy.
fn near_copy(score: f32) -> f64 {
    let score = f64::from(score);
    assert!(
        (0.5..0.94).contains(&score),
        "fixture: the perturbed patch scores {score}"
    );
    score
}

// A thousandth at or below `value`, and one at or above it.
fn thousandth_below(value: f64) -> f64 {
    (value * 1000.0).floor() / 1000.0
}

fn thousandth_above(value: f64) -> f64 {
    (value * 1000.0).ceil() / 1000.0
}

// A template mark `id` searched at its crop, with the threshold `threshold` when one is given.
fn template(id: &str, crop: RecordRect, threshold: Option<f64>) -> MarkSpec {
    MarkSpec {
        search: Some(crop),
        threshold,
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

fn click() -> ClickSpec {
    ClickSpec {
        region: Some(PRESS),
        ..ClickSpec::default()
    }
}

// `record stop` options: the dry run or not, and the bound.
fn stop_options(dry_run: bool, bound: Option<f64>) -> RecordStopOptions {
    RecordStopOptions {
        dry_run,
        template_margin_bound: bound,
        ..RecordStopOptions::default()
    }
}

// The warnings `record_template_near_threshold` of a list of warnings.
fn near(warnings: &Value) -> Vec<Value> {
    warnings
        .as_array()
        .unwrap_or_else(|| panic!("no warnings in {warnings}"))
        .iter()
        .filter(|warning| warning["code"] == NEAR)
        .cloned()
        .collect()
}

// An answer whose `warnings` holds no `record_template_near_threshold`.
fn without_near(answer: &Value) -> Value {
    let mut answer = answer.clone();
    if let Some(warnings) = answer["warnings"].as_array_mut() {
        warnings.retain(|warning| warning["code"] != NEAR);
    }
    answer
}

// A number of an answer.
fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
}

// A state directory with an active session of `INSTANCE` and its Lab recording, before any step;
// the lock is held as one command holds it and dropped before the directory.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Recording {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        // The session file `record start` writes before the Lab recording is created.
        fs::write(
            old_record_path(&state, INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": "active",
                "started_at_unix_ms": 1_700_000_000_000_u64
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_rm1", "Lab-CLI")
            .expect("recording lock");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
        record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        Self { lock, ocr, temp }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // Writes `image` as the frame file `name` and returns its path.
    fn frame(&self, name: &str, image: &Gray) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, image.png()).expect("frame file");
        path.display().to_string()
    }

    // A step of its own frame `image`, its samples, marks, forbidden marks and click, which must
    // stand.
    fn step(
        &self,
        name: &str,
        image: &Gray,
        samples: &[&Gray],
        add: Vec<MarkSpec>,
        forbid: &[&str],
        click: Option<ClickSpec>,
    ) -> MarkOutcome {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(self.frame(name, image)),
            samples: samples
                .iter()
                .enumerate()
                .map(|(index, sample)| self.frame(&format!("{name}_sample_{index}"), sample))
                .collect(),
            add,
            forbid: forbid.iter().map(|id| (*id).to_owned()).collect(),
            click,
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr).unwrap_or_else(|error| {
            panic!(
                "record mark of {name}: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // `record stop` with `options`: the `lab` output with its status.
    fn stop_with(&self, options: &RecordStopOptions) -> Result<Value, LabError> {
        record_stop(&self.lock, options, &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    // `record stop` with `options`, which must stand.
    fn stopped(&self, options: &RecordStopOptions) -> Value {
        self.stop_with(options).unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // `record ready` with the bound `bound`, read as the command reads it.
    fn ready(&self, bound: Option<f64>) -> Result<Value, LabError> {
        record_ready(
            &self.state(),
            INSTANCE,
            None,
            &stop_options(false, bound),
            &self.ocr,
        )
        .map(|outcome| serde_json::to_value(&outcome).expect("record ready JSON"))
    }

    // `record ready` with the bound `bound`, which must answer.
    fn readied(&self, bound: Option<f64>) -> Value {
        self.ready(bound)
            .unwrap_or_else(|error| panic!("record ready: {}: {}", error.code, error.message))
    }

    // `record review --no-preview` of every step with the bound `bound`.
    fn reviewed(&self, bound: Option<f64>) -> Value {
        let outcome = record_review(
            &self.state(),
            INSTANCE,
            &RecordReviewRequest {
                record_id: None,
                step: None,
                no_preview: true,
            },
            &stop_options(false, bound),
            &self.ocr,
        )
        .unwrap_or_else(|error| panic!("record review: {}: {}", error.code, error.message));
        serde_json::to_value(&outcome).expect("record review JSON")
    }

    // The id of the frame of role `role` of the step at `index` (0 for step 1) of the step table.
    fn frame_id(&self, index: usize, role: &str) -> String {
        let recording: Value = serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON");
        recording["steps"][index]["frames"]
            .as_array()
            .and_then(|frames| frames.iter().find(|frame| frame["role"] == role))
            .and_then(|frame| frame["frame_id"].as_str())
            .unwrap_or_else(|| panic!("no {role} frame on step {} in {recording}", index + 1))
            .to_owned()
    }

    // The plan `record stop` generates from the recording as it stands, with no option given.
    fn plan(&self) -> Plan {
        let recording: LabRecording = serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON");
        let settings = StopSettings {
            game: Some("neutral".to_owned()),
            server: Some("test".to_owned()),
            locale: Some("en-US".to_owned()),
            package_id: None,
            requires: None,
            timeout_ms: None,
            arrival_timeout_ms: DEFAULT_ARRIVAL_TIMEOUT_MS,
            application_arrival_timeout_ms: DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS,
        };
        generate_plan(&recording, &settings)
            .unwrap_or_else(|error| panic!("the plan: {}: {}", error.code, error.message))
    }
}

// A recording the warning is about: the mark's score nearest its threshold, the threshold it was
// given and the frame of that score.
struct Fixture {
    recording: Recording,
    score: f64,
    threshold: f64,
    frame_id: String,
}

impl Fixture {
    // Two steps: step 1's button, exact on its primary frame and perturbed on its sample, with a
    // threshold `below` beneath its score on the sample (rounded down to a thousandth), and its
    // click; step 2's next mark at the recording's threshold, exact on its frame.
    fn passing_by(below: f64) -> Self {
        let recording = Recording::new();
        let sample = Gray::frame().draw_at(BUTTON, &perturbed(&button(), 0x0463_0201));
        let on_sample = near_copy(score(&sample, &button(), BUTTON));
        let threshold = thousandth_below(on_sample - below);
        recording.step(
            "first",
            &Gray::frame().draw_at(BUTTON, &button()),
            &[&sample],
            vec![template("ui/button", BUTTON, Some(threshold))],
            &[],
            Some(click()),
        );
        recording.step(
            "second",
            &Gray::frame().draw_at(NEXT, &next()),
            &[],
            vec![template("ui/next", NEXT, None)],
            &[],
            None,
        );
        let frame_id = recording.frame_id(0, "sample");
        Self {
            recording,
            score: on_sample,
            threshold,
            frame_id,
        }
    }

    // Two steps: step 1's button, exact on its frame, with a threshold a hundredth above its score
    // on step 2's frame (rounded up to a thousandth), and its click; step 2's next mark, exact, its
    // page forbidding the button, perturbed on step 2's frame.
    fn forbidding() -> Self {
        let recording = Recording::new();
        let second = Gray::frame()
            .draw_at(NEXT, &next())
            .draw_at(BUTTON, &perturbed(&button(), 0x0463_0202));
        let on_second = near_copy(score(&second, &button(), BUTTON));
        let threshold = thousandth_above(on_second + 0.01);
        recording.step(
            "first",
            &Gray::frame().draw_at(BUTTON, &button()),
            &[],
            vec![template("ui/button", BUTTON, Some(threshold))],
            &[],
            Some(click()),
        );
        recording.step(
            "second",
            &second,
            &[],
            vec![template("ui/next", NEXT, None)],
            &["ui/button"],
            None,
        );
        let frame_id = recording.frame_id(1, "primary");
        Self {
            recording,
            score: on_second,
            threshold,
            frame_id,
        }
    }

    // Two steps: step 1's button at the recording's threshold, exact, beside a perturbed copy of
    // step 2's next mark, and its click; step 2's next mark, exact on its frame, with a threshold a
    // hundredth above its score on step 1's frame (rounded up to a thousandth).
    fn arriving() -> Self {
        let recording = Recording::new();
        let first = Gray::frame()
            .draw_at(BUTTON, &button())
            .draw_at(NEXT, &perturbed(&next(), 0x0463_0203));
        let on_first = near_copy(score(&first, &next(), NEXT));
        let threshold = thousandth_above(on_first + 0.01);
        recording.step(
            "first",
            &first,
            &[],
            vec![template("ui/button", BUTTON, None)],
            &[],
            Some(click()),
        );
        recording.step(
            "second",
            &Gray::frame().draw_at(NEXT, &next()),
            &[],
            vec![template("ui/next", NEXT, Some(threshold))],
            &[],
            None,
        );
        let frame_id = recording.frame_id(0, "primary");
        Self {
            recording,
            score: on_first,
            threshold,
            frame_id,
        }
    }

    // The one warning `record_template_near_threshold` of `warnings`, checked against the
    // fixture's score and threshold: its score the fixture's, its threshold the one given (as the
    // package's `f32` holds it), its margin the distance of the two as the warning gives them, less
    // than 0.03 and at least about a hundredth; the warning as JSON.
    fn single(&self, warnings: &Value) -> Value {
        let found = near(warnings);
        assert_eq!(found.len(), 1, "{warnings}");
        let warning = found[0].clone();
        let (score, threshold, margin) = (
            number(&warning["score"]),
            number(&warning["threshold"]),
            number(&warning["margin"]),
        );
        assert!((score - self.score).abs() < 1e-6, "{warning}");
        assert!((threshold - self.threshold).abs() < 1e-6, "{warning}");
        assert_eq!(
            warning["margin"],
            json!((score - threshold).abs()),
            "{warning}"
        );
        assert!((0.0099..0.0111).contains(&margin), "{warning}");
        assert_eq!(warning["frame_id"], json!(self.frame_id), "{warning}");
        warning
    }
}

// The work order's test 1: a template mark that passes the frames of its step by less than the
// bound. Step 1's button passes its sample by about 0.01 (its primary frame is its exact crop).
// `record ready` lists the warning and is otherwise what it is with the bound 0: `ready`, nothing
// missing; the stop's dry run and the stop answer it with its numbers, its margin the one the
// cross-check's `margins` gives the button; `record review` shows it at step 1 alone; and the package
// the stop writes is byte for byte the package the self-checks build for the same plan with the
// bound 0, which answer no warning.
#[test]
fn rm1_a_template_that_passes_its_frames_by_less_than_the_bound_is_warned_and_changes_nothing() {
    let fixture = Fixture::passing_by(0.01);
    let recording = &fixture.recording;

    let dry = recording.stopped(&stop_options(true, None));
    assert_eq!(dry["lab_status"], "validated", "{dry}");
    let warning = fixture.single(&dry["warnings"]);
    assert_eq!(
        warning,
        json!({
            "code": NEAR,
            "side": "passes",
            "check": "record_step_self_mismatch",
            "step": 1,
            "record_index": 1,
            "page": dry["pages"][0],
            "mark_id": "ui/button",
            "frame_id": fixture.frame_id,
            "score": warning["score"],
            "threshold": warning["threshold"],
            "margin": warning["margin"],
            "bound": DEFAULT_BOUND
        }),
        "{dry}"
    );
    // The margin is the one the cross-check answers for the button on step 1's frames.
    let margins = dry["cross_check"]["margins"]
        .as_array()
        .unwrap_or_else(|| panic!("no margins in {dry}"));
    let button_margin = margins
        .iter()
        .find(|entry| entry["step"] == 1 && entry["target"] == "ui/button")
        .unwrap_or_else(|| panic!("no margin of the button in {dry}"));
    assert_eq!(button_margin["min_margin"], warning["margin"], "{dry}");

    // `record ready` lists it, and says of the recording what it says with the bound 0.
    let ready = recording.readied(None);
    let ready_zero = recording.readied(Some(0.0));
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(
        near(&ready["warnings"]),
        std::slice::from_ref(&warning),
        "{ready}"
    );
    assert!(near(&ready_zero["warnings"]).is_empty(), "{ready_zero}");
    assert_eq!(without_near(&ready), ready_zero);

    // `record review` shows it at the step it names, and at no other.
    let review = recording.reviewed(None);
    assert_eq!(review["steps"][0]["step"], 1, "{review}");
    assert_eq!(review["steps"][0]["warnings"], json!([warning]), "{review}");
    assert!(review["steps"][1].get("warnings").is_none(), "{review}");

    // The package of the same plan with the bound 0, and with the default bound.
    let plan = recording.plan();
    let (zero_bytes, zero_near) =
        package_with_bound(&plan, &recording.ocr, 0.0).expect("the package with the bound 0");
    assert!(zero_near.is_empty(), "{zero_near:?}");
    let (default_bytes, default_near) = package_with_bound(&plan, &recording.ocr, DEFAULT_BOUND)
        .expect("the package with the default bound");
    assert_eq!(default_near.len(), 1, "{default_near:?}");
    assert_eq!(default_bytes, zero_bytes);

    // The stop answers what its dry run answered, and writes the package of the bound 0.
    let stopped = recording.stopped(&stop_options(false, None));
    assert_eq!(stopped["lab_status"], "generated", "{stopped}");
    assert_eq!(near(&stopped["warnings"]), [warning], "{stopped}");
    assert_eq!(stopped["warnings"], dry["warnings"], "{stopped}");
    let written = fs::read(stopped["path"].as_str().expect("the package's path")).expect("package");
    assert_eq!(
        written, zero_bytes,
        "the package is byte for byte the one of the bound 0"
    );
}

// The work order's test 2: a template mark that passes by more than the bound. Step 1's button
// passes its sample by about 0.05 and step 2's next mark its frame by about 0.05: no warning from
// the stop's dry run or from `record ready`.
#[test]
fn rm1_a_template_that_passes_by_more_than_the_bound_is_not_warned() {
    let fixture = Fixture::passing_by(0.05);
    let dry = fixture.recording.stopped(&stop_options(true, None));
    assert_eq!(dry["lab_status"], "validated", "{dry}");
    assert!(near(&dry["warnings"]).is_empty(), "{dry}");
    let ready = fixture.recording.readied(None);
    assert_eq!(ready["status"], "ready", "{ready}");
    assert!(near(&ready["warnings"]).is_empty(), "{ready}");
}

// The work order's test 3: the bound given by `--template-margin-bound` (the options' bound). With
// 0.06 the recording of test 2 warns of both its marks, step 1's button on its sample and step 2's
// next mark on its frame, each with the bound 0.06, in the stop's dry run, in `record ready` and in
// each step's `record review`; with 0.005 the recording of test 1 warns of none. A bound outside 0
// to 1 is `validation_failed` for `record stop` and `record ready`.
#[test]
fn rm1_the_bound_given_another_value_is_the_bound_the_warning_follows() {
    let wide = Fixture::passing_by(0.05);
    let dry = wide.recording.stopped(&stop_options(true, Some(0.06)));
    let warned = near(&dry["warnings"]);
    assert_eq!(
        warned
            .iter()
            .map(|warning| (
                warning["mark_id"].clone(),
                warning["step"].clone(),
                warning["side"].clone(),
                warning["bound"].clone()
            ))
            .collect::<Vec<_>>(),
        [
            (json!("ui/button"), json!(1), json!("passes"), json!(0.06)),
            (json!("ui/next"), json!(2), json!("passes"), json!(0.06))
        ],
        "{dry}"
    );
    assert_eq!(warned[0]["frame_id"], json!(wide.frame_id), "{dry}");
    for warning in &warned {
        let margin = number(&warning["margin"]);
        assert!((0.04..0.06).contains(&margin), "{warning}");
    }
    let ready = wide.recording.readied(Some(0.06));
    assert_eq!(near(&ready["warnings"]), warned, "{ready}");
    let review = wide.recording.reviewed(Some(0.06));
    assert_eq!(
        review["steps"][0]["warnings"],
        json!([warned[0]]),
        "{review}"
    );
    assert_eq!(
        review["steps"][1]["warnings"],
        json!([warned[1]]),
        "{review}"
    );

    let narrow = Fixture::passing_by(0.01);
    let dry = narrow.recording.stopped(&stop_options(true, Some(0.005)));
    assert!(near(&dry["warnings"]).is_empty(), "{dry}");
    assert!(
        near(&narrow.recording.readied(Some(0.005))["warnings"]).is_empty(),
        "record ready with the bound 0.005"
    );

    let refused = narrow
        .recording
        .stop_with(&stop_options(true, Some(1.5)))
        .expect_err("a bound above 1");
    assert_eq!(refused.code, "validation_failed", "{}", refused.message);
    assert_eq!(
        refused
            .details
            .as_ref()
            .map(|details| details["option"].clone()),
        Some(json!("--template-margin-bound")),
        "{:?}",
        refused.details
    );
    let refused = narrow
        .recording
        .ready(Some(1.5))
        .expect_err("a bound above 1");
    assert_eq!(refused.code, "validation_failed", "{}", refused.message);
}

// The work order's test 4, the other direction: a template mark that must not pass and misses its
// threshold by less than the bound. Step 2's page forbids step 1's button, which misses on step 2's
// frame by about 0.01: `misses`, judged by `record_step_self_mismatch`, at step 2. In another
// recording step 2's page is told from step 1's screen by its one template, which misses there by
// about 0.01: `misses`, judged by the arrival gate (`arrival_unconfirmed`), at step 1 with the gate.
// Each is the one warning of its recording, and none is answered with the bound 0.
#[test]
fn rm1_a_template_that_must_not_pass_and_misses_by_less_than_the_bound_is_warned() {
    let forbidding = Fixture::forbidding();
    let dry = forbidding.recording.stopped(&stop_options(true, None));
    assert_eq!(dry["lab_status"], "validated", "{dry}");
    let warning = forbidding.single(&dry["warnings"]);
    assert_eq!(
        warning,
        json!({
            "code": NEAR,
            "side": "misses",
            "check": "record_step_self_mismatch",
            "step": 2,
            "record_index": 2,
            "page": dry["pages"][1],
            "mark_id": "ui/button",
            "frame_id": forbidding.frame_id,
            "score": warning["score"],
            "threshold": warning["threshold"],
            "margin": warning["margin"],
            "bound": DEFAULT_BOUND
        }),
        "{dry}"
    );
    assert!(
        number(&warning["score"]) < number(&warning["threshold"]),
        "{warning}"
    );
    let zero = forbidding.recording.stopped(&stop_options(true, Some(0.0)));
    assert!(near(&zero["warnings"]).is_empty(), "{zero}");

    let arriving = Fixture::arriving();
    let dry = arriving.recording.stopped(&stop_options(true, None));
    assert_eq!(dry["lab_status"], "validated", "{dry}");
    let warning = arriving.single(&dry["warnings"]);
    assert_eq!(
        warning,
        json!({
            "code": NEAR,
            "side": "misses",
            "check": "arrival_unconfirmed",
            "step": 1,
            "record_index": 1,
            "gate": dry["pages"][1],
            "mark_id": "ui/next",
            "frame_id": arriving.frame_id,
            "score": warning["score"],
            "threshold": warning["threshold"],
            "margin": warning["margin"],
            "bound": DEFAULT_BOUND
        }),
        "{dry}"
    );
    assert!(
        number(&warning["score"]) < number(&warning["threshold"]),
        "{warning}"
    );
    let zero = arriving.recording.stopped(&stop_options(true, Some(0.0)));
    assert!(near(&zero["warnings"]).is_empty(), "{zero}");
}

// The work order's test 5: a recording without template marks, each step's page a colour mark on a
// flat square. The stop's dry run and `record ready` with the bound 1, the largest, are what they are
// with the default bound, and answer no warning `record_template_near_threshold`; the package the
// self-checks build is the same with the bounds 0 and 1.
#[test]
fn rm1_a_recording_without_template_marks_answers_as_with_any_bound() {
    let recording = Recording::new();
    recording.step(
        "first",
        &Gray::frame().draw_at(SQUARE_1, &Gray::flat(12, 12, 200)),
        &[],
        vec![MarkSpec::region_only(
            "ui/square_1".to_owned(),
            MarkFamily::Color,
            SQUARE_1,
        )],
        &[],
        Some(click()),
    );
    recording.step(
        "second",
        &Gray::frame().draw_at(SQUARE_2, &Gray::flat(12, 12, 140)),
        &[],
        vec![MarkSpec::region_only(
            "ui/square_2".to_owned(),
            MarkFamily::Color,
            SQUARE_2,
        )],
        &[],
        None,
    );

    let dry = recording.stopped(&stop_options(true, None));
    assert_eq!(dry["lab_status"], "validated", "{dry}");
    assert!(near(&dry["warnings"]).is_empty(), "{dry}");
    assert_eq!(recording.stopped(&stop_options(true, Some(1.0))), dry);
    let ready = recording.readied(None);
    assert_eq!(ready["status"], "ready", "{ready}");
    assert!(near(&ready["warnings"]).is_empty(), "{ready}");
    assert_eq!(recording.readied(Some(1.0)), ready);

    let plan = recording.plan();
    let (zero_bytes, zero_near) =
        package_with_bound(&plan, &recording.ocr, 0.0).expect("the package with the bound 0");
    let (one_bytes, one_near) =
        package_with_bound(&plan, &recording.ocr, 1.0).expect("the package with the bound 1");
    assert!(zero_near.is_empty() && one_near.is_empty(), "{one_near:?}");
    assert_eq!(one_bytes, zero_bytes);
}
