// SPDX-License-Identifier: AGPL-3.0-only

//! Bounded business identity mapping. Physical slot IDs remain layout coordinates.

use actingcommand_contract::candidate_projection::{
    CandidateFeature, CandidateIdentitySource, CandidateUnknownReason,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_IDENTITY_ENTRIES: usize = 128;
pub const MAX_IDENTITY_ALIASES: usize = 8;
pub const MAX_IDENTITY_TEXT_CHARS: usize = 128;
pub const MAX_IDENTITY_TEMPLATES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateIdentityDeclaration {
    pub entries: Vec<CandidateIdentityEntry>,
    pub recognition: CandidateIdentityRecognition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateIdentityEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateIdentityRecognition {
    OcrAliases {
        max_distance: u16,
        minimum_margin: u16,
        minimum_confidence_milli: u16,
        /// Simultaneous single-character substitutions after width/case/space normalization.
        #[serde(default)]
        confusions: BTreeMap<String, String>,
    },
    IconTemplates {
        minimum_score_milli: u16,
        minimum_margin_milli: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateIdentityTemplate {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub target_id: String,
}

impl CandidateIdentityDeclaration {
    pub fn domain(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn source(&self) -> CandidateIdentitySource {
        match self.recognition {
            CandidateIdentityRecognition::OcrAliases { .. } => CandidateIdentitySource::OcrAlias,
            CandidateIdentityRecognition::IconTemplates { .. } => {
                CandidateIdentitySource::IconTemplate
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_IDENTITY_ENTRIES).contains(&self.entries.len()) {
            return Err(format!(
                "identity entries must contain 1..={MAX_IDENTITY_ENTRIES} rows"
            ));
        }
        let mut keys = BTreeSet::new();
        for entry in &self.entries {
            if !valid_identity_name(&entry.id)
                || entry
                    .variant
                    .as_ref()
                    .is_some_and(|variant| !valid_identity_name(variant))
                || !keys.insert((&entry.id, &entry.variant))
                || entry.aliases.len() > MAX_IDENTITY_ALIASES
            {
                return Err("identity IDs/variants must be bounded and unique; alias count exceeds its bound".into());
            }
            if let CandidateIdentityRecognition::OcrAliases { confusions, .. } = &self.recognition {
                if entry.aliases.is_empty() {
                    return Err(format!("OCR identity '{}' has no alias", entry.id));
                }
                for alias in &entry.aliases {
                    if normalize_name(alias, confusions).is_none() {
                        return Err(format!(
                            "identity '{}' has an empty or overlong alias",
                            entry.id
                        ));
                    }
                }
            }
        }
        match &self.recognition {
            CandidateIdentityRecognition::OcrAliases {
                max_distance,
                minimum_margin,
                minimum_confidence_milli,
                confusions,
            } => {
                if *max_distance > 8
                    || !(1..=9).contains(minimum_margin)
                    || !(1..=1000).contains(minimum_confidence_milli)
                    || confusions.len() > 32
                    || confusions.iter().any(|(from, to)| {
                        from.chars().count() != 1
                            || to.chars().count() != 1
                            || from
                                .chars()
                                .chain(to.chars())
                                .any(|c| c.is_control() || c.is_whitespace())
                    })
                {
                    return Err("invalid bounded OCR identity distance, margin, confidence or confusion map".into());
                }
            }
            CandidateIdentityRecognition::IconTemplates {
                minimum_score_milli,
                minimum_margin_milli,
            } => {
                if !(1..=1000).contains(minimum_score_milli)
                    || !(1..=1000).contains(minimum_margin_milli)
                {
                    return Err("invalid icon identity score or margin".into());
                }
            }
        }
        Ok(())
    }

    pub fn map_ocr(&self, text: &str, confidence: Option<u16>) -> CandidateFeature {
        let CandidateIdentityRecognition::OcrAliases {
            max_distance,
            minimum_margin,
            minimum_confidence_milli,
            confusions,
        } = &self.recognition
        else {
            return self.unknown(CandidateUnknownReason::OutOfDomain);
        };
        if confidence.is_none_or(|confidence| confidence < *minimum_confidence_milli) {
            return self.unknown(CandidateUnknownReason::LowConfidence);
        }
        let Some(text) = normalize_name(text, confusions) else {
            return self.unknown(CandidateUnknownReason::OutOfDomain);
        };
        // The runner-up needs max_distance + margin precision; saturated distances beyond
        // that cannot alter admission. Each DP row contains at most 129 cells.
        let limit = max_distance + minimum_margin;
        let mut rows = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let distance = entry
                .aliases
                .iter()
                .filter_map(|alias| normalize_name(alias, confusions))
                .map(|alias| edit_distance(&text, &alias, limit))
                .min()
                .unwrap_or(limit + 1);
            rows.push((entry.id.as_str(), entry.variant.as_deref(), distance));
        }
        self.choose(
            &rows,
            *max_distance,
            *minimum_margin,
            confidence.map(i64::from),
        )
    }

    /// Scores are normalized integer milli, in the slot's template declaration order.
    pub fn map_icons(
        &self,
        templates: &[CandidateIdentityTemplate],
        scores: &[u16],
    ) -> CandidateFeature {
        let CandidateIdentityRecognition::IconTemplates {
            minimum_score_milli,
            minimum_margin_milli,
        } = self.recognition
        else {
            return self.unknown(CandidateUnknownReason::OutOfDomain);
        };
        if templates.len() != scores.len()
            || templates.is_empty()
            || templates.len() > MAX_IDENTITY_TEMPLATES
            || scores.iter().any(|score| *score > 1000)
        {
            return self.unknown(CandidateUnknownReason::OutOfDomain);
        }
        let rows = templates
            .iter()
            .zip(scores)
            .map(|(template, score)| {
                (
                    template.id.as_str(),
                    template.variant.as_deref(),
                    1000 - score,
                )
            })
            .collect::<Vec<_>>();
        let confidence = scores.iter().max().copied().map(i64::from);
        self.choose(
            &rows,
            1000 - minimum_score_milli,
            minimum_margin_milli,
            confidence,
        )
    }

    fn choose(
        &self,
        rows: &[(&str, Option<&str>, u16)],
        maximum: u16,
        margin: u16,
        confidence: Option<i64>,
    ) -> CandidateFeature {
        let mut by_id = BTreeMap::<&str, (u16, Option<&str>, bool)>::new();
        for &(id, variant, distance) in rows {
            if !self
                .entries
                .iter()
                .any(|entry| entry.id == id && entry.variant.as_deref() == variant)
            {
                return self.unknown(CandidateUnknownReason::OutOfDomain);
            }
            let best = by_id.entry(id).or_insert((distance, variant, false));
            if distance < best.0 {
                *best = (distance, variant, false);
            } else if distance == best.0 && variant != best.1 {
                best.2 = true;
            }
        }
        let mut ranked = by_id.into_iter().collect::<Vec<_>>();
        ranked.sort_by_key(|(id, (distance, _, _))| (*distance, *id));
        let Some(&(id, (distance, variant, variant_tie))) = ranked.first() else {
            return self.unknown(CandidateUnknownReason::Missing);
        };
        if distance > maximum {
            return self.unknown(CandidateUnknownReason::LowConfidence);
        }
        if variant_tie
            || ranked
                .get(1)
                .is_some_and(|(_, (runner_up, _, _))| *runner_up < distance + margin)
        {
            return self.unknown(CandidateUnknownReason::Ambiguous);
        }
        CandidateFeature::Identity {
            value: id.to_owned(),
            variant: variant.map(str::to_owned),
            source: self.source(),
            distance,
            confidence,
        }
    }

    fn unknown(&self, reason: CandidateUnknownReason) -> CandidateFeature {
        CandidateFeature::Unknown {
            reason,
            source: Some(self.source()),
        }
    }
}

pub(crate) fn valid_identity_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

/// Width-fold ASCII and halfwidth katakana, compose voiced kana, lowercase and remove
/// Unicode whitespace. Explicit confusion substitutions are simultaneous and nonrecursive.
pub fn normalize_name(text: &str, confusions: &BTreeMap<String, String>) -> Option<String> {
    if text.len() > MAX_IDENTITY_TEXT_CHARS * 4 || text.chars().count() > MAX_IDENTITY_TEXT_CHARS {
        return None;
    }
    const KANA: &str = "。「」、・ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン\u{3099}\u{309a}";
    let kana = KANA.chars().collect::<Vec<_>>();
    let mut normalized = String::new();
    for original in text.chars() {
        let c = match original as u32 {
            0xff01..=0xff5e => char::from_u32(original as u32 - 0xfee0)?,
            0xff61..=0xff9f => kana[(original as u32 - 0xff61) as usize],
            _ => original,
        };
        if c.is_whitespace() {
            continue;
        }
        if c == '\u{3099}' || c == '\u{309a}' {
            let previous = normalized.pop()?;
            let base = if c == '\u{3099}' {
                "うかきくけこさしすせそたちつてとはひふへほウカキクケコサシスセソタチツテトハヒフヘホワヰヱヲ"
            } else {
                "はひふへほハヒフヘホ"
            };
            let voiced = if c == '\u{3099}' {
                "ゔがぎぐげござじずぜぞだぢづでどばびぶべぼヴガギグゲゴザジズゼゾダヂヅデドバビブベボヷヸヹヺ"
            } else {
                "ぱぴぷぺぽパピプペポ"
            };
            if let Some(index) = base.chars().position(|base| base == previous) {
                normalized.push(voiced.chars().nth(index)?);
            } else {
                normalized.push(previous);
                normalized.push(c);
            }
        } else {
            normalized.extend(c.to_lowercase());
        }
    }
    let value = normalized
        .chars()
        .map(|c| {
            confusions
                .get(&c.to_string())
                .cloned()
                .unwrap_or_else(|| c.to_string())
        })
        .collect::<String>();
    (!value.is_empty()
        && value.chars().count() <= MAX_IDENTITY_TEXT_CHARS
        && !value.chars().any(char::is_control))
    .then_some(value)
}

fn edit_distance(left: &str, right: &str, limit: u16) -> u16 {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) > usize::from(limit) {
        return limit + 1;
    }
    let mut previous = (0..=right.len()).map(|n| n as u16).collect::<Vec<_>>();
    for (i, a) in left.iter().enumerate() {
        let mut next = vec![limit + 1; right.len() + 1];
        next[0] = (i + 1) as u16;
        let start = (i + 1).saturating_sub(usize::from(limit)).max(1);
        let end = (i + 1 + usize::from(limit)).min(right.len());
        for j in start..=end {
            next[j] = (previous[j] + 1)
                .min(next[j - 1] + 1)
                .min(previous[j - 1] + u16::from(*a != right[j - 1]));
        }
        previous = next;
    }
    previous[right.len()].min(limit + 1)
}
