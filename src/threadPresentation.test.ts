import { describe, expect, it } from "vitest";
import type { Thread } from "./domain";
import { formatAttachmentSize, formatMailTimestamp, isSummaryStale, sortByRecency, splitAttachmentName } from "./threadPresentation";

const thread = (id: string, received: string): Thread => ({
  id,
  providerThreadId: id,
  subject: id,
  snippet: id,
  participants: [],
  lastMessageAt: received,
  lastReceivedAt: received,
  unread: false,
  starred: false,
  archived: false,
  trashed: false,
  labels: [],
  accountId: "account@example.com",
  summary: null,
  summaryGeneratedAt: null,
  summaryRevision: null,
  hasAttachments: false,
});

describe("thread presentation helpers", () => {
  it("judges a summary stale against the revision it was written from", () => {
    const base = { ...thread("t", "2026-09-19T10:30:00Z"), summary: "- Brief" };
    // Mail newer than the revision is stale even if it predates the save.
    expect(isSummaryStale({ ...base, summaryRevision: "2026-09-19T10:00:00Z", summaryGeneratedAt: "2026-09-19T11:00:00Z" })).toBe(true);
    expect(isSummaryStale({ ...base, summaryRevision: "2026-09-19T10:30:00Z", summaryGeneratedAt: "2026-09-19T11:00:00Z" })).toBe(false);
    // Summaries saved before revisions were recorded use their generation time.
    expect(isSummaryStale({ ...base, summaryRevision: null, summaryGeneratedAt: "2026-09-19T10:00:00Z" })).toBe(true);
    expect(isSummaryStale({ ...base, summaryRevision: null, summaryGeneratedAt: "2026-09-19T11:00:00Z" })).toBe(false);
    expect(isSummaryStale({ ...base, summary: null, summaryRevision: "2026-09-19T10:00:00Z" })).toBe(false);
  });

  it("formats attachment sizes at each unit boundary", () => {
    expect(formatAttachmentSize(512)).toBe("512 B");
    expect(formatAttachmentSize(1024)).toBe("1 KB");
    expect(formatAttachmentSize(1024 * 1024)).toBe("1.0 MB");
  });

  it("splits attachment names so the extension can stay visible while the base truncates", () => {
    expect(splitAttachmentName("ISSF SOW 48 - 9_24_26, 11_27 AM.pdf")).toEqual({
      base: "ISSF SOW 48 - 9_24_26, 11_27 AM",
      extension: ".pdf",
    });
    expect(splitAttachmentName("archive.tar.gz")).toEqual({ base: "archive.tar", extension: ".gz" });
    expect(splitAttachmentName("README")).toEqual({ base: "README", extension: "" });
    expect(splitAttachmentName(".gitignore")).toEqual({ base: ".gitignore", extension: "" });
    expect(splitAttachmentName("invoice.")).toEqual({ base: "invoice.", extension: "" });
    expect(splitAttachmentName("itinerary.backup-2026-09")).toEqual({ base: "itinerary.backup-2026-09", extension: "" });
  });

  it("formats today and older mail differently", () => {
    const now = new Date("2026-09-18T15:30:00");
    expect(formatMailTimestamp("2026-09-18T08:05:00", now)).toBe(new Intl.DateTimeFormat(undefined, {
      hour: "numeric",
      minute: "2-digit",
    }).format(new Date("2026-09-18T08:05:00")));
    expect(formatMailTimestamp("2026-09-17T08:05:00", now)).toBe(new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "2-digit",
    }).format(new Date("2026-09-17T08:05:00")));
  });

  it("sorts without mutating the source array", () => {
    const original = [thread("older", "2026-09-17T00:00:00Z"), thread("newer", "2026-09-18T00:00:00Z")];
    expect(sortByRecency(original).map(({ id }) => id)).toEqual(["newer", "older"]);
    expect(original.map(({ id }) => id)).toEqual(["older", "newer"]);
  });
});

