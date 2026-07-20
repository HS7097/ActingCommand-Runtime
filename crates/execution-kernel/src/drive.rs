// SPDX-License-Identifier: AGPL-3.0-only

//! Pure semantic-drive planning owned by the execution kernel.

use actingcommand_contract::InputAction;
use actingcommand_pack_containment::{NavigationContract, NavigationInput, NavigationRect};
use actingcommand_recognition_pack::PackRect;
use serde::Serialize;
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
    code: &'static str,
    message: String,
    required_conditions: Vec<&'static str>,
}

impl DriveDecisionError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: DriveDecisionErrorKind::InvalidInput,
            code: "drive_plan_invalid",
            message: message.into(),
            required_conditions: Vec::new(),
        }
    }

    fn safety(
        code: &'static str,
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
            code: "drive_package_invalid",
            message: message.into(),
            required_conditions: Vec::new(),
        }
    }

    pub const fn kind(&self) -> DriveDecisionErrorKind {
        self.kind
    }

    pub const fn code(&self) -> &'static str {
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
    rect: PackRect,
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
        let contract = NavigationContract::parse_json(source)
            .map_err(|error| DriveDecisionError::invalid(error.to_string()))?;
        Self::from_contract(&contract)
    }

    pub fn from_contract(contract: &NavigationContract) -> Result<Self, DriveDecisionError> {
        let edges = contract
            .routes()
            .iter()
            .map(|route| {
                Ok(DriveNavigationEdge {
                    id: route.id().to_string(),
                    from_page: route.from_page().to_string(),
                    to_page: route.to_page().to_string(),
                    input: drive_semantic_input(route.input())?,
                    source: route.source().map(str::to_string),
                })
            })
            .collect::<Result<Vec<_>, DriveDecisionError>>()?;
        let destructive_regions = contract
            .destructive_actions()
            .iter()
            .map(|action| DestructiveRegion {
                page: action.page().map(str::to_string),
                rect: pack_rect(action.rect()),
            })
            .collect();
        let control_points = contract
            .control_points()
            .iter()
            .map(|point| point.name().to_string())
            .collect();
        Ok(Self {
            game: Some(contract.game().to_string()),
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
        for rect in semantic_input_rects(input) {
            if self.destructive_regions.iter().any(|other| {
                other
                    .page
                    .as_deref()
                    .is_none_or(|page| page == "any" || page == edge.from_page)
                    && rects_intersect(rect, other.rect)
            }) {
                return Err(DriveDecisionError::safety(
                    "navigation_destructive_overlap",
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
            "semantic_action_requires_destructive_opt_in",
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

fn drive_semantic_input(input: &NavigationInput) -> Result<DriveSemanticInput, DriveDecisionError> {
    match input {
        NavigationInput::Tap { rect } => {
            let rect = pack_rect(*rect);
            Ok(DriveSemanticInput::Tap {
                rect,
                point: drive_rect_center(rect)?,
            })
        }
        NavigationInput::TargetCenter { target_id } => Ok(DriveSemanticInput::TargetCenter {
            target_id: target_id.clone(),
        }),
        NavigationInput::Drag {
            from_rect,
            to_rect,
            duration_ms,
        } => {
            let from_rect = pack_rect(*from_rect);
            let to_rect = pack_rect(*to_rect);
            Ok(DriveSemanticInput::Drag {
                from_rect,
                to_rect,
                from: drive_rect_center(from_rect)?,
                to: drive_rect_center(to_rect)?,
                duration_ms: *duration_ms,
            })
        }
    }
}

fn pack_rect(rect: NavigationRect) -> PackRect {
    PackRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn semantic_input_rects(input: &DriveSemanticInput) -> Vec<PackRect> {
    match input {
        DriveSemanticInput::Tap { rect, .. } => vec![*rect],
        DriveSemanticInput::TargetCenter { .. } => Vec::new(),
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
        "schema_version":"0.3",
        "game":"fixture01",
        "server":"test",
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
                .code(),
            "semantic_action_requires_destructive_opt_in"
        );
        assert_eq!(
            graph
                .validate_resolved_input(&edge, edge.input())
                .expect_err("overlap")
                .code(),
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
}
