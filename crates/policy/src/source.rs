// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{BTreeMap, HashSet};

use serde::de::DeserializeOwned;

use crate::{
    CatalogDiagnostic, CatalogDiagnosticCode, DiagnosticSeverity, RequiredNullable,
    SchedulingDocumentKind, SourceLocation,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDocumentSource {
    pub source_uri: String,
    pub bytes: Vec<u8>,
}

impl CatalogDocumentSource {
    pub fn new(source_uri: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            source_uri: source_uri.into(),
            bytes: bytes.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSources {
    pub tasks: CatalogDocumentSource,
    pub pools: CatalogDocumentSource,
    pub activity: CatalogDocumentSource,
    pub timeline: CatalogDocumentSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    line: u32,
    column: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceMap {
    kind: SchedulingDocumentKind,
    source_uri: String,
    positions: BTreeMap<String, Position>,
}

impl SourceMap {
    pub(crate) fn diagnostic(
        &self,
        code: CatalogDiagnosticCode,
        path: impl Into<String>,
        reason: impl Into<String>,
        descriptor: Option<(&str, u64)>,
    ) -> CatalogDiagnostic {
        let path = path.into();
        let position = self.location(&path);
        CatalogDiagnostic {
            code,
            severity: DiagnosticSeverity::Error,
            json_path: path,
            source: SourceLocation {
                document: self.kind,
                source_uri: self.source_uri.clone(),
                line: position.line,
                column: position.column,
            },
            reason: bounded_reason(reason.into()),
            schema_version: RequiredNullable(Some(crate::SCHEDULING_SCHEMA_VERSION.to_owned())),
            catalog_id: RequiredNullable(descriptor.map(|(id, _)| id.to_owned())),
            catalog_version: RequiredNullable(descriptor.map(|(_, version)| version)),
        }
    }

    fn location(&self, path: &str) -> Position {
        let mut candidate = path;
        loop {
            if let Some(position) = self.positions.get(candidate) {
                return *position;
            }
            let Some(index) = candidate.rfind('/') else {
                return Position { line: 1, column: 1 };
            };
            candidate = &candidate[..index];
        }
    }

    fn nearest_path(&self, line: u32, column: u32) -> String {
        self.positions
            .iter()
            .filter(|(_, position)| (position.line, position.column) <= (line, column))
            .max_by_key(|(_, position)| (position.line, position.column))
            .map(|(path, _)| path.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub(crate) struct ParsedDocument<T> {
    pub(crate) value: T,
    pub(crate) source_map: SourceMap,
}

pub(crate) fn parse_document<T: DeserializeOwned>(
    source: &CatalogDocumentSource,
    kind: SchedulingDocumentKind,
) -> Result<ParsedDocument<T>, Box<CatalogDiagnostic>> {
    let text = std::str::from_utf8(&source.bytes).map_err(|error| {
        let (line, column) = position_for_offset(&source.bytes, error.valid_up_to());
        basic_diagnostic(
            source,
            kind,
            CatalogDiagnosticCode::InvalidJson,
            String::new(),
            line,
            column,
            "catalog document is not valid UTF-8".to_owned(),
        )
    })?;

    if let Err(error) = serde_json::from_str::<serde_json::Value>(text) {
        return Err(Box::new(basic_diagnostic(
            source,
            kind,
            CatalogDiagnosticCode::InvalidJson,
            String::new(),
            error.line() as u32,
            error.column() as u32,
            error.to_string(),
        )));
    }

    let source_map = JsonScanner::new(text, kind, source.source_uri.clone())
        .scan()
        .map_err(|error| match error {
            ScanError::DuplicateKey {
                path,
                position,
                key,
            } => basic_diagnostic(
                source,
                kind,
                CatalogDiagnosticCode::DuplicateKey,
                path,
                position.line,
                position.column,
                format!("duplicate object key `{key}`"),
            ),
            ScanError::Invalid {
                path,
                position,
                reason,
            } => basic_diagnostic(
                source,
                kind,
                CatalogDiagnosticCode::InvalidJson,
                path,
                position.line,
                position.column,
                reason,
            ),
        })?;

    let value = serde_json::from_str::<T>(text).map_err(|error| {
        let line = error.line() as u32;
        let column = error.column() as u32;
        let reason = error.to_string();
        let code = if reason.contains("unknown field") {
            CatalogDiagnosticCode::UnknownField
        } else if reason.contains("missing field") {
            CatalogDiagnosticCode::MissingRequiredField
        } else {
            CatalogDiagnosticCode::TypeMismatch
        };
        basic_diagnostic(
            source,
            kind,
            code,
            source_map.nearest_path(line, column),
            line,
            column,
            reason,
        )
    })?;

    Ok(ParsedDocument { value, source_map })
}

fn basic_diagnostic(
    source: &CatalogDocumentSource,
    kind: SchedulingDocumentKind,
    code: CatalogDiagnosticCode,
    json_path: String,
    line: u32,
    column: u32,
    reason: String,
) -> CatalogDiagnostic {
    CatalogDiagnostic {
        code,
        severity: DiagnosticSeverity::Error,
        json_path,
        source: SourceLocation {
            document: kind,
            source_uri: source.source_uri.clone(),
            line: line.max(1),
            column: column.max(1),
        },
        reason: bounded_reason(reason),
        schema_version: RequiredNullable(None),
        catalog_id: RequiredNullable(None),
        catalog_version: RequiredNullable(None),
    }
}

fn bounded_reason(reason: String) -> String {
    truncate_utf8(&reason, crate::MAX_TEXT_BYTES).to_owned()
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn position_for_offset(bytes: &[u8], offset: usize) -> (u32, u32) {
    let prefix = String::from_utf8_lossy(&bytes[..offset.min(bytes.len())]);
    let mut line = 1_u32;
    let mut column = 1_u32;
    for character in prefix.chars() {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

enum ScanError {
    DuplicateKey {
        path: String,
        position: Position,
        key: String,
    },
    Invalid {
        path: String,
        position: Position,
        reason: String,
    },
}

struct JsonScanner<'a> {
    text: &'a str,
    index: usize,
    line: u32,
    column: u32,
    kind: SchedulingDocumentKind,
    source_uri: String,
    positions: BTreeMap<String, Position>,
}

impl<'a> JsonScanner<'a> {
    fn new(text: &'a str, kind: SchedulingDocumentKind, source_uri: String) -> Self {
        Self {
            text,
            index: 0,
            line: 1,
            column: 1,
            kind,
            source_uri,
            positions: BTreeMap::new(),
        }
    }

    fn scan(mut self) -> Result<SourceMap, ScanError> {
        self.skip_whitespace();
        self.scan_value("")?;
        self.skip_whitespace();
        if self.index != self.text.len() {
            return Err(self.invalid("", "trailing data after JSON document"));
        }
        Ok(SourceMap {
            kind: self.kind,
            source_uri: self.source_uri,
            positions: self.positions,
        })
    }

    fn scan_value(&mut self, path: &str) -> Result<(), ScanError> {
        self.skip_whitespace();
        self.positions.insert(path.to_owned(), self.position());
        match self.peek() {
            Some('{') => self.scan_object(path),
            Some('[') => self.scan_array(path),
            Some('"') => self.scan_string().map(|_| ()),
            Some('t') => self.consume_literal(path, "true"),
            Some('f') => self.consume_literal(path, "false"),
            Some('n') => self.consume_literal(path, "null"),
            Some('-' | '0'..='9') => self.scan_number(),
            _ => Err(self.invalid(path, "expected JSON value")),
        }
    }

    fn scan_object(&mut self, path: &str) -> Result<(), ScanError> {
        self.advance();
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.advance();
            return Ok(());
        }

        let mut keys = HashSet::new();
        loop {
            self.skip_whitespace();
            let key_position = self.position();
            let key = self.scan_string()?;
            let child_path = format!("{path}/{}", escape_pointer_segment(&key));
            if !keys.insert(key.clone()) {
                return Err(ScanError::DuplicateKey {
                    path: child_path,
                    position: key_position,
                    key,
                });
            }
            self.skip_whitespace();
            self.expect(path, ':')?;
            self.scan_value(&child_path)?;
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance();
                }
                Some('}') => {
                    self.advance();
                    return Ok(());
                }
                _ => return Err(self.invalid(path, "expected `,` or `}`")),
            }
        }
    }

    fn scan_array(&mut self, path: &str) -> Result<(), ScanError> {
        self.advance();
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.advance();
            return Ok(());
        }

        let mut index = 0_usize;
        loop {
            self.scan_value(&format!("{path}/{index}"))?;
            index += 1;
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance();
                }
                Some(']') => {
                    self.advance();
                    return Ok(());
                }
                _ => return Err(self.invalid(path, "expected `,` or `]`")),
            }
        }
    }

    fn scan_string(&mut self) -> Result<String, ScanError> {
        let start = self.index;
        self.expect("", '"')?;
        loop {
            match self.peek() {
                Some('"') => {
                    self.advance();
                    return serde_json::from_str(&self.text[start..self.index])
                        .map_err(|error| self.invalid("", error.to_string()));
                }
                Some('\\') => {
                    self.advance();
                    let Some(escape) = self.peek() else {
                        return Err(self.invalid("", "unterminated JSON escape"));
                    };
                    self.advance();
                    if escape == 'u' {
                        for _ in 0..4 {
                            match self.peek() {
                                Some(character) if character.is_ascii_hexdigit() => self.advance(),
                                _ => return Err(self.invalid("", "invalid Unicode escape")),
                            }
                        }
                    }
                }
                Some(character) if character >= '\u{20}' => self.advance(),
                Some(_) => return Err(self.invalid("", "invalid control character in string")),
                None => return Err(self.invalid("", "unterminated JSON string")),
            }
        }
    }

    fn scan_number(&mut self) -> Result<(), ScanError> {
        let start = self.index;
        while matches!(self.peek(), Some('-' | '+' | '.' | 'e' | 'E' | '0'..='9')) {
            self.advance();
        }
        if self.index == start {
            return Err(self.invalid("", "invalid JSON number"));
        }
        Ok(())
    }

    fn consume_literal(&mut self, path: &str, literal: &str) -> Result<(), ScanError> {
        for expected in literal.chars() {
            self.expect(path, expected)?;
        }
        Ok(())
    }

    fn expect(&mut self, path: &str, expected: char) -> Result<(), ScanError> {
        if self.peek() != Some(expected) {
            return Err(self.invalid(path, format!("expected `{expected}`")));
        }
        self.advance();
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r' | '\n')) {
            self.advance();
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.index..].chars().next()
    }

    fn advance(&mut self) {
        if let Some(character) = self.peek() {
            self.index += character.len_utf8();
            if character == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
    }

    fn position(&self) -> Position {
        Position {
            line: self.line,
            column: self.column,
        }
    }

    fn invalid(&self, path: &str, reason: impl Into<String>) -> ScanError {
        ScanError::Invalid {
            path: path.to_owned(),
            position: self.position(),
            reason: reason.into(),
        }
    }
}

fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TasksDocument;

    #[test]
    fn duplicate_keys_fail_with_the_duplicate_source_position() {
        let source = CatalogDocumentSource::new(
            "memory://tasks.json",
            br#"{
  "schema_version": "actingcommand.scheduling.v1",
  "schema_version": "actingcommand.scheduling.v1"
}"#,
        );
        let error = parse_document::<serde_json::Value>(&source, SchedulingDocumentKind::Tasks)
            .expect_err("duplicate key must fail");
        assert_eq!(error.code, CatalogDiagnosticCode::DuplicateKey);
        assert_eq!(error.json_path, "/schema_version");
        assert_eq!(error.source.line, 3);
    }

    #[test]
    fn typed_error_keeps_a_nonzero_source_location() {
        let source = CatalogDocumentSource::new("memory://tasks.json", br#"{"tasks": []}"#);
        let error = parse_document::<TasksDocument>(&source, SchedulingDocumentKind::Tasks)
            .expect_err("missing required fields must fail");
        assert_eq!(error.code, CatalogDiagnosticCode::MissingRequiredField);
        assert!(error.source.line > 0);
        assert!(error.source.column > 0);
    }
}
