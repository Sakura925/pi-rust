# rpi 接入 RSI 调研结论(收敛版)

> 日期:2026-10-14 · 状态:调研收敛,未开工
> 产出过程:rpi 会话初版调研 → codex(GPT-6.1-Sol high,luna 子 agent 取证)独立验证批判 → 本文档收敛。
> 双方原文见仓库根 `RSI-REVIEW-REQUEST.md`(含 codex 完整裁决)。

## 一、问题与结论一句话

**问题**:rpi(pi coding agent 的 Rust 移植 fork,0.3.21,上游 0.3.23)能否接入 RSI(Recursive Self-Improvement)框架增强/进化 harness?

**收敛结论**:方向可行且有保留地采纳——**借鉴 RSIH 的 Genome 架构做小范围可验证试点,不承诺完整 12 组件、工期或自主权等级**。先定义能力与基线(阶段 0,纯文档),验证门(校验/快照/回滚)前置到任何候选应用之前。

## 二、调研材料与证据边界

| 材料 | 已核验 | 未核验(不等于错误) |
|---|---|---|
| CosmosMind-ai/RSI-Harness README | 12 组件、补丁合并语义(缺省继承/null 重置/对象递归合并/数组替换)、GEE 流程(scan→聚合→方案确认→落盘→validate)、两条不变量(无 --genome 时与 pi 行为一致但配置目录变 ~/.rsih;Pi 可配面 Genome 全覆盖)均为 README 声明 | validator 实际检查范围、测试源码与通过状态、reader 实现;README 未固定 commit |
| Theseus Labs 论文 arXiv:2609.11873v3 | 题目、摘要:HCI 指标存在、五级自主权路线图存在 | 全文数字(HCI 52.6/39.9)、L1-L5 细节、"四问"、大厂集中 L1-L2 的具体表述 |
| awesome-rsi 分类 | 目录存在 | 三层分类(self-refinement/persistent/recursive)原文 |
| rpi 本仓库源码 | 见下文逐项 | 上游 0.3.23 的差异未独立核验 |

**口径修正**:初版调研误把用户本地 settings.json 实例(7 个 key)当作配置面——实际 `Settings` struct 有 **36 个顶层字段**(`settings.rs:82-253`),但部分仅解析未消费(`shell_path`、`branch_summary`、`retry.provider`、`compaction.model_overrides`)。**字段存在 ≠ 行为实现,比较配置面必须按"已消费语义"。**

## 三、三大障碍(修正版)

1. **运行时 ABI 与资源执行方式**:rpi 只加载 Rust cdylib/mixed loader(`loader.rs:486-510`),reload 的 `js_paths` 为空;RSIH 是 Node≥22.19 + Pi 0.84.3 的 TS 扩展——且面向的 Pi 版本线也与 rpi 上游不同,语言差异不是全部问题
2. **session 语义**:rpi v4 JSONL 不止 header/record/entry,还含 lane/fact;有效分支按 `parentId` 回溯(`session/jsonl/state.rs:363-400`),context 处理 deferred assistant/compaction/branch summary;RSIH 无现成 rpi adapter
3. **配置来源与优先级**:base prompt 为 CLI > trusted project > global > 内置(`session.rs:400-449`),最终组合 base→append→context→skills(`system_prompt.rs:11-76`);扩展 prompt transform 在 provider context 路径生效可改变模型输入;RSIH 因 Pi 无 settings 注入 API 会在启动时写 `~/.rsih/settings.json`——移植不能忽略这类副作用与项目信任边界(`settings.rs:387-447`)

## 四、rpi 已核验的接入点(有限映射)

| Genome 组件面 | rpi 已核验入口 | 尚不能推出的能力 |
|---|---|---|
| instructions | base/append prompt、每 run 的 prompt transform | Genome 所有权/覆盖/重置语义已实现 |
| tools | 默认工具选择、runtime active tools | argument narrowing、generated tools、策略全覆盖 |
| skills / commands | 资源发现、skill/prompt loader、扩展命令注册 | inline/file 两类资源与冲突规则全面兼容 |
| model | models.json、扩展 provider 注入、runtime set model | cycle list、request options 全面兼容 |
| settings / runtime / resources | 多种设置字段、目录发现加载路径 | 字段均已消费、isolate 等协议全面支持 |
| policies / integrations / appearance / keybindings | 不足以整体判定 | memory、stdio MCP、TS 扩展、主题/键位已对齐 |

**关于"自产武器"的修正**:watchdog 只监控 key 线程心跳(5s 超时转储),不恢复线程、不覆盖 provider/agent/扩展挂起——不能当改进循环的通用安全网。现有测试(478/479 为 CLI lib 历史口径,非 workspace 全量)能防回归,不能证明任务成功率/成本/迁移改善。

## 五、收敛后的路线:B 缩成四阶段可验证试点

A(旁路借用 RSIH 的 gee 扫其他 harness 历史)降级为**候选来源**——产出只能是待验证假设,外部频次统计不是本地效果证据。C(完整 RSI 闭环)留待试点取得效果证据后再评估。

- **阶段 0:定义能力与基线**(纯文档,零代码风险):列清字段类型/默认值/所有权/实际消费者/global-project-CLI-Genome 优先级/信任规则;选代表性 rpi 任务,固定版本环境,建立成功率/成本/延迟基线与验收阈值。**先确认收益目标,不因架构漂亮而加配置层**
- **阶段 1:小型候选配置包试点**:只覆盖少量已实现入口;严格 schema/所有权校验、可读 diff、有效配置快照、显式选择、失败回滚,**全部先于应用**;验证无 Genome 回归、缺省/null/对象/数组语义、来源优先级、未知字段显式拒绝。优先复用现有入口,只有纯协议逻辑确有独立边界才引入新 crate;不在启动时改写用户全局配置
- **阶段 2:rpi 证据适配器 + 人工候选审查**:session reader 首先是**语义恢复器**——先定义统计"所有探索"还是"最终采用分支",再处理多分支去重、重复工具记录、压缩截断标注(fixture 驱动验证);分析预算有界并报告覆盖范围;分析与候选应用解耦,人决定接受
- **阶段 3:对照评估与晋级**:在未参与候选生成的任务上对照基线,重复运行报告波动/成本/回归;固定评估器(候选不得同时修改评分规则);达标晋级,未达标留拒绝记录并回滚

## 六、设计原则(双方共识)

1. **证据与结论强度分级**:每条主张标注"已验证/推断/未验证";README 声明不等于实现正确
2. **频次不是因果**:高频工具可能是高效也可能是反复失败;候选必须绑定具体任务、失败标签、原始证据位置和多维指标(成功率/成本/延迟/权限),不单指标优化
3. **可回滚需要有效配置快照**:目录回滚撤销不了工具已产生的外部副作用;记录候选/父版本、resolved 配置、资源内容 hash、模型/provider/harness 版本
4. **薄接入靠语义边界不靠 crate 数量**:解析/合并产出纯数据,由已有入口应用;未消费字段显式拒绝或报 unsupported,不借"忽略未知字段"伪装支持
5. **改进循环的安全边界**:候选配置可能改变模型行为、成本、工具权限——"纯配置"表示修改载体,不表示影响为零
6. **术语诚实**:循环只更新固定 schema 内的 prompt/skills/config 时,称"持久配置改进";宣称"递归改进"需说明改进机制本身如何变化并被独立验证(awesome-rsi 三层中当前目标属 persistent self-improvement 层)

## 七、分支与集成约束

- 新建 `feat/rsi-genome` 分支，**基点选 `local_master`**（稳定分支，当前即 41f8485）——RSI 直接继承已接纳的日用修复，后续有自己的开发线；不合回 main（main 是官方镜像，不放个人改动）
- 晋级纪律：RSI 试验通过基础测试不足以进入日用版——还需收益验收（对照基线的成功率/成本/延迟）和默认行为回归（无 Genome 时行为不变），最终组合验证通过后合入 local_master
- 与上游同步：Genome 层做成独立 crate + 启动时薄接入点，减少与 0.3.23+ 的冲突面
- 与现有遗留解耦：provider 流超时、渲染热点与本调研无耦合，各自独立工单

## 附:外部证据入口

- RSIH README:https://raw.githubusercontent.com/CosmosMind-ai/RSI-Harness/main/README.md(版本线索:commits/main.atom)
- harness-rsi Genome 设计文档:docs/genome/harness-rsi.md(同仓库)
- 论文:arXiv:2609.11873v3《The Last AI Built by Humans: Toward Genuine Recursive Self-Improvement》
