<div align="center">

**首席执行官 兼 董事长** — HS7097<br/>
**首席技术官 兼 首席架构师** — GPT‑6 Astra<br/>
**董事长顾问** — Fable 5.1<br/>
**首席技术工程师** — GPT‑5.6 Sol<br/>
**正在面试** — DeepSeek

</div>

**🌐 语言 / Language:** 简体中文 · [English](./README.en.md)

# ActingCommand Runtime

> 多游戏模拟器自动化框架的 **Rust 常驻运行时**:一个长驻 daemon 承载调度仲裁、设备控制与全局事件账本;游戏知识全部外置于声明式资源包,运行时内核**零游戏逻辑**。控制面为**净室 Rust 实现**——参照公开行为与协议重写,仓内无任何 C/C++ 源码。
>
> **设计立场:智能体在环外,运行时在环内。**智能体只做维护——规划、制作资源、处理例外;逐帧执行由运行时确定性完成,每一步入账、可审计。推理花在维护,不花在执行。

CI:[主线当前状态](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/ci.yml?query=branch%3Amain)(Windows:fmt / clippy `-D warnings` / test) · [精确 SHA Windows 构建产物](https://github.com/HS7097/ActingCommand-Runtime/actions/workflows/windows-remote-build.yml) · 许可 `AGPL-3.0-only` · 本仓公开

**当前成熟度(2026-09-05)**:调度仲裁、设备咽喉、任务收容、声明式策略目录与预算派发已接入常驻运行时;实例事实、战略求值、报告与规划信号、提案生成及 Runtime Dispatcher 会话协议已有实现。GlobalLedger 是全局唯一事件事实源,正推进为覆盖各模块的权威调试工具。**OCR 识别链已完成一轮官方 CPU 实机全流程验证**:自动回 Home 恢复、模板导航、单触分段滑动翻页、逐帧 16 目标 OCR、字典规范化比对、terminal 锚页判终、`return_home` 收口,一条命令 219 秒无人工输入完成。真实调度时间语义、长期无人值守、OCR 覆盖率与 CUDA 仍需完成对应实证。

早期的 Python mock 与 Go 历史契约、以及 Go/Python 基准工具已迁出本仓(归档于 ActingCommand-Legacy-Runtime,**暂未公开**);仓内保留 Rust 基准工具 `benchmarks/rust` 与历史基准报告。

---

## 🔁 自维护闭环

![ActingCommand 自维护闭环](./docs/assets/self-maintaining-loop.png)

这套架构存在的理由:让游戏自动化摆脱「游戏一更新,全世界等维护者发版」。目标闭环是——游戏版本更新(①)后,智能体试玩摸清变化(②),制作或修订声明式资源包(③);资源包经哈希收容进入运行时,由调度器准入(④)、确定性执行(⑤)、全程入账(⑥);失败时,智能体凭账本证据自诊断并修复资源(⑦),回到 ③。环内逐帧零推理——推理花在维护,不花在执行。

资源制作、执行与账本诊断已在实战中串联:2026-08 至 09 月初的 OCR 任务链上,资源包由智能体撰写、经账本证据诊断并修订后实机通过——含智能体辅助制作的 `return_home` 恢复包(实机验证后成为资源仓中的可复用基线)。Runtime 已具备唤醒记录、会话启动/恢复、响应与有界会话管理;外部智能体的实际自动启动、②(自动摸清)与完整自主维护闭环仍在规划中。

## 🏛 系统形态

![ActingCommand Runtime 架构图](./docs/assets/runtime-architecture.png)

图中绿色/蓝色节点及实线表示已经合入 `main` 并接入相应入口的当前能力;橙色节点及虚线表示规划、推进或待验证的能力。GlobalLedger 是全局唯一事件事实源;已有只读取证与未来权威调试能力都从这一事件源读取。源码接入不等于对应实机场景已经验证。

术语以仓内 [CONTEXT.md](./CONTEXT.md) 为准(Runtime Host / Scheduler / Execution Kernel / Device Throat / DeviceProxy 等逐条定义)。

**GlobalLedger 的诊断定位**:当前已有类型化事件、持久回执与重放,`actingledger` 提供只读取证入口。全模块探针覆盖、复发签名比对与回放考核正在推进,目标是把账本升格为权威调试工具:从同一个事件源定位正常结果、降级与失败原因,保持每条诊断与原始事实可追溯。完整诊断覆盖尚未完成。

## 📍 当前进度(2026-09-05)

| 里程碑 | 内容 |
|---|---|
| **2026-08-10** | 首次实机端到端闭环:哈希封印资源包 → 常驻运行时 → 模拟器实机页面识别 → 收容任务执行 → 类型化调度结局 → 全程入账;单次 3.4 秒,零人工输入。 |
| **2026-08-20** | 日常+周常复合领取任务链实机完成(实弹领取分支验证)。 |
| **2026-09-01** | **官方 CPU 实机 OCR 全流程 PASS**:开局非 Home 由运行时自主恢复(2 步回 Home 复验)→ 模板导航入干员页 → 41 帧单触分段滑动翻页(匀速拖动+垂直刹车,MaaTouch 点流)→ 每帧 16 目标 OCR(920 条映射记录零丢弃)→ 规范/别名/容错字典比对(294 唯一规范名,零越界)→ `operator_end` 锚页判终 → `return_home` 收口。219 秒,严格无回退,42 个投影工件逐一哈希绑定。 |

| 维度 | 状态 |
|---|---|
| **执行与资源入口已接入 `main`** | 常驻 daemon、typed loopback IPC、调度准入与租约 fencing、收容任务执行(任务超时、终止锚页、独立 `max_steps`、恢复包自动回位)、资源包收容、工件存储与官方 OCR 投影(v2,分页)、设备后端(含 `SegmentedSwipe`、MaaTouch/Minitouch 点流、MuMu Nemu IPC 动态绑定)、NCC 模板匹配与颜色判据、OCR provider 生产接线及字典约束比对。ActingLab 已串起录制、草稿、构包、事务化发布与 `package dry-run` 离线预演。 |
| **调度与维护接口已接入 `main`** | 四文档声明式策略目录、纯求值器、不可变目录版本、派发与预算已接入 `actingd`;实例事实 `PublishFact`、战略差额/容量/紧迫度求值、报告、规划信号与提案生成已有实现。Runtime Dispatcher 已有 wake/session/start/resume/response、恢复与有界配置。项目接口 v2 提供项目、实例、目录、事实、目标、决策、运行状态与诊断的只读投影及分页,可供后续 UI 查询。 |
| **当前持久化与诊断** | GlobalLedger 使用分段持久化,是全局唯一事件事实源;RuntimeState 使用 SQLite 保存运行状态与不可变发布代次,并与 Ledger 对账。`ledger-forensics` / `actingledger` 提供只读取证入口。 |
| **正在推进与待验证** | 全模块账本探针覆盖、签名匹配与回放考核;真实调度时间语义与长期无人值守实证;资源产线首批完整任务扩充;OCR 覆盖率、整页多框检测与 CUDA 实测;采集三后端矩阵(adb / droidcast_raw / nemu_ipc)覆盖。CPU OCR 已有上述单次全流程实机结果。 |
| **后续规划** | 外部智能体自动启动与完整自主维护闭环;Rust 原生只读监控台;GlobalLedger SQLite 后端与统一 RuntimeDatabase。 |

## 🗺 路线图

以下列出剩余能力与验证工作,不承诺日期:

1. **权威账本调试**——扩充各模块类型化探针,完成签名匹配与回放考核,使正常结果、降级与失败原因都能从账本定位;
2. **常驻运行实证**——在现有策略、预算、事实与战略报告能力上,完成真实时间语义、恢复与长期无人值守验证,对照实际结果评估规划信号;
3. **资源与识别**——推进首批完整任务,验证名单覆盖率,完成 provider 整页多框检测(整页识别+重叠去重)、CUDA 与采集后端矩阵验证;
4. **客户端与自主维护**——建设 Rust 原生只读监控台,接通外部智能体自动启动与现有 Dispatcher 会话接口,逐步完成自动摸清、资源修订与复验闭环;
5. **后续存储演进**——规划 GlobalLedger SQLite 后端与统一 RuntimeDatabase,保持唯一事件事实源及可恢复的状态对账;
6. **MAA / MaaFramework 兼容**——继续完善 MAA 资源种子导入与 MaaFramework 第二执行后端方向。

## ⚖ 七条结构不变量(守卫 / 测试 / 编译期与真实进程反例执法)

1. **调度器唯一仲裁写路径**:一切改变设备状态的操作先经调度器准入并持有每实例租约;fencing 五元组(epoch / lease / instance / holder / expiry)逐字段校验先于后端调用,takeover 与 epoch 换代永久作废旧牌;只读观察走 epoch 绑定的只读采集能力(非租约),同样全程入账;
2. **Runtime 唯一设备持有**:生产客户端(actingctl / runtime-client / ActingLab)的依赖图与源码均不可触达设备后端,raw adb 只存在于 Runtime 之下的 `device` crate;客户端历史设备命令一律 fail-loud 墓碑。(例外:`apps/device-test` 是直连设备的诊断二进制,不在生产链路、不受该守卫约束);
3. **GlobalLedger 唯一事实源**:唯一账本写入口是 `append(SanitizedEventDraft)`,脱敏先于持久化;终态为吸收态(重复/冲突提交被拒并留审计事实);客户端可经 `PublishFact` 提交类型化实例事实,由 Runtime 受控处理并入账,客户端不直接写账本;
4. **收容为内核资源唯一入口**:哈希校验(常量时间比较)先于解压,并有压缩体积上界预检;`LoadedBundle` capability 按构造使"未校验包被使用"不可表示——由 trybuild 编译失败用例钉死;
5. **任务不得唤起任务**:任务只产出纯数据的后继建议,自身绝不链式启动后继;生产路径遇到后继建议即 fail-loud 交还上层(`contained_task_requires_scheduler`)。由调度器裁决后继属规划中的下一步;
6. **Lab 与资源工具链可拆**:由 `--all-features` 下的依赖图守卫证明——除 Lab / ActingLab / resource-tooling 自身外,任何工作区包都不存在通向它们的依赖路径(含特性门绕过的反例用例);资源工具链亦不得反向触达 Runtime 与设备层;
7. **零游戏身份**:Runtime 自有代码、契约与默认值由架构守卫扫描,禁止出现已知项目身份词(游戏名、包名、区服后缀),该范围内测试代码一并执法;坐标与阈值只存在于资源包、不在运行时代码中——这是设计约定,不由守卫自动执法。框架只认"游戏形状"(资源池、页面、任务),不认"游戏身份"。比对**算法**在 Runtime,被比对的**值**(真值字典等)全部来自资源包,同属本不变量。

另有九条**完成体验收不变量**(确定性重放、重放零副作用、循环有预算、时钟跳变全量重算、崩溃恢复重建同一待决集、合格工作不饿死、非法输入 fail-loud、unknown 不被静默当 false、每次派发有完整理由链)覆盖调度策略面,见 `docs/architecture/runtime-completion-invariants.md`。

## 📦 组件(workspace 成员)

**应用**

| 名称 | 职责 |
|---|---|
| `actingd` | 常驻 daemon 进程适配器,承载下列全部内核组件 |
| `actingctl` | 生产用户 CLI(observe / status / monitor-* / stream / reset / task-run,支持 `--recovery-package` 自动回位);成功结果为单行 JSON |
| `actinglab` | 调试探针 + 资源制作(录制→草稿→构包→事务化发布→`package dry-run` 离线预演);**非生产依赖** |
| `device-test` | 设备后端诊断工具 |
| `vision-provider-check` | 视觉 provider 自检(ABI 校验 / artifact 锁 / OCR·NN 冒烟) |
| `actingledger` (`apps/ledger-forensics`) | GlobalLedger 只读取证 CLI |

**生产内核**

| 名称 | 职责 |
|---|---|
| `runtime-host` | 常驻所有权、本地 typed IPC、租约门控的 DeviceProxy、实例事实与策略/预算派发、战略报告及 Dispatcher 会话生命周期 |
| `runtime-client` | 客户端 typed 本地 IPC及项目接口 v2 只读分页投影;不构造也不持有生产设备后端 |
| `scheduler` | 每实例写准入、租约生命周期与 fencing 权威 |
| `execution-kernel` | daemon 持有的执行会话 + 纯任务/探针决策规划;收容任务超时、步数与终止锚页语义 |
| `ledger` | 分段持久化的全局事件账本(唯一事件事实源与权威诊断来源) |
| `artifact-store` | 工件字节、哈希、留存元数据、帧缓冲与证据归档导出 |
| `runtime-state` | SQLite 承载的 Runtime 状态与不可变发布代次,与 GlobalLedger 对账 |
| `pack-containment` | 资源包海关(开发与生产共用) |
| `device` | 设备层原语;触控经显式后端链选择(含单触分段滑动),单后端失败可见 |
| `recognition` / `recognition-pack` | 模板匹配求值 / 识别包声明词表(含 OCR 目标与真值声明) |
| `page-detector` | 页面检测(规则 + 阈值匹配) |
| `policy` | 四文档策略目录编译、纯调度求值、战略差额/容量/紧迫度计算与有界规划 |
| `actingcommand-contract` | Rust 主线契约定义(协议 / 设备 / 引擎边界词汇) |
| `host-metrics` | 平台性能计数器的安全边界 |

**识别 FFI 边界(已接入生产识别路径,CPU 实机验证)**

| 名称 | 职责 |
|---|---|
| `vision-ffi` | OCR / NN 引擎的安全 Rust 边界(原生闭包绝对路径守卫、严格无回退证明) |
| `onnx-provider-support` | 源码态 ONNXRuntime provider 的共享支撑(初始化、看门狗、会话缓存) |
| `providers/ppocr-onnx-json` | PP-OCR ROI 识别 provider(实现 OCR JSON ABI;当前为区域单行语义,整页多框为待排产能力) |
| `providers/onnxruntime-json` | ONNXRuntime NN provider(实现 NN JSON ABI) |

**开发与验证面(不进生产依赖图)**

| 名称 | 职责 |
|---|---|
| `lab` | 可选的 Lab 制作与调试适配器 |
| `resource-tooling` | 确定性资源编译与包校验(仅 Lab / CI / 密封测试) |
| `ledger-forensics` | 账本只读查询与取证,供 `actingledger` 使用 |
| `tools/actinglab-architecture` | 源码派生的架构守卫(所有权规则执法) |
| `benchmarks/rust` | Rust 基准工具 |

## 🔍 识别面现状

- **可用(实机验证)**:模板匹配(NCC 族)与颜色判据;OCR 生产链路——`PP-OCRv6_medium`(ONNX Runtime,CPU,严格无回退)、逐目标执行证明(provider/模型/设备逐次哈希证明)、字典规范/别名/容错比对与有界重试;
- **已知边界**:provider 当前为区域单行识别语义(每目标一块);名单覆盖率仍需验证,整页多框检测(det→逐框 rec)待实现与验证,目标为"整页读+重叠去重";
- **待实测**:CUDA 执行(闭包、Ready 清单、设备 ordinal/稳定身份校验机制已有实现);CPU 单次流程通过不代表 CUDA、整页识别或完整名单覆盖率通过;
- **不随仓分发**:ONNX Runtime 原生库与 OCR/NN 模型均不在本仓;由钉源验哈希的官方物化工具按任务本地缓存获取,`apps/vision-provider-check` 提供自检入口。

## 🧭 设计原则

- **游戏形状,而非游戏身份**:接入新游戏=新建一个资源仓,运行时零提交;
- **声明先于代码**:识别、导航、操作、恢复与调度策略均采用可静态校验的声明数据;
- **fail-loud**:严重错误显式失败,不返回伪成功;仅暂态错误允许有界重试并完整入账;
- **净室**:参照公开行为与协议,不复制受版权保护的实现;
- **事务化资源发布**:staging→全量验证→哈希→原子替换,失败不留混合树;
- **账本先行诊断**:出红先查全局账本;账本读不出根因的,为对应模块补探针能力,而非新造诊断工装。

## 🚀 构建与运行

当前 CI 使用 Windows 与 Rust stable,默认 Windows 产物目标为 `x86_64-pc-windows-msvc`。本地构建需 Rust/Cargo、Git 与相应 MSVC 构建环境;也可获取上述精确 SHA 构建产物。外部工具与产物校验入口见 [Windows 工具说明](./scripts/windows-tools/README.md)。

首次运行先准备 daemon 配置与至少一个实例。配置需声明 `schema_version`、`state_root`、loopback `bind_host`、16–1024 字节的 `secret_fingerprint_salt` 和非空 `instances`;设备实例需别名、`instance_id`、应用标识、ADB 寻址和显式截图/触控后端。完整字段与校验以 [配置定义](./apps/actingd/src/config.rs) 为准。设备任务另需可用的 ADB/所选后端及自备资源包;OCR 任务还需外部 provider、模型和原生库清单。策略目录说明与中性声明示例见 [调度契约](./contracts/scheduling/README.md),客户端查询契约见 [项目接口 v2](./contracts/runtime-project-interface.md)。

```bash
# 构建需能读取 git 元数据;无 .git 时须显式设置 ACTINGCOMMAND_RUNTIME_HEAD=<40 位提交哈希>
cargo build --release
cargo test --workspace

# 以下命令的产物位于 target/release/(未加入 PATH 时请带路径调用)

# 启动常驻 daemon
# 配置声明 state_root、实例别名与设备寻址、截图/触控后端(必须显式,不接受 auto)、应用标识
# 字段定义见 apps/actingd/src/config.rs;就绪时向 stdout 打印 `actingd ready pid=… host=… port=…`
actingcommand-actingd --config <actingd.json>

# 下方 <state-root> 必须与配置文件中的 state_root 为同一目录:
# 客户端从该目录读取 daemon 端点,不另行指定地址

# daemon 级状态(不接受 --instance)
actingctl status --state-root <state-root>

# 只读观察一帧(经调度器准入,事件与帧工件全部入账)
actingctl observe --state-root <state-root> --instance <alias>

# 执行一个收容任务包(哈希校验先于解压)
# --expected-sha256 为 64 位小写十六进制,不带 `sha256:` 前缀
# 可选:声明恢复包,开局不在入口页面时由运行时自主回位一次
actingctl task-run --state-root <state-root> --instance <alias> \
  --package <task.zip> --expected-sha256 <hash> \
  [--recovery-package <recovery.zip> --recovery-expected-sha256 <hash>]
```

`actingctl` 成功时向 stdout 写单行 JSON(含适用的官方 OCR 投影);参数、连接等错误向 stderr 写文本并以非零状态退出。接入方需同时处理退出码和两个输出通道。两个 CLI 均为手写参数解析,**不提供 `--help` / `--version`**。

所有 `actingctl` 命令均需 `--state-root`;当前各子命令实际使用的参数如下,以 [参数解析源码](./apps/actingctl/src/main.rs) 为准:

| 子命令 | 实例参数与命令参数 |
|---|---|
| `status` / `monitor-status` | 不接受 `--instance` |
| `observe` / `reset` / `monitor-clear` | 必需 `--instance` |
| `monitor-set` | 必需 `--instance`;可选 `--interval-ms`(默认 30000)、`--expect`(默认 `home`)、`--recover` |
| `stream` | 必需 `--instance`;可选 `--max-frames`(默认 1)、`--interval-ms`(默认 250) |
| `task-run` | 必需 `--instance`、`--package`、`--expected-sha256`;恢复参数 `--recovery-package` 与 `--recovery-expected-sha256` 必须成对提供 |

请仅使用对应子命令的参数;当前解析器接收某个已知参数并不表示该子命令会使用它。

## 🎮 资源仓

游戏数据(识别模板、导航图、操作与恢复声明)独立于运行时版本化。以下仓库**目前均为私有**,外部读者暂不可访问:

- **ActingCommand-Resources-Arknights**——上游派生层源自 MAA;自有层现有:日常+周常复合领取任务链(实机验证)、干员名单 OCR 任务包(四修版,官方实机 PASS;声明任务超时、终止锚页、16 目标 OCR 与 422 名真值字典)、`return_home` 恢复基线(实机冻结入库,可复用)、公招与全入口导航/操作集、主题检测声明(hometheme 全套)、角色/材料图鉴、识别与恢复声明、调度声明(CN 区服);
- **ActingCommand-Resources-AzurLane**——上游派生层源自 Alas;自有层现有:主界导航与全入口操作集、角色/装备全量图鉴模板(Git LFS)、识别与恢复声明;
- **ActingCommand-Resources-BlueArchive**——上游派生层源自 BAAH / BAAS(坐标目录与校验区域);自有层现有:每日领取试点任务、全入口操作集、装备/材料图鉴、识别与恢复声明。

各仓采用 `upstream-derived/`(第三方派生素材,含许可证与出处)+ `ours/`(自有声明数据)两层布局。

## 🤝 协作方式

开发通过分支与 PR 协作。评审以明确的源码版本、可观察行为与相关 CI 结果为依据;设备验证结果应说明对应后端、资源包与运行边界。公开仓提供 Runtime 与资源制作入口,复现游戏任务还需取得或自行制作对应资源。

## 约定与许可

- **净室边界**:控制面参照公开行为与协议重写,仓内无任何 C/C++ 源码;随仓分发的第三方产物仅 `external-tools/maatouch`(Apache-2.0),出处与许可见 [NOTICE.md](./NOTICE.md);
- **识别面许可边界**:OCR/NN 经 FFI 动态加载外部 provider,模型与原生库不随仓分发;
- **贡献流程**:默认经分支 + PR 合入,全部必需 CI 通过后方可合并;
- **文档同步**:`README.md` 与 `README.en.md` 必须同批修改,保持事实一致;
- 许可:**AGPL-3.0-only**。
