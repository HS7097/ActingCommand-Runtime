// SPDX-License-Identifier: AGPL-3.0-only

//! Outcome codes (Workflow #378, model v2.2; contract `contracts/runtime-outcomes.md`).
//!
//! Every result that leaves a Runtime process carries one registered code with one category,
//! typed values under the keys of the one key table, and optionally a nested chain of links.
//! Meanings live in the catalog `contracts/outcome-codes.json`; the Runtime stores no
//! explanation next to a code. English exists only in `Display` and developer log lines.
//!
//! - [`Code`] and [`Location`] are what a crate emits. They can be built only through a
//!   registry ([`outcome_codes!`](crate::outcome_codes), [`outcome_locations!`](crate::outcome_locations)).
//! - [`CodeStr`] is what a reader receives: it is built only by deserialising or from a [`Code`],
//!   and an unknown spelling stays readable.
//! - [`Outcome`] is built with the typed setters that `outcome_keys!` generates from the key
//!   table, for example `Outcome::new(ContractCode::ForeignOsError).path(path)`.
//! - [`OutcomeEnvelope`] is the `actingcommand.outcome.v1` interface form, full or codes-only.

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::path::Path;
use std::sync::OnceLock;

use crate::codes::ContractCode;

/// The envelope's `schema_version`.
pub const OUTCOME_SCHEMA_VERSION: &str = "actingcommand.outcome.v1";
/// The catalog's `schema_version`.
pub const CATALOG_SCHEMA_VERSION: &str = "actingcommand.outcome-codes.v1";
/// Bytes of foreign text one `raw_text` value keeps.
pub const RAW_TEXT_MAX_BYTES: usize = 4096;
/// Items one list or records value keeps; a cut adds `<key>_total`.
pub const LIST_MAX_ITEMS: usize = 64;
/// Links one outcome keeps on an interface or in a record; a cut sets `causes_total`.
pub const CAUSES_MAX_LINKS: usize = 32;
/// The deepest link kept (the top's direct links are at depth 1).
pub const CAUSES_MAX_DEPTH: usize = 8;
/// The largest serialised envelope.
pub const ENVELOPE_MAX_BYTES: usize = 32 * 1024;

/// Room the envelope's own members (`schema_version`, `detail`) take beside the body.
const ENVELOPE_BODY_BUDGET: usize = ENVELOPE_MAX_BYTES - 128;
const NAME_MAX_BYTES: usize = 256;
const PATH_MAX_BYTES: usize = 1024;
const POINTER_MAX_BYTES: usize = 512;
const URL_MAX_BYTES: usize = 2048;
const VERSION_MAX_BYTES: usize = 64;
const RAW_TEXT_KEY: &str = "raw_text";
const RAW_TEXT_TRUNCATED_KEY: &str = "raw_text_truncated";

/// Declares an owning crate's outcome codes. Each crate keeps exactly one registry, in its
/// `src/codes.rs`, and lists every entry in its catalog fragment
/// `contracts/outcome-codes/<owner>.json` (the outcome guard's G1 checks both directions).
///
/// Attributes on the enum and on each variant pass through unchanged, serde attributes
/// included: a registry that must also (de)serialize adds `#[derive(serde::Serialize)]` and
/// `#[serde(rename = "...")]` with the registered spelling.
///
/// ```ignore
/// actingcommand_contract::outcome_codes! {
///     pub enum HostCode {
///         InstanceDiscoveryUnavailable => "instance_discovery_unavailable": error,
///     }
/// }
/// ```
#[macro_export]
macro_rules! outcome_codes {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $spelling:literal : $category:ident ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        #[allow(dead_code)]
        impl $name {
            /// Every code of this registry, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The registered spelling.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $spelling),+
                }
            }

            /// The code's one category.
            pub const fn category(self) -> $crate::outcome::Category {
                match self {
                    $(Self::$variant => $crate::__outcome_category!($category)),+
                }
            }

            /// The emitted code.
            pub const fn code(self) -> $crate::outcome::Code {
                $crate::outcome::Code::__from_registry(self.as_str(), self.category())
            }
        }

        impl ::core::convert::From<$name> for $crate::outcome::Code {
            fn from(value: $name) -> Self {
                value.code()
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

/// Declares an owning crate's locations: the registered values of the `operation`, `stage` and
/// `boundary` keys. They live beside the crate's codes in `src/codes.rs`. Attributes on the
/// enum and on each variant, serde attributes included, pass through unchanged.
#[macro_export]
macro_rules! outcome_locations {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $spelling:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        #[allow(dead_code)]
        impl $name {
            /// Every location of this registry, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The registered spelling.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $spelling),+
                }
            }

            /// The emitted location.
            pub const fn location(self) -> $crate::outcome::Location {
                $crate::outcome::Location::__from_registry(self.as_str())
            }
        }

        impl ::core::convert::From<$name> for $crate::outcome::Location {
            fn from(value: $name) -> Self {
                value.location()
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

/// Declares a closed set of tokens for one catalog vocabulary. The outcome guard's G7 checks
/// the tokens against the catalog's `vocabularies` entry of the same name. Attributes on the
/// enum and on each variant, serde attributes included, pass through unchanged.
#[macro_export]
macro_rules! outcome_vocabulary {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident : $vocabulary:literal {
            $( $(#[$variant_meta:meta])* $variant:ident => $token:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        #[allow(dead_code)]
        impl $name {
            /// Every token of this vocabulary, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The token's spelling.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $token),+
                }
            }
        }

        impl $crate::outcome::VocabularyToken for $name {
            const VOCABULARY: &'static str = $vocabulary;

            fn token(self) -> &'static str {
                self.as_str()
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __outcome_category {
    (success) => {
        $crate::outcome::Category::Success
    };
    (info) => {
        $crate::outcome::Category::Info
    };
    (warning) => {
        $crate::outcome::Category::Warning
    };
    (error) => {
        $crate::outcome::Category::Error
    };
    (fatal) => {
        $crate::outcome::Category::Fatal
    };
}

/// Generates the key table, one typed setter per key on [`Outcome`] and on [`Record`]
/// (records hold flat scalar fields only), from the lines in `outcome/keys.rs`.
macro_rules! outcome_keys {
    (@kind code) => { $crate::outcome::KeyKind::Code };
    (@kind location) => { $crate::outcome::KeyKind::Location };
    (@kind vocab($vocab:ident)) => { $crate::outcome::KeyKind::Vocab(::core::stringify!($vocab)) };
    (@kind token) => { $crate::outcome::KeyKind::Token };
    (@kind name) => { $crate::outcome::KeyKind::Name };
    (@kind id) => { $crate::outcome::KeyKind::Id };
    (@kind path) => { $crate::outcome::KeyKind::Path };
    (@kind pointer) => { $crate::outcome::KeyKind::Pointer };
    (@kind url) => { $crate::outcome::KeyKind::Url };
    (@kind integer) => { $crate::outcome::KeyKind::Integer };
    (@kind duration_ms) => { $crate::outcome::KeyKind::DurationMs };
    (@kind unix_ms) => { $crate::outcome::KeyKind::UnixMs };
    (@kind boolean) => { $crate::outcome::KeyKind::Boolean };
    (@kind hash) => { $crate::outcome::KeyKind::Hash };
    (@kind commit) => { $crate::outcome::KeyKind::Commit };
    (@kind version) => { $crate::outcome::KeyKind::Version };
    (@kind evidence) => { $crate::outcome::KeyKind::Evidence };
    (@kind list($($item:tt)*)) => {
        $crate::outcome::KeyKind::List(&outcome_keys!(@kind $($item)*))
    };
    (@kind records($($required:ident),* $(; $($optional:ident),*)?)) => {
        $crate::outcome::KeyKind::Records(&[
            $((::core::stringify!($required), true),)*
            $($((::core::stringify!($optional), false),)*)?
        ])
    };

    (@outcome $key:ident : evidence) => {
        pub fn $key(mut self, value: impl ::core::convert::AsRef<[u8]>) -> Self {
            self.put_evidence(
                ::core::stringify!($key),
                ::core::concat!(::core::stringify!($key), "_truncated"),
                value.as_ref(),
            );
            self
        }
    };
    (@outcome $key:ident : records $fields:tt) => {
        pub fn $key<I>(mut self, records: I) -> Self
        where
            I: ::core::iter::IntoIterator<Item = $crate::outcome::Record>,
        {
            $crate::outcome::put_list(
                self.values_mut(),
                ::core::stringify!($key),
                ::core::concat!(::core::stringify!($key), "_total"),
                records
                    .into_iter()
                    .map(|record| ::serde_json::Value::Object(record.values)),
            );
            self
        }
    };
    (@outcome $($rest:tt)*) => {
        outcome_keys!(@set $($rest)*);
    };
    (@record $key:ident : evidence) => {};
    (@record $key:ident : records $fields:tt) => {};
    (@record $($rest:tt)*) => {
        outcome_keys!(@set $($rest)*);
    };

    (@set $key:ident : code) => {
        pub fn $key(mut self, value: impl ::core::convert::Into<$crate::outcome::Code>) -> Self {
            let value: $crate::outcome::Code = value.into();
            $crate::outcome::put_name(self.values_mut(), ::core::stringify!($key), value.as_str());
            self
        }
    };
    (@set $key:ident : location) => {
        pub fn $key(mut self, value: impl ::core::convert::Into<$crate::outcome::Location>) -> Self {
            let value: $crate::outcome::Location = value.into();
            $crate::outcome::put_name(self.values_mut(), ::core::stringify!($key), value.as_str());
            self
        }
    };
    (@set $key:ident : vocab($vocab:ident)) => {
        pub fn $key<T: $crate::outcome::VocabularyToken>(mut self, value: T) -> Self {
            const {
                ::core::assert!(
                    $crate::outcome::vocabulary_is::<T>(::core::stringify!($vocab)),
                    "the value belongs to another vocabulary than the key's"
                )
            };
            $crate::outcome::put_name(
                self.values_mut(),
                ::core::stringify!($key),
                $crate::outcome::VocabularyToken::token(value),
            );
            self
        }
    };
    (@set $key:ident : path) => {
        pub fn $key(mut self, value: impl ::core::convert::AsRef<::std::path::Path>) -> Self {
            let value = $crate::outcome::path_value(::core::stringify!($key), value.as_ref());
            self.values_mut().insert(::core::stringify!($key).to_owned(), value);
            self
        }
    };
    (@set $key:ident : integer) => {
        pub fn $key(mut self, value: impl $crate::outcome::IntegerValue) -> Self {
            let value = $crate::outcome::IntegerValue::into_json(value);
            self.values_mut().insert(::core::stringify!($key).to_owned(), value);
            self
        }
    };
    (@set $key:ident : unix_ms) => {
        pub fn $key(mut self, value: impl $crate::outcome::IntegerValue) -> Self {
            let value = $crate::outcome::IntegerValue::into_json(value);
            self.values_mut().insert(::core::stringify!($key).to_owned(), value);
            self
        }
    };
    (@set $key:ident : duration_ms) => {
        pub fn $key(mut self, value: impl $crate::outcome::DurationValue) -> Self {
            let value = $crate::outcome::DurationValue::into_json(value);
            self.values_mut().insert(::core::stringify!($key).to_owned(), value);
            self
        }
    };
    (@set $key:ident : boolean) => {
        pub fn $key(mut self, value: bool) -> Self {
            self.values_mut().insert(::core::stringify!($key).to_owned(), ::serde_json::Value::Bool(value));
            self
        }
    };
    (@set $key:ident : list(path)) => {
        pub fn $key<I>(mut self, items: I) -> Self
        where
            I: ::core::iter::IntoIterator,
            I::Item: ::core::convert::AsRef<::std::path::Path>,
        {
            $crate::outcome::put_list(
                self.values_mut(),
                ::core::stringify!($key),
                ::core::concat!(::core::stringify!($key), "_total"),
                items
                    .into_iter()
                    .map(|item| $crate::outcome::path_value(::core::stringify!($key), item.as_ref())),
            );
            self
        }
    };
    (@set $key:ident : list(vocab($vocab:ident))) => {
        pub fn $key<I>(mut self, items: I) -> Self
        where
            I: ::core::iter::IntoIterator,
            I::Item: $crate::outcome::VocabularyToken,
        {
            const {
                ::core::assert!(
                    $crate::outcome::vocabulary_is::<I::Item>(::core::stringify!($vocab)),
                    "the items belong to another vocabulary than the key's"
                )
            };
            $crate::outcome::put_list(
                self.values_mut(),
                ::core::stringify!($key),
                ::core::concat!(::core::stringify!($key), "_total"),
                items.into_iter().map(|item| {
                    ::serde_json::Value::String(
                        $crate::outcome::VocabularyToken::token(item).to_owned(),
                    )
                }),
            );
            self
        }
    };
    (@set $key:ident : list($item:ident)) => {
        pub fn $key<I>(mut self, items: I) -> Self
        where
            I: ::core::iter::IntoIterator,
            I::Item: ::core::convert::Into<::std::string::String>,
        {
            $crate::outcome::put_list(
                self.values_mut(),
                ::core::stringify!($key),
                ::core::concat!(::core::stringify!($key), "_total"),
                items.into_iter().map(|item| {
                    $crate::outcome::text_value(
                        ::core::stringify!($key),
                        outcome_keys!(@kind $item),
                        item.into(),
                    )
                }),
            );
            self
        }
    };
    (@set $key:ident : $text:ident) => {
        pub fn $key(mut self, value: impl ::core::convert::Into<::std::string::String>) -> Self {
            let value = $crate::outcome::text_value(
                ::core::stringify!($key),
                outcome_keys!(@kind $text),
                value.into(),
            );
            self.values_mut().insert(::core::stringify!($key).to_owned(), value);
            self
        }
    };

    ($( $key:ident : $kind:ident $( ( $($arg:tt)* ) )? ),* $(,)?) => {
        /// Every key of the one key table with its type, in table order.
        pub(crate) const KEY_TABLE: &[$crate::outcome::KeySpec] = &[
            $(
                $crate::outcome::KeySpec {
                    name: ::core::stringify!($key),
                    kind: outcome_keys!(@kind $kind $( ($($arg)*) )?),
                },
            )*
        ];

        #[allow(clippy::wrong_self_convention, clippy::should_implement_trait)]
        impl $crate::outcome::Outcome {
            $( outcome_keys!(@outcome $key : $kind $( ($($arg)*) )?); )*
        }

        #[allow(clippy::wrong_self_convention, clippy::should_implement_trait)]
        impl $crate::outcome::Record {
            $( outcome_keys!(@record $key : $kind $( ($($arg)*) )?); )*
        }
    };
}

mod check;
mod keys;
mod vocabulary;

pub use vocabulary::{CauseRelation, IoKind, IoOp, RawSource};

/// The merged outcome catalog `contracts/outcome-codes.json`, embedded at build time. It is
/// generated from the fragments by `outcome-guard merge` (in CI) and never edited by hand.
pub fn catalog() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/outcome-codes.json"
    ))
}

/// The embedded catalog, parsed once on first use and shared by every reader (the debug
/// checks, the MCP `outcome_code` tool, `actingledger codes`).
pub fn catalog_json() -> Result<&'static JsonValue, &'static serde_json::Error> {
    static PARSED: OnceLock<Result<JsonValue, serde_json::Error>> = OnceLock::new();
    PARSED
        .get_or_init(|| serde_json::from_str(catalog()))
        .as_ref()
}

/// The one category of a code, ordered from `success` to `fatal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Done as asked.
    Success,
    /// A neutral fact or state change; nothing to do.
    Info,
    /// Done or continuing, but something was skipped, degraded, left uncertain or caught.
    Warning,
    /// The operation failed; the Runtime, a restart or its operator can recover.
    Error,
    /// The Runtime cannot return to normal operation without an external fix.
    Fatal,
}

impl Category {
    pub const ALL: [Self; 5] = [
        Self::Success,
        Self::Info,
        Self::Warning,
        Self::Error,
        Self::Fatal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Fatal => "fatal",
        }
    }

    /// The process exit code of a top outcome of this category (model section 7.6).
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Success | Self::Info | Self::Warning => 0,
            Self::Error => 1,
            Self::Fatal => 2,
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An emitted code: a registered spelling with its category. Only a registry builds one.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Code {
    name: &'static str,
    category: Category,
}

impl Code {
    /// Used only by [`outcome_codes!`](crate::outcome_codes); the outcome guard (G4) refuses
    /// every other use.
    #[doc(hidden)]
    pub const fn __from_registry(name: &'static str, category: Category) -> Self {
        Self { name, category }
    }

    pub const fn as_str(self) -> &'static str {
        self.name
    }

    pub const fn category(self) -> Category {
        self.category
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.name, self.category)
    }
}

impl fmt::Display for Code {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

/// An emitted location (a value of `operation`, `stage` or `boundary`). Only a registry
/// builds one.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    name: &'static str,
}

impl Location {
    /// Used only by [`outcome_locations!`](crate::outcome_locations); the outcome guard (G4)
    /// refuses every other use.
    #[doc(hidden)]
    pub const fn __from_registry(name: &'static str) -> Self {
        Self { name }
    }

    pub const fn as_str(self) -> &'static str {
        self.name
    }
}

impl fmt::Debug for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

/// A received code: built only by deserialising or from a [`Code`]. An unknown spelling stays
/// readable as it came.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CodeStr(Cow<'static, str>);

impl CodeStr {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Code> for CodeStr {
    fn from(code: Code) -> Self {
        Self(Cow::Borrowed(code.name))
    }
}

impl PartialEq<Code> for CodeStr {
    fn eq(&self, other: &Code) -> bool {
        self.as_str() == other.as_str()
    }
}

impl fmt::Debug for CodeStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for CodeStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for CodeStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CodeStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|text| Self(Cow::Owned(text)))
    }
}

/// A link's relation (a token of the `cause_relation` vocabulary). A received relation that
/// this build does not know stays readable as it came.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Relation(Cow<'static, str>);

impl Relation {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<CauseRelation> for Relation {
    fn from(relation: CauseRelation) -> Self {
        Self(Cow::Borrowed(relation.as_str()))
    }
}

impl PartialEq<CauseRelation> for Relation {
    fn eq(&self, other: &CauseRelation) -> bool {
        self.as_str() == other.as_str()
    }
}

impl fmt::Debug for Relation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for Relation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for Relation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Relation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|text| Self(Cow::Owned(text)))
    }
}

/// A token of a catalog vocabulary. Each vocabulary key's setter accepts only tokens of the
/// key's own vocabulary; a mismatch fails to compile.
pub trait VocabularyToken: Copy {
    /// The catalog vocabulary the token belongs to.
    const VOCABULARY: &'static str;

    fn token(self) -> &'static str;
}

pub(crate) const fn vocabulary_is<T: VocabularyToken>(name: &str) -> bool {
    let left = T::VOCABULARY.as_bytes();
    let right = name.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// A value for an `integer` or `unix_ms` key (an i64 on the wire).
pub trait IntegerValue: Copy {
    #[doc(hidden)]
    fn into_json(self) -> JsonValue;
}

macro_rules! integer_value_within_i64 {
    ($($integer:ty),+) => {
        $(
            impl IntegerValue for $integer {
                fn into_json(self) -> JsonValue {
                    JsonValue::from(self)
                }
            }
        )+
    };
}

integer_value_within_i64!(i8, i16, i32, i64, isize, u8, u16, u32);

macro_rules! integer_value_beyond_i64 {
    ($($integer:ty),+) => {
        $(
            impl IntegerValue for $integer {
                fn into_json(self) -> JsonValue {
                    match i64::try_from(self) {
                        Ok(value) => JsonValue::from(value),
                        Err(_) => {
                            integer_out_of_range();
                            JsonValue::from(self)
                        }
                    }
                }
            }
        )+
    };
}

integer_value_beyond_i64!(u64, usize);

/// A value for a `duration_ms` key, in milliseconds.
pub trait DurationValue: Copy {
    #[doc(hidden)]
    fn into_json(self) -> JsonValue;
}

impl DurationValue for std::time::Duration {
    fn into_json(self) -> JsonValue {
        match i64::try_from(self.as_millis()) {
            Ok(value) => JsonValue::from(value),
            Err(_) => {
                integer_out_of_range();
                JsonValue::from(i64::MAX)
            }
        }
    }
}

impl DurationValue for u64 {
    fn into_json(self) -> JsonValue {
        IntegerValue::into_json(self)
    }
}

impl DurationValue for u32 {
    fn into_json(self) -> JsonValue {
        JsonValue::from(self)
    }
}

impl DurationValue for i64 {
    fn into_json(self) -> JsonValue {
        JsonValue::from(self)
    }
}

fn integer_out_of_range() {
    if cfg!(debug_assertions) {
        panic!("an outcome integer value exceeds the i64 range of the key table");
    }
}

/// The type of one key in the key table (model section 3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Code,
    Location,
    /// A token of the named vocabulary.
    Vocab(&'static str),
    Token,
    Name,
    Id,
    Path,
    Pointer,
    Url,
    Integer,
    DurationMs,
    UnixMs,
    Boolean,
    Hash,
    Commit,
    Version,
    /// `raw_text` only.
    Evidence,
    List(&'static KeyKind),
    /// Flat records; each field is a key, `true` when required.
    Records(&'static [(&'static str, bool)]),
}

impl KeyKind {
    /// Whether codes mode keeps a value of this kind: a registered name (a code, a location or
    /// a vocabulary token), or a list of them.
    pub const fn is_registered_name(self) -> bool {
        match self {
            Self::Code | Self::Location | Self::Vocab(_) => true,
            Self::List(item) => item.is_registered_name(),
            _ => false,
        }
    }
}

/// One key of the key table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub name: &'static str,
    pub kind: KeyKind,
}

/// The one key table, in table order.
pub fn key_table() -> &'static [KeySpec] {
    keys::KEY_TABLE
}

/// The type of a key, if the key table has it.
pub fn key_kind(name: &str) -> Option<KeyKind> {
    keys::KEY_TABLE
        .iter()
        .find(|spec| spec.name == name)
        .map(|spec| spec.kind)
}

/// Whether an interface gets the full outcome or only its registered names (model section 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    /// Codes, categories, relations, and the values whose type is a code, a location or a
    /// vocabulary token.
    Codes,
    /// Everything.
    #[default]
    Full,
}

/// One result: a code with its category, typed values and nested links. The fields sit
/// behind one box, so a `Result<_, Outcome>` (or a module error holding one) stays small.
/// Equality ignores whether the outcome was received from another process.
#[derive(Debug, Clone)]
pub struct Outcome {
    inner: Box<OutcomeInner>,
}

#[derive(Debug, Clone)]
struct OutcomeInner {
    code: CodeStr,
    category: Category,
    values: Map<String, JsonValue>,
    causes: Vec<Link>,
    causes_total: Option<u64>,
    /// Deserialised from another process: relayed unchanged, never checked as an emission.
    received: bool,
}

impl PartialEq for Outcome {
    fn eq(&self, other: &Self) -> bool {
        let (left, right) = (&self.inner, &other.inner);
        left.code == right.code
            && left.category == right.category
            && left.values == right.values
            && left.causes == right.causes
            && left.causes_total == right.causes_total
    }
}

impl Eq for Outcome {}

/// One link of a cause chain: an outcome with its relation to the outcome above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    relation: Relation,
    outcome: Outcome,
}

impl Link {
    pub fn relation(&self) -> &Relation {
        &self.relation
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }
}

/// One flat record of a `records` value. Its fields are keys of the key table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    values: Map<String, JsonValue>,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn values(&self) -> &Map<String, JsonValue> {
        &self.values
    }

    fn values_mut(&mut self) -> &mut Map<String, JsonValue> {
        &mut self.values
    }
}

impl Outcome {
    /// Starts an outcome for an emitted code; add values with the key setters.
    pub fn new(code: impl Into<Code>) -> Self {
        let code = code.into();
        Self::from_inner(OutcomeInner {
            code: CodeStr::from(code),
            category: code.category(),
            values: Map::new(),
            causes: Vec::new(),
            causes_total: None,
            received: false,
        })
    }

    fn from_inner(inner: OutcomeInner) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    fn values_mut(&mut self) -> &mut Map<String, JsonValue> {
        &mut self.inner.values
    }

    /// `foreign_os_error` for a failed OS or file-system call: `raw_source` `os`, `io_kind`,
    /// `io_op`, `os_error` when the OS gave one, and the error's text as `raw_text`.
    ///
    /// Only for an error the OS or std raised. An `io::Error` the Runtime builds itself
    /// (`io::Error::new` or `other` with its own text) gets its own registered code instead.
    pub fn from_io_error(error: &std::io::Error, op: IoOp) -> Self {
        let outcome = Self::new(ContractCode::ForeignOsError)
            .raw_source(RawSource::Os)
            .io_kind(IoKind::from_error_kind(error.kind()))
            .io_op(op);
        let outcome = match error.raw_os_error() {
            Some(number) => outcome.os_error(number),
            None => outcome,
        };
        outcome.raw_text(error.to_string())
    }

    pub fn code(&self) -> &CodeStr {
        &self.inner.code
    }

    pub fn category(&self) -> Category {
        self.inner.category
    }

    /// Whether this outcome carries the given code.
    pub fn is(&self, code: impl Into<Code>) -> bool {
        self.inner.code == code.into()
    }

    pub fn values(&self) -> &Map<String, JsonValue> {
        &self.inner.values
    }

    pub fn value(&self, key: &str) -> Option<&JsonValue> {
        self.inner.values.get(key)
    }

    pub fn causes(&self) -> &[Link] {
        &self.inner.causes
    }

    /// The full number of links when an earlier cut dropped some.
    pub fn causes_total(&self) -> Option<u64> {
        self.inner.causes_total
    }

    /// Attaches the outcome that caused this one.
    pub fn caused_by(self, link: Outcome) -> Self {
        self.link(CauseRelation::CausedBy, link)
    }

    /// Attaches a link with any relation. A received envelope is relayed unchanged by
    /// attaching [`OutcomeEnvelope::into_outcome`].
    pub fn link(mut self, relation: CauseRelation, link: Outcome) -> Self {
        check::debug_assert_link(&link);
        self.inner.causes.push(Link {
            relation: Relation::from(relation),
            outcome: link,
        });
        self
    }

    /// The `actingcommand.outcome.v1` envelope of this outcome, bounded and filtered for the
    /// given detail.
    pub fn envelope(&self, detail: Detail) -> OutcomeEnvelope {
        OutcomeEnvelope::new(self, detail)
    }

    fn put_evidence(&mut self, key: &'static str, truncated_key: &'static str, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        if text.len() <= RAW_TEXT_MAX_BYTES {
            self.inner
                .values
                .insert(key.to_owned(), JsonValue::String(text));
            return;
        }
        diagnostic(self, &text);
        let cut = floor_char_boundary(&text, RAW_TEXT_MAX_BYTES);
        self.inner
            .values
            .insert(key.to_owned(), JsonValue::String(text[..cut].to_owned()));
        self.inner
            .values
            .insert(truncated_key.to_owned(), JsonValue::Bool(true));
    }

    fn link_count(&self) -> u64 {
        self.inner
            .causes
            .iter()
            .map(|link| 1 + link.outcome.link_count())
            .sum()
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.inner.code.as_str())?;
        for (key, value) in &self.inner.values {
            write!(formatter, " {key}=")?;
            match value {
                JsonValue::String(text) => write_escaped(formatter, text)?,
                other => write!(formatter, "{other}")?,
            }
        }
        if !self.inner.causes.is_empty() {
            formatter.write_str(" [")?;
            for (index, link) in self.inner.causes.iter().enumerate() {
                if index > 0 {
                    formatter.write_str("; ")?;
                }
                write!(formatter, "{}: {}", link.relation, link.outcome)?;
            }
            formatter.write_str("]")?;
        }
        if let Some(total) = self.inner.causes_total {
            write!(formatter, " causes_total={total}")?;
        }
        Ok(())
    }
}

/// Writes text with its control characters escaped, so a developer line stays one line.
fn write_escaped(formatter: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    for character in text.chars() {
        if character.is_control() {
            write!(formatter, "{}", character.escape_default())?;
        } else {
            formatter.write_char(character)?;
        }
    }
    Ok(())
}

impl std::error::Error for Outcome {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner
            .causes
            .first()
            .map(|link| &link.outcome as &(dyn std::error::Error + 'static))
    }
}

pub(crate) fn put_name(values: &mut Map<String, JsonValue>, key: &'static str, name: &str) {
    values.insert(key.to_owned(), JsonValue::String(name.to_owned()));
}

pub(crate) fn path_value(key: &'static str, path: &Path) -> JsonValue {
    text_value(key, KeyKind::Path, path.to_string_lossy().into_owned())
}

pub(crate) fn text_value(key: &'static str, kind: KeyKind, text: String) -> JsonValue {
    if cfg!(debug_assertions)
        && let Some(rule) = text_rule_broken(kind, &text)
    {
        panic!("outcome value `{key}` breaks the {rule} rule of the key table: {text:?}");
    }
    JsonValue::String(text)
}

pub(crate) fn put_list(
    values: &mut Map<String, JsonValue>,
    key: &'static str,
    total_key: &'static str,
    items: impl Iterator<Item = JsonValue>,
) {
    let mut kept = Vec::new();
    let mut total: u64 = 0;
    for item in items {
        total += 1;
        if kept.len() < LIST_MAX_ITEMS {
            kept.push(item);
        }
    }
    values.insert(key.to_owned(), JsonValue::Array(kept));
    if total > LIST_MAX_ITEMS as u64 {
        values.insert(total_key.to_owned(), JsonValue::from(total));
    }
}

fn text_rule_broken(kind: KeyKind, text: &str) -> Option<&'static str> {
    let broken = match kind {
        KeyKind::Token => {
            text.is_empty()
                || text.len() > NAME_MAX_BYTES
                || !text.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'.' | b':' | b'-')
                })
        }
        KeyKind::Name | KeyKind::Id => {
            text.len() > NAME_MAX_BYTES || text.chars().any(char::is_control)
        }
        KeyKind::Path => text.len() > PATH_MAX_BYTES,
        KeyKind::Pointer => {
            text.len() > POINTER_MAX_BYTES || !(text.is_empty() || text.starts_with('/'))
        }
        KeyKind::Url => text.len() > URL_MAX_BYTES,
        KeyKind::Hash => !text
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(is_lower_hex)),
        KeyKind::Commit => !(text.len() == 40 && text.bytes().all(is_lower_hex)),
        KeyKind::Version => {
            text.is_empty()
                || text.len() > VERSION_MAX_BYTES
                || !text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
        }
        _ => false,
    };
    broken.then_some(kind_name(kind))
}

fn kind_name(kind: KeyKind) -> &'static str {
    match kind {
        KeyKind::Token => "token",
        KeyKind::Name => "name",
        KeyKind::Id => "id",
        KeyKind::Path => "path",
        KeyKind::Pointer => "pointer",
        KeyKind::Url => "url",
        KeyKind::Hash => "hash",
        KeyKind::Commit => "commit",
        KeyKind::Version => "version",
        _ => "value",
    }
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn floor_char_boundary(text: &str, max: usize) -> usize {
    let mut index = max.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// What receives cut evidence and other developer text: each binary installs one at start.
pub type DiagnosticSink = Box<dyn Fn(&Outcome, &str) + Send + Sync>;

static DIAGNOSTIC_SINK: OnceLock<DiagnosticSink> = OnceLock::new();

/// Why [`install_diagnostic_sink`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSinkError {
    AlreadyInstalled,
}

impl fmt::Display for DiagnosticSinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an outcome diagnostic sink is already installed")
    }
}

impl std::error::Error for DiagnosticSinkError {}

/// Installs the process's diagnostic sink, once.
pub fn install_diagnostic_sink(sink: DiagnosticSink) -> Result<(), DiagnosticSinkError> {
    DIAGNOSTIC_SINK
        .set(sink)
        .map_err(|_| DiagnosticSinkError::AlreadyInstalled)
}

/// Sends developer text about an outcome (for example foreign text cut at its bound) to the
/// installed sink. The contract crate itself never writes to stderr. A call before a sink is
/// installed fails in debug builds.
pub fn diagnostic(outcome: &Outcome, text: &str) {
    match DIAGNOSTIC_SINK.get() {
        Some(sink) => sink(outcome, text),
        None => {
            if cfg!(debug_assertions) {
                panic!("outcome::diagnostic was called for {outcome} before a sink was installed");
            }
        }
    }
}

/// An outcome in its `actingcommand.outcome.v1` interface form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeEnvelope {
    outcome: Outcome,
    detail: Detail,
}

impl OutcomeEnvelope {
    /// Bounds the outcome (32 links, depth 8, 32 KiB) and filters it for the detail.
    pub fn new(outcome: &Outcome, detail: Detail) -> Self {
        check::debug_assert_well_formed(outcome);
        Self {
            outcome: bounded(outcome, detail, ENVELOPE_BODY_BUDGET),
            detail,
        }
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    pub fn into_outcome(self) -> Outcome {
        self.outcome
    }

    pub fn detail(&self) -> Detail {
        self.detail
    }
}

#[derive(Serialize, Deserialize)]
struct WireEnvelope {
    schema_version: String,
    code: CodeStr,
    category: Category,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values: Option<Map<String, JsonValue>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    causes: Vec<WireLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    causes_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<Detail>,
}

#[derive(Serialize, Deserialize)]
struct WireBody {
    code: CodeStr,
    category: Category,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values: Option<Map<String, JsonValue>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    causes: Vec<WireLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    causes_total: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct WireLink {
    code: CodeStr,
    category: Category,
    relation: Relation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values: Option<Map<String, JsonValue>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    causes: Vec<WireLink>,
}

fn wire_values(values: &Map<String, JsonValue>, detail: Detail) -> Option<Map<String, JsonValue>> {
    (detail == Detail::Full || !values.is_empty()).then(|| values.clone())
}

fn wire_links(causes: &[Link], detail: Detail) -> Vec<WireLink> {
    causes
        .iter()
        .map(|link| WireLink {
            code: link.outcome.inner.code.clone(),
            category: link.outcome.inner.category,
            relation: link.relation.clone(),
            values: wire_values(&link.outcome.inner.values, detail),
            causes: wire_links(&link.outcome.inner.causes, detail),
        })
        .collect()
}

fn wire_body(outcome: &Outcome, detail: Detail) -> WireBody {
    WireBody {
        code: outcome.inner.code.clone(),
        category: outcome.inner.category,
        values: wire_values(&outcome.inner.values, detail),
        causes: wire_links(&outcome.inner.causes, detail),
        causes_total: outcome.inner.causes_total,
    }
}

fn received_links(links: Vec<WireLink>) -> Vec<Link> {
    links
        .into_iter()
        .map(|link| Link {
            relation: link.relation,
            outcome: Outcome::from_inner(OutcomeInner {
                code: link.code,
                category: link.category,
                values: link.values.unwrap_or_default(),
                causes: received_links(link.causes),
                causes_total: None,
                received: true,
            }),
        })
        .collect()
}

impl Serialize for OutcomeEnvelope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let body = wire_body(&self.outcome, self.detail);
        WireEnvelope {
            schema_version: OUTCOME_SCHEMA_VERSION.to_owned(),
            code: body.code,
            category: body.category,
            values: body.values,
            causes: body.causes,
            causes_total: body.causes_total,
            detail: (self.detail == Detail::Codes).then_some(Detail::Codes),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OutcomeEnvelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireEnvelope::deserialize(deserializer)?;
        if wire.schema_version != OUTCOME_SCHEMA_VERSION {
            return Err(de::Error::custom(format_args!(
                "unsupported outcome schema_version {}",
                wire.schema_version
            )));
        }
        let detail = wire.detail.unwrap_or_default();
        let values = match (wire.values, detail) {
            (Some(values), _) => values,
            (None, Detail::Codes) => Map::new(),
            (None, Detail::Full) => return Err(de::Error::missing_field("values")),
        };
        Ok(Self {
            outcome: Outcome::from_inner(OutcomeInner {
                code: wire.code,
                category: wire.category,
                values,
                causes: received_links(wire.causes),
                causes_total: wire.causes_total,
                received: true,
            }),
            detail,
        })
    }
}

/// The record form (the envelope without `schema_version` and `detail`), always full.
impl Serialize for Outcome {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        check::debug_assert_well_formed(self);
        wire_body(
            &bounded(self, Detail::Full, ENVELOPE_BODY_BUDGET),
            Detail::Full,
        )
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Outcome {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireBody::deserialize(deserializer)?;
        let Some(values) = wire.values else {
            return Err(de::Error::missing_field("values"));
        };
        Ok(Self::from_inner(OutcomeInner {
            code: wire.code,
            category: wire.category,
            values,
            causes: received_links(wire.causes),
            causes_total: wire.causes_total,
            received: true,
        }))
    }
}

/// A copy that keeps at most 32 links within depth 8 (breadth first), filtered for the detail,
/// and cut to the byte budget: `raw_text` goes first from the deepest link up (to the sink),
/// then the deepest links. Any cut sets `causes_total` to the full link count.
fn bounded(outcome: &Outcome, detail: Detail, budget: usize) -> Outcome {
    let total = outcome.link_count();
    let order = breadth_first(outcome);
    let keep = order
        .iter()
        .filter(|path| path.len() <= CAUSES_MAX_DEPTH)
        .take(CAUSES_MAX_LINKS)
        .cloned()
        .collect::<Vec<_>>();
    let mut cut = keep.len() < order.len();
    let mut result = rebuilt(outcome, &mut Vec::new(), &keep, detail);
    loop {
        let size =
            serde_json::to_vec(&wire_body(&result, detail)).map_or(usize::MAX, |bytes| bytes.len());
        if size <= budget {
            break;
        }
        if cut_deepest_raw_text(&mut result) {
            continue;
        }
        if drop_deepest_link(&mut result) {
            cut = true;
            continue;
        }
        break;
    }
    if cut {
        result.inner.causes_total = Some(outcome.inner.causes_total.unwrap_or(0).max(total));
    }
    result
}

/// Every link's path (child indexes from the top), breadth first.
fn breadth_first(outcome: &Outcome) -> Vec<Vec<usize>> {
    let mut order = Vec::new();
    let mut queue = VecDeque::new();
    for (index, link) in outcome.inner.causes.iter().enumerate() {
        queue.push_back((vec![index], &link.outcome));
    }
    while let Some((path, node)) = queue.pop_front() {
        for (index, link) in node.inner.causes.iter().enumerate() {
            let mut child = path.clone();
            child.push(index);
            queue.push_back((child, &link.outcome));
        }
        order.push(path);
    }
    order
}

fn rebuilt(
    outcome: &Outcome,
    path: &mut Vec<usize>,
    keep: &[Vec<usize>],
    detail: Detail,
) -> Outcome {
    let values = match detail {
        Detail::Full => outcome.inner.values.clone(),
        Detail::Codes => outcome
            .inner
            .values
            .iter()
            .filter(|(key, _)| key_kind(key).is_some_and(KeyKind::is_registered_name))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    };
    let mut causes = Vec::new();
    for (index, link) in outcome.inner.causes.iter().enumerate() {
        path.push(index);
        if keep.iter().any(|kept| kept.as_slice() == path.as_slice()) {
            causes.push(Link {
                relation: link.relation.clone(),
                outcome: rebuilt(&link.outcome, path, keep, detail),
            });
        }
        path.pop();
    }
    Outcome::from_inner(OutcomeInner {
        code: outcome.inner.code.clone(),
        category: outcome.inner.category,
        values,
        causes,
        causes_total: outcome.inner.causes_total,
        received: outcome.inner.received,
    })
}

fn node_mut<'a>(outcome: &'a mut Outcome, path: &[usize]) -> &'a mut Outcome {
    let mut node = outcome;
    for index in path {
        node = &mut node.inner.causes[*index].outcome;
    }
    node
}

fn cut_deepest_raw_text(outcome: &mut Outcome) -> bool {
    let mut paths = vec![Vec::new()];
    paths.extend(breadth_first(outcome));
    for path in paths.iter().rev() {
        let node = node_mut(outcome, path);
        let text = match node.inner.values.get(RAW_TEXT_KEY) {
            Some(JsonValue::String(text)) if !text.is_empty() => text.clone(),
            _ => continue,
        };
        diagnostic(node, &text);
        node.inner
            .values
            .insert(RAW_TEXT_KEY.to_owned(), JsonValue::String(String::new()));
        node.inner
            .values
            .insert(RAW_TEXT_TRUNCATED_KEY.to_owned(), JsonValue::Bool(true));
        return true;
    }
    false
}

fn drop_deepest_link(outcome: &mut Outcome) -> bool {
    let Some(path) = breadth_first(outcome).pop() else {
        return false;
    };
    let Some((index, parent)) = path.split_last() else {
        return false;
    };
    node_mut(outcome, parent).causes.remove(*index);
    true
}

#[cfg(test)]
mod tests;
