// SPDX-License-Identifier: AGPL-3.0-only

//! The `store_foreground` section (STORE-FG-2, design `rd-1029/design-store-fg.md` §2.2, §3;
//! Alice's ruling of 2026-10-29 10:1x: a client update comes only from the official app store,
//! as a pack of its own): the store applications a store update package may tap in.
//!
//! - `allowed_application_ids`: a list of 0 to [`MAX_ALLOWED_APPLICATION_IDS`] unique Android
//!   package names ([`actingcommand_contract::android_package_id_valid`]), default `[]` from the
//!   defaults file. A package's `control.json` `store_update` names the store its steps are
//!   written for; its run starts only when that store is in this list, and each of its store
//!   taps checks the list again (`application_store_not_allowed`,
//!   `contracts/application-lifecycle.md`, "Store foreground allowance").
//!
//! The list is the operator's: no instruction changes it while the Runtime runs
//! (`config_key_not_settable`), so no agent can widen it; the configuration file changes it at
//! the next start. The Runtime compares package ids for equality only and names no store.
//!
//! Refusals, each `config_value_invalid` naming the key: a value that is no list, or an entry
//! that is no string, `wrong_type`; more than [`MAX_ALLOWED_APPLICATION_IDS`] entries, or an
//! entry that is no Android package name, `out_of_range`; an entry listed twice, or (actingd's
//! assembly, [`StoreForegroundConfig::refuse_instance_applications`]) an entry equal to a
//! configured instance's `application_id` (a game is never its own store), `section_check`.

use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::{FactScalar, android_package_id_valid};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "store_foreground";
/// The store applications a store update package may tap in.
pub const ALLOWED_APPLICATION_IDS: &str = "store_foreground.allowed_application_ids";
/// The most store applications the list names. Schema.
pub const MAX_ALLOWED_APPLICATION_IDS: usize = 16;

/// The section as the configuration file and the defaults file write it: every member may be
/// left out and none is unknown (M387 §5.3).
// Interface anchor `actingd-config` revision 21 (contracts/component-interfaces.md): a Runtime
// before STORE-FG-2 refuses a file that names the section, because the file denies unknown
// members; a file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoreForegroundFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allowed_application_ids: Option<Value>,
}

/// One row per key of the section.
pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact(ALLOWED_APPLICATION_IDS),
    file_path: FilePath::Key,
    value_type: ValueType::List,
    range: KeyRange::Any,
    default: DefaultSource::File,
    // Design §2.2 (Alice's question 95, option A): only a person widens the list, in the
    // configuration file, and the Runtime takes it at its next start.
    settable: Settable::No(NotSettable::ReadAtAssembly),
    reader: "STORE-FG-2",
    since: "0.12.29",
}];

/// The `store_foreground` section in effect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreForegroundConfig {
    /// `store_foreground.allowed_application_ids`, in the file's order.
    pub allowed_application_ids: Vec<String>,
}

fn invalid(problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::invalid_value(ALLOWED_APPLICATION_IDS, problem)
}

impl StoreForegroundConfig {
    /// The section from `value` (the configuration in effect), checked whole (module doc).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let Some(FactScalar::String(text)) = value(ALLOWED_APPLICATION_IDS) else {
            return Err(invalid(ConfigValueProblem::WrongType));
        };
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
            return Err(invalid(ConfigValueProblem::WrongType));
        };
        let ids = items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(ConfigValueProblem::WrongType))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if ids.len() > MAX_ALLOWED_APPLICATION_IDS
            || ids.iter().any(|id| !android_package_id_valid(id))
        {
            return Err(invalid(ConfigValueProblem::OutOfRange));
        }
        let unique = ids.iter().collect::<BTreeSet<_>>();
        if unique.len() != ids.len() {
            return Err(invalid(ConfigValueProblem::SectionCheck));
        }
        Ok(Self {
            allowed_application_ids: ids,
        })
    }

    /// Whether a store update package may tap in `store_application_id`: the list names it
    /// exactly.
    pub fn allows(&self, store_application_id: &str) -> bool {
        self.allowed_application_ids
            .iter()
            .any(|allowed| allowed == store_application_id)
    }

    /// actingd's assembly (design §2.2): no entry is a configured instance's `application_id`,
    /// since a game is never its own store; one that is refuses the start and check-config as
    /// `config_value_invalid` (`section_check`) naming the key.
    pub fn refuse_instance_applications<'a>(
        &self,
        application_ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), ConfigFailure> {
        if application_ids.into_iter().any(|id| self.allows(id)) {
            return Err(invalid(ConfigValueProblem::SectionCheck));
        }
        Ok(())
    }
}
