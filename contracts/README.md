# ActingCommand Runtime Contracts

These files are versioned data and protocol contracts between the runtime decision/data core, UI, and execution layer.

## Files

- `runtime-api.openapi.yaml` — local HTTP API reserved for the UI.
- `runtime-events.schema.json` — WebSocket event envelope and payload schema.
- `task-flow.schema.json` — declarative task-flow schema.
- `ocr-fields.md` — operation 0.8 typed post-admission fields and task-run projection.
- `task-diagnostic-stream.md` — task-owned raw evaluation records, verified streaming and bounded read-only export.
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
SHA-256 against the write calculation. `ArtifactStoreIssuer::issue` accepts
actual bytes or calculated opaque material through `ArtifactIssueInput`, and
remains the sole store attachment issuance entry. Material is calculated from
bytes, never a caller-declared final hash. The store then issues the
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

Each physical `input.intent` includes optional typed `provenance` with
`input_action`, `source_step_action_id` and `before_frame_id`. The input action
uses the same persistence sanitization as the task effect declaration: Key and
Text values become `[redacted]`; coordinates, durations and the existing input
execution plan remain exact. The backend receives the original action. The
Host issues the physical action ID in the intent's event links before executing
the action, so provenance survives input failure or a later cleanup failure.

Contained Lab operations supply the FrameId from their verified before frame
to this same input context. They retain existing optional task/run links and
omit a task-step reference when none exists.

Contained task effect intents link their actual preceding frame and supply
their step identity to that physical intent. The first attempted capture after
a successful input links its requested/completed or failed events, and any
CaptureStore frame artifacts, to the returned physical action ID. A failed or
unperformed capture never supplies a successful PNG. These references reuse
existing events and IDs; they do not change input/capture counts, task/run
identity, cleanup, fencing or preemption.

`actingledger --state-root <path> export --task-evidence [--after N]
[--through N] [--limit N]` emits a structured read-only task evidence report.
It uses the existing event-page limit of 1024 and starts after sequence zero by
default. `page.events` preserves native step/index/operation declarations,
physical outcomes, terminal self-reported counts, and CaptureSummary pins and
completeness. `steps` and `inputs` add explicit ID relations with source
sequences, safe original input parameters, before/after FrameIds and verified
PNG references/hashes. Associations require matching request, correlation,
instance, lease, task and run identity and valid lifecycle order. Multiple
competing single-source records remain ambiguous; no nearest-event selection
fills an absent edge.

Relations outside a bounded page are `outside_window_or_missing`, and the
window remains incomplete without declaring those sources absent. Legacy
intents without provenance remain readable as `not_recorded`. Missing sources
in a complete window, identity conflicts, inconsistent links, a corrupt tail
or failed artifact verification are returned as structured gaps; the CLI prints
that partial report and exits nonzero. Pagination alone preserves a successful
exit and an incomplete window. Bare export and performance/stability modes
retain their behavior, including effective-configuration expansion. This mode
writes neither ledger state nor artifacts and does not infer successful input
or capture from the task terminal's outcome or step count.

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
