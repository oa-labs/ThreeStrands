import { useRef, useState } from "react";
import { Modal } from "./AppChrome";
import type { Note } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

/** Names a new note. The note is created only when the user submits. */
export function NewNoteDialog({ onCreated, onClose }: { onCreated(note: Note): void; onClose(): void }) {
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const create = async () => {
    if (!title.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      onCreated(await mailClient.createNote(title, ""));
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
      input.current?.focus();
    }
  };
  return <Modal title="New Note" className="new-note-modal" initialFocusRef={input} onClose={onClose}>
    <form className="modal-form" onSubmit={(event) => { event.preventDefault(); void create(); }}
      onKeyDown={(event) => {
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); void create(); }
      }}>
      <label>Title
        <input {...SEARCH_INPUT_ATTRIBUTES} ref={input} value={title} maxLength={200} aria-invalid={error ? true : undefined}
          aria-describedby={error ? "new-note-error" : undefined} onChange={(event) => setTitle(event.target.value)} />
      </label>
      {error ? <p id="new-note-error" className="form-error" role="alert">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={!title.trim() || busy}>Create Note</button>
      </div>
    </form>
  </Modal>;
}
