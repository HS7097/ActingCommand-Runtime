# A repository root's sensitive zones: `privacy-zones.json`

#463 PRIV-1 (register [v012-register.md](v012-register.md) §1.478, 0.12.74; board #463, work order
#482). The reader of a resource repository root's sensitive-zone declaration, the one list of the
regions a package reads, and the comparison of the two. This version reads, lists and compares; no
command refuses anything for privacy yet ("What this version does not do").

## Why

The owner's ruling of 11-18: a person chooses a package's destination, used only locally or
submitted to a public place (our own repositories count). For a public destination privacy is
checked and restricted: around private information the region that recognises a page and the box
that is pressed are different places, and a recognition region holds no account information. For a
local destination nothing of privacy is handled. The check is one additional step of the public
case. The owner's ruling of the same evening: quantities of what the account holds, shown on a
screen, are private too. A package may still read them at run time (a reading, "A reading" below:
the kernel reads the number there, and it goes no further than the user's own run and ledger,
[resource-readings.md](resource-readings.md)), but their pixels are never stored in a repository.

Unchanged: only recorded linear packages; the tools hand back material and propose nothing; the
core holds no game's information.

## The declaration

A repository root holds one game (its `applications.json`, [resource-verify.md](resource-verify.md));
`privacy-zones.json` lies at the root and names no game. The names below are neutral examples:

```json
{"schema_version": "actingcommand.privacy-zones.v1", "repository": "public",
 "bases": [{"width": 1000, "height": 500,
            "zones": [{"id": "z_name", "rect": {"x": 10, "y": 10, "width": 120, "height": 30}},
                      {"id": "z_stock", "kind": "value",
                       "rect": {"x": 800, "y": 10, "width": 150, "height": 30}}]}],
 "exempt": [{"package_id": "pkg_old", "content": "0123456789ab"}]}
```

| Member | Rule | Refusal (`resource_declaration_invalid` at the pointer, with its reason) |
| --- | --- | --- |
| the document | one JSON object of the members below and no other | not JSON or no object: `InvalidType` at `""`; another member: `UnknownField` at `/<member>` |
| `schema_version` | `actingcommand.privacy-zones.v1` | absent `MissingField`, another text `InvalidValue`, no text `InvalidType`, at `/schema_version` |
| `repository` | `public` or `local` | absent `MissingField`, no text `InvalidType`, another text `InvalidValue`, at `/repository` |
| a `local` declaration | holds nothing else | `bases` or `exempt`: `UnconsumedField` at `/bases` or `/exempt` |
| `bases` (public) | 1 to 16 base sizes, none listed twice (the same `width` and `height`) | absent `MissingField`, no list `InvalidType`, empty or longer `InvalidValue`, at `/bases`; a base listed again `InvalidValue` at `/bases/<i>` |
| a base | `{width, height, zones}`, no other member; `width` and `height` integers from 1 | no object `InvalidType` at `/bases/<i>`; another member `UnknownField`; a side absent `MissingField`, no number `InvalidType`, below 1 or not an integer `InvalidValue`, at `/bases/<i>/<side>` |
| `zones` | a list of 0 to 256 zones; an empty list says this size has no zone | absent `MissingField`, no list `InvalidType`, longer `InvalidValue`, at `/bases/<i>/zones` |
| a zone | `{id, kind?, rect}`, no other member | no object `InvalidType`; another member `UnknownField`, at `/bases/<i>/zones/<j>`… |
| `id` | 1 to 128 bytes of ASCII letters, digits and `_-./:`, unique within its base | absent `MissingField`, no text `InvalidType`, another form or a second zone of that id `InvalidValue`, at `…/id` |
| `kind` | `identity` (who the account is: a nickname, an account number, a server name; nothing may read there) or `value` (a quantity the account holds: only a reading ("A reading" below) may read there, and no scene may hold its pixels); absent means `identity` | no text `InvalidType`, another text `InvalidValue`, at `…/kind` |
| `rect` | `{x, y, width, height}`, no other member, integers: `x` and `y` from 0, `width` and `height` from 1, inside its base (`x + width` at most the base's width, `y + height` at most its height) | a member as for a base's side at `…/rect/<member>`; outside its base `InvalidValue` at `…/rect` |
| `exempt` (public, optional) | the transition list: at most 1024 entries `{package_id, content}`, no other member; `package_id` one path segment of at most 256 bytes that no game root reserves, each package once; `content` a content prefix, 12 lowercase hexadecimal digits, as adoption and a trial name a content ([standard-pack-trial.md](standard-pack-trial.md)) | no list `InvalidType` or longer `InvalidValue` at `/exempt`; an entry's member absent `MissingField`, no text `InvalidType`, another form or a package listed again `InvalidValue`, at `/exempt/<i>/<member>` |

The refusal's details are the declaration's issue, `{declaration_file: "privacy-zones.json",
field_path, schema_version, reason}`, as `declaration-exclusions.json`'s are
([resource-declarations.md](resource-declarations.md)). A file larger than 1 MiB is
`InvalidValue` at `""`; a file that cannot be read is a usage error naming it.

**Three states.** The member `repository` is the root's own statement, `public` or `local`. The
reader (`PrivacyDeclaration`, `crates/resource-tooling`) answers its three states, a public root
with its zones by base size and its transition list, a local root, or **absent** when the root
holds no `privacy-zones.json`, and decides nothing from them.

**Who says a root is public.** Whether a root is public is the caller's to establish, in the slices
that call the comparison (the adoption and the repository's check): a root is public when its
declaration says so, and the owner's criterion, a repository that goes to GitHub, will be applied
there. A root with no declaration file refuses nothing by this reader and is no error; nobody has
to write a file to be left alone.

**Zones hold on every page.** Nothing knows which page of the root a frame shows: a case's scene
pastes every read region of the package, from every case's frame, and a scene's page is only the
package's own name (`step_<nn>_<name>`); a frame carries no page and a case's `source` only the
recording and the step. So a zone of a base size holds on every page of the root at that size; a
recognition region of another page in the same place meets it too. A later version adds a
member `pages` under a new schema version, once a frame carries its root page; this one refuses
it as any unknown member.

**The transition list** names packages that were in the root before its declaration, each with the
content it had then. It is read and checked here; the comparison says nothing of it, and the later
privacy step answers a finding of such a package, while it keeps that content, as the warning
`privacy_exempt` instead. Another content of the package is not exempt.

**Validated where the root's files are.** `actinglab resource validate`, and `resource verify` in
process, read a selected `privacy-zones.json` at the repository root by this reader and report it
with family `privacy_zones`; its removal needs no other document
([resource-declarations.md](resource-declarations.md)). `resource verify`'s root selection without
`--base` and its clean check's root inputs do not name the file yet: the slice of its privacy step
adds it. What this registration changes for a repository that has the file: before this version
`resource validate`, and `resource verify` where its diff against `--base` selects the file,
refused it as an unmapped file; now a valid one passes, and a malformed one is refused as
`resource_declaration_invalid` with its pointer. A repository without the file sees no change.

## The read regions of a package

`RecognitionPack::reads` (`crates/recognition-pack`, `read_regions.rs`) is the one list of the
regions a package's recognition targets read. The Lab's scene of a regression case pastes the
regions its targets declare themselves (`record stop`,
[resource-regression-case.md](resource-regression-case.md)) and the comparison compares every
region of it; neither makes a list of its own. In the pack's order, each target's own regions,
then, layout by layout, what a candidate layout reads itself: its shared read, then each feature
it reads at an offset:

| Kind (`privacy_rect_kind`) | Region | Target |
| --- | --- | --- |
| `template_search` | a template target's `region`, its search area, which holds its crop; `record mark` writes it as the crop with its margin when it is not told one | the template |
| `color_check` | the template's `color_check.region` | the template |
| `color` | a colour probe's region | the probe |
| `color_digest` | a colour digest's region | the digest |
| `ocr` | an OCR target's region | the target |
| `select_name` | an OCR target a candidate layout reads as a feature (a `fixed_slots` slot's `targets`, a feature's `target`), and none as an integer: a select's name region | the target |
| `select_quantity` | an OCR target a candidate layout reads as an integer feature (`ocr_integer`): a select's quantity column, the integer column a `read_list` step publishes into its list; it is a reading | the target |
| `nn` | an NN target's region | the target |
| `shared_ocr` | a candidate layout's shared read, `shared_ocr.region` as declared, a rectangle or `full_frame`: a `fixed_slots` layout's, or since #463 SEL-2L-b a `repeated_anchor` layout's, which covers its readable band | the layout's id |

- A check (`all_of`, `any_of`), a value test and a click-only target read nothing of their own.
- **A press box is not in it**: a target's `click`, a click-only target, a slot's or a layout's
  `click`. Pressing reads nothing.
- **Where a region lies**: a fixed rectangle of the base; `full_frame`, the whole frame; or no fixed
  rectangle (`template_relative`, placed where a template was found, or another keyword). A fixed
  rectangle carries its reach ("Where a read can reach"); a region with no fixed rectangle carries
  the rectangle it is confined to, when the pack guarantees one ("Where an unbounded read is
  confined").
- **A feature read at an offset** (a feature with `target` and `offset`) is read by the kernel at
  its instance's origin plus the offset, with the target's own size, not at the target's own
  rectangle. The list gives it as the layout's read, of the target's kind (`template_search`,
  `color`, `color_digest`, `select_name` or `select_quantity`), beside the target's own region: in
  a `fixed_slots` layout one region per slot, at the slot's origin (its rectangle's top-left
  corner) plus the offset, the rectangle the kernel reads; in a `repeated_anchor` layout one region
  with no fixed rectangle, since the anchor's match, which places it and scales its offset, is
  known only on a frame, also when the layout declares a shared read (whose own region the list
  gives as `shared_ocr`, a fixed rectangle); such a region carries the rectangle it is confined
  to, when the pack guarantees one ("Where an unbounded read is confined"). A template read so
  takes a colour check relative to its match, which its own region already gives as no fixed
  rectangle.
- **Each region names the layout** whose read it is (its shared read, a feature it reads at an
  offset); a region its target declares itself names none.
- **The scene** pastes the regions a target declares itself, those that name no layout, and nothing
  a layout reads itself, which a scene never pasted (a recording generates no feature read at an
  offset, and a select's shared read, #463 SEL-2L-a, only beside the slot targets the scene
  pastes): every scene the Lab builds is byte for byte what it was before this list moved into the
  recognition pack. It pastes a region's area, never its reach or its bound. The scene asks for no
  reading.

### A reading

A **reading** is a region the kernel reads as a number at run time, whether or not the number is
published as a fact of the account (fix round 2, the coordinator's ruling: one meaning). One
function (`RecognitionPack::reading_targets`) collects the reading targets from every place where
the kernel turns a recognised text into a number, from the members of the package's entry task
(`resources/operations/<task_id>/task.json`, `PackReadingDeclarations::of_task`) and from the pack
itself ([resource-readings.md](resource-readings.md)):

| Place | Reading target | Where the text becomes a number |
| --- | --- | --- |
| `resource_readings[]` (a mark) | its `target_id`; the mark's `capacity_fact_key` reads the same target | `ResourceReadingDeclaration::parse_parts` (the contract crate) |
| `resource_adjustments[].amount` | its `target_id`; an adjustment without `amount` reads its key through a mark, whose target is the mark's | `judge_resource_adjustment_amount` (the execution kernel) |
| a candidate layout's integer feature (`ocr_integer`) | the target it reads (a feature's own `target`, or a `fixed_slots` slot's target of the feature): the integer column a `read_list` step publishes into its list, and a select's quantity column, read only to choose | `CandidateIntegerDeclaration::parse` (the recognition pack) |
| `post_admission_ocr.fields[]` (`fields_v1`, task schema `0.8` and `0.9`) | the `target_id` of each `unsigned_integer` field, in a field list (`field_lists`) or not | `parse_ocr_field` (the execution kernel) |
| a value test of the pack (`value_test`) | the `target_id` of its `left` side, and of its `right` side when that is another side, not a constant: the test compares them as numbers | `read_value_operand` (the recognition pack) |

A list's key (a `read_list`'s `key_feature`, a field list's `key_field`), a name, a dictionary or
string field and a boolean column are read as text or tested as a verdict: they are no reading.
The list marks every region whose target is a reading target, the region a target declares itself
and a layout's read of it alike. **A layout's shared read** is a reading when it gives its text to
at least one reading target, as the kernel counts the calls the read serves (`shared_calls`: an OCR
target the layout's slots or features read with the read's model, languages and chain, inside the
read's region on a `fixed_slots` layout, on every instance of a `repeated_anchor` one). Every other
region is not a reading. The mark decides nothing: the later callers rule that a reading in a
`value` zone may read there at run time.

### Where a read can reach

A target's consensus (`target_consensus`) and a feature's (`consensus` of a candidate layout's
feature) read the region moved by the jitter of each of their samples, `dx` and `dy`, at most 8
pixels on each axis; admission checks only that the moved region stays inside the coordinate space,
or, for a feature read at an offset, inside its slot (`fixed_slots`) or its instance's rectangle
(`repeated_anchor`), never that it stays off a zone. So each region that is a
fixed rectangle carries its **reach**: the smallest rectangle that holds it under every shift the
pack declares for it, the region itself when it declares none. The shifts of a region are:

- of a target's own search area or region (a template, a colour probe, a colour digest, an OCR
  target): each sample of its target consensus, and each sample of the consensus of a feature
  that reads it in a `fixed_slots` slot (the slot's target of the feature, or a template of its
  icon pool);
- of a feature a `fixed_slots` layout reads at an offset, in each slot: each sample of the
  feature's consensus.

A template's colour check, an NN target's region and a layout's shared read are never moved: their
reach is their region. The comparison compares a region's reach; a scene pastes its area, as before.

### Where an unbounded read is confined

A feature a `repeated_anchor` layout reads at an offset from its anchor's match has no fixed
rectangle. It carries the **bound** it is confined to when the pack guarantees one. The bound is of
the pixels the read can take its value from, not of the region the pack declares, so the cases are
decided in this order:

1. **The layout's shared read**, its declared rectangle (`full_frame`: the coordinate space), when
   it gives the feature's target its text. The row then reads no pixel of its own: it takes the
   blocks of the read whose centre lies in its region, and a block is held only inside the read's
   rectangle, so it can reach past the row's region and past the band. The read covers the band
   (admission refuses one that does not), so this bound holds whatever the anchor's factor.
2. Else **the layout's readable band** (the coordinate space when it declares none), when the anchor
   is searched at its own size only (no `scales`, or only the factor 1000) and the feature's region
   at the offset, moved by each of its samples, lies inside `instance_rect`. The kernel reads the
   features of an instance only when the instance's rectangle lies inside the band, and at its own
   size the region lies inside the rectangle; admission refuses a region outside it, and the list
   checks it itself. A sample's shift at the band's edge stays inside, since admission and the list
   check the moved region.
3. Else nothing confines it: an anchor matched at another factor moves the region by a scaled offset
   and keeps its own size, so neither the instance's rectangle nor the band holds it, and the row
   reads its own pixels.

A region placed relative to where a template was found (`template_relative`) and a keyword other
than `full_frame` are confined by nothing.

## The comparison

`compare_privacy_zones(package_id, reads, cases, base, zones)` (`crates/resource-tooling`,
`privacy_zones.rs`) takes the package's read regions (`reads`), the regression cases of the package
as its caller read them (`cases`: a case of `actingcommand.resource-regression-case.v1` with its id
and the `rect` of each of its `scene_construction.rects`, or a case of another schema; empty when
the caller has none), the package's base size (`base`, its `coordinate_space`) and a public
declaration (`zones`). It answers `{package_id, findings, unseen}`:

- **`base_undeclared`** first, when the base size is no base of the declaration: no zone is then
  compared.
- Then each region of the list in its order, then each case's rectangles, case by case:
  - **`zone_met`**, once per zone a region's or rectangle's rectangle meets: their intersection's
    area is above zero (rectangles that only touch at an edge do not meet). A region is compared by
    its reach, and a region with no fixed rectangle by its bound; a scene's rectangle as it is. It
    carries the place (a region: its target, its kind, whether it is a reading and the layout
    whose read it is, if any; a scene's rectangle: the case's id and the rectangle's index in
    `scene_construction.rects`, nothing free-form the case writes, and never a reading), the
    rectangle compared and what it is (`extent`: `area`, a scene's rectangle or a region's area;
    `reach`, a region's reach, with its area beside it, when they differ; `bound`, the bound of a
    region with no fixed rectangle, which may lie anywhere inside it), and the zone's rectangle, id
    and kind;
  - **`whole_frame`**, a region's area or a scene's rectangle that covers the whole base, or a
    region `full_frame`: it meets every zone, and is named once, not once per zone;
  - **`region_unbounded`**, a region that is no fixed rectangle and that nothing confines ("Where an
    unbounded read is confined").
- It names no pixel and no expected text, blacks nothing out, proposes no other place and decides
  nothing. Which finding refuses is its callers' rule (later slices): a reading in a `value` zone
  is allowed at run time; a scene's rectangle in any zone is refused; the transition list is
  consulted there.

## What a rectangle cannot see

Beside its findings the comparison counts, by kind (`privacy_unseen_kind`), what a rectangle check
cannot see, for the later "a person looks" note (the warning `privacy_unseen`, its value
`privacy_unseen_counts`), never as a pass:
`expected_text`, the texts the package's OCR targets expect; `alias`, the aliases its layouts'
identity entries are read as; `case_other_schema`, its regression cases of another schema, or of
none, whose scenes were not compared.

## Codes

Declared ahead of their first answer (as remedies' codes were, register §1.110): in 0.12.74 no
command answers them; the later privacy steps of `resource verify` and `standard-pack build` and of
`standard-pack adopt` answer them from the comparison's findings. Each is pending, owner
resource-tooling, ladder class `none`.

| Code | What it says | Refuses or warns | Values |
| --- | --- | --- | --- |
| `privacy_zone_met` | a region (by its reach, or its bound) or a scene's rectangle meets a zone | refuses | `package_id`, `privacy_rect_kind`, `privacy_zone_id`, `privacy_zone_kind`, `privacy_reading`; `target_id`?, `path`?, `json_pointer`? |
| `privacy_whole_frame` | a region or a scene's rectangle covers the whole base | refuses | `package_id`, `privacy_rect_kind`; `target_id`?, `path`?, `json_pointer`? |
| `privacy_region_unbounded` | a region is no fixed rectangle and nothing confines it | refuses | `package_id`, `privacy_rect_kind`, `target_id`; `path`?, `json_pointer`? |
| `privacy_base_undeclared` | the package's base size is not declared | refuses | `package_id`, `base_width`, `base_height` |
| `privacy_destination_local` | a package marked for local use only is in a root that is public (who says a root is public: "Who says a root is public" above) | refuses | `package_id`; `path`? |
| `privacy_undeclared` | the root holds no declaration | warns | `path`? |
| `privacy_unseen` | what a rectangle check of the package cannot see | warns | `package_id`, `privacy_unseen_counts` (records of `privacy_unseen_kind` and `count`) |
| `privacy_exempt` | a finding of a package the transition list names with its content | warns | `package_id`, `failure_code`; `path`?, `json_pointer`? |

A declaration that does not read is the existing `resource_declaration_invalid` (the contract
crate's), with the declaration's issue as its details.

## What this version does not do

- No command calls the comparison, and no step refuses for privacy: `resource verify`,
  `standard-pack build` and `standard-pack adopt` answer what they answered before, but for a
  repository's own `privacy-zones.json`, now read instead of refused as unmapped ("Validated where
  the root's files are" above); no recording, scene or package changes; Runtime admission and
  every interface revision are as before.
- It writes no destination on a recording and reads none from a package.
- It reads no page of a zone (`pages`, a later version).
