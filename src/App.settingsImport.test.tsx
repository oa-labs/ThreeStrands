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
        },
      },
      accountCount: 2,
      splitInboxCount: 1,
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const dialog = screen.getByRole("dialog", { name: "Settings" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Data transfer" }));

    const dataTransfer = within(dialog).getByRole("region", { name: "Data transfer" });
    fireEvent.change(within(dataTransfer).getAllByLabelText("Export password")[1], {
      target: { value: "password123" },
    });
    fireEvent.click(within(dataTransfer).getByRole("button", {
      name: "Choose encrypted settings file",
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
