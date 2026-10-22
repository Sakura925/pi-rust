# 本地构建晋级流水(local_master promotion log)

> 记录每次晋级到 local_master(日用版来源)的 commit、产物 hash、验证记录。
> 注意与 `docs/promotion/`(上游作者的推广文章)无关——此 promotion 非彼 promotion。

## 晋级 #1:2026-10-14 19:0x · v0.3.23 同步晋级

| 项 | 值 |
|---|---|
| 晋级 commit | `e1b7adc`(Merge tag 'v0.3.23' into feat/thinking-config) |
| 版本 | rpi 0.3.23(基线 0.3.21) |
| 产物 SHA-256 | `00f20ade62a5e47ed97c4341fd77665532835ec21633df9c97f2160d87208b83` |
| 构建参数 | `cargo build --release`(strip=none,Cargo.lock locked) |
| 上游内容 | v0.3.22(durable message delivery)+ v0.3.23(plugins 所有权/unsafe 收敛、website×3、release chores),8 commits |
| 个人内容 | 4 个死锁修复(c1dacd4/ad7cf41/2a7d5ca/41f8485)+ 流程文档(cd01e97) |

### 验证记录(全部通过)

| 关卡 | 结果 |
|---|---|
| merge 冲突 | 干净(merge-tree 预检 + 实际合并零冲突) |
| 语义审查 | 上游对重叠文件仅做释放 RAII 化(OwnedString::from_raw),与 keepalive/continuation 修复互补;run_extension_continuation/route_extension_dialog_key/dialog_generation 完好 |
| `cargo test --workspace --locked` | **1756 passed / 0 failed**(71 套件;基线 1680,上游新增 76 全过) |
| 死锁回归 | `dialog_key_routing_*` 1 passed(未挂死) |
| 候选 smoke test | `rpi 0.3.23` + GLM 模型连通("好") |
| 旧 session 兼容 | 副本 `--export` 成功(0.3.21 格式可读) |
| 安装入口检查 | local_master 分支 ✓ 工作树干净 ✓ HEAD=验证 commit ✓ |
| 安装一致性 | `~/.cargo/bin/rpi` 与 `target/release/rpi` SHA-256 相同 |

### 推送记录

- `origin/local_master` 41f8485→e1b7adc ✓
- `origin/feat/thinking-config` 41f8485→e1b7adc ✓
- main 快进至 v0.3.23(9857e79),未推送(纪律:main 永不推)

### 回退保险

- 旧二进制备份:`~/.cargo/bin/rpi-0.3.21-41f8485-bak`(32MB)
- 注意版本只升不降红线:0.3.23 写过的 session 含 `commit_stream` 等 0.3.21 不认识的枚举,**禁止回退日常使用**;仅灾难恢复时按 SKILL.md 降级流程操作

## 晋级 #2:2026-10-15 14:3x · 预算交接与工作状态提醒(harness 收敛能力)

| 项 | 值 |
|---|---|
| 晋级 commit | `8056320`(third review round — flush held AgentEnd on early errors, full-message reminder identity) |
| 版本 | rpi 0.3.23(个人功能叠加,无上游变更) |
| 产物 SHA-256 | `592237550d374eca182c2273da998f5f3cf1cf19f996e8fa2d462c65d999fb1f` |
| 构建参数 | `cargo build --release --locked` |
| 分支 | `feat/budgeted-handoff`(6 commits:2 feature + 4 审查修复),ff 合入 local_master |
| 个人内容 | ① 有界交接:`RPI_MAX_TURNS_PER_RUN` 触顶时一次无工具、限 4096 tokens、限 120s(harness 层 tokio timeout)的总结调用,落盘在 runBudget 通知之前,并入 Completed 的 final_message/final_entry_id;`DeferredAgentEndEmitter` hold 住 loop 的 AgentEnd,交接完成后 flush 恰好一个终止事件(含错误路径) ② working-state 提醒:`RPI_WORKING_STATE_INTERVAL`(每 N 工具轮一行状态检查)+ `RPI_REPEAT_TOOL_REMINDER`(连续 3 轮相同调用提醒),注入身份按完整 UserMessage 全等注册(压缩过滤不误删用户输入),均默认关闭 |

### 验证记录(全部通过)

| 关卡 | 结果 |
|---|---|
| 外部审查 | codex 三轮:一轮 2×P1+3×P2、二轮 3×P2、三轮 2×P2,全部修复;三轮确认正常交接/retry 成功路径/预算关闭不装 wrapper |
| 自审 | run_core 全部提前退出路径(loop 后 5 处)flush 覆盖;flush 幂等;observe_turn 单次;无调试残留 |
| `cargo test --workspace --locked` | ALL PASS |
| 死锁回归 | `dialog_key_routing_*` 1 passed(未挂死) |
| 端到端 | harness_run_e2e 40 passed(单 AgentEnd、handoff 顺序、final_message 契约、提醒不落盘);working_state 单测 11 passed |
| 候选 smoke test | 候选二进制 + 安装后 `~/.cargo/bin/rpi` 各一次真实请求("正常"/"成功") |
| 旧 session 兼容 | 副本 `--export` 成功(v4 会话可读;本改动无新 entry 类型) |
| 安装入口检查 | local_master 分支 ✓ 工作树干净 ✓ HEAD=8056320 ✓ |
| 安装一致性 | `~/.cargo/bin/rpi` 与 `target/release/rpi` SHA-256 相同(`5922…fb1f`) |

### 推送记录

- `origin/local_master` 90ef937→8056320 ✓
- `origin/feat/budgeted-handoff` 新建 → 8056320 ✓
- main 未动(纪律:main 永不推)

### 回退保险

- 旧二进制备份:`~/.cargo/bin/rpi-bak-20261015-pre-handoff`(33MB,e1b7adc 构建产物)
- 版本红线不变:本改动未引入新 session 格式,回退无格式风险,但**不建议回退日常使用**(交接/提醒能力会丢失)

### 后续试运行

- `RPI_MAX_TURNS_PER_RUN=80 rpi`(高于取证 40 轮正当探索,低于 112 轮跑飞案例)
- `RPI_WORKING_STATE_INTERVAL=10 rpi` / `RPI_REPEAT_TOOL_REMINDER=1 rpi`
- 验收指标:BudgetStop 触发次数、单 run 轮数分布(从会话 JSONL 统计);token 配置(32768/4096)按真实输出量与截断率再调

## 测试构建:2026-10-15 15:5x · TUI 白屏修复 + pi-open-tui 功能迁移(未晋级)

> **状态:测试期构建,未合入 local_master。** 日用 `~/.cargo/bin/rpi` 暂时来自 feat 分支;试用验收通过后按晋级流程 ff 合 local_master 再正式归档(届时更新本节状态)。

| 项 | 值 |
|---|---|
| 来源分支 | `feat/tui-main-screen-fix` @ `8e193de`(5 commits:1 白屏修复 + 3 功能迁移 + 1 审查修复) |
| 版本 | rpi 0.3.23(个人功能叠加,无上游变更) |
| 产物 SHA-256 | `615dcd36ca3968e8f82862b201fddd7656e29e92458e1cfff99bbb5807166906` |
| 构建参数 | `cargo build --release --locked` |
| 安装一致性 | `~/.cargo/bin/rpi` 与 `target/release/rpi` SHA-256 相同;macOS 先 rm 再 cp |

### 个人内容

1. **主屏白屏修复**(`tui_alt_screen.rs`):视口跟踪漂移导致的失控滚动(`\r\n`×N 把正文推出视口)改为 scroll>屏高 时全量重发自愈并重同步 viewport 状态;空白行的 reset 尾巴 diff 剪枝(消除 replay 字节流 38% 的"跳列+无文本"噪声)
2. **圆角 editor 边框**(`editor.rs`):`╭─…─╮`/`╰─…─╯`,working spinner 嵌入保留
3. **thinking peek**(`thinking_peek.rs`):hide-thinking 时推理尾部 ticker(`~ think ⠋ …`),正文开始落定 ✓,任务收尾恢复原生标签
4. **turn 遥测**(`turn_telemetry.rs`):每 turn 结束一行 `TPS/TTFT/+/↑/↓/stall/$ per M`

### 验证记录(全部通过)

| 关卡 | 结果 |
|---|---|
| 外部审查 | 探针审查一轮:2×P1(working 边框超宽 1 列、自愈后光标失配)+4×P2(空白行剪枝吞背景变化、peek 拿不到组件、工具链第二条消息不转、stall 合并计数),全部修复(`8e193de`) |
| 探针固化 | `working_border_width_is_exact`、`regular_mode_healed_frame_leaves_cursor_at_document_tail`、`regular_mode_repaints_painted_blank_rows`、`consecutive_slow_deltas_count_each_gap` 等 5 个新回归 |
| `cargo test --workspace --locked` | ALL PASS(rpi-tui 323 / rpi-cli 498) |
| 死锁回归 | `dialog_key_routing_*` 1 passed(未挂死) |
| 冒烟 | `rpi -p "只回复一个字:好"` → `好`(模型连通;TUI 观感待日常试用) |

### 试用验收点(待确认)

- [ ] 长任务(流式大段输出/工具循环)不再白屏,内容持续可见,scrollback 完整
- [ ] hide-thinking 开启时 thinking peek:spinner+推理尾行 → ✓ → 任务结束恢复 `Thinking...`;工具链第二条消息重新旋转
- [ ] 每 turn 结束的遥测行数值合理(TPS/TTFT/stall)
- [ ] 圆角输入框 + working 状态边框不换行不错位(40 列窄终端也正常)
- [ ] 旧 session 副本 `--export` 兼容(本改动无新 entry 类型,预期无风险)

### 回退保险

- 旧二进制备份:`~/.cargo/bin/rpi-bak-20261015-pre-tui-main-screen-fix`(8056320 构建,晋级 #2 产物)
- 版本红线不变:本改动未引入新 session 格式,回退无格式风险;验收通过后走正式晋级流程

## 晋级 #3:2026-10-15 17:4x · v0.3.24 上游同步 + TUI 大改版(tui-main-screen-fix)

| 项 | 值 |
|---|---|
| 晋级 commit | `dd8352b`(feat/tui-main-screen-fix 全链:上游 v0.3.24 merge + 9 功能提交 + 3 审查修复提交),ff 合入 local_master |
| 版本 | rpi 0.3.24(上游 0.3.24 + 个人功能叠加) |
| 产物 SHA-256 | `df06699c0e4a6ee69424b5e03f5c860d172e05c6b05e4d26d0946be1e0ac8083` |
| 构建参数 | `cargo build --release --locked` |
| 安装一致性 | `~/.cargo/bin/rpi` 与 `target/release/rpi` SHA-256 相同;macOS 先 rm 再 cp |

### 个人内容(叠加在 v0.3.24 之上)

1. **主屏白屏修复**:失控滚动自愈(scroll>屏高全量重发+视口重同步)、空白行中性尾巴 diff 剪枝
2. **pi-open-tui 功能迁移**:圆角 editor、thinking peek(推理尾部 ticker)、turn 遥测(TPS/TTFT/stall/$)
3. **codex 式渲染**:bash 面板(无背景、$ dim、折叠 20→6 行、✔/✘/⊘ 单行完成态)、skill ⏵ 紧凑折叠行、footer Starship 增强(git 工作树摘要 + ⚡ 模型速率)
4. **粘贴附件落盘**:`~/.rpi/tmp/paste/` + 路径注入 prompt(vision skill 可用)
5. **delegate_task 子 agent 工具**:`.ai-flow/agents/` 定义驱动嵌套 rpi、`--mode json` 事件流实时进度、四态完成(✓/✗/⏱/⊘)、进程组终止+stderr 排空、`.ai-flow/skills` 发现对齐

### 验证记录(全部通过)

| 关卡 | 结果 |
|---|---|
| 上游重叠审查 | v0.3.24 与本分支文件零交集,merge 零冲突;上游侧改动集中在 install/updates/prebuilt |
| codex 外部审查(tty7 派发) | 2×P1(stderr 管道阻塞、进程树残留),探针复现后全部修复(`82cac8d`) |
| 离线渲染探针审查(用户提供) | 5 项(partial 提前 ✓、timeout 绿勾、无实时进度、标题不稳定、结果偏重),全部修复(`dd8352b`) |
| `cargo test --workspace --locked` | ALL PASS(rpi-tui 326 / rpi-cli 522 / rpi-tools 77) |
| 死锁回归 | `dialog_key_routing_*` 1 passed |
| 冒烟 | `rpi -p` 模型连通("好");`rpi 0.3.24` 版本对上 |
| 旧 session 兼容 | 0.3.21 会话 `--export` 成功 |
| 安装入口检查 | local_master ✓ 工作树干净 ✓ HEAD=dd8352b ✓ |

### 推送记录

- `origin/local_master` 1d73a41→dd8352b ✓
- `origin/feat/tui-main-screen-fix` 新建 → dd8352b ✓
- main 快进至 v0.3.24(7210550),未推送(纪律:main 永不推)

### 回退保险

- 旧二进制备份:`~/.cargo/bin/rpi-bak-20261015-pre-0.3.24-merge`(8e193de 测试构建,0.3.23)
- 版本红线:0.3.24 与 0.3.23 session 格式无新增 entry 类型(上游改动仅 install/updates),回退无格式风险,但会丢失全部个人功能

### 试用验收点(此前测试构建的清单+新增)

- [ ] 白屏消失(scrollback 完整)
- [ ] bash/skill codex 面板观感;footer git 状态/⚡ 速率
- [ ] 粘贴截图 → vision skill 分析
- [ ] lcy 工作区 delegate_task 派 luna:实时活动行(执行 read/思考中…)、四态完成、Ctrl+O 展开 markdown 报告

### 晋级 #3 增量勘误（同日 22:4x，delegate_task 实战调试三连）

| commit | 内容 |
|---|---|
| `216479a` | 内置 delegate_task 接管信封名与参数形状（role/maxTurns/timeoutSeconds 原生兼容，role 无定义走通用子 agent）；假扩展 librpi_subagents.dylib 已移至 extensions-disabled/（strings 逆向证明其从未 spawn 任何进程，纯信封打印机） |
| `33e0026` | delegate 面板心跳：事件流间隙（子 agent 下载网页的静默期）也每 2s 发 partial，elapsed 计时不再冻结 |
| `6d3da74` | 面板出生路径修复：面板从 AssistantUpdate 流式路径诞生时未设置 delegate 身份 → 通用空壳框；apply_delegate_info 补齐到全部四个创建/更新点 |

- 实战验证：delegate_task 派 tty7-cli-skill-writer 完整跑 2m11s（调研 2 页面 → 蒸馏 12KB SKILL.md 落盘 ~/.rpi/agent/skills/tty7-cli/ → 结构化回报），面板 `✓ agent · 2m11s` + 简述 + Ctrl+O 展开全程正确
- 产物 SHA-256：`35113c71…`（06f8b99 时代）→ 后续三个增量提交后已重装，见 `shasum ~/.cargo/bin/rpi`
- 推送：local_master / feat 双分支均至 `6d3da74` ✓

---

## 晋级 #4：上游 v0.5.0 全量同步（2026-10-18）

### 背景

上游连发 3 版（v0.4.0 / v0.4.1 / v0.5.0，44 commits），用户发起同步。核心上游变更：
plugin-owned child agent lifecycle、**runaway-scroll 重复内容修复**（与 2026-10-18 本地取证的「长回复重复两遍」bug 同源）、
thinking_level_map 强类型化、模型 cost 定价、bash 面板 render_panel 重构。

### 合并裁决（6 文件 9 处冲突，全部语义审查）

| 冲突文件 | 裁决 |
|---|---|
| openai_completions.rs | fork 的 mapped_reasoning_effort 退役（上游原生实现 thinking_level_map）；null 回落语义随上游 |
| config.rs / settings.rs | 上游强类型字段与 fork 弱类型重名（自动合并产生重复字段定义，编译错级），保留上游强类型 + fork 特有 thinking_budgets；清除 3 处重复初始化 |
| interactive_tui.rs | dialog key 路由跟上游改名 handle_extension_dialog_key（含等价死锁修复）；死锁回归测试改名保留 |
| agent_harness.rs | run_budget 采上游配置化（max_turns → RunBudget::new）；fork 的 reminder_registry + budget 交接摘要保留 |
| bash_execution.rs | 采上游 render_panel 架构；codex 视觉经 status_line glyph 状态行部分保留 |
| tui_alt_screen.rs | **采上游 e59a6bb 方案**：删除 fork 继承的 force_tail_repaint/healed SD 自愈（上游认定为 scrollback 重复的根因），改 changed_in_scrollback 检测 + 清屏全量重建；自愈测试改造为新契约回归 |

### 关卡

| 关卡 | 结果 |
|---|---|
| `cargo test --workspace --locked` | ALL PASS（1931 tests / 0 failed，含 fmt 后复跑） |
| 死锁回归 | `dialog_key_routing_*` 1 passed（无挂死） |
| 旧 session 兼容 | runloom 23678 行会话 + 本地 0.3.x 会话 `--export` 均成功（安装前后各验一次） |
| 安装入口检查 | local_master ✓ 工作树干净 ✓ HEAD=b32f47f ✓ |

### 产物与安装

- commit：local_master = `b32f47f`（merge `fbe6114` + fmt `b32f47f`）
- 构建：`cargo build --release --locked -p rpi-cli`（2m49s，thin LTO + codegen-units=16）
- 产物 SHA-256：`2d31f51bd350d689e13d9650d2680a02c155eb91ccc684a614c4ddca22d576cc`（安装前后核对一致）
- 安装：备份 → `rm` + `cp`（macOS 签名缓存纪律）→ `rpi 0.5.0` ✓

### 回退保险

- 旧二进制备份：`~/.cargo/bin/rpi-0.3.24-bak`
- **版本红线（本次升级跨 2 个 minor）**：0.5.0 含 child agent 等新机制，升级后**禁止回退 0.3.24 日常使用**（0.5.0 写过的 session 旧二进制 fail-closed）

### 推送记录

- `origin/local_master` 推送超时（GitHub 网络不通，ls-remote 也超时）——**遗留：网络恢复后补推**（`git push origin local_master`）
- main 已快进至 v0.5.0（621b9d9），未推送（纪律：main 永不推）

### 遗留事项

- **feat/render-viewport-drift**：其中 healed 清屏修复已被上游 e59a6bb 方案取代（合并点消失）；仅 resume() SIGTSTP 重锚定仍有价值——需从新 local_master 重开分支重放
- **feat/delegate-visibility**：待 merge 新 main 后继续
- 正在运行的老进程（旧 0.3.24，含 tty7 各会话）继续用旧 inode 写旧格式（可读）；新开会话即 0.5.0

## 晋级 #5:2026-10-19 · tty7 配套主题三连(Harbor / Nord / Everforest)

| 项 | 值 |
|---|---|
| 晋级 commit | `c67ef26`(feat/theme-harbor ff 链:3188d94 Harbor → 5d62709 Nord → c67ef26 Everforest) |
| 版本 | rpi 0.5.0(版本号不变,同版本多构建——以本档案 SHA-256 区分) |
| 产物 SHA-256 | 安装构建 `b82614e121f1625e541f3e86502e618bba1af6d155b325b42bd81b60b52e1f06`;候选验证构建 `a3a877b387ba23e2a1b9137942de95796ecb9848c5a50201879bbb10ec713a0f`(同源码 a005731、同 profile,非确定性构建差异) |
| 构建参数 | `cargo build --release -p rpi-cli --locked`(2m28s,thin LTO + codegen-units=16) |
| 内容 | 新增 3 个 tty7 配套深色主题 preset:Harbor(基于 tty7 内置 Harbor)、Nord(基于 tty7 内置 Nord)、Everforest(Dark Medium,用户确认色值+官方色板补齐);`/theme`、`/settings`、选择器、启动恢复全接线;已有主题不受影响 |

### 验证记录(全部通过)

| 关卡 | 结果 |
|---|---|
| 受影响测试 | rpi-tui 337+6+8 全绿(含 `test_harbor/nord/everforest_preset`);rpi-cli 573+ 全绿 |
| `cargo test --workspace --locked` | **1934 passed / 0 failed**(75 套件;exit 0;唯一 panic 输出来自 `panicking_register_body_is_contained_and_skipped` 测试自身的预期 panic,该测试 ok) |
| 死锁回归 | `dialog_key_routing_releases_slot_locks_before_widget_callbacks` 1 passed(无挂死,确认实际运行) |
| Everforest 独立候选验证 | A: `/theme everforest` 切换立即生效 + settings.json 保存 ✓;B: 重启恢复(首屏零 Dark 残留,`38;5;108`=0)✓;C: 真实对话渲染——工具成功 bg `48;2;51;62;57`×20、失败 bg `48;2;64;54;60`×8、代码块 bg `48;2;48;58;64`×46、diff added `38;2;167;192;128`×17 / removed `38;2;230;152;152`×2、代码正文 `#d3c6aa` ✓;D: 旧 session 副本 `--export` ✓ |
| ask_user smoke | 面板弹出(选项渲染)→ Enter 正常关闭 → 下一条消息正常送达有回复("收到")→ exit 0 ✓ |
| 模型连通 | 验证 C 中真实模型调用+三步工具执行 ✓ |
| 验证环境隔离 | `RPI_CODING_AGENT_DIR=/tmp/rpi-ef-verify/agent` + `--session-dir /tmp/rpi-ef-verify/sessions`(凭据/models 均副本),日用环境零污染 |
| 安装入口检查 | local_master ✓ 工作树干净(stash 用户未提交改动后)✓ HEAD=c67ef26 ✓ |

### 产物与安装

- commit:local_master = `c67ef26`(由 71dc0b9 ff 快进,无 merge commit)
- 构建:安装用 `cargo install --path crates/rpi-cli --locked`(SKILL.md 口径)
- 产物 SHA-256:安装后 `~/.cargo/bin/rpi` = `b82614e1…e1f06`(与 `target/release/rpi` 核对一致);候选验证构建为 `a3a877b3…13a0f`(cargo install 复构建后哈希不同,同源码同 profile,功能等价;安装后 smoke:--version、旧 session 副本 --export 均通过)
- 安装:备份 → `rm` + `cp`(macOS 签名缓存纪律)→ `rpi 0.5.0` ✓
- 用户计划:安装后将重启全部 session(旧 session 兼容负担解除)

### 回退保险

- 旧二进制备份:`~/.cargo/bin/rpi-0.5.0-71dc0b9-bak`
- 版本红线:0.5.0 同版本号升级,格式无演进;若需回退,先按 SKILL.md 检查新 session 格式

### 遗留事项

- tty7 侧 Everforest 为文件主题(`~/.config/tty7/themes/everforest.yaml`),非内置
- feat/render-viewport-drift 与 feat/delegate-visibility 仍待处理(沿 #4 遗留)

### 增量 2026-10-19(午后):Everforest thinking 区分度微调(7a5a16c)

- 内容:Everforest preset `thinking_text` 从 muted 系 `#adb6ad` 改为官方注释色 grey1 `#859289`(用户反馈 thinking 与最终输出亮度差仅 ~30 区分不明显;新值对比度 3.84:1,与正文亮度差 67,叠加既有斜体)
- 验证:rpi-tui 337+6+8 全绿(test_everforest_preset 增加 thinking_text 断言);`cargo test --workspace --locked` **1934 passed / 0 failed**(exit 0);死锁回归 `dialog_key_routing_*` 1 passed 无挂死;安装后旧 session 副本 `--export` ✓
- 产物 SHA-256:`2830376a681cd3e0…`(cargo install 复构建,源码 7a5a16c)
- 回退保险:旧二进制 `~/.cargo/bin/rpi-0.5.0-dba6829-bak`(b82614e1)

### 增量 2026-10-19(晚):thinking 竖线包裹 + truecolor DIM 修复(02f3679)

- 内容:thinking 渲染重构——左侧 │ 竖线(结构性区分)+ 无斜体(真斜体字形笔画细于中文,行内中英明暗斑驳)+ 色值 #97a49b(轨迹 grey1 3.84:1 → 8f9a92 4.28 → 97a49b 4.81);**line_diff truecolor off-by-one 修复**(旧代码要求第 6 个参数,`38;2;R;G;B` 恰 5 参数全走拆分路径,`2` 落入 Intensity 槽成 `[2m` DIM → 折行续行整体被暗淡——上游既有 bug,所有主题彩色折行受影响)
- 验证:rpi-tui 338+6+8 全绿(新增 truecolor 回归测试);workspace **1935 passed / 0 failed**(exit 0);死锁回归 1 passed 无挂死;安装后旧 session 副本 `--export` ✓
- 产物 SHA-256:`857f5c42…3c3`(cargo install 复构建,源码 02f3679)
- 回退保险:旧二进制 `~/.cargo/bin/rpi-0.5.0-8cad397-bak`(2830376a)

## 晋级 #6:2026-10-21 · sync-output fallback——无 CSI 2026 终端的闪屏兜底

| 项 | 值 |
|---|---|
| 晋级 commit | `37a18a6`(feat/sync-output-fallback ff 链:51a4db0 初版 → ef01c6b codex 一审三修正 → 37a18a6 codex 复审残留修复) |
| 版本 | rpi 0.5.0(同版本多构建,以 SHA-256 区分) |
| 产物 SHA-256 | 安装构建 `05cbf5bd6cf36fb2…`(local_master 上 `--locked` 复构建);tty7 实测构建 `b659cef83d440547…`(feat 分支同源码同 profile,非确定性构建差异,功能等价——同 #5 现象) |
| 构建参数 | `cargo build --release -p rpi-cli --locked`(1m21s,thin LTO + codegen-units=16) |
| 内容 | 三处正交兜底:①主屏全量重建改覆盖式重绘(ED3 清固化 scrollback → home → 逐行覆盖+条件 EL → 尾部条件 ED0,消除整屏空白帧;首帧保留基线追加路径不覆盖 shell 输出);②终端能力探测(`synchronized_output_supported_for` 纯函数:白名单 ghostty/iTerm.app/WezTerm/kitty/foot/contour/vscode/Alacritty,黑名单 tty7/Apple_Terminal——行为实证,未知终端保守不降帧,`RPI_SYNC_OUTPUT=0/1` 强制覆盖);③降帧(仅黑名单终端 16ms→66ms,render_interval_ms 原子字段,调度线程每轮读;普通按键 parked 帧同样受 66ms 窗口,回显最坏延迟一个 interval) |

### 根因背景(留档)

rpi 每帧渲染包 `\x1b[?2026h/l`(CSI 2026 同步输出)且无条件发送;主屏重建原为前置 `\x1b[2J\x1b[3J` 清屏+整文档重写,清屏与重写之间的整屏空白窗口在不实现 CSI 2026 的终端上直接暴露为闪屏。对照链:ghostty(实现 2026)同 session 零闪 → tty7 旧构建闪 → feat 构建不闪(tty7 VT core 源自 alacritty_terminal,但 TermMode 位标志不能证明宿主行为,判定依据是 A/B 行为实证;Alacritty 本体 0.13.0+ 官方支持 2026 已移入白名单)。上游 v0.5.0..v0.7.1 无相关修复(已核对 release notes+commit log)。

### 验证记录(全部通过)

| 关卡 | 结果 |
|---|---|
| 受影响测试 | rpi-tui 346 lib+10 集成+8 全绿(新增 4 探针:满宽行重建/首帧追加/alt full_redraw/满宽末行清残留+探测单测 4 个);rpi-cli 586 全绿 |
| `cargo test --workspace --locked` | **1947 passed / 0 failed**(74 套件) |
| 死锁回归 | `dialog_key_routing_releases_slot_locks_before_widget_callbacks` 1 passed 无挂死 |
| codex 独立审查 | 一审 3 项(满宽 EL0 擦末列/首帧覆盖 shell 输出/Alacritty 误判)→ ef01c6b 修正;复审 1 项残留(满宽末行残留不补清——「下一帧补清」假设被证伪)→ 37a18a6 修正(实化换行 CR+LF 再 ED0 当帧清尽+记账);模拟终端按 Ghostty 语义校正(EL0/ED0 含光标格+pending wrap 屏外光标按末列起擦) |
| tty7 真机实测 | ①51a4db0 构建:恢复 33MB 长会话 01a15383(runloom)流式输出全程用户确认无闪烁(此前同终端同场景持续闪屏);②37a18a6 feat 构建(b659cef8):用户在之前闪烁的 session 重启测试确认通过 |
| 旧 session 副本 `--export` | feat 构建与安装后二进制各验一次,均成功导出 2.1MB HTML |
| 安装入口检查 | local_master ✓ ff 无 merge commit ✓ 工作树仅用户既有本地改动(AGENTS.md/Taskfile.yml,不参与构建) |
| 安装与冒烟 | 备份 → `rm`+`cp`(macOS 签名缓存纪律)→ 安装后 SHA 与 target/release 核对一致 → `rpi 0.5.0` ✓ |

### 已知遗留(如实)

- **CPU 热点未治**:每帧仍全量重渲染 transcript(strip_ansi/str_width),实测流式期间 CPU 100%。闪屏(IO 层)已解决;渲染缓存跨帧复用是独立优化方向(方案已论证未实施)
- 探测为静态名单,未做 DECRQM 动态探测(避免 stdin 竞争);名单外新终端需手工维护
- 降帧对黑名单终端的输入回显引入最坏 ~66ms 延迟(parked 帧;immediate render 不受影响)

### 回退保险

- 旧二进制备份:`~/.cargo/bin/rpi-0.5.0-1df07b2-bak`(857f5c42,#5 增量 02f3679 构建)
- 版本红线:0.5.0 同版本号升级,格式无演进;若需回退,先按 SKILL.md 检查新 session 格式

### 推送记录

- `origin/local_master` 推送成功:`1df07b2..37a18a6`(GitHub 网络本次正常,#4 遗留的补推已完成)

## 晋级 #7:2026-10-21 · 上游同步 v0.5.0 → v0.7.1(57 提交)

| 项 | 值 |
|---|---|
| 晋级 commit | `d2a3454`(feat/upstream-v0.7.1 链:da9b180 merge v0.7.1 → d2a3454 合并语义修复) |
| 版本 | **rpi 0.7.1**(跨 2 个 minor:0.5.0→0.6.0→0.7.1) |
| 产物 SHA-256 | 安装构建 `2fe06b3a1977c58a…`(local_master `--locked` 复构建);tty7 冒烟候选 `41f14cabbbcb6668…`(feat 分支同源码同 profile,非确定性构建差异,同 #5/#6 现象) |
| 构建参数 | `cargo build --release -p rpi-cli --locked`(1m36s,thin LTO + codegen-units=16) |
| 上游内容 | v0.6.1 resize 重绘修复(painted_size/resize_dirty/ensure_size 看门狗 + ?7l 重断言)、v0.6.2 iTerm2 队列编辑/provider 热切换、v0.6.3 SDK provider bridge 重构、v0.7.0 ACP/RPC 协议 + 原生 side chat(side_questions)、v0.7.1 provider perf |

### 冲突处理(5 文件 + 2 处自动合并语义损坏)

- tui_alt_screen.rs full_redraw:保留 fork 的「去前置 2J」防闪屏,吸收上游 v0.6.1 resize 修复(?7l DECAWM 重断言);**补尾部条件 ED0**(视口缩小时原 2J 顺手清掉的下缘残留);上游 4 个 resize 测试断言从 `contains("\x1b[2J")` 改为首尾行寻址(语义等价)
- terminal.rs tests:fork 4 个探测测试与上游 iTerm 归一化测试并列
- interactive_tui.rs:模块声明并列(thinking_peek/turn_telemetry + side_questions);render-tick 双看门狗并存(fork key-thread heartbeat + 上游 ensure_size/side chat sync)
- session.rs:保留 fork 的 create_delegate_task_tool 导入
- docs/README.md:上游「来源与致谢」+ fork 个人文档区并存
- tool_execution.rs(自动合并语义损坏 ①):上游 v0.7.1 把渲染拆为 render(缓存包装)→render_panel(纯渲染),fork 的 delegate 分支进 render_panel 后残留旧缓存块(引用外层局部变量)已删——缓存由外层 render 统一写

### 合并语义损坏 ②(根因留档,均为「编译干净但行为错误」)

**budget 截断误判自然完成的 run**(side_questions 3 个测试失败根因):
- 上游 v0.7.0 side chat 的 child 设 `max_turns=1` 作自然单轮上限;fork 的 RunBudget 把 max_turns 映射为截断线——第 1 轮即 TurnLimit,触发 budget_handoff_summary 额外 provider 调用,消耗响应队列并覆盖真实回答(探针实证:side question 拿到脚本第 2 个响应)
- 修复:budget_stop 过滤「叶子无 ToolCall 的 run」(自然完成),handoff/notice 仅对真截断生效

另:ResizableTerminal fork/上游重名去重(fork 版删除,测试改用上游 new()/output()/size())。

### 验证记录(全部通过)

| 关卡 | 结果 |
|---|---|
| `cargo test --workspace --locked` | **2018 passed / 0 failed**(77 套件;side_questions 18、上游 resize 4、harness budget e2e 全绿) |
| 死锁回归 | `dialog_key_routing_` 1 passed 无挂死 |
| 旧 session 副本 `--export` | feat 候选与安装后二进制各验一次,0.5.0 时代 86M runloom 会话成功导出 2.1MB HTML |
| tty7 冒烟 | 0.7.1 候选 TUI 正常启动;模型连通("ok" 回复,TTFT 7.0s;fork 的 TPS 遥测正常) |
| 安装入口检查 | local_master ✓ ff 无 merge commit ✓ 工作树仅用户既有本地改动(AGENTS.md/Taskfile.yml,不参与构建) |
| 安装与冒烟 | 备份 → `rm`+`cp` → SHA 与 target/release 核对一致 → `rpi 0.7.1` ✓ |

### 版本红线(本次跨 2 个 minor,必读)

- **升级后禁止回退 0.5.0 日常使用**:0.6/0.7 的 session 格式有演进可能(0.7.0 side chat 等新机制),旧二进制 fail-closed
- 旧二进制备份:`~/.cargo/bin/rpi-0.5.0-7cf6f87-bak`(05cbf5bd,晋级 #6 产物)——仅作应急,回退前先按 SKILL.md 检查新 session 格式
- 正在运行的旧 0.5.0 进程(tty7 各会话)继续用旧 inode 写旧格式(可读);新开会话即 0.7.1

### 推送记录

- `origin/local_master`:`7cf6f87..d2a3454` ✓;`origin/feat/upstream-v0.7.1` 新分支 ✓
- main 已快进至 v0.7.1(281dfdd,纯本地基座,永不推送)

### 遗留事项

- 升级后建议重启全部 tty7 会话(旧进程旧格式负担解除)
- CPU 热点(每帧全量重渲染)沿 #6 遗留,渲染缓存跨帧复用仍是独立优化方向
- 上游 issue(tty7 CSI 2026 + rpi 闪屏报告)待提交,证据链已备
