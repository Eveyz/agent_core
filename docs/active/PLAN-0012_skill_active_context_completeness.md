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

1. Full `SKILL.md` body (not mid-truncated by Segment 6)
2. Absolute skill directory + asset path index (`templates/`, etc.)
3. Principles that allow `read_file` under active skill dirs
4. Session-active skills inherited by subagents
5. UI skill list aligned with Brain's `SkillManager`

## Implementation summary

| Area | Change |
|------|--------|
| Segment 6 | `loaded_skills.max_tokens = 0` (no hard truncate) |
| Assets | `discover_assets` → `### Skill assets` in active / `skill_load` context |
| Principles / catalog | Active vs inactive copy; allow `read_file` on listed assets |
| `@skill:` | `parse_skill_mentions` + miss notes; steer / follow-up activate |
| Subagent | `resolve_subagent_skills(declared ∪ session actives)` |
| Cache | `get_skills` from Brain; invalidate reloads Brain; draft approve + `skill_reload` clear Redux |

## Related

- Audit: `docs/ai_proposals/2026-07-13_Skill_Execution_Context_Audit.md`
- Cursor plan: `skill_context_fix`
