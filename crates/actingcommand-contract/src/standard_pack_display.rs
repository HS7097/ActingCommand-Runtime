// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401 DA-4 (model 392 v1.2 §4.1, §0.2 C9, §12 row DA-4): a game root's display
//! declaration, `display-layout.json` (`actingcommand.display-layout.v1`), beside `bundle.json`.
//! It says how the game's interface follows a display that is not its packages' base size: the
//! side of the layout area the scale factor is measured on (`scale.by`), whether that area is the
//! whole screen or the safe area (`scale.area`), which area the nine anchors sit on
//! (`anchor_area`), and the radius of the nearest correction (`correction.radius_px`, base
//! pixels, 0 for none, at most 64). Every member is required and no other is read: the Runtime
//! has no default for any of them.
//!
//! This module reads and checks the file only. The root reader
//! ([`crate::read_standard_pack_root`]) runs it for every reader (the Runtime's startup and run
//! preparation, check-config, `actingctl packs`, `package bundle --display-layout` and
//! `actinglab standard-pack check`), so no reader accepts what another refuses. The display
//! adapter consumes the parsed declaration (DA-2a and later). The Runtime names no game here.

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue, check_game};
use crate::{DisplayArea, DisplayScaleBy};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The game root's display declaration (model 392 §4.1).
pub const STANDARD_PACK_DISPLAY_LAYOUT_FILE: &str = "display-layout.json";
/// The largest nearest-correction radius, in base pixels: the bound of the declaration's shape
/// (the correction's `dx`, `dy` stay within it), not a tunable (model 392 §1.1, §4.1).
pub const MAX_DISPLAY_CORRECTION_RADIUS_PX: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StandardPackDisplayLayoutVersion {
    #[serde(rename = "actingcommand.display-layout.v1")]
    V1,
}

/// `display-layout.json` (model 392 §4.1): the game's scale rule, anchor area and nearest
/// correction radius.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPackDisplayLayout {
    pub schema_version: StandardPackDisplayLayoutVersion,
    /// The game directory the file sits in (and `bundle.json`'s `game`).
    pub game: String,
    pub scale: DisplayLayoutScale,
    /// The area the nine anchors sit on.
    pub anchor_area: DisplayArea,
    pub correction: DisplayLayoutCorrection,
}

/// How the scale factor `s` is measured (model 392 §4.2): the layout area's height or width over
/// the base's, the layout area the whole screen or the safe area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayLayoutScale {
    pub by: DisplayScaleBy,
    pub area: DisplayArea,
}

/// The nearest correction (model 392 §5.3): how far around a mapped search hint a translated map
/// looks, in base pixels; 0 turns it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayLayoutCorrection {
    pub radius_px: u32,
}

/// `display-layout.json`'s bytes, decoded and checked on their own (model 392 §4.1): JSON of the
/// file's shape and version with every member and no other (`malformed` at the first member that
/// is unknown, missing or not of its type, empty for the whole document), `game` the game
/// directory `game` (any valid game identifier when `None`, for a repository root;
/// `game_mismatch`), and `correction.radius_px` within 0..=64 (`display_layout_invalid`).
pub fn parse_standard_pack_display_layout(
    bytes: &[u8],
    game: Option<&str>,
) -> Result<StandardPackDisplayLayout, StandardPackRootIssue> {
    let file = STANDARD_PACK_DISPLAY_LAYOUT_FILE;
    let layout: StandardPackDisplayLayout = serde_json::from_slice(bytes).map_err(|_| {
        StandardPackRootIssue::new(file, StandardPackProblem::Malformed, misfit_pointer(bytes))
    })?;
    check_game(file, &layout.game, game)?;
    if layout.correction.radius_px > MAX_DISPLAY_CORRECTION_RADIUS_PX {
        return Err(StandardPackRootIssue::new(
            file,
            StandardPackProblem::DisplayLayoutInvalid,
            "/correction/radius_px",
        ));
    }
    Ok(layout)
}

/// One member of the file's shape: a value of one type, or an object of its own members.
struct Member {
    name: &'static str,
    shape: Shape,
}

enum Shape {
    /// A value, which the function tells to be of the member's type.
    Leaf(fn(&Value) -> bool),
    /// An object of these members.
    Object(&'static [Member]),
}

/// The members of `scale`.
const SCALE: &[Member] = &[
    Member {
        name: "by",
        shape: Shape::Leaf(fits::<DisplayScaleBy>),
    },
    Member {
        name: "area",
        shape: Shape::Leaf(fits::<DisplayArea>),
    },
];

/// The members of `correction`.
const CORRECTION: &[Member] = &[Member {
    name: "radius_px",
    shape: Shape::Leaf(fits::<u32>),
}];

/// The members of `display-layout.json`, in the order they are checked.
const DOCUMENT: &[Member] = &[
    Member {
        name: "schema_version",
        shape: Shape::Leaf(fits::<StandardPackDisplayLayoutVersion>),
    },
    Member {
        name: "game",
        shape: Shape::Leaf(fits::<String>),
    },
    Member {
        name: "scale",
        shape: Shape::Object(SCALE),
    },
    Member {
        name: "anchor_area",
        shape: Shape::Leaf(fits::<DisplayArea>),
    },
    Member {
        name: "correction",
        shape: Shape::Object(CORRECTION),
    },
];

fn fits<T: DeserializeOwned>(value: &Value) -> bool {
    T::deserialize(value).is_ok()
}

/// Where the bytes the typed decoding refused first leave the file's shape: an object's first
/// unknown member, else its first member (in [`DOCUMENT`]'s order, depth first) that is missing
/// or not of its type. Empty for the whole document: no JSON, no object, or a refusal no member
/// explains (a repeated member).
fn misfit_pointer(bytes: &[u8]) -> String {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|document| misfit(&document, "", DOCUMENT))
        .unwrap_or_default()
}

fn misfit(value: &Value, pointer: &str, members: &[Member]) -> Option<String> {
    let Value::Object(object) = value else {
        return Some(pointer.to_owned());
    };
    if let Some(unknown) = object
        .keys()
        .find(|key| members.iter().all(|member| member.name != key.as_str()))
    {
        return Some(format!("{pointer}/{}", segment(unknown)));
    }
    members.iter().find_map(|member| {
        let at = format!("{pointer}/{}", member.name);
        match (object.get(member.name), &member.shape) {
            (None, _) => Some(at),
            (Some(value), Shape::Leaf(fits)) => (!fits(value)).then_some(at),
            (Some(value), Shape::Object(members)) => misfit(value, &at, members),
        }
    })
}

/// One JSON pointer segment (RFC 6901).
fn segment(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    //! Workflow #392 DA-4, test rows DA4-T1 and DA4-T2 (model 392 v1.2 §13; §4.1): the display
    //! declaration of a synthetic `neutral` game root is read by the one root reader when it keeps
    //! its rules, each violation is refused with its `standard_pack_problem` at its JSON pointer,
    //! and the file makes a directory a game root as the other root files do.

    use super::*;
    use crate::{
        STANDARD_PACK_BUNDLE_FILE, StandardPackRoot, StandardPackRootBytes, read_standard_pack_root,
    };
    use serde_json::json;

    const GAME: &str = "neutral";

    fn bundle() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": GAME,
            "packs": [{
                "package_id": "neutral.test.task",
                "server": "test",
                "entry_task_id": "task",
                "path": "packs/neutral.test.task",
                "file_count": 3,
                "byte_count": 64
            }]
        }))
        .expect("bundle JSON")
    }

    fn layout() -> Value {
        json!({
            "schema_version": "actingcommand.display-layout.v1",
            "game": GAME,
            "scale": {"by": "height", "area": "screen"},
            "anchor_area": "screen",
            "correction": {"radius_px": 16}
        })
    }

    /// The layout with the member at `pointer` replaced by `value`.
    fn replaced(pointer: &str, value: Value) -> Value {
        let mut layout = layout();
        *layout.pointer_mut(pointer).expect("a member") = value;
        layout
    }

    /// The layout without the member `name` of the object at `parent`.
    fn without(parent: &str, name: &str) -> Value {
        let mut layout = layout();
        layout
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .expect("an object")
            .shift_remove(name)
            .expect("a member");
        layout
    }

    /// The layout with one more member `name` in the object at `parent`.
    fn added(parent: &str, name: &str, value: Value) -> Value {
        let mut layout = layout();
        layout
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .expect("an object")
            .insert(name.to_owned(), value);
        layout
    }

    /// The game root of `bundle.json` and `display-layout.json` and nothing else, as every reader
    /// reads it.
    fn read_root(
        bundle: Option<&[u8]>,
        display_layout: &Value,
    ) -> Result<Option<StandardPackRoot>, StandardPackRootIssue> {
        let bytes = serde_json::to_vec(display_layout).expect("layout JSON");
        read_standard_pack_root(
            StandardPackRootBytes {
                bundle,
                display_layout: Some(&bytes),
                ..StandardPackRootBytes::default()
            },
            GAME,
        )
    }

    fn issue(problem: StandardPackProblem, pointer: &str) -> StandardPackRootIssue {
        StandardPackRootIssue {
            file: STANDARD_PACK_DISPLAY_LAYOUT_FILE,
            problem,
            pointer: pointer.to_owned(),
        }
    }

    /// DA4-T1: a valid `display-layout.json` is read by `read_standard_pack_root` with every
    /// member as written; radius 64, the bound, and radius 0 pass; a repository root's file
    /// names its game itself.
    #[test]
    fn da4_t1_a_valid_display_layout_is_read_by_the_root_reader() {
        let bundle = bundle();
        let read = |layout: &Value| {
            read_root(Some(&bundle), layout)
                .expect("a valid game root")
                .expect("a game root")
        };
        let root = read(&layout());
        assert_eq!(root.registry.len(), 1);
        assert!(root.pages.is_none());
        assert_eq!(
            root.display_layout,
            Some(StandardPackDisplayLayout {
                schema_version: StandardPackDisplayLayoutVersion::V1,
                game: GAME.to_owned(),
                scale: DisplayLayoutScale {
                    by: DisplayScaleBy::Height,
                    area: DisplayArea::Screen,
                },
                anchor_area: DisplayArea::Screen,
                correction: DisplayLayoutCorrection { radius_px: 16 },
            })
        );
        let mut other = replaced("/scale", json!({"by": "width", "area": "safe_area"}));
        other["anchor_area"] = json!("safe_area");
        other["correction"]["radius_px"] = json!(64);
        let other = read(&other).display_layout.expect("the declaration");
        assert_eq!(
            (other.scale, other.anchor_area, other.correction.radius_px),
            (
                DisplayLayoutScale {
                    by: DisplayScaleBy::Width,
                    area: DisplayArea::SafeArea,
                },
                DisplayArea::SafeArea,
                64
            )
        );
        assert_eq!(
            read(&replaced("/correction/radius_px", json!(0)))
                .display_layout
                .map(|layout| layout.correction.radius_px),
            Some(0)
        );
        parse_standard_pack_display_layout(
            &serde_json::to_vec(&layout()).expect("layout JSON"),
            None,
        )
        .expect("a declaration read without its directory");
    }

    /// DA4-T1: a missing member and an unknown one are `malformed` at that member, at any depth;
    /// a member not of its type or version is `malformed` at it; the whole document when it is
    /// no object or no JSON; another game is `game_mismatch`; radius 65 is
    /// `display_layout_invalid`. The root reader refuses each as the parser does.
    #[test]
    fn da4_t1_each_display_layout_violation_is_its_problem_at_its_pointer() {
        use StandardPackProblem as Problem;
        let cases = [
            (
                "no correction",
                without("", "correction"),
                issue(Problem::Malformed, "/correction"),
            ),
            (
                "no scale.by",
                without("/scale", "by"),
                issue(Problem::Malformed, "/scale/by"),
            ),
            (
                "no correction.radius_px",
                without("/correction", "radius_px"),
                issue(Problem::Malformed, "/correction/radius_px"),
            ),
            (
                "no schema_version",
                without("", "schema_version"),
                issue(Problem::Malformed, "/schema_version"),
            ),
            (
                "an unknown member",
                added("", "extra", json!(true)),
                issue(Problem::Malformed, "/extra"),
            ),
            (
                "an unknown member of scale",
                added("/scale", "origin", json!("top_left")),
                issue(Problem::Malformed, "/scale/origin"),
            ),
            (
                "an unknown member whose name holds a slash",
                added("/correction", "max/px", json!(8)),
                issue(Problem::Malformed, "/correction/max~1px"),
            ),
            (
                "another version",
                replaced("/schema_version", json!("actingcommand.display-layout.v2")),
                issue(Problem::Malformed, "/schema_version"),
            ),
            (
                "scale.by not a side",
                replaced("/scale/by", json!("diagonal")),
                issue(Problem::Malformed, "/scale/by"),
            ),
            (
                "anchor_area not an area",
                replaced("/anchor_area", json!("window")),
                issue(Problem::Malformed, "/anchor_area"),
            ),
            (
                "scale not an object",
                replaced("/scale", json!("height")),
                issue(Problem::Malformed, "/scale"),
            ),
            (
                "radius not an integer",
                replaced("/correction/radius_px", json!(1.5)),
                issue(Problem::Malformed, "/correction/radius_px"),
            ),
            (
                "the document not an object",
                json!([layout()]),
                issue(Problem::Malformed, ""),
            ),
            (
                "another game",
                replaced("/game", json!("other")),
                issue(Problem::GameMismatch, "/game"),
            ),
            (
                "radius 65",
                replaced("/correction/radius_px", json!(65)),
                issue(Problem::DisplayLayoutInvalid, "/correction/radius_px"),
            ),
        ];
        let bundle = bundle();
        for (case, value, expected) in cases {
            let bytes = serde_json::to_vec(&value).expect("case JSON");
            assert_eq!(
                parse_standard_pack_display_layout(&bytes, Some(GAME)),
                Err(expected.clone()),
                "{case}"
            );
            assert_eq!(
                read_root(Some(&bundle), &value).map(|_| ()),
                Err(expected),
                "{case}: the root reader"
            );
        }
        assert_eq!(
            parse_standard_pack_display_layout(b"{", Some(GAME)),
            Err(issue(Problem::Malformed, "")),
            "not JSON"
        );
    }

    /// DA4-T2: a directory whose only root file is `display-layout.json` is a game root without
    /// its `bundle.json` (`bundle_missing`, the bundle's file, the whole document); with
    /// `bundle.json` it needs no `pages.json`.
    #[test]
    fn da4_t2_display_layout_without_bundle_json_is_bundle_missing_and_needs_no_pages() {
        assert_eq!(
            read_root(None, &layout()).map(|_| ()),
            Err(StandardPackRootIssue {
                file: STANDARD_PACK_BUNDLE_FILE,
                problem: StandardPackProblem::BundleMissing,
                pointer: String::new(),
            })
        );
        let bundle = bundle();
        let root = read_root(Some(&bundle), &layout())
            .expect("no pages.json is needed")
            .expect("a game root");
        assert!(root.pages.is_none() && root.interrupts.is_none() && root.navigation.is_none());
        assert!(root.display_layout.is_some());
    }
}
