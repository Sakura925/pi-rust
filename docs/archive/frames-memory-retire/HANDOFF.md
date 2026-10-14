# HANDOFF — rpi 会话恢复内存驻留修复（frames retire on clear-run）

> 交接时间：2026-10-13 23:40 左右
> 交接自：lcy-ai-procurement-workspace 工作区的 rpi 会话（编排者）
> 接手：pi-rust 工作区 agent（本仓库 feat/thinking-config 分支）
> 状态：**补丁已实施，测试剩 1 个失败待处理**（见 §5），处理后走 §7 验证与提交。

---

## 1. 任务目标

修复 rpi（本仓库的 CLI）的**会话恢复内存驻留**问题：长会话用 `--session`/`--continue` 恢复后，RSS 飙到启动时的 ~10 倍（实测 189MB→200MB+，用户报告）。要求：测试全绿 → 编译安装 → 端到端实测内存对照 → 提交。

## 2. 根因（已定案，证据链完整）

**问题**：`crates/rpi-harness/src/session/state.rs` 的 `apply_mutation` → Record 分支，把**每条** `LaneRecord`（含 `AssistantFrame` 流式帧记录）整份 clone **两份**永久驻留：

```rust
self.records.push(record.clone());
self.log.push(LogItem::Record { seq, record: record.clone() });
```

frames 按设计是 **"progress, not history"**（`AssistantFrameOp` 的 doc 注释原话）：流式进度记录，写盘增量，只服务 crash salvage。恢复会话时 reducer 全量重放 JSONL → 77,393 条 frame 记录 ×2 份 clone 驻留内存。

**量化证据（2026-10-13 实测）**：

| 实验 | 会话文件 | footprint | 活跃分配 |
|---|---|---|---|
| 原会话恢复（77K 条 frame） | 25.4MB | **234.6M** | **172.8M** |
| 过滤全部 frames 后恢复 | 1.9MB | **46.7M** | **33.0M** |

→ **~75%（140MB）驻留来自 frames**。frame 的 payload 是 `serde_json::Value` 树，结构化放大 ~3-4 倍 ×2 份 clone。

会话文件构成本例：77,393 条 assistant_frame（92% 磁盘体积）+ 480 条消息 entries（文本 434KB）。

## 3. 补丁设计（已实施）

**语义**：frames 的内存生命周期 = 所属 run 未完结期间；磁盘保留全量（append 写入路径不动）。

**实现**（已改 `crates/rpi-harness/src/session/state.rs`）：

`apply_mutation` 的 Record 分支，在 `AssistantFrame` 且 `op == AssistantFrameOp::ClearRun` 时：

1. `self.records.retain(...)` 清掉该 `run_id` 的全部 frames
2. `self.log.retain(...)` 同样清
3. 这条 clear-run 标记本身也不驻留（used_ids 照常登记，seq 照常推进）
4. 其余记录（非 ClearRun 的 frame 与所有非 frame 记录）行为不变，照常 push

**语义推演**（自检过）：

- **load 重放**：顺序流过——run A 的 frames push → run A 的 clear_run 到达 → retain 清 → run B push → 清……**峰值 = 单个 run 的 frames**（不是全量），这是内存修复的关键
- **open run（crash 后 load，无 clear_run）**：frames 留在 `records` → `replay_run_frames`（salvage）仍可查到 → **crash 恢复语义不破坏**（`reloaded_frames_reduce_to_an_interrupted_message` 测试已验证通过）
- **运行时**：新 frames push → run 完结 clear_run → 清；稳态 ≈ 0
- **get_log 消费者**：frames 从 log 视图消失（get_log 只有测试/导出在用，全仓库无运行时依赖——已排查过：session.rs/sqlite.rs/memory.rs/jsonl/storage.rs 是转发层，m5c_jsonl_atomic_publish.rs 是测试）

改动点：
- `crates/rpi-harness/src/session/state.rs`：Record 分支 + use 引入 `AssistantFrameOp`
- `crates/rpi-harness/tests/frame_progress_recovery.rs`：断言方式适配（见 §4）

未动的：`append_record`/写盘路径（磁盘保留全量 frames）、sqlite/memory 存储（共用同一 state）、`used_ids`（frames 的 id 照常登记，防重复校验不变）。

## 4. 测试当前状态

| 套件 | 结果 |
|---|---|
| `cargo test -p rpi-harness --lib` | ✅ 172 passed |
| `cargo test -p rpi-harness --test frame_progress_recovery` | ✅ 4 passed（**已适配**：`a_cleared_run_salvages_nothing_from_disk` 的断言从「内存查询代表磁盘」改为 ① 直接读会话文件数 `"type":"assistant_frame"` 行 = 4（磁盘全量在）② 内存查询 = 0（retire 语义）；salvage 为空断言不变） |
| `cargo test -p rpi-harness`（全量） | ❌ **234 passed / 1 FAILED**：`harness_run_e2e::a_completed_run_leaves_no_frames_to_salvage`（`crates/rpi-harness/tests/harness_run_e2e.rs:1273`） |

## 5. 待处理：最后一个失败测试

**失败断言**（`harness_run_e2e.rs:1294`）：

```rust
let records = harness.session().find_records(&RecordQuery {
    record_type: Some(ASSISTANT_FRAME_RECORD_TYPE), ..Default::default()
}).await?;
assert!(!records.is_empty(), "frames were recorded while the run was in flight");
```

失败原因：测试在**同进程内**跑完一个 run（`prompt_text` 完成，clear_run 已 apply）后查内存——补丁语义下 run 完结即 retire → `find_records` 返回空 → 断言失败。

**测试意图**：「completed run 不 salvage 出重复内容」（后面的 `salvage_run_frames` 为空断言——这部分仍通过）。

**处理方向（二选一，判断后实施）**：

a. **改断言方式**：前半段意图「frames 落过盘」→ 改为直接读会话文件数 assistant_frame 行数（参考 frame_progress_recovery.rs 已适配的写法）；「run 完结后内存 retire」可顺带加一条正面断言（records 为空 = 新语义）。
b. **删/简化前半断言**：如果该 e2e 测试的会话是 in-memory 存储（`harness_with` 用的是什么 storage 需要确认——若是 InMemory 则没有磁盘文件可读，只能走 a 的变体：把断言改成「salvage 为空」一条 + 注释说明 frames 已 retire）。

注意别破坏测试的原始意图（completed run 不得 salvage 重复内容），修完跑全量 `cargo test -p rpi-harness` 确认 235 全绿。

## 6. 参考数据（接手后复测用）

- 对照基线（补丁前）：原会话恢复 → footprint **234.6M** / 活跃分配 **172.8M** / MallocHelperZone 19.4 万个分配
- 过滤 frames 版：footprint **46.7M** / 活跃分配 **33.0M**
- **预期补丁后**：原会话恢复 → footprint 应降到 **<60M**（接近无 frames 版 + 进行中 run 的少量 frames）
- 实测样本会话：`/Users/zcy/Documents/projects/xiangmucaigou/lcy-ai-procurement-workspace/.rpi/sessions/--Users-zcy-Documents-projects-xiangmucaigou-lcy-ai-procurement-workspace--/2026-10-13T12-08-38-174Z_01a1352a-2a1d-7104-9ace-53b707ba344e.jsonl`（25.4MB，77,393 条 frame；用 `vmmap --summary <pid> | grep footprint` 测）
- 测量注意：`ps` 的 RSS 会含瞬时轮次处理，以 vmmap 的 Physical footprint 为准

## 7. 完成后的验证与提交（按序执行）

1. `cargo test -p rpi-harness` 全绿（235）
2. `cargo test -p rpi-cli --lib` 顺带跑一遍（state 变更可能影响 CLI 层测试）
3. `cargo build --release` → 备份旧二进制 → `cargo install --path crates/rpi-cli --locked`
   - **安装前必须备份**：`cp ~/.cargo/bin/rpi ~/.cargo/bin/rpi-0.3.21-bak-<date>`（回退保险，本机约定）
4. 端到端实测：新 tab 起 `rpi --session-dir <复制的会话目录> --session <id>`，`vmmap --summary` 记录 footprint，对照 §6（预期 <60M）
5. 提交 commit 到 `feat/thinking-config`（建议 message：`fix(harness): retire finished-run frames from memory on clear-run`，正文写清量化数据 + 语义设计），**不 push**（等上游同步节奏，见 §8）
6. 完成后把结果（测试数 + 实测 footprint 对照）回报给交接方（工作区 agent 会从本文件下方「结果回填」节读取）

## 8. 约束与红线（本仓库约定）

- **分支**：改动只在 `feat/thinking-config`（当前分支）；**main 是纯镜像**，永不 push main（push URL 已禁用 `no-push`，pre-push 钩子已拦）
- **上游同步**：作者发新版后按 `.rpi/skills/rpi-fork-sync/SKILL.md` 流程 merge（本次补丁是将来给上游提 PR 的素材，先在 fork 落地）
- **上游 0.3.22 适配提示**：0.3.22 引入了 `AssistantFrameOp::CommitStream`（新变体）和 durable delivery 的恢复逻辑改动（`agent_harness.rs` 会读 records 里的 CommitStream frame 做恢复校验）。**合并 0.3.22 时本补丁的 retain 范围要复核**：CommitStream 绑定的 entryId 校验如果在运行时读内存 records，retire 时机可能要推迟到「entry 确认落盘后」。当前 0.3.21 基线无此问题。
- **bash 3.2 坑**（本机 macOS 默认 bash）：脚本/命令里 `$var` 紧贴非 ASCII（如中文括号）会被解析进变量名 → 全用 `${var}` 括起来（之前踩过两次）
- **不要动**：`scripts/tty7/`（那是采购工作区的，不属于本仓库）；上游 `.gitignore` 的白名单机制（`.rpi/skills/*` 默认 ignore，勿改）

## 9. 结果回填（接手 agent 填写）

```
- 测试全绿时间：2026-10-14 10:20 左右
  - cargo test -p rpi-harness：324 passed / 0 failed（16 个测试二进制全绿；比 §4 的 235 多是因为那次只统计了部分二进制）
  - cargo test -p rpi-cli --lib：472 passed / 0 failed（修了本分支遗留编译错误后，见备注）
- 实测 footprint（补丁后）：42.5M（峰值 63.1M）
  - A/B 同机同会话副本（rpc 模式，vmmap --summary）：
    · 旧二进制（补丁前 0.3.21 备份）：230.3M（峰值 248.8M）——与 §6 基线 234.6M 吻合
    · 新二进制（补丁后）：42.5M（峰值 63.1M）——降 81.5%，优于预期 <60M；峰值 63M 为 load 重放时单个 run 的 frames 流过内存（设计内）
  - 测量样本：会话已增长到 28.6MB / 89,516 行 / 88,290 条 assistant_frame（10-14 上午仍在使用）；末尾 operation_finished 干净收尾，恢复无 salvage 追加，A/B 前均重新复制原文件保证输入一致
- 安装版本/commit：rpi 0.3.21（feat/thinking-config @ 27522ca）；旧二进份已备份 ~/.cargo/bin/rpi-0.3.21-bak-20261014
- 提交：27522ca fix(harness): retire finished-run frames from memory on clear-run（含量化数据与语义设计正文）；db8decb test(cli) 补齐 ModelDefinition 测试构造器的 thinking 字段；均未 push
- 备注：
  1. §5 采取的方案：a 的 InMemory 变体强化版——测试改为慢速流（50 tok/s）+ 轮询验证 run 进行中 frames 在内存可见（≥3 条，非空泛断言），随后断言 run 完结后内存 records 为空（retire 新语义正面断言）+ salvage 为空（原意图不变）；两个方向都能抓回归（改前改后必挂一边）
  2. cargo test -p rpi-cli --lib 原本就编译不过（26f31e9 给 ModelDefinition 加了 thinking_level_map/thinking_budgets 字段，config.rs 两个 #[cfg(test)] 构造器没跟上），与本补丁无关，已单独提交 db8decb 修复
  3. 顺带确认：ClearStream 标记（commit-on-settle）也是 AssistantFrame 记录，ClearRun 的 retain 按同一 run_id 一并清掉，retire 后 find_records 为空成立
  4. 0.3.22 合并时仍需按 §8 复核 CommitStream 的 retain 范围
```
