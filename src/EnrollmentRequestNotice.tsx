import { useCallback, useState } from "react";
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

  // A background prompt must never take the app down, so anything but a
  // well-formed list (an older native build, a failed call) means "none".
  const refresh = useCallback(async () => {
    const pending = (await replicatedSyncEnabled()) === true ? await replicatedSyncPendingRequests() : [];
    setRequests(Array.isArray(pending) ? pending : []);
  }, []);
  useLiveStatus("replicated-sync-status", refresh, reportFailure);

  const visible = requests.filter((request) => !dismissed.has(request.requestId));
  if (suppressed || visible.length === 0) return null;

  return (
    <div className="toast enrollment-request-toast" role="status" aria-live="polite">
      <Smartphone className="enrollment-request-icon" size={18} aria-hidden="true" />
      <span className="enrollment-request-message">
        {visible.length === 1
          ? "A new device is asking to join Replicated Sync."
          : `${visible.length} devices are asking to join Replicated Sync.`}
      </span>
      <button onClick={onReview}>Review</button>
      <button
        aria-label="Dismiss on this device"
        onClick={() => setDismissed((current) => new Set([...current, ...visible.map((request) => request.requestId)]))}
      >
        <X size={14} />
      </button>
    </div>
  );
}
