import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AccountScopeChip, AccountScopeProvider } from "./AccountScope";
import { mailClient } from "./data/client";

describe("AccountScopeChip", () => {
  afterEach(cleanup);

  const accounts = async () => {
    const [account] = await mailClient.listAccounts();
    return [
      { ...account!, email: "joel@openarc.net", displayName: "Joel", nickname: "Work", color: "#34A853" },
      { ...account!, email: "joelreed@gmail.com", displayName: "Joel Reed", nickname: null, color: "#4285F4", sortOrder: 1 },
    ];
  };

  it("names All accounts, with no account colour", async () => {
    render(<AccountScopeProvider accounts={await accounts()} activeAccountId={null}><AccountScopeChip accountId={null} /></AccountScopeProvider>);
    expect(screen.getByText("All accounts").closest(".account-scope-chip")).toHaveAttribute("data-all", "true");
  });

  it("shows the nickname, then the display name, in the account's colour", async () => {
    const list = await accounts();
    const { container, rerender } = render(<AccountScopeProvider accounts={list} activeAccountId="joel@openarc.net"><AccountScopeChip accountId="joel@openarc.net" /></AccountScopeProvider>);
    expect(screen.getByText("Work")).toBeInTheDocument();
    expect(container.querySelector<HTMLElement>(".account-scope-chip")!.style.getPropertyValue("--scope-color")).toBe("#34A853");
    rerender(<AccountScopeProvider accounts={list} activeAccountId="joelreed@gmail.com"><AccountScopeChip accountId="joelreed@gmail.com" /></AccountScopeProvider>);
    expect(screen.getByText("Joel Reed")).toBeInTheDocument();
  });

  it("falls back to the address without a provider", () => {
    render(<AccountScopeChip accountId="someone@example.com" />);
    expect(screen.getByText("someone@example.com")).toBeInTheDocument();
  });

  it("pulses when the account changes, but not on first mount or when mounted long after", async () => {
    const list = await accounts();
    const tree = (id: string | null) => <AccountScopeProvider accounts={list} activeAccountId={id}><AccountScopeChip accountId={id} /></AccountScopeProvider>;
    const { container, rerender } = render(tree("joel@openarc.net"));
    expect(container.querySelector(".account-scope-chip")).not.toHaveClass("pulse");
    await act(async () => { rerender(tree("joelreed@gmail.com")); });
    expect(container.querySelector(".account-scope-chip")).toHaveClass("pulse");
  });
});
