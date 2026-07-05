我将审查前端代码 (`app/src/`)，遵循与核心审查相同的 A++ 标准。让我先读取文档流程和前端代码结构。
让我检查一下现有的代码审查报告以作为格式参考，并继续阅读前端源代码。
让我找一下现有的代码审查报告，并继续阅读前端源代码。
让我来查找现有的代码审查报告，并同时继续阅读前端源代码。
我现在对现有 AI-NOTE-0006 报告的格式和标准有了清晰的了解。让我继续并行阅读剩余的前端源代码，以便全面覆盖所有领域。
我已经大致了解了现有的 AI-NOTE-0006 格式和核心代码库模式。前端代码量很大（约 80 多个文件）。让我来制定一个计划，并派生并行子智能体来系统地审查不同的域。
我将启动并行子智能体，系统地审查前端代码的不同领域。
所有四个子智能体均已报告。让我来综合这些发现并撰写报告。
所有四个子智能体均已报告。让我来综合这些发现并撰写报告。
所有四个子智能体均已报告。让我来综合这些发现并撰写报告。
我现在已经从全部四个审查领域获得了全面的数据。让我来编写报告并更新索引。
报告文件已成功写入（47,262 字节）。现在让我完成剩余的步骤。
前端代码审查报告已完成。以下是摘要：

---

## AI-NOTE-0007: A++ 前端代码质量改进路线图

**报告位置**: `docs/active/AI-NOTE-0007_frontend_code_quality_roadmap.md`
**索引已更新**: `docs/index.md`

### 审查范围
`app/src/` 全部前端代码 — 共 104 个文件（`.ts`/`.tsx`），覆盖：
- `App.tsx`（根组件）
- `features/`（chat、agents、project、workflow、settings、review）
- `hooks/`（17 个自定义 Hook）
- `utils/`（mentions、chatUtils、tokens、format、time）
- `components/`（chat、layout、settings、agents、workflow、review、ui）

### 当前评级: **B+ → 目标 A++**

### 改进总览

| 阶段 | 主题 | 改进项数 | 目标评级 |
|------|------|---------|---------|
| Phase 0 | 关键正确性 Bug 修复 | 6 | B+ → A- |
| Phase 1 | 类型安全与 TypeScript 严格化 | 5 | A- → A |
| Phase 2 | React 性能与渲染优化 | 6 | A → A+ |
| Phase 3 | 状态管理与架构优化 | 5 | A+ → A++ |
| Phase 4 | 测试覆盖与质量保证 | 4 | A++ 巩固 |
| Phase 5 | 可维护性与开发体验 | 4 | A++ 巩固 |

### P0 关键问题（部分）

1. **`processSingleEvent` 手动引用交换** — 后台 session 事件处理通过临时交换 `state.entries` 等 6 个引用实现"伪上下文切换"，`try/finally` 中恢复逻辑脆弱，任何中途抛异常都会导致状态混乱
2. **`SteerMessage.status` 类型不完整** — `types.ts` 定义 `'pending' | 'injected'`，但 `eventHandlers.ts:711` 写入 `'injected'`，缺少 `'cancelled' | 'failed'` 状态，`steer_cancelled` 事件直接从数组删除而非标记状态
3. **`SubagentBlock` 宽松类型** — 所有字段 optional，与 `TurnBlock` 的精确 discriminated union 不一致，`sa.blocks` 访问缺少类型保护
4. **`entriesToEventLog` 嵌套层级丢失** — subagent 事件 payload 直接序列化整个 `SubagentEntry`，但嵌套 subagent 的 `blocks` 中 `subagent_ref` 引用无法被反序列化恢复
5. **selector 返回新对象** — `selectActivePendingApproval` 对 subagent approval 构造新对象返回，破坏 memoization，每次 state 变化都触发重渲染
6. **`handleError` 在无活跃 turn 时创建 `turnIndex: 0` 的 turn** — 可能与合法的第 0 turn 冲突

### Phase 1 类型安全重点
- 消除 `as unknown as Record<string, unknown>` 类型断言链
- `SubagentBlock` 改为 discriminated union
- `RunEventPayload` 的 `items` 字段类型断言 `as TodoItem[]` 无验证
- `createAgent` 的 `params as unknown as Record<string, unknown>` 双重断言

### Phase 2 性能重点
- `ChatArea` → `EntryRow` → `AgentTurn` 全量重渲染问题（无虚拟滚动）
- 内联函数/对象 props 导致子组件 memo 失效
- `useSelector` 返回新数组/对象的选择器未 memoize
- `MarkdownContent` 每个 delta 触发完整 re-parse

报告共 30 项改进，每项含问题描述、目标状态、具体方案、影响范围、工作量预估（S/M/L）和验收标准，与 AI-NOTE-0006（core crate）格式完全一致。
