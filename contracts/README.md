# ActingCommand Runtime Contracts

These files are versioned data and protocol contracts between the runtime decision/data core, UI, and execution layer.

## Files

- `runtime-api.openapi.yaml` — local HTTP API reserved for the UI.
- `runtime-events.schema.json` — WebSocket event envelope and payload schema.
- `global-ledger-query.md` — shared query predicates, indexes, bounded watch and read-only event pages.
- `task-flow.schema.json` — declarative task-flow schema.
- `ocr-fields.md` — operation 0.8 typed post-admission fields and task-run projection.
- `resource-readings.md` — the `task.json` family `resource_readings` (reading marks): declaration and version gate, the kernel reading on the frames a run reaches the marked pages on, joined before `Finalizing`, its failure codes and the recognition maintenance package (Workflow #384 D2-S2); the family `resource_adjustments`, an operation's resource changes read live after it is confirmed (Workflow #384 D2-S3).
- `task-diagnostic-stream.md` — task-owned raw evaluation records, verified streaming and bounded read-only export.
- `device-diagnostic-budget.md` — owner-epoch Shadow diagnostics, source references and bounded close summaries.
- `nemu-owned-resource-close.md` — fenced connection retirement and bounded completion of Runtime-owned Nemu resources.
- `read-session-resource-close.md` — Scheduler-authorized retirement of retained observation sessions.
- `sqlite/schema.sql` — authoritative versioned Runtime state, migration, and release-set schema.
- `runtime-project-interface.md` — versioned read-only project projection and compatibility matrix.
- `ledger-performance-export.md` — read-only, raw-event-paginated stutter and clock-jump export.
- `performance-timing-signals.md` — microsecond `touch_response_us` / `capture_acquire_us` payload fields, their context maxima and the host-level summary fold.
- `performance-control.md` — the per-instance performance-control directive and how policy dispatch admission and new Business leases consume it.
- `page-projection.md` — package annotations and the shared bounded single-frame page projection.
- `runtime-fact-store.md` — the Runtime's own fact records (disjoint from instance facts), the memory-only store, and ledger-only persistence.
- `selection-policy.md` — the declared selection document, its canonical identity, and the pure evaluator's semantics.
- `candidate-projection.md` — `actingcommand.candidate-projection.v1`: one layout's candidate set on one frame, its IDs, hash, budgets and public/controlled forms.
- `color-digest.md` — `color_digest.v1`: the integer color-layout digest of one declared rectangle, its golden values and error codes, and its package declaration (task source entry, derived recognition pack `0.7` `color_digest` target, admission pointers).
- `selection-graph.md` — the #308 selection graph; section Checks: named composite checks (recognition pack `0.7` `composite` targets), their evaluation and where they may be used; section Candidate layouts: recognition pack `0.7` `candidate_layouts`, their rules and feature values; section Records: the `task.selection_evaluated` ledger record and its evaluator mirror.
- `resource-targets.md` — the instance resource target policy: the frozen v1 and the valuation-aware v2 documents, their rejection reasons, the `ApplyResourceTargets` entry and receipts, and their storage as the instance fact `session.resource_targets` (the evaluation is specified in `resource-targets.md`, section "Evaluation", with "Resource weights (v2)").
- `server-keys.md` — persisted server variant key policy.
- `actinglab-dry-run.md` — the global actinglab `--dry-run`: each command's declared mode (preview, refused, no_effect, mixed), the `dry_run_unsupported` refusal, the preview shapes and the registered bootstrap and transient writes.
- `primitive-service.md` — language-neutral execution-layer boundary for Rust or other worker implementations.
- `actingd-check-config.md` — side-effect-free `actingd check-config` configuration validation and its result schema.
- `runtime-config.md` — the shared configuration mechanism (Workflow #387 D1b): the crate `runtime-config`, the file and instruction layers, the defaults file `actingd.defaults.json`, the key table, the facts `config.parameters.<section>`, `config.selection` and `host.config_overrides`, the `ConfigureSettings` instruction with its checks and records, the restore of instruction values at start (the later change wins), and its codes.
- `component-interfaces.md` — the `interfaces` declaration in the Runtime, Tools and UI build manifests and in resource bundles: the interface vocabulary and revisions with their code anchors, what a range means, the containment and negotiation checks, the bump rule, the known table for undeclared v0.11.0–v0.11.2 releases, what Runtime v0.11.3 moved, the checks acsetup makes per operation and why a Runtime rollback from v0.11.3 to v0.11.2 is refused (Workflow #364).
- `actingd-unlock-owner.md` — offline `actingd unlock-owner`: the operator's confirmation appended as the retained owner epoch's close evidence, the `owner.unlock` fact and its result schema.
- `owner-journal.md` — the `owner.lock` owner journal: records, the reader's derived state, the in-epoch checkpoint fold, its side-file crash recovery and error codes.
- `runtime-watchdog.md` — `actingctl watchdog status` / `run-once` / `install` / `uninstall`: the per-minute tick that starts an A/B installation's Runtime again after an end without a formal close, its decision table, start methods, budget, gap grace, the launcher, the per-user scheduled task, `<root>\watchdog\` files and exit codes.
- `device-binding.md` — an instance's device kind (`emulator`, `handset`), the private binding file `<state root>\private\device-bindings.json` that alone holds a handset's serial, the handset's label and fingerprint in the records, and the unbound handset (Workflow #398 RD-1b).
- `emulator-control.md` — explicit emulator instance start/stop/restart: origin gate, per-instance fence, close-before-stop, tool timeouts and wait criteria, event shape, the `device.connected` program fact and `actingctl emulator`.
- `scheduling-pause.md` — operator scheduling pause / resume: origin gate, the dispatch gate and its `Deferred` presentation, the instance drain with `contained_task_paused`, status fields, persistence across a restart (the runtime fact `host.scheduling_pause`, restored at the next start; Workflow #361 B1) with no expiry, and `actingctl pause` / `resume`.
- `linear-steps.md` — the `linear_steps` execution mode: its control declaration and the operation `transition` and `optional` fields, admission rules, step-by-step execution with intermediate states, optional steps with their settle and retries for a swallowed input, failure codes and the mapping onto existing ledger records.
- `policy-suspension.md` — the failure identity a scheduled `linear_steps` or (Workflow #390 EXEC-3) page-graph task's settlement records as its `failure_identity` (beside the failure's `outcome`), which failures accumulate toward a pause, the immediate rerun, lifting a pause by a package update, and the read-only `actingd suspended` report; and, specified ahead of their code (Workflow #386), the suspension of a task package that admission refuses, condition terminations with their owed reruns, and the `scheduling.rerun.*` / `pack_admission.*` keys.
- `scheduling-blackout.md` — scheduling blackouts (catalog `maintenance` events and host entries) and complete silence: which requesters wait, which are dropped and which pass, gradual quiet before a blackout, the safety net at its start, the order of the scheduling deferrals, the `scheduling.blackout.*` keys and codes (Workflow #386, ahead of slices SR-4a and SR-4b).
- `cost-commit.md` — the `task.json` member `cost_commit` (entry-cost tasks): declaration, the task-to-entry-step table, ranking, the pacing-stage gate, the time check before the entry step and the `scheduling.cost_commit.*` keys (Workflow #386, ahead of slice SR-10).
- `lab-recording.md` — the Lab recording (`actinglab record start` / `mark` / `status` / `stop` and `--record` on the device commands): marks and their self-test, steps, intermediate states, application and optional steps, the recording lock, and the `linear_steps` package `record stop` generates, checks and writes as a content container (`package-reference.md`, "Containers") with its binding snippets.
- `application-lifecycle.md` — the `application` effect of task packages (launch / restart / stop of the instance's assigned application), the foreground gate before every pointer input with the `application.foreground` program fact, and the startup package the host schedules after emulator `start` / `restart`.
- `ledger-observer.md` — the ledger as an observer (Workflow #383, target of slices AO-1 to AO-9): the two memory stores, the capture interface and the four host calls with their architecture guard, the silent gates G1–G16, the direct-write exception table (W1, W2), the capture queue with three watermark tiers and spilling, flush order and revisioned sources, write failure (retry, dump, fatal, import) and the dump file, gap records, tags and the `ledger_index` table, start and shutdown order, probes and status blocks, the `ledger_observer.*` keys, the observer's outcome codes and the call-site inventory.
- `v012-register.md` — the v0.12.0 register (Workflow #383 AO-0): every new persisted shape of ledger revision 3 with its code and writer slices, the removals of task-package digests (option 丙) and failure identity v2, the wire shapes handed to AO-0, the new fact keys and their tag class, every key of `actingd-config` revision 5 with its parsing slice, and the row capacity of `config.parameters`.
- `runtime-interface.md` — the unified interface (Workflow #389): one door, the three counters (`cli`, `mcp`, `ui`) of the general interface and the Lab channel; channel binding (`BindChannel`, `via`, the transition until U-6); the block of every request kind and the instance fact key families; the two Lab options (`lab_module_authoring`, `lab_module_debugging`) and the gate's decision; G14's `channel` and `via`; the clean mapping of actingctl, MCP, the console and actinglab; the upgrade state; pack load and unload; the outcome codes; the shapes handed to #383 AO-0, the wording handed to #386 SR-0 and the installer page handed to the UI.
- `runtime-outcomes.md` — the `actingcommand.outcome.v1` envelope (one code and category per result, typed values, cause links, codes mode), the surfaces and the slices that adopt it, the outcome catalog (`outcome-codes/catalog.json`, the owner fragments, the generated `outcome-codes.json`, release snapshots) and the drift guard `tools/outcome-guard` (Workflow #378).
- `scheduling/` — frozen four-document scheduling catalog, diagnostics, canonical hash contract, and neutral examples.
- `test-gates/` — the gate map (test plan v2.2 §2.1): exactly six fragments, one per test inventory, `contract.toml`, `ledger.toml`, `host.toml`, `policy.toml`, `exec.toml` and `apps.toml`; each `[[gate]]` row has exactly four keys: `key` (its inventory's own id as a string: `K1`…, `R1`…, `O1`…, `A1`… in `contract.toml`, plain numbers such as `"31"` in the other five; one row per key), `coverage` (`"Y"`, or `"P"` only for "tested only on the cfg(test) Segment backend", never for "partial"), `tests` (a non-empty list of `<package>::<target>::<libtest path>`, target `lib`, `bin:<name>` or the integration test, each a live test without `#[ignore]`) and `incident` (a list of strings, `[]` when none). A test goes into the fragment of the inventory whose key it covers. A slice appends its tests to the existing row of a key and leaves its coverage unchanged; it creates a row only for a key without one, with `"Y"` when its tests cover that key's whole requirement, and otherwise lists the key in its report; a key no inventory id fits continues that inventory's numbering (say so in the PR body). Every row a slice creates or appends to gets that slice's own incident label once (lists are append-only, no duplicates). A row leaves only when its feature retires, said in the PR body. `gate_map_names_live_tests` and `gate_tests_are_deterministic` (`tools/actinglab-architecture/tests/gate_map.rs`) and `ci/gate_check.py` (check `rust`) enforce it.

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
retain their requested backends, configured ADB, the configured serial's label,
resolved target, timeouts and explicit MuMu path inputs. Providers without a
registered device configuration carry an explicit absent device observation;
task timing remains recorded. The production registry supplies its parsed
configuration directly.

Device serials (Workflow #398 RD-1a): a target is named by its label, never by a
device serial. A target addressed by `HOST:PORT` keeps `HOST:PORT`; a target
addressed by an adb serial is `handset:<alias>`. The serial members of the
effective configuration (`configured_serial`, `resolved_serial`,
`selected_serial`, the input selection's `serial`) and the capture geometry's ADB
source carry the label.

The task interpreter and the record share the same effective timing view:
task/control timeout declarations, control/default task and step budgets,
capture interval, and each operation's explicit expect-after, timeout, interval
and postdelay source. Request response timeout and the Host's existing absolute
deadline are separate; Host remaining time is measured at the stated monotonic
observation time. Recording does not move any timer's start or polling boundary.

The first successful capture adds the same Frame producer's selected backend,
resolved ADB and target label and resolved MuMu installation root, ADB path,
capture DLL path and installation source. Ordinary Frame constructors leave the
selection unobserved. The first InputCommitted adds the successful input
backend's name and selected target label through the existing session and
critical-action return.
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
`1..=1800000` milliseconds. Package build, Runtime admission and Lab validation
share this bound. Omission retains the existing step-timeout fallback. The
Runtime control step timeout remains at most 60000 ms; post-input delay and
postcondition interval remain at most 5000 ms.

After the declared input, postcondition polling only captures and recognizes.
Each iteration includes capture/recognition cost and a bounded interval. A
matching page may complete the local polling budget, but every poll is also
bounded by the original task deadline. The deadline is checked before and after
capture/recognition and immediately before input; sleeps are clipped to its
remaining time. Existing request/lease cancellation and fatal boundaries remain.
The wait does not reset those budgets or change retry, recovery, input, lease,
heartbeat or resource-close behavior.

Operation `0.9` declares non-OCR task budgets directly: optional `timeout_ms`
defaults to 60000 and accepts `1..=1800000`; optional `max_steps` retains the
existing 100-step default and 1000-step maximum. Its package control is
`Lab-1y.control.v2`. The task RPC response ceiling is 1800000 ms and the official
task-run client and scheduled procedure binding select that bounded ceiling. Resources explicitly choose a
long budget; all phases share the original monotonic task timer. An in-flight
capture/provider call retains its existing bounded completion and cancellation
checks; elapsed time never permits a further input.

An optional `phases` array defines 1–32 linear stages, for example:

```json
[
  {"id":"depart","operations":["open_panel"],"target_pages":["panel"]},
  {"id":"business","operations":["submit"],"target_pages":["result"]},
  {"id":"return","operations":["back_to_panel","back_home"],"target_pages":["home"]}
]
```

Each stage selects only its listed existing protected operations, in the task's
operation order. Its cursor advances only after a successful operation confirms
a declared stage target; observing the same page at stage entry does not advance
or finish the task. The final stage targets equal the task targets. A phased
task declares its entry page and scheduling outcome; it uses `navigable_route`
with confirmation enabled and has no internal recovery or stability termination.
Static reachability follows `(phase, page, designated effect)` through finite
postconditions. The designated operation is nonretryable and cannot be reachable
again after its effect. The members of a designated effect set (Workflow #390
OUT-2, [resource-declarations.md](resource-declarations.md)) are nonretryable too;
a member may be reached again up to its cap, and where it may have reached its cap
the stage's next operation from the page is followed as well. So is the next
candidate after one that declares `when` or `on_guard_refused` (Workflow #390
EXEC-1, [resource-declarations.md](resource-declarations.md), "Task schema `0.9`
candidate operations"); a stage page left without a candidate after such
candidates only ends that path. Source parsing,
build and Runtime admission share this check; the operation and control phase
declarations must match exactly.

Shared interrupt pages (Workflow #373 NAV-6). The game root of an installed
standard package may name shared interrupt pages (`interrupts.json`, see
[package-reference.md](package-reference.md), "Standard package root"). A run looks
at them only on a frame its own package did not accept: the verdict of its entry
check (the frame on which the entry's window ended unmatched), and each frame of a
wait that did not show what the package awaited (a page-graph frame no page of the
package matched; a linear step's entry, transition, arrival or retry-decision page;
a page-graph operation's destination). The hooked kernel loops are
`recognize_required_entry`, `recognize_linear_entry`, `await_linear_entry`,
`await_ordinary_home_entry` (entry verdicts) and `capture_until_page`,
`await_postcondition`, `linear_wait`, `linear_wait_set` (waits, per frame). An
interrupt page the running package declares itself is the package's own. One
matched page is handled by its handler package as a layer of the run, and the wait
resumes with its budget paused for the handling; an entry check starts again.
Without `interrupts.json` for the game nothing is read and nothing changes.

Entry page sets (Workflow #373 NAV-3b). In operation `0.9` a page-graph task
(`navigable_route`, `in_page_guard`, `recognize_only`) may declare `entry_page` as
an array of 1–16 distinct pages instead of one page; `any` is never a member and
each member must resolve to exactly one page of the package. A `linear_steps` task
keeps one entry page, and `"any"` keeps its meaning (no entry check; never with
phases). A set is always checked before the run: one frame evaluates only the set's
pages; exactly one match starts the run, none goes to entry recovery, and more than
one fails with `contained_task_recognition_conflict` before any input. An entry
recovery package must end on a page of the set, and the recheck accepts any page
of it. In a phased task each page of the set starts the one stage that lists an
operation whose `from` is that page, and the run enters at the stage of the page it
matched; static reachability is checked from every such start. A single entry page
still starts the first stage. Admission refuses a set with
`contained_task_entry_pages_invalid` and its `entry_problem` (`empty`, `too_many`,
`duplicate`, `any_in_set`, `unresolved`, `phase_start_missing`,
`phase_start_ambiguous`). The `EntryRecognition` record carries `required_pages`
and, when one matched, `matched_page`.

Phases retain one Task/Run/lease, cumulative steps and the original deadline.
Existing StepStarted/StepFinished ledger facts carry optional `phase` evidence
(`index`, `id`, `completed`); a completed StepFinished is committed before the
cursor advances. Ledger failure remains fatal. Cancellation/failure preserves
the actual effect and does not restart a phase or submit another task. Ordinary
declarations without phases retain their page-driven semantics. Consumers that
cannot interpret 0.9/control.v2 reject them explicitly; Lab deep validation uses
Runtime admission for this contract.

A `Lab-1y.control.v2` control may instead declare `execution_mode` `linear_steps`
([Linear steps](linear-steps.md)): the task's operations form one chain from its
entry page to its target page, run step by step without phases, without page-graph
operation selection and without global page recognition. Each wait evaluates only
the page it waits for; an operation may declare a `transition` (a recognizable
intermediate page or a time window), and an operation after the first may be
`optional` (Workflow #339): its page may not appear, and the run then skips it, with
a settle that watches the next page for a late one. The run writes only existing
ledger records; builds without the mode refuse such packages before
`PackageAdmitted`, and builds without optional steps refuse an `optional` field.
A Lab recording generates such packages ([Lab recording](lab-recording.md)); a
scheduled linear task's failure is rerun at once, the same problem again pauses it,
and a package update lifts the pause ([Policy suspension](policy-suspension.md)).

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
A `release_id` is the SHA-256 of the frozen release-set v2 identity encoding, computed the same
way on write and on read (Workflow #191 B1-S1): an additive field never changes a stored id, and
a field that must join the identity needs a new release-set schema version.

Runtime-domain crates (`crates/*`, `providers/*`) do not write to stderr: diagnostics go to the
ledger as facts and artifacts, and failures return as explicit error values. The process shells
under `apps/*` are the only exception; they may print a FATAL or usage line before a non-zero
exit. The architecture guard `runtime_domain_never_writes_stderr` enforces this with an explicit
allow-list that a new site must join in the same change that introduces it.

## Historical Go boundary

The historical Go interfaces were moved to:

- https://github.com/HS7097/ActingCommand-Legacy-Runtime

The UI must use the runtime API and must not own the runtime lifecycle. The execution layer must return structured observations and image references, not raw frame buffers.

Provider construction and read-only checker semantics are defined in
[Provider startup facts](provider-startup.md).

Status provenance and monitor recovery are defined in
[Runtime state observations](runtime-state-observation.md).
