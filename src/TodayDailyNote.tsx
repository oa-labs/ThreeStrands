import { useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type Ref } from "react";
import { NoteEditor, type NoteEditorHandle } from "./NoteEditor";
import { NoteConflictBanner } from "./NoteConflictBanner";
import { useNoteEditor } from "./useNoteEditor";
import { useNoteSyntaxMode } from "./noteSyntaxMode";
import { noteFiles } from "./noteAttachments";
import { notesChanged, useNoteChanges } from "./noteMutations";
import { noteConflictsById } from "./noteConflicts";
import { replicatedSyncConflicts, replicatedSyncDeviceRoster, type DeviceRosterEntry, type FrontierConflict } from "./replicatedSync";
import { errorMessage, logBackgroundFailure } from "./errors";
import { mailClient } from "./data/client";
import { localDay } from "./todayDayChoice";
import { formatNoteDate } from "./noteDates";
import { parseNoteReference } from "./noteEntities";
import { formatShortcut } from "./shortcutLabels";

export type TodayDailyNoteHandle = { focus(): void; openInNotes(): Promise<void> };

/** The same daily note, draft recovery, and guarded save path as Notes. Opening it never writes. */
export function TodayDailyNote({ ref, date, active, refreshKey, onLeave, onOpenNote, onOpenEntity }: {
  ref?: Ref<TodayDailyNoteHandle>;
  date: string;
  active: boolean;
  refreshKey: string;
  onLeave(): void;
  onOpenNote?(id: string): void;
  onOpenEntity?(href: string): Promise<void>;
}) {
  const noteId = `daily:${date}`;
  const body = useRef<NoteEditorHandle>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [conflicts, setConflicts] = useState<FrontierConflict[] | null>(null);
  const [conflictError, setConflictError] = useState<string | null>(null);
  const [roster, setRoster] = useState<DeviceRosterEntry[]>([]);
  const [saved, setSaved] = useState(false);
  const syntax = useNoteSyntaxMode();
  const files = useMemo(() => noteFiles(() => noteId, setActionError), [noteId]);
  const onSaved = useCallback(() => {
    setSaved(true);
    void notesChanged(noteId).catch(logBackgroundFailure("Daily note saved"));
  }, [noteId]);
  const editor = useNoteEditor({ kind: "daily", date }, { onSaved });
  const { refresh, flush } = editor;
  const conflictVersion = useRef(0);
  const reload = useCallback(async () => {
    const version = ++conflictVersion.current;
    files.retry();
    await Promise.all([
      refresh().catch((reason) => setActionError(errorMessage(reason))),
      replicatedSyncConflicts().then((found) => {
        if (version !== conflictVersion.current) return;
        const current = noteConflictsById(found).get(noteId) ?? [];
        setConflicts(current);
        setConflictError(null);
        if (current.length) void replicatedSyncDeviceRoster().then(setRoster, logBackgroundFailure("Note conflict devices"));
      }, (reason) => { if (version === conflictVersion.current) setConflictError(errorMessage(reason)); }),
    ]);
  }, [refresh, files, noteId]);
  const cancelReload = useCallback(() => { conflictVersion.current++; }, []);
  useEffect(() => { void reload(); return cancelReload; }, [reload, refreshKey, active, cancelReload]);
  useNoteChanges(reload);
  useEffect(() => { if (!active) void flush(); }, [active, flush]);
  const stale = editor.saveState.kind === "stale" ? editor.saveState : null;
  const readOnly = editor.loading || Boolean(editor.loadError || editor.note?.trashed || stale || conflictError) || conflicts === null || conflicts.length > 0;
  const leave = () => { void flush(); onLeave(); };
  const act = async (action: () => Promise<void> | void) => {
    setActionError(null);
    try { await action(); } catch (reason) { setActionError(errorMessage(reason)); }
  };
  const beforeOpen = async () => {
    await flush();
    if (editor.unsaved()) throw new Error("Save or resolve this note's changes before opening it in Notes or following a link.");
  };
  useImperativeHandle(ref, () => ({
    focus: () => { if (active && !readOnly) body.current?.focus(); },
    openInNotes: () => act(async () => { await beforeOpen(); onOpenNote?.(noteId); }),
  }));
  const searchLinks = async (query: string) => (await mailClient.suggestNoteTitles(query, 8)).filter((note) => note.id !== noteId).map((note) => ({ title: note.title }));
  const searchEntities = async (query: string) => {
    const [contacts, conversations] = await Promise.all([mailClient.noteLinkChoices("contact", query), mailClient.noteLinkChoices("conversation", query)]);
    return [...contacts.slice(0, 8), ...conversations.slice(0, 8)];
  };
  const status = editor.loading ? "Loading note…" : editor.saveState.kind === "saving" ? "Saving…" : editor.dirty ? "Not saved yet" : saved ? "Saved" : "";
  return <div className="today-daily-note" data-shortcut-scope="compose" onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) void flush();
  }}>
    <div className="today-note-heading">
      <h2 className="notes-editor-title">{formatNoteDate(date)}</h2>
      <p className="notes-save-status" role="status">{status}</p>
    </div>
    <p className="context-status today-note-hint">Enter or click the note to write. Press Escape when you're done; then {formatShortcut("Tab")} switches to Agenda.</p>
    {editor.loadError ? <p className="form-error" role="alert">{editor.loadError}</p> : null}
    {conflictError ? <div role="alert"><p className="form-error">{conflictError}</p><button type="button" className="btn btn-sm" onClick={() => void reload()}>Try Again</button></div> : null}
    {editor.note?.trashed && !stale ? <p className="context-status">This daily note is in Trash. Open it in Notes to restore it.</p> : null}
    {stale ? <div className="notes-stale" role="alert">
      <p>{stale.current?.trashed ? "This note was moved to Trash on another device after you started typing. Your text is kept here until you choose."
        : stale.current ? "This note changed on another device after you started typing. Your text is kept here until you choose."
        : "This note was deleted on another device. Your text is kept here until you choose."}</p>
      {stale.error ? <p className="form-error">{stale.error}</p> : null}
      <div className="notes-stale-actions">
        <button type="button" className="btn btn-primary" onClick={() => void editor.keepMine()}>{stale.current?.trashed ? "Restore and Keep My Text" : "Keep My Version"}</button>
        <button type="button" className="btn" onClick={editor.takeTheirs}>{stale.current && !stale.current.trashed ? "Use Other Version" : "Discard My Text"}</button>
      </div>
    </div> : null}
    {editor.note && !stale && Boolean(conflicts?.length) ? <NoteConflictBanner key={noteId} note={editor.note} title={formatNoteDate(date)} conflicts={conflicts!} roster={roster}
      onResolved={() => { void notesChanged(noteId); void reload(); }} onKeptBoth={() => { void notesChanged(noteId); }} /> : null}
    {!editor.loadError ? <NoteEditor ref={body} noteKey={noteId} value={editor.text.body} label="Daily Note Text" placeholder="Write about the day" mode={syntax} readOnly={readOnly} files={files}
      onChange={(value) => editor.edit({ body: value })} onLeave={leave}
      onSearchLinks={searchLinks} onSearchEntities={searchEntities}
      onOpenLink={(title) => void act(async () => {
        await beforeOpen();
        const day = title.trim();
        if (localDay(day)) { onOpenNote?.(`daily:${day}`); return; }
        const found = await mailClient.findNoteByTitle(title) ?? await mailClient.createNote(title, "");
        onOpenNote?.(found.id);
      })}
      onOpenEntity={(href) => void act(async () => {
        if (!parseNoteReference(href)) throw new Error("Invalid note reference");
        await beforeOpen();
        await onOpenEntity?.(href);
      })} /> : null}
    {editor.saveState.kind === "error" ? <p className="form-error" role="alert">{editor.saveState.message}</p> : null}
    {actionError ? <p className="form-error" role="alert">{actionError}</p> : null}
  </div>;
}
