// Phase 5 run 5: the ONE place a server-search outcome becomes user-facing
// words. The backend hands us typed CODES (see `SearchReport` in domain.ts) —
// never a developer string, a mailbox wire name in modified UTF-7, or the
// user's query echoed back — and this module turns a report into at most one
// quiet status line. Keeping the wording here (not in Rust) makes every string
// testable in vitest.

import type {
  PartialSearchReason,
  SearchReport,
} from "./domain";

/**
 * Shown while a search reaches past this device. Unified search can span Gmail
 * and IMAP accounts at once, so the wording never names a provider.
 */
export const SERVER_SEARCHING_COPY = "Searching mail on the server\u2026";

/** Shown when the server can't be searched; local matches stay on screen. */
export const SERVER_SEARCH_FAILED_COPY =
  "Server search unavailable. Showing mail on this device.";

/** Appended when some planned folders were not searched because they are not synced. */
const UNSYNCED_SUFFIX =
  "Some folders aren't synced, so they weren't searched.";

const PARTIAL_COPY: Record<PartialSearchReason["code"], string> = {
  deadline: "Some folders took too long to search. Showing what was found.",
  budget:
    "Showing the newest matches from the server. Narrow your search to find older ones.",
  folderError: "One folder couldn't be searched. Showing the rest.",
};

const SKIPPED_COPY =
  'This search uses something your mail server can\u2019t search for (like "-" or "OR"), so only mail already on this device is shown.';

/** Skipped for some accounts while others did search their server (unified search). */
const SKIPPED_SOME_COPY =
  'Some of your mail servers can\u2019t search for this (like "-" or "OR"). For those accounts, only mail already on this device is shown.';

const NO_SERVER_MATCHES_COPY =
  "No folder or label by that name on the server.";

/**
 * The single status line to show for a server-search report, or `null` when
 * there is nothing to say.
 *
 * `localResultEmpty` gates the `noServerMatches` message: a definite "nothing
 * by that name" is only worth showing when the local result is ALSO empty;
 * otherwise the user already sees matches and the note would be noise.
 *
 * When several partial reasons are present (a unified search over several
 * accounts), the FIRST is worded — one quiet line, not a list. The
 * unsynced-folders sentence is appended to any shown line (and stands alone
 * when the status is otherwise `complete` but folders were skipped).
 */
export function searchStatusMessage(
  report: SearchReport,
  localResultEmpty: boolean,
): string | null {
  const base = statusLine(report, localResultEmpty);
  const unsynced = report.unsyncedMailboxes > 0;

  if (base === null) {
    // `complete` (or a suppressed no-match) with skipped folders still earns
    // the unsynced note on its own.
    return unsynced ? UNSYNCED_SUFFIX : null;
  }
  return unsynced ? `${base} ${UNSYNCED_SUFFIX}` : base;
}

function statusLine(
  report: SearchReport,
  localResultEmpty: boolean,
): string | null {
  const status = report.status;
  switch (status.kind) {
    case "complete":
      return null;
    case "partial":
      // Word the first reason; the backend orders them by account.
      return PARTIAL_COPY[status.reasons[0]?.code ?? "deadline"];
    case "skipped":
      // In a unified search another account may still have searched its
      // server; never claim that only on-device mail is shown then.
      return report.serverSearchedAccounts > 0 ? SKIPPED_SOME_COPY : SKIPPED_COPY;
    case "noServerMatches":
      return localResultEmpty ? NO_SERVER_MATCHES_COPY : null;
  }
}
