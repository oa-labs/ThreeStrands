import type {
  Account,
  AuthStatus,
  AvailabilityPreferences,
  CalendarAccount,
  CalendarOption,
  Snippet,
  SplitInbox,
  SplitInboxMatchKind,
} from "./domain";
import type { Accent } from "./accent";
import type { Theme } from "./theme";
import type { FontFamily } from "./settings";

export type SettingsSection = "replicatedSync" | "appearance" | "defaultApps" | "accounts" | "calendarAccounts" | "availability" | "splitInboxes" | "snippets" | "ai" | "privacy" | "diagnostics" | "data";

export type SyncDiagnosticsActions = {
  retryFailed(): Promise<void>;
  dismissProblems(): Promise<void>;
  dismissRecovery(): void;
};

/**
 * Settings takes one object per domain rather than one prop per field. Each
 * object is structurally satisfied by the hook that owns that domain's state
 * (`useAppPreferences`, `useAccounts`, `useCalendarAccounts`,
 * `useSplitInboxes`, `useSnippets`), so App can hand them through unchanged.
 */
export type SettingsPreferences = {
  theme: Theme;
  setTheme(theme: Theme): void;
  accent: Accent;
  setAccent(accent: Accent): void;
  fontScale: number;
  setFontScale(value: number): void;
  emailMinimumFontSize: number;
  setEmailMinimumFontSize(value: number): void;
  fontFamily: FontFamily;
  setFontFamily(value: FontFamily): void;
  autoReadDelaySeconds: number;
  setAutoReadDelaySeconds(value: number): void;
  loadRemoteImages: boolean;
  setLoadRemoteImages(value: boolean): void;
  availabilityPreferences: AvailabilityPreferences;
  setAvailabilityPreferences(value: AvailabilityPreferences): void;
};

export type MailAccountSettings = {
  authStatus: AuthStatus | null;
  accounts: Account[];
  activeAccountId: string | null;
  add(): Promise<void>;
  remove(email: string): Promise<void>;
  removeEverywhere(email: string): Promise<void>;
  reconnect(email: string): Promise<void>;
  setDisplayName(email: string, displayName: string | null): Promise<void>;
  setColor(email: string, color: string): Promise<void>;
  reorder(emails: string[]): Promise<void>;
};

export type CalendarAccountSettingsState = {
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  calendarsError: string | null;
  /** True once the calendar list has loaded at least once. */
  calendarsLoaded: boolean;
  add(): Promise<void>;
  reconnect(email: string): Promise<void>;
  remove(email: string): Promise<void>;
  removeEverywhere(email: string): Promise<void>;
  setSelection(accountId: string, calendarIds: string[]): Promise<void>;
};

export type SplitInboxSettingsState = {
  splitInboxes: SplitInbox[];
  create(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<void>;
  rename(id: string, name: string): Promise<void>;
  remove(id: string): Promise<void>;
  reorder(ids: string[]): Promise<void>;
};

export type SnippetSettingsState = {
  snippets: Snippet[];
  create(name: string, body: string): Promise<Snippet>;
  update(id: string, name: string, body: string): Promise<Snippet>;
  remove(id: string): Promise<void>;
};
