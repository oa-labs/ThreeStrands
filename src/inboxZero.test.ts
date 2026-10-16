import { describe, expect, it } from "vitest";
import type { SyncStatus } from "./domain";
import { inboxCheckedLabel } from "./inboxZero";
import { formatMailTimestamp } from "./threadPresentation";

const now = new Date("2026-10-16T15:00:00");
const today = new Date("2026-10-16T14:42:00").toISOString();
const status = (state: SyncStatus["state"], lastSuccessfulSync: string | null): SyncStatus => ({
  state, lastSuccessfulSync, cursor: null, pendingMutations: 0, failedMutations: [], quarantinedMessages: [], error: null,
});

describe("inbox zero checked label", () => {
  it("names the last successful check so an empty inbox reads as current", () => {
    expect(inboxCheckedLabel(status("idle", today), now)).toBe(`Last checked ${formatMailTimestamp(today, now)}`);
  });

  it("shows the date, not just a time, when the last check was on an earlier day", () => {
    const earlier = new Date("2026-10-14T09:00:00").toISOString();
    expect(inboxCheckedLabel(status("idle", earlier), now)).toBe(`Last checked ${formatMailTimestamp(earlier, now)}`);
    expect(formatMailTimestamp(earlier, now)).not.toBe(formatMailTimestamp(today, now));
  });

  it("does not claim the inbox is current while checking, offline, or after a failed check", () => {
    expect(inboxCheckedLabel(status("syncing", today), now)).toBe("Checking for new mail…");
    expect(inboxCheckedLabel(status("error", today), now)).toBe("Couldn't check for new mail");
    expect(inboxCheckedLabel(status("offline", today), now)).toBe(`Offline · Last checked ${formatMailTimestamp(today, now)}`);
    expect(inboxCheckedLabel(status("offline", null), now)).toBe("Offline");
  });

  it("says nothing when there has never been a check to report", () => {
    expect(inboxCheckedLabel(status("idle", null), now)).toBeNull();
    expect(inboxCheckedLabel(null, now)).toBeNull();
  });
});
