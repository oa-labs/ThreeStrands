import { Fragment, useEffect, useId, useState } from "react";
import { mailClient } from "./data/client";
import type { AiDiagnosticSnapshot } from "./domain";
import { aiActivityFeatureLabel } from "./AiActivityList";
import { AI_PROVIDERS } from "./aiSettings";
import { errorMessage } from "./errors";

export function AiDiagnosticsSettings() {
  const [snapshot, setSnapshot] = useState<AiDiagnosticSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [now, setNow] = useState(Date.now);
  const hintId = useId();

  useEffect(() => {
    let active = true;
    mailClient.aiDiagnostics().then(
      (value) => { if (active) setSnapshot(value); },
      (reason: unknown) => { if (active) setError(errorMessage(reason)); },
    );
    const timer = setInterval(() => setNow(Date.now()), 1_000);
    return () => { active = false; clearInterval(timer); };
  }, []);

  async function run(operation: () => Promise<AiDiagnosticSnapshot | void>) {
    setPending(true);
    setError(null);
    try {
      const result = await operation();
      if (result) setSnapshot(result);
      setNow(Date.now());
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setPending(false);
    }
  }

  const capturing = Boolean(snapshot?.captureUntil && new Date(snapshot.captureUntil).getTime() > now);
  return <>
    <h3>Application Logs</h3>
    <p className="settings-hint">Logs and AI diagnostics belong to this device. To troubleshoot another machine, open Diagnostics on that machine.</p>
    <button type="button" className="btn" disabled={pending} onClick={() => void run(() => mailClient.openLogsFolder())}>Open Logs Folder</button>

    <h3>AI Diagnostics</h3>
    <p className="settings-hint">Recent failures and hidden suggestions from Brief, Suggestions, and Ask About This Conversation appear here. Saved suggestions may predate these diagnostics. The latest 100 entries are kept locally for seven days and are not synced or included in settings exports.</p>
    <label className="settings-switch">
      <span>Capture Detailed AI Diagnostics</span>
      <input type="checkbox" checked={capturing} disabled={pending || snapshot === null} aria-describedby={hintId}
        onChange={(event) => { const enabled = event.target.checked; void run(() => mailClient.setAiDiagnosticCapture(enabled)); }} />
    </label>
    <p className="settings-hint" id={hintId}>For the next 15 minutes, also keep rejected date, time, duration, and time zone values. These values may contain private information. Message bodies and evidence excerpts are never captured. Capture stops on restart; captured entries remain until cleared or seven days pass.</p>
    {capturing && snapshot?.captureUntil ? <p role="status">Detailed capture ends at {new Date(snapshot.captureUntil).toLocaleTimeString()}.</p> : null}
    <div className="settings-section-heading-actions">
      <button type="button" className="btn" disabled={pending} onClick={() => void run(() => mailClient.aiDiagnostics())}>Refresh AI Diagnostics</button>
      <button type="button" className="btn" disabled={pending || !snapshot?.entries.length} onClick={() => void run(() => mailClient.clearAiDiagnostics())}>Clear AI Diagnostics</button>
    </div>
    {error ? <p className="form-error" role="alert">{error}</p> : null}
    {!snapshot && !error ? <p role="status">Loading AI diagnostics…</p> : null}
    {snapshot?.entries.length === 0 ? <p>No recent AI failures recorded on this device.</p> : null}
    {snapshot && snapshot.entries.length > 0 ? <ul className="ai-activity-list">
      {snapshot.entries.map((entry, index) => <li key={`${entry.requestId}:${index}`}>
        <strong>{entry.reason}</strong>
        <small>{new Date(entry.at).toLocaleString()} · {aiActivityFeatureLabel(entry.feature)} · {AI_PROVIDERS.find((provider) => provider.id === entry.provider)?.label ?? entry.provider} · {entry.model}</small>
        <details>
          <summary>Request details</summary>
          <p>Request reference: <code>{entry.requestId}</code></p>
          {entry.details ? <dl className="diagnostics">{Object.entries(entry.details).map(([key, value]) => <Fragment key={key}><dt>{key}</dt><dd>{value === null ? "null" : String(value)}</dd></Fragment>)}</dl> : <p>No detailed values captured for this entry.</p>}
        </details>
      </li>)}
    </ul> : null}
  </>;
}
