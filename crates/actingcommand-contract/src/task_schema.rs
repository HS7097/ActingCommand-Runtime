// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-5a (model 392 v1.2, section 0.2 C12 and section 10.2): the task schema of a
//! `task.json` (its `schema_version`) as one ordered type. A member a task schema introduces is
//! admitted from that schema on, `TaskSchema::at_least(version, TaskSchema::V0_9)`, and a rule
//! that differs per schema matches on [`TaskSchema::of`]; neither compares version strings. This
//! file is the one place the task schema strings are written.

/// A task schema, oldest first: each compares greater than every schema before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskSchema {
    V0_3,
    V0_4,
    V0_5,
    V0_6,
    V0_7,
    V0_8,
    V0_9,
}

impl TaskSchema {
    /// Every task schema, oldest first.
    pub const ALL: [Self; 7] = [
        Self::V0_3,
        Self::V0_4,
        Self::V0_5,
        Self::V0_6,
        Self::V0_7,
        Self::V0_8,
        Self::V0_9,
    ];

    /// The `schema_version` that names this schema.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_3 => "0.3",
            Self::V0_4 => "0.4",
            Self::V0_5 => "0.5",
            Self::V0_6 => "0.6",
            Self::V0_7 => "0.7",
            Self::V0_8 => "0.8",
            Self::V0_9 => "0.9",
        }
    }

    /// The task schema `version` names exactly; `None` when it is absent or names none.
    pub fn of(version: impl Into<Option<&str>>) -> Option<Self> {
        let version = version.into()?;
        Self::ALL
            .into_iter()
            .find(|schema| schema.as_str() == version)
    }

    /// Whether `version` names `minimum` or a later task schema. An absent version, or one that
    /// names no task schema, is never at least any.
    pub fn at_least(version: impl Into<Option<&str>>, minimum: Self) -> bool {
        Self::of(version).is_some_and(|schema| schema >= minimum)
    }
}
