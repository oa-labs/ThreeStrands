import { useState } from "react";
import { NOTE_SYNTAX_MODES, readNoteSyntaxMode, writeNoteSyntaxMode, type NoteSyntaxMode } from "./noteSyntaxMode";

const LABELS: Record<NoteSyntaxMode, string> = { hide: "Hide", hybrid: "Hybrid", show: "Show" };
const HINTS: Record<NoteSyntaxMode, string> = {
  hide: "Notes show formatting only, like a document.",
  hybrid: "Markdown characters appear around the cursor while you write.",
  show: "Markdown characters are always visible, still formatted.",
};

/** Notes preferences for this device. */
export function NotesSettings() {
  const [mode, setMode] = useState<NoteSyntaxMode>(readNoteSyntaxMode);
  return (
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
  );
}
