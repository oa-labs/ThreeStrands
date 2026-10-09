import { CircleAlert, CircleCheck } from "lucide-react";
import { useState, type ReactNode } from "react";
import { clearLocalCrashReports, crashReportingEnabled, localCrashReports, setCrashReportingEnabled } from "./crashReporting";
import type { RecoveryStatus, SyncStatus } from "./domain";
import { useSettingsOperation } from "./settingsOperations";
import { ICON_SIZE } from "./iconSizes";
import type { SyncDiagnosticsActions } from "./settingsPanelTypes";

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

/** Lines of the merged sync error that are not just a failed mutation's
 * error repeated: the native status falls back to the newest failed
 * mutation's error, and the merged status prefixes each line with its
 * account, so both shapes are recognised. */
function independentSyncErrors(status: SyncStatus | null): string[] {
  if (!status?.error) return [];
  const failedErrors = status.failedMutations.map((mutation) => mutation.error);
  return status.error
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !failedErrors.some((error) => line === error || line.endsWith(`: ${error}`)));
}

function plural(count: number, singular: string, pluralForm = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : pluralForm}`;
}

function DiagnosticsIssue({
  title,
  children,
  actions,
}: {
  title: string;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="diagnostics-issue" role="group" aria-label={title}>
      <div className="diagnostics-issue-header">
        <CircleAlert size={ICON_SIZE.sm} aria-hidden="true" />
        <strong>{title}</strong>
        {actions ? <span className="diagnostics-issue-actions">{actions}</span> : null}
      </div>
      {children}
    </div>
  );
}

function SyncDiagnosticsDetails({
  status,
  accountCount,
}: {
  status: SyncStatus | null;
  /** Connected mail accounts. The merged status only carries a cursor when
   * there is exactly one, so the row is meaningless beyond that. */
  accountCount: number;
}) {
  return (
    <dl className="diagnostics">
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
      <dt>Last error</dt><dd>{status?.error ?? "None"}</dd>
    </dl>
  );
}

export function DiagnosticsSettings({
  status,
  recovery,
  accountCount,
  actions,
}: {
  status: SyncStatus | null;
  recovery?: RecoveryStatus | null;
  accountCount: number;
  actions?: SyncDiagnosticsActions;
}) {
  const [reporting, setReporting] = useState(crashReportingEnabled);
  const [reportCount, setReportCount] = useState(() => localCrashReports().length);
  const { pending, error, runFor } = useSettingsOperation();
  const failed = status?.failedMutations ?? [];
  const quarantined = status?.quarantinedMessages ?? [];
  const syncErrors = independentSyncErrors(status);
  const issueCount = (recovery ? 1 : 0) + (failed.length ? 1 : 0) + (quarantined.length ? 1 : 0) + (syncErrors.length ? 1 : 0);
  const pendingCount = status?.pendingMutations ?? 0;

  return (
    <section className="settings-section" aria-label="Diagnostics">
      <h3>Sync Health</h3>
      <div className={`diagnostics-summary${issueCount ? " attention" : ""}`} role="status">
        {issueCount ? <CircleAlert size={ICON_SIZE.lg} aria-hidden="true" /> : <CircleCheck size={ICON_SIZE.lg} aria-hidden="true" />}
        <div>
          <strong>{issueCount ? `${plural(issueCount, "item")} to review` : "Sync is healthy"}</strong>
          <span>
            {status?.lastSuccessfulSync
              ? `Last synced ${new Date(status.lastSuccessfulSync).toLocaleString()}`
              : "Not synced yet"}
            {pendingCount ? ` · ${plural(pendingCount, "change")} waiting to sync` : ""}
          </span>
        </div>
      </div>

      {recovery ? (
        <DiagnosticsIssue
          title="Mail cache was recovered"
          actions={actions ? <button className="btn btn-sm" type="button" onClick={actions.dismissRecovery}>Dismiss</button> : null}
        >
          <p>{recoveryStatusMessage(recovery)}</p>
        </DiagnosticsIssue>
      ) : null}

      {syncErrors.length ? (
        <DiagnosticsIssue title="Last sync attempt failed">
          <p>This clears automatically after the next successful sync.</p>
          <ul className="failed-mutations">
            {syncErrors.map((line) => <li key={line}>{line}</li>)}
          </ul>
        </DiagnosticsIssue>
      ) : null}

      {failed.length ? (
        <DiagnosticsIssue
          title={`${plural(failed.length, "change")} couldn’t be applied in Gmail`}
          actions={actions ? (
            <>
              <button className="btn btn-sm" type="button" disabled={pending !== null} onClick={() => runFor("retry", actions.retryFailed)}>
                {pending === "retry" ? "Retrying…" : "Retry"}
              </button>
              <button className="btn btn-sm" type="button" disabled={pending !== null} onClick={() => runFor("dismiss-failed", actions.dismissProblems)}>
                Dismiss
              </button>
            </>
          ) : null}
        >
          <p>Gmail rejected these, so they only took effect in ThreeStrands. Retry once the cause is fixed, such as after reconnecting an account, or dismiss them.</p>
          <ul className="failed-mutations">
            {failed.map((mutation) => (
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
        </DiagnosticsIssue>
      ) : null}

      {quarantined.length ? (
        <DiagnosticsIssue
          title={`${plural(quarantined.length, "message")} couldn’t be read`}
          actions={actions ? (
            <button className="btn btn-sm" type="button" disabled={pending !== null} onClick={() => runFor("dismiss-quarantine", actions.dismissProblems)}>
              Dismiss
            </button>
          ) : null}
        >
          <p>These messages were skipped so the rest of their conversations could sync. They are retried whenever their conversation changes.</p>
          <ul className="failed-mutations">
            {quarantined.map((message) => (
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
        </DiagnosticsIssue>
      ) : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}

      <details className="settings-disclosure diagnostics-details">
        <summary>Technical details</summary>
        <SyncDiagnosticsDetails status={status} accountCount={accountCount} />
      </details>

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
        Disabled by default. A report holds the error message, stack trace, app
        version, and browser engine. Email addresses, URLs, quoted text, and
        message headers are redacted, and nothing from your mail is included.
      </span>
      <button className="btn"
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
