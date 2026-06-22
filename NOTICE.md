# NOTICE.md

ActingCommand Runtime is planned to use `AGPL-3.0-only`.

This split repository was created from the local ActingCommand workspace. It contains the `AliceRuntimeOrchestrator` runtime prototype and independent runtime scripts.

No upstream automation source code has been copied into this repository as part of the split.

## Included external tool binaries

### MaaTouch

- Source project: `MaaAssistantArknights/MaaTouch`
- Source URL: https://github.com/MaaAssistantArknights/MaaTouch
- Source path reviewed locally through BAAH release `DATA/touch.zip` and the upstream `MaaTouch` repository.
- Local destination: `external-tools/maatouch/maatouch`
- License: Apache-2.0
- License text: `external-tools/maatouch/LICENSE`
- Attribution: MaaTouch is maintained by the `MaaAssistantArknights/MaaTouch` upstream project and contributors. The upstream repository and the reviewed `touch.zip/LICENSE.txt` do not provide a separate filled copyright notice beyond the Apache-2.0 license text.
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
