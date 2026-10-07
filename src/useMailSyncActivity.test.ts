import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listenMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

import { mailClient } from "./data/client";
import { useMailSyncActivity, type MailSyncActivityEvent } from "./useMailSyncActivity";

describe("useMailSyncActivity", () => {
  let emit: (payload: MailSyncActivityEvent) => void;
  const unlisten = vi.fn();

  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    listenMock.mockImplementation(async (_event: string, handler: (event: { payload: MailSyncActivityEvent }) => void) => {
      emit = (payload) => handler({ payload });
      return unlisten;
    });
  });

  afterEach(() => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    vi.restoreAllMocks();
    listenMock.mockReset();
    unlisten.mockReset();
  });

  it("is active until every account's sync has finished, reporting each finish", async () => {
    vi.spyOn(mailClient, "mailSyncActivity").mockResolvedValue([]);
    const onFinished = vi.fn();
    const { result } = renderHook(() => useMailSyncActivity(onFinished));
    await waitFor(() => expect(listenMock).toHaveBeenCalledWith("mail-sync-activity", expect.any(Function)));
    expect(result.current).toBe(false);

    act(() => emit({ accountId: "a@example.com", active: true }));
    act(() => emit({ accountId: "b@example.com", active: true }));
    expect(result.current).toBe(true);

    act(() => emit({ accountId: "a@example.com", active: false }));
    expect(result.current).toBe(true);
    expect(onFinished).toHaveBeenCalledWith("a@example.com");

    act(() => emit({ accountId: "b@example.com", active: false }));
    expect(result.current).toBe(false);
    expect(onFinished).toHaveBeenCalledTimes(2);
  });

  it("shows a launch sync that started before the listener existed", async () => {
    vi.spyOn(mailClient, "mailSyncActivity").mockResolvedValue(["a@example.com"]);
    const { result } = renderHook(() => useMailSyncActivity());

    await waitFor(() => expect(result.current).toBe(true));
    act(() => emit({ accountId: "a@example.com", active: false }));
    expect(result.current).toBe(false);
  });

  it("does not let a stale snapshot revive a sync whose finish already arrived", async () => {
    let resolveSnapshot: (accountIds: string[]) => void = () => {};
    vi.spyOn(mailClient, "mailSyncActivity").mockReturnValue(new Promise((resolve) => { resolveSnapshot = resolve; }));
    const { result } = renderHook(() => useMailSyncActivity());
    await waitFor(() => expect(mailClient.mailSyncActivity).toHaveBeenCalled());

    act(() => emit({ accountId: "a@example.com", active: false }));
    await act(async () => resolveSnapshot(["a@example.com"]));

    expect(result.current).toBe(false);
  });

  it("stays idle outside the desktop app and stops listening on unmount", async () => {
    vi.spyOn(mailClient, "mailSyncActivity").mockResolvedValue([]);
    const { unmount } = renderHook(() => useMailSyncActivity());
    await waitFor(() => expect(listenMock).toHaveBeenCalled());
    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalled());

    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    listenMock.mockClear();
    const { result } = renderHook(() => useMailSyncActivity());
    expect(result.current).toBe(false);
    expect(listenMock).not.toHaveBeenCalled();
  });
});
