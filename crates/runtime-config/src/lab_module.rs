// SPDX-License-Identifier: AGPL-3.0-only

//! The two Lab options (Workflow #389, unified interface model v2.2.3 §4.1; slice U-2): the
//! top-level booleans `lab_module_authoring` (option one, authoring) and `lab_module_debugging`
//! (option two, debugging). Absent means `false`: the Lab is not installed by default, and it is
//! installed when at least one is `true`. Only the installer's Lab transaction writes them, so
//! no instruction changes them while the Runtime runs; actingd reads them once at start and
//! reports them in `config.parameters.top_level`. From v0.12.0 to U-6 they are read and
//! reported only: nothing is refused by them.

use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};

/// Option one, authoring.
pub const AUTHORING: &str = "lab_module_authoring";
/// Option two, debugging.
pub const DEBUGGING: &str = "lab_module_debugging";

const fn row(name: &'static str) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type: ValueType::Boolean,
        range: KeyRange::Any,
        // `false` in the defaults file: not installed unless the installer ticked it.
        default: DefaultSource::File,
        // Only acsetup's Lab transaction changes them (model §4.1): not in #387 D1's set.
        settable: Settable::No(NotSettable::ReadAtAssembly),
        reader: "U-2",
        since: "0.12.0",
    }
}

pub(crate) static ROWS: &[KeyRow] = &[row(AUTHORING), row(DEBUGGING)];
