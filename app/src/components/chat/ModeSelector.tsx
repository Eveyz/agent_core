import { useState, useRef, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import MessageCircleIcon from 'lucide-react/dist/esm/icons/message-circle.mjs';
import ClipboardListIcon from 'lucide-react/dist/esm/icons/clipboard-list.mjs';
import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';

export type AgentMode = 'ask' | 'plan' | 'build';

interface ModeOption {
  mode: AgentMode;
  label: string;
  icon: typeof MessageCircleIcon;
  description: string;
}

const MODES: ModeOption[] = [
  {
    mode: 'ask',
    label: 'Ask',
    icon: MessageCircleIcon,
    description: 'Read-only Q&A',
  },
  {
    mode: 'plan',
    label: 'Plan',
    icon: ClipboardListIcon,
    description: 'Research & plan',
  },
  {
    mode: 'build',
    label: 'Build',
    icon: WrenchIcon,
    description: 'Read, write, execute',
  },
];

export default function ModeSelector() {
  const [mode, setMode] = useState<AgentMode>('build');
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  // Load current mode from backend on mount
  useEffect(() => {
    invoke<string>('get_mode')
      .then((m) => setMode(m as AgentMode))
      .catch(() => setMode('build'));
  }, []);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const handleClick = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [open]);

  const handleSelect = useCallback((newMode: AgentMode) => {
    if (newMode === mode) {
      setOpen(false);
      return;
    }
    invoke('set_mode', { mode: newMode })
      .then(() => {
        setMode(newMode);
        setOpen(false);
      })
      .catch((e) => console.error('Failed to set mode:', e));
  }, [mode]);

  const current = MODES.find((m) => m.mode === mode) ?? MODES[2]; // default to Build
  const IconComponent = current.icon;

  return (
    <div className="mode-selector" ref={containerRef}>
      <button
        className="mode-selector-trigger"
        onClick={() => setOpen(!open)}
        title={`Current mode: ${current.label}`}
      >
        <IconComponent size={14} />
        <span className="mode-selector-label">{current.label}</span>
        <ChevronDownIcon size={10} />
      </button>

      {open && (
        <div className="mode-selector-dropdown">
          {MODES.map((opt) => {
            const OptIcon = opt.icon;
            const isActive = opt.mode === mode;
            return (
              <button
                key={opt.mode}
                className={`mode-selector-option${isActive ? ' active' : ''}`}
                onClick={() => handleSelect(opt.mode)}
              >
                <OptIcon size={14} />
                <div className="mode-selector-option-text">
                  <span className="mode-selector-option-label">{opt.label}</span>
                  <span className="mode-selector-option-desc">{opt.description}</span>
                </div>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
