import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import DatabaseIcon from 'lucide-react/dist/esm/icons/database.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import LayersIcon from 'lucide-react/dist/esm/icons/layers.mjs';
import TypeIcon from 'lucide-react/dist/esm/icons/type.mjs';
import MergeIcon from 'lucide-react/dist/esm/icons/merge.mjs';

export default function MemoryTab() {
  const config = useSelector((state: RootState) => state.settings.config);

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">No configuration loaded.</div>
      </div>
    );
  }

  const memory = config.memory;

  if (!memory) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">Memory is not configured. Add a [memory] section to config.toml to enable it.</div>
      </div>
    );
  }

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">
          <BrainIcon size={14} /> Memory Configuration
        </h3>

        <div className="settings-field">
          <DatabaseIcon size={12} />
          <label className="settings-label">Database Path</label>
          <div className="settings-value">{memory.db_path}</div>
        </div>

        <div className="settings-field">
          <BrainIcon size={12} />
          <label className="settings-label">Embedding Model</label>
          <div className="settings-value">{memory.embedding_model}</div>
        </div>

        <div className="settings-field">
          <LayersIcon size={12} />
          <label className="settings-label">Max Core Blocks</label>
          <div className="settings-value">{memory.max_core_blocks}</div>
        </div>

        <div className="settings-field">
          <TypeIcon size={12} />
          <label className="settings-label">Block Max Chars</label>
          <div className="settings-value">{(memory.default_block_max_chars ?? 0).toLocaleString()}</div>
        </div>

        <div className="settings-field">
          <MergeIcon size={12} />
          <label className="settings-label">Consolidation</label>
          <div className="settings-value">
            <span className={`badge badge-${memory.consolidation_enabled ? 'enabled' : 'disabled'}`}>
              {memory.consolidation_enabled ? 'Enabled' : 'Disabled'}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
