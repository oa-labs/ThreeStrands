import { describe, expect, it } from "vitest";
import type { Draft, OutboxItem, ScheduledSendReport } from "./correspondence";
import { adjacentOutboxKey, checkableOutboxKeys, isOutgoing, outboxBatchAbility, outboxEditAsNewSource, outboxEntries, outboxEntryTime, selectedOutboxEntry, type OutboxEntry } from "./outboxView";

const draft: Draft = {
  id: "draft", revision: 1, account: "me@example.com", mode: "new", sourceId: null, threadId: null,
  replyId: null, references: [], to: "", cc: "", bcc: "", subject: "", body: "", attachments: [], updatedAt: 500,
};
const item = (id: string, state: OutboxItem["state"], extra: Partial<OutboxItem> = {}): OutboxItem =>
  ({ id, draft, state, deadline: 0, error: null, ...extra });
const local = (value: OutboxItem): OutboxEntry => ({ key: `local:${value.id}`, kind: "local", item: value });
const report: ScheduledSendReport = {
  operationId: "remote", ownerInstallationId: "peer", ownerSyncDeviceId: null, ownerNameAtCreation: "MacBook",
  account: "me@example.com", subject: "Remote", scheduledAt: 900, timeZone: "UTC", state: "scheduled",
  blockedReason: null, reportRevision: 1, statusChangedAt: 1,
};

describe("outboxEntries", () => {
  it("lists this computer's outgoing sends before peer schedules, leaving finished and canceled ones out", () => {
    const entries = outboxEntries(
      [item("a", "undo_pending"), item("b", "canceled"), item("c", "failed"), item("d", "sent")],
      [report, { ...report, operationId: "gone", state: "canceled" }, { ...report, operationId: "done", state: "sent" }],
    );
    expect(entries.map((entry) => entry.key)).toEqual(["local:a", "local:c", "peer:peer:remote"]);
  });
});

describe("isOutgoing", () => {
  it.each(["scheduled", "overdue", "undo_pending", "ready", "sending", "failed", "uncertain", "unverifiable"] as const)(
    "keeps a %s send in the Outbox",
    (state) => expect(isOutgoing(item("a", state))).toBe(true),
  );

  it("moves a delivered send to Sent once its Sent copy is saved, or when the server keeps its own", () => {
    expect(isOutgoing(item("a", "sent"))).toBe(false);
    expect(isOutgoing(item("a", "sent", { sentCopy: null }))).toBe(false);
    expect(isOutgoing(item("a", "sent", { sentCopy: { state: "done" } }))).toBe(false);
    expect(isOutgoing(item("a", "canceled"))).toBe(false);
  });

  it("keeps a delivered send while its Sent copy is saving or could not be saved", () => {
    expect(isOutgoing(item("a", "sent", { sentCopy: { state: "pending" } }))).toBe(true);
    expect(isOutgoing(item("a", "sent", { sentCopy: { state: "verify" } }))).toBe(true);
    expect(isOutgoing(item("a", "sent", { sentCopy: { state: "abandoned", error: "No Sent folder" } }))).toBe(true);
  });
});

describe("Outbox selection", () => {
  const entries = outboxEntries([item("a", "failed"), item("b", "failed"), item("c", "failed")], []);

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
    const sent = local(item("a", "sent", { providerId: "provider-a", sentCopy: { state: "abandoned" } }));
    expect(outboxEditAsNewSource(sent)).toEqual({ messageId: "provider-a", account: "me@example.com" });
    for (const state of ["undo_pending", "failed", "uncertain", "scheduled"] as const) {
      expect(outboxEditAsNewSource(local(item("a", state, { providerId: "provider-a" })))).toBeNull();
    }
    expect(outboxEditAsNewSource(local(item("a", "sent")))).toBeNull();
    expect(outboxEditAsNewSource(outboxEntries([], [report])[0])).toBeNull();
    expect(outboxEditAsNewSource(null)).toBeNull();
  });
});

describe("outboxEntryTime", () => {
  it("uses the scheduled time when there is one, otherwise the last save", () => {
    expect(outboxEntryTime(local(item("a", "failed")))).toBe(500);
    expect(outboxEntryTime(outboxEntries([item("a", "scheduled", { schedule: { report, canManage: true, visibility: "local" } })], [])[0])).toBe(900);
    expect(outboxEntryTime(outboxEntries([], [report])[0])).toBe(900);
  });
});

describe("outboxBatchAbility", () => {
  it.each(["undo_pending", "ready", "scheduled", "overdue"] as const)("lets a %s send return to Drafts", (state) => {
    expect(outboxBatchAbility(local(item("a", state)))).toEqual({ returnable: true, discardable: false });
  });
  it("lets only a failed send be discarded", () => {
    expect(outboxBatchAbility(local(item("a", "failed")))).toEqual({ returnable: false, discardable: true });
  });
  it.each(["sending", "uncertain", "unverifiable", "sent"] as const)("leaves a %s send out of batch actions", (state) => {
    expect(outboxBatchAbility(local(item("a", state)))).toEqual({ returnable: false, discardable: false });
  });
  it("leaves out schedules this computer can't manage and other computers' reports", () => {
    const schedule = { report, canManage: false, visibility: "local" };
    expect(outboxBatchAbility(local(item("a", "scheduled", { schedule })))).toEqual({ returnable: false, discardable: false });
    expect(outboxBatchAbility({ key: "peer:x", kind: "peer", report })).toEqual({ returnable: false, discardable: false });
  });
  it("lists the keys a batch action can check, in display order", () => {
    const entries = outboxEntries([item("a", "failed"), item("b", "sending"), item("c", "scheduled")], [report]);
    expect(checkableOutboxKeys(entries)).toEqual(["local:a", "local:c"]);
  });
});
