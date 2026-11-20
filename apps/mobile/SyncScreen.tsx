import { useState, type FormEvent } from "react";
import { errorText, mobile, type JoinPreview } from "./api";
import type { useSync } from "./useSync";

export function SyncScreen({ state }: { state: ReturnType<typeof useSync> }) {
  const [code, setCode] = useState("");
  const [preview, setPreview] = useState<JoinPreview>();
  const [accessKeyId, setAccessKeyId] = useState("");
  const [secretAccessKey, setSecretAccessKey] = useState("");
  const [sessionToken, setSessionToken] = useState("");
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string>();
  const [message, setMessage] = useState<string>();
  const { status, running, issue, sync, setStatus } = state;
  const connector = preview?.connectors[0];

  function resetPreview() {
    setPreview(undefined);
    setAccessKeyId("");
    setSecretAccessKey("");
    setSessionToken("");
  }

  async function action(run: () => Promise<void>) {
    setWorking(true);
    setError(undefined);
    setMessage(undefined);
    try { await run(); } catch (failure) { setError(errorText(failure)); }
    finally { setWorking(false); }
  }

  async function pair(event: FormEvent) {
    event.preventDefault();
    await action(async () => {
      if (!preview) { setPreview(await mobile.previewJoin(code)); return; }
      const credentials = connector?.needsCredentials ?
        { accessKeyId, secretAccessKey, sessionToken: sessionToken || undefined } : null;
      setStatus(await mobile.join(code, credentials));
      setCode("");
      resetPreview();
      await sync();
    });
  }

  return <section className="sync-screen" aria-label="Sync settings">
    <h2>Sync settings</h2>
    {!status && !issue && <p role="status">Checking sync…</p>}
    {status?.revoked && <p role="status">Desktop revoked this phone. Your notes and tasks are still on this device.</p>}
    {status?.enrolled && !status.revoked && <div className="callout">
      <h2>Encrypted sync</h2>
      {status.awaitingAdmissionFrom && <p>Waiting for {status.awaitingAdmissionFrom} to sync and admit this phone.</p>}
      {status.lastSuccessAt && <p>Last successful connector contact {new Date(status.lastSuccessAt).toLocaleString()}.</p>}
      {!status.lastSuccessAt && !status.awaitingAdmissionFrom && <p>First sync is pending.</p>}
      <button className="primary" disabled={running || working} onClick={() => void sync()}>{running ? "Syncing…" : "Sync Now"}</button>
    </div>}
    {(error || issue || status?.lastError) && <p role="alert">{error || issue || status?.lastError}</p>}
    {status && !status.enrolled && <form className="editor" onSubmit={pair}>
      <p>On desktop, open Settings → Integrations → Replicated Sync. Add an HTTPS S3 connector, then choose Devices → Add a device and select only that connector for the join code.</p>
      <label>Join code
        <textarea value={code} spellCheck={false} autoComplete="off" autoCapitalize="off" onChange={event => {
          setCode(event.target.value);
          resetPreview();
          setError(undefined);
        }} />
      </label>
      {connector && <div className="callout">
        <h2>Join {preview.inviterName}’s sync group</h2>
        <p>{connector.location}</p>
        <p>This phone will join the entire encrypted group and share its local Notes and Tasks.</p>
        {connector.needsCredentials && <div className="editor">
          <label>Access key ID<input autoComplete="off" required value={accessKeyId} onChange={event => setAccessKeyId(event.target.value)} /></label>
          <label>Secret access key<input type="password" autoComplete="new-password" required value={secretAccessKey} onChange={event => setSecretAccessKey(event.target.value)} /></label>
          <label>Session token (optional)<input type="password" autoComplete="new-password" value={sessionToken} onChange={event => setSessionToken(event.target.value)} /></label>
        </div>}
      </div>}
      <button className="primary" disabled={working || !code.trim()}>{preview ? "Join Sync Group" : "Review Join Code"}</button>
    </form>}
    {status?.enrolled && <div className="note-actions">
      {confirmDisconnect ? <>
        <p>Your notes and tasks will stay on this phone.</p>
        <button disabled={working || running} onClick={() => void action(async () => {
          setStatus(await mobile.disconnect());
          setConfirmDisconnect(false);
          await sync();
        })}>Disconnect and Keep Data</button>
        <button onClick={() => setConfirmDisconnect(false)}>Cancel</button>
      </> : <button disabled={working || running} onClick={() => setConfirmDisconnect(true)}>Disconnect</button>}
    </div>}
    <section aria-label="Export" className="callout">
      <h2>Export</h2>
      <p>Save Markdown notes and a copy of your task records to a location you choose.</p>
      <button disabled={working} onClick={() => void action(async () => {
        if (await mobile.export()) setMessage("Export saved.");
      })}>Export Notes and Tasks</button>
      {message && <p role="status">{message}</p>}
    </section>
  </section>;
}
