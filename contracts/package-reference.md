# Package references and local source material

`PackageRef` is the immutable resource identity used by containment, Runtime requests,
policy bindings and ledger evidence. The local package path only locates the material.

A ZIP reference retains its existing JSON string and SHA-256 value. Slots that already
use `sha256:<hex>` (policy bindings/dispatch facts and `EvidencePackage.sha256`) keep
that encoding; task requests and semantic facts retain bare lowercase hex. Existing
field names and historical event bytes remain unchanged.

A Git source-tree reference is a JSON object in that same typed slot. Its loader is
retired (see "Git source-tree references" below); the form is still decoded, validated
and re-encoded unchanged so that records holding it stay readable:

```json
{
  "schema_version": "actingcommand.package.git-source-tree.v1",
  "repository": "https://example.org/team/resources",
  "commit": {"algorithm": "sha1", "hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
  "bundle_path": "bundles/neutral",
  "tree": {"algorithm": "sha1", "hex": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
}
```

The illustrative OIDs above must be replaced with real objects. SHA-1 uses 40 lowercase
hex digits and SHA-256 uses 64; commit and tree use the same native Git object algorithm.
The repository is a credential-free HTTPS origin with a lowercase host and without a
trailing `.git`. `bundle_path` is a safe relative path, or `.` for the repository root.
All four identity components participate in equality. A changed path, repository or
commit is a different reference even when the tree is equal.

A content-directory reference is the third form in the same slot. It carries only the
digest of the directory's content; no repository, commit or path participates:

```json
{
  "schema_version": "actingcommand.package.content-directory.v1",
  "sha256": "<64 lowercase hex digits>"
}
```

The untagged variants are tried in order (ZIP string, Git source tree, content
directory), so the object forms never decode as each other and existing ZIP strings and
Git source-tree objects decode and re-encode byte-identically. `actingctl --package-ref`
and the Lab `--package-ref` flags parse either object form; admission refuses a Git
source-tree reference with `source_tree_loader_retired`.

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

1. The locator must be absolute. The caller's reference (a request's, a configuration's or a
   Lab flag's) is recorded on the admitted package as the caller gave it and compared with
   nothing: from PKG-S1 to PKG-S4 the fields that still carry a package digest (the
   `package_sha256` of `PackageAdmitted` and of the entry-recovery facts, the Lab evidence's
   expected and actual package references, `PackageDebugSummary.verified_sha256`) hold that
   given value, not a verified one; PKG-S2 and PKG-S4 delete them. A Git source-tree
   reference is refused (`source_tree_loader_retired`).
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
   self-contained layout below) and issued with the recorded reference. Nothing is read
   from disk afterwards.

The digest `content-directory.v1` is the lowercase hex SHA-256 of the line
`actingcommand.package.content-directory.v1` followed by one line per file,
`<lowercase hex SHA-256 of the raw bytes>  <relative path>` (two spaces), ordered by the
UTF-8 bytes of the `/`-separated relative path; every line ends with `\n`. Raw bytes are
hashed: line endings, byte-order marks, alternate data streams, timestamps and
attributes are neither normalized nor included, and paths are not Unicode-normalized.
`actingcommand_contract::content_directory_digest` is the single implementation; no
admission computes it (Workflow #336 PKG-S1), only the authoring and recording tools that
still name a package by it. The same value can be recomputed with coreutils (Git Bash or
Linux):

```sh
cd <package directory> && { printf 'actingcommand.package.content-directory.v1\n'; find . -type f -printf '%P\0' | LC_ALL=C sort -z | xargs -0 sha256sum -b | sed 's/ \*/  /'; } | sha256sum -b | cut -c1-64
```

## Containers

Workflow #336: the same content can be held by three containers. All three have the one
content-directory digest; the container is chosen by the locator alone (a directory, or a regular file by its extension, ASCII case-insensitive) and
is recorded nowhere: references, requests and ledger records are unchanged. A Lab recording's
`record stop` writes its package as `<D>.zip` or `<D>.json` in these containers
(`lab-recording.md`, "Container and digest").

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

Why one content has one digest in every container: the digest depends only on the table of
paths and bytes, sorted by path, so ZIP compression, timestamps and entry order, and JSON key
order, whitespace and escaping (`"\u00e9"` or `"é"`) take no part. A JSON string decodes to
exactly one sequence of code points (unpaired surrogate escapes and non-UTF-8 input are
refused) and has exactly one UTF-8 encoding. Nothing is normalized: line endings, byte-order
marks inside a file and key order stay as they are. Extracting a ZIP, or writing every JSON
string to its path as a file, gives a directory with the same digest; zipping a directory's
contents with `/`-separated UTF-8 names, or storing each file of an all-text directory as its
string, gives a container with the same digest. Files stay strings rather than inline JSON
objects so that the bytes, and the digest, never depend on how a serializer writes JSON.

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

The coreutils formula above, run in the resulting directory, gives the container's digest.

## Git source-tree references

The Git source-tree loader is retired (Workflow #288 A4). Containment no longer runs Git
or reads the locator of this form: once the reference itself is valid, admitting it fails
with `source_tree_loader_retired` (the same source admission error family as the other
loader codes, refused as an invalid package by the host). The reference type, its
validation and its serialization are unchanged, so ledgers, evidence and configuration
that record one still decode and re-encode byte-identically. `actinglab capabilities` no
longer lists it as an accepted package reference.

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

`actinglab package digest --package <directory>` reads the directory with the loader's own
snapshot (the rules of "Content-directory admission"), computes its content-directory
reference and then admits the directory in full by its path. `--package` may equally name a
content container file
(`.zip` or `.json`, "Containers"): the same content prints the same reference whichever
container holds it. It prints `reference` (the object above), the
`package_id`, `server` and `entry_task_id` stated by `control.json`, `file_count` and
`byte_count` (of the expanded table). An author directory or container file with any other
name is used with that reference through
the explicit `--package-ref` flags; no command derives a reference from a path by itself.
A refusal is `package_invalid` with the loader's code, plus the computed digest once the
snapshot was read.

`actinglab package bundle --applications <file> --packs-root <directory> --out <new
directory> [--source-repository <owner>/<name> --source-commit <commit>]` lays out the
resource section of a standard package:

```text
applications.json          the given applications table, copied byte for byte
bundle.json                actingcommand.bundle.v2
packs/<digest>/control.json
packs/<digest>/resources/...
```

Every entry of `--packs-root` must be one pack source directory (anything else fails
`package_bundle_packs_root_invalid`). Each is read with the same snapshot; only
`control.json` and `resources/**` are copied into `packs/<digest>/`, and the copy is
read again and admitted in full under that name. Its `control.json` must state the
applications table's `game` and one of its servers (`package_bundle_pack_mismatch`), and
every `servers.<server>.default_package_id` must name a pack of that server
(`package_bundle_default_package_missing`). The output is written to `<out>.part` and
renamed to `--out` only when complete; `--out` and `<out>.part` must not exist
(`package_bundle_out_exists`). A failure leaves `<out>.part` in place and names it; nothing
is removed.

The bundle index (`actingcommand_contract::BundleIndexV2`, checked by `validate()`) is a
file format only and is never recorded in the ledger:

```json
{"schema_version":"actingcommand.bundle.v2","game":"neutral",
 "source":{"repository":"example-owner/neutral-resources","commit":"<40 lowercase hex digits>"},
 "packs":[{"package_id":"neutral.test.task","server":"test","entry_task_id":"task",
           "digest":"<64 lowercase hex digits>","path":"packs/<same digest>",
           "file_count":4,"byte_count":1234}]}
```

`game` and `server` are 1-128 bytes of `[a-z0-9._-]` usable as one path segment;
`package_id` and `entry_task_id` are trimmed text of 1-128 bytes. At least one pack is
listed, package ids and digests are unique, `path` is exactly `packs/` plus `digest` and
`file_count` is positive. `source` is optional information (`<owner>/<name>` and a 40 or
64 digit lowercase hex commit) and never part of a pack's identity; the index carries no
version or tag. It names no default package: the applications table beside it does. The
version 1 index (`actingcommand.bundle.v1`, ZIP packs and `default_packs`) is a separate
shape; its readers are unchanged.

### Maintenance declarations (bundle v3)

`package bundle --maintenance <file>` requests `actingcommand.bundle.v3`. The file is
a JSON array of the shared `BundleMaintenance` entries, for example:

```json
[{"package_id":"neutral.test.start","server":"test","uses":["startup"]},
 {"package_id":"neutral.test.home","server":"test","uses":["prerequisite","return_home"]}]
```

The resulting `BundleIndexV3` retains v2's `game`, optional `source`, `packs` and
content-directory identities and adds the required `maintenance` array. An explicitly
empty array still requests v3. Without the flag, generation retains v2 behavior. The
input is bounded by the existing package input byte limit (32 MiB). The output contains
the declaration only in `bundle.json`; the source declaration file is not copied.

Resource repositories name the source input `maintenance.json` at their root. The existing
`resource validate` declaration reader decodes this shared array and calls
`validate_bundle_maintenance_declarations` for its source-only structural rules, under
the declaration reader's own 16 MiB document / 64 MiB total read bounds. It does not load
pack material. The bundle index validator reuses those same rules and adds index closure;
generation then checks actual material and full chains through the kernel.

Each entry has exactly `package_id`, `server` and `uses`. Uses are a nonempty set of at
most the four values `startup`, `prerequisite`, `return_home`, `recognition` (Workflow #384
D2-S2: the standard package's recognition maintenance package,
[resource-readings.md](resource-readings.md), "Recognition maintenance packages"). A reference
must name a pack in this index with the same server; the package/server pair is unique. Each
server has at most one startup, one return-home and one recognition entry. Unknown versions, fields or uses,
duplicate uses or roles and missing references fail explicitly. There are at most as
many entries as packs. `BundleIndex::validate()` dispatches the version-specific
structural validation; the v2 type continues to reject v3.

`PreparedContainedTask::describe_path(instance_label, locator, recorded, deadline)` in
execution-kernel admits actual material by its path through Containment's metadata-only
capability (Workflow #336 PKG-S1: `recorded` is recorded on the descriptor and compared
with nothing), performs the same task preparation as Runtime and returns a
read-only `TaskPackageDescriptor`. It returns no runnable task and performs no provider
inference. The descriptor exposes the actual package/task identity, package reference,
mode, game, server, resolution, declared prerequisite, resolved linear entry page,
maximum step count, use qualification and (Workflow #384 D2-S2) the keys of its reading marks.
Insufficient material fails preparation;
callers must treat its identity/qualification as unknown, never infer it from paths,
application names or index labels. This does not prove execution or recognition results.

`validate_bundle_maintenance(&index, &descriptors)` checks that the admitted material set
matches every indexed identity `(package_id, server, entry_task_id)` and the index's game;
the index's digests are compared with nothing (Workflow #336 PKG-S1). It then checks each use
and
complete prerequisite chain against the bindings supplied by the declaration. Startup
uses ordinary task admission and rejects resource readings through
`PreparedContainedTask::startup_incompatibility`, the same predicate used by Host before
its startup lease. Prerequisites retain the existing exclusions for
recognize-only, stability termination, post-admission OCR/fields, resource readings and
designated operations. Return-home packages additionally declare no prerequisite of
their own. A recognition package is no `recognize_only` package, declares no stability
termination and no designated operation and carries at least one reading mark (Workflow #384
D2-S2); it takes no part in the chains. The shared `PrerequisiteChain` predicate owns game/server/resolution
compatibility, cycle detection, three declared layers and the 1,000-step aggregate bound.
A return-home fallback is outside the three declared layers. Only a linear package with
a resolved first-step page and no declared prerequisite selects that fallback; an
application entry or page-graph package does not. Host retains loading, deadlines,
failure facts and execution, consuming this same predicate incrementally.

Consumers validate structure and actual material before deriving bindings: `startup`
supplies the instance's existing `startup_package`; `prerequisite` supplies a
`prerequisite_packages` entry without editing business control; `return_home` supplies
both the package binding and the shared game/server return-home mapping; `recognition`
supplies the instance's `recognition_package` (a location only, Workflow #384 D2-S2).
Configuration conflict handling and application belong to the installer. No declaration triggers a
task directly or adds a Runtime execution mode, ledger field or persistent authority.

Publication callers requesting maintenance must require the machine-readable result's
`index.schema_version` **and the written `bundle.json` schema_version** to equal
`actingcommand.bundle.v3`, and read back its validated maintenance declarations. A zero
exit status from a tool that ignored an unknown flag is insufficient. Existing consumers
that do not understand v3 must reject it before deriving bindings; v1/v2, ZIP and content
container handling continue through their existing paths.

## Consumers

`actingctl task-run` accepts `--package <directory> --package-ref <JSON>` and optional
`--recovery-package <directory> --recovery-package-ref <JSON>` for a content-directory
reference; a Git source-tree reference is refused with `source_tree_loader_retired`. The existing ZIP/hash
flags remain accepted. Package-consuming Lab commands (debug/run, observe, do and
resource restore) accept `--package <directory> --package-ref <JSON>` instead of their
ZIP/hash flags. Wherever a content-directory reference is given, the locator may be a
content container file instead of a directory ("Containers"), including a procedure
binding's `scheduled_execution.package_path`. Evidence replay continues to use its
independent evidence ZIP hash.

Runtime requests, prepared observations, effective configuration, task/recovery facts,
Lab results and `EvidencePackage` carry the complete reference. `scheduled_execution`
uses its existing `package_path` locator and the procedure binding's typed
`package_digest`; the run's identity check compares the request's reference with the
binding's until PKG-S3, and admission records it and compares it with nothing (Workflow
#336 PKG-S1). Git source-tree
and content-directory binding fingerprints hash the canonical tuple of binding version,
procedure alias, complete reference, operation ID and ordered yield points. ZIP bindings
keep the original tuple.
The calendar driver transports the typed request/context through the existing calls.

Forensics and resource restore consume the same reference and require the separately
supplied exact material for reconstruction (the content directory; a recorded Git
source-tree reference is refused with `source_tree_loader_retired`). Evidence ZIP export/replay
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
  <directory>/       one package directory per bundle pack
```

- **Naming.** A root file is `<kind>.json` with `schema_version` `actingcommand.<kind>.v<N>`
  and a `game` equal to its directory's name (and to `bundle.json`'s). Readers look only at the
  directories `bundle.json` lists.
- **Registry.** Package id → package directory: the game root plus the pack's `path` without
  its `packs/` prefix. The registry is held in memory only; it records each pack's bundle
  reference as written and compares it with nothing (PKG-S1).
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
- **One check, three readers.** The contract crate's checks (`parse_standard_pack_pages`,
  `parse_standard_pack_interrupts`, `validate_standard_pack_references`,
  `StandardPackRegistry`) are shared by the Runtime's reader, check-config and
  `actinglab resource validate` (`resource-declarations.md`). A refused file is
  `standard_pack_root_invalid` with `game`, `path`, `standard_pack_problem` and `json_pointer`
  (empty for the whole document); a file that cannot be read for another reason than its
  absence is `standard_pack_root_unreadable`, its OS error the cause.
- **When the Runtime reads it.** At startup every game root, root files only and no package
  admitted: each is recorded as a `runtime.lifecycle_observed` `standard_pack_read` record
  (`standard_pack_registered` with the package count, or the root's failure); a broken root
  never stops the Runtime. In a run's preparation, the root of the run's game when it holds
  `interrupts.json`: a broken root refuses the run before any lease and any task record; a
  registry that differs from the one read last is recorded as `standard_pack_registered`
  with the added and removed package ids; what was read stays fixed for the run. A game root
  without `interrupts.json` is not read for a run, and the run is as it was without NAV-6.
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
