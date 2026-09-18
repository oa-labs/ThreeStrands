import type { Thread, ThreadMutation } from "./domain";

export type MutationTemplate =
  | { kind: "archive"; value: boolean }
  | { kind: "trash"; value: boolean }
  | { kind: "spam"; value: boolean }
  | { kind: "read"; value: boolean }
  | { kind: "star"; value: boolean }
  | { kind: "label"; labelId: string; labelName: string; value: boolean };

export function buildThreadMutation(threadId: string, template: MutationTemplate): ThreadMutation {
  return template.kind === "label"
    ? { kind: "label", threadId, labelId: template.labelId, value: template.value }
    : { kind: template.kind, threadId, value: template.value };
}

export function applyMutationTemplate(thread: Thread, template: MutationTemplate): Thread {
  switch (template.kind) {
    case "archive":
      return { ...thread, archived: template.value };
    case "trash":
      return { ...thread, trashed: template.value };
    case "spam": {
      const next = new Set(thread.labels);
      if (template.value) {
        next.add("SPAM");
        next.delete("INBOX");
      } else {
        next.delete("SPAM");
        next.add("INBOX");
      }
      return { ...thread, archived: template.value, labels: [...next] };
    }
    case "read":
      return { ...thread, unread: !template.value };
    case "star":
      return { ...thread, starred: template.value };
    case "label": {
      const next = new Set(thread.labels);
      if (template.value) next.add(template.labelId);
      else next.delete(template.labelId);
      return { ...thread, labels: [...next] };
    }
  }
}

export function invertMutationTemplate(template: MutationTemplate): MutationTemplate {
  return { ...template, value: !template.value } as MutationTemplate;
}

export function describeMutation(template: MutationTemplate, count: number, labelName?: string): string {
  const many = count > 1;
  switch (template.kind) {
    case "archive":
      return template.value
        ? (many ? `Archived ${count} conversations` : "Conversation archived")
        : (many ? `Marked ${count} conversations as not done` : "Conversation marked as not done");
    case "trash":
      return template.value
        ? (many ? `Moved ${count} conversations to trash` : "Conversation moved to trash")
        : (many ? `Restored ${count} conversations from trash` : "Conversation restored from trash");
    case "spam":
      return template.value
        ? (many ? `Marked ${count} conversations as spam` : "Conversation marked as spam")
        : (many ? `Restored ${count} conversations from spam` : "Conversation restored from spam");
    case "star":
      return template.value
        ? (many ? `Starred ${count} conversations` : "Starred")
        : (many ? `Unstarred ${count} conversations` : "Unstarred");
    case "read":
      return template.value
        ? (many ? `Marked ${count} conversations as read` : "Marked as read")
        : (many ? `Marked ${count} conversations as unread` : "Marked as unread");
    case "label": {
      const name = labelName ?? "Label";
      return template.value
        ? (many ? `${name} added to ${count} conversations` : `${name} added`)
        : (many ? `${name} removed from ${count} conversations` : `${name} removed`);
    }
  }
}

