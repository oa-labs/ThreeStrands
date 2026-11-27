import { useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { TranscriptionModelStatus } from "./domain";
import { logBackgroundFailure } from "./errors";
import { listenForRecordingEvent } from "./recordingEvents";

/** How often a download is read again where native events don't exist (the browser preview). */
const PREVIEW_POLL_MS = 500;

/**
 * The state of both transcription models, read once and then kept current by
 * `transcription-models-changed`. Null until read, or where recording isn't
 * supported.
 */
export function useTranscriptionModels(enabled = true): TranscriptionModelStatus[] | null {
  const [models, setModels] = useState<TranscriptionModelStatus[] | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    mailClient.transcriptionModels().then((found) => { if (active) setModels(found); }, logBackgroundFailure("Reading transcription models"));
    const stop = listenForRecordingEvent("transcription-models-changed", (next) => { if (active) setModels(next); });
    return () => { active = false; stop(); };
  }, [enabled]);

  const downloading = models?.some((model) => model.state === "downloading" || model.state === "verifying") ?? false;
  useEffect(() => {
    if (!downloading || "__TAURI_INTERNALS__" in window) return;
    const timer = window.setInterval(() => {
      mailClient.transcriptionModels().then(setModels, logBackgroundFailure("Reading transcription models"));
    }, PREVIEW_POLL_MS);
    return () => window.clearInterval(timer);
  }, [downloading]);
  return models;
}
