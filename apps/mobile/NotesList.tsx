import { useState } from "react";
import { mobile, type NoteView } from "./api";
import { useResource } from "./useResource";
import { parseNoteTargetKey, type NoteTarget } from "../../src/noteDrafts";

const views: { key: NoteView; label: string }[] = [
  { key: "recent", label: "Recent" }, { key: "starred", label: "Starred" }, { key: "trash", label: "Trash" },
];

export function NotesList({ revision, onOpen, onNew, query, setQuery, view, setView }: {
  revision: number;
  onOpen(target: NoteTarget): void;
  onNew(): void;
  query: string;
  setQuery(query: string): void;
  view: NoteView;
  setView(view: NoteView): void;
}) {
  const [retry, setRetry] = useState(0);
  const { data, error } = useResource(() => mobile.notes(view, query), [view, query, revision, retry]);
  return <section aria-label="Notes">
    <div className="section-heading"><h1>Notes</h1><button onClick={onNew}>New Note</button></div>
    <input type="search" aria-label="Search notes" placeholder="Search notes" value={query} onChange={event => setQuery(event.target.value)} />
    <div className="segments" aria-label="Note views">
      {views.map(({ key, label }) => <button key={key} aria-pressed={view === key} onClick={() => setView(key)}>{label}</button>)}
    </div>
    {error && <p role="alert">{error} <button onClick={() => setRetry(value => value + 1)}>Retry</button></p>}
    {!data && !error && <p role="status">Loading notes…</p>}
    {data?.length === 0 && <p className="muted">{query.trim() ? "No matching notes." : "No notes here yet."}</p>}
    <ul className="rows">
      {data?.map(note => {
        const snippet = "preview" in note ? note.preview : note.matchSnippet.replace(/[\u0001\u0002]/g, "");
        return <li key={note.id}>
          <button className="row-button" onClick={() => onOpen(parseNoteTargetKey(note.id))}>
            <strong>{note.title}</strong>{snippet && <span>{snippet}</span>}
          </button>
        </li>;
      })}
    </ul>
  </section>;
}
