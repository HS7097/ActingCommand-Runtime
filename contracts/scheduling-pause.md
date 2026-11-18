# Scheduling pause and resume

Workflow #191 slices ps1 and ps2 (launcher design v3, Workflow #297 §3.1, §3.2 stages
(a)(b)(c), the resume paragraph and §3.3). `RuntimeOperation::PauseScheduling` stops the
Runtime's own policy dispatch, globally or for one instance, and an instance pause waits until
the instance's device is idle (its session stays open, Workflow #191 H);
`RuntimeOperation::ResumeScheduling` lifts the pause again
and an instance resume reconnects the device at once through the connection preparation phase
(Workflow #317 sc3), answering with its self-check.

## Operations and origin gate

```text
PauseScheduling  { scope, pause_reason, drain_timeout_ms, pause_mode? }
ResumeScheduling { scope, expected? }
pause_mode = "stand" | "quiet_or_lift"          (absent: "stand"; #480 PG-2c, runtime-client 12)
scope = { "kind": "global" } | { "kind": "instance", "instance_alias": "<alias>" }
expected = { "owner_epoch": "<epoch>", "revision": <u64 >= 1> }
```

- `RuntimeRequest::validate` admits both from every envelope of the client set since Workflow
  #434 LAB-6S1b (0.12.58; [lab-two-entries.md](lab-two-entries.md) §2.4), the Lab's
  `(Lab, Lab)` included. Before it, it admitted them only when `(actor, source)` was
  `(User, Ui)` or `(Cli, Cli)`, every other origin being `invalid_scheduling_pause_origin`
  (retired). The scheduler and the Runtime's own work send no request. No request needs a
  governance identity card.
- `pause_reason` is a name (outcome key `pause_reason`, type `name`; Workflow #378 A2a): 1..=64
  bytes of `[a-z0-9_.-]` (`invalid_scheduling_pause_reason`).
- `drain_timeout_ms` is bounded to `1_000..=600_000`
  (`invalid_scheduling_pause_drain_timeout`). It is validated for both scopes and used only
  by an instance pause.
- An instance scope names a registered physical instance (`require_physical_instance_alias`):
  a fixture instance is `fixture_execution_scope_forbidden`, an unknown alias
  `instance_unknown`; this holds for pause and resume alike.
- `pause_mode` (#480 PG-2c, since 0.12.73; `SchedulingPauseMode`, a closed pair): what an instance
  pause does when something is still in the way at the end of its caller's time ("Two modes"
  below). Absent, it is `stand`, so a client of `runtime-client` 11 or lower sends a person's
  pause; every client of 12 writes it. A word outside the pair fails the frame's decode, and a
  Runtime of revision 11 or lower cannot decode a request that names the member (the operation
  denies unknown members): it ends the connection without a receipt. The global pause has no
  stages, so the mode changes nothing for it.

## The dispatch gate

`admit_policy_dispatch` checks the pauses right after `replay_admission` (a replayed decision
is still suppressed as before) and before the instance's queue, the performance and the
capacity gates, as control gates in the one order of
[player-hold.md](player-hold.md), "The one order of the control gates" (R-7a §2.6; Workflow
#429 R-7a2, register §1.234, since 0.12.35): the player hold (R-7b), the pause of the intent's
instance (`dispatch_paused_instance`), the global pause (`dispatch_paused_global`), then the
instance's #403 fence (`dispatch_resource_fenced`). The first in effect is the refusal's code:
with both pauses in effect it is `dispatch_paused_instance`. Before 0.12.35 the global pause
came first. A hit only sets the admission's gate error; the instance's queue, the performance
and capacity gates are not consulted. The trace is the existing one of a refused admission:
`policy.dispatch_intent`, then `policy.dispatch_rejected` (`Denied`, `NotPerformed`) with the
code in its `rejection`.

Every policy evaluation reads the same pauses, and the fences, as it starts: a candidate on a
paused instance is `CandidateEligibility::Deferred` with the effective code by the same order
(`dispatch_paused_instance` when the instance's own pause holds it, whether or not the global
pause does too; `dispatch_paused_global` under the global pause alone) and no next wake time,
so a paused cycle emits no dispatch intent for it. Adding or lifting the global pause over an
instance pause changes neither the reason nor ScheduleExplanation's `since_unix_ms`.

The gate covers policy dispatch and, in the instance's one queue (Workflow #369 Q-3 (4)), its
routine claims. Client and operator requests (`task-run`, leases, observation, emulator control,
resume and self-check) are not gated. Since Workflow #369 S6b (H-4, M6) the startup package's
claim is a gated routine claim: an emulator `start` / `restart` under a pause of its instance or
the global pause (a restored pause included) still drives the emulator, but its startup claim
waits queued and the control's key is released, so no startup run happens until the resume;
the resume's reconnect claim is granted first, and its release hands the key to the startup
claim. A `stop`, a `start` / `restart` whose preparation fails, or (Workflow #369 S6c) a
control that fails, made meanwhile cancels the queued startup claim, so the resume runs none; a
reconnect whose preparation fails and admits a ladder hands the key to that ladder first. A
reconnect or a self-check whose preparation fails and admits no ladder cancels the queued
startup claim before its release (Workflow #369 S6c review low (b)), so no startup run follows
on an instance that failed its preparation. A gated claim does not refuse the pause's hand-back (step (c)), nor a shutdown request
(Workflow #411 A3: `request-shutdown`, `restart`, an install CommitShutdown and a drain's
completion count no routine entry that a pause of its instance or the global pause holds, while
a routine entry that only a resource fence holds still counts; an accepted shutdown disconnects
the gated entry with `lease.queue_disconnected`, and the pause is kept and restored; since
0.12.26, W2-B, a startup claim so disconnected is kept in `host.idle_state` and the next start
queues it again behind the restored pause, `contracts/emulator-control.md`). Since
Workflow #369 S6c a routine claim that waits in the queue (a startup package, a ladder) is never
granted at once while the gate is closed, even on a free instance: it waits queued as a gated
entry from the first moment, and the resume (or the sweep that follows it) grants it. emu-watch
(Alice's ruling 35: a pause holds only dispatches; #414 option 3): the autostart claim of a
configured emulator start, at daemon start or by the emulator watch (`contracts/emulator-control.md`,
"Configured start at daemon start", "Emulator watch"), is no routine claim: a pause, a restored one
included, never holds or queues it and never makes it yield. Before WATCH-P and PAUSE-AUTO the
emulator was so started under the pause; it is not under the instance's own pause since 0.12.32
for the watch (WATCH-P) and 0.12.34 for the daemon start and IDLE-1's wake (PAUSE-AUTO), and not
under the global pause since 0.12.72 (#480 PG-1), all below: the start asks before it requests
the claim, and the claim asks on its key and ends there without a Start. The pause holds that
start's startup package, whose claim
waits queued as a gated entry and runs after the resume. An instance pause whose hand-back (step
(c)) meets such a start holding the instance's key waits for it, since 0.12.72 (#480 PG-1) beyond
the grace with no bound of the pause's own, and is never lifted because of it ("Instance pause:
three stages", (c)); since 0.12.73 (#480 PG-2c, "Two modes") so does a person's pause (`stand`),
its request answering at the end of its caller's time and the host carrying the wait on, while a
tool's hold (`quiet_or_lift`) fails at the grace as before 0.12.72. WATCH-P (since 0.12.32,
Alice's ruling 112, which replaces ruling 35 here for the watch): the emulator watch requests no
such claim, neither a restart nor a recognition, for an instance its own pause holds in any stage;
it only reads and records the emulator's status, records the skip once per pause revision
(`emulator_autostart_skipped`, `skip_reason` `instance_paused`), and after the resume restarts or
recognizes as before (`contracts/emulator-control.md`, "Emulator watch", "Pauses (WATCH-P)").
PAUSE-AUTO (since 0.12.34, Alice's ruling 117 A, which replaces ruling 35 here for the daemon start
and IDLE-1's wake): neither the configured emulator start at daemon start nor IDLE-1's wake after
`stop_emulator` starts the emulator of an instance its own pause holds in any stage. Neither
requests its claim (the wake stays owed), and a claim either requested before the pause asks
again on its key up to just before its Start, then releases the key and starts nothing; so do a
restart and a recognition claim the watch requested before the pause (the coordinator's rulings
of 10-31 on rulings 112 and 117 A), which bind and prepare nothing either and give the place they
took in the watch's bound back. Each skip is `emulator_autostart_skipped` (`skip_reason` `instance_paused`) once per pause revision and
start (`contracts/emulator-control.md`, "Pauses (PAUSE-AUTO)"). #480 PG-1 (since 0.12.72; the owner's
requirement on board #480: after "pause all" the Runtime starts no emulator and launches no game by
itself): the global pause, a restored one included, holds every one of these automatic starts the
same way (the daemon start's, the watch's restart and recognition, IDLE-1's wake, and on its key
each of their claims requested before it), its skip `skip_reason` `global_paused` once per global
pause revision, start and instance; where both pauses hold an instance, its own `instance_paused`
is recorded (`contracts/emulator-control.md`, "Pauses (PG-1)"). After the global resume the watch's
first poll restarts or recognizes, and the next idle check takes a withheld wake up, as after an
instance resume. An explicit emulator control runs under any pause as before. A stuck-recovery ladder is admitted only while its instance is not paused; a pause
that arrives while a ladder holds the instance ends the ladder at its next step
(`recovery_admission_denied`), and its one release lets the pause complete (`contracts/emulator-control.md`, "Stuck-recovery ladder").
A ladder's claim still queued is a routine claim, which the pause holds back in the queue; one
granted under the pause (handed the key just before it) is suppressed at its first check
(`recovery_ladder_suppressed`, `admission_denied`) and starts nothing. #480 PG-2b (since 0.12.73;
the owner's requirement on board #480: a pause a person asked for stands, and the Runtime starts
nothing after it): the ladder's holding check comes before every step of a rung that stops or
launches the application or stops or starts the emulator. Before, it came at every rung boundary,
before the emulator rung's Stop and Start and at every readiness poll, but not before the
application rung's force-stop of the assigned application nor before a rung's startup package
run (which launches the game), so a pause recorded just before either let the rung stop the game
and launch it again. Now it also comes just before that stop and just before each rung's startup
package run (before its `startup_package_scheduled`; an emulator rung on an instance without a
startup package runs nothing there and ends `environment_ready` as before). The return-home
package's first run asks at the rung boundary, its rerun after a busy hold at the hold's last
check right before it, and (since the slice's fix round 1) its rerun from the settle gate right
before it, after the probe: before that round the settle gate asked before its hold and probe
only, so a pause recorded while the probe took its frames let the package start. On a member of a
device group of several the application rung's first check comes (since fix round 1) before the
switch that stops the member in front, or every other member, ahead of the rung's own stop, so a
pause recorded at the rung's boundary stops no other member's application. An interruption at the
new checks ends the rung as an interrupted Stop or Start does: the rung failed with
`recovery_ladder_interrupted` caused by the interruption (`recovery_admission_denied` for a
pause), the ladder `exhausted` at warning at its next holding check, no error point, its one
release; at the settle gate's rerun it ends the climb as one during the hold does, the
return-home rung it was about to run recorded failed with it. What a rung that already stopped the
device group's other members owes the group stays owed as before (the member in front is
forgotten).

A rung that a pause interrupts after it took the application down (the application rung after its
stop) or brought the emulator up (the emulator rung from its Start on: each readiness attempt and
poll, and the check before its startup run) owes its startup run, and since fix round 1 (the
coordinator's ruling) hands it off as an emulator start under a pause hands off its startup
package (`contracts/emulator-control.md`): the scheduling intent `startup_package_scheduled` under
the ladder's links, the startup claim queued as a routine entry this pause holds back (a startup
claim of the instance still queued is cancelled first, as for every new one), and the claim kept
in `host.idle_state` while the ladder still holds the key. Nothing is handed off when the rung has
stopped nothing yet, when it is interrupted between the emulator rung's Stop and its Start (the
emulator stays stopped, and a startup run against a stopped emulator is meaningless, as an
emulator `stop` cancels one), or when another interruption ends it. The claim then goes as the
startup claim of a start under a pause goes, once: after an instance resume it follows the
reconnect, whose release hands the key on (a failed reconnect that admits no ladder cancels it,
one that admits a ladder hands the key to that ladder and the claim waits behind it); after a
global resume the queue's next pump grants it; a Runtime restart while paused drops the claim and
the next start queues it again behind the restored pause (`host.idle_state`); its grant forgets
what was kept, so no later restart runs it again. An explicit emulator `start` or `restart`
meanwhile replaces it with its own claim, a `stop` cancels it. No ladder starts while the pause
holds; a ladder that gets the key before the claim after the resume keeps it queued behind itself,
and that ladder's own startup run does not cancel it, as a startup claim queued behind a failed
preparation's ladder is not cancelled (#411 A50). Before fix round 1 the resume only reconnected
and queued no startup package, so the application stayed stopped until its startup package was
started again (the application rung of a later ladder, a person's or a client's emulator start
or restart, an idle wake), and the instance's first task met it stopped.

An emulator that a rung interrupted between its Stop and its Start (said since fix round 2) stays
stopped while the pause holds: none of the Runtime's automatic starts starts it under a pause (the
watch's restart, the daemon start's configured start and IDLE-1's wake are held, above, and no
ladder is admitted); only a person's or a client's emulator control, which runs under any pause,
does. After the resume the emulator watch restarts it at its next poll, as it restarts every
configured instance it finds stopped (`start_emulator` on, `emulator_control.watch_enabled` on,
within `restart_attempts` in `restart_window_ms`, not held stopped by IDLE-1; an instance
resume's reconnect first finds the emulator stopped, `instance_not_running`, which no shipped
trigger item names, so it admits no ladder and releases the key), and that start owes the startup
run: as every configured start, it queues the instance's startup package and hands the key to it.
An instance whose configuration names no automatic start (`start_emulator` off) stays stopped
until a person or a client starts it, as before this slice.

## Global pause

`PauseScheduling { scope: global }` closes the gate for every instance and answers at once:
`SchedulingPaused { scope, revision, drained: absent }`. It closes no device session and
touches no in-flight run.

## Two modes

#480 PG-2c (since 0.12.73; register §1.472; the coordinator's reading of the owner's words on
board #480: a pause a person asked for closes the gate at once and stands until a person lifts
it; the Runtime never lifts it by itself, not for a timeout, not for something in the way, not at
a shutdown, not for an error of its own). An instance pause carries the mode its request named.
The mode lives in the Runtime's memory only: the persisted row (`host.scheduling_pause`) is as
before, and a restored pause (stage `released`, where the mode changes nothing) has none. Since
0.12.75 (#480 PG-2e) the status answers the mode of every instance pause, `stand` for a pause no
request set ("Status" below).

- **`stand`** (a person's or an agent's pause: the console, `actingctl pause` and its MCP, the
  Lab's `pause` row and `ac_pause`; the absent member). The gate closes at once and the stages run
  as below. Nothing in the way fails it, and nothing lifts it but a person's resume, accepted in
  every stage ("Resume"):
  - another party's run, lease or queued entry, an owed reset, the Runtime's own automatic emulator
    start or ladder, a host package past its checkpoint: each is waited for, a client's or an
    operator's entry queued meanwhile included (`TransferNotSafe` no longer fails it);
  - **the end of the caller's time** (the drain timeout plus `30_000` ms; on its request's thread
    also while something, a Start through its readiness wait, holds the instance's admission
    guard): the request answers **Completed** with `SchedulingPaused { scope, revision, drained }`
    (the runs counted so far) and `scheduling_pause_standing` (info, exit 0) as the success code's
    cause, its members `pause_stage` (`draining` or `paused`) and `in_the_way` (the closed list
    `pause_obstacle`, each word once: `contained_run`, `policy_run`, `lease`, `automatic_start`,
    `recovery_ladder`, `queued_entry`, `client_reset`, `close_refused`, `close_unconfirmed`,
    `check_failed`), read from the host's state at that moment and kept with the pause in the
    Runtime's memory (the status answers it since 0.12.75: "Status" below). One code
    whatever is in the way: the consequence is the same. When the caller's time ends while another
    party holds the instance's admission guard, what is in the way is read outside the guard: an
    empty or absent list in a stage before `released` means that nothing was in the way at that read
    and the hand-back had not finished. A thread of the host then carries the rest of the stages on,
    in memory only (no state file, no record type of its own; since #480 PG-2d the moments of
    "A pause's life in the ledger" below are recorded): runs still in flight were asked to stop at
    their next checkpoint and are waited for with no bound; the stage becomes `paused`; the
    hand-back waits for everything in the way with no bound and becomes `released` once nothing uses
    the device (outside the multi-Nemu gate, after its session close). It ends at `released`, at a
    person's resume, or at the Runtime's shutdown, which joins it after the connections and keeps
    the pause (restored at the next start as stage `released`). Fix round 1: **no wait of its stages
    is blind**: the thread takes the instance's admission guard only when it is free, and between
    tries it reads, every `20` ms, whether the Runtime latched, whether a person's resume lifted the
    pause (its revision) and whether the Runtime is stopping; a thread that has ended is joined when
    the next one starts. Fix round 2: a panic of the thread is caught where it runs, as an instance
    worker's is, and latches the Runtime at once (`runtime_thread_panicked`, fatal), not first at
    its join;
  - outside the multi-Nemu gate a close lease the scheduler refuses (another member's entry ahead,
    the takeover cooldown of `lease_cooldown`, a holder; nothing else is `close_refused`) closes
    nothing and writes nothing to the ledger: the pause stays in stage `paused` with
    `close_refused`, and the close is tried again at most once in `10` s
    (`SCHEDULING_PAUSE_RETRY_INTERVAL`, a constant, no configuration): only the close rests, and
    what is in the way is still read every `20` ms; a close that could not be confirmed fenced the instance (Workflow
    #403) and is not tried again: the pause stays `paused` with `close_unconfirmed` until a person's
    resume of the fenced instance, which takes the reconnect's path once the close lease that close
    keeps has ended;
  - **a check of the stages that fails with a failure that is not fatal** (one the one decision
    point would end only the operation with: neither latching the Runtime nor fencing the instance;
    any other failure of the session close is one) fails nothing and lifts nothing: what is in the
    way is `check_failed` ("the last check of the pause's stages failed with a failure that is not
    fatal; it is tried again"), the check is tried again at most once in `10` s, and the whole check
    rests meanwhile: what is in the way is not read again until then, while the checks whether the
    Runtime latched, whether a resume lifted the pause and whether the Runtime is stopping keep the
    `20` ms poll. The two rests (the close's and the check's) are timers of the thread that runs
    the stages: when the request's caller's time ends and a thread of the host carries the stages
    on, that thread starts with neither, so one try of each follows the handover at once (fix round
    2, as built). The daemon's
    log says the failure once per pause and code, as a skipped background step is said; when the
    caller's time ends first, `scheduling_pause_standing` answers `check_failed` with that failure as
    its own cause. Everything that can refuse the request for its own input (its instance, a pause
    already held, its deadlines) is checked before its row is recorded, so a request refused for its
    input never became a pause, in either mode;
  - **a shutdown** while the pause is not yet `released`, within its caller's time: the request
    answers **Completed** with `scheduling_pause_kept_at_shutdown` (info, exit 0; `pause_stage`)
    as the cause, whatever is in the way; the gate stays closed until the Runtime stops, the
    persisted row still names the instance, and the next start restores it. One that finds nothing
    in the way of the hand-back of an instance inside the multi-Nemu gate ends it as `released`,
    with no cause (PG-1 fix round 3);
  - **a person's resume** while its own request still runs its stages: the request answers
    Completed with `scheduling_pause_lifted_by_resume` (info; `pause_stage`) as the cause. Fix round
    2: also while the request waits for the instance's admission guard, which its wait watches
    with its pause (lifted at its revision): it then writes nothing into the pause's state and no
    thread carries the stages on;
  - **a fatal error of a stage** (a poisoned lock, a latched condition), or a failure the decision
    point turns into a fence: the request answers that failure, as the door contains it (the Runtime
    latches, or the instance is fenced), and lifts nothing. Fix round 2: a failure that is not fatal
    and concerns the instance goes to the one decision point on the request's thread too, as on a
    continuation's (the instance is fenced, the request answers the fence's failure); the gate stays closed and the persisted
    row still names the instance, so the next start restores it; there is no stage to carry on, and
    on a Runtime that latched nothing does.
- **`quiet_or_lift`** (a tool's hold: the rescue module's holds before a deploy, a rollback, a
  `down`, a placement and an instance's addition, and the recording's hold; set in code, never a
  parameter): the instance quiet within the caller's time, or nothing, as before 0.12.72, for every
  obstacle, the Runtime's own included: anything in the way past the grace fails it
  (`scheduling_pause_release_busy`, `scheduling_pause_drain_incomplete`), the instance's admission
  guard still taken by another party at the grace too (`scheduling_pause_release_busy`; fix round
  1: before it, the wait for the guard had no bound), a refusing queued entry at once
  (`lease_transfer_not_safe`), a refused or an unconfirmed close too, any failure of a check, and a
  shutdown during the drain or the hand-back (`scheduling_pause_drain_interrupted`,
  `scheduling_pause_release_interrupted`); each failure lifts it ("If a stage fails" below). The
  waits with no bound of the pause's own that #480 PG-1 (the automatic start) and PG-2b (the
  recovery ladder) gave every requester are a `stand` pause's since 0.12.73, and
  `scheduling_pause_kept_at_shutdown` never answers a tool's hold. What a tool's hold does not wait
  for stays as before for both modes: the instance's own queued autostart claim refuses nothing
  (`AutostartEndsAtGrant`), and outside the multi-Nemu gate the close lease passes the entries the
  pause holds back (PG-2b).

**An automatic start past its last question** (#480 PG-2c, PG-1's leftover). The pause takes no
admission guard (that would keep its gate open until a Start ends, the Start holding the guard
through its readiness wait). An automatic start asks its last question
(`HostShared::automatic_start_hold`) with the instance's admission guard in hand, just before the
provider's Start (`drive_configured_control`) or the recognition's bind
(`prepare_recognized_instance`). A pause recorded before it stops the start there (PG-1); a pause
recorded after it finds the automatic start's claim lease (`ClaimKind::Autostart`) and lists
`automatic_start` in its way, truthfully, and the start that had begun is finished as before. Fix
round 2: the commit mark of the slice's ruling 7 is withdrawn (the coordinator's ruling; it added
only the word that lease already gives and decided nothing). The lease is there whenever the mark
was, because of three lines: `drive_configured_control` takes the instance's admission guard
before its last question and keeps it to the drive's end (`let admission = lock(&instance_guard,
..)`); `prepare_recognized_instance` takes it before its last question and drops it only after the
preparation (`let admission = self.lock_admission(..)`); and the lease sweep collects an expired
lease only when that guard is free (`cleanup_expired_if_guard_free`, `try_lock_admission`). Whoever
changes one of them changes what a pause recorded then lists. The host's gate
`pg2c_a_pause_after_an_automatic_starts_commit_lists_it_and_the_start_is_finished` shows a `stand`
pause recorded while that Start runs listing `automatic_start`, and
`pg2c_fix2_6_a_tools_hold_while_an_automatic_start_runs_fails_past_its_grace_and_is_lifted` a
`quiet_or_lift` pause failing past its grace then and lifted.

**What is answered when**, an instance pause:

| Case | `stand` | `quiet_or_lift` |
|---|---|---|
| `released` within the caller's time | Completed, no cause | Completed, no cause |
| something in the way at the end of the caller's time | Completed, `scheduling_pause_standing`; carried on | Failed, its code; lifted |
| a refusing queued entry | waited for (as above) | Failed, `lease_transfer_not_safe`; lifted |
| a shutdown before `released` | Completed, `scheduling_pause_kept_at_shutdown`; kept | Failed, `..._drain_interrupted` or `..._release_interrupted`; lifted |
| a person's resume meanwhile | Completed, `scheduling_pause_lifted_by_resume` | refused (`scheduling_pause_draining`, `scheduling_pause_releasing`) |
| a check that fails without being fatal | tried again (`check_failed`); at the end of the caller's time `scheduling_pause_standing` | Failed, its code; lifted |
| a refused close lease | tried again (`close_refused`) | Failed, its code (`lease_busy`, `lease_cooldown`); lifted |
| a fatal error of a stage, or a failure that fences the instance | the failure; nothing lifted | the failure; lifted |

Workflow #479 F-2 (since 0.12.74; [device-binding.md](device-binding.md), "An iOS handset"): an
instance whose device system has no backend in this build (an iOS handset, bound or not) meets only
the first row, in both modes (a `stand` pause also the row of a person's resume meanwhile), because
its stages move to `released` at once with nothing drained, waited for, handed back or closed.

## Instance pause: three stages

`Status` shows the stage of an instance pause: `draining` during (a) and (b), `paused` once (b)
is done while (c) runs, `released` once (c) is done. The request answers after (c); a caller whose
own wait ends first finds the pause standing (the end of this section). #480 PG-2c (since 0.12.73;
"Two modes" above): a `stand` pause's request answers at the end of its caller's time at the
latest, and what is said below of a wait past the grace, of a shutdown and of a failure that lifts
the pause holds as written for a `quiet_or_lift` pause; for a `stand` pause "Two modes" decides.

1. **(a) Stop dispatch.** The instance's gate closes at once; `Status` shows its pause with
   `stage: draining`.
2. **(b) Drain.** The request waits for the instance's in-flight contained runs (those in
   flight at the pause and any that start while it drains) to end on their own. A run still
   in flight once `drain_timeout_ms` elapsed is asked to stop at its next checkpoint: it ends
   as a typed failure with the code `contained_task_paused`
   (`ContainedTaskCancellationReason::PausedByOperator`,
   `RuntimeErrorCode::ContainedTaskPaused`): a policy run commits `task.failed` with that
   failure code, a client run commits `task.cancelled` and its receipt names the reason. No
   lease is preempted or reclaimed and `cancel_contained_task` is not used; the scheduler's
   `TransferNotSafe` and `Deferred` guards are unchanged. Once no run of the instance is in
   flight the stage becomes `paused` and the request answers
   `SchedulingPaused { scope, revision, drained: { finished, cancelled } }` once (c) is
   done: `finished` runs ended on their own, `cancelled` runs were asked to stop by this pause
   (a run drained again in (c) is counted too).
3. **(c) Wait until the device is idle** (ps2; Workflow #191 H). A pause is not a disconnect
   (owner ruling 2026-09-29): the instance's device session stays open and the Runtime keeps
   using nothing of it while the gate is closed. Under the instance admission guard the pause
   re-checks that nothing still uses the device:
   - a non-empty lease queue fails the pause at once with the close path's own rule
     (`prepare_resource_close`: `TransferNotSafe`): a queued client waits for the device. Since
     0.12.72 (#480 PG-1, fix round 1) the instance's own queued autostart claim is the exception,
     and the only one: while a scheduling pause, its own or the global one, holds the instance's
     automatic starts, it refuses neither the hand-back nor any grant of a dedicated lease of the
     instance (`grant_dedicated_instance_lease`, whoever asks for it: the resource-close lease a
     fenced close of the retained session takes when no lease holds the instance, the dedicated
     close lease below outside the multi-Nemu gate among them, and the daemon start's preparation
     of a physical instance,
     `DaemonStartPreparation`), which is granted ahead of it; the claim keeps its place and is
     granted after that lease. Another member's autostart claim, a claim of another kind and an operator's entry
     refuse as before. The claim is not cancelled in the queue, and the hand-back does not wait
     for it. Granted under the pause it starts no emulator and ends at its first tick on its key,
     one of two ways: held (the watch's restart or recognition claim and an idle wake's always, a
     daemon start's while its instance is stopped): its skip recorded, the watch's request given
     back, an idle wake owed again; or running (a daemon start's claim whose instance already
     runs, which it asks before the pause): `emulator_autostart_skipped` without `skip_reason`,
     and nothing more;
   - a contained run that started after (b) (a policy dispatch admitted just before the gate
     closed; the pause first waits for policy admissions that read the gate before it closed)
     is drained again under the same drain and grace deadlines; the device is never closed
     under a running task;
   - a client whose run ended cancelled owes the instance its usual `SafeReset`
     (`runtime-client` sends it right after a `ContainedTaskCancelled` receipt, on the same
     connection); the close waits until that reset has finished or that connection closed;
   - an active lease is waited for until it is released.

   These waits end at the grace deadline (the drain timeout plus `30_000` ms); past it the
   pause fails with `scheduling_pause_release_busy` (`LeaseBusy`). #480 PG-1 (since 0.12.72;
   production 11-17: a watch restart held the instance through its readiness wait, and the pause
   a person pressed was lifted when it ended; the coordinator's ruling of the fix round: a
   person's pause is never lifted because of the Runtime's own automatic claim): when the active
   lease is the autostart claim's (`ClaimKind::Autostart`, whoever asked for it: the configured
   start at daemon start, the watch's restart or recognition, IDLE-1's wake) and nothing else is
   waited for (no run to drain again, no owed reset, no policy run on the instance's worker), the
   hand-back waits for it beyond the grace for as long as the claim is there, with no bound of the
   pause's own. The wait ends when the claim ends: by its own deadlines, or once its key has lapsed
   and the lease sweep has taken it back. The gate stays closed throughout, and the stages go
   `draining`, `paused`, `released` as before, only later. A Runtime shutdown during such a wait
   does not lift the pause either (below). #480 PG-2b (since 0.12.73; the coordinator's ruling
   that extends R4′): the same holds for the Runtime's own stuck-recovery ladder. When the active
   lease is a ladder's (`ClaimKind::Ladder`, its continuation's included) and nothing else is
   waited for, the hand-back waits for it beyond the grace with no bound of the pause's own: the
   gate stays closed, the ladder ends at its next holding check ("The dispatch gate" above: before
   every step of a rung that stops or launches the application or stops or starts the emulator),
   and its one release lets the hand-back go on. Before 0.12.73 a ladder's lease was waited for
   only until the grace, like a client's: past it the pause failed with
   `scheduling_pause_release_busy` and was lifted, and the ladder's next holding check then passed
   and it climbed on. A ladder's claim still queued is a routine entry the pause holds back: it
   refuses nothing and is not waited for. In 0.12.72 and PG-2b this held for every requester of an
   instance pause, a person's (the console, `actingctl`, the Lab's `pause` and MCP) and a tool's
   hold (a rescue operation's, the recording's) alike; since #480 PG-2c (0.12.73, "Two modes") the
   wait past the grace is a `stand` pause's alone (and covers everything in the way), and a tool's
   hold (`quiet_or_lift`) fails at the grace for the automatic start and the ladder too, as before
   0.12.72. How long, by the code's own bounds (since the slice's fix round
   1 in this text; the bound corrected in fix round 2): the ladder ends at its next holding check,
   so the hand-back waits for the step the ladder is in. The longest is the emulator rung's Stop,
   which holds the instance's admission guard from the holding check before it to the one before
   its Start: the Stop itself, at most `210_000` ms fixed in the code (the control dispatch
   `60_000`, the wait for the stopped state `120_000`, the last status read `10_000`, the last
   process listing `10_000`, and the status read after a failure `10_000`; the rung's
   `recovery_ladder.rungs[emulator_restart].stop_bound_ms`, default `210_000` ms, renews the
   ladder's key to its value plus `recovery_ladder.hold.margin_ms`, default `5_000` ms, and may not
   be shorter than the device's part, `200_000` ms, but does not bound the step); then, since the
   rung's Stop is a phase of a restart (#411 A50), a Stop whose wait ran out
   (`emulator_control_wait_timeout`) waits, still under the guard, for the old VM to exit: up to
   `emulator_control.restart_exit_wait_ms` (default `60_000` ms; `1_000` to `300_000`), plus its
   last status read, which may end up to `10_000` ms after it. So the bound depends on the
   configuration: `220_000` ms plus `restart_exit_wait_ms`, that is `280_000` ms with the defaults
   (`520_000` ms at that key's largest value). The Start that follows holds the guard up to its own
   fixed bound, `220_000` ms (the control dispatch, the wait for the running state, the last status
   read, the ADB baseline `30_000`; `start_bound_ms`, default `220_000` ms, renews the key to it),
   before the first readiness attempt's check, so never longer than the Stop. Meanwhile the
   hand-back waits on that guard and does not poll, and a shutdown is seen once the guard is let
   go. Before fix round 2 this text gave one Stop's or Start's configured bound plus the margin,
   `225_000` ms, and left out the wait for the old VM. Every other step between two holding checks
   is shorter: the application stop (one ADB command, `60_000` ms at most), the application rung's entry probe
   (`adb_probe_ms`, default `30_000` ms), a readiness attempt's preparation (the backend opens,
   `30_000` ms), the settle gate's probe (`(probe_frames - 1) * probe_interval_ms`, default
   `4_000` ms), a poll of a hold or of a readiness wait (`busy.poll_ms`, `readiness_poll_ms`,
   default `500` ms); through a step that takes the admission guard the hand-back waits on it,
   through the others it polls every `20` ms. A package run the ladder began is not waited for
   so: it is drained again (below). The grace keeps its meaning for everything else (a
   person's or a client's run and lease, an operator's entry, an owed reset), which fails the
   pause as before, whether or not a ladder's claim waits behind it. With nothing left the stage
   becomes `released`; the device session stays as it is and nothing is recorded. Only an
   instance outside the multi-Nemu gate (`read-session-resource-close.md`, "Device session
   lifetime") closes its session here, through the existing fenced resource-close path
   (`close_retained_instance_while_guarded`, no second close implementation) and without an
   active lease: it grants the dedicated resource-close lease (`prepare_resource_close`, the
   fixed resource-close connection, `CapacityUse::Drain`), closes capture first and then input
   under `DeviceCloseAuthority::FencedDeviceWrite`, records the usual
   `runtime.lifecycle_observed` `ResourceQuiescence` observation and releases the lease with
   `LeaseReleaseReason::InstancePaused` (a `lease.released` event; the reason is the
   scheduler's release reason, the event carries no reason field); such an instance whose
   device session is not open closes nothing and records nothing. #480 PG-2b (since 0.12.73):
   this close lease is not held up by the instance's own entries that this pause holds back (its
   routine entries: a startup package, a ladder's claim; the host's
   `grant_dedicated_instance_lease` with the scheduler's `PauseHandBack`). When the lease reads
   the queue, the paused instance's gate is closed by its own pause (stage `paused`); none of those
   entries can be granted while the pause holds, the close lease takes none of them out of the
   queue, and after the resume they are granted as queued (the resume's reconnect first). Before
   0.12.73 the lease read every entry as an open gate does, so a startup package this pause held
   back answered `lease_queue_ahead`, the pause was lifted, and the package then passed the opened
   gate and launched the game. Only this close lease passes them (the lease released
   `InstancePaused`), unlike the autostart exception above, which every grant of a dedicated lease
   of the instance makes. Every other member's entries and every other entry of the instance
   count as before (`lease_queue_ahead`, `lease_transfer_not_safe`). Its other refusals still fail
   the pause and lift it, as before: a device in its takeover cooldown (`lease_cooldown`), a close
   that leaves a fence.

If a stage fails, the request fails as a whole (`Failed` receipt) and the gate it closed is
lifted again: no half-open pause is left behind. #480 PG-2c (since 0.12.73): so it is for a
`quiet_or_lift` pause; a `stand` pause is never lifted by a failure, and the failures and the
shutdown exceptions this paragraph and the next name are a tool's hold's ("Two modes"): for a
`stand` pause a shutdown before `released` within its caller's time answers
`scheduling_pause_kept_at_shutdown` (now Completed, information) whatever is in the way, and for a
`quiet_or_lift` pause it answers `scheduling_pause_drain_interrupted` or
`scheduling_pause_release_interrupted` and lifts it, whatever is in the way, as before 0.12.72. A drain whose stopped runs have not reached
their next checkpoint `30_000` ms after the drain timeout
(`SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS`) fails with `scheduling_pause_drain_incomplete`
(`ContainedTaskBusy`); a Runtime shutdown during the drain fails it with
`scheduling_pause_drain_interrupted`, during the hand-back with
`scheduling_pause_release_interrupted` (`RuntimeUnavailable`). The exceptions (#480 PG-1, since
0.12.72) are judged on the moment of the shutdown (since fix round 2): the hand-back reads again
then what is in the way (the lease's kind, a refusing queued entry, a run in flight, an owed
reset, a policy run on the worker), whatever its last check found and before its first check
too. When the Runtime's own work alone is in the way (above: its automatic emulator start, fix
round 1; since 0.12.73, #480 PG-2b, also its stuck-recovery ladder), the request answers
`scheduling_pause_kept_at_shutdown` (`Failed`, `RuntimeUnavailable`) and lifts nothing. When nothing is in the way any more and no refusing entry is queued (fix round 3:
the automatic claim may let go between the last check and the shutdown), the pause is not lifted
either and the hand-back ends as it would have: inside the multi-Nemu gate the stage becomes
`released` and the request answers success; outside it, where the hand-back would still close
the instance's session, the pause is kept and the request answers
`scheduling_pause_kept_at_shutdown`. Only work that is not the Runtime's own (its automatic
claim, its ladder) in the way (a person's or a client's run or lease, an operator's entry, an owed
reset, a policy run on the worker) answers `scheduling_pause_release_interrupted` and lifts the
pause as before. The
kept
pause stays in stage `paused` until the Runtime stops, its row of `host.scheduling_pause` stays as
it was recorded, and the next start restores it as every persisted pause is restored ("Persistence,
no expiry" below; stage `released`). A failed pause writes no ledger event of its own: its code
is in the request's receipt only, and the ledger shows the two records of `host.scheduling_pause`,
the one that set the pause and the one that lifted it (none for a kept one). #480 PG-2d (since
0.12.74): but a tool's hold lifted without its instance having become quiet (its caller's time
ended first, `scheduling_pause_drain_incomplete`, `scheduling_pause_release_busy`; since its fix
round 1 also a refusing queued entry, `lease_transfer_not_safe`, and a refused close lease, the
scheduler's refusal) writes after its lift one record, `scheduling_pause_lifted_not_quiet`, and a
kept `stand` pause one `scheduling_pause_kept_at_shutdown` ("A pause's life in the ledger"). A failed device close
(an instance outside the multi-Nemu gate) fails the request with the close's own error; an
unconfirmed close stays fatal as on every close path. The
client's receipt wait is the drain timeout plus that grace plus its IO timeout. Since 0.12.72 the
hand-back can last longer than that wait (it waits for an automatic emulator start, and since
0.12.73 for the Runtime's own ladder, with no bound of its own), and no client timeout changed: a caller (`actingctl`, MCP, the Lab, the console) whose
wait ends first reports its own timeout, while the pause stands and goes on. A second pause
request for the instance then answers `scheduling_already_paused`, and `Status` shows the pause
and its stage (`paused` while the hand-back waits, `released` once it is done). A pause that
waits for the ladder can still be lifted after the caller's own wait has ended, and later than
before 0.12.73, when the ladder's lease was waited for only until the grace (#480 PG-2b, said
since its fix round 1): by another party's entry queued meanwhile (a client's or an operator's
claim, `lease_transfer_not_safe` at the hand-back's next check), by a package run the ladder had
already begun (drained again under the pause's deadlines, which past the grace fails at once with
`scheduling_pause_drain_incomplete`), or by a reset that is owed (`scheduling_pause_release_busy`
past the grace). Each fails the request and lifts the pause as before; only the caller no longer
hears of it, and the lift is the pause's record without it. #480 PG-2c (since 0.12.73): none of
this is left. A `stand` pause's request answers at the end of its caller's time
(`scheduling_pause_standing`), so no caller's wait ends first, and the stages the host then
carries on lift nothing; a `quiet_or_lift` pause waits past the grace for nothing.

## Resume

`ResumeScheduling { scope }` lifts the matching gate and bumps the revision of that scope. A
global resume answers `SchedulingResumed { scope, revision, selfcheck: absent }` and
reconnects nothing.

#480 PG-2c (since 0.12.73; the coordinator's ruling 5 on board #480): **a person's pause
(`stand`) is resumable in every stage**. In stage `draining` or `paused` on an instance that is not
fenced, the resume lifts the gate at once, under the persist gate as every resume (its rows recorded
first; Workflow #453 S2 fix 1's hold of an unapplied `start_paused` entry applies as below), with
no reconnect claim (something still uses the instance) and no reconnect. Fix round 3 (the
coordinator's rulings; an instance's resume always carries a `selfcheck`, as the receipt's check
has required since before 0.12.73, so every client reads this answer): it answers
`SchedulingResumed { scope, revision, selfcheck }` whose `selfcheck` has both sides `ok: false`
with no values (a side without a report of a reconnect: none ran) and no `failure_code`, and its
receipt's outcome carries `scheduling_resume_without_reconnect` (info) as the success code's cause
(`RuntimeReceipt::scheduling_resumed`). #480 PG-2d (since 0.12.74): `runtime-client`'s
`resume_scheduling_as` and `resume_scheduling_expected_as` return that outcome with the result
(`SchedulingResumeAnswer`, the shape of `pause_scheduling_as`; `resume_scheduling` and
`resume_scheduling_expected` keep returning the result alone), the Lab's `resume` row (`actinglab
resume`, the Lab's `ac_resume`) answers it as its line's cause, and the lift is one record of "A
pause's life in the ledger". **What says that no reconnect ran is that cause**, not the
self-check's shape: `failure_code` names only a reconnect that failed and is absent here, and a
reconnect that runs answers a code on every failing step this build has (a refused preparation
lease, a failed close of the session it still held, a failed emulator status read, a failed open,
a failed final close). A global resume never carries a `selfcheck`. What
still uses the instance goes on
(a run already asked to stop at its next checkpoint still stops there; a lease is released by its
holder); outside the multi-Nemu gate the session the hand-back has not closed stays open for the
next lease; the pause's own request, or the thread that carries its stages on, ends at its next
check (`scheduling_pause_lifted_by_resume` on the request). A fenced instance (Workflow #403)
resumes through the reconnect below in every stage, the resume being a person's re-proof of its
close. A fenced iOS handset is the exception ([device-binding.md](device-binding.md), "An iOS
handset"): its resume, paused or not, lifts the pause if one holds and writes
`scheduling_resume_without_backend` as on every iOS handset, and the fence stays, because nothing
can certify the instance again until this build has a backend for its device system (an inert
handset has no way to be fenced by a run: nothing it runs opens a session or a backend). A tool's
hold (`quiet_or_lift`) keeps the refusals below. A pause in stage `released` is resumed as follows,
in either mode.

Fix round 1 (#480 PG-2c; outside the multi-Nemu gate, where the hand-back closes the session; inside
it nothing is closed and nothing changes): **the hand-back's close and a person's resume decide
under one lock**, the pause table's. Just before its close, with the instance's admission guard in
hand, the hand-back asks whether its pause is still held at its revision and, in the same step,
marks in memory that its close has begun; a pause no longer held closes nothing and its hand-back
ends. The resume of a `stand` pause in stage `draining` or `paused` reads, under its persist gate
and the same lock, where the hand-back is: its close begun, or the stage `released` reached since
the resume read it, the resume lifts nothing without a reconnect and takes the path of a `released`
pause's resume below (the reconnect claim, the reconnect, `selfcheck` answered); otherwise it marks
in the same step that it lifts without a reconnect, which a hand-back about to close meets (it
closes nothing and looks again at its next check), and a lift that fails ends that mark. A close
lease that was refused closed nothing and ends the close's mark. So, **after a `stand` pause's
resume is answered `Completed`, the instance has the session it would have without the pause**
(never closed by it, or closed and reconnected by the resume), **and nothing of the lifted pause
acts on the instance afterwards**: every check of its stages asks first whether its pause is still
held at its revision, and a pause asked again has another revision.

An instance resume (ps2) accepts a pause in stage `released` only; a resume refused for its
stage or its expectation takes no claim. An instance whose device system has no backend in this
build (an iOS handset, bound or not) takes none of what follows
([device-binding.md](device-binding.md), "An iOS handset"): past the same refusals its resume lifts
the pause under the persist gate and attempts no reconnect (no claim, no preparation, no open), its
`selfcheck` both sides `ok: false` with no values and no `failure_code` and its receipt's cause
`scheduling_resume_without_backend` (a pause held again with `instance.added` answers as on every
instance). Since Workflow #369 S5 the resume then waits for its
reconnect claim (`ResumeReconnect`) in the instance's one queue, at most
`CONNECTION_PREPARATION_CLAIM_WAIT_MS` (`3_000` ms), before anything changes. It is an operator
entry, so the closed gate does not hold it back, while every routine entry stays held back. Not
granted in time, the resume is refused `lease_queue_expired` (`QueueExpired`) before the gate is
lifted: nothing has changed, and the operator retries. Once the claim is granted the resume
lifts the gate (the stage and the expectation are checked again, since the pause can change
while the claim waits; a refusal then releases the claim and lifts nothing) and then reconnects
the device at once instead of at the next lazy open. Since Workflow #317 sc3 the reconnect is the
instance's connection preparation phase (`runtime-fact-store.md`, "Connection preparation
phase"): on the reconnect claim's key, with no dedicated preparation lease, and under the
instance admission guard only for its device steps,
`ExecutionKernel::open_instance_backends` opens the instance's input and capture backends
through the same provider opens as the lazy paths (a Nemu pair opens once) and takes the first
frame of a capture it opened, as the first lazy capture does, then drops it (no frame artifact
is written). The opens are recorded like every open (`backend-open-observation.md`):
`backend.open_observed`, the `backend.selfcheck.*` facts, `device.self_check`, and the
instance's policy availability following the self-check. The claim's key is then released
(its release hands the instance on, so a startup or other claim the lift made eligible follows
the reconnect; no dispatch reaches the instance while the key is held) and the session stays
open for the next tasks (Workflow #191 H; a failed open, or an instance outside
the multi-Nemu gate, closes it before the release). The session kept through the pause, or
one a read-only observe retained after it (the existing sc2 design), is closed first, so a
resume is a disconnect and a reconnect and the receipt's self-check reflects this
preparation's complete opens (Workflow #191 h3). The receipt carries the self-check projected
from those reports:

```text
selfcheck = {
  capture: { ok, width, height, capture_backend },
  touch:   { ok, backend, max_x, max_y, invasive: false },
  failure_code: <device code> | null
}
```

- `capture.ok`: the Capture (or Nemu pair) report's open status and `capture_check` passed;
  `width` / `height` are its frame size, `capture_backend` its selected backend.
- `touch.ok`: the Input (or Nemu pair) report's open status and `input_check` passed;
  `backend` is its selected backend, `max_x` / `max_y` its connection's input geometry, else
  its handshake limits (absent for a Nemu pair). The check reads the connection back and sends
  no touch.
- A side without a report of this resume has `ok: false` and no values.
- `failure_code` is the code of the failing step verbatim: a failed open's device code, for
  example `input_backend_open_failed`, `capture_backend_open_failed`,
  `capture_backend_operation_failed` or `paired_backend_open_failed`; or a failed close's
  code. A resume and a self-check take no preparation lease that could be refused (Workflow
  #369 S5): a claim not granted within its wait refuses the request with `lease_queue_expired`
  instead, and no self-check is reported.

A failed reconnect does not roll the resume back and is not retried: the resume is performed,
the failure is recorded, the instance stays unavailable until a self-check passes, and the
receipt reports it. Only an unconfirmed close or a failed record fails the request. A failed
reconnect that admits a stuck-recovery ladder hands the claim's key to it
([emulator-control.md](emulator-control.md), "Preparation stages"); one that admits none (stuck
recovery off for the instance, a ladder the cool-down suppresses, a failure no trigger item
names) first cancels the instance's queued startup claim (`scheduler.denied
lease_queue_cancelled`, warning, as a `stop` does), then releases the key, so no startup run
follows on an instance that failed its preparation; the failure stays in the receipt's
self-check and in `instance_preparation_finished` (Workflow #369 S6c review low (b)). A
self-check's failed preparation does the same.
`actingctl selfcheck <alias>` (`SelfCheckInstance`) runs the same phase without a pause, on its
own `SelfCheck` claim, which waits as the reconnect claim does (at most
`CONNECTION_PREPARATION_CLAIM_WAIT_MS`, without the admission guard; not granted in time, it is
refused `lease_queue_expired` and no preparation runs), and answers
`InstanceSelfChecked { instance_alias, selfcheck }` with this `selfcheck` shape. The client's
receipt wait for an instance resume and for a self-check is the backend-open bound
`CONNECTION_PREPARATION_WAIT_MS` (`30_000` ms: the touch handshake and the capture prime) plus
the claim's wait `CONNECTION_PREPARATION_CLAIM_WAIT_MS` (`3_000` ms), that is `33_000` ms, plus
its IO timeout (Workflow #191 h2, #369 S5); a global resume keeps the IO timeout. A resume triggers no
extra policy evaluation: the next cycle runs on its ordinary trigger.

## Conditional resume (Workflow #338 R4)

`ResumeScheduling` may carry `expected: { owner_epoch, revision }`, the owner epoch the caller
saw the pause in and the revision of its scope then (a pause receipt's `revision`, or the
`Status` pause's). The field is optional and absent from the wire when unset: a resume without
it is unchanged. `revision` is at least 1 (`invalid_scheduling_pause_expectation`).

With `expected`, the host compares while it holds the pause table's lock, before the state
checks and the lift of the table below: an `owner_epoch` other than its own is
`scheduling_pause_owner_epoch_mismatch`; a `revision` other than the scope's current revision
(zero for an instance scope never paused in this epoch) is `scheduling_pause_revision_mismatch`.
Both are refusals through the same `Denied` / `InvalidRequest` path as the other refusals. A
match then behaves exactly as an unconditional resume. Revisions restart with every owner
epoch, which is why the epoch is part of the condition.

An older Runtime does not know the field: `RuntimeOperation` denies unknown fields, the frame
fails to decode and the connection is dropped without a receipt (stage
`runtime_ipc_request_decode`, effect not performed). Only when the call wrote its own frame
and that connection then ended before the receipt header (end of stream, reset or abort) does
the client (`RuntimeClient::resume_scheduling_expected`) reconnect and, when the same owner
epoch answers, read `Status`; a connection already failed by an earlier call, a receipt
timeout and every other failure are returned unchanged. After such a drop, the scope still
paused at the expected revision means nothing was lifted and is reported as `runtime_operation_unsupported`; any other state, or a failed
reconnect or another epoch, is the uncertain `runtime_scheduling_resume_unconfirmed` (or
`runtime_owner_epoch_changed`). The client never falls back to an unconditional resume.

## Revisions and refusals

The global pause and every instance pause hold their own revision, and neither implies the
other. Setting or lifting a gate (a failed stage included) bumps the revision of its scope; the
stage changes `draining` to `paused` to `released` do not.

| Situation | Receipt | Host code |
|---|---|---|
| Pause while the same scope is already paused (either stage) | `Denied`, `InvalidRequest` | `scheduling_already_paused` |
| Resume of a scope that is not paused | `Denied`, `InvalidRequest` | `scheduling_not_paused` |
| Resume of an instance whose tool's hold (`quiet_or_lift`) is still draining | `Denied`, `InvalidRequest` | `scheduling_pause_draining` |
| Resume of an instance whose tool's hold (`quiet_or_lift`) is still handing its device back (`paused`) | `Denied`, `InvalidRequest` | `scheduling_pause_releasing` |
| Conditional resume naming another owner epoch | `Denied`, `InvalidRequest` | `scheduling_pause_owner_epoch_mismatch` |
| Conditional resume naming another revision of the scope | `Denied`, `InvalidRequest` | `scheduling_pause_revision_mismatch` |

## A pause's life in the ledger

#480 PG-2d (since 0.12.74; register §1.481). Before it, only the client that asked for an instance
pause learned what happened to it, from its receipt; a second console, the Lab, or the same console
after a restart saw a stage and could not tell why. Each moment of an instance pause's life that the
receipt and the stage do not tell is now one record: `runtime.lifecycle_observed`, phase
`device_state_observed` (the record the emulator watch's `emulator_autostart_skipped` and the idle
check's `instance_idle_action` are written as), under the instance, carrying the links of the
pause's own request (its request and correlation identifiers, whichever thread or request writes
the record), its severity the outcome's category. One record per moment, never one per poll or per
try:

| Moment | Outcome (severity) | Written by |
|---|---|---|
| a `stand` pause's caller's time ended with something in the way (also an unconfirmed close, which ends it at once) | `scheduling_pause_standing` (info): the receipt's own cause, the same code with `pause_stage`, `in_the_way` and, for `check_failed`, the failure as its cause | the request, before it answers and before its continuation starts; under the persist gate, only while the pause still holds (fix round 1) |
| that pause later became quiet by itself (stage `released`) | `scheduling_pause_settled` (info, no member) | the continuation; since #480 PG-2e (0.12.75) under the persist gate, only while the pause still holds, as the request writes its own |
| a person's resume lifted a `stand` pause in `draining` or `paused` without a reconnect | `scheduling_resume_without_reconnect` (info): the resume's receipt's own cause | the resume, after the lift, under the persist gate with it |
| Workflow #479 F-2 ([device-binding.md](device-binding.md), "An iOS handset"): an instance whose device system has no backend in this build (an iOS handset, bound or not) is resumed, a person's resume or a tool's, its pause in stage `released` (a restored one and a start's hold included) or, fenced, in any stage or none, and no reconnect is attempted (a pause held again with `instance.added` writes none) | `scheduling_resume_without_backend` (info, no member): the resume's receipt's own cause | the resume, after the lift, under the persist gate with it; under the pause request's links, else (no request set the pause, or none holds) the instance's own under fresh system links |
| a shutdown kept a `stand` pause before `released` | `scheduling_pause_kept_at_shutdown` (info, `pause_stage`): the cause its request answers, or would have answered | the request within its caller's time (as it writes the standing record: under the persist gate, only while the pause still holds), else the continuation (since #480 PG-2e the same way) |
| a `quiet_or_lift` hold was lifted without its instance having become quiet: its caller's time ended first (its drain past the checkpoint grace, `scheduling_pause_drain_incomplete`; its hand-back past the grace, `scheduling_pause_release_busy`, the admission guard still taken by another party included) or, since fix round 1 (the coordinator's ruling: every lift of a hold without quiet is recorded), a queued entry refused its hand-back (`lease_transfer_not_safe`) or the scheduler refused its close lease (another member's entry ahead, the takeover cooldown, a holder: `lease_busy`, `lease_cooldown`) | `scheduling_pause_lifted_not_quiet` (warning): `pause_reason` the hold's reason, `in_the_way` what was in the way (what its last check read, or what is read outside the guard another party holds, which may be empty; for a refusing entry what that check read, `queued_entry` among it; for a refused close lease `close_refused`), that failure as its cause | the request, after the lift |

Nothing else of a pause writes one: a pause quiet within its caller's time (its receipt and its stage
say all); a check that fails and is tried again (the daemon's log says it once per pause and code,
as before; the standing record carries `check_failed` when the caller's time ends on it); a resume
of a pause already `released`, of a fenced instance, or of a pause whose hand-back's close had begun
(the reconnect's own records; that receipt has no cause); a person's resume during the request's
own stages (`scheduling_pause_lifted_by_resume` on that request's receipt; the resume writes the
lift's record); an unconfirmed close the continuation meets (the pause stands, the fence's own
records say it); a tool's hold that failed for anything else (an unconfirmed or an otherwise failed
close, a failed check, a shutdown, a fatal error: the receipt and the fact records say it); a
restored pause, at the restart or later (the restore's fact record and stdout line). The global
pause has no stages and writes none. An iOS handset is the exception to the resume of a pause
already `released` and of a fenced instance ([device-binding.md](device-binding.md), "An iOS
handset"): its resume runs no reconnect, its receipt has the cause
`scheduling_resume_without_backend` and it writes that row's record.

**The order of a pause's records** (#480 PG-2e, since 0.12.75, after PG-2d's fix round 1; what the
code guarantees, no more). Every record that says a pause still holds is decided and written under
the persist gate, only while the pause is still held at its revision (`record_held_pause_moment`):
its request's (`scheduling_pause_standing`, or `scheduling_pause_kept_at_shutdown` within its
caller's time) and, since PG-2e, its continuation's (`scheduling_pause_settled`, or
`scheduling_pause_kept_at_shutdown` after the caller's time). Every lift takes that gate, and a
person's resume writes its `scheduling_resume_without_reconnect` under it with its lift. So no
such record comes after the lift of its pause (the fact record without it) or after the resume's
record: a pause lifted (or held again with `instance.added`) before the writer took the gate writes
none, and the lift's records say what happened. The standing record is written before the thread
that carries the stages on starts, so it comes before every record of that thread. In particular:
a resume of the pause once it is `released` (the reconnect's path) takes the gate either after the
settled record, which then comes before the lift's fact record and the reconnect's records, or
before it, and no settled record is written; inside the multi-Nemu gate, a person's resume that
marks its lift (`Lifting`) and lifts in one hold of the gate, while the hand-back reaches
`released` (there it closes nothing and does not ask the mark) and its stage moves to `released`
before the lift, leaves its `scheduling_resume_without_reconnect` as the pause's last record: the
continuation finds the pause lifted when it takes the gate, and writes no settled record; a resume
already running when a shutdown came either comes after the kept record or leaves it unwritten. A
hold's `scheduling_pause_lifted_not_quiet` comes after its lift's fact record. The resume's record
is written inside the gate, after its lift, so it also comes before every record of whoever holds
the gate next (a later pause of the same instance, its request's records among them).

The links of a pause's request are kept with the pause in the Runtime's memory until it is lifted,
so the record a person's resume writes carries them too; a pause no request set (a restored pause, a
start's hold) is `released` and is never resumed that way, except on an iOS handset
([device-binding.md](device-binding.md), "An iOS handset"), whose resume of it writes
`scheduling_resume_without_backend` under the instance's own links (fresh system links, as the
emulator watch links its records), no request's links being kept for it. The codes are outcome
codes inside an existing record, so a reader of `ledger` revision 9 reads them as it reads every
code it has never seen (it checks a code's spelling, never that it is registered: [component-interfaces.md](component-interfaces.md),
the `ledger` row of revision 5), and neither `ledger` nor `runtime-outcomes` moves, as for
`global_paused` (register §1.468). A record that cannot be written changes nothing the pause does
or answers: the failure is said in the daemon's log (the line of a skipped background step) with
the record it could not write linked to it, and a write the ledger gave up on stops the Runtime by
its own path, as every append does.

## Status

`Status` (`RuntimeControlPlaneStatus`) carries `scheduling_pause: { revision, pause_reason,
since_unix_ms }` while the global pause holds, and each `RuntimeInstanceStatus` carries
`pause: { revision, pause_reason, since_unix_ms, stage: draining | paused | released }` while
its instance is paused. Both fields are absent otherwise (additive wire: a
`deny_unknown_fields` reader must move to this contract before it reads a paused status).

#480 PG-2c (since 0.12.73; `runtime-client` 12; the ruling 4 on board #480, made while the console
drew what it shows of a pause from the status alone; it reads its own pause receipt's cause since,
below): the instance's `pause` gains two members, declared with
revision 12: `pause_mode` (`stand` or `quiet_or_lift`, absent when not answered) and `in_the_way`
(the closed list `pause_obstacle` of "Two modes", each word once, absent when empty and always in
stage `released`). `InstancePauseState` denies unknown members, so a reader of revision 11 or lower
refuses a status that carries either (its client fails `runtime_receipt_decode_failed` and latches
the connection), and the Runtime cannot tell a client's revision on a connection (request v4 and
receipt v2 keep their schema strings; [component-interfaces.md](component-interfaces.md), the
`runtime-client` row of revision 4). So, as Workflow #437 G1 carried `device_held` over (revision
10, register §1.433), **no Runtime of 0.12.73 or 0.12.74 answers either member**: the status of
every pause there is the four members above, which the console pinned to v0.12.71 (revision 11)
reads, and `min` stays 3. The release whose Runtime first answers them raises `runtime-client`'s
`min` to 12; **the console must pin v0.12.73 or later before that release**. #480 PG-2d (register
§1.481): until a Runtime answers the two members, a console learns what is in the way of its own
pause from that pause's receipt's cause (`scheduling_pause_standing` with `pause_stage` and
`in_the_way`), and any observer (a second console, the Lab, the same console after a restart)
learns a pause's stage from `stage` (`draining` or `paused` while the host carries it on), what
still uses the instance from `activity`, `lease_active` and `queued_request_count`, and what
happened to the pause from the records of "A pause's life in the ledger" above.

#480 PG-2e (since 0.12.75; register §1.485): **the Runtime answers both members, and
`runtime-client`'s `min` is 12** ([component-interfaces.md](component-interfaces.md), the row of
revision 12: a reader below 12 refuses a status that carries an instance pause, and the Runtime
cannot tell it apart on a connection; the console pinned v0.12.73 before this release). Since
0.12.75 the status is the source of a pause's mode and of what is in its way, for every observer.
- `pause_mode` is answered for every instance pause in every stage: the mode its request set; a
  pause no request set (restored at a start, held at a first start, held again with
  `instance.added` at a release) answers `stand`, the mode the Runtime treats it as (the Runtime
  never lifts it by itself). The global pause has no stages and gains nothing.
- `in_the_way` is what the pause's own latest check read, kept with the pause in memory: in stage
  (b) the drain's, at every poll (a contained run of the instance in flight, `contained_run`, or
  nothing), then the hand-back's; on the request's thread within its caller's time, afterwards on
  the continuation's. When the caller's time of a `stand` pause ends it is the very reading the
  receipt's `scheduling_pause_standing` and its record carry, word for word; after a check that
  failed and is tried again it is `check_failed`, the standing cause's word for it, not the reading
  before the failure. It is absent before the first check (from the gate's closing to the drain's
  first read, on the request's thread with no wait between them), when the latest check found
  nothing, and always in stage `released`; an iOS handset's pause is `released` at once, so it
  answers its mode and no `in_the_way`.
- A status request runs no check of its own: it reads a copy of the pause table taken under the
  table's lock, as before, takes no other lock and waits on nothing a check waits on. So
  `in_the_way` is as old as the pause's latest check: it can be as old as the continuation's retry
  interval (`SCHEDULING_PAUSE_RETRY_INTERVAL`, `10` s, while the whole check rests after a failed
  check), and older while a check itself waits: for a run it drains again during the hand-back (the
  check read it before, `contained_run` among what is in the way), or for the instance's admission
  guard another party holds (a Start through its readiness wait: on the request's thread until the
  end of its caller's time, then read outside the guard; on the continuation's until it takes it).
- The receipt's cause and the records of "A pause's life in the ledger" stay what they are: a
  receipt answers its caller at once, the records are the pause's history.

## Relation to the player hold

Workflow #429 R-7a (register §1.233; [player-hold.md](player-hold.md)): a player hold is the
instance's own state beside its pause, never a pause. It has its own table, its own runtime fact
`host.player_hold` and its own revision; it never writes, changes or replays a
`host.scheduling_pause` row, so ruling 116's pauses stay through every acquire, release, failure
and restart. Pauses and holds order only under the one persistence gate they share. While a
hold lasts (R-7b) a pause may be set, changed or lifted as usual: every pause stage completes at
once (no run, no lease, the session closed), and an instance resume lifts the pause without a
reconnect, its `selfcheck` both sides `ok: false` with `failure_code: player_hold_active`, which
means the expected, not a failure. Ending a hold reads the pauses of that moment and, under one,
touches no device (`next: instance_paused` or `scheduling_paused`). The control gates' one order
(player hold, instance pause, global pause, resource fence; R-7a §2.6) gives an instance's
effective code; since R-7a2 (register §1.234, 0.12.35) this contract's dispatch gate follows it
("The dispatch gate" above), in place of its former "global pause first".

## Persistence, no expiry

A pause has no TTL; only `ResumeScheduling` lifts it. Since Workflow #361 B1 a pause survives a
restart. The held pauses are the runtime fact `host.scheduling_pause` (scope `Runtime`,
`record_list`, no lifetime; `contracts/runtime-fact-store.md`), one row per held pause:
`scope` (`global` or `instance`), `instance_alias` (instance rows), `pause_reason`,
`since_unix_ms` (`timestamp_ms`), `set_in_owner_epoch` and, on a restored row,
`restored_from_owner_epoch`. An empty list means nothing is paused; a ledger without the fact
(any ledger older than this rule) starts unpaused, as before.

- **Write before change.** A pause records the rows it leads to before it closes the gate; a
  resume records the rows without its scope before it lifts the gate. Pause, resume and the
  restore are ordered by one persist gate. A refused record fails the request with
  `scheduling_pause_persist_failed` (`Failed`, `LedgerFailure`) and the gate stays as it was; a
  failed append poisons the Runtime as every runtime-fact append does. A process that dies
  between the record and the gate change restarts on the side the operator asked for. A failed
  instance pause stage lifts its gate and records the rows without it; if that record fails
  the request poisons the Runtime. Since 0.12.72 (#480 PG-1 fix round 1) a hand-back a shutdown
  interrupted while the Runtime's own automatic emulator start alone (since 0.12.73, #480 PG-2b,
  or its ladder alone) was in the way (fix round 2:
  at the moment of the shutdown; `scheduling_pause_kept_at_shutdown`), or (fix round 3) at which
  nothing was in the way any more, lifts nothing and records nothing: the rows that set the
  pause stay, and the next start restores it.
- **Restore at start.** A start that is not installer-held reads the fact before any physical
  instance is prepared and before the policy driver: the global pause comes back at revision 1
  in the new owner epoch, an instance pause at revision 1 with stage `released`, so the startup
  preparation, which skips a released instance pause, leaves its device untouched.
  Reason and `since` are kept. A pause of an alias that is no longer registered is dropped. The
  restored rows are recorded again with `restored_from_owner_epoch` (the owner epoch before
  this start); that record is the ledger trace of the restore and of every dropped row.
  `actingd` prints one stdout line per pause: `actingd scheduling_pause_restored
  scope=<global|instance:alias> since=<ms>` or `actingd scheduling_pause_dropped
  scope=instance:<alias> reason=instance_not_registered`. A malformed fact fails the start
  (`scheduling_pause_fact_invalid`).
- **Installer-held starts** kept the pauses their install transition carried
  (`host.install_transition.pauses`) and recorded them as `host.scheduling_pause`, until Workflow
  #434 LAB-6S4a (0.12.63) deleted the held start: since then a start invalidates the two
  transition facts a 0.12.61 state root may still hold (`host.install_transition` and its
  `.pauses`, once, as a takeover) and restores only the persisted pauses above; an installer
  pauses each instance itself before it stops the Runtime and resumes it after the new start.
- **Conditional resume** after a restart needs the restored pause's new owner epoch and
  revision 1, read from `Status`.
- **Older Runtimes** ignore the fact: a switch back to one starts unpaused, and a resume made
  there does not clear the fact, so the next start of this Runtime restores that pause again
  (visible in `Status` and in the stdout line); resume it once more.

The ledger event set is unchanged: besides the fact records, a pause or resume leaves its trace
in the request receipt, the refused dispatch admissions, the terminals of the runs it stopped,
the device close's existing records (an instance outside the multi-Nemu gate) and the
reconnect's existing close and open records; since #480 PG-2d (0.12.74) also in the
`device_state_observed` records of "A pause's life in the ledger", an existing record type. A
restore writes none of those: its trace is the fact record above and the stdout line.

## Pauses held at a first start

Workflow #453 S2 (0.12.60; register §1.362; the owner's approval of board #453 on 11-13: an
instance added while the Runtime is stopped starts paused and stays paused until a person resumes
it; the coordinator's rulings 3 and 6 on it). An `instances[]` entry may declare `start_paused:
true` ([runtime-config.md](runtime-config.md), "An entry that starts paused"); the installer writes
it on an instance it adds while the Runtime is stopped (Workflow #453 S3, 0.12.60; register
§1.365: the Lab's rescue module's `instance add` with no Runtime answering, on the new entry only,
[install-cli.md](install-cli.md), "Instances, the first slice", "While the Runtime is stopped").

- **Who holds it.** The Runtime itself, at every start, as the third kind of start hold beside the
  hold of an unsettled dispatch and of an instance whose settlement facts the start skipped: after
  the persisted pauses are restored (an installer-held start, until Workflow #434 LAB-6S4a: after
  its transition's pauses were kept) and those holds are taken, before the instance workers and any instance's daemon-start
  preparation. For each entry that declares it and whose instance id the tagged runtime fact
  `host.start_pause_applied` ([runtime-fact-store.md](runtime-fact-store.md)) does not name yet,
  in the order of the instance ids (the global pause does not count as the instance's own):
  - **not paused:** it holds an **instance** pause with the existing reason `instance.added`, at
    the scope's next revision, stage `released`, set in this owner epoch, and applies the entry;
  - **paused with `instance.added`** (a start that ended between the two records below, or the
    installer's pause of an added instance): that pause stays as it is, and it applies the entry;
  - **paused for another reason:** that pause stays exactly as it is and **nothing is recorded**:
    the member stays unapplied until that pause is released (below).

  To apply is to record the entry's row `{instance_id, applied_at_unix_ms}`. The pause rows
  (`host.scheduling_pause`) are recorded first, the applied rows second; a refused record fails
  the start, as the other start holds fail. A start that ends between the two records finds the
  restored `instance.added` pause at the next start, keeps it and then records the row, so the
  hold is idempotent. As every released instance pause, it leaves the device as it is: the
  daemon-start preparation does not prepare it, and neither the configured emulator start at
  daemon start nor an idle wake starts its emulator (PAUSE-AUTO, "The dispatch gate" above).
- **Applied only by its own pause** (fix 1, the coordinator's ruling on the review). Another
  pause can be released by someone who never looked at the new instance (the installer's
  `--hold` restored at the start, a rescue operation's hold, a pause carried over on a reused
  alias). So when an instance's pause is released while the Runtime runs and its entry's member
  is unapplied, the release holds it with `instance.added` instead of letting it run. Every path
  that ends an instance pause while the Runtime runs is the host's `ResumeScheduling` of the
  instance: a person's resume from any entry, the installer's `release-hold` and every other
  hand-back of its hold, the rescue module's hand-back (each a conditional resume); the one other
  lift, a failed pause stage's, only lifts a pause its own request set on an instance that was
  not paused, which an unapplied entry never is. In the resume's own step, under the persist gate
  that makes the release and on its reconnect claim's key, after the resume's refusals (stage,
  expectation): the old pause is lifted and an `instance.added` pause set (each at the scope's
  next revision, stage `released`, set in this owner epoch), its rows recorded, the live gate
  changed from the old pause to the new one under one lock (the instance is never without a
  pause in between, and the key is held throughout, so nothing is dispatched), then the applied
  row recorded. No reconnect runs; the key is released at once and a routine claim stays gated
  behind the new pause. The answer is the resume's own shape: `SchedulingResumed` with the new
  pause's `revision` and a `selfcheck` whose two sides are `ok: false` with no values and whose
  `failure_code` is `dispatch_paused_instance`, the instance's control gate: the release was
  made, and the instance is held again (as the player hold's `player_hold_active` means the
  expected, not a failure). A refused record fails the request with
  `scheduling_pause_persist_failed`: before the change nothing changed; after it the instance
  stays held with `instance.added` and the member unapplied, so the next release holds it again.
  Resuming the `instance.added` pause itself is an ordinary resume: the row is there and nothing
  happens again, also across restarts. A start that finds the earlier other pause gone (for
  example dropped by the restore because the instance id is configured under another alias now)
  holds `instance.added` then, as for an instance not paused.
- **Once applied, never again.** A start finds the entry's row and holds nothing; the member may
  stay in the file. Each record of the fact keeps only the rows of instance ids this start's
  configuration names (any entry, with or without the member), so the list holds at most one row
  per configured instance; an instance id removed from the configuration that comes back with the
  member is held again. A configuration whose entries do not declare it holds nothing, neither
  reads nor writes `host.start_pause_applied`, and leaves every release exactly as before (the
  set of unapplied entries is empty and the release returns before anything else).
- **The reason.** `instance.added` is the reason of the installer's pause of an added instance on
  a running Runtime too ([install-cli.md](install-cli.md), "Configuration, instances, packs and a
  fresh root"): one meaning for the
  person, "a new instance waits for you". No reason word and no stage is added; the reason is
  `INSTANCE_ADDED_PAUSE_REASON` of the contract crate.
- **Nobody's hand-back hold.** It is no installer's `--hold` and no rescue operation's
  `lab-<op>-<ms>`: nothing hands it back by itself. The rescue module keeps and lists it as
  another party's pause and never lifts it. A hand-back of the installer's or the module's own
  hold that meets an unapplied entry is answered as above: its own pause is gone, and the
  instance shows `instance.added`, another party's pause from then on. The installer's side
  (Workflow #453 S3): every hand-back of the rescue module's own hold reads that answer and
  records the instance in its release's `held_again` (`{alias, reason, revision}`, the reason
  `instance.added`, the revision answered), not among the instances `resumed`, in its journal and
  its answer ([install-cli.md](install-cli.md), "A hand-back the Runtime answers by holding the
  instance again"); `actingctl install release-hold`, whose face does not change, still lists it
  as resumed while it stays paused: the safe side.
- **Who lifts it, and how.** A person, by a resume of the instance: the console's resume,
  `actingctl resume --state-root <s> --instance <alias>` (below) or the Lab's `resume <alias>`.
  Once lifted, the applied row keeps it from coming back.
- **What the installer's stop needs to know.** `actingctl install stop` (and every form that
  stops) treats it as another party's pause: the stop refuses `guard_failed` unless
  `--known-pause <alias>=instance.added` names it, as for the installer's own `instance.added`
  pause today ([install-cli.md](install-cli.md), "Formal stop, start and the hold (INST-2)");
  the start restores it as every pause. An `install start` whose Runtime holds the pause for the
  first time (an instance added while stopped) finds one pause more than its S0; from Workflow
  #453 S3 the post-start row `pauses` expects it (an `instance.added` pause on an instance that S0
  and the hold leave unpaused and whose entry declares `start_paused`) and names the instance in
  its `start_paused`, so the start answers no caveat for it; any other extra pause is still the
  caveat `post_check_mismatch` ("The post-start checks (INST-2)"), not a failure. Before S3 that
  pause too was the caveat. An update's own stop meets an entry not yet applied only while a
  pause of another reason holds it: the stop treats that pause as today, the next start restores
  it, and the entry stays unapplied until that pause is released.

## CLI

```bash
actingctl pause  --state-root <state-root> [--instance <alias>] [--reason <code>] [--drain-timeout-ms <n>]
actingctl resume --state-root <state-root> [--instance <alias>]
actingctl status --state-root <state-root>
```

Origin `(Cli, Cli)`. Without `--instance` the scope is global. `--reason` defaults to
`operator` and `--drain-timeout-ms` to `60000`; both are checked against the bounds above
before any request is sent. `pause` / `resume` print the result; `status` prints the global
and per-instance pause states within the status. #480 PG-2c: `actingctl pause` and its MCP's
`ac_pause` send a person's pause (`stand`, `RuntimeClient::pause_scheduling`); the Lab's `pause` row
(`actinglab pause`, the Lab's `ac_pause`) sends `stand` too and answers the receipt's cause
(`RuntimeClient::pause_scheduling_as`). #480 PG-2d: the Lab's `resume` row (`actinglab resume`,
the Lab's `ac_resume`), its parameters unchanged, answers the receipt's cause the same way
(`RuntimeClient::resume_scheduling_as`, with `expect` `resume_scheduling_expected_as`): a resume of
a person's pause that was not quiet yet answers `runtime_scheduling_resumed` caused by the receipt's
outcome, whose cause is `scheduling_resume_without_reconnect`, exit 0, and a resume of an iOS
handset answers the same way with `scheduling_resume_without_backend` (no backend to reconnect:
[device-binding.md](device-binding.md), "An iOS handset"); an ordinary resume answers no cause.
`actingctl resume` and its MCP's `ac_resume` answer the result as before.
