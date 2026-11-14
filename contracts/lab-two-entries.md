# The Lab and the UI: two entries (Workflow #434)

Workflow #434, 0.12 series, before 1.00. actingd's door takes two kinds of client: the **Lab**,
which has no permissions, and the **UI**, the one entry with permissions. The CLI and the MCP
server become modules of the Lab, `actingctl` goes, and the rescue and installation commands join
the Lab. This document is the contract that slices LAB-1 to LAB-7, UI-L1 and UI-L2 implement; it is
written by slice LAB-0, which changes no code. A behaviour described here exists only from the
slice named for it; until then the contracts it replaces stay in force as they are (section 11).

Alice's words (2026-11-08, coordinator window), among others:
「我计划现在统一一下MCP还有CLI这两个统一并入Lab，作为可拆卸模块并且直连ActingCommand的CTL，CTL处分割出来一个小接口，作为专属UI部分的连接口，Lab不设权限管理，仅对这个UI用的小接口做权限管理，如果是这样呢？」;
「…CTL这个模块可以直接砍掉，直接接入 acting D这个转发台，也就是转发台下只分两种，一种就是来自 Lab，一种就是来自 UI。」;
「救急和安装的功能那就直接并入LAB模块。 并且不可逆的操作，直接记发起者即可，不用加参数或者二次确认」.
Her answers of 23:2x to the design's four questions (board #434): 「1，不开。」;
「2，可以考虑保留，但是改成有的话就激活制包就行」;
「3，只记来自哪个组或者哪个组下的具体模块名就行，比如UI比如Lab-MCP或者Lab-CLI，或者Lab-看门狗」;
「4，可以直接移除」. The coordinator's reading of them (design §7), not her words: no separate
port for the UI (section 2.1); the authoring
option stays and only means "installed" (section 8.3); the initiator is one word naming the group
or the module (section 4); guards and the console's second press go (section 6).

Source: the design `rd-1108/arch-013/design-lab-ui.md` (read on main `2ea50cf5`), whose §7 (the
answers above) wins wherever it differs from its earlier sections; coordinator work order
`rd-1108/arch-013/brief-lab0.md`. The operation table v1 is the machine-readable file
[lab-operations.json](lab-operations.json) (section 9). The slices and their order are in
[v012-register.md](v012-register.md) §1.287; the planned interface revisions in
[component-interfaces.md](component-interfaces.md), "Planned: Workflow #434".

| Slice | Repository | What it turns on |
|---|---|---|
| LAB-0 | Runtime | this contract, the operation table v1, the planned interface rows, the register section; no code |
| LAB-1 | Runtime | `crates/lab-ops`: the table, the default location, the initiator type; actingctl's read and write modules moved in |
| LAB-2 | Runtime | the CLI module: short words, output for people, `--json`, `actinglab serve --stdio`; actinglab's debugging and authoring commands on the table; its retired command words deleted |
| LAB-3 | Runtime | the rescue and installation module (section 7): the journal's initiator, no confirmation parameters (section 5), guards that report (section 6) |
| LAB-4 | Runtime | `crates/watchdog`; `actingwatch.exe` does its own tick |
| LAB-5 | Runtime | `actinglab-mcp.exe`: tools from the table, no tiers; the MCP entry `g2`, a second program (section 8.1); both in the Tools archive inside `tools-layout` 3, and a running server never blocks a deploy (LAB-5G6, section 8.1) |
| LAB-6 | Runtime | the door of section 2 and the UI list of section 3; the initiator in receipts, G14 and the `host.config_overrides` rows (section 4.3). Not one step: cut into the slices LAB-6S0 to LAB-6S5 below, each of which leaves main deployable (board #434, 2026-11-13) |
| LAB-6S0 | Runtime | `deploy` and `rollback` through the Lab's MCP server. Way back: yes |
| LAB-6S1a | Runtime | the door also takes `BindChannel{channel: lab, initiator}` (section 2.2); `runtime-client` [3, 8]. Way back: yes |
| LAB-6S1b | Runtime | the origin gates of `RuntimeRequest::validate()` go (built, 0.12.58: section 2.4); their codes retire. Way back: yes |
| LAB-6S2-M | Runtime | the host's half of the move to `lab` (built, 0.12.59): on a `lab` binding that declares a word the Runtime records the channel that program's binding had on 0.12.58, by the word and the request's envelope (section 2.4, the map M), and receipts a request from any client envelope but the Lab's with it, without `initiator` (section 4.3, the receipt rule R); dormant until a program declares a word. Way back: yes |
| LAB-6S2-B1 | Runtime | the Lab's own programs (`actinglab`, its `serve`, `actinglab-mcp`, the rescue module, `actingwatch`) bind `lab` with their word and keep the envelope they sent on 0.12.58; LAB-6S2-M in the same Runtime build. Built, 0.12.59 (register §1.287, "Slice LAB-6S2-B1"): `actinglab`'s core rows (`Front::Actinglab`, (Lab, Lab)) and its own commands ((Lab, Lab)) with the process's word, `Lab-CLI` or the `Lab-制包` / `Lab-救急` of `ACTINGCOMMAND_INITIATOR`; `serve --stdio` the same, and with `--front mcp` `Lab-MCP`; MCP's `--via mcp` actinglab child `Lab-MCP` without `via` (`--via ui`, which no word names, binds as before); `actinglab-mcp` in its own process (`Front::Mcp`, (Cli, Cli)) `Lab-MCP`; the rescue module's own client ((Cli, Cli)) with the module's word, `Lab-MCP` for `actinglab-mcp`, `serve --front mcp` and the detached run; the watchdog's probe ((Cli, Cli), `Health` and `BindChannel` only) `Lab-看门狗` in `actingwatch`, else the word of the Lab program that reads the watchdog's status or starts the Runtime. `actingctl install` (the rescue module as `actingctl` calls it) and `actingctl`'s other paths through the code it shares with the Lab (`start`, `restart`, `watchdog` and the overview's `watchdog` block) declare no word in this slice and bind as on 0.12.58, `cli` without a word; LAB-6S2-B2 moves them, with the rest of `actingctl` and today's MCP server, which keep their binding until then. Way back: yes |
| LAB-6S2-B2 | Runtime | `actingctl` and today's MCP server (`actingctl mcp-serve`) the same: `lab` with `Lab-CLI` or `Lab-MCP`, the envelope they sent on 0.12.58; after LAB-6S2-M. Built, 0.12.59 (register §1.287, "Slice LAB-6S2-B2"): every connection of `actingctl`'s command lines declares `Lab-CLI`: its own commands ((Cli, Cli); (Agent, Adapter) for `agent-publish-facts` and `agent-apply-resource-targets`), the operation layer's commands (`Front::Actingctl`, (Cli, Cli)) and the code it shares with the Lab, (Cli, Cli): `install` (the rescue module as `actingctl` calls it; its journals still record no initiator), `start`, `restart`, `watchdog` and the overview's `watchdog` block; every connection of today's MCP server declares `Lab-MCP`: the kept one and each write's ((Cli, Cli); (Agent, Adapter) for the resource targets, the remedy choices and the evidence read confirmation), and its overview's `watchdog` block (the server says it is `Front::Mcp`); its actinglab children (`--via mcp`) bind `Lab-MCP` without `via` since LAB-6S2-B1. The Runtime records and answers every request of theirs on `cli` and `mcp` as on 0.12.58 (section 2.4, the map M; section 4.3, the receipt rule R). Since this slice no program of this repository binds `cli` or `mcp`; a `lab` binding without a word is left only where LAB-6S2-B1 keeps one (`--via ui`, and actinglab's own commands under a variable that names no word). Way back: yes |
| LAB-6S3 | Runtime | the UI list on `ui` (section 3, `ui_allows`; section 2.3 step 5): a `ui` connection is refused, `channel_request_refused`, every kind off the list and a setting outside the console's tiers; every other connection as before. Way back: yes |
| LAB-6S4a | Runtime | the five kinds deleted; `install-control` [0, 0]. Way back: yes |
| LAB-6S4b | Runtime | `cli`, `mcp` and a `lab` without a word refused; `via` off the wire; `runtime-client` [8, 8]; `actingd-config` 30. Way back: yes |
| LAB-6S5 | Runtime | the initiator in the ledger and in the settings rows; `ledger` 9. Way back: **no** |
| LAB-6S2-E | Runtime | the envelope becomes the channel's (section 2.4: `lab` → `(Lab, Lab)`), and the map M and the receipt rule R end; with it the host's evidence-read confirmation and its index take `(Lab, Lab)`, the senders drop the identity card's step, and the kept frames of CLI and MCP reads are decided. What a person then sees change comes with it, each the owner's to accept: the console's ledger page and Lab tab show the requests of `actingctl` and of both MCP servers as the Lab's, its settings page says "via Lab" for the rows they set, the runs they start have the origin `lab`, and the frames they read are kept as Lab frames. LAB-6S4b, which takes `via` off the wire, ends M's `via` cell with it. Way back: settled with the slice |
| UI-L1 | UI | the console on `runtime-client` 8: Lab children instead of actingctl, refusals shown, no second press (the stop note stays beside the button, section 6); the settings page shows a row's initiator, else its channel (section 4.3); the READMEs' install line names the rescue module's command |
| LAB-7 | Runtime | `apps/actingctl` deleted; `install-cli` 2 |
| UI-L2 | UI | after UI-2a: acsetup as the graphical shell of the rescue module; its `status`, `discover`, `shutdown`, `check-config` and `ledger verify` through actinglab, and no `install-transition` |

Where a later section says LAB-6 without a slice letter, it means the slice of LAB-6 in the table
above that turns that behaviour on; until it is deployed, the text it replaces stays in force
(section 11).

## 1 Words

- **Entry**: one of the two kinds of client the door takes, named by the channel a connection
  binds: `lab` or `ui`. Both use the one port.
- **Lab**: the operation layer `crates/lab-ops` and the modules on it (section 8). Its front ends
  are the CLI (`actinglab.exe`), MCP (`actinglab-mcp.exe`) and the line loop
  `actinglab serve --stdio`.
- **Operation**: one row of the operation table: one CLI form, one MCP tool, one serve call
  (section 9).
- **Initiator**: the one word that says where a request or an operation came from (section 4).
- **Irreversible operation**: an operation the table marks `irreversible` (section 5).
- **Guard**: a row of the guard table of [install-cli.md](install-cli.md), "Guards" (section 6).

## 2 The door

### 2.1 One port

- One loopback TCP door, as today: its address, owner epoch and pid are in the state root's
  `runtime-info.json`, the frame limit stays 1 MiB, and `HostShared::process_request_observed`
  stays the one gate. There is no second port (Alice's answer 1): the UI's "small interface" is the
  UI list of section 3 on the `ui` channel.
- A channel is a declaration, not authentication ([runtime-interface.md](runtime-interface.md),
  "Strength of the identity", holds): any local process may bind `lab` and reach every operation.
  The UI list prevents misuse, such as a console fault sending a debugging operation, not malice.

### 2.2 The handshake

Each step names the slice of LAB-6 that turns it on.

1. `Health` (the owner epoch check), then `BindChannel` once:
   - `BindChannel{channel: lab, initiator}`, `initiator` one of the five Lab words of section 4
     (from LAB-6S1a; until LAB-6S4b a `lab` binding without a word, with or without `via`, is
     taken as before);
   - `BindChannel{channel: ui}`, without an initiator: the connection's initiator is `UI`,
     implied by its channel and never written on the wire.

   From LAB-6S1a an `initiator` present on `ui`, `cli` or `mcp`, present together with `via`, the
   word `UI`, or a word not of section 4 is refused `invalid_bind_channel`, and the connection
   stays unbound. From LAB-6S4b an `initiator` missing on `lab` is refused the same way.
2. The answer is `ChannelBound{channel, initiator?}` (from LAB-6S1a): it echoes the word of a
   connection that declared one and carries no `initiator` otherwise, on `ui` included. A binding
   without a word is answered byte for byte as before LAB-6S1a.
3. From LAB-6S4b `cli` and `mcp` are no longer bound: `BindChannel` with either is Denied
   `channel_request_refused` (`channel` the value asked, `operation` `bind_channel`), and the
   connection stays unbound. Ledger records that carry `cli`, `mcp` or `via` stay readable
   (section 10).
4. A second `BindChannel` on one connection is `channel_binding_exists`, as today.
5. From LAB-6S4b `via` (`ChannelVia`) leaves the request, the answer and the connection table.
   From LAB-6S1a the table keeps each connection's channel and initiator in memory only; an entry
   leaves with its connection; no status read reports it.
6. The runtime client's three ways of opening a connection keep calling one handshake function,
   so a reconnect keeps the channel and, from LAB-6S1a, the initiator; the player hold's
   temporary client binds both too.

### 2.3 The gate's decision

From LAB-6, in order; the first match decides. It replaces runtime-interface.md, "The gate's
decision (from U-6)", which is not built.

1. A handshake kind (`Health`, `BindChannel`): take it.
2. The connection is not bound: `channel_binding_missing`, in the receipt only.
3. A retired kind: `channel_request_refused`. No kind is retired once LAB-6 deletes the five of
   section 2.5; the step stays for the next retirement.
4. `lab`: take it. No Lab option, block or class is consulted.
5. `ui`: take it only when its kind is on the UI list (section 3). A `ConfigureSettings` also needs
   every change's key to have `ui_tier` `daily` or `advanced` and to be settable
   ([runtime-config.md](runtime-config.md), "Console tiers"). Otherwise
   `channel_request_refused` with `channel` `ui` and `operation`, and for a setting also
   `config_key`, the first key refused.

Step 5 is in force from LAB-6S3 (Workflow #434, 0.12.58; the coordinator's rulings of 2026-11-13
on board #434, not the owner's words), on a connection bound to `ui` however it came to be bound:
by its `BindChannel`, or, until LAB-6S4b, by the envelope of its first request (runtime-interface.md,
"Transition"). The handshake kinds of step 1 are `Health` and `BindChannel`; the identity card,
which the request table files with them, is no kind of the list, and `ui` refuses it. A
`ConfigureSettings` is judged key by key in the request's order, a change that clears a key as one
that sets it; a key the key table does not have has no tier and is refused; "settable" is the key
row's static property, so a change the Runtime's state then refuses is answered by the checks
behind the gate with their own codes. Nothing of a refused request is applied. Steps 2 and 3 come
with LAB-6S4b; until then nothing is refused by channel on an unbound connection or on one bound to
`lab`, `cli` or `mcp`.

The checks behind the gate do not change, except the origin gates of section 2.4. The Runtime's
own work (policy dispatch, the recovery ladder, startup packages, emulator autostart, monitor
capture, pacing, host shutdown handling) is no request and never passes the gate.

### 2.4 The envelope

- The envelope is a fixed function of the channel, and the runtime client is the one place that
  builds it: `lab` → `(Lab, Lab)`, `ui` → `(User, Ui)`. The Runtime judges a request by its
  channel, never by its envelope.
- **Until LAB-6S2-E: the envelope of 0.12.58 on `lab` with a word, and the map M** (Workflow #434
  LAB-6S2-M, Runtime 0.12.59; register §1.287, "Slice LAB-6S2-M"; the coordinator's rulings of
  2026-11-13 on board #434, not the owner's words). LAB-6S2 is cut into LAB-6S2-M, LAB-6S2-B1,
  LAB-6S2-B2 and LAB-6S2-E (the slice table): where this contract says LAB-6S2 of the binding, read
  LAB-6S2-B1 and LAB-6S2-B2; of the envelope, LAB-6S2-E. Until LAB-6S2-E a client that binds `lab`
  with a word may send the envelope it sent on 0.12.58, and LAB-6S2-B1 and LAB-6S2-B2 move every
  program of the Runtime repository onto `lab` that way. The Runtime judges such a request by its
  envelope exactly as on 0.12.58 (nothing in this section changes for it), and wherever it writes
  the request's channel (the command record's `channel` and `via`, the package debug record's, the
  `host.config_overrides` row's `channel`) it writes the binding M gives, the one that program's
  binding had on 0.12.58. M (`ChannelBinding::recorded`, contract crate) applies only to a `lab`
  binding that declares one of the five Lab words, by the word and the source of the request's
  envelope:

  | Word \ envelope source | `Lab` | any other client source (`Cli`, `Adapter`, `Ui`) |
  |---|---|---|
  | `Lab-MCP` | `lab`, `via` `mcp` | `mcp` |
  | `Lab-CLI`, `Lab-制包`, `Lab-救急`, `Lab-看门狗` | `lab` | `cli` |

  Every other binding (`lab` without a word, with or without `via`, `cli`, `mcp`, `ui`, and the
  binding an unbound connection's first request makes) is recorded as it is. A request whose
  envelope's source lies outside the client set (`Runtime`, `Scheduler`, `Device`, `System`) gets
  from M the channel of the other non-Lab envelopes, `cli`, or `mcp` under `Lab-MCP`, so on a
  `lab` binding with a word its refusal `invalid_client_origin` is answered with that channel and
  without the word (section 4.3, the receipt rule R). M never yields `ui`, so the gate's step 5
  (section 2.3) judges the connections bound to `ui` and no other. The connection's own binding
  does not change: `channel_binding_exists` names `lab`, and the answer to `BindChannel` echoes
  the word. Its receipts follow the receipt rule R (section 4.3). LAB-6S4b, which takes `via` off
  the wire, ends M's `via` cell; LAB-6S2-E ends M.
- LAB-6 deletes the origin gates of `RuntimeRequest::validate()` (emulator control, the
  self-check, the player hold, the agent wake test, the origin exceptions of fact families and the
  rest); the content and shape rules stay. Whoever sent a request is in its G14 record.
  **Built by LAB-6S1b** (Runtime 0.12.58; register §1.287, "Slice LAB-6S1b"; the coordinator's
  work order of 2026-11-13 on board #434, not the owner's words). The gates are gone: shutdown
  and the install transition (person or operator only), the authoring, signature and debugging
  kinds (Lab only), the governance card (person or operator) and the approval decision (person
  only), emulator control, discovery, the self-check and the drill (person or operator), the
  scheduling pause and resume (person or operator), the agent wake test (person or operator),
  fact publication by family (the agent dispatcher, or a person or operator for the families
  with the origin exception), the agent dispatcher's kinds, `ApplyResourceTargets` among them
  (the agent dispatcher only), and the player hold (person, operator or Lab). Their codes retire
  in 0.12.58 but `invalid_governance_origin`, which the host's own check of the card still emits
  (section 12). What stays: an envelope outside the client set is `invalid_client_origin`, a
  shape rule of the envelope; every content and shape rule, `fact_origin_mixed` among them; and
  what is judged by the envelope outside `validate()`, none of it a gate: the frame pin's kind
  (`Lab` for a `(Lab, Lab)` request), the C4 intent's record type and origin module
  (`c4_client_source_unsupported`) and the receipt's default detail. The host's own checks of
  the kinds LAB-6S4a deletes stay with them: a governance card only from `(User, Ui)` or
  `(Cli, Cli)` (`invalid_governance_origin`), an approval decision only from `(User, Ui)` on a
  connection whose card was accepted (`governance_authority_required`), an install transition
  other than its Query only on such a connection (`install_identity_required`). So since
  LAB-6S1b the Runtime takes from the Lab's `(Lab, Lab)` envelope what it refused before:
  `actinglab`'s `pause`, `resume`, `shutdown`, `discover`, `emu start|stop|restart`,
  `emu selfcheck`, `drill`, `wake-test`, `facts publish`, `offset`, `remedy set|lift` and
  `targets set`; through `actinglab-mcp`, whose core rows send `(Cli, Cli)` until LAB-6S2,
  `targets set` and `facts publish` of a family without the origin exception. No program changes
  what it sends (LAB-6S2 does); `evidence read` from the Lab is still refused, by the host's
  check of its identity card.
- Frame pins follow the channel as runtime-interface.md, "Blocks", says: a frame a `lab` request
  pins is pinned `Lab`, attached to its debug run and kept in `kept\`; a `ui` request's frames are
  pinned `Explicit`. So a manual run (`run start`) keeps its frames, the direction
  runtime-interface.md, "Envelope", set; so do `frame` and `frames` from the CLI and MCP, which pin
  `Explicit` today (open point 3).

### 2.5 What LAB-6 deletes

- The kinds `DeclareGovernanceIdentity`, `RecordApprovalDecision`, `InstallTransition`,
  `UpgradeState` and `UpgradeStateStatus`, with their results.
- `ChannelVia` and every `via` member.
- The Lab options' part in the gate: `lab_module_debugging` retires and `lab_module_authoring`
  means only "listed by default" (section 8.3). The host then emits neither
  `lab_module_not_installed` nor `lab_module_option_disabled` (section 12).

### 2.6 What a refusal answers and records

- The receipt is Denied and its outcome is the specific code with its values, not only
  `invalid_request`.
- A refused write is recorded `command.rejected` with its channel and initiator; a refused pure
  read is only in its receipt; a request on an unbound connection is only in its receipt
  (runtime-interface.md, "Records").
- **What LAB-6S3 records** (0.12.58). A request the gate refuses on `ui` that G14 copies (the
  request table's `g14_copied`: a write, or a read that writes a record) is recorded
  `command.rejected` with the channel `ui` and the refusal as its outcome, the record the
  receipt's terminal; the member `channel` every command record has carries it, so no record's
  shape and no `ledger` revision changes. A refused pure read is only in its receipt. The
  initiator joins the record with LAB-6S5. `operation` names the request kind by a registered
  location: the kind's spelling on the wire where that spelling is one, otherwise the name the
  runtime client or the contract gives the request (`runtime_input` for `Input`,
  `runtime_project_interface`, `request_runtime_shutdown`, and so on: the host's `kind_location`).
- The console shows the code and the operation (Fail Loud, UI-L1) and never sends a refused
  request again as another kind.
- No new code (section 12).

## 3 The UI list

A static table `ui_allows(kind) -> bool` in the contract crate, beside `interface_class` (LAB-6).
No roles, users or tokens. It equals the table's `ui_kinds` (section 9), which LAB-6's
`gate_two_entries` asserts. Since LAB-6S3 (0.12.58) the function exists
(`actingcommand_contract::ui_allows`, the transitional `RecordClientAction` included) and the gate
holds `ui` to it (section 2.3 step 5); `lab_ops_ui_allows_is_the_tables_ui_kinds` holds it equal to
`ui_kinds`.

| Kind | Class | Note |
|---|---|---|
| `Status`, `RuntimeFactSnapshot`, `ScheduleExplanation`, `StartReport`, `QueryEvents`, `ReadMaterial`, `DiscoverInstances`, `ObserveReadonly` | read | the console sends them today |
| `MonitorStatus` | read | added to the design's list so that the operation `status` is whole on this entry |
| `InstanceFactSnapshot`, `PerformanceStatus`, `FactRefreshStatus`, `RoutineSummary`, `ResourceTargetView`, `PackScheduleView`, `CaptureSequence` | read | on the list before the console uses them (#429's shapes) |
| `PauseScheduling`, `ResumeScheduling`, `ControlEmulatorInstance`, `SelfCheckInstance`, `RequestShutdown` | write | the console sends them today |
| `ApplicationLifecycle` | write | the console sends it today; the kind acts only on the instance's assigned application |
| `ConfigureSettings` | write | the console sends it today; every key `daily` or `advanced` and settable (section 2.3) |
| `StopInstanceRun` | write | built by #429 R-6's first slice (0.12.53) and R-6b (since 0.12.55; `v012-register.md` §1.314, §1.316): it stops a client's direct run and a run the scheduler started (a fact refresh read included), and the Runtime resets the device itself; a stopped scheduled run settles as a stop, neither a success nor a failure of its task; only a run the Runtime started for itself is refused, with `run_origin` `host`. A stop ends this run only: a task whose trigger still holds is dispatched again once its cooldown has passed (at the next evaluation when the stopped run outlasted it); a closing row (`pack-schedule set`) or a pause keeps it from running (runtime-interface.md, "Workbench request kinds") |
| `EditPackSchedule` | write | built by #429 R-8 (0.12.55), with the read `PackScheduleView`: it replaces one task package's rows in one scope's pack schedule list, a compare-and-swap with `expected_version`; a run in progress is not stopped (instance-fact-store.md, "Pack schedule facts") |
| `ApplyResourceTargets` | write | on the list since Workflow #455 T1-R (0.12.60; `v012-register.md` §1.351), before the console sends it: the whole resource target document of one instance, the one `ResourceTargetView` reads ([resource-targets.md](resource-targets.md), "Entry and receipts"), a compare-and-swap with `expected_version` (the view's `active.version`, `0` without `active`; another version is `fact_observation_version_conflict` with `current_version`, nothing written). The owner asked that the console set an application's goals as the Lab does (board #455, step 1); the coordinator's ruling on board #455, not the owner's words, reverses LAB-6S3's decision that refused the kind here |
| `RunInstanceTask`, `EditResourceTarget`, `AcquirePlayerHold`, `ReleasePlayerHold` | write | on the list before they are built (#429 R-5, R-13c, R-7b); until then the Runtime answers them `not_implemented` |
| `RecordClientAction` | write | transitional: the console's button trace, until UI-L1 ships; LAB-7 takes it off the list, since the G14 record's initiator replaces it |

- **Everything else is refused** on `ui`: a run by path, input and leases, stopping a run with the
  client's reset, reset, Lab operations, the Lab observation page, artifact recognition,
  signatures, debug packages, evidence export, pin release, drills, pack load and unload, fact
  publishing (`PublishFact`, `PublishFacts`), monitor settings, the agent wake test, event
  subscriptions, the project projection, the planning session and the authoring and debug events.
- **A clean mapping.** Every kind on the list is a request of an operation whose `entries` hold
  `ui` (section 9). The console builds those requests and reads their answers with `lab-ops`' own
  types, a Runtime crate it pins as it pins `runtime-client`, so the name, the parameters and the
  output are the CLI's and MCP's.
- **Not through this entry.** The console's cold ledger read (so it reads the ledger while
  actingd is stopped), and the Lab processes it starts (section 4.2): the launcher's `up` and
  `unlock`, the settings page's `packs` and `alive` (UI-L1), and acsetup's `status`, `discover`,
  `shutdown`, `check-config`, `ledger verify`, `new` and `deploy` (UI-L2, after UI-2a). Those are
  `lab` requests.
- The authoring workbench (#346) does not use this entry either: it drives
  `actinglab serve --stdio`, and its device operations are `lab` requests (section 4.2). The UI's
  permissions cover the console's entry only.

## 4 The initiator

### 4.1 The words

One string, compared byte for byte (UTF-8), from a closed list of six (Alice's answer 3):

| Word | Entry | Who |
|---|---|---|
| `UI` | `ui` | the console (acui); fixed by the binding, never declared |
| `Lab-CLI` | `lab` | `actinglab.exe` started from a command line: a person or a script |
| `Lab-MCP` | `lab` | `actinglab-mcp.exe`, for an MCP client |
| `Lab-制包` | `lab` | the Lab process the UI starts for its authoring workbench (`actinglab serve --stdio`) |
| `Lab-救急` | `lab` | a Lab process the UI starts outside its workbench: the launcher, acsetup, the settings page |
| `Lab-看门狗` | `lab` | `actingwatch.exe` |

- Nothing else is recorded: no client name (the MCP entry's `--client`), no session or agent
  channel name, no system user. This replaces the design's `initiator {module, client}`.
- The word names where the request came from, not the module that serves the operation: an
  authoring or rescue operation typed at a command line is `Lab-CLI`, and through MCP `Lab-MCP`
  (open point 1).
- From LAB-6S1a the six words are a closed vocabulary of `runtime-client` 8, written once, in the
  contract crate (`Initiator`, which `crates/lab-ops` converts its own type to, so the recordings
  and the installation journal write the same strings); from LAB-6S5 also of `ledger` 9. Adding or
  renaming one later raises both (open point 2); until LAB-6S1a is released a rename changes one
  line of that type, this text and the table.

### 4.2 How a process picks its word

- `actinglab-mcp.exe`: `Lab-MCP`. `actingwatch.exe`: `Lab-看门狗`. The console binds `ui`.
- `actinglab.exe` (the CLI, and `serve` without `--front`): `Lab-CLI`, unless the environment
  variable `ACTINGCOMMAND_INITIATOR` names `Lab-制包` or `Lab-救急`. The UI sets it for every Lab
  process it starts: `Lab-制包` for its workbench's `serve`, `Lab-救急` for every other one.
- Any other value of the variable, `UI`, `Lab-看门狗` and `Lab-MCP` without the marker below
  included, is `actingctl_usage_invalid` with `variable` `ACTINGCOMMAND_INITIATOR` (the code moves
  to actinglab with LAB-7, section 12) wherever the process takes its word: a command line of the
  CLI module (a core or a rescue row) and `serve --stdio` refuse it before anything is sent or
  written, and a command that adds a step or a mark to a Lab recording before it writes one.
  actinglab's own other commands take no word from the variable and do not refuse it: their
  connections bind the Lab channel without a word, as on 0.12.58, and send (Workflow #434
  LAB-6S2-B1; register §1.287, "Slice LAB-6S2-B1", decision (b)). `actingctl` does not read the
  variable: in LAB-6S2-B1 its connections declared no word and bound as on 0.12.58. Since
  LAB-6S2-B2 (0.12.59; register §1.287, "Slice LAB-6S2-B2"; the coordinator's rulings of
  2026-11-13 on board #434, not the owner's words) a command line of `actingctl` declares
  `Lab-CLI` on every connection, `actingctl install` (the rescue module as `actingctl` calls it,
  whose journals still record no initiator: the face of `install-cli` revision 1) and its other
  paths through the code it shares with the Lab included, and `actingctl mcp-serve`, today's MCP
  server, declares `Lab-MCP`, the probe of its overview's `watchdog` block included.
- **The detached run's marker** (Workflow #434 LAB-6S0, 0.12.58; the coordinator's ruling of
  2026-11-13 on board #434, not the owner's words). `actinglab.exe` takes `Lab-MCP` from the
  variable only beside the marker `ACTINGCOMMAND_DETACHED_RUN` set to `rescue`, compared byte for
  byte (`lab_ops::initiator`, `Initiator::of_lab_process_marked`). The rescue module's detached
  start (`--detach`, section 5) is the one place that sets it: for an operation whose initiator is
  `Lab-MCP` it sets the word and the marker for the `actinglab.exe` it starts, so the detached run
  journals `Lab-MCP`; for `Lab-制包` or `Lab-救急` the word alone, for `Lab-CLI` neither. Nothing
  else sets it: not the CLI's or `serve`'s own child starts, not the MCP server, whose
  `serve --front mcp` children keep `ACTINGCOMMAND_INITIATOR` removed. The marker alone, beside
  another word, or with another value changes nothing. `serve --front mcp` refuses the variable
  set to anything, as below, whatever the marker says.
- `actinglab.exe serve --stdio --front mcp` (LAB-5G4a; the option, section 8.2): `Lab-MCP`, the
  word of the MCP server whose child it is. With the option the variable, set to anything, is
  refused before the loop starts (`actingctl_usage_invalid` with `variable`
  `ACTINGCOMMAND_INITIATOR`): the two would name different words, and the MCP server removes the
  variable for its children. No other command and no operation's command line takes the option.
- Like the channel, it is a declaration, not authentication.

### 4.3 Where it is recorded

- **Online.** It is bound with the connection (section 2.2), and every request of the connection
  carries it: from LAB-6S1a the receipt gains `initiator` beside `channel` on a connection that
  declared a word (a `ui` connection's `UI` is implied by its channel and not written, so the
  console reads its receipts byte for byte as before), and `via` leaves the receipt with
  LAB-6S4b; from LAB-6S5 the G14 records `command.received`, `command.validated` and
  `command.rejected` gain `initiator` beside `channel`, in place of `via`; so does the player
  hold's source (`PlayerHoldSource` and its ledger copy). Effect records gain no member: who sent
  a request is found through its request id in its G14 record, as today (LAB-6).
  - **The receipt rule R** (Workflow #434 LAB-6S2-M, Runtime 0.12.59, until LAB-6S2-E; register
    §1.287, "Slice LAB-6S2-M"; the coordinator's rulings of 2026-11-13 on board #434, not the
    owner's words). On a `lab` binding that declares a word, the receipt of a request whose
    envelope's source is not `Lab` carries the binding the Runtime records for it (section 2.4,
    the map M): `channel` `cli`, or `mcp` under `Lab-MCP`, without `via` and without
    `initiator`. The receipt of a request whose envelope's source is `Lab` is LAB-6S1a's: `channel`
    `lab` and the word, without `via`. So a program that moves onto `lab` with its word and keeps
    its 0.12.58 envelope (LAB-6S2-B1, LAB-6S2-B2) reads the receipt of each request it sends with
    the CLI's or an agent adapter's envelope byte for byte as on 0.12.58: acsetup decodes the
    receipts that `actingctl`'s `install-transition` and `request-shutdown` print into a receipt
    type without `initiator` that refuses unknown members. One cell of M is not 0.12.58's: under
    `Lab-MCP` the rescue module's requests (the rescue rows of `actinglab-mcp` and of
    `serve --front mcp`, and the detached run) are answered with `channel` `cli` on 0.12.58 and
    with `mcp` under M; those requests write no record that carries a channel, so only their
    receipt's `channel` member differs, and acsetup, the released console's installer helper,
    reads `actingctl`'s receipts, which are not of that cell. The answer to `BindChannel` is not
    under R: it carries the binding with its word, whose echo the client requires. Every binding
    without a word is receipted as before. R ends with LAB-6S2-E.
  - **Settings overrides.** Each `host.config_overrides` row
    ([runtime-config.md](runtime-config.md), "Facts"; register G4 and U2-5) gains `initiator`,
    written beside `channel` by every `ConfigureSettings` (LAB-6). LAB-6 writes `channel` `lab` or
    `ui` only, so the channel alone would show every Lab module as "经 Lab" on the UI's settings
    page, where it shows "经命令行" or "经 MCP" today. The settings page shows a row's initiator
    where it has one, else its channel (UI-L1).
  - **Rows written before LAB-6** (the coordinator's ruling). Production may hold them, written
    through MCP for example, and the first start after the LAB-6 deploy must not stop on them.
    Such a row is restored as written, without `initiator`. The restore check keeps accepting the
    four channel values it accepts today (`cli`, `mcp`, `ui`, `lab`). An `initiator`, when
    present, must be one of the six words of section 4.1; otherwise the start stops with
    `host_config_overrides_fact_invalid`, as for a bad channel. LAB-6 writes only `lab` or `ui`.
- **Offline.** The installation journal `install\operations\op-<ms>.json`
  ([install-cli.md](install-cli.md), "Documents") gains `initiator`, written before the first step,
  for every rescue operation that writes a journal: all irreversible ones and those that journal
  today (LAB-3). An operation that then sends requests to a running Runtime binds `lab` with the
  same word, so its G14 records carry it too.
- **Recordings.** Each mark and step of a Lab recording carries `initiator` (#350 option B, design
  §3.4); a step that reaches the device is also in G14. LAB-2 writes it (open point 6, the
  coordinator's ruling): the word of section 4.2 for `actinglab.exe` (`Lab-CLI`, or `Lab-制包` /
  `Lab-救急` from `ACTINGCOMMAND_INITIATOR`; `Lab-MCP` for `serve --stdio --front mcp`, LAB-5G4a,
  whose loop runs actinglab's own commands as MCP's actinglab child runs them), and `Lab-MCP`
  (section 4.1) when MCP's actinglab child runs the command (`--via mcp`, LAB-2 fix 1), carried by
  the recording lock and written on each step and mark the command adds
  ([lab-recording.md](lab-recording.md), "recording.json").
  The recording format changes directly: a recording without it does not decode.
- **The watchdog.** Its starts stay recorded in its own files as today. A request it sends beyond
  `Health` carries `Lab-看门狗`; today it sends none.

## 5 Irreversible operations

- **The list** is the table's `irreversible: true` rows: `deploy`, `rollback`, `convert`, `clean`,
  `undo`, `config replace`, `config restore`, `config file commit`, `instance add` (Workflow #421
  INST-5: its first slice commits the configuration it composes, as `config replace` does),
  `place`, `pack remove`, `pack restore`, `unlock`, `state-root claim` (Workflow #453 S12: it
  hands a state root over to this installation, the old record kept), `vision place` (Workflow
  #453 S9: it replaces a vision root's files, the replaced ones kept), `kept clear` and
  `resource import-mpe` (the last two reserved).
- **The rule:** record the initiator (section 4.3), nothing more. No parameter, no second
  confirmation, no plan id carried back by the caller. A plan id is still computed and journalled,
  and an operation still applies only the plan it computed under the writer lock. MCP marks these
  tools `destructiveHint`, and the table's `disruptive` rows too (section 9.1, LAB-5A).
- **Gone from the rescue module** (LAB-3; `actingctl install` keeps its revision-1 face unchanged
  until LAB-7 deletes it, so the console of today keeps working):
  - `--plan <id>` for apply: `deploy`, `rollback`, `config file commit`, `pack remove` and
    `place` plan and apply in one call, and `--plan` (a switch) asks for the plan only;
  - `--accept-ledger-one-way`: a one-way update records `one_way: true` in its journal;
  - `install clean --ui-confirmed <selection> --via ui|cli`;
  - `--known-pause`: every pause found on an instance is a known pause, kept, and listed in the
    answer and the journal; `release-hold` hands back only the operation's own hold;
  - `--ack-guard` (section 6).
- **`unlock`** passes no confirmation from its caller; it runs `actingd unlock-owner` and records
  the initiator. actingd's `--confirm-resources-released` goes with LAB-7, once UI-L1 no longer
  passes it.
- **Through MCP**, `deploy`, `rollback` and `convert` always run detached: they replace the
  programs MCP itself runs. Since Workflow #434 LAB-6S0 (0.12.58; the coordinator's ruling of
  2026-11-13 on board #434, not the owner's words) `actinglab-mcp` serves `ac_deploy` and
  `ac_rollback` on Windows (`convert` is not offered: its source is not built). The front gives
  the row's `detach` true, and the module starts the `actinglab.exe` beside the server (one that
  is not there is refused before the start, with its path) with the
  row's words, the initiator `Lab-MCP` and the marker of section 4.2, and answers at once:
  `detached`, the detached run's `pid` and `log`, and `next` naming `op status`, which follows the
  operation (its id in `unfinished` while it runs, in `last_operations` once it ended, its journal
  with `op`). The detached run journals `Lab-MCP`; until LAB-6 its requests to a running Runtime
  go on the CLI channel, as every request of the module does (section 7). Until LAB-7 production
  deploys through `actingctl install`, not through the rescue module and not through this server
  ([install-cli.md](install-cli.md), "Operational rule until LAB-7").

## 6 Guards report, never block

Alice's answer 4. From LAB-3, in the rescue module:

- Every guard row of install-cli.md, "Guards" (`memory`, `schedule_near`, `foreign_pauses`,
  `ladder_holding`, `adb`; `blocked_window` stays retired) is evaluated and reported as today, and
  never refuses. A row that fails or is not evaluated is noted, and the operation goes on: the
  answer carries the warning `install_caveat_noted` with `install_caveat` `guard_failed` (a word
  LAB-3 adds to the caveat vocabulary; no new code) and the rows as `install_guards`; the journal
  keeps the rows with the initiator.
- `--ack-guard` is gone, since nothing is left to acknowledge.
- `foreign_pauses` reports too: a global pause or another party's pause is listed and kept, never
  refused. The design's earlier "a global pause still refuses" (§2.3) is superseded by §7.
- The `memory` row's wait (`install.start.memory_wait_ms`) is a wait, not a refusal: a start still
  waits up to that long for memory, then goes on and reports the row (open point 4).
- **Not guards, and still refusing**, because they keep data readable and consistent: the
  preconditions, the writer lock and an unfinished operation (`current.json`), the interface checks
  and the rollback gate, and the checks of an operation's inputs.
- **The console** (UI-L1): the buttons that stop or restart an emulator or restart a game act on
  one press. The note the second press shows today when stopping an emulator stays, shown beside
  the button as plain text: it says the emulator will be started again (the UI channel calls it
  「看护会拉起」). It asks nothing and is not a confirmation.

## 7 The rescue and installation module

- **What moves in** (LAB-3): `actingctl install`, `start` and `restart`, `mcp-config` and
  `mcp-entry list` (LAB-5), and `agent-channel`. The watchdog's logic becomes `crates/watchdog`
  and `actingwatch.exe` (LAB-4); `alive`, `watchdog install` and `watchdog uninstall` use it.
- **Why it works while actingd is stopped.** It runs from
  `install\builds\<commit>\runtime\actinglab.exe`, as the deploy runs actingctl today, and touches
  only files, the owner lock, the WMI start and the watchdog.
- **The composites:** `deploy <tag|commit>` (stage, plan, stop with hold, apply, start, alive,
  release); `config replace <file>` (guards, stop with hold, backup kept, swap, check-config, start,
  alive, release; never prints the file); `place <bundle.zip>` (plan, pause the instances bound to
  the bundle's games, wait until idle, place, re-run, `standard-pack check`, wait for the new
  composition, resume, check the schedule; the games come from the bundle, no game is named in
  code; since 0.12.59, Workflow #453 S5, the composition is waited for only when an instance binds
  the game, and, S6, with the Runtime stopped: plan, the stopped rule under the writer lock, place,
  re-run, `standard-pack check` and the live build's check-config, with no hold;
  [install-cli.md](install-cli.md), "The Lab's rescue module"); `alive [--wait]`; and (Workflow
  #421 INST-5, first slice) `instance add <alias>` (compose
  the candidate, then `config replace`'s steps with the new instance paused after the start,
  before the release). The download stays outside the Lab.
- **Without `--root` and `--state-root`:** every rescue operation acts on the root of the
  `actinglab.exe` that runs it (section 9.3); `new <new_root>` names the new root. A rehearsal on
  another root runs that root's own program. Workflow #453 S4 (since 0.12.60;
  [install-cli.md](install-cli.md), "Fresh root"): `new` is the one row served without a located
  root, so the release's own program and the Lab's MCP server run it before any root exists; its
  positional is `new_root` and its state root's option `--new-state-root` (`new_state_root`), never
  `--state-root`, since the module refuses a parameter named `root` or `state_root` on every row.
- The configuration file's candidate (`install config …`) becomes `config file …`, distinct from
  the run-time settings `config set` and `config clear`.
- **As built by LAB-3** (0.12.48; register §1.287, "Slice LAB-3"; [install-cli.md](install-cli.md),
  "The Lab's rescue module"): the module is `crates/lab-rescue`, which actingctl answers
  `install`, `start`, `restart`, `mcp-entry` and `agent-channel` from, unchanged, until LAB-7;
  actinglab runs from `<root>\tools\` until LAB-7 moves it, and the default location takes both;
  a row whose source command is not built yet (`convert`, `clean`, `prepare`,
  `config file …`, `instance remove`, `pack remove` and `restore`) is not offered until the
  slice that builds the source (Workflow #421 INST-5 opened `instance add` with the module's own
  command first, [install-cli.md](install-cli.md), "Configuration, instances, packs and a fresh
  root", and Workflow #453 S4 `new` the same way, "Fresh root"); until LAB-6S2 the module's
  requests to a running Runtime go on the CLI channel, as `actingctl install`'s do: the Runtime refused a pause, a resume and a shutdown from a Lab
  envelope until LAB-6S1b deleted those origin gates (§2.4), and LAB-6S2 moves the module to the
  Lab channel with its word.
- **The MCP rows** (Workflow #434 LAB-5G5b, 0.12.57; register §1.287, "Slice LAB-5G5b";
  [install-cli.md](install-cli.md), "The MCP entry", "The rescue module's rows"). The module also
  serves three read-only rows of slice LAB-5, through the table as its other rows, on the CLI and
  in `serve --stdio`: `mcp plan --entry-protocol 2 [--current-build <commit>]`, the plan the MCP
  entry's second generation asks (its child the server `actinglab-mcp.exe` of the installed
  build; a build without it refuses `mcp_server_not_in_build`; one checked-in sample of its answer
  is the contract between the row and that entry); `mcp entries [--all]`, the entries' status
  files of both generations side by side, each with its generation; and `mcp config --client <c>
  [--list <modules>]`, the second generation's registration snippet, which writes no client file.
  The first generation's `actingctl mcp-entry plan|list` and `mcp-config` stay as they are.
- **"Only the operation's own hold"** (§5), as LAB-3 fix 1 and 2 build it
  ([install-cli.md](install-cli.md), "The Lab's rescue module"): `release-hold <reason>` hands back
  only a hold that a journal of the module in this root recorded in its pause step, only the
  instances that step paused (and the global pause it set, `instance add`'s only: Workflow #421
  INST-5b, install-cli.md, "Instances, the first slice"), each while its pause shows that reason,
  in any owner epoch (the Runtime's restore numbers the pauses anew, so the revision does not
  survive a restart); any
  other reason refuses `hold_not_owned` and nothing is resumed; `down` refuses a reason an
  instance already shows and never adopts a pause; every composite that releases follows the same
  rule; a refusal after the module paused something and before anything changed on disk hands its
  pauses back at once. An answer whose own pauses stay says `kept: true` and names in `next` only
  ways out that work today: the module's `release-hold <reason>`, and until LAB-7 `actingctl
  install release-hold --root <r> --reason <r>` and `actingctl resume` of each instance it paused;
  after LAB-7 the module's `release-hold` and the Lab's core `resume <instance>` (from LAB-6) stay.
  A global pause is kept and listed (§6). `deploy` takes no `--reason` (its form above has none).
  Until LAB-7 production deploys through `actingctl install`: an installer of v0.12.47 or older
  cannot read a journal the module writes (no `install-root` raise; open point 5).

## 8 The Lab's modules

### 8.1 What can be detached

| Part | Program | Detachable | How |
|---|---|---|---|
| Lab core: the operation layer, the CLI, the debugging operations, the rescue module | `actinglab.exe` in the runtime member, where actingctl was | no | the one command line and the one installer; the debugging operations stay with it (Alice's answer 2) |
| MCP | `actinglab-mcp.exe` and the MCP entry `g2` (`actingcommand-mcp-entry-g2.exe`) | yes | not installed, or not registered with a client; the CLI does not depend on it |
| Authoring | `crates/lab`, through the cargo feature `authoring`, in `actinglab.exe` | yes | a build without the feature; `lab_module_authoring` absent or `false` only leaves its operations unlisted (section 8.3) |
| Watchdog | `tools\actingwatch.exe` | its own program | `watchdog uninstall` removes its scheduled task |

The table's `modules` lists them with the module id each operation names (`core`, `debugging`,
`authoring`, `rescue`) and the front ends (`cli`, `mcp`, `serve`, `watchdog`).

**The MCP entry's generations** (LAB-5G5a; the coordinator's ruling of 2026-11-12 on board
#434, which replaces ruling 6 of 6775192250; [install-cli.md](install-cli.md), "The MCP entry").
The second generation is a second program with its own file name, `actingcommand-mcp-entry-g2.exe`,
whose child is `actinglab-mcp.exe`; it goes only into `<root>\mcp-entry\g2\`. `g1\` only ever holds
the first program, `actingcommand-mcp-entry.exe`, which stays generation 1 in every build (child
`actingctl mcp-serve`, plan `actingctl mcp-entry plan --entry-protocol 1`) until the slice that
removes actingctl, after the owner's registrations have moved. Why two programs: no installer
code writes under `<root>\mcp-entry\`; placing `g1` is a copy of `tools\actingcommand-mcp-entry.exe`
by hand, as the documents of every earlier build say, so a second generation under that file name
would reach `g1\` by that copy, and a rollback would make one file name mean two generations.
Placing the second program stays a manual step until an installer step exists, which creates
under `g2\` only.

The MCP module's program exists since LAB-5G2a as the server's shell (section 9.3, "The server's
shell"): it lists its tools and, since LAB-5G2b, serves in its own process the calls of the rows
the CLI module serves and, since its second part, on Windows those of the rescue module's rows
(section 9.3, "Calls served in the server's process"; since LAB-6S0 `deploy` and `rollback` too,
detached, section 5), and since LAB-5G4b
every other offered row through `actinglab.exe serve --stdio --front mcp` children (section 9.3,
"Calls served by `serve` children"); the MCP entry `g2` is slice G5a's.

**Packaging** (Workflow #434 LAB-5G6, 0.12.57; register §1.287, "Slice LAB-5G6"; the
coordinator's rulings 5 and 21 on board #434, not the owner's words). Every build's Tools archive
carries `actinglab-mcp.exe` and `actingcommand-mcp-entry-g2.exe`, each an entry of the Tools
`BUILD-MANIFEST.json` (`size_bytes`, `sha256`) written by the step that writes the others, so a
deploy puts both in `<root>\tools\`. They are added inside `tools-layout` 3
(`platform-tools-v3`), as the first entry was in v0.12.18: no reader of the archive stops working
([component-interfaces.md](component-interfaces.md), "Bump rule"). Nothing starts either program:
no registration names them until the owner registers a client with the second entry (G7), and
placing that entry in `<root>\mcp-entry\g2\` stays the manual step above; no installer code writes
under `<root>\mcp-entry\`. A running `actinglab-mcp.exe` never blocks a deploy that replaces its
file: it is renamed aside as `actingctl.exe` is ([install-cli.md](install-cli.md), "Running
images"); a running `actinglab.exe` other than the process that plans still blocks.

### 8.2 `actinglab serve --stdio`

A resident mode of the Lab core (LAB-2): it reads one `{op, params}` per line on stdin and answers
one `{result}` or `{outcome}` per line, the operations being the table's. Its users are the MCP
module and the UI's authoring workbench, so the three front ends share one loop. Each operation
answers the line its `--json` answers (section 9.3): the CLI module's line, for an operation
actinglab's own command serves too since LAB-2b (open point 10, done), and `status` and `run show`
from their core rows.

- **`--front mcp`** (LAB-5G4a; the coordinator's rulings, board #434 6769399787 points 9 and
  10). The MCP server starts its children as `actinglab serve --stdio --front mcp`, an option
  only `serve --stdio` takes; `mcp` is its one value. Another value, the option twice, or the
  option without `--stdio` is refused as `serve` refuses an unknown word
  (`actingctl_usage_invalid`, the word as `argument`, `unknown`; the option without a value
  `value_missing`), and so is the option with `ACTINGCOMMAND_INITIATOR` set (section 4.2), before
  the loop starts. With it the process's initiator is `Lab-MCP`: on each step and mark
  actinglab's own commands add to a recording (section 4.3) and in a rescue row's journal. The
  connections actinglab's own commands open bind as MCP's `--via mcp` child's do: until
  LAB-6S2-B1 the Lab channel with `via` `mcp`; since LAB-6S2-B1 (0.12.59; register §1.287,
  "Slice LAB-6S2-B1") the Lab channel with `Lab-MCP` and no `via`; the envelope (Lab, Lab) either
  way. The core rows `lab-ops` serves, which that child never serves, connected as without the
  option until LAB-6S2-B1; since then they too bind the Lab channel with `Lab-MCP` and no `via`,
  with their envelope (Lab, Lab). The Runtime records every request of either with that envelope
  as `lab` with `via` `mcp` (section 2.4, the map M): for the own commands as on 0.12.58, for the
  core rows where 0.12.58 recorded `lab` without `via`, a change no MCP server's call reaches,
  since none sends a core row to its child. The rescue rows' connections bind the Lab channel with
  `Lab-MCP` and keep the CLI's envelope (section 4.3). The lines stay `serve`'s (each operation's
  `--json` line), never the envelope a `--via` line prints; since LAB-5G4b each answer line
  carries its line inside the envelope below. Without the option nothing changes.
- **The answer envelope under `--front mcp`** (LAB-5G4b, 0.12.57; register §1.287, "Slice
  LAB-5G4b"; the coordinator's ruling on board #434, 6827665631, not the owner's words). With the
  option
  each answer line is `{"exit_code": <n>, "req_id": <id>, "line": <line>}`: `line` is the line
  the loop answers without the option, byte for byte; `exit_code` the code the command line would
  exit with for that line (0, 1 or 2; 1 for a line that did not encode, which the command line
  does not print); `req_id` the request id of the own command that served the row, when its
  result carries one (the result's `req_id` on a success, its details' on a failure, where MCP's
  `--via mcp` child reads it), absent otherwise. The MCP server answers `isError` by `exit_code`,
  and the client action after a capture names `req_id`; neither can be read from every line. The
  refusals before the loop (the option itself, the installation, the initiator, the table) stay
  one line each, without the envelope. Without the option every line is as before, byte for
  byte.
- **Values that begin with `--`** (LAB-5G4a; board #434 6775192250 point 16). `serve` builds a
  command line for the command that serves a row, and a string beginning with `--` would reach
  that command's parser as a flag. So a parameter whose value is a string that begins with `--`,
  or a list holding such a string, is refused before anything runs, for every row whichever code
  serves it: `actingctl_usage_invalid` with the parameter as `argument` and `not_recognised`, as
  a line, and the loop goes on. Numbers, booleans and strings that begin with one dash are taken
  as before. The command line does not change (there a word that begins with `--` is a flag).

### 8.3 The authoring option

- `lab_module_authoring` (optional boolean, absent `false`) stays in the configuration file and
  means one thing: the authoring module's operations are listed by default (Alice's answer 2;
  the coordinator's ruling of 2026-11-11, LAB-5F fix 2). The Runtime decides nothing by it.
- The Lab reads it from the selected configuration. A build that has the authoring code serves
  its operations whatever the key says. Where the key is absent or `false` inside an installation
  root they are left out of the listings (CLI help, `list commands`, `capabilities`, the MCP
  server's default `tools/list`), and a call still runs: not listed is not refused.
- **Where the Lab runs** (the coordinator's ruling of 2026-11-10 on #434, point 15; LAB-5F). A
  Lab program inside an installation root (one §9.3's default location finds: started through a
  root's selection, or from a kept build's or a single-location root's `runtime\` or `tools\`)
  follows the key of that root's selected configuration, read at each listing. A Lab program
  outside any installation root has no configuration to read: there the authoring operations are
  listed (an unpacked Tools package, as the resource repositories' CI and scripts and the
  coordinator's deployment tools run it). A selection or selected configuration that cannot be
  read, that is no JSON object or whose key is not a boolean leaves them unlisted, and the command
  that lists says why once, on its developer line (`config_read_failed` or `config_decode_failed`
  with the configuration's `path`, and the cause); it refuses nothing because of it, and it is
  never read as listed. `null` reads as absent, as actingd reads it. The decision is one function
  of the operation layer, `lab_ops::locate::authoring_module`, which every front end calls. Main
  has no cargo feature `authoring` yet: every build has the module and serves its operations.
- **Which operations** (LAB-5F fix 1, the coordinator's ruling of 2026-11-11): the key lists
  exactly the rows of module `authoring`. `standard-pack check`, which the rescue module's `place`
  runs as its post-check (section 7), is a row of the core module: it is listed and served
  wherever the Lab runs, whatever the key says, so that an installation without the authoring
  module still places packages. `standard-pack compose` stays an authoring row. Workflow #464 P3
  (0.12.60): `standard-pack ready`, the readiness of every pack of a game root, which only reads,
  is a core row too ([standard-pack-schedule.md](standard-pack-schedule.md), "Readiness").
- `lab_module_debugging` retires (`actingd-config` 28): still accepted when a boolean, with no
  effect, noted with the caveat `config_key_retired` by the commands that read it, as
  `install.guard.blocked_windows` is. The debugging operations are always there.
- Its writers are the rescue module's `new` (`--lab-authoring true|false`) and
  `config file lab-options --authoring true|false`, and through them the installer's check box
  (UI-L2): they set what is listed. `--lab-debugging` and `--debugging` are refused as unknown
  options. The console shows the key read only (`advanced`, not settable); the
  `lab_module_debugging` row leaves the tier table (LAB-6).
- Installing the MCP module is no key: it is installed when `actinglab-mcp.exe` is and the
  client's entry is registered (`mcp config`).

## 9 The operation table

### 9.1 The file

[lab-operations.json](lab-operations.json), schema `actingcommand.lab-operations.v1`, is the one
source of the operations: LAB-1 reads it into `crates/lab-ops`, LAB-2 takes the CLI words from it,
LAB-5 the MCP tools, and LAB-1's test `lab_ops_table_complete` reads the file itself. Members:

- `initiators`, `modules`: sections 4 and 8.
- `ui_kinds`: the UI list (section 3), each `{kind, class, console_today, rule?, until?}`.
- `not_exposed`: every request kind no operation sends, each `{kind, reason, note}`; `reason` is
  `handshake`, `deleted` (LAB-6 deletes the kind, and its row leaves with it) or `unsent` (no front
  end sends it today; the `lab` entry still takes it). A table revision gives it an operation when
  a front end needs it.
- `operations`: one object per operation:

| Member | Meaning |
|---|---|
| `name` | the CLI words, unique; the MCP tool is `ac_` and the name with spaces and `-` as `_`, also unique |
| `module` | `core`, `debugging`, `authoring` or `rescue` |
| `params` | each `{name, form, type, required, repeat?, default?, required_in?, note?}`; `form` is `positional`, `option` or `switch`; an option's CLI flag is `--` and the name with `_` as `-`, its MCP argument the name; positionals keep their order; `required_in` (LAB-5A) lists the front ends (the ids of the `modules` rows of role `front_end`) in which the parameter is required although the row does not require it (today only `mcp`, on `resume`'s `expect`, `pause`'s `reason` and, since LAB-5C, `remedy set`'s `rows`; nothing reads it yet, and the CLI and `serve` do not change); what the schema cannot state (a one-of rule, a value given as a positional or as a flag, a joint or exclusive group, flag sets that depend on a path or a kind) is in the parameter's `note`, and the code enforces it |
| `params_more` | when present: the source command whose further flags carry over unchanged under the same naming rule. Since LAB-5A only a row no program offers yet has it (a rescue row whose source command is not built); every offered row declares all of its parameters (section 9.2) |
| `help` | optional (LAB-5E): what the operation does, in one sentence of the row's own, shared by the command line's help and the MCP tool's description; written on its own line directly before the row's `output`. English, ASCII, 1 to 160 bytes, one line; one sentence or two short ones, beginning with a verb in the third person ("Reads…", "Pauses…", "Lists…"); it says what the operation does and on what and, where a caller must know it to use the row at all, what comes back or what to call next. No rule every row shares (budgets, handles, exports, cursors, what `irreversible` means: those are the MCP server's instructions), no MCP tool name (`ac_…`; a row is named by its words), no game word, no path of a machine, nothing the row's parameters already say. Every offered row has one (section 9.2); a row no program offers (reserved, or planned and not served) has none yet. `actinglab help` prints it (Workflow #434 CLI-HELP, section 9.3, "Help"), read from the table the build embeds, never copied |
| `output` | what the operation answers; the CLI's `--json` line, the MCP result and the serve answer are the same JSON |
| `writes` | whether it changes anything; MCP's `readOnlyHint` is its negation |
| `irreversible` | section 5; MCP's `destructiveHint` is `irreversible` or `disruptive` |
| `disruptive` | optional (LAB-5A), absent is false: the operation stops or restarts something a person may be using, without being irreversible in the sense of section 5 (`emu stop`, `emu restart`, `app stop`, `app restart`, `drill`, `devices unbind`) |
| `runtime` | `required` (refused while actingd is stopped), `optional` (works while it is stopped, does more while it runs) or `offline` (never needs it) |
| `entries` | `["lab"]`, or `["lab", "ui"]` for a `core` operation whose every request kind is on the UI list (not counting the transitional one) |
| `requests` | the request kinds it sends, as read from its source; the slice that moves it corrects the row from the code it moves and says so |
| `planned_requests` | kinds it will send that the contract does not have yet |
| `source` | the commands, tools, console reads and scripts it replaces (`actingctl …`, `MCP ac_…`, `actinglab …`, `actingd …`, `acui: …`, `acsetup: …`, `script <coordinator workspace path>`) |
| `new` | for an operation no program has today: the board item it comes from |
| `slice` | the slice that builds it |
| `status` | `planned` (built by `slice`) or `reserved` (a position only: it opens with `opens_with`) |
| `notes` | anything else the row must say |

Parameter types: `instance` (an alias, an `instance_id` or an ADB port), `string`, `integer`,
`unsigned` (LAB-5A: a whole number from 0 to 2^64 − 1), `decimal` (LAB-5A: a decimal number; the
CLI hands the command its text, `serve` takes a JSON number or that text), `boolean`, `duration`
(a number with `ms`, `s`, `m` or `h`), `path`, `file`, `dir`, `json`, `key`, `key_value`
(`KEY=VALUE`), `list<string>`, `enum:<a|b>`, `request_id`, `run_ref` (LAB-5A: a request id or a
run id), `operation_id` (`op-<ms>`), `material_ref`, `code`, `scope` (an instance, a server or a
game, one word: `instance:<inst>`, `server:<id>` or `game:<id>`; #429 R-8's `pack-schedule set`),
`tag_or_commit`, `sha256`. A millisecond count is a bare number, not a `duration` (the commands
take a bare number): an `integer`, or `unsigned` where the command reads any whole number from 0
(LAB-5A, LAB-5C: the `*_unix_ms` times of `runs`, `events` and `diagnose`). A comma list or JSON
text the CLI hands on as written is a `string` (`record mark`'s `request_json`); a comma list the
words split is a `list<string>` (`events`' `type`), and JSON the words read is a `json`: the CLI's
words parse its text (text that is not JSON is `wrong_type`) and `serve` takes the JSON value
(`resume`'s `expect`, `targets set`'s `targets`, `remedy set`'s `rows`). A flag given more than
once is `repeat`.

### 9.2 What `lab_ops_table_complete` checks (LAB-1)

Names and MCP tool names are unique; every operation has `params`, `output`, `writes`,
`irreversible`, `runtime`, `entries`, `requests`, `source` or `new`, `slice` and `status`; every
kind of `RuntimeOperationKind::ALL` is in some operation's `requests` or once in `not_exposed`, and
every kind the file names exists; `entries` follows its rule; a `reserved` row has `opens_with`.
Since LAB-5A also: every parameter's type is one of section 9.1's (reading the file refuses
another); no offered row carries `params_more` (only a planned rescue row may, while its source
command is not built: the test checks that in `crates/lab-ops`, and actinglab's `gate_434_lab5a`,
which knows which rows actinglab offers, that no offered row has it); every `required_in` names
front ends, on a parameter the row does not require; a `disruptive` row writes and is not
irreversible. Since LAB-5E also: every `help` is ASCII, 1 to 160 bytes on one line (no control
character), names no MCP tool (`ac_`), and no two rows share one; every offered row has one:
every row actinglab offers and every planned row of slice LAB-5, which LAB-5's MCP server offers
itself (actinglab's `gate_434_lab5e`, which knows which rows actinglab offers). The neutrality
guard's test reads the table line by line, each `help` on its own line, so a game word in a
sentence fails that test.

### 9.3 Defaults and output (LAB-1, LAB-2)

- The instance is a positional (`<inst>`): an alias, an `instance_id` or an ADB port. **One
  spelling for every command** (Workflow #434 CLI-IN, register §1.307; the coordinator's rulings of
  2026-11-11, not Alice's words): on actinglab's command line a row that declares a positional
  instance also takes actinglab's global `--instance <inst>` in its place, after the row's words or
  before them (`actinglab pause --instance <inst>`, `actinglab --instance <inst> pause`), and the
  row's other positionals keep their order (`devices bind --instance <alias> <handle>`, `offset
  <task> <ms> --instance <inst>`); a row whose instance is optional takes it the same way. The table
  records it in the parameter's `note`, one text on every such row: "on the command line also
  --instance <x>, in place of the positional". The positional and the flag on one line, whether or
  not they name the same instance, are refused before anything is read or sent, as a surplus word
  is (LAB-5B4): `argument_problem` `repeated`, `argument` the word that gives the instance a second
  time (the positional instance's word, or `--instance` given twice), with
  `actingctl_usage_invalid` on a row of the CLI module and actinglab's `validation_failed` on a row
  its own command serves under other words (`app launch|restart|stop`, `recover`); `--instance`
  without a value is refused `value_missing`. The refusal is that of one word too many: on a row
  whose next positional is not required (`targets set <inst> [<file>]`) a lone word beside the
  flag is that positional (`targets set <file> --instance <inst>`), and the two spellings are
  refused when the file is given too. A line without `--instance` is read as before; a line
  with `--via` that begins with `--instance` stays actinglab's own, as before. The 22 rows
  actinglab's own
  command serves under the row's own name (`tap`, `swipe`, `long-tap`, `key`, `text`,
  `touch-probe`, `do`, `ensure`, `wait`, `tap-target`, `navigate`, `observe`, the nine `record`
  rows that take an instance, `record archive` and `record reopen` among them since Workflow #449
  REC-ARCH, and `locate`) take the instance on the command line only as `--instance`, as before (their
  note: "on the command line only --instance <x> …"); none gains a positional instance (for `do`,
  `ensure`, `wait` and `text` it would be read as the target, the page or the text). `remedy set`
  and `wake-test` declare `instance` as an option of their own and read `--instance` as that
  option, as before. `serve --stdio` and MCP are unchanged: they pass the parameter `instance`.
- The root is located from the program's own place (a slot's `…\runtime\actinglab.exe`, or a fixed
  entry); the state root comes from the selected configuration. No operation takes the
  installation's root or its state root as `--root` or `--state-root` (`new` names its new root).
  Four offline rows take options of those names that name something else, as their consumers pass
  them (LAB-5A): `standard-pack check` (a core row since LAB-5F fix 1) and the authoring row
  `standard-pack compose` take `--root`, a game's package root, and the authoring rows `resource
  restore` and `regression` take `--state-root`, a Runtime state root the offline command reads. One command line keeps a `--root` that is no parameter of its
  row (LAB-5A fix 1): `actinglab devices bind <alias> <handle> --root <r>` still hands it to the
  command, as before LAB-5A, which takes it only when the program runs outside an installed root;
  the row does not declare it, and `serve --stdio` refuses it.
- Output for people by default: one line with the result code and its short phrase from the
  outcome catalog, and a small table where needed. `--json` prints the one line `cli_line` prints
  today. Exit codes stay 0, 1 and 2.
- **actinglab's own commands print the CLI module's line** (Workflow #434 LAB-2b, open point 10;
  register §1.287, "Slice LAB-2b"; before it, LAB-2 had left them their envelope, "changing
  nothing any current consumer gets", the coordinator's ruling of 2026-11-10). Every command form
  of actinglab's own dispatch, with the global options its consumers pass before or after the words
  (`--run-root`, `--verbose`, `--json`), is parsed and run as before; only the printing changed:
  - **Success**: the code `lab_operation_completed`, the result's members flat at the top level
    beside `outcome`; a result that is no JSON object goes into the one member `result`. Warnings a
    result carries stay where they are, its member `warnings` in their own shape `{code, values}`:
    the category stays success, and they write no developer line.
  - **A result whose top level has a member named `outcome` is never flattened** (the
    coordinator's ruling of 2026-11-10 on LAB-2b): it goes whole, unchanged, into one named member,
    which the command declares in one place in the code (`cli_module::own_line::WRAPPED`). Today
    these are `resource verify`, whose `actinglab.resource-verify.v1` report is the member
    `report`, and `run summary`, whose `actingcommand.run-summary.v1` document is the member
    `summary`. A result that names `outcome` without a declared member is refused
    (`actingctl_output_failed`, exit 1, nothing on stdout), so a later command cannot collide
    silently.
  - **Failure**: `{"outcome": {code, category: "error", values}, "message", "details"}`. The code
    is actinglab's own. `values` (LAB-2b fix 1): when the details carry an outcome of the same code
    (they are one, or their member `outcome` is one, as `resource verify`'s report and the
    `{outcome}` of the Lab's OCR, touch, recording and standard-pack refusals), that outcome's
    values; otherwise the members of the details whose names the outcome catalog registers as that
    code's values (none for a code the catalog does not list). `message` is the error's; `details`
    are its details nested whole, never flattened (some carry an `outcome` of their own), and
    absent when it has none. Exit 1, with the developer line on stderr.
  - **Exit codes** 0, 1 and 2, by the outcome's category (LAB-2b fix 1): 0 ends a success; 1 ends
    every failure, and the installation's refusal before the command runs, which is the module's
    `cli_line::refused` of the installation's code: every such code is an error, so the line exits
    1, writes `ERROR actinglab: <code>` on stderr and prints `{outcome}` on stdout (the code for
    people without `--json`), where actinglab before LAB-2b wrote `FATAL actinglab: …`, printed
    nothing on stdout and exited 1, as a `--via` line still does; 2 ends an outcome of category
    fatal, which no own command line has today. `capabilities` reports these three as its
    `exit_codes` (`ok`, `error`, `fatal`); on a `--via` line it reports the envelope's classes, as
    before.
  - **Without `--json`** the output for people, also when stdout is a pipe or a file, and also for
    a missing option value (LAB-2b fix 1), which a `--via` line still prints as the envelope.
  - `ACTINGCOMMAND_DETAIL=codes` removes `values` from the line, as from every CLI line: no
    consumer may set it.
  - Its consumers changed with it (open point 10): acmodel's validator, the four resource
    repositories' CI and scripts and the coordinator's hotplace kit read both shapes before a
    Runtime with LAB-2b is deployed, and raise their Tools pin to it only with that change.
- **MCP's actinglab child keeps the envelope until LAB-7.** A command line with `--via` (only
  `lab_ops::lab_child`'s `--json --via mcp …` sends one) prints the envelope with `ok`, `data` and
  `error` (also without `--json` when stdout is no terminal) byte for byte as before LAB-2b, and
  its exit codes 0 and 2 to 6; MCP's Lab tools answer as before. LAB-5 adds the new server
  (`actinglab-mcp.exe`) beside it and replaces nothing: `actingctl mcp-serve`, with its `--via`
  child, stays as it is until LAB-7 (section 11: LAB-1 to LAB-5 only add; the coordinator's ruling
  of 2026-11-10, board #434, recorded with LAB-5A).
  - **The one exception** to "byte for byte" (the coordinator's ruling of 2026-11-10, LAB-2 fix
    2, a consequence of open point 6): the answers of `record mark` (which MCP's `ac_record_mark`
    passes on) and `session record step` carry the new `initiator` of each mark or step they
    report (section 4.3, "Recordings"), the same word the recording file holds.
- **The CLI module's operations** (the bullets above: the line for people by default, `--json`
  printing the `cli_line` line, exit codes 0, 1 and 2) are the core rows that came from actingctl
  (served by `crates/lab-ops`) and `run start`, `run stop`, `reset` and `drill`. Since LAB-5G1
  (register §1.287, "Slice LAB-5G1") the code that serves all of them from a row and its
  parameters is `crates/lab-ops`' (`lab_ops::core_ops`), which actinglab calls with its place and a
  connection made for each call.
  - **How actinglab tells them apart.** A command line whose words, `--json` taken out, begin with
    `serve` or with the name of such an operation is the CLI module's; so is one without `--via`
    whose words, actinglab's own global options before them taken out, are a core row's (LAB-2b
    fix 3: `--runtime-endpoint <endpoint> status` is given to the module as it is, which refuses
    the option by name as it does after the words). Every other one is actinglab's own and is
    parsed and run by its own dispatch. Two core rows take words that were
    actinglab's own commands, `status` and `run show`: since LAB-2b the command line gives them to
    their core rows too, and actinglab's own `status` (a TCP probe of its user configuration's
    `runtime_endpoint`) and `run show` (a placeholder answering `reserved`) are deleted (open point
    10). Other words both sides have are the module's (LAB-2 fix 2; no
    consumer sends them, and that is what the slice means). `devices` now lists the
    installation's devices and `monitor set|clear` writes an instance's monitor policy to the
    Runtime, where main refused both as retired (exit 6); `devices bind|unbind` writes or removes
    an instance's binding, where main refused the positional (exit 2); `config set` sends
    `ConfigureSettings` to the Runtime, where main wrote actinglab's user configuration file (now
    `user-config set`); `config` reads the Runtime's settings, `config clear` changes them and
    `run evidence|start|stop` serve their run operations, where main answered them as unknown
    (exit 2). Register §1.287, "Slice LAB-2", L2-12 has each. A row actinglab's own
    command serves under other words (`watch` for `lab watch`, `recover` for `session recover`,
    `user-config [<key>]` for `user-config get`, `app launch|restart|stop` for `session app`, and
    the like) is given to that command in its words and answers as it does; refused short words
    are actinglab's own usage error (`validation_failed`).
  - **actinglab's own commands refuse a word they do not read** (Workflow #434 LAB-5B4; register
    §1.287, "Slice LAB-5B4"). Until that slice 24 own commands of the debugging and authoring
    rows, and `run list`, `capture`, `lab receipt` and `lab evidence`, ignored a flag they did not
    read, and a switch took the next word as its value. Now each parses its command line by its
    read list: a flag off the list is refused before the command does anything
    (`validation_failed`, its details `argument` the flag and `argument_problem` `unknown`, the
    message `<command>: unknown <flag>`, as the module refuses a row's undeclared flag); an option
    at the end of the line is refused `value_missing`; a positional beyond those the command reads
    is refused `unknown`. On these command lines, and those of the strict commands that take
    switches (`observe`, `do`, `ensure`, `wait`, `record mark`, `record stop`, `package fmt`,
    `package loading`), a switch takes the next word as its value only when it is `true` or
    `false`, and leaves any other word where it is; `observe`, `do`, `ensure`, `wait`, `package
    preflight`, `resource catalog` and `resource validate` refuse a positional they do not read
    too. No own command of the two modules ignores a flag. `serve --stdio` hands a row only its
    declared parameters, as before. Since Workflow #434 LAB-5B5 (register §1.287, "Slice
    LAB-5B5") the rest of actinglab's own commands that read their flags by name and ignored the
    rest (`help`, `version`, `paths`, `capabilities`, `doctor`, `list`, `detect`, `stream`,
    `explain`, `user-config get|set`, `env resolve|status`, `lab run|validate|replay-evidence|status|
    vendor-stdio-selftest`, `package run`, `control inspect|verify|probe-click|diff`, `session
    status|throat-policy|capture-policy|record-policy|self-heal-policy|contract|api|transport|
    monitor-policy|instance|app|stream`, `run open|export`, `report export`) parse their command line
    by a read list too, with the same three rules and the same refusal; `lab arbitrator`, `control
    export` and `operation run` refuse every line. **No own command of actinglab ignores a flag.**
    Pass-throughs: none; no own command hands its words to another program or a pack. `stream`
    reads the words after `--input-relay` or `--interactive-input` as the relay action's, which
    checks them, and without either reads no word but a first `check`; a line the global
    `--version` makes a version line prints the version whatever its words, as before.
  - **`serve --stdio`** answers each operation the line its `--json` answers: the CLI module's
    line, for actinglab's own commands too since LAB-2b.
  - **`status` and `run show`** (open point 10): the command line and `serve --stdio` answer them
    from the CLI module's core rows, `status` with the watchdog block (LAB-2 fix 2 for `serve`,
    LAB-2b for the command line).
    - **`status` with no Runtime running exits 0** (the coordinator accepted it on 2026-11-10, LAB-2b
      fix 3). The row reports state, and a Runtime that is not running is a state it reports: it
      answers `runtime_overview_reported` with its `daemon` block offline (`online` false, class
      `runtime`, `runtime_unavailable`) and exits 0, as `serve --stdio` answers it, and the
      operation table marks the Runtime optional for the row. actinglab's own `status` before
      LAB-2b exited 5 with `runtime_not_running`. A caller that needs to know whether the Runtime
      runs reads the `daemon` block, not the exit code.
    - **A global option of actinglab's own, which the row does not take** (`--runtime-endpoint`,
      `--run-root`, `--verbose` and the others but `--json`, `--version` and `--via`), is refused
      by name with `actingctl_usage_invalid` (its `argument`, `argument_problem` `unknown`), exit
      1, before or after the words alike (LAB-2b fix 3), and nothing is sent: the row reaches a
      Runtime only through the state root's `runtime-info.json`, whose host must be loopback. A
      line with `--via` stays actinglab's own (its dispatch knows neither word); `--version` before
      the words makes the line actinglab's `version`, and a value actinglab's parser refuses is
      its refusal, as before. Since Workflow #434 CLI-IN the rows take `--instance <inst>`, which
      names their instance (section 9.3's first rule), before or after the words alike; a line
      that begins with it and has another global option before the words is refused naming
      `--instance` as before.
  - `--envelope` is gone: nothing needs it once actinglab's own command lines answer as before.
  - The catalog holds no phrase for a code, so the line for people is the code with its values,
    and none is made up; the small table is the result's members. The CLI module locates the
    state root (`lab_ops::locate`); `ACTINGCOMMAND_RUNTIME_STATE_ROOT`, which MCP's actinglab child
    and the tests set, still names it when set.
- **A request the Runtime answered is never a device error** (Workflow #433 LABDO-1, in force
  from that slice; LAB-2 and LAB-5 keep it). Once the Lab has sent an operation's request and
  the Runtime has answered it, a failure of a check the client runs after the receipt (its event
  queries, its evidence check) names the request and its effect: `lab_operation_check_failed`
  with the receipt's `req_id`, `correlation_id` and `receipt_state`, the `effect` and `executed`
  its record reports, the record's own `failure`, and the client's `client_code` and
  `client_message`. It is never `device_error`, and the caller reads the effect before sending
  the operation again. Since LAB-2b (open point 10) actinglab's `do` exits 1 with it on the command
  line, like every error, its `req_id`, `receipt_state` and `client_code` the outcome's values and
  its details nested whole, so the effect is the lasting signal; MCP's actinglab child (`--via
  mcp`) still gets the envelope with exit 5 until LAB-7 (LAB-5 adds its server beside it). MCP answers it in
  class `runtime` with its `req_id`, actinglab's details and its message (LAB-5 keeps that). A
  failure the Runtime itself reports in the receipt keeps its own code
  ([contained-lab-operation.md](contained-lab-operation.md)).
- MCP lists the operations of the installed modules, with descriptions generated from the table
  and the shared rules written once in its instructions (#410 item 3); no tiers (`--tier` goes);
  the 24 KiB answer budget and `handle` with `wait_s` stay as runtime-interface.md,
  "MCP (counter `mcp`)", has them.
  - **The server's shell** (Workflow #434 LAB-5G2a, 0.12.57; register §1.287, "Slice LAB-5G2a";
    the coordinator's rulings on board #434, 6775192250 and 6784514298, not the owner's words).
    `actinglab-mcp` (`apps/actinglab-mcp`) speaks MCP over stdio with the protocol and the loop
    of `actingctl mcp-serve`, copied (both eras, batches under 2025-03-26, progress, cancellation,
    `actingcommand/entryStatus`), and lists one tool for each row it offers: the rows actinglab
    offers (the rescue module's on Windows only) and the planned rows of slice LAB-5. A tool is
    `ac_` and the row's words with spaces and `-` as `_`, but for two rows held in one place: the
    frame's tool is `ac_frame` and the authoring observe's `ac_lab_observe`; no tool is
    `ac_observe`, a name that is never reused with another meaning. Its description is the row's
    `help`; its input schema has one property per parameter (the parameter's `note` its
    description, its `default` typed, `required` the row's and those whose `required_in` names
    `mcp`, no other property); its hints are `readOnlyHint` (the row does not write),
    `destructiveHint` (irreversible or disruptive) and `openWorldHint` false; no title, no
    `outputSchema`, no `idempotentHint`. `--list <module>[,<module>...]` names the modules whose
    tools are listed (the ids of the table's module rows of role `operations`); without it every
    installed module is, the authoring module where section 8.3's key lists it, read at each
    listing. A tool left out of the list is callable as a listed one; only a name that belongs to
    no offered row is an unknown tool. `--workers <n>` is the number of threads that run
    `tools/call` (1 to 64, 8 when not given; a value outside the range or no whole number is
    refused as a usage error). `--list-tools` prints the `tools/list` result. `--tier` is refused
    (`actingctl_usage_invalid`; the server has no tiers).
  - **Calls served in the server's process** (Workflow #434 LAB-5G2b, 0.12.57; register §1.287,
    "Slice LAB-5G2b" and "Slice LAB-5G2b, second part"; the coordinator's rulings on board #434,
    6775192250 points 1, 9, 11, 12, 17 and 18, 6784514298 and of 2026-11-12, not the owner's
    words).
    - *Which.* A row `lab_ops::core_ops` serves (the core rows but `app launch|restart|stop`,
      `standard-pack check` and, since Workflow #464 P3, `standard-pack ready`, and `run start`,
      `run stop`, `reset`, `drill`) is served by it in the
      server's process. Since the slice's second part (0.12.57), on Windows, where the rescue
      module runs, a rescue row the module serves (`lab_rescue::rescue::serves`; its MCP rows
      `mcp config`, `mcp entries` and `mcp plan` among them) is served by it in the server's
      process; since LAB-6S0 (0.12.58) so are the rows whose `detach` the front gives true
      (`tools::SUPPLIED`: `deploy` and `rollback`, section 5). Since LAB-5G4b every other offered
      row (the three `app` rows, `standard-pack check` and `standard-pack ready`, the debugging
      and authoring rows
      actinglab's own commands serve, `record draft` among them: actinglab's own command since
      LAB-5G4c, 0.12.57, MCP's `ac_binding_draft` ported; register §1.287, "Slice LAB-5G4c") is
      served by a `serve` child ("Calls served by `serve` children", below). Since LAB-6S0 no
      offered row answers `mcp_operation_unsupported` (below).
    - *A rescue row* (the second part) is checked as a core row is (the refusals below), then
      answered by the module for the root the program runs from with the initiator `Lab-MCP`
      (`lab_rescue::rescue::answer_here`), and its answer is the line actinglab's `--json` and
      `serve --stdio` print for it: a line as it is; the notice the line `agent_channel_reported`
      with its `text`; a hook that did its work that line without members; a registered session's
      failed hook its failure (`agent_channel_failed`, refused). The server records no provenance
      for it (no identity card, no client action): the module makes its own connection on its own
      channel until LAB-6 (section 7) and records the initiator in its journal (section 4.3). The
      budget is the core rows' (below). Each call runs as a job, as a core row's does since
      LAB-5G3 (below): a start, a stop or a placement that outlasts the call budget answers its
      job's handle.
    - *The answer* is the line `actinglab --json` prints for the same row and parameters (its
      members, `outcome`, `warnings`): the text content and, from 2025-06-18 on,
      `structuredContent`; `isError` is true exactly when the command line would exit non-zero
      for that line. There is no `{ok, result}` envelope.
    - *The Runtime.* One kept connection of `lab_ops::front`'s `Front::Mcp` (today's envelope,
      (Cli, Cli); until LAB-6S2-B1 on the MCP channel, since LAB-6S2-B1, 0.12.59, on the Lab
      channel with `Lab-MCP` and no `via`, whose requests the Runtime records on the MCP channel,
      section 2.4, the map M), made again as today's server makes its own (`Health` must
      answer the owner epoch it was opened on; a latched failure drops it); a Runtime that does
      not answer gives the line the command line gives (`status` reports it offline). A core row
      the table marks as writing to the Runtime goes on a connection of its own after the
      provenance today's server records before a write: the identity card `actingctl-mcp`, then
      the client action `{surface_id: mcp, control_id: <tool>, kind: command}` naming the
      instance, under the write's correlation (rulings 11 and 12: until LAB-6 the new server
      looks like today's on the wire and in the ledger). The cursors the code issues bind to the
      server's session and that connection's generation; one is taken back since LAB-5G3's
      second part (below). `status` carries the watchdog block on Windows, as on the command
      line.
    - *The budget.* Today's limits: the whole result as sent at most 24 KiB. The code that
      shapes an answer (section 9.3, "The answer budget is the caller's") cuts or exports it to
      the size of the line as sent in the negotiated protocol version, its exports in today's
      directory (`%TEMP%\actingcommand-mcp\materials`, `answer_exported`); a line that does not
      fit, of a row without shaping too, is sent once in `structuredContent` beside a summary
      text where the version has it, else answered `output_budget_exceeded` with the bytes it
      takes and the limit: never cut silently, never sent over the limit.
    - *The front's own members* (one table, `tools::SUPPLIED`): `material` and `frame` take no
      `out`, the front gives its export directory; `material` takes the boolean `export` (true:
      the whole material is written there and the answer names the file; else its text is read)
      and `frame` is always written there; `deploy` and `rollback` take no `detach`, which is
      always true through MCP (section 5), and since LAB-6S0 run detached as `Lab-MCP`.
    - *Refused before anything is sent or written,* with the argument as the call names it: as
      `arguments_invalid`, a member the row does not declare and a parameter the front gives
      itself (`unknown`), a member of the wrong type (`wrong_type`; text that is none of its
      words, or no duration, `not_recognised`), a required one missing (`required`) and a
      relative path in a parameter whose type is a path, on a row the server serves
      (`not_absolute`, its developer text saying an absolute path is required; stricter than
      today's server, which resolves or passes relative paths, and said in the property's
      description); and a string that begins with `--`, alone or in a list, as `serve` refuses it
      since LAB-5G4a (`actingctl_usage_invalid`, `not_recognised`). A member given as `null` is
      absent, as today's server reads it.
    - *A refused identity card:* where the Runtime refuses the card as
      `governance_client_not_allowed`, the client action and the write go ahead as today's
      server lets them, and the line carries the warning `mcp_identity_declaration_refused`
      (caused by the refusal), where today's server answers `mcp_identity_card_refused` with
      `mcp_blocker`, until LAB-6 removes the step.
    - *Every call runs to its end*, since Workflow #434 LAB-5G3 as a job (below).
  - **Jobs, handles and waits** (Workflow #434 LAB-5G3, 0.12.57, its first part; register
    §1.287, "Slice LAB-5G3"; the survey's §3 and the coordinator's rulings on board #434,
    6775192250 points 10 and 20, and 6784514298, not the owner's words). What today's MCP server
    has for long work, copied (its jobs, `actingctl`'s `mcp/jobs.rs`, and `job_answer`,
    `run_pack`, `stop_run` and `get_run`) and adapted to the generated tools.
    - *Jobs.* Every call the server serves runs as a job, on a thread of its own, to its end. A
      call not done within the call budget answers `{handle, job_phase}` with `mcp_job_started`
      (info) and its job goes on; `ac_run_show` with the handle as `id` reads it. A row the server
      does not serve and arguments it refuses answer at once, with no job. A job handle
      (`mcp_job_<session>_<n>`) lasts as long as the process; one it does not hold answers
      `handle_unknown`.
    - *Start options*, each a tunable with today's value as its default (ruling 20): the call
      budget `--call-budget <seconds>` (1 to 300, 25), the jobs that run at once `--jobs <n>` (1
      to 64, 8) and the jobs kept `--kept-jobs <n>` (1 to 1024, 64), each refused outside its
      range as `--workers` is. A call waits for its job up to the budget less today's 2 s for its
      answer (never more than a tenth of a shorter budget). Past `--jobs` a call that would start
      a job answers `job_capacity_exhausted` (`limit_count`); past `--kept-jobs` the oldest
      finished job is dropped, its handle then unknown. The tools' wait members and the
      instructions name the budget in effect.
    - *The front's added members* (one table, `tools::ADDED`): `ac_run_show` and `ac_run_stop`
      take `wait_s` (0 to the call budget; defaults 0 and today's 20, never more than the
      budget); since the second part, `ac_events` and `ac_codes` take `cursor` (below).
      `ac_run_show`'s `id` also takes a job handle (ruling 10): it waits up to `wait_s` for the
      job, then answers `{handle, job_phase}` or, once the job ended, its line; with a run it reads the run again every second while it
      is not found, admitted or running, for at most `wait_s` (and the budget less today's 5 s
      for the last read), as `ac_get_run` waits, and a run this process's job submits carries
      that job as `job`. `ac_run_stop`'s call waits up to its `wait_s` for its job; a run this
      process's job still submits is only asked to stop (`cancel_only`), as `ac_stop_run` asks it.
    - *`ac_run_start`* prepares the run after today's provenance and submits it in a job, and
      answers at once `{handle, correlation_id, job_phase: submitting}`: the handle is the run's
      Runtime request id, which outlives the process (with the row's `wait`, the call waits as
      long, within the budget, for the run's end). After an interrupted call a caller reads first
      (`ac_status`, `ac_run_show`) and never sends the write again, as the instructions say.
    - *A line that does not fit after its work was done* (the work order's point 10) is never cut
      and never lost: the call answers `output_budget_exceeded` (`actual_bytes`, `limit_bytes`)
      with its job's `handle`, the job keeps the line unread, and `ac_run_show` of the handle
      answers the same refusal with the job's `job_phase` and `summary` (the line's own outcome,
      the names of its other members, `warnings_total`, its `truncated`).
    - *`actingcommand/entryStatus`* answers today's members (`outstanding_handles`, `kinds`, the
      tools of the jobs' calls, `handles`, `calls_in_flight`): a job counts while it runs and,
      once it ended, until its line was read, if its handle went out to the client; a run's job
      only while it submits. The second entry switches only at zero.
    - *Progress, cancellation and the end of input* are today's: `notifications/progress` while
      the call is in flight and gave a progress token; `notifications/cancelled` stops the call's
      wait and drops its answer, its handle never issued, its job going on; at stdin EOF the
      server gives answers in flight 2 s and ends, the jobs not waited for (a run goes on in the
      Runtime, its request id still reads it; any other job ends with the process).
    - *Cursors taken back* (LAB-5G3's second part; register §1.287, "Slice LAB-5G3, second
      part"; the work order's point 6). An `events` cursor is taken back on the layer below the
      row (`events::dispatch_within` with the command's words and `--cursor`) under the server's
      binding, its session and the kept connection's generation: after the connection was made
      again it is `cursor_invalid`, nothing read. A `codes` cursor (`--cursor` to the command) and
      `ac_runs`' own `cursor` carry their own position and filters and bind to no session: they
      pass as the command line takes them. A cursor that begins with `--` is refused as `serve`
      refuses such a value.
  - **Calls served by `serve` children** (Workflow #434 LAB-5G4b, 0.12.57, its first part;
    register §1.287, "Slice LAB-5G4b"; the survey's §2, "Children, as the server must handle
    them"; the coordinator's rulings on board #434, 6775192250 points 2, 5 and 20 and 6827665631
    on this slice, not the owner's words).
    - *Which.* A row the server gives neither to `lab_ops::core_ops` nor to the rescue module, and
      that is not refused for a reason of its own (a rescue row where the module does not run,
      which is not offered), is served by a child: one rule (`tools::serving`, `Serving::Child`),
      no list of row names. On Windows all 143 offered rows are served (49 by `core_ops` and 33 by
      the rescue module in the server's process, `deploy` and `rollback` among them since
      LAB-6S0, 61 by children); elsewhere all 110 are served (49 and 61).
    - *The children.* `actinglab.exe` beside the server's own file, started as `actinglab.exe
      serve --stdio --front mcp` (section 8.2) with the environment today's server gives its Lab
      child: `ACTINGCOMMAND_RUNTIME_STATE_ROOT` set to the server's state root when it is known,
      `ACTINGCOMMAND_INITIATOR` removed, pinned to the server's installation as today
      (`lab_ops::child::pin_child`); its streams piped, no console window. A call takes an idle
      child or starts one and sends it one line `{op, params}`, one request per child at a time.
      At most `--children <n>` live (1 to 64, default 8); a call while every one is busy is
      refused, not queued, with the code a job past `--jobs` answers, `job_capacity_exhausted`,
      its `limit_count` the children's limit. A child idle for `--child-idle <seconds>` (0 to 600,
      default 10; 0: at once after its answer) is ended by closing its input, by a thread of the
      server's, without waiting for the next call (ruling 5: a resident server keeps no idle
      child, which would block a deploy). Both options are refused as `--jobs` is: outside the
      range, no whole number, twice, without a value, and beside `--list-tools`.
    - *The answer.* The child's envelope (section 8.2): its `line` (a JSON object, as every line
      `serve` answers; since the slice's second part an envelope whose `line` is none is no
      envelope) is the call's line, text and `structuredContent`, and `isError` is true exactly
      when its `exit_code` is not 0, after what the server does with it (below); a refusal
      the child answers is passed on as it is (a row no command of actinglab serves would be
      refused so; since LAB-5G4c, in the same release, there is none: `record draft` has its
      command). The server checks the arguments first as for every row it
      serves (undeclared and the front's own members, types, required members, relative paths,
      values that begin with `--`), so a call it refuses starts no child; the child checks them
      again.
    - *A child that fails.* A child that ends, or whose output ends, before its answer, or whose
      answer is no envelope, is answered `lab_process_failed` (`exit_code` when it exited) caused
      by a `foreign_other_error` from the process holding the tail of its stderr (its last 2 KiB,
      `raw_text_truncated` when cut), as today's server answers a Lab child that printed no
      answer; the request is never sent again, the child is dropped, and the next call starts a
      new one. A child's stderr is read as it comes, so a full pipe never stops it.
    - *Long calls and the end of input.* A call not done within the call budget answers its job's
      handle as every job does (above); its child stays busy until its line comes and is never
      killed. At the end of the server's input every idle child's input is closed at once, and a
      working one's once its line came or when the server's process ends after its 2 s grace:
      none is killed, each ends after the line it is working on. A call still running then
      leaves its operation running to its end in the child (what it records or sends stays
      recorded and sent), loses its answer (the child's write fails and it ends with
      `actingctl_output_failed` on its own stderr) and its job's handle, which ends with the
      server's process: read the effect before sending the call again.
    - *What the server does with a child's answer* (the slice's second part, 0.12.57; register
      §1.287, "Slice LAB-5G4b", rows L5G4b-10 to L5G4b-14; the coordinator's rulings of
      2026-11-13 on board #434, not the owner's words): the two things today's server does around
      its Lab child (`apps/actingctl/src/mcp/lab.rs`), ported to the child's line, for the rows
      of one table (`after_child::LAB_ROWS`): `record stop` (today's `ac_record_stop`), `observe`
      (`ac_lab_observe`) and `do` (today's `ac_lab_do`, this server's `ac_do`). The operation
      table cannot say what that table says: it names no member of a row's line, and seven of its
      rows take `capture`, of which today's server records a client action after two.
      - *The client action after a capture* (ruling 12 of 6775192250; today's `author` and
        `record_action`): once the child of a call of `observe` or `do` with `capture` true
        answered, whatever `dry_run` says, the server records one client action on a connection
        of its own, through the steps of a write's provenance (above): the identity card
        `actingctl-mcp`, then `{surface_id: mcp, control_id: <the tool called>, kind: command,
        instance_alias: <the call's instance as given>, value: <the envelope's req_id as a
        path-safe string>}`, from (Cli, Cli), on a connection that binds the MCP channel until
        LAB-6S2-B1 and since LAB-6S2-B1 (0.12.59) the Lab channel with `Lab-MCP` and no `via`,
        which the Runtime records on the MCP channel (section 2.4, the map M). Its warnings,
        today's three, join the line's after its own: `client_action_without_req_id` when the
        envelope carries no `req_id` (the action is recorded without a value), the recording's own
        (`mcp_identity_declaration_refused`), and `client_action_not_recorded` caused by the
        failure (the state root not located, the Runtime's or the client's failure) when it was
        not recorded. The call's answer stands, and the Lab command is never run again. A call no
        child answered (refused past `--children`, or a child that gave no answer) records
        nothing.
      - *The Lab overflow rule* (today's `finish`, `fits_as_job` and `keep_frame_parts`): a
        child's success line (`exit_code` 0), with those warnings joined, that does not fit the
        budget as the server measures an answer in the call's protocol version (above, "The
        budget") is written whole, the child's line, by the server's export writer
        (`<sha256>.json` in `%TEMP%\actingcommand-mcp\materials`), and the call answers
        `{req_id, export, overflowed: true}`: `req_id` the envelope's (`null` without one),
        `export` the file's `{path, sha256, size}`; then the parts its row keeps, each while the
        answer still fits, in today's order (`record stop` its four binding parts together under
        `lab`; `observe` and `do` `frame_coordinates`, `frame_path`, `preview` and
        `after_attachment`, each tried in turn); then `warnings`, `answer_exported` first (or the
        export's `material_export_failed`, and no `export`, when the file could not be written)
        and the line's own after it; then the line's `outcome`. A failure line is never exported:
        it is sent as every line over the budget is (`structuredContent` alone beside a summary
        text, or `output_budget_exceeded` with its job's handle), and so is an answer that does
        not fit even so.
  - **A row the server does not serve** answers `mcp_operation_unsupported` (`mcp_tool`) as the
    CLI line of a refusal, `{outcome}`, its arguments unread, nothing sent, run or written. Since
    LAB-6S0 (0.12.58) no offered row is one, on any platform: `deploy` and `rollback` are served
    (section 5), and a rescue row where the module does not run is not offered. A name that is no
    offered row's tool answers as today's server answers an unknown tool (-32602
    `mcp_params_invalid`). The manual is
    [apps/actinglab-mcp/MANUAL.md](../apps/actinglab-mcp/MANUAL.md) since LAB-5G7 (0.12.57;
    register §1.287, "Slice LAB-5G7"), which the instructions name. The program is in
    the Tools archive since LAB-5G6 (0.12.57; section 8.1, "Packaging"); nothing starts it until
    a client is registered with the second entry (G7).
- **The answer budget is the caller's** (Workflow #434 LAB-5D, 0.12.52; register §1.287, "Slice
  LAB-5D"). The reads whose answer can outgrow a caller's budget (`status`, `diagnose`, `events`,
  `schedule`, `runs`, `run evidence`, `codes`; `facts`, `packs`, `remedy`, `summary`) take, in
  `crates/lab-ops`, the budget their caller passes (`lab_ops::budget::Budget`): whether an answer
  fits, and where one that does not is written whole, when the caller has such a place. An
  operation cuts its answer, in its own order and leaving a marker at every cut (never silently),
  or writes it whole and answers its header with the export reference and `answer_exported`, only
  when its caller passes a budget; a caller that passes none gets the whole answer. The CLI, the
  CLI module and `serve --stdio` pass none. MCP passes its own: the size of an answer as sent in
  the protocol version the session negotiated, and its export directory. The cursors it binds to
  its connection, `output_budget_exceeded` for an answer that does not fit even so, and the Lab
  tools' overflow rule stay MCP's. What MCP answers does not change: runtime-interface.md, "MCP
  (counter `mcp`)", still says what it is.
- **Help** (Workflow #434 CLI-HELP, 0.12.55; register §1.313; the coordinator's rulings of
  2026-11-11, not Alice's words).
  - `actinglab help`, and the bare `actinglab`, print without `--json` a text a person reads: the
    usage lines; the global options, one phrase each (the retired `--require-session` is not
    listed); then every command this build offers, grouped: the offered rows by module (core,
    debugging, authoring, rescue; a row a platform does not offer is not listed, and the authoring
    rows only where section 8.3 lists them), then actinglab's own commands that no row serves or
    replaces. Each entry: its words, its parameters in short form and its one sentence. Plain
    ASCII, at most 100 columns, no colour, no paging.
  - A row's sentence is its `help`, read from the table the build embeds. An own command without
    a row has one sentence in one static table of actinglab's code (`cli_help::OWN_WITHOUT_ROW`),
    and none of the table's three facts.
  - `actinglab help <command words>` and `<command words> --help` print one command: its usage
    (a row's name first, then the words of the own command that serves it under other words, and
    the own commands it replaces, each with parameters of its own); its sentence; its module and
    whether it writes, is irreversible and needs the Runtime; each parameter with its form, type,
    whether it is required or repeats, its default and its note. The instance is written as the
    first rule of this section says: `<instance>` (`[<instance>]` when optional) on a row of the
    CLI module or of an own command under other words, `--instance <instance>` on the 20 rows
    actinglab's own command serves under the row's name and on the two that take it as an option.
    Words beyond a command's own (its positionals) are left out; words that begin no command are
    refused as `help`'s unknown word (`validation_failed`, `argument_problem` `unknown`).
  - The words of a group (`lab`, `package`, `session`, `run`, `record`, `emu` and the like) after
    `help` or before `--help`, and one of actinglab's own groups followed by `help` (`lab help`),
    list the group's commands the same way, each by its words under the group and with the row
    that serves or replaces it named.
  - `--help` anywhere on a line asks for the help of the line's command words (actinglab's global
    options and their values left out, up to the first other flag) and runs nothing: the line is
    `help`'s before anything else reads it, and nothing is sent. A row's words keep a `help` after
    them as the row's word: `status help` names the instance `help`, as before; its help is
    `status --help` or `help status`.
  - `--json` prints `help`'s line: every member it answered before, unchanged, and `list`, the
    same list as data, each command's `name`, `module` (`own` for an own command without a row),
    `help`, `usage`, `params`, `writes`, `irreversible`, `runtime` (`null` for an own command
    without a row), `spellings` and `replaces`. `help <command words> --json` answers `command`,
    one such entry, or `group` and `members`. A line with `--via` keeps its envelope.
  - The module's refusal of a row's words names `actinglab help` in its developer text, where it
    named this table's file.
- **Completion** (Workflow #434 CLI-COMP, 0.12.55; register §1.318; the coordinator's rulings of
  2026-11-11, not Alice's words).
  - `actinglab completion powershell` and `actinglab completion bash` print a script for a person
    to load from the shell's profile (without `--json` the script as it is, with it `{shell,
    script}`); actinglab and the installer never edit a profile. PowerShell: `actinglab
    completion powershell | Out-String | Invoke-Expression` turns it on in the session, and the
    same line added to `$PROFILE` in every session. bash: `source <(actinglab completion bash)`,
    and the same line added to `~/.bashrc`. Another shell word, or none, is refused as a read list
    refuses one (`validation_failed`, `argument_problem` `not_recognised` or `required`).
  - The scripts are plain ASCII and thin: each registers itself for `actinglab` and
    `actinglab.exe` (`Register-ArgumentCompleter -Native`; `complete -o default -F`) and, at each
    Tab, calls back the program the line names (its first word as typed: `actinglab` on the path,
    or the path the person typed) with the words typed so far and the index of the word being
    completed, so that a new row or a renamed word needs no new script. Where the callback gives
    no candidate the shell completes file names itself; that is how a `path`, `file` or `dir`
    parameter is completed.
  - The callback, `actinglab __complete [--json] [--wait-ms <ms>] <index> <word>...`, is an own
    command that is not for people. Its words are every word typed so far, the program's first
    (index 0), up to the one being completed, each in the word form: `:` and the word with `%`,
    `"`, a space and a tab written `%25`, `%22`, `%20` and `%09` (it reads any `%` and two hex
    digits as that byte). It prints the candidates of the word at the index, by its prefix, sorted,
    one per line and nothing else; with `--json` the line of an own command with the member
    `candidates`. It runs nothing, sends no request that changes anything, writes no file, and
    exits 0 with no candidate when it has none or anything goes wrong, writing nothing on stderr;
    `main` answers a line that begins with it before anything else reads the line.
  - What completes, from what the build knows: the command words `actinglab help` lists (the
    offered rows' names, the words of the own command that serves a row or that a row replaces,
    the own commands without a row; the callback's own word left out), by prefix at each position;
    `help <words>` as the words. After a row's own name: its options and switches (`--name`, `_`
    as `-`), not one already on the line unless it repeats; an enumeration's words after its
    option or in its positional's place; the global options its line takes (where the row's
    command takes them as written, those a person gives, else `--json` and `--help`), and
    `--instance` while it may still name the row's instance. After an own command without a row,
    or a row's other spelling: the global options only (their own flags come with a later slice).
    The global options a person gives are those `help` lists but `--via`, `--instances`,
    `--profile` and `--quiet`.
  - An instance, wherever a row's parameter is one (the positional, `--instance` before or after
    the words, or the instance option of `remedy set` and `wake-test`; on the 20 rows actinglab's
    own command serves under the row's name only `--instance`, never in a positional's place):
    the aliases of the running Runtime, asked with `Status`, the read that resolves `<instance>`
    today, on one Lab connection whose every exchange ends by the wait (`--wait-ms`, default 500
    milliseconds; 0 asks no Runtime); where no Runtime answers in time, those of the selected
    configuration the offline rows read; none where neither reads. The handshake and `Status`
    record nothing; no configuration key is read for the wait.
  - Not completed: a file, path, directory, key, id, number, duration, JSON or free text, and an
    own command's own flags.

### 9.4 Changing the table

A slice that builds, changes or opens an operation edits its row in the same pull request. A
`reserved` row becomes `planned` with the slice that opens it. The schema string changes only
when a member's meaning changes.

## 10 Interfaces

In force from LAB-6S1a (Runtime 0.12.58): `runtime-client` [3, 8], revision 8 being the
initiator of section 2.2 on `BindChannel`, `ChannelBound` and the receipt
([component-interfaces.md](component-interfaces.md), the revision table). Planned, not in force:
the same file, "Planned: Workflow #434". The numbers this contract first planned for LAB-6
(`runtime-client` 7, `ledger` 8, `actingd-config` 28) were taken by other slices: LAB-6S4a
declares `install-control` [0, 0], LAB-6S4b raises `runtime-client`'s `min` to 8 and
`actingd-config` to 30, LAB-6S5 `ledger` to 9; LAB-7 `install-cli` to 2. `install-control`
retires, `tools-layout` becomes 5 as that section plans it,
`install-selection` does not move, and the MCP entry's second generation, a program of its own
(section 8.1), speaks `entry_protocol` 2 while the first keeps 1 (LAB-5). `actinglab-mcp.exe` and
the second entry raise no layout: LAB-5G6 adds them inside `tools-layout` 3 (section 8.1). LAB-0
changes no declaration.

## 11 What this replaces, and from when

| Text | Replaced by | From |
|---|---|---|
| runtime-interface.md: the three counters, `via`, the Lab options' gate, "The gate's decision (from U-6)", "Before U-6", "Without the Lab, or with one option", the origin gates, the clean mapping's actingctl and MCP counters | sections 2 and 3 | LAB-6 (the mapping: LAB-2 for actingctl's; for MCP's LAB-7, since LAB-5 adds its server beside `actingctl mcp-serve`, whose actinglab child with `--via` stays until LAB-7) |
| runtime-interface.md: the plans U-3 to U-6 and the upgrade state | this contract; they are not built | now (no code follows them) |
| component-interfaces.md: "Planned: `install-control` 2" | `install-control` retires | now |
| install-cli.md: "no MCP tool drives it" (also in MCP's instructions) | MCP covers what the CLI covers, rescue included (Alice: 「Lab不设权限管理」) | LAB-5 |
| install-cli.md: `--plan <id>` for apply, `--accept-ledger-one-way`, `clean --ui-confirmed … --via`, `--known-pause`, `--ack-guard`, guards that refuse | sections 5 and 6 | LAB-3 in the rescue module; LAB-7 for `actingctl install` |
| install-cli.md, "Lab options": two options | section 8.3 | LAB-3 (writers), LAB-6 (the key) |
| actingd-unlock-owner.md: `--confirm-resources-released` | section 5 | LAB-7 |
| #421's body: "plan, then apply with the plan id", "no MCP entry" | sections 5 and 9 | LAB-3, LAB-5 |

Until the slice named, the replaced text stays in force and its code unchanged. LAB-1 to LAB-5
only add: actingctl stays until LAB-7.

## 12 Codes

Two new codes of the owner `actinglab-mcp`: `mcp_operation_unsupported` (`mcp_tool`; LAB-5G2a),
a call of an offered row the MCP server does not serve (since LAB-6S0 no offered row is one; the
code stays registered with the server's check, section 9.3), and `mcp_identity_declaration_refused`
(warning; LAB-5G2b), the Runtime refused the identity card declared before a write (section
9.3). LAB-5G2b adds the word `not_absolute` to the vocabulary `argument_problem` and reuses
today's codes for the rest. The codes this contract uses:

- `channel_request_refused` (`operation`, `channel`), with `config_key` for a refused setting: an
  optional value LAB-6 adds to its catalog entry (added by LAB-6S3, 0.12.58, which first emits the
  code, on `ui`);
- `channel_binding_missing`, `channel_binding_exists`, `invalid_bind_channel`;
- `lab_command_option_disabled` (`operation`, `lab_option`): registered, and not emitted for the
  authoring option, whose key only lists (section 8.3; LAB-5F fix 2): the design's
  `lab_module_not_installed` is a host code, and the host no longer judges modules;
- `actingctl_usage_invalid` (`argument`, `variable`) for a bad option or `ACTINGCOMMAND_INITIATOR`;
  LAB-7 moves actingctl's codes into actinglab's fragment
  (`contracts/outcome-codes/actingctl.json` into `actinglab.json`);
- `install_caveat_noted` with the caveats `guard_failed` (new word, LAB-3) and
  `config_key_retired`;
- `lab_operation_check_failed` (`req_id`, `receipt_state`, `client_code`), actinglab's, for a
  request the Runtime answered whose client check then failed (section 9.3); Workflow #433
  LABDO-1 added it, not a slice of this contract.

After LAB-6 the host emits neither `lab_module_not_installed` nor `lab_module_option_disabled`
(open point 8).

LAB-6S1b (0.12.58) retires the origin codes of `RuntimeRequest::validate()`, which nothing emits
once its origin gates are gone: `invalid_shutdown_origin`, `invalid_install_transition_origin`,
`invalid_resource_authoring_origin`, `invalid_signature_origin`, `invalid_runtime_debug_origin`,
`invalid_emulator_control_origin`, `invalid_scheduling_pause_origin`,
`invalid_agent_wake_test_origin`, `invalid_agent_dispatcher_origin` and
`invalid_player_hold_origin` (each `retired_in` 0.12.58, no `replaced_by`: the Runtime takes the
request). `invalid_governance_origin` stays active: the host's own check of the identity card
emits it until LAB-6S4a deletes the kind. `invalid_client_origin` and `fact_origin_mixed` stay.

## 13 Open points

1. **Who is `Lab-制包` and `Lab-救急`.** Section 4.1 records where a request came from: the two
   words mark Lab processes the UI starts, and an authoring or rescue operation typed at a command
   line is `Lab-CLI` (through MCP `Lab-MCP`). The other reading, the module that serves the
   operation, would record every deploy as `Lab-救急` whoever started it. To confirm.
2. **The final words.** §7 says the module names follow the final list. They become a closed
   vocabulary with LAB-6; until then a rename changes only this text and the table.
3. **Frames of `frame` and `frames`.** On the `lab` entry they pin `Lab` and stay in `kept\`
   (section 2.4), where today's CLI and MCP pin `Explicit`. LAB-6 either keeps that or pins these
   two by a request member; the design does not say.
4. **The memory wait.** Section 6 keeps the start's wait for memory as a wait. If the wait should
   go too, LAB-3 drops it. LAB-3 keeps it (its work order): a start of the rescue module waits up
   to `install.start.memory_wait_ms`, reading memory every 500 ms, then goes on and journals and
   reports the row (`guard_failed`); dropping it later changes only that wait.
5. **`install-root`.** The journal's `initiator` and `one_way` are members of the installers'
   private documents; whether they raise `install-root` (the A/B journal v2) is LAB-3's to judge by
   the bump rule. Settled by LAB-3: no raise. `install-root` 1 covers the A/B root's private files
   (`install-operation.v2`, the slot documents), which no build writes yet and no build declares
   (component-interfaces.md: every build reads as `install-root` [0, 0]); the members go into the
   single-location journal `install-operation.v1`, which that interface does not cover; both are
   optional members written only by the rescue module, so `actingctl install` writes the journal
   byte for byte as before; an older installer refusing a journal the new one wrote is the old-
   version compatibility testing does not keep (Alice, 2026-10-11). The A/B journal v2 takes them
   when R-15c builds it, inside `install-root` 1.
6. **Recordings' initiator.** Settled (the coordinator's ruling, 2026-11-09): LAB-2 writes it,
   section 4.3 "Recordings"; the format changes without compatibility.
7. **actinglab's own `config get` and `config set`** (its user configuration file) collide with
   the `config` operations. Settled (the coordinator's ruling, 2026-11-09): LAB-2 renames them
   `user-config` and `user-config set`, two `debugging` rows of the table (`lab-operations.json`),
   which do what they did; tests and `external-tools/NOTICE.md` use them, so they stay.
8. **The two host codes** `lab_module_not_installed` and `lab_module_option_disabled` lose their
   emitter at LAB-6; whether the catalog retires them is LAB-6's.
9. **The MCP tool count.** The table has one operation per CLI form (168 in LAB-0's v1; 170 since
   LAB-2 added the two `user-config` rows), so MCP would list
   that many tools. LAB-5 may present a family (`record`, `package`, `emu`) as one tool with an
   `action` argument; the table stays one row per form either way.
10. **actinglab's own commands take the new output.** Done (LAB-2b, 0.12.50; the coordinator's
    rulings of 2026-11-10, LAB-2 fix 1 and fix 2, and those of the LAB-2b work order; section
    9.3). actinglab's own commands print the CLI module's output (the line for people, `--json`
    printing the `cli_line` line, exit codes 0, 1 and 2); a result that names `outcome` goes whole
    into its declared member (`resource verify`: `report`, `run summary`: `summary`); a failure is
    `{outcome, message, details}`. `status` and `run show` reach their core rows on the command line
    as through `serve --stdio`, and actinglab's own two implementations are deleted. Their
    consumers move with it: acmodel's validator (UI repository,
    `model/src/acmodel/export/validator.py`), the four resource repositories' CI and scripts
    (`resource-declarations.yml`, `bundle-check.yml`, `scripts/bundle.ps1`,
    `scripts/validate-local.ps1`, `scripts/regression.ps1`) and the coordinator's hotplace kit
    (`hotplace.ps1`, `tools/composer_check_r10.py`) read both shapes before the deploy and raise
    their pins with that change (the slice's pull request, "At deploy"). MCP's actinglab child
    (`--via`) keeps the envelope until LAB-7: LAB-5 adds its server beside `actingctl mcp-serve`
    and replaces neither (section 11; corrected with LAB-5A).
