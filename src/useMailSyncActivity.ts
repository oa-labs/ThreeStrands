import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import { logBackgroundFailure } from "./errors";

/** Payload of the backend's `mail-sync-activity` event. */
export type MailSyncActivityEvent = { accountId: string; active: boolean };

/**
 * Whether any account is checking its provider for mail, whether its polling
 * loop, the launch sync, or a manual refresh started it. `onFinished` hears
 * each account's sync ending so the caller can pick up its new status.
 */
export function useMailSyncActivity(onFinished?: (accountId: string) => void): boolean {
  const [activeAccounts, setActiveAccounts] = useState<ReadonlySet<string>>(() => new Set());
  const onFinishedRef = useRef(onFinished);
  onFinishedRef.current = onFinished;

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let disposed = false;
    // Accounts with an event since the snapshot request below went out; the
    // event is newer than whatever the snapshot says about them.
    const reported = new Set<string>();
    const unlisten = listen<MailSyncActivityEvent>("mail-sync-activity", ({ payload }) => {
      reported.add(payload.accountId);
      setActiveAccounts((current) => {
        if (current.has(payload.accountId) === payload.active) return current;
        const next = new Set(current);
        if (payload.active) next.add(payload.accountId);
        else next.delete(payload.accountId);
        return next;
      });
      if (!payload.active) onFinishedRef.current?.(payload.accountId);
    });
    // The launch sync may have started before this listener existed.
    void unlisten
      .then(() => mailClient.mailSyncActivity())
      .then((accountIds) => {
        if (disposed) return;
        const missed = accountIds.filter((accountId) => !reported.has(accountId));
        if (missed.length) setActiveAccounts((current) => new Set([...current, ...missed]));
      })
      .catch(logBackgroundFailure("Mail sync activity"));
    return () => {
      disposed = true;
      void unlisten.then((fn) => fn());
    };
  }, []);

  return activeAccounts.size > 0;
}
