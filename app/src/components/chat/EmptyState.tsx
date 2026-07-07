import { memo } from 'react';
import Code2Icon from 'lucide-react/dist/esm/icons/code-2.mjs';
import RefreshCwIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';
import CompassIcon from 'lucide-react/dist/esm/icons/compass.mjs';
import SparklesIcon from 'lucide-react/dist/esm/icons/sparkles.mjs';
import { useTranslation } from 'react-i18next';

const PROMPT_SUGGESTIONS = [
  { icon: 'search' as const, key: 'search' },
  { icon: 'refactor' as const, key: 'refactor' },
  { icon: 'explore' as const, key: 'explore' },
  { icon: 'create' as const, key: 'create' },
];

export const EmptyState = memo(function EmptyState({ onSend }: { onSend: (msg: string) => void }) {
  const { t } = useTranslation();

  return (
    <div className="empty-state">
      <div className="empty-state-content">
        <div className="solar-system">
          <div className="sun" />
          <div className="planet-orbit orbit-1">
            <div className="planet planet-1" />
          </div>
          <div className="planet-orbit orbit-2">
            <div className="planet planet-2" />
          </div>
          <div className="planet-orbit orbit-3">
            <div className="planet planet-3" />
          </div>
        </div>

        <h1 className="empty-state-title">{t('chat.emptyState.title')}</h1>
        <p className="empty-state-subtitle">
          {t('chat.emptyState.subtitle')}
        </p>

        <div className="prompt-suggestions">
          {PROMPT_SUGGESTIONS.map((s, i) => {
            const label = t(`chat.emptyState.suggestions.${s.key}`);
            return (
              <button
                key={i}
                className="prompt-card"
                onClick={() => onSend(label)}
              >
                <div className="prompt-card-icon">
                  {s.icon === 'search' && <Code2Icon size={16} />}
                  {s.icon === 'refactor' && <RefreshCwIcon size={16} />}
                  {s.icon === 'explore' && <CompassIcon size={16} />}
                  {s.icon === 'create' && <SparklesIcon size={16} />}
                </div>
                <span className="prompt-card-text">{label}</span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
});

