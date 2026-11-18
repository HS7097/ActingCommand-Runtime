// SPDX-License-Identifier: AGPL-3.0-only

//! #463 PRIV-1 (`contracts/privacy-zones.md`, "The read regions of a package"): the one list of
//! the regions a recognition pack's targets read. The Lab's scene of a regression case pastes the
//! regions its targets declare themselves (`record stop`, `contracts/resource-regression-case.md`)
//! and the privacy comparison of resource tooling compares every region of it with a repository's
//! sensitive zones; neither makes a list of its own.
//!
//! - **What is in it**, in the pack's order: each target in `targets` with what it reads of its
//!   own (a template's search area, `region`, where `record mark` writes the crop with its margin,
//!   then its colour check's region; a colour or colour digest probe's region; an OCR or NN
//!   target's region); then, layout by layout, what a candidate layout reads itself: its shared
//!   OCR read (`shared_ocr.region`, as declared, of a layout of either kind: since #463 SEL-2L-b a
//!   `repeated_anchor` layout's covers its readable band), then (fix round 1) each feature that
//!   reads its target at an `offset`, where the kernel reads it: in a `fixed_slots` layout once per
//!   slot, the target's own size at the slot's origin plus the offset; in a `repeated_anchor`
//!   layout at the anchor's match plus the offset, scaled as the match is, which only a frame
//!   shows, so no fixed rectangle, whether or not the layout declares a shared read. An
//!   OCR target a candidate layout reads as a feature is a select's quantity column when a feature
//!   reads it as an integer (`ocr_integer`), else a select's name region. A check (`all_of`,
//!   `any_of`), a value test and a click-only target read nothing of their own.
//! - **What is not in it**: a press box (a target's `click`, a click-only target, a slot's
//!   `click`, a layout's `click`): pressing reads nothing.
//! - **Each region** carries the target it belongs to (a shared read: its layout's id), its kind,
//!   where it lies (a fixed rectangle, the keyword `full_frame`, or no fixed rectangle at all), the
//!   layout whose read it is (none for a region its target declares itself, the only regions a
//!   case's scene pastes) and whether it is a reading.
//! - **Where a read can reach** (fix round 2): a sample of a target consensus or of a feature's
//!   consensus reads the region moved by its jitter (at most 8 pixels on each axis). A region that
//!   is a fixed rectangle carries its reach, the smallest rectangle that holds it under every shift
//!   the pack declares for it, the region itself when it declares none. The scene still pastes the
//!   region itself.
//! - **Where an unbounded read is confined** (fix round 2): a feature a `repeated_anchor` layout
//!   reads at an offset carries the rectangle the pack confines it to, when it guarantees one
//!   ([`RecognitionPack::reads`]); any other region with no fixed rectangle carries none.
//! - **A reading** (fix round 2, the one meaning) is a region the kernel reads as a number at run
//!   time, whether or not the number is published as a fact: its target is one
//!   [`RecognitionPack::reading_targets`] collects; and a layout's shared read is one when it gives
//!   its text to a reading target.
//! - **What a rectangle does not show** is counted beside the list: the texts an OCR target
//!   expects and the aliases a layout's identity entries are read as.

use crate::candidate_layout::shared_calls;
use crate::template_scales::searched_factors;
use crate::{
    CandidateFeatureDeclaration, CandidateFeatureValue, CandidateLayout, CandidateSampleVariant,
    PackPoint, PackRect, PackRegion, RecognitionPack, RecognitionTarget, ValueTestRight,
    rect_is_within,
};
use actingcommand_contract::candidate_projection::CandidateLayoutKind;
use actingcommand_contract::{
    OcrFieldDeclaration, OcrFieldType, OcrFieldsDeclaration, ResourceAdjustmentDeclaration,
    ResourceReadingDeclaration,
};
use actingcommand_recognition::resample::SCALE_IDENTITY_MILLI;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeSet;

/// What part of a recognition target a read region is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackReadKind {
    /// A template target's search area, its `region`: it holds the template's crop, with the
    /// margin `record mark` gives a search area it is not told.
    TemplateSearch,
    /// A template target's colour check, `color_check.region`.
    ColorCheck,
    /// A colour probe's region.
    Color,
    /// A colour digest's region.
    ColorDigest,
    /// An OCR target's region, when no candidate layout reads the target as a feature.
    Ocr,
    /// An NN target's region.
    Nn,
    /// A candidate layout's shared OCR read, `shared_ocr.region` (a `fixed_slots` or a
    /// `repeated_anchor` layout's); its target is the layout's id.
    SharedOcr,
    /// An OCR target a candidate layout reads as a feature, and none as an integer: a select's
    /// name region.
    SelectName,
    /// Fix round 1: an OCR target a candidate layout reads as an integer feature (`ocr_integer`):
    /// a select's quantity column, which a `read_list` step publishes as an integer column.
    SelectQuantity,
}

/// Where a read region lies, as the pack declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackReadArea {
    /// A fixed rectangle of the pack's coordinate space.
    Rect(PackRect),
    /// The keyword region `full_frame`: the whole frame, whatever its size.
    FullFrame,
    /// No fixed rectangle: a region placed relative to where a template was found
    /// (`template_relative`, or a feature a `repeated_anchor` layout reads at an offset from its
    /// anchor's match), or a keyword other than `full_frame`. Fix round 2: the region's `bound`
    /// says the rectangle the pack confines it to, when it guarantees one.
    Unbounded,
}

/// One region a recognition target of a pack reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackReadRegion {
    /// The target's id; a layout's shared read names its layout's id.
    pub target: String,
    pub kind: PackReadKind,
    pub area: PackReadArea,
    /// Fix round 2 (`contracts/privacy-zones.md`, "Where a read can reach"): of an area that is a
    /// fixed rectangle, its reach, the smallest rectangle that holds it under every shift the pack
    /// declares for it (the jitter of each sample of the target's consensus, or of a feature's
    /// consensus that reads it), the area itself when it declares none; none of an area that is
    /// no fixed rectangle.
    pub reach: Option<PackRect>,
    /// Fix round 2 (`contracts/privacy-zones.md`, "Where an unbounded read is confined"): of an
    /// area that is no fixed rectangle, the rectangle the pack confines it to, when it guarantees
    /// one: the region may lie anywhere inside it; none otherwise.
    pub bound: Option<PackRect>,
    /// Whether the region is a reading: its target is one of the package's reading targets
    /// ([`RecognitionPack::reading_targets`]), or (fix round 2) it is a layout's shared read that
    /// gives its text to one.
    pub reading: bool,
    /// Fix round 1: the candidate layout whose read the region is (its shared read, or a feature
    /// it reads at an offset); none for a region its target declares itself, which is what a
    /// case's scene pastes.
    pub layout: Option<String>,
}

/// The one list of a pack's read regions, and what a rectangle of them does not show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackReads {
    /// Every read region, in the pack's order.
    pub regions: Vec<PackReadRegion>,
    /// How many texts the pack's OCR targets expect: the `expected` values of each, added up.
    pub expected_texts: usize,
    /// How many aliases the identity entries of the pack's candidate layouts are read as.
    pub aliases: usize,
}

/// Fix round 1: the members of a package's entry task (`resources/operations/<task_id>/task.json`)
/// that name targets the kernel reads as a number at run time
/// ([`RecognitionPack::reading_targets`]). The default declares none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackReadingDeclarations {
    /// `resource_readings`, the marks.
    pub readings: Vec<ResourceReadingDeclaration>,
    /// `resource_adjustments`.
    pub adjustments: Vec<ResourceAdjustmentDeclaration>,
    /// Fix round 2: the fields of `post_admission_ocr` (`fields_v1`), each of which the kernel
    /// reads after admission, in a field list or not.
    pub fields: Vec<OcrFieldDeclaration>,
}

impl PackReadingDeclarations {
    /// The members of the entry task `task`, its `task.json` as one JSON value: a member it does
    /// not declare is empty. The fields of `post_admission_ocr` are read when it is declared in
    /// its `fields_v1` form (`"mode": "fields_v1"`, task schema `0.8` and `0.9`); its other form,
    /// an operation `0.7`'s comparison with a truth set, reads no number and declares none. A
    /// member that does not decode is the decoder's error.
    pub fn of_task(task: &Value) -> serde_json::Result<Self> {
        let fields = match task.get("post_admission_ocr") {
            Some(declared) if declared.get("mode").and_then(Value::as_str) == Some("fields_v1") => {
                serde_json::from_value::<OcrFieldsDeclaration>(declared.clone())?.fields
            }
            _ => Vec::new(),
        };
        Ok(Self {
            readings: member(task, "resource_readings")?,
            adjustments: member(task, "resource_adjustments")?,
            fields,
        })
    }
}

/// The member `name` of the task `task`, empty when the task does not declare it.
fn member<T: DeserializeOwned + Default>(task: &Value, name: &str) -> serde_json::Result<T> {
    match task.get(name) {
        Some(declared) => serde_json::from_value(declared.clone()),
        None => Ok(T::default()),
    }
}

/// The targets the pack's candidate layouts read as a feature (a `fixed_slots` slot's targets, a
/// feature's own `target`), and among them those a feature reads as an integer (`ocr_integer`).
#[derive(Default)]
struct LayoutFeatures<'p> {
    read: BTreeSet<&'p str>,
    integer: BTreeSet<&'p str>,
}

impl LayoutFeatures<'_> {
    /// The kind of the region of the OCR target `id`.
    fn ocr_kind(&self, id: &str) -> PackReadKind {
        if self.integer.contains(id) {
            PackReadKind::SelectQuantity
        } else if self.read.contains(id) {
            PackReadKind::SelectName
        } else {
            PackReadKind::Ocr
        }
    }
}

impl RecognitionPack {
    /// Fix round 2 (`contracts/privacy-zones.md`, "A reading", the one meaning): the one
    /// collection of the targets the kernel reads as a number at run time, whether or not the
    /// number is published as a fact, with `task` the members of the package's entry task:
    ///
    /// - each mark's `target_id` (`resource_readings`; its capacity key reads the same target),
    ///   parsed by `ResourceReadingDeclaration::parse_parts`;
    /// - each adjustment's `amount.target_id` (`resource_adjustments`; an adjustment without
    ///   `amount` reads its key through a mark, whose target is the mark's), parsed by the
    ///   execution kernel's `judge_resource_adjustment_amount`;
    /// - each target a candidate layout of this pack reads as an integer feature (`ocr_integer`,
    ///   a feature's own `target` or a `fixed_slots` slot's target of the feature), parsed by
    ///   `CandidateIntegerDeclaration::parse`: the integer column a `read_list` step publishes
    ///   into its list, and a select's quantity column, used only to choose;
    /// - the target of each integer field (`unsigned_integer`) of the task's `post_admission_ocr`,
    ///   in a field list or not, parsed by the execution kernel's `parse_ocr_field`;
    /// - the OCR target of each side of each value test of this pack (`left`, and `right` when it
    ///   is another side), which the test parses and compares as numbers.
    ///
    /// A list's key, a string or dictionary field and a name are no reading: the kernel reads
    /// them as text.
    pub fn reading_targets(&self, task: &PackReadingDeclarations) -> BTreeSet<String> {
        let marks = task
            .readings
            .iter()
            .map(|reading| reading.target_id.as_str());
        let amounts = task
            .adjustments
            .iter()
            .filter_map(|adjustment| adjustment.amount.as_ref())
            .map(|amount| amount.target_id.as_str());
        let columns = self.layout_features().integer;
        let fields = task
            .fields
            .iter()
            .filter(|field| matches!(field.value, OcrFieldType::UnsignedInteger { .. }))
            .map(|field| field.target_id.as_str());
        let sides = self
            .targets
            .iter()
            .filter_map(|target| match target {
                RecognitionTarget::ValueTest(test) => Some(test),
                _ => None,
            })
            .flat_map(|test| {
                std::iter::once(&test.left).chain(match &test.right {
                    Some(ValueTestRight::Operand(operand)) => Some(operand),
                    _ => None,
                })
            })
            .map(|operand| operand.target_id.as_str());
        marks
            .chain(amounts)
            .chain(columns)
            .chain(fields)
            .chain(sides)
            .map(str::to_owned)
            .collect()
    }

    /// The one list of the regions this pack's targets read (the module text), each marked a
    /// reading when its target is one of the reading targets of `task`, the members of the
    /// package's entry task ([`Self::reading_targets`]); a caller that needs no mark (the Lab's
    /// scene) passes none.
    ///
    /// Fix round 2: a region that is a fixed rectangle carries its reach; a layout's shared read is
    /// a reading when it gives its text to a reading target, as the kernel counts the calls it
    /// serves; and a feature a `repeated_anchor` layout reads at an offset carries the rectangle
    /// it is confined to, when the pack guarantees one: a bound of the pixels the read can take
    /// its value from, not of the region the pack declares (fix round 3: in this order):
    ///
    /// - the layout's shared read, when it gives the target its text: the row then reads no pixel
    ///   of its own and takes the blocks of the read whose centre lies in its region
    ///   (`layout_shared_text`); a block is held only inside the read's rectangle, so it may reach
    ///   past the row's region and past the band, and validation makes the read cover the band
    ///   (`validate_shared_ocr`);
    /// - else the layout's readable band (the coordinate space when it declares none), when its
    ///   anchor is searched at its own size only (no factor but 1000) and the feature's region at
    ///   the offset, moved by each of its samples, lies inside `instance_rect`: the kernel reads the
    ///   features of an instance only when the instance's rectangle lies inside the band
    ///   (`SceneEvaluation::anchor_instances`), and admission holds every such region inside the
    ///   rectangle (`validate_repeated_anchor`), which this list checks itself;
    /// - else none: a match at another factor moves the region by a scaled offset and keeps its
    ///   size, so nothing holds it inside the band, and the row reads its own pixels.
    pub fn reads(&self, task: &PackReadingDeclarations) -> PackReads {
        let readings = self.reading_targets(task);
        let features = self.layout_features();
        let region = |target: &str,
                      kind: PackReadKind,
                      area: PackReadArea,
                      shifts: &[(i16, i16)],
                      layout: Option<&str>| PackReadRegion {
            target: target.to_owned(),
            kind,
            area,
            reach: match area {
                PackReadArea::Rect(rect) => Some(reached(rect, shifts)),
                PackReadArea::FullFrame | PackReadArea::Unbounded => None,
            },
            bound: None,
            reading: readings.contains(target),
            layout: layout.map(str::to_owned),
        };
        let mut reads = PackReads::default();
        for target in &self.targets {
            match target {
                RecognitionTarget::Template(template) => {
                    reads.regions.push(region(
                        &template.id,
                        PackReadKind::TemplateSearch,
                        area_of(&template.region),
                        &self.own_shifts(&template.id),
                        None,
                    ));
                    // A sample moves a template's search area, never its colour check.
                    if let Some(check) = &template.color_check {
                        reads.regions.push(region(
                            &template.id,
                            PackReadKind::ColorCheck,
                            area_of(&check.region),
                            &[],
                            None,
                        ));
                    }
                }
                RecognitionTarget::Color(color) => reads.regions.push(region(
                    &color.id,
                    PackReadKind::Color,
                    PackReadArea::Rect(color.region),
                    &self.own_shifts(&color.id),
                    None,
                )),
                RecognitionTarget::ColorDigest(digest) => reads.regions.push(region(
                    &digest.id,
                    PackReadKind::ColorDigest,
                    PackReadArea::Rect(digest.region),
                    &self.own_shifts(&digest.id),
                    None,
                )),
                RecognitionTarget::Ocr(ocr) => {
                    reads.regions.push(region(
                        &ocr.id,
                        features.ocr_kind(&ocr.id),
                        area_of(&ocr.region),
                        &self.own_shifts(&ocr.id),
                        None,
                    ));
                    reads.expected_texts += ocr.expected.len();
                }
                // No sample moves an NN target: consensus takes none.
                RecognitionTarget::Nn(nn) => reads.regions.push(region(
                    &nn.id,
                    PackReadKind::Nn,
                    area_of(&nn.region),
                    &[],
                    None,
                )),
                RecognitionTarget::ClickOnly(_)
                | RecognitionTarget::Composite(_)
                | RecognitionTarget::ValueTest(_) => {}
            }
        }
        for layout in &self.candidate_layouts {
            let shared = self.shared_text(layout);
            if let Some(read) = &layout.shared_ocr {
                // The shared read is read once a frame, where it is declared: no sample moves it.
                let mut shared_read = region(
                    &layout.id,
                    PackReadKind::SharedOcr,
                    area_of(&read.region),
                    &[],
                    Some(layout.id.as_str()),
                );
                shared_read.reading = shared.iter().any(|target| readings.contains(target));
                reads.regions.push(shared_read);
            }
            for feature in &layout.features {
                let (Some(target), Some(offset)) = (&feature.target, feature.offset) else {
                    continue;
                };
                let Some((kind, size)) = self.placed_read(target, &features) else {
                    continue;
                };
                match layout.kind {
                    CandidateLayoutKind::FixedSlots => {
                        let shifts = sampled(feature);
                        for slot in &layout.slots {
                            reads.regions.push(region(
                                target,
                                kind,
                                placed(slot.rect, offset, size),
                                &shifts,
                                Some(layout.id.as_str()),
                            ));
                        }
                    }
                    CandidateLayoutKind::RepeatedAnchor => {
                        let mut row = region(
                            target,
                            kind,
                            PackReadArea::Unbounded,
                            &[],
                            Some(layout.id.as_str()),
                        );
                        row.bound = self.confinement(layout, feature, size, &shared);
                        reads.regions.push(row);
                    }
                }
            }
            reads.aliases += layout
                .features
                .iter()
                .filter_map(|feature| feature.identity.as_ref())
                .flat_map(|identity| identity.entries.iter())
                .map(|entry| entry.aliases.len())
                .sum::<usize>();
        }
        reads
    }

    /// The targets the pack's candidate layouts read as a feature: named by a `fixed_slots` slot's
    /// `targets` or by a feature's `target`; and those a feature of value `ocr_integer` reads.
    fn layout_features(&self) -> LayoutFeatures<'_> {
        let mut features = LayoutFeatures::default();
        for layout in &self.candidate_layouts {
            features.read.extend(
                layout
                    .slots
                    .iter()
                    .flat_map(|slot| slot.targets.values())
                    .chain(
                        layout
                            .features
                            .iter()
                            .filter_map(|feature| feature.target.as_ref()),
                    )
                    .map(String::as_str),
            );
            for feature in layout
                .features
                .iter()
                .filter(|feature| feature.value == CandidateFeatureValue::OcrInteger)
            {
                features.integer.extend(
                    feature
                        .target
                        .iter()
                        .chain(
                            layout
                                .slots
                                .iter()
                                .filter_map(|slot| slot.targets.get(&feature.name)),
                        )
                        .map(String::as_str),
                );
            }
        }
        features
    }

    /// Fix round 2: the shifts the pack declares for the region the target `id` declares itself,
    /// each a sample's jitter `(dx, dy)`: of each sample of its target consensus
    /// (`target_consensus`), and of each sample of the consensus of a candidate layout's feature
    /// that reads it in a `fixed_slots` slot (the slot's target of the feature, or a template of
    /// its icon pool), where the kernel reads the target's own region moved by the sample.
    fn own_shifts(&self, id: &str) -> Vec<(i16, i16)> {
        let mut shifts = Vec::new();
        if let Some(consensus) = self.target_consensus.get(id) {
            shifts.extend(consensus.samples.iter().map(jitter));
        }
        for layout in &self.candidate_layouts {
            for feature in &layout.features {
                let Some(consensus) = &feature.consensus else {
                    continue;
                };
                let reads_it = layout.slots.iter().any(|slot| {
                    slot.targets
                        .get(&feature.name)
                        .is_some_and(|target| target.as_str() == id)
                        || slot
                            .identity_templates
                            .get(&feature.name)
                            .is_some_and(|pool| {
                                pool.iter().any(|template| template.target_id == id)
                            })
                });
                if reads_it {
                    shifts.extend(consensus.samples.iter().map(jitter));
                }
            }
        }
        shifts
    }

    /// Fix round 2: the targets the shared read of `layout` gives its text to, as the kernel
    /// counts the calls it serves (`shared_calls`, the read's own chain): an OCR target the
    /// layout's slots or features read with the read's model, languages and chain, inside the
    /// read's region on a `fixed_slots` layout, on any instance of a `repeated_anchor` one. None
    /// without a shared read.
    fn shared_text(&self, layout: &CandidateLayout) -> BTreeSet<String> {
        let lookup = |target_id: &str| self.targets.iter().find(|target| target.id() == target_id);
        shared_calls(
            layout,
            self.defaults.match_metric,
            self.coordinate_space,
            &lookup,
            false,
        )
        .0
        .into_iter()
        .map(|call| call.4)
        .collect()
    }

    /// Fix round 2: the rectangle the read of `feature`, which the `repeated_anchor` layout
    /// `layout` makes at the feature's offset from each anchor match with the size `size`, is
    /// confined to, when the pack guarantees one ([`Self::reads`]; fix round 3: the shared read's
    /// rectangle first, then the band); `shared` the targets the layout's shared read gives its
    /// text to.
    fn confinement(
        &self,
        layout: &CandidateLayout,
        feature: &CandidateFeatureDeclaration,
        size: PackRect,
        shared: &BTreeSet<String>,
    ) -> Option<PackRect> {
        let (Some(target), Some(offset)) = (&feature.target, feature.offset) else {
            return None;
        };
        let whole = self.coordinate_space.map(|space| PackRect {
            x: 0,
            y: 0,
            width: i32::try_from(space.width).unwrap_or(i32::MAX),
            height: i32::try_from(space.height).unwrap_or(i32::MAX),
        });
        // Fix round 3: a row the shared read gives its text to takes the read's blocks, which
        // may reach past its region and past the band, never past the read's rectangle.
        if shared.contains(target) {
            return match layout.shared_ocr.as_ref().map(|read| &read.region) {
                Some(PackRegion::Rect(rect)) => Some(*rect),
                Some(PackRegion::Keyword(keyword)) if keyword == "full_frame" => whole,
                _ => None,
            };
        }
        let unscaled = matches!(
            layout
                .anchor
                .as_deref()
                .and_then(|id| self.targets.iter().find(|candidate| candidate.id() == id)),
            Some(RecognitionTarget::Template(anchor))
                if searched_factors(anchor)
                    .iter()
                    .all(|factor| *factor == SCALE_IDENTITY_MILLI)
        );
        let inside = layout.instance_rect.is_some_and(|instance| {
            sampled(feature).into_iter().all(|(dx, dy)| {
                rect_is_within(
                    PackRect {
                        x: offset.x.saturating_add(i32::from(dx)),
                        y: offset.y.saturating_add(i32::from(dy)),
                        ..size
                    },
                    instance,
                )
            })
        });
        if unscaled && inside {
            layout.readable_band.or(whole)
        } else {
            None
        }
    }

    /// The kind and the size of the region the target `id` is read with at an offset: a template's
    /// search area, an OCR target's region (each a rectangle), a colour or colour digest probe's
    /// region, moved there with its size. `None` for a target the pack does not hold, or that
    /// cannot be read at an offset: admission refuses such a layout (`candidate_layouts`), so the
    /// kernel never reads it. A template read at an offset takes a colour check relative to its
    /// match, which its own region reports as no fixed rectangle.
    fn placed_read(
        &self,
        id: &str,
        features: &LayoutFeatures<'_>,
    ) -> Option<(PackReadKind, PackRect)> {
        match self.targets.iter().find(|target| target.id() == id)? {
            RecognitionTarget::Template(template) => match &template.region {
                PackRegion::Rect(rect) => Some((PackReadKind::TemplateSearch, *rect)),
                _ => None,
            },
            RecognitionTarget::Ocr(ocr) => match &ocr.region {
                PackRegion::Rect(rect) => Some((features.ocr_kind(&ocr.id), *rect)),
                _ => None,
            },
            RecognitionTarget::Color(color) => Some((PackReadKind::Color, color.region)),
            RecognitionTarget::ColorDigest(digest) => {
                Some((PackReadKind::ColorDigest, digest.region))
            }
            RecognitionTarget::Nn(_)
            | RecognitionTarget::ClickOnly(_)
            | RecognitionTarget::Composite(_)
            | RecognitionTarget::ValueTest(_) => None,
        }
    }
}

/// Where `region` lies.
fn area_of(region: &PackRegion) -> PackReadArea {
    match region {
        PackRegion::Rect(rect) => PackReadArea::Rect(*rect),
        PackRegion::Keyword(value) if value == "full_frame" => PackReadArea::FullFrame,
        PackRegion::Keyword(_) | PackRegion::TemplateRelative(_) => PackReadArea::Unbounded,
    }
}

/// Fix round 1: the region of `size` that a `fixed_slots` layout reads at `offset` from the
/// origin of the slot `slot` (its rectangle's top-left corner), as the kernel places a target read
/// at an offset; no fixed rectangle when a coordinate overflows.
fn placed(slot: PackRect, offset: PackPoint, size: PackRect) -> PackReadArea {
    match (slot.x.checked_add(offset.x), slot.y.checked_add(offset.y)) {
        (Some(x), Some(y)) => PackReadArea::Rect(PackRect {
            x,
            y,
            width: size.width,
            height: size.height,
        }),
        _ => PackReadArea::Unbounded,
    }
}

/// A sample's jitter, `(dx, dy)`.
fn jitter(sample: &CandidateSampleVariant) -> (i16, i16) {
    (sample.dx, sample.dy)
}

/// Fix round 2: the jitter of each sample `feature` reads with, as the kernel samples it
/// (`CandidateLayout::feature_samples`): its consensus's samples, or the one unshifted sample of a
/// feature without consensus.
fn sampled(feature: &CandidateFeatureDeclaration) -> Vec<(i16, i16)> {
    match &feature.consensus {
        Some(consensus) => consensus.samples.iter().map(jitter).collect(),
        None => vec![(0, 0)],
    }
}

/// Fix round 2: the reach of `area` under `shifts`: the smallest rectangle that holds it and each
/// of its copies moved by a shift; `area` itself when there is none.
fn reached(area: PackRect, shifts: &[(i16, i16)]) -> PackRect {
    let (mut left, mut top, mut right, mut bottom) = (0_i32, 0_i32, 0_i32, 0_i32);
    for &(dx, dy) in shifts {
        left = left.min(i32::from(dx));
        right = right.max(i32::from(dx));
        top = top.min(i32::from(dy));
        bottom = bottom.max(i32::from(dy));
    }
    PackRect {
        x: area.x.saturating_add(left),
        y: area.y.saturating_add(top),
        width: area.width.saturating_add(right - left),
        height: area.height.saturating_add(bottom - top),
    }
}
