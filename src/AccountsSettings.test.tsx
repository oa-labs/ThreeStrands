import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AccountsSettings } from "./AccountsSettings";
import type { Account } from "./domain";
import type { ComponentProps } from "react";

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

function renderAccounts(
  onSetColor: (email: string, color: string) => Promise<void>,
  overrides: Partial<ComponentProps<typeof AccountsSettings>> = {},
) {
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
    {...overrides}
  />);
}

describe("mail account settings", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it.each<Account["status"]>(["connected", "needs_reauth"])("labels each card with its account provider when %s", (status) => {
    renderAccounts(vi.fn(), {
      accounts: [
        { ...account, status },
        { ...account, email: "imap@gmail.com", provider: "imap", status },
      ],
    });

    const cards = screen.getAllByRole("listitem");
    expect(within(cards[0]!).getByText("Gmail")).toBeVisible();
    expect(within(cards[0]!).queryByText("IMAP")).not.toBeInTheDocument();
    expect(within(cards[1]!).getByText("IMAP")).toBeVisible();
    expect(within(cards[1]!).queryByText("Gmail")).not.toBeInTheDocument();
    for (const card of cards) {
      expect(within(card).getByText(status === "connected" ? "Connected" : "Needs reconnect")).toBeVisible();
      expect(within(card).queryByRole("button", { name: "Reconnect" }) !== null).toBe(status === "needs_reauth");
    }
  });

  it("offers Gmail and IMAP from one Add Account button without starting sign-in", () => {
    const onAdd = vi.fn();
    renderAccounts(vi.fn(), { onAdd });
    const add = screen.getByRole("button", { name: "Add Account" });
    expect(add).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Gmail" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Add IMAP/ })).not.toBeInTheDocument();
    fireEvent.click(add);
    expect(add).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Gmail" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "IMAP" })).toBeInTheDocument();
    expect(onAdd).not.toHaveBeenCalled();
    fireEvent.click(add);
    expect(add).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("group", { name: "Choose an account type" })).not.toBeInTheDocument();
  });

  it("prevents duplicate sign-in and provider switching while Gmail sign-in is pending", async () => {
    let finish!: () => void;
    const onAdd = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    renderAccounts(vi.fn(), { onAdd });
    fireEvent.click(screen.getByRole("button", { name: "Add Account" }));
    const gmail = screen.getByRole("button", { name: "Gmail" });
    fireEvent.click(gmail);
    expect(screen.getByRole("button", { name: "Waiting for Google…" })).toBeDisabled();
    expect(gmail).toBeDisabled();
    expect(screen.getByRole("button", { name: "IMAP" })).toBeDisabled();
    fireEvent.click(gmail);
    expect(onAdd).toHaveBeenCalledTimes(1);
    await act(async () => finish());
    expect(screen.getByRole("button", { name: "Add Account" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: "Add Account" })).toHaveFocus();
    expect(screen.queryByRole("group", { name: "Choose an account type" })).not.toBeInTheDocument();
  });

  it("keeps the provider choice available after failed Gmail sign-in", async () => {
    const onAdd = vi.fn().mockRejectedValue(new Error("Sign-in cancelled"));
    renderAccounts(vi.fn(), { onAdd });
    fireEvent.click(screen.getByRole("button", { name: "Add Account" }));
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Gmail" })); });
    expect(screen.getByRole("alert")).toHaveTextContent("Sign-in cancelled");
    expect(screen.getByRole("button", { name: "Gmail" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "IMAP" })).toBeEnabled();
  });

  it("focuses IMAP setup and returns focus to Add Account on cancellation", () => {
    const onAdd = vi.fn();
    renderAccounts(vi.fn(), { onAdd });
    const add = screen.getByRole("button", { name: "Add Account" });
    fireEvent.click(add);
    fireEvent.click(screen.getByRole("button", { name: "IMAP" }));
    expect(screen.getByLabelText("Email Address")).toHaveFocus();
    expect(onAdd).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByLabelText("Email Address")).not.toBeInTheDocument();
    expect(add).toHaveAttribute("aria-expanded", "false");
    expect(add).toHaveFocus();
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
