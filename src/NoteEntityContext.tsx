import { useCallback, useEffect, useState } from "react";
import { NotebookText } from "lucide-react";
import { mailClient } from "./data/client";
import { ContextRow, ContextSection } from "./ContextSections";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { useNoteChanges } from "./noteMutations";
import { noteReferenceKey, noteReferenceHref, type NoteBacklink, type NoteReference } from "./noteEntities";

export function NoteBacklinks({ reference, onOpenNote, refreshKey = "", additional }: { reference: NoteReference; onOpenNote?(id: string, markerId?: string, referenceHref?: string): void; refreshKey?: string | number; additional?: NoteReference[] }) {
  const [rows, setRows] = useState<NoteBacklink[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const key = [reference, ...additional ?? []].map(noteReferenceKey).join(";");
  const changed = useCallback(() => setRevision((v) => v + 1), []);
  useNoteChanges(changed);
  useEffect(() => {
    let active = true;
    setRows([]); setError(null);
    void Promise.all([reference, ...additional ?? []].map((reference) => mailClient.noteBacklinks(reference))).then((groups) => groups.flat().filter((row, index, rows) => rows.findIndex((other) => other.noteId === row.noteId) === index)).then((rows) => { if (active) setRows(rows); }, (reason) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
    // Reference identity, independent of the caller's object allocation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, revision, refreshKey]);
  return <>{error ? <p role="alert" className="form-error">{error}</p> : null}{rows.length ? <ContextSection id="entity-note-backlinks" title="Notes" count={rows.length} rows={rows.map((row) => <ContextRow key={row.noteId} title={row.title} detail={row.context} icon={<NotebookText size={ICON_SIZE.sm} />} onActivate={() => onOpenNote?.(row.noteId, undefined, row.reference ? noteReferenceHref(row.reference) : undefined)} />)} /> : null}</>;
}
