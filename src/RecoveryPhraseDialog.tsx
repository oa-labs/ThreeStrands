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

export const RECOVERY_CONFIRMATION_WORDS = 3;

/** Distinct zero-based word positions the user must re-type, in reading order. */
export function pickConfirmationPositions(wordCount: number, count = RECOVERY_CONFIRMATION_WORDS, random = Math.random): number[] {
  const positions = Array.from({ length: wordCount }, (_, index) => index);
  for (let index = positions.length - 1; index > 0; index -= 1) {
    const swap = Math.floor(random() * (index + 1));
    [positions[index], positions[swap]] = [positions[swap]!, positions[index]!];
  }
  return positions.slice(0, Math.min(count, wordCount)).sort((a, b) => a - b);
}

export function recoveryWordMatches(expected: string, typed: string): boolean {
  return typed.trim().toLocaleLowerCase() === expected.toLocaleLowerCase();
}

/** Blocks the app until the pending recovery phrase has been recorded and
 * spot-checked. Renders nothing while no phrase is pending. */
export function PendingRecoveryPhraseDialog() {
  const phrase = useSyncExternalStore(subscribe, snapshot);
  return phrase ? <RecoveryPhraseDialog key={phrase} phrase={phrase} /> : null;
}

function RecoveryPhraseDialog({ phrase }: { phrase: string }) {
  const words = phrase.split(/\s+/).filter(Boolean);
  const [positions] = useState(() => pickConfirmationPositions(words.length));
  const [step, setStep] = useState<"record" | "confirm">("record");
  const [answers, setAnswers] = useState<Record<number, string>>({});
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  const confirmed = positions.every((position) => recoveryWordMatches(words[position]!, answers[position] ?? ""));

  const copy = () => {
    void navigator.clipboard
      .writeText(words.join(" "))
      .then(() => setCopyStatus("Copied. Store it somewhere safe, then clear it from your clipboard."))
      .catch(() => setCopyStatus("Couldn’t copy. Write the words down instead."));
  };

  return (
    <Modal title="Save Your Recovery Phrase" className="recovery-phrase-modal" dismissible={false} onClose={() => {}}>
      {step === "record" ? (
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
            <button type="button" onClick={copy}>Copy</button>
            <button type="button" onClick={() => setStep("confirm")}>I’ve written it down</button>
          </div>
        </div>
      ) : (
        <form
          className="modal-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (confirmed) releaseRecoveryPhrase();
          }}
        >
          <p className="settings-hint">Enter these words from your recovery phrase to confirm you recorded it correctly.</p>
          {positions.map((position) => (
            <label key={position}>
              <span>Word {position + 1}</span>
              <input
                type="text"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                value={answers[position] ?? ""}
                onChange={(event) => setAnswers((current) => ({ ...current, [position]: event.target.value }))}
              />
            </label>
          ))}
          <div className="modal-form-actions">
            <button type="button" onClick={() => setStep("record")}>Show phrase again</button>
            <button type="submit" disabled={!confirmed}>Finish</button>
          </div>
        </form>
      )}
    </Modal>
  );
}
