// SPDX-License-Identifier: AGPL-3.0-only

//! Directories a resource repository excludes from declaration validation (Workflow #396
//! TOOL-2, forced compromise T-04).
//!
//! A repository keeps material that is not a program declaration, such as its regression case
//! families, in directories it names in the root file [`DECLARATION_EXCLUSIONS_FILE`].
//! `actinglab resource validate` reports a file under one of them as `excluded` only where no
//! declaration parser is mapped to it: a mapped declaration family is validated wherever it
//! lies, and an unmapped file outside the declared directories is still refused.

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    LabError, LabErrorClass, LabResult, ResourceDeclarationIssue, ResourceDeclarationReason,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Component, Path};

/// The declaration's path, relative to the repository root (the root only).
pub const DECLARATION_EXCLUSIONS_FILE: &str = "declaration-exclusions.json";
/// The declaration's `schema_version`.
pub const DECLARATION_EXCLUSIONS_SCHEMA: &str = "actingcommand.declaration-exclusions.v1";
/// At most this many directories per repository.
const MAX_EXCLUDED_DIRECTORIES: usize = 64;
/// At most this many bytes per directory.
const MAX_EXCLUDED_DIRECTORY_BYTES: usize = 1024;

/// The directories a repository declares; [`Default`] is a repository without the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclarationExclusions {
    /// Each declared directory, split into its path segments.
    directories: Vec<Vec<String>>,
}

impl DeclarationExclusions {
    /// Reads `declaration-exclusions.json`:
    /// `{"schema_version": "actingcommand.declaration-exclusions.v1", "directories": [...]}`,
    /// no other member. `directories` holds 1 to 64 distinct repository-relative directories,
    /// each written as `/`-separated normal segments (no empty, `.` or `..` segment, no leading
    /// or trailing `/`, no `\`, `:` or control character), at most 1024 bytes. Any other
    /// document is `resource_declaration_invalid` with the member's JSON pointer.
    pub fn parse(bytes: &[u8]) -> LabResult<Self> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| {
            let mut refusal = invalid("", None, ResourceDeclarationReason::InvalidType);
            refusal.message = format!("{DECLARATION_EXCLUSIONS_FILE}: JSON parse: {error}");
            refusal
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("", None, ResourceDeclarationReason::InvalidType))?;
        let schema = object.get("schema_version").and_then(Value::as_str);
        let refuse = |pointer: &str, reason| invalid(pointer, schema, reason);
        if let Some(member) = object
            .keys()
            .find(|member| !matches!(member.as_str(), "schema_version" | "directories"))
        {
            return Err(refuse(
                &format!("/{}", member.replace('~', "~0").replace('/', "~1")),
                ResourceDeclarationReason::UnknownField,
            ));
        }
        match object.get("schema_version") {
            None => {
                return Err(refuse(
                    "/schema_version",
                    ResourceDeclarationReason::MissingField,
                ));
            }
            Some(Value::String(version)) if version == DECLARATION_EXCLUSIONS_SCHEMA => {}
            Some(Value::String(_)) => {
                return Err(refuse(
                    "/schema_version",
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            Some(_) => {
                return Err(refuse(
                    "/schema_version",
                    ResourceDeclarationReason::InvalidType,
                ));
            }
        }
        let entries = match object.get("directories") {
            None => {
                return Err(refuse(
                    "/directories",
                    ResourceDeclarationReason::MissingField,
                ));
            }
            Some(Value::Array(entries)) => entries,
            Some(_) => {
                return Err(refuse(
                    "/directories",
                    ResourceDeclarationReason::InvalidType,
                ));
            }
        };
        if entries.is_empty() || entries.len() > MAX_EXCLUDED_DIRECTORIES {
            return Err(refuse(
                "/directories",
                ResourceDeclarationReason::InvalidValue,
            ));
        }
        let mut seen = BTreeSet::new();
        let mut directories = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let pointer = format!("/directories/{index}");
            let text = entry
                .as_str()
                .ok_or_else(|| refuse(&pointer, ResourceDeclarationReason::InvalidType))?;
            let segments = directory_segments(text)
                .ok_or_else(|| refuse(&pointer, ResourceDeclarationReason::InvalidValue))?;
            if !seen.insert(text) {
                return Err(refuse(&pointer, ResourceDeclarationReason::InvalidValue));
            }
            directories.push(segments);
        }
        Ok(Self { directories })
    }

    /// Whether the repository-relative `path` lies inside one of the declared directories (at
    /// any depth, compared segment by segment and case-sensitively, as Git names paths).
    pub fn covers(&self, path: &Path) -> bool {
        let mut segments = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(segment) => match segment.to_str() {
                    Some(segment) => segments.push(segment),
                    None => return false,
                },
                _ => return false,
            }
        }
        self.directories.iter().any(|directory| {
            segments.len() > directory.len()
                && directory
                    .iter()
                    .zip(&segments)
                    .all(|(declared, segment)| declared == segment)
        })
    }
}

fn directory_segments(text: &str) -> Option<Vec<String>> {
    if text.is_empty()
        || text.len() > MAX_EXCLUDED_DIRECTORY_BYTES
        || text
            .chars()
            .any(|character| character.is_control() || matches!(character, '\\' | ':'))
    {
        return None;
    }
    text.split('/')
        .map(|segment| (!matches!(segment, "" | "." | "..")).then(|| segment.to_owned()))
        .collect()
}

fn invalid(pointer: &str, schema: Option<&str>, reason: ResourceDeclarationReason) -> LabError {
    let issue = ResourceDeclarationIssue {
        declaration_file: DECLARATION_EXCLUSIONS_FILE.to_owned(),
        field_path: pointer.to_owned(),
        schema_version: schema.map(str::to_owned),
        reason,
    };
    LabError::new(
        LabErrorClass::UsageValidation,
        SharedCode::ResourceDeclarationInvalid.as_str(),
        format!("{DECLARATION_EXCLUSIONS_FILE}: field {pointer:?} is not consumable: {reason:?}"),
        &[],
    )
    .with_details(serde_json::json!(issue))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Workflow #396 TOOL-2: a repository declares the directories of its regression case
    // families; a file inside one of them is covered at any depth, and nothing outside is.
    #[test]
    fn declared_directories_cover_only_files_inside_them() {
        let exclusions = DeclarationExclusions::parse(
            br#"{"schema_version":"actingcommand.declaration-exclusions.v1","directories":["regression","fixtures/cases"]}"#,
        )
        .unwrap();
        for (path, covered) in [
            ("regression/group/case-1/case.json", true),
            ("regression/case.json", true),
            ("fixtures/cases/case-2/case.json", true),
            ("fixtures/case.json", false),
            ("regressions/case.json", false),
            ("ours/regression/case.json", false),
            ("notes.json", false),
        ] {
            assert_eq!(exclusions.covers(Path::new(path)), covered, "{path}");
        }
        assert!(!DeclarationExclusions::default().covers(Path::new("regression/case.json")));
    }

    // Workflow #396 TOOL-2 with Fail Loud: a declaration that cannot be read as written is
    // refused with its JSON pointer, never read as "nothing excluded".
    #[test]
    fn malformed_exclusion_declarations_are_refused_with_the_field_pointer() {
        let schema = DECLARATION_EXCLUSIONS_SCHEMA;
        for (document, pointer, reason) in [
            (
                r#"["regression"]"#.to_owned(),
                "",
                ResourceDeclarationReason::InvalidType,
            ),
            (
                r#"{"directories":["regression"]}"#.to_owned(),
                "/schema_version",
                ResourceDeclarationReason::MissingField,
            ),
            (
                r#"{"schema_version":"actingcommand.declaration-exclusions.v2","directories":["regression"]}"#
                    .to_owned(),
                "/schema_version",
                ResourceDeclarationReason::InvalidValue,
            ),
            (
                format!(r#"{{"schema_version":"{schema}","directories":["regression"],"note":""}}"#),
                "/note",
                ResourceDeclarationReason::UnknownField,
            ),
            (
                format!(r#"{{"schema_version":"{schema}"}}"#),
                "/directories",
                ResourceDeclarationReason::MissingField,
            ),
            (
                format!(r#"{{"schema_version":"{schema}","directories":[]}}"#),
                "/directories",
                ResourceDeclarationReason::InvalidValue,
            ),
            (
                format!(r#"{{"schema_version":"{schema}","directories":[1]}}"#),
                "/directories/0",
                ResourceDeclarationReason::InvalidType,
            ),
            (
                format!(r#"{{"schema_version":"{schema}","directories":["regression/"]}}"#),
                "/directories/0",
                ResourceDeclarationReason::InvalidValue,
            ),
            (
                format!(r#"{{"schema_version":"{schema}","directories":["../regression"]}}"#),
                "/directories/0",
                ResourceDeclarationReason::InvalidValue,
            ),
            (
                format!(r#"{{"schema_version":"{schema}","directories":["regression","regression"]}}"#),
                "/directories/1",
                ResourceDeclarationReason::InvalidValue,
            ),
        ] {
            let error = DeclarationExclusions::parse(document.as_bytes()).unwrap_err();
            assert_eq!(
                error.code,
                SharedCode::ResourceDeclarationInvalid.as_str(),
                "{document}"
            );
            assert_eq!(error.class, LabErrorClass::UsageValidation, "{document}");
            let issue: ResourceDeclarationIssue =
                serde_json::from_value(error.details.clone().unwrap()).unwrap();
            assert_eq!(issue.declaration_file, DECLARATION_EXCLUSIONS_FILE, "{document}");
            assert_eq!(issue.field_path, pointer, "{document}");
            assert_eq!(issue.reason, reason, "{document}");
        }
    }
}
