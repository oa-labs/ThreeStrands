import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./userPreferences", async (importOriginal) => {
  const original = await importOriginal<typeof import("./userPreferences")>();
  return {
    ...original,
    previewSettingsImport: vi.fn(),
    applySettingsImport: vi.fn(),
  };
});

import { App } from "./App";
import { applySettingsImport, previewSettingsImport } from "./userPreferences";
import { holdRecoveryPhrase, pendingRecoveryPhrase } from "./RecoveryPhraseDialog";

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
    // A file whose accounts are all already set up here applies straight away.
    vi.mocked(previewSettingsImport).mockResolvedValue({ accounts: [] });
    vi.mocked(applySettingsImport).mockResolvedValue({
      preferences: {
        theme: "dark",
        accent: "rose",
        fontScale: 110,
        fontFamily: "Georgia",
        emailMinimumFontSize: 18,
        emailZoom: 125,
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
          driveTimeBufferMinutes: 0,
        },
      },
      accountCount: 2,
      splitInboxCount: 1,
      contactCount: 0,
      contactGroupCount: 0,
      senderRuleCount: 0,
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    // Settings is lazy-loaded; the first import can outlast the default wait on slow CI runners.
    const dialog = await screen.findByRole("dialog", { name: "Settings" }, { timeout: 10_000 });
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
    expect(previewSettingsImport).toHaveBeenCalledWith("password123");
    expect(applySettingsImport).toHaveBeenCalledWith([]);
    expect(localStorage.getItem("threestrands.settings.emailZoom")).toBe("125");
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

  it("reopens the last section from the sidebar, keyboard shortcut, and command palette", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const settingsButton = screen.getByRole("button", { name: "Settings (⌘,)" });
    settingsButton.focus();
    fireEvent.click(settingsButton);
    let dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));

    fireEvent.click(settingsButton);
    dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(within(dialog).getByRole("button", { name: "Mail Accounts" })).toHaveFocus();
    expect(within(dialog).getByRole("region", { name: "Mail Accounts" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(settingsButton).toHaveFocus();

    fireEvent.keyDown(window, { key: ",", metaKey: true });
    dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(within(dialog).getByRole("button", { name: "Mail Accounts" })).toHaveAttribute("aria-current", "true");
    fireEvent.keyDown(window, { key: "Escape" });

    fireEvent.click(screen.getByRole("button", { name: "Command Palette (⌘K)" }));
    const palette = await screen.findByRole("dialog", { name: "Command Palette" });
    fireEvent.click(within(palette).getByRole("button", { name: /Open Settings/ }));
    dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(within(dialog).getByRole("button", { name: "Mail Accounts" })).toHaveAttribute("aria-current", "true");
  });

  it("restores the last section after restarting the app", async () => {
    const { unmount } = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    let dialog = await screen.findByRole("dialog", { name: "Settings" });
    const appearance = within(dialog).getByRole("button", { name: "Appearance" });
    fireEvent.keyDown(appearance, { key: "ArrowDown" });
    expect(within(dialog).getByRole("button", { name: "Default Apps" })).toHaveAttribute("aria-current", "true");
    unmount();

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(within(dialog).getByRole("button", { name: "Default Apps" })).toHaveAttribute("aria-current", "true");
    expect(within(dialog).getByRole("button", { name: "Default Apps" })).toHaveFocus();
  });

  it("keeps IMAP entry inside the Settings modal keyboard boundary", async () => {
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const settingsButton = screen.getByRole("button", { name: "Settings (⌘,)" });
    settingsButton.focus();
    fireEvent.click(settingsButton);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Add Account" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "IMAP" }));
    const email = within(dialog).getByLabelText("Email Address");
    expect(email).toHaveFocus();
    expect(dialog).toHaveAttribute("data-shortcut-scope", "modal");
    expect(container.inert).toBe(true);
    expect(container).toHaveAttribute("aria-hidden", "true");

    const close = within(dialog).getByRole("button", { name: "Close" });
    const lastControl = within(dialog).getByRole("button", { name: "Disconnect…" });
    lastControl.focus();
    fireEvent.keyDown(lastControl, { key: "Tab" });
    expect(close).toHaveFocus();
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true });
    expect(lastControl).toHaveFocus();
    email.focus();
    fireEvent.keyDown(email, { key: "e" });
    expect(within(dialog).getByLabelText("Email Address")).toHaveFocus();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Settings" })).not.toBeInTheDocument();
    expect(settingsButton).toHaveFocus();
    expect(container.inert).toBeFalsy();
  });

  it("keeps a pending recovery phrase reachable above Settings and protects it from Escape", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    act(() => holdRecoveryPhrase("one two three four"));

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const recovery = await screen.findByRole("dialog", { name: "Save Your Recovery Phrase" });
    try {
      expect(recovery.parentElement).not.toHaveAttribute("aria-hidden");
      expect(recovery.parentElement!.inert).toBeFalsy();
      expect(within(recovery).getByRole("button", { name: "Copy" })).toHaveFocus();
      fireEvent.keyDown(window, { key: "Escape" });
      expect(pendingRecoveryPhrase()).toBe("one two three four");
      expect(screen.getByRole("dialog", { name: "Settings", hidden: true })).toBeInTheDocument();
    } finally {
      fireEvent.click(within(recovery).getByRole("button", { name: "I’ve written it down" }));
    }

    expect(pendingRecoveryPhrase()).toBeNull();
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();
  });
});
