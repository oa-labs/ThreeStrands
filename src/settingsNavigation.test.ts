import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readSettingsSection, saveSettingsSection } from "./settingsNavigation";
import type { SettingsSection } from "./settingsPanelTypes";

describe("remembered settings section", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it.each<SettingsSection>([
    "appearance", "defaultApps", "accounts", "calendarAccounts", "availability",
    "splitInboxes", "snippets", "ai", "replicatedSync", "privacy", "diagnostics", "data",
  ])("restores %s from device storage", (section) => {
    saveSettingsSection(section);
    expect(readSettingsSection()).toBe(section);
  });

  it.each([null, "", "retired-section", "__proto__", "constructor"])("defaults to Appearance for %s", (saved) => {
    if (saved !== null) localStorage.setItem("threestrands.settings.lastSection", saved);
    expect(readSettingsSection()).toBe("appearance");
  });

  it("keeps Settings available when storage cannot be read or written", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("Blocked"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Blocked"); });
    expect(readSettingsSection()).toBe("appearance");
    expect(() => saveSettingsSection("accounts")).not.toThrow();
  });
});
