import { memo } from 'react';
import Code2Icon from 'lucide-react/dist/esm/icons/code-2.mjs';
import RefreshCwIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';
import CompassIcon from 'lucide-react/dist/esm/icons/compass.mjs';
import SparklesIcon from 'lucide-react/dist/esm/icons/sparkles.mjs';

const PROMPT_SUGGESTIONS = [
  { icon: 'search', label: 'Search for TODO comments and FIXME notes across the codebase' },
  { icon: 'refactor', label: 'Refactor error handling to use thiserror and anyhow consistently' },
  { icon: 'explore', label: 'Explain how subagents are spawned and how they share the tool registry' },
  { icon: 'create', label: 'Write comprehensive unit tests for the permission module' },
];

export const EmptyState = memo(function EmptyState({ onSend }: { onSend: (msg: string) => void }) {
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

        <h1 className="empty-state-title">What can I help you build?</h1>
        <p className="empty-state-subtitle">
          Spawn subagents, analyze code, and orchestrate complex tasks.
        </p>

        <div className="prompt-suggestions">
          {PROMPT_SUGGESTIONS.map((s, i) => (
            <button
              key={i}
              className="prompt-card"
              onClick={() => onSend(s.label)}
            >
              <div className="prompt-card-icon">
                {s.icon === 'search' && <Code2Icon size={16} />}
                {s.icon === 'refactor' && <RefreshCwIcon size={16} />}
                {s.icon === 'explore' && <CompassIcon size={16} />}
                {s.icon === 'create' && <SparklesIcon size={16} />}
              </div>
              <span className="prompt-card-text">{s.label}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
});
