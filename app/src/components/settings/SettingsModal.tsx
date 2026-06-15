import { useEffect, useCallback } from 'react';
import { useDispatch, useSelector } from 'react-redux';
import { RootState } from '../../store';
import { closeSettings, setActiveTab, fetchConfig } from '../../features/settings/settingsSlice';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import PlugIcon from 'lucide-react/dist/esm/icons/plug.mjs';
import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import GeneralTab from './GeneralTab';
import ProviderTab from './ProviderTab';
import MemoryTab from './MemoryTab';
import McpTab from './McpTab';
import SkillsTab from './SkillsTab';

const TABS = [
  { key: 'general' as const, label: 'General', icon: SettingsIcon },
  { key: 'provider' as const, label: 'Provider', icon: ServerIcon },
  { key: 'memory' as const, label: 'Memory', icon: BrainIcon },
  { key: 'mcp' as const, label: 'MCP', icon: PlugIcon },
  { key: 'skills' as const, label: 'Skills', icon: WrenchIcon },
];

export default function SettingsModal() {
  const dispatch = useDispatch();
  const isOpen = useSelector((state: RootState) => state.settings.isOpen);
  const activeTab = useSelector((state: RootState) => state.settings.activeTab);
  const loading = useSelector((state: RootState) => state.settings.loading);
  const error = useSelector((state: RootState) => state.settings.error);

  useEffect(() => {
    if (isOpen) {
      dispatch(fetchConfig() as any);
    }
  }, [isOpen, dispatch]);

  const handleClose = useCallback(() => {
    dispatch(closeSettings());
  }, [dispatch]);

  const handleBackdropClick = useCallback((e: React.MouseEvent) => {
    if (e.target === e.currentTarget) {
      handleClose();
    }
  }, [handleClose]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      handleClose();
    }
  }, [handleClose]);

  useEffect(() => {
    if (isOpen) {
      document.addEventListener('keydown', handleKeyDown as any);
      return () => document.removeEventListener('keydown', handleKeyDown as any);
    }
  }, [isOpen, handleKeyDown]);

  if (!isOpen) return null;

  return (
    <div className="settings-modal-backdrop" onClick={handleBackdropClick}>
      <div className="settings-modal" role="dialog" aria-modal="true">
        <div className="settings-modal-header">
          <h2 className="settings-modal-title">Settings</h2>
          <button className="settings-modal-close" onClick={handleClose}>
            <XIcon size={18} />
          </button>
        </div>

        <div className="settings-modal-body">
          <nav className="settings-tabs">
            {TABS.map((tab) => {
              const Icon = tab.icon;
              return (
                <button
                  key={tab.key}
                  className={`settings-tab ${activeTab === tab.key ? 'active' : ''}`}
                  onClick={() => dispatch(setActiveTab(tab.key))}
                >
                  <Icon size={14} />
                  {tab.label}
                </button>
              );
            })}
          </nav>

          <div className="settings-tab-panel">
            {loading && (
              <div className="settings-loading">
                <LoaderIcon size={20} className="settings-spinner" />
                <span>Loading configuration...</span>
              </div>
            )}
            {error && (
              <div className="settings-error">
                <XIcon size={16} />
                <span>Failed to load config: {error}</span>
              </div>
            )}
            {!loading && !error && (
              <>
                {activeTab === 'general' && <GeneralTab />}
                {activeTab === 'provider' && <ProviderTab />}
                {activeTab === 'memory' && <MemoryTab />}
                {activeTab === 'mcp' && <McpTab />}
                {activeTab === 'skills' && <SkillsTab />}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
