# CHECKPOINT.md

## Current status

Runtime repository-local planning has been initialized.

Future Runtime tasks should update and commit this repository's `PLANS.md` and `CHECKPOINT.md` together with Runtime source changes.

## Recent Runtime milestones

- P2.1.1 capture artifact path security close-out:
  - commit `edb69302b4bfe25d2c2a61004b1b94ead32965b4`
  - tag `checkpoint/20260618-p2-1-1-capture-store-security`
- P4a recognition primitive engine:
  - commit `5083b136022abe4907af3dfd653b399952038a65`
  - tag `checkpoint/20260618-p4a-recognition-primitives`

## 2026-06-18 Runtime repo-local planning initialization

### Current status

- Added Runtime-local `AGENTS.md`, `PLANS.md`, and `CHECKPOINT.md`.
- Supersedes the previous routine behavior of mirroring Runtime task planning files into the umbrella repository.
- Runtime future task close-out should commit and push planning/checkpoint updates in this repository.
- Runtime-local planning initialization was pushed to `HS7097/ActingCommand-Runtime`.

### Files changed

- `CHECKPOINT.md`
- `AGENTS.md`
- `PLANS.md`

### Commands run

- Checked Runtime repository status.
- Created Runtime-local planning files.
- Committed and pushed Runtime-local planning files.

### Test results

- Documentation/policy-only change; no code tests required.

### Current blocker

- None.

### Next step

1. Use Runtime-local `PLANS.md` and `CHECKPOINT.md` for the next Runtime task.

## 2026-06-18 Runtime-to-main merge policy

### Current status

- Clarified Runtime-to-main repository merge policy by user instruction.
- Routine Runtime updates stay in `HS7097/ActingCommand-Runtime`.
- Do not merge, copy, or synchronize Runtime changes into the umbrella/main `HS7097/ActingCommand` repository by default.
- Merge a Runtime state into the main repository only after the user explicitly confirms that merge point.
- Runtime-local policy update is recorded in commit `7e587f956067ab21384a11b784df60a8eab788fd`.

### Files changed

- `AGENTS.md`
- `PLANS.md`
- `CHECKPOINT.md`

### Commands run

- Updated Runtime-local policy files.
- `git commit -m "docs: clarify Runtime-to-main merge policy"`
- Amended the checkpoint with the final Runtime commit hash before pushing.

### Test results

- Documentation/policy-only change; no code tests required.

### Current blocker

- None.

### Next step

1. Use this merge policy for future Runtime work.
