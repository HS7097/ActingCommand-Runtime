# Package references and local source material

`PackageRef` is the immutable resource identity used by containment, Runtime requests,
policy bindings and ledger evidence. The local package path only locates the material.

A ZIP reference retains its existing JSON string and SHA-256 value. Slots that already
use `sha256:<hex>` (policy bindings/dispatch facts and `EvidencePackage.sha256`) keep
that encoding; task requests and semantic facts retain bare lowercase hex. Existing
field names and historical event bytes remain unchanged.

A source reference is a JSON object in that same typed slot:

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

## Admission

The caller supplies a local Git worktree and materialized LFS files. The locator names
the bundle directory. Containment uses installed Git's read-only object commands,
checks the origin, hashes raw commit/tree/blob objects with their Git headers, proves
the tree belongs to that commit at the declared path, and compares the actual files.
Only regular files and directories are admitted. Executable mode must agree on Unix;
Windows validates the Git mode and rejects filesystem reparse points. Ambiguous links,
unsafe names, undeclared files, missing objects and out-of-limit material fail explicitly.

An LFS pointer is first verified as a Git blob; its materialized entity must match the
pointer's SHA-256 and size. A remaining pointer is an error. Admission does not fetch,
run filters, execute source code or write derived resources. Every operation shares one
absolute admission deadline and the existing file count, byte and resident limits.

Only after the entire snapshot is verified may it be parsed. The same in-memory bytes
feed the pure converter owned by `pack-containment::source` and the normal reference
closure/recognition/navigation validation. The entry execution document also uses the
existing `canonical_task` transformation (including inferred guards and canonical click
geometry). The loaded capability retains original source entry bytes for provenance and
restore alongside that derived execution document.

## Self-contained source layout

The bundle contains `control.json` with the existing Lab control fields, and:

- `resources/operations/resources.json`: shared resource IDs and optional authored
  `control_points` used by navigation conversion.
- `resources/operations/<task>/task.json`: original operation declarations. The entry
  operation supplies `locale`, `coordinate_space` and `defaults` for conversion;
  `control.json` supplies canonical `game`, `server` and `entry_task_id`.
- All referenced templates, OCR dictionaries/truth sets and operation dependencies at
  their declared local paths inside this tree.
- Optional `resources/navigation/<game>.<server>.projection.json` source annotations.

Recognition pack/pages, navigation, operation index, primitives and the dependency
hash index are derived only in memory. A source tree containing those generated output
paths is rejected. All operations and shared dependencies in the self-contained bundle
are converted together; references to missing resources fail before any input.

## Consumers

`actingctl task-run` accepts `--package <directory> --package-ref <JSON>` and optional
`--recovery-package <directory> --recovery-package-ref <JSON>`. The existing ZIP/hash
flags remain accepted. Package-consuming Lab commands (debug/run, observe, do and
resource restore) accept `--package <directory> --package-ref <JSON>` instead of their
ZIP/hash flags. Evidence replay continues to use its independent evidence ZIP hash.

Runtime requests, prepared observations, effective configuration, task/recovery facts,
Lab results and `EvidencePackage` carry the complete reference. `scheduled_execution`
uses its existing `package_path` locator and the procedure binding's typed
`package_digest`; admission and execution compare that same reference. Source binding
fingerprints hash the canonical tuple of binding version, procedure alias, complete
reference, operation ID and ordered yield points. ZIP bindings keep the original tuple.
The calendar driver transports the typed request/context through the existing calls.

Forensics and resource restore consume the same reference and require the separately
supplied exact source tree/LFS material for reconstruction. Evidence ZIP export/replay
keeps its own byte SHA-256 and does not archive source material or guarantee its future
availability. Current ZIP production and resource deployment remain available.
