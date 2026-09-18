import { describe, expect, it } from "vitest";
import type { Thread } from "./domain";
import { formatAttachmentSize, formatMailTimestamp, sortByRecency } from "./threadPresentation";

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
  hasAttachments: false,
});

describe("thread presentation helpers", () => {
  it("formats attachment sizes at each unit boundary", () => {
    expect(formatAttachmentSize(512)).toBe("512 B");
    expect(formatAttachmentSize(1024)).toBe("1 KB");
    expect(formatAttachmentSize(1024 * 1024)).toBe("1.0 MB");
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

