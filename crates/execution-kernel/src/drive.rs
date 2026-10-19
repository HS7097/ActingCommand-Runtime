// SPDX-License-Identifier: AGPL-3.0-only

//! Pure semantic-drive planning owned by the execution kernel.

use crate::codes::ExecutionKernelCode;
use actingcommand_contract::InputAction;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Code;
use actingcommand_recognition_pack::PackRect;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveDecisionErrorKind {
    InvalidInput,
    SafetyBlocked,
    PackageInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveDecisionError {
    kind: DriveDecisionErrorKind,
    code: Code,
    message: String,
    required_conditions: Vec<&'static str>,
}

impl DriveDecisionError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: DriveDecisionErrorKind::InvalidInput,
            code: ExecutionKernelCode::DrivePlanInvalid.code(),
            message: message.into(),
            required_conditions: Vec::new(),
        }
    }

    fn safety(
        code: Code,
        message: impl Into<String>,
        required_conditions: Vec<&'static str>,
    ) -> Self {
        Self {
            kind: DriveDecisionErrorKind::SafetyBlocked,
            code,
            message: message.into(),
            required_conditions,
        }
    }

    fn package(message: impl Into<String>) -> Self {
        Self {
            kind: DriveDecisionErrorKind::PackageInvalid,
            code: ExecutionKernelCode::DrivePackageInvalid.code(),
            message: message.into(),
            required_conditions: Vec::new(),
        }
    }

    pub const fn kind(&self) -> DriveDecisionErrorKind {
        self.kind
    }

    pub const fn code(&self) -> Code {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn required_conditions(&self) -> &[&'static str] {
        &self.required_conditions
    }
}

impl fmt::Display for DriveDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DriveDecisionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DrivePoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveSemanticInput {
    Tap {
        rect: PackRect,
        point: DrivePoint,
    },
    TargetCenter {
        target_id: String,
    },
    /// Workflow #390 LAB-2 item 7 (REC-4 `offset`): the box at the top-left corner of the box
    /// `target_id` matches plus `offset`, the offset scaled by the factor a template with `scales`
    /// matched at ([`drive_offset_rect`]).
    TargetOffset {
        target_id: String,
        offset: PackRect,
    },
    Drag {
        from_rect: PackRect,
        to_rect: PackRect,
        from: DrivePoint,
        to: DrivePoint,
        duration_ms: u64,
    },
}

impl DriveSemanticInput {
    /// Converts a resolved semantic action into the typed Runtime input contract.
    pub fn resolved_input_action(&self) -> Result<InputAction, DriveDecisionError> {
        let action = match self {
            Self::Tap { point, .. } => InputAction::Tap {
                x: point.x,
                y: point.y,
            },
            Self::TargetCenter { .. } => {
                return Err(DriveDecisionError::invalid(
                    "target_center semantic input must be resolved before execution",
                ));
            }
            Self::TargetOffset { .. } => {
                return Err(DriveDecisionError::invalid(
                    "target_offset semantic input must be resolved before execution",
                ));
            }
            // #411 Lab L3: a drag that does not move (one point in one rectangle, the reader's form
            // of a long press) is pressed as a long tap over its duration, as the kernel presses a
            // pack's long press; every other drag is one swipe.
            Self::Drag {
                from_rect,
                to_rect,
                from,
                to,
                duration_ms,
            } if from == to && from_rect == to_rect => InputAction::LongTap {
                x: from.x,
                y: from.y,
                duration_ms: *duration_ms,
            },
            Self::Drag {
                from,
                to,
                duration_ms,
                ..
            } => InputAction::Swipe {
                x1: from.x,
                y1: from.y,
                x2: to.x,
                y2: to.y,
                duration_ms: *duration_ms,
            },
        };
        action.validate().map_err(|error| {
            DriveDecisionError::invalid(format!(
                "resolved semantic input is invalid: {}",
                error.code()
            ))
        })?;
        Ok(action)
    }
}

/// #411 Lab L2: the click kind of a segmented drag, the gesture the kernel runs as
/// `InputAction::SingleTouchDragWithVerticalBrakeV1`.
const SEGMENTED_DRAG_KIND: &str = "single_touch_drag_with_vertical_brake_v1";

/// #411 Lab L2: the members of a segmented drag click in the form source compilation writes it
/// into the navigation (pack-containment `source/declarations.rs`, the canonical click).
const SEGMENTED_DRAG_FIELDS: [&str; 7] = [
    "kind",
    "from_rect",
    "corner_rect",
    "horizontal_duration_ms",
    "corner_hold_ms",
    "brake_distance_px",
    "brake_duration_ms",
];

/// #411 Lab L2: a `single_touch_drag_with_vertical_brake_v1` click as the navigation readers take
/// it. The Lab does not perform it (only a package run does), so the reader keeps the boxes its
/// touch stops in and checks its durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveSegmentedDrag {
    pub from_rect: PackRect,
    pub corner_rect: PackRect,
    pub brake_distance_px: i32,
}

impl DriveSegmentedDrag {
    /// The boxes its touch stops in, known without a frame: the press, the corner it holds and
    /// the corner moved up by `brake_distance_px`, where the brake releases the touch (the
    /// kernel's segmented swipe ends at the corner's `y` minus the brake distance).
    pub fn boxes(&self) -> Result<Vec<PackRect>, DriveDecisionError> {
        let brake_y = self
            .corner_rect
            .y
            .checked_sub(self.brake_distance_px)
            .ok_or_else(|| DriveDecisionError::invalid("segmented drag brake end overflow"))?;
        Ok(vec![
            self.from_rect,
            self.corner_rect,
            PackRect {
                y: brake_y,
                ..self.corner_rect
            },
        ])
    }
}

/// #411 Lab L2: a navigation click as the readers take it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveNavigationClick {
    /// A tap, a plain drag or a click that follows a target: the page projection gives its input
    /// and a route presses it.
    Input(DriveSemanticInput),
    /// A segmented drag: the page projection lists its element without an input
    /// (`gesture_not_projectable`) and no route passes it.
    SegmentedDrag(DriveSegmentedDrag),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveNavigationEdge {
    id: String,
    from_page: String,
    to_page: String,
    input: DriveSemanticInput,
    source: Option<String>,
}

impl DriveNavigationEdge {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn from_page(&self) -> &str {
        &self.from_page
    }

    pub fn to_page(&self) -> &str {
        &self.to_page
    }

    pub const fn input(&self) -> &DriveSemanticInput {
        &self.input
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DestructiveRegion {
    page: Option<String>,
    area: DestructiveArea,
}

/// Workflow #373 NAV-5 (wave-17 ruling 21): what a destructive page operation's click covers,
/// read in every form a navigation click takes (`point`, `rect`, `target`, `offset`, `drag`,
/// `long_press`; #411 Lab L2: a segmented drag); a `point` or `rect` click alone was read before.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DestructiveArea {
    /// The boxes the click presses, known without a frame: a tap's rectangle, a long press's
    /// box, a drag's two boxes, a segmented drag's three ([`DriveSegmentedDrag::boxes`]).
    Boxes(Vec<PackRect>),
    /// A click that follows a target: the box the target matches on the frame a route is
    /// resolved on, or the box `offset` gives from its top-left corner ([`drive_offset_rect`]).
    Target {
        target_id: String,
        offset: Option<PackRect>,
    },
}

impl DestructiveRegion {
    /// A region without a page or with page `any` applies on every page.
    fn applies_on(&self, page: &str) -> bool {
        self.page
            .as_deref()
            .is_none_or(|region| region == "any" || region == page)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveNavigationGraph {
    game: Option<String>,
    edges: Vec<DriveNavigationEdge>,
    destructive_regions: Vec<DestructiveRegion>,
    control_points: Vec<String>,
}

impl DriveNavigationGraph {
    pub fn parse_json(source: &str) -> Result<Self, DriveDecisionError> {
        let value: Value = serde_json::from_str(source).map_err(|error| {
            DriveDecisionError::invalid(format!("failed to parse navigation JSON: {error}"))
        })?;
        let game = value
            .get("game")
            .and_then(Value::as_str)
            .map(str::to_string);
        // Workflow #390 LAB-2: an edge of an operation whose `to` is a page set (`to_pages`) has
        // no single destination, so no route passes it. #411 Lab L2: nor does an edge whose
        // click is a segmented drag, which is read and checked.
        let edges = value
            .get("navigation")
            .and_then(Value::as_array)
            .ok_or_else(|| DriveDecisionError::invalid("navigation file is missing navigation[]"))?
            .iter()
            .filter(|edge| edge.get("to_pages").is_none())
            .map(parse_navigation_edge)
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, _>>()?;
        let destructive_regions = value
            .get("destructive_actions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_destructive_region)
            .collect::<Result<Vec<_>, _>>()?;
        let control_points = value
            .get("control_points")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_control_point)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            game,
            edges,
            destructive_regions,
            control_points,
        })
    }

    pub fn canonical_page(&self, page: &str) -> String {
        if page.contains('/') {
            return page.to_string();
        }
        self.game
            .as_ref()
            .map(|game| format!("{game}/{page}"))
            .unwrap_or_else(|| page.to_string())
    }

    pub fn find_route(&self, from_page: &str, to_page: &str) -> Option<Vec<DriveNavigationEdge>> {
        let mut queue = VecDeque::from([from_page.to_string()]);
        let mut previous = BTreeMap::<String, (String, usize)>::new();
        let mut seen = BTreeSet::from([from_page.to_string()]);
        while let Some(page) = queue.pop_front() {
            if page == to_page {
                break;
            }
            for (index, edge) in self.edges.iter().enumerate() {
                if edge.from_page != page || seen.contains(&edge.to_page) {
                    continue;
                }
                seen.insert(edge.to_page.clone());
                previous.insert(edge.to_page.clone(), (page.clone(), index));
                queue.push_back(edge.to_page.clone());
            }
        }
        if from_page != to_page && !previous.contains_key(to_page) {
            return None;
        }
        let mut route = Vec::new();
        let mut cursor = to_page.to_string();
        while cursor != from_page {
            let (previous_page, index) = previous.get(&cursor)?.clone();
            route.push(self.edges[index].clone());
            cursor = previous_page;
        }
        route.reverse();
        Some(route)
    }

    pub fn validate_route(&self, route: &[DriveNavigationEdge]) -> Result<(), DriveDecisionError> {
        for edge in route {
            reject_dangerous_semantic_id("navigation edge", edge.id())?;
            self.validate_resolved_input(edge, edge.input())?;
        }
        Ok(())
    }

    pub fn validate_resolved_input(
        &self,
        edge: &DriveNavigationEdge,
        input: &DriveSemanticInput,
    ) -> Result<(), DriveDecisionError> {
        self.validate_resolved_input_on_frame(edge, input, &[])
    }

    /// Workflow #373 NAV-5 (wave-17 ruling 21): the destructive regions of `page` whose click
    /// follows a target (`target`, `offset`): each target id and, for an `offset` click, its
    /// offset. Their boxes are those [`drive_offset_rect`] or the target's own match gives on the
    /// frame a route is resolved on.
    pub fn frame_destructive_targets(&self, page: &str) -> Vec<(&str, Option<PackRect>)> {
        self.destructive_regions
            .iter()
            .filter(|region| region.applies_on(page))
            .filter_map(|region| match &region.area {
                DestructiveArea::Target { target_id, offset } => {
                    Some((target_id.as_str(), *offset))
                }
                DestructiveArea::Boxes(_) => None,
            })
            .collect()
    }

    /// [`Self::validate_resolved_input`] with `frame_regions`, the boxes the edge's page's
    /// destructive regions that follow a target cover on the frame the input was resolved on
    /// ([`Self::frame_destructive_targets`]).
    pub fn validate_resolved_input_on_frame(
        &self,
        edge: &DriveNavigationEdge,
        input: &DriveSemanticInput,
        frame_regions: &[PackRect],
    ) -> Result<(), DriveDecisionError> {
        let regions = self
            .destructive_regions
            .iter()
            .filter(|region| region.applies_on(&edge.from_page))
            .flat_map(|region| match &region.area {
                DestructiveArea::Boxes(boxes) => boxes.clone(),
                DestructiveArea::Target { .. } => Vec::new(),
            })
            .chain(frame_regions.iter().copied())
            .collect::<Vec<_>>();
        for rect in semantic_input_rects(input) {
            if regions.iter().any(|region| rects_intersect(rect, *region)) {
                return Err(DriveDecisionError::safety(
                    SharedCode::NavigationDestructiveOverlap.code(),
                    format!(
                        "navigation edge '{}' overlaps a destructive action region",
                        edge.id
                    ),
                    vec!["navigation_only"],
                ));
            }
        }
        Ok(())
    }

    pub fn control_points(&self) -> &[String] {
        &self.control_points
    }
}

pub fn reject_dangerous_semantic_id(label: &str, value: &str) -> Result<(), DriveDecisionError> {
    let lower = value.to_ascii_lowercase();
    let dangerous = [
        "shop",
        "purchase",
        "buy",
        "construct",
        "retire",
        "delete",
        "decompose",
        "enhance",
        "refill",
        "paid",
        "premium",
    ];
    if dangerous.iter().any(|word| lower.contains(word)) {
        return Err(DriveDecisionError::safety(
            SharedCode::SemanticActionRequiresDestructiveOptIn.code(),
            format!("{label} '{value}' looks destructive and requires --allow-destructive"),
            vec!["navigation_only"],
        ));
    }
    Ok(())
}

pub fn drive_rect_center(rect: PackRect) -> Result<DrivePoint, DriveDecisionError> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(DriveDecisionError::invalid(format!(
            "click rectangle must have positive dimensions: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(DrivePoint {
        x: rect.x + rect.width / 2,
        y: rect.y + rect.height / 2,
    })
}

/// Workflow #390 LAB-2 item 7: the box an `offset` click presses, the top-left corner of the box
/// its target matched plus `offset`. For a template that matched at a declared factor (REC-5 of
/// Workflow #396) each of the four numbers is scaled by it as the kernel scales an `offset`
/// click, `(v * factor + 500) div 1000` with the sign of `v`, a width or height at least 1;
/// without a factor the offset is kept.
pub fn drive_offset_rect(
    matched: PackRect,
    scale_milli: Option<u32>,
    offset: PackRect,
) -> Result<PackRect, DriveDecisionError> {
    let scale = |value: i32| match scale_milli {
        Some(factor) => actingcommand_recognition::resample::scale_signed(value, factor)
            .ok_or_else(|| DriveDecisionError::package("offset click scale overflow")),
        None => Ok(value),
    };
    let x = scale(offset.x)?;
    let y = scale(offset.y)?;
    Ok(PackRect {
        x: matched
            .x
            .checked_add(x)
            .ok_or_else(|| DriveDecisionError::package("offset click x overflow"))?,
        y: matched
            .y
            .checked_add(y)
            .ok_or_else(|| DriveDecisionError::package("offset click y overflow"))?,
        width: scale(offset.width)?.max(1),
        height: scale(offset.height)?.max(1),
    })
}

pub fn derive_absolute_coordinate_rect_from_match(
    kind: &str,
    declared: PackRect,
    expected_rect: PackRect,
    matched_rect: PackRect,
) -> Result<PackRect, DriveDecisionError> {
    let dx = matched_rect
        .x
        .checked_sub(expected_rect.x)
        .ok_or_else(|| DriveDecisionError::package(format!("{kind} x delta overflow")))?;
    let dy = matched_rect
        .y
        .checked_sub(expected_rect.y)
        .ok_or_else(|| DriveDecisionError::package(format!("{kind} y delta overflow")))?;
    Ok(PackRect {
        x: declared
            .x
            .checked_add(dx)
            .ok_or_else(|| DriveDecisionError::package(format!("{kind} translated x overflow")))?,
        y: declared
            .y
            .checked_add(dy)
            .ok_or_else(|| DriveDecisionError::package(format!("{kind} translated y overflow")))?,
        width: declared.width,
        height: declared.height,
    })
}

fn parse_control_point(value: &Value) -> Result<String, DriveDecisionError> {
    let name = required_string_field(value, "name")?.to_string();
    if let Some(click) = value.get("click") {
        parse_navigation_input(click)?;
    } else {
        let rect = parse_control_point_rect(value)?;
        drive_rect_center(rect)?;
    }
    if value.get("note").is_some_and(|note| !note.is_string()) {
        return Err(DriveDecisionError::invalid("field 'note' must be a string"));
    }
    Ok(name)
}

pub(crate) fn parse_control_point_rect(value: &Value) -> Result<PackRect, DriveDecisionError> {
    if let Some(point) = value.get("point") {
        let (x, y) = parse_point_value(point)?;
        return Ok(PackRect {
            x,
            y,
            width: 1,
            height: 1,
        });
    }
    Ok(PackRect {
        x: required_i32_value(value, "x")?,
        y: required_i32_value(value, "y")?,
        width: 1,
        height: 1,
    })
}

fn parse_destructive_region(value: &Value) -> Result<DestructiveRegion, DriveDecisionError> {
    let click = value
        .get("click")
        .ok_or_else(|| DriveDecisionError::invalid("destructive action is missing click"))?;
    // Workflow #373 NAV-5 (wave-17 ruling 21): every click form a navigation edge takes; a
    // `point` or `rect` click alone was read before.
    Ok(DestructiveRegion {
        page: value
            .get("page")
            .and_then(Value::as_str)
            .map(str::to_string),
        area: parse_destructive_area(click)?,
    })
}

fn parse_destructive_area(click: &Value) -> Result<DestructiveArea, DriveDecisionError> {
    if matches!(
        click.get("kind").and_then(Value::as_str),
        None | Some("point") | Some("rect")
    ) {
        return Ok(DestructiveArea::Boxes(vec![parse_navigation_tap_rect(
            click,
        )?]));
    }
    let input = match parse_navigation_click(click)? {
        // #411 Lab L2: the boxes a segmented drag's touch stops in.
        DriveNavigationClick::SegmentedDrag(drag) => {
            return Ok(DestructiveArea::Boxes(drag.boxes()?));
        }
        DriveNavigationClick::Input(input) => input,
    };
    Ok(match input {
        DriveSemanticInput::Tap { rect, .. } => DestructiveArea::Boxes(vec![rect]),
        DriveSemanticInput::Drag {
            from_rect, to_rect, ..
        } => DestructiveArea::Boxes(vec![from_rect, to_rect]),
        DriveSemanticInput::TargetCenter { target_id } => DestructiveArea::Target {
            target_id,
            offset: None,
        },
        DriveSemanticInput::TargetOffset { target_id, offset } => DestructiveArea::Target {
            target_id,
            offset: Some(offset),
        },
    })
}

/// A navigation row with one destination as a route edge; #411 Lab L2: `None` when its click is
/// a segmented drag, which the reader checks and no route passes.
fn parse_navigation_edge(value: &Value) -> Result<Option<DriveNavigationEdge>, DriveDecisionError> {
    let id = required_string_field(value, "id")?.to_string();
    let from_page = required_string_field(value, "from_page")?.to_string();
    let to_page = required_string_field(value, "to_page")?.to_string();
    let DriveNavigationClick::Input(input) =
        parse_navigation_click(required_value_field(value, "click")?)?
    else {
        return Ok(None);
    };
    Ok(Some(DriveNavigationEdge {
        id,
        from_page,
        to_page,
        input,
        source: value
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

/// #411 Lab L2: a navigation click in every kind a navigation row takes: a segmented drag
/// (`single_touch_drag_with_vertical_brake_v1`) besides every input [`parse_navigation_input`]
/// reads.
pub(crate) fn parse_navigation_click(
    value: &Value,
) -> Result<DriveNavigationClick, DriveDecisionError> {
    if value.get("kind").and_then(Value::as_str) == Some(SEGMENTED_DRAG_KIND) {
        return parse_segmented_drag(value).map(DriveNavigationClick::SegmentedDrag);
    }
    parse_navigation_input(value).map(DriveNavigationClick::Input)
}

/// #411 Lab L2: a segmented drag in the shape pack containment admits: exactly its seven
/// members, `from_rect` and `corner_rect` rectangles of four integers, an integer
/// `brake_distance_px` and unsigned `horizontal_duration_ms`, `corner_hold_ms` and
/// `brake_duration_ms`.
fn parse_segmented_drag(value: &Value) -> Result<DriveSegmentedDrag, DriveDecisionError> {
    reject_unknown_members(value, "segmented drag click", &SEGMENTED_DRAG_FIELDS)?;
    for name in [
        "horizontal_duration_ms",
        "corner_hold_ms",
        "brake_duration_ms",
    ] {
        if required_value_field(value, name)?.as_u64().is_none() {
            return Err(DriveDecisionError::invalid(format!(
                "field '{name}' must be an unsigned integer"
            )));
        }
    }
    let rect = |name: &str| -> Result<PackRect, DriveDecisionError> {
        let rect = required_value_field(value, name)?;
        reject_unknown_members(rect, name, &["x", "y", "width", "height"])?;
        parse_navigation_rect(rect)
    };
    Ok(DriveSegmentedDrag {
        from_rect: rect("from_rect")?,
        corner_rect: rect("corner_rect")?,
        brake_distance_px: required_i32_value(value, "brake_distance_px")?,
    })
}

/// #411 Lab L2: `value` is an object whose members are all in `members`.
fn reject_unknown_members(
    value: &Value,
    label: &str,
    members: &[&str],
) -> Result<(), DriveDecisionError> {
    let object = value
        .as_object()
        .ok_or_else(|| DriveDecisionError::invalid(format!("{label} must be an object")))?;
    if let Some(member) = object
        .keys()
        .find(|member| !members.contains(&member.as_str()))
    {
        return Err(DriveDecisionError::invalid(format!(
            "{label} has unknown member '{member}'"
        )));
    }
    Ok(())
}

pub(crate) fn parse_navigation_input(
    value: &Value,
) -> Result<DriveSemanticInput, DriveDecisionError> {
    match value.get("kind").and_then(Value::as_str) {
        Some("point") | Some("rect") => {
            let rect = parse_navigation_tap_rect(value)?;
            Ok(DriveSemanticInput::Tap {
                rect,
                point: drive_rect_center(rect)?,
            })
        }
        Some("target") | Some("target_center") => Ok(DriveSemanticInput::TargetCenter {
            target_id: required_string_field(value, "target_id")?.to_string(),
        }),
        // Workflow #390 LAB-2 item 7: a REC-4 `offset` click (`--click` with `--click-follow`, or
        // a source `rect_move`) presses its offset from the box its target matches.
        Some("offset") => {
            let offset = parse_navigation_rect(required_value_field(value, "offset")?)?;
            if offset.width <= 0 || offset.height <= 0 {
                return Err(DriveDecisionError::invalid(format!(
                    "offset click rectangle must have positive dimensions: {}x{}",
                    offset.width, offset.height
                )));
            }
            Ok(DriveSemanticInput::TargetOffset {
                target_id: required_string_field(value, "target_id")?.to_string(),
                offset,
            })
        }
        Some("drag") => {
            let from_rect = parse_navigation_tap_rect(required_value_field(value, "from")?)?;
            let to_rect = parse_navigation_tap_rect(required_value_field(value, "to")?)?;
            let duration_ms = value
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(500);
            Ok(DriveSemanticInput::Drag {
                from_rect,
                to_rect,
                from: drive_rect_center(from_rect)?,
                to: drive_rect_center(to_rect)?,
                duration_ms,
            })
        }
        // Workflow #390 LAB-2: a long press holds one point; it is a drag that does not move.
        // Workflow #390 lpbox: a long press with `width` and `height` holds its click box, at
        // the box's centre as a navigation tap presses its rectangle's.
        Some("long_press") | Some("long_tap") => {
            let rect = match (value.get("width"), value.get("height")) {
                (None, None) => parse_navigation_point(value)?,
                (Some(_), Some(_)) => parse_navigation_rect(value)?,
                _ => {
                    return Err(DriveDecisionError::invalid(
                        "long press click box requires both width and height",
                    ));
                }
            };
            let point = drive_rect_center(rect)?;
            let duration_ms = value
                .get("duration_ms")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    DriveDecisionError::invalid("long press click requires duration_ms")
                })?;
            Ok(DriveSemanticInput::Drag {
                from_rect: rect,
                to_rect: rect,
                from: point,
                to: point,
                duration_ms,
            })
        }
        other => Err(DriveDecisionError::invalid(format!(
            "unsupported navigation click kind: {other:?}"
        ))),
    }
}

fn parse_navigation_tap_rect(value: &Value) -> Result<PackRect, DriveDecisionError> {
    match value.get("kind").and_then(Value::as_str) {
        Some("point") => parse_navigation_point(value),
        Some("rect") | None => parse_navigation_rect(value),
        Some("drag") => Err(DriveDecisionError::invalid(
            "drag click cannot be used as a tap rectangle",
        )),
        other => Err(DriveDecisionError::invalid(format!(
            "unsupported navigation click kind for tap rect: {other:?}"
        ))),
    }
}

fn parse_navigation_point(value: &Value) -> Result<PackRect, DriveDecisionError> {
    if let Some(point) = value.get("point") {
        let (x, y) = parse_point_value(point)?;
        return Ok(PackRect {
            x,
            y,
            width: 1,
            height: 1,
        });
    }
    Ok(PackRect {
        x: required_i32_value(value, "x")?,
        y: required_i32_value(value, "y")?,
        width: 1,
        height: 1,
    })
}

fn parse_navigation_rect(value: &Value) -> Result<PackRect, DriveDecisionError> {
    Ok(PackRect {
        x: required_i32_value(value, "x")?,
        y: required_i32_value(value, "y")?,
        width: required_i32_value(value, "width")?,
        height: required_i32_value(value, "height")?,
    })
}

fn parse_point_value(value: &Value) -> Result<(i32, i32), DriveDecisionError> {
    if let Some(point) = value.as_str() {
        return parse_point_pair(point);
    }
    if let Some(items) = value.as_array() {
        if items.len() != 2 {
            return Err(DriveDecisionError::invalid(
                "point array must have exactly two items",
            ));
        }
        return Ok((
            parse_i32_json_value(&items[0], "point[0]")?,
            parse_i32_json_value(&items[1], "point[1]")?,
        ));
    }
    Err(DriveDecisionError::invalid(
        "point must be a string x,y or [x,y] array",
    ))
}

fn parse_point_pair(value: &str) -> Result<(i32, i32), DriveDecisionError> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(DriveDecisionError::invalid(format!(
            "point must be formatted as x,y: {value}"
        )));
    }
    let x = parts[0].parse::<i32>().map_err(|error| {
        DriveDecisionError::invalid(format!("failed to parse point x '{}': {error}", parts[0]))
    })?;
    let y = parts[1].parse::<i32>().map_err(|error| {
        DriveDecisionError::invalid(format!("failed to parse point y '{}': {error}", parts[1]))
    })?;
    Ok((x, y))
}

fn required_value_field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, DriveDecisionError> {
    value
        .get(name)
        .ok_or_else(|| DriveDecisionError::invalid(format!("missing field '{name}'")))
}

fn required_string_field<'a>(value: &'a Value, name: &str) -> Result<&'a str, DriveDecisionError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| DriveDecisionError::invalid(format!("field '{name}' must be a string")))
}

fn required_i32_value(value: &Value, name: &str) -> Result<i32, DriveDecisionError> {
    parse_i32_json_value(required_value_field(value, name)?, name)
}

fn parse_i32_json_value(value: &Value, name: &str) -> Result<i32, DriveDecisionError> {
    if let Some(value) = value.as_i64() {
        return i32::try_from(value)
            .map_err(|_| DriveDecisionError::invalid(format!("field '{name}' exceeds i32 range")));
    }
    Err(DriveDecisionError::invalid(format!(
        "field '{name}' must be an integer"
    )))
}

fn semantic_input_rects(input: &DriveSemanticInput) -> Vec<PackRect> {
    match input {
        DriveSemanticInput::Tap { rect, .. } => vec![*rect],
        DriveSemanticInput::TargetCenter { .. } | DriveSemanticInput::TargetOffset { .. } => {
            Vec::new()
        }
        DriveSemanticInput::Drag {
            from_rect, to_rect, ..
        } => vec![*from_rect, *to_rect],
    }
}

fn rects_intersect(a: PackRect, b: PackRect) -> bool {
    let ax2 = a.x.saturating_add(a.width);
    let ay2 = a.y.saturating_add(a.height);
    let bx2 = b.x.saturating_add(b.width);
    let by2 = b.y.saturating_add(b.height);
    a.x < bx2 && ax2 > b.x && a.y < by2 && ay2 > b.y
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAVIGATION: &str = r#"{
        "game":"fixture01",
        "navigation":[
            {"id":"home_to_terminal","from_page":"fixture01/home","to_page":"fixture01/terminal","click":{"kind":"rect","x":10,"y":20,"width":20,"height":10}},
            {"id":"terminal_to_stage","from_page":"fixture01/terminal","to_page":"fixture01/stage","click":{"kind":"target_center","target_id":"stage_entry"}}
        ],
        "destructive_actions":[
            {"page":"fixture01/home","click":{"kind":"rect","x":100,"y":100,"width":20,"height":20}}
        ],
        "control_points":[{"name":"safe","point":[1,2]}]
    }"#;

    #[test]
    fn graph_parses_and_returns_shortest_canonical_route() {
        let graph = DriveNavigationGraph::parse_json(NAVIGATION).expect("graph");
        let route = graph
            .find_route("fixture01/home", &graph.canonical_page("stage"))
            .expect("route");

        assert_eq!(route.len(), 2);
        assert_eq!(route[0].id(), "home_to_terminal");
        assert_eq!(route[1].to_page(), "fixture01/stage");
        assert_eq!(graph.control_points(), ["safe"]);
        graph.validate_route(&route).expect("safe route");
    }

    #[test]
    fn destructive_overlap_and_dangerous_ids_are_safety_blocked() {
        let graph = DriveNavigationGraph::parse_json(NAVIGATION).expect("graph");
        let edge = DriveNavigationEdge {
            id: "open_shop".to_string(),
            from_page: "fixture01/home".to_string(),
            to_page: "fixture01/shop".to_string(),
            input: DriveSemanticInput::Tap {
                rect: PackRect {
                    x: 105,
                    y: 105,
                    width: 1,
                    height: 1,
                },
                point: DrivePoint { x: 105, y: 105 },
            },
            source: None,
        };

        assert_eq!(
            reject_dangerous_semantic_id("navigation edge", edge.id())
                .expect_err("dangerous id")
                .code()
                .as_str(),
            "semantic_action_requires_destructive_opt_in"
        );
        assert_eq!(
            graph
                .validate_resolved_input(&edge, edge.input())
                .expect_err("overlap")
                .code()
                .as_str(),
            "navigation_destructive_overlap"
        );
    }

    #[test]
    fn coordinate_decisions_preserve_translation_and_reject_overflow() {
        let rect = drive_rect_center(PackRect {
            x: 10,
            y: 20,
            width: 8,
            height: 6,
        })
        .expect("center");
        assert_eq!(rect, DrivePoint { x: 14, y: 23 });
        let translated = derive_absolute_coordinate_rect_from_match(
            "click",
            PackRect {
                x: 40,
                y: 50,
                width: 5,
                height: 6,
            },
            PackRect {
                x: 10,
                y: 20,
                width: 1,
                height: 1,
            },
            PackRect {
                x: 13,
                y: 24,
                width: 1,
                height: 1,
            },
        )
        .expect("translation");
        assert_eq!((translated.x, translated.y), (43, 54));
        assert_eq!(
            derive_absolute_coordinate_rect_from_match(
                "click",
                PackRect {
                    x: i32::MAX,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                PackRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                PackRect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            )
            .expect_err("overflow")
            .kind(),
            DriveDecisionErrorKind::PackageInvalid
        );
    }

    #[test]
    fn resolved_semantic_input_maps_to_typed_runtime_action() {
        let tap = DriveSemanticInput::Tap {
            rect: PackRect {
                x: 10,
                y: 20,
                width: 8,
                height: 6,
            },
            point: DrivePoint { x: 14, y: 23 },
        };
        assert_eq!(
            tap.resolved_input_action().expect("tap action"),
            InputAction::Tap { x: 14, y: 23 }
        );

        let unresolved = DriveSemanticInput::TargetCenter {
            target_id: "entry".to_string(),
        };
        assert_eq!(
            unresolved
                .resolved_input_action()
                .expect_err("unresolved target")
                .kind(),
            DriveDecisionErrorKind::InvalidInput
        );
    }

    // Workflow #390 LAB-2 (FCM T-03): the navigation collects an operation that may reach several
    // pages with all of them (`to_pages`); its click does not decide which page comes, so no
    // route passes it, and the edges with one destination still route.
    #[test]
    fn lab2_a_route_never_passes_an_edge_with_several_destinations() {
        let graph = DriveNavigationGraph::parse_json(
            r#"{
            "game":"fixture01",
            "navigation":[
                {"id":"home_to_either","from_page":"fixture01/home","to_pages":["fixture01/stage","fixture01/terminal"],"click":{"kind":"rect","x":10,"y":20,"width":20,"height":10},"source":""},
                {"id":"home_to_terminal","from_page":"fixture01/home","to_page":"fixture01/terminal","click":{"kind":"rect","x":40,"y":20,"width":20,"height":10}}
            ]
        }"#,
        )
        .expect("graph");

        let route = graph
            .find_route("fixture01/home", "fixture01/terminal")
            .expect("route");
        assert_eq!(
            route
                .iter()
                .map(DriveNavigationEdge::id)
                .collect::<Vec<_>>(),
            ["home_to_terminal"]
        );
        assert!(
            graph
                .find_route("fixture01/home", "fixture01/stage")
                .is_none()
        );
    }

    // Workflow #390 LAB-2: a long press holds its one point for its duration, a drag that does
    // not move; #411 Lab L3: it is pressed as a long tap there, as the kernel presses a pack's
    // long press.
    #[test]
    fn lab2_a_long_press_navigation_input_holds_its_point() {
        let input = parse_navigation_input(&serde_json::json!({
            "kind": "long_press", "x": 30, "y": 40, "duration_ms": 800
        }))
        .expect("long press");

        assert_eq!(
            input.resolved_input_action().expect("action"),
            InputAction::LongTap {
                x: 30,
                y: 40,
                duration_ms: 800
            }
        );
    }

    // Workflow #390 lpbox (exec key 76): a long press with a click box holds the box, at its
    // centre as a navigation tap presses its rectangle's, for its duration; a box needs both
    // `width` and `height`.
    #[test]
    fn lpbox_a_long_press_box_navigation_input_holds_its_box() {
        let box_rect = PackRect {
            x: 30,
            y: 40,
            width: 8,
            height: 6,
        };
        let input = parse_navigation_input(&serde_json::json!({
            "kind": "long_press", "x": 30, "y": 40, "width": 8, "height": 6, "duration_ms": 800
        }))
        .expect("long press box");
        assert_eq!(
            input,
            DriveSemanticInput::Drag {
                from_rect: box_rect,
                to_rect: box_rect,
                from: DrivePoint { x: 34, y: 43 },
                to: DrivePoint { x: 34, y: 43 },
                duration_ms: 800,
            }
        );

        let error = parse_navigation_input(&serde_json::json!({
            "kind": "long_press", "x": 30, "y": 40, "width": 8, "duration_ms": 800
        }))
        .expect_err("half a box");
        assert_eq!(error.kind(), DriveDecisionErrorKind::InvalidInput);
    }

    // Workflow #373 NAV-5 (exec key 78; wave-17 ruling 21): a destructive page operation takes
    // every click form a navigation edge takes. Its rectangle, long press box and drag boxes are
    // its region without a frame; a `target` or `offset` click covers the box its target matches
    // on the frame a route is resolved on, which the caller hands in.
    #[test]
    fn nav5_a_destructive_action_takes_every_navigation_click_form() {
        let graph = DriveNavigationGraph::parse_json(
            r#"{
            "game":"fixture01",
            "navigation":[
                {"id":"home_to_terminal","from_page":"fixture01/home","to_page":"fixture01/terminal","click":{"kind":"rect","x":0,"y":0,"width":4,"height":4}}
            ],
            "destructive_actions":[
                {"page":"fixture01/home","click":{"x":100,"y":100,"width":10,"height":10}},
                {"page":"fixture01/home","click":{"kind":"drag","from":{"kind":"rect","x":200,"y":200,"width":10,"height":10},"to":{"kind":"rect","x":300,"y":300,"width":10,"height":10},"duration_ms":400}},
                {"page":"fixture01/home","click":{"kind":"long_press","x":400,"y":400,"width":10,"height":10,"duration_ms":800}},
                {"page":"fixture01/home","click":{"kind":"target","target_id":"buy_button"}},
                {"page":"fixture01/home","click":{"kind":"offset","target_id":"buy_icon","offset":{"x":5,"y":6,"width":7,"height":8}}},
                {"page":"fixture01/other","click":{"kind":"target","target_id":"elsewhere"}}
            ]
        }"#,
        )
        .expect("a graph whose destructive actions take every click form");
        assert_eq!(
            graph.frame_destructive_targets("fixture01/home"),
            [
                ("buy_button", None),
                (
                    "buy_icon",
                    Some(PackRect {
                        x: 5,
                        y: 6,
                        width: 7,
                        height: 8
                    })
                ),
            ]
        );
        let tap = |x: i32, y: i32| DriveNavigationEdge {
            id: "tap".to_string(),
            from_page: "fixture01/home".to_string(),
            to_page: "fixture01/terminal".to_string(),
            input: DriveSemanticInput::Tap {
                rect: PackRect {
                    x,
                    y,
                    width: 1,
                    height: 1,
                },
                point: DrivePoint { x, y },
            },
            source: None,
        };
        let overlaps = |edge: &DriveNavigationEdge, frame: &[PackRect]| {
            graph
                .validate_resolved_input_on_frame(edge, edge.input(), frame)
                .map_err(|error| error.code().as_str().to_owned())
        };
        let blocked = Err("navigation_destructive_overlap".to_owned());
        for (x, y) in [(105, 105), (205, 205), (305, 305), (405, 405)] {
            assert_eq!(overlaps(&tap(x, y), &[]), blocked, "({x},{y})");
        }
        assert_eq!(overlaps(&tap(600, 600), &[]), Ok(()));
        // The frame resolved the target's box at (600,600): a tap there is blocked on it alone.
        let resolved = [PackRect {
            x: 595,
            y: 595,
            width: 10,
            height: 10,
        }];
        assert_eq!(overlaps(&tap(600, 600), &resolved), blocked);
        assert_eq!(
            graph
                .validate_resolved_input(&tap(600, 600), tap(600, 600).input())
                .map_err(|error| error.code().as_str().to_owned()),
            Ok(())
        );
    }
}
