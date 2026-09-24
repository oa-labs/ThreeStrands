import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { pullSyncedPreferences, queuePortablePreferences } from "./syncedPreferences";

describe("synced preferences data boundary", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  it("keeps appearance and selected navigation state off the replica", () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "private@example.com");
    localStorage.setItem("threestrands.theme", "dark");
    localStorage.setItem("threestrands.accent", "green");

    queuePortablePreferences();

    expect(invoke).toHaveBeenCalledWith("update_synced_preferences", {
      preferences: expect.not.objectContaining({
        selectedAccountId: expect.anything(),
        theme: expect.anything(),
        accent: expect.anything(),
      }),
    });
  });

  it("does not queue or pull preferences outside the native app", async () => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");

    queuePortablePreferences();

    expect(await pullSyncedPreferences()).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("queues only the typed portable preference allowlist", () => {
    localStorage.setItem("threestrands.crash-reports", "private diagnostics");
    localStorage.setItem("threestrands.settings.ai.apiKey", "secret");

    queuePortablePreferences();

    expect(invoke).toHaveBeenCalledWith("update_synced_preferences", {
      preferences: expect.not.objectContaining({
        selectedAccountId: expect.anything(),
        crashReports: expect.anything(),
        apiKey: expect.anything(),
      }),
    });
  });

  it("applies synced preferences without replacing local appearance or navigation", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "local@example.com");
    localStorage.setItem("threestrands.theme", "dark");
    localStorage.setItem("threestrands.accent", "teal");
    vi.mocked(invoke).mockResolvedValue({
      theme: "light", // Legacy replica field from an older app version.
      accent: "rose", // Legacy replica field from an older app version.
      selectedAccountId: "remote@example.com",
      fontScale: 110,
      fontFamily: "Georgia",
      autoReadDelaySeconds: 4,
      loadRemoteImages: false,
      aiProvider: "none",
      aiModel: "",
      aiEndpoint: "",
      aiFeatures: { draftAssist: false, summarize: false, actionExtraction: false },
      availabilityPreferences: {
        timeZone: "UTC",
        workingWindows: [],
        defaultDurationMinutes: 30,
        slotIncrementMinutes: 15,
      },
    });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("synced_preferences");
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("local@example.com");
    expect(localStorage.getItem("threestrands.theme")).toBe("dark");
    expect(localStorage.getItem("threestrands.accent")).toBe("teal");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.accent).toBe("teal");
    expect(localStorage.getItem("threestrands.fontScale")).toBe("110");
    expect(localStorage.getItem("threestrands.crash-reports")).toBeNull();
  });

  it("preserves the device theme when a new replica omits it", async () => {
    localStorage.setItem("threestrands.theme", "light");
    localStorage.setItem("threestrands.accent", "graphite");
    vi.mocked(invoke).mockResolvedValue({ fontScale: 120 });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(localStorage.getItem("threestrands.theme")).toBe("light");
    expect(localStorage.getItem("threestrands.accent")).toBe("graphite");
    expect(localStorage.getItem("threestrands.fontScale")).toBe("120");
  });
});
