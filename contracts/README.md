# ActingCommand Runtime Contracts

These files are versioned data and protocol contracts between the runtime decision/data core, UI, and execution layer.

## Files

- `runtime-api.openapi.yaml` — local HTTP API reserved for the UI.
- `runtime-events.schema.json` — WebSocket event envelope and payload schema.
- `task-flow.schema.json` — declarative task-flow schema.
- `ocr-fields.md` — operation 0.8 typed post-admission fields and task-run projection.
- `sqlite/schema.sql` — authoritative versioned Runtime state, migration, and release-set schema.
- `runtime-project-interface.md` — versioned read-only project projection and compatibility matrix.
- `ledger-performance-export.md` — read-only, raw-event-paginated stutter and clock-jump export.
- `page-projection.md` — package annotations and the shared bounded single-frame page projection.
- `server-keys.md` — persisted server variant key policy.
- `primitive-service.md` — language-neutral execution-layer boundary for Rust or other worker implementations.
- `scheduling/` — frozen four-document scheduling catalog, diagnostics, canonical hash contract, and neutral examples.

## Rust mainline boundary

ArtifactStore supports a task-owned `begin_stream` / `seal_stream` lifecycle.
The stream implements standard `Write` and accepts successive byte chunks;
neither opening nor appending publishes an artifact or a ledger event. The
store tracks the actual bytes written. Sealing synchronizes the staging file,
recomputes its material with a fixed 64 KiB read buffer, and compares length and
SHA-256 against the write calculation. The contract's opaque material API
accepts bytes, never a caller-declared final hash. The store then issues the
ordinary artifact identity, derives its object key, and performs its existing
no-overwrite atomic rename and `ArtifactCreated` → verification →
`ArtifactVerified` publication under the same writer mutex. Both required event
failures retain their existing published-file cleanup and fatal return.

The staging stream carries no verified reference. Its owner must consume it
with `seal_stream` or `abort` before normal task termination, including a
normally terminable failure or cancellation. Write and seal errors clean up
the staging file and propagate the original failure with any cleanup error;
an errored stream cannot seal successfully. Explicit abort also reports cleanup
failure. Abrupt process termination can leave an unpublished partial file;
that file has no diagnostic authority or recovery mechanism. Publication facts
remain owned by GlobalLedger. Memory for store IO is bounded independently of
total artifact length, and byte-count overflow and IO failure are explicit.
Record encoding and record limits belong to the producer's schema; the byte
stream does not sample, truncate, or interpret records.

`open_projected_stream` opens an existing ledger-referenced artifact for
bounded sequential reads without a writer lock or filesystem mutation. Bytes
read before `finish` are provisional. `finish` reads any remaining bytes and
returns a verified reference only after EOF, actual length, and the complete
SHA-256 match the published reference. Reading a prefix or a record page alone
does not establish integrity. Consumers enforce the reference's privacy policy
and must complete verification before presenting the page as verified. Recovery
and read-only reference verification also use bounded material calculation;
existing byte-vector read interfaces retain their behavior.

Each formal contained task records its effective configuration before entry
preflight as `actingcommand.runtime.effective-task-configuration.v1` in a
GlobalLedger-linked DiagnosticJson artifact. Registered device configurations
retain their requested backends, configured ADB and serial, resolved target,
timeouts and explicit MuMu path inputs. Providers without a registered device
configuration carry an explicit absent device observation; task timing remains
recorded. The production registry supplies its parsed configuration directly.

The task interpreter and the record share the same effective timing view:
task/control timeout declarations, control/default task and step budgets,
capture interval, and each operation's explicit expect-after, timeout, interval
and postdelay source. Request response timeout and the Host's existing absolute
deadline are separate; Host remaining time is measured at the stated monotonic
observation time. Recording does not move any timer's start or polling boundary.

The first successful capture adds the same Frame producer's selected backend,
resolved ADB/serial and resolved MuMu installation root, ADB path, capture DLL
path and installation source. Ordinary Frame constructors leave the selection
unobserved. The first InputCommitted adds the successful input backend's name
and selected serial through the existing session and critical-action return.
Configured values do not substitute for these actual observations. A bound
entry recovery records its own timing and package identity when used. The task
emits at most four configuration artifacts, each at most 1 MiB; encoding, size
and persistence failures propagate explicitly. No successful context is stored
in a separate cache. Close, failure cleanup, error escalation, lease/fencing
and preemption ordering retain their existing owners and boundaries.

Plain `actingledger --state-root <root> export` expands verified configuration
artifacts in its existing bounded event page. Rows include the source sequence,
artifact reference/hash and typed values, with request/task/run/frame/action
links checked against the ledger. Capture observations also require the prior
verified frame artifact; input observations require the actual InputCommitted.
The leaf reads through `artifact-store::read_projected_verified`; corrupt,
missing, pending-redaction or mismatched evidence fails visibly. Configuration
artifacts retain the existing Internal sensitivity and controlled artifact
references; shared projections retain their existing redaction rules. The
records have no credential, secret, token or salt fields.

Explicit operation `expect_after.timeout_ms` is a polling budget in
`1..=600000` milliseconds. Package build, Runtime admission and Lab validation
share this bound. Omission retains the existing step-timeout fallback. The
Runtime control step timeout remains at most 60000 ms; post-input delay and
postcondition interval remain at most 5000 ms.

After the declared input, postcondition polling only captures and recognizes.
Each iteration includes capture/recognition cost and a bounded interval. A
matching page is checked before polling expiry, so the budget is not a strict
wall-clock deadline. Existing request/lease cancellation and cumulative task
timeout checks retain their precedence at their existing execution boundaries.
The wait does not reset those budgets or change retry, recovery, input, lease,
heartbeat or resource-close behavior.

The Rust mainline contract crate lives in:

- `crates/actingcommand-contract`

The Rust device-layer crate lives in:

- `crates/device`

The Rust scheduling policy contract lives in:

- `crates/policy`

The sole mutable-state owner and executable SQLite schema live in:

- `crates/runtime-state`

Runtime reports, ledgers, mutable state, and release pointers live under the Runtime state root;
they are never written into resource repositories. Release sets pair Runtime, UI, and external
resource versions as immutable generations before one atomic pointer transition.

## Historical Go boundary

The historical Go interfaces were moved to:

- https://github.com/HS7097/ActingCommand-Legacy-Runtime

The UI must use the runtime API and must not own the runtime lifecycle. The execution layer must return structured observations and image references, not raw frame buffers.
