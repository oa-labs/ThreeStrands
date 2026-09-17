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
