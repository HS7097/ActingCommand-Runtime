// SPDX-License-Identifier: AGPL-3.0-only

//! Bounded business identity mapping. Physical slot IDs remain layout coordinates.

use actingcommand_contract::candidate_projection::{
    CandidateFeature, CandidateIdentitySource, CandidateUnknownReason,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The largest business identity domain a pack may declare: its entries are the domain
/// (Workflow #308 L2), and a policy's identity enum must equal it.
pub const MAX_IDENTITY_ENTRIES: usize = 1_024;
pub const MAX_IDENTITY_ALIASES: usize = 8;
pub const MAX_IDENTITY_TEXT_CHARS: usize = 128;
pub const MAX_IDENTITY_TEMPLATES: usize = 16;
/// Workflow #503 LIST-B (`contracts/business-identity-consensus.md`, "A layout's pool"): the most
/// entries the pool of a `repeated_anchor` layout's picture identity holds; a slot's pool keeps
/// [`MAX_IDENTITY_TEMPLATES`].
pub const MAX_LAYOUT_IDENTITY_TEMPLATES: usize = 128;

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

/// Workflow #503 LIST-B (`contracts/business-identity-consensus.md`, "A layout's pool"): one entry
/// of the pool a `repeated_anchor` layout declares for a picture identity feature, for every
/// instance: `{id, variant?, target_id}`, the picture of a key, or `{"other": true, target_id}`,
/// the picture of a known card that is none of the keys, which names no `id` and no `variant`. A
/// slot's pool keeps [`CandidateIdentityTemplate`], which takes no `other`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePoolTemplate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub other: bool,
    pub target_id: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl CandidatePoolTemplate {
    /// The key a key entry names, its `id` and `variant`; `None` for an `other` entry (and for an
    /// entry that names no `id`, which admission refuses). Named `keyed`, not `key`: the outcome
    /// guard keeps the getter name `key` for typed codes (fix round 1, the first build).
    pub fn keyed(&self) -> Option<(&str, Option<&str>)> {
        if self.other {
            return None;
        }
        self.id.as_deref().map(|id| (id, self.variant.as_deref()))
    }
}

/// Workflow #503 LIST-B: what the projection read of one entry of a layout's pool on one
/// instance, as the mapping takes it: the entry's template score in normalized integer milli and,
/// for a target that declares a colour check, whether the check held where the template matched
/// (`None` for a target without one, and, fix round 1, for one whose colour was not measured: its
/// template did not reach the target's own threshold here). The mapping decides what both mean
/// ([`CandidateIdentityDeclaration::read_pool`]); the producer only hands them over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidatePoolScore {
    pub score_milli: u16,
    pub colour_held: Option<bool>,
}

/// Workflow #503 LIST-B (`contracts/resource-readings.md`, "Offline"): the best key entry of one
/// read of a layout's pool: its key, its variant, its target and its score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidatePictureKey {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub target_id: String,
    pub score_milli: u16,
}

/// Workflow #503 LIST-B: the best `other` entry of one read of a layout's pool, by its target, and
/// its score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidatePictureOther {
    pub target_id: String,
    pub score_milli: u16,
}

/// Workflow #503 LIST-B (`contracts/resource-readings.md`, "List readings", "Offline"): one read of
/// a layout's pool on one instance with what the mapping judged of it, the evidence the dry run of
/// a list reading and the Lab's `package list-check` answer: the mark, why it names no key (`None`
/// for a known key), the best key entry and the best `other` entry among the entries that
/// competed, the threshold and the margin it was judged by, and how many entries did not compete
/// because their target's colour check failed. Judged once, beside the projection; it enters no
/// hash and no record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidatePictureReading {
    pub mark: CandidateNameMark,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_cause: Option<CandidateUnreadCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_key: Option<CandidatePictureKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_other: Option<CandidatePictureOther>,
    pub minimum_score_milli: u16,
    pub minimum_margin_milli: u16,
    pub colour_failed: u32,
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
        self.judge_ocr(text, confidence).0
    }

    /// Workflow #503 LIST-A (`contracts/business-identity-consensus.md`, "Identity and readable
    /// attributes"; `contracts/resource-readings.md`, "List readings"): the mark the mapping gives
    /// one OCR read of a name, beside the feature [`Self::map_ocr`] gives it, in the one judgement
    /// both come from. [`CandidateNameMark::Known`] when the read names a wanted key;
    /// [`CandidateNameMark::NoneOfTheKeys`] only when its text is not empty, its confidence is at
    /// least `minimum_confidence_milli`, and its distance to every alias of every entry is greater
    /// than `max_distance + minimum_margin`; [`CandidateNameMark::Unread`] for everything else (an
    /// empty or unreadable text, a confidence missing or below the floor, a read near a wanted name
    /// that does not match it, an ambiguous one, and every icon identity, which reads no text).
    pub fn mark_ocr(&self, text: &str, confidence: Option<u16>) -> CandidateNameMark {
        match self.judge_ocr(text, confidence) {
            (CandidateFeature::Identity { .. }, _) => CandidateNameMark::Known,
            (_, true) => CandidateNameMark::NoneOfTheKeys,
            (_, false) => CandidateNameMark::Unread,
        }
    }

    /// The one judgement of an OCR read (`map_ocr`'s feature), and whether the read passed the
    /// confidence floor, normalized to a text that is not empty, and lies further than
    /// `max_distance + minimum_margin` from every alias of every entry (Workflow #503 LIST-A).
    fn judge_ocr(&self, text: &str, confidence: Option<u16>) -> (CandidateFeature, bool) {
        let CandidateIdentityRecognition::OcrAliases {
            max_distance,
            minimum_margin,
            minimum_confidence_milli,
            confusions,
        } = &self.recognition
        else {
            return (self.unknown(CandidateUnknownReason::OutOfDomain), false);
        };
        if confidence.is_none_or(|confidence| confidence < *minimum_confidence_milli) {
            return (self.unknown(CandidateUnknownReason::LowConfidence), false);
        }
        let Some(text) = normalize_name(text, confusions) else {
            return (self.unknown(CandidateUnknownReason::OutOfDomain), false);
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
        // Workflow #503 LIST-A: a distance is saturated at `limit + 1`, so one above `limit` is
        // exactly one further than `max_distance + minimum_margin` from every alias.
        let beyond_every_alias = rows.iter().all(|&(_, _, distance)| distance > limit);
        let feature = self.choose(
            &rows,
            *max_distance,
            *minimum_margin,
            confidence.map(i64::from),
        );
        (feature, beyond_every_alias)
    }

    /// Workflow #503 LIST-A3 (`contracts/business-identity-consensus.md`, "Identity and readable
    /// attributes"; `contracts/resource-readings.md`, "List readings", "Offline"): the wanted name
    /// nearest to one OCR read of a name, beside the mark [`Self::mark_ocr`] gives it: the entry and
    /// the alias whose normalized text lies at the smallest edit distance from the read's
    /// normalized text (the first in declaration order among equals), that distance itself, not
    /// bounded at the margin, and `max_distance + minimum_margin`, the distance a read must exceed
    /// to be none of the keys. The distance is the one the mapping measures, after the same
    /// normalization and confusions. `None` for a text that normalizes to nothing (empty or
    /// overlong) and for an icon identity, which reads no text. The confidence plays no part: the
    /// nearest name is the same whatever the read's confidence.
    pub fn nearest_ocr(&self, text: &str) -> Option<CandidateNearestName> {
        let CandidateIdentityRecognition::OcrAliases {
            max_distance,
            minimum_margin,
            confusions,
            ..
        } = &self.recognition
        else {
            return None;
        };
        let text = normalize_name(text, confusions)?;
        let limit = max_distance + minimum_margin;
        let mut nearest: Option<CandidateNearestName> = None;
        for entry in &self.entries {
            for alias in &entry.aliases {
                let Some(normalized) = normalize_name(alias, confusions) else {
                    continue;
                };
                let distance = unbounded_edit_distance(&text, &normalized);
                if nearest.as_ref().is_none_or(|kept| distance < kept.distance) {
                    nearest = Some(CandidateNearestName {
                        key: entry.id.clone(),
                        variant: entry.variant.clone(),
                        alias: alias.clone(),
                        distance,
                        limit,
                    });
                }
            }
        }
        nearest
    }

    /// Workflow #503 LIST-A3: everything the mapping says of one OCR read of a name, for a reader
    /// that shows it (the dry run of a list reading): the text and the confidence as read, the
    /// mark ([`Self::mark_ocr`]), why it names no key ([`Self::map_ocr_with_cause`]; `None` for a
    /// known key) and the nearest wanted name ([`Self::nearest_ocr`]). Each part is the mapping's
    /// own; nothing here judges again.
    pub fn read_ocr(&self, text: &str, confidence: Option<u16>) -> CandidateNameReading {
        let (_, unread_cause) = self.map_ocr_with_cause(text, confidence);
        CandidateNameReading {
            text: text.to_owned(),
            confidence_milli: confidence,
            mark: self.mark_ocr(text, confidence),
            unread_cause,
            nearest: self.nearest_ocr(text),
        }
    }

    /// Workflow #463 SEL-GRID G2 ([`CandidateUnreadCause`]): [`Self::map_ocr`]'s feature, and when
    /// the reading names no identity, why, in the words of a select's refusal: the feature's
    /// reason, but a reading whose confidence passed `minimum_confidence_milli` and that no alias
    /// matches within `max_distance` is [`CandidateUnreadCause::BeyondDistance`] where the feature
    /// keeps `low_confidence`. The one mapping: the feature is `map_ocr`'s, and the cause is read
    /// from it afterwards, so `map_ocr` does nothing it did not do before; only the Lab's dry run
    /// asks for the cause.
    pub fn map_ocr_with_cause(
        &self,
        text: &str,
        confidence: Option<u16>,
    ) -> (CandidateFeature, Option<CandidateUnreadCause>) {
        let feature = self.map_ocr(text, confidence);
        // `map_ocr` answers `low_confidence` past the confidence only from its distance: no alias
        // lies within `max_distance`.
        let passed = match &self.recognition {
            CandidateIdentityRecognition::OcrAliases {
                minimum_confidence_milli,
                ..
            } => confidence.is_some_and(|confidence| confidence >= *minimum_confidence_milli),
            CandidateIdentityRecognition::IconTemplates { .. } => false,
        };
        let cause = match &feature {
            CandidateFeature::Unknown {
                reason: CandidateUnknownReason::LowConfidence,
                ..
            } if passed => Some(CandidateUnreadCause::BeyondDistance),
            CandidateFeature::Unknown { reason, .. } => Some(CandidateUnreadCause::of(*reason)),
            _ => None,
        };
        (feature, cause)
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

    /// Workflow #503 LIST-B (`contracts/business-identity-consensus.md`, "A layout's pool";
    /// `contracts/resource-readings.md`, "A list of the wanted keys only"): the one judgement of a
    /// read of a `repeated_anchor` layout's pool on one instance, from every entry's score, in
    /// the pool's order, and the feature it gives the projection. An entry whose target's colour
    /// check failed does not compete at all (it is neither the best key nor the best `other`);
    /// the others are judged:
    ///
    /// - **a key** as [`Self::map_icons`] judges the key entries that compete (the best above
    ///   `minimum_score_milli` and clear by `minimum_margin_milli` of every other key), with the
    ///   best `other` entry counted as one more competitor of a different identity for the margin:
    ///   a key within the margin of it is `ambiguous`;
    /// - **none of the keys** only when the best `other` entry's score is at least
    ///   `minimum_score_milli` and exceeds the best key entry's score by at least
    ///   `minimum_margin_milli`; its feature is Unknown `out_of_domain`, the reason that says the
    ///   read names something outside the identity's domain;
    /// - **unread** in every other case, with the keys' own reason (`low_confidence` when no key
    ///   entry reached the threshold or none competed, `ambiguous` for two keys within the margin).
    ///
    /// A pool without an `other` entry whose entries all compete answers the feature
    /// [`Self::map_icons`] answers over the same scores. An identity that reads no picture, a pool
    /// of another length than its scores, an empty pool, one above
    /// [`MAX_LAYOUT_IDENTITY_TEMPLATES`] and a score above 1000 are unread `out_of_domain`.
    pub fn read_pool(
        &self,
        pool: &[CandidatePoolTemplate],
        scores: &[CandidatePoolScore],
    ) -> (CandidateFeature, CandidatePictureReading) {
        let (minimum_score_milli, minimum_margin_milli) = match self.recognition {
            CandidateIdentityRecognition::IconTemplates {
                minimum_score_milli,
                minimum_margin_milli,
            } => (minimum_score_milli, minimum_margin_milli),
            CandidateIdentityRecognition::OcrAliases { .. } => (0, 0),
        };
        let unread = |feature: CandidateFeature, colour_failed: u32| {
            let reading = CandidatePictureReading {
                mark: CandidateNameMark::Unread,
                unread_cause: unread_cause(&feature),
                best_key: None,
                best_other: None,
                minimum_score_milli,
                minimum_margin_milli,
                colour_failed,
            };
            (feature, reading)
        };
        if !matches!(
            self.recognition,
            CandidateIdentityRecognition::IconTemplates { .. }
        ) || pool.len() != scores.len()
            || pool.is_empty()
            || pool.len() > MAX_LAYOUT_IDENTITY_TEMPLATES
            || scores.iter().any(|score| score.score_milli > 1000)
        {
            return unread(self.unknown(CandidateUnknownReason::OutOfDomain), 0);
        }
        let colour_failed = u32::try_from(
            scores
                .iter()
                .filter(|score| score.colour_held == Some(false))
                .count(),
        )
        .unwrap_or(u32::MAX);
        let mut keys = Vec::new();
        let mut best_key: Option<(&CandidatePoolTemplate, u16)> = None;
        let mut best_other: Option<(&CandidatePoolTemplate, u16)> = None;
        for (entry, score) in pool
            .iter()
            .zip(scores)
            .filter(|(_, score)| score.colour_held != Some(false))
        {
            let milli = score.score_milli;
            let best = match entry.keyed() {
                Some((id, variant)) => {
                    keys.push((id, variant, 1000 - milli));
                    &mut best_key
                }
                None => &mut best_other,
            };
            if (*best).is_none_or(|(_, kept)| milli > kept) {
                *best = Some((entry, milli));
            }
        }
        let keys_feature = if keys.is_empty() {
            self.unknown(CandidateUnknownReason::LowConfidence)
        } else {
            self.choose(
                &keys,
                1000 - minimum_score_milli,
                minimum_margin_milli,
                best_key.map(|(_, milli)| i64::from(milli)),
            )
        };
        let key_score = u32::from(best_key.map_or(0, |(_, milli)| milli));
        let margin = u32::from(minimum_margin_milli);
        let known = matches!(keys_feature, CandidateFeature::Identity { .. });
        let (mark, feature) = match best_other {
            Some((_, other))
                if other >= minimum_score_milli && u32::from(other) >= key_score + margin =>
            {
                (
                    CandidateNameMark::NoneOfTheKeys,
                    self.unknown(CandidateUnknownReason::OutOfDomain),
                )
            }
            Some((_, other)) if known && key_score < u32::from(other) + margin => (
                CandidateNameMark::Unread,
                self.unknown(CandidateUnknownReason::Ambiguous),
            ),
            _ if known => (CandidateNameMark::Known, keys_feature),
            _ => (CandidateNameMark::Unread, keys_feature),
        };
        let reading = CandidatePictureReading {
            mark,
            unread_cause: unread_cause(&feature),
            best_key: best_key.and_then(|(entry, milli)| {
                entry.keyed().map(|(key, variant)| CandidatePictureKey {
                    key: key.to_owned(),
                    variant: variant.map(str::to_owned),
                    target_id: entry.target_id.clone(),
                    score_milli: milli,
                })
            }),
            best_other: best_other.map(|(entry, milli)| CandidatePictureOther {
                target_id: entry.target_id.clone(),
                score_milli: milli,
            }),
            minimum_score_milli,
            minimum_margin_milli,
            colour_failed,
        };
        (feature, reading)
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

/// Workflow #463 SEL-GRID G2 (`contracts/lab-recording.md`, the refusal `select_name_unread`):
/// why a name read no known name, as a select's refusal names it. Every word but one is the
/// feature's Unknown reason; `beyond_distance` is a reading that passed the confidence and that
/// no alias matches within `max_distance`, which the feature, its records and its decisions keep
/// as `low_confidence` (the projection's reasons are a closed vocabulary of the ledger's records,
/// which this slice does not move). A word for a refusal; nothing decides by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateUnreadCause {
    Missing,
    OutOfDomain,
    /// No confidence, or one below `minimum_confidence_milli`.
    LowConfidence,
    /// The confidence passed; no alias lies within `max_distance`.
    BeyondDistance,
    Ambiguous,
    NoConsensus,
}

impl CandidateUnreadCause {
    /// The cause of an Unknown of `reason` that is the reason itself.
    pub const fn of(reason: CandidateUnknownReason) -> Self {
        match reason {
            CandidateUnknownReason::Missing => Self::Missing,
            CandidateUnknownReason::OutOfDomain => Self::OutOfDomain,
            CandidateUnknownReason::LowConfidence => Self::LowConfidence,
            CandidateUnknownReason::Ambiguous => Self::Ambiguous,
            CandidateUnknownReason::NoConsensus => Self::NoConsensus,
        }
    }

    /// The word, as it serializes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::OutOfDomain => "out_of_domain",
            Self::LowConfidence => "low_confidence",
            Self::BeyondDistance => "beyond_distance",
            Self::Ambiguous => "ambiguous",
            Self::NoConsensus => "no_consensus",
        }
    }
}

/// Workflow #503 LIST-B: why the feature `feature` names no key, its Unknown reason; `None` for an
/// identity.
fn unread_cause(feature: &CandidateFeature) -> Option<CandidateUnreadCause> {
    match feature {
        CandidateFeature::Unknown { reason, .. } => Some(CandidateUnreadCause::of(*reason)),
        _ => None,
    }
}

/// Workflow #503 LIST-A (`contracts/resource-readings.md`, "List readings", "A list of the wanted
/// keys only"): what one read of a name is to a reading that wants only the keys its identity
/// declares, the mark the mapping gives ([`CandidateIdentityDeclaration::mark_ocr`]) beside the
/// known key and the unread reading. The feature, its records and its decisions do not change:
/// a read marked [`Self::NoneOfTheKeys`] keeps its Unknown feature (the projection's reasons are
/// a closed vocabulary of the ledger's records, which this slice does not move). Readers take the
/// mark; none sorts the feature's reasons itself.
/// Workflow #503 LIST-A3: the mark serializes as `known`, `unread` or `none_of_the_keys`, for the
/// dry run of a list reading that shows it. Workflow #503 LIST-B: a read of a `repeated_anchor`
/// layout's picture pool gets the same three marks ([`CandidateIdentityDeclaration::read_pool`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateNameMark {
    /// The read names a wanted key: the feature is its identity.
    Known,
    /// The read names no key and is not far from every one: empty, below the confidence floor,
    /// near a wanted name without matching it, ambiguous, or an identity that reads no text.
    Unread,
    /// The read is a name, with confidence, further than `max_distance + minimum_margin` from
    /// every alias of every entry: none of the wanted keys.
    NoneOfTheKeys,
}

/// Workflow #503 LIST-A3 (`contracts/business-identity-consensus.md`, "Identity and readable
/// attributes"): the wanted name nearest to one OCR read, as the mapping measures it
/// ([`CandidateIdentityDeclaration::nearest_ocr`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateNearestName {
    /// The entry's id: the wanted key.
    pub key: String,
    /// The entry's variant, when it declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The alias, as the entry declares it.
    pub alias: String,
    /// The edit distance between the read's and the alias's normalized texts, not bounded.
    pub distance: u16,
    /// `max_distance + minimum_margin`: a read further than this from every alias of every entry
    /// is none of the keys, one within it near a wanted name.
    pub limit: u16,
}

/// Workflow #503 LIST-A3: one OCR read of a name with what the mapping says of it
/// ([`CandidateIdentityDeclaration::read_ocr`]). A name's text is what an account holds: a reader
/// shows it where it was asked to, and records it nowhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateNameReading {
    /// The text as read.
    pub text: String,
    /// The read's confidence, milli, when the provider gave one.
    pub confidence_milli: Option<u16>,
    pub mark: CandidateNameMark,
    /// Why the read names no key; `None` for a known key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_cause: Option<CandidateUnreadCause>,
    /// The nearest wanted name; `None` for a text that normalizes to nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nearest: Option<CandidateNearestName>,
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

/// Workflow #503 LIST-A3: [`edit_distance`] with a bound no distance of the two texts can reach,
/// the longer one's length, so that its band is the whole table and the distance is exact. Both
/// texts are normalized names, at most `MAX_IDENTITY_TEXT_CHARS` characters each.
fn unbounded_edit_distance(left: &str, right: &str) -> u16 {
    let longest = left.chars().count().max(right.chars().count());
    edit_distance(left, right, u16::try_from(longest).unwrap_or(u16::MAX - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Workflow #503 LIST-A (register §1.533, ruling 2): the mark the mapping gives a read of a
    // name. Three wanted keys, each with one alias, read within 1 at a margin of 1 and a floor of
    // 800, so `max_distance + minimum_margin` is 2. A wanted alias, and a read 1 from it, is the
    // known key; a name further than 2 from every alias, read at 950, is none of the keys, and its
    // feature stays the Unknown `low_confidence` it was; a read 2 from an alias (near a wanted name
    // without matching it), an empty text, a confidence below the floor or none, and any read of
    // an icon identity are unread.
    #[test]
    fn list_a_a_name_far_from_every_alias_is_none_of_the_keys_and_a_near_empty_or_low_one_unread() {
        let identity = CandidateIdentityDeclaration {
            entries: [
                ("member-a", "alpha"),
                ("member-b", "bravo"),
                ("member-c", "charlie"),
            ]
            .into_iter()
            .map(|(id, alias)| CandidateIdentityEntry {
                id: id.to_owned(),
                variant: None,
                aliases: vec![alias.to_owned()],
            })
            .collect(),
            recognition: CandidateIdentityRecognition::OcrAliases {
                max_distance: 1,
                minimum_margin: 1,
                minimum_confidence_milli: 800,
                confusions: BTreeMap::new(),
            },
        };
        identity.validate().expect("the identity validates");

        for wanted in ["alpha", "alphx"] {
            assert_eq!(
                identity.mark_ocr(wanted, Some(950)),
                CandidateNameMark::Known,
                "{wanted}"
            );
            assert!(
                matches!(
                    identity.map_ocr(wanted, Some(950)),
                    CandidateFeature::Identity { ref value, .. } if value == "member-a"
                ),
                "{wanted}"
            );
        }

        assert_eq!(
            identity.mark_ocr("zulu", Some(950)),
            CandidateNameMark::NoneOfTheKeys
        );
        assert_eq!(
            identity.map_ocr("zulu", Some(950)),
            CandidateFeature::Unknown {
                reason: CandidateUnknownReason::LowConfidence,
                source: Some(CandidateIdentitySource::OcrAlias),
            },
            "the feature of a name none of the keys is the one it was"
        );

        for (text, confidence, case) in [
            ("alpzz", Some(950), "2 from an alias"),
            ("", Some(950), "an empty text"),
            ("zulu", Some(799), "below the floor"),
            ("zulu", None, "no confidence"),
        ] {
            assert_eq!(
                identity.mark_ocr(text, confidence),
                CandidateNameMark::Unread,
                "{case}"
            );
        }

        let icons = CandidateIdentityDeclaration {
            recognition: CandidateIdentityRecognition::IconTemplates {
                minimum_score_milli: 900,
                minimum_margin_milli: 50,
            },
            ..identity
        };
        assert_eq!(
            icons.mark_ocr("zulu", Some(950)),
            CandidateNameMark::Unread,
            "an icon identity reads no text"
        );
    }

    // Workflow #503 LIST-A3 (register §1.534, ruling 1): the nearest wanted name the mapping gives
    // a read of a name, beside its mark. LIST-A's identity above: `alpha`, `bravo`, `charlie`,
    // within 1 at a margin of 1, so the limit `max_distance + minimum_margin` is 2. An exact alias
    // is at 0 from its own entry and known; a near miss (`alpzz`, two letters changed) is at 2 from
    // `alpha` and unread; a read far from every alias (`cxxxxxe`, five letters of `charlie` changed,
    // seven edits from the others) is at its true distance 5, where the distance the mark is
    // judged on stops at 3 (one past the limit), and none of the keys. An empty text and an icon
    // identity have no nearest name.
    #[test]
    fn list_a3_the_nearest_wanted_name_is_the_true_distance_and_none_for_an_empty_text_or_icons() {
        let identity = CandidateIdentityDeclaration {
            entries: [
                ("member-a", "alpha"),
                ("member-b", "bravo"),
                ("member-c", "charlie"),
            ]
            .into_iter()
            .map(|(id, alias)| CandidateIdentityEntry {
                id: id.to_owned(),
                variant: None,
                aliases: vec![alias.to_owned()],
            })
            .collect(),
            recognition: CandidateIdentityRecognition::OcrAliases {
                max_distance: 1,
                minimum_margin: 1,
                minimum_confidence_milli: 800,
                confusions: BTreeMap::new(),
            },
        };
        identity.validate().expect("the identity validates");
        let nearest = |key: &str, alias: &str, distance: u16| CandidateNearestName {
            key: key.to_owned(),
            variant: None,
            alias: alias.to_owned(),
            distance,
            limit: 2,
        };

        for (text, mark, unread_cause, expected) in [
            (
                "alpha",
                CandidateNameMark::Known,
                None,
                nearest("member-a", "alpha", 0),
            ),
            (
                "alpzz",
                CandidateNameMark::Unread,
                Some(CandidateUnreadCause::BeyondDistance),
                nearest("member-a", "alpha", 2),
            ),
            (
                "cxxxxxe",
                CandidateNameMark::NoneOfTheKeys,
                Some(CandidateUnreadCause::BeyondDistance),
                nearest("member-c", "charlie", 5),
            ),
        ] {
            assert_eq!(identity.nearest_ocr(text), Some(expected.clone()), "{text}");
            assert_eq!(
                identity.read_ocr(text, Some(950)),
                CandidateNameReading {
                    text: text.to_owned(),
                    confidence_milli: Some(950),
                    mark,
                    unread_cause,
                    nearest: Some(expected),
                },
                "{text}"
            );
        }
        assert!(
            identity.nearest_ocr("cxxxxxe").map(|found| found.distance) > Some(3),
            "the true distance, not the one past the limit the mark is judged on"
        );

        assert_eq!(identity.nearest_ocr(""), None, "an empty text");
        assert_eq!(
            identity.read_ocr("", Some(950)).nearest,
            None,
            "an empty text"
        );
        let icons = CandidateIdentityDeclaration {
            recognition: CandidateIdentityRecognition::IconTemplates {
                minimum_score_milli: 900,
                minimum_margin_milli: 50,
            },
            ..identity
        };
        assert_eq!(icons.nearest_ocr("alpha"), None, "an icon identity");
    }

    #[test]
    fn a_pack_declares_an_identity_domain_of_up_to_1024_entries() {
        let declaration = |size: usize| CandidateIdentityDeclaration {
            entries: (0..size)
                .map(|index| CandidateIdentityEntry {
                    id: format!("member-{index:04}"),
                    variant: None,
                    aliases: vec![format!("member {index:04}")],
                })
                .collect(),
            recognition: CandidateIdentityRecognition::OcrAliases {
                max_distance: 1,
                minimum_margin: 1,
                minimum_confidence_milli: 800,
                confusions: BTreeMap::new(),
            },
        };
        assert_eq!(MAX_IDENTITY_ENTRIES, 1_024);
        let domain = declaration(MAX_IDENTITY_ENTRIES);
        domain.validate().expect("a 1024-entry domain validates");
        assert_eq!(domain.domain().len(), 1_024);
        assert!(declaration(MAX_IDENTITY_ENTRIES + 1).validate().is_err());

        // The pack that declares the domain in a layout is admitted at 1024 entries and
        // refused at 1025 under the pack's refusal code.
        let pack = |size: usize| -> crate::RecognitionPack {
            serde_json::from_value(serde_json::json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "targets": [{
                    "type": "ocr", "id": "candidate/name",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
                    "expected": ["member 0000"], "case_sensitive": true,
                    "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
                    "model_sha256": "a".repeat(64)
                }],
                "candidate_layouts": [{
                    "id": "layout-list", "page_id": "neutral/home", "kind": "fixed_slots",
                    "features": [{"name": "member", "value": "identity",
                                  "identity": declaration(size)}],
                    "slots": [{"rect": {"x": 0, "y": 0, "width": 2, "height": 1},
                               "click": {"x": 0, "y": 0, "width": 2, "height": 1},
                               "targets": {"member": "candidate/name"}}]
                }]
            }))
            .expect("pack decodes")
        };
        crate::RecognitionEvaluator::new(std::path::PathBuf::new(), pack(MAX_IDENTITY_ENTRIES))
            .expect("a pack with a 1024-entry domain is admitted");
        let refusal = crate::RecognitionEvaluator::new(
            std::path::PathBuf::new(),
            pack(MAX_IDENTITY_ENTRIES + 1),
        )
        .expect_err("a pack with a 1025-entry domain is refused");
        assert_eq!(
            refusal.code(),
            crate::RecognitionPackErrorCode::InvalidPackage
        );
    }
}
