import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import {
  BOUNDED_LOCAL_READ,
  invokeWithPolicy,
  InvokeTimeoutError,
  WAIT_FOR_NATIVE_COMPLETION,
} from "./invoke";

describe("invokeWithPolicy", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("resolves with the underlying invoke's result", async () => {
    invokeMock.mockResolvedValue("ok");
    await expect(
      invokeWithPolicy("some_command", {}, BOUNDED_LOCAL_READ),
    ).resolves.toBe("ok");
  });

  it("rejects with the underlying invoke's error", async () => {
    invokeMock.mockRejectedValue(new Error("backend failure"));
    await expect(
      invokeWithPolicy("some_command", {}, BOUNDED_LOCAL_READ),
    ).rejects.toThrow("backend failure");
  });

  it("times out a bounded local read", async () => {
    invokeMock.mockReturnValue(new Promise(() => {}));
    const pending = invokeWithPolicy("stuck_read", {}, {
      timeout: "bounded-read",
      timeoutMs: 1_000,
    });
    const assertion = expect(pending).rejects.toBeInstanceOf(InvokeTimeoutError);
    await vi.advanceTimersByTimeAsync(1_000);
    await assertion;
  });

  it("does not time out work which must wait for native completion", async () => {
    let resolveNative: ((value: string) => void) | undefined;
    invokeMock.mockReturnValue(new Promise<string>((resolve) => {
      resolveNative = resolve;
    }));

    const pending = invokeWithPolicy(
      "interactive_or_long_command",
      {},
      WAIT_FOR_NATIVE_COMPLETION,
    );
    await vi.advanceTimersByTimeAsync(180_000);
    resolveNative?.("done");

    await expect(pending).resolves.toBe("done");
  });

  it("clears a bounded read's timer once the command resolves", async () => {
    invokeMock.mockResolvedValue("done");
    await invokeWithPolicy("fast_read", {}, {
      timeout: "bounded-read",
      timeoutMs: 1_000,
    });
    await vi.advanceTimersByTimeAsync(2_000);
  });
});
