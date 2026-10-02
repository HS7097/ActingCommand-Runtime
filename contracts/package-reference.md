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

The locator names a local directory, typically named by its own digest, or a content
container file (`<digest>.zip` or `<digest>.json`, see "Containers"). The locator itself is
canonicalized first (links above it are resolved); every entry inside a directory is read
without following links, and a container file is opened without following a link.
Admission runs in this order under one deadline:

1. The reference is validated and the locator must be absolute.
2. If the last segment of the locator, or of its canonical form, has the digest form
   (64 lowercase hex digits), or is a digest-form stem followed by `.zip` or `.json` (ASCII
   case-insensitive), and that digest differs from the reference,
   `content_directory_name_mismatch` is returned before any file is read. A locator with
   any other name is verified only against the explicit reference.
3. The content is read into memory exactly once, as a table of `/`-separated relative paths
   and their bytes. A directory contributes every regular file at any depth; empty
   directories do not contribute. A regular file is a content container, read whole and
   expanded by the extension of the locator ("Containers"). Anything else fails
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
4. The digest of those bytes is compared in constant time with the reference.
   `content_directory_digest_mismatch` reports the expected and actual digests and the
   file count. No JSON has been parsed at this point.
5. The same snapshot is assembled in memory ("In-memory assembly" and the
   self-contained layout below) and issued with the content-directory reference.
   Nothing is read from disk afterwards.

The digest `content-directory.v1` is the lowercase hex SHA-256 of the line
`actingcommand.package.content-directory.v1` followed by one line per file,
`<lowercase hex SHA-256 of the raw bytes>  <relative path>` (two spaces), ordered by the
UTF-8 bytes of the `/`-separated relative path; every line ends with `\n`. Raw bytes are
hashed: line endings, byte-order marks, alternate data streams, timestamps and
attributes are neither normalized nor included, and paths are not Unicode-normalized.
`actingcommand_contract::content_directory_digest` is the single implementation. The
same value can be recomputed with coreutils (Git Bash or Linux):

```sh
cd <package directory> && { printf 'actingcommand.package.content-directory.v1\n'; find . -type f -printf '%P\0' | LC_ALL=C sort -z | xargs -0 sha256sum -b | sed 's/ \*/  /'; } | sha256sum -b | cut -c1-64
```

## Containers

Workflow #336: the same content can be held by three containers. All three use the one
content-directory reference above and the one digest; the container is chosen by the
locator alone (a directory, or a regular file by its extension, ASCII case-insensitive) and
is recorded nowhere: references, requests and ledger records are unchanged.

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
order, whitespace and escaping (`"é"` or `"é"`) take no part. A JSON string decodes to
exactly one sequence of code points (unpaired surrogate escapes and non-UTF-8 input are
refused) and has exactly one UTF-8 encoding. Nothing is normalized: line endings, byte-order
marks inside a file and key order stay as they are. Extracting a ZIP, or writing every JSON
string to its path as a file, gives a directory with the same digest; zipping a directory
with `/`-separated UTF-8 names, or storing each file of an all-text directory as its string,
gives a container with the same digest. Files stay strings rather than inline JSON objects so
that the bytes, and the digest, never depend on how a serializer writes JSON.

`actingcommand_pack_containment::expand_content_container` expands container bytes in
memory under these rules; `Containment::load_content_entries` (and its
`ExternallyVerifiedBundle` and `PreparedContainedTask` wrappers) admits such a table as
admission of a read locator does from step 3 on, entry rules and digest comparison included,
so a container can be checked before it is written. An instance's `resource_package` and
`startup_package` (`contracts/actingd-check-config.md`) do not take containers: a file there
is still a ZIP package identified by the SHA-256 of the file itself, so a container file
there is refused as an invalid package.

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

Only after the entire snapshot is verified may it be parsed. The same in-memory bytes
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

Recognition pack/pages, navigation, operation index, primitives and the dependency
hash index are derived only in memory. A source tree containing those generated output
paths is rejected. All operations and shared dependencies in the self-contained bundle
are parsed together; references to missing resources fail before any input.

## Content-directory tools and the bundle index

`actinglab package digest --package <directory>` reads the directory with the loader's own
snapshot (the rules of "Content-directory admission" without the name comparison), computes
its content-directory reference and then admits the directory in full against that
reference, so a digest-form name that differs from the content fails
`content_directory_name_mismatch`. `--package` may equally name a content container file
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
`package_digest`; admission and execution compare that same reference. Git source-tree
and content-directory binding fingerprints hash the canonical tuple of binding version,
procedure alias, complete reference, operation ID and ordered yield points. ZIP bindings
keep the original tuple.
The calendar driver transports the typed request/context through the existing calls.

Forensics and resource restore consume the same reference and require the separately
supplied exact material for reconstruction (the content directory; a recorded Git
source-tree reference is refused with `source_tree_loader_retired`). Evidence ZIP export/replay
keeps its own byte SHA-256 and does not archive source material or guarantee its future
availability. Current ZIP production and resource deployment remain available.
