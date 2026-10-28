import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AccountSwitcher } from "./AccountSwitcher";
import { mailClient } from "./data/client";

describe("AccountSwitcher", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows unread inbox totals on each account and the combined account icon", async () => {
    const primary = {
      email: "demo@example.com",
      displayName: null,
      color: "#4285F4",
      status: "connected" as const,
      provider: "gmail" as const,
      labelModel: "gmailLabels" as const,
      sortOrder: 0,
      connectedAt: "2026-03-04T00:00:00Z",
      lastSyncedAt: null,
      onThisDevice: true,
    };
    render(
      <AccountSwitcher
        accounts={[
          primary,
          {
        ...primary,
        email: "work@example.com",
        displayName: "Work",
        color: "#34A853",
        status: "needs_reauth" as const,
        sortOrder: 1,
          },
        ]}
        unreadCounts={{ [primary.email]: 3, "work@example.com": 120 }}
        activeAccountId={null}
        onSwitch={() => {}}
        onShowAll={() => {}}
        onReorder={() => {}}
      />,
    );

    expect(screen.getByRole("radio", { name: "All accounts, 123 unread" })).toHaveTextContent("99+");
    expect(screen.getByRole("radio", { name: `${primary.email}, 3 unread` })).toHaveTextContent("3");
    expect(screen.getByRole("tooltip", { name: "work@example.com · Needs reconnect in Mail Accounts" })).toBeInTheDocument();
    const disconnectedAccount = screen.getByRole("radio", { name: "Work, 120 unread, Needs reconnect" });
    expect(disconnectedAccount).toHaveTextContent("99+");
    expect(disconnectedAccount.querySelector(".account-reconnect-badge")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: `${primary.email}, 3 unread` }).querySelector(".account-reconnect-badge")).not.toBeInTheDocument();
  });

  it("shows a lone account as checked, with its email tooltip and no All accounts option", async () => {
    const [account] = await mailClient.listAccounts();
    render(
      <AccountSwitcher
        accounts={[account!]}
        unreadCounts={{}}
        activeAccountId={null}
        onSwitch={() => {}}
        onShowAll={() => {}}
        onReorder={() => {}}
      />,
    );

    expect(screen.getByRole("radio", { name: account!.email })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("tooltip", { name: account!.email, hidden: true })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /^All accounts/i })).not.toBeInTheDocument();
  });

  it("uses nickname initials and email tooltips for accounts with the same sender name", async () => {
    const [account] = await mailClient.listAccounts();
    const onSwitch = vi.fn();
    const accounts = [
      { ...account!, email: "joel@openarc.net", displayName: "Joel Reed", nickname: "Work" },
      { ...account!, email: "joelreed@gmail.com", displayName: "Joel Reed", nickname: "Personal", status: "needs_reauth" as const },
    ];
    const { rerender } = render(<AccountSwitcher accounts={accounts} unreadCounts={{ "joel@openarc.net": 10 }}
      activeAccountId="joel@openarc.net" onSwitch={onSwitch} onShowAll={vi.fn()} onReorder={vi.fn()} />);
    const work = screen.getByRole("radio", { name: "Work, 10 unread" });
    expect(work).toHaveTextContent("W10");
    expect(work).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Personal, Needs reconnect" })).toHaveTextContent("P");
    expect(screen.getByRole("tooltip", { name: "Work (joel@openarc.net)", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("tooltip", { name: "Personal (joelreed@gmail.com) · Needs reconnect in Mail Accounts", hidden: true })).toBeInTheDocument();
    fireEvent.click(work);
    expect(onSwitch).toHaveBeenCalledWith("joel@openarc.net");
    rerender(<AccountSwitcher accounts={[{ ...accounts[0]!, nickname: null }]} unreadCounts={{}}
      activeAccountId={null} onSwitch={onSwitch} onShowAll={vi.fn()} onReorder={vi.fn()} />);
    expect(screen.getByRole("radio", { name: "Joel Reed" })).toHaveTextContent("J");
  });
});
