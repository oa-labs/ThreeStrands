import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  confirmCloudEnrollment,
  pullCloudPreferences,
  queuePortablePreferences,
} from "./cloudAccount";

describe("cloud account data boundary", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  it("never includes selected navigation state in an enrollment payload", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "private@example.com");
    vi.mocked(invoke).mockResolvedValue(undefined);

    await confirmCloudEnrollment();

    expect(invoke).toHaveBeenCalledWith("cloud_confirm_enrollment", {
      preferences: expect.not.objectContaining({ selectedAccountId: expect.anything() }),
    });
  });

  it("queues only the typed portable preference allowlist", () => {
    localStorage.setItem("threestrands.crash-reports", "private diagnostics");
    localStorage.setItem("threestrands.settings.ai.apiKey", "secret");

    queuePortablePreferences();

    expect(invoke).toHaveBeenCalledWith("cloud_update_preferences", {
      preferences: expect.not.objectContaining({
        selectedAccountId: expect.anything(),
        crashReports: expect.anything(),
        apiKey: expect.anything(),
      }),
    });
  });

  it("applies cloud preferences without replacing device navigation", async () => {
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

    expect(await pullCloudPreferences()).toBe(true);
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("local@example.com");
    expect(localStorage.getItem("threestrands.theme")).toBe("dark");
    expect(localStorage.getItem("threestrands.crash-reports")).toBeNull();
  });
});
