// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-B (0.12.93; register §1.536; contracts/business-identity-consensus.md, "A
// layout's pool"; contracts/selection-graph.md, "Rules"; the coordinator's rulings 1 to 4, 10 and
// 12 and Addendum 1 of the work order of board #503): a `repeated_anchor` layout may declare a
// picture identity feature scored in a region of each instance against one pool of pictures for
// every instance, whose entries are the pictures of keys and of known cards that are none of the
// keys (`other`). The identity mapping judges a read once, from every score of the pool: a key,
// none of the keys, or unread; an entry whose target's colour check failed does not compete. The
// scores stay beside the projection; the projection carries the mapped feature and no evidence of
// the pool, and a pool of 128 over 64 instances projects within the projection's limits.
//
// Every expected value comes from those rulings and the fixtures below, never from the code. The
// frame is 232 x 88 on a background of gray 30: up to 64 cards in eight rows of eight, card k's
// origin at (4 + 28 (k mod 8), 4 + 10 (k div 8)), each a noise anchor of 8 x 6 at its origin, a
// picture of 6 x 6 at (10, 0) from it (gray noise of the card's kind; two kinds never share a
// picture) and, for the colour test, a 4 x 4 square of one colour at (18, 1). A layout's feature
// `member` scores its pool in the region (9, 0, 8, 7) of each card. The gate reads no clock.

use crate::*;
use actingcommand_contract::candidate_projection::{
    CANDIDATE_PROJECTION_MAX_BYTES, CandidateFeature, CandidateIdentitySource,
    CandidateUnknownReason,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const FRAME_WIDTH: u32 = 232;
const FRAME_HEIGHT: u32 = 88;
const LAYOUT: &str = "layout/cards";
const ANCHOR: &str = "ui/card";
const ANCHOR_ASSET: &str = "assets/card.png";
const BACKGROUND: u8 = 30;
const THRESHOLD: u16 = 900;
const MARGIN: u16 = 50;
const RED: [u8; 3] = [200, 0, 0];
const BLUE: [u8; 3] = [0, 0, 200];
const GREEN: [u8; 3] = [0, 200, 0];

// Deterministic gray noise in 16..=239 for the picture of `kind` (the anchor is kind `u64::MAX`),
// `width` x `height`, row-major.
fn noise(kind: u64, width: u32, height: u32) -> Vec<u8> {
    let mut state = kind.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491_4f6c_dd1d;
    (0..width * height)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            16 + ((state >> 33) % 224) as u8
        })
        .collect()
}

fn anchor_pixels() -> Vec<u8> {
    noise(u64::MAX, 8, 6)
}

fn picture_pixels(kind: u64) -> Vec<u8> {
    noise(kind, 6, 6)
}

// The asset path of the picture of `kind`.
fn picture_asset(kind: u64) -> String {
    format!("assets/pic_{kind:03}.png")
}

// The templates' assets: the anchor and the pictures of `kinds`.
#[derive(Debug)]
struct Assets(BTreeMap<String, Vec<u8>>);

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| RecognitionPackError::fatal(format!("no asset {path}")))
    }
}

fn gray_png(width: u32, height: u32, values: &[u8]) -> Vec<u8> {
    let rgb = values
        .iter()
        .flat_map(|value| [*value; 3])
        .collect::<Vec<_>>();
    encode_png(width, height, &rgb)
}

fn assets(kinds: impl IntoIterator<Item = u64>) -> Arc<Assets> {
    let mut files = BTreeMap::from([(ANCHOR_ASSET.to_owned(), gray_png(8, 6, &anchor_pixels()))]);
    for kind in kinds {
        files.insert(picture_asset(kind), gray_png(6, 6, &picture_pixels(kind)));
    }
    Arc::new(Assets(files))
}

// One card on the frame: its index (its place in the grid), the kind of its picture and, for the
// colour test, the colour of its square.
#[derive(Clone, Copy)]
struct Card {
    index: u32,
    kind: u64,
    square: Option<[u8; 3]>,
}

fn origin(index: u32) -> (u32, u32) {
    (4 + 28 * (index % 8), 4 + 10 * (index / 8))
}

fn scene(cards: &[Card]) -> Scene {
    let mut pixels = vec![BACKGROUND; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize];
    let mut draw = |x: u32, y: u32, width: u32, values: &[u8]| {
        for (at, value) in values.iter().enumerate() {
            let (column, row) = (at as u32 % width, at as u32 / width);
            let index = (((y + row) * FRAME_WIDTH + x + column) * 3) as usize;
            pixels[index..index + 3].copy_from_slice(&[*value; 3]);
        }
    };
    for card in cards {
        let (x, y) = origin(card.index);
        draw(x, y, 8, &anchor_pixels());
        draw(x + 10, y, 6, &picture_pixels(card.kind));
    }
    for card in cards {
        if let Some(colour) = card.square {
            let (x, y) = origin(card.index);
            for row in 0..4 {
                for column in 0..4 {
                    let index = (((y + 1 + row) * FRAME_WIDTH + x + 18 + column) * 3) as usize;
                    pixels[index..index + 3].copy_from_slice(&colour);
                }
            }
        }
    }
    Scene::from_rgb8(FRAME_WIDTH, FRAME_HEIGHT, &pixels).expect("the frame")
}

// A template target `id` on the picture of `kind`, with `color_check` when given.
fn picture_target(id: &str, kind: u64, color_check: Option<Value>) -> Value {
    let mut target = json!({"type": "template", "id": id, "template_path": picture_asset(kind),
                            "region": "full_frame", "threshold": 0.9});
    if let Some(check) = color_check {
        target["color_check"] = check;
    }
    target
}

// A colour check of the target `id` on the square to the right of its picture, expecting `colour`.
fn square_check(id: &str, colour: [u8; 3]) -> Value {
    json!({
        "region": {"mode": "template_relative", "anchor_target_id": id,
                   "offset": {"x": 8, "y": 1}, "width": 4, "height": 4},
        "expected": colour, "max_distance": 30.0
    })
}

// A key entry of the pool.
fn key(id: &str, target: &str) -> Value {
    json!({"id": id, "target_id": target})
}

// An `other` entry of the pool.
fn other(target: &str) -> Value {
    json!({"other": true, "target_id": target})
}

// The picture identity `member` over the keys `keys`, scored in the region (9, 0, 8, 7).
fn member(keys: &[String]) -> Value {
    let entries = keys.iter().map(|id| json!({"id": id})).collect::<Vec<_>>();
    json!({
        "name": "member", "value": "identity", "region": {"x": 9, "y": 0, "width": 8, "height": 7},
        "identity": {"entries": entries, "recognition": {
            "kind": "icon_templates", "minimum_score_milli": THRESHOLD,
            "minimum_margin_milli": MARGIN
        }}
    })
}

// The cards' pack: the anchor, the pool's `targets`, and a `repeated_anchor` layout of `features`
// whose pool for `member` is `pool`.
fn cards_pack(targets: Vec<Value>, features: Vec<Value>, pool: Vec<Value>) -> Value {
    let mut all = vec![
        json!({"type": "template", "id": ANCHOR, "template_path": ANCHOR_ASSET,
                              "region": "full_frame", "threshold": 0.9}),
    ];
    all.extend(targets);
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": all,
        "candidate_layouts": [{
            "id": LAYOUT, "page_id": "neutral/cards", "kind": "repeated_anchor",
            "anchor": ANCHOR, "max_instances": 64, "order": "top_to_bottom",
            "suppress_iou_milli": 0,
            "instance_rect": {"x": 0, "y": 0, "width": 24, "height": 8},
            "click": {"x": 0, "y": 0, "width": 8, "height": 6},
            "features": features,
            "identity_templates": {"member": pool}
        }]
    })
}

fn keys(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("member-{index:02}"))
        .collect()
}

// The pack of a pool of `key_count` keys (kinds 0 to `key_count` - 1) and `other_count` others
// (the kinds after them), each picture its own target `card/pic_<kind>`.
fn pool_pack(key_count: usize, other_count: usize) -> Value {
    let total = key_count + other_count;
    let targets = (0..total)
        .map(|kind| picture_target(&format!("card/pic_{kind:03}"), kind as u64, None))
        .collect();
    let pool = (0..total)
        .map(|kind| {
            let target = format!("card/pic_{kind:03}");
            if kind < key_count {
                key(&format!("member-{kind:02}"), &target)
            } else {
                other(&target)
            }
        })
        .collect();
    cards_pack(targets, vec![member(&keys(key_count))], pool)
}

// The evaluator of `pack` with the assets of the anchor and the pictures of `kinds`, or why the
// pack is refused.
fn admitted(
    pack: &Value,
    kinds: impl IntoIterator<Item = u64>,
) -> RecognitionPackResult<RecognitionEvaluator> {
    RecognitionEvaluator::with_asset_resolver(
        load_pack_from_json_str(&pack.to_string())?,
        assets(kinds),
    )
}

// The admission's refusal of `pack`, as its message, which names each refused field's pointer.
fn refusal(pack: &Value) -> String {
    match admitted(pack, 0..200) {
        Ok(_) => panic!("the pack is admitted"),
        Err(error) => error.message().to_owned(),
    }
}

// The picture identity of the mapping tests: `member-a` (and its variant `short`) and `member-b`,
// the threshold 900 and the margin 50.
fn mapping_identity() -> CandidateIdentityDeclaration {
    serde_json::from_value(json!({
        "entries": [{"id": "member-a"}, {"id": "member-a", "variant": "short"}, {"id": "member-b"}],
        "recognition": {"kind": "icon_templates", "minimum_score_milli": THRESHOLD,
                        "minimum_margin_milli": MARGIN}
    }))
    .expect("the identity")
}

// A slot's entry of the equivalence table: its key, its variant and its target.
type SlotEntry<'a> = (&'a str, Option<&'a str>, &'a str);

fn pool_entry(id: Option<&str>, target: &str) -> CandidatePoolTemplate {
    CandidatePoolTemplate {
        id: id.map(str::to_owned),
        variant: None,
        other: id.is_none(),
        target_id: target.to_owned(),
    }
}

fn scored(scores: &[u16]) -> Vec<CandidatePoolScore> {
    scores
        .iter()
        .map(|score| CandidatePoolScore {
            score_milli: *score,
            colour_held: None,
        })
        .collect()
}

fn unknown(reason: CandidateUnknownReason) -> CandidateFeature {
    CandidateFeature::Unknown {
        reason,
        source: Some(CandidateIdentitySource::IconTemplate),
    }
}

// Test 1 (ruling 3; Addendum 1, A9): the one judgement of a read of a layout's pool. The pool is
// `member-a` (pic_a), `member-b` (pic_b) and two `other` entries (pic_x, pic_y), the threshold 900
// and the margin 50. A key above the threshold and clear of the best `other` by the margin is the
// key (also exactly at the margin); an `other` above the threshold and clear of the best key by
// the margin is none of the keys, its feature Unknown `out_of_domain` (also exactly at the
// threshold and the margin); a key and an `other` within the margin are unread `ambiguous`, two
// keys within the margin too, and nothing at the threshold `low_confidence`. An entry whose colour
// check failed does not compete: the best key's colour failing leaves the other key and the
// others below the threshold, unread, with one entry that did not compete. The reading carries the
// best key and the best `other` among the entries that competed, with the threshold and the
// margin.
#[test]
fn list_b_the_mapping_judges_a_key_none_of_the_keys_and_unread_from_the_pool() {
    let identity = mapping_identity();
    let pool = [
        pool_entry(Some("member-a"), "card/pic_a"),
        pool_entry(Some("member-b"), "card/pic_b"),
        pool_entry(None, "card/pic_x"),
        pool_entry(None, "card/pic_y"),
    ];
    let known = |scores: &[u16], expected: &str| {
        let (feature, reading) = identity.read_pool(&pool, &scored(scores));
        assert!(
            matches!(&feature, CandidateFeature::Identity { value, .. } if value == expected),
            "{scores:?}: {feature:?}"
        );
        assert_eq!(reading.mark, CandidateNameMark::Known, "{scores:?}");
        assert_eq!(reading.unread_cause, None, "{scores:?}");
    };
    known(&[990, 200, 300, 100], "member-a");
    known(&[990, 200, 940, 100], "member-a");
    known(&[200, 980, 300, 920], "member-b");

    for (scores, mark, reason) in [
        (
            [300, 200, 980, 100],
            CandidateNameMark::NoneOfTheKeys,
            CandidateUnknownReason::OutOfDomain,
        ),
        (
            [850, 200, 100, 900],
            CandidateNameMark::NoneOfTheKeys,
            CandidateUnknownReason::OutOfDomain,
        ),
        (
            [960, 200, 920, 100],
            CandidateNameMark::Unread,
            CandidateUnknownReason::Ambiguous,
        ),
        (
            [930, 200, 960, 100],
            CandidateNameMark::Unread,
            CandidateUnknownReason::Ambiguous,
        ),
        (
            [950, 920, 100, 100],
            CandidateNameMark::Unread,
            CandidateUnknownReason::Ambiguous,
        ),
        (
            [500, 400, 600, 100],
            CandidateNameMark::Unread,
            CandidateUnknownReason::LowConfidence,
        ),
        (
            [300, 200, 899, 100],
            CandidateNameMark::Unread,
            CandidateUnknownReason::LowConfidence,
        ),
        // Fix round 1 (the review): a key just under the threshold holds an `other` above it to
        // the margin.
        (
            [880, 200, 910, 100],
            CandidateNameMark::Unread,
            CandidateUnknownReason::LowConfidence,
        ),
    ] {
        let (feature, reading) = identity.read_pool(&pool, &scored(&scores));
        assert_eq!(feature, unknown(reason), "{scores:?}");
        assert_eq!(reading.mark, mark, "{scores:?}");
        assert_eq!(
            reading.unread_cause,
            Some(CandidateUnreadCause::of(reason)),
            "{scores:?}"
        );
    }

    let (_, reading) = identity.read_pool(&pool, &scored(&[990, 200, 300, 100]));
    assert_eq!(
        reading,
        CandidatePictureReading {
            mark: CandidateNameMark::Known,
            unread_cause: None,
            best_key: Some(CandidatePictureKey {
                key: "member-a".to_owned(),
                variant: None,
                target_id: "card/pic_a".to_owned(),
                score_milli: 990,
            }),
            best_other: Some(CandidatePictureOther {
                target_id: "card/pic_x".to_owned(),
                score_milli: 300,
            }),
            minimum_score_milli: THRESHOLD,
            minimum_margin_milli: MARGIN,
            colour_failed: 0,
        }
    );

    let mut colours = scored(&[990, 200, 300, 100]);
    colours[0].colour_held = Some(false);
    colours[1].colour_held = Some(true);
    let (feature, reading) = identity.read_pool(&pool, &colours);
    assert_eq!(feature, unknown(CandidateUnknownReason::LowConfidence));
    assert_eq!(reading.mark, CandidateNameMark::Unread);
    assert_eq!(reading.colour_failed, 1);
    assert_eq!(
        reading.best_key.map(|best| (best.key, best.score_milli)),
        Some(("member-b".to_owned(), 200)),
        "the entry whose colour failed is not the best key"
    );
}

// Test 2 (ruling 3): a pool without an `other` entry, whose entries all compete, answers the
// feature a slot's pool answers over the same scores, for every row of a table: clear keys, keys
// within the margin, a variant tied with its key, nothing at the threshold, exactly at the
// threshold and at the margin, a score above 1000 and a score missing.
#[test]
fn list_b_a_pool_without_other_entries_answers_as_a_slots_pool_over_a_table_of_inputs() {
    let identity = mapping_identity();
    let slot = [
        ("member-a", None, "card/pic_a"),
        ("member-b", None, "card/pic_b"),
        ("member-a", Some("short"), "card/pic_c"),
    ]
    .map(
        |(id, variant, target): SlotEntry<'_>| CandidateIdentityTemplate {
            id: id.to_owned(),
            variant: variant.map(str::to_owned),
            target_id: target.to_owned(),
        },
    );
    let pool = slot
        .iter()
        .map(|template| CandidatePoolTemplate {
            id: Some(template.id.clone()),
            variant: template.variant.clone(),
            other: false,
            target_id: template.target_id.clone(),
        })
        .collect::<Vec<_>>();
    let table: [&[u16]; 14] = [
        &[990, 200, 300],
        &[300, 990, 200],
        &[200, 300, 990],
        &[960, 920, 100],
        &[950, 100, 950],
        &[950, 950, 100],
        &[899, 100, 100],
        &[900, 100, 100],
        &[900, 850, 100],
        &[900, 851, 100],
        &[1000, 950, 949],
        &[0, 0, 0],
        &[1001, 0, 0],
        &[990, 200],
    ];
    for scores in table {
        assert_eq!(
            identity.read_pool(&pool, &scored(scores)).0,
            identity.map_icons(&slot, scores),
            "{scores:?}"
        );
    }
}

// Test 3 (rulings 1 and 2; Addendum 1, A5): the declaration. A `repeated_anchor` layout with a
// picture identity scored in a region and its pool of two keys and an `other` entry is admitted,
// and so is a pool of 128 entries; 129 entries, a pool without a key entry, a target named twice,
// an `other` entry that names an id, a template that declares scales or a colour check at a fixed
// rectangle, a region outside the instance rectangle and a picture identity without a region are
// each refused at their pointer. On the wire, an `other` that is not `true`, an `other` entry in a
// slot's pool and a pool of a `fixed_slots` layout are refused at their pointer.
#[test]
fn list_b_a_layout_pool_is_admitted_and_each_wrong_declaration_is_refused_at_its_pointer() {
    let targets = || {
        vec![
            picture_target("card/pic_000", 0, None),
            picture_target("card/pic_001", 1, None),
            picture_target("card/pic_002", 2, None),
        ]
    };
    let base_pool = || {
        vec![
            key("member-00", "card/pic_000"),
            key("member-01", "card/pic_001"),
            other("card/pic_002"),
        ]
    };
    let pool_pointer = "/candidate_layouts/0/identity_templates/member";
    admitted(
        &cards_pack(targets(), vec![member(&keys(2))], base_pool()),
        0..3,
    )
    .unwrap_or_else(|error| panic!("the pool is refused: {}", error.message()));
    admitted(&pool_pack(64, 64), 0..128)
        .unwrap_or_else(|error| panic!("a pool of 128 is refused: {}", error.message()));
    assert!(
        refusal(&pool_pack(64, 65)).contains(&format!(
            "{pool_pointer}: a layout's pool holds 1..=128 entries, got 129"
        )),
        "129 entries"
    );

    let refused = |pool: Vec<Value>, extra: Vec<Value>, expected: &str| {
        let mut all = targets();
        all.extend(extra);
        let message = refusal(&cards_pack(all, vec![member(&keys(2))], pool));
        assert!(message.contains(expected), "{expected}: {message}");
    };
    refused(
        vec![other("card/pic_000"), other("card/pic_001")],
        Vec::new(),
        &format!("{pool_pointer}: a layout's pool holds at least one key entry"),
    );
    refused(
        vec![key("member-00", "card/pic_000"), other("card/pic_000")],
        Vec::new(),
        &format!(
            "{pool_pointer}/1/target_id: target 'card/pic_000' is named by an earlier entry of the pool"
        ),
    );
    refused(
        vec![
            key("member-00", "card/pic_000"),
            json!({"other": true, "id": "member-01", "target_id": "card/pic_001"}),
        ],
        Vec::new(),
        &format!("{pool_pointer}/1: an other entry names no id and no variant"),
    );
    let mut scaled = picture_target("card/pic_scaled", 3, None);
    scaled["scales"] = json!({"factors_milli": [900, 1000]});
    refused(
        vec![key("member-00", "card/pic_scaled")],
        vec![scaled],
        &format!(
            "{pool_pointer}/0/target_id: template 'card/pic_scaled' of a layout's pool declares no mask, scales, unique or luma_floor"
        ),
    );
    let fixed_check = picture_target(
        "card/pic_fixed",
        3,
        Some(json!({"region": {"x": 0, "y": 0, "width": 4, "height": 4},
                    "expected": RED, "max_distance": 30.0})),
    );
    refused(
        vec![key("member-00", "card/pic_fixed")],
        vec![fixed_check],
        &format!(
            "{pool_pointer}/0/target_id: template 'card/pic_fixed' of a layout's pool may declare only a template_relative color_check"
        ),
    );
    // Fix round 1 (the review): a colour is measured only where the template reaches its own
    // threshold, so an entry that declares a colour check may not ask more of its template than
    // the identity asks (0.95 against the identity's 900).
    let mut strict = picture_target(
        "card/pic_strict",
        3,
        Some(square_check("card/pic_strict", RED)),
    );
    strict["threshold"] = json!(0.95);
    refused(
        vec![key("member-00", "card/pic_strict")],
        vec![strict],
        &format!(
            "{pool_pointer}/0/target_id: template 'card/pic_strict' of a layout's pool declares a color_check, so its threshold may not exceed the identity's minimum_score_milli"
        ),
    );

    let with_member = |member: Value| cards_pack(targets(), vec![member], base_pool());
    let mut outside = member(&keys(2));
    outside["region"] = json!({"x": 20, "y": 0, "width": 8, "height": 7});
    assert!(
        refusal(&with_member(outside)).contains(
            "/candidate_layouts/0/features/0/region: the feature region lies outside instance_rect"
        ),
        "a region outside the instance"
    );
    let mut without = member(&keys(2));
    without
        .as_object_mut()
        .expect("the feature")
        .remove("region");
    assert!(
        refusal(&with_member(without)).contains(
            "/candidate_layouts/0/features/0/region: a picture identity of a repeated_anchor layout declares the region its pool is scored in"
        ),
        "no region"
    );

    let wire = |pack: Value, pointer: &str| {
        let Err(error) = load_pack_from_json_str(&pack.to_string()) else {
            panic!("{pointer}: the wire takes it");
        };
        assert_eq!(
            error
                .declaration_issue()
                .map(|issue| issue.field_path.as_str()),
            Some(pointer),
            "{}",
            error.message()
        );
    };
    wire(
        cards_pack(
            targets(),
            vec![member(&keys(2))],
            vec![
                key("member-00", "card/pic_000"),
                json!({"other": false, "target_id": "card/pic_002"}),
            ],
        ),
        &format!("{pool_pointer}/1/other"),
    );
    let mut slotted = cards_pack(targets(), vec![member(&keys(2))], base_pool());
    let layout = &mut slotted["candidate_layouts"][0];
    for field in [
        "anchor",
        "max_instances",
        "order",
        "suppress_iou_milli",
        "instance_rect",
        "click",
    ] {
        layout.as_object_mut().expect("the layout").remove(field);
    }
    layout["kind"] = json!("fixed_slots");
    layout["features"][0]
        .as_object_mut()
        .expect("the feature")
        .remove("region");
    let slot_pool = layout
        .as_object_mut()
        .expect("the layout")
        .remove("identity_templates")
        .expect("the pool");
    layout["slots"] = json!([{
        "rect": {"x": 0, "y": 0, "width": 24, "height": 8},
        "click": {"x": 0, "y": 0, "width": 8, "height": 6},
        "targets": {},
        "identity_templates": slot_pool
    }]);
    wire(
        slotted.clone(),
        "/candidate_layouts/0/slots/0/identity_templates/member/2/other",
    );
    let layout = &mut slotted["candidate_layouts"][0];
    let slot_pool = layout["slots"][0]
        .as_object_mut()
        .expect("the slot")
        .remove("identity_templates")
        .expect("the slot's pool");
    layout["identity_templates"] = slot_pool;
    wire(slotted, "/candidate_layouts/0/identity_templates");
}

// Test 4 (ruling 10; Addendum 1, A1, A2 and A8): a pool of 128 entries (64 keys, 64 others) over a
// viewport of 64 instances. Cards 0 to 31 show the pictures of keys 0 to 31, cards 32 to 63 those
// of others 0 to 31. The projection is admitted within its limits (64 candidates, no OCR or NN
// evaluation, its compact JSON within 32 KiB): each key card's `member` is its key, each other card
// Unknown `out_of_domain`; the projection carries no recognition evidence of `member`; beside it
// the mapping judged each instance once, each key card Known with its key and target as the best
// key, each other card none of the keys with its target as the best `other`, and the one function
// that gives the mark answers the same; the pool's evaluations count in aggregate, 128 templates
// over 64 instances, 8192 evaluations.
#[test]
fn list_b_a_pool_of_128_over_64_instances_projects_within_the_projections_limits() {
    let evaluator =
        admitted(&pool_pack(64, 64), 0..128).unwrap_or_else(|error| panic!("{}", error.message()));
    let cards = (0..64)
        .map(|index| Card {
            index,
            kind: if index < 32 {
                u64::from(index)
            } else {
                64 + u64::from(index - 32)
            },
            square: None,
        })
        .collect::<Vec<_>>();
    let frame = scene(&cards);
    let (projection, _, judged) = evaluator
        .scene_context(&frame)
        .project_candidates_judged(LAYOUT)
        .unwrap_or_else(|failure| panic!("the projection: {failure}"));
    assert_eq!(projection.candidates().len(), 64);
    assert!(
        serde_json::to_vec(&projection)
            .expect("the projection's JSON")
            .len()
            <= CANDIDATE_PROJECTION_MAX_BYTES
    );
    assert!(
        projection
            .recognition_evidence()
            .iter()
            .all(|evidence| evidence.feature != "member"),
        "no evidence of the pool in the projection"
    );
    let layout = evaluator.candidate_layout(LAYOUT).expect("the layout");
    for (index, candidate) in (0_u32..).zip(projection.candidates()) {
        assert!(candidate.actionable, "card {index}");
        let samples = judged.samples(&candidate.id, "member");
        let [reading] = samples else {
            panic!("card {index}: one judged read: {samples:?}");
        };
        let mark = layout.name_mark(&projection, &judged, &candidate.id, "member");
        assert_eq!(mark, reading.mark, "card {index}");
        if index < 32 {
            let wanted = format!("member-{index:02}");
            assert!(
                matches!(candidate.features.get("member"),
                         Some(CandidateFeature::Identity { value, .. }) if *value == wanted),
                "card {index}: {:?}",
                candidate.features
            );
            assert_eq!(reading.mark, CandidateNameMark::Known, "card {index}");
            let best = reading.best_key.as_ref().expect("the best key");
            assert_eq!(
                (best.key.as_str(), best.target_id.clone()),
                (wanted.as_str(), format!("card/pic_{index:03}")),
                "card {index}"
            );
            assert!(best.score_milli >= 990, "card {index}: {best:?}");
        } else {
            assert_eq!(
                candidate.features.get("member"),
                Some(&unknown(CandidateUnknownReason::OutOfDomain)),
                "card {index}"
            );
            assert_eq!(
                reading.mark,
                CandidateNameMark::NoneOfTheKeys,
                "card {index}"
            );
            let best = reading.best_other.as_ref().expect("the best other");
            assert_eq!(
                best.target_id,
                format!("card/pic_{:03}", 64 + index - 32),
                "card {index}"
            );
        }
    }
    let [timing] = judged.timings() else {
        panic!("one pool's timing: {:?}", judged.timings());
    };
    assert_eq!(
        (
            timing.feature.as_str(),
            timing.templates,
            timing.instances,
            timing.evaluations
        ),
        ("member", 128, 64, 128 * 64)
    );
}

// Test 5 (Addendum 1, A9): pictures that differ only in colour are told apart. `member-00` and
// `member-01` share one picture (kind 0); `member-00`'s target checks a red square beside it,
// `member-01`'s a blue one; an `other` entry is the picture of kind 1. Card 0 shows the picture
// with a red square, card 1 with a blue one, card 2 with a green one: card 0 reads `member-00`
// (`member-01` did not compete), card 1 `member-01`, and card 2, whose colour matches neither, is
// unread `low_confidence`, both its keys out of the competition.
#[test]
fn list_b_pictures_of_one_drawing_in_two_colours_are_told_apart_by_their_colour_checks() {
    let pack = cards_pack(
        vec![
            picture_target("card/red", 0, Some(square_check("card/red", RED))),
            picture_target("card/blue", 0, Some(square_check("card/blue", BLUE))),
            picture_target("card/pic_001", 1, None),
        ],
        vec![member(&keys(2))],
        vec![
            key("member-00", "card/red"),
            key("member-01", "card/blue"),
            other("card/pic_001"),
        ],
    );
    let evaluator = admitted(&pack, 0..2).unwrap_or_else(|error| panic!("{}", error.message()));
    let cards = [RED, BLUE, GREEN]
        .into_iter()
        .zip(0_u32..)
        .map(|(colour, index)| Card {
            index,
            kind: 0,
            square: Some(colour),
        })
        .collect::<Vec<_>>();
    let frame = scene(&cards);
    let (projection, _, judged) = evaluator
        .scene_context(&frame)
        .project_candidates_judged(LAYOUT)
        .unwrap_or_else(|failure| panic!("the projection: {failure}"));
    assert_eq!(projection.candidates().len(), 3);
    let reading = |index: usize| {
        let candidate = &projection.candidates()[index];
        let [judged_read] = judged.samples(&candidate.id, "member") else {
            panic!("card {index}: one judged read");
        };
        (
            candidate.features.get("member").cloned(),
            judged_read.clone(),
        )
    };
    for (index, wanted) in [(0, "member-00"), (1, "member-01")] {
        let (feature, read) = reading(index);
        assert!(
            matches!(&feature, Some(CandidateFeature::Identity { value, .. }) if value == wanted),
            "card {index}: {feature:?}"
        );
        assert_eq!(read.mark, CandidateNameMark::Known, "card {index}");
        assert_eq!(read.colour_failed, 1, "card {index}");
        assert_eq!(
            read.best_key.map(|best| best.key),
            Some(wanted.to_owned()),
            "card {index}"
        );
    }
    let (feature, read) = reading(2);
    assert_eq!(
        feature,
        Some(unknown(CandidateUnknownReason::LowConfidence))
    );
    assert_eq!(read.mark, CandidateNameMark::Unread);
    assert_eq!(read.colour_failed, 2);
    assert_eq!(read.best_key, None, "no key competed");
}

// Fix round 1 (the review): a colour is measured only where the template reaches its target's own
// threshold, and a colour that was not measured is no failed check. The pool is test 5's: two keys
// of one picture (kind 0), one checking a red square and one a blue, and the `other` picture of
// kind 1. A card shows the picture of kind 1 beside a red square: neither key's template reaches
// its threshold there, so neither colour was measured; both keys compete on their scores (no check
// failed, and the best key is one of them), the `other` picture is clear of them by the margin,
// and the card is none of the keys.
#[test]
fn list_b_a_colour_that_was_not_measured_is_no_failed_check() {
    let pack = cards_pack(
        vec![
            picture_target("card/red", 0, Some(square_check("card/red", RED))),
            picture_target("card/blue", 0, Some(square_check("card/blue", BLUE))),
            picture_target("card/pic_001", 1, None),
        ],
        vec![member(&keys(2))],
        vec![
            key("member-00", "card/red"),
            key("member-01", "card/blue"),
            other("card/pic_001"),
        ],
    );
    let evaluator = admitted(&pack, 0..2).unwrap_or_else(|error| panic!("{}", error.message()));
    let frame = scene(&[Card {
        index: 0,
        kind: 1,
        square: Some(RED),
    }]);
    let (projection, _, judged) = evaluator
        .scene_context(&frame)
        .project_candidates_judged(LAYOUT)
        .unwrap_or_else(|failure| panic!("the projection: {failure}"));
    assert_eq!(projection.candidates().len(), 1);
    let candidate = &projection.candidates()[0];
    let [read] = judged.samples(&candidate.id, "member") else {
        panic!("one judged read");
    };
    assert_eq!(read.mark, CandidateNameMark::NoneOfTheKeys);
    assert_eq!(
        read.colour_failed, 0,
        "no colour was measured, so none failed"
    );
    assert!(read.best_key.is_some(), "the keys competed on their scores");
    assert_eq!(
        read.best_other.as_ref().map(|best| best.target_id.as_str()),
        Some("card/pic_001")
    );
}

// An uncompressed RGB PNG.
fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for row in pixels.chunks((width * 3) as usize) {
        scanlines.push(0);
        scanlines.extend_from_slice(row);
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    let mut zlib = vec![0x78, 0x01];
    let blocks = scanlines.chunks(65_535).collect::<Vec<_>>();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &scanlines {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}
