import { useEffect, useMemo, type Dispatch, type SetStateAction } from "react";
import type { OutboxItem } from "./correspondence";
import type { Message, ThreadDetail } from "./domain";
import { mailClient } from "./data/client";
import { logBackgroundFailure } from "./errors";

/**
 * Waits between re-reads of the open conversation while a reply that has
 * already been sent still shows as its local outbox copy. The outbox reports
 * a send before the provider's copy reaches the thread cache, so the reload
 * that follows a send can miss it.
 */
export const SENT_REPLY_REFRESH_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 15_000, 30_000];

/**
 * Re-reads the open conversation until each sent reply's provider copy
 * replaces its local outbox copy.
 */
export function useSentReplyRefresh(
  detail: ThreadDetail | null,
  displayedMessages: Message[],
  outbox: OutboxItem[],
  setDetail: Dispatch<SetStateAction<ThreadDetail | null>>,
) {
  const threadId = detail?.thread.id ?? null;
  const awaitingKey = useMemo(() => {
    const sent = new Set(outbox.filter((item) => item.state === "sent").map((item) => `outbox-${item.id}`));
    return displayedMessages.filter((message) => sent.has(message.id)).map((message) => message.id).join(",");
  }, [displayedMessages, outbox]);

  useEffect(() => {
    if (!threadId || !awaitingKey) return;
    let cancelled = false;
    let attempt = 0;
    let timer: number | undefined;
    const schedule = () => {
      // Sent-folder sync can finish after the initial backoff window. Keep
      // checking the local cache at the capped interval while this copy is visible.
      const delay = SENT_REPLY_REFRESH_DELAYS_MS[Math.min(attempt++, SENT_REPLY_REFRESH_DELAYS_MS.length - 1)]!;
      timer = window.setTimeout(() => {
        void mailClient.getThread(threadId)
          .then((next) => {
            if (!cancelled) setDetail((current) => current?.thread.id === threadId ? next : current);
          })
          .catch(logBackgroundFailure("Sent reply refresh"))
          .finally(() => { if (!cancelled) schedule(); });
      }, delay);
    };
    schedule();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [threadId, awaitingKey, setDetail]);
}
