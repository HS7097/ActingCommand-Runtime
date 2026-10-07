# Component interfaces

Workflow #364 (v0.11.3). The Runtime, the UI and each resource repository
release on their own. acsetup decides whether the programs and resource bundles
it is about to combine can work together from what each of them declares, not
from a list of release pairs. A component that changed nothing another one
depends on keeps working with it, whatever its version number.

This document is the vocabulary and the rules. The Runtime's own declaration is
`distribution/windows/component-interfaces.json`; acsetup (UI repository,
`crates/acui-setup`) is the reader that checks them.

## Declaration

A Runtime, Tools or UI build manifest (`BUILD-MANIFEST.json`) carries an
additive `interfaces` object. Every existing reader of the manifest ignores it.

```json
"interfaces": {
  "schema_version": "actingcommand.component-interfaces.v1",
  "speaks": {
    "actingd-config": [2, 2],
    "install-selection": [1, 1],
    "install-control": [0, 1],
    "ledger": [1, 1],
    "runtime-client": [1, 1],
    "package": [1, 1]
  }
}
```

Where each declaration comes from:

- **Runtime.** `distribution/windows/component-interfaces.json`. The Windows
  exact-SHA build copies it unchanged into the Runtime and the Tools manifest
  of the same commit, and stops when the file is not a valid v1 declaration. No
  binary is run to produce it.
- **UI.** `crates/acui-setup/component-interfaces.json` in the UI repository.
  The UI build copies it into the UI manifest, and acsetup compiles the same
  file in as its own declaration, so the program and its manifest cannot
  disagree. The UI manifest keeps `installation_selection_schema`.
- **Resource bundle.** An optional `interfaces.json` at the bundle zip's root,
  next to `bundle.json`:

  ```json
  {
    "schema_version": "actingcommand.component-interfaces.v1",
    "speaks": { "package": [1, 1] },
    "validated_with": {
      "repository": "HS7097/ActingCommand-Runtime",
      "commit": "<the Runtime Tools commit the bundle CI validated with>"
    }
  }
  ```

  `validated_with` records which Runtime Tools the resource CI validated the
  bundle with. acsetup logs it and never checks it. A bundle without
  `interfaces.json` is `package` [1, 1]. `bundle.json` itself is unchanged
  (`BundleIndexV3` denies unknown fields).

### What a range means

A range is `[min, max]`, two integers with `0 <= min <= max`.

- A component **reads** (accepts) every revision from `min` to `max`.
- A component that **writes** an interface writes revision `max`.
- A component that only reads an interface gives as `max` the newest revision
  it reads.
- A bundle writes `package`: its `max` is the revision its packs need. Give
  `min` the same value.

### The two checks

Each interface has one fixed form of check:

- **Containment**, for persisted data that one component writes and others read,
  possibly later and after the writer is gone: the writer's `max` lies in the
  reader's `[min, max]`.
- **Negotiation**, for a live exchange between two running programs, or a
  document that its writer produces in whichever revision its reader takes: the
  two ranges intersect, and the exchange uses the highest common revision.

Checking pairs is enough. Under containment one revision, the writer's `max`,
lies in every reader's range. Under negotiation the two parties settle on one
value.

## Vocabulary (v1)

| Interface | Check | Who writes, who reads | Rev | Meaning | Anchor |
|---|---|---|---|---|---|
| `actingd-config` | negotiation | acsetup and the console write the configuration; actingd reads it | 1 | `actingcommand.actingd.config.v1` with `vision_provider_manifest` and no `vision` section | `apps/actingd/src/config.rs`, the vision fields |
| | | | 2 | the same schema string; a `vision` section; `vision_provider_manifest` refused (#360) | same |
| `install-selection` | containment | acsetup writes `install/active.json`; actingd, actingctl, the Tools, acui and the fixed entries (acforward) read it | 1 | `actingcommand.install-selection.v1` | `crates/actingcommand-contract/src/installation.rs`, `INSTALL_SELECTION_SCHEMA` |
| `install-control` | negotiation | acsetup drives a shutdown and a start; actingd answers | 0 | cold: `actingctl request-shutdown --wait`, and a start without `--install-held` | `apps/actingctl`, `request-shutdown` |
| | | | 1 | the Host installation transition (#352): `begin_drain`, `query`, `commit_shutdown`, `--install-held`, `release` | `crates/actingcommand-contract/src/installation.rs`, `InstallTransitionAction` |
| `ledger` | containment | actingd writes the GlobalLedger; later and earlier Runtimes, the Tools and the console read it | 1 | GlobalLedger SQLite format 1, `actingcommand.event.v2`, and every record that Runtimes v0.11.0 to v0.11.2 write and their readers accept | `crates/ledger/src/global/sqlite.rs`, `FORMAL_FORMAT_VERSION`; `crates/actingcommand-contract/src/event.rs`, `GLOBAL_EVENT_SCHEMA_VERSION` |
| `runtime-client` | negotiation | the console requests; actingd answers | 1 | requests `actingcommand.runtime.request.v3`, receipts `actingcommand.runtime.receipt.v1` | `crates/actingcommand-contract/src/runtime.rs` |
| `package` | containment | a bundle writes packs; actingd runs them, and acsetup admits them before it places them | 1 | packages as the v0.11.x execution kernel admits them: `content-directory.v1` and legacy zip references; bundle index v1, v2 and v3 | `crates/actingcommand-contract/src/package.rs`, `BundleIndex` |
| `tools-layout` | derived | never declared | 1, 2 | the Tools manifest's `tools_payload_layout`: `platform-tools-v1` is 1, `platform-tools-v2` is 2 | `.github/workflows/windows-remote-build.yml` |

The console reads no package, so it is not a `package` reader. What a pack's run
records is covered by `ledger` and `runtime-client`.

## Bump rule

- A revision rises when a peer that speaks the previous revision can no longer
  work with the new behaviour: a field removed or renamed, a meaning changed, or
  a record or document that an older reader refuses. Readers verify by
  re-serialising what they read (the 09-27 ledger lesson), so an added field in a
  persisted structure counts.
- A writer that starts writing the new revision raises `max`. It raises `min`
  only when it stops reading the old revision.
- A provider raises `max` when it adds a capability that a newer consumer may
  require, for example a pack feature that a bundle needs. A consumer that
  requires the capability declares that revision.
- A change that older peers tolerate keeps the revision.
- An RT or UI pull request that touches an anchor states in its description
  whether the revision moves, and acceptance checks that statement. Each anchor
  constant carries a one-line comment pointing here.

## Parse rules

Every v1 reader applies these rules:

- `schema_version` must be exactly `actingcommand.component-interfaces.v1`.
  Anything else is an unsupported declaration and is refused.
- `speaks` is required. Each value is an array of two integers with
  `min <= max`.
- Keys other than `schema_version` and `speaks`, such as a bundle's
  `validated_with`, are logged and otherwise ignored.
- Interface names a reader does not know are logged and ignored: a reader checks
  only the edges it knows.
- A Runtime or UI declaration names all six v1 interfaces. One that lacks any of
  them is refused as an invalid declaration.
- An interface added after v1 states in this document the range a reader
  assumes for a component that does not declare it. Later readers then accept
  earlier declarations without the name. The six v1 names have no default.
- A UI manifest's `installation_selection_schema` must agree with its
  `install-selection` range: `actingcommand.install-selection.v1` requires
  revision 1 in the range.

## Releases without a declaration (the known table)

Manifests built before this document carry no `interfaces`. acsetup keeps a
built-in table, keyed by repository and full commit. An undeclared manifest that
is not in the table is refused: it declares no interfaces and is not a known
release. "—" means the component does not speak that interface.

| Component | Commit | Tag | actingd-config | install-selection | install-control | ledger | runtime-client | package | tools-layout |
|---|---|---|---|---|---|---|---|---|---|
| Runtime | `b70518949c19d56085afc3a84c49274c4cc041fe` | v0.11.0 | [1, 1] | — | [0, 0] | [1, 1] | [1, 1] | [1, 1] | 1 |
| Runtime | `732a546fb0e1c60538e53c5e52763c4e3e43c66e` | v0.11.1 | [1, 1] | [1, 1] | [0, 1] | [1, 1] | [1, 1] | [1, 1] | 1 |
| Runtime | `14b88e04fb31e89f12614192cd38657181218dcb` | v0.11.2 | [2, 2] | [1, 1] | [0, 1] | [1, 1] | [1, 1] | [1, 1] | 2 |
| UI | `b0d70e606e3df6ccb5c351b2519e240cb5db4f03` | v0.11.0 | [1, 1] | — | [0, 0] | [1, 1] | [1, 1] | [1, 1] | — |
| UI | `c47bab660b343639bbd67bd5b3a19e8f27fcafdb` | v0.11.1 | [1, 1] | [1, 1] | [0, 1] | [1, 1] | [1, 1] | [1, 1] | — |
| UI | `3f08f63978877d68f20b2c86077b1bc7c5e39a83` | v0.11.2 | [1, 2] | [1, 1] | [0, 1] | [1, 1] | [1, 1] | [1, 1] | — |

How the less obvious cells were decided:

- Runtime v0.11.1 and v0.11.2, `install-control` [0, 1]: both still answer
  `request-shutdown`. Negotiation with an acsetup that speaks 1 picks 1.
- UI v0.11.1, `actingd-config` [1, 1]: nothing shows that its acsetup handles a
  `vision` section, so the table keeps it at 1.
- UI v0.11.2, `actingd-config` [1, 2]: its acsetup writes revision 1 for a
  retained v0.11.1 slot and revision 2 otherwise.
- `package` [1, 1] throughout. The UI releases build on Runtime crates
  `d3cf973d`, whose kernel still admits only `PP-OCRv6_medium` OCR targets. A
  bundle that uses another `model_ref` (#360) needs an acsetup built on Runtime
  crates at v0.11.2 or later (Workflow #364 ruling X1).

The anchors behind every row were read at each tag: the installation selection
schema and `InstallTransitionAction` (absent at v0.11.0), the configuration's
vision fields (read at v0.11.0 and v0.11.1, refused at v0.11.2), request v3 and
receipt v1, event v2 and SQLite format 1 (unchanged), the Tools layout, the UI
manifests' `installation_selection_schema` (from v0.11.1) and the UI's cold-only
installer at v0.11.0.

## Checks per operation

acsetup applies the checks as follows. Parties:

| Party | Meaning |
|---|---|
| I | the running acsetup, by its own declaration |
| R, U | the Runtime and the UI of the release being installed |
| Rc, Uc | the programs of the selected slot |
| Rt, Ut | the programs of the retained slot that a rollback selects |
| R0, U0 | the old-layout programs that a first migration replaces |
| B | each resource bundle the run admits |

After an operation, the next Runtime R* and UI U* (the selected slot's), the
previous Runtime Rp (whose configuration and state root R* takes over), acsetup
I and the bundles B work together. The edges:

| Interface | Check |
|---|---|
| `actingd-config` | I ∩ R* (I writes R*'s configuration); U* ∩ R* (the console edits it); I ∩ Rp (I reads the current configuration) |
| `install-selection` | I.max in R* and in U* (they read what I writes); where U's acsetup becomes the fixed manager (fresh install, first migration, upgrade), also U.max in R |
| `install-control` | I ∩ Rp to close it, I ∩ R* to start it; the highest common revision is used, and 0 means the cold protocol |
| `ledger` | Rp.max in R*; Rp.max and R*.max in U* |
| `runtime-client` | U* ∩ R* |
| `package` | B.max in R* and in I |
| new slot | a Runtime laid into a new slot needs `tools-layout` 2, `actingd-config` containing 2 and `install-selection` containing 1: a slot is the program core only (#359, #360), and an earlier Runtime looks for its tools inside its slot |

| Operation | R*, U* | Rp | Bundles | New slot |
|---|---|---|---|---|
| Fresh install | R, U | none | none (the instance step checks its own) | yes |
| First migration | R, U | R0 (U0 is identified and logged) | the release's | yes |
| A/B upgrade | R, U | Rc | the release's, and a local bundle the wizard adds | yes |
| Rollback | Rt, Ut | Rc | none | no: a retained v0.11.1 slot keeps its own tools |
| Resource-only update | Rc, Uc | Rc | the one bundle | no |
| Instance step | Rc, Uc | Rc | the bundles it lays out | no |

acsetup lists every failed edge, naming the interface, both components with the
source of their ranges (declared, known release or bundle) and the ranges
themselves. It stops before the installation changes: exit code 6 on the
command line, and exit code 1 for `--rollback`, which reports through its own
entry. The existing gates stay: the selected slot's `check-config`, and
`ledger-maintenance verify` of the new Runtime against the real state root
before a switch. A declared `ledger` check is an early, explicit refusal, not a
replacement for that verification.

The fixed entries in `<root>\runtime\` and `<root>\ui\` (acforward) read
`install/active.json`. An A/B upgrade replaces them with the release's
acforward whenever their bytes differ, so they read what the release's UI reads.
