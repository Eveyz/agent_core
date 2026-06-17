import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';

export function useGitBranch(projectPath: string | undefined) {
  const [branches, setBranches] = useState<string[]>([]);
  const [activeBranch, setActiveBranch] = useState<string>('');
  const [branchError, setBranchError] = useState<string>('');
  const [showBranchDropdown, setShowBranchDropdown] = useState(false);
  const branchDropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!projectPath) return;
    invoke<string[]>('list_git_branches', { path: projectPath })
      .then((b) => {
        setBranches(b);
        setBranchError('');
        if (b.length > 0 && !activeBranch) {
          setActiveBranch(b[0]);
        }
      })
      .catch((e) => {
        setBranches([]);
        setBranchError(String(e));
      });
  }, [projectPath]);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (branchDropdownRef.current && !branchDropdownRef.current.contains(e.target as Node)) {
        setShowBranchDropdown(false);
      }
    }
    if (showBranchDropdown) {
      document.addEventListener('mousedown', handleClick);
      return () => document.removeEventListener('mousedown', handleClick);
    }
  }, [showBranchDropdown]);

  const handleSwitchBranch = useCallback(
    async (branch: string) => {
      if (!projectPath || branch === activeBranch) {
        setShowBranchDropdown(false);
        return;
      }
      try {
        await invoke('switch_git_branch', { path: projectPath, branch });
        setActiveBranch(branch);
        setBranchError('');
      } catch (e) {
        setBranchError(String(e));
      }
      setShowBranchDropdown(false);
    },
    [projectPath, activeBranch]
  );

  return {
    branches,
    activeBranch,
    branchError,
    showBranchDropdown,
    setShowBranchDropdown,
    branchDropdownRef,
    handleSwitchBranch,
  };
}
