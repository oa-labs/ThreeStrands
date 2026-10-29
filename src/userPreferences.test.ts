import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  applyExportablePreferences,
  exportSettings,
  applySettingsImport,
  cancelSettingsImport,
  previewSettingsImport,
  readExportablePreferences,
  type ExportablePreferences,
  type SettingsImportResult,
} from "./userPreferences";
import { CALENDAR_COLORS_KEY, resetCalendarColorsForTests } from "./calendarColors";
import { DEFAULT_AVAILABILITY_PROFILES } from "./settings";
import { WRITING_PROFILE_LIMITS, WRITING_PROFILES_KEY, resetWritingProfilesForTests } from "./writingProfiles";
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
    resetCalendarColorsForTests();
    resetWritingProfilesForTests();
  });

  it("round-trips the complete allowlisted settings shape", () => {
    const preferences: ExportablePreferences = {
      theme: "dark",
      accent: "teal",
      fontScale: 120,
      fontFamily: "Georgia",
      emailMinimumFontSize: 18,
      emailZoom: 125,
      emailTextColors: "app",
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
        driveTimeBufferMinutes: 30,
      },
      availabilityProfiles: {
        defaultProfile: { id: "default", name: "Work" },
        otherProfiles: [{ id: "church", name: "Church", workingWindows: [{ weekday: 2, start: "18:30", end: "21:00" }], defaultDurationMinutes: 60, driveTimeBufferMinutes: 15 }],
        accountDefaults: { "person@example.com": "church" },
      },
      calendarColors: { "person@example.com": { primary: "teal", "team@example.com": "gray" } },
      writingProfiles: { "person@example.com": { aboutYou: "CEO of a consulting firm.\nI sell to CFOs.", reviewInstructions: "" } },
    };

    applyExportablePreferences(preferences);

    expect(readExportablePreferences()).toEqual(preferences);
  });

  it("imports older availability preferences with drive time disabled", () => {
    const { driveTimeBufferMinutes: _buffer, ...previous } = nativeContractFixture.availabilityPreferences;
    applyExportablePreferences(nativeContractFixture);
    applyExportablePreferences({ ...nativeContractFixture, availabilityPreferences: previous } as ExportablePreferences);
    expect(readExportablePreferences().availabilityPreferences).toEqual({ ...previous, driveTimeBufferMinutes: 0 });
  });

  it("imports the preceding preference schema with only the Default profile and keeps its hours", () => {
    const { availabilityProfiles: _profiles, ...previous } = nativeContractFixture;
    applyExportablePreferences(nativeContractFixture);
    applyExportablePreferences(previous as ExportablePreferences);
    expect(readExportablePreferences().availabilityProfiles).toEqual(DEFAULT_AVAILABILITY_PROFILES);
    expect(localStorage.getItem("threestrands.settings.availabilityProfiles")).toBeNull();
    expect(readExportablePreferences().availabilityPreferences).toEqual(previous.availabilityPreferences);
  });

  it("imports the preceding preference schema at 100% email zoom", () => {
    const { emailZoom: _zoom, ...previous } = nativeContractFixture;
    applyExportablePreferences({ ...nativeContractFixture, emailZoom: 150 });
    applyExportablePreferences(previous);
    expect(readExportablePreferences().emailZoom).toBe(100);
    expect(readExportablePreferences().emailMinimumFontSize).toBe(previous.emailMinimumFontSize);
  });

  it("imports the preceding preference schema with improved email text contrast", () => {
    const { emailTextColors: _colors, ...previous } = nativeContractFixture;
    applyExportablePreferences({ ...nativeContractFixture, emailTextColors: "sender" });
    applyExportablePreferences(previous);
    expect(readExportablePreferences().emailTextColors).toBe("contrast");
    expect(readExportablePreferences().emailZoom).toBe(previous.emailZoom ?? 100);
  });

  it("imports the preceding preference schema with the email font floor off", () => {
    const { emailMinimumFontSize: _floor, ...previous } = nativeContractFixture;
    applyExportablePreferences({ ...nativeContractFixture, emailMinimumFontSize: 24 });
    applyExportablePreferences(previous as ExportablePreferences);
    expect(readExportablePreferences().emailMinimumFontSize).toBe(0);
    expect(readExportablePreferences().fontFamily).toBe(previous.fontFamily);
  });

  it("imports the preceding preference schema with every calendar on the default color", () => {
    const { calendarColors: _colors, ...previous } = nativeContractFixture;
    applyExportablePreferences(nativeContractFixture);
    applyExportablePreferences(previous as ExportablePreferences);
    expect(readExportablePreferences().calendarColors).toEqual({});
    expect(localStorage.getItem(CALENDAR_COLORS_KEY)).toBeNull();
  });

  it("imports only calendar colors from the palette", () => {
    applyExportablePreferences({
      ...nativeContractFixture,
      calendarColors: {
        "person@example.com": { primary: "teal", injected: "red; background: url(x)" },
        "list@example.com": ["teal"],
      } as unknown as ExportablePreferences["calendarColors"],
    });
    expect(readExportablePreferences().calendarColors).toEqual({ "person@example.com": { primary: "teal" } });
  });

  it("imports the preceding preference schema with no writing profiles", () => {
    const { writingProfiles: _profiles, ...previous } = nativeContractFixture;
    applyExportablePreferences(nativeContractFixture);
    applyExportablePreferences(previous as ExportablePreferences);
    expect(readExportablePreferences().writingProfiles).toEqual({});
    expect(localStorage.getItem(WRITING_PROFILES_KEY)).toBeNull();
    expect(readExportablePreferences().calendarColors).toEqual(previous.calendarColors);
  });

  it("imports only text writing profiles within the limits under lowercased accounts", () => {
    const limit = WRITING_PROFILE_LIMITS.aboutYou;
    applyExportablePreferences({
      ...nativeContractFixture,
      writingProfiles: {
        "Person@Example.com": { aboutYou: "é".repeat(limit), reviewInstructions: "Be direct." },
        "long@example.com": { aboutYou: "é".repeat(limit + 1), reviewInstructions: "" },
        "blank@example.com": { aboutYou: " ", reviewInstructions: "\n" },
        "partial@example.com": { reviewInstructions: 7, aboutYou: "Founder" },
        "list@example.com": ["Founder"],
      } as unknown as ExportablePreferences["writingProfiles"],
    });
    expect(readExportablePreferences().writingProfiles).toEqual({
      "person@example.com": { aboutYou: "é".repeat(limit), reviewInstructions: "Be direct." },
      "partial@example.com": { aboutYou: "Founder", reviewInstructions: "" },
    });
  });

  it("keeps a custom provider and its endpoint through import and re-export", () => {
    // Settings sync and export read the provider back through readAiProvider,
    // so a custom selection must stay `custom` there; mapping it to anything
    // else would overwrite working endpoints on every synced device.
    applyExportablePreferences({
      ...nativeContractFixture,
      aiProvider: "custom",
      aiModel: "local-llama",
      aiEndpoint: "https://api.example.test/v1",
    });
    const read = readExportablePreferences();
    expect(read.aiProvider).toBe("custom");
    expect(read.aiEndpoint).toBe("https://api.example.test/v1");
    expect(read.aiModel).toBe("local-llama");
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
    // Defaults, as on a fresh install. Calendar color, writing profile, and
    // account default keys are account and calendar ids, so only the field
    // itself belongs to the contract. A fresh install has no extra profiles.
    const schemaPaths = (value: unknown) => keyPaths(value).filter((path) => !path.startsWith("calendarColors.")
      && !path.startsWith("writingProfiles.")
      && !path.startsWith("availabilityProfiles.accountDefaults.")
      && !path.startsWith("availabilityProfiles.otherProfiles[]")).sort();
    expect(schemaPaths(readExportablePreferences())).toEqual(schemaPaths(nativeContractFixture));

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

  it("previews a settings file without applying its preferences", async () => {
    const before = readExportablePreferences();
    const preview = {
      accounts: [{ email: "personal@example.com", displayName: null, nickname: "Personal", provider: "imap", setUpHere: false }],
    };
    vi.mocked(invoke).mockResolvedValueOnce(preview);

    await expect(previewSettingsImport("correct horse battery staple")).resolves.toEqual(preview);

    expect(invoke).toHaveBeenCalledWith("preview_settings_import", { password: "correct horse battery staple" });
    expect(readExportablePreferences()).toEqual(before);
  });

  it("applies imported preferences locally with the accounts left off this device", async () => {
    const result: SettingsImportResult = {
      preferences: nativeContractFixture,
      accountCount: 2,
      splitInboxCount: 1,
      contactCount: 3,
      contactGroupCount: 1,
      senderRuleCount: 0,
    };
    vi.mocked(invoke).mockResolvedValueOnce(result);

    await expect(applySettingsImport(["personal@example.com"])).resolves.toEqual(result);

    expect(invoke).toHaveBeenCalledWith("apply_settings_import", { notHere: ["personal@example.com"] });
    expect(readExportablePreferences()).toEqual(nativeContractFixture);
  });

  it("leaves local preferences untouched when the file dialog is cancelled", async () => {
    const before = readExportablePreferences();
    vi.mocked(invoke).mockResolvedValueOnce(null);

    await expect(previewSettingsImport("correct horse battery staple")).resolves.toBeNull();

    expect(readExportablePreferences()).toEqual(before);
  });

  it("discards a previewed file on cancel", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    await cancelSettingsImport();
    expect(invoke).toHaveBeenCalledWith("cancel_settings_import");
  });

  it("propagates a native import failure without applying anything", async () => {
    const before = readExportablePreferences();
    vi.mocked(invoke).mockRejectedValueOnce("The password is incorrect or the settings export is damaged");

    await expect(previewSettingsImport("wrong password!")).rejects.toBe(
      "The password is incorrect or the settings export is damaged",
    );
    vi.mocked(invoke).mockRejectedValueOnce("Choose a settings file to import first");
    await expect(applySettingsImport([])).rejects.toBe("Choose a settings file to import first");

    expect(readExportablePreferences()).toEqual(before);
  });
});
