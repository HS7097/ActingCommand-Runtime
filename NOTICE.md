# NOTICE.md

ActingCommand Runtime is planned to use `AGPL-3.0-only`.

This split repository was created from the local ActingCommand workspace. It contains the `AliceRuntimeOrchestrator` runtime prototype and independent runtime scripts.

No upstream automation source code has been copied into this repository as part of the split.

## Included external tool binaries

### MaaTouch

- Source project: `LmeSzinc/AzurLaneAutoScript`
- Source path reviewed locally: `bin/MaaTouch/maatouch`
- Local destination: `external-tools/maatouch/maatouch`
- Purpose: MaaTouch/minitouch-compatible input backend binary used by `MaaTouchBackend`.
- Notes: included after license review by project owner instruction. Runtime input still goes through ActingCommand's `MaaTouchBackend`; no ADB input fallback is added.

Before any upstream code, assets, screenshots, templates, OCR data, or model files are copied, adapted, or merged, update this file with:

- upstream project name
- upstream repository URL
- copied/adapted file path
- original license
- original copyright notice
- local destination path
- modification summary
