import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import SettingsIcon from "lucide-react/dist/esm/icons/settings.mjs";
import SendIcon from "lucide-react/dist/esm/icons/send.mjs";
import LoaderIcon from "lucide-react/dist/esm/icons/loader.mjs";
import MessageSquareIcon from "lucide-react/dist/esm/icons/message-square.mjs";
import ZapIcon from "lucide-react/dist/esm/icons/zap.mjs";
import { useAppSelector } from "../../hooks/useAppDispatch";
import type {
  AgentConversationMessage,
  AgentConversationSendResult,
  AgentConversationView,
  AgentDef,
} from "../../features/agents/types";
import { resolveAgentMentions, type SelectedAgentMention } from "../chat/agentMentions";
import { AssistantMarkdownContent } from "../chat/AssistantMarkdownContent";
import "./AgentConversationChat.css";

interface AgentConversationChatProps {
  agent: AgentDef;
  onOpenSettings: () => void;
}

interface AgentMessageMetadata {
  direction?: "outbound_request" | "inbound" | "inbound_reply";
  message_id?: string;
  from_agent_id?: string;
  from_display_name?: string;
  to_agent_id?: string;
  to_display_name?: string;
  display_content?: string;
  kind?: string;
  priority?: boolean;
}

function messageMetadata(message: AgentConversationMessage): AgentMessageMetadata | null {
  const value = message.metadata?.agent_messaging;
  if (!value || typeof value !== "object") return null;
  return value as AgentMessageMetadata;
}

function tokenForAgent(agent: AgentDef): string {
  return `@${agent.name.trim().replace(/\s+/g, "_")}`;
}

function eventPayloadText(payload: Record<string, unknown>, key: string): string | undefined {
  const value = payload[key];
  return typeof value === "string" ? value : undefined;
}

function eventPayloadBoolean(payload: Record<string, unknown>, key: string): boolean {
  return payload[key] === true;
}

export function AgentConversationChat({ agent, onOpenSettings }: AgentConversationChatProps) {
  const agents = useAppSelector((state) => state.agents.agents);
  const activeProjectId = useAppSelector((state) => state.project.activeProjectId);
  const [view, setView] = useState<AgentConversationView | null>(null);
  const [input, setInput] = useState("");
  const [selectedMentions, setSelectedMentions] = useState<SelectedAgentMention[]>([]);
  const [priority, setPriority] = useState(false);
  const [loading, setLoading] = useState(true);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const historyRef = useRef<HTMLDivElement>(null);

  const mentionQuery = useMemo(() => {
    const match = input.match(/(?:^|\s)@([^\s]*)$/u);
    return match?.[1]?.toLowerCase() ?? null;
  }, [input]);
  const mentionCandidates = useMemo(() => {
    if (mentionQuery === null) return [];
    return agents
      .filter((candidate) => candidate.id !== agent.id)
      .filter((candidate) =>
        candidate.name.toLowerCase().replace(/\s+/g, "_").startsWith(mentionQuery),
      )
      .slice(0, 6);
  }, [agent.id, agents, mentionQuery]);
  const resolvedMentions = useMemo(
    () => resolveAgentMentions(input, selectedMentions, agents),
    [agents, input, selectedMentions],
  );
  const hasSingleRecipient = resolvedMentions.length === 1;
  const persistedPeerMessageIds = useMemo(
    () =>
      new Set(
        view?.session.messages
          .map((message) => messageMetadata(message)?.message_id)
          .filter((id): id is string => Boolean(id)) ?? [],
      ),
    [view?.session.messages],
  );
  const pendingInboundEvents = useMemo(
    () =>
      view?.messaging.events.filter(
        (event) =>
          event.event_type === "message_received" &&
          Boolean(event.message_id) &&
          !persistedPeerMessageIds.has(event.message_id!),
      ) ?? [],
    [persistedPeerMessageIds, view?.messaging.events],
  );

  const deliveryStatus = useCallback(
    (messageId?: string, completedLabel = "Completed") => {
      if (!messageId || !view) return null;
      const event = [...view.messaging.events]
        .reverse()
        .find(
          (candidate) =>
            candidate.message_id === messageId && candidate.event_type.startsWith("task_"),
        );
      if (!event) return null;
      if (event.event_type === "task_working") return "Working…";
      if (event.event_type === "task_completed") return completedLabel;
      if (event.event_type === "task_failed") {
        return `Failed: ${eventPayloadText(event.payload, "error") || "delivery failed"}`;
      }
      if (event.event_type === "task_needs_attention") {
        return `Needs attention: ${eventPayloadText(event.payload, "error") || "execution was interrupted"}`;
      }
      return null;
    },
    [view],
  );

  const loadConversation = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<AgentConversationView>("open_agent_conversation", {
        agentId: agent.id,
        projectId: activeProjectId ?? "__adhoc_chat__",
      });
      setView(result);
      window.dispatchEvent(new Event("agent-conversations-changed"));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }, [activeProjectId, agent.id]);

  useEffect(() => {
    setView(null);
    setInput("");
    setSelectedMentions([]);
    setPriority(false);
    void loadConversation();
  }, [loadConversation]);

  useEffect(() => {
    if (!hasSingleRecipient) setPriority(false);
  }, [hasSingleRecipient]);

  useEffect(() => {
    if (!view || sending) return;
    let cancelled = false;
    let refreshing = false;
    const refresh = async () => {
      if (refreshing) return;
      refreshing = true;
      try {
        const result = await invoke<AgentConversationView>("open_agent_conversation", {
          agentId: agent.id,
          projectId: activeProjectId ?? "__adhoc_chat__",
        });
        if (cancelled) return;
        setView((current) => {
          if (
            current &&
            current.messaging.next_sequence === result.messaging.next_sequence &&
            current.session.messages.length === result.session.messages.length
          ) {
            return current;
          }
          window.dispatchEvent(new Event("agent-conversations-changed"));
          return result;
        });
      } catch {
        // Sending remains durable even if a transient refresh fails. The next
        // poll (or reopening the conversation) reconstructs the latest view.
      } finally {
        refreshing = false;
      }
    };
    const timer = window.setInterval(() => void refresh(), 1500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeProjectId, agent.id, sending, view?.conversation.id]);

  useEffect(() => {
    historyRef.current?.scrollTo({ top: historyRef.current.scrollHeight, behavior: "smooth" });
  }, [view?.session.messages.length, sending]);

  const insertMention = useCallback((mentioned: AgentDef) => {
    const token = tokenForAgent(mentioned);
    setInput((current) => current.replace(/@[^\s]*$/u, `${token} `));
    setSelectedMentions((current) => {
      if (current.some((item) => item.agent_id === mentioned.id)) return current;
      return [
        ...current,
        {
          agent_id: mentioned.id,
          revision_id: mentioned.updated_at,
          token,
        },
      ];
    });
  }, []);

  const send = useCallback(async () => {
    const text = input.trim();
    if (!text || !view || sending) return;
    const mentions = resolveAgentMentions(text, selectedMentions, agents);
    const sendPriority = priority && mentions.length === 1;
    setSending(true);
    setError(null);
    setInput("");
    setSelectedMentions([]);
    setPriority(false);
    try {
      const result = await invoke<AgentConversationSendResult>(
        "send_agent_conversation_message",
        {
          conversationId: view.conversation.id,
          input: text,
          agentMentions: mentions.length ? mentions : undefined,
          priority: sendPriority,
        },
      );
      setView(result.view);
      window.dispatchEvent(new Event("agent-conversations-changed"));
    } catch (reason) {
      setInput(text);
      setPriority(sendPriority);
      setError(String(reason));
      await loadConversation();
    } finally {
      setSending(false);
    }
  }, [agents, input, loadConversation, priority, selectedMentions, sending, view]);

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.nativeEvent.isComposing) return;
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        void send();
      }
    },
    [send],
  );

  return (
    <div className="agent-conversation">
      <header className="agent-conversation-header">
        <div className="agent-conversation-identity">
          <span className="agent-conversation-avatar">
            <BotIcon size={18} color={agent.color || "var(--accent)"} />
          </span>
          <span>
            <strong>{agent.name}</strong>
            <small>{agent.model || "Default model"}</small>
          </span>
        </div>
        <button className="agent-conversation-settings" onClick={onOpenSettings}>
          <SettingsIcon size={15} /> Settings
        </button>
      </header>

      <div className="agent-conversation-history" ref={historyRef}>
        {loading && (
          <div className="agent-conversation-state">
            <LoaderIcon className="animate-spin" size={18} /> Opening {agent.name}…
          </div>
        )}
        {!loading && view?.session.messages.length === 0 && (
          <div className="agent-conversation-empty">
            <BotIcon size={30} color={agent.color || "var(--accent)"} />
            <h2>Chat with {agent.name}</h2>
            <p>
              This conversation is durable for the current project. Mention another agent to send
              it a message.
            </p>
          </div>
        )}
        {view?.session.messages.map((message, index) => {
          const metadata = messageMetadata(message);
          if (message.role === "tool" || !message.content.trim()) return null;
          if (metadata?.direction === "inbound" || metadata?.direction === "inbound_reply") {
            const status = deliveryStatus(metadata.message_id, "Processed");
            return (
              <div className="agent-peer-message" key={`${index}-${metadata.message_id ?? "peer"}`}>
                <div className="agent-peer-message-label">
                  {metadata.priority ? <ZapIcon size={13} /> : <MessageSquareIcon size={13} />} Message
                  from {metadata.from_display_name}
                </div>
                <div>{metadata.display_content || message.content}</div>
                {status && <div className="agent-delivery-card">{status}</div>}
              </div>
            );
          }
          if (message.role === "user") {
            return (
              <div className="agent-conversation-user-group" key={`${index}-user`}>
                <div className="message-row user-row">
                  <div className="user-msg">{message.content}</div>
                </div>
                {metadata?.direction === "outbound_request" && (
                  <div className="agent-delivery-card">
                    {metadata.priority ? <ZapIcon size={13} /> : <MessageSquareIcon size={13} />}
                    {metadata.priority ? "Priority messaged" : "Messaged"} {metadata.to_display_name}
                    {deliveryStatus(metadata.message_id, "Replied") && (
                      <span> · {deliveryStatus(metadata.message_id, "Replied")}</span>
                    )}
                  </div>
                )}
              </div>
            );
          }
          return (
            <div className="message-row agent-row" key={`${index}-assistant`}>
              <div className="agent-conversation-assistant">
                <AssistantMarkdownContent content={message.content} />
              </div>
            </div>
          );
        })}
        {pendingInboundEvents.map((event) => (
          <div className="agent-peer-message" key={`pending-${event.message_id}`}>
            <div className="agent-peer-message-label">
              {eventPayloadBoolean(event.payload, "priority") ? (
                <ZapIcon size={13} />
              ) : (
                <MessageSquareIcon size={13} />
              )}{" "}
              Message from {eventPayloadText(event.payload, "from")}
            </div>
            <div>{eventPayloadText(event.payload, "display_content") || "Message received"}</div>
            {deliveryStatus(event.message_id, "Processed") && (
              <div className="agent-delivery-card">
                {deliveryStatus(event.message_id, "Processed")}
              </div>
            )}
          </div>
        ))}
        {sending && (
          <div className="agent-conversation-state agent-conversation-working">
            <LoaderIcon className="animate-spin" size={16} /> Agents are working…
          </div>
        )}
      </div>

      <div className="agent-conversation-composer">
        {error && <div className="agent-conversation-error">{error}</div>}
        <div className="agent-conversation-input-wrap">
          {mentionCandidates.length > 0 && (
            <div className="agent-conversation-mentions">
              {mentionCandidates.map((candidate) => (
                <button key={candidate.id} onClick={() => insertMention(candidate)}>
                  <BotIcon size={14} color={candidate.color || "var(--accent)"} />
                  <span>{candidate.name}</span>
                  <small>{candidate.description}</small>
                </button>
              ))}
            </div>
          )}
          <textarea
            value={input}
            onChange={(event) => setInput(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder={`Message ${agent.name}… Type @ to contact another agent`}
            rows={2}
            disabled={loading || sending}
          />
          <button
            className={`agent-conversation-priority${priority ? " active" : ""}`}
            onClick={() => setPriority((current) => !current)}
            disabled={!hasSingleRecipient || loading || sending}
            aria-pressed={priority}
            title={
              hasSingleRecipient
                ? "Interrupt the recipient's current agent task"
                : "Mention one agent to enable priority"
            }
          >
            <ZapIcon size={13} /> Priority
          </button>
          <button
            className="agent-conversation-send"
            onClick={() => void send()}
            disabled={!input.trim() || loading || sending}
            aria-label="Send message"
          >
            {sending ? <LoaderIcon className="animate-spin" size={16} /> : <SendIcon size={16} />}
          </button>
        </div>
      </div>
    </div>
  );
}
