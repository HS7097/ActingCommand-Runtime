// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #390 OUT-2 (forced compromises O-01, O-02, R-03, S-01): a task's designated effect
//! set. A task schema `0.9` package may name several designated operations in
//! `scheduling_outcome.designated_effects`, each with an explicit repeat cap: a constant, or the
//! integer value of an instance fact read once before the run's first step. An effect that
//! reached its cap is not selected again; the page's next operation runs instead, so a package
//! can loop over a counted business step and leave the loop. The terminal's scheduling
//! disposition records how often each effect of the set completed, so "done n times" and "there
//! was nothing to do" are told apart.

use super::{SanitizationError, SchedulingEffectCondition, validate_task_semantic_label};
use crate::codes::SanitizationCode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most designated effects one set names.
pub const MAX_DESIGNATED_EFFECTS: usize = 8;
/// The highest repeat cap of a designated effect: each completion takes at least one step, and
/// a task takes at most 1,000 steps.
pub const MAX_DESIGNATED_EFFECT_REPEATS: u32 = 1_000;

/// One member of `scheduling_outcome.designated_effects`: an operation of the task whose effect
/// is a designated effect, and how often it may complete in one run (absent: once).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignatedEffectDeclaration {
    operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_repeats: Option<DesignatedEffectRepeats>,
}

impl DesignatedEffectDeclaration {
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub const fn max_repeats(&self) -> Option<&DesignatedEffectRepeats> {
        self.max_repeats.as_ref()
    }

    /// The fact key a cap is read from, when the cap is not a constant.
    pub fn cap_fact_key(&self) -> Option<&str> {
        match &self.max_repeats {
            Some(DesignatedEffectRepeats::Fact(fact)) => Some(fact.fact_key.as_str()),
            _ => None,
        }
    }
}

/// How often a designated effect may complete in one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesignatedEffectRepeats {
    /// A constant from 1 to [`MAX_DESIGNATED_EFFECT_REPEATS`].
    Constant(u32),
    /// The value of an instance fact: an unexpired integer from 0 to
    /// [`MAX_DESIGNATED_EFFECT_REPEATS`], read once before the run's first step.
    Fact(DesignatedEffectRepeatsFact),
}

/// `{"fact_key": <instance fact key>}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignatedEffectRepeatsFact {
    fact_key: String,
}

impl DesignatedEffectRepeatsFact {
    pub fn fact_key(&self) -> &str {
        &self.fact_key
    }
}

/// The checks of a `designated_effects` set: 1 to [`MAX_DESIGNATED_EFFECTS`] members, each
/// naming its operation once, a constant cap from 1 to [`MAX_DESIGNATED_EFFECT_REPEATS`], a fact
/// cap under a valid fact key.
pub(super) fn validate_designated_effects(
    effects: &[DesignatedEffectDeclaration],
) -> Result<(), SanitizationError> {
    let invalid = |field| {
        SanitizationError::new(SanitizationCode::InvalidSchedulingOutcomeDeclaration, field)
    };
    if effects.is_empty() || effects.len() > MAX_DESIGNATED_EFFECTS {
        return Err(invalid("designated_effects"));
    }
    let mut operations = BTreeSet::new();
    for effect in effects {
        validate_task_semantic_label(&effect.operation_id, "designated_effects")?;
        if !operations.insert(effect.operation_id.as_str()) {
            return Err(invalid("designated_effects"));
        }
        match &effect.max_repeats {
            None => {}
            Some(DesignatedEffectRepeats::Constant(cap)) => {
                if !(1..=MAX_DESIGNATED_EFFECT_REPEATS).contains(cap) {
                    return Err(invalid("max_repeats"));
                }
            }
            Some(DesignatedEffectRepeats::Fact(fact)) => {
                crate::fact::validate_fact_key(&fact.fact_key)?;
            }
        }
    }
    Ok(())
}

/// How often one designated effect of a set completed in a run (`count`), and the cap the run
/// held it to (`max_repeats`: the constant, or the fact's value read before the first step).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignatedEffectTally {
    operation_label: String,
    count: u32,
    max_repeats: u32,
}

impl DesignatedEffectTally {
    pub fn new(
        operation_label: impl Into<String>,
        count: u32,
        max_repeats: u32,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            operation_label: operation_label.into(),
            count,
            max_repeats,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn operation_label(&self) -> &str {
        &self.operation_label
    }

    pub const fn count(&self) -> u32 {
        self.count
    }

    pub const fn max_repeats(&self) -> u32 {
        self.max_repeats
    }

    fn validate(&self) -> Result<(), SanitizationError> {
        validate_task_semantic_label(&self.operation_label, "operation_label")?;
        if self.max_repeats > MAX_DESIGNATED_EFFECT_REPEATS || self.count > self.max_repeats {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                "designated_effects",
            ));
        }
        Ok(())
    }
}

/// The tallies of a set's run: one per member, in declaration order, each operation once.
pub(super) fn validate_designated_effect_tallies(
    effects: &[DesignatedEffectTally],
) -> Result<(), SanitizationError> {
    if effects.is_empty() || effects.len() > MAX_DESIGNATED_EFFECTS {
        return Err(SanitizationError::new(
            SanitizationCode::InvalidSchedulingOutcomeDeclaration,
            "designated_effects",
        ));
    }
    let mut operations = BTreeSet::new();
    for effect in effects {
        effect.validate()?;
        if !operations.insert(effect.operation_label.as_str()) {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidSchedulingOutcomeDeclaration,
                "designated_effects",
            ));
        }
    }
    Ok(())
}

/// The mapping condition of a set's run: a designated effect completed when any member did.
pub(super) fn tallies_condition(effects: &[DesignatedEffectTally]) -> SchedulingEffectCondition {
    if effects.iter().any(|effect| effect.count > 0) {
        SchedulingEffectCondition::DesignatedEffectCompleted
    } else {
        SchedulingEffectCondition::NoDesignatedEffect
    }
}

/// A declaration's designated effects as the static reachability checks of a page graph and a
/// phased route see them (Workflow #390 OUT-2). The members are the one `designated_operation`
/// or the members of `designated_effects`, in declaration order; a check follows the set of
/// members that completed at least once on the way to a page as a bit mask.
#[derive(Debug, Clone, Copy)]
pub struct DesignatedEffectReach<'a> {
    declaration: &'a super::SchedulingOutcomeDeclaration,
}

impl<'a> DesignatedEffectReach<'a> {
    pub(super) const fn new(declaration: &'a super::SchedulingOutcomeDeclaration) -> Self {
        Self { declaration }
    }

    /// The member index of `operation_id`, when it is a designated operation.
    pub fn index_of(&self, operation_id: &str) -> Option<usize> {
        match (
            self.declaration.designated_operation(),
            self.declaration.designated_effects(),
        ) {
            (Some(designated), _) => (designated == operation_id).then_some(0),
            (None, Some(effects)) => effects
                .iter()
                .position(|effect| effect.operation_id() == operation_id),
            (None, None) => None,
        }
    }

    /// The bit of member `index` in a mask of completed members.
    pub const fn bit(index: usize) -> u16 {
        1 << index
    }

    /// Whether member `index` is the one `designated_operation`, which completes at most once
    /// and must not be reachable again after its effect.
    pub fn is_single(&self, index: usize) -> bool {
        index == 0 && self.declaration.designated_operation().is_some()
    }

    /// Whether member `index` of a set may already have reached its cap on a path on which the
    /// members in `completed` completed at least once: a fact cap may be 0, a constant cap only
    /// after the member completed.
    pub fn may_be_capped(&self, index: usize, completed: u16) -> bool {
        let Some(effect) = self
            .declaration
            .designated_effects()
            .and_then(|effects| effects.get(index))
        else {
            return false;
        };
        effect.cap_fact_key().is_some() || completed & Self::bit(index) != 0
    }

    /// The mapping condition of a path on which the members in `completed` completed.
    pub const fn condition(completed: u16) -> SchedulingEffectCondition {
        if completed == 0 {
            SchedulingEffectCondition::NoDesignatedEffect
        } else {
            SchedulingEffectCondition::DesignatedEffectCompleted
        }
    }
}
