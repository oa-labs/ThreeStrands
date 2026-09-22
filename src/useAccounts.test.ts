import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { ACCOUNT_STATUS_REFRESH_MS, useAccounts } from "./useAccounts";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("refreshes an open account manager when credentials need reconnection", async () => {
  const [account] = await mailClient.listAccounts();
  let status: "connected" | "needs_reauth" = "connected";
  vi.spyOn(mailClient, "listAccounts").mockImplementation(async () => [
    { ...account!, status },
  ]);
  vi.useFakeTimers({ shouldAdvanceTime: true });

  const { result } = renderHook(() => useAccounts(true));
  await waitFor(() => expect(result.current.accounts[0]?.status).toBe("connected"));

  status = "needs_reauth";
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ACCOUNT_STATUS_REFRESH_MS);
  });

  expect(result.current.accounts[0]?.status).toBe("needs_reauth");
});

it("clears the active account when it is removed and reports that it was active", async () => {
  const [account] = await mailClient.listAccounts();
  vi.spyOn(mailClient, "removeAccount").mockResolvedValue();
  const { result } = renderHook(() => useAccounts(false));
  await waitFor(() => expect(result.current.accounts.length).toBeGreaterThan(0));
  act(() => result.current.setActiveAccountId(account!.email));

  let outcome: { wasActive: boolean } | undefined;
  await act(async () => {
    outcome = await result.current.removeAccount(account!.email);
  });

  expect(mailClient.removeAccount).toHaveBeenCalledWith(account!.email);
  expect(outcome).toEqual({ wasActive: true });
  expect(result.current.activeAccountId).toBeNull();
});

it("keeps the active account when a different account is removed", async () => {
  const [account] = await mailClient.listAccounts();
  vi.spyOn(mailClient, "removeAccount").mockResolvedValue();
  const { result } = renderHook(() => useAccounts(false));
  await waitFor(() => expect(result.current.accounts.length).toBeGreaterThan(0));
  act(() => result.current.setActiveAccountId(account!.email));

  let outcome: { wasActive: boolean } | undefined;
  await act(async () => {
    outcome = await result.current.removeAccount("other@example.com");
  });

  expect(outcome).toEqual({ wasActive: false });
  expect(result.current.activeAccountId).toBe(account!.email);
});

it("refreshes account, auth, and sync state even when reconnecting fails", async () => {
  const [account] = await mailClient.listAccounts();
  const failure = new Error("sign-in cancelled");
  vi.spyOn(mailClient, "reconnectAccount").mockRejectedValue(failure);
  const { result } = renderHook(() => useAccounts(false));
  await waitFor(() => expect(result.current.syncStatus).not.toBeNull());

  const listAccounts = vi.spyOn(mailClient, "listAccounts").mockResolvedValue([{ ...account!, status: "needs_reauth" }]);
  const authStatus = vi.spyOn(mailClient, "googleAuthStatus").mockResolvedValue({ configured: true, connected: false });
  const syncStatus = vi.spyOn(mailClient, "syncStatus");

  await act(async () => {
    await expect(result.current.reconnectAccount(account!.email)).rejects.toBe(failure);
  });

  expect(listAccounts).toHaveBeenCalled();
  expect(authStatus).toHaveBeenCalled();
  expect(syncStatus).toHaveBeenCalled();
  expect(result.current.accounts[0]?.status).toBe("needs_reauth");
  expect(result.current.authStatus).toEqual({ configured: true, connected: false });
});
