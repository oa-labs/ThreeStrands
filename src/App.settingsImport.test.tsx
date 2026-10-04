import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./userPreferences", async (importOriginal) => {
  const original = await importOriginal<typeof import("./userPreferences")>();
  return {
    ...original,
    importSettings: vi.fn(),
  };
});

import { App } from "./App";
import { importSettings } from "./userPreferences";

describe("settings import navigation", () => {
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

  it("keeps Settings open and switches to Accounts after an import", async () => {
    vi.mocked(importSettings).mockResolvedValue({
      preferences: {
        theme: "dark",
        accent: "rose",
        fontScale: 110,
        fontFamily: "Georgia",
        emailMinimumFontSize: 18,
        autoReadDelaySeconds: 8,
        loadRemoteImages: true,
        selectedAccountId: null,
        aiProvider: "none",
        aiModel: "",
        aiFastModel: "",
        aiEndpoint: "",
        aiFeatures: {
          draftAssist: false,
          summarize: false,
          actionExtraction: false,
          contactEnrichment: false,
          proactiveBriefs: false,
          proactiveKnownSendersOnly: false,
          threadChat: false,
        },
        availabilityPreferences: {
          timeZone: "UTC",
          workingWindows: [1, 2, 3, 4, 5].map((weekday) => ({ weekday, start: "09:00", end: "17:00" })),
          defaultDurationMinutes: 30,
          slotIncrementMinutes: 15,
        },
      },
      accountCount: 2,
      splitInboxCount: 1,
      contactCount: 0,
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Data Transfer" }));

    const dataTransfer = within(dialog).getByRole("region", { name: "Data transfer" });
    fireEvent.change(within(dataTransfer).getByLabelText("Backup File Password"), {
      target: { value: "password123" },
    });
    fireEvent.click(within(dataTransfer).getByRole("button", {
      name: "Choose Encrypted Settings File",
    }));

    await waitFor(() => {
      expect(within(dialog).getByRole("region", { name: "Mail Accounts" })).toBeInTheDocument();
    });
    expect(within(dialog).getByRole("button", { name: "Mail Accounts" })).toHaveAttribute(
      "aria-current",
      "true",
    );
    expect(importSettings).toHaveBeenCalledWith("password123");
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    expect(document.documentElement).toHaveAttribute("data-accent", "rose");
  });
});

describe("settings section keyboard navigation", () => {
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

  it("cycles between sections with the arrow keys, wrapping at each end", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(within(dialog).getByRole("button", { name: "Replicated Sync" })).toBeInTheDocument();
    const appearanceButton = within(dialog).getByRole("button", { name: "Appearance" });
    expect(appearanceButton).toHaveAttribute("aria-current", "true");

    fireEvent.keyDown(appearanceButton, { key: "ArrowDown" });
    const accountsButton = within(dialog).getByRole("button", { name: "Mail Accounts" });
    expect(accountsButton).toHaveAttribute("aria-current", "true");
    expect(accountsButton).toHaveFocus();
    expect(within(dialog).getByRole("region", { name: "Mail Accounts" })).toBeInTheDocument();

    fireEvent.keyDown(accountsButton, { key: "ArrowUp" });
    expect(appearanceButton).toHaveAttribute("aria-current", "true");
    expect(appearanceButton).toHaveFocus();

    // Wraps past the first section to the last one.
    fireEvent.keyDown(appearanceButton, { key: "ArrowUp" });
    const lastButton = within(dialog).getByRole("button", { name: "Data Transfer" });
    expect(lastButton).toHaveAttribute("aria-current", "true");
    expect(lastButton).toHaveFocus();
  });

  it("shows a decorative icon for every section without changing its accessible name", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    const nav = within(dialog).getByRole("navigation", { name: "Settings sections" });
    const sectionButtons = Array.from(nav.querySelectorAll<HTMLButtonElement>("button[data-section-id]"));
    expect(sectionButtons).toHaveLength(11);
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
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
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
    search.focus();
    fireEvent.change(search, { target: { value: "minimum email font size" } });
    expect(within(dialog).getByRole("button", { name: "Appearance" })).toHaveAttribute("aria-current", "true");
    expect(within(dialog).getByRole("combobox", { name: "Minimum email font size" })).toBeInTheDocument();
    fireEvent.change(search, {
      target: { value: "remote images" },
    });

    expect(search).toHaveFocus();
    expect(within(dialog).getByRole("button", { name: "Privacy" })).toHaveAttribute("aria-current", "true");
    expect(within(dialog).getByRole("region", { name: "Privacy" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Appearance" })).not.toBeInTheDocument();
  });

  it("puts both account removal scopes behind one confirmation", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
    fireEvent.click(await within(dialog).findByRole("button", { name: "Disconnect…" }));

    const confirm = within(dialog).getByRole("group", { name: "Disconnect mail account confirmation" });
    expect(within(confirm).getByRole("button", { name: "Disconnect this device" })).toBeInTheDocument();
    expect(within(confirm).getByRole("button", { name: "Remove on all devices" })).toBeInTheDocument();
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(within(dialog).queryByRole("group", { name: "Disconnect mail account confirmation" })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Disconnect…" })).toBeInTheDocument();
  });
});
