import { useEffect, useMemo, useState } from "react";
import { mailClient } from "./data/client";
import type { RecordingStatus } from "./domain";
import { formatElapsed } from "./recordingPolicy";

/** How often the displayed clock advances. */
const TICK_MS = 1000;

/**
 * The elapsed time of a recording. The native side reports `elapsedMs` as of
 * the moment it emitted the state, so while recording the clock advances here
 * from when that state arrived; while paused it holds still.
 */
export function useRecordingElapsed(status: RecordingStatus): number {
  const anchor = useMemo(() => ({ elapsedMs: status.elapsedMs, at: Date.now() }), [status]);
  const [, setTick] = useState(0);
  const running = status.state === "recording";
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setTick((tick) => tick + 1), TICK_MS);
    return () => window.clearInterval(timer);
  }, [running, anchor]);
  return running ? anchor.elapsedMs + Math.max(0, Date.now() - anchor.at) : status.elapsedMs;
}

/**
 * Shown in the sidebar, above the account switcher, only while a recording is
 * under way: a dot, the elapsed time (or Paused), and Stop. It never takes
 * focus when it appears; Stop is an ordinary button in the tab order.
 */
export function RecordingIndicator({ status, onStop }: { status: RecordingStatus; onStop(): void }) {
  const elapsed = useRecordingElapsed(status);
  if (status.state === "idle") return null;
  const paused = status.state === "paused";
  const title = status.title ? `${paused ? "Paused" : "Recording"}: ${status.title}` : paused ? "Paused" : "Recording";
  const micTrack = status.tracks.find((entry) => entry.track === "mic");
  const systemTrack = status.tracks.find((entry) => entry.track === "system");
  return (
    <div className={`recording-indicator${paused ? " paused" : ""}`} role="group" aria-label="Recording" title={title}>
      <span className="recording-indicator-dot" aria-hidden="true" />
      <span className="recording-indicator-time" role="timer" aria-label={paused ? "Recording paused" : "Recording time"}>
        {paused ? "Paused" : formatElapsed(elapsed)}
      </span>
      {micTrack?.state === "active" || systemTrack?.state === "active" ? (
        <div className="recording-indicator-signals" aria-label="Audio track signals">
          {micTrack?.state === "active" ? (
            <span
              className={`recording-signal-pill${micTrack.signalActive ? " active" : ""}`}
              aria-label={micTrack.signalActive ? "Microphone signal active" : "Microphone silent"}
            >
              Mic
            </span>
          ) : null}
          {systemTrack?.state === "active" ? (
            <span
              className={`recording-signal-pill${systemTrack.signalActive ? " active" : ""}`}
              aria-label={systemTrack.signalActive ? "System audio signal active" : "System audio silent"}
            >
              Sys
            </span>
          ) : null}
        </div>
      ) : null}
      {status.systemAudioSilentWarning ? (
        <div className="recording-indicator-warning" role="status">
          <span>System audio is silent</span>
          <button type="button" className="btn btn-sm" onClick={() => void mailClient.openSystemAudioSettings()}>
            Check Permission
          </button>
        </div>
      ) : null}
      {status.stillRecordingNudge && !paused ? (
        <span className="recording-indicator-nudge" role="status">Still recording?</span>
      ) : null}
      <button type="button" className="btn btn-sm recording-indicator-stop" aria-label="Stop Recording" onClick={onStop}>Stop</button>
    </div>
  );
}
