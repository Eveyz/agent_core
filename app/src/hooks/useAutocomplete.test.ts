// @vitest-environment jsdom

import { describe, expect, it } from 'vitest';
import { COMMANDS } from './useAutocomplete';

describe('slash command autocomplete', () => {
  it('offers workflow authoring from the chat input', () => {
    expect(COMMANDS).toContainEqual(expect.objectContaining({
      label: 'workflow',
      value: '/workflow ',
      icon: 'cmd-workflow',
    }));
  });
});
