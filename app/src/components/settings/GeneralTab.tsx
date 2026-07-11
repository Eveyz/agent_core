import { useSelector, useDispatch } from 'react-redux';
import { RootState } from '../../store';
import InfoIcon from 'lucide-react/dist/esm/icons/info.mjs';
import { setAppearance, setAgentTrace } from '../../features/settings/settingsSlice';
import { useTranslation } from 'react-i18next';

export default function GeneralTab() {
  const { t, i18n } = useTranslation();
  const config = useSelector((state: RootState) => state.settings.config);
  const appearance = useSelector((state: RootState) => state.settings.appearance);
  const agentTrace = useSelector((state: RootState) => state.settings.agentTrace);
  const dispatch = useDispatch();

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">{t('settings.general.noConfig')}</div>
      </div>
    );
  }

  const handleLanguageChange = (lang: string) => {
    i18n.changeLanguage(lang);
    localStorage.setItem('agent_core_language', lang);
  };

  const totalModels = Object.values(config.providers).reduce(
    (sum, p) => sum + Object.keys(p.models).length,
    0
  );

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">{t('settings.general.application')}</h3>
        
        <div className="settings-field">
          <label className="settings-label">{t('settings.general.appearance')}</label>
          <div className="settings-value">
            <select
              className="settings-input"
              style={{ width: '150px' }}
              value={appearance}
              onChange={(e) => dispatch(setAppearance(e.target.value as 'system' | 'dark' | 'light'))}
            >
              <option value="system">{t('settings.general.themes.system')}</option>
              <option value="dark">{t('settings.general.themes.dark')}</option>
              <option value="light">{t('settings.general.themes.light')}</option>
            </select>
          </div>
        </div>

        <div className="settings-field">
          <label className="settings-label">{t('settings.general.language')}</label>
          <div className="settings-value">
            <select
              className="settings-input"
              style={{ width: '150px' }}
              value={i18n.language}
              onChange={(e) => handleLanguageChange(e.target.value)}
            >
              <option value="en">English</option>
              <option value="zh">中文</option>
            </select>
          </div>
        </div>

        <div className="settings-field">
          <label className="settings-label">{t('settings.general.showThinking')}</label>
          <div className="settings-value" style={{ display: 'flex', flexDirection: 'column', gap: '6px' }}>
            <label style={{ display: 'flex', alignItems: 'center', gap: '8px', cursor: 'pointer' }}>
              <input
                type="checkbox"
                checked={agentTrace === 'verbose'}
                onChange={(e) => dispatch(setAgentTrace(e.target.checked ? 'verbose' : 'concise'))}
              />
              <span>{t('settings.general.showThinkingEnabled')}</span>
            </label>
            <span style={{ fontSize: '12px', color: 'var(--text-tertiary)', lineHeight: 1.4 }}>
              {t('settings.general.showThinkingHint')}
            </span>
          </div>
        </div>

        <div className="settings-field">
          <label className="settings-label">{t('settings.general.defaultModel')}</label>
          <div className="settings-value">{config.default_model}</div>
        </div>

        <div className="settings-field">
          <label className="settings-label">{t('settings.general.availableModels')}</label>
          <div className="settings-value">
            {t('settings.general.modelsConfigured', { count: totalModels })}
          </div>
        </div>
      </div>

      <div className="settings-info-box">
        <InfoIcon size={16} style={{ flexShrink: 0, marginTop: '2px' }} />
        <span>{t('settings.general.tip')}</span>
      </div>
    </div>
  );
}
