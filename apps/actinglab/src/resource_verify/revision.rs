// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #464 P2 (`contracts/resource-verify.md`, "`standard-pack build`", "The `package`
//! revision"): the `package` revision a standard-pack bundle declares, computed from its content.
//! `contracts/component-interfaces.md` says it: "A bundle writes `package`: its `max` is the
//! revision its packs need. Give `min` the same value." Each row of the interface's revisions
//! names what of a bundle needs it; [`content_revision`] is the highest revision any entry of the
//! zip needs, and 3, the bundle formats of Runtime v0.12.0, when none needs a later one.
//!
//! - **The root** ([`entry_revision`]): the notices directory, 10 (Workflow #444); `goals.json`,
//!   12 (Workflow #455 G-R1).
//! - **A pack's loading copy** (`resources/loading.json`, `resources/loading/`), 6 (Workflow #407
//!   LOAD-1 and LOAD-2).
//! - **A member of a pack's JSON file** ([`PACK_MEMBERS`]), read as JSON: a member of a name,
//!   where the row says only there, of a value where the row says only that value; and a
//!   `repeated_anchor` layout whose anchor declares a mask or scales ([`layout_anchor_masked`]).
//!
//! A member is found by its name and its place in the document, not by the declaration reader:
//! the same name elsewhere in a pack's JSON (a note, say) counts as well, so a revision may be
//! declared higher than the pack needs, never lower than a row names. A JSON file that does not
//! read needs nothing here: the format step refuses it. A feature that adds a `package` revision
//! adds its row here, in its own slice.

use super::bundle::PACKAGE_REVISION;
use actingcommand_contract::{
    GOALS_PACKAGE_REVISION, NOTICES_PACKAGE_REVISION, STANDARD_PACK_GOALS_FILE,
    notice_relative_path,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The directory of a bundle's packs, `packs/<package_id>/`.
const PACKS_PREFIX: &str = "packs/";
/// Workflow #407 LOAD-1: a pack's loading copy, its file and the directory of its templates.
const LOADING_COPY_FILE: &str = "resources/loading.json";
const LOADING_COPY_PREFIX: &str = "resources/loading/";
/// Revision 6: a pack's loading copy, among the rest of the row (Workflow #407 LOAD-1, LOAD-2).
const LOADING_COPY_REVISION: u64 = 6;
/// Revision 7: a `repeated_anchor` layout whose anchor declares a mask or scales (Workflow #404
/// FS-1, FS-2).
const LAYOUT_ANCHOR_REVISION: u64 = 7;
/// The members of an `anchors[]` target that declare a mask or scales.
const ANCHOR_MASK_MEMBERS: [&str; 3] = ["mask", "maskRange", "scales"];
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

/// Where a member of [`PACK_MEMBERS`] needs its revision.
#[derive(Debug, Clone, Copy)]
enum Place {
    /// Wherever it stands.
    Anywhere,
    /// Only in an object that the member of this name holds, itself or as an element of its
    /// array.
    Under(&'static str),
}

/// One member of a pack's JSON file that needs a `package` revision above 3.
#[derive(Debug, Clone, Copy)]
struct PackMember {
    name: &'static str,
    /// The text it must hold; `None`: whatever it holds.
    holds: Option<&'static str>,
    place: Place,
    revision: u64,
}

impl PackMember {
    const fn named(name: &'static str, revision: u64) -> Self {
        Self {
            name,
            holds: None,
            place: Place::Anywhere,
            revision,
        }
    }

    const fn under(name: &'static str, holder: &'static str, revision: u64) -> Self {
        Self {
            name,
            holds: None,
            place: Place::Under(holder),
            revision,
        }
    }

    const fn holding(name: &'static str, text: &'static str, revision: u64) -> Self {
        Self {
            name,
            holds: Some(text),
            place: Place::Anywhere,
            revision,
        }
    }

    /// Whether the member `name` holding `member`, in an object the member `holder` holds,
    /// needs this row's revision.
    fn needs(&self, name: &str, member: &Value, holder: Option<&str>) -> bool {
        self.name == name
            && self.holds.is_none_or(|text| member.as_str() == Some(text))
            && match self.place {
                Place::Anywhere => true,
                Place::Under(under) => holder == Some(under),
            }
    }
}

/// The members of a pack's JSON files that the `package` rows 4 to 11 name, each with its row.
const PACK_MEMBERS: [PackMember; 17] = [
    // 4, Workflow #411 B15: a `scheduling_outcome` mapping's `remedy`, `control.json`'s `provides`.
    PackMember::under("remedy", "mappings", 4),
    PackMember::named("provides", 4),
    // 5, Workflow #406 T3: the click `single_touch_path_drag_v1`.
    PackMember::holding("kind", "single_touch_path_drag_v1", 5),
    // 6, Workflow #357 DB-1 and DB-2: a selection document's `tables` and the value source
    // `table`; #428 OCR-SCALE-1: `post_admission_ocr`; ACT-4: `expect_after.target_present` and
    // `target_gone`; #407 LOAD-2: `expect_after.loading` and `expect_after.wait_page`.
    PackMember::named("tables", 6),
    PackMember::holding("source", "table", 6),
    PackMember::named("post_admission_ocr", 6),
    PackMember::under("target_present", "expect_after", 6),
    PackMember::under("target_gone", "expect_after", 6),
    PackMember::under("loading", "expect_after", 6),
    PackMember::under("wait_page", "expect_after", 6),
    // 7, Workflow #404 FS-2 and FS-3: the feature value `anchor_milli`, a select step's
    // `confirm: "track"` and its `track_px` (and FS-1, FS-2: [`layout_anchor_masked`]).
    PackMember {
        name: "value",
        holds: Some("anchor_milli"),
        place: Place::Under("features"),
        revision: 7,
    },
    PackMember {
        name: "confirm",
        holds: Some("track"),
        place: Place::Under("select"),
        revision: 7,
    },
    PackMember::under("track_px", "select", 7),
    // 8, Workflow #428 S168-4 and S168-5a: an operation's `read_list`, the value source
    // `table_max_over_list`.
    PackMember::named("read_list", 8),
    PackMember::holding("source", "table_max_over_list", 8),
    // 9, Workflow #428 S168-LB1b: `table_max_over_list`'s `lower_bound_column`.
    PackMember::named("lower_bound_column", 9),
    // 11, Workflow #450 FL-1: a task's `field_lists`.
    PackMember::named("field_lists", 11),
];

/// The `package` revision the zip's `entries` (its `interfaces.json` aside) need: the highest any
/// entry needs, [`PACKAGE_REVISION`] when none needs a later one.
pub(super) fn content_revision(entries: &BTreeMap<String, Vec<u8>>) -> u64 {
    entries
        .iter()
        .map(|(name, bytes)| entry_revision(name, bytes))
        .fold(PACKAGE_REVISION, u64::max)
}

/// The revision the zip entry `name` with the bytes `bytes` needs.
fn entry_revision(name: &str, bytes: &[u8]) -> u64 {
    if notice_relative_path(name).is_some() {
        return NOTICES_PACKAGE_REVISION;
    }
    if name == STANDARD_PACK_GOALS_FILE {
        return GOALS_PACKAGE_REVISION;
    }
    // A pack's file: `packs/<package_id>/<path within the pack>`.
    let Some((_, within)) = name
        .strip_prefix(PACKS_PREFIX)
        .and_then(|rest| rest.split_once('/'))
    else {
        return PACKAGE_REVISION;
    };
    if within == LOADING_COPY_FILE || within.starts_with(LOADING_COPY_PREFIX) {
        return LOADING_COPY_REVISION;
    }
    if !within.ends_with(".json") {
        return PACKAGE_REVISION;
    }
    match serde_json::from_slice::<Value>(bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(bytes)) {
        Ok(document) => {
            let mut revision = PACKAGE_REVISION;
            walk(&document, None, &mut revision);
            revision
        }
        Err(_) => PACKAGE_REVISION,
    }
}

/// Raises `revision` to what `value`, held by the member `holder`, and everything below it need.
fn walk(value: &Value, holder: Option<&str>, revision: &mut u64) {
    match value {
        Value::Object(members) => {
            if layout_anchor_masked(members) {
                *revision = (*revision).max(LAYOUT_ANCHOR_REVISION);
            }
            for (name, member) in members {
                for row in &PACK_MEMBERS {
                    if row.needs(name, member, holder) {
                        *revision = (*revision).max(row.revision);
                    }
                }
                walk(member, Some(name.as_str()), revision);
            }
        }
        Value::Array(elements) => {
            for element in elements {
                walk(element, holder, revision);
            }
        }
        _ => {}
    }
}

/// Revision 7 (Workflow #404 FS-1, FS-2): the object `members` (a task) declares a
/// `repeated_anchor` layout whose anchor, a target of its `anchors`, declares a mask or scales.
fn layout_anchor_masked(members: &Map<String, Value>) -> bool {
    let Some(layouts) = members.get("candidate_layouts").and_then(Value::as_array) else {
        return false;
    };
    let anchors = members
        .get("anchors")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    layouts
        .iter()
        .filter(|layout| layout.get("kind").and_then(Value::as_str) == Some("repeated_anchor"))
        .filter_map(|layout| layout.get("anchor").and_then(Value::as_str))
        .any(|anchor| {
            anchors.iter().any(|target| {
                target.get("id").and_then(Value::as_str) == Some(anchor)
                    && ANCHOR_MASK_MEMBERS
                        .iter()
                        .any(|member| target.get(*member).is_some())
            })
        })
}
