import { act, cleanup, renderHook } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { SearchReport, Thread } from "./domain";
import { REMOTE_SEARCH_DELAY_MS, useMailboxThreads } from "./useMailboxThreads";

const listenMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

const completeReport: SearchReport = { status: { kind: "complete" }, unsyncedMailboxes: 0, skippedOverCap: 0, serverSearchedAccounts: 1 };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

async function flushReload() {
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
}

describe("useMailboxThreads", () => {
  let threads: Thread[];

  beforeEach(async () => {
    threads = await mailClient.listThreads();
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  function setup(initialSelection: string | null = null) {
    const refreshUnreadCounts = vi.fn();
    const refreshMailboxUnreadCounts = vi.fn();
    const hook = renderHook(({ accountId }: { accountId: string }) => {
      const [selectedId, setSelectedId] = useState(initialSelection);
      const mailbox = useMailboxThreads({
        activeAccountId: accountId,
        mailbox: "inbox",
        activeSplitInboxId: null,
        setSelectedId,
        refreshUnreadCounts,
        refreshMailboxUnreadCounts,
      });
      return { ...mailbox, selectedId };
    }, { initialProps: { accountId: "first@example.com" } });
    return { ...hook, refreshUnreadCounts, refreshMailboxUnreadCounts };
  }

  it("invalidates an old account's response before the replacement reload starts", async () => {
    const oldPage = deferred<{ threads: Thread[]; hasMore: boolean }>();
    vi.spyOn(mailClient, "listThreadsPage")
      .mockReturnValueOnce(oldPage.promise)
      .mockResolvedValueOnce({ threads: [threads[1]!], hasMore: false });
    const { result, rerender, refreshUnreadCounts } = setup();
    await flushReload();

    rerender({ accountId: "second@example.com" });
    await act(async () => { oldPage.resolve({ threads: [threads[0]!], hasMore: true }); });
    expect(result.current.threads).toEqual([]);
    expect(result.current.selectedId).toBeNull();
    expect(refreshUnreadCounts).not.toHaveBeenCalled();

    await flushReload();
    expect(result.current.threads).toEqual([threads[1]]);
    expect(result.current.selectedId).toBe(threads[1]!.id);
    expect(result.current.hasMoreResults).toBe(false);
  });

  it("does not append an old page after switching accounts", async () => {
    const more = deferred<{ threads: Thread[]; hasMore: boolean }>();
    vi.spyOn(mailClient, "listThreadsPage")
      .mockResolvedValueOnce({ threads: [threads[0]!], hasMore: true })
      .mockReturnValueOnce(more.promise)
      .mockResolvedValueOnce({ threads: [threads[2]!], hasMore: false });
    const { result, rerender } = setup();
    await flushReload();
    let loading!: Promise<void>;
    act(() => { loading = result.current.loadMoreResults(); });

    rerender({ accountId: "second@example.com" });
    await flushReload();
    await act(async () => {
      more.resolve({ threads: [threads[1]!], hasMore: true });
      await loading;
    });
    expect(result.current.threads).toEqual([threads[2]]);
    expect(result.current.hasMoreResults).toBe(false);
    expect(result.current.loadingMoreState).toBe(false);
  });

  it("searches all mail by default and only the inbox when narrowed", async () => {
    const searchThreads = vi.spyOn(mailClient, "searchThreads").mockResolvedValue([]);
    const backfill = vi.spyOn(mailClient, "backfillSearchThreads").mockResolvedValue(completeReport);
    const { result } = setup();
    await flushReload();

    act(() => result.current.setQuery("roadmap"));
    await flushReload();
    expect(result.current.includeArchived).toBe(true);
    expect(searchThreads).toHaveBeenLastCalledWith(
      expect.objectContaining({ query: "roadmap", includeArchived: true }),
      "first@example.com",
    );
    await act(async () => { await vi.advanceTimersByTimeAsync(REMOTE_SEARCH_DELAY_MS); });
    expect(backfill).toHaveBeenCalledWith("roadmap", "first@example.com");

    searchThreads.mockClear();
    backfill.mockClear();
    act(() => result.current.setInboxOnly(true));
    await flushReload();
    expect(searchThreads).toHaveBeenLastCalledWith(
      expect.objectContaining({ query: "roadmap", includeArchived: false }),
      "first@example.com",
    );
    await act(async () => { await vi.advanceTimersByTimeAsync(REMOTE_SEARCH_DELAY_MS + 100); });
    expect(backfill).not.toHaveBeenCalled();
  });

  it("waits for the query to hold still before searching the server", async () => {
    const searchThreads = vi.spyOn(mailClient, "searchThreads").mockResolvedValue([]);
    const backfill = vi.spyOn(mailClient, "backfillSearchThreads").mockResolvedValue(completeReport);
    const { result } = setup();
    await flushReload();

    act(() => result.current.setQuery("road"));
    await flushReload();
    // The local search for the first query lands right away...
    expect(searchThreads).toHaveBeenLastCalledWith(expect.objectContaining({ query: "road" }), "first@example.com");
    expect(backfill).not.toHaveBeenCalled();

    // ...but the user keeps typing before the pause ends, so only the final
    // query reaches the server.
    await act(async () => { await vi.advanceTimersByTimeAsync(REMOTE_SEARCH_DELAY_MS - 100); });
    act(() => result.current.setQuery("roadmap"));
    await flushReload();
    await act(async () => { await vi.advanceTimersByTimeAsync(REMOTE_SEARCH_DELAY_MS); });
    expect(backfill).toHaveBeenCalledTimes(1);
    expect(backfill).toHaveBeenCalledWith("roadmap", "first@example.com");
  });

  it("keeps a conversation opened by a context jump selected outside the current page", async () => {
    vi.spyOn(mailClient, "listThreadsPage").mockResolvedValue({ threads: [threads[0]!], hasMore: false });
    const { result } = setup("context-thread");
    result.current.contextOpenedThreadRef.current = "context-thread";
    await flushReload();
    expect(result.current.selectedId).toBe("context-thread");

    result.current.contextOpenedThreadRef.current = null;
    await act(async () => { await result.current.loadThreads(""); });
    expect(result.current.selectedId).toBe(threads[0]!.id);
  });

  describe("background unread count updates", () => {
    let emitUnreadCountsChanged: (accountId: string) => void;

    beforeEach(() => {
      emitUnreadCountsChanged = () => {};
      Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
      listenMock.mockImplementation(async (event: string, handler: (event: { payload: string }) => void) => {
        if (event === "unread-counts-changed") emitUnreadCountsChanged = (payload) => handler({ payload });
        return vi.fn();
      });
    });

    afterEach(() => {
      Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
      listenMock.mockReset();
    });

    it("refreshes the sidebar unread badges when a background account reports a sync", async () => {
      vi.spyOn(mailClient, "listThreadsPage").mockResolvedValue({ threads, hasMore: false });
      const { refreshUnreadCounts, refreshMailboxUnreadCounts } = setup();
      await flushReload();
      refreshUnreadCounts.mockClear();
      refreshMailboxUnreadCounts.mockClear();

      act(() => emitUnreadCountsChanged("other@example.com"));
      expect(refreshUnreadCounts).toHaveBeenCalled();
      expect(refreshMailboxUnreadCounts).toHaveBeenCalled();
    });

    it("does not refetch the thread list for a different account's sync while a single account is active", async () => {
      const listThreadsPage = vi.spyOn(mailClient, "listThreadsPage").mockResolvedValue({ threads, hasMore: false });
      setup();
      await flushReload();
      listThreadsPage.mockClear();

      // The badge is account-agnostic to refresh (cheap, and background
      // accounts' sidebar totals still need to catch up), but the open thread
      // list belongs to the active account and shouldn't reload for mail that
      // landed in an account the user isn't looking at.
      await act(async () => emitUnreadCountsChanged("other@example.com"));
      expect(listThreadsPage).not.toHaveBeenCalled();

      await act(async () => emitUnreadCountsChanged("first@example.com"));
      expect(listThreadsPage).toHaveBeenCalled();
    });
  });
});
