import { useSelector, useDispatch } from 'react-redux';
import { RootState } from '../../store';
import InfoIcon from 'lucide-react/dist/esm/icons/info.mjs';
import { setAppearance } from '../../features/settings/settingsSlice';

export default function GeneralTab() {
  const config = useSelector((state: RootState) => state.settings.config);
  const appearance = useSelector((state: RootState) => state.settings.appearance);
  const dispatch = useDispatch();

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">No configuration loaded.</div>
      </div>
    );
  }

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">Application</h3>
        <div className="settings-field">
          <label className="settings-label">Appearance</label>
          <div className="settings-value">
            <select
              className="settings-input"
              style={{ width: '150px' }}
              value={appearance}
              onChange={(e) => dispatch(setAppearance(e.target.value as 'system' | 'dark' | 'light'))}
            >
              <option value="system">System</option>
              <option value="dark">Dark</option>
              <option value="light">Light</option>
            </select>
          </div>
        </div>
        <div className="settings-field">
          <label className="settings-label">Default Model</label>
          <div className="settings-value">{config.default_model}</div>
        </div>
        <div className="settings-field">
          <label className="settings-label">Available Models</label>
          <div className="settings-value">{Object.values(config.providers).reduce((sum, p) => sum + Object.keys(p.models).length, 0)} configured</div>
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">Permissions</h3>
        <div className="settings-field">
          <label className="settings-label">Permission Mode</label>
          <div className="settings-value">
            <span className={`badge badge-${config.permissions.mode}`}>{config.permissions.mode}</span>
          </div>
        </div>
        {config.permissions.auto_allow_up_to && (
          <div className="settings-field">
            <label className="settings-label">Auto-allow up to</label>
            <div className="settings-value">{config.permissions.auto_allow_up_to}</div>
          </div>
        )}
        <div className="settings-field">
          <label className="settings-label">Custom Rules</label>
          <div className="settings-value">{config.permissions.rules.length} rules</div>
        </div>
        <div className="settings-field">
          <label className="settings-label">Whitelist</label>
          <div className="settings-value">{config.permissions.whitelist.length} entries</div>
        </div>
        <div className="settings-field">
          <label className="settings-label">Blacklist</label>
          <div className="settings-value">{config.permissions.blacklist.length} entries</div>
        </div>
      </div>

      <div className="settings-info-box">
        <InfoIcon size={14} />
        <span>Configuration is read from <code>config.toml</code>. Restart the application to apply changes.</span>
      </div>
    </div>
  );
}
