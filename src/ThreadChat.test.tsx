import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatAttachmentOption } from "./chatAttachments";
import { QUICK_QUESTIONS, sharedChatAttachments, ThreadChat, type ChatEntry } from "./ThreadChat";

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
  addedSuggestions: 0, hiddenSuggestions: 0, sources: [], searched: [], attachments: [], availability: null, ...overrides,
});

describe("ThreadChat", () => {
  afterEach(cleanup);

  it("offers saved events in the schedule while counting only suggestions still awaiting review", () => {
    const saved = { id: "saved", accountId: "you@example.com", title: "Class", start: "2026-10-12T18:00:00Z", end: "2026-10-12T19:30:00Z", allDay: false };
    const onOpenCalendarEvent = vi.fn();
    const { rerender } = render(<ThreadChat {...props({ onOpenCalendarEvent,
      entries: [answer({ addedSuggestions: 1, handledSuggestions: 1, calendarEvents: [saved] })],
    })} />);
    expect(screen.getByRole("button", { name: "Added 1 suggestion to review" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "View Class in schedule" }));
    expect(onOpenCalendarEvent).toHaveBeenCalledWith(saved);
    rerender(<ThreadChat {...props({ entries: [answer({ handledSuggestions: 2 })] })} />);
    expect(screen.queryByRole("button", { name: /suggestion.*to review/ })).not.toBeInTheDocument();
    expect(screen.getByText("Suggestions reviewed")).toBeInTheDocument();
  });

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
    expect(onAsk).toHaveBeenCalledWith("Line one", true, []);
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
    expect(onAsk).toHaveBeenCalledWith(QUICK_QUESTIONS[0], false, []);

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
        { id: "q1", role: "user", content: "Draft a reply", searchMailbox: true, attachments: [] },
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

  describe("sharing attachments with @", () => {
    const attachment = (filename: string, attachmentId = filename): ChatAttachmentOption => ({
      messageId: "m1", attachmentId, filename, sender: "Avery <avery@example.com>", sentAt: "2026-09-01T00:00:00Z",
    });
    const report = attachment("Q3 report.pdf");
    const budget = attachment("budget.xlsx");
    const notes = attachment("notes.txt");

    function openChat(overrides: Partial<Props> = {}) {
      const onAsk = vi.fn();
      render(<ThreadChat {...props({ onAsk, attachments: [report, budget, notes], ...overrides })} />);
      fireEvent.click(screen.getByRole("button", { name: /Ask about this conversation/ }));
      return { onAsk, input: screen.getByRole("textbox", { name: "Ask about this conversation" }) };
    }

    it("lists matching attachments after @ and shares only the chosen one with the question", () => {
      const { onAsk, input } = openChat();
      expect(input).toHaveAttribute("placeholder", expect.stringContaining("Type @ to add an attachment"));
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

      fireEvent.change(input, { target: { value: "Summarize @bud" } });
      const menu = screen.getByRole("listbox", { name: "Attachments" });
      expect(within(menu).getAllByRole("option").map((option) => option.textContent)).toEqual(["budget.xlsxAvery <avery@example.com>"]);
      expect(input).toHaveAttribute("aria-expanded", "true");
      expect(input).toHaveAttribute("aria-activedescendant", within(menu).getByRole("option").id);

      fireEvent.keyDown(input, { key: "Enter" });
      expect(onAsk).not.toHaveBeenCalled();
      expect(input).toHaveValue("Summarize @budget.xlsx ");
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      expect(within(screen.getByRole("list", { name: "Attachments shared with AI" })).getByText("budget.xlsx")).toBeInTheDocument();

      fireEvent.keyDown(input, { key: "Enter" });
      expect(onAsk).toHaveBeenCalledWith("Summarize @budget.xlsx", false, [budget]);
      expect(input).toHaveValue("");
      expect(screen.queryByRole("list", { name: "Attachments shared with AI" })).not.toBeInTheDocument();
    });

    it("moves through matches with the arrow keys, chooses with Tab or a click, and closes the menu on Escape", () => {
      const { onAsk, input } = openChat();
      fireEvent.change(input, { target: { value: "@" } });
      const options = () => within(screen.getByRole("listbox")).getAllByRole("option");
      expect(options()).toHaveLength(3);
      expect(options()[0]).toHaveAttribute("aria-selected", "true");
      fireEvent.keyDown(input, { key: "ArrowDown" });
      expect(options()[1]).toHaveAttribute("aria-selected", "true");
      fireEvent.keyDown(input, { key: "ArrowUp" });
      fireEvent.keyDown(input, { key: "ArrowUp" });
      expect(options()[2]).toHaveAttribute("aria-selected", "true");
      fireEvent.keyDown(input, { key: "Tab" });
      expect(input).toHaveValue("@notes.txt ");

      fireEvent.change(input, { target: { value: "@notes.txt and @" } });
      // Already-chosen files are not offered again.
      expect(options().map((option) => option.querySelector("span")?.textContent)).toEqual(["Q3 report.pdf", "budget.xlsx"]);
      fireEvent.click(options()[0]);
      expect(input).toHaveValue("@notes.txt and @Q3 report.pdf ");

      fireEvent.change(input, { target: { value: "@notes.txt and @Q3 report.pdf vs @b" } });
      const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
      act(() => { input.dispatchEvent(escape); });
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      expect(input).toBeInTheDocument();
      fireEvent.keyDown(input, { key: "Enter" });
      expect(onAsk).toHaveBeenCalledWith("@notes.txt and @Q3 report.pdf vs @b", false, [notes, report]);
    });

    it("ignores @ inside addresses and words, and needs a match to open", () => {
      const { input } = openChat();
      fireEvent.change(input, { target: { value: "Ask sam@bud" } });
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      fireEvent.change(input, { target: { value: "Ask @zzz" } });
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
      fireEvent.change(input, { target: { value: "Line\n@" } });
      expect(screen.getByRole("listbox")).toBeInTheDocument();
    });

    it("lets a chosen attachment be removed before asking", () => {
      const { onAsk, input } = openChat();
      fireEvent.change(input, { target: { value: "@Q3" } });
      fireEvent.keyDown(input, { key: "Enter" });
      fireEvent.click(screen.getByRole("button", { name: "Don’t share Q3 report.pdf" }));
      expect(screen.queryByRole("list", { name: "Attachments shared with AI" })).not.toBeInTheDocument();
      fireEvent.keyDown(input, { key: "Enter" });
      expect(onAsk).toHaveBeenCalledWith("@Q3 report.pdf", false, []);
    });

    it("keeps earlier shares visible, offers only the rest, and stops at the per-chat limit", () => {
      const extra = [attachment("a.txt"), attachment("b.txt"), attachment("c.txt")];
      const { input } = openChat({ attachments: [report, budget, ...extra], sharedAttachments: [report] });
      const shared = screen.getByRole("list", { name: "Attachments shared with AI" });
      expect(within(shared).getByText("Q3 report.pdf")).toBeInTheDocument();
      expect(within(shared).queryByRole("button")).not.toBeInTheDocument();

      fireEvent.change(input, { target: { value: "@" } });
      expect(within(screen.getByRole("listbox")).getAllByRole("option")).toHaveLength(4);
      for (const name of ["budget", "a.txt", "b.txt"]) {
        fireEvent.change(input, { target: { value: `@${name}` } });
        fireEvent.keyDown(input, { key: "Enter" });
      }
      fireEvent.change(input, { target: { value: "@c" } });
      const menu = screen.getByRole("listbox");
      expect(within(menu).getByText("You can share up to 4 attachments in one chat.")).toBeInTheDocument();
      expect(within(menu).queryByRole("option")).not.toBeInTheDocument();
    });

    it("offers no @ menu when the conversation has no readable attachments", () => {
      const { input } = openChat({ attachments: [] });
      expect(input).toHaveAttribute("placeholder", "Ask about this conversation…");
      expect(input).not.toHaveAttribute("aria-expanded");
      fireEvent.change(input, { target: { value: "@" } });
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("shows which files each question shared and when only part of one was read", () => {
      render(<ThreadChat {...props({
        entries: [
          { id: "q1", role: "user", content: "Summarize @Q3 report.pdf", searchMailbox: false, attachments: [report] },
          answer({ attachments: [{ messageId: "m1", attachmentId: report.attachmentId, filename: report.filename, truncated: true }] }),
          { id: "q2", role: "user", content: "And the budget?", searchMailbox: false, attachments: [] },
          answer({ id: "a2", attachments: [{ messageId: "m1", attachmentId: report.attachmentId, filename: report.filename, truncated: false }] }),
        ],
      })} />);
      const log = screen.getByRole("log", { name: "Conversation with AI" });
      expect(within(log).getAllByText("Shared Q3 report.pdf")).toHaveLength(1);
      expect(within(log).getAllByText("Q3 report.pdf is long, so only its first part was shared.")).toHaveLength(1);
    });

    it("collects each shared attachment once from the chat's questions", () => {
      expect(sharedChatAttachments([
        { id: "q1", role: "user", content: "One", searchMailbox: false, attachments: [report, budget] },
        answer(),
        { id: "q2", role: "user", content: "Two", searchMailbox: false, attachments: [budget, notes] },
      ])).toEqual([report, budget, notes]);
    });
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
