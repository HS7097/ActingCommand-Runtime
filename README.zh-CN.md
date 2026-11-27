<p align="right">🌐 <a href="./README.md">English</a> · <b>简体中文</b></p>

<div align="center">

<img src="docs/assets/readme/actingcommand-icon.png" width="112" alt="ActingCommand 图标">

**首席执行官 兼 董事长** — HS7097<br/>
**首席技术官 兼 首席架构师** — Claude Opus 5.5 · GPT‑6 Astra · Claude Fable 5.2 · Claude Fable 5.1 · Claude Fable 5 · GPT‑5.5<br/>
**董事会秘书 兼 首席审计官** — Claude Opus 5.5 · Claude Fable 5.2 · Claude Fable 5.1 · Claude Fable 5 · Claude Opus 4.8<br/>
**首席技术工程师** — GPT‑6 Astra · Claude Opus 5.5 · GPT‑5.6 Sol · GPT‑5.5<br/>
**运维总监** — GPT‑6 Astra<br/>
**运维主管** — GPT‑6 Sol Daybreak · GPT‑5.6 Sol Daybreak · DeepSeek V4.1 Flash 1017<br/>
**业务运营组长** — Gemini‑4 Carbon · Gemini‑4 Argon

</div>

**⚠️ 预发布，调试阶段。到目前为止的每个发布都是预发布；接口、配置与文件格式仍会变化，只支持最新版。**

# ActingCommand Runtime

ActingCommand Runtime 是一个常驻的 Rust 运行时，用于在安卓模拟器和设备上执行多目标自动化。程序本体（Runtime、Tools 与 MCP 服务）保持中立，不含任何具体目标的逻辑或信息；一切目标专属内容都在声明式资源包里，支持另一个目标就是换一份包，程序不改。账本是唯一事实来源；Runtime 只监听本机环回地址，本身没有网络代码；所有边界都 Fail Loud。

[CI 主线状态](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/ci.yml?query=branch%3Amain) · [精确 SHA 的 Windows 构建](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/windows-remote-build.yml) · [Releases](https://github.com/HS7097/ActingCommand-Runtime/releases) · 许可 `AGPL-3.0-only` · [伞仓](https://github.com/HS7097/ActingCommand) · [UI 控制台](https://github.com/HS7097/ActingCommand-UI)

## 现状

写于 2026-11-27，对应当前发布 **v0.12.88**。

- **发布。** 当前发布是 v0.12.88（0.12 系列，预发布），在本仓 [Releases](https://github.com/HS7097/ActingCommand-Runtime/releases) 页。每个 Release 带两个 zip 与 `SHA256SUMS`：`actingcommand-runtime-<sha>.zip`（守护进程 `actingcommand-actingd.exe`、命令行 `actingctl.exe`、配置模板与默认值、`INSTALL.md`、`RELEASE-NOTES.md`）和 `actingcommand-tools-<sha>.zip`（`actinglab.exe`、Lab 的 MCP 服务 `actinglab-mcp.exe`、两代 MCP 入口、账本读取器 `actingledger.exe`、视觉检查程序、看门狗 `actingwatch.exe`，以及 Android platform-tools 37.0.1）。视觉模型与 ONNX Runtime 不随发布。
- **接口与搭配。** v0.12.88 声明 `actingd-config` [5, 35]、`install-selection` [1, 1]、`install-control` [0, 0]、`ledger` [3, 10]、`runtime-client` [12, 14]、`package` [3, 25]、`runtime-outcomes` [1, 1]。各组件各自发版，能否搭配看各自声明的接口修订，不看版本号是否一致：最新的 UI v0.12.34 可与 v0.12.73 起的 Runtime 搭配，包括 v0.12.88；v0.12.86 把 `ledger` 升到修订 10（单向），所以声明 `ledger` [3, 9] 的 UI v0.12.33 及更早版本只能配到 v0.12.85。0.11 系列的 UI、安装向导和标准包都不能用于 0.12。
- **调度。** 守护进程 `actingd` 常驻，调度多台实例：MuMu 模拟器和 USB 连接的安卓真机。任务由时钟槽和实例事实触发；业务任务按单次净收益除以预计运行时长排序，净收益未知或不为正的不派；资源池有停止线与硬红线；可以为账号的资源池设目标，目标的缺口计入对应任务的收益；每个没派出去的候选都记下原因。一段时间没有待办的实例按配置关掉应用或停掉模拟器，下一轮任务前再唤醒。
- **识别与恢复。** 八种识别目标：模板匹配、颜色、颜色摘要、仅点击、OCR、ONNX 神经网络、组合条件、数值比对。标准包按 1280×720 制作，同比例更大的画面缩到这个基准。例行任务失败后由配置里的恢复阶梯接手；阶梯用尽、实例被隔离等人处理、或运行以规则列出的结果码失败时，可以按配置唤醒已登记的智能体会话。只属于一台实例的故障只围住那一台，其余实例照常运转。
- **账本与证据。** 每一步看到了什么、点了哪里、得到什么结果都写进本机的 SQLite 账本，每条带结果的记录带一个登记过的结果码；截图按内容摘要存储，按类别到期清理。
- **Lab 与 MCP。** `actinglab` 的一张操作表（169 个操作）同时驱动命令行和 MCP 服务 `actinglab-mcp`（Windows 上 169 个工具，不分档）：录制任务包、检查与试跑标准包、放置标准包、添加实例、部署与回退。第一代的 `actingctl mcp-serve`（按权限分档的 46 个工具）在移除前继续随 Runtime 提供。
- **安装。** 三条路：UI 仓的安装向导 `acsetup`（从一份安装集安装）、Tools 里的 `actinglab new <新根> --build <发布目录>`，或按 [INSTALL.md](distribution/windows/INSTALL.md) 手动铺开。0.12 的安装从空的状态根开始。
- **实机现状。** 在项目自己的机器上，0.12 系列的 Runtime 调度六台 MuMu 实例，四个标准包每天按目录跑例行批次。标准包的内容覆盖还不完整，未公开。
- **还没做成的。** 同一台设备上多个应用的轮换调度；比基准更宽的画面（目前被拒绝），所以真机按原生分辨率接入还在做；iOS 设备后端（`apps/acib` 在源码里，不随发布）。

## 文档

- [REFERENCE.zh-CN.md](./REFERENCE.zh-CN.md)：从本页移出的详细参考（架构、一次请求怎么走、调度与恢复、证据面、守卫、Workspace 成员、设备与识别、MCP 服务、构建与运行的命令），不随每次发布更新。
- `contracts/`：合约，行为以它为准。
- [INSTALL.md](distribution/windows/INSTALL.md)：安装、配置、启动与关闭、看门狗、升级边界。
- [RELEASE-NOTES.md](distribution/windows/RELEASE-NOTES.md)：各版本的变化。

## 参与

欢迎开 issue。提交身份守卫只接受作者与提交者都在本项目白名单内的提交，因此其他账号的 PR 过不了 CI；改动建议请写在 issue 里。运行时报告、账本、可变状态与发布指针都写在运行时状态根下，绝不写进资源包。

## 许可

`AGPL-3.0-only`。完整文本见 [LICENSE](./LICENSE)。第三方材料见 [NOTICE.md](./NOTICE.md)：本仓没有其他自动化项目的源码。整个项目族的上游许可矩阵见伞仓的 [LICENSE_POLICY.md](https://github.com/HS7097/ActingCommand/blob/main/LICENSE_POLICY.md)。
