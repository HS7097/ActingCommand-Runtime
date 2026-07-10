# ActingLab A7 Interface Amendment

Status: approved and frozen for issue #33 A7 onward

Freeze algorithm: normalize the file to LF, then hash the UTF-8 bytes strictly between the freeze marker lines, excluding the marker lines and including the payload's final LF.

Frozen payload SHA-256: `eb753f19c03cd71bafdc50ac9847800c070713b148b07fccd7f9b335be7264e0`

<!-- A7-INTERFACE-FREEZE-BEGIN -->
## Approval And Scope

Alice approved this A7 amendment on 2026-07-10 after the second A7 review. It amends only the A2a ledger sink shape in `docs/architecture/actinglab-a2a-interface-freeze.md` section 6.3 and records the selected-device seam required to preserve the pre-extraction Lab run order. All other A2a decisions remain frozen.

## Reason

The original two-method `LedgerSink` covered semantic `append_drive` and `finish` operations, but no migrated Lab use case called those methods. The production adapter therefore rejected every call deliberately. A7 also needs a run-scoped ledger lifecycle for archive projection and readback. Exposing `actingcommand_ledger` records, events, readbacks, headers, and last-resort errors on that extension made the application interface mirror the storage implementation.

This amendment removes the unused semantic methods instead of preserving always-error production stubs. Existing semantic command journaling remains the explicit command-scoped `SemanticLedgerContext` flow owned by the ActingLab adapter. It is a context object, not a second ledger trait. A later semantic-ledger ownership migration requires another explicit amendment rather than another parallel trait.

## Revised Single LedgerSink

`LedgerSink` remains the only Lab ledger trait. Its amended shape is:

```rust
pub trait LedgerSink {
    type RunSession;

    fn run_session(&mut self) -> Self::RunSession;
    fn start_run_session(
        session: &mut Self::RunSession,
        request: RunLedgerSessionRequest,
    ) -> LabResult<PathBuf>;
    fn append_run_record(
        session: &mut Self::RunSession,
        record: LedgerRecordEntry,
    ) -> LabResult<()>;
    fn append_run_event(
        session: &mut Self::RunSession,
        event: LedgerEventEntry,
    ) -> LabResult<()>;
    fn sync_run_session(session: &Self::RunSession) -> LabResult<()>;
    fn read_run_session(session: &Self::RunSession) -> LabResult<LedgerReadback>;
    fn write_run_last_resort(
        run_root: Option<&Path>,
        error: &LedgerLastResort,
    ) -> LabResult<PathBuf>;
}
```

`RunLedgerSessionRequest`, `LedgerSessionHeader`, `LedgerRecordEntry`, `LedgerEventEntry`, `LedgerReadback`, and `LedgerLastResort` are Lab-owned opaque types. Their storage fields are private. Public signatures do not name `LabLedger`, `SessionHeader`, `LedgerRecord`, `LightEvent`, `LedgerRead`, `LastResortError`, `LabLogError`, `LabLogResult`, or `serde_json::Value`. The typed record/event/header wrappers may cross the adapter seam through validated encoded JSON, but callers cannot substitute one wrapper kind for another.

The production ActingLab adapter owns concrete `LabLedger` creation, append, sync, readback, and last-resort storage. Lab owns run lifecycle policy, record/event construction, archive projection, and ordering. Sealed Lab tests may use a test adapter backed by `LabLedger`; production Lab code may not construct or store one.

## Selected Device Resolution

After containment has produced and recorded control/resources, Lab selects exactly one instance ID. The request-owned resolver validates only that selected instance and returns one complete `LabRunSelectedDevice` containing serial, global ADB provenance, capture configuration, and touch configuration.

The ActingLab adapter preserves the pre-extraction selected validation order: resolve the selected target, validate capture choice, validate touch choice, resolve the selected instance ADB path and enforce the ADB/path target policy, then resolve global ADB provenance. The resolver must finish all of those checks before Lab assigns `ctx.instance`, starts the normal run ledger, or acquires the lease. No unselected instance configuration is opened or validated.

Capture configuration is already validated when the complete selected device is returned, but `CaptureBackendFactory::open` remains after lease acquisition. Touch configuration is also already validated, including for recognize-only runs, but `InputBackendFactory::open` remains deferred until the first actual input action and reuses the validated configuration.

A selected-configuration failure keeps pre-extraction precedence: it produces the normal failure archive and the failure-only `unknown` ledger shard, without a lease-acquired event or backend open. Failure finalization may create that failure ledger after the selected validation error; the resolver itself must observe no prior ledger start or lease.

## Frozen Invariants

1. Lab defines exactly one ledger trait: `LedgerSink`.
2. Production `crates/lab` does not own or construct concrete `LabLedger` storage.
3. The public Lab ledger interface exposes only Lab-owned opaque DTOs and common `LabResult`, path, and scalar types.
4. Ledger storage and conversion failures remain explicit; no empty, skipped, or fake-success fallback is allowed.
5. Existing run event, dispatch, drive/finalizing, output, and terminal-receipt ordering is unchanged.
6. Existing archive contents, wire fields, exit mapping, cleanup, and all 30 A1 goldens remain unchanged.
7. Semantic command journaling remains app-owned through `SemanticLedgerContext`; it does not create another ledger trait or duplicate run writes.
8. Exactly one selected device resolves after contained resources; unselected configurations stay untouched.
9. Complete selected configuration validation precedes context instance assignment, normal ledger creation, and lease acquisition.
10. Capture opens after lease; touch opens only on first actual input.
11. Issue #26 G2 self-hash and G3 semantic routing remain unchanged.
<!-- A7-INTERFACE-FREEZE-END -->
