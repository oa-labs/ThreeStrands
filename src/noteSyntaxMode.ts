import { useEffect, useState } from "react";

/**
 * How much Markdown syntax the note editor shows, set in
 * **Settings → Notes → Markdown Syntax**. A preference of this device, so it
 * isn't synchronized or exported.
 * - `hide`: formatting only, syntax never visible.
 * - `hybrid`: syntax appears around the cursor.
 * - `show`: syntax always visible, still styled.
 */
export type NoteSyntaxMode = "hide" | "hybrid" | "show";

export const NOTE_SYNTAX_MODES: NoteSyntaxMode[] = ["hide", "hybrid", "show"];
export const DEFAULT_NOTE_SYNTAX_MODE: NoteSyntaxMode = "hide";
const STORAGE_KEY = "threestrands.notes.syntax";
const CHANGED = "threestrands:note-syntax-mode";

export function readNoteSyntaxMode(): NoteSyntaxMode {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return NOTE_SYNTAX_MODES.includes(saved as NoteSyntaxMode) ? saved as NoteSyntaxMode : DEFAULT_NOTE_SYNTAX_MODE;
  } catch {
    return DEFAULT_NOTE_SYNTAX_MODE;
  }
}

export function writeNoteSyntaxMode(mode: NoteSyntaxMode): void {
  try { localStorage.setItem(STORAGE_KEY, mode); } catch { /* the choice lasts until the window closes */ }
  window.dispatchEvent(new CustomEvent(CHANGED, { detail: mode }));
}

/** The current choice, following changes made in Settings. */
export function useNoteSyntaxMode(): NoteSyntaxMode {
  const [mode, setMode] = useState(readNoteSyntaxMode);
  useEffect(() => {
    const changed = (event: Event) => setMode(event instanceof CustomEvent ? event.detail as NoteSyntaxMode : readNoteSyntaxMode());
    window.addEventListener(CHANGED, changed);
    return () => window.removeEventListener(CHANGED, changed);
  }, []);
  return mode;
}
