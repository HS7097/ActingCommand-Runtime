# Windows Runtime distribution

Version: the tag (`vX.Y.Z`) of the `HS7097/ActingCommand-Runtime` Release that
carries this zip; download the zip and `SHA256SUMS` from that Release. The
version exists only in the Release tag, not in the binaries or the manifest.
The same files taken from a pull-request or manually started Actions build are
an unreleased acceptance build with no version. The exact source commit, tree,
Cargo.lock hash, Rust toolchain, target, build profile, Actions run/attempt and
file identities are recorded in the accompanying `BUILD-MANIFEST.json`; the zip
and artifact names bind the complete source SHA.

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
