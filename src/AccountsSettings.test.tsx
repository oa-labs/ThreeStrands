import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AccountsSettings } from "./AccountsSettings";
import type { Account } from "./domain";
import type { ComponentProps } from "react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss from "postcss";

const account: Account = {
  email: "me@example.com",
  displayName: null,
  color: "#112233",
  status: "connected",
  provider: "gmail",
  labelModel: "gmailLabels",
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
    onSetDisplayName={vi.fn()} onSetNickname={vi.fn()}
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

  it("saves a trimmed nickname separately from the outgoing sender name", async () => {
    const onSetNickname = vi.fn().mockResolvedValue(undefined);
    const onSetDisplayName = vi.fn();
    renderAccounts(vi.fn(), { onSetNickname, onSetDisplayName, accounts: [{ ...account, displayName: "Joel Reed" }] });
    const input = screen.getByRole("textbox", { name: `Account nickname for ${account.email}` });
    expect(input).toHaveAccessibleDescription(/only visible in ThreeStrands/i);
    expect(input).toHaveAttribute("placeholder", "Work, Personal…");
    expect(screen.getByRole("textbox", { name: `Sender name for ${account.email}` }))
      .toHaveAccessibleDescription(/shown to recipients/i);
    expect(screen.queryByRole("button", { name: /^Save/ })).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: "  Work  " } });
    await act(async () => { fireEvent.submit(input.closest("form")!); });
    expect(onSetNickname).toHaveBeenCalledWith(account.email, "Work");
    expect(onSetDisplayName).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: `Sender name for ${account.email}` })).toHaveValue("Joel Reed");
  });

  it("autosaves a changed field on blur, confirms inline, and skips unchanged fields", async () => {
    const onSetDisplayName = vi.fn().mockResolvedValue(undefined);
    renderAccounts(vi.fn(), { onSetDisplayName, accounts: [{ ...account, displayName: "Joel Reed" }] });
    const input = screen.getByRole("textbox", { name: `Sender name for ${account.email}` });
    const status = within(input.closest("form")!).getByRole("status");

    fireEvent.blur(input);
    expect(onSetDisplayName).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: "Joel at Work" } });
    await act(async () => { fireEvent.blur(input); });
    expect(onSetDisplayName).toHaveBeenCalledWith(account.email, "Joel at Work");
    expect(status).toHaveTextContent("Saved");

    act(() => vi.advanceTimersByTime(2000));
    expect(status).toBeEmptyDOMElement();
  });

  it("shows the nickname and its initial while retaining the email and allows clearing it", async () => {
    const onSetNickname = vi.fn().mockResolvedValue(undefined);
    const { container } = renderAccounts(vi.fn(), {
      onSetNickname, accounts: [{ ...account, nickname: "Personal", displayName: "Joel Reed" }],
    });
    expect(screen.getByText("Personal", { selector: "strong" })).toBeVisible();
    expect(container.querySelector(".account-card-avatar")).toHaveTextContent("P");
    expect(container.querySelector(".account-card-email")).toHaveTextContent(account.email);
    const input = screen.getByRole("textbox", { name: `Account nickname for ${account.email}` });
    fireEvent.change(input, { target: { value: "   " } });
    await act(async () => { fireEvent.submit(input.closest("form")!); });
    expect(onSetNickname).toHaveBeenCalledWith(account.email, null);
  });

  it("keeps nickname edits available after a failed save and prevents duplicate saves", async () => {
    let reject!: (reason: Error) => void;
    const onSetNickname = vi.fn(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    renderAccounts(vi.fn(), { onSetNickname });
    const input = screen.getByRole("textbox", { name: `Account nickname for ${account.email}` });
    fireEvent.change(input, { target: { value: "Work" } });
    input.focus();
    fireEvent.submit(input.closest("form")!);
    // Read-only rather than disabled, so pressing Enter keeps focus in the field.
    expect(input).toHaveAttribute("readonly");
    expect(input).toHaveFocus();
    expect(within(input.closest("form")!).getByRole("status")).toHaveTextContent("Saving…");
    fireEvent.submit(input.closest("form")!);
    fireEvent.blur(input);
    expect(onSetNickname).toHaveBeenCalledTimes(1);
    await act(async () => reject(new Error("Could not save nickname")));
    expect(screen.getByRole("alert")).toHaveTextContent("Could not save nickname");
    expect(input).not.toHaveAttribute("readonly");
    expect(input).toHaveValue("Work");
    expect(within(input.closest("form")!).getByRole("status")).toBeEmptyDOMElement();
  });

  it("styles provider badges like Connected while keeping reconnect status distinct", () => {
    const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
    function badgeStyles(selectors: string[]) {
      const styles: Record<string, string> = {};
      css.walkRules((rule) => {
        if (rule.selectors.some((selector) => selectors.includes(selector))) {
          rule.walkDecls((declaration) => { styles[declaration.prop] = declaration.value; });
        }
      });
      // The provider badge sits at the top of the row; the status sits beside the name.
      delete styles["align-self"];
      return styles;
    }

    const connected = badgeStyles([".account-status", ".account-status.connected"]);
    expect(badgeStyles([".account-provider-badge"])).toEqual(connected);
    expect(connected).toMatchObject({ border: "0", "font-weight": "var(--weight-regular)" });
    expect(badgeStyles([".account-status", ".account-status.needs_reauth"])).toMatchObject({
      color: "var(--error)",
      background: "color-mix(in srgb, var(--error) 12%, transparent)",
    });
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
      // Healthy accounts rely on the sync time; only a problem earns a badge.
      expect(within(card).queryByText("Connected")).not.toBeInTheDocument();
      expect(within(card).queryByText("Needs reconnect") !== null).toBe(status === "needs_reauth");
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

  it("aligns both name fields in one grid and keeps Disconnect apart from housekeeping", () => {
    const { container } = renderAccounts(vi.fn(), {
      accounts: [account, { ...account, email: "work@example.com", sortOrder: 1 }],
    });
    const card = screen.getAllByRole("listitem")[0]!;
    const fields = card.querySelector(".account-card-fields")!;
    expect(fields.querySelectorAll(":scope > .account-name-field")).toHaveLength(2);
    for (const field of fields.querySelectorAll(".account-name-field")) {
      expect(Array.from(field.children, (child) => child.tagName)).toEqual(["SPAN", "INPUT", "SMALL"]);
    }
    const footer = card.querySelector(".account-card-footer")!;
    expect(within(footer as HTMLElement).getByRole("group", { name: `Order for ${account.email}` })).toBeVisible();
    expect(within(footer as HTMLElement).getByRole("button", { name: `Move ${account.email} down` })).toHaveAttribute("title", "Move down");
    expect(footer.lastElementChild).toHaveTextContent("Disconnect…");
    // The color is picked from the avatar itself, not a separate swatch.
    expect(card.querySelector(".account-card-avatar input[type=\"color\"]")).toBe(screen.getAllByLabelText(`Color for ${account.email}`)[0]);

    const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
    const decls: Record<string, Record<string, string>> = {};
    css.walkRules((rule) => {
      for (const selector of rule.selectors) {
        rule.walkDecls((declaration) => { (decls[selector] ??= {})[declaration.prop] = declaration.value; });
      }
    });
    expect(decls[".account-card-fields"]).toMatchObject({ display: "grid", "align-items": "start" });
    expect(decls[".account-card-footer .account-disconnect"]).toMatchObject({ "margin-left": "auto" });
    expect(container.querySelector(".account-color-swatch")).toBeNull();
  });

  it("hides reorder controls when there is only one account", () => {
    renderAccounts(vi.fn());
    expect(screen.queryByRole("button", { name: `Move ${account.email} up` })).not.toBeInTheDocument();
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
