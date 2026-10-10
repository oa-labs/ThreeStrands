import type { SettingsSection } from "./settingsPanelTypes";

const SETTINGS_SECTION_KEY = "threestrands.settings.lastSection";
const sections: Record<SettingsSection, true> = {
  appearance: true,
  defaultApps: true,
  accounts: true,
  calendarAccounts: true,
  availability: true,
  splitInboxes: true,
  snippets: true,
  ai: true,
  replicatedSync: true,
  privacy: true,
  diagnostics: true,
  data: true,
};

/** Navigation stays on this device, outside the portable settings format. */
export function readSettingsSection(): SettingsSection {
  try {
    const saved = localStorage.getItem(SETTINGS_SECTION_KEY);
    if (saved && Object.hasOwn(sections, saved)) return saved as SettingsSection;
  } catch {
    // Settings must remain reachable if local storage is unavailable.
  }
  return "appearance";
}

export function saveSettingsSection(section: SettingsSection): void {
  try {
    localStorage.setItem(SETTINGS_SECTION_KEY, section);
  } catch {
    // In-memory navigation still works when storage is unavailable.
  }
}
