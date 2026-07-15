# PLAN-0012: Skill Active Context Completeness

```yaml
---
id: PLAN-0012
type: PLAN
title: Skill Active Context Completeness
status: Implemented
author: agent_core
created: 2026-07-13
updated: 2026-07-13
reviewers: [zniverse]
related: [PLAN-0004, PLAN-0006, AI-NOTE-0004]
supersedes: ~
superseded_by: ~
tags: [skills, context, subagent, cache]
---
```

## Objective

When a skill is active (`@skill:` / `skill_load` / auto-trigger), the agent and its subagents must receive:

1. Activated `SKILL.md` body in a dedicated, higher-budget segment
2. Absolute skill directory + asset path index (`templates/`, etc.)
3. Principles that allow `read_file` under active skill dirs
4. Session-active skills inherited by subagents
5. UI skill list aligned with Brain's `SkillManager`

## Implementation summary

| Area | Change |
|------|--------|
| Context | Separate bounded active-skill and compact discovery-catalog segments |
| Discovery | `skill_search` queries metadata when the compact catalog is truncated |
| Assets | `discover_assets` → `### Skill assets` in active / `skill_load` context |
| Resources | `skill_list_resources` + `skill_read_resource` with canonical path containment |
| Dependencies | `requires` resolves dependency-first, rejects missing skills/cycles, and unloads orphaned dependencies |
| Subskills | Discover explicit `<skill>/subskills/<name>/SKILL.md` packages |
| Paths | Run workspace roots include `.agent`, `.agents`, `.claude`, `.codex`, and `skills` |
| Scripts | Canonical path validation, direct argv execution, 600s cap, Build-mode gating |
| Principles / catalog | Active vs inactive copy; prefer contained resource reads |
| `@skill:` | `parse_skill_mentions` + miss notes; steer / follow-up activate |
| Subagent | `resolve_subagent_skills(declared ∪ session actives)` |
| Cache | `get_skills` from Brain; invalidate reloads Brain; draft approve + `skill_reload` clear Redux |

The discovery catalog and activated instructions occupy separate context
segments. Active instructions are ordered before the compact catalog so catalog
growth cannot consume the activated-skill budget. The segment remains bounded
as a defense against oversized or malicious skill packages; the complete body
is also returned by explicit `skill_load`, while references are retrieved on
demand through `skill_read_resource`.

## Related

- Audit: `docs/ai_proposals/2026-07-13_Skill_Execution_Context_Audit.md`
- Cursor plan: `skill_context_fix`
