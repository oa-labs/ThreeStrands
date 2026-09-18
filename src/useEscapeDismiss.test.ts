import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useEscapeDismiss } from "./useEscapeDismiss";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useEscapeDismiss", () => {
  it("dismisses only the topmost mounted overlay", () => {
    const first = vi.fn();
    const second = vi.fn();
    const outer = renderHook(() => useEscapeDismiss(first));
    const inner = renderHook(() => useEscapeDismiss(second));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);

    inner.unmount();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(first).toHaveBeenCalledTimes(1);

    outer.unmount();
  });

  it("ignores composing, prevented, and unrelated key events", () => {
    const dismiss = vi.fn();
    const hook = renderHook(() => useEscapeDismiss(dismiss));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", cancelable: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", isComposing: true, cancelable: true }));
    const prevented = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    prevented.preventDefault();
    window.dispatchEvent(prevented);

    expect(dismiss).not.toHaveBeenCalled();
    hook.unmount();
  });

  it("uses the newest callback without replacing the listener", () => {
    const first = vi.fn();
    const second = vi.fn();
    const hook = renderHook(({ callback }) => useEscapeDismiss(callback), { initialProps: { callback: first } });
    hook.rerender({ callback: second });

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
    hook.unmount();
  });
});

