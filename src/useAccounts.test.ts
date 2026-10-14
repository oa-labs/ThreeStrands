import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { ACCOUNT_STATUS_REFRESH_MS, useAccounts } from "./useAccounts";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("refreshes nickname changes for the account switcher", async () => {
  const [account] = await mailClient.listAccounts();
  vi.spyOn(mailClient, "listAccounts").mockResolvedValue([{ ...account!, nickname: null }]);
  const save = vi.spyOn(mailClient, "setAccountNickname").mockResolvedValue();
  const { result } = renderHook(() => useAccounts(false));
  await waitFor(() => expect(result.current.accounts.length).toBeGreaterThan(0));
  vi.mocked(mailClient.listAccounts).mockResolvedValue([{ ...account!, nickname: "Work" }]);
  await act(async () => result.current.setAccountNickname(account!.email, "Work"));
  expect(save).toHaveBeenCalledWith(account!.email, "Work");
  expect(result.current.accounts[0]?.nickname).toBe("Work");
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

it("refreshes account status while Settings is closed and keeps accounts after a transient failure", async () => {
  const [account] = await mailClient.listAccounts();
  let status: "connected" | "needs_reauth" = "connected";
  const listAccounts = vi.spyOn(mailClient, "listAccounts").mockImplementation(async () => [
    { ...account!, status },
  ]);
  vi.useFakeTimers({ shouldAdvanceTime: true });

  const { result } = renderHook(() => useAccounts(false));
  await waitFor(() => expect(result.current.accounts[0]?.status).toBe("connected"));

  status = "needs_reauth";
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ACCOUNT_STATUS_REFRESH_MS);
  });
  expect(result.current.accounts[0]?.status).toBe("needs_reauth");

  listAccounts.mockRejectedValueOnce(new Error("temporary status failure"));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ACCOUNT_STATUS_REFRESH_MS);
  });
  expect(result.current.accounts[0]?.status).toBe("needs_reauth");

  status = "connected";
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ACCOUNT_STATUS_REFRESH_MS);
  });
  expect(result.current.accounts[0]?.status).toBe("connected");
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

it("keeps accounts set up on other devices out of this device's accounts and the selection", async () => {
  const [account] = await mailClient.listAccounts();
  const elsewhere = { ...account!, email: "personal@example.com", status: "needs_reauth" as const, onThisDevice: false };
  vi.spyOn(mailClient, "listAccounts").mockResolvedValue([{ ...account!, onThisDevice: true }, elsewhere]);
  localStorage.setItem("threestrands.settings.selectedAccountId", "personal@example.com");

  const { result } = renderHook(() => useAccounts(false));

  await waitFor(() => expect(result.current.availableAccounts.map((item) => item.email)).toEqual(["personal@example.com"]));
  expect(result.current.accounts.map((item) => item.email)).toEqual([account!.email]);
  expect(result.current.activeAccountId).toBeNull();
});

it("keeps accounts on other devices after this device's accounts when reordering", async () => {
  const [account] = await mailClient.listAccounts();
  const second = { ...account!, email: "second@example.com", onThisDevice: true };
  const elsewhere = { ...account!, email: "personal@example.com", onThisDevice: false };
  vi.spyOn(mailClient, "listAccounts").mockResolvedValue([{ ...account!, onThisDevice: true }, elsewhere, second]);
  const reorder = vi.spyOn(mailClient, "reorderAccounts").mockResolvedValue();
  const { result } = renderHook(() => useAccounts(false));
  await waitFor(() => expect(result.current.accounts).toHaveLength(2));

  await act(async () => result.current.reorderAccounts(["second@example.com", account!.email]));

  expect(reorder).toHaveBeenCalledWith(["second@example.com", account!.email, "personal@example.com"]);
});

it("announces an account from another device once, across launches", async () => {
  localStorage.clear();
  const [account] = await mailClient.listAccounts();
  const elsewhere = { ...account!, email: "personal@example.com", onThisDevice: false };
  vi.spyOn(mailClient, "listAccounts").mockResolvedValue([{ ...account!, onThisDevice: true }, elsewhere]);

  const first = renderHook(() => useAccounts(false));
  await waitFor(() => expect(first.result.current.newlyAvailableAccounts.map((item) => item.email)).toEqual(["personal@example.com"]));
  act(() => first.result.current.acknowledgeAvailableAccounts());
  expect(first.result.current.newlyAvailableAccounts).toEqual([]);
  first.unmount();

  const next = renderHook(() => useAccounts(false));
  await waitFor(() => expect(next.result.current.availableAccounts).toHaveLength(1));
  expect(next.result.current.newlyAvailableAccounts).toEqual([]);
});
