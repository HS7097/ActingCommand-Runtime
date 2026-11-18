// SPDX-License-Identifier: AGPL-3.0-only

//! #463 PRIV-1 (`contracts/privacy-zones.md`): a repository root's sensitive-zone declaration
//! `privacy-zones.json`, and the comparison of a package's read regions with it.
//!
//! - **The declaration**: its member `repository` is the root's own statement, `public` or
//!   `local`, and for a public root, by base size, the places of its screens that hold private
//!   information (each a zone of a kind: `identity` or `value`, the owner's ruling of 11-18) and
//!   the transition list of packages that were there before it. [`PrivacyDeclaration::read`]
//!   answers its three states, public with its zones, local, or absent when the root holds no such
//!   file, and decides nothing from them. Whether a root is public is established by the slices
//!   that call the comparison (the adoption, the repository's check): a root is public when its
//!   declaration says so, and the owner's criterion, a repository that goes to GitHub, is applied
//!   there. A root without the file refuses nothing by this reader and is no error. A declaration
//!   that cannot be read as written is `resource_declaration_invalid` with its JSON pointer, as
//!   `declaration-exclusions.json` is.
//! - **The comparison** ([`compare_privacy_zones`]) takes the one list of a package's read regions
//!   (`RecognitionPack::reads`), the rectangles of its regression cases' scenes when the caller has
//!   them, the package's base size and a public declaration, and answers what it found as facts:
//!   each region or rectangle that meets a zone (their intersection's area is above zero), a
//!   whole-frame one, a region that is no fixed rectangle, a base size the declaration does not
//!   list; and counts what a rectangle cannot show. A finding names a case by its id, never by
//!   anything the case writes. It names no pixel and no expected text, blacks nothing out,
//!   proposes no other place and decides nothing: which finding refuses is its caller's (later
//!   slices), and so is the transition list.

use crate::codes::{PrivacyRectKind, PrivacyUnseenKind, PrivacyZoneKind};
use crate::standard_pack_trial::content_prefix;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    LabError, LabErrorClass, LabResult, ResourceDeclarationIssue, ResourceDeclarationReason,
    package_id_names_one_path_segment, reserved_package_id,
};
use actingcommand_recognition_pack::{
    PackCoordinateSpace, PackReadArea, PackReadKind, PackReads, PackRect,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// The declaration's path, relative to the repository root (the root only).
pub const PRIVACY_ZONES_FILE: &str = "privacy-zones.json";
/// The declaration's `schema_version`.
pub const PRIVACY_ZONES_SCHEMA: &str = "actingcommand.privacy-zones.v1";
/// The largest declaration read, in bytes.
const MAX_PRIVACY_ZONES_BYTES: u64 = 1024 * 1024;
/// At most this many base sizes.
const MAX_BASES: usize = 16;
/// At most this many zones per base size.
const MAX_ZONES: usize = 256;
/// At most this many entries of the transition list.
const MAX_EXEMPT: usize = 1024;
/// At most this many bytes of a zone's id.
const MAX_ZONE_ID_BYTES: usize = 128;
/// At most this many bytes of a package id in the transition list.
const MAX_PACKAGE_ID_BYTES: usize = 256;
/// The members of the declaration, of a base, of a zone, of a rectangle and of an exemption.
const TOP_MEMBERS: [&str; 4] = ["schema_version", "repository", "bases", "exempt"];
const BASE_MEMBERS: [&str; 3] = ["width", "height", "zones"];
const ZONE_MEMBERS: [&str; 3] = ["id", "kind", "rect"];
const RECT_MEMBERS: [&str; 4] = ["x", "y", "width", "height"];
const EXEMPT_MEMBERS: [&str; 2] = ["package_id", "content"];

/// What a repository root declares of its privacy, as its file says it; this reader decides
/// nothing from it (the module text: whether a root is public is its callers').
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyDeclaration {
    /// `"repository": "public"`: the root declares itself public; its zones by base size and its
    /// transition list.
    Public(PublicPrivacyZones),
    /// `"repository": "local"`: the root declares itself local.
    Local,
    /// The root holds no `privacy-zones.json`: it declares nothing, and refuses nothing by this
    /// reader; it is no error.
    Absent,
}

/// The zones of a public root, by base size, and its transition list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicPrivacyZones {
    /// The base sizes, each once, in the declaration's order.
    pub bases: Vec<PrivacyBase>,
    /// The packages the transition list names, in its order; absent means none.
    pub exempt: Vec<PrivacyExemption>,
}

/// One base size of a public root and its zones, which hold on every page of the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyBase {
    pub width: u32,
    pub height: u32,
    /// The zones in the declaration's order; an empty list says this size has none.
    pub zones: Vec<PrivacyZone>,
}

/// A sensitive zone: a rectangle of its base, inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyZone {
    /// Unique within its base.
    pub id: String,
    /// `identity` when the declaration gives none.
    pub kind: PrivacyZoneKind,
    pub rect: PackRect,
}

/// An entry of the transition list: a package that was in the root before its declaration, and
/// the content it had then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyExemption {
    pub package_id: String,
    /// The first [`crate::CONTENT_PREFIX_DIGITS`] hex digits of the content hash, as adoption and
    /// a trial name a content.
    pub content: String,
}

impl PrivacyDeclaration {
    /// The declaration of the repository root `root`: [`Self::Absent`] when it holds no
    /// `privacy-zones.json`, else the file as [`Self::parse`] reads it, read up to its bound. A
    /// file that cannot be read is a usage error naming it.
    pub fn read(root: &Path) -> LabResult<Self> {
        let path = root.join(PRIVACY_ZONES_FILE);
        let unreadable = |error: io::Error| LabError::usage(format!("{}: {error}", path.display()));
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::Absent),
            Err(error) => return Err(unreadable(error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_PRIVACY_ZONES_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        if bytes.len() as u64 > MAX_PRIVACY_ZONES_BYTES {
            return Err(invalid("", None, ResourceDeclarationReason::InvalidValue));
        }
        Self::parse(&bytes)
    }

    /// Reads `privacy-zones.json` (`contracts/privacy-zones.md`, "The declaration"): schema
    /// [`PRIVACY_ZONES_SCHEMA`]; `repository` `public` or `local`; a local one holds nothing else;
    /// a public one holds `bases`, 1 to 16 distinct base sizes, each with `zones`, 0 to 256 zones
    /// of distinct ids, each of kind `identity` (the default) or `value` and a rectangle inside its
    /// base, and optionally `exempt`, at most 1024 distinct packages each with a content prefix.
    /// No other member is accepted. Any other document is `resource_declaration_invalid` with the
    /// member's JSON pointer. It never answers [`Self::Absent`].
    pub fn parse(bytes: &[u8]) -> LabResult<Self> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| {
            let mut refusal = invalid("", None, ResourceDeclarationReason::InvalidType);
            refusal.message = format!("{PRIVACY_ZONES_FILE}: JSON parse: {error}");
            refusal
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("", None, ResourceDeclarationReason::InvalidType))?;
        let reader = Reader {
            schema: object.get("schema_version").and_then(Value::as_str),
        };
        reader.members(object, "", &TOP_MEMBERS)?;
        match object.get("schema_version") {
            None => {
                return Err(
                    reader.refuse("/schema_version", ResourceDeclarationReason::MissingField)
                );
            }
            Some(Value::String(version)) if version == PRIVACY_ZONES_SCHEMA => {}
            Some(Value::String(_)) => {
                return Err(
                    reader.refuse("/schema_version", ResourceDeclarationReason::InvalidValue)
                );
            }
            Some(_) => {
                return Err(
                    reader.refuse("/schema_version", ResourceDeclarationReason::InvalidType)
                );
            }
        }
        match reader.text(object, "", "repository")? {
            "local" => {
                if let Some(member) = ["bases", "exempt"]
                    .into_iter()
                    .find(|member| object.contains_key(*member))
                {
                    return Err(reader.refuse(
                        &format!("/{member}"),
                        ResourceDeclarationReason::UnconsumedField,
                    ));
                }
                Ok(Self::Local)
            }
            "public" => Ok(Self::Public(PublicPrivacyZones {
                bases: reader.bases(object)?,
                exempt: reader.exempt(object)?,
            })),
            _ => Err(reader.refuse("/repository", ResourceDeclarationReason::InvalidValue)),
        }
    }
}

/// The reader of one declaration: its refusals name the schema it declares.
struct Reader<'a> {
    schema: Option<&'a str>,
}

impl Reader<'_> {
    fn refuse(&self, pointer: &str, reason: ResourceDeclarationReason) -> LabError {
        invalid(pointer, self.schema, reason)
    }

    /// The first member of `object` (at `pointer`) that is not in `known` is `UnknownField`.
    fn members(&self, object: &Map<String, Value>, pointer: &str, known: &[&str]) -> LabResult<()> {
        match object
            .keys()
            .find(|member| !known.contains(&member.as_str()))
        {
            Some(member) => Err(self.refuse(
                &format!("{pointer}/{}", member.replace('~', "~0").replace('/', "~1")),
                ResourceDeclarationReason::UnknownField,
            )),
            None => Ok(()),
        }
    }

    fn required<'v>(
        &self,
        object: &'v Map<String, Value>,
        pointer: &str,
        member: &str,
    ) -> LabResult<&'v Value> {
        object.get(member).ok_or_else(|| {
            self.refuse(
                &format!("{pointer}/{member}"),
                ResourceDeclarationReason::MissingField,
            )
        })
    }

    fn text<'v>(
        &self,
        object: &'v Map<String, Value>,
        pointer: &str,
        member: &str,
    ) -> LabResult<&'v str> {
        self.required(object, pointer, member)?
            .as_str()
            .ok_or_else(|| {
                self.refuse(
                    &format!("{pointer}/{member}"),
                    ResourceDeclarationReason::InvalidType,
                )
            })
    }

    fn array<'v>(&self, value: &'v Value, pointer: &str) -> LabResult<&'v [Value]> {
        value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| self.refuse(pointer, ResourceDeclarationReason::InvalidType))
    }

    fn object<'v>(&self, value: &'v Value, pointer: &str) -> LabResult<&'v Map<String, Value>> {
        value
            .as_object()
            .ok_or_else(|| self.refuse(pointer, ResourceDeclarationReason::InvalidType))
    }

    /// The integer `member` of `object`, at least `least` and at most `i32::MAX`.
    fn integer(
        &self,
        object: &Map<String, Value>,
        pointer: &str,
        member: &str,
        least: i32,
    ) -> LabResult<i32> {
        let place = format!("{pointer}/{member}");
        let value = self.required(object, pointer, member)?;
        if !value.is_number() {
            return Err(self.refuse(&place, ResourceDeclarationReason::InvalidType));
        }
        value
            .as_i64()
            .and_then(|number| i32::try_from(number).ok())
            .filter(|number| *number >= least)
            .ok_or_else(|| self.refuse(&place, ResourceDeclarationReason::InvalidValue))
    }

    /// `bases`: 1 to [`MAX_BASES`] base sizes, none listed twice.
    fn bases(&self, object: &Map<String, Value>) -> LabResult<Vec<PrivacyBase>> {
        let entries = self.array(self.required(object, "", "bases")?, "/bases")?;
        if entries.is_empty() || entries.len() > MAX_BASES {
            return Err(self.refuse("/bases", ResourceDeclarationReason::InvalidValue));
        }
        let mut seen = BTreeSet::new();
        let mut bases = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let pointer = format!("/bases/{index}");
            let base = self.object(entry, &pointer)?;
            self.members(base, &pointer, &BASE_MEMBERS)?;
            let width = self.integer(base, &pointer, "width", 1)?.unsigned_abs();
            let height = self.integer(base, &pointer, "height", 1)?.unsigned_abs();
            if !seen.insert((width, height)) {
                return Err(self.refuse(&pointer, ResourceDeclarationReason::InvalidValue));
            }
            let zones = self.zones(base, &pointer, width, height)?;
            bases.push(PrivacyBase {
                width,
                height,
                zones,
            });
        }
        Ok(bases)
    }

    /// The `zones` of the base at `pointer`, of `width` by `height`: at most [`MAX_ZONES`], ids
    /// distinct, each inside the base.
    fn zones(
        &self,
        base: &Map<String, Value>,
        pointer: &str,
        width: u32,
        height: u32,
    ) -> LabResult<Vec<PrivacyZone>> {
        let place = format!("{pointer}/zones");
        let entries = self.array(self.required(base, pointer, "zones")?, &place)?;
        if entries.len() > MAX_ZONES {
            return Err(self.refuse(&place, ResourceDeclarationReason::InvalidValue));
        }
        let mut ids = BTreeSet::new();
        let mut zones = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let pointer = format!("{place}/{index}");
            let zone = self.object(entry, &pointer)?;
            self.members(zone, &pointer, &ZONE_MEMBERS)?;
            let id = self.text(zone, &pointer, "id")?;
            if !zone_id(id) || !ids.insert(id) {
                return Err(self.refuse(
                    &format!("{pointer}/id"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            let kind = match zone.get("kind") {
                None => PrivacyZoneKind::Identity,
                Some(Value::String(text)) => PrivacyZoneKind::ALL
                    .iter()
                    .copied()
                    .find(|kind| kind.as_str() == text.as_str())
                    .ok_or_else(|| {
                        self.refuse(
                            &format!("{pointer}/kind"),
                            ResourceDeclarationReason::InvalidValue,
                        )
                    })?,
                Some(_) => {
                    return Err(self.refuse(
                        &format!("{pointer}/kind"),
                        ResourceDeclarationReason::InvalidType,
                    ));
                }
            };
            let rect = self.rect(zone, &pointer, width, height)?;
            zones.push(PrivacyZone {
                id: id.to_owned(),
                kind,
                rect,
            });
        }
        Ok(zones)
    }

    /// The `rect` of the zone at `pointer`: `x` and `y` from 0, `width` and `height` from 1, inside
    /// the base of `width` by `height` (else `InvalidValue` at the rectangle).
    fn rect(
        &self,
        zone: &Map<String, Value>,
        pointer: &str,
        width: u32,
        height: u32,
    ) -> LabResult<PackRect> {
        let place = format!("{pointer}/rect");
        let object = self.object(self.required(zone, pointer, "rect")?, &place)?;
        self.members(object, &place, &RECT_MEMBERS)?;
        let rect = PackRect {
            x: self.integer(object, &place, "x", 0)?,
            y: self.integer(object, &place, "y", 0)?,
            width: self.integer(object, &place, "width", 1)?,
            height: self.integer(object, &place, "height", 1)?,
        };
        let right = i64::from(rect.x) + i64::from(rect.width);
        let bottom = i64::from(rect.y) + i64::from(rect.height);
        if right > i64::from(width) || bottom > i64::from(height) {
            return Err(self.refuse(&place, ResourceDeclarationReason::InvalidValue));
        }
        Ok(rect)
    }

    /// `exempt`, when present: at most [`MAX_EXEMPT`] entries, each a package id (one path segment,
    /// no reserved name, each once) and a content prefix.
    fn exempt(&self, object: &Map<String, Value>) -> LabResult<Vec<PrivacyExemption>> {
        let Some(value) = object.get("exempt") else {
            return Ok(Vec::new());
        };
        let entries = self.array(value, "/exempt")?;
        if entries.len() > MAX_EXEMPT {
            return Err(self.refuse("/exempt", ResourceDeclarationReason::InvalidValue));
        }
        let mut seen = BTreeSet::new();
        let mut exempt = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let pointer = format!("/exempt/{index}");
            let exemption = self.object(entry, &pointer)?;
            self.members(exemption, &pointer, &EXEMPT_MEMBERS)?;
            let package_id = self.text(exemption, &pointer, "package_id")?;
            if package_id.is_empty()
                || package_id.len() > MAX_PACKAGE_ID_BYTES
                || !package_id_names_one_path_segment(package_id)
                || reserved_package_id(package_id)
                || !seen.insert(package_id)
            {
                return Err(self.refuse(
                    &format!("{pointer}/package_id"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            let content = self.text(exemption, &pointer, "content")?;
            if !content_prefix(content) {
                return Err(self.refuse(
                    &format!("{pointer}/content"),
                    ResourceDeclarationReason::InvalidValue,
                ));
            }
            exempt.push(PrivacyExemption {
                package_id: package_id.to_owned(),
                content: content.to_owned(),
            });
        }
        Ok(exempt)
    }
}

/// Whether `id` is a zone id: 1 to [`MAX_ZONE_ID_BYTES`] bytes of ASCII letters, digits and
/// `_-./:`, as a reading's id is written.
fn zone_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ZONE_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-./:".contains(&byte))
}

fn invalid(pointer: &str, schema: Option<&str>, reason: ResourceDeclarationReason) -> LabError {
    let issue = ResourceDeclarationIssue {
        declaration_file: PRIVACY_ZONES_FILE.to_owned(),
        field_path: pointer.to_owned(),
        schema_version: schema.map(str::to_owned),
        reason,
    };
    LabError::new(
        LabErrorClass::UsageValidation,
        SharedCode::ResourceDeclarationInvalid.as_str(),
        format!("{PRIVACY_ZONES_FILE}: field {pointer:?} is not consumable: {reason:?}"),
        &[],
    )
    .with_details(serde_json::json!(issue))
}

/// A regression case of the package, as its caller read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyCase {
    /// A case of `actingcommand.resource-regression-case.v1`: its id and its scene's rectangles
    /// (`scene_construction.rects`, each its `rect`), in their order, in the package's base
    /// pixels.
    V1 { case: String, rects: Vec<PackRect> },
    /// A case of another schema, or of none: nothing of its scene is compared, and it is counted.
    OtherSchema { case: String },
}

/// Where a finding is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyPlace {
    /// A region of the one list of the package's read regions: its target (a shared read's
    /// layout), its kind, whether it is a reading (`PackReadRegion::reading`) and (fix round 1)
    /// the candidate layout whose read it is, none for a region its target declares itself.
    Region {
        target: String,
        rect_kind: PrivacyRectKind,
        reading: bool,
        layout: Option<String>,
    },
    /// A rectangle of a regression case's scene: the case's id and the rectangle's index in its
    /// `scene_construction.rects`, nothing the case writes besides. A scene holds the pixels it
    /// pastes, so its rectangle is never a reading.
    Scene { case: String, index: usize },
}

impl PrivacyPlace {
    /// The word of the place: the region's kind, or `scene_rect`.
    pub fn rect_kind(&self) -> PrivacyRectKind {
        match self {
            Self::Region { rect_kind, .. } => *rect_kind,
            Self::Scene { .. } => PrivacyRectKind::SceneRect,
        }
    }

    /// Whether the place is a region that is a reading.
    pub fn reading(&self) -> bool {
        matches!(self, Self::Region { reading: true, .. })
    }
}

/// One fact the comparison found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyFinding {
    /// A region or a scene's rectangle, `rect`, meets the zone `zone_id` of kind `zone_kind` at
    /// `zone_rect`: their intersection's area is above zero (rectangles that only touch at an edge
    /// do not meet). One finding per zone met.
    ZoneMet {
        place: PrivacyPlace,
        rect: PackRect,
        zone_id: String,
        zone_kind: PrivacyZoneKind,
        zone_rect: PackRect,
    },
    /// A region or a scene's rectangle covers the whole base (a region `full_frame` too): it meets
    /// every zone there is, and is named once, as this, not once per zone.
    WholeFrame { place: PrivacyPlace },
    /// A region that is no fixed rectangle (placed relative to a match, a feature a
    /// `repeated_anchor` layout reads at an offset from its anchor's match, or a keyword other
    /// than `full_frame`): where it reads is not known before a run.
    RegionUnbounded { place: PrivacyPlace },
    /// The package's base size is no base of the declaration: no zone is compared.
    BaseUndeclared { width: u32, height: u32 },
}

/// What a rectangle check cannot see, counted by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrivacyUnseen {
    /// The texts the package's OCR targets expect.
    pub expected_texts: usize,
    /// The aliases the identity entries of its candidate layouts are read as.
    pub aliases: usize,
    /// The cases of another schema, or of none, whose scenes were not compared.
    pub cases_other_schema: usize,
}

impl PrivacyUnseen {
    /// Each count with its word, in the vocabulary's order.
    pub fn by_kind(&self) -> [(PrivacyUnseenKind, usize); 3] {
        [
            (PrivacyUnseenKind::ExpectedText, self.expected_texts),
            (PrivacyUnseenKind::Alias, self.aliases),
            (PrivacyUnseenKind::CaseOtherSchema, self.cases_other_schema),
        ]
    }
}

/// What the comparison of one package found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyComparison {
    pub package_id: String,
    /// In this order: the base size undeclared; then each region of the list in its order; then
    /// each case's rectangles, case by case.
    pub findings: Vec<PrivacyFinding>,
    pub unseen: PrivacyUnseen,
}

/// #463 PRIV-1: compares the read regions `reads` of the package `package_id` and the scenes of
/// its regression cases `cases` (empty when the caller has none) with the zones of the public
/// declaration `zones` for the package's base size `base` (the module text). Every region and
/// every rectangle is compared; nothing is left out for a finding before it.
pub fn compare_privacy_zones(
    package_id: &str,
    reads: &PackReads,
    cases: &[PrivacyCase],
    base: PackCoordinateSpace,
    zones: &PublicPrivacyZones,
) -> PrivacyComparison {
    let declared = zones
        .bases
        .iter()
        .find(|declared| (declared.width, declared.height) == (base.width, base.height));
    let mut findings = Vec::new();
    if declared.is_none() {
        findings.push(PrivacyFinding::BaseUndeclared {
            width: base.width,
            height: base.height,
        });
    }
    let base_zones: &[PrivacyZone] = declared
        .map(|found| found.zones.as_slice())
        .unwrap_or_default();
    for region in &reads.regions {
        let place = PrivacyPlace::Region {
            target: region.target.clone(),
            rect_kind: rect_kind(region.kind),
            reading: region.reading,
            layout: region.layout.clone(),
        };
        match region.area {
            PackReadArea::Rect(rect) => compare(place, rect, base, base_zones, &mut findings),
            PackReadArea::FullFrame => findings.push(PrivacyFinding::WholeFrame { place }),
            PackReadArea::Unbounded => findings.push(PrivacyFinding::RegionUnbounded { place }),
        }
    }
    let mut cases_other_schema = 0;
    for entry in cases {
        match entry {
            PrivacyCase::V1 { case, rects } => {
                for (index, rect) in rects.iter().enumerate() {
                    let place = PrivacyPlace::Scene {
                        case: case.clone(),
                        index,
                    };
                    compare(place, *rect, base, base_zones, &mut findings);
                }
            }
            PrivacyCase::OtherSchema { .. } => cases_other_schema += 1,
        }
    }
    PrivacyComparison {
        package_id: package_id.to_owned(),
        findings,
        unseen: PrivacyUnseen {
            expected_texts: reads.expected_texts,
            aliases: reads.aliases,
            cases_other_schema,
        },
    }
}

/// The findings of the rectangle `rect` at `place`: the whole base, or each zone it meets.
fn compare(
    place: PrivacyPlace,
    rect: PackRect,
    base: PackCoordinateSpace,
    zones: &[PrivacyZone],
    findings: &mut Vec<PrivacyFinding>,
) {
    if covers_base(rect, base) {
        findings.push(PrivacyFinding::WholeFrame { place });
        return;
    }
    for zone in zones.iter().filter(|zone| meets(rect, zone.rect)) {
        findings.push(PrivacyFinding::ZoneMet {
            place: place.clone(),
            rect,
            zone_id: zone.id.clone(),
            zone_kind: zone.kind,
            zone_rect: zone.rect,
        });
    }
}

/// Whether `rect` covers the whole of `base`.
fn covers_base(rect: PackRect, base: PackCoordinateSpace) -> bool {
    rect.x <= 0
        && rect.y <= 0
        && i64::from(rect.x) + i64::from(rect.width) >= i64::from(base.width)
        && i64::from(rect.y) + i64::from(rect.height) >= i64::from(base.height)
}

/// Whether the intersection of `one` and `other` has an area above zero.
fn meets(one: PackRect, other: PackRect) -> bool {
    let span = |start: i32, length: i32| (i64::from(start), i64::from(start) + i64::from(length));
    let (left, right) = span(one.x, one.width);
    let (top, bottom) = span(one.y, one.height);
    let (other_left, other_right) = span(other.x, other.width);
    let (other_top, other_bottom) = span(other.y, other.height);
    left.max(other_left) < right.min(other_right) && top.max(other_top) < bottom.min(other_bottom)
}

/// The word of a region's kind.
fn rect_kind(kind: PackReadKind) -> PrivacyRectKind {
    match kind {
        PackReadKind::TemplateSearch => PrivacyRectKind::TemplateSearch,
        PackReadKind::ColorCheck => PrivacyRectKind::ColorCheck,
        PackReadKind::Color => PrivacyRectKind::Color,
        PackReadKind::ColorDigest => PrivacyRectKind::ColorDigest,
        PackReadKind::Ocr => PrivacyRectKind::Ocr,
        PackReadKind::Nn => PrivacyRectKind::Nn,
        PackReadKind::SharedOcr => PrivacyRectKind::SharedOcr,
        PackReadKind::SelectName => PrivacyRectKind::SelectName,
        PackReadKind::SelectQuantity => PrivacyRectKind::SelectQuantity,
    }
}
