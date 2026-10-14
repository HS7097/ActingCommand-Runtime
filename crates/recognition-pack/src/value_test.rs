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

use crate::codes::{
    RecognitionPackCode, RecognitionPackLocation, ValueOp, ValueProblem, ValueSide,
};
use crate::text_locate::{LocateRefusal, pointer_token, refuse as refusal};
use crate::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    OcrFieldReason, OcrUnsignedIntegerFormat, OcrValueTrim, ResourceDeclarationReason,
    TaskDiagnosticValueOperand, TaskDiagnosticValueRight, TaskDiagnosticValueTestData, ValueTestOp,
    ValueTestProblem,
};

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
}

/// The value a side reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueTestValue {
    UnsignedInteger {
        min: u64,
        max: u64,
        #[serde(default)]
        format: OcrUnsignedIntegerFormat,
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
struct SideReading {
    evidence: TaskDiagnosticValueOperand,
    value: Option<u64>,
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

impl RecognitionEvaluator {
    /// Evaluates a value test on the context's frame: both sides are read before the result is
    /// decided, so the evidence shows each; the left side's problem is reported first.
    pub(crate) fn evaluate_value_test(
        &self,
        context: &SceneEvaluation<'_>,
        test: &ValueTestTarget,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let mut reports = Vec::new();
        let read = |operand: &ValueTestOperand,
                    reports: &mut actingcommand_contract::PpocrDiagnostics| {
            self.read_value_operand(context, operand, reports)
        };
        let outcome = (|| {
            let left = read(&test.left, &mut reports)?;
            let right = match &test.right {
                None => None,
                Some(ValueTestRight::Const(constant)) => Some((
                    TaskDiagnosticValueRight::Const {
                        constant: constant.constant,
                    },
                    Some(i128::from(constant.constant)),
                    None,
                )),
                Some(ValueTestRight::Operand(operand)) => {
                    let reading = read(operand, &mut reports)?;
                    let (mul, add) = (operand.mul.unwrap_or(1), operand.add.unwrap_or(0));
                    let problem = reading.evidence.problem;
                    let value = reading
                        .value
                        .map(|value| i128::from(value) * i128::from(mul) + i128::from(add));
                    Some((
                        TaskDiagnosticValueRight::Operand {
                            operand: reading.evidence,
                            mul,
                            add,
                        },
                        value,
                        problem,
                    ))
                }
            };
            Ok((left, right))
        })();
        let (left, right) = outcome.map_err(|mut error: RecognitionPackError| {
            reports.append(&mut error.ppocr_diagnostics);
            error.ppocr_diagnostics = std::mem::take(&mut reports);
            error
        })?;

        let unresolved = |side: ValueSide, problem: ValueTestProblem| {
            Outcome::new(RecognitionPackCode::ValueTestUnresolved)
                .target_id(test.id.as_str())
                .value_side(side)
                .value_problem(ValueProblem::from(problem))
                .stage(RecognitionPackLocation::ValueTestEvaluate)
        };
        let failure = match (&left, &right) {
            (SideReading { value: None, .. }, _) => Some(unresolved(
                ValueSide::Left,
                left.evidence.problem.unwrap_or(ValueTestProblem::Invalid),
            )),
            (_, Some((_, _, Some(problem)))) => Some(unresolved(ValueSide::Right, *problem)),
            (
                SideReading {
                    value: Some(left), ..
                },
                Some((_, Some(right), None)),
            ) if !test.op.holds(i128::from(*left), *right) => Some(
                Outcome::new(RecognitionPackCode::ValueTestNotMet)
                    .target_id(test.id.as_str())
                    .value_op(ValueOp::from(test.op))
                    .stage(RecognitionPackLocation::ValueTestEvaluate),
            ),
            _ => None,
        };
        let passed = failure.is_none();
        Ok(TargetEvaluation {
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
                    right: right.map(|(right, _, _)| right),
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
        })
    }

    /// Reads one side: its OCR target's region, one read per request on this frame, the declared
    /// trim, the shared parser, the declared part and range, and the target's confidence floor.
    /// A side that does not read carries its problem; a provider failure is an error.
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
        let ValueTestValue::UnsignedInteger { min, max, format } = operand.value;
        let trimmed = operand.trim.apply(&ocr.text).into_owned();
        let read = format
            .parse_parts(&trimmed)
            .map_err(parse_problem)
            .and_then(|parts| {
                let value = match operand.part {
                    Some(ValueTestPart::Capacity) => {
                        parts.capacity.ok_or(ValueTestProblem::Invalid)?
                    }
                    Some(ValueTestPart::Current) | None => parts.value,
                };
                if (min..=max).contains(&value) {
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

    /// The provider time a value test may spend: each side's OCR timeout.
    pub(crate) fn value_test_provider_ms(&self, test: &ValueTestTarget) -> u64 {
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
/// 2^53-1) exactly on the right side.
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
    let ValueTestValue::UnsignedInteger { min, max, format } = operand.value;
    if min > max {
        refusal(
            format!("{pointer}/value/min"),
            ResourceDeclarationReason::InvalidValue,
            "min must not exceed max",
        )?;
    }
    match (format, operand.part) {
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
    let operand = |value: &Value, pointer: &str| -> RecognitionPackResult<()> {
        let side = validate_strict_object(
            value,
            &["target_id", "trim", "value", "part", "mul", "add"],
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
        let value_pointer = format!("{pointer}/value");
        let value = validate_strict_object(
            &side["value"],
            &["type", "min", "max", "format"],
            &value_pointer,
        )?;
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
