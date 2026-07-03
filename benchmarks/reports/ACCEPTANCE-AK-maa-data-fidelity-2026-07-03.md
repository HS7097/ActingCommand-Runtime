# ACCEPTANCE · AK MAA Data Fidelity M6 Offline Calibration

Task file: `C:\合作工作区\ActingCommand\TASK-AK-maa-data-fidelity.md`

## Scope

This report records the Codex-side M6 offline evidence available in the current
worktrees. It does not include live click sampling; the task file assigns live
items to a later unified device batch.

## Source freshness

Resource repositories were fetched before this pass and were aligned with their
remote `origin/main` heads before local calibration changes:

| Repository | Path | Base commit |
| --- | --- | --- |
| Arknights | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-Arknights` | `c8a110a3b285f519934307a1897a13564ad245b4` |
| AzurLane | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-AzurLane` | `ea5246ac13985f19ba774863a59539f7d6f4b443` |
| BlueArchive | `C:\Users\Alice\Documents\Azur\ActingCommand-Resources-BlueArchive` | `dae51cf1227445ffffd76acd71ba8a22af88b3bf` |

## Structural reconversion

All three resource repositories were re-converted with the current Runtime
converter and passed page-pack structural validation:

| Repository | Result |
| --- | --- |
| Arknights CN | `resource convert` wrote 10 bundles, 15 targets, 11 pages, 13 edges, 7 page operations, 25 primitives; `detect-page --check-pages` passed. |
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

## Threshold samples

| Target | Frame | Score | Threshold | Passed |
| --- | --- | ---: | ---: | --- |
| `page/home` | `home_retest` | 1.000000 | 0.800000 | true |
| `page/home` | `home_run` | 0.999832 | 0.800000 | true |
| `page/home` | `mission_result` | 0.798440 | 0.800000 | false |
| `page/negative_mission_result` | `home_retest` | 0.664962 | 0.920000 | false |
| `page/negative_mission_result` | `home_run` | 0.661303 | 0.920000 | false |
| `page/negative_mission_result` | `mission_result` | 1.000000 | 0.920000 | true |
| `page/terminal_stage_map` | `home_retest` | 0.721208 | 0.920000 | false |
| `page/terminal_stage_map` | `home_run` | 0.712677 | 0.920000 | false |
| `page/terminal_stage_map` | `mission_result` | 0.628819 | 0.920000 | false |
| `page/terminal_stage_map` | `terminal_stage_map` | 1.000000 | 0.920000 | true |

## Page-discriminativeness result

After conversion, retained-frame page detection produced:

| Frame | Matching pages |
| --- | --- |
| `home_retest` | `arknights/home` |
| `home_run` | `arknights/home` |
| `mission_result` | none |
| `terminal_stage_map` | `arknights/terminal` |

An additional scan across the retained AK screenshot output directories used in
this pass found 81 home matches and one terminal match; no scanned retained
frame produced multiple generated page matches after the terminal-stage guard
was added.

## Remaining evidence gap

The local retained-frame corpus does not contain positive captures for the
destination pages `recruit`, `depot`, `friends`, `gacha`, `infrast`, `mall`,
`mission`, `operator`, or the actual QuickSwitch dropdown overlay. Their
current rules now fail safely on the available non-target frames, but positive
threshold distribution for those destination pages is not proven by the current
corpus.

The broader `TASK-AK-maa-data-fidelity.md` CLI gate should not be marked fully
closed until destination-page positive retained frames are added or a later
accepted task narrows the M6 evidence requirement to the currently retained
home/mission-result corpus.
