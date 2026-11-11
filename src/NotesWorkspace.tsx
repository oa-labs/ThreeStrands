import { formatDateTime } from "./dateFormatting";
import { NoteLinkPicker } from "./NoteLinkPicker";
import { markdownEntityLink, parseNoteReference, type NoteDocument } from "./noteEntities";
import { changeNoteItem, NOTES_CHANGED, prepareNoteMutation } from "./noteMutations";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, CalendarDays, ChevronLeft, ChevronRight, CircleCheck, FilePlus, GitCompareArrows, Mail, NotebookText, Plus, RotateCcw, Search, Trash2, UserRound, X } from "lucide-react";
import type { CalendarOption, ContactProfile, Note, NoteLinks, NoteRelink, NoteSearchHit, NoteSearchScope, NoteSummary, ScheduleEvent } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage, logBackgroundFailure } from "./errors";
import { formatHistoryDate } from "./contactContext";
import { isEditableTarget } from "./commands";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";
import { useWorkspaceCommands } from "./workspaceCommands";
import { WorkspaceContextRail } from "./WorkspaceContextRail";
import { ContextRow, ContextSection, ContextSectionHeader } from "./ContextSections";
import { NoteChecklistSection } from "./NoteChecklistSection";
import { InlineConfirm } from "./InlineConfirm";
import { NoteEditor, type NoteEditorHandle, type NoteLinkSuggestion } from "./NoteEditor";
import { noteFiles } from "./noteAttachments";
import { noteTitleKey } from "./noteWikiLinks";
import { notePreview } from "./notePreview";
import { NewNoteDialog } from "./NewNoteDialog";
import { NoteConflictBanner } from "./NoteConflictBanner";
import { noteConflictsById } from "./noteConflicts";
import { noteTargetKey, parseNoteTargetKey, readLastNoteTarget, todayTarget, writeLastNoteTarget, type NoteTarget } from "./noteDrafts";
import { readNotesSearchState, writeNotesSearchState, type NotesListView } from "./noteSearchState";
import { formatNoteDate } from "./noteDates";
import { NoteMatchSnippet, noteLabel } from "./NoteMatchSnippet";
import { useNoteSyntaxMode } from "./noteSyntaxMode";
import { useNoteEditor } from "./useNoteEditor";
import { replicatedSyncConflicts, replicatedSyncDeviceRoster, type DeviceRosterEntry, type FrontierConflict } from "./replicatedSync";
import type { NotesRequest } from "./useWorkspaces";
import { formatShortcut } from "./shortcutLabels";
import { scrollBehavior } from "./useReaderActions";
import { NotesDayCalendar } from "./NotesDayCalendar";
import { CalendarAgenda } from "./CalendarAgenda";
import { parseCalendarDate } from "./DatePickerField";
import { addDays, dateInputValue } from "./calendarTime";

/** Most notes the Recent list shows. */
const RECENT_LIMIT = 200;
/** Most notes the Trash list shows. */
const TRASH_LIMIT = 200;
/** Most notes the `[[` picker offers. */
const LINK_SUGGESTIONS = 8;
/** Most of a note's links checked against saved contacts for People. */
const PEOPLE_LOOKUPS = 30;
/** Most notes a search lists. */
const SEARCH_LIMIT = 50;
const SEARCH_DELAY_MS = 150;
/** How long "Saved" stays after a save before it clears. */
const SAVED_SHOWN_MS = 2000;
const summaryTarget = (note: { id: string; dailyDate: string | null }): NoteTarget => note.dailyDate ? { kind: "daily", date: note.dailyDate } : { kind: "note", id: note.id };
const formatTime = (value: string) => formatDateTime(new Date(value));

/** Each linked kind's glyph in the Links section. */
const LINK_ICON = { conversation: <Mail size={ICON_SIZE.sm} />, task: <CircleCheck size={ICON_SIZE.sm} />, event: <CalendarDays size={ICON_SIZE.sm} /> } as const;

type Row = { key: string; target: NoteTarget; title: string; detail: ReactNode; date?: string; trashed?: boolean };

/** A `[[link]]` that names a day opens that day's note. */
const linkedDay = (title: string) => {
  const date = title.trim();
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return null;
  const [year, month, day] = date.split("-").map(Number);
  const parsed = new Date(year!, month! - 1, day);
  return parsed.getFullYear() === year && parsed.getMonth() === month! - 1 && parsed.getDate() === day ? date : null;
};

/** What a rename did to links elsewhere, in a sentence. */
export function relinkMessage({ updated, keptOldLink }: NoteRelink): string | null {
  const parts: string[] = [];
  if (updated) parts.push(`Updated links in ${updated} ${updated === 1 ? "note" : "notes"}.`);
  if (keptOldLink.length) {
    const names = keptOldLink.map((title) => linkedDay(title) ? formatNoteDate(title) : `“${title}”`).join(", ");
    parts.push(`${names} ${keptOldLink.length === 1 ? "keeps" : "keep"} the old link, because changing ${keptOldLink.length === 1 ? "it" : "them"} would make ${keptOldLink.length === 1 ? "it" : "them"} too long to sync or would settle a conflict.`);
  }
  return parts.length ? parts.join(" ") : null;
}

/**
 * Notes: the note list on the left, the open note in the middle, and its
 * context on the right. The list is read mode, where Space and Shift+Space
 * page the open note; the note is a writing surface entered with Enter or a
 * click and left with Escape. Trash sits at
 * the bottom of the list, and a trashed note opens read-only. The rail's
 * calendar opens any day's note, and a daily note lists that day's agenda,
 * where Add Note writes a meeting into it.
 */
export function NotesWorkspace({ request = null, onRequestConsumed, refreshKey = "", onOpenContact, onOpenEntity, onReturn, calendars, onOpenEvent, onAddNote }: {
  request?: NotesRequest | null;
  onOpenEntity?(href: string): Promise<void>;
  onReturn?(): void;
  /** Acknowledge the request after selecting its target. */
  onRequestConsumed?(id: number): void;
  refreshKey?: string;
  /** Opens a saved contact in Contacts, for the People section. */
  onOpenContact?(id: string): void;
  /** The user's calendars; a daily note lists its day's agenda when there are any. */
  calendars?: CalendarOption[];
  onOpenEvent?(event: ScheduleEvent): void;
  /** Add Note on an agenda event: records the meeting in its day's note. */
  onAddNote?(event: ScheduleEvent): void;
}) {
  const [target, setTarget] = useState<NoteTarget>(() => readLastNoteTarget() ?? todayTarget());
  const [initialSearch] = useState(readNotesSearchState);
  const [listView, setListView] = useState<NotesListView>(initialSearch.listView);
  const [includeTrash, setIncludeTrash] = useState(initialSearch.includeTrash);
  const [searchFocused, setSearchFocused] = useState(false);
  const [recent, setRecent] = useState<NoteSummary[]>([]);
  const [trashed, setTrashed] = useState<NoteSummary[]>([]);
  const [listError, setListError] = useState<string | null>(null);
  const [query, setQuery] = useState(initialSearch.query);
  const scope: NoteSearchScope = listView === "trash" ? "trash" : includeTrash ? "all" : "active";
  const [result, setResult] = useState<{ query: string; scope: NoteSearchScope; hits: NoteSearchHit[] } | null>(null);
  const searchCurrent = result?.query === query && result.scope === scope;
  const hits = query.trim() ? searchCurrent ? result.hits : [] : null;
  const [searchRevision, setSearchRevision] = useState(0);
  useEffect(() => { writeNotesSearchState({ query, listView, includeTrash }); }, [query, listView, includeTrash]);
  const [creating, setCreating] = useState(false);
  const [focusEditor, setFocusEditor] = useState(false);
  const [conflicts, setConflicts] = useState<Map<string, FrontierConflict[]>>(() => new Map());
  const [roster, setRoster] = useState<DeviceRosterEntry[]>([]);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [links, setLinks] = useState<{ noteId: string; links: NoteLinks } | null>(null);
  const [entityDocument, setEntityDocument] = useState<(NoteDocument & { noteId: string; version: string }) | null>(null);
  const [linkPicker, setLinkPicker] = useState(false);
  const [people, setPeople] = useState<ContactProfile[]>([]);
  const [relinkNotice, setRelinkNotice] = useState<string | null>(null);
  /** The note last saved here, so "Saved" confirms typing rather than greeting every note opened. */
  const [lastSave, setLastSave] = useState<{ noteId: string } | null>(null);
  // Each save is a new object, so a later save restarts the wait.
  useEffect(() => {
    if (!lastSave) return;
    const timer = window.setTimeout(() => setLastSave(null), SAVED_SHOWN_MS);
    return () => window.clearTimeout(timer);
  }, [lastSave]);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const editorPane = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const titleInput = useRef<HTMLInputElement>(null);
  const body = useRef<NoteEditorHandle>(null);
  const syntaxMode = useNoteSyntaxMode();
  const today = todayTarget();
  const todayKey = noteTargetKey(today);
  const selectedKey = noteTargetKey(target);
  // Files go to the open note: its id, which for a day not yet written in is that day's note id.
  const openKey = useRef(selectedKey);
  useEffect(() => { openKey.current = selectedKey; });
  const files = useMemo(() => noteFiles(() => openKey.current, setActionError), []);

  const loadRecent = useCallback(() => {
    void mailClient.listRecentNotes(RECENT_LIMIT).then((notes) => { setRecent(notes); setListError(null); }, (reason) => setListError(errorMessage(reason)));
  }, []);
  const loadTrash = useCallback(() => {
    void mailClient.listTrashedNotes(TRASH_LIMIT).then((notes) => { setTrashed(notes); setListError(null); }, (reason) => setListError(errorMessage(reason)));
  }, []);
  const loadConflicts = useCallback(() => {
    void replicatedSyncConflicts().then((found) => {
      const byNote = noteConflictsById(found);
      setConflicts(byNote);
      // Device names are only needed to describe a conflict.
      if (byNote.size) void replicatedSyncDeviceRoster().then(setRoster, logBackgroundFailure("Sync device names"));
    }, logBackgroundFailure("Note conflicts"));
  }, []);
  const showSaved = useCallback((note: Note) => {
    setRecent((current) => {
      const summary = current.find((item) => item.id === note.id);
      const updated: NoteSummary = { id: note.id, title: note.title, dailyDate: note.dailyDate, preview: notePreview(note.body), updatedAt: note.updatedAt, visitedAt: summary?.visitedAt ?? null, trashedAt: note.trashedAt };
      // A note keeps its place while it's being written in, so the list doesn't jump under the cursor.
      return summary ? current.map((item) => item.id === note.id ? updated : item) : [updated, ...current];
    });
  }, []);
  const select = useCallback((next: NoteTarget) => { setTarget(next); setConfirmingDelete(false); setActionError(null); setRelinkNotice(null); }, []);
  const linksVersion = useRef(0);
  const loadLinks = useCallback((noteId: string | null) => {
    const version = ++linksVersion.current;
    if (!noteId) { setLinks(null); return; }
    void mailClient.noteLinks(noteId).then((found) => { if (version === linksVersion.current) setLinks({ noteId, links: found }); }, logBackgroundFailure("Note links"));
  }, []);
  const openNoteId = useRef<string | null>(null);
  const onSaved = useCallback((note: Note, relink?: NoteRelink) => {
    showSaved(note);
    setSearchRevision((value) => value + 1);
    setLastSave({ noteId: note.id });
    loadLinks(openNoteId.current);
    if (!relink) return;
    setRelinkNotice(relinkMessage(relink));
    // Notes whose links followed the rename changed too.
    if (relink.updated) void mailClient.listRecentNotes(RECENT_LIMIT).then(setRecent, logBackgroundFailure("Recent notes"));
  }, [showSaved, loadLinks]);
  const editor = useNoteEditor(target, { onSaved, onReplaced: (note) => select({ kind: "note", id: note.id }), onRestored: () => loadTrash() });
  useEffect(() => { openNoteId.current = editor.note?.id ?? null; });
  const { refresh } = editor;
  const reloadAll = useCallback(() => {
    setSearchRevision((value) => value + 1);
    loadRecent();
    loadTrash();
    loadConflicts();
    loadLinks(openNoteId.current);
    files.retry();
    void refresh().catch(logBackgroundFailure("Note refresh"));
  }, [loadRecent, loadTrash, loadConflicts, loadLinks, refresh, files]);
  const noteId = editor.note?.id ?? null;
  useEffect(() => {
    let active = true;
    const version = editor.note?.updatedAt ?? "";
    setEntityDocument((current) => current?.noteId === noteId ? current : null);
    if (noteId) void mailClient.noteDocument(noteId).then((value) => { if (active) setEntityDocument({ ...value, noteId, version }); }, (e) => { if (active) setActionError(errorMessage(e)); });
    return () => { active = false; };
  }, [noteId, editor.note?.updatedAt]);
  useEffect(() => { const changed = () => reloadAll(); window.addEventListener(NOTES_CHANGED, changed); return () => window.removeEventListener(NOTES_CHANGED, changed); }, [reloadAll]);
  const pendingFocus = useRef<Extract<NotesRequest, { kind: "note" }> | null>(null);
  useEffect(() => { loadLinks(noteId); }, [noteId, loadLinks]);
  // People: the note's contact references, then its links whose titles are
  // saved contacts' names, up to PEOPLE_LOOKUPS lookups in all.
  const linkTitles = links?.noteId === noteId ? links.links.links : null;
  const contactReferences = useMemo(() => {
    if (entityDocument?.noteId !== noteId) return [];
    const seen = new Set<string>();
    return entityDocument.links.map((link) => link.reference).filter((reference) => reference.kind === "contact" && !seen.has(reference.id) && seen.add(reference.id));
  }, [entityDocument, noteId]);
  useEffect(() => {
    if (!linkTitles?.length && !contactReferences.length) { setPeople([]); return; }
    let active = true;
    const referenced = contactReferences.slice(0, PEOPLE_LOOKUPS).map((reference) => mailClient.resolveNoteContact(reference).then((id) => id ? mailClient.getContactProfile(id) : null));
    const named = (linkTitles ?? []).slice(0, PEOPLE_LOOKUPS - referenced.length).map((title) => mailClient.listContactProfiles(title, 5).then((found) => found.find((contact) => contact.displayName && noteTitleKey(contact.displayName) === noteTitleKey(title))));
    void Promise.all([...referenced, ...named])
      .then((found) => {
        if (!active) return;
        const unique = new Map<string, ContactProfile>();
        for (const contact of found) if (contact) unique.set(contact.id, contact);
        setPeople([...unique.values()]);
      }, logBackgroundFailure("Linked people"));
    return () => { active = false; };
  }, [linkTitles, contactReferences]);

  useEffect(() => { reloadAll(); }, [reloadAll, refreshKey]);
  // Another device's changes arrive with replicated sync.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let stop: (() => void) | undefined;
    let active = true;
    void listen("replicated-sync-status", reloadAll).then((unlisten) => { if (active) stop = unlisten; else unlisten(); }).catch(logBackgroundFailure("Notes sync listener"));
    return () => { active = false; stop?.(); };
  }, [reloadAll]);
  useEffect(() => {
    writeLastNoteTarget(target);
    void mailClient.recordNoteVisit(noteTargetKey(target)).catch(logBackgroundFailure("Note visit"));
  }, [target]);
  useEffect(() => {
    if (!request) return;
    setQuery("");
    setListView("notes");
    if (request.kind === "note") pendingFocus.current = request;
    select(request.kind === "today" ? todayTarget() : parseNoteTargetKey(request.noteId));
    onRequestConsumed?.(request.id);
  }, [request, select, onRequestConsumed]);
  useEffect(() => {
    const pending = pendingFocus.current;
    if (!pending || editor.loading || editor.note?.id !== pending.noteId || selectedKey !== pending.noteId) return;
    pendingFocus.current = null;
    requestAnimationFrame(() => {
      if (pending.markerId) body.current?.revealTask(pending.markerId);
      else if (pending.referenceHref) body.current?.revealReference(pending.referenceHref);
    });
  }, [request, selectedKey, editor.loading, editor.note]);

  useEffect(() => {
    if (!query.trim()) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void mailClient.searchNotes(query, SEARCH_LIMIT, scope).then((found) => { if (active) { setResult({ query, scope, hits: found }); setListError(null); } }, (reason) => { if (active) setListError(errorMessage(reason)); });
    }, SEARCH_DELAY_MS);
    return () => { active = false; window.clearTimeout(timer); };
  }, [query, scope, searchRevision]);

  const editorReady = !editor.loading && !editor.missing && !editor.loadError;
  useEffect(() => {
    if (!focusEditor || !editorReady) return;
    setFocusEditor(false);
    requestAnimationFrame(() => body.current?.focus());
  }, [focusEditor, editorReady]);

  const inTrash = Boolean(editor.note?.trashed);
  const noteConflicts = editor.note ? conflicts.get(editor.note.id) ?? [] : [];

  const list: Row[] = hits
    ? hits.map((hit) => ({ key: hit.id, target: summaryTarget(hit), title: noteLabel(hit), detail: <NoteMatchSnippet text={hit.matchSnippet} />, trashed: hit.trashed }))
    : listView === "trash"
      ? trashed.map((note): Row => ({ key: note.id, target: summaryTarget(note), title: noteLabel(note), detail: note.preview || "No text", date: note.trashedAt ?? note.updatedAt }))
      : [
        { key: todayKey, target: today, title: "Today", detail: formatNoteDate(today.date) },
        ...recent.filter((note) => note.id !== todayKey).map((note): Row => ({
          key: note.id,
          target: summaryTarget(note),
          title: noteLabel(note),
          detail: note.preview || "No text yet",
          date: note.updatedAt,
        })),
      ];
  const moveSelection = (direction: 1 | -1) => {
    const index = list.findIndex((row) => row.key === selectedKey);
    const next = list[Math.max(0, Math.min(list.length - 1, index + direction))];
    if (next) { select(next.target); rows.current.get(next.key)?.focus(); }
  };
  /** Pages the open note by most of a screen, as Space pages a message. */
  const pageNote = (direction: 1 | -1) => {
    const pane = editorPane.current;
    if (pane) pane.scrollBy({ top: direction * pane.clientHeight * 0.9, behavior: scrollBehavior() });
  };
  const focusSelectedRow = () => (rows.current.get(selectedKey) ?? rows.current.values().next().value)?.focus();
  const returnToList = () => {
    void editor.flush();
    focusSelectedRow();
  };
  /** Selects the row after `key` in the list, else the one before it, else Today. */
  const selectNeighbor = (key: string) => {
    const index = list.findIndex((row) => row.key === key);
    const next = index < 0 ? undefined : list[index + 1] ?? list[index - 1];
    select(next ? next.target : todayTarget());
  };
  const act = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setActionError(null);
    try { await action(); } catch (reason) { setActionError(errorMessage(reason)); } finally { setBusy(false); }
  };
  /** Opens the note a `[[link]]` names: that day's note for a date, the note with the title, or a new note with it. */
  const openLink = (title: string) => act(async () => {
    await editor.flush();
    const day = linkedDay(title);
    if (day) { setListView("notes"); select({ kind: "daily", date: day }); return; }
    const found = await mailClient.findNoteByTitle(title);
    if (found) { select(summaryTarget(found)); return; }
    const created = await mailClient.createNote(title, "");
    setListView("notes");
    showSaved(created);
    select({ kind: "note", id: created.id });
  });
  /** Opens `date`'s note, from the calendar or the day arrows, in the Notes list. */
  const [dayChosen, setDayChosen] = useState(0);
  const selectDay = (date: string) => { setListView("notes"); select({ kind: "daily", date }); setDayChosen((count) => count + 1); };
  /** Opens the note `days` days from the open daily note's; nothing while a regular note is open. */
  const shiftDay = (days: number) => {
    const day = target.kind === "daily" ? parseCalendarDate(target.date) : null;
    if (day) selectDay(dateInputValue(addDays(day, days)));
  };
  const searchLinks = useCallback(async (query: string): Promise<NoteLinkSuggestion[]> => {
    const found = await mailClient.suggestNoteTitles(query, LINK_SUGGESTIONS);
    return found.filter((note) => note.id !== openNoteId.current).map((note) => ({ title: note.title }));
  }, []);
  const searchEntities = useCallback(async (query: string) => {
    const [contacts, conversations] = await Promise.all([mailClient.noteLinkChoices("contact", query), mailClient.noteLinkChoices("conversation", query)]);
    return [...contacts.slice(0, LINK_SUGGESTIONS), ...conversations.slice(0, LINK_SUGGESTIONS)];
  }, []);
  const moveToTrash = () => act(async () => {
    const note = editor.note;
    if (!note || note.trashed) return;
    await editor.flush();
    if (editor.unsaved()) throw new Error("This note has changes that weren't saved. Resolve them before moving it to Trash.");
    await mailClient.trashNote(note.id);
    setSearchRevision((value) => value + 1);
    setRecent((current) => current.filter((item) => item.id !== note.id));
    loadTrash();
    // Today stays at the top of the list, so its note stays open, now in Trash.
    if (listView === "notes" && note.id !== todayKey) selectNeighbor(note.id);
    else await refresh();
  });
  const restore = () => act(async () => {
    const note = editor.note;
    if (!note) return;
    const restored = await mailClient.restoreNote(note.id);
    setSearchRevision((value) => value + 1);
    setTrashed((current) => current.filter((item) => item.id !== note.id));
    showSaved(restored);
    await refresh();
  });
  const deletePermanently = () => act(async () => {
    const note = editor.note;
    if (!note) return;
    await mailClient.deleteNote(note.id);
    setSearchRevision((value) => value + 1);
    setConfirmingDelete(false);
    setTrashed((current) => current.filter((item) => item.id !== note.id));
    // Today stays at the top of the list, so its day stays open, now without a note.
    if (note.id === todayKey) await refresh();
    else if (listView === "trash") selectNeighbor(note.id);
    else select(todayTarget());
  });

  const stale = editor.saveState.kind === "stale" ? editor.saveState : null;
  const daily = target.kind === "daily";
  const readOnly = Boolean(stale) || editor.loading || inTrash || noteConflicts.length > 0;
  useWorkspaceCommands({
    "notes.next": () => moveSelection(1),
    "notes.previous": () => moveSelection(-1),
    "notes.new": () => setCreating(true),
    "notes.search": () => requestAnimationFrame(() => search.current?.focus()),
    "notes.trash": () => void moveToTrash(),
    "notes.link": () => { if (!readOnly) setLinkPicker(true); },
    "notes.pageDown": () => pageNote(1),
    "notes.pageUp": () => pageNote(-1),
    "notes.write": () => { if (!readOnly) setFocusEditor(true); },
    "notes.previousDay": () => shiftDay(-1),
    "notes.nextDay": () => shiftDay(1),
    "notes.goToday": () => selectDay(todayTarget().date),
  });

  const entityItemsCurrent = entityDocument?.noteId === noteId && entityDocument?.version === editor.note?.updatedAt;
  const saved = !editor.loading && editor.saveState.kind !== "saving" && !editor.dirty && Boolean(editor.note) && lastSave?.noteId === editor.note?.id;
  const status = editor.loading ? "Loading note…" : editor.saveState.kind === "saving" ? "Saving…" : editor.dirty ? "Not saved yet" : saved ? "Saved" : "";
  // Contact references are listed under People rather than Links.
  const entityLinks = (entityDocument?.noteId === noteId ? entityDocument.links : []).filter((link, index, all) => link.reference.kind !== "contact" && all.findIndex((other) => other.href === link.href) === index);
  const editorTitle = daily ? formatNoteDate(target.date) : editor.text.title;

  return <section className="notes-workspace" aria-label="Notes" onKeyDown={(event) => {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || isEditableTarget(event.target)) return;
    if (event.key === "Escape" && onReturn) { event.preventDefault(); onReturn(); return; }
    if (!(event.target instanceof HTMLElement) || !event.target.closest(".notes-list")) return;
    // Space pages the open note from the list, rather than choosing the row again.
    if (event.code === "Space") { event.preventDefault(); event.stopPropagation(); pageNote(event.shiftKey ? -1 : 1); return; }
    if (!["j", "k", "ArrowDown", "ArrowUp"].includes(event.key)) return;
    event.preventDefault(); event.stopPropagation();
    moveSelection(["j", "ArrowDown"].includes(event.key) ? 1 : -1);
  }}>
    <header className="notes-header">
      <h1>Notes</h1>
      {onReturn ? <button type="button" className="btn-link" onClick={onReturn}>Back to Mail</button> : null}
      <div className="notes-header-actions">
        <button type="button" className="btn" title={`New note (${formatShortcut("Shift+n")})`} aria-keyshortcuts="Shift+N" onClick={() => setCreating(true)}><Plus size={ICON_SIZE.md} aria-hidden="true" />New Note</button>
      </div>
    </header>
    <div className="notes-workspace-body">
      <div className="notes-list-pane">
        <div className="notes-search">
          <label className={`search-box${!searchFocused && !query.trim() ? " idle" : ""}`}>
            <Search size={ICON_SIZE.md} aria-hidden="true" />
            <input ref={search} type="search" {...SEARCH_INPUT_ATTRIBUTES} aria-label="Search Notes" value={query} placeholder={listView === "trash" ? "Search Trash" : "Search notes"}
              data-shortcut-scope="search"
              onFocus={() => setSearchFocused(true)} onBlur={() => setSearchFocused(false)}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") { event.preventDefault(); search.current?.blur(); focusSelectedRow(); return; }
                if (event.key !== "Escape") return;
                event.preventDefault(); event.stopPropagation();
                if (query) setQuery("");
                requestAnimationFrame(() => { focusSelectedRow(); search.current?.blur(); });
              }} />
            <span className="search-input-actions">
              <button type="button" className="btn-icon btn-icon-sm search-close" aria-label="Close Search" title="Close Search (Esc)"
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => { setQuery(""); requestAnimationFrame(focusSelectedRow); }}><X size={ICON_SIZE.sm} /></button>
              {!searchFocused && !query.trim() ? <kbd>/</kbd> : null}
            </span>
          </label>
        </div>
        {listView === "notes" ? <label className="notes-search-scope"><input type="checkbox" checked={includeTrash} onChange={(event) => setIncludeTrash(event.target.checked)} />Include Trash</label> : <p className="notes-search-scope">Searching Trash only</p>}
        {listError ? <p className="form-error" role="alert">{listError}</p> : null}
        {listView === "trash" && !hits ? <h2 className="eyebrow notes-list-heading">Trash</h2> : null}
        <div className="notes-list" role="list" aria-label={hits ? "Matching notes" : listView === "trash" ? "Trash" : "Notes"}>
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
              <span className="notes-list-title-line">
                <strong className="notes-list-title">{row.title}</strong>
                {row.trashed ? <span className="eyebrow">Trash</span> : null}
                {conflicts.has(row.key) ? <span className="notes-list-conflict" title="Changed on two devices"><GitCompareArrows size={ICON_SIZE.sm} aria-hidden="true" /><span className="sr-only">Changed on two devices</span></span> : null}
                {row.date ? <time className="notes-list-date" dateTime={row.date}>{formatHistoryDate(row.date)}</time> : null}
              </span>
              <span className="notes-list-detail">{row.detail}</span>
            </button>
          </div>)}
        </div>
        {hits && searchCurrent && !hits.length ? <p className="context-status">No notes match “{query.trim()}”.</p> : null}
        {!hits && listView === "trash" && !trashed.length ? <p className="context-status">Trash is empty.</p> : null}
        <div className="notes-list-footer">
          {listView === "notes"
            ? <button type="button" className="btn btn-sm" onClick={() => { setListView("trash"); loadTrash(); }}>
              <Trash2 size={ICON_SIZE.sm} aria-hidden="true" />Trash{trashed.length ? <span className="notes-trash-count">{trashed.length}</span> : null}
            </button>
            : <button type="button" className="btn btn-sm" onClick={() => { setListView("notes"); }}>
              <ArrowLeft size={ICON_SIZE.sm} aria-hidden="true" />Back to Notes
            </button>}
        </div>
      </div>
      <div ref={editorPane} className="notes-editor-pane" data-shortcut-scope="compose" onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) void editor.flush();
      }}>
        {editor.loadError ? <p className="form-error" role="alert">{editor.loadError}</p>
          : editor.missing ? <p className="context-status">This note was deleted.</p>
          : <>
            {stale ? <div className="notes-stale" role="alert">
              <p>{stale.current?.trashed ? "This note was moved to Trash on another device after you started typing. Your text is kept here until you choose."
                : stale.current ? "This note changed on another device after you started typing. Your text is kept here until you choose."
                : "This note was deleted on another device. Your text is kept here until you choose."}</p>
              {stale.error ? <p className="form-error">{stale.error}</p> : null}
              <div className="notes-stale-actions">
                <button type="button" className="btn btn-primary" onClick={() => void editor.keepMine()}>{stale.current?.trashed ? "Restore and Keep My Text" : stale.current ? "Keep My Version" : "Keep as New Note"}</button>
                <button type="button" className="btn" onClick={editor.takeTheirs}>{stale.current && !stale.current.trashed ? "Use Other Version" : "Discard My Text"}</button>
              </div>
            </div> : null}
            {editor.note && !stale && noteConflicts.length ? <NoteConflictBanner key={editor.note.id} note={editor.note} title={editorTitle} conflicts={noteConflicts} roster={roster}
              onResolved={reloadAll}
              onKeptBoth={showSaved} /> : null}
            {inTrash && !noteConflicts.length ? <div className="notes-trashed" role="region" aria-label="Note in Trash">
              <p>This note is in Trash. Restore it to edit it.</p>
              <div className="notes-stale-actions">
                <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void restore()}><RotateCcw size={ICON_SIZE.md} aria-hidden="true" />Restore</button>
                <button type="button" className="btn btn-danger" disabled={busy} onClick={() => setConfirmingDelete(true)}>Delete Permanently</button>
              </div>
              {confirmingDelete ? <InlineConfirm ariaLabel="Delete note permanently" stacked focusCancel cancelLabel="Cancel" onCancel={() => setConfirmingDelete(false)} disabled={busy}
                title={`Delete “${editorTitle}” permanently?`}
                actions={[{ label: "Delete Permanently", className: "btn-danger", onClick: () => void deletePermanently() }]}>
                It is removed from every device that syncs with this one. This can't be undone.
              </InlineConfirm> : null}
            </div> : null}
            {actionError ? <p className="form-error" role="alert">{actionError}</p> : null}
            <div className="notes-editor-heading">
              {daily ? <div className="notes-day-title">
                <h2 className="notes-editor-title">{editorTitle}</h2>
                <button type="button" className="btn-icon btn-icon-sm" aria-label="Previous Day" title={`Previous Day (${formatShortcut("-")})`} aria-keyshortcuts="-" onClick={() => shiftDay(-1)}><ChevronLeft size={ICON_SIZE.sm} aria-hidden="true" /></button>
                <button type="button" className="btn-icon btn-icon-sm" aria-label="Next Day" title={`Next Day (${formatShortcut("=")})`} aria-keyshortcuts="=" onClick={() => shiftDay(1)}><ChevronRight size={ICON_SIZE.sm} aria-hidden="true" /></button>
              </div>
                : <input ref={titleInput} className="notes-editor-title" aria-label="Title" value={editor.text.title} readOnly={readOnly} maxLength={200}
                  {...SEARCH_INPUT_ATTRIBUTES}
                  onChange={(event) => editor.edit({ title: event.target.value })}
                  onBlur={() => void editor.flush()}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); returnToList(); }
                    if (event.key === "Enter") { event.preventDefault(); body.current?.focus(); }
                  }} />}
              {editor.saveState.kind === "error" ? null : <p className="notes-save-status" role="status">{status}</p>}
              {editor.note && !inTrash ? <button type="button" className="btn btn-icon" title="Move to Trash (#)" aria-label="Move to Trash" aria-keyshortcuts="#" disabled={busy || Boolean(stale)} onClick={() => void moveToTrash()}><Trash2 size={ICON_SIZE.lg} aria-hidden="true" /></button> : null}
            </div>
            {relinkNotice ? <p className="notes-relink-notice" role="status">{relinkNotice}</p> : null}
            <NoteEditor ref={body} noteKey={selectedKey} value={editor.text.body} label="Note text" readOnly={readOnly} mode={syntaxMode} files={files}
              placeholder={daily ? "Write about the day" : "Write in Markdown"}
              onChange={(value) => editor.edit({ body: value })} onLeave={returnToList}
              onOpenEntity={(href) => void act(async () => { if (!parseNoteReference(href)) throw new Error("Invalid note reference"); await editor.flush(); if (editor.unsaved()) throw new Error("Save or resolve this note before opening its link."); await onOpenEntity?.(href); })}
              onSearchLinks={searchLinks} onSearchEntities={searchEntities} onOpenLink={(title) => void openLink(title)} />
            {editor.saveState.kind === "error" ? <p className="form-error" role="alert">{editor.saveState.message}</p> : null}
          </>}
      </div>
      <WorkspaceContextRail label="Note context" onReturn={focusSelectedRow}>
        <NotesDayCalendar selected={daily ? target.date : null} today={today.date} onSelect={selectDay} revision={searchRevision} reveal={dayChosen} />
        {daily && calendars?.length && onOpenEvent ? <CalendarAgenda key={target.date} today={parseCalendarDate(target.date) ?? new Date()} day="chosen" id="notes-agenda" showDate={false}
          calendars={calendars} onOpenEvent={onOpenEvent} onAddNote={onAddNote} /> : null}
        <ContextSectionHeader title="Note details" icon={<NotebookText size={ICON_SIZE.xs} />} />
        {entityDocument?.warning ? <p role="alert" className="form-error">{entityDocument.warning}</p> : null}
        {!entityLinks.length && !readOnly ? <section className="note-links-empty" aria-label="Links">
          <button type="button" className="btn-link" onClick={() => setLinkPicker(true)}><span className="context-row-glyph" aria-hidden="true"><Plus size={ICON_SIZE.sm} /></span>Link contact or conversation</button>
        </section> : null}
        {entityLinks.length ? <ContextSection id="note-links" title="Links" count={entityLinks.length}
          actions={!readOnly ? <button type="button" className="btn-icon btn-icon-sm" aria-label="Link to…" title="Link to a contact, conversation, task, or event" onClick={() => setLinkPicker(true)}><Plus size={ICON_SIZE.sm} aria-hidden="true" /></button> : null}
          rows={entityLinks.map((link) => <ContextRow key={link.href} icon={LINK_ICON[link.reference.kind as keyof typeof LINK_ICON]} title={link.label} detail={link.reference.account}
            onActivate={() => void act(async () => { await editor.flush(); if (editor.unsaved()) throw new Error("Resolve your unsaved note first."); await onOpenEntity?.(link.href); })} />)} /> : null}
        {entityDocument?.noteId === noteId && entityDocument?.items.length && !inTrash ? <NoteChecklistSection key={noteId} items={entityDocument.items}
          disabled={readOnly || busy || !entityItemsCurrent} onChange={(item, patch) => void act(async () => { await changeNoteItem(noteId!, item.id, editor.note!.updatedAt, patch); })} /> : null}
        {editor.note && links?.noteId === editor.note.id ? <>
          {links.links.backlinks.length ? <ContextSection id="notes-backlinks" title="Backlinks" count={links.links.backlinks.length}
            rows={links.links.backlinks.map((link, index) => <ContextRow key={`${link.noteId}-${index}`} icon={<NotebookText size={ICON_SIZE.sm} />}
              title={noteLabel(link)} detail={link.context} onActivate={() => select(summaryTarget({ id: link.noteId, dailyDate: link.dailyDate }))} />)} /> : null}
          {links.links.unlinked.length && !inTrash ? <ContextSection id="notes-unlinked" title="Unlinked" count={links.links.unlinked.length}
            rows={links.links.unlinked.map((title) => <ContextRow key={title} icon={<FilePlus size={ICON_SIZE.sm} />}
              title={linkedDay(title) ? formatNoteDate(title) : title} detail={linkedDay(title) ? "No note for this day yet" : "Create this note"} disabled={busy} onActivate={() => void openLink(title)} />)} /> : null}
          {people.length && onOpenContact ? <ContextSection id="notes-people" title="People" count={people.length}
            rows={people.map((contact) => <ContextRow key={contact.id} icon={<UserRound size={ICON_SIZE.sm} />}
              title={contact.displayName ?? contact.addresses[0]} detail={contact.company ?? contact.addresses[0]} onActivate={() => onOpenContact(contact.id)} />)} /> : null}
        </> : null}
        {editor.note ? <dl className="notes-details">
          <div><dt>Created</dt><dd><time dateTime={editor.note.createdAt}>{formatTime(editor.note.createdAt)}</time></dd></div>
          {Date.parse(editor.note.updatedAt) !== Date.parse(editor.note.createdAt) ? <div><dt>Updated</dt><dd><time dateTime={editor.note.updatedAt}>{formatTime(editor.note.updatedAt)}</time></dd></div> : null}
          {editor.note.trashedAt ? <div><dt>Trashed</dt><dd><time dateTime={editor.note.trashedAt}>{formatTime(editor.note.trashedAt)}</time></dd></div> : null}
        </dl> : <p className="context-status">{daily ? "This day's note starts when you write in it." : "Select a note to see its details."}</p>}
      </WorkspaceContextRail>
    </div>
    {linkPicker ? <NoteLinkPicker onClose={() => setLinkPicker(false)} onSelect={async (choice) => {
      // Stabilize a provisional target before storing a durable task link.
      if (choice.reference.kind === "task" && choice.reference.note) {
        await prepareNoteMutation(choice.reference.note);
        const target = await mailClient.getNote(choice.reference.note);
        if (!target) throw new Error("Unavailable on this device");
        const doc = await mailClient.noteDocument(target.id);
        const item = doc.items.find((item) => item.id === choice.reference.id && item.task);
        if (!item) throw new Error("Unavailable on this device");
        if (!item.persisted) await changeNoteItem(target.id, item.id, target.updatedAt, { checked: item.checked });
      }
      body.current?.insert(markdownEntityLink(choice.label, choice.reference));
      setLinkPicker(false);
    }} /> : null}
    {creating ? <NewNoteDialog onClose={() => setCreating(false)} onCreated={(note) => {
      setCreating(false);
      setListView("notes");
      showSaved(note);
      select({ kind: "note", id: note.id });
      setFocusEditor(true);
    }} /> : null}
  </section>;
}
