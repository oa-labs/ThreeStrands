import { CalendarDays, ListChecks, Mail, UserRound } from "lucide-react";
import { useEffect, useState } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import type { NoteReference } from "./noteEntities";
import { noteLinkDetail, type NoteLinkChoice } from "./noteLinkChoices";
export type { NoteLinkChoice } from "./noteLinkChoices";

const KIND_ICON = { contact: UserRound, conversation: Mail, task: ListChecks, event: CalendarDays } as const;

export function NoteLinkPicker({ onClose, onSelect }: { onClose(): void; onSelect(choice: NoteLinkChoice): Promise<void> }) {
  const [kind, setKind] = useState<NoteReference["kind"]>("conversation");
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<NoteLinkChoice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true;
    setRows(null); setError(null);
    const timer = window.setTimeout(() => { void mailClient.noteLinkChoices(kind, query).then((rows) => { if (active) setRows(rows); }, (e) => { if (active) { setRows([]); setError(errorMessage(e)); } }); }, 150);
    return () => { active = false; window.clearTimeout(timer); };
  }, [kind, query]);
  return <Modal title="Link to…" onClose={onClose} dismissible={!busy}>
    <div className="modal-form">
      <label>Kind<select value={kind} onChange={(e) => setKind(e.target.value as NoteReference["kind"])}><option value="contact">Contact</option><option value="conversation">Conversation</option><option value="task">Task</option><option value="event">Event</option></select></label>
      <label>Search<input type="search" value={query} onChange={(e) => setQuery(e.target.value)} /></label>
      {kind === "event" ? <p className="context-status">Searches events already loaded on this device. Open Calendar to load other dates.</p> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {rows?.length ? <ul className="note-link-picker-list" aria-label="Matches">{rows.map((row, index) => {
        const Icon = KIND_ICON[row.reference.kind];
        const detail = noteLinkDetail(row);
        return <li key={index}><button type="button" className="note-link-picker-option" disabled={busy} title={detail ? `${row.label}\n${detail}` : row.label} onClick={() => { setBusy(true); void onSelect(row).catch((e) => { setError(errorMessage(e)); }).finally(() => setBusy(false)); }}>
          <span className="note-autocomplete-icon" aria-hidden="true"><Icon size={ICON_SIZE.sm} /></span>
          <span className="note-autocomplete-text"><span>{row.label}</span>{detail ? <small>{detail}</small> : null}</span>
        </button></li>;
      })}</ul> : null}
      {rows === null ? <p className="note-autocomplete-status" role="status">Searching…</p> : !rows.length && !error ? <p className="note-autocomplete-status" role="status">No matching items on this device.</p> : null}
    </div>
  </Modal>;
}
