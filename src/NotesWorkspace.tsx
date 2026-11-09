import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { Plus } from "lucide-react";
import type { Note, NoteSearchHit, NoteSummary } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage, logBackgroundFailure } from "./errors";
import { formatHistoryDate } from "./contactContext";
import { isEditableTarget } from "./commands";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";
import { useWorkspaceCommands } from "./workspaceCommands";
import { WorkspaceContextRail } from "./WorkspaceContextRail";
import { ContextSectionHeader } from "./ContextSections";
import { NoteEditor } from "./NoteEditor";
import { NewNoteDialog } from "./NewNoteDialog";
import { noteTargetKey, readLastNoteTarget, todayTarget, writeLastNoteTarget, type NoteTarget } from "./noteDrafts";
import { useNoteEditor } from "./useNoteEditor";
import type { NotesRequest } from "./useWorkspaces";

/** Most notes the Recent list shows. */
const RECENT_LIMIT = 200;
/** Most notes a search lists. */
const SEARCH_LIMIT = 50;
const SEARCH_DELAY_MS = 150;
const MATCH_START = "\u0001";
const MATCH_END = "\u0002";

/** A daily note's date as people read it: "Monday, November 9, 2026". */
export function formatNoteDate(date: string): string {
  const [year, month, day] = date.split("-").map(Number);
  return new Date(year, month - 1, day).toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" });
}

const noteLabel = (note: { title: string; dailyDate: string | null }) => note.dailyDate ? formatNoteDate(note.dailyDate) : note.title;

function MatchSnippet({ text }: { text: string }) {
  const [before, ...matches] = text.split(MATCH_START);
  return <>{before}{matches.map((segment, index) => {
    const [match, ...rest] = segment.split(MATCH_END);
    return <span key={index}><mark>{match}</mark>{rest.join(MATCH_END)}</span>;
  })}</>;
}

type Row = { key: string; target: NoteTarget; title: string; detail: ReactNode; date?: string };

/**
 * Notes: the note list on the left, the open note in the middle, and its
 * context on the right. The list is read mode; the note is a writing
 * surface entered with Enter or a click and left with Escape.
 */
export function NotesWorkspace({ request = null, refreshKey = "" }: { request?: NotesRequest | null; refreshKey?: string }) {
  const [target, setTarget] = useState<NoteTarget>(() => readLastNoteTarget() ?? todayTarget());
  const [recent, setRecent] = useState<NoteSummary[]>([]);
  const [listError, setListError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<NoteSearchHit[] | null>(null);
  const [creating, setCreating] = useState(false);
  const [focusEditor, setFocusEditor] = useState(false);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const search = useRef<HTMLInputElement>(null);
  const titleInput = useRef<HTMLInputElement>(null);
  const body = useRef<HTMLTextAreaElement>(null);
  const today = todayTarget();
  const todayKey = noteTargetKey(today);
  const selectedKey = noteTargetKey(target);

  const loadRecent = useCallback(() => {
    void mailClient.listRecentNotes(RECENT_LIMIT).then((notes) => { setRecent(notes); setListError(null); }, (reason) => setListError(errorMessage(reason)));
  }, []);
  const showSaved = useCallback((note: Note) => {
    setRecent((current) => {
      const summary = current.find((item) => item.id === note.id);
      const updated: NoteSummary = { id: note.id, title: note.title, dailyDate: note.dailyDate, preview: note.body.split(/\s+/).filter(Boolean).join(" ").slice(0, 140), updatedAt: note.updatedAt, visitedAt: summary?.visitedAt ?? null, trashedAt: note.trashedAt };
      // A note keeps its place while it's being written in, so the list doesn't jump under the cursor.
      return summary ? current.map((item) => item.id === note.id ? updated : item) : [updated, ...current];
    });
  }, []);
  const select = useCallback((next: NoteTarget) => setTarget(next), []);
  const editor = useNoteEditor(target, { onSaved: showSaved, onReplaced: (note) => select({ kind: "note", id: note.id }) });
  const { refresh } = editor;

  useEffect(() => { loadRecent(); }, [loadRecent, refreshKey]);
  useEffect(() => { void refresh().catch(logBackgroundFailure("Note refresh")); }, [refresh, refreshKey]);
  // Another device's changes arrive with replicated sync.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let stop: (() => void) | undefined;
    let active = true;
    const reload = () => { loadRecent(); void refresh().catch(logBackgroundFailure("Note refresh")); };
    void listen("replicated-sync-status", reload).then((unlisten) => { if (active) stop = unlisten; else unlisten(); }).catch(logBackgroundFailure("Notes sync listener"));
    return () => { active = false; stop?.(); };
  }, [loadRecent, refresh]);
  useEffect(() => {
    writeLastNoteTarget(target);
    void mailClient.recordNoteVisit(noteTargetKey(target)).catch(logBackgroundFailure("Note visit"));
  }, [target]);
  useEffect(() => { if (request?.kind === "today") select(todayTarget()); }, [request, select]);

  useEffect(() => {
    if (!query.trim()) { setHits(null); return; }
    let active = true;
    const timer = window.setTimeout(() => {
      void mailClient.searchNotes(query, SEARCH_LIMIT).then((found) => { if (active) setHits(found); }, (reason) => { if (active) setListError(errorMessage(reason)); });
    }, SEARCH_DELAY_MS);
    return () => { active = false; window.clearTimeout(timer); };
  }, [query]);

  const editorReady = !editor.loading && !editor.missing && !editor.loadError;
  useEffect(() => {
    if (!focusEditor || !editorReady) return;
    setFocusEditor(false);
    requestAnimationFrame(() => body.current?.focus());
  }, [focusEditor, editorReady]);

  useWorkspaceCommands({
    "notes.new": () => setCreating(true),
    "notes.search": () => requestAnimationFrame(() => search.current?.focus()),
  });

  const list: Row[] = hits
    ? hits.map((hit) => ({ key: hit.id, target: hit.dailyDate ? { kind: "daily", date: hit.dailyDate } : { kind: "note", id: hit.id }, title: noteLabel(hit), detail: <MatchSnippet text={hit.matchSnippet} /> }))
    : [
      { key: todayKey, target: today, title: "Today", detail: formatNoteDate(today.date) },
      ...recent.filter((note) => note.id !== todayKey).map((note): Row => ({
        key: note.id,
        target: note.dailyDate ? { kind: "daily", date: note.dailyDate } : { kind: "note", id: note.id },
        title: noteLabel(note),
        detail: note.preview || "No text yet",
        date: note.updatedAt,
      })),
    ];
  const returnToList = () => {
    void editor.flush();
    (rows.current.get(selectedKey) ?? rows.current.values().next().value)?.focus();
  };
  const stale = editor.saveState.kind === "stale" ? editor.saveState : null;
  const daily = target.kind === "daily";
  const status = editor.loading ? "Loading note…" : editor.saveState.kind === "saving" ? "Saving…" : editor.dirty ? "Not saved yet" : editor.note ? "Saved" : "";

  return <section className="notes-workspace" aria-label="Notes" onKeyDown={(event) => {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || isEditableTarget(event.target)) return;
    if (!["j", "k", "ArrowDown", "ArrowUp"].includes(event.key)) return;
    if (!(event.target instanceof HTMLElement) || !event.target.closest(".notes-list")) return;
    event.preventDefault(); event.stopPropagation();
    const index = list.findIndex((row) => row.key === selectedKey);
    const next = list[Math.max(0, Math.min(list.length - 1, index + (["j", "ArrowDown"].includes(event.key) ? 1 : -1)))];
    if (next) { select(next.target); rows.current.get(next.key)?.focus(); }
  }}>
    <header className="notes-header">
      <h1>Notes</h1>
      <div className="notes-header-actions">
        <button type="button" className="btn" title="New note (Shift+N)" aria-keyshortcuts="Shift+N" onClick={() => setCreating(true)}><Plus size={ICON_SIZE.sm} />New Note</button>
      </div>
    </header>
    <div className="notes-workspace-body">
      <div className="notes-list-pane">
        <div className="notes-search">
          <label><span className="sr-only">Search Notes</span><input ref={search} type="search" {...SEARCH_INPUT_ATTRIBUTES} value={query} placeholder="Search notes"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== "Escape") return;
              event.preventDefault(); event.stopPropagation();
              if (query) setQuery(""); else rows.current.get(selectedKey)?.focus();
            }} /></label>
          <kbd aria-hidden="true">/</kbd>
        </div>
        {listError ? <p className="form-error" role="alert">{listError}</p> : null}
        <div className="notes-list" role="list" aria-label={hits ? "Matching notes" : "Notes"}>
          {list.map((row) => <div key={row.key} role="listitem" className={`notes-list-item${row.key === selectedKey ? " selected" : ""}`}>
            <button type="button" className="notes-list-select" aria-current={row.key === selectedKey ? "true" : undefined}
              ref={(node) => { if (node) rows.current.set(row.key, node); else rows.current.delete(row.key); }}
              onClick={() => select(row.target)}
              onKeyDown={(event) => {
                // Enter takes the note into writing; other controls keep native Enter.
                if (event.key !== "Enter" || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
                event.preventDefault();
                select(row.target);
                setFocusEditor(true);
              }}>
              <span className="notes-list-title-line"><strong className="notes-list-title">{row.title}</strong>{row.date ? <time className="notes-list-date" dateTime={row.date}>{formatHistoryDate(row.date)}</time> : null}</span>
              <span className="notes-list-detail">{row.detail}</span>
            </button>
          </div>)}
        </div>
        {hits && !hits.length ? <p className="context-status">No notes match “{query.trim()}”.</p> : null}
      </div>
      <div className="notes-editor-pane" data-shortcut-scope="compose" onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) void editor.flush();
      }}>
        {editor.loadError ? <p className="form-error" role="alert">{editor.loadError}</p>
          : editor.missing ? <p className="context-status">This note was deleted on another device.</p>
          : <>
            {stale ? <div className="notes-stale" role="alert">
              <p>{stale.current ? "This note changed on another device after you started typing. Your text is kept here until you choose." : "This note was deleted on another device. Your text is kept here until you choose."}</p>
              <div className="notes-stale-actions">
                <button type="button" className="btn btn-primary" onClick={() => void editor.keepMine()}>{stale.current ? "Keep My Version" : "Keep as New Note"}</button>
                <button type="button" className="btn" onClick={editor.takeTheirs}>{stale.current ? "Use Other Version" : "Discard My Text"}</button>
              </div>
            </div> : null}
            {daily ? <h2 className="notes-editor-title">{formatNoteDate(target.date)}</h2>
              : <input ref={titleInput} className="notes-editor-title" aria-label="Title" value={editor.text.title} readOnly={Boolean(stale) || editor.loading} maxLength={200}
                {...SEARCH_INPUT_ATTRIBUTES}
                onChange={(event) => editor.edit({ title: event.target.value })}
                onKeyDown={(event) => {
                  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); returnToList(); }
                  if (event.key === "Enter") { event.preventDefault(); body.current?.focus(); }
                }} />}
            <NoteEditor ref={body} value={editor.text.body} label="Note text" readOnly={Boolean(stale) || editor.loading}
              placeholder={daily ? "Write about the day" : "Write in Markdown"}
              onChange={(value) => editor.edit({ body: value })} onLeave={returnToList} />
            {editor.saveState.kind === "error" ? <p className="form-error" role="alert">{editor.saveState.message}</p>
              : <p className="notes-save-status" role="status">{status}</p>}
          </>}
      </div>
      <WorkspaceContextRail label="Note context" onReturn={() => rows.current.get(selectedKey)?.focus()}>
        <ContextSectionHeader title="Note details" />
        {editor.note ? <dl className="notes-details">
          <div><dt>Created</dt><dd>{new Date(editor.note.createdAt).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</dd></div>
          <div><dt>Changed</dt><dd>{new Date(editor.note.updatedAt).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</dd></div>
        </dl> : <p className="context-status">{daily ? "This day's note starts when you write in it." : "Select a note to see its details."}</p>}
      </WorkspaceContextRail>
    </div>
    {creating ? <NewNoteDialog onClose={() => setCreating(false)} onCreated={(note) => {
      setCreating(false);
      showSaved(note);
      select({ kind: "note", id: note.id });
      setFocusEditor(true);
    }} /> : null}
  </section>;
}
