import { act, renderHook } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MailboxKind } from "./commands";
import { useMailNavigation } from "./useMailNavigation";
import { STARRED_INBOX_ID } from "./starredInbox";

const custom = { id: "work", name: "Work", accountId: "me@example.com", matchKind: "domain" as const, matchValue: "example.org", sortOrder: 0, createdAt: "2026-11-05" };
const noop = vi.fn();

afterEach(() => localStorage.clear());

function setup(initial = { count: 0, enabled: true, account: "me@example.com" as string | null }) {
  return renderHook(({ count, enabled, account }) => {
    const [mailbox, setMailbox] = useState<MailboxKind>("inbox");
    const [activeSplitInboxId, setActiveSplitInboxId] = useState<string | null>(null);
    const contextOpenedThreadRef = useRef<string | null>(null);
    const messageStackRef = useRef<HTMLDivElement | null>(null);
    const messageRefs = useRef(new Map<string, HTMLElement>());
    const selectedThreadRowRef = useRef<HTMLButtonElement | null>(null);
    const navigation = useMailNavigation({
      mailboxThreads: { threads: [], query: "", setQuery: noop, searchOpen: false, setSearchOpen: noop, contextOpenedThreadRef },
      threadDetail: { visibleDetail: null, setDetail: noop, setDetailLoading: noop },
      reader: { messageExpansionOverrides: new Map(), setMessageExpansionOverrides: noop, messageStackRef, messageRefs },
      workspaces: { rightWorkspace: null, setRightWorkspace: noop },
      correspondence: { context: { openInbox: noop, openDrafts: noop, openOutbox: noop } },
      splitInboxCatalog: { splitInboxes: [custom], loaded: true },
      mailbox, setMailbox, activeSplitInboxId, setActiveSplitInboxId,
      activeAccountId: account, setActiveAccountId: noop, selectedId: null, setSelectedId: noop,
      selectedThreadRowRef, setNotice: noop, starredCount: count, showStarredInbox: enabled,
    });
    return { ...navigation, mailbox, activeSplitInboxId };
  }, { initialProps: initial });
}

describe("automatic Starred tab navigation", () => {
  it("appears without navigating, stays when emptied while active, and hides after leaving", () => {
    const { result, rerender } = setup();
    expect(result.current.accountSplitInboxes.map((tab) => tab.id)).toEqual(["work"]);
    rerender({ count: 2, enabled: true, account: "me@example.com" });
    expect(result.current.mailbox).toBe("inbox");
    expect(result.current.accountSplitInboxes.map((tab) => tab.id)).toEqual([STARRED_INBOX_ID, "work"]);
    act(() => result.current.goToSplitTab(STARRED_INBOX_ID));
    expect(result.current.mailboxTitle).toBe("Starred");
    rerender({ count: 0, enabled: true, account: "me@example.com" });
    expect(result.current.activeSplitInboxId).toBe(STARRED_INBOX_ID);
    expect(result.current.accountSplitInboxes[0]?.id).toBe(STARRED_INBOX_ID);
    act(() => result.current.goToInboxTab());
    expect(result.current.accountSplitInboxes.map((tab) => tab.id)).toEqual(["work"]);
  });

  it("leaves other folders open when an empty Starred tab hides", () => {
    const { result, rerender } = setup({ count: 1, enabled: true, account: "me@example.com" });
    act(() => result.current.goToSplitTab(STARRED_INBOX_ID));
    rerender({ count: 0, enabled: true, account: "me@example.com" });
    act(() => result.current.openFolder("allMail"));
    expect(result.current.mailbox).toBe("allMail");
    expect(result.current.accountSplitInboxes.map((tab) => tab.id)).toEqual(["work"]);
  });

  it("supports keyboard cycling, merged accounts, and disabling while active", () => {
    const { result, rerender } = setup({ count: 3, enabled: true, account: null });
    expect(result.current.accountSplitInboxes.map((tab) => tab.id)).toEqual([STARRED_INBOX_ID]);
    act(() => result.current.goToNextSplitTab());
    expect(result.current.mailboxTitle).toBe("Starred");
    act(() => result.current.goToNextSplitTab());
    expect(result.current.mailbox).toBe("inbox");
    act(() => result.current.goToPreviousSplitTab());
    expect(result.current.mailboxTitle).toBe("Starred");
    rerender({ count: 3, enabled: false, account: null });
    expect(result.current.mailbox).toBe("inbox");
    expect(result.current.accountSplitInboxes).toEqual([]);
  });
});
