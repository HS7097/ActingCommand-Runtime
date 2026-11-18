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
//!   OCR read (`shared_ocr.region`), then (fix round 1) each feature that reads its target at an
//!   `offset`, where the kernel reads it: in a `fixed_slots` layout once per slot, the target's own
//!   size at the slot's origin plus the offset; in a `repeated_anchor` layout at the anchor's match
//!   plus the offset, scaled as the match is, which only a frame shows, so no fixed rectangle. An
//!   OCR target a candidate layout reads as a feature is a select's quantity column when a feature
//!   reads it as an integer (`ocr_integer`), else a select's name region. A check (`all_of`,
//!   `any_of`), a value test and a click-only target read nothing of their own.
//! - **What is not in it**: a press box (a target's `click`, a click-only target, a slot's
//!   `click`, a layout's `click`): pressing reads nothing.
//! - **Each region** carries the target it belongs to (a shared read: its layout's id), its kind,
//!   where it lies (a fixed rectangle, the keyword `full_frame`, or no fixed rectangle at all), the
//!   layout whose read it is (none for a region its target declares itself, the only regions a
//!   case's scene pastes) and whether it is a reading.
//! - **A reading** (fix round 1) is a region whose recognised value the package publishes as, or
//!   into, a fact of the account: its target is one [`RecognitionPack::reading_targets`] collects,
//!   from every place the kernel honours.
//! - **What a rectangle does not show** is counted beside the list: the texts an OCR target
//!   expects and the aliases a layout's identity entries are read as.

use crate::{
    CandidateFeatureValue, PackPoint, PackRect, PackRegion, RecognitionPack, RecognitionTarget,
};
use actingcommand_contract::candidate_projection::CandidateLayoutKind;
use actingcommand_contract::{
    FieldListDeclaration, OcrFieldDeclaration, OcrFieldType, OcrFieldsDeclaration,
    ResourceAdjustmentDeclaration, ResourceReadingDeclaration,
};
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
    /// A candidate layout's shared OCR read, `shared_ocr.region`; its target is the layout's id.
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
    /// anchor's match), or a keyword other than `full_frame`.
    Unbounded,
}

/// One region a recognition target of a pack reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackReadRegion {
    /// The target's id; a layout's shared read names its layout's id.
    pub target: String,
    pub kind: PackReadKind,
    pub area: PackReadArea,
    /// Whether the region is a reading: its target is one of the package's reading targets
    /// ([`RecognitionPack::reading_targets`]).
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
/// that name the targets whose recognised value the package publishes as, or into, a fact of the
/// account ([`RecognitionPack::reading_targets`]). The default declares none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackReadingDeclarations {
    /// `resource_readings`, the marks.
    pub readings: Vec<ResourceReadingDeclaration>,
    /// `resource_adjustments`.
    pub adjustments: Vec<ResourceAdjustmentDeclaration>,
    /// `field_lists`.
    pub field_lists: Vec<FieldListDeclaration>,
    /// The fields of `post_admission_ocr`, which a field list's columns name.
    pub fields: Vec<OcrFieldDeclaration>,
}

impl PackReadingDeclarations {
    /// The members of the entry task `task`, its `task.json` as one JSON value: a member it does
    /// not declare is empty. The fields of `post_admission_ocr` are read only when the task
    /// declares a field list, which needs their `fields_v1` form (an operation `0.7` declares
    /// another form). A member that does not decode is the decoder's error.
    pub fn of_task(task: &Value) -> serde_json::Result<Self> {
        let field_lists: Vec<FieldListDeclaration> = member(task, "field_lists")?;
        let fields = match task.get("post_admission_ocr") {
            Some(declared) if !field_lists.is_empty() => {
                serde_json::from_value::<OcrFieldsDeclaration>(declared.clone())?.fields
            }
            _ => Vec::new(),
        };
        Ok(Self {
            readings: member(task, "resource_readings")?,
            adjustments: member(task, "resource_adjustments")?,
            field_lists,
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
    /// Fix round 1 (`contracts/privacy-zones.md`, "A reading"): the one collection of the targets
    /// whose recognised value the package publishes as, or into, a fact of the account, from every
    /// place the kernel honours, with `task` the members of its entry task:
    ///
    /// - each resource reading's `target_id` (a mark; its capacity key reads the same target);
    /// - each adjustment's `amount.target_id` (an adjustment without `amount` reads its key through
    ///   a mark, whose target is the mark's);
    /// - each target a candidate layout of this pack reads as an integer feature (`ocr_integer`, a
    ///   feature's own `target` or a `fixed_slots` slot's target of the feature): the integer
    ///   column a `read_list` step publishes into its list, and a select's quantity column;
    /// - the target of each integer field (`unsigned_integer`) of its group a field list's column
    ///   names.
    ///
    /// A list's key, its string and its boolean columns name or test a position, and are no
    /// reading.
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
        let fields = task.field_lists.iter().flat_map(|list| {
            list.columns.iter().filter_map(move |column| {
                task.fields
                    .iter()
                    .find(|field| {
                        field.id == column.field
                            && field.group == list.group
                            && matches!(field.value, OcrFieldType::UnsignedInteger { .. })
                    })
                    .map(|field| field.target_id.as_str())
            })
        });
        marks
            .chain(amounts)
            .chain(columns)
            .chain(fields)
            .map(str::to_owned)
            .collect()
    }

    /// The one list of the regions this pack's targets read (the module text), each marked a
    /// reading when its target is one of the reading targets of `task`, the members of the
    /// package's entry task ([`Self::reading_targets`]); a caller that needs no mark (the Lab's
    /// scene) passes none.
    pub fn reads(&self, task: &PackReadingDeclarations) -> PackReads {
        let read = self.reading_targets(task);
        let features = self.layout_features();
        let region =
            |target: &str, kind: PackReadKind, area: PackReadArea, layout: Option<&str>| {
                PackReadRegion {
                    target: target.to_owned(),
                    kind,
                    area,
                    reading: read.contains(target),
                    layout: layout.map(str::to_owned),
                }
            };
        let mut reads = PackReads::default();
        for target in &self.targets {
            match target {
                RecognitionTarget::Template(template) => {
                    reads.regions.push(region(
                        &template.id,
                        PackReadKind::TemplateSearch,
                        area_of(&template.region),
                        None,
                    ));
                    if let Some(check) = &template.color_check {
                        reads.regions.push(region(
                            &template.id,
                            PackReadKind::ColorCheck,
                            area_of(&check.region),
                            None,
                        ));
                    }
                }
                RecognitionTarget::Color(color) => reads.regions.push(region(
                    &color.id,
                    PackReadKind::Color,
                    PackReadArea::Rect(color.region),
                    None,
                )),
                RecognitionTarget::ColorDigest(digest) => reads.regions.push(region(
                    &digest.id,
                    PackReadKind::ColorDigest,
                    PackReadArea::Rect(digest.region),
                    None,
                )),
                RecognitionTarget::Ocr(ocr) => {
                    reads.regions.push(region(
                        &ocr.id,
                        features.ocr_kind(&ocr.id),
                        area_of(&ocr.region),
                        None,
                    ));
                    reads.expected_texts += ocr.expected.len();
                }
                RecognitionTarget::Nn(nn) => {
                    reads
                        .regions
                        .push(region(&nn.id, PackReadKind::Nn, area_of(&nn.region), None))
                }
                RecognitionTarget::ClickOnly(_)
                | RecognitionTarget::Composite(_)
                | RecognitionTarget::ValueTest(_) => {}
            }
        }
        for layout in &self.candidate_layouts {
            if let Some(shared) = &layout.shared_ocr {
                reads.regions.push(region(
                    &layout.id,
                    PackReadKind::SharedOcr,
                    area_of(&shared.region),
                    Some(layout.id.as_str()),
                ));
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
                        for slot in &layout.slots {
                            reads.regions.push(region(
                                target,
                                kind,
                                placed(slot.rect, offset, size),
                                Some(layout.id.as_str()),
                            ));
                        }
                    }
                    CandidateLayoutKind::RepeatedAnchor => reads.regions.push(region(
                        target,
                        kind,
                        PackReadArea::Unbounded,
                        Some(layout.id.as_str()),
                    )),
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
