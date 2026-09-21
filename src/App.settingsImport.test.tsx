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
        invoke: vi.fn().mockResolvedValue(1),
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
        fontScale: 110,
        fontFamily: "Georgia",
        autoReadDelaySeconds: 8,
        loadRemoteImages: true,
        selectedAccountId: null,
        aiProvider: "none",
        aiModel: "",
        aiEndpoint: "",
        aiFeatures: {
          draftAssist: false,
          summarize: false,
          actionExtraction: false,
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
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const dialog = screen.getByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Data Transfer" }));

    const dataTransfer = within(dialog).getByRole("region", { name: "Data transfer" });
    fireEvent.change(within(dataTransfer).getAllByLabelText("Export Password")[1], {
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
        invoke: vi.fn().mockResolvedValue(1),
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
    const dialog = screen.getByRole("dialog", { name: "Settings" });
    const appearanceButton = within(dialog).getByRole("button", { name: "Appearance" });
    expect(appearanceButton).toHaveAttribute("aria-current", "true");

    fireEvent.keyDown(appearanceButton, { key: "ArrowDown" });
    const readingButton = within(dialog).getByRole("button", { name: "Reading" });
    expect(readingButton).toHaveAttribute("aria-current", "true");
    expect(readingButton).toHaveFocus();
    expect(within(dialog).getByRole("region", { name: "Reading" })).toBeInTheDocument();

    fireEvent.keyDown(readingButton, { key: "ArrowUp" });
    expect(appearanceButton).toHaveAttribute("aria-current", "true");
    expect(appearanceButton).toHaveFocus();

    // Wraps past the first section to the last one.
    fireEvent.keyDown(appearanceButton, { key: "ArrowUp" });
    const lastButton = within(dialog).getByRole("button", { name: "Data Transfer" });
    expect(lastButton).toHaveAttribute("aria-current", "true");
    expect(lastButton).toHaveFocus();
  });
});
