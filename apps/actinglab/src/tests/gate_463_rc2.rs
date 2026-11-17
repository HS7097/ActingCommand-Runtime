// SPDX-License-Identifier: AGPL-3.0-only

// #463 RC-2 (contracts/resource-regression-case.md, "A case" and "The reader"; register §1.459):
// the case reader reads past a member of `scene_construction`, of its `canvas` or of a rectangle
// that the contract does not list (a note on how the scene was made), each named once in the
// warning `case_member_unknown` with its place (`json_pointer`, the pointer a failure there would
// name) and its name (`member`); `expect`, `expect.observe` and an entry of `expect.recognize` stay
// closed. On RC-1's neutral package and checkout (`gate_463_rc1.rs`: a 4x1 canvas, page `home` by a
// red pixel at x 0, page `other` by a green pixel at x 1); every case is made up here; no Runtime,
// no device, no OCR model, no clock read by the tests.
// - `rc2_notes_on_the_scene_construction_change_no_verdict_and_warn_with_their_place` (1): a
//   `note` in `scene_construction`, then `note`, `pastes`, `overlays`, `asset_pastes` and
//   `constructed` there beside a top-level `note`: the verdict and the checks are those of the same
//   case without them, one warning per place and name, `/note` told from `/scene_construction/note`;
//   `resource regression --case` answers the row `passed` with those warnings.
// - `rc2_a_member_of_the_canvas_or_of_a_rectangle_is_read_past_with_its_place` (2): a member of the
//   canvas, and `from` in each of two rectangles: the same verdict and checks, one warning at each
//   place.
// - `rc2_an_unknown_member_of_an_expectation_still_fails_member_unknown` (3): a member unknown in
//   `expect`, in `expect.observe` and in an entry of `expect.recognize`: each the case's one check,
//   failed `member_unknown` at its place, and no warning.
// - `rc2_a_wrong_or_missing_listed_member_still_fails_with_its_word` (4): `canvas.width` a text
//   (beside a note that is read past) fails `member_invalid`; no `scene_sha256` fails
//   `member_missing`.
// - `rc2_a_case_as_record_stop_writes_it_warns_no_unknown_member` (5): cases of the members
//   `record stop` writes (#463 S2-10, `case_document`): a step's, the final screen's, an ending's,
//   one without the page judgement and the bare canvas's: each `passed`, no `case_member_unknown`.

use super::env_lock;
use super::gate_463_rc1::{
    BLACK, Checkout, HOME, PACKAGE, PAGE_AT, RED, case, rect, scene, verdict,
};
use crate::cli_module::own_line;
use actingcommand_lab::{
    RegressionCaseVerdict, RegressionProblem, RegressionResult, RegressionVerdict,
    read_regression_case,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The case of RC-1's fixture whose scene shows the red pixel at x 0, pasted at its one rectangle,
/// expecting page `home`, which passes on it.
fn home_case(bytes: &[u8]) -> Value {
    case(
        bytes,
        json!([rect(0, 1)]),
        json!({"observe": {"page": HOME}}),
    )
}

/// Each warning `case_member_unknown` of `verdict` as its place and its name, in the order
/// answered.
fn unknown_members(verdict: &RegressionCaseVerdict) -> Vec<(String, String)> {
    verdict
        .warnings
        .iter()
        .filter(|warning| warning.outcome.code().as_str() == "case_member_unknown")
        .map(|warning| {
            let text = |key: &str| {
                warning
                    .outcome
                    .value(key)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("no {key} in {:?}", warning.outcome))
                    .to_owned()
            };
            (text("json_pointer"), text("member"))
        })
        .collect()
}

/// `places`, each a place and a name, as [`unknown_members`] answers them.
fn named(places: &[(&str, &str)]) -> Vec<(String, String)> {
    places
        .iter()
        .map(|&(place, name)| (place.to_owned(), name.to_owned()))
        .collect()
}

/// Each check of `verdict` as its place and why it failed.
fn checks_of(verdict: &RegressionCaseVerdict) -> Vec<(&str, Option<RegressionProblem>)> {
    verdict
        .checks
        .iter()
        .map(|check| (check.at.as_str(), check.problem))
        .collect()
}

/// `noted` answers what `plain`, the same case without its notes, answers: the same verdict and
/// the same checks, of the same case.
fn assert_same_answer(noted: &RegressionCaseVerdict, plain: &RegressionCaseVerdict) {
    assert_eq!(noted.verdict, plain.verdict, "{noted:?}");
    assert_eq!(noted.checks, plain.checks, "{noted:?}");
    assert_eq!(
        (&noted.id, &noted.package_id),
        (&plain.id, &plain.package_id)
    );
}

/// The reader's verdict on `document`, which it refuses.
fn refused(document: &Value) -> RegressionCaseVerdict {
    let answered =
        read_regression_case(&serde_json::to_vec(document).expect("the case serializes"))
            .expect_err("the reader refuses the case");
    assert_eq!(answered.verdict, RegressionVerdict::Failed, "{answered:?}");
    answered
}

#[test]
fn rc2_notes_on_the_scene_construction_change_no_verdict_and_warn_with_their_place() {
    let _guard = env_lock();
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let plain_case = home_case(&bytes);
    let plain = verdict(&checkout, &plain_case, &bytes);
    assert_eq!(plain.verdict, RegressionVerdict::Passed, "{plain:?}");
    assert_eq!(checks_of(&plain), [(PAGE_AT, None)], "{plain:?}");
    assert!(plain.warnings.is_empty(), "{plain:?}");

    // One note on how the scene was made.
    let mut one = plain_case.clone();
    one["scene_construction"]["note"] = json!("pasted from a frame of the page");
    let noted = verdict(&checkout, &one, &bytes);
    assert_same_answer(&noted, &plain);
    assert_eq!(noted.warnings.len(), 1, "{noted:?}");
    assert_eq!(
        unknown_members(&noted),
        named(&[("/scene_construction/note", "note")])
    );

    // Several, beside a note of the top level: each its own warning, told apart by its place.
    let mut several = plain_case;
    several["note"] = json!("a note of the repository's own");
    let construction = &mut several["scene_construction"];
    construction["note"] = json!("pasted from a frame of the page");
    construction["pastes"] = json!([{"x": 0, "y": 0, "width": 1, "height": 1}]);
    construction["overlays"] = json!([]);
    construction["asset_pastes"] = json!([{"asset": "a neutral patch", "x": 0, "y": 0}]);
    construction["constructed"] = json!("by hand");
    let read_past = verdict(&checkout, &several, &bytes);
    assert_same_answer(&read_past, &plain);
    let expected = named(&[
        ("/note", "note"),
        ("/scene_construction/note", "note"),
        ("/scene_construction/pastes", "pastes"),
        ("/scene_construction/overlays", "overlays"),
        ("/scene_construction/asset_pastes", "asset_pastes"),
        ("/scene_construction/constructed", "constructed"),
    ]);
    assert_eq!(unknown_members(&read_past), expected, "{read_past:?}");
    assert_eq!(read_past.warnings.len(), expected.len(), "{read_past:?}");

    // The operation's answer: the case's row passed, with each warning's place and name.
    checkout.add_case("a-notes", &several, Some(&bytes));
    let result = checkout.run(&["--case", "a-notes"]);
    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the answer");
    let row = &data["cases"][0];
    assert_eq!(row["path"], "regression/neutral/a-notes", "{data}");
    assert_eq!(row["verdict"], "passed", "{row}");
    let answered = row["warnings"]
        .as_array()
        .expect("the row's warnings")
        .iter()
        .map(|warning| {
            (
                warning["outcome"]["code"].clone(),
                warning["outcome"]["values"].clone(),
            )
        })
        .collect::<Vec<_>>();
    let expected_rows = expected
        .iter()
        .map(|(place, name)| {
            (
                json!("case_member_unknown"),
                json!({"json_pointer": place, "member": name}),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(answered, expected_rows, "{row}");
}

#[test]
fn rc2_a_member_of_the_canvas_or_of_a_rectangle_is_read_past_with_its_place() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let plain_case = case(
        &bytes,
        json!([rect(0, 1), rect(1, 1)]),
        json!({"observe": {"page": HOME}}),
    );
    let plain = verdict(&checkout, &plain_case, &bytes);
    assert_eq!(plain.verdict, RegressionVerdict::Passed, "{plain:?}");
    assert!(plain.warnings.is_empty(), "{plain:?}");

    // A member of the canvas.
    let mut canvas = plain_case.clone();
    canvas["scene_construction"]["canvas"]["source_size"] = json!([8, 2]);
    let canvas_noted = verdict(&checkout, &canvas, &bytes);
    assert_same_answer(&canvas_noted, &plain);
    assert_eq!(canvas_noted.warnings.len(), 1, "{canvas_noted:?}");
    assert_eq!(
        unknown_members(&canvas_noted),
        named(&[("/scene_construction/canvas/source_size", "source_size")])
    );

    // `from` in each rectangle: one warning at each place.
    let mut from = plain_case;
    for index in 0_usize..2 {
        from["scene_construction"]["rects"][index]["from"] =
            json!({"frame": "a source frame", "x": index, "y": 0});
    }
    let rects_noted = verdict(&checkout, &from, &bytes);
    assert_same_answer(&rects_noted, &plain);
    assert_eq!(rects_noted.warnings.len(), 2, "{rects_noted:?}");
    assert_eq!(
        unknown_members(&rects_noted),
        named(&[
            ("/scene_construction/rects/0/from", "from"),
            ("/scene_construction/rects/1/from", "from"),
        ])
    );
}

#[test]
fn rc2_an_unknown_member_of_an_expectation_still_fails_member_unknown() {
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let rects = json!([rect(0, 1)]);
    let documents = [
        (
            "/expect/note",
            case(
                &bytes,
                rects.clone(),
                json!({"observe": {"page": HOME}, "note": "a misspelt expectation"}),
            ),
        ),
        (
            "/expect/observe/note",
            case(
                &bytes,
                rects.clone(),
                json!({"observe": {"page": HOME, "note": "a misspelt expectation"}}),
            ),
        ),
        (
            "/expect/recognize/0/note",
            case(
                &bytes,
                rects,
                json!({"recognize": [
                    {"target": "home_red", "passed": true, "note": "a misspelt expectation"}
                ]}),
            ),
        ),
    ];
    for (at, document) in documents {
        let failed = refused(&document);
        assert_eq!(
            failed
                .checks
                .iter()
                .map(|check| (check.at.as_str(), check.result, check.problem))
                .collect::<Vec<_>>(),
            [(
                at,
                RegressionResult::Failed,
                Some(RegressionProblem::MemberUnknown)
            )],
            "{failed:?}"
        );
        assert!(unknown_members(&failed).is_empty(), "{failed:?}");
    }
}

#[test]
fn rc2_a_wrong_or_missing_listed_member_still_fails_with_its_word() {
    let bytes = scene([RED, BLACK, BLACK, BLACK]);

    // `canvas.width` a text, beside a note that is read past.
    let mut wrong = home_case(&bytes);
    wrong["scene_construction"]["canvas"]["width"] = json!("four");
    wrong["scene_construction"]["note"] = json!("pasted from a frame of the page");
    let invalid = refused(&wrong);
    assert_eq!(
        checks_of(&invalid),
        [(
            "/scene_construction/canvas/width",
            Some(RegressionProblem::MemberInvalid)
        )],
        "{invalid:?}"
    );
    assert_eq!(
        unknown_members(&invalid),
        named(&[("/scene_construction/note", "note")])
    );

    // No `scene_sha256`.
    let mut missing = home_case(&bytes);
    missing["scene_construction"] = json!({
        "kind": "roi_patch",
        "canvas": {"width": 4, "height": 1, "fill": [0, 0, 0]},
        "rects": [rect(0, 1)]
    });
    let absent = refused(&missing);
    assert_eq!(
        checks_of(&absent),
        [(
            "/scene_construction/scene_sha256",
            Some(RegressionProblem::MemberMissing)
        )],
        "{absent:?}"
    );
}

/// A case of the package `PACKAGE` over `bytes` with the members `record stop` writes and no other
/// (#463 S2-10, `crates/lab/src/recording/cases.rs`, `case_document`): the black canvas of the
/// recording's size, `rects` each with the targets that read it as its `content`, the scene's
/// SHA-256, `source` and `expect`.
fn as_record_stop_writes(
    id: &str,
    source: Value,
    rects: Value,
    expect: Value,
    bytes: &[u8],
) -> Value {
    json!({
        "schema_version": "actingcommand.resource-regression-case.v1",
        "id": id,
        "package_id": PACKAGE,
        "scene": "scene.png",
        "scene_construction": {
            "kind": "roi_patch",
            "canvas": {"width": 4, "height": 1, "fill": [0, 0, 0]},
            "rects": rects,
            "scene_sha256": format!("{:x}", Sha256::digest(bytes))
        },
        "source": source,
        "expect": expect
    })
}

#[test]
fn rc2_a_case_as_record_stop_writes_it_warns_no_unknown_member() {
    let checkout = Checkout::new();
    let record_id = "1700000000000-4242-neutral_task";
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let bare = scene([BLACK, BLACK, BLACK, BLACK]);
    // Every region a target of the package reads, each once with its readers.
    let regions = json!([
        {"x": 0, "y": 0, "width": 1, "height": 1, "content": "home_red"},
        {"x": 1, "y": 0, "width": 1, "height": 1, "content": "other_green"}
    ]);
    let page = json!({
        "observe": {"page": HOME},
        "recognize": [{"target": "home_red", "passed": true}]
    });
    let cases = [
        as_record_stop_writes(
            "step_01",
            json!({"record_id": record_id, "step": 1}),
            regions.clone(),
            page.clone(),
            &bytes,
        ),
        as_record_stop_writes(
            "final",
            json!({"record_id": record_id, "final": true}),
            regions.clone(),
            page.clone(),
            &bytes,
        ),
        as_record_stop_writes(
            "ending_neutral_key",
            json!({"record_id": record_id, "ending": "neutral_key"}),
            regions.clone(),
            page,
            &bytes,
        ),
        // Without the page judgement: its page's required target passes, the forbidden one fails.
        as_record_stop_writes(
            "step_02",
            json!({"record_id": record_id, "step": 2}),
            regions,
            json!({"recognize": [
                {"target": "home_red", "passed": true},
                {"target": "other_green", "passed": false}
            ]}),
            &bytes,
        ),
    ];
    for document in &cases {
        let answered = verdict(&checkout, document, &bytes);
        assert_eq!(answered.verdict, RegressionVerdict::Passed, "{answered:?}");
        assert!(unknown_members(&answered).is_empty(), "{answered:?}");
    }
    // The bare canvas, run once per package as a case that expects no page.
    let canvas = as_record_stop_writes(
        "bare_canvas",
        json!({"record_id": record_id, "bare_canvas": true}),
        json!([]),
        json!({"observe": {"page": null}}),
        &bare,
    );
    let answered = verdict(&checkout, &canvas, &bare);
    assert_eq!(answered.verdict, RegressionVerdict::Passed, "{answered:?}");
    assert!(unknown_members(&answered).is_empty(), "{answered:?}");
}
