// SPDX-License-Identifier: AGPL-3.0-only

use super::{Bundle, CliError, CliOutcome, ConversionFiles, SourceRead};
use actingcommand_contract::{ResourceDeclarationIssue, ResourceDeclarationReason};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Declaration<'a> {
    file: &'a Path,
    schema: Option<&'a str>,
}

impl Declaration<'_> {
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

    fn point(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if let Some(point) = value.as_str() {
            let parts: Vec<_> = point.split(',').collect();
            if parts.len() != 2 || parts.iter().any(|part| part.trim().parse::<i32>().is_err()) {
                return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
            }
        } else if let Some(point) = value.as_array() {
            if point.len() != 2 || point.iter().any(|value| value.as_i64().is_none()) {
                return Err(self.error(pointer, ResourceDeclarationReason::InvalidType));
            }
        } else {
            let object = self.object(value, pointer, &["x", "y"])?;
            for field in ["x", "y"] {
                if self.required(object, pointer, field)?.as_i64().is_none() {
                    return Err(self.error(
                        &child(pointer, field),
                        ResourceDeclarationReason::InvalidType,
                    ));
                }
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
        for (field, value) in object {
            let pointer = child(pointer, field);
            match field.as_str() {
                "kind" | "target_id" => self.string(value, &pointer)?,
                "point" => self.point(value, &pointer)?,
                "from" | "to" => self.navigation_click(value, &pointer)?,
                "duration_ms" => self.unsigned(value, &pointer)?,
                "x" | "y" | "width" | "height" if value.as_i64().is_none() => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::InvalidType));
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
        self.click(
            self.required(object, pointer, "click")?,
            &child(pointer, "click"),
            canonical,
        )?;
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
            ],
        )?;
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
        for field in ["verify_template", "color_probe"] {
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
            "color_probes" => &["id", "region", "expected", "provenance"],
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
                "click" if !value.is_null() => self.rect(value, &pointer)?,
                "rect_move" => self.rect_move(value, &pointer)?,
                "method" => self.method(value, &pointer)?,
                "mask" | "maskRange" => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::UnconsumedField));
                }
                "color_check" if !value.is_null() => {
                    let check = self.object(value, &pointer, &["region", "expected"])?;
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
            self.required(object, pointer, "expected")?;
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
pub fn validate_bundle_declarations(bundle: &Bundle, files: &ConversionFiles) -> CliOutcome<()> {
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
                "stability_termination",
                "recovery",
                "max_task_retries",
                "on_exhausted",
                "operations",
                "anchors",
                "verify_templates",
                "color_probes",
                "ocr_targets",
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
        if !canonical || object.contains_key("server_scope") {
            self.strings(self.required(object, "", "server_scope")?, "/server_scope")?;
        }
        let space = self.required(object, "", "coordinate_space")?;
        let space = self.object(space, "/coordinate_space", &["width", "height"])?;
        for field in ["width", "height"] {
            self.unsigned(
                self.required(space, "/coordinate_space", field)?,
                &child("/coordinate_space", field),
            )?;
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
                        )?;
                    }
                }
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
        for (field, value) in object {
            let pointer = child(pointer, field);
            match field.as_str() {
                "name" | "note" | "purpose" => self.string(value, &pointer)?,
                "x" | "y" if value.as_i64().is_none() => {
                    return Err(self.error(&pointer, ResourceDeclarationReason::InvalidType));
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
