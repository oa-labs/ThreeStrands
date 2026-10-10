import { useCallback, useMemo, useState } from "react";
import { mailClient } from "./data/client";
import type {
  Account,
  ImapCertificateProbe,
  ImapLabelStorage,
  ImapSecurityMode,
  ImapSetupRequest,
} from "./domain";

/**
 * Minimal, functional IMAP account-setup form (Phase 2 Slice 2).
 *
 * This is the manual-setup path, which the design makes a first-class choice:
 * Proton Bridge and self-hosted servers cannot be autodiscovered from the email
 * domain, so the user can always enter host / port / security directly. The
 * flow is: optionally autodiscover from the email, then (for an untrusted
 * self-signed certificate) review and trust its SHA-256 fingerprint, then
 * "test and save", which only persists after both IMAP and SMTP tests pass.
 *
 * Look-and-feel is deliberately plain — the developer owns polish. The logic
 * (discovery, cert-trust, test-and-save, error surfacing) is what this slice
 * delivers.
 */

const SECURITY_OPTIONS: { value: ImapSecurityMode; label: string }[] = [
  { value: "starttls", label: "STARTTLS" },
  { value: "implicit_tls", label: "Implicit TLS" },
];

function defaultPort(security: ImapSecurityMode, kind: "imap" | "smtp"): number {
  if (kind === "imap") return security === "implicit_tls" ? 993 : 143;
  return security === "implicit_tls" ? 465 : 587;
}

export function ImapAccountSetup({ onConnected }: { onConnected: (account: Account) => void }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [imapHost, setImapHost] = useState("");
  const [imapPort, setImapPort] = useState(143);
  const [imapSecurity, setImapSecurity] = useState<ImapSecurityMode>("starttls");
  const [imapUsername, setImapUsername] = useState("");
  const [smtpHost, setSmtpHost] = useState("");
  const [smtpPort, setSmtpPort] = useState(587);
  const [smtpSecurity, setSmtpSecurity] = useState<ImapSecurityMode>("starttls");
  const [labelStorage, setLabelStorage] = useState<ImapLabelStorage>("folders");
  const [labelContainer, setLabelContainer] = useState("");

  const [probe, setProbe] = useState<ImapCertificateProbe | null>(null);
  const [pinnedFingerprint, setPinnedFingerprint] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  const effectiveImapUsername = useMemo(() => imapUsername.trim() || email.trim(), [imapUsername, email]);

  const runDiscovery = useCallback(async () => {
    setError(null);
    setStatus("Searching for your mail provider…");
    setBusy(true);
    try {
      const result = await mailClient.discoverImapSettings(email.trim());
      if (result) {
        setImapHost(result.imap.host);
        setImapPort(result.imap.port);
        setImapSecurity(result.imap.security);
        setImapUsername(result.imap.username);
        setSmtpHost(result.smtp.host);
        setSmtpPort(result.smtp.port);
        setSmtpSecurity(result.smtp.security);
        setStatus(`Found settings via ${result.source}. Review and continue.`);
      } else {
        setStatus("No settings found automatically — enter them manually below.");
      }
    } catch (e) {
      setError(String(e));
      setStatus(null);
    } finally {
      setBusy(false);
    }
  }, [email]);

  const probeCertificate = useCallback(async () => {
    setError(null);
    setStatus("Checking the server's certificate…");
    setBusy(true);
    try {
      const result = await mailClient.probeImapCertificate(imapHost.trim(), imapPort, imapSecurity);
      setProbe(result);
      if (result.trustedByPlatform) {
        setPinnedFingerprint(null);
        setStatus("The certificate is trusted. You can test and save.");
      } else {
        setStatus("This server uses a self-signed certificate — review its fingerprint below.");
      }
    } catch (e) {
      setError(String(e));
      setStatus(null);
    } finally {
      setBusy(false);
    }
  }, [imapHost, imapPort, imapSecurity]);

  const trustCertificate = useCallback(() => {
    if (probe) {
      setPinnedFingerprint(probe.certificate.sha256Fingerprint);
      setStatus("Certificate trusted for this server. You can test and save.");
    }
  }, [probe]);

  const testAndSave = useCallback(async () => {
    setError(null);
    setStatus("Testing the connection…");
    setBusy(true);
    const request: ImapSetupRequest = {
      email: email.trim(),
      imapHost: imapHost.trim(),
      imapPort,
      imapSecurity,
      imapUsername: effectiveImapUsername,
      imapPassword: password,
      smtpHost: smtpHost.trim(),
      smtpPort,
      smtpSecurity,
      smtpUsername: effectiveImapUsername,
      smtpPassword: null,
      pinnedFingerprint,
      labelStorage,
      labelContainer: labelStorage === "folders" ? labelContainer.trim() || null : null,
    };
    try {
      const account = await mailClient.testAndSaveImapAccount(request);
      setStatus("Account connected.");
      onConnected(account);
    } catch (e) {
      setError(String(e));
      setStatus(null);
    } finally {
      setBusy(false);
    }
  }, [
    email,
    imapHost,
    imapPort,
    imapSecurity,
    effectiveImapUsername,
    password,
    smtpHost,
    smtpPort,
    smtpSecurity,
    pinnedFingerprint,
    labelStorage,
    labelContainer,
    onConnected,
  ]);

  return (
    <form
      className="imap-setup"
      onSubmit={(event) => {
        event.preventDefault();
        void testAndSave();
      }}
    >
      <h2>Add an IMAP account</h2>

      <label>
        Email address
        <input
          type="email"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          autoComplete="email"
          required
        />
      </label>
      <button type="button" className="btn btn-primary" onClick={() => void runDiscovery()} disabled={busy || !email.trim()}>
        Find settings automatically
      </button>

      <fieldset>
        <legend>Incoming mail (IMAP)</legend>
        <label>
          Host
          <input value={imapHost} onChange={(e) => setImapHost(e.target.value)} required />
        </label>
        <label>
          Port
          <input
            type="number"
            value={imapPort}
            onChange={(e) => setImapPort(Number(e.target.value))}
            required
          />
        </label>
        <label>
          Security
          <select
            value={imapSecurity}
            onChange={(e) => {
              const next = e.target.value as ImapSecurityMode;
              setImapSecurity(next);
              setImapPort(defaultPort(next, "imap"));
            }}
          >
            {SECURITY_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <label>
          Username (defaults to your email)
          <input value={imapUsername} onChange={(e) => setImapUsername(e.target.value)} />
        </label>
      </fieldset>

      <fieldset>
        <legend>Outgoing mail (SMTP)</legend>
        <label>
          Host
          <input value={smtpHost} onChange={(e) => setSmtpHost(e.target.value)} required />
        </label>
        <label>
          Port
          <input
            type="number"
            value={smtpPort}
            onChange={(e) => setSmtpPort(Number(e.target.value))}
            required
          />
        </label>
        <label>
          Security
          <select
            value={smtpSecurity}
            onChange={(e) => {
              const next = e.target.value as ImapSecurityMode;
              setSmtpSecurity(next);
              setSmtpPort(defaultPort(next, "smtp"));
            }}
          >
            {SECURITY_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
      </fieldset>

      <label>
        Password
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          autoComplete="current-password"
          required
        />
      </label>

      <fieldset>
        <legend>Labels</legend>
        <label>
          How this account stores labels
          <select
            value={labelStorage}
            onChange={(e) => setLabelStorage(e.target.value as ImapLabelStorage)}
          >
            <option value="folders">Label folders (under a container)</option>
            <option value="keywords">IMAP keywords</option>
            <option value="none">No user labels</option>
          </select>
        </label>
        {labelStorage === "folders" && (
          <label>
            Container mailbox (e.g. Labels)
            <input value={labelContainer} onChange={(e) => setLabelContainer(e.target.value)} />
          </label>
        )}
      </fieldset>

      <button type="button" className="btn" onClick={() => void probeCertificate()} disabled={busy || !imapHost.trim()}>
        Check certificate
      </button>

      {probe && !probe.trustedByPlatform && (
        <section className="imap-cert-trust">
          <h3>Review this server's certificate</h3>
          <p>
            This server is not trusted by a public certificate authority. Compare the fingerprint
            below against the one your server reports (for example Proton Bridge's &ldquo;Export TLS
            certificates&rdquo;) before trusting it.
          </p>
          <dl>
            <dt>Subject</dt>
            <dd>{probe.certificate.subject}</dd>
            <dt>Issuer</dt>
            <dd>{probe.certificate.issuer}</dd>
            <dt>SHA-256 fingerprint</dt>
            <dd>
              <code>{probe.certificate.sha256Fingerprint}</code>
            </dd>
          </dl>
          <button
            type="button"
            className="btn"
            onClick={trustCertificate}
            disabled={pinnedFingerprint === probe.certificate.sha256Fingerprint}
          >
            {pinnedFingerprint === probe.certificate.sha256Fingerprint
              ? "Certificate trusted"
              : "Trust this certificate for this server"}
          </button>
        </section>
      )}

      <button
        type="submit"
        className="btn btn-primary"
        disabled={
          busy ||
          !email.trim() ||
          !imapHost.trim() ||
          !smtpHost.trim() ||
          !password ||
          // A self-signed server must be trusted before we send the password.
          Boolean(probe && !probe.trustedByPlatform && !pinnedFingerprint)
        }
      >
        Test and save
      </button>

      {status && <p className="imap-setup-status">{status}</p>}
      {error && (
        <p className="imap-setup-error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}
