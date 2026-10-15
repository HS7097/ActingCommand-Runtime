// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #308 slice SEL-L4b (frozen selector model S0 v2.1 §2.2 `Decide`, `Return` and
//! `Confirm`, §3.2–§3.4): what a scan select does once its coverage checks passed, and the v2
//! projection its record carries.
//!
//! `Decide` numbers the members `<layout>#NN` in first-seen order and evaluates them once, with
//! the pinned snapshot and instant. The unknown instances then decide whether the decision
//! stands (§3.2): an `empty` or `insufficient` decision with an unknown instance is blocked, and
//! so is a `selected` one that an unknown instance might outrank (its upper bound above the
//! chosen score). An `empty` decision over complete coverage with nothing unknown ends the step
//! normally when it declares `on_empty`. A chosen member is brought back into the band by the
//! declared moves (`Return`), every settled frame aligned directly against the content map, and
//! is confirmed on a fresh frame F2 by its business identity at its content position
//! (`Confirm`). Every path after the decision maps to exactly one confirmation variant (§3.4).

use super::{
    AxisRect, Bound, PositionState, ScanBound, ScanDragMotion, ScanEnd, ScanReading, ScanSession,
    ScanSource, Verdict, bounds_exceeded, candidate_fields, u32_of,
};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use crate::contained_task::{ContainedTaskError, ContainedTaskRunError};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CANDIDATE_PROJECTION_BUDGET_EXCEEDED, CandidateFeature, CandidateFrame, CandidateRect,
    CandidateUnknownReason, SCAN_PROJECTION_MAX_CANDIDATES, ScanAxis, ScanCandidate,
    ScanCandidateProjection, ScanCandidateProjectionParts, ScanCoverage, ScanCoverageState,
    ScanEndEvidence, ScanGatedOut, ScanReposition, ScanUnknownCause, ScanUnknownPosition,
    ScanViewport, TaskSelectionBlocking, TaskSelectionConfirmation, TaskSelectionIdentityState,
    TaskSelectionRepositionStage, TaskSelectionUnknownCheck, TaskSelectionUpperBound,
};
use actingcommand_recognition_pack::{CandidateFeatureValue, CandidateLayout};
use actingcommand_selection_policy::{
    Candidate, FieldDomain, SelectionDecision, SelectionOutcome, SelectionPolicy, upper_bound,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

actingcommand_contract::outcome_vocabulary! {
    /// The evaluator's outcome kind, verbatim (the vocabulary `selection_kind`, S0 §2.7).
    pub(in crate::contained_task) enum SelectionKind: "selection_kind" {
        Selected => "selected",
        Empty => "empty",
        Insufficient => "insufficient",
        Ambiguous => "ambiguous",
        Unknown => "unknown",
    }
}

impl SelectionKind {
    pub(in crate::contained_task) const fn of(outcome: &SelectionOutcome) -> Self {
        match outcome {
            SelectionOutcome::Selected { .. } => Self::Selected,
            SelectionOutcome::Empty => Self::Empty,
            SelectionOutcome::Insufficient { .. } => Self::Insufficient,
            SelectionOutcome::Ambiguous { .. } => Self::Ambiguous,
            SelectionOutcome::Unknown { .. } => Self::Unknown,
        }
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why a decision that unknown instances could change did not stand (the vocabulary
    /// `selection_blocking`, S0 §2.7, rules 2 and 3 of §3.2).
    pub(in crate::contained_task) enum SelectionBlocking: "selection_blocking" {
        UnknownInstances => "unknown_instances",
        OutrankedPossible => "outranked_possible",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// How a confirmed identity differed from the chosen one (the vocabulary `identity_state`,
    /// S0 §2.7): no instance at the chosen position, Unknown until the timeout, another Known
    /// identity, or (`expect_identity`) no choice in this run.
    pub(in crate::contained_task) enum IdentityState: "identity_state" {
        Absent => "absent",
        Unknown => "unknown",
        Different => "different",
        NoChoice => "no_choice",
    }
}

/// How a select over a scan ended once its decision existed (S0 §3.4, P2–P12).
pub(in crate::contained_task) enum ScanSelectEnd<E> {
    /// P11: the fresh frame F2 showed the chosen member, with the chosen business ID, at `rect`
    /// (F2's frame coordinates); the tap goes into its click rectangle.
    Confirmed {
        identity: String,
        rect: CandidateRect,
    },
    /// P4: complete coverage, no member, no unknown instance, and the step declares `on_empty`.
    Empty,
    /// Every other path: the failure the task returns once the record is written.
    Failed(ContainedTaskRunError<E>),
}

/// How `Return` and `Confirm` ended: the record's `confirmation`, and how the attempt ends.
type Confirmation<E> = (TaskSelectionConfirmation, ScanSelectEnd<E>);

/// The decision of a select over a scan, the members of its record besides the projection, and
/// how the attempt ends.
pub(in crate::contained_task) struct ScanSelection<E> {
    pub(in crate::contained_task) decision: SelectionDecision,
    pub(in crate::contained_task) coverage: ScanCoverageState,
    pub(in crate::contained_task) unknown_check: TaskSelectionUnknownCheck,
    pub(in crate::contained_task) confirmation: TaskSelectionConfirmation,
    pub(in crate::contained_task) end: ScanSelectEnd<E>,
}

/// Why [`bound_inputs`] refused: the `key=value` detail its caller reports under
/// `contained_task_select_invalid`.
#[derive(Debug)]
pub(in crate::contained_task) struct BoundInputsRefusal(String);

impl From<String> for BoundInputsRefusal {
    fn from(detail: String) -> Self {
        Self(detail)
    }
}

impl std::fmt::Display for BoundInputsRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The upper-bound inputs of a select step (S0 §3.3): every policy field's domain, built from
/// the layout (`passed` boolean, `measure_milli` 0..=1000, `ocr_integer` its declared range, the
/// identity feature the identity itself), and the identities a position without a Known one may
/// hold: the identity feature's domain, or one neutral hypothesis when no field carries an
/// identity. Another identity field is refused, because one hypothesis fixes one identity.
pub(in crate::contained_task) fn bound_inputs(
    layout: &CandidateLayout,
    policy: &SelectionPolicy,
    identity_feature: Option<&str>,
) -> Result<(BTreeMap<String, FieldDomain>, Vec<String>), BoundInputsRefusal> {
    let mut field_domains = BTreeMap::new();
    for field in &policy.fields {
        let feature = layout
            .features
            .iter()
            .find(|feature| feature.name == field.name)
            .ok_or_else(|| format!("field={} undeclared", field.name))?;
        let domain = match feature.value {
            CandidateFeatureValue::Passed => FieldDomain::Boolean,
            CandidateFeatureValue::MeasureMilli => FieldDomain::Integer { min: 0, max: 1000 },
            CandidateFeatureValue::OcrInteger => {
                let integer = feature
                    .integer
                    .ok_or_else(|| format!("field={} range=absent", field.name))?;
                let bound = |value: u64| {
                    i64::try_from(value)
                        .map_err(|_| format!("field={} range=oversized", field.name))
                };
                FieldDomain::Integer {
                    min: bound(integer.min)?,
                    max: bound(integer.max)?,
                }
            }
            CandidateFeatureValue::Identity if Some(feature.name.as_str()) == identity_feature => {
                FieldDomain::Identity
            }
            CandidateFeatureValue::Identity => {
                return Err(format!("field={} identity=not_the_step_identity", field.name).into());
            }
        };
        field_domains.insert(field.name.clone(), domain);
    }
    let identities = match identity_feature {
        Some(name) => layout
            .features
            .iter()
            .find(|feature| feature.name == name)
            .and_then(|feature| feature.identity.as_ref())
            .map(|identity| {
                identity
                    .entries
                    .iter()
                    .map(|entry| entry.id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            })
            .ok_or_else(|| format!("identity_feature={name} undeclared"))?,
        None => vec![String::new()],
    };
    Ok((field_domains, identities))
}

/// The record's bound of the unknown instances: `none_unknown` without one.
pub(in crate::contained_task) const fn record_bound(
    upper: Option<Bound>,
) -> TaskSelectionUpperBound {
    match upper {
        None => TaskSelectionUpperBound::NoneUnknown,
        Some(Bound::None) => TaskSelectionUpperBound::NoneEligible,
        Some(Bound::Max(milli)) => TaskSelectionUpperBound::Max { milli },
        Some(Bound::Unbounded) => TaskSelectionUpperBound::Unbounded,
    }
}

/// Rules 2 and 3 of S0 §3.2: whether unknown instances block the decision. An `empty` or
/// `insufficient` decision with an unknown instance is blocked; a `selected` one is blocked when
/// the bound over the unknown instances is unbounded or above the chosen score (strictly, so a
/// tie never blocks).
pub(in crate::contained_task) fn decision_blocking(
    decision: &SelectionDecision,
    upper: Option<Bound>,
    unknown_instances: u32,
) -> Result<TaskSelectionBlocking, ContainedTaskError> {
    Ok(match &decision.outcome {
        SelectionOutcome::Empty | SelectionOutcome::Insufficient { .. }
            if unknown_instances >= u32::MAX =>
        {
            TaskSelectionBlocking::UnknownInstances
        }
        SelectionOutcome::Selected { .. } => {
            let score = chosen_score(decision)?;
            match upper {
                Some(Bound::Unbounded) => TaskSelectionBlocking::OutrankedPossible,
                Some(Bound::Max(milli)) if milli > score.saturating_add(1_000_000) => {
                    TaskSelectionBlocking::OutrankedPossible
                }
                _ => TaskSelectionBlocking::None,
            }
        }
        _ => TaskSelectionBlocking::None,
    })
}

/// The score of the one chosen candidate's verdict.
fn chosen_score(decision: &SelectionDecision) -> Result<i64, ContainedTaskError> {
    let chosen = decision.selected.first();
    decision
        .candidates
        .iter()
        .find(|verdict| Some(&verdict.candidate_id) == chosen)
        .and_then(|verdict| verdict.score_milli)
        .ok_or_else(|| {
            ContainedTaskError::with_detail(
                ExecutionKernelCode::SelectionEvaluationFailed.code(),
                "selected candidate has no score",
            )
        })
}

/// `selection_result_blocked` (S0 §2.7) at `Decide`: the evaluator's own kind and key, why it
/// did not stand, and the number of unknown instances.
pub(in crate::contained_task) fn result_blocked(
    decision: &SelectionDecision,
    blocking: TaskSelectionBlocking,
    unknown_instances: u32,
) -> ContainedTaskError {
    let blocking = match blocking {
        TaskSelectionBlocking::OutrankedPossible => SelectionBlocking::OutrankedPossible,
        TaskSelectionBlocking::UnknownInstances | TaskSelectionBlocking::None => {
            SelectionBlocking::UnknownInstances
        }
    };
    let kind = SelectionKind::of(&decision.outcome);
    let outcome_key = decision.outcome_key.clone();
    ContainedTaskError::valued(
        ExecutionKernelCode::SelectionResultBlocked.code(),
        |outcome| {
            outcome
                .stage(ExecutionKernelLocation::SelectDecide)
                .selection_kind(kind)
                .outcome_key(outcome_key)
                .selection_blocking(blocking)
                .count(unknown_instances)
        },
    )
}

/// `selection_not_selected`, as a single-frame select reports it (its detail `<kind>:<key>`).
fn not_selected(decision: &SelectionDecision) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        ExecutionKernelCode::SelectionNotSelected.code(),
        format!(
            "{}:{}",
            SelectionKind::of(&decision.outcome).as_str(),
            decision.outcome_key
        ),
    )
}

/// `selection_identity_unconfirmed` (S0 §2.7) at `Confirm`.
fn identity_unconfirmed(
    expected: &str,
    state: IdentityState,
    observed: Option<&str>,
) -> ContainedTaskError {
    let (expected, observed) = (expected.to_owned(), observed.map(str::to_owned));
    ContainedTaskError::valued(
        ExecutionKernelCode::SelectionIdentityUnconfirmed.code(),
        |outcome| {
            let outcome = outcome
                .stage(ExecutionKernelLocation::SelectConfirm)
                .expected_identity(expected)
                .identity_state(state);
            match observed {
                Some(observed) => outcome.observed_identity(observed),
                None => outcome,
            }
        },
    )
}

/// The outcome a record keeps for a failure after the decision: the task error's own, or, for
/// a failure of the runtime itself, the confirmation capture's (as a single-frame select keeps
/// it).
fn failure_outcome<E>(error: &ContainedTaskRunError<E>) -> Outcome {
    match error {
        ContainedTaskRunError::Task(error) => error.outcome(),
        ContainedTaskRunError::Boundary(_) | ContainedTaskRunError::NonfatalOperation(_) => {
            Outcome::new(ExecutionKernelCode::SelectionConfirmationCaptureFailed.code())
        }
    }
}

/// What one F2 shows at the chosen position.
enum F2Identity {
    Matched,
    Absent,
    Unknown(Option<CandidateUnknownReason>),
    Different(String),
}

/// The SHA-256, as 64 lowercase hexadecimal digits, of the compact JSON array of the gated-out
/// business IDs in ascending order.
fn identities_sha256(identities: &[String]) -> Result<String, ContainedTaskError> {
    let bytes = serde_json::to_vec(identities).map_err(|error| {
        ContainedTaskError::with_detail(
            ExecutionKernelCode::SelectionEvaluationFailed.code(),
            format!("decision record: {error}"),
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// A rectangle split along the axis, back in frame (or content) x and y.
fn rect_of(axis: ScanAxis, rect: AxisRect) -> Result<CandidateRect, ContainedTaskError> {
    let narrow = |value: i64| {
        i32::try_from(value).map_err(|_| {
            ContainedTaskError::with_detail(
                ExecutionKernelCode::SelectionEvaluationFailed.code(),
                "scan content rectangle out of range",
            )
        })
    };
    let (x, y, width, height) = match axis {
        ScanAxis::Vertical => (rect.cross, rect.axis, rect.cross_extent, rect.axis_extent),
        ScanAxis::Horizontal => (rect.axis, rect.cross, rect.axis_extent, rect.cross_extent),
    };
    Ok(CandidateRect {
        x: narrow(x)?,
        y: narrow(y)?,
        width: narrow(width)?,
        height: narrow(height)?,
    })
}

impl ScanSession<'_> {
    /// The select step over this scan (S0 §2.2): the session from `Pin` to its coverage checks,
    /// then `Decide`, and for a selected member `Return` and `Confirm`. A failure before the
    /// decision exists returns as the error, with no record (P1); every later end is a
    /// [`ScanSelection`], whose record precedes the return. `on_empty` says whether the step
    /// declares `on_empty`.
    pub(in crate::contained_task) fn select<S: ScanSource>(
        &mut self,
        source: &mut S,
        on_empty: bool,
    ) -> Result<ScanSelection<S::Error>, ContainedTaskRunError<S::Error>> {
        let (decision, coverage) = match self.run(source)? {
            // P2 and P3: the evaluator's abort is the decision; nothing blocks it and nothing
            // is confirmed.
            ScanEnd::Aborted { decision, coverage } => {
                let unknown = self.unknown_positions();
                let upper = self.unknown_bound(&unknown)?;
                let failure = not_selected(&decision);
                return Ok(ScanSelection {
                    decision: *decision,
                    coverage,
                    unknown_check: TaskSelectionUnknownCheck {
                        unknown_instances: u32_of(unknown.len()),
                        blocking: TaskSelectionBlocking::None,
                        upper_bound: record_bound(upper),
                    },
                    confirmation: TaskSelectionConfirmation::NotAttempted,
                    end: ScanSelectEnd::Failed(failure.into()),
                });
            }
            ScanEnd::Covered => (self.decide()?, ScanCoverageState::Complete),
            ScanEnd::EarlyStopped => (self.decide()?, ScanCoverageState::EarlyStopped),
        };
        let unknown = self.unknown_positions();
        let unknown_instances = u32_of(unknown.len());
        let upper = self.unknown_bound(&unknown)?;
        let blocking = decision_blocking(&decision, upper, unknown_instances)?;
        let unknown_check = TaskSelectionUnknownCheck {
            unknown_instances,
            blocking,
            upper_bound: record_bound(upper),
        };
        let not_attempted = |end| (TaskSelectionConfirmation::NotAttempted, end);
        let (confirmation, end) = if blocking != TaskSelectionBlocking::None {
            // P6 and P7.
            not_attempted(ScanSelectEnd::Failed(
                result_blocked(&decision, blocking, unknown_instances).into(),
            ))
        } else {
            match (&decision.outcome, decision.selected.as_slice()) {
                // P4: rule 1 with `on_empty`; nothing is unknown, or the decision was blocked.
                (SelectionOutcome::Empty, _) if on_empty => not_attempted(ScanSelectEnd::Empty),
                (SelectionOutcome::Selected { .. }, [chosen]) => {
                    let chosen = chosen.clone();
                    self.confirm_member(source, &chosen)?
                }
                (SelectionOutcome::Selected { count }, _) => {
                    return Err(ContainedTaskError::with_detail(
                        ExecutionKernelCode::SelectionEvaluationFailed.code(),
                        format!("selected count={count} where one candidate is required"),
                    )
                    .into());
                }
                // P5.
                _ => not_attempted(ScanSelectEnd::Failed(not_selected(&decision).into())),
            }
        };
        Ok(ScanSelection {
            decision,
            coverage,
            unknown_check,
            confirmation,
            end,
        })
    }

    /// `Decide`'s evaluation (S0 §2.2, §2.6): the members, numbered `<layout>#NN` in first-seen
    /// order, evaluated once with the pinned snapshot and instant. It cannot abort, because each
    /// member passed the same evaluation in the viewport that fixed it.
    fn decide(&self) -> Result<SelectionDecision, ContainedTaskError> {
        let candidates = self
            .numbered_members()?
            .iter()
            .enumerate()
            .map(|(number, &index)| Candidate {
                candidate_id: self.member_id(number),
                fields: match &self.positions[index].state {
                    PositionState::Fixed { features, .. } => candidate_fields(features),
                    _ => BTreeMap::new(),
                },
            })
            .collect::<Vec<_>>();
        let decision = self.evaluate_candidates(&candidates)?;
        if matches!(decision.outcome, SelectionOutcome::Unknown { .. }) {
            return Err(ContainedTaskError::with_detail(
                ExecutionKernelCode::SelectionEvaluationFailed.code(),
                "scan decide aborted after every member was evaluated",
            ));
        }
        Ok(decision)
    }

    /// The members' positions in first-seen order (S0 §2.6): by the viewport that fixed them,
    /// then by axis start, then by cross-axis start. More than 64 fail
    /// `candidate_projection_budget_exceeded` (item `scan_candidates`).
    fn numbered_members(&self) -> Result<Vec<usize>, ContainedTaskError> {
        let mut members = self
            .positions
            .iter()
            .enumerate()
            .filter_map(|(index, position)| match &position.state {
                PositionState::Fixed {
                    viewport,
                    verdict: Some(Verdict::Member { .. }),
                    ..
                } => Some((*viewport, position.rect.axis, position.rect.cross, index)),
                _ => None,
            })
            .collect::<Vec<_>>();
        members.sort_unstable();
        if members.len() > SCAN_PROJECTION_MAX_CANDIDATES {
            return Err(ContainedTaskError::with_detail(
                CANDIDATE_PROJECTION_BUDGET_EXCEEDED,
                format!("item=scan_candidates count={}", members.len()),
            ));
        }
        Ok(members.into_iter().map(|(.., index)| index).collect())
    }

    /// A member's candidate ID: the layout ID and its number.
    fn member_id(&self, number: usize) -> String {
        format!("{}#{number:02}", self.layout_id)
    }

    /// The unknown instances (S0 §3.1): every position that was read but is neither a member nor
    /// gated out, in first-seen order. The record's `unknown[]` lists exactly these.
    fn unknown_positions(&self) -> Vec<usize> {
        self.positions
            .iter()
            .enumerate()
            .filter(|(_, position)| match &position.state {
                PositionState::Unread => false,
                PositionState::Incomplete | PositionState::Contradicted => true,
                PositionState::Fixed { verdict, .. } => {
                    !matches!(verdict, Some(Verdict::Member { .. } | Verdict::GatedOut))
                }
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// The largest upper bound over `unknown` (S0 §3.3): each position at its Known identity, or
    /// over the whole domain without one; `None` when there is no unknown instance.
    fn unknown_bound(&self, unknown: &[usize]) -> Result<Option<Bound>, ContainedTaskError> {
        if unknown.is_empty() {
            return Ok(None);
        }
        let (field_domains, domain) =
            bound_inputs(self.layout, self.policy, Some(&self.scan.identity_feature)).map_err(
                |detail| {
                    ContainedTaskError::with_detail(
                        ExecutionKernelCode::ContainedTaskSelectInvalid.code(),
                        format!("layout={} scan {detail}", self.layout_id),
                    )
                },
            )?;
        let mut upper = Bound::None;
        for &index in unknown {
            let identities = self.positions[index]
                .identity
                .as_ref()
                .map_or_else(|| domain.clone(), |identity| vec![identity.clone()]);
            let bound = upper_bound(
                self.policy,
                self.facts,
                self.now_unix_ms,
                &identities,
                &field_domains,
            )
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    ExecutionKernelCode::SelectionEvaluationFailed.code(),
                    error.to_string(),
                )
            })?;
            upper = upper.max(Bound::from(bound));
        }
        Ok(Some(upper))
    }

    /// `Return` and `Confirm` of the chosen member (S0 §2.2, §3.4): a failure while returning is
    /// `reposition_failed` at `return` (P9); `Confirm` gives its own variant.
    fn confirm_member<S: ScanSource>(
        &mut self,
        source: &mut S,
        chosen: &str,
    ) -> Result<Confirmation<S::Error>, ContainedTaskRunError<S::Error>> {
        let members = self.numbered_members()?;
        let Some(&member) = members
            .iter()
            .enumerate()
            .find(|(number, _)| self.member_id(*number) == chosen)
            .map(|(_, index)| index)
        else {
            return Err(ContainedTaskError::with_detail(
                ExecutionKernelCode::SelectionEvaluationFailed.code(),
                format!("selected candidate {chosen} is not a member"),
            )
            .into());
        };
        if let Err(error) = self.return_to(source, member) {
            return Ok((
                TaskSelectionConfirmation::RepositionFailed {
                    outcome: failure_outcome(&error),
                    stage: TaskSelectionRepositionStage::Return,
                },
                ScanSelectEnd::Failed(error),
            ));
        }
        Ok(self.confirm(source, member, chosen))
    }

    /// `Return` (S0 §2.2): from the latest offset `last`, moves until the chosen member's content
    /// rectangle lies inside the band, `[low, high]` in offsets. A distance above the threshold
    /// *T* = max(`advance.max`, `reposition.max` − (*b* − *e*)) takes one `reposition`, a
    /// shorter one one `advance` (reversed toward the start), so no move passes beyond the
    /// interval.
    /// Every settled frame passes the page and the guard, is aligned against the content map
    /// within its move's window and counts toward `max_reposition_viewports`.
    fn return_to<S: ScanSource>(
        &mut self,
        source: &mut S,
        member: usize,
    ) -> Result<(), ContainedTaskRunError<S::Error>> {
        let target = self.positions[member].rect;
        let band = self.geometry.band;
        let (low, high) = (target.axis_end() - band.axis_end(), target.axis - band.axis);
        let width = band.axis_extent - self.geometry.extent;
        let threshold = self
            .moves
            .advance_steps
            .1
            .max(self.moves.reposition_max - width);
        let mut last = self.previous_offset;
        while !(low..=high).contains(&last) {
            let toward_start = last > high;
            let distance = if toward_start {
                last - high
            } else {
                low - last
            };
            let reposition = distance >= threshold;
            self.check_budget(source, ExecutionKernelLocation::SelectReturn)?;
            let limit = self.scan.bounds.max_reposition_viewports;
            if self.rewind_viewports.saturating_add(self.return_viewports) >= limit {
                return Err(bounds_exceeded(
                    ExecutionKernelLocation::SelectReturn,
                    ScanBound::RepositionViewports,
                    limit,
                )
                .into());
            }
            self.drag(
                source,
                ScanDragMotion::Return {
                    reposition,
                    toward_start,
                },
            )?;
            let index = self.return_viewports;
            let reading = self.settle(source, None, index)?;
            self.return_viewports += 1;
            let window = self.return_window(last, toward_start, reposition);
            last = self.align_to_content_map(
                &reading,
                &window,
                ExecutionKernelLocation::SelectReturn,
                index,
            )?;
        }
        self.previous_offset = last;
        Ok(())
    }

    /// `Confirm` (S0 §1.3, §2.2): a fresh frame F2, settled as after an input; its page, its
    /// guard, its alignment against the content map within `last` ± the tolerance, then the
    /// identity of the instance at the chosen member's content position. While that identity
    /// reads Unknown, F2 is captured again at `settle.interval_ms` until `settle.timeout_ms`; no
    /// input occurs in between. The first failure decides the variant: capture or settle
    /// `capture_failed`, page `page_changed`, guard `guard_failed`, alignment
    /// `reposition_failed` at `confirm`, identity `identity_unconfirmed` (§3.4).
    fn confirm<S: ScanSource>(
        &mut self,
        source: &mut S,
        member: usize,
        chosen: &str,
    ) -> (TaskSelectionConfirmation, ScanSelectEnd<S::Error>) {
        let expected = self.positions[member].identity.clone().unwrap_or_default();
        let interval = Duration::from_millis(self.scan.settle.interval_ms);
        let timeout = Duration::from_millis(self.scan.settle.timeout_ms);
        let started = source.elapsed();
        let index = self.return_viewports;
        let mut fresh = true;
        loop {
            let captured = self
                .check_budget(source, ExecutionKernelLocation::SelectConfirm)
                .map_err(ContainedTaskRunError::Task)
                .and_then(|()| {
                    if fresh {
                        self.settle_capture(source, None, index)
                    } else {
                        source.capture(interval).map(|_| ())
                    }
                });
            fresh = false;
            if let Err(error) = captured {
                let confirmation = match &error {
                    ContainedTaskRunError::Task(task)
                        if task.code().as_str()
                            == ExecutionKernelCode::SelectionPageChanged.as_str() =>
                    {
                        TaskSelectionConfirmation::PageChanged
                    }
                    _ => TaskSelectionConfirmation::CaptureFailed {
                        outcome: failure_outcome(&error),
                    },
                };
                return (confirmation, ScanSelectEnd::Failed(error));
            }
            if let Err(error) = source.guard() {
                return (
                    TaskSelectionConfirmation::GuardFailed {
                        outcome: failure_outcome(&error),
                    },
                    ScanSelectEnd::Failed(error),
                );
            }
            let reading = match source.viewport() {
                Ok(reading) => reading,
                Err(error) => {
                    return (
                        TaskSelectionConfirmation::CaptureFailed {
                            outcome: failure_outcome(&error),
                        },
                        ScanSelectEnd::Failed(error),
                    );
                }
            };
            let tolerance = self.geometry.tolerance;
            let window = [(
                self.previous_offset - tolerance,
                self.previous_offset + tolerance,
            )];
            let offset = match self.align_to_content_map(
                &reading,
                &window,
                ExecutionKernelLocation::SelectConfirm,
                index,
            ) {
                Ok(offset) => offset,
                Err(error) => {
                    let error = ContainedTaskRunError::Task(error);
                    return (
                        TaskSelectionConfirmation::RepositionFailed {
                            outcome: failure_outcome(&error),
                            stage: TaskSelectionRepositionStage::Confirm,
                        },
                        ScanSelectEnd::Failed(error),
                    );
                }
            };
            let (identity, rect) = self.f2_identity(&reading, member, offset, &expected);
            let (state, observed, unknown) = match identity {
                F2Identity::Matched => {
                    return match (reading.frame_id, rect, i32::try_from(offset)) {
                        (Some(frame_id), Some(rect), Ok(content_offset_px)) => (
                            TaskSelectionConfirmation::IdentityMatched {
                                candidate_id: chosen.to_owned(),
                                frame_id,
                                content_offset_px,
                            },
                            ScanSelectEnd::Confirmed {
                                identity: expected,
                                rect,
                            },
                        ),
                        _ => {
                            let error =
                                ContainedTaskRunError::Task(ContainedTaskError::with_detail(
                                    ExecutionKernelCode::SelectionConfirmationCaptureFailed.code(),
                                    "scan confirmation frame unbound",
                                ));
                            (
                                TaskSelectionConfirmation::CaptureFailed {
                                    outcome: failure_outcome(&error),
                                },
                                ScanSelectEnd::Failed(error),
                            )
                        }
                    };
                }
                F2Identity::Unknown(_)
                    if source.elapsed().saturating_sub(started) < Duration::ZERO =>
                {
                    continue;
                }
                F2Identity::Absent => (TaskSelectionIdentityState::Absent, None, None),
                F2Identity::Unknown(reason) => (TaskSelectionIdentityState::Unknown, None, reason),
                F2Identity::Different(observed) => {
                    (TaskSelectionIdentityState::Different, Some(observed), None)
                }
            };
            let vocabulary = match state {
                TaskSelectionIdentityState::Absent => IdentityState::Absent,
                TaskSelectionIdentityState::Unknown => IdentityState::Unknown,
                TaskSelectionIdentityState::Different => IdentityState::Different,
            };
            let error = identity_unconfirmed(&expected, vocabulary, observed.as_deref());
            return (
                TaskSelectionConfirmation::IdentityUnconfirmed {
                    identity_state: state,
                    observed_identity: observed,
                    identity_unknown: unknown,
                },
                ScanSelectEnd::Failed(error.into()),
            );
        }
    }

    /// The identity F2 shows at the chosen member's content position, and that instance's
    /// rectangle in F2's frame coordinates: the nearest instance within the tolerance on both
    /// axes. It must be actionable; nothing else on F2 is compared (S0 §1.3).
    fn f2_identity(
        &self,
        reading: &ScanReading,
        member: usize,
        offset: i64,
        expected: &str,
    ) -> (F2Identity, Option<CandidateRect>) {
        let target = self.positions[member].rect;
        let tolerance = self.geometry.tolerance;
        let found = reading
            .instances
            .iter()
            .filter_map(|instance| {
                let rect = AxisRect::of(self.scan.axis, instance.rect);
                let distance =
                    ((rect.axis + offset) - target.axis).abs() + (rect.cross - target.cross).abs();
                (((rect.axis + offset) - target.axis).abs() <= tolerance
                    && (rect.cross - target.cross).abs() <= tolerance)
                    .then_some((distance, instance))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, instance)| instance);
        let Some(instance) = found.filter(|instance| instance.actionable) else {
            return (F2Identity::Absent, None);
        };
        let identity = match instance.features.get(&self.scan.identity_feature) {
            Some(CandidateFeature::Identity { value, .. }) if value == expected => {
                F2Identity::Matched
            }
            Some(CandidateFeature::Identity { value, .. }) => F2Identity::Different(value.clone()),
            Some(CandidateFeature::Unknown { reason, .. }) => F2Identity::Unknown(Some(*reason)),
            _ => F2Identity::Unknown(None),
        };
        (identity, Some(instance.rect))
    }

    /// The record's v2 projection of this scan (S0 §1.9) at `coverage`: the coverage members its
    /// state carries, the coverage viewports, the rewind and return counts, the members numbered
    /// as at `Decide`, the unknown positions and the gated-out identities. A viewport without a
    /// frame identity cannot be recorded.
    pub(in crate::contained_task) fn projection(
        &self,
        coverage: ScanCoverageState,
    ) -> Result<ScanCandidateProjection, ContainedTaskError> {
        let invalid = |detail: &str| {
            ContainedTaskError::with_detail(
                ExecutionKernelCode::SelectionEvaluationFailed.code(),
                format!("decision record: {detail}"),
            )
        };
        let narrow = |value: i64| {
            i32::try_from(value).map_err(|_| invalid("scan content offset out of range"))
        };
        let extent = self
            .viewports
            .iter()
            .map(|viewport| viewport.offset + self.geometry.frame_axis)
            .max()
            .unwrap_or(self.geometry.frame_axis);
        let extent = u32::try_from(extent).map_err(|_| invalid("scan extent out of range"))?;
        let coverage = ScanCoverage {
            state: coverage,
            start: (coverage != ScanCoverageState::NotStarted)
                .then_some(self.start)
                .flatten(),
            end: (coverage == ScanCoverageState::Complete).then_some(ScanEndEvidence::EndCheck),
            extent_px: matches!(
                coverage,
                ScanCoverageState::EarlyStopped | ScanCoverageState::Complete
            )
            .then_some(extent),
        };
        let viewports = self
            .viewports
            .iter()
            .map(|viewport| {
                Ok(ScanViewport {
                    motion: viewport.motion,
                    content_offset_px: narrow(viewport.offset)?,
                    tracked_instances: viewport.tracked,
                    readable_instances: viewport.readable,
                    unknown_instances: viewport.unknown,
                    frame_id: viewport
                        .frame_id
                        .ok_or_else(|| invalid("scan viewport frame unbound"))?,
                })
            })
            .collect::<Result<Vec<_>, ContainedTaskError>>()?;
        let candidates = self
            .numbered_members()?
            .into_iter()
            .enumerate()
            .map(|(number, index)| {
                let position = &self.positions[index];
                let PositionState::Fixed {
                    viewport, features, ..
                } = &position.state
                else {
                    return Err(invalid("scan member unfixed"));
                };
                Ok(ScanCandidate {
                    id: self.member_id(number),
                    instance_index: u32_of(number),
                    actionable: true,
                    content_rect: rect_of(self.scan.axis, position.rect)?,
                    fixed_viewport: *viewport,
                    features: features.clone(),
                })
            })
            .collect::<Result<Vec<_>, ContainedTaskError>>()?;
        let unknown = self
            .unknown_positions()
            .into_iter()
            .map(|index| {
                let position = &self.positions[index];
                let cause = match &position.state {
                    PositionState::Contradicted => ScanUnknownCause::Contradicted,
                    PositionState::Fixed {
                        verdict: Some(Verdict::UnknownDropped),
                        ..
                    } => ScanUnknownCause::UnknownDropped,
                    _ if position.identity.is_some() => ScanUnknownCause::FeatureUnread,
                    _ => ScanUnknownCause::IdentityUnread,
                };
                Ok(ScanUnknownPosition {
                    position_index: position.index,
                    content_rect: rect_of(self.scan.axis, position.rect)?,
                    first_read_viewport: position.first_read_viewport.unwrap_or_default(),
                    cause,
                    identity: position.identity.clone(),
                    identity_unknown: position
                        .identity
                        .is_none()
                        .then_some(position.identity_unknown)
                        .flatten(),
                })
            })
            .collect::<Result<Vec<_>, ContainedTaskError>>()?;
        let mut gated_out = self
            .positions
            .iter()
            .filter(|position| {
                matches!(
                    position.state,
                    PositionState::Fixed {
                        verdict: Some(Verdict::GatedOut),
                        ..
                    }
                )
            })
            .map(|position| position.identity.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        gated_out.sort();
        ScanCandidateProjection::new(ScanCandidateProjectionParts {
            page_id: self.layout.page_id.clone(),
            layout_id: self.layout_id.to_owned(),
            axis: self.scan.axis,
            frame: CandidateFrame {
                width: self.frame.0,
                height: self.frame.1,
            },
            coverage,
            viewports,
            reposition: ScanReposition {
                rewind_viewports: self.rewind_viewports,
                return_viewports: self.return_viewports,
            },
            candidates,
            unknown,
            gated_out: ScanGatedOut {
                count: u32_of(gated_out.len()),
                identities_sha256: identities_sha256(&gated_out)?,
            },
        })
        .map_err(|error| {
            ContainedTaskError::with_detail(
                error.code(),
                format!("item={} {}", error.item(), error.detail()),
            )
        })
    }
}
