import { useEffect, useMemo, useState } from "react";
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
  return (
    <div className={`recording-indicator${paused ? " paused" : ""}`} role="group" aria-label="Recording" title={title}>
      <span className="recording-indicator-dot" aria-hidden="true" />
      <span className="recording-indicator-time" role="timer" aria-label={paused ? "Recording paused" : "Recording time"}>
        {paused ? "Paused" : formatElapsed(elapsed)}
      </span>
      <button type="button" className="btn btn-sm recording-indicator-stop" aria-label="Stop Recording" onClick={onStop}>Stop</button>
    </div>
  );
}
