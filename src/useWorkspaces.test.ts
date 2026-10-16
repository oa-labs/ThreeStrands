import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ThreadDetail } from "./domain";
import { useWorkspaces } from "./useWorkspaces";

vi.mock("./data/client", () => ({ mailClient: {
  listKeepInTouch: vi.fn().mockResolvedValue([]),
  listCalendarAccounts: vi.fn().mockResolvedValue([]),
  listCalendarOptions: vi.fn().mockResolvedValue([]),
} }));

const thread = (id: string) => ({ thread: { id }, messages: [] }) as unknown as ThreadDetail;

describe("the person picked for the context panel", () => {
  function setup() {
    const readingMessageIdRef = { current: "m1" as string | null };
    const hook = renderHook((props: { detail: ThreadDetail; reading: string | null }) => useWorkspaces({
      settingsOpen: false,
      settingsSection: "general" as never,
      visibleDetail: props.detail,
      readingMessageId: props.reading,
      readingMessageIdRef,
      openSettingsAt: vi.fn(),
      setNotice: vi.fn(),
    }), { initialProps: { detail: thread("t1"), reading: "m1" } });
    return { hook, readingMessageIdRef };
  }

  it("holds while the reader stays on the message it was picked from", () => {
    const { hook } = setup();
    act(() => hook.result.current.contactCardActions.onSelectPerson("ana@partner.org"));
    expect(hook.result.current.contextPersonEmail).toBe("ana@partner.org");
    expect(hook.result.current.contactCardActions.selectedEmail).toBe("ana@partner.org");
  });

  it("lets go once the reader moves to another message or conversation, and can be picked again", () => {
    const { hook, readingMessageIdRef } = setup();
    act(() => hook.result.current.contactCardActions.onSelectPerson("ana@partner.org"));
    hook.rerender({ detail: thread("t1"), reading: "m2" });
    expect(hook.result.current.contextPersonEmail).toBeNull();

    readingMessageIdRef.current = "m2";
    act(() => hook.result.current.contactCardActions.onSelectPerson("ana@partner.org"));
    expect(hook.result.current.contextPersonEmail).toBe("ana@partner.org");

    hook.rerender({ detail: thread("t2"), reading: "m2" });
    expect(hook.result.current.contextPersonEmail).toBeNull();
  });
});
