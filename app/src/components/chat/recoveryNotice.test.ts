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
  it('maps connection retries with attempt and delay', () => {
    const englishBackend =
      'Failed to connect to remote model (rate limit), retrying in 2s (attempt 2/3)';

    expect(translateRecoveryMessage(englishBackend, tFor('en'), 'model_retry')).toBe(
      'Unable to reach the remote model. Retrying 2/3 in 2s…',
    );
    expect(translateRecoveryMessage(englishBackend, tFor('zh'), 'model_stream_retry')).toBe(
      '无法连接远端模型，将在 2 秒后重试（2/3）…',
    );
  });

  it('falls back to generic unreachable when attempt details are missing', () => {
    expect(translateRecoveryMessage('anything', tFor('zh'), 'model_retry')).toBe(
      '无法连接远端模型，正在重试…',
    );
    expect(translateRecoveryMessage('Failed to connect to remote model', tFor('en'))).toBe(
      'Unable to reach the remote model. Retrying…',
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

  it('translates completed proactive compaction with token counts', () => {
    const summary = 'chunked_drop: 299142 → 30429 tokens (model window only)';
    const details = {
      tokens_before: 299142,
      tokens_after: 30429,
      strategy: 'chunked_drop',
    };
    expect(translateRecoveryMessage(summary, tFor('en'), 'context_compacted', details)).toBe(
      'Cleared older turns · 299,142 → 30,429 tokens',
    );
    expect(translateRecoveryMessage(summary, tFor('zh'), 'context_compacted', details)).toBe(
      '已清理旧对话 · 299,142 → 30,429 个 Token',
    );
  });

  it('uses summary copy for llm_summary strategy', () => {
    const details = {
      tokens_before: 100000,
      tokens_after: 20000,
      strategy: 'llm_summary',
    };
    expect(translateRecoveryMessage('', tFor('en'), 'context_compacted', details)).toBe(
      'Summarized older turns · 100,000 → 20,000 tokens',
    );
  });

  it('keeps en/zh recovery key sets aligned', () => {
    const enKeys = Object.keys(en.chat.recovery).sort();
    const zhKeys = Object.keys(zh.chat.recovery).sort();
    expect(zhKeys).toEqual(enKeys);
  });
});
