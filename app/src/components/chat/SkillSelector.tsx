import { useState, useCallback, useRef, useEffect, useMemo } from 'react';

import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import RefreshIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';
import ClockIcon from 'lucide-react/dist/esm/icons/clock.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import { useSkills } from '../../hooks/useSkills';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import type { SkillManifest } from '../../features/chat/types';
import { openSettings, setActiveTab } from '../../features/settings/settingsSlice';

const RECENT_SKILLS_KEY = 'recent_skills';
const MAX_RECENT_SKILLS = 5;

interface SkillSelectorProps {
  onSelect: (skill: SkillManifest) => void;
  externalOpen?: boolean;
  onExternalOpenChange?: (open: boolean) => void;
}

export function SkillSelector({ onSelect, externalOpen, onExternalOpenChange }: SkillSelectorProps) {
  const dispatch = useAppDispatch();
  const { skills, loading, refreshFromDisk } = useSkills();
  const [internalOpen, setInternalOpen] = useState(false);
  const open = externalOpen ?? internalOpen;
  const setOpen = onExternalOpenChange ?? setInternalOpen;
  const [search, setSearch] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [recentSkills, setRecentSkills] = useState<SkillManifest[]>([]);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Load recent skills from localStorage
  useEffect(() => {
    try {
      const stored = localStorage.getItem(RECENT_SKILLS_KEY);
      if (stored) {
        setRecentSkills(JSON.parse(stored));
      }
    } catch (e) {
      console.error('Failed to load recent skills:', e);
    }
  }, []);

  // Save recent skills to localStorage
  const addToRecent = useCallback((skill: SkillManifest) => {
    setRecentSkills((prev) => {
      const filtered = prev.filter((s) => s.name !== skill.name);
      const updated = [skill, ...filtered].slice(0, MAX_RECENT_SKILLS);
      try {
        localStorage.setItem(RECENT_SKILLS_KEY, JSON.stringify(updated));
      } catch (e) {
        console.error('Failed to save recent skills:', e);
      }
      return updated;
    });
  }, []);

  // Outside click dismiss
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false);
        setSearch('');
        setSelectedIndex(0);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  // Auto-focus search on open, and rescan disk so newly added skills appear.
  useEffect(() => {
    if (!open) return;
    void refreshFromDisk();
    if (searchInputRef.current) {
      searchInputRef.current.focus();
    }
  }, [open, refreshFromDisk]);

  // Reset selected index when search changes
  useEffect(() => {
    setSelectedIndex(0);
  }, [search]);

  // Filter skills
  const filteredSkills = useMemo(() => {
    if (!search.trim()) return skills;
    const q = search.toLowerCase();
    return skills.filter(
      (skill) =>
        skill.name.toLowerCase().includes(q) ||
        skill.description.toLowerCase().includes(q) ||
        skill.triggers?.some((t) => t.toLowerCase().includes(q))
    );
  }, [skills, search]);

  // Filter recent skills (exclude those already in filtered results)
  const filteredRecent = useMemo(() => {
    const availableNames = new Set(skills.map((skill) => skill.name));
    const availableRecent = recentSkills.filter((skill) => availableNames.has(skill.name));
    if (!search.trim()) return availableRecent;
    const q = search.toLowerCase();
    return availableRecent.filter(
      (skill) =>
        skill.name.toLowerCase().includes(q) ||
        skill.description.toLowerCase().includes(q) ||
        skill.triggers?.some((t) => t.toLowerCase().includes(q))
    );
  }, [recentSkills, skills, search]);

  // All items for keyboard navigation
  const allItems = useMemo(() => {
    const items: { type: 'recent' | 'all'; skill: SkillManifest }[] = [];
    filteredRecent.forEach((skill) => items.push({ type: 'recent', skill }));
    filteredSkills.forEach((skill) => {
      if (!items.some((i) => i.skill.name === skill.name)) {
        items.push({ type: 'all', skill });
      }
    });
    return items;
  }, [filteredRecent, filteredSkills]);

  // Handle keyboard navigation
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (allItems.length === 0) return;
        setSelectedIndex((prev) => (prev + 1) % allItems.length);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (allItems.length === 0) return;
        setSelectedIndex((prev) => (prev - 1 + allItems.length) % allItems.length);
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (allItems[selectedIndex]) {
          handleSelect(allItems[selectedIndex].skill);
        }
      } else if (e.key === 'Escape') {
        e.preventDefault();
        setOpen(false);
        setSearch('');
        setSelectedIndex(0);
      }
    },
    [allItems, selectedIndex]
  );

  // Scroll selected item into view
  useEffect(() => {
    if (open && listRef.current) {
      const items = listRef.current.querySelectorAll('.skill-dropdown-item');
      const selected = items[selectedIndex] as HTMLElement;
      if (selected) {
        selected.scrollIntoView({ block: 'nearest' });
      }
    }
  }, [selectedIndex, open]);

  const handleSelect = useCallback(
    (skill: SkillManifest) => {
      setOpen(false);
      setSearch('');
      setSelectedIndex(0);
      addToRecent(skill);
      onSelect(skill);
    },
    [onSelect, addToRecent]
  );

  const handleRefresh = useCallback(async () => {
    await refreshFromDisk();
  }, [refreshFromDisk]);

  const handleOpenSkillSettings = useCallback(() => {
    setOpen(false);
    dispatch(setActiveTab('skills'));
    dispatch(openSettings());
  }, [dispatch, setOpen]);



  return (
    <div className="skill-selector-wrapper" ref={dropdownRef}>
      <button
        className="icon-btn"
        onClick={() => setOpen(!open)}
        aria-label="Select skill"
        aria-expanded={open}
        aria-haspopup="listbox"
      >
        <ZapIcon size={16} style={{ color: 'var(--violet-500)' }} />
      </button>

      {open && (
        <div className="model-dropdown-shell">
          <div className="model-dropdown skill-dropdown" onKeyDown={handleKeyDown} role="listbox">
            <div className="model-dropdown-header">
              <button
                className="skill-refresh-btn"
                onClick={handleRefresh}
                title="Refresh skills"
                aria-label="Refresh skills"
              >
                <RefreshIcon size={14} />
              </button>
              <div className="model-dropdown-search">
                <SearchIcon size={14} />
                <input
                  ref={searchInputRef}
                  className="model-dropdown-search-input"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder="Search skills..."
                  aria-label="Search skills"
                />
              </div>
            </div>

            <div className="model-dropdown-list" ref={listRef}>
              {loading && <div className="model-dropdown-empty">Loading skills...</div>}

              {!loading && allItems.length === 0 && (
                <div className="model-dropdown-empty">
                  No skills found
                  {skills.length === 0 && (
                    <div style={{ marginTop: '8px', fontSize: '12px' }}>
                      <button type="button" className="skill-settings-link" onClick={handleOpenSkillSettings}>
                        Install skills in Settings
                      </button>
                    </div>
                  )}
                </div>
              )}

              {!loading && allItems.length > 0 && (
                <>
                  {filteredRecent.length > 0 && (
                    <div className="skill-dropdown-section">
                      <div className="skill-dropdown-section-header">
                        <ClockIcon size={12} />
                        <span>Recent</span>
                      </div>
                      {filteredRecent.map((skill) => (
                        <button
                          key={`recent-${skill.name}`}
                          className={`skill-dropdown-item ${
                            allItems[selectedIndex]?.skill.name === skill.name ? 'selected' : ''
                          }`}
                          onClick={() => handleSelect(skill)}
                          role="option"
                          aria-selected={allItems[selectedIndex]?.skill.name === skill.name}
                        >
                          <ZapIcon size={14} className="skill-item-icon" style={{ color: 'var(--violet-500)' }} />
                          <div className="skill-item-content">
                            <div className="skill-item-name">{skill.name}</div>
                            <div className="skill-item-description">{skill.description}</div>
                          </div>
                        </button>
                      ))}
                    </div>
                  )}

                  {filteredSkills.length > 0 && (
                    <div className="skill-dropdown-section">
                      <div className="skill-dropdown-section-header">
                        <ZapIcon size={12} />
                        <span>All Skills</span>
                      </div>
                      {filteredSkills.map((skill) => (
                        <button
                          key={skill.name}
                          className={`skill-dropdown-item ${
                            allItems[selectedIndex]?.skill.name === skill.name ? 'selected' : ''
                          }`}
                          onClick={() => handleSelect(skill)}
                          role="option"
                          aria-selected={allItems[selectedIndex]?.skill.name === skill.name}
                        >
                          <ZapIcon size={14} className="skill-item-icon" style={{ color: 'var(--violet-500)' }} />
                          <div className="skill-item-content">
                            <div className="skill-item-name">{skill.name}</div>
                            <div className="skill-item-description">{skill.description}</div>
                          </div>
                        </button>
                      ))}
                    </div>
                  )}
                </>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
