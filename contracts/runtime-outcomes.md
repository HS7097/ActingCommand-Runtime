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
| `causes` | optional; links `{code, category, relation, values, causes?}` (an envelope without `schema_version`) |
| `causes_total` | top only, present when links were cut: the full link count |
| `detail` | `codes` when data values were withheld; absent means full |

- **Readers** ignore unknown members, and keep an unknown code or relation readable as it came
  (`CodeStr`, `Relation`). A relayed envelope nests unchanged as a link.
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
  only the values whose type is `code`, `location`, `vocab:*` or a list of them.
- **Foreign text** goes only into `raw_text` on a `foreign_*` or `panic_caught` link, verbatim.

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
| `contracts/outcome-codes.json` | the merged file every reader uses, generated; never edited by hand |
| `contracts/outcome-codes/released/<version>.json` | per-release snapshot, written by the release PR |

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
  a category.
- **Code entry**: `category`, `owner`, `layer`, `uncertain`, `values` (`key: required|optional`),
  `review` (`pending`|`settled`), `status` (`active`|`retired`), `since`, `description`,
  `common_causes`, `handling` (`TBD` while pending), and `retired_in`, `replaced_by` once retired.
  A location entry has `owner`, `layer`, `review`, `status`, `since`, `description`.
- **Release snapshot**: `{schema_version: "actingcommand.outcome-codes.release.v1", release,
  codes: {name: {category, review, status, values: {key: {type, required}}}},
  locations: {name: {review, status}}}`.

### Regenerating the merged file

Never run the merge locally. Push the fragment change; CI's `Outcome catalog merge` step runs
`outcome-guard merge` whenever the Test step ran, pass or fail, and uploads
`contracts/outcome-codes.json` as the artifact `outcome-codes-<sha>` of the pushed head.
Download it (`gh run download <run-id> -n outcome-codes-<sha>`), commit it unchanged and push
again; G9 then passes.

## Drift guard `tools/outcome-guard`

A source scanner that links no workspace crate; its tests run in `cargo test --workspace`.
Test modules and files reached only through them are skipped.

| Check | Fails on | On since |
|---|---|---|
| G1 | a registry entry (`outcome_codes!`, `outcome_locations!`) missing from the fragments, or of another owner or category, or retired; an active fragment entry with no registry entry; a spelling registered twice | A1 |
| G2 | a string-typed field or parameter named `code`, `*_code`, `reason`, `*_reason`, `failure`, `operation`, `*_operation`, `stage`, `*_stage`, `boundary`; a `Result` whose error is a string; a getter `code`, `*_code`, `reason`, `*_reason`, `key`, `operation`, `stage` returning a string | A2a-A2e, member by member |
| G3 | a `json!` key `code`, `*_code`, `reason`, `*_reason`, `category`, `operation`, `*_operation`, `stage`, `*_stage`, `boundary` | A2a-A2e |
| G4 | `From<String>`/`FromStr`/a string constructor for `Code`, `Location`, `CodeStr`, or a registry constructor, outside the outcome module | A2a-A2e |
| G5 | a non-test string literal equal to a registered code or location outside its registry | A2a-A2e |
| G6 | persisted prose fields in contract types | A3 |
| G7 | `outcome_keys!` differs from the catalog `keys`; a vocabulary enum (`outcome_vocabulary!` or a `source` enum) differs from its catalog vocabulary | A1 |
| G8 | an entry since 0.12.0 that breaks `^[a-z][a-z0-9]*(_[a-z0-9]+)+$` (3-64 bytes); a prefix outside its owners; a reserved prefix | A1 |
| G9 | the merged file is not the merge of the fragments; a released settled entry missing or changed (category, key types, required set); a released retired entry active again | A1 |
| G10 | a settled entry without text; a pending entry at the v0.12.1 release; a release tag without a snapshot | A8 |

The G2-G5 allow list (`ALLOW_LIST` in the guard) starts with every workspace member, one reason
each; each A2 part removes the members it converts. Standing entries, kept after B6: wire
decoders, MCP's JSON-RPC `error_response`, `tools/actinglab-architecture` and
`tools/outcome-guard`.

## Rust API (`actingcommand_contract::outcome`)

- One registry per owning crate, in `src/codes.rs`:
  `outcome_codes! { pub enum HostCode { InstanceDiscoveryUnavailable => "instance_discovery_unavailable": error } }`
  and `outcome_locations! { pub enum HostLocation { DiscoverInstances => "discover_instances" } }`.
  They generate `ALL`, `as_str`, `category`/`location` and `From` into `Code` or `Location`,
  which nothing else can build. `outcome_vocabulary!` declares a vocabulary enum.
- `Outcome::new(code)` with one typed setter per key (`.path(p)`, `.stage(location)`,
  `.io_kind(kind)`, …), `.caused_by(link)`, `.link(relation, link)`, `Outcome::from_io_error`,
  and `.envelope(Detail::Codes | Detail::Full)`. A vocabulary key accepts only tokens of its
  own vocabulary (checked at compile time).
- Debug and test builds check each emitted outcome against the embedded catalog: required keys,
  only declared keys, record fields, the category, and the fatal-link rule.
- `install_diagnostic_sink` (once per binary) and `diagnostic(&outcome, text)`; the contract
  crate never writes to stderr itself.
- `catalog()` returns the embedded merged file.

## Open items

- **Simplified Chinese language token.** The model's tag for Simplified Chinese carries a region
  subtag that the C2 genericity guard (`tools/actinglab-architecture`) rejects as a standalone
  word in every file under `contracts/`, so the `language` vocabulary registers `en` only; the
  coordinator decides between a script tag (`zh-Hans`) and a guard exemption.
- **UI keys the UI names next.** By model section 3.4 these draft keys are not keys and wait for
  the UI's next revision: `reason`, `subject`, `context`, the generic `source`, `key`, `value`,
  `party`, `layout`, `expected`, `actual`, `op` (split into `io_op`, `setup_*` stage locations
  and `invocation`). The records `changes`, `edges`, `items`, `rows` and `steps` are registered
  without those fields.
