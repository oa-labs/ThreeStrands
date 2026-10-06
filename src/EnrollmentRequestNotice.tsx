import { useCallback, useEffect, useRef, useState } from "react";
import { Smartphone, X } from "lucide-react";
import { logBackgroundFailure } from "./errors";
import { replicatedSyncEnabled, replicatedSyncPendingRequests, type IncomingEnrollmentRequest } from "./replicatedSync";
import { useLiveStatus } from "./settingsOperations";

const reportFailure = logBackgroundFailure("Checking for devices waiting to join");

/**
 * An app-wide prompt when another device asks to join Replicated Sync, so
 * approving it does not depend on this device happening to have Settings
 * open. Refreshes on the same status event the sync loop emits after every
 * cycle. Dismissing hides the current requests only; a new one shows again.
 */
export function EnrollmentRequestNotice({ suppressed, onReview }: { suppressed: boolean; onReview(): void }) {
  const [requests, setRequests] = useState<IncomingEnrollmentRequest[]>([]);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set());
  const [resolvedNotice, setResolvedNotice] = useState(false);
  const previousRequestIds = useRef<Set<string> | null>(null);
  const dismissedIds = useRef<ReadonlySet<string>>(new Set());

  // A background prompt must never take the app down, so anything but a
  // well-formed list (an older native build, a failed call) means "none".
  const refresh = useCallback(async () => {
    const pending = (await replicatedSyncEnabled()) === true ? await replicatedSyncPendingRequests() : [];
    const next = Array.isArray(pending) ? pending : [];
    const previous = previousRequestIds.current;
    if (previous && [...previous].some((requestId) => !next.some((request) => request.requestId === requestId) && !dismissedIds.current.has(requestId))) {
      setResolvedNotice(true);
    }
    previousRequestIds.current = new Set(next.map((request) => request.requestId));
    setRequests(next);
  }, []);
  useLiveStatus("replicated-sync-status", refresh, reportFailure);

  useEffect(() => {
    if (!resolvedNotice) return;
    const timeout = window.setTimeout(() => setResolvedNotice(false), 5_000);
    return () => window.clearTimeout(timeout);
  }, [resolvedNotice]);

  const visible = requests.filter((request) => !dismissed.has(request.requestId));
  if (suppressed) return null;
  if (visible.length === 0) {
    if (!resolvedNotice) return null;
    return (
      <div className="toast enrollment-request-toast" role="status" aria-live="polite">
        <Smartphone className="enrollment-request-icon" size={18} aria-hidden="true" />
        <span className="enrollment-request-message">A device request was resolved. No action is needed here.</span>
        <button className="btn-icon btn-icon-sm" aria-label="Dismiss status message" onClick={() => setResolvedNotice(false)}><X size={14} /></button>
      </div>
    );
  }

  return (
    <div className="toast enrollment-request-toast" role="status" aria-live="polite">
      <Smartphone className="enrollment-request-icon" size={18} aria-hidden="true" />
      <span className="enrollment-request-message">
        {visible.length === 1
          ? "A new device is asking to join Replicated Sync."
          : `${visible.length} devices are asking to join Replicated Sync.`}
      </span>
      <button className="btn-link" onClick={onReview}>Review</button>
      <button className="btn-icon btn-icon-sm"
        aria-label="Dismiss on this device"
        onClick={() => setDismissed((current) => {
          const next = new Set([...current, ...visible.map((request) => request.requestId)]);
          dismissedIds.current = next;
          return next;
        })}
      >
        <X size={14} />
      </button>
    </div>
  );
}
