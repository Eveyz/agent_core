import { describe, expect, it } from 'vitest';
import { stripContextStatus } from './chatUtils';

describe('stripContextStatus', () => {
  it('keeps the human answer and drops a well-formed handoff tag', () => {
    expect(stripContextStatus(
      'The script is correct.\n<context_status>{"sufficient":true,"missing":[],"unresolved":[]}</context_status>',
    )).toBe('The script is correct.');
  });

  it('drops the malformed attribute form and a Chinese status label', () => {
    expect(stripContextStatus(
      'Looks good.\n\n上下文状态：\n<context_status={"sufficient": true, "missing": [], "unresolved": []}>',
    )).toBe('Looks good.');
  });
});
