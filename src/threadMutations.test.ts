import { describe, expect, it } from "vitest";
import type { Thread } from "./domain";
import {
  applyMutationTemplate,
  buildThreadMutation,
  describeMutation,
  invertMutationTemplate,
} from "./threadMutations";

const thread: Thread = {
  id: "thread-1",
  providerThreadId: "provider-1",
  subject: "Subject",
  snippet: "Snippet",
  participants: ["sender@example.com"],
  lastMessageAt: "2026-09-18T12:00:00Z",
  lastReceivedAt: "2026-09-18T12:00:00Z",
  unread: true,
  starred: false,
  archived: false,
  trashed: false,
  labels: ["INBOX"],
  accountId: "account@example.com",
  summary: null,
  summaryGeneratedAt: null,
  hasAttachments: false,
};

describe("thread mutations", () => {
  it.each([
    [{ kind: "archive", value: true } as const, { archived: true }],
    [{ kind: "trash", value: true } as const, { trashed: true }],
    [{ kind: "read", value: true } as const, { unread: false }],
    [{ kind: "star", value: true } as const, { starred: true }],
  ])("applies %s without changing unrelated thread state", (template, changes) => {
    expect(applyMutationTemplate(thread, template)).toMatchObject({ ...thread, ...changes });
  });

  it("moves spam threads out of the inbox and restores them back", () => {
    const spammed = applyMutationTemplate(thread, { kind: "spam", value: true });
    expect(spammed).toMatchObject({ archived: true, labels: ["SPAM"] });

    expect(applyMutationTemplate(spammed, { kind: "spam", value: false })).toMatchObject({
      archived: false,
      labels: ["INBOX"],
    });
  });

  it("adds and removes user labels and builds provider mutations", () => {
    const template = { kind: "label", labelId: "label-1", labelName: "Project", value: true } as const;
    expect(applyMutationTemplate(thread, template).labels).toEqual(["INBOX", "label-1"]);
    expect(applyMutationTemplate(thread, { ...template, value: false }).labels).toEqual(["INBOX"]);
    expect(buildThreadMutation("thread-1", template)).toEqual({
      kind: "label",
      threadId: "thread-1",
      labelId: "label-1",
      value: true,
    });
  });

  it("inverts a template and describes singular and batch actions", () => {
    const template = { kind: "archive", value: true } as const;
    expect(invertMutationTemplate(template)).toEqual({ kind: "archive", value: false });
    expect(describeMutation(template, 1)).toBe("Conversation archived");
    expect(describeMutation(template, 3)).toBe("Archived 3 conversations");
    expect(describeMutation({ kind: "label", labelId: "x", labelName: "Project", value: false }, 1, "Project")).toBe("Project removed");
  });
});
