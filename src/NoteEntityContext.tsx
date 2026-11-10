import { useCallback, useEffect, useState } from "react";
import { NotebookText } from "lucide-react";
import { mailClient } from "./data/client";
import { ContextRow, ContextSection } from "./ContextSections";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { useNoteChanges } from "./noteMutations";
import { noteReferenceKey, noteReferenceHref, type NoteBacklink, type NoteReference } from "./noteEntities";

type OpenNote = (id: string, markerId?: string, referenceHref?: string) => void;

/** Notes outside Trash that link to any of `references`, one row per note,
 * reloaded when notes change. */
export function useNoteBacklinks(references: NoteReference[], refreshKey: string | number = "") {
  const [rows, setRows] = useState<NoteBacklink[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const key = references.map(noteReferenceKey).join(";");
  const changed = useCallback(() => setRevision((v) => v + 1), []);
  useNoteChanges(changed);
  useEffect(() => {
    let active = true;
    setRows([]); setError(null);
    if (!references.length) return;
    void Promise.all(references.map((reference) => mailClient.noteBacklinks(reference))).then((groups) => groups.flat().filter((row, index, rows) => rows.findIndex((other) => other.noteId === row.noteId) === index)).then((rows) => { if (active) setRows(rows); }, (reason) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
    // Reference identity, independent of the caller's object allocation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, revision, refreshKey]);
  return { rows, error };
}

/** The Notes section listing backlinks; opening a row selects its link. */
export function NoteBacklinkSection({ rows, error, onOpenNote }: { rows: NoteBacklink[]; error: string | null; onOpenNote?: OpenNote }) {
  return <>{error ? <p role="alert" className="form-error">{error}</p> : null}{rows.length ? <ContextSection id="entity-note-backlinks" title="Notes" count={rows.length} rows={rows.map((row) => <ContextRow key={row.noteId} title={row.title} detail={row.context} icon={<NotebookText size={ICON_SIZE.sm} />} onActivate={() => onOpenNote?.(row.noteId, undefined, row.reference ? noteReferenceHref(row.reference) : undefined)} />)} /> : null}</>;
}

export function NoteBacklinks({ reference, onOpenNote, refreshKey = "", additional }: { reference: NoteReference; onOpenNote?: OpenNote; refreshKey?: string | number; additional?: NoteReference[] }) {
  const { rows, error } = useNoteBacklinks([reference, ...additional ?? []], refreshKey);
  return <NoteBacklinkSection rows={rows} error={error} onOpenNote={onOpenNote} />;
}
