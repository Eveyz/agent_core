// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConversationView, AgentDef } from "../../features/agents/types";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  state: null as unknown,
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../../hooks/useAppDispatch", () => ({
  useAppSelector: (selector: (state: unknown) => unknown) => selector(mocks.state),
}));
vi.mock("../chat/AssistantMarkdownContent", () => ({
  AssistantMarkdownContent: ({ content }: { content: string }) => <div>{content}</div>,
}));

import { AgentConversationChat } from "./AgentConversationChat";

const coder: AgentDef = {
  id: "coder",
  name: "Coder",
  description: "Writes code",
  system_prompt: "",
  model: "test/model",
  skills: [],
  tools: [],
  permission_mode: "standard",
  permission_rules: {},
  max_iterations: 20,
  max_context_tokens: 32_000,
  memory_enabled: 1,
  memory_group: "",
  icon: "",
  color: "#3b82f6",
  created_at: "2026-08-25T00:00:00Z",
  updated_at: "coder-rev",
};

const debuggerAgent: AgentDef = {
  ...coder,
  id: "debugger",
  name: "Debugger",
  description: "Finds root causes",
  color: "#8b5cf6",
  updated_at: "debugger-rev",
};

const reviewerAgent: AgentDef = {
  ...coder,
  id: "reviewer",
  name: "Reviewer",
  description: "Reviews completed work",
  color: "#22c55e",
  updated_at: "reviewer-rev",
};

function conversationView(): AgentConversationView {
  return {
    conversation: {
      id: "conversation-coder",
      agent_id: "coder",
      project_id: "__adhoc_chat__",
      session_id: "session-coder",
      unread_count: 0,
      created_at: "2026-08-25T00:00:00Z",
      updated_at: "2026-08-25T00:00:00Z",
    },
    session: {
      meta: { id: "session-coder", title: "Coder", model_used: "test/model" },
      messages: [
        {
          role: "user",
          content: "peer envelope",
          metadata: {
            agent_messaging: {
              direction: "inbound_reply",
              message_id: "reply-1",
              from_agent_id: "debugger",
              from_display_name: "Debugger",
              display_content: "The crash comes from a stale task lease.",
            },
          },
        },
        { role: "assistant", content: "Debugger found a stale task lease." },
      ],
    },
    messaging: {
      next_sequence: 3,
      events: [
        {
          sequence: 1,
          conversation_id: "conversation-coder",
          event_type: "message_received",
          message_id: "reply-1",
          task_id: "task-1",
          payload: { from: "Debugger", kind: "reply" },
          created_at: "2026-08-25T00:00:00Z",
        },
        {
          sequence: 2,
          conversation_id: "conversation-coder",
          event_type: "task_completed",
          message_id: "reply-1",
          task_id: "task-1",
          payload: {},
          created_at: "2026-08-25T00:00:01Z",
        },
      ],
    },
    swarm: {
      run: {
        id: "swarm-1",
        goal: "Fix the crash",
        status: "running",
        max_messages: 8,
        messages_used: 2,
        max_turns: 8,
        turns_used: 2,
        max_hops: 4,
        hops_used: 2,
        summary: "",
        error: "",
      },
      participant_agent_ids: ["coder", "debugger"],
      messages: [{ id: "reply-1" }],
    },
  };
}

describe("AgentConversationChat contact experience", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    mocks.state = {
      agents: { agents: [coder, debuggerAgent, reviewerAgent] },
      project: { activeProjectId: "__adhoc_chat__" },
    };
    mocks.invoke.mockReset();
    mocks.invoke.mockResolvedValue(conversationView());
    Object.defineProperty(HTMLElement.prototype, "scrollTo", {
      configurable: true,
      value: vi.fn(),
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("renders durable peer replies and the active swarm context", async () => {
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    expect(container.textContent).toContain("Agent Swarm · Active");
    expect(container.textContent).toContain("Reply from Debugger");
    expect(container.textContent).toContain("The crash comes from a stale task lease.");
    expect(container.textContent).toContain("Debugger found a stale task lease.");
  });

  it("turns a contact shortcut into a structured mention chip", async () => {
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    const shortcut = container.querySelector(
      'button[title="Mention Debugger"]',
    ) as HTMLButtonElement;
    await act(async () => shortcut.click());

    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    expect(textarea.value).toBe("@Debugger ");
    expect(container.textContent).toContain("Coordinate with");
    expect(container.querySelector('[aria-label="Remove Debugger"]')).not.toBeNull();

    const replacement = container.querySelector(
      'button[title="Mention Reviewer"]',
    ) as HTMLButtonElement;
    await act(async () => replacement.click());
    expect(textarea.value).toBe("@Reviewer ");
    expect(container.querySelector('[aria-label="Remove Debugger"]')).toBeNull();
    expect(container.querySelector('[aria-label="Remove Reviewer"]')).not.toBeNull();
  });

  it("blocks manually typed multi-recipient sends before invoking Tauri", async () => {
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    await act(async () => {
      const valueSetter = Object.getOwnPropertyDescriptor(
        HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      valueSetter?.call(textarea, "@Debugger ask @Reviewer verify");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    });

    expect(container.textContent).toContain("Choose one recipient per message.");
    expect((container.querySelector('[aria-label="Send message"]') as HTMLButtonElement).disabled)
      .toBe(true);
  });
});
