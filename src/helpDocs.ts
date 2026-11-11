import { openUrl } from "@tauri-apps/plugin-opener";
import { docsUrl } from "./gettingStartedSteps";
import type { SettingsSection } from "./settingsPanelTypes";

/** Context links are shared with docs validation; credentials never enter URLs. */
export const SETTINGS_HELP: Record<SettingsSection, { slug: string; section?: string }> = {
  accounts: { slug: "mail-accounts" },
  calendarAccounts: { slug: "calendar", section: "connect-google-calendar" },
  availability: { slug: "calendar", section: "schedule" },
  availabilityAccounts: { slug: "calendar", section: "availability-profiles" },
  meetingTypes: { slug: "calendar", section: "availability-profiles" },
  replicatedSync: { slug: "sync" },
  appearance: { slug: "appearance" },
  reading: { slug: "triage", section: "mark-as-read" },
  defaultApps: { slug: "getting-started", section: "settings-worth-a-look" },
  splitInboxes: { slug: "triage", section: "split-inboxes" },
  mutedSenders: { slug: "triage", section: "newsletter-controls" },
  snippets: { slug: "compose", section: "snippets" },
  signatures: { slug: "compose", section: "signatures" },
  writingProfile: { slug: "ai", section: "features" },
  notes: { slug: "notes", section: "markdown-syntax" },
  ai: { slug: "ai" },
  research: { slug: "ai", section: "web-research" },
  privacy: { slug: "privacy" },
  diagnostics: { slug: "troubleshooting" },
  data: { slug: "settings-and-sync" },
};

/** Opens help in the system browser, keeping the application webview in place. */
export function openDocs(slug = "index", section?: string): Promise<void> {
  return openUrl(docsUrl(slug, section)).catch(() => {});
}
