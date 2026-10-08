// SPDX-License-Identifier: AGPL-3.0-only

//! The catalog header and the owner fragments: validation, the merge into
//! `contracts/outcome-codes.json`, naming (G8) and the released contract (G9).

use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub const CATALOG_SCHEMA_VERSION: &str = "actingcommand.outcome-codes.v1";
pub const RELEASE_SCHEMA_VERSION: &str = "actingcommand.outcome-codes.release.v1";
pub const HEADER_FILE: &str = "contracts/outcome-codes/catalog.json";
pub const FRAGMENT_DIR: &str = "contracts/outcome-codes";
pub const RELEASED_DIR: &str = "contracts/outcome-codes/released";
pub const MERGED_FILE: &str = "contracts/outcome-codes.json";
/// The owner the reserved UI prefixes map to; it owns no Runtime fragment.
pub const UI_OWNER: &str = "ui";
/// The first release whose entries must follow the naming rule.
pub const NAMING_SINCE: (u64, u64, u64) = (0, 12, 0);

const CATEGORIES: &[&str] = &["success", "info", "warning", "error", "fatal"];
const LAYERS: &[&str] = &[
    "process",
    "config",
    "contract",
    "client",
    "runtime",
    "host",
    "scheduler",
    "kernel",
    "store",
    "policy",
    "device",
    "vision",
    "ui",
];
const SCALAR_TYPES: &[&str] = &[
    "code",
    "location",
    "token",
    "name",
    "id",
    "path",
    "pointer",
    "url",
    "integer",
    "duration_ms",
    "unix_ms",
    "boolean",
    "hash",
    "commit",
    "version",
    "evidence",
];
const HEADER_FIELDS: &[&str] = &[
    "schema_version",
    "keys",
    "vocabularies",
    "domains",
    "fragments",
];
const KEY_FIELDS: &[&str] = &["type", "fields", "relative", "description"];
const VOCABULARY_FIELDS: &[&str] = &["description", "review", "source", "tokens"];
const TOKEN_FIELDS: &[&str] = &["description", "category"];
const FRAGMENT_FIELDS: &[&str] = &["owner", "codes", "locations"];
const CODE_FIELDS: &[&str] = &[
    "category",
    "owner",
    "layer",
    "uncertain",
    "values",
    "review",
    "status",
    "since",
    "retired_in",
    "replaced_by",
    "description",
    "common_causes",
    "handling",
];
const LOCATION_FIELDS: &[&str] = &[
    "owner",
    "layer",
    "review",
    "status",
    "since",
    "retired_in",
    "replaced_by",
    "description",
];

/// The validated header sections and the merged fragment entries.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub keys: Map<String, Value>,
    pub vocabularies: Map<String, Value>,
    pub domains: Map<String, Value>,
    /// Owner name to workspace member directory.
    pub owners: BTreeMap<String, String>,
    pub codes: BTreeMap<String, Map<String, Value>>,
    pub locations: BTreeMap<String, Map<String, Value>>,
}

impl Catalog {
    /// Every active code and location spelling.
    pub fn registered_names(&self) -> BTreeSet<String> {
        self.codes
            .iter()
            .chain(self.locations.iter())
            .filter(|(_, entry)| text(entry, "status") == Some("active"))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The owner whose fragment belongs to a workspace member.
    pub fn owner_of(&self, member: &str) -> Option<&str> {
        self.owners
            .iter()
            .find(|(_, owned)| owned.as_str() == member)
            .map(|(owner, _)| owner.as_str())
    }
}

/// Reads and validates the header and every fragment. Any problem fails the whole load.
pub fn load(root: &Path, members: &[String]) -> Result<Catalog, Vec<String>> {
    let header = read_object(root, HEADER_FILE).map_err(|error| vec![error])?;
    let mut errors = Vec::new();
    unknown_fields(&header, HEADER_FIELDS, HEADER_FILE, &mut errors);
    if text(&header, "schema_version") != Some(CATALOG_SCHEMA_VERSION) {
        errors.push(format!(
            "{HEADER_FILE}: schema_version must be {CATALOG_SCHEMA_VERSION}"
        ));
    }
    let keys = object(&header, "keys", HEADER_FILE, &mut errors);
    let vocabularies = object(&header, "vocabularies", HEADER_FILE, &mut errors);
    let domains = object(&header, "domains", HEADER_FILE, &mut errors);
    let fragments = object(&header, "fragments", HEADER_FILE, &mut errors);

    check_vocabularies(&vocabularies, &mut errors);
    check_keys(&keys, &vocabularies, &mut errors);
    let owners = check_fragment_map(&fragments, members, &mut errors);
    check_domains(&domains, &owners, &mut errors);

    let mut codes = BTreeMap::new();
    let mut locations = BTreeMap::new();
    let mut seen = BTreeMap::<String, String>::new();
    for owner in owners.keys() {
        let relative = format!("{FRAGMENT_DIR}/{owner}.json");
        if !root.join(&relative).is_file() {
            continue;
        }
        let fragment = match read_object(root, &relative) {
            Ok(fragment) => fragment,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        unknown_fields(&fragment, FRAGMENT_FIELDS, &relative, &mut errors);
        if text(&fragment, "owner") != Some(owner.as_str()) {
            errors.push(format!("{relative}: owner must be {owner}"));
        }
        for (section, is_code) in [("codes", true), ("locations", false)] {
            let entries = match fragment.get(section) {
                None => continue,
                Some(Value::Object(entries)) => entries,
                Some(_) => {
                    errors.push(format!("{relative}: {section} is not an object"));
                    continue;
                }
            };
            for (name, entry) in entries {
                let place = format!("{relative}: {name}");
                let Some(entry) = entry.as_object() else {
                    errors.push(format!("{place}: is not an object"));
                    continue;
                };
                let canonical = if is_code {
                    check_code(entry, owner, &keys, &place, &mut errors)
                } else {
                    check_location(entry, owner, &place, &mut errors)
                };
                if let Some(previous) = seen.insert(name.clone(), relative.clone()) {
                    errors.push(format!("{place}: also registered in {previous}"));
                    continue;
                }
                if is_code {
                    codes.insert(name.clone(), canonical);
                } else {
                    locations.insert(name.clone(), canonical);
                }
            }
        }
    }
    check_stray_fragments(root, &owners, &mut errors);
    for (table, label) in [(&codes, "code"), (&locations, "location")] {
        for (name, entry) in table {
            if let Some(replacement) = text(entry, "replaced_by")
                && !table.contains_key(replacement)
            {
                errors.push(format!(
                    "{label} {name}: replaced_by names no {label} {replacement}"
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(Catalog {
            keys,
            vocabularies,
            domains,
            owners,
            codes,
            locations,
        })
    } else {
        Err(errors)
    }
}

/// The merged file's text: the header sections and every fragment entry, sorted by name.
pub fn merged_text(catalog: &Catalog) -> Result<String, String> {
    let mut document = Map::new();
    document.insert(
        "schema_version".to_owned(),
        Value::String(CATALOG_SCHEMA_VERSION.to_owned()),
    );
    document.insert("keys".to_owned(), Value::Object(catalog.keys.clone()));
    document.insert(
        "vocabularies".to_owned(),
        Value::Object(catalog.vocabularies.clone()),
    );
    document.insert("domains".to_owned(), Value::Object(catalog.domains.clone()));
    document.insert("codes".to_owned(), entries_value(&catalog.codes));
    document.insert("locations".to_owned(), entries_value(&catalog.locations));
    let mut merged = serde_json::to_string_pretty(&Value::Object(document))
        .map_err(|error| format!("cannot write the merged catalog: {error}"))?;
    merged.push('\n');
    Ok(merged)
}

fn entries_value(entries: &BTreeMap<String, Map<String, Value>>) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(name, entry)| (name.clone(), Value::Object(entry.clone())))
            .collect(),
    )
}

/// G8: names of entries since 0.12.0, prefixes and their owners, reserved prefixes.
pub fn check_naming(catalog: &Catalog) -> Vec<String> {
    let mut violations = Vec::new();
    for (label, table) in [("code", &catalog.codes), ("location", &catalog.locations)] {
        for (name, entry) in table {
            let since = text(entry, "since").and_then(parse_version);
            if since.is_some_and(|since| since >= NAMING_SINCE) && !is_registered_name(name) {
                violations.push(format!(
                    "G8 {label} {name}: breaks the form ^[a-z][a-z0-9]*(_[a-z0-9]+)+$ (3-64 bytes)"
                ));
            }
            let owner = text(entry, "owner").unwrap_or_default();
            match longest_domain(&catalog.domains, name) {
                None => violations.push(format!(
                    "G8 {label} {name}: no domain in {HEADER_FILE} covers its prefix"
                )),
                Some((prefix, owners)) => {
                    if owners.contains(&UI_OWNER) {
                        violations.push(format!(
                            "G8 {label} {name}: the prefix {prefix} is reserved for the UI"
                        ));
                    } else if !owners.contains(&owner) {
                        violations.push(format!(
                            "G8 {label} {name}: the prefix {prefix} belongs to {}, not {owner}",
                            owners.join(", ")
                        ));
                    }
                }
            }
        }
    }
    violations
}

/// G9: the committed merged file equals the merge; released settled entries are kept, and a
/// released retired entry stays retired.
pub fn check_contract(root: &Path, catalog: &Catalog) -> Vec<String> {
    let mut violations = Vec::new();
    match merged_text(catalog) {
        Err(error) => violations.push(format!("G9 {error}")),
        Ok(merged) => match fs::read_to_string(root.join(MERGED_FILE)) {
            Err(error) => violations.push(format!("G9 {MERGED_FILE}: cannot read: {error}")),
            Ok(committed) => {
                if committed.replace("\r\n", "\n") != merged {
                    violations.push(format!(
                        "G9 {MERGED_FILE} is not the merge of the fragments: commit the \
                         outcome-codes-<sha> artifact that CI uploaded for this exact head, \
                         unchanged"
                    ));
                }
            }
        },
    }
    let Ok(entries) = fs::read_dir(root.join(RELEASED_DIR)) else {
        return violations;
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    files.sort();
    for file in files {
        let relative = format!(
            "{RELEASED_DIR}/{}",
            file.file_name().unwrap_or_default().to_string_lossy()
        );
        match read_object(root, &relative) {
            Err(error) => violations.push(format!("G9 {error}")),
            Ok(snapshot) => check_snapshot(&relative, &snapshot, catalog, &mut violations),
        }
    }
    violations
}

fn check_snapshot(
    relative: &str,
    snapshot: &Map<String, Value>,
    catalog: &Catalog,
    violations: &mut Vec<String>,
) {
    if text(snapshot, "schema_version") != Some(RELEASE_SCHEMA_VERSION) {
        violations.push(format!(
            "G9 {relative}: schema_version must be {RELEASE_SCHEMA_VERSION}"
        ));
        return;
    }
    for (section, table) in [("codes", &catalog.codes), ("locations", &catalog.locations)] {
        let Some(released) = snapshot.get(section).and_then(Value::as_object) else {
            continue;
        };
        for (name, old) in released {
            let Some(old) = old.as_object() else {
                violations.push(format!("G9 {relative}: {name} is not an object"));
                continue;
            };
            let current = table.get(name);
            if text(old, "status") == Some("retired")
                && current.is_some_and(|current| text(current, "status") == Some("active"))
            {
                violations.push(format!(
                    "G9 {relative}: {name} was retired and is active again"
                ));
            }
            if text(old, "review") != Some("settled") {
                continue;
            }
            let Some(current) = current else {
                violations.push(format!("G9 {relative}: settled {name} is missing"));
                continue;
            };
            if text(old, "category") != text(current, "category") {
                violations.push(format!(
                    "G9 {relative}: settled {name} changed its category"
                ));
            }
            check_snapshot_values(relative, name, old, current, catalog, violations);
        }
    }
}

fn check_snapshot_values(
    relative: &str,
    name: &str,
    old: &Map<String, Value>,
    current: &Map<String, Value>,
    catalog: &Catalog,
    violations: &mut Vec<String>,
) {
    let empty = Map::new();
    let old_values = old
        .get("values")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let current_values = current
        .get("values")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (key, spec) in old_values {
        let old_type = spec.get("type").and_then(Value::as_str);
        let old_required = spec.get("required").and_then(Value::as_bool);
        let current_type = catalog
            .keys
            .get(key)
            .and_then(|entry| entry.get("type"))
            .and_then(Value::as_str);
        let current_required = current_values
            .get(key)
            .and_then(Value::as_str)
            .map(|presence| presence == "required");
        if current_required.is_none() {
            violations.push(format!("G9 {relative}: settled {name} lost key {key}"));
        } else if old_type != current_type || old_required != current_required {
            violations.push(format!(
                "G9 {relative}: settled {name} changed the type or presence of key {key}"
            ));
        }
    }
    for (key, presence) in current_values {
        if presence.as_str() == Some("required") && !old_values.contains_key(key) {
            violations.push(format!(
                "G9 {relative}: settled {name} gained the required key {key}"
            ));
        }
    }
}

fn check_vocabularies(vocabularies: &Map<String, Value>, errors: &mut Vec<String>) {
    for (name, entry) in vocabularies {
        let place = format!("{HEADER_FILE}: vocabulary {name}");
        let Some(entry) = entry.as_object() else {
            errors.push(format!("{place}: is not an object"));
            continue;
        };
        unknown_fields(entry, VOCABULARY_FIELDS, &place, errors);
        require_text(entry, "description", &place, errors);
        if let Some(review) = entry.get("review")
            && !matches!(review.as_str(), Some("pending" | "settled"))
        {
            errors.push(format!("{place}: review must be pending or settled"));
        }
        if let Some(source) = entry.get("source") {
            let valid = source.as_object().is_some_and(|source| {
                source.len() == 2
                    && text(source, "file").is_some()
                    && text(source, "enum").is_some()
            });
            if !valid {
                errors.push(format!("{place}: source must be {{file, enum}}"));
            }
        }
        let Some(tokens) = entry.get("tokens").and_then(Value::as_object) else {
            errors.push(format!("{place}: tokens is not an object"));
            continue;
        };
        for (token, item) in tokens {
            let place = format!("{place} token {token}");
            let Some(item) = item.as_object() else {
                errors.push(format!("{place}: is not an object"));
                continue;
            };
            unknown_fields(item, TOKEN_FIELDS, &place, errors);
            require_text(item, "description", &place, errors);
            if let Some(category) = item.get("category")
                && !category
                    .as_str()
                    .is_some_and(|value| CATEGORIES.contains(&value))
            {
                errors.push(format!("{place}: category is not a category"));
            }
        }
    }
}

fn check_keys(
    keys: &Map<String, Value>,
    vocabularies: &Map<String, Value>,
    errors: &mut Vec<String>,
) {
    for (name, entry) in keys {
        let place = format!("{HEADER_FILE}: key {name}");
        if !is_key_name(name) {
            errors.push(format!("{place}: a key name is lowercase snake_case"));
        }
        let Some(entry) = entry.as_object() else {
            errors.push(format!("{place}: is not an object"));
            continue;
        };
        unknown_fields(entry, KEY_FIELDS, &place, errors);
        require_text(entry, "description", &place, errors);
        let kind = text(entry, "type").unwrap_or_default();
        if !type_is_valid(kind, vocabularies) {
            errors.push(format!("{place}: type {kind} is not a key type"));
        }
        match (kind, entry.get("fields")) {
            ("records", Some(Value::Object(fields))) if !fields.is_empty() => {
                for (field, presence) in fields {
                    let field_type = keys
                        .get(field)
                        .and_then(|field| field.get("type"))
                        .and_then(Value::as_str);
                    if !field_type.is_some_and(|field_type| {
                        field_type != "evidence" && scalar_or_vocab(field_type, vocabularies)
                    }) {
                        errors.push(format!(
                            "{place}: field {field} is not a key with a scalar type"
                        ));
                    }
                    if !matches!(presence.as_str(), Some("required" | "optional")) {
                        errors.push(format!(
                            "{place}: field {field} is neither required nor optional"
                        ));
                    }
                }
            }
            ("records", _) => errors.push(format!("{place}: records need their fields")),
            (_, Some(_)) => errors.push(format!("{place}: only records have fields")),
            (_, None) => {}
        }
        if let Some(relative) = entry.get("relative")
            && (kind != "path" || relative.as_bool() != Some(true))
        {
            errors.push(format!("{place}: relative is true and only on path keys"));
        }
    }
}

fn check_fragment_map(
    fragments: &Map<String, Value>,
    members: &[String],
    errors: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();
    let mut claimed = BTreeSet::new();
    for (owner, member) in fragments {
        let place = format!("{HEADER_FILE}: fragment owner {owner}");
        if owner == UI_OWNER || !is_owner_name(owner) {
            errors.push(format!("{place}: is not a usable owner name"));
        }
        let Some(member) = member.as_str() else {
            errors.push(format!("{place}: must name a workspace member"));
            continue;
        };
        if !members.iter().any(|known| known.as_str() == member) {
            errors.push(format!("{place}: {member} is not a workspace member"));
        }
        if !claimed.insert(member.to_owned()) {
            errors.push(format!("{place}: {member} already has an owner"));
        }
        owners.insert(owner.clone(), member.to_owned());
    }
    for member in members {
        if !claimed.contains(member) {
            errors.push(format!(
                "{HEADER_FILE}: workspace member {member} has no fragment owner"
            ));
        }
    }
    owners
}

fn check_domains(
    domains: &Map<String, Value>,
    owners: &BTreeMap<String, String>,
    errors: &mut Vec<String>,
) {
    for (prefix, holders) in domains {
        let place = format!("{HEADER_FILE}: domain {prefix}");
        if !prefix.ends_with('_') || !is_key_name(prefix.trim_end_matches('_')) {
            errors.push(format!("{place}: a prefix is lowercase and ends with _"));
        }
        let Some(holders) = holders.as_array().filter(|holders| !holders.is_empty()) else {
            errors.push(format!("{place}: must list its owners"));
            continue;
        };
        for holder in holders {
            let known = holder
                .as_str()
                .is_some_and(|holder| holder == UI_OWNER || owners.contains_key(holder));
            if !known {
                errors.push(format!("{place}: {holder} is not an owner"));
            }
        }
    }
}

fn check_code(
    entry: &Map<String, Value>,
    owner: &str,
    keys: &Map<String, Value>,
    place: &str,
    errors: &mut Vec<String>,
) -> Map<String, Value> {
    unknown_fields(entry, CODE_FIELDS, place, errors);
    if !text(entry, "category").is_some_and(|category| CATEGORIES.contains(&category)) {
        errors.push(format!("{place}: category is not a category"));
    }
    if !entry.get("uncertain").is_some_and(Value::is_boolean) {
        errors.push(format!("{place}: uncertain must be true or false"));
    }
    match entry.get("values").and_then(Value::as_object) {
        None => errors.push(format!("{place}: values is not an object")),
        Some(values) => {
            for (key, presence) in values {
                if !keys.contains_key(key) {
                    errors.push(format!("{place}: value key {key} is not in the key table"));
                }
                if !matches!(presence.as_str(), Some("required" | "optional")) {
                    errors.push(format!(
                        "{place}: value key {key} is neither required nor optional"
                    ));
                }
            }
        }
    }
    require_text(entry, "common_causes", place, errors);
    require_text(entry, "handling", place, errors);
    check_common(entry, owner, place, errors);
    canonical(entry, CODE_FIELDS)
}

fn check_location(
    entry: &Map<String, Value>,
    owner: &str,
    place: &str,
    errors: &mut Vec<String>,
) -> Map<String, Value> {
    unknown_fields(entry, LOCATION_FIELDS, place, errors);
    check_common(entry, owner, place, errors);
    canonical(entry, LOCATION_FIELDS)
}

fn check_common(entry: &Map<String, Value>, owner: &str, place: &str, errors: &mut Vec<String>) {
    if text(entry, "owner") != Some(owner) {
        errors.push(format!("{place}: owner must be {owner}"));
    }
    if !text(entry, "layer").is_some_and(|layer| LAYERS.contains(&layer)) {
        errors.push(format!("{place}: layer is not a layer"));
    }
    if !matches!(text(entry, "review"), Some("pending" | "settled")) {
        errors.push(format!("{place}: review must be pending or settled"));
    }
    match text(entry, "status") {
        Some("active") => {}
        Some("retired") => {
            if !text(entry, "retired_in").is_some_and(|version| parse_version(version).is_some()) {
                errors.push(format!("{place}: a retired entry names retired_in"));
            }
        }
        _ => errors.push(format!("{place}: status must be active or retired")),
    }
    if !text(entry, "since").is_some_and(|version| parse_version(version).is_some()) {
        errors.push(format!("{place}: since must be a release version"));
    }
    require_text(entry, "description", place, errors);
}

fn check_stray_fragments(root: &Path, owners: &BTreeMap<String, String>, errors: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root.join(FRAGMENT_DIR)) else {
        errors.push(format!("{FRAGMENT_DIR}: cannot list"));
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file()
            || !path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if stem != "catalog" && !owners.contains_key(&stem) {
            errors.push(format!(
                "{FRAGMENT_DIR}/{stem}.json: no fragment owner of that name in {HEADER_FILE}"
            ));
        }
    }
}

fn canonical(entry: &Map<String, Value>, order: &[&str]) -> Map<String, Value> {
    order
        .iter()
        .filter_map(|field| {
            entry
                .get(*field)
                .map(|value| ((*field).to_owned(), value.clone()))
        })
        .collect()
}

fn read_object(root: &Path, relative: &str) -> Result<Map<String, Value>, String> {
    let source = fs::read_to_string(root.join(relative))
        .map_err(|error| format!("{relative}: cannot read: {error}"))?;
    match serde_json::from_str::<Value>(&source) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(format!("{relative}: is not a JSON object")),
        Err(error) => Err(format!("{relative}: does not parse: {error}")),
    }
}

fn object(
    parent: &Map<String, Value>,
    member: &str,
    place: &str,
    errors: &mut Vec<String>,
) -> Map<String, Value> {
    match parent.get(member) {
        Some(Value::Object(object)) => object.clone(),
        _ => {
            errors.push(format!("{place}: {member} is not an object"));
            Map::new()
        }
    }
}

fn unknown_fields(
    entry: &Map<String, Value>,
    known: &[&str],
    place: &str,
    errors: &mut Vec<String>,
) {
    for field in entry.keys() {
        if !known.contains(&field.as_str()) {
            errors.push(format!("{place}: unknown member {field}"));
        }
    }
}

fn require_text(entry: &Map<String, Value>, field: &str, place: &str, errors: &mut Vec<String>) {
    if !text(entry, field).is_some_and(|value| !value.trim().is_empty()) {
        errors.push(format!("{place}: {field} must be a non-empty text"));
    }
}

/// A string member of a JSON object.
pub fn text<'a>(entry: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    entry.get(field).and_then(Value::as_str)
}

fn scalar_or_vocab(kind: &str, vocabularies: &Map<String, Value>) -> bool {
    SCALAR_TYPES.contains(&kind)
        || kind
            .strip_prefix("vocab:")
            .is_some_and(|name| vocabularies.contains_key(name))
}

fn type_is_valid(kind: &str, vocabularies: &Map<String, Value>) -> bool {
    if kind == "records" {
        return true;
    }
    if let Some(item) = kind
        .strip_prefix("list<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        return item != "evidence" && scalar_or_vocab(item, vocabularies);
    }
    scalar_or_vocab(kind, vocabularies)
}

fn longest_domain<'a>(
    domains: &'a Map<String, Value>,
    name: &str,
) -> Option<(&'a str, Vec<&'a str>)> {
    domains
        .iter()
        .filter(|(prefix, _)| name.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(prefix, owners)| {
            let owners = owners
                .as_array()
                .map(|owners| owners.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            (prefix.as_str(), owners)
        })
}

/// `x.y.z` with decimal parts.
pub fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// `^[a-z][a-z0-9]*(_[a-z0-9]+)+$`, 3 to 64 bytes: the form of codes and locations.
pub fn is_registered_name(name: &str) -> bool {
    if !(3..=64).contains(&name.len()) {
        return false;
    }
    let mut parts = name.split('_');
    let first_ok = parts.next().is_some_and(|first| {
        first.starts_with(|character: char| character.is_ascii_lowercase())
            && first
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    });
    let mut rest = 0;
    let rest_ok = parts.all(|part| {
        rest += 1;
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    });
    first_ok && rest_ok && rest > 0
}

fn is_key_name(name: &str) -> bool {
    name.starts_with(|character: char| character.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_owner_name(name: &str) -> bool {
    name.starts_with(|character: char| character.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
