---
name: rpi-fork-sync
description: 同步上游 rpi 新版本到本 fork（Sakura925/pi-rust ← bigfish1913/pi-rust）。当上游作者发布新版本/tag，或用户说"同步上游""更新 fork""合并新版本""拉最新 rpi""rpi 发新版了"，或询问/触碰推送规则（能否推 main、推官方仓库）时使用。覆盖 fetch、main 本地基座快进、feature 分支合并、编译安装、session 兼容性验证、推送约束（main 禁推/upstream 禁推/只推自己分支到自己仓库）与降级红线。
---

# rpi fork 同步流程（bigfish1913 → Sakura925）

本仓库是 bigfish1913/pi-rust 的个人 fork。个人修改在 feature 分支维护并自行编译安装使用；`main` 只做上游镜像。

## 仓库约定（每次先确认）

```bash
git remote -v        # origin=Sakura925/pi-rust(自己的fork)  upstream=bigfish1913/pi-rust(作者)
git branch -vv       # 当前 feature 分支
grep '^version' Cargo.toml | head -1
```

- **main 是纯本地基座**：只从 upstream 拉取（`--ff-only` 快进），禁止在 main 上提交个人改动，**且永不 push**（不推 origin、更不推 upstream）
- 个人修改全部走 feature 分支（当前为 `feat/thinking-config`）
- release 以 **tag**（`vX.Y.Z`）为准；tag 不一定已合入 upstream/main（实例：v0.3.22 已发布时 upstream/main 仍落后 1 个 commit）。同步目标认 tag，别只盯 main

## 推送约束（硬性，机器已拦）

1. **main 永不 push 到任何 remote**：仅作为拉取 upstream 的本地基座；合并方向只有一条 `main → feature 分支`
2. 所有个人改动只在 feature 分支提交，push 只推 `origin`（自己的仓库 Sakura925/pi-rust）
3. **upstream（官方 bigfish1913/pi-rust）禁推**：push URL 已置为无效值 `no-push`，任何 `git push upstream ...` 直接报错（且无协作者权限时 GitHub 本身也会拒绝）
4. 推 `origin` 的分支只会进入自己的仓库，官方仓库收不到任何东西；官方采纳个人改动的唯一途径是主动发 PR 且作者合并。注意：若自己的 fork 为 public，分支内容对外可见（作者也能看到），但"可见"≠"推进官方仓库"

## 同步步骤

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
# 不 push：main 永不推送到任何 remote（推送约束 #1）；tag 也无需推 origin
```

非 ff 时停下来人工判断（main 有分叉），不要强推。

### ③ 合入 feature 分支

```bash
git switch feat/thinking-config
git merge-tree --write-tree HEAD main >/dev/null && echo 干净 || echo 有冲突   # 干跑预检(git≥2.38)
git merge main                                # 基座内容合入自己的分支；也可直接 merge vX.Y.Z
git push origin feat/thinking-config          # 唯一允许的推送：自己的分支 → 自己的 fork
```

- rebase 替代：`git rebase vX.Y.Z` + `push --force-with-lease`，历史更干净。注意 rebase **不会丢个人修改**（把 commit 原样搬到新版本之上，编译产物仍是自己的 fork）
- 只取个别修复不整版升级：`git cherry-pick <commit>`

### ④ 编译安装 + 验证（顺序不可乱）

```bash
# 1. 先备份旧二进制（回退保险）
cp ~/.cargo/bin/rpi ~/.cargo/bin/rpi-$(rpi --version | awk '{print $2}')-bak
# 2. 编译安装（bin 在 crates/rpi-cli）
cargo install --path crates/rpi-cli --locked
# 3. 验证版本号对上
rpi --version
# 4. 用旧 session 验证兼容（能导出 = 能解析）
rpi --export <项目>/.rpi/sessions/<某旧session>.jsonl </dev/null
```

## 红线（2026-10-13 实际踩坑）

1. **版本只升不降**。session JSONL 格式随版本演进（0.3.22 新增 `assistant_frame.op=commit_stream`，0.3.21 枚举不认识 → 整个 session 打不开，fail-closed）。新构建读旧格式 ✅；旧构建打开"新构建写过的 session" ❌。升级后禁止再切回旧二进制日常使用。
2. **安装新二进制前必须备份** `~/.cargo/bin/rpi`（④第 1 步）。回退 = 拷回 + 修 session。
3. **main 不放个人改动、永不推送**，推送纪律见「推送约束」。

## 环境加固（本地配置，re-clone 后需重装）

```bash
# 1. 禁推官方仓库：upstream 的 push URL 置为无效值（fetch 不受影响）
git remote set-url --push upstream no-push

# 2. main 的上游指向 upstream，强化"只从官方拉取"语义（顺带让裸 git push 在 main 上直接报错）
git branch --set-upstream-to=upstream/main main
```

3. pre-push 钩子拦截 main 推送（对任何 remote 生效）：创建 `.git/hooks/pre-push` 并 `chmod +x`：

```bash
#!/bin/sh
# main 仅作 upstream 拉取基座，禁止推送（约束见 .rpi/skills/rpi-fork-sync/SKILL.md）
while read local_ref local_sha remote_ref remote_sha; do
  case "$remote_ref" in
    refs/heads/main)
      echo "✋ 禁止推送 main：main 仅作为 upstream 拉取基座，不推送到任何 remote" >&2
      exit 1
      ;;
  esac
done
exit 0
```

验证钩子：`echo 'refs/heads/main aaa refs/heads/main bbb' | .git/hooks/pre-push; echo $?` 应输出拦截信息且非 0；换成 `refs/heads/feat/x` 应静默通过（0）。

## 降级/损坏恢复（误用旧版本打开新格式 session）

报错特征：`unknown variant 'commit_stream', expected one of 'append', 'clear_stream', 'clear_run'`

1. `cp -p` 备份原文件（保留 mtime）
2. 把 `"op":"commit_stream"` 全部改写为 `"op":"clear_stream"`；每条 commit_stream 的 entry 都紧跟存在时无损（可先脚本校验 entryId 是否都有对应 entry）
3. `touch -r <备份> <原文件>` 还原 mtime；`rpi --export` 验证全部 session 可解析
4. 治本：合入新 tag 重新编译（④），之后新旧格式都能读
