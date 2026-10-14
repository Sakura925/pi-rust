# docs/ 目录约束

> 本文件是 rpi 的目录级 context file(agent 在此目录树工作时自动加载进 system prompt)。
> 分支模型与同步纪律见 `.rpi/skills/rpi-fork-sync/SKILL.md`(仅本地)。

## 目录职责(先看这里再决定文件放哪)

| 目录/文件 | 职责 | 硬性约束 |
| --- | --- | --- |
| `specs/` | 活跃需求/spec 文档 | 需求完结(实现合入 local_master)后整组归档到 `archive/` |
| `archive/<主题>/` | 已完结过程档案,一主题一组 | 归档后不改内容;发现错误只允许文末追加"勘误"小节 |
| `local-builds.md` | 本地构建晋级流水(append-only) | 每次晋级 local_master 必须追加一节:commit、产物 SHA-256、构建参数、验证记录 |
| `rsi-integration-research.md` | RSI 调研结论(活跃) | 重大修订走"调研 → 外部独立审查 → 收敛"流程,不单人拍板 |
| `promotion/` | **上游作者的推广文章**(benchmark、Show HN 等) | 不是 fork 的构建晋级!勿混淆、勿修改;fork 的晋级流水在 `local-builds.md` |
| `agent/ architecture/ extensions/ remote/ debugging/ performance/ maintaining/ guides/ rpi-tool/` | 上游项目文档 | 随上游 release 同步更新;fork 个人内容不放这里 |

## 生命周期约定

- **活跃期**:需求文档进 `specs/`(命名 `<主题>-<简述>.md`);临时过程文档可暂放仓库根目录
- **完结**:归档到 `archive/<主题>/`,相关的 HANDOFF/REVIEW/裁决原文放同一目录
- **取证大文件**(`.sample`/`.jsonl`/截图等)不进版本库,存 `~/.rpi/logs/evidence-archive/`,档案内写明取证文件路径
- **索引同步**:任何 docs/ 结构变更(新增/归档/移动)必须同步更新 `docs/README.md`

## 命名约定(归档目录内)

- 交接文档 `HANDOFF.md` · 审查交接 `REVIEW.md` · 调研请求+外部裁决 `REQUEST-AND-VERDICT.md`
- fork 个人文档用中文;上游目录文档语言随上游

## 变更纪律

- docs/ 变更走 feat 分支 → `cargo check --workspace --locked` → ff 合入 `local_master` → 推 origin
- `promotion/` 撞名教训:上游新增目录时先查职责再决定 fork 文档放哪,避免再次撞名
