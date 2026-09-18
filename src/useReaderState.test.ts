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

  it("resets reader state when selection changes and scrolls only without a composer", () => {
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
});
