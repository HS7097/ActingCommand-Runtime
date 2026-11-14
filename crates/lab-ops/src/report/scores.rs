// SPDX-License-Identifier: AGPL-3.0-only

//! The failing step's scores (Workflow #467 RPT-1; `contracts/problem-report.md`, "Scores"): the
//! run's diagnostic stream read as its records, filtered to the failing step's span, and each
//! target's last and best score against its threshold; without the stream, what the step's last
//! recognition record holds (each target matched or not, each candidate page's best score).

use super::source::Recognition;
use serde_json::{Value, json};

/// The members that carry text a frame shows (an OCR read, its blocks, a value test's sides);
/// they leave the excerpt and the scores when the report carries no frames.
const TEXT_KEYS: [&str; 3] = ["raw_text", "derived_text", "text"];
/// The record kinds the scores read.
const STEP_STARTED: &str = "step_started";
const TERMINAL: &str = "terminal";
const TARGET: &str = "target";
const OCR: &str = "ocr";
const PAGE: &str = "page";

/// The records of one diagnostic stream, and how many of its lines were no JSON object.
pub(super) struct Stream {
    pub(super) records: Vec<Value>,
    pub(super) unreadable: usize,
}

/// The records of the diagnostic stream `bytes`: one per line after the header line, each
/// line's leading comma taken off; the footer and empty lines are no record.
pub(super) fn stream(bytes: &[u8]) -> Stream {
    let text = String::from_utf8_lossy(bytes);
    let mut records = Vec::new();
    let mut unreadable = 0;
    for line in text.lines().skip(1) {
        let trimmed = line.trim();
        let record = trimmed.strip_prefix(',').unwrap_or(trimmed);
        if record.is_empty() || record == "]}" {
            continue;
        }
        match serde_json::from_str::<Value>(record) {
            Ok(record) if record.is_object() => records.push(record),
            _ => unreadable += 1,
        }
    }
    Stream {
        records,
        unreadable,
    }
}

fn kind(record: &Value) -> &str {
    record["kind"].as_str().unwrap_or_default()
}

fn starts_step(record: &Value) -> bool {
    kind(record) == STEP_STARTED
}

/// The records of the failing step `step`: from its last `step_started` record to the next
/// step's, and every `terminal` record after them; without that step (or a stream that names
/// none), every record.
pub(super) fn span(records: &[Value], step: Option<u32>) -> Vec<Value> {
    let start = step.and_then(|step| {
        records.iter().rposition(|record| {
            starts_step(record) && record["data"]["step_index"].as_u64() == Some(u64::from(step))
        })
    });
    let Some(start) = start else {
        return records.to_vec();
    };
    let end = records[start + 1..]
        .iter()
        .position(starts_step)
        .map_or(records.len(), |offset| start + 1 + offset);
    let mut kept = records[start..end].to_vec();
    kept.extend(
        records[end..]
            .iter()
            .filter(|record| kind(record) == TERMINAL)
            .cloned(),
    );
    kept
}

/// The newest of `records` whose lines take at most `limit` bytes, oldest first, and whether
/// older ones were left out.
pub(super) fn bounded(records: Vec<Value>, limit: usize) -> (Vec<Value>, bool) {
    let mut kept = Vec::new();
    let mut used = 0;
    let mut cut = false;
    for record in records.into_iter().rev() {
        let size = record.to_string().len() + 1;
        if used + size > limit {
            cut = true;
            break;
        }
        used += size;
        kept.push(record);
    }
    kept.reverse();
    (kept, cut)
}

/// `record` without the text a frame shows ([`TEXT_KEYS`], at any depth), marked
/// `text_removed: true` when it held some; whether it held some.
pub(super) fn strip_text(record: &mut Value) -> bool {
    let mut removed = false;
    strip(record, &mut removed);
    if removed && let Value::Object(members) = record {
        members.insert("text_removed".to_owned(), Value::Bool(true));
    }
    removed
}

fn strip(value: &mut Value, removed: &mut bool) {
    match value {
        Value::Object(members) => {
            for key in TEXT_KEYS {
                if members.remove(key).is_some() {
                    *removed = true;
                }
            }
            for member in members.values_mut() {
                strip(member, removed);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip(item, removed);
            }
        }
        _ => {}
    }
}

/// Where the scores came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScoreSource {
    /// The run's diagnostic stream, the failing step's records.
    Stream,
    /// The failing step's last recognition record.
    Recognition,
    /// Neither holds a target.
    Nothing,
}

impl ScoreSource {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Recognition => "recognition",
            Self::Nothing => "none",
        }
    }
}

/// One target of the failing step: matched or not (its last evaluation), how often it was
/// evaluated, its last and best template score and threshold, its colour distance, the text
/// it read and the value test it compared.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TargetScore {
    pub(super) target: String,
    pub(super) kind: String,
    pub(super) matched: bool,
    pub(super) frames: u64,
    pub(super) last: Option<f64>,
    pub(super) best: Option<f64>,
    pub(super) threshold: Option<f64>,
    /// The colour distance and the largest it may be.
    pub(super) color: Option<(f64, f64)>,
    pub(super) text: Option<String>,
    pub(super) confidence: Option<f64>,
    pub(super) value_test: Option<Value>,
    pub(super) failure: Option<Value>,
}

impl TargetScore {
    fn named(target: &str, kind: &str) -> Self {
        Self {
            target: target.to_owned(),
            kind: kind.to_owned(),
            matched: false,
            frames: 0,
            last: None,
            best: None,
            threshold: None,
            color: None,
            text: None,
            confidence: None,
            value_test: None,
            failure: None,
        }
    }

    /// The last score less the threshold; for a colour, the largest distance less the distance
    /// (both pass at 0 or more).
    pub(super) fn margin(&self) -> Option<f64> {
        match (self.last, self.threshold) {
            (Some(last), Some(threshold)) => Some(last - threshold),
            _ => self.color.map(|(distance, largest)| largest - distance),
        }
    }

    fn json(&self) -> Value {
        json!({
            "target": self.target,
            "kind": self.kind,
            "matched": self.matched,
            "frames": self.frames,
            "last": self.last,
            "best": self.best,
            "threshold": self.threshold,
            "margin": self.margin(),
            "color": self.color.map(|(distance, largest)| {
                json!({"distance": distance, "max_distance": largest})
            }),
            "text": self.text,
            "confidence": self.confidence,
            "value_test": self.value_test,
            "failure": self.failure,
        })
    }
}

/// One page the failing step evaluated: matched or not (last), how often, and, from a
/// recognition record, its best score in milli and its first failed target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageScore {
    pub(super) page: String,
    pub(super) matched: bool,
    pub(super) evaluated: u64,
    pub(super) best_milli: Option<u32>,
    pub(super) first_failed: Option<String>,
}

/// The failing step's scores.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Scores {
    pub(super) source: ScoreSource,
    pub(super) targets: Vec<TargetScore>,
    pub(super) pages: Vec<PageScore>,
    /// The text a frame shows was taken out (a report without frames).
    pub(super) text_removed: bool,
}

impl Scores {
    pub(super) fn json(&self) -> Value {
        json!({
            "from": self.source.as_str(),
            "targets": self.targets.iter().map(TargetScore::json).collect::<Vec<_>>(),
            "pages": self.pages.iter().map(|page| json!({
                "page": page.page,
                "matched": page.matched,
                "evaluated": page.evaluated,
                "best_milli": page.best_milli,
                "first_failed_target": page.first_failed,
            })).collect::<Vec<_>>(),
            "text_removed": self.text_removed,
        })
    }
}

/// The scores of the failing step's stream records `records`: each `target` row counts one
/// evaluation of its target, its `ocr` rows (by `parent_index`) give the text it read, each
/// `page` row one evaluation of its page.
pub(super) fn from_stream(records: &[Value]) -> Scores {
    let mut targets: Vec<TargetScore> = Vec::new();
    let mut pages: Vec<PageScore> = Vec::new();
    // The record index of each target row, and its target's place in `targets`.
    let mut owners: Vec<(u64, usize)> = Vec::new();
    for record in records {
        let data = &record["data"];
        match kind(record) {
            TARGET => {
                let Some(id) = data["id"].as_str() else {
                    continue;
                };
                let place = match targets.iter().position(|target| target.target == id) {
                    Some(place) => place,
                    None => {
                        targets.push(TargetScore::named(
                            id,
                            data["kind"].as_str().unwrap_or_default(),
                        ));
                        targets.len() - 1
                    }
                };
                let target = &mut targets[place];
                target.frames += 1;
                target.matched = data["passed"].as_bool().unwrap_or(false);
                if let Some(score) = data["template"]["score"].as_f64() {
                    target.last = Some(score);
                    target.best = Some(target.best.map_or(score, |best| best.max(score)));
                }
                if let Some(threshold) = data["template"]["threshold"].as_f64() {
                    target.threshold = Some(threshold);
                }
                if let (Some(distance), Some(largest)) = (
                    data["color"]["distance"].as_f64(),
                    data["color"]["max_distance"].as_f64(),
                ) {
                    target.color = Some((distance, largest));
                }
                if !data["value_test"].is_null() {
                    target.value_test = Some(data["value_test"].clone());
                }
                if !data["failure"].is_null() {
                    target.failure = Some(data["failure"].clone());
                }
                if let Some(index) = record["index"].as_u64() {
                    owners.push((index, place));
                }
            }
            OCR => {
                let owner = record["parent_index"]
                    .as_u64()
                    .and_then(|parent| {
                        owners
                            .iter()
                            .rev()
                            .find(|(index, _)| *index == parent)
                            .map(|(_, place)| *place)
                    })
                    .or_else(|| {
                        data["target_id"]
                            .as_str()
                            .and_then(|id| targets.iter().position(|target| target.target == id))
                    });
                if let Some(place) = owner {
                    let text = data["derived_text"]
                        .as_str()
                        .or_else(|| data["raw_text"].as_str());
                    targets[place].text = text.map(str::to_owned);
                    targets[place].confidence = data["confidence"].as_f64();
                }
            }
            PAGE => {
                let Some(page) = data["page_id"].as_str() else {
                    continue;
                };
                let matched = data["matched"].as_bool().unwrap_or(false);
                match pages.iter_mut().find(|known| known.page == page) {
                    Some(known) => {
                        known.evaluated += 1;
                        known.matched = matched;
                    }
                    None => pages.push(PageScore {
                        page: page.to_owned(),
                        matched,
                        evaluated: 1,
                        best_milli: None,
                        first_failed: data["failed_target"]["target_id"]
                            .as_str()
                            .map(str::to_owned),
                    }),
                }
            }
            _ => {}
        }
    }
    let source = if targets.is_empty() && pages.is_empty() {
        ScoreSource::Nothing
    } else {
        ScoreSource::Stream
    };
    Scores {
        source,
        targets,
        pages,
        text_removed: false,
    }
}

/// What the failing step's last recognition record holds: each target matched or not, each
/// candidate page's verdict with its best score in milli and its first failed target.
pub(super) fn from_recognition(recognition: Option<&Recognition>) -> Scores {
    let Some(recognition) = recognition else {
        return Scores {
            source: ScoreSource::Nothing,
            targets: Vec::new(),
            pages: Vec::new(),
            text_removed: false,
        };
    };
    Scores {
        source: ScoreSource::Recognition,
        targets: recognition
            .targets
            .iter()
            .map(|verdict| TargetScore {
                matched: verdict.matched,
                frames: 1,
                ..TargetScore::named(&verdict.target, "")
            })
            .collect(),
        pages: recognition
            .pages
            .iter()
            .map(|page| PageScore {
                page: page.page.clone(),
                matched: page.matched,
                evaluated: 1,
                best_milli: page.best_milli,
                first_failed: page.first_failed.clone(),
            })
            .collect(),
        text_removed: false,
    }
}
