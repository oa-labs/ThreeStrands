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

  it("never includes selected navigation state in queued preferences", () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "private@example.com");

    queuePortablePreferences();

    expect(invoke).toHaveBeenCalledWith("update_synced_preferences", {
      preferences: expect.not.objectContaining({ selectedAccountId: expect.anything() }),
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

  it("applies synced preferences without replacing device navigation", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "local@example.com");
    vi.mocked(invoke).mockResolvedValue({
      theme: "dark",
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
    expect(localStorage.getItem("threestrands.crash-reports")).toBeNull();
  });
});
