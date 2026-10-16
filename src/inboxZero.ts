import type { SyncStatus } from "./domain";
import { formatMailTimestamp } from "./threadPresentation";

/**
 * The line under "Inbox zero" that says whether the empty inbox is current.
 * An empty list looks the same whether mail is done or sync has stalled, so
 * this names the last successful check, or says plainly that it failed.
 */
export function inboxCheckedLabel(status: SyncStatus | null, now = new Date()): string | null {
  const last = status?.lastSuccessfulSync ? formatMailTimestamp(status.lastSuccessfulSync, now) : null;
  switch (status?.state) {
    case "syncing":
      return "Checking for new mail…";
    case "error":
      return "Couldn't check for new mail";
    case "offline":
      return last ? `Offline · Last checked ${last}` : "Offline";
    default:
      return last ? `Last checked ${last}` : null;
  }
}
