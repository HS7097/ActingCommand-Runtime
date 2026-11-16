// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #477 PC-1 (register §1.438; work order `rd-1116/brief-477-pc1-pack-catalog.md`, the
//! tests of the reader and of the index): the `pack-catalog.json` of a synthetic `neutral` game
//! root reads with both its parts; every error of the business part, and of the file as a whole,
//! is reported at its JSON pointer with its `standard_pack_problem`; a display defect is a warning
//! and the group, the display or the package it touches falls back; a member the reader does not
//! know (a value member included) is an error but in a group or `display`, where it is a display
//! defect (fix round 1); the business part's limits are errors at their pointers (fix round 1);
//! and the index holds each relation once, both ways, whatever the quantity.

use super::*;
use crate::{STANDARD_PACK_BUNDLE_FILE, StandardPackRootBytes, parse_standard_pack_schedule};
use serde_json::json;

const GAME: &str = "neutral";
const ALPHA: &str = "neutral.test.alpha";
const BETA: &str = "neutral.test.beta";
const OTHER: &str = "neutral.other.alpha";
const POOL_A: &str = "pool.neutral.a";
const POOL_B: &str = "pool.neutral.b";

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

/// The pools the root's `schedule.json` declares.
fn pools() -> BTreeSet<String> {
    BTreeSet::from([POOL_A.to_owned(), POOL_B.to_owned()])
}

/// A catalog that keeps every rule: two groups, a display, `ALPHA` with its display and a
/// constant consumption and a pending production, `BETA` without display and with a production
/// whose quantity is absent; `OTHER` has no entry.
fn valid() -> Value {
    json!({
        "schema_version": "actingcommand.pack-catalog.v1",
        "game": GAME,
        "groups": [
            {"id": "group_a", "title": {"en": "Group A", "xx-Test": "Gruppe ä"}},
            {"id": "group_b", "title": {"en": "Group B"}}
        ],
        "display": {"aspect_ratios": ["16:9", "13:6"], "safe_area": "declared_word"},
        "packs": [
            {
                "package_id": ALPHA,
                "title": {"en": "Alpha", "ja": "アルファ"},
                "summary": {"en": "The first neutral package."},
                "group": "group_a",
                "risk": ["word_one", "word_two"],
                "consumes": [{"pool_id": POOL_B, "quantity": {"kind": "constant", "amount": 3}}],
                "produces": [{"pool_id": POOL_A, "quantity": {"kind": "pending"}}]
            },
            {"package_id": BETA, "consumes": [], "produces": [{"pool_id": POOL_A}]}
        ]
    })
}

fn encode(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON")
}

fn parse(bytes: &[u8]) -> Result<StandardPackCatalog, Vec<StandardPackRootIssue>> {
    parse_standard_pack_catalog(bytes, GAME, &registry(), Some(&pools()))
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

fn issues(expected: &[(StandardPackProblem, &str)]) -> Vec<StandardPackRootIssue> {
    expected
        .iter()
        .map(|(problem, pointer)| StandardPackRootIssue {
            file: STANDARD_PACK_CATALOG_FILE,
            problem: *problem,
            pointer: (*pointer).to_owned(),
        })
        .collect()
}

fn pool(pool_id: &str, quantity: Option<CatalogQuantity>) -> CatalogPool {
    CatalogPool {
        pool_id: pool_id.to_owned(),
        quantity,
    }
}

/// The reader: a full file reads with both its parts. `ALPHA`'s display is its display members as
/// written and its business part its two pools with their quantities; `BETA`, which writes no
/// display member, has no display (it shows with its id) and no warning, and its production's
/// quantity is absent; `OTHER` is unlisted. The display part a client reads is the groups and the
/// displays as written, and `display` is the file's. The smallest file reads; without a schedule
/// to judge by (`None`) a pool is not judged; a directory whose only root file is the catalog is
/// `bundle_missing` for the root reader of the catalog.
#[test]
fn pc1_a_full_catalog_reads_with_both_parts() {
    let catalog = parse(&encode(&valid())).expect("a valid pack-catalog.json");
    assert!(
        catalog.display_issues().is_empty(),
        "{:?}",
        catalog.display_issues()
    );
    assert_eq!(catalog.entries().len(), 2);
    let alpha = catalog.entry(ALPHA).expect("ALPHA's entry");
    assert_eq!(
        alpha.business,
        CatalogBusiness {
            consumes: vec![pool(POOL_B, Some(CatalogQuantity::Constant { amount: 3 }))],
            produces: vec![pool(POOL_A, Some(CatalogQuantity::Pending))],
        }
    );
    let alpha_display = json!({
        "package_id": ALPHA,
        "title": {"en": "Alpha", "ja": "アルファ"},
        "summary": {"en": "The first neutral package."},
        "group": "group_a",
        "risk": ["word_one", "word_two"]
    });
    assert_eq!(
        alpha.display.clone().map(Value::Object),
        Some(alpha_display.clone())
    );
    let beta = catalog.entry(BETA).expect("BETA's entry");
    assert_eq!(
        beta.display, None,
        "no display member: it shows with its id"
    );
    assert_eq!(beta.business.consumes, Vec::new());
    assert_eq!(beta.business.produces, vec![pool(POOL_A, None)]);
    assert_eq!(catalog.entry(OTHER), None);
    assert_eq!(catalog.unlisted(&registry()), vec![OTHER.to_owned()]);
    assert_eq!(catalog.display(), Some(&valid()["display"]));
    assert_eq!(
        catalog.ui(),
        json!({"groups": valid()["groups"], "packs": [alpha_display]})
    );

    let smallest = json!({
        "schema_version": "actingcommand.pack-catalog.v1",
        "game": GAME,
        "packs": []
    });
    let read = parse(&encode(&smallest)).expect("the smallest pack-catalog.json");
    assert!(read.entries().is_empty());
    assert_eq!(read.display(), None);
    assert_eq!(read.ui(), json!({"groups": [], "packs": []}));
    assert_eq!(
        read.unlisted(&registry()),
        vec![OTHER.to_owned(), ALPHA.to_owned(), BETA.to_owned()]
    );

    let undeclared = replaced("/packs/1/produces/0/pool_id", json!("pool.neutral.absent"));
    let read = parse_standard_pack_catalog(&encode(&undeclared), GAME, &registry(), None)
        .expect("no pool is judged without a schedule to judge by");
    assert_eq!(
        read.entry(BETA).expect("BETA's entry").business.produces,
        vec![pool("pool.neutral.absent", None)]
    );

    assert_eq!(
        read_standard_pack_root_with_catalog(StandardPackRootBytes::default(), true, GAME),
        Err(StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        })
    );
    assert_eq!(
        read_standard_pack_root_with_catalog(StandardPackRootBytes::default(), false, GAME),
        Ok(None)
    );
}

/// The reader's refusals, one fixture each, every one an error at its JSON pointer with its
/// `standard_pack_problem`: the file unreadable as JSON, no object or too large; a member
/// repeated; another version or another game; a document or business member missing or not of
/// its type; a `package_id` the bundle does not register, or listed twice; a `pool_id` the
/// schedule does not declare, or listed twice on one side; a malformed quantity. Several errors
/// are all reported, in the reader's order.
#[test]
fn pc1_each_business_refusal_is_an_error_at_its_pointer() {
    use StandardPackProblem::{
        CatalogDuplicate, CatalogOutOfLimits, CatalogPoolUndeclared, GameMismatch, Malformed,
        PackageNotListed,
    };
    let quantity = "/packs/0/consumes/0/quantity";
    let mut padded = encode(&valid());
    padded.resize(MAX_STANDARD_PACK_CATALOG_BYTES + 1, b' ');
    let repeated = serde_json::to_string(&valid()).expect("JSON").replacen(
        r#""game":"neutral","#,
        r#""game":"neutral","game":"neutral","#,
        1,
    );
    let cases: Vec<(&str, Vec<u8>, Vec<StandardPackRootIssue>)> = vec![
        ("not JSON", b"{".to_vec(), issues(&[(Malformed, "")])),
        (
            "no object",
            encode(&json!([valid()])),
            issues(&[(Malformed, "")]),
        ),
        (
            "a file over 1 MiB",
            padded,
            issues(&[(CatalogOutOfLimits, "")]),
        ),
        (
            "a member repeated",
            repeated.into_bytes(),
            issues(&[(Malformed, "/game")]),
        ),
        (
            "another version",
            encode(&replaced(
                "/schema_version",
                json!("actingcommand.pack-catalog.v2"),
            )),
            issues(&[(Malformed, "/schema_version")]),
        ),
        (
            "another game",
            encode(&replaced("/game", json!("other"))),
            issues(&[(GameMismatch, "/game")]),
        ),
        (
            "no packs",
            encode(&without("", "packs")),
            issues(&[(Malformed, "/packs")]),
        ),
        (
            "an entry without consumes",
            encode(&without("/packs/1", "consumes")),
            issues(&[(Malformed, "/packs/1/consumes")]),
        ),
        (
            "a package_id that is a number",
            encode(&replaced("/packs/1/package_id", json!(7))),
            issues(&[(Malformed, "/packs/1/package_id")]),
        ),
        (
            "a package the bundle does not register",
            encode(&replaced(
                "/packs/1/package_id",
                json!("neutral.test.absent"),
            )),
            issues(&[(PackageNotListed, "/packs/1/package_id")]),
        ),
        (
            "a package listed twice",
            encode(&replaced("/packs/1/package_id", json!(ALPHA))),
            issues(&[(CatalogDuplicate, "/packs/1/package_id")]),
        ),
        (
            "a pool the schedule does not declare",
            encode(&replaced(
                "/packs/1/produces/0/pool_id",
                json!("pool.neutral.absent"),
            )),
            issues(&[(CatalogPoolUndeclared, "/packs/1/produces/0/pool_id")]),
        ),
        (
            "a pool listed twice on one side",
            encode(&replaced(
                "/packs/0/consumes",
                json!([{"pool_id": POOL_B}, {"pool_id": POOL_B}]),
            )),
            issues(&[(CatalogDuplicate, "/packs/0/consumes/1/pool_id")]),
        ),
        (
            "a pool_id that is a number",
            encode(&replaced("/packs/1/produces/0/pool_id", json!(1))),
            issues(&[(Malformed, "/packs/1/produces/0/pool_id")]),
        ),
        (
            "a constant without its amount",
            encode(&replaced(quantity, json!({"kind": "constant"}))),
            issues(&[(Malformed, "/packs/0/consumes/0/quantity/amount")]),
        ),
        (
            "a constant of zero",
            encode(&replaced(
                quantity,
                json!({"kind": "constant", "amount": 0}),
            )),
            issues(&[(Malformed, "/packs/0/consumes/0/quantity/amount")]),
        ),
        (
            "a pending quantity with an amount",
            encode(&replaced(quantity, json!({"kind": "pending", "amount": 1}))),
            issues(&[(Malformed, "/packs/0/consumes/0/quantity/amount")]),
        ),
        (
            "a quantity of another kind",
            encode(&replaced(quantity, json!({"kind": "fact"}))),
            issues(&[(Malformed, "/packs/0/consumes/0/quantity/kind")]),
        ),
        (
            "a quantity that is a number",
            encode(&replaced(quantity, json!(3))),
            issues(&[(Malformed, quantity)]),
        ),
    ];
    for (case, bytes, expected) in cases {
        assert_eq!(parse(&bytes).map(|_| ()), Err(expected), "{case}");
    }

    // Several errors: every one, in the reader's order (the document's own members first).
    let mut several = added("", "extra", json!(true));
    several["packs"][1]["package_id"] = json!("neutral.test.absent");
    several["packs"][0]["consumes"][0]["pool_id"] = json!("pool.neutral.absent");
    assert_eq!(
        parse(&encode(&several)).map(|_| ()),
        Err(issues(&[
            (Malformed, "/extra"),
            (CatalogPoolUndeclared, "/packs/0/consumes/0/pool_id"),
            (PackageNotListed, "/packs/1/package_id"),
        ]))
    );
}

/// The valid document whose entry `entry` lists on `side` the pools `pool.neutral.p<n>` for `n` in
/// `numbers`, each without a quantity.
fn with_pools(entry: usize, side: &str, numbers: std::ops::Range<usize>) -> Value {
    let pools = numbers
        .map(|n| json!({"pool_id": format!("pool.neutral.p{n:04}")}))
        .collect::<Vec<_>>();
    replaced(&format!("/packs/{entry}/{side}"), Value::Array(pools))
}

/// Fix round 1 (the independent review's missing tests): the business part's limits, each an error
/// at its pointer, and each bound itself read. `packs` of 1025 entries is `catalog_out_of_limits`
/// at `/packs` (its first error; the entries repeat one package, each a `catalog_duplicate`), and
/// of 1024 it is not; a side of 257 pools is `catalog_out_of_limits` at the side and nothing else
/// (no pool judged against a schedule), and 256 read; a `pool_id` that is empty or of 129 bytes is
/// `catalog_out_of_limits` at it and one of 128 bytes reads; a constant `amount` of 2^53 - 1
/// reads and one of 2^53 is `malformed` at it.
#[test]
fn pc1_the_business_limits_are_errors_at_their_pointers() {
    use StandardPackProblem::{CatalogOutOfLimits, Malformed};
    let entries = |count: usize| {
        let entry = json!({"package_id": ALPHA, "consumes": [], "produces": []});
        replaced("/packs", Value::Array(vec![entry; count]))
    };
    let over = parse(&encode(&entries(MAX_PACK_CATALOG_PACKS + 1))).expect_err("1025 entries");
    assert_eq!(
        over.first(),
        issues(&[(CatalogOutOfLimits, "/packs")]).first()
    );
    let at_bound = parse(&encode(&entries(MAX_PACK_CATALOG_PACKS))).expect_err("duplicates");
    assert!(
        at_bound
            .iter()
            .all(|issue| issue.problem != CatalogOutOfLimits),
        "{at_bound:?}"
    );

    let unjudged = |document: &Value| {
        parse_standard_pack_catalog(&encode(document), GAME, &registry(), None).map(|_| ())
    };
    assert_eq!(
        unjudged(&with_pools(0, "consumes", 0..MAX_PACK_CATALOG_POOLS + 1)),
        Err(issues(&[(CatalogOutOfLimits, "/packs/0/consumes")]))
    );
    assert_eq!(
        unjudged(&with_pools(0, "consumes", 0..MAX_PACK_CATALOG_POOLS)),
        Ok(())
    );

    let pool_id = "/packs/1/produces/0/pool_id";
    for long in [
        String::new(),
        "p".repeat(MAX_PACK_CATALOG_POOL_ID_BYTES + 1),
    ] {
        assert_eq!(
            parse(&encode(&replaced(pool_id, json!(long)))).map(|_| ()),
            Err(issues(&[(CatalogOutOfLimits, pool_id)])),
            "a pool_id of {} bytes",
            long.len()
        );
    }
    let longest = "p".repeat(MAX_PACK_CATALOG_POOL_ID_BYTES);
    assert_eq!(unjudged(&replaced(pool_id, json!(longest))), Ok(()));

    let amount = "/packs/0/consumes/0/quantity/amount";
    let largest = parse(&encode(&replaced(amount, json!(MAX_PACK_CATALOG_AMOUNT))))
        .expect("the largest amount reads");
    assert_eq!(
        largest
            .entry(ALPHA)
            .expect("ALPHA's entry")
            .business
            .consumes,
        vec![pool(
            POOL_B,
            Some(CatalogQuantity::Constant {
                amount: MAX_PACK_CATALOG_AMOUNT
            })
        )]
    );
    assert_eq!(
        parse(&encode(&replaced(
            amount,
            json!(MAX_PACK_CATALOG_AMOUNT + 1)
        )))
        .map(|_| ()),
        Err(issues(&[(Malformed, amount)]))
    );
}

/// Strictness: a member the reader does not know is an error at its pointer at the document's own
/// level, on an entry, on a pool and on a quantity, and a value member (which a later version adds
/// under its own schema) is one there. Fix round 1 (the owner's answer 6): a member a group or
/// `display` does not know is a display defect, a warning
/// (`pc1_display_defects_are_warnings_with_the_fallback`).
#[test]
fn pc1_an_unknown_member_and_a_value_member_are_errors() {
    use StandardPackProblem::Malformed;
    let cases = [
        (added("", "value", json!(1000)), "/value"),
        (added("/packs/0", "value", json!(1000)), "/packs/0/value"),
        (
            added("/packs/0/produces/0", "value", json!(1000)),
            "/packs/0/produces/0/value",
        ),
        (
            added("/packs/0/consumes/0/quantity", "value", json!(1000)),
            "/packs/0/consumes/0/quantity/value",
        ),
        (
            added("/packs/1", "parameters", json!({})),
            "/packs/1/parameters",
        ),
        (added("/packs/0", "a/b", json!(1)), "/packs/0/a~1b"),
    ];
    for (document, pointer) in cases {
        assert_eq!(
            parse(&encode(&document)).map(|_| ()),
            Err(issues(&[(Malformed, pointer)])),
            "{pointer}"
        );
    }
}

/// Display defects are warnings: the file reads, each defect is reported at its pointer, and what
/// it touches falls back: a group whose title breaks a limit leaves the groups a client reads (a
/// package naming it still finds it), a `display` with an aspect ratio not `<w>:<h>` is not
/// declared, a package naming a group that does not exist and a package that writes a title
/// without a summary show with their ids. Their business parts are read as written. Fix round 1
/// (the owner's answer 6; these two cases asserted errors before): a member a group does not know
/// and a member `display` does not know are `malformed` warnings at their pointers, and that group
/// and that `display` are left out; a package naming that group still finds it.
#[test]
fn pc1_display_defects_are_warnings_with_the_fallback() {
    use StandardPackProblem::{
        CatalogDuplicate, CatalogGroupUnknown, CatalogOutOfLimits, CatalogTextMissing, Malformed,
    };
    let mut document = valid();
    document["groups"][1]["title"] = json!({});
    document["display"]["aspect_ratios"][1] = json!("13x6");
    document["packs"][0]["group"] = json!("group_c");
    document["packs"][1]["title"] = json!({"en": "Beta"});
    let catalog = parse(&encode(&document)).expect("display defects refuse nothing");
    assert_eq!(
        catalog.display_issues(),
        issues(&[
            (CatalogOutOfLimits, "/groups/1/title"),
            (Malformed, "/display/aspect_ratios/1"),
            (CatalogGroupUnknown, "/packs/0/group"),
            (CatalogTextMissing, "/packs/1/summary"),
        ])
        .as_slice()
    );
    assert_eq!(catalog.display(), None);
    let alpha = catalog.entry(ALPHA).expect("ALPHA's entry");
    assert_eq!(alpha.display, None, "a group that does not exist: its id");
    assert_eq!(
        alpha.business.consumes,
        vec![pool(POOL_B, Some(CatalogQuantity::Constant { amount: 3 }))]
    );
    assert_eq!(
        catalog.entry(BETA).expect("BETA's entry").display,
        None,
        "a missing text: its id"
    );
    assert_eq!(
        catalog.ui(),
        json!({"groups": [valid()["groups"][0]], "packs": []})
    );

    // A group id twice, a group without a title, a risk word twice: warnings as well.
    let mut document = valid();
    document["groups"][1]["id"] = json!("group_a");
    document["groups"][0]
        .as_object_mut()
        .expect("a group")
        .shift_remove("title");
    document["packs"][0]["risk"][1] = json!("word_one");
    let catalog = parse(&encode(&document)).expect("display defects refuse nothing");
    assert_eq!(
        catalog.display_issues(),
        issues(&[
            (CatalogTextMissing, "/groups/0/title"),
            (CatalogDuplicate, "/groups/1/id"),
            (CatalogDuplicate, "/packs/0/risk/1"),
        ])
        .as_slice()
    );
    assert_eq!(catalog.ui(), json!({"groups": [], "packs": []}));

    // Fix round 1: a member a group or `display` does not know is a warning, not an error.
    let mut document = added("/groups/0", "rank", json!(1));
    document["display"]["scale"] = json!(2);
    let catalog = parse(&encode(&document)).expect("unknown display members refuse nothing");
    assert_eq!(
        catalog.display_issues(),
        issues(&[(Malformed, "/groups/0/rank"), (Malformed, "/display/scale")]).as_slice()
    );
    assert_eq!(catalog.display(), None, "that display is left out");
    let alpha_display = catalog
        .entry(ALPHA)
        .expect("ALPHA's entry")
        .display
        .clone()
        .map(Value::Object);
    assert_eq!(
        catalog.ui(),
        json!({"groups": [valid()["groups"][1]], "packs": [alpha_display]}),
        "that group is left out; ALPHA, which names it, still shows its display"
    );
}

/// The index both ways, computed by the reader: a package three rules of `schedule.json` name with
/// one pool is one source of it once the writer has written its entry (a constant quantity, every
/// rule stating the same amount; pending where the rules differ); pending and absent quantities
/// are in the index as well as constant ones; producers and consumers are kept apart.
#[test]
fn pc1_the_index_holds_each_relation_once_both_ways() {
    let schedule = parse_standard_pack_schedule(&encode(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [
            {"id": "rule.one", "package_id": ALPHA,
             "produces": [{"pool_id": POOL_A, "amount": 2}]},
            {"id": "rule.two", "package_id": ALPHA,
             "produces": [{"pool_id": POOL_A, "amount": 2}]},
            {"id": "rule.three", "package_id": ALPHA,
             "produces": [{"pool_id": POOL_A, "amount": 2}],
             "consumes": [{"pool_id": POOL_B, "amount": 1}]},
            {"id": "rule.four", "package_id": BETA,
             "consumes": [{"pool_id": POOL_A, "amount": "pending"}]},
            {"id": "rule.five", "package_id": BETA,
             "consumes": [{"pool_id": POOL_A, "amount": 5}]}
        ],
        "pools": [{"id": POOL_A}, {"id": POOL_B}]
    })))
    .expect("the schedule reads");
    let merged = merge_standard_pack_catalog(GAME, &registry(), Some(&schedule), None, None)
        .unwrap_or_else(|errors| panic!("the writer's merge: {errors:?}"));
    assert_eq!(
        merged.added,
        vec![OTHER.to_owned(), ALPHA.to_owned(), BETA.to_owned()]
    );
    let alpha = merged.catalog.entry(ALPHA).expect("ALPHA's entry");
    assert_eq!(
        alpha.business.produces,
        vec![pool(POOL_A, Some(CatalogQuantity::Constant { amount: 2 }))],
        "three rules, one pool, one entry"
    );
    assert_eq!(
        merged
            .catalog
            .entry(BETA)
            .expect("BETA's entry")
            .business
            .consumes,
        vec![pool(POOL_A, Some(CatalogQuantity::Pending))],
        "rules that differ: pending"
    );
    let index = merged.catalog.index();
    assert_eq!(
        index.producing,
        BTreeMap::from([(POOL_A.to_owned(), vec![ALPHA.to_owned()])]),
        "a package three rules name is one source"
    );
    assert_eq!(
        index.consuming,
        BTreeMap::from([
            (POOL_A.to_owned(), vec![BETA.to_owned()]),
            (POOL_B.to_owned(), vec![ALPHA.to_owned()]),
        ])
    );
    assert!(merged.differences.is_empty(), "{:?}", merged.differences);

    // Pending and absent quantities are in the index; producers and consumers apart.
    let catalog = parse(&encode(&json!({
        "schema_version": "actingcommand.pack-catalog.v1",
        "game": GAME,
        "packs": [
            {"package_id": ALPHA,
             "consumes": [{"pool_id": POOL_A, "quantity": {"kind": "pending"}}],
             "produces": [{"pool_id": POOL_B}]},
            {"package_id": BETA,
             "consumes": [],
             "produces": [{"pool_id": POOL_B, "quantity": {"kind": "constant", "amount": 1}}]}
        ]
    })))
    .expect("the catalog reads");
    let index = catalog.index();
    assert_eq!(
        index.producing,
        BTreeMap::from([(POOL_B.to_owned(), vec![ALPHA.to_owned(), BETA.to_owned()])])
    );
    assert_eq!(
        index.consuming,
        BTreeMap::from([(POOL_A.to_owned(), vec![ALPHA.to_owned()])])
    );
}
