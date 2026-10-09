import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { OpenedCalendarFile } from "./domain";
import { logBackgroundFailure } from "./errors";

/** Emitted by the native layer when macOS opens a `.ics` file with ThreeStrands. */
export const CALENDAR_FILE_EVENT = "calendar-file-received";

/**
 * Hands each calendar file macOS opened with ThreeStrands — including the one
 * that launched it — to `onFiles`, now and whenever more arrive. Returns the
 * unsubscribe function.
 */
export function listenForOpenedCalendarFiles(onFiles: (files: OpenedCalendarFile[]) => void): () => void {
  if (!("__TAURI_INTERNALS__" in window)) return () => {};
  const drain = () => {
    void invoke<OpenedCalendarFile[]>("take_pending_calendar_files")
      .then((files) => { if (files.length) onFiles(files); })
      .catch(logBackgroundFailure("receiving calendar files"));
  };
  // Drain once the listener is live, so a file queued before then is shown.
  const listener = listen(CALENDAR_FILE_EVENT, drain);
  void listener.then(drain);
  return () => { void listener.then((unlisten) => unlisten()); };
}
