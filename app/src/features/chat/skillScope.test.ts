import { describe, expect, it } from 'vitest';
import { resolveSkillScope } from './skillScope';

describe('resolveSkillScope', () => {
  it('prefers the active session cwd over its project path', () => {
    expect(
      resolveSkillScope({
        activeProjectId: 'project-a',
        activeSessionId: 'session-a',
        projects: [{ id: 'project-a', path: '/projects/a' }],
        sessions: { 'project-a': [{ id: 'session-a', cwd: '/sessions/adhoc-a' }] },
      }),
    ).toEqual({
      sessionId: 'session-a',
      workspace: '/sessions/adhoc-a',
      scopeKey: '/sessions/adhoc-a',
    });
  });

  it('lets the backend resolve an unloaded session instead of using the project path', () => {
    expect(
      resolveSkillScope({
        activeProjectId: 'project-a',
        activeSessionId: 'session-a',
        projects: [{ id: 'project-a', path: '/projects/a' }],
        sessions: {},
      }),
    ).toEqual({ sessionId: 'session-a', workspace: null, scopeKey: 'session-a' });
  });

  it('uses the project path when there is no active session', () => {
    expect(
      resolveSkillScope({
        activeProjectId: 'project-a',
        activeSessionId: null,
        projects: [{ id: 'project-a', path: '/projects/a' }],
      }),
    ).toEqual({ sessionId: null, workspace: '/projects/a', scopeKey: '/projects/a' });
  });
});
