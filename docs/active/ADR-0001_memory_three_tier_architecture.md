---
id: ADR-0001
type: ADR
title: Memory System — Three-Tier Architecture (Core / Recall / Archival)
status: Implemented
author: zniverse
created: 2026-06-24
updated: 2026-06-24
reviewers: []
related: []
supersedes: ~
superseded_by: ~
tags: [memory, architecture, storage, embedding]
---

# ADR-0001: Memory System — Three-Tier Architecture (Core / Recall / Archival)

## Context

agent_core 需要一个记忆系统来支持 AI 代理的长期对话、上下文管理和个性化交互。设计面临以下约束：

1. **本地优先**：不依赖外部向量数据库（如 Pinecone/Weaviate），必须能在单机 SQLite 上运行
2. **语义检索**：需要通过 embedding 实现语义相似度搜索
3. **生命周期管理**：不同重要性的记忆需要不同的衰减策略和持久化策略
4. **上下文注入**：需要在每次 LLM 调用前，将最相关的记忆注入到 prompt 中
5. **可解释性**：用户应该能查看记忆的内容、重要性和检索分数

## Decision

采用 **三层存储架构** 来管理记忆，每层有明确的职责边界和数据模型：

```
┌─────────────────────────────────────────────────────────────┐
│                    MemoryManager (协调层)                    │
│         ┌──────────┐  ┌──────────┐  ┌──────────┐          │
│         │  Core    │  │  Recall  │  │ Archival │          │
│         │  Memory  │  │  Memory  │  │  Memory  │          │
│         └────┬─────┘  └────┬─────┘  └────┬─────┘          │
│              │             │              │                │
│         结构化标签     向量检索 + 语义    长期存储、大容量     │
│         小容量        自动评分/衰减      无自动衰减          │
│         手动管理       自动遗忘/晋升      手动清理           │
└─────────────────────────────────────────────────────────────┘
```

### 1. Core Memory（核心记忆）

- **数据模型**：`MemoryBlock` — 结构化标签键值对，带 `max_chars` 容量限制
- **存储**：SQLite `memory_blocks` 表（id, label, content, max_chars, updated_at）
- **语义**：AI 代理的"人格设定"和"用户偏好摘要"—— 短、精、直接注入 prompt
- **API**：`create` / `append` / `replace` / `get` / `list` / `to_context_string`
- **特点**：
  - 无 embedding，纯文本匹配
  - 容量限制防止 prompt 膨胀
  - 必须手动管理（代码或工具调用）

### 2. Recall Memory（召回记忆）

- **数据模型**：`RecallRecord` — 每条对话轮次，含 embedding、重要性、记忆强度、访问次数
- **存储**：SQLite `recall_memory` 表（id, session_id, role, content, embedding, importance, memory_strength, access_count, last_accessed_at, created_at）
- **语义**：对话历史的完整记录，支持语义搜索和遗忘曲线
- **API**：`store` / `search` / `search_scored` / `bump_strength` / `prune` / `promote_to_archival`
- **特点**：
  - 自动计算 embedding（BAAI/bge-small-en-v1.5，384维）
  - **Salience Scoring**：`score = α·semantic + β·recall(t,S) + γ·importance`
    - `semantic`：余弦相似度
    - `recall(t,S)`：Ebbinghaus 遗忘曲线，`e^(-t/(S·half_life))`
    - `importance`：基于关键词、路径、长度、角色等启发式自动评分
  - **Strength Reinforcement**：每次检索后 `memory_strength` 增加，减缓衰减
  - **主动遗忘**：`prune_cold_memories` 删除低分记忆；`promote_to_archival` 晋升高重要性旧记忆

### 3. Archival Memory（归档记忆）

- **数据模型**：`ArchivalRecord` — 内容 + embedding + 可选 metadata
- **存储**：SQLite `archival_memory` 表（id, content, embedding, metadata, created_at）
- **语义**：从 recall 晋升的高价值长期记忆，或用户手动插入的知识
- **API**：`insert` / `search` / `delete`
- **特点**：
  - 无自动衰减，但检索时仍需相似度排序
  - 容量无硬性限制（SQLite 文件限制）
  - 清理需手动调用 `delete`

## Consequences

### Positive

- **三层分工清晰**：Core 管人格，Recall 管对话，Archival 管长期知识——每层职责单一，不互相污染
- **本地运行**：完全依赖 SQLite + fastembed，零外部服务依赖，适合单机 CLI 工具
- **可解释性**：每条 Recall 记忆都有 `importance` / `memory_strength` / `access_count` 可查看
- **智能遗忘**：Ebbinghaus 遗忘曲线 + 重要性加权 + 强度强化 = 自然的信息衰减，而非定时清理
- **晋升机制**：高重要性旧记忆自动晋升到 Archival，避免 recall 表膨胀同时保留价值

### Negative / Trade-offs

- **全表扫描**：`search` 时加载全表（`LIMIT 1000`）到内存计算相似度，大数据量时性能瓶颈
  - *缓解*：未来可引入 `fts5` 或分层索引
- **SQLite 并发**：使用 `parking_lot::Mutex` 而非 `RwLock`，读操作也串行化
  - *缓解*：WAL mode 已开启，读写不冲突；单个用户场景下足够
- **Embedding 固定**：仅支持 fastembed 的 BAAI 模型，维度硬编码为 384
  - *缓解*：未来可通过 `EmbeddingModel` trait 抽象化
- **晋升单向**：Recall → Archival 是单向的，无法降级回 recall
  - *缓解*：手动 `delete` + `insert` 可模拟降级

## Alternatives Considered

| Alternative | Pros | Cons | Decision |
|-------------|------|------|----------|
| **单一 SQLite 表 + 全量 embedding** | 简单、代码少 | 无法区分"人格设定"和"对话历史"，prompt 注入混乱 | **Rejected** |
| **使用 PostgreSQL + pgvector** | 向量索引、性能更好 | 引入外部依赖，不符合本地优先 | **Rejected** |
| **使用 Redis / Pinecone** | 专用向量存储，高并发 | 完全外部依赖，需要 API key | **Rejected** |
| **两层（Core + 单一 Recall/Archival 合并）** | 更简单 | 长期知识和短期对话混在一起，无法单独管理衰减策略 | **Rejected** |
| **三层（Core / Recall / Archival）** | 职责分离、策略灵活、可扩展 | 代码量增加、三层之间需要协调 | **Accepted** |

## Implementation Notes

- **核心代码位置**：`core/src/memory/` 目录
  - `mod.rs` — `MemoryManager` 协调层
  - `storage.rs` — SQLite 底层封装
  - `core_memory.rs` — Core Memory 实现（即 `block.rs`）
  - `recall.rs` — Recall Memory 实现
  - `archival.rs` — Archival Memory 实现
  - `embedding.rs` — EmbeddingModel + 相似度计算
  - `salience.rs` — SalienceScorer + Ebbinghaus 遗忘曲线 + 重要性评分
  - `consolidation.rs` — 去重合并（当前仅实现 embedding 去重）
- **工具层暴露**：`core/src/tools/` 下的 `core_memory.rs`、`recall_memory.rs`、`archival_memory.rs` 将 Memory API 包装为 MCP 工具
- **配置参数**：`SalienceConfig` 支持 JSON 序列化，未来可通过配置文件调整权重

## References

- [Ebbinghaus Forgetting Curve](https://en.wikipedia.org/wiki/Forgetting_curve)
- [BAAI/bge-small-en-v1.5](https://huggingface.co/BAAI/bge-small-en-v1.5)
- fastembed Rust crate: `https://github.com/Anush008/fastembed-rs`
- 相关代码：`core/src/memory/*.rs`

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-24 | agent_core | Created as Draft |
| 2026-06-24 | zniverse | Reviewed and moved to Implemented |

---
*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-06-24T21:56:00+08:00*
