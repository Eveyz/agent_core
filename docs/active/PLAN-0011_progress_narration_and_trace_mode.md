---
id: PLAN-0011
type: PLAN
title: Progress Narration + Concise/Verbose Agent Trace Mode
status: In Progress
author: zniverse
created: 2026-07-10
updated: 2026-07-10
reviewers: []
related: []
supersedes: ~
superseded_by: ~
tags: [ux, prompt, frontend, settings, subagent]
---

# PLAN-0011: Tool 前进度旁白 + Concise / Verbose 轨迹模式

## Objective

对标「黑色」agent 的可读时间线：多步任务时模型在 **content 通道**写 1–2 句下一步旁白，UI 在 tool 之间展示；默认 **concise** 不渲染 thinking，可选 **verbose** 显示完整思考过程。Turn 收起时中间旁白与 iteration 全部隐藏，只留最终回答。主 agent 与 subagent 共用同一套行为。

## Background

当前行为（「白色」）：

1. **Prompt**（`core/src/prompt.rs`）明确禁止旁白：`no need to narrate your reasoning in text before acting`。
2. **UI**（`AgentTurn.tsx`）用 `idx > lastIterIdx` **故意不渲染**中间 `assistant` block，只显示最后一轮之后的正文。
3. 用户看到的是 `thinking → tool → thinking → tool → … → final answer`，中间没有面向用户的进度说明；长任务可观测性差。

对比目标行为（「黑色」）：

- Reasoning 进 thinking 通道（可折叠 / 默认可隐藏）。
- Content 通道写短状态（「接下来查 SSE…」），插在 tool 摘要之间。
- 开头可有简短 Thought，但日常不靠展开 CoT 才能跟进度。

技术前提（已具备，不必改事件协议）：

- 流式已区分 `MessageDelta::Thinking` vs `Text`（`utils.ts` → `thinking` / `assistant` blocks）。
- `groupBlocksIntoItems` 已能产出 `thinking → assistant → tools` 顺序。
- Subagent 详情页复用 `AgentTurnUI`（`SubagentDetailPage.tsx`），主对话改对即覆盖详情页。
- Provider 的 `thinking_enabled` 是 **模型 API 是否开 reasoning**，与本计划的 **UI 是否显示 thinking** 正交，禁止混用。

## Decisions (locked)

| # | 决策 | 选择 |
|---|------|------|
| 1 | 旁白强度 | **多步才说**；单次 / 极简单 tool 可跳过 |
| 2 | Turn 收起 | **完全隐藏**中间旁白与 iteration；只留最终回答（+ header / footer） |
| 3 | Subagent | **一起改**（详情页走同一 `AgentTurnUI` + 同一 setting） |
| 4 | 模式 | Setting 可选；**默认 concise**（无 thinking UI）；**verbose** = 显示 thinking（+ 旁白） |
| 5 | Prompt vs UI | Prompt **两种模式共用**（始终鼓励多步短旁白）；setting **只控 UI**，不改 system prompt |
| 6 | Thinking 存储 | **始终存储**（blocks / session metadata / context）；concise 仅不渲染，切换 verbose 可回看 |

## Design

### A. Prompt（两种模式共用）

**文件：** `core/src/prompt.rs`（`DEFAULT_PRINCIPLES` + 遗留 `DEFAULT_REACT_PROMPT`）

替换「禁止 narrate」为多步短旁白规则，约束示例：

- Before calling tools on a **multi-step** task, write **1–2 short sentences** of plain text: what you will do next and why (user-facing, concrete).
- Do **not** dump chain-of-thought into that text; keep CoT in the thinking/reasoning channel.
- Skip narration for trivial single-tool actions (one read, one quick search).
- Keep final answers concise; no greetings / filler / post-hoc summaries of every tool.

保留现有：batch reads、todo / ask_user / subagent 规则、`Be concise`。

**不做：** 按 UI setting 动态改 prompt；不做新 RunEvent / block type。

### B. UI setting：`agentTrace`

| 值 | 默认 | 用户看到的（turn **展开**时） |
|----|------|------------------------------|
| `concise` | ✅ | 中间旁白 + tools；**不渲染** thinking 行 |
| `verbose` | | thinking + 中间旁白 + tools（现有 iteration + 放开的 assistant） |

**Turn 收起（两种模式相同）：** 隐藏全部 iteration 与中间旁白；只渲染 **最后一条** assistant（最终回答）。流式进行中强制按展开逻辑显示。

**存储：** `localStorage`（与 `appearance` 同级），例如 key `agent_core_agent_trace`。  
**不进** `config.toml` / Provider `thinking_enabled`。

**Settings UI：** `GeneralTab` 增加一项，文案建议：

- EN: `Show thinking`（checkbox；勾选 = verbose）
- ZH: `显示思考过程`
- 说明一句：与 Provider 里模型的 Thinking 开关无关，仅影响界面是否展示推理内容。

**内部状态：** `settingsSlice.agentTrace: 'concise' | 'verbose'` + `setAgentTrace`；从 localStorage 初始化。

### C. 主对话 / Subagent 渲染

**文件：**

- `app/src/components/chat/AgentTurn.tsx` — 核心
- `app/src/components/chat/TurnIterationUI.tsx` — concise 时跳过 thinking 块
- `app/src/styles/chat.css`（可选）— 中间旁白轻样式
- Subagent：`SubagentDetailPage` 已复用 `AgentTurnUI`，无独立复制逻辑则 **零改或仅确认**

**渲染规则：**

1. **去掉** `idx > lastIterIdx` 对中间 assistant 的过滤（展开时显示所有 assistant）。
2. **收起时：** assistant 仅当「无后续 iteration 之后的最后一条」或等价：`collapsed && !isProcessing` 时只渲染最终 assistant；iteration 已有 `!collapsed && <TurnIterationUI />`。
3. **Concise：** `TurnIterationUI` / 调用方传入 `showThinking={false}`，不渲染 brain / thinking 正文；tools 与旁白照常。
4. **Verbose：** `showThinking={true}`，保持现有 thinking 折叠行为。
5. **样式（轻量）：** 非最终 assistant 加 class（如 `assistant-msg--progress`）：略淡 / 略小，避免与最终回答抢视觉。最终回答保持现有 `assistant-msg`。

**识别「最终回答」：** 在 `renderItems` 中，最后一条 `type === 'assistant'`，或 `idx > lastIterIdx` 的 assistant（收起逻辑可复用后者）。中间旁白 = 其余 assistant。

### D. 明确不改

- 后端 SSE / `RunEvent` / `MessageDelta` 协议
- Hygiene / thinking 持久化策略（active loop 仍保留 thinking；historical strip 不变）
- Todo 系统（长期状态仍靠 todo；旁白不替代）
- Provider `thinking_enabled` / `reasoning_effort`
- 新 block type（继续用 `assistant` + `thinking`）

## Implementation Phases

### Phase 1 — Prompt + 放开中间旁白 + setting + concise/verbose

1. 改 `core/src/prompt.rs` 旁白规则。
2. `settingsSlice` + `GeneralTab` + `en.json` / `zh.json`。
3. `AgentTurn.tsx`：展开显示中间 assistant；收起只留最终回答；把 `agentTrace` 传给 iteration。
4. `TurnIterationUI.tsx`：`showThinking` 控制 thinking 是否渲染。
5. 可选：`assistant-msg--progress` CSS。
6. 确认 subagent 详情页行为一致（复用路径）。

### Phase 2 — 打磨（可同 PR 或 follow-up）

- 旁白过长时的视觉（仍用 markdown，但 progress class 限制「不像终稿」）。
- Verbose 下非末轮 thinking 默认折叠（现有 `TurnIterationUI` 已接近，核对即可）。
- 若卡片摘要（`SubagentWidgets`）需要一行旁白预览，再单开；**非本计划必须**。

### Phase 3 — 可选质量闸（非阻塞）

- 手测清单固化到 PR description。
- 日后 eval：多步 turn 中带非空 content 的比例（本计划不强制实现）。

## Test Plan

手动（主对话 + 打开一个 subagent 详情）：

- [ ] **Concise + 多步任务：** tool 之间出现短旁白；**无** thinking 行；tools 正常。
- [ ] **Verbose：** 同上，且每轮有可展开 thinking。
- [ ] **单次简单 tool：** 允许无旁白，不强制。
- [ ] **Turn 完成后收起：** 只见「Worked …」header + **最终回答**；展开后旁白/tools（及 verbose 下 thinking）回来。
- [ ] **流式中：** 旁白实时出现；concise 仍不闪 thinking。
- [ ] **Setting 切换：** 刷新后 localStorage 保持；切换立即影响当前/历史 turn 的显示（纯 UI）。
- [ ] **文案：** General 说明不与 Provider Thinking 混淆。

## Risks

| 风险 | 缓解 |
|------|------|
| 模型仍不旁白 → concise 只剩 tools | Prompt 写清「多步」；verbose 可看 thinking；可后续加 eval |
| 旁白变成长 CoT 污染 content | Prompt 限 1–2 句 + progress 样式弱化 |
| Token 略增 | 仅多步短句；可接受 |
| 用户以为关 UI = 关模型 thinking | Settings 说明 + 不碰 `thinking_enabled` |

## Success Criteria

1. 默认 concise 时间线接近黑色 agent：旁白 + tools，无 thinking UI。
2. Verbose 可一键恢复完整思考可见性。
3. 收起 turn 干净：只留最终回答。
4. Subagent 详情与主对话一致。
5. 无后端协议变更；回归现有流式 / 持久化路径。
