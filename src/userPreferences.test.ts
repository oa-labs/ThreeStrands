import { beforeEach, describe, expect, it } from "vitest";
import {
  applyExportablePreferences,
  readExportablePreferences,
  type ExportablePreferences,
} from "./userPreferences";

describe("exportable preferences", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("round-trips the complete allowlisted settings shape", () => {
    const preferences: ExportablePreferences = {
      theme: "dark",
      accent: "teal",
      fontScale: 120,
      fontFamily: "Georgia",
      autoReadDelaySeconds: 8,
      loadRemoteImages: true,
      selectedAccountId: "person@example.com",
      aiProvider: "openrouter",
      aiModel: "example/model",
      aiEndpoint: "https://api.example.test/v1",
      aiFeatures: {
        draftAssist: true,
        summarize: false,
        actionExtraction: false,
      },
      availabilityPreferences: {
        timeZone: "America/New_York",
        workingWindows: [
          { weekday: 1, start: "09:00", end: "17:00" },
          { weekday: 2, start: "09:00", end: "17:00" },
          { weekday: 3, start: "09:00", end: "17:00" },
          { weekday: 4, start: "09:00", end: "17:00" },
          { weekday: 5, start: "09:00", end: "17:00" },
        ],
        defaultDurationMinutes: 30,
        slotIncrementMinutes: 15,
      },
    };

    applyExportablePreferences(preferences);

    expect(readExportablePreferences()).toEqual(preferences);
  });

  it("does not collect secrets or transient storage", () => {
    localStorage.setItem("threestrands.settings.ai.apiKey", "do-not-export");
    localStorage.setItem("threestrands.crashReports", "private diagnostics");
    localStorage.setItem("threestrands.demoCorrespondence", "cached mail");

    const exported = JSON.stringify(readExportablePreferences());

    expect(exported).not.toContain("do-not-export");
    expect(exported).not.toContain("private diagnostics");
    expect(exported).not.toContain("cached mail");
  });
});
