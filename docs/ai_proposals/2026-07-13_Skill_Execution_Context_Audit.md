# 2026-07-13 — Skill Execution Context Audit

Date: 2026-07-13  
Scope: `@skill:` mention → activation → Segment 6 injection → agent filesystem behavior  
Constraint: read-only review; no code changes

## Verdict

When `@skill:<name>` successfully activates, the agent **does** receive:

1. Absolute **skill folder path** (`Skill directory: …`)
2. Full **`SKILL.md` body** (post-frontmatter)
3. **`scripts/` catalog** (if any), plus registered `skill.<name>.<script>` tools

It does **not** receive:

- `templates/` / other asset listings
- Template or schema file contents
- A resolved absolute path for relative references like `templates/output_schema.json`

So main-run context is **right for instructions + root dir + scripts**, but **incomplete for skill assets**. That gap is enough to explain `find`/`glob` for files under `templates/`.

## End-to-end flow

```
UI @select → insert "@skill:<name> " into textarea
  → send raw string (no path/content from frontend)
  → Run::run(user_input)
  → whitespace parse @skill: → activate_for(session, name)   [flag only]
  → run_turn → refresh_context_segments
  → Segment 6 = catalog + build_active_context_for(session)
  → inject <context_injection> into last user message
```

Key code:

| Step | Location |
|------|----------|
| UI mention | `app/src/components/chat/ChatInput.tsx` (`@skill:${skill.name}`) |
| Activate | `core/src/runtime/run/lifecycle.rs` (~136–155) |
| Pack path + body | `core/src/skills/mod.rs` `build_active_context_for` (~317–357) |
| Inject Segment 6 | `core/src/runtime/run/context.rs` (~201–219) |
| Segment budget | `core/src/context.rs` `loaded_skills` max_tokens = **2000** |
| Manual load | `core/src/tools/skill.rs` `skill_load` → `load_skill_context` (also includes `Skill directory:`) |
| Principles | `core/src/prompt.rs` — prefer `skill_load`; discourage reading skill files with file tools |

Packed active block shape:

```text
## Skill: <name> (v<ver>)
Skill directory: <source_dir>
<SKILL.md body>

### Active Scripts
- skill.<name>.<script>: ...
```

## Answers to the three questions

### 1. Do we tell the agent folder + content on `@skill`?

**Yes — after successful activation**, on the same turn’s first model call via Segment 6.

Frontend only sends the mention string. Backend resolves name → scanned `LoadedSkill.source_dir` + reads `SKILL.md`.

If activation fails (unknown name, scan miss, fragile parse), agent only gets the **catalog** (name/description/triggers, **no paths**) plus the literal `@skill:…` in user text.

### 2. Why `find …\dividend-risk\templates\output_schema.json`?

`dividend-risk`’s body says:

> Return strict JSON matching `templates/output_schema.json`

That is a **relative** reference. Runtime injects skill root, not that file.

Likely model behavior:

1. Sees `Skill directory: C:\Users\…\dividend-risk` and relative `templates/output_schema.json`
2. Principles/catalog say: do **not** read skill files with file tools
3. Model avoids `read_file`, reaches for shell `find`/`glob`
4. Or clumsily searches even when path composition would work

The observed `find -name "<full absolute path>"` is malformed `find` usage ( `-name` expects a basename pattern). That looks like model confusion, not missing root discovery — it already knew the skill tree.

Also: only `scripts/` is discovered (`discover_scripts`). `templates/` is invisible to tooling and context.

### 3. Is skill execution under the right context?

**Partially yes on the main Run path.**

| Aspect | Status |
|--------|--------|
| Same-turn activate → inject before first LLM call | Correct |
| Path + SKILL.md + scripts when active | Correct |
| Template / auxiliary assets | Incomplete |
| Catalog path (inactive skills) | No path |
| Activation failure feedback | Silent |
| Steer mid-run `@skill:` | Not parsed (`inject_next_steer` only adds message) |
| Subagents | Own skill list at construction; do not inherit parent session actives |
| Instruction conflict | Catalog/principles push `skill_load` even when already `[ACTIVE]` |

## Issues ranked by impact

1. **Asset gap (high)** — Relative links in SKILL.md to `templates/` etc. are not resolved or indexed at load time.
2. **Instruction conflict (high)** — “Don’t read skill files with file tools” fights skills that require reading schemas/templates; pushes shell search.
3. **Segment 6 truncation (high)** — 2000-token budget truncates the *entire* catalog + all active skill bodies as one blob. `skill_load` tool results are `Instruction` (hygiene-exempt); Segment 6 is not. **Fix in PLAN-0012:** set `loaded_skills.max_tokens = 0`. (AI-NOTE-0004 still relevant for history.)
4. **Silent / fragile `@skill:` parse (medium)** — Whitespace tokens only; `activate_for` false with no user/agent warning; name must match frontmatter `name:`.
5. **Steer / subagent asymmetry (medium)** — Mentions outside initial `Run::run` user_input don’t activate the same way; subagents don’t inherit parent session actives. **Fix in PLAN-0012 §3–§4.**
6. **Catalog without paths (low–medium)** — Pre-activation discovery has no filesystem anchor. Deferred.
7. **UI ↔ Brain skill cache stale (medium)** — Redux 25s + Tauri 30s independent scan vs Brain boot scan. **Fix in PLAN-0012 §5.**

## Recommendations (no implementation)

Follow-up plan: **PLAN-0012** (see Cursor plan `skill_context_fix` / `docs/active/PLAN-0012_…` when written).

1. On activate, append a **skill asset index** (relative + absolute paths under `templates/`, `references/`, etc.), or resolve markdown-relative links against `source_dir`.
2. Soften principles: allow `read_file` for files under an **active** skill directory; keep “don’t browse to discover SKILL.md” for inactive skills.
3. When `[ACTIVE]`, catalog text should not still say “call skill_load”.
4. Surface failed `@skill:` activations to user/agent.
5. Parse `@skill:` on steer/follow-up the same as initial input.
6. **Segment 6:** set `loaded_skills.max_tokens = 0` so catalog + active skill bodies are not hard-truncated at 2000 tokens (global compact remains the safety net). Do not treat skill instructions as disposable retrieval text.
7. Subagent: union parent **session** actives with declared skills at spawn (inject + script sync).
8. Cache: `get_skills` / invalidate / approve_draft / `skill_reload` keep UI list and Brain `SkillManager` aligned.
9. Optionally frontmatter `assets:` list for critical files to pack or path-resolve at load.

## Related docs

- `docs/active/PLAN-0006_skill_selector_in_input_box.md`
- `docs/active/PLAN-0004_skill_activation_legacy_migration.md`
- `docs/active/AI-NOTE-0004_skill_load_truncation_analysis.md`
