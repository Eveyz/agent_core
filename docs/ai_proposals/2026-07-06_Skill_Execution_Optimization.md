# Proposal: Skill Execution Path and Tool Invocation Optimization
Date: 2026-07-06

This document analyzes the issue of skill script execution failures due to working directory discrepancies, evaluates the proposed solutions, and recommends an optimal technical plan.

---

## 1. Problem Diagnosis

When a skill containing executable scripts (located under `scripts/`) is loaded:
1. **`SkillScriptTool` works correctly**: It automatically executes the script using the skill's absolute source directory (`self.skill_dir`) as its working directory.
2. **Generic `bash` tool fails with relative paths**: The `SKILL.md` body describes commands using relative paths (e.g., `bash scripts/make.sh`). When the LLM chooses to execute this command via the general `bash` tool, it runs in the project root directory, causing a "File not found" error.
3. **Prompt instructions suppress directory reading**: The current prompt instructs the LLM not to read the skill directory directly, which prevents it from discovering absolute paths on its own.

---

## 2. Evaluation of Proposed Solutions

### Option A: Dynamic Relative Path Rewriting (Recommended)
* **Description**: In `SkillManager::load_content()`, parse the `SKILL.md` body and rewrite any relative references starting with `scripts/` or `./scripts/` to absolute paths pointing to `{skill_dir}/scripts/`.
* **Pros**: 
  - Zero changes to tool schemas or the generic `bash` tool.
  - The LLM receives absolute paths directly in the context, preventing command copying failures.
* **Cons**:
  - If the script internally references other local files relatively (without changing directories first), executing it from the project root via `bash` might still fail.
* **Implementation Detail**: Use a precise regex to replace `scripts/` and `./scripts/` while avoiding double-replacement of already absolute paths or other directories containing the substring `scripts`:
  `(?m)(^|[^a-zA-Z0-9_/\.-])(?:\./)?scripts/`

### Option B: Modify `BashTool` to Support `skill_context`
* **Description**: Add a `working_dir` or `skill_context` parameter to the generic `bash` tool so it can switch directories dynamically.
* **Pros**: Solves any CWD issue for general commands.
* **Cons**: Increases schema complexity of the primary `bash` tool and relies on the LLM correctly passing the parameter.

### Option C: System Prompt Enhancement (Recommended)
* **Description**: Add an instruction to the system principles guiding the LLM to use the dedicated `skill.<name>.<script>` tools instead of raw `bash`.
* **Pros**:
  - Forces the use of the optimal entry point which already correctly configures the CWD.
  - Simplifies LLM reasoning.
* **Cons**: Relying solely on prompt instructions can occasionally fail if the model ignores the instruction.

---

## 3. Recommended Approach: A + C Hybrid

We recommend implementing **A** and **C** together:
1. **Apply Option A** in `core/src/skills/mod.rs` inside `load_content` using regex. This acts as a robust fallback.
2. **Apply Option C** in `core/src/prompt.rs` by adding a clear rule to `DEFAULT_PRINCIPLES`:
   > "When executing a script provided by an active skill, ALWAYS use the specific `skill.<name>.<script>` tool (e.g., `skill.antigravity_guide.make`). Do NOT use the generic `bash` tool to run skill scripts."

This combination ensures both prompt clarity (guiding the LLM to the correct tool) and technical resilience (fallback path correctness).
