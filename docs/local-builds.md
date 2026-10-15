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
