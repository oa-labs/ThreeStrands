import { describe, expect, it } from "vitest";
import { SERVER_SEARCH_FAILED_COPY, SERVER_SEARCHING_COPY, searchStatusMessage } from "./searchStatusCopy";
import type { SearchReport, SearchReportStatus } from "./domain";

function report(status: SearchReportStatus, counts?: Partial<Pick<SearchReport, "unsyncedMailboxes" | "skippedOverCap" | "serverSearchedAccounts">>): SearchReport {
  return {
    status,
    unsyncedMailboxes: 0,
    skippedOverCap: 0,
    serverSearchedAccounts: status.kind === "skipped" ? 0 : 1,
    ...counts,
  };
}

describe("searchStatusMessage", () => {
  it("does not claim only on-device mail when another account searched its server", () => {
    const mixed = report({ kind: "skipped", reasons: [{ code: "unsupported" }] }, { serverSearchedAccounts: 1 });
    expect(searchStatusMessage(mixed, false)).toBe(
      "Some of your mail servers can\u2019t search for this (like \"-\" or \"OR\"). For those accounts, only mail already on this device is shown.",
    );
  });

  it("shows nothing for a complete search", () => {
    expect(searchStatusMessage(report({ kind: "complete" }), false)).toBeNull();
    expect(searchStatusMessage(report({ kind: "complete" }), true)).toBeNull();
  });

  it("words a deadline partial", () => {
    expect(searchStatusMessage(report({ kind: "partial", reasons: [{ code: "deadline" }] }), false))
      .toBe("Some folders took too long to search. Showing what was found.");
  });

  it("words a budget partial", () => {
    expect(searchStatusMessage(report({ kind: "partial", reasons: [{ code: "budget" }] }), false))
      .toBe("Showing the newest matches from the server. Narrow your search to find older ones.");
  });

  it("words a folder-error partial", () => {
    expect(searchStatusMessage(report({ kind: "partial", reasons: [{ code: "folderError" }] }), false))
      .toBe("One folder couldn't be searched. Showing the rest.");
  });

  it("words a skipped search without echoing the query", () => {
    const message = searchStatusMessage(report({ kind: "skipped", reasons: [{ code: "unsupported" }] }), false);
    expect(message).toBe(
      'This search uses something your mail server can\u2019t search for (like "-" or "OR"), so only mail already on this device is shown.',
    );
  });

  it("shows no-server-matches ONLY when the local result is also empty", () => {
    const status: SearchReportStatus = { kind: "noServerMatches", reason: { code: "unknownName" } };
    expect(searchStatusMessage(report(status), true)).toBe("No folder or label by that name on the server.");
    expect(searchStatusMessage(report(status), false)).toBeNull();
  });

  it("appends the unsynced-folders sentence when the count is non-zero", () => {
    const message = searchStatusMessage(
      report({ kind: "partial", reasons: [{ code: "deadline" }] }, { unsyncedMailboxes: 2 }),
      false,
    );
    expect(message).toBe(
      "Some folders took too long to search. Showing what was found. Some folders aren't synced, so they weren't searched.",
    );
  });

  it("shows the unsynced sentence alone on an otherwise-complete search", () => {
    expect(searchStatusMessage(report({ kind: "complete" }, { unsyncedMailboxes: 1 }), false))
      .toBe("Some folders aren't synced, so they weren't searched.");
  });

  it("words the first reason when several accounts each contribute a partial", () => {
    const message = searchStatusMessage(
      report({ kind: "partial", reasons: [{ code: "folderError" }, { code: "budget" }] }),
      false,
    );
    expect(message).toBe("One folder couldn't be searched. Showing the rest.");
  });
});

describe("server search progress copy", () => {
  it("never names a provider, since one search can cover Gmail and IMAP accounts", () => {
    for (const copy of [SERVER_SEARCHING_COPY, SERVER_SEARCH_FAILED_COPY]) {
      expect(copy).not.toMatch(/gmail|google|imap|proton/i);
    }
    expect(SERVER_SEARCHING_COPY).toBe("Searching mail on the server…");
    expect(SERVER_SEARCH_FAILED_COPY).toBe("Server search unavailable. Showing mail on this device.");
  });
});
