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
