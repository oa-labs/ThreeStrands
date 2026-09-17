import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { invokeWithTimeout, InvokeTimeoutError } from "./invokeWithTimeout";

describe("invokeWithTimeout", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("resolves with the underlying invoke's result", async () => {
    invokeMock.mockResolvedValue("ok");
    await expect(invokeWithTimeout("some_command", {}, 1_000)).resolves.toBe("ok");
  });

  it("rejects with the underlying invoke's error", async () => {
    invokeMock.mockRejectedValue(new Error("backend failure"));
    await expect(invokeWithTimeout("some_command", {}, 1_000)).rejects.toThrow("backend failure");
  });

  it("rejects with InvokeTimeoutError if the command never resolves", async () => {
    invokeMock.mockReturnValue(new Promise(() => {}));
    const pending = invokeWithTimeout("stuck_command", {}, 1_000);
    const assertion = expect(pending).rejects.toBeInstanceOf(InvokeTimeoutError);
    await vi.advanceTimersByTimeAsync(1_000);
    await assertion;
  });

  it("does not fire the timeout once the command has already resolved", async () => {
    invokeMock.mockResolvedValue("done");
    await invokeWithTimeout("fast_command", {}, 1_000);
    // If the timer weren't cleared, advancing past it would trigger an
    // unhandled rejection from the already-settled promise.
    await vi.advanceTimersByTimeAsync(2_000);
  });
});
