import { afterEach, describe, expect, it, vi } from "vitest";
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
  afterEach(() => {
    vi.useRealTimers();
  });

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
  });

  it("does not refresh again after a brief return to the background", () => {
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
  });

  it("judges each absence on its own rather than adding up short ones", () => {
    const clock = installClock();
    const refresh = vi.fn();
    const controller = createForegroundRefreshController(refresh, clock);

    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_IDLE_MS - 1_000);
    controller.onForeground();
    vi.advanceTimersByTime(60_000);

    controller.onBackground();
    vi.advanceTimersByTime(1_000);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).not.toHaveBeenCalled();

    controller.dispose();
  });

  it("treats a focus flicker shorter than the debounce as one continuous absence", () => {
    const clock = installClock();
    const refresh = vi.fn();
    const controller = createForegroundRefreshController(refresh, clock);

    controller.onBackground();
    vi.advanceTimersByTime(FOREGROUND_IDLE_MS - 1_000);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS - 1);
    controller.onBackground();
    vi.advanceTimersByTime(1_000);
    controller.onForeground();
    vi.advanceTimersByTime(FOREGROUND_DEBOUNCE_MS);
    expect(refresh).toHaveBeenCalledTimes(1);

    controller.dispose();
  });
});
