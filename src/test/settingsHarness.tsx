import { useState } from "react";
import { vi } from "vitest";
import type { Account } from "../domain";
import { Settings } from "../SettingsPanel";
import type { SettingsSection } from "../settingsPanelTypes";
import { useAppPreferences } from "../useAppPreferences";

export const settingsAccount: Account = {
  email: "me@example.com",
  displayName: null,
  color: "#112233",
  status: "connected",
  provider: "gmail",
  labelModel: "gmailLabels",
  sortOrder: 0,
  connectedAt: "2026-10-08T12:00:00Z",
  lastSyncedAt: null,
  onThisDevice: true,
};

/**
 * The Settings dialog with real preferences and stub account, calendar, and
 * library state, so section behavior can be tested without mounting the App.
 */
export function SettingsHarness({ initialSection = "appearance" }: { initialSection?: SettingsSection }) {
  const [section, setSection] = useState<SettingsSection>(initialSection);
  const preferences = useAppPreferences();
  return <Settings
    section={section}
    onSectionChange={setSection}
    onClose={vi.fn()}
    preferences={preferences}
    mailAccounts={{
      authStatus: { configured: true, connected: true },
      accounts: [settingsAccount],
      activeAccountId: settingsAccount.email,
      add: vi.fn(), remove: vi.fn(), removeEverywhere: vi.fn(), reconnect: vi.fn(),
      setDisplayName: vi.fn(), setNickname: vi.fn(), setColor: vi.fn(), reorder: vi.fn(),
    }}
    calendarAccounts={{
      accounts: [], calendars: [], calendarsError: null, calendarsLoaded: true,
      add: vi.fn(), reconnect: vi.fn(), remove: vi.fn(), removeEverywhere: vi.fn(), reorder: vi.fn(),
      setSelection: vi.fn(), setAvailabilitySelection: vi.fn(),
    }}
    splitInboxes={{ splitInboxes: [], create: vi.fn(), rename: vi.fn(), remove: vi.fn(), reorder: vi.fn() }}
    mutedSenders={{ senderRules: [], unmute: vi.fn() }}
    labelsByAccount={{}}
    snippets={{ snippets: [], create: vi.fn(), update: vi.fn(), remove: vi.fn() }}
    syncStatus={null}
    recoveryStatus={null}
    onAiConfigChange={vi.fn()}
    onSettingsImported={vi.fn()}
  />;
}
