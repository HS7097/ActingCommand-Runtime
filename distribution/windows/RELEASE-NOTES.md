# Windows Runtime distribution

Version: the tag (`vX.Y.Z`) of the `HS7097/ActingCommand-Runtime` Release that
carries this zip; download the zip and `SHA256SUMS` from that Release. The
version exists only in the Release tag, not in the binaries or the manifest.
The same files taken from a pull-request or manually started Actions build are
an unreleased acceptance build with no version. The exact source commit, tree,
Cargo.lock hash, Rust toolchain, target, build profile, Actions run/attempt and
file identities are recorded in the accompanying `BUILD-MANIFEST.json`; the zip
and artifact names bind the complete source SHA.

## v0.12.60

本版是 v0.12.59 之后的 24 张 PR：5 个直接合并（#1689、#1704、#1706、#1707、#1708）和 5 趟并车（#1697 带 #1690、#1691、#1695；#1699 带 #1692、#1693、#1694；#1705 带 #1698、
#1700、#1701、#1703、#1702；#1714 带 #1711、#1710、#1709、#1712；#1724 带 #1715、#1717、#1716、#1713；车上除了各张的头，只有合并、登记册节号的调整和计数）。按单子分：#453「全新安装根」
的八张（S1 #1698、S2 #1704、S3 #1711、S4 #1710、S8 和 S7 #1693、S9 #1709、S11 #1700、S12 #1712），#437 的六张（MAPP-1c #1690、ORD-1a 和 ORD-1b #1695、MAPP-2b #1689、MAPP-1d #1701、
MAPP-2d #1703、ORD-1c #1713），#455 的四张（T1-R #1691、G-R1 #1694、G-R2a #1702、G-R2s #1708），#463「录制做包」的三张（L1 #1717、L2 #1716、U1 #1715），#450 FL-2（#1692），
#457（#1706），#456 和 #458（#1707）。先说部署：**生产上跑的是 v0.12.58（11-14 起）；v0.12.59 发布了、没有部署。** 从 v0.12.58 直接装本版：**能装，不用任何标志，计划里没有
`ledger_verify`，`defaults_diff` 有三行（两个新键、一张改了默认值的标签表），装上以后能退回 v0.12.58**；跳过 v0.12.59 什么都不坏，它的改动和本版一起到（见下面「升级与退回」）。先装
v0.12.59 再装本版也一样能装，退回的是 v0.12.59。**部署之前要多做两次只读的检查**（G-R2s，见下面「目标」）：用本版的 actinglab 对每个游戏根跑 `standard-pack check`、找
`standard_pack_package_unreadable`；用本版的 `actingd check-config` 读安装的配置，确认每个游戏根都组合得成。**退回有一个前提**：带不可逆标记的目录代派过单以后，更早的构建在启动时拒这一代，
退回之前要先按住带标记的规则（见「退回」）。接口升了两项，都不是单向的：`actingd-config` [5, 29] → [5, 32]（30 是 MAPP-2b 的新键，31 是 S2 的实例成员，32 是 ORD-1c 的新键），
`package` [3, 11] → [3, 12]（G-R1）。运行上要知道的是这几处：

- **状态根只属于一个安装：本版在已有的安装上第一次启动时做的事**（#453 S11、S12，登记 §1.358、§1.368；`contracts/state-root-installation.md`）。
  - **哪个根算安装根**：没有新参数。写状态根的程序（启动、`unlock-owner`、每一种 `ledger-maintenance`、S12 的 `claim-state-root`）取 `--config` 所在的目录，这个目录里有安装工具
    自己的 `install\state.json` 时它就是安装根（`crates/runtime-host/src/state_root_installation.rs` 177–200；actingd 在 `apps/actingd/src/main.rs` 729–737 交给宿主）。安装工具和看门狗都以
    `<根>\runtime\actingcommand-actingd.exe --config <根>\actingd.config.json` 起 Runtime，所以在用的安装的每次启动都从安装根起；从别处起的（CI、开发时直接跑）只多一次读文件属性，
    别的什么都不读、不铸、不写。
  - **第一次启动（核过代码；v0.12.58 和 v0.12.59 跑过的根一样，两版都不认识这两个文件）**：
    1. 在建状态根目录和格式检查之前、写任何东西之前读两个文件（`crates/runtime-host/src/host.rs` 1686–1708）：根上还没有 `<状态根>\installation.json`，算 `unclaimed`，启动继续。
    2. 照旧取属主锁、写这次启动的属主记录（1735–1742），然后在锁下认领（1743–1763，`state_root_installation.rs` 244–304）：再读一次记录，仍然没有；安装还没有身份文件，就从操作
       系统的随机源取 16 字节，写成 32 位小写十六进制，**只在不存在时**放成 `<根>\install\identity.json`（先写 `identity.json.part` 并刷盘，再硬链接到目标、删掉 part 的名字；链接因
       「已存在」以外的原因失败时退到「只在不存在时创建」），再从盘上读回来用；接着用这个编号和本次的属主纪元同样放下 `<状态根>\installation.json`，读回来照检查的规则判一遍。两个文件
       里是编号、时间、进程号、属主纪元和构建的提交（构建不知道时为 `null`）；编号不进配置、事实、回答和日志。
    3. 账本打开以后记一条带标签的运行时事实 `host.state_root_installation`：一行 `{claimed_at_unix_ms, claimed_by_pid, claimed_by_owner_epoch, claimed_by_build_commit?}`，没有编号，来源
       `runtime`，没有有效期（`host.rs` 2471–2496，`state_root_installation.rs` 310–339）；actingd 往启动日志写一行 `state_root_claimed`（info，`path` 是状态根），只有放下记录的那次启动
       写（`host.rs` 2449–2452，`main.rs` 520–523）。
    4. 启动别的地方不变。每次启动本来就写的配置事实里多几行：`config.parameters.scheduling` 多两个新键那两行，`config.parameters.ledger_observer` 的标签表多两项，`config.selection` 里默认值
       文件的摘要变了，`config.subsystems` 多 `queue_wait`。
  - **能让这次启动失败的**：`install\state.json` 的属性读出「不存在」以外的错，`state_root_installation_record_invalid`；认领的某一步做不成（随机源、part 的建写刷删、链接和它的退路
    都失败、读回时文件不见了），`state_root_claim_failed`。两者都是致命的：前者在取属主锁之前，什么都没写；后者在锁下，启动关掉它开的属主纪元；Runtime 停着，看门狗第 5′ 行只看类别，
    把致命的启动按住、不反复重启
    （`contracts/runtime-watchdog.md`）；部署里启动后检查的 `ready` 不过，跑部署的安装工具照常自动退回。身份文件已经放好、记录没写成时，下一次本版的启动从记录那一步接着做。已有的
    根上没有别家的记录，「别家的状态根」那两种拒绝在这里碰不到。
  - **第二次及以后的启动**：读出两个文件、判是自己的，什么都不写：不铸、不放、不写那一行，也不再记事实（最近一条事实的认领时间和属主纪元与记录相同，`fact_for_start` 返回空，
    `state_root_installation.rs` 350–384）。
  - **看门狗起的启动**和安装工具起的是同一个命令行，第一次启动由谁起，谁就认领；看门狗自己不读这两个文件，日志里多的一行 info 不影响它。
  - **启动后检查**：没有一行读这两个文件、这条事实或那一行日志；它读的 `StartReport`、`Status` 和配置事实形状没变（契约库的 `runtime.rs` 自 v0.12.59 只改了说明和一个常量）。
  - **退回以后更早的构建**：两个文件都不读不写，原样留着；配置指向别家的状态根时它照旧会起在上面。那条事实留在账本里，更早的构建照旧回放程序事实（它的默认标签表里没有这个键，只少一条
    索引行），列全部程序事实的读法多显示一条，没有谁读它。哪一版的安装工具都不移动、不改名 `install\identity.json`，`install leftovers` 只列带保留标记的名字，也不列它。再装回本版：
    记录是自己的，启动继续；事实和记录的认领相同，不再记。
  - **在别家的状态根上**：记录是别的安装的，或者状态根有记录而这个安装还没有身份文件，启动在写任何东西之前以致命的 `state_root_installation_mismatch` 拒（`path` 是状态根，`at_unix_ms`
    是认领时间，文字说是哪一种）；文件读不了、过大、不是那份文档，`state_root_installation_record_invalid`。`unlock-owner` 和每一种 `ledger-maintenance`（不带参数的 `verify` 也在内）在取
    属主锁之前做同样的只读检查，从不铸、不认领；`actingd check-config` 多报顶层成员 `state_root_installation`（`claimed_by_this`、`unclaimed`、`not_installed`），另两种在阶段
    `config_state_root` 以同样的码拒，所以安装工具每条先跑 check-config 的路（`config replace`、`instance add`、更新计划、`start`）也拒。只读的取证工具和看门狗不查。最常见的是配置抄错了：
    把 `state_root` 改回本安装自己的状态根就行，两个安装都没被写过。
  - **出路：`actingd claim-state-root --config <配置> --actor <名字> [--new-identity]`**（S12；救急行 `state-root claim`）：一个人明确地把配置里的状态根交给命令所在的安装。没有安装根就拒；
    像启动一样取属主（Runtime 在跑 `owner_conflict`，资源记成在用而进程还在 `owner_resource_unconfirmed`），属主日志里记下这一笔。身份文件读得出就用，没有就照启动的办法铸，读不出要给
    `--new-identity` 才把它改名成 `identity.json.old-<毫秒>` 再铸（读得出却给了这个开关 `invalid_arguments`）；记录没有就写，是自己的什么都不变，是别家的或读不出的就改名成
    `installation.json.old-<毫秒>` 再写本安装的。**什么都不删**；放新文件失败时把旧文件的名字还回去；中途崩了再跑一遍接上。它不打开账本、不读盐；下一次本版的启动发现记录的认领和最近一条
    事实不同，补记一条带 `previous_claimed_at_unix_ms` 的事实，再重启不多记。发起人只在命令的报告和这一行的回答里，不进属主日志和事实。替换了别家的或读不出的记录、或者写在留着的旧记录
    旁边时，回答带 `install_caveat` `fingerprints_discontinuous`：账本里带指纹的记录是用写它们的那份配置的盐算的。救急行只在 Runtime 停着时跑（更新那条路的停机规则）；救急模块的 `op status`
    多一个开关 `state_root_installation`，开了才读状态根的认领状态，不开不起任何进程、回答照旧。
- **录制做包**（#463 L1、L2、U1，登记 §1.370、§1.371、§1.372；`contracts/lab-recording.md`「One call per step」「Following a located text」，`contracts/linear-steps.md`「Retries」
  「A screen the package never recorded」，`contracts/policy-suspension.md`「A failure that pauses at its first occurrence」）。智能体开着录制、经 Lab 玩游戏做包，从本版起用得上这两样；
  装着的 `linear_steps` 包从本版起碰得到第三样。
  - **一步一次调用（L1）**：`do --capture --record` 多四个选项：`--mark-request <文件>` 或 `--mark-request-json <JSON>` 带开着的那一步的 `actingcommand.lab-record-mark.v1` 请求，在写任何东西、
    按任何东西之前用 `record mark` 自己的代码（`mark_session`，调用、不是抄一份）应用并自测；然后照今天的规矩按下去；给了 `--next-frame-after-ms <0..60000>`（可加 `--next-frame-out <png>`）时，
    按下去且没有失败就等这么久、像 `capture --record` 那样再取一帧，打开下一步。整次调用持着录制锁；写下的 `recording.json` 和三次调用写的相同（时间和 Runtime 的编号除外），`record stop`
    生成的包也相同。请求只能作用于开着的那一步：带 `step_action`、`transition`、别的 `step`，或者它的 `frame` 会开新步或换掉一步，都在写和按之前以 `validation_failed` 拒（`details.member`）。
    被拒的什么都不写、不按；按下去失败、没按成或结果不明，标记留着、不取下一帧；同一调用再发一次会被 `record mark` 自己的检查拒（`record_click_exists` 或 `record_mark_id_conflict`），
    消息说先看屏幕（不带 `--record` 的 `observe --capture`，或 `record status`），只在这一步自己的页还在时去掉标记请求再发。今天的 MCP 服务器：`ac_lab_do` 多 `mark`、`next_frame_after_ms`、
    `next_frame_out`，另有五个 author 档的新工具 `ac_record_candidates`、`ac_record_amend`、`ac_record_step`、`ac_record_archive`、`ac_record_reopen`（共 46 个，author 档 12 个）；Lab 的
    MCP 服务器的 `ac_do` 多这一行的四个参数 `mark_request`、`mark_request_json`、`next_frame_after_ms`、`next_frame_out`；两边导出超长的回答都把 `next_frame` 留在正文里。智能体每步一次
    `ac_lab_do`（`capture`、`record`、`mark`、`next_frame_after_ms`），下一次调用之前读回答里的 `next_frame`（它的 `out` 文件）。没有新码、新请求种类、新记录、新配置键或接口修订。
  - **按名字找着按（L2）**：`record mark` 的请求（schema 版本不变）可以给 OCR 标记写 `locate`（`normalize`、`blocks`、`line_merge`、`cells?`）和 `unique`（`margin_milli`），和任务的定位目标逐个
    成员相同，录的时候就照 Runtime 的声明规则判（不合就 `validation_failed`）；`click.follow` 可以指向这一步的这种标记：只写跟随时按找到的那一块或它所在的格（`target`），再给一个框时按找到的
    框的角加上这个框在主帧上的偏移（`offset`，声明时定下、不缩放）。自测用 Lab 的 OCR 在每一帧上跑识别包自己的定位评估（内核的守卫和页识别用的同一个），找不着、在格外、不唯一各以带类型的原因拒
    （`text_locate_not_found`、`text_locate_outside_cells`、`target_match_not_unique`）。`record stop` 写出定位目标、`target` 或 `offset` 点击、守卫 `verify_text` 和
    `"input_recheck": {"kind": "fresh_frame"}`：守卫在这一步的帧上过了以后，内核再取一帧新的、只认这一步的页、再判一遍守卫，从这一帧找到的框按。被跟随的标记只当点击的守卫和目标、不算这一步
    的页的标记，所以这一步要另有一个认得出这一页的标记（不然 `page_needs_followed_text`）。名字（`expected`）录完还能改：一次 `record mark` 把这个标记去掉再加回来、只换名字，`record stop`
    以后、按过以后都行，按过的记成 `names_changed_after_press`，`record status` 看得到。没有 OCR 模型时这种跟随被拒（`lab_ocr_unavailable`）。不带这种标记的录制生成的包和以前一样。内核没改，
    生成的包只用 `package` 接口已有的成员。已知的限度：只看一屏、不滚动；运行时屏上没有这个名字，守卫拒、不按，这次运行以 `contained_task_guard_refused`（带类型的原因）失败，「找不着」还没有
    正常的结束；名字只能是录下的帧上出现过的；复查只取一帧新的，默认的任务时限不算它。
  - **装着的 `linear_steps` 包从本版起会碰到的（U1，核过代码）**：包一个字不改，行为变了。原来一步在等待以后以 `page_confirmation_failed` 结束的地方（最后一次尝试的到达，有没有重试都算；
    看到中间页或跳过目标以后的下一页；门来晚了以后接着等的那一段；两页都没看到的重试判断），内核现在先做一次「收尾判断」（`crates/execution-kernel/src/contained_task/linear.rs` 1603–1785）：
    立刻取第一帧，按取帧间隔再观察一个步骤时限，只看、从不按、从不把帧交给共享的打断页去处理。每一帧单独判：等的那几页或这一步自己的页有一页过了、遇到过渡帧、或者屏上是共享的打断页，
    就照旧以原来的码、原来的细节和计时结束；只有到时限时首末两帧和最后两次取帧都没变（Runtime 唯一的画面变化量 `StillScreen::WHOLE_FRAME`），而且最后一帧上包里别的页和加载页也都不过，
    才以新码 `contained_task_linear_unrecorded_screen` 结束（`pack_operation_id`、`page_ids`、`attempts`，`artifact_id` 是钉住的最后一帧）。收尾判断里自己的取帧、识别或评估出错，照旧是原来的码；
    只有 Runtime 自己记账失败才照常结束运行。**不做收尾判断的**：没看到中间页的、结束在加载余量上的、从应用入口开始的一步、声明了样本共识的包、不是任务自己的运行（前置、回主页、导航、打断这些层，
    恢复入口，启动包，阶梯的一级）、重启段里、离运行的期限或已准入的运行时预算不到两个步骤时限的（`linear_closing_pages`，1603–1645；`own_task_run` 在 `contained_task.rs` 5124–5130）。
    **多花的时间**：失败的那一步最多多一个步骤时限，一次运行最多一次（运行就在这里结束）；成功的路一点不变。**结果**：调度派的运行以新码结束时，宿主把终端记成 `failure_severe`
    （`crates/runtime-host/src/host/contained_task.rs` 9458–9477），这一对（目录任务、实例）第一次出现就停住，不管 `on_failure` 写的是什么，也没有立即重跑；恢复阶梯照旧启动（新码的阶梯类别是
    `pack_logic_mismatch`，同 `page_confirmation_failed` 一样经默认的 `pack_logic` 一项）；然后这台实例接着跑别的任务，实例不停、不围栏，别的组合的准入、次序和计时都不动。直跑和 Lab 跑同样以新码
    结束、不停任何东西。没人声明过的、慢且静止的画面现在也会让它的任务停住：把它声明成一页或一个加载页。**怎么放开**：只有包的内容变了（主包，或失败标识里点名的层）才放开，在这一对下一次到期的
    评估、过了 `scheduling.suspension.lift_check_min_interval_ms`（默认 60000）以后，不用重启；没有手动放开，重启也不放开。**在哪看**：`actingd suspended` 的 `suspended[]`（`failure` 里是新码和它的值、
    `artifact_id`，`effective_class` `severe`，`lifts_when` `content_hash_changes`）、`ac_diagnose`（`suspended` 行、`failures_grouped`）、`ac_schedule` 里 `why` 为 `policy_task_paused` 的候选行；照
    `artifact_id` 那一帧把这个结局录进包，包一变就放开。**页图包不变**：内核页图的执行路径没有改，这个判断只在线性运行里。
  - **不在本版的**：线性任务里的「选择」一步（在列表里挑一个、找不着时正常结束）、录下一次选择、变量（从包外给名字）、Lab 的采纳、生成和就绪那几行，都在以后的版本；`session app --record`
    在两个 MCP 服务器上都还没有工具。
- **用 Runtime 自己的工具建全新的安装根**（#453 S1、S2、S3、S4、S7 和 S8、S9，登记 §1.357、§1.362、§1.365、§1.366、§1.355、§1.367；`contracts/install-cli.md`「The Lab's rescue module」
  「Fresh root」）。这些行都不下载任何东西；除了 S2 是 Runtime 启动时的行为，都不启动任何东西。
  - **`new <新根> --build <目录>`**（S4，`crates/lab-rescue/src/install/new.rs`；可选 `--new-state-root`、`--mumu-root`、`--lab-authoring`、`--watchdog`）：从校验过的发布文件夹（两个 zip、
    `SHA256SUMS`，有就核 `release-facts.json`）或留存的构建文件夹建一个全新的单位置根。新根和状态根要么不存在要么是空的（`root_not_empty`、`state_root_not_empty`），构建要校验得过（缺模板
    也算 `build_invalid`），跑它的安装工具和这个构建的 `actingd-config` 要有交集（`interface_edge`），都在写任何东西之前判。一次记日志的操作（种类 `new`，不在看门狗的放行名单上）：拷构建、
    校验、放程序文件、写第一份配置、建空的状态根、模板没点名视觉根时建空的 `<新根>\vision`、记状态；能续做，撤销把写下的挪到一边、不删。第一份配置来自 Runtime 包里新的模板
    `actingd.config.first.json`（组合模式、一台宿主 `local`、没有实例、标准包根 `packages`），只填状态根、取自操作系统随机源的 32 字节盐（不进日志、回答和任何记录）和给了的两项，先让新构建
    自己的 check-config 带 `--install-root` 过一遍再改名落位。它不铸身份、不写认领记录，那是第一次启动的事（见上）。`--watchdog` 最后给新根装看门狗的计划任务，不启动。这是唯一一行不先找
    安装根的：它作用在自己点名的根上，所以发布包里的 `actinglab.exe` 和 Lab 的 MCP 服务器在还没有任何根时就能跑它。
  - **`instance add` 能加第一台实例**（S1，`crates/lab-rescue/src/install/instance_add.rs`）：原来新行的主机和能力从已有的行抄，全新的配置一行都没有就加不进。现在 `policy.facts.instances`
    为空时取 `policy.resources.hosts` 里唯一的那一台（没有、为空、多于一台、没有编号都拒，键 `policy.resources.hosts`），能力只给标准包那一项，`capabilities_from` 写 `null`；指定了目录（不是
    组合模式）的空配置照旧拒（键 `policy.facts.instances`），没有 `policy` 的也拒、键改成 `policy`。已有行的配置，候选、提交和拒绝逐字节不变。
  - **实例条目可以声明第一次启动先停着：`start_paused`**（S2，`actingd-config` 31；`crates/runtime-host/src/host/start_pause.rs` 1–27；`contracts/scheduling-pause.md`「Pauses held at a first
    start」）：可选布尔，不写就是 `false`，装配时读一次（`apps/actingd/src/config.rs` 1720–1887、2119），不能在线设。为 `true` 时，第一次看到这个条目的启动在恢复完已有暂停之后、准备任何实例
    之前，用现有的原因 `instance.added` 按住它，再在带标签的事实 `host.start_pause_applied` 里记下（先记暂停、后记已应用；记不下就启动失败）；被别的原因按着的原样不动，那个暂停被放开的同一步
    里换成 `instance.added`；人恢复以后再起不再按，成员可以留在文件里。**谁读它**：actingd 的装配和宿主的启动；本版安装工具启动后检查的 `pauses` 一行（声明了它的实例上多出的
    `instance.added` 算正常，点名在 `start_paused` 里）；救急模块交回暂停时把又被按住的实例记进 `held_again`；check-config 对「有这个成员、自己的标签表却漏了 `host.start_pause_applied`」
    给一条 `ledger_tag_key_unclassified` 提示。今天没有哪份配置写它。
  - **Runtime 停着也能 `instance add`**（S3）：原来没有 Runtime 应答就 `runtime_not_running`。现在先照更新那条路的停机规则判（属主锁被持有 `owner_lock_held`、根下有 actingd 进程
    `runtime_running`、判不了 `processes_unreadable`；写候选之前判一次，拿到写锁以后再判一次），新条目带 `start_paused: true`，只做「检查配置、换文件」两步，被换下的文件留着；Runtime 下次
    启动照 S2 按住它，直到有人恢复。这条路要现装版本带 S2（0.12.60），更旧的 check-config 拒这个成员（`config_refused_by_target`），什么都不换。`config replace`、`config restore` 在没有
    Runtime 应答时也照同一条规则判，不会再换掉一个正在启动的 Runtime 脚下的文件。Runtime 在跑时这几行照旧。
  - **`standard-pack check` 报出要哪些模型；视觉根的清单和离线核对**（S8、S7；`contracts/vision-model-folders.md`「The manifest」）：`standard-pack check` 的回答最后多一个成员 `models`（这个
    游戏根的包要用哪些模型：`model_ref`、`model_sha256`、哪些包）。`actingcommand-actingd vision manifest --provider cpu|directml` 答这种跑法的视觉根该有哪些文件（视觉根里的路径、大小、
    SHA-256、组件、许可、是否钉住）和各模型的身份，回答里没有网址；`vision check --config <文件> [--install-root <根>]` 照启动的办法找到视觉根，逐个文件核对、列出模型文件夹并算身份（单个文件
    256 MiB、合计 512 MiB 的上限）。两者只读、离线：不碰状态根、属主锁、写锁、账本和网络，什么都不写；成功码 `vision_manifest_reported`、`vision_check_completed`（新）。救急行 `vision manifest`、
    `vision check` 起现装版本的 actingd 去问，限时，到时结束子进程（`vision check` 那一行总时限不超过 900 秒），答不出来是 `vision_report_unanswered`。核对比启动严：CPU 的清单也列着钉住的两个
    运行库文件，CPU 的启动并不按钉表核它们；`models\` 缺或空时核对回空列表，启动会拒。工具登记里 DirectML 的三个许可文件补了钉值，文字识别模型的三个文件写上了在视觉根里的位置。
  - **`vision place --from <目录>`**（S9，`crates/lab-rescue/src/install/vision.rs`）：视觉根在哪、用哪个提供方、要哪些文件，都问现装版本的 `actingd vision check`。写任何东西之前把要放的每个
    文件在给的文件夹里按大小、再按 SHA-256 核一遍，有一个不对就 `vision_source_invalid`；视觉根不存在就拒（不替人建）；清单没钉住的文件一个都不拷。只在 Runtime 停着、拿着写锁时放（停机
    规则），一次记日志的操作（种类 `vision_place`、步骤 `vision_file`）：只放缺的或哈希不同的，被替换的旧文件改名成 `.old-<毫秒>` 留着；放完再核一遍，不过就 `post_check_failed`、文件留在原处；
    已经齐全的根不开操作、不写。能续做、能 `undo`。
- **在用的安装碰得到的两处修正**（登记 §1.364、§1.363）。
  - **一台实例的会话关闭被拒，只隔离那一台**（#458、#456；`contracts/read-session-resource-close.md`）：原来一台实例的租约（收尾期的任务租约以外的：客户端的、Lab 会话的、Lab 操作的、收尾期
    以外的任务、操作员认领、Runtime 自己的认领）在设备会话还开着时过期，清扫去关会话，这次关闭一定被调度器拒（过期的令牌一律拒），清扫把它当成致命，**整个 Runtime 停下**。只要有两台以上
    实例用模拟器自带的截图或输入后端、其中一台的这类租约在会话开着时过期（一个卡住、连接还开着的客户端就够），一台设备一个应用的安装就碰得到。无钥观察截图失败以后关自己的会话被拒
    （接管冷却、被暂停挡住的排队认领、复用的租约已过期）同样致命（#456）。现在这两条路上只隔离那一台：现有的资源围栏，原因是被拒的那个码（`lease_expired`，截图失败后是 `lease_cooldown` 或
    `lease_busy`），Runtime 的健康保持干净，同一拍清扫照常处理别的租约和别的实例。欠的那次关闭在租约结束以后、在那台实例的准入锁下用「提前关闭」补上，排队的一方拒不了；设备不再带着开着的
    会话交给排队的下一个。补关又被拒的留给围栏的复检（默认每 300 秒）、恢复阶梯或人工自检。账本里多的是那次拒绝的 `runtime.failed`（阶段 `runtime_lifecycle_session_close`）和
    `instance_resources_fenced`。Runtime 自己的故障（记录写不进、锁中毒、属主日志、调度器不变量、停机期间的一切）照旧致命；清扫线程不再因一个非致命的错悄悄退出。**没有关闭被拒时**，今天跑的
    例行任务的记录和码都不变（这一句靠读差异，登记册照实写了）。仍会停整机的两条路（连接断开时碰上已经过期的租约、输入失败时的收尾）另立 #461，本版没修。
  - **直跑的终端一步失败也把设备还回去**（#457；`contracts/resource-readings.md` 355，`contracts/runtime-interface.md` 471）：原来人或智能体自己起的直跑（不是调度派的）在终端那一步出错返回
    （比如事实库拒收它要发布的列表），回答是如实的失败，租约却没还，连接不断就一直占到租约到期，后面的运行白等到排队超时。现在这条路径上五处提前返回都在回答之前先用当时的令牌结束租约，
    回答不变；释放自己失败时，非致命的挂成这次失败的清理关联，致命的照旧毒化。调度派的运行和宿主自己的运行不走这段代码，今天的例行任务不变。审查另查出的同类口子另立了单（Lab 操作的三处
    提前返回 #459；单个应用的调度运行收尾续租以后兜底释放拿的是旧令牌 #460）。
- **目标**（#455 T1-R、G-R1、G-R2a、G-R2s，登记 §1.351、§1.356、§1.361、§1.369；`contracts/resource-targets.md`、`contracts/standard-pack-goals.md`、`contracts/standard-pack-schedule.md`
  「The irreversible mark」）。在用的安装上没有哪个游戏根带 `goals.json`，也没有哪个应用存着第 3 版的目标记录。**T1-R、G-R1、G-R2a 在用的安装上什么都不变**：核过的：没有这个文件的根，
  `standard-pack check` 的回答、打包和放包的计划和以前一样，Runtime 根本不读这个文件；第 1、2 版目标文档的解析、存的行、回执和读取逐字节不变。**G-R2s 碰得到在用的安装**（最后一条）。
  - **T1-R：监控台的入口收整份目标文件**：`ApplyResourceTargets` 进了 `ui` 通道的清单（`crates/actingcommand-contract/src/runtime/interface.rs` 566–608），推翻了 LAB-6S3 当时把它放在拒收
    一边的决定（#455）。它的 `expected_version` 从「未实现」变成真的比较：在写这条记录的那次持闸里比版本，对不上 `fact_observation_version_conflict`（`fact_key` `session.resource_targets`、
    `current_version`），什么都不追加；不带这个成员的写入走的代码和原来一样（actinglab `targets set`、MCP 的 `ac_targets_set`、`actingctl agent-apply-resource-targets` 都不带）。今天的监控台
    （UI v0.12.18）不发它；带版本的写入在 `runtime-client` 的客户端方法里。版本的读法在 G-R2a 修过：撤回以后、只有条目时都比 0。
  - **G-R1：标准包的目标声明 `goals.json`**（`crates/actingcommand-contract/src/standard_pack_goals.rs`）：包根可以带一份，一条条声明这个游戏提供哪些目标条目（稳定的名字、八种类型之一、积木、
    次序、各语言标题、管什么）；严格读取，`standard-pack check` 在排程组合之后检查，十五个新拒绝词 `goal_*`，被拒的文件只出警告；`package bundle --goals` 把它原样放进包；安装工具的 `place`
    放它、后一个包不带时收起来、能撤回，带这个文件却没声明 `package` 12 的包在写任何东西之前拒（`crates/lab-rescue/src/install/place.rs` 540–546、757–759）。任何条目都不许去管带不可逆
    操作的包（按能力算，逐条规则追到它跑的包，跟着链头算值的规则也查；契约写明了这条查法的限度）。本版的 Runtime 不读它。
  - **G-R2a：值存在哪：资源目标第 3 版**（`crates/policy/src/resource_targets/v3.rs`）：值照旧存在那条实例事实里，不加事实键、不加文件；每个目标各带自己的期限，整份没有期限，另加最多 96 条
    「条目」行（名字、值、谁在什么时候经哪个入口设的，设置人和时间由 Runtime 从连接和时钟取）。写的规矩：v3 写在 v3 上目标整体替换、条目只在文档带了时替换；v2 写在 v3 上目标替换、条目保留；
    v1 写在 v3 上拒；目标和条目都没了就是撤回，记录和 v2 的撤回逐字节相同。读出来的那一步 v3 变成 v2 的形状、过期的目标去掉，评估器只有一条路径。没有接口修订、没有新词新码；钉着已发布
    客户端的监控台解得开 v3 记录的视图（看到不认识的版本标识，不解读里面的目标）。
  - **G-R2s：目标从不给能做不可逆之事的包的任务加权**，第 1、2、3 版记录都一样（`crates/actingcommand-contract/src/standard_pack_goals.rs` 的 `IrreversiblePackages`，
    `crates/policy/src/compose.rs`、`evaluator.rs`）。规则只有一份：根里有包把某个能力标成不可逆，这个能力就不可逆，提供它的包算不可逆；一个包的补救列着这种能力，这个包也算；有包读不了，
    整个根的包都按不可逆算。组合器给这种包的任务、和经任何一条评估器会读的链条跟着它的规则写上组合器专用的任务成员 `irreversible_reach: true`（为假不写，所以没有不可逆包的根组合出的字节、
    目录哈希和规则摘要和以前一样；`schedule.json` 自己写它被拒）。评估器对存着目标记录的应用上带标记的候选按没有记录来算（没有目标项、按基础权重、`run_cap` 里没有目标项、点名的 `tasks` 和
    `override` 无效），多一条原因 `resource_target_package_irreversible`（info）；别的候选的数一个不变。**一个游戏的根组合不成时，这个游戏的目标在它组合成之前都不生效**：工作台的检查在内存里记下
    组合不成的游戏（和当前这一代里对不上的游戏），策略循环按实例告诉评估器，存着记录的应用上的候选照带标记那条路算、原因 `resource_target_root_uncomposed`；第一次组合成功就结束。
    **在用的安装上会看到的**：工作台每次检查多取每个包的 `control.json` 和 `resources/operations/resources.json` 的长度和修改时间（N 个包的根一次 2 + 2N 次，默认每 60 秒一次）；部署后第一次
    检查把每个根的包再读一遍；根里有不可逆的包时组合出一份新的目录代（这些任务带标记），别的不变；`bundle.json` 注册了却读不了的包（没做完的 held 包、注册了还没放的、标了不可逆却没有读取器要的
    `parameters` 的）让它那个游戏的任务全带标记、目标全不生效，每个这样的包记一条 `standard_pack_package_unreadable`（警告，`game`、`package_id`，带读取器的拒绝做原因），check-config 和
    `standard-pack check` 也列出来；热放置时一个根可能有一次检查（最多约一分钟）组合不成，这段时间它的目标不生效。没有存目标记录的应用，候选的数不变。**部署之前**（登记 §1.369「At deploy」）：
    用本版的 actinglab 对每个游戏根跑 `standard-pack check --root <游戏根>`，找 `standard_pack_package_unreadable`；用本版的 `actingd check-config` 读安装的配置（组合模式），确认每个游戏根都组合
    得成、没有整份被拒：它的说明和 `standard_pack_schedule`（`problems`、`left_out`）里没有 `standard_pack_root_invalid`、`standard_pack_root_unreadable`。部署是一次重启，重启后第一次检查
    没有标记可比，这时整份被拒会让在用的每个游戏的目标都不生效，直到组合成功。安装工具计划里的 `next_check_config` 只看本版的 check-config 过没过（v0.12.58 的 `install/plan.rs` 911–923），
    不看这几项，所以要另外读报告。已知的限度：手写的目录不在此列；离线的包行（`actingctl packs`）只读 `control.json`、看不到补救；往前的预测还不读不生效的游戏。退回见下面。
- **还用不上、作为部件发出的**：
  - **同一台设备放几个应用**（#437 MAPP-1c、MAPP-1d、MAPP-2b、MAPP-2d，登记 §1.350、§1.359、§1.353、§1.360；`contracts/device-group.md`）：MAPP-1c 关了设备组两处「两个成员的会话同时开着」
    的缺口（用排队的请求拒不了的「提前关闭」租约），把排队次序的锁改成按设备分，并在登记册写下每条跨成员路径的取锁次序；MAPP-1d 是监控探针（设备被同组别的应用占着时这一轮不做）、收尾租约
    过期时先关会话再由队列放行、放行队列按各自应用的门判；MAPP-2b 是运行开始前的应用切换（拿到钥匙后看前台，是别的成员就停掉它，在借来的钥匙上跑本成员的启动包，拉起应用只走 ADB 启动），
    切换失败记 `application_switch_failed`、按失败结算、不交给恢复阶梯，失败的成员冷却一段（新键 `scheduling.device_group.switch_failure_cooldown_ms`，只在内存里，排程说明显示成
    `dispatch_instance_cooldown` 带 `failure_code`）；MAPP-2d 拿掉了 MAPP-2b 的长租约和上限：派发的租约对谁都是任务期限加余量，启动包用钥匙剩下的时间，还回钥匙以后任务续租（续租不成是切换
    失败，步骤 `lease_renewal`）。**装配照旧拒每个多成员的设备组**（`device_group_config_invalid`、`several_members_unsupported`，`apps/actingd/src/config/device_group.rs` 381）。在用的安装都是
    一台设备一个应用，这些路一条都走不到；审查逐处核过单个应用的设备上记录、锁、租约和计时和以前一样（各片的单应用对照测试，例如
    `mapp2b_the_group_check_is_the_only_entry_and_a_lone_instance_never_takes_it`），改成 3840×2160 的那台设备也是单应用的。
  - **一台设备里排先后**（#437 ORD-1a、ORD-1b、ORD-1c，登记 §1.352、§1.373；`contracts/scheduling/v2/README.md`，`contracts/scheduling/README.md`「Order within one device」，
    `contracts/device-group.md`「The member in front」）：ORD-1a 让排程规则给任务写可选的 `max_queue_wait_ms`（1 分钟到 24 小时），靠 `requires` 里的子系统 `queue_wait` 把关（`config.subsystems`
    多这一行，更早的 Runtime 在 `requires` 拒写了它的文件），不写它的目录哈希不变；ORD-1b 是策略库里一步一步的选择（`crates/policy/src/evaluator/device_order.rs`）；ORD-1c 让宿主把三样输入交给
    它：每台设备在前台的成员（内存里的一张表，只由切换写，切换失败、切换时 Runtime 级的失败和另外五处前台会变的地方把它忘掉，启动时为空）、排队等待的起点（`QueueWaits`，只记多应用设备上不在前台、
    还在排队的成员的组合，只在内存，过了上限什么都不写）、新键 `scheduling.device_group.max_queue_wait_ms`（见下面「配置」；`actingd-config` 因它升到 32）。只有设备表里点名了多应用的设备时，
    `policy_device_groups` 才带上这些（`DeviceGroups::with_order`）；不然照旧只给那张表、不读配置（`crates/runtime-host/src/host/policy_dispatch.rs` 143–159）。装配照旧拒每个多应用的设备组，
    所以在用的安装上这些一条都走不到：单应用的设备上前台表一直是空的、等待一条不记，评估器拿到的和以前一样；测试 `ord1c_t8` 证明同一构建开着和关着这一片的两次启动评估和写下的记录逐条相同，
    和以前的构建逐字节相同靠读代码（登记册照实写了）。
  - **字段表经宿主走通**（#450 FL-2，登记 §1.354）：只加测试和文字，产品代码没动：在假设备上证明字段表经宿主的发布、保留、拒绝、读回和重启后重载都和网格读数的列表一样，宿主不需要改。FL-1
    （v0.12.59）加上 FL-2，就是 `contracts/resource-readings.md` 说的同一版发出。本版收下声明 `field_lists` 的包（它的 `interfaces.json` 要写 `package` [11, 11]：v0.12.59 起的 `place` 收、
    v0.12.58 的在计划阶段以 `interface_edge` 拒）；还没有发布的包声明它（资源仓的 FL-3 在本版上了生产之后）。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`，也不用本版加的 `new`、`vision place`、
  `state-root claim` 和 Runtime 停着时的 `instance add`；这些行都不用在生产根上。
- **生产上**：自 2026-11-11 起有一台实例在试跑调度，其余实例暂停着。每个在用的安装都是一台设备一个应用，设备跑在正好 1280×720；只有一台设备为了显示适配的实机验证改成了 3840×2160，那次验证
  还没做，这台的实例暂停着。在用的安装上没有哪个游戏根带目标声明，也没有哪个应用存着第 3 版的目标记录。部署本身不改变派单；部署以后变的是两处：线性包停在没录过的静止画面上时的结局（U1），
  和存着目标记录的应用上带不可逆包的任务不再被目标加权（G-R2s）。

配置：按代码、`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md` 核过，**升了两项：`actingd-config` [5, 29] → [5, 32]，`package` [3, 11] → [3, 12]**。
`component-interfaces.json`（4–10）：`actingd-config` [5, 32]（4）、`package` [3, 12]（9），`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 8]、`runtime-client` [3, 8]、
`runtime-outcomes` [1, 1] 没变；架构测试 `tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120`（157–165）跟着钉。G-R2s、L1、L2、U1 都不动任何修订（新码只是目录里的
字符串，`runtime-outcomes` 仍是 1；`irreversible_reach` 是组合器写的目录成员；录制的请求和 `recording.json` 不是接口）。

- `actingd-config` 30（`contracts/component-interfaces.md` 130，MAPP-2b）：`scheduling` 一节的可选成员 `device_group.switch_failure_cooldown_ms`；31（131，S2）：实例条目的可选布尔 `start_paused`；
  32（132，ORD-1c）：`scheduling` 一节 `device_group` 的第二个成员 `max_queue_wait_ms`。**谁声明**：Runtime（两半的清单）。**本版之前的程序**：更早的 Runtime 拒写了这三个成员的文件（这几处都拒
  未知成员，`config_decode_failed`），没写的文件照收；更早的安装工具没有比 `actingd-config` 范围的边，32 它根本不看，它的 `installer_knows_keys` 拒一份写了它不认识的键的配置（v0.12.58、
  v0.12.59 的 `install/plan.rs` 368–385，以 `interface_edge` 拒），所以部署之前文件里不要写它们；控制台对 32 什么都不拒，见下面「配对」。`runtime-client` 修订 8 不动：T1-R 只给已有的
  `expected_version` 成员行为、给 `ui` 的清单加一项。
- `package` 12（163，G-R1）：修订 11 加游戏根的 `goals.json`。**谁声明**：Runtime 声明 [3, 12]；带 `goals.json` 的包在自己的 `interfaces.json` 里写 [12, 12]，由包的打包脚本写（`resource verify`
  的 `bundle` 步不传 `goals.json`、不声明 12，`apps/actinglab/src/resource_verify/bundle.rs` 183–185）。**本版之前的程序**：之前的 `place`（v0.12.59 的范围 [3, 11]、v0.12.58 的 [3, 10]）在计划
  阶段、挂暂停和写任何东西之前以 `interface_edge` 拒这种包，放不带它的包时把已有的 `goals.json` 留在原处；之前的 Runtime 当它不存在；更新和退回的 `package` 边比的是安装登记里每个根的范围，
  登记里一律没有，照旧是 `pack_declaration_unknown` 警告。
- 配置键：**新增两个**（`actingd.defaults.json` 386–389，`crates/runtime-config/src/scheduling.rs` 897–920，`since` 都是 0.12.60）：`scheduling.device_group.switch_failure_cooldown_ms`
  （MAPP-2b；毫秒，`1..=3600000`，默认 `300000`，可在线设，切换失败时读，没有按实例的形式）和 `scheduling.device_group.max_queue_wait_ms`（ORD-1c；毫秒，`60000..=86400000`，默认 `1800000`，
  可在线设，有多应用设备的宿主每个策略周期读一次，今天没有这样的宿主、所以从不读，没有按实例的形式，不点名应用）。每个根的清单和 `config.parameters.scheduling` 多这两行，来源 `default`。
  实例条目的成员 `start_paused`（布尔，不写为 `false`，装配时读，不可在线设，只在条目写了它时报成 `instance.<编号>.start_paused`）。默认值文件里 `ledger_observer.tag.workbench_patterns` 多
  `host.state_root_installation`、`host.start_pause_applied` 两项（443–444）；`config.subsystems` 多一行 `queue_wait`（总是开）。Runtime 包里多第一份配置的模板 `actingd.config.first.json`，
  示例配置 `actingd.config.example.json` 没改。没有去掉的键。
- 结果码：目录从 3929 个码、1753 个位置、644 个键、228 个词表变成 3939 个码、1760 个位置、644 个键、228 个词表，仍是 31 个分片；在用的 3901 → 3911，状态为 `retired` 的仍是 28 个，审阅定稿的
  仍是 172 个；词从 1679 个变成 1711 个。**新码 10 个**（都 pending、`since` 0.12.60）：`state_root_installation_mismatch`、`state_root_installation_record_invalid`、`state_root_claim_failed`
  （runtime-host，fatal）、`state_root_claimed`（runtime-host，info）、`vision_manifest_reported`、`vision_check_completed`（actingd，success）；`contained_task_linear_unrecorded_screen`
  （execution-kernel，error，阶梯类别 `pack_logic_mismatch`，必带 `pack_operation_id`、`page_ids`、`attempts`，可带 `artifact_id`；U1）；`resource_target_package_irreversible`、
  `resource_target_root_uncomposed`（policy，info，后者可带 `game`；G-R2s）；`standard_pack_package_unreadable`（contract，warning，必带 `game`、`package_id`；G-R2s）。**新位置 7 个**：`check_state_root_installation`、
  `record_state_root_installation`、`config_state_root`、`install_state_root_claim`、`install_vision_manifest`、`install_vision_check`、`install_vision_place`。**新词 32 个**：`install_refusal` 的
  `root_not_empty`、`state_root_not_empty`、`state_root_claim_unanswered`、`vision_report_unanswered`、`vision_root_unresolved`、`vision_source_invalid`；`install_caveat` 的
  `fingerprints_discontinuous`、`vision_incomplete`；`install_operation_kind` 的 `new`、`vision_place`；`install_step_kind` 的 `write_config`、`create_state_root`、`create_vision_root`、
  `vision_file`；`application_switch_step` 的 `lease_renewal`；`standard_pack_problem` 的十五个 `goal_*`；`follow_problem` 的 `press_outside_found_box`、`page_needs_followed_text`（L2）。
  没有新键、新词表。**改了的**：`device_group_waiting` 多可选值 `limit_ms`、`remaining_ms`、
  `required_ms`，`instance_already_selected` 多 `remaining_ms`、`required_ms`，`dispatch_instance_cooldown` 多 `failure_code`（都是现成的键）；`application_switch_failed`、
  `dispatch_instance_cooldown` 的两段文字和描述，`contained_task_deadline_unavailable` 的常见原因，`fact_observation_version_conflict`、`install_caveat_noted`、`install_command_refused`、
  `vision_config_invalid`、`vision_root_in_program_slot`、`vision_root_unavailable` 的描述，位置 `install_new`、`switch_application` 的描述，词 `pack_invalid`、`startup_package` 的说明改写；
  L2 改写了 `lab_ocr_unavailable` 的常见原因和处理、`record_click_follow_invalid` 的描述、常见原因和处理、位置 `record_click_follow_check` 的描述、词表 `follow_problem` 的说明和它的
  `not_template`、`not_required`、`guard_conflict`、`click_outside_frame` 四个词的说明（都放宽到定位文字）。没有码 `retired`。在用码里两段文字都写了的从 714 个变成 724 个（十个新码都写了）。
  本版的发布文件 `contracts/outcome-codes/released/0.12.60.json` 按合入全部 24 张 PR 以后的树生成，没有码、没有位置（十个新码都是 pending，改写的也都不是定稿条目，没有 `retired`）。索引照旧
  由脚本生成，仍是 32 个文件，核过是最新的。
- Lab 的操作表（`contracts/lab-operations.json`）：174 行 → 178 行，多 `vision manifest`、`vision check`（S7）、`vision place`（S9）、`state-root claim`（S12），都在救急模块（49 → 53）；`new` 这一行
  由 S4 打开（参数改成 `new_root`、`build`、`new_state_root`、`mumu_root`、`lab_authoring`、`watchdog`）；`instance add` 的 `runtime` 从 `required` 改成 `optional`；`targets set` 的入口多 `ui`，
  `ui_kinds` 多 `ApplyResourceTargets`；`op status` 多开关 `state_root_installation`；`package bundle` 多参数 `goals`；`standard-pack check` 的输出多 `models`。救急模块服务的行从 34 变成 39
  （`crates/lab-rescue/src/rescue.rs` 119–163）。Lab 的 MCP 服务器照表的命名规则（`crates/lab-ops/src/table.rs` 207–210）跟着多 `ac_new`、`ac_state_root_claim`、`ac_vision_manifest`、
  `ac_vision_check`、`ac_vision_place`。L1、L2 不加行：`do` 多四个参数 `mark_request`、`mark_request_json`、`next_frame_after_ms`、`next_frame_out`，请求多 `CaptureSequence`（L1）；
  `record candidates`、`record amend`、`record step`、`record archive`、`record reopen` 五行的 `source` 多今天的 MCP 工具名（L1）；`record mark` 的说明写上只有请求能声明定位文字的标记（L2）。
- 压缩包：**Runtime 包多一个文件 `actingd.config.first.json`，从七个变成八个**（`.github/workflows/windows-remote-build.yml` 660–668 的 `$runtimeFiles`、785–791 的拷贝，`INSTALL.md` 跟着改）；
  `runtime_payload_layout` 仍是 `distribution-v1`。Tools 包的 `$toolFiles`（672 起）没改，布局仍是 `platform-tools-v3`（773）。本仓库的下载脚本 `Get-ExactBuildArtifact.ps1` 认这个文件（495–509：
  清单声明了它就要它）；v0.12.59 及更早的那份按固定的文件数核，拿到本版的 Runtime 产物会以 payload count mismatch 拒（v0.12.59 的 636–637）。
- 程序文件：`Cargo.toml` 没改；`Cargo.lock` 只多一条内部依赖（救急模块直接依赖策略库，S1），没有新的外部依赖；`.github/` 只改了上面那几行；`ci/` 只有 `ci/retired-gates.toml` 多三条（MAPP-2b 两条、
  `gate_411_a52` 一条改名），后来的五张一条没加，不稳定表照旧是 FLK-08 和 FLK-10；actinglab 仍是 47 个顶层分派臂、102 条命令（`package bundle` 多一个选项 `--goals`，`do --capture --record`
  多四个选项）；`apps/actingctl` 自 v0.12.59 只改了 MCP 一侧（L1）：今天的 MCP 服务器从 41 个工具变成 46 个，`apps/actingctl/src/mcp/tools.rs` 里 46 个工具名，README 两种语言的表跟着改。

升级与退回（部署由当时装着的那一版的安装工具做。v0.12.58 和 v0.12.59 的 `install/plan.rs` 相同，下面写 v0.12.58 的 `plan.rs` 行号对两版都对；`postcheck.rs` 在 v0.12.59 改过，两版的行号分别写）：

- **v0.12.58 的安装工具直接装本版：能装，不用任何标志。** 安装工具的声明是它自己构建里的 `component-interfaces.json`（v0.12.58 的 `install/host.rs` 23–25）：`ledger` [3, 8]、`runtime-client`
  [3, 8]、`package` [3, 10]、`actingd-config` [5, 29]。计划的边逐条（v0.12.58 的 `plan.rs`）：
  - `ledger` 两边 [3, 8]，`previous_max_in_next` 过、`one_way` 不触发（298–310），`ledger_verify` 是 false（924）；`runtime-outcomes`、`install-control` 过；`runtime-client` 安装工具 [3, 8] 和 Rp、
    R\* 都相交（324–340）。`package` `bundle_max_in_next`（341–364）比的是安装登记里每个包的范围，登记里一律没有，每个游戏一条 `pack_declaration_unknown` 警告，不挡；没有哪条边拿安装工具的
    [3, 10] 去比目标的 [3, 12]。`runtime_equals_tools`（765–772）过。
  - `actingd-config` 没有比范围的边，所以 32 不挡：`installer_knows_keys`（368–385）只看配置文件里有没有安装工具不认识的键，生产的文件不写新键，过；`next_check_config`（911–923）：本版的
    check-config 带 `--install-root` 收 v0.12.58 的文件，报告多 `state_root_installation`（这时是 `unclaimed`）和子系统行 `queue_wait`，有读不了的包时还多 G-R2s 的
    `standard_pack_package_unreadable` 说明；安装工具按 JSON 值读这份报告、不严格解码（v0.12.58 的 `install/programs.rs` 243–271），这条边只看它过没过，过。
  - `defaults_diff`（581–607，把两份默认值文件拉平成叶子逐个比，列表整个比）**有三行**，按键名排：`ledger_observer.tag.workbench_patterns`（目标的表多两项；配置文件自己写了这张表时 `source`
    是 `file`，新的两项不会自己加进去：事实照记，只少索引行和同步写）、`scheduling.device_group.max_queue_wait_ms`（`live` 为空、`target` 1800000、`source` `default`）、
    `scheduling.device_group.switch_failure_cooldown_ms`（`live` 为空、`target` 300000、`source` `default`）。默认值文件自 v0.12.58 只改了这两处（`git diff v0.12.58` 只有这几行）。文件计划
    （774–858）里 `runtime/actingd.config.first.json` 是 `new`，Tools 文件两边清单相同。
  - **启动后检查是 v0.12.58 那一版的**：`no_seeding`（`postcheck.rs` 292–310）有 S0 就要求 `load`，生产的根早过了第一段，过；`new_keys`（494–526）只核 `added_keys`（592–601，`defaults_diff` 里
    `live` 为空的键）也就是这两个新键，配置文件不写它们，要求的来源都是 `default`；本版在 `config.parameters.scheduling` 里报这两行、来源都是 `default`（ORD-1c 的 `ord1c_t6` 钉着新键这一行的
    值、来源和可在线设），过；别的行读 `StartReport`、`Status` 和配置事实，形状都没变（`packs` 读启动报告里每个游戏的包数，G-R2s 不改它）。S11 在这次部署的启动里写下身份和认领记录（见上），
    没有哪一行读它们。
  - **跳过 v0.12.59 付的：没有。** v0.12.59 定下的次序是「部署在 v0.12.58 之上」，这条路正是；v0.12.58 的 Runtime 本来就收本仓库程序带词的绑定，部署前后、部署失败的自动回退和退回以后都不产生
    `runtime_protocol_invalid` 那种记录。v0.12.59 的改动和本版一起到：本仓库的程序以 Lab 的身份连 Runtime（记录和回执照 v0.12.58）、救急 `place` 的几处修正、`instance add` 的
    `application_id`、根的第一段和程序事实 `config.start`、`actingd discover` 和 `emulators`、`record archive` 和 `record reopen`（部署以后把生产上一直挂着的那个录制归档）、显示适配的输入侧
    （1280×720 上什么都不变；那台 3840×2160 的设备的实例暂停着，实机验证之前不让它跑）、MAPP-2a、FL-1、#441 的登记。能退回的只有 v0.12.58，v0.12.59 从没在这个根上跑过。
- **先装 v0.12.59、再由 v0.12.59 的安装工具装本版：能装，不用任何标志。** v0.12.59 声明 `package` [3, 11]，别的同上；边、`defaults_diff`（默认值文件自 v0.12.56 到 v0.12.59 没改，所以同样三行）和
  文件计划同上。启动后检查是 v0.12.59 那一版的：`no_seeding`（305–330）认 S0 那次 Runtime 记在关停证据里的第一段，生产的根不是，照旧要求 `load`，过；`new_keys`（514 起，`added_keys` 613 起）
  同上。能退回的是 v0.12.59。
- **配置文件**：v0.12.58、v0.12.59 写的配置文件本版照收，不用改（修订 5 到 31 都还读）；新的拒绝只有 S11 的：状态根里有别家的认领记录时（已有的安装上没有）。
- **已装的包**：本版对不写新成员的包不改准入：FL-1 的门只认 `field_lists`；ORD-1a 的成员要 `queue_wait`；`goals.json` Runtime 不读；G-R2s 不拒任何包，只给不可逆包的任务写标记。本版的
  `standard-pack check` 对今天的根答法和 v0.12.59 相同，只在最后多 `models` 一个成员，有注册了却读不了的包时多一条 `standard_pack_package_unreadable`；没有 `goals.json` 的根不多别的警告。
  **运行变了的只有线性包**：U1 的收尾判断（见上），包不用改；L2 生成的包只用已有的成员，v0.12.58 的内核就收（`fresh_frame` 复查自 #411 B12 就有）。
- **退回（按本版安装工具的边和回退门核过；退回由在线的本版安装工具跑）**：
  - **能退回的只有这次部署替换掉的那一版**（回退门第 2 部分，`crates/lab-rescue/src/install/update.rs` 410–447，`live_operation` 181–204）：从 v0.12.58 直接装上本版的退回 v0.12.58 能过，退到
    v0.12.59 以 `rollback_gate_failed` 拒；经过 v0.12.59 装上的退回 v0.12.59 能过，直接退到 v0.12.58 被拒。部署失败时的自动回退由跑部署的那一版安装工具做，退向被替换的那一版。退回时边都过：
    `ledger` 8 在 [3, 8] 里、`one_way` 不触发；`runtime-client` [3, 8] 两边都相交；`package` 是警告；`installer_knows_keys` 用本版的键表。第 4 部分 `config`：目标的 check-config 读现行的文件，
    文件里写了 `start_paused` 或 `scheduling.device_group`（两个新键都在它下面）就不过（更早的构建拒这些成员），先拿掉；今天的文件不写，过。文件计划里 `runtime/actingd.config.first.json`
    是 `extra`，留在原处，更早的程序不读它。
  - **退回之前要先看的一条（G-R2s，登记 §1.369「Rollback」）**：根里有不可逆的包时，本版组合出带 `irreversible_reach` 的目录代；**这一代派过单以后，更早的构建在启动时拒它**（它要编译派单和
    决策记忆点名的每一代，它的读取拒这个成员，`unknown_field`），同 SCHED-C1a。所以退回之前先按住带标记的规则（它们的包），让这些组合在不带标记的一代下派一次单，再退；在部署自己按住的期间
    退回（带标记的一代还没派过单，部署失败时的自动回退就是这种）什么都不用做。手写的目录不在此列。
  - **更早的构建对本版在在用的安装上能写下的东西**：
    - `install\identity.json` 和 `<状态根>\installation.json`：不读不写，原样留着（见上）；`host.state_root_installation` 那条事实留在账本里，照旧回放、没人读；再装回本版对得上。
    - 新键和 `start_paused`：只在文件写了时有，更早的构建拒，先拿掉。运行中经 `ac_config_set` 记下的、点名两个新键之一的指令（`host.config_overrides` 的一行）不用管：更早的构建恢复指令时丢掉
      它不认识的键、记 `config_override_dropped`、照常启动（v0.12.58 的 `crates/runtime-config/src/effective.rs` 1324–1344）。`host.start_pause_applied` 只在有成员时写，更早的构建不读；按着的
      `instance.added` 暂停照旧恢复、照旧由人放开。
    - 账本里的新码和为它停住的组合（U1）：更早的构建把不认识的码照原样读成收到的拼写（v0.12.58 的 `crates/actingcommand-contract/src/outcome.rs` 684–687），停住按终端的 `failure_severe` 重建
      （`scheduled_failure_class`，v0.12.58 的 `crates/actingcommand-contract/src/event/payload.rs` 3027–3036：`failure_severe` 为真就是 `severe`），所以照旧停着；失败标识是从记录里读回来的，
      包的内容变了照旧放开。退回以后更早的构建不再产生这个码：同样的画面又以 `page_confirmation_failed` 结束、照 `on_failure` 走；它的 `ac_outcome_code` 不认识新码。G-R2s 的两条原因和那条警告
      同样照原样读，没人据它们做什么。
    - 录制（L1、L2）：一次调用写下的录制就是三次调用的文件，更早的 Lab 照读；更早的 actinglab 拒那四个选项，更早的 MCP 服务器没有那五个工具、`ac_lab_do` 不认那三个成员。存着定位文字标记的
      `recording.json` 更早的 Lab 解不开（`lab_recording_unreadable`），文件原样留着：一次录制里别换着用不同的构建。用定位文字生成的包在 v0.12.58 的 Runtime 上照跑。
    - 修正（#457、#456、#458）：什么都不落盘，更早的构建照旧读本版写的每条记录，在本版隔离的地方又会停整机。MAPP-1c、1d、2b、2d、ORD-1（含 ORD-1c 的前台表和等待，只在内存）、FL-2 在在用的
      安装上什么都没写。
  - **只有新加的行才写的东西**：`new` 建出的根是普通的单位置根，更早的安装工具读得了它的 `state.json`，种类 `new` 的流水只有本版的安装工具读（更早的拒这个种类）；`vision place` 的流水
    （种类 `vision_place`）更早的构建不读，它放下的文件和 `.old-<毫秒>` 留着，更早的看门狗遇到 `current.json` 里这个种类读不了、照旧站到一边；停机时 `instance add` 写出的条目带 `start_paused`，
    更早的 Runtime 拒，先拿掉或留在带 S2 的构建上，更早的安装工具把又被按住的实例报成已恢复；`state-root claim` 交过的状态根留着本安装的记录和改名留下的旧文件，更早的构建不读；第 3 版目标
    记录：更早的构建读成不认识的版本（`resource_target_policy_unreadable`，`unsupported_schema_version`），那个应用走基础调度、视图答 `resource_target_view_policy_unreadable`，重新写一份第 2 版
    文档就恢复，不会失败；`goals.json`：更早的 Runtime 当它不存在，更早的 `place` 拒带它的包、放不带它的包时把它留在原处；`ui` 上写下的目标记录是普通的记录，更早的构建照读，它的 `ui` 又拒
    `ApplyResourceTargets`、带 `expected_version` 的写入又是 `not_implemented`。
- 配对：
  - **控制台 UI**（最新发版仍是 UI v0.12.18，是 UI 的 `3a04aa39`，钉 Runtime v0.12.59 `803fdfa3`；前一版 v0.12.17 是 `ab31c42c`；UI main 在 `10fbf924`，比 v0.12.18 多一个只读的目标页签）：
    - **它对 `actingd-config` 32 做什么：什么都不拒。** v0.12.18 声明 `actingd-config` [5, 29]、`ledger` [3, 8]、`runtime-client` [3, 8]、`runtime-outcomes` [1, 1]、`package` [1, 2]
      （`crates/acui-setup/component-interfaces.json` 3–11，构建时写进它的 `BUILD-MANIFEST.json`，`.github/workflows/build.yml` 372–409）。核对这几个范围的只有 0.12 的安装向导 `acsetup.exe`：
      它核安装集时调用 `console_problems`（`crates/acui-wizard/src/set.rs` 676），`actingd-config` 那条边是「有交集」（`crates/acui-wizard/src/manifest.rs` 208–213 的 `CONSOLE_EDGES`，268–271
      的判断：`ours[0] <= theirs[1] && theirs[0] <= ours[1]`）。[5, 29] 和本版的 [5, 32] 有交集（5 ≤ 32、5 ≤ 29），所以过；`ledger` 8 在 [3, 8] 里、`runtime-client` 相交、`runtime-outcomes` 1 在
      [1, 1] 里，都过。监控台程序本身不读 Runtime 声明的 `actingd-config`（UI 仓里再没有别的地方读它；0.11 的 `crates/acui-setup` 不在工作区里）。包的边（`manifest.rs` 192–198）拿 Runtime 的
      [3, 12] 比，声明 12 的包也收。登记 §1.373 和 `component-interfaces.md` 第 32 行写「读 `actingd-config` 的控制台和安装工具要先收 32」，照上面核下来，今天发的控制台和安装工具对 32 都不拒，
      所以下一版控制台把声明写到 [5, 32] 只是让声明照实，不是放行的条件。
    - **它解码的**：v0.12.18 钉的 Runtime crate 在 v0.12.59 的发版提交；本版在 `ui` 上的回答和回执没有新形状（契约库的线上类型没改，只改了说明和一个常量），新的两条事实是标量组成的
      `record_list`，照读；第 3 版目标记录的视图解得开（G-R2a 有测试钉着）。v0.12.18 不发 `ApplyResourceTargets`；以后的控制台在本版上发得进去，在更早的 Runtime 上得到 `channel_request_refused`，
      什么都不变。
  - **部署工具包**（照现在的做法，用在线那一版的 `actingctl install` 跑每一步）：`release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop --hold --known-pause …`、
    `status --root [--operation]`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause … --start --detach`、`release-hold --root --reason`；另用目标
    构建的 `install status` 轮询上线，用目标构建的 actinglab 跑 `--json standard-pack check --root <游戏根>` 预查包根。从 v0.12.58 或 v0.12.59 装本版时，回答这几步的都是在线安装工具自己的代码，
    **没有一条被拒、没有一条换了答法**；`release stage` 按清单核文件，本版的清单列着那个模板，第八个文件不算多出来的；目标构建的 `install status` 只读文件、不连 Runtime，S12 也没改它；
    `standard-pack check` 只多 `models`（有读不了的包时多一条 `standard_pack_package_unreadable` 警告）。**会把标准错误有输出、或第一行变了当成失败的：没有。** 跑安装工具的那一处把标准错误并进输出、按退出码判；轮询上线和汇总守卫的两处从并起来的输出里取
    第一行以 `{` 开头的；解析计划、状态、分离日志的辅助脚本取第一行（分离日志取最后一行）以 `{` 开头的；预查包根的那一处分开读标准输出；这条路上没有程序在标准错误上多打一行。**工具包要
    跟着改的**：目标提交和标签；跑每一步的安装工具照旧指向在线那一版；预期改成 `ledger_verify` 为 false、`defaults_diff` **三行**（上面那三行）、启动后检查的 `new_keys` 核
    `scheduling.device_group.max_queue_wait_ms` 和 `scheduling.device_group.switch_failure_cooldown_ms` 都是 `default`、文件计划多一个 `new` 的 `runtime/actingd.config.first.json`、每个游戏一条
    `package` 警告；给热放置工具包写构建记录的那一步，核新包清单的 `actingd-config`、`package`、`ledger` 现在是 **[5, 32]、[3, 12]**、[3, 8]，记成 **`5,32`**、`3,12`、`3,8`（`5,29`、`5,31`、
    `3,11` 都会让那一步和后面的重钉不过）；默认值文件又多了一个键，它的哈希跟着变；要逐个核装上的文件时，Runtime 那一半多一个模板。**部署之前多两步只读的预查（G-R2s）**：预查包根的那一行
    要把 `standard_pack_package_unreadable` 当成要停下来看的结果；另用目标构建的 `actingd check-config` 读安装的配置，`standard_pack_schedule` 的 `problems`、`left_out` 和说明里不能有
    `standard_pack_root_invalid`、`standard_pack_root_unreadable`（计划的 `next_check_config` 只看它过没过）。**部署以后**：线性包的任务可能因新码停住，看 `actingd suspended`；退回之前先看上面
    「退回之前要先看的一条」。它下载用的是 `gh release download`，不经上面那份下载脚本，所以不受「v0.12.59 及更早的下载脚本拒第八个文件」影响。部署以后的
    启动日志里多一行 `state_root_claimed`（只这一次），工具包对启动日志的别的检查不受影响。
  - **热放置工具包的命令行，本版都照旧收、答法和 v0.12.58 相同**：actinglab 两行，`--json standard-pack check --root <游戏根>`（回答最后多 `models`；有读不了的包时多一条警告）和
    `--json standard-pack compose --root <包根> --instance <别名>=<游戏>/<区服> … --out <新目录>`（不写 `max_queue_wait_ms`、没有不可逆包的根组合出的目录字节和哈希不变；有不可逆包的根，
    这些任务多 `irreversible_reach`、哈希跟着变，和 Runtime 自己组合的那一代相同）；actingctl 的 `status`、`status --config`、`pause --instance <别名> --reason <理由>
    --drain-timeout-ms <毫秒>`、`resume --instance <别名>`、`schedule --instance <别名>`、`overview --root <根> --instance <别名> --sections catalog`（都带 `--state-root`）；actingledger 的
    `events --after … --through … --limit …`（`apps/ledger-forensics`、`crates/ledger-forensics` 都没改；`apps/actingctl` 只改了 MCP 一侧）。`status --config` 多两个新键的两行（来源都是
    `default`）、标签表多两项、`config.subsystems` 多 `queue_wait`；它按来源找在线指令，多出来的默认行不算指令。它读 actingctl 的地方都把标准输出和标准错误分开、按退出码和整段标准输出的 JSON
    判。**热放置工具包要学的**：重钉时比的构建记录里 `actingd_config` 是 `5,32`、`package` 是 `3,12`、默认值文件的哈希是新的；它的放包脚本钉着 `package` 的上限，等声明 11 或 12 的包出现时再学
    （今天没有）；它要是拿组合出的目录哈希去比，有不可逆包的根会比出不同（见上）。
  - **今天的 MCP 服务器（`actingctl mcp-serve`，现在注册着的那个）**：从 41 个工具变成 46 个（L1）：多 author 档的 `ac_record_candidates`、`ac_record_amend`、`ac_record_step`、
    `ac_record_archive`、`ac_record_reopen`，各自跑它那条命令；`ac_lab_do` 多 `mark`、`next_frame_after_ms`、`next_frame_out`，导出超长的回答把 `next_frame` 留在正文里（帧的部分从四样变成
    五样）；不带新成员的 `ac_lab_do` 走同一段代码，回答和记录不变；别的工具答法照旧。客户端重新拉工具表以后看到新工具（稳定入口照每次更新那样换子进程）。看得到的差别还有：`ac_outcome_code`
    认得十个新码和改写的文字，列全部程序事实的 `ac_facts` 在第一次启动以后多一条 `host.state_root_installation`，`ac_packs` 对没有行却读不了 `control.json` 的包多一条
    `package_binding_unreadable` 警告，`ac_diagnose`、`ac_schedule` 会显示 U1 停住的组合。退回 v0.12.58 以后它照 v0.12.59 一节写的那样连 v0.12.58：照收，记录写 `lab`、回执带 `Lab-MCP`；
    退回装上的是更早的 MCP 服务器，它没有那五个工具、`ac_lab_do` 的三个新成员它答 `unknown`（登记 §1.370「Rollback」）。
  - **Lab 的 MCP 服务器（`actinglab-mcp`，没有注册指向它）**：多五个工具，`ac_new`、`ac_state_root_claim`、`ac_vision_manifest`、`ac_vision_check`、`ac_vision_place`（都由救急模块在它自己的进程里
    服务，`ac_new` 在还没有根时也能跑）；`ac_instance_add` 在 Runtime 停着时也服务；`ac_op_status` 多开关 `state_root_installation`；`ac_package_bundle` 多参数 `goals`；`ac_standard_pack_check`
    的回答多 `models`；`ac_do` 多 `mark_request`、`mark_request_json`、`next_frame_after_ms`、`next_frame_out`，导出超长的回答同样把 `next_frame` 留在正文里（L1）。别的工具答法照旧。

运行上的变化：

- 从 v0.12.58 直接部署：不用标志，没有 `ledger_verify`，`defaults_diff` 三行（两个新键、一张标签表），启动后检查的 `new_keys` 核这两个新键；部署之前对每个游戏根跑 `standard-pack check`、
  用本版的 check-config 确认每个根都组合得成；能退回 v0.12.58，带不可逆标记的一代派过单以后要先按住那些规则。跳过 v0.12.59 不付任何代价。
- 本版第一次在安装上启动时铸 `install\identity.json`、写状态根的 `installation.json`、记一条事实、写一行 `state_root_claimed`；以后的启动什么都不多写。配置指向别家的状态根时启动被拒，出路是
  改回 `state_root` 或 `state-root claim`。
- 装着的线性包：一步失败后屏幕静止、包里什么都认不出时，多观察一个步骤时限，以 `contained_task_linear_unrecorded_screen` 结束，这个任务第一次出现就停住、阶梯照旧，包改了才放开；在
  `actingd suspended`、`ac_diagnose` 里看。页图包不变。
- 智能体做包：一步一次调用（`ac_lab_do` 带 `mark`、`next_frame_after_ms`），按名字找着按（定位文字的标记和跟随它的点击，名字录完还能改）；今天的 MCP 服务器多五个录制工具，共 46 个。
- 一台实例的会话关闭被拒不再停整个 Runtime，只隔离那一台；直跑的终端一步失败以后设备立即还回去。
- 救急模块多了建全新安装根的一整套：`new`、停机时的 `instance add`（新实例第一次启动先停着）、`vision manifest`、`vision check`、`vision place`、`state-root claim`；`standard-pack check` 报要哪些
  模型。这些给新安装根和图形安装向导用，生产不用。
- 目标：监控台的入口收整份目标文件；目标声明 `goals.json` 和第 3 版目标记录已就位，在用的安装上都还没有；目标从不给带不可逆包的任务加权，根组合不成时那个游戏的目标不生效。
- 接口：`actingd-config` [5, 32]、`package` [3, 12]；工具包的构建记录记 `5,32` 和 `3,12`；Runtime 包多一个模板文件。

本版合并的 PR：

- #1690 MAPP-1c（#437，登记 §1.350）：提前关闭的租约、按设备分的排队次序锁、锁次序表；主机门行 `#437 MAPP-1c` 和调度器的 `gate_mapp1c`，单应用对照测试重做过。
- #1691 T1-R（#455，登记 §1.351）：`ui` 收 `ApplyResourceTargets`、`expected_version` 的比较；主机门行 `#455 T1-R`，改了两条旧测试（`gate_429_r0` 去掉「未实现」的用例；LAB-6S3 的 `ui` 清单
  30 → 31 种）。
- #1695 ORD-1a、ORD-1b（#437，登记 §1.352）：`max_queue_wait_ms`、子系统 `queue_wait`、设备内的选择；策略门行 `#437 ORD-1a`、`#437 ORD-1b`（`gate_ord1`），目录金哈希不变。
- #1689 MAPP-2b（#437，登记 §1.353）：切换的四处调用方、失败结算、成员冷却和新键，`actingd-config` 30；主机门行 `#437 MAPP-2b`（`gate_mapp2b`）。
- #1692 FL-2（#450，登记 §1.354）：只加测试和文字；主机门行 `#450 FL-2`（`gate_450_fl2`）。
- #1693 S8、S7（#453，登记 §1.355）：`standard-pack check` 的 `models`、`actingd vision manifest`、`vision check`、两行救急操作、工具登记的成员和 DirectML 许可文件的钉值；两个新码；apps 和 exec 门行
  `#453 S7`、`#453 S8`，改了十一条旧测试（理由在登记册）。
- #1694 G-R1（#455，登记 §1.356）：`goals.json` 的契约、读取、检查、`package bundle --goals`、`place`，`package` 12；契约、exec、apps 门行 `#455 G-R1`，十五个新词。
- #1698 S1（#453，登记 §1.357）：`instance add` 的第一行；apps 门行 `#453 S1`，新夹具 `inst5-first-row.json`，没有改旧测试。
- #1700 S11（#453，登记 §1.358）：身份文件、认领记录、检查、认领和事实，四个新码；主机和 apps 门行 `#453 S11`，改了两条旧测试（默认标签表的条数；进程级启动测试从安装根再跑一遍）。
- #1701 MAPP-1d（#437，登记 §1.359）：探针、过期时的交接、按成员判的放行；主机门行 `#437 MAPP-1d`。
- #1703 MAPP-2d（#437，登记 §1.360）：切换时的租约和续租，新词 `lease_renewal`；主机门行 `#437 MAPP-2d`，`ci/retired-gates.toml` 两条（MAPP-2b 钉长租约和已知缺口的两条测试）。
- #1702 G-R2a（#455，登记 §1.361）：资源目标第 3 版；策略和主机门行 `#455 G-R2a`，改了三条旧测试。
- #1704 S2（#453，登记 §1.362）：`start_paused` 和 `host.start_pause_applied`，`actingd-config` 31；主机和 apps 门行 `#453 S2`，改了两条旧测试（接口钉 [5, 31]；默认标签表 16 条）。
- #1706（#457，登记 §1.363）：直跑的终端失败先放租约；改了 FL-2 门文件里钉旧行为的一条。
- #1707（#456、#458，登记 §1.364）：关闭被拒只隔离那一台；新门文件 `gate_458_close_isolates`，`gate_411_a52` 一条测试改名（`ci/retired-gates.toml` 一条）。
- #1711 S3（#453，登记 §1.365）：停机时的 `instance add`、`held_again`、`pauses` 一行；apps 门行 `#453 S3`，没有改旧测试。
- #1710 S4（#453，登记 §1.366）：`new` 和模板，一个操作种类、三个步骤种类、两个拒绝词；apps 门行 `#453 S4`，改了两处测试设施。
- #1709 S9（#453，登记 §1.367）：`vision place`；apps 门行 `#453 S9`，没有改旧测试。
- #1712 S12（#453，登记 §1.368）：`claim-state-root`、救急行、`op status` 的开关、启动比认领的规则；主机和 apps 门行 `#453 S12`，改了一条旧测试（`rescue_handed_rows_hand_what_they_declare`）。
- #1708 G-R2s（#455，登记 §1.369）：不可逆的规则只有一份、组合器的必填输入和 `irreversible_reach`、评估器把带标记的候选按没有记录来算、工作台按包的声明文件重读、组合不成的游戏目标不生效，
  三个新码；策略、主机和 apps 门行 `#455 G-R2s`（加 exec、主机的修正轮几行），组合器的输入改成必填，组合的各门照原样补上输入；改了两条钉着旧行为的测试（`gate_386_rlt::rlt_1_…`、
  `gate_434_lab2b::own_commands_print_the_cli_line`：现在每个注册的包钉一条 `standard_pack_package_unreadable`）。
- #1715 U1（#463，登记 §1.372）：线性一步的收尾判断、新码 `contained_task_linear_unrecorded_screen`、第一次出现就停住、阶梯类别 `pack_logic_mismatch`；exec 和主机门行 `#463 U1`（和它的修正轮），
  `ladder_tests.rs` 一条新测试；改了 `gate_e5_linear` 两条和 `gate_load4::linear_loading_pauses_the_arrival_budget`（收尾判断多出来的帧和暂停），`gate_373_nav6` 的 `run` 交给新的 `run_at`。
- #1717 L1（#463，登记 §1.370）：一步一次调用、`ac_lab_do` 的三个成员、五个录制工具；apps 门行 `#463 L1`，没有改旧测试。
- #1716 L2（#463，登记 §1.371）：定位文字的标记、跟随它的点击、生成的守卫和新帧复查、录完改名字，两个新词；exec 门行 `#463 L2`，没有改旧测试。
- #1713 ORD-1c（#437，登记 §1.373）：前台的成员、排队等待、把次序的输入交给评估器、新键，`actingd-config` 32；主机门行 `#437 ORD-1c`（`gate_ord1c` t1 到 t15），改了 `gate_mapp1b` 两条、
  `gate_mapp2b` 两条（次序给等待行添值、或换了挑中的一方，理由在登记册）和钉 [5, 32] 的架构测试。

## v0.12.59

本版是 v0.12.58 之后 main 上的 16 张 PR：7 个直接合并（#1672、#1669、#1671、#1674、#1673、#1678、#1679）和 3 趟并车（#1684 带 #1676、#1677、#1675；#1686 带 #1680、#1681、
#1682、#1683；#1688 带 #1685、#1687；车上除了各张的头，只有合并、登记册节号的调整，和两张在车上补的第三轮修正（#1676、#1685，各自并进了车））。按单子分：#434 LAB-6S2 的三片（LAB-6S2-M #1671、LAB-6S2-B1 #1673、LAB-6S2-B2 #1683），#451 的两片
（PLACE-PKG #1672、LOAD-P #1669），#453 的两张（S5 和 S6 #1680、S10 #1681），#454 的两张（#1679、#1685），#392 的两片（DA-3 #1682、DA-3b #1687），#437 MAPP-2a（#1676），#450 FL-1
（#1677），#449 REC-ARCH（#1675），#452（#1674），#441（#1678）。先说部署：**生产上跑的是 v0.12.54，11-14 上午换成 v0.12.56（跨过 v0.12.55 的单向一步，见 v0.12.56 一节）；v0.12.57、
v0.12.58 发布了、都没有部署；计划是从 v0.12.56 装 v0.12.58，再从 v0.12.58 装本版。** 从 v0.12.58 装本版：**能装，不用任何标志，计划里没有 `ledger_verify`，`defaults_diff` 是空的，装上以后能
退回 v0.12.58**。从 v0.12.56 直接装本版也能装、也不用标志，代价在退回那一头（见下面「次序」和「退回」）。没有新的配置键；接口只升了一项，`package` [3, 10] → [3, 11]（FL-1），不是单向的。
运行上要知道的是这几处：

- **本版定下的次序：本仓库的程序以 Lab 的身份连 Runtime**（#434 LAB-6S2-M、LAB-6S2-B1、LAB-6S2-B2，登记 §1.287「Slice LAB-6S2-M」「Slice LAB-6S2-B1」「Slice LAB-6S2-B2」；
  `contracts/lab-two-entries.md` §2.4、§4.2、§4.3）。
  - **谁改绑、报什么词**：每个程序都绑 `lab` 并声明发起者一词，信封照 0.12.58 不变。`actingctl` 的每条连接（自己的命令、操作层的命令、`install`、`start`、`restart`、`watchdog`、
    总览的 `watchdog` 一块）报 `Lab-CLI`（`apps/actingctl/src/main.rs` 153、412 起）；今天的 MCP 服务器（`actingctl mcp-serve`）的常驻连接和每次写的连接报 `Lab-MCP`
    （`apps/actingctl/src/mcp/runtime.rs` 352–360，总览那一块经 `front::set`，`mcp/mod.rs` 90–96）；`actinglab` 的核心行和自己的命令报进程的词（`Lab-CLI`，或
    `ACTINGCOMMAND_INITIATOR` 给的 `Lab-制包`、`Lab-救急`；`serve --stdio --front mcp` 和 MCP 的 `--via mcp` 子进程报 `Lab-MCP`、不带 `via`）；Lab 的 MCP 服务器进程内的连接和
    救急模块自己的客户端报各自的词、信封 `(Cli, Cli)`（`crates/lab-ops/src/front.rs` 94–137，`crates/lab-rescue/src/runtime_start.rs` 121–123）；看门狗的探测报 `Lab-看门狗`
    （`apps/actingwatch/src/main.rs` 57），只发 `Health` 和 `BindChannel`。只剩 `--via ui` 和一个不是词的 `ACTINGCOMMAND_INITIATOR` 下 actinglab 自己的命令照旧绑 `lab` 不带词。
  - **为什么记录不变（映射 M 和回执规则 R）**：主机在门口把「词 + 信封来源」映射回那个程序在 0.12.58 上的通道（`crates/actingcommand-contract/src/runtime/interface.rs` 222–250
    `ChannelBinding::recorded`，门里只用一处，`crates/runtime-host/src/host/requests.rs` 41–62）：`Lab-MCP` 下 Lab 信封记 `lab` 带 `via` `mcp`、别的信封记 `mcp`；另四个词下 Lab 信封记
    `lab`、别的信封记 `cli`；不带词的绑定原样记，映射永远得不出 `ui`，所以 UI 清单那道闸只判绑在 `ui` 上的连接。回执：信封来源是 Lab 的请求带连接的绑定和词，别的信封带映射后的通道、
    不带 `initiator`，`BindChannel` 自己的应答照旧。于是信封、记录和 `actingctl`、今天的 MCP 服务器拿到的回执都和 0.12.58 逐字节相同（`BindChannel` 那一帧和它的应答除外，那一帧多了词；门测试拿 0.12.58 的字面值守着）。**唯一变了的
    一条记录**：人手在 `serve --stdio --front mcp` 的输入里发核心行，0.12.58 记 `lab`，本版记 `lab` 带 `via` `mcp`；没有已发布的程序这样发。回执里变的只在 Lab 自家的程序：Lab 信封的
    回执多 `initiator`，MCP 子进程的回执不再带 `via` `mcp`、改带 `Lab-MCP`，`Lab-MCP` 下救急模块的回执写 `mcp`（原来 `cli`）；没有已发布的程序严格解码它们（见下面「控制台」）。
    没有接口修订、没有新码。
  - **0.12.57 及更早的 Runtime 碰上这些程序（两边的代码都核过）**：旧 Runtime 解不了带 `initiator` 的 `BindChannel`（v0.12.57 的 `crates/actingcommand-contract/src/runtime.rs`
    3777–3779 拒未知成员）。客户端每条新连接先发 `Health`、再发 `BindChannel`（本版 `crates/runtime-client/src/client.rs` 1008–1019），所以掉在第二帧：v0.12.57 的
    `crates/runtime-host/src/host/requests.rs` 1400–1408 解码失败就以 `runtime_protocol_invalid` 结束这条连接，在 1413 的准入之前、不给回执；随后在 1296–1300 记一条连接失败：
    `runtime.failed`，级别 error（这是请求级、不致命的错误，`error.rs` 718–728），诊断码 `runtime_protocol_invalid`，细节 `runtime_connection`，关联在每一帧开头清空（1364），所以
    不带实例、任务的关联（`host/lifecycle.rs` 491–539）。v0.12.56（`requests.rs` 1295、1406、1411）和 v0.12.54（1288、1399、1404）是同一段代码。被丢掉的只是绑定那一帧，写请求都在
    握手之后才发，**没有任何请求被执行两次**。客户端在同一属主纪元里重连一次，这个客户端以后不再发 `BindChannel`、以未绑定的连接继续（`client.rs` 1033–1035、1059–1066、
    2007–2018）；旧 Runtime 按第一条请求的信封归通道，`(Cli, Cli)` 和 `(Agent, Adapter)` 都归 `cli`（v0.12.57 `requests.rs` 56–84），于是今天的 MCP 服务器的请求在那边记成命令行；
    回答照那个 Runtime 自己的规则（包括它的来源门），信封没变，所以照旧收。每个进程往标准错误写一行 `client_channel_unsupported` 警告（`client.rs` 6965–6973；actingctl 的报告器
    `main.rs` 171–173 → `cli_line.rs` 21–23），退出码不受影响；看门狗的那一行写进 `watchdog.log`，不写标准错误（`apps/actingwatch/src/main.rs` 207、231–235）。**每建一条新连接，旧 Runtime 的账本多一条**：一次 `actingctl overview`（带安装根）2 条（它的连接和 `watchdog` 一块的
    探测），本版看门狗每次 tick 在属主锁被持有时 1 条，安装模块每次读写 1 条（`crates/lab-rescue/src/install/runtime.rs` 286–579，每个方法各连一次）。**看得到它们的**只有不带实例的
    `diagnose` 的失败分组、`events` 和控制台按级别的计数；没有守卫、启动后检查的哪一行、看门狗的关注或总览的警告读它们，什么都不会因此停下、拒绝或告警。0.12.58 的 Runtime 接受这个
    绑定，不产生这些记录（它没有映射 M：这些程序的请求都记 `lab`、回执带词，actingctl 和今天的 MCP 服务器设的设置行在控制台上显示成「经 Lab」）。
  - **在部署、退回、总览和看门狗的哪一步碰上**：部署由当时装着的那一版的安装工具跑，`--detach` 的分离运行重起的也是被调用的那个程序（本版
    `crates/lab-rescue/src/install/host.rs` 350–352、389–393；这两处只说明分离运行重起的是被调用的那个程序，用哪一版去调用由部署的人定）。所以**只要部署的每一步（连同
    `--detach` 的分离运行）用的都是在线那一版的 `actingctl install`**，从 v0.12.58 或 v0.12.56 装本版时，部署本身和部署失败时的自动回退都由不报词的旧安装工具做，它们
    **一条这种记录都不产生**；部署以后程序和 Runtime 同一版。这段时间里别的本版程序连到 0.12.57 或更早的 Runtime 照样每条连接记一条，比如 MCP 入口已经把子进程换成本版、
    而部署又自动退回了旧版的时候。要是改用本版的 `actingctl install` 对着 0.12.57 或更早的 Runtime 跑
    `update apply --stop --hold … --start`，旧 Runtime 关掉之前会记下 N + 9 条（N 是被挂暂停的实例数，登记 §1.287「Slice LAB-6S2-B2」逐条数过）。真正会碰上的是**从本版退回
    v0.12.56**（只有从 v0.12.56 直接装本版的那条路能退到它）：本版的安装工具在 v0.12.56 起来以后的就绪探测、启动后检查（`Status`、`RuntimeFactSnapshot`、`StartReport`）和交还暂停
    （`Status`、每台的 `ResumeScheduling`、`ScheduleExplanation`，`install/deploy.rs` 1237–1331）每条连接各记一条，那个进程写一行警告；退回以后、MCP 入口把子进程换回旧版本之前，
    本版的 MCP 服务器子进程每建一条连接也记一条。Tools 随 Runtime 一起退回，看门狗退回后是旧版本的，不报词。
  - **运维规矩：本版部署在 v0.12.58 之上；部署工具按退出码判断成败，不按标准错误有没有输出判断。**
- **安装工具的 `place`（Lab 救急模块的放包）**（#451 PLACE-PKG、LOAD-P，#453 S5、S6，登记 §1.339、§1.338、§1.345；`contracts/install-cli.md`「The Lab's rescue module」）。
  - **原来拒每个真正发布的包**（PLACE-PKG）：v0.12.58 的 `place` 拿读 Runtime、Tools 声明的 `parse_declaration` 读包的 `interfaces.json`（v0.12.58 的 `place.rs` 535），那个读法要六个
    v1 接口都在；资源仓发布的包只声明 `package`（加 `validated_with`），所以每个都在挂暂停之前以 `interface_edge` 被拒。现在按包自己的规则读（`crates/lab-rescue/src/install/manifest.rs`
    161–190 `parse_bundle_package`：schema 对、`speaks` 下每一项都是 `[min, max]` 整数对、必须有 `package`），边照旧：声明的上限要在本安装工具的 `package` 范围里。声明六个接口的包
    计划不变。
  - **载入模板目录 `loading/`**（LOAD-P）：原来布局检查不认 `loading/<文件>`，以 `pack_invalid` 拒（还没有发布的包带它）。现在只收压缩包自己的 `loading.json` 声明的模板
    （`place.rs` 559 起 `check_loading`）：没声明的、更深一层的、没有 `loading.json` 时的成员 `pack_invalid` 点名；声明了却缺的模板 `loading_template_invalid`（`path` 是缺的成员）；
    带这个目录的包里有叫 `loading`（任何大小写）的任务包 `loading_pack_name`（新词）；全在计划阶段、挂暂停和写任何东西之前。整个目录是一个条目，种类 `loading`，排在任务包之后、
    根文件之前；`new`、`replace`、`unchanged`，后来的包不带时 `retire`，挪成 `loading.old-<毫秒>` 留着；`undo` 按种类和摘要反做，什么都不删。不带这个目录的包放到没有它的根上，
    计划、计划号、步骤和流水和以前一样。`package` 不动。
  - **没有实例绑这个游戏时不再空等**（S5）：原来放完、`schedule.json` 变了，就等 Runtime 换出新目录，最多 `standard_packs.check_interval_ms` 加 `install.hot.idle_wait_ms`（默认约
    11 分钟）；而工作台只读配置了实例的游戏，于是白等以后 `post_check_failed`、暂停留着。现在流水的 `inputs` 记下状态里绑这个游戏的实例 `bound`；空的就读一次目录、直接过
    （`expected` `unchanged`、`bound` `[]`）；没有 `bound` 的旧流水续跑照旧等。
  - **Runtime 停着也能放**（S6；`crates/lab-rescue/src/rescue.rs` 1842 起 `place_cold`）：原来没有 Runtime 应答就 `runtime_not_running`（v0.12.58 的 `rescue.rs` 1722）。现在的次序：
    计划照旧算（不含运行状态，同一个包停着和跑着计划号相同）→ 拿写锁 → 有没做完的操作就拒 → 照停机规则判（`install::launch::refuse_running`：属主锁被持有 `owner_lock_held`、
    根下有 actingd 进程 `runtime_running`、判不了 `processes_unreadable`，在锁下判）→ 开 `pack_place`（`inputs` 带 `cold: true` 和按配置算的 `bound`，没有 S0、没有守卫）→ 每个变了的
    条目一步 `pack_item`（都不标 `first`，不等空闲）→ `post_check`；没有 `pause`、没有 `release_hold`。事后检查四行：`rerun`、`standard_pack_check`、`check_config`（现装版本的
    check-config 对现配置，带 `--install-root`；按它不截短的报告判这个游戏：游戏根或表的问题、点名这个游戏的组合问题、整体被拒的组合、记在别的游戏上的 `rule_id_conflict` 都判失败；
    答复被截短、报告里这个游戏一张表都没列时 `game_tables` `not_judged`）、`composition`（`not_evaluated`，`runtime` `stopped`）。哪一行没过就 `post_check_failed`，条目已经放好，
    `next` 只给 `undo <操作号>`；`undo` 把放上的改名成 `.rolledback-<毫秒>`、把原来的换回来。冷放置的 `undo` 和续跑也先拿写锁、照停机规则判（`install/undo.rs` 205–215，
    `install/update.rs` 757–764）。完成的回答 `next`：有实例绑这个游戏时 `composition_at_start`（`up`），没有时 `composition_at_start_once_bound`。没做完时 `current.json` 记着这个
    操作，看门狗第 7a″ 行不放行，半放好的游戏根上起不来 Runtime。
  - **已知的口子**：`check_config` 这一行的 check-config 子进程到 `install.child.check_config_timeout_ms`（默认 900 秒）不结束、留着（`install/programs.rs` 98–114 经 `run_program`）；
    它留在根下时，安装模块（包括这次冷放置的 `undo` 和续跑）和看门狗会把它当成在跑的 Runtime（`runtime_running`），直到它自己退出。只有下面 `emulators` 那一行的子进程到时会被结束。
  - **生产不用它**：生产的放包走总览自己的放包工具包，救急模块的 `place` 到 LAB-7 前不用在生产根上。
- **新根上的 `instance add` 和「根的第一段」**（#452，登记 §1.300「Fix #452」；#454，登记 §1.341、§1.348；`contracts/install-cli.md`「The post-start checks (INST-2)」）。
  - **`instance add` 写 `application_id`**（#452）：原来写出的条目只有五个成员、没有 `application_id`，而 actingd 的装配要求每个按发现绑定的条目有非空的 `application_id`
    （`apps/actingd/src/config.rs` 2780、2901，`application_identity_missing`），所以现装版本的 check-config 拒掉它拼出的每份候选（`config_refused_by_target`），候选留在现配置旁边。
    现在这一行多一个可选参数 `--application-id`（按装配自己的规则核：非空、最多 256 字节、没有控制字符，`crates/lab-rescue/src/install/instance_add.rs` 107）；不给就取游戏根下
    `applications.json` 给这个服务器声明的 `servers.<server>.application_id`（459 起）；两边都没有就在写任何东西之前 `instance_invalid`。条目变成六个成员；流水的 `instance_add` 输入和
    回答的 `added` 多 `application_id`、`application_id_from`（`given` 或 `declared`）。生产照旧手工加实例。
  - **没过的启动后检查把读到的写进它那一步**（#1679）：原来检查没过、以 `post_check_failed` 结束的操作（`config replace`、`config restore`、`instance add`、组合回退），流水里
    `post_check` 这一步停在 `started`、`evidence: null`，`op status` 说不出哪一行没过。现在这一步以 `done` 结束，证据是 `{commit, checks, failed}`，和检查通过时同样的三项，跟操作结果
    在同一次写里落盘（`install/deploy.rs` 788–794，`install/journal.rs` 380–389）；写之前崩掉，这一步还是 `started`，续跑会重新检查。`done` 只表示检查跑完了，过没过看 `failed`。
  - **根的第一段重启不再被误判**（#1685）：一个根的第一次启动找不到更早的启动记录、不写「带标签的状态已启用」标记，所以第二次启动按设计 seed 一次再写标记。安装模块的 `no_seeding`
    在有 S0 时只认 `load`，于是在新根上重启一个还在第一段的 Runtime 的操作（运行中 `config replace`、`instance add`、组合更新或回退、`install stop` 后 `install start`）都以
    `post_check_failed`、`no_seeding` `{"tagged_state": "seed"}` 结束（在隔离的新根上复现过；生产的根早过了第一段）。现在：
    - Runtime 每次启动都和配置事实一起记一条程序事实 **`config.start`**：一行 `{owner_epoch, first_start, tagged_state}`，即本次的属主纪元、是否没找到更早的启动记录、这次是
      `seed` 还是 `load`；运行时范围、`record_list`、来源 `runtime`、没有有效期；在 `config.parameters.*` 之后、任何实例准备之前写，覆盖上一次启动留下的那一行；写不进就启动失败
      （`crates/runtime-host/src/host.rs` 2544–2551，`host/runtime_config.rs` 188–224；`contracts/runtime-fact-store.md`「Producers」）。只有带配置清单的启动记它，actingd 总带。
    - 安装模块在 `request-shutdown` 之前本来就要读一次事实快照（S0 的第二次读），从里面取这一条（`install/runtime.rs` 352–356，不多建连接）；事实的纪元等于 S0 的纪元、`first_start`
      为真、`tagged_state` 是 `seed`（`install/postcheck.rs` 648–652），而且根上别的操作的 S0 里没有别的纪元（第二道防线，`install/deploy.rs` 439–465），才把 `first_epoch: true` 记进
      关停那一步的证据（404–428）；读不到记 `first_epoch_unread`（失败的码、`runtime_not_running`，或旧 Runtime 的 `fact_absent`，388–397），防线拒了记 `first_epoch_guard`
      （`{operation_id, owner_epoch}`，某份流水读不了时 `{operation_id, journal_unread}`，流水列不出时 `{journals_unlisted}`）。S0 和流水的结构都不变。
    - `no_seeding`：有 `first_epoch` 时 `seed` 和 `load` 都过，值 `{tagged_state, first_epoch: true}`；没有时和以前逐字节一样（`postcheck.rs` 305–330）。`install start` 读停机的那次操作
      的证据，带在自己流水的 `inputs.first_epoch`（`deploy.rs` 2010 起、2093–2105）。
    - 顺带：`bindings` 改按重启后拿到的配置判（换文件以后的新文件，`postcheck.rs` 635–640），删掉实例的新文件不再误判；加进来却没注册上的实例现在 `bindings` 不过。
  - **谁读 `config.start`**：只有本版起的安装模块；本版之前的安装工具只读 `host.config_overrides` 和 `config.parameters.*`（v0.12.58 的 `install/runtime.rs` 292–322），不看它；控制台、
    `actingctl status --config` 各挑自己的键；列全部程序事实的（`facts --program`、`ac_facts`）多显示一条。关停的是 0.12.58 或更早的 Runtime 时没有这条事实，照旧要求 `load`，所以新根
    第一段的重启要等起它的 Runtime 是 0.12.59 或更新的才过。生产的根上只是每次启动多一行事实，策略不读它。
- **Runtime 没开时列出本机的模拟器实例**（#453 S10，登记 §1.346；`contracts/emulator-control.md`「Offline instance discovery」）。
  - `actingcommand-actingd discover [--config <文件> [--install-root <根>]] [--mumu-root <模拟器目录>]`（`apps/actingd/src/discover.rs`）：配置文件只读 `mumu_root`、`allow_env_overrides`
    和按发现绑定的条目的绑定键，各按启动时的规则判；照 check-config 的办法找模拟器安装；只跑厂商工具的 `version` 和 `info -v all` 各一次，各限 10 秒
    （`crates/device/src/mumu_manager.rs` 50）；没指名目录时多一次进程列表，限 10 秒（69）。不碰状态根、不取属主锁（A/B 安装下只像 check-config 一样拿所在槽位的共享锁）、不读账本、
    不写任何文件；动手之前先把自己的标准输出、标准错误设成不可继承，免得厂商工具的后代攥着调用方的管道。回答 `actingcommand.actingd.discover.v1`：`mumu_root {path, source}`、
    `provider_version` 和与在线发现同形状的 `instances`，带 `--config` 时每个实例带绑它的条目的 `bound_alias`（几个条目指同一台取实例编号最小的；只写主机和端口的条目离线不匹配；
    check-config 会拒的配置照答，别名只是提示）。成功码 `actingd_discover_completed`（新）；失败沿用 `instance_discovery_unavailable`、`mumu_manager_version_unsupported` 和
    check-config 的装载码。
  - 救急行 `emulators`（新，没有参数，只读、离线）：跑现装版本的 `actingd discover --config <这个根的配置> --install-root <根>`，限 60 秒，到时结束并回收这个子进程
    （`crates/lab-rescue/src/install/programs.rs` 325–362，`install/host.rs` 273–304 `run_program_ending`）；失败是 `install_command_refused`，`install_refusal` 新词
    `emulator_discovery_failed`，actingd 的结果是原因，到时被结束的带新的可选键 `timeout_ms`（60000）、没有原因。命令行、`serve --stdio` 和 Lab 的 MCP 服务器都有；救急模块的行
    从 33 变成 34。
  - 启动时的实例发现跑的东西不变（同一个函数配系统的读法）。Runtime 运行时跑它，厂商工具就同时被两方读，两次 `info` 同时跑会怎样没有核过。
- **录制可以只留素材地收掉**（#449 REC-ARCH，登记 §1.344；`contracts/lab-recording.md`「Archiving」；`crates/lab/src/recording/archive.rs` 188 起）。
  - **为什么 `record stop` 收不掉**：它是用来出包的，要求每步都有识别标记、最后一步不带点击（`record_step_without_recognition`、`record_final_step_has_click`），只为留素材的录制过不了，
    会话就一直是进行中；之后同一台实例上任何人带 `--record` 的动作都会灌进去（生产上有一台实例的录制从 51 步涨到 91 步），要换录制只能 `record start --force`，丢下旧目录。
  - **`record archive [--record-id <id>] [--copy-to <空目录的绝对路径>]`**：不对步骤提要求。在录制自己的目录里写 `archive-manifest-<n>.json`（schema
    `actingcommand.lab-recording-archive.v1`；表头有 `record_id`、实例、`task_id`、开始时间、应用、归档时间、步数、写它的构建、步骤表的文件和 sha256；每步一行：种类、帧文件和 sha256、
    前后页、包、点击或滑动、时间、请求号），只从步骤表和会话头写，不发 Runtime 请求、不读帧的像素；`n` 取已有的最大号加一，从不替换。会话头记成 `archived`；步骤表和帧一个字节不动，
    什么都不删。`--copy-to` 只收不存在或空的目录的绝对路径（空、全空白、相对路径 `validation_failed`，非空 `record_archive_target_not_empty`，都在写任何东西之前），复制帧、步骤表和
    清单以后逐个核 sha256（核不上 `record_archive_verify_failed`；不删已复制的，录制仍是已归档，可按 `--record-id` 重试，不写第二份清单）。按编号也能归档 `record start --force` 丢下的
    录制（从它自己的步骤表写清单，不动现在的会话头）。
  - **`record reopen [--record-id <id>]`**：把已归档的录制变回进行中（另一个在进行时 `record_session_active`，没归档的 `record_not_archived`），之后照常标记、`record stop` 出包；再归档时
    写下一份清单，旧的留着。
  - **滑动成为一步**：在没有点击的步上 `do --capture --record --swipe x1,y1,x2,y2,ms` 记成这一步的滑动（原来 `record_step_click_missing`），`record stop` 照任务格式现成的 `drag`
    生成；没有滑动步的录制生成的字节和以前一样。带 `--record` 的动作的回答里 `record` 多 `record_started_at_unix_ms`，和原有的 `record_id`、`step` 一起说明进了哪个录制、成了第几步。
  - **新码三个**（`lab`，error）：`record_archive_target_not_empty`、`record_archive_verify_failed`、`record_not_archived`。命令行（也作 `session record archive|reopen`）、`serve --stdio`
    和 Lab 的 MCP 服务器（`ac_record_archive`、`ac_record_reopen`，经 `serve` 子进程）都有；今天的 MCP 服务器不加。
  - **更早的版本留下的、还开着的录制**：步骤表是 0.12.47 起的格式，本版的命令行能直接 `record archive`（被 `--force` 丢下的按 `--record-id`）。部署本版以后把生产上那台一直挂着的录制
    归档；归档之前那一台照旧谁都不带 `--record`。
- **显示适配的输入侧**（#392 DA-3、DA-3b，登记 §1.347、§1.349；`contracts/display-adapter.md` 第 10 节）。
  - **在用的安装上什么都不变**：每个在用的安装都把设备跑在正好 1280×720、一台设备一个应用，包也是 1280×720，映射是普通的恒等；恒等下宿主什么都不换算、不保留、不多记，发出的输入和
    记录与 0.12.58 逐字节相同（`crates/runtime-host/src/host/contained_task.rs` 2804–2812、5372–5382；主机测试
    `da3_identity_a_1280x720_package_on_a_1280x720_device_records_and_sends_as_before` 拿原来的字面值守着）。下面的缩放路径在这样的安装上一条都走不到。**本版之前没有哪台设备在缩放的
    画面上量过；下面这些只经测试核过**，用缩放的设备之前要先实机验。
  - **放行的缩放和为什么**：恒等，和约分后分母是 1 或 2 的纯缩放（3/2、2、5/2、3……；对 1280×720 的包是 1920×1080、2560×1440、3200×1800、3840×2160）。只有这些倍数保证每个基准框里的
    每个点都落进它向内取整的设备框里（`crates/display-adapter/src/lib.rs` 378–383 `doubled_is_whole`，932–946 `admit_input`；适配器测试枚举了框和点）。
  - **在哪换算**：运行的每个输入都在宿主的同一处出去（`contained_task.rs` 5372–5382）；非恒等时先过 `admit_input`，再核映射还是不是做决定的那一帧的（`current_for`），再换算
    （2847–2915；`lib.rs` 964 起 `device_input`）：点取 `round_half_up(v · s)`，必须落在设备帧里、也落在它所画的框换算后的设备框里，不夹回；刹车的横向长度按比例；路径拖动的框向内取整；
    时长、停顿、斜率和点数不变。
  - **两个意图记什么**：效果意图照旧记包要的输入（基准坐标、取样区域、`frame_extent`）；输入意图记实际发出去的（设备坐标）。记录不加成员。
  - **竖向刹车拖动**（DA-3b）：倍数不为 1 时按「拖 200 毫秒、停 150 毫秒、刹车 200 毫秒」的停顿刹车拖动发，起点和拐角照上面换算，刹车终点在拐角正上方 `round_half_up(100 · s)`，出了
    设备帧就拒、不夹回（`lib.rs` 1027 起、1372 起）；两种拖动在设备层是同一串事件（主机测试逐事件比过）。效果意图记包里的竖向刹车拖动，输入意图记发出的那一种、不带执行计划。倍数为 1
    时照旧发竖向刹车拖动。不改接口。
  - **每一种拒绝和它的时刻**（都在任何输入之前；`display_map_unresolved`，阶段 `display_input_map`，带帧和基准的尺寸）：平移或裁剪的画面 `translation_unsupported`、转了的画面
    `orientation_mismatch`、分母不是 1 或 2 的纯缩放 `input_scale_unsupported`（第一个输入就拒、不看点在哪，例如 1280×720 的包在 1600×900 上；只截图识别、不发输入的运行不受影响）、
    框换算后没有设备像素 `collapsed`、点在缩放帧外 `outside_frame`；换算后的点落到设备帧或设备框外 `contained_task_input_out_of_bounds`（放行的倍数下合法的点碰不到）；输入依据的帧和
    映射建立时的尺寸不同 `display_map_stale`（警告，第一次真的发出），运行结束。取帧以后才转屏的检测不到，归转屏验证那一片。
  - **Lab 的声明操作**：它的运行时不读游戏声明、帧也不带几何，所以只在包分辨率大小的帧上发输入；别的帧上在保活和任何输入之前以 `display_map_unresolved`（`input_scale_unsupported`）让
    这一个操作在 `input` 阶段失败（`crates/runtime-host/src/host/lab_operation.rs` 1518–1547、1838–1853）。原来会把基准坐标原样发到大画面上。
  - **事后取证配对**：`actingledger task-evidence` 把效果意图的输入用产品自己的 `admit_input` 和 `device_input`，按这次运行最后一条几何记录的基准和效果意图的 `frame_extent` 换算后再比；
    几何记录在整本快照里往回找，分页读和整读结果一样；恒等下照旧逐字节比（`crates/ledger-forensics/src/input_pairing.rs` 1–20）。为此取证 crate 多依赖显示适配 crate（它只依赖契约
    crate），架构守卫的白名单加一项。
- **还用不上、作为部件发出的**：
  - **应用切换的部件**（#437 MAPP-2a，登记 §1.342；`contracts/device-group.md`「The switch before a run」）：在设备钥匙下停掉同组另一个成员的应用、在借来的钥匙上跑成员的启动包并原样
    交还钥匙、忘掉被停成员的前台应用和页面，和它们唯一的入口 `RuntimeHost::switch_application`（`crates/runtime-host/src/host/switch.rs` 472）；Runtime 里没有调用方，加载照旧拒每个
    多成员的设备组（`device_group_config_invalid`、`several_members_unsupported`，`apps/actingd/src/config/device_group.rs` 381）。新码 `application_switch_failed`、词表
    `application_switch_step`、位置 `switch_application`，本版不会发出。在用的安装都是一台设备一个应用，走不到这里；运行前的切换、它的失败结算和成员的冷却（MAPP-2b）在下一版。
  - **由字段组成的列表**（#450 FL-1，登记 §1.343；`contracts/resource-readings.md`「Lists from fields」）：任务格式 `0.8`、`0.9` 的任务可以声明一个 `field_lists`（一组 OCR 字段、一个当键、
    其余当列，自己的置信度下限、事实键、有效期、`start`、`end`）；规则只在契约 crate 一处（`crates/actingcommand-contract/src/taskflow.rs` 2066 起 `validate_field_lists`），声明门
    （`crates/pack-containment/src/source/declarations.rs` 1488、1546）和准入（`crates/execution-kernel/src/contained_task/field_list.rs` 203 起）都调它；内核按帧攒读数，读全才建表，
    重复的键、读不出的行、没见到开始或结束都整张不写（`reading_problem` 新词 `list_start_unseen`、`list_end_unseen`，可选值 `frame_indices`）。**按代码，本版的 Runtime 收下声明它的包**：
    内核在结束帧给出和 `read_list` 一样的痕迹，主机照 `read_list` 那条路发布（`contained_task.rs` 6328–6400）；作为层或恢复入口时照读数的规矩拒（`crates/execution-kernel/src/contained_task.rs`
    3590–3598）。**本版没有的**：主机这一半的测试（FL-2）——契约原写 FL-1、FL-2 同一个 Runtime 版本发出（`resource-readings.md` 903–904），本版只有 FL-1；也没有发布的包声明它
    （FL-3）。声明它的包要在 `interfaces.json` 写 `package` [11, 11]：本版的 `place` 收，之前的 `place` 在计划阶段以 `interface_edge` 拒，之前的 Runtime 在准入以
    `resource_declaration_invalid`（`/field_lists` 未知成员）拒。在用的安装上没有包声明它。
- **第三方登记：视觉运行库随安装包分发**（#441，登记 §1.340；`NOTICE.md` 的 ONNXRuntime「Release boundary」）：Windows 安装包（图形安装向导用的完整离线包，在本仓之外拼、发在伞仓）按
  #441 的决定带 ONNX Runtime 1.24.4 的 DirectML 版（`onnxruntime.dll`、`onnxruntime_providers_shared.dll`，MIT，带 `LICENSE`、`ThirdPartyNotices.txt`）、DirectML 1.15.4 的
  `DirectML.dll`（DirectML 自己的许可条款：只能随应用、在 Windows 上用、不单独分发）和 PP-OCRv6 中等检测、识别模型和字典（Apache-2.0，带 PaddleOCR 的 `LICENSE`），各带大小和 SHA-256。
  工具登记 `scripts/windows-tools/windows-tool-sources.v1.json` 里这三个组件的 `license.redistribution` 改成 `redistributed_in_installation_package_by_owner_decision` 并带说明；
  `onnxruntime-gpu-1.24.4` 照旧只作本地缓存。**本版的两个压缩包没有多带任何文件**：Runtime 包的文件清单和 Tools 包的 `$toolFiles`（`.github/workflows/windows-remote-build.yml`
  659–678）自 v0.12.58 没改，`.github/` 整个没改；Runtime 包里的 `INSTALL.md` 换了一句「谁带这些文件」。没有产品代码改动（只有 `runtime_pin.rs` 的一段说明）。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`、`instance add`、`emulators`，也不经 Lab
  的 MCP 服务器；救急模块的 `place` 不用在生产根上。
- **生产上**：自 2026-11-11 起有一台实例在试跑调度，其余实例暂停着；每个在用的安装都把设备跑在 1280×720、一台设备一个应用。部署本身不改变派单。

配置：按代码、`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md` 核过，**升了的只有 `package` 一项，[3, 10] → [3, 11]（11，FL-1），别的没动**。
`component-interfaces.json`（4–10）：`actingd-config` [5, 29]、`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 8]、`runtime-client` [3, 8]、`runtime-outcomes` [1, 1]
没变，`package` [3, 11]（9）；架构测试 `tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120`（149–157）跟着钉。

- `contracts/component-interfaces.md`：新加 `package` 修订 11 一行（160）；`runtime-client` 修订 8 那一行（149）补了一句哪些回执带 `initiator`（规则 R），修订号不动。**谁声明 11**：
  Runtime 声明 [3, 11]；只有任务里用了 `field_lists` 的包在自己的 `interfaces.json` 里声明 [11, 11]，那由资源仓的打包脚本写（FL-3）；`resource verify` 的 `bundle` 步照旧写 [3, 3]、
  带 `notices/` 的写 [10, 10]，`package bundle` 不写 `interfaces.json`；不用这个成员的包保留原来的修订。**本版之前的程序对声明 11 的包**：之前的 `place`（范围 [3, 10]）在计划阶段、
  挂暂停和写任何东西之前以 `interface_edge` 拒（v0.12.58 的 `place` 本来就拒只声明 `package` 的包，见上）；之前的 Runtime 在准入以 `resource_declaration_invalid`（`UnknownField`，
  `/field_lists`）拒那个包；更新和退回的 `package` 边比的是安装登记里每个根的范围，登记里一律没有，照旧是 `pack_declaration_unknown` 警告。根上还没有这种包时，退回到 11 以下不受影响。
- 配置键：没有新增，也没有去掉。`distribution/windows/actingd.defaults.json`、`distribution/windows/actingd.config.example.json` 和 `crates/runtime-config` 自 v0.12.56 没改，
  `contracts/runtime-config.md` 自 v0.12.58 没改；`apps/actingd` 只加了只读的 `discover` 子命令和测试。`config.subsystems` 不变。
- 结果码：目录从 3924 个码、1751 个位置、642 个键、227 个词表变成 3929 个码、1753 个位置、644 个键、228 个词表，仍是 31 个分片；在用的 3896 → 3901，状态为 `retired` 的仍是 28 个，
  审阅定稿的仍是 172 个；词从 1672 个变成 1679 个。**新码 5 个**（都 pending、`since` 0.12.59）：`actingd_discover_completed`（actingd，success）、`application_switch_failed`
  （runtime-host，error）、`record_archive_target_not_empty`、`record_archive_verify_failed`、`record_not_archived`（lab，error）。**新位置** `install_emulators`、`switch_application`；
  **新键** `application_switch_step`、`frame_indices`；**新词表** `application_switch_step`（`foreground_read`、`stop`、`startup_package`）；**新词** `reading_problem` 的
  `list_start_unseen`、`list_end_unseen`，`standard_pack_problem` 的 `loading_pack_name`，`install_refusal` 的 `emulator_discovery_failed`。**改了的**：
  `contained_task_resource_reading_unresolved` 多可选值 `frame_indices`，`install_command_refused` 多可选值 `timeout_ms`，两者描述跟着改；`display_map_unresolved`、`display_map_stale`、
  `contained_task_input_out_of_bounds`、`check_config_install_root_invalid` 的描述和 `host_stdio_inheritance_failed` 的描述与处理改写；词 `input_scale_unsupported`、
  `list_position_unread`、`list_duplicate_key`、`loading_template_invalid` 的说明改写。没有码 `retired`。在用码里两段文字都写了的从 709 个变成 714 个（五个新码都写了）。本版的发布文件
  `contracts/outcome-codes/released/0.12.59.json` 没有码、没有位置（没有新的定稿或 `retired` 条目）。索引照旧由脚本生成，仍是 32 个文件，核过是最新的。
- Lab 的操作表（`contracts/lab-operations.json`）：171 行 → 174 行，多 `record archive`、`record reopen`（创作模块）和 `emulators`（救急模块）；`instance add` 多可选参数 `application_id`；
  `place` 的 `runtime` 从 `required` 改成 `optional`，说明跟着改。Lab 的 MCP 服务器跟着多 `ac_record_archive`、`ac_record_reopen`、`ac_emulators` 三个工具。
- Tools 包：文件清单 `$toolFiles`（`.github/workflows/windows-remote-build.yml` 670–678）自 v0.12.58 没改，布局仍是 `platform-tools-v3`（771）：七个程序加 `platform-tools/` 下的五个文件。
  Runtime 包的文件不变（659–666）。
- 程序文件：`Cargo.toml` 没改；`Cargo.lock` 只多两条内部依赖（取证 crate 依赖显示适配 crate；runtime-host 的测试依赖取证 crate），没有新的外部依赖；`.github/`、`ratchet/` 没改；`ci/`
  只有 `ci/retired-gates.toml` 多两条（DA-2a、DA-3 各一条改名的测试），不稳定表照旧是 FLK-08 和 FLK-10；actinglab 的命令清单仍是 47 个顶层分派臂、102 条命令；今天的 MCP 服务器仍是
  41 个工具。

升级与退回（部署由当时装着的那一版的安装工具做。安装工具的 `plan.rs`、`release.rs`、`adopt.rs`、`hold.rs` 自 v0.12.57 没改，所以 v0.12.58 一节写「本版和 v0.12.57 相同的 `plan.rs`」
的行号对 v0.12.58 和本版的安装工具都对；本版的安装工具改了 `deploy.rs`、`postcheck.rs`、`runtime.rs`、`place.rs`、`undo.rs`、`update.rs`（只在 735 以后，冷放置的续跑）、`programs.rs`、
`manifest.rs`、`journal.rs`、`host.rs`、`instance_add.rs`、`mod.rs`（`dispatch` 改成收发起者的词）。v0.12.56 的 `plan.rs` 就是 v0.12.54 的那份，下面写「v0.12.56 的」行号对 v0.12.54 的安装工具也对）：

- **v0.12.58 的安装工具装本版（计划的那条路）：能装，不用任何标志。** 安装工具的声明是它自己构建里的 `component-interfaces.json`（`host.rs` 23–25）：v0.12.58 的是 `ledger` [3, 8]、
  `runtime-client` [3, 8]、`package` [3, 10]。计划的边逐条（v0.12.58 的 `plan.rs`，和本版相同）：
  - `ledger` 两边 [3, 8]，`previous_max_in_next` 过、`one_way` 不触发（297–310），`ledger_verify` 是 false（924）；`runtime-outcomes`、`install-control` 过；`runtime-client` 安装工具 [3, 8]
    和 Rp [3, 8]、R\* [3, 8] 都相交（324–340）。
  - `package` `bundle_max_in_next`（341–364）比的是安装登记里每个包的范围，登记里一律没有（`crates/lab-rescue/src/install/adopt.rs` 134 写 `None`），所以每个游戏一条
    `pack_declaration_unknown` 警告，不挡；没有哪条边拿安装工具的 [3, 10] 去比目标的 [3, 11]。`speaks` `runtime_equals_tools`（765–772）：两半声明相同，过。
  - `actingd-config` `installer_knows_keys`（369–385）：本版没有新键，过；`next_check_config`：本版的 check-config 收 v0.12.58 的文件，过。`defaults_diff`（583 起）是空的 `[]`，部署后
    检查的 `new_keys` 没有要核的键。Tools 文件两边清单相同，没有 `new`。
  - **启动后检查的 `no_seeding` 是 v0.12.58 那一版的**（跑检查的是跑部署的安装工具，v0.12.58 的 `postcheck.rs` 292–310）：有 S0 就要求 `load`；它从本版 Runtime 读的是 `StartReport`
    的 `tagged_state`（形状不变），生产的根早过了第一段，答 `load`，过。它不读 `config.start`，关停那一步的证据里也不记 `first_epoch`。
  - **次序**：v0.12.58 的安装工具不报词，部署前连的是 v0.12.58 的 Runtime、部署后连的是本版的，都不掉线；v0.12.58 的 Runtime 本来就收带词的绑定，所以这条路上用哪一版的安装工具都不产生那种记录。能退回的是 v0.12.58。
- **v0.12.56 的安装工具直接装本版（跳过 v0.12.57、v0.12.58）：能装，不用任何标志。** v0.12.56 声明 `ledger` [3, 8]、`runtime-client` [3, 7]、`package` [3, 9]。边（v0.12.56 的 `plan.rs`）：
  `ledger` `previous_max_in_next`（288–294）过、`one_way`（295–301）不触发、`ledger_verify` false（915）；`runtime-client` 的 `installer_meets_previous`、`installer_meets_next`（315–331）
  只看区间有没有交集（`meet`，241–243）：[3, 7] 和 Rp [3, 7]、R\* [3, 8] 都相交，过；`package`（332–355）照旧是警告；`runtime_equals_tools`（756–763）过；`installer_knows_keys`（359–376）
  过；`next_check_config`（902–914）：本版的 check-config 收 v0.12.56 的文件，SBUD-1 拒的包在这里是一条 `pack_admission_suspended` 注记、不算失败（见 v0.12.57 一节）。`defaults_diff`
  （574–598）是空的，`new_keys` 没有要核的键。v0.12.57 的两个 Tools 文件（`actinglab-mcp.exe`、`actingcommand-mcp-entry-g2.exe`）在计划里是 `new`（815–819），和 v0.12.58 一节写的
  一样。工具包要是照 v0.12.56 那次还带着 `--accept-ledger-one-way` 也能过，但计划号会变、日志的 `acknowledged` 会记下它，去掉。`no_seeding` 是 v0.12.56 那一版的（同一段代码，
  `postcheck.rs` 292–310），生产根答 `load`，过。
  - **跳过 v0.12.58（和 v0.12.57）付的**：每一步都用 v0.12.56 的安装工具跑时，部署本身不掉线（它不报词，部署失败时的自动回退也由它做）；用本版的安装工具对着 v0.12.56 的 Runtime 跑，则每条连接各掉一次、各记一条（见上面「次序」）。v0.12.57、v0.12.58 的改动和本版一起到：SBUD-1 的准入检查（部署前照
    v0.12.57 一节用本版的 `standard-pack check` 查遍包根）、IDLE-3（临时设成 `none` 的 `idle.action` 部署以后可以撤回）、LAB-6S1b 拿掉来源门、LAB-6S3 的 `ui` 闸（控制台 v0.12.16 发的都在清单上，
    见 v0.12.58 一节）、notices 目录。**代价在退回那一头**：这样装上的只能退回 v0.12.56，而 v0.12.56 是 0.12.57 之前的 Runtime：退回以后本版的安装工具对它的就绪探测、启动后检查和交还
    暂停，以及换回旧子进程之前本版的 MCP 服务器，每建一条连接在它的账本里记一条 error 级的 `runtime_protocol_invalid`，每个进程往标准错误写一行警告，什么都不拒、不重做（见上面「次序」）。
    经过 v0.12.58 再装本版，退回去的是 v0.12.58，一条都不记。
- **v0.12.54 的安装工具直接装本版：能装，和从 v0.12.54 装 v0.12.56 一样要带 `--accept-ledger-one-way`**（预览、`install update plan`、`install update apply` 三步都带；不带时 `ledger` 的
  `one_way` 不过，以 `interface_edge` 拒，`plan.rs` 295–301、1030–1041），计划多一步 `ledger_verify`（915）。v0.12.54 声明 `runtime-client` [3, 6]、`package` [3, 8]：`runtime-client`
  和 Rp [3, 6]、R\* [3, 8] 都相交，过；`package` 照旧是警告。`defaults_diff` 是那三行（`recovery_ladder.profiles.without_emulator_restart.rungs`、`scheduling.task_held.after_ms`、
  `scheduling.task_held.enabled`），部署后检查的 `new_keys` 核这三个键；`installer_knows_keys` 不认识 `scheduling.task_held`，文件里不要提前写它；两个新的 Tools 文件是 `new`。v0.12.54 的
  安装工具也不报词，部署时不掉线。**比经过 v0.12.56、v0.12.58 差的地方**：装上并跑过以后同一个状态根上没有退回的路（见下），部署后检查不过时的自动回退也走不了；v0.12.55 到本版五个版本
  的改动在同一次部署里一起到；控制台要先换成 UI v0.12.16 或更新。没有让它装不上的东西。
- **退回（按安装工具的边和回退门核过）**：
  - **能退回的只有这次部署替换掉的那一版**（回退门第 2 部分 `ledger_readability`，`update.rs` 400–437，`live_operation` 181–204，自 v0.12.58 没改）。从 v0.12.58 装上本版的退回 v0.12.58
    能过，直接退到 v0.12.56 以 `rollback_gate_failed` 拒；从 v0.12.56 直接装上本版的退回 v0.12.56 能过。退回时边都过：`ledger` 8 在 [3, 8] 里、`one_way` 不触发；`runtime-client` 本版
    安装工具 [3, 8] 和两边都相交；`package` 是警告；`installer_knows_keys` 用本版的键表，没有新键；第 4 部分 `config`：目标的 check-config 读现行的文件（`update.rs` 490–503），本版没往
    文件里加成员（`instance add` 写的 `application_id` 是早就有的成员），过。部署失败时的自动回退由跑部署的那一版安装工具做，退向被替换的那一版。
  - **更早的构建对本版写下的东西**：
    - 程序事实 `config.start`：`config.` 族是更早的构建也认的九族之一（v0.12.56 的 `crates/actingcommand-contract/src/runtime_fact.rs` 85–121），照旧重放进事实库；更早的 Runtime 不写也
      不读它，这一行留着、写的是本版那次启动的纪元；列全部事实的读法多显示一条；以后再装 0.12.59 或更新的版本，安装模块先比纪元，不会误信。
    - 已归档的录制和它的清单：更早的 actinglab 拿会话头的 `status` 当字符串和 `active` 比（v0.12.58 的 `apps/actinglab/src/commands/session_record.rs` 565–578，
      `crates/lab/src/recording/steps.rs` 166），已归档的读作「没有进行中的录制」：`record start` 不用 `--force`，`--record` 动作和标记以 `record_session_not_active` 拒；更早的
      `record stop` 把它当成已停的会话、照旧尝试出包（步骤照旧要合规，回答带 `session_already_stopped`，v0.12.58 的 `crates/lab/src/recording_api.rs` 402–404）；清单是没人读的文件。
    - `pack_place` 流水：S5、S6 的 `bound`、`cold` 是 `inputs` 里的自由成员，更早的安装工具读得了、不认：续跑没做完的冷放置时照旧放条目（都不标 `first`），检查里没有 `check_config`，
      组合因为没有 Runtime 应答是 `not_evaluated`；续跑带 `bound` 的热放置照旧等目录。种类是 `loading` 的条目更早的安装工具解不了：撤销时报错、核对时跳过，要撤先用本版撤（生产上不用
      救急 `place`，不会有这种流水）。
    - `undo` 留下的 `.old-<毫秒>`、`.rolledback-<毫秒>` 副本：和 LAB-3 起的 `place`、`undo` 一直留下的名字一样，更早的构建照旧对待。
    - 带 `application_id` 的实例条目：早就有的成员，更早的装配照收；更早的 `instance add` 写出的又是没有它的五成员条目，被 check-config 拒。
    - 关停那一步证据里的 `first_epoch` 等成员、没过的检查那一步的 `done` 和证据、`start` 流水的 `inputs.first_epoch`：都是 `install-operation.v1` 的自由值，S0 和流水的结构不变，更早的
      安装工具照读，`no_seeding` 照旧要求 `load`。
    - FL-1、MAPP-2a 和显示适配在在用的安装上什么都没写：没有包声明 `field_lists`，切换没有调用方、也没有多成员的组，恒等映射下不换算、不多记。LAB-6S2 的三片不落盘。
  - **本版的工具在退回到 0.12.57 或更早以后**：Runtime 和 Tools 一起换回，看门狗是旧版本的；还在跑的本版程序（换回之前的 MCP 子进程、另外拿来用的本版 `actingctl`）照上面「次序」那样：
    每条连接掉一次、重连、记一条，照旧服务。退回到 v0.12.58 时它接受词：这些程序的请求都记 `lab`、回执带词，actingctl 和今天的 MCP 服务器设的设置行在控制台上显示成「经 Lab」，什么都
    不拒。
  - **从 v0.12.54 直接装上本版的**：哪一版都不让退回：退到 v0.12.54 或更早，`ledger` 的边不过（8 ∉ [3, 7]）；退到 v0.12.55 到 v0.12.58，边都过，但回退门第 2 部分只对被替换的 v0.12.54
    放行，以 `rollback_gate_failed` 拒；部署的自动回退和起过新 Runtime 的 `install undo` 同样被拒。出了问题剩下的是：配置的问题改好文件再起；程序的问题往前修；或者换一个全新的状态根。
- 配对：
  - **v0.12.54、v0.12.56、v0.12.58 写的配置文件**：本版照收，不用改。没有新成员、新键，也没有新的配置拒收；v0.12.54 写的文件只有 v0.12.56 起就有的那一个例外：两个条目没编成一组、却
    指向同一台设备，被拒。
  - **已装的包**：本版对不声明 `field_lists` 的包不改准入（FL-1 的门只认这个成员；内核对不带它的包在运行路径上只多几处判空；SBUD-1 是 v0.12.57 的）。`package` 的修订只在包自己声明 11
    时起作用，今天没有这样的包。本版的 `standard-pack check` 对今天的根答法和 v0.12.58 逐字节相同（`crates/resource-tooling` 自 v0.12.58 没改；契约 crate 只多了放包时才用的
    `loading_pack_name` 一个词）。
  - **控制台 UI**（最新发版 UI v0.12.17 是 UI 的 `ab31c42c`；UI main 在它之后只改了安装向导核安装集的那一处，和下面核的范围、请求无关；前一版 UI v0.12.16 是 `6c2e30f2`）：
    - **声明的范围**：v0.12.17 声明 `actingd-config` [5, 29]、`ledger` [3, 8]、`runtime-client` [3, 8]、`runtime-outcomes` [1, 1]（`crates/acui-setup/component-interfaces.json`），和本版
      逐项相合；v0.12.16 声明 [5, 28]、[3, 8]、[3, 7]、[1, 1]，安装器取共有的最高修订 28 和 7，和 v0.12.58 时一样。两版都声明 `package` [1, 2]，控制台和 Runtime 之间没有这条边
      （v0.12.17 的 `crates/acui-wizard/src/manifest.rs` 200–213 只核那四条）。
    - **它解码的**：v0.12.17 钉的 Runtime crate 在 `ef94d950`（v0.12.58 的发版提交），v0.12.16 钉在 `c0f75c7e`（v0.12.55）。本版在 `ui` 上的回答和回执没有新形状（契约 crate 的
      `runtime.rs` 自 v0.12.58 没改，映射 M 原样返回 `ui`）；事实快照多一条 `config.start`，是字符串和布尔两种标量的 `record_list`，`fact.rs` 自 `c0f75c7e` 只改了一段注释，两版都照读。
    - **`actingctl` 的回执谁解码**：两版发布的程序里没有严格解码 `actingctl` 回执的。v0.12.16 只发 `acui.exe`、`acforward.exe`（UI 仓 `.github/workflows/build.yml` 348），v0.12.17
      多发 `acsetup.exe`，是新的 0.12 安装向导（353）：它用新根自己的 `runtime\actingctl.exe request-shutdown --wait 60`（和那个根的 Runtime 同一版）关停，只看退出码、结果码和
      `shutdown.state`；用根里的 `tools\actinglab.exe` 以 `Lab-救急` 跑救急行；都按 JSON 值读、跳过不认识的成员（`crates/acui-wizard/src/runtime.rs` 1–12、223–297、401–422）。0.11 的
      安装助手（`crates/acui-setup`，会跑 `install-transition` 并严格解码回执）不在工作区里，两版都不构建它。控制台本身经钉的客户端绑 `ui` 直连 Runtime，不跑 actingctl 或 actinglab。
    - **它显示的通道**：控制台的账本页和设置页读记录里的通道，照映射 M 和 0.12.58 一样：actingctl 的请求是 `cli`（经命令行），今天的 MCP 服务器和 Lab 的 MCP 服务器进程内的请求是 `mcp`
      （经 MCP），actinglab 的是 `lab`（MCP 子进程的带 `via` `mcp`）。
    - v0.12.17 的安装向导自己核包的 `package` 边（`manifest.rs` 192–198），配本版的 [3, 11] 也收声明 11 的包。它在 0.12.58 上绕开的那个问题（新根上运行中 `config replace` 被
      `no_seeding` 判失败，`runtime.rs` 35–52）本版修了（见上，#454），它先关停再换的做法照旧能用。
  - **部署工具包的命令行**：照现在的做法，它用在线那一版的 `actingctl install` 跑每一步（上面「次序」里「一条都不产生」的结论以此为前提）：`release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop --hold --known-pause …`、
    `status --root [--operation]`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause … --start --detach`、`release-hold --root --reason`；
    另用目标构建的 `install status` 轮询是否上线，用目标构建的 actinglab 跑 `--json standard-pack check --root <游戏根>` 预查包根。从 v0.12.58 或 v0.12.56 装本版时，回答部署这几步的都是
    在线安装工具自己的代码，**没有一条被拒、没有一条换了答法**；目标构建的 `install status` 不连 Runtime（`install/status.rs` 216 起只读文件），`standard-pack check` 对今天的根答法和
    v0.12.58 相同。**有没有哪一行会把标准错误有输出、或第一行变了当成失败：没有。** 跑安装工具的那一处把标准错误并进输出、按退出码判；轮询上线和汇总守卫的两处从并起来的输出里取第一行
    以 `{` 开头的；解析计划、状态、分离日志的辅助脚本取第一行（分离日志取最后一行）以 `{` 开头的；预查包根的那一处分开读标准输出。本版的 `actingctl` 对旧 Runtime 打的那一行警告不以 `{`
    开头，不会被当成 JSON 行；而照工具包的做法部署时这一行根本不出现（在线安装工具不报词）。**工具包要跟着改的**：目标提交和标签；跑每一步的安装工具指向在线那一版；从 v0.12.56 或 v0.12.58
    装时预览、计划、应用三步都去掉 `--accept-ledger-one-way`；预期改成没有 `ledger_verify`、`defaults_diff` 为空、`new_keys` 为空（从 v0.12.56 装时另有两个 `new` 文件）；给热放置工具包写
    构建记录的那一步，核新包清单的 `actingd-config`、`package`、`ledger` 现在是 [5, 29]、**[3, 11]**、[3, 8]，记成 `5,29`、`3,11`、`3,8`（照旧的 `3,9` 或 `3,10` 会让那一步和后面的
    重钉都不过）。以后由本版的安装工具跑的操作，关停那一步的证据多 `first_epoch`（生产根上是 `false`），没过的检查那一步是 `done` 带证据；工具包的辅助脚本只打印这些成员，不严格解析。
  - **热放置工具包的命令行，本版都照旧收、答法和 v0.12.58 相同**：actinglab 两行，`--json standard-pack check --root <游戏根>` 和 `--json standard-pack compose --root <包根>
    --instance <别名>=<游戏>/<区服> … --out <新目录>`；actingctl 的 `status`、`status --config`、`pause --instance <别名> --reason <理由> --drain-timeout-ms <毫秒>`、`resume --instance <别名>`、
    `schedule --instance <别名>`、`overview --root <根> --instance <别名> --sections catalog`（都带 `--state-root`）；actingledger 的 `events --after … --through … --limit …`。部署以后它用的
    actingctl 和 Runtime 同一版：actingctl 绑 `lab` 报 `Lab-CLI`、信封 `(Cli, Cli)`，Runtime 照 `cli` 记录和回执，暂停、恢复、计划、总览的答法不变；`status --config` 只打印
    `config.subsystems`、`config.selection` 和 `config.parameters.*`，不显示 `config.start`；`standard-pack check`、`standard-pack compose`（`crates/policy` 没改）对今天的根不变；
    `apps/ledger-forensics` 没改，`crates/ledger-forensics` 只改了 `task-evidence` 的配对，`events` 不受影响。它读 actingctl 的地方都把标准输出和标准错误分开、按退出码和整段标准输出的
    JSON 判，标准错误只照抄进日志行。**热放置工具包要学的**：重钉时比的构建记录里 `package` 是 `3,11`；它的放包要是钉着 `package` 的上限，等声明 11 的包出现时再学（今天没有）。
  - **今天的 MCP 服务器（`actingctl mcp-serve`，现在注册着的那个）**：仍是 41 个工具，每个答法照旧。它的连接改绑 `lab` 报 `Lab-MCP`（写目标、补救、核对证据读取仍用代理调度器的信封
    `(Agent, Adapter)`，别的是 `(Cli, Cli)`），本版 Runtime 照映射 M 记成 `mcp`、回执不带词，记录和回执与 v0.12.58 相同（门测试拿真的 `mcp-serve` 核过）。看得到的差别只有：
    `ac_outcome_code` 认得五个新码和改写的文字；列全部程序事实的 `ac_facts` 多一条 `config.start`。**它碰上更旧的 Runtime 时注册的客户端看到的**：0.12.57 及更早：常驻的连接和每次写的
    连接各被丢一次、重连、不绑定，请求按信封归 `cli`、照那个 Runtime 的规则回答，工具的答案照旧，只是回执和设置行里的通道是 `cli`（设置页显示「经命令行」）；`client_channel_unsupported`
    那一行写在服务器进程的标准错误上（标准输出是协议），客户端在协议里看不到；旧 Runtime 每条连接多一条 error 记录。0.12.58：照收，记录写 `lab`、回执带 `Lab-MCP`，设置行在控制台上显示
    「经 Lab」。
  - **Lab 的 MCP 服务器（`actinglab-mcp`，没有注册指向它）**：多三个工具，`ac_record_archive`、`ac_record_reopen`（经 `serve` 子进程）和 `ac_emulators`（Windows 上由救急模块在它自己的
    进程里服务）；`ac_instance_add` 多参数 `application_id`；`ac_place` 在 Runtime 停着时也服务、收得下发布的包；`ac_do` 带 `record` 和 `swipe` 时把滑动记成一步，带 `record` 的回答多
    `record_started_at_unix_ms`；救急行的回执通道从 `cli` 变成 `mcp`。别的工具答法照旧。

运行上的变化：

- 从 v0.12.58 部署：不用标志，没有 `ledger_verify`，`defaults_diff` 为空，能退回 v0.12.58。本版部署在 v0.12.58 之上；部署工具按退出码判断，不按标准错误。
- 从 v0.12.56 直接部署也能装、不用标志、部署时不掉线；只能退回 v0.12.56，退回以后本版程序对它的每条连接在它的账本里记一条 `runtime_protocol_invalid`，照旧服务。
- 本仓库的程序都以 `lab` 加发起者一词连 Runtime，本版 Runtime 照 0.12.58 的通道记录和回执；控制台的账本页、设置页看到的和 0.12.58 一样。
- 救急模块的 `place` 收得下发布的包、认 `loading/`、没有实例绑定时不空等、Runtime 停着也能放；`instance add` 写 `application_id`；新根第一段的重启不再被 `no_seeding` 误判，没过的检查
  把读到的记下来。这些给新安装根和图形安装向导用，生产不用。
- 新命令 `actingd discover`、救急行 `emulators`、`record archive`、`record reopen`；部署以后把生产上一直挂着的那个录制归档。
- 显示适配在 1280×720 的整数倍、半整数倍画面上发输入（只经测试核过），在用的 1280×720 安装上什么都不变；`package` 升到 [3, 11]，还没有包声明它，工具包的构建记录记 `3,11`。

本版合并的 PR：

- #1672 PLACE-PKG（#451，登记 §1.339）：`manifest::parse_bundle_package`，`place` 的 `package` 边按包自己的规则读声明；apps 门行 `#451 PLACE-PKG` 的 4 个测试，没有改现有测试。
- #1669 LOAD-P（#451，登记 §1.338）：`place` 认 `loading/`，条目种类 `loading`，新词 `loading_pack_name`；apps 门行 `#451 LOAD-P` 的 4 个测试，没有改现有测试。
- #1671 LAB-6S2-M（#434，登记 §1.287）：映射 M（`ChannelBinding::recorded`）和门的回执规则 R，没有程序报词；主机门行 `#434 LAB-6S2-M`，S1a 的一条测试改用 Lab 信封发。
- #1674（#452，登记 §1.300「Fix #452」）：`instance add` 的 `application_id`，操作表一个可选参数；apps 门行 `#452 INST-5 application id`，其中一条让 actingd 的真装配吃下固定样例，
  INST-5 的假游戏根多一份 `applications.json`。
- #1673 LAB-6S2-B1（#434，登记 §1.287）：actinglab、Lab 的 MCP 服务器、救急模块的客户端和看门狗的探测报词；apps 门行 `#434 LAB-6S2-B1`，改了 4 条钉旧绑法的测试和协议金样里一条回执
  （多 `initiator`，手改，不是重录）。
- #1678（#441，登记 §1.340）：工具登记、`NOTICE.md`、`INSTALL.md`、`vision-model-folders.md` 的文字和 `runtime_pin.rs` 的一段说明；两条钉旧词的测试改钉新词，没有产品代码。
- #1679（#454，登记 §1.341）：`Operation::close_with_end`，没过的检查那一步带证据结束；apps 门行 `#454 POST-CHECK RECORD` 的 3 个测试。
- #1676 MAPP-2a（#437，登记 §1.342）：`host/switch.rs` 的部件和入口，新码、新词表、新位置；主机门行 `#437 MAPP-2a` 的 7 个测试，没有改现有测试。
- #1677 FL-1（#450，登记 §1.343）：`field_lists` 的声明、规则、门和内核，`package` [3, 11]；契约门和 exec 门 `#450 FL-1` 共 13 个测试，几处测试字面量只为编译补成员。
- #1675 REC-ARCH（#449，登记 §1.344）：`record archive`、`record reopen`、滑动成一步、三个新码，操作表两行；apps 门行 `#449 REC-ARCH` 的 12 个测试，改了 2 条（滑动从拒改成记一步，读清单的
  命令 29 → 31）。
- #1680 S5、S6（#453，登记 §1.345）：`bound`、冷放置、`check_config` 行、冷放置的 `undo` 和续跑；apps 门行 `#453 S5`、`#453 S6`、`S6 fix 1`、`S6 fix 2` 共 22 个测试。
- #1681 S10（#453，登记 §1.346）：`actingd discover`、救急行 `emulators`、`Host::run_program_ending`，一个新码、一个新词、一个新位置、一个新可选键；apps 门行 `#453 S10` 的测试。
- #1682 DA-3（#392，登记 §1.347）：输入侧的换算和拒绝、Lab 声明操作的尺寸核对、取证配对；适配器、主机、取证三处的新测试，DA-2a 一条测试改名（`ci/retired-gates.toml` 一条），架构守卫的
  取证依赖白名单加一项。
- #1683 LAB-6S2-B2（#434，登记 §1.287）：actingctl 和今天的 MCP 服务器报词；apps 门行 `#434 LAB-6S2-B2`，几处假 Runtime 改成回显词、两条钉旧绑法的测试跟着改。
- #1685（#454，登记 §1.348）：`config.start`、关停证据里的第一段、`no_seeding` 和 `bindings`；apps 门行和主机门行 `#454 FIRST EPOCH`，一条测试的结构体字面量补新输入。
- #1687 DA-3b（#392，登记 §1.349）：缩放下的竖向刹车拖动按停顿刹车拖动发；适配器和主机的 `gate_392_da3b`，DA-3 一条测试改名（`ci/retired-gates.toml` 一条）。

## v0.12.58

本版是 v0.12.57 之后 main 上的 9 个合并，都是单张 PR，没有并车。按单子分：#434 LAB-6 的四片（LAB-6S0 #1661、LAB-6S1a #1662、LAB-6S1b #1667、LAB-6S3 #1668），
#444 notices 目录的三片（NOTICES-1 #1660、NOTICES-2 #1664、NOTICES-3 #1666），#429 RG-STOP（#1665），#446 的一片（#1663，只动 CI）。先说部署：**生产上跑的是 v0.12.54，
v0.12.55、v0.12.56、v0.12.57 发布了、都没有部署**；计划是 11-14 先从 v0.12.54 装 v0.12.56（跨过 v0.12.55 的单向一步，见 v0.12.56 一节），下一次从 v0.12.56 装 v0.12.57 再装本版，
或者直接装本版。从 v0.12.56 直接装本版：**能装，不用任何标志，计划里没有 `ledger_verify`，`defaults_diff` 是空的，装上以后能退回 v0.12.56**；跳过 v0.12.57 不多付什么，
v0.12.57 的改动（SBUD-1、IDLE-3、新 MCP 服务器的两个 Tools 文件）和本版一起到，部署前照 v0.12.57 一节用本版的 `standard-pack check` 查遍包根。没有新的配置键；接口升了两项，
`runtime-client` [3, 8] 和 `package` [3, 10]，两项都不是单向的。运行上要知道的是这几处：

- **Runtime 不再因为请求自称的来源（信封）拒它**（#434 LAB-6S1b，登记 §1.287「Slice LAB-6S1b」；`contracts/lab-two-entries.md` §2.4）。
  - **原来拒的**（v0.12.57 的 `crates/actingcommand-contract/src/runtime.rs` 4773–4974，`RuntimeRequest::validate()` 里十四处按 `(actor, source)` 拒）：关机 `RequestShutdown` 和安装切换
    `InstallTransition` 只收人 `(User, Ui)` 和操作员 `(Cli, Cli)`；制包事件 `RecordAuthoringEvent`、签名的登记、匹配和撤下，调试事件、Lab 钉、受控 Lab 操作、画面识别、调试包和证据导出只收
    Lab `(Lab, Lab)`；身份卡只收人和操作员、批准只收人；模拟器控制、实例发现、自检、阶梯演练只收人和操作员；调度的暂停和恢复只收人和操作员；叫醒测试 `AgentWakeTest` 只收人和操作员；
    按键族发布事实只收代理调度器 `(Agent, Adapter)`，带来源例外的键族（手动优先级偏移、补救选择、包的日程）另收人和操作员；代理调度器的那些请求（规划会话的十种和 `ApplyResourceTargets`）
    只收代理调度器；玩家占用的取和放收人、操作员和 Lab。
  - **现在收的**：`validate()` 只查信封在不在客户端那几种之内（不在就 `invalid_client_origin`，这是信封自己的形状规则）和一次发布不混两个键族（`fact_origin_mixed`），别的形状和内容检查都不变
    （本版 `runtime.rs` 4785–4826）。信封一向只是声明、不是鉴权（`contracts/lab-two-entries.md` §2.1）：从本版起本机任何连得上 Runtime 的程序，不论自称什么，都能发暂停、恢复、关机、
    控制模拟器这些请求，以前只有自称是人或操作员的才行（actingctl 和今天的 MCP 服务器一直就能发）；`ui` 通道上的限制是下一条。
  - **Lab 自己的命令现在能用了**：`actinglab` 的命令行和 `serve --stdio` 用 `(Lab, Lab)` 信封（`crates/lab-ops/src/front.rs` 67–71），以前被拒、现在收下的是 `pause`、`resume`、`shutdown`、
    `discover`、`emu start|stop|restart`、`emu selfcheck`、`drill`、`wake-test`、`facts publish`、`offset`、`remedy set|lift`、`targets set`。Lab 的 MCP 服务器的核心行用 `(Cli, Cli)`（73–77），
    多通了 `ac_targets_set` 和发布不带来源例外键族的 `ac_facts_publish`（以前都是 `invalid_agent_dispatcher_origin`）。
  - **仍然被拒的**：Lab 的 `evidence read`。它先声明身份卡，主机自己对身份卡的来源复核照旧只收 `(User, Ui)` 和 `(Cli, Cli)`（`crates/runtime-host/src/host/governance.rs` 396–398，
    `invalid_governance_origin`）。主机另外两处复核也原样留着：批准只收人、并且这条连接的身份卡被接受过（`governance_authority_required`），安装切换除了 `Query` 都要身份卡被接受过
    （`install_identity_required`，`host/installation.rs` 705）。`hold`、`unhold` 是预留的行，主机照旧答 `not_implemented`。
  - **没有程序改了它发的东西**：actingctl、控制台、今天的 MCP 服务器、看门狗绑哪个通道、带什么信封都没变，回答也和以前一样。
  - **十个码标成 `retired`**（`retired_in` 0.12.58，没有 `replaced_by`：请求现在是被收下，不是换一个码拒）：`invalid_shutdown_origin`、`invalid_install_transition_origin`（0.12.0 定稿的）、
    `invalid_resource_authoring_origin`、`invalid_signature_origin`、`invalid_runtime_debug_origin`、`invalid_emulator_control_origin`、`invalid_scheduling_pause_origin`、
    `invalid_agent_wake_test_origin`、`invalid_agent_dispatcher_origin`、`invalid_player_hold_origin`。`invalid_governance_origin` 留着（主机的身份卡复核还发它）。
- **控制台的入口只收它的清单**（#434 LAB-6S3，登记 §1.287「Slice LAB-6S3」；`contracts/lab-two-entries.md` §2.3 第 5 步、§2.6、§3）。
  - **规则**：一条连接绑在 `ui` 上（自己 `BindChannel` 绑的，或者没绑、第一条请求带 `source: ui` 的信封被门归到 `ui`，`crates/runtime-host/src/host/requests.rs` 57–85），门只收握手的
    `Health`、`BindChannel` 和 UI 清单上的 30 种（`crates/actingcommand-contract/src/runtime/interface.rs` 541–582，`ui_allows`）；其中 `ConfigureSettings` 还要每个键的控制台档位是
    `daily` 或 `advanced`、并且键表里那一行能在运行中改（`crates/runtime-host/src/host/ui_gate.rs` 33–78；`crates/runtime-config/src/ui_tier.rs` 52–100；`contracts/runtime-config.md`
    1968–1985），按请求里的次序逐个判，清除一个键和设置它同样判，键表里没有的键没有档、直接拒。判断在请求自身的检查之后、任何工作开始之前（`requests.rs` 143–163），被拒的请求不占工作计数。
  - **`ui` 上从此被拒、以前会被处理的**：清单外的 47 种请求（Lab 的调试和制包、租约、输入、按路径跑包、发布事实、监视设置、整份资源目标、订阅、投影、规划会话、LAB-6S4a 要删的五种
    （身份卡、批准、安装切换、升级状态的两种）、叫醒测试、装卸包），以及点名这 11 个键以外任何键的设置（实例的 `stuck_recovery`、Lab 的两个选项、模拟器根目录、两个进程优先级、没有档位的键、
    未知的键）。回答是 Denied `channel_request_refused`（这个码第一次真的发出），值 `channel` 是 `ui`，`operation` 是那种请求的登记位置名（79 种里有 17 种线上的拼写不能当位置，用客户端或契约给它的
    名字，比如 `runtime_input`、`runtime_status`），设置还带 `config_key`，即第一个被拒的键；整条请求一个键都不生效。**记下的**：会被 G14 抄录的那些（写入，和会留记录的读，如
    `ObserveContainedPage`、`RecognizeArtifact`、`DebugPackage`、`ExportEvidence`）记一条 `command.rejected`，用命令记录本来就有的 `channel` 成员写 `ui`；纯读只在回执里
    （`ui_gate.rs` 102–142）。
  - **装着的控制台 v0.12.16 一样都不发**（在 UI 仓 v0.12.16 的源码上核的：它所有连接都绑 `ui`，`crates/acui-source/src/lib.rs` 574、837–861，`src/workbench.rs` 284–288、682）：它发的
    17 种是 `Status`、`RuntimeFactSnapshot`、`QueryEvents`、`ReadMaterial`、`ScheduleExplanation`、`StartReport`、`DiscoverInstances`、`ObserveReadonly`、`PauseScheduling`、
    `ResumeScheduling`、`ConfigureSettings`、`SelfCheckInstance`、`ControlEmulatorInstance`、`ApplicationLifecycle`、`RequestShutdown`、`RecordClientAction`、`StopInstanceRun`，外加握手，
    都在清单上；它的设置页发的 11 个键是性能预设的四个（`performance.pacing.mode`、`performance.pacing.max_concurrent_runs`、`performance.pacing.heavy_dispatch_limit.normal`、
    `scheduling.parallel.enabled`）、`scheduling.blackout.host`、`frame_retention_evidence_max_age_days`，和每台实例的 `scheduling.blackout`、`idle.action`、`start_emulator`、
    `touch_backend`、`capture_backend`（`src/workbench.rs` 493–540、`src/workbench/rules.rs` 444–509、`src/workbench/backends.rs` 438–439），在本版都是 `daily` 或 `advanced` 且能改，
    主机测试 `lab6s3_the_released_consoles_eleven_keys_are_taken_on_ui` 守着。它不发身份卡，也不订阅。UI main（`02fc7a36`）新加的 `PackScheduleView`、`EditPackSchedule` 也在清单上。
  - **别的通道照旧**：`lab`、`cli`、`mcp` 和没绑的连接这一版什么都不按通道判（那是 LAB-6S4b）。
  - **拿 `ui` 做测试的工具要改的**：清单外的请求和档位外的键改绑 `cli`（或 `lab`）再发；只是不绑、拿控制台的信封直接发也会被归到 `ui`，同样要先显式绑 `cli`。
- **门多认一个发起者**（#434 LAB-6S1a，登记 §1.287「Slice LAB-6S1a」；`contracts/component-interfaces.md` 149）。
  - **形状**：`BindChannel{channel, via?, initiator?}`，答 `ChannelBound{channel, via?, initiator?}`，这条连接的每张回执在 `channel` 旁边多 `initiator`；三处都是可选、没有就不写。
    `lab` 绑定可以声明的五个词是 `Lab-CLI`、`Lab-MCP`、`Lab-制包`、`Lab-救急`、`Lab-看门狗`（`runtime/interface.rs` 102–107；`UI` 不写，由 `ui` 通道隐含）。
  - **以 `invalid_bind_channel` 拒、连接保持未绑定的**：词写在 `ui`、`cli`、`mcp` 上，词和 `via` 一起，写 `UI`，写表外的词（`interface.rs` 191–221；`host/governance.rs` 206–255）。
  - **不带词的绑定**（`lab`、带 `via` 的 `lab`、`cli`、`mcp`、`ui`、第一条请求做的隐式绑定）回答和回执逐字节和以前一样，主机测试拿写死的 JSON 守着。本版没有任何程序声明词
    （`RuntimeClientConfig::with_initiator` 只有测试调用；声明是 LAB-6S2）。
  - **它定下的次序（按代码核过）**：本版之前的 Runtime 解不了带 `initiator` 的 `BindChannel`：v0.12.57 的 `RuntimeOperation` 拒未知成员（v0.12.57 的 `runtime.rs` 3778），解码失败就以
    `runtime_request_decode_failed` 掐断连接、不给回执（v0.12.57 的 `crates/runtime-host/src/host/requests.rs` 1399–1410）。本版的客户端遇到这种掐断，在同一个属主纪元里重连一次，
    之后不再发 `BindChannel`、以未绑定的连接继续，每个进程报一次 `client_channel_unsupported`（`crates/runtime-client/src/client.rs` 1021–1069）；旧 Runtime 再按第一条请求的信封给它归通道
    （v0.12.57 的 `requests.rs` 56–84），照那个 Runtime 自己的规则（包括它的来源门）回答，词哪里都不留。所以声明词的版本（LAB-6S2）应当部署在本版之上；部署在更早的 Runtime 上，或者退回到本版之前，都是上面这种降级：照那个 Runtime 的规则回答，词不留。
- **经 Lab 的 MCP 服务器部署和回滚**（#434 LAB-6S0，登记 §1.287「Slice LAB-6S0」；`apps/actinglab-mcp/MANUAL.md` 第 9、10 节）。
  - **变了的**：Windows 上 `ac_deploy`、`ac_rollback` 不再答 `mcp_operation_unsupported`，而是服务：服务器自己给这两行的 `detach` 填 true（`apps/actinglab-mcp/src/tools.rs` 136–141、
    245–256），救急模块起一个分离的 `actinglab.exe`（服务器旁边的那个；不在就当场拒、带路径，`crates/lab-rescue/src/install/host.rs` 293–317），带 `ACTINGCOMMAND_INITIATOR=Lab-MCP`
    和只有这一处会设的标记 `ACTINGCOMMAND_DETACHED_RUN=rescue`（`crates/lab-rescue/src/rescue.rs` 784–801）；Lab 进程只在标记在时才认变量里的 `Lab-MCP`（`crates/lab-ops/src/initiator.rs`
    121–143）。调用立即答 `install_command_done`、`detached: true`、分离进程的 `pid` 和 `log`（`<根>\install\detached-<毫秒>.log`）、`next` 指向 `op status`；回答里没有操作号，用
    `ac_op_status` 跟。分离运行的流水记 `Lab-MCP`，它对运行中的 Runtime 发的请求照旧走命令行通道。工具清单不变（Windows 上 143 个，现在全部服务）；`mcp_operation_unsupported` 留在码表里，
    提供的行都不会再发它。
  - **注册到它的客户端现在能起什么**：经二代入口注册到这个服务器的客户端，它的代理能对那个根发起部署和回滚。「由人决定」是手册第 10 节和服务器说明文字里的规矩（`MANUAL.md` 257–262），
    服务器不拦，照样服务、只记谁发起。**安装不会注册它**：没有安装步骤写 `mcp-entry\` 或客户端的注册，`mcp config` 只打印片段（`MANUAL.md` 241–246），所以主人改注册之前什么都碰不到。
  - **生产部署到 LAB-7 之前照旧走 `actingctl install`**（`contracts/install-cli.md` 923–926；手册第 9 节）。
- **标准包的 notices 目录**（#444 NOTICES-1、NOTICES-2、NOTICES-3，登记 §1.334、§1.335、§1.336；`contracts/package-reference.md`「Notices」）：一个包可以带作者要求随包发出的文字，放在
  压缩包根上的 `notices/` 里，不列进 `bundle.json`，不进任何包的哈希、目录版本或组合代；安装时整个目录作为一个条目放进游戏根，Runtime 的读者一个都不打开它。规则只写在契约 crate 一处
  （`crates/actingcommand-contract/src/standard_pack_notices.rs` 44–62 常数，184–381 各条规则），放包、打包、检查都调它：

  | 项 | 规则 | 违反时的 `standard_pack_problem` |
  |---|---|---|
  | 目录 | 压缩包根（或游戏根）上的 `notices`，只认全小写 | — |
  | 深度 | 目录下最多 2 段 | `notice_depth`（`limit_count`、`count`） |
  | 名字 | 每段 1 到 64 字节，只用 `[A-Za-z0-9._-]`，首字节是字母或数字，不是设备名，不以点结尾；两条路径不能只差大小写，文件不能和另一条路径的目录同名，两个目录名不能只差大小写 | `notice_name` |
  | 后缀 | 文件名里没有点，或最后一个点后面是 `md`、`txt`、`tsv`、`csv`（不分大小写） | `notice_name` |
  | 个数 | 最多 64 个文件 | `notice_count`（`limit_count`、`count`） |
  | 单个大小 | 1 到 2,097,152 字节 | `notice_bytes`（`limit_bytes`、`actual_bytes`） |
  | 合计 | 最多 8,388,608 字节 | `notice_total_bytes`（`limit_bytes`、`actual_bytes`） |
  | 文本 | 严格 UTF-8，不许 U+0000–U+0008、U+000B、U+000E–U+001F、U+007F；TAB、LF、FF、CR 和任何行尾都收，开头的字节序标记也收；只判不改 | `notice_not_text` |
  | 包名 | 带这个目录的包里没有叫 `notices`（任何大小写）的任务包 | `notice_name`（带 `package_id`） |
  | 声明 | 带这个目录的包声明的 `package` 上限不小于 10 | `notice_undeclared` |

  - **违反的回答**：`standard_pack_root_invalid`，`path` 是那个文件（个数和合计时是目录本身），`json_pointer` 为空，有上限的带上限和实际值；放包时它是 `pack_invalid` 的原因。
  - **救急模块的 `place`**（`crates/lab-rescue/src/install/place.rs`）：先核 `package` 这条边、再看布局（528–547），所以带了更新版本东西的包是被「版本不够」拒，不是被「不认识的成员」拒；
    唯一变了的拒绝是同时过不了边、布局也不认的包，从 `pack_invalid` 变成 `interface_edge`。布局多认 `notices/<路径>`（556–563），全部规则在计划阶段、挂暂停之前、写任何东西之前核，每个文件
    最多读「上限加一个字节」，压缩包谎报大小也拦得住（465–499）。整个目录是一个条目，种类 `notices`，**排在所有条目最前**；状态有 `new`、`replace`、`unchanged`，游戏根里有这个目录而后来的包不带时
    是 `retire`，挪到一边留着（`notices.old-<毫秒>`），包里正好有叫 `notices` 的任务包时不挪（656–679）。撤销按种类和摘要反做，什么都不删。不带这个目录的包放到没有这个目录的根上，计划、
    计划号、步骤和流水和以前完全一样。
  - **Tools**：`actinglab package bundle --notices <目录>` 在创建任何东西之前核遍目录（链接照包加载器的拒法拒，路径不是目录或目录里没有文件照 `--loading` 给错的拒法拒），核过才逐字节放进
    `<out>/notices/`，不写 `interfaces.json`；不带参数时输出和以前逐字节相同（`apps/actinglab/src/package_build.rs` 37–38、117）。`standard-pack check` 遇到游戏根下的 `notices` 目录照
    同一套规则核，回答多 `notices {files, bytes}`（不回显文件名和内容）；没有这个目录的根回答逐字节不变（`crates/resource-tooling/src/notices_directory.rs` 101–118，`api.rs` 163–179）。
    `resource verify` 的 `bundle` 步：仓库根有 `notices/` 时当 `--notices` 传下去，`interfaces.json` 写 `package` [10, 10]，装出来的副本也放这个目录，并要求检查数出来的和压缩包里的一致；
    没有时每个字节和以前一样（`apps/actinglab/src/resource_verify/bundle.rs` 120–145）。命令行和 Lab 的 MCP 服务器都有：操作表的 `package bundle` 一行多了参数 `notices`。
  - **`package` [3, 10]，谁声明 10**：Runtime 声明 [3, 10]（`distribution/windows/component-interfaces.json` 9）；只有带这个目录的包在自己的 `interfaces.json` 里声明 [10, 10]，那是打包脚本写的
    （资源仓的脚本还没学，`resource verify` 的镜像已经会写）；`package bundle` 不写 `interfaces.json`；不带这个目录的包保留原来的修订，压缩包、`SHA256SUMS`、计划和流水都不变。
  - **本版之前的程序**：本版之前的 `place` 在布局检查就拒带 `notices/` 的压缩包（`pack_invalid`，点名按名字排第一个的 `notices/` 成员；v0.12.57 的 `place.rs` 417–441 在边 443–460 之前），
    在挂暂停之前，什么都没暂停、没写；游戏根里已经有 `notices\` 时，它照常放不带目录的包、不碰这个目录。本版之前的 Runtime 和 `standard-pack check` 把有 `notices\` 的根当成没有它来读。
    本版之前的安装工具读得了这份 `pack_place` 流水，但解不了里面种类是 `notices` 的那一条（v0.12.57 的 `place.rs` 101–111 只有三种）：撤销时报错，核对时跳过它。
  - **总览的放包工具和资源仓的打包脚本还不认这个目录：改好之前不发布带这个目录的包。**
- **运行恢复分组不再把计划内的停下当失败**（#429 RG-STOP，登记 §1.337；`contracts/global-ledger-query.md` 里 `run_recovery` 那一段）。
  - **原来显示的**：一次被请求停下的调度运行，账本视图的运行恢复分组把停下自己的三条记录（运行的收尾 `task.failed`、调度驱动的 `runtime.failed`、`policy.execution_recorded` 的 `stopped`
    结算，都带 `contained_task_cancelled`，这个码的类别是 error）列成「失败、未见恢复」，缺口 `missing_relation`，状态 `unknown`。
  - **规则**：推分组时，结果码的阶梯分类是 `planned_stop` 的记录不算失败、不列进证据、不引出缺口，除非某个失败种类认得它（`input.failed`、`recognition.failed`、入口恢复失败）
    （`crates/ledger/src/global/projection/views.rs` 309–319、419–440）。分类从程序自带的码目录里读，规则不点名码。**这一类现在有 18 个码，不止 #1665 当初说的两个（登记册和契约的那句已随本版改准）**：error 的
    `contained_task_cancelled`、`contained_task_paused`、`blackout_run_stopped`、`capture_pressure_pause_stopped`、`lab_arrival_shutdown`、`recovery_ladder_shutdown_requested`、
    `recovery_ladder_drain_requested`，warning 的 `contained_task_feature_locked`、`contained_task_entry_busy`、`recovery_ladder_interrupted`、`recovery_ladder_hold_exhausted`、
    `device_thermal_hold`、`device_battery_low`、`device_in_use`，另有 4 个 info 码本来就不算；挂在一次运行上、带这些码的记录都不再进它的分组。
  - **照旧的**：这些记录自己的级别不变，在错误视图里照旧是错误行；输入失败、识别失败、入口恢复失败和别的 warning 以上的记录照旧算失败（停下时复位失败的记录、隔离的记录带的是它们自己的码）。
    只停过或只暂停过的运行就没有分组了。什么都不落盘：Runtime 的读页、actingledger 的 `views` 和离线快照走同一个函数，控制台读到的页跟着 Runtime 变。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`、`instance add`，也不经 Lab 的 MCP 服务器；
  救急模块的 `place` 不用在生产根上（所以生产上不会出现带 `notices` 条目的流水）。
- **生产上**：自 2026-11-11 起有一台实例在试跑调度，其余实例暂停着；部署本身不改变派单。

配置：按代码、`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md` 核过，**升了的正好是 `runtime-client` 和 `package` 两项，别的没动**。
`component-interfaces.json`（4–10）：`actingd-config` [5, 29]、`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 8] 没变，`runtime-client` [3, 7] → [3, 8]（8，LAB-6S1a）、
`package` [3, 9] → [3, 10]（9，NOTICES-2），`runtime-outcomes` [1, 1] 没变；架构测试 `tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120`（148–154）跟着钉。

- `contracts/component-interfaces.md`：新加 `runtime-client` 修订 8 一行（149）和 `package` 修订 10 一行（159）；「Planned: Workflow #434」一节（267 起）把 LAB-0 预留、后来被别的片占掉的号改成
  现在的 8（`runtime-client`：LAB-6S1a 已经打开其中一部分，其余归 LAB-6S3、S4a、S4b）、9（`ledger`，LAB-6S5）、30（`actingd-config`，LAB-6S4b）；9 和 30 都还没生效。`actingd-config` 29、`ledger` 8 的行没动；`install-selection`、`install-control`、
  `player-hold`、`tools-layout` 不变。两项升级都不是单向的：账本、覆盖行、录制、安装流水没有一样新形状，`ledger` 不动。
- 配置键：没有新增，也没有去掉。`distribution/windows/actingd.defaults.json` 自 v0.12.56 一字没改；`apps/actingd` 和 `crates/runtime-config` 自 v0.12.57 没改；`contracts/runtime-config.md`
  只在「Console tiers」一节加了一句 `ui` 通道照档位判（1963–1966）。`config.subsystems` 不变（`apps/actingd/src/config/manifest.rs`、`crates/policy/src/compose.rs` 没改）。
- 结果码：目录仍是 3924 个码、1751 个位置、31 个分片、642 个键、227 个词表；在用的 3906 → 3896，状态为 `retired` 的 18 → 28（上面那十个），审阅定稿的仍是 172 个
  （`invalid_install_transition_origin` 是定稿的，现在是定稿且 `retired`）；没有新码、新位置、新键、新分片。词表 `standard_pack_problem` 多 7 个词（`notice_count`、`notice_bytes`、
  `notice_total_bytes`、`notice_depth`、`notice_name`、`notice_not_text`、`notice_undeclared`，词从 1665 个变成 1672 个），说明也改了。改了的：`channel_request_refused` 多可选值
  `config_key`，三段文字改写成现在生效的样子；`standard_pack_root_invalid` 多四个可选值（`limit_count`、`count`、`limit_bytes`、`actual_bytes`，都是现成的键），描述补了 notices 的一段；
  `invalid_bind_channel` 的三段文字加了发起者的拒法；`list_reading_key_reserved` 的描述、`mcp_operation_unsupported` 的常见原因照实改写。在用码里两段文字都写了的从 712 个变成 709 个
  （三个有文字的码 `retired` 了）。本版的发布文件 `contracts/outcome-codes/released/0.12.58.json` 是那十个 `retired` 的码，没有位置。索引照旧由脚本生成，仍是 32 个文件，重生成没有变化。
- Tools 包：文件清单 `$toolFiles`（`.github/workflows/windows-remote-build.yml` 670–678）自 v0.12.57 没改，布局仍是 `platform-tools-v3`（771）：七个程序加 `platform-tools/` 下的五个文件。
  Runtime 包的文件不变。
- 程序文件：`Cargo.toml`、`Cargo.lock` 没改，没有新的工作区成员和依赖；`.github/` 只改了 `commit-identity-guard.yml`（#1663）；`ci/` 只有 `ci/retired-gates.toml` 多了 LAB-6S1b 退出门表的条目
  （K6 整行、删掉的两条测试、改名的四条），不稳定表照旧是 FLK-08 和 FLK-10；`ratchet/`、`apps/actingd`、`apps/actingwatch`、`apps/mcp-entry`、`apps/mcp-entry-g2`、`apps/ledger-forensics`
  没改；`apps/actingctl` 只改了 MCP 工具 `ac_ladder_drill` 的说明（不再列 `invalid_emulator_control_origin`）和测试假件，今天的 MCP 服务器仍是 41 个工具。

升级与退回（部署由当时装着的那一版的安装工具做。安装工具的 `plan.rs`、`update.rs`、`deploy.rs`、`postcheck.rs`、`programs.rs`、`undo.rs`、`release.rs`、`manifest.rs` 自 v0.12.57 没改，
所以 v0.12.57 一节写「本版」的行号对本版的安装工具都对；本版的安装工具只改了 `place.rs`（上面的 notices）、`host.rs`（分离启动的程序和变量）、`rescue.rs` 和测试夹具。v0.12.56 的安装工具
就是 v0.12.55 的那份代码，v0.12.54 的也是：`plan.rs` 在 v0.12.54 和 v0.12.56 之间没改，下面写「v0.12.56 的」行号对 v0.12.54 的安装工具也对）：

- **v0.12.56 的安装工具装本版：能装，不用任何标志。** 安装工具的声明是它自己构建里的 `component-interfaces.json`（`host.rs` 23–25）：v0.12.56 的是 `ledger` [3, 8]、`runtime-client` [3, 7]、
  `package` [3, 9]。计划的边逐条（v0.12.56 的 `plan.rs`）：
  - `ledger` `previous_max_in_next`（288–294）：8 在 [3, 8] 里，过；`one_way`（295–301）：8 不大于 8，不触发；`ledger_verify` 是 false（915）。
  - `runtime-outcomes` `next_max_in_installer`（302–314）：过。`install-control` 三方 [0, 1]，过。**`runtime-client` 的 `installer_meets_previous`、`installer_meets_next`（315–331）
    只看区间有没有交集（`meet`，241–243）**：安装工具的 [3, 7] 和 Rp [3, 7]、R\* [3, 8] 都相交，过。
  - `package` `bundle_max_in_next`（332–355）：比的是安装登记里每个包的范围，登记里一律没有（`crates/lab-rescue/src/install/adopt.rs` 134 写 `None`，别处不写），所以每个游戏一条
    `pack_declaration_unknown` 警告，和装 v0.12.56、v0.12.57 时一样，不挡。`speaks` `runtime_equals_tools`（756–763）：两半声明相同，过。
  - `actingd-config` `installer_knows_keys`（359–376）：本版没有新键，过。`next_check_config`（902–914）：本版的 check-config（`apps/actingd` 自 v0.12.57 没改）收 v0.12.56 的文件，过；SBUD-1
    拒的包在这里是一条 `pack_admission_suspended` 注记、不算失败（见 v0.12.57 一节）。
  - **要带的标志：没有**；工具包要是照 v0.12.56 那次还带着 `--accept-ledger-one-way` 也能过，但计划号会跟着变、日志的 `acknowledged` 会记下它，去掉。`defaults_diff`（574–598）是空的
    `[]`，部署后检查的 `new_keys`（`postcheck.rs` 494–526，`added_keys` 593–601）没有要核的键，过。
  - **跳过 v0.12.57 有什么不同：没有要多付的。** v0.12.57 的两个 Tools 文件在这次计划里是 `new`（v0.12.56 的 `plan.rs` 815–819，`notice.added` 2），和 v0.12.57 一节写的从 v0.12.56 装 v0.12.57
    一模一样：部署前不存在、不会挡也不会被挡，没有注册指向它们；下一次部署是本版的安装工具做的，正在跑的 `actinglab-mcp.exe` 改名让开（本版 `plan.rs` 37、41–44、953、966–967）。SBUD-1 的准入
    检查（`crates/execution-kernel` 自 v0.12.57 没改）随这次部署到：部署前查包、首次启动时怎样、放包的次序都照 v0.12.57 一节；本版的 `standard-pack check` 对没有 `notices` 的根答法和
    v0.12.57 的相同。IDLE-3 也随这次到，临时设成 `none` 的 `idle.action` 在本版部署以后撤回。唯一不同的是退回的去处：这样装上的，能退回的是 v0.12.56，不是 v0.12.57（见下）。
- **v0.12.57 的安装工具装本版**（先装了 v0.12.57 的那条路）：能装，不用任何标志。边（本版和 v0.12.57 相同的 `plan.rs`）：`ledger` 两边 [3, 8]，`one_way` 不触发（297–310），`ledger_verify`
  是 false（924）；`runtime-client` 安装工具 [3, 7] 和 Rp [3, 7]、R\* [3, 8] 都相交（324–340）；`package` 照旧是警告（341–364）；`installer_knows_keys`（369–385）过。`defaults_diff`（583 起）
  是空的，`new_keys` 没有要核的键。Tools 文件两边清单相同，没有 `new`，正在跑的 `actinglab-mcp.exe` 改名让开、不挡。能退回的是 v0.12.57。
- **v0.12.54 的安装工具直接装本版：能装，和从 v0.12.54 装 v0.12.56 一样要带 `--accept-ledger-one-way`**（预览、`install update plan`、`install update apply` 三步都带；不带时 `ledger` 的
  `one_way` 不过，以 `interface_edge` 拒，`plan.rs` 295–301、1030–1041），计划多一步 `ledger_verify`（915）。v0.12.54 声明 `runtime-client` [3, 6]、`package` [3, 8]：`runtime-client` 的
  [3, 6] 和 Rp [3, 6]、R\* [3, 8] 都相交，过；`package` 照旧是警告。`defaults_diff` 是那三行（`recovery_ladder.profiles.without_emulator_restart.rungs`、`scheduling.task_held.after_ms`、
  `scheduling.task_held.enabled`；默认值文件自 v0.12.56 没改），部署后检查的 `new_keys` 核这三个键（两个压着键的来源是 `default`；档的那一行进 `unseen`，不算不过）；`installer_knows_keys` 不认识 `scheduling.task_held`，文件里不要提前写它；
  两个新的 Tools 文件是 `new`。**比经过 v0.12.56 差的地方**：装上并跑过以后同一个状态根上没有退回的路（见下），部署后检查不过时的自动回退也走不了；v0.12.55 到本版四个版本的改动
  （单向的账本、两个条目一台设备被拒、压着记录、补救表、SBUD-1、IDLE-3、本版这几处）在同一次部署里一起到生产；控制台要先换成 UI v0.12.16。没有让它装不上的东西。
- **退回（按安装工具的边和回退门核过）**：
  - **能退回的只有这次部署替换掉的那一版。** 回退门第 2 部分 `ledger_readability` 只对被替换的那一版放行（`update.rs` 400–437，`live_operation` 181–204）。从 v0.12.56 装上本版的，退回
    v0.12.56 能过，退到 v0.12.57 以 `rollback_gate_failed`（`ledger_readability`，`previous` 是 v0.12.56）拒（那个根上也没有 v0.12.57 的保留构建，除非另外 stage 过）；从 v0.12.57 装上本版的，退回 v0.12.57
    能过，直接退到 v0.12.56 同样被拒。
  - 退回 v0.12.56 或 v0.12.57 时边都过：`ledger` 8 在 [3, 8] 里、`one_way` 不触发；`runtime-client` 在线的本版安装工具 [3, 8] 和两边都相交；`package` 是警告；`installer_knows_keys`
    用的是本版的键表。第 4 部分 `config`：目标的 check-config 读现行的文件（`update.rs` 490–503），本版没加成员，过。部署的自动回退（`deploy.rs` 758–854，这一段 v0.12.56 和本版行号相同，
    813 不带标志）退向被替换的那一版，同样能过。
  - **更早的构建对本版写下的东西**：带 `channel` `ui` 的 `command.rejected`：命令记录的 `channel` 成员 U-2 起就有（v0.12.57 的 `crates/actingcommand-contract/src/event/payload.rs`
    6104–6123），照读。存着 `notices\` 的游戏根：只有本版的救急 `place` 会放出来（生产上不用它），更早的 Runtime 只按名字读游戏根里的文件、只枚举包根下的游戏目录（v0.12.57 的
    `crates/runtime-host/src/host/standard_pack.rs` 333–346），当它不存在；更早的 `place` 不碰它。带 `notices` 条目的 `pack_place` 流水：更早的安装工具解不了这个种类，撤销不了那次放置，要撤先用
    本版撤，或者留着那个没人读的目录。**别的都没有**：LAB-6S1a、LAB-6S1b、LAB-6S3、RG-STOP 都不落盘，LAB-6S0 的流水记的 `Lab-MCP` 是更早的构建也认的六个词之一。退回以后，来源门回来
    （Lab 的 `pause` 那些又被拒），`ui` 通道又只标记、不拒，运行恢复分组回到原样。
  - **从 v0.12.54 直接装上本版的**：和 v0.12.56、v0.12.57 两节一样，安装工具哪一版都不让退回：退到 v0.12.54 或更早，`ledger` 的边不过（8 ∉ [3, 7]）；退到 v0.12.55、v0.12.56 或 v0.12.57，
    边都过，但回退门第 2 部分只对被替换的 v0.12.54 放行，以 `rollback_gate_failed` 拒；部署的自动回退（813 不带标志）和起过新 Runtime 的 `install undo` 同样被拒。出了问题剩下的是：配置的问题，
    改好文件再起；程序的问题，往前修；或者换一个全新的状态根（之前的账本、事实、运行中设的指令、封顶回合和空闲状态都留在旧根上）。
- 配对：
  - **v0.12.54、v0.12.56、v0.12.57 写的配置文件**：本版照收，不用改。本版没有新成员、新键，也没有新的配置拒收（`apps/actingd`、`crates/runtime-config` 自 v0.12.57 没改）；v0.12.54 写的文件只有
    v0.12.56 起就有的那一个例外：两个条目没编成一组、却指向同一台设备，被拒。
  - **已装的包**：本版没有改准入（`crates/execution-kernel`、`crates/pack-containment`、`crates/policy` 自 v0.12.57 没改；SBUD-1 是 v0.12.57 的）。本版的 `standard-pack check` 对今天的根
    （都没有 `notices`）答法和 v0.12.57 的逐字节相同，回答里没有 `notices` 这一项。
  - **控制台 UI**（最新发版 UI v0.12.16 是 UI 的 `6c2e30f2`；UI main 现在是 `02fc7a36`，之后加了按任务包的开关，没有动控制台的声明和 Runtime 钉点）：它声明 `actingd-config` [5, 28]、
    `ledger` [3, 8]、`runtime-client` [3, 7]、`runtime-outcomes` [1, 1]（`crates/acui-setup/component-interfaces.json`）。和本版配：`ledger` 8 在 [3, 8] 里；`runtime-client` 它的安装器取两边
    共有的最高修订 7（`crates/acui-setup/src/interfaces.rs` 650 `negotiate`），控制台运行时不比范围；`actingd-config` 三方共有 28；`runtime-outcomes` 相同。它钉的 Runtime crate 在
    `c0f75c7e`（v0.12.55 的发版提交，`Cargo.toml` 39–44）：本版给 `ui` 连接的 `ChannelBound` 和回执都不带 `initiator`（`ui` 上声明词就被拒，回执的这个成员没有就不写，`runtime.rs` 5718–5722），
    别的回答没有新成员，所以它照读；十个 `retired` 的码它一个都不点名（v0.12.16 和 main 都搜过）。它发的请求和键对清单和档位的核对见上面 LAB-6S3 一条：一样都不被拒。
  - **部署工具包的命令行**：它跑的是在线安装工具的 `actingctl install`：`release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop --hold --known-pause …`、
    `status --root [--operation]`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause … --start --detach`、`release-hold --root --reason`，
    和新版 actinglab 的 `--json standard-pack check --root <游戏根>`。回答部署的是在线的 v0.12.56（或 v0.12.57）的安装工具，代码就是上面核过的那份，**没有一条被拒**。和装 v0.12.57 相比
    答法变了的只有内容：计划里目标的 `speaks` 是 `runtime-client` [3, 8]、`package` [3, 10]，`installer_meets_next` 和 `bundle_max_in_next` 两行的 R\* 方是这两个范围，结果照旧是过和警告；
    部署以后用新构建的安装工具跑的 `install status` 和 v0.12.57 的同一份代码。`standard-pack check` 对今天的根答法和 v0.12.57 的相同。**工具包要跟着改的**：目标提交；从 v0.12.56 装时三步都
    去掉 `--accept-ledger-one-way`；预期改成没有 `ledger_verify`、`defaults_diff` 为空、`new_keys` 为空（从 v0.12.56 装时另有两个 `new` 文件）；给热放置工具包写构建记录的那一步，核新包清单的
    `actingd-config`、`package`、`ledger` 现在是 [5, 29]、**[3, 10]**、[3, 8]，记成 `5,29`、`3,10`、`3,8`（照旧的 `3,9` 会让那一步和后面的重钉都不过）。
  - **热放置工具包的命令行，本版都照旧收、答法和 v0.12.57 相同**：actinglab 两行，`--json standard-pack check --root <游戏根>` 和 `--json standard-pack compose --root <包根>
    --instance <别名>=<游戏>/<区服> … --out <新目录>`；actingctl 的 `status`、`status --config`、`pause --instance <别名> --reason <理由> --drain-timeout-ms <毫秒>`、`resume --instance <别名>`、
    `schedule --instance <别名>`、`overview --root <根> --instance <别名> --sections catalog`（都带 `--state-root`）；actingledger 的 `events --after … --through … --limit …`。actingctl 的命令行
    代码没改，它绑 `cli`、用 `(Cli, Cli)`，暂停和恢复本来就收，LAB-6S3 不判 `cli`；`apps/ledger-forensics` 没改，RG-STOP 只动视图页的分组，`events` 不受影响；`standard-pack check` 只对有
    `notices` 的根多一项。**没有一条被拒，也没有一条换了答法的形状。热放置工具包要学的**：重钉时比的构建记录里 `package` 是 `3,10`；等包里开始带 `notices/` 以后，`standard-pack check` 会多
    `notices {files, bytes}`、会拒违反规则的目录；它自己的放包还不认这个目录（登记 §1.335 的表），学会之前不发带这个目录的包。
  - **今天的 MCP 服务器（`actingctl mcp-serve`，现在注册着的那个）**：每个工具答法照旧。写目标和补救用的是代理调度器的信封 `(Agent, Adapter)`（`apps/actingctl/src/mcp/operator.rs` 561–564，
    `remedy.rs` 60–61），这一直是收的；它绑 `mcp`，LAB-6S3 不判。看得到的差别只有：`ac_ladder_drill` 的说明不再列 `invalid_emulator_control_origin`；`ac_outcome_code` 把那十个码显示为
    `retired`、新的文字和值；读页的运行恢复分组不再列停下的记录（RG-STOP）。
  - **Lab 的 MCP 服务器（`actinglab-mcp`，没有注册指向它）**：工具清单不变；答法变了的是 `ac_deploy`、`ac_rollback`（服务了，见上），`ac_targets_set` 和发布不带来源例外键族的 `ac_facts_publish`
    （以前 `invalid_agent_dispatcher_origin`，现在收下），`ac_package_bundle` 多参数 `notices`，`ac_standard_pack_check` 对有目录的根多 `notices`，说明文字改了两句。

运行上的变化：

- 从 v0.12.56 部署：不用标志，没有 `ledger_verify`，`defaults_diff` 为空，能退回 v0.12.56；跳过 v0.12.57 不多付什么，部署前照 v0.12.57 一节用本版的 `standard-pack check` 查遍包根。
- Runtime 不再按自称的来源拒请求：Lab 的 `pause`、`resume`、`shutdown`、`emu`、`drill`、`facts publish`、`targets set` 这些现在能用，十个来源码 `retired`；`evidence read` 照旧被拒。
- `ui` 通道只收控制台清单上的 30 种请求和档位里能改的 11 个键，别的以 `channel_request_refused` 拒；装着的控制台 v0.12.16 一样都不发，别的通道照旧。
- 声明发起者的客户端（LAB-6S2）应当部署在本版之上（更早的 Runtime 会丢掉它的绑定，它退回不绑定、照旧工作）；Lab 的 MCP 服务器能部署和回滚了，但生产到 LAB-7 前照旧用 `actingctl install`。
- 包可以带 `notices/`，`package` 升到 [3, 10]；放包工具和资源仓的打包脚本学会之前不发布这种包。停下或暂停的运行不再出现在运行恢复分组里。

本版合并的 PR：

- #1660 NOTICES-1（#444，登记 §1.334）：契约 crate 的新模块 `standard_pack_notices`、七个新词、`standard_pack_root_invalid` 的四个可选值、`package-reference.md` 的「Notices」；契约门行
  `#444 NOTICES-1` 的 4 个测试，没有改现有测试；这一片没有程序调它。
- #1661 LAB-6S0（#434，登记 §1.287）：标记变量和分离启动的程序，`actinglab-mcp` 服务 `deploy`、`rollback`，手册、说明文字、契约、操作表备注；apps 门行 `#434 LAB-6S0` 的 3 个测试，改造了 4 处
  原来钉着「这两行被拒」的测试。
- #1663（#446）：`.github/workflows/commit-identity-guard.yml` 先把 `git log` 读进变量，读不到提交就失败，最后一行打印核了几个提交；只动 CI，没有测试。
- #1662 LAB-6S1a（#434，登记 §1.287）：`Initiator`、`BindChannel`/`ChannelBound`/回执的 `initiator`、主机的连接表、客户端的 `with_initiator` 和回显检查，`runtime-client` [3, 8]；主机 3 个、
  客户端 3 个、契约往返 3 个和 lab-ops 词表一致 1 个新测试；改造的现有测试只是钉声明的两处和给结构体补上新成员，断言没变。
- #1664 NOTICES-2（#444，登记 §1.335）：`place` 先核边、认 `notices/`、条目种类 `notices`，集合规则多一条目录名大小写，`package` [3, 10]；apps 门行 `#444 NOTICES-2` 的 5 个测试，测试夹具多一个
  安装工具 `package` 范围的字段，改造了 2 个测试（集合规则的新边界、架构钉 [3, 10]）。
- #1665 RG-STOP（#429，登记 §1.337）：`run_recovery` 的失败判断和两个小函数；账本门行 `#429 RG-STOP` 的 5 个测试，每一页还和内存索引的同一页对照；没有改现有测试。
- #1666 NOTICES-3（#444，登记 §1.336）：`package bundle --notices`、`standard-pack check` 的 `notices`、`resource verify` 的 `bundle` 步，常数 `NOTICES_PACKAGE_REVISION` 挪进契约 crate，
  操作表 `package bundle` 多参数；exec 门行 `#444 NOTICES-3` 的 7 个测试（resource-tooling 6 个、actinglab 1 个），2 个现有测试只给结构体补上新成员。
- #1667 LAB-6S1b（#434，登记 §1.287）：`validate()` 的来源门删掉，十个码 `retired`，操作表 `evidence read` 的 `requests` 列出身份卡，十几份契约改了文字；契约门行和主机门行 `#434 LAB-6S1b`
  的新测试，架构守卫改成校验函数里读信封的只剩 `valid_client_origin`；`ci/retired-gates.toml` 记下 K6 整行、删掉的 2 条测试和改名的 4 条，几条测试只改了来源那一半，CI 第一轮查出的
  2 条旧主机测试改成伪造客户端之外的信封。
- #1668 LAB-6S3（#434，登记 §1.287）：`ui_allows`、主机的 `host::ui_gate`、`CommandPayloadDraft::rejected_on_channel`，`channel_request_refused` 多 `config_key`，结果码守卫给 `ui_gate::kind_location`
  一条 G5 豁免，操作表 `StopInstanceRun` 的 `console_today` 改成 true；主机门行和契约门行 `#434 LAB-6S3` 共 6 个新测试，6 条旧测试和 #1667 的 1 条改成先绑 `cli`，断言没变。

## v0.12.57

本版是 v0.12.56 之后 main 上的 19 个合并：17 个 PR 和两趟并车，并车又带进 5 个 PR（#1640 带 #1636、#1638、#1639，#1643 带 #1641、#1642）。按单子分：#434 LAB-5G 的新 MCP 服务器一整组（LAB-5G1b #1636、LAB-5G2a #1639、
LAB-5G5a #1637、LAB-5G6 #1641、LAB-5G5b #1642、LAB-5G2b #1644 和 #1645、LAB-5G3 #1646 和 #1647、LAB-5G4b #1651 和 #1652、LAB-5G4c #1650、LAB-5G7 #1658）和 LAB-5B1 三片（#1654、#1655、
#1657），#437 MAPP-1b（#1635），#447 IDLE-3（#1648），#448 SBUD-1（#1649），#428 LAB-OCR-DL（#1638），#446 的两片（#1633 只动 CI，#1653 只换掉本机路径）。先说部署：**生产上跑的是
v0.12.54，v0.12.55 和 v0.12.56 发布了、都没有部署**；计划是先从 v0.12.54 装 v0.12.56（跨过 v0.12.55 的单向一步，见 v0.12.56 一节），过几天再从 v0.12.56 装本版。从 v0.12.56 装本版：
**没有一个接口修订移动，不用任何标志，计划里没有 `ledger_verify`，默认值文件不变（`defaults_diff` 是空的），装上以后能退回 v0.12.56**。没有新的配置键，`config.subsystems` 不变；
Tools 包多两个文件（布局仍是 `platform-tools-v3`）。**部署前要做的一件事：用本版的 `standard-pack check` 把每个已装的包根查一遍，查出被拒的包先放改好的那一版**（SBUD-1，下面第一条）。
运行上要知道的是这几处：

- **声明的采样需要放不进时限的包，装载时就拒**（#448 SBUD-1，登记 §1.333）。
  - **原来的毛病**（#448）：一个已发布的包，选择步的候选布局加了用 OCR 读整数的特征（四行同时读、每个目标的 `timeout_ms` 是 5000、没有共识），步骤没写时限（默认 5000 毫秒）。
    `standard-pack check` 和 Runtime 的准入都放行了，跑到那一步每次都失败：`candidate_sample_budget_insufficient`，`required_ms` 20000、`remaining_ms` 4997，跑之前什么都看不出来。
    原因是准入只比 `sampling_budget_ms`，它只在声明了共识时才有答数。
  - **现在准入比的是运行时那条规则本身**：哪些布局要采样并检查（有共识，或有读身份、读整数的特征）、需要多少（布局声明的最坏识别时间加等待，减去帧间等待）收进一个函数
    `selection::candidate_provider_need_ms`（`crates/execution-kernel/src/contained_task/selection.rs` 726），运行时的检查（1278）和准入都调它。准入在 `prepare_bundle` 的循环里
    （`contained_task.rs` 3367–3408）拿这个需要量比这一步的上限：步骤和任务时限里小的那个；扫描选择和 `read_list` 用自己的 `max_ms`，再受任务时限限制。声明了目标共识的包里，`read_list`
    每次截图的页识别也按 `max_ms` 比。超了就拒：`recognition_sample_budget_invalid`（`required_ms`、`limit_ms`、`pack_operation_id`），正好相等放行（但实机拍完第一帧以后剩下的时间更少，
    正好相等的包实机仍会失败，作者要留余量）。没有新码、新键，`package` 不升。
  - **拒的是整个包**：准入是一个任务包（一个 `package_id`）一起过的，包里哪一步超了，整个包被拒，它绑定的所有任务都跟着停，不只是用到那一步的任务。
  - **操作员看到的**：Runtime 照默认的 `scheduling.pack_admission.suspend_codes`（只有 `recognition_sample_budget_invalid`，`distribution/windows/actingd.defaults.json` 354–358）把这个包挂起，
    码是 `pack_admission_suspended`（`failure_code` 是 `recognition_sample_budget_invalid`，带 `package_id`、`actual_digest`、`runtime_commit`）；离线的 `standard-pack check`、`package check`、
    `package preflight` 以 `package_invalid` 拒，原因里是 `recognition_sample_budget_invalid` 和那三个值。首次启动时具体会怎样，见下面配对里的「已装的包」。
  - **部署前操作员要做的**：本版的 Lab 不必装上就能跑：`install release stage` 以后它在 `<根>\install\builds\<本版提交>\tools\actinglab.exe`（或者解开的 Tools 包里）。对每个已装的
    包根跑 `actinglab --json standard-pack check --root <standard_packs.root>\<游戏目录>`。**干净的回答**：退出码 0，行的 `outcome` 是成功（`lab_operation_completed`），`status` 是 `checked`，`pack_count` 是这个根登记的包数，
    `warnings` 里只有以前就有的警告。**被拒的回答**：退出码非 0，`outcome` 是 `package_invalid`（导航角色的包是 `package_bundle_navigation_package_invalid`，`navigation_problem`
    `admission_failed`），原因里是 `recognition_sample_budget_invalid` 和 `pack_operation_id`、`required_ms`、`limit_ms`（`crates/resource-tooling/src/standard_pack_check.rs` 967–983、
    1053–1059）。检查碰到第一个被拒的包就停，改好以后要再跑一遍，直到每个根都是干净的回答。有被拒的包，就先在 v0.12.56 上照常热放置改好的版本（给它的步骤写上不小于需要量的时限，
    扫描写 `max_ms`，或者减少、缩短识别），再部署。
- **空闲动作只照「算进了上一次运行结束」的计划做**（#447 IDLE-3，登记 §1.332；`contracts/runtime-config.md` 的 `idle` 一节）。
  - **原来的毛病**（#447，生产 v0.12.54）：一台实例上两个串起来的计划任务，第一个结束后不到一秒，空闲动作把应用关了（`instance_idle_action` `close_app`），第二个任务的派单排在冷启动
    后面被拒（`dispatch_instance_held`），它的包从第一个任务结束的那一页起步，第一步就失败。原因：空闲动作只等「上次看到实例在用以后」评估出的计划（对租约的抽样）；租约在结算之前就放了，
    而链上的下一个任务要等上一个的完成记进策略才到期，一次在完成之前读了输入、在放租约之后才评估的计划就过了抽样。
  - **现在**：回主页、关应用、关模拟器这三种动作，只在这样的计划上决定：它的评估读输入时，策略在这台实例上没有开着的派单（从意向到完成或被拒），而且之后没有新开的
    （`crates/runtime-host/src/policy_host.rs` 3181 `idle_dispatches_read`、3205 `idle_runs_counted`；`crates/runtime-host/src/host/idle.rs` 387–424，周期的检查和每秒清扫的检查
    读的是同一把锁下的同一组值）。
    拿到这台的钥匙以后再照同一条规则读一次最新计划，不算数、有到期的任务或读数、或者下一件事已经近于这一档的门槛，就照「中途来了暂停」那样放手：什么都不发，不留记录，下一次检查再定
    （`idle.rs` 1141、1244、1430–1443，`idle_action.rs` 329 `stands_on_key`）。策略不派的活（手动、直跑、Lab、阶梯、宿主自己的运行）照旧用抽样那条；叫醒、`none`、跨启动保留的状态都不变。
  - **没有新的键、码、记录或日志行**。看得出规则在起作用的地方是现有的记录：链上第一个任务的 `policy.dispatch_completed` 和下一个任务的 `policy.dispatch_intent` 之间不再夹着
    `instance_idle_action` `close_app`，下一个任务也不再以 `dispatch_instance_held` 被拒；真该做的空闲动作照旧记 `instance_idle_action`，出现在这台实例最后一次 `policy.dispatch_completed`
    之后的那次评估之后（通常同一秒内）。放手的那一次什么都不留。
  - **临时把 `idle.action` 设成 `none` 的**：部署本版以后可以撤回。这个键运行中能改（`contracts/runtime-config.md` 1157）：用指令设的，`config clear idle.action` 回到文件或默认值
    （默认 `close_app`），或者 `config set idle.action=<值>`（Lab 的 `config set`、MCP `ac_config_set`），下一次空闲检查就按新值；写在配置文件里的，改文件，下次启动生效。v0.12.54 和
    v0.12.56 都还有这个毛病，部署之前不要撤回。
- **新的 MCP 服务器随 Tools 发布，但没有任何注册指向它**（#434 LAB-5G，登记 §1.287 里 LAB-5G1b 到 LAB-5G7 各小节；`contracts/lab-two-entries.md` §8、§9.3）。
  - **发的是什么**：Tools 包多两个程序：Lab 的 MCP 服务器 `actinglab-mcp.exe`，和 MCP 入口的第二代 `actingcommand-mcp-entry-g2.exe`（它每次新起子进程时先找已装构建的
    `<根>\runtime\actinglab-mcp.exe`，没有就找 `<根>\tools\actinglab-mcp.exe`，把客户端的消息转给它，换版以后在两次调用之间自己切过去）。部署把两个文件装进 `<根>\tools\`。服务器给操作表里
    每个提供的行一个工具，Windows 上 143 个（不列创作模块 108 个；别的平台 110 个），没有档位；141 个在服务（核心行 49 个、救急行 31 个在它自己的进程里，其余 61 个交给
    `actinglab serve --stdio --front mcp` 子进程），只有两个拒（下面的例外）。它照今天的服务器那样记来源（身份卡 `actingctl-mcp`、客户端动作），`governance.allowed_clients` 不用改。
  - **没有东西注册它**：今天的服务器 `actingctl mcp-serve`、`mcp-entry\g1\` 里的第一代入口和现有的每一份客户端注册都照旧工作（`apps/actingctl/src`、`apps/mcp-entry` 自 v0.12.56 没改）。
    一个客户端只有在它的主人改了注册以后才会换过去，做法见 `apps/actinglab-mcp/MANUAL.md` 第 11 节「Changing a registration」：二代入口要手放一次（把 `<根>\tools\actingcommand-mcp-entry-g2.exe`
    复制进 `<根>\mcp-entry\g2\`，不能放进 `g1\`；没有安装步骤写 `mcp-entry\` 下面）；`actinglab --json mcp config --client <claude|codex|antigravity|dsh> [--list <模块>]` 只印出要写的片段，
    不读也不写任何客户端的文件（入口还没放时照样印，带警告 `mcp_entry_not_placed`）；拿它替换今天的注册（服务名同是 `actingcommand`），开新会话；`mcp entries` 列两代入口的状态。
    **退回**：把今天的注册写回去（`actingctl mcp-config --client <客户端> --entry --root <根> --tier <档位>` 再印一次），开新会话；**安装退回到 0.12.57 以前之前先这样退回**（第 11.7 节）。
  - **唯一的例外**：经它的部署和回滚还不服务：`ac_deploy`、`ac_rollback` 答 `mcp_operation_unsupported`，什么都不启动（到 LAB-6）。手册第 10 节：Windows 上启动、停止、重启 Runtime、
    替换或恢复配置文件、加实例（`ac_up`、`ac_down`、`ac_restart`、`ac_config_replace`、`ac_config_restore`、`ac_instance_add`）它照命令行一样服务，但做不做由人决定，代理只在人在当次对话里
    要求这一件事时才调用。
  - **从本版的安装工具起**：部署时正在跑的 `actinglab-mcp.exe` 像 `actingctl.exe` 一样改名让开（`<文件>.old-<毫秒>`）、不挡部署，`install status` 把它列在 `mcp_servers_on_old_images`；
    二代入口起的服务器不要求客户端重连（`crates/lab-rescue/src/install/plan.rs` 37、41–44、953、966，同目录 `host.rs` 461–476）。部署是当时装着的那一版的安装工具做的，所以这条规则从本版之后的那次
    部署才起作用：先装上本版，再让客户端换过去（Tools 文件的配对见下面）。
  - `actinglab` 多三个救急行：`mcp config`、`mcp entries`、`mcp plan`（二代入口问的计划）；`actinglab serve --stdio --front mcp` 现在答信封 `{exit_code, req_id, line}`（`req_id` 可无），不带这个选项的每一行不变。
- **一台设备上几个应用：宿主那一半做了，多个成员的组照旧拒收**（#437 MAPP-1b，登记 §1.330；`contracts/device-group.md`）。
  - **宿主现在做的**：一台设备一个工作线程（多个成员的组叫 `actingd-device-<组名>`，独占一台设备的实例照旧叫 `actingd-instance-<别名>`）；占用按整组算（设备的租约、任何成员登记了的运行、
    排在最前的等待），别的成员占着时本成员带 `device_group_waiting` `held` 等；准入的守卫按设备认；暂停一个成员只等它自己的活；别的成员的玩家占用和围栏挡住本成员；多个成员的组里，成员的租约结束后不留会话；
    链上的下一环排在同设备别的成员前面；两种拒绝用新码 `device_group_action_refused`（`alias`、`operation`）：操作员对某个成员的模拟器控制，和别的成员占着设备时对它的无钥观察。
  - **这一版收什么、拒什么**：不写 `device_group` 的条目和一个成员的组照收，行为和以前一样；**多个成员的组在装配时照旧拒**（`device_group_config_invalid`、`several_members_unsupported`；
    `apps/actingd/src/config/device_group.rs` 377–383 没改），两个条目没编成一组却指向同一台设备也照旧拒（v0.12.56 起）。所以本版收的配置走不到这些新代码，`device_group_action_refused`
    不会出现。第一个收多成员组的版本要把 MAPP-1b、MAPP-2（开跑前的切换）和「同一台上先后怎么排」一起带上（`contracts/device-group.md` 634–646）。设备组什么都不落盘。
- **离线评估存下来的画面时可以放宽 OCR 期限**（#428 LAB-OCR-DL，登记 §1.331）：十行（`observe`、`do`、`ensure`、`wait`、`recognize`、`detect-page`、`current-page`、`is-visible`、
  `tap-target`、`navigate`）在 `--scene` 的路径上多一个开关 `--ocr-deadline-max`（行参数 `ocr_deadline_max`）：每个 OCR 请求的期限放到契约上限 60000 毫秒（`crates/lab/src/ocr.rs` 472–496），
  回答多 `ocr_deadline_raised_to_ms: 60000`；和 `--capture` 一起、或者没有 `--scene`，以 `validation_failed`（`conflicting`）拒（`apps/actinglab/src/offline_ocr.rs` 43–56）。
  生产永远不走它：Runtime 不带这段代码，执行内核、识别包和引擎一个文件没改；不带开关时每一行都和以前一样。`replay` 不收它。资源仓的回归脚本在它的 Tools 钉到本版以后才能用。
- **`record draft` 成了 actinglab 的命令**（#434 LAB-5G4c）：`actinglab record draft --record-stop <json>` 或 `--record-stop-export <json>`，就是 MCP `ac_binding_draft` 搬过来的同一份做法，
  命令行和 `serve --stdio` 都能用。今天的 `ac_binding_draft` 没变，调它的人和脚本什么都不用改；以前 actinglab 没有这条命令。
- **LAB-5B1 三片不改行为**（#434 LAB-5B1、LAB-5B1b、LAB-5B1c）：表驱动的每一行读哪些参数、交给安装工具和代理通道哪些标志，都和操作表钉成一致，由测试守着；每条命令行、`serve --stdio`
  的行都逐字节不变（读到表外参数的检查只在测试构建里）。改了东西的只有一处：`config restore` 声明了 `sha`，命令行收 `--sha <sha256>`、`serve --stdio` 收成员 `sha`，保留的文件的 SHA-256
  对不上时先以 `actingctl_usage_invalid`（`--sha`、`unusable`）拒，什么都不改，和 `config replace` 一样。另外一个不是行的 `agent <词>` 现在由救急模块拒，不再交给代理通道去拒。
- **只换本机路径、只动 CI 的两片**：#446 的 #1653 把仓里现行文件中本机的路径换掉了：测试、注释、一处契约示例、七份七月的报告，和 `distribution/windows/INSTALL.md`、
  `scripts/windows-tools/README.md` 的下载示例（示例目录换成中立的 `ActingCommand` 目录，仍在 D 盘，因为下载脚本只收 D 盘上的任务根）。没有哪个工具的行为或默认值变了，也没有脚本因此多要一个
  参数（这一片没改任何脚本）。#1633 只动 CI：提交身份检查的白名单只剩三个账号的 noreply 地址和 GitHub 网页端提交者。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`、`instance add`，也不经新的 MCP 服务器
  （它本来就拒 `ac_deploy`、`ac_rollback`）。
- **生产上**：自 2026-11-11 起有一台实例在试跑调度，其余实例暂停着；部署本身不改变派单。

配置：没有接口修订移动，按代码、`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md` 核过。`component-interfaces.json`（4–10）自 v0.12.56 没改：
`actingd-config` [5, 29]、`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 8]、`runtime-client` [3, 7]、`package` [3, 9]、`runtime-outcomes` [1, 1]；架构测试
`tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120`（141–142）照旧钉 [5, 29]。

- `contracts/component-interfaces.md` 里 `actingd-config` 29（129）、`ledger` 8（141）、`runtime-client` 7（148）、`package` 9（157）几行没动；改的只有 `tools-layout` 一行的说明（161）和
  计划中的布局 5 那一段（291–296）：两个新文件照一代入口在 v0.12.18 的先例加在布局 3 里面，不升，没有读者因此不能用（安装工具和控制台的安装器装清单列的每个文件，资源仓只核整包哈希、
  清单的构建成员和 `actinglab.exe`）。SBUD-1 只是收紧准入，`package` 不升（登记 §1.333 的 “Interfaces”）；MAPP-1b、IDLE-3 什么都不落盘；二代入口的 `entry_protocol` 2 只是它和
  `mcp plan` 之间的约定，状态文件的位置、名字和形状不变。`install-selection`、`install-control`、`player-hold` 不变。
- 配置键：没有新增，也没有去掉。`distribution/windows/actingd.defaults.json` 自 v0.12.56 一字没改；`contracts/runtime-config.md` 只改了文字：`idle` 一节「什么时候动」的句子
  （1184–1257，IDLE-3）和 MAPP-2 那个计划中的键的说明（1747–1752，它跟 MAPP-2 走、取下一个空号）。`apps/actingd/src/config.rs` 只多把每个条目的组名交给宿主（2140–2143），不读新成员。
- `config.subsystems` 不变：`apps/actingd/src/config/manifest.rs` 和 `crates/policy/src/compose.rs` 自 v0.12.56 都没改，没有新的子系统，也没有新的 `requires` 词。
- 结果码：目录从 3917 个码变成 3924 个（在用的 3899 → 3906，状态为 `retired` 的仍是 18 个，定稿的仍是 172 个），位置从 1750 个变成 1751 个，分片从 29 个变成 31 个（新属主 `actinglab-mcp`、
  `mcp-entry-g2`），键仍是 642 个，词表仍是 227 个、词多 3 个。新码 7 个（都待定稿，since 0.12.57，原因和处理办法都写了）：`device_group_action_refused`（runtime-host，error，ladder `none`；
  `alias`、`operation` 必有）、`mcp_operation_unsupported`（actinglab-mcp，error）、`mcp_identity_declaration_refused`（actinglab-mcp，warning）、`mcp_job_started`（actinglab-mcp，info）、
  `mcp_entry_g2_outcome_unknown`、`mcp_entry_g2_child_unavailable`、`mcp_entry_g2_usage_invalid`（mcp-entry-g2，error），ladder 都是 `none`。新位置 1 个：`install_mcp_config`（actingctl）。
  新词 3 个：`argument_problem` 的 `not_absolute`、`install_refusal` 的 `mcp_server_not_in_build`、`install_caveat` 的 `mcp_entry_not_placed`；新域前缀 `mcp_entry_g2_`，前缀 `mcp_` 多给了
  `actinglab-mcp`。改了的：`lab_operation_completed` 从 actinglab 的分片搬到 actingctl 的分片（`actingctl codes`、`ac_outcome_code` 和索引里显示的属主从 `actinglab` 变成 `actingctl`，拼写、
  类别、值和文字都没变）；`binding_requires_unavailable`、`preflight_failed`、`check_config_unavailable` 的描述（前一个和后一个还有常见原因）提到了 `record draft`。没有新停用（`retired`）或新定稿的码，
  没有哪个码的类别、阶梯分类、值、状态或审阅变了；在用码里两段文字都写了的从 705 个变成 712 个。本版的发布文件是空的。索引照旧由脚本生成，现在是 32 个文件。
- Tools 包：布局仍是 `platform-tools-v3`（`.github/workflows/windows-remote-build.yml` 771），文件清单 `$toolFiles`（670–678）多 `actinglab-mcp.exe`、`actingcommand-mcp-entry-g2.exe`，
  一共七个程序加 `platform-tools/` 下的五个文件，每个都是 Tools `BUILD-MANIFEST.json` 的一项；`scripts/windows-tools/Get-ExactBuildArtifact.ps1` 照一代入口的写法，清单声明了哪个就要求哪个，
  以前的包照样通过。Runtime 包的文件不变。
- 程序文件：`Cargo.toml` 多两个工作区成员（`apps/actinglab-mcp`、`apps/mcp-entry-g2`），`Cargo.lock` 只多它们两项，没有新的外部依赖；`.github/` 改了 `ci.yml`（两个新包进 `apps` 作业）、
  `windows-remote-build.yml`（两个程序）和 `commit-identity-guard.yml`（#1633）；`ci/` 一个文件没改（不稳定表照旧是 FLK-08 和 FLK-10，`ci/retired-gates.toml` 没改，没有测试离开门表）；
  `ratchet/`、`apps/ledger-forensics`、`apps/mcp-entry`、`apps/actingctl/src` 都没改，今天的 MCP 服务器仍是 41 个工具。

升级与退回（部署由当时装着的那一版的安装工具做。v0.12.56 的安装工具就是 v0.12.55 的那份代码：`crates/lab-rescue` 在 v0.12.55 和 v0.12.56 之间没改，其中 `plan.rs`、`update.rs`、
`programs.rs`、`postcheck.rs`、`host.rs`、`undo.rs` 自 v0.12.51 没改，`deploy.rs` 自 v0.12.54 没改，所以 v0.12.56 一节核过的行号对 v0.12.54、v0.12.55、v0.12.56 的安装工具都对，下面
写「v0.12.56 的」行号就是那份代码的；本版的安装工具只改了 `plan.rs` 的两个名字清单、`host.rs` 的一个谓词、各命令的标志清单提成常量（内容不变）和三个新的救急行）：

- **v0.12.56 的安装工具装本版：能装，不用任何标志。** 安装工具的声明是它自己构建里的 `component-interfaces.json`（`host.rs` 23–25），v0.12.56 的和本版一模一样。计划的边逐条（v0.12.56 的
  `plan.rs`）：
  - `ledger` `previous_max_in_next`（288–294）：8 在 [3, 8] 里，过；`one_way`（295–301）：8 不大于 8，不触发，不带标志也过。`ledger_verify` 是 false（915），步骤里没有它；
    `notice.rollback_window_bounded` 是 false（203）。
  - `runtime-outcomes` `next_max_in_installer`（302–314）、`install-control` 和 `runtime-client` 的 `installer_meets_previous`、`installer_meets_next`（315–331）：三方相同，都过。
  - `package` `bundle_max_in_next`（332–355）：目标仍是 [3, 9]，和 v0.12.56 上一样；`speaks` `runtime_equals_tools`（756–763）：过。
  - `actingd-config` `installer_knows_keys`（359–376）：本版没有新键，v0.12.56 的键表都认识，过。`next_check_config`（902–914）：本版的 check-config 带 `--install-root` 读现行的文件，
    v0.12.56 收的文件本版照收，过。SBUD-1 在这里看得出来：check-config 默认（`scheduling.pack_admission.check_in_check_config`）把要调度的包（组合模式下是组合出来的过程绑定的包）
    都准入一遍，被拒的包是一条 `pack_admission_suspended` 注记，不算失败
    （`apps/actingd/src/check_config.rs` 257–279，`contracts/actingd-check-config.md` 601–603），计划照常不拒；计划的 `check_config.target.notes` 里会有 `pack_admission_suspended`，
    `check_config.differs` 多 `notes`（v0.12.56 的 `programs.rs` 141–159、251–260）。
  - **要带的标志：没有**。部署工具包要是照 v0.12.56 那次还带着 `--accept-ledger-one-way`，预览、计划、apply 三步都带的话照样能过（那条边本来就过），但计划里
    `inputs.accept_ledger_one_way` 是 true、计划 id 跟着变，日志的 `acknowledged` 会记下它（`deploy.rs` 1720–1724），容易看错，去掉。
  - 计划的 `defaults_diff`（574–598）：两边的默认值文件相同，是空的 `[]`。部署后检查的 `new_keys`（`postcheck.rs` 494–526；`added_keys` 593–601）：`keys` 是空的，结果是过（511–517）。
  - **两个新的 Tools 文件**：`release stage` 照清单逐项核（v0.12.56 的 `release.rs` 496–507：包里的文件和清单要正好一致，两个新文件都在清单里，过；布局名照旧要 `platform-tools-v3`，
    `manifest.rs` 18）。计划把它们列在 `files` 里，动作是 `new`（`plan.rs` 815–819；`notice.added` 是 2，191–195）；apply 跟别的文件一样先放 `.part` 再换上，新文件没有要留的旧文件
    （`update.rs` 539–565），`verify_tree` 核清单列的每个文件（`manifest.rs` 358–383）。正在跑的程序只有动作是 `replace` 的才算（`plan.rs` 951–953），这两个文件部署前在 `<根>\tools\` 下
    还不存在，所以**这次部署它们不会被挡、也不会挡别人**；v0.12.56 的安装工具不认识它们（可改名让开的只有 `actingctl.exe`，`plan.rs` 34、957–964；也不把它算作 MCP 服务器），但这次碰不到。
    **下一次部署**是本版的安装工具做的：没有客户端注册新服务器时，它们是普通的替换；有客户端注册了、`actinglab-mcp.exe` 正在跑时，它改名让开、不挡部署，apply 带
    `mcp_clients_on_previous_build` 提示，二代入口起的那个在两次调用之间自己换版，不用客户端重连（本版 `plan.rs` 37、41–44、953、966–967）。
  - 部署的自动回退（`deploy.rs` 763–854）在部署后检查不过时退回 v0.12.56：边都过，回退门第 2 部分对被替换的那一版放行，能退。
- **v0.12.54 的安装工具直接装本版：能装，和从 v0.12.54 装 v0.12.56 完全一样**，要带 `--accept-ledger-one-way`（预览、`install update plan`、`install update apply` 三步都带，不带时
  `ledger` 的 `one_way` 不过，以 `interface_edge` 拒，`plan.rs` 295–301、1030–1041），计划多一步 `ledger_verify`（915；`update.rs` 528–538），`defaults_diff` 是那三行
  （`recovery_ladder.profiles.without_emulator_restart.rungs`、`scheduling.task_held.after_ms`、`scheduling.task_held.enabled`，本版的默认值文件和 v0.12.56 的相同），`installer_knows_keys`
  不认识 `scheduling.task_held`，文件里不要提前写它；两个新的 Tools 文件和上面一样是 `new`。**比经过 v0.12.56 差的地方**：装上并跑过以后同一个状态根上没有退回的路（见下面）；
  部署后检查不过时的自动回退也走不了（退向 v0.12.54，`ledger` 那条边不过，以 `rollback_gate_failed` 结束，新构建和 hold 都留着）；v0.12.55、v0.12.56 的改动（单向的账本、两个条目一台设备
  被拒、压着记录、补救表）和本版的 SBUD-1 在同一次部署里一起到生产；控制台要先换成 UI v0.12.16。没有让它装不上的东西。做法照 v0.12.56 一节「从 v0.12.54 部署」。
- **退回：从 v0.12.56 装上本版以后，能退回 v0.12.56**（退回由当时装着的本版安装工具做；它的边和回退门的代码和 v0.12.56 的相同：`update.rs` 没改，`plan.rs` 的行号往下移了 9 行）。
  - 边都过：`ledger` 8 在 [3, 8] 里，`one_way` 不触发，别的三方相同；回退门第 2 部分 `ledger_readability` 对被替换的那一版放行（`update.rs` 400–437，`live_operation` 181–204），
    v0.12.56 就是被替换的那一版，它自己的读者读本版写的页；第 4 部分 `config`：v0.12.56 的 check-config 读现行的文件（490–503），本版没加成员，过。
  - **v0.12.56 对本版写下的东西**：账本里没有新的记录形状，本版能新写的码要么只在多成员的组里才会出现（`device_group_action_refused`，本版拒收这种组），要么是现成的形状
    （`pack_admission_suspended` 规划信号是 SR-3 起就有的形状，新服务器记的客户端动作和今天的服务器一样），v0.12.56 照读；MAPP-1b 和 IDLE-3 什么都没落盘，没有要清的。SBUD-1 的挂起只在内存里，
    退回以后 v0.12.56 又会收那种包，它照旧在那一步每次失败；IDLE-3 的毛病也回来。
  - **Tools 目录里多的两个文件**：退回的文件计划只管目标清单列的文件，这两个不在 v0.12.56 的清单里，列在计划的 `extra` 里、原样留着（本版 `plan.rs` 846–856），没有步骤删它们，
    `verify_tree` 只核列出的文件；正在跑的 `actinglab-mcp.exe` 不在目标的文件里，既不让开也不挡。v0.12.56 自己从不启动或读它们。**有客户端已经换到新服务器的，退回之前先退回注册**
    （手册 11.7）：不然二代入口会接着起留在 `<根>\tools\` 的本版服务器，它问的 `mcp plan` v0.12.56 的 actinglab 不认识，计划一直被拒、隔一阵再问；之后再用 v0.12.56 的安装工具部署换掉这个
    文件时，正在跑的它会被当成挡住部署的程序（v0.12.56 的 `plan.rs` 34、957–964）。
  - **退到 v0.12.54 或更早**：本版声明 `ledger` [3, 8]，以 `interface_edge` 拒（8 ∉ [3, 7]），和 v0.12.56 一节说的一样。
- **从 v0.12.54 直接装上本版的**：和 v0.12.56 一节一样，安装工具哪一版都不让退回：退到 v0.12.54 或更早，`ledger` 的边不过；退到 v0.12.55 或 v0.12.56，边都过，但回退门第 2 部分只对被替换的
  v0.12.54 放行，以 `rollback_gate_failed` 拒；部署的自动回退（`deploy.rs` 813 不带标志）和起过新 Runtime 的 `install undo`（本版 `undo.rs` 395）同样被拒。出了问题剩下的是：配置的问题，
  改好文件再起；程序的问题，往前修；或者换一个全新的状态根（之前的账本、事实、运行中设的指令、封顶回合和空闲状态都留在旧根上）。
- 配对：
  - **v0.12.54 或 v0.12.56 写的配置文件**：本版照收，不用改。本版没有新成员、新键，也没有新的配置拒收；v0.12.54 写的文件只有一个例外，是 v0.12.56 起就有的「两个条目没编成一组、却指向同一台
    设备」被拒。
  - **已装的包：SBUD-1 在部署后第一次启动时做什么**（这决定部署和放包的次序）：
    - 启动本身不因此失败，组合代也不受影响：调度器工作台照旧组合、激活组合出来的一代（组合只看 `schedule.json`，准入不在其中，
      `crates/runtime-host/src/host/standard_pack_schedule.rs` 26–44）。
    - 工作台第一次检查之后，后台的准入检查（`scheduling.pack_admission.check_after_start`，默认开）把组合出来的过程绑定的每个包准入一遍（`crates/runtime-host/src/host.rs` 6621–6645，
      `crates/runtime-host/src/host/failure_settlement.rs` 212–237），被拒的包按它的 `package_id` 挂起（`crates/runtime-host/src/pack_admission.rs` 244–269）。在它查到之前，这个包的任务要是
      先到期，那次派单在准备包时就被拒、照常写执行记录，包也从这次起挂起（`failure_settlement.rs` 140–147）。
    - 挂起以后，**每台实例**上绑定这个包的每个任务都在评估里以 `pack_admission_suspended` 推迟，不派单、不准备（`pack_admission.rs` 286–380）；同一个标准包里别的任务包、这台实例别的任务、
      别的实例上别的任务照常调度（`contracts/policy-suspension.md` 620–627）。账本里每台实例第一次推迟时记一条 `pack_admission_suspended` 规划信号（`goal_missed`）；`actingd suspended` 的
      `packs`、`ac_diagnose` 和 `actingctl diagnose` 的包行、`ac_overview` 和 `ac_schedule` 的「没跑」行都看得到。
    - 解除：包的内容哈希变了就解除（放了改好的包；任务到期时查，每个包最多每 `scheduling.suspension.lift_check_min_interval_ms` 一次，默认 60 秒），不用重启；重启以后挂起不在了，后台检查
      再判一次。改好以后还超的，下一次准备又挂起。
    - 安装工具在换文件之前说的：预览里目标的 check-config 给每个被拒的包一条 `pack_admission_suspended` 注记，计划照常通过（见上）；它只查组合出来的过程绑定的包，`standard-pack check`
      查根里登记的每一个包。
    - **次序**：先用本版的 `standard-pack check` 查遍每个包根；查出被拒的，先在 v0.12.56 上放改好的包（v0.12.56 收新旧两版）；再预览，看目标 check-config 的注记里没有
      `pack_admission_suspended`；然后部署。反过来先部署也装得上，只是被拒的包绑定的任务在每台实例上都停下，直到改好的包放上去。
  - **控制台 UI**（最新发版 UI v0.12.16 是 UI main 的 `6c2e30f2`；UI main 现在是 `d19a2b69`，之后只改了建模的目录，没有动控制台的声明和 Runtime 钉点）：它声明 `actingd-config` [5, 28]、
    `ledger` [3, 8]、`runtime-client` [3, 7]、`runtime-outcomes` [1, 1]（`crates/acui-setup/component-interfaces.json`），本版这几项和 v0.12.56 相同，所以和 v0.12.56 一节的结论一样配得上：
    `ledger` 8 在 [3, 8] 里，`runtime-client` 两边都是 [3, 7]，`actingd-config` 三方共有修订 28。它的安装器照清单装文件，两个新的 Tools 文件也装进 `tools\`。在它的范围升到 29 以前，
    照旧不要用它改 `scheduling.task_held` 的键或编辑写了 `device_group` 的配置。
  - **部署工具包的命令行**：它跑的全是在线安装工具的 `actingctl install`：`release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop --hold --known-pause …`、
    `status --root [--operation]`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause … --start --detach`、`release-hold --root --reason`；
    回答它们的是在线的 v0.12.56 的安装工具，代码就是上面核过的那份，**没有一条被拒**。**答法变了的**：计划的 `ledger_verify` 是 false、步骤里没有它，`notice.rollback_window_bounded` 是
    false，`defaults_diff` 是空的，`files` 里多两条 `new`（`notice.added` 2），被拒的包会让 `check_config.target.notes` 多 `pack_admission_suspended`；部署以后 `new_keys` 的 `keys` 是空的，
    `install status` 的 `rollback_window` 里 v0.12.56 的 `ledger_edge_passes` 是 true。工具包另用新版 actinglab 跑的 `--json standard-pack check --root <包根>`：干净的根答法照旧，
    有声明超时限的包的根现在被拒（见上）。**工具包要跟着改的**：在线安装工具换成 v0.12.56 的那一份；三步都去掉 `--accept-ledger-one-way`；预期改成没有 `ledger_verify`、`defaults_diff`
    为空、`new_keys` 为空、多两个 `new` 文件；「装上就没有退回的路」那一段对这一步不再成立（能退回 v0.12.56）；部署前查包那一步（新版 actinglab 的 `standard-pack check`）要求每个根都退出 0、
    `status` 是 `checked`，否则先放改好的包；部署后核文件的那一步可以加上两个新程序的哈希；给热放置工具包写构建记录那一步，`actingd-config`、`package`、`ledger` 照旧是 `5,29`、`3,9`、
    `3,8`，换的只是提交和清单。
  - **热放置工具包的命令行**：actinglab 两行，`--json standard-pack check --root <游戏根>` 和 `--json standard-pack compose --root <包根> --instance <别名>=<游戏>/<区服> … --out <新目录>`：
    `apps/actinglab/src/standard_pack_cli.rs` 和 `crates/resource-tooling` 自 v0.12.56 没改，标志清单不变；但重钉到本版以后，check 用的是本版的准入，**一个包声明的采样需要超过时限的根现在
    被拒**（退出码非 0，`package_invalid`），工具包在 S7 停下、实例保持暂停，这是对的，要放的是改好的包；干净的根答法照旧。actingctl 的 `status`、`status --config`、
    `pause --instance <别名> --reason <理由> --drain-timeout-ms <毫秒>`、`resume --instance <别名>`、`schedule --instance <别名>`、`overview --root <根> --instance <别名> --sections catalog`
    （都带 `--state-root`）：`apps/actingctl/src` 没改，`overview` 走的 `lab_ops::overview::dispatch` 现在经不带预算的 `dispatch_within`（`crates/lab-ops/src/overview.rs` 1392–1393），整份答回，和以前一样；actingledger 的
    `events --after … --through … --limit …`：`apps/ledger-forensics` 没改。**没有一条被拒，也没有一条换了答法的形状**；`status --config` 的参数行不变（没有新键），`schedule --instance`
    可能多出 `pack_admission_suspended` 推迟（现成的形状）。**热放置工具包要学的**：重钉到本版（构建记录的三个值不变，默认值文件内容不变），S7 的检查会拒 SBUD-1 的包；没有新的子系统、
    检查警告码或 `requires` 词。
  - **今天的 MCP 服务器（`actingctl mcp-serve`，现在注册着的那个）**：`apps/actingctl/src/mcp` 自 v0.12.56 没改，41 个工具和档位不变。LAB-5G1b 把 Lab 的核心行的服务代码改成接受调用方给的
    预算和游标绑定，但今天的服务器调的是 `crates/lab-ops` 里下面一层的读法和 `within_budget`，这些没改；各个 `dispatch_on` 和 `dispatch` 都是不带预算的 `dispatch_within`。所以每个工具答法照旧。
    看得到的差别只有：`ac_outcome_code` 里 `lab_operation_completed` 的属主是 `actingctl`，新码查得到，那三条提到 `record draft` 的描述；`ac_diagnose`、`ac_schedule` 可能列出 SBUD-1 挂起的包。

运行上的变化：

- 从 v0.12.56 部署：不用标志，没有 `ledger_verify`，`defaults_diff` 为空，能退回 v0.12.56；部署前用本版的 `standard-pack check` 查遍包根，被拒的先放改好的包。
- 声明的采样需要超过时限的包装载时就被拒、挂起，绑定它的任务在每台实例上都不跑，放上改好的包就自动解除；不再是跑到那一步每次失败。
- 链上的下一个任务不会再碰上被空闲动作关掉的应用；临时设成 `none` 的 `idle.action` 可以撤回。
- Tools 里多了新 MCP 服务器和二代入口，但没有注册指向它们；今天的服务器和注册都照旧，换不换由主人按手册第 11 节做。
- 设备组的宿主那一半做了，多个成员的组照旧拒收；`actinglab record draft`、离线的 `--ocr-deadline-max`、`config restore --sha` 是新的命令行。

本版合并的 PR：

- #1633（#446）：`.github/workflows/commit-identity-guard.yml` 的白名单、注释和失败说明；只动 CI，没有测试。
- #1637 LAB-5G5a（#434）：新程序 `apps/mcp-entry-g2`（二代入口，代码先原样复制一代再改），新码 3 个；照搬改写的一代入口测试、5 个新测试和一个进程测试；这时还不打包。
- #1640 并车 0.12.57a：#1636 LAB-5G1b（#434，登记 §1.287）：核心行的服务代码接受预算和游标绑定，`lab_ops::front::Front::Mcp`，`lab_operation_completed` 搬到 lab-ops，5 个新测试；
  #1639 LAB-5G2a（#434）：新程序 `apps/actinglab-mcp` 的外壳和从操作表生成的工具清单，新码 `mcp_operation_unsupported`，outcome guard 给它的 `error_response` 一条 G3 豁免，6 个新测试；
  #1638 LAB-OCR-DL（#428，登记 §1.331）：开关 `--ocr-deadline-max`，3 个新测试。
- #1635 MAPP-1b（#437，登记 §1.330）：宿主那一半的八项和链上下一环的次序，新码 `device_group_action_refused`；主机门表和策略门表的新测试，测试用的假后端多一个截图钩子；没有改现有测试。
- #1643 并车 0.12.57b：#1641 LAB-5G6（#434）：两个程序进 Tools 包，安装工具的两个名字清单和 `serves_mcp`，2 个新测试和打包脚本的 3 种新情形；#1642 LAB-5G5b（#434）：救急行
  `mcp config`、`mcp entries`、`mcp plan`，新位置 `install_mcp_config`、两个新词，一份检入的 `mcp plan` 样例，10 个新测试。
- #1644、#1645 LAB-5G2b（#434）：新服务器在自己的进程里服务核心行，再服务救急行；新码 `mcp_identity_declaration_refused`、新词 `not_absolute`；9 个新测试，改造了 LAB-5G2a 钉着中间状态的断言。
- #1646、#1647 LAB-5G3（#434）：新服务器的作业、句柄、等待和收回游标，新码 `mcp_job_started`；4 个新测试，改造了两处钉工具形状的断言。
- #1648 IDLE-3（#447，登记 §1.332）：空闲动作的规则；主机门行 `IDLE-3` 的 5 个测试（`gate_idle3` 4 个、`gate_idle3_host` 1 个）；没有改现有测试，没有新码、新键。
- #1649 SBUD-1（#448，登记 §1.333）：准入的比较；执行门行 `#448 SBUD-1` 的 4 个测试和 actinglab 的 1 个；一个现有夹具的步骤时限按它的需要量放到 32000（断言没动）。
- #1651、#1652 LAB-5G4b（#434）：新服务器经 `actinglab serve --stdio --front mcp` 子进程服务其余的行，Lab 的超长回答导出规则和截图后的客户端动作；4 个新测试，改造了 4 处钉服务划分的断言。
- #1650 LAB-5G4c（#434）：`actinglab record draft`；5 个新测试，一个列表测试多算一条命令。
- #1653（#446）：本机路径换成中立示例（测试、注释、一处契约示例、七份报告、两份下载说明）；没有增删或放宽测试，`agent_wake/summary.rs` 的期望长度按新示例重算。
- #1654、#1655、#1657 LAB-5B1、LAB-5B1b、LAB-5B1c（#434）：表驱动的行读的参数、交出的标志和接收方的标志清单都钉成和操作表一致，`config restore` 声明 `sha`；只有新测试，一个现有测试多查一类行。
- #1658 LAB-5G7（#434）：`apps/actinglab-mcp/MANUAL.md`，说明文字定稿；2 个新测试。

## v0.12.56

本版是 v0.12.55 之后合并的 10 个 PR：#439 CODES-IX（#1622）、CODES-T1（#1626）、CODES-T2（#1632），#441 许可声明（只改文档，#1627），#438 HELD-C（只有契约，#1625）、
HELD-1（#1630），#71 FLK-13（只动 CI，#1628），#437 MAPP-C（只有契约，#1624）、MAPP-1a（#1631），以及 #442 REM-1（#1629）。先说部署：**v0.12.55 发布了、但一直没有部署，
生产上跑的是 v0.12.54，本版从 v0.12.54 直接装，所以 v0.12.55 那一步单向的修订就在这次部署里跨过去**：`ledger` 从 [3, 7] 到 [3, 8]，`runtime-client` 从 [3, 6] 到 [3, 7]，`package`
从 [3, 8] 到 [3, 9]（这三处是 v0.12.55 升的，本版没再动）；本版自己只把 `actingd-config` 从 [5, 28] 升到 [5, 29]，`runtime-outcomes` 仍是 [1, 1]。部署要带
`--accept-ledger-one-way`（预览、`install update plan` 和 `install update apply` 都要带），计划多一步 `ledger_verify`；**装上并跑过以后，同一个状态根上没有退回的路**（见下面的升级与退回）；
控制台要先换成读得到 `ledger` 8、`runtime-client` 7 的 UI v0.12.16。新键是 `scheduling.task_held` 的两个（整节和每台）和实例条目的 `device_group`；默认值文件多这两个键和一份恢复档；
`config.subsystems` 不变。**部署前要只读看一遍配置文件**：两个条目指向同一台设备、又没有编成一组的，本版拒收（check-config 和启动都拒）。运行上要知道的是这几处：

- **从 v0.12.54 部署，跨过 v0.12.55 的单向一步**（为什么是单向的、修订 7 的读者碰到什么会失败，见下面 v0.12.55 一节的开头；`contracts/component-interfaces.md` 的 `ledger` 8 一行 141、
  `runtime-client` 7 一行 148、`package` 9 一行 157；`distribution/windows/component-interfaces.json` 4–10）。在本版的代码上又核了一遍，操作员要做的是：
  - **标志**：`install update plan --root <根> --build <提交> --preview`、不带 `--preview` 的 `install update plan` 和 `install update apply … --plan <id> …` 都加 `--accept-ledger-one-way`。
    不带时 `ledger` 的 `one_way` 边不过（`crates/lab-rescue/src/install/plan.rs` 295–301），计划以 `interface_edge` 拒（1030–1041），什么都不改；计划 id 是整份计划的哈希、含这个输入
    （966–992），所以 apply 要用带了标志的预览给的 id。
  - **多的一步**：计划的 `ledger_verify` 是 true（915），apply 在换任何文件之前先跑目标构建的 `actingcommand-actingd.exe ledger-maintenance verify --config <根>\actingd.config.json`
    （`update.rs` 528–538、836–862），期限是 `install.child.ledger_verify_timeout_ms`（默认 1800000，`distribution/windows/actingd.defaults.json` 576）。不过或到期限时什么文件都没换，
    但带 `--stop` 的部署这时 Runtime 已经停了、hold 还在。
  - **退不回去**：声明 `ledger` [3, 8] 的 Runtime 在一个状态根上跑过以后，声明 [3, 7] 的 v0.12.54 及更早的版本都不能再在这个根上跑，不论写没写过新形状的记录；从 v0.12.54 装上来的根，
    连 v0.12.55 也退不过去（见退回）。
  - **控制台先换**：UI v0.12.16（11-12 发布，钉 Runtime v0.12.55）声明 `ledger` [3, 8]、`runtime-client` [3, 7]，读得了本版；UI v0.12.15 读不了（见 v0.12.55 一节的配对）。没有机器检查替你
    守这个次序：控制台的边只在门测试里算（`plan.rs` 414–431），`actingctl install` 不看控制台。
  - **配置**：部署前只读核一遍，没有两个条目指向同一台设备（见下面的设备组）；也不要提前往文件里写本版的新成员（v0.12.54 自己拒，它的安装工具也不认识 `scheduling.task_held`）。
  - **生产上**：自 2026-11-11 起有一台实例在试跑调度，其余实例暂停着；部署本身不改变派单。
- **重启以后补救照常生效**（#442 REM-1，登记 §1.326；`contracts/remedies.md` 7.1）。
  - **原来的毛病**：组合模式（调度器工作台组合任务包）的根上，启动时先建补救表、后装组合出来的过程清单：表建成空的，却带着当前目录的哈希，目录不变就不重建，建表时也不打任何一行。
    所以每次重启以后，包里声明了补救的结果都不开补救的一轮，直到下一次放包引起激活；没有码、记录或日志说明。
  - **现在**：组合模式下，启动在建表之前先认领当前生效的组合代、把它的过程清单装上（`crates/runtime-host/src/host.rs` 2361–2384，`host/standard_pack_schedule.rs` 421
    `adopt_standard_pack_generation_on_start`、449 `adopt_active_generation`），第一轮工作台检查看到已经认领过，不再认领一次，读各游戏目录、组合照旧；表记下它所用清单的摘要，评估时清单的
    摘要变了就立刻重建（`crates/runtime-host/src/remedy.rs` 的 `table_due`）。认领不出清单时和以前一样用一份什么都不绑的清单，没有新的失败路径。配置模式不变。
  - **每张生效的表打一行**（`remedy.rs` 142–180）：`actingd remedy_table built catalog_hash=<哈希> manifest=<sha256:…|none> tasks=<n> located=<n> remedy_tasks=<n> providers=<n> unbound=<n>`；
    有任务找不到包位置时再打一行 `actingd remedy_table unbound tasks=<前十个> more=<其余的个数>`。原有的 `finding`、`unreadable` 两行照旧。
  - **怎么看**：重启后的启动日志里，`actingd startup_recovery` 之前应有一行 `remedy_table built`。组合模式下健康的样子是 `manifest=sha256:…`、`located` 等于 `tasks`、`unbound=0`，
    `remedy_tasks`、`providers` 是声明了补救的任务和提供方的个数，没有 `unbound` 那一行。`manifest=none` 是说这张表建的时候没有清单，哪个任务都没读到包；出现 `unbound` 那一行，是说列出的
    那些任务没读到包，它们的阻塞结果不开补救的一轮。之后每次激活、清单变化、补救轮次开着时的每次重建各再打一行；还没有组合代的第一次启动，第一行在第一轮评估时才出现。
  - 没有配置键、记录形状或接口变化；旧版本照旧有这个毛病。本版上线以后，重启不再要靠放包引起的激活来把表建对。
- **压着太久留一条记录**（#438 HELD-1，登记 §1.322 契约、§1.327；`contracts/scheduling/README.md` 的 “Held records”）。
  - **记什么、什么时候记**：一对（任务、实例）该跑（或判断不出该不该跑），被「压着」那一类原因里的同一个挡过门限，记一条警告 `task_held`（`policy.planning_signal_observed`，种类
    `goal_missed`，`signal:task_held:held:<hex>`；值 `gate_code`、`held_state`（`due` 或 `undecidable`）、`held_since_unix_ms`、`held_for_ms`、`held_after_ms`，挡住它的原因整条挂成一个
    `caused_by`）；这次压着结束，再记一条信息 `task_held_ended`（`held_end`）。同一次压着只报一次，重启前后也一样（`crates/runtime-host/src/policy_host/held.rs`）。计时用「为什么没跑」
    那张表自己的 `since_unix_ms`；同一个读数（`fact_key`）的三种刷新等待共用一个计时。
  - **哪些原因算「压着」**：封闭的 18 个码（`held.rs` 44 `HELD_CODES`），加两个按值定的；比如无进展护栏、实例不可用、排在别的运行后面、等读数刷新、判断不出触发条件。人、配置或数值
    说了不跑的（暂停、免打扰、关着的包、用完的预算）、等它自己的时间的（冷却、间隔、退避）和没到点的，从不算；正在派单或在跑的一对也不算。
  - **不改任何决定**：记录在评估和它的解释之后才找，没有决定读它；开着、关着，每一轮的决定、派单和别的记录都一样。
  - **两个键**：`scheduling.task_held.enabled`（默认 `true`）和 `scheduling.task_held.after_ms`（默认 1800000，即 30 分钟；60000 到 86400000），每台也可以单独设
    （`instance.<实例 id>.scheduling.task_held.<成员>`，默认取整节的），运行中可改，下一轮评估生效；关掉只是不再记新的，已经报过的照样记结束。默认值文件带着它们，配置文件不用写。
  - **在哪里看**：`actingd suspended` 的 `held`（还开着的、或在账本最新一条的 48 小时内结束的，新的在前，最多 200 条，结束了的带 `ended`；另有 `held_total`）；带实例的 `ac_diagnose`
    和 `actingctl diagnose` 的 `held` 行，对得上的 `not_run` 行标 `held_reported: true`；`ac_events` 和不带实例的 `ac_diagnose` 的错误视图。规划信号不挂实例，按实例过滤的事件读法看不到它。
  - **什么结束一次压着**：这一对被选中（`selected`）；换了原因（`why_changed`：触发条件变假、包被关、暂停，或者换了读数键的刷新等待）；这一对的行没了（`row_gone`）；实例不在评估里了
    （`instance_gone`）；Runtime 重启（`restarted`：启动在记结算锚点之前，给之前的运行留下的、没结束的压着各补一条，不带 `held_for_ms`）。本版第一次启动没有要结束的。
  - **部署以后**：一台实例上有一对被压过 30 分钟，错误视图里就出现 `task_held` 警告；生产上暂停着的实例，暂停这一原因本身不算「压着」。启动日志里要是出现
    `actingd task_held unreadable sequences=<序号,…> more=<n>`，是启动收尾时碰到了读不回的压着记录（原因码不是本版目录登记过的、缺值、或者编号和内容对不上，比如别的版本写的）：
    它不结束这几条、照常启动，把它们的账本序号（前八条，其余计数）打在这一行，`actingd suspended` 一直把它们列成未结束；不影响运行，报给维护的人。
  - **不叫醒任何人**：叫醒那一半（HELD-2，规则种类 `task_held`）不在本版，没有消息会到人的频道。
- **一台设备上几个应用：先做写法和检查**（#437 MAPP-1a，登记 §1.323 契约、§1.328；`contracts/device-group.md`）。
  - **写法**：实例条目多一个可选成员 `device_group`，1 到 64 字节、照别名规则的名字，不写任何设备值；写同一个名字的条目是一组。只在装配时读一次，不能运行中改；配置清单只给写了它的
    条目一行 `instance.<实例 id>.device_group`，check-config 的那一行多 `device_group`。名字不合规拒成 `device_group_config_invalid`、`name_invalid`
    （`apps/actingd/src/config/device_group.rs` 229）。
  - **不写它的条目、只有一个成员的组，和以前一样**。
  - **两个条目没编成一组、却指向同一台设备，现在拒收**（`device_group_config_invalid`、`undeclared_shared_device`）。check-config 和启动在装配时比 `instance_index`、`instance_name`、
    `serial`，或 `host` 加 `port`（端点只在都没写序列号的条目之间比；`device_group.rs` 363–376）；启动在发现之后再比一次（同一台模拟器的编号和名字，或者一个写死的端点和一个靠发现绑定的
    条目），这一处是 Provider 启动失败，整个启动失败（`apps/actingd/src/config/provider_startup.rs` 139–180、262–264），check-config 不跑发现、看不出来。以前的版本会给这样两个条目各起一个
    工作线程去开同一台设备。**部署前只读看一遍配置文件，确认没有这样的两个条目**；有的话先改好文件再部署。
  - **多个成员的组这一版一律拒收**：组内的规则（成员指同一台、不是手机或夹具、`application_id` 各不相同、都有 `startup_package`、不开整台设备的动作）查过以后，最后照样拒成
    `several_members_unsupported`（`device_group.rs` 377–383），等带「同一台上先后怎么排」那一片的版本才放开。宿主那一半（MAPP-1b）和开跑前的切换（MAPP-2）都不在本版，这一版收的配置
    走不到它们。
  - **调度器和评估器已经按设备认**：一台设备一份租约、一个队列，一轮每台只选一个，同台别的成员带 `device_group_waiting` 等（这个码已归进「压着」那一类）；只有一个成员时设备键就是它
    自己的实例 id，决定和以前逐字节相同。设备组只在内存里，什么都不落盘。
  - **默认值文件多一份恢复档** `without_emulator_restart`（`return_home`、`application_restart`；`distribution/windows/actingd.defaults.json` 111–113），给组的成员点名用。照
    `contracts/device-group.md` 的 “A group of one is today”：文件和指令都没改 `recovery_ladder.profiles` 的根上，变的只有三样：配置清单里 `recovery_ladder.profiles` 那一行的值、带着它的
    程序事实 `config.parameters.recovery_ladder`、恢复阶梯的配置摘要（`recovery_ladder_started` 的 `config_digest`）；文件自己写了一组档的根，整组替换默认值，什么都不变；用指令设过这个键的
    根保留指令的值，启动时记 `config_override_base_changed`。没有条目点名它，就没有阶梯按它爬，每条阶梯照旧爬原来的档。
- **结果码说清原因和处理办法，仓里有了索引**（#439 CODES-IX、CODES-T1、CODES-T2，登记 §1.321、§1.324、§1.329）。
  - **索引**：`contracts/outcome-codes/index/`（一份总表、每个属主一份，共 30 个文件），由 `ci/outcome_codes_index.py` 从码目录生成，CI 的 `rust` 一项用 `--check` 守它是最新的；
    没有程序读它，运行上没有变化。
  - **第一批**：54 个致命和警告级的码（致命 4、警告 50）写上「常见原因」和「处理办法」；`display_map_stale`、`fact_reader_document_mismatch` 代码里已经没有地方发出，没写。**第二批**：
    生产上近七天碰到过、还没写的 21 个错误码（合同 9、执行内核 10、宿主 2）。另改写了 13 条描述。`actingctl codes <码>`、MCP `ac_outcome_code` 看得到；类别、阶梯分类、值、状态都没动。
- **只改文档或 CI 的两片**：#441 把仓库的许可声明和第三方声明更新到现状（`NOTICE.md`、两份 README 的许可一节，和 `resources/upstream-manifest.toml` 里一条只给人读的来源记录）；#71 FLK-13
  给 CI 每个模块作业的测试步骤加了比作业低的时限（`host-lab` 作业 45 分钟），步骤超时后日志照样上传，`ci/gate_check.py` 认得中途停下的日志、点名没跑完的测试。产品不变。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`、`instance add`。
  `crates/lab-rescue` 自 v0.12.55 没改，`actingctl install` 走的就是 v0.12.55 说明核过的那份代码。

配置：只升 `actingd-config` 一次，按代码、`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md` 核过。`component-interfaces.json` 是 `actingd-config` [5, 29]、
`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 8]、`runtime-client` [3, 7]、`package` [3, 9]、`runtime-outcomes` [1, 1]；架构测试
`tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120`（141 起）钉 [5, 29]。

- 升 `actingd-config` 一次，两片共用（`contracts/component-interfaces.md` 修订 29 那一行，129）：HELD-1 的 `scheduling` 一节和实例条目 `scheduling` 成员里的 `task_held`
  （`crates/runtime-config/src/scheduling.rs` 的 `SchedulingFile::task_held`、`InstanceSchedulingFile::task_held`），和 MAPP-1a 的实例条目成员 `device_group`（`apps/actingd/src/config.rs`
  1158–1163）及 check-config 报告里的 `device_group`。都是可选的：不写它们的文件还是原来的修订，本版照读修订 5 到 28；本版以前的 Runtime 拒收写了它们的文件（那一节和条目都不收未知成员，
  `config_decode_failed`）。HELD-2 的规则词和 MAPP-2 的切换冷却键不在本版，契约里写的是同一版就并进 29、否则取下一个空号，那几处原文没动。
- `ledger` [3, 8]、`runtime-client` [3, 7]、`package` [3, 9]、`runtime-outcomes` [1, 1] 和 v0.12.55 一样，本版没动：压着记录是现成的 `policy.planning_signal_observed`（`goal_missed`），
  读者只拼写码（`ledger` 修订 5 的规矩）；设备组不落盘；没有请求或回答改形状（`ac_diagnose` 的 `held` 是 actingctl 自己拼的）。`install-selection`、`install-control` 不变。
- 配置键（`distribution/windows/actingd.defaults.json` 和 `contracts/runtime-config.md` 的键表）：新增 `scheduling.task_held.enabled`（布尔，默认 `true`）、`scheduling.task_held.after_ms`
  （毫秒 60000 到 86400000，默认 1800000），每台的 `instance.<实例 id>.scheduling.task_held.enabled`、`.after_ms`（默认取整节的；这四个都能经 `ConfigureSettings` 运行中改：Lab 的
  `config set`、MCP `ac_config_set`），和 `instance.<实例 id>.device_group`（字符串，没有默认值，不能改，装配时读，只在写了它的条目上有这一行）。默认值文件多两处：`scheduling.task_held`
  （382–385）和 `recovery_ladder.profiles` 的 `without_emulator_restart`（111–113，是已有键的一个值）。没有去掉的键。
- `config.subsystems` 不变：`apps/actingd/src/config/manifest.rs` 和 `crates/policy/src/compose.rs` 自 v0.12.55 都没改，没有新的子系统，也没有新的 `requires` 词。
- 结果码：目录从 3912 个码变成 3917 个（在用的 3894 → 3899，退役的仍是 18 个），位置仍是 1750 个，仍是 29 个分片，键从 635 个变成 642 个，词表从 223 个变成 227 个，没有退役或定稿的码。
  新码 5 个（都待定稿，since 0.12.56，文字都写了）：`task_held`（runtime-host，warning，ladder `none`）、`task_held_ended`（runtime-host，info，`none`）、`device_group_config_invalid`
  （actingd，error，`none`）、`device_group_assembled`（actingd，info，`none`；这一版没有哪次启动会打它，多成员的组都被拒）、`device_group_waiting`（policy，info，`none`）。新键 7 个：
  `held_state`、`held_end`、`held_since_unix_ms`、`held_for_ms`、`held_after_ms`、`device_group_problem`、`device_group_wait`；新词表 4 个：`held_state`（`due`、`undecidable`）、`held_end`
  （`selected`、`why_changed`、`row_gone`、`instance_gone`、`restarted`）、`device_group_problem`（`name_invalid`、`device_mismatch`、`handset_member`、`fixture_member`、`application_shared`、
  `startup_package_missing`、`device_action_unsupported`、`undeclared_shared_device`、`several_members_unsupported`）、`device_group_wait`（`selected`、`held`、`player_hold`、`fenced`）。
  改了的：75 个在用码的「常见原因」和「处理办法」从待填写成了（CODES-T1 54 个、CODES-T2 21 个），13 条描述改写（`sqlite_backup_busy`、`sqlite_backup_locked` 和 CODES-T2 的 11 条），
  键 `gate_code` 只改了说明，前缀 `device_` 的域多给了 `policy`；没有哪个码的类别、阶梯分类、值、状态或审阅变了。在用的码里两段文字都写了的从 625 个变成 705 个。本版的发布文件是空的：
  没有新定稿或退役的条目。
- 程序文件：`Cargo.toml`、`Cargo.lock` 自 v0.12.55 都没改，没有新的依赖；`.github/` 只改了 `ci.yml`（FLK-13 的步骤时限，CODES-IX 的两步）；`ci/` 多了索引脚本和它的测试，`gate_check.py`
  认得中途停下的日志，`ci/outcome_release.py` 把读分片的那段提成了一个函数（发布文件的写法不变）；`ci/retired-gates.toml` 没改，没有测试离开门表。`crates/lab-rescue`、`apps/actinglab`、
  `apps/ledger-forensics`、`apps/mcp-entry`、`ratchet/`、`contracts/lab-operations.json` 都没改；`apps/actingctl` 只改了 `ac_diagnose` 的说明和结果模式（多 `held`），MCP 工具仍是 41 个。
  不稳定表照旧是 FLK-08 和 FLK-10，这几片都没动它，也没动它们读的测试和金样；FLK-13 是 CI 的超时，不在表里。

升级与退回（安装工具的代码 `crates/lab-rescue` 自 v0.12.55 没改，自 v0.12.54 只改了救急模块的用法文字和一个配对测试；其中 `plan.rs`、`update.rs`、`undo.rs`、`programs.rs`、`postcheck.rs`、
`host.rs` 自 v0.12.51 没改，`deploy.rs` 自 v0.12.54 没改，所以 v0.12.54、v0.12.55 和本版的安装工具走的是同一份代码，下面的行号对三者都对；判法和 v0.12.55 说明核过的一样，这里是本版的数）：

- **v0.12.54 的安装工具装本版：能装，要带 `--accept-ledger-one-way`，不带就被拒。** 安装工具的声明是它自己构建里的 `component-interfaces.json`（`host.rs` 23–25），v0.12.54 的是
  `actingd-config` [5, 28]、`ledger` [3, 7]、`runtime-client` [3, 6]、`package` [3, 8]、`runtime-outcomes` [1, 1]。计划的边逐条（`plan.rs`）：
  - `ledger` `previous_max_in_next`（288–294）：Rp [3, 7] 的上限 7 在 R\* [3, 8] 里，过。`one_way`（295–301）：8 大于 7，只有带了标志才过；不带时这是唯一不过的边。带了以后
    `inputs.accept_ledger_one_way` 是 true，`notice.rollback_window_bounded` 是 true（203），`ledger_verify` 是 true（915）。
  - `runtime-outcomes` `next_max_in_installer`（302–314）：R\* 的 1 在安装工具的 [1, 1] 里，过。
  - `install-control`、`runtime-client` 的 `installer_meets_previous` 和 `installer_meets_next`（315–331）：安装工具的 `runtime-client` [3, 6] 和 Rp [3, 6]、R\* [3, 7] 都相交；
    `install-control` 三方都是 [0, 1]。都过。
  - `package` `bundle_max_in_next`（332–355）：目标是 [3, 9]，现装的包声明的上限都落在里面（没有声明的照旧是 `pack_declaration_unknown` 警告），过。`speaks` `runtime_equals_tools`
    （756–763）：两半的声明相同，过。
  - `actingd-config`：**更新计划里没有一条边拿安装工具的 [5, 28] 去比目标的 [5, 29]**（三方共有一个修订的那条边只在 `ui_edges` 里，门测试才算，430）。`installer_knows_keys`（359–376、
    764；`unknown_keys` 506–536）拿安装工具自己构建里的键表和默认值比：现在的文件不写新键，过；v0.12.54 不认识 `scheduling.task_held`，文件里写了它这条边就不过（`instances` 不是它查的
    一节，`device_group` 不进这条边）。`next_check_config`（902–914）：本版的 check-config 带 `--install-root`（29 ≥ 17，854–868）读现行的文件，过；只有文件里有两个条目在装配时就看得出
    指向同一台设备时，计划以 `config_refused_by_target` 拒（1021–1029），原因是 `device_group_config_invalid` `undeclared_shared_device`。只有发现之后才看得出的那一种，预览看不出，要到
    新 Runtime 启动时才拒（见上面的设备组）。
  - 计划的 `defaults_diff`（574–598）：从 v0.12.54 装是三行，`recovery_ladder.profiles.without_emulator_restart.rungs`、`scheduling.task_held.after_ms`、`scheduling.task_held.enabled`，
    `live` 都没有，`source` 是 `default`（文件写了那个路径时是 `file`；文件自己写了一组档的根，第一行照样是 `default`，但新档被文件的那一组替换，不生效）。
  - 部署后检查的 `new_keys`（`postcheck.rs` 494–526；`added_keys` 593–601 取 `defaults_diff` 里 `live` 为空的行，就是这三行）：两个压着键在 `config.parameters.scheduling` 里的来源应是
    `default`；那份档的路径不是配置清单的一行（清单只有 `recovery_ladder.profiles` 一行），列在 `unseen` 里，不算不过。
  - **操作员要做的**：先换控制台（UI v0.12.16）；只读核一遍配置（两个条目、一台设备）；`install update plan --root <根> --build <提交> --preview --accept-ledger-one-way`，在预览里看到
    `ledger` 的 `one_way` 是过的、`ledger_verify` 是 true、`defaults_diff` 是上面三行；再用这次预览给的计划 id 跑 `install update apply --root <根> --build <提交> --plan <id>
    --accept-ledger-one-way --stop --hold <理由> … --start --detach`；日志的 `acknowledged` 记下 `accept_ledger_one_way`（`update.rs` 700–704，`deploy.rs` 1720–1724）。
- **从 v0.12.55 装本版**（v0.12.55 没有部署过，只作对照）：两边的 `ledger` 都是 [3, 8]，`one_way` 不触发，不用标志，计划里没有 `ledger_verify`；v0.12.55 的安装工具的 `runtime-client`
  [3, 7] 和两边相交；别的边和上面一样，`installer_knows_keys` 同样不认识 `scheduling.task_held`；`defaults_diff` 也是那三行（v0.12.55 的默认值文件和 v0.12.54 的一样）。
- **退回：从 v0.12.54 装上本版并跑过以后，安装工具哪一版都不让退回。**
  - **退到 v0.12.54 或更早**：本版声明 `ledger` [3, 8]，它们声明 [3, 7]，`rollback plan`、`rollback apply` 的 `previous_max_in_next` 不过（8 ∉ [3, 7]），在算回退门之前就以
    `interface_edge` 拒（`plan.rs` 288–294、1030–1041），带不带标志都一样（标志只放行 `one_way`，退回方向本来就过）。
  - **退到 v0.12.55**：边都过（8 ∈ [3, 8]，`runtime-client`、`package` 两边相同；`actingd-config` 29 对 28 没有边要比，`installer_knows_keys` 用的是在线的本版安装工具，认识新键），但回退门的
    第 2 部分 `ledger_readability` 只对「本版替换掉的那一版」放行（`update.rs` 400–437，`live_operation` 181–204；`contracts/install-cli.md` 的 “The rollback gate” 第 2 条）。从 v0.12.54
    装上来的根，替换掉的是 v0.12.54，所以退到 v0.12.55 以 `rollback_gate_failed`（部分 `ledger_readability`，`s0` 为空，`previous` 是 v0.12.54 的提交）拒。
  - **部署的自动回退也走不了**：新 Runtime 起来以后部署后检查不过时，部署按回退到 v0.12.54 算计划（`deploy.rs` 763–854，813 不带标志），同一条边不过，以 `rollback_gate_failed`（部分
    `interface_edges`，803）结束，新构建和 hold 都留着。起过本版 Runtime 的操作，`install undo` 也按回退判（`undo.rs` 376–406，388 不带标志），同样被拒；没起过 Runtime 的未完成操作照旧能
    撤销。部署以后 `install status` 的 `rollback_window` 把 v0.12.54 及更早的保留构建列成 `ledger_edge_passes: false`。
  - **对操作员一句话：从 v0.12.54 装上本版并跑过以后，同一个状态根上没有退回的路。** 出了问题剩下的是：配置的问题，改好文件再起（比如上面那两个条目）；程序的问题，往前修（装更新的版本）；
    或者换一个全新的状态根（空账本：之前的账本、事实、运行中设的指令、封顶回合和空闲状态都留在旧根上）。
  - **如果是从 v0.12.55 装上来的**（只作对照）：退回 v0.12.55 边都过，回退门第 2 部分对它放行（它就是被替换的那一版），要过的是第 4 部分 `config`：v0.12.55 的 check-config 读现行的文件
    （`update.rs` 490–503），文件里写了 `scheduling.task_held`（整节或条目）或 `device_group` 就拒（`config_decode_failed`），所以先把它们去掉，去掉以后照常过。
  - **旧版在这个根上会怎样**（只作说明）：v0.12.55 读得了压着记录和它的结束，什么也不做（登记 §1.322 的 “The interface walk”：读者只拼写码、不查登没登记；它的 `actingd suspended`
    只读它自己的前缀）；换版时还开着的压着不会有结束记录。补救表的毛病在旧版上照旧。设备组什么都没落盘，没有要清的。
- 配对：
  - **v0.12.54 写的配置文件**：本版照收，不用改。两个新成员都是可选的；一个文件只有写了 `scheduling.task_held`（整节或某个条目的 `scheduling`）或某个条目的 `device_group`，才算修订 29。
    唯一的例外是上面的「两个条目、一台设备」：v0.12.54 收，本版拒。v0.12.55 写的文件同样。
  - **安装工具**：见上；从 v0.12.54 部署跑的是在线的 v0.12.54 的安装工具，要带 `--accept-ledger-one-way`。
  - **部署工具包的命令行**：它跑的全是在线安装工具的 `actingctl install`：`release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop --hold --known-pause …`、
    `status --root [--operation]`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause … --start --detach`、`release-hold --root --reason`；
    回答它们的是 v0.12.54 的安装工具，代码就是上面核过的那份。**被拒的**：不带 `--accept-ledger-one-way` 的 `update plan`（连预览）和 `update apply`，以 `interface_edge` 拒，不过的是
    `ledger` 的 `one_way`。**答法变了的**（带了标志以后）：计划的 `ledger_verify` 是 true、步骤以 `ledger_verify` 开头、`notice.rollback_window_bounded` 是 true、
    `inputs.accept_ledger_one_way` 是 true，日志的 `acknowledged` 多这一项；`defaults_diff` 是上面三行；部署后检查的 `new_keys` 核这三个键；部署以后 `install status` 的
    `rollback_window` 里 v0.12.54 及更早的构建 `ledger_edge_passes` 是 false。`release stage`、`guards`、`status`、`release-hold` 答法照旧。工具包另用新版 actinglab 跑的
    `--json standard-pack check --root <游戏根>` 照旧收：`apps/actinglab` 自 v0.12.55 没改，这一行不带 `--help`、不以 `--instance` 开头、不是 `events`，v0.12.55 的命令行改动碰不到它，
    也没有新的检查警告码。**工具包要跟着改的**（它上一次用在 v0.12.54）：预览、计划、apply 三步都加 `--accept-ledger-one-way`；说明里「不带单向标志、没有 `ledger_verify`」和退回那一段
    不再对（从 v0.12.54 装上本版以后没有退回）；安装工具是在线的 v0.12.54 的那一份；给热放置工具包写构建记录的那一步，核新包清单的 `actingd-config`、`package`、`ledger` 现在是
    [5, 29]、[3, 9]、[3, 8]，要记成 `5,29`、`3,9`、`3,8`（热放置工具包重钉时拿这三个值和清单比，照旧的 `5,28`、`3,8`、`3,7` 会让重钉不过），重钉前核记录的那几处也要换成这三个值。
  - **热放置工具包的命令行，本版都照旧收**：actinglab 两行，`--json standard-pack check --root <游戏根>` 和 `--json standard-pack compose --root <包根> --instance <别名>=<游戏>/<区服>
    … --out <新目录>`：`apps/actinglab` 自 v0.12.55 没改；`compose` 的 `--instance` 是这一行自己的绑定选项，CLI-IN 不碰它；两行都不带 `--help`。actingctl 的 `status`、`status --config`、
    `pause --instance <别名> --reason <理由> --drain-timeout-ms <毫秒>`、`resume --instance <别名>`、`schedule --instance <别名>`、`overview --root <根> --instance <别名> --sections catalog`
    （都带 `--state-root`）：`apps/actingctl` 的命令行代码本版没改。读账本用的是 actingledger 的 `events --after … --through … --limit …`（`apps/ledger-forensics` 没改，而且给了位置），
    v0.12.55 改的 `events` 默认次序碰不到它。**没有一条被拒，也没有一条的行换了次序。** 答的内容上看得到的：`status --config` 的参数行多 `scheduling.task_held.enabled`、`.after_ms`
    （整节和每台），`recovery_ladder.profiles` 那一行的值多一份档，子系统行不变；`schedule --instance` 的行形状不变，同一个读数的三种刷新等待之间换不再重新计时，换了读数键的刷新等待则从新
    计时；账本里会多出 `task_held`、`task_held_ended` 这种规划信号；激活以后的日志多一行 `actingd remedy_table built`。**热放置工具包要学的**：重钉时比的构建记录是上面那三个值和新的默认值
    文件；本版没有新的子系统、新的检查警告码或新的 `requires` 词。
  - **控制台 UI**（最新发版 UI v0.12.16 就是 UI main 的 `6c2e30f2`，把 Runtime crate 钉在 `c0f75c7e`，即 v0.12.55；`Cargo.toml` 的 `[workspace.dependencies]`）：
    - 它声明 `actingd-config` [5, 28]、`ledger` [3, 8]、`runtime-client` [3, 7]、`runtime-outcomes` [1, 1]（`crates/acui-setup/component-interfaces.json`）：`ledger` 8 在 [3, 8] 里，
      `runtime-client` 两边都是 [3, 7]，`runtime-outcomes` 1 在 [1, 1] 里；`actingd-config` 三方（安装工具 [5, 28]、控制台 [5, 28]、本版 [5, 29]）共有修订 28，不写新成员的文件就在 28 以下，
      照它自己 README 的规矩配得上。它的 README 说适配 Runtime v0.12.47 到 v0.12.55，先换控制台、再换 Runtime。
    - 压着记录对它是现成形状的事件，公开投影里只有码，它钉的合同 crate 照读（登记 §1.322 的 walk）；它不认识的码怎么显示是 UI 仓的事。
    - **在控制台的范围升到 29 以前**：不要用它改 `scheduling.task_held` 的键，不要用它编辑写了 `device_group` 的配置，也不要指望它显示这个成员（契约：控制台要先到修订 29 才能改、才能显示）。
      它的任务列表照旧只读，不发 `EditPackSchedule`。
  - **MCP**：带实例的 `ac_diagnose` 多 `held` 行（`held_truncated`），`not_run` 行可能带 `held_reported`；不带实例时错误视图里会有 `task_held`。别的工具答法不变。

运行上的变化：

- 从 v0.12.54 部署：带 `--accept-ledger-one-way`，多一步 `ledger_verify`，装上就没有退回的路；先换 UI v0.12.16，先核配置里没有两个条目指向同一台设备。
- 组合模式下重启以后补救表就是对的，启动日志里每次都有一行 `actingd remedy_table built` 说清建出了什么。
- 一对被压过 30 分钟留一条 `task_held` 警告、结束时一条 `task_held_ended`，在 `actingd suspended` 和 `ac_diagnose` 里看；不改任何决定，也不叫醒人。
- 实例条目可以写 `device_group`，但这一版只收一个成员的组；默认恢复档多一份，阶梯照旧。
- `actingctl codes`、`ac_outcome_code` 多了 75 个码的原因和处理办法；仓里有结果码的索引。

本版合并的 PR：

- #1622 CODES-IX（#439，登记 §1.321）：`ci/outcome_codes_index.py` 和它的 5 个 Python 测试，CI 的 `gate` 作业（检查 `rust`）多两步，`ci/outcome_release.py` 读分片的那段提成
  `owner_fragments`（发布文件不变）；索引 30 个文件。没有 Rust 改动，没有新码。
- #1626 CODES-T1（#439，登记 §1.324）：11 个分片里 54 个码的文字和 2 条描述，重新生成的索引；没有测试。
- #1627（#441）：`NOTICE.md`、`README.md`、`README.zh-CN.md`、`resources/upstream-manifest.toml`；只改文档。
- #1625 HELD-C（#438，登记 §1.322）：只有契约：压着记录、它的结束、原因的普查、计时、两个键、启动收尾、读法和接口的逐项推演；没有代码和测试。
- #1628 FLK-13（#71）：`.github/workflows/ci.yml` 的测试步骤时限和 `host-lab` 的 45 分钟，`ci/gate_check.py` 的 `timed_out`；`ci/test_gate_check.py` 新加 5 个测试、改造一个辅助函数。
  没有 Rust 改动，没动门表和不稳定表。
- #1624 MAPP-C（#437，登记 §1.323）：只有契约：新文件 `contracts/device-group.md` 和八份契约里的指引；没有代码和测试。
- #1629 REM-1（#442，登记 §1.326）：启动次序、表跟着清单走、两行日志；主机门行 `#442 REM-1` 的 4 个测试，`gate_rl1b` 的三个造数据函数放开共用，断言没动。没有新码、新键。
- #1630 HELD-1（#438，登记 §1.327）：`policy_host/held.rs`、启动收尾、两个键和两种读法；主机门行 `#438 HELD-1` 的 12 个测试（`held_t1` 到 `held_t11`，`held_t9` 的诊断那一半在 lab-ops），
  改造 5 个按成员构造诊断结果的测试（补空的 `held`，断言没动）；`actingd-config` [5, 29]，新码 2 个、新键 5 个、新词表 2 个。
- #1631 MAPP-1a（#437，登记 §1.328）：配置成员、装配检查、调度器和评估器按设备认、默认值文件的档；19 个测试（apps 门表 actingd 7 个、runtime-config 3 个，策略门表调度器 5 个、策略 3 个，
  主机门行 `#438 HELD-1` 里的 `held_t12`），没有改现有测试；新码 3 个、新键 2 个、新词表 2 个。
- #1632 CODES-T2（#439，登记 §1.329）：3 个分片里 21 个码的文字和 11 条描述，重新生成的索引；没有测试。

## v0.12.55

本版是 v0.12.54 之后合并的 15 个 PR：合并列车 #1608 带进的 #434 CLI-EV（#1606）、CLI-IN（#1604）、#429 R-8（#1607）和 #428 S168-LB1（只有契约，#1603）；列车 #1614
带进的 #428 SS-4b（#1609）、S168-LB1a（#1613）、S168-LB1b（#1611）、#434 CLI-HELP（#1612）和 #429 R-6b（只有契约，#1610）；列车 #1618 带进的 #429 R-6b1（#1616）、
#434 NEUT-2（#1615）和 #428 NAV-L1（#1617）；以及 #434 CLI-COMP（#1619）、#429 R-6b2（#1620）和 #428 DIAG-SI（#1621）。先说部署：**本版是单向的**。`ledger` 从 [3, 7]
升到 [3, 8]，`runtime-client` 从 [3, 6] 升到 [3, 7]，`package` 从 [3, 8] 升到 [3, 9]；`actingd-config` 仍是 [5, 28]，`runtime-outcomes` 仍是 [1, 1]。部署要带
`--accept-ledger-one-way`（`install update plan` 连预览和 `install update apply` 都要带，不带就以 `interface_edge` 被拒），计划多一步 `ledger_verify`。**v0.12.54 的安装工具和
v0.12.52 的安装工具都能装本版，都要带这个标志**（逐条边见下面的升级与退回）。**本版在一个状态根上跑过以后，安装工具拒绝退回本版以下的任何版本**，不论有没有写过新形状的记录；
出了问题只能往前修，或者换全新的状态根。**控制台要先于 Runtime 换**：读不到 `runtime-client` 7、`ledger` 8 的控制台，读到含被停调度运行记录的事件页就失败；UI 仓的最新发版和 main
眼下都读不到，而且没有机器检查替你守这个次序（见配对）。没有新键，默认值文件没改，`config.subsystems` 不变。运行上要知道的是这几处：

- **本版是单向的：账本修订 8、客户端修订 7、包修订 9**（`contracts/component-interfaces.md` 的 `ledger` 8 一行 140、`runtime-client` 7 一行 147、`package` 9 一行 156 和
  “Workflow #429 R-6b” 一节 389 起；`distribution/windows/component-interfaces.json` 7–9）。
  - **修订 8 的账本是 7 加两样**：一条 `record_list` 有 257 到 1024 行的 `fact.published`（S168-LB1a：行数上限 `MAX_FACT_RECORD_LIST_ROWS` 1024，
    `crates/actingcommand-contract/src/fact.rs` 20、624；记录的形状没变，变的是行数），和 `policy.execution_recorded` 的第三种结果 `stopped {outcome, runtime_ms}`（R-6b：被
    `StopInstanceRun` 停下的调度运行；`crates/actingcommand-contract/src/event/payload.rs` 3324–3341）。**修订 7 的读者两样都读不了**：前一样它按记录自己的检查拒成
    `fact_record_limit_exceeded`（它的上限还是 256），后一样是封闭的结果种类、没有兜底，拒成 `corrupt_ledger_record`；在哪里读到都一样：启动时（载入每条现行记录、核最新的
    `open_verify_tail_records` 条）、任何含它的页、回退门的 `ledger_readability`。
  - **什么时候第一次写下这种记录**：前一样只在有包的 `read_list` 读到超过 256 个位置的列表（或有客户端发布超过 256 行的记录列表）时才写；后一样只在有人用 `StopInstanceRun`
    停下一次调度派的运行时才写。部署本身两样都不写。
  - **退不回去不看有没有写过**（Rp.max 的规矩）：安装工具判的是两个构建清单里声明的范围，不读账本，账本里也不记修订号（`crates/lab-rescue/src/install/plan.rs` 288–301）。所以
    声明 [3, 8] 的 Runtime 在一个状态根上跑过以后，本版以下的版本都不能再在这个根上跑，哪怕一条新形状的记录都还没写（见退回）。SQLite 格式 2、`actingcommand.event.v3`、记录类型、
    表、索引和视图都没变，修订 3 到 7 的账本照读，不用迁移，也不用新的状态根。
  - **修订 7 的客户端是 6 加**：`InstanceFactSnapshot` 的视图里可以有 257 到 1024 行的记录列表，请求多可选的 `key_prefix`，回答多可选的 `records_total`（S168-LB1a；
    `crates/actingcommand-contract/src/runtime.rs` 3807、4383–4387、5530–5532）；事件页带着每条记录的结果，所以也带 `stopped`（公开投影里只有码；R-6b）。修订 6 的客户端读不了含被停
    运行记录的事件页（`runtime_receipt_decode_failed`，不论是 `actingctl events`、`diagnose`、`ac_events` 还是控制台的事件读），也读不了存着长列表的实例的事实；修订 6 的 Runtime
    收到带 `key_prefix` 的请求会断开连接。不带 `key_prefix` 的请求和以前逐字节相同，`min` 仍是 3。
  - **修订 9 的包是 8 加** `table_max_over_list` 的可选成员 `lower_bound_column`（S168-LB1b）。本版以前的读者在准入时拒收写了它的文档（`contained_task_select_policy_invalid`）；
    不写它的文档字节和身份都不变。用它的包声明 `package` [9, 9]：先部署 Runtime，再放这种包。
  - **控制台要先换**：`runtime-client` 低于 7 的控制台解不开含被停调度运行记录的事件页，也读不了存着长列表的实例事实，所以先换上读到 `runtime-client` 7、`ledger` 8 的控制台，再部署本版。
    **没有机器检查替你守这个次序**：控制台的边（`ui_edges`）只在门测试里算，根目录里有控制台成员以后才进计划（`plan.rs` 414–431）；生产根是单一位置根，里面没有控制台成员，
    `actingctl install` 不看控制台。
- **调度派的运行也能停下，结算成「停止」**（#429 R-6b，登记 §1.314 契约、§1.315 R-6b1、§1.316 R-6b2；`contracts/runtime-interface.md` 的 `StopInstanceRun` 一行，
  `contracts/policy-suspension.md` 的 “A stopped run”）。
  - **能停的**：`StopInstanceRun` 现在照停直接运行的办法停调度器派的运行（读数刷新的运行也算）：在下一个检查点以 `ClientRequested` 停，等法、`settled` 和 `reset` 都和直接运行一样。
    调度运行的控制块在读包之前就进登记表（`crates/runtime-host/src/host/contained_task.rs` 7936–7946），读包期间发来的停止也够得着，运行在第一个检查点结束。
  - **拒的**：只剩 Runtime 自己起的运行（启动包、恢复阶梯的一级、空闲动作的回主页歇），拒成 `scheduled_contained_task_not_client_cancellable`，`run_origin` 是 `host`
    （`contained_task.rs` 911–919 `stop_refused_origin`，`host/stop_instance_run.rs` 115–137 `stop_target`）；`run_origin` 的词 `policy` 还登记着，不再出现。客户端自己的取消
    （`CancelContainedTask`：`ac_stop_run`、actinglab `run stop`）照旧拒每一次调度运行。
  - **复位**：Runtime 用这次运行自己的租约，在终态和租约结束之前复位设备（一条挂在这次运行名下的 `InputReset`）；复位失败只围栏这一台（#419），停止答 `reset: failed`。被停的运行
    自己的终态不起阶梯，不管 `recovery_ladder.triggers` 列了什么；复位失败留下的围栏照所有围栏的规矩，由租约清扫交给 `resource_fence` 阶梯或交给人（`contained_task.rs` 8614–8707）。
  - **终态不变**：仍是 `task.failed` 带 `contained_task_cancelled`（同一段 8631–8635），`actingctl runs`、`ac_runs` 里显示 `failed`；新的是结算记录 `policy.execution_recorded`
    的种类 `stopped`。
  - **怎么结算**：既不是成功也不是失败。不记失败、不进连败、不退避、不进恢复阶梯、不暂停敏感任务、不算条件终止（不管 `scheduling.rerun.condition_codes` 列了什么）、不欠重跑，
    也不会被改判成超出时长预算；准入时扣掉的次数不退，运行时长照成功那样计入窗口；这次派单服务过的 Runtime 自己的重跑（立即重跑的那一轮、欠着的重跑、补救重试）随它结束
    （`crates/runtime-host/src/policy_control.rs` 1530–1731 `commit_paced_execution`、1740–1760 `preview_stopped_settlement`；运行中结算 `host/policy_outcome.rs` 1775–1786）。
    四条结算事实不动，仍是这一对上一次成功或失败的；任务快照显示 `cancelled`；例行汇总把它算在「打断」里。
  - **重启以后**：账本里有这次运行的终态（`task.failed` 带 `contained_task_cancelled`），就按账本的恢复规则结算成 `stopped`（`crates/ledger/src/global/storage.rs` 1681–1709，
    `host/policy_outcome.rs` 2845–2852）；没有终态的照旧按中断结算。
  - **停下只停这一次**：冷却从派单起算，触发条件还成立的，冷却一过就再派（被停的那次跑得比冷却还久时，下一轮评估就派）；时钟档位算已经服务过，`once_per_reading` 的读数不算。
    不想让它再跑，用下面的任务包开关关掉它，或者暂停这一台。
  - **怎么发**：`actinglab task stop <实例> <请求号> [--wait <时长>]`（实例也可以写 `--instance <实例>`，也能经 `serve --stdio` 调；`crates/lab-ops/src/core_ops.rs` 839 起），回答照旧是
    `InstanceRunStopped {stopped: {request_id, settled, reset?}}`、成功码 `request_stop_instance_run_completed`。**MCP 没有发它的工具**：`ac_stop_run` 发的是客户端自己的取消，照旧拒
    调度运行；智能体要停调度运行，用 actinglab 的这一行。监控台在调度运行上用它是之后的一片，现有的监控台还不用（见配对）。
  - **顺带变了的一处**：调度运行读包期间就在登记表里，所以暂停一台时的排空现在也看得见正在读包的调度运行，到了排空期限照样请它停，它在第一个检查点以 `contained_task_paused` 结束；
    以前要等读完包才看得见。
  - **生产上**：自 2026-11-11 起有一台实例在试跑调度，其余实例暂停着。本版上线以后在那一台上停下一次调度运行，就写下第一条 `stopped` 记录；从这一刻起，读不到修订 8 的控制台和工具
    读到那一页就失败。
- **一台实例上单独开、关一个任务包**（#429 R-8，登记 §1.308；`contracts/instance-fact-store.md` 的 “Pack schedule facts”，`contracts/scheduling-blackout.md` 的第 5 位）。
  - **命令**：`actinglab pack-schedule <实例>` 读（`PackScheduleView`），`actinglab pack-schedule set <范围> <包> --rows <json> [--expected-version <n>]` 改（`EditPackSchedule`；
    `core_ops.rs` 867–960）；范围写成一个词：`instance:<实例>`、`server:<区服 id>`、`game:<游戏 id>`。没有 `actingctl pack-schedule`，MCP 也没有这个工具。两个请求以前答
    `not_implemented`，现在有了各自的成功码 `request_pack_schedule_view_completed`、`request_edit_pack_schedule_completed`。
  - **行**：`{package_id, from_unix_ms, until_unix_ms?, schedulable}`，不写 `until_unix_ms` 就一直有效。结束不晚于开始、同一个包的行重叠、形状不对、一个范围超过 256 行（这个键自己的
    上限，不随 S168-LB1a 升到 1024），都拒成 `session_pack_schedule_invalid`（带 `fact_key` 和指进请求的 `field`），不致命，什么都不追加。`--rows '[]'` 撤掉这个包在这个范围的行。
  - **比较再写入**：在事实写入闸下读出这个范围的整张表，比 `expected_version`（不写就不比；0 表示这个范围还没有表；别的值必须等于这张表那条 `fact.published` 的序号，不等就拒成
    `fact_observation_version_conflict`，带 `current_version`），只换这个包的行，整表检查，写回成一条 `fact.published`，留着写的人的来源（`crates/runtime-host/src/host/pack_schedule.rs`
    37–100）。同一毫秒里对同一个范围改出不同的表，第二次拒成 `fact_observation_not_newer`，重发一次就行。
  - **效果**：每个包单独看，实例、区服、游戏三层里有这个包的行的最具体那一层说了算：那层有盖住此刻的行就照它，没有就是可调度（`crates/runtime-host/src/pack_schedule.rs` 388
    `resolve`）。关着的包，它的候选排在免打扰时段和应用更新之后推迟，理由 `pack_schedule_closed`（`package_id`，`next_eligible_unix_ms` 是这一行的结束，一直有效的行不带），到点自己醒
    （`crates/runtime-host/src/policy_host.rs` 3347–3357）。评估和准入之间有人改了表，准入以 `policy_facts_stale` 拒掉。行里点名、但这台的现行目录里没有任务在用的包，每个评估过的
    周期报一次 `pack_schedule_unknown_package`。
  - **正在跑的那一次不停**：关包只管之后的派单；要连这一次一起停，再发上面的 `task stop`。
  - **没人写表之前什么都不变**：生产上没有这个键的记录，每个快照身份和决定都和以前一样；`pack-schedule <实例>` 列出这台每个任务包，都是可调度、来源 `none`、三层的版本都是 0。
- **无进展护栏只挡它服务过的那个原因**（#428 SS-4b，登记 §1.310；`contracts/scheduling/README.md` 的 “No-progress guard”）。
  - **放掉的**：要压住一对（任务、实例）之前，把触发条件照武装时的读法再读一遍，得到「现在让它成立的读数键」：一个都没有（只靠时钟档位、时间线、结果或依赖成立），或者有一个键
    不在护栏的依据里，就是别的原因：不压，护栏放掉、不再回来（`crates/policy/src/evaluator/no_progress.rs` 140–197 `hold`、`serves`；Runtime 每次评估之后丢掉放掉的护栏，
    `policy_host.rs` 3557 `forget_released`）。所以因为一个完成时刻被压住的链头，到下一个时钟档位、或者另一个完成时刻到了就会跑，链跟着走；时钟档位定的派单不武装新护栏。
  - **还压着的**：跑了什么都没变、又因为同一批读数到期的一对，每一轮照旧压着，`all` 里读数旁边并排一个时钟的也一样。
  - **带 `reserves` 的任务是边界**（登记 §1.310 的 (a)）：预留闸读的那几个键（池的读数和上限）总在依据里，所以声明了预留、只靠时钟到期的任务，到了时钟档位照旧压着，要等池的读数变；
    这保住了 SS-2d 第 16 条测试钉着的防线。资源仓的排程文件眼下没有规则写 `reserves`。
  - 护栏照旧只在内存里，没有新码、新值、新键。
- **长列表**（#428 S168-LB1a、S168-LB1b，登记 §1.309 契约、§1.311、§1.312；`contracts/instance-fact-store.md`、`contracts/resource-readings.md`，`contracts/selection-policy.md` 的
  “Lower bound of a list maximum”）。
  - **行数上限**：一条 `record_list` 事实最多 1024 行；「一次观察、一份快照、一个视图最多 256 条记录」不动，字节上限（一次观察 512 KiB）也不动，没有配置键。列表读数扫到第 1025 个
    位置才停（`list_rows_exceeded`，以前是第 257 个）；整份读完但超过 512 KiB 的，算这份列表没读到（`reading_problem` 的新词 `list_bytes_exceeded`，带读到的行数），运行照常结算，
    原来那条记录留着（`host/contained_task.rs` 377、6275）。
  - **调度器不读列表**：`inventory.list.` 下的记录不进调度评估的事实（`crates/runtime-host/src/fact_store.rs` 1680–1691），所以列表发布不再让等着准入的派单判成事实过期；目录里有
    成员点名这种键来读，编译时在那个键上拒成 `type_mismatch`（`crates/policy/src/validation.rs` 3188 `validate_read_fact_key`；资源仓的 main 里没有这样的目录）。别的键下超过 256 行的
    记录列表，只对它适用的实例留在评估之外：读它的谓词是未知，也不为它发读数刷新；每条这样的记录每次启动后报一次 Warning `policy_fact_rows_exceeded`（`fact_key`、`count`；
    `host/policy_dispatch.rs` 3829–3869），别的实例照常（#419）。
  - **按前缀读一台实例的事实**：`InstanceFactSnapshot` 多可选的 `key_prefix`（非空、最长 192 字节、不含控制字符），Runtime 先筛、再建视图、再量帧的大小，所以整份放不进一帧的实例，
    按放得下的前缀照样读得出；回答的 `records_total` 是筛之前的条数，只在带前缀时给（`host/instance_fact_read.rs` 25–57、93–120）。`actingctl facts --instance <实例> --key-prefix <前缀>`
    和 `ac_facts` 都把前缀交给 Runtime（以前是读回整份再在客户端筛）。
  - **下界**：`table_max_over_list` 可以写 `lower_bound_column`：同一张表里另一个已声明的整数列，由做表的人给「数值还不知道的项」填下界（`crates/selection-policy/src/schema.rs`
    1230–1270）。写了它的引用读成精确的 M 或「至少 L」，什么都不知道时才是未知（`list_max.rs` 152 起 `read_list_maximum`）；比较只在 [L, +∞) 里每个值都给同一个答案时才定，阈值要 L
    够到门槛才给分，原值项取 L，上界估算当它没有上端。`selection_table_list_max` 多 `bound`（词表 `list_max_bound`：`exact`、`at_least`）和 `unknown_count`，读成「至少」时用
    `lower_bound`、`lower_bound_identity` 代替最大值和它的键；一个决定里有门或项靠下界定下来，就多一条 info `selection_decided_on_lower_bound`（`rule_id`、`count`），一个决定一条。
    判断步骤怎么用下界（LB1-12）归 S168-5b，不在本版。
  - **没有包读超过 256 个位置的列表、没有文档写 `lower_bound_column` 之前，什么都不变**；生产上有 `inventory.list.` 记录的实例，调度读的事实少了这些记录，决定不变。
- **命令行**（#434 CLI-EV、CLI-IN、CLI-HELP、CLI-COMP，登记 §1.306、§1.307、§1.313、§1.318；`contracts/lab-two-entries.md` §9.3）。
  - **`events` 不给位置时答最新的**（CLI-EV）：`actinglab events`、`actingctl events`、`ac_events` 不写 `after_sequence`、游标、`through_sequence`、`since_unix_ms`、`until_unix_ms`、
    `run_id`、`request_id`、`correlation_id` 时，答过滤条件匹配到的最新 `limit` 条，页内从旧到新（以前是从序号 1 起最旧的几条）。读法有界：先读一次账本头，再在头的快照上往回找，
    窗口三档 256、4096、65536 个序号（`crates/lab-ops/src/events.rs` 121 `NEWEST_WINDOWS`），不带 `wait_s` 时一次调用最多 23 次分页读。回答多 `first_sequence`（往前翻就用
    `through_sequence` = `first_sequence − 1`）和 `searched_from_sequence`（没找满又没搜到账本开头时说搜到哪）。**以前靠「不给位置」从头翻完整本账的脚本，要改成给 `after_sequence` 0**；
    给了位置或窗口的读法照旧。
  - **`--instance` 写在哪个命令上都认**（CLI-IN）：actinglab 操作表里有位置实例的 34 行（CLI 模块必填 17 行、可选 13 行，加 `app launch`、`app restart`、`app stop`、`recover`），都可以
    改写成 `--instance <实例>`，放在命令词前面、后面、中间都行；位置写法照旧。两种写法同时出现拒成 `repeated`，什么都不发（`apps/actinglab/src/cli_module/words.rs` 156–180）。
    actinglab 同名自有命令的 20 行照旧只收 `--instance`；`remedy set`、`wake-test` 照旧当自己的选项读；`standard-pack compose` 的 `--instance <别名>=<游戏>/<区服>` 是绑定、不是实例，
    不变。例外：`targets set <x> --instance <y>` 里的 `<x>` 读成文件。actingctl 不变。
  - **人能读的帮助**（CLI-HELP）：`actinglab help`（和只敲 `actinglab`）不带 `--json` 打出用法、全局选项和这一版提供的每条命令各一句；`actinglab help <命令>`、`<命令> --help` 打出这一条；
    `help <组>`、`<组> --help` 列这一组（actinglab 自己的组也可以写 `<组> help`）。**一行里任何位置写了 `--help` 都只打帮助、什么都不执行**（退出码 0；
    `apps/actinglab/src/cli_help.rs` 410 `asked`），以前是被拒。`--json help` 原有成员一个不少，多一个 `list`。`status help` 照旧把 `help` 当实例名。
  - **按 Tab 补全**（CLI-COMP）：`actinglab completion powershell|bash` 打印一段脚本，要用的人自己加进 shell 的配置，actinglab 和安装工具都不改配置文件。每种 shell 两行（第一行这一次就开，
    第二行以后每次都开）：
    - PowerShell：`actinglab completion powershell | Out-String | Invoke-Expression`，`Add-Content $PROFILE 'actinglab completion powershell | Out-String | Invoke-Expression'`；
    - bash：`source <(actinglab completion bash)`，`echo 'source <(actinglab completion bash)' >> ~/.bashrc`。
    补的是命令词、`help` 后面的词、一条命令的选项和开关、枚举的取值、这一行能带的全局选项和实例别名。**在实例的位置按 Tab 会问一次正在跑的 Runtime**：握手、绑定通道和一次 `Status`，
    都不记账，默认最多等 500 毫秒（`--wait-ms`；`apps/actinglab/src/cli_completion.rs` 59–61、742 起）；没有 Runtime 答，就读选中的配置文件里的别名。回调 `__complete` 从不往提示符里
    打错误，总是退出 0。命令清单从 100 条变成 102 条，顶层分派臂从 45 个变成 47 个。
- **分阶段的包当层跑时，从入口页所在的阶段起跑**（#428 NAV-L1，登记 §1.319；`contracts/README.md` 的 “Entry page sets”）。宿主把一个分阶段的包当层来跑（前置、导航边、共用打断的
  处理包、回主页包、入口恢复包、恢复链的一步）时，它看到的第一页是这个包的入口页，就从这一页所在的阶段起跑，和它自己跑、和线下模拟走同一条路；第一页不是入口页的，照旧从第一阶段起
  （`crates/execution-kernel/src/contained_task.rs` 5121–5134）。接受哪些第一页、等多久、各种上限和码都不变；普通运行、声明操作的运行、不分阶段的包和只有一个入口页的包，决定逐字节不变。
  所以某个阶段不必再为每个入口页都备一步。
- **「运行状态不对」这个码说清楚是哪里不对**（#428 DIAG-SI，登记 §1.320）。`contained_task_state_invalid` 的类别、阶梯分类和每一处结束都不变，结果多 `state_problem`（新词表：
  `no_move_from_page` 是包作者的问题，`transition_refused`、`runtime_invariant`、`held_state_unusable` 是 Runtime 自己的问题）和发出处知道的页 `page_id`、操作 `pack_operation_id`、
  阶段 `phase_id`（新键）、包 `package_id`；运行的终态、回执和层的 `task.entry_preflight` 记录都带上，只给码的那一档保留 `state_problem`。事故那一类读作
  `{"code":"contained_task_state_invalid","category":"error","values":{"operation":"run_contained_task","state_problem":"no_move_from_page","page_id":"neutral/busy","phase_id":"first"}}`：
  给 `phase_id` 那个阶段加一步离开 `page_id` 那一页，或者把那一页设成目标，当层用的再把它连同起跑的阶段列进入口页；另外三种带着运行的记录来报。
- **核心的测试和夹具不再有游戏缩写**（#434 NEUT-2，登记 §1.317）：只改测试夹具和一道守卫（架构测试 `neut2_source_and_test_trees_hold_no_game_abbreviation`，门表 A4 行），产品不变。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`、`instance add`。
  `crates/lab-rescue` 自 v0.12.54 只改了救急模块的用法文字和一个钉 `runtime-client` [3, 7] 的配对测试，`actingctl install` 走的代码就是 v0.12.54 说明核过的那份。

配置：三处升，都按代码、`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md` 核过。`component-interfaces.json` 是 `actingd-config` [5, 28]、
`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 8]、`runtime-client` [3, 7]、`package` [3, 9]、`runtime-outcomes` [1, 1]；架构测试
`tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120`（136–144）随之钉这三处，救急模块的配对测试 `gate_429_r0b_pairing.rs` 钉 `runtime-client` [3, 7]。

- 升 `ledger` 一次，两片共用（S168-LB1a 先合、升到 8；R-6b 并进去，不再升）：理由见上。`crates/ledger` 只改了账本自己的恢复规则（`storage.rs` 1681–1709：只有终态是 `task.failed`
  带 `contained_task_cancelled` 的调度运行才恢复成 `stopped`）和帧保留把 `stopped` 记录同它的终态配对（`retention.rs`）；没有新记录类型、表或索引。LAB-6 和 S168-5b 计划里的 `ledger` 8
  不在本版，做的时候取下一个空号，含义不变。
- 升 `runtime-client` 一次，两片共用：`crates/runtime-client` 多 `instance_fact_snapshot_by_prefix`，运行摘要读得出 `stopped`（`client.rs`）；工作台的请求和回答都没改形状，
  `StopInstanceRun`、`InstanceRunStopped` 原样。LAB-6 计划的 `runtime-client` 7 同上。
- 升 `package` 一次（S168-LB1b）：选择文档没有自己的 `requires`，包声明的范围就是门；S168-5b 计划的 `package` 9 同上。S168-LB1a 不升 `package`：行数上限是 Runtime 的读法，不是包声明的成员。
- 不升 `actingd-config`：没有新键。`runtime-outcomes` 仍是 1：新码、键和词表都在这一修订里追加。`install-selection`、`install-control` 不变。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`apps/actingd`、`distribution/windows/actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.54 都没改，
  从 v0.12.54 算，部署计划的默认值对比是空的。`config.subsystems` 不变：`apps/actingd/src/config/manifest.rs` 没改，S168-LB1b 不加 `requires` 的词（`crates/policy/src/compose.rs` 没改）。
- 结果码：目录从 3908 个码变成 3912 个码，位置仍是 1750 个，仍是 29 个分片，键从 629 个变成 635 个，词表从 221 个变成 223 个，没有退役或定稿的码。新码 4 个（都待定稿，since 0.12.55）：
  `request_edit_pack_schedule_completed`、`request_pack_schedule_view_completed`（contract，success，ladder `none`）、`policy_fact_rows_exceeded`（runtime-host，warning，`none`）和
  `selection_decided_on_lower_bound`（selection-policy，info，`none`）。R-8 第一次发出的 `pack_schedule_closed`、`pack_schedule_unknown_package` 和 `session_pack_schedule_invalid` 是早就登记的码。
  新键 6 个：`bound`（词表 `list_max_bound`）、`unknown_count`、`lower_bound`（都是 `integer`）、`lower_bound_identity`（`id`）、`state_problem`（词表 `state_problem`）、`phase_id`（`id`）；
  新词表 2 个：`list_max_bound`（`exact`、`at_least`）、`state_problem`（`no_move_from_page`、`transition_refused`、`runtime_invariant`、`held_state_unusable`）；词表 `reading_problem`
  多一个词 `list_bytes_exceeded`。改了的：`contained_task_state_invalid` 多五个可选值 `state_problem`、`page_id`、`pack_operation_id`、`phase_id`、`package_id`，说明、常见原因和处理从待填
  写成按词的写法；`selection_table_list_max` 多四个可选值 `bound`、`unknown_count`、`lower_bound`、`lower_bound_identity`；只改说明的：`scheduled_contained_task_not_client_cancellable`、
  `request_stop_instance_run_completed`、`policy_no_progress`、`fact_observation_version_conflict`、`session_pack_schedule_invalid`、`contained_task_resource_reading_unresolved`，键
  `run_origin`，词表 `run_origin` 和它的两个词，`reading_problem` 的 `list_rows_exceeded`。本版的发布文件是空的：没有新定稿或退役的条目。
- 程序文件：workspace 仍是 39 个成员，`Cargo.toml`、`Cargo.lock` 自 v0.12.54 都没改，没有新的依赖；`.github/` 没改，两个包的文件清单和 v0.12.54 一样；`ci/` 只在
  `ci/retired-gates.toml` 多两条退役（R-0c 那条钉「视图未实现」的测试，R-6 第一片那条「调度运行被拒」的测试改了名）；`ratchet/actinglab_commands.json` 随 CLI-COMP 变成 47 个分派臂、
  102 条命令；`tools/` 只改了架构测试（修订的钉值、NEUT-2 的守卫）和 `identifier_words` 的可见性。`apps/actingctl` 只改了 `ac_events` 的说明和结果模式，`apps/mcp-entry` 没改，
  MCP 工具仍是 41 个。`contracts/lab-operations.json`：`pack-schedule`、`pack-schedule set` 两行开放（`reserved` → `planned`），`task stop` 的 `help`、`notes` 和 `events` 的 `help` 改了，
  有位置实例的行的参数说明多一句；`ui_kinds` 里 `StopInstanceRun` 的 `console_today` 仍是 `false`（发布了的监控台都还不发它）；救急模块提供的行仍是 30 行。不稳定表照旧是 FLK-08 和
  FLK-10，这几片都没动它，也没动它们读的测试和金样。

升级与退回（安装工具的代码 `crates/lab-rescue/src/install/` 自 v0.12.54 只改了一个测试文件 `gate_429_r0b_pairing.rs`；其中 `plan.rs`、`update.rs`、`undo.rs`、`programs.rs`、
`postcheck.rs`、`host.rs` 自 v0.12.51 都没改，在 v0.12.52、v0.12.54 和本版里逐字节相同，下面这几份的行号对三个版本的安装工具都对；`deploy.rs` 在 v0.12.52 里少 INST-5b 的几段，
行号是本版的，也是 v0.12.54 的）：

- **v0.12.54 的安装工具装本版：要带 `--accept-ledger-one-way`，不带就被拒。** 计划的边逐条（`plan.rs`）：
  - `ledger` `previous_max_in_next`（288–294）：Rp [3, 7] 的上限 7 在 R\* [3, 8] 里，过。`one_way`（295–301）：8 大于 7，**只有带了标志才过**；不带时这是唯一不过的边，计划以
    `interface_edge` 拒绝（1030–1041），什么都不改。带了以后计划的 `inputs.accept_ledger_one_way` 是 true，`notice.rollback_window_bounded` 是 true（203）；计划 id 是整份计划的哈希
    （966–992），所以预览和 apply 要都带。
  - `ledger_verify`（915）：两边的 `ledger` 不同，计划有这一步；apply 把它排在所有换文件的步骤之前（`update.rs` 528–538），跑目标构建里的
    `actingcommand-actingd.exe ledger-maintenance verify --config <根>\actingd.config.json`（`update.rs` 836–862，`programs.rs` 294–310），期限是 `install.child.ledger_verify_timeout_ms`
    （默认 1800000，`distribution/windows/actingd.defaults.json` 569）。现行账本只有修订 3 到 7 的记录，本版读得了。不过或到期限时什么文件都没换，但带 `--stop` 的部署这时 Runtime
    已经停了、hold 还在（和 v0.12.43 一节一样）。日志的 `acknowledged` 记下 `accept_ledger_one_way`（`update.rs` 700–704，`deploy.rs` 1720–1724）。
  - `runtime-outcomes` `next_max_in_installer`（302–314）：R\* 的 1 在安装工具的 [1, 1] 里，过。
  - `install-control`、`runtime-client` 的 `installer_meets_previous` 和 `installer_meets_next`（315–331）：安装工具的声明 I 是它自己构建里的 `component-interfaces.json`
    （`host.rs` 23–25），v0.12.54 的 `runtime-client` 是 [3, 6]，和 Rp [3, 6]、R\* [3, 7] 都相交；`install-control` 三方都是 [0, 1]。都过。
  - `package` `bundle_max_in_next`（332–355）：目标是 [3, 9]，本版以前没有包能声明 9，每个包声明的上限都落在里面，各包的结果和装 v0.12.54 时一样（没有声明的照旧是
    `pack_declaration_unknown` 警告）。`speaks` `runtime_equals_tools`（756–763）：两半的声明相同，过。
  - `actingd-config` `installer_knows_keys`（359–376、764；`unknown_keys` 506–536）：本版没有新键，现在的配置文件不写安装工具不认识的键，过。`next_check_config`（902–914）：本版的
    check-config（带 `--install-root`：28 ≥ 17）和 v0.12.54 是同一份 actingd 和配置代码，收现行的文件，过。
  - 计划的 `defaults_diff`（574 起）：默认值文件没改，从 v0.12.54 装是空的；部署后检查的 `new_keys`（`postcheck.rs` 494–526，`added_keys` 593 起）没有要核的键。
  - **操作员要做的**：`install update plan --root <根> --build <提交> --preview` 和 `install update apply --root <根> --build <提交> --plan <id> --stop --hold <理由> … --start --detach`
    都加上 `--accept-ledger-one-way`，apply 用带了标志的预览给的计划 id；预览里看得到 `ledger` `one_way` 这条边是过的、`ledger_verify` 是 true。
- **v0.12.52 的安装工具直接装本版**：代码同上，每条边的判法一样，同样**要带 `--accept-ledger-one-way`**，计划同样有 `ledger_verify`。它的声明是 `actingd-config` [5, 27]、
  `runtime-client` [3, 6]：`runtime-client` 和两边都相交；它的 [5, 27] 在更新计划里不和目标的 [5, 28] 比（三方的 `installer_ui_next_common` 只在门测试里算，`plan.rs` 430）；
  `installer_knows_keys` 拿它自己的键表比，生产的文件不写 v0.12.54 的新成员（v0.12.52 自己就拒），过；本版的 check-config 收修订 27 的文件。和从 v0.12.52 装 v0.12.54 一样，
  `defaults_diff` 有三行（两个空闲门槛，`source` 是 `default`，加 v0.12.53 带来的 `ledger_observer.tag.workbench_patterns`），部署后检查的 `new_keys` 核两个空闲门槛来源是 `default`；
  v0.12.53、v0.12.54 两节里部署时要核的同样要核。
- **退回：安装工具拒绝退回本版以下的任何版本，带不带标志都一样。** 本版声明 `ledger` [3, 8]，v0.12.54、v0.12.53、v0.12.52 都声明 [3, 7]：`rollback plan`、`rollback apply` 的
  `previous_max_in_next` 要现行构建的 Rp.max 落在目标的 R\* 里，8 不在 [3, 7] 里，这条边不过，在算回退门之前就以 `interface_edge` 拒绝（`plan.rs` 288–294、1030–1041），不过的只有这一行，
  什么都不改；`--accept-ledger-one-way` 只放行 `one_way`（295–301；退回方向 7 不大于 8，这条本来就过）。回退门的第 1 部分（接口边，`update.rs` 393–399）同样不过。
  - **部署的自动回退也走不了**：新 Runtime 起来以后部署后检查不过时，部署先按回退算一次计划（`deploy.rs` 763–854 的 `rollback_gate`，813 `accept_ledger_one_way: false`），同一条边
    不过，以 `rollback_gate_failed`（部分 `interface_edges`，803）结束，新构建和 hold 都留着。起过本版 Runtime 的操作，`install undo` 也按回退判（`undo.rs` 376–406，388 不带标志），
    同样被拒；没起过 Runtime 的未完成操作照旧能撤销。
  - **为什么写没写都一样**：安装工具只看两个构建清单声明的范围，账本里不记修订号，所以刚部署完、一条新形状的记录都还没写时，退回也一样被拒。
  - **旧版在这个根上会怎样**（只作说明，不是操作步骤）：写过超过 256 行的列表或停过一次调度运行以后，v0.12.54 及更早的 Runtime 启动时载入每条现行记录、核最新的
    `open_verify_tail_records` 条，碰到这种记录就拒（`fact_record_limit_exceeded` 或 `corrupt_ledger_record`），含它的页整页读不了，回退门的 `ledger_readability` 也不过；同版的
    actinglab、actingctl 和 MCP 读到这种页也一样失败。
  - **对操作员一句话：本版装上并跑过以后，同一个状态根上没有退回本版以下的路。** 剩下的是往前修（装更新的版本），或者换一个全新的状态根（空账本：之前的账本、事实、运行中设的指令、
    封顶回合和空闲状态都留在旧根上）。
  - **逐片还要知道的**：
    - **R-8**：关包的行是这个根账本里的事实（`session.pack.schedule`），只活在这个根上；更新的版本照读照用，换新状态根就没有了（每个包又都可调度），要保留就在新根上重写；不想留，先对
      每个包写一次 `--rows '[]'`。
    - **CLI-COMP**：补全脚本是人加进 shell 配置的那一行，不在状态根里；换成不认 `completion`、`__complete` 的 actinglab，按 Tab 只是不给候选（shell 照常补文件名），不要了就从配置里删掉
      那一行。
    - **S168-LB1b**：写了 `lower_bound_column` 的包要本版或更新的 Runtime。**R-6b、S168-LB1a**：就是上面说的账本 8 单向。**SS-4b、CLI-EV、CLI-IN、CLI-HELP、NAV-L1、DIAG-SI、NEUT-2**：
      什么都没落盘，没有要做的。
- 配对：
  - **安装工具**：见上；从 v0.12.54 装本版跑的是在线的 v0.12.54 的安装工具，从 v0.12.52 直接装跑的是 v0.12.52 的，两者都要带 `--accept-ledger-one-way`。
  - **部署工具包的命令行**：它跑的全是 `actingctl install` 的 `release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop --hold --known-pause …`、
    `status --root [--operation]`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause … --start --detach`、`release-hold --root --reason`，
    跑的是在线的安装工具（v0.12.54 或 v0.12.52），命令行代码和 v0.12.54 说明核过的一样（`gate_434_lab3.rs` 照旧逐条钉住）。**被拒的**：不带 `--accept-ledger-one-way` 的 `update plan`
    （连预览）和 `update apply`，都以 `interface_edge` 拒，不过的是 `ledger` 的 `one_way`。**答法变了的**（带了标志以后）：计划的 `ledger_verify` 是 true、步骤以 `ledger_verify` 开头、
    `notice.rollback_window_bounded` 是 true、`inputs.accept_ledger_one_way` 是 true，日志的 `acknowledged` 多这一项；`defaults_diff` 从 v0.12.54 算是空的（从 v0.12.52 算是三行）；
    `edges` 的行数和形状不变。`release stage`、`guards`、`status`、`release-hold` 答法照旧。工具包另用新版 actinglab 跑的 `--json standard-pack check --root <游戏根>` 同下。
    **工具包要跟着改的**：预览、计划、apply 三步都加 `--accept-ledger-one-way`；说明里「不带单向标志、没有 `ledger_verify`」和退回那一段不再对；它给热放置工具包写构建记录的那一步，
    核新包清单的 `actingd-config` 仍是 [5, 28]，`package`、`ledger` 现在是 [3, 9]、[3, 8]，要记成 `3,9`、`3,8`（热放置工具包重钉时拿这三个值和清单比，照旧的 `3,8`、`3,7` 会让重钉不过）。
  - **热放置工具包的命令行，本版都照旧收**：actinglab 两行，`--json standard-pack check --root <游戏根>`（S7）和 `--json standard-pack compose --root <包根> --instance <别名>=<游戏>/<区服>
    … --out <新目录>`（K3）：`apps/actinglab/src/standard_pack_cli.rs` 自 v0.12.52 没改；`compose` 的 `--instance` 是这一行自己的选项（它的类型是绑定，不是实例），CLI-IN 不碰它；两行都不带
    `--help`、也不以 `--instance` 开头，CLI-HELP、CLI-IN 都不改它们的走法。actingctl 的 `status`、`status --config`、`pause --instance <别名> --reason <理由> --drain-timeout-ms <毫秒>`、
    `resume --instance <别名>`、`schedule --instance <别名>`、`overview --root <根> --instance <别名> --sections catalog`（都带 `--state-root`）：`apps/actingctl` 的命令行代码本版没改。
    工具包读账本用的是 actingledger 的 `events --after … --through …`（`apps/ledger-forensics` 没改，而且给了位置），没有哪一行不带位置地读 `events`，CLI-EV 的新默认碰不到它。
    **没有一条被拒，也没有一条的行换了次序。** 答的内容上能看到的：`standard-pack check` **没有新的警告码**，回答照旧；`status --config` 的子系统行和参数行都不变；`schedule --instance`
    在有人写了任务包的表以后可能出现 `pack_schedule_closed`、`pack_schedule_unknown_package`，被护栏压住的 `policy_no_progress` 变少；`pause … --drain-timeout-ms` 的排空现在也等正在读包的
    调度运行，到期限时请它停。**热放置工具包要学的**：本版没有新的子系统、新的检查警告码或新的 `requires` 词，只是重钉时比的构建记录要是上面那三个值。从 v0.12.52 直接装本版的，
    v0.12.54 说明里要它学的（子系统 `consumption_fact`、`pool_trigger_line`，已知警告 `standard_pack_line_trigger_redundant`）照旧要学。
  - **控制台 UI**（最新发版 UI v0.12.15 的提交是 `bb0fd1c7`，把 Runtime crate 钉在 `d06910c6`，即 v0.12.47；UI 的 main 已到 `8d2c8310`、还没发版，钉在 `59fa27f0`，即 v0.12.53；
    `Cargo.toml` 的 `[workspace.dependencies]`）：
    - 两者声明的都是 `actingd-config` [5, 27]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]（`crates/acui-setup/component-interfaces.json`），**都读不了本版**：
      `ledger` 8 不在 [3, 7] 里（照边算 `next_max_in_ui` 不过）；它们钉的合同 crate 解不开 `stopped` 这种结果，也解不开超过 256 行的列表记录。在线时它用 `ui` 投影读事件页，含被停调度
      运行记录的那一页以 `runtime_receipt_decode_failed` 失败；离线用它钉的账本 crate 读，同一页也读不了。`runtime-client` [3, 5] 和本版的 [3, 7] 照边算是相交的，但这不说明它读得了。
    - **钉在本版以前的控制台，在有调度运行被停过以后就不能再对着本版用**；部署本版之前先换上钉 0.12.55 或更新、`runtime-client` 读到 7、`ledger` 读到 8 的控制台。眼下两个版本都还不是。
    - **它自己的 README 怎么说**：main 的 README 说它适配 Runtime v0.12.47 到 v0.12.53（v0.12.15 的说适配 v0.12.47 和 v0.12.48），能不能配看两边声明的接口、不看版本号，写者的 `max`
      要落在读者的范围里；部署的先后它没有写。照它自己的这条规矩，它和本版不配。
    - main 的「停下这次运行」只对直接运行发 `StopInstanceRun`，调度运行上按钮是灰的；v0.12.15 没有这个按钮。两者都不发 `EditPackSchedule`、`PackScheduleView`，任务列表照旧说单个任务还
      不能开关。
  - **MCP**：`ac_events` 和 `actingctl events` 一样，不给位置时答最新的；`ac_facts` 把前缀交给 Runtime，对修订 6 的 Runtime 发带前缀的请求会被断开（部署切换的那几秒里才碰得到）。

运行上的变化：

- 部署是单向的：带 `--accept-ledger-one-way`、多一步 `ledger_verify`，装上就退不回本版以下；先换读到修订 7、8 的控制台。部署本身不改变派单。
- 调度派的运行可以用 `actinglab task stop` 停下：设备复位，结算成「停止」，不算失败；触发条件还成立的，冷却过后再派，要它不再跑就用任务包开关或暂停。
- 可以在一台实例上关掉一个任务包（`actinglab pack-schedule set`），下一轮评估起推迟，正在跑的那一次跑完。
- 被无进展护栏压住的链头，到下一个时钟档位或另一个完成时刻就放；超过 256 行的列表读得出来。
- actinglab：`events` 默认答最新的，`--instance` 处处可写，`help` 和 `--help` 给人看，可以按 Tab 补全。

本版合并的 PR：

- #1606 CLI-EV（#434，登记 §1.306；经列车 #1608）：`crates/lab-ops/src/events.rs` 的最新页读法、`first_sequence`、`searched_from_sequence`；apps 门行 `#434 CLI-EV` 的 5 个测试
  （actinglab 4 个，actingctl 和 `ac_events` 同页的 1 个），改造 `mcp_shell.rs` 的两个辅助函数和一处调用（明写从 `after_sequence` 0 起，牵动 5 条旧测试）。没有新码。
- #1604 CLI-IN（#434，登记 §1.307；经列车 #1608）：`apps/actinglab/src/cli_module/words.rs` 的规则、`route` 认开头的 `--instance`、操作表实例参数的说明；apps 门行 `#434 CLI-IN` 的 4 个
  测试，列车里改了它自己的一条（`targets set` 的短写法读成文件）。没有新码。
- #1607 R-8（#429，登记 §1.308；经列车 #1608）：`crates/runtime-host/src/pack_schedule.rs`、`host/pack_schedule.rs` 和 Lab 的两行核心行；主机门行 `#429 R-8` 的 9 个测试和 actinglab 的
  1 个，改造 3 处（两条钉「没实现」的测试和契约的成功码普查），R-0c 那条测试退役（`ci/retired-gates.toml`）；列车里删掉比两台宿主快照身份的那一句，测试改名
  `r8_without_a_record_every_decision_is_unchanged`。新成功码 2 个。
- #1603 S168-LB1（#428，登记 §1.309；经列车 #1608）：只有契约，长列表和下界的规矩与十问的答复；没有代码和测试。
- #1608 合并列车 A：带进上面四个，登记号改成 §1.306 到 §1.309；R-8 和 S168-LB1 在 256 行那一句、R-8 和 CLI-IN 在 `pack-schedule` 的实例说明上各对成一句；契约多一句同一毫秒的改动答
  `fact_observation_not_newer`。
- #1609 SS-4b（#428，登记 §1.310；经列车 #1614）：`crates/policy/src/evaluator/no_progress.rs` 的 `hold`、`serves`、`forget_released`；策略门行 `#428 SS-4b` 的 4 个测试；没改现有测试。
- #1613 S168-LB1a（#428，登记 §1.311；经列车 #1614）：1024 行、调度器不读列表、`policy_fact_rows_exceeded`、`list_bytes_exceeded`、`key_prefix` 和 `records_total`；契约 crate 3 个、策略
  2 个、宿主 5 个加 1 个测试，改造 3 处（内核钉第 257 个位置的改成第 1025 个，救急模块钉 `runtime-client` 的改成 [3, 7]，MCP 的一条补新成员）。`ledger` [3, 8]、`runtime-client` [3, 7]，
  新码 1 个、新词 1 个。
- #1611 S168-LB1b（#428，登记 §1.312；经列车 #1614）：`lower_bound_column` 和它的读者；策略门行 `#428 S168-LB1b` 的 10 个测试，改造架构测试的修订钉值。`package` [3, 9]，新码 1 个、
  新键 4 个、新词表 1 个。
- #1612 CLI-HELP（#434，登记 §1.313；经列车 #1614）：`apps/actinglab/src/cli_help.rs`；apps 门行 `#434 CLI-HELP` 的 5 个测试；没改现有测试。
- #1610 R-6b（#429，登记 §1.314；经列车 #1614）：只有契约，「停止」这种结算和八问的答复；没有代码和测试。
- #1614 合并列车 B：带进上面五个，两片的修订在接口声明和架构测试里各对成一行，`component-interfaces.md` 里 `package` 9 那一句改成实情。
- #1616 R-6b1（#429，登记 §1.315；经列车 #1618）：`stopped` 的形状、检查、决策记忆、投影、账本的恢复规则和各个读者；契约、账本、主机、apps 四张门表 `#429 R-6b1` 的 19 个测试，
  6 个按结果种类分支的测试辅助补上第三种，断言没动。
- #1615 NEUT-2（#434，登记 §1.317；经列车 #1618）：actinglab 测试 8 个文件 195 行夹具换成中性的值，守卫 1 个（门表 A4 行）；断言和金样没动，没有产品代码。
- #1617 NAV-L1（#428，登记 §1.319；经列车 #1618）：内核一处；执行门行 `#428 NAV-L1` 的 4 个测试；没改现有测试。
- #1618 合并列车 C：带进上面三个，只解登记和门表的冲突。
- #1619 CLI-COMP（#434，登记 §1.318）：`apps/actinglab/src/cli_completion.rs` 和两条新命令；apps 门行 `#434 CLI-COMP` 的 6 个测试和 1 个进程级测试，改造 CLI-HELP 那条测试的一个数
  （34 → 36）。没有新码。
- #1620 R-6b2（#429，登记 §1.316）：停调度运行、复位、运行中结算和重启结算；宿主 8 个、策略控制 2 个、策略 2 个测试（门行 `#429 R-6b2`），改造 2 个（R-6 第一片那条改名、进退役表；
  R-8 那条的第二步），两个测试辅助改名。不升修订，没有新码。
- #1621 DIAG-SI（#428，登记 §1.320）：`state_problem`、`phase_id` 和 56 处发出点；内核 4 个、宿主 1 个测试（门行 `#428 DIAG-SI`）；没改现有测试。新键 2 个、新词表 1 个。

## v0.12.54

本版是 v0.12.53 之后合并的 4 个 PR：合并列车 #1599 带进的 #386 IDLE-2（#1597）和 #428 SCHED-C1e（#1598），以及 #434 NEUT-1（#1600）和 #428 SCHED-C1f1（#1602）。
先说部署：**`actingd-config` 从 [5, 27] 升到 [5, 28]，两片只升这一次**；`ledger` 仍是 [3, 7]、`runtime-client` 仍是 [3, 6]、`package` 仍是 [3, 8]，部署不用带
`--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步。**v0.12.52 的安装工具可以直接装本版（跳过 v0.12.53），不用任何标志，也不被拒**（逐条边见下面的升级与退回）；
装上以后安装工具允许退回**本版替换的那一版**，不用标志，但要先把文件里和运行中设的本版新东西去掉（见退回）。新键是两个空闲门槛（整节和每台）和每台的
`scheduling.pool_lines`（没有默认值）；默认值文件多这两个门槛；`config.subsystems` 多一行常开的 `consumption_fact`。从 v0.12.52 直接装本版的，v0.12.53 说明里部署时要核的
`ledger_observer.tag.workbench_patterns` 那一处同样要核。运行上要知道的是这几处：

- **配置修订只升一次，两片共用**（IDLE-2、SCHED-C1e；`contracts/component-interfaces.md` 的修订 28 那一行，`distribution/windows/component-interfaces.json` 4）。
  - 修订 28 是 27 加三样：`idle.action` 的值 `auto`（整节和 `instances[]` 那一项的 `idle.action`）；`idle` 一节和那一项 `idle` 成员的 `return_home_min_gap_ms`、
    `stop_emulator_min_gap_ms`；`instances[]` 那一项 `scheduling` 成员的 `pool_lines`。
  - **不写这些的文件修订不变**，本版照读（仍读修订 5 到 27）；生产现在的文件不用改。
  - **本版以前的版本拒收写了其中任何一样的文件**（v0.12.53、v0.12.52 在启动和 `check-config` 都拒：未知的值、未知的成员）。所以先部署本版、再往文件里写；退回前先从文件里去掉。
  - `host.idle_state` 绝不记 `auto`（记的是实际取的档），所以账本修订不动。
- **空闲动作 `auto`**（#386 IDLE-2，登记 §1.304；`contracts/runtime-config.md` 的 “The `idle` section”）。
  - **怎么选档**：空闲检查发现一台实例什么都不欠（没有租约、排队、无钥匙的活，没有暂停、玩家占用、黑名单时段、围栏，模拟器开着，上次活之后做过计划且没有到期的任务）时，
    读它到下一件事的间隔（下一个任务的唤醒或读数刷新，取早的，和今天 `idle.min_gap_ms` 的读法一样），直接取一档，恰好等于门槛时取从那里开始的一档
    （`crates/runtime-host/src/idle_action.rs` 200–213 `auto_tier`）：
    - 不到 `idle.return_home_min_gap_ms`（默认 600000，10 分钟）：不动；
    - 从它起、不到 `idle.min_gap_ms`（默认 1800000）：回主页歇一次；
    - 从 `idle.min_gap_ms` 起、不到 `idle.stop_emulator_min_gap_ms`（默认 7200000，2 小时）：`close_app`，和今天的 `close_app` 完全一样，带它的唤醒；
    - 从 `idle.stop_emulator_min_gap_ms` 起，或者没有下一件事：`stop_emulator`，和今天一样，带它的唤醒；模拟器不归 Runtime 启动（`start_emulator` 关）的实例只到
      `close_app`，不算错。
    - 档位还受「回得来」限制（`IdleView::auto_cap`，同一文件 163–171）：没有启动包的实例不关应用也不关模拟器，但仍可以歇（歇不需要回来）。
  - **回主页歇一次是什么**（`crates/runtime-host/src/host/idle.rs` 1204 起 `carry_out_idle_rest`）：在空闲动作的那把模拟器控制钥匙上，把这个游戏的标准包为这台实例的区服
    声明的回主页包（`bundle.json` 里 `uses` 含 `return_home` 的维护项，读法和恢复阶梯、前置链相同，资源仓不用加新声明；游戏和区服取实例的策略身份，和 `close_app`
    关的应用一样）作为宿主自己的一次运行跑**一次**（`HostPackageRun::IdleReturnHome`），响应期限是生效的 `idle.return_home_min_gap_ms`（最多一个任务的最长时限），所以
    在下一个任务到期前跑完。跑完记一条 `instance_idle_action`（`operation` `run_idle_action`、`idle_action` `return_home`，不带 `at_unix_ms`，之后没有唤醒）。不论怎么
    结束，这一段空闲都算歇过：不欠唤醒，`host.idle_state` 里没有它的行，有活（租约、排队、无钥匙的活）来之前不再歇；后来的计划间隔更长，就照常从它往上取档。
  - **不是什么**：不进恢复阶梯，失败不在同一段里重试（只记宿主运行的失败记录，类别 `run_idle_action`），不停实例也不停 Runtime；暂停、玩家占用、黑名单时段、围栏挡着时
    不歇，和别的空闲动作一样；歇的钥匙排不上、或者碰上没有声明的包，也算这一段歇过；碰上暂停或排队的请求就不执行，实例回到活动状态。adb 没就绪报
    `startup_package_adb_not_ready`，和别的不进阶梯的宿主运行一样。
  - **两条警告**（都是已有的码 `instance_idle_action_unsupported`，不加新码）：没有启动包的实例，`idle_action` `auto`、`member` `startup_package`，和别的动作一样报一次；
    游戏没为这个区服声明回主页包（或实例的策略身份没有游戏和区服）时，`idle_action` `return_home`、`member` `maintenance`、`operation` `run_idle_action`，每次启动每台一次，
    不歇，上面两档不受影响。处理：在标准包里给这个区服声明回主页包，或者把 `idle.return_home_min_gap_ms` 提到 `idle.min_gap_ms`，就没有间隔落在歇这一档。
  - **记录里只写实际取的档**：`host.idle_state` 只会是 `close_app` 或 `stop_emulator`，绝不是 `auto`（形状不变，旧版照样读）；歇过的实例开着、没有行。`auto` 下「动作变了、
    立刻叫醒」指的是它取的那一档不再被允许（启动包没了，或 `start_emulator` 关了），不是配置的词变了，所以重启恢复出来的已关应用不会立刻被叫醒；启动时读不回空闲状态的
    `auto` 实例，按它能取的最高一档关着算（`idle_action.rs` 838 起 `wake`，`host/idle.rs` 的 `restore_idle_state`）。
  - **这一版不做的**：已经关了应用的实例，后来间隔变长也不升成关模拟器（唤醒只会提前）。`idle.action` 的默认值仍是 `close_app`，两个唤醒提前量的默认值不变。裁定里
    「回主页门槛要大于包的时限、配置检查里核」没有做成检查（配置检查读不到包自己的时限），改成歇的这次运行以回主页门槛为响应期限，总览 11-11 认了。
  - **两个新键** `idle.return_home_min_gap_ms`、`idle.stop_emulator_min_gap_ms`：毫秒，60000 到 86400000，整节和每台（`instance.<id>.idle.*`）都能设，可在运行中改，
    每次空闲检查都读；两个默认值都是待实测的起始值。`auto` 生效时（整节的或某台的），整节检查还要求 `return_home_min_gap_ms` ≤ `min_gap_ms` ≤ `stop_emulator_min_gap_ms`，
    违反时在破坏规矩的那个键上拒成 `config_value_invalid`（`section_check`；`crates/runtime-config/src/idle.rs` 359–372）；别的动作下只核范围。`auto` 不需要 `start_emulator`。
  - **看得到的**：歇的时候 `actingctl status` 和总览里这台的钥匙由 Runtime 的认领占着、跑的是回主页包，和启动包的运行一样；然后实例空出来，应用开着，没有关闭的行，也没有唤醒。
  - **配置不写 `auto` 就什么都不变**：`none`、`close_app`、`stop_emulator` 都和以前一样。
- **池的线可以按实例给**（#428 SCHED-C1e，登记 §1.299 的 “Slice SCHED-C1e”；`contracts/scheduling/README.md` 的 “Lines for one instance”，`contracts/runtime-config.md` 的
  “Pool lines for one instance”）。
  - **键** `instance.<实例 id>.scheduling.pool_lines`（文件里是 `instances[]` 那一项的 `scheduling.pool_lines`）：一个对象，没有默认值，可在运行中设；最多 32 个池，成员名是
    这台实例 `schedule.json` 里的池 id（组合出的副本不带 `@<别名>`；目录的标识符规矩：1 到 128 字节，小写 ASCII 字母或数字开头，其后小写字母、数字、`.`、`_`、`:`、`-`），
    每个池写 `{"stop_line", "hard_line", "trigger_line"?}`，每条线是 0 到 9007199254740991 的整数，或 `{"type": "fact", "fact_key", "offset"}`，和目录里的线一样两种写法，
    引用在池的范围里读（`crates/runtime-config/src/scheduling.rs` 的 `instance_pool_lines`，`instance.rs` 281–290 的键表行、607 的整台检查）。
  - **检查**（启动、`check-config` 和指令都核）：拒成 `config_value_invalid`，`config_key` 是这台的键，带出错处在值里的 JSON 指针（`/<池 id>`、`/<池 id>/<线>`、
    `/<池 id>/<线>/<成员>`）：值、池、线或引用的形状不对或多了成员，`wrong_type`；第 33 个池、不合标识符规矩的成员名、越界的常数、偏移或事实键，`out_of_range`；
    缺一条线（指在缺的那条上），或两边都是常数、或引用同一个事实而能比出大小时不满足「硬线 < 停手线 ≤ 触发线」（指在硬线或触发线上），`section_check`。
  - **只有指令还要问现行的一代目录**：值里点名的每个池，都必须是现行一代给这台实例、并且声明了估值的池，否则 `section_check`、指针指到那个池；宿主在核请求之前把这些池
    填进检查的上下文（`crates/runtime-host/src/host/runtime_config.rs` 217、1254 起 `valued_pools_context`），不加新码。启动时恢复以前设的值、`check-config`、改别的键都不问
    这一条，所以后来某一代不再给这台的池，值留着、在那个池回来之前什么都不改。
  - **效果**：某台设了某个池，在这台上就**整套替换**目录里这个池的线（没写 `trigger_line` 就是上限，不沿用目录的）；别的池、别的实例照旧用目录的线。宿主每个评估过的
    策略周期都读（同一文件 1214 起 `pool_lines_context`），所以指令下一个周期生效；改线不结束正在进行的封顶回合，下一次评估按新线走，回合只照自己的规矩结束；清掉就照同样
    的办法回到目录的线。`S ≤ 上限`、`T ≤ 上限` 每次评估照旧核（`lines_invalid`）。评估认目录里同 id 的池，也认这台的副本 `<id>@<别名>`
    （`crates/policy/src/evaluator/capped.rs` 898–924 `lines`：先实例的设定、再目录、再默认）。
  - **`lines_source`**（新键，词表 `lines_source`：`instance`、`catalog`、`default`）：凡是带线值（`stop_line`、`hard_line`）的理由都多带它：`capped_spend`；作为决定理由的
    `pool_lines_unspendable`（编译诊断只读目录，不带）；`pool_line_held` 的 `would_cross_hard_line`、`lines_invalid` 和回合里带线的挂起（`capped_not_dispatchable`、
    `capped_limited_spent`、`capped_would_cross_hard_line`、回合不试的成员的 `capped_not_head`）。不带线的挂起（`capped_cap_unknown`、`capped_round_empty`、排在已派出的
    队首后面的块候选的 `capped_not_head`）不带。开发者文本末尾多 ` lines_source=<词>`。目录里它是可选的，v0.12.53 写的记录照旧读。池既没写线、也没人设时是 `default`，
    目录写了线是 `catalog`，有实例的设定就是 `instance`，不论设定用哪种写法。
  - **怎么设、怎么清**：Lab 的 `config set <键>=<值>` 和 `config clear <键>`（请求 `ConfigureSettings`），MCP 的 `ac_config_set`（observer 档，和除设备后端以外的每个键一样）；
    每次改动都记进 `host.config_overrides` 的一行，带 `set_unix_ms`、`request_id` 和渠道。监控台不给这个键分档（看不到，UI 仓要用再加）。
  - **生产上**：自 2026-11-11 起有一台实例的一个池声明了估值、在它上面试跑调度，其余实例暂停着。所以本版上线后，那一台带线值的理由从第一个评估过的周期起就多带
    `lines_source` `catalog`；没人设线之前，派单和以前一样。
- **一场消耗多少可以从事实读**（#428 SCHED-C1f1，登记 §1.303 的 “Slice SCHED-C1f1”；`contracts/scheduling/README.md` 的 “A run's consumption read from a fact”，
  `contracts/scheduling/v2/README.md`，`contracts/standard-pack-schedule.md` 的同名一节）。v0.12.53 只写了契约，本版做成。
  - **写法**：只在 V2，`consumes` 那一项的 `amount` 可以写成 `{"type": "fact", "fact_key": B, "fallback": F}`：每次评估从这台实例范围里的事实 B 读一场的量，读不到可用的值时用
    F。三个成员都要写；写错的各在出错的成员上拒（V1 里 `unknown_field`；`type` 不是 `fact`、键不是标识符、兜底不是整数 `type_mismatch`；兜底越界 `limit_exceeded`，超过常数
    上限 `effect_incompatible`；缺成员 `missing_required_field`；多成员 `unknown_field`；和 `expected_amount_milli` 一起写、写在 `produces` 上、同一任务同一池写两项这种形式，各见
    v2/README）；兜底值 1 到 9007199254740991，且不超过池的常数上限（`crates/policy/src/schema.rs` `EffectAmount::Fact`，`validation.rs` `validate_consumption_fact`）。
    `schedule.json` 里有规则写它的，要在 `requires` 里写 `consumption_fact`，否则在第一个这种 `amount` 处拒成 `schedule_invalid`（`crates/policy/src/compose.rs` 1428–1434）。
    **不写这种形式的目录，序列化、哈希和决定都逐字节照旧。**
  - **什么时候用事实、什么时候用兜底**（照所有事实的规矩；`crates/policy/src/evaluator/consumption.rs` 223–288 `resolve`、292 起 `read`）：每次评估，每个任务对它范围覆盖的
    每台实例各读一次，只读这台实例范围里的那个键。在、没过期、没被重置作废、可信度大于 0、是 1 到 2^53−1 的整数且不超过池的常数上限，就用它；缺了、过期了、被重置作废了，用
    兜底值、不告警；在而且是现行的、但不能用（可信度 0、别的类型、越界），用兜底值并告警。**永远不会是「未知」**，也不会为它发起读数刷新。前向投影开头读一次，整段投影都用这个数。
  - **所有读消耗的地方读同一个数**：净收益的成本项、池的读数和硬线检查、`pool_lines_unspendable`、封顶回合的排位、计划和队首检查、保留量的各处、平局判定、前向投影。封顶回合
    按消费者认回合的键只记兜底值，所以读数变了回合照旧、次序不变，计划和检查读当时的量；兜底值改了才算消费者变了。`host.capped_episodes` 的行不变。
  - **警告** `consumption_fact_unusable`（新码，policy，warning，ladder `none`；`task_id`、`pool_id`、`fact_key`、`unknown_reason`（`fact_low_confidence` 或 `type_mismatch`）、
    `consumption`）：事实在、没过期但不能用时，排在这个任务在这台实例上每个决定的最前面（每个池一次，在 `benefit_fixed_value` 之前），从不决定结果。处理：改正包为这个键写的
    读数（`ac_facts` 看得到那条记录），或者改 `schedule.json` 里的兜底值。
  - **理由上多两个值**：消耗从事实读的任务，在 `pool_line_held`、`capped_spend`、`resource_reserve_insufficient`、`pool_lines_unspendable` 上多带 `consumption`（这次评估读到的
    整单位量）和 `consumption_source`（词表 `consumption_source`：`fact`、`fallback`）；常数消耗两个都不带。
  - **新鲜度和唤醒**：用上的那条事实什么时候过期，同一台上消耗或保留这个池的候选的新鲜度就到什么时候，评估在过期后一毫秒醒来、改读兜底值；用兜底值时什么都不约束。
  - **工具**：`actinglab standard-pack check` 和 `package bundle --schedule` 的回答多一个 `consumption_facts`（每个游戏、没挂起的规则里每个这种项一条
    `{rule_id, pool_id, fact_key, fallback}`，按文件顺序；有 `schedule.json` 时总出现、可以为空，没有时不出现；`crates/resource-tooling/src/api.rs` 153–157、195–199）；覆盖统计
    把它算作声明了数量。`config.subsystems` 多一行常开的 `consumption_fact`（`apps/actingd/src/config/manifest.rs` 218–226）。
  - **没有排程文件写这种形式之前什么都不变**：没有新配置键、新账本形状，也不升接口修订。
- **核心的测试和夹具不再点名真实游戏**（#434 NEUT-1，登记 §1.305）：只改测试、夹具、一份金样和一道守卫（架构测试 `neut1_source_and_test_trees_name_no_real_game`，门表
  `contracts/test-gates/contract.toml` 的 A4 行）；产品代码、行为、契约、配置、账本、结果码和接口都不变。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`、`instance add`。
  `crates/lab-rescue` 自 v0.12.53 一个文件都没改，`actingctl install` 走的代码就是 v0.12.53 说明核过的那份。

配置：只升 `actingd-config`，按代码和 `contracts/component-interfaces.md` 核过。`distribution/windows/component-interfaces.json` 是 `actingd-config` [5, 28]、`install-selection`
[1, 1]、`install-control` [0, 1]、`ledger` [3, 7]、`runtime-client` [3, 6]、`package` [3, 8]、`runtime-outcomes` [1, 1]；架构测试
`tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120` 一行随之钉 [5, 28]。

- 升 `actingd-config` 一次（修订 28 一行写两片）：理由见上；修订 27 的文件就是修订 28 的文件。计划表里 LAB-6 那一行仍写 28，表下多一句：28 已被本版占用，LAB-6 做的时候取
  下一个空号，含义不变。
- 不升 `ledger`：`crates/ledger` 自 v0.12.53 没改；没有新记录、新动作、新载荷成员。歇记在已有的 `run_idle_action` 下（`idle_action`、`member` 是名字型的键，`return_home`、
  `auto`、`maintenance` 是新值，不用登记）；`lines_source`、`consumption`、`consumption_source` 是已有决定理由的可选值，读者对码只核拼写（`component-interfaces.md` 137）；
  `host.idle_state`、`host.capped_episodes` 形状不变；指令行不是文件或记录的形状（`host.config_overrides` 的行没变）。
- 不升 `runtime-client`：`crates/runtime-client` 自 v0.12.52 没改，契约 crate 只多三个键名（`crates/actingcommand-contract/src/outcome/keys.rs`）。不升 `package`：
  `consumes` 的事实写法由 `requires` 的 `consumption_fact` 把关（和 §1.289、§1.299 一样）。`runtime-outcomes` 仍是 1：新码、键和词表都在这一修订里追加。
- 配置键：新键是整节的 `idle.return_home_min_gap_ms`（默认 600000）、`idle.stop_emulator_min_gap_ms`（默认 7200000）（`distribution/windows/actingd.defaults.json` 530–531，
  `crates/runtime-config/src/idle.rs` 172–186）；每台的 `instance.<id>.idle.return_home_min_gap_ms`、`instance.<id>.idle.stop_emulator_min_gap_ms`（默认取整节的；
  `INSTANCE_ROWS` 4 → 6，同一文件 190–218）；每台的 `instance.<id>.scheduling.pool_lines`（对象，没有默认值，只在文件写了或指令设了的实例上报行；`instance.rs` 281–290）。
  都可在运行中改，`reader` 是 IDLE-2 或 SCHED-C1e，`since` 0.12.54。`idle.action` 的词表多 `auto`（`idle.rs` 62）。没有去掉的键；默认值文件只多那两项，示例配置没改。
  `config.parameters.idle` 多两行，`config.parameters.instance` 每台多两行（设了线的再多一行），`actingd check-config` 列出它们；`config.selection` 里随包默认值文件的摘要随之改变。
- `config.subsystems`：多一行常开的 `consumption_fact`（`apps/actingd/src/config/manifest.rs` 218–226；`contracts/actingd-check-config.md`、`contracts/runtime-config.md` 各添
  一句）。从 v0.12.52 直接装的，还会看到 v0.12.53 加的 `pool_trigger_line`。
- 结果码：目录从 3907 个码变成 3908 个码，位置仍是 1750 个，仍是 29 个分片，键从 626 个变成 629 个，词表从 219 个变成 221 个，没有退役的码。新码 1 个：
  `consumption_fact_unusable`（policy，warning，ladder `none`，待定稿，since 0.12.54）。新键 3 个：`lines_source`（词表 `lines_source`）、`consumption`（`integer`）、
  `consumption_source`（词表 `consumption_source`）；新词表 2 个：`lines_source`（`instance`、`catalog`、`default`）、`consumption_source`（`fact`、`fallback`）；前缀
  `consumption_` 归 policy。改了的：`capped_spend`、`pool_line_held`、`pool_lines_unspendable` 各多三个可选键 `lines_source`、`consumption`、`consumption_source`，
  `resource_reserve_insufficient` 多两个可选键 `consumption`、`consumption_source`，说明随之改；`instance_idle_action`、`instance_idle_action_unsupported` 的说明、常见原因和
  处理多了 `auto` 和歇的写法；位置 `run_idle_action` 的说明多一句歇；键 `idle_action` 的说明多 `auto` 和 `return_home`。本版的发布文件是空的：没有新定稿或退役的条目。
- 程序文件：workspace 仍是 39 个成员，`Cargo.toml`、`Cargo.lock` 自 v0.12.53 都没改，没有新的依赖。`.github/`、`ci/`、`ratchet/` 自 v0.12.53 都没改；`tools/` 只改了架构测试
  （修订的钉值和 NEUT-1 的守卫）；两个包的文件清单和 v0.12.53 一样。`apps/actingctl`、`apps/mcp-entry` 没改，MCP 工具仍是 41 个；`apps/actinglab` 只改了测试，仍是 100 条命令、
  45 个顶层分派臂；`contracts/lab-operations.json` 没改，救急模块提供的行仍是 30 行。不稳定表照旧是 FLK-08 和 FLK-10；NEUT-1 重算了 FLK-10 那个测试
  （`protocol_goldens_match_current_cli`）读的金样里 8 处钉值，表本身没动。

升级与退回（安装工具的代码 `crates/lab-rescue/src/install/` 自 v0.12.53 一个文件都没改；其中 `plan.rs`、`update.rs`、`undo.rs`、`programs.rs`、`postcheck.rs` 自 v0.12.51
都没改，在 v0.12.52、v0.12.53 和本版里逐字节相同，下面这几份的行号对三个版本的安装工具都对；`deploy.rs` 在 v0.12.52 里少 INST-5b 的几段，行号是本版的，v0.12.52 的另注）：

- **v0.12.52 的安装工具直接装本版：不用任何单向标志，也不被拒，生产可以跳过 v0.12.53。** 计划的边逐条（`plan.rs`）：
  - `ledger` `previous_max_in_next`（288–294）：Rp [3, 7] 的上限 7 在 R\* [3, 7] 里，过；`one_way`（295–301）：7 不大于 7，过；计划里没有 `ledger_verify`（915）。
  - `runtime-outcomes` `next_max_in_installer`（302–314）：R\* 的 1 在安装工具的 [1, 1] 里，过。
  - `install-control`、`runtime-client` 的 `installer_meets_previous` 和 `installer_meets_next`（315–331）：安装工具的 [0, 1]、[3, 6] 和两边的声明都相交，过。
  - `package` `bundle_max_in_next`（332–355）：目标仍是 [3, 8]，各包的结果和装 v0.12.52、v0.12.53 时一样。`speaks` `runtime_equals_tools`（756–763）：两半的声明相同，过。
  - `actingd-config` `installer_knows_keys`（359–376、764；`unknown_keys` 506–536）：比的是**现在的配置文件**和**安装工具自己**（v0.12.52）的键表与默认值；生产的文件不写本版的
    任何新成员（v0.12.52 自己就拒），`keys` 为空，过。安装工具声明的 [5, 27] 在这条边上只是列出来：**更新计划里没有哪条边拿安装工具的 `actingd-config` 去比目标的
    [5, 28]**；三方共同修订的 `installer_ui_next_common`（`ui_edges` 431–465）只在门测试里算（430 `#[cfg_attr(not(test), allow(dead_code))]`），根目录有 UI 成员以后才进计划。
  - `actingd-config` `next_check_config`（902–914）：本版的 check-config（带 `--install-root`：28 ≥ 17，`programs.rs` 34，`plan.rs` 858–866）读现行的文件，修订 27 的文件
    它照收，过。
  - 计划的 `defaults_diff`（574–598）：从 v0.12.52 装是三行，`idle.return_home_min_gap_ms`（`live` 为空，目标 600000）、`idle.stop_emulator_min_gap_ms`（空 → 7200000）两行
    `source` 是 `default`，加上 v0.12.53 带来的 `ledger_observer.tag.workbench_patterns`；从 v0.12.53 装只有前两行。只是列出，不拒。
  - 部署后检查的 `new_keys`（`postcheck.rs` 494–526，`added_keys` 593–601）核那两个空闲键在新 Runtime 的 `config.parameters.idle` 里来源是 `default`（文件不写它们）；本版报这
    两行，过（v0.12.52 的 `runtime.rs` 254–285 和本版的 292–323 读法相同）。
  - 所以部署本版仍由在线的 v0.12.52 安装工具跑，命令行和部署 v0.12.53 时的一样。
- **v0.12.53 的安装工具装本版**：代码同上，声明和 v0.12.52 的相同（`actingd-config` [5, 27] 等），每条边同样过，不用标志；生产没有上过 v0.12.53，眼下用不到。
- **退回：安装工具允许退回本版替换的那一版，不用标志。** `install rollback`、部署的自动回退（`deploy.rs` 763–854 的 `rollback_gate`，813 `accept_ledger_one_way: false`；
  v0.12.52 的是 728、778）和 `install undo`（`undo.rs` 388、396）走同一个回退门（`update.rs` 383–505），四部分都要过：
  1. 接口边（393–399）：除 `actingd-config` 外两边的声明都相同，都过。`installer_knows_keys` 用跑退回的那个安装工具的键表：本版的安装工具总认得；用 v0.12.52 的安装工具而文件
     还写着本版的新成员时不过（`interface_edge`）。
  2. 账本可读（400–437）：**只对本版替换的那一版过**（`contracts/install-cli.md` 的 “The rollback gate” 第 2 条）。生产从 v0.12.52 直接装本版的话，退回 v0.12.52 可以；
     **退回 v0.12.53 被拒**（`rollback_gate_failed`，部分 `ledger_readability`，`previous` 写着 v0.12.52）：v0.12.53 没在这个根上跑过，要上它只能先退回 v0.12.52 再装 v0.12.53。
     反过来，先装 v0.12.53 再装本版的，退回 v0.12.53 可以，直接退回 v0.12.52 不行（先退到 v0.12.53）。
  3. 打开时间（438–489）：照旧。
  4. 配置（490–503）：目标的 check-config 要收现行的配置文件。v0.12.53、v0.12.52 拒收写了 `auto`、两个空闲门槛或 `scheduling.pool_lines` 的文件，这一部分就不过
     （`rollback_gate_failed`，部分 `config`），所以**退回前先改文件**。
  - 旧版启动时，`host.config_overrides` 里它没有的键（两个空闲门槛、`scheduling.pool_lines`）和值不再合规的（`idle.action` `auto`）都丢掉，记 `config_override_dropped`、列在
    启动报告的 `instructions_dropped` 里（`contracts/runtime-config.md` 的 “Restore at start”），带启动的退回的部署后检查 `instructions` 一项会因此不过
    （`postcheck.rs` 415–448）。所以**运行中设的也先用 `config clear` 清掉**，再退。
- **退回以后旧版在启动时读到的，逐片要先做的**：
  - **IDLE-2**：把 `idle.action`（整节和每台）改回 `close_app`、`stop_emulator` 或 `none`，把 `return_home_min_gap_ms`、`stop_emulator_min_gap_ms`（整节和每台）从文件里去掉，
    运行中设过的清掉，再退。`host.idle_state` 里只有 `close_app` 或 `stop_emulator`，每个版本都读得了；歇过的实例开着、没有行。
  - **SCHED-C1e**：先用 `config clear` 清掉每台的 `scheduling.pool_lines`，文件里写了这个成员的去掉。旧版没有这个键，恢复出来的值它丢掉（`config_override_dropped`），那个池回到
    目录的线；写了这个成员的文件它拒收（修订 27，那一项不收未知成员）。账本不用动。
  - **SCHED-C1f1**：还没有排程文件写这种形式之前，没有要撤的。写了并生效以后，先把它换成常数 `amount`、让本版启用一代不带它的目录，再退：旧版在读取处拒这一项（`amount` 处
    `type_mismatch`），也不认 `consumption_fact` 这个子系统（组合时 `schedule_requires_missing`），而它启动时要编译派单和决定记忆点名的每一代目录（§1.225 的退回更正）。
    `host.capped_episodes` 的行不变（登记 §1.303 的 “SCHED-C1f1 rollback”）。
  - **退回 v0.12.52 时另外**（v0.12.53 说明的退回）：SCHED-C1c 写了 `trigger_line` 并生效的，同样先换上不写它的排程文件、让本版启用那一代再退；SCHED-C1d 没有要撤的，
    v0.12.52 不读也不记 `host.capped_episodes`，之后更新的版本再启动时那条记录按锚点规则过期、整份丢掉，记一条 Warning `capped_state_failed`（`load_capped_episodes`）；
    INST-5b 留下的全局暂停 v0.12.52 的 `release-hold` 不管，用 `actingctl resume --state-root <s>` 解除，生产在 LAB-7 前不用 `instance add`，碰不到。
  - **退回 v0.12.53 时**：`host.capped_episodes` 的形状不变，v0.12.53 照它自己的锚点规则读。
  - **NEUT-1**：没有要做的。
- 配对：
  - **安装工具**：见上，部署本版跑的是 v0.12.52 的安装工具。
  - **部署工具包的命令行，本版都照旧收**：它跑的全是 `actingctl install` 的 `release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop
    --hold --known-pause …`、`status --root [--operation]`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause …
    --start --detach`、`release-hold --root --reason`，跑的是在线的 v0.12.52 的安装工具；`apps/actingctl` 和 `crates/lab-rescue` 自 v0.12.53 没改，`gate_434_lab3.rs` 照旧
    逐条钉住。**没有一条被拒。** 答法变了的只有内容上的：预览和计划的 `defaults_diff` 多上面那两（或三）行，部署后检查的 `new_keys` 列出两个空闲键；`edges`、`steps`、
    `check_config`、`extra` 的形状和结果不变。工具包另用新版 actinglab 跑的 `--json standard-pack check --root <游戏根>`，同下。**工具包要跟着改的**：它给热放置工具包写构建记录
    的那一步核新包清单里的 `actingd-config` 是 [5, 27]、记成 `5,27`，本版要改成 [5, 28]、`5,28`（热放置工具包重钉时拿这个值和清单比）。
  - **热放置工具包的命令行，本版都照旧收**：actinglab 两行，`--json standard-pack check --root <游戏根>`（S7）和 `--json standard-pack compose --root <包根>
    --instance <别名>=<游戏>/<区服> … --out <新目录>`（K3）；actingctl 的 `status`、`status --config`、`pause --instance <别名> --reason <理由> --drain-timeout-ms <毫秒>`、
    `resume --instance <别名>`、`schedule --instance <别名>`、`overview --root <根> --instance <别名> --sections catalog`（都带 `--state-root`）。命令行代码自 v0.12.53 没改
    （`apps/actinglab/src/standard_pack_cli.rs` 自 v0.12.52 也没改；v0.12.53 说明核过这些行在 LAB-5B4、LAB-5B5 之后照旧收）。**没有一条被拒。** 答的内容上能看到的：
    `standard-pack check` 有 `schedule.json` 时多 `consumption_facts`（工具包不读）；本版**没有新的检查警告码**；`status --config` 的 `config.subsystems` 多 `consumption_fact`
    （从 v0.12.52 算还有 `pool_trigger_line`），`config.parameters.idle` 多两行、每台多两行（工具包只读来源为 instruction 的条目和 `standard_packs.check_interval_ms`）；
    `schedule --instance` 在有估值的池上多 `lines_source`，消耗从事实读时多 `consumption`、`consumption_source`。**热放置工具包要学的**：它的子系统名单要加 `consumption_fact`；
    跳过 v0.12.53 的话 v0.12.53 那次重钉没做，名单里也还没有 `pool_trigger_line`，要一起加，不然 `requires` 写了它们的排程文件会被工具包判成阻断；已知警告要加 v0.12.53 起就有、
    本版照旧会报的 `standard_pack_line_trigger_redundant`（S7 把不认识的警告码当失败）。
  - **控制台 UI**（最新发版 UI v0.12.15 的提交是 `bb0fd1c7`；UI 的 main 已到 `90b87443`，多 22 个提交、还没发版；两者都把 Runtime crate 钉在 `d06910c6`，即 v0.12.47，
    `Cargo.toml`）：
    - 两者声明的 `actingd-config` 都是 [5, 27]（`crates/acui-setup/component-interfaces.json`），**还到不了 28**。契约和登记写的是「和认得 28 的监控台同版发」，这件 UI 仓还没做。
      眼下的影响：安装工具还不算 UI 的边（见上），部署不被它挡；控制台改设置走 `ConfigureSettings`，不写配置文件；`runtime-client` [3, 5] 和本版的 [3, 6] 相交，账本 [3, 7] 相同。
    - **空闲动作 `auto`**：每台的 `idle.action` 在控制台的日常档。设置页照原样写出 `auto`（`crates/acui-app/src/workbench/settings.rs` 的 `value_words`：没有说法的词照写）；
      主页的规则面板把它当作「没有名字的词」照原样显示（`crates/acui-source/src/workbench/rules.rs` 的 `IdleNow::Other`），人可以改成「什么都不做」或「关游戏」（控制台不提供
      关模拟器），一改就换掉 `auto`。两个文件 v0.12.15 和 main 相同。
    - **新键**：两个空闲门槛（整节和每台）和每台的 `scheduling.pool_lines` 不在它钉的 v0.12.47 键表里，控制台按「键表没有的键」藏起来，也不算读不了的行
      （`crates/acui-source/src/workbench.rs` 的 `take`：main 1218–1224，v0.12.15 1106–1107）。
    - **`lines_source`**：v0.12.15 读不运行说明时只读理由的码，main 另读 `benefit_gap`、`pool_line_hold` 两个值（main 的 `workbench.rs` 1717–1738）；`lines_source`、
      `consumption`、`consumption_source` 两版都不读、不显示。新码 `consumption_fact_unusable` 不在它钉的码目录里，读者只核拼写。

运行上的变化：

- 部署本身不改变派单：配置不写 `auto`、没人给实例设线、排程文件不写消耗的事实写法，决定都和以前一样；`config.subsystems` 多一行，`config.parameters` 多几行。
- 生产上有估值的那一个池（11-11 起，一台实例试跑），带线值的理由从本版第一个评估过的周期起多带 `lines_source` `catalog`；从 v0.12.52 直接装时，v0.12.53 的回合活过重启也随之上线。
- 可以在运行中给某台的某个池单独设线（`config set`、`ac_config_set`），下一个周期生效、不结束回合；清掉回到目录的线。
- `idle.action` 可以设成 `auto`：按到下一件事的间隔歇一次、关应用或关模拟器；不设就和今天一样关应用。

本版合并的 PR：

- #1597 IDLE-2（#386，登记 §1.304；经列车 #1599）：`idle.action` `auto`、两个门槛键、回主页歇一次（`crates/runtime-host/src/idle_action.rs`、`host/idle.rs`、
  `crates/runtime-config/src/idle.rs`）；主机门行 `#386 IDLE-2 auto` 的 13 个测试（规划器 6 个 `gate_idle2_auto`，宿主 4 个 `gate_idle2_auto_host`，配置 3 个 `idle2_auto_tests`）；
  改造架构测试钉修订的那一行（[5, 28]），现有的 IDLE 测试一个没动。没有新码。
- #1598 SCHED-C1e（#428，登记 §1.299；经列车 #1599）：`instance.<实例 id>.scheduling.pool_lines`、指令只认有估值的池、评估先读实例的线、`lines_source`；策略 4 个
  （`gate_sched_c1e`）、配置 5 个、宿主 1 个测试（门行 `#428 SCHED-C1e`），改造同一个架构测试的修订钉值，金样没有重录。新键 1 个和它的词表。
- #1599 合并列车：带进上面两个，修订 28 在 `contracts/component-interfaces.md` 里对成一行，架构测试的注释并成一句，登记 §1.304 记下总览认了「以响应期限代替配置检查」；
  没有别的代码和测试。
- #1600 NEUT-1（#434，登记 §1.305）：16 个文件 240 行测试夹具换成中性的值，金样 `apps/actinglab/tests/golden/expected.json` 的 8 处钉值照产品自己的算法重算，新守卫 1 个
  （门表 A4 行）；断言没动，没有产品代码。
- #1602 SCHED-C1f1（#428，登记 §1.303）：`consumes` 的 `amount` 从事实读、带兜底（`crates/policy/src/evaluator/consumption.rs` 等）、`consumption_fact_unusable`、理由上的
  `consumption` 和 `consumption_source`、`consumption_facts`、子系统行 `consumption_fact`；门行 `#428 SCHED-C1f1` 的 7 个测试（策略 6 个，资源工具 1 个）；改造
  `gate_cov1` 的 1 个测试（整体构造的回答结构体多一个成员），金样没有重录。新码 1 个、新键 2 个、新词表 1 个，不升任何接口修订。

## v0.12.53

本版是 v0.12.52 之后合并的 9 个 PR：合并列车 #1592 带进的 #429 R-6 第一片（#1586）、#434 LAB-5B4（#1587）、#428 SCHED-C1b（#1588）、SCHED-C1c（#1589）和
#421 INST-5b（#1590），以及 #428 SCHED-C1d（#1593）、SCHED-C1f（只有契约，#1594）、COV-1（#1595）和 #434 LAB-5B5（#1596）。先说部署：接口声明都不变
（`actingd-config` 仍是 [5, 27]，`package` 仍是 [3, 8]，账本仍是 [3, 7]），部署不用带 `--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.52 的安装工具装本版
照常；装上以后安装工具允许退回 v0.12.52，不用带标志，**但有排程文件写了 `trigger_line` 并生效以后，要先换上不写它的排程文件、让本版启用那一代，再退**（见下面的退回）。
没有新键，也没有去掉的键；默认值文件只改了一处：`ledger_observer.tag.workbench_patterns` 的默认列表多一项 `host.capped_episodes`（**配置文件自己写了这个列表的，要自己
加上这一项**，见下面的封顶消耗顺序）；`config.subsystems` 多一行常开的 `pool_trigger_line`。运行上要知道的是这几处：

- **以前悄悄接受的 actinglab 命令行，现在会被拒**（#434 LAB-5B4、LAB-5B5，登记 §1.287 的 “Slice LAB-5B4” 和 “Slice LAB-5B5”；`contracts/lab-two-entries.md`
  §9.3）。actinglab 的自带命令一开头就照各自的读取清单解析整行（`apps/actinglab/src/flag_args.rs` 83 `parse_reading`、122 `parse_switches`、147
  `refuse_unread_positionals`），有意只改三件事：
  - **命令不读的参数被拒**，以前是略过；
  - **开关不再吞掉后面的词**：开关后面的词只有是 `true` 或 `false` 才算它的值，别的词留在原处，当位置词或下一个参数读；
  - **命令不读的位置词被拒**；带值的选项落在行尾没有值，拒成 `value_missing`。
  - 拒法和 CLI 模块拒一行多余的短词相同（`apps/actinglab/src/cli_module.rs` 248 `refused_word`）：`validation_failed`，文字 `<命令>: unknown <词>`（或
    `value_missing`），细节 `{argument, argument_problem}`；actinglab 自己那一行退出码 1，开发者那一行 `ERROR actinglab: …`，和 `watch --frobnicate` 一样；带 `--via` 的行
    退出码 2。都在命令读写任何东西、连 Runtime、建会话目录或取帧之前。actinglab 的全局选项（`--instance`、`--game`、`--server`、`--dry-run`、`--verbose`、`--json` 等）
    照旧在行里任何位置都收；旧的路由参数 `--via-daemon`、`--local`、`--state-dir` 照旧在原来拒的地方按名拒。
  - **LAB-5B4 的 28 条命令**（三条都适用；每条读的选项、开关、位置词，和一个以前悄悄接受、现在被拒的例子）：
    - `touch-probe`：什么都不读（`--touch-backend` 照旧拒）。例：`touch-probe --instnace neutral.b`。
    - `recover`（`recover <实例>`、`session recover`）：选项 `--seed`、`--fresh-delay-ms`、`--pack`、`--pack-root`、`--pages`、`--navigation`、`--env-task`、
      `--scene`、`--to`、`--max-actions`、`--step-timeout-ms`、`--poll-ms`、`--startup-max-rounds`、`--startup-interval-ms`、`--startup-login-file`；开关
      `--stale-capture`、`--startup-login`、`--capture`、`--diagnose`、`--require-fresh`；不读位置词。例：`session recover --capture --instnace neutral.b`。
    - `tap-target`：选项 `--target`、`--seed`、`--scene`、`--package`、`--zip`；开关 `--allow-destructive`、`--require-fresh`；位置词 `<target>`。例：
      `tap-target ui/ok --sede 7`。
    - `navigate`：选项 `--to`、`--step-timeout-ms`、`--poll-ms`、`--seed`、`--scene`、`--package`、`--zip`；开关 `--allow-destructive`、`--require-fresh`。例：
      `navigate --to home extra`。
    - `lab debug-package`：`--package`、`--zip`。例：`lab debug-package --package p --frobnicate`。`lab export-evidence`：`--package`、`--zip`、`--out`、`--outcome`。
      例：`lab export-evidence --package p --out e.zip --outcom failure`。`recognize-artifact`：`--request`。例：`recognize-artifact --request r.json extra`。
    - `record start`：选项 `--state-dir`、`--task-id`、`--locale`、`--template-threshold`、`--metric`、`--holder`、`--lease-holder`、`--record-id`、`--lease-id`；开关
      `--force`。例：`record start neutral.b --task-id t`（以前别名被略过，录制开在默认实例上）。`record status`：`--state-dir`。例：`record status neutral.b`。
      `record candidates`：`--state-dir`、`--step-id`；位置词 `<step-id>`。例：`record candidates s1 s2`。
    - `record amend`：选项 `--state-dir`、`--from-drift-diagnostics`、`--step-id`、`--id`、`--candidate-index`、`--auto-candidate`、`--region`、`--threshold`、
      `--contrast-frame`、`--negative-frame`、`--metric`、`--from`、`--to`、`--click`；开关 `--color-check`、`--no-color-check`、`--clear-threshold`、`--destructive`、
      `--non-destructive`；位置词 `<step-id>`。例：`record amend --from-drift-diagnostics d.json --holder h`（`--holder`、`--lease-holder`、`--lease-id` 以前被放过、从不读）。
    - `record step`：选项 `--state-dir`、`--kind`、`--step-id`、`--id`、`--region`、`--threshold`、`--frame`、`--source-frame`、`--artifact-dir`、`--fresh-delay-ms`、
      `--contrast-frame`、`--negative-frame`、`--metric`、`--from`、`--to`、`--click`、`--swipe`、`--drag`、`--long-press`、`--long-tap`、`--duration-ms`；开关
      `--color-check`、`--capture`、`--current-frame`、`--require-fresh`、`--destructive`。例：`record step --kind anchor --id a --frame f.png --colour-check`。
    - `package validate`、`package inspect`：`--zip`。例：`package validate --zip p.zip --strict`、`package inspect --zip p.zip --detail full`。`package bundle`：
      `--applications`、`--packs-root`、`--out`、`--source-repository`、`--source-commit`、`--maintenance`、`--pages`、`--interrupts`、`--navigation`、`--schedule`、
      `--display-layout`、`--loading`。例：`package bundle --applications a.json --packs-root packs --out o --layout l.json`。
    - `resource compile-maa`：选项 `--repo`、`--maa-tasks`、`--task`；开关 `--facts`。例：`resource compile-maa --repo r --maa-tasks m --tasks t`。
    - `operation validate`：`--repo`、`--operation-dir`；`operation inspect`、`operation explain`：`--operation-dir`。例：`operation validate --repo r --operation-dir d extra`、
      `operation inspect --operation-dir d --out x`、`operation explain --operation-dir d extra`。
    - `recognize`：选项 `--target`、`--scene`、`--package`、`--zip`；开关 `--require-fresh`。例：`recognize --target t --scene f.png --zip p.zip --threshold 0.9`。
      `detect-page`：选项 `--scene`、`--package`、`--zip`；开关 `--check-pages`、`--require-fresh`。例：`detect-page --scene f.png --zip p.zip --all`。`current-page`：选项
      `--scene`、`--package`、`--zip`；开关 `--require-fresh`。例：`current-page --scene f.png --zip p.zip extra`。`is-visible`：选项 `--target`、`--scene`、`--package`、
      `--zip`；开关 `--require-fresh`；位置词 `<target>`。例：`is-visible t1 t2 --scene f.png --zip p.zip`。`locate`：选项 `--template`、`--metric`、`--scene`、
      `--fresh-delay-ms`；开关 `--capture`、`--require-fresh`；位置词 `<template>`。例：`locate t.png --scene f.png --region 0,0,9,9`。
    - `run list`：什么都不读。例：`run list --limit 5`。`capture`（`session capture`）：选项 `--out`、`--fresh-delay-ms`；开关 `--diagnose`、`--record`、`--require-fresh`；
      位置词只认 `diagnose`。例：`capture diagnostics --out f.png`（以前别的词被略过，照样取一帧）。`lab receipt`：`--req`；位置词 `<req_id>`。例：
      `lab receipt --req r1 --limit 5`。`lab evidence`：`--id`；位置词 `<evidence_id>`。例：`lab evidence --id e1 --run r1`。
  - **LAB-5B4 只改开关和位置词两条的严格命令**：`observe` 的开关（`--capture`、`--preview`、`--record`、`--pretty` 等）只收 `true`/`false`，不读位置词，例：
    `observe --capture neutral.b`（以前 `neutral.b` 被当成 `--capture` 的值，`--capture` 于是为假；现在因位置词被拒，指定实例写 `--instance neutral.b`）；`do` 同样的开关，
    第二个位置词被拒（读 `<target>`），例：`do --capture el1 el2`；`ensure`、`wait` 同样，各读 `<page>`，例：`wait --capture --page home extra`；`record mark`、`record stop`
    的开关只收 `true`/`false`，剩下的词由它们自己的位置词检查拒（以前也拒，现在换成位置词的理由），例：`record mark --close-step word`；`package fmt` 的 `--check`、
    `package loading` 的 `--write`、`--check` 只收 `true`/`false`，例：`package fmt --package d --check yes`（以前 `--check` 读成假，文件被真的改写）；`package preflight`、
    `resource catalog`、`resource validate` 不读位置词，例：`resource validate --repo r extra`。
  - **LAB-5B5 的 38 条命令**（登记的普查：命令清单 100 条里，59 条本来就会拒，38 条这一片收掉，3 条 `lab arbitrator`、`control export`、`operation run` 本来每一行都拒、
    原样不动，没有由操作表驱动的；读取清单是各命令旁边的产品常量，如 `cli_information.rs` 的 `HELP_READS`）：
    - `help`、`version`、`paths`、`capabilities`、`doctor`、`lab vendor-stdio-selftest`：什么都不读。例：`help list`、`version --short`、`paths --all`、
      `capabilities --command observe`、`doctor --fix`、`lab vendor-stdio-selftest --trace`。（全局的 `--version` 写在哪里都照旧只印版本。）
    - `list`：位置词 `<kind>`。例：`list targets extra`。`explain`：位置词 `<run-id>`。例：`explain --run r1`（以前把这个参数当成运行号）。`user-config get`：位置词
      `<key>`；`user-config set`：`<key>`、`<value>`。例：`user-config get adb_path extra`、`user-config set adb_path p extra`。
    - `detect`：选项 `--resource-root`、`--game`、`--instance`、`--server`、`--task`、`--scene`、`--fresh-delay-ms`；开关 `--require-fresh`、`--dry-run`。例：
      `detect --task t --sceen f.png`。`env resolve`：选项是这四个范围参数加 `--task`、`--path`、`--value`、`--key`；位置词 `<input>`。例：`env resolve --task t --keys k`。
      `env status`：四个范围参数加 `--task`。例：`env status --task t extra`。
    - `stream`（`session stream`）：选项 `--max-frames`、`--interval-ms`、`--fresh-delay-ms`、`--instance`、`--input-relay`、`--interactive-input`、`--input-event`、
      `--relay-event`；开关 `--dry-run`、`--require-fresh`；位置词只认开头的 `check`，带 `--input-relay` 或 `--interactive-input` 时其后的词都归转发的那个动作（`tap`、
      `swipe`、`long-tap`、`key`、`text`，由动作自己核个数）。例：`stream --max-frame 5`、`stream extra`。
    - `lab run`：`--package`、`--zip`、`--out`（旧路由参数和 12 个执行覆盖参数照旧拒）。例：`lab run --zip p.zip --out o.zip --timeout-ms 5`。`lab validate`：`--zip`。
      例：`lab validate --zip p.zip --strict`。`lab replay-evidence`：`--zip`、`--expected-sha256`。例：`lab replay-evidence --zip e.zip --expected-sha256 h extra`。
      `package run`：`--zip`、`--out`。例：`package run --zip p.zip --task t`。
    - `session status`（`lab status`）：开关 `--diagnostics`。例：`lab status --diagnostic`。`session throat-policy`、`session capture-policy`、`session record-policy`、
      `session self-heal-policy`、`session contract`、`session api`：什么都不读（旧路由参数照旧拒）。例：`session contract --format md`。`session transport`：`--endpoint`；
      位置词 `<action>`（`plan`、`check`）。例：`session transport check --endpoint 127.0.0.1:1 --timeout 5`。
    - `session monitor-policy status`、`clear`：`--instance`，动作之后不读位置词。例：`session monitor-policy clear --all`。`session monitor-policy set`：选项 `--instance`、
      `--interval-ms`、`--expect`、`--to`、`--scene`；开关 `--recover`、`--capture`（旧路由参数和 `--max-iterations` 照旧拒）。例：`session monitor-policy set --capture
      --holder h`。`session instance`：动作之后不读位置词（`app` 把其余的词交给 `session app`）。例：`session instance list extra`。`session app`（`session instance app`）：
      `--instance`；开关 `--record`（旧路由参数、`--package` 照旧拒）。例：`session app launch --wait-ms 5`。
    - `control inspect`：`--control`。例：`control inspect --controls c`。`control verify`：`--candidate`、`--candidate-id`。例：`control verify --candidate c.json
      --candidate-id i --click`。`control probe-click`：`--effect`、`--expect-before`、`--expect-after`。例：`control probe-click --effect navigation_only --expect-befor p`
      （以前拒的理由是缺保护条件，不是拼错的参数）。`control diff`：`--candidate`、`--stable`。例：`control diff --candidate a --stable b --ignore-order`。
    - `run open`：位置词 `<run-id>`。例：`run open r1 r2`。`run export`：`--out`；位置词 `<run-id>`。例：`run export r1 --out o.zip --format zip`。`report export`：
      `--out`；开关 `--last-error`。例：`report export --last-error --out r.zip --since 1d`。
  - **LAB-5B5 点名的两种旧写法**：`stream tap 10 20 --input-relay`（转发开关写在行尾，以前把第一个位置词当成动作）现在拒成 `value_missing`；`--input-relay` 写在别的参数
    前面，或者把动作写成它的值（`stream --input-relay tap 10 20`），照旧。`session monitor-policy set` 带 `--request-timeout-ms`、`--lease-holder`、`--holder`、`--lease-id`
    的，以前存监控参数时被放过、从不读，现在拒（和 LAB-5B4 对 `record amend --from-drift-diagnostics` 那三个参数一样）。另有两处以前把行尾的选项当成 `true`、现在拒成
    `value_missing`：`control inspect --control`、`env resolve --task t --path`。
  - **从拒变成收的**：开关后面跟着命令自己的位置词，`do --capture <目标>`、`ensure --capture <页面>`、`wait --capture <页面>`、`tap-target --allow-destructive <目标>`
    （以前开关把这个词吞成值，整行被拒或走了别的路；现在走取帧那条路，用这个目标或页面，LAB-5B4）；`stream --require-fresh check`（以前 `check` 被当成开关的值，跑的是
    Runtime 的序列；现在做检查，带 `--require-fresh`，LAB-5B5）。
  - **`serve --stdio` 和 MCP 看到的不变**：`serve` 只把一行声明过的参数交给命令，而每一个都在那条命令的读取清单上，开关只交参数名本身；MCP 的 actinglab 子进程
    （`--via mcp`）给 `record start`、`record status` 的只有清单上的参数，给 `observe`、`do` 的只有开关本身，`session app <动作> --instance <别名>` 也在清单上。两片的测试各有
    一条核这件事。
  - **我们自己的调用方都不受影响**：合并前两次只读摸底（资源仓、监控台的仓、Runtime 自己的子进程和 MCP、总览的工具包；LAB-5B4 的命令 78 处调用，LAB-5B5 的 38 条命令
    30 处调用，其中 29 处是契约里的示例），没有一处会被拒。仓外的脚本若靠被略过的参数，现在会明确报错，按上面的清单改。
- **停掉一次运行**（#429 R-6 第一片，登记 §1.302；`contracts/runtime-interface.md` 的 `StopInstanceRun` 一行，表 `contracts/lab-operations.json` 的 `task stop`）。
  - `actinglab task stop <实例> <请求号> [--wait <时长>]`（核心行，也能经 `serve --stdio` 调；`crates/lab-ops/src/core_ops.rs` 841 起）发 `StopInstanceRun`，能停的是
    客户端直接发起的运行（`RunContainedTask`：actinglab `run start`、MCP `ac_run_pack`）：运行在下一个检查点停，以 `contained_task_cancelled` 结束；结束前、租约放掉前，Runtime
    在运行自己的线程上用运行自己的租约做一次输入复位（一条挂在这次运行名下的 `InputReset`），发停止的客户端断不断开都一样，运行的客户端不再欠一次复位
    （`crates/runtime-host/src/host/stop_instance_run.rs` 52–100，`contained_task.rs` 的取消分支）。在例行统计里它算一次中断，不算失败，不升梯、不欠重跑、不占次数。
  - **还停不了的**：调度器派的运行和 Runtime 自己的运行（启动包、恢复阶梯的一级）拒成 `scheduled_contained_task_not_client_cancellable`，带 `run_origin`（`policy` 或
    `host`）和运行的 `request_id`，什么都不取消（`stop_instance_run.rs` 290–301）；这类运行照旧要暂停这一台（或用玩家占用）来停。调度运行要等 `ledger` 8 才有
    「中断」的记法。这一台既没在跑、也不是最近停过的那个请求号，拒成 `contained_task_cancellation_identity_missing`（带 `request_id`）。每次拒绝都入账（`command.rejected`）。
  - **回答**：`InstanceRunStopped {stopped: {request_id, settled, reset?}}`，成功码是新的 `request_stop_instance_run_completed`。等到运行结束是 `settled: true` 和 `reset`：
    `done`；`failed`（复位失败：只围栏这一台，租约按失败的运行收尾，成功码的原因里挂 `instance_resources_fenced`，自检可解）；`not_needed`（运行在看到停止之前自己结束了，
    或者被期限、暂停先结束了）。等满 `wait_ms` 还没结束是 `settled: false`、没有 `reset`，停止照旧有效。不带 `--wait` 就是 `wait_ms` 0，马上答。再发同一个停止，从这一台
    「最近一次被停的运行」读结果（只在内存里，每台一条，重启就没了）。`ac_stop_run` 和 actinglab `run stop`（客户端自己的取消加复位）不变。
  - 监控台的「关闭这个任务」用的就是这个请求，但现有的监控台版本还没把它接上，按钮照旧灰着（见下面的配对）。
- **封顶消耗顺序：触发线、花不掉的线、活过重启的回合**（#428 SCHED-C1c、SCHED-C1d，登记 §1.299 的 “Slice SCHED-C1c” 和 “Slice SCHED-C1d”；
  `contracts/scheduling/README.md` 的 “Pool lines”、“Capped spending order”，`contracts/standard-pack-schedule.md` 的 “A redundant trigger”）。
  - **池可以写 `trigger_line`**（只在 V2，和 `stop_line`、`hard_line` 一起写才收，要求 0 ≤ 硬线 < 停线 ≤ 触发线 ≤ 上限；写法和两条线相同；`crates/policy/src/schema.rs`
    1186，检查在 `validation.rs` 2211 起）：读数到了触发线就开始封顶回合，读数 ≤ 停线且 < 触发线时这一回合花完（`crates/policy/src/evaluator/capped.rs` 415–427）；
    不写就是上限，行为、序列化和目录哈希都和以前一样。`schedule.json` 里有池写它的，要在 `requires` 里写 `pool_trigger_line`，否则在第一个写它的池处拒成
    `schedule_invalid`（`crates/policy/src/compose.rs` 1378–1385）；旧版 Runtime 不认这个子系统。池写了触发线时，`capped_spend` 和 `pool_line_held` 多带 `trigger_line`。
  - **线花不掉时告警** `pool_lines_unspendable`（warning；`pool_id`、`pool_capacity`、`stop_line`、`hard_line`、`least_consumption`）：有估值的池在某一台上，每个参加判定的
    消耗者跑一次都比「上限减硬线」消耗得多时，读数到了触发线也不开始回合（进行中的回合等着），这些候选以这个理由推迟，`ScheduleExplanation` 和不运行的说明里看得到；以前是
    开始一回合又立刻以 `capped_round_empty` 结束。编译器分辨得出时，目录也带同码的编译告警，进 `config.catalog_warnings` 和总览（`validation.rs` 2395 `unspendable_lines`）。
  - **旧触发的提示** `standard_pack_line_trigger_redundant`（warning）：池已经写了两条线和估值块，规则的触发里经 `all` 还要求「读数 ≥ 容量事实减余量」，
    `actinglab standard-pack check` 和 `package bundle --schedule` 在 `warnings` 里报它（排在 SS-4 那条之后；`compose.rs` 546 `redundant_triggers`）。只是提示，规则照旧
    组合、运行；工作台不记它。
  - **回合活过重启**（SCHED-C1d）：回合存成带标签的程序事实 `host.capped_episodes`（Runtime 范围的记录列表：写入者的启动锚点一行，每个回合一行，后面跟它的消费者行和成员行；
    `crates/runtime-host/src/host/capped_episodes.rs`）。评估过的一轮里回合变了才记一次；启动时在第一次评估之前读回（`crates/runtime-host/src/host.rs` 2535–2539，在
    `host.idle_state` 之后）。只有写它的那次启动正是上一次完整结算的启动才留（`host.idle_state` 的锚点规则，`capped_episodes.rs` 279–283）；读不回、不合规矩、过期的整份丢，
    实例没登记或池在现行目录里没有估值块的只丢那一回合，各记一条 Warning `capped_state_failed`（`operation` `load_capped_episodes`；事实库拒收一次记录是
    `store_capped_episodes`，回合留在内存里，下次变化再记），启动照常。新一代目录不再一律结束回合：这个池在这台上的消费者（各自的身份、每次消耗 ⌈C/1000⌉、是否限量）没变，
    回合就接着走；容量、读数或线所读的事实给不出值时，进行中的回合等着（候选以 `pool_line_held` `capped_cap_unknown` 挂住），值回来再走或照规矩结束。生产上自 11-11 起
    有一台实例的一个池有估值（v0.12.52 上线后放置的排程文件），本版上线后那一台的回合就记成这条事实；别的实例没有池有估值，没有回合。
  - **部署时要核一处**：`ledger_observer.tag.workbench_patterns` 的默认列表多了 `host.capped_episodes`（`crates/ledger/src/global/capture.rs` 45–61，
    `distribution/windows/actingd.defaults.json` 431）。**配置文件自己写了这个列表的，要把这一项加上**，否则这条事实不带标签，启动时找不到，回合照旧活不过重启（不报错，和
    SCHED-C1d 之前一样）。看法：`actingctl status --config` 里 `config.parameters.ledger_observer` 的 `ledger_observer.tag.workbench_patterns` 那一行，`source` 是 `default`、
    值 14 项、末尾是 `host.capped_episodes` 就对；`source` 是 `explicit` 而值里没有它，就要改配置文件。部署前 `install update plan --preview` 的 `defaults_diff` 里这一行的
    `source` 是 `file`，也说明配置文件自己写了它。`config.selection` 里随包默认值文件的摘要随之改变。
- **目录里的值是谁写的；随链的规则**（#428 SCHED-C1b、COV-1，登记 §1.299 的 “Slice SCHED-C1b” 和 “Slice COV-1”；`contracts/standard-pack-schedule.md` 的
  “Provenance”、“The scheduler workbench”、“Coverage”）。
  - 生效的组合一代里有规则写固定值、或池声明估值块时，工作台在记编译告警的同一处、同一时刻记程序事实 `config.catalog_provenance`
    （`crates/runtime-host/src/host/standard_pack_schedule.rs` 821 起）：先是计数行 `{game, kind, origin, count}`（`kind` 是 `fixed_value` 或 `valuation`，`origin` 是 `pack`
    或 `model`），再是逐个的 id 行；最多 256 行，放不下时先裁 id 行、再裁计数行，末行 `{counts_omitted, ids_omitted}` 写明各缺多少。出处变了才记，每次启动后第一次检查再记一次；
    没有这类值的一代不记，从有变没有时记一条空的；没有自己的账本记录。
  - `actingctl overview` 和 `ac_overview` 的 `catalog` 块随之多一个成员 `benefit_provenance {fixed_value {pack, model}, valuation {pack, model}, items [{game, kind,
    origin, id}]}`（计数是各游戏的合计，被裁时带 `items_omitted`、`counts_omitted`；`crates/lab-ops/src/overview.rs` 727 起）。没有这条事实（旧版写的状态根）或事实为空时
    没有这个成员，目录块逐字节照旧。MCP 超预算时先裁 `catalog.benefit_provenance.items`（留 `items_truncated: true`），再裁 `catalog.compile_warnings.items`。
  - `actinglab standard-pack check` 和 `package bundle --schedule`：一条参加判定、没写产出、消耗和固定值的规则，触发经 `all` 依赖另一条规则的成功（`dependency_completed` 只认
    `succeeded`、带 `since_own_run: true`），被依赖的那条带值或者自己也随链（一路往上走，每条只走一次），而且触发里每条链接指向的都是参加判定的规则，就算「随链」：
    `benefit_coverage` 多一项 `follower`（为 0 时不出现），回答多一个 `benefit_followers [{task_id, rides_with}]`（`rides_with` 是走到头的那条带值规则，没有随链规则时不出现；
    `crates/policy/src/compose.rs` 236–262、792 起，`crates/resource-tooling/src/api.rs` 148–152），它们不再算没声明，也不再各报一条 `standard_pack_benefit_undeclared`。
    没有随链规则的文件，回答逐字节照旧。这只是工具对文件的静态读法：组合、启用和决定都不变。
- **`actinglab instance add` 重启前后多挂一份全局暂停**（#421 INST-5b，登记 §1.300 的 “INST-5b”；`contracts/install-cli.md` 的 “No moment unpaused” 和 “Ways out of a
  kept hold”）。
  - 挂住各实例之后、关 Runtime 之前，再挂一份全局暂停，理由就是这次 hold 的理由 `lab-instance-<毫秒>`（`crates/lab-rescue/src/install/hold.rs` 359 起）；已经有任何人的全局
    暂停时不挂，那一份的理由和修订都不动。Runtime 把它落盘，启动时在任何实例做事之前恢复，所以新实例从新 Runtime 就绪起一直有暂停挡着，直到它自己的 `instance.added`
    暂停挂上，这时才先交还这份全局暂停、再交还 hold 的实例暂停（`release` 一步写 `global_resumed: true`）。INST-5 留下的那段空窗没了。
  - **失败留下的怎么解**：新实例暂停不上时以 `post_check_failed` 收尾，`hold` `kept: true`，全局暂停和 hold 的实例暂停都留着，`next` 给出 `pause_added`、`release_hold`、
    `actingctl_release_hold`、`actingctl_resume` 和 `actingctl_resume_global`。先暂停新实例（`pause_added`，除非它的账号已经可以跑），再用救急模块自己的
    `release-hold <理由>`：它交还这份全局暂停和 hold 的实例暂停，别的不碰。`actingctl install release-hold` 不管全局暂停，用它的话之后再 `actingctl resume --state-root <s>`。
    设全局暂停时 Runtime 不答拒成 `runtime_not_running`，拒绝则拒成 `runtime_stop_failed`，都在磁盘上有任何改动之前，已挂的暂停当场交还。
  - **生产照旧手工加实例**，到 LAB-7 为止。
- **一场消耗多少可以从事实读，只写了契约**（#428 SCHED-C1f，登记 §1.303）：`consumes` 的 `amount` 写成 `{"type": "fact", "fact_key": …, "fallback": …}` 的规矩已经写好、
  已裁定，每条都标着 “not built yet”，代码片 SCHED-C1f1 还没做；本版的 Runtime 不认这种写法。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`、
  `instance add`。`crates/lab-rescue/src/install/` 里 `actingctl install` 走的 `plan.rs`、`update.rs`、`undo.rs`、`place.rs`、`status.rs`、`programs.rs` 和钉住它们的
  `gate_434_lab3.rs` 自 v0.12.51 没改；`deploy.rs`、`hold.rs`、`runtime.rs` 改了，新分支只在救急模块发起、而且 hold 要全局暂停时走（`deploy.rs` 229
  `rescue && hold::global_of(&step.intent)`）：`actingctl install` 的 `--hold` 解析恒为 `global: false`（`hold.rs` 99–104），它写的意图、证据和日志不多任何成员
  （`hold.rs` 331–333、421）。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过。`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md`
自 v0.12.52 都没改：`actingd-config` [5, 27]、`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 7]、`runtime-client` [3, 6]、`package` [3, 8]、
`runtime-outcomes` [1, 1]。

- 不升 `ledger`：`crates/ledger` 只改了默认标签列表这个常量（`capture.rs` 45–61，13 项变 14 项）。`host.capped_episodes` 和 `config.catalog_provenance` 都是已有记录
  `runtime.fact_recorded` 的新键（前者带标签，有它的索引行，和 `host.idle_state`、`host.agent_wake` 加进来时一样）；R-6 写的是已有的命令、输入、租约和任务记录和已有的动作；
  新码都在已有的记录里，读者对码只核拼写（`component-interfaces.md` 136）；SQLite 格式 2、`actingcommand.event.v3`、表、索引和视图都没变。INST-5b 的安装器自己不写账本记录，
  日志仍是 `actingcommand.install-operation.v1`，只在 `instance add` 的 `config_replace` 日志里多几个成员。
- 不升 `runtime-client`：`crates/runtime-client` 自 v0.12.52 没改；`StopInstanceRun` 和 `InstanceRunStopped` 是 R-0 早就登记的形状，契约只多一个造回执的函数
  （`crates/actingcommand-contract/src/runtime.rs` 6013–6036）和这个请求种类的成功码。不升 `package`：池的 `trigger_line` 和 `schedule.json` 的成员由 `requires` 把关
  （和 §1.289、§1.299 一样）。不升 `actingd-config`：没有新键，只是已有列表键的默认值多一项（和 AGT-1a 加 `host.agent_wake` 一样）。`runtime-outcomes` 仍是 1：成功码、键和词表
  都在这一修订里追加。操作表的格式名不变：`task stop` 一行开放（`reserved` → `planned`，多一句 `help`，请求多 `Status`），`task run` 的片号改标 R-18。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config` 只改了测试里的默认标签清单；`actingd.defaults.json` 只多 `host.capped_episodes` 这一项；示例配置没改；`apps/actingd`
  只在 `config/manifest.rs` 209–217 加了 `config.subsystems` 的常开行 `pool_trigger_line`（`contracts/actingd-check-config.md`、`contracts/runtime-config.md` 各添一句）。
- 结果码：目录从 3903 个码变成 3907 个码，位置从 1748 个变成 1750 个，仍是 29 个分片，键从 623 个变成 626 个，词表从 218 个变成 219 个，没有退役的码。新码 4 个（都待定稿，
  since 0.12.53）：`request_stop_instance_run_completed`（contract，success，ladder `none`）、`standard_pack_line_trigger_redundant`（contract，warning，
  `declaration_error`）、`pool_lines_unspendable`（policy，warning，`none`）和 `capped_state_failed`（runtime-host，warning，`none`）。新位置 2 个：`load_capped_episodes`、
  `store_capped_episodes`（runtime-host）。新键 3 个：`run_origin`（词表 `run_origin`：`policy`、`host`）、`trigger_line` 和 `least_consumption`（都是 `integer`）；新词表 1 个
  `run_origin`；前缀 `capped_` 也归 runtime-host。改了的：`scheduled_contained_task_not_client_cancellable` 多一个可选键 `run_origin`，`capped_spend` 和 `pool_line_held`
  各多一个可选键 `trigger_line`，说明随之改；`contained_task_cancellation_identity_missing` 的说明多一句 R-6 的拒法；键 `capped_episode` 的说明和词表 `pool_line_hold` 里
  `capped_cap_unknown` 的说明随 SCHED-C1d 改了。本版的发布文件是空的：没有新定稿或退役的条目。
- 程序文件：workspace 仍是 39 个成员，`Cargo.toml`、`Cargo.lock` 自 v0.12.52 都没改，没有新的依赖。`.github/`、`ci/`、`tools/`、`ratchet/` 自 v0.12.52 都没改，两个包的文件
  清单和 v0.12.52 一样。不稳定表照旧是 FLK-08 和 FLK-10 两条，这几片都没有动它。`apps/actingctl` 只改了 `ac_overview` 的工具说明（多写 `benefit_provenance` 和它的裁法），
  `apps/mcp-entry` 没改，MCP 工具仍是 41 个；actinglab 仍是 100 条命令、45 个顶层分派臂；救急模块提供的行仍是 30 行。

升级与退回（安装工具的代码 `crates/lab-rescue/src/install/` 里 `plan.rs`、`update.rs`、`undo.rs` 自 v0.12.51 没改，就是 v0.12.48 到 v0.12.52 说明核过的那份，下面这三份的行号
也是它的；`deploy.rs` 本版因 INST-5b 多了几段，行号是本版的）：

- **v0.12.52 的安装工具装本版，不用任何单向标志。** 两边的声明逐字相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way`（7 不大于 7）都过（`plan.rs` 288–301），
  计划里没有 `ledger_verify`（915）；`next_max_in_installer`、`installer_meets_previous`、`installer_meets_next`（302–331）、`bundle_max_in_next`（332–355：目标仍是 [3, 8]）
  和 `installer_knows_keys`（359–376，没有新键）都和装 v0.12.52 时一样；`next_check_config`（902–914）：本版的 check-config 收现行配置，它的报告只多一行常开的子系统，计划里比的
  摘要（outcome、目录计划、批准、note 的码、各游戏的包数；`programs.rs` 125–160）不含子系统，排程文件不写 `trigger_line` 时目录计划也和 v0.12.52 的相同。计划的
  `defaults_diff` 多一行 `ledger_observer.tag.workbench_patterns`（`plan.rs` 574–598），只是列出，不拒。
- **退回 v0.12.52：安装工具允许，不用标志。** 接口边两边相同、都过（`rollback_edges`，`plan.rs` 383–403，不带单向确认）；退回时 `next_check_config` 不算接口边（1030–1036）。
  部署的自动回退（`deploy.rs` 763–825 的 `rollback_gate`，813 `accept_ledger_one_way: false`）和 `install undo`（`undo.rs` 338、388）走同一组边和同一个回退门（`update.rs`
  383–505）：本版写的记录没有新形状，新码都在已有的记录里，v0.12.52 的读者只核拼写。部署时新版的部署后检查不过的话，apply 会自动退回它替换的版本，也就是 v0.12.52。
- **退回以后 v0.12.52 在启动时读到的**：
  - **SCHED-C1c**：还没有排程文件写 `trigger_line` 之前，没有要撤的。**写了并生效以后**，v0.12.52 的读者不认池的 `trigger_line`（`unknown_field`），也不认
    `pool_trigger_line` 这个子系统（组合时 `schedule_requires_missing`），而它启动时要编译派单和决定记忆点名的每一代目录（§1.225 的退回更正），带这个成员的那一代它编译不过。
    所以先放一个不写它的排程文件，等本版启用了一代不带它的目录，再退（登记 §1.299 的 “SCHED-C1c rollback”）。
  - **SCHED-C1d**：没有要撤的。v0.12.52 不读也不记 `host.capped_episodes`，每个池都从没有回合开始，和以前一样；最后那条记录留在账本里。它的启动完成结算、挪了启动锚点，所以之后
    本版（或更新的）再启动时，按锚点规则那条记录过期，整份丢掉，记一条 Warning `capped_state_failed`（`operation` `load_capped_episodes`），再记一条没有回合的事实
    （`capped_episodes.rs` 279–283、337）。
  - **INST-5b**：落盘的东西没有改形状，v0.12.52 读带 `global`、`own_pauses.global` 的日志时略过这些成员。但 v0.12.52 的 `release-hold` 只交还 hold 的实例暂停（它的
    `crates/lab-rescue/src/install/` 里没有全局暂停的设与解），本版 `instance add` 挂下、还留着的全局暂停 `lab-instance-<毫秒>` 它不管：用 `actingctl resume --state-root <s>`
    解除。生产在 LAB-7 前不用 `instance add`，眼下碰不到。
  - **LAB-5B4、LAB-5B5**：什么都没落盘；退回后那 66 条命令又会略过不认识的参数。**R-6**：`StopInstanceRun` 又拒成 `not_implemented`，`task stop` 不再提供；停掉的运行只记在
    内存里，没有要撤的；直接运行照旧用 `run stop` 和客户端的复位停。**SCHED-C1b**：`config.catalog_provenance` 是已有形状的事实记录，旧版照常读、启动时不载入（`config.`
    每次重新观察），总览里没有这个成员。**COV-1**：没有新文件、账本或配置形状，没有要撤的。
- 配对：
  - **v0.12.52 的安装工具装本版**：照常，不带标志，见上面。部署本版跑的是 v0.12.52 的安装工具。
  - **部署工具包的命令行，本版都照旧收**：它跑的全是 `actingctl install` 的 `release stage --root --dir --facts --expect-tag --expect-commit`、`guards --root --for stop
    --hold --known-pause …`、`update plan --root --build [--preview]`、`update apply --root --build --plan --stop --hold --known-pause … --start --detach`、`status --root
    [--operation]`、`release-hold --root --reason`，部署本版时跑的是在线的 v0.12.52 的安装工具。这些不是 actinglab 的自带命令，LAB-5B4、LAB-5B5 不管它们，actingctl 的
    命令行代码本版没改；`actingctl install` 的代码见上面 LAB-3 那一条，`gate_434_lab3.rs` 照旧逐条钉住。**答法变了的只有内容上的一处**：预览和计划的 `defaults_diff`
    多一行 `ledger_observer.tag.workbench_patterns`（v0.12.52 那次是空的），工具包只把它打印出来，不据此拒；`edges`、`steps`、`check_config`、`extra` 的形状不变。工具包另用
    新版 actinglab 跑的 `--json standard-pack check --root <游戏根>`，同下。
  - **热放置工具包的命令行，本版都照旧收**：actinglab 两行，`--json standard-pack check --root <游戏根>`（S7；旧轮次的脚本里是不带 `--json` 的同一行）是核心行，
    由它自己的参数表把关（只收 `--root`、不收位置词；`apps/actinglab/src/standard_pack_cli.rs` 23、52–58，自 v0.12.52 没改），不在 LAB-5B4、LAB-5B5 改的命令里；
    `--json standard-pack compose --root <包根> --instance <别名>=<游戏>/<区服> … --out <新目录>`（制包的严格行，`standard_pack_cli.rs` 26、71–77）也一样。actingctl 的
    `status`、`status --config`、`pause --instance <别名> --reason <理由> --drain-timeout-ms <毫秒>`、`resume --instance <别名>`、`schedule --instance <别名>`、
    `overview --root <根> --instance <别名> --sections catalog`（都带 `--state-root`）：命令行代码没改。**没有一条命令行在本版上被拒。** 答的内容上能看到的：
    `standard-pack check` 可能多 `benefit_coverage.follower`、`benefit_followers`（工具包不读），`standard_pack_benefit_undeclared` 变少，以及新警告
    `standard_pack_line_trigger_redundant`：**热放置工具包的 S7 把它不认识的警告码当失败**，所以在哪个 `schedule.json` 给写了两条线的池加估值块、又留着旧的容量阈值之前，
    要把这个码加进工具包的已知警告，或者同一次改动里去掉旧阈值。`overview --sections catalog` 的目录块在生效一代有固定值或估值块时多 `benefit_provenance`（工具包读的
    `catalog_id`、`catalog_hash`、`approval_id` 等不变）；`status --config` 的 `config.subsystems` 多一行、标签列表多一项（工具包只读来源为 instruction 的条目和
    `standard_packs.check_interval_ms`）；`schedule --instance` 只有在有估值的池上才可能出现新理由。
  - **控制台 UI**（最新发版 UI v0.12.15 的提交是 `bb0fd1c7`；UI 的 main 已到 `e0016bb5`，多 13 个提交、还没发版；两者都把 Runtime crate 钉在 `d06910c6`，即 v0.12.47，
    `Cargo.toml` 39–44；`crates/acui-setup/component-interfaces.json` 和它跑的 actingctl、安装器命令所在的文件自 v0.12.15 没改）：本版的声明、线上请求和账本形状都和 v0.12.52
    一样，控制台对本版照常。它会遇到：
    - `StopInstanceRun` 现在有答了，但两个版本都不发它：「关闭这个任务」一直灰着，说明写的仍是「这个 Runtime 不实现 `StopInstanceRun`」
      （`crates/acui-app/src/workbench/rescue.rs` 202–203、281–292、416–423，两版相同）。UI 接上它以后，停得了手动运行，调度运行会带 `run_origin` 被拒。
    - INST-5b 的全局暂停 `lab-instance-<毫秒>`：main 已把 `instance` 加进 Lab 救急一类（`crates/acui-app/src/ledger/resume.rs` 72–73、134–145），这种暂停不给恢复，只说明是
      Lab 救急操作挂的；v0.12.15 还没有这一类（同一文件 53–59），会给恢复按钮。生产在 LAB-7 前不用 `instance add`，眼下碰不到。
    - 账本里的新码（`request_stop_instance_run_completed`、`capped_state_failed`、`pool_lines_unspendable`）和新键 `run_origin` 不在它钉的 v0.12.47 码目录里：它钉的账本 crate
      只核拼写、读得了，控制台照原样显示码名、不配说明。
    - 建模工具 acmodel 跑的 `--json resource validate --repo <仓> --changed-path <文件>`（`model/src/acmodel/export/validator.py` 85）只带 `resource validate` 读的参数、没有
      位置词（`apps/actinglab/src/resource_declarations.rs` 58–71），照旧收。

运行上的变化：

- 部署本身不改变调度的决定：没有排程文件写 `trigger_line`，不写它的池照旧到上限才开始回合，现有的包运行结果不变；`config.subsystems` 多一行，默认标签列表多一项。
  生产上已有估值的那一个池（11-11 起，一台实例）多的是：回合从此记成 `host.capped_episodes`、活过重启；新一代目录里它的消费者没变时回合接着走；读数给不出值时
  回合等着，不再结束。
- actinglab 的 66 条自带命令不再略过不认识的参数和词，靠被略过的参数的脚本会明确报错；`serve --stdio`、MCP 和我们自己的调用方都不受影响。
- 手动发起的运行可以用 `actinglab task stop` 停下，Runtime 自己复位；调度运行和 Runtime 自己的运行照旧靠暂停来停。
- 生效一代有固定值时，总览的目录块说明每个值是谁写的；`standard-pack check` 把随链的规则单列，「没声明收益」的告警变少。

本版合并的 PR：

- #1586 R-6 第一片（#429，登记 §1.302；经列车 #1592）：`StopInstanceRun` 的处理（新文件 `crates/runtime-host/src/host/stop_instance_run.rs`）、运行自己的复位、Lab 的核心行
  `task stop`；主机门行 `#429 R-6` 的 6 个测试（`gate_429_r6`）和 actinglab 的 1 个，改造把这个请求钉成「没实现」的 2 个测试（`gate_429_r0` 的 1 个、契约的成功码普查）。
  新码 1 个、新键 1 个和它的词表，不升任何接口修订。
- #1587 LAB-5B4（#434；经列车 #1592）：读取清单成为产品代码，28 条命令照清单解析，开关和位置词的规矩；apps 门行 `#434 LAB-5B4` 的 6 个测试（`gate_434_lab5b4`），改造
  LAB-5B2 的 2 个、LAB-5B3 的 3 个、LAB-5A 的 1 个测试；`golden_protocol.rs` 的 `detect_page_failure` 用例去掉 `detect-page` 从来不读的三个参数，期望输出不变，金样没有重录。
  没有新码。
- #1588 SCHED-C1b（#428；经列车 #1592）：`config.catalog_provenance` 和总览的 `catalog.benefit_provenance`；宿主 2 个、总览 2 个测试（门行 `#428 SCHED-C1b`）；没有改现有测试。
- #1589 SCHED-C1c（#428；经列车 #1592）：池的触发线、`pool_lines_unspendable`、`standard_pack_line_trigger_redundant`、子系统行 `pool_trigger_line`；策略门行
  `#428 SCHED-C1c` 的 5 个测试（`gate_sched_c1c`）；没有改现有测试，金样没有重录。新码 2 个、新键 2 个，不升任何接口修订。
- #1590 INST-5b（#421；经列车 #1592）：`instance add` 跨重启的全局暂停；apps 门行 `#421 INST-5b` 的 4 个测试（`gate_421_inst5`），夹具的假 Runtime 多全局的暂停和恢复；没有改
  现有测试。
- #1593 SCHED-C1d（#428）：`host.capped_episodes`（`crates/policy/src/evaluator/capped/kept.rs`、`crates/runtime-host/src/host/capped_episodes.rs`）、按消费者认回合、读数未知时
  等待；策略 5 个、宿主 4 个测试（门行 `#428 SCHED-C1d`，`gate_sched_c1d`）；改造 `ledger_observer_tests` 的默认标签清单（多一项，和 AGT-1a 同类）。新码 1 个、新位置 2 个，
  不升任何接口修订。
- #1594 SCHED-C1f（#428，登记 §1.303）：只有契约，每条新规矩标着 “not built yet”；代码片 SCHED-C1f1 排在 SCHED-C1d、SCHED-C1e 之后。没有代码和测试。
- #1595 COV-1（#428）：覆盖统计的 `follower` 和 `benefit_followers`；门行 `#428 COV-1` 的 4 个测试（`gate_cov1`），改造 6 个整结构比对覆盖统计的测试（各加 `follower: 0`）。
  没有新码。
- #1596 LAB-5B5（#434）：其余 38 条自带命令的读取清单、`stream` 转发的写法；apps 门行 `#434 LAB-5B5` 的 6 个测试（`gate_434_lab5b5`），`gate_434_lab5b4` 的 3 个辅助函数改成
  同模块可见，断言没动。没有新码。

## v0.12.52

本版是 v0.12.51 之后合并的 16 个 PR：#71 FLK-11（#1570）和 FLK-12（#1575）；#428 CI-OCR-1（#1567）、VIS-LOAD-1（#1572）、SCHED-C1（契约，#1582）和
SCHED-C1a（#1585）；#434 LAB-5A（#1569）、LAB-5D（#1568）、LAB-5C（#1573）、LAB-5B2（#1574）、LAB-5G4a 与 LAB-5E（合并列车 #1580，带着 #1577、#1578）、
LAB-5G1（#1579）、LAB-5B3（#1581）和 LAB-5F（#1576）；#419 ISO-START-1（#1583）；#421 INST-5 第一片（#1584）。先说部署：接口声明都不变（`package` 仍是
[3, 8]，账本仍是 [3, 7]），部署不用带 `--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.51 的安装工具装本版照常；装上以后安装工具允许
退回 v0.12.51，不用带标志，**但有排程文件写了固定值并生效以后，要先换上不写固定值的排程文件、让本版启用那一代，再退**（见下面的退回）。没有新键，默认值文件
没改，`config.subsystems` 多一行常开的 `benefit_fixed_value`。运行上要知道的是这几处：

- **`schedule.json` 可以给规则写固定值**（SCHED-C1a，登记 §1.299 的 “Slice SCHED-C1a”；`contracts/scheduling/README.md` 的 “Fixed values written in the
  pack”，`contracts/standard-pack-schedule.md`）。
  - 参加收益判定的规则写 `fixed_value_milli`，组合时成为 V2 任务成员 `benefit_fixed_value_milli`（1 到 2^53−1，只有 V2 有，要求 `benefit_judged`；
    `crates/policy/src/schema.rs` 416，编译期检查在 `validation.rs` 540 起）。它就是这条规则的已知收益：没有消耗时净收益就是这个值，有消耗时减去所消耗的
    池的成本；判定从这个值起算，不读 `produces`（`crates/policy/src/evaluator/benefit.rs` 393 起）。所消耗的池照旧要有估值、可再生池照旧要有读数，否则照旧
    是收益未知、不派。封顶轮里照旧按「值 ÷ 消耗」排，封顶的排法没变。
  - 池可以写包里声明的常数估值，出处用 `valuation_origin` 标（不写就是包里手写，`"model"` 是模型导出）；规则的出处是 `fixed_value_origin`。出处只在
    `schedule.json` 里，不进合成后的目录：只差出处的两个文件，目录哈希和规则摘要相同。
  - 净收益来自固定值的每个决定，第一条理由是 `benefit_fixed_value`（info；`benefit_milli`、`fixed_value_milli`、`expected_duration_ms`，`benefit.rs` 858
    起），胜出者的 `rank_breakdown` 末尾带 ` fixed_value_milli=<V>`。
  - 拒绝：V1 里写是 `unknown_field`，越界是 `limit_exceeded`，没有 `benefit_judged` 是 `missing_required_field`，和有价池上的 `produces` 或
    `benefit_realization: true` 同时写是新码 `fixed_value_conflict`（error）；`schedule.json` 写了这些成员却没在 `requires` 里写 `benefit_fixed_value`、
    写在维护类规则上、规则自己写任务成员、出处写错，都在那个成员处拒成 `schedule_invalid`（读者 `crates/actingcommand-contract/src/standard_pack_schedule.rs`
    333–409，`crates/policy/src/compose.rs` 1032–1048、1133–1150）。
  - **包不写这些成员，什么都不变**：不带它们的目录，哈希、规则摘要和决定都和以前一样。写了的排程文件要在 `requires` 里写 `benefit_fixed_value`，也就要本版或
    更新的 Runtime：更旧的 Runtime 不认这个子系统，组合时在 `requires` 处拒成 `schedule_requires_missing`（组合器这一步，`compose.rs` 1151–1165）。
    `config.subsystems` 多一行常开的 `benefit_fixed_value`（`apps/actingd/src/config/manifest.rs` 199–208）。
  - `actinglab standard-pack check` 和 `package bundle --schedule` 的回答，`benefit_coverage` 多一项 `fixed_value`（写了固定值的规则数），另多一个成员
    `benefit_provenance`（`fixed_value` 和 `valuation` 各分 `pack`、`model`，按文件顺序列 id；没有 `schedule.json` 时没有；
    `crates/resource-tooling/src/api.rs` 142–147）。
- **启动时一对（任务、实例）的结算事实被拒，不再让整个 Runtime 起不来**（ISO-START-1，登记 §1.301；`contracts/runtime-fact-store.md` 的 “Settlement
  facts”）。每次启动都为每一对重推四条结算事实；以前走的是运行中的那条路，任何一次拒绝都致命，所以一对过期的事实（UNSET-2 退回的那条边，§1.296）会让所有实例
  都起不来。现在逐对分三种（`crates/runtime-host/src/host/runtime_facts.rs` 974–1044）：
  - **过期**（`runtime_fact_stale`：库里已有同一个键、观测时刻不早于这次结算的记录）：这一对不再写，记一条 Warning `runtime.failed`
    `policy_settlement_facts_skipped`（`failure_code` `runtime_fact_stale`，带任务、实例别名、事实键和观测时刻，原因链接带库里那条的 `observed_at_unix_ms`；
    1064–1118），什么都不暂停，启动继续。
  - **只关这一对的别的拒绝**（值放不下 `policy_settlement_fact_overflow`、键不合法 `invalid_runtime_fact_key`、库满 `runtime_fact_capacity_exceeded` 等）：
    同一条 Warning，`failure_code` 是那个码；这一对的实例由启动时现成的那种暂停挂住，理由 `policy_settlement_facts_skipped`，排在
    `hold_unsettled_dispatches` 之后（`crates/runtime-host/src/host.rs` 2535–2538，`runtime_facts.rs` 1127–1136）；已经暂停着的实例保留原来的暂停，其它实例
    照常起。`actingctl status` 和 `ac_overview` 显示这个暂停，看过之后用 `actingctl resume` 解除；下次启动还遇到同样的拒绝会再挂。
  - **不属于哪一对的故障**（账本写不进、锁、库和账本对不上，或一条记录提交之后才报的失败）：照旧致命，启动失败（990–996、1032–1036）。
  - 平常什么都不变：每对的事实照旧推导，没有新记录。Warning 也写进 actingd 的日志（`WARN actingd:` 行）。运行中的结算照旧是任何拒绝都致命。
- **Lab 救急模块的 `actinglab instance add`**（INST-5 第一片，登记 §1.300；`contracts/install-cli.md` 的 “Instances, the first slice” 和 “The Lab's rescue
  module”）：`actinglab instance add <别名> --game <g> --server <s> --mumu-index <n> --touch-backend <b> --capture-backend <b>`，六个参数都必填、都没有默认值，
  也能经 `serve --stdio` 调（`crates/lab-rescue/src/rescue.rs` 101、1409 起）。
  - 它在配置的 `instances` 和 `policy.facts.instances` 两处一起加上这个实例（新铸 `instance_id`）。写之前先拒：别名已有、模拟器序号已被绑、标准包根里没有这个
    游戏的 `bundle.json` 或包里没有这个区服、配置里没有可抄宿主和能力的策略行（都是 `instance_invalid`）；没有 Runtime 在跑时拒 `runtime_not_running`
    （`install/instance_add.rs` 227 起、408 起）。
  - 候选配置写在现文件旁边（`actingd.config.json.new-<毫秒>.part`），走 `config replace` 那条提交路：check-config → 挂住所有没暂停的实例
    （`lab-instance-<毫秒>`）→ 关 → 换（旧文件留成 `actingd.config.json.prev-<毫秒>`）→ 起 → 起后检查，一份 `config_replace` 日志（输入 `instance_add`）。
  - 起来以后、放开挂起之前，新实例用它自己的理由 `instance.added` 暂停（`rescue.rs` 1496 起）；放开时只交还这次自己挂的，所以新实例一直停着，等人在监控台或用
    `actingctl resume --state-root <s> --instance <别名>` 解除；之前的每个暂停都原样。
  - 登记写明的一段不受控：新 Runtime 就绪到新实例被暂停之间（起后检查加一次状态读取），新实例没有暂停。暂停不上时以 `post_check_failed` 收尾、挂起留着，
    `next.pause_added` 给出手动暂停的命令。
  - **生产照旧手工加实例**，到 LAB-7 为止：救急模块的组合操作在 LAB-7 之前不在生产根上用。
- **Lab：操作表是命令行、`serve --stdio` 和以后的 MCP 工具共用的一份**（#434 LAB-5A 到 LAB-5G1，登记 §1.287 的各个 “Slice LAB-5…”；
  `contracts/lab-two-entries.md` §8.3、§9；表 `contracts/lab-operations.json`）。
  - actinglab 提供的每一行都写全了自己的参数（LAB-5A：71 行、455 个参数，新类型 `unsigned`、`decimal`、`run_ref`，新成员 `required_in`、`disruptive`）；
    严格命令的选项表、宽松命令读的参数都由测试和它的行对齐（LAB-5B2、LAB-5B3）；今天只有 MCP 收的参数也成了行的参数（LAB-5C）；每行多一句 `help`
    （LAB-5E，138 行；`crates/lab-ops/src/table.rs` 176，本版还没有命令把它印出来）。表的格式名仍是 `actingcommand.lab-operations.v1`；仓外用严格解析读这张表
    的，升钉时要认这些成员和类型。
  - **制包开关只决定列不列，从不拒绝调用**（LAB-5F；`crates/lab-ops/src/locate.rs` 151–186，`apps/actinglab/src/cli_module/authoring.rs` 103–175）：安装根里
    选中的配置写 `lab_module_authoring: true`（生产就是），或者不在任何安装根里，制包的 34 行在 `help`、`list commands`、`capabilities` 里列出；缺、`null`、
    `false` 就不列，但照样能跑；配置读不出时也不列，并在开发者那一行写一次 `WARN actinglab: …`。`standard-pack check` 改成核心行，哪里都列、都能跑（救急模块的
    `place` 和 `actingctl install pack place` 拿它做事后检查）。
  - `serve --stdio --front mcp` 记 `Lab-MCP`（LAB-5G4a；`apps/actinglab/src/cli_module/serve.rs` 61、129）：带上它，这个进程往录制里加的步骤和标记、救急行的
    日志都记发起者 `Lab-MCP`，连接和今天 MCP 的 `--via mcp` 子进程一样；同时设了 `ACTINGCOMMAND_INITIATOR` 就拒。现在没有东西这样启动 `serve`。
  - 回答的预算归调用方（LAB-5D，`crates/lab-ops/src/budget.rs`），核心行的服务代码搬进操作层（LAB-5G1，`crates/lab-ops/src/core_ops.rs`）：命令行和
    `serve --stdio` 不给预算，照旧整份打印；今天的 MCP 服务器逐字节照旧。
  - **本版的命令行收什么、拒什么，除下面这几处都和 v0.12.51 一样**（按各小节的 “What … changes live” 核过；LAB-5B2、LAB-5B3、LAB-5D、LAB-5E、LAB-5G1
    在命令行上什么都不变）：
    - LAB-5A（`apps/actinglab/src/cli_module/words.rs` 106、124）：原来拒、现在收的 `op status --check-rollback <op>` 和 `release stage --local-build <dir>`；
      原来收、现在拒的两种不常见写法：LAB-5A 补全的 15 个核心和救急行上，选项后面跟一个 `--` 开头的词当它的值（`runs --task-prefix --cursor …`、
      `gap --why "--…"`，`value_missing`），以及引擎行上不带值的全局选项后面跟没声明的选项（`watch --run-root --x`）。原来拒、现在也拒只是码或文字变了的：缺
      必填参数由词法先拒（核心和救急行 `actingctl_usage_invalid`，actinglab 自己的命令 `validation_failed`，`argument_problem` `required`）；`regression` 带全局
      选项缺参数从 `offline_device_scope_forbidden` 变成 `validation_failed`；`signatures register|match|retire`、`unpin` 带 `--dry-run` 缺参数或带没声明的
      选项、`export-evidence --dry-run` 不带 `--out`，从 `dry_run_unsupported` 变成 `validation_failed`。核心行的整数按数值转写（`--limit 05` 交成 `5`）。
    - LAB-5C：命令行多出这些选项（`serve --stdio` 同样收）：`events` 的 `--view`、`--since-unix-ms`、`--until-unix-ms`、`--through-sequence`、
      `--event-type`、`--exclude-event-types`、`--fact-key`、`--min-severity`、`--max-severity`、`--origin-module`、`--task-id`、`--correlation-id`、
      `--detail`、`--wait-s`；`diagnose --since-unix-ms`；`codes --prefix|--owner|--limit`；`packs --role`；`remedy set --rows <json>`（和规则文件二选一：
      都不给照旧拒，都给拒成 `--rows` `conflicting`，超过规则文件的上限拒成 `out_of_range`，都在发请求之前，`words.rs` 146）；`material --out <dir>`。不带新
      选项的命令行答法照旧。
    - `serve --stdio`：拒行里没声明的参数（LAB-5A）；拒以 `--` 开头的字符串值（`actingctl_usage_invalid`，`argument_problem` `not_recognised`，LAB-5G4a，
      `words.rs` 328）；收 `recover` 的 `env_task`、`game`、`server`，`record step` 的 `metric`、`duration_ms`，`record amend` 的 `metric`、`negative_frame`
      （LAB-5B3）；多认 `--front mcp`。
    - LAB-5F 只改列出：没有写这个键的安装根上，帮助里不列制包命令，调用照样走到命令。
- **模型加载后的头一次 OCR**（VIS-LOAD-1，登记 §1.298；`contracts/vision-model-folders.md` 的 “Startup and first use”）。要先检测的请求，以前检测模型是在
  请求的期限起算之后才第一次加载，冷进程里「加载加检测」超过 `timeout_ms` 时，识别还没开始就报超时（`PPOCR recognizer could not start before the
  configured timeout`）。现在请求先定走哪条路，要检测而检测模型还没建的先建好，然后才起算期限（`providers/ppocr-onnx-json/src/lib.rs` 1466–1484）：
  `timeout_ms` 只限推理。所以每次启动以后、每个 Lab 进程里头一次要检测的请求，不再因为检测模型还在加载而失败。加载失败照旧报加载自己的错，推理超时照旧是原来的
  码和文字；包的 `timeout_ms` 和它的范围（1 到 60 000）没变，运行级的上限也没变。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place`、`config replace`，
  也不用本版新加的 `instance add`。`crates/lab-rescue/src/install/` 里 `actingctl install` 走的 `plan.rs`、`update.rs`、`deploy.rs`、`undo.rs`、`place.rs`
  和钉住它们的 `gate_434_lab3.rs` 自 v0.12.51 没改；改的是救急模块的 `rescue.rs`、只有它调用的 `config_swap.rs`、`hold.rs`（一个函数改成 `pub(crate)`）和
  新文件 `instance_add.rs`。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过。`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md`
自 v0.12.51 都没改：`actingd-config` [5, 27]、`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 7]、`runtime-client` [3, 6]、`package` [3, 8]、
`runtime-outcomes` [1, 1]。

- 不升 `ledger`：`crates/ledger` 自 v0.12.51 没改。ISO-START-1 写的是已有的 `runtime.failed` 记录（一个新码）和已有的暂停行（`host.scheduling_pause`，理由是名字），
  `benefit_fixed_value` 是决定理由里的一个新码，读者对码只核拼写（`component-interfaces.md` 136）；SQLite 格式 2、`actingcommand.event.v3`、表、索引和视图都
  没变。INST-5 的安装器自己不写账本记录，日志仍是 `actingcommand.install-operation.v1`，多一种已有形式的输入。
- 不升 `runtime-client`：`crates/runtime-client` 自 v0.12.51 没改，没有新的请求种类或成员。不升 `package`：包的格式没变，SCHED-C1a 的目录成员和
  `schedule.json` 成员由 `requires` 把关（和 §1.289 一样）。不升 `actingd-config`：配置的形状没变，INST-5 写的是已有成员。`install-cli` 修订 1 收下实例
  命令，不升。操作表的格式名不变，只加了可选成员和类型。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.51 都没改；`apps/actingd` 只在
  `config/manifest.rs` 加了 `config.subsystems` 的常开行 `benefit_fixed_value`（`contracts/actingd-check-config.md` 和 `contracts/runtime-config.md` 各添一句）。
- 结果码：目录从 3900 个码变成 3903 个码，位置仍是 1748 个，仍是 29 个分片，没有退役的码。新码 3 个（都待定稿，since 0.12.52）：
  `policy_settlement_facts_skipped`（runtime-host，warning，ladder `none`）、`benefit_fixed_value`（policy，info，`none`）和 `fixed_value_conflict`（policy，
  error，`none`）。新键 1 个：`fixed_value_milli`（`integer`）；新前缀 1 个：`fixed_value_`（policy）；没有新位置、新词。改了的 2 个：`install_command_refused`
  多带两个已有的键 `server`、`instance_index`（INST-5 的 `instance_invalid`），说明随之改；`runtime_fact_stale` 多一个可选值 `observed_at_unix_ms`（只作
  `policy_settlement_facts_skipped` 的原因链接），说明随之改。LAB-5F 首轮改过的 `lab_command_option_disabled` 已恢复原样，和 v0.12.51 相同。本版的发布文件是
  空的：没有新定稿或退役的条目。
- 程序文件：workspace 仍是 39 个成员，`Cargo.toml`、`Cargo.lock` 自 v0.12.51 都没改，没有新的依赖。`.github/` 只改了
  `.github/workflows/windows-remote-build.yml`：构建作业现有的工具测试那一步多跑一个脚本测试（CI-OCR-1 的 `Test-MaterializeVisionRoot.ps1`，不联网）；`ci/`、
  `tools/`、`ratchet/` 自 v0.12.51 都没改，两个包的文件清单和 v0.12.51 一样（CI-OCR-1 的脚本和工具登记是仓里的文件，不进包）。不稳定表照旧是 FLK-08 和 FLK-10
  两条：FLK-11、FLK-12 是测试自己的问题，已在原因上修掉，不进表。`apps/actingctl` 只改了 `src/mcp/`（LAB-5D 把裁剪和导出的代码搬去 `crates/lab-ops`，回答
  逐字节照旧），`apps/mcp-entry` 没改，MCP 工具仍是 41 个；actinglab 仍是 100 条命令、45 个顶层分派臂；救急模块提供的行从 29 行变成 30 行（`instance add`）。

升级与退回（安装工具的代码 `crates/lab-rescue/src/install/` 里 `plan.rs`、`update.rs`、`deploy.rs`、`undo.rs` 自 v0.12.51 没改，就是 v0.12.48 到 v0.12.51 说明核过
的那份，下面的行号也是它的）：

- **v0.12.51 的安装工具装本版，不用任何单向标志。** 两边的声明逐字相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way`（7 不大于 7）都过
  （`plan.rs` 288–301），计划里没有 `ledger_verify`（915）；`next_max_in_installer`、`installer_meets_previous`、`installer_meets_next`（302–331）、
  `bundle_max_in_next`（332–355：目标仍是 [3, 8]；生产根的登记不记范围，每个游戏根只给 `pack_declaration_unknown` 警告）和 `installer_knows_keys`
  （359–376，没有新键）都和装 v0.12.51 时一样；`next_check_config`（902–914）：本版的 check-config 收现行配置，它的报告只多一行常开的子系统，计划里比的摘要
  （outcome、目录计划、批准、note 的码、各游戏的包数；`crates/lab-rescue/src/install/programs.rs` 129–160、251–280）不含子系统；排程文件不写固定值时目录计划
  也和 v0.12.51 的相同。
- **退回 v0.12.51：安装工具允许，不用标志。** 接口边两边相同、都过；退回时 `next_check_config` 不算接口边（`plan.rs` 1030–1036）。部署的自动回退
  （`deploy.rs` 778）和 `install undo`（`undo.rs` 388）都是 `accept_ledger_one_way: false`，和 `rollback` 走同一组边（`rollback_edges`，`plan.rs` 383–403）和
  同一个回退门（`update.rs` 383–505）：本版写的记录没有新形状，新码都在已有的记录里，v0.12.51 的读者只核拼写。部署时新版的部署后检查不过的话，apply 会自动退回
  它替换的版本，也就是 v0.12.51（`deploy.rs` 717–790）。
- **退回以后 v0.12.51 在启动时读到的**：
  - **SCHED-C1a**：还没有包写固定值之前，没有要撤的。**有排程文件写了 `fixed_value_milli` 并生效以后**，v0.12.51 的读者不认任务成员
    `benefit_fixed_value_milli`（`unknown_field`），也不认 `benefit_fixed_value` 这个子系统（组合时 `schedule_requires_missing`），而它启动时要编译派单和决定
    记忆点名的每一代目录（§1.225 的退回更正），带这个成员的那一代它编译不过。所以先放一个不写这些成员的排程文件，等本版启用了一代不带它们的目录，再退（登记
    §1.299 的 “SCHED-C1a rollback”）。
  - **ISO-START-1**：被拒的一对在 v0.12.51 的启动时又是致命的（任何拒绝都致命，`runtime_fact_stale` 也是）。本版落盘的、理由为
    `policy_settlement_facts_skipped` 的暂停，v0.12.51 照普通实例暂停恢复，人看过以后解除；新记录对它是一个不处理的码。暂停理由是名字，v0.12.51 和本版用的是
    同一份校验（`crates/actingcommand-contract/src/runtime.rs` 2260–2272，自 v0.12.51 没改）。
  - **INST-5**：落盘的东西没有改形状。加了实例的配置两处都写了，v0.12.51 照样收；新实例的 `instance.added` 暂停每次启动照样恢复，等人解除；v0.12.51 的安装工具
    照旧读 `config_replace` 日志（`inputs` 是自由的 JSON）。只撤这个实例的话用 `config restore <备份>`：它按撤回前的配置和暂停判起后检查，会以
    `post_check_failed` 收尾、挂起留着，再用 `release-hold <它的理由>` 交还（登记 §1.300 的 “Rollback”）。
  - **LAB-5F**：之前的构建不读这个键，所有行都列出来。LAB-5A、LAB-5C 新收的写法和选项，在 v0.12.51 的 actinglab 上照旧拒；带 `Lab-MCP` 的录制和日志旧版读得了
    （六个发起者词之一）。VIS-LOAD-1：头一次要检测的请求又要从自己的 `timeout_ms` 里付检测模型的加载。
- 配对：
  - **v0.12.51 的安装工具装本版**：照常，不带标志，见上面。部署本版跑的是 v0.12.51 的安装工具。
  - **部署工具包的命令行对本版的根答得一样**：它的 `actingctl install` 那几行（`release stage`、`guards --for stop`、`update plan [--preview]`、
    `update apply … --start --detach`、`status [--operation]`、`release-hold`）跑的是在线的安装工具，`actingctl install` 的代码自 v0.12.51 没改，
    `crates/lab-rescue/src/install/gate_434_lab3.rs` 416–692 照旧逐条钉住；预览的 `check_config` 摘要不含子系统那一行，工具包读的 `edges`、`steps`、
    `check_config`、`extra`、`defaults_diff` 等成员的形状不变（接口边的结果见上面）。工具包拿新版 actinglab 跑的 `--json standard-pack check --root <游戏根>`：这一行 LAB-5F 以后是核心行，
    仍由 actinglab 自己的命令服务、印 CLI 模块的行（`apps/actinglab/src/standard_pack_cli.rs` 47–65，`cli_module/own_line.rs` 114–135 没改），工具包读的
    `outcome`、`status`、`pack_count`、`warnings` 的 `code`、`root_files` 都不变；回答多出的 `benefit_provenance`（有 `schedule.json` 时）和
    `benefit_coverage.fixed_value`（今天的包都是 0），工具包不读。
  - **热放置工具包的命令行对本版的根答得一样**：它跑的 actinglab 两行，`--json standard-pack check --root <游戏根>`（同上）和 `--json standard-pack compose
    --root <包根> --instance <别名>=<游戏>/<区服> … --out <新目录>`（制包行，LAB-5F 以后不论键怎样都照样服务；回答的成员和 v0.12.51 一样，读的 `status`、
    `games`、`tasks`、`pools`、`profiles`、`events`、`manifest`、`rules_digest`、`warnings` 不变，不写固定值的排程文件合成的目录哈希和规则摘要也不变）；
    actingctl 的 `status`、`status --config`、`pause`、`resume`、`schedule --instance`、`overview --instance … --sections catalog`：actingctl 的命令行代码自
    v0.12.51 没改，`crates/lab-ops` 里搬动的读法在命令行上不给预算、整份打印（LAB-5D、LAB-5G1）。**没有一条命令行在本版上答得不一样。** 内容上能看到的只有：
    `status --config` 的 `config.subsystems` 多一行（工具包只读来源为 instruction 的条目和 `standard_packs.check_interval_ms`），状态里可能出现新的暂停理由。
  - **控制台 UI**（最新发版 UI v0.12.15 的提交是 `bb0fd1c7`；UI 的 main 已到 `affa0bfb`，多 8 个提交、还没发版；两者都把 Runtime crate 钉在 `d06910c6`，即
    v0.12.47，`Cargo.toml` 39–44；`crates/acui-setup/component-interfaces.json` 自 v0.12.15 没改，声明 `actingd-config` [5, 27]、`ledger` [3, 7]、
    `runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：本版的声明、线上请求和账本形状都和 v0.12.51 一样，控制台对本版照常。它跑的 actingctl 命令
    （`crates/acui-app/src/workbench/tools.rs` 383、415）和安装器的命令（`crates/acui-setup/src/lifecycle.rs` 81、207、220）都没变，这两份文件自 v0.12.15 没改。
    它会遇到：
    - 新的实例暂停理由 `policy_settlement_facts_skipped`（ISO-START-1）和 `instance.added`（INST-5）：UI v0.12.15（`crates/acui-app/src/ledger/resume.rs`
      53–61）和 main（同一文件 107–118）都把它们当别的工具挂的暂停，给恢复按钮，和两片的用意一致（看过以后解除）。
    - INST-5 挂的 `lab-instance-<毫秒>`（`rescue.rs` 511–512、1436）：main 的 Lab 救急一类只认 `config`、`deploy`、`down`、`place`、`restore`、`rollback`
      （`resume.rs` 54），没有 `instance`，v0.12.15 还没有这一类，所以这种挂起在控制台上也给恢复按钮；`instance add` 以 `post_check_failed` 收尾、挂起留着时也是
      这样。生产在 LAB-7 前不用 `instance add`，眼下碰不到；UI 仓要把 `instance` 加进这张表。
    - 账本里的新码（`policy_settlement_facts_skipped`、`benefit_fixed_value`）不在它钉的 v0.12.47 码目录里：它钉的账本 crate 只核拼写、读得了，控制台照原样显示
      码名、不配说明（`crates/acui-rows/src/outcome.rs` 160–176）。
    - UI 仓不读操作表（它依赖的 Runtime crate 里没有 `crates/lab-ops`）；建模工具 acmodel 跑的 `--json resource validate`（`model/src/acmodel/export/validator.py`）
      本版没改。

运行上的变化：

- 部署本身不改变调度的决定：还没有包写固定值，现有的包运行结果不变；`config.subsystems` 多一行。
- 启动时某一对的结算事实被拒，只影响那一对：过期的只记一条 Warning，别的拒绝只停那一台；账本或库的故障照旧让启动失败。
- 启动以后、每个 Lab 进程里头一次要检测的 OCR，不再被检测模型的加载吃掉时限。
- Lab 的命令行只有上面列的几处收拒变了；救急模块多一条 `instance add`（生产不用）；MCP 和 actingctl 的答法照旧。

本版合并的 PR：

- #1570 FLK-11（#71）：「阶梯档运行中途崩溃」那条宿主测试，子进程的屏障只等到一帧的「工件已核验」就让父进程杀它，而帧留存视图只认带留存钉的帧；改成两条都在才算
  存下过一帧。只改测试，不进不稳定表，没有新码。
- #1575 FLK-12（#71）：导航那条宿主测试（`gate_373_nav5` 的 N5-8）等回执只等 10 秒，而运行自己的期限是 60 秒；改成运行的期限再加 10 秒。只改测试，不进不稳定表。
- #1567 CI-OCR-1（#428，登记 §1.297）：脚本 `scripts/windows-tools/Materialize-VisionRoot.ps1` 照工具登记填好或核对一个 CPU 用的识别根目录，给资源仓的 CI 真跑
  OCR；登记里 `ppocrv6-medium-onnx` 改成 `Ready`，ONNX Runtime 那一项多两份许可文件的钉值；构建作业多跑它的测试脚本（7 个用例，不联网）。部署出去的 Runtime
  没有任何变化，0.12.52 是头一个带它们的版本。
- #1572 VIS-LOAD-1（#428，登记 §1.298）：OCR 引擎的请求路径（`read_request`）先建好路线要的会话再起算期限。新门行 `#428 VIS-LOAD-1` 的 2 个测试
  （`gate_428_vis_load1`）。没有新码，不升任何接口修订。
- #1569 LAB-5A（#434，登记 §1.287）：操作表补全 71 行，三种类型、两个成员，actinglab 的词法和路由按表；新门行 `#434 LAB-5A` 的 2 个测试（actinglab 和救急模块各一），
  `lab_ops_table_complete` 多查几项，改造 `gate_434_lab2::cli_short_words_parse`。
- #1568 LAB-5D（#434）：`crates/lab-ops/src/budget.rs`，七个会裁的读、四个会导出的读搬进操作层；新门行 `#434 LAB-5D` 的 3 个测试（`gate_434_lab5d`）。
- #1573 LAB-5C（#434）：上面列的新参数；`remedy::dispatch_rows`；新门行 `#434 LAB-5C` 的 3 个测试（`gate_434_lab5c`）。
- #1574 LAB-5B2（#434）：31 个严格命令的选项表各成一个常量；新门行 `#434 LAB-5B2` 的 2 个测试（`gate_434_lab5b2`）。
- #1580 合并列车（#434）：LAB-5G4a（#1577，`serve --stdio --front mcp`、拒以 `--` 开头的值；新门行的 2 个测试 `gate_434_lab5g4a`）和 LAB-5E（#1578，每行一句
  `help`；新门行的 1 个测试 `gate_434_lab5e`，`lab_ops_table_complete` 多查说明的形式）。
- #1579 LAB-5G1（#434）：核心行的服务代码搬进 `crates/lab-ops/src/core_ops.rs`，连接由调用方给（`lab_ops::front::Connection`）；新门行 `#434 LAB-5G1` 的 2 个测试
  （`gate_434_lab5g1`）。
- #1581 LAB-5B3（#434）：24 个宽松命令各有一张只在测试构建里的读取清单，表补 `recover`、`record step`、`record amend` 的 4 处参数；新门行 `#434 LAB-5B3` 的 3 个
  测试（`gate_434_lab5b3`）。
- #1576 LAB-5F（#434）：`lab_ops::locate::authoring_module`、`cli_module::authoring` 的列出规则，`standard-pack check` 归核心模块；新门行 `#434 LAB-5F` 的 2 个
  测试（`gate_434_lab5f`），改造 `gate_434_lab5a` 的 1 个和 `gate_434_lab5b2` 的 2 个测试（那一行换了模块）。
- #1582 SCHED-C1（#428，登记 §1.299）：只有契约（排程契约、标准包排程契约和登记），每条新规矩标着由哪一片来做；没有代码和测试。
- #1585 SCHED-C1a（#428，登记 §1.299 的 “Slice SCHED-C1a”）：固定值的任务成员、判定、理由、编译期拒绝、`schedule.json` 成员、子系统行和工具的出处回答；两条
  `#428 SCHED-C1a` 门行的 9 个测试（策略 8 个 `gate_sched_c1a`、资源工具 1 个），改造 5 个整结构比对覆盖统计的测试（各加 `fixed_value: 0`），金样没有重录。新码 2 个、新键 1 个、
  新前缀 1 个，不升任何接口修订。
- #1583 ISO-START-1（#419，登记 §1.301）：启动时逐对分三种的结算事实写法和暂停；主机门行 `#419 ISO-START-1` 的 3 个测试（`gate_419_iso_start1`）。新码 1 个，
  `runtime_fact_stale` 多一个可选值，不升任何接口修订。
- #1584 INST-5 第一片（#421，登记 §1.300）：救急模块的 `instance add`（新文件 `crates/lab-rescue/src/install/instance_add.rs`），`config replace`、
  `config restore` 和它共用一段提交代码；apps 门行 `#421 INST-5` 的 6 个测试（`gate_421_inst5`），夹具多一个默认为空的开关。`install_command_refused` 多带两个
  已有的键，不升任何接口修订。

## v0.12.51

本版是 v0.12.50 之后合并的 3 个 PR：#71 FLK-10（#1563）、#428 RD-BUDGET-1（#1565）和 #428 UNSET-2（#1564）。先说部署：接口声明都不变（`package` 仍是
[3, 8]，账本仍是 [3, 7]），部署不用带 `--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.50 的安装工具装本版照常；装上以后安装工具允许
退回 v0.12.50，不用带标志，**但本版有一次启动在了结之后完整走完以前，不要退回**（见下面的退回）。没有新键，默认值文件没改。运行上要知道的是这几处：

- **头一次启动把「运行自己释放了租约、还没写终态就被停机切断」留下的派单了结一次**（UNSET-2，登记 §1.296；`contracts/scheduling/README.md` 的
  “Failures (#419)”）。以前账本不许把这种运行按被中断了结（它有输入或效果记录），派单就一直开着：每次启动都为它写一条 Warning
  `policy_settlement_dispatch_left_open`（头一次碰到它的启动写的是 `policy_settlement_dispatch_unsettled`，并暂停了它的实例），这一对（任务、实例）一直算在飞，
  资格年龄停在 0。生产上有一条，10-26 的。
  - 账本的检查改了：没有终态、和运行相连的释放只有一条而且是它自己的（effect `performed`），有输入或效果记录也按被中断了结；有终态的、有两条释放的照旧拒
    （`crates/ledger/src/global/storage.rs` 1420–1428）。
  - **头一次启动写的记录**（按代码核过，和 #1564 的 “At deploy” 一致）：先是了结的两条，`policy.execution_recorded`（`policy_settlement_interrupted`，Severe，
    运行时间 0，时刻取那条释放；`crates/runtime-host/src/host/policy_outcome.rs` 2849–2857）和 `policy.dispatch_completed`，由账本的同一个命令
    `reconcile_scheduled_policy_settlement` 写（2935–2951）；接着在运行的链接下写一条 `runtime.failed`，码是新的 `policy_settlement_dispatch_closed`（Info），
    `failure_code` 是之前那次拒绝的码 `scheduled_execution_recovery_interruption_conflict`，`operation` 是 `reconcile_policy_outcomes`，另有 `instance_id`、
    `task_id`，派单只有一次租约授予时还有 `lease_id`（514–550、626–634；启动里排在了结之后、报告仍开着的派单之前，`crates/runtime-host/src/host.rs`
    2303–2316）。这一对后来没再跑过的话，这次启动还记下被切断那次运行的了结事实（比这一对原有的新）。每次启动的那条 Warning 从此不再出现。了结那次启动要是
    切在执行记录和完成之间，下一次启动补上完成并由它写这条记录；切在完成和这条记录之间，这条就不写了，了结和完成两条是持久的记录。
  - **迟到的了结不算这一对的失败**：连续失败、暂停、退避、重试轮、最近一次失败都和了结前一样。这一对后来跑过的话，后来那次照旧是它最近一次；没跑过的话，
    被切断那次就是最近一次，读成被中断，这一对的结果键为空（和任何被中断的最近一次一样）。`policy_settlement_dispatch_closed` 不解除当初
    `policy_settlement_dispatch_unsettled` 在实例上留的暂停：还暂停着的，照旧先看一眼实例的屏幕再解。
  - **之后的启动（本版）**：派单已经了结，不再写了结、Warning 或第二条 `policy_settlement_dispatch_closed`。这一对的最近一次完成一律按准入顺序取
    （`crates/runtime-host/src/policy_host.rs` 575 的 `admission_order`；`failures_through` 1383、`journal_records_of` 1421、`outcome_key_snapshot` 4463、
    `latest_completed_policy_runs` 4580），值记录、任务快照、结果键和启动时重算的了结事实（`crates/runtime-host/src/host/runtime_facts.rs` 830–843）读的是
    同一次运行，照常起来；`actingd suspended` 不列迟到的那次中断。这一对不再算在飞，资格年龄重新累计，时段规划的记录照常写。
  - 退回 v0.12.50 以后看到的，见下面的退回。
- **`actinglab resource validate` 的全量扫描不再因为重复计数被拒**（RD-BUDGET-1，登记 §1.295；`contracts/resource-declarations.md`、
  `contracts/package-reference.md`）。声明读取器把每个文件第一次读到的字节留到这次运行结束，同一路径再读直接用留下的字节，不读磁盘、不再计数
  （`apps/actinglab/src/resource_declarations.rs` 423–465）。以前一个任务里的每个策略文件都要把整个任务再读一遍，计数随策略个数成平方涨，只因为重复计数越过
  64 MiB 的扫描被拒成 `resource_declaration_invalid`（`declaration read budget exceeded`，459）；现在能过。上限 64 MiB、单文件上限、每项检查读哪些文件、检查的
  顺序、拒绝的码和话都不变；回答里的 `read_bytes`（从磁盘读的字节数）只在同一文件读了不止一次的运行里变小。`resource verify` 的声明一步和 `operation`
  用的是同一个读取器。Runtime 自己装包走另一个读取器，不受影响。
- **Lab 的环境检测不再起 PowerShell**（FLK-10，登记 §1.294）。`actinglab detect` 的结果锁在 Windows 上读持有者身份的三样都直接问系统：进程启动时间
  （`OpenProcess`、`GetExitCodeProcess`、`GetProcessTimes`；`crates/lab/src/env_detection/lock.rs` 1058–1071）、主机名（`GetComputerNameExW`，1134–1138）
  和令牌的随机种子（`BCryptGenRandom`，1183–1188），调用都放在 `crates/host-metrics` 里。不再起 `powershell.exe` 和 `hostname.exe`，负载很重的机器上不再报
  `owner identity unavailable: PowerShell process inspector … timed out`。锁文件和各成员的文字不变，两边写的锁互相认得；三处读取失败时的 Windows 错误文字
  变了（比如 `process inspector could not read PID <pid>: process_query_refused; …`，1067–1069）；读不了的进程是错误、不当成已经不在，活着的持有者留着锁。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place` 或
  `config replace`。`crates/lab-rescue` 自 v0.12.50 没改。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过。`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md`
自 v0.12.50 都没改：`actingd-config` [5, 27]、`install-selection` [1, 1]、`install-control` [0, 1]、`ledger` [3, 7]、`runtime-client` [3, 6]、`package` [3, 8]、
`runtime-outcomes` [1, 1]。

- 不升 `ledger`：UNSET-2 改的是账本收一次恢复了结的规则，不是记录形状；新码不是形状变化（`component-interfaces.md` 136：读者对码只核拼写，不核是否登记）；
  值记录（登记 L7、LN4-7）形状不变，只是它点名的最近一次按准入顺序取。SQLite 格式 2、`actingcommand.event.v3`、表、索引和视图都没变。较旧的 Runtime 照样
  起来、照样调度（带补救声明的一种情形见下面的退回），按升级规则（`component-interfaces.md` 331–341）不升。
- 不升 `runtime-client`：`crates/runtime-client` 和合同 crate 自 v0.12.50 没改，没有新的请求种类或成员。不升 `package`：包的格式没变，RD-BUDGET-1 只改
  actinglab 的读取器。FLK-10 的结果锁不是登记的接口，`env-result-lock.v2` 不变，host-metrics 新加的函数只在 Runtime 内部用。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`apps/actingd`、`actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.50 都没改。
- 结果码：目录从 3899 个码变成 3900 个码，位置仍是 1748 个，仍是 29 个分片，没有退役的码。新码 1 个（待定稿，since 0.12.51）：
  `policy_settlement_dispatch_closed`（runtime-host，info，ladder `none`）；没有新位置、新键或新词。只改说明的 3 个：`policy_settlement_dispatch_unsettled`、
  `policy_settlement_dispatch_left_open`（runtime-host）和 `scheduled_execution_recovery_interruption_conflict`（ledger，原来的 `TBD` 换成了说明）。本版的发布
  文件是空的：没有新定稿或退役的条目。
- 程序文件：workspace 仍是 39 个成员，`Cargo.toml` 没改；`Cargo.lock` 只多一行（`crates/lab` 在 Windows 上依赖工作区里的 `actingcommand-host-metrics`），
  host-metrics 多开 windows-sys 的一个功能 `Win32_Security_Cryptography`，没有新的第三方依赖。`.github/`、`ci/`、`tools/`、`ratchet/` 自 v0.12.50 都没改，
  两个包的文件清单和 v0.12.50 一样。FLK-10 照旧在不稳定表里（`ci/flaky-tests.toml`）：`protocol_goldens_match_current_cli` 仍不在 `host-lab` 门禁里、在
  不作门禁的 `flaky` 作业里跑，看稳了再拿掉。`apps/actingctl`、`apps/mcp-entry`、`crates/lab-ops` 自 v0.12.50 没改，MCP 工具仍是 41 个；actinglab 仍是
  100 条命令、45 个顶层分派臂。

升级与退回（安装工具的代码 `crates/lab-rescue/src/install/` 自 v0.12.50 没改，产品代码就是 v0.12.48 到 v0.12.50 说明核过的那份，下面的行号也是它的）：

- **v0.12.50 的安装工具装本版，不用任何单向标志。** 两边的声明逐字相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way`（7 不大于 7）都过
  （`plan.rs` 288–301），计划里没有 `ledger_verify`（915）；`next_max_in_installer`、`installer_meets_previous`、`installer_meets_next`（302–331）、
  `bundle_max_in_next`（332–355：目标仍是 [3, 8]，和装 v0.12.50 时一样；生产根的登记不记范围，每个游戏根只给 `pack_declaration_unknown` 警告）和
  `installer_knows_keys`（359–376）都和装 v0.12.50 时一样；`next_check_config`（902–914）：本版的 check-config 和 v0.12.50 是同一份代码。
- **退回 v0.12.50：安装工具允许，不用标志。** 接口边两边相同、都过（`package` 两边都是 [3, 8]，声明 8 的包也过）；退回时 `next_check_config` 不算接口边
  （`plan.rs` 1030–1036）。部署的自动回退（`deploy.rs` 778）和 `install undo`（`undo.rs` 388）都是 `accept_ledger_one_way: false`，和 `rollback` 走同一组边
  （`rollback_edges`，`plan.rs` 383–403）和同一个回退门（`update.rs` 383–505）：本版写的记录没有新形状，`policy_settlement_dispatch_closed` 是已有的
  `runtime.failed` 记录里的一个新码，v0.12.50 的读者只核拼写；开启时间和配置两版的 actingd 和配置代码相同。注意：部署时新版的部署后检查不过的话，apply 会
  自动退回它替换的版本，也就是 v0.12.50（`deploy.rs` 717–790）。
- **退回以后 v0.12.50 在启动时读到的**（登记 §1.296 的 “A rollback”，第二次审查核过）：
  - **了结那次启动完整走完以后**，v0.12.50 能起来、能调度，留着这次了结，对已关的派单什么都不报；值记录和事实的形状不变，`policy_settlement_dispatch_closed`
    对它是一个不处理的码。这一对后来没再跑过的，或者任务不声明补救、不提供能力的，v0.12.50 读到的最近一次和本版相同（v0.12.50 的 `actingd suspended`
    按记录顺序列，会把迟到的那次中断列成这一对最近一次失败）。
  - **例外：任务声明了补救或提供了能力，而且这一对在被切断那次之后又跑过。** 这种任务的值记录也点名迟到的那次完成；v0.12.50 按完成顺序取一对的最近一次
    （v0.12.50 的 `crates/runtime-host/src/policy_host.rs` 4344–4360 的 `outcome_key_snapshot`、4430–4449 的 `latest_completed_policy_runs`），就把迟到的了结
    读成这一对最近一次：**这一对的结果键为空，直到它下一次完成**，引用这个结果的规则在那之前按最近一次被中断来判；失败计数照旧对（迟到的完成留的是后来那次的
    计数）。
  - **一条边：了结那次启动在「了结写了、值记录还没写」之间被切断，接着起的是 v0.12.50。** 那次能起来：它按旧规则把迟到的执行当成这一对最近一次失败，值记录
    也点名它。这一对后来跑过的话，**再下一次启动，不论哪个版本，都以 `runtime_fact_stale` 失败（致命）**：它只加载迟到的那次，把那次的了结事实写在后来那次
    更新的事实上面（`runtime_facts.rs` 811–821，`precheck_runtime_fact` 1446–1455）；只有这一对在中间又完成一次运行才避得开。这一对后来没跑过的，不会失败。
    审查另记了一条：了结那次启动切在执行记录和完成之间、接着起 v0.12.50，派单又留成开着、每次启动报 Warning，不致命。
  - **运维规矩（总览定，2026-11-11）：部署 0.12.51 以后，在 0.12.51 有一次启动在了结之后完整走完以前，不退回 v0.12.50；头一次启动被切断，就再起一次
    0.12.51。**
- 配对：
  - **v0.12.50 的安装工具装本版**：照常，不带标志，见上面。部署本版跑的是 v0.12.50 的安装工具。
  - **部署工具包的 `actingctl install` 命令行对本版的根答得一样**：工具包用的 `release stage`、`guards --for stop`、`update plan [--preview]`、
    `update apply … --start --detach`、`status [--operation]`、`release-hold` 的代码自 v0.12.50 没改，`crates/lab-rescue/src/install/gate_434_lab3.rs` 416–692
    照旧逐条钉住。工具包核新版 actinglab 用的 `--json standard-pack check` 的代码也没改，RD-BUDGET-1 的读取器不在这条路上。
  - **控制台 UI**（最新发版 UI v0.12.15 的提交就是 main `bb0fd1c7`，它把 Runtime crate 钉在 `d06910c6`，即 v0.12.47，`Cargo.toml` 39–44；
    `crates/acui-setup/component-interfaces.json` 声明 `actingd-config` [5, 27]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：本版的
    声明、线上请求和账本形状都和 v0.12.50 一样，控制台对本版照常。它跑的 actingctl 命令 `packs`、`watchdog status --root`
    （`crates/acui-app/src/workbench/tools.rs` 383、415）和安装器的 `install-transition`、`status`、`request-shutdown`（`crates/acui-setup/src/lifecycle.rs`
    81、207、220）都没变，这两份文件自 v0.12.50 说明核过的 UI v0.12.13 没改。账本里新写的 `policy_settlement_dispatch_closed` 不在它钉的 v0.12.47 码目录里：
    它钉的账本 crate 只核拼写、读得了，控制台照原样显示码名、不配说明（`crates/acui-rows/src/outcome.rs` 160–176）。UI 仓的建模工具 acmodel 从 UI v0.12.15
    起两种回答都读（`model/src/acmodel/export/validator.py` 67–78），v0.12.50 说明里它答 `export_validator_missing` 的那条已经不成立；它只读 `entries` 里那个
    文件的 `status`，不读 `read_bytes`（102–112）。

运行上的变化：

- 部署本版的头一次启动了结那条开着的派单（两条了结记录加一条 `policy_settlement_dispatch_closed`），每次启动的 `policy_settlement_dispatch_left_open` 停掉；
  这一对不再算在飞，资格年龄重新累计，失败状态不变。其余调度决定不变。
- `actinglab resource validate` 只因重复计数被拒的全量扫描现在能过；`actinglab detect` 不再起 PowerShell。别的命令答法照旧。

本版合并的 PR：

- #1563 FLK-10（#71，登记 §1.294）：host-metrics 新加 `probe_process_creation`、`dns_host_name`、`fill_system_random`，`crates/lab` 的结果锁改用它们；起子进程
  的执行器和 30 秒时限删掉。新门行 `#71 FLK-10` 的 1 个测试（`flk10_windows_holder_start_is_read_from_the_system_as_dotnet_ticks`）；删掉不在门行里的
  `windows_probe_timeout_terminates_helper`（它测的执行器没了）。没有新码，不升任何接口修订。
- #1565 RD-BUDGET-1（#428，登记 §1.295）：`DeclarationReader` 留下每个文件的字节，每个路径只在第一次读时计数。新门行 `#428 RD-BUDGET-1` 的 4 个测试
  （`gate_428_rd_budget1`）。没有新码，不升任何接口修订。
- #1564 UNSET-2（#428，登记 §1.296；`contracts/scheduling/README.md` 的 “Failures (#419)”；LN4-6、LN4-7、LN4-8 原地改正）：账本的检查（改 #670 的 R4）、
  `policy_settlement_dispatch_closed`、迟到的了结不算失败、一对的最近一次按准入顺序取、派单日志的修剪留着迟到的完成。账本门行 `#428 UNSET-2` 的 2 个测试
  （其中 `interrupted_policy_execution_recovery_accepts_effects_and_rejects_duplicate_release` 是钉旧规则的那个，有意改造并改名），主机门行 `#428 UNSET-2`
  的 7 个测试（`policy_admission`）。新码 1 个，不升任何接口修订。

## v0.12.50

本版是 v0.12.49 之后合并的 4 个 PR：#434 LAB-2b（#1556）、#428 S168-5a（#1558）、#428 TB-R1（#1561）和 #428 S168-4（#1560）。先说部署：`package` 从
[3, 7] 升到 [3, 8]，其余接口声明都不变（账本仍是 [3, 7]），部署不用带 `--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.49 的安装工具装
本版照常；装上以后能退回 v0.12.49，不用带标志，只要还没有包声明 `package` 8（见下面的退回）。没有新键，默认值文件没改。**actinglab 自己的命令改印新的输出行，
调 actinglab 的脚本要先改**；MCP 的 actinglab 子进程照旧。运行上要知道的是这几处：

- **调 actinglab 的脚本要改什么**（LAB-2b，登记 §1.287；契约 `lab-two-entries.md` §9.3 和未决第 10 条）。actinglab 自己的命令行不带 `--via` 时，印的是 CLI
  模块的行（`apps/actinglab/src/main.rs` 243–261，`apps/actinglab/src/cli_module/own_line.rs`）；除了 `status` 和 `run show`（下一条），命令的解析和结果本身没变：
  - **带 `--json` 是一行 JSON，顶层有 `outcome` 对象的就是新行。** 旧信封的 `schema_version`、`cli_version`、`runtime_version`、`ok`、`command`、`data`、
    `error` 都不再出现。
  - **成功时命令的成员直接在顶层，不再放在 `data` 下**，旁边是 `outcome`（`lab_operation_completed`，category `success`），退出码 0（`own_line.rs` 114–135）。
    结果不是 JSON 对象时放在 `result` 下；结果自带的 `warnings` 照旧是它自己的成员（`{code, values}`），不改变 category。顶层本来就有 `outcome` 的两个结果整份
    套进一个成员：`resource verify` 的报告在 `report` 下，`run summary` 的文档在 `summary` 下（61–62）。
  - **失败时读 `outcome.code`、`message` 和 `details`，退出码是 1**（以前按错误类是 2 到 6）。`outcome` 的 category 是 `error`，`values` 取 `details` 里同码
    结论的值，否则取码目录给这个码登记的那些成员；`details` 整份原样，没有时不出现；旧的 `blocked_by` 没有了（`own_line.rs` 137–211）。**失败现在还往 stderr
    写一行** `ERROR actinglab: <code> … <message>`，JSON 行照旧在 stdout（`apps/actinglab/src/cli_module.rs` 389–406）：把 stderr 并进 stdout 再解析的脚本要把
    两者分开。退出码 2 只给 fatal 的结论，现在没有命令会这样。安装选择在命令运行前就拒绝时，stdout 印 `{outcome}`、stderr 写 `ERROR actinglab: <code>`、退出 1
    （`own_line.rs` 88–100），以前是 stderr 写 `FATAL actinglab: …`、stdout 什么都不印。
  - **不带 `--json` 一律印给人看的文字，进管道或文件也一样**；以前 stdout 不是终端时默认印信封。要读 JSON 的脚本必须带 `--json`。
  - **带 `--via` 的行不变**：只有 MCP 的 actinglab 子进程发（`--json --via mcp …`），照旧印信封，逐字节相同，退出码 0 和 2 到 6，到 LAB-5 为止（`main.rs`
    257–290）。
  - 部署工具读的两条命令（成员按代码核过，`apps/actinglab/src/tests/gate_434_lab2b.rs` 376–479 逐个断言它们在顶层）：
    - `actinglab --json standard-pack check --root <游戏根>`：成功行的顶层是 `status`（`checked`）、`root`、`game`、`root_files`、`pack_count`、`warnings`，
      有 `schedule.json` 时还有 `benefit_coverage`，再加 `outcome`（`crates/resource-tooling/src/api.rs` 116–142）。
    - `actinglab --json standard-pack compose --root <包根> --instance <别名>=<游戏>/<区服> … --out <新目录>`：顶层是 `status`（`composed`）、`out`、
      `catalog_hash`、`rules_digest`、`tasks`、`pools`、`profiles`、`events`、`games`、`manifest`、`warnings`，再加 `outcome`
      （`crates/resource-tooling/src/standard_pack_schedule.rs` 70–85）。
    - 两条都把结果原样摊到顶层（`apps/actinglab/src/standard_pack_cli.rs` 49–52、93–98）。失败时读 `outcome.code`；带自己结论的拒绝把结论放在
      `details.outcome` 里（`crates/resource-tooling/src/standard_pack_check.rs` 1034、1046、1074，`standard_pack_schedule.rs` 428），这个结论的值也在行的
      `outcome.values` 里。
  - 其余：`capabilities` 不带 `--via` 时报的 `exit_codes` 是 `ok`、`error`、`fatal`（0、1、2）；`serve --stdio` 里 actinglab 自己的命令也答新行；设了
    `ACTINGCOMMAND_DETAIL=codes` 时新行的 `outcome` 不带 `values`，读值的脚本不要设它。Lab 救急模块的 `place` 跑 `--json standard-pack check` 只看退出码是不是
    0，不受影响（`crates/lab-rescue/src/install/place.rs` 909–935）。
  - 已知还读旧信封的：UI 仓的建模工具 acmodel 校验导出的表时跑 `--json resource validate`、不带 `--via`，读 `ok` 和 `data.entries`
    （`model/src/acmodel/export/validator.py` 61–97）。它配置的 actinglab 换成本版以后，每次导出表都答 `export_validator_missing`（`answer_invalid`），要等它改读
    新行。资源仓的 CI 和脚本在把 Tools 钉升到本版的那个 PR 里一起改读法（#1556 的 “At deploy”）。
- **`actinglab status` 没有 Runtime 在跑时退出 0**（LAB-2b 修复 3，总览认的）：**判断 Runtime 在不在跑读 `daemon` 一栏，不看退出码。** `status` 和 `run show`
  现在是 CLI 模块的核心行，actinglab 自己的旧实现删了（`main.rs` 326 起的自有命令表里没有它们）。没有 Runtime 时答 `runtime_overview_reported`，`daemon` 是
  `{"online": false, "error": …}`（`runtime_unavailable`），退出 0；有 Runtime 时是 `{"online": true, "owner_epoch": …}`（`crates/lab-ops/src/overview.rs`
  308–327、404–407）。以前是 `runtime_not_running`、退出 5。actinglab 的全局选项（比如 `--runtime-endpoint <端点>`）写在这两条命令的词前或词后，都按名拒绝
  （`actingctl_usage_invalid`，退出 1），什么请求都不发（`cli_module.rs` 118–127、154–165）。
- **`package` 8：包从本版起可以声明的两样**（S168-4、S168-5a，登记 §1.291；`contracts/component-interfaces.md` 152）：
  - 任务步骤 `read_list`（任务 schema `0.9` 的页图任务）：一次扫描把一页上的整张清单读全，运行成功结束时发布成 `inventory.list.` 下的一条 `record_list`
    事实，行按键排序，最多 256 行。只有整张读全、和结束检查通过的那一帧对上、运行成功，才发布；读漏、键重复、超过 256 行都不发布，记警告
    `contained_task_resource_reading_unresolved`，运行照常；扫描失败则运行失败（位置 `read_list`），也不发布。一个锚点都没匹配到时是空清单，发布零行，
    所以包的首尾检查要写得够具体（`contracts/resource-readings.md` 的 “List readings”）。
  - 选择文档的值来源 `table_max_over_list`：一张标准包表某一列在 1 到 16 张记录清单的键的并集上、钉住的类别里的最大值，连同持有它的最小的键。清单不可用、
    缺键或格子为空就是未知，从不当成 0 或 `empty`，按规则声明的办法处理（剔除候选或中止评估）；每个引用在任何候选之前只读一次，记一条审计理由
    `selection_table_list_max`（`crates/selection-policy/src/list_max.rs`；`contracts/selection-policy.md` 的 “Table maximum over a list”）。调度的目录编译拒收它，
    只能在 select 步里用。
  - **已发布的包还没有一个声明它们**：四个资源仓的 main 里都没有 `read_list`、`table_max_over_list`，也没有落在 `inventory.list.` 下的键，所以部署本版没有运行
    会变。用了其中一样的包，bundle 声明 `package` [8, 8]。
  - **更早的 Runtime 遇到这样的包**：这个包的每次运行在准入时被拒，别的包照常。v0.12.49 的准入先跑声明门（v0.12.49 的
    `crates/execution-kernel/src/contained_task.rs` 3195–3200），声明门的操作成员表里没有 `read_list`（v0.12.49 的
    `crates/pack-containment/src/source/declarations.rs` 471–519、2566–2584），拒成 `resource_declaration_invalid`（`UnknownField`，`/operations/<i>/read_list`）；
    读 `table_max_over_list` 的文档，v0.12.49 的值来源枚举里没有这一种（v0.12.49 的 `crates/selection-policy/src/schema.rs` 233–234），拒成
    `contained_task_select_policy_invalid`。检查这样的包也要用本版或更新的 actinglab。
- **`inventory.list.` 只归清单读数**（S168-4）：能发布事实的来源（Agent、Adapter 和 Runtime 自己的普通路径）发布这个前缀下的键，一律拒成
  `list_reading_key_reserved`（`fact_key`，不致命），什么都不写（`crates/runtime-host/src/host/facts.rs` 390–405）；CLI、UI 自己的来源照旧在组请求时就拒
  （`invalid_agent_dispatcher_origin`）。读数 mark 和调整的 `fact_key`、mark 的 `capacity_fact_key` 落在这个前缀下，在声明门和准入时被拒
  （`crates/actingcommand-contract/src/taskflow.rs` 1894–1898）。**被拒的 mark 的准入细节从本版起是 `<id>:<reason>`**：
  `contained_task_resource_reading_invalid` 按 mark 自己的规则拒绝时，细节前面多了 mark 的 id（`crates/execution-kernel/src/contained_task.rs` 8990–9011），
  以前只有 `<reason>`；几个 mark 合起来的规则（个数、重复的 id 或键）照旧只有 `<reason>`。
- **读表时跳过就地替换留在 `tables/` 里的名字**（TB-R1，登记 §1.293）：以 `.part` 结尾，或含 `.old-`、`.abandoned-`、`.rolledback-` 的名字不读、不出行、
  不记启动记录、不占 64 张的名额（`crates/actingcommand-contract/src/package.rs` 223–228，`standard_pack_table.rs` 515–519）。启动、check-config、
  `actingctl packs`、`ac_packs`、运行准备读单张表和 `actinglab resource validate` 都这样读。以前游戏根里留着旧表副本（比如替换时留下的
  `<表>.json.old-<毫秒>`）时，每次启动都为它报 `standard_pack_table_invalid`；从本版起不再报。真表无效、别的名字（比如 `notes.txt`）照旧报；Runtime 不删不挪
  任何文件。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place` 或
  `config replace`。`crates/lab-rescue` 自 v0.12.49 只改了一个测试文件（`install/gate_434_lab3.rs`，TB-R1 的放置断言）。

配置：接口修订只升 `package` 一处，按代码和 `contracts/component-interfaces.md` 核过：`package` [3, 7] 改为 [3, 8]（S168-5a 先合，按代码收它的成员、声明不动；
S168-4 后合，声明 [3, 8]；一次发版只升一次，登记 §1.291）。`distribution/windows/component-interfaces.json`、合同表 `package` 8 一行（152；计划段 300–305 写明
这一行从 0.12.50 起生效，`package` 9 和 `ledger` 8 仍是计划）和架构测试 K27（`tools/actinglab-architecture/tests/component_interfaces.rs` 130）三处一致。其余都
不变：`actingd-config` 仍是 [5, 27]，`ledger` 仍是 [3, 7]，`runtime-client` 仍是 [3, 6]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，
`runtime-outcomes` 仍是 [1, 1]。

- 不升 `ledger`：`crates/ledger` 和合同 crate 的事件部分自 v0.12.49 都没改。清单是已有的 `record_list` 值的 `fact.published`，清单最大值的记录是一条决策理由，
  清单的读漏是已有警告行里 `reading_problem` 的新词和已有码的可选值 `count`，读者对码和键只核拼写（`component-interfaces.md` 311）；SQLite 格式 2、
  `actingcommand.event.v3`、表、索引和视图都没变。
- 不升 `runtime-client`：`crates/runtime-client` 自 v0.12.49 没改，请求和回执的形状没变；`list_reading_key_reserved` 是已有的发布请求多一种拒绝码。
- actinglab 的输出行没有接口行（LAB-2b 核过 `component-interfaces.md`），MCP 子进程的信封不变，不升任何修订。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`apps/actingd`、`actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.49 都没改。
- 结果码：目录从 3897 个码、1747 个位置变成 3899 个码、1748 个位置，仍是 29 个分片，没有退役的码。新码 2 个（都待定稿，since 0.12.50）：
  `selection_table_list_max`（selection-policy，info）和 `list_reading_key_reserved`（runtime-host，error）。新位置 1 个：`read_list`（execution-kernel）。新键
  3 个：`fact_keys`（`list<token>`）、`maximum`（`integer`）、`maximum_identity`（`id`）。词表 `reading_problem` 多 3 个词：`list_position_unread`、
  `list_duplicate_key`、`list_rows_exceeded`。`contained_task_resource_reading_unresolved` 多可选值 `count`（已有的键）；只改说明的：`lab_operation_completed`、
  `lab_operation_check_failed`。结果码守卫的允许表多一条（G5：声明门的 `read_list_member` 和位置 `read_list` 同名）。本版的发布文件是空的：没有新定稿或退役的
  条目。
- 程序文件：workspace 仍是 39 个成员，`Cargo.toml`、`Cargo.lock` 自 v0.12.49 都没改；`.github/` 只改了 `.github/workflows/ci.yml`，它和
  `ci/flaky-tests.toml` 一起把 FLK-10（actinglab 的 `golden_protocol::protocol_goldens_match_current_cli`）从 `host-lab` 门禁挪进不作门禁的 `flaky` 作业（发版
  PR #1562 里改的），构建和两个包不受影响，两个包的文件清单和 v0.12.49 一样。在读进程启动时间的 PowerShell 检查改好以前，`actinglab detect` 的环境探测在负载很重
  的机器上可能失败，报 `owner identity unavailable: PowerShell process inspector … timed out`，这时还没写结果和锁，再跑一次是安全的。`apps/actingctl`、
  `apps/mcp-entry`、`crates/lab-ops`、`crates/runtime-client` 自 v0.12.49 没改，MCP 工具仍是 41 个。actinglab 的命令从 102 条变成 100 条、顶层分派臂从 46 个
  变成 45 个（`ratchet/actinglab_commands.json`）。

升级与退回（安装工具的代码 `crates/lab-rescue/src/install/` 自 v0.12.49 只改了测试文件 `gate_434_lab3.rs`，产品代码就是 v0.12.48、v0.12.49 说明核过的那份，
下面的行号也是它的）：

- **v0.12.49 的安装工具装本版，不用任何单向标志。** `ledger` 两边都是 [3, 7]，`previous_max_in_next` 和 `one_way`（7 不大于 7）都过（`plan.rs` 288–301），
  计划里没有 `ledger_verify`（915）；`next_max_in_installer`、`installer_meets_previous`、`installer_meets_next`（302–331）和 `installer_knows_keys`（359–376）
  和装 v0.12.49 时一样，没有新键；`next_check_config`（902–914）：本版的 check-config 收现行配置，读表只多跳过遗留副本。
- **`package` 的边**：`bundle_max_in_next` 拿包登记里每个游戏根声明的范围的上限，看它在不在目标的 `package` 里（334–355）。生产根是单一位置根，包登记由
  `adopt` 写，不记声明的范围（`adopt.rs` 129–135），所以每个游戏根都只给 `pack_declaration_unknown` 警告（338–344），v0.12.49 的部署预览就是这样；登记里记着
  范围的，今天的包最多声明 7，在 [3, 8] 里，过（345）。没有一条边不过。
- **退回 v0.12.49：允许，不用标志，只要还没有包声明 `package` 8。** 退回时目标的 `package` 是 [3, 7]：登记不记范围的游戏根照旧只给警告，记着的最多 7，过；
  其余的边两边相同；退回时 `next_check_config` 不算接口边（1030–1036）。部署的自动回退（`deploy.rs` 778）和 `install undo`（`undo.rs` 388）都是
  `accept_ledger_one_way: false`，和 `rollback` 走同一组边（`rollback_edges`，`plan.rs` 383–403）和同一个回退门（`update.rs` 383–505）。落盘的东西没有
  v0.12.49 读不了的：没有新键、新文件、新记录形状，没有包声明清单时也没有清单事实。退回以后 `tables/` 里的遗留副本又会在每次启动报无效，actinglab 回到旧信封。
- **有包声明 `package` 8 以后**：登记里记着这个包 [8, 8] 的，`bundle_max_in_next`（8 不在 [3, 7] 里）不过，`rollback`、自动回退和 `install undo` 都以
  `interface_edge` 拒绝（1037–1041）。生产根的登记不记范围，安装工具只给警告、照样退回，v0.12.49 随后在准入时拒绝这个包的每次运行（见上面的 `package` 8）；
  已经发布的清单是 `record_list` 值，v0.12.49 读得了。所以退回之前，先把声明 8 的包换回最多声明 7 的版本。
- 配对：
  - **v0.12.49 的安装工具装本版**：照常，不带标志，见上面。部署本版跑的是 v0.12.49 的安装工具。
  - **部署工具包的 `actingctl install` 命令行对本版的根答得一样**：`release stage`、`guards --for stop`、`update plan [--preview]`、
    `update apply … --start --detach`、`status [--operation]`、`release-hold` 的代码没变，`crates/lab-rescue/src/install/gate_434_lab3.rs` 416–692 照旧逐条钉住
    （TB-R1 只在文件头的注释里加了一行，这个测试没动）。工具包不调 actinglab。
  - **MCP 和它的 actinglab 子进程**：MCP 跑的每个 actinglab 子进程都由 `crates/lab-ops/src/lab_child.rs` 70–78 组命令行，参数一律以 `--json --via <计数方>` 开头；
    MCP 构造它的唯一一处给的是 `mcp`（`apps/actingctl/src/mcp/lab.rs` 66、771–778；`ac_app` 也走它，1214）。所以 MCP 发的每一行都带 `--via`，信封和退出码
    不变；`apps/actingctl`、`crates/lab-ops` 自 v0.12.49 没改，MCP 工具的答法照旧。
  - **控制台 UI**（最新发版 UI v0.12.13 的提交就是 main `2f4a0cd7`，它把 Runtime crate 钉在 `d06910c6`，即 v0.12.47，`Cargo.toml` 39–44；
    `crates/acui-setup/component-interfaces.json` 声明 `actingd-config` [5, 27]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]，`package`
    [1, 2] 不是控制台的边）：本版这几项和 v0.12.49 一样，线上请求和账本形状也一样，控制台对本版照常；控制台不准入包。控制台本身不跑 actinglab：它跑的
    actingctl 命令 `packs`、`watchdog status --root`（`crates/acui-app/src/workbench/tools.rs` 383、415）和安装器的 `install-transition`、`status`、
    `request-shutdown`（`crates/acui-setup/src/lifecycle.rs` 81、207、220）都没变，只在核 Tools 文件清单时列出 `actinglab.exe`（`crates/acui-setup/src/verify.rs`
    38）。同一个仓里的建模工具 acmodel 跑 actinglab 不带 `--via`、读旧信封，见上面。

运行上的变化：

- 部署本身不改变调度的决定，现有的包运行结果不变（还没有包声明 `package` 8）；游戏根 `tables/` 里的遗留副本不再在每次启动报无效。
- actinglab 自己的命令改印新行（`--json` 的成员在顶层、失败退出 1 并往 stderr 写一行、不带 `--json` 进管道也是文字），`status` 没有 Runtime 也退出 0；MCP 和
  actingctl 的答法照旧。

本版合并的 PR：

- #1556 LAB-2b（#434，登记 §1.287；契约 `lab-two-entries.md` §8.2、§9.3 和未决第 10 条）：actinglab 自己的命令印 CLI 模块的行（新文件
  `apps/actinglab/src/cli_module/own_line.rs`），带 `--via` 的行保留信封到 LAB-5；`status`、`run show` 交给核心行，旧实现删掉，命令从 102 条变成 100 条；
  `resource verify`、`run summary` 的结果整份放在 `report`、`summary` 下；没有新码。新门行 `#434 LAB-2b` 的 6 个测试（`own_commands_print_the_cli_line`、
  `own_commands_print_for_people_into_a_pipe`、`mcp_child_lines_keep_the_envelope`、`status_and_run_show_take_their_core_rows`、
  `own_results_never_name_outcome`、`do_check_failure_exits_one`）；金样 `expected.json` 的 30 例和 `consumers.json` 的 13 例照新行重写（MCP 的 2 例一个字节没变），
  改造的测试改读新行。不升任何接口修订。
- #1558 S168-5a（#428，登记 §1.291）：值来源 `table_max_over_list`（新文件 `crates/selection-policy/src/list_max.rs`），审计理由 `selection_table_list_max`，
  新键 `fact_keys`、`maximum`、`maximum_identity`；新门行 `#428 S168-5a` 的 9 个测试（`gate_428_s168_5a`）。`package` 8 的两片之一，先合，声明保持 [3, 7]。
- #1561 TB-R1（#428，登记 §1.293）：合同 crate 的 `replacement_leftover_name`，读 `tables/` 的三处跳过遗留名字；新测试
  `gate_357_db1::table_listing_skips_replacement_leftovers`（门行 `#428 TB-R1`），改造 `gate_434_lab3::place_holds_only_the_bundles_idle_instances`（放置和
  `undo` 之后都只列出那一张表）。不升任何接口修订。
- #1560 S168-4（#428，登记 §1.291）：任务步骤 `read_list`（新文件 `crates/execution-kernel/src/contained_task/list_reading.rs`），声明门、准入、内核读和宿主
  发布；保留前缀 `inventory.list.`、新码 `list_reading_key_reserved`、位置 `read_list` 和 `reading_problem` 的三个词；被拒 mark 的细节 `<id>:<reason>`；
  `package` [3, 8]。新门行 `#428 S168-4` 的 10 个测试（内核、声明门和宿主三处）；改造 K27（`package` [3, 8]）。

## v0.12.49

本版是 v0.12.48 之后合并的 2 个 PR：#422 QP-1（#1557）和 #428 S168-0（#1554，只有契约）。先说部署：接口声明都不变（账本仍是 [3, 7]），部署不用带
`--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.48 的安装工具装本版照常；装上以后能退回 v0.12.48，不用带标志，落盘的东西
v0.12.48 都读得了。没有新键，默认值文件没改，没有新增或退役的结果码。运行上要知道的是这几处：

- **账本的页不管规划器怎么选，都留在序号界内**（QP-1，登记 §1.292）。页语句里的时间条件一律写成不能走索引的 `+e.timestamp_unix_ms`，带不带逐 run
  链接都一样；不带逐 run 链接的语句，窗口自己的序号条件也写成 `+e.sequence`，只有段界能驱动语句（`crates/ledger/src/global/sqlite.rs` 4759–4772；
  LN-8d 的链接集页照旧认这种写法，4873）。
  - 答案不变：读预算没截断的页，行、顺序、游标、读范围和续读都和以前一样。带时间条件的页可能换一个计划，再也不会顺着时间索引把窗口之前的历史
    整段读一遍（调查在 40 万条记录上量过：一页约 6,500 步，强制走时间索引约 1,800 万步）。
  - 读预算截断的页，停在哪里取决于计划花了多少工作量，可能和以前停在不同的地方，给的是那个地方的行、游标、读范围和续读。
  - 窗口不约束的两种读没变，现在写进了契约（`contracts/ledger-observer.md` 1491–1510）：Lab 关系的补读（从关系索引的末尾读到钉点；索引没建好时读
    整段历史）和 run 恢复（每个 run 经 `ledger_links_run` 从序号 0 读，最多 1,025 条）。Runtime 不限制窗口的宽度。
  - **控制台只有对本版或更新的 Runtime 才可以在线发时间条件。** 控制台的账本窗口现在不发：UI main（`a56640c2`，账本窗口片 1，还没发版）在线不带
    时间条件，发出前就拒（`crates/acui-source/src/lib.rs` 649–682）；最新发版 UI v0.12.11 的账本窗口在线仍可能带上时间条件（`crates/acui-model/src/lib.rs`
    89–101），对本版答案不变，只是不再走时间索引。
- **S168-0 只是计划中的契约文字**（登记 §1.291）：清单读数写成 `record_list` 事实、清单上的表最大值、不做输入的判断步骤，三片的契约；没有代码、测试、
  门行、键或码，构建里什么都没变。`package` 8、9 和 `ledger` 8 是计划行，没有生效（`contracts/component-interfaces.md` 297–323）。
- **LAB-3 的运维规矩到 LAB-7 前照旧：生产照旧用 `actingctl install` 部署**，不对生产根用 Lab 救急模块的 `deploy`、`rollback`、`place` 或
  `config replace`。`crates/lab-rescue` 自 v0.12.48 没改：模块写的日志 v0.12.48 和本版读得了，v0.12.47 及更早的仍读不了。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过。`distribution/windows/component-interfaces.json` 自 v0.12.48 没改：`actingd-config`
[5, 27]、`ledger` [3, 7]、`runtime-client` [3, 6]、`package` [3, 7]、`install-selection` [1, 1]、`install-control` [0, 1]、`runtime-outcomes` [1, 1]。

- 不升 `ledger`：QP-1 只改读语句的文字，SQLite 格式 2、`actingcommand.event.v3`、表、索引、视图和记录形状都没变。不升 `runtime-client`：
  `crates/runtime-client` 和合同 crate 自 v0.12.48 没改，请求成员和答复都没变。`component-interfaces.md` 只多了 #434 计划段里的一句（274–275）和
  S168 的计划段（297–323），都不生效。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`apps/actingd`、`actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.48 都没改；QP-1 的契约提到的
  `history_scan_rows`、`history_page_rows` 都是已有的键。
- 结果码：没有新码，也没有退役的码。`contracts/outcome-codes/` 自 v0.12.48 没改，目录仍是 3897 个码、1747 个位置，29 个分片；本版的发布文件是空的。
- 程序文件：只有 `crates/ledger` 改了（`sqlite.rs` 和两个测试文件）。`Cargo.toml`、`Cargo.lock`、`.github/`、`apps/`、`tools/` 自 v0.12.48 都没改，
  workspace 仍是 39 个成员，两个包的文件清单和 v0.12.48 一样；MCP 工具仍是 41 个。

升级与退回（安装工具的代码 `crates/lab-rescue/src/install/` 自 v0.12.48 没改，就是 v0.12.48 说明的“退回”一节核过的那份；行号是它的）：

- **v0.12.48 的安装工具装本版，不用任何单向标志。** 两边的声明逐字相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way`（7 不大于 7）都过
  （`plan.rs` 288–301），计划里没有 `ledger_verify`（915）；`next_max_in_installer`、`installer_meets_previous`、`installer_meets_next`、`bundle_max_in_next`
  和 `installer_knows_keys`（302–376）和装 v0.12.48 时一样；`next_check_config`（902–914）：本版的 check-config 和 v0.12.48 是同一份代码。
- **退回 v0.12.48：允许，不用标志。** 接口边都过，退回时 `next_check_config` 不算接口边（`plan.rs` 1030–1036）；部署的自动回退（`deploy.rs` 778）和
  `install undo`（`undo.rs` 388）都是 `accept_ledger_one_way: false`，和 `rollback` 走同一个回退门（`update.rs` 383–505）：本版不写新的记录形状，
  v0.12.48 的读者读部署之后的账本页（400–437）；开启时间和配置（438–503）两版的 actingd 和配置代码相同。落盘的东西没有 v0.12.48 读不了的：没有新键、
  新文件、新记录形状，`actingctl install` 写的日志和 v0.12.48 出自同一份代码。退回以后，带时间条件的页又可能被时间索引驱动。
- 配对：
  - **部署工具包的 `actingctl install` 命令行对本版的根答得一样**：`release stage`、`guards --for stop`、`update plan [--preview]`、
    `update apply … --start --detach`、`status [--operation]`、`release-hold` 的代码没变，`crates/lab-rescue/src/install/gate_434_lab3.rs` 415–691 照旧
    逐条钉住。部署本版跑的是 v0.12.48 的安装工具。
  - **控制台 UI**（main `a56640c2` 和最新发版 UI v0.12.11 `2461e518` 都把 Runtime crate 钉在 `d06910c6`，即 v0.12.47，`Cargo.toml` 39–44；
    `crates/acui-setup/component-interfaces.json` 声明 `actingd-config` [5, 27]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：
    本版的声明、线上请求和账本形状都和 v0.12.48 一样，控制台对本版照常。它跑的 actingctl 命令 `packs`、`watchdog status --root`
    （`crates/acui-app/src/workbench/tools.rs` 383、415）和安装器的 `install-transition`、`status`、`request-shutdown`（`crates/acui-setup/src/lifecycle.rs`
    81、207、220）都没变。控制台离线读账本用的是它钉的账本 crate，要等它改钉才有 QP-1。

运行上的变化：

- 部署本身不改变调度的决定，命令答法照旧；账本页带时间条件时不再顺着时间索引读窗口之前的历史，读预算没截断的页答案不变。

本版合并的 PR：

- #1557 QP-1（#422，登记 §1.292；契约 `ledger-observer.md` §15 的 QP-1 条、`global-ledger-query.md` 的 QP-1 段）：`history_statement` 的时间条件
  一律带 `+`，不带逐 run 链接的语句窗口的序号条件也带 `+`。新门行 `QP-1` 的 3 个测试（`page_statements_stay_inside_sequence_bounds`、
  `time_bound_never_drives_a_page`、`page_results_unchanged_by_plan_bounds`）在带 STAT4 的 SQLite 上读查询计划、比对分页结果；改造 1 处
  （`gate_diag_run::diag_run_t1_a_per_run_read_is_driven_by_its_run_link_index`：A、C 两种形式从“一个 `+` 都没有”改成“只有时间条件带 `+`”）。
  不升任何接口修订。
- #1554 S168-0（#428，登记 §1.291）：只有契约，7 个契约文件，写下 S168-4 `read_list`、S168-5a `table_max_over_list`、S168-5b `judge` 的计划；没有
  代码、测试、门行、键或码，不升现行修订。

## v0.12.48

本版是 v0.12.47 之后合并的 1 个 PR：#434 LAB-3（#1553）。先说部署：接口声明都不变（账本仍是 [3, 7]），部署不用带 `--accept-ledger-one-way`，
计划里也没有 `ledger_verify` 这一步；v0.12.47 的安装工具装本版照常（见下面的升级）；装上以后能退回 v0.12.47，不用带标志（Lab 救急模块写的安装日志
v0.12.47 读不了，所以 LAB-7 之前生产不用它部署，见下面的退回）。没有新键，默认值文件没改。运行上要知道的是这几处：

- **Lab 有了救急与安装模块**（LAB-3，登记 §1.287）。`actinglab` 的命令行和 `actinglab serve --stdio` 多了操作表 `rescue` 模块的 29 行
  （`crates/lab-rescue/src/rescue.rs` 76–106）：`up`、`down`、`restart`、`alive [--wait]`、`deploy <tag|commit>`、`rollback`、`undo`、`op status|resume`、
  `leftovers`、`guards`、`release-hold`、`release verify|stage`、`adopt`、`config replace|restore`、`place <bundle.zip>`、`unlock`、`check-config`、
  `ledger verify`、`watchdog install|uninstall` 和 `agent …`。
  - 它作用在 actinglab 所在的根上，不收 `--root`、`--state-root`：`<root>\tools\actinglab.exe deploy …` 部署的就是这个根，演练要用演练根自己的 actinglab。
  - 组合操作一次调用做完：`deploy` 是 stage、plan、带暂停停机、apply、启动、等 `alive`、交还暂停；`config replace <file>` 先用现行构建的 check-config
    核新文件，再停机、换文件（换下的留作 `actingd.config.json.prev-<ms>`，文件内容不打印）、启动、等 `alive`、交还，`config restore <prev>` 换回；
    `place <bundle.zip>` 暂停绑定这个包的实例、等它们空闲、逐项放置（旧的留作 `.old-<ms>`）、检查、交还、读排程，代码里不写任何游戏；`alive` 是看门狗
    对这个根的 status。
  - **不问确认，记下谁发起的**：`--plan <id>`、`--accept-ledger-one-way`、`--known-pause`、`--ack-guard`、`--ui-confirmed`、`--via` 都答
    `actingctl_usage_invalid`（`unknown`；`rescue.rs` 115–123），`--plan` 在这里只是“只出计划”的开关。模块写的每份安装日志在第一步之前记 `initiator`
    （命令行默认 `Lab-CLI`，`ACTINGCOMMAND_INITIATOR` 可设 `Lab-救急`），撤销记录也记；单向的账本边不再要确认，记 `one_way: true`
    （`crates/lab-rescue/src/install/journal.rs` 93–103、117–135）。`unlock` 把发起者当 actingd 的 `--actor`。
  - **守卫只报不拦**：没过或没评估的守卫给警告 `install_caveat_noted`（`install_caveat` `guard_failed`，`install_guards` 列出），操作照常往下走，日志留着
    这些行（`install/guards.rs` 359–384）；启动前照旧等内存。前置条件、写锁、未完成的操作、接口检查、回退门和输入检查照旧拒绝。
  - 从 `<root>\tools\actinglab.exe` 跑的 `deploy`、`rollback` 按 pid 认出自己的映像，像 `actingctl.exe` 一样挪开继续跑（计划里的 `running.renamed_own`，
    `install/plan.rs` 129–134、936–964）；根下别的 `actinglab.exe`（MCP 的子进程、另一个命令行）照旧拒绝 `running_image_blocks`，先关掉。
- **救急模块只交还自己设的暂停，从不解别人的**（LAB-3 修复 1 至 3）。它的暂停原因是 `lab-<操作>-<毫秒>`（或 `down --reason` 给的）。`release-hold <reason>`
  只认模块自己的日志在暂停那一步记下的原因，只恢复那一步暂停的实例，而且只在实例现在仍显示这个原因时恢复；按原因和别名认，不按修订号，所以 Runtime
  重启过也认（`install/hold.rs` 712–786）。别的原因一律答 `hold_not_owned`（`hold_reason`），什么都不恢复；`down --reason` 给的原因若已在某台实例上，
  也在暂停之前就答 `hold_not_owned`，从不接手已有的暂停（`hold.rs` 788–804）。生产五台实例现有的停调度暂停因此不会被它解开。全局暂停保留并列出，
  停机照常；在动磁盘之前被拒，模块立即交还自己刚设的暂停（`hold.kept: false`）。读不出某个暂停归不归它时，答复的 `next` 里不放这条 `release-hold`，
  并带说明 `hold_ownership_unconfirmed`。`actingctl install release-hold` 和 `install stop` 没变。
- **LAB-7 之前的运维规矩：生产照旧用 `actingctl install` 部署**（总览的部署工具包），不对生产根用模块的 `deploy`、`rollback`、`place` 或 `config replace`。
  模块写的日志带 `initiator`、`one_way`、暂停证据里的 `own_pauses`，种类还可能是 `config_replace`、`pack_place`，v0.12.47 及更早的安装工具都读不了
  （#1553 的 “At deploy”，`contracts/install-cli.md` 的 “The Lab's rescue module” 末条）；生产根上有了这种日志，需要退回 v0.12.47 时它就看不见这些操作
  （见下面的退回）。
- **`actingctl install`、`start`、`restart`、`mcp-entry`、`agent-channel` 没变**：代码原样搬进 `crates/lab-rescue`，actingctl 从那里回答
  （`apps/actingctl/src/main.rs` 88–95；`install` 用 `Face::Install`，`crates/lab-rescue/src/install/mod.rs` 105–138、236–247），选项、输出行、码、退出码和
  写的日志都和以前一样（`install-cli` 修订 1 的面，留到 LAB-7）。部署工具包用的命令行由测试逐条钉住（见下面的配对）。
- **actinglab 从 `<root>\tools\` 跑时认得出自己的根**：`crates/lab-ops` 的定位以前只认 `runtime\` 下的程序，现在 `tools\` 下的也认
  （`crates/lab-ops/src/locate.rs` 113–119）。所以 LAB-2 的短命令从安装根的 `tools\actinglab.exe` 跑，不设 `ACTINGCOMMAND_RUNTIME_STATE_ROOT` 也找得到
  根和状态根（v0.12.47 答 `install_root_unresolved`）。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过：`actingd-config` 仍是 [5, 27]，`ledger` 仍是 [3, 7]，`runtime-client` 仍是 [3, 6]，
`package` 仍是 [3, 7]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。`distribution/windows/component-interfaces.json`
和架构测试 `tools/actinglab-architecture/tests/component_interfaces.rs` 自 v0.12.47 没改；`component-interfaces.md` 只在计划段写下 LAB-3 对 `install-root`
的判定（287–292）。

- 不升 `install-root`：`initiator`、`one_way` 和 `own_pauses` 是单一位置根的日志 `install-operation.v1` 的可选成员，只有救急模块写；`install-root` 管的是
  A/B 根的私有文件（`component-interfaces.md` 158），不含单一位置根的日志。代价是 v0.12.47 及更早的安装工具读不了模块写的日志（总览的裁定，测试阶段不做
  旧版兼容），所以有上面的运维规矩。
- 不升 `install-cli`：`actingctl install` 的面没变；模块的种类 `config_replace`、`pack_place` 是修订 1 的 UI 从来不发起的操作（同上 291–292）。计划中的
  `install-cli` 2 仍跟 LAB-7 走（272）。
- 不升 `runtime-client`、`ledger`：LAB-6 之前模块经 CLI 通道发已有的请求，`crates/runtime-client`、合同 crate 和 `crates/ledger` 自 v0.12.47 都没改，没有
  新记录形状。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`apps/actingd`、`actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.47 都没改，
  部署计划的默认值对比是空的；模块读的 `install.*` 键（如 `install.start.memory_wait_ms`、`install.start.ready_timeout_ms`）都是已有的。
- 结果码：没有新码，也没有退役的码，目录仍是 3897 个码，位置从 1741 个变成 1747 个。新位置 6 个（actingctl 片，待定稿，since 0.12.48）：
  `install_deploy`、`install_config_replace`、`install_alive`、`install_unlock`、`install_check_config`、`install_ledger_verify`。新词 6 个：`install_refusal`
  的 `hold_not_owned`；`install_caveat` 的 `guard_failed`、`hold_ownership_unconfirmed`；`install_operation_kind` 的 `config_replace`、`pack_place`；
  `install_step_kind` 的 `pack_item`。两个待定稿的码多了可选值（都是已有的键）：`install_caveat_noted` 多 `install_guards`、`hold_reason`，
  `install_command_refused` 多 `hold_reason`；只改说明的：键 `hold_reason`、`install_refusal` 的 `instances_not_idle`。结果码目录头的片名多了 `lab-rescue`
  （没有分片文件，不登记码）。本版的发布文件是空的：没有新定稿或退役的条目。
- 账本：不升，没有新记录形状。
- 程序文件：workspace 成员从 38 个变成 39 个（新库 `crates/lab-rescue`，只在 Windows 上编，CI 归 `host-lab` 作业）。它是库，链进 `actingctl.exe` 和
  `actinglab.exe`；`windows-remote-build.yml` 和 `release.yml` 自 v0.12.47 没改，Runtime 包和 Tools 包的文件清单都和 v0.12.47 一样。`Cargo.lock` 只多了
  这个 workspace 包，没有新的外部依赖。`apps/actingd`、`apps/actingwatch`、`apps/mcp-entry`、`crates/runtime-client`、`crates/ledger`、合同 crate 自
  v0.12.47 没改；`crates/watchdog` 只加了给模块用的 `Asked`、`Answered`、`answer` 和两个操作种类，`actingwatch.exe` 的 tick 和 `actingctl watchdog …`
  答法不变。MCP 工具仍是 41 个（`apps/actingctl/src/mcp` 没改）。

升级时要知道的（按安装工具的代码核过；部署本版跑的是 v0.12.47 的安装工具，下面的行号是它的 `apps/actingctl/src/install/`）：

- 不用 `--accept-ledger-one-way`：两边的 `ledger` 都是 [3, 7]，`previous_max_in_next`（7 在 [3, 7] 里）和 `one_way`（7 不大于 7）都过（`plan.rs` 281–294），
  带上也不改判断；现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`（908），apply 不读账本。
- 其余的边都过：两边的声明逐字相同，v0.12.47 安装工具自己的声明 I 编在程序里（`host.rs` 23–24），和本版的声明一样；`next_max_in_installer`（295–307）、
  `installer_meets_previous` 和 `installer_meets_next`（308–323）都过；`package` 两边都是 [3, 7]，`bundle_max_in_next` 在单一位置根上只有
  `pack_declaration_unknown` 警告（325–348）；`installer_knows_keys`（353–369）不受影响，没有新键；`next_check_config`（895–907）：本版的 check-config 和
  v0.12.47 是同一份代码，收现行配置，写了 DirectML 的也一样。
- **v0.12.47 的安装工具没有新东西要发或要读**：两个包的文件清单没变，它照常把新的 `runtime\actingctl.exe` 和 `tools\actinglab.exe` 换上去；计划任务不用
  重新注册。本版的 `actingctl install` 读 v0.12.47 写的日志和 `current.json`：新成员都可缺省（`#[serde(default)]`，`crates/lab-rescue/src/install/journal.rs`
  93–103、117–135），种类和步骤都是旧的超集。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，回退门的配置一项也过（见下面的退回）。

退回 v0.12.47（按本版安装工具的代码核过，行号是 `crates/lab-rescue/src/install/` 的）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。两边的声明相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way` 都过（`plan.rs` 288–301），
  其余接口两边都一样，没有失败的边；退回时 `next_check_config` 不算接口边（`plan.rs` 1019–1034）。部署的自动回退（`deploy.rs` 772–781，
  `accept_ledger_one_way: false`）、`rollback plan|apply` 和起过本版 Runtime 以后的 `install undo`（`undo.rs` 177–191、327–360、376 起）走的是同一组边和
  同一个回退门（`update.rs` 383–505）。回退门其余几项：本版不写新的记录形状，v0.12.47 的读者读部署之后的账本页（`ledger_readability`，400–437）；
  开启时间（438–489）和配置（490–503）两版的 actingd 和配置代码相同。
- **`actingctl install` 写的日志 v0.12.47 照读**：本版的 `actingctl install` 是 `Face::Install`（`mod.rs` 105–138），不写 `initiator`（`None` 不写）、
  `one_way`（`false` 不写）和撤销记录的 `initiator`（`journal.rs` 93–103、117–135）；暂停步骤的证据只在救急面写 `own_pauses`（`deploy.rs` 219–264）；
  计划里的 `running.renamed_own` 为空时不写（`plan.rs` 129–134），`actingctl install` 自己的映像是 `actingctl.exe`，走原来的 `renamed`（936–964）。种类和
  步骤都是 v0.12.47 已有的，所以 v0.12.47 的 `Journal`（`deny_unknown_fields`）照读。测试断言 apply 写的日志没有 `initiator`、`one_way`
  （`gate_434_lab3.rs` 600–606）。
- **救急模块写的日志 v0.12.47 读不了（已知的缺口）**：v0.12.47 的 `Journal`、`UndoRecord` 拒收未知成员（v0.12.47 的
  `apps/actingctl/src/install/journal.rs` 89–94、109–131），`StepKind` 没有 `pack_item`（26–47），`OperationKind` 没有 `config_replace`、`pack_place`
  （v0.12.47 的 `crates/watchdog/src/install_state.rs` 17–29）。v0.12.47 遇到这种日志：
  - 点名读它的命令拒绝：`install resume|undo --operation <id>` 答 `state_invalid`（v0.12.47 `journal.rs` 284–297），`install status --operation <id>` 答
    用法错误（v0.12.47 `status.rs` 326–328）。
  - 遍历所有日志的读者不报错、直接跳过它（旧行为，记作后续）：回退门推算各时段的现行构建（v0.12.47 `update.rs` 215–217）、`status` 的最近操作
    （`status.rs` 229–233）、`leftovers` 的留底归属（`status.rs` 56–60）、`release-hold` 找暂停记下的延后工作（`deploy.rs` 834–870）、单独 `start` 找上次
    停机（`deploy.rs` 1466–1472）。所以这些操作在 v0.12.47 里看不见，它的 `release-hold` 也交不回模块的暂停记下的延后工作。
  - `install\current.json` 若还指着一个没做完的 `config_replace` 或 `pack_place`，v0.12.47 读不了它：`install status` 和每个要开新操作的命令都答
    `state_invalid`（v0.12.47 `state.rs` 254–263、`status.rs` 220–227、`journal.rs` 171–183），看门狗当作有未完成的操作而不动（v0.12.47
    `install_state.rs` 56–81）。
  按 LAB-7 之前的规矩，生产根上不会有这种日志；安装工具不看它们，不会因此拦退回。
- 落盘的东西除了救急模块的日志都不挡：没有新键、新文件、新记录形状。
- 退回以后：actinglab 没有救急的 29 行，从 `tools\` 跑又找不到根（要设 `ACTINGCOMMAND_RUNTIME_STATE_ROOT`）；actingctl、MCP 和看门狗和本版一样。
- 客户端和 Runtime 的配对：
  - v0.12.47 的安装工具装本版：照常，不带标志，见上面的升级。
  - **部署工具包的 `actingctl install` 命令行对本版的根答得一样**：它用的 `release stage --root --dir --facts --expect-tag --expect-commit`、
    `guards --for stop --hold --known-pause`、`update plan [--preview]`、`update apply --plan --stop --hold --known-pause --start [--detach]`、`release-hold`、
    `status [--operation]`，连同脚本读的成员（`guards[].id`、`result`；预览的 `plan_id`、`kind`、`plan` 的 `inputs`、`live` 和 `target` 的 `commit`、
    `speaks`、`edges`、`ledger_verify`、`check_config`、`extra`、`config_sha256`、`files`；`status` 的 `live.commit`、`live.operation_id`、`builds[]`），由
    `crates/lab-rescue/src/install/gate_434_lab3.rs` 415–691 逐条钉住：退出码、`--detach` 的命令行、带 `--known-pause` 的外来暂停原样保留、
    `release-hold` 只放本次暂停的实例、写的日志没有 `initiator` 和 `one_way`、没点名的外来暂停照旧拒绝 `stop`、缺 `--plan` 照旧是用法错误。用法文本
    （`mod.rs` 140）和参数分派与 v0.12.47 相同。部署本版时跑的是 v0.12.47 的安装工具，代码本来没变；下一次部署用本版的安装工具，命令行照旧。
  - **控制台 UI**（UI 仓 main 仍是 `0e324e21`，最新发版仍是 UI v0.12.10 `c03d18ea`；`Cargo.toml` 39–44 仍把合同、账本、取证、`runtime-client`、配置和
    内核 crate 钉在 `a322caa4`，即 v0.12.43；`crates/acui-setup/component-interfaces.json` 仍声明 `actingd-config` [5, 26]、`ledger` [3, 7]、
    `runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：本版的声明、线上请求和账本都和 v0.12.47 一样，所以控制台对本版和对 v0.12.47 一样。它跑的
    actingctl 命令是 `packs`（`crates/acui-app/src/workbench/tools.rs` 378–388）和 `watchdog status --root`（405–415），安装器跑 `install-transition`、
    `status`、`request-shutdown`（`crates/acui-setup/src/lifecycle.rs` 81–85、207、220–221），都不是搬走的命令，答法不变；它不跑 `actingctl install`、
    `start`、`restart`、`agent-channel`，也不跑 actinglab（只在核 Tools 文件清单时列出 `actinglab.exe`，`crates/acui-setup/src/verify.rs` 38）。本版
    没有东西逼控制台换钉。
  - MCP（actingctl 的 `mcp-serve`）和它跑的 actinglab 子进程（`--json --via mcp`）答法不变；救急行拒收 `--via`，MCP 没有调用它们的工具。

运行上的变化：

- 部署本身不改变调度的决定，actingctl 和 MCP 的命令答法照旧；actinglab 多了救急模块（LAB-7 之前不对生产根用它部署、回退、放置或换配置），从
  `tools\` 跑时认得出自己的根。

本版合并的 PR：

- #1553 LAB-3（#434，登记 §1.287；契约 `lab-two-entries.md` §4.3、§5、§6、§7、§9.3 和未决第 1、4、5 条）：actingctl 的 `install`（连同 `mcp-entry`）、
  `start`、`restart` 和 `agent-channel` 用 `git mv` 原样搬进新库 `crates/lab-rescue`，actingctl 从那里回答，面不变；actinglab 的 CLI 模块和
  `serve --stdio` 提供 29 行救急操作和组合操作 `deploy`、`config replace|restore`、`place`、`alive`；日志记 `initiator`、`one_way`；没有确认参数；守卫
  只报不拦（`guard_failed`）；只交还自己的暂停（修复 1：`hold_not_owned`；修复 2：按原因和别名、跨重启；修复 3：`hold_ownership_unconfirmed`）；从
  `tools\actinglab.exe` 跑时挪开自己的映像；`crates/lab-ops` 的定位认 `tools\`；`crates/watchdog` 加 `Asked`、`Answered`、`answer` 和两个操作种类。不升
  `install-root`、`install-cli`（总览的裁定）。新测试 11 个（`gate_434_lab3`：`rescue_without_actingd`、`irreversible_journal_initiator`、
  `deploy_steps_in_order`、`actingctl_install_answers_the_deploy_kit_as_before`，修复补的 `rescue_hands_back_only_its_own_hold`、
  `rescue_hands_back_its_own_hold_across_a_restart`、`rescue_refused_after_its_pause_hands_its_own_back`、`rescue_keeps_a_global_pause_and_goes_on`、
  `rescue_renames_its_own_running_image`、`place_holds_only_the_bundles_idle_instances`、`rescue_notes_a_hold_whose_ownership_cannot_be_read`）；
  33 个门测试随文件换了路径名（旧名进 `ci/retired-gates.toml`，带 `covered_by`）；改造 2 处（`cli_short_words_parse` 的未建示例改成 `clean`，
  `c3b_client_device_authority_stays_behind_runtime` 多扫 `crates/lab-rescue`）；CI 的 `host-lab` 多跑 `-p actingcommand-lab-rescue`。

## v0.12.47

本版是 v0.12.46 之后合并的 4 个 PR：#434 LAB-1b（#1548）、#434 LAB-4（#1549）、#434 LAB-2（#1550）和 #428 CW-1（#1551）。先说部署：接口声明都不变（账本
仍是 [3, 7]），部署不用带 `--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.46 的安装工具装本版照常（见下面的升级）；装上以后能退回
v0.12.46，不用带标志（本版写的 Lab 录制 v0.12.46 读不了，见下面的退回）。没有新键，默认值文件没改。运行上要知道的是这几处：

- **部署之前：进行中的 Lab 录制先做完**（LAB-2）。本版录制的每个步骤和标记都必带 `initiator`，不带兼容（`crates/lab/src/recording/model.rs` 192、547），
  所以部署前开始的录制，部署后本版解码不了；反过来本版写的录制 v0.12.46 也解码不了（见下面的退回）。
- **看门狗：计划任务不变，`actingwatch.exe` 换了，tick 在它自己的进程里跑**（LAB-4，登记 §1.287）。
  - 计划任务的名字、动作（`<root>\tools\actingwatch.exe`，不带参数）、工作目录、触发器和设置都不变，部署不重新注册（`crates/watchdog/src/task.rs`
    150–159 的任务 XML 逐字节没变）。新的 `actingwatch.exe` 不再启动 `<root>\runtime\actingctl.exe`，用 `crates/watchdog` 在自己的进程里跑同一个 tick
    （`apps/actingwatch/src/main.rs` 58–70、134–161、187–221）：判定、`state.json`、`watchdog.log`、`actingd-<ms>.log`、`run.lock`、码、退出码和 WMI 启动都和
    以前一样。`actingctl watchdog …`（overview 的看门狗块、控制台的看门狗页也是它）走同一个 crate，答法不变。
  - 不一样的三处：没有子进程了，`watchdog_launcher_spawn_failed` 退役；tick 启动的 actingd 不再继承 `watchdog.log` 的句柄；panic 的消息由 actingwatch
    自己写进 `watchdog.log`（`thread 'main' panicked at …`）。
  - 新命令：`<root>\tools\actingwatch.exe status|install|uninstall`，只对它所在的根，答法同 `actingctl watchdog <词> --root <root>`。从控制台起时输出打在
    那个控制台上，但控制台不等它（它是 GUI 子系统程序）：cmd 里用 `start /wait "" <root>\tools\actingwatch.exe status`，再读 `%ERRORLEVEL%`。
  - **部署后核三项**（#1549 的 “At deploy”）：
    1. `tools\actingwatch.exe` 换上了：它的 SHA-256 等于新 Tools zip（`actingcommand-tools-<sha>.zip`）根目录 `BUILD-MANIFEST.json` 的 `files` 里 `path` 为
       `actingwatch.exe` 那一项的 `sha256`。Release 的 `SHA256SUMS` 只算整个 zip，不能拿来比。哈希不同就是 Tools 没部署上：旧启动器还在经
       `runtime\actingctl.exe` 跑同一个 tick，下面两项照样会过，所以这一项必须核。
    2. overview（`ac_overview` / `actingctl overview`）的 `watchdog` 块：`decision` 是 `watchdog_alive`，`last_tick` 每分钟往前走，`attention` 里没有
       `watchdog_task_missing`、`watchdog_task_mismatch`；`actingctl watchdog status --root <root>` 答的一样。
    3. `<root>\watchdog\watchdog.log` 没有新的 `ERROR actingwatch:` 或 `ERROR actingctl:` 行；任务计划程序里这个任务的上次运行结果仍是 0。
- **actinglab：现有调用方拿到的东西不变，多了从 actingctl 来的短命令和 `actinglab serve --stdio`**（LAB-2，登记 §1.287）。
  - actinglab 原有的命令（全局选项 `--run-root`、`--verbose`、`--via` 和 `--json` 写在哪都行）照旧：给人看的输出、`--json` 信封（`ok`、`data`、`error`）和
    退出码（0、2 到 6）逐字节不变（总览的裁定）。MCP 的 actinglab 子进程（`--json --via mcp …`）、acmodel 的校验、资源仓的 CI 和脚本、热放置工具包都不用改，
    它们的命令行由 golden 钉住（`apps/actinglab/tests/consumer_goldens.rs`）；分流在 `apps/actinglab/src/cli_module.rs` 88–132。唯一的例外：`record mark`
    （MCP `ac_record_mark`）和 `session record step` 的回答带上 `initiator`。
  - 新的短命令：操作表 `contracts/lab-operations.json` 里 LAB-1、LAB-1b 的核心操作（例如 `facts`、`schedule`、`summary`、`events`、`runs`、`diagnose`、
    `packs`、`codes`、`pause`、`resume`、`offset`、`emu start|stop|restart|selfcheck`、`shutdown`），以及 `run start`、`run stop`、`reset`、`drill`，用表里的
    词。实例写在位置参数上（别名、`instance_id` 或 ADB 端口），没有 `--root`、`--state-root`。默认给人看（码和它的值，结果的成员列成小表）；`--json` 打印
    和 actingctl 同样的一行 `{…结果, "outcome": …}`；退出码 0、1、2，拒绝至少是 1。表里由 actinglab 自己的命令服务的行，短词转给那条命令（如 `watch` 是
    `lab watch`，`app launch` 是 `session app launch`），答法和那条命令一样。`status` 和 `run show` 在命令行上仍是 actinglab 原来的命令。
  - 两边都有的词现在由新模块回答（没有调用方在用）：`devices` 列出安装的设备（以前拒绝，退出码 6）；`devices bind|unbind <实例>` 写或去掉实例的绑定；
    `monitor set|clear <实例>` 把实例的监视策略写给 Runtime；`config`、`config set|clear` 读写的是 **Runtime 的设置**（发 `ConfigureSettings`）。以前
    `actinglab config set` 写的是 actinglab 自己的用户配置文件，这个功能改名 `user-config` / `user-config set`。`run evidence|start|stop` 也归模块。
  - `actinglab serve --stdio`：每行读一个 `{op, params}`，每行答一行，就是那个操作的 `--json` 打印的那一行（模块的操作是模块的行，`status`、`run show`
    也是；actinglab 自己的命令服务的行是它的信封）。`serve` 的 `status` 带看门狗块和下面的 `compile_warnings`。现在还没有调用方。
  - 发起者：Lab 录制的每个步骤和标记写 `initiator`：默认 `Lab-CLI`；环境变量 `ACTINGCOMMAND_INITIATOR` 可设 `Lab-制包` 或 `Lab-救急`（其他值答
    `actingctl_usage_invalid`，带 `variable`，什么都不发、不写）；经 MCP（`--via mcp`）添加的记 `Lab-MCP`。
  - 删掉的命令词：已退役和保留的词（122 个能力条目、退役的 `session` 词、`ledger`、`lab start|lease|preempt|release`、`scheduler`、`operation dry-run`、
    `resource check-release` 和 `resource` 的两个保留词）现在大多是未知命令（`validation_failed`，退出码 2）；`resource_check_release_retired` 随之退役。
  - LAB-6 删掉来源门之前，Runtime 只收操作员或代理来源的请求（`drill`、`offset`、`facts publish`、`targets set`、`wake-test`）经 actinglab 发出可能被拒；
    到 LAB-7 之前，这些照旧用 actingctl。
- **组装目录的编译警告现在看得见**（CW-1，登记 §1.290）。v0.12.46 的编译器开始带出警告（`pool_lines_unvalued`），但工作台组装、启用目录时读完就丢，
  账本、overview 和日志里都没有。现在：
  - 启用一代组装目录时，本来就为改了的 `schedule.json` 写的 `standard_pack_registered` 记录多一个值 `compile_warnings`（每条警告的码和它在那个文件里的
    JSON 指针，最多 64 条，带总数；`crates/runtime-host/src/host/standard_pack_schedule.rs` 670–697）。文件没改的游戏不补写记录，生产的事件流不变。
  - 当前一代的全部警告记在程序事实 `config.catalog_warnings` 里：先是计数行 `{warning, game?, count}`，再是位置行 `{warning, game?, pointer}`，最多
    256 行，超了不悄悄截，末行写 `{counts_omitted, places_omitted}`；只在变了时记（同一文件 751–779），重启后照样有。
  - overview 的 `catalog` 块（`actingctl overview`、`ac_overview` 和 `serve --stdio` 的 `status`）多一个 `compile_warnings {count, codes, items}`，事实被截时
    还有 `items_omitted`、`counts_omitted`（`crates/lab-ops/src/overview.rs` 638–656）；MCP 超出输出预算时，在裁整个 `catalog` 块之前先裁 `items`（留 `items_truncated`）。actinglab
    命令行上的 `status` 照 LAB-2 的裁定不变。
  - 生产现在会看到：每个写了两条线、没写估值的池一条 `pool_lines_unvalued`，指在它的 `stop_line`（现在三个写了线的规则文件各一个这样的池，应是 3 条）。
    它不改变任何决定：只有写了估值的池的线参与硬线和封顶次序。没有警告时，记录、事实、overview 和 MCP 都和 v0.12.46 逐字节一样；编译、组装、启用和
    决定都没变。
- **LAB-1b 只是内部搬移**：剩下的 23 个核心操作（`run show`、`material`、`targets`、`pause`、`resume`、`config set|clear`、`monitor set|clear`、`offset`、
  `facts publish`、`emu …`、`app …`、`shutdown`、`wake-test` 等）发请求、整理回答的代码从 actingctl 和 MCP 搬进 `crates/lab-ops`，actingctl 的每条命令和
  每个 MCP 工具都和以前答的一样；操作表按搬过去的代码订正了几行的 `requests`。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过：`actingd-config` 仍是 [5, 27]，`ledger` 仍是 [3, 7]，`runtime-client` 仍是 [3, 6]，
`package` 仍是 [3, 7]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。`contracts/component-interfaces.md`、
`distribution/windows/component-interfaces.json` 和架构测试 `tools/actinglab-architecture/tests/component_interfaces.rs` 自 v0.12.46 没改。

- LAB-1b 不升接口：没有新请求种类，`crates/runtime-client` 没改，只搬代码、改操作表的文字。
- LAB-4 不升接口：actingwatch 现在自己读选择和配置、自己发 `Health`，但单一位置根上 `runtime\` 和 `tools\` 一起换，看门狗和它看的 Runtime 来自同一版；
  A/B 根（R-15c/d）用它之前要先给它加版本范围检查（`contracts/install-cli.md`、`INSTALL.md`）。Tools 的文件清单没变，布局仍是 `platform-tools-v3`
  （`tools-layout` 3）；计划中的 `tools-layout` 5 跟着 LAB-5、LAB-7 的文件清单走（`component-interfaces.md` 280–282）。
- LAB-2 不升接口：actinglab 的输出行和录制文件在 `component-interfaces.md` 里没有行，actinglab 发给 Runtime 的请求没变；录制的 `initiator` 是不带兼容的
  格式变化，影响见上面和下面的退回。#434 的计划行（`runtime-client` 7、`ledger` 8、`actingd-config` 28、`install-cli` 2）仍不生效。
- CW-1 不升接口：`compile_warnings` 是已有记录的 outcome 值，账本读者只核拼写（`component-interfaces.md` 136 的读者规则），记录形状没变；
  `config.catalog_warnings` 是 `config.` 族的新键，值是已有的 `RecordList`（`crates/actingcommand-contract/src/runtime_fact.rs` 80–116 只按族校验键），
  随已有的 `RuntimeFactSnapshot` 传，`runtime-client` 的请求和答复形状都没变；overview 的新成员由客户端从事实拼出来。没有用 `catalog.activated`：它的
  成员是类型化的，加成员就要升 `ledger`。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`apps/actingd`、`actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.46 都没改，
  键表、默认值文件和配置示例都没改，部署计划的默认值对比是空的；`config.subsystems` 没有新行。`config.catalog_warnings` 是程序事实，不是配置键。
- 结果码：新码 1 个 `lab_operation_completed`（actinglab，success，`ladder_class` none，待定稿，since 0.12.47）。退役 2 个，都是待定稿、error、
  `ladder_class` none、`retired_in` 0.12.47、没有 `replaced_by`，登记项一并删掉：`watchdog_launcher_spawn_failed`（actingctl；actingwatch 不再起子进程）、
  `resource_check_release_retired`（actinglab；`resource check-release` 这个词删了）。只改说明的：`actingctl_usage_invalid`（已定稿；加
  `ACTINGCOMMAND_INITIATOR`）、`lab_operation_check_failed`、`standard_pack_registered`（多一个可选值 `compile_warnings`）。新值键 2 个：`compile_warnings`
  （records：`warning_code`、`json_pointer`）和 `warning_code`（code）（`crates/actingcommand-contract/src/outcome/keys.rs` 756–757）；结果码目录头的片名多了
  `watchdog`、`actingwatch`（没有分片文件，不登记码）。没有新位置和新词表。结果码目录从 3896 个码、1741 个位置变成 3897 个、1741 个（退役的码留在目录里）。
  本版的发布文件有 2 条，就是这两个退役码（退役的条目约束以后的目录）；新码是待定稿，不进发布文件。
- 账本：不升，没有新记录形状。`compile_warnings` 只在已有记录的 outcome 值里，`config.catalog_warnings` 是已有形状的事实记录。
- 程序文件：workspace 成员从 36 个变成 38 个（新库 `crates/watchdog`，CI 归 `host-lab` 作业；新程序包 `apps/actingwatch`，CI 归 `apps` 作业）。
  `actingwatch.exe` 从 actingctl 的包挪到 `apps/actingwatch`，发版构建的 `--bin actingwatch`（`.github/workflows/windows-remote-build.yml` 394）照旧找到它；
  `windows-remote-build.yml` 和 `release.yml` 自 v0.12.46 没改，Runtime 包和 Tools 包的文件清单都和 v0.12.46 一样。`Cargo.lock` 只多了这两个 workspace 包，
  没有新的外部依赖。`apps/actingd`、`crates/runtime-config`、`crates/runtime-client`、`crates/ledger`、`crates/ledger-forensics`、`apps/ledger-forensics`、
  `apps/mcp-entry` 自 v0.12.46 没改；合同 crate 只加了两个值键和事实 `config.catalog_warnings` 的类型。MCP 工具仍是 41 个（只改了 `ac_overview` 的说明）。

升级时要知道的（按安装工具的代码核过；LAB-4 只把 `install\current.json` 的类型和读取原样挪进 `crates/watchdog` 再在 `install` 里引用：`journal.rs`、
`state.rs` 改了引用，`deploy.rs` 和 `update.rs` 各改一行 import，`plan.rs`、`undo.rs`、`host.rs` 没改，所以下面的行号在 v0.12.46 和本版相同）：

- 不用 `--accept-ledger-one-way`：两边的 `ledger` 都是 [3, 7]，`previous_max_in_next`（7 在 [3, 7] 里）和 `one_way`（7 不大于 7）都过
  （`apps/actingctl/src/install/plan.rs` 281–294），带上也不改判断；现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`（908），apply 不读账本。
- 其余的边都过：两边的声明逐字相同，安装工具自己的声明 I 编在程序里（`install/host.rs` 24–25），v0.12.46 的 I 和本版的声明一样；`next_max_in_installer`
  （295–307）、`installer_meets_previous` 和 `installer_meets_next`（308–323）都过；`package` 两边都是 [3, 7]，`bundle_max_in_next` 在单一位置根上只有
  `pack_declaration_unknown` 警告（325–348）；`installer_knows_keys`（353–369）不受影响，没有新键；`next_check_config`（895–907）：本版的 check-config 和
  v0.12.46 是同一份代码（`apps/actingd`、`crates/runtime-config`、`crates/vision-ffi` 都没改），收现行配置，写了 DirectML 的也一样。
- **v0.12.46 的安装工具没有新东西要发或要读**：两个包的文件清单没变，它照常把新的 `tools\actingwatch.exe` 换上去；它持有 `install\writer.lock` 时任务的
  tick 什么也不做（`apps/actingwatch/src/main.rs` 145–150，和以前的启动器一样）；计划任务不用重新注册。它写的 `install\current.json` 格式没变，新看门狗
  照旧读（`crates/watchdog` 的 `install_state`，从 actingctl 原样搬来）。从本版的构建目录跑本版的安装工具，结果相同。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，回退门的配置一项也过（见下面的退回）。

退回 v0.12.46（按安装工具的代码核过）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。两边的声明相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way` 都过（`plan.rs` 281–294），
  `runtime-client`、`install-control`、`runtime-outcomes`、`package` 和 `actingd-config` 两边都一样，没有失败的边；退回时 `next_check_config` 不算接口边
  （`plan.rs` 1019–1025）。部署的自动回退（`install/deploy.rs` 715–733，`accept_ledger_one_way: false`）、`rollback plan|apply` 和起过本版 Runtime 以后的
  `install undo`（`install/undo.rs` 177–189、324–356）走的是同一组边和同一个回退门（`install/update.rs` 378 起）。
- 回退门其余几项：v0.12.46 自己的读者读部署之后写的账本页（`ledger_readability`，`update.rs` 395–432，只对本版替换掉的那个构建算，就是 v0.12.46）：
  没有新记录形状；`compile_warnings` 在 outcome 的值里，合同 crate 把值读成自由的映射（`crates/actingcommand-contract/src/outcome.rs` 1573–1577，两版相同）；
  `config.catalog_warnings` 是 `config.` 族、已有 `RecordList` 形状的事实，v0.12.46 照常读、照普通事实重放（v0.12.46 的 `runtime_fact.rs` 76–112，
  `crates/scheduler/src/facts.rs` 83–88 两版相同），但不再更新它，它的 overview 也不显示。v0.12.46 在这个根上最近一次打开在它的期限减余量之内
  （433–484）；现行配置过 v0.12.46 的 check-config（485–498）：两版的配置代码相同。
- **Lab 录制：v0.12.46 的 actinglab 读不了本版写的录制。** 本版录制的步骤和标记带 `initiator`，v0.12.46 的 `RecordingStep`、`RecordedMark` 拒收未知成员
  （v0.12.46 的 `crates/lab/src/recording/model.rs` 163–165、489–491，`deny_unknown_fields`）。退回之前，把本版开始的录制做完，否则退回后要重录。
  安装工具不看录制，不会因此拦退回。
- **看门狗**：退回时 Tools 换回 v0.12.46 的 `actingwatch.exe`，它是启动器，照旧起 `<root>\runtime\actingctl.exe watchdog run-once --root <root>
  --from-task`（v0.12.46 的 `apps/actingctl/src/watch_launcher.rs` 122、133–137），由 v0.12.46 的 actingctl 跑 tick。计划任务两版相同（动作都是
  `<root>\tools\actingwatch.exe`；任务 XML 在 v0.12.46 的 `apps/actingctl/src/watchdog/task.rs` 和本版的 `crates/watchdog/src/task.rs` 都是 150–159，内容
  一样），不用重新注册；`state.json`（`actingcommand.runtime-watchdog-state.v2`）和 `watchdog.log` 是同样的文件和格式，退回后接着用。v0.12.46 的
  `actingwatch.exe` 不看参数（v0.12.46 `watch_launcher.rs` 39–41），带 `status` 运行也是跑一次 tick：退回后看状态用 `actingctl watchdog status --root <root>`。
  `watchdog_launcher_spawn_failed` 在 v0.12.46 里还是现行的码，可能又出现。
- **规则文件的水位线不挡退回**：生产的规则文件写的 `stop_line`、`hard_line` 和 `requires` 里的 `pool_lines` 要 ≥ 0.12.46，v0.12.46 正是：它认子系统
  `pool_lines`（v0.12.46 的 `apps/actingd/src/config/manifest.rs` 190–198）、读池的两条线（`crates/policy/src/schema.rs` 1135–1142，两版相同）、按同样的
  规则查 `requires`（v0.12.46 的 `crates/policy/src/compose.rs` 862–880），有线没估值的池照样编译（警告不拒收，`crates/policy/src/compiler.rs` 418–425，两版
  相同），所以退回 v0.12.46 不用动规则文件。再往前退到 v0.12.45 不行：它按名字拒收 `pool_lines`（见 v0.12.46 的退回）。
- 落盘的东西除了录制都不挡：没有新键、新文件、新记录形状。
- 退回以后：看门狗又经 `runtime\actingctl.exe` 跑 tick；actinglab 没有短命令和 `serve --stdio`，`config set` 又写 actinglab 自己的用户配置；编译警告又只
  编译不记，overview 没有 `compile_warnings`；actingctl 和 MCP 的命令和本版一样。
- 客户端和 Runtime 的配对：
  - v0.12.46 的安装工具装本版：照常，不带标志，见上面的升级。
  - **actinglab ≤ v0.12.46 和 MCP（actingctl ≤ v0.12.46）连本版的 Runtime**：线上什么都不变。`crates/runtime-client` 没改，合同 crate 只加了两个值键和
    一个事实类型（编码成已有的 `RecordList`），Runtime 收的请求、发的回执和写的记录形状都和 v0.12.46 一样。旧客户端只是显示上少一点：旧的
    `actingctl overview`、`ac_overview` 的 catalog 块没有 `compile_warnings`（它由本版的 lab-ops 从事实拼出来，`crates/lab-ops/src/overview.rs` 638–656），
    事实本身旧的 `actingctl facts`、`ac_facts` 照样列出；它内嵌的码目录没有 `lab_operation_completed`，`actingctl codes`、`ac_outcome_code` 查它答
    `outcome_code_unknown`（v0.12.46 的 `crates/lab-ops/src/code_lookup.rs` 188–200）。
  - 旧 MCP 跑本版的 actinglab（MCP 跑的是安装根里的 `<root>\tools\actinglab.exe`，带 `--json --via mcp`，v0.12.46 的 `apps/actingctl/src/mcp/lab.rs` 61、
    739）：走 actinglab 的旧路，输出和退出码逐字节照旧；只有 `ac_record_mark` 的答复多了 `initiator`（旧 MCP 原样转 `data`，v0.12.46 `lab.rs` 3–4、842），
    录制里记 `Lab-MCP`。旧 actinglab 读本版写的录制读不了（见上面）。生产根里的 actingctl（含 MCP 服务）、`tools\actinglab.exe`、`tools\actingwatch.exe`
    和 Runtime 是同一个构建，随部署一起换。
  - **控制台 UI**（UI 仓 main `0e324e21`，最新发版仍是 UI v0.12.10 `c03d18ea`；自 `e60e3366` 只改了 `model/` 下的 acmodel，控制台没动：`Cargo.toml` 39–44
    仍把合同、账本、取证、`runtime-client`、配置和内核 crate 钉在 `a322caa4`，即 v0.12.43；`crates/acui-setup/component-interfaces.json` 仍声明
    `actingd-config` [5, 26]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：本版的声明和 v0.12.46 逐字相同，线上也没变，所以
    控制台对本版和对 v0.12.46 一样。CW-1 的审查结论按 UI 的代码核过：控制台不读 overview（UI 仓 `model/` 以外的 Rust 代码里没有 overview 的读取）；它取
    事实只按指定的键或 `config.parameters.` 前缀（`crates/acui-source/src/workbench.rs` 1284–1285、1453–1456，`crates/acui-source/src/workbench/backends.rs`
    586–587、685），新事实在快照里能解码（钉住的合同 crate 只按族校验键，值是已有的 `RecordList`）但不显示；outcome 的值由钉住的合同 crate 按自由的
    映射解码（`a322caa4` 的 `crates/actingcommand-contract/src/outcome.rs` 1573–1577），账本里带 `compile_warnings` 的记录照读；不在它码目录里的码照原样
    显示（`crates/acui-rows/src/outcome.rs` 152、156–161）。看门狗页跑的是 `actingctl watchdog status --root <root>`（`crates/acui-app/src/workbench/tools.rs`
    404–415），本版答法不变。配置写了修订 27 的成员（切了 DirectML）时不在它声明的 [5, 26] 里，这在 v0.12.44 就是这样，本版没变。本版没有东西逼控制台
    换钉。

运行上的变化：

- 部署本身不改变调度的决定；看门狗的 tick 改由 `actingwatch.exe` 自己跑（判定和文件不变），overview 的 catalog 块多了编译警告，actinglab 多了短命令和
  `serve --stdio`（原有命令的输出不变）；部署前先做完进行中的录制。

本版合并的 PR：

- #1548 LAB-1b（#434，登记 §1.287）：剩下的 23 个核心操作的代码搬进 `crates/lab-ops`（新模块 `apps`、`configure`、`emulators`、`fact_publish`、`frames`、
  `lab_child`、`material`、`run_show`、`scheduling`、`served`、`shutdown`、`targets`、`wait`、`wake_test`；`process_probe` 和关机等待随 `shutdown` 一起搬），
  actingctl 和 MCP 调这些函数，答法不变；登记表 `served::SERVED` 写明每个核心行由哪个函数服务；操作表 23 行改归 LAB-1b，按代码订正 `requests`；
  `ac_config_set`、`ac_targets_set` 的编码失败从 panic 改答已登记的 `actingctl_output_failed`（实际走不到）。新测试 1 个
  （`lab_ops_core_rows_served_by_lab_ops`），改造 1 个（`gate_mcp13a`：改读 `ac_app` 实际跑的子进程命令行，断言不变）；没有新码、新键。
- #1549 LAB-4（#434，登记 §1.287）：新库 `crates/watchdog`（actingctl 的看门狗用 `git mv` 搬来，连同 `install\current.json` 的读取）和新程序包
  `apps/actingwatch`（`actingwatch.exe` 自己跑 tick，加 `status|install|uninstall`）；`actingctl watchdog …` 走同一个 crate；`crates/host-metrics` 加
  `attach_parent_console`；`watchdog_launcher_spawn_failed` 退役；操作表的 `watchdog install|uninstall` 两行改归 LAB-3；`INSTALL.md`、`install-cli.md`、
  `runtime-watchdog.md` 跟着改。新测试 1 个（`actingwatch_tick_in_process`），42 个门测试随文件换了路径名（旧名进 `ci/retired-gates.toml`，带
  `covered_by`），退役 1 个（固定入口跑不了的那条），改造 3 处（启动器的注入改为记 tick、`watchdog_install_registers_the_task_and_uninstall_removes_it`
  放一个占位的 `tools\actingwatch.exe`、架构守卫多读两个包）。
- #1550 LAB-2（#434，登记 §1.287；契约 `lab-two-entries.md` §9.3 和未决第 6、7、10 条）：actinglab 的 CLI 模块和 `serve --stdio`；原有命令的输出逐字节
  不变（修复 1，总览的裁定）；录制带 `initiator`（第 6 条；`record mark` 和 `session record step` 的回答是唯一的例外）；`config get|set` 改名 `user-config`
  （第 7 条）；`status`、`run show` 在命令行上仍是 actinglab 原来的（第 10 条）；删掉退役和保留的命令词（`ratchet/actinglab_commands.json` 变成 46 个分派臂、
  102 条命令）；`lab-ops` 不再打印（`cli_line` 返回 `Exit`）；操作表多两行（170 项）。新码 1 个、退役 1 个。新测试 5 个（`gate_434_lab2`）和一个集成测试
  （`consumer_goldens`：15 个消费方命令行的 golden，照 main 的代码写）；删了 6 个测退役词的测试（都不在门测表里）；改造的有三个能力测试、退役 `session`
  选择器的测试、`config get|set` 的测试（改测 `user-config`）、两个录制夹具和 `crates/lab` 里七个取录制锁的门测试（`Lab-CLI`）；架构守卫删掉
  `lab-ops` 的 stderr 放行条目，命令清单跟着改。
- #1551 CW-1（#428，登记 §1.290）：组装器把每条编译警告定位到所在游戏的 `schedule.json`；启用时写的 `standard_pack_registered` 带 `compile_warnings`；
  程序事实 `config.catalog_warnings`；overview 的 `catalog.compile_warnings` 和 MCP 的裁剪次序；新值键 2 个。新测试 8 个（`gate_rl1b` 的 `cw1_a` 到
  `cw1_e`、`gate_mcp8` 的裁剪测试、lab-ops overview 的两个），没有改既有测试。

## v0.12.46

本版是 v0.12.45 之后合并的 2 个 PR：#428 SS-2f（#1543）和 #428 SS-2h（#1546）。先说部署：接口声明都不变（账本仍是 [3, 7]），部署不用带
`--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.45 的安装工具装本版照常（见下面的升级）；装上以后能退回 v0.12.45，不用带标志
（规则文件写了水位线或 `limited` 的，退回前要先拿掉，见下面的退回）。没有新键，默认值文件没改。运行上要知道的是这几处：

- **部署本身不改变调度的决定**：生产的规则文件现在都没写池的估值、水位线和 `limited`，裁定 116 的停摆照旧（被评判的业务候选仍是
  `benefit_unknown`，一个也不派）。SS-2h 在这样的目录上逐字节不变（目录的序列化、哈希和每个决定都一样）。SS-2f 只管业务候选跨实例的先后；116 之下
  还能派的业务候选只有不被评判的那几种（由指令触发的业务规则，和随它运行的链条后继），只有两台实例的这类候选在同一轮争同一个共享运行名额
  （`max_concurrent_runs`、重场景名额）时，谁先谁后才可能和以前不同（见下一条）。
- **SS-2f：业务档跨实例，谁等得最久谁先**（登记 §1.288）。
  - 目录带业务标记（`benefit_judged`）时，两台不同实例的业务候选先按实例排：这台实例最近一次策略派发最早的先，从没派发过的排在所有派发过的前面，
    同一时刻（两台都没派过也算）按实例 id 小的先（`crates/policy/src/evaluator.rs` 2657–2672，`evaluator/turns.rs` 44）。「最近一次」取这台实例
    各任务快照里最新的 `last_dispatched_unix_ms`，读数刷新的派发也算；读的是每次评估本来就有的快照，不加新读取，不查账本。
  - 这个键排在 `promoted`、档和零档的位置之后：维护档仍整体在前，零档仍在每台实例的正收益和未评判候选后面；同一实例内的次序不变（未评判的先，
    评判的按 `B ÷ E`，零档最后）；维护档和不带标记的目录照旧；保留门按同一个次序取候选，仍按实例算。
  - 116 解除以后的效果：几台实例的业务候选争共享运行名额时，给等得最久的实例，不再给数大的游戏（各游戏的价值单位不同，`B` 跨游戏不可比）。
    没有新码、新键。
- **SS-2h：池的两条水位线，和封顶时的花费次序**（登记 §1.289；只对写了估值的池和被评判的消耗者起作用）。
  - **平时全价**：被评判候选的消耗一律按 `W·c` 算，不管池满不满；SS-2b 按满度的线性折价退役，SS-2d 的 `ε` 读整个 `C`。被消耗的再生池照旧要读数
    （未知或过期是 `benefit_unknown`），下一步变化时再评。
  - **两条线**：V2 池可以写 `stop_line`（停止线 S）和 `hard_line`（硬线 H），要么都写、要么都不写；每条是一个事实引用（事实加偏移，同 CF-5 的上限
    引用）或常数。规则是 `0 ≤ H < S ≤ 上限`：编译时比得出来的不合是 `pool_lines_invalid`（目录拒收），评估时才读得到的不合是 `lines_invalid`；V1 写它
    是 `unknown_field`。不写时 S 就是上限、H 是 0。线由模型、人或代理写，Runtime 从不自己算。
  - **上限以下**：被评判、本来能派的候选，跑完会让池落到 H 以下的不派：`pool_line_held`（`pool_line_hold` 是 `would_cross_hard_line`，带
    `amount_after`）。
  - **封顶**：读数到上限（开机时也算）时，这台实例上的这个池开始一个封顶期。每轮列出这个池现在能派的被评判消耗者，按「不算这个池的成本时一次运行
    的价值 V ÷ 消耗量 c」冻结次序（平手按任务 id，从不按成本排），从读数往下计划到 S；最后一个可以越过 S，但不能让池落到 H 以下。一次只派排头一个
    （`capped_spend`，带两条线、`pool_capacity`、`pool_reading`、`capped_episode`、`capped_round`、`capped_rank`），每次派前再核实际读数；读数还在 S
    以上就开下一轮，第二轮起只排没标 `limited` 的。排头在分配里占它自己的位置（档和 `B ÷ E`，`B` 只算把读数压到 S 以下的那部分消耗），所以更急的档
    照样先走；派单在调度器里面，其余的门照走。其余的消耗者这一轮等着：`pool_line_held`，`pool_line_hold` 是 `capped_not_head`、
    `capped_limited_spent`、`capped_would_cross_hard_line`、`capped_not_dispatchable`（带 `gate_code`）、`capped_round_empty` 或 `capped_cap_unknown`。
  - 回合记忆（`CappedMemory`）只在内存里，和 SS-4 的一样：重启或启用新的一代目录就从头算（`crates/runtime-host/src/policy_host.rs` 1254、2888、
    3061）。空闲计划把 `pool_line_held` 当作被收益门挡住的，不算到期，也不为它叫醒（`schedule_explanation.rs` 197）。线只约束调度器：手动请求、随链
    运行的链条后继和 Runtime 自己的任务不受它挡。
  - **规则文件的新成员**：池的 `stop_line`、`hard_line`；规则的 `"limited": true`（编排器写成任务的 `benefit_limited`，只用在被评判的规则上，维护规则
    写它会被拒）。**写了水位线的规则文件要本版的 Runtime**：它必须在 `requires` 里写子系统 `pool_lines`（本版的 `config.subsystems` 多这一行，常开），
    不写的话本版的编排器拒收（`schedule_invalid`，指在那条线上，`crates/policy/src/compose.rs` 862–880）；v0.12.45 不认这个子系统，按名字拒收
    （`schedule_requires_missing`）。
  - **有线没估值**：池写了两条线、却没有估值（没写或是 `"pending"`）时，目录照样编译，带一条警告 `pool_lines_unvalued`（指在 `stop_line`，列在编译
    结果和试跑报告里）；只有写了估值的池的线才参与硬线和封顶次序，所以这两条线不改变任何决定。这是编译器的第一个警告：以前有诊断就拒收，现在有
    错误才拒收（`crates/policy/src/compiler.rs` 418–425），其余诊断都是错误级，原有检查一条没放松。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过：`actingd-config` 仍是 [5, 27]，`ledger` 仍是 [3, 7]，`runtime-client` 仍是 [3, 6]，
`package` 仍是 [3, 7]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。`contracts/component-interfaces.md`、
`distribution/windows/component-interfaces.json` 和架构测试 `tools/actinglab-architecture/tests/component_interfaces.rs` 自 v0.12.45 没改。

- SS-2f 不升接口：评估器的排序不是锚点，没有对端读的字段、记录或文件变了（`component-interfaces.md` 的升级规则，290–304）。
- SS-2h 不升接口：新成员由 `requires` 的 `pool_lines` 把关（旧 Runtime 拒收整个文件，不会读错），新码是结果码目录的追加（`runtime-outcomes` 1 的读者
  本来就收），回合记忆不落盘；同一条规则里「旧对端容得下的改动不升修订」。池的两条线和任务的 `benefit_limited` 是目录格式的追加，不是账本形状；它们对
  退回的影响见下面。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`actingd.defaults.json` 和 `actingd.config.example.json` 自 v0.12.45 都没改，键表、默认值
  文件和配置示例都没改，部署计划的默认值对比是空的。`pool_lines` 是程序事实 `config.subsystems` 里的一行（`apps/actingd/src/config/manifest.rs`
  190–198，`config_subsystem_always_on`），不是配置键；`apps/actingd` 只改了这一处。
- 结果码：SS-2h 新码 4 个，都在 policy 分片，待定稿，`ladder_class` none，since 0.12.46：`capped_spend`（info）、`pool_line_held`（info）、
  `pool_lines_invalid`（error，目录诊断）、`pool_lines_unvalued`（warning，目录诊断）。新值键 8 个：整数 `pool_capacity`、`stop_line`、`hard_line`、
  `pool_reading`、`capped_episode`、`capped_round`、`capped_rank` 和词表键 `pool_line_hold`；新词表 `pool_line_hold`（8 个词：`would_cross_hard_line`、
  `capped_not_dispatchable`、`capped_limited_spent`、`capped_would_cross_hard_line`、`capped_round_empty`、`capped_not_head`、`capped_cap_unknown`、
  `lines_invalid`）；policy 多一个前缀 `capped_`，前缀 `pool_` 加上 policy。`policy_benefit_nonpositive`、`benefit_unknown`、`benefit_gap` 的词
  `reading_unknown`、`benefit_milli` 和 `tolerance_milli` 的说明去掉了折价。SS-2f 没有码。没有新位置，没有新定稿或退役的条目。结果码目录从 3892 个码、
  1741 个位置变成 3896 个、1741 个；新码都是待定稿，不进发布文件，所以本版的发布文件是空的。
- 账本：不升，没有新记录形状。新码和新值只放在已有的原因链里（而且只有写了估值的池才会出现），回合记忆不落盘。
- 程序文件：workspace 成员仍是 36 个（`turns.rs`、`capped.rs` 是 `crates/policy` 里的新模块），Runtime 包和 Tools 包的文件清单都和 v0.12.45 一样
  （`.github/workflows` 自 v0.12.45 没改）；`Cargo.lock` 没改，没有新依赖。改了的只有 `crates/policy`、`crates/runtime-host`（`policy_host.rs`、
  `schedule_explanation.rs`）、合同 crate 的值键表（`crates/actingcommand-contract/src/outcome/keys.rs` 743–753）和 `apps/actingd` 的一行子系统；
  `crates/runtime-client`、`crates/ledger`、`crates/ledger-forensics`、`apps/actingctl`、`apps/actinglab` 自 v0.12.45 没改，MCP 工具仍是 41 个。

升级时要知道的（按安装工具的代码核过；`apps/actingctl/src/install` 自 v0.12.43 没改，下面的行号在 v0.12.45 和本版相同）：

- 不用 `--accept-ledger-one-way`：两边的 `ledger` 都是 [3, 7]，`previous_max_in_next`（7 在 [3, 7] 里）和 `one_way`（7 不大于 7）都过
  （`apps/actingctl/src/install/plan.rs` 281–294），带上也不改判断；现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`（908），apply 不读账本。
- 其余的边都过：两边的声明逐字相同，安装工具自己的声明 I 编在程序里（`install/host.rs` 24–25），v0.12.45 的 I 和本版的声明一样；`next_max_in_installer`
  （295–307）、`installer_meets_previous` 和 `installer_meets_next`（308–323）都过；`package` 两边都是 [3, 7]，`bundle_max_in_next` 在单一位置根上只有
  `pack_declaration_unknown` 警告（325–348）；`installer_knows_keys`（353–369）不受影响，没有新键；`next_check_config`（895–907）：本版的 check-config
  只在报告的子系统里多一行 `pool_lines`，配置代码（`crates/runtime-config`、`apps/actingd/src/config.rs`、`crates/vision-ffi`）没改，收现行配置，写了
  DirectML 的也一样。
- **v0.12.45 的安装工具没有新东西要发或要读**：它和 Runtime 之间的请求、答复形状都没变，它不编排规则文件、不读目录。从本版的构建目录跑本版的安装
  工具，结果相同。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，回退门的配置一项也过；刚部署完的规则文件没写水位线，退回也不受目录挡（见下面的退回）。

退回 v0.12.45（按安装工具的代码核过）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。两边的声明相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way` 都过（`plan.rs` 281–294），
  `runtime-client`、`install-control`、`runtime-outcomes`、`package` 和 `actingd-config` 两边都一样，没有失败的边；退回时 `next_check_config` 不算接口边
  （`plan.rs` 1019–1025）。部署的自动回退（`install/deploy.rs` 715–733，`accept_ledger_one_way: false`）、`rollback plan|apply` 和起过本版 Runtime 以后的
  `install undo`（`install/undo.rs` 177–189、324–356）走的是同一组边和同一个回退门（`install/update.rs` 378 起）。
- 回退门其余几项：v0.12.45 自己的读者读部署之后写的账本页（`ledger_readability`，`update.rs` 395–432，只对本版替换掉的那个构建算，就是 v0.12.45）：
  没有新记录形状，新码和新值都在已有的原因链里，读得了；v0.12.45 在这个根上最近一次打开在它的期限减余量之内（433–484）；现行配置过 v0.12.45 的
  check-config（485–498）：两版的配置代码相同。
- **规则文件写了水位线或 `limited` 时，安装工具不拦，但 v0.12.45 不收、甚至起不来**：回退门不看规则文件和目录。
  - v0.12.45 不认子系统 `pool_lines`，它的编排器以 `schedule_requires_missing` 拒收写了线的 `schedule.json`（v0.12.45 的 `crates/policy/src/compose.rs`
    890–903）；它的读取器也不认任务的 `benefit_limited`（`unknown_field`）。这个游戏退回启用的代里的那份规则文件，其他游戏照常。
  - 更要紧的是启动：v0.12.45 启动时要重新编译启用的那一代、每个未了结的派单钉住的那一代，以及决策记忆点名的每次派单所在的那一代（§1.225 的退回
    更正）。其中只要有一代带水位线或 `benefit_limited`，它的读取器就拒收（任务和池都是 `deny_unknown_fields`，v0.12.45 的 `crates/policy/src/schema.rs`
    305–307、1051–1053），以 `catalog_generation_invalid` 致命地停下（v0.12.45 的 `crates/runtime-host/src/policy_host.rs` 6677，配置族的全局故障），整个
    Runtime 起不来。
  - 所以退回之前，先在本版上把各游戏根 `schedule.json` 里的 `stop_line`、`hard_line`、`limited` 和 `requires` 里的 `pool_lines` 拿掉，等下一次工作台
    检查启用了不带它们的一代（程序事实 `config.catalog` 换成了新的代），再确认没有未了结的计划派单；在带线的代下派过计划运行的每个（任务、实例）对，
    都要先在新的代下再完成一次派单。生产的规则文件现在都没写这些，所以现在退回什么都不用做。
- 落盘的东西都不挡：没有新键、新文件、新记录形状；回合记忆不落盘。
- 退回以后：业务档跨实例又按 `B ÷ E` 排；写了估值的再生池又按满度折价；没有水位线和封顶次序。
- 客户端和 Runtime 的配对：
  - v0.12.45 的安装工具装本版：照常，不带标志，见上面的升级。
  - **actinglab ≤ v0.12.45 和 MCP（actingctl ≤ v0.12.45）连本版的 Runtime**：线上什么都不变。`crates/runtime-client`、`apps/actingctl`、`apps/actinglab`
    自 v0.12.45 没改，合同 crate 只在值键表里加了 8 个键，Runtime 收的请求、发的回执和写的记录形状都和 v0.12.45 一样。只有规则文件写了估值和水位线
    以后，`actingctl schedule`、`ac_schedule` 和 `ac_overview`、`ac_diagnose` 的未运行一行里才会出现新码：旧客户端照原样显示码和值，但它内嵌的码目录
    没有这 4 个码，`actingctl codes`、`ac_outcome_code` 查它们答 `outcome_code_unknown`（`crates/lab-ops/src/code_lookup.rs` 188）。生产根里的
    actingctl（含 MCP 服务和看门狗）、`tools\actinglab.exe` 和 Runtime 是同一个构建，随部署一起换。
  - **控制台 UI**（UI 仓 main `e60e3366`，最新发版仍是 UI v0.12.10；自 `c03d18ea` 只改了 `model/` 下的 acmodel，控制台没动：`Cargo.toml` 39–44 仍把
    合同、账本、取证、`runtime-client`、配置和内核 crate 钉在 `a322caa4`，即 v0.12.43；`crates/acui-setup/component-interfaces.json` 仍声明
    `actingd-config` [5, 26]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：本版的声明和 v0.12.45 逐字相同，线上也没变，所以
    控制台对本版和对 v0.12.45 一样。`ledger` 7 在 [3, 7] 里，`runtime-client` [3, 5] 和 [3, 6] 有交集，`runtime-outcomes` 1 在 [1, 1] 里；配置写了修订
    27 的成员（切了 DirectML）时不在它声明的 [5, 26] 里，这在 v0.12.44 就是这样，本版没变：控制台把配置当 JSON 读、只查 `vision_provider_manifest`
    （`crates/acui-installation/src/lib.rs` 77），画面刷新走 `observe_readonly`（`crates/acui-source/src/workbench/preview.rs` 586–595）。本版的 4 个新码
    不在它钉住的码目录里，遇到时它照原样显示码（`crates/acui-rows/src/outcome.rs` 152、156–161）；生产的规则文件没写估值和水位线，现在不会出现。本版
    没有东西逼控制台换钉。

运行上的变化：

- 部署本身不改变运行：没写估值和水位线的目录 SS-2h 逐字节不变；SS-2f 只在两台实例不被评判的业务候选争同一个共享运行名额时，先给最近一次派发最早的实例。

本版合并的 PR：

- #1543 SS-2f（#428，登记 §1.288，问题 137 的答复 K2）：`allocation_order` 加一个键，业务档跨实例按最近一次策略派发排（新模块 `evaluator/turns.rs`），
  保留门用同一个次序；契约改调度 README 和 `standard-pack-schedule.md`；新测试 4 个（`gate_ss2f`），没有改既有测试；没有新码、新键。
- #1546 SS-2h（#428，登记 §1.289，裁定 138 和 172、173）：池的 `stop_line`、`hard_line` 和 `pool_lines_invalid`；平时全价（SS-2b 的折价退役）；硬线和
  封顶花费次序（新模块 `evaluator/capped.rs`，新入口 `evaluate_with_capped_spending`，runtime-host 持有只在内存的回合记忆）；任务成员 `benefit_limited`
  （规则成员 `limited`）；常开子系统 `pool_lines`；编译器开始带出警告（`pool_lines_unvalued`）；新码 4 个、值键 8 个、词表 1 个；新测试 19 个
  （`gate_ss2h`，设计的 17 个加修复补的 2 个），改造 2 个（`gate_ss2b::ss2b_t6`、`gate_ss2d::ss2d_t15`：满的再生池原来不计成本，现在全价，到上限时作为
  封顶排头派出）。它取代了没合入的 SS-2h-a（#1544）。

## v0.12.45

本版是 v0.12.44 之后合并的 3 个 PR：#434 LAB-0（#1540）、#433 LABDO-1（#1542）和 #434 LAB-1（#1541）。先说部署：接口声明都不变（账本仍是 [3, 7]），
部署不用带 `--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.44 的安装工具装本版照常（见下面的升级）；装上以后能退回 v0.12.44，不用带
标志（见下面的退回）。没有新键，默认值文件没改。运行上要知道的是这几处：

- **Runtime 已经做了的 Lab 操作，客户端自己的核验没过时，答 `lab_operation_check_failed`（退出码 5），不再是设备错误**（LABDO-1）。
  - 以前：`actinglab do --capture`（元素、坐标或 `--operation`）收到 Runtime 的回执以后，`runtime-client` 还要按请求、租约和各个动作查事件，再核这次操作的
    证据；其中任何一步失败，actinglab 都答 `device_error`（退出码 4），MCP `ac_lab_do` 转成 `device` 类，不带 `req_id`，也不带 actinglab 的消息。可这时操作已经
    在设备上做完了。
  - 现在：答 `lab_operation_check_failed`，退出码 5（Runtime 和状态类，`apps/actinglab/src/cli_result.rs` 93–100）。details 带回执的 `req_id`、`correlation_id`、
    `receipt_state`，操作记录报的 `effect`（`performed`、`not_performed` 或 `indeterminate`；回执里没有 Lab 操作记录时为 null）和 `executed`，记录自己的
    `failure`（没有时为 null），以及客户端自己的 `client_code`、`client_message`（`apps/actinglab/src/lab2_cli/operation.rs` 222–258）。Runtime 拒绝了的、
    或者根本没回答的请求，照旧是 `device_error`。
  - **收到这个码，不要把这个操作当成没跑过再发一次。** 重发是一个新请求，没有幂等键，设备上会再做一次。先看 `effect`，再看那台实例的画面，确认页面以后再决定
    下一步；把 `client_code` 和 `req_id` 报上来，好把那项核验改对。
  - 本版只改了答法，没改核验本身：让核验不过的原因还在，部署以后带 OCR 的 Lab 操作还可能遇到，只是现在报得出是哪一项（#1542 的诊断认为最可能是
    `runtime_lab_diagnostic_budget_mismatch`：DirectML 会话载入后的第一次 OCR 多存了一份节点分布报告，记在这次请求名下，客户端数诊断件时对不上；只看代码定不了，
    要用 `client_code` 坐实）。客户端照旧不往账本写这次失败。
  - `do --capture --record` 遇到这个码照旧不记那一步；答复写明了效果，人可以照它关掉或接受这一步。
  - MCP：所有 Lab 工具的错误在已有的码和 details 两条链接后面，再加一条 `foreign_other_error`（`raw_source` process），`raw_text` 是 actinglab 的消息原文
    （`apps/actingctl/src/mcp/lab.rs` 871–917）。`ac_lab_do` 对这个码答 `runtime` 类的 `lab_tool_failed {exit_code: 5, req_id}`，cause 是
    `lab_operation_check_failed` 和带 `effect` 的 details。Lab 工具的共用说明（`lab_errors!`）跟着改了一句，MCP 工具仍是 41 个。
  - 契约 `contracts/lab-two-entries.md` §9.3 写明以后的 LAB-2、LAB-5 也照这样：请求发出去、Runtime 回答了以后的失败，要写明 `req_id` 和效果，绝不算设备错误。
- **actingctl 的读写模块挪进新的库 `crates/lab-ops`，只是内部搬移；actingctl 的每条命令都和以前答的一样**（LAB-1）。
  - 用 `git mv` 搬了 16 个模块（`overview`、`events`、`run_list`、`run_evidence`、`diagnose`、`facts`、`schedule`、`perf_preset`、`routine_summary`、`remedy`、
    `devices`、`packs`、`code_lookup`、`capability_gap`、`evidence_read`、`observe_export`）和它们共用的 5 个辅助模块（`child`、`cli_line`、`codes`、
    `instance_selector`、`material_export`）；actingctl 从 `actingcommand_lab_ops` 引入同名模块（`apps/actingctl/src/main.rs` 34–43、83–84）。命令的词、输出、
    码和退出码都没变；搬过去的测试只改了包名前缀，断言一条没动。
  - overview 的看门狗块：看门狗留在 actingctl 里（到 LAB-4），actingctl 启动时把读取函数交给 lab-ops（`main.rs` 129–132，只在 Windows 上），所以
    `actingctl overview` 和 MCP `ac_overview` 的看门狗块照旧。
  - 新加的三样：操作表读取（`table.rs`，把 `contracts/lab-operations.json` 读成带类型的行）、发起者类型（`initiator.rs`，契约 §4.1 的六个词）和默认位置
    （`locate.rs`，契约 §9.3）。本版还没有程序用它们，只有两个新测试读。
  - 结果码目录的头：片名 `actingctl` 改指 `crates/lab-ops`（码表随 `codes.rs` 搬了过去），新片名 `actingctl-app` 指 `apps/actingctl`、没有分片文件；
    LAB-1 没动 `actingctl.json`，码的拼写和所属都没变；合并目录本身不带片名映射，所以嵌进程序的那份不受这处改动影响。`evidence confirm-read` 记的客户端
    版本照旧是 `0.1.0`（lab-ops 用 actingctl 的版本号）。
- **LAB-0 只写契约，运行上什么都不变**（LAB-0）：`contracts/lab-two-entries.md`（Lab 和 UI 两个入口的契约）、`contracts/lab-operations.json`（操作表 v1，
  168 项）、`contracts/component-interfaces.md` 的 “Planned: Workflow #434” 一节（四条计划行，不生效）和登记 §1.287。没有代码、键和结果码。

配置：接口修订都不变，按代码和 `contracts/component-interfaces.md` 核过：`actingd-config` 仍是 [5, 27]，`ledger` 仍是 [3, 7]，`runtime-client` 仍是 [3, 6]，
`package` 仍是 [3, 7]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。`distribution/windows/component-interfaces.json`
和架构测试 `tools/actinglab-architecture/tests/component_interfaces.rs` 自 v0.12.44 没改。

- #434 的计划行不生效：`component-interfaces.md` 新加的 “Planned: Workflow #434” 一节写了 `runtime-client` 7、`ledger` 8、`actingd-config` 28（LAB-6）和
  `install-cli` 2（LAB-7），明写 not in force，没有构建声明它们；声明文件没动，本版也不声明 `install-cli`。同一节把 `install-control` 2（#389 U-4）的计划标成
  被取代。
- LABDO-1 不升接口：`LabOperationAnswer` 是 `runtime-client` 错误类型里的进程内成员（`crates/runtime-client/src/error.rs` 192，挂在
  `client/lab_operation.rs` 229–239 的 `map_err` 上），客户端发的请求、回执和握手都没变；`runtime-outcomes` 1 多一个登记的码、MCP 工具错误多一条 cause
  链接，都是 v1 读者本来就收的追加。LAB-1 不升接口：没有新请求种类，`runtime-client` 和账本都没改。
- 配置键：没有新键，也没有去掉的键。`crates/runtime-config`、`apps/actingd` 和合同 crate 自 v0.12.44 都没改，键表、默认值文件和配置示例都没改，部署计划的
  默认值对比是空的。
- 结果码：新码 1 个 `lab_operation_check_failed`（actinglab 分片，error，`ladder_class` none，待定稿，since 0.12.45；必带 `req_id`、`receipt_state`、
  `client_code`，三个都是已有的键）；`lab_tool_failed`（actingctl，待定稿）只改了说明。没有新位置、新值键或新词，没有新定稿或退役的条目。结果码目录从
  3891 个码、1741 个位置变成 3892 个、1741 个；新码是待定稿，不进发布文件，所以本版的发布文件是空的。
- 账本：不升，没有新记录形状。
- 程序文件：workspace 成员从 35 个变成 36 个（多了库 `crates/lab-ops`，CI 里归 `host-lab` 作业），Runtime 包和 Tools 包的文件清单都和 v0.12.44 一样
  （`.github/workflows/windows-remote-build.yml` 自 v0.12.44 没改）；外部依赖没变。`apps/actingd`、`crates/runtime-host`、`crates/ledger`、
  `crates/ledger-forensics` 和合同 crate 自 v0.12.44 没改；`crates/runtime-client` 只改了错误类型、Lab 操作的 `map_err` 和只在测试里编的辅助函数。

升级时要知道的（按安装工具的代码核过；`apps/actingctl/src/install` 自 v0.12.43 没改，下面的行号在 v0.12.44 和本版相同）：

- 不用 `--accept-ledger-one-way`：两边的 `ledger` 都是 [3, 7]，`previous_max_in_next`（7 在 [3, 7] 里）和 `one_way`（7 不大于 7）都过
  （`apps/actingctl/src/install/plan.rs` 281–294），带上也不改判断；现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`（908），apply 不读账本。
- 其余的边都过：两边的声明逐字相同，安装工具自己的声明 I 编在程序里（`install/host.rs` 24–25），v0.12.44 的 I 和本版的声明一样；`next_max_in_installer`
  （295–307）、`installer_meets_previous` 和 `installer_meets_next`（308–323）都过；`package` 两边都是 [3, 7]，`bundle_max_in_next` 在单一位置根上只有
  `pack_declaration_unknown` 警告（325–348）；`installer_knows_keys`（353–369）不受影响，没有新键；`next_check_config`（895–907）：本版的 check-config 和
  v0.12.44 是同一份代码（`apps/actingd`、`crates/runtime-config`、`crates/vision-ffi` 都没改），收现行配置，写了 DirectML（修订 27 的成员）的也一样。
- **v0.12.44 的安装工具没有新东西要发或要读**：它连 Runtime 发的请求和读到的答复形状都没变，它不跑 Lab 操作。从本版的构建目录跑本版的安装工具，结果相同。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，回退门的配置一项也过（见下面的退回）。

退回 v0.12.44（按安装工具的代码核过）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。两边的声明相同：`ledger` [3, 7] 的 `previous_max_in_next` 和 `one_way` 都过（`plan.rs` 281–294），
  `runtime-client`、`install-control`、`runtime-outcomes`、`package` 和 `actingd-config` 两边都一样，没有失败的边；退回时 `next_check_config` 不算接口边
  （`plan.rs` 1019–1025）。部署的自动回退（`install/deploy.rs` 715–733，`accept_ledger_one_way: false`）、`rollback plan|apply` 和起过本版 Runtime 以后的
  `install undo`（`install/undo.rs` 177–189、324–356）走的是同一组边和同一个回退门（`install/update.rs` 378 起）。
- 回退门其余几项：v0.12.44 自己的读者读部署之后写的账本页（`ledger_readability`，`update.rs` 395–432，只对本版替换掉的那个构建算，就是 v0.12.44）：
  本版的 Runtime 代码没变，写下的记录形状和 v0.12.44 一样，读得了；v0.12.44 在这个根上最近一次打开在它的期限减余量之内（433–484）；现行配置过 v0.12.44
  的 check-config（485–498）：两版的配置代码相同，本版收的配置（含 DirectML 的）v0.12.44 都收。
- 落盘的东西都不挡：没有新键、新文件、新记录形状。
- 退回以后：Lab 操作在回执之后的核验失败又答 `device_error`（退出码 4），MCP 的 Lab 工具错误又不带 actinglab 的消息；actingctl 的命令和本版一样。
- 客户端和 Runtime 的配对：
  - v0.12.44 的安装工具装本版：照常，不带标志，见上面的升级。
  - **actinglab ≤ v0.12.44 和 MCP（actingctl ≤ v0.12.44）连本版的 Runtime**：线上什么都不变。合同 crate、`crates/runtime-host` 和 `apps/actingd` 自 v0.12.44
    没改，Runtime 收的请求、发的回执和写的记录都和 v0.12.44 一样；`runtime-client` 的改动只在客户端进程里（错误类型）。旧 actinglab 遇到回执之后的核验失败，
    照旧答 `device_error`（退出码 4，v0.12.44 的 `apps/actinglab/src/lab2_cli/operation.rs` 103–105、315–317），那是旧客户端自己的答法。
  - 旧 MCP 跑本版的 actinglab（MCP 跑的是安装根里的 `<root>\tools\actinglab.exe`，本版 `apps/actingctl/src/mcp/lab.rs` 722）：退出码 5 照它原有的规则读成
    `runtime` 类（v0.12.44 `lab.rs` 861–886），`req_id` 从 details 取（824–830），cause 带 `lab_operation_check_failed` 和含 `effect` 的 details，只是没有消息那条
    链接。生产根里的 actingctl（含 MCP 服务和看门狗）、`tools\actinglab.exe` 和 Runtime 是同一个构建，随部署一起换。
  - **控制台 UI**（UI 仓 main `c03d18ea`，UI v0.12.10：`Cargo.toml` 39–44 把合同、账本、取证、`runtime-client`、配置和内核 crate 都钉在 `a322caa4`，即 v0.12.43；
    `crates/acui-setup/component-interfaces.json` 声明 `actingd-config` [5, 26]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：本版的
    声明和 v0.12.44 逐字相同，线上也没变，所以控制台对本版和对 v0.12.44 一样。`ledger` 7 在 [3, 7] 里，`runtime-client` [3, 5] 和 [3, 6] 有交集，
    `runtime-outcomes` 1 在 [1, 1] 里；配置文件写了修订 27 的成员（切了 DirectML）时不在它声明的 [5, 26] 里，这在 v0.12.44 就是这样，本版没变：控制台把配置
    当 JSON 读、只查 `vision_provider_manifest`（`crates/acui-installation/src/lib.rs` 74–77），画面刷新走 `observe_readonly`
    （`crates/acui-source/src/workbench/preview.rs` 586–595），源码里没有 Lab 操作的请求（`run_contained_lab_operation`）。本版没有东西逼控制台换钉；换钉、
    声明 [5, 27] 仍是为了让它的声明和写了新成员的文件一致。

运行上的变化：

- 部署本身不改变运行；只有 Lab 操作在回执之后的核验失败改答 `lab_operation_check_failed`（退出码 5，带 `req_id` 和效果），MCP 的 Lab 工具错误多带
  actinglab 的消息。

本版合并的 PR：

- #1540 LAB-0（#434，登记 §1.287）：只改契约的四个文件。`contracts/lab-two-entries.md`（新：一个端口上的 Lab 和 UI 两个入口和握手、UI 白名单和被拒时的答法、
  六个发起者词、不可逆操作只记发起者、守卫只报告不拦、模块怎么拆）；`contracts/lab-operations.json`（新：操作表 v1，168 项）；`component-interfaces.md` 的
  四条计划行（不生效）；登记 §1.287。没有代码、测试、键和结果码。
- #1542 LABDO-1（#433）：`runtime-client` 在回执之后的核验失败上带 `LabOperationAnswer`；actinglab 答 `lab_operation_check_failed`（退出码 5）；MCP 的 Lab 工具
  带上 actinglab 的消息；契约 §9.3 写下这条规则；新码 1 个；新测试 2 个（`gate_433_labdo1`，actinglab 和 MCP 各一个），改造 1 个（`gate_mcpt1`：原来断言不带
  消息，现在要求带上原文）。
- #1541 LAB-1（#434）：新库 `crates/lab-ops`（操作表读取、发起者类型、默认位置）；actingctl 的 16 个读写模块和 5 个辅助模块搬进去，actingctl 的行为不变；操作表
  按搬过来的代码改了 11 行的 `requests`，23 行加注（代码还在 actingctl 里，以后另一片再搬）；结果码目录头的片名映射；新测试 2 个（`lab_ops_table_complete`、
  `lab_ops_locate_defaults`），搬过去的 27 个门测试只改包名前缀，旧名登记进 `ci/retired-gates.toml`；两条架构守卫扩到 `crates/lab-ops`；没有新码、新键。

## v0.12.44

本版是 v0.12.43 之后合并的 7 个 PR：#407 GPU-1 的六片 A（#1532）、D（#1533）、B1（#1534）、B2（#1536）、C（#1535）、E（#1537），和只改 CI 的 FLK-08（#1538）。
先说部署：`actingd-config` 从 [5, 26] 升到 [5, 27]，其余接口声明都不变（账本仍是 [3, 7]），部署不用带 `--accept-ledger-one-way`，计划里也没有 `ledger_verify`
这一步；v0.12.43 的安装工具装本版照常（见下面的升级）；装上以后能退回 v0.12.43，只要配置文件里还没写修订 27 的三样东西（见下面的退回）。没有新键，默认值文件
没改。运行上要知道的是这几处：

- **OCR 可以跑在 DirectML 的 GPU 上，默认关**（GPU-1）。配置文件不改，OCR 和 v0.12.43 逐字节相同，照旧跑 CPU：`cpu` 的运行库闭包、会话选项、会话键摘要和执行
  证明都没变，不枚举显卡、不核钉表，启动记录也和以前一样。本版的 Runtime 包、Tools 包和安装器都不带 DirectML 的运行库文件（两个包的文件清单和 v0.12.43 一样）。
- **打开它要一个单独的视觉根，加上配置 `vision` 段里的三样东西**（GPU-1 A、D）：
  - 视觉根的 `ort\` 放钉住的 DirectML 运行库，三个文件：ONNX Runtime 1.24.4 DirectML 版的 `onnxruntime.dll`（17,328,152 B，SHA-256
    `e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40`）、`onnxruntime_providers_shared.dll`（22,040 B，
    `265c8daf29637cb259cac8be9f08f2cd45f3883f0f0e4949cbfddd5b4cbec3b6`）和 DirectML 1.15.4 的 `DirectML.dll`（18,527,776 B，
    `9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1`），许可文件放在旁边，`models\` 照原来的放。钉表在 `crates/vision-ffi/src/runtime_pin.rs`
    （`DIRECTML_RUNTIME_PIN`），和 `scripts/windows-tools/windows-tool-sources.v1.json` 的两条来源 `onnxruntime-directml-1.24.4`、`directml-1.15.4`（官方 NuGet 地址
    和包的大小、哈希）由逻辑测试钉成一致。建议另开一个根（比如 `<install root>\vision-dml`，用 `root` 指过去），CPU 的根原样不动，切回去就不用动文件。
  - 三样东西：`"execution_provider": "directml"`；`"directml_device": {"expected_identity": "dxgi-pci:<vendor>:<device>:<subsys>:<rev>"}`（小写十六进制，4、4、8、2
    位，check-config 列出的就是这个写法；不写 DXGI 序号、不写 LUID，写了是未知成员，`config_decode_failed`）；可选的 `"gpu_fallback"`（`cpu`，默认；或 `refuse`）。
    `directml` 必须带 `directml_device`、不许带 `cuda_device`；`cpu` 和 `cuda` 不许带这两个新成员；违反就是 `vision_config_invalid`（`config_assemble`）。
  - 这一段在 Runtime 下次启动时生效，不是设置项：不进键表，在线设置改不了它，控制台也不写。照 `INSTALL.md` 改文件：先对候选配置跑
    `actingcommand-actingd.exe check-config --config <候选>`，看报告里的 `vision.directml`，再把候选写成 `<install root>\actingd.config.json`，只重启 Runtime，模拟器
    不动；不用 `acsetup --commit-config`。
  - check-config 对 `directml`：`ort\onnxruntime.dll` 和 `ort\onnxruntime_providers_shared.dll` 按钉表核大小和 SHA-256，缺、不一样或读不了就以
    `vision_runtime_unavailable` 失败（`config_vision`，带 `path` 和 OS 错误），启动也一样拒绝。然后列出全部 DXGI 适配器，按身份选卡（照 `EnumAdapters1` 的顺序，
    只留物理卡，去掉间接显示、软件和半虚拟的副本，正好剩一张才用；LUID 只记录、不比对，从不换用别的卡），再核 `ort\DirectML.dll`。报告的 `vision.directml` 是
    `{expected_identity, gpu_fallback, adapters[…], resolved {dxgi_index, luid}, unavailable_reason, runtime_pin {onnxruntime, providers_shared, directml},
    nn_execution_provider}`；DirectML 用不了时 check-config 不失败，只带一条 `vision_gpu_unavailable` 的 `note`（`adapter_missing`、`adapter_not_physical`、
    `adapter_ambiguous`、`directml_library_missing` 或 `directml_library_mismatch`）。NN 模型在 `directml` 下照旧跑 CPU（`nn_execution_provider` 是 `cpu`）。
- **GPU 用不了时，`gpu_fallback=cpu` 让 OCR 转到 CPU，状态里亮一条 `vision_gpu_unavailable`；`refuse` 则让 OCR 请求失败**（GPU-1 B2、C、D、E）。
  - 启动时选不到卡或 `DirectML.dll` 不对：照常启动，写一条警告（stage `vision_startup`，在 actingd 的开发日志里），perf-status 的 `warnings` 里显示到第一次 OCR 载入
    完成。第一次 OCR 载入（运行库初始化）再判一次，以它为准：还是用不了，`cpu` 下整个进程用 CPU 闭包（不载 `DirectML.dll`），一条不带 `model_name` 的警告，直到重启。
  - 建 DirectML 会话失败（选不到卡、注册不了、建不成、读不出节点分布）：这个模型这次载入整个改到 CPU（每次载入只回退一次），一条带 `model_name` 的警告；检测模型
    建不成时识别模型也一起挪。
  - 跑失败（GPU-1 E：设备被移除或重置等 ONNX Runtime 的运行错误，看门狗超时不算）：这次请求失败（`ProviderUnavailable`，`DirectML cannot be used (inference_failed)`），
    内核卸载这个引擎、不封存，下一次请求重建它（重新选卡，照上面的规则回退）；重建后的第一次调用又失败，这次请求也失败，之后每次载入都整模型跑 CPU，直到进程结束，
    带一条 `vision_gpu_unavailable`（`inference_failed`）；重建后第一次成功就算恢复，以后再失败再重建一次。
  - `refuse`：不回退，请求失败（`ProviderUnavailable` 或 `ProviderFailure`，消息以 `DirectML cannot be used (<原因>)` 开头），下一次请求再试；跑失败每次都重建，
    不挪到 CPU。启动时的那条警告照样显示。
  - perf-status 的 `warnings`：进程级的一条，加上每个跑在 CPU 上的模型各一条，相同的只列一次；跑在 DirectML 上的模型不显示。MCP `ac_overview` 的说明补上了这个码。
  - 启动记录（`provider.startup` 的 Binding）多两条：`execution_provider`（配置 `directml`；实际 `directml`，用不了时 `cpu` 下记 `cpu`、`refuse` 下记 `none`）和
    `directml_adapter`（配置的身份；实际 `<身份>;index=<n>;luid=<luid>` 或 `none`）。能用 DirectML 时，`runtime_library_paths[i]` 列的是 DirectML 的闭包
    （`DirectML.dll` 排在 `onnxruntime.dll` 前面）。警告本身不进账本。
  - 证据：OCR 行的执行段是 `directml`/`directml`（CPU EP 不注册，`fallback_observed` 按每次调用记，跑过的模型角色里有 CPU 节点就是 true），或整模型回退以后的
    `directml`/`cpu`（`fallback_observed` 为 true）；同一个目标的绑定不再比 `fallback_observed`。每次建 DirectML 会话都读节点分布，报告升到
    `actingcommand.ppocr_node_placement_diagnostic.v2`（加速的 EP、节点数和 `device`），存成已有的 `DiagnosticJson` 诊断件。

配置：接口修订只升一处，按代码和 `contracts/component-interfaces.md` 核过：`actingd-config` [5, 26] 改为 [5, 27]（GPU-1 D，登记 §1.286：`vision.execution_provider`
的新值 `directml`、新成员 `vision.directml_device` 和可选的 `vision.gpu_fallback`；其余五片都不升）。`distribution/windows/component-interfaces.json`、合同表
`actingd-config` 27 一行、架构测试（`tools/actinglab-architecture/tests/component_interfaces.rs` 的 `V0120` 表）三处一致。其余都不变（设计 §6.2、§6.3）：`ledger`
仍是 [3, 7]，`runtime-client` 仍是 [3, 6]，`package` 仍是 [3, 7]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。

- 修订 27 读得了修订 5 到 26 的配置文件，没有例外。不写这三样的文件保持原来的修订，v0.12.43 照常读；写了任何一样，v0.12.43 就解不开这个文件（它的
  `VisionConfigFile` 拒收不认识的成员，`OnnxExecutionProvider` 只有 `cpu`、`cuda`：v0.12.43 的 `apps/actingd/src/config.rs` 501–513、
  `crates/vision-ffi/src/artifacts.rs` 460–465，报 `config_decode_failed`，同一文件 1240–1241）。
- 配置键：没有新键。三样都只在 `vision` 段里，`vision` 段不是键表里的键（`crates/runtime-config/src/ui_tier.rs` 15），键表、默认值文件和配置示例都没改，部署
  计划的默认值对比是空的。
- 结果码：新码 1 个 `vision_gpu_unavailable`（warning，`ladder_class` none，待定稿，since 0.12.44；必带 `execution_provider`、`gpu_unavailable_reason`、
  `gpu_fallback`，可带 `stage`、`model_name`）；新值键 2 个（`gpu_unavailable_reason`、`gpu_fallback`）；新词表 2 个：`gpu_unavailable_reason` 9 个词
  （`adapter_missing`、`adapter_ambiguous`、`adapter_not_physical`、`directml_library_missing`、`directml_library_mismatch`、`provider_register_failed`、
  `session_build_failed`、`placement_unread`、`inference_failed`），`gpu_fallback` 2 个词（`cpu`、`refuse`）；已有词表 `execution_provider` 加 `directml`；新位置 1 个
  `vision_startup`（actingd，待定稿）；`vision_runtime_unavailable`（已定稿）多可选值 `path`，它和 `vision_config_invalid` 的说明补上 DirectML。没有退役的条目。结果码
  目录从 3890 个码、1740 个位置变成 3891 个、1741 个；本版的发布文件只有一条：`vision_runtime_unavailable` 多了可选值 `path` 以后的签名。
- 账本：不升，没有新记录形状。切到 `directml` 以后多出来的都是已有形状里的新值：两条 Binding 的 `field`、`configured`、`resolved` 是自由文本，读者只核长度和非空
  （`crates/actingcommand-contract/src/event/payload/provider.rs` 72–77、134–143）；OCR 行的 `requested_provider`、`resolved_provider` 是 `String`
  （`event/artifact.rs` 509–510），`fallback_observed` 是已有的 `Option<bool>`；v2 报告在已有种类的诊断件里，账本只记它的元数据。合同 crate 的事件部分自 v0.12.43
  没改。
- 程序文件：workspace 成员、Runtime 包和 Tools 包的文件清单都和 v0.12.43 一样（`.github/workflows/windows-remote-build.yml` 650–669 自 v0.12.43 没改）；没有新依赖，
  选卡用手写的最小 DXGI 接口和 D3DKMT，`dxgi.dll`、`gdi32.dll` 只从 System32 载入，进程的 DLL 搜索不变。`apps/actingctl` 只改了 MCP `ac_overview` 说明里的一行，
  MCP 工具仍是 41 个；`apps/actinglab`、`crates/ledger`、`crates/ledger-forensics` 自 v0.12.43 没改；合同 crate 只改了键表（两个键）和节点分布报告 v2。

升级时要知道的（按安装工具的代码核过；`apps/actingctl` 自 v0.12.43 只改了 `mcp/tools.rs` 的一行说明，下面的行号在 v0.12.43 和本版相同）：

- 不用 `--accept-ledger-one-way`：两边的 `ledger` 都是 [3, 7]，`previous_max_in_next`（7 在 [3, 7] 里）和 `one_way`（7 不大于 7）都过
  （`apps/actingctl/src/install/plan.rs` 281–294），带上也不改判断；现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`（908），apply 不读账本。
- `actingd-config` 升到 27 不挡：没有哪条边比两边的范围。安装工具只算 `installer_knows_keys`（353–369；`vision` 段不在键表和默认值里，`unknown_keys` 不往里查，
  499–529）和 `next_check_config`（895–907：本版的 check-config 收现行配置，它的 `vision` 是 `cpu`）。
- 其余的边都过：`runtime-client` 两边都是 [3, 6]，安装工具自己的声明 I 编在程序里（`install/host.rs` 23–25），v0.12.43 的 I 是 [3, 6]，`installer_meets_previous`、
  `installer_meets_next` 都过（308–323）；`install-control` [0, 1] 相同，`runtime-outcomes` 1 在 I 的 [1, 1] 里（295–307）；`package` 两边都是 [3, 7]，
  `bundle_max_in_next` 在单一位置根上只有 `pack_declaration_unknown` 警告（325–348）。
- **v0.12.43 的安装工具没有新东西要发或要读**：它连 Runtime 发的请求和读到的答复形状都没变（`runtime-client` 6 不变；C 只是让官方 OCR 的证据认 `directml`，安装
  工具不跑任务）。从本版的构建目录跑本版的安装工具，结果相同。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，配置还没写修订 27 的成员（刚部署完就是这样），回退门的配置一项也过（见下面的退回）。

退回 v0.12.43（按安装工具的代码核过）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。`ledger` 两边都是 [3, 7]：`previous_max_in_next` 和 `one_way` 都过（`plan.rs` 281–294）；`runtime-client`
  两边都是 [3, 6]，I [3, 6] 和两边都有交集；`install-control`、`runtime-outcomes` 两边相同；`package` 两边都是 [3, 7]；`installer_knows_keys` 不查 `vision` 段，有没有
  新成员都过。部署的自动回退（`install/deploy.rs` 715–733，不带标志）、`rollback plan|apply` 和起过本版 Runtime 以后的 `install undo`（`install/undo.rs` 177–188、
  324–356）走的是同一组边和同一个回退门（`install/update.rs` 378 起）。
- `actingd-config` 27 → 26：没有哪条边比两边的范围，挡住退回的是配置的内容。回退门的配置一项（`update.rs` 485–498）让 v0.12.43 的 check-config 读现行配置：`vision`
  只要写了 `directml`、`directml_device` 或 `gpu_fallback`，v0.12.43 就以 `config_decode_failed` 拒绝（见上面的配置），退回以 `rollback_gate_failed`
  （`rollback_gate_parts` 含 `config`，check-config 的拒绝是它的链接，`update.rs` 152–173）结束，什么都不改；退回计划本身不先拒，因为退回时 `next_check_config` 不算
  接口边（`plan.rs` 1019–1025）。**所以切过 DirectML 的，退回之前先把 `vision` 改回 `{"execution_provider": "cpu"}`（两个新成员都删掉）、重启 Runtime，再退回**
  （设计 §8.3）。没写这三样的配置不用改，生产现在就是这样。
- 回退门其余几项：v0.12.43 自己的读者读部署之后写的账本页（`ledger_readability`，`update.rs` 395–432，只对本版替换掉的那个构建算，就是 v0.12.43）：账本没有新形状，
  切过 DirectML 留下的 Binding 和 `directml` 的 OCR 行它也读得了；v0.12.43 在这个根上最近一次打开在它的期限减余量之内（433–484）。
- 落盘的东西都不挡：没有新键、新文件、新记录形状；DirectML 的视觉根是另一个目录，退回以后不读它。
- 退回以后：OCR 只能跑 CPU（或 CUDA），不再有 `vision_gpu_unavailable`。
- 客户端和 Runtime 的配对（`crates/ledger`、`crates/ledger-forensics`、`apps/actinglab` 和合同 crate 的事件部分自 v0.12.43 没改；下面旧读者的行号是 v0.12.43 的）：
  - v0.12.43 的安装工具装本版：照常，不带标志，见上面的升级。
  - **本版自己的读者**：Runtime、actinglab、actingctl 和 MCP 都经本版的识别包和 `runtime-client` 读证据，认得 `directml` 的两种绑定。
  - **actinglab ≤ v0.12.43、actingctl 和 MCP 读本版的账本**：配置还是 `cpu` 时，本版写下的和 v0.12.43 一样，什么都不变。切到 `directml` 以后，事件页、离线
    `resource restore` 和取证照常：新东西都只是已有形状里的值（见上面的账本）。**只有一处读不了**：钉 v0.12.43 或更早的 `runtime-client` 跑任务、等结果时
    （actingctl `task-run`、actinglab `lab run`、MCP `ac_run_pack`，都经 `submit_prepared` → `flow_output`，`crates/runtime-client/src/client.rs` 2392–2459、4107），
    要读运行的 OCR 观察诊断件做官方 OCR 投影；它的执行证据用封闭枚举 `{cpu, cuda}`（664–667，`OcrProviderEvidence` 4213–4217），读到 `requested_provider`
    为 `directml` 就解不开（`runtime_official_ocr_observation_malformed`，5211–5212），客户端在终点之后报 `runtime_official_ocr_projection_failed_after_terminal`
    （4116），运行本身已经在 Runtime 里结束。整模型回退到 CPU 以后也一样（`requested_provider` 还是 `directml`）。生产根里的 actingctl（含看门狗和 MCP 服务）和
    `tools\actinglab.exe` 和 Runtime 是同一个构建，随部署一起换；在根外跑的、钉旧版的那几份要在切换之前先换。
  - **控制台 UI**（UI 仓 main `c03d18ea`，UI v0.12.10：`Cargo.toml` 39–44 把合同、账本、取证、`runtime-client`、配置和内核 crate 都钉在 `a322caa4`，即 v0.12.43；
    `crates/acui-setup/component-interfaces.json` 声明 `actingd-config` [5, 26]、`ledger` [3, 7]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：
    - 配置文件没写新成员：照边算全过（合同表的 U\* 行：`actingd-config` 三方共同的 5 到 26，`ledger` 7 在 [3, 7] 里，`runtime-client` [3, 5] 和 [3, 6] 有交集，
      `runtime-outcomes` 1 在 [1, 1] 里；`actingctl install` 的计划不算控制台的边，`plan.rs` 424 的 `ui_edges` 只有闸门测试调用），和连 v0.12.43 一样。
    - 写了新成员：文件是修订 27，不在控制台声明的 [5, 26] 里，三方协商在这个文件上不成立。实际上控制台不解释 `vision` 段：它构建的 workspace（`Cargo.toml`
      2–13）把配置当 JSON 读，只查旧的 `vision_provider_manifest`（`crates/acui-installation/src/lib.rs` 74–77），源码里没有 `directml`；它不跑任务（没有
      `run_contained_task`、`submit_prepared`），画面刷新走 `observe_readonly`（`crates/acui-source/src/workbench/preview.rs` 586–595），那不是任务运行，官方 OCR
      投影是空的；会做三方协商的 acsetup 在 workspace 之外，不构建。
    - 所以切换之前没有东西逼着先换控制台；生产根是单一位置根，里面没装控制台，切换照 `INSTALL.md` 改文件，不用 acsetup。控制台换钉 0.12.44、声明 [5, 27]，是为了
      让它的声明和写了新成员的文件一致。

运行上的变化：

- GPU 上的 OCR（#407 GPU-1）：默认关，配置不改就什么都不变；切到 `directml` 要另开视觉根、写三样配置并重启 Runtime，GPU 用不了时按 `gpu_fallback` 转 CPU 或失败。

本版合并的 PR：

- #1532 GPU-1 A（#407）：`OnnxExecutionProvider::Directml`；按身份 `dxgi-pci:…` 选卡（DXGI 枚举加 D3DKMT 的适配器类型，只认物理卡，LUID 只记不比）；DirectML 运行库
  钉表 `runtime_pin.rs` 和两条工具来源（加冻结断言）；DirectML 闭包、会话键和摘要的 DirectML 分支；新码 `vision_gpu_unavailable` 和它的两个键、两个词表；新测试
  6 个，测试辅助函数补两个 `None` 成员。
- #1533 GPU-1 D（#407）：`vision.directml_device`、`vision.gpu_fallback` 和它们的规则；check-config 和启动按钉表核运行库（`vision_runtime_unavailable` 多 `path`）；
  启动时选一次卡，写两条 Binding，用不了时一条警告（`vision_startup`）；check-config 的 `vision.directml`；`actingd-config` [5, 27]；`INSTALL.md` 写改文件的做法；
  新测试 5 个，架构测试的钉值改一处。
- #1534 GPU-1 B1（#407）：DirectML 会话（关 memory pattern、顺序执行、允许 CPU 节点），影子会话和正式会话的选项出自同一个函数；运行库初始化的 DirectML 顺序；每次
  建会话都重新选卡；DirectML 每次都读节点分布，报告 v2；执行证明的 `cpu_nodes` 和 `fallback_observed`；本片里 DirectML 失败一律报错；新测试 3 个，改造 2 个（报告
  v2），`gate_411_a36c.rs` 的字面量补成员。
- #1536 GPU-1 B2（#407）：`gpu_fallback` 的两种语义；运行库初始化时的进程级 CPU 决定；会话建不成时整模型改 CPU（检测模型连带识别模型）；`OcrModelLoad::Loaded` 带
  执行状态；NN 在 `directml` 下用 CPU；新测试 4 个，没有新码。
- #1535 GPU-1 C（#407）：识别包、内核、runtime-host、`runtime-client` 认 `directml` 的两种绑定（`directml`/`directml` 和整模型回退以后的 `directml`/`cpu`）；
  `fallback_observed` 不进绑定比较（fix 1）；启动警告和各模型的回退警告进 perf-status（fix 2）；MCP `ac_overview` 的说明；新测试 5 个，没有改既有测试。
- #1537 GPU-1 E（#407）：DirectML 跑失败答 `ProviderUnavailable`（`inference_failed`），内核卸载引擎、下次请求重建；重建后第一次又失败，`cpu` 下整模型改 CPU 直到
  进程结束；词表 `gpu_unavailable_reason` 加 `inference_failed`；新测试 1 个。
- #1538 FLK-08：`fact_snapshot_catches_up_with_critical_ledger_events` 挪进不稳定表和不作门禁的 `flaky` 作业（只改 CI；原因没定位到，14 天 649 次运行里失败过
  1 次）。

## v0.12.43

本版是 v0.12.42 之后合并的 2 个 PR：#404 FS-1 + FS-2（#1528）和 FS-3 + FS-2b（#1530）。先说部署：`package` 从 [3, 6] 升到 [3, 7]，`ledger` 从 [3, 6]
升到 [3, 7]，**账本这一步是单向的**：部署要带 `--accept-ledger-one-way`，计划多一步 `ledger_verify`；装上本版以后安装工具拒绝退回 v0.12.42，不论有没有包
用过新的确认方式（见下面的升级和退回）。没有新键，默认值文件没改。运行上要知道的是这几处：

- **包现在可以在整张图（或一个区域）里找出一个锚点的每一处匹配，带遮罩、按几个比例找，按 `anchor_milli` 排序，点下选中的那一个；确认帧上它挪了位置，也能
  找回来再点**（FS-1、FS-2、FS-2b、FS-3）。
  - 整帧多匹配（`repeated_anchor` 版面）改在精确分数图上枚举：直算和变换（FFT）两种引擎取更便宜的一种，分数和直算逐位相同，不走金字塔粗搜。锚点现在可以带
    `mask` 和 `scales`（以前准入拒收）；各比例的匹配合在一起排序（分数降序，同分时声明在前的比例优先，再按 `(y, x)`）、去重叠，同一个对象在不同比例下只算
    一个实例；实例框、点击框和特征偏移按匹配到的比例缩放（取整照 REC-5）。
  - 打分、收集、排序和去重叠共用 5 秒的搜索时限：超时是 `candidate_search_incomplete`，去重后超过 `max_instances` 是 `candidate_projection_budget_exceeded`，
    都不给残缺的列表。
  - 新特征值 `anchor_milli`：实例自己的锚点匹配分（千分，向下取整），只用在 `repeated_anchor` 版面上，不读目标、不带 `offset`；选择策略把它当整数读，门、
    打分和同分排序都能用（FS-2b 补上源声明门这一处，以前读它的策略加载不了）。
  - select 步可以声明 `confirm: "track"` 加 `track_px`（1–64）：确认帧 F2 用同一份事实快照、同一时刻再决定一次；F2 选中的那一个和原来选中的那一个，锚点
    匹配原点的切比雪夫距离不超过 `track_px`，就记 `tracked`，点 F2 上那一个的点击框（绑定 F2）；否则记 `track_lost`，在任何输入之前以
    `selection_decision_mismatch` 失败，带 `confirm`（`track`）和 `shift_px`，F2 谁都没选时带 `selection_kind`。只收 `repeated_anchor` 版面上的单帧 select：
    `track_px` 缺了或越界、`track_px` 没配 `track`、`track` 配 `scan` 或 `fixed_slots`，都以 `contained_task_select_invalid` 拒收。默认的确认
    （`candidate_set`）和 `decision` 不变。
  - **`track`、`anchor_milli` 和锚点的 `mask`、`scales` 都要包声明 `package` 7**（用了它们的包，bundle 声明 `package` [7, 7]）；更早的读者拒收这样的包
    （见配对）。
- **现有的包照旧**：没带 `mask`、`scales` 的锚点投影和以前逐字节相同（钉住的候选集哈希没改）；以前会撞上 5 秒上限的整帧搜索，现在可能做得完。扫描（`scan`）
  选择新拒收带 `scales` 的锚点（明细 `scan anchor=scaled`），以前锚点本来就不能带 `scales`，现有包不受影响。超时的 `candidate_search_incomplete` 里，`count`
  现在可能是 0（先算完分数图再取位置）。
- **账本升到修订 7，只能往前修**（FS-3）。修订 7 是 6 加上 `task.selection_evaluated` 的两种确认 `tracked` 和 `track_lost`。**只有声明了
  `confirm: "track"` 的 select 步才写它们**，没有这种包就一条都不写；但安装工具按两个构建声明的范围判，账本里也不记修订号，所以部署一完成就退不回
  v0.12.42。读这个 Runtime 账本的控制台要建在修订 7 的读者上：UI v0.12.9（钉 v0.12.40，`ledger` [3, 6]）的账本窗口读到含这种记录的那一页会失败。所以
  **在任何声明 `track` 的包运行之前，先把控制台换成钉 0.12.43 或更新、`ledger` 上限至少 7 的构建**（见配对）。

配置：接口修订两处升，都按代码和 `contracts/component-interfaces.md` 核过：`package` [3, 6] 改为 [3, 7]（FS-1、FS-2 升一次，FS-3 的 `confirm: "track"`、
`track_px` 也进修订 7，不再升，FS-2b 也不升；登记 §1.282、§1.283、§1.284、§1.285），`ledger` [3, 6] 改为 [3, 7]（FS-3，登记 §1.284）。`distribution/windows/component-interfaces.json`、
合同表里 `package` 7 和 `ledger` 7 两行、架构测试 K27（`tools/actinglab-architecture/tests/component_interfaces.rs`）三处一致。其余都不变：`actingd-config`
仍是 [5, 26]，`runtime-client` 仍是 [3, 6]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。

- 没有新键，键表、默认值文件和配置示例都没改，部署计划的默认值对比是空的。
- 结果码：没有新码、新位置。`selection_decision_mismatch`（待定稿）多三个可选值 `confirm`、`shift_px`、`selection_kind`，说明和处理补上 track 一段；键表多
  `confirm`（词表 `selection_confirm`）和 `shift_px`（整数）；新词表 `selection_confirm`，只有 `track`；`candidate_search_incomplete` 只改了说明。结果码目录
  仍是 3890 个码、1740 个位置；没有新定稿或退役的条目，所以本版的发布文件是空的。
- 账本：`ledger` 7 是 6 加上确认 `tracked {candidate_set_sha256, decision_sha256, shift_px, projection?}` 和
  `track_lost {candidate_set_sha256, decision_sha256, shift_px?, projection?}`，拒收不认识的成员；读者只收 v1 的 `repeated_anchor` 投影、没有 `multi_select`、
  64 位十六进制哈希、`tracked` 的 `shift_px` 不超过 64（`crates/actingcommand-contract/src/event/payload/task_selection.rs` 358–375，校验 1239–1281）。别的片加的
  都不进读者核的形状：`anchor_milli` 是已有的整数特征，缩放后的实例框是已有的矩形，锚点原点不进记录，`confirm`、`shift_px` 和 `selection_confirm` 读者只核
  拼写。SQLite 格式 2、`actingcommand.event.v3`、记录类型、表、索引和每个视图的类型表都不变，没有迁移，也不用新的状态根，修订 3 到 6 的账本照常读。
- 程序文件：workspace 成员、Runtime 包和 Tools 包的文件清单都和 v0.12.42 一样。`apps/actingctl` 只改了 HS-2b 的安装测试（改用 [3, 6] 的夹具，断言不变），
  `apps/actinglab`、`crates/runtime-client`、`crates/ledger`、`crates/ledger-forensics` 自 v0.12.42 没改；合同 crate 只改了 `task_selection.rs` 和键表；
  MCP 工具仍是 41 个。

升级时要知道的（按安装工具的代码核过；安装命令的代码自 v0.12.42 只改了一个测试文件，下面的行号在 v0.12.42 和本版相同）：

- 账本单向：`update plan`、`update apply` 不带 `--accept-ledger-one-way` 时，`ledger` 的 `one_way` 边不过（R\*.max 7 大于 Rp.max 6，
  `apps/actingctl/src/install/plan.rs` 288–294），以 `interface_edge` 拒绝（1019–1031），不过的只有这一行，什么都不改；带上以后 `previous_max_in_next`
  （6 在 [3, 7] 里，282–287）和 `one_way` 都过，计划的 `notice.rollback_window_bounded` 是 true（197、214–220），计划文件的 `inputs` 记下
  `accept_ledger_one_way`（955–961）。
- 账本校验：现行和目标的 `ledger` 范围不同，所以计划有 `ledger_verify`（908），apply 把它排在所有换文件的步骤之前（`install/update.rs` 527–533），跑的是
  本版构建里的 `actingcommand-actingd.exe ledger-maintenance verify --config <root>\actingd.config.json`（update.rs 829–833，`install/programs.rs` 293–306），
  期限是 `install.child.ledger_verify_timeout_ms`（默认 1800000）。现行账本只有修订 3 到 6 的记录，本版读得了。不过或到期限时怎么办，和 v0.12.39 一节的账本
  校验一样：什么文件都没换，但 Runtime 已经停了、hold 还在。
- 没有自动回退：部署的新 Runtime 起来以后检查不过时，部署自己的自动回退先按回退算一次计划（`install/deploy.rs` 715–733，不带标志），`ledger` 边不过（见下面的
  退回），以 `rollback_gate_failed`（`rollback_gate_parts` 是 `interface_edges`，709–714）结束，新构建和 hold 都留着。起过本版 Runtime 的操作，`install undo`
  也按回退判（`install/undo.rs` 177–188；边在 324–356，用 `plan.rs` 376–396 的 `rollback_edges`，不带标志），同一条边以 `interface_edge` 拒绝；没起过 Runtime
  的未完成操作照旧能撤销。出了问题只能往前修。
- 其余的边都过：`runtime-client` 两边都是 [3, 6]，安装工具自己的声明 I 编在程序里（`install/host.rs` 23–25，就是它构建里的 `component-interfaces.json`），
  v0.12.42 的 I 是 [3, 6]，`installer_meets_previous`、`installer_meets_next` 都过（`plan.rs` 308–323，交集在 234–236）；`install-control` [0, 1] 相同，
  `runtime-outcomes` 1 在 I 的 [1, 1] 里（295–307）；`package` 的 `bundle_max_in_next` 在单一位置根上只有 `pack_declaration_unknown` 警告（325–348）；现行
  配置没有它不认识的键（`installer_knows_keys`，353–369），本版的 check-config 收现行配置（`next_check_config`，895–907）。
- **v0.12.42 的安装工具没有新东西要发或要读**：它连 Runtime 发的请求和读到的答复形状都没变（`runtime-client` 自 v0.12.42 没改，合同 crate 只多了
  select 记录的两个确认词和两个结果值键，都只在声明 `track` 的包运行时出现，部署时还没有）；账本校验跑的是本版构建里的程序。从本版的构建目录跑本版的安装工具，结果相同。

退回 v0.12.42（按安装工具的代码和 HS-2b 的安装测试核过）：

- 安装工具拒绝退回。本版声明 `ledger` [3, 7]，v0.12.42 声明 [3, 6]。`rollback plan`、`rollback apply` 的 `previous_max_in_next` 要求现行构建的 Rp.max 落在
  目标的 R\* 里（`plan.rs` 282–287）：7 不在 [3, 6] 里，这条边不过，两条命令都在算回退门之前就以 `interface_edge` 拒绝（1019–1031），不过的只有这一行，
  什么都不改。带不带 `--accept-ledger-one-way` 都一样：它只放行 `one_way`（288–294；退回方向 6 不大于 7，这条本来就过）。HS-2b 的安装测试
  （`install/gate_inst1_update.rs` 1857–1915）在 [3, 5] 和 [3, 6] 两个构建上钉的就是这两件：更新缺标志只有 `one_way` 不过，退回带不带标志都只有
  `previous_max_in_next` 不过；判法只看两个范围，本版的 [3, 6] 和 [3, 7] 走同一段代码。
- **账本里不记修订号，所以部署一完成就退不回去**：安装工具判的是两个构建清单里声明的范围（`plan.rs` 728–737 的 `speaks()`），不读账本；账本的写者元数据只有
  schema、owner、pid、在不在写和起止时间（`crates/ledger/src/global/read_only.rs` 61–68），SQLite 格式和事件 schema 都没变（合同表 `ledger` 7 一行）。所以
  刚部署完、一条修订 7 的记录都还没写时，退回也一样被拒；有没有包用过 `track` 都一样。
- 同一条边挡住部署的自动回退和起过本版 Runtime 以后的 `install undo`（见上面的升级）；v0.12.42 的 actingctl 跑的是同一份安装代码，判的也一样。
- 为什么要这样（只作记录，不是操作步骤）：v0.12.42 的合同 crate 里 `TaskSelectionConfirmation` 是按 `kind` 打标签、`deny_unknown_fields`、没有兜底的封闭
  枚举（`task_selection.rs` 245–347，v0.12.40 到 v0.12.42 相同），解不开 `tracked`、`track_lost`；它的账本逐条认证时解码失败就是 `corrupt_ledger_record`
  （`crates/ledger/src/global/sqlite.rs` 4138–4144）。这种记录在最新的 `ledger_observer.open_verify_tail_records` 条里时 v0.12.42 起不来，含它的页整页失败，
  回退门的 `ledger_readability` 也不过。
- v0.12.42 一节的退回条件是退回 v0.12.41 的条件；本版连 v0.12.42 都退不回去，它们不适用。
- 客户端和 Runtime 的配对（`crates/runtime-client`、`crates/ledger`、`crates/ledger-forensics`、`apps/actinglab` 自 v0.12.42 没改，合同 crate 的事件部分自
  v0.12.40 到 v0.12.42 没改，所以下面旧读者的行号在 v0.12.40、v0.12.42 和本版相同）：
  - v0.12.42 的安装工具装本版：照常，**要带 `--accept-ledger-one-way`**，计划有 `ledger_verify`，见上面的升级。
  - **本版自己的读者**：Runtime（打开账本、`ledger-maintenance verify`、回退门的读页）、actinglab、actingctl 和 MCP 都经本版的合同 crate 解码，认得两个新词。
  - **本版不写修订 7 的记录，除非有包声明了 `track`**：`tracked`、`track_lost` 只在 `crates/execution-kernel/src/contained_task/selection.rs` 1864、1877 构造，
    都在 `SelectConfirm::Track` 分支里（1823），这个分支只在准入记下的 `confirm` 是 `track` 时走（1742）；`confirm` 没写时是 `candidate_set`（214–215），准入
    只在 `repeated_anchor` 单帧 select 上、带合法 `track_px` 时收 `track`（`track_refusal` 476–491，在 404 调用）。所以控制台和旧读者只有在这种包运行以后才会
    碰到这种记录。
  - **actinglab ≤ v0.12.42 在线**：它的 Lab 操作只按自己的请求、关联、租约和动作查事件（`crates/runtime-client/src/client/lab_operation.rs` 178–215），任务的
    select 记录不在里面，`do` 照常。它读到含修订 7 记录的事件页时（`lab`、`forensic` 等带载荷的投影；只有 `cli`、`concise` 不带载荷，
    `crates/ledger/src/global/projection.rs` 440），回执按它的合同 crate 解不开，以 `runtime_receipt_decode_failed` 失败，连接作废
    （`crates/runtime-client/src/ipc.rs` 228–233）。本地检查声明 `package` 7 的包（`package check`，`apps/actinglab/src/package_cli.rs` 64）在 v0.12.42 的门里
    被拒：select 的 `track_px` 不在它认的成员里（`crates/pack-containment/src/source/declarations.rs` 1036），`confirm` 只认 `candidate_set` 和 `decision`
    （1098–1099），特征值不认 `anchor_milli`（2246），锚点带 `mask`、`scales` 准入拒收，搜索里也拒（`crates/recognition-pack/src/candidate_layout.rs`
    1010–1025）。
  - **actinglab ≤ v0.12.42 离线 `resource restore`**：开账本时读并认证每一条记录（`apps/actinglab/src/resource_restore.rs` 117–128 →
    `crates/ledger/src/global/evidence.rs` 922–923 → `sqlite.rs` 1979–1994），账本里只要有一条修订 7 的记录，就以 `corrupt_ledger_record`（`sqlite.rs`
    5555–5561）整次失败，报 `resource restore ledger: …`；没有这种记录时照常。所以声明 `track` 的包跑过以后，离线恢复要用本版的 actinglab。
  - **控制台 UI v0.12.9**（`Cargo.toml` 39–44 把合同、账本、取证、`runtime-client`、配置和内核 crate 都钉在 `232b3ed8`，即 v0.12.40；
    `crates/acui-setup/component-interfaces.json` 声明 `actingd-config` [5, 26]、`ledger` [3, 6]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）：
    - 修订 7 的记录：它用 `ui` 投影读事件页（`crates/acui-source/src/lib.rs` 921，另有 225、971 和 `workbench/preview.rs` 537）。`ui` 投影带公开载荷
      （Runtime 的 `projection.rs` 452–455），公开载荷里就是整条 select 记录，确认原样保留，只把原因削成码（`crates/actingcommand-contract/src/event/payload.rs`
      13329–13330、5303 起），它钉的合同 crate 解不开 `tracked`、`track_lost`，这一页以 `runtime_receipt_decode_failed` 失败。没有声明 `track` 的包运行过，
      账本里就没有这种记录，控制台照常；别的请求和回执的形状自 v0.12.42 都没变。
    - 照边算（合同表的 U\* 行；`actingctl install` 的计划不算控制台的边，`plan.rs` 424 的 `ui_edges` 只有闸门测试调用）：`ledger` 的 `next_max_in_ui` 不过
      （7 不在 [3, 6] 里），`previous_max_in_ui` 过（6 在里面）；`runtime-client` [3, 5] 和 [3, 6] 有交集，`actingd-config` 26 三方共有，`runtime-outcomes` 1
      在 [1, 1] 里。生产根是单一位置根，里面没装控制台。
    - 声明 `package` 7 的包：控制台不准入包。它构建的 workspace（`Cargo.toml` 2–13）里没有用执行内核的代码，唯一用它的 `crates/acui-setup/src/maintenance.rs`
      14 在 workspace 之外，不构建。
    - 所以控制台要换成建在修订 7 读者上的构建（钉 0.12.43 或更新、`ledger` 上限至少 7），再让声明 `track` 的包运行。
  - **actingctl**：生产根里的 actingctl（含看门狗和 MCP 服务）和 Runtime 是同一个构建，随部署一起换，认得新词；看门狗只连接、读 runtime-info
    （`apps/actingctl/src/watchdog/observe.rs` 572–591），不读事件页。钉 v0.12.42 或更早的 actingctl（在根外跑的那份）读到含修订 7 记录的事件页时，带载荷的
    投影以 `runtime_receipt_decode_failed` 失败（`ipc.rs` 228–233），`cli`、`concise` 投影照常。
  - **MCP**：Lab 工具（`ac_lab_do`、`ac_pack_check` 等）跑安装里的 `<root>\tools\actinglab.exe`（`apps/actingctl/src/mcp/lab.rs` 3–4），部署以后就是本版的
    actinglab，认得新包和新记录；`ac_events`、`ac_run_evidence` 和 actingctl 一样走 `runtime-client`。钉 v0.12.42 或更早的 MCP（根外那份）和同版的 actingctl、
    actinglab 一样：读到这种记录的页失败，检查这种包被拒。

运行上的变化：

- 整帧搜索和跟踪确认（#404 FS-1 到 FS-3）：`repeated_anchor` 的整帧多匹配改在精确分数图上做，现有包结果不变；账本修订 7，部署带 `--accept-ledger-one-way`、
  多 `ledger_verify`，装上后退不回 v0.12.42；声明 `track` 的包运行之前先换控制台。

本版合并的 PR：

- #1528 FS-1 + FS-2（#404）：`repeated_anchor` 的整帧多匹配改在精确分数图上枚举（直算或 FFT，取更便宜的），锚点收 `mask` 和 `scales`，各比例合并、排序和
  去重叠都算进 5 秒时限，超时或超上限都不截断；扫描准入拒收带 `scales` 的锚点；特征值 `anchor_milli`，只收 `repeated_anchor`；`package` [3, 7]；新测试 7 个，
  改造 2 个，没有新码、新键。
- #1530 FS-3 + FS-2b（#404）：`repeated_anchor` 上的单帧 select 可以 `confirm: "track"` 加 `track_px`（1–64），F2 再决定一次、`track_px` 内找回就点 c2 的点击框
  （`tracked`），否则输入前失败（`track_lost`，复用 `selection_decision_mismatch`，新值 `confirm`、`shift_px`、`selection_kind`，新词表 `selection_confirm`）；
  选择策略可以读 `anchor_milli`（源声明门和内核一起收）；`ledger` [3, 7]；新测试 4 个，改造 HS-2b 的安装测试和 K27 两处。

## v0.12.42

本版是 v0.12.41 之后合并的 1 个 PR：#402 HS-5e。先说部署：接口声明都不变（账本仍是 [3, 6]），部署不用带 `--accept-ledger-one-way`，计划里也没有
`ledger_verify` 这一步；v0.12.41 的安装工具装本版照常，本版没有它要发或要读的新东西（见下面的升级）；装上以后能退回 v0.12.41，条件见下面的退回。没有新键，
默认值文件没改。运行上要知道的只有这一处：

- **Lab 里被前台检查拒掉的点按现在读作「未执行」，不再是「结果未定」**（HS-5e）。点按或别的指针输入，在应用不在前台（`application_not_foreground`）或读不出
  前台（`application_foreground_unknown`）时由前台检查拒绝。这个检查在唯一的输入入口里、写下输入意图之前，所以这种情况下**设备上什么都没有发出去**。以前 Lab
  只要输入调用一返回，就把效果记成 `indeterminate`，只有输入的结果事件才会改它，而被拒的输入从来不写结果事件；于是记录说一个从没按下的点按「结果未定」，
  `do --capture --record` 把它当成结果未定的点按记下，这一步一直开着。
  - 现在：记录的 `input_returned` 为 false，效果 `not_performed`，没有输入意图、动作和输入事件；失败照旧在 `input` 阶段、码照旧，回执照旧是 `failed`。`do` 的答复
    是 `executed: false`、`effect: not_performed`；`do --capture --record` 什么都不记，返回这次操作的错误，和别的没执行的输入一样，不再留下开着的一步。声明操作
    （`do --operation`）的每次尝试都一样，整个操作的效果仍是各次尝试的合计。
  - 三条都满足才这样记：拒绝码是上面两个之一；账本里没有这次输入的意图；返回的终点是本动作自己那条效果为 `not_performed` 的 `command.rejected`。前台检查
    围住实例的那种拒绝（报的是围栏的码）和意图写下以后的任何失败都不算，照旧。
  - **已经发出去、结果不明的输入照旧读作「结果未定」**：`input_returned` 为 true，效果 `indeterminate`，等输入的结果事件来定。
  - **任务运行从来不受影响**：任务的效果不读这条路径；以前记成 `indeterminate` 时，也没有因此围栏过实例。
  - 合同 `contracts/contained-lab-operation.md` 写清了 `input_returned` 的边界：输入到了设备的输入路径、写下意图以后才为 true，在那之前的拒绝留 false、效果
    `not_performed`（登记 §1.281）。这是已有字段的边界，不是新含义，旧读者本来就接受（见下面的配对）。

配置：接口修订都不变：`actingd-config` 仍是 [5, 26]，`ledger` 仍是 [3, 6]，`package` 仍是 [3, 6]，`runtime-client` 仍是 [3, 6]，`install-selection` 仍是 [1, 1]，
`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。`distribution/windows/component-interfaces.json` 和 `contracts/component-interfaces.md` 自 v0.12.41
没改；登记 §1.281 写不升：记录的形状和每个词都没变，新出现的组合（`input_returned` false、`not_performed`、没有意图）是旧校验器本来就收的。

- 没有新键，键表、默认值文件和配置示例都没改，部署计划的默认值对比是空的。
- 结果码：没有新码、新位置、新值键或新词，结果码目录仍是 3890 个码、1740 个位置；没有新定稿或退役的条目，所以本版的发布文件是空的。
- 账本：不升，没有新记录形状。这种 Lab 请求最后那条 `command.rejected` 的效果从 `indeterminate` 变成 `not_performed`，是已有的词。
- 程序文件：workspace 成员、Runtime 包和 Tools 包的文件清单都和 v0.12.41 一样；只改了 `crates/runtime-host`（Lab 操作和它的测试），`apps/actingctl`、
  `apps/actingd`、`apps/actinglab`、`crates/runtime-client` 和合同 crate 自 v0.12.41 都没改，MCP 工具仍是 41 个。

升级时要知道的（按安装工具的代码核过；`apps/actingctl` 自 v0.12.41 没改，下面的行号在 v0.12.41 和本版相同）：

- 不用 `--accept-ledger-one-way`：两边的 `ledger` 都是 [3, 6]，`previous_max_in_next` 和 `one_way` 都过（`apps/actingctl/src/install/plan.rs` 281–294）；
  现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`（`plan.rs` 908），apply 不读账本。
- `runtime-client` 的边：安装工具自己的声明 I 编在程序里（`install/host.rs` 24–25），v0.12.41 的是 [3, 6]，和 Rp [3, 6]、R\* [3, 6] 都有交集，
  `installer_meets_previous`、`installer_meets_next` 都过（`plan.rs` 308–323，交集判断在 234–236）。`install-control` [0, 1] 相同，`runtime-outcomes` 1 在
  它的 [1, 1] 里，现行配置没有它不认识的键（`installer_knows_keys`），本版的 check-config 收现行配置（`next_check_config`）。
- **v0.12.41 的安装工具没有新东西要发或要读。** 它连 Runtime 只有一处 `connect`（`install/runtime.rs` 178），经它发的只有 Status（210）、
  `RuntimeFactSnapshot`（239）、`ScheduleExplanation`（272）、暂停（340）、恢复（371）、关闭（387）和 `StartReport`（417），都不是 Lab 操作，答复的形状也都没变；
  守卫、启动后检查和 `install status` 都只走这几样。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，回退门的配置一项也过（见下面的退回）。

退回 v0.12.41（按安装工具的代码核过）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。`ledger` 两边都是 [3, 6]：`previous_max_in_next`（Rp.max 6 在 R\* [3, 6] 里）和 `one_way` 都过。
  `runtime-client` 两边都是 [3, 6]，安装工具 I [3, 6] 和两边都有交集，`installer_meets_previous`、`installer_meets_next` 都过；`install-control`、
  `runtime-outcomes` 两边相同；`package` 两边都是 [3, 6]，`bundle_max_in_next` 在单一位置根上只有 `pack_declaration_unknown` 警告；`actingd-config` 两边都是
  [5, 26]，配置里没有 v0.12.41 不认得的键（`installer_knows_keys`）。部署的自动回退、`rollback plan|apply`、`install undo` 走的是同一组边和同一个回退门
  （`install/update.rs` 378 起）。
- 回退门其余几项：v0.12.41 自己的读者读部署之后写的账本页（`ledger_readability`，`update.rs` 395–432，只对本版替换掉的那个构建算，就是 v0.12.41；账本没有
  新形状，读得了）；v0.12.41 在这个根上最近一次打开在它的期限减余量之内（433–484）；现行配置过 v0.12.41 的 check-config（485–498，没有新键）。
- 拒绝的只有这些：退到 v0.12.38 及更早（`ledger` 上限 5），`previous_max_in_next` 不过（6 不在 [3, 5] 里），带不带标志都以 `interface_edge` 拒绝；
  退到不是本版替换掉的构建（比如 v0.12.40），`ledger_readability` 不过，以 `rollback_gate_failed` 结束。
- 落盘的东西都不挡。本版写下的 Lab 记录（`input_returned` false、`not_performed`、没有意图）退回后：v0.12.41 的 Runtime 不读回 Lab 记录（只在
  `crates/runtime-host/src/host/lab_operation.rs` 226–227 写）；v0.12.41 的 actinglab 在 `resource restore` 里读它，照下面的配对收下，当成缺口。没有新键、新文件；
  本版对这一步什么都不记，所以也没有录制草稿要清。
- 退回以后：被前台检查拒掉的 Lab 点按又读作 `indeterminate`，`do --capture --record` 又会把它当成结果未定的点按记下、留着那一步。
- 客户端和 Runtime 的配对（行号在 v0.12.40、v0.12.41 和本版相同：合同 crate、`runtime-client`、`apps/actinglab`、`crates/ledger-forensics` 自 v0.12.40 没改）：
  - v0.12.41 的安装工具装本版：照常，见上面的升级。从本版的构建目录跑本版的安装工具，结果相同。
  - **v0.12.41 及更早的 actinglab 在线 `do`** 读本版的答复：收下。`runtime-client` 先验回执（`crates/actingcommand-contract/src/runtime.rs` 6449–6450 调记录的
    `validate`）：`input_returned` 为 false 时只要求没有意图、没有输入事件、效果 `not_performed`（`crates/actingcommand-contract/src/runtime/lab_operation.rs`
    574–577；声明操作的每次尝试 758–761，整个记录和最后一次尝试一致 801–813），记录都满足；「返回了输入却没有结果事件就得是 `indeterminate`」那条（578–581、
    762–764）只管返回了的输入，不适用。再过共享校验器：动作事件只按记录的 `input_action_id` 去查（`crates/runtime-client/src/client/lab_operation.rs` 198–205），
    没有就是空的；`lab_operation_evidence.rs` 631–635（没报意图就不许有意图和动作事件）、702–718（没有输入事件时不许有后帧，只有返回了的输入才查失败阶段）、
    932–936（最后的 `command.rejected` 带记录的效果 `not_performed`）都过。然后 actinglab 答 `executed: false`（`apps/actinglab/src/lab2_cli/operation.rs`
    127–131）；带 `--record` 时效果是 `not_performed`、有失败，什么都不记（186–197），返回这次操作的错误（198–203）。这几条规则记录一侧自 v0.1.0、尝试一侧自
    v0.12.10 就在，0.12 系列的 actinglab 都收。
  - **v0.12.41 及更早的 actinglab 离线 `resource restore`**：读到这条记录照常解码（`apps/actinglab/src/resource_restore.rs` 204–206），过同一个校验器（348），
    恢复草稿时把它记成缺口 `input_not_committed`、`lab_input_failed`（`crates/resource-tooling/src/resource_restore.rs` 356–360、364–369），不生成转移，和它以前的
    `indeterminate` 写法得到的一样；整次恢复不会因为它失败。
  - 控制台 UI v0.12.9（钉 v0.12.40，`Cargo.toml` 39–44 的 `232b3ed8`；`acui-setup` 声明 `actingd-config` [5, 26]、`ledger` [3, 6]、`runtime-client` [3, 5]、
    `runtime-outcomes` [1, 1]）：接口自 v0.12.41 没变，照边算对本版和对 v0.12.41 一样全过。它的源码里没有 Lab 操作的请求；它的取证读 Lab 记录只认 schema 字串
    （`crates/ledger-forensics/src/task_records.rs` 266–283，`verified_legacy`，原样显示文档），不检查成员，所以显示新记录照常。
  - actingctl（含看门狗）不解码 Lab 记录（测试以外的代码里没有 `LabOperationRecord`），它读事件时看到的 `command.rejected` 带 `not_performed`，是已有的词，照常。
  - MCP：`ac_lab_do` 跑安装里的 `<root>\tools\actinglab.exe`，原样转它的数据（`apps/actingctl/src/mcp/lab.rs` 3–4、721），所以答的就是 actinglab 答的：
    `executed: false`、`effect: not_performed`，失败照旧是 `lab_tool_failed`。
  - 钉 v0.12.41 或更早的 actingctl、MCP（在根外跑的那份）和 actinglab 连本版：线上形状都没变，照常。

运行上的变化：

- Lab 被前台检查拒掉的点按（HS-5e）：答复和记录读作未执行，`--record` 不记；判断只比对已经读到的返回终点，不多读账本；任务运行不变。

本版合并的 PR：

- #1526 HS-5e（#402）：前台检查在写下输入意图之前拒掉的 Lab 点按（`application_not_foreground`、`application_foreground_unknown`）记 `input_returned` false、
  效果 `not_performed`，不再是 `indeterminate`，单次点按和多次尝试两条路径都改；已经发出、结果不明的照旧 `indeterminate`；合同写清 `input_returned` 的边界；
  不升接口（旧校验器本来就收这个组合）；改造一个测试、新增一个，没有新码、新键。

## v0.12.41

本版是 v0.12.40 之后合并的 1 个 PR：#407 PERF-READ。先说部署：`runtime-client` 从 [3, 5] 升到 [3, 6]，其余接口声明都不变（账本仍是 [3, 6]），部署不用带
`--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；v0.12.40 的安装工具装本版照常，它不读性能状态（见下面的配对）；装上以后能退回 v0.12.40，
条件见下面的退回。没有新键，默认值文件没改。运行上要知道的是这几处：

- **性能读数现在说得出 Runtime 的内存去了哪里、视觉请求等了多久**（PERF-READ）。`actingctl perf-status` 和 `ac_overview` 的 `performance` 块多两个成员：
  - `memory`，Runtime 进程自己的内存和各块各归谁：
    - `process`：进程自己的私有字节 `private_bytes`（和 `Get-Process` 的私有内存是同一个数）、工作集、峰值工作集，在各块读完之后才采样。
    - `frame_stores`：现在活着的截图帧存储（每个运行、只读观察、Lab 观察和监视探针各持一个）有几个，在内存里一共存了多少字节、多少帧，最大的一个多少字节；
      按各存储自己的记账。
    - `vision_models`：已经载入的视觉模型（`ocr` 或 `nn`），每个带载入前后私有字节的增量 `load_private_bytes`（载入期间别的线程分配的也算进去；采样失败、
      或者引擎不是它自己载的就不写）。没有模型时不显示。
    - `ledger_capture`：账本采集队列内存段的字节数和条数。
    - `packages`：在跑的运行里已经量出主包的个数，和这些包的字节合计（任务计时的 `package_load_bytes`）。
    - `facts`：实例事实库和 Runtime 事实库的条数（是条数，不是字节）。
  - `vision`：每个视觉模型自 Runtime 启动以来的请求数、推理次数，等模型锁的时间和在引擎里推理的时间，各有合计和最长（微秒）。载入两边都不算；等锁超时的
    请求没有推理，所以平均推理时间要按 `inferences` 算。没有模型被请求过时不显示。
  - 全部从内存读，不读也不写账本。
  - `process` 在两种情况下缺省，绝不显示成 0：进程采样失败（警告 `perf_memory_unavailable`，原因是采样器的码）；各块的字节加起来超过私有字节（同一个警告，
    带 `actual_bytes` 各块之和、`limit_bytes` 私有字节；几个模型同时载入、增量互相算进去时可能碰到）。这时各块照常显示，这条警告排在最前面。有 `process`
    时各块之和不超过私有字节是形状规则，违反的答复是 `invalid_performance_status`。
  - 为什么：#407 的性能工作进了 0.12，每一处改动都要用这些读数量改前和改后；以前只有 `Get-Process` 的一个总数，说不出内存在谁手里、视觉慢在等锁还是在推理。
- **读这个状态的程序要和 Runtime 一起换**：`max` 低于 6 的读者读到带新成员的答复会以 `runtime_receipt_decode_failed` 失败并锁住连接，配对检查照样通过；
  本版每次都填 `memory`，所以这样的读者根本读不了 `PerformanceStatus`，其余读法不变。Runtime 没法只对这样的读者省掉新成员：请求 v4、回执 v2 的 schema
  字串不变，请求里不带修订号。同一构建的 actingctl（含 MCP 服务和看门狗）和 actinglab 随 Runtime 一起换；安装工具、看门狗、actinglab 和现有的控制台都不读它
  （见下面的配对）。

配置：接口修订只有 `runtime-client` 一处升：[3, 5] 改为 [3, 6]，`min` 不变（`contracts/component-interfaces.md` 的修订 6，登记 §1.280、§3 W23；
`distribution/windows/component-interfaces.json` 同改）。其余都不变：`actingd-config` 仍是 [5, 26]，`ledger` 仍是 [3, 6]，`package` 仍是 [3, 6]，
`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。

- 没有新键，键表、默认值文件和配置示例都没改，部署计划的默认值对比是空的。
- 结果码：新码 `perf_memory_unavailable`（warning，`since` 0.12.41，值键有 `actual_bytes`、`limit_bytes`）；`performance_memory_status_failed` 和
  `performance_memory_structure_invalid` 的说明补上了进程计数，签名不变。结果码目录从 3889 个码变成 3890 个，位置仍是 1740 个。新码还没定稿（`review` 待定），
  也没有退役的条目，所以本版的发布文件是空的。
- 账本：不升，没有新记录形状；新读数都不进账本。
- 程序文件：workspace 成员、Runtime 包和 Tools 包的文件清单都和 v0.12.40 一样；`apps/actingctl` 只改了 `ac_overview` 的说明（写上 `memory`、`vision`），
  MCP 工具仍是 41 个；`apps/actingd` 给视觉提供者装上私有字节的采样。

升级时要知道的（按安装工具的代码核过；安装命令的代码自 v0.12.40 只改了一个测试文件，下面的行号在 v0.12.40 和本版相同）：

- 不用 `--accept-ledger-one-way`：两边的 `ledger` 都是 [3, 6]，`previous_max_in_next` 和 `one_way` 都过（`apps/actingctl/src/install/plan.rs` 282–294）；
  现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`，apply 不读账本。
- `runtime-client` 的边：安装工具自己的声明 I 编在程序里（`install/host.rs` 25），v0.12.40 的是 [3, 5]，和 Rp [3, 5]、R\* [3, 6] 都有交集，
  `installer_meets_previous`、`installer_meets_next` 都过（`plan.rs` 308–323，交集判断在 234–236）。`install-control` [0, 1] 相同，`runtime-outcomes` 1 在
  它的 [1, 1] 里，现行配置没有它不认识的键（`installer_knows_keys`），本版的 check-config 收现行配置（`next_check_config`）。
- **v0.12.40 的安装工具不发 `PerformanceStatus`。** 它连 Runtime 只有一处 `connect`（`install/runtime.rs` 178），经它发的只有 Status（210）、
  `RuntimeFactSnapshot`（238）、`ScheduleExplanation`（271）、暂停（339）、恢复（370）、关闭（387）和 `StartReport`（417）；守卫、启动后检查和
  `install status` 都只走这几样。存活检查是看门狗的 `live_owner`（`watchdog/observe.rs` 573–592），只连接、读 runtime-info。它跑的子程序是 actingd 的
  `check-config`、`ledger-maintenance verify` 和 actingledger 的读页（`install/programs.rs` 186、293、332、426），都不碰这个读。所以启动后检查照常，不会因为
  新成员解码失败。
- 整个仓里发这个请求的只有 `runtime-client` 的 `performance_status()`（`crates/runtime-client/src/client.rs` 1320–1325），它唯一的调用者是
  `apps/actingctl/src/overview.rs` 650–659 的 `performance_block`，用在 `perf-status`（`apps/actingctl/src/main.rs` 688）和概览的 `performance` 块
  （`overview.rs` 434–452；`actingctl overview` 经 `main.rs` 252，MCP `ac_overview` 经 `mcp/observer.rs` 103）。actinglab、MCP 入口和看门狗都不调它。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，回退门的配置一项也过（见下面的退回）。

退回 v0.12.40（按安装工具的代码核过）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。`ledger` 两边都是 [3, 6]：`previous_max_in_next`（Rp.max 6 在 R\* [3, 6] 里）和 `one_way` 都过。
  `runtime-client`：本版的安装工具 I 是 [3, 6]，和 Rp [3, 6]、R\* [3, 5] 都有交集（用 v0.12.40 的安装工具 I [3, 5] 也都过），`installer_meets_previous`、
  `installer_meets_next` 都过；`install-control`、`runtime-outcomes` 两边相同；`package` 两边都是 [3, 6]，`bundle_max_in_next` 在单一位置根上只有
  `pack_declaration_unknown` 警告；`actingd-config` 两边都是 [5, 26]，配置里没有 v0.12.40 不认得的键（`installer_knows_keys`）。部署的自动回退、
  `rollback plan|apply`、`install undo` 走的是同一组边和同一个回退门（`install/update.rs` 378 起）。
- 回退门其余几项：v0.12.40 自己的读者读部署之后写的账本页（`ledger_readability`，只对本版替换掉的那个构建算，就是 v0.12.40；账本没有新形状，读得了）；
  v0.12.40 在这个根上最近一次打开在它的期限减余量之内；现行配置过 v0.12.40 的 check-config（没有新键）。
- 拒绝的只有这些：退到 v0.12.38 及更早（`ledger` 上限 5），`previous_max_in_next` 不过（6 不在 [3, 5] 里），带不带标志都以 `interface_edge` 拒绝；
  退到不是本版替换掉的构建（比如 v0.12.39），`ledger_readability` 不过，以 `rollback_gate_failed` 结束。
- 落盘的东西都不挡：新读数只在内存里，没有账本记录、键或文件。
- 退回以后：`perf-status` 和 `ac_overview` 的 `performance` 块没有 `memory`、`vision`；本版的读者读 v0.12.40 的答复照常（两个成员都是可选的）。
- 客户端和 Runtime 的配对（`runtime-client` 本版 [3, 6]，v0.12.40 [3, 5]）：
  - v0.12.40 的安装工具装本版：照常，见上面的升级。从本版的构建目录跑本版的安装工具，结果相同。
  - 看门狗（`actingctl watchdog`，计划任务跑 Tools 包里的 `actingwatch`）、MCP 服务（`actingctl mcp-serve`）和 actinglab 是同一个构建，安装工具把 Runtime
    和 Tools 两半一起换，它们随 Runtime 一起更新。看门狗和 actinglab 不读这个状态；MCP 的 `ac_overview` 读。稳定入口在安装核验完整以后、两次调用之间换到新的
    actingctl 子进程；换之前那一小段，旧子进程的 `ac_overview` 读 `performance` 会以 `runtime_receipt_decode_failed` 失败：答复带这条警告和 `incomplete`、
    没有 `performance` 块，连接已经锁住，同一次里排在它后面的 `fact_refresh` 也读不到；MCP 服务丢掉锁住的连接，下一次调用重连（`mcp/runtime.rs` 275–287），
    其余块照常。
  - 控制台（按 `contracts/install-cli.md` 的 UI 边）：UI v0.12.7、v0.12.8 钉的都是 v0.12.39，它们的 `acui-setup` 声明 `actingd-config` [5, 26]、`ledger` [3, 6]、
    `runtime-client` [3, 5]、`runtime-outcomes` [1, 1]。照边算对本版全过：`installer_ui_next_common`（26 三方共有）、`ledger` 的 `previous_max_in_ui` 和
    `next_max_in_ui`（6 在 [3, 6] 里）、`ui_meets_next`（[3, 5] 和 [3, 6] 有交集）、`runtime-outcomes` 的 `next_max_in_ui`。这两个版本的源码里没有
    `PerformanceStatus`，也不调 `performance_status()`（按两个 tag 的源码查过），所以连本版照常：请求、回执、Status、设置列表和账本页的形状都没变。生产根
    是单一位置根，里面没装控制台；`actingctl install` 的计划不算控制台的边（`ui_edges` 现在只有闸门测试在算）。
  - 钉 v0.12.40 或更早的 actingctl、MCP（在根外跑的那份）连本版：`perf-status` 和 `ac_overview` 的 `performance` 以 `runtime_receipt_decode_failed` 失败，
    其余照常；钉 v0.12.40 的 actinglab 照常。

运行上的变化：

- 性能读数（PERF-READ）：每次状态读多拿几把短锁、采一次进程；每个帧存储的记账变化和每个视觉请求多拿一把短锁；每次载入模型采两次私有字节。

本版合并的 PR：

- #1524 PERF-READ（#407）：`PerformanceStatus` 多 `memory`（进程自己的私有字节、工作集和峰值；活着的截图帧存储；已载的视觉模型和载入增量；账本采集队列；
  在跑运行的包；两个事实库的条数）和 `vision`（每个模型的请求、推理、等锁和推理时间，合计和最长）；警告 `perf_memory_unavailable`；`runtime-client`
  [3, 6]；全从内存读，不碰账本；新测试四个，没有新键。

## v0.12.40

本版是 v0.12.39 之后合并的 4 个 PR、5 片：HS-L、HS-5c、HS-5a 和 HS-5b（一个 PR）、HS-5d。先说部署：接口声明都不变（账本仍是 [3, 6]），部署不用带
`--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；装上以后能退回 v0.12.39，条件见下面的退回。没有新键，默认值文件没改。
**本版自己不改任何实例的后端。** 运行上要知道的是这几处：

- **应用没在跑时，Nemu 触控的实例也能用了**（HS-5a、HS-5b）。以前：11-04 一台实例在线切到 Nemu 触控和截图以后应用被停，客户端跑启动包时，
  配对会话的第一帧就以 `display_missing` 打开失败，失败的打开又放不掉自己连上的 Nemu 连接，这台被围栏、转人工等待。现在应用在不在跑不再是打开的条件，
  和 adb 触控一样：
  - 配对会话找不到应用包的显示时截主显示（配置的 `display_id`，就是只截图的 Nemu 会话截的那块，截到的是桌面），直到应用的显示出现；之后的帧来自包的显示，
    会话不换。显示出现、消失或换号（包括主显示 0 换成包的显示 0）时，之前所有帧的几何作废，不会在另一块显示的帧上决定点按。所以应用没跑时，识别、
    应用入口（启动包拉起应用）、自检和复查都照常。
  - 应用还没到前台时的点按，由已有的前台检查拒绝（`application_not_foreground` 或 `application_foreground_unknown`），什么都不按、不围栏；从没按下的接触点
    复位什么都不调用。
  - 没写 `nemu_app_index` 时，MuMuManager 的 `app info` 读到 `stopped` 也取 0（和 `running` 一样）；取到的编号只有重新绑定才清，打开失败、应用停下或启动都不清。
    `not_installed`、读不到、回答格式不对照旧以 `nemu_app_index_problem` 具名拒绝。
  - 打开在连上之后、交出后端之前失败（分辨率探测、第一帧、内存准入、探测缓存）时，打开者在 `LocalOnly` 下断开自己连上、没按过键的连接，这次打开以资源
    已确认失败，只让打开它的那次工作失败，不围栏；断开本身失败照旧是未确认，照旧围住这一台。只截图的 Nemu 会话也一样：Nemu 截图配 `adb` 触控的实例，
    第一帧失败以前会围住这台，现在不会。
  - 配对会话不再报 `display_missing`（`capture_open_cause`），也不再报 `application_not_running`（`nemu_app_index_problem`）；两个词留在词表里，结果码目录的
    说明写了 0.12.40 起停发。
  - 换显示时到达等待接着等（HS-5d）。以前：包声明了目标共识时，一次观察要取两帧以上；主显示换成应用的显示（几何代号加一）正好落在这几帧之间，
    这次观察就以 `recognition_sample_geometry_changed` 结束、不重试，整次运行失败（启动包、阶梯的应用重启级、运行的应用入口和任何一步的确认都可能碰上）。
    现在页面图的步骤确认和步骤计划在点按或应用效果之后的等待，把这样的观察当成一帧没确认的，在同一窗口里、隔同样的间隔接着观察；窗口用完照旧以等待自己的码
    结束（`page_confirmation_failed`、`contained_task_linear_application_unconfirmed`）。只取一帧的观察本来就不受换显示影响；其他等待（入口等待、可选步骤、
    Lab 的在线观察等）照旧以这个码失败。三个资源仓的包目前都没声明目标共识，生产上还没碰到。
- **切回原后端是一种补救**（HS-5c）：被自己的后端打开失败围住的实例可以切换后端，不用先自检，比如
  `actingctl config clear instance.<instance id>.touch_backend instance.<instance id>.capture_backend` 两个后端键都清回文件值（MCP `ac_config_set`、控制台的
  「恢复为文件值」、Lab 的 `session backend --clear` 也一样）。放行的围栏原因只有三种：`input_backend_open_failed`、`capture_backend_open_failed`、
  `paired_backend_open_failed`，入口和准入锁下各核一次。别的原因（没确认抬起的接触点、adb 子进程或管道、应用后端、会话连死两次、截图操作失败、
  其他包容失败）照旧拒 `backend_switch_refused`（`instance_fenced`）；恢复阶梯拿着这台时认领照旧等不到，拒 `instance_busy`。围栏怎么结束：新后端的准备
  自检通过就解除（记 `instance_resources_fence_lifted`，链接这次切换的请求），人工等待随之结束，然后才提交；新后端没过、恢复的原后端自检过了，围栏也解
  （`open_failed`）；两边都没过（`restore_failed`），围栏、人工等待、已做的复查次数和下次复查时间原样不动。
- **Lab 也能设后端**（HS-L）：`actinglab --instance <alias> session backend [--touch <name>] [--capture <name>] [--clear]`。先读 Status 取这台的实例 id（别名没注册
  是 `instance_not_found`），再在 `lab` 通道上发一个 `ConfigureSettings`（经 MCP 或控制台转发时带 `via`），两个都给就放在同一个请求里；`--clear` 把两个键都
  退回文件值，这本身也是一次切换，不能和另外两个一起用。等回执的时长是 `runtime-client` 对后端键算的那个（133 秒加 IO 余量），Lab 自己不算。
  - 成功打印 `{instance, instance_id, settings, config_version}`。被拒时错误码是回执的顶层码：`config_value_invalid`、`config_key_unknown`、
    `config_key_not_settable` 退出 2，其余（如 `backend_switch_refused`）退出 4；`details` 带 `config_key`、`config_value_problem` 或 `backend_switch_problem`、
    `receipt_state` 和整个结果（含原因链）。没有回执的失败照 Lab 原来的映射（`runtime_not_running`、`device_error`）。
  - 写法不对（没有选项、`--clear` 配别的、重复、不认识的选项或多余的词）在连接之前以 `validation_failed` 拒；`--dry-run` 也在连接之前拒（`dry_run_unsupported`）。
  - 没有只在 Lab 里生效的后端，每台实例只有一个状态。`session instance list` 和 `session instance registry` 多一列 `runtime_backends`（从 Runtime 读的生效后端
    和来源，不读 Lab 文件），列表旁边是 `runtime_backends_read`。Lab 命令从 141 条变成 142 条。
- **生产实例的触控仍是 `adb_shell_input`**：本版不改任何实例的后端。总览在真实例上核过冷路径（应用停着时打开、启动包拉起应用）以后，再逐台切换。

配置：接口修订都不变：`actingd-config` 仍是 [5, 26]，`ledger` 仍是 [3, 6]，`package` 仍是 [3, 6]，`runtime-client` 仍是 [3, 5]，`install-selection` 仍是 [1, 1]，
`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。`component-interfaces.json` 和 `contracts/component-interfaces.md` 自 v0.12.39 没改，五片的登记
（§1.275 到 §1.279）都写不升：没有新请求、成员、记录、键、码或词。

- 没有新键，键表、默认值文件和配置示例都没改，部署计划的默认值对比是空的。
- 结果码：没有新码、新位置、新值键或新词，结果码目录仍是 3889 个码、1740 个位置。停发的两个词（`capture_open_cause` 的 `display_missing`、
  `nemu_app_index_problem` 的 `application_not_running`）不退役：词表的词没有退役这一说，目录只在它们的说明里写了 0.12.40 起不再报；`paired_backend_open_failed`
  和 `nemu_app_index_missing` 的说明也照改，签名不变。没有新定稿或退役的条目，所以本版的发布文件是空的。
- 账本：不升，没有新记录形状。HS-5c 的解除用的是已有的 `instance_resources_fence_lifted`，只是链接的是切换的请求；HS-L 的读取是 Status 和
  `RuntimeFactSnapshot`，什么都不记。
- 程序文件：workspace 成员、Runtime 包和 Tools 包的文件清单都和 v0.12.39 一样；`apps/actingctl`、`apps/actingd` 自 v0.12.39 没改，MCP 工具仍是 41 个。

升级时要知道的：

- 不用 `--accept-ledger-one-way`（按安装工具的代码核过）：两边的 `ledger` 都是 [3, 6]，`previous_max_in_next`（6 在 [3, 6] 里）和 `one_way`
  （R\*.max 6 不大于 Rp.max 6）都过；带上也不改判断。现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`，apply 不读账本。
- 安装命令的代码自 v0.12.39 没有改，v0.12.39 的安装工具判的边和本版一样。本版的 check-config 读现行配置照常通过（没有新键、新成员）；包的边在单一位置根上
  只有 `pack_declaration_unknown` 警告。
- 部署的自动回退和 `install undo` 这次可以用：接口边都过，回退门的配置一项也过（见下面的退回）。

退回 v0.12.39（按安装工具的代码核过）：

- 安装工具允许退回，不用 `--accept-ledger-one-way`。`ledger` 两边都是 [3, 6]：`previous_max_in_next`（Rp.max 6 在 R\* [3, 6] 里）和 `one_way` 都过。
  `runtime-client` 两边都是 [3, 5]，`installer_meets_previous`、`installer_meets_next` 都过；`install-control`、`runtime-outcomes` 两边相同；`package` 两边都是
  [3, 6]，`bundle_max_in_next` 在单一位置根上只有 `pack_declaration_unknown` 警告；`actingd-config` 两边都是 [5, 26]，配置里没有 v0.12.39 不认得的键
  （`installer_knows_keys`）。部署的自动回退、`rollback plan|apply`、`install undo` 走的是同一组边和同一个回退门。
- 回退门其余几项：v0.12.39 自己的读者读部署之后写的账本页（`ledger_readability`，只对本版替换掉的那个构建算，就是 v0.12.39；账本没有新形状，读得了）；
  v0.12.39 在这个根上最近一次打开在它的期限减余量之内；现行配置过 v0.12.39 的 check-config（没有新键、新成员）。
- 拒绝的只有这些：退到 v0.12.39 之前（v0.12.38 及更早，`ledger` 上限 5），`previous_max_in_next` 不过（6 不在 [3, 5] 里），带不带标志都以 `interface_edge` 拒绝，
  和 v0.12.39 一节一样；退到不是本版替换掉的构建，`ledger_readability` 不过，以 `rollback_gate_failed` 结束。
- 落盘的东西都不挡：账本没有新形状；`host.config_overrides` 里在线设的后端键是 v0.12.39 认得的键，启动时照常恢复；围栏和取到的 Nemu 编号只在内存里；
  Lab 文件没有新成员。
- 退回以后：应用没跑时，Nemu 触控的实例又会像 11-04 那样以 `display_missing` 打开失败并被围住，围着时切换又被 `instance_fenced` 拒，`stopped` 又以
  `application_not_running` 拒；Nemu 截图的打开失败又会围住那台；声明了目标共识的包，一次观察的几帧跨过换显示又以 `recognition_sample_geometry_changed`
  让运行失败；Lab 没有 `session backend`，实例列表没有 `runtime_backends`。所以已经有实例切到 Nemu
  触控时，退回之前先在本版上把它切回（`config clear` 两个后端键）。
- 客户端和 Runtime 的配对（`runtime-client` 本版和 v0.12.39 都是 [3, 5]；`apps/actingctl` 自 v0.12.39 没改）：
  - v0.12.39 的安装工具装本版：照常，不带 `--accept-ledger-one-way`。它的安装代码和本版一样：`ledger` 两条边都过、没有 `ledger_verify`；它的声明 I 是
    `runtime-client` [3, 5]，和 Rp [3, 5]、R\* [3, 5] 都有交集（`installer_meets_previous`、`installer_meets_next`），`install-control` [0, 1] 相同，
    `runtime-outcomes` 1 在它的 [1, 1] 里，现行配置没有它不认识的键（`installer_knows_keys`），本版的 check-config 收现行配置（`next_check_config`）。
    从本版的构建目录跑本版的安装工具，结果相同。
  - 控制台（按 `contracts/install-cli.md` 的 UI 边）：生产根是单一位置根，里面没装控制台；`actingctl install` 的更新和回退计划不算控制台的边（`ui_edges`
    现在只有闸门测试在算，R-15d 以后才进计划）。照边算：钉 v0.12.39 的 UI v0.12.7、v0.12.8（`actingd-config` [5, 26]、`ledger` [3, 6]、`runtime-client` [3, 5]、
    `runtime-outcomes` [1, 1]）对本版全过：`installer_ui_next_common`（26 三方共有）、`ledger` 的 `previous_max_in_ui` 和 `next_max_in_ui`（6 在 [3, 6] 里）、
    `ui_meets_next`、`runtime-outcomes` 的 `next_max_in_ui`。从根外跑的这两个控制台连本版：请求、回执、Status、设置列表和账本页的形状都没变，照常。被自己的
    后端打开失败围住的实例，设置页的「恢复为文件值」现在直接能切回；控制台要不要改掉「先自检」的提示，由 UI 频道定（#429）。
  - 钉 v0.12.39 的 actingctl、MCP 和 actinglab 连本版：线上形状都没变，照常；钉 v0.12.39 的 actinglab 没有 `session backend`。

运行上的变化：

- Lab 设后端（HS-L）：`actinglab session backend` 走 CLI、MCP、控制台同一个 `ConfigureSettings`；实例列表多 `runtime_backends` 一列；Lab 命令 142 条。
- 围栏下切回（HS-5c）：自己的后端打开失败造成的围栏放行切换，新后端或恢复的原后端自检通过就解围栏、结束人工等待；其余围栏照旧 `instance_fenced`。
- 配对会话截主显示（HS-5a）：应用没跑时截主显示、不报 `display_missing`，显示换了作废旧帧几何；打开失败放掉自己连上的连接，不围栏（只截图的 Nemu 会话也一样）。
- 编号读取（HS-5b）：`stopped` 取 0，不再报 `application_not_running`；取到的编号只在重新绑定时清。
- 换显示时接着等（HS-5d）：包声明目标共识时，一次观察的几帧跨过换显示不再让运行失败，当成一帧没确认的、在同一窗口里接着等；目前没有包声明目标共识。

本版合并的 PR：

- #1516 HS-L（#402）：`actinglab --instance <alias> session backend [--touch] [--capture] [--clear]`，在 `lab` 通道上发和 CLI 同一个 `ConfigureSettings`，等待时长取 `runtime-client` 的；拒绝以回执的码退出 2 或 4；`session instance list`、`registry` 多一列 `runtime_backends`；Lab 命令 142 条。
- #1518 HS-5c（#402）：切换的控制门读围栏原因，`input_backend_open_failed`、`capture_backend_open_failed`、`paired_backend_open_failed` 三种放行，候选自检通过就由现有的准备解围栏、结束人工等待；候选和恢复都没过时围栏原样；其余原因照旧 `instance_fenced`。
- #1519 HS-5a、HS-5b（#402）：配对会话没有包的显示就截主显示，显示换了作废帧几何；打开失败时打开者放掉自己连上、没按过的连接；`app info` 的 `stopped` 取 0，编号只在重新绑定时清；`display_missing`（配对会话）和 `application_not_running` 停发；HS-3 的两个测试按要求改换并记进退役门表。
- #1522 HS-5d（#402）：页面图的步骤确认和步骤计划在点按或应用效果之后的等待，遇到 `recognition_sample_geometry_changed` 当成一帧没确认的、在同一窗口里接着等；`gate_hs5` 的阶梯应用重启级和启动包两个测试改用手动时钟（#1521 的 CI 红在前一个，是测试自己的竞态）；没有新码、新键、新成员。

## v0.12.39

本版是 v0.12.38 之后合并的 8 个 PR：HS-4、HS-1、RH-1、HS-2、BX-1、HS-3、R-3、HS-2b。先说部署：账本的声明从 [3, 5] 改为 [3, 6]（HS-2b），
这一步是单向的；没有新键，默认值文件没改。**本版自己不改任何实例的后端**：只有操作员设了后端键才会切换。运行上要知道的是这几处：

- **账本升到修订 6，只能往前修**（HS-2b）。修订 6 是修订 5 加上运行中切换后端的记录 `device.backend_switched`。部署（`actingctl install update plan`、
  `update apply`）要带 `--accept-ledger-one-way`，计划多一步账本校验 `ledger_verify`；装上本版以后，安装工具拒绝退回 v0.12.38，部署自己的自动回退和
  `install undo` 也一样被拒，**不论有没有切换过后端**（见下面的升级和退回）。出了问题只能往前修。读这个 Runtime 账本的控制台要建在修订 6 的读者上
  （控制台的下一个构建，钉 v0.12.39）；更早的控制台（UI v0.12.6、v0.12.5，`ledger` [3, 5]）账本窗口读到含切换记录的那一页会失败，指示灯、卡片、
  设置页和实时画面不受影响（UI 频道 11-04 的判读，见配对）。
- **实例的触控和截图后端现在可以在运行中换**（HS-1、HS-2）。键是 `instance.<instance id>.touch_backend`（`adb_shell_input`、`maatouch`、`minitouch`、
  `nemu_ipc`）和 `instance.<instance id>.capture_backend`（`adb`、`droidcast_raw`、`nemu_ipc`、`scrcpy`），就是配置条目里原有的两个成员。用
  `actingctl config set instance.<id>.touch_backend=… instance.<id>.capture_backend=…` 和 `config clear`、MCP `ac_config_set`（带后端键要 operator 档，
  否则 `tier_not_enabled`，`required_tier` 为 `operator`）；控制台的设置页等它提供这两行以后走的也是这一条路。一次请求只能改一台的后端键（夹带别的键或
  别台的后端键：`config_value_invalid`，`backend_switch_not_alone`）；设备种类没有这个后端（模拟器不能 `scrcpy`，真机不能 `nemu_ipc`、`droidcast_raw`，
  夹具一个都没有）是 `backend_kind_unsupported`，Nemu 触控不配 Nemu 截图是 `backend_pair_required`。
  - 切换要等这台空下来，最多等 3 秒（`CONNECTION_PREPARATION_CLAIM_WAIT_MS`，常量）：跑着的任务（停在两步之间也算）、恢复阶梯、客户端租约、Lab 操作、
    自检或模拟器控制 3 秒后还占着它，就拒绝，不往后推。暂停算边界：要在任务中途切，先暂停这台、等它排空、切、再恢复。
  - 拒绝是 `backend_switch_refused`，带 `backend_switch_problem`：`instance_busy`（3 秒内没空下来，原因 `lease_queue_expired` 带 `holder_kind`）、
    `player_held`（玩家持有；R-7b 之前不会出现）、`instance_fenced`（#403 围栏围着，自检通过才解）、`endpoint_unavailable`（模拟器没开，或真机没绑定）、
    `open_failed`（新后端没打开或自检没过，原来的已经恢复，原因是失败那一步的码）、`restore_failed`（新的没打开，原来的也没恢复）。
  - 切换失败时关掉新后端、换回原来的后端再准备一次，所以失败的切换留下的是原来的后端；原来的也打不开时，只有这一台不可用（它的自检失败），别的实例不受影响。
  - 只有切换成功才记 `host.config_overrides`、`config.parameters.instance` 和新记录 `device.backend_switched`；被拒的切换什么都不提交。带后端键的请求，客户端
    等回执最多 `BACKEND_SWITCH_WAIT_MS`（133 秒，HS-2b）加 IO 余量。这是切换最坏的情况，各步上界照顺序相加：认领 3 秒、关原来的会话 30 秒、准备新后端 30 秒、
    提交写账本 10 秒、关新后端 30 秒、恢复原后端的准备 30 秒。所以要用本版的 actingctl 或 MCP 来切。
  - 等完还没拿到回执（`runtime_receipt_timeout`），结果是「未确认」，不是「没生效」：切换可能还在跑，也可能在客户端不等以后提交了（`config_version` 只在提交时变）。
    再发之前先重读设置的 `config_version` 和这台实例 Status 里的 `activity`（切换在跑时是带这次 `request_id` 的 `selfcheck`）：版本变了、生效的是要的后端，
    就是切好了；活动还是这次请求，就是还在切，过一会儿再读。`ledger_observer.writer_reply_timeout_ms` 设得比默认 10 秒大，或者会话关得比额度慢时，切换可能
    比这个等待还久，规矩一样。
  - 重启以后在线设的后端照样恢复，在准备实例之前写进注册表；恢复的后端打不开，这台就不可用，不会悄悄退回文件里的值，要人 `config clear`（这本身也是一次切换）。
  - **切之前先升级控制台**：账本里一旦有 `device.backend_switched`，建在修订 5 读者上的控制台（UI v0.12.6、v0.12.5）账本窗口读到这一页会失败，钉 v0.12.38
    或更早契约的 actingctl、MCP 和 actinglab 读带载荷的事件页也解不开（见配对）。所以先把控制台换成钉 v0.12.39 的构建，再切。
- **Nemu 触控补上复位、长按和普通拖动**（HS-4）：复位只在接触点没确认抬起时抬一次，不要帧（抬起没得到确认就以 Unconfirmed 失败，照旧由 #403 围住这一台）；
  长按是按下、按住、抬起；拖动沿直线每 16 ms 移一次（最多 60 次）再抬起。长按和拖动带这一步的帧，受几何围栏和时限约束，和点击一样。取消直接运行之后欠的
  那次复位、`actingctl reset`、Lab 的 `reset`，以及包里的长按和普通拖动，在 Nemu 触控的实例上不再被拒，包不用改；`key`、`text` 仍是
  `nemu_input_capability_unsupported`。生产实例的触控都是 `adb_shell_input`，所以在有人把某台切到 Nemu 触控之前什么都不变。
- **`nemu_app_index`**（HS-1、HS-3）：触控和截图都写 `nemu_ipc` 的配置条目不用再写它（v0.12.38 以 `nemu_paired_input_configuration_missing` 拒收这种文件）。
  没写时，Runtime 在打开配对会话之前读一次 `MuMuManager control -v <MuMu 序号> app info -pkg <application_id>`（最多 10 秒，算在这次准备的时间里）：
  `running` 取 0（主应用）；包没装、应用没在跑、读不到、回答不是文档里的格式，都拒绝打开，`paired_backend_open_failed`，原因 `nemu_app_index_missing` 带
  `nemu_app_index_problem`（`package_not_installed`、`application_not_running`、`info_unavailable`、`info_malformed`），只有这一台打不开。取到的编号只在内存里，
  重新绑定或打开以 `display_missing` 失败时清掉；真机从不读 MuMuManager。文件写了编号就用文件的，什么都不读。模拟器条目现在不论后端是什么都可以写编号
  （写了就查范围，只有 Nemu 触控用它）；Nemu 触控配别的截图，改以 `nemu_input_requires_paired_capture` 拒收。这条 MuMuManager 读取还没在真实例上用过，
  第一次用先在实验实例上核。
- **`start_emulator` 在运行中可改**（R-3）：`actingctl config set instance.<id>.start_emulator=true|false`、`config clear`、MCP `ac_config_set`（observer 档）
  或控制台。只有经发现绑定的实例能设 `true`，夹具、真机、按端口绑定的实例以 `config_value_invalid`（`discovery_binding_required`）拒绝。改这个值本身不启动、
  也不停止任何模拟器：守护启动时的自动拉起、模拟器看守的重启、IDLE-1 `stop_emulator` 之后的唤醒，都在下一次决定时读生效值，各自的认领在 Start 之前再读
  一次，值是关的就不启动（`emulator_autostart_skipped`，`skip_reason` 为 `start_emulator_off`）。实例暂停期间照旧不自动拉起它的模拟器（裁定 117 A），动手前
  再核一次暂停。`idle.action` 为 `stop_emulator` 的实例仍要求 `start_emulator` 为 `true`，所以在这种实例上关掉它要同一条请求里改 `idle.action`。
- **请求自带的入口恢复包**（RH-1）和配置的回主页包按同一条规矩判：只声明了 `scheduling_outcome`、没有指定操作（也没有资源读数和调整、`on_empty`、阶段
  `end_pages`、带键的 `on_max_runs` / `on_guard_refused`）的包，用 `--recovery-package` 点名时不再以 `contained_task_home_recovery_package_incompatible` 拒绝。
  它的 `scheduling_outcome` 不起作用，运行的调度处置只取运行自己的包；其余条件和拒绝码不变。

配置：`actingd-config` 从 [5, 25] 改为 [5, 26]（HS-1：模拟器条目不论后端都可以写 `nemu_app_index`，Nemu 触控不再要求它；HS-2、HS-3、R-3 都不再升，
两个后端成员和 `start_emulator` 本来就是条目的成员，指令行也不是文件形状）；`ledger` 从 [3, 5] 改为 [3, 6]（HS-2b：HS-2 的记录 `device.backend_switched`）；
`package` 仍是 [3, 6]，`runtime-client` 仍是 [3, 5]（请求和回执的形状没变，HS-2b 只改了客户端等回执的时长），`install-selection` 仍是 [1, 1]，
`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]（新码、新词都是修订 1 里的增量）。`component-interfaces.json`、`contracts/component-interfaces.md`
的 `actingd-config` 26 一行和 `ledger` 6 一行、架构测试三处一致。

- 没有新键，默认值文件和配置示例都没改，部署计划的默认值对比是空的。修订 26 读得了修订 5 到 25 的配置文件，没有例外。
- 键表里变了的行：`instance.<instance id>.touch_backend`、`instance.<instance id>.capture_backend` 两行是新的（`since` 0.12.39，没有默认值，可在线改，
  控制台档位 `advanced`；`advanced` 从 6 个变成 8 个，满了）；`instance.<instance id>.start_emulator` 从不可设改成可设（读者 R-3，`since` 仍是 0.12.0，
  档位仍是 `daily`）。每台写了明确后端的设备实例，`config.parameters.instance` 多两行，按设备层的名字报（文件写触控 `adb` 报 `adb_shell_input`）；
  所以本版第一次启动时设置的版本号会变一次，控制台之前拿到的版本号被拒（`fact_observation_version_conflict`），重读设置就好。
- 结果码：新码 1 个（`backend_switch_refused`），新位置 2 个（`switch_instance_backends`、`apply_restored_backends`），新值键 2 个（`backend_switch_problem`、
  `nemu_app_index_problem`），新词表 2 个（`backend_switch_problem` 6 个词、`nemu_app_index_problem` 4 个词）。已有词表加词：`config_value_problem` 4 个
  （`backend_kind_unsupported`、`backend_pair_required`、`backend_switch_not_alone`、`discovery_binding_required`），`claim_kind` 的 `backend_switch`，
  `emulator_autostart_skip_reason` 的 `start_emulator_off`，`event_action` 的 `device.backend_switched`；`nemu_app_index_missing` 多可选值
  `nemu_app_index_problem`。退役 2 个，都是 `retired_in` 0.12.39：`nemu_paired_input_configuration_missing`（`replaced_by` `nemu_input_requires_paired_capture`）
  和 `nemu_app_index_requires_paired_input`。结果码目录从 3888 个码、1738 个位置变成 3889 个、1740 个；本版的发布文件列的就是这两个退役的码。
- 账本：`ledger` 6 是 5 加上 `runtime.lifecycle_observed` 的封闭动作 `device.backend_switched` 和它的载荷（`instance_alias`、触控和截图各一个
  `{from, to}`、可选的 `nemu_app_index` 和封闭词表 `nemu_app_index_source`（`file`、`resolved`）、`session_generation` 和审计，拒收不认识的成员），只在切换
  成功时写。别的片加的都不进读者核的封闭词表：认领种类 `backend_switch` 只在内存里，进账本只是结果值 `holder_kind`；`start_emulator_off`、两个新的问题词表和
  增删的码，读者只核拼写。SQLite 格式 2、`actingcommand.event.v3`、记录类型、表、索引和每个视图的类型表都不变，没有迁移，也不用新的状态根，修订 3 到 5 的
  账本照常读。声明 [3, 5] 的 Runtime（v0.12.38 及更早）解不开修订 6 的记录（`corrupt_ledger_record`），所以不论写没写过，都拒绝退回到它（见退回）。
  HS-2 当初把这条记录当成像 `device.self_check` 那样的增量，不升修订；`device.self_check` 随第一批打 tag 的 Runtime 发出，早于接口表，属于修订 1，不是先例。
- 程序文件：workspace 成员、Runtime 包和 Tools 包的文件清单都和 v0.12.38 一样。BX-1 只是 `actingcommand-runtime-config` 导出的免打扰条目解析和序列化
  函数（`BlackoutListKey`、`BlackoutEntry::parse`、`parse_list`、`to_json`、`list_json`），给控制台用，拒绝和配置检查的一样；Runtime 的行为不变。

升级时要知道的：

- 账本单向（HS-2b，按安装工具的代码和 HS-2b 的安装测试核过）：`update plan`、`update apply` 不带 `--accept-ledger-one-way` 时，`ledger` 的 `one_way` 边
  不过（R\*.max 6 大于 Rp.max 5），以 `interface_edge` 拒绝，不过的只有这一行，什么都不改；带上以后 `previous_max_in_next`（5 在 [3, 6] 里）和 `one_way`
  都过，计划的 `notice.rollback_window_bounded` 是 true，操作的 `acknowledged` 记下 `accept_ledger_one_way`。
- 账本校验：现行和目标的 `ledger` 范围不同，所以 apply 多一步 `ledger_verify`：`--stop` 停下 Runtime 之后、换任何文件之前，用本版的
  `actingcommand-actingd.exe ledger-maintenance verify --config <root>\actingd.config.json` 做启动时那一套有界检查（`verified-sqlite-bounded`，最新
  `ledger_observer.open_verify_tail_records` 条逐条认证），期限是 `install.child.ledger_verify_timeout_ms`（默认 1800000）。现行账本只有修订 3 到 5 的记录，本版
  读得了。不过或到期限时怎么办，和 v0.12.36 一节的账本校验一样：什么文件都没换，但 Runtime 已经停了、hold 还在。
- 没有自动回退：部署的新 Runtime 起来以后检查不过时，部署自己的自动回退先按回退算一次计划，计划的 `ledger` 边不过（见下面的退回），以 `rollback_gate_failed`
  （`rollback_gate_parts` 是 `interface_edges`）结束，新构建和 hold 都留着。起过本版 Runtime 的操作，`install undo` 也按回退判，同一条边拒绝；没起过 Runtime 的
  未完成操作照旧能撤销。出了问题只能往前修。
- 安装命令的代码自 v0.12.38 没有改（只加了 HS-2b 的安装测试），v0.12.38 的安装工具判的边和本版一样。本版的 check-config 读现行配置照常通过（现行配置没有
  Nemu 触控，Nemu 截图配 `adb` 触控照旧允许，也没写 `nemu_app_index`）；包的边在单一位置根上只有 `pack_declaration_unknown` 警告；没有新键，部署计划的默认值
  对比是空的。
- 第一次真切后端之前：先把控制台换成钉 v0.12.39 的构建，用本版的 actingctl 或 MCP 来切。Nemu 触控配对第一次不写编号，先在实验实例上核 MuMuManager 的那次读取。

退回 v0.12.38（按安装工具的代码和 HS-2b 的安装测试核过）：

- 安装工具拒绝退回。本版声明 `ledger` [3, 6]，v0.12.38 声明 [3, 5]。`rollback plan`、`rollback apply` 的 `ledger` 边 `previous_max_in_next` 要求现行构建的
  Rp.max 落在目标的 R\* 里：6 不在 [3, 5] 里，这条边不过，两条命令都在算回退门之前就以 `interface_edge` 拒绝，不过的只有这一行，什么都不改。带不带
  `--accept-ledger-one-way` 都一样：它只放行升级方向的 `one_way` 边（HS-2b 的安装测试两种都核了）。这只看两边的声明，和账本里有没有 `device.backend_switched`
  无关；v0.12.38 及更早的版本声明的 `ledger` 上限都不到 6，一样被拒。
- 同一条边挡住部署的自动回退和起过本版 Runtime 以后的 `install undo`（见上面的升级）；v0.12.38 的 actingctl 跑的是同一份安装代码，判的也一样。
- 为什么要这样（只作记录，不是操作步骤）：v0.12.38 的载荷里没有 `device.backend_switched`，切换成功过一次以后，这条记录还在最新的
  `ledger_observer.open_verify_tail_records` 条（默认 4096）里时 v0.12.38 打不开账本，读到含它的页也整页失败，回退门的 `ledger_readability` 也不过。发版前核过的
  另外两处也一并记下：在线设过的后端键和 `start_emulator` 的行，v0.12.38 启动时逐行丢掉（`config_override_dropped`），照常启动；触控、截图都是 `nemu_ipc`
  却没写 `nemu_app_index` 的配置文件，v0.12.38 以 `nemu_paired_input_configuration_missing` 拒收。手工放回旧程序不在保证内（测试阶段不做旧版兼容）。
- v0.12.38 一节的退回条件是退回 v0.12.37 的条件；本版连 v0.12.38 都退不回去，它们不适用。
- 客户端和 Runtime 的配对（`runtime-client` 本版、v0.12.38 和 v0.12.37 都是 [3, 5]；`apps/actingctl` 自 v0.12.38 只改了 `ac_config_set` 带后端键要 operator 档
  和 HS-2b 的安装测试，MCP 工具仍是 41 个）：
  - v0.12.38 的安装工具装本版：照常，**要带 `--accept-ledger-one-way`**。它的安装代码和本版一样：不带标志 `one_way` 不过、带上就过，`previous_max_in_next`
    （5 在 [3, 6] 里）过，计划有 `ledger_verify`，跑的是本版构建里的 `ledger-maintenance verify`。它的声明 I 是 `runtime-client` [3, 5]，和 Rp [3, 5]、
    R\* [3, 5] 都有交集（`installer_meets_previous`、`installer_meets_next`），`install-control` [0, 1] 相同，`runtime-outcomes` 1 在它的 [1, 1] 里，现行配置
    没有它不认识的键（`installer_knows_keys`），本版的 check-config 收现行配置（`next_check_config`）。从本版的构建目录跑本版的安装工具，结果相同。
  - 控制台（按 `contracts/component-interfaces.md` 的边）：生产根是单一位置根，里面没装控制台；`actingctl install` 的更新和回退计划不算控制台的边（`ui_edges`
    现在只有闸门测试在算，根里有 UI 成员、R-15d 以后才进计划），所以 v0.12.38 和本版的安装工具都不核它。照边算：UI v0.12.6（`actingd-config` [5, 25]、
    `ledger` [3, 5]、`runtime-client` [3, 5]、`runtime-outcomes` [1, 1]）和 UI v0.12.5（`actingd-config` [5, 24]，其余相同）对本版只有 `ledger` 的
    `next_max_in_ui` 不过（R\*.max 6 不在 [3, 5] 里）；Rp.max 5 在里面，`actingd-config` 三方有公共修订，`runtime-client` 有交集，`runtime-outcomes` 1 在 [1, 1] 里。
    从根外跑的这两个控制台连本版：请求和回执照常；账本窗口读到含 `device.backend_switched` 的那一页会失败，指示灯、卡片、设置页和实时画面不受影响（UI 频道
    11-04 的判读）。所以控制台要换成建在修订 6 读者上的构建（钉 v0.12.39，`ledger` 含 6）。设置页上，UI v0.12.6 钉的 `ui_tier` 不认两个后端键，当作 `hidden`，
    不显示；`start_emulator` 照旧在 `daily` 里。
  - 钉 v0.12.38 的 actingctl、MCP 和 actinglab 连本版：设置列表的形状没变，`config.parameters.instance` 每行仍是 `key`、`value`、`source`、`settable`，两行新的
    后端行和 `settable` 变成 true 的 `start_emulator` 行都解得开；新码 `backend_switch_refused` 在回执里是开放的字符串，也解得开。要注意的两处：
    - 它们发带后端键的 `ConfigureSettings` 只按普通请求等回执（IO 超时，默认 5 秒），切换没答完就报超时，而切换可能照样提交，所以不要用旧客户端切；
      超时以后照上面「未确认」的规矩先重读再说。
    - 切换成功过以后，读到 `device.backend_switched` 的事件页或订阅帧，`forensic` 投影的 actingctl `events` 和 MCP `ac_events`、`lab` 投影的 actinglab 都解不开
      （只有 `cli`、`concise` 投影不带载荷）。
  - 钉 v0.12.37 的 actingctl、MCP 和 actinglab 连本版：和钉 v0.12.38 的一样。
  - 本版的 actingctl 和 MCP 连 v0.12.38 的 Runtime（部署切换的几秒里）：照常；带后端键的 `config set` 和 `ac_config_set` 拿到 `config_key_unknown`，
    设 `start_emulator` 拿到 `config_key_not_settable`。

运行上的变化：

- Nemu 触控的动作（HS-4）：复位、长按、普通拖动在 Nemu 触控上执行，`key`、`text` 仍不支持；能力表补上声明 `input.segmented_swipe`。
- 后端两行（HS-1）：每台设备实例的触控、截图后端是两行设置（`advanced`），带整实例检查；`actingd-config` [5, 26]，`nemu_app_index` 放宽。
- 入口恢复包（RH-1）：请求自带的恢复包和配置的回主页包按同一条规矩判，只声明 `scheduling_outcome` 的不再被拒。
- 运行中切换后端（HS-2）：带后端键的 `ConfigureSettings` 先切设备再提交，记 `device.backend_switched`；被拒是 `backend_switch_refused`，失败恢复原后端。
- 免打扰条目（BX-1）：解析和序列化导出给客户端，Runtime 不变。
- 配对编号（HS-3）：没写 `nemu_app_index` 时，打开配对会话前读一次 MuMuManager 的 `app info -pkg`，读不出来以 `nemu_app_index_problem` 具名拒绝。
- 自动启动模拟器（R-3）：`start_emulator` 在线可改，只限发现绑定的实例；每个自动启动在决定时读生效值。
- 账本修订 6（HS-2b）：`ledger` [3, 6]，部署要带 `--accept-ledger-one-way`、多一步 `ledger_verify`，装上后退不回 v0.12.38；切换的回执最多等 133 秒，等完没有
  回执算未确认，先重读 `config_version` 和实例的 `activity` 再决定重发。

本版合并的 PR：

- #1509 HS-4（#402）：Nemu 触控的复位（接触点没确认抬起才抬一次，不要帧）、长按和普通拖动（带这一步的帧，受几何围栏和时限约束）；`NemuInputSupport` 一份声明，主机闸门和内核都按它走；能力表和 Lab 清单照实写。
- #1508 HS-1（#402）：`instance.<instance id>.touch_backend`、`.capture_backend` 两行（`advanced`），整实例后端检查（`backend_kind_unsupported`、`backend_pair_required`、`backend_switch_not_alone`），启动恢复丢掉破坏检查的后端值；`nemu_app_index` 放宽，`nemu_paired_input_configuration_missing` 退役。`actingd-config` [5, 26]。
- #1510 RH-1（#433）：请求自带的入口恢复包和配置的回主页包共用 `scheduling_outcome_incompatibility`，只声明 `scheduling_outcome` 的恢复包不再被拒，它的调度结果不起作用。
- #1511 HS-2（#402）：运行中切换实例的触控和截图后端：认领 `backend_switch` 最多等 3 秒、控制门、关会话、换注册、按「切换」目的准备，成功才提交并记 `device.backend_switched`，失败恢复原后端；`backend_switch_refused`；客户端回执等待 `BACKEND_SWITCH_WAIT_MS`；`ac_config_set` 带后端键要 operator 档。
- #1513 BX-1（#429）：`BlackoutListKey`、`BlackoutEntry::parse`、`parse_list`、`to_json`、`list_json` 导出给客户端，配置检查和客户端共用一份实现。
- #1512 HS-3（#402）：配对会话打开时没写 `nemu_app_index` 就读一次 MuMuManager 的 `app info -pkg`，`running` 取 0，其余以 `nemu_app_index_problem` 具名拒绝；编号缓存在注册项里；`nemu_app_index_requires_paired_input` 退役。
- #1514 R-3（#429）：`instance.<instance id>.start_emulator` 在线可改（只限发现绑定的实例，`discovery_binding_required`），守护启动、看守重启和 IDLE-1 唤醒都读生效值，`start_emulator_off`。
- #1517 HS-2b（#402）：HS-2 的记录 `device.backend_switched`（动作、载荷和词表 `nemu_app_index_source`）定为账本修订 6，声明 `ledger` [3, 6]，更新要带 `--accept-ledger-one-way`、回退被拒；`BACKEND_SWITCH_WAIT_MS` 改成切换各步上界之和 133 秒（新契约常量 `LEDGER_WRITER_REPLY_WAIT_MS` 10 秒，账本的默认值取它，值不变），等完没回执算未确认、先重读；新安装测试核更新和回退两边。

## v0.12.38

本版是 v0.12.37 之后合并的 5 个 PR：LOAD-4、AGT-2b、LOAD-4b、R-12、RA-1。先说部署：账本的声明不变（仍是 [3, 5]），部署不用带
`--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；装上以后能退回 v0.12.37，条件见下面的退回。没有新键，默认值文件没改。
运行上要知道的是这几处：

- **加载页仍然默认关**（`execution.loading.mode` 默认 `off`）。设成 `on` 以后，固定步骤类任务（`linear_steps`）现在也像 0.12.37 起的页图任务那样
  在加载画面上等（LOAD-4）：加载页优先于候选页，加载画面不算到达、不算中间页，也不会在上面补点；等待按每次输入一份的额度暂停，额度用完才
  带加载值具名失败。`shadow` 只评估和记录。不论哪种模式，准入时多拒收两种写法：
  - 声明了加载页、却哪里都没写上限（没有 `expect_after.loading.max_ms`、没有等待页、包里也没有加载页副本）：`contained_task_program_invalid`，
    `program_problem` 为 `loading_max_missing`，`json_pointer` 为 `/operations/<i>/expect_after/loading`，页图和固定步骤都拒（LOAD-4）；
  - 固定步骤里 `loading.add` 列的页或等待页，是这一步的等待要等的页：可选操作串里输入可能到达的可选页、串的跳过目标（`from`、目的页和中间页
    LOAD-2 已经拒）：`contained_task_linear_invalid`，`reason=loading_page`，带 `operation=<id>`（LOAD-4b）。

  按登记簿，现在没有包声明 `loading`，这两种拒收不影响装着的包。
- **每日汇总**（AGT-2b）要配置里有 `agent_wake` 段，段里有一条 `daily_summary` 规则；生产配置没有这一段，所以什么也不发。打开唤起的顺序不变：
  先把控制台升级到声明 `runtime-client` 上限 5 的版本，再加这一段并重启，再用 `actingctl agent-channel add` 登记频道。规则只写
  `on.kinds: ["daily_summary"]`，不带 `codes`，写 `on.at_minute_of_day`（0 到 1439）和 `on.utc_offset_minutes`（−720 到 840），`daily_cap` 为 1；
  别的写法以 `agent_wake_config_invalid`（`config_value_problem` 为 `daily_summary_shape`）拒绝。按规则自己的偏移算本地日，每天到那一分钟发一条
  `今日汇总 <日期> 正文 <路径>`；那一刻 Runtime 没在跑，就在同一天启动后补一次，绝不补前一天；静默、hold 或功能关闭期间等结束再发，当天先结束
  就没有，下一份在 `skipped` 里写明。
- **`RoutineSummary` 现在有回答**（以前是 `not_implemented`，R-12）：按实例和任务数成功、按结果码分开的失败和打断（只有 `contained_task_cancelled`
  算打断），按宿主本地钟的小时或自然日分桶，每个实例最多留 48 个小时桶、14 个日桶；只在内存里，不读也不写账本，重启以后从头算（新的
  `owner_epoch`，覆盖起点是这次启动）。新命令 `actingctl routine-summary (--hour | --day) [--instance <alias|instance_id|ADB port>] [--since <revision>]`，
  新 MCP 工具 `ac_routine_summary`（observer 档，MCP 工具从 40 个变成 41 个），成功码是新码 `request_routine_summary_completed`。
- **读帧不再排在账本写线程后面**（RA-1）：取素材的两次账本查找改由账本的读者池在写线程最后发布的点上回答，不再等本轮写入，也不再挡住写入；
  没有读者池或读者全部结束时退回写线程，和以前一样。答复和拒绝都不变，没有新的失败种类。

配置：`actingd-config` 从 [5, 24] 改为 [5, 25]（AGT-2b，`agent_wake.rules` 里一条规则的 `on.at_minute_of_day`、`on.utc_offset_minutes` 两个成员，
没有新键；本版其余几片都不动它）；`package` 仍是 [3, 6]（LOAD-4、LOAD-4b 只是对已收形状更严的拒收，没有新成员）；`runtime-client` 仍是 [3, 5]
（R-12 是 R-0 已有请求的行为，形状没变）；`ledger` 仍是 [3, 5]（RA-1 不改任何落盘形状）；`install-selection` 仍是 [1, 1]，`install-control` 仍是
[0, 1]，`runtime-outcomes` 仍是 [1, 1]。`component-interfaces.json` 和 `contracts/component-interfaces.md` 的 `actingd-config` 25 一行一致。

- 没有新键，默认值文件和配置示例都没改，部署计划的默认值对比是空的。修订 25 读得了修订 5 到 24 的配置文件，只有一种例外：写了 `daily_summary`
  规则却没写时间的文件（0.12.37 收下这种规则，但从不按它做事），本版以 `daily_summary_shape` 拒绝。生产配置没有 `agent_wake` 段，不受影响。
- 结果码：新码 1 个（成功码 `request_routine_summary_completed`），没有新位置；已有词表加词 2 个（`config_value_problem` 的 `daily_summary_shape`、
  `program_problem` 的 `loading_max_missing`）；`contained_task_linear_entry_unmatched`、`contained_task_linear_application_unconfirmed` 多可选值
  `loading_page_id`、`loading_ms`、`loading_limit_ms`（`page_confirmation_failed` 从 0.12.37 起就有）。结果码目录从 3887 个码、1738 个位置变成
  3888 个、1738 个；没有新定稿或退役的条目，所以本版的发布文件是空的。
- 账本：`ledger` 不升，没有新记录形状。`RoutineSummary` 不读不写账本；每日汇总的行沿用 `host.agent_wake` 已有的行形状（`kind` 为 `daily_summary`，
  没有 `instance_alias` 和 `code`），正文 `actingcommand.agent-wake-body.v1` 多一个成员 `summary`。取素材在写线程的提交统计里照旧记作
  `ResolveArtifact`，不算读者池的页数。
- 程序文件：workspace 成员、Runtime 包和 Tools 包的文件清单都和 v0.12.37 一样。

升级时要知道的：

- 不用 `--accept-ledger-one-way`（按安装工具的代码核过）：两边的 `ledger` 都是 [3, 5]，`previous_max_in_next`（5 在 [3, 5] 里）和 `one_way`
  （R\*.max 5 不大于 Rp.max 5）都过；带上也不改判断。现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`，apply 不读账本。
- 安装命令的代码自 v0.12.37 没有改，v0.12.37 的安装工具判的边和本版一样。本版的 check-config 读现行配置照常通过（现行配置没有 `agent_wake` 段）；
  包的边在单一位置根上只有 `pack_declaration_unknown` 警告。
- 部署的自动回退和 `install undo` 这次可以用：账本不变，接口边都过，只要配置还没写进修订 25 的成员（刚部署完就是这样），回退门的配置一项也过。
- 资源侧要留意：打算用 `loading.add` 的包，要么写 `loading.max_ms`，要么带等待页或加载页副本，加的页也不能是这一步要等的页，否则本版准入就拒。

退回 v0.12.37（按安装工具的代码核过）：

- 安装工具允许退回。`ledger` 两边都是 [3, 5]：`previous_max_in_next`（Rp.max 5 在 R\* [3, 5] 里）和 `one_way` 都过，不用 `--accept-ledger-one-way`。
  `runtime-client` 两边都是 [3, 5]，`installer_meets_previous`、`installer_meets_next` 都过；`install-control`、`runtime-outcomes` 两边相同；`package`
  两边都是 [3, 6]，`bundle_max_in_next` 在单一位置根上只有 `pack_declaration_unknown` 警告。部署的自动回退、`rollback plan|apply`、`install undo`
  走的是同一组边和同一个回退门。
- `actingd-config` 25 → 24：没有哪条边直接比两边的范围。`installer_knows_keys` 也挡不住：`agent_wake.rules` 是 v0.12.37 认得的键（一个列表），它不往
  列表里面查。挡住退回的是配置的内容：回退门的配置一项让 v0.12.37 的 check-config 读现行配置，只要有一条规则写了 `on.at_minute_of_day` 或
  `on.utc_offset_minutes`，它就拒绝（规则的 `on` 拒收不认识的成员：`agent_wake_config_invalid`，`config_key` 为 `agent_wake.rules`，`config_value_problem`
  为 `wrong_type`），退回以 `rollback_gate_failed`（`rollback_gate_parts` 含 `config`，带 check-config 的拒绝）结束，什么都不改。**所以只有配置里有
  `daily_summary` 规则时，退回之前先从配置文件里删掉这条规则**；只删这两个成员 v0.12.37 也收，但再升回本版时没写时间的 `daily_summary` 规则会被
  拒，要补回。没有 `agent_wake` 段、或段里没有这种规则的配置不用改。在线改过的 `agent_wake.rules`（`host.config_overrides` 的行）不用管：v0.12.37
  启动时这一段恢复的在线值整段丢掉（`config_override_dropped`），不拒绝。
- `package` 不挡：本版新拒收的两种写法 v0.12.37 照收（固定步骤不等加载页；没写上限的加载页在页图里额度为 0）。
- 落盘的东西都不挡：账本没有新形状；`host.agent_wake` 里汇总的行是 v0.12.37 读得了的行形状，启动时照常恢复、计进总的每日上限；退回时还没送出的
  汇总行 v0.12.37 照样送，消息里没有日期（正文完整）。`RoutineSummary` 的统计只在内存里，没有落盘。运行诊断里 LOAD-4 的行沿用原来的行形状。
- 退回以后：`RoutineSummary` 又答 `not_implemented`，每日汇总不再生成；固定步骤不再等加载页；取素材回到写线程上。
- 客户端和 Runtime 的配对（`runtime-client` 本版和 v0.12.37 都是 [3, 5]、v0.12.36 是 [3, 4]；`apps/actingctl` 自 v0.12.37 只加了 `routine-summary`
  和 `ac_routine_summary`，Status 和 overview 的形状都没变）：
  - v0.12.37 的安装工具装本版：照常，不带 `--accept-ledger-one-way`。它的声明 I 是 `runtime-client` [3, 5]，和 Rp [3, 5]、R\* [3, 5] 都有交集
    （`installer_meets_previous`、`installer_meets_next`），`install-control` [0, 1] 相同，`runtime-outcomes` 1 在它的 [1, 1] 里，现行配置没有它
    不认识的键（`installer_knows_keys`），本版的 check-config 收现行配置（`next_check_config`）；账本两条边、没有 `ledger_verify` 都和上面一样。
    从本版的构建目录跑本版的安装工具，结果相同。
  - 声明 [3, 5] 的客户端（v0.12.37 的 actingctl、MCP 和 actinglab；UI v0.12.5 将是）连本版：有没有 `agent_wake` 段都照常。线上形状和 v0.12.37 一样，
    Status 的 `agent_wake` 也一样；`RoutineSummary` 的回答是 R-0 就有的形状，结果码在回执里是开放的字符串，新成功码也解得开（v0.12.37 的 actingctl
    和 MCP 本来就不发这个请求）。配置文件写进修订 25 的规则成员以后，UI 自己读写配置文件时认不认得，要 UI 侧确认（单一位置根的计划还不算 UI 的边，
    R-15c、R-15d 起才算）。
  - 声明 [3, 4] 的客户端（v0.12.36 的 actingctl、MCP 和 actinglab；UI v0.12.4 声明 `actingd-config` [5, 23]、`ledger` [3, 5]、`runtime-client` [3, 4]、
    `runtime-outcomes` [1, 1]）连本版：和连 v0.12.37 一样。
    - 没有 `agent_wake` 段：照常，[3, 4] 和 [3, 5] 有交集，Status 不带 `agent_wake`，别的回答形状和请求都没变。
    - 有 `agent_wake` 段（不论有没有 `daily_summary` 规则）：每次读 Status 都解不开（`RuntimeControlPlaneStatus` 拒收不认识的成员），报
      `runtime_receipt_decode_failed`，状态和 overview 都读不了，别的请求照常。配对检查照样通过，挡不住，所以先升级控制台。
  - 本版的 actingctl 和 MCP 连 v0.12.37 的 Runtime（部署切换的几秒里，或退回以后）：照常；只有 `routine-summary` 和 `ac_routine_summary` 拿到 Runtime
    的 `not_implemented` 拒绝（actingctl 打印回执，退出 1）。

运行上的变化：

- 固定步骤的加载页（LOAD-4）：`on` 下入口、中间页、下一步的页、可选候选、重试判定的等待都认加载页，额度用完以 `contained_task_linear_entry_unmatched`、
  `page_confirmation_failed` 或 `contained_task_linear_application_unconfirmed` 带加载值结束；`off`、`shadow` 的决定不变；没写上限的加载页准入时拒收。
- 每日汇总（AGT-2b）：`daily_summary` 规则按自己的偏移每天到点发一份汇总，正文带各实例的 `RoutineSummary` 日页、当天各类唤起的计数、上一份和
  中间空缺的日子；`actingd-config` [5, 25]。
- 加载页不能是要等的页（LOAD-4b）：固定步骤的 `loading.add` 和等待页落在这一步的可选候选或跳过目标上时准入拒收；页图不变。
- 例行健康统计（R-12）：`RoutineSummary` 从宿主内存回答，`actingctl routine-summary`、`ac_routine_summary`；不碰账本，重启清零。
- 取素材走读者池（RA-1）：两次账本查找由读者池回答，不排在写入后面，也不挡写入；读者自己的失败不致命，账本本身的故障照旧致命。

本版合并的 PR：

- #1502 LOAD-4（#407）：固定步骤类任务在 `execution.loading.mode` `on` 下的加载页等待，复用 LOAD-3 的判定和每次输入一份的额度；准入拒收哪里都没写上限的加载页（`loading_max_missing`）；`contained_task_linear_entry_unmatched`、`contained_task_linear_application_unconfirmed` 多三个加载值。
- #1503 AGT-2b（#432）：每日汇总，`daily_summary` 规则的 `on.at_minute_of_day`、`on.utc_offset_minutes`（`daily_summary_shape`），按规则的本地日到点、补发、静默后发，正文带每个实例的 `RoutineSummary`。`actingd-config` [5, 25]。
- #1504 LOAD-4b（#407）：固定步骤的加载页和等待页不能是这一步等待要等的页（可选候选、跳过目标），沿用 `contained_task_linear_invalid`，`reason=loading_page`。
- #1505 R-12（#429）：`RoutineSummary` 从宿主内存回答（成功、按码的失败、打断，小时和日桶，48 和 14 个上限，修订号和覆盖起点）；`actingctl routine-summary`、MCP `ac_routine_summary`；新成功码 `request_routine_summary_completed`。
- #1506 RA-1（#422）：取素材的账本查找改由读者池在已发布的点上回答，写线程只在没有读者时兜底；`ledger` 不变。

## v0.12.37

本版是 v0.12.36 之后合并的 10 个 PR，其中两个是各带五片的集成 PR：S4a-1、AGT-1a、STILL-1、OCR-SCALE-1，集成一（PF-1、ACT-1+2、AGT-1b、AGT-T1、PF-2），
集成二（AGT-2、AGT-T2、AGT-T3、ACT-4、LOAD-1），以及 LOAD-2、AGT-T1b、LOAD-3、SUB-LAG-1。先说部署：账本的声明不变（仍是 [3, 5]），部署不用带
`--accept-ledger-one-way`，计划里也没有 `ledger_verify` 这一步；装上以后能退回 v0.12.36，条件见下面的退回。
新行为在配置之前全都是关的：动作之后读帧的路径（`execution.action_gap.mode` 默认 `declared`）、静止画面的上限（`execution.capture.still_limit_ms`
默认 0）、加载页（`execution.loading.mode` 默认 `off`）、智能体唤起（没有 `agent_wake` 段，也没有登记的频道）。包侧的新写法（选择文档读共享表、
schema 0.9 的 OCR 字段、`expect_after.target_present` / `target_gone` / `loading`、包里的加载页副本）现在装着的包都没有用（v0.12.36 会拒收前几种）。
调度停着（每台实例都暂停）的时候，运行上只有一处变化：
**事件订阅者跟不上时，现在只结束它自己的这次订阅（SUB-LAG-1）。** 读事件流（`SubscribeEvents`）的客户端，比如控制台，读得慢、实时队列满了，
回执是 `failed`，码是 `subscription_lagged`（`host_halted` false），带 `resume_after_sequence`，按它重新订阅不丢事件；Runtime 和别的订阅者照常。
以前这种情况会让整个 Runtime 停下。
有运行的时候（不论是不是调度发起的）另有两处默认就生效，都不改任何决定：诊断流按块做容量判定（PF-1，每块 1 MiB，拒绝最多晚一块），
识别开始的两次提交也计进任务计时的分段（PF-2，两族的 `attempts` 变成开始加完成）。
另外要知道的：

- **`runtime-client` [3, 5]**（AGT-1b）：Status 多一个成员 `agent_wake`，只在配置有 `agent_wake` 段时出现。声明上限 4 的控制台（UI v0.12.4 是 [3, 4]）
  读到它就解不开整个 Status，安装工具的配对检查却照样通过，所以**先升级控制台，再往配置里加 `agent_wake` 段**。
- **`package` [3, 6]**：包可以声明 `expect_after.target_present` / `target_gone`（ACT-4）和 `expect_after.loading`（LOAD-2），选择文档可以声明
  `tables`（S4a-1），schema 0.9 的任务可以声明 `post_admission_ocr`（OCR-SCALE-1），包可以带 `resources/loading.json`（LOAD-1）。用到这些的资源版本
  要求本版：先部署 Runtime，再部署包。
- 新命令：`actingctl agent-channel add|remove|list|claude-hook|claude-register|notice`、`actingctl agent-wake-test`（都不是 MCP 工具，MCP 工具仍是
  40 个）；`actinglab package loading --root-file <file> --packs-root <dir> (--write | --check)`、`actinglab package bundle --loading`。

配置：`actingd-config` 从 [5, 23] 改为 [5, 24]（AGT-1a 升一次，STILL-1、ACT-1+2、PF-1、ACT-4、LOAD-3 都并进修订 24）；`package` 从 [3, 5] 改为
[3, 6]（S4a-1 升一次，OCR-SCALE-1、ACT-4、LOAD-1、LOAD-2 并进修订 6）；`runtime-client` 从 [3, 4] 改为 [3, 5]（AGT-1b，`min` 不动）；`ledger` 仍是
[3, 5]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。`component-interfaces.json` 和
`contracts/component-interfaces.md` 的 `actingd-config` 24、`runtime-client` 5、`package` 6 三行一致。

- 新键（`since` 都是 0.12.37；控制台分档没改，新键都是 `hidden`）：
  - `execution.capture.still_limit_ms`（STILL-1）：0 或 30000 到 1800000，默认 0（关）；可在线改，每一步读；每实例
    `instance.<id>.execution.capture.still_limit_ms`，默认跟节里的值。1 到 29999 由整节检查拒绝。
  - `execution.action_gap.*`（ACT-1、ACT-2、ACT-4）十一个：`mode`（`declared` 或 `settled`，默认 `declared`）、`kind`（`fixed`、`random`、
    `performance`，默认 `fixed`）、`fixed_ms`、`random_min_ms`、`random_max_ms`（0 到 60000，默认 0；`random_min_ms` 不大于 `random_max_ms`）、
    `poll_ms`（0 到 1000，默认 50）、`settle_frames`（1 到 8，默认 1）、`settle_change_max_milli`（0 到 1000000，默认 500）、`settled_max_stage`
    （`s0` 到 `s4`，默认 `s1`）、`fallback_hold_ms`（0 到 86400000，默认 3600000）、`settled_limit_ms`（0 到 5000，默认 2000）。都可在线改，
    每一步读；每实例同名键 `instance.<id>.execution.action_gap.<成员>` 没有默认值，不写就用节里的。`mode` 是 `declared` 时其余各键都不起作用。
  - `execution.loading.mode`（LOAD-3）：`off`、`shadow`、`on`，默认 `off`；可在线改；每实例 `instance.<id>.execution.loading.mode` 没有默认值。
    Lab 操作固定 `off`。
  - `artifact_store.stream_admit_chunk_bytes`（PF-1，新节）：0 到 67108864，默认 1048576；可在线改，新开的流才用新值；0 就是以前的每次写都判一次。
  - `agent_wake` 段（AGT-1a）的 19 个键：都没有默认值，整段写全或不写；不写时没有这些键、没有 `agent_wake` 的 subsystem 行，`ConfigureSettings`
    对它们答 `config_key_unknown`，所以第一次打开是改配置文件再重启。`body.dir`、`delivery.queue_capacity` 不能在线改，其余 17 个可以。写法、
    取值范围和示例在 `contracts/agent-wake.md`。
- 默认值文件：多上面 14 个有默认值的键，`ledger_observer.tag.workbench_patterns` 多一条 `host.agent_wake`（没有 `agent_wake` 段时 Runtime 唯一的
  不同）。部署计划的默认值对比因此有 15 行，来源都是 `default`，没有已有键的值变化。程序事实 `config.parameters` 多 `artifact_store` 一行，有
  `agent_wake` 段时再多它的参数行和 `config.subsystems` 的一行。
- `instances[]` 的条目多一个可选成员 `execution`（`capture.still_limit_ms`、`action_gap`、`loading`）。配置示例 `actingd.config.example.json` 现在
  写了 `execution.action_gap.kind: performance`，只在 `settled` 下起作用；照它写的配置 v0.12.36 不认（见退回）。
- 结果码：新码 18 个（S4a-1 的 `standard_pack_table_missing`、`standard_pack_table_mismatch`、`standard_pack_table_read`、`term_fallback_used`、
  `resource_table_identity_uncovered`；唤起的 `agent_wake_config_invalid`、`agent_wake_delivery_failed`、`agent_wake_message_too_long`、
  `agent_wake_channel_unknown`、`agent_wake_disabled`、`agent_budget_exhausted`、`invalid_agent_wake_test_origin`、`request_agent_wake_test_completed`、
  `agent_wake_test_queued`、`agent_channel_reported`、`agent_channel_failed`；诊断里的 `contained_task_action_gap_recorded`、
  `contained_task_loading_recorded`），新位置 5 个（`agent_wake_worker`、`agent_wake_restore`、`agent_wake_test`、`join_agent_wake_worker`、
  `runtime_agent_wake_test`），新值键 12 个（含 `resume_after_sequence`），新词表 4 个（`agent_delivery_state`、`agent_delivery_problem`、
  `loading_event`、`table_mismatch`）。已有的码多了可选值：`contained_task_step_unconfirmed`、`contained_task_page_unknown`、
  `page_confirmation_failed` 多 `held_ms`、`limit_ms`（STILL-1）和 `loading_page_id`、`loading_ms`、`loading_limit_ms`（LOAD-3）；
  `subscription_lagged` 多 `ledger_sequence`、`resume_after_sequence`；`standard_pack_root_invalid` 多 `package_id`。已有词表加词：
  `config_value_problem` 9 个、`program_problem` 7 个、`standard_pack_problem` 8 个。结果码目录从 3869 个码、1733 个位置变成 3887 个、1738 个；
  没有新定稿或退役的条目，所以本版的发布文件是空的。
- 账本：`ledger` 不升，没有新记录形状。新码只出现在已有的记录上（比如 `standard_pack_read` 的 `standard_pack_table_read`、唤起工作线程的非致命
  `runtime.failed`）。PF-2 把 `RecognitionStarted` 的两次提交也记进已有的 `recognition_payload_stages`、`recognition_task_stages`，这两族的
  `attempts` 和 v0.12.37 以前的值不能逐次比较。
- 程序文件：多一个 workspace 成员 `crates/agent-forward`（库，不是新程序）；Runtime 包和 Tools 包的文件清单和 v0.12.36 一样。AGT-T1b 的 C# 小助手
  和 AGT-T3 的 DSH 助手都嵌在程序里，前者在第一次唤起会话时用 .NET Framework 的 `csc.exe` 编译到状态根的 `agent-forward\claude-input\`。
  `NOTICE.md` 多一节从 codex-goal-lifecycle（MIT）改写的源码，`actingctl agent-channel notice` 打出它的许可文本。

升级时要知道的：

- 不用 `--accept-ledger-one-way`（按安装工具的代码核过）：两边的 `ledger` 都是 [3, 5]，`previous_max_in_next`（5 在 [3, 5] 里）和 `one_way`
  （R\*.max 5 不大于 Rp.max 5）都过；带上也不改判断。现行和目标的 `ledger` 相同，所以计划里没有 `ledger_verify`，apply 不读账本。
- 安装命令的代码自 v0.12.36 没有改（只改了一个测试的期望值和一行注释），v0.12.36 的安装工具判的边和本版一样，只是自己的声明和认得的键不同（见下面的配对）。
  本版的 check-config 读现行配置照常通过（现行配置没有修订 24 的成员）；包的边在单一位置根上只有 `pack_declaration_unknown` 警告（见退回）。
- 部署的自动回退和 `install undo` 这次可以用：账本不变，接口边都过，只要配置还没写进新成员（刚部署完就是这样），回退门的配置一项也过。
- 打开唤起的顺序：先把控制台升级到声明 `runtime-client` 上限 5 的版本，再加 `agent_wake` 段并重启，再用 `actingctl agent-channel add` 登记频道，
  最后用 `actingctl agent-wake-test` 试一次。四种转发都没在真会话上试过（开发和 CI 没碰过真会话、真管道和真令牌，AGT-T1b 能否拉起已停的会话也没验证），
第一次实测之前不要依赖它；
  客户端每次更新后跑一次测试消息。
- STILL-1 的上限要先离线统计成功运行里最长的合法静止段再定，在那之前保持关，或只对单台实例开。ACT-1+2 开 `settled` 之前要实测 `when`、
  `on_guard_refused` 判最早那一帧会不会选错。

退回 v0.12.36（按安装工具的代码核过）：

- 安装工具允许退回。`ledger` 两边都是 [3, 5]：`previous_max_in_next`（Rp.max 5 在 R\* [3, 5] 里）和 `one_way` 都过，不用 `--accept-ledger-one-way`。
  `runtime-client` 5 → 4：`installer_meets_previous`、`installer_meets_next` 只比安装工具和两边的交集，本版的 I [3, 5] 和 Rp [3, 5]、R\* [3, 4]
  都有交集（v0.12.36 的 I [3, 4] 也一样），过。`install-control`、`runtime-outcomes` 两边相同。部署的自动回退、`rollback plan|apply`、`install undo`
  走的是同一组边和同一个回退门。
- `actingd-config` 24 → 23：没有哪条边直接比两边的范围，挡住退回的是配置的内容。回退门的配置一项让 v0.12.36 的 check-config 读现行配置，配置里只要有
  修订 24 的成员（`agent_wake` 段、`artifact_store` 段、`execution.action_gap`、`execution.capture.still_limit_ms`、`execution.loading`，或
  `instances[]` 条目的 `execution`），它就拒绝（这些地方都拒收不认识的成员），退回以 `rollback_gate_failed`（`rollback_gate_parts` 含 `config`，
  带 check-config 的拒绝）结束，什么都不改。用 v0.12.36 的 `actingctl` 来退回时，`execution` 下的新成员先让 `installer_knows_keys` 不过，以
  `interface_edge` 拒绝。**所以退回之前先从配置文件里删掉这些成员。** 在线改过的值（`host.config_overrides` 的行）不用管：v0.12.36 启动时逐行丢掉
  （`config_override_dropped`），不拒绝。
- `package` 6 → 5：`bundle_max_in_next` 对登记的每个游戏根比它声明的上限在不在 R\* [3, 5] 里，声明 [6, 6] 的会以 `interface_edge` 拒绝。可单一位置根的包
  登记是接管时写的，不带声明，所以这条边只给 `pack_declaration_unknown` 警告，挡不住。用了新写法的包在 v0.12.36 上准入时被拒：选择文档的 `tables`
  （`contained_task_select_policy_invalid`），`expect_after.target_present` / `target_gone` / `loading` 和 schema 0.9 的 `post_admission_ocr`
  （声明门 `resource_declaration_invalid`），步骤计划的 `wait_page`（`contained_task_linear_invalid`）；只带加载页副本的包照常跑，副本被忽略。
  **所以退回之前先把包换回不用这些写法的版本。** 现在没有包用它们。
- 落盘的东西都不挡：账本没有新形状，回退门的账本一项会让 v0.12.36 自己的读者把部署以来的页读一遍来核；运行时事实 `host.agent_wake`，v0.12.36 当作
  不认识的键接受；`<state_root>\agent-forward\`（频道登记、Claude 会话记录、编译好的小助手、`claude-woken.json`）和唤起的正文目录留在盘上，
  v0.12.36 不读。会话记录里有 DPAPI 保护的令牌，要清掉就先 `actingctl agent-channel remove`。运行诊断里 ACT-1+2、ACT-4、LOAD-3 的新行沿用原来的行形状。
- 退回以后，一个读得慢的事件订阅者又会让 v0.12.36 的 Runtime 停下（SUB-LAG-1 只在本版）。
- 客户端和 Runtime 的配对（`runtime-client` 本版 [3, 5]、v0.12.36 [3, 4]；`apps/actingctl` 自 v0.12.36 加了 `agent-channel`、`agent-wake-test`，
  以及 `overview` 和 `ac_overview` 的 `agent_wake` 块，MCP 工具仍是 40 个）：
  - v0.12.36 的安装工具装本版：照常，不带 `--accept-ledger-one-way`。它的声明 I 是 `runtime-client` [3, 4]，和 Rp [3, 4]、R\* [3, 5] 都有交集
    （`installer_meets_previous`、`installer_meets_next`），`install-control` [0, 1] 相同，`runtime-outcomes` 1 在它的 [1, 1] 里，现行配置没有它
    不认识的键（`installer_knows_keys`）；账本两条边、没有 `ledger_verify` 都和上面一样。从本版的构建目录跑本版的安装工具（I 是 [3, 5]）结果相同。
  - 修订 [3, 4] 的客户端连本版（UI v0.12.4 声明 `actingd-config` [5, 23]、`ledger` [3, 5]、`runtime-client` [3, 4]、`runtime-outcomes` [1, 1]；
    v0.12.36 的 actingctl、MCP 和 actinglab 也是 [3, 4]）：
    - 没有 `agent_wake` 段：照常。Status 不带新成员，别的回答形状和请求都没变。订阅跟不上时它收到 `failed` 的 `subscription_lagged`
      （`host_halted` false）；结果的值是开放的，不认识的 `resume_after_sequence` 也解得开，它不读这个游标，就按自己记下的游标重订。UI 的边按代码
      都过：`actingd-config` 三方共同的 5 到 23；Rp.max、R\*.max 都是 5，在 [3, 5] 里；[3, 4] 和 [3, 5] 有交集；`runtime-outcomes` 1 在 [1, 1] 里。
    - 有 `agent_wake` 段：每次读 Status 都解不开（`RuntimeControlPlaneStatus` 拒收不认识的成员），报 `runtime_receipt_decode_failed`，状态和 overview
      都读不了，别的请求照常。配对检查照样通过，挡不住，所以先升级控制台。MCP 前端还没换子进程时的旧 actingctl 也一样。
    - UI 声明的 `actingd-config` 上限是 23：配置文件写进修订 24 的成员以后，UI 自己读写配置文件时认不认得，要 UI 侧确认（单一位置根的计划还不算
      UI 的边，R-15c、R-15d 起才算）。
  - 本版的 actingctl 和 MCP 连 v0.12.36 的 Runtime（部署切换的几秒里，或退回以后）：照常。Status 没有 `agent_wake`，overview 就不显示这一块；只有
    `agent-wake-test` 会被断开连接（v0.12.36 解不开 `AgentWakeTest`）。`agent-channel` 不连 Runtime。
  - actinglab 和 Runtime 混版：`package loading`、`package bundle --loading`、`resource validate` 的新族和 `package check` 的 `loading` 成员都在本机读，
    和 Runtime 无关。资源侧要留意：从本版起，带加载页副本的包，游戏根上没有 `loading.json` 时 `standard-pack check` 拒绝它，`package bundle` 不带
    `--loading` 也拒绝它。

运行上的变化：

- 事件订阅（SUB-LAG-1）：跟不上的订阅者只结束自己这次订阅，带 `resume_after_sequence`；Runtime 不再因此停下，写入线程的故障仍然致命。
- 读共享表（S4a-1）：选择文档可声明 `tables`；表缺失、不一致或坏了只拒这一次运行；每张读到的表记一条 `standard_pack_read`；调度层仍拒读表的文档。
- 唤起配置（AGT-1a）：`agent_wake` 段和频道登记的形状；没有这一段时只多一条 `workbench_patterns`。
- 唤起工作线程（AGT-1b）：有这一段时起一个线程，写 `host.agent_wake`，Status 和 overview 多 `agent_wake`；人手动的 `actingctl agent-wake-test`。
- 自动唤起（AGT-2）：有带规则的段时，阶梯用尽、实例隔离后等人处理、规则列出码的运行失败会唤起规则指定的频道；同一实例一阵合并成一条，有冷却和按本机日的上限。
- Claude Code 转发（AGT-T1）：`actingctl agent-channel` 登记频道；令牌只在 DPAPI 文件和发送那一刻的内存里。
- 唤起已停的 Claude 会话（AGT-T1b）：发送前经 Claude Desktop 把会话打开，只打开、不输入；每条消息最多一次，`agent-channel add --restore off` 可关。
- Codex 转发（AGT-T2）、DSH 转发（AGT-T3）：这两种频道的消息不再一直等到过期。
- 流的容量判定（PF-1）：每 1 MiB 判一次（`artifact_store.stream_admit_chunk_bytes`），拒绝和卷绑定变化最多晚一块被看到。
- 识别开始的分段计时（PF-2）：计进已有的两族，`attempts` 是开始加完成；不升 `ledger`。
- 静止上限（STILL-1）：静止又不被接受的等待到 `execution.capture.still_limit_ms` 提前结束，默认关。
- 动作后读帧和动作间隔（ACT-1+2）：`execution.action_gap.*`，默认 `declared`，和以前逐字节相同。
- 终态目标（ACT-4）：包可声明 `target_present`、`target_gone`；`settled` 下没写 `post_delay_ms` 的点击最多读 `settled_limit_ms`。
- OCR 字段（OCR-SCALE-1）：schema 0.9 可声明 `post_admission_ocr` 的 `fields_v1`，字段框跟着带 `scales` 的锚点缩放。
- 加载页（LOAD-1、LOAD-2、LOAD-3）：游戏根的 `loading.json` 由 `actinglab package loading` 拷进每个包，加载器编译副本、准入 `expect_after.loading`；
  页图只在 `execution.loading.mode` `on` 下等加载页，`shadow` 只记录，默认 `off`。

本版合并的 PR：

- #1481 S4a-1（#357 DB-2，#430）：选择文档可声明 `tables`，按键从游戏根的共享表取值；运行准备时绑定，表的问题只拒这一次运行（新码 `standard_pack_table_missing`、`standard_pack_table_mismatch`）；每张读到的表记一条 `standard_pack_read`（`standard_pack_table_read`）。`package` [3, 6]。
- #1482 AGT-1a（#432）：`agent_wake` 配置段（19 个键，整段写全或不写）、频道登记的形状、新 crate `agent-forward` 的骨架，以及消息和正文的纯函数。`actingd-config` [5, 24]。
- #1484 STILL-1（#407）：`execution.capture.still_limit_ms`，静止又不被接受的页等待和确认到上限提前结束，沿用原来的码并带 `held_ms`、`limit_ms`；默认 0（关）。
- #1485 OCR-SCALE-1（#428）：schema 0.9 可声明 `post_admission_ocr` 的 `fields_v1`；字段框挂在带 `scales` 的锚点上，随匹配倍率缩放；0.7 的 comparison 在 0.9 里仍被拒。
- #1490 0.12.37 集成：PF-1（#407）流的每块一次容量判定；ACT-1+2（#407）`execution.action_gap.*`，默认 `declared`；AGT-1b（#432）唤起工作线程、`host.agent_wake`、Status 和 overview 的 `agent_wake`、`actingctl agent-wake-test`，`runtime-client` [3, 5]；AGT-T1（#432）Claude Code 转发和 `actingctl agent-channel`；PF-2（#407）识别开始记录的分段计时。
- #1496 0.12.37 集成二：AGT-2（#432）首批三种自动触发、规则、合并、冷却和按本机日的上限；AGT-T2（#432）Codex 转发；AGT-T3（#432）DSH 转发；ACT-4（#407）`expect_after.target_present`、`target_gone` 和 `settled_limit_ms`；LOAD-1（#407）游戏根的 `loading.json`、包里的副本和 `actinglab package loading`。
- #1497 LOAD-2（#407）：加载器编译包里的加载页副本，准入 `expect_after.loading` 和步骤计划的 `wait_page`；`resource validate` 认得这两个文件；运行时还不等。
- #1498 AGT-T1b（#432）：Claude 频道的会话已停时，发送前经 Claude Desktop 把它打开，只打开、不输入；每条消息最多一次，没登记上线的会话不再反复打开。
- #1499 LOAD-3（#407）：页图在加载页上的等待、`execution.loading.mode`（`off`、`shadow`、`on`，默认 `off`）和诊断码 `contained_task_loading_recorded`。
- #1500 SUB-LAG-1（#419）：跟不上的事件订阅者只结束自己的订阅和这次请求，带 `resume_after_sequence`；写入线程的故障仍然致命。

## v0.12.36

本版是 v0.12.35 之后合并的 8 个 PR：114-A、R-4、R-0b、R-0b2、R-11、LN-VERIFY、INST-UNDO-EDGE、FLAKY-LNWS1。先说部署：账本的声明从 [3, 4] 改为
[3, 5]（114-A），这一步是单向的。部署（`actingctl install update plan`、`update apply`）要带 `--accept-ledger-one-way`；装上本版以后，安装工具拒绝退回
v0.12.35 或更早的版本，部署自己的自动回退也一样被拒（见下面的退回）。本版一条修订 5 的记录都不写，单向的只是声明。部署多一步账本校验，从本版起它是
有界的，不再整本读进内存（LN-VERIFY，见下面的账本校验）。
运行中会注意到的变化有三件：
一是 `actingctl status`、`actingctl overview` 和 `ac_overview` 都显示每台实例现在在做什么（R-0b、R-0b2）：实例行的 `activity`（`{kind, request_id?,
package_id?, since_unix_ms}`，`kind` 是 `policy_run`、`direct_run`、`host_run`、`reconnect`、`selfcheck`、`emulator_control`、`ladder` 之一），什么都不做
的实例没有这个成员。它来自 overview 本来就读的那一次 Status，不多发请求、不读账本，MCP 的输出预算不会截掉它。
二是设置的读写都带 `config_version`（R-4）：`actingctl config set|clear` 和 `ac_config_set` 在 `settings` 旁边答改完后的版本，
`actingctl status --config` 和 `ac_facts`（scope `config`）在记录旁边答现在的版本。带着过期 `expected_version` 的 `ConfigureSettings`
以 `fact_observation_version_conflict` 拒绝（`fact_key` 是 `host.config_overrides`，`current_version` 是现在的版本），什么都不改，只记一条
`command.rejected`。本版的 actingctl 和 MCP 都不发 `expected_version`，所以照旧是后写者赢，只有发它的控制台才会碰到这种拒绝。
三是包列表多了几个成员（R-11）：`actingctl packs` 和 `ac_packs` 在组合模式下给每个组合出的安装行加 `task_ids` 和 `irreversible`，游戏根
带 `ui.json` 时游戏行和安装行再加 `ui`、`display`；现在没有资源仓的标准包带 `ui.json`。
调度停着（每台实例都暂停）的时候，运行上别的都不变：114-A 不写记录，LN-VERIFY 只改维护命令 `verify`，INST-UNDO-EDGE 只改 `install undo`，FLAKY-LNWS1 只改
测试。v0.12.35 已在本版之前发布。

配置：`ledger` 从 [3, 4] 改为 [3, 5]（114-A）；`runtime-client` 从 [2, 4] 改为 [3, 4]（R-0b，`min` 升到 3）；`actingd-config` 仍是
[5, 23]，`package` 仍是 [3, 5]，`install-selection` 仍是 [1, 1]，`install-control` 仍是 [0, 1]，`runtime-outcomes` 仍是 [1, 1]。

- 没有新配置键：配置键表和 `actingd.defaults.json` 自 v0.12.35 都没改，在线可改的键不变，程序事实 `config.parameters`、
  `config.subsystems` 都没有新行。已有的 `ledger_observer.open_verify_tail_records` 多一个读者：不带标志的 `verify` 按它决定认证多少条（LN-VERIFY）。R-4 的 `ui_tier`（`actingcommand-runtime-config`，契约的「Console tiers」一节）只给已有的键分档：
  daily 8 个（性能预设的 `performance.pacing.mode`、`performance.pacing.max_concurrent_runs`、
  `performance.pacing.heavy_dispatch_limit.normal`、`scheduling.parallel.enabled`，`scheduling.blackout.host`，每实例的
  `scheduling.blackout`、`start_emulator`、`idle.action`），advanced 6 个（`mumu_root`、`frame_retention_evidence_max_age_days`、
  `host.process_memory_priority`、`host.process_io_priority`、`lab_module_authoring`、`lab_module_debugging`，上限 8 个），其余都是
  `hidden`。分档不改哪个键能不能在线改（`start_emulator` 在 R-3 之前仍不能在线改）。
- `ledger` 5 是 4 加上记录类型 `policy.not_run_changed`（policy 族，动作 `policy.not_run`，info）和它的全部形状：公共头、变动集、基线
  分片、预算标记、退役、`prior` 和 `closes`，账本自己的 Status 和 ScheduleExplanation 副本（四个控制成员、`effective`、`PlayerHoldEnd`、
  行的 `state` 和 `eligibility`、汇总、派单状态）和它们的闭合词表，以及两条索引前缀 `not_run/<alias>/<task>`、
  `not_run_stream/<alias>`。只有形状，本版的 Runtime 一条都不写（写侧是 114-B）：SQLite 格式 2、`actingcommand.event.v3`、表、索引和
  每个视图的类型表都不变，没有迁移，也不用新的状态根，修订 3、4 的账本照常读。读侧只查形状、闭合词表、不靠码的自洽和码的拼写，不查码
  有没有登记；以后加码不升修订，往闭合词表里加词、增删成员才升。
- `runtime-client` 3、4 的形状都没变，本版第一次填修订 3 的回答成员：Status 实例行的 `activity`（R-0b），`SettingsConfigured` 和
  `RuntimeFactSnapshot` 的 `config_version`（R-4）。这些成员所在的形状都拒收不认识的成员，修订 2 的读者解不开，所以 `min` 升到 3。
  请求 v4、回执 v2 的 schema 字符串不变；玩家持有的两种请求照旧答 `not_implemented`，Status 照旧不带持有的成员。
- 结果码：没有新码。新位置 1 个（`begin_instance_activity`，R-0b：登记一项活动失败时用，和每个中毒的宿主锁一样致命）；新值键 1 个
  （`current_version`，integer，R-4）；`fact_observation_version_conflict` 多两个可选值 `fact_key`、`current_version`，说明、常见原因和
  处理都写了；`package_binding_unreadable`、`standard_pack_root_invalid`、`maintenance_artifact_material_unavailable`、`ledger_open_measured` 只改说明。已有词表加词：`standard_pack_problem` 的
  `ui_out_of_limits`、`ui_duplicate`、`ui_group_unknown`、`ui_parameter_undeclared`（R-11），`event_type` 的 `policy.not_run_changed`
  和 `event_action` 的 `policy.not_run`（114-A）；`bundle_missing` 的说明多了 `ui.json`。结果码目录仍是 3869 个码，位置从 1732 个变成
  1733 个；没有新定稿或退役的条目，所以本版的发布文件是空的。
- 只在内存里或本机读的：R-4 的设置版本是内存里算的令牌（生效中的每个键和生效中的 `host.config_overrides` 行，做 SHA-256 取前 63 位，
  只比相等），不落库，`host.config_overrides` 的行形状不变；R-0b 的 `activity` 只在内存里，重启后为空；R-11 只在本机读包根。状态文件的
  形状也不变。
- 包根多一个可选文件 `ui.json`（`actingcommand.ui.v1`，R-11）：只查形状、上限和引用（文件 256 KiB、组 64、包 1024 等，见
  `package-reference.md` 的上限表），每处违规是一条带 JSON 指针的 `standard_pack_root_invalid` 警告，不坏游戏根，也不改行。只有
  `actingctl packs` 和 `ac_packs` 读它，Runtime 的根读取器、check-config 和 `actinglab standard-pack check` 都不读；`package` 不升。
- 程序文件：没有新程序、新 workspace 成员，Runtime 包和 Tools 包的文件清单和 v0.12.35 一样。

升级时要知道的：

- 账本单向（114-A，按安装工具的代码核过）：`update plan`、`update apply` 不带 `--accept-ledger-one-way` 时，`ledger` 的 `one_way` 边
  不过（R\*.max 5 大于 Rp.max 4），以 `interface_edge` 拒绝，什么都不改；带上以后 `previous_max_in_next`（4 在 [3, 5] 里）和 `one_way`
  都过，计划的 `notice.rollback_window_bounded` 是 true，操作的 `acknowledged` 记下 `accept_ledger_one_way`。部署前还要确认在用的控制台
  声明的 `ledger` 上限到 5（UI 的 v0.12.0、v0.12.1 声明 [3, 5]）：单一位置根上 `actingctl install` 还不核控制台的边，UI 的边要到
  R-15c、R-15d 才进计划。
- 账本校验（LN-VERIFY）：现行和目标的 `ledger` 范围不同，所以 apply 多一步 `ledger_verify`：`--stop` 停下 Runtime 之后、换任何文件之前，用本版的
  `actingcommand-actingd.exe ledger-maintenance verify --config <root>\actingd.config.json` 校验账本。从本版起，不带标志的 `verify` 做的是 Runtime 启动时
  那一套有界检查，报告 `verified-sqlite-bounded`：格式和视图定义、带密钥的 meta 行和账本头、`MAX(sequence)`、序号连续，以及最新
  `ledger_observer.open_verify_tail_records` 条记录逐条认证。安装工具传的参数没变，由 v0.12.35 的安装工具部署也走这条路。
  - 它不读更早的记录和 State 表，不跑全文件 `quick_check`，也不看保留状态。内存不随历史增长；序号连续那一步走完一个索引，耗时随索引大小增长。
  - 所以更早的记录或未读的页坏了，这一步查不出来；State 表的校验和没做完的驱逐留给下次启动。要整本核，用 `verify --full`（以前的完整读，报告
    `verified-sqlite`，全部事件留在内存里，账本大时要留够内存和时间）；`--frame-query` 也是完整读。
  - 外面的期限是 `install.child.ledger_verify_timeout_ms`（默认 1800000）。不过时以 `ledger_verify_failed` 结束，什么文件都没换，现行仍是 v0.12.35，但 Runtime
    已经停了，hold 的暂停也还在：要自己把 Runtime 起来（`install start`），再撤掉 hold（`install release-hold`）。到期限还在跑的，操作以
    `install_operation_interrupted` 留着，等它结束后 `install resume`（再核一遍）或 `install undo`。
- 没有自动回退：部署的新 Runtime 起来以后检查不过时，部署自己的自动回退先按回退算一次计划，计划的 `ledger` 边不过（见下面的退回），
  所以以 `rollback_gate_failed`（`rollback_gate_parts` 是 `interface_edges`）结束，新构建和 hold 都留着。出了问题只能往前修。
- 别的部署不改文件（按代码核过）：配置修订不变（本版仍读 5 到 23），默认值文件没变，部署计划的默认值对比没有行，没有账本或状态的迁移。
  安装命令的代码自 v0.12.35 只改了 `install undo` 的判断（INST-UNDO-EDGE，见下面的退回），`update` 和 `rollback` 判的边和 v0.12.35 的安装工具一样，只是自己的声明
  不同（见下面的配对）。
  本版作为安装工具，不再接受 `runtime-client` `max` 是 2 的现行构建（`installer_meets_previous`），也就是只能从 0.12.30 或更新的版本
  升级上来；生产是 0.12.35，不受影响。

退回 v0.12.35（按安装工具的代码核过）：

- 安装工具拒绝退回。本版声明 `ledger` [3, 5]，v0.12.35 声明 [3, 4]。`rollback plan`、`rollback apply` 的 `ledger` 边
  `previous_max_in_next` 要求现行构建的 Rp.max 落在目标的 R\* 里：5 不在 [3, 4] 里，这条边不过，两条命令都在算回退门之前就以
  `interface_edge` 拒绝，什么都不改。`--accept-ledger-one-way` 只放行升级方向的 `one_way` 边，管不了这一条。这只看两边的声明，和账本里
  有没有修订 5 的记录无关；v0.12.34 及更早的版本声明的 `ledger` 上限都是 4，一样被拒。
- 同一条边也挡住部署的自动回退（上面的 `rollback_gate_failed`），以及 `install undo`（INST-UNDO-EDGE）。用本版的安装工具，只要被撤销的构建起过 Runtime，包括组合部署
  自己在操作里起的那一次，`install undo` 就按回退判：操作做没做完都一样，先过接口边，再过整个回退门（`ledger_readability`、`open_time`、`config`），不过就以
  `interface_edge` 或 `rollback_gate_failed` 拒绝，什么都不改。没起过 Runtime 的未完成操作、普通更新、拉起之前就被拒的启动，照旧能撤销；已经过门的自动回退中断后
  照旧能撤销。组合部署成功时，如果这条边会拒绝撤销，答复不再给出 `next.undo`。
- **v0.12.35 的安装工具没有这个修复。** 部署 v0.12.36 这一次是它在答，答复里还有 `next.undo`，不要照它做；部署完以后也不要用 v0.12.35 的 `actingctl` 跑 `install
  undo`，它在安装命令之外停过 Runtime 时不判这条边，会换回 v0.12.35。本版不写修订 5 的记录，所以这样换回去读得了账本，但这绕过了单向的声明。
- 114-B 以后：安装工具的拒绝不变，它只看声明。变的只是绕过安装工具（手工放回旧程序，或用 v0.12.35 的 `actingctl` 撤销）的情形：114-B 写出第一条修订 5 的记录以后，
  v0.12.35 及更早的 Runtime 解不出它（`EventType` 没有兜底，`corrupt_ledger_record`）：这样的记录在最新的
  `ledger_observer.open_verify_tail_records` 条里时启动失败，含它的在线或离线页整页失败。在那之前账本里没有修订 5 的记录，手工放回的
  v0.12.35 读得了它，但手工放回不在保证内（测试阶段不做旧版兼容）。
- 如果没有这道边，各片要做的（只作记录）：114-A 什么都没写，不用做什么。R-4 的版本令牌只在内存里：v0.12.35 不答 `config_version`，对
  `expected_version` 答 `not_implemented`，控制台按修订规则 2 不带它再发；`host.config_overrides` 的行照常恢复（本版没有新键，不会有行
  落进 `instructions_dropped`）。R-0b 的 `activity` 只在内存里：v0.12.35 不答它，它声明的 [2, 4] 仍和 [3, 4] 的控制台配得上。R-0b2 也只在
  内存里：行里的成员消失。R-11 只在本机读：行里的新成员消失，`ui.json` 又被忽略。LN-VERIFY 和 INST-UNDO-EDGE 不落任何东西。FLAKY-LNWS1 只改测试。
- v0.12.35 一节的退回条件（SS-2d 的 `realization` 和代、R-7a、R-7a2、R-15b 没做完的 UI 暂存）是退回 v0.12.34 的条件；本版连 v0.12.35
  都退不回去，它们不适用。
- 客户端和 Runtime 的配对（`runtime-client` 本版 [3, 4]、v0.12.35 [2, 4]；`apps/actingctl` 自 v0.12.35 只改了设置读写处的
  `config_version`、`packs` 和 `ac_packs` 的行、`overview` 和 `ac_overview` 的 `activity`，以及 `install undo` 的判断，MCP 工具仍是 40 个）：
  - v0.12.35 的安装工具装本版：照常，带 `--accept-ledger-one-way`。它的声明 I 是 `runtime-client` [2, 4]，和 Rp [2, 4]、R\* [3, 4]
    都有交集（`installer_meets_previous`、`installer_meets_next`），`runtime-outcomes` 1 在它的 [1, 1] 里，本版没有它不认识的配置键
    （`installer_knows_keys`）；账本的两条边和本版的安装工具一样（见上面）。从本版的构建目录跑本版的安装工具（I 是 [3, 4]）结果相同。
  - 本版的 actingctl 和 MCP 连 v0.12.35 的 Runtime（部署切换的几秒里，或 MCP 前端还没换子进程时）：照常。v0.12.35 不答
    `config_version`，读写处就不显示它；Status 里没有 `activity`；请求都不变（本版不发 `expected_version`）。`packs` 的新成员在本机读出，
    和 Runtime 无关。
  - v0.12.35 的 actingctl 和 MCP 连本版的 Runtime：照常。它的契约 crate 从 0.12.30 起就有 `activity` 和两个 `config_version` 的形状，读得了
    本版的回答（`actingctl status` 照样打出 `activity`，它的 `overview` 和 `ac_overview` 不显示，`config set` 的回答不显示版本）；它内嵌的码目录没有新位置
    `begin_instance_activity` 和新值键 `current_version`，可它们只出现在致命故障和发了 `expected_version` 的拒绝里。
  - 修订 2 的客户端（v0.12.29 及更早的 actingctl、MCP 和 actinglab，或声明 `max` 2 的控制台）连本版：每个完成的设置写入的回执、每个
    RuntimeFactSnapshot 的回答（都带 `config_version`）和带 `activity` 的 Status 它都解不开（`RuntimeResult`、`RuntimeInstanceStatus`
    拒收不认识的成员），报 `runtime_receipt_decode_failed`；按声明，没有实例在做事时的 Status 和别的请求照常。安装时：本版作为安装工具
    不接受这样的现行构建，v0.12.29 及更早的安装工具（I 的 `runtime-client` [2, 2]）也装不了本版（`installer_meets_next`），声明 `max` 2
    的控制台过不了 `ui_meets_next`。生产里没有这样的客户端。
  - actinglab 和 Runtime 混版两个方向都照常（actinglab 本版没改，调试事件没变）。资源工具要留意：`actinglab standard-pack check` 还不查
    `ui.json`，资源侧可以用本版的 `actingctl packs` 查。
  - 控制台（UI）：UI 的 v0.12.0、v0.12.1 声明 `actingd-config` [5, 23]、`ledger` [3, 5]、`runtime-client` [3, 4]、`runtime-outcomes`
    [1, 1]，和本版每条 UI 边都配得上（三方共同的 `actingd-config` 5 到 23；Rp.max 4、R\*.max 5 都在 [3, 5] 里；[3, 4] 和 [3, 4] 有交集；
    `runtime-outcomes` 1 在 [1, 1] 里），和 v0.12.35 也配得上；它读得了 `activity` 和 `config_version`。按修订规则 2，从本版起控制台写设置时
    总带 `expected_version`，过期的被拒、什么都不改，它从拒绝里的 `current_version` 拿到现在的版本再发。按代码看，UI 的 `acsetup` 只认 `ledger-maintenance.v1` 的校验报告和
    `verified-sqlite`、`verified-segment`，Runtime 的报告早已是 v2，所以那个门现在就不通过，不是本版带来的；已转 UI 处理。生产用 `actingctl install` 部署，不走它。

运行上的变化：

- 账本声明 [3, 5]（114-A）：部署要带 `--accept-ledger-one-way`，计划多一步 `ledger_verify`，装上后退不回 v0.12.35；本版不写修订 5 的记录。
- 账本校验（LN-VERIFY）：不带标志的 `ledger-maintenance verify` 做启动时的有界检查，报告 `verified-sqlite-bounded`；完整读要带 `--full`。
- 撤销（INST-UNDO-EDGE）：起过 Runtime 的构建，`install undo` 过接口边和整个回退门，不过就拒绝、什么都不改。
- 实例现在在做什么（R-0b、R-0b2）：`actingctl status`、`actingctl overview` 和 `ac_overview` 的实例行带 `activity`（七种，只在内存里）；`runtime-client` 声明 [3, 4]。
- 设置的版本（R-4）：设置的读写处都答 `config_version`；带过期 `expected_version` 的写入被拒、什么都不改；`ui_tier` 给控制台分档。
- 包列表（R-11）：组合出的安装行多 `task_ids`、`irreversible`，游戏根带 `ui.json` 时多 `ui`、`display`；只在本机读，不升修订。
- 测试（FLAKY-LNWS1）：账本语句记录器按线程各记各的，`ln_ws_t1` 偶发的空记录修好了；生产代码不变。

本版合并的 PR：

- #1469 114-A（#386）：「为什么没跑」的记录 `policy.not_run_changed` 的形状、账本自己的副本、两条索引前缀和全部词表，读侧先行；声明 `ledger` [3, 5]，单向。本版不写任何记录，写侧在 114-B。
- #1475 R-4（#429）：`ConfigureSettings.expected_version` 是比较交换（不一致时 `fact_observation_version_conflict` 带 `current_version`，codes 模式也保留）。回执和各读取处带 `config_version`。一次提交只改它点名的键。`ui_tier` 定下 daily 8 项、advanced 6 项。
- #1474 R-0b（#429）：Status 的 `activity` 七种一次填全，只在内存里。`runtime-client` 声明 [3, 4]；作为安装工具，不再接受 `runtime-client` `max` 是 2 的现行构建。
- #1473 R-11（#429）：包根可选的 `ui.json`（只查形状、上限和引用）；`actingctl packs` 和 `ac_packs` 离线给出 `ui`、`display`、`irreversible` 和安装行的 `task_ids`，不升修订。
- #1476 FLAKY-LNWS1（#422）：只改测试设施：账本的语句记录器按线程各留一份；生产代码不变。
- #1478 R-0b2（#429）：`actingctl overview` 和 `ac_overview` 的实例行带 `activity`，来自 overview 本来就读的那一次 Status，MCP 的输出预算不截它。
- #1480 LN-VERIFY（#422）：`ledger-maintenance verify` 默认做启动时的有界检查（`verified-sqlite-bounded`），完整读挪到 `--full`；部署的 `ledger_verify` 因此不再整本读进内存。
- #1479 INST-UNDO-EDGE（#421）：`install undo` 不能绕开回退要过的接口边和回退门；组合部署成功时，会被这条边拒绝的撤销不再出现在 `next.undo` 里。

## v0.12.35

本版是 v0.12.34 之后合并的 4 个 PR：SS-2d、R-7a、R-7a2、R-15b。调度停着（每台实例都暂停）的时候，运行上只有一处变化：一台实例
同时有自己的暂停和全局暂停时，显示的原因是实例暂停。别的变化要等调度恢复，或者有了新请求、新写法、新文件才看得到。
一是控制门只有一个顺序（R-7a2）：评估、事实刷新、准入和时钟槽都按「玩家持有 → 实例暂停 → 全局暂停 → 资源围栏」取原因，玩家持有
这一维在 R-7b 之前恒为假。两种暂停同时在时，候选的原因和准入的拒绝码都是 `dispatch_paused_instance`，不再是
`dispatch_paused_global`。现在每台实例都带着自己的暂停，所以加上或撤下全局暂停，`actingctl schedule`、`ac_schedule` 和
`ac_overview` 未运行一行的 `why` 都不变，`since_unix_ms` 也不再重新计时。
二是净收益落在取整误差以内的候选不再按正负号派或挡（SS-2d）：调度恢复、池写了估值之后才看得到。被判候选的净收益 S（取整前）落在
它自己的误差界 ε 以内（|S| ≤ ε）时，记 `benefit_within_tolerance`（info），不当作正收益派出，也不当作负收益一直挡住；带模型核实的
`realization` 标记的兑现或转换任务，在业务档的最后一段放行。没有候选落进零带的目录，决定、排序和记录都逐字节和以前一样。
三是玩家持有的两种请求有了形状（R-7a）：`AcquirePlayerHold`、`ReleasePlayerHold` 在 ui、cli、mcp、lab 四个通道上都答
`not_implemented`，什么都不记，Status 也不变；本版的 actingctl、MCP 和 actinglab 都不发它们。持有的行为在 R-7b。
四是安装工具认得 UI 的发布（R-15b）：生产照旧是单根，A/B 路径都不运行。`actingctl install release verify` 和 `stage` 遇到目录里
认不出的 zip 时以 `zip_set` 拒收并列出名字，以前是跳过它；Release 工作流的发布目录只有两个 zip 和 `SHA256SUMS`，部署不受影响。
v0.12.34 已在本版之前发布。

配置：`runtime-client` 从 [2, 3] 改为 [2, 4]（R-7a，`min` 仍是 2）；`actingd-config` 仍是 [5, 23]，账本仍是 [3, 4]，`package` 仍是
[3, 5]，`install-selection` 仍是 [1, 1]，其他接口也不变。

- 没有新配置键：配置键表和 `actingd.defaults.json` 自 v0.12.34 都没改，在线可改的键不变，程序事实 `config.parameters`、
  `config.subsystems` 都没有新行。四个 PR 都不带配置键。
- `runtime-client` 4 是 3 加上玩家持有的形状：两种请求和它们的结果，Status 实例行的 `player_hold`、`player_hold_operation`、
  `control`，启动报告的 `player_holds_restored`、`player_holds_dropped`、`player_hold_not_durable`。本版一个新成员都不填（空时不
  出现），所以修订 3 的客户端读得了本版的每个回答。请求 v4、回执 v2 的 schema 字符串不变，Runtime 分不出客户端的修订，只有安装时
  核配对；R-7b 第一次填 Status 的新成员时才把 `min` 升到 4。
- 契约写下、本版还不声明的接口（`component-interfaces.json` 里没有它们）：R-15b 冻结的 `install-selection` 2（A/B 选择 v2 和槽
  文档 `SLOT.json`；契约 crate 能解析、校验，没有程序读它，R-15c 起声明）、`tools-layout` 4（两个固定入口程序，R-15c 起；本版的
  Tools 包仍是 `platform-tools-v3`）、新接口 `install-cli` 1（R-15d 起）和私有接口 `install-root` 1（R-15c 起），以及 R-7a 写下的
  `player-hold` 1（R-7b 起）。
- 新结果码 47 个，都还是待审（pending）：SS-2d 2 个（`benefit_within_tolerance`，info；目录诊断 `realization_cycle`，error）；
  R-7a 29 个（runtime-host 的 16 个 `player_hold_*` 和 `dispatch_player_held`、`dispatch_resource_fenced`，契约的 8 个，
  `runtime-client` 的 3 个）；R-15b 16 个（新 owner `actingcommand-entry` 的 10 个 `entry_*`，契约的 5 个 `install_*`，actingd 的
  `unlock_owner_install_unfinished`）。R-7a2 不加码，它是 `dispatch_resource_fenced` 的第一个写者。新位置 7 个（R-7a 2 个、R-15b
  5 个），新值键 47 个，新词表 49 个；已有词表加词：`emulator_autostart_skip_reason` 的 `player_held`（R-7b 才记），
  `install_refusal` 28 个、`install_caveat` 19 个、`install_guard` 5 个、`release_check` 1 个（`selection_schema_mismatch`）。已有的码
  `install_command_refused` 多一个可选值 `attempt_phase`，另有 7 个码只改了说明。结果码目录从 3822 个码变成 3869 个，位置从 1725 个
  变成 1732 个；没有新定稿或退役的条目，所以本版的发布文件是空的。
- 账本没有新表、新索引、新事件类型或新形状，`ledger` 不升；状态文件的形状也不变。控制门的顺序和零带的判断都只在内存里；Runtime 写的
  新码（`benefit_within_tolerance`、`dispatch_resource_fenced`）只进已有的原因链、拒绝记录和时钟槽记录。
- 目录格式多两种写法，格式版本不变（SS-2d）：V2 任务成员 `benefit_realization` 和 `schedule.json` 规则成员 `realization`；
  v0.12.34 及以前的 Runtime 拒收它们（见下面的退回）。资源工具的 `benefit_coverage` 多一个计数 `realization`。
- 安装（R-15b）：`state.json`、日志和 Runtime 构建的 `release.json` 都不变。新 workspace 成员 `apps/actingcommand-entry` 现在只是放
  码的库，不出程序，所以 Runtime 包和 Tools 包的文件清单和 v0.12.34 一样。

升级时要知道的：

- 部署不改文件（按代码核过）：`actingctl install` 只换程序文件，不碰配置文件；配置修订不变（本版仍读 5 到 23），默认值文件没变，
  部署计划的默认值对比没有行。没有账本或状态的迁移。配对：安装工具按 `runtime-client` 核 I ∩ Rp、I ∩ R\* 和 UI 的 U\* ∩ R\*，
  本版的 [2, 4] 和 v0.12.34 的 [2, 3]、声明到修订 3 的 UI 都有交集，不挡部署。
- 发布目录（R-15b）：本版的 `release verify`、`release stage` 遇到目录里任何不是 `actingcommand-runtime-<sha>.zip`、
  `actingcommand-tools-<sha>.zip` 或 `acui-windows-<sha>.zip` 的 `.zip`，以 `zip_set` 拒收整个目录，每个这样的名字记一条失败（`file`
  是它的名字）；UI 的 zip 和 Runtime 的 zip 放在一起也拒。下载 Release 时放进一个只有它那三个文件的新目录。不是 zip 的文件照旧不看。
- UI 发布的暂存（R-15b，新能力）：只有 `acui-windows-<sha>.zip` 和 `SHA256SUMS` 的目录是 UI 发布。`release verify` 和 `stage` 核它
  的来源（facts 的仓库、清单的提交、文件集和摘要、按解析规则读的声明，`installation_selection_schema` 要和 `install-selection` 的
  范围一致，否则 `selection_schema_mismatch`），`stage` 分三步把它放进 `install\ui-builds\<sha>\`（`ui` 和 `release.json`，
  `member: "ui"`），不写 `state.json`。R-15c 之前没有程序读它。
- 控制门的顺序（R-7a2）：位置 1 拆成 1a 到 1d：玩家持有、实例自己的暂停（任一阶段）、全局暂停、资源围栏，之后才是实例的队列、性能门
  和容量门。准入在准入锁下现读同一个顺序：两种暂停同时在时拒 `dispatch_paused_instance`；被围栏实例的、围栏之前评估好的意图拒
  `dispatch_resource_fenced`（info / none，值 `instance_id`），以前是排在性能门之后的 `instance_resources_fenced`（error /
  environment_fault），所以按错误类别数的行会少。评估时被围栏实例的候选是 `Deferred`、`dispatch_resource_fenced`，不设唤醒；但宿主
  自检设备的实例（所有模拟器实例）被围栏时，策略可用性也一起撤下（#403），候选照旧在放置阶段以 `instance_unavailable` 挡住，到不了
  位置 1d，和以前一样：1d 只对可用性没被撤下的实例和准入起作用。到期的时钟槽碰上围栏，第一次看到就记 `clock_slot_deferred`
  （`gate_code` 是 `dispatch_resource_fenced`）。事实刷新的挡住和它的 `gate_code` 取同一个原因。设备请求碰上围栏、围栏自己的记录照旧
  是 `instance_resources_fenced`；恢复阶梯只看有没有暂停，不变。
- 零带（SS-2d）：ε 由求和里的两处取整算出（导出的权重、每个期望量），和 S 一样按精确分数累计，消耗一侧读 SS-2g 的千分之一单位。
  |S| ≤ ε 记 `benefit_within_tolerance`，值 `benefit_milli`（⌊S⌋）、`tolerance_milli`（⌈ε⌉）和 `zero_tier`；零带以外照旧：
  `B = ⌊S⌋ > 0` 为正，其余已知的 B 是 `policy_benefit_nonpositive`（S 大于 ε 而 B 为 0 的也算）。`zero_tier` 是 `released` 时放行：
  任务带 `benefit_realization`、B 已知、至少一个产出池在本实例上权重大于 0、本实例最近一次运行没有失败；放行的候选排在业务档里所有
  未判的和所有正收益的候选之后（跨实例），升档也抬不上去，保留门、共享运行位、暂停和持有、SS-4 的无进展护栏照旧在后面检查。其余的
  延后（`unmarked`、`no_downstream_value`、`latest_failed`），像不为正的候选一样在再生池的下一步醒来。空闲计划不把没被选中的零带
  决定算作到期，也不算它自己的唤醒，不会为它把关着的游戏叫醒。
- `realization` 标记（SS-2d）：只由 `acmodel bindings apply` 按模型的结论写在 `schedule.json` 的规则上（`"realization": true`，只认
  `true`，不能和 `"maintenance": true` 同写），组合器在被判的规则上写 `benefit_realization: true`；规则自己写 `benefit_realization`、
  维护角色包的规则写 `realization` 都拒收（`schedule_invalid`）。编译器要求它在 `benefit_judged` 任务上、至少一条 `consumes` 和一条
  `produces`、每个消耗池有保留线并由事实读数观测、带标记的任务按「消耗池 → 产出池」不成环（`realization_cycle`）；V1 写它是
  `unknown_field`。资源侧要等所有部署都在 0.12.35 或以上之后才写它（v0.12.34 的 Runtime 拒收，见下面的退回）。
- 玩家持有的请求（R-7a）：两种请求只做契约的检查（实例别名、`drain_timeout_ms` 1000 到 600000、`expected` 的形状、来源闸门
  User/Ui、Cli/Cli 或 Lab/Lab），然后答 `not_implemented`（位置 `acquire_player_hold`、`release_player_hold`），不记账、不改
  Status。`runtime-client` 多了 prepare / submit 两步的方法、回执读截止和丢了回执时按一次 Status 结算（`runtime_player_hold_settled`、
  `runtime_player_hold_unconfirmed`、`runtime_operation_unsupported`），给以后的控制台用。

退回 v0.12.34 之前要做的事（按 v0.12.34 的代码核对过）：

- SS-2d：账本、配置和状态文件都不用处理。`benefit_within_tolerance` 和它的值只在已有的原因链里（放行的派单的原因链里有它，延后的
  可能是相关候选的 `why`），v0.12.34 照常读：码按原拼写读回、不查码，值按映射保留、不查键。目录要先处理：v0.12.34 的任务结构拒收
  不认识的成员，`benefit_realization` 是 `unknown_field`；它的 `schedule.json` 读取器不认 `realization`，把它留在规则里、原样抄进
  组合出的任务，同样是 `unknown_field`，报成那条规则的 `schedule_invalid`。而 v0.12.34 启动时要重新编译启用的那一代、每个未了结
  派单钉住的代，以及决策记忆点名的每次派单所在的代（每个未了结的派单、每对（任务，实例）最近一次完成的计划派单、补救任务已准入的
  运行），编译不过就以 `catalog_generation_invalid` 致命地停下，整个 Runtime 起不来。所以回退前先在 0.12.35 上把 `realization` 从
  各游戏根的 `schedule.json` 里拿走，等下一次工作台检查启用了不带这个成员的代（程序事实 `config.catalog` 换成了新的代），再确认没有
  未了结的计划派单（调度暂停并排空）。看的是整代：在带这个成员的代下派过、完成过计划运行的每一对，都要先在新的代下再完成一次派单，
  换掉决策记忆点名的那一次，否则 v0.12.34 启动照样停下；规则已经拿走、不会再派的对换不掉，这时不能退回。调度一直停着（每台实例都
  暂停）时没有这样的派单，只要启用的代不带这个成员就能退回。
- R-7a：不用做什么，按代码核过：什么都没落盘，也没有行为要撤（本版不填任何新成员，两种请求不记账）。已经升到 `runtime-client`
  修订 4、会发这两种请求的控制台（本版的 actingctl、MCP 和 actinglab 都不发）连 v0.12.34 时：v0.12.34 解不开不认识的请求种类
  （`runtime_protocol_invalid`，位置 `runtime_request_decode_failed`），断开这条连接，不回回执；本版的客户端在同一个 owner epoch
  里重连、读一次 Status，没有实例带持有的成员，按 A7、R8 报 `runtime_operation_unsupported`，句柄照常可用，什么都没做成。v0.12.34
  声明 [2, 3]：声明 [2, 4] 或 [3, 4] 的 UI 照常配对，只有 `min` 是 4 的 UI（R-7b 起才会有）过不了安装工具的 `ui_meets_next`。
- R-7a2：不用做什么，按代码核过：什么都没落盘，形状也没改。退回后 v0.12.34 又是全局暂停在先（两种暂停同时在时原因是
  `dispatch_paused_global`，加上或撤下全局暂停时 `why` 和 `since_unix_ms` 又会变），准入又在性能门之后以 `instance_resources_fenced`
  拒被围栏的实例。本版写下的 `dispatch_resource_fenced` 只在已有的原因链、拒绝记录的结果和时钟槽记录的 `gate_code` 里，v0.12.34 照常
  读：码按原拼写、值按映射，暂停报告把时钟槽记录的 `gate_code` 当任意收到的码（`CodeStr`）读；时钟槽门类的改名只在内存里。
- R-15b：落盘的形状都没变：`state.json`、日志和 Runtime 构建的 `release.json` 和以前一样；`install\ui-builds\` 下暂存的 UI 构建是
  v0.12.34 从不读的目录。只有一点：没做完的 UI 暂存（三步的 `release_stage` 日志：解包、核树、放置，目标在 `install/ui-builds/`
  下）v0.12.34 续不了：它的 `install resume` 在目录里找不到成对的 Runtime 和 Tools zip，以 `zip_set` 拒绝；但它的 `install undo`
  照常撤销（把解包的部分放到一边记 `abandoned`，已放置的 UI 构建放到一边记 `rolledback`，什么都不删）。没做完的操作还会挡住别的
  安装操作，回退的 `rollback apply` 也一样，所以回退前先用本版的 `actingctl install resume` 做完它，或者用 `install undo` 撤掉它，
  再照常用单根的 `rollback`。
- v0.12.34 一节的退回条件（PAUSE-AUTO 不用做什么；SS-4 的配置成员和在线覆盖；SS-2g 的期望消耗和代）是退回 v0.12.33 的条件，从本版
  退回 v0.12.34 时不适用：v0.12.34 自己不开暂停中实例的模拟器、认得 `scheduling.no_progress`、读得了消耗上的
  `expected_amount_milli`，本版也没改这些。要再往前退到 v0.12.33，先做上面四条（v0.12.33 对它们的做法和 v0.12.34 相同：任务结构和
  组合器、请求的解码、暂停的先后和准入的围栏、安装命令的发布读取都一样），再照 v0.12.34 一节的退回条件做；本版的记录 v0.12.33 也
  读得了（它读结果的代码和 v0.12.34 相同）。
- 客户端和 Runtime 的配对（`runtime-client` 本版 [2, 4]、v0.12.34 [2, 3]；`apps/actingctl` 自 v0.12.34 只改了安装命令的发布读取和
  码表，MCP 工具仍是 40 个）：
  - 本版的 actingctl 和 MCP 连 v0.12.34 的 Runtime：照常。没有新命令、新工具，也从不发两种持有请求；它内嵌的码目录认得 v0.12.34
    的每个码。安装命令的发布读取在本机做，按本版的规则（认不出的 zip 拒 `zip_set`，认得 UI 发布）。
  - v0.12.34 的 actingctl 和 MCP 连本版的 Runtime：照常，请求不变，回答里没有新成员（Status 和启动报告的新成员都不出现）。读面里
    可能出现新码 `benefit_within_tolerance`（带 `tolerance_milli`、`zero_tier`）和 `dispatch_resource_fenced`，旧客户端照原样显示码
    和值；但它内嵌的码目录没有本版的 47 个新码，`actingctl codes`、`ac_outcome_code` 查它们答 `outcome_code_unknown`，个别按码分类
    显示的地方把它们当作未登记的码（`outcome_relay_unregistered`，原拼写在 `raw_code`）。v0.12.34 的安装命令照旧跳过认不出的 zip，
    也暂存不了 UI 发布。
  - actinglab 和 Runtime 混版两个方向都照常（调试事件没变）。资源工具要留意：本版的 `actinglab standard-pack check`、
    `package bundle --schedule` 接受 `realization` 并报 `realization_cycle`，它放行的带标记的规则文件 v0.12.34 的 Runtime 不收；
    v0.12.34 的资源工具拒收这个标记。
  - 控制台（UI）：声明到修订 3 的 UI 和本版、v0.12.34 都配得上；会发修订 4 请求的控制台（R-7b 起）连 v0.12.34 得
    `runtime_operation_unsupported`（见上面 R-7a）。

运行上的变化：

- 两种暂停同时在时显示实例暂停（R-7a2）：原因和拒绝码是 `dispatch_paused_instance`，加减全局暂停时 `why` 和 `since_unix_ms` 不变；被围栏实例的意图在准入拒 `dispatch_resource_fenced`。
- 净收益在取整误差以内的候选进零带（SS-2d）：调度恢复、池有估值之后记 `benefit_within_tolerance`，带 `realization` 标记的在业务档最后放行。
- 玩家持有的两种请求（R-7a）：形状和码都有了，Runtime 答 `not_implemented`；`runtime-client` 声明 [2, 4]。
- 发布读取（R-15b）：`release verify`、`stage` 拒收认不出的 zip（`zip_set`），能暂存 UI 发布；生产照旧是单根。

本版合并的 PR：

- #1465 SS-2d（#386）：被判候选的净收益落在它自己的取整误差 ε 以内时既不为正也不为负（`benefit_within_tolerance`）；模型核实、带 `realization` 标记的兑现任务可以在业务档的最后一段放行，受保留线、最近一次成功和 SS-4 约束。不碰零带的目录逐字节和以前一样决定、排序和记录。登记簿 §1.232 的回退条件改成按整代说。
- #1466 R-7a（#429）：玩家持有冻结的契约和形状：契约 crate 的类型和检查、`runtime-client` 的方法和结算、`contracts/player-hold.md`。Runtime 在每个通道上都答两种新请求 `not_implemented`，什么都不记。`runtime-client` 声明 [2, 4]。
- #1467 R-7a2（#429）：派单的每一处都按同一个控制门顺序：玩家持有（R-7b 之前恒为假）、实例暂停、全局暂停、资源围栏。两种暂停同时在时原因是 `dispatch_paused_instance`；准入以 `dispatch_resource_fenced` 拒被围栏的实例。宿主自检的模拟器实例被围栏时，放置阶段照旧显示 `instance_unavailable`（#403）。
- #1468 R-15b（#429）：A/B 安装的契约文本、`contracts/stable-entry.md` 和安装选择 v2 的契约类型，还没有程序调用它们。发布读取器认得 UI 发布的 zip，认不出的 zip 以 `zip_set` 拒绝。

## v0.12.34

本版是 v0.12.33 之后合并的 3 个 PR：PAUSE-AUTO、SS-2g、SS-4。运行中会注意到的变化有三件，第一件部署后就生效，后两件要等调度恢复
或资源侧写了新写法才看得到。
一是暂停中的实例，模拟器不再被自动开起来（PAUSE-AUTO）：实例自己的调度暂停期间（`draining`、`paused`、`released` 任一阶段，重启后
恢复的暂停也算），守护进程启动时按配置开模拟器（`start_emulator`，部署和看门狗拉起的守护进程启动也算）、IDLE-1 在 `stop_emulator`
之后的唤醒都不再开它的模拟器，改记一条 `emulator_autostart_skipped`（`skip_reason` 是 `instance_paused`）；暂停开始前已经排下、还在
等钥匙的启动，在真正开机前再查一次暂停，遇到就什么都不开。所以调度停着（每台实例都暂停）的时候，部署或守护进程启动之后，关着的
模拟器保持关着，被空闲动作关掉的模拟器也不会被唤醒开起来。要做手动运行或 Lab，照旧先用模拟器控制（`actingctl emulator`、
`ac_emulator`）把它开起来：人、代理和 Lab 的显式模拟器控制不看暂停，和以前一样。全局暂停不挡这些自动启动，也和以前一样。
二是成功却没有进展的任务不再一遍遍重跑（SS-4）：调度停着的时候什么都不变（不派单，护栏也不积压）。调度重新跑起来之后，一个任务
成功跑完，而它被派出时依据的事实（依据键）自准入以来没有一个读到不同的值，这一对（任务，实例）就以 `policy_no_progress` 等着，不再
每个冷却期跑一次，直到某个依据键读到不同的值。失败的运行照旧走重试和升级。开关是新键 `scheduling.no_progress.enabled`，默认开。
三是目录可以写期望消耗（SS-2g）：V2 的 `consumes` 条目可以像产出一样写 `expected_amount_milli`（千分之一单位）。资源侧写之前什么都
不变，没有期望消耗的目录逐字节和以前一样评估。v0.12.33 已在本版之前发布。

配置：`actingd-config` 从 [5, 22] 改为 [5, 23]；`runtime-client` 仍是 [2, 3]（`min` 仍是 2），账本仍是 [3, 4]，`package` 仍是 [3, 5]，
其他接口也不变。

- 新键（SS-4）：`scheduling.no_progress.enabled`，布尔，默认 `true`，可在线改（`actingctl config set|clear`、`ac_config_set`），每次
  评估都读，下一次评估就按新值。也可以按实例设：`instance.<实例 id>.scheduling.no_progress.enabled`，在配置文件里是 `instances[]`
  条目的 `scheduling.no_progress.enabled`，默认跟节里的值，也可在线改。某台实例关掉时，它的评估和本版之前逐字节相同，它的护栏也
  清空，所以关了再开就放掉这台的全部护栏。`actingd-config` 升到 23 只因为这两个成员；不写它们的配置文件保持原来的修订，照常读。
  别的键都没改，PAUSE-AUTO 和 SS-2g 没有配置键；在线可改的键只多这两个。
- `actingd.defaults.json` 自 v0.12.33 以来只多了 `scheduling.no_progress.enabled`（`true`）一项，别的默认值都没改；部署计划的默认值
  对比会列出它（目标有、现装没有）。程序事实 `config.parameters` 里多这个键（和每台实例的同名键）；`config.subsystems` 没有新行。
- 新结果码 2 个，都还是待审（pending）：`policy_no_progress`（info，SS-4）和 `standard_pack_escalation_threshold_high`（warning，
  SS-4，只由资源工具报告）。结果码目录从 3820 个码变成 3822 个，位置仍是 1725 个；没有新定稿或退役的条目，所以本版的发布文件是
  空的。新值键 3 个：`basis_readings`（记录列表）、`reading_value`、`reading_digest`；没有新词表。已有的码 `emulator_autostart_skipped`
  多一个可选值 `idle_action`（键已有）。
- 账本没有新表、新索引、新事件类型或新形状，`ledger` 不升；状态文件的形状也不变（被挡下的唤醒在 `host.idle_state` 里就是普通的
  关闭行）。护栏和跳过的记忆只在内存里。
- 目录格式多一种写法，格式版本不变：V2 `consumes` 条目上的 `expected_amount_milli`（SS-2g）；v0.12.33 及以前的 Runtime 拒收它
  （见下面的退回）。

升级时要知道的：

- 部署不改文件（按代码核过）：`actingctl install` 只换程序文件（包括程序自带的 `actingd.defaults.json`），不碰配置文件；本版读修订
  5 到 23，不写 `scheduling.no_progress` 的配置文件保持原来的修订，目标的 `check-config` 照常通过，新键取默认值。没有账本或状态的
  迁移。
- 部署时的模拟器（PAUSE-AUTO）：部署、看门狗或人启动守护进程时，暂停中、模拟器关着的实例不建认领，不计入
  `emulator_autostart_queued`，每台记一条 `emulator_autostart_skipped`（`skip_reason=instance_paused`，不带 `watch_kind`），同时是
  实例下的一条 `device_state_observed` 记录和一行报告。IDLE-1 在 `stop_emulator` 之后的唤醒，暂停期间不排：实例保持关着、欠着这次
  唤醒，每个暂停 revision 只记一条带 `idle_action=stop_emulator` 的跳过，不管多少次空闲检查发现它到期。暂停开始前已经排下的认领
  （守护进程启动的、唤醒的、监视的重启和识别）在持钥等待的每一拍都再问一次暂停，最后一次就在开机之前：遇到暂停就放掉钥匙，不写
  `command.*`、不写失败对、不开机、不绑定，记同样的跳过（监视的带 `watch_kind` `restart` 或 `recognize`）；唤醒的认领这样结束后，
  这次唤醒又记为欠着；监视的这次请求退还它在 `emulator_control.restart_attempts` 里占的名额。每种启动、每个暂停 revision 只记一次。
  `close_app` 的唤醒（只排启动包，不开模拟器）、全局暂停、时段黑名单、空闲关机本身、恢复阶梯（每一步前本来就查暂停）和没有暂停的
  实例都不变。v0.12.32 一节「没变的」里写的「守护进程启动时按配置开模拟器、IDLE-1 的唤醒，以及暂停开始前已经排下的监视认领都
  不看暂停」，从本版起不再成立。
- 恢复之后（PAUSE-AUTO）：下一次空闲检查照常排欠着的唤醒，监视下一次轮询照常重启或识别。守护进程启动不会在恢复时重跑：它在暂停
  期间没开的模拟器，由恢复后监视的第一次轮询开（`emulator_control.watch_enabled` 开着时），否则要人去开。
- 怎么读 `policy_no_progress`（SS-4）：在 `actingctl schedule`、`ac_schedule` 和 `ac_overview`、`ac_diagnose` 的未运行一行里，被
  护栏挡住的一对最后的原因是 `policy_no_progress`，带 `served_by_dispatch_unix_ms`（那次成功运行的派单时刻）和 `basis_readings`：
  每个依据键一条，`fact_key` 加上准入时的读数（`reading_value` 是它的规范 JSON，不超过 128 字节，更长的改给 `reading_digest`，即
  SHA-256）和 `observed_at_unix_ms`，当时没有读数的键只有 `fact_key`；最多 32 条。它不派、不设自己的唤醒，空闲计划不把它算作到期，
  不会为它把关着的游戏叫醒。某个依据键读到和准入时不同的值（原来没读数、现在有了也算），护栏就放开，之后一直放着；同值的新读数
  （例如读数刷新又读到同一个数）不算进展，被重置清掉的键什么都不放。派单的决策记录里，它作为相关候选
  （`decision_candidate_not_selected`）的 `why` 出现。
- 依据键是什么（SS-4）：按选中这次派单的那次评估重读触发条件，只取让它为真的部分（`all` 的每个子项、`any` 里第一个为真的子项、
  `not` 下的全部事实）里的 `resource.`、`inventory.` 事实、每个 `fact_time_reached` 的键和 `resource_projection` 投影的池事实，再加
  任务 `reserves` 的池读数和容量事实。锁、健康标志、会话事实、反馈停止条件、只出现在 `consumes` 或 `produces` 里的池都不算。只由
  时钟、时间线、结果或依赖决定的派单没有依据，也就没有护栏；超过 32 个键的也不设。失败或取消的运行、立即重跑、补救请求和重试、
  欠下的重跑都不受护栏影响。护栏只在内存里：Runtime 启动时清空（每一对最多多跑一次，下一次成功后再武装），新的目录代放掉它决定的
  那些对的护栏，关掉开关也放掉这台的。
- 资源工具的新警告（SS-4）：`actinglab standard-pack check` 和 `package bundle --schedule` 对没有日界（没有 `loop_budget`，或其中
  没有 `daily_limit`）、`on_failure.escalation_threshold` 又大于 10 的规则报 `standard_pack_escalation_threshold_high`（warning，值
  `game`、`task_id`、`count`、`limit_count` 10），只报不拒，规则照常编排。护栏只管成功的运行；这样的规则一直失败时，要失败这么多次
  才暂停这一对，所以资源侧去掉 `daily_limit` 时，把 `escalation_threshold` 一起改成 2 或 3。
- 期望消耗（SS-2g，只在 V2）：`consumes` 条目写 `expected_amount_milli`（整数 `0..=9007199254740991`，每次运行的千分之一单位）代替
  `amount`；不能超过池的常量容量 × 1000（超过报 `effect_incompatible`）；和 `amount` 或 `"pending"` 同写、或写在 V1 里，照旧是
  `limit_exceeded`。净收益按千分之一精确计成本（`C = Σ amount × 1000 + Σ expected_amount_milli`）；其余按整数读消耗的地方一律读
  `⌈C/1000⌉`：保留视图和保留门、读数还没显示的消耗、SS-2 的平手键和选择字段 `task.consumes.<池>`。保留了按期望消耗的池的任务，
  派单不带 `run_cap`（`spend_allowance` 照常）；前向投影遇到期望条目照旧以 `effect_evidence_insufficient` 停下。没有新码、新值或
  新键。资源侧要等所有部署都在 0.12.34 或以上之后才写它（v0.12.33 的 Runtime 拒收，见下面的退回）。

退回 v0.12.33 之前要做的事（按代码核对过）：

- PAUSE-AUTO：不用做什么，按代码核过：没有新的账本、状态、目录或配置形状；本版写的跳过记录 v0.12.33 读得了（码和 `idle_action`
  键都是已有的，结果读回时值按映射保留、不查键），跳过的记忆不落盘，被挡下的唤醒在 `host.idle_state` 里是普通的关闭行。退回后
  v0.12.33 又不看暂停：暂停中、配了 `start_emulator`、模拟器关着的实例，在守护进程启动时就被开起来；被空闲动作关掉模拟器、欠着
  唤醒的实例，在空闲检查发现唤醒到期时被开起来；暂停前排下的监视认领照样开机或绑定，也照样占监视的名额。
- SS-4：账本和状态文件不用处理，护栏只在内存里；退回后没有护栏，被挡住的对又按写着的次数和冷却成为候选。`policy_no_progress` 在
  账本里只作为派单决策记录里相关候选的 `why`（以及未列出候选的 `omitted_why` 计数）出现，v0.12.33 照常读：码按原拼写读回、不查码，
  值按映射保留、不查键；`basis_readings` 不出内存。配置：v0.12.33 的 `scheduling` 节和 `instances[]` 条目的 `scheduling` 成员都拒收
  不认识的成员，所以配置文件写了 `scheduling.no_progress`（节里，或某个 `instances[]` 条目里）时，v0.12.33 启动和 `check-config`
  以 `config_decode_failed` 拒收整个文件。acsetup 和安装器不写它；有人手写过的，回退前先把这些成员删掉并提交
  （`acsetup --commit-config`），再用 v0.12.33 的 `check-config` 核一遍。在线覆盖（`scheduling.no_progress.enabled` 和每实例的键）：
  v0.12.33 没有这些键，启动恢复时逐行以 `config_override_dropped` 丢掉、列进 `instructions_dropped`，不挡启动；但部署工具回退启动后
  的 `instructions` 检查因此不通过，回退前先 `actingctl config clear <键>` 把它们清掉，或者接受这一项不过。
- SS-2g：账本和配置不用处理。v0.12.33 的编译器拒收 `consumes` 上的 `expected_amount_milli`（条目处 `limit_exceeded`），而它启动时
  要重新编译启用的那一代，以及决策记忆点名的每次派单所在的那一代（每个未了结的派单、每个任务和实例这一对最近一次完成的计划派单、
  补救任务已准入的运行），编译不过就以 `catalog_generation_invalid` 致命地停下，整个 Runtime 起不来。所以回退前先在 0.12.34 上让它
  启用一个不带这种条目的代：把这类条目（或它的规则）从各游戏根的 `schedule.json` 里拿走，等下一次工作台检查启用了新的代（程序事实
  `config.catalog` 换成了新的代），再确认没有未了结的计划派单（调度暂停并排空）。看的是整代，不只是写了期望消耗的那个任务：在带这种
  条目的代下派过、完成过计划运行的每一对（任务、实例），都要先在新的代下再完成一次派单，把决策记忆点名的那一次换掉，否则 v0.12.33
  启动照样停下；规则已经拿走、不会再派的对换不掉，这时不能退回。调度一直停着（每台实例都暂停）时没有这样的派单，只要启用的代不带
  这种条目就能退回。
- v0.12.33 一节的退回条件（LAB-OBS-1 要 Runtime 包和 Tools 包一起退回；SS-2e 不用做什么）是退回 v0.12.32 的条件，从本版退回
  v0.12.33 时不适用：Lab 调试事件（`RuntimeDebugEvent`）、actinglab 和 Lab 的代码自 v0.12.33 都没改。v0.12.32 一节的条件（SS-2b 的
  标记和代、SS-5 和 SS-2c 的成员、WATCH-P）是退回 v0.12.31 的条件，也不适用。要再往前退到 v0.12.32，先做上面 SS-4 和 SS-2g 两条
  （v0.12.32 同样不认 `scheduling.no_progress` 和消耗上的期望值，也不看暂停），再照 v0.12.33 一节的退回条件做；本版的记录 v0.12.32
  也读得了（它读结果的代码和 v0.12.33 相同）。
- 客户端和 Runtime 的配对（`runtime-client` 两边都是 [2, 3]；actingctl、actinglab 和 `runtime-client` 的代码自 v0.12.33 没改）：
  - 本版的 actingctl 和 MCP 连 v0.12.33 的 Runtime：照常。没有新命令、新请求或新工具（MCP 工具仍是 40 个）；`config set` 两个新键
    被 v0.12.33 以 `config_key_unknown` 拒绝。
  - v0.12.33 的 actingctl 和 MCP 连本版的 Runtime：照常，请求不变；键存不存在由 Runtime 回答，所以它的 `config set` 也能设两个新键。
    `ac_schedule` 里会出现 `policy_no_progress`（带 `basis_readings`），`emulator_autostart_skipped` 多一个值 `idle_action`，旧客户端
    照原样显示码和值；但它内嵌的码目录没有这 2 个新码，`actingctl codes`、`ac_outcome_code` 查它们答 `outcome_code_unknown`，个别按码
    分类显示的地方把它们当作未登记的码（`outcome_relay_unregistered`，原拼写在 `raw_code`）。
  - actinglab 和 Runtime 混版两个方向都照常（调试事件没变）。资源工具要留意：本版的 `actinglab standard-pack check`、
    `package bundle --schedule` 接受消耗上的 `expected_amount_milli`，它放行的规则文件 v0.12.33 的 Runtime 不收，还有部署在 0.12.33
    上时资源侧先别写；v0.12.33 的资源工具拒收这种条目，也不报 `standard_pack_escalation_threshold_high`。

运行上的变化：

- 暂停中的实例，模拟器不再被自动开起来（PAUSE-AUTO）：守护进程启动、IDLE-1 唤醒和暂停前排下的监视认领都看实例自己的暂停，改记 `emulator_autostart_skipped`。
- 成功却没有进展的一对等着（SS-4）：调度恢复后，依据键没读到新值的成功运行之后以 `policy_no_progress` 等着；`scheduling.no_progress.enabled` 默认开。
- 消耗可以写期望值（SS-2g）：V2 `consumes` 的 `expected_amount_milli`，净收益精确计、整数读者向上取整；资源侧写之前不变。

本版合并的 PR：

- #1461 PAUSE-AUTO（#386）：实例自己的调度暂停期间，守护进程启动时按配置开模拟器和 IDLE-1 在 `stop_emulator` 之后的唤醒都不开它的模拟器；暂停前已排下的启动（含监视的重启和识别）在执行前再查一次暂停；跳过记一次 `emulator_autostart_skipped`（`skip_reason=instance_paused`）。人、代理和 Lab 的显式模拟器控制不变。
- #1463 SS-2g（#424）：V2 的消耗可以像产出一样写 `expected_amount_milli`。净收益精确读它；其余整数读者（保留视图和保留门、读数还没显示的消耗、SS-2 的平手键、`task.consumes.<池>`）向上取整。
- #1462 SS-4（#386）：无进展护栏。一次成功运行之后，派单依据的事实自准入以来没读到不同的值，这一对就以 `policy_no_progress` 等到某个依据键读到不同的值；失败照旧走重试。新键 `scheduling.no_progress.enabled`（默认开，可按实例，可在线设），`actingd-config` 修订 23。`actinglab standard-pack check` 和 `package bundle --schedule` 的新警告 `standard_pack_escalation_threshold_high`。

## v0.12.33

本版是 v0.12.32 之后合并的 2 个 PR：LAB-OBS-1、SS-2e。运行中会注意到的变化有两件，第一件要等调度恢复才看得到。
一是接在维护任务后面的业务规则也按净收益评判（SS-2e）：调度停着（每台实例都暂停）的时候，运行上什么都不变。调度重新跑起来之后，
一条业务规则如果从它沿链条往上走到链头，经过的链节（链头也算）全是维护规则，例如接在回主界面（`return_home`）后面的业务规则，
就按它自己声明的 `produces` 和 `consumes` 评判：这两项写上之前，它和别的没写数值的业务规则一样是 `benefit_unknown`（`benefit_gap`
是 `no_effects`），不派；数值都有、净收益不大于 0 的是 `policy_benefit_nonpositive`，也不派。在 v0.12.32 上，这样的规则每次维护任务
成功后都不经评判就派出去，还排在评判过的业务候选前面。业务任务的后继照旧跟着那个业务任务走、不评判：`M → A → B`（`M` 是维护
规则，`A`、`B` 是业务规则）里，`A` 自己评判，`B` 跟着 `A`。
二是没认出页面的 Lab 观察不再留下 error 级的 `command.rejected`（LAB-OBS-1）：`ac_lab_observe`（`capture=true`）或
`actinglab observe --capture` 的帧不匹配包里任何页面时，回答照旧是 `ok`、`state: no_match`，收尾的 `runtime.debug_observe` 改记 info 级的 `command.validated`，错误计数
和诊断不再显示它。失败的 Lab 命令（observe、do、ensure、wait）在它的 `command.rejected` 上写明原因 `failure_code`，不再是空的
`values`。v0.12.32 已在本版之前发布。

配置：没有改。`actingd-config` 仍是 [5, 22]，`runtime-client` 仍是 [2, 3]（`min` 仍是 2），账本仍是 [3, 4]，`package` 仍是 [3, 5]，
其他接口也不变；`actingd.defaults.json` 自 v0.12.32 没有改动，没有新键，在线可改的键也不变，程序事实 `config.subsystems` 也没有新行。

- 没有新结果码：结果码目录仍是 3820 个码、1725 个位置，没有新定稿或退役的条目，所以本版的发布文件是空的。`command_rejected`
  （待审）多声明一个可选值 `failure_code`（键表里已有，类型 `code`）；`policy_benefit_nonpositive` 只改了说明。
- 账本没有新表、新索引、新事件类型或新形状，`ledger` 不升：`command.rejected` 从账本修订 3 起就可以带结果。
- 目录和规则文件的格式不变：编排器和目录读取器都没改，SS-2e 只改评估时谁被评判。
- Lab 调试事件（`RecordDebugEvent` 请求里的 `RuntimeDebugEvent`）多一个可选成员 `failure_code`，接口修订不升：这个请求只有同一构建的
  actinglab 发，所以 actinglab 要和 Runtime 同版（见下面的配对）。

升级时要知道的：

- `command_rejected` 上的 `failure_code`（LAB-OBS-1）：Lab 调试命令 observe、do、ensure、wait 失败时记一条 error 级的
  `command.rejected`，它的结果是 `command_rejected`，值 `failure_code` 是这次失败的码：observe 请求本身失败时是 Runtime 回答的码，
  do、ensure、wait 失败时是回答的码（转述时目录里没有的拼写变成 `outcome_relay_unregistered`）。observe 遇到账本或材料的致命错误时
  本身已是终态，照旧不追加收尾记录。`lab_run` 失败照旧记 `task.failed`，不带 `failure_code`。失败的命令事件必须带一个已登记的码，
  别的事件都不能带：违反的，客户端发送前报 `runtime_debug_event_invalid`，Runtime 收到时以 `invalid_runtime_debug_event` 拒绝，都不
  落记录，所以不会再有 `values` 为空的 `command.rejected`。
- 观察的收尾（LAB-OBS-1）：观察成立时，不管页面状态是 `no_match`、`conflict` 还是 `partial`，`runtime.debug_observe` 都以
  `completed` 收尾（记 `command.validated`），和 do、ensure、wait 一样；页面状态只在回答的 `state` 和观察自己的记录里。认出页面的
  观察不变。
- 谁被评判（SS-2e）：v0.12.32 不评判链头运行窗口里的任何链条后继；本版只在后继往上走到链头、经过的链节（每一节都在自己的窗口里，
  链头也算）里至少有一条带 `benefit_judged` 标记的业务规则时才不评判。经过的全是维护规则（没有标记）的业务候选，和不在任何窗口里的
  候选一样评判，原因、值和档内排序都是 SS-2b 的；它仍带着链头的 `priority` 和 `strategic_weight_milli`（SS-2），但在业务档里不再按
  它们排。维护规则的后继、手动请求、维护档候选和没有标记的目录（逐字节不变）都照旧。资源侧要让这类规则派出去，就给它写上
  `produces`、`consumes` 和所碰池的估值。没有新码、新值或新键。

退回 v0.12.32 之前要做的事（按代码核对过）：

- LAB-OBS-1：配置和状态文件都没动，不用先改什么，但 Runtime 包和 Tools 包要一起退回（actinglab 在 Tools 包里，MCP 的 Lab 工具跑的
  是根下 `tools` 目录里的 actinglab）。v0.12.32 的 `RuntimeDebugEvent` 是 `deny_unknown_fields`，本版 actinglab 在失败命令的收尾上写的
  `failure_code` 让它解不开整个请求，以 `runtime_protocol_invalid`（`runtime_request_decode_failed`）拒绝，不落记录。本版写下的记录
  v0.12.32 读得了：带结果的 `command.rejected` 是账本修订 3 就有的形状（`rejected_with_outcome`），结果读回时码按原拼写、值按映射
  保留，不查码也不查键（读回的结果不按发出的结果核），`failure_code` 也在它的键表里；没认出页面的观察留下的 `command.validated`
  是已有的记录。退回后，没认出页面的观察又会留下一条没有原因的 error 级 `command.rejected`，失败命令的记录又不带原因。
- SS-2e：不用做什么，按代码核过：没有新的账本、状态、目录、配置或线上形状，新被评判的候选给的原因（`benefit_unknown`、
  `policy_benefit_nonpositive`）是 v0.12.32 自己的码。v0.12.32 的条件是「有标记、不在任何链头窗口里、不是手动请求」，退回后链头窗口
  里的后继又全都不评判，接在维护规则后面的业务规则又会在每次维护成功后不经评判派出去（调度恢复后才会发生）。
- v0.12.32 一节的退回条件（SS-2b 的标记和代、SS-5 和 SS-2c 的成员、WATCH-P 的监视）是退回 v0.12.31 的条件，从本版退回 v0.12.32
  时不适用：本版没改编排器、目录格式和读取器（自 v0.12.32 起只改了评估器、Lab 调试事件和它的记录），本版启用和钉住的代、规则文件
  里的 `benefit_judged`、`"pending"`、`on_unknown`、`maintenance` 都是 v0.12.32 自己写、自己认的，监视对暂停实例的做法两版一样。要再
  往前退到 v0.12.31，照 v0.12.32 一节的退回条件全部做；本版的记录 v0.12.31 也读得了（它读结果的代码和 v0.12.32 相同，键表里也有
  `failure_code`）。
- 客户端和 Runtime 的配对（`runtime-client` 两边都是 [2, 3]；`apps/actingctl` 和 `crates/runtime-client` 自 v0.12.32 没改）：
  - 本版的 actingctl 和 MCP 连 v0.12.32 的 Runtime：照常。没有新命令、新请求或新工具（MCP 工具仍是 40 个），也没有新码。
  - v0.12.32 的 actingctl 和 MCP 连本版的 Runtime：照常，请求不变。本版没有新码，它内嵌的码目录认得本版发出的每个码；
    `command_rejected` 上的 `failure_code` 照原样显示。
  - actinglab 要和 Runtime 同版。本版的 actinglab 连 v0.12.32 的 Runtime：观察照常（成立的观察收尾不带 `failure_code`），但失败的
    observe、do、ensure、wait 的收尾被拒（`runtime_protocol_invalid`），命令照样失败，回答里多一句收尾没写进账本，这次调试命令没有
    终态记录。v0.12.32 的 actinglab 连本版的 Runtime：没认出页面（以及 `conflict`、`partial`）的观察仍发不带原因的失败收尾，被拒
    （`invalid_runtime_debug_event`），旧 actinglab 把这次观察答成设备错误，尽管观察本身成立；失败的 do、ensure、wait 的收尾也被拒。

运行上的变化：

- 接在维护任务后面的业务规则也按净收益评判（SS-2e）：调度恢复后，没写数值的停在 `benefit_unknown`；业务任务的后继照旧跟着走。
- 没认出页面的 Lab 观察记 `command.validated`（LAB-OBS-1）：不再留下 error 级的 `command.rejected`；失败的 Lab 命令写明 `failure_code`。

本版合并的 PR：

- #1458 LAB-OBS-1（#428）：帧不匹配任何页面的 `observe --capture` 以已完成（`command.validated`）收尾它的 `runtime.debug_observe`，不再记成没有原因的 `command.rejected`。失败的 Lab observe、do、ensure、wait 在它的 `command.rejected` 上写明原因 `failure_code`；不带原因的失败事件被拒收。
- #1459 SS-2e（#386）：链头运行窗口里的业务任务，只有往上走到链头经过的链节里有业务任务时才免于净收益评判；只接在维护链节（例如 `return_home`）后面的业务任务按它自己的净收益评判。

## v0.12.32

本版是 v0.12.31 之后合并的 6 个 PR：SS-2、WATCH-P、SS-5、LN-OFF-1、SS-2b、SS-2c。运行中会注意到的变化有三件，第一件会让业务任务
停下来。
一是业务任务按净收益评判（SS-2b）：包不是维护角色、规则也没有声明 `"maintenance": true` 的标准包规则都是业务规则，编排器给它们打上
`benefit_judged`；评估时这类候选在同一档里按「一次运行的净收益 ÷ 预计时长」从高到低排，净收益未知或不为正的不派。规则文件现在都
还没有写池的估值和每次运行的产出量、消耗量，所以部署后每个业务候选都是 `benefit_unknown`（`benefit_gap` 是 `no_effects`），一个也
不派。这是裁定接受的停摆：资源侧把数值写进规则文件之后，才逐条恢复。维护角色包的规则、声明了 `"maintenance": true` 的规则和
Runtime 自己的读数刷新照旧派；手动运行和 Lab 不经过这道门。没有开关，也没有配置键。资源工具（`actinglab standard-pack check`、
`package bundle --schedule`）按游戏报 `benefit_coverage`，从中可以看出数值写到了哪一步。
二是暂停中的实例，监视不再替它开模拟器（WATCH-P）：实例自己的调度暂停期间（`draining`、`paused`、`released` 任一阶段，重启后恢复的
暂停也算），模拟器监视照常读状态、记录变化，但不再重启它关着的模拟器，也不再识别、绑定它开着的模拟器，改记一条
`emulator_autostart_skipped`（`skip_reason` 是 `instance_paused`）。所以对暂停中、模拟器关着的实例做手动运行或 Lab 之前，先用模拟器
控制（`actingctl emulator`、`ac_emulator`）把它开起来：手动运行和 Lab 操作自己不开模拟器，实例的端点待绑定时以
`instance_not_running` 结束。为了不让监视碰暂停中的实例而设成 `false` 的 `emulator_control.watch_enabled`，现在可以打开了。
三是离线读者不再把整本账读进内存（LN-OFF-1）：UI 的离线读面和 `actingledger views`、`material`、`facts --at` 打开账本时，只做写入端
打开时的那套检查，之后在钉住的头上一页一页地短读，不再把整本账读进内存，也不再每读一页就把整本账重证一遍；运行中的根也能读。
另外三件各一句。SS-5：事实谓词可以写 `on_unknown: "probe"`，只有本包读得到的事实未知时，每个未知期放这个任务跑一次；要等资源侧
写了才起作用。SS-2c：`schedule.json` 的规则可以用 `"maintenance": true` 声明自己是维护，不受净收益评判；也要等资源侧写。SS-2：
每个游戏的优先级按两档写（维护高、业务一般；规则用了超过两个值，资源工具报警告，只报不拒），链条后继在链头这次运行的窗口里随链头
排序，平手时先失去资格的先跑、再按声明顺序；后两条部署后就生效。v0.12.31 已在本版之前发布。

配置：没有改。`actingd-config` 仍是 [5, 22]，`runtime-client` 仍是 [2, 3]（`min` 仍是 2），账本仍是 [3, 4]，`package` 仍是 [3, 5]，
其他接口也不变；`actingd.defaults.json` 自 v0.12.31 没有改动，没有新键，在线可改的键也不变。

- 程序事实 `config.subsystems` 多一行常开的 `fact_unknown_probe`（SS-5，`config_subsystem_always_on`）：写了 `on_unknown` 的
  `schedule.json` 必须在 `requires` 里写它，所以只有认得这一行的 Runtime 才编排这个文件。这是事实里的一行，不是配置键。
- 离线读的期限（LN-OFF-1）不加新键：打开默认用 `ledger_observer.open_timeout_ms` 的出厂值（600 秒），之后每次读默认用
  `ledger_observer.writer_reply_timeout_ms` 的出厂值（10 秒，从这次读开始算）；每页的上限沿用现有的常量（256 条、768 KiB）。
- 新结果码 6 个，都还是待审（pending）：`standard_pack_priority_tiers_exceeded`（SS-2）、`fact_probe`（SS-5）、`benefit_unknown`、
  `policy_benefit_nonpositive`、`standard_pack_benefit_undeclared`（SS-2b）、`standard_pack_priority_tier_mismatch`（SS-2c）。结果码目录
  从 3814 个码变成 3820 个，位置仍是 1725 个；没有新定稿或退役的条目，所以本版的发布文件是空的。新值键 4 个：`skip_reason`
  （WATCH-P）、`benefit_gap`、`benefit_milli`、`expected_duration_ms`（SS-2b）；新词表 `benefit_gap`、`emulator_autostart_skip_reason`；
  policy 的码多一个前缀 `benefit_`。
- 账本没有新表、新索引、新事件类型或新形状，`ledger` 不升；新原因和新值都放在已有的原因链里。
- 目录和规则文件的格式多了几个成员，格式版本不变，v0.12.31 及以前的 Runtime 拒收它们（见下面的退回）：V2 任务的 `benefit_judged`
  （由编排器写，规则文件自己写会被拒）；池的 `valuation` 和效果的 `amount` 可以写 `"pending"`；`fact` 谓词的 `on_unknown`（只在
  V2）；`schedule.json` 规则的 `maintenance`（只认 `true`，不进编排出的任务）。

升级时要知道的：

- 业务停摆，以及怎么读 `benefit_unknown`（SS-2b）：部署后第一次工作台检查就编排出新的一代（业务规则都加了标记，规则摘要变了）并
  启用。之后在 `actingctl schedule`、`ac_schedule` 和 `ac_overview`、`ac_diagnose` 的未运行一行里，业务候选的原因是
  `benefit_unknown`，由 `benefit_gap` 说明缺什么：
  - `no_effects`：任务既没写 `produces` 也没写 `consumes`；
  - `entry_pending`：某一项的 `amount` 是 `"pending"`；
  - `valuation_pending`：它碰到的池，估值是 `"pending"`；
  - `valuation_missing`：它碰到的池在这台实例上没有估值；
  - `reading_unknown`：它消耗的再生池读数未知或过期；这时同时请求读数刷新，候选最后的原因是刷新的原因。

  `pool_id` 指出是哪个池，读事实的还带 `fact_key` 和 `unknown_reason`。部分已知也算未知，已知部分绝不相加。数值都有、净收益不大于
  0 的是 `policy_benefit_nonpositive`（带 `benefit_milli`、`expected_duration_ms`），它消耗的再生池下一步变化时再评。两种都不派；空闲
  计划不把它们算作到期，不会为了它们把关着的游戏叫醒。不评判的有：维护角色包的规则、声明了 `"maintenance": true` 的规则、在链头运行
  窗口里的链条后继、由指令触发的任务（触发条件只经 `all` 要求一个 `session.` 事实），以及 Runtime 自己的读数刷新。排序：`promoted`
  在前；然后分档，没有标记的（维护档）在有标记的（业务档）前面；档内没评判的照以前的顺序在前，评判的按净收益 ÷ 预计时长从高到低，
  再按 SS-2 的平手规则；保留门也按这个顺序取候选。派出去的评判候选，`rank_breakdown` 带 `benefit_milli` 和
  `expected_duration_ms`。商店更新规则的包按根文件不是维护角色，也会被评判：遇到客户端更新对话框的实例会一直等着，直到这条规则写上
  `"maintenance": true`（SS-2c）或数值。这道门不解除、也不绕过任何暂停。
- `schedule.json` 的新成员和 `requires` 的新子系统（SS-5、SS-2c）：
  - `on_unknown: "probe"`（SS-5）写在规则 `trigger` 里的 `fact` 谓词上，只认 `"probe"`，只在 V2（V1 报 `unknown_field`）。键必须是
    实例作用域的读数标记键（`resource.` 或 `inventory.`），也不能放在 `not` 下或 `feedback_stop` 里，否则报 `predicate_uncomputable`
    （指在 `on_unknown`）。写了它的文件必须在 `requires` 里写 `fact_unknown_probe`，否则编排器拒收（`schedule_invalid`，指在第一个
    `on_unknown`）。键还必须是这条规则所绑的包自己的读数标记写的键：Runtime 看不到包的内容，所以由 `actinglab standard-pack check`
    和 `package bundle --schedule` 核，只有别的包读或没人读的键报 `standard_pack_root_invalid`、`schedule_invalid`。键未知时谓词为真
    （原因 `fact_probe`，带 `fact_key`、`unknown_reason`），不为这个键请求读数维护包。未知期从清掉这个键的时间线重置最近一次发生、或
    过期读数的新鲜期结束算起，取较晚的；这之后派出的本任务成功跑完一次，这一期就用掉了（`fact_probe` 带
    `served_by_dispatch_unix_ms`，谓词为假，不设唤醒），直到有新读数、下一次重置或过期。失败或取消的运行不算用掉。从不重置的键只会
    被探一次，资源侧要给这类键配上重置。
  - `"maintenance": true`（SS-2c）写在 `schedule.json` 的 `rules[]` 上，和 `package_id` 同级，可选，只认布尔 `true`：`false`、字符串、
    数字、`null` 一律拒收（`standard_pack_root_invalid`，`schedule_invalid`，指针 `/rules/<i>/maintenance`），「没声明」只有不写这一种
    写法。读取器把它从规则里取出，编排出的任务里没有它；原始 V2 任务写它报 `unknown_field`。效果只有一条：编排器不给这条规则打
    标记，它留在维护档，按 SS-2b 之前的顺序排、照旧派，`priority` 和各道门都不变；包已经是维护角色时再写也无害，编排结果逐字节相同。
    工具的 `benefit_coverage` 多一个计数 `maintenance_declared`；一个游戏的规则恰好用两个 `priority` 值时，声明了维护却用低值、或被
    评判却用高值的规则，工具报 `standard_pack_priority_tier_mismatch`（只是警告）。
  - `requires` 新的子系统只有 `fact_unknown_probe`；`maintenance` 和 `"pending"` 没有子系统，旧 Runtime 直接拒收这些成员。
- `"pending"` 的写法（SS-2b，只在 V2）：池写 `"valuation": "pending"`，效果（产出或消耗都行）写 `"amount": "pending"`，就写在值的位置
  上，不加新成员。它只让碰到它的被评判任务的净收益变成未知，别的都不变：估值标记对其他读者等于没有估值；数量标记对资源项、保留门、
  平手规则和派单值等于这一项没有声明数量，前向投影当作 `effect_evidence_insufficient`。别的字符串是 `type_mismatch`，V1 文档写它也是；
  `amount` 写了 `"pending"` 又写 `expected_amount_milli` 是 `limit_exceeded`；估值写 `null` 仍是没有估值。工具里，缺的值全是
  `"pending"` 的规则算 pending，不算 undeclared，也不报 `standard_pack_benefit_undeclared`。
- 监视的新跳过记录（WATCH-P）：`emulator_autostart_skipped`（info，已有的码）带 `alias`、`watch_kind`（`restart` 或 `recognize`）和
  新值 `skip_reason` `instance_paused`，同时是实例下的一条 `device_state_observed` 记录和一行报告；每个暂停 revision、每种
  `watch_kind` 只记一次，不是每次轮询都记。跳过不排认领、不占重启次数（`emulator_control.restart_attempts`、`restart_window_ms`），
  也不记 `emulator_watch_restart_exhausted`。恢复不加新东西：恢复照常重连、自检，暂停解除后的第一次轮询照常重启或识别；再暂停是
  新的 revision，会再记一条。没变的：全局暂停不挡监视；守护进程启动时按配置开模拟器（`start_emulator`）、IDLE-1 的唤醒，以及暂停
  开始前已经排下的监视认领都不看暂停，照旧执行；`watch_enabled = false` 仍是整个停掉轮询。
- 离线读（LN-OFF-1）：打开时不再对整个文件跑 `quick_check`，只核写入端打开时核的那些（meta 行、标记 `ready`、头、最新
  `open_verify_tail_records` 条逐条证实并链到头哈希），再加连续性；更早的记录读到时才核，所以打开时不再发现它们的损坏，整本核验用
  `ledger-maintenance verify`。每页最多 256 条或 768 KiB，超出的部分带游标续读，不会被悄悄截断。超过期限以已有的
  `ledger_read_budget_exceeded` 拒绝，detail 带 `phase=open|proofs|page|resolve`。`actingledger events`（走 `open_range`）和整本证实的
  读法（`open_evidence`）不变。

退回 v0.12.31 之前要做的事（按代码核对过）：

- 业务规则的标记（SS-2b）：0.12.32 第一次工作台检查后，启用的那一代带 `benefit_judged`（规则文件写了 `"pending"` 或 `on_unknown`
  的，那一代也带着它们）。v0.12.31 启动时要重新编译启用的那一代、每个未了结的派单钉住的那一代，以及决策记忆点名的每次派单（每个
  任务和实例这一对最近一次完成的派单，补救任务已准入的运行）所在的那一代；它的 V2 读取器不认这些成员（`unknown_field`、
  `type_mismatch`），于是以 `catalog_generation_invalid` 致命地停下，整个 Runtime 起不来。所以回退前先在 0.12.32 上让它启用一个不带
  这些成员的代：把各游戏根里的 `schedule.json` 拿走（编排出合法的空目录），或者只留维护角色包的规则、并且不写 `"pending"`、
  `on_unknown`、`maintenance`；等下一次工作台检查启用了它（程序事实 `config.catalog` 换成了新的代），再确认没有未了结的计划派单
  （调度暂停并排空，计划运行都结束了）。在带标记的代下没派过计划运行时（调度一直暂停着就是这样），v0.12.31 就能在这一代上启动，
  第一次检查再编排它自己的、不带标记的代。已经在带标记的代下派过计划运行的，每个这样的（任务、实例）对都要先在新的代下再完成一次
  派单，把决策记忆点名的那一次换掉，否则 v0.12.31 启动照样以 `catalog_generation_invalid` 停下；规则已经拿走、不会再派的对换不掉，
  这时不能退回。
- `schedule.json` 里的 SS-5、SS-2c 成员：v0.12.31 不认 `fact_unknown_probe` 这个子系统，写了 `on_unknown` 的文件以
  `schedule_requires_missing` 拒收；写了 `maintenance` 的，它的读取器把这个成员原样交给任务，编译器报 `unknown_field`。两种都记
  `standard_pack_root_invalid`，这个游戏退回启用的代里的那份规则文件；那份也被拒、或者从来没有好的，这个游戏就没有规则，其他游戏
  照常。回退前先把这两个成员从各游戏根的 `schedule.json` 里删掉。`on_unknown` 还会进编排出的代（`maintenance` 不会，读取器已把它
  取出），所以它也受上一条的启动编译限制。
- 监视（WATCH-P）：v0.12.31 的监视对暂停中的实例照样重启、识别（只有一条识别路径排除 `released` 阶段）。有实例暂停着时，回退前先
  `actingctl config set emulator_control.watch_enabled=false`（或用 `ac_config_set`）；这个键 v0.12.31 也能在线设，覆盖照样保留。本版
  写的跳过记录 v0.12.31 读得了（码是已有的，值按映射保留、不查键），跳过的记忆不落盘。
- LN-OFF-1 和 SS-2：不用做什么，按代码核过，没有新的账本、状态、目录、配置或线上形状。退回后 v0.12.31 的离线读者打开时和每读一页
  又会把整本账读进内存，大账本上不要用它们跑；SS-2 的决胜和链头字段只写在内存里那些决定的开发者文本里，档数警告只在工具里、不入
  账本，退回后链条后继用自己的优先级，平手回到种子哈希。
- 账本：本版写的新原因和新值都放在已有的原因链里，v0.12.31 读得了（结果码按原拼写、值按映射读回，不查键）。
- 只在程序里、不存状态的改动，退回后回到 v0.12.31 的行为：不按净收益评判，监视又会动暂停中的实例，离线读整本，没有事实探针。
- 客户端和 Runtime 的配对（`runtime-client` 两边都是 [2, 3]，请求种类和回答形状都没变）：
  - 本版的客户端连 v0.12.31 的 Runtime：照常。客户端这边没有新命令、新请求或新工具（MCP 工具仍是 40 个），v0.12.31 也不会发出本版
    的新码。本版的资源工具接受 `on_unknown`、`maintenance` 和 `"pending"`，它放行的规则文件 v0.12.31 的 Runtime 不收：还有部署在
    0.12.31 上时，资源侧先别写这些成员。
  - v0.12.31 的客户端连本版的 Runtime：照常，请求不变。`actingctl schedule`、`ac_schedule` 里会出现 `benefit_unknown`、
    `policy_benefit_nonpositive`、`fact_probe`，`emulator_autostart_skipped` 多一个值 `skip_reason`，旧客户端照原样显示码和值；但它内嵌
    的码目录没有这 6 个新码，`actingctl codes`、`ac_outcome_code` 查它们答 `outcome_code_unknown`，个别按码分类显示的地方把它们当作
    未登记的码（`outcome_relay_unregistered`，原拼写在 `raw_code`）。

运行上的变化：

- 业务候选按净收益评判（SS-2b）：未知或不为正的不派；部署后业务候选都是 `benefit_unknown`，维护照旧派。
- 暂停中的实例，监视不开模拟器、不识别（WATCH-P）：改记 `emulator_autostart_skipped`（`skip_reason=instance_paused`）。
- 离线读账本有界（LN-OFF-1）：打开只核头尾，按页短读，每页最多 256 条或 768 KiB；运行中的根也能读。
- 自读事实探针（SS-5）：`on_unknown: "probe"`，每个未知期放行一次。
- 规则的维护声明（SS-2c）：写了 `"maintenance": true` 的规则不受净收益评判。
- 两档优先级、链条随链头、平手规则（SS-2）：超过两档工具报警告；平手时先失去资格的先跑，再按声明顺序。

本版合并的 PR：

- #1451 SS-2（#386）：两档优先级，组合器在一个游戏的规则用了超过两个值时报 `standard_pack_priority_tiers_exceeded`（只是警告）；链条后继随链头排序；平手时限定条件紧的先跑，再按声明顺序。
- #1452 SS-5（#386）：事实谓词可以写 `on_unknown: "probe"`，本包自己读的事实未知时，每个未知期放行这个任务一次。原因 `fact_probe`；子系统 `fact_unknown_probe`；资源工具核这个键是不是本包读数标记写的。
- #1453 LN-OFF-1（#422）：只读读者的有界离线打开：先做写入端的头尾检查，之后钉住的短页读和取材料的单点读。运行中的根可以读。每页最多 256 条或 768 KiB，带游标续读。超期报 `ledger_read_budget_exceeded`，带 `phase=`。
- #1454 WATCH-P（#386）：模拟器监视对实例自己的调度暂停（任一阶段）下的实例既不重启也不识别；每个暂停 revision 记一条 `emulator_autostart_skipped`（`skip_reason=instance_paused`）。
- #1455 SS-2b（#386）：同一档里，非维护角色包的规则按净收益 ÷ 预计时长评判；`policy_benefit_nonpositive` 和 `benefit_unknown` 不派。`"pending"` 值标记。工具里的 `benefit_coverage` 和 `standard_pack_benefit_undeclared`。
- #1456 SS-2c（#386）：`schedule.json` 规则上的 `"maintenance": true` 让它不受评判。覆盖率里的 `maintenance_declared`。警告 `standard_pack_priority_tier_mismatch`。

## v0.12.31

本版是 v0.12.30 之后合并的 4 个 PR：SS-6、R-1、R-2、RR-1。运行中会注意到的变化有五件，第一件是生产缺陷的修复。
一是立即重跑占住实例的死循环修好了（RR-1）：在 v0.12.30 上，一个任务失败后开了立即重跑的一轮（`failure_retry_immediate`），
这一轮开着时它的预计时长变了（最常见的是部署后第一次跑这个包时还不认得包、按声明时长记下，之后改按实测样本算；或者这个包
又有了新的成功样本）。从此评估每一轮都选中这个立即重跑，准入每次都以 `policy_retry_round_ended` 拒掉，同一台实例上的其他
任务一直轮不到（显示 `instance_already_selected`），账本里每 1–45 秒多一条错误事件。本版评估和准入用同一个判断，也比较
预计时长：条件已变的立即重跑不再被选中，退回按自己的触发和冷却走；准入仍以这个码拒掉的（评估和准入之间窗口刚好结束），
这一轮当场撤掉，实时和按账本重放都一样。升级后第一次评估起，卡住的实例就不再被这一轮占住、照常派其他任务；按账本重建决策
记忆时，v0.12.30 记下的这种拒绝也会把那一轮撤掉。二是性能预设和性能键热改（R-2）：`performance.pacing.mode`、
`.max_concurrent_runs`、`.heavy_dispatch_limit.normal` 和 `.paused` 可以在线改了，下一次评估就按新值，正在跑的运行不打断；
新键 `performance.presets` 定义三个预设（standard、adaptive、light），`actingctl perf-preset get|set <档名>` 和 MCP
`ac_perf_preset` 读当前档或一次套用一档。「同时执行」指同时执行的任务数 `max_concurrent_runs`，不是开着的模拟器数。三是
新装的默认值改了（R-2）：`performance.pacing.mode` 默认 `pace`，`scheduling.parallel.enabled` 默认 `true`，新装不加覆盖就是
adaptive 档；配置文件自己写了这两个键的、用指令设过的，按自己的值不变。四是派单时交给包的值（SS-6）：可花的量、剩余缺口和
回数上限；生产目前没有包读它们，也没有任务写 `reserves` 或 `produces`，派单不变。五是配置文件没写、又没有默认值的键也能
在线设了（R-1）：每实例的 `scheduling.blackout`、`recovery_ladder_profile` 和 `scheduling.fact_refresh.overdue_ms`。
v0.12.30 已在本版之前发布。

配置：`actingd-config` 从 [5, 21] 改为 [5, 22]；`runtime-client` 仍是 [2, 3]（`min` 仍是 2）；账本仍是 [3, 4]，`package` 仍是
[3, 5]，其他接口不变。

- 新键一个（R-2）：`performance.presets`，对象，档名 → `{mode, max_concurrent_runs（可不写）, heavy_dispatch_limit_normal,
  parallel_enabled}`。默认值在 `actingd.defaults.json`：standard 是 `observe`、`heavy_dispatch_limit_normal` 2、并行开；
  adaptive 是 `pace`、2、并行开；light 是 `pace`、`max_concurrent_runs` 1、`heavy_dispatch_limit_normal` 1、并行开。它在装配时
  读一次，不能在线改（指令报 `config_key_not_settable`），`config.parameters.performance` 里多一行。配置文件可以自己写这个
  成员：档名 1–64 字节、只用 `a-z0-9_`、不能叫 `custom`，预设里不能是 `full`，两档成员不能完全相同，不合规的启动和
  `check-config` 以 `perf_config_invalid` 拒绝。`actingd-config` 升到 22 只因为这个成员；不写它的配置文件照常读。
- 改了的默认值两个（R-2）：`actingd.defaults.json` 自 v0.12.30 以来只改了这两处，另外加了上面的 `performance.presets`：
  - `performance.pacing.mode` 从 `observe` 改为 `pace`：运行内的等待按实例的节奏系数拉长（宿主不忙时系数是 1000，不拉长），
    `observe` 只测量。
  - `scheduling.parallel.enabled` 从 `false` 改为 `true`：准入的计划运行在各自实例的工作线程上跑，一台实例的长运行不再挡住这
    一轮和其他实例。
  - 其他都没改：`heavy_dispatch_limit.normal` 仍是 2、`.paused` 仍是 1，`max_concurrent_runs` 仍不写（不限）。部署计划的默认值
    对比会列出这两项。配置文件自己写了这两个键的按文件里的值，这次改动对它不起作用。用指令设过的（v0.12.30 只能设
    `scheduling.parallel.enabled`），指令照样恢复，在 `instructions` 检查的 `base_changed` 里点名，不算失败。想保持升级前行为
    又没写文件的安装：升级后在部署挂起期间用指令把 `mode` 设回 `observe`、`parallel.enabled` 设回 `false`（`mode` 本版起能在线
    设），或者写进配置文件。
- 在线可改的键：`performance.pacing.mode`、`.max_concurrent_runs`、`.heavy_dispatch_limit.normal`、`.paused` 从不可改变为
  可改（R-2）；`max_concurrent_runs` 不论文件写没写都能设，清掉回到文件的值，文件没写就是不限。配置文件没写、又没有默认值的
  键，只要它的实例（每实例键）或它的节在配置里，就能在线设和清除（R-1）：每实例的 `scheduling.blackout`、
  `recovery_ladder_profile`，以及 `scheduling.fact_refresh.overdue_ms`，以前报 `config_key_unknown`。其他没有默认值的键
  （`standard_packs.root`，每实例的 `device_kind`、`recognition_package`、`start_emulator`）仍是装配时读，报
  `config_key_not_settable`。
- 新结果码：没有。结果码目录仍是 3814 个码、1725 个位置，本版的发布文件是空的，没有退役的码。新的值键 6 个（SS-6）：
  `run_cap`、`effect_index`、`spend_allowance`、`gap_remaining`（整数），`spend_allowances`、`gaps_remaining`（记录列表），
  都是 `policy_dispatch_parameters` 的可选值。
- 账本没有新表、新索引、新事件类型或新形状，`ledger` 不升。`host.config_overrides` 的行沿用原形状；文件没写的键的那一行
  `base_source` 是 `default`、`base_json` 是 `null`（R-1）。

升级时要知道的：

- `full` 不能用指令设（R-2）：`ConfigureSettings` 把 `performance.pacing.mode` 设成 `full` 以 `config_value_invalid`
  （`config_value_problem` 为 `out_of_range`）拒绝，启动恢复时记录里的 `full` 也不恢复（`config_override_dropped`）。`full`
  只能写在配置文件里；文件是 `full` 时，可以用指令或预设离开，清掉指令就回到 `full`。`.paused` 不大于 `.normal` 的整节检查照旧
  （`section_check`）。
- 套用预设是一次 `ConfigureSettings`（R-2）：四个成员（`mode`、`max_concurrent_runs`、`heavy_dispatch_limit.normal`、
  `scheduling.parallel.enabled`）一起写，全成或全不成，`host.config_overrides` 只多一条记录。预设没写的成员（standard、adaptive
  不写 `max_concurrent_runs`）发不带值的改动，清掉这个键的在线覆盖：回到文件的值，文件没写就是不限，回执里这一项是
  `{key, value: "null"（字符串）, source: default}`。一个成员过不了检查（例如文件把 `.paused` 写得比 light 的 `normal` 1 大），
  整档都不写。当前档：每个成员都等于某一档就是那一档，否则是 `custom`；standard 和 adaptive 只差 `mode`，单改 `mode` 就从一档
  变成另一档；文件写了 `max_concurrent_runs` 的，套用 standard 或 adaptive 后回到文件的值，显示 `custom`。表里没有的档名在写之前
  就被拒（CLI `actingctl_usage_invalid`，MCP `arguments_invalid`，都是 `not_recognised`）。`ac_perf_preset` 在 observer 层。
- 热改的生效时机（R-2）：`mode` 每次评估读，运行中下一步就按新模式的系数；三个上限在每次策略评估和前向投影读，已准入的运行
  不打断，下一轮按新值。
- 派单交给包的值（SS-6，给资源侧）：任务写了 `reserves`、`consumes` 里有同一个池，或者 `produces` 的池被生效目标覆盖时，派单
  不用 opt-in 就带上 `session.dispatch.schedule.spend_allowance.<i>`（`consumes` 第 i 项：保留门自己的可用量，即投影减去读数还
  没反映的其他任务消耗，再减保留线，不足取 0，所以包永远不会被告知可以花到保留线以下）、`session.dispatch.schedule.gap_remaining.<j>`
  （`produces` 第 j 项：生效目标的缺口，达成为 0）和 `session.dispatch.schedule.run_cap`，都是整数，只在运行的快照里叠加，
  不落库。`run_cap` 是各保留消耗池上「允许量 ÷ 每次消耗」向下取整的最小值，产出池都没有基准权重时再和最大的「缺口 ÷ 每次产出」
  向上取整取小，最后**不超过 1000**（`max_repeats` 事实上限的上限），所以包用 `max_repeats: {"fact_key":
  "session.dispatch.schedule.run_cap"}` 读它不会撞上范围错误；声明不全时不给。B8 的 `daily_remaining`、`last_daily_slot` 不变。
  只有这两类任务的派单记录会变，其他派单逐字节不变。
- 清掉文件没写的键（R-1）：回执里这一项是 `{key, value: "null"（字符串）, source: default}`，值是四个字符的字符串 `null`，不是
  JSON 的 null（回执的线上形状不变，`ConfiguredSetting.value` 是标量）；之后 `config.parameters` 里没有这个键的行。设在文件没写的
  键上的覆盖，文件仍没写就保留，文件后来写了就作废（`config_override_superseded`）。无默认值的键原来文件写了、后来删了时，覆盖
  以前报 `config_override_dropped`，现在报 `config_override_superseded`；部署工具的 `instructions` 检查对两者一样算不通过。
- 重放的变化（RR-1）：v0.12.30 写下的、含被准入以 `policy_retry_round_ended` 拒掉的立即重跑的账本，本版重放时没有那一轮
  （v0.12.30 重放会留着），这是有意的，留着的正是缺陷的状态。失败状态、连败、退避和计数都不变，这个任务下一次合法的正常触发
  再开新的一轮；成功照旧清掉这一轮。被撤的那一对没有专门的原因码，靠它原有的原因说明在等什么（例如 `task_cooldown_active`）。

退回 v0.12.30 之前要做的事（按代码核对过）：

- 配置文件写了 `performance.presets`：v0.12.30 的性能节不认这个成员，启动和 `check-config` 拒绝整个文件。回退前先把这个成员
  删掉并提交（`acsetup --commit-config`）。acsetup 和安装器不写它，没手写过就不用管。v0.12.30 自己的默认值重新生效：没有覆盖的
  `mode` 回到 `observe`，`parallel.enabled` 回到 `false`。
- 四个性能键的在线覆盖和 R-1 的行：行的形状没变，v0.12.30 读得了，但它认为这四个键不可改、配置里也没有文件没写的键的设置，
  所以逐行作为 `config_override_dropped` 丢掉，列进 `instructions_dropped`。启动不失败，但它下一条 `host.config_overrides` 记录
  不再有这些行（丢了，不会留到再升级），部署工具回退启动后的 `instructions` 检查也因此不通过。回退前先
  `actingctl config clear <键>` 把它们清掉（包括套用预设留下的），或者接受这一项不过；想在 v0.12.30 保留这些值就写进配置文件。
  `scheduling.parallel.enabled` 的覆盖 v0.12.30 也能设，照样保留（基值变回 `false`，在 `base_changed` 里点名，不算失败）。
- 派单值（SS-6）：不用做什么，v0.12.30 读得了本版写的派单链（这类值不做键检查）。但它恢复这样的派单时只叠加 B8 的键，读新键
  的包遇到未知事实：用 `max_repeats` 读 `run_cap` 的，在任何输入之前以已有的 `contained_task_designated_effect_cap_unresolved`
  结束。所以资源侧要等所有部署都在 0.12.31 或以上之后才让包读这些键；回退前先把读它们的包换回不读的版本。
- 立即重跑（RR-1）：账本和配置都不用处理，不带 `retry_round` 的值记录本来就是 v0.12.30 写和读的形状。但缺陷会回来：立即重跑的
  一轮开着时预计时长一变，这台实例又会被占住，表现是 `ac_schedule` 里这台实例的其他候选一直是 `instance_already_selected`，
  账本里反复出现 `policy_retry_round_ended`。
- 只在程序里、不存状态的改动，回退后回到 v0.12.30 的行为：四个性能键回到装配时读、不可改；`perf-preset` 和 `ac_perf_preset`
  没有了；文件没写的键又报 `config_key_unknown`；评估不再和准入共用判断。
- 客户端和 Runtime 的配对（`runtime-client` 两边都是 [2, 3]，请求种类和回答形状都没变）：
  - 本版的客户端连 v0.12.30 的 Runtime：照常。`actingctl perf-preset get|set` 和 `ac_perf_preset` 在 v0.12.30 上找不到
    `performance.presets` 那一行，在写任何东西之前以 `config_facts_missing` 结束；`config set` 四个性能键被 v0.12.30 以
    `config_key_not_settable` 拒绝（文件没写的 `max_concurrent_runs` 是 `config_key_unknown`），文件没写的 R-1 键以
    `config_key_unknown` 拒绝。
  - v0.12.30 的客户端连本版的 Runtime：照常。请求不变，它的 `config set` 现在也能设这四个键和 R-1 的键（设 `full` 被拒）；清掉
    文件没写的键时回执值是字符串 `"null"`，旧的回执结构照样解码；`config.parameters.performance` 多一行 `performance.presets`。
    本版没有新结果码。它没有 `perf-preset` 命令。

运行上的变化：

- 被拒的立即重跑不再占住实例（RR-1）：评估和准入用同一个判断；被准入以 `policy_retry_round_ended` 拒掉的立即重跑当场撤掉那一轮，同一台实例的其他任务照常派。
- 派单给包的值（SS-6）：可花的量、剩余缺口和回数上限（最多 1000）随派单交给运行；生产没有包读它们，派单不变。
- 文件没写的键也能在线设（R-1）：每实例的 `scheduling.blackout`、`recovery_ladder_profile` 和 `scheduling.fact_refresh.overdue_ms`。
- 性能热改与预设（R-2）：四个性能键在线可改；三个性能预设，`actingctl perf-preset`、`ac_perf_preset`；新装默认 `pace`、并行开。

本版合并的 PR：

- #1446 SS-6（#386）：派单把 `session.dispatch.schedule.spend_allowance.<i>`、`.gap_remaining.<j>` 和 `.run_cap` 交给运行。允许量是保留门自己的可用量减保留线；`run_cap` 最多 1000。生产没有包读它们，派单不变。
- #1447 R-1（#429）：文件没写、又没有默认值的键可以在线设和清除：每实例的 `scheduling.blackout`、`recovery_ladder_profile`，以及 `scheduling.fact_refresh.overdue_ms`。
- #1448 R-2（#429）：`mode`、`max_concurrent_runs`、`heavy_dispatch_limit.normal`/`.paused` 可以在线改（指令设 `full` 被拒）；新键 `performance.presets`（standard、adaptive、light）；套用预设是一次全成或全不成的 `ConfigureSettings`；共用的「当前档」函数；`actingctl perf-preset`、MCP `ac_perf_preset`。改了的默认值：`performance.pacing.mode` → `pace`，`scheduling.parallel.enabled` → `true`。`actingd-config` [5, 22]。
- #1449 RR-1（#428）：生产缺陷修复。被准入拒掉（`policy_retry_round_ended`）的立即重跑不再一直占住实例：选候选和准入共用一个判断，也比较预计时长；被拒的立即重跑撤掉那一轮，实时和重放都一样。

## v0.12.30

本版是 v0.12.29 之后合并的 7 个 PR：SS-3、SS-1、R-0、R-15a、R-0c、R-0d、UPD-1。运行中会注意到的新东西有两件。
一是客户端过期时自动更新（UPD-1）：资源根可以在 `interrupts.json` 里给游戏的「客户端过期」弹窗页写
`result: "application_update_required"`。运行碰到这个页时，处理包照跑，然后运行以 `contained_task_application_update_required`
（warning，不开阶梯）结束；结束之前 Runtime 给这台实例记下事实 `health.application.update_required` 为 true。之后这台实例上
除商店更新包（`control.json` 写了 `store_update` 的包）以外的计划任务都推迟（`application_update_pending`），由 `schedule.json`
里按这个事实触发的商店更新规则去跑商店更新包；它运行成功后事实变回 false，被弹窗打断的任务按欠下的补跑先跑。别的实例不受
影响。这套流程要等资源根同时写了 `result` 和商店更新规则才生效，商店更新包点商店仍要配置的允许名单
`store_foreground.allowed_application_ids` 写了这个商店（v0.12.29）。二是工作台的地基（R-0、R-0c、R-0d、R-15a）：控制台要用
的请求和回答形状一次定下，`runtime-client` 声明 [2, 3]，客户端有了对应的方法；新的请求种类本版都答 `not_implemented`，什么
都不做，行为由后续版本补。另外，派单的次数上限可以不写了（SS-3），池的基础估值在每个实例上都计入（SS-1）；生产资源仓的规则
都写齐了次数上限，也没有池写估值，所以这两项本版不改变线上派单。不写 `result` 的根、不改配置的实例，运行和 v0.12.29 一样，
只多了启动时的一条种子事实（见下面）。v0.12.29 已在本版之前发布。

配置：`actingd-config` 仍是 [5, 21]；`runtime-client` 从 [2, 2] 改为 [2, 3]（`min` 仍是 2）；账本仍是 [3, 4]，`package` 仍是
[3, 5]，其他接口不变。

- 新键：没有。`actingd-config` 没有升 revision，现有的配置文件照常读。
- 改了的默认值（UPD-1）：`scheduling.rerun.condition_codes` 末尾加了 `contained_task_application_update_required`，从 9 个码
  变成 10 个，原来的 9 个不变。`actingd.defaults.json` 只改了这一处。部署计划的默认值对比会列出这一项。配置文件自己写了
  `condition_codes` 的按文件里的值，这次改动对它不起作用：碰到弹窗的计划运行不按条件终止，按普通失败结算（扣次数、加连败），
  需要的话自己加上这个码；用指令设过这个键的，指令照样恢复，在 `instructions` 检查的 `base_changed` 里点名，不算失败。
- 其他默认值都没改，现有的配置都不需要修改。
- 账本没有新表、新索引或新事件类型，`ledger` 不升；本版写的新记录都是已有的记录类型（见下面）。SS-3 的准入回执成员改为可选，
  但只在资源侧省略某个界之后才会写出少成员的回执（见下面的回退）。
- 新结果码 3 个（UPD-1）：`contained_task_application_update_required`（warning，阶梯类 none，带 `page_id`）、
  `application_update_pending`（info，带 `instance_id`、`fact_key`）、`application_update_key_reserved`（error，带 `fact_key`）。
  `standard_pack_problem` 多了一个原因词 `interrupt_result_invalid`。新位置 12 个（R-0、R-0c）：runtime-host 的
  `run_instance_task`、`stop_instance_run`、`edit_pack_schedule`、`edit_resource_target`、`routine_summary`、
  `pack_schedule_view`，runtime-client 的同名六个（前缀 `runtime_`）。它们的评审状态都是 pending，结果码发布文件只收已定稿或
  已退役的条目，所以本版的发布文件是空的。没有退役的码。
- 新事实键 `health.application.update_required`（布尔，实例范围），只有 Runtime 写；每台实例占事实库 256 条容量里的 1 条。

升级时要知道的：

- `result` 要本版或以上（UPD-1）：v0.12.29 及更早的 Runtime 不认 `interrupts.json` 的 `result`（打断项拒绝不认识的成员），
  启动时把写了它的整个游戏根记为 `standard_pack_root_invalid`（`malformed`，`check-config` 也列出），准备阶段拒绝每个要用这个
  根的运行，Runtime 不停；v0.12.29 的 Tools 打包时也一样拒收。商店更新规则本身是普通的事实触发规则，v0.12.29 读得了，但它
  不写这个事实，规则永远不会触发。所以资源仓要等本版部署并核过之后，在同一个 PR 里把 CI 钉的 Tools 换到 0.12.30 或以上，把
  `result` 和商店更新规则一起加上，再热放置。
- 先把商店写进允许名单：商店更新包点商店要 `store_foreground.allowed_application_ids` 写了它声明的商店（只能改配置文件、重启
  生效）。名单没写时商店更新包的运行以 `application_store_not_allowed` 失败，失败的商店运行不写事实，实例一直被扣住。
- 只写了 `result`、没写商店更新规则的根：碰到弹窗后这台实例的计划任务一直推迟，每个候选都显示 `application_update_pending`，
  不是静默的。处理：手动更新客户端后直接运行这个游戏的商店更新包（`ac_run_pack`），它成功（包括「不需要更新」的成功）就解除。
- 扣住的范围（UPD-1）：事实为 true 时，这台实例除商店更新任务以外的所有候选都推迟，欠下的补跑、立即重跑、补救请求和读数刷新
  （`fact_refresh_pending`，`gate_code` 是 `application_update_pending`）都算在内，不论排名；不设唤醒时间，事实一变就重新评估。
  空闲计划不为被扣住的任务唤醒这台实例。其他实例不受影响。商店更新任务照常过其他各道门。
- 启动种子（UPD-1）：本版每次启动时，给每个配置了的例行实例中还没有这条记录的写一条 `health.application.update_required` =
  false（`source_snapshot_id` 为 `application_update:seed`，观测时间为 1，不过期），所以升级后第一次启动每台实例一条，以后的
  启动不再写；已有的记录（包括 true）保留。观测时间取 1 而不是启动时刻：种在启动时刻会让问更早时间的评估和前推整份被拒。
  种子写不进去（比如事实库满）时启动失败，和其他种子一样。
- 商店更新规则的写法（给资源侧）：每个服一条，`trigger` 为 `health.application.update_required` eq true，`feedback_stop` 为
  eq false，`cooldown_ms` 是商店还没有新版本时隔多久再看，必须大于 0：`package bundle --schedule` 和 `standard-pack check` 以
  `standard_pack_root_invalid`（`schedule_invalid`，`/rules/<i>/cooldown_ms`）拒收为 0 的规则（这是 Tools 的检查，Runtime 的
  组装不重复）。
- 时间线不能作废这个事实（UPD-1）：`schedule.json` 里某个时间线事件的 `invalidates_fact_prefixes` 覆盖这个键（键本身或它的
  任何前缀，包括 `health.`），而同一事件的 `excludes_fact_prefixes` 没有保住它的，组装时被拒：Runtime、`check-config`、
  `package bundle --schedule`、`standard-pack check` 都记为 `standard_pack_root_invalid`（`schedule_invalid`，
  `/timeline/<i>/invalidates_fact_prefixes/<j>`），这个游戏沿用当前生效代的规则。这一条对所有资源根都生效，部署前先核现有的
  时间线有没有这样的前缀。
- 弹窗页的写法（给资源侧）：`result` 只能配 `then: "end_run"`，配别的 `then` 是 `interrupt_result_invalid`
  （`/interrupts/<i>/result`），别的值或类型是 `malformed`；Runtime、`package bundle`、`standard-pack check` 用同一个解析。普通
  任务包不要把这个页声明成自己的页；商店更新包把它当入口页，`package_bundle_interrupt_page_shadowed` 不再对这种包报。商店更新
  包「不需要更新」的结束要算成功，只有成功才清掉事实。处理包非致命失败时运行仍以新码结束（处理包的失败作为 `related` 链接，
  事实照样先记 true）；致命失败照旧毒化运行，处理期间被取消、暂停或到期按那次停止结束。
- 阶梯和开机包（UPD-1）：阶梯某一级的包运行以新码结束时，这一级记 `recovery_ladder_interrupted`，阶梯以 `exhausted`（warning）
  结束，不再爬下一级，不会因为客户端旧而重启模拟器。开机包碰到弹窗记一条 warning 的 `runtime.failed`，不开阶梯。敏感任务碰到
  弹窗仍按普通失败结算。
- 保留键（UPD-1）：客户端的 `PublishFact`、`PublishFacts` 写这个键一律被拒（`application_update_key_reserved`），什么都不记。
  要手动解除就直接运行商店更新包。
- 次数上限可以不写（SS-3，给资源侧）：V2 目录里任务的 `loop_budget` 和它的三项、活动档的三项都可以不写；不写的那一维准入不
  预留、不拒绝，结算不会因它改写成 `policy_runtime_budget_exceeded`，没写 `max_runtime_ms` 时预计时长不封顶，B8 的
  `daily_remaining` 只从写了的日界算。写了的界照旧生效，全部写齐的目录哈希、回执和意图逐字节不变。V1 仍要六项都写（缺了报
  `missing_required_field`），显式写 `null` 是 `type_mismatch`。Runtime 不记没写那一维的计数，也不清已记的计数：当天（或时段
  内）又写回时读回省略前的计数，省略期间准入的运行不计。**资源根一旦省略某个界，就不能再退到 0.12.30 以下**（见下面的回退），
  所以资源侧要等所有部署都在 0.12.30 或以上之后才省略。
- 基础估值常开（SS-1）：没有生效中的 v2 目标（没有记录、已撤回、已过期、读不了、v1 目标）时，产出带 `valuation` 的池的任务也
  得到基础项（每次产出 × base / scale），原因链里是 `resource_weights` 的 `policy=none`；目标只叠加缺口项，达成后任务仍是候选。
  资源层不读任务开关、实例覆盖、优先级和偏移。v1 目标的 `U` 现在把被实例覆盖禁用的点名任务也算进去，以前禁用一个点名任务会抬高
  其他点名任务的分。
- 工作台（R-0、R-0c、R-0d，给控制台）：新请求种类 `RunInstanceTask`、`StopInstanceRun`、`EditPackSchedule`、
  `EditResourceTarget`、`RoutineSummary`、`PackScheduleView` 本版都只先核实例别名，然后答 `not_implemented`，不写记录、不改状态。
  请求带了 `ConfigureSettings.expected_version` 或 `ApplyResourceTargets.expected_version` 也答 `not_implemented`、什么都不改，
  去掉这个成员重发即可。新的回答成员（`activity`、两个 `config_version`、`active.rows`、`stale_schema`、`below`）本版都不填。
  控制台发任何修订 3 的形状就要声明 `min` 3；第一个填新回答成员的版本会把 Runtime 的 `min` 升到 3。`ControlEmulatorInstance`、
  `SelfCheckInstance`、`ApplicationLifecycle` 改划为一般指令，只在握手时读，行为不变。runtime-client 的 `configure_settings`、
  `apply_resource_targets` 多了可选的 `expected_version` 参数；actingctl 和 MCP 的命令、参数都没变。
- 安装器（R-15a）：`actingctl install` 有了判 UI 接口声明的五条边（`actingd-config`、`ledger` 两条、`runtime-client`、
  `runtime-outcomes`），目前只有测试调用，更新和回退计划要到后续片才接入；图形向导显式传两个 Lab 旗标的约定只改了契约。

退回 v0.12.29 之前要做的事（按代码核对过）：

- 配置文件：本版没有新键，v0.12.29 照常读本版的配置文件。例外是配置文件自己的 `scheduling.rerun.condition_codes` 写了
  `contained_task_application_update_required`：v0.12.29 的目录里没有这个码，`check-config` 和启动以 `config_value_invalid`
  （`code_unknown`）拒绝整个文件。回退前先把这个码从文件里去掉并提交（`acsetup --commit-config`）。用指令设过这个键、值里有
  这个码的，v0.12.29 启动时作为 `config_override_dropped` 丢掉并列进 `instructions_dropped`，回退启动的 `instructions` 检查
  不通过；回退前先 `actingctl config clear scheduling.rerun.condition_codes`，或重设一个不含这个码的值。
- 种子和其他事实记录：不用做什么，v0.12.29 读得了。它们是普通的 `fact.published`（`fact.v1`，置信度 1000，不过期，观测时间
  不小于 1），账本的格式、表、索引和事件类型都没变，v0.12.29 照样打开本版写过的账本，把它们当普通的实例事实载入。v0.12.29
  不扣住、也不保留这个键：值为 true 的记录不再挡任何任务，只有点名这个键的规则会读它。再升回本版时记录照原样生效：值为 true
  的实例又被扣住，直到商店更新包运行成功。
- 用了 `result` 的资源根：v0.12.29 把整个游戏根记为 `standard_pack_root_invalid`，准备阶段拒绝每个要用这个根的运行，Runtime
  不停。回退前先把资源根换回不带 `result` 的版本并热放置，商店更新规则一起去掉：v0.12.29 读得了这条规则，但商店更新成功后不把
  事实写回 false，事实为 true 时它会每隔 `cooldown_ms` 再跑一次商店更新包。回退后客户端过期又要人手更新。
- 省略了界的资源根和回执（SS-3）：资源侧没省略任何界之前，准入回执逐字节和以前一样，v0.12.29 照常读，可以回退。资源侧第一次
  省略某个界之后就**不能退到 0.12.30 以下**，只能往前修：按它准入的运行写下的回执少了成员，v0.12.29 和更早的 Runtime 解不了。
  这种记录在账本打开时校验的尾部（`ledger_observer.open_verify_tail_records` 行）里时，以 `corrupt_ledger_record` 拒开账本；
  不在尾部的，后台复核读到它时记全局失败，以同一个码停下 Runtime。v0.12.29 也拒收省略了界的目录（那个游戏的 `schedule.json`
  记为 `standard_pack_root_invalid`，沿用当前生效代的规则）。0.12.30 之前的 Tools 和 UI 解码带这种回执的账本查询页
  （`PublicPayload.policy_admission`）也会失败。
- 改了的默认值和新码：v0.12.29 用它自己的默认表（9 个码）。已经以新码按条件终止结算的运行照记录重放；本版的新码不在 v0.12.29
  的目录里，读回时显示为 `outcome_relay_unregistered`（`raw_code` 是原码）。
- 只在程序里、不存状态的改动，回退后回到 v0.12.29 的行为：
  - UPD-1：扣住、空闲计划的例外、阶梯在新码处停下、开机包的处理、保留键和两条组装检查都没有了。
  - SS-1：只有实例持有生效中的非空 v2 目标表时才计基础估值；v1 目标的 `U` 又不算被禁用的点名任务。生产没有池写估值，派单不变。
  - R-0、R-0c、R-0d、R-15a：新请求种类、客户端方法、安装器的 UI 边都没有了；三个种类回到原来的分区，行为不变。
- 客户端和 Runtime 的配对：
  - 本版的客户端（`runtime-client` [2, 3]）连 v0.12.29 的 Runtime（[2, 2]）：照常。修订 2 的请求不变，`configure_settings`、
    `apply_resource_targets` 不带 `expected_version` 时线上字节和以前一样。只有发修订 3 的新种类时 v0.12.29 不认，断开连接、
    不给回执：`routine_summary`、`pack_schedule_view` 报 `runtime_operation_unsupported`，另外四个方法报连接断开。本版的
    actingctl 和 MCP 不发这些种类。v0.12.29 不保留 `health.application.update_required`，把它当普通事实键处理。
  - v0.12.29 的客户端连本版的 Runtime：照常。请求不变；本版不填任何新回答成员，严格的旧 DTO 照样解码；新码读到时显示为
    `outcome_relay_unregistered`；它写 `health.application.update_required` 会被拒（`application_update_key_reserved`）。资源侧
    省略界之后，它解码带少成员回执的账本查询页会失败（同上）。

运行上的变化：

- 次数上限可选（SS-3）：派单的次数、窗口和时长上限可以不写，不写的那一维不限；写齐的目录逐字节不变。
- 基础估值常开（SS-1）：池的基础估值在每个实例上都计入，资源层不读任务状态；生产没有池写估值，派单不变。
- 工作台形状（R-0）：工作台的请求和回答形状定下，新种类答 `not_implemented`；`runtime-client` [2, 3]；三个种类改划为一般指令。
- 安装器 UI 边（R-15a）：安装器有了判 UI 接口声明的判据，暂不接入计划；Lab 两个旗标由图形向导显式传入。
- 客户端方法（R-0c）：runtime-client 有了工作台的方法；新读面 `PackScheduleView`，本版答 `not_implemented`。
- 下层生效值（R-0d）：`PackScheduleView` 的每个包可带 `below`（下一层的决定），本版不填。
- 客户端过期（UPD-1）：弹窗页以 `contained_task_application_update_required` 结束运行，记下事实，扣住这台实例的其他任务，商店更新包成功后解除并先补跑被打断的任务。

本版合并的 PR：

- #1440 SS-3（#386）：次数上限（`loop_budget` 各项、活动档各项）改为可选；写齐的目录逐字节不变；`PolicyBudgetReceipt` 的成员成对可选，账本修订号不变。
- #1438 SS-1（#386）：池的基础估值在每个实例上都计入，不论有没有目标记录；资源层不读任何任务状态（单向规则）。生产没有池声明估值，线上不变。
- #1439 R-0（#429）：工作台的线上形状；新种类答 `not_implemented`；`runtime-client` 声明 [2, 3]，`min` 仍是 2；三个种类改划为一般指令。
- #1437 R-15a（#429）：安装器的 UI 方接口边（判据和测试），以及 Lab 两个旗标显式传入的约定。
- #1443 R-0c（#429）：runtime-client 的工作台方法；`PackScheduleView`（R-8 之前答 `not_implemented`）；契约收尾。
- #1444 R-0d（#429）：`PackScheduleView` 的包带 `below`。
- #1442 UPD-1（#428）：「客户端过期」弹窗以 `contained_task_application_update_required` 结束运行；只由宿主写的事实 `health.application.update_required`；扣住这台实例的非商店任务；商店更新包运行；更新后补跑被打断的任务。改了的默认值：这个码加进 `scheduling.rerun.condition_codes`。

## v0.12.29

本版是 v0.12.28 之后合并的 5 个 PR：CFL-1、STORE-FG-1、CFL-2、STORE-FG-2、LAB-WAIT。运行中会注意到的新东西有两件。
一是商店更新包：包可以在 `control.json` 里写 `store_update`，在官方应用商店处于前台时点它声明过的商店步骤，用来更新实例上
的客户端（STORE-FG-1）；但只有配置里的允许名单 `store_foreground.allowed_application_ids` 写了这个商店才行。名单默认是空
的，所以在操作员把商店写进配置文件并重启之前，每个商店更新包的运行都在截图和输入之前以 `application_store_not_allowed`
被拒，设备上什么都不发生（STORE-FG-2）。二是识别预处理：包的 OCR 目标可以写 `preprocess`，先对目标区域跑一条固定的处理
链（保留颜色、颜色范围、反转、灰度、对比度、二值化），再把处理后的区域交给 OCR（CFL-1、CFL-2）；没写的目标逐字节照旧读。
按约定，生产资源包在真实画面验收（CF-RF1）之前不用它。另外，离线 `observe` 能加载 `to` 是页面集合的等待步骤了
（LAB-WAIT）。不写这两个新成员的包、不改配置的实例，运行和 v0.12.28 一样。v0.12.28 已在本版之前发布。

配置：`actingd-config` 升为 revision 21，本版声明 [5, 21]，仍读 revision 5 到 20 的配置文件；`runtime-client` 仍是
[2, 2]，账本仍是 [3, 4]，`package` 仍是 [3, 5]，其他接口不变。

- 新键 `store_foreground.allowed_application_ids`（STORE-FG-2）：商店更新包可以在其中点击的商店应用，0 到 16 个不重复的
  Android 包名；默认 `[]`（在 `actingd.defaults.json` 里），即不允许任何商店。不可在运行时设置（`ConfigureSettings`、
  `ac_config_set` 答 `config_key_not_settable`），任何代理都放宽不了它：只有人改配置文件，下次启动 Runtime 时生效。Runtime
  只按包名逐字比较，代码和默认值里不写任何商店。
- 这个键写错时，启动和 `check-config` 都以 `config_value_invalid` 拒绝并点名它：值不是列表或某项不是字符串
  （`wrong_type`）；超过 16 项或某项不是 Android 包名（`out_of_range`）；某项重复，或某项等于某个已配置实例自己的
  `application_id`（`section_check`，游戏不会是自己的商店）。
- 改了的默认值：没有。`actingd.defaults.json` 只多了 `store_foreground` 一节（`"allowed_application_ids": []`），已有的
  默认值一个都没改，现有的配置都不需要修改。
- 账本没有新表、新索引或新事件类型，格式不变；本版写的新记录都是已有的记录类型（见下面）。
- 新结果码 3 个：`application_store_not_allowed`（STORE-FG-2，error，阶梯类 `declaration_error`）、
  `package_bundle_store_tap_unlocated`（STORE-FG-1，warning）、`package_bundle_preprocess_divergent`（CFL-2，warning）。新位置
  1 个：`admit_store_foreground`（STORE-FG-2）。已有的 `contained_task_program_invalid` 的 `program_problem` 多了三个原因词：
  `store_operation_invalid`、`store_proof_invalid`、`store_way_back_missing`（STORE-FG-1）。它们的评审状态都是 pending，结果
  码发布文件只收已定稿或已退役的条目，所以本版的发布文件是空的。没有退役的码。
- 两个包成员：`control.json` 的 `store_update` 和 OCR 目标的 `preprocess` 都归 package 接口 revision 6（由 B 列车的收尾片
  BT-PK6 声明），在那之前 Runtime 按代码接纳它们，所以 `package` 仍是 [3, 5]。`preprocess` 只写在任务 schema 0.9 的
  `ocr_targets[]`、`candidate_layouts[].shared_ocr` 上，派生到识别包 schema 0.7 的 OCR 目标和 `shared_ocr`（两个 schema
  都是已有的）；更早的 schema 拒收（`UnconsumedField`）。

升级时要知道的：

- 新成员要本版或以上（STORE-FG-1、CFL-2）：v0.12.28 及更早的 Runtime 在准备阶段拒收写了 `store_update` 的包（`control.json`
  的 `/store_update`）和写了 `preprocess` 的包（识别包的 `/targets/<i>/preprocess`），都是 `resource_declaration_invalid`
  （`UnknownField`），要用这个包的每个运行都被拒，Runtime 不停；v0.12.28 的 Tools 打包时也一样拒收。所以资源仓要等本版部署并
  核过之后，在同一个 PR 里把 CI 钉的 Tools 换到 0.12.29 或以上，再热放置。不写这两个成员的包，本版 Tools 派生出来的文件逐
  字节和以前一样。
- 商店不在允许名单里（STORE-FG-2）：商店更新包自己的运行（直接运行或计划运行）在开跑检查、任何截图和输入之前先核名单。名单
  没写它声明的商店时，运行以 `application_store_not_allowed` 结束（error，阶梯类 `declaration_error`，回执 `denied`，带
  `alias` 和声明的商店 `application_id`，位置 `admit_store_foreground`），记为这次运行的失败终态，什么都不点。默认配置的阶梯
  触发项都不匹配这个阶梯类，所以默认不开阶梯。每次商店点击在读前台之前还会再核一次名单（位置
  `require_foreground_application`，和其他被拒的输入一样记 `command.rejected` 和 `runtime.failed`）。处理：确认是官方应用
  商店之后，把它的包名写进配置文件的 `store_foreground.allowed_application_ids` 并重启 Runtime；不是的话改正包里的
  `store_application_id`。
- 商店点击的前台（STORE-FG-2）：只有包声明的商店步骤的普通点击（`Tap`）要求前台正好是声明的商店，而且名单写了它。前台是游戏
  （商店已经关了）、别的应用（登录页、浏览器）或没有活动页时，这次点击照已有的规则被拒：`application_not_foreground`（细节里
  `expected=` 是商店）或 `application_foreground_unknown`；对上时记一次 `application.foreground` 为商店。包里的其他步骤（游戏
  自己的更新对话框）、其他包、层、Lab 操作和客户端 `Input` 的所有输入照旧要求前台是实例分配的应用。真机开跑检查在
  `device_state.in_use_gate` 下不把这次运行声明的商店算作别的应用（`device_in_use`），其他运行不变。商店运行失败时 Runtime
  不自己把游戏拉回前台：计划运行的失败照恢复阶梯的触发项处理，直接运行由发起方 `ac_app restart`。允许只对这一次运行的商店点击
  有效，不存任何事实、标志或实例状态。
- 商店更新包从不当层用：作前置、回首页、导航边、恢复链、打断处理或 `then.run` 包时以 `reason=store_update` 拒绝；作启动包时
  以 `startup_package_admission_failed`（`reason=store_update`）拒绝；也不当阶梯级。
- 商店更新包的写法（STORE-FG-1，给包作者）：只用于 `navigable_route` 或 `linear_steps` 的包；每个商店步骤都是从包里的一个页
  （不是 `any`）出发、带着该页守卫的点击（不能是 `unguarded_trusted_coordinate`）；每个证明目标是带 `expected` 的 OCR 目标，
  直接写在出发页的 `required` 里；包里还要有一个 `application` 的 `launch` 或 `restart` 步骤回到游戏。违反的在准入时以
  `contained_task_control_invalid`（`reason=store_update_invalid`）或 `contained_task_program_invalid`（上面三个原因词之一）
  拒收。商店步骤点固定位置（`point`、`rect`、`specific_rect`）时，`package bundle` 和 `standard-pack check` 报
  `package_bundle_store_tap_unlocated` 警告，不拒收。等商店更新用等待步骤或确认的 `wait_page`，`max_ms` 最多 1800000，并且要
  留在运行的 `timeout_ms` 之内。
- 识别预处理（CFL-1、CFL-2，给包作者）：一条链 1 到 4 步，算法版本 `preprocess.v1`；语法、算术、证据和金样见契约
  `recognition-preprocess.md`。`gray` 一步和场景灰度逐字节相同（用的是场景同一个转换），其他步骤都是整数运算。带链的目标只把
  处理后的区域交给 OCR；整帧的目标带链必须写 `locate`；共享读只服务链完全相同的目标、不收 `otsu`，链不同的目标自己读，
  `package bundle` 和 `standard-pack check` 报 `package_bundle_preprocess_divergent` 警告。处理链跑完之后按采样期限再核一次，
  不够时是已有的采样预算不足错误，不调 OCR；存档帧 OCR 的超时扣掉处理链用的时间。
- 识别证据（CFL-2）：处理链的证据（`preprocess`）和共享读的证据（`shared_read`）只出现在只读识别、存档帧 OCR 报告、观测的
  `candidate_sample` 行和 Lab 输出里，从不进账本，任务诊断的目标行和 OCR 行不变。由共享读服务的观测行不论有没有链都多一个
  `shared_read`（每行约 90 字节）；目前各资源仓都没有用 `shared_ocr`。
- 等待步骤（LAB-WAIT，给包作者）：只改 Tools。`to` 是页面集合、没有 `click` 的等待步骤（有 `wait`，没有 `click`、
  `application`、`select`），离线 `observe` 现在能加载，不把它列为动作，它的目的页和内核给出的一样；以前它报
  `missing field 'click'`，只能改写成 `expect_after.page_id`，那种写法照样可用。Runtime 和内核一直接受这种写法，所以不用等
  Runtime 部署，CI 钉的 Tools 换到 0.12.29 或以上就行。同时写了 `wait` 和 `click` 的步骤仍按点击步骤读（内核拒收为
  `wait_member_refused`）；`to` 是页面集合的点击步骤仍然必须有 `click`。

退回 v0.12.28 之前要做的事（按代码核对过）：

- 配置文件：v0.12.28 只认 [5, 20]，配置文件拒绝不认识的成员。配置文件只要写了 `store_foreground` 节（哪怕写的是默认的
  `[]`），v0.12.28 的 `check-config` 和启动都会拒绝整个文件，回退闸门的 `config` 部分也不通过。回退前先把这一节从配置文件里
  删掉并提交（`acsetup --commit-config`）；没写这一节的文件 v0.12.28 照常读。这个键不可设置，没有运行时指令要清。
- 用了 `store_update` 或 `preprocess` 的包：v0.12.28 在准备阶段以 `resource_declaration_invalid`（`UnknownField`）拒收这个
  包，要用它的每个运行都被拒（写了 `preprocess` 的包被别的运行当作层用时，那些运行也被拒），Runtime 不停。回退前先把资源根换
  回不带这两个成员的版本并热放置。回退后 Runtime 做不了商店更新（v0.12.28 的前台闸门拒绝商店在前台时的任何点击），需要更新
  客户端时由人手动更新。
- 账本里的新记录：不用做什么。账本的格式、表、索引和事件类型都没变，v0.12.28 照样打开本版写过的账本。新记录都是已有类型：
  `application_store_not_allowed` 的运行终态和被拒点击的 `command.rejected`、`runtime.failed`；商店点击记下的
  `application.foreground`（值是商店的包名，回退后下一次普通点击会记回分配的应用）；`contained_task_program_invalid` 的新原因
  词和 `startup_package_admission_failed` 的 `reason=store_update`。本版的新码不在 v0.12.28 的目录里，它读回时显示为
  `outcome_relay_unregistered`（`raw_code` 是原码）。识别证据不进账本。
- 只在程序里、不存状态的改动，回退后回到 v0.12.28 的行为：
  - STORE-FG-2：允许名单、商店点击的前台判断和开跑检查的例外都没有了。
  - CFL-1、CFL-2：没有预处理，OCR 目标只按原帧读。
  - LAB-WAIT 和两个打包警告在 Tools 里：回退 Tools 后，离线 `observe` 又对 `to` 是页面集合的等待步骤报
    `missing field 'click'`。
- 请求成员：本版没有新增请求类型或请求成员，`runtime-client` 仍是 [2, 2]。新增的只是一个不可设置的配置键、两个包成员和识别
  证据里的成员（`preprocess`、`shared_read`，只在用了链或共享读时出现）。
- 客户端和 Runtime 的配对：
  - 本版的客户端连 v0.12.28 的 Runtime：照常。请求不变；对它设 `store_foreground.*` 会被当作未知键拒绝（`config_key_unknown`，
    本版的 Runtime 答 `config_key_not_settable`）；LAB-WAIT 和两个警告在 Tools 自己里，不依赖 Runtime。
  - v0.12.28 的客户端连本版的 Runtime：照常。请求不变；`application_store_not_allowed` 不在它的目录里，读到时显示为
    `outcome_relay_unregistered`；识别证据里的新成员只在包用了链或共享读时才出现，现在的包都没用。

运行上的变化：

- 识别预处理（CFL-1）：识别库多了一条纯函数的预处理链和它的契约，本身不接任何读取。
- 商店更新包的声明（STORE-FG-1）：`control.json` 可以写 `store_update`；准入核对商店步骤、证明目标和回到游戏的步骤；商店步骤点固定位置时报 `package_bundle_store_tap_unlocated`。
- 预处理接进 OCR（CFL-2）：OCR 目标和共享读可以写 `preprocess`，只把处理后的区域交给 OCR；目标和共享读的链不一致时报 `package_bundle_preprocess_divergent`。
- 商店允许名单（STORE-FG-2）：商店更新包只在名单写了它的商店、前台正好是这个商店时点商店步骤，否则 `application_store_not_allowed` 或 `application_not_foreground`；名单默认空。
- 等待步骤（LAB-WAIT）：离线 `observe` 能加载 `to` 是页面集合的等待步骤。

本版合并的 PR：

- #1429 CFL-1（#428）：识别预处理的纯函数链（`preprocess.v1`，六种步骤）和契约 `recognition-preprocess.md`；只是库，不接读取。
- #1432 STORE-FG-1（#428）：`control.json` 的 `store_update`：包声明官方应用商店在前台时它执行的步骤；新的 `program_problem` 原因词；新警告 `package_bundle_store_tap_unlocated`；更早的 Runtime 拒收写了它的包。
- #1433 CFL-2（#428）：预处理接进 OCR 读取；新的可选包成员 `preprocess`（任务 schema 0.9、识别包 0.7）；新警告 `package_bundle_preprocess_divergent`；生产包在真实画面验收 CF-RF1 之前不用。
- #1434 STORE-FG-2（#428）：新键 `store_foreground.allowed_application_ids`（默认空，不可在运行时设置），`actingd-config` revision 20 → 21；新结果码 `application_store_not_allowed`；声明的商店步骤只在前台正好是声明的商店时点击；作启动包时以 `reason=store_update` 拒绝。
- #1436 LAB-WAIT（#428）：actinglab 的离线 `observe` 能加载 `to` 是页面集合的等待步骤。

## v0.12.28

本版是 v0.12.27 之后合并的 10 个 PR：SR-92、UNSET-1、RB-2、RB-1、B35、B43、EMU-BIND、LAB-SMALL、WD-MT、ISO-PE。运行中
会注意到的：10-29 11:19 那种长时间停机不会再出现。当时一轮策略评估因为一项输入不合法（`policy_evaluation_input_invalid`）
结束了策略驱动线程，整个 Runtime 随即停下，所有实例都停了；看门狗又从目录项读到过时的日志时间（Runtime 拉起的 adb server
继承并一直开着它的日志），把这次出错的停机判成正式关闭，35 分钟没有重启。本版里，不是全局性的评估失败只跳过这一轮，记
一条 `policy_evaluation_skipped`，默认 15 秒后再评，连续 3 次再记一条 `policy_evaluation_awaiting_person` 等人看，Runtime
一直运行（ISO-PE）；看门狗改按日志文件本身的时间判断，actingd 拉起的子进程也不再继承它的日志（WD-MT），出错的停机会在
看门狗下一次检查时重启。整机重启后由看门狗拉起的 Runtime 不再以低内存优先级和低 I/O 优先级运行（RB-2）；冷启动后列运行
和概况的运行块不再因为每页重读整段历史的 Lab 关系而连续几小时失败（RB-1）。启动模拟器时就绪等待里一次 `info` 读取失败
不再判启动失败，模拟器已经在跑、端点还是 pending 的实例由监视识别并绑定，不用人手（EMU-BIND）。被一单结算不了的运行在
每次启动时重新扣住的实例，现在只在第一次扣（UNSET-1）。操作员暂停或部署排空打断的计划运行按条件终止结算，不再算任务
失败（SR-92，默认值有变）。资源侧可以给共享打断页声明 `limit_cause`，但要等本版部署之后（B43）。v0.12.27 已在本版之前
发布。

配置：`actingd-config` 升为 revision 20，本版声明 [5, 20]，仍读 revision 5 到 19 的配置文件；`runtime-client` 仍是
[2, 2]，账本仍是 [3, 4]，其他接口不变。

- 新键 `host.process_memory_priority`（RB-2）：`normal`（默认，在 `actingd.defaults.json` 里）或 `low`。不可在运行时设置
  （账本打开之前就用了，`ConfigureSettings` 答 `config_key_not_settable`），每次启动读一次；要改就改配置文件，下次启动生效。
- 新键 `host.process_io_priority`（RB-2）：同上，`normal`（默认）或 `low`。两个键写了别的值，启动和 `check-config` 都以
  `config_value_invalid` 拒绝并点名这个键。
- 新键 `scheduling.evaluation.retry_ms`（ISO-PE）：默认 `15000`，1 到 3 600 000 毫秒，可在运行时设置：跳过一轮评估之后隔
  多久再评。
- 新键 `scheduling.evaluation.escalate_after`（ISO-PE）：默认 `3`，至少 1，可在运行时设置：连续跳过多少轮记一次
  `policy_evaluation_awaiting_person`。这两个键在每次跳过时读，指令从下一次跳过起生效。
- 改了的默认值（SR-92，Alice 的裁定 92）：`scheduling.rerun.condition_codes` 末尾加了 `contained_task_paused`，从 8 个码
  变成 9 个，原来的 8 个不变（见 v0.12.27 的说明）。部署计划的默认值对比会列出这一项和上面四个新键。配置文件自己写了
  `condition_codes` 的按文件里的值，这次改动对它不起作用，需要的话自己加上这个码；用指令设过这个键的，指令照样恢复，在
  `instructions` 检查的 `base_changed` 里点名，不算失败。
- 其他默认值都没改，现有的配置都不需要修改。
- 账本没有新表、新索引或新事件类型，格式不变；本版写的新记录都是已有的记录类型（见下面）。
- 新结果码 7 个：`policy_evaluation_skipped`、`policy_evaluation_awaiting_person`（ISO-PE）、`host_process_priority_applied`、
  `host_process_priority_failed`（RB-2）、`host_stdio_inheritance_failed`（WD-MT）、
  `contained_task_interrupt_environment_limit_reached`（B43）、`package_bundle_interrupt_page_shadowed`（B35）。新位置 8 个：
  `read_unsettled_dispatch_records`（UNSET-1）、`build_lab_relations`（RB-1）、`process_memory_priority_set`、
  `process_memory_priority_read`、`process_io_priority_set`、`process_io_priority_read`（RB-2）、
  `process_stdout_inheritance_clear`、`process_stderr_inheritance_clear`（WD-MT）。它们的评审状态都是 pending，结果码发布
  文件只收已定稿或已退役的条目，所以本版的发布文件是空的。没有退役的码。已有的码只加了可选值：`arguments_invalid` 的
  `req_id`、`policy_evaluation_input_invalid` 的输入说明、`policy_settlement_dispatch_left_open` 的 `failure_code`，以及
  模拟器自动启动和监视的七个码的 `watch_kind`。

升级时要知道的：

- 进程优先级（RB-2）：从 v0.12.27 升级不用做什么。每次启动读完配置、打开账本之前，actingd 把自己进程的内存优先级和 I/O
  优先级设成两个键的级别并读回；CPU 优先级类不动（看门狗拉起时仍低于正常）。启动日志里随即多一行 `OUTCOME actingd:`：
  `host_process_priority_applied`（info），带 `memory_priority_requested`、`io_priority_requested`（Windows 的级别，默认
  normal 是 5 和 2）和读回的 `memory_priority_read`、`io_priority_read`；有调用失败或读回的级别不对时是
  `host_process_priority_failed`（warning），每个失败的调用一条 `caused_by` `foreign_os_error`，启动照常。这一行不进账本，
  也不进概况，要核实就看启动日志开头。I/O 优先级用的是没有文档的原生调用，以后的 Windows 拒绝它时也只是这条警告。
- 跳过的评估（ISO-PE）：账本里记的是已有类型 `runtime.failed` 的两种记录，同时写成 daemon 行。
  `policy_evaluation_skipped`（warning）带 `count`、`limit_count`（即 `escalate_after`）、`retry_after_ms`、
  `next_eligible_unix_ms`，`caused_by` 是这次评估的失败：策略的 `policy_evaluation_input_invalid` 和被拒的那项输入
  （`policy_input`、`policy_input_problem`，以及 `fact_key`、`instance_id`、`task_id` 或 `pool_id`；时间检查还有
  `observed_at_unix_ms`、`evaluated_at_unix_ms`），连同策略自己的说明。连续跳过到 `escalate_after` 次时再记一次
  `policy_evaluation_awaiting_person`（error）。连续次数只记在内存里，评估成功一次或重启就清零。
  - 跳过期间所有实例都不从策略得到新的派发（策略一次校验全部输入），已经交给实例工作线程的运行照常跑完。
  - 概况里还看不到跳过和等人的状态（后续片）；在那之前，每次部署后查一下账本里有没有新出现的这两个码。
  - 仍会停下 Runtime 的只有全局失败：账本读取失败、锁中毒、时钟出错、生效配置读不出、共享投影和账本对不上，以及已经锁存
    的条件。
  - 评估时间改在拿到两道闸门之后取样，并行结算不会再让评估把自己的输入判成“晚于评估时间”。投影还没就绪
    （`outcome_projection_not_ready`）时，没有重算唤醒的触发也不再结束驱动线程，而是等一个去抖间隔，最多连续
    `escalate_after` 次，之后按跳过处理。
- 日志时间和句柄继承（WD-MT）：看门狗和 `actingctl install`（读停机结果、回退闸门给启动日志定时间）都按日志文件本身的修改
  时间判断，不再读目录项；读不到时报 `watchdog_log_unreadable`。actingd 启动的第一步清掉自己 stdout、stderr 的继承标志：
  成功时什么都不写，失败时写一行 `host_stdio_inheritance_failed`（warning），启动照常。本版部署之前已经被继承的句柄收不
  回来：以前的 actingd 拉起的 adb server 会一直开着那时的日志，直到 adb server 或整机重启；因为改成读文件本身的时间，这
  不影响判断。部署这一次的停机如果由旧版安装器读结果，仍可能报一次 `stop_terminal_unconfirmed` 提示。
- 识别在跑的模拟器（EMU-BIND）：从 v0.12.27 升级不用做什么。
  - 监视读到模拟器在跑、Android 已起、有 ADB 端口，而这个实例的端点是 pending 或绑在别的端口上时，在该实例自己的队列上排
    一个识别认领（`emulator_autostart_queued`，`watch_kind` 为 `recognize`），暂停与否都排：绑定上报的端口
    （`runtime.instance_bound`），等 ADB 基线，记 `device.connected`，打开输入和截图，再像启动之后那样排启动包。它从不
    启动或关闭模拟器。
  - 已绑定但没准备好的实例（复核 M1）：实例已经绑在上报的端口上，却没有任何 `backend.selfcheck.*` 事实（Start 绑了端口
    之后在 ADB 基线或期限上失败），它的钥匙空着，也没有被空闲动作、`released` 阶段的暂停或实例隔离（fence）扣着，监视
    也用同样的认领识别它一次：跑一次自检，启动包只排一次，不发 Start。升级前在 v0.12.27 下留成这样的实例，升级后在不再
    被扣着时的第一次读取就会被识别。
  - 识别次数和重启一样受 `restart_attempts` / `restart_window_ms` 限制，分开计数；用尽时记一次
    `emulator_watch_restart_exhausted`（`watch_kind` `recognize`），处理是 `actingctl selfcheck`。
  - 启动的就绪等待里 `info` 读取被拒只算没有读到，继续等到 120 秒期限；`launch_err_code` 不是 0 仍立即失败；失败记录带上
    被拒那次读取的输出。
- 只扣一次（UNSET-1）：启动结算不了一单计划运行时，先按这单的运行和任务链接读一次它以前的 `runtime.failed`，读到第一条
  `policy_settlement_dispatch_unsettled` 为止。以前记过：改记已有的 warning `policy_settlement_dispatch_left_open`（带
  `failure_code`），不再暂停实例，还在的暂停保持原样；没记过：照旧记 error 并扣住实例。v0.12.26、v0.12.27 记下的也算，
  所以升级后的第一次启动就不会再为这种旧运行扣住实例。读取超时或超出读取预算时照旧扣住，记录里用 `note` 写明原因。放开
  扣住之前先看画面。这种运行本身怎么结算留给后续片（UNSET-2）。
- 暂停打断（SR-92）：从本版起，非敏感任务的计划运行被操作员暂停（`ac_pause`、`actingctl pause`）或部署排空打断时，按
  v0.12.27 说明里的条件终止结算：退回次数、不加连败、记下欠补跑，恢复后补跑先等节奏段、到时段终点过期。敏感任务仍是
  普通失败。
- `limit_cause`（B43）：`interrupts.json` 的打断项可以写可选成员 `limit_cause`：`"pack_logic"`（缺省，同以前）或
  `"environment"`。
  - 声明 `environment` 的页在一次运行里超过自己的 `max_per_run` 时，运行以
    `contained_task_interrupt_environment_limit_reached` 结束：任何范围都不累计，只重跑，不会因为重复失败暂停这对任务。
  - 它的阶梯类是 `budget_or_permission`，默认配置没有触发项匹配，所以默认不开阶梯；要阶梯就在配置里加一个点名这个码的
    触发项。
  - 一次运行里所有页合计第 17 次处理仍以 `contained_task_interrupt_limit_reached` 结束。
  - v0.12.28 之前的 Runtime 不认这个成员，会拒收写了它的整个游戏根。所以资源仓要等本版部署并核过之后，在同一个 PR 里把
    CI 钉的 Tools 换到 0.12.28 或以上，再热放置；放置之后，要回退到 0.12.28 以下就得先把资源根换回去（见回退）。
- 共享打断页被包挡住（B35）：`standard-pack check` 和带 `--interrupts` 的 `package bundle` 对每个声明了共享打断页的注册表
  包报一条 `package_bundle_interrupt_page_shadowed` 警告（`package_id`、`page_id`），只警告不拒收；`pages.json` 的特征包
  和该页自己的处理包不报。资源仓 CI 钉的 Tools 换到 0.12.28 以上之后才会看到。
- Lab 关系索引（RB-1）：读者池每次启动后在后台用自己的读连接建一次 Lab 关系索引，期限是 `ledger_observer.open_timeout_ms`；
  建好之前各页照以前那样读，建好之后每页只读索引之后新增的记录。建不成只是一条 warning（daemon 行
  `actingd ledger reader_pool lab_relations`，位置 `build_lab_relations`），不致命，各页照以前那样读到下次启动。索引只在
  内存里。
- Lab（LAB-SMALL）：只改 Tools 和 MCP。离线 `observe` 遇到缺 OCR 或 NN 模型的布局只跳过这个布局，在
  `candidate_not_projected` 里列出；`--targets` 的每项带 `target_center`；`--with-frame` 的帧文件在答复成立之后才写；
  `observe` 的 `standby` 恒为 `false`，认没认出页面看 `matched` 和 `state`；`ac_lab_do` 在 actinglab 运行之前给出的
  `arguments_invalid` 带一个 `req_id`（不记录任何东西）。升级时 MCP 入口还没切到本版子进程之前，旧子进程没有这个
  `req_id`。

退回 v0.12.27 之前要做的事（按代码核对过）：

- 配置文件：v0.12.27 只认 [5, 19]，配置文件和 `scheduling` 节都拒绝不认识的成员。配置文件只要写了 `host` 节（哪怕写的是
  默认的 `normal`）或 `scheduling.evaluation`，v0.12.27 的 `check-config` 和启动都会拒绝整个文件，回退闸门的 `config`
  部分也不通过。回退前先把它们从配置文件里删掉并提交（`acsetup --commit-config`）；两样都没写的文件 v0.12.27 照常读。
- 运行时指令：用 `config set` 或 `ac_config_set` 设过 `scheduling.evaluation.retry_ms` 或 `escalate_after` 的，指令记在
  `host.config_overrides` 里。v0.12.27 不认识这两个键，启动时作为 `config_override_dropped` 丢掉（只是警告，启动照常），
  并列进 `instructions_dropped`，所以回退启动的 `instructions` 检查不通过。回退前先执行
  `actingctl config clear scheduling.evaluation.retry_ms` 和 `actingctl config clear scheduling.evaluation.escalate_after`。
  `host.*` 不可设置，不会有指令。设过 `scheduling.rerun.condition_codes` 的指令照样恢复（`config_override_base_changed`），
  不算失败。
- 改了的默认值（SR-92）：v0.12.27 用它自己的默认表（8 个码）。回退后被暂停或部署排空打断的计划运行又按普通失败算：扣
  次数、加连败，可能暂停这对任务。已经按条件终止结算的照记录重放；L7 里已经欠下的、`failure_code` 为
  `contained_task_paused` 的补跑，v0.12.27 照样提前、等节奏段、到时段终点过期（它不按码表筛欠着的补跑）。
- 用了 `limit_cause` 的资源根（B43）：v0.12.27 启动时把写了它的整个游戏根记为 `standard_pack_root_invalid`（`check-config`
  也列出），准备阶段拒绝每个要用这个根的运行，Runtime 不停。回退前先把资源根换回不带 `limit_cause` 的版本（或删掉这个
  成员）并热放置。已经以新码结算的运行照记录；v0.12.27 若要按终态补结算一条带新码的运行，会按旧规则（码以
  `contained_task_interrupt_` 开头）在准备阶段以外累计。
- 账本里的新记录：不用做什么。账本的格式、表、索引和事件类型都没变，v0.12.27 照样打开本版写过的账本。新记录都是已有
  类型：ISO-PE 的 `runtime.failed`，UNSET-1 的 `policy_settlement_dispatch_left_open`（多一个可选值），EMU-BIND 的认领和
  绑定记录（多一个 `watch_kind`），B43 的运行终态。本版的新码不在 v0.12.27 的目录里，它读回时显示为
  `outcome_relay_unregistered`（`raw_code` 是原码）。RB-2、WD-MT 的那几行只写在启动日志里。
- 只在程序里、不存状态的改动，回退后回到 v0.12.27 的行为：
  - ISO-PE：不是全局的评估失败又会结束策略驱动线程并停下整个 Runtime（11:19 那种情况）。
  - WD-MT：子进程又会继承 actingd 的日志；看门狗又读目录项的时间，日志被别的进程开着时可能把出错的停机判成正式关闭、
    不重启，要人手启动。
  - RB-2：看门狗拉起的 actingd 又以低内存优先级和低 I/O 优先级运行。
  - RB-1：每页又重读整段历史的 Lab 关系，冷缓存下列运行和概况的运行块可能连续失败（`ledger_read_budget_exceeded`）。
  - EMU-BIND：就绪等待里一次 `info` 读取失败又判启动失败；在跑但端点 pending 的实例没人绑定，只能手动
    `ac_emulator start`。识别时记下的绑定是已有的 `runtime.instance_bound`，v0.12.27 照常读。
  - UNSET-1：结算不了的旧运行又会在每次启动时扣住它的实例；每次重启后看一下有没有这样被扣住的实例，看过画面再恢复。
  - B35、LAB-SMALL：Tools 和 MCP 回到旧的行为。
- 请求成员：本版没有新增请求类型或请求成员，`runtime-client` 仍是 [2, 2]。新增的只是可设置的配置键（指令的取值）和
  Lab、MCP 答复里的成员（`target_center`、`candidate_not_projected`、拒绝里的 `req_id`）。
- 客户端和 Runtime 的配对：
  - 本版的客户端连 v0.12.27 的 Runtime：照常。请求不变；对它设 `scheduling.evaluation.*` 会被当作未知键拒绝
    （`config_key_unknown`）；Lab 的改动都在 Tools 和 MCP 自己里，不依赖 Runtime。
  - v0.12.27 的客户端连本版的 Runtime：照常。请求不变；本版的新码它的目录里没有，读到时显示为
    `outcome_relay_unregistered`；共享投影的 `standby` 在没认出页面时是 `false`，成员和形状不变。

运行上的变化：

- 条件码默认值（SR-92）：`contained_task_paused` 进默认码表，操作员暂停或部署排空打断的计划运行按条件终止结算。
- 只扣一次（UNSET-1）：结算不了的同一单运行只在第一次扣住实例，之后的启动记 `policy_settlement_dispatch_left_open`、不暂停。
- 进程优先级（RB-2）：actingd 启动时把自己的内存和 I/O 优先级设成 `host.*` 的级别（默认 normal），写一行 daemon 行。
- Lab 关系索引（RB-1）：读者池在后台建一次 Lab 关系索引，各页只读索引之后新增的部分。
- 打断页警告（B35）：`standard-pack check` 和 `package bundle` 对挡住共享打断页的包报 `package_bundle_interrupt_page_shadowed`。
- 环境类打断上限（B43）：`limit_cause` 为 `"environment"` 的页到上限以新码结束，只重跑、不累计暂停。
- 识别在跑的模拟器（EMU-BIND）：监视绑定端点 pending 的在跑模拟器，识别已绑定却没自检的实例；就绪等待里 `info` 读取失败继续等。
- Lab 小修（LAB-SMALL）：离线 `observe` 跳过缺模型的布局、`target_center`、帧文件晚写、`standby` 恒为 `false`、`ac_lab_do` 拒绝带 `req_id`。
- 日志时间（WD-MT）：看门狗按日志文件本身的时间判断；actingd 的子进程不再继承它的日志。
- 评估失败（ISO-PE）：不是全局的评估失败跳过这一轮、`retry_ms` 后重评、连续 `escalate_after` 次等人，Runtime 不停；细节保留；评估时间的竞态已修。

本版合并的 PR：

- #1421 SR-92（#386）：`contained_task_paused` 加进 `scheduling.rerun.condition_codes` 的默认值（裁定 92），被暂停或部署排空打断的计划运行按条件终止结算；敏感任务仍是普通失败。
- #1423 UNSET-1（#421）：结算不了的同一单计划运行只在第一次启动时扣住实例，之后记 `policy_settlement_dispatch_left_open`（带 `failure_code`）、不暂停；新位置 `read_unsettled_dispatch_records`。
- #1424 RB-2（#421）：actingd 启动时把自己的内存和 I/O 优先级设回正常，新键 `host.process_memory_priority`、`host.process_io_priority`，`actingd-config` revision 19 → 20；新结果码 `host_process_priority_applied`、`host_process_priority_failed`。
- #1425 RB-1（#422）：读者池的页改从后台建好的 Lab 关系索引读；新位置 `build_lab_relations`。
- #1426 B35（#428）：`standard-pack check` 和 `package bundle` 对声明了共享打断页的包报警告；新结果码 `package_bundle_interrupt_page_shadowed`。
- #1427 B43（#428）：打断项的可选成员 `limit_cause`，`environment` 的上限只重跑、不累计暂停；新结果码 `contained_task_interrupt_environment_limit_reached`；更早的 Runtime 拒收写了它的根。
- #1422 EMU-BIND（#421）：监视识别并绑定端点 pending 的在跑模拟器，也识别已绑定却没准备好的实例（M1）；就绪等待里 `info` 读取失败继续等；失败带上读取的输出。
- #1428 LAB-SMALL（#428）：B47，加上 Lab 的 P57、P58、P60、P61。
- #1430 WD-MT（#421）：看门狗按日志文件本身的时间判断；actingd 的子进程不再继承它的 stdout、stderr；新结果码 `host_stdio_inheritance_failed`。
- #1431 ISO-PE（#419）：不是全局的策略评估失败不再停下 Runtime：跳过这一轮、重试、连续 N 次后等人；修掉评估时间的竞态；保留错误细节；新键 `scheduling.evaluation.retry_ms`、`escalate_after`（revision 20）；新结果码 `policy_evaluation_skipped`、`policy_evaluation_awaiting_person`。

## v0.12.27

本版是 v0.12.26 之后合并的 6 个 PR，都是 0.12 第二波：CAP-IDX、P32-B1、SR-2b-1、ISO-5a、IDLE-WG、SR-2b-2。运行中会注意
到的：宿主满载时，采样超时、采样预算不够这类负载造成的失败（默认码表的八个码，见下面）不再算任务失败，而是“条件终止”：退回
这次占用的次数，不加连败，记下一次欠着的补跑（SR-2b-1）；欠着的补跑等实例的节奏段（pacing stage）回到 S2 及以下并稳定
60 秒后，排在本实例其他任务前面先跑，到活动时段结束还没跑就过期，不顺延到第二天（SR-2b-2）。同一时段里连续第三次条件
终止按普通失败结算，并记一条 `policy_rerun_exhausted`。`actingd suspended` 和 `ac_diagnose` 多了 `reruns`，列出欠着、
在等和已过期的补跑。运行收尾、租约持有者收尾的任何一步都不再让 Runtime 停下，收尾时租约会在宽限期内到期就先续一次，
账本里会多看到续租记录（ISO-5a）。模拟器重开后端口顺延到下一个时，截图按绑定里的编号打开，不再打不开（CAP-IDX）。空闲
实例只为此刻真能派出去的任务叫醒，不再为被活动时段或黑名单挡着的任务白跑一次启动包（IDLE-WG）。运行列表的速度没有变：
P32-B1 只定义了运行汇总的契约和折叠函数，Runtime 端的运行汇总还没有做。v0.12.26 已在本版之前发布。

配置：没有新键，也没有新的 revision，`actingd-config` 仍是 [5, 19]，`runtime-client` 仍是 [2, 2]，账本仍是 [3, 4]；
没有改任何默认值。

- 组件接口表、默认配置文件、配置 crate、actingd 和账本都没有改动；契约 crate 只加了运行汇总的折叠函数、`QueryRuns`
  的请求和结果类型（只定义，没有任何操作带它们）和一个结果码。
- `scheduling.rerun` 这组键 v0.12.0 起就有、默认值没变，但以前没有代码读，本版开始生效：`condition_codes`、
  `max_per_occurrence`、`refund_counts` 在一次运行第一次结算时读（实时结算或按终态对账；启动时的对账读 actingd 配置
  文件里的值），`resume_stage`、`resume_settle_ms`、`promote` 在每轮评估时读。配置里改过这组键的，本版起按改过的值
  生效。
- `host_claims.run_end_grace_ms`（默认 120 秒）以前只在运行到截止时间时读，现在任何一次运行开始收尾时都读（ISO-5a）。
- 账本没有新表、新索引或新事件类型，格式不变；本版写的新记录都是已有的记录类型（见下面）。
- 新结果码一个：`invalid_query_runs`（`since` 0.12.27）。它的评审状态还是 pending，结果码发布文件只收已定稿或已退役
  的条目，所以本版的发布文件是空的。没有退役的码。
- 现有的配置都不需要修改。

升级时要知道的：

- 条件终止（SR-2b-1、SR-2b-2）：从 v0.12.26 升级不用做什么。v0.12.26 不写执行记录里的 `condition`，也不写 L7 的
  `condition_reruns`，升级时没有欠着的补跑。默认码表是 `recognition_sample_budget_insufficient`、
  `candidate_sample_budget_insufficient`、`recognition_sample_deadline_exceeded`、`candidate_sample_deadline_exceeded`、
  `blackout_run_stopped`、`perf_run_yielded`、`perf_hold_expired` 和 `cost_commit_time_insufficient`：被黑名单时段停下、
  被性能控制让出的运行也按条件终止。只限可恢复、非敏感任务的失败；操作员暂停（`contained_task_paused`）不在表里。
- L7 和新记录：每对任务（任务、实例）的决策记录 `policy.control_state_recorded` 带 `condition_reruns`
  `{failure_code, count, expires_at_unix_ms}`，每对至多一条，重启后照它载回；执行记录的失败部分带 `condition`
  `{refunded, count, expires_at_unix_ms}`。新记的是三种计划信号，载体都是已有的 `policy.planning_signal_observed`
  （kind `goal_missed`，`signal_id` 以 `signal:policy_rerun:` 开头）：`policy_rerun_exhausted`（warning，同一时段连续
  超过 `max_per_occurrence` 次）、`policy_rerun_waiting`（info，每个欠补跑第一次因节奏段等待时记一次）和
  `policy_rerun_expired`（warning，过了时段终点）。评估把欠着的补跑当作已触发、跳过冷却，原因 `policy_rerun_owed`。
- 欠着补跑期间，这次运行的失败终态不进任务快照，依赖它的链条任务（`dependency_completed`）等着；过期之后失败终态回到
  快照，按普通失败算。同一时段刚用尽的那对任务不再提前补跑，只等它自己的触发和冷却；这个“用尽”只记在内存里，重启后
  同一时段里新欠的补跑又会被提前。
- `reruns`（SR-2b-2）：`actingd suspended` 多了 `reruns` 列表，每对任务一行（`task_id`、`instance_id`、`failure_code`、
  `count`、`expires_at_unix_ms`、`state`）；`ac_diagnose` 带出被诊断实例的那些行，按输出预算截断时排在 `remedies`
  后面（`reruns_truncated`）。`state` 看 Runtime 自己写的记录：有 `policy_rerun_expired` 是 `expired`，否则有
  `policy_rerun_waiting` 是 `waiting`，否则是 `owed`；时段在 Runtime 停机期间结束的，重启后第一轮评估记下过期之前仍
  显示 `owed` 或 `waiting`。升级时 MCP 入口还没切到本版子进程之前，旧子进程的 `ac_diagnose` 没有 `reruns`。
- 收尾续租（ISO-5a）：运行收尾（成功、其他失败、取消）时，只要还持有租约、租约没过期，而且会在 `run_end_grace_ms`
  之内到期，就先续一次。这次运行的历史里会多三条已有类型的记录：`scheduler.admitted`、`lease.transition_intent`
  （`lease.renew`）和 `lease.renewed`，和运行到截止时间时的收尾续租一样。成功终态已经写下、收尾超过宽限期才碰到租约
  已结束的计划运行，按成功结算，只有它的回执失败。只让这一次操作失败的内核拒绝，回执的 `runtime_code` 由
  `runtime_halted` 改为已有的 `backend_operation_failed`。还没修的：多 Nemu 截图时，客户端或宿主认领的租约带着打开的
  会话过期，仍会让 Runtime 停下（后续 ISO-5b）。
- 截图编号（CAP-IDX）：有发现绑定的实例，截图取绑定里的编号。端口不是该编号的基础端口时，打开截图或配对会话之前先用
  MuMuManager `info` 读一次（有时限），报告的端口必须和绑定的一致，否则只有这一台打不开（`capture_backend_open_failed`
  或 `paired_backend_open_failed`，原因 `target_identity_mismatch`；读取失败是 `target_identity_unavailable`）。没有
  发现绑定的条目只在基础端口上推得出编号，原来前后两个端口的宽容没有了。
- 空闲唤醒（IDLE-WG）：不用做什么。被别的门挡着的任务（预算用尽、间隔、退避、补救 hold）在暂停或占用期间仍可能叫醒
  实例，这部分归后续片。
- 新码 `invalid_query_runs`（P32-B1）：本版没有任何操作发送或答复 `QueryRuns`，账本和回执里不会出现这个码。

退回 v0.12.26 之前要做的事（按代码核对过）：

- 配置和运行时指令：本版没有新键，也没有改默认值，v0.12.26 照样读本版的配置文件（`scheduling.rerun` 各键它也解析，
  只是不用），回退启动的 `instructions` 检查不会因本版失败。
- L7 里的 `condition_reruns`：不用做什么，但要知道后果。这个成员 v0.12.0 起就在 L7 的形状里，v0.12.26 照常读这条记录、
  校验通过，但载入时不理会它：欠着的补跑不提前、不等节奏段、不过期，`actingd suspended` 也不列；藏起来的失败终态回到
  任务快照，依赖它的链条可能读到 `dependency_completed(head, failed)`。v0.12.26 下次写决策记录时，发现这些对的记录和
  内存里的不同，就改写成不带 `condition_reruns` 的记录，之后再升到本版也找不回来。已经退回的次数仍是退回的，没加的
  连败也仍没加（都在计数里）；那对任务回退后照常由它自己的触发再跑，需要马上补的手动跑一次。
- 执行记录里的 `condition`：不用做什么。v0.12.26（SR-2a 起）重放时照它结算：不加连败、按 `refunded` 退回次数、不计
  运行时长。回退后 v0.12.26 自己结算的负载失败又按普通失败算：扣次数、加连败，连续三次暂停这对任务。
- 新记录：不用做什么。`policy_rerun_*` 三种计划信号是已有的记录类型和 kind，v0.12.26 当普通计划信号读；它的
  `actingd suspended` 按 `signal_id` 前缀挑记录，不列它们。ISO-5a 的续租记录也是已有类型，v0.12.26 当普通续租读。
- 收尾（ISO-5a）：规则编在程序里，不存状态。回退后这 13 种收尾情形又会让 Runtime 被 poison 或停机，只让操作失败的
  拒绝回执又是 `runtime_halted`。
- 截图编号（CAP-IDX）：不存状态。回退后截图又只按端口推算编号，端口顺延的实例又打不开截图（10-29 00:44 那种情况），
  只能让它回到自己的基础端口（关掉再启动这台模拟器）。
- 空闲唤醒（IDLE-WG）：不存状态。回退后又可能为被活动时段挡着的任务叫醒实例、白跑一次启动包。
- 结果码：`invalid_query_runs` 只是新登记，本版不会把它写进账本或回执，回退不用做什么；没有退役的码。
- 请求成员：本版没有新增请求类型或请求成员。`QueryRuns` 的请求和结果只定义了类型，没有操作带它们，`runtime-client`
  仍是 [2, 2]。
- 客户端和 Runtime 的配对：
  - 本版的客户端连 v0.12.26 的 Runtime：照常。请求和以前一样，不发 `QueryRuns`；运行状态改用契约的折叠函数，读的是
    同样的答复，结果不变。`ac_diagnose` 的 `reruns` 来自安装选中的那一代的 `actingd suspended`，那一代是 v0.12.26
    时它是空列表。
  - v0.12.26 的客户端连本版的 Runtime：照常。请求不变；回执里的 `backend_operation_failed` 是它认得的码；本版
    `actingd suspended` 多出的 `reruns` 它不读，`ac_diagnose` 没有 `reruns`。

运行上的变化：

- 截图编号（CAP-IDX）：截图按绑定里的 MuMu 编号打开；端口顺延时先用 `info` 核对端口，不符只拒这一台。
- 运行状态（P32-B1）：运行状态改由契约里可交换的折叠函数算出，结果不变；`QueryRuns` 只定义、不上线，列表速度不变。
- 条件终止（SR-2b-1）：负载造成的失败退回次数、不加连败、记下欠补跑（L7 `condition_reruns`），超限记 `policy_rerun_exhausted`。
- 收尾（ISO-5a）：运行和持有者收尾的 13 种情形不再让 Runtime 停下；收尾贴着租约终点时续租一次。
- 空闲唤醒（IDLE-WG）：被黑名单或关着的活动时段挡着的任务不叫醒空闲实例，只把唤醒挪到门放行的时刻减去提前量。
- 欠补跑（SR-2b-2）：欠着的补跑提前、等节奏段回落并稳定、到时段终点过期；`actingd suspended` 和 `ac_diagnose` 列 `reruns`。

本版合并的 PR：

- #1417 CAP-IDX（#421）：有发现绑定的条目截图取绑定的 MuMu 编号；端口不是基础端口时先用有时限的 `info` 核对端口，不符以 `target_identity_mismatch` 只拒这一台；没有绑定的条目只按基础端口推编号。
- #1414 P32-B1（#422）：运行汇总的契约（`QueryRuns` 的请求和结果，只定义、不提供）和可交换的运行状态折叠函数，客户端已改用它；新结果码 `invalid_query_runs`。
- #1415 SR-2b-1（#386）：负载造成的采样失败按条件终止结算：退回当日次数、不加连败；超限记 `policy_rerun_exhausted`；欠补跑记进 L7 的 `condition_reruns`。
- #1416 ISO-5a（#419）：运行或持有者收尾的任何一步都不让 Runtime 停下（13 项）；降为非致命的失败回执 `backend_operation_failed`；收尾时租约会在宽限期内到期就续一次。
- #1418 IDLE-WG：空闲实例只为此刻能派出去的任务叫醒（不为被活动时段或黑名单挡着的任务叫醒）。
- #1419 SR-2b-2（#386）：欠着的补跑提前、等节奏段、到时段终点过期；`actingd suspended` 和 `ac_diagnose` 列出 `reruns`。

## v0.12.26

本版是 v0.12.25 之后合并的 5 个 PR，都是 0.12 第二波：P32-A、W2-A、W2-B、W2-C、W2-E。运行中会注意到的：运行记录最多
的实例列运行（概览的最新运行、`ac_runs` 和诊断的运行部分）应该会快一些，因为列表页不再带载荷，按单个运行读状态也不再
每页把整个运行排一次序（P32-A）；但根本的修法是 Runtime 端的运行汇总（P32-B），还没有做，本版不承诺列表在多长时间内
答完，最忙的实例仍可能答复不完整。被读取预算切断的状态读取现在接着往下读，来不及就标不完整，不再读成“没有重启”“没有
租约终结”或“手动运行”（P32-A）。计划运行在截图、帧提交或帧解析时遇到会话正在关闭、端点变了或帧被拒绝，只算一次普通
失败，不再把任务暂停到有人恢复（W2-C）；输入遇到会话正在关闭、端点变了或帧不再提交时也一样，而且不再隔离这个实例
（W2-E）。暂停期间启动模拟器排上的启动包经过一次 Runtime 重启（例如部署）也不丢，解除暂停后跑一次（W2-B）。条件任务
自己的派发被挡住时仍请求读取它的事实，挡住读取的闸门通过 `fact_refresh_pending` 点名（W2-A）。v0.12.25 已在本版之前
发布。

配置：没有新键，也没有新的 revision，`actingd-config` 仍是 [5, 19]，`runtime-client` 仍是 [2, 2]，账本仍是 [3, 4]；
没有改任何默认值。

- 组件接口表、配置 crate 和 actingd 都没有改动；契约 crate 只从计划策略“总是严重”的表里拿掉五个码（W2-C），并补了
  说明。
- `host.idle_state` 多了一种行 `{instance_alias, startup_package_queued: true}`（W2-B）：这是已有运行时事实的新行形式，
  不是配置键。
- 运行列表改用页请求已有成员 `profile` 的已有取值 `concise`（P32-A）：没有新请求类型，也没有新请求成员（配对见下面的
  回退）。
- 账本没有新表、新索引或新事件类型，格式不变；P32-A 只改了按单个运行的链接读历史时的排序，行和顺序都不变。
- 没有新结果码，也没有退役的码，结果码发布文件是空的。
- 现有的配置都不需要修改。

升级时要知道的：

- 暂停中排着的启动包（W2-B）：从 v0.12.25 升级不用做什么，v0.12.25 写的 `host.idle_state` 没有新行，本版照常读。但
  部署这次重启还是旧的结果：升级之前在 v0.12.25 下、暂停期间排上的启动包，v0.12.25 没有记下来，这次重启之后不会再排，
  解除暂停后不会跑（和以前一样）；要它跑，解除暂停后手动补跑一次那个实例的启动包。本版运行起来之后排上的才会被记住。
- 运行列表（P32-A）：部署之后命令行就是本版。升级时 MCP 入口还没切到本版子进程之前，旧子进程照旧用 `forensic` 列运行，
  答复照常，只是没有本版客户端一侧的加速，被切断的状态读取也仍只读第一页；切换之后就是本版的读法。
- W2-C：计划运行在截图、帧提交或帧解析时因 `execution_session_close_in_progress`、`execution_instance_identity_mismatch`、
  `input_frame_capture_mismatch`、`input_frame_not_committed` 或 `input_frame_session_missing` 失败一次，不再把任务暂停
  到有人恢复，而是按普通失败退避重试、计入任务的失败暂停。
- W2-E：输入遇到 `execution_session_close_in_progress`、`execution_instance_identity_mismatch` 或 `input_frame_not_committed`
  只让这次输入失败，不再把任务暂停到有人恢复，也不再隔离实例、送它上恢复阶梯。
- 升级之前已经因为这些码暂停的任务，升级后仍是暂停（暂停状态照记录恢复），照旧要人恢复。

退回 v0.12.25 之前要做的事（按代码核对过）：

- 配置和运行时指令：本版没有新键，也没有改默认值，v0.12.25 照样读本版的配置文件，回退启动的 `instructions` 检查不会
  因本版失败。
- 空闲状态记录（W2-B）：不用做什么，但要知道后果。只有某个实例的启动包正排在暂停后面时，记录里才有
  `startup_package_queued` 行。有这一行时，v0.12.25 不认识这种行，把整条记录读成别的形状：回退后的第一次启动记一条
  `invalid_runtime_state_fact`（操作 `restore_idle_state`），不致命，启动继续；每个空闲动作不是 `none`、配置了启动包
  （`stop_emulator` 还要配置了模拟器启动）的实例按 v0.12.25 说明里的办法唤醒一次、启动包跑一次。v0.12.25 不恢复记下
  的启动包本身：没有空闲动作的实例丢掉排着的启动包（和本版之前一样）；没有任何实例配置空闲动作时，什么都不唤醒，只记
  那一条失败。v0.12.25 随后按它自己的旧形状重写记录，不带这一行，所以之后再升到本版也不会把它找回来、不会再排。需要的
  话，回退并解除暂停后手动补跑一次那个实例的启动包。记录里没有这一行时，v0.12.25 照常读。
- 账本：不用做什么。账本的格式、表、索引和事件类型都没变，v0.12.25 照样打开本版写过的账本；P32-A 的排序只在读取时
  起作用，回退后按单个运行读状态又回到每页排序的速度。已经结算的计划运行照记录读；停机时还没结算的运行由 v0.12.25 的
  启动恢复按它自己的表结算。
- 计划策略（W2-C、W2-E）：“总是严重”的表编在程序里，不存状态。回退后这些码又按 v0.12.25 判：计划运行遇到它们一次就
  把任务暂停到有人恢复，输入遇到它们又会隔离实例。
- 结果码：本版没有新码，也没有退役的码，不用做什么。
- 请求成员：本版没有新增请求类型或请求成员。
- 客户端和 Runtime 的配对：
  - 本版的客户端连 v0.12.25 的 Runtime：照常。`concise` 是页请求已有成员的已有取值，v0.12.25（和更早）的 Runtime
    答复同样的行、游标和读取范围，载荷省略、链接照带，本版客户端按链接认出运行；接着读被切断的页用的也是每个 Runtime
    都答复的游标。
  - v0.12.25 的客户端连本版的 Runtime：照常。它仍用 `forensic` 请求，答复的行和顺序都不变。

运行上的变化：

- 运行列表（P32-A）：列表页用 `concise` 请求、不带载荷，按链接认运行；按请求、租约、运行等单个运行的链接读历史时按
  链接的顺序读，不再每页建临时排序；状态读取（Runtime 重启、租约终结、计划意图）遇到被读取预算切断的页接着读，截止
  时间到了就标不完整（`latest_runs`、`list_contained_runs` 答 `incomplete`），不再读成“没有”；诊断的计划意图读取被切断
  时，`runs_grouped` 标不完整并带 `call_budget_exhausted`。
- 事实读取需求（W2-A）：条件任务自己的派发被挡住（重场景名额已满、宿主负载不够、实例不可用）时仍提出读取需求，由读取
  任务自己的闸门决定：重场景名额满时轻量读取照常派；实例不可用时任务在 `trigger_unknown` 之后带 `fact_refresh_pending`
  （`gate_code` 为 `instance_unavailable`）；实例不提供它的操作或实例覆盖禁用它的任务不提需求。
- 暂停中的启动包（W2-B）：暂停期间排上的启动包（`ac_emulator start` / `restart` 或配置的启动）记进 `host.idle_state`；
  Runtime 重启后在恢复的暂停后面重新排一次（新的 `startup_package_scheduled`），解除暂停后跑一次、不会跑两次；启动包
  被授予、模拟器 `stop`、控制失败、准备失败的重连或自检都会把它忘掉。
- 会话和帧的拒绝（W2-C）：计划运行的截图、帧提交或帧解析遇到这五个码一次，按普通失败退避重试，不再暂停任务。
- 输入的拒绝（W2-E）：输入遇到会话正在关闭、端点变了或帧不再提交，只让这次输入失败，不暂停任务、不隔离实例；设备或
  后端故障、会话死亡和没确认的关闭仍按原来的严重程度处理。

本版合并的 PR：

- #1407 P32-A（#422）：按单个运行的链接读历史时按链接的顺序（不再建临时排序）；`first_status_events` 在 `has_more` 时接着读，被切断的页点名不完整，不再当成“没有”；列表页用不带载荷的 `concise` 请求（页请求已有成员 `profile` 的已有取值，没有新请求成员）。
- #1409 W2-C（#419）：剩下的五个码移出“总是严重”的表，计划运行因它们失败一次不再暂停任务。
- #1410 W2-A：条件任务的派发被挡住时仍请求事实读取，候选通过 `fact_refresh_pending` 点名挡住读取的闸门。
- #1411 W2-B：暂停期间排上的启动包经过部署重启也保留（`host.idle_state` 里的 `startup_package_queued` 行），解除暂停后跑一次。
- #1412 W2-E（#419）：输入遇到 `execution_session_close_in_progress`、`execution_instance_identity_mismatch` 或 `input_frame_not_committed` 只让这次输入失败，不暂停任务、不隔离实例。

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
