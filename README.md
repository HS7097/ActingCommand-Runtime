<p align="right">🌐 <b>English</b> · <a href="./README.zh-CN.md">简体中文</a></p>

<div align="center">

<img src="docs/assets/readme/actingcommand-icon.png" width="112" alt="ActingCommand icon">

**Chief Executive Officer & Chairman** — HS7097<br/>
**Chief Technology Officer & Chief Architect** — Claude Opus 5.5 · GPT‑6 Astra · Claude Fable 5.2 · Claude Fable 5.1 · Claude Fable 5 · GPT‑5.5<br/>
**Board Secretary & Chief Audit Officer** — Claude Opus 5.5 · Claude Fable 5.2 · Claude Fable 5.1 · Claude Fable 5 · Claude Opus 4.8<br/>
**Principal Engineer** — GPT‑6 Astra · Claude Opus 5.5 · GPT‑5.6 Sol · GPT‑5.5<br/>
**Director of Fleet Operations** — GPT‑6 Astra<br/>
**Fleet Supervisor** — GPT‑6 Sol Daybreak · GPT‑5.6 Sol Daybreak · DeepSeek V4.1 Flash 1017<br/>
**Business Operations Lead** — Gemini‑4 Carbon · Gemini‑4 Argon

</div>

**⚠️ Pre-release, debug phase. Every release so far is a pre-release; interfaces, configuration and file formats still change, and only the latest release is supported.**

# ActingCommand Runtime

ActingCommand Runtime is a resident Rust runtime for running multi-target automation on Android emulators and devices. The program core (Runtime, Tools and the MCP server) is neutral: it contains no logic or information about any concrete target. Everything specific to a target lives in declarative resource packs, so supporting another target means another pack, not another program. The ledger is the single source of truth, the Runtime listens only on the local loopback address and contains no network code, and every boundary fails loud.

[CI main status](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/ci.yml?query=branch%3Amain) · [Exact-SHA Windows build](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/windows-remote-build.yml) · [Releases](https://github.com/HS7097/ActingCommand-Runtime/releases) · License `AGPL-3.0-only` · [Umbrella repository](https://github.com/HS7097/ActingCommand) · [UI console](https://github.com/HS7097/ActingCommand-UI)

## Current state

Written on 2026-11-28, for the current release **v0.12.93**.

- **Release.** The current release is v0.12.93 (the 0.12 series, a pre-release), on this repository's [Releases](https://github.com/HS7097/ActingCommand-Runtime/releases) page. Every release carries two zips and `SHA256SUMS`: `actingcommand-runtime-<sha>.zip` (the daemon `actingcommand-actingd.exe`, the command line `actingctl.exe`, the configuration templates and defaults, `INSTALL.md`, `RELEASE-NOTES.md`) and `actingcommand-tools-<sha>.zip` (`actinglab.exe`, the Lab's MCP server `actinglab-mcp.exe`, the MCP entries of both generations, the ledger reader `actingledger.exe`, the vision check, the watchdog `actingwatch.exe`, and Android platform-tools 37.0.1). Vision models and ONNX Runtime are never shipped.
- **Interfaces and pairing.** v0.12.93 declares `actingd-config` [5, 35], `install-selection` [1, 1], `install-control` [0, 0], `ledger` [3, 10], `runtime-client` [12, 14], `package` [3, 29] and `runtime-outcomes` [1, 1]. Components are released independently, and whether they fit together is decided by the interface revisions each one declares, not by matching version numbers: the newest UI release, v0.12.34, fits every Runtime from v0.12.73 on, v0.12.93 included; v0.12.86 raised `ledger` to revision 10, one way, so UI v0.12.33 and earlier, which declare `ledger` [3, 9], fit Runtime releases up to v0.12.85 only. The UI, the setup wizard and the standard packs of the 0.11 series cannot be used with 0.12.
- **Scheduling.** The daemon `actingd` is resident and schedules several instances: MuMu emulators and Android handsets connected over USB. Tasks are triggered by clock slots and by instance facts; business tasks are ranked by net benefit divided by expected run time, and a task whose net benefit is unknown or not positive is not dispatched; resource pools have stop lines and hard redlines; an account can set targets on its resource pools, and a target's gap counts toward the benefit of the tasks that fill it; every candidate that was not dispatched has its reason recorded. An instance with nothing due for a while has its application closed or its emulator stopped as configured, and is woken ahead of its next run.
- **Recognition and recovery.** Eight recognition target kinds: template matching, color, color digest, click-only, OCR, ONNX neural network, composite and value comparison. Standard packs are made at 1280×720, and a larger frame of the same ratio is scaled to that base. A failed routine run hands the instance to the recovery ladder in the configuration; when the ladder runs out, an instance is fenced to wait for a person, or a run fails with an outcome code a rule lists, configured rules can wake a registered agent session. A failure that belongs to one instance fences only that instance, and the others keep running.
- **Ledger and evidence.** Every step (what was seen, where it clicked, what came back) is written to a local SQLite ledger, and every record with a result carries a registered outcome code; frames are stored by content digest and expire by class.
- **Lab and MCP.** One operation table of `actinglab` (169 operations) drives both the command line and the MCP server `actinglab-mcp` (169 tools on Windows, no tiers): recording task packs, checking and trial-running standard packs, placing standard packs, adding instances, deploying and rolling back. The first-generation `actingctl mcp-serve` (46 tools in tiers) keeps shipping with the Runtime until it is removed.
- **Installing.** Three ways: the UI repository's setup wizard `acsetup` (from an install set), `actinglab new <new root> --build <release directory>` from the Tools, or laying the files out by hand as [INSTALL.md](distribution/windows/INSTALL.md) describes. A 0.12 installation starts from an empty state root.
- **Real-device status.** On the project's own machine the Runtime of the 0.12 series schedules six MuMu instances, and routine batches run every day by catalog with four standard packs. The content coverage of the standard packs is still incomplete, and they are not published.
- **Not built yet.** Rotating several applications on one device; frames wider than the base ratio (refused today), so connecting handsets at their native resolution is still in progress; the iOS device backend (`apps/acib` is in the source and is not part of a release).

## Documents

- [REFERENCE.md](./REFERENCE.md): the detailed reference moved out of this page (architecture, how one request travels, scheduling and recovery, the evidence plane, guards, workspace members, devices and recognition, the MCP server, the build and run commands); it is not updated with every release.
- `contracts/`: the contracts, which decide the behaviour.
- [INSTALL.md](distribution/windows/INSTALL.md): installing, configuration, starting and closing, the watchdog, the upgrade boundary.
- [RELEASE-NOTES.md](distribution/windows/RELEASE-NOTES.md): what each version changed.

## Contributing

Issues are welcome. The commit identity guard accepts only commits whose author and committer are on the project's allowlist, so pull requests from other accounts cannot pass CI; please describe changes in an issue instead. Runtime reports, the ledger, mutable state and release pointers are all written under the runtime state root, never into a resource pack.

## License

`AGPL-3.0-only`. Full text in [LICENSE](./LICENSE). Third-party material in [NOTICE.md](./NOTICE.md): no source code of another automation project is in this repository. The upstream license matrix of the whole family is in the umbrella repository's [LICENSE_POLICY.md](https://github.com/HS7097/ActingCommand/blob/main/LICENSE_POLICY.md).
