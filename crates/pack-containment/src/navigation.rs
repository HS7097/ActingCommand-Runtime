// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::Value;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &["0.3", "0.4", "0.5"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationContractError {
    message: String,
}

impl NavigationContractError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for NavigationContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for NavigationContractError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavigationRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationInput {
    Tap {
        rect: NavigationRect,
    },
    TargetCenter {
        target_id: String,
    },
    Drag {
        from_rect: NavigationRect,
        to_rect: NavigationRect,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationRoute {
    id: String,
    from_page: String,
    to_page: String,
    input: NavigationInput,
    source: Option<String>,
}

impl NavigationRoute {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn from_page(&self) -> &str {
        &self.from_page
    }

    pub fn to_page(&self) -> &str {
        &self.to_page
    }

    pub fn input(&self) -> &NavigationInput {
        &self.input
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationPageAction {
    task_id: String,
    id: String,
    page: String,
    input: NavigationInput,
}

impl NavigationPageAction {
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn page(&self) -> &str {
        &self.page
    }

    pub fn input(&self) -> &NavigationInput {
        &self.input
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationDestructiveAction {
    task_id: Option<String>,
    id: Option<String>,
    page: Option<String>,
    rect: NavigationRect,
}

impl NavigationDestructiveAction {
    pub fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn page(&self) -> Option<&str> {
        self.page.as_deref()
    }

    pub const fn rect(&self) -> NavigationRect {
        self.rect
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationControlPoint {
    name: String,
    input: NavigationInput,
}

impl NavigationControlPoint {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn input(&self) -> &NavigationInput {
        &self.input
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationContract {
    schema_version: String,
    game: String,
    server: String,
    routes: Vec<NavigationRoute>,
    page_operations: Vec<NavigationPageAction>,
    destructive_actions: Vec<NavigationDestructiveAction>,
    control_points: Vec<NavigationControlPoint>,
}

impl NavigationContract {
    pub fn parse_json(source: &str) -> Result<Self, NavigationContractError> {
        let value: Value = serde_json::from_str(source).map_err(|error| {
            NavigationContractError::new(format!("failed to parse navigation JSON: {error}"))
        })?;
        Self::parse_value(&value)
    }

    pub fn parse_value(value: &Value) -> Result<Self, NavigationContractError> {
        let schema_version = required_string(value, "schema_version")?;
        if !SUPPORTED_SCHEMA_VERSIONS.contains(&schema_version.as_str()) {
            return Err(NavigationContractError::new(format!(
                "unsupported navigation schema_version '{schema_version}'; expected 0.3, 0.4, or 0.5"
            )));
        }
        let game = required_string(value, "game")?;
        let server = required_string(value, "server")?;

        let mut route_ids = BTreeSet::new();
        let routes = required_array(value, "navigation")?
            .iter()
            .map(parse_route)
            .map(|result| {
                let route = result?;
                if !route_ids.insert(route.id.clone()) {
                    return Err(NavigationContractError::new(format!(
                        "navigation route id '{}' is duplicated",
                        route.id
                    )));
                }
                Ok(route)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let page_operations = optional_array(value, "page_operations")?
            .unwrap_or(&[])
            .iter()
            .map(parse_page_action)
            .collect::<Result<Vec<_>, _>>()?;
        let destructive_actions = optional_array(value, "destructive_actions")?
            .unwrap_or(&[])
            .iter()
            .map(parse_destructive_action)
            .collect::<Result<Vec<_>, _>>()?;

        let mut control_point_names = BTreeSet::new();
        let control_points = optional_array(value, "control_points")?
            .unwrap_or(&[])
            .iter()
            .map(parse_control_point)
            .map(|result| {
                let point = result?;
                if !control_point_names.insert(point.name.clone()) {
                    return Err(NavigationContractError::new(format!(
                        "navigation control point name '{}' is duplicated",
                        point.name
                    )));
                }
                Ok(point)
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            schema_version,
            game,
            server,
            routes,
            page_operations,
            destructive_actions,
            control_points,
        })
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn game(&self) -> &str {
        &self.game
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn routes(&self) -> &[NavigationRoute] {
        &self.routes
    }

    pub fn page_operations(&self) -> &[NavigationPageAction] {
        &self.page_operations
    }

    pub fn destructive_actions(&self) -> &[NavigationDestructiveAction] {
        &self.destructive_actions
    }

    pub fn control_points(&self) -> &[NavigationControlPoint] {
        &self.control_points
    }
}

fn parse_route(value: &Value) -> Result<NavigationRoute, NavigationContractError> {
    Ok(NavigationRoute {
        id: required_string(value, "id")?,
        from_page: required_string(value, "from_page")?,
        to_page: required_string(value, "to_page")?,
        input: parse_navigation_input(required_value(value, "click")?)?,
        source: match value.get("source") {
            None | Some(Value::Null) => None,
            Some(Value::String(source)) => Some(source.clone()),
            Some(_) => {
                return Err(NavigationContractError::new(
                    "field 'source' must be a string when present",
                ));
            }
        },
    })
}

fn parse_page_action(value: &Value) -> Result<NavigationPageAction, NavigationContractError> {
    Ok(NavigationPageAction {
        task_id: required_string(value, "task_id")?,
        id: required_string(value, "id")?,
        page: required_string(value, "page")?,
        input: parse_navigation_input(required_value(value, "click")?)?,
    })
}

fn parse_destructive_action(
    value: &Value,
) -> Result<NavigationDestructiveAction, NavigationContractError> {
    let object = value.as_object().ok_or_else(|| {
        NavigationContractError::new("destructive_actions entries must be objects")
    })?;
    let has_identity = object.contains_key("task_id");
    let (task_id, id, page) = if has_identity {
        (
            Some(required_string(value, "task_id")?),
            Some(required_string(value, "id")?),
            Some(required_string(value, "page")?),
        )
    } else {
        (None, None, optional_string(value, "page")?)
    };
    let rect = parse_navigation_tap_rect(required_value(value, "click")?)?;
    Ok(NavigationDestructiveAction {
        task_id,
        id,
        page,
        rect,
    })
}

fn parse_control_point(value: &Value) -> Result<NavigationControlPoint, NavigationContractError> {
    let name = required_string(value, "name")?;
    let input = if let Some(click) = value.get("click") {
        parse_navigation_input(click)?
    } else {
        NavigationInput::Tap {
            rect: parse_control_point_rect(value)?,
        }
    };
    if value.get("note").is_some_and(|note| !note.is_string()) {
        return Err(NavigationContractError::new(
            "field 'note' must be a string",
        ));
    }
    Ok(NavigationControlPoint { name, input })
}

fn parse_navigation_input(value: &Value) -> Result<NavigationInput, NavigationContractError> {
    match required_string(value, "kind")?.as_str() {
        "point" | "rect" => Ok(NavigationInput::Tap {
            rect: parse_navigation_tap_rect(value)?,
        }),
        "target" | "target_center" => Ok(NavigationInput::TargetCenter {
            target_id: required_string(value, "target_id")?,
        }),
        "drag" => {
            let duration_ms = match value.get("duration_ms") {
                None => 500,
                Some(value) => value.as_u64().ok_or_else(|| {
                    NavigationContractError::new("field 'duration_ms' must be an unsigned integer")
                })?,
            };
            if duration_ms == 0 {
                return Err(NavigationContractError::new(
                    "field 'duration_ms' must be greater than zero",
                ));
            }
            Ok(NavigationInput::Drag {
                from_rect: parse_navigation_tap_rect(required_value(value, "from")?)?,
                to_rect: parse_navigation_tap_rect(required_value(value, "to")?)?,
                duration_ms,
            })
        }
        other => Err(NavigationContractError::new(format!(
            "unsupported navigation click kind: '{other}'"
        ))),
    }
}

fn parse_navigation_tap_rect(value: &Value) -> Result<NavigationRect, NavigationContractError> {
    let rect = match value.get("kind") {
        Some(Value::String(kind)) if kind == "point" => parse_navigation_point(value)?,
        Some(Value::String(kind)) if kind == "rect" => parse_navigation_rect(value)?,
        None => parse_navigation_rect(value)?,
        Some(Value::String(kind)) => {
            return Err(NavigationContractError::new(format!(
                "unsupported navigation click kind for tap rectangle: '{kind}'"
            )));
        }
        Some(_) => {
            return Err(NavigationContractError::new(
                "field 'kind' must be a string when present",
            ));
        }
    };
    validate_rect(rect)?;
    Ok(rect)
}

fn parse_navigation_point(value: &Value) -> Result<NavigationRect, NavigationContractError> {
    let (x, y) = if let Some(point) = value.get("point") {
        parse_point_value(point)?
    } else {
        (required_i32(value, "x")?, required_i32(value, "y")?)
    };
    Ok(NavigationRect {
        x,
        y,
        width: 1,
        height: 1,
    })
}

fn parse_navigation_rect(value: &Value) -> Result<NavigationRect, NavigationContractError> {
    Ok(NavigationRect {
        x: required_i32(value, "x")?,
        y: required_i32(value, "y")?,
        width: required_i32(value, "width")?,
        height: required_i32(value, "height")?,
    })
}

fn parse_control_point_rect(value: &Value) -> Result<NavigationRect, NavigationContractError> {
    let rect = if let Some(point) = value.get("point") {
        let (x, y) = parse_point_value(point)?;
        NavigationRect {
            x,
            y,
            width: 1,
            height: 1,
        }
    } else {
        NavigationRect {
            x: required_i32(value, "x")?,
            y: required_i32(value, "y")?,
            width: 1,
            height: 1,
        }
    };
    validate_rect(rect)?;
    Ok(rect)
}

fn validate_rect(rect: NavigationRect) -> Result<(), NavigationContractError> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(NavigationContractError::new(format!(
            "navigation rectangle must have positive dimensions: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(())
}

fn parse_point_value(value: &Value) -> Result<(i32, i32), NavigationContractError> {
    if let Some(point) = value.as_str() {
        let parts = point.split(',').map(str::trim).collect::<Vec<_>>();
        if parts.len() != 2 {
            return Err(NavigationContractError::new(format!(
                "point must be formatted as x,y: {point}"
            )));
        }
        let x = parts[0].parse::<i32>().map_err(|error| {
            NavigationContractError::new(format!("failed to parse point x '{}': {error}", parts[0]))
        })?;
        let y = parts[1].parse::<i32>().map_err(|error| {
            NavigationContractError::new(format!("failed to parse point y '{}': {error}", parts[1]))
        })?;
        return Ok((x, y));
    }
    if let Some(items) = value.as_array() {
        if items.len() != 2 {
            return Err(NavigationContractError::new(
                "point array must have exactly two items",
            ));
        }
        return Ok((
            parse_i32(&items[0], "point[0]")?,
            parse_i32(&items[1], "point[1]")?,
        ));
    }
    Err(NavigationContractError::new(
        "point must be a string x,y or [x,y] array",
    ))
}

fn required_value<'a>(value: &'a Value, name: &str) -> Result<&'a Value, NavigationContractError> {
    value
        .get(name)
        .ok_or_else(|| NavigationContractError::new(format!("missing field '{name}'")))
}

fn required_string(value: &Value, name: &str) -> Result<String, NavigationContractError> {
    match required_value(value, name)? {
        Value::String(value) if !value.trim().is_empty() => Ok(value.clone()),
        _ => Err(NavigationContractError::new(format!(
            "field '{name}' must be a non-empty string"
        ))),
    }
}

fn optional_string(value: &Value, name: &str) -> Result<Option<String>, NavigationContractError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        Some(_) => Err(NavigationContractError::new(format!(
            "field '{name}' must be a non-empty string when present"
        ))),
    }
}

fn required_array<'a>(
    value: &'a Value,
    name: &str,
) -> Result<&'a [Value], NavigationContractError> {
    required_value(value, name)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| NavigationContractError::new(format!("field '{name}' must be an array")))
}

fn optional_array<'a>(
    value: &'a Value,
    name: &str,
) -> Result<Option<&'a [Value]>, NavigationContractError> {
    match value.get(name) {
        None => Ok(None),
        Some(Value::Array(values)) => Ok(Some(values.as_slice())),
        Some(_) => Err(NavigationContractError::new(format!(
            "field '{name}' must be an array when present"
        ))),
    }
}

fn required_i32(value: &Value, name: &str) -> Result<i32, NavigationContractError> {
    parse_i32(required_value(value, name)?, name)
}

fn parse_i32(value: &Value, name: &str) -> Result<i32, NavigationContractError> {
    if let Some(value) = value.as_i64() {
        return i32::try_from(value).map_err(|_| {
            NavigationContractError::new(format!("field '{name}' exceeds i32 range"))
        });
    }
    if let Some(value) = value.as_u64() {
        return i32::try_from(value).map_err(|_| {
            NavigationContractError::new(format!("field '{name}' exceeds i32 range"))
        });
    }
    Err(NavigationContractError::new(format!(
        "field '{name}' must be an integer"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_navigation() -> Value {
        json!({
            "schema_version": "0.3",
            "game": "neutral",
            "server": "test",
            "navigation": [{
                "id": "home_to_terminal",
                "from_page": "neutral/home",
                "to_page": "neutral/terminal",
                "click": {"kind": "point", "point": [1, 2]}
            }],
            "page_operations": [{
                "task_id": "task",
                "id": "dismiss",
                "page": "neutral/terminal",
                "click": {"kind": "rect", "x": 3, "y": 4, "width": 5, "height": 6}
            }],
            "destructive_actions": [{
                "id": "danger_region",
                "page": "neutral/terminal",
                "click": {"kind": "rect", "x": 10, "y": 10, "width": 2, "height": 2}
            }],
            "control_points": [{"name": "home", "point": [0, 0]}]
        })
    }

    #[test]
    fn parses_versioned_navigation_into_typed_contract() {
        let contract = NavigationContract::parse_value(&valid_navigation()).expect("contract");

        assert_eq!(contract.schema_version(), "0.3");
        assert_eq!(contract.game(), "neutral");
        assert_eq!(contract.server(), "test");
        assert_eq!(contract.routes()[0].id(), "home_to_terminal");
        assert_eq!(contract.page_operations()[0].task_id(), "task");
        assert_eq!(
            contract.destructive_actions()[0].page(),
            Some("neutral/terminal")
        );
        assert_eq!(contract.control_points()[0].name(), "home");
    }

    #[test]
    fn rejects_malformed_route_destructive_action_and_control_point() {
        for (label, mutate) in [("route", 0_u8), ("destructive", 1_u8), ("control", 2_u8)] {
            let mut value = valid_navigation();
            match mutate {
                0 => value["navigation"][0]["click"]["kind"] = json!("unsupported"),
                1 => value["destructive_actions"] = json!([{}]),
                2 => value["control_points"] = json!([{}]),
                _ => unreachable!(),
            }

            NavigationContract::parse_value(&value)
                .expect_err(&format!("{label} mutation must fail"));
        }
    }
}
