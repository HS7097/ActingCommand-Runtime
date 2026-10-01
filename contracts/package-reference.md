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

The locator names a local directory, typically named by its own digest. The directory
itself is canonicalized first (links above it are resolved); every entry inside it is
read without following links. Admission runs in this order under one deadline:

1. The reference is validated and the locator must be absolute.
2. If the last segment of the locator, or of its canonical form, has the digest form
   (64 lowercase hex digits) and differs from the reference,
   `content_directory_name_mismatch` is returned before any file is read. A directory
   with any other name is verified only against the explicit reference.
3. Every regular file at any depth is read into memory exactly once. Empty directories
   do not contribute. Links, junctions and every other reparse point, including cloud
   placeholders, fail with `content_directory_link_or_type`. A relative path must be
   UTF-8 (`content_directory_path_encoding`), safe as defined for `bundle_path`, and
   must not contain a `.git` segment (`content_directory_path_invalid`). Paths that
   differ only by ASCII case fail with `content_directory_case_collision`; executable or
   script extensions are rejected as for ZIP entries. A file that starts with
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
`content_directory_name_mismatch`. It prints `reference` (the object above), the
`package_id`, `server` and `entry_task_id` stated by `control.json`, `file_count` and
`byte_count`. An author directory with any other name is used with that reference through
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
ZIP/hash flags. Evidence replay continues to use its independent evidence ZIP hash.

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
