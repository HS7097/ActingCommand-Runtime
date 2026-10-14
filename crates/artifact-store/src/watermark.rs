// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-2 (`contracts/ledger-observer.md` §7.1): the pure arithmetic of a three-tier
//! watermark with hysteresis, shared by the frame store and the ledger's capture queue. It sits
//! outside the `capture` feature, so the ledger (which depends on this crate with
//! `default-features = false`) can use it.
//!
//! A tier's activation line is `floor(budget × ratio)`; its release line is
//! `floor(activation × (1 − hysteresis))`. A tier activates when the amount in use reaches its
//! activation line and is released only when the amount falls to its release line or below.

/// The three tiers' ratios of a budget and the hysteresis of their release lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatermarkRatios {
    pub tier1: f64,
    pub tier2: f64,
    pub tier3: f64,
    pub hysteresis: f64,
}

/// What breaks a watermark configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatermarkConfigError {
    /// A ratio is not a finite number, the tiers do not rise strictly within (0, 1], or the
    /// hysteresis lies outside (0, 1).
    RatiosInvalid,
    /// Two tiers fall on the same amount of the budget.
    LinesNotDistinct,
    /// A release line is not below its activation line.
    ReleaseNotBelowActivation,
    /// Tier 3 − tier 2 of the budget is smaller than the gap the user needs.
    GapTooSmall,
}

/// The activation and release lines of the three tiers for one budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatermarkLines {
    pub tier1: u64,
    pub tier2: u64,
    pub tier3: u64,
    pub tier1_release: u64,
    pub tier2_release: u64,
    pub tier3_release: u64,
}

/// A watermark's tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WatermarkTier {
    #[default]
    Below,
    Tier1,
    Tier2,
    Tier3,
}

/// `floor(amount × ratio)`, the frame store's arithmetic.
pub fn watermark_amount(amount: u64, ratio: f64) -> u64 {
    ((amount as f64) * ratio).floor() as u64
}

impl WatermarkRatios {
    /// The lines of the three tiers for `budget`.
    pub fn lines(&self, budget: u64) -> WatermarkLines {
        let tier1 = watermark_amount(budget, self.tier1);
        let tier2 = watermark_amount(budget, self.tier2);
        let tier3 = watermark_amount(budget, self.tier3);
        let release = 1.0 - self.hysteresis;
        WatermarkLines {
            tier1,
            tier2,
            tier3,
            tier1_release: watermark_amount(tier1, release),
            tier2_release: watermark_amount(tier2, release),
            tier3_release: watermark_amount(tier3, release),
        }
    }

    /// tier1 < tier2 < tier3 <= 1, all above 0, the hysteresis within (0, 1); for `budget` each
    /// line distinct, each release line below its activation line and tier 3 − tier 2 at least
    /// `min_gap` (the frame store's flush workspace, the ledger's spill batch).
    pub fn validate(&self, budget: u64, min_gap: u64) -> Result<(), WatermarkConfigError> {
        let finite = [self.tier1, self.tier2, self.tier3, self.hysteresis]
            .iter()
            .all(|ratio| ratio.is_finite());
        if !finite
            || self.tier1 <= 0.0
            || self.tier1 >= self.tier2
            || self.tier2 >= self.tier3
            || self.tier3 > 1.0
            || self.hysteresis <= 0.0
            || self.hysteresis >= 1.0
        {
            return Err(WatermarkConfigError::RatiosInvalid);
        }
        self.lines(budget).validate(min_gap)
    }
}

impl WatermarkLines {
    /// Each line distinct, each release line below its activation line, and tier 3 − tier 2
    /// at least `min_gap`.
    pub fn validate(&self, min_gap: u64) -> Result<(), WatermarkConfigError> {
        if !(self.tier1 < self.tier2 && self.tier2 < self.tier3) {
            return Err(WatermarkConfigError::LinesNotDistinct);
        }
        if self.tier1_release >= self.tier1
            || self.tier2_release >= self.tier2
            || self.tier3_release >= self.tier3
        {
            return Err(WatermarkConfigError::ReleaseNotBelowActivation);
        }
        if self.tier3.saturating_sub(self.tier2) < min_gap {
            return Err(WatermarkConfigError::GapTooSmall);
        }
        Ok(())
    }

    fn activation(&self, tier: WatermarkTier) -> u64 {
        match tier {
            WatermarkTier::Below => 0,
            WatermarkTier::Tier1 => self.tier1,
            WatermarkTier::Tier2 => self.tier2,
            WatermarkTier::Tier3 => self.tier3,
        }
    }

    fn release(&self, tier: WatermarkTier) -> u64 {
        match tier {
            WatermarkTier::Below => 0,
            WatermarkTier::Tier1 => self.tier1_release,
            WatermarkTier::Tier2 => self.tier2_release,
            WatermarkTier::Tier3 => self.tier3_release,
        }
    }

    /// The tier for `used`, given the previous tier: a tier above the previous one activates
    /// when `used` reaches its activation line; the previous tier and those below it are
    /// released only when `used` falls to their release line or below.
    pub fn tier(&self, used: u64, previous: WatermarkTier) -> WatermarkTier {
        let mut tier = previous;
        while tier != WatermarkTier::Below && used <= self.release(tier) {
            tier = lower(tier);
        }
        while let Some(next) = higher(tier) {
            if used < self.activation(next) {
                break;
            }
            tier = next;
        }
        tier
    }
}

fn lower(tier: WatermarkTier) -> WatermarkTier {
    match tier {
        WatermarkTier::Below | WatermarkTier::Tier1 => WatermarkTier::Below,
        WatermarkTier::Tier2 => WatermarkTier::Tier1,
        WatermarkTier::Tier3 => WatermarkTier::Tier2,
    }
}

fn higher(tier: WatermarkTier) -> Option<WatermarkTier> {
    match tier {
        WatermarkTier::Below => Some(WatermarkTier::Tier1),
        WatermarkTier::Tier1 => Some(WatermarkTier::Tier2),
        WatermarkTier::Tier2 => Some(WatermarkTier::Tier3),
        WatermarkTier::Tier3 => None,
    }
}

/// The tier for `used` of `budget`, given the previous tier (§7.1).
pub fn watermark_tier(
    ratios: &WatermarkRatios,
    used: u64,
    budget: u64,
    previous: WatermarkTier,
) -> WatermarkTier {
    ratios.lines(budget).tier(used, previous)
}
