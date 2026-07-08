import { useState } from "react";
import { SessionMeta } from "../../features/project/projectSlice";
import { formatTimeAgo } from "../../utils/time";
import TrashIcon from "lucide-react/dist/esm/icons/trash.mjs";
import MoreHorizontalIcon from "lucide-react/dist/esm/icons/more-horizontal.mjs";

// ── Context menu hook ────────────────────────────────────────────────

function useContextMenu() {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ x: 0, y: 0 });

  return { open, setOpen, pos, setPos };
}

// ── Session context menu ─────────────────────────────────────────────

function SessionContextMenu({
  sessionId,
  onDelete,
}: {
  sessionId: string;
  onDelete: (sessionId: string) => void;
}) {
  const { open, setOpen, pos, setPos } = useContextMenu();

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
        <>
          <div className="context-menu-backdrop" onClick={() => setOpen(false)} />
          <div
            className="context-menu"
            style={{ left: pos.x, top: pos.y }}
            onClick={(e) => e.stopPropagation()}
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
        </>
      )}
    </>
  );
}

// ── Session row ──────────────────────────────────────────────────────

interface SessionRowProps {
  session: SessionMeta;
  isActive: boolean;
  isProcessing: boolean;
  onSelect: (sessionId: string) => void;
  onDelete: (sessionId: string) => void;
}

function SessionRow({ session, isActive, isProcessing, onSelect, onDelete }: SessionRowProps) {
  return (
    <div
      className={`session-row ${isActive ? "session-row-active" : ""}`}
      onClick={() => onSelect(session.id)}
    >
      <span className="session-row-text">
        {session.title || "Untitled"}
      </span>
      <span className="session-row-time">
        {isProcessing ? (
          <span className="session-processing-spinner" />
        ) : (
          formatTimeAgo(session.updated_at)
        )}
      </span>
      <span className="session-row-actions">
        <SessionContextMenu
          sessionId={session.id}
          onDelete={onDelete}
        />
        <button
          className="sidebar-context-trigger session-delete-btn"
          onClick={(e) => {
            e.stopPropagation();
            onDelete(session.id);
          }}
          title="Delete session"
        >
          <TrashIcon size={12} />
        </button>
      </span>
    </div>
  );
}

// ── Session list ─────────────────────────────────────────────────────

interface SessionListProps {
  sessions: SessionMeta[];
  activeSessionId: string | null;
  onSelectSession: (sessionId: string) => void;
  title: string;
  emptyMessage: string;
  processingBySession: Record<string, boolean>;
  onDeleteSession: (sessionId: string) => void;
  paddingLeft?: string;
}

export function SessionList({
  sessions,
  activeSessionId,
  onSelectSession,
  emptyMessage,
  processingBySession,
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
      {visibleSessions.map((session) => (
        <SessionRow
          key={session.id}
          session={session}
          isActive={activeSessionId === session.id}
          isProcessing={processingBySession[session.id] ?? false}
          onSelect={onSelectSession}
          onDelete={onDeleteSession}
        />
      ))}
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
