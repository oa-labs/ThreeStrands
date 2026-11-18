import { invoke } from "@tauri-apps/api/core";
import { useEffect, useId, useRef, useState } from "react";
import { aiRequestSnapshot, recoverAiRequests, cancelAiRequest, setAiRequestPolicy, usePendingAiRequests, type AiRequestSnapshot, type AiRequestRecord } from "./aiRequests";
import { aiActivityFeatureLabel } from "./AiActivityList";
import { formatUsd } from "./aiUsage";
import { errorMessage } from "./errors";

export function AiCancelButton({ command, owner, label, className = "btn btn-sm" }: { command: string | string[]; owner?: string | null; label?: string; className?: string }) {
  const pending = usePendingAiRequests();
  const [error, setError] = useState("");
  const matching = pending.filter((request) => owner != null && request.owner === owner && (typeof command === "string" ? request.command === command : command.includes(request.command)));
  if (!matching.length) return null;
  return <><button type="button" className={className} onClick={() => {
    setError("");
    void Promise.all(matching.map((request) => cancelAiRequest(request.requestId))).catch((reason: unknown) => setError(errorMessage(reason)));
  }}>{label ?? `Cancel AI Request${matching.length > 1 ? "s" : ""}`}</button>{error ? <p role="alert" className="form-error">{error}</p> : null}</>;
}

export function AiRequestControls() {
  const [snapshot, setSnapshot] = useState<AiRequestSnapshot | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [budget, setBudget] = useState("");
  const [error, setError] = useState("");
  const first = useRef(true);
  const hint = useId();
  useEffect(() => {
    let active = true;
    let loading = false;
    const refresh = async () => {
      if (loading) return;
      loading = true;
      try {
        const next = await aiRequestSnapshot(historyOpen);
        if (active) { setSnapshot(next); if (first.current) { setBudget(next.policy.dailyBudgetUsd?.toString() ?? ""); first.current = false; } }
      } catch (reason) { if (active) setError(errorMessage(reason)); }
      finally { loading = false; }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    return () => { active = false; clearInterval(timer); };
  }, [historyOpen]);
  return <section className="settings-section ai-request-controls" aria-label="AI request controls">
    <h3>Request Controls</h3>
    <p id={hint} className="settings-hint">A daily estimated spending limit for this device. Enter model prices under Usage first. Each request reserves its full output allowance; failed or cancelled requests keep their reservation when billing is unknown. Your provider’s bill can differ. Web Research has separate limits.</p>
    {error ? <p role="alert" className="form-error">{error}</p> : null}
    {!snapshot && error ? <details><summary>Recover AI Controls</summary><p>Recovery preserves the unreadable file and resets the budget and history. Today's unknown spending reservations will be lost.</p><button type="button" className="btn" onClick={() => void recoverAiRequests().then(() => aiRequestSnapshot(historyOpen)).then((next) => {setSnapshot(next);setError("");setBudget("");}).catch((reason: unknown) => setError(errorMessage(reason)))}>Reset Controls and Unknown Reservations</button></details> : null}
    <div className="ai-budget-row">
      <label className="settings-field">Daily AI Budget (USD)
        <input type="number" min="0.01" step="0.01" aria-describedby={hint} value={budget} onChange={(event) => setBudget(event.target.value)} placeholder="No limit" />
      </label>
      <button type="button" className="btn" disabled={!snapshot} onClick={() => {
        const dailyBudgetUsd = budget.trim() === "" ? null : Number(budget);
        if (dailyBudgetUsd !== null && (!Number.isFinite(dailyBudgetUsd) || dailyBudgetUsd <= 0)) { setError("Enter a positive daily AI budget, or leave it blank."); return; }
        setError("");
        void setAiRequestPolicy({ dailyBudgetUsd }).then(() => aiRequestSnapshot(historyOpen)).then(setSnapshot).catch((reason: unknown) => setError(errorMessage(reason)));
      }}>Save Budget</button>
    </div>
    {snapshot ? <>
      <p className="settings-hint">{formatUsd(snapshot.chargedUsd)} charged or reserved today; unpriced requests are not included. Up to {snapshot.maxConcurrent} requests run at once; at most {snapshot.maxPending} can be running or waiting.</p>
      {snapshot.active.map((request) => <p key={request.requestId}>{aiActivityFeatureLabel(request.feature)} <button type="button" className="btn btn-sm" onClick={() => void cancelAiRequest(request.requestId).catch((reason: unknown) => setError(errorMessage(reason)))}>Cancel Request</button></p>)}
      <details className="settings-disclosure ai-request-history" onToggle={(event) => setHistoryOpen(event.currentTarget.open)}><summary>Request Provenance</summary>
        <div className="settings-disclosure-body">
          <p className="settings-hint">Kept on this device for 30 days, up to 500 attempts and 8 MB. Source references and fingerprints are retained; email text, instructions, profiles, and provider replies are not.</p>
          {historyOpen ? snapshot.records.length ? <ul>{snapshot.records.slice(0, 20).map((record) => <ProvenanceEntry key={record.attemptId} record={record} />)}</ul> : <p className="settings-hint">No requests recorded yet.</p> : null}
        </div>
      </details>
    </> : null}
  </section>;
}

function ProvenanceEntry({ record }: { record: AiRequestRecord }) {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const sources = showAll ? record.context.sources : record.context.sources.slice(0, 50);
  return <li>
    {aiActivityFeatureLabel(record.feature)} · {record.model} · {record.requestOutcome} · {record.outcome}
    <details onToggle={(event) => setOpen(event.currentTarget.open)}><summary>Versions and Sources</summary>
      {open ? <>
        <p>Prompt {record.promptVersion} · Schema {record.schemaVersion} · Context {record.context.version}</p>
        <p>Request: {record.requestId}</p><p>Prompt fingerprint: {record.promptRevision}</p><p>Context fingerprint: {record.context.payloadRevision}</p>
        {[...new Set(record.context.scope.accountIds)].map((account) => <p key={account}>Account: {account}</p>)}
        <ul>{sources.map((source) => <li key={source.path}>{source.sourceId ?? (source.path || "Thread excerpt")} · {source.characters} characters{source.truncated ? " · truncated" : ""} · {source.revision}</li>)}</ul>
        {!showAll && sources.length < record.context.sources.length ? <button type="button" className="btn-link" onClick={() => setShowAll(true)}>Show All {record.context.sources.length} Sources</button> : null}
      </> : null}
    </details>
  </li>;
}

export function AiCachedProvenance({ threadId }: { threadId: string }) {
  const [records, setRecords] = useState<AiRequestRecord[]>([]);
  const [error, setError] = useState("");
  return <details className="ai-cached-provenance" onToggle={(event) => {
    if (!event.currentTarget.open || !("__TAURI_INTERNALS__" in window)) return;
    void invoke<[AiRequestRecord | null, AiRequestRecord | null]>("ai_result_provenance", {threadId}).then((rows) => {
      const unique = new Map<string,AiRequestRecord>();
      for (const record of rows ?? []) if (record) unique.set(record.attemptId,record);
      setRecords([...unique.values()]);
    }).catch(() => setError("Generation details could not be loaded."));
  }}><summary>Generation details</summary>
    {error ? <p role="status">{error}</p> : null}
    {records.length ? <ul>{records.map((record) => <ProvenanceEntry key={record.attemptId} record={record} />)}</ul> : <p>No recorded generation details for this cached result.</p>}
  </details>;
}
