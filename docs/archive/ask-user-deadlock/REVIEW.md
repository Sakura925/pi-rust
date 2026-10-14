# Code Review 交接:ask_user / TUI 冻结系列死锁的修复(4 个 commit)

> 供 AI 审查用。仓库:`pi-rust`(fork),分支 `feat/thinking-config`,基座 e925a22。
> 待审 commit(时间序):`c1dacd4` → `ad7cf41` → `2a7d5ca` → `5515d11`。
> 证据文件:`HANDOFF-ask-user-deadlock.md`(完整排查史,§1-13)、`/tmp/hang*.sample`、`/tmp/lldb-bt-*.txt`(部分现场可能已清理)。

## 1. 问题背景

rpi(rust 编码 agent CLI,普通交互 TUI)在 macOS 15.4 / Apple Silicon 上连续发生 4 次"整窗冻结":输入框不更新、按键无效、Ctrl+C 无效,只能 `kill -INT`。共同形态:**key 线程(spawn_blocking 的 crossterm 输入循环)死于同步无限等待**,其余线程(主线程 park、渲染线程 idle、tokio worker 空闲)全部健康,因此没有任何自愈路径。

四次现场(全部有进程级取证):

| # | 进程 | 触发场景 | key 线程死因(lldb/sample 实锤) |
|---|---|---|---|
| 1 | 7265 | ask_user 3 问作答后,模型重试第 3 次调用 | `dispatch_ui_prompt_event_end` **持 extension_session 锁**同步跑扩展 handler;rpi-tty7-bridge 的 handler 用 `Command::status()` 同步等 tty7-app 子进程(daemon 忙循环时永不退出) |
| 2 | 55248 | 粘贴文本后(修复版仍卡) | 键线程闭包 → `TUI::request_render` → `do_render` 区域 → `__psynch_mutexwait`;持有者未定位(证据不完整,栈帧有污染) |
| 3 | 66535 | 提交 `/runloom` 斜杠命令 | `ExtensionCommand::execute` 在 key 线程同步跑 FFI → runloom 调 `SendUserMessage` runtime action → trampoline 的 `rx.recv()` 永等(spawn 出的任务排队在 harness 的 run 互斥后,而前一 run 的 provider 流挂起无超时) |
| 4 | 74431 | ask_user 输入框 Enter 作答 | `on_submit` 回调 → `close_extension_editor` → `state.active_extension_editor.lock()` 永等。**深取证:该锁 EBUSY 但全进程(13 线程栈 + 全部 malloc 区域 + 512MB nano zone)无任何持有者引用——孤儿锁**(成因未闭合:疑似 double-unlock UB 或 firstfit 状态残留) |

统一架构结论:**key 线程承担了对话框生命周期、渲染请求、扩展命令、事件分发等过多同步职责;任何一处无限等待都会冻结整个 TUI(输入/对话框/Ctrl+C 共用该线程)。**

## 2. 修复设计(按 commit)

### c1dacd4 — 事件 fan-out 锁外化 + 有界化

`crates/rpi-cli/src/interactive_tui.rs`、`state.rs`:

- `dispatch_ui_prompt_event/_end`(UiPromptStart/End)、`dispatch_key_event` 的 Input claim、`sync_editor_change`(EditorChange)三处原本在**持有 `state.extension_session` 锁的状态下同步调用扩展 handler**(handler 是 FFI,可任意慢)。
- 改为:session 锁只护 O(1) snapshot Arc clone;fan-out 锁外执行,经 `bounded_handler_call`(spawn 线程 + `recv_timeout`)有界等待:UiPrompt/EditorChange 500ms、Input claim 150ms;超时降级(prompt 丢弃、key claim 降级为 not-claimed)。
- detached 超时线程闭包持有 snapshot Arc,天然 keepalive,自生自灭。

### ad7cf41 — key 线程 watchdog + 取证转储

- key 循环每次迭代写心跳(`key_heartbeat_ms`,AtomicU64,以 `tui_start_instant` 为 epoch)。
- 渲染 tick 任务(独立于 key/主/渲染线程)检查心跳,停 5s 即一次性转储 `~/.rpi/logs/key-thread-hang.txt`(pid、时长、lldb/sample 取证命令)。
- **release 二进制改为 strip=none(约 32MB)**:下次现场 sample/lldb 可直接符号化。此变更是否保留请审查(取舍:取证能力 vs 体积)。

### 2a7d5ca — 扩展斜杠命令移出 key 线程

`crates/rpi-cli/src/interactive_tui/extensions.rs`:

- `ExtensionCommand::execute` 原在 key 线程同步执行 FFI handler(即第 3 现场死因)。改为 `spawn_blocking` 跑 handler + `tokio::spawn` 的 async 任务收结果并渲染;key 线程立即返回(编辑器清空路径不再被阻断)。
- pet Esc 的 "quiet" 同类调用同样 spawn 化。
- 副作用语义变化:命令结果消息与其他 UI 事件可能乱序(晚到的消息);扩展命令失败降级为错误消息而非阻塞。

### 5515d11 — 对话框生命周期锁 try_lock 化

`extensions.rs` 的 `close_extension_editor`、`open_extension_input/editor`、`open_ask_user_input`:

- 对 `active_extension_editor/input/cancel` 三把 `std::sync::Mutex` 从 `.lock().unwrap()` 改 `try_lock` + 忙时跳过。
- 动机:第 4 现场的孤儿锁(锁定态、无持有者)无法根因修复,防御目标改为"锁异常永不冻结 key 线程";最坏降级为对话框状态延迟收敛。

## 3. 请重点审查的问题(作者自陈的不确定点)

1. **try_lock 降级的状态一致性**:`close_extension_editor` 跳过清理后,`active_extension_input` 残留 `Some(旧 input)`,下一次按键仍路由到已关闭的对话框?路由处 `interactive_tui.rs:2100-2110` 读的是同一批锁(仍是 `.lock()`),会不会出现"close 跳过清理 + 下一键仍进旧对话框"的活锁或重入?是否需要在路由处也做一致性收敛?
2. **bounded_handler_call 的 detached 线程堆积**:每次 UiPrompt 事件(每次 selector/dialog 开关)在订阅者存在时都 spawn 线程并可能超时遗留;ask_user 多问题场景下事件频率如何?是否会积累(线程泄漏上限)?
3. **ExtensionCommand::execute 异步化的时序**:同一命令重入(用户连按两次 Enter / 命令 A 未完成又提交 B)时 `spawn_blocking` 任务并发执行 FFI handler——插件命令是否需要串行化(队列)?`handle_extension_ui_result` 中 `open_extension_*` 与 ask_user 抢占同一 input 槽的交错(第 4 现场的场景可能与此有关)。
4. **watchdog 的误报**:key 线程合法长阻塞点(外部编辑器、长时间 paste burst、`crossterm::event::read` 50ms poll 本身)是否会超 5s 触发误报?心跳写在循环顶部,`read()` 无按键时最长等多久?
5. **孤儿锁成因未闭合**:全进程无持有者的 EBUSY 锁;取证期间的 `pthread_mutex_unlock` 实验可能污染部分状态,但卡死先于实验。审查者若有 firstfit mutex / double-unlock UB 方向的洞察,欢迎补充;也请检查代码里是否存在可疑的 double-unlock 或 guard 泄漏路径(如 `lock().unwrap_or_else(|e| e.into_inner())` 模式之外的裸 mutex 操作)。
6. **strip=none 与 Cargo.toml**:profile.release 的 strip 改动是否应该用条件(cfg/debug_assertions 或环境变量)而不是永久保留。
7. **回归测试覆盖**:当前仅 `bounded_handler_call` 的超时/直通单测;try_lock 降级、扩展命令异步化均无行为测试。测试基建(`interactive_tui/tests.rs`)是否足以补(需要 TuiState 构造已有先例)。

## 4. 已知遗留(不在本次修复范围,已记录)

- provider 流挂起无超时(Tencent-GLM5.3/linglong;第 3 现场里 run 永不完成的源头,现在只是不再连带冻结 TUI)。
- s002 终端 38% CPU 忙循环会话(proc 55176,`--session 01a13877`)未排查。
- 第一次死锁会话(01a13835)尾部悬挂 ask_user 的 resume 验证在修复版下尚未完整回归(用户中途再次遇到冻结,即第 4 现场)。

## 5. 复现/验证指引

- 关键复现场景(第 4 现场):TUI 中 ask_user 弹出 → 切换终端 → 切回 → 输入框作答 Enter;修复后 Enter 必须立即返回(锁异常时允许 UI 状态延迟收敛)。
- 第 3 现场场景:在装了 runloom 扩展的工作区提交 `/runloom continue`(在 run 进行中);修复后命令立即返回,run 排队执行。
- watchdog 触发自检:人为 stall key 线程难做;可单测心跳逻辑(当前未覆盖)。
- 全量测试:`cargo test -p rpi-cli --lib`(478 passed)。

---

## 审查结论落地记录(2026-10-14 16:10,commit 41f8485)

外部审查(codex)指出三个问题,全部修复落地(479 测试全过,已构建安装 16:10 版):

1. **自重入死锁根治(第四现场根因更正)**:路由代码 `if let Some(x) = state.lock().unwrap().clone()` 的临时 guard 在 Edition 2021 下活到整个 if/else 结束,Enter 回调重入锁 = 自死锁;我此前的 try_lock 修复只是把死锁降级为“旧 input 残留”。真正修复:提取 `route_extension_dialog_key`(clone 出锁后再调用,对齐 provider_form 路由的既有先例)+ 路由级回归测试(若路由再持 guard,回调里的 lock 会让测试挂死)。“孤儿锁”观察更正为:自重入死锁 + try_lock 降级的表象 + 取证实验的状态污染。
2. **keepalive 生命周期**:三处 fan-out + invoke_extension_command 全部在锁内同时捕获 `Arc<PluginKeepalive>` 并持到 handler 返回(snapshot 只带函数指针,keepalive 才拥有 dylib;对齐 session.rs:1077 先例)。
3. **continuation 覆盖补齐**:input/editor 的 on_submit、selector 的 on_select、全部 cancel 回调原先仍在 key 线程同步跑 FFI;统一改走 `run_extension_continuation`(先 close 再 spawn_blocking),`ExtensionCommand::execute` 也复用它;dialog generation 计数守卫陈旧结果(降级为纯消息)。
4. **低优先级**:watchdog 恢复重置已补;线程单在途去重记为后续;strip=none 维持。
