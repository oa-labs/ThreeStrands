import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AccountsSettings } from "./AccountsSettings";
import * as replicatedSync from "./replicatedSync";
import { mailClient } from "./data/client";
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
  onThisDevice: true,
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
    const input = screen.getByRole("textbox", { name: `Account Nickname for ${account.email}` });
    expect(input).toHaveAccessibleDescription();
    expect(input).toHaveAttribute("placeholder", "Work, Personal…");
    expect(screen.getByRole("textbox", { name: `Sender Name for ${account.email}` }))
      .toHaveAccessibleDescription();
    expect(screen.queryByRole("button", { name: /^Save/ })).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: "  Work  " } });
    await act(async () => { fireEvent.submit(input.closest("form")!); });
    expect(onSetNickname).toHaveBeenCalledWith(account.email, "Work");
    expect(onSetDisplayName).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: `Sender Name for ${account.email}` })).toHaveValue("Joel Reed");
  });

  it("autosaves a changed field on blur, confirms inline, and skips unchanged fields", async () => {
    const onSetDisplayName = vi.fn().mockResolvedValue(undefined);
    renderAccounts(vi.fn(), { onSetDisplayName, accounts: [{ ...account, displayName: "Joel Reed" }] });
    const input = screen.getByRole("textbox", { name: `Sender Name for ${account.email}` });
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
    const input = screen.getByRole("textbox", { name: `Account Nickname for ${account.email}` });
    fireEvent.change(input, { target: { value: "   " } });
    await act(async () => { fireEvent.submit(input.closest("form")!); });
    expect(onSetNickname).toHaveBeenCalledWith(account.email, null);
  });

  it("keeps nickname edits available after a failed save and prevents duplicate saves", async () => {
    let reject!: (reason: Error) => void;
    const onSetNickname = vi.fn(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    renderAccounts(vi.fn(), { onSetNickname });
    const input = screen.getByRole("textbox", { name: `Account Nickname for ${account.email}` });
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

  it("keeps secondary actions in each account menu and opens the corresponding settings", () => {
    const work = { ...account, email: "work@example.com", sortOrder: 1 };
    const onSignatureDefaults = vi.fn();
    renderAccounts(vi.fn(), { accounts: [account, work], onSignatureDefaults });
    const workMenu = screen.getByRole("button", { name: `More actions for ${work.email}` });
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Signature Defaults|Aliases|Switch to Gmail/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Sending Addresses" })).not.toBeInTheDocument();
    fireEvent.keyDown(workMenu, { key: "ArrowDown" });
    const signatureDefaults = screen.getByRole("menuitem", { name: "Signature Defaults…" });
    expect(signatureDefaults).toHaveFocus();
    fireEvent.click(signatureDefaults);
    expect(onSignatureDefaults).toHaveBeenCalledWith(work.email);
    expect(workMenu).toHaveFocus();

    fireEvent.click(workMenu);
    fireEvent.click(screen.getByRole("menuitem", { name: "Aliases…" }));
    const aliases = screen.getByRole("dialog", { name: "Sending Addresses" });
    expect(within(aliases).getByText(work.email)).toBeVisible();
    fireEvent.keyDown(aliases, { key: "Escape" });
    expect(aliases).not.toBeInTheDocument();
    expect(workMenu).toHaveFocus();
  });

  it("offers Signature Defaults and Sending Addresses for IMAP without the Gmail actions", async () => {
    const sendingOptions = vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({
      identities: [{ address: account.email }],
      serverSavesSentOverride: false,
      sentCopyDetected: null,
    });
    const onSignatureDefaults = vi.fn();
    try {
      await act(async () => {
        renderAccounts(vi.fn(), { accounts: [{ ...account, provider: "imap" }], onSignatureDefaults });
      });
      expect(screen.queryByRole("dialog", { name: "Sending Addresses" })).not.toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "Sending addresses" })).not.toBeInTheDocument();

      const menu = screen.getByRole("button", { name: `More actions for ${account.email}` });
      fireEvent.click(menu);
      expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
        "Signature Defaults…",
        "Sending Addresses…",
      ]);
      fireEvent.click(screen.getByRole("menuitem", { name: "Signature Defaults…" }));
      expect(onSignatureDefaults).toHaveBeenCalledWith(account.email);
      expect(menu).toHaveFocus();

      fireEvent.click(menu);
      await act(async () => {
        fireEvent.click(screen.getByRole("menuitem", { name: "Sending Addresses…" }));
      });
      const dialog = screen.getByRole("dialog", { name: "Sending Addresses" });
      expect(within(dialog).getByText(account.email)).toBeVisible();
      fireEvent.keyDown(dialog, { key: "Escape" });
      expect(dialog).not.toBeInTheDocument();
      expect(menu).toHaveFocus();
    } finally {
      sendingOptions.mockRestore();
    }
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

describe("accounts on other devices", () => {
  const personal: Account = { ...account, email: "personal@example.com", nickname: "Personal", provider: "imap", labelModel: "imapLabelFolders", status: "needs_reauth", onThisDevice: false };
  const side: Account = { ...account, email: "side@example.com", status: "needs_reauth", onThisDevice: false };

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("lists accounts set up elsewhere apart from this device's accounts, without reconnect warnings", () => {
    renderAccounts(vi.fn(), { availableAccounts: [personal, side] });
    const section = screen.getByRole("region", { name: "On Your Other Devices" });
    expect(within(section).getByText("Personal")).toBeInTheDocument();
    expect(within(section).getByText("personal@example.com")).toBeInTheDocument();
    expect(within(section).queryByText("Needs reconnect")).not.toBeInTheDocument();
    expect(within(section).queryByRole("button", { name: /Disconnect/ })).not.toBeInTheDocument();
    expect(within(section).getByRole("button", { name: "Set up side@example.com on this device" })).toBeInTheDocument();
  });

  it("centers the provider badge with Set Up Here", () => {
    renderAccounts(vi.fn(), { availableAccounts: [side] });
    const section = screen.getByRole("region", { name: "On Your Other Devices" });
    const row = within(section).getByRole("button", { name: "Set up side@example.com on this device" }).parentElement!;
    expect(row).toHaveClass("account-card-row");
    expect(row.querySelector(".account-provider-badge")).toHaveTextContent("Gmail");

    const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
    const decls: Record<string, Record<string, string>> = {};
    css.walkRules((rule) => {
      for (const selector of rule.selectors) {
        rule.walkDecls((declaration) => { (decls[selector] ??= {})[declaration.prop] = declaration.value; });
      }
    });
    expect(decls[".account-card-row"]).toMatchObject({ "align-items": "center" });
    expect(decls[".available-account .account-provider-badge"]).toMatchObject({ "align-self": "center" });
  });

  it("omits the section when every account is set up here", () => {
    renderAccounts(vi.fn());
    expect(screen.queryByRole("region", { name: "On Your Other Devices" })).not.toBeInTheDocument();
  });

  it("sets up a Gmail account here through the normal sign-in", async () => {
    const onReconnect = vi.fn().mockResolvedValue(undefined);
    renderAccounts(vi.fn(), { availableAccounts: [side], onReconnect });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Set up side@example.com on this device" }));
    });
    expect(onReconnect).toHaveBeenCalledWith("side@example.com");
  });

  it("asks for this device's password when an IMAP account's server settings came with an import", async () => {
    vi.spyOn(mailClient, "getImapConnectionSettings").mockResolvedValue({
      imapHost: "127.0.0.1", imapPort: 1143, imapSecurity: "start_tls", imapUsername: "personal@example.com",
      smtpHost: "127.0.0.1", smtpPort: 1025, smtpSecurity: "start_tls", smtpUsername: "personal@example.com",
      imapCertificatePinned: true, smtpCertificatePinned: true,
    });
    renderAccounts(vi.fn(), { availableAccounts: [personal] });
    const setUp = screen.getByRole("button", { name: "Set up personal@example.com on this device" });
    fireEvent.click(setUp);
    expect(setUp).toHaveAttribute("aria-expanded", "true");
    expect(await screen.findByLabelText("Password")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Test and Save" })).toBeInTheDocument();
  });

  it("runs full IMAP setup for the same address when this device has no server settings for it", async () => {
    vi.spyOn(mailClient, "getImapConnectionSettings").mockRejectedValue(new Error("IMAP settings not found"));
    renderAccounts(vi.fn(), { availableAccounts: [personal] });
    fireEvent.click(screen.getByRole("button", { name: "Set up personal@example.com on this device" }));
    expect(await screen.findByRole("heading", { name: "Set Up on This Device" })).toBeInTheDocument();
    const email = screen.getByLabelText("Email Address");
    expect(email).toHaveValue("personal@example.com");
    expect(email).toHaveAttribute("readonly");
  });
});

describe("IMAP editing device", () => {
  afterEach(() => vi.restoreAllMocks());

  it("hands off the account from Mail Accounts to an active enrolled peer", async () => {
    vi.spyOn(replicatedSync, "replicatedSyncEnabled").mockResolvedValue(true);
    vi.spyOn(replicatedSync, "imapLabelEditor").mockResolvedValue({
      hasPeers: true, designated: "current", isEditor: true, ready: true, canEdit: true,
      eligibleDevices: ["current", "other"],
    });
    vi.spyOn(replicatedSync, "replicatedSyncDeviceRoster").mockResolvedValue([
      { deviceId: "current", label: "Desktop", status: "active", isSelf: true },
      { deviceId: "other", label: "Laptop", status: "active", isSelf: false },
      { deviceId: "revoked", label: "Retired", status: "revoked", isSelf: false },
      { deviceId: "phone", label: "Phone", status: "active", isSelf: false },
    ]);
    const handoff = vi.spyOn(replicatedSync, "handoffImapLabelEditor").mockResolvedValue(undefined);
    await act(async () => { renderAccounts(vi.fn()); });

    fireEvent.click(screen.getByRole("button", { name: `More actions for ${account.email}` }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "IMAP Editing Device…" }));
    const dialog = screen.getByRole("dialog", { name: "IMAP Editing Device" });
    const select = await within(dialog).findByRole("combobox", { name: "New editing device" });
    expect(within(select).queryByRole("option", { name: "Retired" })).not.toBeInTheDocument();
    expect(within(select).queryByRole("option", { name: "Phone" })).not.toBeInTheDocument();
    const save = within(dialog).getByRole("button", { name: "Change Editing Device" });
    expect(save).toBeDisabled();
    fireEvent.change(select, { target: { value: "other" } });
    await act(async () => { fireEvent.click(save); });
    expect(handoff).toHaveBeenCalledWith(account.email, "other");
    expect(dialog).not.toBeInTheDocument();
  });
});

describe("switching Gmail to IMAP", () => {
  afterEach(() => vi.restoreAllMocks());

  it("tests the app password, reviews label mappings, and refreshes the switched account", async () => {
    const preview = vi.spyOn(mailClient, "previewGmailImapSwitch").mockResolvedValue({
      taskCount: 2,
      goalCount: 1,
      unverifiedTaskLinks: ["Call the nursery"],
      splitInboxes: [
        { id: "orchard", name: "Orchard", oldLabelId: "Label_1", oldLabelName: "Projects/Orchard", suggestedLabelId: "folder:Projects/Orchard" },
        { id: "vendors", name: "Vendors", oldLabelId: "Label_2", oldLabelName: null, suggestedLabelId: null },
      ],
      labels: [
        { id: "folder:Projects/Orchard", name: "Projects/Orchard", kind: "user" },
        { id: "folder:Vendors", name: "Vendors", kind: "user" },
      ],
    });
    const commit = vi.spyOn(mailClient, "switchGmailToImap").mockResolvedValue({ ...account, provider: "imap" });
    const onGmailImapSwitched = vi.fn().mockResolvedValue(undefined);
    const onRemove = vi.fn();
    renderAccounts(vi.fn(), { onGmailImapSwitched, onRemove });
    fireEvent.click(screen.getByRole("button", { name: `More actions for ${account.email}` }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Switch to Gmail via IMAP…" }));
    const password = screen.getByLabelText("App Password");
    expect(password).toHaveFocus();
    fireEvent.change(password, { target: { value: "abcd efgh ijkl mnop" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Test Connection" })); });
    expect(preview).toHaveBeenCalledWith(account.email, "abcdefghijklmnop");
    expect(commit).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Review Gmail switch" })).toHaveFocus();
    expect(screen.getByText("Call the nursery")).toBeVisible();
    expect(screen.getByText(/Your other devices stay connected and can switch separately/)).toBeVisible();
    expect(screen.getByText(/Shared snoozes and other devices’ scheduled sends are kept/)).toBeVisible();
    expect(screen.getByRole("combobox", { name: "Orchard · Projects/Orchard" })).toHaveValue("folder:Projects/Orchard");
    fireEvent.change(screen.getByRole("combobox", { name: "Vendors" }), { target: { value: "folder:Vendors" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Switch to Gmail via IMAP" })); });
    expect(commit).toHaveBeenCalledWith(account.email, "abcdefghijklmnop", [
      { id: "orchard", oldLabelId: "Label_1", labelId: "folder:Projects/Orchard" },
      { id: "vendors", oldLabelId: "Label_2", labelId: "folder:Vendors" },
    ]);
    expect(onGmailImapSwitched).toHaveBeenCalledOnce();
    expect(onRemove).not.toHaveBeenCalled();
  });

  it("leaves the Gmail connection in place when the connection test fails or is canceled", async () => {
    let rejectPreview!: (error: Error) => void;
    vi.spyOn(mailClient, "previewGmailImapSwitch").mockImplementation(() => new Promise((_, reject) => {
      rejectPreview = reject;
    }));
    const commit = vi.spyOn(mailClient, "switchGmailToImap");
    renderAccounts(vi.fn());
    const menu = screen.getByRole("button", { name: `More actions for ${account.email}` });
    fireEvent.click(menu);
    fireEvent.click(screen.getByRole("menuitem", { name: "Switch to Gmail via IMAP…" }));
    const dialog = screen.getByRole("dialog", { name: "Switch to Gmail via IMAP" });
    fireEvent.change(within(dialog).getByLabelText("App Password"), { target: { value: "abcd efgh ijkl mnop" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Test Connection" }));
    const cancel = within(dialog).getByRole("button", { name: "Cancel" });
    expect(cancel).toBeDisabled();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(dialog).toBeVisible();
    await act(async () => rejectPreview(new Error("The app password was rejected.")));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("The app password was rejected.");
    expect(within(dialog).queryByRole("heading", { name: "Review Gmail switch" })).not.toBeInTheDocument();
    fireEvent.click(cancel);
    expect(menu).toHaveFocus();
    fireEvent.click(menu);
    expect(screen.getByRole("menuitem", { name: "Switch to Gmail via IMAP…" })).toBeEnabled();
    expect(commit).not.toHaveBeenCalled();
  });
});
