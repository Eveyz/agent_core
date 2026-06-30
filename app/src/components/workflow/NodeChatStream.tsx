import type { LiveLogEntry } from "../../features/workflow/types";
import "./NodeChatStream.css";

export function NodeChatStream({ logs }: { logs: LiveLogEntry[] }) {
  if (!logs || logs.length === 0) return null;

  return (
    <div className="node-chat-stream">
      {logs.map((log, i) => (
        <div 
          key={log.id || i} 
          className={`node-chat-log-item ${log.type}`}
        >
          <div className="node-chat-log-type">
            {log.type === "thought" && <span className="log-badge thought">Thought</span>}
            {log.type}
          </div>
          <div className="node-chat-log-content">{log.content}</div>
        </div>
      ))}
    </div>
  );
}
