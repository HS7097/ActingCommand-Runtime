// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-L0 (`contracts/display-adapter.md` section 14; register §1.521): the layout
//! probe, an offline Lab operation (`layout probe`, MCP `ac_layout_probe`). From one saved frame of
//! the base's ratio or wider and a package it says, for every template mark the chosen pages
//! require, where the mark is found on the frame brought to the base's height and how that place
//! relates to the place the base gives it. It describes a measurement: it writes no declaration and
//! decides nothing.
//!
//! - **What it reads.** The package and the scene as the offline `recognize` reads them, from the
//!   same input ([`crate::ReadonlyRecognitionInput`]: the externally verified bundle with its
//!   recognition pack and page set, and the scene file). No capture, no device input, no run, no
//!   Runtime; nothing is written.
//! - **The frame** ([`ScaleFrame::of`]). A frame of the base's orientation, at least the base's
//!   height and whose width over its height is not below the base's is brought to the base's height
//!   by the display adapter's map measured by height on the screen (`DisplayMap::build`) and
//!   resampled once by the function the frame gate resamples a frame of the base's ratio with
//!   (REC-5, `resample_pixels`, area average): the scale frame, of the base's height and the base's
//!   width plus the room. A frame of the base's ratio has no room. Any other frame is refused
//!   `lab_scene_size_unsupported` before anything is searched, with its word: the base's other
//!   orientation `orientation_mismatch`, lower than the base `scale_below_base`, narrower than the
//!   base's ratio `ratio_below_base`.
//! - **A mark** ([`Search::template`]). A template mark is searched over the whole scale frame, its
//!   declared region aside, with its template scored as recognition scores it (the pack's metric,
//!   the mark's mask and factors, on the exact score maps): its best place, its score and its
//!   threshold. Its base place is its template, of the size it was found at, centred in its
//!   declared rectangle; `dx` and `dy` are the best place's offset from it. A template declared over
//!   the whole frame has no base place and no offsets. Every other mark (a colour, a colour digest,
//!   an OCR, a check, a value test, a template this build cannot evaluate) is listed as not probed,
//!   with its kind and its declared rectangle.
//! - **Classes** ([`Places`]). With a room `R`, the places are `start` (0), `centre` (`R / 2`
//!   rounded down, as the nine-grid's middle) and `end` (`R`); a found mark (its score reaches its
//!   threshold) whose `dy` and `dx` lie within the tolerance of 0 and of a place is at that place,
//!   any other found mark `other`, a mark not found `missing`. Without room the three places are
//!   one: `as_mapped`. Where the windows of two places would overlap (`R > 0` and `R / 2` within
//!   twice the tolerance) the answer says the room is too small to tell and classes no mark.
//! - **Per page and overall** ([`summary`]): the counts by class, the distinct `dx` of the found
//!   marks with their counts and one line of reading ([`reading`]); the pages ordered by the share
//!   of their template marks found come first.

use crate::display_base::scene_size_unsupported;
use crate::readonly::{load_evaluator, load_page_detector, recognition_scene};
use crate::{Lab, LabPorts, ReadonlyRecognitionInput, package_base};
use actingcommand_contract::{CaptureExtent, DisplayArea, DisplayScaleBy, LabError, LabResult};
use actingcommand_display_adapter::{
    DisplayMap, DisplayObservation, DisplayProblem, DisplayRule, Orientation,
};
use actingcommand_page_detector::{PageDefinition, PageDetector};
use actingcommand_recognition::resample::{SCALE_IDENTITY_MILLI, resample_pixels};
use actingcommand_recognition::{
    RegionSpectra, Scene, SearchClock, TemplateMask, TemplateMatchTimingStage,
};
use actingcommand_recognition_pack::{
    PackRect, PackRegion, RecognitionEvaluator, RecognitionMask, RecognitionTarget, TargetKind,
    TemplateTarget,
};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

/// How far, in scale-frame pixels, a mark may lie from a place and still be at it, when the caller
/// gives no tolerance.
pub const LAYOUT_PROBE_DEFAULT_TOLERANCE_PX: u32 = 4;

/// The reading of a room too small to tell the places apart.
const ROOM_TOO_SMALL: &str = "the room is too small to tell";
/// The reading of a page none of whose required marks is a template.
const NOTHING_PROBED: &str = "no template mark to probe";
/// The reading where no template mark is found.
const NOT_THIS_PAGE: &str = "not this page";
/// The reading of a frame without room whose every found mark is where the base puts it.
const AS_MAPPED: &str = "as mapped: a frame of the base's ratio";
/// The reading of a frame without room where a found mark is elsewhere.
const OFF_THE_BASE: &str = "not where the base puts them: the offsets say where";
/// The reading where every found mark is at the centre.
const CENTRED_WINDOW: &str = "a centred base-ratio window";
/// The readings of the edges the found marks follow.
const EDGES_FOLLOWED: &str = "edges are followed";
const START_FOLLOWED: &str = "the start edge is followed";
const END_FOLLOWED: &str = "the end edge is followed";
/// The reading where no other reading holds.
const NO_LAYOUT: &str = "neither a centred window nor a followed edge: the offsets say where";

/// The rule the frame is brought to the base by: its height over the base's, on the whole screen.
const BY_HEIGHT: DisplayRule = DisplayRule {
    scale_by: DisplayScaleBy::Height,
    scale_area: DisplayArea::Screen,
    anchor_area: DisplayArea::Screen,
};

/// What the layout probe reads: the input the offline `recognize` reads (its scene a saved frame,
/// given as `scene` or `scene_path`; no capture and no earlier frames), one page of the package or
/// every page, and the tolerance.
pub struct LayoutProbeRequest {
    pub input: ReadonlyRecognitionInput,
    /// One page of the package by its id; `None`: every page.
    pub page: Option<String>,
    /// How far, in scale-frame pixels, a mark may lie from a place and still be at it.
    pub tolerance_px: u32,
}

/// A size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeSize {
    pub width: u32,
    pub height: u32,
}

/// The factor the frame was brought to the base's height by, the frame's height over the base's,
/// reduced: `num / den`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeFactor {
    pub num: u32,
    pub den: u32,
}

/// A rectangle: in scale-frame pixels, or (a mark not probed) in the base's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// The `dx` each place stands for: 0, half the room rounded down, the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbePlaces {
    pub start: i64,
    pub centre: i64,
    pub end: i64,
}

/// Where a mark is: at one of the places, elsewhere (`other`), not found (`missing`), or, without
/// room, where the base puts it (`as_mapped`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkClass {
    AsMapped,
    Start,
    Centre,
    End,
    Other,
    Missing,
}

/// One template mark as the probe found it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkProbe {
    pub id: String,
    /// The mark's base place: its template, of the size it was found at, centred in its declared
    /// rectangle; `None` for a template declared over the whole frame.
    pub base: Option<ProbeRect>,
    /// The best place of its template on the scale frame, found or not.
    pub best: ProbeRect,
    /// The best place's score, normalised (0 to 1).
    pub score: f32,
    /// The mark's threshold: it is found when its score reaches it.
    pub threshold: f32,
    /// The best place's offset from the base place, across and down; `None` without a base place.
    pub dx: Option<i64>,
    pub dy: Option<i64>,
    /// `None` where the room is too small to tell.
    pub class: Option<MarkClass>,
}

impl MarkProbe {
    /// Whether the mark is found: its best score reaches its threshold.
    pub fn found(&self) -> bool {
        self.score >= self.threshold
    }
}

/// A mark a page requires that the probe does not search: no template, or a template this build
/// cannot evaluate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotProbedMark {
    pub id: String,
    pub kind: TargetKind,
    /// Its declared rectangle in the base; `None` for a mark that declares none.
    pub region: Option<ProbeRect>,
}

/// The marks of one class each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ClassCounts {
    pub as_mapped: u32,
    pub start: u32,
    pub centre: u32,
    pub end: u32,
    pub other: u32,
    pub missing: u32,
}

impl ClassCounts {
    fn add(&mut self, class: MarkClass) {
        let count = match class {
            MarkClass::AsMapped => &mut self.as_mapped,
            MarkClass::Start => &mut self.start,
            MarkClass::Centre => &mut self.centre,
            MarkClass::End => &mut self.end,
            MarkClass::Other => &mut self.other,
            MarkClass::Missing => &mut self.missing,
        };
        *count += 1;
    }
}

/// One distinct `dx` of the found marks and how many have it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DxCount {
    pub dx: i64,
    pub count: u32,
}

/// What a set of template marks says: how many were searched and found, the counts by class, the
/// distinct `dx` of the found marks and one line of reading.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeSummary {
    pub probed: u32,
    pub found: u32,
    pub counts: ClassCounts,
    pub dx_values: Vec<DxCount>,
    pub reading: String,
}

/// One page's answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageProbe {
    pub page: String,
    pub marks: Vec<MarkProbe>,
    pub not_probed: Vec<NotProbedMark>,
    pub summary: ProbeSummary,
}

/// One page's share of its template marks found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageShare {
    pub page: String,
    pub found: u32,
    pub probed: u32,
}

/// The layout probe's answer: mark ids, rectangles, offsets, scores, counts, the frame's size and
/// the factor; no pixel, no text read from the frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutProbeResponse {
    /// The chosen pages, the highest share of their template marks found first (ties in the
    /// package's order): the frame shows one page.
    pub pages_by_share: Vec<PageShare>,
    pub frame: ProbeSize,
    pub base: ProbeSize,
    pub factor: ProbeFactor,
    pub scale_frame: ProbeSize,
    /// The scale frame's width less the base's.
    pub room: u32,
    pub tolerance_px: u32,
    pub places: ProbePlaces,
    pub room_too_small: bool,
    pub pages: Vec<PageProbe>,
    /// Every template mark of the chosen pages, each once.
    pub overall: ProbeSummary,
}

impl<P: LabPorts> Lab<P> {
    /// Workflow #392 DA-L0: the layout probe (see the module header).
    pub fn layout_probe(&mut self, request: LayoutProbeRequest) -> LabResult<LayoutProbeResponse> {
        let LayoutProbeRequest {
            mut input,
            page,
            tolerance_px,
        } = request;
        if input.scene.is_none() && input.scene_path.is_none() {
            return Err(LabError::usage(
                "the layout probe reads a saved frame (--scene <png>); it captures none",
            ));
        }
        if !input.sample_scenes.is_empty() {
            return Err(LabError::usage(
                "the layout probe reads one saved frame; it takes no earlier frames",
            ));
        }
        let (evaluator, _) = load_evaluator(self, &mut input)?;
        let detector = load_page_detector(
            &input,
            "the layout probe reads a package's pages: its package holds no page set",
        )?;
        let base = package_base(&evaluator).ok_or_else(|| {
            LabError::package_invalid(
                "the package declares no coordinate_space, the base the layout probe measures \
                 against",
            )
        })?;
        let pages = chosen_pages(&detector, page.as_deref())?;
        let frame = ScaleFrame::of(base, recognition_scene(self, &mut input)?)?;
        let places = Places::new(frame.room, tolerance_px);
        let mut search = Search {
            evaluator: &evaluator,
            input: &input,
            scene: &frame.scene,
            spectra: RegionSpectra::default(),
            clock: SearchClock::system(),
        };
        // Each mark once, in the order the pages first require it.
        let mut measured = BTreeMap::new();
        let mut order = Vec::new();
        for definition in &pages {
            for id in &definition.required {
                if let Entry::Vacant(entry) = measured.entry(id.clone()) {
                    entry.insert(search.mark(id, &places)?);
                    order.push(id.clone());
                }
            }
        }
        let mut answers = Vec::with_capacity(pages.len());
        for definition in &pages {
            let mut marks = Vec::new();
            let mut not_probed = Vec::new();
            for id in &definition.required {
                match measured.get(id) {
                    Some(Measured::Probed(mark)) => marks.push(mark.clone()),
                    Some(Measured::NotProbed(mark)) => not_probed.push(mark.clone()),
                    None => {
                        return Err(LabError::device(format!(
                            "the layout probe measured no mark {id} of the page {}",
                            definition.id
                        )));
                    }
                }
            }
            let page_summary = summary(&places, &marks.iter().collect::<Vec<_>>());
            answers.push(PageProbe {
                page: definition.id.clone(),
                marks,
                not_probed,
                summary: page_summary,
            });
        }
        let every = order
            .iter()
            .filter_map(|id| match measured.get(id) {
                Some(Measured::Probed(mark)) => Some(mark),
                _ => None,
            })
            .collect::<Vec<_>>();
        let overall = summary(&places, &every);
        let mut pages_by_share = answers
            .iter()
            .map(|answer| PageShare {
                page: answer.page.clone(),
                found: answer.summary.found,
                probed: answer.summary.probed,
            })
            .collect::<Vec<_>>();
        // Stable: pages of one share keep the package's order.
        pages_by_share.sort_by(|left, right| share_order(right, left));
        Ok(LayoutProbeResponse {
            pages_by_share,
            frame: ProbeSize {
                width: frame.given.0,
                height: frame.given.1,
            },
            base: ProbeSize {
                width: base.0,
                height: base.1,
            },
            factor: frame.factor,
            scale_frame: ProbeSize {
                width: frame.scene.width(),
                height: frame.scene.height(),
            },
            room: frame.room,
            tolerance_px,
            places: ProbePlaces {
                start: 0,
                centre: places.centre,
                end: places.room,
            },
            room_too_small: places.too_small,
            pages: answers,
            overall,
        })
    }
}

/// The pages the probe answers: the one `page` names, or every page of the package in its order.
fn chosen_pages<'a>(
    detector: &'a PageDetector,
    page: Option<&str>,
) -> LabResult<Vec<&'a PageDefinition>> {
    let pages = detector.page_definitions();
    let Some(wanted) = page else {
        return Ok(pages.iter().collect());
    };
    match pages.iter().find(|definition| definition.id == wanted) {
        Some(definition) => Ok(vec![definition]),
        None => Err(LabError::usage(format!(
            "the package has no page {wanted}; its pages are {}",
            pages
                .iter()
                .map(|definition| definition.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// `left`'s share of its template marks found against `right`'s (0 for a page without one), in
/// whole numbers.
fn share_order(left: &PageShare, right: &PageShare) -> Ordering {
    let left_share = u64::from(left.found) * u64::from(right.probed.max(1));
    let right_share = u64::from(right.found) * u64::from(left.probed.max(1));
    left_share.cmp(&right_share)
}

/// The saved frame brought to the base's height.
struct ScaleFrame {
    /// The frame's own size.
    given: (u32, u32),
    factor: ProbeFactor,
    /// The scale frame.
    scene: Scene,
    room: u32,
}

impl ScaleFrame {
    /// `scene` brought to the base `base` (see the module header), or the refusal of a frame the
    /// probe does not take.
    fn of(base: (u32, u32), scene: Scene) -> LabResult<Self> {
        let given = (scene.width(), scene.height());
        let (Some(base_extent), Some(frame_extent)) = (
            CaptureExtent::new(base.0, base.1),
            CaptureExtent::new(given.0, given.1),
        ) else {
            return Err(LabError::package_invalid(format!(
                "a base of {}x{} or a frame of {}x{} has no pixel",
                base.0, base.1, given.0, given.1
            )));
        };
        if let Some(problem) = refused(base_extent, frame_extent) {
            return Err(scene_size_unsupported(
                problem,
                given,
                base,
                format!(
                    "the scene is {}x{} and its package's base {}x{}: the layout probe takes a \
                     frame of the base's orientation, at least the base's height, whose width \
                     over its height is not below the base's; nothing was searched",
                    given.0, given.1, base.0, base.1
                ),
            ));
        }
        let map = DisplayMap::build(
            base_extent,
            DisplayObservation {
                frame: frame_extent,
                display: frame_extent,
                rotation: None,
                crop: None,
                insets: None,
            },
            Some(BY_HEIGHT),
        )
        .map_err(|error| {
            LabError::device(format!(
                "the display adapter did not bring the {}x{} frame to the {}x{} base by its \
                 height: {error:?}",
                given.0, given.1, base.0, base.1
            ))
        })?;
        let target = map.scale_frame_extent();
        let size = (target.width(), target.height());
        let room = size.0.checked_sub(base.0).ok_or_else(|| {
            LabError::device(format!(
                "the {}x{} scale frame is narrower than the {}x{} base",
                size.0, size.1, base.0, base.1
            ))
        })?;
        let scale = map.scale();
        let factor = ProbeFactor {
            num: scale.num(),
            den: scale.den(),
        };
        let scene = if size == given {
            scene
        } else {
            let pixels = resample_pixels(scene.rgb8_pixels(), given, 3, size).map_err(|error| {
                LabError::device(format!(
                    "the {}x{} frame could not be resampled to {}x{}: {error}",
                    given.0, given.1, size.0, size.1
                ))
            })?;
            Scene::from_rgb8(size.0, size.1, &pixels).map_err(|error| {
                LabError::device(format!("the {}x{} scale frame: {error}", size.0, size.1))
            })?
        };
        Ok(Self {
            given,
            factor,
            scene,
            room,
        })
    }
}

/// The word of a frame the probe does not take for the base `base`, in this order: the base's
/// other orientation, lower than the base, narrower than the base's ratio; `None` for a frame it
/// takes.
fn refused(base: CaptureExtent, frame: CaptureExtent) -> Option<DisplayProblem> {
    if !Orientation::of(frame).agrees_with(Orientation::of(base)) {
        return Some(DisplayProblem::OrientationMismatch);
    }
    if frame.height() < base.height() {
        return Some(DisplayProblem::ScaleBelowBase);
    }
    let across = u64::from(frame.width()) * u64::from(base.height());
    let base_across = u64::from(base.width()) * u64::from(frame.height());
    (across < base_across).then_some(DisplayProblem::RatioBelowBase)
}

/// The places a found mark is classed by (see the module header), in scale-frame pixels.
#[derive(Debug, Clone, Copy)]
struct Places {
    room: i64,
    centre: i64,
    tolerance: i64,
    /// The room is above 0 and the windows of the start and the centre overlap (and so those of
    /// the centre and the end, which lie at least as far apart).
    too_small: bool,
}

impl Places {
    fn new(room: u32, tolerance_px: u32) -> Self {
        let room = i64::from(room);
        let centre = room / 2;
        let tolerance = i64::from(tolerance_px);
        Self {
            room,
            centre,
            tolerance,
            too_small: room > 0 && centre <= 2 * tolerance,
        }
    }

    /// Whether `value` lies within the tolerance of `place`.
    fn near(&self, value: i64, place: i64) -> bool {
        (value - place).abs() <= self.tolerance
    }

    /// The class of a mark `found` or not, whose best place lies `offsets` (`dx`, `dy`) from its
    /// base place, `None` without one; `None` where the room is too small to tell.
    fn class_of(&self, found: bool, offsets: Option<(i64, i64)>) -> Option<MarkClass> {
        if self.too_small {
            return None;
        }
        if !found {
            return Some(MarkClass::Missing);
        }
        let Some((dx, dy)) = offsets else {
            return Some(MarkClass::Other);
        };
        Some(if !self.near(dy, 0) {
            MarkClass::Other
        } else if self.room == 0 {
            if self.near(dx, 0) {
                MarkClass::AsMapped
            } else {
                MarkClass::Other
            }
        } else if self.near(dx, 0) {
            MarkClass::Start
        } else if self.near(dx, self.centre) {
            MarkClass::Centre
        } else if self.near(dx, self.room) {
            MarkClass::End
        } else {
            MarkClass::Other
        })
    }

    /// The inset of a mark at none of the places whose best place lies `dx` across from its base
    /// place and within the tolerance of it down: its distance from the edge it lies nearer than to
    /// the centre, inward of it; `None` for one nearest the centre or outward of the edge.
    fn inset(&self, dx: i64, dy: i64) -> Option<i64> {
        if !self.near(dy, 0) {
            return None;
        }
        let from_start = dx.abs();
        let from_centre = (dx - self.centre).abs();
        let from_end = (dx - self.room).abs();
        if dx > 0 && from_start < from_centre && from_start <= from_end {
            Some(dx)
        } else if dx < self.room && from_end < from_centre && from_end < from_start {
            Some(self.room - dx)
        } else {
            None
        }
    }
}

/// The counts, the `dx` values and the reading of `marks`.
fn summary(places: &Places, marks: &[&MarkProbe]) -> ProbeSummary {
    let mut counts = ClassCounts::default();
    let mut dx_values = BTreeMap::<i64, u32>::new();
    let mut found = 0_u32;
    for mark in marks {
        if let Some(class) = mark.class {
            counts.add(class);
        }
        if mark.found() {
            found += 1;
            if let Some(dx) = mark.dx {
                *dx_values.entry(dx).or_insert(0) += 1;
            }
        }
    }
    ProbeSummary {
        probed: u32::try_from(marks.len()).unwrap_or(u32::MAX),
        found,
        counts,
        dx_values: dx_values
            .into_iter()
            .map(|(dx, count)| DxCount { dx, count })
            .collect(),
        reading: reading(places, marks),
    }
}

/// One line that describes what `marks` show (see the module header); it decides nothing.
fn reading(places: &Places, marks: &[&MarkProbe]) -> String {
    if places.too_small {
        return ROOM_TOO_SMALL.to_owned();
    }
    if marks.is_empty() {
        return NOTHING_PROBED.to_owned();
    }
    let found = marks
        .iter()
        .filter(|mark| mark.found())
        .copied()
        .collect::<Vec<_>>();
    if found.is_empty() {
        return NOT_THIS_PAGE.to_owned();
    }
    let any = |class: MarkClass| found.iter().any(|mark| mark.class == Some(class));
    let all = |class: MarkClass| found.iter().all(|mark| mark.class == Some(class));
    if places.room == 0 {
        let line = if all(MarkClass::AsMapped) {
            AS_MAPPED
        } else {
            OFF_THE_BASE
        };
        return line.to_owned();
    }
    if all(MarkClass::Centre) {
        return CENTRED_WINDOW.to_owned();
    }
    let mut parts = Vec::new();
    match (any(MarkClass::Start), any(MarkClass::End)) {
        (true, true) => parts.push(EDGES_FOLLOWED.to_owned()),
        (true, false) => parts.push(START_FOLLOWED.to_owned()),
        (false, true) => parts.push(END_FOLLOWED.to_owned()),
        (false, false) => {}
    }
    let insets = found
        .iter()
        .filter(|mark| mark.class == Some(MarkClass::Other))
        .filter_map(|mark| places.inset(mark.dx?, mark.dy?))
        .collect::<BTreeSet<_>>();
    parts.extend(
        insets
            .into_iter()
            .map(|inset| format!("an inset of about {inset}")),
    );
    if parts.is_empty() {
        return NO_LAYOUT.to_owned();
    }
    parts.join("; ")
}

/// A mark a page requires, as the probe measured it.
#[derive(Debug, Clone)]
enum Measured {
    Probed(MarkProbe),
    NotProbed(NotProbedMark),
}

/// The search of the marks on one scale frame, sharing its region's transforms.
struct Search<'a> {
    evaluator: &'a RecognitionEvaluator,
    input: &'a ReadonlyRecognitionInput,
    scene: &'a Scene,
    spectra: RegionSpectra,
    clock: SearchClock,
}

impl Search<'_> {
    /// The mark `id` of the recognition pack: searched when it is a template this build evaluates,
    /// listed as not probed otherwise.
    fn mark(&mut self, id: &str, places: &Places) -> LabResult<Measured> {
        let target = self
            .evaluator
            .pack()
            .targets
            .iter()
            .find(|target| declared(target).0 == id)
            .ok_or_else(|| {
                LabError::package_invalid(format!(
                    "a page requires the mark {id}, which the recognition pack does not declare"
                ))
            })?;
        let unsupported = self
            .evaluator
            .unsupported_targets()
            .iter()
            .any(|unsupported| unsupported.id == id);
        match target {
            RecognitionTarget::Template(template) if !unsupported => {
                self.template(template, places).map(Measured::Probed)
            }
            other => {
                let (name, kind, region) = declared(other);
                Ok(Measured::NotProbed(NotProbedMark {
                    id: name.to_owned(),
                    kind,
                    region,
                }))
            }
        }
    }

    /// The template mark `target` searched over the whole scale frame (see the module header).
    fn template(&mut self, target: &TemplateTarget, places: &Places) -> LabResult<MarkProbe> {
        let png = self
            .input
            .resources
            .loaded_bundle()
            .resource_entry(&target.template_path)
            .map_err(|error| {
                LabError::package_invalid(format!(
                    "the template {} of the mark {} does not read: {error}",
                    target.template_path, target.id
                ))
            })?;
        // A mark this build evaluates declares a gray range in a schema 0.7 pack or no mask: a
        // mask of another schema is unsupported, and admission refuses any other in 0.7.
        let mask = match &target.mask {
            None => None,
            Some(RecognitionMask::Range { lower, upper }) => Some(TemplateMask {
                lower: *lower,
                upper: *upper,
            }),
            Some(RecognitionMask::Bitmap { .. }) => {
                return Err(LabError::package_invalid(format!(
                    "the mark {} declares a bitmap mask, which no admitted package holds",
                    target.id
                )));
            }
        };
        let factors = target.scales.as_ref().map_or_else(
            || vec![SCALE_IDENTITY_MILLI],
            |scales| scales.factors_milli.clone(),
        );
        let best = self
            .scene
            .exact_score_maps(
                png,
                None,
                self.evaluator.default_match_metric(),
                TemplateMatchTimingStage::Exact,
                mask,
                &factors,
                &mut self.spectra,
                &self.clock,
            )
            .and_then(|maps| maps.best())
            .map_err(|error| {
                LabError::device(format!(
                    "the layout probe could not search the mark {}: {error}",
                    target.id
                ))
            })?
            .matched;
        let threshold = target
            .threshold
            .unwrap_or(self.evaluator.pack().defaults.template_threshold);
        let best_place = ProbeRect {
            x: i64::from(best.x),
            y: i64::from(best.y),
            width: i64::from(best.width),
            height: i64::from(best.height),
        };
        let base = match &target.region {
            PackRegion::Rect(region) => Some(base_place(*region, best_place)),
            PackRegion::Keyword(_) | PackRegion::TemplateRelative(_) => None,
        };
        let offsets = base.map(|base| (best_place.x - base.x, best_place.y - base.y));
        let found = best.score >= threshold;
        Ok(MarkProbe {
            id: target.id.clone(),
            base,
            best: best_place,
            score: best.score,
            threshold,
            dx: offsets.map(|(dx, _)| dx),
            dy: offsets.map(|(_, dy)| dy),
            class: places.class_of(found, offsets),
        })
    }
}

/// The base place of a template found at `best`: a rectangle of `best`'s size centred in the
/// declared `region` (half the difference rounded down).
fn base_place(region: PackRect, best: ProbeRect) -> ProbeRect {
    ProbeRect {
        x: i64::from(region.x) + (i64::from(region.width) - best.width).div_euclid(2),
        y: i64::from(region.y) + (i64::from(region.height) - best.height).div_euclid(2),
        width: best.width,
        height: best.height,
    }
}

/// A declared rectangle of the base.
fn rect(region: PackRect) -> ProbeRect {
    ProbeRect {
        x: i64::from(region.x),
        y: i64::from(region.y),
        width: i64::from(region.width),
        height: i64::from(region.height),
    }
}

/// The rectangle a region declares, when it declares one.
fn region_rect(region: &PackRegion) -> Option<ProbeRect> {
    match region {
        PackRegion::Rect(region) => Some(rect(*region)),
        PackRegion::Keyword(_) | PackRegion::TemplateRelative(_) => None,
    }
}

/// A mark's id, its kind and the rectangle it declares in the base, when it declares one.
fn declared(target: &RecognitionTarget) -> (&str, TargetKind, Option<ProbeRect>) {
    match target {
        RecognitionTarget::Template(target) => (
            target.id.as_str(),
            TargetKind::Template,
            region_rect(&target.region),
        ),
        RecognitionTarget::Color(target) => (
            target.id.as_str(),
            TargetKind::Color,
            Some(rect(target.region)),
        ),
        RecognitionTarget::ClickOnly(target) => (target.id.as_str(), TargetKind::ClickOnly, None),
        RecognitionTarget::Ocr(target) => (
            target.id.as_str(),
            TargetKind::Ocr,
            region_rect(&target.region),
        ),
        RecognitionTarget::Nn(target) => (
            target.id.as_str(),
            TargetKind::Nn,
            region_rect(&target.region),
        ),
        RecognitionTarget::ColorDigest(target) => (
            target.id.as_str(),
            TargetKind::ColorDigest,
            Some(rect(target.region)),
        ),
        RecognitionTarget::Composite(target) => (target.id.as_str(), TargetKind::Composite, None),
        RecognitionTarget::ValueTest(target) => (target.id.as_str(), TargetKind::ValueTest, None),
    }
}
