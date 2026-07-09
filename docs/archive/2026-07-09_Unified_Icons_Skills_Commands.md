# Plan: Unified Icons and Style for Skills & Commands

Date: 2026-07-09

This plan tracks the design and implementation of unifying skill icons (to use `ZapIcon` with violet coloring) and customizing command icons for the autocomplete dropdown.

## Proposed Changes

### Skills Icon Unification
* **`app/src/components/chat/ChatInput.tsx`**: Ensure `@skill` lists render the violet `ZapIcon`.
* **`app/src/components/chat/SkillSelector.tsx`**: Use `ZapIcon` instead of `WandIcon` in input actions and dropdown.
* **`app/src/components/settings/SettingsModal.tsx`**: Replace `WrenchIcon` with `ZapIcon` for settings tabs.
* **`app/src/components/settings/SkillsTab.tsx`**: Use `ZapIcon` with violet coloring for title and grid items.
* **`app/src/components/agents/tabs/AgentConfigTab.tsx`**: Use `ZapIcon` and style skill chips with violet colors.

### Command-Specific Icons
* **`app/src/hooks/useAutocomplete.ts`**: Add separate icon types for commands: `/btw`, `/learn`, `/goal`, `/subagents`, `/clear`, `/help`.
* **`app/src/components/chat/ChatInput.tsx`**: Map command-specific icon types to respective Lucide icons and colors in autocomplete menu.

---

## Tasks

Detailed tasks were tracked locally in `task.md`.
