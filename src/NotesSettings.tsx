import { useEffect, useState } from "react";
import type { NotesExportFolder, NotesExportOutcome, NotesImportProblem, NotesImportSummary, NotesStorage } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage, logBackgroundFailure } from "./errors";
import { InlineConfirm } from "./InlineConfirm";
import { formatAttachmentSize } from "./threadPresentation";
import { NOTE_SYNTAX_MODES, readNoteSyntaxMode, writeNoteSyntaxMode, type NoteSyntaxMode } from "./noteSyntaxMode";

const LABELS: Record<NoteSyntaxMode, string> = { hide: "Hide", hybrid: "Hybrid", show: "Show" };
const HINTS: Record<NoteSyntaxMode, string> = {
  hide: "Notes show formatting only, like a document.",
  hybrid: "Markdown characters appear around the cursor while you write.",
  show: "Markdown characters are always visible, still formatted.",
};

const IN_USE = "This folder already holds other files. Choose an empty folder, or one an earlier export of notes wrote.";

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

function exportedMessage(outcome: NotesExportOutcome): string {
  const files = outcome.attachments ? ` and ${count(outcome.attachments, "file", "files")}` : "";
  const missing = outcome.missingAttachments
    ? ` ${count(outcome.missingAttachments, "file hasn't", "files haven't")} reached this device yet, so ${outcome.missingAttachments === 1 ? "it was" : "they were"} left out.`
    : "";
  return `Exported ${count(outcome.notes, "note", "notes")}${files} to ${outcome.path}.${missing}`;
}

/** Export to Markdown Folder: every note outside Trash as a Markdown file. */
function NotesExport() {
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<NotesExportFolder | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const write = async (replace: boolean) => {
    setBusy(true);
    try {
      const outcome = await mailClient.exportNotes(replace);
      setMessage(exportedMessage(outcome));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
      setConfirming(null);
    }
  };
  const choose = async () => {
    setError(null);
    setMessage(null);
    setConfirming(null);
    setBusy(true);
    let folder: NotesExportFolder | null;
    try {
      folder = await mailClient.chooseNotesExportFolder();
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
      return;
    }
    setBusy(false);
    if (!folder) return;
    if (folder.state === "inUse") setError(IN_USE);
    else if (folder.state === "earlierExport") setConfirming(folder);
    else await write(false);
  };
  return (
    <section className="settings-section" aria-label="Export notes">
      <h3>Export</h3>
      <div className="settings-row">
        <button type="button" className="btn" disabled={busy} aria-describedby="notes-export-hint" onClick={() => void choose()}>Export to Markdown Folder…</button>
      </div>
      <p id="notes-export-hint" className="settings-hint">Writes each note outside Trash as a Markdown file you can open in other apps: daily notes in a “daily” folder, the rest in “notes”, and their files in “attachments”. Choose an empty folder, or one you exported to before.</p>
      {confirming ? <InlineConfirm ariaLabel="Replace earlier export" stacked focusCancel cancelLabel="Cancel" disabled={busy} onCancel={() => setConfirming(null)}
        title={`Replace the earlier export in ${confirming.path}?`}
        actions={[{ label: "Replace Export", onClick: () => void write(true) }]}>
        The files that export wrote are replaced with your notes as they are now. Anything else in the folder stays.
      </InlineConfirm> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {message ? <p className="settings-hint" role="status">{message}</p> : null}
    </section>
  );
}

/** What won't be imported, folded away so a long list doesn't push the actions out of view. */
function ImportProblems({ problems, label }: { problems: NotesImportProblem[]; label: string }) {
  if (!problems.length) return null;
  return <details className="notes-import-problems">
    <summary>{label}</summary>
    <ul>{problems.map((problem) => <li key={`${problem.file}:${problem.reason}`}><code>{problem.file}</code> {problem.reason}</li>)}</ul>
  </details>;
}

/** Import Markdown Folder: read a folder first, show what it holds, then import on confirmation. */
function NotesImport() {
  const [busy, setBusy] = useState(false);
  const [plan, setPlan] = useState<NotesImportSummary | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [failed, setFailed] = useState<NotesImportProblem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const choose = async () => {
    setError(null); setMessage(null); setFailed([]); setPlan(null);
    setBusy(true);
    try {
      const summary = await mailClient.chooseNotesImportFolder();
      if (!summary) return;
      if (summary.notes + summary.dailyNotes === 0) {
        setMessage(summary.alreadyHere ? `Every note in ${summary.path} is already in Notes.` : `${summary.path} holds no notes to import.`);
        setFailed(summary.problems);
      } else {
        setPlan(summary);
      }
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  const run = async () => {
    setBusy(true);
    try {
      const outcome = await mailClient.importNotes();
      const files = outcome.files ? ` and ${count(outcome.files, "file", "files")}` : "";
      setMessage(`Imported ${count(outcome.notes, "note", "notes")}${files}.`);
      setFailed(outcome.problems);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
      setPlan(null);
    }
  };
  const total = plan ? plan.notes + plan.dailyNotes : 0;
  return (
    <section className="settings-section" aria-label="Import notes">
      <h3>Import</h3>
      <div className="settings-row">
        <button type="button" className="btn" disabled={busy} aria-describedby="notes-import-hint" onClick={() => void choose()}>Import Markdown Folder…</button>
      </div>
      <p id="notes-import-hint" className="settings-hint">Adds the Markdown notes in a folder, such as a Reflect folder or an earlier export, with their pictures and files. Nothing is imported until you confirm, and notes already here are never replaced.</p>
      {plan ? <InlineConfirm ariaLabel="Import notes" stacked focusCancel cancelLabel="Cancel" disabled={busy} onCancel={() => setPlan(null)}
        title={`Import ${count(total, "note", "notes")} from ${plan.path}?`}
        actions={[{ label: "Import Notes", onClick: () => void run() }]}>
        {count(plan.notes, "note", "notes")}, {count(plan.dailyNotes, "daily note", "daily notes")}, and {count(plan.files, "file", "files")}.{plan.alreadyHere ? ` ${count(plan.alreadyHere, "note is", "notes are")} already here and will be skipped.` : ""}
        <ImportProblems problems={plan.problems} label={`${count(plan.problems.length, "file", "files")} won’t be imported`} />
      </InlineConfirm> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {message ? <p className="settings-hint" role="status">{message}</p> : null}
      {!plan ? <ImportProblems problems={failed} label={`${count(failed.length, "file wasn’t", "files weren’t")} imported`} /> : null}
    </section>
  );
}

/** How much of their sync space notes use. */
function NotesStorageUsage() {
  const [storage, setStorage] = useState<NotesStorage | null>(null);
  useEffect(() => {
    void mailClient.notesStorage().then(setStorage, logBackgroundFailure("Notes storage"));
  }, []);
  if (!storage) return null;
  return (
    <section className="settings-section" aria-label="Notes storage">
      <h3>Sync Space</h3>
      <p className="settings-hint">Notes use {formatAttachmentSize(storage.usedBytes)} of the {formatAttachmentSize(storage.limitBytes)} set aside for them in cross-device sync, Trash included.</p>
      {storage.attachmentCount ? <p className="settings-hint">Their {count(storage.attachmentCount, "file takes", "files take")} {formatAttachmentSize(storage.attachmentBytes)} more. Files sync separately from note text, so they don’t count toward that space.</p> : null}
    </section>
  );
}

/** Notes preferences for this device, export, and sync space. */
export function NotesSettings() {
  const [mode, setMode] = useState<NoteSyntaxMode>(readNoteSyntaxMode);
  return (
    <>
      <section className="settings-section" aria-label="Note editor">
        <h3>Markdown Syntax</h3>
        <div className="settings-radio-row" role="radiogroup" aria-label="Markdown syntax" aria-describedby="note-syntax-hint">
          {NOTE_SYNTAX_MODES.map((value) => (
            <label key={value}>
              <input type="radio" name="note-syntax" checked={mode === value} onChange={() => { setMode(value); writeNoteSyntaxMode(value); }} />
              {LABELS[value]}
            </label>
          ))}
        </div>
        <p id="note-syntax-hint" className="settings-hint">{HINTS[mode]} This choice applies to this device only.</p>
      </section>
      <NotesImport />
      <NotesExport />
      <NotesStorageUsage />
    </>
  );
}
