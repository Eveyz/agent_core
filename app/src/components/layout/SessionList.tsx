import { useState, useRef, useEffect } from "react";
import { SessionMeta } from "../../features/project/projectSlice";
import { formatTimeAgo } from "../../utils/time";
import LoaderIcon from "lucide-react/dist/esm/icons/loader.mjs";
import TrashIcon from "lucide-react/dist/esm/icons/trash.mjs";
import MoreHorizontalIcon from "lucide-react/dist/esm/icons/more-horizontal.mjs";

// ── Context menu hook ────────────────────────────────────────────────

function useContextMenu() {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    if (open) {
      document.addEventListener("mousedown", handleClick);
      return () => document.removeEventListener("mousedown", handleClick);
    }
  }, [open]);

  return { open, setOpen, pos, setPos, menuRef };
}

// ── Session context menu ─────────────────────────────────────────────

function SessionContextMenu({
  sessionId,
  onDelete,
}: {
  sessionId: string;
  onDelete: (sessionId: string) => void;
}) {
  const { open, setOpen, pos, setPos, menuRef } = useContextMenu();

  return (
    <>
      <button
        className="sidebar-context-trigger"
        onClick={(e) => {
          e.stopPropagation();
          setPos({ x: e.clientX, y: e.clientY });
          setOpen(true);
        }}
      >
        <MoreHorizontalIcon size={12} />
      </button>
      {open && (
        <div
          ref={menuRef}
          className="context-menu"
          style={{ left: pos.x, top: pos.y }}
        >
          <div
            className="context-menu-item context-menu-danger"
            onClick={(e) => {
              e.stopPropagation();
              setOpen(false);
              onDelete(sessionId);
            }}
          >
            <TrashIcon size={13} /> Delete
          </div>
        </div>
      )}
    </>
  );
}

// ── Session list ─────────────────────────────────────────────────────

interface SessionListProps {
  sessions: SessionMeta[];
  activeSessionId: string | null;
  onSelectSession: (sessionId: string) => void;
  title: string;
  emptyMessage: string;
  isProcessing: boolean;
  onDeleteSession: (sessionId: string) => void;
  paddingLeft?: string;
}

export function SessionList({
  sessions,
  activeSessionId,
  onSelectSession,
  emptyMessage,
  isProcessing,
  onDeleteSession,
  paddingLeft,
}: SessionListProps) {
  const maxInitial = 6;
  const [fullyExpanded, setFullyExpanded] = useState(false);

  if (sessions.length === 0) {
    return (
      <div
        style={{ padding: "8px 20px", color: "var(--text-tertiary)", fontSize: "12px" }}
      >
        {emptyMessage}
      </div>
    );
  }

  const visibleSessions = fullyExpanded
    ? sessions
    : sessions.slice(0, maxInitial);
  const hiddenCount = sessions.length - maxInitial;

  return (
    <>
      {visibleSessions.map((session) => {
        const isActive = activeSessionId === session.id;
        return (
          <div
            key={session.id}
            className={`session-row ${isActive ? "session-row-active" : ""}`}
            style={paddingLeft ? { paddingLeft } : undefined}
            onClick={() => onSelectSession(session.id)}
          >
            <span className="session-row-text">
              {session.title || "Untitled"}
            </span>
            <span className="session-row-time">
              {isActive && isProcessing ? (
                <LoaderIcon
                  size={12}
                  className="session-processing-spinner"
                  style={{ marginRight: 0 }}
                />
              ) : (
                formatTimeAgo(session.updated_at)
              )}
            </span>
            <span className="session-row-actions">
              <SessionContextMenu
                sessionId={session.id}
                onDelete={onDeleteSession}
              />
              <button
                className="sidebar-context-trigger session-delete-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  onDeleteSession(session.id);
                }}
                title="Delete session"
              >
                <TrashIcon size={12} />
              </button>
            </span>
          </div>
        );
      })}
      {!fullyExpanded && hiddenCount > 0 && (
        <div
          className="session-expand-toggle"
          style={paddingLeft ? { paddingLeft } : undefined}
          onClick={() => setFullyExpanded(true)}
        >
          See all ({hiddenCount})
        </div>
      )}
      {fullyExpanded && hiddenCount > 0 && (
        <div
          className="session-expand-toggle"
          style={paddingLeft ? { paddingLeft } : undefined}
          onClick={() => setFullyExpanded(false)}
        >
          View less
        </div>
      )}
    </>
  );
}
