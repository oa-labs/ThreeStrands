import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  applyExportablePreferences,
  exportSettings,
  importSettings,
  readExportablePreferences,
  type ExportablePreferences,
  type SettingsImportResult,
} from "./userPreferences";
// Shared with the native `transfer::tests::webview_preferences_fixture_matches_the_native_transfer_preferences`
// test, which deserializes it into `TransferPreferences` (deny_unknown_fields).
import webviewPreferences from "../src-tauri/tests/fixtures/settings-transfer/webview-preferences.json";

const nativeContractFixture = webviewPreferences as ExportablePreferences;

/** Dotted key paths of an object tree; arrays contribute their first element's keys. */
function keyPaths(value: unknown, prefix = ""): string[] {
  if (Array.isArray(value)) return value.length ? keyPaths(value[0], `${prefix}[]`) : [];
  if (value === null || typeof value !== "object") return [];
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return [path, ...keyPaths(child, path)];
  });
}

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
      emailMinimumFontSize: 18,
      autoReadDelaySeconds: 8,
      loadRemoteImages: true,
      selectedAccountId: "person@example.com",
      aiProvider: "openrouter",
      aiModel: "example/model",
      aiFastModel: "example/fast-model",
      aiEndpoint: "https://api.example.test/v1",
      aiFeatures: {
        draftAssist: true,
        summarize: false,
        actionExtraction: false,
        contactEnrichment: false,
        proactiveBriefs: false,
        proactiveKnownSendersOnly: false,
        threadChat: false,
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

  it("imports the preceding preference schema with the email font floor off", () => {
    const { emailMinimumFontSize: _floor, ...previous } = nativeContractFixture;
    applyExportablePreferences({ ...nativeContractFixture, emailMinimumFontSize: 24 });
    applyExportablePreferences(previous as ExportablePreferences);
    expect(readExportablePreferences().emailMinimumFontSize).toBe(0);
    expect(readExportablePreferences().fontFamily).toBe(previous.fontFamily);
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

  it("emits exactly the key set the native transfer contract accepts", () => {
    // Defaults, as on a fresh install.
    expect(keyPaths(readExportablePreferences()).sort()).toEqual(keyPaths(nativeContractFixture).sort());

    applyExportablePreferences(nativeContractFixture);
    const exported = readExportablePreferences();
    expect(keyPaths(exported).sort()).toEqual(keyPaths(nativeContractFixture).sort());
    expect(exported).toEqual(nativeContractFixture);
  });
});

describe("settings transfer commands", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(invoke).mockReset();
  });

  it("exports the allowlisted preferences with the password and returns the saved path", async () => {
    applyExportablePreferences(nativeContractFixture);
    vi.mocked(invoke).mockResolvedValueOnce("/tmp/threestrands-settings.dispatch-settings");

    await expect(exportSettings("correct horse battery staple")).resolves.toBe(
      "/tmp/threestrands-settings.dispatch-settings",
    );

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("export_settings", {
      preferences: nativeContractFixture,
      password: "correct horse battery staple",
    });
  });

  it("returns null when the export dialog is cancelled", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(null);
    await expect(exportSettings("correct horse battery staple")).resolves.toBeNull();
  });

  it("applies imported preferences locally and returns the native summary", async () => {
    const result: SettingsImportResult = {
      preferences: nativeContractFixture,
      accountCount: 2,
      splitInboxCount: 1,
      contactCount: 3,
    };
    vi.mocked(invoke).mockResolvedValueOnce(result);

    await expect(importSettings("correct horse battery staple")).resolves.toEqual(result);

    expect(invoke).toHaveBeenCalledWith("import_settings", { password: "correct horse battery staple" });
    expect(readExportablePreferences()).toEqual(nativeContractFixture);
  });

  it("leaves local preferences untouched when the import is cancelled", async () => {
    const before = readExportablePreferences();
    vi.mocked(invoke).mockResolvedValueOnce(null);

    await expect(importSettings("correct horse battery staple")).resolves.toBeNull();

    expect(readExportablePreferences()).toEqual(before);
  });

  it("propagates a native import failure without applying anything", async () => {
    const before = readExportablePreferences();
    vi.mocked(invoke).mockRejectedValueOnce("The password is incorrect or the settings export is damaged");

    await expect(importSettings("wrong password!")).rejects.toBe(
      "The password is incorrect or the settings export is damaged",
    );

    expect(readExportablePreferences()).toEqual(before);
  });
});
