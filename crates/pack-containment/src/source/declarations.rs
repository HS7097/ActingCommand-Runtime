// SPDX-License-Identifier: AGPL-3.0-only

use super::{Bundle, CliError, CliOutcome, ParseFiles, SourceRead};
use actingcommand_contract::candidate_projection::{
    CANDIDATE_PROJECTION_MAX_CANDIDATES, CANDIDATE_PROJECTION_MAX_FEATURES,
    validate_candidate_feature_name, validate_candidate_layout_id,
};
use actingcommand_contract::codes::{SharedCode, TaskflowCode};
use actingcommand_contract::{ResourceDeclarationIssue, ResourceDeclarationReason};
use actingcommand_recognition::color_digest::{
    self, ColorDigest, ColorDigestAlgorithm, ColorDigestGrid, MAX_GRID_AXIS,
};
use actingcommand_selection_policy::{SelectionPolicy, ValueType};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Members of one `checks` entry (`contracts/selection-graph.md`, section Checks).
const CHECK_MEMBERS: std::ops::RangeInclusive<usize> = 2..=8;
/// Workflow #390 EXEC-2: an operation's `observation_window`, by its `until`, with its required
/// unsigned members.
const OBSERVATION_WINDOW_MEMBERS: &[(&str, &[&str])] = &[
    ("window_end", &["window_ms"]),
    ("stable", &["window_ms", "frames"]),
];
/// Workflow #390 EXEC-2: an operation's `input_recheck`, by its `kind`, with its required unsigned
/// members.
const INPUT_RECHECK_MEMBERS: &[(&str, &[&str])] =
    &[("fresh_frame", &[]), ("stable_frames", &["frames"])];
/// Workflow #390 EXEC-1: an operation's `when` names its target under exactly one of these.
const WHEN_MEMBERS: &[&str] = &["present", "absent"];
/// Workflow #390 EXEC-1: the one value of an operation's `on_guard_refused`.
const GUARD_REFUSAL_NEXT_CANDIDATE: &str = "next_candidate";

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
                "prerequisite_package_id",
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
                "schema_version"
                | "package_id"
                | "execution_mode"
                | "game"
                | "server"
                | "entry_task_id"
                | "prerequisite_package_id" => self.string(value, &pointer)?,
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
                            &["id", "from_page", "to_page", "to_pages", "click", "source"]
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
                            for name in ["id", "from_page"] {
                                self.string(
                                    self.required(entry, &pointer, name)?,
                                    &child(&pointer, name),
                                )?;
                            }
                            // Workflow #390 LAB-2: an edge has one destination (`to_page`) or,
                            // for an operation whose `to` is a page set, every page it may
                            // reach (`to_pages`, at least one), never both.
                            match (entry.get("to_page"), entry.get("to_pages")) {
                                (Some(_), Some(_)) => {
                                    return Err(self.error(
                                        &child(&pointer, "to_pages"),
                                        ResourceDeclarationReason::UnknownField,
                                    ));
                                }
                                (None, Some(pages)) => {
                                    let pages_pointer = child(&pointer, "to_pages");
                                    let pages = self.array(pages, &pages_pointer)?;
                                    if pages.is_empty() {
                                        return Err(self.error(
                                            &pages_pointer,
                                            ResourceDeclarationReason::InvalidValue,
                                        ));
                                    }
                                    for (index, page) in pages.iter().enumerate() {
                                        self.string(
                                            page,
                                            &child(&pages_pointer, &index.to_string()),
                                        )?;
                                    }
                                }
                                _ => {
                                    self.string(
                                        self.required(entry, &pointer, "to_page")?,
                                        &child(&pointer, "to_page"),
                                    )?;
                                }
                            }
                        }
                        if field == "navigation" || field == "destructive_actions" {
                            self.required(entry, &pointer, "click")?;
                        }
                        for (field, value) in entry {
                            let pointer = child(&pointer, field);
                            match field.as_str() {
                                "click" => self.navigation_click(value, &pointer)?,
                                // Workflow #390 LAB-2: checked above.
                                "to_pages" => {}
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
                "select",
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
                "transition",
                "optional",
                "observation_window",
                "input_recheck",
                "when",
                "on_guard_refused",
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
        // Exactly one effect: `click`, the `application` effect of slice #316-B3, or a select
        // step (Workflow #308; task schema `0.6` and later).
        if object.contains_key("select") && !self.schema_0_6_or_later() {
            return Err(self.error(
                &child(pointer, "select"),
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
        match (
            object.get("click").filter(|value| !value.is_null()),
            object.get("application").filter(|value| !value.is_null()),
            object.get("select").filter(|value| !value.is_null()),
        ) {
            (Some(click), None, None) => self.click(click, &child(pointer, "click"), canonical)?,
            (None, Some(application), None) => {
                self.application(application, &child(pointer, "application"))?;
            }
            (None, None, Some(select)) => self.select_step(object, select, pointer)?,
            (Some(_), Some(_), None) => {
                return Err(self.error(
                    &child(pointer, "application"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            (_, _, Some(_)) => {
                return Err(self.error(
                    &child(pointer, "select"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            (None, None, None) => {
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
                "transition" if !value.is_null() => self.transition(value, &pointer)?,
                "optional" if !value.is_null() => self.optional(value, &pointer)?,
                "observation_window" if !value.is_null() => {
                    self.tagged(value, &pointer, "until", OBSERVATION_WINDOW_MEMBERS)?;
                }
                "input_recheck" if !value.is_null() => {
                    self.tagged(value, &pointer, "kind", INPUT_RECHECK_MEMBERS)?;
                }
                "when" if !value.is_null() => self.when(value, &pointer)?,
                "on_guard_refused" if !value.is_null() => {
                    self.on_guard_refused(value, &pointer)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// An optional operation (Workflow #339, `linear_steps` only): `{"settle_ms"}`, nothing
    /// else. Runtime admission checks the value.
    fn optional(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["settle_ms"])?;
        self.unsigned(
            self.required(object, pointer, "settle_ms")?,
            &child(pointer, "settle_ms"),
        )
    }

    /// Workflow #390 EXEC-2: an operation's `observation_window` or `input_recheck`, task schema
    /// `0.9` only (`UnconsumedField` before it): an object whose `tag` names one of `variants`,
    /// with that variant's unsigned members, all required, and nothing else. Runtime admission
    /// checks the values.
    fn tagged(
        &self,
        value: &Value,
        pointer: &str,
        tag: &str,
        variants: &[(&str, &[&str])],
    ) -> CliOutcome<()> {
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let object = value
            .as_object()
            .ok_or_else(|| self.error(pointer, ResourceDeclarationReason::InvalidType))?;
        let tag_pointer = child(pointer, tag);
        let name = self
            .required(object, pointer, tag)?
            .as_str()
            .ok_or_else(|| self.error(&tag_pointer, ResourceDeclarationReason::InvalidType))?;
        let members = variants
            .iter()
            .find(|(variant, _)| *variant == name)
            .map(|(_, members)| *members)
            .ok_or_else(|| self.error(&tag_pointer, ResourceDeclarationReason::InvalidValue))?;
        let allowed = std::iter::once(tag)
            .chain(members.iter().copied())
            .collect::<Vec<_>>();
        let object = self.object(value, pointer, &allowed)?;
        for member in members {
            self.unsigned(
                self.required(object, pointer, member)?,
                &child(pointer, member),
            )?;
        }
        Ok(())
    }

    /// Workflow #390 EXEC-1: an operation's `when`, task schema `0.9` only (`UnconsumedField`
    /// before it): `{"present": "<target>"}` or `{"absent": "<target>"}`, one member naming a
    /// target, nothing else. Runtime admission checks the target.
    fn when(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let object = self.object(value, pointer, WHEN_MEMBERS)?;
        let mut declared = WHEN_MEMBERS
            .iter()
            .filter_map(|member| object.get(*member).map(|target| (*member, target)));
        match (declared.next(), declared.next()) {
            (Some((member, target)), None) => self.string(target, &child(pointer, member)),
            (None, _) => Err(self.error(pointer, ResourceDeclarationReason::MissingField)),
            (Some(_), Some(_)) => Err(self.error(pointer, ResourceDeclarationReason::InvalidValue)),
        }
    }

    /// Workflow #390 EXEC-1: an operation's `on_guard_refused`, task schema `0.9` only
    /// (`UnconsumedField` before it): the string `next_candidate`.
    fn on_guard_refused(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        match value.as_str() {
            Some(GUARD_REFUSAL_NEXT_CANDIDATE) => Ok(()),
            Some(_) => Err(self.error(pointer, ResourceDeclarationReason::InvalidValue)),
            None => Err(self.error(pointer, ResourceDeclarationReason::InvalidType)),
        }
    }

    /// An operation's intermediate state (Workflow #336, `linear_steps` only):
    /// `{"kind":"page","page_id","timeout_ms"?,"interval_ms"?}` or
    /// `{"kind":"window","min_ms","max_ms"}`. Runtime admission checks the values.
    fn transition(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = value
            .as_object()
            .ok_or_else(|| self.error(pointer, ResourceDeclarationReason::InvalidType))?;
        let kind_pointer = child(pointer, "kind");
        match self.required(object, pointer, "kind")?.as_str() {
            Some("page") => {
                let object = self.object(
                    value,
                    pointer,
                    &["kind", "page_id", "timeout_ms", "interval_ms"],
                )?;
                self.string(
                    self.required(object, pointer, "page_id")?,
                    &child(pointer, "page_id"),
                )?;
                for field in ["timeout_ms", "interval_ms"] {
                    if let Some(value) = object.get(field).filter(|value| !value.is_null()) {
                        self.unsigned(value, &child(pointer, field))?;
                    }
                }
            }
            Some("window") => {
                let object = self.object(value, pointer, &["kind", "min_ms", "max_ms"])?;
                for field in ["min_ms", "max_ms"] {
                    self.unsigned(
                        self.required(object, pointer, field)?,
                        &child(pointer, field),
                    )?;
                }
            }
            Some(_) => {
                return Err(self.error(&kind_pointer, ResourceDeclarationReason::InvalidValue));
            }
            None => {
                return Err(self.error(&kind_pointer, ResourceDeclarationReason::InvalidType));
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

    /// A select step (`contracts/selection-graph.md`, section Select step):
    /// `{"layout_id", "policy": {"path"}}`, where the layout ID has the candidate layout grammar
    /// and the policy is a task-local `policies/<name>.json` document (a `sha256` the policy may
    /// still carry is ignored, Workflow #336 PKG-S2). The step is guarded and confirmed: it
    /// declares its `guard` and its `expect_after`, and is never an unguarded trusted
    /// coordinate. Whether the layout and the policy fit the task is checked with the policy
    /// document (`select_policy`).
    fn select_step(
        &self,
        operation: &Map<String, Value>,
        value: &Value,
        pointer: &str,
    ) -> CliOutcome<()> {
        let select_pointer = child(pointer, "select");
        let select = self.object(value, &select_pointer, &["layout_id", "policy"])?;
        let layout_pointer = child(&select_pointer, "layout_id");
        let layout_id = self.required(select, &select_pointer, "layout_id")?;
        self.string(layout_id, &layout_pointer)?;
        if validate_candidate_layout_id(layout_id.as_str().unwrap_or_default()).is_err() {
            return Err(self.error(&layout_pointer, ResourceDeclarationReason::InvalidValue));
        }
        let policy_pointer = child(&select_pointer, "policy");
        let policy = self.required(select, &select_pointer, "policy")?;
        self.json_reference(policy, &policy_pointer)?;
        if !policy["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("policies/") && path.ends_with(".json"))
        {
            return Err(self.error(
                &child(&policy_pointer, "path"),
                ResourceDeclarationReason::InvalidValue,
            ));
        }
        for field in ["guard", "expect_after"] {
            if operation.get(field).is_none_or(Value::is_null) {
                return Err(self.error(
                    &child(pointer, field),
                    ResourceDeclarationReason::MissingField,
                ));
            }
        }
        if operation.get("unguarded_trusted_coordinate") == Some(&Value::Bool(true)) {
            return Err(self.error(
                &child(pointer, "unguarded_trusted_coordinate"),
                ResourceDeclarationReason::InvalidValue,
            ));
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
            "long_press" | "long_tap" => &["kind", "x", "y", "width", "height", "duration_ms"],
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
            // Workflow #390 lpbox: a long press's click box, `width` and `height` together; a
            // long press without either holds its point.
            if matches!(kind, "long_press" | "long_tap")
                && matches!(*field, "width" | "height")
                && !object.contains_key("width")
                && !object.contains_key("height")
            {
                continue;
            }
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
                "verify_text",
            ],
        )?;
        if object.contains_key("check") && !self.schema_0_6_or_later() {
            return Err(self.error(
                &child(pointer, "check"),
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
        // Workflow #396 REC-1: a locate OCR target guards the input from task schema `0.9`.
        if object.contains_key("verify_text") && self.schema != Some("0.9") {
            return Err(self.error(
                &child(pointer, "verify_text"),
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
        for field in ["verify_template", "color_probe", "check", "verify_text"] {
            if let Some(value) = object.get(field).filter(|value| !value.is_null()) {
                self.string(value, &child(pointer, field))?;
            }
        }
        Ok(())
    }

    /// Workflow #396 REC-8: `candidate_layouts[].shared_ocr` of task schema `0.9`, a
    /// `fixed_slots` layout's one shared detection read: a `rect` or `full_frame` region, the
    /// OCR request's `languages`, `timeout_ms`, `model_ref` and `model_sha256`, `blocks` and an
    /// optional `line_merge`. Its shape is checked here; its values with the derived
    /// recognition pack (`contracts/selection-graph.md`, section Shared OCR read).
    fn shared_ocr(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let object = self.object(
            value,
            pointer,
            &[
                "region",
                "languages",
                "timeout_ms",
                "model_ref",
                "model_sha256",
                "blocks",
                "line_merge",
            ],
        )?;
        self.source_region(
            self.required(object, pointer, "region")?,
            &child(pointer, "region"),
            false,
        )?;
        self.strings(
            self.required(object, pointer, "languages")?,
            &child(pointer, "languages"),
        )?;
        self.unsigned(
            self.required(object, pointer, "timeout_ms")?,
            &child(pointer, "timeout_ms"),
        )?;
        for field in ["model_ref", "model_sha256", "blocks"] {
            self.string(
                self.required(object, pointer, field)?,
                &child(pointer, field),
            )?;
        }
        if let Some(line_merge) = object.get("line_merge") {
            let line_merge_pointer = child(pointer, "line_merge");
            if line_merge.is_object() {
                let factor = self.object(line_merge, &line_merge_pointer, &["gap_factor"])?;
                self.unsigned(
                    self.required(factor, &line_merge_pointer, "gap_factor")?,
                    &child(&line_merge_pointer, "gap_factor"),
                )?;
            } else {
                self.string(line_merge, &line_merge_pointer)?;
            }
        }
        Ok(())
    }

    /// Workflow #396 REC-1: `ocr_targets[].locate` of task schema `0.9`. Its shape is checked
    /// here; its values with the derived recognition pack (`contracts/ocr-fields.md`).
    fn locate(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        const CELLS: [&str; 8] = [
            "x", "y", "width", "height", "pitch_x", "pitch_y", "columns", "rows",
        ];
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let object = self.object(
            value,
            pointer,
            &["normalize", "blocks", "line_merge", "cells"],
        )?;
        for field in ["normalize", "blocks"] {
            self.string(
                self.required(object, pointer, field)?,
                &child(pointer, field),
            )?;
        }
        if let Some(line_merge) = object.get("line_merge") {
            let line_merge_pointer = child(pointer, "line_merge");
            if line_merge.is_object() {
                let factor = self.object(line_merge, &line_merge_pointer, &["gap_factor"])?;
                self.unsigned(
                    self.required(factor, &line_merge_pointer, "gap_factor")?,
                    &child(&line_merge_pointer, "gap_factor"),
                )?;
            } else {
                self.string(line_merge, &line_merge_pointer)?;
            }
        }
        if let Some(cells) = object.get("cells") {
            let cells_pointer = child(pointer, "cells");
            let cells = self.object(cells, &cells_pointer, &CELLS)?;
            for field in CELLS {
                if self
                    .required(cells, &cells_pointer, field)?
                    .as_i64()
                    .is_none()
                {
                    return Err(self.error(
                        &child(&cells_pointer, field),
                        ResourceDeclarationReason::InvalidType,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Workflow #396 REC-2: `unique` of an `anchors[]`, `verify_templates[]` or `ocr_targets[]`
    /// entry of task schema `0.9`. Its shape is checked here; its value with the derived
    /// recognition pack.
    fn unique(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let object = self.object(value, pointer, &["margin_milli"])?;
        self.unsigned(
            self.required(object, pointer, "margin_milli")?,
            &child(pointer, "margin_milli"),
        )
    }

    /// Workflow #396 REC-5: `scales` of an `anchors[]` or `verify_templates[]` entry of task
    /// schema `0.9`: `{"factors_milli": [...]}` of integers. Its values are checked with the
    /// derived recognition pack.
    fn scales(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        let object = self.object(value, pointer, &["factors_milli"])?;
        let factors_pointer = child(pointer, "factors_milli");
        let factors = self.array(
            self.required(object, pointer, "factors_milli")?,
            &factors_pointer,
        )?;
        for (position, factor) in factors.iter().enumerate() {
            self.unsigned(factor, &child(&factors_pointer, &position.to_string()))?;
        }
        Ok(())
    }

    /// Workflow #396 REC-7: the `mask` (`{"type": "range", "lower", "upper"}`) or MAA
    /// `maskRange` (`[lower, upper]`) of an `anchors[]` or `verify_templates[]` entry of task
    /// schema `0.9`: integer gray bounds in 0..=255 with `lower <= upper`, one of the two forms.
    fn mask(
        &self,
        entry: &Map<String, Value>,
        field: &str,
        value: &Value,
        pointer: &str,
    ) -> CliOutcome<()> {
        if self.schema != Some("0.9") {
            return Err(self.error(pointer, ResourceDeclarationReason::UnconsumedField));
        }
        if field == "maskRange" && entry.contains_key("mask") {
            return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
        }
        let (lower, upper) = if field == "mask" {
            let mask = self.object(value, pointer, &["type", "lower", "upper"])?;
            let kind_pointer = child(pointer, "type");
            let kind = self.required(mask, pointer, "type")?;
            self.string(kind, &kind_pointer)?;
            if kind.as_str() != Some("range") {
                return Err(self.error(&kind_pointer, ResourceDeclarationReason::InvalidValue));
            }
            (
                (
                    self.required(mask, pointer, "lower")?,
                    child(pointer, "lower"),
                ),
                (
                    self.required(mask, pointer, "upper")?,
                    child(pointer, "upper"),
                ),
            )
        } else {
            let range = self.array(value, pointer)?;
            if range.len() != 2 {
                return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
            }
            (
                (&range[0], child(pointer, "0")),
                (&range[1], child(pointer, "1")),
            )
        };
        let bound = |(value, pointer): (&Value, String)| match value.as_u64() {
            Some(gray) if gray <= u64::from(u8::MAX) => Ok(gray),
            _ if value.is_i64() || value.is_u64() => {
                Err(self.error(&pointer, ResourceDeclarationReason::InvalidValue))
            }
            _ => Err(self.error(&pointer, ResourceDeclarationReason::InvalidType)),
        };
        if bound(lower)? > bound(upper)? {
            return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
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
                "unique",
                "scales",
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
                "color_check",
                "maa_task",
                "maa_task_id",
                "provenance",
                "unique",
                "scales",
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
                "locate",
                "unique",
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
                "locate" => self.locate(value, &pointer)?,
                "unique" => self.unique(value, &pointer)?,
                // Workflow #396 REC-5.
                "scales" => self.scales(value, &pointer)?,
                "rect_move" => self.rect_move(value, &pointer)?,
                "method" => self.method(value, &pointer)?,
                // Workflow #396 REC-7: a gray-range mask of task schema 0.9.
                "mask" | "maskRange" => self.mask(object, field, value, &pointer)?,
                // Workflow #396 REC-7: `verify_templates[]` takes `color_check` from task
                // schema 0.9 on.
                "color_check" if family == "verify_templates" && self.schema != Some("0.9") => {
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
    /// 2..=8 distinct member targets under exactly one of `all_of` and `any_of`, or (task
    /// schema `0.9`, Workflow #396 REC-6) declares one `value_test`. Whether each member exists,
    /// and is not itself a check, is checked against the derived pack; a value test's sides and
    /// values are checked with the derived pack's declaration rules.
    fn checks(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        for (index, check) in self.array(value, pointer)?.iter().enumerate() {
            let pointer = child(pointer, &index.to_string());
            let object = self.object(check, &pointer, &["id", "all_of", "any_of", "value_test"])?;
            self.string(
                self.required(object, &pointer, "id")?,
                &child(&pointer, "id"),
            )?;
            if object.contains_key("value_test") {
                let value_test_pointer = child(&pointer, "value_test");
                if self.schema != Some("0.9") {
                    return Err(self.error(
                        &value_test_pointer,
                        ResourceDeclarationReason::UnconsumedField,
                    ));
                }
                if let Some(field) = ["all_of", "any_of"]
                    .into_iter()
                    .find(|field| object.contains_key(*field))
                {
                    return Err(self.error(
                        &child(&pointer, field),
                        ResourceDeclarationReason::UnconsumedField,
                    ));
                }
                let test = self.object(
                    &object["value_test"],
                    &value_test_pointer,
                    &["left", "op", "right"],
                )?;
                self.object(
                    self.required(test, &value_test_pointer, "left")?,
                    &child(&value_test_pointer, "left"),
                    &["target_id", "trim", "value", "part"],
                )?;
                self.string(
                    self.required(test, &value_test_pointer, "op")?,
                    &child(&value_test_pointer, "op"),
                )?;
                if let Some(right) = test.get("right") {
                    self.object(
                        right,
                        &child(&value_test_pointer, "right"),
                        &["const", "target_id", "trim", "value", "part", "mul", "add"],
                    )?;
                }
                continue;
            }
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
    /// layouts): each layout has an ID, a page, the kind `fixed_slots` or `repeated_anchor`, and
    /// 1..=8 distinct features; a feature may read its own `target` at an `offset` (both or
    /// neither). A `fixed_slots` layout has 1..=64 slots, whose `rect` and `click` lie inside
    /// the coordinate space and whose `targets` keys are declared features that read no target
    /// of their own. A `repeated_anchor` layout names its anchor, 1..=64 `max_instances`, an
    /// `order`, an optional `suppress_iou_milli` of 0..=999, an `instance_rect` and a `click`
    /// relative to the anchor match (a signed origin and a positive size), and an optional
    /// `readable_band` inside the coordinate space; each of its features reads a target at an
    /// offset. A field of the other kind is unknown. Whether the page is one the task declares,
    /// and whether each target exists in the derived pack with a kind the feature can read, is
    /// checked while the pack is derived.
    fn candidate_layouts(&self, value: &Value, pointer: &str, frame: [u64; 2]) -> CliOutcome<()> {
        for (index, layout) in self.array(value, pointer)?.iter().enumerate() {
            let pointer = child(pointer, &index.to_string());
            let object = self.object(
                layout,
                &pointer,
                &[
                    "id",
                    "page_id",
                    "kind",
                    "anchor",
                    "max_instances",
                    "order",
                    "suppress_iou_milli",
                    "instance_rect",
                    "click",
                    "readable_band",
                    "features",
                    "slots",
                    "unknown_identity",
                    "sample_interval_ms",
                    "shared_ocr",
                ],
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
            let anchored = match kind.as_str() {
                Some("fixed_slots") => false,
                Some("repeated_anchor") => true,
                _ => {
                    return Err(self.error(&kind_pointer, ResourceDeclarationReason::InvalidValue));
                }
            };
            let other_kind: &[&str] = if anchored {
                &["slots", "shared_ocr"]
            } else {
                &[
                    "anchor",
                    "max_instances",
                    "order",
                    "suppress_iou_milli",
                    "instance_rect",
                    "click",
                    "readable_band",
                ]
            };
            if let Some(field) = other_kind.iter().find(|field| object.contains_key(**field)) {
                return Err(self.error(
                    &child(&pointer, field),
                    ResourceDeclarationReason::UnknownField,
                ));
            }
            if let Some(shared) = object.get("shared_ocr") {
                self.shared_ocr(shared, &child(&pointer, "shared_ocr"))?;
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
            let mut placed = BTreeSet::new();
            for (feature_index, feature) in features.iter().enumerate() {
                let feature_pointer = child(&features_pointer, &feature_index.to_string());
                let feature = self.object(
                    feature,
                    &feature_pointer,
                    &[
                        "name",
                        "value",
                        "identity",
                        "consensus",
                        "integer",
                        "target",
                        "offset",
                    ],
                )?;
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
                if !matches!(
                    value.as_str(),
                    Some("passed" | "measure_milli" | "identity" | "ocr_integer")
                ) {
                    return Err(self.error(&value_pointer, ResourceDeclarationReason::InvalidValue));
                }
                if anchored || feature.contains_key("target") || feature.contains_key("offset") {
                    self.string(
                        self.required(feature, &feature_pointer, "target")?,
                        &child(&feature_pointer, "target"),
                    )?;
                    self.offset_point(
                        self.required(feature, &feature_pointer, "offset")?,
                        &child(&feature_pointer, "offset"),
                    )?;
                    placed.insert(name);
                }
            }
            if anchored {
                self.string(
                    self.required(object, &pointer, "anchor")?,
                    &child(&pointer, "anchor"),
                )?;
                let max_pointer = child(&pointer, "max_instances");
                let max = self.required(object, &pointer, "max_instances")?;
                self.unsigned(max, &max_pointer)?;
                if !max.as_u64().is_some_and(|max| {
                    (1..=CANDIDATE_PROJECTION_MAX_CANDIDATES as u64).contains(&max)
                }) {
                    return Err(self.error(&max_pointer, ResourceDeclarationReason::InvalidValue));
                }
                let order_pointer = child(&pointer, "order");
                let order = self.required(object, &pointer, "order")?;
                self.string(order, &order_pointer)?;
                if !matches!(order.as_str(), Some("top_to_bottom" | "left_to_right")) {
                    return Err(self.error(&order_pointer, ResourceDeclarationReason::InvalidValue));
                }
                if let Some(suppress) = object.get("suppress_iou_milli") {
                    let suppress_pointer = child(&pointer, "suppress_iou_milli");
                    self.unsigned(suppress, &suppress_pointer)?;
                    if suppress.as_u64().is_none_or(|milli| milli > 999) {
                        return Err(
                            self.error(&suppress_pointer, ResourceDeclarationReason::InvalidValue)
                        );
                    }
                }
                for field in ["instance_rect", "click"] {
                    self.relative_rect(
                        self.required(object, &pointer, field)?,
                        &child(&pointer, field),
                    )?;
                }
                if let Some(band) = object.get("readable_band") {
                    self.frame_rect(band, &child(&pointer, "readable_band"), frame)?;
                }
            }
            let slots_pointer = child(&pointer, "slots");
            let slots: &[Value] = if anchored {
                &[]
            } else {
                self.array(self.required(object, &pointer, "slots")?, &slots_pointer)?
            };
            if !anchored && !(1..=CANDIDATE_PROJECTION_MAX_CANDIDATES).contains(&slots.len()) {
                return Err(self.error(&slots_pointer, ResourceDeclarationReason::InvalidValue));
            }
            for (slot_index, slot) in slots.iter().enumerate() {
                let slot_pointer = child(&slots_pointer, &slot_index.to_string());
                let slot = self.object(
                    slot,
                    &slot_pointer,
                    &["rect", "click", "targets", "identity_templates"],
                )?;
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
                    if !names.contains(name.as_str()) || placed.contains(name.as_str()) {
                        return Err(
                            self.error(&target_pointer, ResourceDeclarationReason::InvalidValue)
                        );
                    }
                    self.string(target, &target_pointer)?;
                }
            }
            // Workflow #396 REC-8: `shared_ocr` holds a source region, checked above; the
            // features are checked on the pack shape without it.
            let mut layout = layout.clone();
            if let Some(object) = layout.as_object_mut() {
                object.remove("shared_ocr");
            }
            let typed: actingcommand_recognition_pack::CandidateLayout =
                serde_json::from_value(layout).map_err(|error| {
                    self.refuse(
                        &pointer,
                        ResourceDeclarationReason::InvalidType,
                        &error.to_string(),
                    )
                })?;
            for feature in &typed.features {
                if (feature.value
                    == actingcommand_recognition_pack::CandidateFeatureValue::OcrInteger)
                    != feature.integer.is_some()
                {
                    return Err(self.refuse(
                        &pointer,
                        ResourceDeclarationReason::InvalidValue,
                        "ocr_integer value and declaration must appear together",
                    ));
                }
                if let Some(integer) = &feature.integer {
                    integer.validate().map_err(|error| {
                        self.refuse(&pointer, ResourceDeclarationReason::InvalidValue, error)
                    })?;
                }
                if (feature.value
                    == actingcommand_recognition_pack::CandidateFeatureValue::Identity)
                    != feature.identity.is_some()
                {
                    return Err(self.refuse(
                        &pointer,
                        ResourceDeclarationReason::InvalidValue,
                        "identity value and declaration must appear together",
                    ));
                }
                if let Some(identity) = &feature.identity {
                    identity.validate().map_err(|error| {
                        self.refuse(&pointer, ResourceDeclarationReason::InvalidValue, &error)
                    })?;
                }
                if let Some(consensus) = &feature.consensus {
                    consensus.validate(feature.value).map_err(|error| {
                        self.refuse(&pointer, ResourceDeclarationReason::InvalidValue, &error)
                    })?;
                }
            }
        }
        Ok(())
    }

    /// A rectangle relative to an anchor match: integers within the range of a recognition
    /// pack rectangle, a signed origin and a positive size.
    fn relative_rect(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["x", "y", "width", "height"])?;
        for field in ["x", "y", "width", "height"] {
            let field_pointer = child(pointer, field);
            let value = self
                .required(object, pointer, field)?
                .as_i64()
                .ok_or_else(|| {
                    self.error(&field_pointer, ResourceDeclarationReason::InvalidType)
                })?;
            if i32::try_from(value)
                .ok()
                .filter(|value| *value > 0 || matches!(field, "x" | "y"))
                .is_none()
            {
                return Err(self.error(&field_pointer, ResourceDeclarationReason::InvalidValue));
            }
        }
        Ok(())
    }

    /// An offset `{x, y}` of integers within the range of a recognition pack coordinate.
    fn offset_point(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["x", "y"])?;
        for field in ["x", "y"] {
            let field_pointer = child(pointer, field);
            let value = self
                .required(object, pointer, field)?
                .as_i64()
                .ok_or_else(|| {
                    self.error(&field_pointer, ResourceDeclarationReason::InvalidType)
                })?;
            if i32::try_from(value).is_err() {
                return Err(self.error(&field_pointer, ResourceDeclarationReason::InvalidValue));
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
            SharedCode::ResourceDeclarationInvalid.as_str(),
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
            SourceRead::BoundedBytes(limit) | SourceRead::SelectionPolicy(limit) => limit,
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
        if let SourceRead::SelectionPolicy(_) = read {
            select_policy(&declaration, &operation, &path, Path::new(&name), bytes)?;
            continue;
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
                // A policy document keeps its own read category and limit.
                SourceRead::SelectionPolicy(limit) => {
                    requests.insert(path, SourceRead::SelectionPolicy(limit));
                    continue;
                }
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
    let task_file = bundle.task_json_path();
    let task = Declaration {
        file: &task_file,
        schema: bundle.data.get("schema_version").and_then(Value::as_str),
    };
    for (path, read) in requests {
        let mut declaration = Declaration {
            file: &path,
            schema: None,
        };
        let bytes = files
            .read(&path)
            .map_err(|_| declaration.error("", ResourceDeclarationReason::InvalidValue))?;
        if let SourceRead::BoundedBytes(limit) | SourceRead::SelectionPolicy(limit) = read
            && bytes.len() as u64 > limit
        {
            return Err(declaration.error("", ResourceDeclarationReason::InvalidValue));
        }
        if let SourceRead::SelectionPolicy(_) = read {
            select_policy(&task, bundle, &path, &path, bytes)?;
            continue;
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

/// The policy document `file` that select steps of `bundle` name at `requested`
/// (`contracts/selection-graph.md`, section Select step). It is read as the selection-policy
/// crate reads a document: decoded, refused when its canonical form would be ambiguous
/// (floats, unsafe integers, duplicate keys), then validated. Each select step that names it
/// must name a candidate layout its own task declares on the step's `from` page (a
/// `policy.sha256` it may still carry is ignored, Workflow #336 PKG-S2); the document must
/// apply to that layout, read only features the layout declares with their type (`passed` as
/// boolean, `measure_milli` as integer), and require exactly one candidate. A document that
/// declares a record list, which `keyed_fact` reads by the candidate's identity, needs a layout
/// whose `unknown_identity` is `reject`.
/// The parser's declaration gate and contained task admission
/// both run this check, so an author sees the refusal before a package is built.
fn select_policy(
    task: &Declaration<'_>,
    bundle: &Bundle,
    requested: &Path,
    file: &Path,
    bytes: &[u8],
) -> CliOutcome<()> {
    let refuse_document = |pointer: &str, detail: String| {
        Declaration { file, schema: None }.refuse(
            pointer,
            ResourceDeclarationReason::InvalidValue,
            &detail,
        )
    };
    if bytes.len() > actingcommand_selection_policy::MAX_DOCUMENT_BYTES {
        return Err(refuse_document(
            "",
            format!(
                "{} bytes exceed the {}-byte policy document limit",
                bytes.len(),
                actingcommand_selection_policy::MAX_DOCUMENT_BYTES
            ),
        ));
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| refuse_document("", format!("not JSON: {error}")))?;
    let document = Declaration {
        file,
        schema: value.get("schema_version").and_then(Value::as_str),
    };
    let refuse = |pointer: &str, detail: String| {
        document.refuse(pointer, ResourceDeclarationReason::InvalidValue, &detail)
    };
    let policy: SelectionPolicy = serde_json::from_value(value.clone())
        .map_err(|error| refuse("", format!("not a selection policy document: {error}")))?;
    actingcommand_selection_policy::parse_canonical_json(bytes)
        .map_err(|error| refuse("", format!("not canonical: {error}")))?;
    policy
        .validate()
        .map_err(|error| refuse("", format!("invalid selection policy: {error}")))?;
    // A gate or term id becomes a selection reason's `rule_id` (a token), so one the evaluator
    // could not record is refused here, at admission, rather than met during a run.
    for (index, gate) in policy.gates.iter().enumerate() {
        if !actingcommand_selection_policy::is_rule_id(&gate.gate_id) {
            return Err(refuse(
                &format!("/gates/{index}/gate_id"),
                "not a token: a gate id is its reasons' rule_id".to_owned(),
            ));
        }
    }
    for (index, term) in policy.scoring.iter().enumerate() {
        if !actingcommand_selection_policy::is_rule_id(&term.term_id) {
            return Err(refuse(
                &format!("/scoring/{index}/term_id"),
                "not a token: a term id is its reasons' rule_id".to_owned(),
            ));
        }
    }
    let game = bundle
        .data
        .get("game")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for (index, operation) in super::array_field(&bundle.data, "operations")
        .iter()
        .enumerate()
    {
        let Some(select) = operation.get("select").filter(|value| !value.is_null()) else {
            continue;
        };
        if select
            .pointer("/policy/path")
            .and_then(Value::as_str)
            .is_none_or(|path| bundle.dir.join(path) != requested)
        {
            continue;
        }
        let pointer = format!("/operations/{index}/select");
        let layout_id = select
            .get("layout_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(layout) = super::array_field(&bundle.data, "candidate_layouts")
            .iter()
            .find(|layout| layout.get("id").and_then(Value::as_str) == Some(layout_id))
        else {
            return Err(task.refuse(
                &format!("{pointer}/layout_id"),
                ResourceDeclarationReason::InvalidValue,
                &format!("the task declares no candidate layout '{layout_id}'"),
            ));
        };
        let from = operation
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let page = layout
            .get("page_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if super::normalize_page_rule_id(game, from) != super::normalize_page_rule_id(game, page) {
            return Err(task.refuse(
                &format!("{pointer}/layout_id"),
                ResourceDeclarationReason::InvalidValue,
                &format!(
                    "candidate layout '{layout_id}' belongs to page '{page}'; the step runs from '{from}'"
                ),
            ));
        }
        if policy.applies_to.candidate_layout_id != layout_id {
            return Err(refuse(
                "/applies_to/candidate_layout_id",
                format!(
                    "the policy applies to candidate layout '{}'; the select step {pointer} evaluates '{layout_id}'",
                    policy.applies_to.candidate_layout_id
                ),
            ));
        }
        for (field_index, field) in policy.fields.iter().enumerate() {
            let feature = super::array_field(layout, "features")
                .iter()
                .find(|feature| {
                    feature.get("name").and_then(Value::as_str) == Some(field.name.as_str())
                })
                .and_then(|feature| feature.get("value").and_then(Value::as_str));
            match (feature, &field.value_type) {
                (Some("passed"), ValueType::Boolean)
                | (Some("measure_milli" | "ocr_integer"), ValueType::Integer) => {}
                (Some("identity"), ValueType::EnumString { allowed }) => {
                    let declaration = super::array_field(layout, "features")
                        .iter()
                        .find(|feature| feature["name"].as_str() == Some(field.name.as_str()))
                        .and_then(|feature| feature.get("identity"))
                        .cloned()
                        .ok_or_else(|| {
                            refuse(
                                &format!("/fields/{field_index}"),
                                "identity declaration missing".to_owned(),
                            )
                        })?;
                    let identity: actingcommand_recognition_pack::CandidateIdentityDeclaration =
                        serde_json::from_value(declaration).map_err(|error| {
                            refuse(&format!("/fields/{field_index}"), error.to_string())
                        })?;
                    let mut allowed = allowed.clone();
                    allowed.sort();
                    if allowed != identity.domain() {
                        return Err(refuse(
                            &format!("/fields/{field_index}/value_type"),
                            "identity enum differs from its closed domain".to_owned(),
                        ));
                    }
                }
                (None, _) => {
                    return Err(refuse(
                        &format!("/fields/{field_index}/name"),
                        format!(
                            "'{}' is not a feature of candidate layout '{layout_id}'",
                            field.name
                        ),
                    ));
                }
                (Some(value), _) => {
                    return Err(refuse(
                        &format!("/fields/{field_index}/value_type"),
                        format!(
                            "feature '{}' of candidate layout '{layout_id}' is {value}: a passed feature is boolean and a measure_milli feature integer",
                            field.name
                        ),
                    ));
                }
            }
        }
        // A record list is read by the candidate's identity, so the layout rejects an unknown
        // identity instead of handing the evaluator an unknown key.
        if let Some(list_index) = policy
            .facts
            .iter()
            .position(|fact| matches!(fact.value_type, ValueType::RecordList { .. }))
            && let Some(handling) = layout
                .get("unknown_identity")
                .and_then(Value::as_str)
                .filter(|handling| *handling != "reject")
        {
            return Err(refuse(
                &format!("/facts/{list_index}/value_type"),
                format!("layout={layout_id} unknown_identity={handling} record_list=declared"),
            ));
        }
        if policy.selection.required_count != 1 {
            return Err(refuse(
                "/selection/required_count",
                "a select step chooses exactly one candidate".to_owned(),
            ));
        }
    }
    Ok(())
}

impl Declaration<'_> {
    /// [`Self::error`] with a detail appended to its message.
    fn refuse(&self, pointer: &str, reason: ResourceDeclarationReason, detail: &str) -> CliError {
        let mut error = self.error(pointer, reason);
        error.message = format!("{}; {detail}", error.message);
        error
    }

    fn scheduling(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(
            value,
            pointer,
            &["designated_operation", "designated_effects", "mappings"],
        )?;
        if let Some(operation) = object
            .get("designated_operation")
            .filter(|value| !value.is_null())
        {
            self.string(operation, &child(pointer, "designated_operation"))?;
        }
        // Workflow #390 OUT-2: a designated effect set is a task schema `0.9` declaration.
        if let Some(effects) = object.get("designated_effects") {
            let effects_pointer = child(pointer, "designated_effects");
            if self.schema != Some("0.9") {
                return Err(
                    self.error(&effects_pointer, ResourceDeclarationReason::UnconsumedField)
                );
            }
            for (index, effect) in self.array(effects, &effects_pointer)?.iter().enumerate() {
                let pointer = child(&effects_pointer, &index.to_string());
                let effect = self.object(effect, &pointer, &["operation_id", "max_repeats"])?;
                self.string(
                    self.required(effect, &pointer, "operation_id")?,
                    &child(&pointer, "operation_id"),
                )?;
                if let Some(repeats) = effect.get("max_repeats") {
                    let repeats_pointer = child(&pointer, "max_repeats");
                    if repeats.is_object() {
                        let fact = self.object(repeats, &repeats_pointer, &["fact_key"])?;
                        self.string(
                            self.required(fact, &repeats_pointer, "fact_key")?,
                            &child(&repeats_pointer, "fact_key"),
                        )?;
                    } else {
                        self.unsigned(repeats, &repeats_pointer)?;
                    }
                }
            }
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
        // Workflow #390 READ-1: the optional second reading key of a `current_capacity` mark.
        let members = [
            "id",
            "fact_key",
            "capacity_fact_key",
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
            let object = self.object(reading, &pointer, &members)?;
            for field in fields {
                self.required(object, &pointer, field)?;
            }
            for field in ["id", "fact_key", "page_id", "target_id", "trim"] {
                self.string(&object[field], &child(&pointer, field))?;
            }
            if let Some(capacity_fact_key) = object.get("capacity_fact_key") {
                self.string(capacity_fact_key, &child(&pointer, "capacity_fact_key"))?;
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
                let refused = TaskflowCode::ALL
                    .iter()
                    .copied()
                    .find(|code| code.code() == reason);
                let path: &[&str] = match refused {
                    Some(TaskflowCode::ResourceReadingIdInvalid) => &["id"],
                    Some(TaskflowCode::ResourceReadingFactKeyInvalid) => &["fact_key"],
                    Some(TaskflowCode::ResourceReadingCapacityFactKeyInvalid) => {
                        &["capacity_fact_key"]
                    }
                    Some(TaskflowCode::ResourceReadingPageIdInvalid) => &["page_id"],
                    Some(TaskflowCode::ResourceReadingTargetIdInvalid) => &["target_id"],
                    Some(TaskflowCode::ResourceReadingValueMaxInvalid) => &["value", "max"],
                    Some(TaskflowCode::ResourceReadingValueMinInvalid) => &["value", "min"],
                    Some(TaskflowCode::ResourceReadingMinimumConfidenceInvalid) => {
                        &["minimum_confidence_milli"]
                    }
                    Some(TaskflowCode::ResourceReadingValidForInvalid) => &["valid_for_ms"],
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
            // Workflow #390 READ-1: the capacity key is a reading key of the task like any other.
            if let Some(capacity_fact_key) = declaration.capacity_fact_key
                && !fact_keys.insert(capacity_fact_key)
            {
                return Err(self.error(
                    &child(&pointer, "capacity_fact_key"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
        }
        Ok(())
    }

    /// Workflow #384 D2-S3: operation `resource_adjustments` (`contracts/resource-readings.md`,
    /// "Adjustments"). Structure is checked field by field; the rules are the shared
    /// `validate_resource_adjustments`, whose reason names the field reported here. An amount
    /// is one target and a direction, never a value: any other member is unknown. The amount
    /// target's cross-reference belongs to the parser's package validation.
    fn resource_adjustments(
        &self,
        value: &Value,
        pointer: &str,
        task: &Map<String, Value>,
    ) -> CliOutcome<()> {
        let adjustments = self.array(value, pointer)?;
        if adjustments.is_empty()
            || adjustments.len() > actingcommand_contract::MAX_RESOURCE_ADJUSTMENTS
        {
            return Err(self.error(pointer, ResourceDeclarationReason::InvalidValue));
        }
        if task.get("scheduling_outcome").is_none_or(Value::is_null) {
            return Err(self.error(
                "/scheduling_outcome",
                ResourceDeclarationReason::MissingField,
            ));
        }
        let mut declarations = Vec::with_capacity(adjustments.len());
        for (index, adjustment) in adjustments.iter().enumerate() {
            let pointer = child(pointer, &index.to_string());
            let object = self.object(
                adjustment,
                &pointer,
                &["id", "fact_key", "operation_id", "amount"],
            )?;
            for field in ["id", "fact_key", "operation_id"] {
                self.string(
                    self.required(object, &pointer, field)?,
                    &child(&pointer, field),
                )?;
            }
            if let Some(amount) = object.get("amount").filter(|amount| !amount.is_null()) {
                let amount_pointer = child(&pointer, "amount");
                let amount = self.object(amount, &amount_pointer, &["target_id", "direction"])?;
                for field in ["target_id", "direction"] {
                    self.string(
                        self.required(amount, &amount_pointer, field)?,
                        &child(&amount_pointer, field),
                    )?;
                }
                if serde_json::from_value::<actingcommand_contract::ResourceAdjustmentDirection>(
                    amount["direction"].clone(),
                )
                .is_err()
                {
                    return Err(self.error(
                        &child(&amount_pointer, "direction"),
                        ResourceDeclarationReason::InvalidValue,
                    ));
                }
            }
            let declaration: actingcommand_contract::ResourceAdjustmentDeclaration =
                serde_json::from_value(adjustment.clone())
                    .map_err(|_| self.error(&pointer, ResourceDeclarationReason::InvalidValue))?;
            declarations.push(declaration);
        }
        // A reading mark list that does not parse is refused by its own check.
        let readings = match task
            .get("resource_readings")
            .filter(|readings| !readings.is_null())
        {
            None => None,
            Some(readings) => match serde_json::from_value::<
                Vec<actingcommand_contract::ResourceReadingDeclaration>,
            >(readings.clone())
            {
                Ok(readings) => Some(readings),
                Err(_) => return Ok(()),
            },
        };
        let operation_ids = task
            .get("operations")
            .and_then(Value::as_array)
            .map(|operations| {
                operations
                    .iter()
                    .filter_map(|operation| operation.get("id").and_then(Value::as_str))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // The shared rules, entry by entry, so that a refusal names the entry it refuses.
        for (index, _) in declarations.iter().enumerate() {
            let Err(reason) = actingcommand_contract::validate_resource_adjustments(
                &declarations[..=index],
                readings.as_deref(),
                &operation_ids,
            ) else {
                continue;
            };
            let refused = TaskflowCode::ALL
                .iter()
                .copied()
                .find(|code| code.code() == reason);
            let path: &[&str] = match refused {
                Some(
                    TaskflowCode::ResourceAdjustmentIdInvalid
                    | TaskflowCode::ResourceAdjustmentIdDuplicate,
                ) => &["id"],
                Some(
                    TaskflowCode::ResourceAdjustmentFactKeyInvalid
                    | TaskflowCode::ResourceAdjustmentFactKeyDuplicate
                    | TaskflowCode::ResourceAdjustmentMarkMissing,
                ) => &["fact_key"],
                Some(TaskflowCode::ResourceAdjustmentOperationIdInvalid) => &["operation_id"],
                Some(TaskflowCode::ResourceAdjustmentTargetIdInvalid) => &["amount", "target_id"],
                _ => &[],
            };
            let pointer = path
                .iter()
                .fold(child(pointer, &index.to_string()), |pointer, field| {
                    child(&pointer, field)
                });
            return Err(self.error(&pointer, ResourceDeclarationReason::InvalidValue));
        }
        Ok(())
    }

    /// A task-local JSON document reference: `{"path"}`. Workflow #336 PKG-S2 (option 丙, D5):
    /// a `sha256` the reference may still carry is optional and ignored, whatever its value.
    fn json_reference(&self, value: &Value, pointer: &str) -> CliOutcome<()> {
        let object = self.object(value, pointer, &["path", "sha256"])?;
        self.string(
            self.required(object, pointer, "path")?,
            &child(pointer, "path"),
        )?;
        if !object["path"]
            .as_str()
            .is_some_and(super::safe_task_local_resource_path)
        {
            return Err(self.error(
                &child(pointer, "path"),
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
                "resource_adjustments",
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
                "target_consensus",
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
        if object.contains_key("target_consensus") && !self.schema_0_6_or_later() {
            return Err(self.error(
                "/target_consensus",
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
        if object.contains_key("resource_readings") && !matches!(self.schema, Some("0.8" | "0.9")) {
            return Err(self.error(
                "/resource_readings",
                ResourceDeclarationReason::UnconsumedField,
            ));
        }
        // Workflow #384 D2-S3: adjustments come with the reading marks' task schemas.
        if object.contains_key("resource_adjustments")
            && !matches!(self.schema, Some("0.8" | "0.9"))
        {
            return Err(self.error(
                "/resource_adjustments",
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
                "locale" | "goal" if !value.is_null() => self.string(value, &pointer)?,
                // Workflow #373 NAV-3b: one entry page or an entry page set.
                "entry_page" if !value.is_null() => self.page(value, &pointer)?,
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
                "resource_adjustments" => self.resource_adjustments(value, &pointer, object)?,
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
                "target_consensus" => {
                    let values: std::collections::BTreeMap<
                        String,
                        actingcommand_recognition_pack::TargetConsensus,
                    > = serde_json::from_value(value.clone()).map_err(|_| {
                        self.error(&pointer, ResourceDeclarationReason::InvalidType)
                    })?;
                    for declaration in values.values() {
                        declaration.validate().map_err(|_| {
                            self.error(&pointer, ResourceDeclarationReason::InvalidValue)
                        })?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A select step's document that gates and scores on a record list through `keyed_fact`.
    fn keyed_policy_bytes() -> Vec<u8> {
        let list = serde_json::json!({
            "source": "keyed_fact", "fact_key": "session.example.list.targets",
            "key_field": "member", "value_column": "rank",
            "absent": {"type": "integer", "value": 0}
        });
        serde_json::to_vec(&serde_json::json!({
            "schema_version": "actingcommand.selection-policy.v1",
            "policy_id": "policy-list",
            "applies_to": {"candidate_layout_id": "layout-list", "outcome_keys": {
                "selected": "list-selected", "empty": "list-empty",
                "insufficient": "list-insufficient", "ambiguous": "list-ambiguous",
                "unknown": "list-unknown"}},
            "fields": [{"name": "member", "value_type": {
                "type": "enum_string", "allowed": ["member-a", "member-b"]}}],
            "facts": [{"fact_key": "session.example.list.targets",
                       "value_type": {"type": "record_list", "key_column": "id",
                                      "columns": {"rank": {"type": "integer"}}},
                       "max_age_ms": 3_600_000, "minimum_confidence_milli": 1_000}],
            "gates": [{"gate_id": "gate-listed",
                       "predicate": {"kind": "integer_at_least", "value": list.clone(),
                                     "threshold": 1},
                       "on_unknown": {"kind": "abort_evaluation"}}],
            "scoring": [{"term_id": "term-rank", "value": list,
                         "transform": {"kind": "identity"}, "weight_milli": 1_000,
                         "on_unknown": {"kind": "abort_evaluation"}}],
            "selection": {"mode": "none_allowed", "required_count": 1},
            "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
        }))
        .expect("policy bytes")
    }

    /// Workflow #336 PKG-S1: a select step's pin is still required and format-checked, but never
    /// compared with the document, so a fixed well-formed pin stands in for it.
    const POLICY_PIN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// A task whose one select step names the document, on a layout with `unknown_identity`.
    fn task_bundle(unknown_identity: &str) -> Bundle {
        Bundle {
            task_id: "list-task".to_owned(),
            dir: PathBuf::from("tasks/list-task"),
            data: serde_json::json!({
                "game": "game-a",
                "operations": [{"id": "choose", "from": "list", "select": {
                    "layout_id": "layout-list",
                    "policy": {"path": "policies/list.json",
                               "sha256": POLICY_PIN}}}],
                "candidate_layouts": [{"id": "layout-list", "page_id": "list",
                    "unknown_identity": unknown_identity,
                    "features": [{"name": "member", "value": "identity", "identity": {
                        "entries": [{"id": "member-a", "aliases": ["a"]},
                                    {"id": "member-b", "aliases": ["b"]}],
                        "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                        "minimum_margin": 1, "minimum_confidence_milli": 800}}}]}]
            }),
        }
    }

    #[test]
    fn a_record_list_document_needs_a_layout_that_rejects_unknown_identities() {
        let bytes = keyed_policy_bytes();
        let task = Declaration {
            file: Path::new("tasks/list-task/task.json"),
            schema: Some("0.9"),
        };
        let requested = PathBuf::from("tasks/list-task").join("policies/list.json");
        select_policy(
            &task,
            &task_bundle("reject"),
            &requested,
            &requested,
            &bytes,
        )
        .expect("a rejecting layout admits the document");

        let error = select_policy(
            &task,
            &task_bundle("readable_attributes"),
            &requested,
            &requested,
            &bytes,
        )
        .expect_err("readable attributes would hand the evaluator an unknown key");
        assert_eq!(error.code, "resource_declaration_invalid");
        let issue = error.details.expect("declaration issue");
        assert_eq!(issue["field_path"], "/facts/0/value_type");
        assert_eq!(issue["reason"], "invalid_value");
        assert!(
            error
                .message
                .contains("unknown_identity=readable_attributes"),
            "{}",
            error.message
        );
    }

    /// Coordinator ruling 5 on Workflow #378 A2-P: a gate or term id becomes a selection
    /// reason's `rule_id`, a token. A document whose id is not one is refused at admission with
    /// the declaration refusal pointing at the id, so no run ever records it (the outcome value
    /// check would panic in a debug build).
    #[test]
    fn a_gate_or_term_id_that_is_not_a_token_is_refused_at_admission() {
        let task = Declaration {
            file: Path::new("tasks/list-task/task.json"),
            schema: Some("0.9"),
        };
        let requested = PathBuf::from("tasks/list-task").join("policies/list.json");
        for pointer in ["/gates/0/gate_id", "/scoring/0/term_id"] {
            let mut document: Value =
                serde_json::from_slice(&keyed_policy_bytes()).expect("policy document");
            *document.pointer_mut(pointer).expect("rule id") = Value::from("Rule Listed");
            let bytes = serde_json::to_vec(&document).expect("policy bytes");
            let error = select_policy(
                &task,
                &task_bundle("reject"),
                &requested,
                &requested,
                &bytes,
            )
            .expect_err("an id that is not a token is refused at admission");
            assert_eq!(error.code, "resource_declaration_invalid");
            let issue = error.details.expect("declaration issue");
            assert_eq!(
                issue["declaration_file"],
                "tasks/list-task/policies/list.json"
            );
            assert_eq!(issue["field_path"], pointer);
            assert_eq!(issue["reason"], "invalid_value");
        }
    }

    /// Workflow #390 EXEC-2: task schema `0.9` declares an operation's `observation_window`
    /// (`window_end` with `window_ms`, `stable` with `window_ms` and `frames`) and its
    /// `input_recheck` (`fresh_frame`, `stable_frames` with `frames`); an older task schema
    /// refuses both as unconsumed, and a member the variant does not have, a missing member, an
    /// unknown variant or a member that is no unsigned integer is refused at its pointer.
    #[test]
    fn exec2_the_observation_window_and_the_input_recheck_declare_their_variants() {
        let operation = |member: &str, value: Value| {
            let mut operation = serde_json::json!({
                "id": "open", "from": "home", "to": "terminal",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true
            });
            operation[member] = value;
            operation
        };
        let task = |schema| Declaration {
            file: Path::new("tasks/task/task.json"),
            schema: Some(schema),
        };
        for (member, value) in [
            (
                "observation_window",
                serde_json::json!({"until": "window_end", "window_ms": 60_000}),
            ),
            (
                "observation_window",
                serde_json::json!({"until": "stable", "window_ms": 1_000, "frames": 3}),
            ),
            ("input_recheck", serde_json::json!({"kind": "fresh_frame"})),
            (
                "input_recheck",
                serde_json::json!({"kind": "stable_frames", "frames": 3}),
            ),
        ] {
            task("0.9")
                .operation(&operation(member, value), "/operations/0", true)
                .expect("task schema 0.9 declares it");
        }
        for (schema, member, value, pointer, refusal) in [
            (
                "0.8",
                "observation_window",
                serde_json::json!({"until": "window_end", "window_ms": 1_000}),
                "/operations/0/observation_window",
                "unconsumed_field",
            ),
            (
                "0.8",
                "input_recheck",
                serde_json::json!({"kind": "fresh_frame"}),
                "/operations/0/input_recheck",
                "unconsumed_field",
            ),
            (
                "0.9",
                "observation_window",
                serde_json::json!({"until": "window_end", "window_ms": 1_000, "frames": 3}),
                "/operations/0/observation_window/frames",
                "unknown_field",
            ),
            (
                "0.9",
                "observation_window",
                serde_json::json!({"until": "stable", "window_ms": 1_000}),
                "/operations/0/observation_window/frames",
                "missing_field",
            ),
            (
                "0.9",
                "observation_window",
                serde_json::json!({"until": "later", "window_ms": 1_000}),
                "/operations/0/observation_window/until",
                "invalid_value",
            ),
            (
                "0.9",
                "input_recheck",
                serde_json::json!({"kind": "fresh_frame", "frames": 2}),
                "/operations/0/input_recheck/frames",
                "unknown_field",
            ),
            (
                "0.9",
                "input_recheck",
                serde_json::json!({"kind": "stable_frames", "frames": "3"}),
                "/operations/0/input_recheck/frames",
                "invalid_type",
            ),
        ] {
            let error = task(schema)
                .operation(&operation(member, value), "/operations/0", true)
                .expect_err("the declaration is refused");
            assert_eq!(error.code, "resource_declaration_invalid");
            let issue = error.details.expect("declaration issue");
            assert_eq!(issue["field_path"], pointer);
            assert_eq!(issue["reason"], refusal);
        }
    }

    /// Workflow #390 EXEC-1: task schema `0.9` declares an operation's `when` (`present` or
    /// `absent`, naming a target) and its `on_guard_refused` (`next_candidate`); an older task
    /// schema refuses both as unconsumed, and a `when` with no member, two members, another
    /// member or a target that is no string, or another `on_guard_refused`, is refused at its
    /// pointer.
    #[test]
    fn exec1_when_and_on_guard_refused_declare_a_candidate_operation() {
        let operation = |member: &str, value: Value| {
            let mut operation = serde_json::json!({
                "id": "claim", "from": "home", "to": "terminal",
                "click": {"kind": "point", "x": 1, "y": 0},
                "guard": {"page_id": "home", "target_id": "flag/ready",
                          "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                          "color_probe": "flag/ready"}
            });
            operation[member] = value;
            operation
        };
        let task = |schema| Declaration {
            file: Path::new("tasks/task/task.json"),
            schema: Some(schema),
        };
        for (member, value) in [
            ("when", serde_json::json!({"present": "flag/ready"})),
            ("when", serde_json::json!({"absent": "flag/ready"})),
            ("on_guard_refused", serde_json::json!("next_candidate")),
        ] {
            task("0.9")
                .operation(&operation(member, value), "/operations/0", true)
                .expect("task schema 0.9 declares it");
        }
        for (schema, member, value, pointer, refusal) in [
            (
                "0.8",
                "when",
                serde_json::json!({"present": "flag/ready"}),
                "/operations/0/when",
                "unconsumed_field",
            ),
            (
                "0.8",
                "on_guard_refused",
                serde_json::json!("next_candidate"),
                "/operations/0/on_guard_refused",
                "unconsumed_field",
            ),
            (
                "0.9",
                "when",
                serde_json::json!({}),
                "/operations/0/when",
                "missing_field",
            ),
            (
                "0.9",
                "when",
                serde_json::json!({"present": "flag/ready", "absent": "flag/ready"}),
                "/operations/0/when",
                "invalid_value",
            ),
            (
                "0.9",
                "when",
                serde_json::json!({"passes": "flag/ready"}),
                "/operations/0/when/passes",
                "unknown_field",
            ),
            (
                "0.9",
                "when",
                serde_json::json!({"absent": 1}),
                "/operations/0/when/absent",
                "invalid_type",
            ),
            (
                "0.9",
                "on_guard_refused",
                serde_json::json!("fail"),
                "/operations/0/on_guard_refused",
                "invalid_value",
            ),
        ] {
            let error = task(schema)
                .operation(&operation(member, value), "/operations/0", true)
                .expect_err("the declaration is refused");
            assert_eq!(error.code, "resource_declaration_invalid");
            let issue = error.details.expect("declaration issue");
            assert_eq!(issue["field_path"], pointer);
            assert_eq!(issue["reason"], refusal);
        }
    }

    // Workflow #390 LAB-2 (FCM T-03): a navigation edge names its one destination (`to_page`) or
    // every page its operation may reach (`to_pages`, at least one page), never both and never
    // neither.
    #[test]
    fn lab2_a_navigation_edge_names_one_destination_or_every_page_it_may_reach() {
        let validate = |edge: Value| {
            validate_navigation_declarations(
                Path::new("navigation.json"),
                &serde_json::json!({"schema_version": "1.0", "navigation": [edge]}),
            )
        };
        let edge = |destination: Option<(&str, Value)>| {
            let mut edge = serde_json::json!({
                "id": "open", "from_page": "neutral/home",
                "click": {"kind": "rect", "x": 1, "y": 1, "width": 2, "height": 2},
                "source": ""
            });
            if let Some((name, value)) = destination {
                edge[name] = value;
            }
            edge
        };
        validate(edge(Some((
            "to_pages",
            serde_json::json!(["neutral/first", "neutral/second"]),
        ))))
        .expect("an edge with every page it may reach");
        validate(edge(Some(("to_page", serde_json::json!("neutral/first")))))
            .expect("an edge with one destination");
        let mut both = edge(Some(("to_page", serde_json::json!("neutral/first"))));
        both["to_pages"] = serde_json::json!(["neutral/second"]);
        for (edge, pointer, refusal) in [
            (both, "/navigation/0/to_pages", "unknown_field"),
            (
                edge(Some(("to_pages", serde_json::json!([])))),
                "/navigation/0/to_pages",
                "invalid_value",
            ),
            (
                edge(Some(("to_pages", serde_json::json!(["neutral/first", 7])))),
                "/navigation/0/to_pages/1",
                "invalid_type",
            ),
            (edge(None), "/navigation/0/to_page", "missing_field"),
        ] {
            let error = validate(edge).expect_err("the edge is refused");
            assert_eq!(error.code, "resource_declaration_invalid");
            let issue = error.details.expect("declaration issue");
            assert_eq!(issue["field_path"], pointer);
            assert_eq!(issue["reason"], refusal);
        }
    }
}
