import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { FOREGROUND_DEBOUNCE_MS, FOREGROUND_IDLE_MS } from "./foregroundRefresh";
import { useMailRefresh } from "./useMailRefresh";

function renderRefresh(activeAccountId: string | null = null) {
  const loadThreads = vi.fn(async () => {});
  return renderHook(() => useMailRefresh({
    activeAccountId,
    query: "",
    loadThreadsRef: { current: loadThreads },
    setSyncStatus: vi.fn(),
    refreshTaskIndicators: vi.fn(async () => {}),
  }));
}

describe("foreground mail refresh", () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
  });

  it("checks for new mail after the window has been in the background", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const sync = vi.spyOn(mailClient, "sync");
    renderRefresh();

    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(FOREGROUND_IDLE_MS);
    });
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(FOREGROUND_DEBOUNCE_MS);
    });

    expect(sync).toHaveBeenCalledWith({ catchUp: true });
  });

  it("syncs every account when the user asks to refresh", async () => {
    const sync = vi.spyOn(mailClient, "sync");
    const { result } = renderRefresh();

    act(() => result.current.refreshMail());

    expect(sync).toHaveBeenCalledWith({ catchUp: false });
  });

  it.each(["work@example.com", null])("pull refresh checks the selected account scope (%s)", async (accountId) => {
    const sync = vi.spyOn(mailClient, "sync");
    const { result } = renderRefresh(accountId);
    await act(async () => { await result.current.refreshMailbox(); });
    expect(sync).toHaveBeenCalledWith(accountId ? { catchUp: false, accountId } : { catchUp: false });
  });

  it("reloads the current search after a slow server refresh finishes", async () => {
    let finish!: (status: Awaited<ReturnType<typeof mailClient.sync>>) => void;
    const status = await mailClient.syncStatus();
    vi.spyOn(mailClient, "sync").mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const loadThreads = vi.fn(async () => {});
    const options = { activeAccountId: "work@example.com", loadThreadsRef: { current: loadThreads }, setSyncStatus: vi.fn(), refreshTaskIndicators: vi.fn(async () => {}) };
    const { result, rerender } = renderHook(({ query }) => useMailRefresh({ ...options, query }), { initialProps: { query: "old" } });
    let refresh!: Promise<void>;
    act(() => { refresh = result.current.refreshMailbox(); });
    rerender({ query: "current" });
    await act(async () => { finish(status); await refresh; });
    expect(loadThreads).toHaveBeenCalledWith("current");
  });

  it("does not sync on a brief focus flicker", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const sync = vi.spyOn(mailClient, "sync");
    renderRefresh();

    await act(async () => {
      window.dispatchEvent(new Event("blur"));
      await vi.advanceTimersByTimeAsync(200);
      window.dispatchEvent(new Event("focus"));
      await vi.advanceTimersByTimeAsync(FOREGROUND_DEBOUNCE_MS);
    });

    expect(sync).not.toHaveBeenCalled();
  });
});
