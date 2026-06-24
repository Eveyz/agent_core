---
id: ADR-0001
type: ADR
title: Memory Layer Abstraction — Core, Recall, and Archival Storage
status: Draft
author: agent_core (AI-generated, requires human review)
created: 2026-06-24
updated: 2026-06-24
reviewers: []
related: []
supersedes: ~
superseded_by: ~
tags: [memory, architecture, sqlite, embedding]
---

## Context

`agent_core` 需要一个记忆系统来支持长期对话代理（long-running conversational agent）。该系统的核心需求包括：

1. **结构化可编辑状态**：存储用户画像、任务状态、偏好设置等——这些需要被 AI 精确读写和修改。
2. **对话历史检索**：存储完整的对话轮次，支持语义搜索和基于时间/重要性的检索。
3. **长期知识沉淀**：存储从对话中提取的持久知识、事实、规则——这些需要长期保留，不因时间衰减。
4. **容量管理**：记忆不能无限增长，需要分层淘汰（forgetting）和升级（promotion）机制。
5. **离线优先**：不依赖外部向量数据库（如 Pinecone/Qdrant），需要在本地 SQLite 中运行。

## Decision

采用 **三层存储架构**（Three-Layer Memory Architecture），所有数据统一存储在 SQLite 中，通过 Rust 类型系统严格区分三层：

```
┌─────────────────────────────────────────────┐
│           MemoryManager (协调层)              │
├─────────────┬──────────────┬────────────────┤
│  Core Memory│ Recall Memory│ Archival Memory│
│  (结构化)   │  (对话历史)   │  (知识沉淀)    │
│             │              │                │
│  • 有标签   │  • 完整轮次  │  • 知识片段    │
│  • 可编辑   │  • 语义搜索  │  • 语义搜索    │
│  • 大小限制 │  • 时间衰减  │  • 无衰减      │
│  • 无时序   │  • 重要性评分 │  • 可删除      │
└─────────────┴──────────────┴────────────────┘
              │
              ▼
        ┌─────────────┐
        │  Storage    │
        │  (SQLite)   │
        │             │
        │  • memory_blocks      ← Core    │
        │  • recall_memory      ← Recall  │
        │  • archival_memory    ← Archival│
        │  • conversation_summaries       │
        │  • sessions / session_messages  │
        └─────────────┘
```

### 各层定义

| 层级 | 数据模型 | 存储表 | 检索方式 | 生命周期 | 淘汰机制 |
|------|----------|--------|----------|----------|----------|
| **Core** | `MemoryBlock` (id, label, content, max_chars) | `memory_blocks` | 精确 ID 读取 | 永久，可编辑 | 无 |
| **Recall** | `RecallRecord` (id, session_id, role, content, embedding, importance, memory_strength, access_count, created_at) | `recall_memory` | 语义搜索 + 时间衰减评分 | 会话级，自动衰减 | `prune_cold_memories` / `promote_to_archival` |
| **Archival** | `ArchivalRecord` (id, content, embedding, metadata, created_at) | `archival_memory` | 语义搜索 | 永久，手动删除 | 无（人工删除） |

### 关键设计决策

1. **SQLite 统一存储，而非外部向量数据库**
   - 使用 `fastembed` 在本地生成 embedding
   - 向量以 `BLOB` (f32 LE bytes) 存储在 SQLite 中
   - 检索时全表加载 + 内存计算 cosine similarity
   - 上限 1000 条 recall + 1000 条 archival（后续可扩展为 HNSW 索引）

2. **Salience 评分公式（Ebbinghaus + 语义 + 重要性）**
   ```
   score = α · semantic_similarity + β · recall(t, S) + γ · importance
   
   where:
     recall(t, S) = e^(-t / (S × half_life × importance_factor))
     S = memory_strength (1.0~5.0, grows with access)
     importance_factor = 1 + (modifier - 1) × (importance - 0.5) × 2   [for importance > 0.5]
   ```
   默认参数：`α=0.55`, `β=0.25`, `γ=0.20`, `half_life=168h`, `max_strength=5.0`

3. **自动重要性评分（Auto-Rating）**
   - 无需 LLM 参与，基于启发式规则：
     - 决策关键词（"决定"、"always"、"never"）→ +0.08
     - 文件路径（".rs"、"/"）→ +0.03
     - 长度 > 500 → +0.05
     - 工具短回复（"ok"）→ 减分
   - 用户消息默认比助手消息高 0.1

4. **记忆强度强化（Reinforcement）**
   - 每次检索命中后：`strength = old × 1.05 + 0.15`，上限 5.0
   - 高频访问的记忆衰减更慢

5. **分层流动机制（Memory Flow）**
   ```
   Recall Memory ──[promote_to_archival]──► Archival Memory
        │  (importance >= threshold, 最老的优先)
        │
        └─[prune_cold_memories]──► 删除
           (recall_score < min_score AND importance < min_importance)
   ```
   - 高重要性老记忆 → 升级到 Archival（永久保存）
   - 低重要性冷记忆 → 删除
   - Core Memory 不参与流动（人工管理）

6. **三层各自独立的工具接口**
   - Core: `core_memory_read`, `core_memory_append`, `core_memory_replace`
   - Recall: `conversation_search`, `conversation_search_date`
   - Archival: `archival_memory_insert`, `archival_memory_search`, `archival_memory_delete`

## Consequences

### Positive
- **离线可用**：无需外部服务，单机即可运行
- **语义检索能力**：基于 fastembed + cosine similarity，支持跨语言搜索
- **可控的遗忘**：Ebbinghaus 曲线 + 重要性加权，模拟人类记忆
- **知识升级**：重要对话自动沉淀为知识，避免重复对话
- **成本可控**：SQLite 零运维成本，embedding 本地计算
- **三层隔离**：Core 的可编辑性不会污染对话历史，Archival 的永久性保证知识不丢失

### Negative / Trade-offs
- **向量检索性能**：全表扫描 O(N) 复杂度，当前上限 1000 条。如果记忆量暴增，需要迁移到 HNSW 或 IVF 索引（如 `sqlite-vec` 扩展）
- **Embedding 精度**：fastembed 默认 384 维 BGE-Small，对于复杂语义可能不够精确，可升级但增加内存占用
- **SQLite 并发**：使用 `parking_lot::Mutex` 保护 Connection，单写多读。高并发场景下可能成为瓶颈
- **重要性评分启发式**：规则引擎可能误判，例如讽刺语句中的 "never" 会被误标为高重要性。未来可考虑 LLM 辅助评分，但增加成本和延迟
- **无跨层联合搜索**：Core / Recall / Archival 各自独立搜索，没有统一的 "搜索所有记忆" 接口。需要用户（或上层代理）明确选择搜索哪一层

## Alternatives Considered

| Alternative | Pros | Cons | Decision |
|-------------|------|------|----------|
| **单一向量存储**（所有记忆存为一个表，统一检索） | 简单，统一搜索接口 | 无法区分可编辑状态 vs 对话历史；Core Memory 的 append/replace 语义难以实现 | Rejected |
| **外部向量数据库**（Qdrant/Pinecone） | 高性能 ANN 检索，可扩展 | 引入外部依赖，离线场景失效，增加运维成本 | Rejected |
| **纯文件系统存储**（JSON/文本文件） | 极简，无依赖 | 无语义检索，无结构化查询，性能差 | Rejected |
| **四层架构**（+ Ephemeral Memory 临时缓存） | 更细粒度，临时对话可快速丢弃 | 增加复杂度，当前 Recall 的 session_id 机制已足够区分临时和持久 | Rejected（保留为未来扩展） |
| **PostgreSQL + pgvector** | 高性能，工业级 | 需要安装 PostgreSQL 和 pgvector 扩展，超出本地优先的设计原则 | Rejected |
| **当前方案**（三层 + SQLite + fastembed） | 平衡了性能、功能、运维复杂度 | 向量检索上限约 1000 条/层 | **Accepted** |

## Implementation Notes

### 关键代码位置
- **MemoryManager**: `core/src/memory/mod.rs` — 协调层
- **CoreMemory**: `core/src/memory/block.rs` — 结构化块
- **RecallMemory**: `core/src/memory/recall.rs` — 对话历史 + 搜索
- **ArchivalMemory**: `core/src/memory/archival.rs` — 知识存储
- **Storage**: `core/src/memory/storage.rs` — SQLite 底层
- **SalienceScorer**: `core/src/memory/salience.rs` — 评分系统
- **EmbeddingModel**: `core/src/memory/embedding.rs` — fastembed 封装
- **工具接口**: `core/src/tools/core_memory.rs`, `core/src/tools/recall_memory.rs`, `core/src/tools/archival_memory.rs`

### 数据流示例
```
用户输入 → store_conversation(role="user", content) → Recall Memory
               ↓ (auto-rated importance)
         LLM 生成回复 → store_conversation(role="assistant", content) → Recall Memory
               ↓ (检索时 salience 评分 + 强化 strength)
         用户查询旧知识 → conversation_search(query) → 语义检索 Recall
               ↓ (如果命中高重要性老记忆)
         promote_to_archival() → 升级到 Archival Memory
               ↓ (定期 consolidate)
         dedup_recall_memory() / dedup_archival_memory() → 去重
```

### 配置参数
SalienceConfig 支持通过 `config.toml` 调整：
```toml
[memory.salience]
alpha = 0.55               # 语义权重
beta = 0.25                # 时间衰减权重
gamma = 0.20               # 重要性权重
default_half_life_hours = 168.0  # 1 week
importance_decay_modifier = 3.0   # 高重要性记忆衰减慢 3x
strength_bump_additive = 0.15
strength_bump_multiplicative = 1.05
max_strength = 5.0
recall_floor = 0.01
```

## References

- `core/src/memory/` 目录下全部实现
- `core/src/tools/core_memory.rs` — Core Memory 工具
- `core/src/tools/recall_memory.rs` — Recall Memory 工具
- `core/src/tools/archival_memory.rs` — Archival Memory 工具
- Ebbinghaus forgetting curve: https://en.wikipedia.org/wiki/Forgetting_curve
- fastembed: https://github.com/Anush008/fastembed-rs
- BGE-Small 模型: https://huggingface.co/BAAI/bge-small-en-v1.5

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-06-24 | agent_core | Created as Draft (AI-generated) |

---
*Generated by AI Agent (agent_core)*
*Model: gpt-4o | Timestamp: 2026-06-24T23:30:00+08:00*
