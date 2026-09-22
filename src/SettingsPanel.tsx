import {
  AlertCircle,
  CalendarDays,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Download,
  Mail,
  Plus,
  RefreshCw,
  Search,
  Upload,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { listen } from "@tauri-apps/api/event";
import {
  clearLocalCrashReports,
  crashReportingEnabled,
  localCrashReports,
  setCrashReportingEnabled,
} from "./crashReporting";
import { formatLabelName } from "./labels";
import { Modal } from "./AppChrome";
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
import {
  cloudAccountStatus,
  cloudConflicts,
  cloudDeleteAccount,
  cloudDevices,
  cloudRevokeDevice,
  cloudSignIn,
  cloudSignOut,
  confirmCloudEnrollment,
  queuePortablePreferences,
  resolveCloudConflict,
  retryCloudSync,
  type CloudAccountStatus,
  type CloudConflict,
  type CloudDevice,
} from "./cloudAccount";
import {
  replicatedSyncAddFolder,
  replicatedSyncAddIpfsRpc,
  replicatedSyncApproveRequest,
  replicatedSyncBeginGenesis,
  replicatedSyncBetaEnabled,
  replicatedSyncConfirmEnrollment,
  replicatedSyncConflicts,
  replicatedSyncDeviceRoster,
  replicatedSyncEnabled,
  replicatedSyncEnrollmentStatus,
  replicatedSyncJoinWithRecoveryPhrase,
  replicatedSyncNow,
  replicatedSyncPendingRequests,
  replicatedSyncProbeIpfsRpc,
  replicatedSyncRejectRequest,
  replicatedSyncRemoveTransport,
  replicatedSyncResolveConflict,
  replicatedSyncRequestEnrollment,
  replicatedSyncRotateEpoch,
  replicatedSyncSetBetaEnabled,
  replicatedSyncStatus,
  type DeviceRosterEntry,
  type EnrollmentStatus,
  type FrontierConflict,
  type IncomingEnrollmentRequest,
  type IpfsRpcProbeReport,
  type ReplicatedSyncTransportStatus,
} from "./replicatedSync";
import { FrontierConflictEditor } from "./FrontierConflictEditor";

export type SettingsSection = "cloudAccount" | "replicatedSync" | "appearance" | "reading" | "accounts" | "calendarAccounts" | "availability" | "splitInboxes" | "snippets" | "ai" | "privacy" | "diagnostics" | "data";

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
          <dd className="recovery-notice">{recoveryStatusMessage(recovery)}</dd>
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
};

const SETTINGS_GROUPS: SettingsGroup[] = ["General", "Accounts", "Workflow", "Integrations", "System"];

const SETTINGS_SECTIONS: SettingsSectionDefinition[] = [
  { id: "appearance", label: "Appearance", group: "General", description: "Choose how ThreeStrands looks and reads.", keywords: "theme light dark font size family" },
  { id: "reading", label: "Reading", group: "General", description: "Control what happens when you open a conversation.", keywords: "mark read delay conversation" },
  { id: "cloudAccount", label: "Three Strands Account", group: "Accounts", description: "Manage your profile, devices, and cross-device sync.", keywords: "cloud profile devices sign in sync" },
  { id: "accounts", label: "Mail Accounts", group: "Accounts", description: "Connect mail accounts and manage their identity and order.", keywords: "gmail sender name color reconnect disconnect" },
  { id: "calendarAccounts", label: "Calendar Accounts", group: "Accounts", description: "Connect calendars and choose which ones appear in the sidebar.", keywords: "google calendar connect selection" },
  { id: "availability", label: "Availability", group: "Workflow", description: "Set your timezone, working hours, and meeting defaults.", keywords: "timezone working hours duration slots meetings" },
  { id: "splitInboxes", label: "Split Inboxes", group: "Workflow", description: "Create focused inbox views for the messages that matter.", keywords: "filtered inbox domain label address pattern" },
  { id: "snippets", label: "Snippets", group: "Workflow", description: "Manage reusable text for faster replies.", keywords: "canned text reply templates compose" },
  { id: "ai", label: "AI Provider", group: "Integrations", description: "Connect an AI provider and choose which features may use it.", keywords: "api key model endpoint draft summary actions" },
  { id: "replicatedSync", label: "Replicated Sync (Beta)", group: "Integrations", description: "Configure end-to-end encrypted replication transports.", keywords: "folder ipfs rpc encrypted beta" },
  { id: "privacy", label: "Privacy", group: "System", description: "Control local retention and remote message content.", keywords: "storage retention remote images cache" },
  { id: "diagnostics", label: "Diagnostics", group: "System", description: "Inspect synchronization health and crash-reporting controls.", keywords: "sync status errors crash reports troubleshooting" },
  { id: "data", label: "Data Transfer", group: "System", description: "Move encrypted settings and account metadata between devices.", keywords: "import export backup password" },
];

export function Settings({
  section,
  onSectionChange,
  onClose,
  theme,
  onThemeChange,
  fontScale,
  onFontScaleChange,
  fontFamily,
  onFontFamilyChange,
  autoReadDelaySeconds,
  onAutoReadDelayChange,
  loadRemoteImages,
  onLoadRemoteImagesChange,
  availabilityPreferences,
  onAvailabilityPreferencesChange,
  syncStatus,
  recoveryStatus,
  onAiConfigChange,
  authStatus,
  accounts,
  calendarAccounts,
  calendarOptions,
  calendarOptionsError,
  activeAccountId,
  onAddAccount,
  onRemoveAccount,
  onRemoveAccountEverywhere,
  onReconnectAccount,
  onSetAccountDisplayName,
  onSetAccountColor,
  onReorderAccounts,
  onAddCalendarAccount,
  onReconnectCalendarAccount,
  onRemoveCalendarAccount,
  onRemoveCalendarAccountEverywhere,
  onSetCalendarSelection,
  onSettingsImported,
  splitInboxes,
  labelsByAccount,
  onCreateSplitInbox,
  onRenameSplitInbox,
  onDeleteSplitInbox,
  onReorderSplitInboxes,
  snippets,
  onCreateSnippet,
  onUpdateSnippet,
  onDeleteSnippet,
}: {
  section: SettingsSection;
  onSectionChange(section: SettingsSection): void;
  onClose(): void;
  theme: Theme;
  onThemeChange(theme: Theme): void;
  fontScale: number;
  onFontScaleChange(value: number): void;
  fontFamily: FontFamily;
  onFontFamilyChange(value: FontFamily): void;
  autoReadDelaySeconds: number;
  onAutoReadDelayChange(value: number): void;
  loadRemoteImages: boolean;
  onLoadRemoteImagesChange(value: boolean): void;
  availabilityPreferences: AvailabilityPreferences;
  onAvailabilityPreferencesChange(value: AvailabilityPreferences): void;
  syncStatus: SyncStatus | null;
  recoveryStatus: RecoveryStatus | null;
  onAiConfigChange(): void;
  authStatus: AuthStatus | null;
  accounts: Account[];
  calendarAccounts: CalendarAccount[];
  calendarOptions: CalendarOption[];
  calendarOptionsError: string | null;
  activeAccountId: string | null;
  onAddAccount(): Promise<void>;
  onRemoveAccount(email: string): Promise<void>;
  onRemoveAccountEverywhere(email: string): Promise<void>;
  onReconnectAccount(email: string): Promise<void>;
  onSetAccountDisplayName(email: string, displayName: string | null): Promise<void>;
  onSetAccountColor(email: string, color: string): Promise<void>;
  onReorderAccounts(emails: string[]): Promise<void>;
  onAddCalendarAccount(): Promise<void>;
  onReconnectCalendarAccount(email: string): Promise<void>;
  onRemoveCalendarAccount(email: string): Promise<void>;
  onRemoveCalendarAccountEverywhere(email: string): Promise<void>;
  onSetCalendarSelection(accountId: string, calendarIds: string[]): Promise<void>;
  onSettingsImported(result: SettingsImportResult): Promise<void>;
  splitInboxes: SplitInbox[];
  labelsByAccount: Record<string, Label[]>;
  onCreateSplitInbox(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<void>;
  onRenameSplitInbox(id: string, name: string): Promise<void>;
  onDeleteSplitInbox(id: string): Promise<void>;
  onReorderSplitInboxes(ids: string[]): Promise<void>;
  snippets: Snippet[];
  onCreateSnippet(name: string, body: string): Promise<Snippet>;
  onUpdateSnippet(id: string, name: string, body: string): Promise<Snippet>;
  onDeleteSnippet(id: string): Promise<void>;
}) {
  const [settingsQuery, setSettingsQuery] = useState("");
  const settingsPanelRef = useRef<HTMLDivElement>(null);
  const selectedSectionButtonRef = useRef<HTMLButtonElement>(null);
  // Replicated Sync's own section handles its "not enabled yet" state
  // itself (it shows the "enable beta features" toggle there) — the nav
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
            <div>
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
          {section === "cloudAccount" ? <CloudAccountSettings /> : null}
          {section === "replicatedSync" ? <ReplicatedSyncSettings /> : null}
          {section === "appearance" ? (
            <AppearanceSettings
              theme={theme}
              onThemeChange={onThemeChange}
              fontScale={fontScale}
              onFontScaleChange={onFontScaleChange}
              fontFamily={fontFamily}
              onFontFamilyChange={onFontFamilyChange}
            />
          ) : null}
          {section === "reading" ? (
            <ReadingSettings
              autoReadDelaySeconds={autoReadDelaySeconds}
              onAutoReadDelayChange={onAutoReadDelayChange}
            />
          ) : null}
          {section === "accounts" ? (
            <AccountsSettings
              authStatus={authStatus}
              accounts={accounts}
              onAdd={onAddAccount}
              onRemove={onRemoveAccount}
              onRemoveEverywhere={onRemoveAccountEverywhere}
              onReconnect={onReconnectAccount}
              onSetDisplayName={onSetAccountDisplayName}
              onSetColor={onSetAccountColor}
              onReorder={onReorderAccounts}
            />
          ) : null}
          {section === "calendarAccounts" ? (
            <CalendarAccountsSettings
              authStatus={authStatus}
              accounts={calendarAccounts}
              calendars={calendarOptions}
              calendarsError={calendarOptionsError}
              onAdd={onAddCalendarAccount}
              onReconnect={onReconnectCalendarAccount}
              onRemove={onRemoveCalendarAccount}
              onRemoveEverywhere={onRemoveCalendarAccountEverywhere}
              onSetSelection={onSetCalendarSelection}
            />
          ) : null}
          {section === "availability" ? (
            <AvailabilitySettings
              preferences={availabilityPreferences}
              onChange={onAvailabilityPreferencesChange}
            />
          ) : null}
          {section === "splitInboxes" ? (
            <SplitInboxesSettings
              splitInboxes={splitInboxes}
              accounts={accounts}
              activeAccountId={activeAccountId}
              labelsByAccount={labelsByAccount}
              onCreate={onCreateSplitInbox}
              onRename={onRenameSplitInbox}
              onDelete={onDeleteSplitInbox}
              onReorder={onReorderSplitInboxes}
            />
          ) : null}
          {section === "snippets" ? (
            <SnippetsSettings
              snippets={snippets}
              onCreate={onCreateSnippet}
              onUpdate={onUpdateSnippet}
              onDelete={onDeleteSnippet}
            />
          ) : null}
          {section === "ai" ? <AiProviderSettings onChange={() => { onAiConfigChange(); queuePortablePreferences(); }} /> : null}
          {section === "privacy" ? (
            <PrivacySettings
              loadRemoteImages={loadRemoteImages}
              onLoadRemoteImagesChange={onLoadRemoteImagesChange}
            />
          ) : null}
          {section === "diagnostics" ? (
            <DiagnosticsSettings status={syncStatus} recovery={recoveryStatus} accountCount={accounts.length} />
          ) : null}
          {section === "data" ? <DataTransferSettings onImported={onSettingsImported} /> : null}
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}

function CloudAccountSettings() {
  const [status, setStatus] = useState<CloudAccountStatus | null>(null);
  const [devices, setDevices] = useState<CloudDevice[]>([]);
  const [conflicts, setConflicts] = useState<CloudConflict[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [confirmDeleteAccount, setConfirmDeleteAccount] = useState(false);

  const refresh = useCallback(async () => {
    const next = await cloudAccountStatus();
    setStatus(next);
    if (next.signedIn) {
      const [nextDevices, nextConflicts] = await Promise.all([
        cloudDevices().catch(() => []),
        next.syncEntitled ? cloudConflicts().catch(() => []) : Promise.resolve([]),
      ]);
      setDevices(nextDevices);
      setConflicts(nextConflicts);
    } else {
      setDevices([]);
      setConflicts([]);
    }
  }, []);

  useEffect(() => {
    void refresh().catch((error: unknown) => setMessage(String(error)));
    let unlisten: (() => void) | undefined;
    if ("__TAURI_INTERNALS__" in window) {
      void listen("cloud-sync-status", () => void refresh()).then((stop) => { unlisten = stop; });
    }
    return () => unlisten?.();
  }, [refresh]);

  const act = (operation: () => Promise<unknown>) => {
    setBusy(true);
    setMessage(null);
    void operation()
      .then(refresh)
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  if (!status) return <section className="settings-section" aria-label="Three Strands Account"><p className="settings-hint">Loading account status…</p></section>;
  if (!status.configured) {
    return (
      <section className="settings-section" aria-label="Three Strands Account">
        <h3>Three Strands Account</h3>
        <p className="settings-hint">This build has no Three Strands service configured. Everything continues to work locally without an account.</p>
      </section>
    );
  }
  if (!status.signedIn) {
    return (
      <section className="settings-section" aria-label="Three Strands Account">
        <h3>Use Three Strands on Multiple Computers</h3>
        <p className="settings-hint">Sign in separately from your mail accounts. Mail, drafts, attachments, OAuth tokens, and AI keys are never uploaded.</p>
        <button type="button" className="primary-action" disabled={busy} onClick={() => act(cloudSignIn)}>{busy ? "Waiting for Google…" : "Sign in with Google"}</button>
        {message ? <p role="status" className="settings-hint">{message}</p> : null}
      </section>
    );
  }

  return (
    <section className="settings-section accounts-manager" aria-label="Three Strands Account">
      <div className="accounts-manager-header">
        <div><h3>{status.profile?.displayName ?? status.profile?.email}</h3><p className="settings-hint">{status.profile?.email}</p></div>
        <button type="button" className="account-action-button" disabled={busy} onClick={() => act(cloudSignOut)}>Sign out</button>
      </div>
      {!status.syncEntitled ? (
        <div className="accounts-config-notice"><strong>Sync is not enabled for this account</strong><p>Your account is ready, but it has not received a beta sync entitlement. Local features remain available.</p></div>
      ) : !status.enrollmentConfirmed ? (
        <div className="accounts-config-notice">
          <strong>Review what will be synchronized</strong>
          <p>Tasks (including subject snapshots and evidence), snippets, Split Inboxes, mail and calendar account metadata, calendar selections, retention, and portable preferences will be readable by the Three Strands service.</p>
          <p>Mail bodies, drafts, attachments, provider tokens, AI keys, crash reports, and device layout stay on this computer. Existing local collections are merged without deletion; cloud portable preferences win on an already-enrolled account.</p>
          <button type="button" className="primary-action" disabled={busy} onClick={() => act(confirmCloudEnrollment)}>Enable automatic sync</button>
        </div>
      ) : (
        <>
          <dl className="sync-details">
            <dt>Status</dt><dd>{status.error ? "Needs attention" : status.pendingOperations ? "Synchronizing" : "Up to date"}</dd>
            <dt>Last successful sync</dt><dd>{status.lastSuccessfulSync ? new Date(status.lastSuccessfulSync).toLocaleString() : "Not yet"}</dd>
            <dt>Pending changes</dt><dd>{status.pendingOperations}</dd>
            <dt>Conflicts</dt><dd>{status.conflictCount}</dd>
          </dl>
          {status.error ? <div className="accounts-config-notice"><strong>Synchronization failed</strong><p>{status.error}</p><button type="button" className="account-action-button" disabled={busy} onClick={() => act(retryCloudSync)}>Retry now</button></div> : null}
          {conflicts.length ? <div><h3>Resolve Conflicts</h3>{conflicts.map((conflict) => <CloudConflictEditor key={conflict.id} conflict={conflict} disabled={busy} onResolve={(payload, deleted) => act(() => resolveCloudConflict(conflict.id, conflict.currentVersion, payload, deleted))} />)}</div> : null}
          <h3>Signed-In Devices</h3>
          <ul className="accounts-list">{devices.map((device) => <li className="account-card" key={device.id}><div className="account-card-row"><div className="account-card-identity"><strong>{device.name}{device.current ? " · This device" : ""}</strong><span className="account-card-email">Last active {new Date(device.lastSeenAt).toLocaleString()}</span></div><button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => act(() => cloudRevokeDevice(device.id))}>{device.current ? "Sign out" : "Revoke"}</button></div></li>)}</ul>
        </>
      )}
      <h3>Delete Cloud Account</h3>
      <p className="settings-hint">Deletes synchronized cloud data and sessions. Local data remains on this computer; encrypted backups expire according to the service retention policy.</p>
      <button type="button" className="account-action-button danger-action" disabled={busy} aria-expanded={confirmDeleteAccount} onClick={() => setConfirmDeleteAccount(true)}>Delete cloud account…</button>
      {confirmDeleteAccount ? (
        <div className="settings-inline-confirm" role="group" aria-label="Delete cloud account confirmation">
          <p><strong>Delete synchronized cloud data and sessions?</strong><br />Local data on this computer remains. This cloud data cannot be recovered after backup retention expires.</p>
          <span className="settings-inline-confirm-actions">
            <button type="button" disabled={busy} onClick={() => setConfirmDeleteAccount(false)}>Cancel</button>
            <button type="button" className="danger-action" disabled={busy} onClick={() => { setConfirmDeleteAccount(false); act(cloudDeleteAccount); }}>Delete cloud account</button>
          </span>
        </div>
      ) : null}
      {message ? <p role="status" className="settings-hint">{message}</p> : null}
    </section>
  );
}

function formatStorageEstimate(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(1)} ${units[index]}`;
}

const FILEBASE_RPC_URL = "https://rpc.filebase.io";

function ReplicatedSyncSettings() {
  const [available, setAvailable] = useState<boolean | null>(null);
  const [betaEnabled, setBetaEnabledState] = useState(false);
  const [transports, setTransports] = useState<ReplicatedSyncTransportStatus[]>([]);
  const [conflicts, setConflicts] = useState<FrontierConflict[]>([]);
  const [enrollmentStatus, setEnrollmentStatus] = useState<EnrollmentStatus | null>(null);
  const [pendingRequests, setPendingRequests] = useState<IncomingEnrollmentRequest[]>([]);
  const [deviceRoster, setDeviceRoster] = useState<DeviceRosterEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [ipfsBaseUrl, setIpfsBaseUrl] = useState("");
  const [ipfsToken, setIpfsToken] = useState("");
  const [ipfsProbe, setIpfsProbe] = useState<IpfsRpcProbeReport | null>(null);
  const [disconnectingTransport, setDisconnectingTransport] = useState<string | null>(null);
  const [revokingDevice, setRevokingDevice] = useState<string | null>(null);
  const [recoveryPhrase, setRecoveryPhrase] = useState<string | null>(null);
  const [recoveryPhraseInput, setRecoveryPhraseInput] = useState("");

  const refresh = useCallback(async () => {
    const [enabled, beta] = await Promise.all([replicatedSyncEnabled(), replicatedSyncBetaEnabled()]);
    setAvailable(enabled);
    setBetaEnabledState(beta);
    if (enabled) {
      const [nextTransports, nextConflicts, nextStatus, nextPending, nextRoster] = await Promise.all([
        replicatedSyncStatus(),
        replicatedSyncConflicts(),
        replicatedSyncEnrollmentStatus(),
        replicatedSyncPendingRequests(),
        replicatedSyncDeviceRoster(),
      ]);
      setTransports(nextTransports);
      setConflicts(nextConflicts);
      setEnrollmentStatus(nextStatus);
      setPendingRequests(nextPending);
      setDeviceRoster(nextRoster);
    } else {
      setTransports([]);
      setConflicts([]);
      setEnrollmentStatus(null);
      setPendingRequests([]);
      setDeviceRoster([]);
    }
  }, []);

  useEffect(() => {
    void refresh().catch((error: unknown) => setMessage(String(error)));
    let unlisten: (() => void) | undefined;
    if ("__TAURI_INTERNALS__" in window) {
      void listen("replicated-sync-status", () => void refresh()).then((stop) => { unlisten = stop; });
    }
    return () => unlisten?.();
  }, [refresh]);

  const act = (operation: () => Promise<unknown>) => {
    setBusy(true);
    setMessage(null);
    void operation()
      .then(refresh)
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  const addFolder = () => {
    setBusy(true);
    setMessage(null);
    void replicatedSyncAddFolder()
      .then((status) => {
        if (!status) setMessage("No folder selected.");
        return refresh();
      })
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  const probeIpfsRpc = () => {
    setBusy(true);
    setMessage(null);
    setIpfsProbe(null);
    void replicatedSyncProbeIpfsRpc(ipfsBaseUrl, ipfsToken.trim() ? ipfsToken : null)
      .then((report) => {
        setIpfsProbe(report);
        if (!report.versionOk) setMessage("Could not reach an IPFS RPC endpoint at that URL.");
      })
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  const addIpfsRpc = () => {
    setBusy(true);
    setMessage(null);
    void replicatedSyncAddIpfsRpc(ipfsBaseUrl, ipfsToken.trim() ? ipfsToken : null)
      .then((status) => {
        if (!status) setMessage("Could not add that endpoint.");
        setIpfsBaseUrl("");
        setIpfsToken("");
        setIpfsProbe(null);
        return refresh();
      })
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  const toggleBeta = (on: boolean) => {
    setBusy(true);
    setMessage(null);
    void replicatedSyncSetBetaEnabled(on)
      .then(refresh)
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  const beginGenesis = () => {
    setBusy(true);
    setMessage(null);
    void replicatedSyncBeginGenesis()
      .then((phrase) => {
        setRecoveryPhrase(phrase);
        return refresh();
      })
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  const joinWithPhrase = () => {
    setBusy(true);
    setMessage(null);
    void replicatedSyncJoinWithRecoveryPhrase(recoveryPhraseInput.trim())
      .then(() => {
        setRecoveryPhraseInput("");
        return refresh();
      })
      .catch((error: unknown) => setMessage(String(error)))
      .finally(() => setBusy(false));
  };

  if (available === null) {
    return (
      <section className="settings-section" aria-label="Replicated Sync">
        <p className="settings-hint">Loading replicated sync status…</p>
      </section>
    );
  }

  if (!available) {
    return (
      <section className="settings-section" aria-label="Replicated Sync">
        <h3>Replicated Sync (Beta)</h3>
        <p className="settings-hint">
          An in-development, end-to-end encrypted alternative to the Three Strands Account sync above, with no
          Three Strands-operated server: it replicates directly through folders or an IPFS endpoint you choose.
          Turn it on to set it up on this device.
        </p>
        <label className="settings-field settings-field-inline">
          <input type="checkbox" checked={betaEnabled} disabled={busy} onChange={(event) => toggleBeta(event.target.checked)} />
          <span>Enable beta features</span>
        </label>
        {message ? <p role="status" className="settings-hint">{message}</p> : null}
      </section>
    );
  }

  return (
    <section className="settings-section" aria-label="Replicated Sync">
      <h3>Replicated Sync (Beta)</h3>
      <p className="settings-hint">
        Replicates tasks, snippets, Split Inboxes, and account metadata as end-to-end encrypted files through folders
        you choose. There is no Three Strands-operated sync server: ThreeStrands never sees the plaintext, but
        anyone with access to a selected folder can see the encrypted files themselves (their size and timing, not
        their contents). Deleting a folder here removes this device's copy of the synchronized data from that
        folder; it does not erase copies elsewhere (other devices, cloud provider version history, or other
        configured folders).
      </p>
      <label className="settings-field settings-field-inline">
        <input type="checkbox" checked={betaEnabled} disabled={busy} onChange={(event) => toggleBeta(event.target.checked)} />
        <span>Enable beta features</span>
      </label>

      {conflicts.length > 0 ? (
        <div>
          <h3>Resolve Conflicts</h3>
          {conflicts.map((conflict) => (
            <FrontierConflictEditor
              key={`${conflict.entityType}-${conflict.entityId}-${conflict.field}`}
              conflict={conflict}
              disabled={busy}
              onResolve={(chosen) => act(() => replicatedSyncResolveConflict(conflict, chosen))}
            />
          ))}
        </div>
      ) : null}

      <h3>Devices &amp; Enrollment</h3>
      {transports.length === 0 ? (
        <p className="settings-hint">Add a sync folder or IPFS RPC endpoint below first — enrollment needs somewhere to publish to.</p>
      ) : null}
      {recoveryPhrase ? (
        <div className="settings-field">
          <p className="settings-hint">
            <strong>Save this recovery phrase now — it is shown only this once and is never stored anywhere.</strong>{" "}
            It is the only way to recover this sync space if every other device is lost.
          </p>
          <p style={{ fontFamily: "monospace", userSelect: "text" }}>{recoveryPhrase}</p>
          <button type="button" className="account-action-button" onClick={() => setRecoveryPhrase(null)}>
            I&apos;ve saved it
          </button>
        </div>
      ) : null}
      {enrollmentStatus?.state === "notStarted" ? (
        <>
          <p className="settings-hint">Set this device up as the first device in a new encrypted sync space, or join a space that already exists on another device.</p>
          <button type="button" className="primary-action" disabled={busy || transports.length === 0} onClick={beginGenesis}>
            Create a new sync space
          </button>
          <button
            type="button"
            className="account-action-button"
            disabled={busy || transports.length === 0}
            onClick={() => act(replicatedSyncRequestEnrollment)}
          >
            Request to join from an existing device
          </button>
          <label className="settings-field">
            <span>Or join with a recovery phrase</span>
            <input
              type="text"
              placeholder="24 words separated by spaces"
              value={recoveryPhraseInput}
              disabled={busy}
              onChange={(event) => setRecoveryPhraseInput(event.target.value)}
            />
          </label>
          <button type="button" className="account-action-button" disabled={busy || !recoveryPhraseInput.trim() || transports.length === 0} onClick={joinWithPhrase}>
            Join with recovery phrase
          </button>
        </>
      ) : null}
      {enrollmentStatus?.state === "awaitingGrant" ? (
        <p className="settings-hint">
          Waiting for an existing device to approve this device. When it does, compare fingerprints on both screens
          before confirming. This device&apos;s fingerprint: <strong style={{ fontFamily: "monospace" }}>{enrollmentStatus.fingerprint}</strong>
        </p>
      ) : null}
      {enrollmentStatus?.state === "awaitingConfirmation" ? (
        <div className="settings-field">
          <p className="settings-hint">
            An approval arrived. Compare these fingerprints with what the approving device shows — they must match
            exactly before you confirm.
          </p>
          <p style={{ fontFamily: "monospace" }}>This device: {enrollmentStatus.fingerprint}</p>
          <p style={{ fontFamily: "monospace" }}>Approver: {enrollmentStatus.approverFingerprint}</p>
          <button
            type="button"
            className="primary-action"
            disabled={busy}
            onClick={() => {
              const requestId = enrollmentStatus.requestId;
              void act(() => replicatedSyncConfirmEnrollment(requestId));
            }}
          >
            Confirm — fingerprints match
          </button>
        </div>
      ) : null}
      {enrollmentStatus?.state === "enrolled" ? (
        <>
          <p className="settings-hint">Enrolled · {enrollmentStatus.deviceCount} active device{enrollmentStatus.deviceCount === 1 ? "" : "s"}.</p>
          {pendingRequests.length > 0 ? (
            <ul className="accounts-list">
              {pendingRequests.map((request) => (
                <li className="account-card" key={request.requestId}>
                  <div className="account-card-row">
                    <div className="account-card-identity">
                      <strong style={{ fontFamily: "monospace" }}>{request.fingerprint}</strong>
                      <span className="account-card-email">Requested {new Date(request.createdAt).toLocaleString()}</span>
                    </div>
                    <button type="button" className="primary-action" disabled={busy} onClick={() => act(() => replicatedSyncApproveRequest(request.requestId))}>
                      Approve
                    </button>
                    <button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => act(() => replicatedSyncRejectRequest(request.requestId))}>
                      Reject
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          ) : null}
          <ul className="accounts-list">
            {deviceRoster.map((device) => (
              <li className="account-card" key={device.deviceId}>
                <div className="account-card-row">
                  <div className="account-card-identity">
                    <strong style={{ fontFamily: "monospace" }}>{device.deviceId}</strong>
                    <span className="account-card-email">
                      {device.status}
                      {device.isSelf ? " · this device" : ""}
                    </span>
                  </div>
                  {!device.isSelf && device.status === "active" ? (
                    <button type="button" className="account-action-button danger-action" disabled={busy} aria-expanded={revokingDevice === device.deviceId} onClick={() => setRevokingDevice(device.deviceId)}>
                      Revoke…
                    </button>
                  ) : null}
                </div>
                {revokingDevice === device.deviceId ? (
                  <div className="settings-inline-confirm" role="group" aria-label="Revoke device confirmation">
                    <p><strong>Revoke this device?</strong><br />It keeps existing data, but future writes from it will no longer be trusted.</p>
                    <span className="settings-inline-confirm-actions">
                      <button type="button" disabled={busy} onClick={() => setRevokingDevice(null)}>Cancel</button>
                      <button type="button" className="danger-action" disabled={busy} onClick={() => { setRevokingDevice(null); act(() => replicatedSyncRotateEpoch(device.deviceId)); }}>Revoke device</button>
                    </span>
                  </div>
                ) : null}
              </li>
            ))}
          </ul>
        </>
      ) : null}

      {transports.length === 0 ? (
        <p className="settings-hint">No folders configured yet.</p>
      ) : (
        <ul className="accounts-list">
          {transports.map((transport) => (
            <li className="account-card" key={transport.instanceId}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <strong>{transport.location}</strong>
                  <span className="account-card-email">
                    {transport.kind === "ipfs_rpc" ? "IPFS RPC" : "Folder"}
                    {!transport.headDiscovery ? " · storage-only" : ""} · {transport.health} · {transport.pending} pending
                    {transport.failed ? `, ${transport.failed} failed` : ""}
                    {transport.storageBytes != null ? ` · ${formatStorageEstimate(transport.storageBytes)}` : ""}
                  </span>
                  {transport.lastSuccessAt ? (
                    <span className="account-card-email">Last synced {new Date(transport.lastSuccessAt).toLocaleString()}</span>
                  ) : null}
                  {transport.lastError ? <span className="account-card-email">{transport.lastError}</span> : null}
                </div>
                <button
                  type="button"
                  className="account-action-button danger-action"
                  disabled={busy}
                  aria-expanded={disconnectingTransport === transport.instanceId}
                  onClick={() => setDisconnectingTransport(transport.instanceId)}
                >
                  Disconnect…
                </button>
              </div>
              {disconnectingTransport === transport.instanceId ? (
                <div className="settings-inline-confirm" role="group" aria-label="Disconnect sync transport confirmation">
                  <p>
                    <strong>Stop syncing to this {transport.kind === "ipfs_rpc" ? "endpoint" : "folder"}?</strong><br />
                    {transport.kind === "ipfs_rpc"
                      ? "Pinned objects remain with the provider until you remove them there."
                      : "You can keep the encrypted files for another device or delete this device’s copy."}
                  </p>
                  <span className="settings-inline-confirm-actions">
                    <button type="button" disabled={busy} onClick={() => setDisconnectingTransport(null)}>Cancel</button>
                    <button type="button" disabled={busy} onClick={() => { setDisconnectingTransport(null); act(() => replicatedSyncRemoveTransport(transport.instanceId, false)); }}>Disconnect and keep data</button>
                    {transport.kind !== "ipfs_rpc" ? (
                      <button type="button" className="danger-action" disabled={busy} onClick={() => { setDisconnectingTransport(null); act(() => replicatedSyncRemoveTransport(transport.instanceId, true)); }}>Delete files and disconnect</button>
                    ) : null}
                  </span>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      )}

      <button type="button" className="primary-action" disabled={busy} onClick={addFolder}>
        Add a sync folder
      </button>
      <button
        type="button"
        className="account-action-button"
        disabled={busy || transports.length === 0}
        onClick={() => act(replicatedSyncNow)}
      >
        Sync now
      </button>

      <h3>Add an IPFS RPC endpoint</h3>
      <p className="settings-hint">
        Advanced: point at a Kubo-compatible RPC endpoint (for example a Filebase bucket, or a local Kubo daemon) to
        replicate through it instead of, or alongside, a folder. The access token, if any, is stored only in this
        device&apos;s OS keychain, never in a settings export.
      </p>
      <label className="settings-field">
        <span>RPC base URL</span>
        <input
          type="text"
          placeholder="https://rpc.filebase.io"
          value={ipfsBaseUrl}
          disabled={busy}
          onChange={(event) => {
            setIpfsBaseUrl(event.target.value);
            setIpfsProbe(null);
          }}
        />
      </label>
      <label className="settings-field">
        <span>Access token (optional)</span>
        <input
          type="password"
          value={ipfsToken}
          disabled={busy}
          onChange={(event) => {
            setIpfsToken(event.target.value);
            setIpfsProbe(null);
          }}
        />
      </label>
      <button
        type="button"
        className="account-action-button"
        disabled={busy}
        onClick={() => {
          setIpfsBaseUrl(FILEBASE_RPC_URL);
          setIpfsProbe(null);
        }}
      >
        Use Filebase preset
      </button>
      <button type="button" className="account-action-button" disabled={busy || !ipfsBaseUrl} onClick={probeIpfsRpc}>
        Test connection
      </button>
      {ipfsProbe ? (
        <p className="settings-hint">
          {ipfsProbe.versionOk
            ? `Reachable · ${ipfsProbe.mfsAvailable ? "supports discovery (MFS)" : "storage-only, no MFS discovery"}`
            : "Not reachable at that URL."}
        </p>
      ) : null}
      <button
        type="button"
        className="primary-action"
        disabled={busy || !ipfsBaseUrl}
        onClick={addIpfsRpc}
      >
        Add IPFS RPC endpoint
      </button>

      {message ? <p role="status" className="settings-hint">{message}</p> : null}
    </section>
  );
}

function CloudConflictEditor({ conflict, disabled, onResolve }: { conflict: CloudConflict; disabled: boolean; onResolve(payload: Record<string, unknown> | null, deleted: boolean): void }) {
  const fields = conflict.overlappingFields.includes("*") ? ["*"] : conflict.overlappingFields;
  const [choices, setChoices] = useState<Record<string, "cloud" | "device">>(() => Object.fromEntries(fields.map((field) => [field, "cloud"])));
  const resolve = () => {
    if (fields.includes("*")) {
      const useDevice = choices["*"] === "device";
      onResolve(useDevice ? conflict.devicePatch : conflict.cloudPayload, useDevice ? conflict.deviceDeleted : conflict.cloudDeleted);
      return;
    }
    const merged = { ...(conflict.cloudPayload ?? {}) };
    for (const field of fields) if (choices[field] === "device" && conflict.devicePatch && field in conflict.devicePatch) merged[field] = conflict.devicePatch[field];
    onResolve(merged, false);
  };
  return (
    <div className="accounts-config-notice">
      <strong>{conflict.entityType.replaceAll("_", " ")} conflict</strong>
      {fields.map((field) => <fieldset key={field} className="settings-field"><legend>{field === "*" ? "Deletion and edit overlap" : field}</legend><label><input type="radio" name={`${conflict.id}-${field}`} checked={choices[field] === "cloud"} onChange={() => setChoices((value) => ({ ...value, [field]: "cloud" }))} /> Cloud: {JSON.stringify(field === "*" ? conflict.cloudPayload : conflict.cloudPayload?.[field])}</label><label><input type="radio" name={`${conflict.id}-${field}`} checked={choices[field] === "device"} onChange={() => setChoices((value) => ({ ...value, [field]: "device" }))} /> This device: {conflict.deviceDeleted ? "Delete" : JSON.stringify(field === "*" ? conflict.devicePatch : conflict.devicePatch?.[field])}</label></fieldset>)}
      <button type="button" className="primary-action" disabled={disabled} onClick={resolve}>Resolve conflict</button>
    </div>
  );
}

function AppearanceSettings({
  theme,
  onThemeChange,
  fontScale,
  onFontScaleChange,
  fontFamily,
  onFontFamilyChange,
}: {
  theme: Theme;
  onThemeChange(theme: Theme): void;
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
  const [busyEmail, setBusyEmail] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmEverywhereEmail, setConfirmEverywhereEmail] = useState<string | null>(null);

  const act = (busyKey: string, operation: () => Promise<void>) => {
    setBusyEmail(busyKey);
    setError(null);
    void operation()
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setBusyEmail(null));
  };

  const move = (index: number, direction: -1 | 1) => {
    const target = index + direction;
    if (target < 0 || target >= accounts.length) return;
    const next = [...accounts];
    [next[index], next[target]] = [next[target]!, next[index]!];
    act(accounts[index]!.email, () => onReorder(next.map((account) => account.email)));
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
          onClick={() => act("__add__", onAdd)}
        >
          <Plus size={15} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Add Account"}
        </button>
      </div>
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="accounts-config-notice">
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
                      setError(reason instanceof Error ? reason.message : String(reason));
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
                          setError(reason instanceof Error ? reason.message : String(reason));
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
                      onClick={() => act(account.email, () => onReconnect(account.email))}
                    >
                      Reconnect
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyEmail !== null}
                    onClick={() => act(account.email, () => onRemove(account.email))}
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
                <div className="settings-inline-confirm" role="group" aria-label="Remove mail account from all devices confirmation">
                  <p><strong>Remove from every device?</strong><br />This disconnects {account.email} everywhere. Gmail itself is not changed.</p>
                  <span className="settings-inline-confirm-actions">
                    <button type="button" disabled={busyEmail !== null} onClick={() => setConfirmEverywhereEmail(null)}>Cancel</button>
                    <button
                      type="button"
                      className="danger-action"
                      disabled={busyEmail !== null}
                      onClick={() => {
                        act(account.email, () => onRemoveEverywhere(account.email));
                        setConfirmEverywhereEmail(null);
                      }}
                    >
                      Remove on all devices
                    </button>
                  </span>
                </div>
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
  const [busyEmail, setBusyEmail] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmEverywhereEmail, setConfirmEverywhereEmail] = useState<string | null>(null);
  const act = (busyKey: string, operation: () => Promise<void>) => {
    setBusyEmail(busyKey);
    setError(null);
    void operation()
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setBusyEmail(null));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Calendar Accounts">
      <div className="accounts-manager-header">
        <div>
          <h3>Google Calendar</h3>
          <p>
            Calendar access is connected separately from mail and is read-only.
            Each account gets its own Calendar consent and keychain credential.
          </p>
        </div>
        <button
          type="button"
          className="primary-action settings-add-account"
          disabled={busyEmail !== null}
          onClick={() => act("__add__", onAdd)}
        >
          <Plus size={15} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Connect Calendar"}
        </button>
      </div>
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="accounts-config-notice">
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
                  <span className="account-card-email">Read-only calendar access</span>
                </div>
                <span className="accounts-list-actions">
                  {account.status === "needs_reauth" ? (
                    <button
                      type="button"
                      className="account-action-button"
                      disabled={busyEmail !== null}
                      onClick={() => act(account.email, () => onReconnect(account.email))}
                    >
                      Reconnect
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyEmail !== null}
                    onClick={() => act(account.email, () => onRemove(account.email))}
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
                <div className="settings-inline-confirm" role="group" aria-label="Remove calendar account from all devices confirmation">
                  <p><strong>Remove from every device?</strong><br />This disconnects {account.email} everywhere. Google Calendar itself is not changed.</p>
                  <span className="settings-inline-confirm-actions">
                    <button type="button" disabled={busyEmail !== null} onClick={() => setConfirmEverywhereEmail(null)}>Cancel</button>
                    <button
                      type="button"
                      className="danger-action"
                      disabled={busyEmail !== null}
                      onClick={() => {
                        act(account.email, () => onRemoveEverywhere(account.email));
                        setConfirmEverywhereEmail(null);
                      }}
                    >
                      Remove on all devices
                    </button>
                  </span>
                </div>
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
                            act(account.email, () => onSetSelection(account.email, selected));
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

function SplitInboxesSettings({
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
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // A split inbox belongs to one account, so only that account's labels are
  // valid matches for it.
  const labelOptions = (labelsByAccount[accountId] ?? [])
    .filter((label) => label.kind === "user")
    .sort((a, b) => formatLabelName(a).localeCompare(formatLabelName(b), undefined, { sensitivity: "base" }));

  const act = (busyKey: string, operation: () => Promise<void>) => {
    setBusyId(busyKey);
    setError(null);
    void operation()
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setBusyId(null));
  };

  const move = (index: number, direction: -1 | 1) => {
    const target = index + direction;
    if (target < 0 || target >= splitInboxes.length) return;
    const next = [...splitInboxes];
    [next[index], next[target]] = [next[target]!, next[index]!];
    act(splitInboxes[index]!.id, () => onReorder(next.map((splitInbox) => splitInbox.id)));
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
            .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
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
                <span className="account-card-avatar split-inbox-avatar" aria-hidden="true">
                  {splitInbox.name.charAt(0).toUpperCase()}
                </span>
                <div className="account-card-identity">
                  <SplitInboxNameInput
                    splitInbox={splitInbox}
                    onCommit={(nextName) => act(splitInbox.id, () => onRename(splitInbox.id, nextName))}
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
                    onClick={() => act(splitInbox.id, () => onDelete(splitInbox.id))}
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

function SnippetsSettings({
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
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

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
                <span className="account-card-avatar split-inbox-avatar" aria-hidden="true">
                  {snippet.name.charAt(0).toUpperCase()}
                </span>
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
                    onClick={() => {
                      setBusyId(snippet.id);
                      setError(null);
                      void onDelete(snippet.id)
                        .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
                        .finally(() => setBusyId(null));
                    }}
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
            .catch(() => {})
            .finally(() => { pendingCount.current--; });
        }, 200);
      }}
    />
  );
}

function AiProviderSettings({ onChange }: { onChange?: () => void }) {
  const [provider, setProvider] = useState(readAiProvider);
  const [model, setModel] = useState(readAiModel);
  const [endpoint, setEndpoint] = useState(readAiEndpoint);
  const [features, setFeatures] = useState<AiFeatureFlags>(readAiFeatures);
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
    setFeatures((current) => {
      const next = { ...current, [flag]: value };
      saveAiFeatures(next);
      return next;
    });
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
                  .catch((reason: unknown) => setConfigurationError(reason instanceof Error ? reason.message : String(reason)))
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
                  .catch((reason: unknown) => setConfigurationError(reason instanceof Error ? reason.message : String(reason)))
                  .finally(() => setBusy(false));
              }}
            >
              Remove Key
            </button>
          </div>
          <div className="settings-row ai-connection-actions">
            <button
              type="button"
              disabled={busy || testingConnection || !keyConfigured || !resolveAiModel(provider, model) || (provider === "custom" && !endpoint.trim())}
              onClick={() => {
                setTestingConnection(true);
                setConfigurationError(null);
                setConnectionTested(false);
                void testAiConnection(provider, resolveAiModel(provider, model), endpoint)
                  .then(() => setConnectionTested(true))
                  .catch((reason: unknown) => setConfigurationError(reason instanceof Error ? reason.message : String(reason)))
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
            <span>Thread Actions</span>
            <input
              type="checkbox"
              checked={features.actionExtraction}
              onChange={(event) => updateFeature("actionExtraction", event.target.checked)}
            />
          </label>
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
    setMessage(error instanceof Error ? error.message : String(error));
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
