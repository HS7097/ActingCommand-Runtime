// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-L0 (`contracts/display-adapter.md` section 14; register §1.521): the layout
//! probe, an offline Lab operation (`layout probe`, MCP `ac_layout_probe`). From one saved frame of
//! the base's ratio or wider and a package it says, for every template mark the chosen pages name,
//! where the mark is found on the frame brought to the base's height and how that place relates to
//! the place the base gives it. It describes a measurement: it writes no declaration and decides
//! nothing.
//!
//! - **What it reads.** The package and the scene as the offline `recognize` reads them, from the
//!   same input ([`crate::ReadonlyRecognitionInput`]: the externally verified bundle with its
//!   recognition pack and page set, and the scene file). No capture, no device input, no run, no
//!   Runtime; nothing is written.
//! - **The frame** ([`ScaleFrame::of`]). A frame the run path's frame gate takes as the base's (its
//!   size, or its shape within a pixel: the kernel's `lab_scene_on_base`) is that gate's reduced
//!   frame, with no room. Any other frame of the base's orientation, at least the base's height and
//!   whose width over its height is not below the base's is brought to the base's height by the
//!   display adapter's map measured by height on the screen (`DisplayMap::build`) and resampled
//!   once by the function the frame gate resamples with (REC-5, `resample_pixels`, area average):
//!   the scale frame, of the base's height and the base's width plus the room. Any other frame is
//!   refused `lab_scene_size_unsupported` before anything is searched, with its word: the base's
//!   other orientation `orientation_mismatch`, lower than the base `scale_below_base`, narrower
//!   than the base's ratio `ratio_below_base`.
//! - **The marks of a page** ([`named`]): every mark its rules name, `required`, then the members
//!   of its `any_of` groups, `optional`, `forbidden`, each once, with the rule that names it.
//! - **A mark** ([`Search::template`]). A template mark is searched over the whole scale frame, its
//!   declared region aside, with its template scored as recognition scores it (the pack's metric,
//!   its mask by `scored_mask`, its factors by `searched_factors`, its threshold by
//!   `RecognitionPack::template_threshold`, on the exact score maps): its best place, its score, its
//!   threshold and how many places reach the threshold, overlaps suppressed (`matches`; above one
//!   the mark is `repeated`). Every other mark (a colour, a colour digest, an OCR, a check, a value
//!   test, a template this build cannot evaluate) is listed as not probed, with its kind and its
//!   declared rectangle.
//! - **The base place** ([`base_place`]). A package keeps a mark's search box, not where its
//!   template was cut. On each axis the base place is the recording's default box read backwards
//!   (`default_search`, the crop grown by `LAB_RECORDING_DEFAULT_SEARCH_MARGIN` on each side and
//!   clamped to the base): a box off the start edge puts the crop one margin inside its start; a box
//!   at the start edge and off the end edge puts it one margin and the template's length before its
//!   end; the place is taken (`default_growth`) only when the recording's own function grows it back
//!   into the declared box. A box across the whole axis, one of any other size (a search area
//!   someone declared), a template declared over the whole frame or relative to another mark: that
//!   axis is `unknown`. `dx` and `dy` are the best place's offset from the base place, on the axes
//!   where it is known.
//! - **Classes** ([`Places`]). With a room `R`, the places are `start` (0), `centre` (`R / 2`
//!   rounded down, as the nine-grid's middle) and `end` (`R`); a found mark (its score reaches its
//!   threshold) whose `dy` (when known) and `dx` lie within the tolerance of 0 and of a place is at
//!   that place, any other found mark `other`, a mark not found `missing`. Without room the three
//!   places are one: `as_mapped`. A found mark whose base place across is unknown has no class and
//!   is counted `unknown`. Where the windows of the start and the centre would overlap (`R > 0` and
//!   `R / 2` rounded down at most twice the tolerance) the answer says the room is too small to
//!   tell and classes no mark.
//! - **Per page and overall** ([`summary`]): the counts by class, the distinct `dx` of the found
//!   marks with their counts and one line of reading ([`reading`]) over the found marks that take
//!   part in it (a known place across, not repeated, not forbidden); the pages ordered by the share
//!   of their template marks found come first.

use crate::display_base::scene_size_unsupported;
use crate::readonly::{load_evaluator, load_page_detector, recognition_scene};
use crate::recording::marks::default_search;
use crate::{
    LAB_RECORDING_DEFAULT_SEARCH_MARGIN, Lab, LabPorts, ReadonlyRecognitionInput, RecordRect,
    RecordSize, package_base,
};
use actingcommand_contract::{CaptureExtent, DisplayArea, DisplayScaleBy, LabError, LabResult};
use actingcommand_display_adapter::{
    DisplayMap, DisplayMapError, DisplayObservation, DisplayProblem, DisplayRule, Orientation,
};
use actingcommand_execution_kernel::lab_scene_on_base;
use actingcommand_page_detector::{PageDefinition, PageDetector};
use actingcommand_recognition::resample::resample_pixels;
use actingcommand_recognition::{
    RecognitionError, RegionSpectra, Scene, SearchClock, TemplateMatchTimingStage,
};
use actingcommand_recognition_pack::{
    PackRect, PackRegion, RecognitionEvaluator, RecognitionTarget, TargetKind, TemplateTarget,
    scored_mask, searched_factors,
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
/// The reading of a page none of whose marks it looks for is a template.
const NOTHING_PROBED: &str = "no template mark to probe";
/// The reading where no template mark the page looks for is found.
const NOT_THIS_PAGE: &str = "not this page";
/// The reading where marks are found but none takes part in the reading.
const NOTHING_PLACED: &str =
    "no found mark to read: its base place across is unknown or it is repeated";
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

/// A rectangle: in scale-frame pixels, or (a declared box) in the base's.
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

/// The rule of a page that names a mark: it requires it, it is a member of one of its `any_of`
/// groups, it is optional, or it is forbidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkRule {
    Required,
    AnyOf,
    Optional,
    Forbidden,
}

/// How a mark's base place was taken on one axis: the recording's default box read backwards, or
/// not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BasePlaceBy {
    DefaultGrowth,
    Unknown,
}

/// A mark's base place on one axis: where its template starts in the base, and how that was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AxisPlace {
    /// `None` where the axis is `unknown`.
    pub at: Option<i64>,
    pub by: BasePlaceBy,
}

impl AxisPlace {
    fn of(at: Option<i64>) -> Self {
        let by = if at.is_some() {
            BasePlaceBy::DefaultGrowth
        } else {
            BasePlaceBy::Unknown
        };
        Self { at, by }
    }
}

/// A mark's base place, across and down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProbeBase {
    pub x: AxisPlace,
    pub y: AxisPlace,
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
    /// The page's rule that names the mark.
    pub rule: MarkRule,
    /// The mark's declared search box in the base; `None` for a template declared over the whole
    /// frame or relative to another mark.
    pub region: Option<ProbeRect>,
    /// The size of its template as the package holds it.
    pub template: ProbeSize,
    /// Its base place, axis by axis (see the module header).
    pub base: ProbeBase,
    /// The best place of its template on the scale frame, found or not.
    pub best: ProbeRect,
    /// The best place's score, normalised (0 to 1).
    pub score: f32,
    /// The mark's threshold: it is found when its score reaches it.
    pub threshold: f32,
    /// How many places of the scale frame reach the threshold, overlapping places taken once.
    pub matches: u32,
    /// More than one place reaches the threshold: the best place is one of them.
    pub repeated: bool,
    /// The best place's offset from the base place, across and down; `None` where that axis of the
    /// base place is unknown.
    pub dx: Option<i64>,
    pub dy: Option<i64>,
    /// `None` where the room is too small to tell, or for a found mark whose base place across is
    /// unknown.
    pub class: Option<MarkClass>,
}

impl MarkProbe {
    /// Whether the mark is found: its best score reaches its threshold.
    pub fn found(&self) -> bool {
        self.score >= self.threshold
    }

    /// Whether the mark takes part in its page's reading: found, its base place across known, not
    /// repeated and not forbidden.
    fn takes_part(&self) -> bool {
        self.found() && self.dx.is_some() && !self.repeated && self.rule != MarkRule::Forbidden
    }
}

/// A mark a page names that the probe does not search: no template, or a template this build
/// cannot evaluate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotProbedMark {
    pub id: String,
    /// The page's rule that names the mark.
    pub rule: MarkRule,
    pub kind: TargetKind,
    /// Its declared rectangle in the base; `None` for a mark that declares none.
    pub region: Option<ProbeRect>,
}

/// The marks of one class each, and the found marks without a class (`unknown`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ClassCounts {
    pub as_mapped: u32,
    pub start: u32,
    pub centre: u32,
    pub end: u32,
    pub other: u32,
    pub missing: u32,
    pub unknown: u32,
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
    /// Every template mark of the chosen pages, each once, as the first page that names it
    /// answers it.
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
            base,
            spectra: RegionSpectra::default(),
            clock: SearchClock::system(),
        };
        // Each mark once, in the order the pages first name it.
        let mut measured = BTreeMap::new();
        for definition in &pages {
            for (id, _) in named(definition) {
                if let Entry::Vacant(entry) = measured.entry(id.to_owned()) {
                    entry.insert(search.mark(id, &places)?);
                }
            }
        }
        let mut answers = Vec::with_capacity(pages.len());
        for definition in &pages {
            let mut marks = Vec::new();
            let mut not_probed = Vec::new();
            for (id, rule) in named(definition) {
                match measured.get(id) {
                    Some(Measured::Probed(mark)) => marks.push(MarkProbe {
                        rule,
                        ..mark.clone()
                    }),
                    Some(Measured::NotProbed(mark)) => not_probed.push(NotProbedMark {
                        rule,
                        ..mark.clone()
                    }),
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
        // Each mark once, as the first page that names it answers it.
        let mut seen = BTreeSet::new();
        let mut every = Vec::new();
        for answer in &answers {
            for mark in &answer.marks {
                if seen.insert(mark.id.as_str()) {
                    every.push(mark);
                }
            }
        }
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

/// Every mark the rules of the page `definition` name, each once with the first rule that names
/// it, in the order `required`, the members of `any_of`, `optional`, `forbidden`.
fn named(definition: &PageDefinition) -> Vec<(&str, MarkRule)> {
    let rules = definition
        .required
        .iter()
        .map(|id| (id, MarkRule::Required))
        .chain(
            definition
                .any_of
                .iter()
                .flatten()
                .map(|id| (id, MarkRule::AnyOf)),
        )
        .chain(
            definition
                .optional
                .iter()
                .map(|id| (id, MarkRule::Optional)),
        )
        .chain(
            definition
                .forbidden
                .iter()
                .map(|id| (id, MarkRule::Forbidden)),
        );
    let mut list: Vec<(&str, MarkRule)> = Vec::new();
    for (id, rule) in rules {
        if !list.iter().any(|(known, _)| *known == id.as_str()) {
            list.push((id.as_str(), rule));
        }
    }
    list
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
        // A frame the run path takes as the base's: the frame gate's own reduction, no room. A
        // frame it refuses is the probe's to judge below.
        if let Ok(reduced) = lab_scene_on_base(base, &scene) {
            let (scene, factor) = match reduced {
                None => (scene, ProbeFactor { num: 1, den: 1 }),
                Some(reduced) => {
                    let down = reduced.map.height_scale();
                    (
                        reduced.scene,
                        ProbeFactor {
                            num: down.num(),
                            den: down.den(),
                        },
                    )
                }
            };
            return Ok(Self {
                given,
                factor,
                scene,
                room: 0,
            });
        }
        let (Some(base_extent), Some(frame_extent)) = (
            CaptureExtent::new(base.0, base.1),
            CaptureExtent::new(given.0, given.1),
        ) else {
            return Err(LabError::package_invalid(format!(
                "a base of {}x{} or a frame of {}x{} has no pixel",
                base.0, base.1, given.0, given.1
            )));
        };
        let refusal = |problem: DisplayProblem| {
            scene_size_unsupported(
                problem,
                given,
                base,
                format!(
                    "the scene is {}x{} and its package's base {}x{}: the layout probe takes a \
                     frame of the base's orientation, at least the base's height, whose width \
                     over its height is not below the base's; nothing was searched",
                    given.0, given.1, base.0, base.1
                ),
            )
        };
        // A saved frame has no rotation, and the adapter answers a frame of the base's other
        // orientation without one as a rotation unknown: the probe says the orientation itself.
        if !Orientation::of(frame_extent).agrees_with(Orientation::of(base_extent)) {
            return Err(refusal(DisplayProblem::OrientationMismatch));
        }
        // A frame lower than the base: the adapter's own refusal, `scale_below_base`.
        let built = DisplayMap::build(
            base_extent,
            DisplayObservation {
                frame: frame_extent,
                display: frame_extent,
                rotation: None,
                crop: None,
                insets: None,
            },
            Some(BY_HEIGHT),
        );
        let map = match built {
            Ok(map) => map,
            Err(DisplayMapError::Unresolved(unresolved)) => {
                return Err(refusal(unresolved.problem));
            }
            Err(error) => {
                return Err(LabError::device(format!(
                    "the display adapter did not bring the {}x{} frame to the {}x{} base by its \
                     height: {error:?}",
                    given.0, given.1, base.0, base.1
                )));
            }
        };
        // A frame narrower than the base's ratio, which the adapter brings to the base's height
        // all the same: the probe's own word, `ratio_below_base`.
        if u64::from(given.0) * u64::from(base.1) < u64::from(base.0) * u64::from(given.1) {
            return Err(refusal(DisplayProblem::RatioBelowBase));
        }
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

/// The places a found mark is classed by (see the module header), in scale-frame pixels.
#[derive(Debug, Clone, Copy)]
struct Places {
    room: i64,
    centre: i64,
    tolerance: i64,
    /// The room is above 0 and the windows of the start and the centre overlap (and so those of
    /// the centre and the end, which lie at least as far apart): `R / 2` rounded down at most twice
    /// the tolerance.
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

    /// The class of a mark `found` or not, whose best place lies `dx` across and `dy` down from
    /// its base place (`None` on an axis where that is unknown); `None` where the room is too small
    /// to tell, and for a found mark whose base place across is unknown.
    fn class_of(&self, found: bool, dx: Option<i64>, dy: Option<i64>) -> Option<MarkClass> {
        if self.too_small {
            return None;
        }
        if !found {
            return Some(MarkClass::Missing);
        }
        let dx = dx?;
        Some(if dy.is_some_and(|dy| !self.near(dy, 0)) {
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
    /// place and, where `dy` is known, within the tolerance of it down: its distance from the edge
    /// it lies nearer than to the centre, inward of it; `None` for one nearest the centre or
    /// outward of the edge.
    fn inset(&self, dx: i64, dy: Option<i64>) -> Option<i64> {
        if dy.is_some_and(|dy| !self.near(dy, 0)) {
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
        match mark.class {
            Some(class) => counts.add(class),
            // Found, its base place across unknown: counted apart (no count where the room is too
            // small to tell).
            None if !places.too_small && mark.found() => counts.unknown += 1,
            None => {}
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
    let looked_for = marks
        .iter()
        .filter(|mark| mark.rule != MarkRule::Forbidden)
        .copied()
        .collect::<Vec<_>>();
    if looked_for.is_empty() {
        return NOTHING_PROBED.to_owned();
    }
    if !looked_for.iter().any(|mark| mark.found()) {
        return NOT_THIS_PAGE.to_owned();
    }
    let found = looked_for
        .iter()
        .filter(|mark| mark.takes_part())
        .copied()
        .collect::<Vec<_>>();
    if found.is_empty() {
        return NOTHING_PLACED.to_owned();
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
        .filter_map(|mark| places.inset(mark.dx?, mark.dy))
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

/// A mark a page names, as the probe measured it; its rule is the page's, set per page.
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
    /// The package's base, the frame its marks' boxes were grown in.
    base: (u32, u32),
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
                    "a page names the mark {id}, which the recognition pack does not declare"
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
                    rule: MarkRule::Required,
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
        let own = Scene::from_png(png).map_err(|error| {
            LabError::package_invalid(format!(
                "the template {} of the mark {} does not decode: {error}",
                target.template_path, target.id
            ))
        })?;
        let template = (own.width(), own.height());
        let pack = self.evaluator.pack();
        let threshold = pack.template_threshold(target);
        let searched = |error: RecognitionError| {
            LabError::device(format!(
                "the layout probe could not search the mark {}: {error}",
                target.id
            ))
        };
        let maps = self
            .scene
            .exact_score_maps(
                png,
                None,
                self.evaluator.default_match_metric(),
                TemplateMatchTimingStage::Exact,
                scored_mask(&pack.schema_version, target),
                &searched_factors(target),
                &mut self.spectra,
                &self.clock,
            )
            .map_err(searched)?;
        let best = maps.best().map_err(searched)?.matched;
        let at_threshold = maps
            .matches_at_least(threshold, |_| Ok(true))
            .and_then(|places_found| maps.suppress_overlaps(places_found, 0))
            .map_err(searched)?;
        let matches = u32::try_from(at_threshold.len()).unwrap_or(u32::MAX);
        let best_place = ProbeRect {
            x: i64::from(best.x),
            y: i64::from(best.y),
            width: i64::from(best.width),
            height: i64::from(best.height),
        };
        let region = region_rect(&target.region);
        let base = base_place(region, template, self.base);
        let dx = base.x.at.map(|at| best_place.x - at);
        let dy = base.y.at.map(|at| best_place.y - at);
        let found = best.score >= threshold;
        Ok(MarkProbe {
            id: target.id.clone(),
            rule: MarkRule::Required,
            region,
            template: ProbeSize {
                width: template.0,
                height: template.1,
            },
            base,
            best: best_place,
            score: best.score,
            threshold,
            matches,
            repeated: matches > 1,
            dx,
            dy,
            class: places.class_of(found, dx, dy),
        })
    }
}

/// The base place of a template of `template` size (width, height) searched in the declared box
/// `region` of the base `base`, axis by axis (see the module header); every axis unknown without a
/// box.
fn base_place(region: Option<ProbeRect>, template: (u32, u32), base: (u32, u32)) -> ProbeBase {
    let Some(region) = region else {
        return ProbeBase {
            x: AxisPlace::of(None),
            y: AxisPlace::of(None),
        };
    };
    let across = axis_crop(
        region.x,
        region.width,
        i64::from(template.0),
        i64::from(base.0),
    )
    .filter(|x| {
        grown(*x, 0, template, base).is_some_and(|default_box| {
            (i64::from(default_box.x), i64::from(default_box.width)) == (region.x, region.width)
        })
    });
    let down = axis_crop(
        region.y,
        region.height,
        i64::from(template.1),
        i64::from(base.1),
    )
    .filter(|y| {
        grown(0, *y, template, base).is_some_and(|default_box| {
            (i64::from(default_box.y), i64::from(default_box.height)) == (region.y, region.height)
        })
    });
    ProbeBase {
        x: AxisPlace::of(across),
        y: AxisPlace::of(down),
    }
}

/// Where on one axis the crop of a box starting at `start`, `length` long, began, by the
/// recording's default growth read backwards, for a template `template` long in a base `frame`
/// long: one margin inside a start off the edge; one margin and the template before an end off the
/// edge, for a box at the start edge; `None` for a box across the whole axis or a crop outside the
/// base. Whether the recording grows that crop back into the box is [`grown`]'s to say.
fn axis_crop(start: i64, length: i64, template: i64, frame: i64) -> Option<i64> {
    let margin = i64::from(LAB_RECORDING_DEFAULT_SEARCH_MARGIN);
    let end = start + length;
    let crop = if start > 0 {
        start + margin
    } else if start == 0 && end < frame {
        end - margin - template
    } else {
        return None;
    };
    (0..=frame - template).contains(&crop).then_some(crop)
}

/// The recording's default box (`default_search`) of a crop of `template` size at (`x`, `y`) in
/// the base `base`; `None` for a length outside the recording's range.
fn grown(x: i64, y: i64, template: (u32, u32), base: (u32, u32)) -> Option<RecordRect> {
    let crop = RecordRect {
        x: i32::try_from(x).ok()?,
        y: i32::try_from(y).ok()?,
        width: i32::try_from(template.0).ok()?,
        height: i32::try_from(template.1).ok()?,
    };
    Some(default_search(
        crop,
        RecordSize {
            width: base.0,
            height: base.1,
        },
    ))
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
