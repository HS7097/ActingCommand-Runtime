# Runtime interface

Workflow #389 (unified interface). One door takes every request to actingd. Behind it
are the general interface, which normal operation needs and which three counters (CLI,
MCP, UI) reach, and the Lab, a debugging module that is not installed by default and
has two independent options. This document is the contract that slices U-1 to U-6
implement; a behaviour described here exists in the Runtime only from the slice named
for it.

Source: the frozen model v2.2.3 (Workflow `rd-1009/perm/model-unified-interface.md`),
with its §14.2 correction (writing resource targets and instance facts is general) and
its §14.3 correction (Workflow #336, option 丙, M-PKG v1.1 §5.1: task packages have no
content digest). Where the model's body and §14.2 differ, this document follows §14.2.
Alice decided M-PKG D10 on 2026-10-12: an unload stops the manual runs of the same
`package_id`.

| Slice | Release | What it brings |
|---|---|---|
| U-1 | v0.12.0 | the classification `RuntimeOperation::interface_class()` and `fact_key_family()`; the `LabOption` enum; the wire shapes of the new request kinds; the owner-journal reader accepts the upgrade-state marker (`owner-journal.md`). No behaviour changes |
| U-2 | v0.12.0 | channel binding that marks and never refuses; the two Lab options read and reported, never enforced; actinglab's envelopes become Lab; actingd's own two connections become in-process calls |
| U-3 | v0.12.x | Lab commands and the clean mapping; pack load and unload; MCP tiers retired |
| U-4 | v0.12.x, same Runtime release as U-3 | the upgrade state; the watchdog row; install-control 2 |
| U-5 | next UI release | the installer's Lab page and its transaction; acsetup speaks install-control 2 |
| U-6 | first paired Runtime and UI release, after #383 AO-4 | the gate closes: refusals by channel and by option; the origin gates leave the contract |

## Words

- **Door**: actingd's request entry (`crates/runtime-host/src/host/requests.rs`), one
  loopback TCP pipe for every client.
- **Counter**: one of the three entries of the general interface: `cli` (actingctl,
  and acsetup and scripts through it), `mcp` (`actingctl mcp-serve`), `ui` (acui).
- **Lab channel**: the channel `lab`, bound by actinglab and by whatever forwards to it.
- **Channel** (`RuntimeChannel`): the identity a connection binds once: `cli`, `mcp`,
  `ui` or `lab`.
- **via** (`ChannelVia`): on the Lab channel only, the counter that forwarded the work
  to actinglab: `mcp` or `ui`.
- **Option one, authoring** (`lab_module_authoring`) and **option two, debugging**
  (`lab_module_debugging`): the two Lab options (`LabOption`: `authoring`,
  `debugging`; section Lab options).
- **Block**: where a request kind belongs: handshake, general, option one, option two,
  retired or compat.
- **Class**: read, instruction or operation. It explains a kind; only the block
  decides whether the gate takes it.
- **G14**: the ledger's request write boundary (#383): every write request is copied as
  `command.received`, `command.validated` or `command.rejected`.

## The gate

- Position: in `process_request_observed`, after `request.validate()` and before
  `begin_request`.
- Duties, nothing else:
  1. know the connection's channel;
  2. look up the request kind's block (section Blocks), and the two Lab options, and
     decide whether this channel takes it (section Lab options, The gate's decision);
  3. tag the request with its channel (and `via`) and hand it to its existing handler.
- The channel decision is made at the gate only, never in `RuntimeRequest::validate()`.
  The host builds `(Agent, Adapter)` lease requests itself and validates them
  (`policy_dispatch.rs`, `events.rs`); internal requests do not pass the gate.
  `validate()` keeps the shape and content checks only; the origin gates now in it are
  deleted at U-6.
- Everything behind the gate stays as it is, including the C4 source check and frame
  pinning.
- Not requests, never at the gate, whether or not the Lab is installed: the Runtime's
  own policy dispatch, recovery ladder (emulator restart included), startup packages,
  emulator autostart, monitor capture and recovery, performance pacing, host shutdown
  handling, the upgrade state's own drain and close, and the stopping of runs when a
  package is unloaded.

## Channels

### Binding

1. A connection's identity is the counter or the Lab channel it came through. There
   are four values: `cli`, `mcp`, `ui`, `lab`. Nothing records a person, an agent or a
   script, and there is no "undeclared" value.
2. A connection binds once, at open: the handshake is `Health` (owner epoch check)
   followed by `BindChannel{channel, via?}`. Every request on the connection then
   carries that channel.
   - The runtime client opens TCP connections in three places: `connect`,
     `reconnect_in_owner_epoch` (the fallback of `send_or_dropped`) and
     `recover_contained_task_timeout`. All three call one internal handshake function,
     so a reconnect keeps the channel.
3. The channel is never inferred from a request: neither from its envelope (actor,
   source) nor from its kind. A `(Lab, Lab)` envelope on a `cli` connection is a `cli`
   request.
4. A second `BindChannel` on one connection is refused with `channel_binding_exists`.
5. From U-6, a connection that has not bound is refused everything but the handshake
   with `channel_binding_missing`, in the receipt only, never in the ledger.
6. Each frontend hard-codes its channel, and its user cannot change it: actingctl `cli`;
   `actingctl mcp-serve` `mcp`; acui `ui`; actinglab `lab`. acsetup reaches the Runtime
   only through actingctl child processes, so it is `cli`.
7. `via` exists only on the Lab channel and only a forwarding counter sets it: MCP's
   option tools start actinglab with `--via mcp`; a future UI Lab frontend that forwards
   to actinglab passes `--via ui`. actinglab puts it into `BindChannel{lab, via}`.
   actinglab run directly from a shell has no `via`.
8. `BindChannel` is its own request kind, not a field of `Health`: `Health` is what the
   watchdog and every reconnect use to check the owner epoch, and a new client must
   still be able to check the epoch on an old actingd.

The Runtime keeps a connection-to-channel (and `via`) table in memory only; an entry
leaves when its connection closes. It replaces today's `governance_connections` set.
The table is not part of any status read and no connection count is reported:
`RuntimeControlPlaneStatus` denies unknown fields, and which channel sent a request is
visible in its receipt and its G14 record.

### Old Runtimes

An actingd older than U-2 fails to decode `BindChannel` and drops the connection without
a receipt (and records one connection failure in its own ledger). The client sends
`BindChannel` through `send_or_dropped`; when the connection is dropped it reconnects
once, checks the owner epoch again, goes on unbound and reports one line
`client_channel_unsupported` (stderr, or the MCP warnings). Later reconnects of that
client process send no `BindChannel`. `runtime-info.json` carries no interface
revision, so the client cannot ask first. This happens only when root Tools are newer
than the slot's actingd, for example after a slot rollback.

### Transition: v0.12.0 to U-6

v0.12.0 ships the Runtime only; the console still uses the runtime client pinned at
`484bdc14`, which never sends `BindChannel`. Until U-6, an unbound connection is bound
at its first request from that request's envelope source: `Ui` → `ui`, `Lab` → `lab`,
`Cli` and `Adapter` → `cli`. The recorded value is still one of the four. U-6 deletes
this rule; from then on an unbound connection is refused (`channel_binding_missing`).
The rule is not a configuration switch.

### Envelope

- Until U-6 each frontend fills the envelope (actor, source) as today; the C4 check,
  frame pinning, debug-run attachment and the offset projection's `origin` read it. A
  channel is not a source value (#338 Q2 holds).
- From U-6 the envelope is a fixed function of the channel, and the runtime client is
  the one place that guarantees it: its configuration is built from a channel only.
  `cli` → `(Cli, Cli)`; `mcp` → `(Cli, Cli)` (MCP is a clean mapping of the CLI, so the
  same request has the same envelope); `ui` → `(User, Ui)` (both console connections);
  `lab` → `(Lab, Lab)`. The Runtime never rewrites an envelope; a request whose
  envelope does not match its channel is judged by its channel.
- The actor is then a legacy field and expresses no identity.
- actinglab's three `(Cli, Cli)` connections (the input proxy behind tap, swipe,
  long-tap, key, text, `touch-probe`, `session recover` and the stream's input
  forwarding; and `session app`, twice) bind `lab` and send `(Lab, Lab)` from U-2. They
  carry Input, lease and ApplicationLifecycle requests, which pin no frame by envelope,
  so frame retention does not change for them.
- A manual run (RunContainedTask) pins its frames by envelope: once `task-run` and
  `ac_run_pack` move to actinglab `run start` (U-3), their frames are pinned `Lab` and
  kept in `kept\` like every Lab frame (Alice 10-10 12:3x item 7).

### actingd's own connections

Today actingd connects to itself twice: at start, as `(User, Ui)` with an identity
card, to transcribe the approvals written in its configuration; and as
`(Agent, Adapter)` for the policy driver's queries and subscription. U-2 turns both into
in-process calls:

- The approval transcription calls the host's approval write directly and is recorded
  through #383 G15. The ledger provenance of an approval record stays
  `(Governance, User, Ui)`: the approval projection is replayed before every approval
  write and at every start, and a record of another provenance is a fatal
  `approval_projection_origin_invalid`. The catalog transition still comes before the
  approvals.
- The policy driver reads the host directly (it no longer subscribes after #383 AO-7).

External requests then come only through the three counters and the Lab channel.

### Lifecycle control

`BindChannel`, `UpgradeState` and `UpgradeStateStatus` are lifecycle control, like
`Health` and `InstallTransition` today: the Runtime takes them while draining, while
held and while preparing. Otherwise a held new Runtime could not even bind a channel.

## Blocks

### Criteria

A kind's block depends on its channel and its kind only (for PublishFact and
PublishFacts, on its key family too, section Instance fact key families). The table is
static and exhaustive over `RuntimeOperation`: no content, no heuristics.

- **General interface: what normal operation needs.** All three counters take it,
  whether or not the Lab is installed.
  - Reads: every query (status, ledger, frames, instance discovery, resource targets).
    A read may record an observation (a capture) or write a copy (material export).
    A successful instance discovery appends no observation record (Workflow #383
    AO-4q); only G14 copies its request.
  - Instructions: a decision or setting handed to the Runtime, which does the work
    itself: pause and resume, monitor settings, whether a package is scheduled for a
    period, the priority offset, instance facts and resource targets, request shutdown,
    and the upgrade state (for upgrades only).
  - Approvals stay in the configuration; they are not requests.
- **Lab: the debugging module.** It takes the kinds its two options open:
  - option one, authoring: the agent planning session and the package tools
    (recording, binding draft, the Lab observation page's capture, artifact
    recognition, signature matching, package check, evidence export) and the
    resource-authoring event;
  - option two, debugging: what only the Runtime itself could otherwise do: tap and
    input, manual package runs, stopping a run, reset, application start and stop,
    emulator start, stop and restart and the self-check, pack load and unload, Lab
    operations, releasing a frame pin and clearing kept frames, registering and
    retiring diagnostic signatures, debug events.
  - The Lab channel also takes general reads (actinglab reads status, events and
    receipts). It takes no general instruction: the counters are their only entry.
- **Handshake**: `Health` and `BindChannel`, on any connection and any channel, before
  or after binding, whether or not the Lab is installed.
- Capture is the one kind on both sides: ObserveReadonly and CaptureSequence are general
  reads on every channel. From a counter they pin `Explicit`; from the Lab channel they
  pin `Lab` and attach to the debug run, as today. That is retention, not permission.

### The 58 request kinds

Column G14: whether #383's request boundary copies the request (after AO-4). "no" means
a pure query that writes nothing. A refused request is copied exactly when an accepted
one would be (section Records).

**Handshake (2)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| Health | handshake | — | every connection; the watchdog connects and sends only this | no |
| DeclareGovernanceIdentity | handshake, retired at U-6 | — | replaced by `BindChannel`; accepted from old clients until U-6 | no (today it writes `governance.identity_declared`; not after U-6) |

**General reads (11)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| Status, MonitorStatus, RuntimeFactSnapshot, ProjectInterface, QueryEvents, SubscribeEvents, ReadMaterial, ResourceTargetView | general | read | cli, mcp, ui; the Lab channel too | no (a failed ReadMaterial is recorded as today) |
| ObserveReadonly, CaptureSequence | general | read (frames) | cli `observe`, `stream`; mcp `ac_observe`, `ac_stream` (new); both pin `Explicit`. Lab channel: actinglab's capture commands and `runtime observe`, pinned `Lab` | yes |
| DiscoverInstances | general | read (instance discovery) | cli `emulator discover` (and acsetup's instance step); mcp `ac_emulator_discover` (new); ui instance discovery | yes (the request only; a successful discovery appends no observation record, Workflow #383 AO-4q) |

**General instructions (7)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| PauseScheduling, ResumeScheduling | general | instruction | cli `pause`, `resume`; mcp `ac_pause`, `ac_resume`; ui later | yes |
| ConfigureMonitor, ClearMonitor | general | instruction (monitor settings, `recovery_enabled` included) | cli `monitor-set`, `monitor-clear`; mcp `ac_monitor_set`, `ac_monitor_clear` (new). actinglab `session monitor-policy` retires | yes |
| RequestShutdown | general | instruction (the Runtime closes itself) | cli `request-shutdown` (stays); ui "request shutdown", sent directly as today; mcp `ac_request_shutdown` (new) | yes |
| RecordClientAction | general | instruction (trace of an action outside the Runtime) | ui (for example starting the daemon) | yes |
| ApplyResourceTargets | general (§14.2) | instruction (write resource targets) | cli `agent-apply-resource-targets` (stays); mcp `ac_targets_set` (general tool) | yes |

**General by key family (2)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| PublishFact, PublishFacts | general for every key family (§14.2) | instruction | cli `task-offset`, `agent-publish-facts` (stays), the planned `pack-schedule set` and the SR-5a entry; mcp `ac_task_offset` (new), the planned `ac_pack_schedule_set`; ui (#386 UI-2) | yes |

**Option one, authoring (16)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| StartAgentSession, ResumeAgentSession, RecordAgentResponse, PromoteProposal, PrepareStrategicReport, AssessPredictiveMaintenance | option one | instruction (agent planning session) | no program sends them today; later actinglab. PromoteProposal also needs an approval | yes |
| ProjectPolicyForward, AgentSessionStatus, ProjectPolicyInputIdentity, CompileProposal | option one | read (the planning session's reads) | as above | ProjectPolicyForward yes (it writes its entry and return); the other three no |
| ObserveContainedPage | option one | read (the Lab observation page, pins a `Lab` frame) | actinglab `observe --capture`; mcp `ac_lab_observe` | yes |
| RecognizeArtifact, MatchDiagnosticSignatures, DebugPackage, ExportEvidence | option one | read (package tools; an export writes a copy only) | actinglab `recognize-artifact`, `lab signatures match`, `lab debug-package`, `lab export-evidence` | yes |
| RecordAuthoringEvent | option one | operation (resource-authoring event) | actinglab's resource-authoring session | yes |

**Option two, debugging (18)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| RunContainedTask | option two | operation (manual package run) | actinglab `run start` (new, replaces `actingctl task-run`) and `lab run`; mcp `ac_run_pack` (forwarded to `run start`). Frames pinned `Lab` and kept | yes |
| Input | option two | operation (tap and input) | actinglab tap commands, `touch-probe`, `session recover`, `tap-target`, `navigate`, `ensure`, the stream's input forwarding | yes |
| AcquireLease, QueueLease, PollQueuedLease, RenewLease, ReleaseLease, CancelQueuedLease | option two | operation (the leases input needs) | actinglab's input proxy and debug session; queueing and polling stay on one Lab connection | yes |
| SafeReset | option two | operation (reset) | actinglab `runtime reset`, `run stop`; `actingctl reset` retires | yes |
| ApplicationLifecycle | option two | operation (application start and stop) | actinglab `session app` | yes |
| RunContainedLabOperation | option two | operation (Lab operation) | actinglab `do`; mcp `ac_lab_do` | yes |
| CancelContainedTask | option two | operation (stop a run) | actinglab `run stop` (`--cancel-only` stops without reset); mcp `ac_stop_run` | yes |
| ControlEmulatorInstance | option two | operation (emulator start, stop, restart) | actinglab `emulator start`, `stop`, `restart` (new); mcp `ac_emulator`. actingctl's three subcommands retire | yes |
| SelfCheckInstance | option two | operation (self-check) | actinglab `emulator selfcheck` (new); `actingctl selfcheck` retires | yes |
| RegisterDiagnosticSignature, RetireDiagnosticSignature | option two | operation | actinglab `lab signatures register`, `retire` | yes |
| RecordDebugEvent | option two | operation (debug event) | actinglab's debug session | yes |
| ReleaseLabPin | option two | operation (release a frame pin) | actinglab `lab unpin` | yes |

**Retired (1)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| RecordApprovalDecision | retired (external approval) | instruction | today only actingd itself, transcribing its configuration's approvals; U-2 makes that in-process. From U-6 it is refused on every channel with `channel_request_refused` | no; the in-process transcription is recorded through G15 with provenance `(Governance, User, Ui)` |

**Compat (1)**

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| InstallTransition | compat: general on `cli`, only for an old acsetup speaking install-control revision 1; superseded by the upgrade state from U-4; retires when the install-control minimum becomes 2 | instruction | an old acsetup through the installed slot's `actingctl install-transition` | yes |

Counts: handshake 2, general reads 11, general instructions 7, general by key family 2,
option one 16, option two 18, retired 1, compat 1; 58 in total. Tests assert each kind,
never a total.

Two consequences of §14.2 for U-3 (the model's body predates it): `actingctl
agent-apply-resource-targets` and `agent-publish-facts` stay in actingctl and do not
move to actinglab (no actinglab `targets set`, `targets apply` or `facts publish`), and
the contract's source gates need no widening for ApplyResourceTargets or for instance
facts: they keep reaching the Runtime from the counters with today's envelopes until
U-6 deletes the source gates.

### New request kinds

| Kind | Block | Class | Sent by | G14 |
|---|---|---|---|---|
| `BindChannel{channel, via?}` | handshake | — | the runtime client's handshake, once on every new TCP connection | no |
| `UpgradeState{target, action}` (`enter`, `cancel`, `leave`) | general | instruction (upgrades only) | cli: `actingctl upgrade-state`, used by acsetup. The gate takes it from mcp and ui too, but neither frontend maps it | yes |
| `UpgradeStateStatus{transition_id}` | general | read | cli | no |
| `InstanceFactSnapshot{instance}` (#389 MCP-4) | general | read (one instance's facts, from memory) | cli `facts --instance`; mcp `ac_facts` (scope instance) | no |
| `DrillRecoveryLadder{instance_alias, mode}` (#390 DRILL-1; `mode` `plan` or `device`) | option two | operation (a drill of one physical instance's stuck-recovery ladder, recorded like a real one and tagged `drill`; [emulator-control.md](emulator-control.md), "Drills") | cli `ladder-drill`; mcp `ac_ladder_drill` (operator before U-3); the emulator control origin gate (User+Ui, Cli+Cli) | yes |
| `LoadPack`, `UnloadPack` | option two | operation (pack load and unload) | actinglab `pack load`, `pack unload` (new) | yes |
| `ClearKeptFolder` (#375 R5e, planned) | option two | operation (clear kept frames) | actinglab `clear-kept` (moved from the planned actingctl command) | yes |
| `FactRefreshStatus` (#384 D2-S5, the D2 S5 transition query; register §1.68) | general | read (what the fact refresh waits for, from the transition recorder's memory; nothing read from or written to the ledger) | cli `fact-refresh`; mcp `ac_overview` (block `fact_refresh`) | no |
| `PerformanceStatus` (#385 P-m; register §1.61 W15) | general | read (the shadow measurement's status, from memory; nothing read from or written to the ledger) | cli `perf-status`; mcp `ac_overview` (block `performance`) | no |
| `ScheduleExplanation{instance?, candidates}` (#386 SR-6b, MCP-19a; register §1.143, §3 W5) | general | read (the latest evaluation's not-run explanation, from the policy host's memory; nothing read from or written to the ledger) | cli `schedule`; mcp `ac_schedule`, `ac_overview` (`not_running`), `ac_diagnose` (`not_run`) | no |
| a later #369 same-queue request, if any | option two | operation (lease family) | actinglab | yes |

Until U-6 the new option-two kinds `LoadPack` and `UnloadPack` accept only a
`(Lab, Lab)` envelope, like today's Lab-only kinds.

### Instance fact key families

`fact_key_family(key)` in the contract looks only at a key's spelling and returns one
of five families:

| Family | Keys | Block |
|---|---|---|
| offset | `session.task.<task_id>.priority_offset` | general |
| pack schedule (#386 SR-11) | `session.pack.schedule` | general |
| feature lock, manual mark (#386 SR-5a) | `session.feature.<feature_key>` | general |
| remedy choices (#411 B15 R6) | `session.remedy.choices` | general |
| instance information | every other key | general (§14.2) |

- Every family is general, so the gate takes PublishFact and PublishFacts from the three
  counters and refuses them on the Lab channel (from U-6). The function stays: the
  content rules use it.
- A write mixes no families: `validate()` refuses a mix with `fact_origin_mixed`, as
  today for offset and other keys; SR-11 and SR-5a add "only keys of this family" for
  theirs. `validate()` therefore guarantees one family per request.
- The two #386 families have no code yet: SR-11 and SR-5a add them to the function.
  Until then the function knows offset and the rest, as the contract does today.
- U-6 deletes only the by-origin half of today's rule (agent versus person); the
  content rules (offset checks, `session_pack_schedule_invalid` and the like) stay.
- The remedy choices family (Workflow #411 B15, [remedies.md](remedies.md)) is built (slice
  R6): `fact_key_family` returns it for `session.remedy.choices`, with the content rule
  `session_remedy_choices_invalid` ([instance-fact-store.md](instance-fact-store.md), "Remedy
  choice facts"). `validate()` refuses any observation whose keys belong to two families as
  `fact_origin_mixed`; until U-6 a family with the origin exception (offsets, remedy choices)
  is taken from `(agent, adapter)`, `(user, ui)` and `(cli, cli)`, and its `fact.published`
  event keeps the writer's origin. Its writers are actingctl `remedy set|get|lift` and the MCP
  general tools `ac_remedy_set` and `ac_remedy_get`. A record is written at instance or game
  scope only: the store's `server` scope names a server without its game, so a server record
  would apply to every game whose instances share that server id. `remedy set` takes
  `--instance` or `--game` (`--server` is `actingctl_usage_invalid`, `unknown`),
  `ac_remedy_set` takes `instance` or `game` (`server` is `arguments_invalid`, `unknown`), and
  the content rule refuses a server-scoped record from every writer
  (`session_remedy_choices_invalid`, `field` `/scope`).
- The prefix `session.dispatch.` is reserved for a run's read-only parameters (Workflow
  #411 B15 R5): it is no family, since nothing may write it; `PublishFact` and
  `PublishFacts` of such a key are refused from every origin as
  `session_dispatch_key_reserved`, and the records exist only as an overlay of a run's
  snapshots ([instance-fact-store.md](instance-fact-store.md), "Run parameters").

## Lab options

### Configuration

- Two optional booleans in the actingd configuration: `lab_module_authoring` (option
  one) and `lab_module_debugging` (option two). Absent means `false`: the Lab is not
  installed by default. The Lab is installed when at least one is `true`.
- A file names them only when `true`; a configuration that ticks neither does not name
  them and keeps its revision. A file that names one needs `actingd-config` revision 5,
  the one revision v0.12.0 raises (`component-interfaces.md`).
- One committer: acsetup commits configuration generations. `acsetup --commit-config`
  refuses a proposal that changes either key (`setup_lab_options_locked`); only the
  installer's Lab transaction changes them. They are not in #387 D1's editable set:
  `config.parameters` reports their effective value and source, and no setting, MCP
  tool or CLI command changes them.
- actingd reads them once at start; they do not change during an epoch. They are
  reported as configuration facts (`config.*`), which `actingctl status --config` and
  MCP read. They are not part of `Status`.
- Only the configuration counts, never `tools\actinglab.exe` on disk: the Runtime never
  runs actinglab, a file cannot tell two options apart, and copying the file back must
  not reinstall the Lab. When the file and the configuration disagree,
  `actingctl status --config` and acsetup report it from the local machine; the
  Runtime does not.
- Editing a configuration generation by hand, past acsetup, is not prevented: the gate
  prevents misuse, not malice (section Strength of the identity).

### The gate's decision (from U-6)

In order; the first match decides.

1. Handshake (`Health`, `BindChannel`) → take.
2. Connection not bound → `channel_binding_missing` (receipt only).
3. Retired kind (external approval; from U-6 also the identity card) →
   `channel_request_refused`.
4. Channel is a counter (`cli`, `mcp`, `ui`):
   - general (reads, instructions, PublishFact and PublishFacts) → take;
   - option one or option two → `channel_request_refused`.
5. Channel is `lab`:
   - neither option installed → `lab_module_not_installed`, general reads included;
   - general read → take;
   - general instruction (facts, shutdown request and upgrade state included) →
     `channel_request_refused`;
   - option-one kind: `lab_module_authoring` true → take; otherwise
     `lab_module_option_disabled` with `lab_option` = `authoring`;
   - option-two kind: likewise with `lab_module_debugging`, `lab_option` = `debugging`.

From U-6 a refusal's receipt carries its specific code, not only `InvalidRequest`.

### Before U-6

From v0.12.0 (U-2) to U-6 the Runtime reads both options and reports them, and every
receipt carries its channel, but nothing is refused by channel or by option. Absent
keys mean "not installed", and the installer page that writes them comes only with U-5:
refusing earlier would cut every machine that uses the Lab today.

### Without the Lab, or with one option

- Never affected: every read and instruction of the three counters (status, events,
  frames, discovery, pause, resume, monitor settings, offsets, pack schedule, instance
  facts, resource targets, shutdown request, upgrade state), and every requester of the
  Runtime itself.
- Neither option: nobody outside the Runtime starts a run, taps a device, loads or
  unloads a package, or controls an emulator, so a blackout is complete silence. No
  planning session. Stopping the Runtime works as before (request shutdown is general);
  `kept\` still collects the Runtime's own error-window frames, and clearing it is an
  option-two operation, so without it the folder is cleared by hand. MCP lists general
  tools only.
- Option one only: planning sessions and the package tools; no tap, run, stop, reset,
  application or emulator control, pack load or unload, kept clearing or pin release.
- Option two only: every option-two kind; no planning session, Lab observation page,
  artifact recognition, signature matching, package check or evidence export. MCP does
  not list the option-one tools, and actinglab's offline package tools refuse with
  `lab_command_option_disabled`.

## Records

- Every write is recorded (Alice 10-10 12:3x item 2: 「只要有记录就不怕乱改」). G14
  copies every general instruction from a counter and every write on the Lab channel
  as `command.received`, `command.validated` or `command.rejected`, with two members
  added by this model: `channel` (`cli`, `mcp`, `ui`, `lab`) and the optional `via`
  (`mcp`, `ui`).
- Reads that write records (the two capture kinds, option one's reads, the planning
  projection) and discovery (its request only: a successful discovery appends no
  observation record, Workflow #383 AO-4q) are copied with their channel too; pure
  queries (status, events, material, snapshots, the target view, upgrade-state status)
  are not.
- A refused request is copied when an accepted one of its kind would be:
  `lab_module_not_installed`, `lab_module_option_disabled` and
  `channel_request_refused` on a write become `command.rejected`; a refused pure query
  is only in its receipt.
- A request on an unbound connection has not passed the door: its refusal is only in
  the receipt.
- The gate adds no direct ledger write; the exception table keeps only W1 and W2.
- Who sent a request is found through its request id in its G14 record; effect records
  gain no member.
- Before #383 AO-4: from v0.12.0 (U-2), the `command.received` that the C4 path already
  writes carries the channel; other writes show their channel in the receipt only.
- After U-6 the identity card's `governance.identity_declared` is no longer written.

A request that MCP forwards to actinglab is a `lab` request with `via = mcp`: the Lab
handles it, and `via` keeps what the ledger already shows today (the `client.action`
`surface=mcp` record after a Lab tool) in one optional member instead of an extra record.

## Clean mapping

### Rules

1. The CLI command set is the original of the general interface. A general command is
   a name, a parameter structure, the request it sends and its outcome codes.
2. The command table lives in the runtime client. actingctl parses its arguments and
   calls it; MCP tools are generated from it (same parameters, same request, same
   codes); console actions call it.
3. Every MCP tool maps 1:1 to one command. Parameter names are the CLI flag names
   without `--` and with `-` replaced by `_`; units and defaults are the CLI's. The CLI
   gains what only MCP had (the instance selector takes an alias, an `instance_id` or an
   ADB port; run time limits). MCP's own handle for long operations (`handle`,
   `wait_s`) is presentation, not a parameter difference.
4. A composite tool is a new command of named steps, and the tool maps 1:1 to it:
   `actingctl overview` = Status + MonitorStatus + QueryEvents (`ac_overview`);
   `actingctl diagnose` = Runtime reads + the `actingd suspended` child process
   (`ac_diagnose`); `actinglab record binding-draft` = `package preflight` + the
   `actingd check-config` child process (`ac_binding_draft`).
5. MCP has no tiers. A tool is listed when it is general (always) or when its Lab
   option is installed (section MCP).
6. Every general CLI command has an MCP tool, 1:1 or covered whole by a composite
   (`status`, `monitor-status` and `emulator status` by `ac_overview`). Two exceptions:
   the upgrade state (only the installer uses it; an agent that entered it without
   holding `install\writer.lock` would hand the watchdog a restart, which is exactly the
   shutdown use Alice ruled out) and the compat `install-transition`.
7. Lab commands live in the Lab module, and actinglab is its command line. An MCP
   option tool maps 1:1 to one actinglab command and runs
   `<root>\tools\actinglab.exe --json` as a child process. MCP and actinglab judge by
   the same option.
8. The Lab commands of actingctl move without forwarding shells (U-3): each new home
   first passes its end-to-end test, then the old command retires.

### actingctl (counter `cli`)

| Command | Request | Block | New home |
|---|---|---|---|
| `status` | Status | general read | stays; `ac_overview` covers it; #390 九处-1 (board #390 comment 6095942731 item 1, corrected wave-18 ruling 4): each instance carries `emulator_state`, whether its emulator runs as the Runtime last read it, `{state: on \| off, observed_at_unix_ms}`, or `{state: unknown}` while the Runtime holds no reading (never `off` by default); the readings are `MuMuManager info` at the start's discovery (a discovery-bound instance bound to its port is `on`, bound pending `off`), a confirmed emulator start (`on`) or stop (`off`), the state read after a failed control (`unknown` when that read settled nothing after the vendor command ran) and an on-demand `DiscoverInstances` (an instance it no longer lists is `unknown`); no poll is added; an instance that is not discovery-bound is always `unknown` |
| `status --config`, `facts --program` | RuntimeFactSnapshot | general read | stays; reports the two Lab options from `config.*` and checks `<root>\tools\actinglab.exe` locally, reporting a mismatch; `ac_facts` (new); #389 MCP-4: both answer `{scope, read_at_unix_ms, snapshot \| records, records_total, records_kept}`, `facts --program` also takes `--key-prefix` |
| new: `facts --instance <alias\|instance_id\|ADB port> [--key-prefix <prefix>]` (#389 MCP-4, before U-3) | InstanceFactSnapshot | general read | actingctl; `ac_facts` with `scope` instance |
| `monitor-status` | MonitorStatus | general read | stays; `ac_overview` covers it |
| `emulator status` | Status (one instance) | general read | stays; `ac_overview` with `instance` covers it |
| `observe`, `stream` | ObserveReadonly, CaptureSequence | general read | stay (pin `Explicit`); `ac_observe`, `ac_stream` (new) |
| `emulator discover` | DiscoverInstances | general read | stays; `ac_emulator_discover` (new) |
| `pause`, `resume` | PauseScheduling, ResumeScheduling | general instruction | stay; parameters unified with `ac_pause`, `ac_resume` (MCP table) |
| `monitor-set`, `monitor-clear` | ConfigureMonitor, ClearMonitor | general instruction | stay; `ac_monitor_set`, `ac_monitor_clear` (new) |
| `task-offset` | PublishFacts (offset) | general instruction | stays; `ac_task_offset` (new) |
| `request-shutdown` | RequestShutdown | general instruction | stays; `ac_request_shutdown` (new) |
| new: `config set`, `config clear` (#387 D1b) | ConfigureSettings | general instruction | actingctl; `ac_config_set` (observer until U-3); the console's settings page in the next UI release ([runtime-config.md](runtime-config.md)) |
| `agent-publish-facts` | PublishFacts | general instruction (§14.2) | stays |
| `agent-apply-resource-targets` | ApplyResourceTargets | general instruction (§14.2) | stays (whole `--policy-file` form) |
| `install-transition` | InstallTransition | compat | stays until the install-control minimum is 2; not mapped |
| `reset` | SafeReset | option two | retires → actinglab `runtime reset` |
| `task-run` | RunContainedTask | option two | retires → actinglab `run start` (not `lab run`, which takes no recovery package or time limit, must write a zip and answers only at the end) |
| `emulator start`, `stop`, `restart` | ControlEmulatorInstance | option two | retire → actinglab `emulator start`, `stop`, `restart` |
| `selfcheck` | SelfCheckInstance | option two | retires → actinglab `emulator selfcheck` |
| new: `ladder-drill --instance <alias\|instance_id\|ADB port> [--device]` (#390 DRILL-1, before U-3) | DrillRecoveryLadder | option two | actingctl until U-3 moves the option-two commands; `ac_ladder_drill`; without `--device` a plan drill (`mode` `plan`) that never acts on the device; answers the admitted drill `{instance_alias, mode, ladder_request_id}`; success `recovery_ladder_drill_queued`; refusals `recovery_ladder_drill_disabled`, `recovery_ladder_drill_already_running`, `recovery_ladder_drill_cooling_down`, `recovery_admission_denied`, `instance_unknown`, `instance_ambiguous` |
| new: `upgrade-state enter`, `cancel`, `leave`, `status` | UpgradeState, UpgradeStateStatus | general | actingctl only; not mapped to MCP or the console |
| new: `overview`, `events`, `run get`, `material`, `diagnose`, `targets list`, `targets get` | as their MCP tools | general read | added so that every general MCP read has a CLI original |
| new: `overview [--instance <alias\|instance_id\|ADB port>] [--sections <block>[,<block>…]] [--root <install root>]` (#389 MCP-8, before U-3) | Status + MonitorStatus + QueryEvents (none records anything); #385 P-m: + PerformanceStatus for the block `performance`; #384 D2-S5: + FactRefreshStatus for the block `fact_refresh` | general read | actingctl; `ac_overview`; the answer is `ac_overview`'s, with every block whole; success `runtime_overview_reported` |
| new: `perf-status [--instance <alias\|instance_id\|ADB port>]` (#385 P-m) | PerformanceStatus (records nothing); a given `--instance` is resolved with one Status read | general read | actingctl; `ac_overview` answers the same object as its block `performance`; the status' `instances` narrowed to the one instance; actingd absent: the client's failure, exit 1; success `runtime_performance_reported` |
| new: `fact-refresh [--instance <alias\|instance_id\|ADB port>]` (#384 D2-S5) | FactRefreshStatus (records nothing); a given `--instance` is resolved with one Status read | general read | actingctl; `ac_overview` answers the same object as its block `fact_refresh`; the status' `instances` narrowed to the one instance; actingd absent: the client's failure, exit 1; success `runtime_fact_refresh_reported` |
| new: `packs` (#389 MCP-11, before U-3) | not a request: reads the selected configuration generation (`install\active.json` → its configuration file); from U-3 also the loaded-pack table through RuntimeFactSnapshot | general read (offline, no `--state-root`) | actingctl; `ac_packs` (new) |
| new: `codes <code>`, `codes [--prefix <text>] [--owner <owner>] [--limit <1-100>] [--cursor <next_cursor>]` (#389 MCP-14, before U-3) | not a request: reads the outcome catalog the build embeds | general read (offline, no `--state-root`) | actingctl; `ac_outcome_code` (new); one code answers its catalog entry as the catalog holds it with its `code`, the list `{codes: [{code, category, ladder_class, status}], next_cursor?}` in name order; success `outcome_codes_reported`; refusals `outcome_code_unknown`, `cursor_invalid`, `outcome_catalog_unreadable` |
| new: `capability-gap --capability <text> --why <text>` (#410 item 6) | not a request: appends one line to the user's local log `%LOCALAPPDATA%\ActingCommand\actingctl\capability-gaps.jsonl` | general instruction (local only, no `--state-root`, no Runtime, no ledger record) | actingctl; `ac_capability_gap` (new); each text one line, `capability` 1–120 and `why` 1–500 characters; answers `{path, gap: {schema_version, recorded_at_unix_ms, via, capability, why}}`, `via` `cli` or `mcp`; success `actingctl_capability_gap_recorded`; refusals `actingctl_usage_invalid`, `actingctl_capability_gap_write_failed`; nothing reads the log back and nothing sends it anywhere |
| new: `run evidence (--request-id <id> \| --run-id <id>) [--from-step <n>] [--limit <1-100>]` (#389 MCP-6, before U-3) | QueryEvents (Forensic) by the run's request_id, after the run is found as `ac_get_run` finds it; the frames and the overlay are material refs read with ReadMaterial (`ac_material`) | general read | actingctl; `ac_run_evidence` (new); `{request_id, run_id?, task_id?, steps: [{step_index, page, targets: [{target, matched, box?}], action?: {kind, point?, rect?}, frames: [{ref}]}], final: {page?, frame_ref?}, next_step?, overlay?: {ref}}`; the output shape is the client's only; success `run_evidence_reported`; refusals `run_unknown`, `actingctl_usage_invalid` |
| new: `--wait <s>` on `emulator start`, `stop`, `restart` and `task-run` (#389 MCP-21, before U-3) | as the command | as the command | `--wait` 1–3600 s; without it the command waits for its receipt as before; a wait that ends first prints `{request_id?, correlation_id, state: "in_progress"}` with `command_wait_expired` (`operation`, `request_id` when the command knows it, `waited_ms`) and exits 1, and the request goes on in the Runtime (it is served on its connection's thread whether the client still waits or not); no MCP argument (`ac_emulator`, `ac_run_pack` answer within their call budget or with a handle); moves with the commands to actinglab at U-3 |
| planned: `pack-schedule set`, `pack-schedule get` (SR-11), the SR-5a entry (`perf-status` is built, #385 P-m, and the D2 S5 query `fact-refresh`, #384 D2-S5, above) | PublishFacts, reads | general | actingctl; MCP and console as their slices define |
| planned: `clear-kept` (#375 R5e) | ClearKeptFolder | option two | actinglab `clear-kept` instead |
| `watchdog *`, `mcp-config` | not requests | — | stay; `mcp-config`'s example drops `--tier` |

### MCP (counter `mcp`)

Today 22 tools; U-3 adds 8 general tools, for 30: 18 general, 8 option one, 4 option
two. #410 item 6 adds one general tool, `ac_capability_gap` (observer until U-3): the build's
tool table goes from 34 to 35 tools (observer 19 to 20), and U-3's count gains one general
tool.

| Tool | Class | Maps to | Parameters | Block |
|---|---|---|---|---|
| `ac_overview` | read | `actingctl overview` | `instance` ↔ `--instance`; `sections` ↔ `--sections` (#389 MCP-8: a list of block names, `global_pause`, `instances`, `run_window`, `lab_tool`, `install`, #385 P-m's `performance`, and #389 MCP-9a's `catalog`, `config_generation`, `watchdog`, default all; `daemon` and `incomplete` are always answered; another name is `arguments_invalid`); each instance row carries `foreground {package, observed_at_unix_ms}` from the instance program fact `application.foreground` when it has one: the package last seen before a pointer input, read with RuntimeFactSnapshot, no wire change (#389 MCP-5b, D10); each instance row carries `emulator_state`, the status' reading of whether the instance's emulator runs (`on`, `off` or `unknown`, with `observed_at_unix_ms` for `on` and `off`), as `actingctl status` shows it (#390 九处-1), never cut; each instance row carries `backend_selfcheck {<entry>: {status, checked_at_unix_ms}}`, the last self-check of each backend entry (`input`, `capture`, `nemu`) the instance's program facts hold, `status` verbatim from `backend.selfcheck.<entry>.status` (`passed`, `failed`, `unknown`) and the time from `.checked_at_unix_ms`, read with RuntimeFactSnapshot, no wire change; an entry whose facts are absent or invalidated is omitted, `{}` while none was checked, and the member is absent only when the snapshot could not be read (a warning and `incomplete`); never cut (#411 A7, register §1.98 A7-1); `catalog` is the program fact `config.catalog` (register A5) read back, null while no generation is active; `config_generation {running?, selected?}` is the generation of `config.selection` beside the one `install\active.json` selects now, with the warning `config_generation_mismatch` (`path`, the selected configuration) when they differ and one `package_binding_unreadable` per binding of the selected configuration whose path is missing or unreadable (MCP-11's read); `watchdog {decision, attention, starts_24h?, last_tick?}` is `actingctl watchdog status` read locally, null without an install root (#389 MCP-9a); the server's location stands for `--root`; output as the CLI's; only MCP cuts an answer past the output budget, in the order below (presentation) | general |
| `ac_events` | read | `actingctl events` | `instance`, `view`, `since_unix_ms`, `until_unix_ms`, `after_sequence`, `through_sequence`, `event_type`, `event_types`, `exclude_event_types`, `fact_key`, `min_severity`, `max_severity`, `origin_module`, `task_id`, `run_id`, `request_id`, `correlation_id`, `detail`, `limit`, `cursor`, `wait_s` ↔ same flags (#389 MCP-1; `event_types` and `fact_key` #389 MCP-2, a list comma-separated on the CLI). `detail` is `brief` (the default), `summary` (each row adds the outcome its record carries, none on a state change; the MCP-1 follow-up) or `payload`. Paging: MCP with `next_cursor`; the CLI with `--after-sequence <last_sequence>`, because a cursor is bound to the process and connection that issued it, so the `next_cursor` one CLI call prints always answers `cursor_invalid` in the next (by design) | general |
| `ac_get_run` | read | `actingctl run get` | `handle` or `run_id` ↔ `--handle` or `--run-id`; `wait_s` is presentation | general |
| `ac_material` | read | `actingctl material` | `ref`, `mode`, `offset`, `max_bytes` ↔ same flags | general |
| `ac_diagnose` | read | `actingctl diagnose` | `instance` (optional: without it every instance, #389 MCP-7), `task_id`, `since_unix_ms` ↔ same flags; the server's location stands for `--root`. Beside today's sections it answers `runs_grouped` and `failures_grouped`, read with QueryEvents. Its text says that `actingd suspended` reports the configuration and not runtime pack loads or unloads; as in the CLI, the generation it reads is found again at every call; only MCP cuts lists to the output budget (presentation) | general |
| `ac_resources_list`, `ac_targets_get` | read | `actingctl targets list`, `targets get` | `instance` ↔ `--instance` (alias, `instance_id` or ADB port) | general |
| `ac_packs` (new, #389 MCP-11) | read | `actingctl packs` | `instance`, `role` ↔ same flags (`instance` matches the configuration's alias, `instance_id` or ADB port); the server's location stands for `--root` and, as in the CLI, the selected generation is found again at every call; an answer past the output budget is exported (presentation) | general |
| `ac_devices`, `ac_device_bind`, `ac_device_unbind` (new, #398 RD-1c; observer before U-3) | read; instruction (the state root's private binding file) | `actingctl devices list`, `devices bind`, `devices unbind` | none; `alias`, `handle`; `alias` ↔ the positional words; the server's location stands for `--root` and `--state-root`; output as the CLI's; [device-binding.md](device-binding.md), "Choosing a device" | general |
| `ac_outcome_code` (new, #389 MCP-14; observer before U-3) | read (offline) | `actingctl codes` | `code` ↔ the positional `<code>`; `prefix`, `owner`, `limit`, `cursor` ↔ same flags (`limit` 1–100, default 20, on both sides); `code` excludes the other four; output as the CLI's; a cursor holds across processes and both sides, for the same `prefix` and `owner`; only MCP cuts a list to the output budget (`truncated`, presentation) | general |
| `ac_capability_gap` (new, #410 item 6; observer before U-3) | instruction (the user's local log only) | `actingctl capability-gap` | `capability`, `why` ↔ same flags; output as the CLI's, the line's `via` `mcp`; observer because a gap shows wherever an agent works, and it touches neither the Runtime nor a device | general |
| `ac_run_evidence` (new, #389 MCP-6; observer before U-3) | read | `actingctl run evidence` | `handle` ↔ `--request-id` (as for `ac_get_run`), `run_id`, `from_step`, `limit` ↔ same flags (`limit` 1–100, default 20, on both sides); exactly one of `handle` and `run_id`; output as the CLI's; only MCP cuts a page to the output budget, from its last step (`truncated`, presentation), and `next_step` continues after the last step answered | general |
| `ac_pause` | instruction | `actingctl pause` | `drain_timeout_ms` in milliseconds, its range a contract constant; default reason `operator` for both (the counter is recorded by the channel); the CLI's `--instance` takes all three selector forms | general |
| `ac_resume` | instruction | `actingctl resume` | the CLI gains the optional expected-pause parameter `ac_resume` has; selector as above | general |
| `ac_targets_set` | instruction | the actingctl command with `--instance`, `--targets`, `--valid-days` that builds the v2 document as the tool does today (named by U-3) | `instance`, `targets`, `valid_days` | general (§14.2) |
| `ac_request_shutdown` (new) | instruction | `actingctl request-shutdown` | `wait_s` ↔ `--wait` in seconds; MCP allows 0–20 s inside its 25 s call budget, the CLI 3600 s (presentation); an expired wait gives the CLI's answer | general |
| `ac_facts` (new; #389 MCP-4, observer before U-3) | read | `actingctl facts --program`, `facts --instance` or `status --config` | `scope` (`program`, `instance` or `config`) selects the form; `instance` ↔ `--instance` (alias, `instance_id` or ADB port; scope instance only), `key_prefix` ↔ `--key-prefix` (program and instance); output as the CLI's, exported to a file past the output budget | general |
| `ac_observe` (new) | read (frames) | `actingctl observe` | `instance` ↔ `--instance`; pins `Explicit` | general |
| `ac_stream` (new) | read (frames) | `actingctl stream` | `instance`, `max_frames`, `interval_ms` ↔ same flags (defaults 1 frame, 250 ms); past the call budget it returns a process-local handle | general |
| `ac_emulator_discover` (new) | read (discovery) | `actingctl emulator discover` | as the CLI | general |
| `ac_monitor_set`, `ac_monitor_clear` (new) | instruction | `actingctl monitor-set`, `monitor-clear` | `instance`, `interval_ms`, `expect`, `recover` ↔ same flags (defaults 30 000 ms, `home`) | general |
| `ac_task_offset` (new) | instruction | `actingctl task-offset` | as the CLI | general |
| `ac_lab_observe` | read | actinglab `observe` | unchanged, without package reference parameters (M-PKG); `preview` ↔ `--preview` (#410 item 1: every answer states `frame_coordinates`, the preview is a JPEG file answered by its path and scale, never inlined; an exported answer keeps the frame parts inline while they fit); `sample_scenes` (1 to 4) ↔ the earlier `--scene` frames, before `scene`'s (#411 Lab B-2) | option one |
| `ac_record_start`, `ac_record_mark`, `ac_record_stop`, `ac_record_status` | offline (recording files) | actinglab `record start`, `mark`, `stop`, `status` | unchanged | option one |
| `ac_binding_draft` | offline | actinglab `record binding-draft` (new composite) | `record_stop`, `record_stop_export` ↔ same flags | option one |
| `ac_pack_check` | offline | actinglab `package check` (structural admission, M-PKG; #411 A5: answers `{status, package_id, entry_task_id}`) | `package` ↔ `--package` | option one |
| `ac_catalog_check` | offline | actinglab `resource catalog` | unchanged | option one |
| `ac_run_pack` | operation | actinglab `run start` (new) | `instance`, `package`, `recovery_package`, `deadline_s` ↔ same flags; `deadline_s` 60–1800, default 1800 | option two |
| `ac_stop_run` | operation | actinglab `run stop` (new) | `handle` ↔ `--request-id`; `wait_s` ↔ `--wait-s` (0–25); `--cancel-only` while this process's job still runs | option two |
| `ac_emulator` | operation | actinglab `emulator start`, `stop`, `restart` (new) | `instance`, `action` ↔ `--instance` and the subcommand | option two |
| `ac_app` (new, #389 MCP-13a; operator before U-3) | operation | actinglab `session app <launch\|restart\|stop\|force-stop>` | `action` ↔ the action word, `instance` ↔ `--instance` (both required); never `--record`; `force-stop` is the same action as `stop`; marked destructive; actinglab's data verbatim and its errors as every Lab tool relays them | option two |
| `ac_lab_do` | operation | actinglab `do` | unchanged, without package reference parameters (M-PKG); `preview` ↔ `--preview` (#410 item 1: every answer states `frame_coordinates`; after an action `after_attachment` is the known after page's projection or the after frame's path and preview path, never an image; an exported answer keeps the frame parts inline while they fit); `sample_scenes` (1 to 4) ↔ the earlier `--scene` frames, before `scene`'s (#411 Lab B-2) | option two |
| `ac_ladder_drill` (new, #390 DRILL-1; operator before U-3) | operation | `actingctl ladder-drill` | `instance` ↔ `--instance` (alias, `instance_id` or ADB port), `device` ↔ `--device` (a boolean, absent false); output as the CLI's; marked destructive (a device drill may stop the application or restart the emulator); answers at admission, before the drill climbs | option two |

- `ac_overview` past the output budget (#389 MCP-8, MCP-0 command table §4.9): it cuts one
  block at a time until the answer fits, in this order: `fact_refresh`, `host`, `startup`,
  `tagged_state`, `ledger`, `performance`, `watchdog`, `catalog`, `config_generation`, each
  instance row's `run` (the last row first), each row's `monitor`, each row's `foreground`
  (#389 MCP-5b), `install.build`. A cut
  block leaves `<block>_truncated: true` where it was. The daemon, the global pause, each
  row's identity, `emulator_state` (#390 九处-1), `backend_selfcheck` (#411 A7), `pause`,
  `lease_active` and `queued`, and `incomplete` are never cut; an
  answer that still does not fit is `output_budget_exceeded`. A slice that adds an overview
  block adds its name to `--sections` and registers its place in this order in the same PR.
- The block `performance` (#385 P-m) is the shadow measurement's status, the object
  `actingctl perf-status` prints (PerformanceStatus, register §1.61 W15), its rows narrowed by
  `instance` too. A read that fails leaves a warning and `incomplete`, never an empty block.
- The block `fact_refresh` (#384 D2-S5) is what the scheduler's fact refresh waits for, the
  object `actingctl fact-refresh` prints (FactRefreshStatus, register §1.68): the numbers of
  the latest cycle and transition and one row per instance with a waiting key, its rows
  narrowed by `instance` too ([scheduling/README.md](scheduling/README.md), "Fact Refresh").
  A read that fails leaves a warning and `incomplete`, never an empty block.
- `ac_run_pack` mints the request id and correlation id itself and returns `handle` =
  that request id at once (valid across restarts, as today); its job runs
  `actinglab --json run start … --request-id <id> --correlation-id <id> --via mcp` with
  a child deadline of `deadline_s` plus a margin. `ac_get_run` reads the Runtime by
  request id. The runtime client gains an entry that prepares a run with a given
  request id.
- `ac_stop_run`: while this process's job still runs, `actinglab run stop --request-id
  <id> --cancel-only --via mcp` (the job's actinglab resets after the cancel);
  otherwise `actinglab run stop --request-id <id> --wait-s <n> --via mcp` (cancel and
  reset).
- **How MCP knows the options**: from the configuration facts (`config.*` in
  RuntimeFactSnapshot, which the Runtime answers while draining too) at start and at
  every `tools/list`; when the Runtime does not answer, from the selected configuration
  generation (`install\active.json` → its configuration file, as the watchdog reads
  it), with a `tools/list` warning naming the source. From U-3 to U-6 a configuration
  that names either key is taken as written; one that names neither lists every option
  tool when `tools\actinglab.exe` exists and none otherwise. From U-6, naming neither
  means neither is installed.
- The server declares `tools.listChanged: false`, so a changed option shows in the tool
  list from the agent's next session. Each call to an option tool checks the options
  again: an option that is off (or a tool that was not listed) gives
  `lab_tool_option_disabled` with `mcp_blocker` = `installer_lab_option` and starts no
  child process; a missing `actinglab.exe` gives `lab_tool_unavailable`; a refusal by
  the Runtime (`lab_module_*`) passes through.
- `--tier` is accepted and ignored: the server starts as usual and reports
  `mcp_tier_retired` once on stderr and in the `tools/list` warnings. A later release
  removes the argument.
- U-6 stops MCP's identity card, its per-write `client.action` record and the record
  after a Lab tool; G14's `mcp` and `via` replace them. MCP writes are `(Cli, Cli)`.
- Instructions text: no tiers; "queries and the instructions operation needs are always
  there; authoring and debugging tools appear only when the installer ticked their
  option, from the next session"; "approvals and configuration file edits belong to
  the person, a setting the Runtime may change while running is set with
  ac_config_set and every change is recorded (#387 D1b, M387 §8.4); starting the
  daemon belongs to the person and the watchdog; request shutdown is a general
  request".

### Console (counter `ui`)

| Action | Request | Block | Maps to |
|---|---|---|---|
| browse and follow the ledger | QueryEvents; cold file reads | general read | `actingctl events`; a cold read is not a request |
| follow polling | RuntimeFactSnapshot | general read | `actingctl facts --program` |
| read material | ReadMaterial | general read | `actingctl material` |
| status | Status | general read | `actingctl status` |
| instance discovery | DiscoverInstances | general read | `actingctl emulator discover` |
| request shutdown | RequestShutdown | general instruction | `actingctl request-shutdown`; sent directly as today, with or without the Lab |
| button trace | RecordClientAction | general instruction | none: it records actions outside the Runtime only. The trace before a request retires at U-6; G14's `ui` replaces it |
| start the daemon, save instance settings, unlock the owner | not requests | — | unchanged; a configuration change takes effect by request shutdown and a new daemon start, as today |

Both console connections bind `ui`; from U-6 both send `(User, Ui)`.

### actinglab (the Lab channel)

| Command | Request | Block |
|---|---|---|
| tap, swipe, long-tap, key, text, `touch-probe`, `session recover`, the stream's input forwarding | leases, Input | option two |
| `session app`, `session instance app …` | ApplicationLifecycle | option two |
| `tap-target`, `navigate`, `ensure`, `do`, `lab run`, `runtime reset` | Lab operation, Input, manual run, reset | option two |
| capture commands, `runtime observe` | ObserveReadonly, CaptureSequence | general read (pinned `Lab`) |
| `observe --capture` | ObserveContainedPage | option one |
| `status`, `session status`, `lab status`, `lab watch`, `lab receipt`, `run summary` | general reads | general |
| `lab signatures match`, `lab debug-package`, `lab export-evidence`, `recognize-artifact` | package-tool reads | option one |
| the resource-authoring session | RecordAuthoringEvent | option one |
| `lab signatures register`, `retire`, `lab unpin`, the debug session's events | signatures, pin release, RecordDebugEvent | option two |
| `session monitor-policy set`, `clear` | ConfigureMonitor, ClearMonitor | retires → `actingctl monitor-set`, `monitor-clear` |
| new: `run start` | RunContainedTask | option two: `--instance` (three selector forms), `--package`, `--recovery-package`, `--deadline-s` (60–1800, default 1800), `--request-id`, `--correlation-id` (minted when absent), `--out` (only when given, a zip projection); output shaped as today's `actingctl task-run` (`receipt.result.outcome`, `final_page`, `executed_steps`) |
| new: `run stop` | CancelContainedTask (and SafeReset) | option two: `--request-id`, `--wait-s`, `--cancel-only` |
| new: `emulator start`, `stop`, `restart`, `selfcheck` | ControlEmulatorInstance, SelfCheckInstance | option two |
| new: `pack load`, `pack unload` | LoadPack, UnloadPack | option two; `pack unload --wait-s` (0–25) |
| new: `clear-kept` (planned) | ClearKeptFolder | option two, with #375 R5e |
| `package check` (#411 A5); new: `record binding-draft` | offline (`record binding-draft` a composite) | option one |
| record (with `record binding-draft`), resource catalog, package, resource, scheduling and operation offline commands | none (no Runtime connection) | option one (package tools); judged by actinglab itself; the per-command list is U-3's |
| help, version, paths, capabilities, doctor, schema, config, explain | none | actinglab's own upkeep; not judged |
| `resource restore` cold read | none | unchanged |

- Every actinglab connection binds `lab`, with `via` when `--via` is given.
- actinglab judges its offline package tools as MCP judges its tools: it reads the two
  options from `config.*` through RuntimeFactSnapshot (a general read on the Lab
  channel; a `lab_module_not_installed` answer means neither is installed), and from
  the selected configuration generation when the Runtime does not answer; until U-6 a
  configuration that names neither counts as both installed. Without option one it
  refuses with `lab_command_option_disabled` (one JSON line, exit code 1, no file
  touched).
- A refused actinglab command prints one JSON line and exits 1.

Scripts that call `actingctl task-run` move to `actinglab run start` with the same
parameters and the same output shape, in the same deployment as U-3. Scripts that stop
the Runtime keep `actingctl request-shutdown`: the upgrade state is for upgrades only,
and a script that does not hold `install\writer.lock` would race the watchdog.

## Upgrade state

The installer's family of general instructions, used for upgrades only (Alice 10-10
10:5x: 「安装器只是向RT传递一个专用指令：进入升级态，待RT返回OK后，再执行」; 12:3x item 6: it
is not a shutdown: 「不能，得是单独的指令」). The Runtime drains, records its pause,
releases and closes itself. `enter` closes the old Runtime; `cancel` undoes it; `status`
reads; `leave` releases the new Runtime and stays with the installer, which first proves
that the held Runtime is the process it started (#381 A). It works with or without the
Lab. Shapes are install-control revision 2 (`component-interfaces.md`), from U-4.

### Shapes

Every upgrade-state request is `UpgradeState{target, action}`: `target` is the Runtime
it is meant for (`RuntimeShutdownTarget`, as in `InstallTransition`;
`install_owner_mismatch` when it is not this owner), and `action`
(`UpgradeStateAction`) is `enter`, `cancel` or `leave` with the fields below.

| Request | Fields | Sender | The Runtime | Reply |
|---|---|---|---|---|
| `UpgradeState{enter}` | `transition_id` (from the installer); `timeout_ms` (drain limit, default 60 000) | the installer, to the running Runtime | closes admission and drains (as BeginDrain today); when idle, records its pause and reaches Drained; while the requesting connection is still open, accepts its own close (as CommitShutdown today), writes the upgrade-state marker on its owner-journal close record, closes and exits. The request thread watches its own connection while it waits; if the connection closes before the close is accepted, the Runtime handles it as `cancel`: admission reopens, the transition is recorded as Aborted with `failure_code` = `upgrade_state_requester_disconnected`, and the Runtime keeps running | `upgrade_state_entered` (with the ticket) once drained and closed; otherwise a refusal code |
| `UpgradeState{cancel}` | `transition_id` | any `cli` connection | while draining: reopens admission and records Aborted (as Abort today) | `upgrade_state_cancelled`; after the close was accepted, `install_abort_state_invalid` |
| `UpgradeState{leave}` | the ticket; `timeout_ms` (default 60 000) | the installer, to the new Runtime started with `--install-held` | Held → prepares the provider, restores the pause of the previous ticket, opens admission (as Release today) | `upgrade_state_released`; otherwise a refusal code |
| `UpgradeStateStatus` | `transition_id` | any `cli` connection | reads memory only; writes nothing | the phase: `draining`, `drained`, `held`, `preparing`, `released`, `aborted`, `failed` (today's `InstallTransitionPhase`, no new value) |

- The new Runtime is started held by the start argument `--install-held`, as today; that
  is not a request. acsetup checks with `status` that it is its own process (pid and
  minted request id), then sends `leave`. Without `leave` within 60 s the start ends
  with `held_timeout` and the watchdog applies its rules.
- The transition reuses today's transition machine, `host.install_transition` (a
  workbench record) and the lifecycle records; no new phase value.

### The marker, the maintenance owner and the watchdog

- An upgrade-state close carries a marker on its owner-journal close record: the
  field `closed_by` with the value `upgrade_state`. The field is optional; the
  owner-record schema number does not change. U-1 makes every v0.12.0 reader
  (actingd's start, `ledger-maintenance`, and `actingctl request-shutdown --wait`,
  which reads loosely) accept it and names it in `owner-journal.md`; U-4 writes it.
  `owner.lock` is not the ledger.
- A maintenance owner (both lock sites of `ledger-maintenance`) whose preceding record
  is a marked close copies the marker onto its own close. The marker is used up when a
  non-maintenance Runtime owner starts: that owner's close does not carry it. So the
  cold ledger verification that acsetup runs after `enter` does not hide the marker.
  `unlock-owner` acts only on live records and is unchanged.
- The watchdog gains one decision row, after rows 5, 5′ and 6′ and before row 6: "J
  inactive and its close carries the upgrade-state marker → consider a start (rows
  7a–7f)" (`runtime-watchdog.md`, section Upgrade-state close). A held new Runtime has
  its own owner, so the row no longer matches once it runs.
- The installer is "working" exactly while it holds `install\writer.lock`. Every
  installer flow that closes the Runtime and starts it again (instance step,
  `ensure_running`, resource-only update, Lab-options-only change, upgrade, rollback)
  holds the lock from before the close until after the start (U-5).
- The watchdog brings a Runtime back only when its scheduled task is registered;
  acsetup never registers it. Before acsetup says that the watchdog will start the
  selected generation, it runs the read-only `watchdog status --root <root>` through the
  installed slot's actingctl and says so only when `attention` holds neither
  `task_missing` nor `task_mismatch`; otherwise it reports "Runtime remains stopped" and
  how to start it, as today.
- An external stop is only RequestShutdown: a formal close without the marker, which
  the watchdog never restarts.

### Time limits

- actingctl `upgrade-state enter`: one deadline for the whole run = `timeout_ms` + 10 s
  (exchange) + `--wait` (≤ 60 s); 130 s by default.
- acsetup's child-process limit for that step = the above + 5 s, 135 s by default; its
  other actingctl children (`status`, `leave`, `cancel`, probes) keep 75 s.

### Refusals

Today's codes with today's meaning. All are already registered under runtime-host (the
`install_*` codes and `drain_timeout` by #378 A2b, `release_timeout` and `held_timeout` before
it, `upgrade_state_aborted` by U-0); U-4 changes only their paths and texts:

| Code | When |
|---|---|
| `install_owner_mismatch` | the target is not this owner |
| `install_owner_stopping` | already closing |
| `install_transition_conflict` | already in another upgrade state, or the `transition_id` repeats |
| `install_transition_unknown`, `install_ticket_mismatch` | `cancel`, `leave` or `status` do not match |
| `install_abort_state_invalid` | `cancel` after the close was accepted |
| `install_release_state_invalid` | `leave` while not Held |
| `drain_timeout` | the drain timed out: the Runtime reopens admission itself (Aborted) and keeps running |
| `upgrade_state_aborted` | the reply to an `enter` still draining when a `cancel` withdrew it |
| `release_timeout`, `held_timeout` | the release or the held start timed out; the start ends |

`install_identity_required` retires: there is no identity card. No domain is left to
decide: `install_` is already shared by six owners (actingctl, actingd, artifact-store,
contract, runtime-client and runtime-host), so runtime-client's
`install_transition_receipt_unconfirmed` keeps its place beside the host's codes;
`drain_`, `held_` and `upgrade_state_` are runtime-host's, and `release_` is shared.

### The installer when refused or unanswered (Fail Loud)

| Case | Installer | Result |
|---|---|---|
| `upgrade_state_entered`, then the close record and the process exit | goes on: cold ledger verification, root tools, selection commit, new start | the held new Runtime has its own owner |
| a refusal code | installs nothing, leaves the selection, writes the code into its log and page, exits 1 | the old Runtime keeps running |
| `drain_timeout` | as above | the Runtime has reopened itself |
| the installer vanishes during `enter` before the close | — | handled as `cancel`; Aborted with `upgrade_state_requester_disconnected`; the Runtime keeps running |
| no answer (exchange timeout, installer still alive) | does not resend; reads `status` once, reports the phase, installs nothing, exits 1 | if the Runtime had accepted its close, it closed with the marker; the watchdog (when registered) starts the selected generation once the installer releases the writer lock |
| a step after `upgrade_state_entered` fails, or the installer vanishes after it | keeps or restores the selection; reports the watchdog restart only when the task is registered, otherwise "Runtime remains stopped" and how to start it; exits 1 | the maintenance owner copied the marker; the watchdog (when registered) starts the selected generation after the lock is released |
| `leave` refused or timed out | reports it; the new start ends | the watchdog applies #381 A |

### Negotiation

- The Runtime declares install-control [0, 2] from U-4's release: revision 0
  (`actingctl request-shutdown --wait`, a start without `--install-held`) still works
  because `request-shutdown` stays, so the minimum does not rise. v0.12.0 still
  declares [0, 1].
- acsetup declares [0, 2] (U-5) and uses the highest common revision. An old acsetup
  ([0, 1]) uses revision 1 with a new Runtime, as today. acsetup closes the old Runtime
  with the installed slot's actingctl, so the first upgrade to a Runtime that speaks 2
  still closes the old one with revision 1.
- To a Runtime that declares at most 0, acsetup uses `request-shutdown --wait`.
- A later release declares [2, 2]: InstallTransition and `actingctl install-transition`
  retire (Alice decides when).

## Pack load and unload

Pack load and unload are ordinary option-two operations (U-3), not a slice or a release
of their own (Alice 10-10 12:3x item 4). Loading puts a task package into the Runtime's
in-memory table at a given place; unloading takes it out of memory (10-10 10:2x
item 5). Neither touches files on disk.

### The loaded-pack table

- The Runtime builds its packages from the configuration generation at start: the
  procedure manifest (`procedure_ref` → binding: `operation_id`, `yield_points`,
  optional `scheduled_execution`), the startup package per instance, the ladder settings.
  Some are `OnceLock` values set once. Workflow #373 NAV-3a (M-373 §4.3): the start table's
  prerequisite and return-home part is the standard-pack registry, read when it is used (not a
  start snapshot): every package an installed standard pack lists, and its game and server's
  return-home package.
- The effective table = the start table + runtime changes, one logical table:
  - the start table is what the configuration built; it never changes, and the
    procedure manifest is never rebuilt;
  - a runtime change is one entry per package in the scheduler workbench: `unloaded`,
    or `loaded{a complete binding}`;
  - changes override the start table.
- Scope: procedure manifest bindings (with `scheduled_execution`), startup packages,
  prerequisite and return-home packages: the packages the Runtime runs or resolves on
  its own. The catalog is not in it.
- Key: a package's identity is its `package_id`, which is the procedure manifest's
  `procedure_ref` and the `package_id` of SR-11's rows. Startup and prerequisite
  packages are located by their configuration item (a prerequisite package by its registry
  entry); a `LoadPack PrerequisitePackage` lays its `package_id` over the registry, and an
  `UnloadPack` of it makes that id unbound (when U-3 brings the behaviour). A request names an
  entry by
  `PackRegistryKey` (U-1): `procedure{package_id}`, `startup_package{instance_alias}`
  or `prerequisite_package{package_id}`; the workbench key shape is #383 AO-0's.
- Every use reads the effective table: policy evaluation and admission, startup
  packages, the ladder's `return_home` and `application_restart`, prerequisite chain
  resolution, D2's reader dispatch.
- Unloading everything does not trigger `procedure_manifest_empty`: every candidate is
  deferred with `pack_registry_not_loaded` and evaluation ends normally.
- The table belongs to the scheduler workbench; there is no third store.
- `actingd suspended` reports the configuration, without runtime loads or unloads.
- A manual run needs no load: `run start` brings its package location, and the Runtime
  admits the package structurally on the spot.

### Shapes

No shape carries a package digest (M-PKG).

| Request | Fields | Checks | Success |
|---|---|---|---|
| `LoadPack{entry: restore}` (restore a start-table entry) | `key` (`PackRegistryKey`) | the start table has it; the package at its location passes structural admission | `pack_registry_loaded` |
| `LoadPack{entry: procedure}` (a new package or a replacing binding) | one `policy.procedure_manifest` entry: `package_id` (= `procedure_ref`), `operation_id`, `yield_points`, optional `scheduled_execution` (`mode` + `package_path`; absent = a client run) | as at start: the binding's shape; `scheduled_execution`'s instance and backend mode; structural admission; when a catalog task names it, `operation_id` and `yield_points` match; a package no task names may load and is unused, visibly in the receipt and the workbench record | `pack_registry_loaded` |
| `LoadPack{entry: startup_package}`, `LoadPack{entry: prerequisite_package}` | the configuration item (`instance_alias` or `package_id`) + the package location (`package_path`) | as above | `pack_registry_loaded` |
| `UnloadPack{key, wait_ms}` | `key` (`PackRegistryKey`: a procedure's `package_id`, or the startup or prerequisite configuration item); `wait_ms` (0–25 000, as `run stop` waits) | in the effective table; never refused because the package is in use | `pack_registry_unloaded`; the result `PackUnloaded{key, stopped_runs[{request_id, settled}], withdrawn_dispatches}` lists the stopped runs and counts the withdrawn dispatches |

- A package that cannot be read or declares something invalid keeps the structural
  admission codes; there is no new code for it.
- A replacing binding must keep the start-table entry's run mode
  (`scheduled_execution`); otherwise `pack_registry_entry_invalid`. A run mode changes
  through the configuration.
- `pack_registry_entry_unknown`: the package is neither in the effective table nor in
  the configuration.

### Unloading a package in use

The user's or agent's input is always the highest priority (Alice 10-10 12:3x item 4:
「不论正在进行，因为用户/智能体输入永远是最高级那一档」). The Runtime:

1. **Closes the package's admission**: the package leaves the effective table's
   dispatchable set at once; evaluation defers it with `pack_registry_not_loaded`.
2. **Stops the runs that use it**, through the path of a run stop: policy dispatches and
   runs the Runtime scheduled, startup packages, the ladder's `return_home` and
   `application_restart`, the prerequisite layer that is running the package, and the
   manual runs (`run start`, `lab run`) of the same `package_id` (M-PKG D10, decided
   by Alice 2026-10-12).
   - The Runtime sets the cancellation itself. For a scheduled run it bypasses the
     "scheduled runs are not client-cancellable" check
     (`scheduled_contained_task_not_client_cancellable`), as the instance pause's drain
     timeout does today (`request_pause_cancel`).
   - The reason is the existing `ClientRequested`: no new persisted value, and it does
     not trigger the ladder.
   - The run ends at its next checkpoint and is reset (as `run stop`'s SafeReset).
   - A cancelled scheduled run ends as `TaskOutcome::Failure`; how the decision memory
     settles it (not as the task's own failure) is U-3's.
3. **Drops its queued and admitted work**:
   - an admitted dispatch of the package that has not started is withdrawn and recorded
     with `pack_registry_dispatch_withdrawn` (info, in the decision record);
   - of the queued host claims, only the package's startup-package claims are dropped,
     with `pack_registry_not_loaded`;
   - unchanged: policy dispatches never queue; queued client requests stay (a manual
     run waiting for the key starts after the unload and runs, below); queued ladder
     claims stay and skip the rung that needs the package when their turn comes.
     `cancel_instance_queue` is not used (it clears the instance's whole queue);
   - an evaluated but not yet admitted intent: the pack table's revision is part of the
     composite snapshot identity (as SR-11), so admission answers `policy_facts_stale`
     and the next cycle evaluates again.
4. **Unloads**: writes the workbench record and answers `pack_registry_unloaded`.

- The table changes at step 1. The reply waits until the stopped runs reached their
  terminal and were reset, at most `wait_ms`; after that it still answers success and
  marks the runs still stopping (as `run stop --wait-s`).
- A manual run is matched by the `package_id` of its admitted package: the same id
  stops, another id does not; there is one copy per package, so "another version" no
  longer exists (M-PKG D10, decided by Alice 2026-10-12). A manual run started after the unload
  runs: a manual run never needed a load. A prerequisite chain that needs an unloaded prerequisite package is refused as
  today with `contained_task_prerequisite_*`.
- A ladder rung whose run is stopped by an unload is recorded as skipped
  (`RecoveryRungOutcome::Skipped` with the existing reason `no_recovery_package` or
  `no_startup_package`, and the stopped run's id), not as failed; the ladder goes on to
  its next rung. How the ladder recognises such an end is U-3's.

| Work | Load (or replace) | Unload |
|---|---|---|
| a run the Runtime started | unaffected (it uses what its admission read) | step 2 |
| an admitted, unstarted dispatch | unaffected | step 3: withdrawn |
| queued host claims | unaffected | step 3: only its startup-package claims; ladder claims stay; client leases untouched |
| an evaluated, unadmitted intent | `policy_facts_stale` at admission | the same |
| later policy evaluation | uses the new binding; one with `scheduled_execution` is run by the Runtime | candidates deferred with `pack_registry_not_loaded` (before SR-11's `pack_schedule_closed`); evaluation never fails at the missing binding |
| startup package after an emulator start | the new one | skipped, `pack_registry_not_loaded`; no wait |
| ladder `return_home` | a failed run's own recovery package is unaffected; the return-home package is the new one | a failed run's own recovery package is used; a needed return-home package skips the rung as a missing package; a running rung is stopped and skipped (`no_recovery_package`) |
| ladder `application_restart` | the new startup package | skips the rung; a running rung is stopped and skipped (`no_startup_package`) |
| prerequisite chain | the new one | the running layer stops; later resolution is refused (`contained_task_prerequisite_*`) |
| D2 reader dispatch | the new one | the reader's own decision is deferred; a running reader stops; affected candidates fall to D2 §3.7 row 6: `fact_refresh_pending` (info, `gate_code` = `pack_registry_not_loaded`), and `fact_reader_overdue` when `overdue_ms` is set |
| manual Lab runs | unrelated | the running ones of the same `package_id` stop; new ones run |

### Records and lifetime

- The request: G14 (`channel = lab`, possibly `via`), with the existing action value
  `runtime.action`; no new value (`EventAction` is a closed code that older readers
  cannot decode).
- The effect: a workbench record (#383 G3) in the existing `host.` family, key
  `host.pack_registry.<package_id>` (startup and prerequisite packages by
  configuration item, as AO-0 fixes), one per load or unload; its value is the entry
  after the change, and an unload's value also lists the request ids of the runs it
  stopped. The value shape is AO-0's.
- Stopped runs keep today's terminal records, with cancellation reason
  `ClientRequested`; the unload's receipt and workbench value name them by request id.
- Deferrals, skips and withdrawals (`pack_registry_not_loaded`,
  `pack_registry_dispatch_withdrawn`) go into the evaluation record and the reason chain
  (#383 G5), like SR-11's `pack_schedule_closed`.
- Memory only: a new epoch drops these workbench keys (as the installation transition's
  keys at takeover) and the table returns to the configuration generation.
  `host.pack_registry.` is in `ledger_observer.tag.reobserve_prefixes`: not loaded at
  the next start, and never `ledger_tag_key_unclassified`.

### Relation to SR-11 and to installation on disk

| | Pack load and unload | SR-11 pack schedule | installation on disk (acsetup) |
|---|---|---|---|
| What | option two: whether the package is in memory | general instruction: whether the package is scheduled in a period | the installer places packages, rebinds and commits a configuration generation, outside the Runtime |
| Stored in | the workbench, this epoch | the instance store, `session.pack.schedule`, across restarts | disk and configuration generations |
| Sent by | the Lab channel with option two | the three counters | acsetup, while the Runtime closed itself in the upgrade state |
| Running work | stopped first | not stopped | the Runtime has drained |
| Without the Lab or option two | refused | taken | done |
| Order | first: not loaded → `pack_registry_not_loaded` | then: closed → `pack_schedule_closed` | decides the next start's table |

An unloaded package keeps its SR-11 rows; they act again once it is loaded.

## Outcome codes

Every code follows `runtime-outcomes.md` and the outcome-code writing spec. The 21
Runtime codes below are registered by U-0 as pending entries, in their owners'
fragments and registries, with their domains; the slice in the last column first emits
them.

| Code | Category | Owner (domain) | When | Values | Emitted from |
|---|---|---|---|---|---|
| `channel_binding_missing` | error | runtime-host (`channel_`) | an unbound connection sent a request other than the handshake (receipt only) | `operation` | U-6 |
| `channel_binding_exists` | error | runtime-host (`channel_`) | a second BindChannel on one connection | `channel` | U-2 |
| `channel_request_refused` | error | runtime-host (`channel_`) | the kind is not taken on this channel: an option kind on a counter, a general instruction on the Lab channel, an external approval anywhere | `operation`, `channel` | U-6 |
| `lab_module_not_installed` | error | runtime-host (`lab_module_`) | neither option installed; any non-handshake request on the Lab channel | `operation` | U-6 |
| `lab_module_option_disabled` | error | runtime-host (`lab_module_`) | the Lab is installed but not the kind's option | `operation`, `lab_option` | U-6 |
| `upgrade_state_entered` | success | runtime-host (`upgrade_state_`) | `enter`: drained, own close accepted | `transition_id` | U-4 |
| `upgrade_state_cancelled` | success | runtime-host (`upgrade_state_`) | `cancel` took effect | `transition_id` | U-4 |
| `upgrade_state_released` | success | runtime-host (`upgrade_state_`) | `leave`: the new Runtime is released | `transition_id` | U-4 |
| `upgrade_state_requester_disconnected` | warning | runtime-host (`upgrade_state_`) | the `enter` connection closed before the close was accepted; Aborted's `failure_code`, never a reply | `transition_id` | U-4 |
| `upgrade_state_aborted` | error | runtime-host (`upgrade_state_`) | `enter`'s reply when a `cancel` withdrew it during the drain | `transition_id` | U-4 |
| `pack_registry_loaded` | success | runtime-host (`pack_registry_`) | load succeeded | `package_id` | U-3 |
| `pack_registry_unloaded` | success | runtime-host (`pack_registry_`) | unload completed (runs stopped first, dispatches withdrawn, runs still stopping marked) | `package_id` | U-3 |
| `pack_registry_entry_unknown` | error | runtime-host (`pack_registry_`) | the package is in neither the table nor the configuration | `package_id` | U-3 |
| `pack_registry_entry_invalid` | error | runtime-host (`pack_registry_`) | a replacing binding changes the run mode | `package_id` | U-3 |
| `pack_registry_not_loaded` | info | runtime-host (`pack_registry_`) | a Runtime requester met an unloaded package: deferred or skipped | `package_id`; optional `instance_id` | U-3 |
| `pack_registry_dispatch_withdrawn` | info | runtime-host (`pack_registry_`) | an unload withdrew an admitted, unstarted dispatch | `package_id`; optional `request_id`, `instance_id` | U-3 |
| `client_channel_unsupported` | warning | runtime-client (`client_channel_`) | an old actingd dropped the connection on BindChannel; the client goes on unbound | `operation` | U-2 |
| `client_channel_refused` | error | runtime-client (`client_channel_`) | the client library's Lab-only session check: the channel is not `lab` | `operation`, `channel` | U-6 |
| `lab_tool_option_disabled` | error | actingctl (`lab_tool_`) | an MCP option tool whose option is not installed | `operation`, `lab_option`, `mcp_blocker` (= `installer_lab_option`) | U-3 |
| `mcp_tier_retired` | warning | actingctl (`mcp_tier_`) | mcp-serve got the retired `--tier`; it starts as usual | `operation` | U-3 |
| `lab_command_option_disabled` | error | actinglab (`lab_command_`) | an actinglab offline package tool without option one | `operation`, `lab_option` | U-3 |

- `pack_registry_loaded` carries `package_id` only (M-PKG). Its result is
  `PackLoaded{key, named_by_catalog?}` (U-1).
- U-0 registers the keys `channel` and `lab_option` and their vocabularies of the
  same names, whose sources are U-1's `RuntimeChannel` and `LabOption` (coordinator
  decision 2, 10-12).
- Each prefix belongs to one crate: `lab_tool_` is actingctl's (its existing
  `lab_tool_unavailable` is in it), `lab_command_` actinglab's, `lab_module_`
  runtime-host's.
- Not adopted: `pack_registry_unload_busy` (an unload never refuses a package in use)
  and `setup_lab_developer_conflict` (acsetup has no developer option).

**Codes of the UI** (the UI's table, U-5; listed here for the installer's contract):

| Code | Category | When | Values |
|---|---|---|---|
| `setup_lab_choice_required` | error | a headless upgrade of an old installation (`actinglab.exe` present, neither key in the configuration) without `--lab` | `operation` |
| `setup_lab_options_locked` | error | a `--commit-config` proposal changes a Lab option | `config_path` |
| `setup_lab_tool_source_missing` | error | the Lab-options-only path must place `actinglab.exe` and finds no Tools package of the installed release | `tag` |

**Registered by the slices that change the path**:

- U-4: the paths and texts of the upgrade-state refusals of section Upgrade state,
  Refusals; the codes and their domains are already registered.
- U-3: `lab_tool_unavailable` (registered by #378 A2a) changes its path: option tools
  now appear by option.

**Retired later** (status `retired` once the path changes; codes the catalog does not
hold yet are registered as retired then):

| Code | Retires at | `replaced_by` |
|---|---|---|
| `invalid_shutdown_origin` | U-6 | `channel_request_refused` |
| `invalid_resource_authoring_origin`, `invalid_signature_origin`, `invalid_runtime_debug_origin` | U-6 | `channel_request_refused` |
| `invalid_emulator_control_origin` | U-6 | `channel_request_refused` (emulator, self-check; discovery has no gate) |
| `invalid_scheduling_pause_origin` | U-6 | none |
| `invalid_install_transition_origin` | with InstallTransition | none |
| `invalid_governance_origin` | U-6 | `channel_request_refused` |
| `invalid_agent_dispatcher_origin` | U-6 | `channel_request_refused` |
| `governance_authority_required` | U-6 | none |
| `install_identity_required` | U-4 | none |
| `runtime_debug_origin_invalid`, `runtime_authoring_origin_invalid` | U-6 | `client_channel_refused` |
| `tier_not_enabled` | U-3 | `lab_tool_option_disabled` |
| `invalid_governance_identity_card` and the identity card's refusals | U-6 | none |

Kept: `invalid_client_origin` (the envelope's own check), `fact_origin_mixed` (a content
rule), `c4_client_source_unsupported` (C4).

**Keys and vocabularies**: U-3, in the PR that deletes the MCP `Tier` enum, deletes the
key `required_tier` and the vocabulary `mcp_tier` (whose source is that enum), removes
the `mcp_blocker` token `server_tier` and adds the token `installer_lab_option`. If a
released snapshot holds them by then, they stay unused and lose their source, as #378's
rules decide.

## Hand-off to #383 AO-0

AO-0 registers every v0.12.0 persisted shape at once. From this model:

1. G14's two members: `channel` (vocabulary `channel`: `cli`, `mcp`, `ui`, `lab`) and
   the optional `via` (`mcp`, `ui`).
2. G14 action values: none new. `UpgradeState`, `LoadPack` and `UnloadPack` reuse the
   value AO-0 gives InstallTransition (suggested: the existing `runtime.action`);
   `ClearKeptFolder` reuses `artifact.retention`.
3. The pack table's workbench record: key `host.pack_registry.<package_id>` (startup
   and prerequisite packages by configuration item, shape AO-0's); one record per load
   or unload; value = the entry after the change: `unloaded` with the request ids of
   the stopped runs, or `loaded` with its binding (`package_id`, `operation_id`,
   `yield_points`, optional `scheduled_execution{mode, package_path}`). No package
   digest.
4. Tag classification: `host.pack_registry.` in `ledger_observer.tag.reobserve_prefixes`
   (memory only).
5. The two Lab options in `actingd-config` revision 5, the one v0.12.0 raise;
   `config.parameters` reports them; not in #387 D1's editable set.
6. Not AO-0's: the upgrade-state marker on the owner-journal close record (U-1 reader,
   U-4 writer; `owner.lock` is not the ledger). The number stays so that references
   match the model.
7. Not AO-0's either (coordinator decision 2, 10-12): U-0 registers the vocabularies
   `channel` (`cli`, `mcp`, `ui`, `lab`; source `RuntimeChannel`, U-1) and
   `lab_option` (`authoring`, `debugging`; source `LabOption`, U-1) and the keys of
   the same names, which the codes above carry.

The persisted invariants this model keeps: the approval records' provenance
`(Governance, User, Ui)`, the structure of `Status`, and `ContainedTaskCancellationReason`
(an unload reuses `ClientRequested`).

## Wording handed to #386 SR-0

Wording only; no behaviour changes. Line numbers are those of the scheduling model
`rd-1009/sched/model-sched-rules.md` (v1.4.2). §14.2 of the interface model applies.

- Blackout rule: "the blackout and the gradual silence hold back only the requesters the
  Runtime starts itself; runs and operations that come through the Lab channel (option
  two) are always let through". Detection by requester kind is unchanged.
- T44, T45 (:1063-1064): the entry is "the Lab channel's option two (`run start`,
  `lab run`, and `ac_run_pack` forwarded to it)".
- :158, :612: "a direct run by a person, an agent or the Lab is an instruction" → "is a
  Lab option-two operation, let through".
- :606: "a manual reconnect self-check is an instruction, done" → "is a Lab option-two
  operation, done (it cannot be sent when the Lab or option two is not installed)".
- :616-619 (today's three direct-run entries, :618 "operator-tier `ac_run_pack`"): →
  "once the model lands, one entry: the Lab channel's option two: actinglab `run start`,
  `lab run`, and MCP `ac_run_pack` forwarded to it (no tiers)"; requester kinds and the
  blackout test unchanged.
- :1259 (G2's answer): "three entries" → "only Lab option two once the model lands".
- :738: "MCP `ac_pack_schedule_set` / `_get` (operator tier, like `ac_targets_set`)" →
  "MCP general tools (no tiers)"; the CLI splits into `pack-schedule set` and
  `pack-schedule get`.
- :721, :750: the resource-target write is a general instruction (§14.2): actingctl and
  MCP `ac_targets_set` (a general tool, three counters, with or without the Lab). It is
  still the first layer and acts on #308's selection and scoring; unchanged otherwise.
  (This replaces the model body's "actinglab `targets set` and MCP `ac_targets_set`
  (Lab option one)".)
- :734 (SR-11) and :797 (SR-5a), "origin exception … accepts (user, ui), (cli, cli),
  (agent, adapter)": → "general by key family: register the family as general in
  `fact_key_family` (`runtime-interface.md`, Instance fact key families); a mix is
  refused with `fact_origin_mixed`; after U-6 there is no envelope-source exception. A
  slice that lands before U-6 accepts (user, ui) and (cli, cli) in the meantime, like
  today's offset exception, and U-6 deletes it". MCP's general tools write `(Cli, Cli)`
  from U-6, so (agent, adapter) is not needed.
- T40 (SR-11), T21 (SR-5a): "the three counters are accepted; from U-6 the Lab channel is
  refused with `channel_request_refused`"; the mixing and validation half is unchanged.
- The new deferral code `pack_registry_not_loaded` enters the evaluation closure before
  `pack_schedule_closed`; `pack_registry_dispatch_withdrawn` enters the reason-code
  table (:916-918).

## Installer page (UI repository, U-5)

The specification the UI channel implements; the page's name, an English version and
its number in the step bar are settled with the UI channel.

- **Place**: a page between "0 location" and "1 install/upgrade". The choice joins the
  same installation or upgrade transaction: one new configuration generation, one
  restart through the upgrade state.
- **Content**: the page name, two checkboxes and the page's note line (which reports
  only why "next" failed). No consequences and no list of what an option opens (Alice:
  「以上勾选这个都不显示后果或操作，当做门槛」).
- **Labels**, verbatim:
  1. option one: 「我已安装智能体的harness，并且知晓智能体控制会有一些异常或我不想要的结果」
  2. option two: 「勾选此项即代表，我/智能体是调试者/我是高级用户，我/智能体知道我/智能体在做什么，我/智能体能判断我需要什么。」
- The two are independent and may both be ticked; ticking either installs the Lab
  (places `tools\actinglab.exe`); ticking neither does not install it.
- **Defaults**: a fresh install ticks neither; an upgrade preselects the installation's
  current options; the first upgrade of an installation that has neither key ticks
  neither.
- **Headless** (`--plan`, `--yes`): `--lab <none|authoring|debugging|authoring,debugging>`.
  A fresh install without it is `none`; an upgrade without it keeps the current options;
  the first upgrade of an old installation (`actinglab.exe` present, neither key)
  without it is refused with `setup_lab_choice_required`, changes nothing and lists
  `--lab` among the arguments still needed.
- **Developer channel**: only in the pwsh and bash one-click deployment scripts
  (`-Developer`, `--developer`). It bypasses every threshold by passing everything
  acsetup needs, both options included (`--lab authoring,debugging`), and says so in
  its own output and deployment record. Neither the installer's pages nor acsetup have a
  developer option.
- **Transaction**: place or retire `tools\actinglab.exe` (retire = move into the backup;
  root tools are never deleted); write the ticked keys (only `true`) into the new
  configuration generation, only when the target Runtime speaks `actingd-config` 5;
  restart through the upgrade state. Failure rolls back as today. This is the only path
  that changes the two keys.
- **Lab options only** (a new maintenance path, like "resource-only update"): page 0 →
  the Lab page → apply; close and start through the lifecycle (the upgrade state for a
  Runtime speaking install-control 2), holding the writer lock from before the close to
  after the start. `actinglab.exe` is needed only when going from no option to one, and
  comes only from the installed release's Tools package (`<root>\downloads\<installed
  tag>\`, a release folder the person gives, or online when the latest release is the
  installed tag), checked as at installation (`SHA256SUMS`, `MEMBERS.json` equal to the
  selected generation's, the zip's `BUILD-MANIFEST`); never the copy in the backup.
  Otherwise `setup_lab_tool_source_missing`, nothing changes.
- `--commit-config` refuses a proposal that changes either key
  (`setup_lab_options_locked`) and commits other changes as today.
- `actinglab.exe` becomes an optional member of the Tools installation.
- acsetup speaks install-control [0, 2] and uses only the upgrade state with a Runtime
  that speaks 2 (section Upgrade state, with its time limits and Fail Loud table).
- Every flow that closes the Runtime and starts it again holds `install\writer.lock` from
  before the close until after the start; `maintenance::Transaction::commit` takes the
  held writer.
- The console binds `ui` on both connections, sends request shutdown as today, and adds
  the new codes to its name table.

## Strength of the identity

- A channel is a declaration, not authentication: loopback, no credentials (the shared
  secret was ruled out on 09-15). Any local process can bind any channel. The gate
  prevents misuse (a script sending taps through the CLI, an MCP read connection sending
  a debug operation), not malice.
- The two Lab options are enforced by the Runtime, not by the frontends: a copied-back
  `actinglab.exe` is still refused. Only the installer's Lab transaction changes them;
  editing a configuration generation by hand is not prevented.
- The installer page is a threshold, not authentication: its labels say "I/the agent",
  and whoever ticks counts.
- After U-6 the Runtime's identity rules are four, all by channel: option kinds only on
  the Lab channel and only with their option installed; general instructions only on the
  counters; the whole Lab channel closed when neither option is installed; external
  approvals on no channel. Approval records keep their provenance `(Governance, User,
  Ui)`.
