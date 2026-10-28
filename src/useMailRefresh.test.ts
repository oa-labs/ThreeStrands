import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { FOREGROUND_DEBOUNCE_MS, FOREGROUND_IDLE_MS } from "./foregroundRefresh";
import { useMailRefresh } from "./useMailRefresh";

function renderRefresh() {
  const loadThreads = vi.fn(async () => {});
  return renderHook(() => useMailRefresh({
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

    expect(sync).toHaveBeenCalled();
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
