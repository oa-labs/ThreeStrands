import { describe, expect, it } from "vitest";
import type { Draft, OutboxItem, ScheduledSendReport } from "./correspondence";
import { adjacentOutboxKey, outboxEditAsNewSource, outboxEntries, outboxEntryTime, selectedOutboxEntry } from "./outboxView";

const draft: Draft = {
  id: "draft", revision: 1, account: "me@example.com", mode: "new", sourceId: null, threadId: null,
  replyId: null, references: [], to: "", cc: "", bcc: "", subject: "", body: "", attachments: [], updatedAt: 500,
};
const item = (id: string, state: OutboxItem["state"], extra: Partial<OutboxItem> = {}): OutboxItem =>
  ({ id, draft, state, deadline: 0, error: null, ...extra });
const report: ScheduledSendReport = {
  operationId: "remote", ownerInstallationId: "peer", ownerSyncDeviceId: null, ownerNameAtCreation: "MacBook",
  account: "me@example.com", subject: "Remote", scheduledAt: 900, timeZone: "UTC", state: "scheduled",
  blockedReason: null, reportRevision: 1, statusChangedAt: 1,
};

describe("outboxEntries", () => {
  it("lists this computer's sends before peer schedules and hides canceled ones", () => {
    const entries = outboxEntries(
      [item("a", "sent"), item("b", "canceled"), item("c", "failed")],
      [report, { ...report, operationId: "gone", state: "canceled" }],
    );
    expect(entries.map((entry) => entry.key)).toEqual(["local:a", "local:c", "peer:peer:remote"]);
  });
});

describe("Outbox selection", () => {
  const entries = outboxEntries([item("a", "sent"), item("b", "sent"), item("c", "sent")], []);

  it("shows the first row until another is chosen, and again once the chosen row leaves", () => {
    expect(selectedOutboxEntry(entries, null)?.key).toBe("local:a");
    expect(selectedOutboxEntry(entries, "local:b")?.key).toBe("local:b");
    expect(selectedOutboxEntry(entries, "local:gone")?.key).toBe("local:a");
    expect(selectedOutboxEntry([], null)).toBeNull();
  });

  it("steps through rows and stops at either end", () => {
    expect(adjacentOutboxKey(entries, null, 1)).toBe("local:b");
    expect(adjacentOutboxKey(entries, "local:b", 1)).toBe("local:c");
    expect(adjacentOutboxKey(entries, "local:c", 1)).toBe("local:c");
    expect(adjacentOutboxKey(entries, "local:b", -1)).toBe("local:a");
    expect(adjacentOutboxKey(entries, "local:a", -1)).toBe("local:a");
    expect(adjacentOutboxKey([], null, 1)).toBeNull();
  });
});

describe("outboxEditAsNewSource", () => {
  it("copies only a delivered send whose provider message is known", () => {
    const [sent] = outboxEntries([item("a", "sent", { providerId: "provider-a" })], []);
    expect(outboxEditAsNewSource(sent)).toEqual({ messageId: "provider-a", account: "me@example.com" });
    for (const state of ["undo_pending", "failed", "uncertain", "scheduled"] as const) {
      expect(outboxEditAsNewSource(outboxEntries([item("a", state, { providerId: "provider-a" })], [])[0])).toBeNull();
    }
    expect(outboxEditAsNewSource(outboxEntries([item("a", "sent")], [])[0])).toBeNull();
    expect(outboxEditAsNewSource(outboxEntries([], [report])[0])).toBeNull();
    expect(outboxEditAsNewSource(null)).toBeNull();
  });
});

describe("outboxEntryTime", () => {
  it("uses the scheduled time when there is one, otherwise the last save", () => {
    expect(outboxEntryTime(outboxEntries([item("a", "sent")], [])[0])).toBe(500);
    expect(outboxEntryTime(outboxEntries([item("a", "scheduled", { schedule: { report, canManage: true, visibility: "local" } })], [])[0])).toBe(900);
    expect(outboxEntryTime(outboxEntries([], [report])[0])).toBe(900);
  });
});
