import { useEffect, useState } from "react";
import { errorMessage } from "./errors";
import {
  JOIN_CODE_LIFETIME_HOURS,
  replicatedSyncCancelJoinCode,
  replicatedSyncCreateJoinCode,
  replicatedSyncDismissJoinCodeNotice,
  replicatedSyncJoinWithCode,
  replicatedSyncPickJoinFolder,
  replicatedSyncPreviewJoinCode,
  replicatedSyncRotateEpoch,
  type JoinCodeNotice,
  type JoinCodePreview,
  type OutstandingJoinCode,
  type ReplicatedSyncTransportStatus,
} from "./replicatedSync";
import {
  connectorDisplayName,
  connectorKindLabel,
  EMPTY_S3_CREDENTIALS,
  S3CredentialFields,
  s3CredentialsComplete,
  s3CredentialsFromDraft,
  type S3CredentialDraft,
} from "./SyncConnectors";
import { formatTimeUntil, InlineStatus, plural, type Operation } from "./syncSettingsParts";
import { InlineConfirm } from "./InlineConfirm";

/** How long a pasted code waits before it's parsed, like the recovery-phrase check. */
const PREVIEW_DELAY_MS = 200;

export type JoinCodeReadiness = { ready: boolean; missing: string[] };

/** Whether a previewed code has everything it needs from this device. */
export function joinCodeReadiness(
  preview: JoinCodePreview | null,
  folders: Readonly<Record<number, string>>,
  credentials: Readonly<Record<number, S3CredentialDraft>>,
): JoinCodeReadiness {
  if (!preview) return { ready: false, missing: [] };
  if (preview.expired) return { ready: false, missing: ["This join code expired. Ask for a new one."] };
  const usable = preview.connectors.filter((connector) => connector.supported);
  if (usable.length === 0) {
    return { ready: false, missing: ["This version of ThreeStrands can’t use any connector in this code. Update this app."] };
  }
  const missing: string[] = [];
  for (const connector of usable) {
    if (connector.needsFolder && !folders[connector.index]) {
      missing.push(`Choose this device’s copy of “${connector.folderName ?? connector.location}”.`);
    }
    if (connector.needsCredentials && !s3CredentialsComplete(credentials[connector.index] ?? EMPTY_S3_CREDENTIALS)) {
      missing.push(`Enter the access key for ${connector.location}.`);
    }
  }
  return { ready: missing.length === 0, missing };
}

/** Paste a join code from another device and join in one step. The code
 * lives only in this component's state, so leaving the panel discards it. */
export function JoinCodePanel({ operation, refresh, inputId }: { operation: Operation; refresh(): Promise<void>; inputId?: string }) {
  const { busy, pending, runFor } = operation;
  const [code, setCode] = useState("");
  const [preview, setPreview] = useState<JoinCodePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [folders, setFolders] = useState<Record<number, string>>({});
  const [credentials, setCredentials] = useState<Record<number, S3CredentialDraft>>({});

  useEffect(() => {
    setFolders({});
    setCredentials({});
    if (!code.trim()) {
      setPreview(null);
      setPreviewError(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      replicatedSyncPreviewJoinCode(code).then(
        (next) => { if (!cancelled) { setPreview(next); setPreviewError(null); } },
        (reason: unknown) => { if (!cancelled) { setPreview(null); setPreviewError(errorMessage(reason)); } },
      );
    }, PREVIEW_DELAY_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [code]);

  const readiness = joinCodeReadiness(preview, folders, credentials);
  const inviter = preview?.inviterName.trim() || "another device";

  const chooseFolder = (index: number) => runFor(`join-folder-${index}`, async () => {
    const path = await replicatedSyncPickJoinFolder();
    if (path) setFolders((current) => ({ ...current, [index]: path }));
  });

  const join = () => runFor("join-code", async () => {
    await replicatedSyncJoinWithCode(code, {
      folders: Object.entries(folders).map(([index, path]) => ({ connectorIndex: Number(index), path })),
      credentials: Object.entries(credentials)
        .filter(([index]) => preview?.connectors[Number(index)]?.needsCredentials)
        .map(([index, draft]) => ({ connectorIndex: Number(index), credentials: { kind: "s3" as const, ...s3CredentialsFromDraft(draft) } })),
    });
    setCode("");
    await refresh();
  });

  return (
    <>
      <label className="settings-field">
        <span>Join code</span>
        <textarea
          id={inputId}
          rows={3}
          className="sync-join-code"
          placeholder="TSJOIN1-…"
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          value={code}
          disabled={busy}
          aria-invalid={previewError ? true : undefined}
          onChange={(event) => setCode(event.target.value)}
        />
      </label>
      {previewError ? <p role="status" className="settings-hint recovery-phrase-feedback-attention">{previewError}</p> : null}
      {preview ? (
        <>
          <p className="settings-hint" role="status">
            {preview.expired
              ? "This join code expired. Ask for a new one."
              : `From ${inviter} · expires ${formatTimeUntil(preview.expiresAt)}`}
          </p>
          <ul className="accounts-list" aria-label="Connectors in this join code">
            {preview.connectors.map((connector) => (
              <li key={connector.index} className="account-card">
                <div className="account-card-identity">
                  <strong>{connector.label?.trim() || connector.location || connectorKindLabel(connector.kind)}</strong>
                  <span className="account-card-email">
                    <span className="sync-kind-badge">{connectorKindLabel(connector.kind)}</span>
                    {connector.supported && connector.credentialsIncluded ? " · Credentials included" : ""}
                    {connector.needsCredentials ? " · You’ll enter credentials" : ""}
                  </span>
                  {!connector.supported ? (
                    <span className="account-card-email">This version of ThreeStrands can’t use this connector, so it will be skipped.</span>
                  ) : null}
                </div>
                {connector.supported && connector.needsFolder ? (
                  <>
                    <p className="settings-hint">On {inviter} this folder is named “{connector.folderName}”. Choose this device’s copy of it.</p>
                    <div className="settings-row">
                      <button type="button" className="account-action-button" disabled={busy} onClick={() => chooseFolder(connector.index)}>
                        {folders[connector.index] ? "Choose a different folder…" : "Choose folder…"}
                      </button>
                      {folders[connector.index] ? <span className="settings-hint">{folders[connector.index]}</span> : null}
                    </div>
                    <InlineStatus operation={operation} for={`join-folder-${connector.index}`} />
                  </>
                ) : null}
                {connector.supported && connector.needsCredentials ? (
                  <S3CredentialFields
                    value={credentials[connector.index] ?? EMPTY_S3_CREDENTIALS}
                    disabled={busy}
                    onChange={(next) => setCredentials((current) => ({ ...current, [connector.index]: next }))}
                  />
                ) : null}
              </li>
            ))}
          </ul>
          {readiness.missing.map((message) => <p key={message} className="settings-hint">{message}</p>)}
        </>
      ) : null}
      <button type="button" className="primary-action" disabled={busy || !readiness.ready} onClick={join}>
        {pending === "join-code" ? "Joining…" : "Join sync group"}
      </button>
      <InlineStatus operation={operation} for="join-code" />
    </>
  );
}

function lifetimeLabel(hours: number): string {
  return hours % 24 === 0 && hours >= 48 ? plural(hours / 24, "day") : plural(hours, "hour");
}

/** Creates a join code for a new device. Closing the panel discards the code. */
export function AddDevicePanel({
  transports,
  operation,
  refresh,
  onClose,
}: {
  transports: ReplicatedSyncTransportStatus[];
  operation: Operation;
  refresh(): Promise<void>;
  onClose(): void;
}) {
  const { busy, runFor } = operation;
  const [hours, setHours] = useState<number>(24);
  const [included, setIncluded] = useState<Record<string, boolean>>(() => Object.fromEntries(transports.map((transport) => [transport.instanceId, true])));
  const [withCredentials, setWithCredentials] = useState<Record<string, boolean>>(() => Object.fromEntries(transports.map((transport) => [transport.instanceId, true])));
  const [created, setCreated] = useState<{ code: string; expiresAt: string; credentials: boolean } | null>(null);
  const [copied, setCopied] = useState(false);
  const chosen = transports.filter((transport) => included[transport.instanceId]);

  const create = () => runFor("create-join-code", async () => {
    const choices = chosen.map((transport) => ({
      instanceId: transport.instanceId,
      includeCredentials: transport.kind !== "folder" && Boolean(withCredentials[transport.instanceId]),
    }));
    const code = await replicatedSyncCreateJoinCode(hours, choices);
    setCreated({
      code,
      expiresAt: new Date(Date.now() + hours * 3_600_000).toISOString(),
      credentials: choices.some((choice) => choice.includeCredentials),
    });
    await refresh();
  });

  const copy = () => {
    if (!created) return;
    void navigator.clipboard?.writeText(created.code).then(() => setCopied(true), () => setCopied(false));
  };

  return (
    <div className="settings-inline-panel" role="group" aria-label="Add a device">
      <p>Paste this code on your new device. It sets up the same connectors and joins this sync group in one step.</p>
      {created ? (
        <>
          <textarea className="sync-join-code" readOnly rows={4} aria-label="Join code" value={created.code} onFocus={(event) => event.target.select()} />
          <div className="settings-row">
            <button type="button" className="primary-action" onClick={copy}>{copied ? "Copied" : "Copy"}</button>
          </div>
          <p className="settings-hint sync-attention-text">
            Anyone with this code can join your sync group{created.credentials ? " and use the included storage credentials" : ""}.
            It works once and expires {formatTimeUntil(created.expiresAt)}. Send it only to yourself through a private channel.
            {created.credentials ? " The credentials don’t expire with the code, so a key limited to this bucket is safest." : ""}
          </p>
          <span className="settings-inline-confirm-actions">
            <button type="button" onClick={onClose}>Done</button>
          </span>
        </>
      ) : (
        <>
          <label className="settings-field">
            <span>Expires after</span>
            <select value={hours} disabled={busy} onChange={(event) => setHours(Number(event.target.value))}>
              {JOIN_CODE_LIFETIME_HOURS.map((option) => <option key={option} value={option}>{lifetimeLabel(option)}</option>)}
            </select>
          </label>
          <fieldset className="sync-join-connectors">
            <legend>Connectors to include</legend>
            {transports.map((transport) => (
              <div key={transport.instanceId} className="sync-join-connector">
                <label className="settings-checkbox">
                  <input
                    type="checkbox"
                    checked={Boolean(included[transport.instanceId])}
                    disabled={busy}
                    onChange={(event) => setIncluded((current) => ({ ...current, [transport.instanceId]: event.target.checked }))}
                  />
                  <span>{connectorDisplayName(transport)} ({connectorKindLabel(transport.kind)})</span>
                </label>
                {transport.kind === "folder" ? (
                  <p className="settings-hint">The other device will choose its own copy of this folder.</p>
                ) : (
                  <label className="settings-checkbox sync-join-credentials">
                    <input
                      type="checkbox"
                      checked={Boolean(withCredentials[transport.instanceId])}
                      disabled={busy || !included[transport.instanceId]}
                      onChange={(event) => setWithCredentials((current) => ({ ...current, [transport.instanceId]: event.target.checked }))}
                    />
                    <span>Include credentials</span>
                  </label>
                )}
              </div>
            ))}
          </fieldset>
          <span className="settings-inline-confirm-actions">
            <button type="button" disabled={busy} onClick={onClose}>Cancel</button>
            <button type="button" className="primary-action" disabled={busy || chosen.length === 0} onClick={create}>Create join code</button>
          </span>
          <InlineStatus operation={operation} for="create-join-code" />
        </>
      )}
    </div>
  );
}

export function describeJoinCode(code: OutstandingJoinCode, now: number = Date.now()): string {
  switch (code.status) {
    case "open": return `Open · expires ${formatTimeUntil(code.expiresAt, now)}`;
    case "redeemed": return `Used by ${code.redeemedByName?.trim() || "a new device"}`;
    case "expired": return "Expired unused";
    case "cancelled": return "Cancelled";
    default: return code.status;
  }
}

/** How many closed codes to keep listed beside the open ones. */
const RECENT_CLOSED_CODES = 5;

function OutstandingJoinCodeRow({ code, operation }: { code: OutstandingJoinCode; operation: Operation }) {
  const { busy, actFor } = operation;
  const [confirming, setConfirming] = useState(false);
  const key = `join-code:${code.invitationCid}`;
  return (
    <li className={`account-card${code.rejectedAttempts > 0 ? " sync-location-attention" : ""}`}>
      <div className="account-card-row">
        <div className="account-card-identity">
          <strong>{describeJoinCode(code)}</strong>
          <span className="account-card-email">Created {new Date(code.createdAt).toLocaleString()}</span>
          {code.rejectedAttempts > 0 ? (
            <span className="account-card-email sync-attention-text">Refused {plural(code.rejectedAttempts, "later attempt")} to use this code</span>
          ) : null}
        </div>
        {code.status === "open" && !confirming ? (
          <button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => setConfirming(true)}>Cancel…</button>
        ) : null}
      </div>
      {confirming ? (
        <InlineConfirm ariaLabel="Cancel join code confirmation" cancelLabel="Keep" onCancel={() => setConfirming(false)} disabled={busy}
          actions={[{ label: "Cancel code", className: "danger-action", onClick: () => { setConfirming(false); actFor(key, () => replicatedSyncCancelJoinCode(code.invitationCid)); } }]}>
          <strong>Cancel this join code?</strong><br />It stops working, and your sync group’s keys change so it can’t be used later.
        </InlineConfirm>
      ) : null}
      <InlineStatus operation={operation} for={key} />
    </li>
  );
}

export function OutstandingJoinCodes({ codes, operation }: { codes: OutstandingJoinCode[]; operation: Operation }) {
  const open = codes.filter((code) => code.status === "open");
  const recent = codes.filter((code) => code.status !== "open").slice(0, RECENT_CLOSED_CODES);
  const shown = [...open, ...recent];
  if (shown.length === 0) return null;
  return (
    <>
      <h4>Join codes</h4>
      <ul className="accounts-list" aria-label="Join codes">
        {shown.map((code) => <OutstandingJoinCodeRow key={code.invitationCid} code={code} operation={operation} />)}
      </ul>
    </>
  );
}

function JoinCodeNoticeRow({ notice, operation }: { notice: JoinCodeNotice; operation: Operation }) {
  const { busy, actFor } = operation;
  const [revoking, setRevoking] = useState(false);
  const key = `join-notice:${notice.redemptionCid}`;
  const inviter = notice.inviterName?.trim() || "another device";
  return (
    <li className="account-card sync-notice">
      <div className="account-card-row">
        <p className="account-card-identity">
          {notice.kind === "joined"
            ? `${notice.deviceName || "A new device"} joined with a join code from ${inviter}.`
            : `A device named “${notice.deviceName || "unnamed"}” tried to use a join code that was already used, expired, or cancelled.`}
        </p>
        {notice.kind === "joined" && !revoking ? (
          <button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => setRevoking(true)}>Not you? Revoke…</button>
        ) : null}
        <button type="button" className="account-action-button" disabled={busy} onClick={() => actFor(key, () => replicatedSyncDismissJoinCodeNotice(notice.redemptionCid))}>
          Dismiss
        </button>
      </div>
      {revoking ? (
        <InlineConfirm ariaLabel="Revoke device confirmation" cancelLabel="Cancel" onCancel={() => setRevoking(false)} disabled={busy}
          actions={[{ label: "Revoke device", className: "danger-action", onClick: () => { setRevoking(false); actFor(key, () => replicatedSyncRotateEpoch(notice.deviceId)); } }]}>
          <strong>Revoke this device?</strong><br />It keeps existing data, but future writes from it will no longer be trusted.
        </InlineConfirm>
      ) : null}
      <InlineStatus operation={operation} for={key} />
    </li>
  );
}

export function JoinCodeNotices({ notices, operation }: { notices: JoinCodeNotice[]; operation: Operation }) {
  if (notices.length === 0) return null;
  return (
    <ul className="accounts-list" aria-label="Join code notices">
      {notices.map((notice) => <JoinCodeNoticeRow key={notice.redemptionCid} notice={notice} operation={operation} />)}
    </ul>
  );
}
