import type { Thread, TodayMail } from "../domain";

export function todayThread(id: string, overrides: Partial<Thread> = {}): Thread {
  return {
    id, providerThreadId: id, subject: `Subject ${id}`, snippet: "", participants: ["Someone"],
    lastMessageAt: "2026-10-23T09:00:00Z", lastReceivedAt: "2026-10-23T09:00:00Z", unread: true, starred: false,
    archived: false, trashed: false, labels: ["INBOX", "UNREAD"], accountId: "me@example.com", summary: null,
    summaryGeneratedAt: null, summaryRevision: null, hasAttachments: false, ...overrides,
  };
}

export function todayItem(id: string, overrides: Partial<TodayMail> = {}, thread: Partial<Thread> = {}): TodayMail {
  return {
    thread: todayThread(id, thread), sender: `Person ${id} <${id}@example.org>`,
    listUnsubscribe: false, addressedToMe: true, toCount: 1, hasInvitation: false, corresponded: true, ...overrides,
  };
}
