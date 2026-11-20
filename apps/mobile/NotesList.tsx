import { useRef, useState } from "react";
import { Search, X } from "lucide-react";
import { NoteMatchSnippet, noteLabel } from "../../src/NoteMatchSnippet";
import { notePlainText } from "../../src/notePreview";
import { parseNoteTargetKey, type NoteTarget } from "../../src/noteDrafts";
import { ActionsMenu } from "./ActionsMenu";
import { mobile, type NoteView } from "./api";
import { useResource } from "./useResource";

export function NotesList({ revision, onOpen, query, setQuery, view, setView, onTrash }: {
  revision: number;
  onOpen(target: NoteTarget): void;
  query: string;
  setQuery(query: string): void;
  view: NoteView;
  setView(view: Exclude<NoteView, "trash">): void;
  onTrash(): void;
}) {
  const [retry, setRetry] = useState(0);
  const search = useRef<HTMLInputElement>(null);
  const { data, error } = useResource(async () => ({
    view, query, notes: await mobile.notes(view, query),
  }), [view, query, revision, retry]);
  const notes = data?.view === view && data.query === query ? data.notes : undefined;
  return <section aria-label={view === "trash" ? "Trashed notes" : "Notes"}>
    <div className="note-search">
      <Search size={19} aria-hidden="true" />
      <input ref={search} type="search" aria-label="Search notes" placeholder="Search notes" value={query}
        onChange={event => setQuery(event.target.value)} />
      {query && <button className="icon-button" aria-label="Clear search" onClick={() => {
        setQuery("");
        search.current?.focus();
      }}><X size={18} aria-hidden="true" /></button>}
    </div>
    {view !== "trash" && <div className="list-controls">
      <div className="segments" aria-label="Note views">
        <button aria-pressed={view === "recent"} onClick={() => setView("recent")}>Recent</button>
        <button aria-pressed={view === "starred"} onClick={() => setView("starred")}>Starred</button>
      </div>
      <ActionsMenu label="Notes options"><button onClick={onTrash}>Trash</button></ActionsMenu>
    </div>}
    {error && <p role="alert">{error} <button onClick={() => setRetry(value => value + 1)}>Retry</button></p>}
    {!notes && !error && <p role="status">Loading notes…</p>}
    {notes?.length === 0 && <p className="muted">{query.trim() ? "No matching notes." : "No notes here yet."}</p>}
    <ul className="rows">
      {notes?.map(note => {
        const snippet = "preview" in note ? note.preview : notePlainText(note.matchSnippet);
        return <li key={note.id}>
          <button className="row-button" onClick={() => onOpen(parseNoteTargetKey(note.id))}>
            <strong>{noteLabel(note)}</strong>
            {snippet && <span className="row-excerpt">{"preview" in note ? snippet : <NoteMatchSnippet text={snippet} />}</span>}
          </button>
        </li>;
      })}
    </ul>
  </section>;
}
