# ACCEPTANCE · AK MAA Data Fidelity M6 Offline Calibration

Task file: `C:\合作工作区\ActingCommand\TASK-AK-maa-data-fidelity.md`

## Scope

This report records historical Codex-side M6 offline evidence gathered before
the 2026-07-04 r3/r4 task-file boundary update.

After that update, current Codex work on this chain is limited to Runtime logic
and synthetic/offline program validation. True resource-repository verification,
M6 calibration, and M8 template/art processing are Claude-owned resource-lane
work. This report remains useful as prior evidence and attribution context, but
it is not a current Codex-owned completion gate.

## Historical source freshness

Resource repositories were fetched before this pass and were aligned with their
remote `origin/main` heads before local calibration changes:

| Repository | Path | Base commit |
| --- | --- | --- |
| Arknights | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights` | `0b318bf8517344e45eeea502b5da0d3ea78b2dd7` |
| AzurLane | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane` | `ea5246ac13985f19ba774863a59539f7d6f4b443` |
| BlueArchive | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive` | `dae51cf1227445ffffd76acd71ba8a22af88b3bf` |

The Runtime repository was also fetched before the 2026-07-04 continuation and
was aligned with `origin/main` at `169ad688a64ab42ee577ae6ea1af633c2df04969`.

## Structural reconversion

All three resource repositories were re-converted with the current Runtime
converter and passed page-pack structural validation:

| Repository | Result |
| --- | --- |
| Arknights CN | `resource convert` wrote 10 bundles, 16 targets, 11 pages, 13 edges, 7 page operations, 25 primitives; `detect-page --check-pages` passed. |
| AzurLane JP | `resource convert` wrote 41 bundles, 81 targets, 41 pages, 43 edges, 17 page operations, 89 primitives; `detect-page --check-pages` passed. |
| BlueArchive JP | `resource convert` wrote 20 bundles, 22 targets, 20 pages, 19 edges, 23 page operations, 53 primitives; `detect-page --check-pages` passed. |

## Retained-frame corpus

The current local retained-frame corpus found for AK page calibration contains:

| Label | Expected page result | Path |
| --- | --- | --- |
| `home_retest` | `arknights/home` | `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\actinglab-labpkg\ak16416-retest-current.png` |
| `home_run` | `arknights/home` | `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\actinglab-labpkg\runs-retest\lab1y-20260625_051921_950\output\screenshots\20260625_051922_653.png` |
| `mission_result` | no generated page should match | `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\actinglab-labpkg\ak16416-current.png` |
| `terminal_stage_map` | `arknights/terminal` | `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\actinglab-labpkg\runs\lab1y-20260625_050455_365\output\screenshots\20260625_050456_022.png` |
| `operator_positive` | `arknights/operator` | `C:\Users\Alice\Documents\Azur\ActingCommand-Runtime\target\p2_2_smoke\capture-16416.png` |
| `depot_positive` | `arknights/depot` | `C:\Users\Alice\AppData\Local\Temp\claude\C--Users-Alice--Cloude-Code\4b6f507f-47bd-4051-9315-5e8cf04f9b4a\scratchpad\akpg_depot.png` |
| `friends_positive` | `arknights/friends` | `C:\Users\Alice\AppData\Local\Temp\claude\C--Users-Alice--Cloude-Code\4b6f507f-47bd-4051-9315-5e8cf04f9b4a\scratchpad\akpg_friends.png` |
| `mission_positive` | `arknights/mission` | `C:\Users\Alice\AppData\Local\Temp\claude\C--Users-Alice--Cloude-Code\4b6f507f-47bd-4051-9315-5e8cf04f9b4a\scratchpad\akpg_mission.png` |
| `signin_modal` | no generated page should match | `C:\Users\Alice\AppData\Local\Temp\claude\C--Users-Alice--Cloude-Code\4b6f507f-47bd-4051-9315-5e8cf04f9b4a\scratchpad\wf_arknights_recruit.png` |
| `announcement_modal` | no generated page should match | `C:\Users\Alice\AppData\Local\Temp\claude\C--Users-Alice--Cloude-Code\4b6f507f-47bd-4051-9315-5e8cf04f9b4a\scratchpad\recak_close0.png` |
| `quickswitch_dropdown_positive` | `arknights/quickswitch_dropdown` | `C:\Users\Alice\AppData\Local\Temp\claude\C--Users-Alice--Cloude-Code\4b6f507f-47bd-4051-9315-5e8cf04f9b4a\scratchpad\akqs_overlay.png` |

## Calibration decision

The previous AK generated page pack allowed several destination-button targets
to pass while the client was still on the home screen, and also allowed the
mission-result screen to satisfy unrelated destination anchors. This is unsafe
because navigation-panel buttons are visible before navigation has happened.

The calibration pass adds two explicit page-discrimination controls:

- A retained-frame negative target, `page/negative_mission_result`, cropped from
  the mission-result screen and used only as a forbidden target.
- A retained-frame terminal-stage target, `page/terminal_stage_map`, cropped
  from the retained terminal CE map frame and required by `arknights/terminal`.
- Source-level `page_rules` support in Runtime conversion, so resource bundles
  can carry page forbidden-target rules without hand-editing generated pages.

AK page rules now forbid `page/negative_mission_result` on every generated page.
All non-home generated pages also forbid `page/home`, preventing home-screen
navigation buttons from being misread as successful arrival on destination pages.
All non-terminal pages also forbid `page/terminal_stage_map`, preventing the
retained terminal stage map from satisfying unrelated QuickSwitch-derived page
anchors.

The 2026-07-04 continuation also fixes the `arknights/operator` page definition
semantics. Its two operator anchors are now emitted as one `any_of` page group
instead of two simultaneously required targets, so a real operator frame can
match either the expanded-role or collapsed-role visual state. This removes a
structural false negative in the generated page pack.

The same continuation adds `page/operator_0` and `page/operator_1` as forbidden
targets for non-operator QuickSwitch-derived pages. This lets the retained
operator frame match `arknights/operator` only instead of being accepted by
other destination-button pages.

The follow-up calibration pass adds retained-frame positive evidence for
`arknights/depot`, `arknights/friends`, and `arknights/mission`. It raises the
shared home anchor threshold from `0.80` to `0.85` so the depot frame no longer
trips `page/home`, raises `page/mission` from `0.85` to `0.92` so the depot and
terminal frames no longer trip `page/mission`, and forbids the retained
`page/depot`, `page/friends`, and `page/mission` anchors on non-owner pages.

The modal-negative calibration pass adds retained-frame negative targets for
the daily sign-in modal and the announcement modal. These modal frames were
previously able to satisfy several QuickSwitch-derived destination targets even
though no destination navigation had completed. The new negative targets are
used only as forbidden page-rule targets, so modal overlays now resolve to
standby/no generated page instead of silently claiming arrival.

The QuickSwitch dropdown calibration pass replaces the small home-icon menu
anchor with a wider retained dropdown navigation-strip anchor and adds it as a
forbidden target on non-dropdown pages. This makes actual dropdown frames match
`arknights/quickswitch_dropdown` only, while retained home, depot, friends,
mission, sign-in, and announcement frames no longer satisfy the dropdown page.

## Threshold samples

| Target | Frame | Score | Threshold | Passed |
| --- | --- | ---: | ---: | --- |
| `page/home` | `home_retest` | 1.000000 | 0.850000 | true |
| `page/home` | `home_run` | 0.999832 | 0.850000 | true |
| `page/home` | `mission_result` | 0.798440 | 0.850000 | false |
| `page/home` | `home` | 0.999999 | 0.850000 | true |
| `page/home` | `depot_positive` | 0.805745 | 0.850000 | false |
| `page/home` | `friends_positive` | 0.794509 | 0.850000 | false |
| `page/home` | `mission_positive` | 0.787997 | 0.850000 | false |
| `page/negative_mission_result` | `home_retest` | 0.664962 | 0.920000 | false |
| `page/negative_mission_result` | `home_run` | 0.661303 | 0.920000 | false |
| `page/negative_mission_result` | `mission_result` | 1.000000 | 0.920000 | true |
| `page/negative_signin` | `signin_modal` | 1.000000 | 0.920000 | true |
| `page/negative_signin` | `home` | 0.637873 | 0.920000 | false |
| `page/negative_signin` | `depot_positive` | 0.738727 | 0.920000 | false |
| `page/negative_announcement` | `announcement_modal` | 1.000000 | 0.920000 | true |
| `page/negative_announcement` | `home` | 0.649569 | 0.920000 | false |
| `page/negative_announcement` | `depot_positive` | 0.791150 | 0.920000 | false |
| `page/quickswitch_dropdown` | `quickswitch_dropdown_positive` | 1.000000 | 0.900000 | true |
| `page/quickswitch_dropdown` | `depot_positive` | 0.810085 | 0.900000 | false |
| `page/quickswitch_dropdown` | `home` | 0.684120 | 0.900000 | false |
| `page/quickswitch_dropdown` | `signin_modal` | 0.716161 | 0.900000 | false |
| `page/quickswitch_dropdown` | `announcement_modal` | 0.768541 | 0.900000 | false |
| `page/terminal_stage_map` | `home_retest` | 0.721208 | 0.920000 | false |
| `page/terminal_stage_map` | `home_run` | 0.712677 | 0.920000 | false |
| `page/terminal_stage_map` | `mission_result` | 0.628819 | 0.920000 | false |
| `page/terminal_stage_map` | `terminal_stage_map` | 1.000000 | 0.920000 | true |
| `page/operator_0` | `operator_positive` | 0.926657 | 0.900000 | true |
| `page/operator_1` | `operator_positive` | 0.752787 | 0.900000 | false |
| `page/operator_0` | `home_retest` | 0.826827 | 0.900000 | false |
| `page/operator_0` | `mission_result` | 0.824092 | 0.900000 | false |
| `page/operator_0` | `terminal_stage_map` | 0.797798 | 0.900000 | false |
| `page/depot` | `depot_positive` | 0.999884 | 0.920000 | true |
| `page/depot` | `home` | 0.843894 | 0.920000 | false |
| `page/depot` | `friends_positive` | 0.705160 | 0.920000 | false |
| `page/depot` | `mission_positive` | 0.891082 | 0.920000 | false |
| `page/depot` | `terminal_stage_map` | 0.895703 | 0.920000 | false |
| `page/friends` | `friends_positive` | 0.999370 | 0.850000 | true |
| `page/friends` | `home` | 0.735043 | 0.850000 | false |
| `page/friends` | `depot_positive` | 0.751854 | 0.850000 | false |
| `page/friends` | `mission_positive` | 0.594007 | 0.850000 | false |
| `page/mission` | `mission_positive` | 0.999949 | 0.920000 | true |
| `page/mission` | `home` | 0.766288 | 0.920000 | false |
| `page/mission` | `depot_positive` | 0.870751 | 0.920000 | false |
| `page/mission` | `terminal_stage_map` | 0.881791 | 0.920000 | false |

## Page-discriminativeness result

After conversion, retained-frame page detection produced:

| Frame | Matching pages |
| --- | --- |
| `home_retest` | `arknights/home` |
| `home_run` | `arknights/home` |
| `mission_result` | none |
| `terminal_stage_map` | `arknights/terminal` |
| `operator_positive` | `arknights/operator` |
| `depot_positive` | `arknights/depot` |
| `friends_positive` | `arknights/friends` |
| `mission_positive` | `arknights/mission` |
| `signin_modal` | none |
| `announcement_modal` | none |
| `quickswitch_dropdown_positive` | `arknights/quickswitch_dropdown` |

An additional release-build scan across 93 retained AK screenshots under the
current Runtime `target` tree found 91 home matches, one terminal match, and
one standby/no-match frame; no scanned retained frame produced multiple
generated page matches after the terminal-stage and any-of updates.

A broader local 1280x720 inventory scan across 241 Runtime and cooperation
workspace screenshots found the retained operator frame as a single
`arknights/operator` match. The same mixed-game stress scan still produced 74
multi-match results on non-AK AzurLane/BlueArchive frames for several
QuickSwitch-derived destination pages, so those mixed-corpus results are
recorded as a remaining false-positive risk rather than accepted AK calibration
evidence.

## Historical remaining evidence gap

The local retained-frame corpus does not contain accepted positive captures for
the destination pages `recruit`, `gacha`, `infrast`, or `mall`. The QuickSwitch
dropdown, depot, friends, mission, and operator pages now each have at least one
positive retained frame, but single-frame evidence is not a full threshold
distribution. Positive threshold distribution for the remaining destination
pages is not proven by the current corpus.

Under the updated task split, this retained-frame evidence gap is owned by the
Claude resource lane. A future Codex task should only reopen this report if true
resource verification identifies a Runtime parser/converter bug that can be
reproduced with synthetic fixtures.
