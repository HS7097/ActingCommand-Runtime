// SPDX-License-Identifier: AGPL-3.0-only

//! Pure semantic-drive planning owned by the execution kernel.

use actingcommand_contract::InputAction;
use actingcommand_pack_containment::{
    AdmittedAction, AdmittedEffectCapability, AdmittedPackage, BoundedRect, OperationKey,
    PageSelector, TargetOffset, TargetTapMode,
};
use actingcommand_recognition_pack::{PackRect, TargetEvaluation};
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
    TargetTap {
        target_id: String,
        mode: TargetTapMode,
        offset: Option<TargetOffset>,
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
            Self::TargetTap { .. } => {
                return Err(DriveDecisionError::invalid(
                    "target semantic input must be resolved before execution",
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
    operation_key: OperationKey,
    id: String,
    from_page: String,
    to_page: String,
    input: DriveSemanticInput,
    effect_capability: AdmittedEffectCapability,
    source: Option<String>,
}

impl DriveNavigationEdge {
    pub fn operation_key(&self) -> &OperationKey {
        &self.operation_key
    }

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

    pub const fn effect_capability(&self) -> AdmittedEffectCapability {
        self.effect_capability
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DestructiveRegion {
    page: PageSelector,
    rect: PackRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DriveControlPoint {
    input: DriveSemanticInput,
    effect_capability: AdmittedEffectCapability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveNavigationGraph {
    game: String,
    semantic_fingerprint: String,
    edges: Vec<DriveNavigationEdge>,
    destructive_regions: Vec<DestructiveRegion>,
    control_points: BTreeMap<String, DriveControlPoint>,
}

impl DriveNavigationGraph {
    pub fn from_admitted(package: &AdmittedPackage) -> Result<Self, DriveDecisionError> {
        let navigation = package
            .navigation()
            .ok_or_else(|| DriveDecisionError::package("admitted package has no navigation"))?;
        let edges = navigation
            .routes()
            .iter()
            .map(|route| {
                let operation = package.operation(route.operation()).ok_or_else(|| {
                    DriveDecisionError::package(format!(
                        "admitted route references missing operation '{}'",
                        route.operation()
                    ))
                })?;
                let to_page = operation.to().ok_or_else(|| {
                    DriveDecisionError::package(format!(
                        "admitted route operation '{}' has no target page",
                        operation.key()
                    ))
                })?;
                Ok(DriveNavigationEdge {
                    operation_key: operation.key().clone(),
                    id: operation.key().operation().to_string(),
                    from_page: page_selector_text(operation.from()),
                    to_page: to_page.to_string(),
                    input: drive_semantic_input_from_admitted(operation.action())?,
                    effect_capability: operation.effect_capability(),
                    source: route.source().map(str::to_string),
                })
            })
            .collect::<Result<Vec<_>, DriveDecisionError>>()?;
        let destructive_regions = navigation
            .destructive_regions()
            .iter()
            .map(|action| DestructiveRegion {
                page: action.page().clone(),
                rect: pack_rect(action.rect()),
            })
            .collect();
        let control_points = navigation
            .control_points()
            .iter()
            .map(|point| {
                Ok((
                    point.name().to_string(),
                    DriveControlPoint {
                        input: drive_semantic_input_from_admitted(point.action())?,
                        effect_capability: point.effect_capability(),
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>, DriveDecisionError>>()?;
        Ok(Self {
            game: package.control().game().to_string(),
            semantic_fingerprint: package.semantic_fingerprint().to_string(),
            edges,
            destructive_regions,
            control_points,
        })
    }

    pub fn canonical_page(&self, page: &str) -> String {
        if page.contains('/') {
            return page.to_string();
        }
        format!("{}/{page}", self.game)
    }

    pub fn edges(&self) -> &[DriveNavigationEdge] {
        &self.edges
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
                if (edge.from_page != page && edge.from_page != "any")
                    || seen.contains(&edge.to_page)
                {
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

    pub fn validate_route(
        &self,
        route: &[DriveNavigationEdge],
        allow_destructive: bool,
    ) -> Result<(), DriveDecisionError> {
        for edge in route {
            self.validate_resolved_input(edge, edge.input(), allow_destructive)?;
        }
        Ok(())
    }

    pub fn validate_resolved_input(
        &self,
        edge: &DriveNavigationEdge,
        input: &DriveSemanticInput,
        allow_destructive: bool,
    ) -> Result<(), DriveDecisionError> {
        validate_effect_capability(
            &format!("navigation edge '{}'", edge.id),
            edge.effect_capability(),
            allow_destructive,
        )?;
        if edge.effect_capability().requires_explicit_opt_in() {
            return Ok(());
        }
        for rect in semantic_input_rects(input) {
            if self.destructive_regions.iter().any(|other| {
                destructive_page_matches(&other.page, &edge.from_page)
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

    fn validate_direct_input_on_page(
        &self,
        label: &str,
        page: Option<&str>,
        input: &DriveSemanticInput,
        capability: AdmittedEffectCapability,
        allow_destructive: bool,
    ) -> Result<(), DriveDecisionError> {
        validate_effect_capability(label, capability, allow_destructive)?;
        if capability.requires_explicit_opt_in() {
            return Ok(());
        }
        for rect in semantic_input_rects(input) {
            if self.destructive_regions.iter().any(|other| {
                page.is_none_or(|page| destructive_page_matches(&other.page, page))
                    && rects_intersect(rect, other.rect)
            }) {
                return Err(DriveDecisionError::safety(
                    "navigation_destructive_overlap",
                    format!("{label} overlaps a destructive action region"),
                    vec!["navigation_only"],
                ));
            }
        }
        Ok(())
    }

    pub fn control_point_names(&self) -> impl Iterator<Item = &str> {
        self.control_points.keys().map(String::as_str)
    }

    pub fn validate_control_point_input(
        &self,
        name: &str,
        input: &DriveSemanticInput,
    ) -> Result<(), DriveDecisionError> {
        let point = self.control_points.get(name).ok_or_else(|| {
            DriveDecisionError::package(format!(
                "control point '{name}' is not owned by the admitted navigation graph"
            ))
        })?;
        if &point.input != input {
            return Err(DriveDecisionError::package(format!(
                "control point '{name}' input diverged from its admitted action"
            )));
        }
        self.validate_direct_input_on_page(
            &format!("control point '{name}'"),
            None,
            input,
            point.effect_capability,
            false,
        )
    }

    pub fn resolve_direct_target(
        &self,
        package: &AdmittedPackage,
        target_id: &str,
        target: &TargetEvaluation,
        page: Option<&str>,
        allow_destructive: bool,
    ) -> Result<crate::CanonicalEffectPoint, DriveDecisionError> {
        if package.semantic_fingerprint() != self.semantic_fingerprint {
            return Err(DriveDecisionError::package(
                "target package does not match the admitted navigation graph",
            ));
        }
        let operation = package.target_operation(target_id).ok_or_else(|| {
            DriveDecisionError::safety(
                "semantic_action_capability_unknown",
                format!("target '{target_id}' has no unique canonical admitted target action"),
                vec!["typed_operation_capability"],
            )
        })?;
        let intent = crate::resolve_admitted_effect_intent(package, operation, Some(target))
            .map_err(|error| DriveDecisionError::package(error.to_string()))?;
        let crate::CanonicalEffectIntent::Tap { point } = intent else {
            return Err(DriveDecisionError::package(format!(
                "canonical admitted action for target '{target_id}' did not resolve to a tap"
            )));
        };
        let input = DriveSemanticInput::Tap {
            rect: PackRect {
                x: point.rect.x,
                y: point.rect.y,
                width: point.rect.width,
                height: point.rect.height,
            },
            point: DrivePoint {
                x: point.x,
                y: point.y,
            },
        };
        self.validate_direct_input_on_page(
            &format!("target '{target_id}'"),
            page,
            &input,
            operation.effect_capability(),
            allow_destructive,
        )?;
        Ok(point)
    }
}

fn validate_effect_capability(
    label: &str,
    capability: AdmittedEffectCapability,
    allow_destructive: bool,
) -> Result<(), DriveDecisionError> {
    if capability.requires_explicit_opt_in() && !allow_destructive {
        return Err(DriveDecisionError::safety(
            "semantic_action_requires_destructive_opt_in",
            format!("{label} is typed destructive and requires --allow-destructive"),
            vec!["typed_destructive_capability", "allow_destructive"],
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

pub fn resolve_drive_target_input(
    package: &AdmittedPackage,
    edge: &DriveNavigationEdge,
    target: &TargetEvaluation,
) -> Result<DriveSemanticInput, DriveDecisionError> {
    let operation = package.operation(edge.operation_key()).ok_or_else(|| {
        DriveDecisionError::package(format!(
            "admitted navigation operation '{}' is missing",
            edge.operation_key()
        ))
    })?;
    let intent = crate::resolve_admitted_effect_intent(package, operation, Some(target))
        .map_err(|error| DriveDecisionError::package(error.to_string()))?;
    match intent {
        crate::CanonicalEffectIntent::Tap { point } => Ok(DriveSemanticInput::Tap {
            rect: PackRect {
                x: point.rect.x,
                y: point.rect.y,
                width: point.rect.width,
                height: point.rect.height,
            },
            point: DrivePoint {
                x: point.x,
                y: point.y,
            },
        }),
        _ => Err(DriveDecisionError::package(format!(
            "admitted target operation '{}' did not resolve to a tap",
            edge.operation_key()
        ))),
    }
}

/// Projects a canonical admitted action into the shared semantic-drive representation.
/// Target mode and offset remain opaque until recognition resolves them through the
/// canonical effect decision boundary.
pub fn drive_semantic_input_from_admitted(
    input: &AdmittedAction,
) -> Result<DriveSemanticInput, DriveDecisionError> {
    match input {
        AdmittedAction::Tap { rect, point } => {
            let rect = pack_rect(*rect);
            Ok(DriveSemanticInput::Tap {
                rect,
                point: DrivePoint {
                    x: point.x(),
                    y: point.y(),
                },
            })
        }
        AdmittedAction::TargetTap {
            target,
            mode,
            offset,
        } => Ok(DriveSemanticInput::TargetTap {
            target_id: target.to_string(),
            mode: *mode,
            offset: *offset,
        }),
        AdmittedAction::Drag {
            from_rect,
            to_rect,
            from,
            to,
            duration,
        } => {
            let from_rect = pack_rect(*from_rect);
            let to_rect = pack_rect(*to_rect);
            Ok(DriveSemanticInput::Drag {
                from_rect,
                to_rect,
                from: DrivePoint {
                    x: from.x(),
                    y: from.y(),
                },
                to: DrivePoint {
                    x: to.x(),
                    y: to.y(),
                },
                duration_ms: duration.milliseconds(),
            })
        }
        AdmittedAction::LongTap { .. } => Err(DriveDecisionError::package(
            "admitted navigation route cannot use a long-tap action",
        )),
    }
}

fn pack_rect(rect: BoundedRect) -> PackRect {
    PackRect {
        x: rect.x(),
        y: rect.y(),
        width: rect.width(),
        height: rect.height(),
    }
}

fn page_selector_text(selector: &PageSelector) -> String {
    match selector {
        PageSelector::Any => "any".to_string(),
        PageSelector::Exact(page) => page.to_string(),
    }
}

fn destructive_page_matches(selector: &PageSelector, page: &str) -> bool {
    match selector {
        PageSelector::Any => true,
        PageSelector::Exact(expected) => page == "any" || expected.to_string() == page,
    }
}

fn semantic_input_rects(input: &DriveSemanticInput) -> Vec<PackRect> {
    match input {
        DriveSemanticInput::Tap { rect, .. } => vec![*rect],
        DriveSemanticInput::TargetTap { .. } => Vec::new(),
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
    use crate::{ExternalExpectedSha256, ExternallyVerifiedBundle};
    use actingcommand_pack_containment::Sha256Hash;
    use actingcommand_recognition_pack::{TargetKind, TemplateEvaluation};
    use serde_json::{Value, json};
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::FileOptions};

    #[test]
    fn graph_parses_and_returns_shortest_canonical_route() {
        let graph = admitted_graph();
        let route = graph
            .find_route("fixture01/home", &graph.canonical_page("stage"))
            .expect("route");

        assert_eq!(route.len(), 2);
        assert_eq!(route[0].id(), "home_to_terminal");
        assert_eq!(route[1].to_page(), "fixture01/stage");
        assert_eq!(graph.control_point_names().collect::<Vec<_>>(), ["safe"]);
        let safe_control_point = graph
            .control_points
            .get("safe")
            .expect("admitted control point")
            .input
            .clone();
        graph
            .validate_control_point_input("safe", &safe_control_point)
            .expect("admitted safe control point");
        let DriveSemanticInput::Tap { rect, .. } = safe_control_point else {
            panic!("fixture control point must be a tap");
        };
        assert_eq!(
            graph
                .validate_control_point_input(
                    "safe",
                    &DriveSemanticInput::Tap {
                        rect,
                        point: DrivePoint { x: 2, y: 2 },
                    },
                )
                .expect_err("fabricated control-point input")
                .kind(),
            DriveDecisionErrorKind::PackageInvalid
        );
        graph.validate_route(&route, false).expect("safe route");
    }

    #[test]
    fn any_source_route_survives_admission_and_is_a_runtime_fallback() {
        let graph = admitted_graph_with_source("any", "any");
        let route = graph
            .find_route("fixture01/home", &graph.canonical_page("terminal"))
            .expect("any-source route");

        assert_eq!(route.len(), 1);
        assert_eq!(route[0].from_page(), "any");
        assert_eq!(route[0].to_page(), "fixture01/terminal");
        graph
            .validate_route(&route, false)
            .expect("safe any-source route");
    }

    #[test]
    fn typed_capability_replaces_names_and_non_destructive_overlap_is_always_blocked() {
        let graph = admitted_graph();
        let safe_named_edge = DriveNavigationEdge {
            operation_key: graph.edges[0].operation_key.clone(),
            id: "open_shop".to_string(),
            from_page: "fixture01/home".to_string(),
            to_page: "fixture01/shop".to_string(),
            input: DriveSemanticInput::Tap {
                rect: PackRect {
                    x: 50,
                    y: 50,
                    width: 1,
                    height: 1,
                },
                point: DrivePoint { x: 50, y: 50 },
            },
            effect_capability: AdmittedEffectCapability::NavigationOnly,
            source: None,
        };
        graph
            .validate_resolved_input(&safe_named_edge, safe_named_edge.input(), false)
            .expect("names do not define safety capability");

        let destructive_edge = DriveNavigationEdge {
            id: "neutral_action".to_string(),
            effect_capability: AdmittedEffectCapability::Destructive,
            ..safe_named_edge.clone()
        };
        assert_eq!(
            graph
                .validate_resolved_input(&destructive_edge, destructive_edge.input(), false)
                .expect_err("typed destructive operation requires opt-in")
                .code(),
            "semantic_action_requires_destructive_opt_in"
        );
        graph
            .validate_resolved_input(&destructive_edge, destructive_edge.input(), true)
            .expect("explicit opt-in permits typed destructive operation");

        let overlapping_safe_edge = DriveNavigationEdge {
            operation_key: graph.edges[0].operation_key.clone(),
            id: "neutral_navigation".to_string(),
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
            effect_capability: AdmittedEffectCapability::NavigationOnly,
            source: None,
        };
        assert_eq!(
            graph
                .validate_resolved_input(
                    &overlapping_safe_edge,
                    overlapping_safe_edge.input(),
                    true,
                )
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

        let unresolved = DriveSemanticInput::TargetTap {
            target_id: "entry".to_string(),
            mode: TargetTapMode::Center,
            offset: None,
        };
        assert_eq!(
            unresolved
                .resolved_input_action()
                .expect_err("unresolved target")
                .kind(),
            DriveDecisionErrorKind::InvalidInput
        );
    }

    #[test]
    fn every_target_tap_form_uses_the_canonical_effect_resolver() {
        let bundle = admitted_target_bundle();
        let package = bundle.admitted_package();
        assert!(
            package.target_operation("home_button").is_none(),
            "multiple operation identities must not become an ambiguous direct-target authority"
        );
        let target = TargetEvaluation {
            id: "home_button".to_string(),
            kind: TargetKind::Template,
            passed: true,
            template: Some(TemplateEvaluation {
                x: 30,
                y: 40,
                width: 4,
                height: 6,
                raw_score: 1.0,
                score: 1.0,
                threshold: 0.99,
            }),
            color: None,
            message: "fixture target passed".to_string(),
        };

        for (operation_id, expected_mode, expected_offset) in [
            ("tap_target", TargetTapMode::Deterministic, None),
            ("tap_target_center", TargetTapMode::Center, None),
            (
                "tap_target_offset",
                TargetTapMode::Center,
                Some((1, 2, 2, 2)),
            ),
        ] {
            let operation = package
                .entry_task()
                .operations()
                .iter()
                .find(|operation| operation.key().operation() == operation_id)
                .expect("target operation");
            let projected = drive_semantic_input_from_admitted(operation.action())
                .expect("canonical drive projection");
            match &projected {
                DriveSemanticInput::TargetTap {
                    target_id,
                    mode,
                    offset,
                } => {
                    assert_eq!(target_id, "home_button");
                    assert_eq!(*mode, expected_mode);
                    assert_eq!(
                        offset.map(|offset| {
                            (offset.x(), offset.y(), offset.width(), offset.height())
                        }),
                        expected_offset
                    );
                }
                other => panic!("expected target projection, got {other:?}"),
            }

            let canonical =
                crate::resolve_admitted_effect_intent(package, operation, Some(&target))
                    .expect("canonical effect intent");
            let crate::CanonicalEffectIntent::Tap { point } = &canonical else {
                panic!("{operation_id} did not resolve to a canonical tap");
            };
            assert_eq!(
                point.algorithm,
                match expected_mode {
                    TargetTapMode::Deterministic => "xorshift64_uniform_rect_v1",
                    TargetTapMode::Center => "center_point_v1",
                },
                "{operation_id} lost its admitted target mode"
            );
            let edge = DriveNavigationEdge {
                operation_key: operation.key().clone(),
                id: operation_id.to_string(),
                from_page: "fixture/home".to_string(),
                to_page: "fixture/home".to_string(),
                input: projected,
                effect_capability: operation.effect_capability(),
                source: None,
            };
            let resolved = resolve_drive_target_input(package, &edge, &target)
                .expect("drive target resolution");
            assert_eq!(
                resolved
                    .resolved_input_action()
                    .expect("drive input action"),
                canonical.input_action(),
                "{operation_id} diverged from the canonical resolver"
            );
        }
    }

    fn admitted_graph() -> DriveNavigationGraph {
        admitted_graph_with_source("home", "fixture01/home")
    }

    fn admitted_graph_with_source(
        operation_from_page: &str,
        navigation_from_page: &str,
    ) -> DriveNavigationGraph {
        let bytes = package_zip(&[
            (
                "control.json",
                json!({
                    "schema_version":"Lab-1y.control.v1",
                    "package_id":"fixture01.task",
                    "execution_mode":"navigable_route",
                    "game":"fixture01",
                    "server":"test",
                    "resolution":{"width":200,"height":200},
                    "entry_task_id":"task"
                }),
            ),
            (
                "resources/manifest.json",
                json!({"schema_version":"0.3","entry_task_id":"task"}),
            ),
            (
                "resources/operations/task/task.json",
                json!({
                    "schema_version":"0.6",
                    "task_id":"task",
                    "game":"fixture01",
                    "server_scope":["test"],
                    "coordinate_space":{"width":200,"height":200},
                    "operations":[
                        {"id":"home_to_terminal","from":operation_from_page,"to":"terminal","click":{"kind":"rect","x":10,"y":20,"width":20,"height":10},"unguarded_trusted_coordinate":true},
                        {"id":"terminal_to_stage","from":"terminal","to":"stage","click":{"kind":"point","x":30,"y":40},"unguarded_trusted_coordinate":true}
                    ]
                }),
            ),
            (
                "resources/recognition/fixture01.test.pack.json",
                json!({
                    "schema_version":"0.5",
                    "game":"fixture01",
                    "server":"test",
                    "coordinate_space":{"width":200,"height":200},
                    "targets":[
                        {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                        {"type":"color","id":"page/terminal","region":{"x":1,"y":0,"width":1,"height":1},"expected":[0,255,0]},
                        {"type":"color","id":"page/stage","region":{"x":2,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                    ]
                }),
            ),
            (
                "resources/recognition/fixture01.test.pages.json",
                json!({"schema_version":"0.5","pages":[
                    {"id":"fixture01/home","required":["page/home"]},
                    {"id":"fixture01/terminal","required":["page/terminal"]},
                    {"id":"fixture01/stage","required":["page/stage"]}
                ]}),
            ),
            (
                "resources/navigation/fixture01.test.navigation.json",
                json!({
                    "schema_version":"0.5",
                    "game":"fixture01",
                    "server":"test",
                    "navigation":[
                        {"id":"home_to_terminal","from_page":navigation_from_page,"to_page":"fixture01/terminal","click":{"kind":"rect","x":10,"y":20,"width":20,"height":10}},
                        {"id":"terminal_to_stage","from_page":"fixture01/terminal","to_page":"fixture01/stage","click":{"kind":"point","x":30,"y":40}}
                    ],
                    "destructive_actions":[
                        {"page":"fixture01/home","click":{"kind":"rect","x":100,"y":100,"width":20,"height":20}}
                    ],
                    "control_points":[{"name":"safe","point":[1,2]}]
                }),
            ),
        ]);
        let expected = ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(&bytes).to_string())
            .expect("expected hash");
        let bundle = ExternallyVerifiedBundle::load("drive_fixture", &bytes, expected)
            .expect("admitted package");
        DriveNavigationGraph::from_admitted(bundle.admitted_package()).expect("graph")
    }

    fn admitted_target_bundle() -> ExternallyVerifiedBundle {
        let bytes = package_zip_with_binary(
            &[
                (
                    "control.json",
                    json!({
                        "schema_version":"Lab-1y.control.v1",
                        "package_id":"fixture.target",
                        "execution_mode":"navigable_route",
                        "game":"fixture",
                        "server":"test",
                        "resolution":{"width":100,"height":100},
                        "entry_task_id":"task"
                    }),
                ),
                (
                    "resources/manifest.json",
                    json!({"schema_version":"0.3","entry_task_id":"task"}),
                ),
                (
                    "resources/operations/task/task.json",
                    json!({
                        "schema_version":"0.6",
                        "task_id":"task",
                        "game":"fixture",
                        "server_scope":["test"],
                        "goal":"canonical target effect fixture",
                        "coordinate_space":{"width":100,"height":100},
                        "operations":[
                            {
                                "id":"home_to_target",
                                "purpose":"navigation closure",
                                "from":"fixture/home",
                                "to":"fixture/target",
                                "click":{"kind":"rect","x":10,"y":20,"width":4,"height":6},
                                "unguarded_trusted_coordinate":true
                            },
                            {
                                "id":"tap_target",
                                "purpose":"deterministic target fixture",
                                "from":"fixture/home",
                                "to":null,
                                "click":{"kind":"target","target_id":"home_button"},
                                "guard":{
                                    "page_id":"fixture/home",
                                    "target_id":"home_button",
                                    "expected_rect":{"x":10,"y":20,"width":4,"height":6},
                                    "verify_template":"assets/home_button.png"
                                }
                            },
                            {
                                "id":"tap_target_center",
                                "purpose":"center target fixture",
                                "from":"fixture/home",
                                "to":null,
                                "click":{"kind":"target_center","target_id":"home_button"},
                                "guard":{
                                    "page_id":"fixture/home",
                                    "target_id":"home_button",
                                    "expected_rect":{"x":10,"y":20,"width":4,"height":6},
                                    "verify_template":"assets/home_button.png"
                                }
                            },
                            {
                                "id":"tap_target_offset",
                                "purpose":"offset target fixture",
                                "from":"fixture/home",
                                "to":null,
                                "click":{
                                    "kind":"target_center",
                                    "target_id":"home_button",
                                    "offset":{"x":1,"y":2,"width":2,"height":2}
                                },
                                "guard":{
                                    "page_id":"fixture/home",
                                    "target_id":"home_button",
                                    "expected_rect":{"x":10,"y":20,"width":4,"height":6},
                                    "verify_template":"assets/home_button.png"
                                }
                            }
                        ]
                    }),
                ),
                (
                    "resources/recognition/fixture.test.pack.json",
                    json!({
                        "schema_version":"0.3",
                        "coordinate_space":{"width":100,"height":100},
                        "targets":[
                            {
                                "type":"color",
                                "id":"home_anchor",
                                "region":{"x":0,"y":0,"width":1,"height":1},
                                "expected":[255,0,0]
                            },
                            {
                                "type":"template",
                                "id":"home_button",
                                "template_path":"operations/task/assets/home_button.png",
                                "region":{"x":10,"y":20,"width":4,"height":6},
                                "threshold":0.99,
                                "color_check":{
                                    "region":{"x":10,"y":20,"width":4,"height":6},
                                    "expected":[255,0,0]
                                },
                                "click":{"x":10,"y":20,"width":4,"height":6}
                            }
                        ]
                    }),
                ),
                (
                    "resources/recognition/fixture.test.pages.json",
                    json!({
                        "schema_version":"0.3",
                        "pages":[
                            {"id":"fixture/home","required":["home_anchor"]},
                            {"id":"fixture/target","required":["home_button"]}
                        ]
                    }),
                ),
                (
                    "resources/navigation/fixture.test.navigation.json",
                    json!({
                        "schema_version":"0.3",
                        "game":"fixture",
                        "server":"test",
                        "navigation":[{
                            "id":"home_to_target",
                            "from_page":"fixture/home",
                            "to_page":"fixture/target",
                            "click":{"kind":"rect","x":10,"y":20,"width":4,"height":6}
                        }],
                        "destructive_actions":[]
                    }),
                ),
            ],
            &[(
                "resources/operations/task/assets/home_button.png",
                RED_4X6_PNG,
            )],
        );
        let expected = ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(&bytes).to_string())
            .expect("expected hash");
        ExternallyVerifiedBundle::load("target_fixture", &bytes, expected)
            .expect("admitted target package")
    }

    fn package_zip(entries: &[(&str, Value)]) -> Vec<u8> {
        package_zip_with_binary(entries, &[])
    }

    fn package_zip_with_binary(
        entries: &[(&str, Value)],
        binary_entries: &[(&str, &[u8])],
    ) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (path, value) in entries {
            zip.start_file(*path, options).expect("zip entry");
            serde_json::to_writer(&mut zip, value).expect("zip JSON");
            zip.write_all(b"\n").expect("zip newline");
        }
        for (path, bytes) in binary_entries {
            zip.start_file(*path, options).expect("zip binary entry");
            zip.write_all(bytes).expect("zip binary bytes");
        }
        zip.finish().expect("finish zip").into_inner()
    }

    const RED_4X6_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x06, 0x08, 0x02, 0x00, 0x00, 0x00, 0x6b,
        0x5b, 0xa8, 0x22, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x47, 0x0c, 0x94, 0x72, 0x00, 0xbc, 0xbb, 0x17, 0xe9, 0x28, 0x27, 0x30,
        0xc4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
}
