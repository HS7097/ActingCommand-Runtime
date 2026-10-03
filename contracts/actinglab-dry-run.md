# ActingLab global `--dry-run`

`--dry-run` is a global ActingLab option. It is read at any argv position and folded into the
global options before dispatch; a command never receives the token itself. This contract states
what every command does when the flag is present (Workflow #341). The capability entries of
`capabilities`, help and `list commands` carry the same declaration (`actinglab-capabilities.md`).

## Rule

Every actinglab command declares its behaviour under the global `--dry-run` flag as exactly one of
`preview`, `refused` or `no_effect`; a command whose forms differ is `mixed` and declares each
form. `--dry-run` is read from any argv position. A change that adds or alters a side effect
declares the command's mode in the same commit; a command without a declaration is reported as
`undeclared` and is a defect. Agents read `commands[].dry_run_mode` before probing.

- `preview`: with `--dry-run` the command sends no input, controls no application, makes no
  state-changing Runtime request and writes no file, configuration or ledger entry beyond the
  writes registered in "Bootstrap and transient writes". Its data carries the marker declared in
  `dry_run_marker`; previews added by #341 carry `dry_run: true` with a `planned` or `validated`
  status, input previews also `executed: false`, write previews also `persisted: false`. A preview
  is not an admission: what it did not check is listed in `not_checked`.
- `refused`: with `--dry-run` the command fails before any Runtime connection and before any
  write, with `dry_run_unsupported` (exit 2, `details.dry_run: true`, `details.executed: false`)
  unless the table names an older code. Argument checks listed for the form run first; the others
  run only on a real invocation. A refusal is not an argument error: do not drop `--dry-run` and
  retry unless the user asked for the action. `refused` says only that the flag is rejected; it
  does not imply a side effect.
- `no_effect`: the command has no side effect beyond the registered bootstrap and transient writes
  (it reads or prints, is retired or reserved, or always refuses); `--dry-run` does not change its
  behaviour.

## Capability fields

Each capability entry carries `dry_run_mode` (`preview`, `refused`, `no_effect`, `mixed` or, for
a defect, `undeclared`) and one conditional field:

| Mode | Field | Value |
|---|---|---|
| `preview` | `dry_run_marker` | `dry_run`: `data.dry_run == true`; `executed_false`: `data.executed == false`; `capture_dry_run`: `data.capture.dry_run == true` |
| `refused` | `dry_run_refusal_code` | the refusal code of the table below |
| `mixed` | `dry_run_forms` | one object per form: `form`, its `dry_run_mode` and that mode's field |

An entry the declaration table does not list is `no_effect` when its status is `retired`,
`reserved` or `unavailable`, and `undeclared` otherwise.

## Refusal codes

| Code | Exit | Commands | Marker |
|---|---|---|---|
| `dry_run_unsupported` | 2 | the #341 refusals below | `details.dry_run: true`, `details.executed: false`, `blocked_by: []` |
| `explicit_offline_entry_required` | 3 | `lab run` | none; identified by this table and `dry_run_refusal_code` |
| `offline_device_scope_forbidden` | 2 | `package dry-run` | none |
| `validation_failed` | 2 | `scheduling compile`, `scheduling timeline` | none |
| `record_flag_unsupported` | 2 | `--record` combined with `--dry-run` (#336) | none |

The `dry_run_unsupported` envelope:

```json
{"ok":false,"command":"session app","error":{"code":"dry_run_unsupported",
 "message":"session app restart has no dry run; this is not an argument error and nothing was sent or written. Run `session status --diagnostics` for a read-only view; drop --dry-run only if the user asked for this action.",
 "blocked_by":[],
 "details":{"form":"session app restart","dry_run":true,"executed":false,
            "read_only_alternative":"session status --diagnostics",
            "action":"restart","instance":"<resolved instance>"}}}
```

`read_only_alternative` is `null` when the form has none. The human-readable line is
`dry_run_unsupported: <message> (dry run: nothing executed or stored)`.

## Preview shapes added by #341

Direct input (`tap`, `swipe`, `long-tap`, `key`, `text`), exit 0. The instance is resolved by the
same selector as the real command; an `--instance` alias is echoed and only checked against the
Runtime on a real invocation, so `planned` is not an admission:

```json
{"status":"planned","mode":"dry_run","dry_run":true,"executed":false,
 "backend":"runtime_proxy","control_mode":"direct_trusted_manual",
 "instance":"<id>","action":{"type":"key","key":"4"},
 "checked":["arguments","instance_selection"],
 "not_checked":["runtime_instance","lease","foreground_application"],
 "input_outcome":{"input_stage":"not_submitted","input_receipt":null}}
```

`config set`, exit 0, `config.json` unchanged: the key and value pass the same validation as the
real command.

```json
{"config_path":"<path>","key":"<key>","value":"<value>","status":"validated","dry_run":true,
 "persisted":false,"next":"re-run without --dry-run to write config.json"}
```

`detect` with a detector that has no steps, exit 0: the detection is evaluated on `--scene` and
returned with `status: "validated"`, `dry_run: true`, `steps_executed: false`, `persisted: false`,
`next` and the `result`, without `result_path`. No result file is written, so `env resolve`,
`env status` and `{env:…}` markers keep reading the stored result. A detector with steps keeps its
existing `planned` preview.

The human-readable line of any response whose data has `dry_run: true` ends with
` (dry run: nothing executed or stored)`; JSON output is unchanged.

## Commands

Commands in the current inventory and capability declarations. "Checks first" lists the local checks a
refused form runs before it refuses; every other check runs only on a real invocation.

| Command | Mode | Code or marker | Checks first / notes |
|---|---|---|---|
| `help` | no_effect | | |
| `version` | no_effect | | |
| `paths` | no_effect | | |
| `capabilities` | no_effect | | |
| `doctor` | no_effect | | |
| `status` | no_effect | | |
| `devices` | no_effect | | retired |
| `schema` | no_effect | | |
| `list` | no_effect | | |
| `touch-probe` | refused | `dry_run_unsupported` | positional arguments, `--touch-backend`; alternative `session status --diagnostics` |
| `tap` | preview | `dry_run` | input preview |
| `swipe` | preview | `dry_run` | input preview |
| `long-tap` | preview | `dry_run` | input preview |
| `key` | preview | `dry_run` | input preview; named keys are normalized (`back` is `4`) |
| `text` | preview | `dry_run` | input preview |
| `capture` | mixed | | `--out <path>` (any form other than diagnose): refused, `dry_run_unsupported`, flag parsing only, alternative `capture diagnose`; `diagnose` / `--diagnose`: no_effect |
| `detect` | preview | `dry_run` | `validated` without steps, `planned` with steps |
| `detect-page` | no_effect | | |
| `recognize` | no_effect | | |
| `recognize-artifact` | no_effect | | |
| `observe` | mixed | | `--with-frame <path>` (offline and `--capture`): refused, `dry_run_unsupported`, flag parsing only, alternative `observe` without `--with-frame`; a bare `--with-frame` and every other form: no_effect |
| `do` | preview | `executed_false` | `--capture --dry-run` writes the registered debug audit events |
| `ensure` | preview | `executed_false` | as `do` |
| `wait` | no_effect | | |
| `current-page` | no_effect | | |
| `is-visible` | no_effect | | |
| `locate` | no_effect | | |
| `tap-target` | preview | `executed_false` | |
| `navigate` | preview | `executed_false` | |
| `monitor` | no_effect | | retired |
| `stream` | mixed | | `stream check`: no_effect; every other form, `--input-relay` included: preview, `capture_dry_run` |
| `record` | mixed | | `start`, `step`, `amend`: refused, `dry_run_unsupported`, action name only, alternative `record status` (start) or `record candidates` (step, amend); `status`, `candidates`: no_effect; `stop`, `mark`, `build-task`, `promote`: preview, `dry_run` |
| `explain` | no_effect | | |
| `config get` | no_effect | | |
| `config set` | preview | `dry_run` | `validated`, `persisted: false` |
| `env resolve` | no_effect | | |
| `env status` | no_effect | | |
| `lab run` | refused | `explicit_offline_entry_required` (exit 3) | flag parsing, legacy routing flags; alternative `package dry-run` |
| `lab validate` | no_effect | | |
| `lab signatures` | refused | `dry_run_unsupported` | `register`, `match`, `retire`; no check first; alternative the read-only `actingledger --state-root <historical-root> signatures --through <sequence> --catalog-state-root <registered-ledger-root> --catalog-through <sequence>` (`diagnostic-signatures.md`) |
| `lab debug-package` | refused | `dry_run_unsupported` | no check first; alternative `lab validate` |
| `lab watch` | no_effect | | |
| `lab unpin` | refused | `dry_run_unsupported` | no check first; alternative `lab watch` |
| `lab export-evidence` | refused | `dry_run_unsupported` | no check first; no alternative |
| `lab replay-evidence` | no_effect | | |
| `lab start` | no_effect | | reserved, always refuses |
| `lab status` | no_effect | | |
| `lab lease` | no_effect | | retired |
| `lab preempt` | no_effect | | retired |
| `lab release` | no_effect | | retired |
| `lab receipt` | no_effect | | |
| `lab evidence` | no_effect | | |
| `lab arbitrator` | no_effect | | retired |
| `lab vendor-stdio-selftest` | no_effect | | registered transient log |
| `package validate` | no_effect | | |
| `package preflight` | no_effect | | shared contained-task preparation |
| `package dry-run` | refused | `offline_device_scope_forbidden` | positional arguments, `--version` |
| `package inspect` | no_effect | | |
| `package run` | refused | `dry_run_unsupported` | flag parsing, legacy routing flags, `--zip`, package validation, instance selector; alternative `package dry-run` |
| `package build-task` | preview | `dry_run` | registered transient ZIP and `--from-remote` clone |
| `package build-pack` | preview | `dry_run` | as `package build-task` |
| `package digest` | no_effect | | |
| `package bundle` | refused | `dry_run_unsupported` | flag parsing only; alternative `package digest` |
| `operation validate` | no_effect | | |
| `operation inspect` | no_effect | | |
| `operation explain` | no_effect | | |
| `operation dry-run` | no_effect | | reserved |
| `operation run` | no_effect | | always refuses (`lab_lease_required`) |
| `control inspect` | no_effect | | |
| `control verify` | no_effect | | |
| `control probe-click` | no_effect | | always refuses |
| `control export` | no_effect | | reserved |
| `control diff` | no_effect | | |
| `scheduler status` | no_effect | | reserved |
| `scheduler pause` | no_effect | | reserved |
| `scheduler resume` | no_effect | | reserved |
| `scheduler start` | no_effect | | reserved |
| `scheduler stop` | no_effect | | reserved |
| `scheduling compile` | refused | `validation_failed` | argument size bound; the command is read-only |
| `scheduling timeline` | refused | `validation_failed` | as `scheduling compile` |
| `resource restore` | refused | `dry_run_unsupported` | flag parsing, `--repo`; no alternative |
| `resource validate` | no_effect | | |
| `resource convert` | preview | `dry_run` | |
| `resource compile-maa` | no_effect | | |
| `resource` import route of the external-tool data (reserved) | no_effect | | reserved; one of the two inventory entries whose names the C2 genericity guard keeps out of `contracts/` |
| `resource` drift route of the external-tool data (reserved) | no_effect | | reserved; the other such entry |
| `resource check-release` | no_effect | | |
| `runtime reset` | refused | `dry_run_unsupported` | positional arguments, Runtime flags, `--state-root`, instance; no alternative; no capability entry |
| `runtime observe` | no_effect | | |
| `ledger show` | no_effect | | retired |
| `ledger events` | no_effect | | retired |
| `ledger receipts` | no_effect | | retired |
| `ledger diagnose` | no_effect | | retired |
| `ledger evidence` | no_effect | | retired |
| `session status` | no_effect | | |
| `session bootstrap` | no_effect | | retired |
| `session throat-policy` | no_effect | | |
| `session capture-policy` | no_effect | | |
| `session record-policy` | no_effect | | |
| `session self-heal-policy` | no_effect | | |
| `session self-heal-plan` | no_effect | | retired |
| `session phase-c-plan` | no_effect | | retired |
| `session readiness` | no_effect | | retired |
| `session connect-plan` | no_effect | | retired |
| `session stream-plan` | no_effect | | retired |
| `session queue` | no_effect | | retired |
| `session command-check` | no_effect | | retired |
| `session submit-plan` | no_effect | | retired |
| `session validation-plan` | no_effect | | retired |
| `session start` | no_effect | | retired |
| `session stop` | no_effect | | retired |
| `session cleanup` | no_effect | | retired |
| `session daemon` | no_effect | | retired |
| `session request` | no_effect | | retired (always exit 6) |
| `session contract` | no_effect | | |
| `session api` | no_effect | | |
| `session transport` | no_effect | | |
| `session journal` | no_effect | | retired |
| `session events` | no_effect | | retired |
| `session response` | no_effect | | retired |
| `session request-state` | no_effect | | retired |
| `session monitor-policy` | mixed | | `status`: no_effect; `set`, `clear`: refused, `dry_run_unsupported`, action name, flag parsing and legacy flags first (the `set` value checks run only on a real invocation), alternative `session monitor-policy status` |
| `session instance` | mixed | | `list`, `registry` and the retired verbs: no_effect; `app <launch\|stop\|force-stop\|restart>`: refused as `session app` |
| `session app` | refused | `dry_run_unsupported` | the verb, legacy routing flags, `--package`, configuration read and instance resolution, verb validity; alternative `session status --diagnostics`; `details` carry `action` and `instance` |
| `session capture` | mixed | | as `capture` |
| `session stream` | preview | `capture_dry_run` | the `check` form (its own entry `session stream check`): no_effect |
| `session recover` | preview | `executed_false` | the `--stale-capture` form (its own entry): no_effect |
| `session lease` | no_effect | | retired |
| `session record` | mixed | | as `record` |
| `run list` | no_effect | | |
| `run show` | no_effect | | |
| `run open` | no_effect | | |
| `run summary` | no_effect | | |
| `run export` | refused | `dry_run_unsupported` | the run id, configuration read, `--out`; no alternative; `details.run_id`; no capability entry |
| `report export` | refused | `dry_run_unsupported` | `--last-error`, `--out`; no alternative; no capability entry |

`record stop` and `session record stop` run the L4 generator and self-checks in memory under
`--dry-run`, return `dry_run: true` and status `validated`, and preserve the recording and output
artifacts. A missing session returns `not_started` with the same dry-run marker. `record mark`
and its session alias likewise preview the mark changes with `dry_run: true`.

`resource catalog` is `no_effect`: it reads the authoring catalog and its validation results.

With #336, a `--record` invocation that also carries `--dry-run` is refused first by the
`--record` gate with `record_flag_unsupported` (exit 2).

## Bootstrap and transient writes

These writes exist today, are not changed and are not side effects for this contract; they apply
to `preview` and `no_effect` alike:

- the salt `<state>/env-detection/.local_salt`, generated only when it is missing, by `detect`,
  `env resolve`, `env status` and by any resolution of `{env:…}` markers (for example
  `session recover`, `package build-task` and `package build-pack`); a request without a marker
  does not reach it;
- the session state directory that every `record` / `session record` action, `status`,
  `candidates`, `build-task` and `promote` included, creates before it runs;
- the existing per-instance recording lock and its holder metadata, used by `record stop`
  and `record mark` previews to read one consistent recording; the operating-system lock
  is released when the command exits;
- the temporary ZIP of `package build-task` / `package build-pack` (written, then deleted) and the
  `--from-remote` clone under the system temporary directory;
- the `lab vendor-stdio-selftest` log under the system temporary directory, deleted afterwards;
- the Runtime debug audit events of `do` / `ensure` with `--capture --dry-run`.

## Read audit boundary

A read request leaves the read-only audit events the Runtime itself writes, such as the
`Observed` event of a status read. These are not side effects for this contract. The previews and
refusals added by #341 do not send even a read request: they open no Runtime connection.

## Agent guidance

- Read `commands[].dry_run_mode` before probing a command with `--dry-run`.
- `refused` only means the flag is rejected; it does not mean the command has a side effect
  (`scheduling compile`, `scheduling timeline` and `package dry-run` only read).
- A dry-run refusal is not an argument error. Run the read-only alternative it names; never drop
  `--dry-run` and retry on your own.
- `planned` and `validated` are not an admission; `not_checked` names what was not checked.
