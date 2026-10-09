import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AccountsSettings } from "./AccountsSettings";
import type { Account } from "./domain";

const account: Account = {
  email: "me@example.com",
  displayName: null,
  color: "#112233",
  status: "connected",
  provider: "gmail",
  sortOrder: 0,
  connectedAt: "2026-10-08T12:00:00Z",
  lastSyncedAt: null,
};

function renderAccounts(onSetColor: (email: string, color: string) => Promise<void>) {
  return render(<AccountsSettings
    authStatus={null}
    accounts={[account]}
    onAdd={vi.fn()}
    onRemove={vi.fn()}
    onRemoveEverywhere={vi.fn()}
    onReconnect={vi.fn()}
    onSetDisplayName={vi.fn()}
    onSetColor={onSetColor}
    onReorder={vi.fn()}
  />);
}

describe("mail account settings", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("saves only the latest color after continuous picker changes", async () => {
    const onSetColor = vi.fn().mockResolvedValue(undefined);
    renderAccounts(onSetColor);
    const picker = screen.getByLabelText(`Color for ${account.email}`);
    fireEvent.change(picker, { target: { value: "#223344" } });
    act(() => vi.advanceTimersByTime(150));
    fireEvent.change(picker, { target: { value: "#334455" } });
    act(() => vi.advanceTimersByTime(150));
    expect(picker).toHaveValue("#334455");
    expect(onSetColor).not.toHaveBeenCalled();

    await act(async () => { vi.advanceTimersByTime(50); });
    expect(onSetColor).toHaveBeenCalledTimes(1);
    expect(onSetColor).toHaveBeenCalledWith(account.email, "#334455");
  });

  it("cancels a pending color save when the section unmounts", async () => {
    const onSetColor = vi.fn().mockResolvedValue(undefined);
    const { unmount } = renderAccounts(onSetColor);
    fireEvent.change(screen.getByLabelText(`Color for ${account.email}`), { target: { value: "#223344" } });
    unmount();

    await act(async () => { vi.advanceTimersByTime(200); });
    expect(onSetColor).not.toHaveBeenCalled();
  });
});
