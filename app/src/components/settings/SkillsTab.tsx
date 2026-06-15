import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';

export default function SkillsTab() {
  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">
          <WrenchIcon size={14} /> Skills
        </h3>
        <div className="settings-empty">
          <BookOpenIcon size={32} style={{ marginBottom: '12px', opacity: 0.5 }} />
          <p>Skills management is not yet available in the UI.</p>
          <p style={{ fontSize: '12px', marginTop: '8px', opacity: 0.6 }}>
            Skills are loaded from the <code>~/.workbuddy/skills/</code> directory automatically.
          </p>
        </div>
      </div>
    </div>
  );
}
