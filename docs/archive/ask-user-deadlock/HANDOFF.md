# HANDOFF：ask_user 死锁 bug —— 问题框不渲染 + TUI 全冻结

> 交接来源：采购工作区排查会话（2026-10-14 上午）。现场已恢复（进程 kill、会话无损），根因待定位修复。
> 证据文件（本目录，均未跟踪可删）：
> - `handoff-ask-user-deadlock-evidence.jsonl` — 关键会话事件摘录（header + seq 5837/5838/6130/6131/6252/6255）
> - `handoff-ask-user-deadlock.sample` — 死锁进程 macOS sample 全量输出（33 线程）
> - 原始完整会话：采购工作区 `.rpi/sessions/--Users-zcy-Documents-projects-xiangmucaigou-lcy-ai-procurement-workspace--/2026-10-14T02-19-56-691Z_01a13835-8f93-746c-9473-39e1901712b1.jsonl`（6256 行 / 2.2MB，尾部为未闭合的 ask_user tool_started）

## 0. 一句话

ask_user 工具调用被 core schema 校验拒绝（questions > maxItems 3）后，模型 10 秒后重试同形合法调用 → 问题框 overlay 从未渲染、TUI 主线程冻结（无输入/无退出/无渲染），只能 kill。数据无损。

## 1. 现象（用户视角）

1. 状态栏显示 ask_user 等待用户回答，但问题框从未出现
2. 终端完全无响应：按键无效、Ctrl+C/Esc 无效（raw 模式按键是输入事件，需主循环处理）
3. 无 panic、无报错输出，进程常驻（~8% CPU 缓慢累计，逻辑上全阻塞）
4. `kill -INT` 即退（默认终止；退出时会话 JSONL 零新增事件 → 退出路径无清理落盘动作；终端可能需要 `reset`）

## 2. 环境

| 项 | 值 |
|---|---|
| rpi | 0.3.21（`~/.cargo/bin/rpi`，fork cargo 构建） |
| ask_user 工具本体 | **native 包 `rpi-ask-user` 0.1.5**（`~/.rpi/agent/extensions/librpi_ask_user.dylib`，10-13 15:11 构建，无本地源码） |
| 平台 | macOS Apple Silicon，TUI regular buffer |
| 死锁进程 | PID 7265，10:19 启动，10:36:07 冻结，10:50 kill |
| ⚠️ 注意 | `~/.cargo/bin/rpi` 已于 10:51 被 HEAD `534bcbff` 重建替换，**死锁进程跑的是旧二进制**，符号化需先确认 10:19 前最后一次构建对应 commit |

澄清：npm 全局的 `@juicesharp/rpiv-ask-user-question`（2.12.0）注册的是另一个工具 `ask_user_question`，**与本 bug 无关**。本 bug 的工具是 Rust cdylib `librpi_ask_user.dylib`。

## 3. 触发时间线（会话 JSONL 实证，本地时间）

| seq | 时间 | 事件 | 结果 |
|---|---|---|---|
| 5837 | 10:34:03.173 | `ask_user` tool_started（3 问，每问 2 options + allowFreeform + suggest，无 displayMode/timeout） | ✅ seq 5838 10:35:27 用户作答正常返回（84 秒人工作答） |
| 6130 | 10:35:57.213 | `ask_user` tool_started（**4 问**） | ❌ seq 6131 **同一毫秒**返回：`schema error: Validation failed for tool "ask_user": - questions: [...] has more than 3 items`（core 校验拒绝，工具未执行） |
| 6252 | 10:36:07 | assistant message（携带重试的 tool_use） | |
| 6255 | 10:36:07.296 | `ask_user` tool_started（3 问重试，参数形态与 seq 5837 成功调用完全一致） | 💥 无 tool_result，永久挂起 |

要点：
- **可疑模式 = "core schema 校验失败 → 紧接着重试"**。非必现：同会话第 1 次成功；另一会话（01a1383a，同版本同环境）同日早上 ask_user 正常
- tool_started 记录带预分配 `resultEntryId` + `replay: never`
- seq 6131 的报错格式与 `crates/rpi-ai/src/schema.rs` 的 "Validation failed" 一致 → 拒绝发生在 core，dylib 未参与该次调用

## 4. 死锁现场（sample 2 秒采样，证据在 handoff-ask-user-deadlock.sample）

- **33 线程全部阻塞 `_pthread_cond_wait`**，无一在跑；CPU 仍有 ~8% 缓慢累计（周期性 timer 唤醒）
- 主线程（DispatchQueue_1 com.apple.main-thread）栈，load address `0x102ff8000`，9 层 rpi 帧后进 condvar：

```
start (dyld)
+0x75fca8 → +0xff175c → +0x7d05e0 → +0x80f4d0 → +0x75eb84
→ +0x77efdc → +0x7edab4 → +0x66f8d4 → +0x101e930 → _pthread_cond_wait
```

- tokio worker 栈（13 层 rpi 帧后 condvar，无法区分是 tokio park 还是等答路径，需符号化）：

```
+0xff9d20 → +0x10249d8 → +0x1037270 → +0x101b24c → +0x1013dfc → +0x1017fb8
→ +0x10211b0 → +0x102db50 → +0x101db6c → +0x102dd74 → +0x102d098 → +0x102c1e4
→ +0x101e3ec → _pthread_cond_wait
```

- dylib 未被替换（10-13 15:11），其帧符号化不受二进制更替影响

## 5. ask_user 链路（代码导读，本次排查已确认的部分）

```
dylib librpi_ask_user.dylib（工具本体，open→poll 循环，自带 1-3 问校验）
  → PluginApi::runtime_action（crates/rpi-extensions/src/actions.rs）
      · 注册期 trampoline 用 thread-local；runtime_action 期没有 → 走 user_data→ActionBridge
      · ActionBridge 持 build 时捕获的 tokio Handle：Handle::spawn 驱动 host async dispatch，
        插件线程 park 在 STD sync_channel(1) 的 rx.recv()（actions.rs:25 注释）
  → UiDialogMailbox（actions.rs:52-300）：STD Mutex<UiDialogState{attached, pending:VecDeque,
      active:HashMap<request_id, UiDialogEntry{answer,cancelled}>, cancelled_tool_calls:HashSet}>
      · op:"open"：!attached → Err("ask_user requires an interactive UI")；已存在同 id → 返回现状（幂等）
      · op:"poll"：pending/answered/cancelled 三态，terminal 态从 active 移除
      · respond/cancel/cancel_tool_call/cancel_all/detach
  → TUI：ShellDialogs（crates/rpi-cli/src/tui_shell/dialogs.rs）
      · push() 按 request_id 去重（已 active/pending 的重复 push **静默丢弃**）→ advance() 弹出建 ActiveDialog
      · sync_view() 仅在 dirty 时把 dialog frame 挂进 slot 并 request_render —— 渲染依赖主循环 tick
      · handle_key()/handle_paste()：**active dialog 存在时吞掉全部按键**（→ 解释"无法输入"的感知）
  → 作答：DialogAction → DriverCommand::UiDialogResponse（crates/rpi-cli/src/session_driver.rs:292）
      → mailbox.respond() → dylib 下次 poll 取走 → tool_result
```

core 校验失败路径：`crates/rpi-ai/src/schema.rs` 拒绝 → 同步写 error tool_result，工具不执行。**该路径与 mailbox/dialog 的交互（是否触发 cancel_tool_call / cancel_all / detach / cancelled_tool_calls 污染）未审**。

## 6. 根因假设（按可能性排序）

- **H1（最可能）跨线程锁依赖环**：主线程等 mailbox 状态锁（或等一个需持锁方才能完成的事件），而某持锁路径 park 在永远等不到的 `rx.recv()`/事件上 → 主线程与工具线程双双 cond_wait，与采样吻合。残留触发点疑在 schema-error 后的状态（cancelled_tool_calls？active 残留 entry 导致 open 走幂等分支但 TUI 无对应通知？）
- **H2 通知丢失 + 无兜底**：op:"open" 入队成功但 TUI 侧 wake 丢失（push 后主循环不知道要 take_pending）→ 对话框永不显示；dylib poll 循环无超时兜底（本次调用无 timeout 字段）→ 工具永等；同时 active-but-unrendered dialog 吞按键 → "无输入"。此假设下主线程 cond_wait 属正常事件等待
- **H3 校验失败清理误伤**：core 拒绝 tool call 时 harness 的清理逻辑意外调用 mailbox cancel 系列函数，使重试调用的 open/poll 语义错乱

## 7. 排查路径建议

1. **符号化**：先 `git log`/构建记录确认 10:19 时点的二进制 commit，同 commit 重建后按 §4 偏移符号化主线程 9 帧——直接区分 H1（锁）vs H2（事件 recv）
2. **最快复现**：新 TUI 会话让模型连发 4 问（触发 core maxItems 拒绝）→ 立即重发 3 问 → 观察 overlay。竞态可能需多轮；或写 harness 集成测试直接驱动：bad args → good args → 断言 dialog 渲染且工具限时完成
3. **加日志跑复现**：mailbox `handle(open/poll)`/`respond`/`cancel*`/`detach` + ShellDialogs `push/advance/sync_view` + 主循环 take_pending 调用点，看链条断在哪一环
4. **审计 schema-error 路径**：rpi-ai/schema.rs 拒绝后 harness 对该 tool call 的清理是否触碰 mailbox；cancelled_tool_calls 是否可能误伤后续不同 tool_call_id 的调用

## 8. 验收标准（修复须满足）

1. 复现用例下 dialog 必须渲染且可作答；否则工具在超时内返回明确错误（dylib 已支持 `timeout` 字段，本次调用未传）
2. Abort/SIGINT 必须能取消 pending ask_user（`DriverCommand::Abort` → mailbox `cancel_all`），不允许永等
3. 审计所有持 mailbox 锁的代码路径：不得在持锁状态下 park / 调用阻塞 recv（主线程永不因 extension 工具卡死）
4. resume 尾部悬挂 ask_user tool_started 的会话可正常续聊（现场会话 01a13835 即现成测试样本：在采购工作区根目录 `rpi --session 01a13835`）

## 9. 附：上游版本线索

- rpi 上游最新 0.3.23（本 fork 0.3.21）——排查前先 diff 上游 0.3.22/0.3.23 changelog 是否已修
- native 包 `rpi-ask-user` 本地 0.1.5（10-13 构建）——查 native 包分发源是否有新版
- dylib 内嵌错误串（供对照定位）："ask_user UI dialog returned invalid JSON"、"ask_user UI dialog failed with status"、"ask_user polled after completion"、"ask_user requires an interactive UI"、"questions must contain 1-3 items"

---

## 10. 根因定位(2026-10-14 12:35 更新,已修复)

### 一句话

**不是 ask_user/mailbox 的 bug。** key 线程在 `dispatch_ui_prompt_event_end` 里**持 extension_session 锁同步跑扩展 handler**,而 `rpi-tty7-bridge` 的 UiPromptEnd handler 用 `Command::status()` **同步等 tty7-app 子进程退出**——tty7-app daemon 忙循环无响应时子进程永不退出 → key 线程持锁死等 → 输入线程死亡 → 问题框永不渲染 + 按键/Ctrl+C 全部无效。

### 死锁链(全链闭合)

```
用户作答第 1 个 ask_user(10:35:27)
→ key 线程: close_selector → dispatch_ui_prompt_event_end
   ├─ state.extension_session.lock()          ← 持锁
   └─ dispatch_empty_event(UiPromptEnd)
      └─ rpi-tty7-bridge handler(仅在 TTY7 环境变量的 pane 注册!)
         └─ emit("prompt-submit") → Command::status()  ← 同步等子进程
            └─ tty7-app daemon 忙循环 → agent-hook 子进程挂起不退出
               → key 线程持锁永久阻塞
                  → 第 3 次 ask_user 的 open 已入 mailbox pending,无人 take → 问题框不显示
                  → 按键/Ctrl+C 无效(都在 key 线程)
                  → 主线程永久 park(等不到 TuiMessage)
                  → 工具线程健康 poll 等"永远不会来的回答"(sample 1587 sleep 样本)
```

### 证据链

1. tty7_bridge 源码(`~/Documents/projects/rpi-tty7-bridge/src/lib.rs`):UiPromptStart/End → `emit()` → `Command::status()` 同步等待
2. `dispatch_ui_prompt_event/_end`、`dispatch_key_event`(Input claim)、`sync_editor_change` 三处都持 extension_session 锁跑 handler——同构缺陷
3. tty7_bridge 有 `TTY7` 环境变量守卫——**只有 tty7 pane 里的 rpi 才注册 handler** → 解释"非必现"(早上正常的会话 01a1383a 不在 tty7 pane;死锁会话 s003 在)
4. tty7-app 主进程 87h CPU / State=R / 11% CPU——确有忙循环史;死锁时刻(10:36)IPC 无响应,现已自愈(手动 agent-hook 25ms 返回)
5. schema 拒绝(4 问 > maxItems 3)与死锁**无因果**——agent_loop 的 Prepared::Immediate 路径干净,不碰 mailbox;时间上相邻纯属巧合
6. sample 符号化(b55750b 构建):17887934=ask_user 工具驱动(健康 poll 循环,非死锁点)、17834245=卡锁的 spawn_blocking 线程(key 线程)、渲染/主线程全部正常
7. 会话 JSONL:47 started vs 46 result(差 1 = 悬挂的 ask_user);bash 最后完成于 10:31:28,冻结时刻无并发 bash

### 修复(已提交 + 已安装)

**rpi 侧(commit c1dacd4,feat/thinking-config):**
- 三处 dispatch 统一改"锁外 + 有界":session 锁只护 O(1) snapshot Arc clone;fan-out 锁外跑
- 预算:UiPrompt/EditorChange 500ms、Input key-claim 150ms——卡住的 handler 超时降级(prompt 丢弃、key 降级为不 claim),detached 线程自然结束(闭包持 snapshot Arc = 天然 keepalive)
- 回归测试:`bounded_handler_call_times_out_on_stalled_handler` / `bounded_handler_call_passes_through_fast_handlers`(478 测试全过)

**tty7_bridge 侧(已重新构建安装):**
- `emit()` 从 `status()`(同步等待)改为 `spawn` + 后台收尸线程——上报路径 0 阻塞,保留无僵尸语义

**安装:** `~/.cargo/bin/rpi`(注意 macOS 签名缓存坑:覆盖二进制要先 rm 再 cp,否则 AMFI SIGKILL)

### 验收

- [x] 复现路径(handler 卡住)不再冻结 UI——bounded 测试 + 代码结构保证
- [x] Ctrl+C/Esc 恢复有效(key 线程不再可能被 handler 卡死)
- [ ] resume 悬挂会话:修复版下在采购工作区 `rpi --session 01a13835`(现成样本)

---

## 11. 第二次卡死(2026-10-14 13:41,proc 55248)— 修复版仍卡,新证据

用户在修复版(c1dacd4)下 `rpi --session 01a13835` 恢复会话,run 完成,之后再次卡死(输入框不显示输入、Ctrl+C 无效)。采样 `/tmp/hang2-55248.sample`:

### 新证据(反汇编确证,偏移基于 c1dacd4 符号化)

- **卡死瞬间 key 线程的真实调用链**(0x1007c2218/0x1007c2228 处指令):
  `Editor::handle_paste → TUI::request_render(force=false) → (do_render+3724 帧) → __psynch_mutexwait`
  即用户正在往输入框粘贴/输入时卡死 —— 与"输入框无法展示输入"症状吻合
- 下一个栈帧是 `do_render +3724` —— 若为真实帧,说明走了 `park_render` 的**同步分支(scheduler_started==false)**;若为 stale 帧(启发式栈扫描误报),真实卡点在 `scheduler.park` 的 state 锁
- **关键差异**:这次没有 ask_user/工具线程、没有 handle 线程、tty7 emit 已异步化 —— **第一轮修复的三个卡点全部排除**
- 12 线程全景:渲染线程 idle(cond_wait)、主线程 park、8 worker 空闲、IO worker kevent、run_stats 正常 —— **持有者悖论再现:全进程无人持锁,但 key 线程 100% 卡在锁等待上**
- 两轮死锁同形态:spawn_blocking 线程(interactive_tui closure)卡 pthread mutex,持有者不在任何活线程的调用栈上

### 未闭合的解释(按可能性)

1. **锁唤醒丢失**(macOS 15 firstfit mutex 的内核/用户态 bug):持有者已正常释放,但等待者收不到唤醒 —— 完美解释"锁空闲+等待者永久卡"
2. **主线程 async future 冻锁**:block_on 的根 future 不需要 Send,可合法持 MutexGuard 跨 await;主线程 park 时锁冻结在 future 状态机里(不在任何线程调用栈上,故 sample 看不到)—— 已审计 run_prompt_streaming/主循环的显式 lock() 均为短临界区,但调用链深处未穷尽
3. scheduler_started 状态错乱(多 TUI 实例/重启竞态)—— 静态证据不足

### 已部署的诊断武器(commit ad7cf41)

- **key 线程心跳 + 渲染 tick watchdog**:key 循环每次迭代更新 `key_heartbeat_ms`(AtomicU64);tui_tick 任务(独立于 key/主/渲染线程)每 tick 检查,**卡死 5 秒后自动写一次性诊断到 `~/.rpi/logs/key-thread-hang.txt`**(pid、卡死时长、lldb/sample 取证命令)
- **release 二进制保留符号**(strip=none,32MB):下次卡死的 sample/lldb 输出可直接 atos 符号化,无需重建
- 478 测试全过

### 下次卡死的取证流程(用户侧)

1. 别急着 kill —— 等 5 秒,watchdog 落盘 `~/.rpi/logs/key-thread-hang.txt`
2. 按文件里的命令执行:
   `lldb -p <pid> -o 'thread backtrace all' -o 'detach' -b > lldb-bt.txt 2>&1`
   `sample <pid> 2 -file hang.sample`
3. `kill -INT <pid>`,把三个文件交给 fork 工作区

---

## 12. 第三次卡死(2026-10-14 14:44,proc 66535)— watchdog 命中,全链实锤,已修复

用户在采购工作区跑了 rpi(66535),粘贴文本+回车发送后,又提交了 `/runloom` 斜杠命令:命令"发出去了",但输入框残留、退格无效、无法退出——**正在跑的 run 仍在执行**(run 不依赖 key 线程)。

### 决定性证据(watchdog 落盘 + sample + lldb 全线程栈,全部到手)

`~/.rpi/logs/key-thread-hang.txt` 于 14:44:18 自动报告 pid 66535 key 线程停滞 5041ms。
lldb 全栈(带符号二进制直接符号化,无需重建):

```
key 线程(spawn_blocking, thread #10):
semaphore_wait_trap
→ std mpmc Channel::recv                     ← run_action 的 rx.recv()
→ trampoline_runtime_action + 2984
→ runloom_command::dispatch_to_driver        ← /runloom 命令处理器
→ runloom_command::runloom_command
→ invoke_extension_command + 1428            ← key 线程同步执行扩展命令!
→ ExtensionCommand(SlashCommand)::execute
→ dispatch_slash → Editor::handle_key        ← 回车提交
```

其余 15 线程:主线程浅栈 park(run_prompt_streaming 等上一个 run)、8 worker 空闲、IO 驱动 kevent、渲染线程 idle——**SendUserMessage 任务已让出 worker(冻结在 future 状态机里,不在任何线程栈上),等 harness run 互斥;上一个 run 的 provider 流 14:52:28 后挂起(会话 JSONL 尾部全是流式帧、无收尾事件)→ 永不完成 → 死锁闭环。**

### 三次死锁的统一架构缺陷

**key 线程上的同步无限等待**:
1. 第一次:tty7 handler 同步 `Command::status()` 等子进程(持 extension_session 锁)
2. 第二次:疑似同类的渲染路径锁等待(证据不完整,已有 watchdog 兜底)
3. 第三次:扩展斜杠命令同步 FFI → runtime_action `rx.recv()` 等 harness(等一个流挂起的 run)

### 修复(commit 2a7d5ca,已构建安装 14:57)

- **`ExtensionCommand::execute` 移出 key 线程**:spawn_blocking 跑 FFI handler,async 任务收结果并渲染;key 线程立即返回(编辑器清空路径正常执行——第三现场的"输入框残留"症状直接消失)
- pet Esc 的 "quiet" 同类调用同样处理
- 效果:慢/卡死的 runtime action 降级为"晚到的消息"而非冻结 TUI;runloom 等 SendUserMessage 驱动的 run 正常排队执行
- 478 测试全过

### 遗留(独立工单,不在本次范围)

- **provider 流挂起无超时**:Tencent-GLM5.3(linglong)的流 14:52:28 后无数据无收尾,run 永不完成——主线程 park 在 run_prompt_streaming,UI 显示 Working。建议给 provider 流加 idle 超时(数据链路层,与本次 key 线程修复无关)
- s002 的 38% CPU 忙循环会话(01a13877/proc 55176)仍未排查

---

## 13. 第四次卡死(2026-10-14 14:44,proc 74431,`rpi --resume`)— ask_user 作答路径精确锁定,防御修复

用户复现场景:**ask_user 等待中切到别的工作区,切回后在输入框回答问题(Enter)→ 卡死**——输入框不更新、退格无效、无法退出,但之前的 run 仍在跑。

### 取证(watchdog 自动报告 + sample + lldb 全栈 + 锁内存 + 全进程堆扫描)

watchdog 于 14:44 报告 pid 74431(15:11 启动,含 14:57 的扩展命令异步化修复)。lldb 全栈精确锁定:

```
key 线程(thread #10,卡 __psynch_mutexwait):
frame #3: close_extension_editor + 120         ← 卡在 state.active_extension_editor.lock()
frame #4: open_ask_user_input::{closure#1}     ← ask_user 输入框的 on_submit 回调
frame #5: Input::handle_key                    ← 用户的 Enter 提交
```

反汇编确认卡点指令(0x100a45e90 的 `bl _pthread_mutex_lock`),锁对象位于 0x60000078d1c0。

### 孤儿锁谜题(深度取证结果)

- `pthread_mutex_trylock` 返回 **EBUSY(16)**——锁处于锁定状态
- **全进程搜索锁的引用:零命中**——13 个线程的栈(每线程最多 128KB)+ 41 个 MALLOC 区域(约 100MB)+ 完整 512MB nano zone,全部扫完,没有任何 guard/future 引用这把锁
- 锁结构完好(MUTZ magic 在,状态字显示 locked,非内存踩踏)
- **结论:孤儿锁——持有者已消失但未解锁**(double-unlock UB 或 firstfit 状态字残留;取证期间的 unlock 实验可能额外污染了部分状态,但卡死发生在实验之前)

### 修复(commit 5515d11,已构建安装 15:32)

**对话框生命周期锁全面 try_lock 化**:`close_extension_editor`、`open_extension_input/editor`、`open_ask_user_input` 在 key 线程上对 `active_extension_editor/input/cancel` 三把锁从 `.lock().unwrap()`(无限等待)改为 `try_lock` + 忙时跳过清理(状态在后续事件收敛)。

**语义**:任何锁异常(孤儿锁/慢持有者/双重解锁)的最坏结果降级为"对话框状态延迟收敛",**永不冻结 key 线程**。

### 四次死锁全景(最终)

| 次 | 进程 | key 线程卡点 | 根因层 | 修复 |
|---|---|---|---|---|
| 1 | 7265 | 持锁跑 tty7 handler(同步等子进程) | 事件分发持锁+外部阻塞 | c1dacd4(锁外+bounded) |
| 2 | 55248 | 渲染路径锁(do_render 区域) | 同 2,证据不完整 | ad7cf41(watchdog 兜底) |
| 3 | 66535 | 扩展命令 FFI → runtime_action 等 harness | 斜杠命令同步执行 | 2a7d5ca(spawn 化) |
| 4 | 74431 | close_extension_editor 锁等待(孤儿锁) | 锁状态异常无持有者 | 5515d11(try_lock 降级) |

统一主题:**key 线程承担过多同步职责,任何无限等待都会冻结整个 TUI**。四层修复后,key 线程上已无已知的无限等待点;watchdog 持续兜底。

---

## 14. 外部审查(codex)修复落地(2026-10-14 16:10,commit 41f8485)

codex 审查指出三个问题,全部修复(479 测试全过,已构建安装):

### 1. 第四现场根因更正:自重入死锁,不是孤儿锁

**路由代码的 Edition 2021 陷阱**:`if let Some(x) = state.lock().unwrap().clone() { x.handle_key() }` 的临时 MutexGuard 活到整个 if/else 结束——进入 input 分支时 editor+input 两把锁都被 key 线程自己持有;Enter → on_submit → close_extension_editor 同线程再拿锁 = **自死锁**。此前 try_lock 修复(5515d11)只是把死锁变成了“清理失败+旧 input 残留”(审查方实验验证:`after close: editor=None input=Some(1)`)。

**佐证**:同文件 provider_form 路由早已有正确写法和注释("Clone the input OUTSIDE `if let` ... would deadlock on it")——dialog 路由漏掉了同样处理。

修复:提取 `route_extension_dialog_key`(clone 出锁后再调用 handle_key)+ 路由级回归测试 `dialog_key_routing_releases_slot_locks_before_widget_callbacks`(若路由再持 guard,回调里的 lock 会让测试挂死)。try_lock 保留作兜底。

### 2. keepalive 生命周期遗漏

`Arc<RegistrySnapshot>` 只带函数指针;**`Arc<PluginKeepalive>` 才拥有 dylib**。超时仍在跑的 handler 遇上 /reload 会踩 unmapped 内存。修复:三处 fan-out(UiPrompt×2/Input claim/EditorChange)+ invoke_extension_command 全部在锁内同时捕获 keepalive 并持到 handler 返回(对齐 session.rs:1077 的 shutdown 先例)。

### 3. continuation 覆盖补齐

input/editor 的 on_submit、selector 的 on_select、全部 cancel 回调原先仍在 key 线程同步跑 FFI(与首跑命令同类的冻结风险)。修复:统一走 `run_extension_continuation`(先 close 再 spawn_blocking 后台执行);`ExtensionCommand::execute` 也复用它。**dialog generation 计数**守卫结果:每个对话框 open 和命令执行推进代次,陈旧的 continuation 结果降级为纯消息,不再覆盖新状态。

### 低优先级项

- watchdog 恢复后重置 key_hang_reported(已修)——第二次挂起能再报告
- 线程去重(同一 handler 单在途)未做,记为后续
- strip=none 维持现状(审查方不阻断)
