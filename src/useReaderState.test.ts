import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Message, ThreadDetail } from "./domain";
import { useReaderState } from "./useReaderState";

const message = (id: string, unread = false): Message => ({
  id,
  threadId: "thread-1",
  sender: `${id}@example.com`,
  recipients: ["me@example.com"],
  sentAt: "2026-09-18T12:00:00Z",
  bodyHtml: `<p>${id}</p>`,
  bodyText: id,
  unread,
  attachments: [],
});

const detail: ThreadDetail = {
  thread: {
    id: "thread-1",
    providerThreadId: "provider-1",
    subject: "Subject",
    snippet: "Snippet",
    participants: ["sender@example.com"],
    lastMessageAt: "2026-09-18T12:00:00Z",
    lastReceivedAt: "2026-09-18T12:00:00Z",
    unread: true,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: "account@example.com",
    summary: null,
    summaryGeneratedAt: null,
    summaryRevision: null,
    hasAttachments: false,
  },
  messages: [message("older"), message("latest")],
};

describe("useReaderState", () => {
  it("expands the latest message and tracks the displayed latest message", () => {
    const { result } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1",
        detail: detail as ThreadDetail | null,
        displayedMessages: detail.messages,
        composerOpen: false,
      },
    });

    expect(result.current.messageExpansionOverrides).toEqual(new Map([
      ["older", false],
      ["latest", true],
    ]));
    expect(result.current.activeMessageId).toBe("latest");
    expect(result.current.latestDisplayedMessageId).toBe("latest");
  });

  it("preserves explicit expansion choices while detail messages refresh", () => {
    const { result, rerender } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1",
        detail: detail as ThreadDetail | null,
        displayedMessages: detail.messages,
        composerOpen: true,
      },
    });

    act(() => result.current.setMessageExpansionOverrides(new Map([["older", true], ["latest", false]])));
    rerender({
      selectedThreadId: "thread-1",
      detail: { ...detail, messages: [...detail.messages, message("newest")] },
      displayedMessages: [...detail.messages, message("newest")],
      composerOpen: true,
    });

    expect(result.current.messageExpansionOverrides).toEqual(new Map([
      ["older", true],
      ["latest", false],
      ["newest", true],
    ]));
  });

  it("resets reader state when selection changes and does not scroll without thread detail", () => {
    const scrollIntoView = vi.fn();
    const { result, rerender } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1",
        detail: detail as ThreadDetail | null,
        displayedMessages: detail.messages,
        composerOpen: false,
      },
    });
    result.current.latestMessageRef.current = { scrollIntoView } as unknown as HTMLElement;
    rerender({
      selectedThreadId: "thread-2",
      detail: null,
      displayedMessages: [],
      composerOpen: false,
    });

    expect(result.current.messageExpansionOverrides).toEqual(new Map());
    expect(result.current.activeMessageId).toBeNull();
    expect(scrollIntoView).not.toHaveBeenCalled();
  });

  it.each(["read status", "summary", "refresh"])("preserves scroll and active message after a %s update", (update) => {
    const scrollIntoView = vi.fn();
    const { result, rerender } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1",
        detail: null as ThreadDetail | null,
        displayedMessages: [] as Message[],
        composerOpen: false,
      },
    });
    result.current.latestMessageRef.current = { scrollIntoView } as unknown as HTMLElement;
    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: false });
    expect(scrollIntoView).toHaveBeenCalledTimes(1);
    act(() => {
      result.current.activeMessageIdRef.current = "older";
      result.current.setActiveMessageId("older");
    });
    const refreshed: ThreadDetail = {
      ...detail,
      thread: {
        ...detail.thread,
        ...(update === "read status" ? { unread: false } : {}),
        ...(update === "summary" ? { summary: "A new summary" } : {}),
      },
      messages: detail.messages.map((item) => ({ ...item, unread: false })),
    };
    rerender({ selectedThreadId: "thread-1", detail: refreshed, displayedMessages: refreshed.messages, composerOpen: false });

    expect(scrollIntoView).toHaveBeenCalledTimes(1);
    expect(result.current.activeMessageId).toBe("older");
    expect(result.current.activeMessageIdRef.current).toBe("older");
  });

  it("scrolls when a new latest message arrives and when a conversation is reopened", () => {
    const scrollIntoView = vi.fn();
    const { result, rerender } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1" as string | null,
        detail: null as ThreadDetail | null,
        displayedMessages: [] as Message[],
        composerOpen: false,
      },
    });
    result.current.latestMessageRef.current = { scrollIntoView } as unknown as HTMLElement;
    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: false });
    const next = { ...detail, messages: [...detail.messages, message("newest")] };
    rerender({ selectedThreadId: "thread-1", detail: next, displayedMessages: next.messages, composerOpen: false });
    expect(scrollIntoView).toHaveBeenCalledTimes(2);
    expect(result.current.activeMessageId).toBe("newest");

    rerender({ selectedThreadId: null, detail: null, displayedMessages: [], composerOpen: false });
    rerender({ selectedThreadId: "thread-1", detail: next, displayedMessages: next.messages, composerOpen: false });
    expect(scrollIntoView).toHaveBeenCalledTimes(3);
  });

  it("does not scroll again when a composer closes after the conversation was positioned", () => {
    const scrollIntoView = vi.fn();
    const { result, rerender } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1",
        detail: null as ThreadDetail | null,
        displayedMessages: [] as Message[],
        composerOpen: false,
      },
    });
    result.current.latestMessageRef.current = { scrollIntoView } as unknown as HTMLElement;
    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: false });
    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: true });
    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: false });
    expect(scrollIntoView).toHaveBeenCalledTimes(1);
  });

  it.each([
    { composerOpen: false, scrolls: true },
    { composerOpen: true, scrolls: false },
  ])("scrolls to the latest message when detail loads only without a composer (composerOpen: $composerOpen)", ({ composerOpen, scrolls }) => {
    const scrollIntoView = vi.fn();
    const { result, rerender } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1",
        detail: null as ThreadDetail | null,
        displayedMessages: [] as Message[],
        composerOpen,
      },
    });
    result.current.latestMessageRef.current = { scrollIntoView } as unknown as HTMLElement;

    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen });

    if (scrolls) {
      expect(scrollIntoView).toHaveBeenCalledTimes(1);
      expect(scrollIntoView).toHaveBeenCalledWith({ block: "start" });
    } else {
      expect(scrollIntoView).not.toHaveBeenCalled();
    }
    expect(result.current.latestDisplayedMessageId).toBe("latest");
  });

  it("scrolls to the latest message once the composer closes over loaded detail", () => {
    const scrollIntoView = vi.fn();
    const { result, rerender } = renderHook((props) => useReaderState(props), {
      initialProps: {
        selectedThreadId: "thread-1",
        detail: detail as ThreadDetail | null,
        displayedMessages: detail.messages,
        composerOpen: true,
      },
    });
    result.current.latestMessageRef.current = { scrollIntoView } as unknown as HTMLElement;
    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: true });
    expect(scrollIntoView).not.toHaveBeenCalled();

    rerender({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: false });
    expect(scrollIntoView).toHaveBeenCalledTimes(1);
    expect(scrollIntoView).toHaveBeenCalledWith({ block: "start" });
  });
});
