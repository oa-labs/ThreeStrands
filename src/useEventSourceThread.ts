import { useEffect, useState } from "react";
import { mailClient } from "./data/client";

/**
 * The conversation an event was added from, when its source email is cached on
 * this device. Resolved by the same backend rule as the Conversations list, so
 * a message that matches no single thread yields null. A result belongs to the
 * message it was asked for, so switching events never shows the last one's.
 */
export function useEventSourceThread(sourceMessageId: string | null | undefined, accountIds: string[]): string | null {
  const [found, setFound] = useState<{ key: string; threadId: string | null } | null>(null);
  const accountKey = JSON.stringify(accountIds);
  const key = sourceMessageId ? `${accountKey}:${sourceMessageId}` : "";
  useEffect(() => {
    if (!sourceMessageId) return;
    let active = true;
    void mailClient.contextCandidates({ accountIds: JSON.parse(accountKey) as string[], sourceMessageId, terms: [], attendees: [] })
      .then((rows) => { if (active) setFound({ key, threadId: rows.find((row) => row.source)?.threadId ?? null }); })
      .catch(() => { if (active) setFound({ key, threadId: null }); });
    return () => { active = false; };
  }, [key, accountKey, sourceMessageId]);
  return key && found?.key === key ? found.threadId : null;
}
