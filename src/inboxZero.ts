import type { Account, SyncStatus } from "./domain";
import { formatMailTimestamp } from "./threadPresentation";

export type InboxCheckedState = {
  text: string;
  /** The account in view needs signing in again before mail can be checked. */
  needsReconnect: boolean;
};

/**
 * The line under "Inbox zero" that says whether the empty inbox is current.
 * An empty list looks the same whether mail is done or sync has stalled, so
 * this names the last successful check of the accounts in view, or says
 * plainly why it could not check. Only those accounts count: another
 * account's trouble, or a queued action that failed, says nothing about
 * whether this inbox is current.
 */
export function inboxCheckedLabel(
  status: SyncStatus | null,
  accounts: Pick<Account, "email" | "status" | "lastSyncedAt">[],
  now = new Date(),
): InboxCheckedState | null {
  if (accounts.length === 0) return null;
  const single = accounts.length === 1;
  const reconnect = accounts.filter((account) => account.status === "needs_reauth");
  if (reconnect.length > 0) {
    const text = single
      ? "Reconnect this account to check for new mail"
      : reconnect.length === 1
        ? `Reconnect ${reconnect[0].email} to check for new mail`
        : `Reconnect ${reconnect.length} accounts to check for new mail`;
    return { text, needsReconnect: true };
  }
  if (status?.state === "syncing") return { text: "Checking for new mail…", needsReconnect: false };

  // The oldest check is the honest one: the inbox is only as current as its least recent account.
  const checks = accounts.map((account) => account.lastSyncedAt);
  const oldest = checks.some((check) => !check) ? null : checks.reduce((left, right) => (left! < right! ? left : right));
  const last = oldest ? `Last checked ${formatMailTimestamp(oldest, now)}` : null;
  const failing = accounts.filter((account) => status?.fetchErrors?.some((error) => error.accountId === account.email));
  if (failing.length > 0) {
    const text = single || failing.length === accounts.length ? "Couldn't check for new mail" : `Couldn't check ${failing[0].email} for new mail`;
    return { text: last ? `${text} · ${last}` : text, needsReconnect: false };
  }
  if (status?.state === "offline") return { text: last ? `Offline · ${last}` : "Offline", needsReconnect: false };
  return last ? { text: last, needsReconnect: false } : null;
}
