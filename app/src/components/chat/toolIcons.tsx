import React from 'react';
import TerminalIcon from 'lucide-react/dist/esm/icons/terminal.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import ScanTextIcon from 'lucide-react/dist/esm/icons/scan-text.mjs';
import EyeIcon from 'lucide-react/dist/esm/icons/eye.mjs';
import FolderSearchIcon from 'lucide-react/dist/esm/icons/folder-search.mjs';
import FileSearchIcon from 'lucide-react/dist/esm/icons/file-search.mjs';
import GitBranchIcon from 'lucide-react/dist/esm/icons/git-branch.mjs';
import GitCommitIcon from 'lucide-react/dist/esm/icons/git-commit.mjs';
import GitCompareIcon from 'lucide-react/dist/esm/icons/git-compare.mjs';
import GlobeIcon from 'lucide-react/dist/esm/icons/globe.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import UsersIcon from 'lucide-react/dist/esm/icons/users.mjs';
import DatabaseIcon from 'lucide-react/dist/esm/icons/database.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import ReplaceIcon from 'lucide-react/dist/esm/icons/replace.mjs';
import TrashIcon from 'lucide-react/dist/esm/icons/trash.mjs';
import WandIcon from 'lucide-react/dist/esm/icons/wand.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import ListTodoIcon from 'lucide-react/dist/esm/icons/list-todo.mjs';
import CalendarIcon from 'lucide-react/dist/esm/icons/calendar.mjs';
import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';

// ── Per-tool icon mapping ───────────────────────────────────────────
// Each tool gets a distinct icon so the user can tell at a glance what the
// agent is doing. Falls back to WrenchIcon for unknown tools.
const TOOL_ICONS: Record<string, React.ComponentType<{ size?: number; color?: string; className?: string; style?: React.CSSProperties }>> = {
  bash: TerminalIcon,
  edit: PencilIcon,
  sed: ScanTextIcon,
  read_file: EyeIcon,
  write_file: FileIcon,
  glob: FolderSearchIcon,
  glob_search: FolderSearchIcon,
  grep: FileSearchIcon,
  grep_search: FileSearchIcon,
  git_status: GitBranchIcon,
  git_diff: GitCompareIcon,
  git_commit: GitCommitIcon,
  git_log: GitCompareIcon,
  git_show: GitCompareIcon,
  webfetch: GlobeIcon,
  tavily_search: SearchIcon,
  subagent: UsersIcon,
  subagents: UsersIcon,
  invoke_subagent: UsersIcon,
  core_memory_read: DatabaseIcon,
  core_memory_append: PlusIcon,
  core_memory_replace: ReplaceIcon,
  archival_memory_search: SearchIcon,
  archival_memory_insert: PlusIcon,
  archival_memory_delete: TrashIcon,
  conversation_search: SearchIcon,
  conversation_search_date: CalendarIcon,
  skill_load: WandIcon,
  skill_reload: WandIcon,
  skill_deactivate: WandIcon,
  skill_list: BookOpenIcon,
  todo_read: ListTodoIcon,
  todo_write: ListTodoIcon,
  todo_update: ListTodoIcon,
};

export function getToolIcon(name: string): React.ComponentType<{ size?: number; color?: string; className?: string; style?: React.CSSProperties }> {
  return TOOL_ICONS[name] || WrenchIcon;
}
