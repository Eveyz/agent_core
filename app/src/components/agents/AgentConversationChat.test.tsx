// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConversationView, AgentDef } from "../../features/agents/types";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  state: null as unknown,
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, callback: (event: { payload: unknown }) => void) => {
    mocks.listeners.set(name, callback);
    return () => mocks.listeners.delete(name);
  }),
}));
vi.mock("../../hooks/useAppDispatch", () => ({
  useAppSelector: (selector: (state: unknown) => unknown) => selector(mocks.state),
}));
vi.mock("../chat/AssistantMarkdownContent", () => ({
  AssistantMarkdownContent: ({
    content,
    className,
  }: {
    content: string;
    className?: string;
  }) => (
    <div data-testid="assistant-markdown" className={className}>
      {content}
    </div>
  ),
}));
vi.mock("../chat/AgentTurn", () => ({
  AgentTurnUI: ({
    entry,
  }: {
    entry: { blocks?: Array<{ type: string; text?: string; name?: string }> };
  }) => (
    <div data-testid="agent-turn">
      {entry.blocks?.map((block, index) => (
        <div key={index} data-block-type={block.type}>
          {block.type === "tool" ? block.name : block.text}
        </div>
      ))}
    </div>
  ),
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
    mocks.listeners.clear();
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

    expect(container.querySelector(".agent-swarm-strip")).toBeNull();
    expect(container.textContent).not.toContain("Agent Swarm · Active");
    expect(container.textContent).not.toContain("Fix the crash");
    expect(container.querySelector('[aria-label="Swarm usage"]')).not.toBeNull();
    expect(container.textContent).toContain("2/8 msg");
    expect(container.textContent).toContain("2/8 turns");
    expect(container.textContent).toContain("2/4 hops");
    expect(container.querySelector('[aria-label="Stop swarm"]')).not.toBeNull();
    expect(container.textContent).toContain("Reply from Debugger");
    expect(container.textContent).toContain("The crash comes from a stale task lease.");
    expect(container.textContent).toContain("Debugger found a stale task lease.");
    const replyMarkdown = container.querySelector(
      ".agent-peer-message-content [data-testid='assistant-markdown']",
    );
    expect(replyMarkdown).not.toBeNull();
    expect(replyMarkdown?.className).toContain("agent-peer-markdown");
  });

  it("cancels the swarm from the composer Stop button", async () => {
    const view = conversationView();
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "command_agent_swarm") {
        return Promise.resolve({
          ...view.swarm,
          run: { ...view.swarm!.run, status: "cancelled" },
        });
      }
      return Promise.resolve(view);
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    const stop = container.querySelector('[aria-label="Stop swarm"]') as HTMLButtonElement;
    expect(stop).not.toBeNull();
    await act(async () => {
      stop.click();
    });
    expect(mocks.invoke).toHaveBeenCalledWith(
      "command_agent_swarm",
      expect.objectContaining({
        runId: "swarm-1",
        command: { type: "cancel", reason: "Cancelled by user" },
      }),
    );
    expect(container.querySelector('[aria-label="Send message"]')).not.toBeNull();
  });

  it("renders inbound peer message cards through the markdown renderer", async () => {
    const view = conversationView();
    view.session.messages = [
      {
        role: "user",
        content: "peer envelope",
        metadata: {
          agent_messaging: {
            direction: "inbound",
            message_id: "review-1",
            from_agent_id: "coder",
            from_display_name: "Coder",
            display_content:
              "Review the primary agent's completed work:\n\n**Code created (`demo.py`)**\n```python\nprint(1 + 1)\n```",
          },
        },
      },
    ];
    mocks.invoke.mockResolvedValue(view);

    await act(async () => {
      root.render(<AgentConversationChat agent={debuggerAgent} onOpenSettings={() => {}} />);
    });

    expect(container.textContent).toContain("Message from Coder");
    const markdown = container.querySelector(
      ".agent-peer-message-content [data-testid='assistant-markdown']",
    );
    expect(markdown).not.toBeNull();
    expect(markdown?.className).toContain("agent-peer-markdown");
    expect(markdown?.textContent).toContain("Code created (`demo.py`)");
    expect(markdown?.textContent).toContain("print(1 + 1)");
  });

  it("renders persisted thinking and tool calls with the chat turn UI", async () => {
    const view = conversationView();
    view.session.messages.push(
      {
        role: "assistant",
        content: "<think>write a tiny script</think>\nCreated demo.py",
        tool_calls: [{ id: "call-1", function: { name: "write_file", arguments: "{\"path\":\"demo.py\"}" } }],
      },
      { role: "tool", content: "wrote demo.py", tool_call_id: "call-1", name: "write_file" },
    );
    mocks.invoke.mockResolvedValue(view);

    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    expect(container.textContent).toContain("write a tiny script");
    expect(container.textContent).toContain("Created demo.py");
    expect(container.textContent).toContain("write_file");
  });

  it("keeps a reply receipt under the matching reply instead of the thread footer", async () => {
    const view = conversationView();
    view.conversation.agent_id = "debugger";
    view.session.messages = [
      {
        role: "user",
        content: "peer envelope",
        metadata: {
          agent_messaging: {
            direction: "inbound",
            message_id: "request-1",
            from_agent_id: "coder",
            from_display_name: "Coder",
            display_content: "Please review demo.py",
          },
        },
      },
      {
        role: "assistant",
        content: "The script is correct.",
        tool_calls: [{
          id: "call-reply",
          function: { name: "send_agent_message", arguments: "{\"to\":\"coder\",\"message\":\"Looks correct\"}" },
        }],
      },
      { role: "tool", content: "sent", tool_call_id: "call-reply", name: "send_agent_message" },
      { role: "user", content: "keep going on tests" },
      { role: "assistant", content: "I will write more tests." },
    ];
    view.messaging.events = [
      {
        sequence: 1,
        conversation_id: "conversation-coder",
        event_type: "message_received",
        message_id: "request-1",
        payload: { from: "Coder", kind: "request" },
        created_at: "2026-08-25T00:00:00Z",
      },
      {
        sequence: 2,
        conversation_id: "conversation-coder",
        event_type: "message_sent",
        message_id: "reply-1",
        payload: { to: "coder", kind: "reply" },
        created_at: "2026-08-25T00:00:02Z",
      },
      {
        sequence: 3,
        conversation_id: "conversation-coder",
        event_type: "message_received",
        message_id: "later-inbound",
        payload: { from: "Coder", kind: "request", display_content: "Queued follow-up" },
        created_at: "2026-08-25T00:00:03Z",
      },
    ];
    mocks.invoke.mockResolvedValue(view);

    await act(async () => {
      root.render(<AgentConversationChat agent={debuggerAgent} onOpenSettings={() => {}} />);
    });

    const text = container.textContent ?? "";
    const receiptAt = text.indexOf("Debugger replied to coder");
    expect(receiptAt).toBeGreaterThan(-1);
    expect(receiptAt).toBeLessThan(text.indexOf("keep going on tests"));
    expect(receiptAt).toBeLessThan(text.indexOf("Queued follow-up"));
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

  it("keeps a sent message visible while the agent turn is still running", async () => {
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "open_agent_conversation") return Promise.resolve(conversationView());
      if (command === "send_agent_conversation_message") return new Promise(() => {});
      return Promise.resolve(undefined);
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    await act(async () => {
      const valueSetter = Object.getOwnPropertyDescriptor(
        HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      valueSetter?.call(textarea, "build the calculator");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      (container.querySelector('[aria-label="Send message"]') as HTMLButtonElement).click();
    });

    expect(container.textContent).toContain("build the calculator");
    expect(container.querySelector(".agent-conversation-avatar.working")).not.toBeNull();
    expect(container.querySelector(".agent-turn-avatar.working")).not.toBeNull();
    expect(container.querySelector(".agent-turn-avatar .agent-presence-dot.working")).not.toBeNull();
    expect(container.querySelector(".agent-turn-avatar .animate-spin")).toBeNull();
    expect(container.querySelector('[data-testid="agent-turn-status"]')?.textContent).toMatch(/Working/);
    const pending = container.querySelector('[data-testid="pending-user-message"]');
    expect(pending?.classList.contains("agent-conversation-user-group")).toBe(true);
    expect(pending?.querySelector(".user-row")).not.toBeNull();
    expect(pending?.querySelector(".agent-pending-message-status")?.textContent).toContain(
      "Queued · waiting for Coder",
    );
  });

  it("streams live thinking into the chat turn UI", async () => {
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });
    await act(async () => {
      mocks.listeners.get("agent-conversation-event")?.({
        payload: {
          conversation_id: "conversation-coder",
          agent_id: "coder",
          turn_id: "turn-1",
          event: {
            SubagentMessageUpdate: {
              subagent_id: "coder",
              message_id: "m1",
              delta: { Thinking: "planning the script" },
            },
          },
        },
      });
    });

    expect(container.textContent).toContain("planning the script");
    expect(container.querySelector(".agent-turn-avatar.working")).not.toBeNull();
    expect(container.querySelector('[data-testid="agent-turn-status"]')?.textContent).toMatch(/Working/);
  });

  it("does not carry a pending contact's working state into another agent", async () => {
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "open_agent_conversation") return Promise.resolve(conversationView());
      if (command === "send_agent_conversation_message") return new Promise(() => {});
      return Promise.resolve(undefined);
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });
    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    await act(async () => {
      const valueSetter = Object.getOwnPropertyDescriptor(
        HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      valueSetter?.call(textarea, "build the calculator");
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
      (container.querySelector('[aria-label="Send message"]') as HTMLButtonElement).click();
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={debuggerAgent} onOpenSettings={() => {}} />);
    });

    expect(container.textContent).not.toContain("Debugger is working");
    expect(container.querySelector(".agent-conversation-avatar.working")).toBeNull();
  });

  it("routes a tool approval to the agent conversation turn that requested it", async () => {
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });
    await act(async () => {
      mocks.listeners.get("agent-conversation-approval")?.({
        payload: {
          conversation_id: "conversation-coder",
          agent_id: "coder",
          turn_id: "turn-coder",
          event_type: "required",
          prompt_id: "prompt-repl",
          tool_name: "repl",
          tool_input: { code: "1 + 1" },
          danger_level: "medium",
          explanation: "Execute Python code",
        },
      });
    });

    expect(container.textContent).toContain("Approval Required: repl");
    await act(async () => {
      (container.querySelector("button.btn-allow") as HTMLButtonElement).click();
    });
    expect(mocks.invoke).toHaveBeenCalledWith("approve_agent_conversation_tool", {
      turnId: "turn-coder",
      promptId: "prompt-repl",
      choice: "allow_once",
    });
  });

  it("locks an approval card after the first choice and does not promise persistent rules", async () => {
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "open_agent_conversation") return Promise.resolve(conversationView());
      if (command === "approve_agent_conversation_tool") return new Promise(() => {});
      return Promise.resolve(undefined);
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });
    await act(async () => {
      mocks.listeners.get("agent-conversation-approval")?.({
        payload: {
          conversation_id: "conversation-coder",
          agent_id: "coder",
          turn_id: "turn-coder",
          event_type: "required",
          prompt_id: "prompt-shell",
          tool_name: "shell",
          tool_input: { command: "pytest" },
          danger_level: "medium",
          explanation: "Run tests",
        },
      });
    });

    const allow = [...container.querySelectorAll("button")]
      .find((button) => button.textContent === "Allow Once") as HTMLButtonElement;
    await act(async () => allow.click());

    expect(container.textContent).toContain("Applying your choice…");
    expect(container.textContent).not.toContain("Always Allow");
    expect(container.textContent).not.toContain("Deny Always");
    expect([...container.querySelectorAll(".agent-approval-actions button")]
      .every((button) => (button as HTMLButtonElement).disabled)).toBe(true);
  });

  it("restores a pending sent message after switching away and back", async () => {
    mocks.invoke.mockImplementation((command: string, args?: { agentId?: string }) => {
      if (command === "send_agent_conversation_message") return new Promise(() => {});
      if (command === "open_agent_conversation") {
        const result = conversationView();
        if (args?.agentId === "coder") {
          result.pending_messages = [{ turn_id: "coder-turn", content: "build the calculator" }];
        }
        return Promise.resolve(result);
      }
      return Promise.resolve(undefined);
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={debuggerAgent} onOpenSettings={() => {}} />);
    });
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    expect(container.textContent).toContain("build the calculator");
  });

  it("ignores an approval from another project conversation for the same agent", async () => {
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });
    await act(async () => {
      mocks.listeners.get("agent-conversation-approval")?.({
        payload: {
          conversation_id: "conversation-coder-other-project",
          agent_id: "coder",
          turn_id: "other-turn",
          event_type: "required",
          prompt_id: "other-approval",
          tool_name: "shell",
        },
      });
    });

    expect(container.textContent).not.toContain("Approval Required: shell");
  });

  it("ignores a stale conversation load that resolves after switching projects", async () => {
    let resolveOld: ((view: AgentConversationView) => void) | undefined;
    mocks.state = {
      agents: { agents: [coder, debuggerAgent, reviewerAgent] },
      project: { activeProjectId: "project-old" },
    };
    mocks.invoke.mockImplementation((command: string, args?: { projectId?: string }) => {
      if (command !== "open_agent_conversation") return Promise.resolve(undefined);
      if (args?.projectId === "project-old") {
        return new Promise<AgentConversationView>((resolve) => { resolveOld = resolve; });
      }
      const current = conversationView();
      current.conversation.id = "conversation-coder-new";
      current.conversation.project_id = "project-new";
      return Promise.resolve(current);
    });

    act(() => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });
    await act(async () => { await Promise.resolve(); });
    mocks.state = {
      agents: { agents: [coder, debuggerAgent, reviewerAgent] },
      project: { activeProjectId: "project-new" },
    };
    await act(async () => {
      root.render(<AgentConversationChat agent={coder} onOpenSettings={() => {}} />);
    });

    const stale = conversationView();
    stale.conversation.id = "conversation-coder-old";
    stale.conversation.project_id = "project-old";
    await act(async () => resolveOld?.(stale));
    await act(async () => {
      mocks.listeners.get("agent-conversation-approval")?.({
        payload: {
          conversation_id: "conversation-coder-old",
          agent_id: "coder",
          turn_id: "old-turn",
          event_type: "required",
          prompt_id: "old-approval",
          tool_name: "shell",
          tool_input: { command: "dangerous-old-project-command" },
          danger_level: "high",
          explanation: "Old project approval",
        },
      });
    });

    expect(container.textContent).not.toContain("Old project approval");
  });
});
