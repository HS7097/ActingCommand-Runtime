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

/// Workflow #503 LIST-A (`contracts/resource-readings.md`, "List readings", "A list of the wanted
/// keys only"): what one read of a name is to a reading that wants only the keys its identity
/// declares, the mark the mapping gives ([`CandidateIdentityDeclaration::mark_ocr`]) beside the
/// known key and the unread reading. The feature, its records and its decisions do not change:
/// a read marked [`Self::NoneOfTheKeys`] keeps its Unknown feature (the projection's reasons are
/// a closed vocabulary of the ledger's records, which this slice does not move). Readers take the
/// mark; none sorts the feature's reasons itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
