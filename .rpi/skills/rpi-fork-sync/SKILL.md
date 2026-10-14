---
name: rpi-fork-sync
description: 同步上游 rpi 新版本到本 fork（Sakura925/pi-rust ← bigfish1913/pi-rust）。当上游作者发布新版本/tag，或用户说"同步上游""更新 fork""合并新版本""拉最新 rpi""rpi 发新版了"，或询问/触碰推送规则（能否推 main、推官方仓库）时使用。覆盖 fetch、main 本地基座快进、local_master 稳定分支、候选隔离验证与晋级安装、编译安装、session 兼容性验证、推送约束与降级红线。
---

# rpi fork 同步流程（bigfish1913 → Sakura925）

本仓库是 bigfish1913/pi-rust 的个人 fork。个人修改在 feature 分支维护;`main` 只做上游镜像;`local_master` 是本地构建打包的稳定基线。

## 分支模型（三层）

```
upstream(官方 bigfish1913/pi-rust)
   │ fetch --tags(只拉不推,push URL=no-push)
   ▼
main(基准分支:官方镜像,只快进,永不推送)
   │ 按需 merge main
   ▼
feat/*(需求/场景开发分支:编译+测试)
   │ 验证通过并决定接纳后晋级
   ▼
local_master(本地构建打包源:日常 ~/.cargo/bin/rpi 从这出)
   ▼
~/.cargo/bin/rpi(日用版,版本只升不降)
```

- **main 是纯本地基座**：只从 upstream 快进（`--ff-only`），禁止在 main 上提交个人改动，**禁止推送到任何 remote**（pre-push 钩子拦截，含"把 feature 推成目标 main"的误操作）
- **feat/\* 是开发分支**：从 local_master 开出（继承已接纳的共同基线），编译+测试在这里做；修复不应长期滞留 feature，接纳后及时晋级
- **local_master 是稳定分支**：本地构建打包的唯一来源；每个合入点都必须是"最终组合验证通过"的 commit

## 推送约束（硬性，机器已拦）

1. **main 永不 push 到任何 remote**：pre-push 钩子拦截任何目标为 `refs/heads/main` 的推送；合并方向只有 `main → feat/*`
2. 所有个人改动只在 feature 分支提交；**local_master 和 feat/\* 可推 origin**（自己的仓库 Sakura925/pi-rust）作为备份
3. **upstream（官方 bigfish1913/pi-rust）禁推**：push URL 已置为无效值 `no-push`，任何 `git push upstream ...` 直接报错
4. 推 `origin` 的分支只会进入自己的仓库，官方仓库收不到任何东西；官方采纳个人改动的唯一途径是主动发 PR 且作者合并

## 晋级流程（核心：先验证候选，再晋级安装）

> 教训（2026-10-14 第四现场）：候选修复直接安装成日用版，半成品 bug 直接进入生产工具。
> **真正防止半成品进入日用环境的是安装门槛，不是分支本身。**

### ① 开发与候选验证（随时可做，不覆盖日用版）

- feat/* 上编译后**直接运行独立二进制验证**（`target/release/rpi`），或安装到独立目录；**默认不覆盖 `~/.cargo/bin/rpi`**
- 候选验证用独立配置/session/测试工作区隔离：`RPI_CODING_AGENT_DIR`、`--session-dir` 可提供部分隔离——**候选写过的数据可能让稳定版无法继续使用**（session 格式兼容事故见「红线」），验证 session 一律用副本

### ② 最终组合验证（验证对象必须是最终合并结果）

- 两个 feature 各自通过测试 ≠ 合起来通过。合入 local_master 前：

```
local_master + 待合入 feature
    → 临时候选分支 / git worktree
    → 全量门槛测试(见下)
    → 手动场景验证
    → local_master 快进(--ff-only)到同一个已验证 commit
    → 从该 commit 构建并安装
```

- feature 上验证可以提前发现问题，但不能替代最终组合验证；安装的必须是**同一份已验证产物**——若重新构建，核对来源 commit 一致并做 smoke test

### ③ 合入门槛建议（晋级前逐项确认）

- `cargo test --workspace --locked`：普通 `cargo test` 在此仓库只覆盖 default-members
- 死锁回归：运行 `dialog_key_routing_` 过滤测试（带进程级超时，回归发生时测试会挂住而不是失败），确认实际运行了测试
- release 候选手动验证：ask_user Enter 后正常关闭、下一次输入无残留、模型连通、旧 session 副本可读取（`rpi --export`）
- **记录晋级档案**：commit、产物 SHA-256、构建参数、验证结果——同一版本号下多个本地构建，`rpi --version` 无法区分

### ④ 安装入口检查（防误装）

- 安装前确认：当前分支是 **local_master**、工作树**无未提交改动**（`git status --porcelain` 为空）、HEAD 即已验证 commit
- **禁止**从 feature 分支或带未提交源码的工作树构建安装日用版
- 安装顺序（不可乱）：备份旧二进制 → 编译 → 安装 → `rpi --version` → 旧 session 兼容验证

## 上游同步步骤

### ① 拉上游

```bash
git fetch upstream --tags --prune
git tag --sort=-version:refname | head -3     # 最新 release
git rev-list --count main..vX.Y.Z             # main 落后多少
```

### ② 同步 main（本地基座，不推送）

```bash
git switch main
git merge --ff-only upstream/main             # 或 git merge --ff-only vX.Y.Z
# 不 push：main 永不推送到任何 remote
```

非 ff 时停下来人工判断（main 有分叉），不要强推。

### ③ 合入 feature 分支（按需）

```bash
git switch feat/xxx
git merge-tree --write-tree HEAD main >/dev/null && echo 干净 || echo 有冲突   # 干跑预检(git≥2.38)
git merge main
git push origin feat/xxx
```

- 只取个别修复不整版升级：`git cherry-pick <commit>`
- **语义审查**：上游与本 fork 修改重叠的文件（extensions/session/interactive_tui 等），merge 干净 ≠ 语义正确，逐处确认

### ④ 晋级（走上面的「晋级流程」②~④，不再从 feature 直接构建安装）

```bash
# 1. 先备份旧二进制（回退保险）
cp ~/.cargo/bin/rpi ~/.cargo/bin/rpi-$(rpi --version | awk '{print $2}')-bak
# 2. 最终组合验证通过后,local_master 快进到已验证 commit
# 3. 编译安装（bin 在 crates/rpi-cli）
cargo install --path crates/rpi-cli --locked
# 4. 验证版本号对上
rpi --version
# 5. 用旧 session 副本验证兼容（能导出 = 能解析）
rpi --export <副本目录>/<某旧session>.jsonl </dev/null
```

## 红线（2026-10-13 实际踩坑）

1. **版本只升不降**。session JSONL 格式随版本演进（0.3.22 新增 `assistant_frame.op=commit_stream`，0.3.21 枚举不认识 → 整个 session 打不开，fail-closed）。新构建读旧格式 ✅；旧构建打开"新构建写过的 session" ❌。升级后禁止再切回旧二进制日常使用。
2. **安装新二进制前必须备份** `~/.cargo/bin/rpi`。回退 = 拷回 + 修 session。
3. **main 不放个人改动、永不推送**；日用版只从 local_master 的已验证 commit 构建。
4. **候选验证数据隔离**：候选版写过的 session/配置，稳定版可能读不了——验证一律用副本，验证目录与日用目录分离。

## 环境加固（本地配置，re-clone 后需重装）

```bash
# 1. 禁推官方仓库：upstream 的 push URL 置为无效值（fetch 不受影响）
git remote set-url --push upstream no-push

# 2. main 的上游指向 upstream，强化"只从官方拉取"语义
git branch --set-upstream-to=upstream/main main
```

3. pre-push 钩子（已装，拦截任何目标为 main 的推送）：`.git/hooks/pre-push`，验证方式：

```bash
echo 'refs/heads/main aaa refs/heads/main bbb' | .git/hooks/pre-push; echo $?   # 应非 0
echo 'refs/heads/feat/x aaa refs/heads/feat/x bbb' | .git/hooks/pre-push; echo $?  # 应为 0
echo 'refs/heads/feat/x aaa refs/heads/main bbb' | .git/hooks/pre-push; echo $?    # 应非 0
```

## 降级/损坏恢复（误用旧版本打开新格式 session）

报错特征：`unknown variant 'commit_stream', expected one of 'append', 'clear_stream', 'clear_run'`

1. `cp -p` 备份原文件（保留 mtime）
2. 把 `"op":"commit_stream"` 全部改写为 `"op":"clear_stream"`；每条 commit_stream 的 entry 都紧跟存在时无损（可先脚本校验 entryId 是否都有对应 entry）
3. `touch -r <备份> <原文件>` 还原 mtime；`rpi --export` 验证全部 session 可解析
4. 治本：合入新 tag 重新编译（晋级流程），之后新旧格式都能读
