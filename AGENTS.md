# rpi fork 工作约束(agent 会话自动加载)

> 本文件是 rpi 的 context file:cwd 在本仓库内时自动进 system prompt(祖先链机制,见 `crates/rpi-harness/src/context_files.rs`)。
> 子目录不放 AGENTS.md——rpi 只加载 agentDir + cwd→root 祖先链,子目录文件不会被读取。

## 分支纪律(硬性)

- **main**:官方镜像,只从 upstream 快进,永不推送(pre-push 钩子拦截)
- **local_master**:本地构建打包的唯一来源;日用 `~/.cargo/bin/rpi` 只从它的已验证 commit 构建
- **feat/\***:需求/场景开发分支,从 local_master 开出;个人改动一律走 feat 分支,**禁止直接提交到 main/local_master**(提交前 `git branch --show-current` 确认)
- 晋级流程:feat 验证 → 最终组合验证(workspace 全量测试 + 死锁回归 + smoke test + 旧 session 副本兼容)→ ff 合 local_master → 安装入口检查 → 备份旧二进制 → 构建 → 安装 → 推 origin
- 每次晋级在 `docs/local-builds.md` 追加档案:commit、产物 SHA-256、构建参数、验证记录
- 完整流程与推送约束见 `.rpi/skills/rpi-fork-sync/SKILL.md`(仅本地,不进版本库)

## 版本红线

- **版本只升不降**:session JSONL 格式随版本演进,旧二进制打不开新格式(fail-closed);升级后禁止回退旧二进制日常使用
- 安装前必须备份 `~/.cargo/bin/rpi`;macOS 安装必须先 `rm` 再 `cp`(签名缓存,否则 SIGKILL exit 137)

## docs/ 目录约定(摘要,详见 docs/README.md)

- `specs/` 活跃需求/spec;`archive/<主题>/` 已完结过程档案(归档后不改内容,勘误只文末追加)
- `local-builds.md` 晋级流水(append-only)
- `promotion/` 是**上游作者的推广文章**,与 fork 构建晋级无关,勿混淆勿修改
- 上游目录(agent/ architecture/ extensions/ 等)随上游 release 同步,fork 个人内容不放
- 取证大文件(.sample/.jsonl)不进版本库,存 `~/.rpi/logs/evidence-archive/`
- docs/ 结构变更必须同步更新 `docs/README.md` 索引

## 工程习惯

- 测试口径:`cargo test --workspace --locked`(普通 cargo test 只覆盖 default-members);死锁回归跑 `dialog_key_routing_` 过滤(回归时会挂死,注意超时)
- 已知遗留独立工单:provider 流挂起无超时、长会话渲染热点(strip_ansi/str_width)、s002 忙循环会话(proc 55176,`--session 01a13877`)
- 本仓库是 bigfish1913/pi-rust 的 fork(当前基于 0.3.23);上游同步认 tag 不认 main
