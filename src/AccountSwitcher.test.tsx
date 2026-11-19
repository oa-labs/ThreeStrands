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
        onReselect={() => {}}
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
        onReselect={() => {}}
        onReorder={() => {}}
      />,
    );

    expect(screen.getByRole("radio", { name: account!.email })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("tooltip", { name: account!.email, hidden: true })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /^All accounts/i })).not.toBeInTheDocument();
  });

  it("keeps All accounts selectable for saved views with only one connected account", async () => {
    const [account] = await mailClient.listAccounts();
    const onShowAll = vi.fn();
    const props = { accounts: [account!], unreadCounts: {}, onSwitch: vi.fn(), onShowAll, onReselect: vi.fn(), onReorder: vi.fn(), showAllAccounts: true };
    const { rerender } = render(<AccountSwitcher {...props} activeAccountId={account!.email} />);
    const all = screen.getByRole("radio", { name: /^All accounts/i });
    expect(all).toHaveAttribute("aria-checked", "false");
    fireEvent.click(all);
    expect(onShowAll).toHaveBeenCalledOnce();
    rerender(<AccountSwitcher {...props} activeAccountId={null} />);
    expect(all).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: account!.email })).toHaveAttribute("aria-checked", "false");
  });

  it("uses nickname initials and email tooltips for accounts with the same sender name", async () => {
    const [account] = await mailClient.listAccounts();
    const onSwitch = vi.fn();
    const accounts = [
      { ...account!, email: "joel@openarc.net", displayName: "Joel Reed", nickname: "Work" },
      { ...account!, email: "joelreed@gmail.com", displayName: "Joel Reed", nickname: "Personal", status: "needs_reauth" as const },
    ];
    const { rerender } = render(<AccountSwitcher accounts={accounts} unreadCounts={{ "joel@openarc.net": 10 }}
      activeAccountId="joel@openarc.net" onSwitch={onSwitch} onShowAll={vi.fn()} onReselect={vi.fn()} onReorder={vi.fn()} />);
    const work = screen.getByRole("radio", { name: "Work, 10 unread" });
    expect(work).toHaveTextContent("W10");
    expect(work).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Personal, Needs reconnect" })).toHaveTextContent("P");
    expect(screen.getByRole("tooltip", { name: "Work (joel@openarc.net)", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("tooltip", { name: "Personal (joelreed@gmail.com) · Needs reconnect in Mail Accounts", hidden: true })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "Personal, Needs reconnect" }));
    expect(onSwitch).toHaveBeenCalledWith("joelreed@gmail.com");
    rerender(<AccountSwitcher accounts={[{ ...accounts[0]!, nickname: null }]} unreadCounts={{}}
      activeAccountId={null} onSwitch={onSwitch} onShowAll={vi.fn()} onReselect={vi.fn()} onReorder={vi.fn()} />);
    expect(screen.getByRole("radio", { name: "Joel Reed" })).toHaveTextContent("J");
  });

  describe("clicking the selected account", () => {
    const accounts = async () => {
      const [account] = await mailClient.listAccounts();
      return [
        { ...account!, email: "joel@openarc.net", displayName: "Joel", nickname: "Work" },
        { ...account!, email: "joelreed@gmail.com", displayName: "Joel", nickname: "Personal", sortOrder: 1 },
      ];
    };
    const handlers = () => ({ onSwitch: vi.fn(), onShowAll: vi.fn(), onReselect: vi.fn(), onReorder: vi.fn() });

    it("reselects instead of switching, and switches without reselecting for another account", async () => {
      const h = handlers();
      render(<AccountSwitcher accounts={await accounts()} unreadCounts={{}} activeAccountId="joel@openarc.net" {...h} />);
      fireEvent.click(screen.getByRole("radio", { name: "Work" }));
      expect(h.onReselect).toHaveBeenCalledOnce();
      expect(h.onSwitch).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("radio", { name: "Personal" }));
      expect(h.onSwitch).toHaveBeenCalledWith("joelreed@gmail.com");
      expect(h.onReselect).toHaveBeenCalledOnce();
    });

    it("treats All accounts the same way", async () => {
      const h = handlers();
      const { rerender } = render(<AccountSwitcher accounts={await accounts()} unreadCounts={{}} activeAccountId={null} {...h} />);
      fireEvent.click(screen.getByRole("radio", { name: /^All accounts/i }));
      expect(h.onReselect).toHaveBeenCalledOnce();
      expect(h.onShowAll).not.toHaveBeenCalled();
      rerender(<AccountSwitcher accounts={await accounts()} unreadCounts={{}} activeAccountId="joel@openarc.net" {...h} />);
      fireEvent.click(screen.getByRole("radio", { name: /^All accounts/i }));
      expect(h.onShowAll).toHaveBeenCalledOnce();
      expect(h.onReselect).toHaveBeenCalledOnce();
    });

    it("reselects the implicitly selected only account", async () => {
      const h = handlers();
      render(<AccountSwitcher accounts={[(await accounts())[0]!]} unreadCounts={{}} activeAccountId={null} {...h} />);
      fireEvent.click(screen.getByRole("radio", { name: "Work" }));
      expect(h.onReselect).toHaveBeenCalledOnce();
      expect(h.onSwitch).not.toHaveBeenCalled();
    });

    it("describes the click-again hint only on the selected account outside Mail", async () => {
      const h = handlers();
      const props = { accounts: await accounts(), unreadCounts: {}, activeAccountId: "joel@openarc.net", ...h };
      const { rerender } = render(<AccountSwitcher {...props} mailInView={false} />);
      expect(screen.getByRole("radio", { name: "Work" })).toHaveAccessibleDescription();
      expect(screen.getByRole("radio", { name: "Personal" })).not.toHaveAccessibleDescription();
      expect(screen.getByRole("radio", { name: /^All accounts/i })).not.toHaveAccessibleDescription();
      rerender(<AccountSwitcher {...props} mailInView />);
      expect(screen.getByRole("radio", { name: "Work" })).not.toHaveAccessibleDescription();
      rerender(<AccountSwitcher {...props} activeAccountId={null} mailInView={false} />);
      expect(screen.getByRole("radio", { name: /^All accounts/i })).toHaveAccessibleDescription();
    });

    it("pops the newly selected avatar only after the selection changes", async () => {
      const h = handlers();
      const props = { accounts: await accounts(), unreadCounts: {}, ...h };
      const { rerender } = render(<AccountSwitcher {...props} activeAccountId="joel@openarc.net" />);
      expect(screen.getByRole("radio", { name: "Work" })).not.toHaveClass("just-selected");
      rerender(<AccountSwitcher {...props} activeAccountId="joelreed@gmail.com" />);
      expect(screen.getByRole("radio", { name: "Personal" })).toHaveClass("just-selected");
      expect(screen.getByRole("radio", { name: "Work" })).not.toHaveClass("just-selected");
    });
  });
});
