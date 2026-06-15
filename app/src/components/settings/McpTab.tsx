import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';
import TerminalIcon from 'lucide-react/dist/esm/icons/terminal.mjs';
import LinkIcon from 'lucide-react/dist/esm/icons/link.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import CheckCircleIcon from 'lucide-react/dist/esm/icons/check-circle.mjs';
import XCircleIcon from 'lucide-react/dist/esm/icons/x-circle.mjs';

export default function McpTab() {
  const config = useSelector((state: RootState) => state.settings.config);

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">No configuration loaded.</div>
      </div>
    );
  }

  const servers = config.mcp.servers;

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">
          <ServerIcon size={14} /> MCP Servers ({servers.length})
        </h3>

        {servers.length === 0 && (
          <div className="settings-empty">
            No MCP servers configured. Add [[mcp.servers]] sections to config.toml to register tools from MCP servers.
          </div>
        )}

        {servers.map((server, idx) => (
          <div key={idx} className="mcp-server-card">
            <div className="mcp-server-header">
              <span className="mcp-server-name">{server.name}</span>
              <span className={`mcp-server-status ${server.enabled !== false ? 'status-enabled' : 'status-disabled'}`}>
                {server.enabled !== false ? (
                  <><CheckCircleIcon size={12} /> Enabled</>
                ) : (
                  <><XCircleIcon size={12} /> Disabled</>
                )}
              </span>
            </div>
            <div className="mcp-server-body">
              {server.transport && (
                <div className="mcp-field">
                  <LinkIcon size={12} />
                  <span className="mcp-field-label">Transport</span>
                  <span className="mcp-field-value">{server.transport}</span>
                </div>
              )}
              {server.command && (
                <div className="mcp-field">
                  <TerminalIcon size={12} />
                  <span className="mcp-field-label">Command</span>
                  <span className="mcp-field-value">{server.command}</span>
                </div>
              )}
              {server.args && server.args.length > 0 && (
                <div className="mcp-field">
                  <SettingsIcon size={12} />
                  <span className="mcp-field-label">Args</span>
                  <span className="mcp-field-value">{server.args.join(' ')}</span>
                </div>
              )}
              {server.url && (
                <div className="mcp-field">
                  <LinkIcon size={12} />
                  <span className="mcp-field-label">URL</span>
                  <span className="mcp-field-value">{server.url}</span>
                </div>
              )}
              {server.env && Object.keys(server.env).length > 0 && (
                <div className="mcp-field mcp-field-full">
                  <span className="mcp-field-label">Environment</span>
                  <div className="mcp-env-list">
                    {Object.entries(server.env).map(([k, v]) => (
                      <div key={k} className="mcp-env-item">
                        <code>{k}</code> = <code className="mcp-env-value">{v}</code>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
