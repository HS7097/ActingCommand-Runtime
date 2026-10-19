// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #384 D2-S2 (`rd-1009/d2/model-d2-fact-refresh.md` §3.2, §3.4, §3.5): reading marks
//! on any page of the task. When a run reaches a marked page, the frame it reached the page on is
//! handed to recognition and the run goes on without waiting for the result. A successful run
//! joins its readings before `Finalizing`, in hand-off order and within the runtime's join
//! timeout. A mark that gives no reading is a warning of its own and never fails the run. A
//! requested key set reads only the marks of its keys, and a linear package runs only the
//! segments that reach one of them.
//!
//! Workflow #384 D2-S3 (§3.6): an operation that declares resource adjustments is confirmed on
//! the frame its step finished on; an adjustment that reads the change the screen shows hands
//! that frame to recognition for its amount, and the run joins the amounts after the readings.

use super::*;
use actingcommand_contract::ocr_confidence_milli;
use actingcommand_recognition_pack::{
    OcrObservationEvaluation, RecognitionPackResult, RememberedOcrObservations,
};
use std::sync::mpsc;

/// The recognition results of one handed-off frame, one per mark in mark order.
type MarkResults = Vec<RecognitionPackResult<OcrObservationEvaluation>>;

actingcommand_contract::outcome_vocabulary! {
    /// Why one reading mark gave no reading on a frame handed to recognition (the vocabulary
    /// `reading_problem`).
    pub enum ResourceReadingProblem: "reading_problem" {
        /// The trimmed text is empty.
        Empty => "empty",
        /// The text is not an integer of the declared format.
        InvalidInteger => "invalid_integer",
        /// The integer does not fit 64 bits.
        Overflow => "overflow",
        /// The integer is outside the declared range.
        OutOfRange => "out_of_range",
        /// The OCR target's region could not be placed on the frame.
        RegionUnresolved => "region_unresolved",
        /// The OCR provider failed, or the recognition ended without an answer.
        ProviderFailed => "provider_failed",
        /// The provider gave no confidence.
        ConfidenceMissing => "confidence_missing",
        /// The confidence is below the mark's floor.
        LowConfidence => "low_confidence",
        /// The recognition had not returned when the join timeout ended.
        JoinTimeout => "join_timeout",
    }
}

impl ResourceReadingProblem {
    /// The problem of a text the declaration's parser refused. The parser gives only the four
    /// reasons of a text it could not read as the declared integer; every other reason of the
    /// shared set names a field result the parser never gives and reads as no integer.
    const fn of_parse(reason: OcrFieldReason) -> Self {
        match reason {
            OcrFieldReason::Empty => Self::Empty,
            OcrFieldReason::Overflow => Self::Overflow,
            OcrFieldReason::OutOfRange => Self::OutOfRange,
            OcrFieldReason::InvalidInteger
            | OcrFieldReason::Resolved
            | OcrFieldReason::UnknownEntry
            | OcrFieldReason::AmbiguousEntry
            | OcrFieldReason::LimitExceeded
            | OcrFieldReason::ProviderFailed
            | OcrFieldReason::RegionUnresolved
            | OcrFieldReason::NotCollected
            | OcrFieldReason::LowConfidence => Self::InvalidInteger,
        }
    }
}

/// One reading mark that gave no reading on a frame handed to recognition (model §3.5). In
/// memory only; the runtime reports it as the warning `contained_task_resource_reading_unresolved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceReadingMiss {
    pub declaration: ResourceReadingDeclaration,
    pub problem: ResourceReadingProblem,
}

impl ResourceReadingMiss {
    /// The warning of this miss: `contained_task_resource_reading_unresolved` with the mark's
    /// `reading_id`, `fact_key` and `reading_problem`.
    pub fn outcome(&self) -> Outcome {
        Outcome::new(ExecutionKernelCode::ContainedTaskResourceReadingUnresolved)
            .reading_id(self.declaration.id.as_str())
            .fact_key(self.declaration.fact_key.as_str())
            .reading_problem(self.problem)
    }
}

/// Workflow #384 D2-S3: the change one adjustment read on its confirmation frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceAdjustmentAmountValue {
    pub value: u64,
    pub confidence_milli: u16,
}

/// Workflow #384 D2-S3 (model §3.6): one adjustment of a confirmed operation, joined before
/// `Finalizing`. An adjustment that reads its amount carries the amount or why it gave none
/// (a miss is the warning `contained_task_resource_reading_unresolved` of the adjustment); one
/// read after its operation carries none. In memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAdjustmentResult {
    pub declaration: ResourceAdjustmentDeclaration,
    pub amount: Option<Result<ResourceAdjustmentAmountValue, ResourceReadingProblem>>,
}

impl ResourceAdjustmentResult {
    /// The warning of an amount that gave no reading: `contained_task_resource_reading_unresolved`
    /// with the adjustment's id as `reading_id`, its `fact_key` and the `reading_problem`.
    pub fn amount_miss(&self) -> Option<Outcome> {
        let Some(Err(problem)) = self.amount else {
            return None;
        };
        Some(
            Outcome::new(ExecutionKernelCode::ContainedTaskResourceReadingUnresolved)
                .reading_id(self.declaration.id.as_str())
                .fact_key(self.declaration.fact_key.as_str())
                .reading_problem(problem),
        )
    }
}

/// Workflow #384 D2-S3: judges one adjustment amount's OCR evaluation: the shared trim and
/// parse of a change (at most one sign of its direction), then the floor its OCR target
/// declares (`minimum_confidence`).
pub fn judge_resource_adjustment_amount(
    amount: &ResourceAdjustmentAmount,
    minimum_confidence: f32,
    result: &RecognitionPackResult<OcrObservationEvaluation>,
) -> Result<ResourceAdjustmentAmountValue, ResourceReadingProblem> {
    let evaluated = match result {
        Ok(evaluated) => evaluated,
        Err(error) if error.code() == RecognitionPackErrorCode::RegionUnresolved => {
            return Err(ResourceReadingProblem::RegionUnresolved);
        }
        Err(_) => return Err(ResourceReadingProblem::ProviderFailed),
    };
    let value = amount
        .parse(&evaluated.text)
        .map_err(ResourceReadingProblem::of_parse)?;
    let confidence_milli = ocr_confidence_milli(evaluated.confidence)
        .ok_or(ResourceReadingProblem::ConfidenceMissing)?;
    if ocr_confidence_milli(Some(minimum_confidence)).is_some_and(|floor| confidence_milli < floor)
    {
        return Err(ResourceReadingProblem::LowConfidence);
    }
    Ok(ResourceAdjustmentAmountValue {
        value,
        confidence_milli,
    })
}

/// Judges one mark's OCR evaluation: the shared trim and parse, then the confidence floor. The
/// readings of a mark that held: its value, and (Workflow #390 READ-1) for a mark that declares
/// `capacity_fact_key` then its capacity, both at the evaluation's confidence. A mark gives
/// both readings or none.
pub fn judge_resource_reading(
    declaration: &ResourceReadingDeclaration,
    result: &RecognitionPackResult<OcrObservationEvaluation>,
) -> Result<Vec<ResourceReadingValue>, ResourceReadingProblem> {
    let evaluated = match result {
        Ok(evaluated) => evaluated,
        Err(error) if error.code() == RecognitionPackErrorCode::RegionUnresolved => {
            return Err(ResourceReadingProblem::RegionUnresolved);
        }
        Err(_) => return Err(ResourceReadingProblem::ProviderFailed),
    };
    let parts = declaration
        .parse_parts(&evaluated.text)
        .map_err(ResourceReadingProblem::of_parse)?;
    let confidence_milli = ocr_confidence_milli(evaluated.confidence)
        .ok_or(ResourceReadingProblem::ConfidenceMissing)?;
    if confidence_milli < declaration.minimum_confidence_milli {
        return Err(ResourceReadingProblem::LowConfidence);
    }
    let reading = |fact_key: &str, value: u64| ResourceReadingValue {
        declaration: declaration.clone(),
        fact_key: fact_key.to_owned(),
        value,
        confidence_milli,
    };
    let mut readings = vec![reading(declaration.fact_key.as_str(), parts.value)];
    if let (Some(capacity_fact_key), Some(capacity)) =
        (declaration.capacity_fact_key.as_deref(), parts.capacity)
    {
        readings.push(reading(capacity_fact_key, capacity));
    }
    Ok(readings)
}

/// Workflow #384 D2-S2: the package id and the reading-mark keys of a package, read from its
/// declarations without preparing it as a task (D2 model §3.2, the binding checks a Runtime
/// start makes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingMarks {
    pub package_id: String,
    /// The keys of its reading marks, in declaration order (Workflow #390 READ-1: a mark's
    /// capacity key after its key).
    pub fact_keys: Vec<String>,
}

impl PreparedContainedTask {
    /// Workflow #384 D2-S2: the [`ReadingMarks`] of the package `locator` names, admitted by its
    /// path through the metadata-only containment as [`Self::describe_path`] admits it, but
    /// without preparing the task: its qualification as a reader is not decided here.
    pub fn read_reading_marks(
        instance_label: &str,
        locator: &std::path::Path,
        deadline: Instant,
    ) -> Result<ReadingMarks, ContainedTaskError> {
        let bundle =
            ExternallyVerifiedBundle::load_metadata_path(instance_label, locator, deadline)
                .map_err(contained_task_admission_error)?;
        let bundle = bundle.loaded_bundle();
        let control = bundle.control().cloned().ok_or_else(|| {
            ContainedTaskError::new(ExecutionKernelCode::ContainedTaskControlMissing.code())
        })?;
        let control = parse_task_control_declaration(control)?;
        let readings = match bundle.operation().get("resource_readings") {
            None => Vec::new(),
            Some(value) => serde_json::from_value::<Vec<ResourceReadingDeclaration>>(value.clone())
                .map_err(|_| {
                    ContainedTaskError::new(
                        ExecutionKernelCode::ContainedTaskResourceReadingInvalid.code(),
                    )
                })?,
        };
        Ok(ReadingMarks {
            package_id: control.package_id,
            fact_keys: readings
                .iter()
                .flat_map(|reading| reading.fact_keys())
                .map(str::to_owned)
                .collect(),
        })
    }
}

/// One frame handed to recognition: its device capture time, the marks of its page and the
/// channel its recognition answers on.
#[derive(Debug)]
struct HandedOffFrame {
    captured_at: SystemTime,
    declarations: Vec<ResourceReadingDeclaration>,
    results: mpsc::Receiver<MarkResults>,
}

/// Workflow #384 D2-S3: one adjustment of a confirmed operation, with its amount's recognition
/// when it reads one.
#[derive(Debug)]
struct ConfirmedAdjustment {
    declaration: ResourceAdjustmentDeclaration,
    amount: Option<mpsc::Receiver<MarkResults>>,
}

/// Workflow #384 D2-S3: one confirmation of an operation that declares adjustments: the
/// confirmation frame's device capture time and the operation's adjustments.
#[derive(Debug)]
struct Confirmation {
    captured_at: SystemTime,
    adjustments: Vec<ConfirmedAdjustment>,
}

/// The reading marks of one ordinary run and the frames it handed to recognition (model §3.5);
/// Workflow #384 D2-S3: its adjustments and the confirmations of their operations (§3.6).
#[derive(Debug, Default)]
pub(super) struct ReadingRelay<'a> {
    declarations: &'a [ResourceReadingDeclaration],
    adjustments: &'a [ResourceAdjustmentDeclaration],
    /// The requested key set (model §3.4); `None` reads every mark.
    requested: Option<&'a BTreeSet<String>>,
    /// The run's recognition evaluator, shared with its recognition threads once a frame is
    /// handed off.
    evaluator: Option<Arc<RecognitionEvaluator>>,
    /// The page of the last frame offered: a stay on one page is one pass.
    last_page: Option<String>,
    frames: Vec<HandedOffFrame>,
    confirmations: Vec<Confirmation>,
}

impl<'a> ReadingRelay<'a> {
    pub(super) fn new(
        declarations: &'a [ResourceReadingDeclaration],
        adjustments: &'a [ResourceAdjustmentDeclaration],
        requested: Option<&'a BTreeSet<String>>,
    ) -> Self {
        Self {
            declarations,
            adjustments,
            requested,
            ..Self::default()
        }
    }

    /// The requested key set, when the run was given one.
    pub(super) fn requested(&self) -> Option<&'a BTreeSet<String>> {
        self.requested
    }

    /// Every reading mark of the run, whether or not the requested key set reads it.
    pub(super) fn declarations(&self) -> &'a [ResourceReadingDeclaration] {
        self.declarations
    }

    /// Workflow #390 READ-1: a key set reads a mark when it requests any key the mark writes.
    fn wanted(&self, declaration: &ResourceReadingDeclaration) -> bool {
        self.requested
            .is_none_or(|keys| declaration.fact_keys().any(|key| keys.contains(key)))
    }

    /// Whether the run reads a mark on `page`.
    pub(super) fn marks_wanted_on(&self, game: &str, page: &str) -> bool {
        self.declarations.iter().any(|declaration| {
            crate::page_anchor_matches(game, page, &declaration.page_id) && self.wanted(declaration)
        })
    }

    /// The marks the run reads on a frame of `page_label`, when that frame starts a new pass
    /// of the page; empty when it does not or the page has none.
    fn pass(&mut self, game: &str, page_label: &str) -> Vec<ResourceReadingDeclaration> {
        if self.last_page.as_deref() == Some(page_label) {
            return Vec::new();
        }
        self.last_page = Some(page_label.to_owned());
        self.declarations
            .iter()
            .filter(|declaration| {
                crate::page_anchor_matches(game, page_label, &declaration.page_id)
                    && self.wanted(declaration)
            })
            .cloned()
            .collect()
    }

    /// Hands `scene` to recognition for `declarations` and returns at once. The recognition
    /// runs on its own thread; when no thread can be started, it runs here, so the marks are
    /// still read. Workflow #396 REC-6: a mark's target that the page gate observed on this
    /// frame (`remembered`) takes that observation instead of calling the provider again.
    fn hand_off(
        &mut self,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        captured_at: SystemTime,
        declarations: Vec<ResourceReadingDeclaration>,
        remembered: RememberedOcrObservations,
    ) -> Result<(), ContainedTaskError> {
        let targets = declarations
            .iter()
            .map(|declaration| declaration.target_id.clone())
            .collect::<Vec<_>>();
        let results = self.recognize(evaluator, scene, targets, remembered)?;
        self.frames.push(HandedOffFrame {
            captured_at,
            declarations,
            results,
        });
        Ok(())
    }

    /// Evaluates `targets` on `scene` on a thread of its own and returns the channel its answer
    /// arrives on; when no thread can be started, it evaluates them here.
    fn recognize(
        &mut self,
        evaluator: &RecognitionEvaluator,
        scene: &Scene,
        targets: Vec<String>,
        remembered: RememberedOcrObservations,
    ) -> Result<mpsc::Receiver<MarkResults>, ContainedTaskError> {
        let evaluator = Arc::clone(
            self.evaluator
                .get_or_insert_with(|| Arc::new(evaluator.clone())),
        );
        let scene =
            Scene::from_rgb8(scene.width(), scene.height(), scene.rgb8_pixels()).map_err(|_| {
                ContainedTaskError::new(ExecutionKernelCode::ContainedTaskFrameInvalid.code())
            })?;
        let job = Arc::new(ReadingJob {
            evaluator,
            scene,
            targets,
            remembered,
        });
        let (sender, results) = mpsc::channel();
        let here = sender.clone();
        let thread_job = Arc::clone(&job);
        let spawned = thread::Builder::new()
            .name("contained-task-reading".to_owned())
            .spawn(move || {
                // The join may already have given up on this frame; its answer is then unread.
                let _unread = sender.send(read_marks(&thread_job));
            });
        if spawned.is_err() {
            let _unread = here.send(read_marks(&job));
        }
        Ok(results)
    }
}

/// Waits for one recognition's answer, until `deadline` when the runtime has a join timeout.
/// The time left is read on the runtime's clock (test plan E-3).
fn receive<R: ContainedTaskRuntime>(
    runtime: &mut R,
    results: &mpsc::Receiver<MarkResults>,
    deadline: Option<Instant>,
) -> Result<MarkResults, mpsc::RecvTimeoutError> {
    match deadline {
        None => results
            .recv()
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        Some(deadline) => {
            results.recv_timeout(deadline.saturating_duration_since(runtime.monotonic_now()))
        }
    }
}

/// One handed-off frame's recognition work: the run's evaluator, the frame, its marks' targets
/// in mark order and the page gate's observations of them on that frame (Workflow #396 REC-6).
struct ReadingJob {
    evaluator: Arc<RecognitionEvaluator>,
    scene: Scene,
    targets: Vec<String>,
    remembered: RememberedOcrObservations,
}

/// The OCR evaluation of every mark's target on one handed-off frame, in mark order.
fn read_marks(job: &ReadingJob) -> MarkResults {
    let context = job
        .evaluator
        .scene_context(&job.scene)
        .with_remembered_observations(job.remembered.clone());
    job.targets
        .iter()
        .map(|target| context.evaluate_ocr_observation(target))
        .collect()
}

impl PreparedContainedTask {
    /// The run reached `page_label` on the frame it captured last, at `captured_at`. When the
    /// frame starts a pass of a page the run reads marks on, the runtime is told the frame was
    /// handed off and the frame goes to recognition; the run does not wait (model §3.5).
    /// `remembered` holds the page gate's observations of the run's reading targets on that frame
    /// (Workflow #396 REC-6), which the recognition takes instead of calling the provider again.
    pub(super) fn hand_off_reading_frame<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        page_label: &str,
        scene: &Scene,
        captured_at: SystemTime,
        remembered: RememberedOcrObservations,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let Some(relay) = ocr_collector.reading_relay.as_mut() else {
            return Ok(());
        };
        let declarations = relay.pass(&self.control.game, page_label);
        if declarations.is_empty() {
            return Ok(());
        }
        runtime
            .record(ContainedTaskTrace::ResourceReadingFrame { captured_at })
            .map_err(ContainedTaskRunError::Boundary)?;
        relay.hand_off(
            &self.evaluator,
            scene,
            captured_at,
            declarations,
            remembered,
        )?;
        Ok(())
    }

    /// Before `Finalizing` of a successful run: waits for the readings of every handed-off
    /// frame, in hand-off order, until the runtime's join timeout ends, records each mark's
    /// evaluation against its frame, and records each frame's readings and misses. A mark whose
    /// recognition had not returned is the miss `join_timeout`; a recognition thread that ended
    /// without an answer is `provider_failed` for each of its marks.
    ///
    /// #385 P-k (performance model v1.4.2 §5.3): the join timeout is budget time on the run's
    /// budget clock `pace`. The answers are awaited on the wall clock, so the join's deadline is
    /// the wall instant its budget deadline falls on: now + timeout × f / 1000.
    pub(super) fn join_resource_readings<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        relay: ReadingRelay<'_>,
        pace: PaceClock,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        if relay.frames.is_empty() && relay.confirmations.is_empty() {
            return Ok(());
        }
        let join_timeout = runtime
            .reading_join_timeout()
            .map_err(ContainedTaskRunError::operation::<R>)?;
        let deadline =
            join_timeout.map(|timeout| runtime.monotonic_now() + pace.wall_span(timeout));
        for frame in relay.frames {
            let answer = receive(runtime, &frame.results, deadline);
            let mut readings = Vec::new();
            let mut unresolved = Vec::new();
            match answer {
                Ok(results) => {
                    for (declaration, result) in frame.declarations.into_iter().zip(results) {
                        runtime
                            .record_reading_evaluation(
                                frame.captured_at,
                                &declaration.target_id,
                                &result,
                            )
                            .map_err(ContainedTaskRunError::Boundary)?;
                        match judge_resource_reading(&declaration, &result) {
                            Ok(held) => readings.extend(held),
                            Err(problem) => unresolved.push(ResourceReadingMiss {
                                declaration,
                                problem,
                            }),
                        }
                    }
                }
                Err(timeout) => {
                    let problem = match timeout {
                        mpsc::RecvTimeoutError::Timeout => ResourceReadingProblem::JoinTimeout,
                        mpsc::RecvTimeoutError::Disconnected => {
                            ResourceReadingProblem::ProviderFailed
                        }
                    };
                    unresolved.extend(frame.declarations.into_iter().map(|declaration| {
                        ResourceReadingMiss {
                            declaration,
                            problem,
                        }
                    }));
                }
            }
            runtime
                .record(ContainedTaskTrace::ResourceReadings {
                    captured_at: frame.captured_at,
                    readings,
                    unresolved,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
        }
        // Workflow #384 D2-S3 (model §3.6): the amounts of the confirmed adjustments, within the
        // same join timeout, confirmation by confirmation.
        for confirmation in relay.confirmations {
            let mut adjustments = Vec::with_capacity(confirmation.adjustments.len());
            for adjustment in confirmation.adjustments {
                let amount = match (&adjustment.declaration.amount, adjustment.amount) {
                    (Some(declared), Some(results)) => Some(self.join_adjustment_amount(
                        runtime,
                        confirmation.captured_at,
                        declared,
                        &results,
                        deadline,
                    )?),
                    _ => None,
                };
                adjustments.push(ResourceAdjustmentResult {
                    declaration: adjustment.declaration,
                    amount,
                });
            }
            runtime
                .record(ContainedTaskTrace::ResourceAdjustments {
                    captured_at: confirmation.captured_at,
                    adjustments,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
        }
        Ok(())
    }

    /// Workflow #384 D2-S3: waits for one adjustment amount's recognition, records its
    /// evaluation on the confirmation frame and judges it.
    fn join_adjustment_amount<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        captured_at: SystemTime,
        amount: &ResourceAdjustmentAmount,
        results: &mpsc::Receiver<MarkResults>,
        deadline: Option<Instant>,
    ) -> Result<
        Result<ResourceAdjustmentAmountValue, ResourceReadingProblem>,
        ContainedTaskRunError<R::Error>,
    > {
        let result = match receive(runtime, results, deadline) {
            Ok(mut results) => match results.pop() {
                Some(result) => result,
                None => return Ok(Err(ResourceReadingProblem::ProviderFailed)),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Ok(Err(ResourceReadingProblem::JoinTimeout));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Ok(Err(ResourceReadingProblem::ProviderFailed));
            }
        };
        runtime
            .record_adjustment_evaluation(captured_at, &amount.target_id, &result)
            .map_err(ContainedTaskRunError::Boundary)?;
        let minimum_confidence = ocr_target_minimum_confidence(&self.evaluator, &amount.target_id)?;
        Ok(judge_resource_adjustment_amount(
            amount,
            minimum_confidence,
            &result,
        ))
    }

    /// Workflow #384 D2-S3 (model §3.6): the run confirmed `operation_label` on `observation`,
    /// the frame its step finished on. When the operation declares adjustments, the runtime is
    /// told so, and each adjustment that reads its amount hands the frame to recognition for
    /// it; the run does not wait.
    pub(super) fn confirm_resource_adjustments<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        operation_label: &str,
        observation: &PageObservation,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let Some(relay) = ocr_collector.reading_relay.as_mut() else {
            return Ok(());
        };
        let declarations = relay
            .adjustments
            .iter()
            .filter(|adjustment| adjustment.operation_id == operation_label)
            .cloned()
            .collect::<Vec<_>>();
        if declarations.is_empty() {
            return Ok(());
        }
        runtime
            .record(ContainedTaskTrace::ResourceAdjustmentConfirmed {
                captured_at: observation.captured_at,
                operation_label: operation_label.to_owned(),
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let mut adjustments = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let amount = match &declaration.amount {
                Some(amount) => Some(relay.recognize(
                    &self.evaluator,
                    &observation.scene,
                    vec![amount.target_id.clone()],
                    RememberedOcrObservations::default(),
                )?),
                None => None,
            };
            adjustments.push(ConfirmedAdjustment {
                declaration,
                amount,
            });
        }
        relay.confirmations.push(Confirmation {
            captured_at: observation.captured_at,
            adjustments,
        });
        Ok(())
    }
}
