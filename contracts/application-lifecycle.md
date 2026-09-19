# Application lifecycle: task effect, foreground gate, startup package

Runtime slice #316-B3. Three things, all free of game knowledge:

1. a task package can (re)start or stop *the application the instance is assigned* as one
   operation effect, without naming a package or a coordinate;
2. no pointer input reaches a physical instance unless the package Android reports in the
   foreground is that assigned application (`application.foreground`, an instance program
   fact);
3. an instance may declare a startup package that the host runs by itself, as an ordinary
   contained task, after a successful emulator `start` / `restart`.

The pointer is always the instance configuration's `application_id`: one instance runs one
application, assigned by a person or an agent, and packages never carry a package name. ADB is
the baseline for all three (`am force-stop`, `monkey ... LAUNCHER`, `dumpsys activity
activities`): a Nemu paired session, when one is open, neither starts applications nor
answers the foreground question, and `device.connected` stays the only health anchor.

## The `application` effect (task.json 0.6)

An operation carries exactly one effect: the existing `click` object, or

```json
{
  "id": "relaunch",
  "from": "any",
  "to": "<game>/home",
  "application": { "action": "restart" }
}
```

- `action` is `launch`, `restart` or `stop` (`ApplicationLifecycleAction`, snake_case). No
  other key is accepted inside `application`; `click` and `application` on one operation, or
  neither, is a declaration error.
- Execution resolves the instance's `application_id` and drives the existing
  `ApplicationLifecycle` path (`control_application`: adb `force-stop` for `stop`, `monkey`
  launch for `launch`, both for `restart`) under the run's lease, with the task and run ids on
  every event. The step records no `task.effect_intent` (there is no input to sample); its
  chain is `task.step_started` -> `application.intent` -> `application.completed` /
  `application.failed` -> `task.effect_completed`, then the ordinary post-step observation
  and `task.step_finished`. No guard is evaluated and no foreground gate runs for this step:
  the effect is what brings the assigned application to the foreground.
- Success is the target page, exactly as for a click: the recognition pack must report the
  operation's `to` page before the step timeout (`step_timeout`, the same default and bound as
  every other step). An application that does not reach the page fails the task with the
  ordinary step-timeout outcome; the effect itself failing (adb error) fails the step with the
  `application_backend_operation_failed` chain the `ApplicationLifecycle` request already
  uses.
- An instance without an assigned application cannot run the effect:
  `application_effect_requires_assigned_application` (`invalid_request`, denied) before any
  device write. Registered device instances always carry one (`application_identity_missing`
  is a configuration error), so this refuses fixture-simulated runs and providers without an
  application surface.
- Lab (`actinglab package build` / `validate`), `pack-containment` and the execution kernel
  accept the effect in the same declaration slot; the `resource_declaration_invalid` /
  `unknown_field` path treats `application` as a known operation field.

The minimal "application start" package is therefore one entry (`any`), one step
(`application.restart`) and one target page (the home page the recognition pack declares).
It is also the second rung of the recovery ladder (#316-B4, out of this slice): return-home
package -> this package -> `emulator restart` -> this package again.

## The foreground gate

Before any pointer input (`tap`, `long_tap`, `swipe`, `single_touch_drag_with_vertical_brake_v1`)
reaches a physical instance, the host reads the foreground package through ADB and compares it
with the instance's `application_id`. The check sits in the single input choke point
(`HostShared::input`, after the input-frame resolution and before the input is prepared), so
one check covers the adb and the Nemu touch backends alike, for client `Input` requests and
for every step of a contained task. `key`, `text` and `reset` inputs and fixture instances
are not gated.

- Query: `adb -s <serial> shell dumpsys activity activities`, package of the first
  `ActivityRecord{... u<user> <package>/<activity> ...}` behind `topResumedActivity=`, then
  `mResumedActivity:` / `ResumedActivity:`. The provider goes through the same bound-endpoint
  guard and `ensure_device` as `control_application` and opens no session.
- Match: the input proceeds, and the instance program fact `application.foreground` =
  `string(<package>)` is recorded ledger first (`runtime.fact_recorded`) whenever the observed
  value differs from the stored one (`contracts/runtime-fact-store.md`, "Producers").
- Mismatch: `application_not_foreground` (`invalid_request`, receipt state `denied`):
  `command.rejected` for the input's own action (diagnostic `runtime.diagnostic`, effect
  `not_performed`, the receipt terminal) plus one `runtime.failed` (stage `operation_cleanup`)
  whose native detail names the alias, the observed package and the assigned one. The fact
  is still recorded, so the ledger shows which application took the foreground. Inside a
  contained task the refusal travels the ordinary refused-input path of the run.
- No resumed activity reported (a transition, an empty display):
  `application_foreground_unknown`, same shape, nothing recorded.
- ADB failure (`ensure_device` or the command itself): the ADB baseline is lost. The host
  invalidates `device.connected` and `application.foreground` with reason `adb_unreachable`
  (`runtime.fact_invalidated`, absent keys ignored) and refuses with
  `application_foreground_unknown`, even while a Nemu session still delivers frames. The next
  successful gate re-records `application.foreground`; `device.connected` is written again by
  the next emulator control action.

Rule (ADB baseline, Alice 09-19): whatever other connection is open, the ADB endpoint must
stay bound and answering; Nemu never replaces ADB as instance identity (the port, #322) or as
the health anchor. The per-input query is that health probe while inputs flow; no separate
periodic ADB probe is added by this slice.

## The startup package hook

Instance configuration (`actingd`, `contracts/actingd-check-config.md`):

```json
{
  "alias": "mumu.c",
  "instance_id": "...",
  "application_id": "<assigned package>",
  "instance_index": 1,
  "startup_package": { "package": "packages/neutral-startup.zip", "expected_sha256": "<64 hex>" }
}
```

Same semantics as `actingctl task-run --package <locator> --expected-sha256 <hex>`: a ZIP
locator (relative paths resolve against the configuration file's directory; the assembled
path must be absolute) and the bare lowercase hex digest; the response deadline is the
contract maximum. Nothing is opened or hashed at assembly or startup: `check-config` echoes
the declaration per instance and counts them as `instances_startup_package_count`; host
startup binds each alias to a registered physical instance
(`startup_package_instance_unknown`, `startup_package_requires_physical_instance` are fatal;
a fixture instance is refused at assembly with `instance_config_invalid`).

Behaviour (`contracts/emulator-control.md`, "Startup package hook"): only a successful
`start` / `restart` schedules the package; the control request appends one
`runtime.lifecycle_observed` (phase `startup_package_scheduled`, the locator in the audit
path, a fresh causation id) and returns `startup_package: scheduled`; the host's own
scheduling thread then runs the package as an ordinary contained task under that causation
id, with self-minted request / correlation / holder ids, origin `(Agent, Adapter)`, a
synthesized connection, hash admission, its own lease and the full `task.*` chain. A
configured package is always invoked; no configuration, `stop`, a failed action, or an
instance found already running at daemon startup pull nothing.

Typed codes: `startup_package_missing` (the locator does not open),
`startup_package_admission_failed` (every other admission refusal; the underlying
`contained_task_package_*` code is the related failure), both `package_invalid` and recorded
before any lease as `runtime.failed` (category `startup_package`, stage `operation_cleanup`)
under the instance and the causation id. Failures after admission are the ordinary contained
task failures (`task.failed`, lease release, `runtime.failed` on the cleanup path).

## Expected ledger sequence (one `emulator restart` with a startup package)

```
client.cli_command / command.received          emulator.instance.restart
runtime.lifecycle_observed ...                  (resource close of a retained session, if any)
command.validated (performed)                  receipt terminal
runtime.instance_bound                          adb_host + adb_port
runtime.fact_recorded                           device.connected = true
runtime.lifecycle_observed                      startup_package_scheduled, causation C
--- scheduling thread, every event below carries causation C ---
command.received / command.validated            runtime.task_run (scheduler / runtime)
lease.requested / lease.granted
task.requested / task.started ... task.effect_intent
application.intent / application.completed      application.restart
task.effect_completed ... task.step_finished
runtime.fact_recorded                           application.foreground = <assigned package>
                                                (first gated pointer input, if the package has one)
task.completed | task.failed
lease.released
```

## Typed codes

`application_not_foreground`, `application_foreground_unknown` (`invalid_request`, denied);
`application_effect_requires_assigned_application` (`invalid_request`, denied);
`startup_package_missing`, `startup_package_admission_failed` (`package_invalid`, denied);
startup-time fatal: `startup_package_instance_unknown`,
`startup_package_requires_physical_instance`, `invalid_startup_package`; `actingd` assembly:
`startup_package_path_invalid`, `startup_package_digest_invalid`, `startup_package_invalid`.
Fact invalidation reason: `adb_unreachable`.
