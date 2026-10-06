import { useState, useSyncExternalStore } from "react";
import { Modal } from "./AppChrome";

// The one-time recovery phrase from starting a sync space is never stored
// anywhere by the native side, so this in-memory holder is its only copy
// until the user proves they recorded it. It lives outside any component so
// switching Settings sections or closing Settings cannot drop it; only
// quitting the app can. It is deliberately never written to web storage.
let pendingPhrase: string | null = null;
const listeners = new Set<() => void>();

function notify() {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot() {
  return pendingPhrase;
}

export function holdRecoveryPhrase(phrase: string) {
  pendingPhrase = phrase;
  notify();
}

export function pendingRecoveryPhrase(): string | null {
  return pendingPhrase;
}

function releaseRecoveryPhrase() {
  pendingPhrase = null;
  notify();
}

/** Blocks the app until the user acknowledges recording the recovery phrase.
 * Renders nothing while no phrase is pending. */
export function PendingRecoveryPhraseDialog() {
  const phrase = useSyncExternalStore(subscribe, snapshot);
  return phrase ? <RecoveryPhraseDialog key={phrase} phrase={phrase} /> : null;
}

function RecoveryPhraseDialog({ phrase }: { phrase: string }) {
  const words = phrase.split(/\s+/).filter(Boolean);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);

  const copy = () => {
    void navigator.clipboard
      .writeText(words.join(" "))
      .then(() => setCopyStatus("Copied. Store it somewhere safe, then clear it from your clipboard."))
      .catch(() => setCopyStatus("Couldn’t copy. Write the words down instead."));
  };

  return (
    <Modal title="Save Your Recovery Phrase" className="recovery-phrase-modal" dismissible={false} onClose={() => {}}>
      <div className="modal-form">
        <p className="settings-hint">
          <strong>This is the only time this phrase is shown, and it is never stored anywhere.</strong> It is the only
          way to recover this sync space if every device is lost. Keep it offline, somewhere only you can reach.
        </p>
        <ol className="recovery-phrase-words" aria-label="Recovery phrase">
          {words.map((word, index) => <li key={index}>{word}</li>)}
        </ol>
        {copyStatus ? <p role="status" className="settings-hint">{copyStatus}</p> : null}
        <div className="modal-form-actions">
          <button type="button" className="btn" onClick={copy}>Copy</button>
          <button type="button" className="btn btn-primary" onClick={releaseRecoveryPhrase}>I’ve written it down</button>
        </div>
      </div>
    </Modal>
  );
}
