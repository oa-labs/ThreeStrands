import { describe, expect, it, vi } from "vitest";
import {
  FOREGROUND_DEBOUNCE_MS,
  FOREGROUND_IDLE_MS,
  createForegroundRefreshController,
} from "./foregroundRefresh";

function installClock(start = 1_000_000) {
  vi.useFakeTimers();
  vi.setSystemTime(start);
  return {
    now: () => Date.now(),
    setTimeout: (callback: () => void, ms: number) => window.setTimeout(callback, ms),
    clearTimeout: (id: number) => window.clearTimeout(id),
  };
}

describe("foreground refresh controller", () => {
  it("does not refresh until the app has been backgrounded past the poll interval", () => {
    const clock = installClock();
    const refresh = vi.fn();
    const controller = createForegroundRefreshController(refresh, clock);

    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).not.toHaveBeenCalled();

    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_IDLE_MS - 1);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).not.toHaveBeenCalled();

    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_IDLE_MS);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).toHaveBeenCalledTimes(1);

    controller.dispose();
    vi.useRealTimers();
  });

  it("cancels a pending refresh when focus flickers back to the background", () => {
    const clock = installClock();
    const refresh = vi.fn();
    const controller = createForegroundRefreshController(refresh, clock);

    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_IDLE_MS);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS - 1);
    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).not.toHaveBeenCalled();

    controller.dispose();
    vi.useRealTimers();
  });

  it("does not spam the server on rapid foreground cycles", () => {
    const clock = installClock();
    const refresh = vi.fn();
    const controller = createForegroundRefreshController(refresh, clock);

    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_IDLE_MS);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).toHaveBeenCalledTimes(1);

    controller.onBackground();
    vi.advanceTimersByTime(500);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).toHaveBeenCalledTimes(1);

    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_IDLE_MS);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).toHaveBeenCalledTimes(2);

    controller.dispose();
    vi.useRealTimers();
  });
});
