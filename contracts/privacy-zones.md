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
screen, are private too. A package may still read them at run time (a resource reading, whose value
goes only into the user's own ledger, [resource-readings.md](resource-readings.md)), but their
pixels are never stored in a repository.

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
| `kind` | `identity` (who the account is: a nickname, an account number, a server name; nothing may read there) or `value` (a quantity the account holds: only a resource reading may read there, and no scene may hold its pixels); absent means `identity` | no text `InvalidType`, another text `InvalidValue`, at `…/kind` |
| `rect` | `{x, y, width, height}`, no other member, integers: `x` and `y` from 0, `width` and `height` from 1, inside its base (`x + width` at most the base's width, `y + height` at most its height) | a member as for a base's side at `…/rect/<member>`; outside its base `InvalidValue` at `…/rect` |
| `exempt` (public, optional) | the transition list: at most 1024 entries `{package_id, content}`, no other member; `package_id` one path segment of at most 256 bytes that no game root reserves, each package once; `content` a content prefix, 12 lowercase hexadecimal digits, as adoption and a trial name a content ([standard-pack-trial.md](standard-pack-trial.md)) | no list `InvalidType` or longer `InvalidValue` at `/exempt`; an entry's member absent `MissingField`, no text `InvalidType`, another form or a package listed again `InvalidValue`, at `/exempt/<i>/<member>` |

The refusal's details are the declaration's issue, `{declaration_file: "privacy-zones.json",
field_path, schema_version, reason}`, as `declaration-exclusions.json`'s are
([resource-declarations.md](resource-declarations.md)). A file larger than 1 MiB is
`InvalidValue` at `""`; a file that cannot be read is a usage error naming it.

**Three states.** The reader (`PrivacyDeclaration`, `crates/resource-tooling`) answers a public
root with its zones by base size and its transition list, a local root, or **absent** when the
root holds no `privacy-zones.json`. A root is public only when its declaration says so: an absent
declaration is neither public nor local, and nothing of this contract takes it for public (the
later slices check nothing there and say so, `privacy_undeclared`).

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
adds it.

## The read regions of a package

`RecognitionPack::reads` (`crates/recognition-pack`, `read_regions.rs`) is the one list of the
regions a package's recognition targets read. The Lab's scene of a regression case pastes them
(`record stop`, [resource-regression-case.md](resource-regression-case.md)) and the comparison
compares them; neither makes a list of its own. In the pack's order, each target's own regions,
then each layout's shared read:

| Kind (`privacy_rect_kind`) | Region | Target |
| --- | --- | --- |
| `template_search` | a template target's `region`, its search area, which holds its crop; `record mark` writes it as the crop with its margin when it is not told one | the template |
| `color_check` | the template's `color_check.region` | the template |
| `color` | a colour probe's region | the probe |
| `color_digest` | a colour digest's region | the digest |
| `ocr` | an OCR target's region | the target |
| `select_name` | an OCR target a candidate layout reads as a feature (a `fixed_slots` slot's `targets`, a feature's `target`): a select's name region | the target |
| `nn` | an NN target's region | the target |
| `shared_ocr` | a `fixed_slots` layout's shared read, `shared_ocr.region` | the layout's id |

- A check (`all_of`, `any_of`), a value test and a click-only target read nothing of their own.
- **A press box is not in it**: a target's `click`, a click-only target, a slot's or a layout's
  `click`. Pressing reads nothing.
- **Where a region lies**: a fixed rectangle of the base; `full_frame`, the whole frame; or no fixed
  rectangle (`template_relative`, placed where a template was found, or another keyword).
- **A reading**: a region whose target a resource reading of the package names is marked a
  reading. A package's readings are the entry task's `resource_readings`, a top-level member of its
  `resources/operations/<task_id>/task.json`, each naming an OCR target of the task by `target_id`
  ([resource-readings.md](resource-readings.md)); every other region is not a reading.
- **The scene** pastes every region of the list but a layout's shared read, which no recording
  generates and which a scene never pasted, so that every scene the Lab builds is byte for byte
  what it was before this list moved into the recognition pack.

## The comparison

`compare_privacy_zones(package_id, reads, cases, base, zones)` (`crates/resource-tooling`,
`privacy_zones.rs`) takes the package's read regions (`reads`), the regression cases of the package
as its caller read them (`cases`: a case of `actingcommand.resource-regression-case.v1` with its id
and its `scene_construction.rects`, each with its `content`, or a case of another schema; empty when
the caller has none), the package's base size (`base`, its `coordinate_space`) and a public
declaration (`zones`). It answers `{package_id, findings, unseen}`:

- **`base_undeclared`** first, when the base size is no base of the declaration: no zone is then
  compared.
- Then each region of the list in its order, then each case's rectangles, case by case:
  - **`zone_met`**, once per zone a region's or rectangle's rectangle meets: their intersection's
    area is above zero (rectangles that only touch at an edge do not meet). It carries the place (a
    region: its target, its kind, whether it is a reading; a scene's rectangle: the case, the
    rectangle's index in `scene_construction.rects` and its `content`; a scene's rectangle is never
    a reading), both rectangles, and the zone's id and kind;
  - **`whole_frame`**, a region or rectangle that covers the whole base, or a region `full_frame`:
    it meets every zone, and is named once, not once per zone;
  - **`region_unbounded`**, a region that is no fixed rectangle.
- It names no pixel and no expected text, blacks nothing out, proposes no other place and decides
  nothing. Which finding refuses is its callers' rule (later slices): a reading in a `value` zone
  is allowed at run time; a scene's rectangle in any zone is refused; the transition list is
  consulted there.

## What a rectangle cannot see

Beside its findings the comparison counts, by kind (`privacy_unseen_kind`), what a rectangle check
cannot see, for the later "a person looks" note (`privacy_unseen`), never as a pass:
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
| `privacy_zone_met` | a region or a scene's rectangle meets a zone | refuses | `package_id`, `privacy_rect_kind`, `privacy_zone_id`, `privacy_zone_kind`, `privacy_reading`; `target_id`?, `path`?, `json_pointer`? |
| `privacy_whole_frame` | a region or a scene's rectangle covers the whole base | refuses | `package_id`, `privacy_rect_kind`; `target_id`?, `path`?, `json_pointer`? |
| `privacy_region_unbounded` | a region is no fixed rectangle | refuses | `package_id`, `privacy_rect_kind`, `target_id`; `path`?, `json_pointer`? |
| `privacy_base_undeclared` | the package's base size is not declared | refuses | `package_id`, `base_width`, `base_height` |
| `privacy_destination_local` | a package marked local in a root that is not local | refuses | `package_id`; `path`? |
| `privacy_undeclared` | the root holds no declaration | warns | `path`? |
| `privacy_unseen` | what a rectangle check of the package cannot see | warns | `package_id`, `privacy_unseen` (records of `privacy_unseen_kind` and `count`) |
| `privacy_exempt` | a finding of a package the transition list names with its content | warns | `package_id`, `failure_code`; `path`?, `json_pointer`? |

A declaration that does not read is the existing `resource_declaration_invalid` (the contract
crate's), with the declaration's issue as its details.

## What this version does not do

- No command calls the comparison, and no step refuses for privacy: `resource verify`,
  `standard-pack build` and `standard-pack adopt` answer what they answered before; no recording,
  scene or package changes; Runtime admission and every interface revision are as before.
- It writes no destination on a recording and reads none from a package.
- It reads no page of a zone (`pages`, a later version).
