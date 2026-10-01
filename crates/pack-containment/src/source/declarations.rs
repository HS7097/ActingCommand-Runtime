// SPDX-License-Identifier: AGPL-3.0-only

use super::{Bundle, CliError, CliOutcome, ParseFiles, SourceRead};
use actingcommand_contract::candidate_projection::{
    CANDIDATE_PROJECTION_MAX_CANDIDATES, CANDIDATE_PROJECTION_MAX_FEATURES,
    validate_candidate_feature_name, validate_candidate_layout_id,
};
use actingcommand_contract::{ResourceDeclarationIssue, ResourceDeclarationReason};
use actingcommand_recognition::color_digest::{
    self, ColorDigest, ColorDigestAlgorithm, ColorDigestGrid, MAX_GRID_AXIS,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Members of one `checks` entry (`contracts/selection-graph.md`, section Checks).
const CHECK_MEMBERS: std::ops::RangeInclusive<usize> = 2..=8;

struct Declaration<'a> {
    file: &'a Path,
    schema: Option<&'a str>,
}

/// A refusal of `bundle`'s `task.json` at `pointer`, for a rule the parser checks while it
/// derives the pack, where the referenced targets may come from other tasks.
pub(super) fn task_declaration_error(
    bundle: &Bundle,
    pointer: &str,
    reason: ResourceDeclarationReason,
    detail: &str,
) -> CliError {
    let file = bundle.task_json_path();
    let mut error = Declaration {
        file: &file,
        schema: bundle.data.get("schema_version").and_then(Value::as_str),
    }
    .error(pointer, reason);
    error.message = format!("{}; {detail}", error.message);
    error
}

impl Declaration<'_> {
    /// Task schemas `0.6` through `0.9`, which accept the declarations of pack schema `0.7`
    /// (`checks`, color digests, per-target `max_distance`, `guard.check` and
    /// `candidate_layouts`).
    fn schema_0_6_or_later(&self) -> bool {
        matches!(self.schema, Some("0.6" | "0.7" | "0.8" | "0.9"))
    }

    fn control(&self, value: &Value) -> CliOutcome<()> {
        let object = self.object(
            value,
            "",
            &[
                "schema_version",
                "package_id",
                "execution_mode",
                "game",
                "server",
                "resolution",
                "entry_task_id",
                "resource_root",
                "phases",
                "capture_interval_ms",
                "timeout_ms",
                "step_timeout_ms",
                "max_steps",
                "stop_on_confirmation",
                "stability_termination",
            ],
        )?;
        if object.contains_key("phases")
            && self.schema != Some(actingcommand_contract::PHASED_CONTROL_SCHEMA)
        {
            return Err(self.error("/phases", ResourceDeclarationReason::UnconsumedField));
        }
        for (field, value) in object {
            let pointer = child("", field);
            match field.as_str() {
                "resolution" => {
                    let resolution = self.object(value, &pointer, &["width", "height"])?;
                    for field in ["width", "height"] {
                        self.unsigned(
                            self.required(resolution, &pointer, field)?,
                            &child(&pointer, field),
                        )?;
                    }
                }
                "phases" => self.phases(value, &pointer)?,
                "stability_termination" => self.stability(value, &pointer)?,
                "stop_on_confirmation" if !value.is_null() => self.boolean(value, &pointer)?,
                "capture_interval_ms" | "timeout_ms" | "step_timeout_ms" | "max_steps"
                    if !value.is_null() =>
                {
                    self.unsigned(value, &pointer)?
                }
                "schema_version" | "package_id" | "execution_mode" | "game" | "server"
                | "entry_task_id" => self.string(value, &pointer)?,
                "resource_root" if !value.is_null() => self.string(value, &pointer)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn manifest(&self, value: &Value) -> CliOutcome<()> {
        let object = self.object(
            value,
            "",
            &["schema_version", "entry_task_id", "hashes", "files"],
        )?;
        for (field, value) in object {
            let pointer = child("", field);
            match field.as_str() {
                "schema_version" | "entry_task_id" => self.string(value, &pointer)?,
                "hashes" => {
                    let hashes = value.as_object().ok_or_else(|| {
                        self.error(&pointer, ResourceDeclarationReason::InvalidType)
                    })?;
                    for (path, hash) in hashes {
                        self.string(hash, &child(&pointer, path))?;
                    }
                }
                "files" => {
                    for (index, file) in self.array(value, &pointer)?.iter().enumerate() {
                        let pointer = child(&pointer, &index.to_string());
                        let file =
                            self.object(file, &pointer, &["path", "sha256", "hash", "bytes"])?;
                        self.string(
                            self.required(file, &pointer, "path")?,
                            &child(&pointer, "path"),
                        )?;
                        for (field, value) in file {
                            if field == "bytes" {
                                // PackageValidationResponse preserves the declared manifest metadata.
                                self.unsigned(value, &child(&pointer, field))?;
                            } else if !value.is_null() {
                                self.string(value, &child(&pointer, field))?;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn navigation_coordinate(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let coordinate = value
            .as_i64()
            .ok_or_else(|| self.error(pointer, ResourceDeclarationReason::InvalidType))?;
        i32::try_from(coordinate)
            .map(|_| ())
            .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))
    }

    fn point(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if let Some(point) = value.as_str() {
            let parts: Vec<_> = point.split(',').collect();
            if parts.len() != 2 || parts.iter().any(|part| part.trim().parse::<i32>().is_err()) {
                return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
            }
        } else if let Some(point) = value.as_array() {
            if point.len() != 2 {
                return Err(self.error(pointer, ResourceDeclarationReason::InvalidType));
            }
            for (index, value) in point.iter().enumerate() {
                self.navigation_coordinate(value, &child(pointer, &index.to_string()))?;
            }
        } else {
            let object = self.object(value, pointer, &["x", "y"])?;
            for field in ["x", "y"] {
                self.navigation_coordinate(
                    self.required(object, pointer, field)?,
                    &child(pointer, field),
                )?;
            }
        }
        Ok(())
    }

    fn navigation_click(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let kind = value.get("kind").and_then(Value::as_str);
        let fields: &[&str] = match kind {
            Some("point") => &["kind", "point", "x", "y"],
            Some("rect") | None => &["kind", "x", "y", "width", "height"],
            Some("target" | "target_center") => {
                if value.get("offset").is_some() {
                    return Err(self.error(
                        &child(pointer, "offset"),
                        ResourceDeclarationReason::UnconsumedField,
                    ));
                }
                &["kind", "target_id"]
            }
            Some("drag") => &["kind", "from", "to", "duration_ms"],
            // Generated task metadata preserves this supported contained-task action.
            Some("single_touch_drag_with_vertical_brake_v1") => {
                return self.click(value, pointer, true);
            }
            Some("long_press" | "long_tap" | "offset" | "specific_rect") => {
                return self.click(value, pointer, false);
            }
            _ => {
                return Err(self.error(
                    &child(pointer, "kind"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        };
        let object = self.object(value, pointer, fields)?;
        let required: &[&str] = match kind {
            Some("point") if object.contains_key("point") => &["point"],
            Some("point") => &["x", "y"],
            Some("rect") | None => &["x", "y", "width", "height"],
            Some("target" | "target_center") => &["target_id"],
            Some("drag") => &["from", "to"],
            _ => &[],
        };
        for field in required {
            self.required(object, pointer, field)?;
        }
        for (field, value) in object {
            let pointer = child(pointer, field);
            match field.as_str() {
                "kind" | "target_id" => self.string(value, &pointer)?,
                "point" => self.point(value, &pointer)?,
                "from" | "to" => self.navigation_click(value, &pointer)?,
                "duration_ms" => self.unsigned(value, &pointer)?,
                "x" | "y" | "width" | "height" => {
                    self.navigation_coordinate(value, &pointer)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn navigation(&self, value: &Value) -> CliOutcome<()> {
        let object = self.object(
            value,
            "",
            &[
                "schema_version",
                "converter_schema_version",
                "generated",
                "generated_by",
                "game",
                "server",
                "coordinate_space",
                "control_points",
                "navigation",
                "page_operations",
                "destructive_actions",
            ],
        )?;
        // Source metadata may contain only control points; generated edges are
        // validated when declared.
        for (field, value) in object {
            let pointer = child("", field);
            match field.as_str() {
                "schema_version"
                | "converter_schema_version"
                | "generated_by"
                | "game"
                | "server" => self.string(value, &pointer)?,
                "generated" => self.boolean(value, &pointer)?,
                "coordinate_space" => {
                    let object = self.object(value, &pointer, &["width", "height"])?;
                    for field in ["width", "height"] {
                        self.unsigned(
                            self.required(object, &pointer, field)?,
                            &child(&pointer, field),
                        )?;
                    }
                }
                "control_points" => {
                    for (index, point) in self.array(value, &pointer)?.iter().enumerate() {
                        self.control_point(point, &child(&pointer, &index.to_string()))?;
                    }
                }
                "navigation" | "page_operations" | "destructive_actions" => {
                    for (index, entry) in self.array(value, &pointer)?.iter().enumerate() {
                        let pointer = child(&pointer, &index.to_string());
                        let fields: &[&str] = if field == "navigation" {
                            &["id", "from_page", "to_page", "click", "source"]
                        } else {
                            &[
                                "task_id",
                                "page",
                                "id",
                                "purpose",
                                "click",
                                "expect_after",
                                "verify_template",
                                "consumes",
                                "produces",
                            ]
                        };
                        let entry = self.object(entry, &pointer, fields)?;
                        if field == "navigation" {
                            for name in ["id", "from_page", "to_page"] {
                                self.string(
                                    self.required(entry, &pointer, name)?,
                                    &child(&pointer, name),
                                )?;
                            }
                        }
                        if field == "navigation" || field == "destructive_actions" {
                            self.required(entry, &pointer, "click")?;
                        }
                        for (field, value) in entry {
                            let pointer = child(&pointer, field);
                            match field.as_str() {
                                "click" => self.navigation_click(value, &pointer)?,
                                "consumes" | "produces" => self.strings(value, &pointer)?,
                                "expect_after" if !value.is_null() => {
                                    let expected = self.object(
                                        value,
                                        &pointer,
                                        &["page_id", "timeout_ms", "interval_ms"],
                                    )?;
                                    self.page(
                                        self.required(expected, &pointer, "page_id")?,
                                        &child(&pointer, "page_id"),
                                    )?;
                                    for field in ["timeout_ms", "interval_ms"] {
                                        if let Some(value) =
                                            expected.get(field).filter(|value| !value.is_null())
                                        {
                                            self.unsigned(value, &child(&pointer, field))?;
                                        }
                                    }
                                }
                                "expect_after" => {}
                                _ if !value.is_null() => self.string(value, &pointer)?,
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn operation(&self, value: &Value, pointer: &str, canonical: bool) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &[
                "id",
                "from",
                "to",
                "expect_after",
                "click",
                "application",
                "on_error",
                "retryable",
                "max_attempts",
                "retry_interval_ms",
                "timeout_ms",
                "pre_delay_ms",
                "post_delay_ms",
                "pre_wait_freezes_ms",
                "post_wait_freezes_ms",
                "effect",
                "destructive",
                "guard",
                "unguarded_trusted_coordinate",
                "verify_template",
                "consumes",
                "produces",
                "purpose",
                "verified_live",
                "provenance",
                "threshold",
                "method",
                "mask",
                "maskRange",
                "rect_move",
                "maa_task",
                "maa_task_id",
            ],
        )?;
        if object.get("verify_template").is_none_or(Value::is_null) {
            for field in [
                "threshold",
                "method",
                "mask",
                "maskRange",
                "rect_move",
                "maa_task",
                "maa_task_id",
            ] {
                if object.contains_key(field) {
                    return Err(self.error(
                        &child(pointer, field),
                        ResourceDeclarationReason::UnconsumedField,
                    ));
                }
            }
        }
        for field in ["id", "from"] {
            self.string(
                self.required(object, pointer, field)?,
                &child(pointer, field),
            )?;
        }
        // Exactly one effect: `click`, or the `application` effect of slice #316-B3.
        match (
            object.get("click").filter(|value| !value.is_null()),
            object.get("application").filter(|value| !value.is_null()),
        ) {
            (Some(click), None) => self.click(click, &child(pointer, "click"), canonical)?,
            (None, Some(application)) => {
                self.application(application, &child(pointer, "application"))?;
            }
            (Some(_), Some(_)) => {
                return Err(self.error(
                    &child(pointer, "application"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            (None, None) => {
                let click = self.required(object, pointer, "click")?;
                self.click(click, &child(pointer, "click"), canonical)?;
            }
        }
        for (field, value) in object {
            let pointer = child(pointer, field);
            match field.as_str() {
                "to" if !value.is_null() => self.page(value, &pointer)?,
                "expect_after" if !value.is_null() => {
                    let expected =
                        self.object(value, &pointer, &["page_id", "timeout_ms", "interval_ms"])?;
                    self.page(
                        self.required(expected, &pointer, "page_id")?,
                        &child(&pointer, "page_id"),
                    )?;
                    for field in ["timeout_ms", "interval_ms"] {
                        if let Some(value) = expected.get(field).filter(|value| !value.is_null()) {
                            self.unsigned(value, &child(&pointer, field))?;
                        }
                    }
                }
                "on_error" | "effect" | "verify_template" | "purpose" | "maa_task"
                | "maa_task_id"
                    if !value.is_null() =>
                {
                    self.string(value, &pointer)?
                }
                "retryable" | "verified_live" if !value.is_null() => {
                    self.boolean(value, &pointer)?
                }
                "unguarded_trusted_coordinate" => self.boolean(value, &pointer)?,
                "max_attempts"
                | "retry_interval_ms"
                | "timeout_ms"
                | "pre_delay_ms"
                | "post_delay_ms"
                | "pre_wait_freezes_ms"
                | "post_wait_freezes_ms"
                    if !value.is_null() =>
                {
                    self.unsigned(value, &pointer)?
                }
                "destructive" => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::UnconsumedField));
                }
                "consumes" | "produces" => self.strings(value, &pointer)?,
                "guard" if !value.is_null() => self.guard(value, &pointer)?,
                "provenance" if !value.is_null() => {
                    // Authoring/package output and Lab records preserve the original JSON;
                    // navigation_ref also feeds the generated navigation source.
                    let provenance = value.as_object().ok_or_else(|| {
                        self.error(&pointer, ResourceDeclarationReason::InvalidType)
                    })?;
                    if let Some(value) = provenance.get("navigation_ref") {
                        self.string(value, &child(&pointer, "navigation_ref"))?;
                    }
                }
                "threshold" => self.number(value, &pointer)?,
                "method" => self.method(value, &pointer)?,
                "mask" | "maskRange" => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::UnconsumedField));
                }
                "rect_move" => self.rect_move(value, &pointer)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// The `application` effect (slice #316-B3): `{ "action": "launch" | "restart" | "stop" }`,
    /// nothing else. The package name is never declared here; it is the instance's pointer.
    fn application(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["action"])?;
        let action = self.required(object, pointer, "action")?;
        let pointer = child(pointer, "action");
        self.string(action, &pointer)?;
        if !matches!(action.as_str(), Some("launch" | "restart" | "stop")) {
            return Err(self.error(&pointer, ResourceDeclarationReason::InvalidValue));
        }
        Ok(())
    }

    fn click(&self, value: &Value, pointer: &str, canonical: bool) -> CliOutcome<()> {
        let kind = value.get("kind").and_then(Value::as_str).ok_or_else(|| {
            self.error(
                &child(pointer, "kind"),
                ResourceDeclarationReason::InvalidType,
            )
        })?;
        let fields: &[&str] = match kind {
            "point" => &["kind", "x", "y"],
            "long_press" | "long_tap" => &["kind", "x", "y", "duration_ms"],
            "rect" | "specific_rect" => &["kind", "x", "y", "width", "height"],
            "offset" => &["kind", "target_id", "offset"],
            "target" | "target_center" => &["kind", "target_id", "offset"],
            "drag" if canonical && value.get("from").is_none() && value.get("to").is_none() => {
                &["kind", "from_rect", "to_rect", "duration_ms"]
            }
            "drag" => &["kind", "from", "to", "duration_ms"],
            "single_touch_drag_with_vertical_brake_v1" if canonical => &[
                "kind",
                "from_rect",
                "corner_rect",
                "horizontal_duration_ms",
                "corner_hold_ms",
                "brake_distance_px",
                "brake_duration_ms",
            ],
            "single_touch_drag_with_vertical_brake_v1" => &[
                "kind",
                "from",
                "corner",
                "horizontal_duration_ms",
                "corner_hold_ms",
                "brake_distance_px",
                "brake_duration_ms",
            ],
            _ => {
                return Err(self.error(
                    &child(pointer, "kind"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        };
        let object = self.object(value, pointer, fields)?;
        for field in fields.iter().filter(|field| **field != "kind") {
            if (*field == "target_id" && kind == "offset")
                || (*field == "offset" && kind != "offset")
            {
                if let Some(value) = object.get(*field).filter(|value| !value.is_null()) {
                    if *field == "target_id" {
                        self.string(value, &child(pointer, field))?;
                    } else {
                        self.rect(value, &child(pointer, field))?;
                    }
                }
                continue;
            }
            let value = self.required(object, pointer, field)?;
            let pointer = child(pointer, field);
            match *field {
                "from" | "to" | "corner" | "from_rect" | "to_rect" | "corner_rect" | "offset" => {
                    self.rect(value, &pointer)?
                }
                "target_id" => self.string(value, &pointer)?,
                "x" | "y" | "width" | "height" | "brake_distance_px" => {
                    if value.as_i64().is_none() {
                        return Err(self.error(&pointer, ResourceDeclarationReason::InvalidType));
                    }
                }
                _ => self.unsigned(value, &pointer)?,
            }
        }
        Ok(())
    }

    fn guard(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &[
                "page_id",
                "target_id",
                "expected_rect",
                "verify_template",
                "color_probe",
                "check",
            ],
        )?;
        if object.contains_key("check") && !self.schema_0_6_or_later() {
            return Err(self.error(
                &child(pointer, "check"),
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
        for field in ["page_id", "target_id"] {
            self.string(
                self.required(object, pointer, field)?,
                &child(pointer, field),
            )?;
        }
        self.rect(
            self.required(object, pointer, "expected_rect")?,
            &child(pointer, "expected_rect"),
        )?;
        for field in ["verify_template", "color_probe", "check"] {
            if let Some(value) = object.get(field).filter(|value| !value.is_null()) {
                self.string(value, &child(pointer, field))?;
            }
        }
        Ok(())
    }

    fn method(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if value.is_string() || value.as_i64().is_some() {
            super::normalize_maa_method(value)
                .map(|_| ())
                .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))
        } else {
            Err(self.error(pointer, ResourceDeclarationReason::InvalidType))
        }
    }

    fn rect_move(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if let Some(values) = value.as_array() {
            if values.len() != 4 {
                return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
            }
            for (index, value) in values.iter().enumerate() {
                if value.as_i64().is_none() {
                    return Err(self.error(
                        &child(pointer, &index.to_string()),
                        ResourceDeclarationReason::InvalidType,
                    ));
                }
            }
            Ok(())
        } else {
            self.rect(value, pointer)
        }
    }

    fn source_region(&self, value: &Value, pointer: &str, relative: bool) -> CliOutcome<()> {
        let mode = value.get("mode").and_then(Value::as_str).ok_or_else(|| {
            self.error(
                &child(pointer, "mode"),
                ResourceDeclarationReason::InvalidType,
            )
        })?;
        match mode {
            "full_frame" => {
                self.object(value, pointer, &["mode"])?;
            }
            "rect" => {
                let object = self.object(value, pointer, &["mode", "rect"])?;
                self.rect(
                    self.required(object, pointer, "rect")?,
                    &child(pointer, "rect"),
                )?;
            }
            "template_relative" if relative => {
                let object = self.object(
                    value,
                    pointer,
                    &["mode", "anchor_target_id", "offset", "width", "height"],
                )?;
                self.string(
                    self.required(object, pointer, "anchor_target_id")?,
                    &child(pointer, "anchor_target_id"),
                )?;
                for field in ["width", "height"] {
                    self.unsigned(
                        self.required(object, pointer, field)?,
                        &child(pointer, field),
                    )?;
                }
                let offset = self.object(
                    self.required(object, pointer, "offset")?,
                    &child(pointer, "offset"),
                    &["x", "y"],
                )?;
                for field in ["x", "y"] {
                    if self
                        .required(offset, &child(pointer, "offset"), field)?
                        .as_i64()
                        .is_none()
                    {
                        return Err(self.error(
                            &child(&child(pointer, "offset"), field),
                            ResourceDeclarationReason::InvalidType,
                        ));
                    }
                }
            }
            _ => {
                return Err(self.error(
                    &child(pointer, "mode"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        }
        Ok(())
    }

    fn target(
        &self,
        value: &Value,
        pointer: &str,
        family: &str,
        canonical: bool,
        frame: [u64; 2],
    ) -> CliOutcome<()> {
        let fields: &[&str] = match family {
            "anchors" => &[
                "id",
                "template",
                "region",
                "threshold",
                "method",
                "mask",
                "maskRange",
                "rect_move",
                "color_check",
                "maa_task",
                "maa_task_id",
                "provenance",
            ],
            "verify_templates" => &[
                "id",
                "template",
                "region",
                "threshold",
                "method",
                "mask",
                "maskRange",
                "rect_move",
                "maa_task",
                "maa_task_id",
                "provenance",
            ],
            "color_probes" => &[
                "id",
                "region",
                "expected",
                "max_distance",
                "digest",
                "provenance",
            ],
            "ocr_targets" => &[
                "id",
                "region",
                "languages",
                "timeout_ms",
                "match_mode",
                "expected",
                "case_sensitive",
                "minimum_confidence",
                "model_ref",
                "model_sha256",
                "click",
            ],
            _ => unreachable!("fixed declaration family"),
        };
        let object = self.object(value, pointer, fields)?;
        self.string(self.required(object, pointer, "id")?, &child(pointer, "id"))?;
        if let Some(region) = object.get("region") {
            self.source_region(region, &child(pointer, "region"), family == "ocr_targets")?;
        } else if !canonical || family != "anchors" {
            self.required(object, pointer, "region")?;
        }
        if matches!(family, "anchors" | "verify_templates") {
            self.string(
                self.required(object, pointer, "template")?,
                &child(pointer, "template"),
            )?;
        }
        for (field, value) in object {
            let pointer = child(pointer, field);
            match field.as_str() {
                "maa_task" | "maa_task_id" | "match_mode" | "model_ref" | "model_sha256" => {
                    self.string(value, &pointer)?
                }
                "threshold" | "minimum_confidence" => self.number(value, &pointer)?,
                "timeout_ms" => self.unsigned(value, &pointer)?,
                "case_sensitive" => self.boolean(value, &pointer)?,
                "provenance" if !value.is_null() && !value.is_object() => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::InvalidType));
                }
                "languages" => self.strings(value, &pointer)?,
                "expected" if family == "ocr_targets" => self.strings(value, &pointer)?,
                "expected" => self.color(value, &pointer)?,
                "max_distance" => self.max_distance(value, &pointer)?,
                "click" if !value.is_null() => self.rect(value, &pointer)?,
                "rect_move" => self.rect_move(value, &pointer)?,
                "method" => self.method(value, &pointer)?,
                "mask" | "maskRange" => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::UnconsumedField));
                }
                "color_check" if !value.is_null() => {
                    let check =
                        self.object(value, &pointer, &["region", "expected", "max_distance"])?;
                    let region = self.required(check, &pointer, "region")?;
                    if region.get("mode").is_some() {
                        self.source_region(region, &child(&pointer, "region"), true)?;
                    } else {
                        self.rect(region, &child(&pointer, "region"))?;
                    }
                    self.color(
                        self.required(check, &pointer, "expected")?,
                        &child(&pointer, "expected"),
                    )?;
                    if let Some(distance) = check.get("max_distance") {
                        self.max_distance(distance, &child(&pointer, "max_distance"))?;
                    }
                }
                _ => {}
            }
        }
        if family == "ocr_targets" {
            for field in [
                "languages",
                "timeout_ms",
                "match_mode",
                "expected",
                "case_sensitive",
                "minimum_confidence",
                "model_ref",
                "model_sha256",
            ] {
                self.required(object, pointer, field)?;
            }
        } else if family == "color_probes" {
            // A color probe matches a mean color (`expected`) or verifies a digest, never both.
            match (object.get("expected"), object.get("digest")) {
                (_, None) => {
                    self.required(object, pointer, "expected")?;
                }
                (None, Some(digest)) => {
                    if object.contains_key("max_distance") {
                        return Err(self.error(
                            &child(pointer, "max_distance"),
                            ResourceDeclarationReason::UnconsumedField,
                        ));
                    }
                    self.color_digest(
                        digest,
                        &child(pointer, "digest"),
                        self.required(object, pointer, "region")?,
                        &child(pointer, "region"),
                        frame,
                    )?;
                }
                (Some(_), Some(_)) => {
                    return Err(self.error(
                        &child(pointer, "digest"),
                        ResourceDeclarationReason::InvalidValue,
                    ));
                }
            }
        }
        Ok(())
    }

    /// A per-target color threshold of pack schema `0.7`: a finite number `>= 0`.
    fn max_distance(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if !self.schema_0_6_or_later() {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let distance = value
            .as_f64()
            .ok_or_else(|| self.error(pointer, ResourceDeclarationReason::InvalidType))?;
        if distance < 0.0 || !(distance as f32).is_finite() {
            return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
        }
        Ok(())
    }

    /// A `color_digest.v1` declaration (`contracts/color-digest.md`): an absolute region
    /// inside the coordinate space, a grid that fits it, and cells, exclusions and thresholds
    /// that parse. Nothing is defaulted, clipped or normalized.
    fn color_digest(
        &self,
        value: &Value,
        pointer: &str,
        region: &Value,
        region_pointer: &str,
        frame: [u64; 2],
    ) -> CliOutcome<()> {
        if !self.schema_0_6_or_later() {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let object = self.object(
            value,
            pointer,
            &[
                "algorithm",
                "columns",
                "rows",
                "cells",
                "exclude_cells",
                "max_mean_milli",
                "max_cell",
            ],
        )?;
        let extent = self.digest_region(region, region_pointer, frame)?;
        let algorithm_pointer = child(pointer, "algorithm");
        let algorithm = self
            .required(object, pointer, "algorithm")?
            .as_str()
            .ok_or_else(|| {
                self.error(&algorithm_pointer, ResourceDeclarationReason::InvalidType)
            })?;
        ColorDigestAlgorithm::parse(algorithm)
            .map_err(|_| self.error(&algorithm_pointer, ResourceDeclarationReason::InvalidValue))?;
        let mut axes = [0_u32; 2];
        for ((axis, field), length) in axes.iter_mut().zip(["columns", "rows"]).zip(extent) {
            let field_pointer = child(pointer, field);
            let count = self.digest_u32(self.required(object, pointer, field)?, &field_pointer)?;
            if !(1..=MAX_GRID_AXIS).contains(&count) || u64::from(count) > length {
                return Err(self.error(&field_pointer, ResourceDeclarationReason::InvalidValue));
            }
            *axis = count;
        }
        let grid = ColorDigestGrid::new(axes[0], axes[1])
            .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))?;
        let cells_pointer = child(pointer, "cells");
        let cells = self
            .required(object, pointer, "cells")?
            .as_str()
            .ok_or_else(|| self.error(&cells_pointer, ResourceDeclarationReason::InvalidType))?;
        ColorDigest::from_hex(grid, cells)
            .map_err(|_| self.error(&cells_pointer, ResourceDeclarationReason::InvalidValue))?;
        if let Some(exclude) = object.get("exclude_cells") {
            let exclude_pointer = child(pointer, "exclude_cells");
            let mut excluded = Vec::new();
            for (index, cell) in self.array(exclude, &exclude_pointer)?.iter().enumerate() {
                excluded.push(self.digest_u32(cell, &child(&exclude_pointer, &index.to_string()))?);
            }
            color_digest::validate_exclude_cells(grid, &excluded).map_err(|_| {
                self.error(&exclude_pointer, ResourceDeclarationReason::InvalidValue)
            })?;
        }
        self.digest_u32(
            self.required(object, pointer, "max_mean_milli")?,
            &child(pointer, "max_mean_milli"),
        )?;
        if let Some(max_cell) = object.get("max_cell") {
            self.digest_u32(max_cell, &child(pointer, "max_cell"))?;
        }
        Ok(())
    }

    /// The width and height of the rectangle a digest samples. A `rect` region lies entirely
    /// inside the coordinate space with a non-negative origin and a positive size; a
    /// `full_frame` region is the whole coordinate space. Template-relative regions were
    /// already refused for color probes.
    fn digest_region(
        &self,
        region: &Value,
        pointer: &str,
        frame: [u64; 2],
    ) -> CliOutcome<[u64; 2]> {
        if region.get("mode").and_then(Value::as_str) == Some("full_frame") {
            return Ok(frame);
        }
        let rect_pointer = child(pointer, "rect");
        let mut values = [0_u64; 4];
        for (slot, field) in values.iter_mut().zip(["x", "y", "width", "height"]) {
            let field_pointer = child(&rect_pointer, field);
            let value = region
                .pointer(&format!("/rect/{field}"))
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    self.error(&field_pointer, ResourceDeclarationReason::InvalidType)
                })?;
            *slot = u64::try_from(value)
                .ok()
                .filter(|value| *value > 0 || matches!(field, "x" | "y"))
                .ok_or_else(|| {
                    self.error(&field_pointer, ResourceDeclarationReason::InvalidValue)
                })?;
        }
        let [x, y, width, height] = values;
        if x.checked_add(width).is_none_or(|end| end > frame[0])
            || y.checked_add(height).is_none_or(|end| end > frame[1])
        {
            return Err(self.error(&rect_pointer, ResourceDeclarationReason::InvalidValue));
        }
        Ok([width, height])
    }

    fn digest_u32(&self, value: &Value, pointer: &str) -> CliOutcome<u32> {
        let value = value
            .as_u64()
            .ok_or_else(|| self.error(pointer, ResourceDeclarationReason::InvalidType))?;
        u32::try_from(value)
            .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))
    }

    /// The `checks` family (`contracts/selection-graph.md`, section Checks): each check names
    /// 2..=8 distinct member targets under exactly one of `all_of` and `any_of`. Whether each
    /// member exists, and is not itself a check, is checked against the derived pack.
    fn checks(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        for (index, check) in self.array(value, pointer)?.iter().enumerate() {
            let pointer = child(pointer, &index.to_string());
            let object = self.object(check, &pointer, &["id", "all_of", "any_of"])?;
            self.string(
                self.required(object, &pointer, "id")?,
                &child(&pointer, "id"),
            )?;
            let mode = match (object.contains_key("all_of"), object.contains_key("any_of")) {
                (true, false) => "all_of",
                (false, true) => "any_of",
                (true, true) => {
                    return Err(self.error(
                        &child(&pointer, "any_of"),
                        ResourceDeclarationReason::InvalidValue,
                    ));
                }
                (false, false) => {
                    return Err(self.error(
                        &child(&pointer, "all_of"),
                        ResourceDeclarationReason::MissingField,
                    ));
                }
            };
            let members_pointer = child(&pointer, mode);
            let members = self.array(&object[mode], &members_pointer)?;
            if !CHECK_MEMBERS.contains(&members.len()) {
                return Err(self.error(&members_pointer, ResourceDeclarationReason::InvalidValue));
            }
            let mut seen = BTreeSet::new();
            for (member_index, member) in members.iter().enumerate() {
                let member_pointer = child(&members_pointer, &member_index.to_string());
                let member = member.as_str().ok_or_else(|| {
                    self.error(&member_pointer, ResourceDeclarationReason::InvalidType)
                })?;
                if !seen.insert(member) {
                    return Err(
                        self.error(&member_pointer, ResourceDeclarationReason::InvalidValue)
                    );
                }
            }
        }
        Ok(())
    }

    /// The `candidate_layouts` family (`contracts/selection-graph.md`, section Candidate
    /// layouts): each layout has an ID, a page, the kind `fixed_slots`, 1..=8 distinct
    /// features and 1..=64 slots. A slot's `rect` and `click` lie inside the coordinate space
    /// and each of its `targets` keys is a declared feature. Whether the page is one the task
    /// declares, and whether each target exists in the derived pack, is checked while the pack
    /// is derived.
    fn candidate_layouts(&self, value: &Value, pointer: &str, frame: [u64; 2]) -> CliOutcome<()> {
        for (index, layout) in self.array(value, pointer)?.iter().enumerate() {
            let pointer = child(pointer, &index.to_string());
            let object = self.object(
                layout,
                &pointer,
                &["id", "page_id", "kind", "features", "slots"],
            )?;
            let id_pointer = child(&pointer, "id");
            let id = self.required(object, &pointer, "id")?;
            self.string(id, &id_pointer)?;
            if validate_candidate_layout_id(id.as_str().unwrap_or_default()).is_err() {
                return Err(self.error(&id_pointer, ResourceDeclarationReason::InvalidValue));
            }
            self.string(
                self.required(object, &pointer, "page_id")?,
                &child(&pointer, "page_id"),
            )?;
            let kind_pointer = child(&pointer, "kind");
            let kind = self.required(object, &pointer, "kind")?;
            self.string(kind, &kind_pointer)?;
            if kind.as_str() != Some("fixed_slots") {
                return Err(self.error(&kind_pointer, ResourceDeclarationReason::InvalidValue));
            }
            let features_pointer = child(&pointer, "features");
            let features = self.array(
                self.required(object, &pointer, "features")?,
                &features_pointer,
            )?;
            if !(1..=CANDIDATE_PROJECTION_MAX_FEATURES).contains(&features.len()) {
                return Err(self.error(&features_pointer, ResourceDeclarationReason::InvalidValue));
            }
            let mut names = BTreeSet::new();
            for (feature_index, feature) in features.iter().enumerate() {
                let feature_pointer = child(&features_pointer, &feature_index.to_string());
                let feature = self.object(feature, &feature_pointer, &["name", "value"])?;
                let name_pointer = child(&feature_pointer, "name");
                let name = self.required(feature, &feature_pointer, "name")?;
                self.string(name, &name_pointer)?;
                let name = name.as_str().unwrap_or_default();
                if validate_candidate_feature_name(name).is_err() || !names.insert(name) {
                    return Err(self.error(&name_pointer, ResourceDeclarationReason::InvalidValue));
                }
                let value_pointer = child(&feature_pointer, "value");
                let value = self.required(feature, &feature_pointer, "value")?;
                self.string(value, &value_pointer)?;
                if !matches!(value.as_str(), Some("passed" | "measure_milli")) {
                    return Err(self.error(&value_pointer, ResourceDeclarationReason::InvalidValue));
                }
            }
            let slots_pointer = child(&pointer, "slots");
            let slots = self.array(self.required(object, &pointer, "slots")?, &slots_pointer)?;
            if !(1..=CANDIDATE_PROJECTION_MAX_CANDIDATES).contains(&slots.len()) {
                return Err(self.error(&slots_pointer, ResourceDeclarationReason::InvalidValue));
            }
            for (slot_index, slot) in slots.iter().enumerate() {
                let slot_pointer = child(&slots_pointer, &slot_index.to_string());
                let slot = self.object(slot, &slot_pointer, &["rect", "click", "targets"])?;
                for field in ["rect", "click"] {
                    self.frame_rect(
                        self.required(slot, &slot_pointer, field)?,
                        &child(&slot_pointer, field),
                        frame,
                    )?;
                }
                let targets_pointer = child(&slot_pointer, "targets");
                let targets = self
                    .required(slot, &slot_pointer, "targets")?
                    .as_object()
                    .ok_or_else(|| {
                        self.error(&targets_pointer, ResourceDeclarationReason::InvalidType)
                    })?;
                for (name, target) in targets {
                    let target_pointer = child(&targets_pointer, name);
                    if !names.contains(name.as_str()) {
                        return Err(
                            self.error(&target_pointer, ResourceDeclarationReason::InvalidValue)
                        );
                    }
                    self.string(target, &target_pointer)?;
                }
            }
        }
        Ok(())
    }

    /// A rectangle of integers with a non-negative origin and a positive size, each within the
    /// range of a recognition pack rectangle, that lies entirely inside the coordinate space.
    fn frame_rect(&self, value: &Value, pointer: &str, frame: [u64; 2]) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["x", "y", "width", "height"])?;
        let mut values = [0_u64; 4];
        for (slot, field) in values.iter_mut().zip(["x", "y", "width", "height"]) {
            let field_pointer = child(pointer, field);
            let value = self
                .required(object, pointer, field)?
                .as_i64()
                .ok_or_else(|| {
                    self.error(&field_pointer, ResourceDeclarationReason::InvalidType)
                })?;
            *slot = i32::try_from(value)
                .ok()
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| *value > 0 || matches!(field, "x" | "y"))
                .ok_or_else(|| {
                    self.error(&field_pointer, ResourceDeclarationReason::InvalidValue)
                })?;
        }
        let [x, y, width, height] = values;
        // Each value fits an `i32`, so neither sum overflows.
        if x + width > frame[0] || y + height > frame[1] {
            return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
        }
        Ok(())
    }

    fn color(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let values = self.array(value, pointer)?;
        if values.len() != 3 {
            return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
        }
        for (index, value) in values.iter().enumerate() {
            if value.as_u64().is_none_or(|value| value > 255) {
                return Err(self.error(
                    &child(pointer, &index.to_string()),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        }
        Ok(())
    }

    fn error(&self, pointer: &str, reason: ResourceDeclarationReason) -> CliError {
        let issue = ResourceDeclarationIssue {
            declaration_file: self.file.to_string_lossy().replace('\\', "/"),
            field_path: pointer.to_owned(),
            schema_version: self.schema.map(str::to_owned),
            reason,
        };
        CliError::new(
            actingcommand_contract::LabErrorClass::UsageValidation,
            "resource_declaration_invalid",
            format!(
                "resource declaration {} field {} is not consumable: {reason:?}",
                issue.declaration_file, issue.field_path,
            ),
            &[],
        )
        .with_details(serde_json::json!(issue))
    }

    fn object<'v>(
        &self,
        value: &'v Value,
        pointer: &str,
        fields: &[&str],
    ) -> CliOutcome<&'v Map<String, Value>> {
        let object = value
            .as_object()
            .ok_or_else(|| self.error(pointer, ResourceDeclarationReason::InvalidType))?;
        for field in object.keys() {
            if !fields.contains(&field.as_str()) {
                return Err(self.error(
                    &child(pointer, field),
                    ResourceDeclarationReason::UnknownField,
                ));
            }
        }
        Ok(object)
    }

    fn required<'v>(
        &self,
        object: &'v Map<String, Value>,
        pointer: &str,
        field: &str,
    ) -> CliOutcome<&'v Value> {
        object.get(field).ok_or_else(|| {
            self.error(
                &child(pointer, field),
                ResourceDeclarationReason::MissingField,
            )
        })
    }

    fn string(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if value.is_string() {
            Ok(())
        } else {
            Err(self.error(pointer, ResourceDeclarationReason::InvalidType))
        }
    }

    fn number(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if value.is_number() {
            Ok(())
        } else {
            Err(self.error(pointer, ResourceDeclarationReason::InvalidType))
        }
    }

    fn unsigned(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if value.as_u64().is_some() {
            Ok(())
        } else {
            Err(self.error(pointer, ResourceDeclarationReason::InvalidType))
        }
    }

    fn boolean(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if value.is_boolean() {
            Ok(())
        } else {
            Err(self.error(pointer, ResourceDeclarationReason::InvalidType))
        }
    }

    fn array<'v>(&self, value: &'v Value, pointer: &str) -> CliOutcome<&'v [Value]> {
        value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| self.error(pointer, ResourceDeclarationReason::InvalidType))
    }

    fn strings(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        for (index, item) in self.array(value, pointer)?.iter().enumerate() {
            self.string(item, &child(pointer, &index.to_string()))?;
        }
        Ok(())
    }

    fn page(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if value.is_string() {
            self.string(value, pointer)
        } else {
            self.strings(value, pointer)
        }
    }

    fn rect(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["x", "y", "width", "height"])?;
        for field in ["x", "y", "width", "height"] {
            let value = self.required(object, pointer, field)?;
            if value.as_i64().is_none() {
                return Err(self.error(
                    &child(pointer, field),
                    ResourceDeclarationReason::InvalidType,
                ));
            }
        }
        Ok(())
    }
}

fn child(pointer: &str, field: &str) -> String {
    format!("{pointer}/{}", field.replace('~', "~0").replace('/', "~1"))
}

/// Checks the execution declarations of an already verified, contained bundle.
/// Source trees have already passed the same operation checks before canonicalization.
pub fn validate_contained_declarations(bundle: &crate::LoadedBundle) -> CliOutcome<()> {
    if let Some(control) = bundle.control() {
        validate_control_declarations(Path::new("control.json"), control)?;
    }
    let path = Path::new(bundle.operation_path());
    let declaration = Declaration {
        file: path,
        schema: bundle
            .operation()
            .get("schema_version")
            .and_then(Value::as_str),
    };
    // Runtime task preparation does not consume the Lab recovery settings.
    for field in ["max_task_retries", "on_exhausted"] {
        if bundle.operation().get(field).is_some() {
            return Err(declaration.error(
                &child("", field),
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
    }
    if let Some(defaults) = bundle.operation().get("defaults") {
        for field in [
            "timeout_ms",
            "pre_delay_ms",
            "post_delay_ms",
            "pre_wait_freezes_ms",
            "post_wait_freezes_ms",
        ] {
            if defaults.get(field).is_some() {
                return Err(declaration.error(
                    &child("/defaults", field),
                    ResourceDeclarationReason::UnconsumedField,
                ));
            }
        }
    }
    if let Some(operations) = bundle
        .operation()
        .get("operations")
        .and_then(Value::as_array)
    {
        for (index, operation) in operations.iter().enumerate() {
            for field in [
                "timeout_ms",
                "pre_delay_ms",
                "pre_wait_freezes_ms",
                "post_wait_freezes_ms",
                "effect",
            ] {
                if operation.get(field).is_some() {
                    return Err(declaration.error(
                        &child(&format!("/operations/{index}"), field),
                        ResourceDeclarationReason::UnconsumedField,
                    ));
                }
            }
        }
    }
    declaration.bundle(bundle.operation(), true)?;
    let operation = Bundle {
        task_id: bundle.task_id().as_str().to_owned(),
        dir: path.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
        data: bundle.operation().clone(),
    };
    for (path, read) in super::source_file_requests(std::slice::from_ref(&operation)) {
        if matches!(read, SourceRead::Metadata) {
            continue;
        }
        let name = path.to_string_lossy().replace('\\', "/");
        let mut dependency = Declaration {
            file: Path::new(&name),
            schema: None,
        };
        let bytes = bundle
            .entry(&name)
            .ok_or_else(|| dependency.error("", ResourceDeclarationReason::InvalidValue))?;
        let limit = match read {
            SourceRead::BoundedBytes(limit) => limit,
            SourceRead::Bytes => operation
                .data
                .pointer("/post_admission_ocr/limits/max_total_bytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    declaration.error(
                        "/post_admission_ocr/limits/max_total_bytes",
                        ResourceDeclarationReason::InvalidType,
                    )
                })?,
            SourceRead::Metadata => unreachable!("metadata excluded above"),
        };
        if bytes.len() as u64 > limit {
            return Err(dependency.error("", ResourceDeclarationReason::InvalidValue));
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| dependency.error("", ResourceDeclarationReason::InvalidValue))?;
        dependency.schema = value.get("schema_version").and_then(Value::as_str);
        dependency.truth_set(&value, true)?;
    }
    Declaration {
        file: Path::new(bundle.manifest_path()),
        schema: bundle
            .manifest()
            .get("schema_version")
            .and_then(Value::as_str),
    }
    .manifest(bundle.manifest())?;
    if let (Some(path), Some(navigation)) = (bundle.navigation_path(), bundle.navigation()) {
        validate_navigation_declarations(Path::new(path), navigation)?;
    }
    Ok(())
}

/// The declaration part of navigation admission; no route planning or device access.
pub fn validate_navigation_declarations(path: &Path, navigation: &Value) -> CliOutcome<()> {
    Declaration {
        file: path,
        schema: navigation.get("schema_version").and_then(Value::as_str),
    }
    .navigation(navigation)
}

/// The same control declaration grammar used before contained execution, without IO.
pub fn validate_control_declarations(path: &Path, value: &Value) -> CliOutcome<()> {
    Declaration {
        file: path,
        schema: value.get("schema_version").and_then(Value::as_str),
    }
    .control(value)
}

pub(crate) fn validate_loaded_declarations(metadata: &crate::PackageMetadata) -> CliOutcome<()> {
    if let Some(control) = &metadata.control {
        validate_control_declarations(Path::new("control.json"), control)?;
    }
    Declaration {
        file: Path::new(&metadata.manifest_path),
        schema: metadata
            .manifest
            .get("schema_version")
            .and_then(Value::as_str),
    }
    .manifest(&metadata.manifest)?;
    // Generic module containment also accepts opaque operations; a versioned executable task
    // uses the declared operation grammar before projection or recognition can consume it.
    if metadata.operation.get("schema_version").is_some() {
        Declaration {
            file: Path::new(&metadata.operation_path),
            schema: metadata
                .operation
                .get("schema_version")
                .and_then(Value::as_str),
        }
        .bundle(&metadata.operation, true)?;
    }
    if let (Some(path), Some(navigation)) = (&metadata.navigation_path, &metadata.navigation) {
        validate_navigation_declarations(Path::new(path), navigation)?;
    }
    Ok(())
}

/// Checks only the declared resource table, without reading resource material.
pub fn validate_resource_declarations(path: &Path, resources: &Value) -> CliOutcome<()> {
    let declaration = Declaration {
        file: path,
        schema: resources.get("schema_version").and_then(Value::as_str),
    };
    let object = declaration.object(
        resources,
        "",
        &[
            "schema_version",
            "resources",
            "resource_count",
            "control_points",
        ],
    )?;
    if let Some(schema) = object.get("schema_version") {
        declaration.string(schema, "/schema_version")?;
    }
    if let Some(count) = object.get("resource_count") {
        // add_resources_json preserves this metadata and derives it for selected task subsets.
        declaration.unsigned(count, "/resource_count")?;
    }
    let resources = declaration.required(object, "", "resources")?;
    for (index, resource) in declaration
        .array(resources, "/resources")?
        .iter()
        .enumerate()
    {
        let pointer = format!("/resources/{index}");
        let resource = declaration.object(resource, &pointer, &["id", "name"])?;
        declaration.string(
            declaration.required(resource, &pointer, "id")?,
            &child(&pointer, "id"),
        )?;
        // Full and selected package output retain the resource table's localized names.
        if let Some(name) = resource.get("name") {
            let pointer = child(&pointer, "name");
            if name.is_string() {
                declaration.string(name, &pointer)?;
            } else {
                let names = name.as_object().ok_or_else(|| {
                    declaration.error(&pointer, ResourceDeclarationReason::InvalidType)
                })?;
                for (locale, name) in names {
                    declaration.string(name, &child(&pointer, locale))?;
                }
            }
        }
    }
    if let Some(points) = object.get("control_points") {
        for (index, point) in declaration
            .array(points, "/control_points")?
            .iter()
            .enumerate()
        {
            declaration.control_point(point, &format!("/control_points/{index}"))?;
        }
    }
    Ok(())
}

/// Only bounded, task-local JSON dependencies belong to the declaration gate.
pub fn declaration_file_requests(bundles: &[Bundle]) -> CliOutcome<BTreeMap<PathBuf, SourceRead>> {
    let mut requests = BTreeMap::new();
    for bundle in bundles {
        let path = bundle.task_json_path();
        let declaration = Declaration {
            file: &path,
            schema: bundle.data.get("schema_version").and_then(Value::as_str),
        };
        declaration.bundle(&bundle.data, false)?;
        for (path, read) in super::source_file_requests(std::slice::from_ref(bundle)) {
            let limit = match read {
                SourceRead::Metadata => continue,
                SourceRead::Bytes => bundle.data["post_admission_ocr"]["limits"]["max_total_bytes"]
                    .as_u64()
                    .ok_or_else(|| {
                        declaration.error(
                            "/post_admission_ocr/limits/max_total_bytes",
                            ResourceDeclarationReason::InvalidType,
                        )
                    })?,
                SourceRead::BoundedBytes(limit) => limit,
            };
            requests
                .entry(path)
                .and_modify(|read| {
                    if let SourceRead::BoundedBytes(previous) = read {
                        *previous = (*previous).min(limit);
                    }
                })
                .or_insert(SourceRead::BoundedBytes(limit));
        }
    }
    Ok(requests)
}

/// Declaration validation never reads template metadata, images or models.
pub fn validate_bundle_declarations(bundle: &Bundle, files: &ParseFiles) -> CliOutcome<()> {
    let requests = declaration_file_requests(std::slice::from_ref(bundle))?;
    for (path, read) in requests {
        let mut declaration = Declaration {
            file: &path,
            schema: None,
        };
        let bytes = files
            .read(&path)
            .map_err(|_| declaration.error("", ResourceDeclarationReason::InvalidValue))?;
        if let SourceRead::BoundedBytes(limit) = read
            && bytes.len() as u64 > limit
        {
            return Err(declaration.error("", ResourceDeclarationReason::InvalidValue));
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| declaration.error("", ResourceDeclarationReason::InvalidValue))?;
        declaration.schema = value.get("schema_version").and_then(Value::as_str);
        declaration.truth_set(
            &value,
            bundle.data["schema_version"].as_str() == Some("0.8"),
        )?;
    }
    Ok(())
}

impl Declaration<'_> {
    fn scheduling(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["designated_operation", "mappings"])?;
        if let Some(operation) = object
            .get("designated_operation")
            .filter(|value| !value.is_null())
        {
            self.string(operation, &child(pointer, "designated_operation"))?;
        }
        let mappings = self.required(object, pointer, "mappings")?;
        for (index, mapping) in self
            .array(mappings, &child(pointer, "mappings"))?
            .iter()
            .enumerate()
        {
            let pointer = child(&child(pointer, "mappings"), &index.to_string());
            let mapping = self.object(
                mapping,
                &pointer,
                &["outcome_key", "effect", "terminal_pages"],
            )?;
            for field in ["outcome_key", "effect"] {
                self.string(
                    self.required(mapping, &pointer, field)?,
                    &child(&pointer, field),
                )?;
            }
            self.strings(
                self.required(mapping, &pointer, "terminal_pages")?,
                &child(&pointer, "terminal_pages"),
            )?;
        }
        let declaration: actingcommand_contract::SchedulingOutcomeDeclaration =
            serde_json::from_value(value.clone())
                .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))?;
        declaration
            .validate()
            .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))?;
        Ok(())
    }

    fn stability(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &[
                "region",
                "comparison",
                "consecutive_unchanged_threshold",
                "max_steps",
            ],
        )?;
        self.rect(
            self.required(object, pointer, "region")?,
            &child(pointer, "region"),
        )?;
        for field in ["consecutive_unchanged_threshold", "max_steps"] {
            self.unsigned(
                self.required(object, pointer, field)?,
                &child(pointer, field),
            )?;
        }
        let comparison = self.object(
            self.required(object, pointer, "comparison")?,
            &child(pointer, "comparison"),
            &["mode", "parameters"],
        )?;
        self.string(
            self.required(comparison, &child(pointer, "comparison"), "mode")?,
            &child(&child(pointer, "comparison"), "mode"),
        )?;
        if comparison["mode"].as_str() != Some("exact_pixels_v1") {
            return Err(self.error(
                &child(&child(pointer, "comparison"), "mode"),
                ResourceDeclarationReason::InvalidValue,
            ));
        }
        self.object(
            self.required(comparison, &child(pointer, "comparison"), "parameters")?,
            &child(&child(pointer, "comparison"), "parameters"),
            &[],
        )?;
        Ok(())
    }

    fn post_ocr(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let fields_mode = self.schema == Some("0.8");
        let fields: &[&str] = if fields_mode {
            &["mode", "page_ids", "fields", "limits", "outcome_key"]
        } else {
            &[
                "page_id",
                "page_ids",
                "target_id",
                "target_ids",
                "truth_set",
                "normalization",
                "comparison",
                "limits",
                "outcome_key",
            ]
        };
        let object = self.object(value, pointer, fields)?;
        self.string(
            self.required(object, pointer, "outcome_key")?,
            &child(pointer, "outcome_key"),
        )?;
        let limits_pointer = child(pointer, "limits");
        let limit_fields = [
            "max_frames",
            "max_items",
            "max_string_bytes",
            "max_total_bytes",
            "max_truth_entries",
        ];
        let limits = self.object(
            self.required(object, pointer, "limits")?,
            &limits_pointer,
            &limit_fields,
        )?;
        for field in limit_fields {
            self.unsigned(
                self.required(limits, &limits_pointer, field)?,
                &child(&limits_pointer, field),
            )?;
        }
        // These are the existing OCR declaration limits, before requesting JSON bytes.
        for (field, maximum) in [
            ("max_frames", 256),
            ("max_items", 4096),
            ("max_string_bytes", 4096),
            ("max_total_bytes", 4 * 1024 * 1024),
            ("max_truth_entries", 4096),
        ] {
            if limits[field]
                .as_u64()
                .is_none_or(|limit| limit == 0 || limit > maximum)
            {
                return Err(self.error(
                    &child(&limits_pointer, field),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        }
        if fields_mode {
            self.string(
                self.required(object, pointer, "mode")?,
                &child(pointer, "mode"),
            )?;
            self.strings(
                self.required(object, pointer, "page_ids")?,
                &child(pointer, "page_ids"),
            )?;
            let fields_pointer = child(pointer, "fields");
            let fields = self.required(object, pointer, "fields")?;
            for (index, field) in self.array(fields, &fields_pointer)?.iter().enumerate() {
                self.ocr_field(field, &child(&fields_pointer, &index.to_string()))?;
            }
            let fields: actingcommand_contract::OcrFieldsDeclaration =
                serde_json::from_value(value.clone())
                    .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))?;
            fields
                .validate()
                .map_err(|_| self.error(pointer, ResourceDeclarationReason::InvalidValue))?;
        } else {
            for (field, expected) in [
                ("normalization", "trim_lowercase_v1"),
                ("comparison", "exact_set_v1"),
            ] {
                self.string(
                    self.required(object, pointer, field)?,
                    &child(pointer, field),
                )?;
                if object[field].as_str() != Some(expected) {
                    return Err(self.error(
                        &child(pointer, field),
                        ResourceDeclarationReason::InvalidValue,
                    ));
                }
            }
            for field in ["page_id", "target_id"] {
                if let Some(value) = object.get(field) {
                    self.string(value, &child(pointer, field))?;
                }
            }
            for field in ["page_ids", "target_ids"] {
                if let Some(value) = object.get(field) {
                    self.strings(value, &child(pointer, field))?;
                }
            }
            self.json_reference(
                self.required(object, pointer, "truth_set")?,
                &child(pointer, "truth_set"),
            )?;
            super::post_admission_ocr_page_ids(object).map_err(|_| {
                self.error(
                    &child(pointer, "page_ids"),
                    ResourceDeclarationReason::InvalidValue,
                )
            })?;
            super::post_admission_ocr_target_ids(object).map_err(|_| {
                self.error(
                    &child(pointer, "target_ids"),
                    ResourceDeclarationReason::InvalidValue,
                )
            })?;
        }
        Ok(())
    }

    /// Operation `resource_readings` (`contracts/resource-readings.md`). Structure is checked
    /// field by field; the value rules are the shared `ResourceReadingDeclaration::validate`,
    /// whose reason names the field reported here. Page and target cross-references belong
    /// to the parser's package validation.
    fn resource_readings(
        &self,
        value: &Value,
        pointer: &str,
        task: &Map<String, Value>,
    ) -> CliOutcome<()> {
        let readings = self.array(value, pointer)?;
        if readings.is_empty() || readings.len() > actingcommand_contract::MAX_RESOURCE_READINGS {
            return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
        }
        if task.get("scheduling_outcome").is_none_or(Value::is_null) {
            return Err(self.error(
                "/scheduling_outcome",
                ResourceDeclarationReason::MissingField,
            ));
        }
        let fields = [
            "id",
            "fact_key",
            "page_id",
            "target_id",
            "trim",
            "value",
            "minimum_confidence_milli",
            "valid_for_ms",
        ];
        let mut ids = BTreeSet::new();
        let mut fact_keys = BTreeSet::new();
        for (index, reading) in readings.iter().enumerate() {
            let pointer = child(pointer, &index.to_string());
            let object = self.object(reading, &pointer, &fields)?;
            for field in fields {
                self.required(object, &pointer, field)?;
            }
            for field in ["id", "fact_key", "page_id", "target_id", "trim"] {
                self.string(&object[field], &child(&pointer, field))?;
            }
            if serde_json::from_value::<actingcommand_contract::OcrFieldTrim>(
                object["trim"].clone(),
            )
            .is_err()
            {
                return Err(self.error(
                    &child(&pointer, "trim"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            let value_pointer = child(&pointer, "value");
            let value_type = self.object(
                &object["value"],
                &value_pointer,
                &["type", "min", "max", "format"],
            )?;
            let kind = self.required(value_type, &value_pointer, "type")?;
            self.string(kind, &child(&value_pointer, "type"))?;
            if kind.as_str() != Some("unsigned_integer") {
                return Err(self.error(
                    &child(&value_pointer, "type"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            for field in ["min", "max"] {
                self.unsigned(
                    self.required(value_type, &value_pointer, field)?,
                    &child(&value_pointer, field),
                )?;
            }
            if let Some(format) = value_type.get("format") {
                let format_pointer = child(&value_pointer, "format");
                self.string(format, &format_pointer)?;
                if serde_json::from_value::<actingcommand_contract::OcrUnsignedIntegerFormat>(
                    format.clone(),
                )
                .is_err()
                {
                    return Err(
                        self.error(&format_pointer, ResourceDeclarationReason::InvalidValue)
                    );
                }
            }
            for field in ["minimum_confidence_milli", "valid_for_ms"] {
                self.unsigned(&object[field], &child(&pointer, field))?;
            }
            if object["minimum_confidence_milli"]
                .as_u64()
                .is_none_or(|value| u16::try_from(value).is_err())
            {
                return Err(self.error(
                    &child(&pointer, "minimum_confidence_milli"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            let declaration: actingcommand_contract::ResourceReadingDeclaration =
                serde_json::from_value(reading.clone())
                    .map_err(|_| self.error(&pointer, ResourceDeclarationReason::InvalidValue))?;
            if let Err(reason) = declaration.validate() {
                let path: &[&str] = match reason {
                    "resource_reading_id_invalid" => &["id"],
                    "resource_reading_fact_key_invalid" => &["fact_key"],
                    "resource_reading_page_id_invalid" => &["page_id"],
                    "resource_reading_target_id_invalid" => &["target_id"],
                    "resource_reading_value_max_invalid" => &["value", "max"],
                    "resource_reading_value_min_invalid" => &["value", "min"],
                    "resource_reading_minimum_confidence_invalid" => &["minimum_confidence_milli"],
                    "resource_reading_valid_for_invalid" => &["valid_for_ms"],
                    _ => &[],
                };
                let pointer = path
                    .iter()
                    .fold(pointer.clone(), |pointer, field| child(&pointer, field));
                return Err(self.error(&pointer, ResourceDeclarationReason::InvalidValue));
            }
            if !ids.insert(declaration.id) {
                return Err(self.error(
                    &child(&pointer, "id"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            if !fact_keys.insert(declaration.fact_key) {
                return Err(self.error(
                    &child(&pointer, "fact_key"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        }
        Ok(())
    }

    fn json_reference(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["path", "sha256"])?;
        for field in ["path", "sha256"] {
            self.string(
                self.required(object, pointer, field)?,
                &child(pointer, field),
            )?;
        }
        if !object["path"]
            .as_str()
            .is_some_and(super::safe_task_local_resource_path)
        {
            return Err(self.error(
                &child(pointer, "path"),
                ResourceDeclarationReason::InvalidValue,
            ));
        }
        if !object["sha256"].as_str().is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(self.error(
                &child(pointer, "sha256"),
                ResourceDeclarationReason::InvalidValue,
            ));
        }
        Ok(())
    }

    fn ocr_field(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &[
                "id",
                "group",
                "target_id",
                "required",
                "privacy",
                "trim",
                "value",
                "text_extraction",
            ],
        )?;
        for field in ["id", "group", "target_id", "privacy", "trim"] {
            self.string(
                self.required(object, pointer, field)?,
                &child(pointer, field),
            )?;
        }
        self.boolean(
            self.required(object, pointer, "required")?,
            &child(pointer, "required"),
        )?;
        let field_type = self.required(object, pointer, "value")?;
        let value_pointer = child(pointer, "value");
        match field_type.get("type").and_then(Value::as_str) {
            Some("unsigned_integer") => {
                let object = self.object(
                    field_type,
                    &value_pointer,
                    &["type", "min", "max", "format"],
                )?;
                for field in ["min", "max"] {
                    self.unsigned(
                        self.required(object, &value_pointer, field)?,
                        &child(&value_pointer, field),
                    )?;
                }
                if let Some(format) = object.get("format") {
                    self.string(format, &child(&value_pointer, "format"))?;
                }
            }
            Some("dictionary_entry") => {
                let object = self.object(field_type, &value_pointer, &["type", "dictionary"])?;
                self.json_reference(
                    self.required(object, &value_pointer, "dictionary")?,
                    &child(&value_pointer, "dictionary"),
                )?;
            }
            _ => {
                return Err(self.error(
                    &child(&value_pointer, "type"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        }
        if let Some(extraction) = object
            .get("text_extraction")
            .filter(|value| !value.is_null())
        {
            let pointer = child(pointer, "text_extraction");
            let object = self.object(extraction, &pointer, &["mode", "suffix"])?;
            self.string(
                self.required(object, &pointer, "mode")?,
                &child(&pointer, "mode"),
            )?;
            let suffix = self.required(object, &pointer, "suffix")?;
            for (index, segment) in self
                .array(suffix, &child(&pointer, "suffix"))?
                .iter()
                .enumerate()
            {
                let pointer = child(&child(&pointer, "suffix"), &index.to_string());
                match segment.get("type").and_then(Value::as_str) {
                    Some("ascii_digits") => {
                        let object = self.object(segment, &pointer, &["type", "count"])?;
                        self.unsigned(
                            self.required(object, &pointer, "count")?,
                            &child(&pointer, "count"),
                        )?;
                    }
                    Some("literal") => {
                        let object = self.object(segment, &pointer, &["type", "value"])?;
                        self.string(
                            self.required(object, &pointer, "value")?,
                            &child(&pointer, "value"),
                        )?;
                    }
                    _ => {
                        return Err(self.error(
                            &child(&pointer, "type"),
                            ResourceDeclarationReason::InvalidValue,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn bundle(&self, value: &Value, canonical: bool) -> CliOutcome<()> {
        let object = self.object(
            value,
            "",
            &[
                "schema_version",
                "task_id",
                "game",
                "server_scope",
                "locale",
                "goal",
                "provenance",
                "coordinate_space",
                "defaults",
                "entry_page",
                "target_page",
                "error_pages",
                "phases",
                "timeout_ms",
                "max_steps",
                "scheduling_outcome",
                "post_admission_ocr",
                "resource_readings",
                "stability_termination",
                "recovery",
                "max_task_retries",
                "on_exhausted",
                "operations",
                "anchors",
                "verify_templates",
                "color_probes",
                "ocr_targets",
                "checks",
                "candidate_layouts",
                "page_rules",
            ],
        )?;
        for field in ["schema_version", "task_id", "game"] {
            self.string(self.required(object, "", field)?, &child("", field))?;
        }
        if !matches!(
            self.schema,
            Some("0.3" | "0.4" | "0.5" | "0.6" | "0.7" | "0.8" | "0.9")
        ) {
            return Err(self.error("/schema_version", ResourceDeclarationReason::InvalidValue));
        }
        match self.schema {
            Some("0.7" | "0.8") => {
                self.required(object, "", "post_admission_ocr")?;
            }
            _ if object.contains_key("post_admission_ocr") => {
                return Err(self.error(
                    "/post_admission_ocr",
                    ResourceDeclarationReason::UnconsumedField,
                ));
            }
            _ => {}
        }
        for field in ["timeout_ms", "max_steps"] {
            if object.contains_key(field) && !matches!(self.schema, Some("0.7" | "0.8" | "0.9")) {
                return Err(self.error(
                    &child("", field),
                    ResourceDeclarationReason::UnconsumedField,
                ));
            }
        }
        if object.contains_key("phases") && self.schema != Some("0.9") {
            return Err(self.error("/phases", ResourceDeclarationReason::UnconsumedField));
        }
        if object.contains_key("ocr_targets")
            && !matches!(self.schema, Some("0.6" | "0.7" | "0.8" | "0.9"))
        {
            return Err(self.error("/ocr_targets", ResourceDeclarationReason::UnconsumedField));
        }
        if object.contains_key("checks") && !self.schema_0_6_or_later() {
            return Err(self.error("/checks", ResourceDeclarationReason::UnconsumedField));
        }
        if object.contains_key("candidate_layouts") && !self.schema_0_6_or_later() {
            return Err(self.error(
                "/candidate_layouts",
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
        if object.contains_key("resource_readings") && !matches!(self.schema, Some("0.8" | "0.9")) {
            return Err(self.error(
                "/resource_readings",
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
        if !canonical || object.contains_key("server_scope") {
            self.strings(self.required(object, "", "server_scope")?, "/server_scope")?;
        }
        let space = self.required(object, "", "coordinate_space")?;
        let space = self.object(space, "/coordinate_space", &["width", "height"])?;
        let mut frame = [0_u64; 2];
        for (bound, field) in frame.iter_mut().zip(["width", "height"]) {
            let pointer = child("/coordinate_space", field);
            *bound = self
                .required(space, "/coordinate_space", field)?
                .as_u64()
                .ok_or_else(|| self.error(&pointer, ResourceDeclarationReason::InvalidType))?;
        }
        for (field, value) in object {
            let pointer = child("", field);
            match field.as_str() {
                "locale" | "goal" | "entry_page" if !value.is_null() => {
                    self.string(value, &pointer)?
                }
                "on_exhausted" if !value.is_null() => self.string(value, &pointer)?,
                "max_task_retries" if !value.is_null() => self.unsigned(value, &pointer)?,
                "provenance" if !value.is_null() && !value.is_object() => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::InvalidType));
                }
                "target_page" if !value.is_null() => self.page(value, &pointer)?,
                "error_pages" => self.strings(value, &pointer)?,
                "timeout_ms" | "max_steps" => self.unsigned(value, &pointer)?,
                "defaults" => self.defaults(value, &pointer)?,
                "phases" => self.phases(value, &pointer)?,
                "scheduling_outcome" if !value.is_null() => self.scheduling(value, &pointer)?,
                "post_admission_ocr" => self.post_ocr(value, &pointer)?,
                "resource_readings" => self.resource_readings(value, &pointer, object)?,
                "stability_termination" => self.stability(value, &pointer)?,
                "recovery" if !value.is_null() => self.recovery(value, &pointer)?,
                "operations" => {
                    for (index, operation) in self.array(value, &pointer)?.iter().enumerate() {
                        self.operation(operation, &child(&pointer, &index.to_string()), canonical)?;
                    }
                }
                "anchors" | "verify_templates" | "color_probes" | "ocr_targets" => {
                    for (index, target) in self.array(value, &pointer)?.iter().enumerate() {
                        self.target(
                            target,
                            &child(&pointer, &index.to_string()),
                            field,
                            canonical,
                            frame,
                        )?;
                    }
                }
                "checks" => self.checks(value, &pointer)?,
                "candidate_layouts" => self.candidate_layouts(value, &pointer, frame)?,
                "page_rules" => {
                    let rules = value.as_object().ok_or_else(|| {
                        self.error(&pointer, ResourceDeclarationReason::InvalidType)
                    })?;
                    for (page, rule) in rules {
                        self.page_rule(rule, &child(&pointer, page))?;
                    }
                }
                _ => {}
            }
        }
        self.array(self.required(object, "", "operations")?, "/operations")?;
        Ok(())
    }

    fn defaults(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &[
                "template_threshold",
                "color_max_distance",
                "match_metric",
                "max_attempts",
                "retry_interval_ms",
                "timeout_ms",
                "pre_delay_ms",
                "post_delay_ms",
                "pre_wait_freezes_ms",
                "post_wait_freezes_ms",
            ],
        )?;
        for (field, value) in object {
            let pointer = child(pointer, field);
            match field.as_str() {
                "template_threshold" | "color_max_distance" => self.number(value, &pointer)?,
                "match_metric" => {
                    self.string(value, &pointer)?;
                    if !matches!(value.as_str(), Some("ccorr_normed" | "ccoeff_normed")) {
                        return Err(self.error(&pointer, ResourceDeclarationReason::InvalidValue));
                    }
                }
                _ if value.is_null() => {}
                _ => self.unsigned(value, &pointer)?,
            }
        }
        Ok(())
    }

    fn phases(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        for (index, phase) in self.array(value, pointer)?.iter().enumerate() {
            let pointer = child(pointer, &index.to_string());
            let phase = self.object(phase, &pointer, &["id", "operations", "target_pages"])?;
            self.string(
                self.required(phase, &pointer, "id")?,
                &child(&pointer, "id"),
            )?;
            for field in ["operations", "target_pages"] {
                self.strings(
                    self.required(phase, &pointer, field)?,
                    &child(&pointer, field),
                )?;
            }
        }
        Ok(())
    }

    fn recovery(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if value.is_string() {
            return Ok(());
        }
        let object = self.object(value, pointer, &["kind", "task_id"])?;
        self.string(
            self.required(object, pointer, "kind")?,
            &child(pointer, "kind"),
        )?;
        if let Some(task) = object.get("task_id").filter(|value| !value.is_null()) {
            self.string(task, &child(pointer, "task_id"))?;
        }
        Ok(())
    }

    fn page_rule(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &["required", "optional", "forbidden", "any_of"],
        )?;
        for (field, value) in object {
            let pointer = child(pointer, field);
            if field == "any_of" {
                for (index, group) in self.array(value, &pointer)?.iter().enumerate() {
                    self.strings(group, &child(&pointer, &index.to_string()))?;
                }
            } else {
                self.strings(value, &pointer)?;
            }
        }
        Ok(())
    }

    fn control_point(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &["name", "point", "x", "y", "click", "note", "purpose"],
        )?;
        self.string(
            self.required(object, pointer, "name")?,
            &child(pointer, "name"),
        )?;
        if !object.contains_key("click") && !object.contains_key("point") {
            for field in ["x", "y"] {
                self.required(object, pointer, field)?;
            }
        }
        for (field, value) in object {
            let pointer = child(pointer, field);
            match field.as_str() {
                "name" | "note" | "purpose" => self.string(value, &pointer)?,
                "x" | "y" => {
                    self.navigation_coordinate(value, &pointer)?;
                }
                "point" => self.point(value, &pointer)?,
                "click" => self.navigation_click(value, &pointer)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn truth_set(&self, value: &Value, nullable_aliases: bool) -> CliOutcome<()> {
        let object = self.object(value, "", &["schema_version", "items", "aliases"])?;
        self.string(
            self.required(object, "", "schema_version")?,
            "/schema_version",
        )?;
        match self.schema {
            Some("actingcommand.ocr-truth-set.v1") => {
                if object
                    .get("aliases")
                    .is_some_and(|aliases| !nullable_aliases || !aliases.is_null())
                {
                    return Err(self.error("/aliases", ResourceDeclarationReason::UnconsumedField));
                }
            }
            Some("actingcommand.ocr-truth-set.v2") => {}
            _ => return Err(self.error("/schema_version", ResourceDeclarationReason::InvalidValue)),
        }
        self.strings(self.required(object, "", "items")?, "/items")?;
        if let Some(aliases) = object
            .get("aliases")
            .filter(|value| !nullable_aliases || !value.is_null())
        {
            for (index, alias) in self.array(aliases, "/aliases")?.iter().enumerate() {
                let pointer = format!("/aliases/{index}");
                let alias = self.object(alias, &pointer, &["observed", "canonical"])?;
                for field in ["observed", "canonical"] {
                    self.string(
                        self.required(alias, &pointer, field)?,
                        &child(&pointer, field),
                    )?;
                }
            }
        }
        Ok(())
    }
}
