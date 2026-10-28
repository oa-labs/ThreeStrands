import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NOTICE_TIMEOUT_MS, useNotice } from "./useNotice";

describe("useNotice", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("dismisses itself after the notice timeout", () => {
    const { result } = renderHook(() => useNotice());
    act(() => result.current[1]({ message: "Conversation archived" }));
    expect(result.current[0]?.message).toBe("Conversation archived");

    act(() => vi.advanceTimersByTime(NOTICE_TIMEOUT_MS - 500));
    expect(result.current[0]).not.toBeNull();

    act(() => vi.advanceTimersByTime(500));
    expect(result.current[0]).toBeNull();
  });

  it("can still be dismissed manually before the timeout", () => {
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    const { result } = renderHook(() => useNotice());
    act(() => result.current[1]({ message: "Conversation archived" }));

    act(() => result.current[1](null));
    expect(result.current[0]).toBeNull();
    expect(clearTimeout).toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("keeps a replacement notice on screen for its own full timeout", () => {
    const { result } = renderHook(() => useNotice());
    act(() => result.current[1]({ message: "Conversation archived" }));
    act(() => vi.advanceTimersByTime(NOTICE_TIMEOUT_MS - 1000));
    act(() => result.current[1]({ message: "Conversation archived" }));

    act(() => vi.advanceTimersByTime(1500));
    expect(result.current[0]?.message).toBe("Conversation archived");

    act(() => vi.advanceTimersByTime(NOTICE_TIMEOUT_MS));
    expect(result.current[0]).toBeNull();
  });

  it("clears the pending dismiss timer when its owner unmounts", () => {
    const setTimeout = vi.spyOn(window, "setTimeout");
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    const { result, unmount } = renderHook(() => useNotice());
    act(() => result.current[1]({ message: "Conversation archived" }));
    const dismissTimer = setTimeout.mock.results
      .filter((_, index) => setTimeout.mock.calls[index][1] === NOTICE_TIMEOUT_MS)
      .map((entry) => entry.value)
      .at(-1);
    expect(dismissTimer).toBeDefined();

    unmount();
    expect(clearTimeout).toHaveBeenCalledWith(dismissTimer);
  });
});
