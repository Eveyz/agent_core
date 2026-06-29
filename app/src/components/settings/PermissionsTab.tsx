import { useSelector, useDispatch } from 'react-redux';
import { RootState } from '../../store';
import { saveConfig } from '../../features/settings/settingsSlice';
import ShieldIcon from 'lucide-react/dist/esm/icons/shield.mjs';
import ShieldAlertIcon from 'lucide-react/dist/esm/icons/shield-alert.mjs';
import ShieldCheckIcon from 'lucide-react/dist/esm/icons/shield-check.mjs';
import CodeIcon from 'lucide-react/dist/esm/icons/code.mjs';
import UnlockIcon from 'lucide-react/dist/esm/icons/unlock.mjs';
import InfoIcon from 'lucide-react/dist/esm/icons/info.mjs';

const PERMISSION_LEVELS = [
  {
    id: 'strict',
    name: 'Strict',
    icon: ShieldAlertIcon,
    description: 'Agent asks for permission for every action. Highest security, lowest autonomy.',
    mode: 'paranoid',
    auto_allow_up_to: undefined,
  },
  {
    id: 'standard',
    name: 'Standard',
    icon: ShieldIcon,
    description: 'Agent can automatically read files and search the web, but asks to execute commands or modify files. High security, medium autonomy.',
    mode: 'standard',
    auto_allow_up_to: undefined,
  },
  {
    id: 'developer',
    name: 'Developer (Recommended)',
    icon: CodeIcon,
    description: 'Agent automatically executes safe read-only shell commands (ls, pwd, etc), reads files, and searches the web. File modifications or destructive commands still require permission.',
    mode: 'standard',
    auto_allow_up_to: 'readonly',
  },
  {
    id: 'permissive',
    name: 'Permissive',
    icon: ShieldCheckIcon,
    description: 'Agent automatically modifies files and makes network requests. System commands still prompt. Low security, high autonomy.',
    mode: 'permissive',
    auto_allow_up_to: undefined,
  },
  {
    id: 'yolo',
    name: 'YOLO',
    icon: UnlockIcon,
    description: 'Everything is allowed. No prompts ever. Use at your own risk.',
    mode: 'yolo',
    auto_allow_up_to: undefined,
  },
];

export default function PermissionsTab() {
  const config = useSelector((state: RootState) => state.settings.config);
  const dispatch = useDispatch<any>();

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">No configuration loaded.</div>
      </div>
    );
  }

  const currentMode = config.permissions.mode;
  const currentAutoAllow = config.permissions.auto_allow_up_to;

  const handleSelectLevel = (mode: string, autoAllow?: string) => {
    const newConfig = {
      ...config,
      permissions: {
        ...config.permissions,
        mode,
        auto_allow_up_to: autoAllow,
      },
    };
    dispatch(saveConfig(newConfig));
  };

  const activeLevelId = PERMISSION_LEVELS.find(
    (level) =>
      level.mode === currentMode &&
      level.auto_allow_up_to === currentAutoAllow
  )?.id || 'custom';

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">Security & Autonomy Level</h3>
        <p className="settings-section-description">
          Choose how much autonomy the agent has. Higher autonomy means fewer prompts, but less control over what the agent does.
        </p>
        <div className="permission-levels-grid" style={{ display: 'flex', flexDirection: 'column', gap: '12px', marginTop: '16px' }}>
          {PERMISSION_LEVELS.map((level) => {
            const Icon = level.icon;
            const isActive = activeLevelId === level.id;
            return (
              <div
                key={level.id}
                onClick={() => handleSelectLevel(level.mode, level.auto_allow_up_to)}
                className={`permission-card ${isActive ? 'active' : ''}`}
              >
                <div className="permission-card-icon">
                  <Icon size={24} />
                </div>
                <div className="permission-card-content">
                  <div className="permission-card-header">
                    <h4 className="permission-card-title">
                      {level.name}
                    </h4>
                    <div className="settings-radio">
                      <div className="settings-radio-inner" />
                    </div>
                  </div>
                  <p className="permission-card-desc">
                    {level.description}
                  </p>
                </div>
              </div>
            );
          })}
        </div>
      </div>
      
      {activeLevelId === 'custom' && (
        <div className="settings-info-box" style={{ marginTop: '16px' }}>
          <InfoIcon size={14} />
          <span>You are using a custom permission configuration defined in <code>config.toml</code>. Selecting a preset will override it.</span>
        </div>
      )}
    </div>
  );
}
