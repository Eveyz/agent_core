# ⚠️ DEPRECATED — 文档约定已升级

> **Status**: Obsolete | **Superseded by**: [README_PROCESS.md](./README_PROCESS.md)
>
> 本文档为旧版约定，已不再维护。新项目文档请遵循 **Documentation & Decision Lifecycle Process (DDLP)**。

---

## 旧版内容（归档参考）

### 目录结构

- `docs/active_plans/` → 迁移到 `docs/active/`
- `docs/ai_proposals/` → 迁移到 `docs/active/` 或 `docs/archive/`
- `docs/archive/` → 保留，与新流程兼容

### 旧版命名约定

文件名格式：`YYYY-MM-DD_Subject_Or_Title.md`

示例：`2026-06-18_Code_Review.md`

> ⚠️ 新流程使用 `TYPE-NNNN_title.md` 格式，全局编号，状态驱动。

### 迁移说明

| 旧目录 | 旧文件示例 | 新位置建议 | 新类型 |
|--------|-----------|-----------|--------|
| `active_plans/` | `2026-06-20_Roadmap.md` | `docs/active/PLAN-0001_roadmap.md` | PLAN |
| `ai_proposals/` | `2026-06-18_Code_Review.md` | `docs/active/AI-NOTE-0001_code_review.md` | AI-NOTE |
| `archive/` | 任意 | `docs/archive/`（兼容） | 保持原类型 |

---

*Legacy Document | Last Updated: 2026-06-24*
