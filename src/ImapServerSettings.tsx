import { type RefObject, useCallback, useEffect, useId, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { Account, ImapConnectionSettings, ImapSecurityMode } from "./domain";
import { useSettingsOperation } from "./settingsOperations";
import { errorMessage } from "./errors";
import {
  CertificateCheck,
  type CertificateState,
  SECURITY_OPTIONS,
  defaultPort,
  endpointKey,
} from "./ImapAccountSetup";

type Endpoint = { host: string; port: number; security: ImapSecurityMode; username: string };
type Kind = "imap" | "smtp";

const SECURITY_LABEL: Record<ImapSecurityMode, string> = Object.fromEntries(
  SECURITY_OPTIONS.map((option) => [option.value, option.label]),
) as Record<ImapSecurityMode, string>;

function describe(endpoint: Endpoint): string {
  return `${endpoint.host}:${endpoint.port} · ${SECURITY_LABEL[endpoint.security]} · ${endpoint.username}`;
}

function endpoints(settings: ImapConnectionSettings): Record<Kind, Endpoint> {
  return {
    imap: { host: settings.imapHost, port: settings.imapPort, security: settings.imapSecurity, username: settings.imapUsername },
    smtp: { host: settings.smtpHost, port: settings.smtpPort, security: settings.smtpSecurity, username: settings.smtpUsername },
  };
}

/**
 * The "Server" panel for ONE IMAP account: a read-only summary of the
 * incoming and outgoing servers, and an editor that changes them or the
 * password. Saving tests both servers first (nothing is written on failure)
 * and keeps the account's sending addresses, mailbox mapping and labels.
 * An account that needs reconnecting opens the editor straight away;
 * `reconnectRequest` changing (the header Reconnect button) opens it and
 * focuses the password.
 */
export function ImapServerSettings({
  account,
  needsReconnect,
  reconnectRequest = 0,
  onConnected,
}: {
  account: string;
  needsReconnect: boolean;
  reconnectRequest?: number;
  onConnected?(account: Account): Promise<void> | void;
}) {
  const { pending, busy, error, setError, runFor } = useSettingsOperation();
  const [settings, setSettings] = useState<ImapConnectionSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editing, setEditing] = useState(needsReconnect);
  const [saved, setSaved] = useState(false);
  const headingId = useId();
  const editButton = useRef<HTMLButtonElement>(null);
  const passwordInput = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    setSettings(await mailClient.getImapConnectionSettings(account));
  }, [account]);

  useEffect(() => {
    setLoadError(null);
    void load().catch((reason) => setLoadError(errorMessage(reason)));
  }, [load]);

  useEffect(() => {
    if (needsReconnect) setEditing(true);
  }, [needsReconnect]);

  useEffect(() => {
    if (reconnectRequest === 0) return;
    setEditing(true);
    setSaved(false);
    // The form mounts on this render; focus once it exists.
    requestAnimationFrame(() => passwordInput.current?.focus());
  }, [reconnectRequest]);

  const heading = <h4 id={headingId}>Server</h4>;
  if (loadError) {
    return (
      <section className="settings-subsection imap-server" aria-labelledby={headingId}>
        {heading}
        <p className="form-error" role="alert">{loadError}</p>
      </section>
    );
  }
  if (!settings) {
    return (
      <section className="settings-subsection imap-server" aria-labelledby={headingId}>
        {heading}
        <p role="status">Loading…</p>
      </section>
    );
  }

  const current = endpoints(settings);

  return (
    <section className="settings-subsection imap-server" aria-labelledby={headingId}>
      {heading}
      {editing ? (
        <ServerEditor
          account={account}
          settings={settings}
          needsReconnect={needsReconnect}
          busy={busy}
          saving={pending === "save"}
          error={error}
          passwordInput={passwordInput}
          onCancel={() => {
            setError(null);
            setEditing(false);
            requestAnimationFrame(() => editButton.current?.focus());
          }}
          onSave={(update) => runFor("save", async () => {
            const connected = await mailClient.updateImapConnection(update);
            await load();
            setEditing(false);
            setSaved(true);
            await onConnected?.(connected);
          })}
        />
      ) : (
        <>
          <dl className="imap-server-summary">
            <dt>Incoming (IMAP)</dt>
            <dd>{describe(current.imap)}</dd>
            <dt>Outgoing (SMTP)</dt>
            <dd>{describe(current.smtp)}</dd>
          </dl>
          {saved ? <p className="settings-hint" role="status">Server settings saved. Syncing mail…</p> : null}
          <div className="imap-server-actions">
            <button ref={editButton} type="button" className="btn btn-sm" onClick={() => {
              setSaved(false);
              setEditing(true);
            }}>
              Edit server settings…
            </button>
          </div>
        </>
      )}
    </section>
  );
}

function ServerEditor({
  account,
  settings,
  needsReconnect,
  busy,
  saving,
  error,
  passwordInput,
  onCancel,
  onSave,
}: {
  account: string;
  settings: ImapConnectionSettings;
  needsReconnect: boolean;
  busy: boolean;
  saving: boolean;
  error: string | null;
  passwordInput: RefObject<HTMLInputElement | null>;
  onCancel(): void;
  onSave(update: Parameters<typeof mailClient.updateImapConnection>[0]): void;
}) {
  const initial = endpoints(settings);
  const [values, setValues] = useState<Record<Kind, Endpoint>>(initial);
  const [password, setPassword] = useState("");
  const [certificates, setCertificates] = useState<Record<Kind, CertificateState | null>>({ imap: null, smtp: null });
  const [probing, setProbing] = useState<Kind | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);
  const passwordHint = useId();

  const key = (kind: Kind) => endpointKey(values[kind].host, values[kind].port, values[kind].security);
  // A trust decision belongs only to the endpoint that was probed.
  const trust = (kind: Kind) => (certificates[kind]?.endpoint === key(kind) ? certificates[kind] : null);
  const update = (kind: Kind, patch: Partial<Endpoint>) =>
    setValues((current) => ({ ...current, [kind]: { ...current[kind], ...patch } }));

  const probe = async (kind: Kind) => {
    setProbeError(null);
    setProbing(kind);
    setCertificates((current) => ({ ...current, [kind]: null }));
    try {
      const { host, port, security } = values[kind];
      const result = kind === "imap"
        ? await mailClient.probeImapCertificate(host.trim(), port, security)
        : await mailClient.probeSmtpCertificate(host.trim(), port, security);
      setCertificates((current) => ({ ...current, [kind]: { endpoint: key(kind), probe: result, pin: null } }));
    } catch (reason) {
      setProbeError(errorMessage(reason));
    } finally {
      setProbing(null);
    }
  };

  // A probed self-signed certificate must be trusted before the password is sent.
  const untrusted = (["imap", "smtp"] as const).some((kind) => {
    const state = trust(kind);
    return state && !state.probe.trustedByPlatform && !state.pin;
  });
  const disabled = busy || probing !== null;
  const shownError = probeError ?? error;

  return (
    <form
      className="imap-server-editor"
      onSubmit={(event) => {
        event.preventDefault();
        const imapUsername = values.imap.username.trim() || account;
        onSave({
          email: account,
          imapHost: values.imap.host.trim(),
          imapPort: values.imap.port,
          imapSecurity: values.imap.security,
          imapUsername,
          smtpHost: values.smtp.host.trim(),
          smtpPort: values.smtp.port,
          smtpSecurity: values.smtp.security,
          smtpUsername: values.smtp.username.trim() || imapUsername,
          password: password || null,
          imapPinnedFingerprint: trust("imap")?.pin ?? null,
          smtpPinnedFingerprint: trust("smtp")?.pin ?? null,
        });
      }}
    >
      {needsReconnect ? (
        <p className="imap-server-reconnect-notice">
          ThreeStrands couldn&apos;t sign in to this account. Enter your current password, check the server
          details, then test the connection.
        </p>
      ) : null}
      <fieldset className="imap-server-settings" disabled={disabled}>
        <label className="settings-field settings-field-row">
          <span>Password</span>
          <input
            ref={passwordInput}
            type="password"
            value={password}
            autoComplete="current-password"
            aria-describedby={passwordHint}
            onChange={(event) => setPassword(event.target.value)}
          />
        </label>
        <p id={passwordHint} className="settings-hint settings-field-detail">
          Leave blank to keep your current password.
        </p>
      </fieldset>
      {(["imap", "smtp"] as const).map((kind) => (
        <EndpointFields
          key={kind}
          kind={kind}
          value={values[kind]}
          disabled={disabled}
          certificate={trust(kind)}
          pinnedNow={kind === "imap" ? settings.imapCertificatePinned : settings.smtpCertificatePinned}
          unchanged={key(kind) === endpointKey(initial[kind].host, initial[kind].port, initial[kind].security)}
          onChange={(patch) => update(kind, patch)}
          onProbe={() => void probe(kind)}
          onTrust={() => {
            const state = trust(kind);
            if (state) setCertificates((current) => ({ ...current, [kind]: { ...state, pin: state.probe.certificate.sha256Fingerprint } }));
          }}
        />
      ))}
      <div className="imap-server-actions">
        <button type="button" className="btn btn-sm" disabled={busy} onClick={onCancel}>Cancel</button>
        <button
          type="submit"
          className="btn btn-sm btn-primary"
          disabled={disabled || untrusted || !values.imap.host.trim() || !values.smtp.host.trim()}
        >
          {saving ? "Testing…" : "Test and Save"}
        </button>
      </div>
      {probing ? <p className="settings-hint" role="status">Checking the {probing.toUpperCase()} server&apos;s certificate…</p> : null}
      {shownError ? <p className="form-error" role="alert">{shownError}</p> : null}
    </form>
  );
}

function EndpointFields({
  kind,
  value,
  disabled,
  certificate,
  pinnedNow,
  unchanged,
  onChange,
  onProbe,
  onTrust,
}: {
  kind: Kind;
  value: Endpoint;
  disabled: boolean;
  certificate: CertificateState | null;
  pinnedNow: boolean;
  unchanged: boolean;
  onChange(patch: Partial<Endpoint>): void;
  onProbe(): void;
  onTrust(): void;
}) {
  const name = kind.toUpperCase() as "IMAP" | "SMTP";
  return (
    <>
      <fieldset className="imap-server-settings" disabled={disabled}>
        <legend>{kind === "imap" ? "Incoming Mail (IMAP)" : "Outgoing Mail (SMTP)"}</legend>
        <label className="settings-field settings-field-row">
          <span>Host</span>
          <input value={value.host} required onChange={(event) => onChange({ host: event.target.value })} />
        </label>
        <label className="settings-field settings-field-row">
          <span>Port</span>
          <input type="number" value={value.port} required onChange={(event) => onChange({ port: Number(event.target.value) })} />
        </label>
        <label className="settings-field settings-field-row">
          <span>Security</span>
          <select
            value={value.security}
            onChange={(event) => {
              const security = event.target.value as ImapSecurityMode;
              onChange({ security, port: defaultPort(security, kind) });
            }}
          >
            {SECURITY_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>{option.label}</option>
            ))}
          </select>
        </label>
        <label className="settings-field settings-field-row">
          <span>Username</span>
          <input value={value.username} onChange={(event) => onChange({ username: event.target.value })} />
        </label>
        {pinnedNow && unchanged && !certificate?.pin ? (
          <p className="settings-hint settings-field-detail">Using the certificate you trusted for this server.</p>
        ) : null}
      </fieldset>
      <CertificateCheck kind={name} host={value.host} certificate={certificate} onProbe={onProbe} onTrust={onTrust} />
    </>
  );
}
