import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ANY_OPERATION, moveItem, useLiveStatus, useSettingsOperation } from "./settingsOperations";

const listenMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

describe("moveItem", () => {
  it("swaps an entry with its neighbour in either direction without mutating the input", () => {
    const items = ["a", "b", "c"];
    expect(moveItem(items, 0, 1)).toEqual(["b", "a", "c"]);
    expect(moveItem(items, 2, -1)).toEqual(["a", "c", "b"]);
    expect(items).toEqual(["a", "b", "c"]);
  });

  it("returns null when the move would leave the list", () => {
    expect(moveItem(["a", "b"], 0, -1)).toBeNull();
    expect(moveItem(["a", "b"], 1, 1)).toBeNull();
    expect(moveItem(["a"], 5, -1)).toBeNull();
  });
});

describe("useSettingsOperation", () => {
  it("marks the keyed item pending until the operation settles", async () => {
    const { result } = renderHook(() => useSettingsOperation());
    const operation = deferred<void>();

    act(() => result.current.runFor("you@example.com", () => operation.promise));
    expect(result.current.pending).toBe("you@example.com");
    expect(result.current.busy).toBe(true);

    await act(async () => { operation.resolve(); await operation.promise; });
    await waitFor(() => expect(result.current.pending).toBeNull());
    expect(result.current.error).toBeNull();
  });

  it("records the failure message and clears it when the next operation starts", async () => {
    const { result } = renderHook(() => useSettingsOperation());

    await act(async () => { result.current.run(() => Promise.reject(new Error("offline"))); });
    await waitFor(() => expect(result.current.error).toBe("offline"));
    expect(result.current.busy).toBe(false);

    const next = deferred<void>();
    act(() => result.current.run(() => next.promise));
    expect(result.current.pending).toBe(ANY_OPERATION);
    expect(result.current.error).toBeNull();
    await act(async () => { next.resolve(); await next.promise; });
  });

  it("refreshes after a successful act but not after a failed one", async () => {
    const refresh = vi.fn().mockResolvedValue(undefined);
    const { result } = renderHook(() => useSettingsOperation(refresh));

    await act(async () => { result.current.act(() => Promise.resolve()); });
    await waitFor(() => expect(refresh).toHaveBeenCalledTimes(1));

    await act(async () => { result.current.act(() => Promise.reject("sign-in cancelled")); });
    await waitFor(() => expect(result.current.error).toBe("sign-in cancelled"));
    expect(refresh).toHaveBeenCalledTimes(1);
  });
});

describe("useLiveStatus", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  afterEach(() => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    listenMock.mockReset();
  });

  it("loads on mount and reloads whenever the status event fires", async () => {
    let handler: (() => void) | undefined;
    listenMock.mockImplementation(async (_event: string, callback: () => void) => {
      handler = callback;
      return vi.fn();
    });
    const refresh = vi.fn().mockResolvedValue(undefined);

    renderHook(() => useLiveStatus("cloud-sync-status", refresh, vi.fn()));

    expect(refresh).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(listenMock).toHaveBeenCalledWith("cloud-sync-status", expect.any(Function)));
    handler!();
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("reports failures from both the initial load and event-driven reloads", async () => {
    let handler: (() => void) | undefined;
    listenMock.mockImplementation(async (_event: string, callback: () => void) => {
      handler = callback;
      return vi.fn();
    });
    const refresh = vi.fn().mockRejectedValue("status unavailable");
    const onError = vi.fn();

    renderHook(() => useLiveStatus("replicated-sync-status", refresh, onError));

    await waitFor(() => expect(onError).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(handler).toBeDefined());
    handler!();
    await waitFor(() => expect(onError).toHaveBeenCalledTimes(2));
    expect(onError).toHaveBeenCalledWith("status unavailable");
  });

  it("releases the subscription when unmounted before listen resolves", async () => {
    const subscription = deferred<() => void>();
    listenMock.mockReturnValue(subscription.promise);
    const unlisten = vi.fn();

    const { unmount } = renderHook(() => useLiveStatus("cloud-sync-status", vi.fn().mockResolvedValue(undefined), vi.fn()));
    unmount();
    await act(async () => { subscription.resolve(unlisten); await subscription.promise; });

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("does not subscribe outside the native app", () => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    const refresh = vi.fn().mockResolvedValue(undefined);

    renderHook(() => useLiveStatus("cloud-sync-status", refresh, vi.fn()));

    expect(refresh).toHaveBeenCalledTimes(1);
    expect(listenMock).not.toHaveBeenCalled();
  });
});
