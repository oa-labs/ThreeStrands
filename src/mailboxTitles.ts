import type { MailboxKind } from "./commands";
export const MAILBOX_TITLES: Record<MailboxKind, string> = {
  inbox: "Inbox",
  allMail: "All Mail",
  sent: "Sent",
  trash: "Trash",
  drafts: "Drafts",
  outbox: "Outbox",
  split: "Split Inbox",
};
