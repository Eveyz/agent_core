# PLAN-0003: Agent Core 自我进化系统长期演进路线图

```yaml
---
id: PLAN-0003
type: PLAN
title: Agent Core 自我进化系统长期演进路线图
status: Draft
author: zniverse
created: 2026-06-28
updated: 2026-06-28
reviewers: []
related: [PLAN-0002, ADR-0001]
supersedes: ~
superseded_by: ~
tags: [self-evolution, reflector, hermes, long-term, roadmap]
---
```

## Objective

将当前基于简单规则检测的 Reflector 系统演进为具备持续学习、进化优化能力的自我进化系统，对标 Hermes Agent 的 GEPA + DSPy 架构，实现从"模式检测器"到"自我进化系统"的跨越。

## Background

### 现状评估

**当前 Reflector (Phase F):**
- 3 条简单启发式规则 (连续错误、工具循环、权限拒绝)
- 模板化 SKILL 生成
- 无评估框架、无反馈循环、无进化能力
- 基本休眠状态，未接入 Runtime

**目标状态 (Hermes 级别):**
- GEPA (Genetic-Pareto Prompt Evolution) 进化优化
- DSPy 框架驱动的程序化提示优化
- 完整评估体系 (测试、基准、约束守门)
- 持续学习循环：执行 → 反思 → 进化 → 部署
- 三层记忆系统深度集成

### 核心差距

1. **检测深度**: 模式匹配 vs 因果理解
2. **生成机制**: 模板填充 vs 进化搜索
3. **验证体系**: 无 vs 多层守门
4. **反馈循环**: 单次 vs 持续迭代
5. **集成程度**: 休眠 vs 核心能力

## Design Philosophy

### 渐进式演进原则

- **向后兼容**: 每个阶段保持现有功能可用
- **增量价值**: 每个阶段都产生实际可用的改进
- **风险可控**: 新功能默认关闭，需显式启用
- **可观测性**: 每个阶段都有完整的监控和日志

### 技术选型策略

- **Rust 核心**: 保持 Rust 实现的核心逻辑
- **Python 生态**: 利用 DSPy/GEPA 的 Python 生态
- **混合架构**: Rust 处理运行时，Python 处理进化优化
- **桥接设计**: 通过 gRPC/HTTP 实现 Rust-Python 互操作

## Phased Roadmap

### Phase 1: 基础激活 (0-2 周)

**目标**: 激活现有 Reflector，建立基础数据流

#### 1.1 Runtime 集成
- [ ] 实现 PLAN-0002 中的 R1-R4 任务
- [ ] Reflector 适配 EventLog 格式
- [ ] Run 结束后自动触发 Reflector
- [ ] 基础日志和监控

#### 1.2 质量提升
- [ ] 增强 Digester 规则 (增加到 10+ 条)
- [ ] 改进 SKILL 生成质量 (基于错误上下文)
- [ ] 添加去重机制 (避免重复建议)
- [ ] 实现 suggestion 优先级排序

#### 1.3 验证框架
- [ ] 建立 suggestion 效果追踪
- [ ] 实现 A/B 测试基础设施
- [ ] 添加基础指标 (应用率、成功率、用户反馈)

**成功标准**:
- Reflector 在每次 Run 后自动运行
- 每周生成 5-10 个有价值的 suggestion
- 至少 30% 的 suggestion 被应用或获得正面反馈

---

### Phase 2: 智能分析 (2-6 周)

**目标**: 引入 LLM 驱动的深度分析，超越简单模式匹配

#### 2.1 根因分析引擎
- [ ] 设计失败分类体系 (语法错误、逻辑错误、权限错误、资源错误等)
- [ ] 实现 LLM 驱动的根因分析
- [ ] 构建错误模式知识库
- [ ] 实现跨会话的模式识别

#### 2.2 上下文感知生成
- [ ] 基于 tool 调用上下文生成针对性 SKILL
- [ ] 实现参数敏感的建议 (不同参数不同建议)
- [ ] 添加领域特定规则 (编程、数据分析、系统管理等)
- [ ] 实现建议个性化 (基于用户历史)

#### 2.3 评估框架雏形
- [ ] 设计 SKILL 质量评估指标
- [ ] 实现自动化语法检查
- [ ] 添加语义相似度检测 (避免重复)
- [ ] 建立用户反馈收集机制

**技术架构**:
```
Run EventLog → Digester (规则) → Root Cause Analyzer (LLM) → 
Context Generator → Suggestion Engine → Quality Gate → Apply
```

**成功标准**:
- 根因分析准确率 > 70%
- 生成的 SKILL 非模板化内容占比 > 50%
- 用户满意度 > 60%

---

### Phase 3: 进化优化 (6-12 周)

**目标**: 引入候选变体生成和进化搜索，对标 GEPA 核心能力

#### 3.1 候选变体生成
- [ ] 设计变体生成策略 (参数调整、结构重组、逻辑优化)
- [ ] 实现基于 LLM 的变体生成器
- [ ] 建立变体多样性保证机制
- [ ] 实现变体去重和聚类

#### 3.2 评估体系
- [ ] 设计自动化测试框架
- [ ] 实现基准测试集
- [ ] 建立性能指标 (成功率、效率、资源使用)
- [ ] 实现多目标优化 (准确率 vs 简洁性 vs 速度)

#### 3.3 进化算法
- [ ] 实现基础遗传算法 (选择、交叉、变异)
- [ ] 集成帕累托优化
- [ ] 实现迭代收敛检测
- [ ] 添加进化过程可视化

#### 3.4 守门机制
- [ ] 实现测试守门 (所有变体必须通过测试)
- [ ] 添加大小限制 (SKILL ≤ 15KB)
- [ ] 实现语义漂移检测
- [ ] 建立人工审查流程

**技术架构**:
```
Current Skill → Variant Generator (LLM) → 
Candidate Pool → Evaluator (Tests + Benchmarks) → 
Multi-objective Optimizer → Best Variant → 
Quality Gates → PR/Apply
```

**成功标准**:
- 能够生成 5-10 个有意义的候选变体
- 进化算法在 10 代内收敛
- 优化后的 SKILL 性能提升 > 20%

---

### Phase 4: 持续学习 (12-20 周)

**目标**: 建立完整的反馈循环和持续学习机制

#### 4.1 效果追踪系统
- [ ] 实现 SKILL 应用效果追踪
- [ ] 建立成功/失败指标收集
- [ ] 设计长期效果评估
- [ ] 实现因果归因分析

#### 4.2 反馈循环
- [ ] 实现基于效果的自动调整
- [ ] 建立学习率自适应机制
- [ ] 实现遗忘机制 (过期知识清理)
- [ ] 添加异常检测和回滚

#### 4.3 知识图谱
- [ ] 构建 SKILL 依赖关系图
- [ ] 实现知识冲突检测
- [ ] 建立知识版本管理
- [ ] 实现知识推荐引擎

#### 4.4 Memory 深度集成
- [ ] Reflector 与 Memory 系统双向通信
- [ ] 基于 Memory 内容优化建议
- [ ] 实现跨会话模式学习
- [ ] 建立用户画像驱动的个性化

**技术架构**:
```
Execution → Effect Tracking → Memory Update → 
Pattern Mining → Knowledge Graph Update → 
Reflector Optimization → Next Execution
```

**成功标准**:
- 完整的反馈循环运行稳定
- SKILL 应用效果可量化追踪
- 系统性能随时间持续提升

---

### Phase 5: 生态集成 (20-30 周)

**目标**: 与外部工具和平台集成，构建完整生态系统

#### 5.1 DSPy 集成
- [ ] 设计 Rust-DSPy 桥接层
- [ ] 实现 DSPy 程序化提示优化
- [ ] 集成 DSPy 的评估框架
- [ ] 实现 DSPy 模块与 SKILL 的互转

#### 5.2 外部数据源
- [ ] 集成 GitHub Copilot 会话历史
- [ ] 支持 Claude Code 数据导入
- [ ] 实现公开数据集学习
- [ ] 建立数据质量评估

#### 5.3 协作机制
- [ ] 实现 SKILL 分享和发现
- [ ] 建立社区审核机制
- [ ] 实现 SKILL 市场集成
- [ ] 添加贡献者激励系统

#### 5.4 多平台支持
- [ ] 支持跨平台 SKILL 同步
- [ ] 实现云端进化服务
- [ ] 添加移动端支持
- [ ] 建立分布式进化网络

**技术架构**:
```
Local Agent → Cloud Evolution Service → 
Community Knowledge Base → 
Multi-agent Learning → Global Optimization
```

**成功标准**:
- DSPy 集成完成，可进行端到端优化
- 社区 SKILL 共享机制运行
- 跨平台同步稳定可用

---

## Technical Architecture

### 系统架构图

```
┌─────────────────────────────────────────────────────────────┐
│                     Agent Core Runtime                       │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │   Brain      │  │  Memory      │  │   Skills     │       │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘       │
│         │                 │                 │               │
│         └─────────────────┴─────────────────┘               │
│                           │                                 │
│                    EventLog (JSONL)                         │
└───────────────────────────┬───────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                   Reflector System                           │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │  Digester    │→ │ Root Cause   │→ │  Variant     │       │
│  │  (Rules)     │  │  Analyzer    │  │  Generator   │       │
│  └──────────────┘  └──────┬───────┘  └──────┬───────┘       │
│                            │                 │               │
│                            ▼                 ▼               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │  Evaluator   │← │  Evolution   │→ │  Quality     │       │
│  │  (Tests)     │  │  Optimizer   │  │  Gates       │       │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘       │
│         │                 │                 │               │
│         └─────────────────┴─────────────────┘               │
│                           │                                 │
│                    Suggestions                              │
└───────────────────────────┬───────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│              Effect Tracking & Memory                        │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │   Metrics    │→ │  Knowledge   │→ │  Feedback    │       │
│  │  Collector   │  │  Graph       │  │  Loop        │       │
│  └──────────────┘  └──────┬───────┘  └──────┬───────┘       │
│                            │                 │               │
│                            └─────────────────┘               │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│              Python Evolution Layer (Optional)                │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │    DSPy      │  │    GEPA      │  │   External   │       │
│  │  Bridge      │  │  Bridge      │  │   Data       │       │
│  └──────────────┘  └──────────────┘  └──────────────┘       │
└─────────────────────────────────────────────────────────────┘
```

### 核心组件设计

#### 1. Root Cause Analyzer
```rust
pub struct RootCauseAnalyzer {
    client: OpenAIClient,
    knowledge_base: Arc<RwLock<ErrorPatternKB>>,
}

impl RootCauseAnalyzer {
    pub async fn analyze(&self, error: &ToolError, context: &ExecutionContext) 
        -> RootCauseAnalysis {
        // 1. 查询知识库
        // 2. LLM 深度分析
        // 3. 模式匹配
        // 4. 返回根因分类和解释
    }
}
```

#### 2. Variant Generator
```rust
pub struct VariantGenerator {
    strategies: Vec<Box<dyn VariantStrategy>>,
    llm_client: OpenAIClient,
}

impl VariantGenerator {
    pub async fn generate(&self, skill: &Skill, feedback: &Feedback) 
        -> Vec<SkillVariant> {
        // 1. 应用多种生成策略
        // 2. 确保多样性
        // 3. 去重和聚类
        // 4. 返回候选变体
    }
}
```

#### 3. Evolution Optimizer
```rust
pub struct EvolutionOptimizer {
    population_size: usize,
    generations: usize,
    mutation_rate: f64,
    crossover_rate: f64,
}

impl EvolutionOptimizer {
    pub fn optimize(&self, candidates: Vec<SkillVariant>, 
                    evaluator: &dyn Evaluator) -> SkillVariant {
        // 1. 初始化种群
        // 2. 遗传算法迭代
        // 3. 帕累托优化
        // 4. 返回最优解
    }
}
```

#### 4. Effect Tracker
```rust
pub struct EffectTracker {
    metrics_db: Arc<Mutex<MetricsDB>>,
    event_log: Arc<Mutex<EventLog>>,
}

impl EffectTracker {
    pub fn track_application(&self, skill_id: &str, outcome: &Outcome) {
        // 1. 记录应用事件
        // 2. 更新指标
        // 3. 计算因果效应
        // 4. 触发反馈循环
    }
}
```

### 数据流设计

```
1. Execution Phase:
   User Request → Agent Execution → EventLog → Memory Update

2. Reflection Phase:
   EventLog → Digester → Root Cause Analysis → 
   Context Understanding → Suggestion Generation

3. Evolution Phase (Periodic):
   Skills + Feedback → Variant Generation → 
   Evaluation → Evolution → Best Variant → Quality Gates

4. Learning Phase:
   Application Outcomes → Effect Tracking → 
   Knowledge Graph Update → Reflector Optimization
```

## Implementation Strategy

### 开发优先级

**高优先级 (必须做)**:
1. Phase 1: 基础激活 (立即开始)
2. Phase 2.1: 根因分析引擎 (核心能力)
3. Phase 3.2: 评估体系 (质量保证)

**中优先级 (应该做)**:
1. Phase 2.2: 上下文感知生成 (质量提升)
2. Phase 3.1: 候选变体生成 (进化基础)
3. Phase 4.1: 效果追踪系统 (反馈基础)

**低优先级 (可以做)**:
1. Phase 5: 生态集成 (长期规划)
2. Phase 4.3: 知识图谱 (高级特性)
3. Phase 3.3: 进化算法 (可选优化)

### 技术债务管理

**当前技术债务**:
- Reflector 与 EventLog 格式不兼容
- Digester 规则过于简单
- 无测试和验证框架
- 缺乏监控和日志

**偿还计划**:
- Phase 1: 修复格式兼容性
- Phase 2: 增强规则和测试
- Phase 3: 建立完整监控

### 风险缓解

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|----------|
| LLM 成本过高 | 高 | 中 | 实现缓存、批量处理、成本监控 |
| 进化算法不收敛 | 中 | 中 | 设置最大代数、回退机制、人工干预 |
| 生成质量下降 | 高 | 低 | 严格守门、A/B 测试、人工审查 |
| 系统复杂度失控 | 高 | 中 | 模块化设计、渐进式添加、充分测试 |
| 性能影响 | 中 | 中 | 异步处理、资源限制、降级策略 |

## Resource Requirements

### 人力资源

**Phase 1-2 (基础)**:
- 1 名全职 Rust 开发 (8 周)
- 1 名兼职 ML 工程师 (4 周)
- 总计: 12 人周

**Phase 3-4 (核心)**:
- 1 名全职 Rust 开发 (16 周)
- 1 名全职 ML 工程师 (16 周)
- 1 名兼职 QA (8 周)
- 总计: 40 人周

**Phase 5 (生态)**:
- 1 名全职 Rust 开发 (10 周)
- 1 名全职 Python 开发 (10 周)
- 1 名兼职 DevOps (5 周)
- 总计: 25 人周

**总计**: 77 人周 (约 1.5 年 - 2 年，视并行度而定)

### 基础设施

**计算资源**:
- 开发环境: 2 台高性能工作站 (GPU 可选)
- 测试环境: 1 台中等配置服务器
- 生产环境: 按需扩展

**存储资源**:
- EventLog 存储: 100GB 起 (增长型)
- Memory 数据库: 50GB 起
- Knowledge Graph: 20GB 起

**外部服务**:
- LLM API: OpenAI/Azure (成本预估 $500-2000/月)
- 监控服务: Datadog/New Relic (可选)
- CI/CD: GitHub Actions (免费层够用)

### 技术栈

**核心**:
- Rust 1.70+ (主要实现)
- Python 3.10+ (DSPy/GEPA 集成)
- SQLite (Memory 存储)
- Tokio (异步运行时)

**ML/AI**:
- OpenAI API (LLM)
- DSPy (程序化提示)
- GEPA (进化优化，可选)
- FastEmbed (嵌入模型)

**工具链**:
- Cargo (Rust 包管理)
- Poetry (Python 依赖管理)
- pytest (测试)
- GitHub Actions (CI/CD)

## Success Metrics

### Phase 1 (基础激活)
- Reflector 自动运行率: 100%
- Suggestion 生成率: 5-10 个/周
- Suggestion 应用率: >30%
- 系统稳定性: 99.9%

### Phase 2 (智能分析)
- 根因分析准确率: >70%
- SKILL 非模板化率: >50%
- 用户满意度: >60%
- 分析延迟: <10s

### Phase 3 (进化优化)
- 候选变体质量: >60% 可用
- 进化收敛率: >80%
- 性能提升: >20%
- 优化周期: <1h

### Phase 4 (持续学习)
- 反馈循环完整性: 100%
- 长期性能提升: >50% (6 个月)
- 知识图谱覆盖率: >80%
- 个性化效果: >30% 提升

### Phase 5 (生态集成)
- DSPy 集成完成度: 100%
- 社区 SKILL 数量: >100
- 跨平台同步成功率: >95%
- 外部数据源利用率: >50%

## Timeline

### Gantt Chart (概览)

```
Phase 1: ████████████████████ (Weeks 1-2)
Phase 2:         ████████████████████████████████ (Weeks 3-6)
Phase 3:                   ████████████████████████████████████████████ (Weeks 7-12)
Phase 4:                                     ████████████████████████████████████████████████████ (Weeks 13-20)
Phase 5:                                                           ████████████████████████████████████ (Weeks 21-30)
```

### 里程碑

**M1 (Week 2)**: Reflector 基础激活完成
**M2 (Week 6)**: 智能分析引擎上线
**M3 (Week 12)**: 进化优化系统可用
**M4 (Week 20)**: 持续学习循环建立
**M5 (Week 30)**: 生态集成完成

## Open Questions

### 技术决策
1. **是否必须集成 DSPy/GEPA？**
   - 优势: 成熟框架、社区支持
   - 劣势: Python 依赖、复杂度增加
   - 建议: Phase 5 再评估，前期自研简化版本

2. **进化算法的复杂度？**
   - 简单遗传算法 vs 帕累托优化 vs 强化学习
   - 建议: 从简单开始，逐步增加复杂度

3. **LLM 使用策略？**
   - 实时分析 vs 批处理 vs 混合模式
   - 建议: 混合模式，关键路径实时，其他批处理

### 产品决策
1. **用户控制程度？**
   - 完全自动 vs 人工审批 vs 混合模式
   - 建议: 分级控制，敏感操作人工审批

2. **多租户支持？**
   - 单用户 vs 多用户 vs 企业版
   - 建议: 先单用户，架构支持多用户

3. **开源策略？**
   - 完全开源 vs 部分开源 vs 商业版
   - 建议: 核心开源，高级特性商业版

## Next Steps

### 立即行动 (本周)
1. [ ] 审查并批准此计划
2. [ ] 启动 Phase 1.1 (Runtime 集成)
3. [ ] 设置开发环境和基础设施
4. [ ] 建立 Project Board 和追踪机制

### 短期行动 (本月)
1. [ ] 完成 Phase 1 所有任务
2. [ ] 启动 Phase 2.1 (根因分析引擎设计)
3. [ ] 建立 MVP 测试环境
4. [ ] 收集初始用户反馈

### 中期行动 (本季度)
1. [ ] 完成 Phase 2 所有任务
2. [ ] 启动 Phase 3.1 (候选变体生成)
3. [ ] 建立完整的评估框架
4. [ ] 进行中期架构评审

## Conclusion

本路线图提供了一个从当前简单 Reflector 到 Hermes 级别自我进化系统的渐进式演进路径。通过 5 个阶段的系统性建设，我们可以在 30 周内实现：

1. **从模式检测到因果理解**: 引入深度分析和上下文感知
2. **从模板生成到进化优化**: 实现候选变体生成和多目标优化
3. **从单次改进到持续学习**: 建立完整的反馈循环和知识管理
4. **从孤立系统到生态集成**: 连接外部工具和社区知识

关键成功因素是保持渐进式演进、每个阶段都产生实际价值、严格控制风险和复杂度。通过这种方式，我们可以在可控的风险下实现自我进化能力的跨越式提升。

---

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-28 | zniverse | Created as Draft |
