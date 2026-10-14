# RSI 接入调研结论 — 请 codex 独立验证与批判

> 本文档由 rpi fork 维护会话产出。请你对以下调研结论做**独立验证与深度批判**:
> 你赞同哪些、反对哪些、有没有被遗漏的更深思考或更优方案。
> 你可以自由查阅本仓库代码与联网调研,但结论必须落到本文档末尾的"回复区"。

## 你的行为约束(必须遵守)

- 你(sol 模型)**仅做聚合、总结、推理、分析**——不要自己执行调研、搜索、阅读文件、写入文档这类动作
- 所有**调研、搜索、阅读、写入**类工作必须**派发子 agent 执行,子 agent 使用 luna 模型**(`-c model="gpt-6-luna"`)
- 即:你负责思考和裁决,luna 负责跑腿

## 工作区约束(硬性)

- 你的全部工作以及所有子 agent 的工作,必须锁死在**本仓库目录**内(`/Users/zcy/Documents/projects/rpi-fork/pi-rust`)。**禁止读写其它任何目录**——尤其禁止碰机器上其它项目的工作区,以免污染别的需求
- 派发 luna 子 agent 时必须带上仓库锁定,命令模板:
  ```
  codex exec --cd /Users/zcy/Documents/projects/rpi-fork/pi-rust --sandbox workspace-write -c model="gpt-6-luna" "<子任务:只在本仓库内操作>"
  ```
- 联网调研类子任务也用同一模板,只是把产出写入**本仓库内**的临时文件(如 `tmp-rsi-evidence-*.md`),不许写到仓库外

## 背景

- 本仓库:rpi(pi coding agent 的 Rust 移植 fork),当前 0.3.21,上游 0.3.23
- 问题:能否接入 RSI(Recursive Self-Improvement)框架来增强/进化 harness?

## 调研已覆盖的材料

1. **CosmosMind-ai/RSI-Harness(RSIH)**:Pi coding agent(Node/TS)之上的 Genome 配置层
   - Genome = 可版本化 harness 配置目录,12 组件,字段所有权互斥,补丁合并语义(缺省继承/null 重置/对象递归合并/数组替换)
   - GEE:从 session 历史(支持 Pi/Claude/Codex/Copilot 五种存储)聚合证据生成新 Genome,方案先确认再落盘,产物过 validate 门
   - 两条测试不变量:无 --genome 时与 pi 行为一致;Pi 可配置面 Genome 全覆盖(pi-surface.test.ts)
   - 设计原则:extension 只做 agent 做不到的 3 件事(session 目录编码、有界读、键盘焦点),方法论全在 skill 文字里
2. **Theseus Labs 论文 arXiv:2609.11873**:HCI 指数(软件工程 52.6、工具 Agent 39.9,RSI 杠杆最大的领域)、L1→L5 自主权分级(头部大厂集中 L1-L2)、改进验证四问(可衡量/可保持/可迁移/可回滚)
3. **awesome-rsi 分类**:self-refinement / persistent self-improvement / recursive self-improvement 三层;RSIH 属第二层,L5 无人抵达

## rpi 现状(实测)

- 扩展机制:**Rust cdylib**(rpi-plugin-sdk),不兼容 RSIH 的 TS 扩展
- session 存储:自有 JSONL 格式(kind: header/record/entry, version 4),RSIH 的 reader 不认
- 已有能力:skills 发现(~/.rpi/agent/skills + 项目 .rpi/skills)、扩展注册斜杠命令、--system-prompt/--append-system-prompt、models.json、478 个测试、key 线程 watchdog
- settings.json 仅 7 个 key,配置面比 Pi 小

## 调研结论(待你验证)

### 结论一:不能直接复用 RSIH,但可低成本移植其架构

三大障碍:技术栈(Rust vs TS)、session schema、配置面大小。

### 结论二:rpi 有两个加分项

1. 扩展能力已对齐 Genome 所需核心面(skills/命令/prompt 覆盖/模型配置,12 组件中 6-7 个有落点)
2. 自产武器是 harness 代码本身(测试/watchdog/格式自知),改进循环的"可验证性"强

### 结论三:三条路线

- **A. 旁路借用**(零改造,半天):装 RSIH 用 gee 扫其他 harness 历史,人工搬结论到 rpi。缺点:与 rpi 使用证据脱钩
- **B. 移植 Genome 架构**(推荐,独立分支 feat/rsi-genome,2-4 周):
  1. Genome 目录协议 + 补丁合并,新 crate rpi-genome,启动时 --genome 应用
  2. rpi session reader(有界聚合:工具直方图/高频命令/纠正模式)
  3. validate 门 + 表面守恒测试(settings 可配置项全枚举,漏路由即红)
  - 集成约束:独立 crate + 薄接入点,减少与上游 0.3.23 同步的冲突面
- **C. 完整 RSI 闭环**(L3-L4 方向,数月):回归基准 + 持久记忆 + 独立评估,只定方向不进第一版

### 结论四:分期建议

第一期 Genome 协议+应用层(纯配置,零风险);第二期 session 分析 + validate/表面测试。对应 Theseus 分级约 L1→L2。

---

## 回复区(请 codex 在此写入)

### 总体裁决

**有保留地赞同借鉴 Genome 架构，反对把当前证据升级为“低成本移植已成立”“核心面已对齐”或“第一期零风险”。** 当前 rpi 具备可利用的配置、资源发现和扩展入口，但接入需要适配实际配置优先级、项目信任边界、session 分支语义及行为验证。推荐先做有界的配置演进试点，再决定是否建设完整 Genome 层。

本轮分工：Sol 负责分析与裁决；Luna 负责全部文件阅读、源码定位、公共原文取证及本回复落盘。本地操作限于本仓库，未读取个人 session stores、全局配置或其他项目，未实施代码修改，未运行测试。根 `.codegraph/` 存在，但 shell 命令不可用且无可调用的 CodeGraph MCP，故 Luna 转用 `rg` 与源码静态核对。

证据分三档：下文“已验证”指本轮取得的源码或原文支持；“推断”指由这些证据推导的设计判断；“未验证”不等于错误。外部源码和论文全文部分访问失败，不能据 README 或摘要宣称实现正确、测试通过或论文数字成立。

### 逐条验证

#### 结论一：不能直接复用 RSIH，但可低成本移植其架构

**裁决：前半句有证据支持；“低成本”未证实，“三大障碍”的表述需要修正。**

- 已验证：`Cargo.toml:26-33` 的 workspace 版本为 `0.3.21`；本轮未独立核验上游 `0.3.23`。SDK 明确采用 Rust `cdylib` / C ABI（`crates/rpi-plugin-sdk/src/lib.rs:1-8`）。实际 mixed loader 仍只加载 `cdylib`（`crates/rpi-extensions/src/loader.rs:486-510`），reload 的 `js_paths` 为空（`crates/rpi-cli/src/session.rs:1341-1368`）。`extension_api.rs:1-5` 的 Node/RPC 抽象意图不能当成已实现的 TS 兼容能力。
- 已验证：当前 session writer 写入 `kind:"header", version:4`，decoder 仅接受 v4（`crates/rpi-harness/src/session/jsonl/codec.rs:85-150`）。RSIH README 列出自身、Pi、Claude Code、Codex 和 Copilot stores，未列 rpi；因此没有现成 rpi adapter 的文档证据。由于未成功取得其 reader 源码，本轮不能进一步断言其内部探测逻辑必然如何拒绝 rpi。
- 已验证：RSIH README 标明 Node ≥22.19、Pi 0.84.3。它面向的 Pi 版本线也需要适配，语言与 JSONL 格式并非全部问题。
- 反证：`Settings` 有 **36 个顶层字段**，不是 7 个；`terminal` 等还包含子字段（`crates/rpi-cli/src/settings.rs:82-253`）。字段存在不等于行为实现：`shell_path`、`branch_summary` 注释明确表示当前仅解析；`retry.provider`、`compaction.model_overrides` 也保留而未消费（同文件 `30-49,121-156,322-337`）。应比较已消费的配置语义，不能用字段数量替代兼容性。

更准确的障碍是：运行时 ABI 与资源执行方式、session 语义、配置来源与优先级，以及可验证行为的覆盖。独立 crate 可以集中纯解析/合并逻辑，但模型、工具、prompt、资源和 UI 的应用仍跨越 CLI/运行时，薄接入并不自动成立。**2–4 周只能作为未经拆解的估计，不能作为已验证工期。**

#### 结论二：扩展核心面已对齐，harness 的可验证性强

**裁决：存在可复用入口；“已对齐”和“12 组件中 6–7 个有落点”的强度过高。代码可测与改善可测也须分开。**

已核对 SDK 注册/动作类型与实际消费者：`PluginApi` 支持命令、资源和 provider hooks（`crates/rpi-plugin-sdk/src/lib.rs:1147-1175,1205-1239`）；模型和 active tools 动作由 host 调入 harness（`crates/rpi-cli/src/extensions_actions.rs:365-383`）；扩展 providers 被加入 provider 列表（`session.rs:1732-1749`）；资源发现结果进入 skill/prompt loader（`session.rs:477-531`）。这支持“有接入点”，不支持“整个组件已兼容”。

RSIH README 的 12 组件是 `instructions / tools / skills / commands / model / runtime / policies / integrations / appearance / settings / keybindings / resources`。根据本轮证据，可作如下有限映射：

| 组件面 | rpi 已核验入口 | 尚不能推出的能力 |
| --- | --- | --- |
| instructions | base/append prompt、每 run 的 prompt transform | Genome 所有权、覆盖与重置语义已实现 |
| tools | 默认工具选择、runtime active tools | argument narrowing、generated tools、工具策略全覆盖 |
| skills / commands | 资源发现、skill/prompt loader、命令注册 | inline/file 两类资源与冲突规则全面兼容 |
| model | `models.json`、provider 注入、runtime set model | cycle list、request options 全面兼容 |
| settings / runtime / resources | 多种设置字段、目录发现和加载路径 | 字段均已消费、max turns/isolate 等协议全面支持 |
| policies / integrations / appearance / keybindings | 尚不足以作整体判定 | memory、stdio MCP、TS extensions、主题及绑定协议已对齐 |

“自产武器”的优势是能修改并测试自己的实现，确实有利于可控实验；但 watchdog 只检查 TUI key thread heartbeat，约 5 秒超时后记录诊断，并不恢复线程，也不覆盖 provider/agent/扩展所有挂起（`crates/rpi-cli/src/interactive_tui.rs:454-504,1763-1774,1865-1874`）。不能把它当作改进循环的通用安全网。

“478 个测试”来自历史记录：`REVIEW-ask-user-hang-fixes.md:74` 记载 CLI lib 478 passed，同文件 `78-80` 后续为 479 passed；既不是 workspace 全量，也不是本轮新鲜结果。现有功能测试能帮助防回归，但不能独立证明任务成功率、成本、延迟或跨任务迁移得到改善。

#### 结论三：A / B / C 三条路线

**裁决：路线划分有用；B 应先缩成可验证试点，A 只能产生假设，C 的自主级别与工期应保留为未知。**

- **A：旁路借用。** 从其他 harness 历史发现可迁移习惯有启发价值，但其工具、prompt、任务分布不同，不能直接证明 rpi 的瓶颈或改进收益。“零改造、半天”未实测。若采用，产物应是待验证建议，由用户选择并在 rpi 上评估，不能把外部频次统计当成本地效果证据。
- **B：Genome 移植。** 推荐保留方向，先选少量已消费、语义清楚的配置项，定义来源优先级、字段所有权与有效配置，再决定新 crate 和启动接入。覆盖测试必须同时检查“声明→路由→消费者→行为”，不能仅枚举 settings struct；也应覆盖独立的模型目录、CLI flags、prompt、资源和扩展入口。
- **C：完整闭环。** 回归基准、持久产物与独立评估都是必要方向，但“拥有这些设施”不等于完成 recursive self-improvement。若循环只更新固定 schema 内的 prompt/skills/config，称“持久配置改进”更准确；要宣称递归改进，还需说明改进机制本身如何变化、如何被独立验证。“L3–L4”“数月”本轮无充分依据。

RSIH README 确认 GEE 会选 stores、按工作目录分组，聚合工具直方图、高频 bash、热点文件、重复纠正，展示计划并在确认后落盘。README 还说明部分 Codex/Copilot 读取每文件前 2 MiB、缺 cwd 等会标不完整。它支持“有界且人工确认的候选生成”，**不等于有界采样无偏，也不等于已经测出候选更好。**

#### 结论四：第一期协议+应用层零风险，第二期再加 validate/表面测试，对应 L1→L2

**裁决：反对“零风险”，反对把验证门推迟到应用之后；自主权映射未验证。**

rpi 的 base prompt 优先级是 CLI > trusted project > global > 内置；append 有独立回退逻辑（`crates/rpi-cli/src/session.rs:400-449`），最终组合是 base→append→context→skills（`crates/rpi-harness/src/system_prompt.rs:11-76`）。扩展 prompt transform 在 provider context 路径生效（`crates/rpi-cli/src/provider_hooks.rs:90-125`；`crates/rpi-harness/src/agent_harness.rs:2390-2396`），可改变实际模型输入。模型、provider、工具集合的配置同样能改变行为、成本和工具权限范围。**纯配置表示修改载体，不表示影响为零。**

此外，RSIH README 明示：因 Pi 没有 settings 注入 API，它会在启动时把 managed keys 写入 `~/.rsih/settings.json`，而项目 settings 又优先于 Genome。所谓移植不能忽略这类副作用与覆盖关系。rpi 已有项目信任判断及部分字段 overlay（`crates/rpi-cli/src/settings.rs:387-447`）；Genome 不应绕过这些边界。

建议将 schema/所有权校验、有效配置检查、无 Genome 回归和回滚设计放入第一期，在任何候选应用前具备。RSIH 的 README 不变量也需精确引用：无 `--genome` 时与 Pi 行为相同，**但配置目录变为 `~/.rsih`**；它不是无条件的环境等价，本轮亦未运行其测试。

### 更深的思考或遗漏

1. **来源与结论强度。** RSIH 的 schema/合并/GEE/测试不变量本轮主要获得 README 支持，validator 实际检查范围、测试源码与通过状态未验证。外部 README 来自 `main`，未固定 commit；Atom 报告的最新 SHA 为 `7bf09774bbe205956ec5a46e5f934602224cc437`，不能据此声称已读的 README 与该 SHA 完全一致。正式实现前应固定版本并复核协议与测试。

2. **论文不能替代项目评估。** arXiv `2609.11873v3` 元数据确认题目为 *The Last AI Built by Humans: Toward Genuine Recursive Self-Improvement*；摘要支持 Headroom-Closed Index 和从执行自主、策略自主、经验获取自主、环境适应自主到递归元改进的路线。全文访问失败，故 **52.6/39.9、L1–L5、“四问”、大厂集中 L1–L2，以及 Theseus Labs 归属均未验证**。`awesome-rsi` 的确切来源未取得，其三层分类和“L5 无人抵达”也应降为待核实。“可衡量/可保持/可迁移/可回滚”可作为本项目自行采用的检查清单，但暂不归因于该论文。

3. **session reader 首先是语义恢复器。** v4 不止 header/record/entry，还包含 lane/fact（`crates/rpi-harness/src/session/jsonl/codec.rs:180-229`）。有效分支按 `parentId` 回溯（`session/jsonl/state.rs:363-400`）；context 会处理 deferred assistant、compaction 和 branch summary（`session/jsonl/context.rs:73-145`）。ToolStarted records 与 message tool calls/results 存在不同用途，assistant frames 是进度记录（`session/types.rs:798-816,866-875,975-993`）。先定义统计“所有探索”还是“最终采用分支”，再去重、归因并标注截断/压缩；否则会把废弃分支、重复记录和未进入模型上下文的内容当作同一种证据。

4. **频次不是目标，也不是因果。** 高频工具可能高效，也可能是反复失败；纠正可能来自用户需求变化。候选需绑定具体任务、失败标签、原始证据位置和评价指标。至少同时考虑任务成功率、成本、延迟与权限变化，不以“少调用工具”或“少被纠正”单指标优化。

5. **可回滚和可复现需要有效配置快照。** 回滚目录不能撤销工具已产生的外部副作用。应记录候选/父版本、resolved 配置、资源内容 hash、模型/provider 与 harness 版本；否则相同 Genome 仍可能因 global/project/CLI/资源变化得到不同运行。数组整体替换和 `null` 重置尤其需要行为验证。审查历史时也要限定 workspace、证据范围与输出内容，避免把日志中的凭据或用户数据写进可分发配置。

6. **真正薄接入靠语义边界，不靠 crate 数量。** 优先让解析/合并生成纯数据，再由已有入口应用；不让分析器或配置 loader 获得任意代码修改能力。不兼容或未消费的字段应显式拒绝或报告 unsupported，不能借当前 settings 的“忽略未知字段”伪装为成功支持。

### 修正后的建议

**推荐 B 的小范围试点；暂不承诺完整 12 组件、工期或自主权等级。** A 可作为候选来源，C 留待试点取得效果证据后再评估。本次仅审查与文档落盘，不创建分支、crate 或实现。

1. **阶段 0：定义能力与基线 → verify：** 列清字段类型、默认值、所有权、实际消费者、global/project/CLI/Genome 优先级及信任规则；选代表性 rpi 任务，固定版本和环境，建立成功率/成本/延迟基线与验收阈值。先确认收益目标，避免只因架构漂亮而增加配置层。
2. **阶段 1：小型候选配置包与应用试点 → verify：** 只覆盖少量已实现入口；先完成严格校验、可读 diff、有效配置快照、显式选择和失败回滚，再应用候选。验证无 Genome 行为、缺省/null/对象/数组语义、来源优先级、权限变化及未知字段。优先复用现有配置/资源入口；只有纯协议逻辑确有独立边界时再引入 crate，避免启动时改写用户全局配置。
3. **阶段 2：rpi 证据适配器与人工候选审查 → verify：** 从本地 fixture 验证多分支、重复工具记录、压缩和截断；分析预算有界且报告覆盖范围。把历史分析与候选应用解耦，输出建议、证据与未知项，由人决定接受。
4. **阶段 3：对照评估与晋级 → verify：** 在未参与候选生成的任务上与 baseline 对照，重复运行并报告波动、成本和回归；固定评估器，避免候选同时修改评分规则。达标才晋级，未达标保留拒绝记录并回滚。稳定收益成立后，再讨论扩大组件覆盖、自动化及改进机制本身的演进。

**外部证据入口：**

- RSIH README：<https://raw.githubusercontent.com/CosmosMind-ai/RSI-Harness/main/README.md>；版本线索：<https://github.com/CosmosMind-ai/RSI-Harness/commits/main.atom>。README 声明已核验；协议/reader/validator/测试实现与运行结果未核验。
- 论文元数据/摘要：<https://arxiv.org/abs/2609.11873v3>、<https://export.arxiv.org/api/query?id_list=2609.11873>。全文未取得；数字、分级与归属保留为未知。

