import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';

interface SkillManifest {
  name: string;
  description: string;
  version: string;
  triggers: string[];
  tags: string[];
  read_when: string[];
}

export default function SkillsTab() {
  const [skills, setSkills] = useState<SkillManifest[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    async function loadSkills() {
      try {
        const data = await invoke<SkillManifest[]>('get_skills');
        setSkills(data);
      } catch (err) {
        console.error('Failed to load skills', err);
      } finally {
        setLoading(false);
      }
    }
    loadSkills();
  }, []);

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">
          <WrenchIcon size={14} /> Skills
        </h3>
        
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
                    <ZapIcon size={14} style={{ color: 'var(--accent-color)' }} />
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
