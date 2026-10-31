// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-11 (register §1.240; work order `rd-1031/brief-r11.md`, tests 1 and 2): the
//! `ui.json` of a synthetic `neutral` game root reads back exactly as written when it keeps its
//! rules, and every violation is reported at its JSON pointer with its `standard_pack_problem`:
//! shape, limits, an unregistered `package_id`, a missing group, an undeclared parameter.

use super::*;
use crate::{STANDARD_PACK_BUNDLE_FILE, StandardPackRootBytes};
use serde_json::json;

const GAME: &str = "neutral";
const ALPHA: &str = "neutral.test.alpha";
const BETA: &str = "neutral.test.beta";
const OTHER: &str = "neutral.other.alpha";

/// A bundle index v4 of the three neutral packages.
fn bundle() -> Vec<u8> {
    let packs = [(ALPHA, "test"), (BETA, "test"), (OTHER, "other")]
        .iter()
        .map(|(package_id, server)| {
            json!({
                "package_id": package_id,
                "server": server,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 3,
                "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packs
    }))
    .expect("bundle JSON")
}

fn registry() -> StandardPackRegistry {
    StandardPackRegistry::from_bundle_bytes(&bundle(), GAME).expect("a valid bundle")
}

/// A valid `ui.json`, its members in an order that is not alphabetical, one text not ASCII.
const VALID: &str = concat!(
    r#"{"schema_version":"actingcommand.ui.v1","game":"neutral","#,
    r#""groups":[{"id":"group_a","title":{"en":"Group A","xx-Test":"Gruppe ä"}},"#,
    r#"{"id":"group_b","title":{"en":"Group B"}}],"#,
    r#""packs":[{"title":{"en":"Alpha","ja":"アルファ"},"package_id":"neutral.test.alpha","#,
    r#""summary":{"en":"The first neutral package."},"group":"group_a","#,
    r#""risk":["word_one","word_two"]},"#,
    r#"{"package_id":"neutral.test.beta","title":{"en":"Beta"},"summary":{"en":"Second."}}],"#,
    r#""display":{"safe_area":"declared_word","aspect_ratios":["16:9","13:6"]}}"#
);

fn valid() -> Value {
    serde_json::from_str(VALID).expect("the valid document")
}

/// The valid document with the member at `pointer` replaced by `value`.
fn replaced(pointer: &str, value: Value) -> Value {
    let mut document = valid();
    *document.pointer_mut(pointer).expect("a member") = value;
    document
}

/// The valid document with one more member `name` in the object at `parent`.
fn added(parent: &str, name: &str, value: Value) -> Value {
    let mut document = valid();
    document
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .expect("an object")
        .insert(name.to_owned(), value);
    document
}

/// The valid document without the member `name` of the object at `parent`.
fn without(parent: &str, name: &str) -> Value {
    let mut document = valid();
    document
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .expect("an object")
        .shift_remove(name)
        .expect("a member");
    document
}

fn parse(bytes: &[u8]) -> Result<StandardPackUi, Vec<StandardPackRootIssue>> {
    parse_standard_pack_ui(bytes, GAME, &registry())
}

fn issues(expected: &[(StandardPackProblem, &str)]) -> Vec<StandardPackRootIssue> {
    expected
        .iter()
        .map(|(problem, pointer)| StandardPackRootIssue {
            file: STANDARD_PACK_UI_FILE,
            problem: *problem,
            pointer: (*pointer).to_owned(),
        })
        .collect()
}

/// `count` groups, the id of each by `id`, each titled `{"en": "x"}`.
fn groups(count: usize, id: impl Fn(usize) -> String) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"id": id(index), "title": {"en": "x"}}))
            .collect(),
    )
}

/// Test 1: a valid `ui.json` reads back exactly as written, byte for byte when written compactly:
/// member order, a text that is not ASCII, the optional members. Its entry of a package and its
/// `display` are the file's; a package without an entry has none. The smallest valid file and a
/// file at the limits read too.
#[test]
fn r11_t1_a_valid_ui_json_reads_back_exactly_as_written() {
    let ui = parse(VALID.as_bytes()).expect("a valid ui.json");
    assert_eq!(
        serde_json::to_string(&ui).expect("encode the read file"),
        VALID
    );
    assert_eq!(&Value::Object(ui.document().clone()), &valid());
    assert_eq!(ui.pack(ALPHA), Some(&valid()["packs"][0]));
    assert_eq!(ui.pack(BETA), Some(&valid()["packs"][1]));
    assert_eq!(ui.pack(OTHER), None, "a package without an entry");
    assert_eq!(ui.display(), Some(&valid()["display"]));

    let smallest =
        r#"{"schema_version":"actingcommand.ui.v1","game":"neutral","groups":[],"packs":[]}"#;
    let read = parse(smallest.as_bytes()).expect("the smallest ui.json");
    assert_eq!(serde_json::to_string(&read).expect("encode"), smallest);
    assert_eq!(read.display(), None);
    assert_eq!(read.pack(ALPHA), None);

    let mut bounds = valid();
    bounds["groups"] = groups(MAX_PACK_UI_GROUPS, |index| format!("g{index}"));
    bounds["packs"][0]["group"] = json!("g0");
    bounds["packs"][0]["title"] = json!({"en": "t".repeat(MAX_PACK_UI_TITLE_BYTES)});
    bounds["packs"][0]["summary"] = Value::Object(
        (0..MAX_PACK_UI_LOCALES)
            .map(|index| {
                (
                    format!("{index:0>width$}", width = MAX_PACK_UI_LOCALE_BYTES),
                    json!("s".repeat(MAX_PACK_UI_SUMMARY_BYTES)),
                )
            })
            .collect(),
    );
    bounds["packs"][0]["risk"] = Value::Array(
        (0..MAX_PACK_UI_RISK_WORDS)
            .map(|index| json!(format!("{index:w>width$}", width = MAX_PACK_UI_WORD_BYTES)))
            .collect(),
    );
    bounds["display"]["aspect_ratios"] = Value::Array(
        (1..=MAX_PACK_UI_ASPECT_RATIOS)
            .map(|index| json!(format!("99999:{index}")))
            .collect(),
    );
    let bytes = serde_json::to_vec(&bounds).expect("encode");
    let read = parse(&bytes).expect("a ui.json at its limits");
    assert_eq!(Value::Object(read.document().clone()), bounds);
}

/// Test 2: each kind of violation is reported at its place, the problem with the file and its
/// JSON pointer: shape (`malformed`, `game_mismatch`), limits (`ui_out_of_limits`), an
/// unregistered `package_id` (`package_not_listed`), a missing group (`ui_group_unknown`), an
/// undeclared parameter (`ui_parameter_undeclared`), and an id listed twice (`ui_duplicate`).
/// Several violations are all reported in document order, at most 64. A `ui.json` without
/// `bundle.json` is `bundle_missing` for the root reader of `ui.json`.
#[test]
fn r11_t2_each_ui_json_violation_is_reported_at_its_place() {
    use StandardPackProblem::{
        GameMismatch, Malformed, PackageNotListed, UiDuplicate, UiGroupUnknown, UiOutOfLimits,
        UiParameterUndeclared,
    };
    let encode = |value: &Value| serde_json::to_vec(value).expect("case JSON");
    let parameter = json!({"label": {"en": "Level"}, "options": {"low": {"en": "Low"}}});
    let mut nine_words = valid();
    nine_words["packs"][0]["risk"] = Value::Array(
        (0..=MAX_PACK_UI_RISK_WORDS)
            .map(|index| json!(format!("w{index}")))
            .collect(),
    );
    let mut too_many_groups = valid();
    too_many_groups["groups"] = groups(MAX_PACK_UI_GROUPS + 1, |index| format!("g{index}"));
    too_many_groups["packs"][0]["group"] = json!("g0");
    let long_locale = "l".repeat(MAX_PACK_UI_LOCALE_BYTES + 1);
    let mut padded = VALID.as_bytes().to_vec();
    padded.resize(MAX_STANDARD_PACK_UI_BYTES + 1, b' ');
    let repeated = VALID.replacen(
        r#""game":"neutral","#,
        r#""game":"neutral","game":"neutral","#,
        1,
    );
    let cases: Vec<(&str, Vec<u8>, Vec<StandardPackRootIssue>)> = vec![
        // Shape.
        ("not JSON", b"{".to_vec(), issues(&[(Malformed, "")])),
        (
            "no object",
            encode(&json!([valid()])),
            issues(&[(Malformed, "")]),
        ),
        (
            "an unknown member",
            encode(&added("", "extra", json!(true))),
            issues(&[(Malformed, "/extra")]),
        ),
        (
            "an unknown member of a pack, its name holding a slash",
            encode(&added("/packs/0", "a/b", json!(1))),
            issues(&[(Malformed, "/packs/0/a~1b")]),
        ),
        (
            "no packs",
            encode(&without("", "packs")),
            issues(&[(Malformed, "/packs")]),
        ),
        (
            "a pack without summary",
            encode(&without("/packs/1", "summary")),
            issues(&[(Malformed, "/packs/1/summary")]),
        ),
        (
            "another version",
            encode(&replaced("/schema_version", json!("actingcommand.ui.v2"))),
            issues(&[(Malformed, "/schema_version")]),
        ),
        (
            "a title that is no text",
            encode(&replaced("/packs/0/title", json!("Alpha"))),
            issues(&[(Malformed, "/packs/0/title")]),
        ),
        (
            "a risk word that is a number",
            encode(&replaced("/packs/0/risk/0", json!(1))),
            issues(&[(Malformed, "/packs/0/risk/0")]),
        ),
        (
            "a member repeated",
            repeated.into_bytes(),
            issues(&[(Malformed, "/game")]),
        ),
        (
            "an aspect ratio not of the form w:h",
            encode(&replaced("/display/aspect_ratios/1", json!("16x9"))),
            issues(&[(Malformed, "/display/aspect_ratios/1")]),
        ),
        (
            "an aspect ratio with a leading zero",
            encode(&replaced("/display/aspect_ratios/0", json!("016:9"))),
            issues(&[(Malformed, "/display/aspect_ratios/0")]),
        ),
        (
            "another game",
            encode(&replaced("/game", json!("other"))),
            issues(&[(GameMismatch, "/game")]),
        ),
        // Limits.
        (
            "a file over 256 KiB",
            padded,
            issues(&[(UiOutOfLimits, "")]),
        ),
        (
            "a title over 128 bytes",
            encode(&replaced(
                "/packs/0/title/en",
                json!("t".repeat(MAX_PACK_UI_TITLE_BYTES + 1)),
            )),
            issues(&[(UiOutOfLimits, "/packs/0/title/en")]),
        ),
        (
            "a text without a locale",
            encode(&replaced("/packs/0/title", json!({}))),
            issues(&[(UiOutOfLimits, "/packs/0/title")]),
        ),
        (
            "a locale name over 35 bytes",
            encode(&added("/packs/1/summary", &long_locale, json!("x"))),
            issues(&[(
                UiOutOfLimits,
                format!("/packs/1/summary/{long_locale}").as_str(),
            )]),
        ),
        (
            "an empty group id",
            encode(&replaced("/groups/1/id", json!(""))),
            issues(&[(UiOutOfLimits, "/groups/1/id")]),
        ),
        (
            "65 groups",
            encode(&too_many_groups),
            issues(&[(UiOutOfLimits, "/groups")]),
        ),
        (
            "nine risk words",
            encode(&nine_words),
            issues(&[(UiOutOfLimits, "/packs/0/risk")]),
        ),
        (
            "no aspect ratio",
            encode(&replaced("/display/aspect_ratios", json!([]))),
            issues(&[(UiOutOfLimits, "/display/aspect_ratios")]),
        ),
        // References.
        (
            "an unregistered package",
            encode(&replaced(
                "/packs/1/package_id",
                json!("neutral.test.absent"),
            )),
            issues(&[(PackageNotListed, "/packs/1/package_id")]),
        ),
        (
            "a missing group",
            encode(&replaced("/packs/0/group", json!("group_c"))),
            issues(&[(UiGroupUnknown, "/packs/0/group")]),
        ),
        (
            "an undeclared parameter",
            encode(&added(
                "/packs/0",
                "parameters",
                json!({"level": parameter}),
            )),
            issues(&[(UiParameterUndeclared, "/packs/0/parameters/level")]),
        ),
        // Ids listed twice.
        (
            "a group id twice",
            encode(&replaced("/groups/1/id", json!("group_a"))),
            issues(&[(UiDuplicate, "/groups/1/id")]),
        ),
        (
            "a package's entry twice",
            encode(&replaced("/packs/1/package_id", json!(ALPHA))),
            issues(&[(UiDuplicate, "/packs/1/package_id")]),
        ),
        (
            "a risk word twice",
            encode(&replaced("/packs/0/risk/1", json!("word_one"))),
            issues(&[(UiDuplicate, "/packs/0/risk/1")]),
        ),
    ];
    for (case, bytes, expected) in cases {
        assert_eq!(parse(&bytes), Err(expected), "{case}");
    }

    // Several violations: every one, in document order.
    let mut several = added("", "extra", json!(true));
    several["packs"][0]["title"]["en"] = json!("t".repeat(MAX_PACK_UI_TITLE_BYTES + 1));
    several["packs"][0]["group"] = json!("group_c");
    several["packs"][0]["parameters"] = json!({"level": parameter});
    several["packs"][1]["package_id"] = json!("neutral.test.absent");
    assert_eq!(
        parse(&encode(&several)),
        Err(issues(&[
            (Malformed, "/extra"),
            (UiOutOfLimits, "/packs/0/title/en"),
            (UiGroupUnknown, "/packs/0/group"),
            (UiParameterUndeclared, "/packs/0/parameters/level"),
            (PackageNotListed, "/packs/1/package_id"),
        ]))
    );

    // At most 64, the first in document order.
    let mut many = valid();
    many["groups"] = groups(MAX_PACK_UI_GROUPS + 6, |_| String::new());
    let refused = parse(&encode(&many)).expect_err("70 empty group ids");
    assert_eq!(refused.len(), MAX_STANDARD_PACK_UI_ISSUES);
    assert_eq!(
        refused[..2],
        issues(&[(UiOutOfLimits, "/groups"), (UiOutOfLimits, "/groups/0/id")])[..]
    );

    // The root reader of ui.json: ui.json alone is bundle_missing; with bundle.json it is a root.
    let bundle = bundle();
    assert_eq!(
        read_standard_pack_root_with_ui(StandardPackRootBytes::default(), true, GAME),
        Err(StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        })
    );
    assert_eq!(
        read_standard_pack_root_with_ui(StandardPackRootBytes::default(), false, GAME),
        Ok(None)
    );
    let root = read_standard_pack_root_with_ui(
        StandardPackRootBytes {
            bundle: Some(&bundle),
            ..StandardPackRootBytes::default()
        },
        true,
        GAME,
    )
    .expect("a game root")
    .expect("bundle.json makes it one");
    assert_eq!(root.registry, registry());
}
