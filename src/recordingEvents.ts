import { listen } from "@tauri-apps/api/event";
import { logBackgroundFailure } from "./errors";
import type { RecordingStartRequest, RecordingStatus, ScheduleEvent, TranscriptionModelStatus } from "./domain";

/** Events the native recorder sends the webview. */
export type RecordingEvents = {
  /** Every state change. `elapsedMs` is as of the moment of emitting. */
  "recording-state": RecordingStatus;
  /** A recording's row changed, such as when its state moved on. */
  "recordings-changed": { recordingId: string };
  /** Whole-percent progress of a transcription, only when the percent changes. */
  "transcription-progress": { recordingId: string; percent: number };
  /** Every transcription model state change; the full list, fast then accurate. */
  "transcription-models-changed": TranscriptionModelStatus[];
  /** A menu bar command that needs the app's own flow. */
  "recording-tray-command": { command: "start" | "startOffer" | "openSettings" | "review" | "openMeeting"; recordingId?: string; startRequest?: RecordingStartRequest; meetingEvent?: ScheduleEvent };
  /** The user tried to quit while recording. */
  "recording-quit-requested": null;
};

/** Listens for a native recording event. Returns the unsubscribe function; a no-op outside the desktop app. */
export function listenForRecordingEvent<K extends keyof RecordingEvents>(name: K, handler: (payload: RecordingEvents[K]) => void): () => void {
  if (!("__TAURI_INTERNALS__" in window)) return () => {};
  let active = true;
  let stop: (() => void) | undefined;
  void listen<RecordingEvents[K]>(name, (event) => handler(event.payload))
    .then((unlisten) => { if (active) stop = unlisten; else unlisten(); })
    .catch(logBackgroundFailure(`Recording ${name} listener`));
  return () => { active = false; stop?.(); };
}

const SETTINGS_CHANGED = "threestrands:recording-settings-changed";

/** Tells the recording controller that Settings saved, so it reads them again. */
export function announceRecordingSettingsChanged(): void {
  window.dispatchEvent(new Event(SETTINGS_CHANGED));
}

export function listenForRecordingSettingsChanged(handler: () => void): () => void {
  window.addEventListener(SETTINGS_CHANGED, handler);
  return () => window.removeEventListener(SETTINGS_CHANGED, handler);
}
