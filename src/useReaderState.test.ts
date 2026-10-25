import { act, fireEvent, render, renderHook } from "@testing-library/react";
import { createElement } from "react";
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

  describe("keeping the latest message at the top while its body sizes", () => {
    /** Mounts a stack whose latest message grows later, with a controllable ResizeObserver. */
    function mountGrowingReader() {
      const resize = { notify: () => {}, disconnected: false };
      vi.stubGlobal("ResizeObserver", class {
        constructor(callback: () => void) { resize.notify = () => { if (!resize.disconnected) callback(); }; }
        observe() {}
        disconnect() { resize.disconnected = true; }
      });
      const scrollIntoView = vi.fn();
      function Reader() {
        const reader = useReaderState({ selectedThreadId: "thread-1", detail, displayedMessages: detail.messages, composerOpen: false });
        return createElement("div", { ref: reader.messageStackRef, "data-testid": "stack" },
          createElement("article", { ref: (node: HTMLElement | null) => {
            if (node) {
              node.scrollIntoView = scrollIntoView;
              Object.defineProperty(node, "offsetTop", { configurable: true, value: 900 });
            }
            reader.latestMessageRef.current = node;
          } }));
      }
      const view = render(createElement(Reader));
      return { stack: view.getByTestId("stack"), resize, scrollIntoView, unmount: view.unmount };
    }

    it("scrolls the stack back to the latest message's top, inside its scroll padding, each time it grows", () => {
      try {
        const reader = mountGrowingReader();
        expect(reader.scrollIntoView).toHaveBeenCalledTimes(1);
        reader.stack.style.scrollPaddingTop = "24px";
        reader.resize.notify();
        expect(reader.stack.scrollTop).toBe(876);
        reader.stack.scrollTop = 500;
        reader.resize.notify();
        expect(reader.stack.scrollTop).toBe(876);
        reader.unmount();
        expect(reader.resize.disconnected).toBe(true);
      } finally {
        vi.unstubAllGlobals();
      }
    });

    it.each(["wheel", "touchMove", "pointerDown", "keyDown"] as const)("stops once the reader moves through the conversation (%s)", (event) => {
      try {
        const reader = mountGrowingReader();
        fireEvent[event](window);
        expect(reader.resize.disconnected).toBe(true);
        reader.resize.notify();
        expect(reader.stack.scrollTop).toBe(0);
      } finally {
        vi.unstubAllGlobals();
      }
    });

    it("stops when the stack scrolls away from where it was pinned, as from inside the email", () => {
      try {
        const reader = mountGrowingReader();
        fireEvent.scroll(reader.stack);
        expect(reader.resize.disconnected).toBe(false);

        reader.stack.scrollTop = 240;
        fireEvent.scroll(reader.stack);
        expect(reader.resize.disconnected).toBe(true);
      } finally {
        vi.unstubAllGlobals();
      }
    });
  });
});

describe("the message being read", () => {
  const three = [message("first"), message("middle"), message("latest")];
  const threeDetail = { ...detail, messages: three };

  /** Mounts the reader's scroll container and message nodes, with their positions set by `layout`. */
  function mountReader() {
    let reader!: ReturnType<typeof useReaderState>;
    function Reader() {
      reader = useReaderState({ selectedThreadId: "thread-1", detail: threeDetail, displayedMessages: three, composerOpen: false });
      return createElement("div", { ref: reader.messageStackRef, "data-testid": "stack" },
        three.map((item) => createElement("article", { key: item.id, ref: (node: HTMLElement | null) => { if (node) reader.messageRefs.current.set(item.id, node); } })));
    }
    const view = render(createElement(Reader));
    const stack = view.getByTestId("stack");
    const rect = (top: number, height: number) => ({ top, bottom: top + height, height, left: 0, right: 0, width: 0, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;
    /** Places each message's top relative to the reader, which is 600px tall at the viewport's top. */
    const layout = (tops: Record<string, number>, atBottom = false) => {
      stack.getBoundingClientRect = () => rect(0, 600);
      Object.defineProperty(stack, "clientHeight", { configurable: true, value: 600 });
      Object.defineProperty(stack, "scrollHeight", { configurable: true, value: 3000 });
      Object.defineProperty(stack, "scrollTop", { configurable: true, value: atBottom ? 2400 : 1000 });
      for (const item of three) {
        reader.messageRefs.current.get(item.id)!.getBoundingClientRect = () => rect(tops[item.id], 500);
      }
    };
    const scroll = () => act(() => { fireEvent.scroll(stack); });
    return { current: () => reader, layout, scroll };
  }

  it("follows the message at the reading line as the reader scrolls, without changing the active message", () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 0; });
    try {
      const reader = mountReader();
      expect(reader.current().readingMessageId).toBe("latest");

      reader.layout({ first: -300, middle: 100, latest: 700 });
      reader.scroll();
      expect(reader.current().readingMessageId).toBe("middle");
      expect(reader.current().activeMessageId).toBe("latest");

      reader.layout({ first: 50, middle: 650, latest: 1250 });
      reader.scroll();
      expect(reader.current().readingMessageId).toBe("first");

      // At the end of the conversation the last message counts, even when short.
      reader.layout({ first: -1900, middle: -1300, latest: 500 }, true);
      reader.scroll();
      expect(reader.current().readingMessageId).toBe("latest");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("holds a clicked message while any of it is on screen", () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 0; });
    try {
      const reader = mountReader();
      act(() => reader.current().setReadingMessage("latest", true));

      reader.layout({ first: -300, middle: 100, latest: 550 });
      reader.scroll();
      expect(reader.current().readingMessageId).toBe("latest");

      reader.layout({ first: -200, middle: 0, latest: 700 });
      reader.scroll();
      expect(reader.current().readingMessageId).toBe("middle");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
