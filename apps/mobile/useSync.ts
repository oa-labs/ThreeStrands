import { useCallback, useEffect, useRef, useState } from "react";
import { errorText, mobile, type SyncStatus } from "./api";

const SYNC_INTERVAL = 30_000;

export function useSync(onChange: () => void) {
  const [status, setStatus] = useState<SyncStatus>();
  const [issue, setIssue] = useState<string>();
  const [running, setRunning] = useState(false);
  const inFlight = useRef<Promise<void> | null>(null);
  const sync = useCallback(() => {
    if (inFlight.current) return inFlight.current;
    setRunning(true);
    const promise = mobile.syncNow().then(
      next => {
        setStatus(next);
        setIssue(undefined);
        if (next.enrolled) onChange();
      },
      async failure => {
        setIssue(errorText(failure));
        // A cycle can finish its pull even when its push failed.
        onChange();
        try { setStatus(await mobile.syncStatus()); } catch { /* Preserve the cycle error. */ }
      },
    ).finally(() => {
      setRunning(false);
      inFlight.current = null;
    });
    inFlight.current = promise;
    return promise;
  }, [onChange]);

  useEffect(() => {
    const whenOpen = () => {
      if (document.visibilityState !== "hidden") void sync();
    };
    whenOpen();
    const timer = window.setInterval(whenOpen, SYNC_INTERVAL);
    document.addEventListener("visibilitychange", whenOpen);
    window.addEventListener("online", whenOpen);
    window.addEventListener("threestrands-resume", whenOpen);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", whenOpen);
      window.removeEventListener("online", whenOpen);
      window.removeEventListener("threestrands-resume", whenOpen);
    };
  }, [sync]);

  return { status, issue, running, sync, setStatus };
}
