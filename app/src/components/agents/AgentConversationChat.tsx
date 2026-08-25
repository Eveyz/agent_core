import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import BotIcon from "lucide-react/dist/esm/icons/bot.mjs";
import SettingsIcon from "lucide-react/dist/esm/icons/settings.mjs";
import SendIcon from "lucide-react/dist/esm/icons/send.mjs";
import LoaderIcon from "lucide-react/dist/esm/icons/loader.mjs";
import MessageSquareIcon from "lucide-react/dist/esm/icons/message-square.mjs";
import ZapIcon from "lucide-react/dist/esm/icons/zap.mjs";
import NetworkIcon from "lucide-react/dist/esm/icons/network.mjs";
import UsersIcon from "lucide-react/dist/esm/icons/users.mjs";
import AlertTriangleIcon from "lucide-react/dist/esm/icons/alert-triangle.mjs";
import CheckCircleIcon from "lucide-react/dist/esm/icons/circle-check.mjs";
import StopCircleIcon from "lucide-react/dist/esm/icons/circle-stop.mjs";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";
import SparklesIcon from "lucide-react/dist/esm/icons/sparkles.mjs";
import { useAppSelector } from "../../hooks/useAppDispatch";
import type {
  AgentConversationMessage,
  AgentConversationSendResult,
  AgentConversationView,
  AgentDef,
  AgentMessageEvent,
} from "../../features/agents/types";
import {
  insertAgentMentionToken,
  resolveAgentMentions,
  type SelectedAgentMention,
} from "../chat/agentMentions";
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

type AgentTaskEvent = Extract<AgentMessageEvent, { event_type: `task_${string}` }>;
type AgentMessageReceivedEvent = Extract<AgentMessageEvent, { event_type: "message_received" }>;
type AgentMessageSentEvent = Extract<AgentMessageEvent, { event_type: "message_sent" }>;

function isTaskEvent(event: AgentMessageEvent): event is AgentTaskEvent {
  return event.event_type.startsWith("task_");
}

function isMessageReceivedEvent(
  event: AgentMessageEvent,
): event is AgentMessageReceivedEvent {
  return event.event_type === "message_received";
}

function isMessageSentEvent(event: AgentMessageEvent): event is AgentMessageSentEvent {
  return event.event_type === "message_sent";
}

interface PeerMessageCardProps {
  senderName: string;
  senderColor?: string;
  content: string;
  kind: "message" | "reply";
  priority: boolean;
  status?: string | null;
  pending?: boolean;
}

function PeerMessageCard({
  senderName,
  senderColor,
  content,
  kind,
  priority,
  status,
  pending = false,
}: PeerMessageCardProps) {
  return (
    <div className={`agent-protocol-event${pending ? " pending" : ""}`}>
      <div
        className="agent-protocol-avatar"
        style={{ "--peer-color": senderColor || "var(--accent)" } as React.CSSProperties}
      >
        <BotIcon size={14} />
      </div>
      <div className="agent-protocol-body">
        <div className="agent-peer-message-label">
          <span>{kind === "reply" ? "Reply" : "Message"} from {senderName}</span>
          {priority && <span className="agent-protocol-priority"><ZapIcon size={11} /> Priority</span>}
        </div>
        <div className="agent-peer-message-content">{content}</div>
        {status && <div className="agent-protocol-status">{status}</div>}
      </div>
    </div>
  );
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
  const otherAgents = useMemo(
    () => agents.filter((candidate) => candidate.id !== agent.id),
    [agent.id, agents],
  );
  const agentsById = useMemo(
    () => new Map(agents.map((candidate) => [candidate.id, candidate])),
    [agents],
  );
  const agentsByName = useMemo(
    () => new Map(agents.map((candidate) => [candidate.name, candidate])),
    [agents],
  );
  const resolvedMentions = useMemo(
    () =>
      resolveAgentMentions(
        input,
        selectedMentions,
        agents.filter((candidate) => candidate.id !== agent.id),
      ),
    [agent.id, agents, input, selectedMentions],
  );
  const hasSingleRecipient = resolvedMentions.length === 1;
  const hasTooManyRecipients = resolvedMentions.length > 1;
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
        (event): event is AgentMessageReceivedEvent =>
          isMessageReceivedEvent(event) &&
          Boolean(event.message_id) &&
          !persistedPeerMessageIds.has(event.message_id!),
      ) ?? [],
    [persistedPeerMessageIds, view?.messaging.events],
  );
  const outboundReplyEvents = useMemo(
    () =>
      view?.messaging.events.filter(
        (event): event is AgentMessageSentEvent =>
          isMessageSentEvent(event) &&
          event.payload.kind === "reply",
      ) ?? [],
    [view?.messaging.events],
  );

  const deliveryStatus = useCallback(
    (messageId?: string, completedLabel = "Completed") => {
      if (!messageId || !view) return null;
      const event = [...view.messaging.events]
        .reverse()
        .find(
          (candidate) =>
            candidate.message_id === messageId && isTaskEvent(candidate),
        );
      if (!event || !isTaskEvent(event)) return null;
      if (event.event_type === "task_working") return "Working…";
      if (event.event_type === "task_completed") return completedLabel;
      if (event.event_type === "task_failed") {
        return `Failed: ${event.payload.error || "delivery failed"}`;
      }
      if (event.event_type === "task_needs_attention") {
        return `Needs attention: ${event.payload.error || "execution was interrupted"}`;
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
    const token = `@${mentioned.name.trim().replace(/\s+/g, "_")}`;
    setInput((current) => {
      const withoutPreviousRecipient = selectedMentions.reduce(
        (value, mention) => value.replace(mention.token, ""),
        current,
      );
      return insertAgentMentionToken(
        withoutPreviousRecipient.replace(/\s{2,}/g, " ").trimStart(),
        mentioned,
      );
    });
    setSelectedMentions([
      {
        agent_id: mentioned.id,
        revision_id: mentioned.updated_at,
        token,
      },
    ]);
    setError(null);
  }, [selectedMentions]);

  const removeMention = useCallback((mentioned: SelectedAgentMention) => {
    setSelectedMentions((current) =>
      current.filter((item) => item.agent_id !== mentioned.agent_id),
    );
    setInput((current) => current.replace(mentioned.token, "").replace(/\s{2,}/g, " ").trimStart());
  }, []);

  const participantAgents = useMemo(
    () =>
      view?.swarm?.participant_agent_ids
        .map((agentId) => agentsById.get(agentId))
        .filter((candidate): candidate is AgentDef => Boolean(candidate)) ?? [],
    [agentsById, view?.swarm?.participant_agent_ids],
  );

  const swarmStatus = view?.swarm?.run.status;
  const swarmStatusLabel = swarmStatus
    ? {
        running: "Active",
        completing: "Finishing",
        completed: "Completed",
        cancelled: "Cancelled",
        needs_attention: "Needs attention",
      }[swarmStatus]
    : null;

  const send = useCallback(async () => {
    const text = input.trim();
    if (!text || !view || sending) return;
    const mentions = resolveAgentMentions(text, selectedMentions, agents);
    if (mentions.length > 1) {
      setError("Mention one agent at a time. That agent can continue the coordination chain.");
      return;
    }
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

  const cancelSwarm = useCallback(async () => {
    if (
      !view?.swarm ||
      (view.swarm.run.status !== "running" && view.swarm.run.status !== "completing")
    ) return;
    try {
      const swarm = await invoke<NonNullable<AgentConversationView["swarm"]>>(
        "command_agent_swarm",
        { runId: view.swarm.run.id, command: { type: "cancel", reason: "Cancelled by user" } },
      );
      setView((current) => (current ? { ...current, swarm } : current));
    } catch (reason) {
      setError(String(reason));
    }
  }, [view]);

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
    <div
      className="agent-conversation"
      style={{ "--current-agent-color": agent.color || "var(--accent)" } as React.CSSProperties}
    >
      <header className="agent-conversation-header">
        <div className="agent-conversation-identity">
          <span className="agent-conversation-avatar">
            <BotIcon size={18} />
            <span className={`agent-presence-dot${sending ? " working" : ""}`} />
          </span>
          <span>
            <strong>{agent.name}</strong>
            <small>
              <span className="agent-contact-status">{sending ? "Working" : "Ready"}</span>
              <span aria-hidden="true"> · </span>
              {agent.model || "Default model"}
            </small>
          </span>
        </div>
        <div className="agent-conversation-header-actions">
          {view?.swarm && (
            <span className={`agent-header-swarm-pill ${view.swarm.run.status}`}>
              <NetworkIcon size={13} /> {swarmStatusLabel}
            </span>
          )}
          <button
            className="agent-conversation-settings"
            onClick={onOpenSettings}
            title={`Configure ${agent.name}`}
          >
            <SettingsIcon size={16} />
            <span>Settings</span>
          </button>
        </div>
      </header>

      {view?.swarm && (
        <section className={`agent-swarm-strip ${view.swarm.run.status}`} aria-label="Swarm status">
          <div className="agent-swarm-summary">
            <span className="agent-swarm-icon"><NetworkIcon size={15} /></span>
            <span className="agent-swarm-copy">
              <strong>Agent Swarm · {swarmStatusLabel}</strong>
              <small>{view.swarm.run.goal}</small>
            </span>
          </div>
          <div className="agent-swarm-participants" title={`${participantAgents.length} participants`}>
            {participantAgents.slice(0, 5).map((participant) => (
              <span
                key={participant.id}
                className="agent-swarm-participant"
                style={{ "--participant-color": participant.color || "var(--accent)" } as React.CSSProperties}
                title={participant.name}
              >
                <BotIcon size={12} />
              </span>
            ))}
            <span className="agent-swarm-participant-count">
              <UsersIcon size={12} /> {view.swarm.participant_agent_ids.length}
            </span>
          </div>
          <div className="agent-swarm-budgets">
            <span title="Messages used"><b>{view.swarm.run.messages_used}</b>/{view.swarm.run.max_messages} msg</span>
            <span title="Turns used"><b>{view.swarm.run.turns_used}</b>/{view.swarm.run.max_turns} turns</span>
            <span title="Maximum hop reached"><b>{view.swarm.run.hops_used}</b>/{view.swarm.run.max_hops} hops</span>
          </div>
          {view.swarm.run.error && (
            <span className="agent-swarm-error"><AlertTriangleIcon size={13} /> {view.swarm.run.error}</span>
          )}
          {(view.swarm.run.status === "running" || view.swarm.run.status === "completing") && (
            <button className="agent-swarm-cancel" onClick={() => void cancelSwarm()}>
              <StopCircleIcon size={13} /> Stop
            </button>
          )}
        </section>
      )}

      <div className="agent-conversation-history" ref={historyRef}>
        {loading && (
          <div className="agent-conversation-state">
            <LoaderIcon className="animate-spin" size={18} /> Opening {agent.name}…
          </div>
        )}
        {!loading && view?.session.messages.length === 0 && (
          <div className="agent-conversation-empty">
            <span className="agent-conversation-empty-avatar">
              <BotIcon size={26} />
              <span />
            </span>
            <h2>Message {agent.name}</h2>
            <p>
              Work directly with this agent, or bring teammates into the conversation with an @mention.
              Agent-to-agent messages and replies will appear here automatically.
            </p>
            <div className="agent-starter-grid">
              <button onClick={() => setInput("Review the current project and suggest the highest-impact next step.")}>
                <SparklesIcon size={14} /> Review this project
              </button>
              <button onClick={() => setInput("Investigate the current task and report what you find.")}>
                <MessageSquareIcon size={14} /> Start an investigation
              </button>
              {otherAgents[0] && (
                <button onClick={() => insertMention(otherAgents[0])}>
                  <NetworkIcon size={14} /> Coordinate with {otherAgents[0].name}
                </button>
              )}
            </div>
          </div>
        )}
        {view?.session.messages.map((message, index) => {
          const metadata = messageMetadata(message);
          if (message.role === "tool" || !message.content.trim()) return null;
          if (metadata?.direction === "inbound" || metadata?.direction === "inbound_reply") {
            const status = deliveryStatus(metadata.message_id, "Processed");
            const sender = metadata.from_agent_id ? agentsById.get(metadata.from_agent_id) : undefined;
            return (
              <PeerMessageCard
                key={`${index}-${metadata.message_id ?? "peer"}`}
                senderName={metadata.from_display_name || sender?.name || "Agent"}
                senderColor={sender?.color}
                content={metadata.display_content || message.content}
                kind={metadata.direction === "inbound_reply" ? "reply" : "message"}
                priority={Boolean(metadata.priority)}
                status={status}
              />
            );
          }
          if (message.role === "user") {
            const status = metadata?.direction === "outbound_request"
              ? deliveryStatus(metadata.message_id, "Replied")
              : null;
            return (
              <div className="agent-conversation-user-group" key={`${index}-user`}>
                <div className="message-row user-row">
                  <div className="user-msg">{message.content}</div>
                </div>
                {metadata?.direction === "outbound_request" && (
                  <div className={`agent-delivery-card${status?.startsWith("Failed") || status?.startsWith("Needs") ? " error" : ""}`}>
                    {metadata.priority ? <ZapIcon size={12} /> : <CheckCircleIcon size={12} />}
                    <span>{metadata.priority ? "Priority messaged" : "Messaged"} {metadata.to_display_name}</span>
                    {status && <span>· {status}</span>}
                  </div>
                )}
              </div>
            );
          }
          return (
            <div className="agent-assistant-turn" key={`${index}-assistant`}>
              <div className="agent-turn-avatar"><BotIcon size={14} /></div>
              <div className="agent-turn-content">
                <div className="agent-turn-author">{agent.name}</div>
                <div className="agent-conversation-assistant">
                  <AssistantMarkdownContent content={message.content} />
                </div>
              </div>
            </div>
          );
        })}
        {pendingInboundEvents.map((event) => {
          const status = deliveryStatus(event.message_id, "Processed");
          const sender = event.payload.from ? agentsByName.get(event.payload.from) : undefined;
          return (
            <PeerMessageCard
              key={`pending-${event.message_id}`}
              senderName={event.payload.from || "Agent"}
              senderColor={sender?.color}
              content={event.payload.display_content || "Message received"}
              kind={event.payload.kind === "reply" ? "reply" : "message"}
              priority={Boolean(event.payload.priority)}
              status={status || "Queued for this agent"}
              pending
            />
          );
        })}
        {outboundReplyEvents.map((event) => (
          <div className="agent-reply-receipt" key={`reply-${event.message_id}`}>
            <CheckCircleIcon size={13} />
            <span>
              {agent.name} replied to {event.payload.to || "the requesting agent"}
            </span>
            {deliveryStatus(event.message_id, "Delivered") && (
              <small>· {deliveryStatus(event.message_id, "Delivered")}</small>
            )}
          </div>
        ))}
        {sending && (
          <div className="agent-thinking-row">
            <div className="agent-turn-avatar"><LoaderIcon className="animate-spin" size={14} /></div>
            <div>
              <strong>{resolvedMentions.length > 0 ? "Coordinating agents" : `${agent.name} is working`}</strong>
              <small>{resolvedMentions.length > 0 ? "Messages and replies will appear as they arrive" : "Thinking and using tools…"}</small>
            </div>
          </div>
        )}
      </div>

      <div className="agent-conversation-composer">
        {error && <div className="agent-conversation-error"><AlertTriangleIcon size={13} /> {error}</div>}
        {otherAgents.length > 0 && (
          <div className="agent-coordinate-bar">
            <span><NetworkIcon size={12} /> Coordinate with</span>
            <div className="agent-coordinate-list">
              {otherAgents.slice(0, 6).map((candidate) => (
                <button
                  key={candidate.id}
                  onClick={() => insertMention(candidate)}
                  disabled={selectedMentions.some((mention) => mention.agent_id === candidate.id)}
                  style={{ "--mention-color": candidate.color || "var(--accent)" } as React.CSSProperties}
                  title={`Mention ${candidate.name}`}
                >
                  <BotIcon size={11} /> {candidate.name}
                </button>
              ))}
            </div>
          </div>
        )}
        <div className="agent-conversation-input-wrap">
          {mentionCandidates.length > 0 && (
            <div className="agent-conversation-mentions">
              <div className="agent-mentions-heading">Coordinate with another agent</div>
              {mentionCandidates.map((candidate) => (
                <button key={candidate.id} onClick={() => insertMention(candidate)}>
                  <span
                    className="agent-mention-avatar"
                    style={{ "--mention-color": candidate.color || "var(--accent)" } as React.CSSProperties}
                  ><BotIcon size={13} /></span>
                  <span>{candidate.name}</span>
                  <small>{candidate.description || "Custom agent"}</small>
                </button>
              ))}
            </div>
          )}
          {selectedMentions.length > 0 && (
            <div className="agent-selected-mentions">
              {selectedMentions.map((mention) => {
                const mentionedAgent = agentsById.get(mention.agent_id);
                return (
                  <span key={mention.agent_id}>
                    <BotIcon size={11} /> {mentionedAgent?.name || mention.token.slice(1)}
                    <button onClick={() => removeMention(mention)} aria-label={`Remove ${mentionedAgent?.name || mention.token}`}>
                      <XIcon size={11} />
                    </button>
                  </span>
                );
              })}
            </div>
          )}
          {hasTooManyRecipients && (
            <div className="agent-recipient-warning">
              <AlertTriangleIcon size={12} /> Choose one recipient per message.
            </div>
          )}
          <textarea
            value={input}
            onChange={(event) => setInput(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder={`Message ${agent.name}… Use @ to involve another agent`}
            rows={2}
            disabled={loading || sending}
          />
          <div className="agent-composer-footer">
            <span>Enter to send · Shift + Enter for new line</span>
            <div className="agent-composer-actions">
              <button
                className={`agent-conversation-priority${priority ? " active" : ""}`}
                onClick={() => setPriority((current) => !current)}
                disabled={!hasSingleRecipient || loading || sending}
                aria-pressed={priority}
                title={hasSingleRecipient ? "Prioritize the collaborator handoff" : "Mention one collaborator to enable priority"}
              >
                <ZapIcon size={12} /> Priority
              </button>
              <button
                className="agent-conversation-send"
                onClick={() => void send()}
                disabled={!input.trim() || loading || sending || hasTooManyRecipients}
                aria-label="Send message"
              >
                {sending ? <LoaderIcon className="animate-spin" size={15} /> : <SendIcon size={15} />}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
