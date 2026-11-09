import { useId, useState } from "react";
import type { Note } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { replicatedSyncResolveConflict, type DeviceRosterEntry, type FrontierConflict } from "./replicatedSync";
import { conflictChoices, deviceName, diffLines, noteConflictView, type NoteVersion } from "./noteConflicts";

/**
 * Shown above a note that two devices changed before either saw the other's
 * change. Every device shows the same version until someone chooses; the
 * note stays read-only meanwhile, since typing would quietly discard the
 * other version.
 */
export function NoteConflictBanner({ note, title, conflicts, roster, onResolved, onKeptBoth }: {
  note: Note;
  /** The note's name as the editor shows it. */
  title: string;
  conflicts: FrontierConflict[];
  roster: DeviceRosterEntry[];
  /** Called after a choice is saved, to reload the note and its conflicts. */
  onResolved(): void;
  onKeptBoth(created: Note): void;
}) {
  const { others } = noteConflictView(note, conflicts);
  const [pick, setPick] = useState(0);
  const [comparing, setComparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const compareId = useId();
  const other: NoteVersion | undefined = others[Math.min(pick, others.length - 1)];
  if (!other) return null;
  const otherName = deviceName(other.deviceId, roster);

  const run = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  const keep = (deviceId: string | null) => run(async () => {
    for (const { conflict, chosen } of conflictChoices(note, conflicts, deviceId)) {
      await replicatedSyncResolveConflict(conflict, chosen);
    }
    onResolved();
  });
  const keepBoth = () => run(async () => {
    // The other version becomes its own note first, so nothing is lost if keeping this one fails.
    const created = await mailClient.createNote(`${other.title === note.title ? title : other.title} (from ${otherName})`, other.body);
    onKeptBoth(created);
    for (const { conflict, chosen } of conflictChoices(note, conflicts, null)) {
      await replicatedSyncResolveConflict(conflict, chosen);
    }
    onResolved();
  });

  const titleDiffers = other.title !== note.title;
  const trashDiffers = other.trashed !== note.trashed;
  return <div className="notes-conflict" role="region" aria-label="Note conflict">
    <p>This note was also changed on {otherName} before your devices synced. Choose which version to keep before editing it.</p>
    {others.length > 1 ? <label className="notes-conflict-pick">Other version
      <select value={pick} onChange={(event) => setPick(Number(event.target.value))}>
        {others.map((version, index) => <option key={version.deviceId} value={index}>From {deviceName(version.deviceId, roster)}</option>)}
      </select>
    </label> : null}
    <div className="notes-conflict-actions">
      <button type="button" className="btn" aria-expanded={comparing} aria-controls={compareId} onClick={() => setComparing((open) => !open)}>{comparing ? "Hide Comparison" : "Compare"}</button>
      <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void keep(null)}>Keep This Version</button>
      <button type="button" className="btn" disabled={busy} onClick={() => void keep(other.deviceId)}>Use Other Version</button>
      <button type="button" className="btn" disabled={busy} onClick={() => void keepBoth()}>Keep Both</button>
    </div>
    {error ? <p className="form-error" role="alert">{error}</p> : null}
    {comparing ? <div id={compareId} className="notes-conflict-compare">
      {titleDiffers ? <p>Title from {otherName}: <strong>{other.title}</strong></p> : null}
      {trashDiffers ? <p>{other.trashed ? `${otherName} moved this note to Trash.` : `${otherName} restored this note from Trash.`}</p> : null}
      <ol className="notes-diff" aria-label={`Changes from this version to the one from ${otherName}`}>
        {diffLines(note.body, other.body).map((line, index) => <li key={index} className={`notes-diff-${line.kind}`}>
          {line.kind === "same" ? null : <span className="sr-only">{line.kind === "added" ? `Only from ${otherName}: ` : "Only in this version: "}</span>}
          <span aria-hidden="true" className="notes-diff-mark">{line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "}</span>
          {line.text || " "}
        </li>)}
      </ol>
    </div> : null}
  </div>;
}
