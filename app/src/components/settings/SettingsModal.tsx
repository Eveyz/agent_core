import { useEffect, useCallback, Suspense, lazy } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import { closeSettings, setActiveTab, fetchConfig } from '../../features/settings/settingsSlice';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import PlugIcon from 'lucide-react/dist/esm/icons/plug.mjs';
import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import ShieldIcon from 'lucide-react/dist/esm/icons/shield.mjs';
import GeneralTab from './GeneralTab';
import { useTranslation } from 'react-i18next';

const ProviderTab = lazy(() => import('./ProviderTab'));
const MemoryTab = lazy(() => import('./MemoryTab'));
const McpTab = lazy(() => import('./McpTab'));
const SkillsTab = lazy(() => import('./SkillsTab'));
const PermissionsTab = lazy(() => import('./PermissionsTab'));

const TABS = [
  { key: 'general' as const, icon: SettingsIcon },
  { key: 'provider' as const, icon: ServerIcon },
  { key: 'memory' as const, icon: BrainIcon },
  { key: 'mcp' as const, icon: PlugIcon },
  { key: 'skills' as const, icon: WrenchIcon },
  { key: 'permissions' as const, icon: ShieldIcon },
];

export default function SettingsModal() {
  const { t } = useTranslation();
  const dispatch = useAppDispatch();
  const isOpen = useSelector((state: RootState) => state.settings.isOpen);
  const activeTab = useSelector((state: RootState) => state.settings.activeTab);
  const loading = useSelector((state: RootState) => state.settings.loading);
  const error = useSelector((state: RootState) => state.settings.error);

  useEffect(() => {
    if (isOpen) {
      dispatch(fetchConfig());
    }
  }, [isOpen, dispatch]);

  const handleClose = useCallback(() => {
    dispatch(closeSettings());
  }, [dispatch]);

  const handleBackdropClick = useCallback(
    (e: React.MouseEvent) => {
      if (e.target === e.currentTarget) {
        handleClose();
      }
    },
    [handleClose]
  );

  useEffect(() => {
    if (isOpen) {
      const handler = (e: KeyboardEvent) => {
        if (e.key === 'Escape') handleClose();
      };
      document.addEventListener('keydown', handler);
      return () => document.removeEventListener('keydown', handler);
    }
  }, [isOpen, handleClose]);

  if (!isOpen) return null;

  return (
    <div className="settings-modal-backdrop" onClick={handleBackdropClick}>
      <div className="settings-modal" role="dialog" aria-modal="true">
        <div className="settings-modal-header">
          <h2 className="settings-modal-title">{t('settings.title')}</h2>
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
                  {t(`settings.tabs.${tab.key}`)}
                </button>
              );
            })}
          </nav>

          <div className="settings-tab-panel">
            {loading && (
              <div className="settings-loading">
                <LoaderIcon size={20} className="settings-spinner" />
                <span>{t('settings.loading')}</span>
              </div>
            )}
            {error && (
              <div className="settings-error">
                <XIcon size={16} />
                <span>{t('settings.error', { error })}</span>
              </div>
            )}
            {!loading && !error && (
              <Suspense
                fallback={
                  <div className="settings-loading">
                    <LoaderIcon size={20} className="settings-spinner" />
                  </div>
                }
              >
                {activeTab === 'general' && <GeneralTab />}
                {activeTab === 'provider' && <ProviderTab />}
                {activeTab === 'memory' && <MemoryTab />}
                {activeTab === 'mcp' && <McpTab />}
                {activeTab === 'skills' && <SkillsTab />}
                {activeTab === 'permissions' && <PermissionsTab />}
              </Suspense>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

