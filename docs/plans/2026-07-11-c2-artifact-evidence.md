# C2 Artifact Store And Evidence Exporter Implementation Plan

Status: active.

## Authority and baseline

- GitHub authority: Issue #35, authored by `HS7097` and labeled `状态:已批准`.
- Frozen specification: `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`.
- Frozen specification SHA-256:
  `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`.
- Approved resource-authoring amendment:
  `AMENDMENT-Issue35-Lab-resource-authoring-ownership.md`.
- Amendment SHA-256:
  `de039ad910b0b8208d52b8582b260868d4bcd2b04ec2b272977890661a136b54`.
- C2 baseline: `3643c5a0ad0867bd893846e13843e3023bbc7e0e`.
- Implementation branch: `issue-35-runtime-ledger-v3`.

## Goal

Add the production-owned artifact and evidence subsystem required by C2. Artifact bytes stay
outside ledger JSON, while typed references and capture/artifact lifecycle facts enter the global
ledger. The frame pipeline preserves the accepted Tier1/Tier2/Tier3 behavior, gives semantic and
pinned frames lossless priority, and produces evidence archives whose task outcome and evidence
completeness are independently verifiable.

## Module boundary

### `actingcommand-contract`

- Own closed artifact kinds, media types, producers, redaction states, retention classes, task
  outcomes, evidence completeness values, pinned reasons, pressure states, and policy reasons.
- Own typed capture pipeline and artifact lifecycle event payloads.
- Expose an artifact-store issuer boundary that can mint attachment capabilities but cannot be
  reconstructed from transport metadata.
- Keep artifact bytes, filesystem IO, ZIP generation, and mutable retention state out of the
  contract crate.

### `crates/artifact-store`

- Own durable artifact files, SHA-256 verification, collision-safe object keys, metadata,
  retention classification, frame buffering, and evidence export.
- Accept injected clocks, memory sampling, and typed event sinks. It does not own the global
  ledger writer, Runtime lifecycle, scheduler, device backend, task outcome, or game behavior.
- Never depend on `actingcommand-lab`, `actingcommand-actinglab`, Runtime host, scheduler, or a
  production device backend factory.
- Report critical lifecycle facts through a typed event sink. Required event append failure is
  fatal and cannot produce a successful store/export result.

### Lab compatibility during extraction

- Move the accepted frame-store mechanics and tests from `crates/lab` into `artifact-store`
  without changing thresholds, hysteresis, pause/resume, spill, or recovery behavior.
- Keep a narrow Lab compatibility facade only where needed to preserve existing A7 APIs and
  protocol goldens. Lab must consume artifact-store behavior rather than retain a second frame
  authority.
- Existing Lab ZIP behavior remains compatibility inventory until C6 routes online evidence
  export through the shared exporter. C2 acceptance uses the production exporter directly.

## Artifact store behavior

- Write to a same-directory temporary file, flush and sync it, then atomically rename it into the
  final object key. Existing final objects are never overwritten.
- Read back and verify byte count plus SHA-256 before returning a verified artifact capability.
- Use typed links for run, frame, and correlation identifiers.
- Record kind, object key, MIME type, byte count, SHA-256, creation time, producer, retention
  class, and redaction state in the artifact reference.
- Screenshot display names use `yyyyMMddHHmmssfff.png`; a same-millisecond collision uses the
  first free `-NN` suffix. Exhaustion or any pre-existing final collision fails loudly.
- Retention classes are `debug_full`, `adaptive`, and `light`. Selection is explicit in the store
  request and never inferred from a successful task outcome.

## Frame pipeline behavior

- Default observation cadence is 300 ms. A runtime override is validated and emits
  `capture.policy_changed`.
- Available frame memory is measured memory minus the configured OS reserve.
- Tier1 begins near-frame deduplication at the configured threshold and emits a closed
  `capture.dedup_window` with representative frame, count, and duration.
- Tier2 spills resident ordinary frames to artifact store and emits the corresponding pressure
  transition.
- Tier3 pauses ordinary cadence capture and records a resumable checkpoint. Hysteresis must be
  satisfied before `tier3_resumed`.
- Semantic/pinned frames bypass similarity deduplication and pressure dropping. At Tier3 they are
  written directly to artifact store.
- Pinned persistence failure is fatal for the frame operation and permanently lowers evidence
  completeness to `failed`.
- Ordinary pressure loss is counted separately from deduplication and lowers completeness to
  `partial` only when every required pinned frame remains present.
- The pipeline records enough events to distinguish an unchanged screen from a period in which
  capture was paused.

## Evidence exporter behavior

- Export a deterministic archive containing:
  - `evidence/result.json`;
  - `evidence/events.jsonl`;
  - `evidence/diagnostics.json`;
  - `evidence/summary.txt`;
  - `evidence/manifest.json`;
  - retained screenshots under `screenshots/`.
- The manifest contains run/correlation identity, package name/hash/verification, ledger sequence
  bounds, independent task outcome and evidence completeness, terminal receipt, artifact count
  and SHA-256 values, four-way screenshot counts, pinned reason distribution and missing list,
  projection profile, retention class, normalized absolute output path, and final ZIP SHA-256.
- Build the archive at a temporary path, finish and sync it, calculate and verify its real hash,
  then atomically publish it.
- Append `artifact.export_completed` only after durable publication and hashing. Structure,
  manifest, artifact, or ledger failures append `artifact.export_failed` when possible, remove the
  temporary output, return a fatal error, and never synthesize success.
- Export success describes archive structure only. It does not rewrite a `partial` or `failed`
  evidence completeness value to `complete`.

## Implementation tasks

### Task 1: Contract and authority

Status: complete.

- Add closed C2 codes, payloads, event types, projections, and strict serde tests.
- Replace the test-only artifact issuer with the C2 store issuer boundary.
- Add compile-fail and architecture guards against transport-to-authority promotion and use of
  the issuer outside artifact-store.

### Task 2: Durable artifact store

Status: active.

- Add `crates/artifact-store`, typed configuration/error models, atomic writes, hashing,
  verification, safe object paths, screenshot naming, and retention metadata.
- Add success and adversarial tests for empty bytes, path escape, collisions, interrupted writes,
  hash mismatch, sync/rename failure, and required event append failure.
- Retire `runtime-core::capture_store` after compatibility and reverse-dependency proof.

### Task 3: Frame pipeline extraction

- Move the accepted frame-store algorithm and tests into artifact-store.
- Make semantic/pinned reasons explicit and bypass both deduplication and pressure dropping.
- Route Tier2/Tier3 persistence through artifact store.
- Emit policy, dedup-window, pressure, artifact, and pinned-failure facts.
- Preserve existing Lab behavior through a narrow compatibility facade and unchanged goldens.

### Task 4: Evidence exporter

- Add typed export request, manifest, screenshot counters, pinned accounting, and export receipt.
- Implement success/failure/cancelled task-outcome exports.
- Add archive verification that reopens the ZIP and rehashes every declared artifact.
- Cover ordinary drop as `partial`, missing pinned frame as `failed`, same-millisecond names,
  corrupted source artifact, output collision, and injected archive failure.

### Task 5: Sealed/global-ledger acceptance

- Run a sealed record/replay-style fixture through frame pipeline, artifact store, real global
  ledger ingress, and evidence exporter.
- Query capture policy/dedup/pressure and artifact lifecycle facts by correlation.
- Verify all manifest hashes, ledger bounds, terminal receipt, screenshot count columns, pinned
  reason distribution, and final ZIP hash.
- Prove artifact/event failure cannot return a successful receipt or leave a published archive.

### Task 6: Closeout

- Run focused contract, artifact-store, ledger integration, Lab compatibility, and architecture
  suites.
- Run formatting, full workspace tests, non-Lab tests, Clippy with warnings denied, all-features
  checks, source/dependency guards, and `git diff --check`.
- Update `PLANS.md` and `CHECKPOINT.md`, push each completed unit, create a stable C2 checkpoint
  tag, and record evidence in Issue #36.
- Do not merge this branch into `main` or the umbrella repository.

## Explicit non-goals

- No daemon-owned capture, execution kernel, queueing, priority, preemption, or task lifecycle;
  those remain C3b/C5 work.
- No UI, remote API, game logic, OCR, SQLite, resource-repository read, or live-device action.
- No resource authoring, conversion, package publication, or `resource-tooling`; those remain the
  amendment's C5/C6 internal chain.
- No automatic retry, reconnect, fallback, fake success, or silent loss.

## Acceptance criteria

1. Artifact bytes never enter event JSON; every durable artifact has a typed, hash-verified
   reference and collision-safe object path.
2. Tier thresholds, hysteresis, spill, pause/resume, and recovery behavior remain covered by the
   migrated frame-store tests.
3. Pinned frames bypass deduplication and pressure loss; any missing pinned frame yields
   `evidence_completeness=failed`.
4. Ordinary pressure loss is separate from deduplication and yields `partial` only when pinned
   evidence remains complete.
5. Default 300 ms cadence and every override are explicit and ledger-visible.
6. Success, failure, and cancelled runs produce verifiable archives with independent task outcome
   and evidence completeness.
7. Every manifest artifact hash, ledger range, screenshot counter, pinned reason, terminal
   receipt, normalized output path, and ZIP hash is independently verified.
8. Corrupt input, collision, write/sync/rename failure, missing pinned artifact, or required ledger
   append failure fails loudly and cannot produce a false completed event or published ZIP.
9. Production packages do not depend on Lab, and artifact-store does not depend on Runtime host,
   scheduler, Lab, resource tooling, or device backend factories.
10. Full workspace, non-Lab, formatting, Clippy, architecture, and all-features gates pass.
