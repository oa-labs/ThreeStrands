import {
  Activity,
  AlertCircle,
  ArrowLeftRight,
  BookOpen,
  CalendarDays,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Clock,
  Download,
  Inbox,
  Mail,
  Network,
  Palette,
  Plus,
  RefreshCw,
  Search,
  ShieldCheck,
  Sparkles,
  TextQuote,
  Upload,
  type LucideIcon,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  clearLocalCrashReports,
  crashReportingEnabled,
  localCrashReports,
  setCrashReportingEnabled,
} from "./crashReporting";
import { formatLabelName } from "./labels";
import { Modal } from "./AppChrome";
import { InlineConfirm } from "./InlineConfirm";
import type {
  Account,
  AuthStatus,
  AvailabilityPreferences,
  CalendarAccount,
  CalendarOption,
  Label,
  RecoveryStatus,
  Snippet,
  SplitInbox,
  SplitInboxMatchKind,
  SyncStatus,
} from "./domain";
import { snippetBodyPreview } from "./snippets";
import { SnippetEditor } from "./SnippetPicker";
import { formatTimeOnly } from "./threadPresentation";
import {
  FONT_SCALE_STEP,
  MAX_FONT_SCALE,
  MIN_FONT_SCALE,
} from "./fontScale";
import { ACCENTS, type Accent } from "./accent";
import type { Theme } from "./theme";
import {
  DEFAULT_FONT_FAMILY,
  fontFamilyStack,
  MAX_AUTO_READ_DELAY_SECONDS,
  MIN_AUTO_READ_DELAY_SECONDS,
  type FontFamily,
} from "./settings";
import { listSystemFontFamilies } from "./systemFonts";
import {
  AI_MODEL_PLACEHOLDERS,
  AI_MODEL_SUGGESTIONS,
  AI_PROVIDER_OPTIONS,
  clearAiApiKey,
  isAiApiKeyConfigured,
  readAiEndpoint,
  readAiFeatures,
  readAiModel,
  readAiProvider,
  resolveAiModel,
  saveAiEndpoint,
  saveAiFeatures,
  saveAiModel,
  saveAiProvider,
  setAiApiKey,
  testAiConnection,
  type AiFeatureFlags,
  type AiProvider,
} from "./aiSettings";
import { getRetentionDays, setRetentionDays, RETENTION_OPTIONS } from "./retentionSettings";
import { exportSettings, importSettings, type SettingsImportResult } from "./userPreferences";
import { queuePortablePreferences } from "./syncedPreferences";
import { ReplicatedSyncSettings } from "./ReplicatedSyncSettings";
import { PendingRecoveryPhraseDialog } from "./RecoveryPhraseDialog";
import { moveItem, useSettingsOperation } from "./settingsOperations";
import { errorMessage, logBackgroundFailure } from "./errors";
import { AiUsageSummary } from "./AiUsageSummary";
import { MIN_PROACTIVE_DWELL_SECONDS } from "./proactiveBrief";

export type SettingsSection = "replicatedSync" | "appearance" | "reading" | "accounts" | "calendarAccounts" | "availability" | "splitInboxes" | "snippets" | "ai" | "privacy" | "diagnostics" | "data";

function recoveryStatusMessage(recovery: RecoveryStatus): string {
  switch (recovery.kind) {
    case "restoredFromBackup":
      return "Your mail cache was damaged and has been restored from its most recent local backup. " +
        "A few of the most recent changes may be missing until the next sync.";
    case "freshDatabase":
      return "Your mail cache was damaged and could not be restored from a backup, so it was rebuilt " +
        "from scratch. Your mail is safe on the server; ThreeStrands is resyncing it now.";
  }
}

function SyncDiagnosticsDetails({
  status,
  recovery,
  accountCount,
}: {
  status: SyncStatus | null;
  recovery?: RecoveryStatus | null;
  /** Connected mail accounts. The merged status only carries a cursor when
   * there is exactly one, so the row is meaningless beyond that. */
  accountCount: number;
}) {
  return (
    <dl className="diagnostics">
      {recovery ? (
        <>
          <dt>Database recovery</dt>
          <dd className="notice">{recoveryStatusMessage(recovery)}</dd>
        </>
      ) : null}
      <dt>State</dt><dd>{status?.state ?? "unknown"}</dd>
      <dt>Last successful sync</dt>
      <dd>{status?.lastSuccessfulSync ? new Date(status.lastSuccessfulSync).toLocaleString() : "Never"}</dd>
      {accountCount <= 1 ? (
        <>
          <dt>History cursor</dt>
          <dd>{status?.cursor ?? "Not initialized"}</dd>
        </>
      ) : null}
      <dt>Pending mutations</dt><dd>{status?.pendingMutations ?? 0}</dd>
      <dt>Permanently failed operations</dt>
      <dd>
        {status?.failedMutations?.length ? (
          <ul className="failed-mutations">
            {status.failedMutations.map((mutation) => (
              <li key={mutation.id}>
                <strong>{mutation.kind}</strong>
                {" · "}
                {mutation.error}
                <small>
                  {mutation.attempts} {mutation.attempts === 1 ? "attempt" : "attempts"}
                  {" · "}
                  {new Date(mutation.createdAt).toLocaleString()}
                </small>
              </li>
            ))}
          </ul>
        ) : "None"}
      </dd>
      <dt>Quarantined messages</dt>
      <dd>
        {status?.quarantinedMessages?.length ? (
          <ul className="failed-mutations">
            {status.quarantinedMessages.map((message) => (
              <li key={`${message.threadId}:${message.messageId}`}>
                <strong>Message {message.messageId}</strong>
                {" · "}
                {message.error}
                <small>
                  Thread {message.threadId}
                  {" · "}
                  {new Date(message.createdAt).toLocaleString()}
                </small>
              </li>
            ))}
          </ul>
        ) : "None"}
      </dd>
      <dt>Last error</dt><dd>{status?.error ?? "None"}</dd>
    </dl>
  );
}

export function DiagnosticsSettings({
  status,
  recovery,
  accountCount,
}: {
  status: SyncStatus | null;
  recovery?: RecoveryStatus | null;
  accountCount: number;
}) {
  const [reporting, setReporting] = useState(crashReportingEnabled);
  const [reportCount, setReportCount] = useState(() => localCrashReports().length);

  return (
    <section className="settings-section" aria-label="Diagnostics">
      <h3>Sync Diagnostics</h3>
      <p className="settings-hint">
        This information can help troubleshoot synchronization problems. Most people will not need to change anything here.
      </p>
      <SyncDiagnosticsDetails status={status} recovery={recovery} accountCount={accountCount} />

      <h3>Crash Reports</h3>
      <label className="settings-switch">
        <span>Share Sanitized Crash Reports</span>
        <input
          type="checkbox"
          checked={reporting}
          onChange={(event) => {
            setReporting(event.target.checked);
            setCrashReportingEnabled(event.target.checked);
          }}
        />
      </label>
      <span className="settings-hint">
        Disabled by default. Email addresses and URLs are redacted.{" "}
        Policy: <code>docs/crash-reporting.md</code>
      </span>
      <button
        disabled={reportCount === 0}
        onClick={() => {
          clearLocalCrashReports();
          setReportCount(0);
        }}
      >
        Clear {reportCount} local {reportCount === 1 ? "report" : "reports"}
      </button>
    </section>
  );
}

type SettingsGroup = "General" | "Accounts" | "Workflow" | "Integrations" | "System";

type SettingsSectionDefinition = {
  id: SettingsSection;
  label: string;
  group: SettingsGroup;
  description: string;
  keywords: string;
  icon: LucideIcon;
};

const SETTINGS_GROUPS: SettingsGroup[] = ["General", "Accounts", "Workflow", "Integrations", "System"];

const SETTINGS_SECTIONS: SettingsSectionDefinition[] = [
  { id: "appearance", label: "Appearance", group: "General", description: "Choose how ThreeStrands looks and reads.", keywords: "theme light dark accent color font size family", icon: Palette },
  { id: "reading", label: "Reading", group: "General", description: "Control what happens when you open a conversation.", keywords: "mark read delay conversation", icon: BookOpen },
  { id: "accounts", label: "Mail Accounts", group: "Accounts", description: "Connect mail accounts and manage their identity and order.", keywords: "gmail sender name color reconnect disconnect", icon: Mail },
  { id: "calendarAccounts", label: "Calendar Accounts", group: "Accounts", description: "Connect calendars and choose which ones appear in the sidebar.", keywords: "google calendar connect selection", icon: CalendarDays },
  { id: "availability", label: "Availability", group: "Workflow", description: "Set your timezone, working hours, and meeting defaults.", keywords: "timezone working hours duration slots meetings", icon: Clock },
  { id: "splitInboxes", label: "Split Inboxes", group: "Workflow", description: "Create focused inbox views for the messages that matter.", keywords: "filtered inbox domain label address pattern", icon: Inbox },
  { id: "snippets", label: "Snippets", group: "Workflow", description: "Manage reusable text for faster replies.", keywords: "canned text reply templates compose", icon: TextQuote },
  { id: "ai", label: "AI Provider", group: "Integrations", description: "Connect an AI provider and choose which features may use it.", keywords: "api key model endpoint draft summary actions", icon: Sparkles },
  { id: "replicatedSync", label: "Replicated Sync", group: "Integrations", description: "Configure end-to-end encrypted replication transports.", keywords: "folder ipfs rpc encrypted", icon: Network },
  { id: "privacy", label: "Privacy", group: "System", description: "Control local retention and remote message content.", keywords: "storage retention remote images cache", icon: ShieldCheck },
  { id: "diagnostics", label: "Diagnostics", group: "System", description: "Inspect synchronization health and crash-reporting controls.", keywords: "sync status errors crash reports troubleshooting", icon: Activity },
  { id: "data", label: "Data Transfer", group: "System", description: "Move encrypted settings and account metadata between devices.", keywords: "import export backup password", icon: ArrowLeftRight },
];

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
            <Search size={14} aria-hidden="true" />
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
                if (matches.length > 0 && !matches.some((item) => item.id === section)) {
                  onSectionChange(matches[0]!.id);
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
                      className={item.id === section ? "active" : ""}
                      aria-current={item.id === section ? "true" : undefined}
                      ref={item.id === section ? selectedSectionButtonRef : undefined}
                      onClick={() => onSectionChange(item.id)}
                    >
                      <item.icon size={15} aria-hidden="true" />
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
              {visibleSections.length === 0 ? <Search size={18} /> : <selectedSection.icon size={18} />}
            </span>
            <div className="settings-page-title">
              <h2>{visibleSections.length === 0 ? "Search settings" : selectedSection.label}</h2>
              <p>{visibleSections.length === 0 ? "No matching controls or sections are currently visible." : selectedSection.description}</p>
            </div>
            <span className="settings-save-note"><Check size={13} aria-hidden="true" /> Preference changes save automatically</span>
          </header>
          {visibleSections.length === 0 ? (
            <div className="settings-search-empty">
              <strong>No settings match “{settingsQuery.trim()}”</strong>
              <p>Try a feature name, account, privacy, or sync.</p>
              <button type="button" onClick={() => setSettingsQuery("")}>Clear search</button>
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
              fontFamily={preferences.fontFamily}
              onFontFamilyChange={preferences.setFontFamily}
            />
          ) : null}
          {section === "reading" ? (
            <ReadingSettings
              autoReadDelaySeconds={preferences.autoReadDelaySeconds}
              onAutoReadDelayChange={preferences.setAutoReadDelaySeconds}
            />
          ) : null}
          {section === "accounts" ? (
            <AccountsSettings
              authStatus={mailAccounts.authStatus}
              accounts={mailAccounts.accounts}
              onAdd={mailAccounts.add}
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
            <DiagnosticsSettings status={syncStatus} recovery={recoveryStatus} accountCount={mailAccounts.accounts.length} />
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

function AppearanceSettings({
  theme,
  onThemeChange,
  accent,
  onAccentChange,
  fontScale,
  onFontScaleChange,
  fontFamily,
  onFontFamilyChange,
}: {
  theme: Theme;
  onThemeChange(theme: Theme): void;
  accent: Accent;
  onAccentChange(accent: Accent): void;
  fontScale: number;
  onFontScaleChange(value: number): void;
  fontFamily: FontFamily;
  onFontFamilyChange(value: FontFamily): void;
}) {
  const [fontFamilies, setFontFamilies] = useState<string[]>([]);
  const [fontQuery, setFontQuery] = useState("");
  const [fontsLoading, setFontsLoading] = useState(true);
  const [fontLoadFailed, setFontLoadFailed] = useState(false);
  const [fontPickerOpen, setFontPickerOpen] = useState(false);
  const fontPickerRef = useRef<HTMLDivElement>(null);
  const fontTriggerRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    let cancelled = false;
    setFontsLoading(true);
    setFontLoadFailed(false);
    listSystemFontFamilies()
      .then((families) => {
        if (!cancelled) setFontFamilies(families);
      })
      .catch(() => {
        if (!cancelled) setFontLoadFailed(true);
      })
      .finally(() => {
        if (!cancelled) setFontsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);
  useEffect(() => {
    if (!fontPickerOpen) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (event.target instanceof Node && !fontPickerRef.current?.contains(event.target)) setFontPickerOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setFontPickerOpen(false);
        window.setTimeout(() => fontTriggerRef.current?.focus(), 0);
      }
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [fontPickerOpen]);
  const normalizedFontQuery = fontQuery.trim().toLocaleLowerCase();
  const visibleFontFamilies = fontFamilies.filter((family) =>
    family.toLocaleLowerCase().includes(normalizedFontQuery)
  );
  const showSystemFont = !normalizedFontQuery
    || "system default".includes(normalizedFontQuery);
  const selectedFontIsInstalled = fontFamily === DEFAULT_FONT_FAMILY
    || fontFamilies.includes(fontFamily);
  const displayedFontFamilies = visibleFontFamilies.slice(0, 8);
  const hiddenFontCount = Math.max(0, visibleFontFamilies.length - displayedFontFamilies.length);
  const chooseFont = (next: FontFamily) => {
    onFontFamilyChange(next);
    setFontPickerOpen(false);
    setFontQuery("");
    window.setTimeout(() => fontTriggerRef.current?.focus(), 0);
  };
  const themeOptions: { value: Theme; label: string }[] = [
    { value: "system", label: "Match System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];
  const accentOptions: { value: Accent; label: string }[] = ACCENTS.map((value) => ({
    value,
    label: value.charAt(0).toLocaleUpperCase() + value.slice(1),
  }));
  return (
    <section className="settings-section" aria-label="Appearance">
      <h3>Theme</h3>
      <div className="settings-radio-row" role="radiogroup" aria-label="Theme">
        {themeOptions.map((option) => (
          <label key={option.value}>
            <input
              type="radio"
              name="theme"
              checked={theme === option.value}
              onChange={() => onThemeChange(option.value)}
            />
            {option.label}
          </label>
        ))}
      </div>

      <h3>Accent Color</h3>
      <p className="settings-hint">Used for highlights, selection, and buttons throughout the app.</p>
      <div className="settings-radio-row" role="radiogroup" aria-label="Accent color">
        {accentOptions.map((option) => (
          <label key={option.value}>
            <input
              type="radio"
              name="accent"
              checked={accent === option.value}
              onChange={() => onAccentChange(option.value)}
            />
            <span className="accent-swatch" data-accent={option.value} aria-hidden="true" />
            {option.label}
          </label>
        ))}
      </div>

      <h3>Font Size</h3>
      <div className="settings-row">
        <input
          type="range"
          min={MIN_FONT_SCALE}
          max={MAX_FONT_SCALE}
          step={FONT_SCALE_STEP}
          value={fontScale}
          aria-label="Font Size"
          onChange={(event) => onFontScaleChange(Number(event.target.value))}
        />
        <span>{fontScale}%</span>
      </div>

      <h3>Default Font</h3>
      <p className="settings-hint">Used throughout the app and for unformatted message text.</p>
      <div className="font-picker-control" ref={fontPickerRef}>
        <button
          type="button"
          ref={fontTriggerRef}
          className="font-picker-trigger"
          aria-label={`Default font: ${fontFamily === DEFAULT_FONT_FAMILY ? "System Default" : fontFamily}`}
          aria-haspopup="dialog"
          aria-expanded={fontPickerOpen}
          onClick={() => setFontPickerOpen((open) => !open)}
          style={{ fontFamily: fontFamilyStack(fontFamily) }}
        >
          <span>{fontFamily === DEFAULT_FONT_FAMILY ? "System Default" : fontFamily}</span>
          <span className="font-option-preview" aria-hidden="true">Aa</span>
          <ChevronDown size={15} aria-hidden="true" />
        </button>
        {fontPickerOpen ? (
          <div className="font-picker-popover" role="dialog" aria-label="Choose default font">
            <label className="font-search">
              <Search size={15} aria-hidden="true" />
              <input
                type="search"
                autoFocus
                value={fontQuery}
                placeholder="Search installed fonts"
                aria-label="Search Installed Fonts"
                onChange={(event) => setFontQuery(event.target.value)}
              />
            </label>
            <div className="font-picker" role="radiogroup" aria-label="Default font">
              {showSystemFont ? (
                <label className={`font-option${fontFamily === DEFAULT_FONT_FAMILY ? " selected" : ""}`} style={{ fontFamily: fontFamilyStack(DEFAULT_FONT_FAMILY) }}>
                  <input type="radio" name="default-font" value={DEFAULT_FONT_FAMILY} checked={fontFamily === DEFAULT_FONT_FAMILY} onChange={() => chooseFont(DEFAULT_FONT_FAMILY)} />
                  <span>System Default</span>
                  <span className="font-option-preview" aria-hidden="true">Aa</span>
                </label>
              ) : null}
              {!fontsLoading && !selectedFontIsInstalled && fontFamily !== DEFAULT_FONT_FAMILY ? (
                <label className="font-option selected" style={{ fontFamily: fontFamilyStack(fontFamily) }}>
                  <input type="radio" name="default-font" value={fontFamily} checked readOnly />
                  <span>{fontFamily} <small>Unavailable</small></span>
                  <span className="font-option-preview" aria-hidden="true">Aa</span>
                </label>
              ) : null}
              {displayedFontFamilies.map((family) => (
                <label key={family} className={`font-option${fontFamily === family ? " selected" : ""}`} style={{ fontFamily: fontFamilyStack(family) }}>
                  <input type="radio" name="default-font" value={family} checked={fontFamily === family} onChange={() => chooseFont(family)} />
                  <span>{family}</span>
                  <span className="font-option-preview" aria-hidden="true">Aa</span>
                </label>
              ))}
              {fontsLoading ? <p className="font-picker-status">Loading installed fonts…</p> : null}
              {fontLoadFailed ? <p className="font-picker-status">Installed fonts couldn’t be loaded. System default remains available.</p> : null}
              {!fontsLoading && !fontLoadFailed && !showSystemFont && visibleFontFamilies.length === 0 && normalizedFontQuery ? <p className="font-picker-status">No installed fonts match “{fontQuery.trim()}”.</p> : null}
              {!fontsLoading && hiddenFontCount > 0 ? <p className="font-picker-status">Showing 8 of {visibleFontFamilies.length}. Search to narrow the list.</p> : null}
            </div>
          </div>
        ) : null}
      </div>
    </section>
  );
}

function ReadingSettings({
  autoReadDelaySeconds,
  onAutoReadDelayChange,
}: {
  autoReadDelaySeconds: number;
  onAutoReadDelayChange(value: number): void;
}) {
  return (
    <section className="settings-section" aria-label="Reading">
      <h3>Mark as Read</h3>
      <label className="settings-field settings-field-inline">
        <span>After Opening a Conversation</span>
        <div className="settings-row">
          <input
            type="number"
            min={MIN_AUTO_READ_DELAY_SECONDS}
            max={MAX_AUTO_READ_DELAY_SECONDS}
            step="1"
            value={autoReadDelaySeconds}
            aria-label="Auto-Read Delay"
            onChange={(event) => {
              if (event.target.value === "") return;
              const next = Number(event.target.value);
              if (Number.isFinite(next)) onAutoReadDelayChange(next);
            }}
          />
          <span>seconds</span>
        </div>
      </label>
      <p className="settings-hint">
        Set to 0 to mark conversations read immediately. The timer resets when you open a different conversation.
      </p>
    </section>
  );
}

function AccountsSettings({
  authStatus,
  accounts,
  onAdd,
  onRemove,
  onRemoveEverywhere,
  onReconnect,
  onSetDisplayName,
  onSetColor,
  onReorder,
}: {
  authStatus: AuthStatus | null;
  accounts: Account[];
  onAdd(): Promise<void>;
  onRemove(email: string): Promise<void>;
  onRemoveEverywhere(email: string): Promise<void>;
  onReconnect(email: string): Promise<void>;
  onSetDisplayName(email: string, displayName: string | null): Promise<void>;
  onSetColor(email: string, color: string): Promise<void>;
  onReorder(emails: string[]): Promise<void>;
}) {
  const { pending: busyEmail, error, setError, runFor } = useSettingsOperation();
  const [confirmEverywhereEmail, setConfirmEverywhereEmail] = useState<string | null>(null);

  const move = (index: number, direction: -1 | 1) => {
    const next = moveItem(accounts, index, direction);
    if (next) runFor(accounts[index]!.email, () => onReorder(next.map((account) => account.email)));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Mail Accounts">
      <div className="accounts-manager-header">
        <div>
          <h3>Connected Mail Accounts</h3>
          <p>
            ThreeStrands keeps accounts separate and merges their inboxes by default.
            Use the sidebar or command palette to filter to one account.
          </p>
        </div>
        <button
          type="button"
          className="primary-action settings-add-account"
          disabled={busyEmail !== null}
          onClick={() => runFor("__add__", onAdd)}
        >
          <Plus size={15} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Add Account"}
        </button>
      </div>
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="notice accounts-config-notice">
          <AlertCircle size={16} />
          <div>
            <strong>Google OAuth is not configured</strong>
            <p>
              Set <code>THREESTRANDS_GOOGLE_CLIENT_ID</code> and{" "}
              <code>THREESTRANDS_GOOGLE_CLIENT_SECRET</code> from a Google Desktop
              app credential, then restart ThreeStrands.
            </p>
          </div>
        </div>
      ) : null}
      {accounts.length === 0 ? (
        <div className="accounts-empty">
          <span className="accounts-empty-icon"><Mail size={18} /></span>
          <strong>No accounts connected</strong>
          <p>Add a Gmail account to start syncing mail on this device.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {accounts.map((account, index) => (
            <li className="account-card" key={account.email}>
              <div className="account-card-row">
                <span className="account-card-avatar" aria-hidden="true" style={{ background: account.color }}>
                  {(account.displayName ?? account.email).charAt(0).toUpperCase()}
                </span>
                <div className="account-card-identity">
                  <div className="account-card-heading">
                    <strong>{account.displayName ?? account.email}</strong>
                    <span className={`account-status ${account.status}`}>
                      {account.status === "needs_reauth" ? <AlertCircle size={13} /> : <CheckCircle2 size={13} />}
                      {account.status === "needs_reauth" ? "Needs reconnect" : "Connected"}
                    </span>
                  </div>
                  <span className="account-card-email">
                    {account.displayName ? `${account.email} · ` : null}
                    {account.lastSyncedAt ? `Last synced ${formatTimeOnly(account.lastSyncedAt)}` : "Not synced yet"}
                  </span>
                </div>
              </div>
              <div className="account-card-controls">
                <AccountSenderNameInput
                  email={account.email}
                  name={account.displayName}
                  onCommit={(name) =>
                    onSetDisplayName(account.email, name).catch((reason: unknown) => {
                      setError(errorMessage(reason));
                      throw reason;
                    })
                  }
                />
                <span className="accounts-list-actions">
                  <span className="account-reorder">
                    <button
                      type="button"
                      aria-label={`Move ${account.email} up`}
                      disabled={index === 0 || busyEmail !== null}
                      onClick={() => move(index, -1)}
                    >
                      <ChevronUp size={14} />
                    </button>
                    <button
                      type="button"
                      aria-label={`Move ${account.email} down`}
                      disabled={index === accounts.length - 1 || busyEmail !== null}
                      onClick={() => move(index, 1)}
                    >
                      <ChevronDown size={14} />
                    </button>
                  </span>
                  <label className="account-color-swatch" title={`Color for ${account.email}`}>
                    <AccountColorInput
                      email={account.email}
                      color={account.color}
                      onCommit={(color) =>
                        onSetColor(account.email, color).catch((reason: unknown) => {
                          setError(errorMessage(reason));
                          throw reason;
                        })
                      }
                    />
                  </label>
                  {account.status === "needs_reauth" ? (
                    <button
                      type="button"
                      className="account-action-button"
                      disabled={busyEmail !== null}
                      onClick={() => runFor(account.email, () => onReconnect(account.email))}
                    >
                      Reconnect
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyEmail !== null}
                    onClick={() => runFor(account.email, () => onRemove(account.email))}
                  >
                    Disconnect
                  </button>
                  <button
                    type="button"
                    className="account-action-button"
                    disabled={busyEmail !== null}
                    aria-expanded={confirmEverywhereEmail === account.email}
                    onClick={() => setConfirmEverywhereEmail(account.email)}
                  >
                    More…
                  </button>
                </span>
              </div>
              {confirmEverywhereEmail === account.email ? (
                <InlineConfirm
                  ariaLabel="Remove mail account from all devices confirmation"
                  cancelLabel="Cancel"
                  onCancel={() => setConfirmEverywhereEmail(null)}
                  disabled={busyEmail !== null}
                  actions={[{ label: "Remove on all devices", className: "danger-action", onClick: () => {
                    runFor(account.email, () => onRemoveEverywhere(account.email));
                    setConfirmEverywhereEmail(null);
                  } }]}
                >
                  <strong>Remove from every device?</strong><br />This disconnects {account.email} everywhere. Gmail itself is not changed.
                </InlineConfirm>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      <p className="accounts-footnote">
        Disconnecting removes this account and its local ThreeStrands cache. Gmail and the account itself are not changed.
      </p>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

function AvailabilitySettings({
  preferences,
  onChange,
}: {
  preferences: AvailabilityPreferences;
  onChange(value: AvailabilityPreferences): void;
}) {
  const weekdayLabels = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
  const [timeZoneDraft, setTimeZoneDraft] = useState(preferences.timeZone);
  const [timeZoneError, setTimeZoneError] = useState<string | null>(null);
  const systemTimeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const timeZones = useMemo(() => {
    try {
      return Intl.supportedValuesOf("timeZone");
    } catch {
      return ["UTC"];
    }
  }, []);

  useEffect(() => setTimeZoneDraft(preferences.timeZone), [preferences.timeZone]);

  const commitTimeZone = (value: string) => {
    const normalized = value.trim();
    try {
      new Intl.DateTimeFormat("en-US", { timeZone: normalized }).format();
      setTimeZoneDraft(normalized);
      setTimeZoneError(null);
      if (normalized !== preferences.timeZone) onChange({ ...preferences, timeZone: normalized });
    } catch {
      setTimeZoneError("Choose a valid timezone, such as America/New_York.");
    }
  };
  const updateWindow = (weekday: number, patch: Partial<{ start: string; end: string }>) => {
    const current = preferences.workingWindows.find((window) => window.weekday === weekday);
    const next = current
      ? preferences.workingWindows.map((window) => window.weekday === weekday ? { ...window, ...patch } : window)
      : [...preferences.workingWindows, { weekday, start: patch.start ?? "09:00", end: patch.end ?? "17:00" }];
    onChange({ ...preferences, workingWindows: next });
  };
  return (
    <section className="settings-section" aria-label="Availability">
      <h3>Timezone</h3>
      <label className="settings-field">
        <span>Timezone</span>
        <input
          list="availability-timezones"
          value={timeZoneDraft}
          aria-label="Availability Timezone"
          aria-invalid={timeZoneError ? "true" : undefined}
          onChange={(event) => {
            setTimeZoneDraft(event.target.value);
            setTimeZoneError(null);
          }}
          onBlur={(event) => commitTimeZone(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitTimeZone(event.currentTarget.value);
            }
          }}
        />
        <datalist id="availability-timezones">
          {timeZones.map((timeZone) => <option key={timeZone} value={timeZone} />)}
        </datalist>
      </label>
      <div className="settings-row">
        <button type="button" onClick={() => commitTimeZone(systemTimeZone)}>Use system timezone</button>
        <span className="settings-hint">Times include daylight-saving transitions.</span>
      </div>
      {timeZoneError ? <p className="form-error" role="alert">{timeZoneError}</p> : null}
      <div className="settings-section-heading-row">
        <h3>Working Hours</h3>
        <span className="settings-section-heading-actions">
          <button
            type="button"
            onClick={() => {
              const monday = preferences.workingWindows.find((window) => window.weekday === 1)
                ?? { weekday: 1, start: "09:00", end: "17:00" };
              const weekends = preferences.workingWindows.filter((window) => window.weekday === 0 || window.weekday === 6);
              onChange({
                ...preferences,
                workingWindows: [
                  ...weekends,
                  ...[1, 2, 3, 4, 5].map((weekday) => ({ weekday, start: monday.start, end: monday.end })),
                ].sort((a, b) => a.weekday - b.weekday),
              });
            }}
          >
            Copy Monday to weekdays
          </button>
          <button type="button" onClick={() => onChange({ ...preferences, workingWindows: [] })}>Clear</button>
        </span>
      </div>
      <div className="availability-windows">
        {weekdayLabels.map((label, weekday) => {
          const window = preferences.workingWindows.find((candidate) => candidate.weekday === weekday);
          return (
            <div className="availability-window" key={label}>
              <label><input type="checkbox" checked={Boolean(window)} onChange={(event) => {
                if (event.target.checked) updateWindow(weekday, {});
                else onChange({ ...preferences, workingWindows: preferences.workingWindows.filter((candidate) => candidate.weekday !== weekday) });
              }} /> {label}</label>
              {window ? <>
                <input type="time" aria-label={`${label} start`} value={window.start} onChange={(event) => updateWindow(weekday, { start: event.target.value })} />
                <span>to</span>
                <input type="time" aria-label={`${label} end`} value={window.end} onChange={(event) => updateWindow(weekday, { end: event.target.value })} />
              </> : <span className="settings-hint">Unavailable</span>}
            </div>
          );
        })}
      </div>
      <h3>Meeting Defaults</h3>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Default Duration</span><select value={preferences.defaultDurationMinutes} onChange={(event) => onChange({ ...preferences, defaultDurationMinutes: Number(event.target.value) })}>{[15, 30, 45, 60, 90, 120].map((value) => <option key={value} value={value}>{value} minutes</option>)}</select></label>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Slot Increment</span><select value={preferences.slotIncrementMinutes} onChange={(event) => onChange({ ...preferences, slotIncrementMinutes: Number(event.target.value) })}>{[5, 10, 15, 30, 60].map((value) => <option key={value} value={value}>{value} minutes</option>)}</select></label>
    </section>
  );
}

function CalendarAccountsSettings({
  authStatus,
  accounts,
  calendars,
  calendarsError,
  onAdd,
  onReconnect,
  onRemove,
  onRemoveEverywhere,
  onSetSelection,
}: {
  authStatus: AuthStatus | null;
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  calendarsError: string | null;
  onAdd(): Promise<void>;
  onReconnect(email: string): Promise<void>;
  onRemove(email: string): Promise<void>;
  onRemoveEverywhere(email: string): Promise<void>;
  onSetSelection(accountId: string, calendarIds: string[]): Promise<void>;
}) {
  const { pending: busyEmail, error, runFor } = useSettingsOperation();
  const [confirmEverywhereEmail, setConfirmEverywhereEmail] = useState<string | null>(null);

  return (
    <section className="settings-section accounts-manager" aria-label="Calendar Accounts">
      <div className="accounts-manager-header">
        <div>
          <h3>Google Calendar</h3>
          <p>
            Calendar access is connected separately from mail and can create events.
            Each account gets its own Calendar consent and keychain credential.
          </p>
        </div>
        <button
          type="button"
          className="primary-action settings-add-account"
          disabled={busyEmail !== null}
          onClick={() => runFor("__add__", onAdd)}
        >
          <Plus size={15} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Connect Calendar"}
        </button>
      </div>
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="notice accounts-config-notice">
          <AlertCircle size={16} />
          <div>
            <strong>Google OAuth is not configured</strong>
            <p>Configure the Google Desktop app credentials used for mail, then restart ThreeStrands.</p>
          </div>
        </div>
      ) : null}
      {accounts.length === 0 ? (
        <div className="accounts-empty">
          <span className="accounts-empty-icon"><CalendarDays size={18} /></span>
          <strong>No calendars connected</strong>
          <p>Connect Google Calendar to use the T shortcut and see your live schedule.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {accounts.map((account) => (
            <li className="account-card" key={account.email}>
              <div className="account-card-row">
                <span className="account-card-avatar calendar-account-avatar" aria-hidden="true">
                  <CalendarDays size={18} />
                </span>
                <div className="account-card-identity">
                  <div className="account-card-heading">
                    <strong>{account.email}</strong>
                    <span className={`account-status ${account.status}`}>
                      {account.status === "needs_reauth" ? <AlertCircle size={13} /> : <CheckCircle2 size={13} />}
                      {account.status === "needs_reauth" ? "Needs reconnect" : "Connected"}
                    </span>
                  </div>
                  <span className="account-card-email">Calendar events and availability</span>
                </div>
                <span className="accounts-list-actions">
                  {account.status === "needs_reauth" ? (
                    <button
                      type="button"
                      className="account-action-button"
                      disabled={busyEmail !== null}
                      onClick={() => runFor(account.email, () => onReconnect(account.email))}
                    >
                      Reconnect
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyEmail !== null}
                    onClick={() => runFor(account.email, () => onRemove(account.email))}
                  >
                    Disconnect
                  </button>
                  <button
                    type="button"
                    className="account-action-button"
                    disabled={busyEmail !== null}
                    aria-expanded={confirmEverywhereEmail === account.email}
                    onClick={() => setConfirmEverywhereEmail(account.email)}
                  >
                    More…
                  </button>
                </span>
              </div>
              {confirmEverywhereEmail === account.email ? (
                <InlineConfirm
                  ariaLabel="Remove calendar account from all devices confirmation"
                  cancelLabel="Cancel"
                  onCancel={() => setConfirmEverywhereEmail(null)}
                  disabled={busyEmail !== null}
                  actions={[{ label: "Remove on all devices", className: "danger-action", onClick: () => {
                    runFor(account.email, () => onRemoveEverywhere(account.email));
                    setConfirmEverywhereEmail(null);
                  } }]}
                >
                  <strong>Remove from every device?</strong><br />This disconnects {account.email} everywhere. Google Calendar itself is not changed.
                </InlineConfirm>
              ) : null}
              {account.status === "connected" ? (
                <fieldset className="calendar-picker">
                  <legend>Calendars shown in the sidebar</legend>
                  {calendars.filter((calendar) => calendar.accountId === account.email).length === 0 ? (
                    <p>Loading calendars…</p>
                  ) : calendars
                    .filter((calendar) => calendar.accountId === account.email)
                    .map((calendar) => (
                      <label key={calendar.id}>
                        <input
                          type="checkbox"
                          checked={calendar.selected}
                          disabled={busyEmail !== null}
                          onChange={(event) => {
                            const selected = calendars
                              .filter((candidate) =>
                                candidate.accountId === account.email
                                && candidate.selected
                                && candidate.id !== calendar.id
                              )
                              .map((candidate) => candidate.id);
                            if (event.target.checked) selected.push(calendar.id);
                            runFor(account.email, () => onSetSelection(account.email, selected));
                          }}
                        />
                        <span>{calendar.name}{calendar.primary ? " (Primary)" : ""}</span>
                      </label>
                    ))}
                </fieldset>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      {calendarsError ? <p className="form-error" role="alert">{calendarsError}</p> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

/** Resolves a label id to its display name by scanning every account's label
 * catalog rather than storing the name on the split inbox — ids are stable,
 * but the label could be renamed after the split inbox is created. */
function describeSplitInboxRule(splitInbox: SplitInbox, labelsByAccount: Record<string, Label[]>): string {
  switch (splitInbox.matchKind) {
    case "domain":
      return `Sending domain: ${splitInbox.matchValue}`;
    case "pattern":
      return `Address contains: ${splitInbox.matchValue}`;
    case "label": {
      // Gmail label ids are opaque per account, so only the split's own
      // account's catalog can resolve this id to a real name.
      const label = (labelsByAccount[splitInbox.accountId] ?? []).find(
        (candidate) => candidate.id === splitInbox.matchValue,
      );
      return `Label: ${label ? formatLabelName(label) : splitInbox.matchValue}`;
    }
  }
}

function SplitInboxNameInput({
  splitInbox,
  onCommit,
}: {
  splitInbox: SplitInbox;
  onCommit(name: string): void;
}) {
  const [value, setValue] = useState(splitInbox.name);
  const normalized = value.trim();

  useEffect(() => setValue(splitInbox.name), [splitInbox.name]);

  return (
    <form
      className="account-sender-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (!normalized || normalized === splitInbox.name) return;
        onCommit(normalized);
      }}
    >
      <input
        aria-label={`Name for ${splitInbox.name}`}
        value={value}
        maxLength={200}
        onChange={(event) => setValue(event.target.value)}
      />
      <button type="submit" disabled={!normalized || normalized === splitInbox.name}>
        Save Name
      </button>
    </form>
  );
}

export function SplitInboxesSettings({
  splitInboxes,
  accounts,
  activeAccountId,
  labelsByAccount,
  onCreate,
  onRename,
  onDelete,
  onReorder,
}: {
  splitInboxes: SplitInbox[];
  accounts: Account[];
  activeAccountId: string | null;
  labelsByAccount: Record<string, Label[]>;
  onCreate(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<void>;
  onRename(id: string, name: string): Promise<void>;
  onDelete(id: string): Promise<void>;
  onReorder(ids: string[]): Promise<void>;
}) {
  const [name, setName] = useState("");
  const [matchKind, setMatchKind] = useState<SplitInboxMatchKind>("domain");
  const [matchValue, setMatchValue] = useState("");
  const [accountId, setAccountId] = useState(() => activeAccountId ?? accounts[0]?.email ?? "");
  const [creating, setCreating] = useState(false);
  const { pending: busyId, error, setError, runFor } = useSettingsOperation();

  // A split inbox belongs to one account, so only that account's labels are
  // valid matches for it.
  const labelOptions = (labelsByAccount[accountId] ?? [])
    .filter((label) => label.kind === "user")
    .sort((a, b) => formatLabelName(a).localeCompare(formatLabelName(b), undefined, { sensitivity: "base" }));


  const move = (index: number, direction: -1 | 1) => {
    const next = moveItem(splitInboxes, index, direction);
    if (next) runFor(splitInboxes[index]!.id, () => onReorder(next.map((splitInbox) => splitInbox.id)));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Split Inboxes">
      <div className="accounts-manager-header">
        <div>
          <h3>Split Inboxes</h3>
          <p>
            Show a filtered slice of your inbox as its own view in the sidebar —
            for example, every message from one client's sending domain.
          </p>
        </div>
      </div>
      <form
        className="split-inbox-add"
        onSubmit={(event) => {
          event.preventDefault();
          const normalizedName = name.trim();
          const normalizedValue = matchValue.trim();
          if (!normalizedName || !normalizedValue || !accountId || creating) return;
          setCreating(true);
          setError(null);
          void onCreate(normalizedName, matchKind, normalizedValue, accountId)
            .then(() => {
              setName("");
              setMatchValue("");
            })
            .catch((reason: unknown) => setError(errorMessage(reason)))
            .finally(() => setCreating(false));
        }}
      >
        <input
          value={name}
          placeholder="Name (e.g. Acme Corp)"
          aria-label="Split Inbox Name"
          onChange={(event) => setName(event.target.value)}
        />
        <select
          value={accountId}
          aria-label="Account"
          onChange={(event) => {
            setAccountId(event.target.value);
            setMatchValue("");
          }}
        >
          {accounts.map((account) => (
            <option key={account.email} value={account.email}>{account.email}</option>
          ))}
        </select>
        <select
          value={matchKind}
          aria-label="Match By"
          onChange={(event) => {
            setMatchKind(event.target.value as SplitInboxMatchKind);
            setMatchValue("");
          }}
        >
          <option value="domain">Sending domain</option>
          <option value="label">Label</option>
          <option value="pattern">Address contains</option>
        </select>
        {matchKind === "label" ? (
          <select value={matchValue} aria-label="Label" onChange={(event) => setMatchValue(event.target.value)}>
            <option value="" disabled>Choose a label</option>
            {labelOptions.map((label) => (
              <option key={label.id} value={label.id}>{formatLabelName(label)}</option>
            ))}
          </select>
        ) : (
          <input
            value={matchValue}
            placeholder={matchKind === "domain" ? "acme.com" : "boss@"}
            aria-label={matchKind === "domain" ? "Sending Domain" : "Address Pattern"}
            onChange={(event) => setMatchValue(event.target.value)}
          />
        )}
        <button type="submit" className="primary-action" disabled={creating || !name.trim() || !matchValue.trim() || !accountId}>
          <Plus size={15} />
          {creating ? "Adding…" : "Add Split Inbox"}
        </button>
      </form>
      {splitInboxes.length === 0 ? (
        <div className="accounts-empty">
          <strong>No split inboxes yet</strong>
          <p>Add one above to see a filtered slice of your inbox in the sidebar.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {splitInboxes.map((splitInbox, index) => (
            <li className="account-card" key={splitInbox.id}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <SplitInboxNameInput
                    splitInbox={splitInbox}
                    onCommit={(nextName) => runFor(splitInbox.id, () => onRename(splitInbox.id, nextName))}
                  />
                  <span className="account-card-email">
                    {describeSplitInboxRule(splitInbox, labelsByAccount)} — {splitInbox.accountId}
                  </span>
                </div>
              </div>
              <div className="account-card-controls">
                <span className="accounts-list-actions">
                  <span className="account-reorder">
                    <button
                      type="button"
                      aria-label={`Move ${splitInbox.name} up`}
                      disabled={index === 0 || busyId !== null}
                      onClick={() => move(index, -1)}
                    >
                      <ChevronUp size={14} />
                    </button>
                    <button
                      type="button"
                      aria-label={`Move ${splitInbox.name} down`}
                      disabled={index === splitInboxes.length - 1 || busyId !== null}
                      onClick={() => move(index, 1)}
                    >
                      <ChevronDown size={14} />
                    </button>
                  </span>
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyId !== null}
                    onClick={() => runFor(splitInbox.id, () => onDelete(splitInbox.id))}
                  >
                    Delete
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

export function SnippetsSettings({
  snippets,
  onCreate,
  onUpdate,
  onDelete,
}: {
  snippets: Snippet[];
  onCreate(name: string, body: string): Promise<Snippet>;
  onUpdate(id: string, name: string, body: string): Promise<Snippet>;
  onDelete(id: string): Promise<void>;
}) {
  const [editorTarget, setEditorTarget] = useState<Snippet | "new" | null>(null);
  const { pending: busyId, error, runFor } = useSettingsOperation();

  const orderedSnippets = [...snippets].sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
  );

  return (
    <section className="settings-section accounts-manager" aria-label="Snippets">
      <div className="accounts-manager-header">
        <div>
          <h3>Snippets</h3>
          <p>
            Canned text you can insert into a reply with <kbd>⌘/Ctrl ;</kbd>. Use{" "}
            <code>{"{first_name}"}</code> to insert the recipient's first name.
          </p>
        </div>
        <button type="button" className="primary-action" onClick={() => setEditorTarget("new")}>
          <Plus size={15} /> Add Snippet
        </button>
      </div>
      {orderedSnippets.length === 0 ? (
        <div className="accounts-empty">
          <strong>No snippets yet</strong>
          <p>Add one above, or create one from the snippet picker (<kbd>⌘/Ctrl ;</kbd>) while composing.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {orderedSnippets.map((snippet) => (
            <li className="account-card" key={snippet.id}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <strong>{snippet.name}</strong>
                  <span className="account-card-email">{snippetBodyPreview(snippet.body)}</span>
                </div>
              </div>
              <div className="account-card-controls">
                <span className="accounts-list-actions">
                  <button type="button" className="account-action-button" onClick={() => setEditorTarget(snippet)}>
                    Edit
                  </button>
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyId !== null}
                    onClick={() => runFor(snippet.id, () => onDelete(snippet.id))}
                  >
                    Delete
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {editorTarget ? (
        <SnippetEditor
          target={editorTarget}
          initialName={editorTarget === "new" ? "" : editorTarget.name}
          backLabel="Cancel"
          onClose={() => setEditorTarget(null)}
          onBack={() => setEditorTarget(null)}
          onCreate={async (name, body) => { await onCreate(name, body); setEditorTarget(null); }}
          onUpdate={async (id, name, body) => { await onUpdate(id, name, body); setEditorTarget(null); }}
        />
      ) : null}
    </section>
  );
}

function AccountSenderNameInput({
  email,
  name,
  onCommit,
}: {
  email: string;
  name: string | null;
  onCommit(name: string | null): Promise<void>;
}) {
  const [value, setValue] = useState(name ?? "");
  const [saving, setSaving] = useState(false);
  const normalized = value.trim();
  const saved = name?.trim() ?? "";

  useEffect(() => setValue(name ?? ""), [name]);

  return (
    <form
      className="account-sender-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (saving || normalized === saved) return;
        setSaving(true);
        void onCommit(normalized || null).finally(() => setSaving(false));
      }}
    >
      <input
        aria-label={`Sender name for ${email}`}
        value={value}
        maxLength={200}
        placeholder="Sender name"
        disabled={saving}
        onChange={(event) => setValue(event.target.value)}
      />
      <button type="submit" disabled={saving || normalized === saved}>
        {saving ? "Saving…" : "Save Name"}
      </button>
    </form>
  );
}

/**
 * A color swatch that saves on its own debounced schedule instead of on
 * every drag tick. `<input type="color">` fires `onChange` continuously
 * while the native picker is open, not just once on commit — driving that
 * straight into a save-and-disable cycle (the previous implementation) could
 * disable the input mid-drag and drop the rest of the gesture, so only the
 * first flicker of color ever got saved. Local `value` gives smooth
 * dragging; `color` (the saved value) is only adopted once no locally
 * committed save is still in flight, so a slow save can't snap the swatch
 * back to a stale color out from under the user.
 */
function AccountColorInput({
  email,
  color,
  onCommit,
}: {
  email: string;
  color: string;
  onCommit(color: string): Promise<void>;
}) {
  const [value, setValue] = useState(color);
  const pendingCount = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (pendingCount.current === 0) setValue(color);
  }, [color]);

  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);

  return (
    <input
      type="color"
      aria-label={`Color for ${email}`}
      value={value}
      onChange={(event) => {
        const next = event.target.value;
        setValue(next);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => {
          pendingCount.current++;
          onCommit(next)
            .catch(logBackgroundFailure("Account color save"))
            .finally(() => { pendingCount.current--; });
        }, 200);
      }}
    />
  );
}

export function AiProviderSettings({ onChange }: { onChange?: () => void }) {
  const [provider, setProvider] = useState(readAiProvider);
  const [model, setModel] = useState(readAiModel);
  const [endpoint, setEndpoint] = useState(readAiEndpoint);
  const [features, setFeatures] = useState<AiFeatureFlags>(readAiFeatures);
  const featuresRef = useRef(features);
  const [keyConfigured, setKeyConfigured] = useState(false);
  const [keyInput, setKeyInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [configurationError, setConfigurationError] = useState<string | null>(null);
  const [testingConnection, setTestingConnection] = useState(false);
  const [connectionTested, setConnectionTested] = useState(false);

  useEffect(() => {
    void isAiApiKeyConfigured().then(setKeyConfigured);
  }, []);

  const updateFeature = (flag: keyof AiFeatureFlags, value: boolean) => {
    const next = { ...featuresRef.current, [flag]: value };
    featuresRef.current = next;
    saveAiFeatures(next);
    setFeatures(next);
    onChange?.();
  };

  return (
    <section className="settings-section" aria-label="AI provider">
      <p className="settings-hint">
        Disabled by default. ThreeStrands only sends thread content to your chosen
        provider for the features you turn on below, using your own API key.
      </p>

      <label className="settings-field">
        <span>Provider</span>
        <select
          value={provider}
          onChange={(event) => {
            const next = event.target.value as AiProvider;
            setProvider(next);
            setConnectionTested(false);
            saveAiProvider(next);
            onChange?.();
          }}
        >
          {AI_PROVIDER_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      </label>

      {provider !== "none" ? (
        <>
          <label className="settings-field">
            <span>Model</span>
            <input
              value={model}
              placeholder={AI_MODEL_PLACEHOLDERS[provider]}
              onChange={(event) => {
                setModel(event.target.value);
                setConnectionTested(false);
                saveAiModel(event.target.value);
              }}
            />
          </label>

          {AI_MODEL_SUGGESTIONS[provider].length > 0 ? (
            <div className="model-suggestions" aria-label="Suggested models">
              <span className="settings-hint">Suggestions</span>
              <div>
                {AI_MODEL_SUGGESTIONS[provider].map((suggestion) => (
                  <button
                    key={suggestion}
                    type="button"
                    className={model.trim() === suggestion ? "selected" : undefined}
                    onClick={() => {
                      setModel(suggestion);
                      setConnectionTested(false);
                      saveAiModel(suggestion);
                    }}
                  >
                    {suggestion}
                  </button>
                ))}
              </div>
            </div>
          ) : null}

          {provider === "custom" ? (
            <label className="settings-field">
              <span>Endpoint URL</span>
              <input
                value={endpoint}
                placeholder="https://api.example.com/v1"
                onChange={(event) => {
                  setEndpoint(event.target.value);
                  setConnectionTested(false);
                  saveAiEndpoint(event.target.value);
                }}
              />
            </label>
          ) : null}

          <label className="settings-field">
            <span>API Key</span>
            <input
              type="password"
              value={keyInput}
              placeholder={keyConfigured ? "Saved to keychain" : "Paste API key"}
              onChange={(event) => setKeyInput(event.target.value)}
            />
          </label>
          <span className={`settings-connection-status${keyConfigured ? " configured" : ""}`} role="status">
            {keyConfigured ? <CheckCircle2 size={13} aria-hidden="true" /> : <AlertCircle size={13} aria-hidden="true" />}
            {keyConfigured ? "API key configured" : "API key required"}
          </span>
          <div className="settings-row">
            <button
              disabled={busy || !keyInput.trim()}
              onClick={() => {
                setBusy(true);
                setConfigurationError(null);
                void setAiApiKey(keyInput)
                  .then(() => {
                    setKeyInput("");
                    return isAiApiKeyConfigured();
                  })
                  .then(setKeyConfigured)
                  .then(() => onChange?.())
                  .catch((reason: unknown) => setConfigurationError(errorMessage(reason)))
                  .finally(() => setBusy(false));
              }}
            >
              Save Key
            </button>
            <button
              disabled={busy || !keyConfigured}
              onClick={() => {
                setBusy(true);
                setConfigurationError(null);
                void clearAiApiKey()
                  .then(() => isAiApiKeyConfigured())
                  .then(setKeyConfigured)
                  .then(() => onChange?.())
                  .catch((reason: unknown) => setConfigurationError(errorMessage(reason)))
                  .finally(() => setBusy(false));
              }}
            >
              Remove Key
            </button>
          </div>
          <div className="settings-row ai-connection-actions">
            <button
              className="ai-connection-test"
              type="button"
              disabled={busy || testingConnection || !keyConfigured || !resolveAiModel(provider, model) || (provider === "custom" && !endpoint.trim())}
              onClick={() => {
                setTestingConnection(true);
                setConfigurationError(null);
                setConnectionTested(false);
                void testAiConnection(provider, resolveAiModel(provider, model), endpoint)
                  .then(() => setConnectionTested(true))
                  .catch((reason: unknown) => setConfigurationError(errorMessage(reason)))
                  .finally(() => setTestingConnection(false));
              }}
            >
              <RefreshCw size={14} aria-hidden="true" />
              {testingConnection ? "Testing connection…" : "Test Connection"}
            </button>
            {connectionTested ? <span className="settings-connection-status configured" role="status"><CheckCircle2 size={13} aria-hidden="true" /> Connection successful</span> : null}
          </div>
          <span className="settings-hint">
            Stored in your OS keychain, never in the mail database.
          </span>
          {configurationError ? <p className="form-error" role="alert">{configurationError}</p> : null}

          <h3>Features</h3>
          <label className="settings-switch">
            <span>Draft Assist</span>
            <input
              type="checkbox"
              checked={features.draftAssist}
              onChange={(event) => updateFeature("draftAssist", event.target.checked)}
            />
          </label>
          <label className="settings-switch">
            <span>Thread Summaries</span>
            <input
              type="checkbox"
              checked={features.summarize}
              onChange={(event) => updateFeature("summarize", event.target.checked)}
            />
          </label>
          <label className="settings-switch">
            <span>Suggestions</span>
            <input
              type="checkbox"
              checked={features.actionExtraction}
              onChange={(event) => updateFeature("actionExtraction", event.target.checked)}
            />
          </label>
          <label className="settings-switch">
            <span>Thread Chat</span>
            <input
              type="checkbox"
              checked={features.threadChat}
              onChange={(event) => updateFeature("threadChat", event.target.checked)}
            />
          </label>
          <p className="settings-hint">Thread chat answers questions about the open conversation. Press q or ⌘J to ask; Escape returns to shortcuts. It shares other emails only for a question where you choose Search all mail.</p>
          <label className="settings-switch">
            <span>Proactive Suggestions</span>
            <input
              type="checkbox"
              checked={features.proactiveBriefs}
              disabled={!features.summarize && !features.actionExtraction}
              onChange={(event) => updateFeature("proactiveBriefs", event.target.checked)}
            />
          </label>
          <label className="settings-switch">
            <span>Only for People I&rsquo;ve Emailed</span>
            <input
              type="checkbox"
              checked={features.proactiveKnownSendersOnly}
              disabled={!features.proactiveBriefs || (!features.summarize && !features.actionExtraction)}
              onChange={(event) => updateFeature("proactiveKnownSendersOnly", event.target.checked)}
            />
          </label>
          <p className="settings-hint">Proactive suggestions prepare the brief once you stay on a conversation for the mark-read delay, at least {MIN_PROACTIVE_DWELL_SECONDS} seconds. They skip mailing lists and conversations with no one else in them, and read each conversation again only when a new message arrives.</p>
          <label className="settings-switch">
            <span>Contact Enrichment</span>
            <input type="checkbox" checked={features.contactEnrichment} onChange={(event) => updateFeature("contactEnrichment", event.target.checked)} />
          </label>
          <p className="settings-hint">Contact enrichment starts with three local emails. If they yield no supported suggestions, it checks up to nine more. You can choose to search more emails when the first three yield suggestions.</p>
          <AiUsageSummary provider={provider} model={resolveAiModel(provider, model)} />
        </>
      ) : null}
    </section>
  );
}

function PrivacySettings({
  loadRemoteImages,
  onLoadRemoteImagesChange,
}: {
  loadRemoteImages: boolean;
  onLoadRemoteImagesChange(value: boolean): void;
}) {
  const [retentionDays, setRetentionDaysState] = useState<number | null>(null);

  useEffect(() => {
    void getRetentionDays().then(setRetentionDaysState);
  }, []);

  return (
    <section className="settings-section" aria-label="Privacy">
      <h3>Local Storage</h3>
      <label className="settings-field">
        <span>Keep Mail on This Device For</span>
        <select
          value={retentionDays === null ? "forever" : String(retentionDays)}
          onChange={(event) => {
            const next = event.target.value === "forever" ? null : Number(event.target.value);
            setRetentionDaysState(next);
            void setRetentionDays(next);
          }}
        >
          {RETENTION_OPTIONS.map((option) => (
            <option key={option.label} value={option.value === null ? "forever" : String(option.value)}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
      <span className="settings-hint">
        Mail older than this is removed from ThreeStrands's local cache to keep the
        database from growing without bound. It stays on the server.
      </span>

      <h3>Message Images</h3>
      <label className="settings-switch">
        <span>Load Remote Images Automatically</span>
        <input
          type="checkbox"
          checked={loadRemoteImages}
          onChange={(event) => onLoadRemoteImagesChange(event.target.checked)}
        />
      </label>
      <span className="settings-hint">
        When disabled, images stay blocked until you choose Load images in a message.
      </span>

    </section>
  );
}

function DataTransferSettings({
  onImported,
}: {
  onImported(result: SettingsImportResult): Promise<void>;
}) {
  const [exportPassword, setExportPassword] = useState("");
  const [exportConfirmation, setExportConfirmation] = useState("");
  const [importPassword, setImportPassword] = useState("");
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const isDesktop = "__TAURI_INTERNALS__" in window;
  const passwordsMatch = exportPassword.length >= 8 && exportPassword === exportConfirmation;

  const showError = (error: unknown) => {
    setMessage(errorMessage(error));
  };

  return (
    <section className="settings-section" aria-label="Data transfer">
      <h3>Export Settings and Accounts</h3>
      <p className="settings-hint">
        Creates a password-encrypted file containing your preferences, account
        list, Split Inboxes, and retention setting. Mail, OAuth credentials,
        API keys, and other keychain secrets are never exported.
      </p>
      <label className="settings-field">
        <span>Export Password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={exportPassword}
          onChange={(event) => setExportPassword(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <label className="settings-field">
        <span>Confirm Password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={exportConfirmation}
          onChange={(event) => setExportConfirmation(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <button
        type="button"
        className="settings-transfer-action"
        disabled={!isDesktop || !passwordsMatch || busy !== null}
        onClick={() => {
          setBusy("export");
          setMessage(null);
          void exportSettings(exportPassword)
            .then((path) => {
              if (path) {
                setMessage(`Settings exported to ${path}`);
                setExportPassword("");
                setExportConfirmation("");
              }
            })
            .catch(showError)
            .finally(() => setBusy(null));
        }}
      >
        <Download size={15} aria-hidden="true" />
        {busy === "export" ? "Exporting…" : "Export Encrypted Settings"}
      </button>

      <h3>Import Settings and Accounts</h3>
      <p className="settings-hint">
        Importing replaces preferences and Split Inboxes from this installation.
        Existing connected accounts stay connected. Other imported accounts
        appear as “Connect on this device” and require Google authorization.
      </p>
      <label className="settings-field">
        <span>Backup File Password</span>
        <input
          type="password"
          autoComplete="current-password"
          aria-label="Backup File Password"
          value={importPassword}
          onChange={(event) => setImportPassword(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <button
        type="button"
        className="settings-transfer-action"
        disabled={!isDesktop || importPassword.length < 8 || busy !== null}
        onClick={() => {
          setBusy("import");
          setMessage(null);
          void importSettings(importPassword)
            .then(async (result) => {
              if (!result) return;
              await onImported(result);
            })
            .catch(showError)
            .finally(() => setBusy(null));
        }}
      >
        <Upload size={15} aria-hidden="true" />
        {busy === "import" ? "Importing…" : "Choose Encrypted Settings File"}
      </button>
      {!isDesktop ? (
        <p className="settings-hint" role="status">
          Settings transfer is available in the ThreeStrands desktop app.
        </p>
      ) : null}
      {message ? <p className="settings-hint" role="status">{message}</p> : null}
    </section>
  );
}
