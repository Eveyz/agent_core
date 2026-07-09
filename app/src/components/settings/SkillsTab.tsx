import { useState } from 'react';


import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import RefreshIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';
import { useSkills } from '../../hooks/useSkills';

export default function SkillsTab() {
  const { skills, loading, refresh, invalidate } = useSkills();
  const [refreshing, setRefreshing] = useState(false);

  const handleRefresh = async () => {
    setRefreshing(true);
    await invalidate();
    await refresh();
    setRefreshing(false);
  };

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
          <h3 className="settings-section-title" style={{ margin: 0 }}>
            <ZapIcon size={14} style={{ color: 'var(--violet-500)' }} /> Skills
          </h3>
          <button
            className="icon-btn"
            onClick={handleRefresh}
            disabled={loading || refreshing}
            title="Refresh skills"
            style={{ opacity: loading || refreshing ? 0.5 : 1 }}
          >
            <RefreshIcon size={14} className={refreshing ? 'spinning' : ''} />
          </button>
        </div>

        {loading ? (
          <div className="settings-empty">Loading skills...</div>
        ) : skills.length === 0 ? (
          <div className="settings-empty">
            <BookOpenIcon size={32} style={{ marginBottom: '12px', opacity: 0.5 }} />
            <p>No skills installed.</p>
            <p style={{ fontSize: '12px', marginTop: '8px', opacity: 0.6 }}>
              Skills are loaded from the <code>~/.agverse/skills/</code> directory automatically.
            </p>
          </div>
        ) : (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))', gap: '16px', marginTop: '16px' }}>
            {skills.map((skill, i) => (
              <div key={i} style={{ 
                background: 'var(--bg-tertiary)', 
                border: '1px solid var(--border-color)', 
                borderRadius: '8px', 
                padding: '16px',
                display: 'flex',
                flexDirection: 'column',
                gap: '8px'
              }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <div style={{ fontWeight: 600, fontSize: '14px', color: 'var(--text-primary)', display: 'flex', alignItems: 'center', gap: '6px' }}>
                    <ZapIcon size={14} style={{ color: 'var(--violet-500)' }} />
                    {skill.name}
                  </div>
                  <div style={{ fontSize: '11px', color: 'var(--text-secondary)', background: 'var(--bg-secondary)', padding: '2px 6px', borderRadius: '4px' }}>
                    v{skill.version}
                  </div>
                </div>
                
                <div style={{ fontSize: '13px', color: 'var(--text-secondary)', lineHeight: '1.4' }}>
                  {skill.description}
                </div>

                {skill.triggers && skill.triggers.length > 0 && (
                  <div style={{ marginTop: 'auto', paddingTop: '8px', display: 'flex', flexWrap: 'wrap', gap: '6px' }}>
                    {skill.triggers.map(trigger => (
                      <span key={trigger} style={{ 
                        fontSize: '11px', 
                        background: 'var(--bg-secondary)', 
                        color: 'var(--text-secondary)',
                        padding: '2px 8px',
                        borderRadius: '12px',
                        border: '1px solid var(--border-color)'
                      }}>
                        {trigger}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
