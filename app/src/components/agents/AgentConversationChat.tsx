import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
  AgentConversationApproval,
  AgentConversationApprovalRequired,
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
import { AgentTurnUI } from "../chat/AgentTurn";
import { AssistantMarkdownContent } from "../chat/AssistantMarkdownContent";
import { ElapsedTime } from "../chat/ProcessingTimer";
import type { ChatEntry } from "../../features/chat/types";
import {
  applyConversationAgentEvent,
  createLiveTurn,
  groupConversationItems,
  liveTurnEntry,
  messageMetadata,
  placeOutboundReplyReceipts,
  type LiveConversationTurn,
} from "./conversationTurns";
import { stripContextStatus } from "../../utils/chatUtils";
import "./AgentConversationChat.css";

interface AgentConversationChatProps {
  agent: AgentDef;
  onOpenSettings: () => void;
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

function AgentTurnFrame({
  name,
  working,
  entry,
}: {
  name: string;
  working: boolean;
  entry: ChatEntry;
}) {
  return (
    <div className="agent-assistant-turn">
      <div className={`agent-turn-avatar${working ? " working" : ""}`}>
        <BotIcon size={14} />
        {working && <span className="agent-presence-dot working" />}
      </div>
      <div className="agent-turn-content">
        <div className="agent-turn-meta">
          <div className="agent-turn-author">{name}</div>
          {working && (
            <div className="agent-turn-status" data-testid="agent-turn-status">
              Working
              {entry.startTime ? (
                <>
                  <span aria-hidden="true"> · </span>
                  <ElapsedTime startTime={entry.startTime} endTime={entry.endTime} />
                </>
              ) : null}
            </div>
          )}
        </div>
        <AgentTurnUI entry={entry} variant="conversation" />
      </div>
    </div>
  );
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
        <div className="agent-peer-message-content">
          <AssistantMarkdownContent
            className="assistant-msg agent-peer-markdown"
            content={stripContextStatus(content)}
          />
        </div>
        {status && <div className="agent-protocol-status">{status}</div>}
      </div>
    </div>
  );
}

interface ReplyReceiptProps {
  fromName: string;
  toName?: string;
  status?: string | null;
}

function ReplyReceipt({ fromName, toName, status }: ReplyReceiptProps) {
  return (
    <div className="agent-reply-receipt-row">
      <div className="agent-reply-receipt">
        <CheckCircleIcon size={13} />
        <span>
          {fromName} replied to {toName || "the requesting agent"}
        </span>
        {status && <small>· {status}</small>}
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
  const [pendingUserMessage, setPendingUserMessage] = useState<string | null>(null);
  const [pendingApprovals, setPendingApprovals] = useState<AgentConversationApprovalRequired[]>([]);
  const [resolvingApprovals, setResolvingApprovals] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [liveTurn, setLiveTurn] = useState<LiveConversationTurn | null>(null);
  const historyRef = useRef<HTMLDivElement>(null);
  const conversationIdRef = useRef<string | null>(null);
  const loadGenerationRef = useRef(0);
  const liveStartedMessageCountRef = useRef(0);

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
  const hasWorkingPeerTask = useMemo(() => {
    const seenTasks = new Set<string>();
    for (const event of [...(view?.messaging.events ?? [])].reverse()) {
      if (!isTaskEvent(event) || !event.task_id || seenTasks.has(event.task_id)) continue;
      seenTasks.add(event.task_id);
      if (event.event_type === "task_working") return true;
    }
    return false;
  }, [view?.messaging.events]);
  const agentIsWorking = sending || hasWorkingPeerTask || Boolean(liveTurn && !liveTurn.endTime);

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
    const generation = ++loadGenerationRef.current;
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<AgentConversationView>("open_agent_conversation", {
        agentId: agent.id,
        projectId: activeProjectId ?? "__adhoc_chat__",
      });
      if (generation !== loadGenerationRef.current) return;
      conversationIdRef.current = result.conversation.id;
      setView(result);
      setPendingApprovals(result.approvals ?? []);
      window.dispatchEvent(new Event("agent-conversations-changed"));
    } catch (reason) {
      if (generation !== loadGenerationRef.current) return;
      setError(String(reason));
    } finally {
      if (generation === loadGenerationRef.current) setLoading(false);
    }
  }, [activeProjectId, agent.id]);

  useEffect(() => {
    setView(null);
    setInput("");
    setSelectedMentions([]);
    setPriority(false);
    setSending(false);
    setPendingUserMessage(null);
    setPendingApprovals([]);
    setResolvingApprovals({});
    setLiveTurn(null);
    liveStartedMessageCountRef.current = 0;
    conversationIdRef.current = null;
    void loadConversation();
    return () => {
      loadGenerationRef.current += 1;
    };
  }, [loadConversation]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let mounted = true;
    void listen<AgentConversationApproval>("agent-conversation-approval", (event) => {
      if (
        !mounted ||
        event.payload.agent_id !== agent.id ||
        event.payload.conversation_id !== conversationIdRef.current
      ) return;
      setPendingApprovals((current) => {
        if (event.payload.event_type === "resolved") {
          return current.filter((approval) => approval.prompt_id !== event.payload.prompt_id);
        }
        return event.payload.event_type === "required" ? [
          ...current.filter((approval) => approval.prompt_id !== event.payload.prompt_id),
          event.payload,
        ] : current;
      });
    }).then((dispose) => {
      if (mounted) unlisten = dispose;
      else dispose();
    });
    return () => {
      mounted = false;
      unlisten?.();
    };
  }, [agent.id]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let mounted = true;
    void listen<{
      conversation_id: string;
      agent_id: string;
      turn_id: string;
      event: unknown;
    }>("agent-conversation-event", (event) => {
      if (
        !mounted ||
        event.payload.agent_id !== agent.id ||
        event.payload.conversation_id !== conversationIdRef.current
      ) return;
      setLiveTurn((current) => applyConversationAgentEvent(
        current ?? createLiveTurn(event.payload.turn_id),
        event.payload.event,
      ));
    }).then((dispose) => {
      if (mounted) unlisten = dispose;
      else dispose();
    });
    return () => {
      mounted = false;
      unlisten?.();
    };
  }, [agent.id]);

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
        conversationIdRef.current = result.conversation.id;
        setPendingApprovals(result.approvals ?? []);
        if (result.session.messages.length > liveStartedMessageCountRef.current) {
          setLiveTurn(null);
        }
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
    if ((sending || hasWorkingPeerTask) && !liveTurn) {
      liveStartedMessageCountRef.current = view?.session.messages.length ?? 0;
      setLiveTurn(createLiveTurn("pending"));
    }
  }, [hasWorkingPeerTask, liveTurn, sending, view?.session.messages.length]);

  useEffect(() => {
    historyRef.current?.scrollTo({ top: historyRef.current.scrollHeight, behavior: "smooth" });
  }, [liveTurn, sending, view?.pending_messages?.length, view?.session.messages.length]);

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
        ?.map((agentId) => agentsById.get(agentId))
        .filter((candidate): candidate is AgentDef => Boolean(candidate)) ?? [],
    [agentsById, view?.swarm?.participant_agent_ids],
  );

  const swarmCanCancel =
    view?.swarm?.run.status === "running" || view?.swarm?.run.status === "completing";
  const showStop = swarmCanCancel && !sending && !input.trim();

  const conversationItems = useMemo(
    () => groupConversationItems(view?.session.messages ?? []),
    [view?.session.messages],
  );
  const placedReplies = useMemo(
    () => placeOutboundReplyReceipts(conversationItems, outboundReplyEvents),
    [conversationItems, outboundReplyEvents],
  );
  const showLiveTurn = liveTurn != null;
  const liveReplyReceipts = showLiveTurn ? placedReplies.leftover : [];
  const historyLeftoverReceipts = showLiveTurn ? [] : placedReplies.leftover;

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
    setPendingUserMessage(text);
    liveStartedMessageCountRef.current = view.session.messages.length;
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
      setPendingUserMessage(null);
      setLiveTurn(null);
      window.dispatchEvent(new Event("agent-conversations-changed"));
    } catch (reason) {
      setPendingUserMessage(null);
      setLiveTurn(null);
      setInput(text);
      setPriority(sendPriority);
      setError(String(reason));
      await loadConversation();
    } finally {
      setSending(false);
    }
  }, [agents, input, loadConversation, priority, selectedMentions, sending, view]);

  const respondToApproval = useCallback(async (
    approval: AgentConversationApprovalRequired,
    choice: string,
  ) => {
    const approvalKey = `${approval.turn_id}:${approval.prompt_id}`;
    setResolvingApprovals((current) => ({ ...current, [approvalKey]: choice }));
    try {
      await invoke("approve_agent_conversation_tool", {
        turnId: approval.turn_id,
        promptId: approval.prompt_id,
        choice,
      });
      setPendingApprovals((current) =>
        current.filter((candidate) => candidate.prompt_id !== approval.prompt_id),
      );
    } catch (reason) {
      setResolvingApprovals((current) => {
        const next = { ...current };
        delete next[approvalKey];
        return next;
      });
      setError(String(reason));
    }
  }, []);

  const cancelSwarm = useCallback(async () => {
    if (
      !view?.swarm ||
      (view.swarm.run.status !== "running"
        && view.swarm.run.status !== "completing"
        && view.swarm.run.status !== "cancelling")
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
          <span className={`agent-conversation-avatar${agentIsWorking ? " working" : ""}`}>
            <BotIcon size={18} />
            <span className={`agent-presence-dot${agentIsWorking ? " working" : ""}`} />
          </span>
          <span>
            <strong>{agent.name}</strong>
            <small>
              <span className={`agent-contact-status${agentIsWorking ? " working" : ""}`}>
                {agentIsWorking ? "Working" : "Ready"}
              </span>
              <span aria-hidden="true"> · </span>
              {agent.model || "Default model"}
            </small>
          </span>
        </div>
        <div className="agent-conversation-header-actions">
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

      <div className="agent-conversation-history" ref={historyRef}>
        {loading && (
          <div className="agent-conversation-state">
            <LoaderIcon className="animate-spin" size={18} /> Opening {agent.name}…
          </div>
        )}
        {!loading && view?.session.messages.length === 0 && !(view.pending_messages?.length) && !pendingUserMessage && (
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
        {conversationItems.map((item) => {
          if (item.type === "peer") {
            const metadata = messageMetadata(item.message);
            const status = deliveryStatus(metadata?.message_id, "Processed");
            const sender = metadata?.from_agent_id ? agentsById.get(metadata.from_agent_id) : undefined;
            return (
              <PeerMessageCard
                key={item.key}
                senderName={metadata?.from_display_name || sender?.name || "Agent"}
                senderColor={sender?.color}
                content={metadata?.display_content || item.message.content}
                kind={metadata?.direction === "inbound_reply" ? "reply" : "message"}
                priority={Boolean(metadata?.priority)}
                status={status}
              />
            );
          }
          if (item.type === "user") {
            const metadata = messageMetadata(item.message);
            const status = metadata?.direction === "outbound_request"
              ? deliveryStatus(metadata.message_id, "Replied")
              : null;
            return (
              <div className="agent-conversation-user-group" key={item.key}>
                <div className="message-row user-row">
                  <div className="user-msg">{item.message.content}</div>
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
          const receipts = placedReplies.byTurnKey.get(item.key) ?? [];
          return (
            <div key={item.key} className="agent-turn-with-receipts">
              <AgentTurnFrame
                name={agent.name}
                working={false}
                entry={item.entry}
              />
              {receipts.map((event) => (
                <ReplyReceipt
                  key={`reply-${event.message_id ?? event.payload.to}`}
                  fromName={agent.name}
                  toName={event.payload.to}
                  status={deliveryStatus(event.message_id, "Delivered")}
                />
              ))}
            </div>
          );
        })}
        {historyLeftoverReceipts.map((event) => (
          <ReplyReceipt
            key={`reply-leftover-${event.message_id ?? event.payload.to}`}
            fromName={agent.name}
            toName={event.payload.to}
            status={deliveryStatus(event.message_id, "Delivered")}
          />
        ))}
        {pendingUserMessage && (
          <div className="agent-conversation-user-group pending" data-testid="pending-user-message">
            <div className="message-row user-row">
              <div className="user-msg">{pendingUserMessage}</div>
            </div>
            <div className="agent-pending-message-status">
              <LoaderIcon className="animate-spin" size={12} /> Queued · waiting for {agent.name}
            </div>
          </div>
        )}
        {view?.pending_messages
          ?.filter((message) => message.content !== pendingUserMessage)
          .map((message) => (
            <div
              className="agent-conversation-user-group pending"
              data-testid="pending-user-message"
              key={message.turn_id}
            >
              <div className="message-row user-row">
                <div className="user-msg">{message.content}</div>
              </div>
              <div className="agent-pending-message-status">
                <LoaderIcon className="animate-spin" size={12} /> Queued · waiting for {agent.name}
              </div>
            </div>
          ))}
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
        {showLiveTurn && liveTurn && (
          <div className="agent-turn-with-receipts">
            <AgentTurnFrame
              name={agent.name}
              working
              entry={liveTurnEntry(liveTurn)}
            />
            {liveReplyReceipts.map((event) => (
              <ReplyReceipt
                key={`reply-live-${event.message_id ?? event.payload.to}`}
                fromName={agent.name}
                toName={event.payload.to}
                status={deliveryStatus(event.message_id, "Delivered")}
              />
            ))}
          </div>
        )}
        {pendingApprovals.map((approval) => {
          const approvalKey = `${approval.turn_id}:${approval.prompt_id}`;
          const resolvingChoice = resolvingApprovals[approvalKey];
          return (
          <div className="agent-approval-card" key={approvalKey} aria-busy={Boolean(resolvingChoice)}>
            <div className="agent-approval-header">
              <strong>Approval Required: {approval.tool_name || "tool"}</strong>
              {approval.danger_level && (
                <span className={`danger-badge danger-${approval.danger_level}`}>
                  {approval.danger_level}
                </span>
              )}
            </div>
            <p>{approval.explanation || `${agent.name} wants to use a tool.`}</p>
            <pre>{typeof approval.tool_input === "string"
              ? approval.tool_input
              : JSON.stringify(approval.tool_input ?? {}, null, 2)}</pre>
            <div className="agent-approval-actions">
              <button className="btn-deny" disabled={Boolean(resolvingChoice)} aria-pressed={resolvingChoice === "deny"} onClick={() => void respondToApproval(approval, "deny")}>Deny Once</button>
              <button className="btn-allow" disabled={Boolean(resolvingChoice)} aria-pressed={resolvingChoice === "allow_once"} onClick={() => void respondToApproval(approval, "allow_once")}>Allow Once</button>
              <button className="btn-allow" disabled={Boolean(resolvingChoice)} aria-pressed={resolvingChoice === "allow_session"} onClick={() => void respondToApproval(approval, "allow_session")}>Allow for this run</button>
            </div>
            {resolvingChoice && <small className="agent-approval-resolving">Applying your choice…</small>}
          </div>
          );
        })}
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
                className={`agent-conversation-send${showStop ? " stop" : ""}`}
                onClick={() => showStop ? void cancelSwarm() : void send()}
                disabled={
                  showStop
                    ? loading
                    : !input.trim() || loading || sending || hasTooManyRecipients
                }
                aria-label={showStop ? "Stop swarm" : "Send message"}
              >
                {sending
                  ? <LoaderIcon className="animate-spin" size={15} />
                  : showStop
                    ? <StopCircleIcon size={15} />
                    : <SendIcon size={15} />}
              </button>
            </div>
          </div>
        </div>
        {view?.swarm && (
          <div className="agent-swarm-metrics" aria-label="Swarm usage">
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
          </div>
        )}
      </div>
    </div>
  );
}
