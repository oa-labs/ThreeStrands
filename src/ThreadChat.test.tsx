import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { QUICK_QUESTIONS, ThreadChat, type ChatEntry } from "./ThreadChat";

type Props = Parameters<typeof ThreadChat>[0];

function props(overrides: Partial<Props> = {}): Props {
  return {
    enabled: true,
    available: true,
    entries: [],
    pending: false,
    error: null,
    focusRequest: 0,
    onAsk: vi.fn(),
    onRetry: vi.fn(),
    onUseReply: vi.fn(),
    onOpenThread: vi.fn(),
    onShowSuggestions: vi.fn(),
    onOpenSettings: vi.fn(),
    ...overrides,
  };
}

const answer = (overrides: Partial<Extract<ChatEntry, { role: "assistant" }>> = {}): ChatEntry => ({
  id: "a1", role: "assistant", content: "They need the deck by Friday.", replyDraft: null,
  addedSuggestions: 0, hiddenSuggestions: 0, sources: [], searched: [], availability: null, ...overrides,
});

describe("ThreadChat", () => {
  afterEach(cleanup);

  it("stays a read-only prompt until activated, so single-key shortcuts keep working", () => {
    render(<ThreadChat {...props()} />);
    const prompt = screen.getByRole("button", { name: /Ask about this conversation/ });
    expect(prompt).toHaveAttribute("aria-keyshortcuts", "q");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();

    fireEvent.click(prompt);
    const input = screen.getByRole("textbox", { name: "Ask about this conversation" });
    expect(input).toHaveFocus();
    expect(input.closest("[data-shortcut-scope]")).toHaveAttribute("data-shortcut-scope", "modal");
  });

  it("activates for a new shortcut request but not for one made before it mounted", () => {
    const { rerender } = render(<ThreadChat {...props({ focusRequest: 3 })} />);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    rerender(<ThreadChat {...props({ focusRequest: 4 })} />);
    expect(screen.getByRole("textbox", { name: "Ask about this conversation" })).toHaveFocus();
  });

  it("returns to read mode and the previous focus on Escape, keeping unsent text", () => {
    const { rerender } = render(<>
      <button type="button">Reader</button>
      <ThreadChat {...props()} />
    </>);
    screen.getByRole("button", { name: "Reader" }).focus();
    rerender(<>
      <button type="button">Reader</button>
      <ThreadChat {...props({ focusRequest: 1 })} />
    </>);
    const input = screen.getByRole("textbox", { name: "Ask about this conversation" });
    fireEvent.change(input, { target: { value: "When is the deadline?" } });
    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    const windowListener = vi.fn();
    window.addEventListener("keydown", windowListener);
    act(() => { input.dispatchEvent(escape); });
    window.removeEventListener("keydown", windowListener);

    expect(windowListener).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reader" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: /When is the deadline\?/ }));
    expect(screen.getByRole("textbox", { name: "Ask about this conversation" })).toHaveValue("When is the deadline?");
  });

  it("asks on Enter, keeps Shift+Enter for new lines, and resets the mailbox search after each question", () => {
    const onAsk = vi.fn();
    render(<ThreadChat {...props({ onAsk })} />);
    fireEvent.click(screen.getByRole("button", { name: /Ask about this conversation/ }));
    const input = screen.getByRole("textbox", { name: "Ask about this conversation" });

    fireEvent.change(input, { target: { value: "Line one" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(onAsk).not.toHaveBeenCalled();

    const search = screen.getByRole("checkbox", { name: "Search all mail" });
    fireEvent.click(search);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onAsk).toHaveBeenCalledWith("Line one", true);
    expect(input).toHaveValue("");
    expect(search).not.toBeChecked();
    expect(input).toHaveFocus();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onAsk).toHaveBeenCalledTimes(1);
  });

  it("offers quick questions before the first exchange and restores the prompt when left empty", () => {
    const onAsk = vi.fn();
    render(<><button type="button">Elsewhere</button><ThreadChat {...props({ onAsk })} /></>);
    fireEvent.click(screen.getByRole("button", { name: /Ask about this conversation/ }));
    const quick = screen.getByRole("group", { name: "Suggested questions" });
    fireEvent.click(within(quick).getByRole("button", { name: QUICK_QUESTIONS[0] }));
    expect(onAsk).toHaveBeenCalledWith(QUICK_QUESTIONS[0], false);

    fireEvent.blur(screen.getByRole("textbox"), { relatedTarget: screen.getByRole("button", { name: "Elsewhere" }) });
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("renders answers as plain text with reply, suggestion, and source actions", () => {
    const onUseReply = vi.fn();
    const onOpenThread = vi.fn();
    const onShowSuggestions = vi.fn();
    render(<ThreadChat {...props({
      onUseReply, onOpenThread, onShowSuggestions,
      entries: [
        { id: "q1", role: "user", content: "Draft a reply", searchMailbox: true },
        answer({
          content: "<img src=x onerror=alert(1)> Sure.",
          replyDraft: "Thanks, Friday works.",
          addedSuggestions: 2,
          hiddenSuggestions: 1,
          sources: [{ threadId: "t2", accountId: "you@example.com", subject: "Pricing", lastMessageAt: "2026-09-01T00:00:00Z" }],
          searched: [
            { threadId: "t2", accountId: "you@example.com", subject: "Pricing", lastMessageAt: "2026-09-01T00:00:00Z" },
            { threadId: "t3", accountId: "you@example.com", subject: "Invoice", lastMessageAt: "2026-08-01T00:00:00Z" },
          ],
        }),
      ],
    })} />);

    const log = screen.getByRole("log", { name: "Conversation with AI" });
    expect(within(log).getByText("Searched all mail")).toBeInTheDocument();
    expect(within(log).getByText("<img src=x onerror=alert(1)> Sure.")).toBeInTheDocument();
    expect(log.querySelector("img")).toBeNull();
    fireEvent.click(within(log).getByRole("button", { name: "Use as Reply" }));
    expect(onUseReply).toHaveBeenCalledWith("Thanks, Friday works.");
    fireEvent.click(within(log).getByRole("button", { name: "Added 2 suggestions to review" }));
    expect(onShowSuggestions).toHaveBeenCalled();
    expect(within(log).getByText(/1 suggestion couldn’t be matched/)).toBeInTheDocument();
    fireEvent.click(within(within(log).getByRole("navigation", { name: "Sources" })).getByRole("button", { name: "Pricing" }));
    expect(onOpenThread).toHaveBeenCalledWith("t2");
    expect(within(log).getByText("Shared 2 other emails with AI")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Suggested questions" })).not.toBeInTheDocument();
  });

  it("explains a disabled or unconfigured chat instead of offering the box", () => {
    const onOpenSettings = vi.fn();
    const { rerender } = render(<ThreadChat {...props({ enabled: false, available: false, onOpenSettings, focusRequest: 0 })} />);
    expect(screen.getByText(/Turn on Thread Chat in AI settings/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "AI Settings" }));
    expect(onOpenSettings).toHaveBeenCalled();
    rerender(<ThreadChat {...props({ enabled: false, available: false, onOpenSettings, focusRequest: 1 })} />);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();

    rerender(<ThreadChat {...props({ enabled: true, available: false })} />);
    expect(screen.getByText(/Set up an AI provider and API key/)).toBeInTheDocument();
  });

  it("shows progress and a retryable failure", () => {
    const onRetry = vi.fn();
    const { rerender } = render(<ThreadChat {...props({ pending: true })} />);
    expect(screen.getByRole("status")).toHaveTextContent("Thinking…");
    rerender(<ThreadChat {...props({ error: "Couldn't reach the AI provider.", onRetry })} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn't reach the AI provider.");
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(onRetry).toHaveBeenCalled();
  });
});
