import { beforeAll, describe, expect, it } from 'vitest';
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import en from '../../locales/en.json';
import zh from '../../locales/zh.json';
import { translateRecoveryMessage } from './recoveryNotice';

beforeAll(async () => {
  await i18n.use(initReactI18next).init({
    lng: 'en',
    fallbackLng: 'en',
    resources: {
      en: { translation: en },
      zh: { translation: zh },
    },
    interpolation: { escapeValue: false },
  });
});

function tFor(lang: 'en' | 'zh') {
  return (key: string, opts?: Record<string, unknown>) =>
    i18n.getFixedT(lang)(key, opts) as string;
}

describe('translateRecoveryMessage i18n', () => {
  it('maps connection retry codes to unreachable in both languages', () => {
    const englishBackend =
      'Failed to connect to remote model (rate limit), retrying in 2s (attempt 2/3)';

    expect(translateRecoveryMessage(englishBackend, tFor('en'), 'model_retry')).toBe(
      'Unable to reach the remote model. Retrying…',
    );
    expect(translateRecoveryMessage(englishBackend, tFor('zh'), 'model_stream_retry')).toBe(
      '无法连接远端模型，正在重试…',
    );
  });

  it('does not require English text when code is present', () => {
    expect(translateRecoveryMessage('anything', tFor('zh'), 'model_retry')).toBe(
      '无法连接远端模型，正在重试…',
    );
  });

  it('translates compacting with percentage', () => {
    expect(
      translateRecoveryMessage(
        'context too long; compacting to 60% before retry',
        tFor('zh'),
        'context_compaction_retry',
      ),
    ).toBe('上下文过长；重试前将压缩至 60%');
  });

  it('uses generic compacting copy when percentage is missing', () => {
    expect(translateRecoveryMessage('compacting', tFor('en'), 'context_compaction_retry')).toBe(
      'Context too long; compacting before retry…',
    );
  });

  it('keeps en/zh recovery key sets aligned', () => {
    const enKeys = Object.keys(en.chat.recovery).sort();
    const zhKeys = Object.keys(zh.chat.recovery).sort();
    expect(zhKeys).toEqual(enKeys);
  });
});
