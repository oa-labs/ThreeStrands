import { useState } from "react";
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
  { value: "start_tls", label: "STARTTLS" },
  { value: "implicit_tls", label: "Implicit TLS" },
];

function defaultPort(security: ImapSecurityMode, kind: "imap" | "smtp"): number {
  if (kind === "imap") return security === "implicit_tls" ? 993 : 143;
  return security === "implicit_tls" ? 465 : 587;
}

type CertificateState = { endpoint: string; probe: ImapCertificateProbe; pin: string | null };
const endpointKey = (host: string, port: number, security: ImapSecurityMode) =>
  JSON.stringify([host.trim(), port, security]);

export function ImapAccountSetup({ onConnected }: { onConnected: (account: Account) => void | Promise<void> }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [imapHost, setImapHost] = useState("");
  const [imapPort, setImapPort] = useState(143);
  const [imapSecurity, setImapSecurity] = useState<ImapSecurityMode>("start_tls");
  const [imapUsername, setImapUsername] = useState("");
  const [smtpHost, setSmtpHost] = useState("");
  const [smtpPort, setSmtpPort] = useState(587);
  const [smtpSecurity, setSmtpSecurity] = useState<ImapSecurityMode>("start_tls");
  const [smtpUsername, setSmtpUsername] = useState("");
  const [labelStorage, setLabelStorage] = useState<ImapLabelStorage>("folders");
  const [labelContainer, setLabelContainer] = useState("");
  const [imapCertificate, setImapCertificate] = useState<CertificateState | null>(null);
  const [smtpCertificate, setSmtpCertificate] = useState<CertificateState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const imapEndpoint = endpointKey(imapHost, imapPort, imapSecurity);
  const smtpEndpoint = endpointKey(smtpHost, smtpPort, smtpSecurity);
  // A trust decision belongs to exactly the endpoint that was probed. Editing
  // any endpoint field immediately makes its old certificate and pin unusable.
  const imapTrust = imapCertificate?.endpoint === imapEndpoint ? imapCertificate : null;
  const smtpTrust = smtpCertificate?.endpoint === smtpEndpoint ? smtpCertificate : null;

  async function runDiscovery() {
    setError(null);
    setStatus("Searching for your mail provider…");
    setBusy(true);
    setImapCertificate(null);
    setSmtpCertificate(null);
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
        setSmtpUsername(result.smtp.username);
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
  }

  async function probeCertificate(kind: "imap" | "smtp") {
    setError(null);
    setStatus(`Checking the ${kind.toUpperCase()} server's certificate…`);
    setBusy(true);
    const setCertificate = kind === "imap" ? setImapCertificate : setSmtpCertificate;
    setCertificate(null);
    try {
      const probe = kind === "imap"
        ? await mailClient.probeImapCertificate(imapHost.trim(), imapPort, imapSecurity)
        : await mailClient.probeSmtpCertificate(smtpHost.trim(), smtpPort, smtpSecurity);
      setCertificate({ endpoint: kind === "imap" ? imapEndpoint : smtpEndpoint, probe, pin: null });
      setStatus(probe.trustedByPlatform
        ? `${kind.toUpperCase()} certificate is trusted by your system.`
        : `Review the ${kind.toUpperCase()} certificate fingerprint below.`);
    } catch (e) {
      setError(String(e));
      setStatus(null);
    } finally {
      setBusy(false);
    }
  }

  async function testAndSave() {
    setError(null);
    setStatus("Testing the connection…");
    setBusy(true);
    const request: ImapSetupRequest = {
      email: email.trim(),
      imapHost: imapHost.trim(), imapPort, imapSecurity,
      imapUsername: imapUsername.trim() || email.trim(),
      imapPassword: password,
      smtpHost: smtpHost.trim(), smtpPort, smtpSecurity,
      smtpUsername: smtpUsername.trim() || imapUsername.trim() || email.trim(),
      smtpPassword: null,
      imapPinnedFingerprint: imapTrust?.pin ?? null,
      smtpPinnedFingerprint: smtpTrust?.pin ?? null,
      labelStorage,
      labelContainer: labelStorage === "folders" ? labelContainer.trim() || null : null,
    };
    try {
      const account = await mailClient.testAndSaveImapAccount(request);
      setStatus("Account saved. IMAP mail sync is not available yet.");
      await onConnected(account);
    } catch (e) {
      setError(String(e));
      setStatus(null);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="imap-setup"
      onSubmit={(event) => {
        event.preventDefault();
        void testAndSave();
      }}
    >
      <h2>Add an IMAP account</h2>
      <p>Save and test your server settings. IMAP mail sync is not available yet.</p>
      <fieldset disabled={busy}>
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

        <CertificateCheck kind="IMAP" host={imapHost} certificate={imapTrust}
          onProbe={() => void probeCertificate("imap")}
          onTrust={() => { if (imapTrust) setImapCertificate({ ...imapTrust, pin: imapTrust.probe.certificate.sha256Fingerprint }); }} />

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
          <label>
            SMTP username (defaults to your IMAP username)
            <input value={smtpUsername} onChange={(e) => setSmtpUsername(e.target.value)} />
          </label>
        </fieldset>

        <CertificateCheck kind="SMTP" host={smtpHost} certificate={smtpTrust}
          onProbe={() => void probeCertificate("smtp")}
          onTrust={() => { if (smtpTrust) setSmtpCertificate({ ...smtpTrust, pin: smtpTrust.probe.certificate.sha256Fingerprint }); }} />

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
            [imapTrust, smtpTrust].some((trust) => trust && !trust.probe.trustedByPlatform && !trust.pin)
          }
        >
          Test and save
        </button>

      </fieldset>

      {status && <p className="imap-setup-status">{status}</p>}
      {error && (
        <p className="imap-setup-error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

function CertificateCheck({ kind, host, certificate, onProbe, onTrust }: {
  kind: "IMAP" | "SMTP";
  host: string;
  certificate: CertificateState | null;
  onProbe(): void;
  onTrust(): void;
}) {
  const probe = certificate?.probe;
  return <section aria-label={`${kind} certificate`}>
    <button type="button" className="btn" onClick={onProbe} disabled={!host.trim()}>
      Check {kind} certificate
    </button>
    {probe && !probe.trustedByPlatform && <div className="imap-cert-trust">
      <h3>Review the {kind} server's certificate</h3>
      <p>Compare this fingerprint with the one your server reports before trusting it.</p>
      <dl>
        <dt>Server</dt><dd>{host}</dd>
        <dt>Subject</dt><dd>{probe.certificate.subject}</dd>
        <dt>Issuer</dt><dd>{probe.certificate.issuer}</dd>
        <dt>SHA-256 fingerprint</dt><dd><code>{probe.certificate.sha256Fingerprint}</code></dd>
      </dl>
      <button type="button" className="btn" onClick={onTrust}
        disabled={certificate?.pin === probe.certificate.sha256Fingerprint}>
        {certificate?.pin === probe.certificate.sha256Fingerprint
          ? `${kind} certificate trusted` : `Trust this ${kind} certificate for this server`}
      </button>
    </div>}
  </section>;
}
