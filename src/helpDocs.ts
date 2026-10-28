import { openUrl } from "@tauri-apps/plugin-opener";
import { docsUrl } from "./gettingStartedSteps";
import type { SettingsSection } from "./settingsPanelTypes";

/** Context links are shared with docs validation; credentials never enter URLs. */
export const SETTINGS_HELP: Record<SettingsSection, { slug: string; section?: string }> = {
  accounts: { slug: "mail-accounts" },
  calendarAccounts: { slug: "calendar", section: "connect-google-calendar" },
  availability: { slug: "calendar", section: "schedule" },
  replicatedSync: { slug: "sync" },
  appearance: { slug: "appearance" },
  defaultApps: { slug: "getting-started", section: "settings-worth-a-look" },
  splitInboxes: { slug: "triage", section: "split-inboxes" },
  mutedSenders: { slug: "triage", section: "newsletter-controls" },
  snippets: { slug: "compose", section: "snippets" },
  signatures: { slug: "compose", section: "signatures" },
  writingProfile: { slug: "ai", section: "features" },
  ai: { slug: "ai" },
  privacy: { slug: "privacy" },
  diagnostics: { slug: "troubleshooting" },
  data: { slug: "settings-and-sync" },
};

/** Opens help in the system browser, keeping the application webview in place. */
export function openDocs(slug = "index", section?: string): Promise<void> {
  return openUrl(docsUrl(slug, section)).catch(() => {});
}
