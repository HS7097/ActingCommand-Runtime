# Vision model folders

Workflow #360. actingd runs its OCR and NN models in-process. The models of a
daemon live in one vision root; a recognition pack names a model by
`model_ref` and `model_sha256`, and the daemon finds it by folder name.

## Vision root

```text
<vision root>\
  ort\onnxruntime.dll                    required when `vision` is configured
  ort\onnxruntime_providers_shared.dll   optional; loaded first when present
  ort\onnxruntime_providers_cuda.dll     required for execution_provider cuda
  models\<name>\                         one folder per model; <name> is its model_ref
```

The `vision` section of `actingd.config.json` selects it (see "Vision root" in
`contracts/actingd-check-config.md`): `root` when given, else
`<install root>\vision` when the daemon runs from an install root, else
`vision` next to the configuration file. A root inside the install root's
`A\` or `B\` is refused. Plain files directly under `models\` (the single-folder
file layout of earlier releases, manifests) are not models and are ignored.

## Folder layouts

A model folder holds exactly one of these layouts:

| Role | Flat | Nested |
|---|---|---|
| OCR detector (optional) | `det.onnx` | `det\inference.onnx` |
| OCR recognizer | `rec.onnx` | `rec\inference.onnx` |
| OCR dictionary | `keys.txt` | `rec\keys.txt` |
| NN classifier | `model.onnx` | — |
| Description (optional) | `model.json` | `model.json` |

Other files in a folder are ignored. An OCR model without a detector reads only
regions; a `full_frame` region on it fails with the reason. An angle classifier
(`cls.onnx` or `cls\`) is not supported.

## Description `model.json`

UTF-8 JSON of at most 64 KiB, unknown fields refused:

```json
{"schema_version":"actingcommand.vision_model.v1","family":"ppocr-ctc","languages":["en"]}
```

- `schema_version`: `actingcommand.vision_model.v1`.
- `family`: `ppocr-ctc` for an OCR folder, `onnx-classify` for an NN folder;
  it must match the layout.
- `languages`: optional, informational; recorded at startup and never compared
  with a target's languages.

A folder without `model.json` has the family's defaults. The description digest
is the SHA-256 of the canonical description with every default filled in, so a
missing file and a file that spells out the defaults have the same digest. The
description is read once at startup; a change needs a restart.

## Identity

- OCR: `actingcommand.ppocr-model-set.v1` over the SHA-256 of the detector
  (`none` when absent), recognizer and dictionary files, with the classifier
  `none`. It does not depend on the layout or the file names.
- NN: the SHA-256 of `model.onnx`.
- The description is not part of the identity. It enters the engine binding
  digest each OCR execution records as `provider_binary_sha256`, together with
  the SHA-256 of the running executable and of `onnxruntime.dll`.

## Invalid folders

A folder breaks the rule when its name is not a valid `model_ref` (empty, over
255 bytes, a path separator, a colon or a control character, or not UTF-8), it
or a model file is a symbolic link or junction, it mixes layouts, a layout is
incomplete, it holds an angle classifier, or its `model.json` is unreadable,
invalid or names another family. Startup records such a folder with its reason
and keeps running; every request naming it fails with that reason.
`actingd check-config` fails with `vision_model_folder_invalid`. A missing
`models\` folder, a missing `ort\onnxruntime.dll` and an empty `models\` fail
startup.

## Startup and first use

Startup reads directory entries, file metadata and descriptions only, and
records, under the `provider.startup_observed` path binding stage, the
`vision_root`, the `onnxruntime_library`, each runtime library, one
`ocr_model:<name>` or `nn_model:<name>` binding per folder (layout, family,
detector presence, description digest, languages) and one
`invalid_model:<name>` binding per invalid folder with its reason.

On a model's first use the engine reads its files once, computes the identity
from those bytes and builds the sessions from the same bytes only when the
identity equals the target's `model_sha256`; a different identity fails with
`ModelMismatch` and builds nothing. The first build initialises ONNX Runtime
once per process. At most four models are loaded at once; the least recently
used idle model is unloaded to make room and is read and hashed again on its
next use. A model whose files changed while the daemon runs is retired until
restart. Waiting for a model held by another request counts against the
target's `timeout_ms`; the model's own load does not.

An OCR recognizer's output class count must be the dictionary size plus one
(the CTC blank) or plus two (the blank and the space class); any other count
fails the load naming both counts.
