import { useEffect, useState } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import type { NoteReference } from "./noteEntities";
export type NoteLinkChoice = { label: string; detail: string; reference: NoteReference };

export function NoteLinkPicker({ onClose, onSelect }: { onClose(): void; onSelect(choice: NoteLinkChoice): Promise<void> }) {
  const [kind, setKind] = useState<NoteReference["kind"]>("conversation");
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<NoteLinkChoice[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true;
    setRows([]); setError(null);
    const timer = window.setTimeout(() => { void mailClient.noteLinkChoices(kind, query).then((rows) => { if (active) setRows(rows); }, (e) => { if (active) setError(errorMessage(e)); }); }, 150);
    return () => { active = false; window.clearTimeout(timer); };
  }, [kind, query]);
  return <Modal title="Link to…" onClose={onClose} dismissible={!busy}>
    <div className="modal-form">
      <label>Kind<select value={kind} onChange={(e) => setKind(e.target.value as NoteReference["kind"])}><option value="conversation">Conversation</option><option value="task">Task</option><option value="event">Event</option></select></label>
      <label>Search<input type="search" value={query} onChange={(e) => setQuery(e.target.value)} /></label>
      {kind === "event" ? <p className="context-status">Searches events already loaded on this device. Open Calendar to load other dates.</p> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <ul>{rows.map((row, index) => <li key={index}><button type="button" className="btn-link" disabled={busy} onClick={() => { setBusy(true); void onSelect(row).catch((e) => { setError(errorMessage(e)); }).finally(() => setBusy(false)); }}>{row.label}<small> · {row.detail}</small></button></li>)}</ul>
      {!rows.length ? <p role="status">No matching items on this device.</p> : null}
    </div>
  </Modal>;
}
