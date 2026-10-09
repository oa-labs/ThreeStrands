import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { listen } from "@tauri-apps/api/event";
import type { MailboxKind } from "./commands";
import type { Thread } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage, logBackgroundFailure } from "./errors";

const SEARCH_PAGE_SIZE = 50;
// While a Gmail backfill scan is running, matches land in the local cache
// incrementally (see sync.rs's flushed ingest_threads batches), so poll
// local search at this cadence rather than waiting for the whole scan to
// finish before a match becomes visible.
const REMOTE_SEARCH_POLL_MS = 1200;

type Options = {
  activeAccountId: string | null;
  mailbox: MailboxKind;
  activeSplitInboxId: string | null;
  setSelectedId: Dispatch<SetStateAction<string | null>>;
  refreshUnreadCounts: () => void;
  refreshMailboxUnreadCounts: () => void;
};

/** Owns mailbox pages and local/remote search, including stale-response guards. */
export function useMailboxThreads({
  activeAccountId, mailbox, activeSplitInboxId, setSelectedId, refreshUnreadCounts,
  refreshMailboxUnreadCounts,
}: Options) {
  const [threads, setThreads] = useState<Thread[]>([]);
  const [query, setQuery] = useState("");
  const queryRef = useRef(query);
  queryRef.current = query;
  const [searchOpen, setSearchOpen] = useState(false);
  const [includeArchived, setIncludeArchived] = useState(false);
  const [remoteSearchState, setRemoteSearchState] = useState<"idle" | "searching" | "error">("idle");
  const [hasMoreResults, setHasMoreResults] = useState(false);
  const [loading, setLoading] = useState(true);
  const [mailboxError, setMailboxError] = useState("");
  const threadsRequest = useRef(0);
  const contextOpenedThreadRef = useRef<string | null>(null);
  const loadingMore = useRef(false);
  const [loadingMoreState, setLoadingMoreState] = useState(false);
  const remoteSearchKeyRef = useRef<string | null>(null);
  const remoteSearchRequestRef = useRef(0);
  const remoteSearchPromiseRef = useRef<Promise<void> | null>(null);
  const remoteSearchInFlightRef = useRef(false);
  const fetchMailboxPage = useCallback((box: MailboxKind, accountId: string | undefined, offset: number) => {
    if (box === "allMail") return mailClient.listAllMailPage(accountId, offset, SEARCH_PAGE_SIZE);
    if (box === "trash") return mailClient.listTrashPage(accountId, offset, SEARCH_PAGE_SIZE);
    if (box === "split" && activeSplitInboxId) {
      return mailClient.listSplitInboxPage(activeSplitInboxId, offset, SEARCH_PAGE_SIZE);
    }
    return mailClient.listThreadsPage(accountId, offset, SEARCH_PAGE_SIZE);
  }, [activeSplitInboxId]);

  const loadThreads = useCallback(async (search: string, accountOverride?: string | null, mailboxOverride?: MailboxKind) => {
    const requestId = ++threadsRequest.current;
    const box = mailboxOverride ?? mailbox;
    if (box === "drafts" || box === "outbox" || (box === "split" && !activeSplitInboxId)) {
      setThreads([]);
      setSelectedId(null);
      setHasMoreResults(false);
      setMailboxError("");
      return;
    }
    setMailboxError("");
    const trimmed = search.trim();
    const accountId = (accountOverride !== undefined ? accountOverride : activeAccountId) ?? undefined;
    const searchableMailbox = box === "inbox" || box === "split";
    if (!searchableMailbox || !trimmed || !includeArchived) {
      remoteSearchKeyRef.current = null;
      remoteSearchRequestRef.current += 1;
      remoteSearchPromiseRef.current = null;
      remoteSearchInFlightRef.current = false;
      setRemoteSearchState("idle");
    }
    const commitPage = (page: { threads: Thread[]; hasMore: boolean }) => {
      if (requestId !== threadsRequest.current) return false;
      setMailboxError("");
      setThreads(page.threads);
      setHasMoreResults(page.hasMore);
      refreshUnreadCounts();
      refreshMailboxUnreadCounts();
      setSelectedId((current) =>
        current && (page.threads.some((thread) => thread.id === current) || contextOpenedThreadRef.current === current)
          ? current
          : (page.threads[0]?.id ?? null),
      );
      return true;
    };
    try {
      if ((box === "inbox" || box === "split") && trimmed) {
        const searchRequest = {
          query: trimmed,
          limit: SEARCH_PAGE_SIZE,
          includeArchived,
        };
        const localThreads = await mailClient.searchThreads(searchRequest, accountId);
        if (!commitPage({ threads: localThreads, hasMore: localThreads.length === SEARCH_PAGE_SIZE })) return;

        const remoteSearchKey = `${accountId ?? "all"}\u0000${trimmed}`;
        if (includeArchived && remoteSearchKeyRef.current !== remoteSearchKey) {
          remoteSearchKeyRef.current = remoteSearchKey;
          const remoteRequestId = ++remoteSearchRequestRef.current;
          remoteSearchInFlightRef.current = true;
          setRemoteSearchState("searching");
          const backfillPromise = mailClient.backfillSearchThreads(trimmed, accountId);
          remoteSearchPromiseRef.current = backfillPromise;
          const pollForRemoteMatches = () => {
            if (remoteRequestId !== remoteSearchRequestRef.current) return;
            void mailClient.searchThreads(searchRequest, accountId).then((polled) => {
              if (remoteRequestId !== remoteSearchRequestRef.current || requestId !== threadsRequest.current) return;
              commitPage({ threads: polled, hasMore: polled.length === SEARCH_PAGE_SIZE });
            });
          };
          const pollInterval = window.setInterval(pollForRemoteMatches, REMOTE_SEARCH_POLL_MS);
          void backfillPromise
            .then(async () => {
              window.clearInterval(pollInterval);
              if (remoteRequestId !== remoteSearchRequestRef.current) return;
              const refreshed = await mailClient.searchThreads(searchRequest, accountId);
              if (remoteRequestId !== remoteSearchRequestRef.current) return;
              remoteSearchInFlightRef.current = false;
              if (requestId === threadsRequest.current) {
                commitPage({ threads: refreshed, hasMore: refreshed.length === SEARCH_PAGE_SIZE });
                setRemoteSearchState("idle");
              }
            })
            .catch(() => {
              window.clearInterval(pollInterval);
              // Gmail-backed search is an enhancement to the local result,
              // not a reason to make search fail while offline.
              if (remoteSearchKeyRef.current === remoteSearchKey && remoteRequestId === remoteSearchRequestRef.current) {
                remoteSearchInFlightRef.current = false;
                remoteSearchKeyRef.current = null;
                setRemoteSearchState("error");
              }
            });
        } else if (includeArchived && remoteSearchKeyRef.current === remoteSearchKey && remoteSearchInFlightRef.current) {
          const remoteRequestId = remoteSearchRequestRef.current;
          const remotePromise = remoteSearchPromiseRef.current;
          if (remotePromise) {
            void remotePromise.then(async () => {
              if (requestId !== threadsRequest.current || remoteRequestId !== remoteSearchRequestRef.current) return;
              const refreshed = await mailClient.searchThreads(searchRequest, accountId);
              if (requestId !== threadsRequest.current || remoteRequestId !== remoteSearchRequestRef.current) return;
              commitPage({ threads: refreshed, hasMore: refreshed.length === SEARCH_PAGE_SIZE });
              remoteSearchInFlightRef.current = false;
              setRemoteSearchState("idle");
            }).catch(logBackgroundFailure("Remote search refresh"));
          }
        }
        return;
      }

      const page = await fetchMailboxPage(box, accountId, 0);
      commitPage(page);
    } catch (error) {
      if (requestId !== threadsRequest.current) return;
      setMailboxError(errorMessage(error));
    }
  }, [
    mailbox, activeSplitInboxId, activeAccountId, includeArchived, setSelectedId,
    refreshUnreadCounts, refreshMailboxUnreadCounts, fetchMailboxPage,
  ]);

  // Sync round trips can outlive a mailbox/split-inbox switch. Reading
  // loadThreads through a ref at resolution time (rather than closing over
  // whichever instance existed when the sync started) keeps a slow sync from
  // repainting the thread list for a view the user has since navigated away
  // from, even though the header already reflects the new view.
  const loadThreadsRef = useRef(loadThreads);
  loadThreadsRef.current = loadThreads;
  // Background accounts keep polling Gmail while a different account is
  // active in the UI; without this, their sidebar badge only catches up to
  // what the backend already knows the next time the mailbox reloads (e.g.
  // switching into that account). The active account's Inbox badge and
  // visible thread list need the same treatment, or a long-idle-but-focused
  // session can silently accumulate unread mail with no on-screen change
  // until some unrelated action (e.g. sending mail) happens to reload the
  // mailbox.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const unlisten = listen<string>("unread-counts-changed", (event) => {
      refreshUnreadCounts();
      refreshMailboxUnreadCounts();
      // A null activeAccountId is the merged "All accounts" view, which
      // shows every account's mail, so any account's sync should refresh it.
      if (activeAccountId === null || event.payload === activeAccountId) {
        void loadThreadsRef.current(queryRef.current);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refreshUnreadCounts, refreshMailboxUnreadCounts, activeAccountId]);

  const loadMoreResults = useCallback(async () => {
    const trimmed = query.trim();
    if ((mailbox === "drafts" || mailbox === "outbox") || loadingMore.current) return;
    if (mailbox === "split" && !activeSplitInboxId) return;
    loadingMore.current = true;
    setLoadingMoreState(true);
    const requestId = threadsRequest.current;
    try {
      const accountId = activeAccountId ?? undefined;
      const page = trimmed
        ? await mailClient.searchThreads({
            query: trimmed,
            limit: SEARCH_PAGE_SIZE,
            offset: threads.length,
            includeArchived,
          }, accountId).then((items) => ({ threads: items, hasMore: items.length === SEARCH_PAGE_SIZE }))
        : await fetchMailboxPage(mailbox, accountId, threads.length);
      if (requestId !== threadsRequest.current) return;
      setThreads((current) => [...current, ...page.threads]);
      setHasMoreResults(page.hasMore);
    } catch (error) {
      if (requestId === threadsRequest.current) {
        setMailboxError(errorMessage(error));
      }
    } finally {
      loadingMore.current = false;
      setLoadingMoreState(false);
    }
  }, [query, threads.length, includeArchived, activeAccountId, mailbox, activeSplitInboxId, fetchMailboxPage]);

  useEffect(() => {
    // Invalidate an in-flight request as soon as the view inputs change. The
    // debounce below is intentionally only for starting the replacement
    // request; it must not leave an older search eligible to paint.
    ++threadsRequest.current;
    // SearchField already debounces typing before committing `query`.
    const timeout = window.setTimeout(() => {
      void loadThreads(query).finally(() => setLoading(false));
    }, 0);
    return () => window.clearTimeout(timeout);
  }, [query, loadThreads]);

  useEffect(() => {
    // The toggle itself is only reachable while searching; reset it with the
    // search box so a stale "include archived" flag can't linger over into
    // the plain inbox view, where the backend always excludes archived mail.
    if (!query.trim()) setIncludeArchived(false);
  }, [query]);

  useEffect(() => {
    if (!includeArchived) {
      remoteSearchKeyRef.current = null;
      remoteSearchRequestRef.current += 1;
      remoteSearchPromiseRef.current = null;
      remoteSearchInFlightRef.current = false;
      setRemoteSearchState("idle");
    }
  }, [includeArchived]);

  return {
    threads, setThreads, query, setQuery, queryRef, searchOpen, setSearchOpen, includeArchived,
    setIncludeArchived, remoteSearchState, hasMoreResults, loading, mailboxError, loadingMoreState,
    loadThreads, loadThreadsRef, loadMoreResults, contextOpenedThreadRef,
  };
}
