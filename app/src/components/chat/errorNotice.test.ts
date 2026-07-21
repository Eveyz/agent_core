import { beforeAll, describe, expect, it } from 'vitest';
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import en from '../../locales/en.json';
import zh from '../../locales/zh.json';
import { translateErrorMessage } from './errorNotice';

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

describe('translateErrorMessage i18n', () => {
  it('translates provider unavailable with retry seconds', () => {
    const englishBackend =
      'The AI provider is temporarily unavailable after repeated failures. Try again in about 45s.';

    expect(translateErrorMessage(englishBackend, tFor('en'))).toBe(
      'The AI provider is temporarily unavailable after repeated failures. Try again in about 45s.',
    );
    expect(translateErrorMessage(englishBackend, tFor('zh'))).toBe(
      'AI 服务因连续失败暂时不可用，请约 45 秒后再试。',
    );
  });

  it('translates provider unavailable without seconds', () => {
    const englishBackend =
      'The AI provider is temporarily unavailable after repeated failures. Try again in a minute.';

    expect(translateErrorMessage(englishBackend, tFor('zh'))).toBe(
      'AI 服务因连续失败暂时不可用，请稍后再试。',
    );
  });

  it('maps legacy circuit breaker copy', () => {
    expect(
      translateErrorMessage(
        'Circuit breaker open: Circuit breaker is OPEN. Requests are blocked.',
        tFor('zh'),
      ),
    ).toBe('AI 服务因连续失败暂时不可用，请稍后再试。');
  });

  it('passes through unknown errors', () => {
    expect(translateErrorMessage('something else went wrong', tFor('zh'))).toBe(
      'something else went wrong',
    );
  });

  it('keeps en/zh error key sets aligned', () => {
    const enKeys = Object.keys(en.chat.errors).sort();
    const zhKeys = Object.keys(zh.chat.errors).sort();
    expect(zhKeys).toEqual(enKeys);
  });
});
