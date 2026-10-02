// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336: the two content container files of a `content-directory.v1` package. A ZIP
//! holds the directory layout; a single JSON (`actingcommand.package.content-json.v1`) holds
//! every file as a string. Each expands to the path-to-bytes table a directory reads, under the
//! same entry rules and codes, so the same content has the same digest in every container.
//! Refusals that only a container can have are `SourceTree` codes of their own.

use super::*;
use crate::content_dir::{EntryTable, check_time};
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use std::cell::Cell;
use std::time::Instant;

/// The schema of the single JSON content container.
pub const CONTENT_JSON_V1: &str = "actingcommand.package.content-json.v1";

/// A content container file, named by the extension of its locator (ASCII case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentContainer {
    /// `.zip`: one table entry per file entry, with its decompressed bytes; directory entries
    /// are ignored.
    Zip,
    /// `.json`: one table entry per key of `files`, with the UTF-8 bytes of its string.
    Json,
}

impl ContentContainer {
    pub fn from_locator(locator: &Path) -> Option<Self> {
        let extension = locator.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("zip") {
            Some(Self::Zip)
        } else if extension.eq_ignore_ascii_case("json") {
            Some(Self::Json)
        } else {
            None
        }
    }
}

/// Expands the bytes of one content container into the table admission compares and
/// assembles, under the entry rules and limits the loader applies to a container file.
pub fn expand_content_container(
    bytes: &[u8],
    container: ContentContainer,
    limits: ContainmentLimits,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    expand(bytes, container, limits, None)
}

pub(super) fn expand(
    bytes: &[u8],
    container: ContentContainer,
    limits: ContainmentLimits,
    deadline: Option<Instant>,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    if bytes.len() as u64 > limits.max_compressed_bytes {
        return Err(source_error("content_container_size_limit"));
    }
    match container {
        ContentContainer::Zip => expand_zip(bytes, limits, deadline),
        ContentContainer::Json => expand_json(bytes, limits),
    }
}

fn expand_zip(
    bytes: &[u8],
    limits: ContainmentLimits,
    deadline: Option<Instant>,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|_| source_error("content_zip_invalid"))?;
    let mut table = EntryTable::new(limits);
    for index in 0..archive.len() {
        if let Some(deadline) = deadline {
            check_time(deadline)?;
        }
        // A damaged or encrypted entry has no readable bytes.
        let mut entry = archive
            .by_index(index)
            .map_err(|_| source_error("content_zip_invalid"))?;
        // The raw name bytes read as UTF-8, never the CP437 reading of an entry without the
        // UTF-8 flag: the same path has the same digest in a directory and in a ZIP.
        let name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| source_error("content_zip_entry_invalid"))?
            .to_owned();
        if name.ends_with('/') {
            continue;
        }
        // Only regular files: a symbolic link or any other special entry is refused.
        if entry
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
        {
            return Err(source_error("content_zip_entry_invalid"));
        }
        validate_relative_ref(&name).map_err(|_| source_error("content_zip_entry_invalid"))?;
        table.add(name, |limit| {
            if entry.size() > limit {
                return Err(source_error("content_directory_size_limit"));
            }
            let mut data = Vec::new();
            Read::by_ref(&mut entry)
                .take(limit.saturating_add(1))
                .read_to_end(&mut data)
                .map_err(|_| source_error("content_zip_invalid"))?;
            Ok(data)
        })?;
    }
    Ok(table.into_entries())
}

fn expand_json(
    bytes: &[u8],
    limits: ContainmentLimits,
) -> ContainmentResult<BTreeMap<String, Vec<u8>>> {
    let refusal = Cell::new(None);
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let files = ContentJson { refusal: &refusal }
        .deserialize(&mut deserializer)
        .and_then(|files| deserializer.end().map(|()| files))
        .map_err(|_| source_error(refusal.get().unwrap_or("content_json_invalid")))?;
    let mut table = EntryTable::new(limits);
    for (relative, text) in files {
        table.add(relative, |_| Ok(text.into_bytes()))?;
    }
    Ok(table.into_entries())
}

/// The document: exactly `schema_version`, equal to `CONTENT_JSON_V1`, and a non-empty `files`.
struct ContentJson<'a> {
    refusal: &'a Cell<Option<&'static str>>,
}

impl<'de> DeserializeSeed<'de> for ContentJson<'_> {
    type Value = Vec<(String, String)>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ContentJson<'_> {
    type Value = Vec<(String, String)>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a content-json.v1 document")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut schema = false;
        let mut files = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "schema_version" if !schema => {
                    if map.next_value::<String>()? != CONTENT_JSON_V1 {
                        return Err(de::Error::custom("content_json_invalid"));
                    }
                    schema = true;
                }
                "files" if files.is_none() => {
                    files = Some(map.next_value_seed(ContentJsonFiles {
                        refusal: self.refusal,
                    })?);
                }
                _ => return Err(de::Error::custom("content_json_invalid")),
            }
        }
        match files {
            Some(files) if schema && !files.is_empty() => Ok(files),
            _ => Err(de::Error::custom("content_json_invalid")),
        }
    }
}

/// `files`: one string per path. A repeated path, which a plain map would let the later value
/// overwrite silently, and any value other than a string are refused with their own codes.
struct ContentJsonFiles<'a> {
    refusal: &'a Cell<Option<&'static str>>,
}

impl ContentJsonFiles<'_> {
    fn refuse<E: de::Error>(&self, code: &'static str) -> E {
        self.refusal.set(Some(code));
        E::custom(code)
    }
}

impl<'de> DeserializeSeed<'de> for ContentJsonFiles<'_> {
    type Value = Vec<(String, String)>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ContentJsonFiles<'_> {
    type Value = Vec<(String, String)>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object of file texts")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut files = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some(path) = map.next_key::<String>()? {
            if !seen.insert(path.clone()) {
                return Err(self.refuse("content_json_duplicate_path"));
            }
            match map.next_value::<Value>()? {
                Value::String(text) => files.push((path, text)),
                _ => return Err(self.refuse("content_json_file_not_string")),
            }
        }
        Ok(files)
    }
}
