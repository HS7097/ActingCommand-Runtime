// SPDX-License-Identifier: AGPL-3.0-only

//! #463 PRIV-1 (`contracts/privacy-zones.md`, "The read regions of a package"): the one list of
//! the regions a recognition pack's targets read. The Lab's scene of a regression case pastes them
//! (`record stop`, `contracts/resource-regression-case.md`) and the privacy comparison of resource
//! tooling compares them with a repository's sensitive zones; neither makes a list of its own.
//!
//! - **What is in it**, in the pack's order: each target in `targets` with what it reads of its
//!   own (a template's search area, `region`, where `record mark` writes the crop with its margin,
//!   then its colour check's region; a colour or colour digest probe's region; an OCR or NN
//!   target's region), then each candidate layout's shared OCR read (`shared_ocr.region`). An OCR
//!   target that a candidate layout reads as a feature (a `fixed_slots` slot's `targets`, a
//!   feature's `target`) is a select's name region. A check (`all_of`, `any_of`), a value test and
//!   a click-only target read nothing of their own.
//! - **What is not in it**: a press box (a target's `click`, a click-only target, a slot's
//!   `click`, a layout's `click`): pressing reads nothing.
//! - **Each region** carries the target it belongs to (a shared read: its layout's id), its kind,
//!   where it lies (a fixed rectangle, the keyword `full_frame`, or no fixed rectangle at all) and
//!   whether a resource reading of the package names its target (`contracts/resource-readings.md`:
//!   a reading's `target_id`, an OCR target of the task).
//! - **What a rectangle does not show** is counted beside the list: the texts an OCR target
//!   expects and the aliases a layout's identity entries are read as.

use crate::{PackRect, PackRegion, RecognitionPack, RecognitionTarget};
use actingcommand_contract::ResourceReadingDeclaration;
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
    /// An OCR target a candidate layout reads as a feature: a select's name region.
    SelectName,
}

/// Where a read region lies, as the pack declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackReadArea {
    /// A fixed rectangle of the pack's coordinate space.
    Rect(PackRect),
    /// The keyword region `full_frame`: the whole frame, whatever its size.
    FullFrame,
    /// No fixed rectangle: a region placed relative to where a template was found
    /// (`template_relative`), or a keyword other than `full_frame`.
    Unbounded,
}

/// One region a recognition target of a pack reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackReadRegion {
    /// The target's id; a layout's shared read names its layout's id.
    pub target: String,
    pub kind: PackReadKind,
    pub area: PackReadArea,
    /// Whether a resource reading of the package names the target (its `target_id`).
    pub reading: bool,
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

impl RecognitionPack {
    /// The one list of the regions this pack's targets read (the module text), each marked a
    /// reading when one of `readings`, the resource readings of the package, names its target.
    pub fn reads(&self, readings: &[ResourceReadingDeclaration]) -> PackReads {
        let read = readings
            .iter()
            .map(|reading| reading.target_id.as_str())
            .collect::<BTreeSet<_>>();
        let names = self.layout_feature_targets();
        let region = |target: &str, kind: PackReadKind, area: PackReadArea| PackReadRegion {
            target: target.to_owned(),
            kind,
            area,
            reading: read.contains(target),
        };
        let mut reads = PackReads::default();
        for target in &self.targets {
            match target {
                RecognitionTarget::Template(template) => {
                    reads.regions.push(region(
                        &template.id,
                        PackReadKind::TemplateSearch,
                        area_of(&template.region),
                    ));
                    if let Some(check) = &template.color_check {
                        reads.regions.push(region(
                            &template.id,
                            PackReadKind::ColorCheck,
                            area_of(&check.region),
                        ));
                    }
                }
                RecognitionTarget::Color(color) => reads.regions.push(region(
                    &color.id,
                    PackReadKind::Color,
                    PackReadArea::Rect(color.region),
                )),
                RecognitionTarget::ColorDigest(digest) => reads.regions.push(region(
                    &digest.id,
                    PackReadKind::ColorDigest,
                    PackReadArea::Rect(digest.region),
                )),
                RecognitionTarget::Ocr(ocr) => {
                    let kind = if names.contains(ocr.id.as_str()) {
                        PackReadKind::SelectName
                    } else {
                        PackReadKind::Ocr
                    };
                    reads
                        .regions
                        .push(region(&ocr.id, kind, area_of(&ocr.region)));
                    reads.expected_texts += ocr.expected.len();
                }
                RecognitionTarget::Nn(nn) => {
                    reads
                        .regions
                        .push(region(&nn.id, PackReadKind::Nn, area_of(&nn.region)));
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
                ));
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

    /// The targets a candidate layout reads as a feature: named by a `fixed_slots` slot's
    /// `targets` or by a feature's `target`.
    fn layout_feature_targets(&self) -> BTreeSet<&str> {
        self.candidate_layouts
            .iter()
            .flat_map(|layout| {
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
            })
            .map(String::as_str)
            .collect()
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
