import { useEffect, useState } from "react";
import { RecentEmailsSection } from "./ContextSections";
import { mailClient } from "./data/client";
import type { ContactTimelineItem } from "./domain";
import { errorMessage } from "./errors";

const PAGE_SIZE = 20;

/** Remount when membership changes so paging always describes the current group. */
export function GroupRecentEmails({ groupId, accountId, onOpenThread }: {
  groupId: string;
  accountId: string | null;
  onOpenThread(id: string): void;
}) {
  const [items, setItems] = useState<ContactTimelineItem[]>([]);
  const [offset, setOffset] = useState(0);
  const [retry, setRetry] = useState(0);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    void mailClient.contactGroupTimeline(groupId, offset, PAGE_SIZE, accountId ?? undefined).then((page) => {
      if (!active) return;
      setItems(current => offset === 0 ? page : [...current, ...page]);
      setHasMore(page.length === PAGE_SIZE);
    }).catch(reason => {
      if (active) setError(errorMessage(reason));
    }).finally(() => {
      if (active) setLoading(false);
    });
    return () => { active = false; };
  }, [groupId, accountId, offset, retry]);

  return <RecentEmailsSection
    items={items}
    limit={8}
    onOpenThread={onOpenThread}
    onLoadOlder={hasMore && !loading && !error ? () => setOffset(items.length) : undefined}
    closing={<>
      {loading ? <p className="context-status" role="status">Loading emails…</p>
        : error ? <div><p className="context-status contacts-error" role="alert">{error}</p><button type="button" className="btn-link context-link-button" onClick={() => setRetry(value => value + 1)}>Retry loading emails</button></div>
        : !items.length ? <p className="context-status">No emails with current group members{accountId ? " in this account" : ""}.</p> : null}
    </>}
  />;
}
