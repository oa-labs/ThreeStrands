import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ThreadDetail } from "./domain";
import { useReaderActions } from "./useReaderActions";

function setup() {
  const overrides = new Map<string, boolean>();
  const reader = {
    activeMessageIdRef: { current: null as string | null },
    setActiveMessageId: vi.fn(),
    pendingMessageToggleFocusRef: { current: null as string | null },
    setMessageExpansionOverrides: vi.fn((update: (current: Map<string, boolean>) => Map<string, boolean>) => {
      for (const [key, value] of update(overrides)) overrides.set(key, value);
    }),
    messageRefs: { current: new Map<string, HTMLElement>() },
    latestMessageRef: { current: null as HTMLElement | null },
  };
  const visibleDetail = { thread: { id: "thread-1" }, messages: [] } as unknown as ThreadDetail;
  const hook = renderHook(() => useReaderActions({
    reader: reader as never,
    displayedMessages: [],
    visibleDetail,
    selected: null,
    correspondence: { context: {} } as never,
    jumpToThread: vi.fn(),
    mailbox: "inbox",
    includeArchived: false,
    recordTriageEvent: vi.fn(),
  }));
  return { hook, reader, overrides };
}

describe("showing a Trust link", () => {
  it("expands and activates the message and asks its card for the link, with a new request each time", () => {
    const { hook, reader, overrides } = setup();
    const link = { href: "https://partnr.example/pay", text: "pay the invoice" };
    act(() => hook.result.current.showMessageLink("thread-1", "m2", link));
    expect(reader.setActiveMessageId).toHaveBeenCalledWith("m2");
    expect(overrides.get("m2")).toBe(true);
    const first = hook.result.current.linkReveal;
    expect(first).toEqual({ messageId: "m2", reveal: { ...link, key: expect.any(Number) } });

    act(() => hook.result.current.showMessageLink("thread-1", "m2", link));
    expect(hook.result.current.linkReveal?.reveal.key).not.toBe(first?.reveal.key);
  });

  it("ignores a link from a conversation that is no longer open", () => {
    const { hook, reader } = setup();
    act(() => hook.result.current.showMessageLink("thread-9", "m2", { href: "https://a.example/", text: "a" }));
    expect(reader.setActiveMessageId).not.toHaveBeenCalled();
    expect(hook.result.current.linkReveal).toBeNull();
  });
});
