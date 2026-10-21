// SPDX-License-Identifier: AGPL-3.0-only

//! Value tests (Workflow #396 REC-6, `contracts/resource-declarations.md`, section Task schema
//! `0.9` value tests).
//!
//! A value test reads one ordinary OCR target on each side, trims the text, parses it with the
//! shared `OcrUnsignedIntegerFormat` parser and compares the left value with a constant or with
//! the other side's value times `mul` plus `add`. Each side reads its OCR target once per
//! request and frame, within the consumer's text read budget (`text_locate`). A side that does
//! not read fails the test with `value_test_unresolved`; a comparison that does not hold fails
//! it with `value_test_not_met`. A provider failure is an evaluation error, as for every OCR
//! target.
//!
//! Workflow #411 B1 (register §1.163): a side's `value` may declare a `dictionary`, applied
//! between the trim and the parser, and a side may declare an `agreement`: the test is then read
//! on each of its frames and the side has the one value read on at least `k` of them, else it is
//! unread with `no_agreement`.

use crate::codes::{
    RecognitionPackCode, RecognitionPackLocation, ValueOp, ValueProblem, ValueSide,
};
use crate::text_locate::{LocateRefusal, pointer_token, refuse as refusal};
use crate::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    IntegerAgreement, IntegerDictionary, OcrFieldReason, OcrUnsignedIntegerFormat, OcrValueTrim,
    ResourceDeclarationReason, TaskDiagnosticSampleData, TaskDiagnosticSamplingData,
    TaskDiagnosticValueOperand, TaskDiagnosticValueRight, TaskDiagnosticValueTestData, ValueTestOp,
    ValueTestProblem,
};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

/// The largest constant and `add` magnitude: 2^53 - 1, exact in every JSON consumer.
const VALUE_TEST_MAX_EXACT: u64 = (1 << 53) - 1;
/// The largest `mul`.
const VALUE_TEST_MAX_MUL: u64 = 1_000_000;

/// Schema 0.7 `value_test` target: a comparison of read values. It is never clicked and locates
/// nothing.
#[derive(Debug, Clone, Deserialize)]
pub struct ValueTestTarget {
    pub id: String,
    pub left: ValueTestOperand,
    pub op: ValueTestOp,
    /// Absent exactly for `parses`.
    #[serde(default)]
    pub right: Option<ValueTestRight>,
}

/// One side that reads an ordinary OCR target. `mul` and `add` are written on the right side
/// only, and there both.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTestOperand {
    pub target_id: String,
    pub trim: OcrValueTrim,
    pub value: ValueTestValue,
    /// Required with `current_capacity`, refused with any other format.
    #[serde(default)]
    pub part: Option<ValueTestPart>,
    #[serde(default)]
    pub mul: Option<u64>,
    #[serde(default)]
    pub add: Option<i64>,
    /// Workflow #411 B1: the frames that must read one value before the side has it; absent,
    /// the side reads the frame it is evaluated on.
    #[serde(default)]
    pub agreement: Option<IntegerAgreement>,
}

/// The value a side reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueTestValue {
    UnsignedInteger {
        min: u64,
        max: u64,
        #[serde(default)]
        format: OcrUnsignedIntegerFormat,
        /// Workflow #411 B1: the number's dictionary; absent, the text is parsed as before.
        #[serde(default)]
        dictionary: Option<IntegerDictionary>,
    },
}

/// Which part of a `current_capacity` text a side reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueTestPart {
    Current,
    Capacity,
}

/// `{"const": n}` or another side.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ValueTestRight {
    Const(ValueTestConst),
    Operand(ValueTestOperand),
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTestConst {
    #[serde(rename = "const")]
    pub constant: u64,
}

/// A value test's evidence and the PP-OCR reports of the reads it made.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValueTestEvaluation {
    #[serde(flatten)]
    pub data: TaskDiagnosticValueTestData,
    #[serde(skip)]
    pub ppocr_diagnostics: actingcommand_contract::PpocrDiagnostics,
}

/// One side as read: its evidence and, when it read, its value.
#[derive(Clone)]
struct SideReading {
    evidence: TaskDiagnosticValueOperand,
    value: Option<u64>,
}

/// The right side as read: a constant, or another side scaled by `mul` and `add`.
#[derive(Clone)]
enum RightReading {
    Const(u64),
    Operand {
        reading: SideReading,
        mul: u64,
        add: i64,
    },
}

impl ValueTestTarget {
    /// The sides that read an OCR target: `left`, then an operand `right`.
    fn operands(&self) -> impl Iterator<Item = &ValueTestOperand> {
        std::iter::once(&self.left).chain(match &self.right {
            Some(ValueTestRight::Operand(operand)) => Some(operand),
            _ => None,
        })
    }

    /// Workflow #411 B1: the frames the test reads when a side declares an agreement (the most
    /// any of its sides declares) and their interval; `None` when no side declares one.
    pub(crate) fn agreement_frames(&self) -> Option<(usize, u64)> {
        self.operands()
            .filter_map(|operand| operand.agreement)
            .map(|agreement| (agreement.frame_count(), agreement.interval_ms))
            .max_by_key(|(frames, _)| *frames)
    }

    /// Workflow #411 B1 (B-train plan §2.1 item 2): the target consensus entry a test with an
    /// agreement is sampled under, so every observation of its consumers takes its frames
    /// `interval_ms` apart as for target consensus: one unperturbed sample on each of its frames,
    /// the current frame last. Its `k` is never read; the agreement decides each side's value.
    /// `None` without an agreement.
    pub(crate) fn agreement_consensus(&self) -> Option<TargetConsensus> {
        let (frames, interval_ms) = self.agreement_frames()?;
        Some(TargetConsensus {
            samples: (0..frames)
                .map(|frame| CandidateSampleVariant {
                    frame: u8::try_from(frame).unwrap_or(u8::MAX),
                    ..CandidateSampleVariant::default()
                })
                .collect(),
            k: u8::try_from(frames).unwrap_or(u8::MAX),
            sample_interval_ms: u16::try_from(interval_ms).unwrap_or(u16::MAX),
        })
    }
}

impl From<ValueTestOp> for ValueOp {
    fn from(op: ValueTestOp) -> Self {
        match op {
            ValueTestOp::Eq => Self::Eq,
            ValueTestOp::Ne => Self::Ne,
            ValueTestOp::Lt => Self::Lt,
            ValueTestOp::Le => Self::Le,
            ValueTestOp::Gt => Self::Gt,
            ValueTestOp::Ge => Self::Ge,
            ValueTestOp::Parses => Self::Parses,
        }
    }
}

impl From<ValueTestProblem> for ValueProblem {
    fn from(problem: ValueTestProblem) -> Self {
        match problem {
            ValueTestProblem::Empty => Self::Empty,
            ValueTestProblem::Invalid => Self::Invalid,
            ValueTestProblem::Overflow => Self::Overflow,
            ValueTestProblem::OutOfRange => Self::OutOfRange,
            ValueTestProblem::RegionUnresolved => Self::RegionUnresolved,
            ValueTestProblem::LowConfidence => Self::LowConfidence,
            ValueTestProblem::NoAgreement => Self::NoAgreement,
        }
    }
}

/// The value test problem of a parse failure.
fn parse_problem(reason: OcrFieldReason) -> ValueTestProblem {
    match reason {
        OcrFieldReason::Empty => ValueTestProblem::Empty,
        OcrFieldReason::Overflow => ValueTestProblem::Overflow,
        OcrFieldReason::OutOfRange => ValueTestProblem::OutOfRange,
        _ => ValueTestProblem::Invalid,
    }
}

/// The evaluation of `test` from its sides as read: a side that did not read fails it with
/// `value_test_unresolved` (the left side first), a comparison that does not hold with
/// `value_test_not_met`.
fn value_test_evaluation(
    test: &ValueTestTarget,
    left: SideReading,
    right: Option<RightReading>,
    reports: actingcommand_contract::PpocrDiagnostics,
) -> TargetEvaluation {
    let (right, right_value, right_problem) = match right {
        None => (None, None, None),
        Some(RightReading::Const(constant)) => (
            Some(TaskDiagnosticValueRight::Const { constant }),
            Some(i128::from(constant)),
            None,
        ),
        Some(RightReading::Operand { reading, mul, add }) => {
            let value = reading
                .value
                .map(|value| i128::from(value) * i128::from(mul) + i128::from(add));
            let problem = reading.evidence.problem;
            (
                Some(TaskDiagnosticValueRight::Operand {
                    operand: reading.evidence,
                    mul,
                    add,
                }),
                value,
                problem,
            )
        }
    };
    let unresolved = |side: ValueSide, problem: ValueTestProblem| {
        Outcome::new(RecognitionPackCode::ValueTestUnresolved)
            .target_id(test.id.as_str())
            .value_side(side)
            .value_problem(ValueProblem::from(problem))
            .stage(RecognitionPackLocation::ValueTestEvaluate)
    };
    let failure = match (left.value, right_value, right_problem) {
        (None, _, _) => Some(unresolved(
            ValueSide::Left,
            left.evidence.problem.unwrap_or(ValueTestProblem::Invalid),
        )),
        (_, _, Some(problem)) => Some(unresolved(ValueSide::Right, problem)),
        (Some(left), Some(right), None) if !test.op.holds(i128::from(left), right) => Some(
            Outcome::new(RecognitionPackCode::ValueTestNotMet)
                .target_id(test.id.as_str())
                .value_op(ValueOp::from(test.op))
                .stage(RecognitionPackLocation::ValueTestEvaluate),
        ),
        _ => None,
    };
    let passed = failure.is_none();
    TargetEvaluation {
        id: test.id.clone(),
        kind: TargetKind::ValueTest,
        sample_evaluations: Vec::new(),
        sampling: None,
        passed,
        template: None,
        color: None,
        ocr: None,
        nn: None,
        color_digest: None,
        composite: None,
        value_test: Some(Box::new(ValueTestEvaluation {
            data: TaskDiagnosticValueTestData {
                left: left.evidence,
                op: test.op,
                right,
                passed,
            },
            ppocr_diagnostics: reports,
        })),
        failure,
        uniqueness: None,
        message: if passed {
            "value test passed".to_owned()
        } else {
            "value test not passed".to_owned()
        },
    }
}

/// Workflow #411 B1 (B-train plan §2.1 items 2 and 3): the side `operand` from its values on
/// each frame of the test (oldest first): with an agreement, the one value read on at least `k`
/// of its last `frames` frames, else unread with `no_agreement` (no default, no last value);
/// without one, `current`, its read on the current frame.
fn agreed_side(
    operand: &ValueTestOperand,
    values: Vec<Option<u64>>,
    current: SideReading,
) -> SideReading {
    let Some(agreement) = operand.agreement else {
        return current;
    };
    let window = values.len().saturating_sub(agreement.frame_count());
    let value = k_of_n_value(values[window..].iter().copied(), agreement.required());
    SideReading {
        evidence: TaskDiagnosticValueOperand {
            target_id: operand.target_id.clone(),
            raw_text: None,
            value,
            problem: value.is_none().then_some(ValueTestProblem::NoAgreement),
        },
        value,
    }
}

impl RecognitionEvaluator {
    /// Evaluates a value test on the context's frame: both sides are read before the result is
    /// decided, so the evidence shows each; the left side's problem is reported first. Workflow
    /// #411 B1: a test whose side declares an agreement is evaluated on its frames.
    pub(crate) fn evaluate_value_test(
        &self,
        context: &SceneEvaluation<'_>,
        test: &ValueTestTarget,
    ) -> RecognitionPackResult<TargetEvaluation> {
        if let Some((frames, _)) = test.agreement_frames() {
            return self.evaluate_value_test_agreement(context, test, frames);
        }
        let (left, right, reports) = self.read_value_sides(context, test)?;
        Ok(value_test_evaluation(test, left, right, reports))
    }

    /// Reads the sides of `test` on the context's frame, the left side first, with the PP-OCR
    /// reports of the reads; a provider failure carries the reports of the reads before it.
    fn read_value_sides(
        &self,
        context: &SceneEvaluation<'_>,
        test: &ValueTestTarget,
    ) -> RecognitionPackResult<(
        SideReading,
        Option<RightReading>,
        actingcommand_contract::PpocrDiagnostics,
    )> {
        let mut reports = Vec::new();
        let read = (|| -> RecognitionPackResult<(SideReading, Option<RightReading>)> {
            let left = self.read_value_operand(context, &test.left, &mut reports)?;
            let right = match &test.right {
                None => None,
                Some(ValueTestRight::Const(constant)) => {
                    Some(RightReading::Const(constant.constant))
                }
                Some(ValueTestRight::Operand(operand)) => Some(RightReading::Operand {
                    reading: self.read_value_operand(context, operand, &mut reports)?,
                    mul: operand.mul.unwrap_or(1),
                    add: operand.add.unwrap_or(0),
                }),
            };
            Ok((left, right))
        })();
        match read {
            Ok((left, right)) => Ok((left, right, reports)),
            Err(mut error) => {
                reports.append(&mut error.ppocr_diagnostics);
                error.ppocr_diagnostics = reports;
                Err(error)
            }
        }
    }

    /// Workflow #411 B1 (B-train plan §2.1 items 2 and 3, register §1.163): a test whose side
    /// declares an agreement is read on the last `frames` frames of the context's samples,
    /// oldest first, both sides on each frame. Each frame's evaluation is a sample of the test,
    /// handed to the context's sample recorder (recorded as `agreement_sample`, register §1.153
    /// LR-6) or kept in `sample_evaluations`. A side with an agreement then has the one value
    /// read on at least `k` of its last `frames` frames, and without one it is unread with
    /// `no_agreement` (LR-5): its evidence carries the agreed value or that problem, the read
    /// texts staying in the frames' evidence. A side without an agreement has its read on the
    /// current frame. A provider failure on any frame is an evaluation error, as on one frame.
    fn evaluate_value_test_agreement(
        &self,
        context: &SceneEvaluation<'_>,
        test: &ValueTestTarget,
        frames: usize,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let available = context.sample_scenes.len();
        if available < frames {
            return Err(RecognitionPackError::fatal(format!(
                "target '{}' recognition samples unavailable: requires {frames} frames, got {available}",
                test.id
            )));
        }
        let scenes = &context.sample_scenes[available - frames..];
        let frame_provider_ms = self.value_test_frame_provider_ms(test);
        let mut reads = Vec::with_capacity(frames);
        let mut reports = Vec::new();
        let mut sample_evaluations = Vec::new();
        for (index, scene) in scenes.iter().enumerate() {
            let remaining_provider_ms =
                frame_provider_ms.saturating_mul(u64::try_from(frames - index).unwrap_or(u64::MAX));
            if context.sample_deadline.is_some_and(|deadline| {
                Instant::now() >= deadline
                    || Duration::from_millis(remaining_provider_ms)
                        > deadline.saturating_duration_since(Instant::now())
            }) {
                return Err(RecognitionPackError::fatal(
                    "recognition sample budget insufficient before backend evaluation",
                ));
            }
            let started = Instant::now();
            // The current frame is read in the context itself, an earlier one in its own.
            let (left, right, frame_reports) = if index + 1 == frames {
                self.read_value_sides(context, test)?
            } else {
                self.read_value_sides(&self.scene_context(scene), test)?
            };
            let ended = Instant::now();
            let mut evaluation =
                value_test_evaluation(test, left.clone(), right.clone(), frame_reports.clone());
            evaluation.sampling = Some(TaskDiagnosticSamplingData::Sample {
                sample: TaskDiagnosticSampleData {
                    frame_index: u8::try_from(index).unwrap_or(u8::MAX),
                    frame_rgb8_sha256: format!("{:x}", Sha256::digest(scene.rgb8_pixels())),
                    dx: 0,
                    dy: 0,
                    template_metric: None,
                    elapsed_us: u64::try_from(ended.duration_since(started).as_micros())
                        .unwrap_or(u64::MAX),
                    passed: Some(evaluation.passed),
                },
            });
            evaluation.message = format!(
                "value test agreement frame {index}: {}",
                if evaluation.passed {
                    "passed"
                } else {
                    "not passed"
                }
            );
            match &context.sample_recorder {
                Some(recorder) => {
                    (recorder.borrow_mut())(test.id.as_str(), &Ok(evaluation), started, ended)?;
                }
                None => sample_evaluations.push(evaluation),
            }
            reports.extend(frame_reports);
            reads.push((left, right));
        }
        let (current_left, current_right) = reads
            .last()
            .cloned()
            .ok_or_else(|| RecognitionPackError::fatal("value test agreement read no frame"))?;
        let left = agreed_side(
            &test.left,
            reads.iter().map(|(left, _)| left.value).collect(),
            current_left,
        );
        let right = match (&test.right, current_right) {
            (
                Some(ValueTestRight::Operand(operand)),
                Some(RightReading::Operand { reading, mul, add }),
            ) => {
                let values = reads
                    .iter()
                    .map(|(_, right)| match right {
                        Some(RightReading::Operand { reading, .. }) => reading.value,
                        _ => None,
                    })
                    .collect();
                Some(RightReading::Operand {
                    reading: agreed_side(operand, values, reading),
                    mul,
                    add,
                })
            }
            (_, right) => right,
        };
        // With a recorder, each frame's reports went with its sample, as for target consensus.
        let reports = if context.sample_recorder.is_none() {
            reports
        } else {
            Vec::new()
        };
        let mut evaluation = value_test_evaluation(test, left, right, reports);
        evaluation.sample_evaluations = sample_evaluations;
        Ok(evaluation)
    }

    /// Reads one side: its OCR target's region, one read per request on this frame, the declared
    /// trim, (Workflow #411 B1) the declared dictionary, the shared parser, the declared part and
    /// range, and the target's confidence floor. A side that does not read carries its problem;
    /// a provider failure is an error.
    fn read_value_operand(
        &self,
        context: &SceneEvaluation<'_>,
        operand: &ValueTestOperand,
        reports: &mut actingcommand_contract::PpocrDiagnostics,
    ) -> RecognitionPackResult<SideReading> {
        let RecognitionTarget::Ocr(target) = self.target(&operand.target_id)? else {
            return Err(RecognitionPackError::fatal(format!(
                "value test side '{}' is not an OCR target",
                operand.target_id
            )));
        };
        let mut evidence = TaskDiagnosticValueOperand {
            target_id: operand.target_id.clone(),
            raw_text: None,
            value: None,
            problem: None,
        };
        let (region, region_evidence) = match context.ocr_region(target) {
            Ok(region) => region,
            Err(error) if error.code() == RecognitionPackErrorCode::RegionUnresolved => {
                evidence.problem = Some(ValueTestProblem::RegionUnresolved);
                return Ok(SideReading {
                    evidence,
                    value: None,
                });
            }
            Err(error) => return Err(error),
        };
        let ocr = (|| {
            let provider = self.vision_provider.as_ref().ok_or_else(|| {
                RecognitionPackError::fatal_with_code(
                    RecognitionPackErrorCode::VisionProviderMissing,
                    format!("ocr target '{}' has no injected vision provider", target.id),
                )
            })?;
            provider
                .require_ocr_model(&target.model_ref, &target.model_sha256)
                .map_err(|error| provider_error(&target.id, "ocr admission", error))?;
            let result = context.read_text_once(
                provider.as_ref(),
                target,
                region,
                OcrProviderBlocks::Implicit,
            )?;
            validate_ocr_result(result, region)
        })()
        .map_err(|error: RecognitionPackError| error.with_region(&region_evidence))?;
        reports.extend(ocr.ppocr_diagnostics.iter().cloned());
        let ValueTestValue::UnsignedInteger {
            min,
            max,
            format,
            dictionary,
        } = &operand.value;
        let trimmed = operand.trim.apply(&ocr.text).into_owned();
        let text = match dictionary {
            Some(dictionary) => dictionary.apply(&trimmed),
            None => Ok(std::borrow::Cow::Borrowed(trimmed.as_str())),
        };
        let read = text
            .and_then(|text| format.parse_parts(&text))
            .map_err(parse_problem)
            .and_then(|parts| {
                let value = match operand.part {
                    Some(ValueTestPart::Capacity) => {
                        parts.capacity.ok_or(ValueTestProblem::Invalid)?
                    }
                    Some(ValueTestPart::Current) | None => parts.value,
                };
                if (*min..=*max).contains(&value) {
                    Ok(value)
                } else {
                    Err(ValueTestProblem::OutOfRange)
                }
            })
            .and_then(|value| {
                if ocr
                    .confidence
                    .is_some_and(|confidence| confidence >= target.minimum_confidence)
                {
                    Ok(value)
                } else {
                    Err(ValueTestProblem::LowConfidence)
                }
            });
        evidence.raw_text = Some(ocr.text);
        match read {
            Ok(value) => evidence.value = Some(value),
            Err(problem) => evidence.problem = Some(problem),
        }
        Ok(SideReading {
            value: evidence.value,
            evidence,
        })
    }

    /// The provider time a value test may spend: each side's OCR timeout, on each frame it
    /// reads (Workflow #411 B1: each of its agreement's frames, else one).
    pub(crate) fn value_test_provider_ms(&self, test: &ValueTestTarget) -> u64 {
        let frames = test
            .agreement_frames()
            .map_or(1, |(frames, _)| u64::try_from(frames).unwrap_or(u64::MAX));
        self.value_test_frame_provider_ms(test)
            .saturating_mul(frames)
    }

    /// The provider time of one frame's reads of a value test: each side's OCR timeout.
    fn value_test_frame_provider_ms(&self, test: &ValueTestTarget) -> u64 {
        let timeout = |target_id: &str| match self.target(target_id) {
            Ok(RecognitionTarget::Ocr(target)) => target.timeout_ms,
            _ => 0,
        };
        let right = match &test.right {
            Some(ValueTestRight::Operand(operand)) => timeout(&operand.target_id),
            _ => 0,
        };
        timeout(&test.left.target_id).saturating_add(right)
    }

    /// Whether `target_id` tests values: a value test, or a check with one as member.
    pub fn tests_values(&self, target_id: &str) -> bool {
        tests_values(&self.pack, target_id)
    }

    /// Whether a page recognition evaluates `target_id` lazily (Workflow #396 REC-6): a locate
    /// OCR target, a value test, or a check with one of them as member. Every other target is
    /// evaluated first.
    pub fn evaluates_lazily(&self, target_id: &str) -> bool {
        let lazy = |id: &str| {
            matches!(
                self.target(id),
                Ok(RecognitionTarget::ValueTest(_)
                    | RecognitionTarget::Ocr(OcrTarget {
                        locate: Some(_),
                        ..
                    }))
            )
        };
        lazy(target_id)
            || matches!(
                self.target(target_id),
                Ok(RecognitionTarget::Composite(composite))
                    if composite.members.iter().any(|member| lazy(member))
            )
    }
}

/// Whether `target_id` is a value test, or a check with one as member.
fn tests_values(pack: &RecognitionPack, target_id: &str) -> bool {
    let value_test = |id: &str| {
        pack.targets
            .iter()
            .any(|target| matches!(target, RecognitionTarget::ValueTest(test) if test.id == id))
    };
    value_test(target_id)
        || pack.targets.iter().any(|target| {
            matches!(
                target,
                RecognitionTarget::Composite(composite)
                    if composite.id == target_id
                        && composite.members.iter().any(|member| value_test(member))
            )
        })
}

/// The declaration rules of value tests (`contracts/resource-declarations.md`, section Task
/// schema `0.9` value tests). The first refused declaration is returned with its pointer.
pub(crate) fn validate_value_test_declarations(
    pack: &RecognitionPack,
) -> Result<(), LocateRefusal> {
    for (index, target) in pack.targets.iter().enumerate() {
        let RecognitionTarget::ValueTest(test) = target else {
            continue;
        };
        let base = format!("/targets/{index}");
        validate_operand(pack, &test.left, &format!("{base}/left"), false)?;
        match (test.op, &test.right) {
            (ValueTestOp::Parses, Some(_)) => refusal(
                format!("{base}/right"),
                ResourceDeclarationReason::UnconsumedField,
                "op 'parses' reads the left side only",
            )?,
            (ValueTestOp::Parses, None) => {}
            (_, None) => refusal(
                format!("{base}/right"),
                ResourceDeclarationReason::MissingField,
                "a comparison requires its right side",
            )?,
            (_, Some(ValueTestRight::Const(constant))) => {
                if constant.constant > VALUE_TEST_MAX_EXACT {
                    refusal(
                        format!("{base}/right/const"),
                        ResourceDeclarationReason::InvalidValue,
                        "a constant lies in 0..=2^53-1",
                    )?;
                }
            }
            (_, Some(ValueTestRight::Operand(operand))) => {
                validate_operand(pack, operand, &format!("{base}/right"), true)?;
            }
        }
    }
    for id in pack.target_consensus.keys() {
        if tests_values(pack, id) {
            refusal(
                format!("/target_consensus/{}", pointer_token(id)),
                ResourceDeclarationReason::InvalidValue,
                "a value test cannot declare target consensus",
            )?;
        }
    }
    // Workflow #411 B1: an agreement takes its frames from the pack's one sampling plan, so the
    // agreements and the multi-frame target consensus of one pack share one frame interval.
    let mut interval = pack
        .target_consensus
        .values()
        .find(|declaration| declaration.required_frames() > 1)
        .map(|declaration| u64::from(declaration.sample_interval_ms));
    for (index, target) in pack.targets.iter().enumerate() {
        let RecognitionTarget::ValueTest(test) = target else {
            continue;
        };
        let right = match &test.right {
            Some(ValueTestRight::Operand(operand)) => Some(operand),
            _ => None,
        };
        for (side, operand) in [("left", Some(&test.left)), ("right", right)] {
            let Some(agreement) = operand.and_then(|operand| operand.agreement) else {
                continue;
            };
            match interval {
                None => interval = Some(agreement.interval_ms),
                Some(shared) if shared != agreement.interval_ms => refusal(
                    format!("/targets/{index}/{side}/agreement/interval_ms"),
                    ResourceDeclarationReason::InvalidValue,
                    "the agreements and multi-frame target consensus of one pack share one interval",
                )?,
                Some(_) => {}
            }
        }
    }
    for (layout_index, layout) in pack.candidate_layouts.iter().enumerate() {
        for (slot_index, slot) in layout.slots.iter().enumerate() {
            for (name, target_id) in &slot.targets {
                if tests_values(pack, target_id) {
                    refusal(
                        format!(
                            "/candidate_layouts/{layout_index}/slots/{slot_index}/targets/{}",
                            pointer_token(name)
                        ),
                        ResourceDeclarationReason::InvalidValue,
                        "a value test cannot be a candidate layout slot target or feature",
                    )?;
                }
            }
        }
        for (feature_index, feature) in layout.features.iter().enumerate() {
            if let Some(target_id) = &feature.target
                && tests_values(pack, target_id)
            {
                refusal(
                    format!("/candidate_layouts/{layout_index}/features/{feature_index}/target"),
                    ResourceDeclarationReason::InvalidValue,
                    "a value test cannot be a candidate layout slot target or feature",
                )?;
            }
        }
    }
    Ok(())
}

/// One side: an ordinary OCR target (no `locate`, no consensus), a range with `min <= max`, a
/// `part` exactly with `current_capacity`, and `mul` (1..=1,000,000) and `add` (|add| <=
/// 2^53-1) exactly on the right side; (Workflow #411 B1) a `dictionary` and an `agreement` that
/// follow their declaration rules (`IntegerDictionary::refused_member`,
/// `IntegerAgreement::refused_member`).
fn validate_operand(
    pack: &RecognitionPack,
    operand: &ValueTestOperand,
    pointer: &str,
    right: bool,
) -> Result<(), LocateRefusal> {
    let ordinary_ocr = pack.targets.iter().any(|target| {
        matches!(
            target,
            RecognitionTarget::Ocr(OcrTarget { id, locate: None, .. }) if *id == operand.target_id
        )
    });
    if !ordinary_ocr {
        refusal(
            format!("{pointer}/target_id"),
            ResourceDeclarationReason::InvalidValue,
            "a value test side reads an OCR target without locate",
        )?;
    }
    if pack.target_consensus.contains_key(&operand.target_id) {
        refusal(
            format!("{pointer}/target_id"),
            ResourceDeclarationReason::InvalidValue,
            "a value test side reads a target without target consensus",
        )?;
    }
    let ValueTestValue::UnsignedInteger {
        min,
        max,
        format,
        dictionary,
    } = &operand.value;
    if min > max {
        refusal(
            format!("{pointer}/value/min"),
            ResourceDeclarationReason::InvalidValue,
            "min must not exceed max",
        )?;
    }
    if let Some(member) = dictionary
        .as_ref()
        .and_then(IntegerDictionary::refused_member)
    {
        refusal(
            format!("{pointer}/value/dictionary{member}"),
            ResourceDeclarationReason::InvalidValue,
            "a dictionary declares 1..=32 confusions of one character to one ASCII digit, none \
             repeated, affixes of 1..=64 bytes and digits within 1..=20",
        )?;
    }
    if let Some(member) = operand
        .agreement
        .and_then(|agreement| agreement.refused_member(false))
    {
        refusal(
            format!("{pointer}/agreement{member}"),
            ResourceDeclarationReason::InvalidValue,
            "an agreement declares 2..=5 frames, k within 1..=frames and interval_ms within \
             1..=1000",
        )?;
    }
    match (*format, operand.part) {
        (OcrUnsignedIntegerFormat::CurrentCapacity, None) => refusal(
            format!("{pointer}/part"),
            ResourceDeclarationReason::MissingField,
            "format 'current_capacity' requires part",
        )?,
        (OcrUnsignedIntegerFormat::CurrentCapacity, Some(_)) | (_, None) => {}
        (_, Some(_)) => refusal(
            format!("{pointer}/part"),
            ResourceDeclarationReason::UnconsumedField,
            "part is declared only with format 'current_capacity'",
        )?,
    }
    match (right, operand.mul, operand.add) {
        (false, Some(_), _) => refusal(
            format!("{pointer}/mul"),
            ResourceDeclarationReason::UnconsumedField,
            "mul is declared on the right side only",
        )?,
        (false, None, Some(_)) => refusal(
            format!("{pointer}/add"),
            ResourceDeclarationReason::UnconsumedField,
            "add is declared on the right side only",
        )?,
        (false, None, None) => {}
        (true, None, _) => refusal(
            format!("{pointer}/mul"),
            ResourceDeclarationReason::MissingField,
            "the right side declares mul",
        )?,
        (true, Some(_), None) => refusal(
            format!("{pointer}/add"),
            ResourceDeclarationReason::MissingField,
            "the right side declares add",
        )?,
        (true, Some(mul), Some(add)) => {
            if !(1..=VALUE_TEST_MAX_MUL).contains(&mul) {
                refusal(
                    format!("{pointer}/mul"),
                    ResourceDeclarationReason::InvalidValue,
                    "mul lies in 1..=1000000",
                )?;
            }
            if add.unsigned_abs() > VALUE_TEST_MAX_EXACT {
                refusal(
                    format!("{pointer}/add"),
                    ResourceDeclarationReason::InvalidValue,
                    "add lies in -(2^53-1)..=2^53-1",
                )?;
            }
        }
    }
    Ok(())
}

/// The wire shape of a schema 0.7 `value_test` target's sides, before the typed declaration
/// checks the values: each side's members, the right side's `const` form, and the JSON types
/// serde would otherwise refuse without a pointer.
pub(crate) fn validate_value_test_wire(
    object: &Map<String, Value>,
    index: usize,
) -> RecognitionPackResult<()> {
    let base = format!("/targets/{index}");
    let invalid_type = |pointer: String| {
        RecognitionPackError::fatal(format!("{pointer} has the wrong type"))
            .at_declaration(pointer, ResourceDeclarationReason::InvalidType)
    };
    let missing = |pointer: String| {
        RecognitionPackError::fatal(format!("{pointer} is missing"))
            .at_declaration(pointer, ResourceDeclarationReason::MissingField)
    };
    if !object.get("op").is_some_and(Value::is_string) {
        return Err(match object.get("op") {
            None => missing(format!("{base}/op")),
            Some(_) => invalid_type(format!("{base}/op")),
        });
    }
    // Workflow #411 B1: a dictionary's or an agreement's refusal at the member it names.
    let refused = |pointer: String, reason: ResourceDeclarationReason| {
        RecognitionPackError::fatal(format!("{pointer} is refused")).at_declaration(pointer, reason)
    };
    let operand = |value: &Value, pointer: &str| -> RecognitionPackResult<()> {
        let side = validate_strict_object(
            value,
            &[
                "target_id",
                "trim",
                "value",
                "part",
                "mul",
                "add",
                "agreement",
            ],
            pointer,
        )?;
        for field in ["target_id", "trim", "value"] {
            if !side.contains_key(field) {
                return Err(missing(format!("{pointer}/{field}")));
            }
        }
        for field in ["target_id", "trim", "part"] {
            if side.get(field).is_some_and(|value| !value.is_string()) {
                return Err(invalid_type(format!("{pointer}/{field}")));
            }
        }
        if side.get("mul").is_some_and(|value| !value.is_u64()) {
            return Err(invalid_type(format!("{pointer}/mul")));
        }
        if side.get("add").is_some_and(|value| !value.is_i64()) {
            return Err(invalid_type(format!("{pointer}/add")));
        }
        if let Some(agreement) = side.get("agreement")
            && let Some((member, reason)) =
                actingcommand_contract::integer_agreement_refusal(agreement, false)
        {
            return Err(refused(format!("{pointer}/agreement{member}"), reason));
        }
        let value_pointer = format!("{pointer}/value");
        let value = validate_strict_object(
            &side["value"],
            &["type", "min", "max", "format", "dictionary"],
            &value_pointer,
        )?;
        if let Some(dictionary) = value.get("dictionary")
            && let Some((member, reason)) =
                actingcommand_contract::integer_dictionary_refusal(dictionary)
        {
            return Err(refused(
                format!("{value_pointer}/dictionary{member}"),
                reason,
            ));
        }
        if value.get("type").and_then(Value::as_str) != Some("unsigned_integer") {
            return Err(RecognitionPackError::fatal(format!(
                "{value_pointer}/type must be 'unsigned_integer'"
            ))
            .at_declaration(
                format!("{value_pointer}/type"),
                ResourceDeclarationReason::InvalidValue,
            ));
        }
        for field in ["min", "max"] {
            match value.get(field) {
                None => return Err(missing(format!("{value_pointer}/{field}"))),
                Some(bound) if !bound.is_u64() => {
                    return Err(invalid_type(format!("{value_pointer}/{field}")));
                }
                Some(_) => {}
            }
        }
        Ok(())
    };
    match object.get("left") {
        None => return Err(missing(format!("{base}/left"))),
        Some(left) => operand(left, &format!("{base}/left"))?,
    }
    if let Some(right) = object.get("right") {
        let pointer = format!("{base}/right");
        if right.get("const").is_some() {
            let constant = validate_strict_object(right, &["const"], &pointer)?;
            if !constant["const"].is_u64() {
                return Err(invalid_type(format!("{pointer}/const")));
            }
        } else {
            operand(right, &pointer)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "value_test_tests.rs"]
mod tests;
