import {
  Activity,
  AppWindow,
  ArrowLeftRight,
  CalendarDays,
  Check,
  Clock,
  Inbox,
  Mail,
  Network,
  Palette,
  Search,
  ShieldCheck,
  Sparkles,
  TextQuote,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { DefaultAppsSettings } from "./DefaultAppsSettings";
import type { Label, RecoveryStatus, SyncStatus } from "./domain";
import type { SettingsImportResult } from "./userPreferences";
import { queuePortablePreferences } from "./syncedPreferences";
import { ReplicatedSyncSettings } from "./ReplicatedSyncSettings";
import { PendingRecoveryPhraseDialog } from "./RecoveryPhraseDialog";
import { ICON_SIZE } from "./iconSizes";
import { DiagnosticsSettings } from "./DiagnosticsSettings";
import { AppearanceSettings, ReadingSettings } from "./AppearanceSettings";
import { AccountsSettings } from "./AccountsSettings";
import { AvailabilitySettings } from "./AvailabilitySettings";
import { CalendarAccountsSettings } from "./CalendarAccountsSettings";
import { SplitInboxesSettings } from "./SplitInboxesSettings";
import { SnippetsSettings } from "./SnippetsSettings";
import { AiProviderSettings } from "./AiProviderSettings";
import { PrivacySettings } from "./PrivacySettings";
import { DataTransferSettings } from "./DataTransferSettings";
import type {
  SettingsSection,
  SyncDiagnosticsActions,
  SettingsPreferences,
  MailAccountSettings,
  CalendarAccountSettingsState,
  SplitInboxSettingsState,
  SnippetSettingsState,
} from "./settingsPanelTypes";

type SettingsGroup = "General" | "Accounts" | "Workflow" | "Integrations" | "System";

type SettingsSectionDefinition = {
  id: SettingsSection;
  label: string;
  group: SettingsGroup;
  description: string;
  keywords: string;
  icon: LucideIcon;
  /** Every control on the page saves as it changes, with no Save button. */
  autosaves?: boolean;
};

const SETTINGS_GROUPS: SettingsGroup[] = ["General", "Accounts", "Workflow", "Integrations", "System"];

const SETTINGS_SECTIONS: SettingsSectionDefinition[] = [
  { id: "appearance", label: "Appearance", group: "General", description: "Choose how ThreeStrands looks and when conversations are marked read.", keywords: "theme light dark accent color font size family minimum email font size accessibility reading mark read delay conversation", icon: Palette, autosaves: true },
  { id: "defaultApps", label: "Default Apps", group: "General", description: "Open email links and calendar invitations in ThreeStrands.", keywords: "mailto default email reader mail client ics calendar invitation handler macos", icon: AppWindow },
  { id: "accounts", label: "Mail Accounts", group: "Accounts", description: "Connect mail accounts and manage their identity and order.", keywords: "gmail imap smtp sender name color reconnect disconnect", icon: Mail },
  { id: "calendarAccounts", label: "Calendar Accounts", group: "Accounts", description: "Connect calendars and choose which ones appear in the sidebar.", keywords: "google calendar connect selection", icon: CalendarDays },
  { id: "availability", label: "Availability", group: "Workflow", description: "Set your timezone, working hours, and meeting defaults.", keywords: "timezone working hours duration slots meetings", icon: Clock, autosaves: true },
  { id: "splitInboxes", label: "Split Inboxes", group: "Workflow", description: "Create focused inbox views for the messages that matter.", keywords: "filtered inbox domain label address pattern", icon: Inbox },
  { id: "snippets", label: "Snippets", group: "Workflow", description: "Manage reusable text for faster replies.", keywords: "canned text reply templates compose", icon: TextQuote },
  { id: "ai", label: "AI Provider", group: "Integrations", description: "Connect an AI provider and choose which features may use it.", keywords: "api key model endpoint draft summary actions", icon: Sparkles },
  { id: "replicatedSync", label: "Replicated Sync", group: "Integrations", description: "Configure end-to-end encrypted replication transports.", keywords: "folder ipfs rpc encrypted", icon: Network },
  { id: "privacy", label: "Privacy", group: "System", description: "Control local retention and remote message content.", keywords: "storage retention remote images cache", icon: ShieldCheck, autosaves: true },
  { id: "diagnostics", label: "Diagnostics", group: "System", description: "Inspect synchronization health and crash-reporting controls.", keywords: "sync status errors crash reports troubleshooting", icon: Activity, autosaves: true },
  { id: "data", label: "Data Transfer", group: "System", description: "Move encrypted settings and account metadata between devices.", keywords: "import export backup password", icon: ArrowLeftRight },
];

export function Settings({
  section,
  onSectionChange,
  onClose,
  preferences,
  mailAccounts,
  calendarAccounts,
  splitInboxes,
  labelsByAccount,
  snippets,
  syncStatus,
  recoveryStatus,
  syncDiagnostics,
  onAiConfigChange,
  onSettingsImported,
}: {
  section: SettingsSection;
  onSectionChange(section: SettingsSection): void;
  onClose(): void;
  preferences: SettingsPreferences;
  mailAccounts: MailAccountSettings;
  calendarAccounts: CalendarAccountSettingsState;
  splitInboxes: SplitInboxSettingsState;
  labelsByAccount: Record<string, Label[]>;
  snippets: SnippetSettingsState;
  syncStatus: SyncStatus | null;
  recoveryStatus: RecoveryStatus | null;
  syncDiagnostics?: SyncDiagnosticsActions;
  onAiConfigChange(): void;
  onSettingsImported(result: SettingsImportResult): Promise<void>;
}) {
  const [settingsQuery, setSettingsQuery] = useState("");
  const settingsPanelRef = useRef<HTMLDivElement>(null);
  const selectedSectionButtonRef = useRef<HTMLButtonElement>(null);
  // Replicated Sync's own section handles its "not enabled yet" state
  // itself (it shows the "enable replicated sync" toggle there) — the nav
  // entry must stay visible even before that toggle is on, or there would
  // be no way to reach the toggle at all.
  const availableSections = SETTINGS_SECTIONS;
  const normalizedSettingsQuery = settingsQuery.trim().toLocaleLowerCase();
  const visibleSections = availableSections.filter((item) =>
    !normalizedSettingsQuery
    || `${item.label} ${item.group} ${item.description} ${item.keywords}`.toLocaleLowerCase().includes(normalizedSettingsQuery)
  );
  const selectedSection = availableSections.find((item) => item.id === section) ?? availableSections[0]!;

  useEffect(() => {
    if (settingsPanelRef.current) settingsPanelRef.current.scrollTop = 0;
  }, [section]);

  return (
    <>
    <Modal title="Settings" className="settings-modal" initialFocusRef={selectedSectionButtonRef} onClose={onClose}>
      <div className="settings-body">
        <nav
          className="settings-nav"
          aria-label="Settings sections"
          onKeyDown={(event) => {
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            if (event.target instanceof HTMLInputElement) return;
            event.preventDefault();
            if (visibleSections.length === 0) return;
            const currentIndex = visibleSections.findIndex((item) => item.id === section);
            const delta = event.key === "ArrowDown" ? 1 : -1;
            const startIndex = currentIndex < 0 ? (delta === 1 ? -1 : 0) : currentIndex;
            const next = visibleSections[(startIndex + delta + visibleSections.length) % visibleSections.length]!;
            onSectionChange(next.id);
            event.currentTarget.querySelector<HTMLButtonElement>(`[data-section-id="${next.id}"]`)?.focus();
          }}
        >
          <label className="settings-search">
            <Search size={ICON_SIZE.sm} aria-hidden="true" />
            <input
              type="search"
              value={settingsQuery}
              aria-label="Search Settings"
              placeholder="Search settings"
              onChange={(event) => {
                const nextQuery = event.target.value;
                setSettingsQuery(nextQuery);
                const normalized = nextQuery.trim().toLocaleLowerCase();
                if (!normalized) return;
                const matches = availableSections.filter((item) =>
                  `${item.label} ${item.group} ${item.description} ${item.keywords}`.toLocaleLowerCase().includes(normalized)
                );
                const [firstMatch] = matches;
                if (firstMatch && !matches.some((item) => item.id === section)) {
                  onSectionChange(firstMatch.id);
                }
              }}
            />
          </label>
          <div className="settings-nav-sections">
            {SETTINGS_GROUPS.map((group) => {
              const groupSections = visibleSections.filter((item) => item.group === group);
              if (groupSections.length === 0) return null;
              return (
                <div className="settings-nav-group" key={group}>
                  <span className="settings-nav-group-label">{group}</span>
                  {groupSections.map((item) => (
                    <button
                      key={item.id}
                      type="button"
                      data-section-id={item.id}
                      className={`settings-nav-item${item.id === section ? " active" : ""}`}
                      aria-current={item.id === section ? "true" : undefined}
                      ref={item.id === section ? selectedSectionButtonRef : undefined}
                      onClick={() => onSectionChange(item.id)}
                    >
                      <item.icon size={ICON_SIZE.sm} aria-hidden="true" />
                      {item.label}
                    </button>
                  ))}
                </div>
              );
            })}
            {visibleSections.length === 0 ? <p className="settings-nav-empty">No settings found</p> : null}
          </div>
        </nav>
        <div className="settings-panel" ref={settingsPanelRef}>
          <header className="settings-page-header">
            <span className="settings-page-icon" aria-hidden="true">
              {visibleSections.length === 0 ? <Search size={ICON_SIZE.lg} /> : <selectedSection.icon size={ICON_SIZE.lg} />}
            </span>
            <div className="settings-page-title">
              <h2>{visibleSections.length === 0 ? "Search settings" : selectedSection.label}</h2>
              <p>{visibleSections.length === 0 ? "No matching controls or sections are currently visible." : selectedSection.description}</p>
            </div>
            {visibleSections.length > 0 && selectedSection.autosaves ? (
              <span className="settings-save-note"><Check size={ICON_SIZE.xs} aria-hidden="true" /> Changes save automatically</span>
            ) : null}
          </header>
          {visibleSections.length === 0 ? (
            <div className="settings-search-empty">
              <strong>No settings match “{settingsQuery.trim()}”</strong>
              <p>Try a feature name, account, privacy, or sync.</p>
              <button className="btn" type="button" onClick={() => setSettingsQuery("")}>Clear search</button>
            </div>
          ) : (
            <>
          {section === "replicatedSync" ? <ReplicatedSyncSettings /> : null}
          {section === "appearance" ? (
            <AppearanceSettings
              theme={preferences.theme}
              onThemeChange={preferences.setTheme}
              accent={preferences.accent}
              onAccentChange={preferences.setAccent}
              fontScale={preferences.fontScale}
              onFontScaleChange={preferences.setFontScale}
              emailMinimumFontSize={preferences.emailMinimumFontSize}
              onEmailMinimumFontSizeChange={preferences.setEmailMinimumFontSize}
              fontFamily={preferences.fontFamily}
              onFontFamilyChange={preferences.setFontFamily}
            />
          ) : null}
          {section === "appearance" ? (
            <ReadingSettings
              autoReadDelaySeconds={preferences.autoReadDelaySeconds}
              onAutoReadDelayChange={preferences.setAutoReadDelaySeconds}
            />
          ) : null}
          {section === "defaultApps" ? <DefaultAppsSettings /> : null}
          {section === "accounts" ? (
            <AccountsSettings
              authStatus={mailAccounts.authStatus}
              accounts={mailAccounts.accounts}
              onAdd={mailAccounts.add}
              onImapConnected={mailAccounts.refresh}
              onRemove={mailAccounts.remove}
              onRemoveEverywhere={mailAccounts.removeEverywhere}
              onReconnect={mailAccounts.reconnect}
              onSetDisplayName={mailAccounts.setDisplayName}
              onSetColor={mailAccounts.setColor}
              onReorder={mailAccounts.reorder}
            />
          ) : null}
          {section === "calendarAccounts" ? (
            <CalendarAccountsSettings
              authStatus={mailAccounts.authStatus}
              accounts={calendarAccounts.accounts}
              calendars={calendarAccounts.calendars}
              calendarsError={calendarAccounts.calendarsError}
              calendarsLoaded={calendarAccounts.calendarsLoaded}
              onAdd={calendarAccounts.add}
              onReconnect={calendarAccounts.reconnect}
              onRemove={calendarAccounts.remove}
              onRemoveEverywhere={calendarAccounts.removeEverywhere}
              onSetSelection={calendarAccounts.setSelection}
            />
          ) : null}
          {section === "availability" ? (
            <AvailabilitySettings
              preferences={preferences.availabilityPreferences}
              onChange={preferences.setAvailabilityPreferences}
            />
          ) : null}
          {section === "splitInboxes" ? (
            <SplitInboxesSettings
              splitInboxes={splitInboxes.splitInboxes}
              accounts={mailAccounts.accounts}
              activeAccountId={mailAccounts.activeAccountId}
              labelsByAccount={labelsByAccount}
              onCreate={splitInboxes.create}
              onRename={splitInboxes.rename}
              onDelete={splitInboxes.remove}
              onReorder={splitInboxes.reorder}
            />
          ) : null}
          {section === "snippets" ? (
            <SnippetsSettings
              snippets={snippets.snippets}
              onCreate={snippets.create}
              onUpdate={snippets.update}
              onDelete={snippets.remove}
            />
          ) : null}
          {section === "ai" ? <AiProviderSettings onChange={() => { onAiConfigChange(); queuePortablePreferences(); }} /> : null}
          {section === "privacy" ? (
            <PrivacySettings
              loadRemoteImages={preferences.loadRemoteImages}
              onLoadRemoteImagesChange={preferences.setLoadRemoteImages}
            />
          ) : null}
          {section === "diagnostics" ? (
            <DiagnosticsSettings
              status={syncStatus}
              recovery={recoveryStatus}
              accountCount={mailAccounts.accounts.length}
              actions={syncDiagnostics}
            />
          ) : null}
          {section === "data" ? <DataTransferSettings onImported={onSettingsImported} /> : null}
            </>
          )}
        </div>
      </div>
    </Modal>
    {/* A sibling after Settings, not a child: effects run children-first, so
        nesting it would let Settings mark it inert and take the Escape stack
        above it whenever Settings reopens with a phrase still pending. */}
    <PendingRecoveryPhraseDialog />
    </>
  );
}
