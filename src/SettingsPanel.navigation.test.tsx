import { openUrl } from "@tauri-apps/plugin-opener";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsHarness } from "./test/settingsHarness";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

async function openSettings() {
  render(<SettingsHarness />);
  return screen.findByRole("dialog", { name: "Settings" });
}

describe("settings section navigation", () => {
  beforeEach(() => {
    localStorage.clear();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {
        transformCallback: vi.fn(() => 1),
        unregisterCallback: vi.fn(),
        // Font lookup is cached per module, so answer it with a real list.
        invoke: vi.fn(async (command: string) => (command === "list_system_font_families" ? [] : 1)),
      },
    });
    Object.defineProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
      configurable: true,
      value: { unregisterListener: vi.fn() },
    });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("opens contextual help for appearance, accounts, privacy, transfer, and delivery diagnostics", async () => {
    const dialog = await openSettings();
    for (const [section, slug] of [["Appearance", "appearance"], ["Mail Accounts", "mail-accounts"], ["Privacy", "privacy"], ["Data Transfer", "settings-and-sync"], ["Diagnostics", "troubleshooting"]]) {
      fireEvent.click(within(dialog).getByRole("button", { name: section }));
      fireEvent.click(within(dialog).getByRole("button", { name: `Help with ${section}` }));
      expect(openUrl).toHaveBeenLastCalledWith(`https://www.threestrands.app/docs/${slug}.html`);
    }
  });

  it("searches for IMAP setup in Mail Accounts", async () => {
    const dialog = await openSettings();
    fireEvent.change(within(dialog).getByRole("searchbox", { name: "Search Settings" }), { target: { value: "IMAP" } });
    expect(within(dialog).getByRole("button", { name: "Mail Accounts" })).toHaveAttribute("aria-current", "true");
    expect(within(dialog).getByRole("button", { name: "Add Account" })).toBeInTheDocument();
  });

  it("cycles between sections with the arrow keys, wrapping at each end", async () => {
    const dialog = await openSettings();
    expect(within(dialog).getByRole("button", { name: "Replicated Sync" })).toBeInTheDocument();
    const appearanceButton = within(dialog).getByRole("button", { name: "Appearance" });
    expect(appearanceButton).toHaveAttribute("aria-current", "true");

    fireEvent.keyDown(appearanceButton, { key: "ArrowDown" });
    const defaultAppsButton = within(dialog).getByRole("button", { name: "Default Apps" });
    expect(defaultAppsButton).toHaveAttribute("aria-current", "true");
    expect(defaultAppsButton).toHaveFocus();
    expect(within(dialog).getByRole("region", { name: "Default Apps" })).toBeInTheDocument();

    fireEvent.keyDown(defaultAppsButton, { key: "ArrowDown" });
    const accountsButton = within(dialog).getByRole("button", { name: "Mail Accounts" });
    expect(accountsButton).toHaveAttribute("aria-current", "true");
    expect(accountsButton).toHaveFocus();
    expect(within(dialog).getByRole("region", { name: "Mail Accounts" })).toBeInTheDocument();

    fireEvent.keyDown(accountsButton, { key: "ArrowUp" });
    fireEvent.keyDown(defaultAppsButton, { key: "ArrowUp" });
    expect(appearanceButton).toHaveAttribute("aria-current", "true");
    expect(appearanceButton).toHaveFocus();

    // Wraps past the first section to the last one.
    fireEvent.keyDown(appearanceButton, { key: "ArrowUp" });
    const lastButton = within(dialog).getByRole("button", { name: "Data Transfer" });
    expect(lastButton).toHaveAttribute("aria-current", "true");
    expect(lastButton).toHaveFocus();
  });

  it.each(["search", "section control"])("keeps arrow navigation working after clicking a section from %s", async (focusOrigin) => {
    const dialog = await openSettings();
    const previousControl = focusOrigin === "search"
      ? within(dialog).getByRole("searchbox", { name: "Search Settings" })
      : within(dialog).getByRole("spinbutton", { name: "Auto-Read Delay" });
    previousControl.focus();

    // A click alone does not focus buttons in macOS WebKit. Send subsequent
    // keys to the actual focused element, as the browser does.
    const defaultApps = within(dialog).getByRole("button", { name: "Default Apps" });
    fireEvent.click(defaultApps);
    expect(defaultApps).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    const accounts = within(dialog).getByRole("button", { name: "Mail Accounts" });
    expect(accounts).toHaveAttribute("aria-current", "true");
    expect(accounts).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    expect(defaultApps).toHaveAttribute("aria-current", "true");
    expect(defaultApps).toHaveFocus();
  });

  it("shows a decorative icon for every section without changing its accessible name", async () => {
    const dialog = await openSettings();
    const nav = within(dialog).getByRole("navigation", { name: "Settings sections" });
    const sectionButtons = Array.from(nav.querySelectorAll<HTMLButtonElement>("button[data-section-id]"));
    expect(sectionButtons).toHaveLength(15);
    for (const button of sectionButtons) {
      const icon = button.querySelector("svg");
      expect(icon).not.toBeNull();
      expect(icon).toHaveAttribute("aria-hidden", "true");
    }
    const headerIcon = dialog.querySelector(".settings-page-icon");
    expect(headerIcon).toHaveAttribute("aria-hidden", "true");
    expect(headerIcon?.querySelector("svg")).not.toBeNull();
    expect(within(dialog).getByRole("heading", { name: "Appearance" })).toBeInTheDocument();
  });

  it("groups and searches settings by control keywords", async () => {
    const dialog = await openSettings();
    expect(within(dialog).getByText("General")).toBeInTheDocument();
    expect(within(dialog).getByText("Accounts")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Appearance" })).toBeInTheDocument();
    expect(within(dialog).getByText("Changes save automatically")).toBeInTheDocument();
    // Mark-read timing lives on the Appearance page rather than its own one.
    expect(within(dialog).getByRole("spinbutton", { name: "Auto-Read Delay" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Reading" })).not.toBeInTheDocument();
    // Pages with explicit Save buttons must not claim everything autosaves.
    fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
    expect(within(dialog).queryByText("Changes save automatically")).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Appearance" }));

    const search = within(dialog).getByRole("searchbox", { name: "Search Settings" });
    expect(search).toHaveAttribute("writingsuggestions", "false");
    search.focus();
    fireEvent.change(search, { target: { value: "minimum email font size" } });
    expect(within(dialog).getByRole("button", { name: "Appearance" })).toHaveAttribute("aria-current", "true");
    expect(within(dialog).getByRole("combobox", { name: "Minimum Email Font Size" })).toBeInTheDocument();
    expect(within(dialog).getByRole("slider", { name: "Email Zoom" })).toBeInTheDocument();
    fireEvent.change(search, {
      target: { value: "remote images" },
    });

    expect(search).toHaveFocus();
    expect(within(dialog).getByRole("button", { name: "Privacy" })).toHaveAttribute("aria-current", "true");
    expect(within(dialog).getByRole("region", { name: "Privacy" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Appearance" })).not.toBeInTheDocument();

    // The writing profile is a Workflow section of its own, not part of AI Provider.
    fireEvent.change(search, { target: { value: "review instructions" } });
    expect(within(dialog).getByRole("button", { name: "Writing Profile" })).toHaveAttribute("aria-current", "true");
    expect(within(dialog).getByText("Workflow")).toBeInTheDocument();
    expect(within(dialog).getByRole("region", { name: "Writing Profile" })).toBeInTheDocument();
    const nav = within(dialog).getByRole("navigation", { name: "Settings sections" });
    expect(within(nav).queryByRole("button", { name: "AI Provider" })).not.toBeInTheDocument();
  });

  it("puts both account removal scopes behind one confirmation", async () => {
    const dialog = await openSettings();
    fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
    fireEvent.click(await within(dialog).findByRole("button", { name: "Disconnect…" }));

    const confirm = within(dialog).getByRole("group", { name: "Disconnect mail account confirmation" });
    expect(within(confirm).getByRole("button", { name: "Disconnect this device" })).toBeInTheDocument();
    expect(within(confirm).getByRole("button", { name: "Remove on all devices" })).toBeInTheDocument();

    // "Remove on all devices" opens a second "are you sure?" step rather than
    // acting immediately, since it affects every device.
    fireEvent.click(within(confirm).getByRole("button", { name: "Remove on all devices" }));
    const sure = within(dialog).getByRole("group", { name: "Remove mail account on all devices confirmation" });
    expect(within(sure).getByRole("button", { name: "Yes, remove everywhere" })).toBeInTheDocument();
    // Backing out of the second step returns to the first confirmation.
    fireEvent.click(within(sure).getByRole("button", { name: "Keep connected" }));
    const back = within(dialog).getByRole("group", { name: "Disconnect mail account confirmation" });

    fireEvent.click(within(back).getByRole("button", { name: "Cancel" }));
    expect(within(dialog).queryByRole("group", { name: "Disconnect mail account confirmation" })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Disconnect…" })).toBeInTheDocument();
  });

  it("clears section-local password drafts when navigating away and back", async () => {
    const dialog = await openSettings();
    fireEvent.click(within(dialog).getByRole("button", { name: "Data Transfer" }));
    for (const label of ["Export Password", "Confirm Password", "Backup File Password"]) {
      fireEvent.change(within(dialog).getByLabelText(label), { target: { value: "temporary password" } });
    }

    fireEvent.click(within(dialog).getByRole("button", { name: "Diagnostics" }));
    expect(within(dialog).queryByRole("region", { name: "Data transfer" })).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Data Transfer" }));
    for (const label of ["Export Password", "Confirm Password", "Backup File Password"]) {
      expect(within(dialog).getByLabelText(label)).toHaveValue("");
    }
  });
});
