# Runtime outcomes (`runtime-outcomes`, revision 1)

Workflow #378. Every result that leaves a Runtime process, success included, carries one
registered **code** with one **category**, typed **values** under the keys of one key table,
and optionally a nested chain of **links**. The Runtime stores no explanation next to a code:
meanings live in the catalog, and the UI adds the ZH and EN sentences. Runtime English exists
only in developer log lines and `Display`.

Slice A1 (v0.12.0) delivers the contract module (`actingcommand_contract::outcome`), the
catalog files and the drift guard `tools/outcome-guard`. The surfaces below adopt the envelope
in later slices (A3 ledger, A4 IPC, A5 CLI/MCP/watchdog, A6a actingd); until a surface has
moved, it keeps its v0.11 shape.

## Envelope `actingcommand.outcome.v1`

```json
{"schema_version":"actingcommand.outcome.v1","code":"adb_install_mismatch","category":"fatal",
 "values":{"stage":"check_adb","adb_path":"F:\\Runtime\\tools\\platform-tools\\adb.exe",
   "aliases":["main"],
   "files":[{"path":"F:\\Runtime\\tools\\platform-tools\\AdbWinApi.dll","state":"unreadable"}]},
 "causes":[{"code":"foreign_os_error","category":"error","relation":"caused_by",
   "values":{"path":"F:\\Runtime\\tools\\platform-tools\\AdbWinApi.dll","os_error":5,
             "io_kind":"permission_denied","io_op":"open","raw_source":"os",
             "raw_text":"Access is denied. (os error 5)"}}]}
```

| Member | Rule |
|---|---|
| `schema_version` | top only, `actingcommand.outcome.v1` |
| `code`, `category` | always; one category per code (`success`, `info`, `warning`, `error`, `fatal`) |
| `values` | required (may be `{}`) unless `detail` is `codes`; keys and types from the catalog `keys` |
| `causes` | optional; links `{code, category, relation, values, causes?}` |
| `causes_total` | top only, present when links were cut: the full link count |
| `detail` | `codes` when data values were withheld; absent means full |

- **Readers** ignore unknown members, and keep an unknown code or relation readable as it came
  (`CodeStr`, `Relation`).
- **Relaying.** A received envelope is attached as a link, `{code, category, relation, values,
  causes?}`: it keeps its code, category, values and nested links, takes the relation the
  relaying outcome gives it, and has no `schema_version`, `detail` or `causes_total`.
- **Relations** (vocabulary `cause_relation`): `caused_by` (default), `cleanup`, `secondary`,
  `recording`, `related`, `after_commit`, `note`, `diagnostic_summary`, the 21 names the Runtime
  host passes to `with_related_failure` and the 3 `RuntimeFailureRelation` joins.
- **Bounds.** At most 32 links within depth 8, kept breadth first; at most 32 KiB per envelope
  (the emitter first empties `raw_text` from the deepest link up, then drops the deepest links);
  any cut sets `causes_total`. `raw_text` keeps at most 4096 bytes and sets `raw_text_truncated`;
  a list or records value keeps 64 items and adds `<key>_total`. Cut text goes to the
  diagnostic sink. The record form (ledger, A3) is the same body without `schema_version` and
  `detail`, always full.
- **Codes mode** (`detail: "codes"`) keeps every node's `code`, `category` and `relation`, and
  only the values whose type is `code`, `location`, `vocab:*` or a list of them. One exception:
  a denied or failed receipt keeps its top's `host_halted` in every detail mode, because the
  client's own fatal handling runs on it (Workflow #378 A4, coordinator ruling A-M1).
- **Foreign text** goes only into `raw_text` on a `foreign_*` or `panic_caught` link, verbatim.
  A child process's foreign code carries `exit_code` when the process exited.

## Categories and exit codes

| Category | Meaning | Exit code |
|---|---|---|
| `success` | done as asked | 0 |
| `info` | a neutral fact or state change | 0 |
| `warning` | done or continuing, but something was skipped, degraded, uncertain or caught | 0 |
| `error` | the operation failed; the Runtime, a restart or its operator can recover | 1 |
| `fatal` | the Runtime cannot return to normal operation without an external fix | 2 |

A fatal link requires a fatal top. A process exits by its top outcome's category (A6a).

## Classes

| Class | Channel | Default |
|---|---|---|
| (a) full-function client | IPC with `source: ui` | `codes` |
| (b) CLI and agents | IPC from `cli`, `adapter`, `lab`; actingctl stdout; MCP; actingd subcommand reports | `full` |

A request's `detail` (`codes` or `full`), actingctl's `--detail` or `ACTINGCOMMAND_DETAIL`, and
the MCP server option override the default. Daemon `OUTCOME` lines and logs are always full.
Between Runtime modules outcomes always travel full.

## Surfaces and line formats

| Surface | Form | Slice |
|---|---|---|
| actingd lifecycle | `OUTCOME actingd: <envelope>` on stderr, written in the same `write_all` as its developer line `<INFO\|WARN\|ERROR\|FATAL> actingd: <code> <text>` | A6a |
| start refusal before the ledger opens | the same two lines; also appended to `%LOCALAPPDATA%\ActingCommand\logs\actingd-<ms>.log` when stderr is not a file or a pipe, with `install_root` once known | A6a |
| actingctl | exactly one stdout JSON line per exit, `{…result, "outcome": …}` or `{"outcome": …}`, before any stderr text; `mcp-serve` and `mcp-config` print none | A5 |
| check-config | `actingcommand.actingd.check-config.v2`, `{schema_version, outcome, report?}` | A6a |
| watchdog | `actingcommand.watchdog.report.v2`, `{outcome, attention: [outcome…], …}` | A5 |
| IPC | request v4 `detail`; receipt v2 `outcome` on every receipt | A4 |
| MCP tool error | `{class, outcome}`; JSON-RPC transport errors put the code's spelling in `error.message` and `{outcome}` in `error.data` | A5 |
| ledger | `outcome` member of result-bearing payloads, `severity` = category | A3 |
| developer text | `DIAG <program>:` lines from the diagnostic sink; never an interface | A6a |

## Catalog files

| File | Holds |
|---|---|
| `contracts/outcome-codes/catalog.json` | the header: `schema_version` `actingcommand.outcome-codes.v1`, `keys`, `vocabularies`, `domains`, `fragments` |
| `contracts/outcome-codes/<owner>.json` | one owner's `codes` and `locations` (`{owner, codes, locations}`) |
| (merged catalog, not committed) | the header and every fragment merged into one document, the one every reader uses: the contract crate's `build.rs` generates it into its `OUT_DIR` and embeds it, and `outcome-guard merge` prints the same text (Workflow #420 S2) |
| `contracts/outcome-codes/released/<version>.json` | per-release release file in the compact form, written by the release PR with `ci/outcome_release.py` (`0.12.0.json` first, #378 A8b; compact since #420 S3) |

- **`fragments`** maps each owner to its workspace member (`"runtime-host": "crates/runtime-host"`).
  Every workspace member has exactly one owner; an owner's fragment file exists once it
  registers an entry. Owner names are the package names without `actingcommand-`.
- **`domains`** maps a prefix (ending in `_`) to its owners; an entry's longest matching prefix
  decides. `setup_` and `console_` belong to the UI (`ui`); `foreign_` and `panic_` to the
  contract crate.
- **`keys`**: `{type, description, fields?, relative?}`. Types: `code`, `location`, `vocab:<name>`,
  `token`, `name`, `id`, `path`, `pointer`, `url`, `integer`, `duration_ms`, `unix_ms`,
  `boolean`, `hash`, `commit`, `version`, `evidence` (`raw_text` only), `list<T>` (T a scalar or
  vocabulary) and `records` (flat; `fields` maps each field, itself a key, to `required` or
  `optional`). Keys are append-only: a key is never renamed, retyped or reused. Keys named
  `setup_*` or `console_*` belong to the UI's own table.
- **`vocabularies`**: `{description, tokens: {token: {description, category?}}, source?, review?}`.
  `source` `{file, enum}` names the Rust enum the tokens must equal; every `event_type` token has
  a category. Within a ledger revision an event type's category is frozen: a record without an
  outcome stores it as its severity, so recategorising a token needs a ledger revision bump.
- **Code entry**: `category`, `ladder_class`, `owner`, `layer`, `uncertain`, `values`
  (`key: required|optional`), `review` (`pending`|`settled`), `status` (`active`|`retired`),
  `since`, `description`, `common_causes`, `handling` (`TBD` while pending), and `retired_in`,
  `replaced_by` once retired. A location entry has `owner`, `layer`, `review`, `status`, `since`,
  `description`.
- **`ladder_class`** (Workflow #387 D1a): how the recovery ladder's trigger matching reads the
  code. Every code entry, retired ones included, names exactly one token of the vocabulary
  `ladder_class`; a `stability_warning` code is of category `warning` or `error`. The class does
  not decide whether a code starts the ladder; the ladder's configuration does, by class or by
  code. Once settled, the class never changes (G9). The class is the first row, in this order,
  whose rule the code meets:

  | # | `ladder_class` | Rule |
  |---|---|---|
  | 1 | `planned_stop` | a person, a rule or the pack's design stopped the run on purpose |
  | 2 | `budget_or_permission` | the work lacked time, samples, attempts, quota or permission |
  | 3 | `declaration_error` | a pack, a binding or a configuration the work uses is itself wrong or incompatible; decided without looking at the screen, the same on every run |
  | 4 | `pack_logic_mismatch` | the screen or the flow differs from what the pack declares; known only by looking at the screen |
  | 5 | `environment_fault` | a device, a backend, ADB, the emulator or the vision provider could not open or failed, and the work ended because of it |
  | 6 | `stability_warning` | a device, connection, capture or input anomaly on one instance that the Runtime retried or survived (category `warning` or `error`) |
  | 7 | `none` | a success or an info, a Runtime-internal invariant, a refused request or contract value, a client error or a configuration error |

  Rows 1-6 describe how work on an instance (a run, its preparation, its device) ended or
  degraded. A code of a process start or a command line, a client or MCP call, the Runtime's own
  configuration or a refused contract value is row 7, whatever its words; a pack, binding or
  configuration that a run uses and finds wrong is row 3. Rows 3 and 4 part at the screen: a
  condition decided from the declarations alone is row 3, one that needs the screen is row 4.
  A receipt or projection code takes the class of the outcome it projects: `recognition_failed`
  is `pack_logic_mismatch`, `capture_failed` is `environment_fault`.
- **Release files** (compact since Workflow #420 S3): `{schema_version:
  "actingcommand.outcome-codes.release.v2", release, codes: {name: {category, ladder_class,
  review, status, values: {key: {type, required}}}}, locations: {name: {review, status}}}`, one
  entry per line, in name order. An entry's signature is these members; `type` is the key's type
  in the key table and `required` whether the entry requires the key. A release file holds only
  the entries its release binds, the settled and the retired ones, and of those only the ones
  whose signature no earlier release file holds; a release that changed none has a file with
  empty `codes` and `locations`. Together the release files hold every signature any release
  bound, so G9 checking each file is G9 checking each release's full snapshot. The release PR
  writes `X.Y.Z.json` with `py -3 -I -B ci/outcome_release.py X.Y.Z`, from the header and the
  fragments, and nothing edits it afterwards; G9 holds every later catalog to the signatures,
  and G10 stops a release whose file is missing. A release candidate `X.Y.Z-rc.N` has no file
  of its own.

### The merged catalog is generated, not committed (Workflow #420 S2)

A code change edits only the header and the owner fragments; nothing regenerates, downloads or
commits a merged file. The contract crate's `build.rs` merges the fragments with the guard's own
loader (`tools/outcome-guard/src/catalog.rs`) into its `OUT_DIR` at every build, and a header or
fragment that does not load fails the build with every error. So a pull request that changes
codes is green on its first push, and two pull requests that change different fragments never
meet in one file. After a rebase or merge from `main`, keep both sides' entries in the fragments
and the header. `contracts/outcome-codes.json` is no longer tracked: a branch that still carries
it deletes it when it merges `main` (accept the deletion) and keeps only its fragment changes.

### Reading the catalog: `actingledger codes`

`actingledger codes [--code <code>] [--json | --markdown]` (#378 A8b) prints the codes of the
catalog its build embeds, offline: it takes no `--state-root` and reads no ledger. Every form
lists the codes in name order, or only the code `--code` names.

| Form | Output |
|---|---|
| (default) | one line per code, tab-separated: code, `category`, `ladder_class`, `review`, `status`, `description` (each on one line) |
| `--markdown` | the line ``Outcome catalog SHA-256 `<hex>`.``, an empty line, then a table of the same columns, the code in backticks and a pipe in a cell escaped with a backslash |
| `--json` | one line `{schema_version: "actingcommand.actingledger.codes.v1", catalog_sha256, codes: {name: entry}}`, each entry as the catalog holds it |

`catalog_sha256` is the SHA-256 of the embedded merged catalog (the text `outcome-guard merge`
prints for the same fragments), the content half
of the catalog's version; the Runtime release is the other half and lives only in the release
tag, so no binary prints it. A code the catalog does not list is `outcome_code_unknown`, and a
catalog that does not parse, has no codes table or holds an entry without one of the row's text
members is `outcome_catalog_unreadable` (both at `read_outcome_catalog`, the codes `actingctl
codes` answers with too); a usage error is `invalid_arguments` at `parse_arguments`. Each prints
`actingledger: <code> during <location>: <text>` on stderr and exits 1, like every
actingledger failure. `actingctl codes` and `ac_outcome_code` (#389 MCP-14) read the same
catalog as pages for agents.

## Drift guard `tools/outcome-guard`

A source scanner that links no workspace crate; its tests run in `cargo test --workspace`.
Crate roots come from each manifest (`[lib]`, `[[bin]]`) and Cargo's defaults; the module tree
is walked from them. Code under `#[cfg(test)]` or `#[cfg(all(test, ...))]`, `#[test]` functions
and files reached only through them are skipped; any other predicate, `not(test)` included, is
scanned. An unparsable registry macro fails G1.

| Check | Fails on | On since |
|---|---|---|
| G1 | a registry entry (`outcome_codes!`, `outcome_locations!`) missing from the fragments, or of another owner, category or ladder class, or retired; an active fragment entry with no registry entry; a spelling registered twice | A1; ladder class #387 D1a |
| G2 | a string-typed field or parameter named `code`, `*_code`, `reason`, `*_reason`, `failure`, `operation`, `*_operation`, `stage`, `*_stage`, `boundary`; a `Result` whose error is a string; a getter `code`, `*_code`, `reason`, `*_reason`, `key`, `operation`, `stage` returning a string. A tuple, array or slice holding a string type counts. A non-test `include!` is reported too, since the guard cannot see the included source | A2a-A2e, member by member |
| G3 | a `json!` key `code`, `*_code`, `reason`, `*_reason`, `category`, `operation`, `*_operation`, `stage`, `*_stage`, `boundary` | A2a-A2e |
| G4 | `From<String>`/`FromStr`/a string constructor for `Code`, `Location`, `CodeStr`, or a registry constructor, outside the outcome module | A2a-A2e |
| G5 | a non-test string literal equal to a registered code or location outside its registry | A2a-A2e |
| G6 | persisted prose fields in contract types | A3 |
| G7 | `outcome_keys!` differs from the catalog `keys`; a vocabulary enum (`outcome_vocabulary!` or a `source` enum) differs from its catalog vocabulary | A1 |
| G8 | an entry since 0.12.0 that breaks `^[a-z][a-z0-9]*(_[a-z0-9]+)+$` (3-64 bytes); a prefix outside its owners; a reserved prefix | A1 |
| G9 | the header and fragments do not parse or merge (a spelling registered twice included); a released settled entry missing or changed (category, ladder class, key types, required set); a released retired entry active again. Read from the fragments and the compact release files only: no merged file is committed or compared (#420 S2, S3) | A1; ladder class #387 D1a; #420 S2 |
| G10 | a settled entry without text (`description`, and for a code `common_causes` and `handling`, empty or `TBD`); at a release `X.Y.Z`, no release file `released/X.Y.Z.json`, or, from v0.13.0 on, a pending entry (a final of the 0.12 series may carry pending entries; coordinator decision of 2026-10-19) | A8b; boundary v0.13.0 (2026-10-19) |

Loading the catalog, which every check does first, refuses a code entry without a
`ladder_class`, with a class that is not a token of the vocabulary `ladder_class`, or a
`stability_warning` code of another category than `warning` or `error` (#387 D1a).

The G2-G5 allow list (`ALLOW_LIST` in the guard) starts with every workspace member, one reason
each; each A2 part removes the members it converts. Standing entries, kept after B6: wire
decoders, MCP's JSON-RPC `error_response`, `tools/actinglab-architecture`,
`tools/outcome-guard`, and G5 for the configuration manifest's subsystem names (vocabulary
tokens; `runtime_fact_snapshot` equals a runtime-client location).

An entry names a whole member (`item: None`), or one item of one file: a function, which covers
the findings inside it and in its signature, or a struct or variant field, named `Type.field` or
`Enum::Variant.field`, which covers only that field's G2 finding. A method is named by its impl's
self type, `Type::method` (also inside `impl Trait for Type`), or by its trait, `Trait::method`,
so an entry allows exactly one function; a free or nested function keeps its bare name. A
channel that a converted member cannot type yet, because another owner's code fills it or a
later slice replaces it, stays a string behind such an item entry (a bridge); its reason names
the slice that removes it.

## Rust API (`actingcommand_contract::outcome`)

- One registry per owning crate, in `src/codes.rs`:
  `outcome_codes! { pub enum HostCode { InstanceDiscoveryUnavailable => "instance_discovery_unavailable": error / environment_fault } }`
  (category, then the ladder class after `/`)
  and `outcome_locations! { pub enum HostLocation { DiscoverInstances => "discover_instances" } }`.
  They generate `ALL`, `as_str`, `category` and `ladder_class` (a `LadderClass`) or `location`,
  and `From` into `Code` or `Location`, which nothing else can build. `outcome_vocabulary!`
  declares a vocabulary enum. Attributes on the enum and its variants, serde attributes included,
  pass through unchanged.
- `Outcome::new(code)` with one typed setter per key (`.path(p)`, `.stage(location)`,
  `.io_kind(kind)`, …), `.caused_by(link)`, `.link(relation, link)`, `Outcome::from_io_error`,
  and `.envelope(Detail::Codes | Detail::Full)`. A vocabulary key accepts only tokens of its
  own vocabulary (checked at compile time). `Outcome` keeps its fields behind one box, and its
  equality ignores whether it was received.
- `Outcome::from_io_error` is only for an error the OS or std raised. An `io::Error` the
  Runtime builds itself (`io::Error::new` or `other` with its own text) gets its own registered
  code.
- Debug and test builds check each emitted outcome against the embedded catalog: required keys,
  only declared keys, record fields, the category, and the fatal-link rule (not for a received
  outcome). They read the committed merged file the build embeds, so a fragment change is seen
  once its regenerated merged file is committed.
- `install_diagnostic_sink` (once per binary) and `diagnostic(&outcome, text)`; the contract
  crate never writes to stderr itself.
- `catalog()` returns the embedded merged file; `catalog_json()` parses it once and shares the
  parsed document with every reader.

## Failure scope (Workflow #419, ISO-M; Runtime 0.12.13)

Alice (#419): one emulator's fault must not stop the whole ActingCommand. Every failure the
Runtime host meets has exactly one **scope**, and only a `Global` failure may stop the Runtime.
The frozen model is `rd-1024/iso-design/model.md` (ISO-M) §5 and §6; ISO-1 builds the
mechanism, and the later ISO slices move their paths onto it.

| Scope | Means | At a boundary |
|---|---|---|
| `Global` | the Runtime as a whole cannot run normally | latches the Runtime (its fatal state; actingd then closes) |
| `Instance(i)` | the failure originates in, or concerns only, instance `i` | fences `i` (Workflow #403: one `runtime.failed` record of `instance_resources_fenced` caused by the failure when it is newly fenced, its policy availability withheld, then the ladder or a person); the work ends with the fence's non-fatal failure; every other instance goes on |
| `Operation` | one request or operation failed and its instance is sound | only that operation fails, with its own code |

**The scope rule** (`RuntimeHostError::scope_at`; R0.1-R0.4):
- The constructor carries the scope, and the wrappers (`into_fatal`, the request poisons,
  `with_caused_by`) keep it. `instance_fault(code, operation, runtime_code, instance)` builds a
  failure that leaves its instance's state unknown: it is severe for its own work (a fatal
  projection) and `Instance` of that instance. `instance_scoped()` tags an existing failure as
  one instance's; without an instance of its own it takes the boundary's.
- A failure tagged as one instance's stays `Global` when its code is on the Global list below,
  or when neither it nor its boundary names an instance.
- An **untagged** failure is `Global` when it is fatal and `Operation` otherwise. This is the
  fail-safe default: a path no slice has classified keeps halting as before.
- A failure joined with a `Global` failure (B7, `with_complete_failure`) is `Global`.
- `halts_runtime()` is `scope() == Global`. `is_fatal()` and the projection's `fatal` say only
  how severe the failure is for the work that met it (`failure_severe`, the ladder).

**One decision point** (`contain_failure(error, boundary_instance)` in
`crates/runtime-host/src/host/failure_scope.rs`; R0.5-R0.7): `Global` latches, `Instance` fences,
`Operation` fails the operation. A fence that fails (its ledger record) latches the fence's
failure, and an `Instance` failure that no registered instance owns latches as itself:
containment never fails silently.

**The five boundaries** that call it (R0.5):

| Boundary | Where | Since |
|---|---|---|
| the request door, also the direct run's boundary: a failure that would poison the Runtime is contained against the request's instance (its lease token's, else the registered instance of its alias) | `host/requests.rs`, `process_request_at_door` | ISO-1 |
| the scheduled run and the host package run | `host.rs`, `startup_package.rs` | ISO-2 |
| the instance worker | `host/host_claims.rs` | ISO-3 |
| each per-instance step of a shared loop | the sweep, pump, fence and monitor loops | ISO-4a |
| actingd's policy driver, per intent (through `halts_runtime()`) | `apps/actingd/src/main.rs` | ISO-6 |

**Receipts** (#411 A54, §5.4): a denied or failed receipt says `host_halted: true` only when the
Runtime latched (it holds a fatal state when the receipt is written); how severe the failure
was for its request does not make it say so. A failure that latched nothing (a read-only
observe whose capture failed, an IPC input or safe reset whose backend failed, a contained
`Instance` failure) answers `host_halted: false`, with its own `runtime_code`. On a host that
latched, a receipt is as before. A direct contained run's failed receipt keeps its failure's
severity as `host_halted`: the client reads it to know whether the run kept its OCR fields
report, which the host aborts for a fatal failure. Since ISO-2 a capture of the instance's dead
session (ISO-M row A01) and a failed geometry read (A03) fail the run non-fatally; a fatal
input failure (E02) keeps its severity and latches nothing.

**Device and kernel failures** (ISO-2; R1, R2): a failure of the execution kernel (the device,
its session, its capture and input backends) concerns only the session's instance:
`RuntimeHostError::execution` tags it `Instance`, of the instance it names, else of its
boundary's, and keeps the kernel's code and severity. A kernel code on the Global list below
(the session registry, an OS thread) stays `Global`. So:
- a scheduled run, a host package run (a startup package, a ladder rung), a handset's link
  step and an emulator control whose failure would stop the Runtime fence that instance
  instead, and fail with the fence's non-fatal failure; actingd's driver goes on with the
  other instances;
- a run's capture that met its instance's dead, closing or changed session fails only the run,
  as a capture failure (the failure's close retired the session, and the next capture opens a
  fresh one); a geometry read that failed with its resources confirmed fails the run as
  `contained_task_geometry_unavailable`; a frame commit or resolve the session refused fails
  only its run, observe or Lab operation;
- every fatal failure of the instance's own application command fences the instance (#411 A46,
  widened to every kernel code);
- a retained session's close before an emulator control, a pause or a handset's link loss, and
  a session close at a lease end, that failed with its resources confirmed holds nothing: its
  failure is recorded and the work goes on. An unconfirmed close fences the instance (#403).

**A lock of one instance alone** that guards no data (its admission guard, its queue-order lock;
R8) is taken through `lock_instance`: when a panic left it poisoned, the poison is cleared and
the work fails with `instance_lock_poisoned` (error, ladder class `none`), an `Instance` failure
of that instance. A poisoned lock that every instance shares stays `Global`
(`runtime_state_poisoned`).

**The closed Global list** (`GLOBAL_CODES`; R0.3), 142 codes in ten families, plus every catalog
code of category `fatal` (family `CatalogFatal`):

| Family | Codes | List |
|---|---|---|
| ledger: write, commit, integrity, format, dump, a failed append, the fact store out of step | 28 | `ledger_failure`, `agent_ledger_failure`, `monitor_ledger_failure`, `catalog_failure_fact_undurable`, `release_failure_fact_undurable`, `runtime_fact_store_desync`, `fact_store_sync_failed`, `fact_store_recovery_failed`, `runtime_fact_replay_failed`, `ledger_migration_required`, `ledger_write_failed`, `ledger_insert_incomplete`, `ledger_io`, `ledger_sqlite_failed`, `critical_event_not_durable`, `deferred_append_unconfirmed`, `writer_unavailable`, `writer_panicked`, `corrupt_ledger_record`, `corrupt_segment`, `ledger_index_mismatch`, `ledger_tag_index_mismatch`, `sequence_discontinuity`, `sequence_exhausted`, `ledger_format_unsupported`, `ledger_format_marker_mismatch`, `ledger_dump_failed`, `ledger_dump_unsupported` |
| configuration: actingd configuration, effective configuration, catalog, approvals | 36 | `invalid_runtime_host_config`, `invalid_runtime_config_manifest`, `agent_dispatcher_config_missing`, `policy_inputs_unconfigured`, `procedure_manifest_unconfigured`, `performance_config_invalid`, `performance_control_config_invalid`, `config_read_failed`, `config_decode_failed`, `effective_configuration_too_large`, `agent_dispatcher_config_invalid`, `invalid_capacity_thresholds`, `host_config_overrides_fact_invalid`, `host_config_overrides_persist_failed`, `host_config_value_type_invalid`, `catalog_compile_failed`, `catalog_manifest_invalid`, `catalog_pointer_invalid`, `catalog_generation_invalid`, `catalog_plan_generation_mismatch`, `catalog_migration_source_missing`, `catalog_state_create_failed`, `policy_catalog_mismatch`, `policy_catalog_approval_conflict`, `policy_catalog_unavailable`, `policy_catalog_compile_failed`, `policy_catalog_approval_invalid`, `approval_replay_conflict`, `approval_replay_payload_mismatch`, `approval_projection_query_failed`, `approval_projection_position_invalid`, `approval_projection_capacity_exceeded`, `approval_projection_order_invalid`, `approval_projection_origin_invalid`, `approval_projection_payload_mismatch`, `approval_projection_record_invalid` |
| the owner lock and the owner journal I/O | 15 | `owner_lock_failed`, `owner_file_open_failed`, `owner_file_missing`, `owner_write_failed`, `owner_sync_failed`, `owner_record_encode_failed`, `owner_revision_overflow`, `owner_epoch_issue_failed`, `owner_compaction_cleanup_failed`, `owner_compaction_rewrite_failed`, `owner_compaction_side_read_failed`, `owner_compaction_stage_failed`, `owner_unlock_failed`, `owner_seek_failed`, `owner_compaction_unrecoverable` |
| identifier issuance | 14 | `runtime_identifier_issue_failed`, `event_issuer_failed`, `event_id_issue_failed`, `action_id_issue_failed`, `policy_identifier_issue_failed`, `monitor_probe_id_issue_failed`, `run_id_issue_failed`, `task_id_issue_failed`, `agent_wake_issue_failed`, `readonly_capability_issue_failed`, `lab_projection_id_issue_failed`, `lab_frame_id_issue_failed`, `lab_recognition_id_issue_failed`, `observation_recognition_id_issue_failed` |
| an invalid or regressed clock | 9 | `system_clock_invalid`, `system_clock_overflow`, `monotonic_clock_regressed`, `monotonic_clock_overflow`, `policy_admission_clock_regressed`, `policy_admission_clock_overflow`, `policy_dispatch_clock_regressed`, `policy_execution_clock_regressed`, `clock_before_epoch` |
| a poisoned lock every instance shares; the kernel session registry | 7 | `runtime_state_poisoned`, `fatal_state_poisoned`, `capacity_projection_poisoned`, `deferred_replies_poisoned`, `execution_kernel_state_poisoned`, `execution_kernel_closed`, `execution_session_generation_exhausted` |
| the process: IPC listener, OS threads, state root, runtime information file | 21 | `runtime_bind_failed`, `runtime_listener_address_failed`, `runtime_listener_config_failed`, `runtime_socket_mode_failed`, `runtime_accept_failed`, `runtime_accept_spawn_failed`, `runtime_connection_spawn_failed`, `runtime_sweeper_spawn_failed`, `runtime_monitor_spawn_failed`, `runtime_performance_spawn_failed`, `runtime_pack_admission_spawn_failed`, `instance_worker_spawn_failed`, `execution_session_spawn_failed`, `runtime_thread_panicked`, `state_root_create_failed`, `state_root_inspect_failed`, `runtime_info_invalid`, `runtime_info_create_failed`, `runtime_info_write_failed`, `runtime_info_publish_failed`, `runtime_info_replace_failed` |
| the scheduler's cross-instance invariants | 6 | `invalid_scheduler_config`, `invalid_connection_id`, `identifier_issuance_failed`, `lease_expiry_overflow`, `lease_queue_sequence_overflow`, `lease_step_sequence_overflow` |
| the shared policy projection out of step with the ledger | 2 | `policy_admission_event_invalid`, `policy_state_ledger_incomplete` |
| the installer's transitions | 4 | `runtime_component_not_prepared`, `runtime_component_already_prepared`, `install_abort_state_invalid`, `install_release_state_invalid` |

## Open items

- **UI keys the UI names next.** By model section 3.4 these draft keys are not keys and wait for
  the UI's next revision: `reason`, `subject`, `context`, the generic `source`, `key`, `value`,
  `party`, `layout`, `expected`, `actual`, `op` (split into `io_op`, `setup_*` stage locations
  and `invocation`). The records `changes`, `edges`, `items`, `rows` and `steps` are registered
  without those fields.
