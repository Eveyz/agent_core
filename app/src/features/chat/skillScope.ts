export interface SkillScopeProjectState {
  activeProjectId?: string | null;
  activeSessionId?: string | null;
  projects?: Array<{ id: string; path: string }>;
  sessions?: Record<string, Array<{ id: string; cwd: string }>>;
}

export interface SkillScope {
  sessionId: string | null;
  workspace: string | null;
  scopeKey: string;
}

/** Match the exact cwd selection used by Run creation. */
export function resolveSkillScope(project?: SkillScopeProjectState): SkillScope {
  const sessionId = project?.activeSessionId ?? null;
  const activeProjectId = project?.activeProjectId ?? null;
  const sessionWorkspace = sessionId
    ? Object.values(project?.sessions ?? {})
        .flat()
        .find((session) => session.id === sessionId)?.cwd ?? null
    : null;
  const projectWorkspace = project?.projects?.find((item) => item.id === activeProjectId)?.path ?? null;
  const workspace = sessionWorkspace ?? (sessionId ? null : projectWorkspace);
  return {
    sessionId,
    workspace,
    scopeKey: sessionWorkspace ?? sessionId ?? projectWorkspace ?? activeProjectId ?? '__global__',
  };
}
