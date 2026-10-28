# Windows Runtime distribution

Version: the tag (`vX.Y.Z`) of the `HS7097/ActingCommand-Runtime` Release that
carries this zip; download the zip and `SHA256SUMS` from that Release. The
version exists only in the Release tag, not in the binaries or the manifest.
The same files taken from a pull-request or manually started Actions build are
an unreleased acceptance build with no version. The exact source commit, tree,
Cargo.lock hash, Rust toolchain, target, build profile, Actions run/attempt and
file identities are recorded in the accompanying `BUILD-MANIFEST.json`; the zip
and artifact names bind the complete source SHA.

## v0.12.25

本版是 v0.12.24 之后合并的 11 个 PR：0.12 第一波的 10 片（W1-1 到 W1-10）和第二波的第一片（P22-0）。运行中会注意到的：
玩全屏游戏或编译时，第三方压力不再让性能控制器一路升到停止派发（P22-0）；暂停期间用 `ac_emulator start` 启动的实例，
解除暂停后会跑一次启动包（W1-6），空闲动作唤醒的实例在启动包跑完之前仍记为关着，重启不再丢掉排着的启动包（W1-2）；
画面还在动、但认得出是回主页包入口页时，恢复阶梯重跑回主页，不再等满 `busy.screen.max_wait_ms`（W1-3）；每日重置不再
清掉任务自己的上一次结果（W1-4）；计划说明点名占住实例的是什么（W1-5）；Status 和概览不再等账本写入线程（W1-8）；一个
实例的会话连续两次死掉只扣住这一个实例（W1-7）；S2 自动回退先判回退闸门、再关新 Runtime（W1-1）。升级后的第一次启动
会记一条 `invalid_runtime_state_fact`，这是预期的（见下面的“升级时要知道的”）。v0.12.24 已在本版之前发布。

配置：没有新键，也没有新的 revision，`actingd-config` 仍是 [5, 19]，`runtime-client` 仍是 [2, 2]，账本仍是 [3, 4]；
没有改任何默认值。

- `config.subsystems` 多了常开的一行 `reset_exclusions`（W1-4）：这是已有事实的新行值，不是配置键。`schedule.json` 的
  时间线事件写了 `excludes_fact_prefixes` 时，`requires` 必须列出它。
- `PerformanceStatus` 的答复新增可选成员 `ledger_read_pages`（W1-8），仍在 `runtime-client` revision 2 之内，不升
  revision（配对见下面的回退）。没有新请求类型，也没有新请求成员。
- 新出现的限值都是程序里的固定值，不是配置键：手动运行 `actingd suspended` 时默认 60 秒截止，找 daemon 启动事实最多
  读 32 768 行（W1-9）。
- 现有的配置都不需要修改。

升级时要知道的：

- 空闲状态记录（W1-2）：本版读不了 v0.12.24 写的 `host.idle_state`（v0.12.24 的写入者行是 `owner_epoch`，本版是启动
  锚点 `start_anchor`）。账本里有这条记录时，升级后的第一次启动记一条 `invalid_runtime_state_fact`（操作
  `restore_idle_state`），是 Runtime 范围的记录，不致命，启动继续。这次启动不知道空闲动作关了哪些游戏：每个空闲动作
  不是 `none`、配置了启动包（`stop_emulator` 还要配置了模拟器启动）的实例按“已关闭、唤醒时间就是启动时刻”恢复，第一次
  空闲检查唤醒它一次；它的启动包排在部署的保持后面，`release-hold` 之后跑一次，下一次计划再按正常的唤醒时间关上。
  这是预期的，不要因为这条记录或多跑的这次启动包回退。退回 v0.12.24 时反方向也一样发生一次（见下）。
- 重置排除（W1-4）：声明了 `requires: reset_exclusions`（或写了 `excludes_fact_prefixes`）的 `schedule.json`，v0.12.24
  拒绝。只在本版运行起来之后才加这样的声明；加了之后，S2 回退到 v0.12.24 时，v0.12.24 拒绝那个游戏的文件。另外，规则
  对自己的 `dependency_completed`（例如对自己 `failed` 的重试分支）现在跨过重置读到上一轮的结果；只该在本轮触发的分支
  要以同一轮更早一步的 `dependency_completed` 作本轮闸门（已有的这类分支在本版合入之前已在资源侧加上）。
- 止损指令（W1-3）：本版运行之后，止损用的运行时指令 `recovery_ladder.busy.screen.max_wait_ms=120000` 不再需要。清掉
  它是运维步骤，不在发布里：`actingctl config clear recovery_ladder.busy.screen.max_wait_ms`（或 `ac_config_set` 只给
  这个键、不给 `value`），回到默认的 900 000。

退回 v0.12.24 之前要做的事（按代码核对过）：

- 配置和运行时指令：本版没有新键，也没有改默认值，v0.12.24 照样读本版的配置文件，回退启动的 `instructions` 检查不会
  因本版失败。清掉了上面的止损指令之后再回退的，v0.12.24 没有入口页比较，画面一直在动时又会等满
  `busy.screen.max_wait_ms`；需要时回退后再设一次这条指令。
- `schedule.json`：已经加了 `requires: reset_exclusions` 的游戏，回退前先把声明和 `excludes_fact_prefixes` 从文件里拿掉，
  否则 v0.12.24 拒绝那个游戏的文件。没有加的不用做什么；v0.12.24 的重置照旧清掉任务自己的上一次结果。
- 空闲状态记录：不用做什么。v0.12.24 读不了本版写的 `start_anchor`，回退后的第一次启动同样记一条
  `invalid_runtime_state_fact`（操作 `restore_idle_state`），每个上面那样的实例唤醒一次、启动包跑一次。
- MCP 入口的状态文件：不用做什么。换上本版的 `actingcommand-mcp-entry.exe` 之后，入口在状态文件里多写一个成员
  `process_created_at_unix_ms`。v0.12.24 的 `actingctl mcp-entry list` 把状态文件当一般 JSON 读、只取 `pid`，不认识的
  成员忽略，照常列出，只是又只按 pid 判断入口是否还活着。入口和子进程之间的约定仍是 `g1`，本版的入口照样切换到
  v0.12.24 的子进程。
- 账本：不用做什么。账本的格式、表、索引和事件类型都没变，v0.12.24 照样打开本版写过的账本。本版不再为只有一条
  `client.action` 的作用域写 `prior_epoch_scope_closed`（W1-10），回退后 v0.12.24 的第一次启动照旧为上一个纪元的这类
  作用域补写关闭，只是多写一些记录。会话死亡的计数（W1-7）和淘汰证明集（W1-8）只在内存里。
- 结果码：退役的 `recent_runs_budget_invalid` 只由客户端的 `recent_runs` 答出，从不写进账本；v0.12.24 里它仍是有效的码，
  不用做什么。本版没有新结果码。
- 客户端和 Runtime 的配对：
  - 本版的客户端连 v0.12.24 的 Runtime：照常。本版没有新请求类型或请求成员；v0.12.24 的 `PerformanceStatus` 答复没有
    `ledger_read_pages`，本版客户端按没有这个可选成员读。
  - v0.12.24 的客户端连本版的 Runtime：除了 `PerformanceStatus` 都照常。v0.12.24 的 `PerformanceStatus` 拒绝不认识的
    成员，而本版 Runtime 的账本有读者池，答复带 `ledger_read_pages`（读者池没能启动时才没有），所以 v0.12.24 的客户端解码失败
    （`runtime_receipt_decode_failed`）并锁住这次连接：`actingctl perf-status` 报错；`ac_overview` 没有 `performance` 块，
    带这条警告、标 `incomplete: true`，这次调用里在它之后读的 `fact_refresh` 也失败，下一次调用重新连接。这只发生在
    升级时 MCP 入口还没切到本版子进程之前，以及直接启动、还没重启的 `actingctl mcp-serve`；命令行用根目录的
    `runtime\actingctl.exe`，部署之后就是本版。回退方向没有这个问题。
- 回退本身（W1-1）：S2 的自动回退先判回退闸门。闸门不过时以 `rollback_gate_failed` 结束，什么都不关：新构建、正在运行
  的新 Runtime 和保持都留着，由人决定（`next` 指向 `release-hold`）。

运行上的变化：

- 回退闸门（W1-1）：S2 自动回退在关新 Runtime 之前判闸门，闸门不过时新 Runtime 继续运行并保持住。
- 暂停中的启动（W1-6）：暂停期间 `ac_emulator start` / `restart` 也把启动包排在暂停后面，解除暂停（先重连）后跑一次；
  答复是 `startup_package: scheduled`，`startup_package_withheld` 点名挡住它的暂停。
- 重置（W1-4）：任务自己的 `dependency_completed`、`since_own_run` 和 `once_per_reading` 读到重置之前的上一次结果；
  `excludes_fact_prefixes` 列出的前缀不被清掉；时钟槽可能多记一些 `clock_slot_superseded` / `clock_slot_expired`（只是诊断）。
- 计划说明（W1-5）：实例被占住时，等未知键的候选在 `trigger_unknown` 或 `feedback_stop_unknown` 之后带上占住它的原因和唤醒（`dispatch_instance_held`、
  `dispatch_paused_*`、`dispatch_instance_cooldown` 或时段黑名单）；占住期间不读，解除后的第一轮再读。
- 入口页复查（W1-3）：保持之后的复查里画面还在动，最后一帧是回主页包声明的入口页时先续租、再重跑回主页；认不出的画面
  照旧保持，入口是 `any` 的包不比较，比较做不成的记一条 `runtime.failed`、照旧保持。
- 诊断（W1-9）：runtime-client 公开的 `recent_runs` 退役；`actingd suspended` 在截止时间内读每个显示的运行，来不及的
  留空并标 `ledger_read_incomplete`，找不到 daemon 启动时答 `daemon_start_unknown`、其余照常。
- 性能控制器（P22-0）：只有宿主响应度让旧控制器升级，第三方压力和全屏前台单独或一起都不再升级，所以单靠压力不会到
  Suspended，不推迟派发、不拒租约、不降重场景名额。
- 启动和 MCP 入口（W1-10）：启动不再为客户端动作的作用域写关闭；`mcp-entry list` 按 pid 和进程创建时间判断入口
  是否活着（旧入口的状态文件没有创建时间，按 `started_at_unix_ms` 判断），判断不了的行标 `stale: null` 和
  `probe_failed`、答复带 `process_probe_failed` 警告，命令照常答复；更新的 `notice.mcp_clients_reconnect` 不再算入口自己的子进程；推迟中的
  切换遇到安装又变时放弃，稳定后重新计划。
- 实例隔离（W1-7）：会话连续两次死掉（中间没有一次截图成功）只隔离这个实例，其他实例照跑；E45、E48 只让这次操作
  失败；输入因运行的截止时间、取消或租约到期被挡下时，是这次操作自己的不致命失败。
- 账本读取（W1-8）：Status、MonitorStatus 和事实快照读写入线程已发布的头，不再等写入线程（事实快照仍等事实写入闸）；
  `actingctl perf-status` 和 `ac_overview` 的 `performance` 块显示 `ledger_read_pages {pages, total_ms, longest_ms, last_ms}`。
- 空闲记录（W1-2）：唤醒之后、启动包跑完之前实例记为 `woken`、仍算关着；`stop_emulator` 的唤醒发现模拟器已在运行时
  也排一次启动包；中间的离线账本维护或部署的账本检查不再让记录作废；`finished` 两端都失败时两个错误都报。

本版合并的 PR（最后 4 个 #1403、#1404、#1401、#1402 经集成 PR #1406 一起合入，#1406 本身不加内容）：

- #1395 W1-1（#421 P12）：S2 自动回退在关新 Runtime 之前判回退闸门，不过时以 `rollback_gate_failed` 结束，新 Runtime 继续运行并保持住。
- #1396 W1-6（#411 A71）：暂停期间 `ac_emulator start` 排上的启动包在解除暂停后跑一次，不再一直扣着。
- #1397 W1-4（#411 P19）：每日重置保留任务自己的上一次结果；重置可以声明不清的前缀（`excludes_fact_prefixes`，要 `requires: reset_exclusions`）。
- #1398 W1-5（#411 裁定 77 A）：计划说明点名占住实例的是什么（占用者、暂停、冷却、时段黑名单）。
- #1399 W1-3（#411 A56）：阶梯复查时画面显示回主页包的入口页之一就先续租、再重跑回主页。
- #1400 W1-9：runtime-client 公开的 `recent_runs` 退役（`recent_runs_budget_invalid` 一起退役）；`actingd suspended` 在截止时间内读，手动运行有默认截止时间和行数上限。
- #1405 P22-0（#420）：第三方压力和全屏前台单独不再让性能控制器升级。
- #1403 W1-10（#411 P51、P49）：只读的客户端动作作用域在下次启动时不再写关闭；`mcp-entry list` 比较 pid 和进程创建时间；判断失败的行标出来。
- #1404 W1-7（#419 ISO-2b）：会话连续两次死掉只扣住那个实例；E45、E48、E08 只让操作失败；会话死亡的码移出“总是严重”的表。
- #1401 W1-8（#422）：共享的淘汰证明集；`PerformanceStatus` 新增 `ledger_read_pages`；Status、Monitor 和事实快照读已发布的头。
- #1402 W1-2（#405 P14–P16）：空闲记录保持“已关闭、已唤醒”直到启动包跑完；绑在启动锚点上；两端的错误都报。

## v0.12.24

本版是 v0.12.23 之后合并的 1 个 PR（#1393 QR-2b，#422）：`ac_diagnose`（和 `actingctl diagnose`）重新能在调用的
预算内答复；有哪个部分来不及读完或读取失败，就在答复里点名，其余部分照常答复。v0.12.23 已在本版之前发布。

配置：没有新键，也没有新的 revision，`actingd-config` 仍是 [5, 19]，`runtime-client` 仍是 [2, 2]，账本仍是 [3, 4]。

- 改动都在客户端一侧（actingctl 和 MCP 的诊断，以及 runtime-client 新增的一次性列表读取）；契约、账本和 actingd
  答复请求的代码都没变。诊断的每一页带的 `without_run_recovery` 是 v0.12.23 已有的页请求成员（QR-2），本版没有新
  请求类型，也没有新请求成员。
- 诊断的截止时间不是配置键：MCP 用调用预算减 1 秒，命令行仍是原来的 24 秒，也没有新增命令行参数。
- 现有的配置都不需要修改。退回 v0.12.23 之前要做的事（按代码核对过）：什么都不用做。
  - 配置和运行时指令：本版没有新键，也没有改任何默认值，回退启动的 `instructions` 检查不会因本版失败。
  - 账本：不需要做什么。诊断只读（QueryEvents 不写记录），本版的改动不写任何文件或状态；账本的格式、表、索引和
    事件类型都没变。没有新结果码：`call_budget_exhausted` 是已有的码，只补充了说明，结果码发布文件也是空的。
  - 客户端要和 Runtime 配对吗：不用。本版客户端发出的请求成员和 v0.12.23 相同，所以本版的客户端（例如回退后
    还没切换子进程的 MCP 前端）连 v0.12.23 的 Runtime 照常答复，诊断也一样；v0.12.23 的客户端连本版 Runtime
    也照常。回退后 MCP 入口切回 v0.12.23 的子进程，切换前后都不会因为配对报错。

运行上的变化：

- 只列一次（QR-2b）：一次诊断只列一遍窗口里的运行（5 种列表类型各一次查询，同一个快照），`runs` 和
  `runs_grouped` 都从这一次列表取运行。以前要把整个窗口（包括入口事实）列两遍。
- 不读已结算的运行：列表里只有一个终结记录、而且是 `task.completed` 或 `task.cancelled`，主准入
  （`task.requested`）也在窗口里的运行，不再读它的状态；它既不是失败也不是未结束，只在“最近 10 个运行”里占位，
  不会显示出来。其他运行在最近 10 个还没凑齐时都读状态，凑齐之后只读 `runs_grouped` 要留下的。计划意图页只为
  `runs_grouped` 留下的、计划发起的失败或未结束运行读；转发终结记录只在转发码没有在目录里登记时读。
- 不带运行恢复：错误页、失败记录页和转发终结页现在也带 `without_run_recovery`（列表页和状态页从 v0.12.23 起
  就带），诊断的每一页都不再为运行读上下文。错误页的游标仍可以在 `ac_events` 里接着读。
- 截止时间：每个部分只在调用的截止时间之前开始读，顺序是错误页、`failures_grouped`、`not_run`，然后才是运行列表、
  `runs` 和 `runs_grouped`，这样随窗口变长的列表不会把别的部分挤掉。诊断里原来 `recent_runs` 自带的 20 秒预算
  去掉了。截止时间到时正在进行的读取不会在那一刻被切断：除了在途的那一页，列表还会发出它没读到的列表类型的第一页
  （最多 4 页），或者一次运行状态读取还会发出 Runtime 重启页（最多 4 页）和租约页（最多 2 页）；每页最长到客户端
  的 5 秒套接字超时为止。
- 新成员和警告：截止时间切断的部分，或读取失败的部分，答复 `<section>_incomplete: true`，共四个：
  `runs_incomplete`、`errors_page_incomplete`、`runs_grouped_incomplete`、`failures_grouped_incomplete`（MCP 的
  输出 schema 加了这四个布尔成员），并带警告：被截止时间切断的是 `call_budget_exhausted`，`member` 是这个部分的名字；
  读取失败的是那次读取的失败。整个诊断标 `incomplete: true`，其他部分照常答复。没读到的错误页没有行。`not_run`
  来不及读时不出现，带 `call_budget_exhausted`（`member` 为 `not_run`）。只有带 `instance` 时用来解析实例的那次
  Status 读取失败，才会让整个诊断失败。
- 带实例时，列表被截止时间切断就不给 `runs`（以前 `recent_runs` 会读部分列表里的运行），和 `ac_runs` 的列表一样：
  部分列表可能漏掉更新的运行。
- 不带实例的调用仍受那一次完整列表限制：它要列出所有实例在窗口里的全部列表事实（包括入口事实），窗口越长、运行
  越多，这一读越久；要再快得靠 Runtime 端的运行汇总，本版没有做。列表被截止时间切断时，`runs_grouped`（最近 10 个
  还没凑齐时还有 `runs`）标 `_incomplete`，错误页、`failures_grouped` 和 `not_run` 在它之前已经读完。
- 用时：v0.12.21 上实测 `ac_diagnose` 用了 27 秒，没有答复。本版按评审对生产规模账本的估算，每次调用读的页数：
  不带实例约 65 到 185 页（以前 700 到 1 200 页），带实例约 20 到 55 页（以前 170 到 290 页）；还没有在本版上实测。
- `ac_events`、`ac_runs`、`ac_get_run` 和概览不变。

本版合并的 PR：

- #1393 QR-2b（#422）：`ac_diagnose` / `actingctl diagnose` 每次调用只列一次窗口里的运行，`runs` 和 `runs_grouped` 共用；列表显示只有一个 `task.completed` 或 `task.cancelled` 终结的运行不读状态；计划意图页只为 `runs_grouped` 留下的计划发起的失败或未结束运行读；每一页都带 `without_run_recovery`；每个部分只在调用的截止时间之前读，被切断的部分答复 `<section>_incomplete: true` 并带 `call_budget_exhausted`（`member` 为该部分），其余部分照常答复，只有解析实例的 Status 读取能让整个诊断失败；MCP 输出 schema 加四个 `_incomplete` 成员；命令行保持 24 秒，MCP 用调用预算减 1 秒；修正 1：测试 T4 的截止时间改为确定的、窗口起点按主准入计算、补文档说明。没有新键、新结果码，不升 revision。

## v0.12.23

本版是 v0.12.22 之后合并的 1 个 PR（#1391 QR-2，#422）：概览、`ac_runs` 和 `ac_get_run` 读账本时不再做没有
调用方用到的按运行的工作，所以几秒内就能答复。v0.12.22 已在本版之前发布。

配置：没有新键，也没有新的 revision，`actingd-config` 仍是 [5, 19]，`runtime-client` 仍是 [2, 2]，账本仍是 [3, 4]。

- `QueryEvents` 的页请求新增可选成员 `without_run_recovery`（见下），仍在 `runtime-client` revision 2 之内，不升
  revision：不写时等于 `false`，请求的序列化和页的答复都和以前一样，所以 v0.12.22 的客户端发给本版 Runtime 的
  请求照旧答复。
- 读线程的备忘（最近 4 个快照的 Lab 关系、最近 4 个窗口起点）的个数是程序里的固定值，不是配置键。
- 现有的配置都不需要修改。退回 v0.12.22 之前要做的事（按代码核对过）：
  - 配置和运行时指令：不需要做什么。本版没有新键，也没有改任何默认值，v0.12.22 照样读本版的配置文件，回退启动
    的 `instructions` 检查不会因本版失败。
  - 账本：不需要做什么。备忘只在每个读线程的内存里，从不写进账本，读线程结束就没有了；账本的格式、表、索引和
    事件类型都没变，也没有新结果码，v0.12.22 照样打开本版写过的账本。
  - 客户端要和 Runtime 配对：v0.12.22 的页请求拒绝不认识的成员。本版的客户端（actingctl 和 MCP）在列运行和读运行
    状态时带上 `without_run_recovery`，v0.12.22 的 Runtime 解码失败就断开连接（`runtime_request_decode_failed`），
    客户端读答复失败并锁住这次连接，这次调用里之后的读取也都失败：概览、`ac_runs`、`ac_get_run`、
    `ac_run_evidence`、`ac_diagnose` 这类要列运行或读运行状态的调用会报错（不会当作成功）。没有兼容代码（测试
    阶段不做旧版兼容）。所以回退之后要用 v0.12.22 的客户端：
    - 经 MCP 入口（`actingcommand-mcp-entry.exe`）连接的客户端：回退（`install rollback apply`，或组合更新的自动
      回退）换回 `runtime\actingctl.exe` 和 `runtime\BUILD-MANIFEST.json` 之后，入口在变化保持
      `install.mcp_entry.settle_ms`（默认 10 秒）不动时询问计划；安装完整（写锁空闲、没有未完成的安装操作、文件
      与清单一致）时，在两次调用之间切到 v0.12.22 的子进程，不用重启客户端。在切换之前（包括子进程还有未读结果的
      句柄、切换推迟的时候），仍是本版的子进程在答，上面那类调用会失败；切换之后恢复。`actingctl mcp-entry list`
      显示每个入口当前子进程的构建。
    - 直接启动 `actingctl mcp-serve` 的客户端：进程继续用被改名留下的本版程序（`install status` 的
      `mcp_servers_on_old_images` 列出它们），要重启这个 MCP 服务才换成 v0.12.22。
    - 命令行：`overview`、`runs` 等用根目录自己的 `runtime\actingctl.exe`（回退后是 v0.12.22）。安装器（从最新
      留存的构建运行）的停、启和启动后检查不列运行、不读运行状态，不受影响。

运行上的变化：

- 不带运行恢复的页（QR-2）：概览的运行块、`ac_runs`、`ac_get_run`（按请求和按运行）、`ac_run_evidence`，以及
  `actingctl overview`、`runs` 等列运行和读运行状态的地方，读的每一页都带 `without_run_recovery`。这样的页不再为
  每个运行读取并认证它的上下文（每个运行最多 1 025 条记录），只是少了这些调用从不读的 `run_recovery` 组；行、视图、
  游标和读取范围都和以前相同（响应字节上限不再计入组，所以一页可能多装几行）。
- 读线程的备忘（QR-2）：读者池的每个读线程在两页之间记住：
  - 一页快照上的 Lab 关系，最多 4 个快照，最久没用的先丢；同一快照的后续页不再重读，换到它已不记得的快照时重读。
  - 窗口起点（`from_timestamp_unix_ms` 处的第一条记录），最多 4 个。账本事件只增不改，所以找到的第一条在之后的
    快照上仍然准确，跨快照保留；没找到的只在它那个快照上记。
  - 读取预算没有截断的页，有没有备忘结果都一样；被预算截断的页因省下的读取可能走得更远。
- 用时：v0.12.21 上实测概览的运行块每个实例 5 到 28 秒；本版预计约 0.5 到 1.5 秒（还没有在本版上实测）。
- `ac_diagnose`：它列最近运行和运行列表的部分同样走上面的新路径；它自己读的错误等事件页还没有改，仍为每个运行
  读上下文，留给后续的 QR-2b。`ac_events` 等其他事件查询不变。

本版合并的 PR：

- #1391 QR-2（#422）：`QueryEvents` 的页请求新增可选成员 `without_run_recovery`（不写即和以前相同），带它的页不为每个运行读上下文、没有 `run_recovery` 组；读者池的每个读线程按快照记住 Lab 关系（最多 4 个）和找到的窗口起点（最多 4 个）；客户端列运行和读运行状态（概览的运行块、`ac_runs`、`ac_get_run`）的页都带这个成员；修正 1：备忘保留多个快照、补 Lab 锚点和两个快照交替的测试、契约写明 v0.12.21 和 v0.12.22 的 Runtime 收到这个成员时断开连接。没有新键、新结果码，`runtime-client` 不升 revision。

## v0.12.22

本版是 v0.12.21 之后合并的 3 个 PR，都属于按实例并行调度（Workflow #427）：调度的一轮可以把计划运行交给各实例
自己的工作线程，不同实例同时运行（#427-A）；同一宿主上的实例之间只共享真正共享的上限（#427-B 及其后续）。并行
运行由开关 `scheduling.parallel.enabled` 控制，默认关闭。开关关着时一切照旧，只有两处变化：快照变了时有界地重新
取样（C1），以及每轮不再按 CPU、GPU、IO 预扣宿主预算。v0.12.21 已在本版之前发布。

配置：`actingd-config` 升为 revision 19，本版声明 [5, 19]，仍读 revision 5 到 18 的配置文件；其他接口不变。

- 新键 `scheduling.parallel.enabled`（#427-A）：布尔，默认 `false`（在 `actingd.defaults.json` 里），可在运行时设置。
  actingd 在每一轮调度开始时读它，启动后的第一轮也读，所以运行时的指令从下一轮起生效；已经交给实例工作线程的运行
  照样在那里跑完并结算。
  - 打开：`actingctl config set scheduling.parallel.enabled=true`，或 MCP `ac_config_set`
    `{"changes": [{"key": "scheduling.parallel.enabled", "value": true}]}`。
  - 关闭：`actingctl config clear scheduling.parallel.enabled`（或 `ac_config_set` 只给这个键、不给 `value`），回到
    默认的 `false`，不留指令。设成 `false` 也能关，但会留下一条指令（见下面的回退）。
  - 也可以写进配置文件（经 `acsetup --commit-config`，下次启动生效）。
- 新键 `performance.pacing.max_concurrent_runs`（#427-B）：整数 1 到 65 535，没有默认值，不可在运行时设置，启动时读
  一次。配置文件不写它就没有这个上限，`config.parameters` 里也没有这一行；写了，所有实例合计已准入或正在运行的策略
  派发，加上本轮更早选中的，最多这么多个。`1` 表示所有实例合计一次只跑一个策略派发。改它要改配置文件并重启。
- 其他键和默认值都不变，现有配置不需要修改。`performance.pacing.heavy_dispatch_limit.normal` / `.paused`（默认 2 / 1）
  现在是同一宿主上重场景的共享名额（见下）。
- 退回 v0.12.21 之前要做的事（按代码核对过）：
  - 配置文件：v0.12.21 只认 [5, 18]，`scheduling` 节和 `performance.pacing` 都拒绝不认识的成员。配置文件只要写了
    `scheduling.parallel`（哪怕是 `false`）或 `performance.pacing.max_concurrent_runs`，v0.12.21 的 `check-config` 和
    启动都会拒绝整个文件。回退前先把这两个键从配置文件里删掉并提交；两个都没写的文件 v0.12.21 照常读。
  - 运行时指令：只要用 `config set` 或 `ac_config_set` 设过 `scheduling.parallel.enabled`（`true` 和 `false` 都算），
    这条指令就记在 `host.config_overrides` 里。v0.12.21 不认识这个键，启动时把它作为 `config_override_dropped` 丢掉
    （只是警告，启动照常），并列进 `instructions_dropped`，所以回退启动的 `instructions` 检查不通过。回退前先执行
    `actingctl config clear scheduling.parallel.enabled`，这也同时关掉并行运行。`max_concurrent_runs` 不可设置，不会
    有指令。
  - 结果码：不需要做什么。退役的 `host_budget_deferred` 在 v0.12.21 里仍在用，回退后每轮重新按 CPU、GPU、IO 预扣
    并给出它。本版写的派发决定记录里，`decision_candidate_not_selected` 的 `why` 和 `omitted_why` 可能写有 v0.12.21
    不认识的 `policy_concurrent_runs_deferred`，重场景原因也多了 4 个值；结果里不认识的码和值照样可读。账本接口仍是
    [3, 4]，没有新事件类型，表和索引都没变。
  - 账本里的状态：不需要做什么。在途登记只在内存里，从不写进账本；交给工作线程的运行写的记录和调度线程自己跑时
    一样；停止时被拒绝的运行记的是普通的 `policy_settlement_interrupted` 结算，v0.12.21 只按结果码和连续次数决定是否
    解除，和启动时记下的中断一样；被放弃的运行在线结算时写的是启动时的形状（租约已结束的不再写释放）。

运行上的变化，开关关着（默认）时：

- 重新取样（C1，不受开关控制）：一轮评估在取样和校验之间发现采样的结果键变了（`policy_outcome_snapshot_changed`：
  另一个运行在这之间结算了），以前这个错误会结束策略驱动线程（随后报 `policy_driver_stopped`）。现在这一轮重新取样
  再评估，每轮最多 3 次，每次打印 `actingd policy_cycle_resampled code=… attempt=<n> limit=3`；3 次之后仍变就放弃这一
  轮，打印 `actingd policy_cycle_given_up code=… resamples=3`，推迟到下一次触发（一个去抖间隔之后）。驱动线程不会因此
  停下。
- 不再预扣（#427-B）：每轮不再把选中派发的 CPU、GPU、IO 负载从宿主预算里扣掉，也没有按实例的预算池，
  `host_budget_deferred` 退役，不再出现。任务自身的负载超过宿主容量的，仍在放置阶段以 `host_capacity_insufficient`
  挡住。所以一轮可以在同一宿主的多个实例上各选一个派发（例如五个普通负载的实例一轮全部选中）；开关关着时，这些
  派发仍由调度线程在这一轮里一个接一个地执行。
- 重场景名额：重场景候选（按 `performance.load.heavy_threshold_milli` 判断）占宿主的一个名额，名额数是
  `heavy_dispatch_limit.normal`，性能控制器高于 Normal 时是 `.paused`。占用的名额 = 已准入、尚未结算的重场景策略派发
  （任何实例）+ 本轮更早选中的重场景派发，每个派发只算一次；准入被拒的和已结算的派发都释放名额。已准入的派发就占
  满名额时，候选在放置阶段以 `heavy_scene_budget_exhausted` 挡住；本轮更早的选择占满时，以
  `heavy_scene_budget_deferred` 推迟。`max_concurrent_runs` 写了时先查它，到上限的候选以新码
  `policy_concurrent_runs_deferred` 推迟。这些上限与 `performance.pacing.mode` 无关，`observe`（发布默认）、`pace`、
  `full` 下一样生效。
- 带数值的原因：这三个原因都带 `used`（占用）、`limit_count`（上限）、`admitted_count`（已准入或正在运行的占用）和
  `round_selected_count`（本轮更早选中的占用），例如重场景 2/2、并发运行 3/3。压力下 `heavy_scene_budget_exhausted`
  的 `used` 可以大于 `limit_count`。`ac_schedule` 和 `actingctl schedule`、`ac_overview` 的 `not_running`、
  `ac_diagnose` 的 `not_run` 都原样显示这些值；`ac_diagnose` 的 `not_run` 除了 `deferred` 的行，现在也列出因这三个
  共享上限之一而 `blocked` 的行（#1389）。

开关打开后：

- 实例通道：每个获准的计划运行连同它的请求交给它所在实例的工作线程（`actingd-instance-<别名>`），由它运行、结算，
  再唤醒调度线程。同一实例一次一个，不同实例同时运行。一轮不再等运行结束，启动后的第一轮也不再推迟
  `runtime_ready`。评估和准入仍只在调度线程上。
- 在途登记：从交出到结算为止，运行登记在它的实例上。期间这个实例算作被占用：评估推迟它的候选，准入以
  `dispatch_instance_held`（`holder_kind` `policy_dispatch`）拒绝新派发，终止记录已写、租约已释放但还没结算的那段
  时间也一样；对账跳过工作线程还持有的运行，不会提前或重复结算。
- 排空：实例的调度暂停和安装排空都等在途运行。在暂停或排空开始之前交出的运行照常进行（排空等它，暂停像对待之前
  准入的运行一样排空它），不为此记失败。只有在 Runtime 的停止已经请求之后工作线程才取到的运行被拒绝
  （`runtime_stopping`）：不启动，释放租约，记一次实时的中断结算（`policy_settlement_interrupted`，带实际经过的
  `runtime_ms`），重启后像启动时记下的中断一样解除（连续第一次）。关闭时，工作线程已取到的运行跑完并结算，还在队列
  里的被拒绝并按中断结算，什么都不重放。
- 故障（#419）：运行的故障走该实例的故障隔离，实例级故障只隔离那一个实例，其他实例照跑；只有全局故障才停
  Runtime。工作线程崩溃（panic）时这个运行被丢弃、不重试，租约到期失效；登记保留并标为放弃，实例在租约有效期间
  保持占用。租约结束后，对账按启动时的规则在线结算它一次（有终止记录就按终止记录，否则从租约结束的时刻按中断
  结算），然后忘掉登记，实例重新可用。租约以不止一种方式结束的仍保持占用，留给下次启动记录。
- 共享名额：各实例同时运行时，重场景名额按所有实例已准入未结算的重场景派发计数，按默认值同一宿主最多同时 2 个重
  场景（压力下 1 个）；写了 `max_concurrent_runs` 时，所有实例合计的派发数不超过它。
- 回落：并行运行出问题时，最快的是关掉开关（`config clear`，下一轮起生效）。要保留开关、但所有实例合计一次只跑一个
  派发，在配置文件里写 `performance.pacing.max_concurrent_runs` 为 `1` 并重启。

本版合并的 PR：

- #1387 #427-B（#427）：每轮不再按 CPU、GPU、IO 预扣宿主预算，`host_budget_deferred` 退役，任务自身负载超过宿主容量仍在放置阶段挡住；重场景名额按已准入未结算加本轮已选中计数；新的可选键 `performance.pacing.max_concurrent_runs`（不写即不限，启动时读）和新码 `policy_concurrent_runs_deferred`；三个共享上限的原因带 `used`、`limit_count`、`admitted_count`、`round_selected_count`。
- #1386 #427-A（#427）：开关 `scheduling.parallel.enabled`（默认关，可在运行时设置，每轮读）打开时，计划运行交给各实例的工作线程；在途登记从交出到结算，供占用、对账、调度暂停和安装排空使用；C1 有界重新取样（不受开关控制）；开关打开时启动的第一轮不再推迟 `runtime_ready`；工作线程崩溃后放弃的运行在租约结束后在线按中断结算，停止时被拒绝的运行按中断结算；`actingd-config` 升到 revision 19（两个新键）。
- #1389 #427-B 后续：`ac_diagnose` 的 `not_run` 也列出因共享上限在放置阶段被挡住的行及其数值；结果码和契约说明里的 "host budget" 改称 "shared run limits"（共享运行上限）；补一个宿主级的名额释放测试。没有新键、新结果码。

## v0.12.21

本版是 v0.12.20 之后合并的 3 个 PR：概览和诊断读账本时不再排在账本写入线程后面，也不再等帧清理做完：
`QueryEvents` 的页改由读者池在写入线程之外读，帧清理交给写入线程的工作分段进行（QR-1）；部署不再因为某个
运行时指令覆盖的默认值变了而自动回退（S2-IF）；另有 README 人员名单的一处文档修改。v0.12.20 已在本版之前
发布。

配置：没有新键，也没有新的 revision，`actingd-config` 仍是 [5, 18]，`runtime-client` 仍是 [2, 2]。

- 读者池的大小（2 个读线程）是程序里的固定值，不是配置键。帧清理分段的大小沿用已有的
  `ledger_observer.frame_query_chunk_frames`（默认 256，范围 1 到 4 096，启动时读取），默认值和范围不变。
- 启动报告 `StartReport` 的答复新增可选列表 `instructions_base_changed`（见下），仍在 `runtime-client`
  revision 2 之内，不升 revision：列表为空时不出现，本版的 actingctl 读较早构建的报告时按没有这个列表读。
- 现有的配置都不需要修改。退回 v0.12.20 之前也不需要做任何事：
  - 本版没有新键，v0.12.20 照样读本版的配置文件。
  - 读者池读的提交点（已提交的头序号、头记录的哈希、标记状态和删除证明）只由写入线程在内存里发布，从不写进
    账本；读者池用的是只读连接。账本的格式、表和索引都没变，v0.12.20 照样打开本版写过的账本。
  - 帧清理分段修剪时的保留规则（第一段开始之后才有记录引用的对象留下）只作用于写入线程常驻的内存，不写账本。
  - `instructions_base_changed` 只在启动报告的答复里，不写进账本。本版没有改任何默认值，每次启动又以当时的
    默认值重新记下运行时指令，所以从本版退回 v0.12.20 时，没有哪个指令会因为默认值变了被 v0.12.20 列为丢掉，
    回退启动的 `instructions` 检查不会因此失败。

运行上的变化：

- 读者池（QR-1）：`QueryEvents` 的页（概览里各实例最近一次运行、`ac_diagnose`、事件和运行证据的查询都经它读）
  和 Lab 订阅的页，由读者池的 2 个读线程读。每个读线程有自己的只读 SQLite 连接，不在写入线程上运行，也不等
  写入线程：写入线程正在提交或做帧清理时，这些读取照常答复。
  - 每页在写入线程最后发布的提交点读。写入线程每次提交之后、答复这次提交的追加之前发布，所以追加已得到答复
    的调用方能读到它。一页在一个读事务里读，`through_sequence` 不超过已提交的头，读不到还没提交的记录。答复和
    写入线程在同一个头读出的页相同：同样的选择、行认证和删除观察，同样的读取预算和语句界限
    （`ledger_observer.read_budget_ms` 等，从读线程开始读这一页时算起）。
  - 队列放不下、或在调用方的答复时限内没有读线程答复的页，答 `writer_response_timeout`（操作
    `project_ledger_view_page`），不致命，再读一次即可，和以前写入线程没有按时答复一样。在队列里等到调用方已经
    不再等待的页，以同样的 `writer_response_timeout` 答复，不再去读；关闭时队列里的页一页也不读，各自答
    `writer_unavailable`（操作 `shutdown_writer`，不致命）。
  - 读者池读的页照旧记进写入线程的提交统计（`ProjectViewPage`，带用时）。
  - 最新序号、按查询读、订阅与重放、标记状态、索引记录和制品查询等其他读取仍在写入线程上。
- 读者池的警告（QR-1）：读者池正常时什么也不打印。
  - 启动时读者池的连接打不开，写入线程像以前一样自己读这些页。启动时 actingd 日志里写一条警告（结果码和操作是
    让连接打不开的那次失败的），并打印开发者行 `actingd ledger reader_pool fallback=writer code=… operation=…
    detail=…`。
  - 运行中第一个意外结束的读线程写一条警告 `writer_panicked`（操作 `query_ledger_view`，细节
    `reader_ended readers_left=<剩下的数>`），剩下的读线程继续答复。一个读线程也不剩时再写一条
    `writer_panicked`（操作 `project_ledger_view_page`，细节 `readers_left=0 fallback=writer`），之后的页由写入
    线程读。这两条各写一次，同时打印 `actingd ledger reader_pool reader_ended code=… operation=… detail=…`。
  - 这些都只是警告，Runtime 继续运行。
- 帧清理时的读写（QR-1）：每轮帧清理开始时，写入线程把常驻的对象修剪到帧存储里现有的帧。以前这是一条写入命令，
  一次做完，期间所有追加和读取都排在后面。现在分段进行：每段是一条写入命令，最多看
  `ledger_observer.frame_query_chunk_frames` 个对象，两段之间写入线程先处理排队的其他命令（追加和读取）；按运行
  和范围记的事实在最后一段一起修剪。第一段开始之后才有记录引用的对象属于更新的状态，留下。Runtime 在两段之间
  停下时这一轮清理不开始，没看过的对象继续常驻：只多占内存，不丢事实（丢掉的对象在有记录引用时按索引读回）。
- S2 的启动后检查（S2-IF）：`StartReport` 把保留下来、但它覆盖的默认值变了的运行时指令
  （`config_override_base_changed`）列进新的可选列表 `instructions_base_changed`，算作已恢复（计入
  `instructions_restored`），不再列进 `instructions_dropped`；`instructions_dropped` 只列没有恢复的
  （`config_override_dropped`、`config_override_superseded`）。安装器的 `instructions` 检查在期望和实际相同、且
  没有真正丢掉或被取代的指令时通过；默认值变了的键在这一行的 `values.base_changed` 里列出，只作提示，从不让检查
  失败。例如从 v0.12.19 升级时，`instance.<实例>.idle.close_app_wake_lead_ms` 的指令（默认值从 60 000 改为
  300 000）以前会让这项检查失败、整个部署自动回退，现在通过。检查读的是刚启动的 Runtime 的报告：v0.12.20 及
  更早的 actingd 仍把这类指令列进 `instructions_dropped`，所以回退到默认值与本版不同的更早版本时，回退启动的
  这项检查仍不通过。

本版合并的 PR：

- #1383（文档）：README 的人员名单新增一行「正在面试 — Gemini‑4 Argon」，中英文 README 都改了。没有程序改动。
- #1384 S2-IF（#421）：启动报告把保留下来、但覆盖的默认值变了的指令列进新的可选列表 `instructions_base_changed`、算作已恢复，`instructions_dropped` 只列丢掉和被取代的；安装器的 `instructions` 检查在期望等于实际、且没有真正丢掉的指令时通过，默认值变了的键列在 `values.base_changed` 里作提示。没有新键、新结果码，`runtime-client` 不升 revision。
- #1385 QR-1（#422）：账本的视图页（`QueryEvents` 和 Lab 订阅的页）由 2 个只读连接组成的读者池在写入线程之外读，读的是写入线程答复之前发布的提交点；帧清理交给写入线程的修剪分段进行；修正 1：调用方已不再等待的页不读，关闭时拒绝队列里的页而不去读，读线程意外结束时写警告、一个不剩时由写入线程重新读页，读者池读的页记进提交统计。没有新键、新结果码。

## v0.12.20

本版是 v0.12.19 之后合并的 3 个 PR：组合模式（`policy` 节没有 `catalog`，由组合器按标准包组合）下读配置的各个读者
都能用了，`ac_packs` 和 `ac_diagnose` 重新有答复（RL-1c）；概览总在 MCP 输出预算之内，只读文本的客户端也能看到它的
每条警告（OV-24K）；空闲动作关掉的游戏在 Runtime 重启之后仍按关着处理、按时唤醒（IDLE-1P）。v0.12.19 已在本版之前
发布。

配置：没有新键，也没有新的 revision，`actingd-config` 仍是 [5, 18]。

- `idle.close_app_wake_lead_ms` 的默认值从 60 000 改为 300 000（5 分钟，够游戏冷启动一次），每个实例的
  `instance.<实例>.idle.close_app_wake_lead_ms` 默认跟随节里的值。配置文件里显式写的值和运行中设的指令仍然优先。每个
  唤醒提前量仍须小于 `idle.min_gap_ms`（两个提前量都检查，不论用哪个动作）：把 `idle.min_gap_ms` 设在 300 000 或
  以下、写了更小的 `stop_emulator_wake_lead_ms` 却没写 `close_app_wake_lead_ms` 的文件，v0.12.19 接受，本版以
  `config_value_invalid`（`section_check`）拒绝，要同时写上一个更小的 `close_app_wake_lead_ms`。
- `ledger_observer.tag.workbench_patterns` 的默认值加上 `host.idle_state`（空闲状态记录，见下）。配置文件显式写了这个
  列表的，显式的列表优先，要自己加上 `host.idle_state`，否则启动载入不了这条记录，每次启动都按未知处理。
- 现有的配置都不需要修改。退回 v0.12.19 之前也不需要做任何事：本版没有新键，v0.12.19 照样读本版的配置文件；本版写进
  账本的 `host.idle_state` 不在 v0.12.19 的 `tag.workbench_patterns` 默认值里，v0.12.19 启动时不载入它，也不因它拒绝
  （账本索引不拒绝按别的标记模式写下的行）。回退之后 `idle.close_app_wake_lead_ms` 的默认值回到 60 000，空闲关掉的
  游戏在重启之后又会被忘记（v0.12.19 原来的行为）；之后再升到 v0.12.20 时，那条记录是更早的所有者写的，启动按未知
  处理（见下）。

运行上的变化：

- 从 v0.12.19 升级后的第一次启动（IDLE-1P）：v0.12.19 没有留下空闲状态记录，这次启动不知道哪些游戏关着。每个空闲
  动作不是 `none`、配置了启动包（`stop_emulator` 还要配置了模拟器启动）的实例都按“已关闭、唤醒时间就是启动时刻”
  恢复：启动后的第一次空闲检查把它唤醒一次，跑它的启动包。启动包是例行的认领，排在部署的保持和任何调度暂停后面，
  保持释放、暂停解除之后才跑；`stop_emulator` 的配置的模拟器启动照旧不受暂停影响（裁定 35）。下一次计划再按正常的
  唤醒时间把它关上。代价是这次启动每个这样的实例多跑一次启动包；之后的启动都从上一个所有者的记录恢复。
- 重启之后仍记得关着的游戏（IDLE-1P）：空闲动作关掉（或正在关）的实例和它们的唤醒时间，每次变化都记成 runtime 事实
  `host.idle_state`（第一行是写入者的 `owner_epoch`，之后每个实例一行：`instance_alias`、`action`，有唤醒时还有
  `wake_at_unix_ms`）。启动只用上一个所有者写的记录，在恢复调度暂停之后、准备任何实例之前恢复，再以自己的纪元记
  一次。恢复的实例欠着它的唤醒：唤醒时间已过、或计划显示有任务到期时，第一次空闲检查就唤醒它，所以在唤醒提前量之内
  重启的实例在启动时就被唤醒。记录是更早的所有者写的（中间跑过不记录的旧构建，例如回退之后）、或读不懂的记录
  （`invalid_runtime_state_fact`，操作 `restore_idle_state`，记录下来、启动继续），都和升级后的第一次启动一样按未知
  处理。daemon 启动时的配置的模拟器启动跳过空闲动作停着的模拟器，留给它的唤醒。
- 关着的游戏不再只为读一个状态事实而冷启动（IDLE-1P）：实例被空闲动作关着时，它的识别维护包（R_I）只在有需要这次
  读取的任务到期时才跑，这时空闲检查先唤醒实例；否则 R_I 不跑、也不带唤醒，决定带 `instance_idle_action`，等待的
  任务答 `fact_refresh_pending`，`gate_code` 是 `instance_idle_action`。R_I 自己也在最小间隔、失败退避或限额之内时，
  等待的任务带那个闸门的 `gate_code`，但不拿 R_I 的唤醒时间，所以 R_I 的冷却结束不会冷启动关着的游戏。单独一次读取
  既不唤醒空闲的实例，也不设它的唤醒；在运行的实例上，到期或临近的读取仍和任务一样让它不被关掉。
- `ac_packs`（RL-1c）：`actingctl packs` 能解码组合模式的配置（以前报 `config_decode_failed`，缺 `procedure_manifest`），
  安装行是组合出的程序（`operation.package.run`，`scheduled: true`），启动包和资源路径检查重新运行。
- `ac_diagnose`（RL-1c）：`actingd suspended` 按组合判断，不再答 `suspended_policy_unconfigured`（只有没有 `policy` 节
  的配置才这样答），所以 `ac_diagnose` 的 `suspended`、`packs`、`remedies`、`lift`、`repeating` 和 `clock_slots` 重新
  有内容。组合时被拒的游戏不在组合里，它的暂停对是 `task_not_in_catalog`；整个组合被拒是
  `policy_catalog_compile_failed`，原因列出各个问题。组合的问题排在报告 `warnings` 最前面，形如
  `<code>:<game>:sha256:<hex>`（没读到 `schedule.json` 时 `<hex>` 处是 `unread`）。
  - 读取有界：按类型索引只读 6 种记录（`policy.dispatch_intent`、`policy.execution_recorded`、
    `policy.dispatch_admitted`、`policy.planning_signal_observed`、`task.failed`、`runtime.failed`），配置事实只读最新的
    一条 `config.subsystems`，账本的最新时间经时间索引读一行，只为显示的运行读它自己的行。
  - 截止时间：`actingd suspended --deadline-ms <ms>` 是调用方的截止时间，`actingctl diagnose` 和 `ac_diagnose` 传调用
    截止时间减 2 秒。读到截止时间就停下，以已读到的内容答复，`warnings` 最后加 `ledger_read_incomplete`（还没读到选中
    的记录时各列表为空、`through_sequence` 为 `null`；没读到的运行 `step` 为 `null`，帧比较是 `unavailable`，原因
    `ledger_read_budget_exceeded`），这次诊断标为 `incomplete`。到调用截止时间子进程还在运行，就在那时结束它，答
    `suspended_report_timeout`，不再留着运行。
- check-config 和启动后的准入（RL-1c）：组合模式下 `fact_reader_missing`、`pack_admission_suspended`、
  `policy_remedy_declaration_unusable` 和识别包的检查都按组合计算，不再悄悄跳过。启动后的标准包准入等工作台的第一次
  检查（第一个策略周期）组合好、或 Runtime 停下，再准入组合出的程序；以前组合模式下它什么也没准入。
- 概览的输出预算（OV-24K）：预算（24 KiB）按会话协商的协议版本实际发出的字节计算，所有工具都走这条路径。
  - 放得下时答复逐字节不变。2025-06-18 起的会话里 text 和 structuredContent 放不下两份时，text 改为摘要
    `{ok, structured_content_only, warnings, warnings_omitted, truncated, narrow}`，旁边是完整的 structuredContent：
    摘要带答复的每条警告（放不下的数在 `warnings_omitted`）、被截掉的块和缩小范围的提示（`ac_overview` 提示用
    `instance` 或 `sections`）。`ac_overview` 截到摘要能放下每条警告为止，所以只读文本的客户端也能看到每条警告。
    2025-03-26 没有 structuredContent，答复以文本发一次。
  - 超出预算时不悄悄截断：警告 `output_budget_exceeded` 给出整个答复的大小（`actual_bytes`）和预算（`limit_bytes`），
    `truncated` 依次列出每个被截块的 JSON 指针，被截的块留下 `<block>_truncated: true`。截断顺序是 performance、
    watchdog、catalog、config_generation、各行的 run（从最后一行起）、各行的 monitor、各行的 foreground、
    install.build，`fact_refresh` 最后截（以前最先截，会把 `fact_refresh_unbound` 藏掉）。
  - `performance` 里每种不同的 dwell 只写一次（`dwell`），各视图用 `dwell_24h_ref`、`dwell_total_ref` 按下标引用；
    `perf-status` 打印同一个对象。
  - 截止时间：`performance`、`fact_refresh` 和各行的 `not_running` 读之前先检查调用预算，用完就不读，留下
    `<block>_incomplete: true`（`not_running` 是每行的 `not_running_incomplete: true`），警告 `call_budget_exhausted`
    用 `member` 指出跳过的块，概览标为 `incomplete`。
  - 配置未生效：选中的配置文件的 sha256 和 actingd 启动时读的（`config.selection` 的 `config_sha256`）不同，就报
    `config_generation_mismatch`（`path`），在 `config_generation` 为 `{}` 的单目录安装上也一样；重启 actingd 才生效。

本版合并的 PR：

- #1379 OV-24K（#422）：`ac_overview` 在 MCP 输出预算之内：按协商的协议版本只序列化一次，放不下时文本是带每条警告、截掉的块和缩小提示的摘要，放得下的答复逐字节不变；`dwell` 只写一次；每个块（包括各行的 `not_running`）都检查调用截止时间，`call_budget_exhausted` 用 `member` 指出跳过的块；超出预算时 `fact_refresh` 最后截，每次截断都写明；配置文件的 sha256 和运行中的不同时报 `config_generation_mismatch`，单目录安装也一样。没有新键、新结果码。
- #1380 RL-1c（#411）：组合模式下每个读者都能用：`actingctl packs` 解码组合配置、列出组合出的程序；`actingd suspended` 读组合，`ac_diagnose` 重新有答复，读取有界（6 种记录、只读最新的配置事实、经时间索引读最新时间、调用方的截止时间 `--deadline-ms`、截短时 `ledger_read_incomplete`、到截止时间结束子进程 `suspended_report_timeout`）；check-config 按组合计算 `fact_reader_missing`、补救、准入和识别检查；启动后的标准包准入在第一次组合之后运行；组合的问题先作为警告列出。没有新键、新结果码。
- #1381 IDLE-1P（#405）：空闲关掉的游戏在 Runtime 重启之后仍按关着处理：空闲状态记成带写入者纪元的 `host.idle_state`，启动只用上一个所有者的记录，没有可用记录时每个有空闲动作的实例以启动时刻为唤醒、跑一次启动包（排在部署保持和暂停之后）；`idle.close_app_wake_lead_ms`（和每个实例的默认）默认改为 300 000；读取器在间隔或退避之内时，关着的游戏不再只为读一个状态事实而冷启动，等待的任务答 `fact_refresh_pending` 并带闸门。新位置 `restore_idle_state`，没有新键、新结果码。

## v0.12.19

本版是 v0.12.18 之后合并的 2 个 PR（为可靠部署服务的功能，按 Alice 的规则编为 0.12.x）：部署成为一条产品命令，
`actingctl install update apply … --stop --hold … --start` 依次保持各实例、正式停下 Runtime、换文件、启动新版本、
做启动后检查；新版本启动之后、恢复调度之前检查不通过，就自动回退到上一个构建（S2，裁定 74 A）；保持期间推迟的
工作逐条记下，释放时交还（裁定 75）。长时间空闲的实例在概览里读最近一次运行不再因读取预算用完而失败（IDLE-OV）。
v0.12.18 已在本版之前发布。

配置：`actingd-config` 升到 [5, 18]。revision 18 在 `install` 节新增五个可选键。和 revision 17 的 `install` 节一样，
actingd 只解析和检查它们、不使用；`actingctl install` 每次执行时从根目录的配置文件读取，不能在运行中修改，改了配置
文件之后从下一次命令起生效。各键（时间单位都是毫秒）：

- `install.start.sample_interval_ms`：安装器的启动多久记一次进度样本（启动日志的大小、正在启动的 Runtime 读了多少），
  默认 30 000，范围 1 000 到 600 000。
- `install.start.stall_after_ms`：启动日志这么久没有增长，就报疑似卡住 `start_stalled`，默认 720 000，范围 60 000 到
  3 600 000。只报告，不杀任何进程。
- `install.start.stall_read_bytes`（整数，单位字节）：正在启动的 Runtime 在就绪之前读了超过这么多，就报 `start_stalled`，
  默认 53 687 091 200（50 GiB），范围 0 到 1 099 511 627 776。只报告，不杀任何进程。
- `install.start.memory_wait_ms`：每次启动尝试等 `memory` 守卫通过的时间（刚停下的 Runtime 的内存会陆续释放），超过就
  拒绝，默认 120 000，范围 0 到 3 600 000。
- `install.stop.busy_wait_ms`：Runtime 因实例还有排队的请求而拒绝暂停时，保持等这些请求清空、再暂停一次的时间（进行中
  的运行交给暂停自己的排空），默认 300 000，范围 0 到 3 600 000。

revision 17 已有的 INST-2 键（`install.stop.drain_timeout_ms`、`install.stop.shutdown_wait_ms`、
`install.start.ready_timeout_ms`、`install.start.attempts`、`install.hold_after_start`、
`install.guard.min_available_memory_mb`、`install.guard.before_next_start_ms` 和 `install.guard.adb_timeout_ms`）从本版
起起作用，默认值和范围不变；`install.guard.before_next_start_ms` 只决定 `schedule_near` 列出多久之内到期的工作。
`install.hot.idle_wait_ms` 仍属于之后的 INST-4。

`install.guard.blocked_windows` 在 revision 18 退役（裁定 75：部署没有允许或禁止的时间段，改为保持期间记下推迟的工作、
释放时交还）：配置文件仍可以写它，是列表就接受、不起作用、其中的时间窗不再判断，每个读这一节的 `actingctl install`
命令都以 `install_caveat_noted`（`config_key_retired`，`config_key` 是这个键）提示；不是列表的值仍是
`config_value_invalid`。默认文件里的 `[]` 不提示。守卫 `blocked_window` 不再评估。

现有的配置都不需要修改：没有写这五个新键的配置文件保持原来的 revision，Runtime 仍读 revision 5 到 17 的文件。
v0.12.18 拒绝写了任何 revision 18 键的配置文件，因为 `install` 节和其中每个对象都不接受未知成员；退回 v0.12.18 之前
要先去掉这些键（回退闸门的 check-config 部分同样因此不通过，自动回退也一样）。

运行上的变化：

- 一条命令部署（S2）：`actingctl install update apply --root <root> --build <commit> --plan <id> --stop --hold <reason>
  [--known-pause ALIAS=REASON]… [--start] [--ack-guard <id>]… [--detach]`。计划在 Runtime 运行时用
  `update plan … --preview` 做，apply 重算一次；命令从保持之前到结束一直持写锁，整个过程是一次操作、逐步写进日志，
  任何一步中断都能 `install resume`。单独的 `install stop --hold <reason>`、`install start` 和
  `install guards --for stop|start`（只报告、不改动）也可以用。
  - 保持：改动之前先从运行中的 Runtime 读 S0（所有者纪元、所有暂停、所有运行时指令）记进日志，并在第一次暂停之前记下
    推迟记录：每个实例排队的请求、是否有运行在进行，以及 Runtime 最近的计划里选中的决定、到期的任务和最早的唤醒。然后
    同时暂停 S0 里所有未暂停的实例（原因就是 `--hold`），不会有实例在别的实例排空时继续派发。进行中的运行由 Runtime
    自己排空：在 `install.stop.drain_timeout_ms` 内跑完，否则在下一个检查点停下，绝不在动作中途切断。实例因有排队的
    请求拒绝暂停时，等它清空再暂停一次。别人的暂停原样保留，只有用 `--known-pause` 写明它的原因才允许；全局暂停或
    没写明的暂停直接拒绝（`guard_failed`，确认也不放行）。保持在任何改动之前被拒绝时，恢复它暂停的实例，命令仍以
    拒绝结束。
  - 停下：`request-shutdown` 正式关闭，在 `install.stop.shutdown_wait_ms` 内等它退出；关闭之前再读一次运行时指令和别人
    的暂停，启动时恢复的就是这一份。读不到停下的结局时报 `stop_terminal_unconfirmed`（Runtime 照样已停）。
  - 启动：每次尝试之前检查 `memory` 守卫，在 `install.start.memory_wait_ms` 内每 500 毫秒重读；组合命令里正在运行的
    Runtime 占用的内存算作可用（停下后会释放）。经看门狗的手动启动方法启动，等就绪，按 `sample_interval_ms` 记进度
    样本；疑似卡住只报 `start_stalled`；到 `install.start.ready_timeout_ms` 还没就绪是 `start_timeout`，进程留着运行，
    也不在旁边再起一个。
  - 启动后检查：`ready`（就绪，且启动时就绪的那个所有者仍在应答）、`open`、`no_seeding`、`bindings`、`packs`、
    `pauses`、`instructions`、`no_dispatch`、`new_keys`、`config`、`indexes` 和 `one_way`，对照 S0、配置和
    `state.json` 判断，不对照某个版本的预期。新版本用新的 Runtime 读取 `StartReport` 报告自己的启动（账本打开的各阶段、
    载入还是重新播种、恢复的暂停和指令、绑定的实例、登记的标准包、启动以来每个实例准入的派发；`runtime-client` 仍是 2，
    只有同一构建的 actingctl 发送它）；更早的构建（例如回退到的 v0.12.18）从状态、配置事实和启动日志检查，看不出的
    记为 `not_evaluated`，不会触发回退。`config` 对照操作开始时的配置文件哈希，所以 adopt 之后改过配置不算失败。
  - 自动回退（裁定 74 A）：只在带 `--start` 的组合更新里，只在新版本启动之后、恢复调度之前。失败类检查不通过，或新
    启动没有就绪就退出了，就先把回退的各步写进日志，再关闭新的 Runtime、对上一个构建过回退闸门、反做这次的文件步骤
    （被换下的文件都留着）、启动上一个构建并检查，它的检查通过就释放保持、恢复调度。命令以 `deploy_rolled_back` 结束，
    即使调度已经恢复也报失败。回退中途中断时 `install resume` 继续回退，不再重新判断新版本。闸门不通过
    （`rollback_gate_failed`）时新构建和保持都留着，只能向前修。`start_timeout` 和疑似卡住不触发回退；恢复调度之后不再
    自动回退，`install rollback plan|apply`（带闸门）照旧。单独的 `stop`、`start` 或回退命令从不自动回退，失败类检查不
    通过以 `post_check_failed` 结束、保持留着。自动回退到 INST-2 之前的构建之后，要从最新的留存构建
    （`install\builds\<提交>\runtime\`）运行 `install`，因为旧构建的 actingctl 读不懂停、启的日志。
  - `release-hold --reason <reason>`：只恢复以这个原因暂停的实例，从不动别人的暂停，并交还推迟的工作：按推迟记录逐个
    实例记下是否已恢复、每个到期任务和选中的决定是否已重新评估（`re_evaluated` 或 `pending_evaluation`）、排队的请求，
    以及排空时在检查点停下的运行。Runtime 从下一轮起自己重新评估推迟的派发；安装器不重发记录里的任何东西，已经发出的
    操作不会重放。`install.hold_after_start`（默认 true）让组合命令启动后仍保持，直到人释放；失败的步骤也保留保持，
    并在 `next` 里写出 `release-hold` 的用法。
  - 不在中途继续的：排空时在检查点停下的运行不从中途继续，释放后 Runtime 从头重新评估它们的任务
    （提示 `runs_not_continued`，带数量）；空闲动作不是 `none` 的实例欠下的空闲唤醒没有读出、也没有记录，Runtime 启动
    后重新推导（提示 `idle_wakes_rederived`，带数量）。每个做了保持的答复都带这两个提示。
  - `--detach`：先拒绝未完成的操作和读不了的配置，然后经看门狗的手动启动方法在后台运行同一条命令（不带 `--detach`），
    它不随启动它的会话结束，输出追加到 `install\detached-<ms>.log`，命令立即答复 `pid` 和 `log`。之后的拒绝（计划
    过期、守卫不通过）只在这个日志里，`next` 指向 `install status`。
  - 守卫：`schedule_near` 只列出保持会推迟的、`install.guard.before_next_start_ms` 之内到期的工作，从不拒绝
    （`--ack-guard schedule_near` 接受、没有作用）；`adb` 探测不应答时再试一次；`memory` 和 `foreign_pauses` 照上。
  - 什么都不杀、不删：等待超时只报告，进程留着；被替换的文件留作 `.old-<ms>`。`install status` 新增
    `mcp_servers_on_old_images`，列出映像已被更新改名挪开、还在运行的 `mcp-serve`，它们的客户端重新连接即可。
- 概览读最近一次运行（IDLE-OV）：每个实例行的最近一次运行改为在实例上按时间窗向前列出（从窗口的第一条记录起），不再
  从账本尾部按窗口一段段往回走。窗口里没有运行的空闲实例约 5 页（每种列表一页）就答 `run_none_in_window`，不论窗口里
  别的记录有多少，也从不读窗口之前的历史；忙的实例只从最近一次运行的第一条列表事实起读入口记录，读取量不再随窗口里的
  入口记录增长。被时限截断的列表仍是 `run_incomplete`（带原因），从不说“没有运行”。
- 账本读取页的 Lab 关系（IDLE-OV）：只有 Lab 视图在选择之前读；其他的页只在第一行指向请求、关联或运行的记录处读，空页
  从不读，所以不会再因此被拒绝。关系在那一行被预算截断时，这一页在它之前结束、答不完整，下一页从那一行起。
- 事件读取的失败码（IDLE-OV）：QueryEvents 的读取用完读取预算或写入线程没有按时答复时，以 `ledger_read_failure`
  （`runtime_unavailable`，状态 `failed`）答复，不再是 `protocol_invalid`，和其他 Runtime 读取一样，稍后再读即可，
  Runtime 照常运行；`ac_observe` 在终态之后的投影也以这个读取失败作为原因。Lab 视图订阅的重放仍报 `protocol_invalid`。

本版合并的 PR：

- #1375 INST-2（S2）：部署成为一条产品命令：`actingctl install stop`、`start`、`release-hold`、`guards` 和组合的 `update apply … --stop --hold … [--start] [--detach]`，保持期间的自动回退（裁定 74 A），保持、记录并交还推迟的工作（裁定 75），新的 Runtime 读取 `StartReport`（`runtime-client` 仍是 2）和启动后检查，`schedule_near` 只作提示，`install.guard.blocked_windows` 退役，`actingd-config` [5, 18] 的五个新键（见上）。新结果码 `request_start_report_completed` 和 `invalid_start_report`，新位置 `runtime_start_report`；`install_refusal` 新增 `runtime_not_running`、`runtime_stop_failed`、`post_check_failed`、`deploy_rolled_back`，`install_caveat` 新增 `start_stalled`、`stop_terminal_unconfirmed`、`runs_not_continued`、`idle_wakes_rederived`、`config_key_retired`。
- #1377 IDLE-OV：概览按时间窗向前列出实例的最近一次运行，空闲实例约 5 页答 `run_none_in_window`，忙的实例读取量不再随入口记录增长；Lab 关系只在第一行有关联的记录处读；QueryEvents 把读取预算用完和写入线程超时报为 `ledger_read_failure`（`runtime_unavailable`）。没有新键、新结果码。

## v0.12.18

本版是 v0.12.17 之后合并的 3 个 PR（为可靠部署服务的功能，按 Alice 的规则编为 0.12.x）：部署改由产品自己的
`actingctl install` 进行，有日志、写锁、带闸门的回退，什么都不删（INST-1b-F）；MCP 客户端都经一个稳定的入口
连接，之后换版时入口在两次调用之间自动切到新版本，不用重启客户端，只有第一次改为经入口连接时要重启一次客户端
（MCP-ENTRY）；资源池的容量和资源预测的比较值可以读事实（CF-5）。v0.12.17 已在本版之前发布。

配置：`actingd-config` 升到 [5, 17]。revision 17 新增可选的 `install` 节：`actingctl install` 和 MCP 入口的可调
参数。actingd 只解析和检查这一节、不使用它；`actingctl install` 和 `actingctl mcp-entry plan` 每次执行时从根目录
的配置文件读取它，所以这些键都不能在运行中修改，改了配置文件之后从下一次命令起生效。这一节和其中每个对象都不接受
未知成员；值超出范围是 `config_value_invalid`。各键（时间单位都是毫秒）：

- `install.lock_wait_ms`：等待写锁 `install\writer.lock` 的时间，默认 60 000，范围 1 000 到 600 000。
- `install.rollback.open_margin_ms`：回退闸门里目标构建打开账本的耗时要比时限少出的余量，默认 20 000，范围 0 到 600 000。
- `install.child.check_config_timeout_ms`：安装器运行的每次 `actingd check-config` 的时限，默认 900 000，范围 10 000 到 3 600 000。
- `install.child.ledger_read_timeout_ms`：每次 `actingledger` 读取（S0，回退闸门读取部分的每一页）的时限，默认 300 000，范围 10 000 到 3 600 000。
- `install.child.ledger_verify_timeout_ms`：更新或回退时 `actingd ledger-maintenance verify` 的时限，默认 1 800 000，范围 60 000 到 86 400 000。
- `install.mcp_entry.poll_ms`：MCP 入口多久看一次安装，默认 2 000，范围 100 到 60 000。
- `install.mcp_entry.settle_ms`：安装的变化要保持多久不动，入口才询问计划，默认 10 000，范围 0 到 600 000。
- `install.mcp_entry.verify_timeout_ms`：候选子进程的核验、计划和句柄计数的时限，默认 60 000，范围 1 000 到 600 000。
- `install.mcp_entry.drain_max_ms`：切换等未完成的句柄多久之后在状态里说明（之后仍然等），默认 600 000，范围 0 到 86 400 000。
- `install.mcp_entry.refused_retry_max_ms`：计划连续被拒时，入口下一次询问前的等待从 `settle_ms` 起逐次加倍，加倍到的上限，默认 300 000，范围 1 000 到 86 400 000。
- 属于之后的切片（INST-2 的停、启、保持和守卫，INST-4 的热放置）的键：`install.stop.drain_timeout_ms`（默认 300 000，
  范围 1 000 到 3 600 000）、`install.stop.shutdown_wait_ms`（60 000，10 000 到 600 000）、
  `install.start.ready_timeout_ms`（900 000，60 000 到 3 600 000）、`install.start.attempts`（整数，2，1 到 5）、
  `install.hold_after_start`（布尔，默认 true）、`install.hot.idle_wait_ms`（600 000，0 到 3 600 000）、
  `install.guard.min_available_memory_mb`（整数，单位 MB，10 000，0 到 1 048 576）、`install.guard.before_next_start_ms`
  （900 000，0 到 86 400 000）、`install.guard.blocked_windows`（时间窗列表，默认空，不内置任何时段）和
  `install.guard.adb_timeout_ms`（15 000，1 000 到 60 000）。

现有的配置都不需要修改：没有写 `install` 节的配置文件保持原来的 revision，Runtime 仍读 revision 5 到 16 的文件。
v0.12.17 及更早的 Runtime 拒绝写了 `install` 节（任何 `install.*` 键）的配置文件，因为配置文件不接受未知成员；
退回旧版本之前要先去掉这些键。同一 revision 带来 `actingd check-config --install-root <root>`，v0.12.17 及更早的
构建以 `check_config_usage_invalid` 拒绝这个选项，所以 `actingctl install` 对这样的目标构建不带它运行 check-config。

运行上的变化：

- 命令行安装（INST-1b-F）：`actingctl install` 安装、更新和回退一个单一位置的根目录，每个改动都是一次操作，逐步写进
  日志（`install\operations\`），改动 `runtime\`、`tools\` 和配置文件时独占写锁。
  - `adopt`：在一个由脚本安装的根目录上第一次运行。按 `runtime\` 和 `tools\` 自己的清单核对每个文件，把清单列出的文件
    和两份清单复制到根目录下留存（`install\builds\<提交>\`），写下当前构建、标准包登记和配置的 SHA-256。有文件与清单
    不符时拒绝（`live_build_modified`）；同一构建再 adopt 一次什么都不改。
  - `release verify` 和 `release stage`：核对发行版的两个 zip 和 `SHA256SUMS`（可以再核 release-facts、期望的 tag 和
    提交）；`stage` 在核对通过后把这个构建留存到根目录下，供更新和回退使用。
  - `update plan`（只读）：核对当前构建和目标构建、接口边，逐文件列出新增、替换和不变（清单之外的文件列为 `extra`、原样
    留下），列出默认值的差异，分别运行目标和当前构建的 check-config，需要时安排账本校验；Runtime 在运行时拒绝，
    `--preview` 只报告。`update apply` 带上计划 id，在写锁下重算计划（不同就以 `plan_changed` 拒绝），记下账本头 S0，
    每个文件先复制成 `.part` 并核 SHA-256，再把被替换的文件改名为 `.old-<ms>`、新文件改名就位（先 `tools\` 后
    `runtime\`），核对整棵树，最后记下新的当前构建。本版的 apply 要求 Runtime 已经停下，也不动配置（停、启和配置候选
    属于之后的切片）。中途失败时操作保持未完成，根目录要先 `resume` 或 `undo` 才能正常运行。
  - `rollback plan` 和 `rollback apply`：回到一个留存的构建，只恢复 `runtime\` 和 `tools\` 的程序文件，不动状态根、账本、
    包、策略和配置文件。回退闸门在计划时评估一次，apply 时在写锁下第一个改动之前再评估一次，四部分都要通过：接口边；
    目标构建自己的读取器能读完 S0 之后的每一页账本；目标构建在这个根目录上最近一次打开账本的耗时不超过打开时限减
    `install.rollback.open_margin_ms`；当前的配置通过目标构建的 check-config（写了更新的 revision 才有的键，
    例如 `install.*`，就在这里不通过，要先去掉）。闸门不通过就以 `rollback_gate_failed` 拒绝，只能向前修。**回退只
    允许回到这次更新替换掉的那个构建**：要回到更早的构建，先回到它，再回退一次。
  - `status`：报告布局、当前和留存的构建、包登记、配置、未完成的操作、最近完成的操作、遗留物数量和回退窗口里便宜的
    部分；`status --check-rollback` 对上一个构建评估完整的回退闸门，不改任何东西。
  - `resume` 继续一个未完成的操作（逐步核对磁盘上的状态，已完成的步骤不重做）；`undo` 撤销一个未完成的操作，或在
    之后没有别的操作时撤销最近完成的那一个（撤销完成的更新或回退同样要过闸门）。
  - **什么都不删。** 被替换的文件留作 `.old-<ms>`，撤销时挪开的文件也留下；`leftovers` 列出所有留下的文件和文件夹，
    带所属操作 id 和大小，从不删除。
  - 安装器运行的每个子程序都有时限（`install.child.*`）。到时限还在运行的子程序不会被杀，只报告它的 pid 和对应的键：
    操作开始之前的读取以对应的结果拒绝、什么都不改；账本校验超时则操作以 `install_operation_interrupted` 结束、什么都
    没换，留给 `resume`（再校验一次）或 `undo`。
  - 看门狗行 7a″：`install\current.json` 记着一个未完成的更新或回退时，看门狗不启动 Runtime
    （`watchdog_installer_unfinished`），`actingctl start` 和 `restart` 也拒绝；先 `resume` 或 `undo`。未完成的
    `release stage` 和 `adopt` 不挡启动。
  - `actingd check-config --install-root <root>`：按给定的安装根目录检查配置；安装器用它检查目标构建。
- MCP 统一入口（MCP-ENTRY）：
  - 位置：Tools 包新增 `actingcommand-mcp-entry.exe`，放在 `<root>\mcp-entry\g1\`。`g1` 是入口自己的代数，只有入口和
    子进程之间的约定变化时才换一个新的 `g<N>` 目录，从不覆盖正在运行的入口。本版的 `install update` 还不放置入口：
    部署时把 Tools 里的这个文件复制过去。
  - 登记：`actingctl mcp-config --client claude|codex|antigravity|dsh --entry --root <root> [--tier …]` 打印各客户端的
    配置片段，不写任何客户端文件。客户端改为启动 `<root>\mcp-entry\g1\actingcommand-mcp-entry.exe --root <root> --client
    <名字> -- mcp-serve --tier …`；`--` 之后的参数原样传给 `<root>\runtime\actingctl.exe`，档位和权限范围不变。第一次
    改为经入口连接时要重启一次客户端。
  - 何时切换：入口监视 `runtime\BUILD-MANIFEST.json` 和 `runtime\actingctl.exe`，变化保持 `settle_ms` 不动之后询问
    `actingctl mcp-entry plan`。只有安装完整时才切换：写锁空闲、没有未完成的安装操作、已安装的文件与清单一致；否则
    计划被拒，当前子进程继续服务。候选子进程先用客户端的握手核验，核验失败就记下、旧子进程继续服务。切换只发生在两次
    调用之间；当前子进程还有进程内的句柄（结果还没读的后台任务、Lab 句柄、还在提交的运行）或还有调用在进行时，切换
    推迟、绝不切断，超过 `drain_max_ms` 仍然等，并在状态里说明。运行的句柄就是 Runtime 的 request_id，不受切换影响。
    v0.12.17 及更早的子进程不回答 `actingcommand/entryStatus`，没有调用在进行时算作没有句柄。
  - 结果未知：调用进行中子进程结束时，这个调用答 `mcp_entry_outcome_unknown`，绝不重发；先用 `ac_get_run` 或
    `ac_overview` 查清结果，下一个请求交给新的子进程。
  - 各客户端怎样拿到新工具表：入口发 `notifications/tools/list_changed`（2026-07-28 协议的客户端在它订阅的流上收到同样
    的通知）。支持自动更新工具表的客户端（Claude Code）自己重新拉取；Codex 要重新加载这个服务；Antigravity 在 MCP
    管理器里点 Refresh；DSH 要重启这个服务（插件重新加载），工具白名单的名字不变。
  - 游标：`ac_events` 等工具在切换前发出的 `next_cursor` 属于旧的子进程，切换后答 `cursor_invalid`，从 `after_sequence`
    重读即可。
  - 状态：每个入口写 `<root>\mcp-entry\state\entry-<pid>.json`（当前子进程和构建、上一次切换、推迟、候选失败、通知、
    该客户端的刷新方式），`actingctl mcp-entry list --root <root> [--all]` 查看，pid 已不在的标 `stale`；状态文件从不删除。
- 资源池容量读事实（CF-5）：资源池的 `capacity` 和 `resource_projection` 的 `value` 都可以写成 CF-4 的事实引用
  `{"type": "fact", "fact_key": …, "offset": …}`，例如“预计数量 ≥ 读到的上限 − 20”，上限不再是常数。每次评估时，容量
  是这个事实在资源池自己作用域里的整数加偏移量（要达到资源池的最低置信度、没有过期，且至少为 1）。读不到时资源池没有
  值：这条 `resource_projection` 为未知，决定推迟（`trigger_unknown`），并请求刷新这个事实，不退回任何常数。读数高于
  读到的容量时按读数算，不截断。比较值读不到时这个谓词为假（`fact_reference_unavailable`），和 `fact` 谓词一样。写
  常数的规则和以前完全一样，序列化和哈希都不变。用到事实形式的 `schedule.json` 要在 `requires` 里写
  `pool_fact_reference`（actingd 新增的常开子系统），否则以 `schedule_invalid` 拒绝；更早的 Runtime 在 `requires` 处
  拒绝（`schedule_requires_missing`）。

本版合并的 PR：

- #1373 CF-5：资源池的容量和资源预测谓词的比较值可以读事实；新的常开子系统 `pool_fact_reference`。没有新键、新结果码。
- #1321 INST-1b-F：`actingctl install` 的 adopt、release stage、update 和 rollback 的 plan/apply、resume、undo、status（含 `--check-rollback`）和 leftovers，写锁和日志，`actingd check-config --install-root`，`install` 节（`actingd-config` [5, 17]），看门狗行 7a″。没有新结果码。
- #1374 MCP-ENTRY：稳定的 MCP 入口 `actingcommand-mcp-entry.exe`（Tools 包），`actingctl mcp-entry plan|list`，`mcp-config --entry`，`actingcommand/entryStatus`，五个 `install.mcp_entry.*` 键，三个新结果码 `mcp_entry_outcome_unknown`、`mcp_entry_child_unavailable` 和 `mcp_entry_usage_invalid`；切换只在两次调用之间，从不重发请求。

## v0.12.17

本版是 v0.12.16 之后合并的 3 个 PR（修复，以及为修复服务的功能）：Runtime 自己定期做的账本读取不再从
序号 0 起走过整段历史，写入线程没有按时答复的读取只让它自己那一步或那个请求失败（LN-WS-F）；帧清理器
改为按账本查询决定每一帧，打开账本不再读帧，证据帧在确认读过后或到期时删除（LN-7c Q，裁定 70）；以及
离线核对清理器查询的 `ledger-maintenance verify --frame-query`（LN-7c V）。v0.12.15 和 v0.12.16 都已
在本版之前发布。

配置：`actingd-config` 升到 [5, 16]。LN-7c Q 新增三个键：顶层的 `frame_retention_evidence_max_age_days`
（与 revision 4 的三个帧保留开关并列；`kept\` 里没有确认读过的证据帧按入库时间满多少天删除，默认 15，
范围 1 到 3 650，可以在运行中修改，清理器在每一轮清理开始时读取，所以从下一轮起生效）、
`ledger_observer.frame_query_steps_per_second`（清理器的查询每秒最多走的 SQLite 虚拟机步数，默认
500 000，范围 1 000 到 100 000 000，可以在运行中修改，每条语句之后读取）和
`ledger_observer.frame_query_chunk_frames`（一条 L 或 K 语句最多处理的帧数，默认 256，范围 1 到
4 096，启动时读取，运行中不能改）。没有 14 天的键。现有的配置都不需要修改：没有写这三个键的配置文件
保持原来的 revision，Runtime 仍读 revision 5 到 15 的文件。LN-7c Q 之前的 Runtime（包括 v0.12.16）
拒绝写了其中任何一个键的配置文件，因为配置文件和 `ledger_observer` 节都不接受未知成员；退回旧版本之前
要先去掉它们。

运行上的变化：

- 启动：打开账本不再读帧（LN-7c Q）。v0.12.15 和 v0.12.16 打开账本时要列出在库的帧文件，读出这些帧的记录和帧链接，以及它们所属的运行、租约、请求和关联的记录等（生产账本上约 56 万行，估计），这是打开中随在库帧增长的那一部分；写入线程也不再常驻帧的保留状态（约 50–100 MB，估计）。加上 v0.12.16 的 LN-7c O，打开账本只读元数据和链头、尾部的记录、索引的状态，以及最近两个所有者纪元的所有者记录：按设计估计，通常约 1–2 秒，两个各 3 天的所有者纪元之后约 3–5 秒，峰值约 50–100 MB（均为估计）。
- 帧保留（LN-7c Q）：清理器不再从写入线程取帧视图，写入线程也不保留任何帧的保留状态；清理器在自己的只读连接上用账本查询（W、L、K 三条语句，全部按索引走，不扫整段历史）决定每一帧，查询按 SQLite 虚拟机步数限速（`frame_query_steps_per_second`）。每一轮清理（最多每 10 分钟一轮）只查看到了下一个判断时刻的在库帧：入库满 60 秒还没判断过的帧，判为保留的普通帧满 1 天、资源帧满 7 天时再查，运行中、被占用和本进程还没见过的帧每轮都查；`kept\` 里的帧每轮都查。启动后的第一轮要查看全部在库帧，按设计估计约 3–5 分钟，期间每 2 秒的处理照常；之后每轮约 12–18 秒（均为估计）。普通帧 1 天、资源帧 7 天的保留期不变，但改从帧的入库时间起算（以前从帧所属运行结束时起算），所以最多比以前早约 30 分钟；仍然要等帧定下来之后才处理。帧索引没有覆盖整个账本时（例如它的补建还没有完成），这一轮什么都不判断、什么都不动，打印 `actingd frame_retention pass skipped=frame_refs_incomplete`。查询失败只算这一轮的错误，不会停下 Runtime。每轮清理的输出行新增 `kept_examined`、`removed_confirmed`、`removed_expired`、`kept_unknown` 和 `query_steps`。
- 证据帧（LN-7c Q，裁定 70）：Lab 帧和错误帧（还在库里的，或已挪进 `kept\` 的）一经确认读过，下一轮清理就删除；还在库里的直接删除，不再挪进 `kept\`。确认可以针对一张帧，也可以针对一次运行（只算确认之前截下的帧）；只有代理经 MCP（`ac_evidence_confirm_read`）、命令行（`actingctl evidence confirm-read`）和 UI 发出的确认才算，只打开或查看帧不算确认。v0.12.16 里已经记下的确认同样算数，升级后第一轮清理就删除它们覆盖的证据帧。`kept\` 里没有确认过的证据帧，按入库时间满 `frame_retention_evidence_max_age_days`（默认 15 天）后删除：在 10 月 18 日 21:37 开始的账本上，最早的未确认证据帧要到 11 月 2 日 21:37 左右才满 15 天。删除后空了的子文件夹和日期文件夹一并删除，不递归。`kept\` 里账本没有记录的文件只计数（`kept_unknown`）、在 `actingledger frames` 里列出，从不删除。确认的答复新增 `covers`（这次确认覆盖的证据帧数 `evidence_frames`，其中在 `kept\` 里的 `in_kept`）；数不出来时不带 `covers`、加一条警告，确认照样记下。
- 账本读取（LN-WS-F）：Runtime 自己定期做的账本读取不再从序号 0 起走过某一事件类型的整段历史：有待结算的调度时，每次结算检查不再读出历史上全部的 `policy.dispatch_intent`，而是按调度日志记下的序号读该条记录，再按它的链接读同一决定的意图；提案和战略报告的证据核对只按产物索引读它引用的产物的 `artifact.verified` 记录。读出的结果与以前相同。这些读取在写入线程的应答时限（`writer_reply_timeout_ms`）内没有得到答复、或用完读取预算时，只让它自己那一步或那个请求失败：结算检查这一轮不再读后面的结算，被跳过的结算什么都没写，留到下一轮再读，actingd 日志写一行；请求（提案、战略报告、审批、项目接口等）以账本的结果码（`writer_response_timeout` 或 `ledger_read_budget_exceeded`）失败，调度准入这一次不准入、记为非致命的准入失败，下次评估时再来；Runtime 照常运行。仍然停下 Runtime 的：账本的其他故障（例如损坏的记录，原因带上账本的结果码）；提交之后的读取（例如一次运行的执行记录提交之后读它的完成记录），因为跳过它会让已提交的结算只做了一半；以及启动时的读取。
- 离线核对（LN-7c V）：`actingd ledger-maintenance verify --frame-query` 在 verify 的完整读取通过之后，用清理器的 W、L、K 语句对同一个根目录、同样的帧文件列表和同一个时钟作答，与完整的帧视图逐帧比较；有不一致（裁定的差别和记录能说明原因的安全差别除外）时报 `ledger_index_mismatch`（操作 `verify_frame_query`，带 `artifact_id` 和 `field`），以非零退出码结束，帧索引没有覆盖账本时也失败。它要在 Runtime 停着时运行，Runtime 自己从不运行它。要写 `--frame-query` 才做，不写时 verify 和以前完全一样；写了而根目录不是正式根目录、或用于 verify 以外的操作时，以 `maintenance_arguments_invalid` 拒绝，从不忽略。它要在 verify 的时限（默认 120 秒）内做完完整读取和这次核对，账本很大时可能来不及，那时以 `ledger_read_budget_exceeded` 失败。

本版合并的 PR：

- #1369 LN-WS-F：Runtime 定期的账本读取（调度的结算与对账、提案和战略报告的证据核对）不再从序号 0 起走过整段历史；写入线程没有按时答复或用完预算的读取只让它自己那一步或那个请求失败；提交之后和启动时的读取仍然是致命的（见上）。没有新键、新结果码。
- #1370 LN-7c Q：帧清理器按账本查询（W、L、K）决定每一帧，打开账本不再读帧，写入线程不保留帧视图；证据帧确认读过后或满 `frame_retention_evidence_max_age_days`（默认 15 天）后删除；`ac_evidence_confirm_read` 和 `actingctl evidence confirm-read` 的答复新增 `covers`；`actingledger frames` 的在库帧分类改由同样的语句给出，并新增 `evidence` 成员；`actingd-config` [5, 16]，三个新键（见上）。没有新结果码。
- #1371 LN-7c V：`ledger-maintenance verify --frame-query`：离线、需显式开启，用完整帧视图核对清理器的语句（见上）；不能运行的地方拒绝。没有新键、新结果码。

## v0.12.16

本版是 v0.12.15 之后合并的 5 个 PR（修复，以及为修复服务的功能）：修 10 月 26 日 23:23:04 生产上的停机
（DIAG-RUN），启动时打开账本不再读出全部所有者记录（LN-7c O），为按账本查询保留帧（裁定 70）建帧索引和
读取确认（LN-7c R、C），以及声明式安全区接口（DA-4b-D，裁定 69）。v0.12.15 已经发布，但还没有在生产上
运行过，生产上运行的仍是 v0.12.13。

配置：`actingd-config` 升到 [5, 15]。DA-4b-D 新增 `display_safe_area` 节，三个键都可以在运行中修改，
从下一帧起生效：`display_safe_area.directory`（声明文件所在的目录，默认 `display-safe-area`，相对路径
按 Runtime 的状态根目录解析，不能为空）、`display_safe_area.check_interval_ms`（两次读取该目录之间的
最短间隔，默认 1 000 毫秒，范围 0 到 3 600 000，0 表示每个需要它的帧都读一次）和
`display_safe_area.model`（各实例默认声明的设备机型，默认空，即不声明）；`instances[]` 条目新增
`display_safe_area` 成员（`model`：该实例声明的机型，默认取节里的 `display_safe_area.model`，同样可以
在运行中修改）。现有的配置都不需要修改：没有写这一节和这个成员的配置文件保持原来的 revision，Runtime
仍读 revision 5 到 14 的文件。DA-4b-D 之前的 Runtime（包括 v0.12.15）拒绝写了这一节或这个成员的配置
文件，因为配置文件和实例条目都不接受未知成员；退回旧版本之前要先去掉它们。

运行上的变化：

- 启动：打开账本不再读出整段历史的全部所有者记录（LN-7c O；生产账本上约 7–9 秒、约 300 MB，均为估计），只读最近两个所有者纪元的所有者记录，以及启动可能还要收尾的更早所有者的记录，保留的所有者的状态与读全部时相同。上一轮留下了账本转储文件、或有所有者落不进任何一次启动的纪元时，照旧读全部，并打印一行 `actingd ledger_open owner_read=full reason=…`（`dump_present` 或 `owner_in_no_start_epoch`）。本版第一次以写入方式打开账本时，还要把帧索引 `ledger_frame_refs` 补建一次（LN-7c R；生产账本上约 8.5 万条记录、约 5–10 秒，均为估计）：按页进行（每页最多 `ledger_observer.history_page_rows` 条），每页之前检查打开账本的时限，每页提交一次；没有补完不会让打开失败，只写一条警告，下次打开时接着补。以后每条记录的索引行与记录在同一个事务里写入。旧版本照常打开、写入和复查带这张表的账本，只是不写它；再用本版打开时补上缺的部分。
- 不再因一次运行的诊断停下 Runtime（DIAG-RUN）：每次运行结束时，任务诊断要读出该运行的 `artifact.verified` 记录。这个读取走的是事件类型索引，从序号 0 起走过整段历史里所有这类记录（生产上约 10 万到 17 万条）；10 月 26 日 23:23:04 它没能在写入线程的应答时限（`writer_reply_timeout_ms`，10 秒）内答复，这个失败又被当成致命故障，于是一个实例的一次运行停下了整个 Runtime。v0.12.15 中这个读取与 v0.12.13 相同（LN-WS 只限制视图分页和回放页）。现在这个读取走该运行的链接索引，工作量只随该运行的记录增长；写入线程没有按时答复、或读取用完预算时，只让该运行失败，带上账本的结果码（`writer_response_timeout` 或 `ledger_read_budget_exceeded`，操作 `task_diagnostic_failed`）；账本的其他故障仍然停下 Runtime。错误帧定位、策略运行的关联事件等按运行读取的其他读取也改走运行的链接索引。
- 读取确认（LN-7c C）：新增 MCP 工具 `ac_evidence_confirm_read`（operator 档，该档从 8 个工具变为 9 个）和命令 `actingctl evidence confirm-read`（`--artifact <id>` 或 `--run <id>`），确认某个证据帧或某次运行的帧已被代理或人读过：记一条 `client.action`，帧索引里记一行 `read_confirmed`。不存在的帧或运行以 `client_action_invalid` 拒绝，什么都不记；只打开或查看帧（`ac_run_evidence`、`ac_observe`）不算确认。本版不删除任何东西，按确认删除帧随 LN-7c Q 实现。
- 声明式安全区（DA-4b-D）：声明文件（`actingcommand.display-safe-area.v1`）写一个或多个设备机型，以及它们每种显示大小和旋转的四边 insets；实例用上面的设置声明自己的机型。Runtime 在运行中读取：修改声明文件（按 `check_interval_ms` 重读目录）或实例的机型设置，从下一帧起生效（记为 `changed`，`insets_changed`，映射重建），不用重启。只影响显示声明按安全区量的游戏的运行，全屏（`screen`）布局不受影响。没有声明机型时不读声明文件：模拟器的 insets 取 0、运行照常（v0.12.15 中以 `contained_task_geometry_unavailable` 拒绝），真机照旧以 `contained_task_geometry_unavailable` 拒绝。声明文件坏了、一个机型出现在两个文件里、声明的机型没有文件、文件里没有该帧的显示大小和旋转时，该运行在第一帧、发送任何输入之前以 `display_map_unresolved` 结束（`display_problem` 分别是 `safe_area_declaration_invalid`、`safe_area_model_repeated`、`safe_area_model_undeclared`、`safe_area_form_undeclared`）。Runtime 不解析屏幕的刘海和挖孔，也不识别设备机型。

本版合并的 PR：

- #1362 LN-7c R：账本新增帧索引 `ledger_frame_refs`：记录只在载荷里提到的帧、产物和运行（摘要帧、摘要固定、资源读数、发布的帧和产物、输入前的帧、去重保留的帧、读取确认），按键可查，留给 LN-7c Q 按查询保留帧；与记录在同一个事务里写入，第一次以写入方式打开时补建一次（见上）；后台复查和 `ledger-maintenance verify` 也核对这张表，复查发现不符时只在 actingd 日志写一行、Runtime 照常运行，下次打开时重建。没有新键、新结果码。
- #1363 DA-4b-D：声明式安全区（见上）：声明文件按机型给出 insets，实例用设置声明机型，修改后从下一帧起生效；`actingd-config` [5, 15]；四种拒绝沿用 `display_map_unresolved`，`display_problem` 新增四个值。没有新结果码。
- #1364 LN-7c O：打开账本只读最近两个所有者纪元的所有者记录（见上），保留的所有者的状态与读全部时相同；有转储文件或所有者落不进启动纪元时读全部并打印一行原因。只读不写，旧版本照常打开同一个账本。没有新键、新结果码。
- #1366 LN-7c C：确认证据帧或运行已读：MCP `ac_evidence_confirm_read` 和 `actingctl evidence confirm-read`（见上）。本版只记录确认，不删除任何东西。没有新键、新结果码。
- #1367 DIAG-RUN：修 10 月 26 日 23:23:04 的停机（见上）：按运行读取的查询走该运行的链接索引；任务诊断的读取没有按时答复或用完预算时只让该运行失败，带上账本的结果码。没有新键、新结果码。

## v0.12.15

v0.12.14 没有上线：10 月 26 日部署时，启动停在打开账本这一步、一直没有就绪，已退回 v0.12.13。原因是
LN-7b 的帧保留读取按页读链接集，每读一页都要把该页之后的全部链接记录排序一次，读取量随记录数的平方
增长；生产账本的现有帧分布在整段历史里（1 323 次运行、约 40 万条记录），这次读取读不完，打开账本的
时限也没有在它里面检查。CI 的规模测量没有发现，因为合成账本里没有存帧。

所以 v0.12.15 是 v0.12.13 之后第一个上线的版本：它包含 v0.12.14 的全部内容（见下面 v0.12.14 一节），
另加 v0.12.14 之后合并的 13 个 PR（修复，以及为修复服务的功能）。本版第一次准备的发版快照（#1355）没有
发布出去：它合并之后 #1356 改了主线上的工作流文件，在工作流文件与主线不同的提交上打新标签需要 Actions
令牌没有的 `workflows` 权限，三次发版运行都在构建完成后被拒绝（HTTP 403），#1359 让发版在构建之前就拒绝
这样的源提交并保留发版文件，所以本版从当前主线重新准备。

配置：`actingd-config` 升到 [5, 14]。LN-WS 新增两个键，`ledger_observer.read_check_vm_steps`（默认
10 000，范围 100 到 1 000 000）和 `ledger_observer.history_scan_rows`（默认 65 536，范围 1 024 到
1 048 576），都在打开账本时读取，运行中不能改。现有的配置都不需要修改：没有写这两个键的配置文件照常
读取，Runtime 仍读 revision 5 到 13 的文件。

运行上的变化：

- 启动：打开账本不再因现有帧分布在整段历史里而停住。打开仍要读出每个有现有帧的运行的全部记录（生产账本上约 40 万条），现在每条只读一次，读取量随它们线性增长；每读一页都检查 `ledger_observer.open_timeout_ms`（默认 600 秒），超时以 `ledger_read_budget_exceeded` 失败，不会一直读下去。按生产账本快照上的回放估计，生产账本上打开账本约需 40–90 秒；这是估计，Rust 实现还没有在生产账本上实测，“只读头部和尾部”的目标随 LN-7c 实现。
- 读取不再能让 Runtime 停下（LN-WS）：10 月 26 日 19:12 生产上那次停机，是一个不带实例、时间窗 8 分钟的 `ac_events` 读取在账本写入线程上作为一条 SQLite 语句从头扫了约 158 万行（15 到 18 秒），排在它后面的一次调度追加等满 `writer_reply_timeout_ms`（10 秒）后成了致命的 `ledger_failure`。现在为请求服务的读取（视图分页、订阅的回放页）在一条语句内部也按 `read_budget_ms` 看时钟，用完就中断：分页返回不完整（`read_budget`），回放页返回 `ledger_read_budget_exceeded`；视图分页的每条历史语句只跨有限的序号范围，带时间窗的第一页从窗口里的第一条记录开始读。排在读取后面的写入只等大约一个读取预算（默认 1 秒），不再超时；`ledger_failure` 现在带上账本给出的原因。
- 空闲关掉的游戏会在下一个任务之前被拉起（IDLE-2）：v0.12.13 在生产上，空闲中的实例只要某次检查碰到它暂时被占用，就回到活动状态、忘掉唤醒，下一个任务遇到的是已关闭的游戏（`contained_task_page_unknown`、恢复阶梯、冷启动）。现在空闲实例在任何占用中都保留唤醒，唤醒是它唯一的出口；`close_app` 发出关闭请求后失败的，也照样唤醒。
- 截图闸门（DA-2a）：每一帧先经过显示适配器的帧闸门。帧和任务包的坐标空间一样大时是恒等映射，记录和输入逐字节和以前一样；宽高比相同、比包大的帧缩放一次到包的大小再识别；其他大小（平移、裁剪、旋转、比包小的帧）以 `display_map_unresolved` 拒绝。不是恒等映射的帧上不发送任何输入（`display_map_unresolved`，`input_scale_unsupported`），直到 DA-3。按安全区量的声明找不到 insets，以 `contained_task_geometry_unavailable` 拒绝，直到 DA-4b。以前尺寸不符的帧以 `contained_task_frame_resolution_mismatch` 拒绝，这个码本版退役。本版没有资源仓带 `display-layout.json`，没有这个文件的游戏根目录照旧。
- 帧时间索引（F6）：第一次打开账本时建立 `ledger_artifacts_created`，按设计估计不到 1 秒（约 10 MB），以后的打开不再重建；现在还没有读取方。旧版本照常打开带这个索引的账本。
- 黑名单：黑名单时段（主机列表、实例自己的列表、目录的 `maintenance` 事件）内该实例完全静默：调度不派任务，恢复阶梯不启动，启动包和模拟器自动启动（包括 EMU-W 监视和 IDLE-1 唤醒排上的）等到黑名单结束后才做，监视探针不截图也不记录；黑名单开始前，预计会跨进去的任务以 `blackout_approaching` 推迟，实例逐渐静下来。直接运行、Lab 操作和操作员的控制不受影响；两个列表都为空、目录也没有 `maintenance` 事件时一切照旧。
- 预计时长 E：任务的预计时长 E 取该任务包在该实例上最近 `scheduling.expected_runtime.runs` 次（默认 5）成功调度运行的平均时长加 `margin_ms`（默认 5 分钟），评估、准入和记录都按 E；`ac_schedule` 的候选行显示 `expected_runtime_ms` 和 `expected_runtime_source`（`declared` 或 `runs:<n>`）。还没有样本的包，以及重启后在本进程内第一次被调度运行之前，用声明的时长；`scheduling.expected_runtime.enabled = false` 关闭 E。
- 日界：默认开启（`scheduling.day_boundary.quiet` 为 `true`）：按 E 在当前活动时段结束前跑不完的任务等到下一个时段开始（目录 v33 的活动时段是 06:00–23:30），较短的任务照常跑到时段结束；操作员在 `ac_schedule` 里看到它的 `why` 是 `policy_day_boundary_approaching`，带 `window_end_unix_ms`、`required_ms` 和 `next_eligible_unix_ms`。不要这个行为就把该键（或实例自己的键）设为 `false`。

本版合并的 PR：

- #1345 SR-4a3：黑名单时段内 Runtime 自己发起的动作完全静默：恢复阶梯直接丢弃（`recovery_ladder_suppressed`，原因 `blackout`），正在爬的阶梯不再启动时限加 `scheduling.blackout.quiet_margin_ms` 会碰到下一个黑名单的那一级，人工演练被拒（`recovery_admission_denied`）；启动包在黑名单内、或其时限加 `quiet_margin_ms` 会碰到下一个黑名单时排队等待；模拟器自动启动（包括 EMU-W 监视发现模拟器停了时的那一次，以及 IDLE-1 唤醒排上的启动包和启动）在黑名单内不取钥匙、不启动，结束后执行，IDLE-1 的闲置动作本身也暂停；监视探针不截图、不记录，结束后第一轮补做；直接运行、Lab 操作、操作员控制和人工自检不受影响。没有新键、新结果码。
- #1347 SR-4a2：调度策略按黑名单收口：三个来源的黑名单在周期开始时按实例判一次，覆盖中的实例的候选以 `blackout_active` 推迟到结束（位置 4）；候选的时间范围（默认是响应时限，`scheduling.blackout.quiet_horizon` 为 `expected` 时是 E）加 `quiet_margin_ms` 会越过下一个黑名单的开始、或已进入 `hard_quiet_ms` 时，以 `blackout_approaching` 推迟（位置 7）；随后是日界：`scheduling.day_boundary.quiet` 为真（默认）的实例上，按 E 在活动时段结束前跑不完的运行以新结果码 `policy_day_boundary_approaching` 等到下一个时段；准入时在准入锁下按准入时刻再判一次位置 4 和 7（拒绝记为 Info，不取租约）；新抽的随机延迟截在静默点之前，日界适用时也截在时段结束减 E 之前；`config.subsystems` 新增常开的 `scheduling_blackout`（`count` 为主机列表的条数）；`ac_schedule` 把这三个码作为 `why` 显示。没有新键。
- #1350 ED-2：运行时长样本 `health.run_duration`：每个实例一条记录、按任务包分行，成功的调度运行在结算时把 `runtime_ms` 记为该包最新的样本（失败、条件结束、直接运行、Lab 操作和重放不记），每行保留最近 `scheduling.expected_runtime.runs` 个；预计时长 E（近几次平均加 5 分钟）在评估、准入和记录中生效：准入时 E 与评估时不同则 `policy_facts_stale`，预留时长不是声明值时 `policy.dispatch_intent` 写可选成员 `expected_duration_ms`，恢复与重放按它预留；事实刷新的声明时长改读 `scheduling.fact_refresh.expected_duration_ms`；该键只由 Runtime 写，其他发布被拒（`run_duration_key_reserved`），一个实例超过 256 个包等情况丢掉一行时写警告 `run_duration_row_dropped`；`ac_schedule` 候选行新增 `expected_runtime_ms` 与 `expected_runtime_source`；`config.subsystems` 新增常开的 `expected_runtime`。没有新键。
- #1352 LN-8d：修 v0.12.14 启动停住：链接集的历史分页每页先在各维度自己的索引上选出该页的序号（在索引里排序和截取），再按主键读记录，每页的工作量不再随该页之后的链接记录增长；帧保留读取的 `read_linked` 把一个链接集的各维度一次读完，一个运行的记录只读一次、只验一次；打开账本在每页和每条范围语句之前检查时限，超时以 `ledger_read_budget_exceeded`（操作 `open_runtime_ledger`）失败；分页大小取 `ledger_observer.history_page_rows`（以前用固定的默认值）。新增三个回归测试，规模测量的合成账本改成生产的帧存储形态（每 16 帧存一帧并保持在用，分布在整段历史里）。没有新键，账本结构不变。
- #1353 DA-5a：任务包 schema 版本的约 69 处字符串比较（和 `"0.9"` 比）换成一个有序的版本类型，以后加 0.10 时编译器会停在每一处要定规则的地方。纯重构，行为不变，没有新键、新结果码。
- #1354 DA-4：标准包的游戏根目录可以放显示声明 `display-layout.json`（`actingcommand.display-layout.v1`：按高还是按宽缩放、在全屏还是安全区上量、锚点区域、修正半径），启动记录、运行准备、调度工作台、`actingctl packs`、`package bundle --display-layout`、`actinglab standard-pack check` 和 `compose`、`resource verify` 都用同一个检查；坏文件在启动时只记录，运行准备时在取租约之前拒绝该运行（`standard_pack_root_invalid`，修正半径超过 64 是新问题 `display_layout_invalid`）。没有新键。
- #1355：v0.12.15 第一次的发版快照（结果码发布文件和本说明），没有发布出去（见上）。
- #1356 DA-1：新 crate `display-adapter`：一帧的显示映射（恒等、纯缩放、平移）、九宫格锚点、取整规则和安全区算术的纯函数，不做 I/O，本身不改变运行行为；新结果码 `display_map_unresolved`（错误）和 `display_map_stale`（警告，只登记，DA-3 才发出）。没有新键。
- #1357 DA-2a：显示适配器的截图侧：六个截图入口（入口页、导航定位、采样、页面帧、读数一致帧、Lab 试运行选择）都经过同一个帧闸门（见上）；识别证据仍按设备帧的大小和设备上的框记录；运行准备时读到的 `display-layout.json` 在整个运行中不变；帧的大小、旋转或 insets 变了时重读几何，记为 `changed`，运行继续；退役 `contained_task_frame_resolution_mismatch`（由 `display_map_unresolved` 取代）。没有新键。
- #1358 IDLE-2：空闲中的实例在任何占用中都保留唤醒，唤醒是它唯一的出口（见上）；`close_app` 在发出关闭请求之后失败、结果不确定时也欠一次唤醒；`stop_emulator` 的失败照旧不唤醒，因为模拟器监视会重启停掉的模拟器。没有新键、新结果码。
- #1359：发版运行保留它要发布的文件（两个 zip、`SHA256SUMS` 和发布说明，作为 Actions 产物 `release-files-<标签>`），发布被拒时可以用同样的字节手工补发；发版的准备步骤在构建之前拒绝 `.github/workflows` 与主线不同的源提交。只改发版工作流，Runtime 不变。
- #1360 LN-WS：任何读取都不能让写入失败（见上）：为请求服务的读取在语句内部每 `read_check_vm_steps` 个 SQLite 步骤看一次时钟；视图分页的每条历史语句最多跨 `history_scan_rows` 个序号，跨度从 1 起逐次加倍，每页总能向前推进；带时间窗的第一页从窗口里的第一条记录开始；追加失败时 `ledger_failure` 把账本的结果码记为原因（`caused_by`）。两个新键，`actingd-config` [5, 14]；没有新结果码。
- #1361 LN-7c F6：账本新增帧时间索引 `ledger_artifacts_created`（只含 `capture.frame` 的产物），以写入方式第一次打开时建立，以后的打开不重建；现在还没有读取方，留给 LN-7c 后面的切片；旧版本照常打开、写入和复查带这个索引的账本。另加账本测试用的查询计划检查。没有新键、新结果码。

## v0.12.14

本版是 v0.12.13 之后合并的 6 个 PR（修复，以及为修复服务的功能），主要是 #422 账本精简的收尾：
启动打开账本时不再读整个账本。`actingd-config` 升到 [5, 13]：LN-8 和 LN-9 的新键都有默认值，
没有写这些键的配置文件照常读取。

运行上的变化：在 100 万条记录的合成账本上，稳定启动（规模测量中同一账本的第三次启动）约 0.6 秒
就绪，其中打开账本约 0.26 秒，启动过程中的私有内存峰值约 43 MiB（规模测量运行 40257524915 与
40262403978）；v0.12.13 的同一测量中峰值约 4 500 MiB（运行 40242733023）。启动打开账本时只校验
最新的 `ledger_observer.open_verify_tail_records` 条记录（默认 4096）；整条链由后台复查按节奏从头
校验，大约每 40 分钟一轮，发现旧记录损坏时以完整校验对同一损坏给出的同一结果码作为全局故障停下
Runtime。`ledger-maintenance verify` 仍然校验全部记录。调度规则现在可以写“事实 A ≥ 事实 B + 偏移”，
用到它的 `schedule.json` 要在 `requires` 里写 `fact_reference`。工具修了 RL-T 的两处：
`actinglab standard-pack compose` 能收到 `--instance`，`actinglab resource validate` 能读资源仓
根目录的 `schedule.json`。

- #1342 LN-7b：帧保留状态有上限：写入线程只常驻删除证明集、所有者的变化与结束、启动可能还要收尾的范围，以及帧存储中现有帧和它们的分类要读的记录；打开账本时按事件类型和索引读出这些，帧清理每轮开始时丢掉其余部分，记录引用了已不常驻的运行或帧时先按索引读回再检查。没有新键。
- #1343 LN-8：启动打开账本只读 meta 行、`MAX(sequence)`、最新的 `ledger_observer.open_verify_tail_records` 条记录（新键，默认 4096）和索引；启动不再对状态数据库做整文件的 `quick_check`（`ledger-maintenance verify` 照旧做）；`ledger_opened` 新增各阶段耗时 `meta_ms`、`tail_ms`、`index_ms`、`import_ms`；规模测试的目标从本版起是门禁。
- #1344 LN-9：后台按节奏复查整条链：用自己的只读连接分块校验每条记录，一轮结束时做一次 `quick_check`；新键 `ledger_observer.scrub_rows_per_second`（默认每秒 625 行，运行中可改）、`ledger_observer.scrub_chunk_rows`（默认 256）和 `ledger_observer.scrub_pass_interval_ms`（默认 40 分钟）；发现损坏时作为全局故障停下 Runtime（裁定 57）。
- #1346 CF-4：V2 `fact` 条件的 `value` 可以是同一作用域另一个事实加整数偏移（`{"type": "fact", "fact_key": …, "offset": …}`）；被引用的事实给不出整数时条件为假，原因 `fact_reference_unavailable`；写了引用的 `schedule.json` 要在 `requires` 里写 `fact_reference`，Runtime 在 `config.subsystems` 里报告常开的 `fact_reference`。
- #1348 RL-T：修两个工具缺陷：`actinglab standard-pack compose` 收到写在命令之后的 `--instance <别名>=<游戏>/<服务器>`（以前总被当成全局选项，结果报缺少 `--instance`）；`actinglab resource validate` 用 RL-1a 的读取器读资源仓根目录的 `schedule.json`（以前因没有对应的解析器而失败）。
- #1349 LN-8c：启动时运行时事实的恢复按页读取（每页 `ledger_observer.history_page_rows` 条，最多 1 024），不再一次读出最新快照集之后的全部记录；100 万条记录时单次最大读取从 8 424 条降到 256 条。没有新键。

## v0.12.13

本版是 v0.12.12 之后合并的 13 个 PR（修复，以及为修复服务的功能），大部分属于 #422 账本精简：
账本历史留在磁盘上，启动不再重放整个账本。`actingd-config` 升到 [5, 12]：新键都有默认值，
没有写这些键的配置文件照常读取。

运行上的变化：在 100 万条事件的合成账本上，启动完成后的内存从约 2.4 GB 降到约 0.13 GB
（规模测量运行 40215455859 与 40242733023），启动过程中的峰值仍约 4.4 GB。本版第一次启动时要把
标记状态播种一次（写 `ledger_tagged_state_seeded`），并给账本建一个新索引，所以比平常长（同一测量中
约 345 秒，之后约 83 秒）；以后的启动直接从标记载入。RL-1b 的调度工作台只在 `policy` 节不写
`catalog`（组合模式）时才工作，写了 `catalog` 的配置不受影响。IDLE-1 默认开启：`idle.action`
默认是 `close_app`，配置中的实例在调度器看来 30 分钟（`idle.min_gap_ms`）内没有任务时，Runtime
关闭模拟器里的游戏，在下一个任务前 1 分钟（`idle.close_app_wake_lead_ms`）用启动包拉起；不要这个
行为就把 `idle.action` 设为 `none`（也可以按实例设）。`close_app` 要从标准包根目录下该游戏的
`applications.json` 读出该服务器的应用 id，读不到时这次动作失败并记录，只影响该实例；没有启动包的
实例不动作，只写一条警告 `instance_idle_action_unsupported`。

- #1326 LN-5：账本历史只留在磁盘（SQLite）上，写入线程不再在内存里保存全部记录及其索引；历史读取按页读 SQLite，每页 `ledger_observer.history_page_rows` 行（默认 256），追加时的检查改为索引查找。
- #1327 A67-R：帧存储的内存预算落到下限时，警告 `frame_store_budget_floor_applied` 写进 actingd 日志和账本，各一次；离开下限后再落下会再记一次。
- #1329 LN-2：启动时实例基础信息库从账本索引的标记状态行载入，不再重放整个账本；索引行与记录不符时，启动以 `ledger_tag_index_mismatch` 停下。
- #1330 RL-1a：标准包 `schedule.json`（`actingcommand.schedule.v1`）的读取和组合规则的纯函数；工具 `standard-pack check` 与 `package bundle --schedule` 检查 `schedule.json`，新增 `standard-pack compose`。
- #1332 ISO-2：单个实例的设备故障和执行内核故障只隔离该实例或让该操作失败，不再停整个 Runtime；只有登记为全局的故障才会停。
- #1333 IDLE-1：实例近期没有任务时关闭模拟器内的游戏（`close_app`，默认）或停掉模拟器（`stop_emulator`），在下一个任务前拉起；新增 `idle` 节和实例的 `idle` 成员（裁定 41）。
- #1334 LN-4：调度的决策记忆、派发日志、唤醒义务和监视登记从标记记录载入；还没有标记的启动先重放一次，写入这些记录和标记；常驻的派发日志有上限，其余按需读取。
- #1335 RL-1b：Runtime 内的调度工作台：组合模式下按在用的游戏组合标准包的 `schedule.json`，用到时载入、不用时卸载，不需要批准；新键 `standard_packs.check_interval_ms`（默认 60 秒）和 `standard_packs.cached_generations`（默认 1）。
- #1336 LN-DIAG：Release 基线的读取改为按事件类型选取，100 万条事件的第一次启动不再因 `writer_response_timeout` 失败。
- #1337 LN-TUNE：账本写入线程的应答等待改为配置键 `ledger_observer.writer_reply_timeout_ms`（默认 10 秒，原为固定值）；分页的历史读取每行只读一次，监视登记播种时的重放约减半。
- #1338 LN-7 第 1 部分：启动时只收尾紧接的上一个纪元，日志导入按所有者只检查一次；`owner_epochs` 只记所有者的变化，不再随历史增长。
- #1339 LN-6：视图分页和产物解析直接读 SQLite，写入线程不再保留已验证的历史前缀。
- #1340 LN-4 后续：测试用的参照读取改为分页；`agent_ledger_failure` 保留账本的原始错误细节，日志里能看出原因。

## v0.12.12

本版是 v0.12.11 之后合并的 17 个 PR（修复，以及为修复服务的功能）。`actingd-config` 升到 [5, 11]：
新键都有默认值，没有写这些键的配置文件照常读取。

- #1295 A64：到时限的运行在收尾阶段遇到租约过期，只隔离该实例、不再停整个 Runtime；这类运行在收尾宽限 `host_claims.run_end_grace_ms`（默认 120 秒）内结束（裁定 C-43）。
- #1298 A65、A66、A67：启动时结算上一轮的运行改为按页读账本；看门狗的启动窗口改为配置键 `watchdog.start_window_ms`（默认 900 秒，原为固定 180 秒）；可用内存扣掉系统保留量后低于下限时，帧存储按下限 `frame_store.floor_mem_bytes`（默认 256 MiB）打开并写警告 `frame_store_budget_floor_applied`，不再启动失败。
- #1303 PERF-B1：步骤的等待可声明自己的截图间隔，上限为 `execution.capture.max_interval_ms`（默认 60 秒）；等待中画面不变时，下一次间隔逐次加倍。
- #1304 CF-1：调度条件边：`dependency_completed.since_own_run` 让后继任务对前驱的每个新结果只跑一次，`fact_time_reached.once_per_reading` 让同一个完成时刻只用一次。
- #1306 INST-0：命令行安装器的契约（`install-cli.md`）、相关契约修订和 `install_*` 结果码；只有文档和登记，行为不变。
- #1308 SR-4a1：时段黑名单和跨日判断的纯函数，新增按实例的和一次性的黑名单条目；尚未接入调度，行为不变。
- #1309 EMU-W：调度暂停不再挡住模拟器启动；新增模拟器监视，配置为自动启动的模拟器停掉后会在限定时间内重新拉起（裁定 35）。
- #1311 DB-1：标准包游戏根目录下的共享表 `tables/`（`actingcommand.table.v1`）及其读取；启动时只报告被拒的表，运行还不读表。
- #1312 CF-2：资源读数新增 `timestamp_ms` 类型，把画面上的倒计时（H:MM:SS）加上截图时间，作为完成时刻发布。
- #1315 INST-1a：`actingctl install release verify` 与 `stage`、`install adopt`、安装操作日志与 `install resume`。
- #1318 ED-1：任务预计时长 E（该任务近几次耗时的平均值加余量）的纯计算和配置键 `scheduling.expected_runtime.*`；尚未接入调度，行为不变。
- #1320 ISO-1：故障范围核心：每个故障归为全局、单实例或单操作，只有全局故障才会停整个 Runtime；回执只在 Runtime 真的停下时才报 `host_halted`（A54）。
- #1322 #420 S5：移除按源码文本钉死的架构守卫，只保留真实边界的检查。
- #1323 #420 S2、S3：合并后的结果码目录改为构建时生成、不再提交；各版本的结果码快照改为紧凑格式。
- #1324 #420 S1、S6：门禁映射的新行不再需要手工分配的编号；计数断言改为对照实际清单。
- #1325 LN-1：合成账本生成器和规模测试工具（只有测试和工具）。
- #1328 LN-3：调度工作台的决策记忆、唤醒义务和监视登记改为写值记录。

## v0.12.11

账本打开时限进入配置（P0-LO）：启动时打开账本的时限由固定 120 秒改为配置键
`ledger_observer.open_timeout_ms`，默认 600 秒（可设 10 到 600 秒，只在启动时读取）。
打开的实际耗时超过该时限的 `ledger_observer.open_warn_ratio_milli` 千分之（默认 500，
即一半）时，actingd 紧跟 `ledger_opened` 写一条警告 `ledger_open_slow`（`elapsed_ms`、
`timeout_ms`、`event_count`）。原因：账本持续增长，打开耗时随之增长，按原来 120 秒的时限，
开机时的打开很快会超时，Runtime 就无法启动。`actingd-config` 升到 [5, 10]；没有写这两个键的
配置文件照常读取。

Ledger open deadline in configuration (P0-LO): the deadline of the ledger open at
start is the configuration key `ledger_observer.open_timeout_ms`, default 600 s
instead of the fixed 120 s (10 to 600 s, read only at start). When the open takes
longer than `ledger_observer.open_warn_ratio_milli` thousandths of it (default 500,
half), actingd writes the warning `ledger_open_slow` (`elapsed_ms`, `timeout_ms`,
`event_count`) right after `ledger_opened`. Why: the ledger keeps growing and the
open's time with it; under the old 120 s deadline an open at boot would soon run
out of time and the Runtime would not start. `actingd-config` rises to [5, 10]; a
configuration that names neither key is read as before.

The Windows `x86_64-pc-windows-msvc` release-profile Runtime artifact includes the
resident `actingcommand-actingd.exe`, the `actingctl.exe` command client, an
editable configuration template, `INSTALL.md` and these notes. The manifest's
`runtime_payload_layout` is `distribution-v1`; all five payload files are required
and individually bound by size and SHA-256.

The existing exact-artifact downloader understands this layout and the historical
fixed two-executable Runtime layout whose manifest omits the field. It continues
to reject incomplete or unexpected payloads. The separate Tools artifact carries
`actinglab.exe`, `actingledger.exe`, `actingcommand-vision-provider-check.exe`,
the watchdog launcher `actingwatch.exe` and,
under `platform-tools/`, the official Android platform-tools 37.0.1 files
`adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll`, `NOTICE.txt` and
`source.properties`. Its own manifest declares
`tools_payload_layout: "platform-tools-v3"` and binds all nine files.
The OCR and NN engine is linked into `actingcommand-actingd.exe`; no vision
provider DLL ships (Workflow #360), and actingd reads its models from the vision
root named by the configuration's `vision` section. The downloader still accepts
`platform-tools-v3` Tools artifacts built before v0.11.6 (the same files plus
`actingcommand-device-test.exe`), the historical `platform-tools-v2` layout (those
ten files without `actingwatch.exe`), the historical `platform-tools-v1` layout (the
`platform-tools-v2` files plus `ac_fastdeploy_ppocr.dll`) and the five-file Tools
layout whose manifest omits the field, and rejects any other layout.

The device-test probe `actingcommand-device-test.exe` is retired (v0.11.6), and the
Tools artifact no longer contains it.

`actingctl watchdog status` / `run-once` and the launcher `actingwatch.exe`
(Workflow #374) start an A/B installation's Runtime again after an end without a
formal close, and stay down after a FATAL; see `INSTALL.md`, "Runtime watchdog".

The build takes `platform-tools_r37.0.1-win.zip` only from Google's official
`https://dl.google.com/android/repository/` URL and fails unless the archive has
the size and SHA-1 that Google publishes, the pinned SHA-256, and each shipped
file its pinned size and SHA-256, and unless `adb.exe version` reports
`37.0.1-15733141`. Of the archive's binaries only `adb.exe` and its two DLLs are
shipped; Google's `NOTICE.txt` and `source.properties` stay unchanged beside them.
They are redistributed by the owner's decision under the Android SDK License and
the open-source licenses in `NOTICE.txt`. Installed, they are in
`<install root>\tools\platform-tools\`, and
`<install root>\tools\platform-tools\adb.exe` is the adb that an instance without
`adb_path` (key omitted) uses.

Real-device screen stream (#402 RD-3): `actingcommand-actingd.exe` carries the
official scrcpy server 5.0.1 (`scrcpy-server-v5.0.1`, 733930 bytes, SHA-256
`764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536`), which the
build takes only from the scrcpy GitHub release and checks against its pin, and
OpenH264 compiled from source (Cisco's OpenH264 bundled by the `openh264-sys2`
0.9.8 crate). The capture backend `scrcpy` pushes that server to a real device's
`/data/local/tmp`, starts it without its control and audio channels and without
any option that changes a device setting, and decodes its H.264 stream; it is never
chosen automatically and is refused on emulator instances
(`device_config_invalid`).

The scrcpy server: Copyright (C) 2018 Genymobile, Copyright (C) 2018-2026 Romain
Vimont, licensed under the Apache License, Version 2.0
(http://www.apache.org/licenses/LICENSE-2.0); distributed on an "AS IS" basis,
without warranties or conditions of any kind. OpenH264: Copyright (c) 2013, Cisco
Systems, all rights reserved; the `openh264` and `openh264-sys2` crates: Ralf
Biedert and contributors. Both are distributed under this BSD-2-Clause licence:
Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met: redistributions of
source code must retain the above copyright notice, this list of conditions and
the following disclaimer; redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution. THIS SOFTWARE
IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT
SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR
BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN
ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH
DAMAGE.

The daemon recognises an install root from its own path: any directory named
`runtime` that holds this artifact with its `BUILD-MANIFEST.json` makes its
parent one (an acsetup install, acsetup's upgrade staging, or a hand layout). There an instance without `adb_path`, explicit or discovery-bound,
uses that adb; startup and `check-config` compare the SHA-256 of `adb.exe`,
`AdbWinApi.dll` and `AdbWinUsbApi.dll` with the build's pin before the ledger
opens and refuse with `adb_install_missing` or `adb_install_mismatch`, with no
fallback; a discovery-bound `adb_path` may name the discovered MuMu adb or the
install root's adb. Outside an install root behaviour is unchanged.
`check-config` reports the adb as `adb_default`. See `INSTALL.md`, "Bundled adb".

Before rolling back to v0.9.0, give every explicit instance without `adb_path`
an adb that still exists afterwards (MuMu's own or a separate 37.0.1 copy, not
the install root's), and remove an `adb_path` that names the install root's adb
from discovery-bound instances; otherwise v0.9.0 refuses with
`instance_config_invalid` or `instance_discovery_conflict`. When `ui\` is the
working directory of a running adb server, v0.9.0's acsetup reports that the
console must be closed first although it is closed; stop the adb server and
retry (this disconnects other tools sharing port 5037), or roll back by hand
(see `INSTALL.md`, "Upgrade boundary").

Upgrading in place from v0.11.1, v0.11.2 or v0.11.3 while the Runtime runs
(Workflow #376): the old daemon's policy monitor fails when acsetup's drain
closes admission, before `commit_shutdown` is read, so the installer can end with
a connection reset and an unchanged selection. This is likely when the Runtime is
idle and certain while it is running work. For these upgrades, first close the
old Runtime formally (`actingctl request-shutdown --state-root <state-root>
--wait 60`; if it is refused as busy, retry once it is idle), then run acsetup,
which takes the cold ledger gate, then start the Runtime. That cold gate runs
the old version's `ledger-maintenance verify`, which reads every frame twice in
two passes under a 120 s deadline and refuses ledgers above 200,000 events: on a
large state root (for example 130k events and 30 GB of frames) it fails with
`ledger_read_budget_exceeded`, and the upgrade stops with the selection
unchanged. Such a root cannot be upgraded through either path of the old
version; check its verify time on a copy before upgrading. From this release on
the resident daemon waits through a drain, resumes after an abort or the drain
timeout, and ends with the accepted `commit_shutdown`. A drain that work in
flight holds past its timeout (60 s by default) still stops the installer, and
the Runtime keeps running.

Ledger verification and the start (Workflow #375): from this release on,
`ledger-maintenance verify` and the ledger open at start verify each distinct
frame once, on up to eight workers, in one pass bounded by the authenticated
head, with no event-count cap; a frame deleted with an authenticated retention
proof is accepted. The daemon prints one stdout line after the open, also
before a failure: `actingd ledger_open events=… artifacts=… artifact_bytes=…
workers=… sql_read_ms=… verify_ms=… material_ms=… restore_ms=…
deadline_ms=120000`. Follow its timings as the state root grows: the open
still has a 120 s deadline, and acsetup waits 60 s for a Runtime it starts.

Kept frames (Workflow #375): this release moves the screenshots it keeps for
errors and for Lab into `<state root>\kept\<date>\…`. Delete a folder to delete
them, with Shift+Delete or `Remove-Item -Recurse`; Explorer's Delete keeps them in
the Recycle Bin. Other screenshots are deleted automatically: near-duplicates once
their run has ended, resource readings after 7 days, the rest after a day.
Near-duplicates inside the 30 s before an error are also deleted, and Lab
screenshots are all kept; to change either, add `"frame_retention_dedup_error":
false` or `"frame_retention_dedup_lab": true` to `actingd.config.json`, commit it
with `acsetup --commit-config` and restart actingd. A change applies only to
screenshots not yet moved into a folder. The cleaner runs while
`frame_retention_enabled` is on (the default), and prints one
`actingd frame_retention pass …` line per sweep that visited frames; with it off,
actingd prints `actingd frame_retention disabled` once at start. Keep `kept`
a plain folder on the state root's volume: a junction there makes reads of
kept screenshots fail with `artifact_kept_walk_failed`, and moving a leaf out
by hand counts as deleting it. Once a screenshot has been deleted or moved, or
once the file names either switch, v0.11.4 can no longer open this state root.

check-config's catalog preview (Workflow #375) now reads only the catalog and
approval records (authenticated, with the ledger head and contiguity) instead of
every ledger record; startup and `ledger-maintenance verify` still check every
record. It no longer runs the whole-file `PRAGMA quick_check(1)` when it opens
the database, so its cost no longer grows with the size of
`runtime-state.sqlite` (it was about 13 s at 550 MB on a file not in the cache).
Startup and `ledger-maintenance verify` still run that check.

Rollback: an explicit `acsetup --rollback` runs the target slot's own cold
ledger gate. Rolling back from this release to v0.11.3 or earlier therefore
runs the old verify described above, and on a large state root it fails, which
leaves the installation unchanged. Treat the upgrade to this release as one-way
on such roots.

Configuration uses `actingcommand.actingd.config.v1` and the existing
`actingcommand-actingd --config <path>` entry. The supplied template has empty
private state-root and salt values and no device instances; once `state_root`
and `secret_fingerprint_salt` are filled, the template's `"instances": []` starts
a control-plane-only daemon, and instances are added by editing the
configuration and restarting. Follow `INSTALL.md`
and the exact source schema to provide a private configuration and any required
provider dependencies before use.

This distribution change adds packaging and documentation to the existing
Actions build. Apart from the adb default above, it does not change
daemon/client operation, install dependencies
or services, or perform startup, state migration or device actions. Actions
results establish only their recorded build and check outcomes; installation,
provider availability, real-device behavior and functional acceptance require
their own evidence. Features from other unaccepted candidates are not represented
by these notes. Release version, tag, publication channel and final sign-off
remain separate decisions.
