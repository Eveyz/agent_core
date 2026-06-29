import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import FolderIcon from 'lucide-react/dist/esm/icons/folder.mjs';
import FileImageIcon from 'lucide-react/dist/esm/icons/file-image.mjs';
import FileArchiveIcon from 'lucide-react/dist/esm/icons/file-archive.mjs';
import FileLockIcon from 'lucide-react/dist/esm/icons/file-lock.mjs';
import FileCogIcon from 'lucide-react/dist/esm/icons/file-cog.mjs';
import DatabaseIcon from 'lucide-react/dist/esm/icons/database.mjs';
import FileSpreadsheetIcon from 'lucide-react/dist/esm/icons/file-spreadsheet.mjs';
import TerminalIcon from 'lucide-react/dist/esm/icons/terminal.mjs';
import { 
  SiRust, SiJavascript, SiTypescript, SiReact, 
  SiPython, SiC, SiCplusplus, SiGo, 
  SiHtml5, SiCss, SiJson, SiMarkdown, SiToml,
  SiYaml, SiDocker, SiGit, SiVuedotjs, SiSvelte,
  SiPhp, SiRuby, SiSwift, SiKotlin
} from 'react-icons/si';
import { FaFileWord, FaFilePdf } from 'react-icons/fa';

function getFileIcon(name: string) {
  const lowerName = name.toLowerCase();
  
  if (lowerName.endsWith('.lock') || lowerName.includes('-lock')) return <FileLockIcon size={14} color="var(--text-muted)" />;
  if (lowerName === 'dockerfile' || lowerName === '.dockerignore') return <SiDocker size={14} color="#2496ed" />;
  if (lowerName === '.gitignore' || lowerName === '.gitattributes') return <SiGit size={14} color="#f14e32" />;
  if (lowerName.startsWith('.env')) return <FileCogIcon size={14} color="var(--text-muted)" />;

  const ext = name.split('.').pop()?.toLowerCase();
  switch (ext) {
    case 'rs':
      return <SiRust size={14} color="#dea584" />;
    case 'js':
      return <SiJavascript size={14} color="#f7df1e" />;
    case 'ts':
      return <SiTypescript size={14} color="#3178c6" />;
    case 'jsx':
    case 'tsx':
      return <SiReact size={14} color="#61dafb" />;
    case 'py':
      return <SiPython size={14} color="#3776ab" />;
    case 'c':
      return <SiC size={14} color="#a8b9cc" />;
    case 'cpp':
    case 'cc':
      return <SiCplusplus size={14} color="#00599c" />;
    case 'go':
      return <SiGo size={14} color="#00add8" />;
    case 'html':
      return <SiHtml5 size={14} color="#e34f26" />;
    case 'css':
      return <SiCss size={14} color="#1572b6" />;
    case 'json':
      return <SiJson size={14} color="var(--text-muted)" />;
    case 'toml':
      return <SiToml size={14} color="var(--text-muted)" />;
    case 'yaml':
    case 'yml':
      return <SiYaml size={14} color="#cb171e" />;
    case 'vue':
      return <SiVuedotjs size={14} color="#4fc08d" />;
    case 'svelte':
      return <SiSvelte size={14} color="#ff3e00" />;
    case 'php':
      return <SiPhp size={14} color="#777bb4" />;
    case 'rb':
    case 'ruby':
      return <SiRuby size={14} color="#cc342d" />;
    case 'swift':
      return <SiSwift size={14} color="#f05138" />;
    case 'kt':
    case 'kotlin':
      return <SiKotlin size={14} color="#7f52ff" />;
    case 'sh':
    case 'bash':
    case 'zsh':
      return <TerminalIcon size={14} />;
    case 'db':
    case 'sqlite':
    case 'sql':
      return <DatabaseIcon size={14} />;
    case 'md':
      return <SiMarkdown size={14} color="var(--text-muted)" />;
    case 'doc':
    case 'docx':
      return <FaFileWord size={14} color="#2b579a" />;
    case 'pdf':
      return <FaFilePdf size={14} color="#f40f02" />;
    case 'csv':
    case 'xlsx':
    case 'xls':
      return <FileSpreadsheetIcon size={14} color="#107c41" />;
    case 'png':
    case 'jpg':
    case 'jpeg':
    case 'gif':
    case 'svg':
      return <FileImageIcon size={14} />;
    case 'zip':
    case 'tar':
    case 'gz':
      return <FileArchiveIcon size={14} />;
    default:
      return <FileIcon size={14} />;
  }
}

interface FileNode {
  name: string;
  type: 'file' | 'dir';
  size: string;
  path: string;
}

interface FileTreeItemProps {
  node: FileNode;
  level: number;
  onSelect?: (path: string) => void;
}

function FileTreeItem({ node, level, onSelect }: FileTreeItemProps) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<FileNode[]>([]);
  const [loading, setLoading] = useState(false);

  const isDir = node.type === 'dir';

  const handleToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isDir) {
      if (onSelect) onSelect(node.path);
      return;
    }

    if (!expanded && children.length === 0) {
      setLoading(true);
      try {
        const result: any[] = await invoke('list_directory', { path: node.path });
        const visibleFiles = result.filter(item => !item.name.startsWith('.'));
        setChildren(visibleFiles.map(item => ({
          ...item,
          path: `${node.path}/${item.name}`
        })));
      } catch (err) {
        console.error("Failed to load directory", err);
      } finally {
        setLoading(false);
      }
    }
    setExpanded(!expanded);
  };

  return (
    <div>
      <div 
        className="file-tree-row" 
        style={{ paddingLeft: `${level * 12 + 8}px` }}
        onClick={handleToggle}
      >
        <span className="file-tree-icon-wrap">
          {isDir ? (
            expanded ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />
          ) : (
            <span style={{ width: 14, display: 'inline-block' }} />
          )}
        </span>
        <span className="file-tree-type-icon">
          {isDir ? <FolderIcon size={14} /> : getFileIcon(node.name)}
        </span>
        <span className="file-tree-name">{node.name}</span>
      </div>
      {expanded && isDir && (
        <div className="file-tree-children">
          {loading ? (
            <div className="file-tree-loading" style={{ paddingLeft: `${(level + 1) * 12 + 28}px` }}>Loading...</div>
          ) : (
            children.map(child => (
              <FileTreeItem key={child.path} node={child} level={level + 1} onSelect={onSelect} />
            ))
          )}
        </div>
      )}
    </div>
  );
}

interface FileTreeProps {
  rootPath: string;
  onSelectFile?: (path: string) => void;
}

export function FileTree({ rootPath, onSelectFile }: FileTreeProps) {
  const [nodes, setNodes] = useState<FileNode[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!rootPath) return;
    
    let isMounted = true;
    setLoading(true);

    invoke('list_directory', { path: rootPath })
      .then((result: any) => {
        if (!isMounted) return;
        const visibleFiles = result.filter((item: any) => !item.name.startsWith('.'));
        setNodes(visibleFiles.map((item: any) => ({
          ...item,
          path: `${rootPath}/${item.name}`
        })));
      })
      .catch(err => {
        console.error("Failed to load root directory", err);
      })
      .finally(() => {
        if (isMounted) setLoading(false);
      });

    return () => { isMounted = false; };
  }, [rootPath]);

  if (!rootPath) return <div className="empty-message">No project loaded</div>;
  if (loading) return <div className="empty-message">Loading tree...</div>;

  return (
    <div className="file-tree-container">
      {nodes.map(node => (
        <FileTreeItem key={node.path} node={node} level={0} onSelect={onSelectFile} />
      ))}
    </div>
  );
}
