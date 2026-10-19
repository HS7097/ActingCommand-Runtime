# Package references and local source material

Workflow #336 option 丙: a package is identified by the `package_id` its `control.json`
declares (recorded as `package_label`) and found by its path. Since PKG-S4 no input surface
takes a package digest: the Runtime requests (`ContainedTaskRequest` and its recovery
binding, `PackageDebugRequest`, the contained Lab operation, contained observation and
saved-artifact OCR requests), the actingd configuration (`instances[].startup_package`,
`policy.procedure_manifest[]`), `actingctl task-run`, the MCP
tools and the actinglab flags name a package by its path alone, and a request or a
configuration that still names `expected_sha256` or `package_digest` does not decode. Since
PKG-S5 no package reference type remains (`PackageRef`, its content-directory and legacy ZIP
forms and the `sha256:`-prefixed serializers are gone with their last users): the bundle index
names each pack by its `package_id` (bundle v4, below), a Lab recording writes
`<package_id>.<ext>`, and `actinglab package digest` is an unknown command. The
`content-directory.v1` value below is only the content hash of the suspension rule.

## Content-directory admission

Workflow #336 PKG-S1 (option 丙): a package is admitted by its path, structurally. Neither a
digest of the package nor a pin inside it (a legacy manifest's per-file hashes, a select
step's `policy.sha256`, an OCR dictionary's or truth set's `sha256`) is computed or compared,
and no name is read as an identity. The path alone chooses the reading:

| Path | Reading |
|---|---|
| a directory | the content directory, a source package |
| `*.json` | the single JSON container, a source package |
| `*.zip` without `resources/manifest.json` | the ZIP container, a source package |
| `*.zip` holding `resources/manifest.json` | an assembled (legacy) package, expanded as it is |

A source package never holds `resources/manifest.json` (`source_contains_derived_output`),
so the two kinds of ZIP are never confused. The locator itself is canonicalized first (links
above it are resolved); every entry inside a directory is read without following links, and
a file is opened without following a link. Admission runs in this order under one deadline:

1. The locator must be absolute. Nothing names the package but its path (PKG-S4). Since
   PKG-S2 the ledger and the evidence name a package by its `package_id` (`package_label`:
   `PackageAdmitted`, the entry-recovery facts, the Lab and observation evidence) and carry
   no package digest; since PKG-S4 neither does a package debug summary.
2. The content is read into memory exactly once, as a table of `/`-separated relative paths
   and their bytes. A directory contributes every regular file at any depth; empty
   directories do not contribute. A regular file is read whole and treated by its extension
   (the table above, and "Containers"). Anything else fails
   `content_directory_not_directory`. Links, junctions and every other reparse point,
   including cloud placeholders, fail with `content_directory_link_or_type`. A relative
   path in a directory must be UTF-8 (`content_directory_path_encoding`). Every entry, from
   any container, passes the same rules with the same codes: the path is safe as defined
   for `bundle_path` and contains no `.git` segment (`content_directory_path_invalid`);
   paths that differ only by ASCII case fail with `content_directory_case_collision`;
   executable or script extensions are rejected as for ZIP entries. A file that starts with
   `version https://git-lfs.github.com/spec/v1` fails with
   `content_directory_lfs_pointer`. The existing file count, per-file, total and
   resident limits apply, and nesting is limited to 64 segments.
3. A directory's file identity (its volume and file index) is read before and after the
   read; if they differ, the directory was replaced while it was read (for example by the
   two renames of an in-place package update) and admission fails
   `content_directory_changed_during_read`, so a table read part from the old directory and
   part from the new one is never admitted (D16). A container or an assembled ZIP is read as
   one file and needs no such check.
4. The same snapshot is assembled in memory ("In-memory assembly" and the
   self-contained layout below) and issued. Nothing is read from disk afterwards.

The content hash `content-directory.v1` is the lowercase hex SHA-256 of the line
`actingcommand.package.content-directory.v1` followed by one line per file,
`<lowercase hex SHA-256 of the raw bytes>  <relative path>` (two spaces), ordered by the
UTF-8 bytes of the `/`-separated relative path; every line ends with `\n`. Raw bytes are
hashed: line endings, byte-order marks, alternate data streams, timestamps and
attributes are neither normalized nor included, and paths are not Unicode-normalized.
`actingcommand_contract::content_directory_digest` is the single implementation. It is the
content hash of the suspension rule (Workflow #336 M-PKG §3.3, [policy-suspension.md](policy-suspension.md)):
since PKG-S5 its one caller is containment's `measure_content_hash`, measured when a failure
suspends a pair and at a lift check (an assembled package ZIP is measured as the SHA-256 of the
whole file); no admission, authoring tool or recording computes it (guards S1-T7, S3-T6,
S5-T5). The same value can be recomputed with coreutils (Git Bash or Linux):

```sh
cd <package directory> && { printf 'actingcommand.package.content-directory.v1\n'; find . -type f -printf '%P\0' | LC_ALL=C sort -z | xargs -0 sha256sum -b | sed 's/ \*/  /'; } | sha256sum -b | cut -c1-64
```

## Containers

Workflow #336: the same content can be held by three containers. All three have the one
content hash; the container is chosen by the locator alone (a directory, or a regular file by its extension, ASCII case-insensitive) and
is recorded nowhere: requests and ledger records are unchanged. A Lab recording's
`record stop` writes its package as `<package_id>.zip` or `<package_id>.json` in these
containers (`lab-recording.md`, "Container and name").

| Container | Locator | Table entries |
|---|---|---|
| Directory | a directory | one per regular file, its raw bytes |
| ZIP | a regular file named `*.zip` | one per file entry, its decompressed bytes; directory entries (names ending in `/`) are ignored |
| Single JSON | a regular file named `*.json`, schema below | one per key of `files`, the UTF-8 encoding of its string value |

A regular file with another extension fails `content_container_unsupported`, and a container
file larger than the compressed-package limit (512 MiB) fails
`content_container_size_limit`. Every expanded entry then passes the entry rules of
"Content-directory admission" with the directory's codes; a path repeated inside one ZIP
fails `content_directory_case_collision` like two paths that differ only by case.

ZIP rules:

- An entry name is its raw bytes read as UTF-8, whether or not the entry sets the UTF-8 flag
  (never the CP437 reading of an unflagged name); bytes that are not UTF-8 fail
  `content_zip_entry_invalid`.
- A name must already be a `/`-separated relative path: no `\`, no `:`, no `..` segment and
  no leading `/` (`content_zip_entry_invalid`, the rule of ZIP package entries).
- Entry names are paths from the archive root (`control.json`, not `<D>/control.json`): zip
  the directory's contents, not the directory itself. Windows Explorer "Send to > Compressed
  (zipped) folder" and `Compress-Archive -Path <D>` add a top-level `<D>/`, and the package
  then has no `control.json` at its root and fails admission;
  `Compress-Archive -Path <D>\* -DestinationPath <D>.zip`
  or Python `shutil.make_archive(<out>, "zip", root_dir=<D>)` put the entries at the root.
- A symbolic link or any other non-regular entry fails `content_zip_entry_invalid`.
- An archive that cannot be read, an encrypted entry, an unsupported compression method or
  damaged entry data fails `content_zip_invalid`.

The single JSON container, `actingcommand.package.content-json.v1`:

```json
{"schema_version":"actingcommand.package.content-json.v1",
 "files":{
   "control.json":"{\n  \"schema_version\": \"Lab-1y.control.v2\",\n  ...\n}\n",
   "resources/operations/resources.json":"{\n  \"schema_version\": \"1.0\",\n  \"resources\": [],\n  \"resource_count\": 0\n}\n",
   "resources/operations/<task>/task.json":"{\n  \"schema_version\": \"0.9\",\n  ...\n}\n"}}
```

- The document is UTF-8 JSON without a byte-order mark, with exactly the two keys
  `schema_version` (exactly the value above) and `files`, an object with at least one key;
  anything else fails `content_json_invalid`.
- Every value of `files` is a string (`content_json_file_not_string`). A path that appears
  twice fails `content_json_duplicate_path`; it is never resolved by keeping either value.
- Any all-text content directory fits, with any number of tasks and files; a file that is not
  UTF-8 text (an image, for example) needs a directory or a ZIP.

Why one content has one content hash in every container: the hash depends only on the table of
paths and bytes, sorted by path, so ZIP compression, timestamps and entry order, and JSON key
order, whitespace and escaping (`"\u00e9"` or `"é"`) take no part. A JSON string decodes to
exactly one sequence of code points (unpaired surrogate escapes and non-UTF-8 input are
refused) and has exactly one UTF-8 encoding. Nothing is normalized: line endings, byte-order
marks inside a file and key order stay as they are. Extracting a ZIP, or writing every JSON
string to its path as a file, gives a directory with the same hash; zipping a directory's
contents with `/`-separated UTF-8 names, or storing each file of an all-text directory as its
string, gives a container with the same hash. Files stay strings rather than inline JSON
objects so that the bytes, and the hash, never depend on how a serializer writes JSON.

`actingcommand_pack_containment::expand_content_container` expands container bytes in
memory under these rules; `Containment::load_content_entries` (and its
`ExternallyVerifiedBundle` and `PreparedContainedTask` wrappers) admits such a table as
admission of a read locator does from step 2 on, entry rules included (and, like every
admission, compares no digest), so a container can be checked before it is written. Since
Workflow #336 PKG-S1 an instance's `resource_package` and `startup_package`
(`contracts/actingd-check-config.md`) are read by their path like every other locator.

A Runtime from before Workflow #336 refuses a container file with
`content_directory_not_directory`, and its configuration check refuses a procedure binding
that points at one with `procedure_package_not_regular` (see
`contracts/actingd-check-config.md`), in both cases before any package is admitted.

To expand a JSON container into a directory (Python 3, standard library only; the target
directory must not exist):

```python
import json, os, sys

SCHEMA = "actingcommand.package.content-json.v1"

def unique(pairs):
    found = {}
    for key, value in pairs:
        if key in found:
            sys.exit(f"content_json_duplicate_path: {key}")
        found[key] = value
    return found

source, target = sys.argv[1], sys.argv[2]
with open(source, "rb") as handle:
    document = json.loads(handle.read().decode("utf-8"), object_pairs_hook=unique)
if not isinstance(document, dict) or set(document) != {"schema_version", "files"} \
        or document["schema_version"] != SCHEMA \
        or not isinstance(document["files"], dict) or not document["files"]:
    sys.exit("content_json_invalid")
os.mkdir(target)
for path, text in document["files"].items():
    if not isinstance(text, str):
        sys.exit(f"content_json_file_not_string: {path}")
    parts = path.split("/")
    if "\\" in path or ":" in path or any(part in ("", ".", "..") for part in parts):
        sys.exit(f"content_directory_path_invalid: {path}")
    destination = os.path.join(target, *parts)
    os.makedirs(os.path.dirname(destination), exist_ok=True)
    with open(destination, "xb") as handle:
        handle.write(text.encode("utf-8"))
```

The coreutils formula above, run in the resulting directory, gives the container's content hash.

## Git source-tree references

The Git source-tree loader is retired (Workflow #288 A4), and Workflow #336 PKG-S5 removed the
reference type with the rest of `PackageRef` (and the code `source_tree_loader_retired` with
it): no input names a package by a source tree, and containment never runs Git.
`actinglab capabilities` does not list it.

## In-memory assembly

Only after the entire snapshot is read may it be parsed. The same in-memory bytes
feed the pure parser owned by `pack-containment::source` and the normal reference
closure/recognition/navigation validation. The entry execution document also uses the
existing `canonical_task` transformation (including inferred guards and canonical click
geometry). The loaded capability retains original source entry bytes for provenance and
restore alongside that derived execution document.

## Self-contained source layout

The bundle contains `control.json` with the existing Lab control fields, and:

- `resources/operations/resources.json`: shared resource IDs and optional authored
  `control_points` used by navigation parsing.
- `resources/operations/<task>/task.json`: original operation declarations. The entry
  operation supplies `locale`, `coordinate_space` and `defaults` for parsing;
  `control.json` supplies canonical `game`, `server` and `entry_task_id`.
- All referenced templates, OCR dictionaries/truth sets and operation dependencies at
  their declared local paths inside this tree.
- Optional `resources/navigation/<game>.<server>.projection.json` source annotations.

Recognition pack/pages, navigation, operation index, primitives and the derived manifest
(only its `entry_task_id`, no file hashes since Workflow #336 PKG-S1) are derived only in
memory. A source tree containing those generated output
paths is rejected. All operations and shared dependencies in the self-contained bundle
are parsed together; references to missing resources fail before any input.

## Content-directory tools and the bundle index

Workflow #336 PKG-S5 retired `actinglab package digest`: it is an unknown package command. No
tool names a package by a digest; a pack is named by the `package_id` its `control.json`
declares.

`actinglab package bundle --applications <file> --packs-root <directory> --out <new
directory> [--source-repository <owner>/<name> --source-commit <commit>] [--maintenance <file>]
[--pages <file>] [--interrupts <file>] [--navigation <file>]` lays out the resource section of
a standard package:

```text
applications.json              the given applications table, copied byte for byte
bundle.json                    actingcommand.bundle.v4
pages.json                     with --pages, copied byte for byte (Workflow #373 NAV-T)
interrupts.json                with --interrupts, copied byte for byte
navigation.json                with --navigation, copied byte for byte
packs/<package_id>/control.json
packs/<package_id>/resources/...
```

Every entry of `--packs-root` must be one pack source directory (anything else fails
`package_bundle_packs_root_invalid`). Each is read with the loader's snapshot; only
`control.json` and `resources/**` are copied into `packs/<package_id>/`, named by the
`package_id` that `control.json` declares, and the copy is read again and admitted in full by
its path (structural admission; no digest is computed). A `package_id` that is not one path
segment or is a name the game root reserves, or that two pack directories both declare, fails
`package_bundle_index_invalid`. The reserved names (`reserved_package_id`, any ASCII case) are
`tables` (the game root's tables directory), a root file's name, every one of which is
`<kind>.json` (Workflow #373 NAV-T, M-373 §3.1), and what a replacement in place leaves,
`<name>.part` and `<name>.old-<unix_ms>`. Its `control.json` must state
the applications table's `game` and one of its servers (`package_bundle_pack_mismatch`), and
every `servers.<server>.default_package_id` must name a pack of that server
(`package_bundle_default_package_missing`). The output is written to `<out>.part` and renamed
to `--out` only when complete; `--out` and `<out>.part` must not exist
(`package_bundle_out_exists`). A failure leaves `<out>.part` in place and names it; nothing
is removed.

**Standard-pack root files** (Workflow #373 NAV-T, M-373 §3.9 second row). `--pages`,
`--interrupts` and `--navigation` name the root files the standard package carries beside
`bundle.json` (a flag without its path is refused, a file that cannot be read is
`validation_failed` naming it). With any of them every pack is admitted for real
(`PreparedContainedTask::describe_navigation_path`, the admission of `describe_path`), the game
root the bundle makes is read by the one structure check every reader runs
(`read_standard_pack_root`, "Standard package root" below: a refused file is
`standard_pack_root_invalid` with `game`, the given file's `path`, `standard_pack_problem` and
`json_pointer`), and its packages are cross-checked for the parts the files name them for, as
the Runtime checks them when a run first uses them:

1. each feature package of `pages.json` is a `recognize_only` package of the game and its bundle
   server whose pages are exactly the listed pages (its admission checks the REC-M static budget
   of its page set);
2. each edge's package can be a layer of a run of its server, its entry holds the edge's `from`
   (its `entry_page`, or a `linear_steps` package's first step page), it ends on the edge's `to`,
   and its entry is no application entry (`any`);
3. each recovery chain, handler and `then.run` package can be a layer and declares no
   prerequisite package of its own;
4. each handler's entry holds its interrupt page;
5. (Workflow #411 B15, [remedies.md](remedies.md); written ahead of slice R1) across the packages
   of the game root: every capability a mapping's `remedy` names is provided by a package of the
   root, the packages' remedies and provided capabilities form no cycle, and the providers of one
   capability declare identical parameters. A remedy that breaks this is reported (below), never
   refused: the Runtime stops such a remedy for a person at run time.

"Can be a layer" is the Runtime's predicate (`prerequisite_incompatibility_with`): no
`recognize_only` package, stability termination, post-admission OCR, readings, adjustments,
`on_empty` select step or designated operation, and the game, the package's bundle server, and
the resolution of that server's feature package (the one every run of the server that navigates
or handles a shared interrupt page uses; a server without one compares no resolution). A
package that does not fit is `package_bundle_navigation_package_invalid` with `package_id` and
the `navigation_role` and `navigation_problem` the Runtime's
`contained_task_navigation_package_invalid` would carry; a package that cannot be admitted is
`admission_failed` with the loader's refusal as its cause. Then the report "entry page not in
the list" (M-373 §4.4, §10.1 NAV-4): one `package_bundle_entry_page_unlisted` warning (with
`package_id` and `page_ids`) in `warnings` per pack whose entry pages, as the Runtime's entry
gate reads them when its game has navigation data, are not all listed in `pages.json`. After
them, the report of item 5: one `package_bundle_remedy_invalid` warning in `warnings` per finding,
with `package_id`, `remedy_problem` (`capability_unprovided`: no package of the root provides the
capability; `cycle`: one entry per package on the cycle; `parameters_mismatch`: two providers of
one capability declare different parameters) and `need`, `capability` and `outcome_key` where they
apply; a root whose remedies are all provided reports nothing new. The
root files are copied byte for byte last.

`actinglab standard-pack check --root <game root>` runs the same checks on an installed game
root `<standard_packs.root>/<game>/` (the game is the directory's name; its packages are the
directories its `bundle.json` lists): the root files it holds, the cross-check and the report.
It only reads. Its answer is `{status: "checked", root, game, root_files, pack_count,
warnings}`, where `warnings` is the report (empty when there is nothing to report; its
entry-page entries need `pages.json`); a root without any root file is `standard_pack_root_invalid` `bundle_missing`, a
listed package without its directory `admission_failed`.

The bundle index v4 (`actingcommand_contract::BundleIndex`, checked by `validate()`) is a file
format only and is never recorded in the ledger:

```json
{"schema_version":"actingcommand.bundle.v4","game":"neutral",
 "source":{"repository":"example-owner/neutral-resources","commit":"<40 lowercase hex digits>"},
 "packs":[{"package_id":"neutral.test.task","server":"test","entry_task_id":"task",
           "path":"packs/neutral.test.task","file_count":4,"byte_count":1234}],
 "maintenance":[{"package_id":"neutral.test.task","server":"test","uses":["startup"]}],
 "tables":["unit_scores"]}
```

`game` and `server` are 1-128 bytes of `[a-z0-9._-]` usable as one path segment;
`package_id` and `entry_task_id` are trimmed text of 1-128 bytes, and `package_id` is one safe
path segment (no `/`, `\`, `:` or other characters a file name cannot hold, never `.` or `..`)
and never `tables` in any ASCII case (`invalid_bundle_pack_path`; Workflow #357 M-357a §2.1).
At least one pack is listed, package ids are unique (`duplicate_bundle_pack`), `path` is
exactly `packs/` plus `package_id` and `file_count` is positive. No member carries a digest.
`source` is optional information (`<owner>/<name>` and a 40 or 64 digit lowercase hex commit)
and never part of a pack's identity; the index carries no version or tag. It names no default
package: the applications table beside it does. `maintenance` is optional ("Maintenance
declarations" below). `tables` is optional (M-357a TB12): at most 64 unique table ids of
`[a-z][a-z0-9_]{0,63}`, the tables the standard package ships in the game root's `tables/`
directory; a reader checks it (`invalid_bundle_index`) and otherwise ignores it, and only an
updater uses it: it replaces exactly the listed tables, leaves the root's other tables alone,
and replaces the tables before the package directories. `package bundle` writes no `tables`
member; the resource repositories' bundle script adds it (PKG-S7). Bundle index versions 1, 2
and 3 (`actingcommand.bundle.v1`, `.v2`, `.v3`) are other shapes no reader of this build
accepts: a root whose `bundle.json` holds one is `standard_pack_root_invalid` with
`standard_pack_problem` `malformed` (no old version is read).

### Maintenance declarations

`package bundle --maintenance <file>` writes the bundle's `maintenance` declarations. The file
is a JSON array of the shared `BundleMaintenance` entries, for example:

```json
[{"package_id":"neutral.test.start","server":"test","uses":["startup"]},
 {"package_id":"neutral.test.home","server":"test","uses":["return_home"]}]
```

The bundle index then carries the required declarations as `maintenance`; an explicitly empty
array is written as `[]` and still requests the full qualification below. Without the flag the
index has no `maintenance` member and each pack is admitted structurally only. The input is
bounded by the existing package input byte limit (32 MiB). The output contains the declaration
only in `bundle.json`; the source declaration file is not copied.

Resource repositories name the source input `maintenance.json` at their root. The existing
`resource validate` declaration reader decodes this shared array and calls
`validate_bundle_maintenance_declarations` for its source-only structural rules, under
the declaration reader's own 16 MiB document / 64 MiB total read bounds. It does not load
pack material. The bundle index validator reuses those same rules and adds index closure;
generation then checks actual material and full chains through the kernel.

Each entry has exactly `package_id`, `server` and `uses`. Uses are a nonempty set of at
most the three values `startup`, `return_home`, `recognition` (Workflow #384
D2-S2: the standard package's recognition maintenance package,
[resource-readings.md](resource-readings.md), "Recognition maintenance packages"). The use
`prerequisite` is retired (Workflow #373 NAV-3a, M-373 §3.8): every pack of a bundle is a
prerequisite package of its game's registry ("Standard package root" below), so no
declaration names one. An entry that still names it is an unknown use and is refused, with no
compatibility reading: `package bundle --maintenance` and `resource validate` refuse the
declaration file, and a game root whose `bundle.json` holds it is `standard_pack_root_invalid`
with `standard_pack_problem` `malformed`. A reference
must name a pack in this index with the same server; the package/server pair is unique. Each
server has at most one startup, one return-home and one recognition entry. Unknown versions, fields or uses,
duplicate uses or roles and missing references fail explicitly. There are at most as
many entries as packs. `BundleIndex::validate()` checks them with the rest of the index.

`PreparedContainedTask::describe_path(instance_label, locator, deadline)` in
execution-kernel admits actual material by its path through Containment's metadata-only
capability (Workflow #336 PKG-S5: by its path alone), performs the same task preparation as
Runtime and returns a read-only `TaskPackageDescriptor`. It returns no runnable task and
performs no provider inference. The descriptor exposes the actual package/task identity,
mode, game, server, resolution, declared prerequisite, resolved linear entry page,
maximum step count, use qualification and (Workflow #384 D2-S2) the keys of its reading marks.
Insufficient material fails preparation;
callers must treat its identity/qualification as unknown, never infer it from paths,
application names or index labels. This does not prove execution or recognition results.

`validate_bundle_maintenance(&index, &descriptors)` checks that the admitted material set
matches every indexed identity `(package_id, server, entry_task_id)` and the index's game
(the index carries no digest, Workflow #336 PKG-S5). It then checks each use, and every
complete prerequisite chain against the registry the bundle becomes (Workflow #373 NAV-3a):
every pack of the bundle binds a prerequisite package, a declared prerequisite the bundle
does not hold is `contained_task_prerequisite_unbound`, and `return_home` binds each server's
fallback. Startup
uses ordinary task admission and rejects resource readings through
`PreparedContainedTask::startup_incompatibility`, the same predicate used by Host before
its startup lease. Prerequisites retain the existing exclusions for
recognize-only, stability termination, post-admission OCR/fields, resource readings and
designated operations. Return-home packages additionally declare no prerequisite of
their own. A recognition package is no `recognize_only` package, declares no stability
termination and no designated operation and carries at least one reading mark (Workflow #384
D2-S2); it takes no part in the chains. The shared `PrerequisiteChain` predicate owns game/server/resolution
compatibility, cycle detection, three declared layers and the 1,000-step aggregate bound.
A return-home fallback is outside the three declared layers. A layer with a page entry
and no declared prerequisite selects that fallback: a linear package with a resolved
first-step page, and (Workflow #373 NAV-3a) a page-graph layer below the chain's own package
whose entry is checked; an application entry does not, and a page-graph package as the
chain's own package keeps its home entry instead. Host retains loading, deadlines, failure
facts and execution, consuming this same predicate incrementally.

Consumers validate structure and actual material before deriving bindings: `startup`
supplies the instance's existing `startup_package`; `recognition` supplies the instance's
`recognition_package` (a location only, Workflow #384 D2-S2). Workflow #373 NAV-3a (M-373
§4.3): the Runtime itself reads an installed bundle as its game's registry ("Standard package
root" below), so every listed package can be a prerequisite package and `return_home` names
the game/server return-home package there; neither is copied into the configuration any more
(the keys `prerequisite_packages` and `return_home_packages` are retired), and the use
`prerequisite` is gone. Configuration conflict handling and application belong to the installer. No declaration triggers a
task directly or adds a Runtime execution mode, ledger field or persistent authority.

Publication callers requesting maintenance must require the machine-readable result's
`index.schema_version` **and the written `bundle.json` schema_version** to equal
`actingcommand.bundle.v4` with a `maintenance` member, and read back its validated maintenance
declarations. A zero exit status from a tool that ignored an unknown flag is insufficient.
Consumers that do not understand v4 must reject it before deriving bindings.

### Formatting and editing a content directory

Workflow #390 TOOL-1 (forced compromise P-03). Two `actinglab` tools work on a content
directory named by its path (`--package <directory>`, relative to the working directory when it
is not absolute; a file such as a `.zip` or `.json` container is a usage error,
`validation_failed`). Each reads the directory once with the loader's own snapshot (the rules of
"Content-directory admission": no links, no LFS pointers, the same limits; a refusal is
`package_invalid`). Neither computes, compares nor reports a digest, and a changed package needs
no other step: the next admission reads it as it is (option 丙).

The canonical serialization of a JSON file of a package: UTF-8 without a byte-order mark;
two-space indentation, one object member or array element per line, `": "` after a member name,
an empty object or array as `{}` or `[]`; members in their document order; only what JSON
requires escaped (text stays UTF-8, a line break inside a string stays `\n`); every line ends
with LF, the last one included, so a CRLF file is rewritten with LF (the coordinator's wave-15
ruling: line endings are no part of a package's identity under option 丙, and the Lab and
resource tooling write LF). An integer keeps its digits; any other number is written in the
shortest form that reads back the same value (`0.90` becomes `0.9`). A file is a JSON file by
its extension `.json` (ASCII case insensitive); a JSON file that does not parse (a byte-order mark is accepted on input) is
`package_invalid`, and nothing is written.

`actinglab package fmt --package <directory> [--check]` rewrites every JSON file of the
directory that is not in the canonical serialization and leaves every other file byte for byte:

```json
{"mode":"package_fmt","package":"<directory>","json_files":12,"reformatted":["resources/operations/task/task.json"]}
```

With `--check` it writes nothing. When every JSON file is canonical it answers
`{"mode":"package_fmt_check","package":…,"json_files":…,"unformatted":[]}`; otherwise it fails
with `package_fmt_check_failed` (exit 2), whose details carry `package`, `json_files` and
`unformatted`, the paths in the directory's `/`-separated form, so a CI job can require it.

`actinglab package edit --package <directory> --patch <file>` applies a list of edits, each an
RFC 6902 `add` or `replace` operation on one JSON file of the package:

```json
[{"file":"resources/operations/task/task.json","op":"replace","path":"/defaults/template_threshold","value":0.92},
 {"file":"resources/operations/task/task.json","op":"add","path":"/operations/-","value":{"id":"…"}}]
```

`file` is a path in the directory (`\` is read as `/`), `path` an RFC 6901 JSON pointer (`""` is
the whole document, `~1` is `/`, `~0` is `~`) and `value` any JSON value; no other member is
accepted, and the file is at most 32 MiB. `--file <path> --add <pointer> --value <JSON>` (or
`--replace <pointer>`) gives one edit instead of `--patch`. As RFC 6902 says, `add` sets an
object member (an existing member is replaced in its place, a new one goes last) or inserts
into an array at an index up to its length (`-` appends), and its parent must exist; `replace`
needs the member or element to exist. The edits apply in order, in memory, each seeing the ones
before it. A list that cannot be read, an unknown operation or member, a file the package does
not hold or that is not a JSON file, or a path that is not a JSON pointer is
`package_edit_patch_invalid`; a pointer that names no place its operation can take is
`package_edit_pointer_unresolved`. Both name the edit by its index (`edit`, with `file` and
`path`).

Then the edited content table, each edited file in the canonical serialization, is admitted
structurally, as actingd admits a content directory (the shared entry rules, source
compilation, declaration validation and task preparation; `PreparedContainedTask::load_content_entries`).
A refusal is `package_edit_admission_refused` (details: `package`, the admission's `code`,
`detail` and `declaration` with its file, JSON pointer and reason, `written: false`) and nothing
is written. So control.json and a task that must agree are edited in one list, and a list that
leaves them apart is refused. Only after admission are the files the edits changed written:

```json
{"mode":"package_edit","package":"<directory>","package_id":"neutral.test.task","task_id":"task","edits":2,"files":["resources/operations/task/task.json"],"admission":"passed","written":true}
```

With the global `--dry-run` the edits are applied and admitted the same way and nothing is
written (`written: false`, `status: "validated"`, `dry_run: true`, `persisted: false`).

Both tools write a file to `<name>.<process id>.part` beside it and rename it over the old one,
so a reader sees the old file or the new one. A failed write stops at that file:
`package_fmt_write_failed` or `package_edit_write_failed`, with `package`, `file`, the files
already `written` and `part_left`, the `.part` file left behind (`null` when none); nothing is
removed.

## Consumers

Workflow #336 PKG-S4 (option 丙): `actingctl task-run --package <path>` and the optional
`--recovery-package <path>` name the packages by their paths; `--expected-sha256`,
`--package-ref` and their `--recovery-*` forms are refused as unknown flags. Package-consuming
Lab commands (debug/run, observe, do, preflight, dry run and resource restore) take
`--package <path>` (a directory, or a `.zip` or `.json` file, read as the path chooses) or
`--zip <path>` (a published `.zip`) and refuse the retired digest flags by name. MCP
`ac_run_pack` takes `package` and `recovery_package`, and `ac_pack_check` takes `package`.
Workflow #411 A5 (M-PKG §3.2, S5-T4): `actinglab package check --package <path>` (a directory,
a `.zip` or a `.json` file; no other flag, no positional argument) is the Runtime's own
structural admission, `PreparedContainedTask::describe_path`, the one check-config and the
daemon apply to a pack, so it admits exactly what the Runtime admits and a declared OCR model
need not be installed locally. It answers `{status: "admitted", package_id, entry_task_id}`
(exit 0), the two names as the package's control declares them, and refuses with
`package_invalid` (exit 2) whose `details.code` is the admission's own code
(`content_directory_*`, `content_container_*`, `resource_declaration_invalid` with
`details.declaration`, `contained_task_*`); a usage error is `validation_failed` (exit 2). It
only reads. `ac_pack_check {package}` runs it and answers its data verbatim; `package
preflight` and `ac_binding_draft`, which runs preflight, are unchanged.
A procedure binding's `scheduled_execution.package_path` may equally name a directory or a
container file. Evidence replay continues to
use its independent evidence ZIP hash (`lab replay-evidence --expected-sha256`).

Runtime requests carry the package path only. Workflow #336 PKG-S2 (option 丙): prepared
observations, effective configuration, task/recovery facts and Lab results name the package
by its `package_id` (`package_label`), and `EvidencePackage` by its file name; none of them
carries a package digest, and a stored record that still carries one is not read. `scheduled_execution`
uses its existing `package_path` locator; since Workflow #336 PKG-S3 the binding names no
package content (the binding fingerprint hashes the tuple v3,
`actingcommand.procedure-binding.v3`, procedure alias, operation ID, ordered yield points),
the run's identity check compares the request's path with the binding's `package_path`, and
since PKG-S4 the binding has no `package_digest` key at all. A prerequisite or return-home
package is the package its `package_id` names (Workflow #373 NAV-3a: a registry entry): one
whose control declares another `package_id` is refused with
`contained_task_prerequisite_mismatch` (M-PKG D7), as a chain layer, as the return-home
package of a page-graph entry and on the recovery ladder's first rung.
The calendar driver transports the typed request/context through the existing calls.

Forensics and resource restore require the separately supplied original package for
reconstruction, named by its path and paired with the records by its `package_id`. Evidence ZIP export/replay
keeps its own byte SHA-256 and does not archive source material or guarantee its future
availability. Current ZIP production and resource deployment remain available.

## Standard package root

Workflow #373 NAV-6 (model M-373 v1.1 §3, §4.3, §7). An installed standard package keeps its
data files beside its package directories, one directory per game below the configured
`standard_packs.root` (`contracts/actingd-check-config.md`, "Standard packs"):

```text
<standard_packs.root>/<game>/
  bundle.json        the bundle index (above); its packs are the game root's registry
  pages.json         actingcommand.pages.v1: the game's page ids and its feature packages
  interrupts.json    actingcommand.interrupts.v1: the shared interrupt pages
  navigation.json    actingcommand.navigation.v1: the edges and the recovery chain (NAV-5)
  <directory>/       one package directory per bundle pack
```

- **Naming.** A root file is `<kind>.json` with `schema_version` `actingcommand.<kind>.v<N>`
  and a `game` equal to its directory's name (and to `bundle.json`'s). Readers look only at the
  directories `bundle.json` lists.
- **Registry.** Package id → package directory: the game root plus the pack's `path` without
  its `packs/` prefix, that is the game root plus the `package_id` (bundle v4, Workflow #336
  PKG-S5); and (Workflow #373 NAV-3a) server → return-home package, the `maintenance[]` entry
  whose `uses` holds `return_home`. The registry is held in memory only. `tables/` is a
  reserved directory of the game root (Workflow #357 M-357a §2.1), so no pack is named
  `tables`. The registry is where the Runtime finds every prerequisite package (in the registry
  of the dependent package's game) and the return-home package of a game and server
  (`linear-steps.md`, "Prerequisite packages", "Return-home fallback"); no configuration key
  names either.
- **`pages.json`.** `{schema_version, game, features, pages}`: `features` names 1–8 packages of
  the registry, at most one per server (the feature package: a `recognize_only` package whose
  pages are exactly the listed ones); `pages` lists 1–256 pages `{id, kind}`, each `id` 1–64
  bytes of `[a-z][a-z0-9_]*` and listed once, `kind` `page` or `overlay`. A package's page is
  the listed page whose id equals the package's page id without the game prefix. Unknown
  members are refused.
- **`interrupts.json`.** `{schema_version, game, interrupts}`: at most 32 entries
  `{page, handlers, then, max_per_run}`; `page` is a page of `pages.json`, named once;
  `handlers` names 1–8 packages of the registry, at most one per server; `then` is `"resume"`,
  `"end_run"` or `{"run": [packages]}` (1–8 packages, at most one per server); `max_per_run`
  is 1–8. The file requires `pages.json`.
- **`navigation.json`** (Workflow #373 NAV-5, M-373 §3.5). `{schema_version, game, edges,
  recovery_chain}`: at most 1024 edges `{from, to, package_id}`, each "run `package_id` on the
  page `from` to reach the page `to`"; `from` and `to` are different pages of `pages.json`
  (never `any`, never a draft's `@origin`: `edge_page_invalid`, `edge_page_unlisted`,
  `edge_self_loop`), no two edges with the same `from`, `to` and `package_id`
  (`edge_duplicate`); `recovery_chain` names 1–8 distinct packages (`recovery_chain_count`,
  `recovery_chain_duplicate`). Every package is a package of the registry; both lists may name
  packages of every server, and a run uses those of its own. The file requires `pages.json`.
- **One check, three readers.** The contract crate's checks (`parse_standard_pack_pages`,
  `parse_standard_pack_interrupts`, `parse_standard_pack_navigation`,
  `validate_standard_pack_references`, `validate_standard_pack_navigation_references`,
  `StandardPackRegistry`) are shared by the Runtime's reader, check-config and
  `actinglab resource validate` (`resource-declarations.md`). Workflow #373 NAV-T: a whole game
  root is read by `read_standard_pack_root`, which the Runtime's reader, `package bundle` and
  `actinglab standard-pack check` run alike ("Content-directory tools and the bundle index"). A refused file is
  `standard_pack_root_invalid` with `game`, `path`, `standard_pack_problem` and `json_pointer`
  (empty for the whole document); a file that cannot be read for another reason than its
  absence is `standard_pack_root_unreadable`, its OS error the cause.
- **When the Runtime reads it.** At startup every game root, root files only and no package
  admitted: each is recorded as a `runtime.lifecycle_observed` `standard_pack_read` record
  (`standard_pack_registered` with the package count, or the root's failure); a broken root
  never stops the Runtime. In a run's preparation, the root of the run's game when it holds
  `interrupts.json` or (Workflow #373 NAV-5) `navigation.json`, and (Workflow #373 NAV-3a)
  when the run resolves a prerequisite package
  or the return-home package: a broken root refuses the run before any lease and any task
  record; a registry that differs from the one read last (its packages or its return-home
  packages) is recorded as `standard_pack_registered` with the added and removed package ids;
  what was read stays fixed for the run. A game root the run does not use is not read for
  it, and the run is as it was without NAV-6.
  Packages are admitted by their path when a run first uses them; a file replaced in place is
  used from the next run on, with no restart.
- **Shared interrupt pages** (§6). A run uses its server's feature package and the
  interrupts whose handlers name a package of its server. Each frame on which the running
  package's own expectation failed (its entry check's verdict, a page-graph frame no page of the
  package matched, a frame of a wait for a step's, transition or `expect_after` page, a frame of
  an operation's observation window that shows none of its destinations) is
  evaluated against the interrupt pages the running package does not declare itself, in one
  scene evaluation of the feature package. One match is handled: the handler runs as a layer of
  the run (`EntryRecovery*` records with `role: interrupt_handler`), then the `then.run`
  package, and the run resumes (a wait's budget is paused while it is handled), or ends with
  `contained_task_interrupt_escalated` (`end_run`); an `interrupt_handled` record follows each.
  Two or more matches are an `interrupt_ambiguous` record and nothing is handled. A page
  handled `max_per_run` times, or a run that handled 16 pages, ends the run with
  `contained_task_interrupt_limit_reached`. A feature, handler or `then.run` package that is
  not of the run's server, cannot be admitted or is not eligible (a `recognize_only` feature
  package of the run's game, server and resolution with exactly the listed pages; a handler or
  `then.run` package eligible as a prerequisite layer of the run, declaring no prerequisite
  package, a handler's entry holding its page) ends the run with
  `contained_task_navigation_package_invalid` and its `navigation_role` and
  `navigation_problem`. A handler or `then.run` package never looks at the interrupt pages
  itself.
- **Navigation** (Workflow #373 NAV-5, M-373 §4.4, §5). A run navigates at its entry gate when
  its game root holds `pages.json` and `navigation.json`, its server has a feature package, its
  package's entry pages (a `linear_steps` package's first step page; a page-graph package's
  `entry_page`, one page or a set, whether or not the gate checks it without navigation data)
  are all pages of `pages.json`, and the run is not one the host starts itself (a startup
  package, a ladder rung; their gate is as before). An entry the page list does not hold is
  recorded as `contained_task_navigation_entry_unlisted` (warning) and the run is as without
  navigation data. The gate's order: the run's own entry check (its last frame offered to the
  shared interrupt pages); then an explicit layer (the package's declared prerequisite chain,
  or a request's own recovery package of a page-graph package, which must end on a page of
  the entry or the run fails before any navigation, README "Entry page sets") and the entry
  checked again; then, when the entry still does not pass, the navigation, which takes the
  place of the return-home fallback (no return-home layer is resolved for the run's own
  package):
  - **Locate.** The feature package evaluates every listed page on one frame, in one scene
    evaluation: the entry check's last frame first, a new frame for every later locate. A shared
    interrupt page the run's package does not declare is handled first (as above) and the run
    locates again; two of them are `contained_task_interrupt_ambiguous`. Exactly one other page
    is `contained_task_navigation_located` and becomes the run's `task.page` fact
    (`<game>/<page id>`); no page is `contained_task_navigation_page_unrecognized`, several
    `contained_task_navigation_page_ambiguous`; nothing is guessed. Each locate is a
    `navigation_located` record (`attempt`, the matched pages, at most 8 listed and all counted,
    the interrupt page it handed on, the outcome).
  - **Route and hops.** The fewest-hops route from the located page to an entry page over the
    edges of the run's server, ties going to the edge the file names first (a
    `navigation_route_planned` record); page ids are compared as equal or not. No route is
    `contained_task_navigation_route_missing`. Each hop's package is admitted on its first use
    in the run and checked for the edge (eligible as a prerequisite layer of the run, its entry
    holding `from`, ending on `to`, no application entry); it runs as a layer
    (`EntryRecovery*` with `role: navigation_hop`), and the feature package then awaits `to`
    alone, a frame every capture interval of the hop's package for its step timeout (an
    `entry_recognition` of phase `navigation_target`). A hop that did not arrive is
    `contained_task_navigation_hop_unconfirmed` and the run locates again.
  - **Recovery chain.** Unrecognised, ambiguous or without a route, the next step of the run's
    server's chain is admitted (eligible as a prerequisite layer, declaring no prerequisite
    package of its own; an application entry package may be one) and runs as a layer
    (`role: recovery_chain`). It is a run the host starts itself, with the option of a startup
    package and a ladder rung: a page-graph application entry begins with its effect (Workflow
    #390 EXEC-3, wave-17 ruling 1). Then the run locates again. A spent chain ends the run with
    `contained_task_navigation_recovery_exhausted` (`count` the steps run, caused by the last
    verdict that sent the run to the chain).
  - **Arrival.** On an entry page of the page list, the run's package's own recheck decides,
    for its step timeout: it starts (its steps after the navigation's layers'), or the run ends
    with `contained_task_navigation_entry_unconfirmed`.
  - **Bounds.** 16 locates and 16 hops per gate; a layer (or the run's package) whose
    `max_steps` would take the run past 1000 steps is not run;
    `contained_task_navigation_limit_reached` names the bound (`navigation_limit`). An edge or
    chain package that is not of the run's server is not used; one that cannot be admitted or
    is not fit for its part ends the run with `contained_task_navigation_package_invalid`
    (`navigation_role` `edge` or `chain`). A layer's own failure ends the run with its own
    code. The navigation's layers, an interrupt's handler and the run the host starts itself
    never navigate. Requests carry no field for any of these packages.
