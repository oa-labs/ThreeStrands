import { CircleAlert, RefreshCw, X } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";
import { SERVER_SEARCH_FAILED_COPY, SERVER_SEARCHING_COPY } from "./searchStatusCopy";

/**
 * The quiet status line for an all-mail search that reaches the server: a
 * spinner while it runs, a notice when the server can't be reached, or the
 * worded outcome of a partial or skipped search.
 */
export function SearchStatusToast({ remoteSearchState, status, onDismiss }: {
  remoteSearchState: "idle" | "searching" | "error";
  status: string | null;
  onDismiss(): void;
}) {
  if (remoteSearchState === "searching") {
    return (
      <div className="toast search-status-toast" role="status" aria-live="polite">
        <RefreshCw size={ICON_SIZE.sm} className="spin" />
        {SERVER_SEARCHING_COPY}
      </div>
    );
  }
  if (remoteSearchState === "error") {
    return (
      <div className="toast search-status-toast error" role="status" aria-live="polite">
        <CircleAlert size={ICON_SIZE.sm} />
        {SERVER_SEARCH_FAILED_COPY}
      </div>
    );
  }
  if (!status) return null;
  return (
    <div className="toast search-status-toast" role="status" aria-live="polite">
      {status}
      <button className="btn-icon btn-icon-sm" aria-label="Dismiss" onClick={onDismiss}><X size={ICON_SIZE.sm} /></button>
    </div>
  );
}
